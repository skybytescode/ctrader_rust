//! Trading-strategy rules, shared verbatim by the backtester and the live bot.
//!
//! Modules here are intentionally PURE (std only, no I/O, no `crate::` references)
//! so the exact same source file is the single source of truth for both
//! consumers — the backtest harness (`backtest::sim`) and the live bot
//! (`bots::eurusd_orb`). This guarantees the live bot trades exactly what was
//! validated, eliminating the classic "backtest and live silently diverge" bug.

pub mod orb;
