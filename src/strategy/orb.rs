//! London opening-range breakout (ORB) — the EURUSD bot's mechanical entry rule.
//!
//! This module is PURE and self-contained (std only, no `crate::` refs, no I/O)
//! so the SAME file is the single source of truth for both the backtester and
//! the live bot. Session / DST / calendar handling lives in the *caller*; this
//! module only turns a set of opening-range bars into a concrete trade plan.
//!
//! The caller's job:
//!   1. Resolve the DST-aware session anchor for the date.
//!   2. Slice the bars covering `[anchor, anchor + range_minutes)`.
//!   3. Call [`build_plan`]; if `Some`, arm both triggers (OCO) and run the
//!      fill / management loop (in the simulator or the live bot).

/// One OHLCV bar. `ts` is the bar's open time as a UTC unix timestamp (seconds);
/// `v` is the tick-count volume proxy cTrader uses for FX/CFD trendbars.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bar {
    pub ts: i64,
    pub o: f64,
    pub h: f64,
    pub l: f64,
    pub c: f64,
    pub v: i64,
}

/// Trade direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Long,
    Short,
}

/// How the protective stop is placed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StopMode {
    /// Stop at the opposite edge of the opening range.
    RangeEdge,
    /// Stop at `mult × ATR` from the entry trigger (ATR passed to [`build_plan`]).
    Atr(f64),
}

/// Strategy parameters. Pip-denominated fields keep the rule instrument-agnostic
/// while the bot targets EURUSD (`pip_size` = 0.0001, `pip_position` = 4).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrbConfig {
    /// Length of the opening range in minutes (e.g. 30 → 07:00–07:30).
    pub range_minutes: i64,
    /// Trigger offset beyond the range edge, in pips (wick-noise guard).
    pub buffer_pips: f64,
    /// Skip the day if the range is narrower than this (no volatility).
    pub min_range_pips: f64,
    /// Skip the day if the range is wider than this (move already happened).
    pub max_range_pips: f64,
    /// Stop placement method.
    pub stop: StopMode,
    /// Take-profit distance as a multiple of risk (R).
    pub target_r: f64,
    /// Move the stop to breakeven once price reaches this R. Used by the
    /// fill/management layer, not by [`build_plan`] itself.
    pub breakeven_at_r: f64,
    /// Price increment of one pip (0.0001 for EURUSD).
    pub pip_size: f64,
}

impl Default for OrbConfig {
    /// Baseline params pre-registered for the first backtest pass.
    fn default() -> Self {
        Self {
            range_minutes: 30,
            buffer_pips: 1.5,
            min_range_pips: 6.0,
            max_range_pips: 40.0,
            stop: StopMode::RangeEdge,
            target_r: 1.5,
            breakeven_at_r: 1.0,
            pip_size: 0.0001,
        }
    }
}

/// A concrete, ready-to-arm plan for one session. Both directions are pre-armed
/// (OCO): whichever trigger fills first cancels the other.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrbPlan {
    pub orh: f64,
    pub orl: f64,
    pub range_pips: f64,
    pub buy_trigger: f64,
    pub sell_trigger: f64,
    pub long_stop: f64,
    pub long_target: f64,
    pub short_stop: f64,
    pub short_target: f64,
}

impl OrbPlan {
    /// Entry trigger for a side (convenience for the fill loop).
    pub fn trigger_for(&self, side: Side) -> f64 {
        match side {
            Side::Long => self.buy_trigger,
            Side::Short => self.sell_trigger,
        }
    }
    /// Protective stop for a filled side.
    pub fn stop_for(&self, side: Side) -> f64 {
        match side {
            Side::Long => self.long_stop,
            Side::Short => self.short_stop,
        }
    }
    /// Take-profit target for a filled side.
    pub fn target_for(&self, side: Side) -> f64 {
        match side {
            Side::Long => self.long_target,
            Side::Short => self.short_target,
        }
    }
    /// Risk distance (entry → stop) in price for a filled side. Always positive.
    pub fn risk_for(&self, side: Side) -> f64 {
        (self.trigger_for(side) - self.stop_for(side)).abs()
    }
}

/// High / low of the opening-range bars. `None` if empty or degenerate.
fn range_high_low(bars: &[Bar]) -> Option<(f64, f64)> {
    if bars.is_empty() {
        return None;
    }
    let mut hi = f64::MIN;
    let mut lo = f64::MAX;
    for b in bars {
        if b.h > hi {
            hi = b.h;
        }
        if b.l < lo {
            lo = b.l;
        }
    }
    if !(hi > lo) {
        return None;
    }
    Some((hi, lo))
}

/// Build the day's trade plan from the opening-range bars.
///
/// `atr_pips` is required when `cfg.stop` is [`StopMode::Atr`] (ignored for
/// [`StopMode::RangeEdge`]); pass the current ATR in pips. Returns `None` —
/// meaning "no trade today" — when the range fails the width filters, the ATR
/// stop is requested without an ATR value, or the inputs are degenerate.
pub fn build_plan(range_bars: &[Bar], atr_pips: Option<f64>, cfg: &OrbConfig) -> Option<OrbPlan> {
    let (orh, orl) = range_high_low(range_bars)?;
    let pip = cfg.pip_size;
    if pip <= 0.0 {
        return None;
    }

    let range_pips = (orh - orl) / pip;
    if range_pips < cfg.min_range_pips || range_pips > cfg.max_range_pips {
        return None; // quality filter: too quiet or already moved
    }

    let buf = cfg.buffer_pips * pip;
    let buy_trigger = orh + buf;
    let sell_trigger = orl - buf;

    let (long_stop, short_stop) = match cfg.stop {
        StopMode::RangeEdge => (orl, orh),
        StopMode::Atr(mult) => {
            let dist = atr_pips? * pip * mult; // None ⇒ can't size ⇒ no trade
            (buy_trigger - dist, sell_trigger + dist)
        }
    };

    let long_risk = buy_trigger - long_stop;
    let short_risk = short_stop - sell_trigger;
    if long_risk <= 0.0 || short_risk <= 0.0 {
        return None;
    }

    Some(OrbPlan {
        orh,
        orl,
        range_pips,
        buy_trigger,
        sell_trigger,
        long_stop,
        long_target: buy_trigger + cfg.target_r * long_risk,
        short_stop,
        short_target: sell_trigger - cfg.target_r * short_risk,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(ts: i64, h: f64, l: f64) -> Bar {
        Bar { ts, o: (h + l) / 2.0, h, l, c: (h + l) / 2.0, v: 1 }
    }

    /// Default config but with the width filter opened up, for tests that aren't
    /// exercising the filter itself.
    fn open_cfg() -> OrbConfig {
        OrbConfig { min_range_pips: 0.0, max_range_pips: 1.0e6, ..OrbConfig::default() }
    }

    #[test]
    fn computes_range_triggers_and_target() {
        // Opening range: high 1.1020, low 1.1000 → 20 pips.
        let bars = [bar(0, 1.1010, 1.1000), bar(60, 1.1020, 1.1005)];
        let p = build_plan(&bars, None, &open_cfg()).unwrap();

        assert!((p.orh - 1.1020).abs() < 1e-9);
        assert!((p.orl - 1.1000).abs() < 1e-9);
        assert!((p.range_pips - 20.0).abs() < 1e-6);

        // Triggers sit 1.5 pips beyond each edge.
        assert!((p.buy_trigger - 1.10215).abs() < 1e-9);
        assert!((p.sell_trigger - 1.09985).abs() < 1e-9);

        // RangeEdge stop = opposite edge; target = entry + 1.5R.
        assert!((p.long_stop - 1.1000).abs() < 1e-9);
        let long_risk = p.buy_trigger - p.long_stop;
        assert!((p.long_target - (p.buy_trigger + 1.5 * long_risk)).abs() < 1e-9);
        let short_risk = p.short_stop - p.sell_trigger;
        assert!((p.short_target - (p.sell_trigger - 1.5 * short_risk)).abs() < 1e-9);
    }

    #[test]
    fn rejects_too_narrow_range() {
        let bars = [bar(0, 1.10005, 1.10000)]; // 0.5 pip
        let cfg = OrbConfig { min_range_pips: 6.0, ..open_cfg() };
        assert!(build_plan(&bars, None, &cfg).is_none());
    }

    #[test]
    fn rejects_too_wide_range() {
        let bars = [bar(0, 1.1100, 1.1000)]; // 100 pips
        let cfg = OrbConfig { max_range_pips: 40.0, ..open_cfg() };
        assert!(build_plan(&bars, None, &cfg).is_none());
    }

    #[test]
    fn atr_stop_uses_atr_distance() {
        let bars = [bar(0, 1.1020, 1.1000)];
        let cfg = OrbConfig { stop: StopMode::Atr(1.0), ..open_cfg() };
        // ATR 10 pips × mult 1.0 → 10-pip stop from the trigger.
        let p = build_plan(&bars, Some(10.0), &cfg).unwrap();
        let dist = p.buy_trigger - p.long_stop;
        assert!((dist - 10.0 * cfg.pip_size).abs() < 1e-9);
        assert!((p.short_stop - p.sell_trigger - 10.0 * cfg.pip_size).abs() < 1e-9);
    }

    #[test]
    fn atr_mode_without_atr_is_no_trade() {
        let bars = [bar(0, 1.1020, 1.1000)];
        let cfg = OrbConfig { stop: StopMode::Atr(1.0), ..open_cfg() };
        assert!(build_plan(&bars, None, &cfg).is_none());
    }

    #[test]
    fn empty_or_degenerate_is_no_trade() {
        assert!(build_plan(&[], None, &open_cfg()).is_none());
        // flat bar (high == low) → degenerate range
        assert!(build_plan(&[bar(0, 1.1000, 1.1000)], None, &open_cfg()).is_none());
    }

    #[test]
    fn side_helpers_match_fields() {
        let bars = [bar(0, 1.1020, 1.1000)];
        let p = build_plan(&bars, None, &open_cfg()).unwrap();
        assert_eq!(p.trigger_for(Side::Long), p.buy_trigger);
        assert_eq!(p.trigger_for(Side::Short), p.sell_trigger);
        assert_eq!(p.stop_for(Side::Short), p.short_stop);
        assert_eq!(p.target_for(Side::Long), p.long_target);
        assert!((p.risk_for(Side::Long) - (p.buy_trigger - p.long_stop)).abs() < 1e-12);
    }
}
