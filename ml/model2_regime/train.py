"""
Model 2 — Regime Detection (GaussianHMM) Training
===================================================
Run from the project root:

    python -m ml.model2_regime.train

Outputs:
    ml/trained/model2_regime.pkl       <- GaussianHMM + StandardScaler (joblib)
    ml/trained/model2_state_map.json   <- {state_id: regime_label}
    ml/trained/model2_metrics.json     <- fit quality + per-state statistics
"""

import gc
import json
import sys
import time
from pathlib import Path

import joblib
import numpy as np
import pandas as pd
from hmmlearn.hmm import GaussianHMM
from sklearn.preprocessing import StandardScaler

PROJECT_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(PROJECT_ROOT))

from ml.model2_regime.features import load_data, compute_features, FEATURE_NAMES

# ── Configuration ──────────────────────────────────────────────────────────────

CONFIG = {
    "db_path":    "Bots_db/Algo_EURUSD.duckdb",
    "start_year": 2013,
    "n_states":   4,
    "n_iter":     100,
    "tol":        1e-5,
    "n_restarts": 5,      # multiple EM restarts → pick best log-likelihood
    "stride":     3,      # use every Nth bar (reduces ~4.8M → ~1.6M, still robust)
    "model_dir":  "ml/trained",
}


# ── State label assignment ─────────────────────────────────────────────────────

def assign_state_labels(
    model: GaussianHMM,
    scaler: StandardScaler,
) -> tuple[dict[int, str], np.ndarray]:
    """
    Assign interpretable labels to HMM states from their learned means.

    Strategy:
        1. Inverse-transform means back to original feature scale.
        2. Sort states by mean log_return (ascending).
        3. Lowest mean_return  → "Trending Down"
           Highest mean_return → "Trending Up"
        4. For the two middle states: higher realized_vol_20 → "Volatile",
           lower → "Ranging".

    Returns (state_map, means_raw) where means_raw has shape (n_states, n_features).
    """
    means_raw = scaler.inverse_transform(model.means_)

    ret_idx = FEATURE_NAMES.index("log_return")
    vol_idx = FEATURE_NAMES.index("realized_vol_20")

    sorted_by_ret = np.argsort(means_raw[:, ret_idx])   # ascending

    state_map: dict[int, str] = {}
    state_map[int(sorted_by_ret[0])]  = "Trending Down"
    state_map[int(sorted_by_ret[-1])] = "Trending Up"

    mid = sorted_by_ret[1:-1]
    if len(mid) == 2:
        if means_raw[mid[0], vol_idx] > means_raw[mid[1], vol_idx]:
            state_map[int(mid[0])] = "Volatile"
            state_map[int(mid[1])] = "Ranging"
        else:
            state_map[int(mid[0])] = "Ranging"
            state_map[int(mid[1])] = "Volatile"
    elif len(mid) == 1:
        state_map[int(mid[0])] = "Ranging"

    return state_map, means_raw


# ── Per-state statistics ───────────────────────────────────────────────────────

def compute_state_stats(
    states: np.ndarray,
    means_raw: np.ndarray,
    state_map: dict[int, str],
) -> list[dict]:
    """Compute pct_bars and average run-length per state."""
    n   = len(states)
    ret_idx = FEATURE_NAMES.index("log_return")
    vol_idx = FEATURE_NAMES.index("realized_vol_20")

    stats = []
    for sid, label in state_map.items():
        mask = states == sid
        pct  = float(mask.sum()) / n * 100.0

        # Run-length (consecutive bars in this state)
        runs, count = [], 0
        for v in mask:
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
            "state_id":          sid,
            "label":             label,
            "mean_return_pips":  round(float(means_raw[sid, ret_idx]) * 10_000, 4),
            "mean_vol_20":       round(float(means_raw[sid, vol_idx]), 8),
            "pct_bars":          round(pct, 2),
            "avg_duration_bars": round(avg_dur, 1),
        })

    stats.sort(key=lambda x: x["state_id"])
    return stats


# ── Main training pipeline ─────────────────────────────────────────────────────

def train():
    t0 = time.time()
    print("=" * 60)
    print("Model 2 — Regime Detection (GaussianHMM) Training")
    print("=" * 60)

    # 1. Load
    print(f"\n[1/4] Loading data (start_year={CONFIG['start_year']})...")
    df = load_data(CONFIG["db_path"], CONFIG["start_year"])

    # 2. Features
    print("\n[2/4] Computing regime features...")
    df_feat = compute_features(df)
    del df
    gc.collect()

    stride = CONFIG["stride"]
    if stride > 1:
        df_feat = df_feat.iloc[::stride]
        print(f"  Subsampled every {stride} bars -> {len(df_feat):,} observations")

    X        = df_feat[FEATURE_NAMES].to_numpy(dtype=np.float64)
    scaler   = StandardScaler()
    X_scaled = scaler.fit_transform(X)

    # 3. Fit HMM with multiple restarts
    print(f"\n[3/4] Fitting GaussianHMM  "
          f"(n_states={CONFIG['n_states']}, n_iter={CONFIG['n_iter']}, "
          f"n_restarts={CONFIG['n_restarts']})...")

    best_model: GaussianHMM | None = None
    best_score = -np.inf

    for restart in range(CONFIG["n_restarts"]):
        seed = 42 + restart * 17
        print(f"  Restart {restart + 1}/{CONFIG['n_restarts']}  (seed={seed})...", flush=True)
        m = GaussianHMM(
            n_components    = CONFIG["n_states"],
            covariance_type = "full",
            n_iter          = CONFIG["n_iter"],
            tol             = CONFIG["tol"],
            random_state    = seed,
            verbose         = False,
        )
        try:
            m.fit(X_scaled)
            score = m.score(X_scaled)
            converged = m.monitor_.converged
            print(f"    log-likelihood: {score:,.2f}  (converged: {converged})")
            if score > best_score:
                best_score = score
                best_model = m
        except Exception as e:
            print(f"    FAILED: {e}")

    if best_model is None:
        print("ERROR: all restarts failed — aborting.")
        return

    print(f"\n  Best log-likelihood: {best_score:,.2f}")

    # Label states
    state_map, means_raw = assign_state_labels(best_model, scaler)
    print("\n  State assignments:")
    for sid in sorted(state_map):
        label    = state_map[sid]
        ret_pips = means_raw[sid, FEATURE_NAMES.index("log_return")] * 10_000
        vol_val  = means_raw[sid, FEATURE_NAMES.index("realized_vol_20")]
        print(f"    State {sid}: {label:<16}  "
              f"mean_return={ret_pips:+.4f}p  mean_vol20={vol_val:.7f}")

    # Decode full training set
    print("\n  Decoding states on training data...", flush=True)
    states      = best_model.predict(X_scaled)
    state_stats = compute_state_stats(states, means_raw, state_map)

    print("\n  State distribution:")
    for s in state_stats:
        print(f"    {s['label']:<16}: {s['pct_bars']:5.1f}% of bars  "
              f"avg_duration={s['avg_duration_bars']:.0f} bars")

    # AIC / BIC
    n      = len(X_scaled)
    n_s    = CONFIG["n_states"]
    n_f    = len(FEATURE_NAMES)
    n_params = (
        n_s * (n_s - 1)               # transition matrix
        + (n_s - 1)                    # startprob
        + n_s * n_f                    # means
        + n_s * n_f * (n_f + 1) // 2  # full covariances
    )
    aic = -2 * best_score + 2 * n_params
    bic = -2 * best_score + np.log(n) * n_params
    print(f"\n  AIC: {aic:,.2f}  BIC: {bic:,.2f}  n_params: {n_params}")

    # 4. Save
    print(f"\n[4/4] Saving outputs...")
    model_dir = Path(CONFIG["model_dir"])
    model_dir.mkdir(parents=True, exist_ok=True)

    pkl_path     = model_dir / "model2_regime.pkl"
    map_path     = model_dir / "model2_state_map.json"
    metrics_path = model_dir / "model2_metrics.json"

    joblib.dump({"model": best_model, "scaler": scaler}, str(pkl_path))

    state_map_str = {str(k): v for k, v in state_map.items()}
    map_path.write_text(json.dumps(state_map_str, indent=2))

    metrics = {
        "model_type":     "GaussianHMM",
        "covariance_type":"full",
        "n_states":       CONFIG["n_states"],
        "n_features":     len(FEATURE_NAMES),
        "feature_names":  FEATURE_NAMES,
        "stride":         stride,
        "train_bars":     n,
        "train_range":    [
            df_feat.index[0].isoformat(),
            df_feat.index[-1].isoformat(),
        ],
        "n_restarts":     CONFIG["n_restarts"],
        "n_iter":         CONFIG["n_iter"],
        "log_likelihood": round(float(best_score), 4),
        "aic":            round(aic, 4),
        "bic":            round(bic, 4),
        "n_params":       n_params,
        "state_map":      state_map_str,
        "state_stats":    state_stats,
    }
    metrics_path.write_text(json.dumps(metrics, indent=2))

    elapsed = time.time() - t0
    print(f"\nSaved {pkl_path}")
    print(f"Saved {map_path}")
    print(f"Saved {metrics_path}")
    print(f"\nDone in {elapsed:.0f}s")

    return best_model, scaler, state_map


# ── Inference helper ───────────────────────────────────────────────────────────

def load_model(model_dir: str = "ml/trained"):
    """Load saved Model 2 for inference. Returns (model, scaler, state_map)."""
    d    = Path(model_dir)
    pkg  = joblib.load(str(d / "model2_regime.pkl"))
    smap = json.loads((d / "model2_state_map.json").read_text())
    smap = {int(k): v for k, v in smap.items()}
    return pkg["model"], pkg["scaler"], smap


if __name__ == "__main__":
    train()
