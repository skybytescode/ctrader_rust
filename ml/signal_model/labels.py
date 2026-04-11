"""
Signal Model — ATR-Based Target Engineering
=============================================
Instead of fixed pip targets, uses volatility-adjusted labels:

    Label = 1  →  Price moves > 1.5 ATR in the predicted direction
                  within HORIZON bars, BEFORE hitting 1.0 ATR stop.
    Label = 0  →  Stop hit first, or neither target/stop hit.

This answers the actual trading question:
    "Will price hit 2R before 1R?" (with 1.5:1 reward-risk ratio)

Session filter: London (07:00-16:00 UTC) + NY (13:00-21:00 UTC).
"""

import numpy as np
import pandas as pd

# ── Trading parameters ──────────────────────────────────────────────────────
ATR_PERIOD = 14
TARGET_ATR_MULT = 1.5   # target = 1.5 × ATR (reward)
STOP_ATR_MULT = 1.0     # stop   = 1.0 × ATR (risk)
HORIZON_BARS = 120      # 2 hours lookback

# ── Numba JIT (optional) ────────────────────────────────────────────────────
try:
    import numba

    @numba.jit(nopython=True, cache=True)
    def _jit_atr_labels_long(close, high, low, atr, target_mult, stop_mult, horizon):
        n = len(close)
        labels = np.empty(n, dtype=np.float64)
        for i in range(n):
            labels[i] = np.nan
        for i in range(n - 1):
            if np.isnan(atr[i]) or atr[i] < 1e-10:
                continue
            entry = close[i]
            target_lvl = entry + atr[i] * target_mult
            stop_lvl = entry - atr[i] * stop_mult
            end = min(i + horizon + 1, n)
            for j in range(i + 1, end):
                if high[j] >= target_lvl:
                    labels[i] = 1.0
                    break
                if low[j] <= stop_lvl:
                    labels[i] = 0.0
                    break
        return labels

    @numba.jit(nopython=True, cache=True)
    def _jit_atr_labels_short(close, high, low, atr, target_mult, stop_mult, horizon):
        n = len(close)
        labels = np.empty(n, dtype=np.float64)
        for i in range(n):
            labels[i] = np.nan
        for i in range(n - 1):
            if np.isnan(atr[i]) or atr[i] < 1e-10:
                continue
            entry = close[i]
            target_lvl = entry - atr[i] * target_mult
            stop_lvl = entry + atr[i] * stop_mult
            end = min(i + horizon + 1, n)
            for j in range(i + 1, end):
                if low[j] <= target_lvl:
                    labels[i] = 1.0
                    break
                if high[j] >= stop_lvl:
                    labels[i] = 0.0
                    break
        return labels

    _NUMBA_AVAILABLE = True
    print("signal_model/labels.py: numba JIT available")

except ImportError:
    _NUMBA_AVAILABLE = False
    print("signal_model/labels.py: numba not available — using pure Python (slower)")


def _compute_atr(high, low, close, period=ATR_PERIOD):
    """Compute ATR series."""
    hl = high - low
    hpc = (high - close.shift(1)).abs()
    lpc = (low - close.shift(1)).abs()
    tr = pd.concat([hl, hpc, lpc], axis=1).max(axis=1)
    return tr.rolling(period).mean()


def _pure_python_labels(close, high, low, atr, target_mult, stop_mult, horizon, direction="long"):
    """Fallback label computation without numba."""
    n = len(close)
    labels = np.full(n, np.nan)
    for i in range(n - 1):
        if np.isnan(atr[i]) or atr[i] < 1e-10:
            continue
        entry = close[i]
        if direction == "long":
            target_lvl = entry + atr[i] * target_mult
            stop_lvl = entry - atr[i] * stop_mult
        else:
            target_lvl = entry - atr[i] * target_mult
            stop_lvl = entry + atr[i] * stop_mult
        end = min(i + horizon + 1, n)
        for j in range(i + 1, end):
            if direction == "long":
                if high[j] >= target_lvl:
                    labels[i] = 1.0
                    break
                if low[j] <= stop_lvl:
                    labels[i] = 0.0
                    break
            else:
                if low[j] <= target_lvl:
                    labels[i] = 1.0
                    break
                if high[j] >= stop_lvl:
                    labels[i] = 0.0
                    break
    return labels


def build_labels(df: pd.DataFrame, direction: str = "long") -> pd.Series:
    """
    Build ATR-based first-touch labels for the given direction.

    Args:
        df: DataFrame with [open, high, low, close, volume] and DatetimeIndex
        direction: "long" or "short"

    Returns:
        Series of labels (1.0=win, 0.0=loss, NaN=undecided)
    """
    atr = _compute_atr(df["high"], df["low"], df["close"], ATR_PERIOD)
    atr_vals = atr.values
    close = df["close"].values
    high = df["high"].values
    low = df["low"].values

    if _NUMBA_AVAILABLE:
        if direction == "long":
            labels = _jit_atr_labels_long(close, high, low, atr_vals,
                                          TARGET_ATR_MULT, STOP_ATR_MULT, HORIZON_BARS)
        else:
            labels = _jit_atr_labels_short(close, high, low, atr_vals,
                                           TARGET_ATR_MULT, STOP_ATR_MULT, HORIZON_BARS)
    else:
        labels = _pure_python_labels(close, high, low, atr_vals,
                                     TARGET_ATR_MULT, STOP_ATR_MULT, HORIZON_BARS, direction)

    return pd.Series(labels, index=df.index, name=f"label_{direction}")


def session_filter(df: pd.DataFrame) -> pd.Series:
    """
    Return boolean mask for tradeable sessions.
    London: 07:00-16:00 UTC, New York: 13:00-21:00 UTC
    Combined: 07:00-21:00 UTC (covers both sessions + overlap)
    """
    hour = df.index.hour
    return (hour >= 7) & (hour < 21)
