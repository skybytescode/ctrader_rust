"""
Model 4 — Economic Calendar Feature Engineering
=================================================
Computes ~70 features from the eurusd_economic_calendar table,
aligned to M1 bar timestamps.  All features are backward-looking only
(no look-ahead bias).

Usage:
    from ml.model4_econcal.features import load_ec_events, compute_ec_features
    df_ec = load_ec_events("Bots_db/Algo_EURUSD.duckdb")
    df_feat = compute_ec_features(m1_index, df_ec)
"""

import numpy as np
import pandas as pd
import duckdb

DB_PATH  = "Bots_db/Algo_EURUSD.duckdb"
EC_TABLE = "eurusd_economic_calendar"

# Named high-impact event patterns (case-insensitive substring match)
_NAMED_EVENTS = {
    "nfp":  ["nonfarm", "non-farm", "nfp"],
    "fomc": ["fomc", "fed interest rate decision", "fed monetary policy"],
    "ecb":  ["ecb main refinancing", "ecb rate on deposit", "ecb monetary policy"],
    "cpi":  ["consumer price index", "cpi"],
}

LOOKBACK_WINDOWS = {
    "1h":  60,
    "4h":  240,
    "24h": 1440,
    "1w":  10080,
}


# ── Data loading ─────────────────────────────────────────────────────────────

def load_ec_events(db_path: str = DB_PATH) -> pd.DataFrame:
    """Load economic calendar events with actual values from DuckDB."""
    db = duckdb.connect(db_path, read_only=True)
    df = db.execute(f"""
        SELECT timestamp_utc, event_id, event_name, currency,
               volatility, actual, forecast, previous,
               surprise, beats_forecast
        FROM {EC_TABLE}
        WHERE actual IS NOT NULL
        ORDER BY timestamp_utc
    """).fetchdf()
    db.close()

    df["timestamp_utc"] = pd.to_datetime(df["timestamp_utc"], utc=True)
    df = df.set_index("timestamp_utc").sort_index()

    # Normalise surprise by event's historical std (avoid divide-by-zero)
    grouped_std = df.groupby("event_id")["surprise"].transform("std")
    df["surprise_norm"] = df["surprise"] / grouped_std.replace(0, np.nan)
    df["surprise_norm"] = df["surprise_norm"].fillna(0.0)

    # Tag named events
    name_lower = df["event_name"].str.lower()
    for tag, patterns in _NAMED_EVENTS.items():
        df[f"is_{tag}"] = name_lower.str.contains("|".join(patterns), regex=True).astype(np.int8)

    return df


# ── Per-bar feature computation ──────────────────────────────────────────────

def compute_ec_features(
    m1_index: pd.DatetimeIndex,
    df_ec: pd.DataFrame,
) -> pd.DataFrame:
    """
    Build EC features aligned to M1 bar timestamps.

    Args:
        m1_index: DatetimeIndex of M1 bars (tz-naive UTC).
        df_ec:    Output of load_ec_events().

    Returns:
        DataFrame with same index as m1_index, ~70 feature columns.
    """
    # Ensure both indices are tz-naive and same resolution for merge_asof
    if m1_index.tz is not None:
        m1_index = m1_index.tz_localize(None)
    m1_index = m1_index.as_unit("us")
    ec = df_ec.copy()
    if ec.index.tz is not None:
        ec.index = ec.index.tz_localize(None)
    ec.index = ec.index.as_unit("us")

    n_bars = len(m1_index)
    result = pd.DataFrame(index=m1_index)

    # ── A. Last event features (merge_asof per currency × vol tier) ──────
    for cur in ["EUR", "USD"]:
        ec_cur = ec[ec["currency"] == cur].copy()
        for vol in [1, 2, 3]:
            ec_sub = ec_cur[ec_cur["volatility"] >= vol][
                ["surprise_norm", "surprise", "beats_forecast"]
            ].copy()
            if ec_sub.empty:
                for col in ["surprise_norm", "abs_surprise", "beats", "hours_ago"]:
                    result[f"last_{cur.lower()}_vol{vol}_{col}"] = 0.0
                continue

            ec_sub = ec_sub.rename(columns={
                "surprise_norm":  f"last_{cur.lower()}_vol{vol}_surprise_norm",
                "surprise":       f"_raw_surprise",
                "beats_forecast": f"last_{cur.lower()}_vol{vol}_beats",
            })
            ec_sub[f"last_{cur.lower()}_vol{vol}_abs_surprise"] = ec_sub["_raw_surprise"].abs()
            ec_sub[f"_ts"] = ec_sub.index

            merged = pd.merge_asof(
                pd.DataFrame(index=m1_index),
                ec_sub,
                left_index=True,
                right_index=True,
                direction="backward",
            )
            hours_ago = (m1_index - merged["_ts"]).dt.total_seconds() / 3600.0
            result[f"last_{cur.lower()}_vol{vol}_surprise_norm"] = merged[
                f"last_{cur.lower()}_vol{vol}_surprise_norm"
            ].values
            result[f"last_{cur.lower()}_vol{vol}_abs_surprise"] = merged[
                f"last_{cur.lower()}_vol{vol}_abs_surprise"
            ].values
            result[f"last_{cur.lower()}_vol{vol}_beats"] = merged[
                f"last_{cur.lower()}_vol{vol}_beats"
            ].values
            result[f"last_{cur.lower()}_vol{vol}_hours_ago"] = hours_ago.values

    # ── B. Rolling event counts per window ───────────────────────────────
    # Create minute-resolution event count series
    for cur in ["EUR", "USD"]:
        ec_cur = ec[ec["currency"] == cur]
        for vol in [1, 2, 3]:
            ec_sub = ec_cur[ec_cur["volatility"] >= vol]
            counts = ec_sub.resample("1min").size()
            counts = counts.reindex(m1_index, method=None).fillna(0)
            for win_name, win_mins in LOOKBACK_WINDOWS.items():
                col = f"ec_count_{cur.lower()}_vol{vol}_{win_name}"
                result[col] = counts.rolling(win_mins, min_periods=1).sum().values

    # ── C. Aggregate surprise differential per window ────────────────────
    for cur in ["EUR", "USD"]:
        ec_cur = ec[ec["currency"] == cur]
        # Sparse surprise series on minute grid
        surprise_series = ec_cur["surprise_norm"].resample("1min").mean()
        surprise_series = surprise_series.reindex(m1_index, method=None)

        for win_name, win_mins in LOOKBACK_WINDOWS.items():
            col = f"{cur.lower()}_avg_surprise_{win_name}"
            result[col] = (
                surprise_series
                .rolling(win_mins, min_periods=1)
                .mean()
                .fillna(0)
                .values
            )

    # Net surprise differential (EUR - USD)
    for win_name in LOOKBACK_WINDOWS:
        result[f"net_surprise_{win_name}"] = (
            result[f"eur_avg_surprise_{win_name}"]
            - result[f"usd_avg_surprise_{win_name}"]
        )

    # ── D. Named event day flags + hours since ───────────────────────────
    for tag in _NAMED_EVENTS:
        ec_tag = ec[ec[f"is_{tag}"] == 1]
        if ec_tag.empty:
            result[f"is_{tag}_day"] = 0
            result[f"hours_since_{tag}"] = 9999.0
            continue

        # is_X_day: any event with this tag on the same calendar day
        tag_dates = set(ec_tag.index.date)
        result[f"is_{tag}_day"] = pd.Series(
            m1_index.date, index=m1_index
        ).isin(tag_dates).astype(np.int8).values

        # hours_since_X: merge_asof to find last occurrence
        tag_ts = pd.DataFrame({"_ts": ec_tag.index}, index=ec_tag.index)
        merged = pd.merge_asof(
            pd.DataFrame(index=m1_index),
            tag_ts,
            left_index=True,
            right_index=True,
            direction="backward",
        )
        hours = (m1_index - merged["_ts"]).dt.total_seconds() / 3600.0
        result[f"hours_since_{tag}"] = np.clip(hours.values, 0, 9999)

    # ── E. High-vol temporal proximity ───────────────────────────────────
    ec_high = ec[ec["volatility"] >= 3]
    if not ec_high.empty:
        high_ts = pd.DataFrame({"_ts": ec_high.index}, index=ec_high.index)
        merged = pd.merge_asof(
            pd.DataFrame(index=m1_index),
            high_ts,
            left_index=True,
            right_index=True,
            direction="backward",
        )
        mins_since = (m1_index - merged["_ts"]).dt.total_seconds() / 60.0
        result["bars_since_last_high_vol"] = np.clip(mins_since.values, 0, 1440)
        result["high_vol_in_last_1h"] = (mins_since.values <= 60).astype(np.int8)
        result["high_vol_in_last_4h"] = (mins_since.values <= 240).astype(np.int8)
    else:
        result["bars_since_last_high_vol"] = 1440.0
        result["high_vol_in_last_1h"] = 0
        result["high_vol_in_last_4h"] = 0

    # Fill any remaining NaN with 0
    result = result.fillna(0).astype(np.float32)

    print(f"  EC features: {result.shape[1]} columns, {result.shape[0]:,} rows")
    return result
