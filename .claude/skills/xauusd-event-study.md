---
name: xauusd-event-study
description: Event-study for XAUUSD (gold). For each vol≥2 economic event in the last N days (default 7), pulls the M1 candles ±60 minutes around the release and reports the actual price reaction — net move in USD and ATR-multiples, follow-through vs fade. Tells you whether the tape is agreeing with the consensus interpretation. Read-only, deterministic.
---

# XAUUSD Event Study (last 7 days)

For each vol≥2 EC event in the lookback window with a non-null `actual`, pull the M1 candles ±60 minutes around the release timestamp and analyze the market's actual reaction.

Default lookback: **3 calendar days** (was 7). The DuckDB candle tables are now
a **shallow rolling window** kept fresh by the running app — `xauusd_m1` holds
only ~4 days, `xauusd_m5` ~5 days, `xauusd_h1` ~8 days. A 7-day study would
silently lose its oldest reactions, so default to 3 days and only widen if the
user asks (and accept that older events will fall back to coarser TFs or drop
out entirely). `xauusd_economic_calendar` is likewise **today-centric**, so the
event list itself may be thin beyond a couple of days — report what's actually
present, never assume a full week exists.

**Why this matters.** Surprises tell you what the data said. Reactions tell you what the *market believed*. When CPI comes in soft and gold rallies — confirmed dovish; the data and positioning agree. When CPI comes in soft and gold *sells off* — positioning is fighting the data, and that's a stronger signal about the regime than any single print.

## Step 1 — Compute the window

- `start_ts` = now − N days (UTC)
- `month_glob` for the EC archives covering the window

## Step 2 — Pull the vol≥2 events with actuals

```bash
/c/duckdb_cli-windows-amd64/duckdb d:/MyProjects/ctrader_rust/Bots_db/xauusd.duckdb -c "
SELECT timestamp_utc, currency, volatility, event_name,
       actual, forecast, previous, surprise
FROM xauusd_economic_calendar
WHERE timestamp_utc >= '<start_iso>'
  AND volatility >= 2
  AND actual IS NOT NULL
ORDER BY timestamp_utc DESC LIMIT 30;
"
```

Prioritize:
1. **All USD vol≥2** events (direct gold impact).
2. **All USD vol=3** events even if older than the window.
3. **EUR vol≥2** if they materially surprised.
4. **Fed-speaker rows** with vol≥2 (their `actual` is usually NULL, but the timestamp marks the speech window — include them and flag as "Fed-speaker / no surprise").

## Step 3 — For each event, pull the M1 reaction window

For each event timestamp `T_ev`:

```bash
/c/duckdb_cli-windows-amd64/duckdb d:/MyProjects/ctrader_rust/Bots_db/xauusd.duckdb -c "
WITH ev AS (SELECT EXTRACT(EPOCH FROM TIMESTAMP '<T_ev_iso>')::BIGINT AS t_ev)
SELECT
  (SELECT close FROM xauusd_m1, ev
    WHERE timestamp BETWEEN ev.t_ev - 1800 AND ev.t_ev - 60
    ORDER BY timestamp DESC LIMIT 1) AS px_pre_30m,
  (SELECT close FROM xauusd_m1, ev
    WHERE timestamp BETWEEN ev.t_ev - 60  AND ev.t_ev + 60
    ORDER BY timestamp ASC  LIMIT 1) AS px_t0,
  (SELECT close FROM xauusd_m1, ev
    WHERE timestamp BETWEEN ev.t_ev + 14*60 AND ev.t_ev + 16*60
    ORDER BY timestamp ASC  LIMIT 1) AS px_post_15m,
  (SELECT close FROM xauusd_m1, ev
    WHERE timestamp BETWEEN ev.t_ev + 59*60 AND ev.t_ev + 61*60
    ORDER BY timestamp ASC  LIMIT 1) AS px_post_60m,
  (SELECT MAX(high) - MIN(low) FROM xauusd_m1, ev
    WHERE timestamp BETWEEN ev.t_ev - 60 AND ev.t_ev + 60*60) AS reaction_range;
"
```

If `xauusd_m1` doesn't cover the event (it now holds only ~4 days), **fall back
to `xauusd_m5`** (~5 days), then to `xauusd_h1` (~8 days), each with a wider
tolerance (`±5 min`, then `±60 min` snap windows). Note the TF used in the
output row. If even `xauusd_h1` doesn't reach the event, drop that row and say
so — there is no deeper candle history to fall back to.

## Step 4 — Compute the verdict

For each event row, compute:
- **Net move (15m):** `px_post_15m − px_pre_30m`
- **Net move (60m):** `px_post_60m − px_pre_30m`
- **Reaction range (±60m):** `MAX(high) − MIN(low)` over the window
- **ATR multiple:** `net_move_60m / ATR(14, H1)` (use current ATR; doesn't have to be perfectly matched)
- **Direction expected from surprise:**
  - USD soft data (actual < forecast) → expect gold **up** (dovish for Fed)
  - USD hot data (actual > forecast) → expect gold **down** (hawkish for Fed)
  - Inflation (CPI/PCE) hot → expect gold **down** (real-yield channel dominates over haven channel for these prints)
  - Geopolitical-haven release → expect gold **up**
- **Verdict:** `FOLLOW` if net 60m move agrees with expected direction by ≥ 0.5× ATR(14, H1); `FADE` if it disagrees by ≥ 0.5× ATR; `MUTED` otherwise.

## Output template

```markdown
# XAUUSD Event Study — last {N} days

**Events analyzed:** {n} · **FOLLOW:** {n} · **FADE:** {n} · **MUTED:** {n}

## Event table

| Date / time UTC | Cur | Event | Forecast | Actual | Surprise | Px pre−30m | Px +15m | Px +60m | Range ±60m | ATR× | Verdict |
|---|---|---|---|---|---|---|---|---|---|---|---|
| ... |

## Notable reactions

### {Event 1 name — date}
- **Surprise:** {direction + magnitude}
- **Reaction:** gold moved {+/-Δ USD} ({+/-x.x× ATR}) in 60 min
- **Verdict:** {FOLLOW / FADE / MUTED}
- **Interpretation:** {1-2 sentences — what this says about the current regime}

### {Event 2 …}

## Aggregate read

- **Follow-through bias:** {are recent USD-hot surprises pushing gold lower as expected, or is the tape fading them? — say so directly}
- **Inflation prints:** {how have the last 1-2 CPI/PCE/PPI prints been digested?}
- **Fed speakers:** {dovish/hawkish balance from speech windows}
- **What this implies:** {2-3 sentences — what the market's *reaction pattern* is telling you about positioning and regime, independent of the raw data}
```

Stop there. **No bias, no entry, no recommendation.** For a tradeable view, the user runs `/xauusd-trade-idea`.
