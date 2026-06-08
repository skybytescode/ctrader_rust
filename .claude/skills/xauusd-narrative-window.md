---
name: xauusd-narrative-window
description: Multi-day narrative summary for XAUUSD (gold). Reads the last 5 trading days of news bodies + EC events + daily price action and produces a deterministic theme-tracking report — dominant story, trajectory, confounders. No bias, no trade recommendation, just the narrative. Pair with /xauusd-event-study and /xauusd-trade-idea for a full pre-trade picture.
---

# XAUUSD Narrative Window (last 5 days)

Produce a single Markdown report that captures the **dominant story driving gold over the last 5 trading days**. Pure narrative — no bias, no levels, no recommendation.

Default lookback: **5 calendar days** (use `5` unless the user specifies a different `--days` argument).

> **Data note (verified):** the per-day JSON archives under `news_data/` and
> `ec_events_data/` are **abandoned and stale** (they stopped updating) — do
> **not** query them with `read_json_auto`. The live source of truth is the
> DuckDB at `Bots_db/xauusd.duckdb`: `news_historical` for articles,
> `xauusd_economic_calendar` for events, and the `xauusd_*` candle tables for
> price. Those tables hold only a **shallow rolling window** (~weeks), kept
> fresh by the running app — plenty for a 5-day narrative window.

## Step 1 — Define the window

Compute:
- `today_utc` = current UTC date as `YYYY-MM-DD`
- `start_utc` = today − N days as `YYYY-MM-DD`

## Step 2 — Pull gold-relevant headlines from the window

```bash
/c/duckdb_cli-windows-amd64/duckdb d:/MyProjects/ctrader_rust/Bots_db/xauusd.duckdb -c "
SELECT published_utc, title, article_id
FROM news_historical
WHERE published_utc >= '<start_utc>'
  AND (lower(title) LIKE '%gold%' OR lower(title) LIKE '%xau%'
    OR lower(title) LIKE '%fed%' OR lower(title) LIKE '%powell%'
    OR lower(title) LIKE '%fomc%' OR lower(title) LIKE '%dxy%'
    OR lower(title) LIKE '%dollar%' OR lower(title) LIKE '%yield%'
    OR lower(title) LIKE '%real%' OR lower(title) LIKE '%cpi%'
    OR lower(title) LIKE '%inflation%' OR lower(title) LIKE '%tariff%'
    OR lower(title) LIKE '%iran%' OR lower(title) LIKE '%russia%'
    OR lower(title) LIKE '%china%' OR lower(title) LIKE '%pboc%'
    OR lower(title) LIKE '%safe-haven%' OR lower(title) LIKE '%haven%')
ORDER BY published_utc DESC LIMIT 100;
"
```

If the result is sparse (< 10 articles), drop the AND clause and just pull the
window's headlines, then visually filter. (`news_historical` itself is shallow —
a rebuilt DB may hold only ~1k rows — so a short window is expected.)

## Step 3 — Read the bodies of the top 6–10 articles

Pick the most narratively-loaded titles (not generic price recaps) and pull
their bodies in one query. A `body` may be `NULL` (never fetched) or `''`
(FXStreet had none) — skip those and pick the next-best title:

```bash
/c/duckdb_cli-windows-amd64/duckdb d:/MyProjects/ctrader_rust/Bots_db/xauusd.duckdb -c "
SELECT published_utc, title, body
FROM news_historical
WHERE article_id IN ('<id1>','<id2>','<id3>')
  AND body IS NOT NULL AND body <> '';
"
```

Read enough body text to understand the **mechanism** each article describes —
not just the conclusion.

## Step 4 — Pull EC surprises from the window (vol ≥ 2)

```bash
/c/duckdb_cli-windows-amd64/duckdb d:/MyProjects/ctrader_rust/Bots_db/xauusd.duckdb -c "
SELECT timestamp_utc, currency, volatility, event_name,
       actual, forecast, previous, surprise
FROM xauusd_economic_calendar
WHERE timestamp_utc >= '<start_utc>'
  AND volatility >= 2
  AND actual IS NOT NULL
ORDER BY timestamp_utc DESC LIMIT 40;
"
```

Flag the ones that materially surprised (|surprise / forecast| > 10% or > 1 std
dev of typical print). Note: `xauusd_economic_calendar` is kept **current**
(today-centric), so older-than-a-few-days surprises may be absent — report what
the window actually returns rather than assuming a full week is present. If the
table errors as missing, the app hasn't run an EC fetch yet this session.

## Step 5 — Price regime over the window (D1)

```bash
/c/duckdb_cli-windows-amd64/duckdb d:/MyProjects/ctrader_rust/Bots_db/xauusd.duckdb -c "
SELECT to_timestamp(timestamp) AS day, open, high, low, close,
       ROUND((close - open) / open * 100, 2) AS pct_change
FROM xauusd_d1
WHERE timestamp >= EXTRACT(EPOCH FROM (now() - INTERVAL <N> DAY))::BIGINT
ORDER BY timestamp;
"
```

Note the cumulative move, average range, and whether daily candles trended or
chopped. (`xauusd_d1` holds ~weeks of bars — fine for a 5-day window.)

## Step 6 — Synthesize themes

Identify **1–3 dominant themes** that thread through both the news and the events. For each:
- Name it in 1 sentence.
- Cite 2–3 supporting headlines (with timestamps).
- Note its trajectory: **escalating** (more / sharper mentions over the window), **fading** (tapering off), **shifting** (replaced by a new theme).
- Identify confounders — articles or events in the same window pointing the other way.

## Output template

```markdown
# XAUUSD Narrative Window — {start_utc} → {today_utc}

**Price action:** {start_close} → {today_close}  ({+/-Δ USD, +/-Δ%}) · cumulative {regime: rally / sell-off / chop}
**Realized daily range avg:** {x.x} USD · **EC events with |surprise|>10%:** {n}

## Dominant theme

**{One sentence — the story.}**

Supporting:
- {HH:MM UTC YYYY-MM-DD} — {headline} — {1-line takeaway from body}
- ...

**Trajectory:** {escalating / fading / shifting}

## Secondary themes (if any)

**{theme 2 name}**
- {1-2 supporting headlines}

## Confounders

- {Article(s) or event(s) pointing the other way, with timestamps and 1-line summary}
- *(or "None material in the window")*

## EC surprises this window (vol ≥ 2)

| Date / time UTC | Cur | Event | Forecast | Actual | Surprise |
|---|---|---|---|---|---|
| ... |

*(Bold the ones that materially diverged from consensus.)*

## What this means for the immediate gold picture

{2-4 sentences. Not a trade idea — just: "the dominant theme is X, the tape is consistent / inconsistent with it, and that means the next 24-48h will likely…" Stay descriptive, not prescriptive.}
```

Stop there. **Do not produce a bias, entry, or stop.** For that, the user runs `/xauusd-trade-idea`.
