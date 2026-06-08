---
name: xauusd-instrument-context
description: "[DEPRECATED] Long-range XAUUSD context is no longer available. The branch rebuilt xauusd.duckdb to a shallow rolling window (~weeks); there is no deep daily/weekly/monthly history (1998+), so YTD/ATH/seasonality/10-yr returns cannot be computed. Kept as a stub. Use /xauusd-snapshot, /xauusd-narrative-window, /xauusd-event-study, or /xauusd-trade-idea instead."
---

# XAUUSD Instrument Context — DEPRECATED

This skill is **deprecated and non-functional**. It required the full daily
history back to ~1998 to report YTD return, distance from all-time high,
1Y/5Y/10Y range positions, annual returns, and monthly seasonality.

That deep history **no longer exists anywhere**. On 2026-05-27 the database was
rebuilt (`Bots_db/rebuild_xauusd.sql`) into a lean `xauusd.duckdb` that keeps
only a **shallow rolling window** of candles — `xauusd_d1` currently holds
~35–50 days, refreshed live by the running app. None of the long-range anchors
this skill computed can be produced from that.

**Do not run the old queries** — `xauusd_d1` has only weeks of data, so every
"10-year" / "since 1998" / "distance from ATH" figure would be wrong, not just
missing.

## Use these instead

- **`/xauusd-snapshot`** — current price, ATR(14,H1), 24h range, today's EC
  events, today's gold headlines. The fast factual dump.
- **`/xauusd-narrative-window`** — the dominant multi-day theme from
  `news_historical` + recent EC surprises + recent D1 action.
- **`/xauusd-event-study`** — how gold actually reacted to recent vol≥2 events.
- **`/xauusd-trade-idea`** — opinionated bias/setup from the `xauusd-trader`
  subagent.

If deep history is ever reloaded into a daily table, restore this skill from
git history (the original long-range queries are preserved there).
