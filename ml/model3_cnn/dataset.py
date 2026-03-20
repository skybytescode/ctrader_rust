"""
Model 3 — CNN Chart Pattern Dataset
=====================================
Extracts sliding OHLCV windows of size WINDOW_SIZE (60 M1 bars = 1 hour of M1 data)
and computes the same first-touch labels as Model 1.

Each sample:
    X : float32 array of shape (5, WINDOW_SIZE)
        channels = [open, high, low, close, volume]
        OHLC normalised relative to entry bar's close and window price range.
        Volume normalised by window mean.
    y : float32 scalar  (1.0 = target hit first, 0.0 = stop hit first, NaN = excluded)

No look-ahead: the window ending at bar i predicts bars i+1 … i+HORIZON.
"""

import sys
import numpy as np
import pandas as pd
import duckdb
from numpy.lib.stride_tricks import sliding_window_view
from pathlib import Path

PROJECT_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(PROJECT_ROOT))

from ml.model1_technical.labels import (
    compute_labels_vectorized,
    compute_short_labels_vectorized,
)

# ── Defaults ──────────────────────────────────────────────────────────────────
DB_PATH     = "Bots_db/Algo_EURUSD.duckdb"
TABLE       = "eurusd_m1"
WINDOW_SIZE = 60   # bars = 1 hour of M1 data


# ── Data loading ──────────────────────────────────────────────────────────────

def load_candles(db_path: str = DB_PATH, table: str = TABLE) -> pd.DataFrame:
    """Load M1 OHLCV from DuckDB. Returns tz-aware UTC DatetimeIndex DataFrame."""
    con = duckdb.connect(db_path, read_only=True)
    df  = con.execute(
        f"SELECT timestamp, open, high, low, close, volume "
        f"FROM {table} ORDER BY timestamp"
    ).df()
    con.close()
    df["timestamp"] = pd.to_datetime(df["timestamp"], unit="s", utc=True)
    df = df.set_index("timestamp")
    print(f"Loaded {len(df):,} M1 candles  ({df.index[0]} → {df.index[-1]})")
    return df


# ── Vectorised window extraction ──────────────────────────────────────────────

def make_windows(
    df: pd.DataFrame,
    entry_positions: np.ndarray,   # integer positions in df (0-indexed) for ENTRY bars
    window_size: int = WINDOW_SIZE,
) -> np.ndarray:
    """
    Extract OHLCV windows for the given entry-bar positions.

    Uses numpy sliding_window_view (zero-copy view) then indexes only the
    needed rows, so memory is proportional to len(entry_positions).

    Normalisation (per window, no look-ahead):
        OHLC : (value - entry_close) / window_range    → bounded, 0 = entry close
        Volume: value / window_mean                     → ratio to local mean

    Returns X of shape (len(entry_positions), 5, window_size), float32.
    """
    # Drop entry positions that don't have a full window behind them
    valid_mask       = entry_positions >= window_size - 1
    entry_positions  = entry_positions[valid_mask]

    o = df["open"].to_numpy(dtype=np.float32)
    h = df["high"].to_numpy(dtype=np.float32)
    l = df["low"].to_numpy(dtype=np.float32)
    c = df["close"].to_numpy(dtype=np.float32)
    v = df["volume"].to_numpy(dtype=np.float32)

    # sliding_window_view[j] = arr[j : j+W], entry bar at position j+W-1
    # → for entry position p, window index = p - (W-1)
    win_idx = entry_positions - (window_size - 1)

    X = np.empty((len(entry_positions), 5, window_size), dtype=np.float32)
    for ch_i, arr in enumerate([o, h, l, c, v]):
        view        = sliding_window_view(arr, window_shape=window_size)  # (N-W+1, W) view
        X[:, ch_i, :] = view[win_idx]                                     # copies needed rows

    # Vectorised normalisation
    entry_close = X[:, 3, -1]                                             # (M,) last close
    win_range   = X[:, 1, :].max(axis=1) - X[:, 2, :].min(axis=1) + 1e-8  # high.max - low.min

    for ch in range(4):   # OHLC channels
        X[:, ch, :] = (X[:, ch, :] - entry_close[:, None]) / win_range[:, None]

    vol_mean    = X[:, 4, :].mean(axis=1) + 1e-8                         # (M,) volume mean
    X[:, 4, :] /= vol_mean[:, None]

    return X


# ── Full dataset builders ─────────────────────────────────────────────────────

def _build_one_direction(
    df: pd.DataFrame,
    labels: pd.Series,
    direction: str,
    window_size: int = WINDOW_SIZE,
    session_filter: bool = True,
) -> tuple[np.ndarray, np.ndarray, pd.DatetimeIndex]:
    """
    Internal helper: extract windows + labels for one direction.

    Returns (X, y, timestamps) — only rows with a valid label are included.
    """
    n = len(df)

    # Position of each bar in the df (integer index)
    all_pos = np.arange(n)

    # Session filter mask (entry bars only — look-forward already done before calling)
    if session_filter:
        hour = df.index.hour
        sess_mask = ((hour >= 8) & (hour < 12)) | ((hour >= 13) & (hour < 17))
        entry_pos = all_pos[sess_mask]
        ts_all    = df.index[sess_mask]
    else:
        entry_pos = all_pos
        ts_all    = df.index

    # Align labels to entry timestamps
    y_all = labels.reindex(ts_all).to_numpy(dtype=np.float32)

    # Drop NaN labels (no-touch horizon bars + warm-up period)
    valid      = ~np.isnan(y_all)
    entry_pos  = entry_pos[valid]
    y          = y_all[valid]
    timestamps = ts_all[valid]

    # Extract windows only for valid positions
    print(f"  [{direction}] Extracting {len(entry_pos):,} windows "
          f"(window_size={window_size})...")
    X = make_windows(df, entry_pos, window_size)

    win_rate = y.mean() * 100
    print(f"  [{direction}] Dataset: {len(X):,} windows  |  "
          f"win-rate={win_rate:.1f}%  |  X.shape={X.shape}")

    return X, y, timestamps


def build_cnn_dataset_single(
    df: pd.DataFrame,
    direction: str,            # "long" or "short"
    window_size:    int  = WINDOW_SIZE,
    target_pips:    int  = 15,
    stop_pips:      int  = 10,
    horizon:        int  = 120,
    session_filter: bool = True,
) -> tuple:
    """
    Build CNN dataset for ONE direction only.
    Avoids holding both LONG and SHORT arrays in memory simultaneously.

    Returns (X, y, timestamps).
    """
    if direction.upper() == "LONG":
        print(f"\nComputing LONG labels  (target={target_pips}p up,   stop={stop_pips}p down)...")
        labels = compute_labels_vectorized(df, target_pips, stop_pips, horizon)
    else:
        print(f"\nComputing SHORT labels (target={target_pips}p down, stop={stop_pips}p up)...")
        labels = compute_short_labels_vectorized(df, target_pips, stop_pips, horizon)

    return _build_one_direction(df, labels, direction.upper(), window_size, session_filter)


def build_cnn_dataset_dual(
    df: pd.DataFrame,
    window_size:    int  = WINDOW_SIZE,
    target_pips:    int  = 15,
    stop_pips:      int  = 10,
    horizon:        int  = 120,
    session_filter: bool = True,
) -> tuple:
    """
    Build LONG and SHORT CNN datasets from the same raw candles.

    Returns:
        (X_long, y_long, ts_long, X_short, y_short, ts_short)

    Each X has shape (N, 5, window_size), float32.
    Each y has shape (N,), float32 — 1.0 = win, 0.0 = stop-out.
    Each ts is a pd.DatetimeIndex of entry-bar timestamps.
    """
    print(f"\nComputing LONG labels  (target={target_pips}p up,   stop={stop_pips}p down)...")
    long_labels  = compute_labels_vectorized(df, target_pips, stop_pips, horizon)
    print(f"Computing SHORT labels (target={target_pips}p down, stop={stop_pips}p up)...")
    short_labels = compute_short_labels_vectorized(df, target_pips, stop_pips, horizon)

    X_l, y_l, ts_l = _build_one_direction(
        df, long_labels,  "LONG",  window_size, session_filter)
    X_s, y_s, ts_s = _build_one_direction(
        df, short_labels, "SHORT", window_size, session_filter)

    return X_l, y_l, ts_l, X_s, y_s, ts_s
