---
name: xauusd-snapshot
description: Quick XAUUSD market snapshot — current price, recent volatility, today's economic calendar, latest gold-relevant news headlines. Fast, no LLM reasoning. Run before deciding whether to ask for a full trade idea.
---

# XAUUSD Snapshot

Pull a compact one-screen view of the gold market right now. Run these queries and present the output as a single Markdown report — no analysis, no recommendation, just facts.

> **Data note (verified):** all queries below hit live tables in
> `Bots_db/xauusd.duckdb` that the running app keeps fresh — `xauusd_m1`,
> `xauusd_h1` (shallow rolling window of ~days), `xauusd_economic_calendar`
> (today-centric), and `news_historical`. This is correct for a *current*
> snapshot. Two caveats: if a table errors as "not found", the app hasn't
> populated it yet this session (e.g. `xauusd_economic_calendar` only appears
> after the first EC fetch) — note it as "no data yet" rather than failing.
> The candle tables are shallow, so don't reach further back than ~a day or two.

For deeper views, run `/xauusd-narrative-window` (multi-day theme tracker), `/xauusd-event-study` (recent event reactions), `/xauusd-instrument-context` (long-range structure + seasonality), or `/xauusd-trade-idea` (full bias from the xauusd-trader subagent).

## 1. Current price + last update

```bash
/c/duckdb_cli-windows-amd64/duckdb d:/MyProjects/ctrader_rust/Bots_db/xauusd.duckdb -c "
SELECT
  to_timestamp(timestamp) AS ts_utc,
  open, high, low, close, volume
FROM xauusd_m1
ORDER BY timestamp DESC
LIMIT 1;
"
```

Report: current close, exact UTC timestamp, and **flag if older than 15 minutes** (markets may be closed).

## 2. Recent range + ATR(14, H1)

```bash
/c/duckdb_cli-windows-amd64/duckdb d:/MyProjects/ctrader_rust/Bots_db/xauusd.duckdb -c "
WITH last14 AS (
  SELECT high - low AS range
  FROM xauusd_h1
  ORDER BY timestamp DESC
  LIMIT 14
)
SELECT
  ROUND(AVG(range), 2) AS atr_14_h1_usd,
  ROUND(MAX(range), 2) AS max_range_last_14h,
  ROUND(MIN(range), 2) AS min_range_last_14h
FROM last14;
"
```

```bash
/c/duckdb_cli-windows-amd64/duckdb d:/MyProjects/ctrader_rust/Bots_db/xauusd.duckdb -c "
WITH last24 AS (SELECT high, low FROM xauusd_h1 ORDER BY timestamp DESC LIMIT 24)
SELECT
  ROUND(MAX(high), 2) AS h24_high,
  ROUND(MIN(low),  2) AS h24_low,
  ROUND(MAX(high) - MIN(low), 2) AS h24_range
FROM last24;
"
```

Report: ATR(14, H1) in USD, 24h high/low/range.

## 3. Today's + next 24h EC events (vol ≥ 2)

Compute today's UTC date and tomorrow's UTC date first, then:

```bash
/c/duckdb_cli-windows-amd64/duckdb d:/MyProjects/ctrader_rust/Bots_db/xauusd.duckdb -c "
SELECT
  timestamp_utc, currency, volatility, event_name,
  actual, forecast, previous
FROM xauusd_economic_calendar
WHERE timestamp_utc >= '<today_iso>'
  AND timestamp_utc <  '<day_after_tomorrow_iso>'
  AND volatility >= 2
ORDER BY timestamp_utc ASC;
"
```

Show as a small table. **Bold any Fed/Powell/CPI/PCE/NFP/FOMC events** — those are gold-critical.

## 4. Today's gold-relevant news headlines

```bash
/c/duckdb_cli-windows-amd64/duckdb d:/MyProjects/ctrader_rust/Bots_db/xauusd.duckdb -c "
SELECT published_utc, title
FROM news_historical
WHERE published_utc LIKE '<today_yyyy_mm_dd>%'
  AND (lower(title) LIKE '%gold%' OR lower(title) LIKE '%xau%'
    OR lower(title) LIKE '%fed%' OR lower(title) LIKE '%powell%'
    OR lower(title) LIKE '%dxy%' OR lower(title) LIKE '%dollar%'
    OR lower(title) LIKE '%yield%' OR lower(title) LIKE '%real%')
ORDER BY published_utc DESC
LIMIT 15;
"
```

List time + title. No bodies.

## Output template

Produce a single Markdown report shaped like:

```markdown
# XAUUSD Snapshot — {YYYY-MM-DD HH:MM UTC}

**Price:** {close} (M1 ts {ts_utc} — {fresh|stale Δmin ago})
**ATR(14, H1):** {x.x} USD
**24h range:** {low} – {high} ({range} USD wide)

## EC events next 24h (vol ≥ 2)

| Time UTC | Cur | Vol | Event | Fcst | Prev | Actual |
|---|---|---|---|---|---|---|
| ... |

## Today's gold-relevant headlines

- {HH:MM UTC} — {title}
- ...
```

Stop there. **No analysis, no bias, no recommendation.** This skill is purely a data dump — the user can decide whether to follow up with `/xauusd-trade-idea` for an opinionated view.
