"""
scrape_econcal.py — populate eurusd_economic_calendar table from FXStreet.

Behaviour
---------
- If table already exists  → print row count + last record, then exit.
- If table does not exist  → create it and scrape 2009-01-02 → today
  month-by-month via the econcal proxy (localhost:6000).
  Each month's raw JSON is cached to ml/model4_news/cache/<YYYY-MM>.json
  so a partial run can be resumed without re-fetching.

Run via:
    python -m ml.model4_news.scrape_econcal

Requires:
    - econcal proxy running:  cd econcal && node econcal.js
    - DuckDB at Bots_db/Algo_EURUSD.duckdb
    - pip packages: duckdb, requests
"""

import json
import sys
from datetime import date, datetime, timedelta
from pathlib import Path

import duckdb
import requests

# ── Config ────────────────────────────────────────────────────────────────────

DB_PATH   = "Bots_db/Algo_EURUSD.duckdb"
PROXY_URL = "http://localhost:6000"
TABLE     = "eurusd_economic_calendar"
START     = date(2009, 1, 2)
CACHE_DIR = Path("ml/model4_news/cache")

# ── Schema ────────────────────────────────────────────────────────────────────

CREATE_SQL = f"""
CREATE TABLE IF NOT EXISTS {TABLE} (
    event_date_id   VARCHAR PRIMARY KEY,
    event_id        VARCHAR NOT NULL,
    event_name      VARCHAR NOT NULL,
    currency        VARCHAR NOT NULL,
    country_code    VARCHAR NOT NULL,
    volatility      TINYINT NOT NULL,
    timestamp_utc   TIMESTAMP NOT NULL,
    weekday         TINYINT NOT NULL,
    hour_utc        TINYINT NOT NULL,
    actual_raw      VARCHAR,
    forecast_raw    VARCHAR,
    previous_raw    VARCHAR,
    actual          DOUBLE,
    forecast        DOUBLE,
    previous        DOUBLE,
    surprise        DOUBLE,
    beats_forecast  TINYINT,
    unit            VARCHAR
)
"""

# ── Helpers ───────────────────────────────────────────────────────────────────

def parse_value(s: str):
    """Parse '2.8%', '256K', '-0.3' → (float|None, unit_str)."""
    if not s or s.strip() in ("", "--", "N/A", "n/a"):
        return None, ""
    s = s.strip()
    unit = ""
    val  = s
    for suffix, u in [("%", "%"), ("K", "K"), ("B", "B"), ("M", "M"), ("T", "T")]:
        if s.endswith(suffix):
            unit = u
            val  = s[:-len(suffix)]
            break
    val = val.replace(",", "").strip()
    try:
        return float(val), unit
    except ValueError:
        return None, unit


def month_ranges(start: date, end: date):
    """Yield (start_str, end_str, cache_key) for each calendar month."""
    cur = start.replace(day=1)
    while cur <= end:
        y, m  = cur.year, cur.month
        nxt   = date(y + 1, 1, 1) if m == 12 else date(y, m + 1, 1)
        m_end = min(nxt - timedelta(days=1), end)
        yield cur.strftime("%Y%m%d"), m_end.strftime("%Y%m%d"), cur.strftime("%Y-%m")
        cur = nxt


def fetch_or_load(start_s: str, end_s: str, cache_key: str) -> list:
    """Return events from cache file if present, else fetch from proxy and cache."""
    cache_file = CACHE_DIR / f"{cache_key}.json"
    if cache_file.exists():
        with open(cache_file, encoding="utf-8") as f:
            return json.load(f)

    url  = f"{PROXY_URL}/v4/eventdate/mini?view=1&start={start_s}&end={end_s}"
    resp = requests.get(url, timeout=30, proxies={"http": None, "https": None})
    text = resp.text.strip()

    # econcal sometimes appends stray `{}` after the JSON array — strip it
    last_bracket = text.rfind("]")
    if last_bracket != -1:
        text = text[: last_bracket + 1]

    data = json.loads(text) if text else []

    CACHE_DIR.mkdir(parents=True, exist_ok=True)
    with open(cache_file, "w", encoding="utf-8") as f:
        json.dump(data, f)

    return data


def rows_from_events(events: list) -> list:
    """Convert raw API events → list of row tuples (USD/EUR only)."""
    out = []
    for ev in events:
        try:
            event_block = ev.get("Event") or {}
            currency = event_block.get("CurrencyId", "")
            if currency not in ("USD", "EUR"):
                continue

            eid      = event_block.get("Id", "")
            date_utc = ev.get("DateUtc", "")
            if not date_utc:
                continue

            # Stable PK: event_type_id + exact datetime string
            edid = f"{eid}__{date_utc}"

            name = event_block.get("Name", "")
            cc   = event_block.get("InternationalCountryCode", "")
            vol  = int(ev.get("Volatility") or 0)  # top-level field

            ts      = datetime.fromisoformat(date_utc.replace("Z", ""))
            weekday = ts.weekday()
            hour    = ts.hour

            # Actual/Previous are already floats at top level;
            # Consensus is the forecast field
            actual   = ev.get("Actual")
            forecast = ev.get("Consensus")
            previous = ev.get("Previous")

            # Store raw string representations for reference
            a_raw = str(actual)   if actual   is not None else None
            f_raw = str(forecast) if forecast is not None else None
            p_raw = str(previous) if previous is not None else None

            surprise = None
            beats    = None
            if actual is not None and forecast is not None:
                surprise = actual - forecast
                beats    = 1 if surprise > 0 else (-1 if surprise < 0 else 0)

            out.append((
                edid, eid, name, currency, cc, vol,
                ts.isoformat(), weekday, hour,
                a_raw, f_raw, p_raw,
                actual, forecast, previous, surprise, beats, None,
            ))
        except Exception:
            continue
    return out


# ── Main ──────────────────────────────────────────────────────────────────────

def main():
    db = duckdb.connect(DB_PATH)

    # ── Check if table already exists ────────────────────────────────────────
    existing = {r[0] for r in db.execute("SHOW TABLES").fetchall()}
    if TABLE in existing:
        count = db.execute(f"SELECT COUNT(*) FROM {TABLE}").fetchone()[0]
        if count > 0:
            row = db.execute(f"""
                SELECT event_name, currency, volatility, timestamp_utc,
                       actual_raw, forecast_raw, previous_raw, surprise
                FROM {TABLE}
                ORDER BY timestamp_utc DESC
                LIMIT 1
            """).fetchone()
            ts = row[3]
            last_str = ts.strftime("%Y-%m-%d %H:%M") if hasattr(ts, 'strftime') else str(ts)[:16]
            db.close()
            print(f"{count:,} rows | Last updated: {last_str} | consider update")
            return
        # Table exists but empty — fall through to scrape
        print(f"Table '{TABLE}' exists but is empty - starting scrape...", flush=True)

    # ── Create table and scrape ───────────────────────────────────────────────
    db.execute(CREATE_SQL)
    db.commit()

    today  = date.today()
    months = list(month_ranges(START, today))
    total  = len(months)
    print(f"Creating '{TABLE}' - scraping {total} months ({START} to {today})...", flush=True)
    print(f"Proxy: {PROXY_URL}  |  Cache: {CACHE_DIR}", flush=True)

    inserted = 0
    ph       = ", ".join(["?"] * 18)
    insert_sql = f"INSERT INTO {TABLE} VALUES ({ph})"

    for i, (s, e, key) in enumerate(months):
        try:
            events = fetch_or_load(s, e, key)
        except Exception as ex:
            print(f"  WARN [{key}]: {ex} — skipping", flush=True)
            continue

        rows = rows_from_events(events)
        if rows:
            # Insert row-by-row to skip duplicates without crashing the batch
            for row in rows:
                try:
                    db.execute(insert_sql, list(row))
                except Exception:
                    pass  # PK conflict on re-run — skip
            db.commit()
            inserted += len(rows)

        if (i + 1) % 12 == 0 or (i + 1) == total:
            print(f"  {i + 1}/{total} months | {inserted:,} rows", flush=True)

    print(f"Done — {inserted:,} rows in '{TABLE}'", flush=True)
    db.close()


if __name__ == "__main__":
    main()
