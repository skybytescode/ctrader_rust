# cTrader Rust — EUR/USD Algorithmic Trading System

Bevy 0.15 (ECS) UI + Tokio async networking + cTrader OpenAPI + DuckDB + ML Pipeline.

## Real-Time Data

The **Real-Time Data** card in the UI provides live market intelligence from three sources:
Depth of Market (DoM), Economic Calendar (EC), and News (coming soon).

---

### 1. Price DoM (Depth of Market) Capture

Captures live Level 2 order book data from cTrader's `ProtoOaDepthEvent` (payload 2155)
for EURUSD. Records every quote change (new, update, delete) at the per-pip price level.

**How it works:**

1. Click **"Start Capture"** in the Real-Time Data card.
2. Rust subscribes to the DoM feed via cTrader OpenAPI.
3. Each event updates an **in-memory order book** (`HashMap<quote_id, (side, price, size)>`).
4. Raw events are **batch-flushed** to DuckDB every 2 seconds (or 5,000 rows).
5. Every **60 seconds**, the book state is aggregated into M1 features.
6. Click **"Pause Capture"** to flush remaining data and release the DB.

**DuckDB Tables:**

| Table | Retention | Write Freq | Purpose |
|-------|-----------|------------|---------|
| `eurusd_dom_raw` | 8 weeks (rolling) | Every 2s | Raw event-level data |
| `eurusd_dom_features_m1` | Forever | Every 60s | M1 aggregated features for ML |

**`eurusd_dom_raw` schema:**

| Column | Type | Description |
|--------|------|-------------|
| `ts_ms` | BIGINT | Millisecond timestamp (UTC) |
| `event_type` | TINYINT | 0 = new/update, 1 = delete |
| `quote_id` | BIGINT | Unique quote lifecycle ID |
| `side` | TINYINT | 0 = bid, 1 = ask |
| `price` | INTEGER | Price as integer (115320 = 1.15320) |
| `size` | BIGINT | Volume in cents (0 for deletes) |

At ~31 events/second during active hours, expect ~30M rows/day, ~10-20 GB for 8 weeks.
Auto-cleanup removes rows older than 8 weeks on each capture start.

**`eurusd_dom_features_m1` schema:**

| Column | Type | Description |
|--------|------|-------------|
| `timestamp` | BIGINT | Minute start (unix seconds), PRIMARY KEY |
| `obi_mean` | FLOAT | Mean Order Book Imbalance (-1 to +1) |
| `obi_std` | FLOAT | OBI volatility (flickering/spoofing detection) |
| `spread_mean` | FLOAT | Average spread in pips |
| `spread_max` | FLOAT | Widest spread (LP withdrawal signal) |
| `bid_vol_mean` | FLOAT | Average total bid-side volume |
| `ask_vol_mean` | FLOAT | Average total ask-side volume |
| `bid_vol_min` | FLOAT | Minimum bid volume (liquidity vacuum detection) |
| `ask_vol_min` | FLOAT | Minimum ask volume (liquidity vacuum detection) |
| `bid_ask_ratio` | FLOAT | bid_vol / ask_vol (directional pressure) |
| `bid_levels` | FLOAT | Average number of bid price levels |
| `ask_levels` | FLOAT | Average number of ask price levels |
| `churn_rate` | FLOAT | (adds + deletes) / snapshots (spoofing signal) |
| `quotes_added` | INTEGER | Total new/update events this minute |
| `quotes_deleted` | INTEGER | Total delete events this minute |
| `best_bid_max` | BIGINT | Peak volume at best bid (wall detection) |
| `best_ask_max` | BIGINT | Peak volume at best ask (wall detection) |
| `snapshots` | INTEGER | Number of book snapshots taken this minute |

Each M1 row is computed from ~1,800 book snapshots (at ~31 events/sec).
This table accumulates forever and will train Model 5 (Depth of Market) after 6+ months.

**Data flow:**

```
cTrader DoM Feed (ProtoOaDepthEvent, ~31/sec)
    |
    v
In-memory order book (HashMap, microsecond access)
    |
    +--> Raw batch buffer --> eurusd_dom_raw (every 2s, 8-week rolling)
    |
    +--> M1 accumulator --> eurusd_dom_features_m1 (every 60s, forever)
    |
    +--> UI status: "Active | 31 rows/s | 9,662 total | DB updated: 18:34"
```

---

### 2. EC Calendar (Economic Calendar)

Fetches EUR/USD economic events from FXStreet via the econcal proxy (`localhost:6000`).
Uses **smart scheduling** to minimize API calls while capturing actuals as fast as possible.

**How it works:**

1. On app start (10 seconds after authentication), fetches today's EC events.
2. Parses event times and builds a **fetch schedule**: 8 fetches per event
   (every 30 seconds for the first 3 minutes, then one final at +5 minutes).
3. Between events, **no fetching** occurs (saves bandwidth).
4. On new day, resets the today table and fetches the new schedule.
5. Countdowns in the UI refresh every **30 seconds** from cached data (no DB reads).

**Smart scheduling example (21 events today):**

```
App start --> fetch schedule: 21 events x 8 fetches = 168 scheduled
Event at 12:30 UTC:
  12:30:00 --> fetch (catch actual on release)
  12:30:30 --> fetch
  12:31:00 --> fetch
  12:31:30 --> fetch
  12:32:00 --> fetch
  12:32:30 --> fetch
  12:33:00 --> fetch
  12:35:00 --> fetch (final, catch revisions)
  ... quiet until next event at 13:15 ...
```

**DuckDB Tables:**

| Table | Retention | Purpose |
|-------|-----------|---------|
| `eurusd_ec_today` | 1 day (cleared daily) | Fast UI reads, today's events only |
| `eurusd_economic_calendar` | Forever (since 2009) | Historical data for Model 4 training |

Both tables share the same 18-column schema:

| Column | Type | Description |
|--------|------|-------------|
| `event_date_id` | VARCHAR | PK: `{event_id}__{datetime}` |
| `event_id` | VARCHAR | Recurring event type ID |
| `event_name` | VARCHAR | e.g. "Fed Interest Rate Decision" |
| `currency` | VARCHAR | EUR or USD |
| `country_code` | VARCHAR | US, DE, FR, etc. |
| `volatility` | TINYINT | 1=low, 2=medium, 3=high impact |
| `timestamp_utc` | VARCHAR/TIMESTAMP | Event time in UTC |
| `weekday` | TINYINT | 0=Mon ... 6=Sun |
| `hour_utc` | TINYINT | Hour in UTC |
| `actual` | DOUBLE | Released actual value |
| `forecast` | DOUBLE | Market consensus forecast |
| `previous` | DOUBLE | Previous period value |
| `surprise` | DOUBLE | actual - forecast |
| `beats_forecast` | TINYINT | +1 beat, -1 miss, 0 inline |

**UI Display (Real-Time Data card):**

```
EC Calendar: 23 events (4 upcoming) | next: 19:30 | updated: 18:45:32

--- UPCOMING (4) ---
  19:30 USD *   F:-- P:-134.50  CFTC S&P 500 NC Net Positions (in 3m)
  19:30 USD *   F:-- P:228.00   CFTC Oil NC Net Positions (in 3m)
--- JUST RELEASED (2) ---
  18:30 USD *** A:3.90 F:3.70 P:3.60 S:+0.20  Producer Price Index
--- PAST (17) ---
  07:00 EUR **  A:-0.50 F:0.30 P:-0.60 S:-0.80  Producer Price Index (MoM)
```

**Timezone note:** FXStreet's `DateUtc` field is actually CET (UTC+1).
The app corrects this by subtracting 1 hour, so all displayed times are real UTC.

**Requires:** econcal proxy running: `cd econcal && node econcal.js`

---

### 3. News (Coming Soon)

Will fetch EURUSD-tagged headlines from FXStreet via the same econcal proxy.
Stored in `eurusd_news` table, accumulating for future News Sentiment ML model.

---

## Prerequisites

- econcal proxy: `cd econcal && node econcal.js` (must be running for EC Calendar)
- cTrader OpenAPI credentials in `.env` file
- DuckDB database at `Bots_db/Algo_EURUSD.duckdb`
