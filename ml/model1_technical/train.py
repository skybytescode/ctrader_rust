"""
Model 1 — XGBoost Training with Walk-Forward Validation
=========================================================
Run from the project root:

    python -m ml.model1_technical.train

Or directly:

    cd c:/Users/kushn/RustProjects/ctrader_rust
    python ml/model1_technical/train.py

Outputs:
    ml/trained/model1_technical.json   ← final model trained on all data
    ml/trained/model1_feature_names.txt
    ml/trained/model1_metrics.json     ← walk-forward CV results
"""

import gc
import json
import sys
import time
from pathlib import Path

import numpy as np
import pandas as pd
import xgboost as xgb
from sklearn.metrics import (
    accuracy_score,
    classification_report,
    log_loss,
    precision_score,
    recall_score,
    roc_auc_score,
)

# Add project root to path when running directly
PROJECT_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(PROJECT_ROOT))

from ml.model1_technical.features import (
    load_candles, load_tick_features, add_tick_features, compute_features,
)
from ml.model1_technical.labels import build_dataset

# ── Configuration ─────────────────────────────────────────────────────────────

CONFIG = {
    # Data
    "db_path":       "Bots_db/Algo_EURUSD.duckdb",
    "table":         "eurusd_m1",

    # Label parameters
    "target_pips":   15,
    "stop_pips":     10,
    "horizon_bars":  120,     # 2 hours

    # Walk-forward: number of yearly folds
    "n_folds":       4,       # e.g. test on 2022, 2023, 2024, 2025

    # Decision threshold for precision/recall (not for training)
    "threshold":     0.55,

    # XGBoost hyperparameters
    "xgb_params": {
        "n_estimators":       500,
        "max_depth":          5,
        "learning_rate":      0.05,
        "subsample":          0.8,
        "colsample_bytree":   0.8,
        "min_child_weight":   10,   # prevents overfitting on small groups
        "reg_alpha":          0.1,  # L1 regularization
        "reg_lambda":         1.0,  # L2 regularization
        "scale_pos_weight":   1.0,  # adjust if class imbalance > 60/40
        "eval_metric":        "logloss",
        "early_stopping_rounds": 30,
        "random_state":       42,
        "n_jobs":             -1,
        "verbosity":          0,
    },

    # Output
    "model_dir":     "ml/trained",
}


# ── Walk-forward validation ───────────────────────────────────────────────────

def walk_forward_splits(dataset: pd.DataFrame, n_folds: int):
    """
    Expanding-window walk-forward splits.

    Example with n_folds=4 and data 2010–2025:
        Fold 0: train 2010–2021, test 2022
        Fold 1: train 2010–2022, test 2023
        Fold 2: train 2010–2023, test 2024
        Fold 3: train 2010–2024, test 2025

    Returns list of (train_idx, test_idx, test_year) tuples.
    """
    years = sorted(dataset.index.year.unique())
    if len(years) < n_folds + 1:
        raise ValueError(
            f"Not enough years ({len(years)}) for {n_folds} folds. "
            f"Need at least {n_folds + 1} years."
        )

    test_years = years[-(n_folds):]
    splits     = []

    for test_year in test_years:
        train_mask = dataset.index.year < test_year
        test_mask  = dataset.index.year == test_year
        splits.append((
            np.where(train_mask)[0],
            np.where(test_mask)[0],
            test_year,
        ))

    return splits


def evaluate_fold(y_true, y_proba, threshold: float, fold_label: str) -> dict:
    """Compute all metrics for one fold."""
    y_pred = (y_proba >= threshold).astype(int)

    acc   = accuracy_score(y_true, y_pred)
    prec  = precision_score(y_true, y_pred, zero_division=0)
    rec   = recall_score(y_true, y_pred, zero_division=0)
    auc   = roc_auc_score(y_true, y_proba)
    ll    = log_loss(y_true, y_proba)
    n_sig = int(y_pred.sum())   # trades taken at this threshold

    print(f"\n  {fold_label}")
    print(f"    Accuracy  : {acc:.4f}")
    print(f"    Precision : {prec:.4f}  (win rate at threshold {threshold})")
    print(f"    Recall    : {rec:.4f}")
    print(f"    ROC-AUC   : {auc:.4f}")
    print(f"    Log-loss  : {ll:.4f}")
    print(f"    Signals   : {n_sig:,} / {len(y_pred):,} bars ({n_sig/len(y_pred)*100:.1f}%)")

    if n_sig > 0:
        print(classification_report(y_true, y_pred,
                                    target_names=["Loss (0)", "Win (1)"],
                                    digits=3))
    return {
        "accuracy":   round(acc,  4),
        "precision":  round(prec, 4),
        "recall":     round(rec,  4),
        "roc_auc":    round(auc,  4),
        "log_loss":   round(ll,   4),
        "signals":    n_sig,
        "test_bars":  len(y_pred),
    }


# ── Feature importance pruning ────────────────────────────────────────────────

def prune_features(model: xgb.XGBClassifier,
                   feature_names: list[str],
                   keep_top: int = 50) -> list[str]:
    """Return top-N features by XGBoost feature importance (gain)."""
    importances = model.feature_importances_
    ranked      = sorted(zip(feature_names, importances),
                         key=lambda x: x[1], reverse=True)
    top         = [name for name, _ in ranked[:keep_top]]
    print(f"\nTop {keep_top} features selected (from {len(feature_names)})")
    print("  Top 10:", [name for name, _ in ranked[:10]])
    return top


# ── Main training pipeline ────────────────────────────────────────────────────

def train():
    t0 = time.time()
    print("=" * 60)
    print("Model 1 — Technical Indicators XGBoost Training")
    print("=" * 60)

    # 1. Load and compute features
    print("\n[1/5] Loading candles...")
    df_raw = load_candles(CONFIG["db_path"], CONFIG["table"])

    print("\n[2/5] Computing features...")
    df_features = compute_features(df_raw)

    # Join tick-level features (spread, tick_count) — improves model ~2-3% AUC
    print("\n      Loading tick features from eurusd_tick_features_m1...")
    df_ticks = load_tick_features(CONFIG["db_path"])
    df_features = add_tick_features(
        df_features, df_ticks, target_pips=CONFIG["target_pips"]
    )
    del df_ticks
    gc.collect()

    # Downcast to float32 to halve memory (84 cols × 5.5M rows × 4 bytes = ~1.85 GB vs 3.7 GB)
    df_features = df_features.astype("float32")
    gc.collect()

    print("\n[3/5] Building labeled dataset...")
    dataset = build_dataset(
        df_raw,
        df_features,
        target_pips        = CONFIG["target_pips"],
        stop_pips          = CONFIG["stop_pips"],
        horizon            = CONFIG["horizon_bars"],
        session_filter     = True,
        use_fast_labels    = True,
        wide_spread_filter = 1.5,   # drop news/illiquid bars (spread > 1.5p)
    )
    del df_raw, df_features
    gc.collect()

    feature_cols = [c for c in dataset.columns if c != "label"]

    # Replace inf/-inf with NaN then drop any remaining bad rows
    df_feat   = dataset[feature_cols].replace([np.inf, -np.inf], np.nan)
    valid_mask = df_feat.notna().all(axis=1)
    n_dropped  = (~valid_mask).sum()
    if n_dropped > 0:
        print(f"Dropped {n_dropped:,} rows with inf/nan in features")
    df_feat = df_feat[valid_mask]
    dataset = dataset[valid_mask]

    X = df_feat.to_numpy(dtype=np.float32)
    y = dataset["label"].to_numpy(dtype=np.int32)

    print(f"\nFinal dataset: {X.shape[0]:,} rows x {X.shape[1]} features")
    print(f"Date range: {dataset.index[0]} -> {dataset.index[-1]}")

    # 2. Walk-forward cross-validation
    print("\n[4/5] Walk-forward validation...")
    splits = walk_forward_splits(dataset, CONFIG["n_folds"])

    fold_metrics  = []
    best_fold_auc = 0.0
    best_model    = None

    for train_idx, test_idx, test_year in splits:
        X_tr, X_te = X[train_idx], X[test_idx]
        y_tr, y_te = y[train_idx], y[test_idx]

        # Adjust class weight if imbalanced
        pos_frac = y_tr.mean()
        spw = (1 - pos_frac) / (pos_frac + 1e-10)  # scale_pos_weight
        params = {**CONFIG["xgb_params"], "scale_pos_weight": round(spw, 2)}

        model = xgb.XGBClassifier(**params)
        model.fit(
            X_tr, y_tr,
            eval_set=[(X_te, y_te)],
            verbose=False,
        )

        y_proba = model.predict_proba(X_te)[:, 1]
        metrics = evaluate_fold(
            y_te, y_proba,
            threshold   = CONFIG["threshold"],
            fold_label  = f"Fold: test_year={test_year}  "
                          f"(train {train_idx.shape[0]:,} / test {test_idx.shape[0]:,})",
        )
        metrics["test_year"] = test_year
        fold_metrics.append(metrics)

        if metrics["roc_auc"] > best_fold_auc:
            best_fold_auc = metrics["roc_auc"]
            best_model    = model

    # 3. Summary
    avg = {
        k: round(np.mean([m[k] for m in fold_metrics]), 4)
        for k in ["accuracy", "precision", "recall", "roc_auc", "log_loss"]
    }
    print("\n" + "=" * 40)
    print("Walk-forward average:")
    for k, v in avg.items():
        print(f"  {k:<12}: {v:.4f}")
    print("=" * 40)

    # 4. Feature pruning + final model on ALL data
    print("\n[5/5] Training final model on full dataset...")

    # First pass: use best fold model to get feature importance
    top_features = prune_features(best_model, feature_cols, keep_top=55)
    top_idx      = [feature_cols.index(f) for f in top_features]
    X_pruned     = X[:, top_idx]

    # Adjust class weight for full dataset
    pos_frac_all = y.mean()
    spw_all      = (1 - pos_frac_all) / (pos_frac_all + 1e-10)
    final_params = {**CONFIG["xgb_params"]}
    final_params.pop("early_stopping_rounds", None)  # no eval set for final
    final_params["n_estimators"]     = best_model.best_iteration + 1 if hasattr(best_model, "best_iteration") else 400
    final_params["scale_pos_weight"] = round(spw_all, 2)

    final_model = xgb.XGBClassifier(**final_params)
    final_model.fit(X_pruned, y, verbose=False)

    # 5. Save outputs
    model_dir = Path(CONFIG["model_dir"])
    model_dir.mkdir(parents=True, exist_ok=True)

    model_path   = model_dir / "model1_technical.json"
    names_path   = model_dir / "model1_feature_names.txt"
    metrics_path = model_dir / "model1_metrics.json"

    final_model.save_model(str(model_path))
    names_path.write_text("\n".join(top_features))
    metrics_path.write_text(json.dumps({
        "config":         CONFIG,
        "fold_metrics":   fold_metrics,
        "avg_metrics":    avg,
        "n_features_used": len(top_features),
        "feature_names":  top_features,
    }, indent=2, default=str))

    elapsed = time.time() - t0
    print(f"\nSaved model  -> {model_path}")
    print(f"Saved names  -> {names_path}")
    print(f"Saved metrics-> {metrics_path}")
    print(f"\nDone in {elapsed:.0f}s")

    return final_model, top_features, avg


# ── Inference helper (used by live Rust ↔ Python bridge later) ───────────────

def load_model(model_dir: str = "ml/trained"):
    """
    Load a saved Model 1 for inference.

    Returns (model, feature_names) ready for predict_proba().
    """
    d = Path(model_dir)
    model = xgb.XGBClassifier()
    model.load_model(str(d / "model1_technical.json"))
    feature_names = (d / "model1_feature_names.txt").read_text().strip().splitlines()
    return model, feature_names


if __name__ == "__main__":
    train()
