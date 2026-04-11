"""
Model 2 — Rule-Based Regime Detection
======================================
Replaces the HMM-based regime with a simple, robust rule-based approach
using ATR percentile (volatility) + trend slope (direction).

Regime states:
  - "Trending Up"   : slope > threshold AND volatility not extreme
  - "Trending Down"  : slope < -threshold AND volatility not extreme
  - "Volatile"       : ATR percentile > 80th (regardless of direction)
  - "Ranging"        : low volatility + no clear trend

All features are backward-looking only (no look-ahead).

Usage:
    from ml.model2_regime.features import compute_regime
    regime_name, regime_id = compute_regime(df)  # df has OHLCV columns
"""

import numpy as np
import pandas as pd

# Regime state IDs
REGIME_MAP = {
    0: "Trending Down",
    1: "Ranging",
    2: "Volatile",
    3: "Trending Up",
}

REGIME_REVERSE = {v: k for k, v in REGIME_MAP.items()}

# ── Configuration ────────────────────────────────────────────────────────────

# ATR settings
ATR_PERIOD = 14
ATR_PERCENTILE_WINDOW = 500   # ~8 hours of M1 bars for percentile calc

# Trend slope: linear regression slope of close over N bars, normalized by ATR
SLOPE_PERIOD = 60             # 1-hour slope lookback
SLOPE_TREND_THRESHOLD = 0.5   # slope/ATR ratio above this = trending

# Volatility threshold: ATR percentile above this = volatile
VOLATILITY_PERCENTILE_THRESHOLD = 80


# ── Core computation ─────────────────────────────────────────────────────────

def compute_atr(df: pd.DataFrame, period: int = ATR_PERIOD) -> pd.Series:
    """Compute ATR(period) from OHLC data."""
    hl = df["high"] - df["low"]
    hpc = (df["high"] - df["close"].shift(1)).abs()
    lpc = (df["low"] - df["close"].shift(1)).abs()
    tr = pd.concat([hl, hpc, lpc], axis=1).max(axis=1)
    return tr.rolling(period).mean()


def compute_slope(close: pd.Series, period: int = SLOPE_PERIOD) -> pd.Series:
    """
    Compute linear regression slope of close prices over rolling window.
    Returns slope in price-per-bar units.
    """
    def _linreg_slope(arr):
        if len(arr) < period or np.isnan(arr).any():
            return np.nan
        x = np.arange(len(arr))
        return np.polyfit(x, arr, 1)[0]

    return close.rolling(period).apply(_linreg_slope, raw=True)


def compute_regime_series(df: pd.DataFrame) -> pd.DataFrame:
    """
    Compute regime for every bar in the DataFrame.

    Args:
        df: DataFrame with columns [open, high, low, close, volume]
            and a DatetimeIndex.

    Returns:
        DataFrame with columns [regime_name, regime_id, atr, atr_percentile,
        slope_norm] aligned to input index.
    """
    atr = compute_atr(df, ATR_PERIOD)

    # ATR percentile: where does current ATR sit relative to recent history?
    atr_pct = atr.rolling(ATR_PERCENTILE_WINDOW, min_periods=50).apply(
        lambda x: pd.Series(x).rank(pct=True).iloc[-1] * 100, raw=True
    )

    # Trend slope normalized by ATR (unit-free directional strength)
    slope = compute_slope(df["close"], SLOPE_PERIOD)
    slope_norm = slope / (atr + 1e-10)

    # Classify regime
    regime_id = pd.Series(1, index=df.index, dtype=int)  # default: Ranging

    # Volatile: ATR percentile > threshold (checked first, overrides trend)
    volatile_mask = atr_pct > VOLATILITY_PERCENTILE_THRESHOLD
    regime_id[volatile_mask] = 2

    # Trending: slope exceeds threshold AND not volatile
    trending_up = (slope_norm > SLOPE_TREND_THRESHOLD) & ~volatile_mask
    trending_down = (slope_norm < -SLOPE_TREND_THRESHOLD) & ~volatile_mask
    regime_id[trending_up] = 3
    regime_id[trending_down] = 0

    regime_name = regime_id.map(REGIME_MAP)

    result = pd.DataFrame({
        "regime_name": regime_name,
        "regime_id": regime_id,
        "atr": atr,
        "atr_percentile": atr_pct,
        "slope_norm": slope_norm,
    }, index=df.index)

    return result.dropna()


def compute_regime(df: pd.DataFrame) -> tuple[str, int]:
    """
    Compute regime for the latest bar only.
    Returns (regime_name, regime_id) for the most recent bar.
    """
    result = compute_regime_series(df)
    if result.empty:
        return "Ranging", 1

    last = result.iloc[-1]
    return str(last["regime_name"]), int(last["regime_id"])


def compute_features(df: pd.DataFrame) -> pd.DataFrame:
    """
    Backward-compatible interface for predict_all.py.
    Returns a DataFrame with regime features for the latest bar.
    """
    result = compute_regime_series(df)
    if result.empty:
        return pd.DataFrame()
    return result.iloc[[-1]]
