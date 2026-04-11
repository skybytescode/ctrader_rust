"""
Model 2 — Rule-Based Regime Detection (no ML training needed)
==============================================================
This script validates the rule-based regime on historical data and saves
metrics for the UI status display.

Run from the project root:
    python -m ml.model2_regime.train

Outputs:
    ml/trained/model2_state_map.json   <- {state_id: regime_label}
    ml/trained/model2_metrics.json     <- regime distribution statistics
"""

import json
import sys
import time
from pathlib import Path

import numpy as np
import pandas as pd
import duckdb

PROJECT_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(PROJECT_ROOT))

from ml.model2_regime.features import (
    compute_regime_series, REGIME_MAP,
    ATR_PERIOD, ATR_PERCENTILE_WINDOW, SLOPE_PERIOD,
    SLOPE_TREND_THRESHOLD, VOLATILITY_PERCENTILE_THRESHOLD,
)

CONFIG = {
    "db_path":    "Bots_db/Algo_EURUSD.duckdb",
    "start_year": 2013,
    "model_dir":  "ml/trained",
}


def load_data(db_path: str, start_year: int) -> pd.DataFrame:
    """Load M1 OHLCV candles from DuckDB."""
    con = duckdb.connect(db_path, read_only=True)
    cutoff = int(pd.Timestamp(f"{start_year}-01-01", tz="UTC").timestamp())
    df = con.execute(f"""
        SELECT timestamp, open, high, low, close, volume
        FROM eurusd_m1
        WHERE timestamp >= {cutoff}
        ORDER BY timestamp
    """).df()
    con.close()

    df["timestamp"] = pd.to_datetime(df["timestamp"], unit="s", utc=True)
    df = df.set_index("timestamp")
    print(f"Loaded {len(df):,} M1 bars  ({df.index[0]} -> {df.index[-1]})")
    return df


def compute_state_stats(regime_df: pd.DataFrame) -> list[dict]:
    """Compute percentage and average duration per regime state."""
    n = len(regime_df)
    stats = []

    for state_id, label in REGIME_MAP.items():
        mask = regime_df["regime_id"] == state_id
        pct = float(mask.sum()) / n * 100.0

        # Run-length (consecutive bars in this state)
        runs, count = [], 0
        for v in mask.values:
            if v:
                count += 1
            else:
                if count:
                    runs.append(count)
                count = 0
        if count:
            runs.append(count)
        avg_dur = float(np.mean(runs)) if runs else 0.0

        stats.append({
            "state_id": state_id,
            "label": label,
            "pct_bars": round(pct, 2),
            "avg_duration_bars": round(avg_dur, 1),
        })

    stats.sort(key=lambda x: x["state_id"])
    return stats


def train():
    """Validate regime detection on historical data and save metrics."""
    t0 = time.time()
    print("=" * 60)
    print("Model 2 — Rule-Based Regime Detection (Validation)")
    print("=" * 60)

    # 1. Load data
    print(f"\n[1/3] Loading data (start_year={CONFIG['start_year']})...")
    df = load_data(CONFIG["db_path"], CONFIG["start_year"])

    # 2. Compute regimes
    print("\n[2/3] Computing regimes...")
    regime_df = compute_regime_series(df)
    print(f"  Computed regimes for {len(regime_df):,} bars")

    # State distribution
    state_stats = compute_state_stats(regime_df)
    print("\n  State distribution:")
    for s in state_stats:
        print(f"    {s['label']:<16}: {s['pct_bars']:5.1f}% of bars  "
              f"avg_duration={s['avg_duration_bars']:.0f} bars")

    # 3. Save
    print(f"\n[3/3] Saving outputs...")
    model_dir = Path(CONFIG["model_dir"])
    model_dir.mkdir(parents=True, exist_ok=True)

    map_path = model_dir / "model2_state_map.json"
    metrics_path = model_dir / "model2_metrics.json"

    state_map_str = {str(k): v for k, v in REGIME_MAP.items()}
    map_path.write_text(json.dumps(state_map_str, indent=2))

    metrics = {
        "model_type": "Rule-Based (ATR percentile + trend slope)",
        "n_states": len(REGIME_MAP),
        "parameters": {
            "atr_period": ATR_PERIOD,
            "atr_percentile_window": ATR_PERCENTILE_WINDOW,
            "slope_period": SLOPE_PERIOD,
            "slope_trend_threshold": SLOPE_TREND_THRESHOLD,
            "volatility_percentile_threshold": VOLATILITY_PERCENTILE_THRESHOLD,
        },
        "train_bars": len(regime_df),
        "train_range": [
            regime_df.index[0].isoformat(),
            regime_df.index[-1].isoformat(),
        ],
        "state_map": state_map_str,
        "state_stats": state_stats,
    }
    metrics_path.write_text(json.dumps(metrics, indent=2))

    elapsed = time.time() - t0
    print(f"\nSaved {map_path}")
    print(f"Saved {metrics_path}")
    print(f"\nDone in {elapsed:.0f}s")


# ── Inference helper (backward-compatible) ──────────────────────────────────

def load_model(model_dir: str = "ml/trained"):
    """
    Backward-compatible load. Returns (None, None, state_map).
    The rule-based regime doesn't need a saved model — it computes live.
    """
    d = Path(model_dir)
    smap = json.loads((d / "model2_state_map.json").read_text())
    smap = {int(k): v for k, v in smap.items()}
    return None, None, smap


if __name__ == "__main__":
    train()
