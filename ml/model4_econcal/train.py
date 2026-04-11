"""
Model 4 — Economic Calendar Event Filter (Validation)
======================================================
This is now a rule-based filter, not an ML model.
The train script validates filter behavior on historical data
and saves metrics for UI display.

Run from the project root:
    python -m ml.model4_econcal.train

Outputs:
    ml/trained/model4_metrics.json  <- filter statistics
"""

import json
import sys
import time
from pathlib import Path

import numpy as np
import pandas as pd

PROJECT_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(PROJECT_ROOT))

from ml.model4_econcal.features import (
    load_ec_events, check_event_filter,
    EVENT_FILTER_MINUTES, MIN_VOLATILITY_FILTER,
)

CONFIG = {
    "db_path":   "Bots_db/Algo_EURUSD.duckdb",
    "model_dir": "ml/trained",
}


def train():
    """Validate event filter on historical data and save metrics."""
    t0 = time.time()
    print("=" * 60)
    print("Model 4 — Economic Calendar Event Filter (Validation)")
    print("=" * 60)

    # 1. Load events
    print(f"\n[1/2] Loading economic calendar events...")
    df_ec = load_ec_events(CONFIG["db_path"])
    if df_ec.empty:
        print("No economic calendar data found. Skipping.")
        return

    total_events = len(df_ec)
    high_impact = df_ec[df_ec["volatility"] >= MIN_VOLATILITY_FILTER]
    print(f"  Total events: {total_events:,}")
    print(f"  High-impact (vol>={MIN_VOLATILITY_FILTER}): {len(high_impact):,}")

    # Count named events
    name_lower = high_impact["event_name"].str.lower()
    named_counts = {}
    from ml.model4_econcal.features import _NAMED_EVENTS
    for tag, patterns in _NAMED_EVENTS.items():
        mask = name_lower.str.contains("|".join(patterns), regex=True)
        named_counts[tag] = int(mask.sum())
        print(f"    {tag.upper()}: {named_counts[tag]} events")

    # 2. Estimate filter coverage
    print(f"\n[2/2] Estimating filter coverage...")
    # How many minutes per day are blocked on average?
    if not high_impact.empty:
        # Group by date
        event_dates = high_impact.index.date
        unique_dates = set(event_dates)
        events_per_day = len(high_impact) / max(len(unique_dates), 1)
        blocked_mins_per_event = EVENT_FILTER_MINUTES * 2  # before + after
        avg_blocked_per_day = events_per_day * blocked_mins_per_event
        trading_mins_per_day = 16 * 60  # ~16h active trading
        pct_blocked = (avg_blocked_per_day / trading_mins_per_day) * 100

        print(f"  Unique event days: {len(unique_dates):,}")
        print(f"  Avg high-impact events/day: {events_per_day:.1f}")
        print(f"  Avg blocked mins/day: {avg_blocked_per_day:.0f}")
        print(f"  % trading time blocked: {pct_blocked:.1f}%")
    else:
        events_per_day = 0
        avg_blocked_per_day = 0
        pct_blocked = 0

    # Save metrics
    model_dir = Path(CONFIG["model_dir"])
    model_dir.mkdir(parents=True, exist_ok=True)

    metrics_path = model_dir / "model4_metrics.json"
    metrics = {
        "model_type": "Rule-Based Event Filter",
        "description": f"No trading within +/-{EVENT_FILTER_MINUTES}min of high-impact events (vol>={MIN_VOLATILITY_FILTER})",
        "parameters": {
            "filter_window_minutes": EVENT_FILTER_MINUTES,
            "min_volatility": MIN_VOLATILITY_FILTER,
        },
        "total_events": total_events,
        "high_impact_events": len(high_impact),
        "named_event_counts": named_counts,
        "date_range": [
            df_ec.index[0].isoformat(),
            df_ec.index[-1].isoformat(),
        ],
        "avg_events_per_day": round(events_per_day, 2),
        "avg_blocked_mins_per_day": round(avg_blocked_per_day, 1),
        "pct_trading_time_blocked": round(pct_blocked, 1),
    }
    metrics_path.write_text(json.dumps(metrics, indent=2))

    elapsed = time.time() - t0
    print(f"\nSaved {metrics_path}")
    print(f"Done in {elapsed:.0f}s")


def load_model(model_dir: str = "ml/trained", direction: str = "long"):
    """
    Backward-compatible interface. Event filter has no model to load.
    Returns (None, []) to signal that this is now a filter, not a model.
    """
    return None, []


if __name__ == "__main__":
    train()
