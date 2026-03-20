"""
update_econcal.py — append new events to eurusd_economic_calendar since last record.

Behaviour
---------
- Reads the last timestamp_utc from the table.
- Fetches month-by-month from that date to today via econcal proxy (localhost:6000).
- Inserts new rows (PK conflicts silently skipped).
- Prints a one-line summary:
    "Updated: +N rows | X days Y hours behind"   (if new rows found)
    "Already up to date | X hours behind"         (if nothing new)

Run via:
    python -m ml.model4_news.update_econcal
"""

import json
import sys
from datetime import date, datetime, timedelta, timezone
from pathlib import Path

import duckdb
import requests

DB_PATH   = "Bots_db/Algo_EURUSD.duckdb"
PROXY_URL = "http://localhost:6000"
TABLE     = "eurusd_economic_calendar"
CACHE_DIR = Path("ml/model4_news/cache")

# ── Reuse helpers from scrape_econcal ─────────────────────────────────────────

def month_ranges(start: date, end: date):
    cur = start.replace(day=1)
    while cur <= end:
        y, m  = cur.year, cur.month
        nxt   = date(y + 1, 1, 1) if m == 12 else date(y, m + 1, 1)
        m_end = min(nxt - timedelta(days=1), end)
        yield cur.strftime("%Y%m%d"), m_end.strftime("%Y%m%d"), cur.strftime("%Y-%m")
        cur = nxt


def fetch_month(start_s: str, end_s: str, cache_key: str) -> list:
    """Fetch from proxy (skip cache for update — we need fresh data)."""
    url  = f"{PROXY_URL}/v4/eventdate/mini?view=1&start={start_s}&end={end_s}"
    resp = requests.get(url, timeout=30, proxies={"http": None, "https": None})
    text = resp.text.strip()
    last_bracket = text.rfind("]")
    if last_bracket != -1:
        text = text[: last_bracket + 1]
    return json.loads(text) if text else []


def rows_from_events(events: list) -> list:
    out = []
    for ev in events:
        try:
            event_block = ev.get("Event") or {}
            currency    = event_block.get("CurrencyId", "")
            if currency not in ("USD", "EUR"):
                continue

            eid      = event_block.get("Id", "")
            date_utc = ev.get("DateUtc", "")
            if not date_utc:
                continue

            edid = f"{eid}__{date_utc}"
            name = event_block.get("Name", "")
            cc   = event_block.get("InternationalCountryCode", "")
            vol  = int(ev.get("Volatility") or 0)

            ts      = datetime.fromisoformat(date_utc.replace("Z", ""))
            weekday = ts.weekday()
            hour    = ts.hour

            actual   = ev.get("Actual")
            forecast = ev.get("Consensus")
            previous = ev.get("Previous")

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


def format_lag(last_ts: datetime, now: datetime) -> str:
    diff    = now - last_ts
    total_s = int(diff.total_seconds())
    if total_s < 0:
        return "up to date"
    days  = diff.days
    hours = (total_s % 86400) // 3600
    if days > 0:
        return f"{days} day{'s' if days > 1 else ''} {hours} hour{'s' if hours != 1 else ''} behind"
    return f"{hours} hour{'s' if hours != 1 else ''} behind"


# ── Main ──────────────────────────────────────────────────────────────────────

def main():
    db = duckdb.connect(DB_PATH)

    # Verify table exists and has data
    existing = {r[0] for r in db.execute("SHOW TABLES").fetchall()}
    if TABLE not in existing:
        print("ERROR: Table does not exist — click 'Economic Calendar' first to create it.")
        db.close()
        sys.exit(1)

    count = db.execute(f"SELECT COUNT(*) FROM {TABLE}").fetchone()[0]
    if count == 0:
        print("ERROR: Table is empty — click 'Economic Calendar' first to populate it.")
        db.close()
        sys.exit(1)

    # Get last timestamp
    row = db.execute(
        f"SELECT timestamp_utc FROM {TABLE} ORDER BY timestamp_utc DESC LIMIT 1"
    ).fetchone()
    last_ts = row[0]  # datetime object from DuckDB
    if hasattr(last_ts, 'tzinfo') and last_ts.tzinfo is None:
        last_ts = last_ts.replace(tzinfo=timezone.utc)

    last_date = last_ts.date() if hasattr(last_ts, 'date') else date.fromisoformat(str(last_ts)[:10])
    today     = date.today()

    now_utc = datetime.now(timezone.utc)
    lag_str = format_lag(last_ts.replace(tzinfo=timezone.utc) if last_ts.tzinfo is None else last_ts, now_utc)

    months = list(month_ranges(last_date, today))
    if not months:
        print(f"Already up to date | {lag_str}")
        db.close()
        return

    ph         = ", ".join(["?"] * 18)
    insert_sql = f"INSERT INTO {TABLE} VALUES ({ph})"
    inserted   = 0

    for start_s, end_s, key in months:
        try:
            events = fetch_month(start_s, end_s, key)
        except Exception as ex:
            print(f"WARN [{key}]: {ex} — skipping", flush=True)
            continue

        rows = rows_from_events(events)
        for r in rows:
            try:
                db.execute(insert_sql, list(r))
                inserted += 1
            except Exception:
                pass  # PK conflict — already exists
        if rows:
            db.commit()

    db.close()

    # Re-compute lag using newest record after update
    if inserted > 0:
        db2    = duckdb.connect(DB_PATH, read_only=True)
        row2   = db2.execute(f"SELECT timestamp_utc FROM {TABLE} ORDER BY timestamp_utc DESC LIMIT 1").fetchone()
        new_ts = row2[0]
        db2.close()
        if hasattr(new_ts, 'tzinfo') and new_ts.tzinfo is None:
            new_ts = new_ts.replace(tzinfo=timezone.utc)
        lag_str = format_lag(new_ts, now_utc)
        print(f"Updated: +{inserted:,} rows | {lag_str}")
    else:
        print(f"Already up to date | {lag_str}")


if __name__ == "__main__":
    main()
