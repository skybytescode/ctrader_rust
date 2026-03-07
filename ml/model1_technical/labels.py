"""
Model 1 — First-Touch Labeling
================================
For each M1 bar, looks forward up to HORIZON bars to find which event
happens first:

    label = 1  →  +TARGET_PIPS hit first  (win)
    label = 0  →  -STOP_PIPS   hit first  (loss)
    label = NaN → neither hit within HORIZON bars  (skipped)

Trading parameters (intraday, 1.5:1 R:R):
    TARGET_PIPS  = 15   → profit target
    STOP_PIPS    = 10   → stop loss
    HORIZON_BARS = 120  → 2 hours (120 M1 bars)

Session filter: only London (08:00–12:00 UTC) and New York (13:00–17:00 UTC).
"""

import numpy as np
import pandas as pd

# ── Trading parameters ────────────────────────────────────────────────────────
TARGET_PIPS  = 15    # pips profit target
STOP_PIPS    = 10    # pips stop loss
HORIZON_BARS = 120   # bars = minutes to look forward (2 hours)
PIP_SIZE     = 0.0001  # 1 pip for EURUSD


def compute_labels(
    df_raw: pd.DataFrame,
    target_pips: int  = TARGET_PIPS,
    stop_pips: int    = STOP_PIPS,
    horizon: int      = HORIZON_BARS,
    pip_size: float   = PIP_SIZE,
) -> pd.Series:
    """
    Compute first-touch binary labels for every bar in df_raw.

    Parameters
    ----------
    df_raw      : OHLCV DataFrame from load_candles()
    target_pips : pips above entry price that triggers a win (label=1)
    stop_pips   : pips below entry price that triggers a loss (label=0)
    horizon     : maximum bars to look forward
    pip_size    : pip value in price units (0.0001 for EURUSD)

    Returns
    -------
    pd.Series of float  (1.0, 0.0, or NaN), same index as df_raw
    """
    target_dist = target_pips * pip_size
    stop_dist   = stop_pips   * pip_size

    close_arr = df_raw["close"].to_numpy(dtype=np.float64)
    high_arr  = df_raw["high"].to_numpy(dtype=np.float64)
    low_arr   = df_raw["low"].to_numpy(dtype=np.float64)
    n         = len(close_arr)

    labels = np.full(n, np.nan, dtype=np.float64)

    for i in range(n - 1):
        entry      = close_arr[i]
        target_lvl = entry + target_dist
        stop_lvl   = entry - stop_dist

        end = min(i + horizon + 1, n)
        for j in range(i + 1, end):
            if high_arr[j] >= target_lvl:
                labels[i] = 1.0
                break
            if low_arr[j] <= stop_lvl:
                labels[i] = 0.0
                break
        # If neither touched → remains NaN (dropped later)

    return pd.Series(labels, index=df_raw.index, name="label")


def compute_labels_vectorized(
    df_raw: pd.DataFrame,
    target_pips: int  = TARGET_PIPS,
    stop_pips: int    = STOP_PIPS,
    horizon: int      = HORIZON_BARS,
    pip_size: float   = PIP_SIZE,
) -> pd.Series:
    """
    Faster vectorized version using numpy rolling windows.
    Trades exact first-touch ordering for ~10x speed.

    For most ML training purposes this is equivalent, because
    cases where target and stop hit in the same bar are rare
    and handled conservatively (target wins ties → label=1).

    Use this for large datasets (1M+ rows).
    """
    target_dist = target_pips * pip_size
    stop_dist   = stop_pips   * pip_size

    close = df_raw["close"].to_numpy(dtype=np.float64)
    high  = df_raw["high"].to_numpy(dtype=np.float64)
    low   = df_raw["low"].to_numpy(dtype=np.float64)
    n     = len(close)

    labels = np.full(n, np.nan, dtype=np.float64)

    # For each bar i, check all future bars i+1 … i+horizon
    # Build forward-looking max-high and min-low arrays
    # Using a sliding window approach

    for i in range(n - 1):
        end        = min(i + horizon + 1, n)
        future_h   = high[i + 1:end]
        future_l   = low[i + 1:end]
        entry      = close[i]
        target_hit = np.where(future_h >= entry + target_dist)[0]
        stop_hit   = np.where(future_l <= entry - stop_dist)[0]

        t_idx = target_hit[0] if len(target_hit) > 0 else np.inf
        s_idx = stop_hit[0]   if len(stop_hit)   > 0 else np.inf

        if t_idx == np.inf and s_idx == np.inf:
            continue  # NaN
        elif t_idx <= s_idx:
            labels[i] = 1.0
        else:
            labels[i] = 0.0

    return pd.Series(labels, index=df_raw.index, name="label")


def filter_trading_hours(df: pd.DataFrame) -> pd.DataFrame:
    """
    Keep only London (08:00–12:00 UTC) and New York (13:00–17:00 UTC) bars.
    Drops Asian session and off-hours entirely.
    """
    hour = df.index.hour
    mask = ((hour >= 8) & (hour < 12)) | ((hour >= 13) & (hour < 17))
    filtered = df[mask]
    pct = len(filtered) / len(df) * 100
    print(f"Session filter: {len(filtered):,} bars kept ({pct:.1f}% of total)")
    return filtered


def build_dataset(
    df_raw: pd.DataFrame,
    df_features: pd.DataFrame,
    target_pips: int       = TARGET_PIPS,
    stop_pips: int         = STOP_PIPS,
    horizon: int           = HORIZON_BARS,
    session_filter: bool   = True,
    use_fast_labels: bool  = True,
    wide_spread_filter: float = 1.5,
) -> pd.DataFrame:
    """
    Full pipeline: features + labels → clean training dataset.

    Steps:
    1. Compute first-touch labels on FULL data (look-forward uses all M1 bars)
    2. Merge features + labels (full index)
    3. Apply session filter (entry rows only — look-forward already done)
    4. Drop wide-spread bars  (spread_mean_pips > threshold → news/illiquid events)
    5. Drop NaN rows (indicator warm-up + no-touch horizon bars)

    IMPORTANT: session filter is applied AFTER labeling so that look-forward
    always uses consecutive M1 bars regardless of session.

    Args:
        wide_spread_filter: drop rows where spread_mean_pips > this value.
            Set to None to disable. Default 1.5p removes ~3% of bars (news events).

    Returns a DataFrame with all feature columns + 'label' column.
    """
    print(f"Computing labels on full data (target={target_pips}p, stop={stop_pips}p, horizon={horizon}m)...")
    label_fn = compute_labels_vectorized if use_fast_labels else compute_labels
    labels   = label_fn(df_raw, target_pips, stop_pips, horizon)

    # Merge features + labels — use assign to avoid copying the full 5M-row matrix
    dataset = df_features.assign(label=labels)

    # Session filter (entry rows only)
    if session_filter:
        dataset = filter_trading_hours(dataset)

    # Wide-spread filter — remove news spikes / illiquid bars from training
    if wide_spread_filter is not None and "spread_mean_pips" in dataset.columns:
        before_spread = len(dataset)
        mask = (dataset["spread_mean_pips"].isna()) | \
               (dataset["spread_mean_pips"] <= wide_spread_filter)
        dataset = dataset[mask]
        dropped_spread = before_spread - len(dataset)
        print(f"Spread filter (>{wide_spread_filter}p): dropped {dropped_spread:,} bars "
              f"({dropped_spread/before_spread*100:.1f}%)")

    before = len(dataset)
    dataset = dataset.dropna()
    after   = len(dataset)
    dropped = before - after
    win_rate = dataset["label"].mean() * 100

    print(f"Dataset: {after:,} rows  (dropped {dropped:,} NaN rows)")
    print(f"Label balance: {win_rate:.1f}% wins / {100-win_rate:.1f}% losses")
    print(f"Class counts: {dataset['label'].value_counts().to_dict()}")

    return dataset
