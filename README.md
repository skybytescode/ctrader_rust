# cTrader Rust — XAUUSD Trading Terminal

Tauri (Rust backend) + React/TypeScript frontend + Tokio async networking +
cTrader OpenAPI + DuckDB. Live spot prices, 14-timeframe candle charts with
local cache, economic calendar, and news pipeline — all in one desktop app.

## Live Data Pipelines

The frontend exposes four tabs — **Dashboard** (XAUUSD chart), **Calendar**
(EC events), **News** (today's headlines), and **Archives** (per-day news
JSON dump). Two of these are backed by always-on Rust loops that ingest from
FXStreet via the bundled econcal proxy on `localhost:6000` (auto-spawned
on app start). The Dashboard chart is fed by a per-(symbol, timeframe)
DuckDB cache plus live spot ticks from cTrader.

---

### 1. EC Calendar (Economic Calendar)

Fetches economic events from FXStreet via the econcal proxy (`localhost:6000`).
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
| `eurusd_economic_calendar` | Forever | Historical archive of every released actual / forecast |

Both tables share the same schema:

| Column | Type | Description |
|--------|------|-------------|
| `event_date_id` | VARCHAR | PK: `{event_id}__{datetime}` |
| `event_id` | VARCHAR | Recurring event type ID |
| `event_name` | VARCHAR | e.g. "Fed Interest Rate Decision" |
| `currency` | VARCHAR | EUR, USD, etc. |
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

**UI:** the **Calendar** tab in the React frontend renders today's events as
a sortable table (time, currency, volatility dot, event name, actual /
forecast / previous). Rows update whenever the backend pushes a fresh
`ec_today` snapshot over WebSocket.

**Timezone note:** FXStreet's `DateUtc` field is actually CET (UTC+1).
The app corrects this by subtracting 1 hour, so all displayed times are real UTC.

**Requires:** econcal proxy running on `:6000` — auto-spawned by the Rust app
on startup.

---

### 2. News

Fetches FXStreet's general news feed via the econcal proxy (`localhost:6000`),
stores metadata + plain-text bodies in DuckDB, exposes a live **News** tab
in the UI, and (on demand via the **Archives** tab) dumps one JSON file per
UTC day under `news_data/all/YYYY-MM/YYYY-MM-DD.json` for downstream readers
(model summaries, RAG, future ML training).

The same DuckDB row is the source of truth for the live UI, the on-demand
article modal, and the per-day JSON archive — there is no separate news state.

**How it works:**

1. On app start (and every **5 minutes** thereafter), the session loop reads
   `MAX(published_utc)` from `news_historical` and calls
   `fetch_news_since(cutoff)` against
   `proxy:/v4/en/post/filter/GeneralFeed/{page}/50/PlainText`.
2. The fetcher walks pages newest-first (up to 200 pages = 10,000 articles)
   and stops the moment it sees an article whose timestamp ≤ cutoff.
3. **Crypto filter** drops anything whose title or tags match a hard-coded
   keyword list (`bitcoin`, `eth`, `crypto`, …) — those articles never enter
   the DB.
4. Surviving rows are **upserted** into `news_historical` (insert new,
   refresh metadata on existing, **preserve any already-fetched body**).
5. The loop re-reads today's slice from the DB and pushes
   `{"type":"news_today","articles":[…]}` over WebSocket → React updates
   the **News** tab badge + list.
6. Clicking an article opens a modal. If the row has no body, the modal
   invokes `fetch_article_body_on_demand`, which hits
   `proxy:/v4/en/post/{article_id}`, strips HTML to plain text, and writes
   it back to `news_historical.body` for next time.

**Gap-fill on startup:** Because the cutoff comes from
`MAX(news_historical.published_utc)`, the first 5-min cycle after the app
has been closed for days automatically backfills every missing article up
to ~133 days (the 200-page × 50-article ceiling).

**DuckDB Tables:**

| Table | Purpose |
|-------|---------|
| `news_historical` | The real source of truth. Every fetched article ever seen, with optional `body`. Upserted (never truncated). Currently ~37k rows since Feb 2025. |
| `news_today` | Vestigial. Dropped + recreated on every fetch but **never read** — the "today's news" path queries `news_historical` filtered by today's date. |

**`news_historical` schema:**

| Column | Type | Description |
|--------|------|-------------|
| `article_id` | VARCHAR PK | FXStreet article GUID |
| `title` | VARCHAR | Headline |
| `published_utc` | VARCHAR | ISO `YYYY-MM-DDTHH:MM:SS` (UTC) |
| `summary` | VARCHAR | One-paragraph teaser from the feed |
| `url` | VARCHAR | Canonical fxstreet.com URL |
| `author` | VARCHAR | Byline (may be empty) |
| `tags` | VARCHAR | Comma-separated FXStreet tag names |
| `hour_utc` | TINYINT | 0–23 |
| `weekday` | TINYINT | 0 = Mon … 6 = Sun |
| `body` | VARCHAR | `NULL` = never fetched, `''` = FXStreet had no body, otherwise full plain text |

**Archives tab — `News_Updates` button**

A single-button workflow that takes whatever's in `news_historical` and
produces / refreshes the per-day JSON files on disk, fetching missing
bodies in the process:

1. Walk `news_data/all/YYYY-MM/` to find the newest `YYYY-MM-DD.json`.
2. Parse it, read `max(article.published_utc)` → that's the cutoff. (If no
   archive file exists yet, cutoff defaults to 7 days ago.)
3. `ensure_econcal_alive()` — respawn the proxy if it's down.
4. `fetch_news_since(cutoff)` → upsert metadata for everything newer.
5. For every article in the affected days that still has `body IS NULL`
   (whether it landed via the 5-min loop or this fetch), call the
   per-article body fetcher **sequentially** (concurrency 1 — the puppeteer
   proxy serialises requests internally; higher concurrency confuses its
   shared page state).
6. `UPDATE news_historical SET body = ?` for each result. Failures leave
   `body=NULL` so the next click retries. HTTP 429 aborts the body loop
   gracefully — the day files still get written with whatever bodies we
   already had, and the UI shows the retry-after time.
7. Regenerate one JSON file per affected day from `news_historical`.

**Per-day JSON file format** (`news_data/all/YYYY-MM/YYYY-MM-DD.json`):

```json
{
  "date": "2026-05-22",
  "count": 102,
  "generated_at": "2026-05-22T15:08:50.696Z",
  "articles": [
    {
      "article_id": "e91769a6-…",
      "title": "…",
      "published_utc": "2026-05-22T07:14:00",
      "summary": "…",
      "url": "https://www.fxstreet.com/news/…",
      "author": "…",
      "tags": "Currencies,Commodities,…",
      "hour_utc": 7,
      "weekday": 4,
      "body": "Full plain text article body…"
    }
  ]
}
```

**Data flow:**

```
FXStreet (via econcal :6000 puppeteer proxy)
    |
    |  every 5 min: fetch_news_since(MAX(news_historical.published_utc))
    v
DuckDB news_historical  (upsert; bodies preserved across upserts)
    |
    +--> read_news_today() --> WS "news_today" --> React <NewsView/>
    |
    +--> ArticleModal (on click) --> fetch_article_body_on_demand
    |        |                              |
    |        v                              v
    |   shows body                  UPDATE news_historical.body
    |
    +--> Archives tab "News_Updates" button:
              find archive cutoff -> fetch_news_since(cutoff)
              -> UPDATE news_historical (metadata + bodies)
              -> regenerate news_data/all/YYYY-MM/YYYY-MM-DD.json per affected day
```

**Requires:** econcal proxy running on `:6000` (`cd econcal && node econcal.js`),
auto-spawned by the Rust app on startup.

---

## Prerequisites

- **Node.js** installed (the Rust app auto-spawns `econcal/econcal.js` and
  the Vite dev server on startup; both need `node` / `npm` on PATH)
- **cTrader OpenAPI credentials** in a `.env` file at the project root:
  `CTRADER_CLIENT_ID`, `CTRADER_SECRET`, `CTRADER_ACCESS_TOKEN`,
  `CTRADER_ACCOUNT_ID`, and optionally `CTRADER_SYMBOL` (defaults to XAUUSD)
- **DuckDB database** at `Bots_db/Algo_EURUSD.duckdb` — created on first run.
  Path is hard-coded; the filename is historical (from the EUR/USD era)
  but the DB is symbol-agnostic.

Run with `cargo run` — the Rust app boots the Tauri webview, auto-spawns
the econcal Node proxy on `:6000` and the Vite dev server on `:5173`,
opens the WebSocket on `:6001`, and connects to cTrader OpenAPI.
