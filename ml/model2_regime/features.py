"""
Model 2 — Regime Detection Feature Engineering
===============================================
Computes 8 volatility/return features from EURUSD M1 candles + tick spread
data stored in DuckDB.

All features characterize the *current* bar using only past and current-bar
data.  No session filter — regimes operate 24h.  The JOIN with
eurusd_tick_features_m1 means the earliest usable bar is ~Aug 2012;
we default to 2013-01-01 to ensure full tick coverage.

Usage:
    from ml.model2_regime.features import load_data, compute_features
    df      = load_data()
    df_feat = compute_features(df)
"""

import numpy as np
import pandas as pd
import duckdb

# ── Default paths ─────────────────────────────────────────────────────────────
DB_PATH    = "Bots_db/Algo_EURUSD.duckdb"
START_YEAR = 2013          # tick features fully available from here

# Ordered list of feature names (must stay stable across versions)
FEATURE_NAMES = [
    "log_return",       # bar log-return
    "realized_vol_20",  # 20-bar rolling std of log-returns
    "realized_vol_5",   # 5-bar rolling std (fast vol)
    "atr_ratio",        # ATR(14) / close  (normalized bar range)
    "hl_range",         # (high - low) / close
    "spread_mean_pips", # mean bid-ask spread during bar (pips)
    "return_abs_20",    # 20-bar rolling mean of |log_return|
    "vol_ratio",        # realized_vol_5 / realized_vol_20  (breakout signal)
]


# ── Data loading ──────────────────────────────────────────────────────────────

def load_data(db_path: str = DB_PATH, start_year: int = START_YEAR) -> pd.DataFrame:
    """
    Load M1 OHLCV candles joined with per-bar spread features.
    Returns a DataFrame indexed by tz-aware UTC DatetimeIndex.
    Only bars where tick_count > 0 are included (ensures spread is valid).
    """
    con     = duckdb.connect(db_path, read_only=True)
    cutoff  = int(pd.Timestamp(f"{start_year}-01-01", tz="UTC").timestamp())
    df      = con.execute(f"""
        SELECT m.timestamp, m.high, m.low, m.close,
               t.spread_mean_pips
        FROM eurusd_m1 m
        JOIN eurusd_tick_features_m1 t ON m.timestamp = t.timestamp
        WHERE m.timestamp >= {cutoff}
          AND t.tick_count > 0
        ORDER BY m.timestamp
    """).df()
    con.close()

    df["timestamp"] = pd.to_datetime(df["timestamp"], unit="s", utc=True)
    df = df.set_index("timestamp")
    print(f"Loaded {len(df):,} M1 bars  ({df.index[0]} -> {df.index[-1]})")
    return df


# ── Feature computation ───────────────────────────────────────────────────────

def compute_features(df: pd.DataFrame) -> pd.DataFrame:
    """
    Compute all 8 regime features.

    Input : OHLCV + spread_mean_pips DataFrame (from load_data)
    Output: feature-only DataFrame with FEATURE_NAMES columns.
            Warmup NaN rows (first ~20 bars) are dropped here.
    """
    out = pd.DataFrame(index=df.index)

    log_ret = np.log(df["close"] / df["close"].shift(1))

    out["log_return"]      = log_ret
    out["realized_vol_20"] = log_ret.rolling(20).std()
    out["realized_vol_5"]  = log_ret.rolling(5).std()

    # ATR(14): max of HL, |H-PrevC|, |L-PrevC|, smoothed over 14 bars
    hl  = df["high"] - df["low"]
    hpc = (df["high"] - df["close"].shift(1)).abs()
    lpc = (df["low"]  - df["close"].shift(1)).abs()
    atr = pd.concat([hl, hpc, lpc], axis=1).max(axis=1).rolling(14).mean()
    out["atr_ratio"]    = atr / (df["close"] + 1e-10)

    out["hl_range"]         = (df["high"] - df["low"]) / (df["close"] + 1e-10)
    out["spread_mean_pips"] = df["spread_mean_pips"]
    out["return_abs_20"]    = log_ret.abs().rolling(20).mean()
    out["vol_ratio"]        = out["realized_vol_5"] / (out["realized_vol_20"] + 1e-10)

    # Drop warmup rows (rolling windows produce NaN for first N bars)
    before = len(out)
    out    = out.dropna()
    print(f"Computed {out.shape[1]} features over {len(out):,} bars  "
          f"(dropped {before - len(out)} warmup rows)")
    return out[FEATURE_NAMES]
