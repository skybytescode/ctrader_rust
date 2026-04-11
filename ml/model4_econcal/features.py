"""
Model 4 — Economic Calendar Event Filter
==========================================
Simplified from directional prediction to a pure filter:
- Detects proximity to high-impact economic events
- Returns "no trade" windows (±30 min around high-impact events)
- Tracks upcoming event info for UI display

This is NOT a directional model — it tells you WHEN NOT to trade.

Usage:
    from ml.model4_econcal.features import load_ec_events, check_event_filter
    df_ec = load_ec_events("ctrader.duckdb")
    filter_result = check_event_filter(current_time, df_ec)
"""

import numpy as np
import pandas as pd
import duckdb

DB_PATH = "Bots_db/Algo_EURUSD.duckdb"
EC_TABLE = "eurusd_ec_historical"

# High-impact event patterns (case-insensitive substring match)
_NAMED_EVENTS = {
    "nfp":  ["nonfarm", "non-farm", "nfp"],
    "fomc": ["fomc", "fed interest rate decision", "fed monetary policy"],
    "ecb":  ["ecb main refinancing", "ecb rate on deposit", "ecb monetary policy"],
    "cpi":  ["consumer price index", "cpi"],
}

# Filter window: don't trade within this many minutes of a high-impact event
EVENT_FILTER_MINUTES = 30

# Minimum volatility tier to trigger the filter (3 = highest impact)
MIN_VOLATILITY_FILTER = 3


def load_ec_events(db_path: str = DB_PATH) -> pd.DataFrame:
    """Load economic calendar events from DuckDB."""
    db = duckdb.connect(db_path, read_only=True)
    try:
        df = db.execute(f"""
            SELECT timestamp_utc, event_name, currency,
                   volatility, actual, forecast, previous
            FROM {EC_TABLE}
            ORDER BY timestamp_utc
        """).fetchdf()
    except Exception:
        db.close()
        return pd.DataFrame()
    db.close()

    if df.empty:
        return df

    df["timestamp_utc"] = pd.to_datetime(df["timestamp_utc"], utc=True)
    df = df.set_index("timestamp_utc").sort_index()
    return df


def check_event_filter(
    current_time: pd.Timestamp,
    df_ec: pd.DataFrame,
    window_minutes: int = EVENT_FILTER_MINUTES,
    min_volatility: int = MIN_VOLATILITY_FILTER,
) -> dict:
    """
    Check if current time is within the no-trade window of any high-impact event.

    Returns dict:
        {
            "trade_allowed": bool,
            "reason": str or None,
            "nearest_event": str or None,
            "minutes_to_event": float or None,
            "minutes_since_event": float or None,
            "is_nfp_day": bool,
            "is_fomc_day": bool,
            "is_ecb_day": bool,
            "is_cpi_day": bool,
        }
    """
    result = {
        "trade_allowed": True,
        "reason": None,
        "nearest_event": None,
        "minutes_to_event": None,
        "minutes_since_event": None,
        "is_nfp_day": False,
        "is_fomc_day": False,
        "is_ecb_day": False,
        "is_cpi_day": False,
    }

    if df_ec.empty:
        return result

    # Ensure current_time is tz-aware UTC
    if current_time.tzinfo is None:
        current_time = current_time.tz_localize("UTC")

    # Filter to high-impact events only
    high_impact = df_ec[df_ec["volatility"] >= min_volatility]
    if high_impact.empty:
        return result

    # Check named event day flags
    today = current_time.date()
    name_lower = high_impact["event_name"].str.lower()
    for tag, patterns in _NAMED_EVENTS.items():
        day_events = high_impact[
            name_lower.str.contains("|".join(patterns), regex=True)
        ]
        if not day_events.empty:
            event_dates = set(day_events.index.date)
            result[f"is_{tag}_day"] = today in event_dates

    # Find nearest event (before or after current time)
    time_diffs = (high_impact.index - current_time).total_seconds() / 60.0

    # Past events (negative diff)
    past_mask = time_diffs <= 0
    if past_mask.any():
        most_recent_idx = time_diffs[past_mask].argmax()  # closest to 0
        mins_since = abs(time_diffs[past_mask].iloc[most_recent_idx])
        result["minutes_since_event"] = round(mins_since, 1)

        if mins_since <= window_minutes:
            event_name = high_impact.iloc[most_recent_idx]["event_name"]
            result["trade_allowed"] = False
            result["reason"] = f"Within {window_minutes}min after: {event_name}"
            result["nearest_event"] = event_name
            return result

    # Future events (positive diff)
    future_mask = time_diffs > 0
    if future_mask.any():
        next_idx = time_diffs[future_mask].argmin()  # closest to 0
        mins_to = time_diffs[future_mask].iloc[next_idx]
        result["minutes_to_event"] = round(mins_to, 1)

        next_event = high_impact[future_mask].iloc[next_idx]
        result["nearest_event"] = next_event["event_name"]

        if mins_to <= window_minutes:
            result["trade_allowed"] = False
            result["reason"] = f"Within {window_minutes}min before: {next_event['event_name']}"
            return result

    return result


def compute_ec_features(
    m1_index: pd.DatetimeIndex,
    df_ec: pd.DataFrame,
) -> pd.DataFrame:
    """
    Backward-compatible interface for predict_all.py.
    Returns a simplified feature set focused on event proximity.
    """
    if m1_index.tz is None:
        m1_index = m1_index.tz_localize("UTC")

    results = []
    for ts in m1_index:
        r = check_event_filter(ts, df_ec)
        results.append(r)

    return pd.DataFrame(results, index=m1_index)
