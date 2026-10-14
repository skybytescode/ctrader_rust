//! Risk rules enforced in the backend, where a reload of the UI or a restart
//! of the app cannot bypass them.
//!
//! - Every order needs a stop-loss on the correct side of its entry.
//! - Automated orders (the gold loop and the EURUSD bot) stop for the rest of
//!   the UTC day once the balance has fallen `RISK_DAILY_LOSS_PCT` (default 5)
//!   below the day's opening balance, or after `RISK_MAX_AUTO_TRADES`
//!   (default 10) automated orders. The day's anchor is saved to disk, so a
//!   restart after losses doesn't re-anchor at the lower balance.

use serde::{Deserialize, Serialize};
use std::sync::Mutex;

const STATE_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/Bots_db/risk_state.json");

/// The stop-loss is required; the take-profit is dropped when it is on the
/// wrong side, as before.
pub fn protective_levels(
    is_buy: bool,
    entry: f64,
    stop: Option<f64>,
    target: Option<f64>,
) -> Result<(f64, Option<f64>), String> {
    let Some(sl) = stop.filter(|s| s.is_finite()) else {
        return Err("refusing an order without a stop-loss".into());
    };
    let sl_ok = if is_buy { sl < entry } else { sl > entry };
    if !sl_ok {
        return Err(format!(
            "refusing a {} order: stop-loss {sl} is not {} the entry {entry}",
            if is_buy { "BUY" } else { "SELL" },
            if is_buy { "below" } else { "above" },
        ));
    }
    let tp = target.filter(|&t| t.is_finite() && if is_buy { t > entry } else { t < entry });
    Ok((sl, tp))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    pub daily_loss_pct: f64,
    pub max_auto_trades: u32,
}

impl Limits {
    pub fn from_env() -> Self {
        let num = |name: &str, default: f64| {
            std::env::var(name).ok().and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| *v > 0.0).unwrap_or(default)
        };
        Limits {
            daily_loss_pct: num("RISK_DAILY_LOSS_PCT", 5.0),
            max_auto_trades: num("RISK_MAX_AUTO_TRADES", 10.0) as u32,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DayState {
    /// UTC date, YYYY-MM-DD.
    pub day: String,
    /// Balance when the day's first check ran.
    pub anchor_balance: f64,
    pub auto_trades: u32,
    /// Why automated trading stopped for the day, if it did.
    pub halted: Option<String>,
}

impl DayState {
    /// Whether an automated order may be sent now. Starts a new day when the
    /// date changes and latches the halt once a limit is hit.
    pub fn check_auto_order(&mut self, today: &str, balance: f64, limits: Limits) -> Result<(), String> {
        if balance <= 0.0 || !balance.is_finite() {
            return Err("account balance not known yet — automated orders wait for it".into());
        }
        if self.day != today {
            *self = DayState { day: today.to_string(), anchor_balance: balance, ..DayState::default() };
        }
        if let Some(why) = &self.halted {
            return Err(format!("automated trading halted for today: {why}"));
        }
        let floor = self.anchor_balance * (1.0 - limits.daily_loss_pct / 100.0);
        if balance <= floor {
            let why = format!(
                "daily loss limit hit (balance {balance:.2} ≤ {floor:.2}, −{}% of {:.2})",
                limits.daily_loss_pct, self.anchor_balance
            );
            self.halted = Some(why.clone());
            return Err(format!("automated trading halted for today: {why}"));
        }
        if self.auto_trades >= limits.max_auto_trades {
            let why = format!("{} automated orders already placed today", self.auto_trades);
            self.halted = Some(why.clone());
            return Err(format!("automated trading halted for today: {why}"));
        }
        Ok(())
    }
}

static STATE: Mutex<Option<DayState>> = Mutex::new(None);

fn with_state<T>(f: impl FnOnce(&mut DayState) -> T) -> T {
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let state = guard.get_or_insert_with(|| {
        std::fs::read_to_string(STATE_PATH).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    });
    let out = f(state);
    if let Ok(json) = serde_json::to_string_pretty(state) {
        if let Err(e) = std::fs::write(STATE_PATH, json) {
            eprintln!("[risk] could not save {STATE_PATH}: {e}");
        }
    }
    out
}

fn today_utc() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

/// Gate for automated orders; call right before sending one.
pub fn check_auto_order(balance: f64) -> Result<(), String> {
    let limits = Limits::from_env();
    with_state(|s| s.check_auto_order(&today_utc(), balance, limits))
}

/// Count an automated order that the broker accepted.
pub fn record_auto_order() {
    with_state(|s| s.auto_trades += 1);
}

/// Snapshot for the UI.
pub fn status() -> (DayState, Limits) {
    (with_state(|s| s.clone()), Limits::from_env())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: Limits = Limits { daily_loss_pct: 5.0, max_auto_trades: 3 };

    #[test]
    fn stop_loss_is_required_and_must_be_on_the_right_side() {
        assert_eq!(protective_levels(true, 2650.0, Some(2640.0), Some(2670.0)), Ok((2640.0, Some(2670.0))));
        assert_eq!(protective_levels(false, 1.1000, Some(1.1010), Some(1.0980)), Ok((1.1010, Some(1.0980))));
        assert!(protective_levels(true, 2650.0, None, Some(2670.0)).is_err());
        assert!(protective_levels(true, 2650.0, Some(2655.0), None).is_err(), "buy stop above entry");
        assert!(protective_levels(false, 1.1, Some(1.09), None).is_err(), "sell stop below entry");
        assert!(protective_levels(true, 2650.0, Some(f64::NAN), None).is_err());
    }

    #[test]
    fn wrong_side_take_profit_is_dropped() {
        assert_eq!(protective_levels(true, 2650.0, Some(2640.0), Some(2600.0)), Ok((2640.0, None)));
    }

    #[test]
    fn closed_losses_count_towards_the_daily_limit() {
        let mut s = DayState::default();
        assert!(s.check_auto_order("2026-10-14", 10_000.0, LIMITS).is_ok());
        assert_eq!(s.anchor_balance, 10_000.0);
        // Losses realised on closed trades lower the balance.
        assert!(s.check_auto_order("2026-10-14", 9_600.0, LIMITS).is_ok());
        let err = s.check_auto_order("2026-10-14", 9_500.0, LIMITS).unwrap_err();
        assert!(err.contains("daily loss limit"), "{err}");
        // The halt latches even if the balance recovers.
        assert!(s.check_auto_order("2026-10-14", 9_900.0, LIMITS).is_err());
    }

    #[test]
    fn a_restart_keeps_the_days_anchor() {
        let mut s = DayState::default();
        s.check_auto_order("2026-10-14", 10_000.0, LIMITS).unwrap();
        let saved = serde_json::to_string(&s).unwrap();
        let mut restored: DayState = serde_json::from_str(&saved).unwrap();
        assert!(restored.check_auto_order("2026-10-14", 9_400.0, LIMITS).is_err());
    }

    #[test]
    fn new_day_resets_the_anchor_count_and_halt() {
        let mut s = DayState { day: "2026-10-13".into(), anchor_balance: 10_000.0, auto_trades: 3, halted: Some("x".into()) };
        assert!(s.check_auto_order("2026-10-14", 9_000.0, LIMITS).is_ok());
        assert_eq!((s.anchor_balance, s.auto_trades, s.halted.clone()), (9_000.0, 0, None));
    }

    #[test]
    fn automated_orders_per_day_are_capped() {
        let mut s = DayState::default();
        for _ in 0..3 {
            s.check_auto_order("2026-10-14", 10_000.0, LIMITS).unwrap();
            s.auto_trades += 1;
        }
        assert!(s.check_auto_order("2026-10-14", 10_000.0, LIMITS).unwrap_err().contains("3 automated orders"));
    }

    #[test]
    fn unknown_balance_blocks_automated_orders() {
        assert!(DayState::default().check_auto_order("2026-10-14", 0.0, LIMITS).is_err());
    }
}
