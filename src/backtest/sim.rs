//! ORB backtest simulator — reads M1 bars, runs the shared `orb` rule day by
//! day, simulates fills with a realistic cost model, and reports metrics.
//!
//! Pessimistic by construction: when one M1 bar spans both stop and target, the
//! stop is assumed to fill first; stop fills (and the entry) pay slippage. The
//! account is modeled at a fixed 0.01 lot (the $40-account minimum) with the
//! real commission read off the symbol spec ($30 / 1M USD, per side).
//!
//! This module is backtest-only and references the bin-local `crate::orb`.

use crate::orb::{self, Bar, OrbConfig, OrbPlan, Side};
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc, Weekday};
use std::collections::BTreeMap;

const UNITS_PER_LOT: f64 = 100_000.0;

/// Account sizing + cost assumptions. Costs are in pips except commission, which
/// is the cTrader `USD_PER_MILLION_USD` rate charged **per side**.
#[derive(Clone, Copy, Debug)]
pub struct CostModel {
    pub spread_pips: f64,
    pub slippage_pips: f64,
    pub commission_per_m_usd_per_side: f64,
    pub lots: f64,
}

impl Default for CostModel {
    fn default() -> Self {
        Self { spread_pips: 0.2, slippage_pips: 0.3, commission_per_m_usd_per_side: 30.0, lots: 0.01 }
    }
}

/// Session timing (the caller-side part the pure `orb` rule deliberately omits).
#[derive(Clone, Copy, Debug)]
pub struct SessionConfig {
    /// Latest minutes-after-anchor a breakout may trigger (else "no trade").
    pub entry_window_min: i64,
    /// Force-flat at this UTC hour (before the 21:00 rollover).
    pub flat_hour_utc: u32,
    /// Skip entries that fire inside the approximate US/EU news window.
    pub blackout: bool,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self { entry_window_min: 240, flat_hour_utc: 20, blackout: true }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitReason { Target, Stop, FlatEod }

/// One simulated trade.
#[derive(Clone, Copy, Debug)]
pub struct TradeResult {
    pub date: NaiveDate,
    pub side: Side,
    pub entry: f64,
    pub exit: f64,
    pub stop: f64,
    pub target: f64,
    pub gross_pips: f64,
    pub net_pips: f64,
    pub r_multiple: f64,
    pub pnl_usd: f64,
    pub reason: ExitReason,
}

/// Per-day outcomes that did NOT produce a trade (for honest accounting).
#[derive(Default, Clone, Copy, Debug)]
pub struct DayCounts {
    pub filtered: u32,  // range failed the width filter / no session data
    pub no_trigger: u32, // armed but neither side triggered in the window
    pub blackout: u32,  // breakout fired inside the news window
    pub weekend: u32,   // skipped (Sat/Sun)
}

// ── UK daylight-saving (BST) ────────────────────────────────────────────────
// BST runs from the last Sunday of March to the last Sunday of October. London
// open is 08:00 local → 07:00 UTC under BST, 08:00 UTC otherwise. Day-granularity
// is fine here because we never trade the transition Sundays.

fn last_sunday(year: i32, month: u32) -> NaiveDate {
    let first_next = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    }
    .unwrap();
    let last = first_next - Duration::days(1);
    last - Duration::days(last.weekday().num_days_from_sunday() as i64)
}

fn london_in_bst(date: NaiveDate) -> bool {
    date >= last_sunday(date.year(), 3) && date < last_sunday(date.year(), 10)
}

/// UTC hour of the London open for a given date.
fn anchor_hour_utc(date: NaiveDate) -> u32 {
    if london_in_bst(date) { 7 } else { 8 }
}

/// Approximate tier-1 US/EU news window in UTC minutes-of-day (≈12:10–14:05),
/// covering the 12:30 US data and 14:00 releases. Crude v1 stand-in until the
/// historical EC calendar is wired in.
fn in_blackout(ts: i64) -> bool {
    let min = ts.rem_euclid(86_400) / 60;
    (730..=845).contains(&min)
}

fn pip_value_usd(cost: &CostModel, pip: f64) -> f64 {
    cost.lots * UNITS_PER_LOT * pip
}

/// Total round-trip cost of a trade in pips (spread once + entry slippage +
/// stop-exit slippage when stopped + commission both sides).
fn cost_pips(cost: &CostModel, pip: f64, entry: f64, stopped: bool) -> f64 {
    let mut c = cost.spread_pips + cost.slippage_pips;
    if stopped {
        c += cost.slippage_pips;
    }
    let notional_usd = cost.lots * UNITS_PER_LOT * entry;
    let commission_usd = 2.0 * cost.commission_per_m_usd_per_side * notional_usd / 1.0e6;
    c + commission_usd / pip_value_usd(cost, pip)
}

/// Walk bars from the entry bar to the flat time, returning the exit. Pessimistic
/// (stop checked before target); breakeven raises the stop once +`breakeven_at_r`.
fn simulate_exit(bars: &[Bar], entry_idx: usize, side: Side, entry: f64, plan: &OrbPlan, cfg: &OrbConfig, flat_ts: i64) -> (f64, ExitReason) {
    let risk = plan.risk_for(side);
    let target = plan.target_for(side);
    let mut stop = plan.stop_for(side);
    let be_r = cfg.breakeven_at_r;
    let mut be_done = be_r <= 0.0;
    let be_level = match side {
        Side::Long => entry + be_r * risk,
        Side::Short => entry - be_r * risk,
    };

    let mut last_close = entry;
    for b in &bars[entry_idx..] {
        if b.ts > flat_ts {
            break;
        }
        last_close = b.c;
        if !be_done {
            let reached = match side {
                Side::Long => b.h >= be_level,
                Side::Short => b.l <= be_level,
            };
            if reached {
                stop = entry;
                be_done = true;
            }
        }
        let (stop_hit, tgt_hit) = match side {
            Side::Long => (b.l <= stop, b.h >= target),
            Side::Short => (b.h >= stop, b.l <= target),
        };
        if stop_hit {
            return (stop, ExitReason::Stop); // pessimistic: stop before target
        }
        if tgt_hit {
            return (target, ExitReason::Target);
        }
    }
    (last_close, ExitReason::FlatEod)
}

/// Run the backtest over `bars` (ascending M1). Returns the trades plus the
/// no-trade day accounting.
pub fn run_backtest(bars: &[Bar], cfg: &OrbConfig, cost: &CostModel, session: &SessionConfig)
    -> (Vec<TradeResult>, DayCounts)
{
    // Group bars by UTC calendar day. The London session + hold (07/08:00 →
    // flat_hour) stays within one UTC date, so per-day vectors are sufficient.
    let mut by_day: BTreeMap<NaiveDate, Vec<Bar>> = BTreeMap::new();
    for b in bars {
        if let Some(dt) = DateTime::<Utc>::from_timestamp(b.ts, 0) {
            by_day.entry(dt.date_naive()).or_default().push(*b);
        }
    }

    let pip = cfg.pip_size;
    let mut trades = Vec::new();
    let mut counts = DayCounts::default();

    for (&date, day_bars) in &by_day {
        if matches!(date.weekday(), Weekday::Sat | Weekday::Sun) {
            counts.weekend += 1;
            continue;
        }
        let hour = anchor_hour_utc(date);
        let anchor = date.and_hms_opt(hour, 0, 0).unwrap().and_utc().timestamp();
        let range_end = anchor + cfg.range_minutes * 60;
        let entry_deadline = anchor + session.entry_window_min * 60;
        let flat_ts = date.and_hms_opt(session.flat_hour_utc, 0, 0).unwrap().and_utc().timestamp();

        let range_bars: Vec<Bar> = day_bars.iter().copied()
            .filter(|b| b.ts >= anchor && b.ts < range_end)
            .collect();
        let Some(plan) = orb::build_plan(&range_bars, None, cfg) else {
            counts.filtered += 1;
            continue;
        };

        // Find the first breakout in the entry window.
        let mut fill: Option<(usize, Side, f64)> = None;
        let mut blacked_out = false;
        for (i, b) in day_bars.iter().enumerate() {
            if b.ts < range_end {
                continue;
            }
            if b.ts > entry_deadline {
                break;
            }
            let buy_hit = b.h >= plan.buy_trigger;
            let sell_hit = b.l <= plan.sell_trigger;
            if !(buy_hit || sell_hit) {
                continue;
            }
            if session.blackout && in_blackout(b.ts) {
                blacked_out = true;
                break;
            }
            // If a single bar spans both triggers, taking the long stops out the
            // same bar (its low is below the long stop) — a conservative loss.
            let (side, entry) = if buy_hit {
                (Side::Long, plan.buy_trigger)
            } else {
                (Side::Short, plan.sell_trigger)
            };
            fill = Some((i, side, entry));
            break;
        }

        let Some((idx, side, entry)) = fill else {
            if blacked_out { counts.blackout += 1; } else { counts.no_trigger += 1; }
            continue;
        };

        let (exit, reason) = simulate_exit(day_bars, idx, side, entry, &plan, cfg, flat_ts);

        let dir = match side { Side::Long => 1.0, Side::Short => -1.0 };
        let gross_pips = dir * (exit - entry) / pip;
        let net_pips = gross_pips - cost_pips(cost, pip, entry, reason == ExitReason::Stop);
        let risk_pips = plan.risk_for(side) / pip;
        trades.push(TradeResult {
            date, side, entry, exit,
            stop: plan.stop_for(side),
            target: plan.target_for(side),
            gross_pips,
            net_pips,
            r_multiple: if risk_pips > 0.0 { net_pips / risk_pips } else { 0.0 },
            pnl_usd: net_pips * pip_value_usd(cost, pip),
            reason,
        });
    }

    (trades, counts)
}

// ── Metrics & reporting ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Metrics {
    pub n: usize,
    pub wins: usize,
    pub losses: usize,
    pub win_rate: f64,
    pub total_pnl_usd: f64,
    pub expectancy_usd: f64,
    pub expectancy_r: f64,
    pub avg_win_r: f64,
    pub avg_loss_r: f64,
    pub profit_factor: f64,
    pub max_dd_usd: f64,
    pub longest_loss_streak: usize,
    pub mean_r: f64,
    pub std_r: f64,
}

pub fn compute_metrics(trades: &[TradeResult]) -> Metrics {
    let n = trades.len();
    if n == 0 {
        return Metrics {
            n: 0, wins: 0, losses: 0, win_rate: 0.0, total_pnl_usd: 0.0, expectancy_usd: 0.0,
            expectancy_r: 0.0, avg_win_r: 0.0, avg_loss_r: 0.0, profit_factor: 0.0,
            max_dd_usd: 0.0, longest_loss_streak: 0, mean_r: 0.0, std_r: 0.0,
        };
    }
    let wins: Vec<&TradeResult> = trades.iter().filter(|t| t.pnl_usd > 0.0).collect();
    let losses: Vec<&TradeResult> = trades.iter().filter(|t| t.pnl_usd <= 0.0).collect();
    let total_pnl_usd: f64 = trades.iter().map(|t| t.pnl_usd).sum();
    let gross_profit: f64 = wins.iter().map(|t| t.pnl_usd).sum();
    let gross_loss: f64 = losses.iter().map(|t| -t.pnl_usd).sum();
    let mean_r = trades.iter().map(|t| t.r_multiple).sum::<f64>() / n as f64;
    let var_r = trades.iter().map(|t| (t.r_multiple - mean_r).powi(2)).sum::<f64>() / n as f64;

    // Max drawdown on the cumulative-$ equity curve.
    let (mut equity, mut peak, mut max_dd) = (0.0, 0.0, 0.0);
    let (mut streak, mut longest) = (0usize, 0usize);
    for t in trades {
        equity += t.pnl_usd;
        if equity > peak { peak = equity; }
        let dd = peak - equity;
        if dd > max_dd { max_dd = dd; }
        if t.pnl_usd <= 0.0 { streak += 1; longest = longest.max(streak); } else { streak = 0; }
    }

    let avg_win_r = if wins.is_empty() { 0.0 } else { wins.iter().map(|t| t.r_multiple).sum::<f64>() / wins.len() as f64 };
    let avg_loss_r = if losses.is_empty() { 0.0 } else { losses.iter().map(|t| t.r_multiple).sum::<f64>() / losses.len() as f64 };

    Metrics {
        n,
        wins: wins.len(),
        losses: losses.len(),
        win_rate: wins.len() as f64 / n as f64,
        total_pnl_usd,
        expectancy_usd: total_pnl_usd / n as f64,
        expectancy_r: mean_r,
        avg_win_r,
        avg_loss_r,
        profit_factor: if gross_loss > 0.0 { gross_profit / gross_loss } else { f64::INFINITY },
        max_dd_usd: max_dd,
        longest_loss_streak: longest,
        mean_r,
        std_r: var_r.sqrt(),
    }
}

/// Parse the downloader's CSV (`timestamp,open,high,low,close,volume`) into bars.
pub fn load_csv(path: &str) -> Result<Vec<Bar>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("read {}: {}", path, e))?;
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if i == 0 || line.is_empty() {
            continue; // header / blank
        }
        let f: Vec<&str> = line.split(',').collect();
        if f.len() < 6 {
            continue;
        }
        out.push(Bar {
            ts: f[0].parse().map_err(|_| format!("bad ts on line {}", i + 1))?,
            o: f[1].parse().map_err(|_| format!("bad open on line {}", i + 1))?,
            h: f[2].parse().map_err(|_| format!("bad high on line {}", i + 1))?,
            l: f[3].parse().map_err(|_| format!("bad low on line {}", i + 1))?,
            c: f[4].parse().map_err(|_| format!("bad close on line {}", i + 1))?,
            v: f[5].parse().map_err(|_| format!("bad volume on line {}", i + 1))?,
        });
    }
    out.sort_by_key(|b| b.ts);
    Ok(out)
}

/// Write the per-trade log to CSV.
pub fn write_trades_csv(trades: &[TradeResult], path: &str) -> Result<(), String> {
    use std::io::Write as _;
    let f = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut w = std::io::BufWriter::new(f);
    let mut cum = 0.0;
    writeln!(w, "date,side,entry,exit,stop,target,gross_pips,net_pips,r,pnl_usd,cum_usd,reason").map_err(|e| e.to_string())?;
    for t in trades {
        cum += t.pnl_usd;
        writeln!(w, "{},{:?},{:.5},{:.5},{:.5},{:.5},{:.1},{:.1},{:.2},{:.3},{:.3},{:?}",
            t.date, t.side, t.entry, t.exit, t.stop, t.target,
            t.gross_pips, t.net_pips, t.r_multiple, t.pnl_usd, cum, t.reason)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Print the human-readable summary.
pub fn print_summary(m: &Metrics, counts: &DayCounts, cfg: &OrbConfig, cost: &CostModel, session: &SessionConfig, span: (NaiveDate, NaiveDate)) {
    println!("\n========== EURUSD ORB backtest ==========");
    println!("span: {} → {}", span.0, span.1);
    println!("rule: range={}min buffer={}pip filter={}–{}pip stop={:?} target={}R BE@{}R",
        cfg.range_minutes, cfg.buffer_pips, cfg.min_range_pips, cfg.max_range_pips, cfg.stop, cfg.target_r, cfg.breakeven_at_r);
    println!("cost: spread={}pip slip={}pip commission={}USD/1M/side lots={}  | session: entry≤{}min flat@{}:00 blackout={}",
        cost.spread_pips, cost.slippage_pips, cost.commission_per_m_usd_per_side, cost.lots,
        session.entry_window_min, session.flat_hour_utc, session.blackout);
    println!("days: filtered={} no-trigger={} blackout={} weekend={}",
        counts.filtered, counts.no_trigger, counts.blackout, counts.weekend);
    println!("-----------------------------------------");
    println!("trades:        {}", m.n);
    println!("win rate:      {:.1}%  ({}W / {}L)", m.win_rate * 100.0, m.wins, m.losses);
    println!("expectancy:    {:+.3} R/trade   ({:+.3} USD/trade)", m.expectancy_r, m.expectancy_usd);
    println!("avg win/loss:  {:+.2}R / {:+.2}R", m.avg_win_r, m.avg_loss_r);
    println!("profit factor: {:.2}", m.profit_factor);
    println!("total P/L:     {:+.2} USD   (at {} lot)", m.total_pnl_usd, cost.lots);
    println!("max drawdown:  {:.2} USD", m.max_dd_usd);
    println!("loss streak:   {}", m.longest_loss_streak);
    println!("R mean/std:    {:+.3} / {:.3}   (per-trade Sharpe {:.3})",
        m.mean_r, m.std_r, if m.std_r > 0.0 { m.mean_r / m.std_r } else { 0.0 });
    println!("=========================================");
    let verdict = m.n >= 200 && m.expectancy_r > 0.0 && m.profit_factor > 1.3;
    println!("Phase-0 gate (≥200 trades, expectancy>0, PF>1.3): {}",
        if verdict { "PASS ✅" } else { "FAIL ❌ (revise rule / not yet validated)" });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_sunday_of_march_2026() {
        // 2026-03-29 is the last Sunday of March 2026.
        assert_eq!(last_sunday(2026, 3), NaiveDate::from_ymd_opt(2026, 3, 29).unwrap());
        assert_eq!(last_sunday(2026, 10), NaiveDate::from_ymd_opt(2026, 10, 25).unwrap());
    }

    #[test]
    fn bst_anchor_hours() {
        // Summer → BST → 07:00 UTC; winter → GMT → 08:00 UTC.
        assert_eq!(anchor_hour_utc(NaiveDate::from_ymd_opt(2026, 7, 1).unwrap()), 7);
        assert_eq!(anchor_hour_utc(NaiveDate::from_ymd_opt(2026, 1, 15).unwrap()), 8);
    }

    #[test]
    fn blackout_covers_us_window() {
        let day = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let at = |h, m| day.and_hms_opt(h, m, 0).unwrap().and_utc().timestamp();
        assert!(in_blackout(at(12, 30)));
        assert!(in_blackout(at(14, 0)));
        assert!(!in_blackout(at(9, 0)));
        assert!(!in_blackout(at(15, 0)));
    }

    /// A clean long breakout that runs to target yields a positive net result
    /// after costs, with the expected ~1.5R.
    #[test]
    fn long_breakout_hits_target_net_positive() {
        let cfg = OrbConfig { min_range_pips: 0.0, max_range_pips: 1e6, breakeven_at_r: 0.0, ..OrbConfig::default() };
        let cost = CostModel::default();
        let session = SessionConfig { blackout: false, ..SessionConfig::default() };
        // Winter date → anchor 08:00 UTC.
        let date = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
        let t = |h, m| date.and_hms_opt(h, m, 0).unwrap().and_utc().timestamp();
        let bar = |ts, h: f64, l: f64| Bar { ts, o: (h + l) / 2.0, h, l, c: (h + l) / 2.0, v: 1 };

        let mut bars = Vec::new();
        // Opening range 08:00–08:30: high 1.1020, low 1.1000 (20 pips).
        bars.push(bar(t(8, 0), 1.1010, 1.1000));
        bars.push(bar(t(8, 15), 1.1020, 1.1005));
        // 08:35 breaks up through buy_trigger 1.10215 and runs to target.
        // risk = 1.10215 - 1.1000 = 21.5 pips; target = +1.5R ≈ 1.13440.
        bars.push(bar(t(8, 35), 1.1300, 1.1021));
        bars.push(bar(t(8, 36), 1.1350, 1.1340)); // hits target 1.1344

        let (trades, _) = run_backtest(&bars, &cfg, &cost, &session);
        assert_eq!(trades.len(), 1);
        let tr = &trades[0];
        assert_eq!(tr.side, Side::Long);
        assert_eq!(tr.reason, ExitReason::Target);
        assert!(tr.pnl_usd > 0.0, "pnl {}", tr.pnl_usd);
        assert!((tr.r_multiple - 1.5).abs() < 0.2, "r {}", tr.r_multiple);
    }
}
