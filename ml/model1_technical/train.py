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

import sys
print("Python process started, importing libraries...", flush=True)

import gc
import json
import time
from pathlib import Path

import numpy as np; print("  numpy ok", flush=True)
import pandas as pd; print("  pandas ok", flush=True)
import xgboost as xgb; print("  xgboost ok", flush=True)
from sklearn.metrics import (
    accuracy_score, classification_report, log_loss,
    precision_score, recall_score, roc_auc_score,
); print("  sklearn ok", flush=True)

PROJECT_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(PROJECT_ROOT))

print("  loading features module (pandas_ta + duckdb)...", flush=True)
from ml.model1_technical.features import (
    load_candles, load_tick_features, add_tick_features, compute_features,
    load_cross_pair_candles, compute_cross_pair_features, add_regime_feature,
    load_order_flow_features, add_order_flow_features,
); print("  features ok", flush=True)
from ml.model1_technical.labels import build_dataset, build_dataset_dual
print("All imports done.", flush=True)

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
    "n_folds":       2,       # 2023-2024 train, 2025 test; 2023-2025 train, 2026 test

    # Decision threshold for precision/recall (not for training)
    "threshold":     0.50,

    # Train both a long model and a short model
    "train_short_model": True,

    # Regime filter: removed (M2 dropped from stack)
    "regime_filter": False,

    # XGBoost hyperparameters
    "xgb_params": {
        "n_estimators":       800,
        "max_depth":          4,
        "learning_rate":      0.05,
        "subsample":          0.75,
        "colsample_bytree":   0.7,
        "min_child_weight":   5,     # reduced: allow finer splits → higher recall
        "reg_alpha":          0.3,
        "reg_lambda":         1.5,
        "scale_pos_weight":   1.0,   # set per-fold from class balance
        "eval_metric":        "logloss",
        "early_stopping_rounds": 40,
        "random_state":       42,
        "device":             "cuda",
        "tree_method":        "hist",
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
    """Compute all metrics for one fold, with precision-recall at multiple thresholds."""
    y_pred = (y_proba >= threshold).astype(int)

    acc   = accuracy_score(y_true, y_pred)
    prec  = precision_score(y_true, y_pred, zero_division=0)
    rec   = recall_score(y_true, y_pred, zero_division=0)
    auc   = roc_auc_score(y_true, y_proba)
    ll    = log_loss(y_true, y_proba)
    n_sig = int(y_pred.sum())

    print(f"\n  {fold_label}")
    print(f"    ROC-AUC   : {auc:.4f}")
    print(f"    Log-loss  : {ll:.4f}")
    print(f"    Accuracy  : {acc:.4f}")
    print(f"    Threshold breakdown:")
    for t in [0.50, 0.52, 0.55, 0.58, 0.60, 0.65]:
        yp = (y_proba >= t).astype(int)
        p  = precision_score(y_true, yp, zero_division=0)
        r  = recall_score(y_true, yp, zero_division=0)
        ns = int(yp.sum())
        print(f"      t={t:.2f}  prec={p:.3f}  recall={r:.3f}  signals={ns:,}")
    print(f"    At t={threshold}: Precision={prec:.4f}  Recall={rec:.4f}  "
          f"Signals={n_sig:,} / {len(y_pred):,} ({n_sig/len(y_pred)*100:.1f}%)")

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

def _train_one_direction(
    X: np.ndarray,
    y: np.ndarray,
    dataset: pd.DataFrame,
    feature_cols: list[str],
    direction: str,
) -> tuple:
    """
    Run walk-forward CV + final model training for one direction (long or short).

    Returns (final_model, top_features, fold_metrics, avg_metrics).
    """
    print(f"\n{'='*60}")
    print(f"  Training {direction.upper()} model")
    print(f"{'='*60}")

    splits = walk_forward_splits(dataset, CONFIG["n_folds"])

    fold_metrics  = []
    best_fold_auc = 0.0
    best_model    = None

    for train_idx, test_idx, test_year in splits:
        X_tr, X_te = X[train_idx], X[test_idx]
        y_tr, y_te = y[train_idx], y[test_idx]

        pos_frac = y_tr.mean()
        spw      = (1 - pos_frac) / (pos_frac + 1e-10)
        params   = {**CONFIG["xgb_params"], "scale_pos_weight": round(spw, 2)}

        model = xgb.XGBClassifier(**params)
        model.fit(X_tr, y_tr, eval_set=[(X_te, y_te)], verbose=False)

        y_proba = model.predict_proba(X_te)[:, 1]
        metrics = evaluate_fold(
            y_te, y_proba,
            threshold  = CONFIG["threshold"],
            fold_label = f"[{direction}] Fold test_year={test_year} "
                         f"(train {train_idx.shape[0]:,} / test {test_idx.shape[0]:,})",
        )
        metrics["test_year"] = test_year
        fold_metrics.append(metrics)

        if metrics["roc_auc"] > best_fold_auc:
            best_fold_auc = metrics["roc_auc"]
            best_model    = model

    avg = {
        k: round(np.mean([m[k] for m in fold_metrics]), 4)
        for k in ["accuracy", "precision", "recall", "roc_auc", "log_loss"]
    }
    print(f"\n  [{direction}] Walk-forward averages:")
    for k, v in avg.items():
        print(f"    {k:<12}: {v:.4f}")

    # Final model on full data with top features
    top_features = prune_features(best_model, feature_cols, keep_top=70)
    top_idx      = [feature_cols.index(f) for f in top_features]
    X_pruned     = X[:, top_idx]

    pos_frac_all = y.mean()
    spw_all      = (1 - pos_frac_all) / (pos_frac_all + 1e-10)
    final_params = {**CONFIG["xgb_params"]}
    final_params.pop("early_stopping_rounds", None)
    final_params["n_estimators"]     = (
        best_model.best_iteration + 1
        if hasattr(best_model, "best_iteration") and best_model.best_iteration
        else 400
    )
    final_params["scale_pos_weight"] = round(spw_all, 2)

    final_model = xgb.XGBClassifier(**final_params)
    final_model.fit(X_pruned, y, verbose=False)

    return final_model, top_features, fold_metrics, avg


def train():
    t0 = time.time()
    print("=" * 60)
    print("Model 1 — Technical Indicators XGBoost Training")
    print("=" * 60)

    # ── 1. Load raw candles ──────────────────────────────────────────────────
    print("\n[1/6] Loading candles...")
    df_raw = load_candles(CONFIG["db_path"], CONFIG["table"])

    # ── 2. Compute features ──────────────────────────────────────────────────
    print("\n[2/6] Computing features...")
    df_features = compute_features(df_raw)

    print("\n      Loading tick features from eurusd_tick_features_m1...")
    try:
        df_ticks = load_tick_features(CONFIG["db_path"])
        if len(df_ticks) > 0:
            df_features = add_tick_features(
                df_features, df_ticks, target_pips=CONFIG["target_pips"]
            )
            del df_ticks
        else:
            print(f"      WARNING: tick features table is empty — training without spread features")
    except Exception as e:
        print(f"      WARNING: tick features not available ({e}) — training without spread features")
    gc.collect()

    print("\n      Loading cross-pair M1 data (GBPUSD, USDJPY, USDCHF, AUDUSD, EURJPY, XAUUSD)...")
    try:
        cross_closes = load_cross_pair_candles(CONFIG["db_path"])
        if cross_closes:
            df_cross = compute_cross_pair_features(df_raw, cross_closes)
            df_features = df_features.join(df_cross, how="left")
            del df_cross
        del cross_closes
    except Exception as e:
        print(f"      WARNING: cross-pair data not available ({e}) — training without cross-pair features")
    gc.collect()

    print("\n      Loading order flow delta from tick data...")
    try:
        df_of = load_order_flow_features(CONFIG["db_path"])
        df_features = add_order_flow_features(df_features, df_of)
        del df_of
    except Exception as e:
        print(f"      WARNING: order flow failed ({e}) — skipping")
    gc.collect()

    df_features = df_features.astype("float32")
    gc.collect()

    # ── 3. Build labeled datasets (long + short) ─────────────────────────────
    print("\n[3/6] Building labeled datasets (long + short)...")
    if CONFIG.get("train_short_model", False):
        dataset_long, dataset_short = build_dataset_dual(
            df_raw,
            df_features,
            target_pips        = CONFIG["target_pips"],
            stop_pips          = CONFIG["stop_pips"],
            horizon            = CONFIG["horizon_bars"],
            session_filter     = True,
            wide_spread_filter = 1.5,
        )
    else:
        dataset_long = build_dataset(
            df_raw,
            df_features,
            target_pips        = CONFIG["target_pips"],
            stop_pips          = CONFIG["stop_pips"],
            horizon            = CONFIG["horizon_bars"],
            session_filter     = True,
            use_fast_labels    = True,
            wide_spread_filter = 1.5,
        )
        dataset_short = None

    del df_raw, df_features
    gc.collect()

    # ── 3b. Regime filter: restrict to favorable HMM states ─────────────────
    if CONFIG.get("regime_filter", True):
        map_path = Path(CONFIG["model_dir"]) / "model2_state_map.json"
        if map_path.exists() and "regime_state" in dataset_long.columns:
            state_map = {int(k): v for k, v in json.loads(map_path.read_text()).items()}
            print(f"\n[3b] Applying regime filter using {map_path.name}:")
            print(f"     State map: { {k: v for k, v in sorted(state_map.items())} }")

            # States that are favorable for each direction
            long_ok  = {sid for sid, lbl in state_map.items()
                        if lbl in ("Trending Up", "Ranging")}
            short_ok = {sid for sid, lbl in state_map.items()
                        if lbl in ("Trending Down", "Ranging")}

            before = len(dataset_long)
            dataset_long = dataset_long[dataset_long["regime_state"].isin(long_ok)]
            kept_pct = len(dataset_long) / before * 100
            print(f"     LONG : {before:,} → {len(dataset_long):,} rows ({kept_pct:.1f}% kept)"
                  f" — states: { [state_map[s] for s in sorted(long_ok)] }")

            if dataset_short is not None:
                before = len(dataset_short)
                dataset_short = dataset_short[dataset_short["regime_state"].isin(short_ok)]
                kept_pct = len(dataset_short) / before * 100
                print(f"     SHORT: {before:,} → {len(dataset_short):,} rows ({kept_pct:.1f}% kept)"
                      f" — states: { [state_map[s] for s in sorted(short_ok)] }")
        else:
            if not map_path.exists():
                print(f"\n[3b] Regime filter: {map_path.name} not found — train Model 2 first, skipping")
            else:
                print(f"\n[3b] Regime filter: 'regime_state' column not in dataset — skipping")

    # ── Helper: clean dataset → X, y arrays ─────────────────────────────────
    def prepare_arrays(dataset: pd.DataFrame):
        feature_cols = [c for c in dataset.columns if c != "label"]
        df_feat      = dataset[feature_cols].replace([np.inf, -np.inf], np.nan)

        # Drop columns that are entirely NaN (missing tick/cross-pair data)
        all_nan_cols = df_feat.columns[df_feat.isna().all()]
        if len(all_nan_cols) > 0:
            print(f"  Dropping {len(all_nan_cols)} all-NaN columns: {list(all_nan_cols[:10])}...")
            df_feat = df_feat.drop(columns=all_nan_cols)
            feature_cols = [c for c in feature_cols if c not in all_nan_cols]

        valid_mask   = df_feat.notna().all(axis=1)
        n_dropped    = (~valid_mask).sum()
        if n_dropped > 0:
            print(f"  Dropped {n_dropped:,} rows with inf/nan in features")
        df_feat  = df_feat[valid_mask]
        dataset  = dataset[valid_mask]

        if len(dataset) == 0:
            raise ValueError("No valid training rows after filtering. "
                             "Make sure tick data and cross-pair data are downloaded.")

        X = df_feat.to_numpy(dtype=np.float32)
        y = dataset["label"].to_numpy(dtype=np.int32)
        print(f"  Final: {X.shape[0]:,} rows x {X.shape[1]} features "
              f"({dataset.index[0].date()} → {dataset.index[-1].date()})")
        return X, y, dataset, feature_cols

    # ── 4. Walk-forward CV ───────────────────────────────────────────────────
    print("\n[4/6] Walk-forward validation (LONG)...")
    X_long, y_long, dataset_long, feat_cols_long = prepare_arrays(dataset_long)

    long_model, long_features, long_fold_metrics, long_avg = _train_one_direction(
        X_long, y_long, dataset_long, feat_cols_long, "LONG"
    )

    short_model, short_features, short_fold_metrics, short_avg = None, [], [], {}
    if dataset_short is not None:
        print("\n[5/6] Walk-forward validation (SHORT)...")
        X_short, y_short, dataset_short, feat_cols_short = prepare_arrays(dataset_short)
        short_model, short_features, short_fold_metrics, short_avg = _train_one_direction(
            X_short, y_short, dataset_short, feat_cols_short, "SHORT"
        )
    else:
        print("\n[5/6] Skipped (train_short_model=False)")

    # ── 5 & 6. Save outputs ──────────────────────────────────────────────────
    print("\n[6/6] Saving models...")
    model_dir = Path(CONFIG["model_dir"])
    model_dir.mkdir(parents=True, exist_ok=True)

    # Long model (also saved as model1_technical.json for backward compatibility)
    long_path  = model_dir / "model1_long.json"
    compat_path = model_dir / "model1_technical.json"
    long_names_path = model_dir / "model1_long_feature_names.txt"
    long_model.save_model(str(long_path))
    long_model.save_model(str(compat_path))
    long_names_path.write_text("\n".join(long_features))

    # Short model
    if short_model is not None:
        short_path = model_dir / "model1_short.json"
        short_names_path = model_dir / "model1_short_feature_names.txt"
        short_model.save_model(str(short_path))
        short_names_path.write_text("\n".join(short_features))

    # model1_metrics.json — flat format, long model only (UI reads this)
    metrics_path = model_dir / "model1_metrics.json"
    metrics_path.write_text(json.dumps({
        "config":          CONFIG,
        "fold_metrics":    long_fold_metrics,
        "avg_metrics":     long_avg,
        "n_features_used": len(long_features),
        "feature_names":   long_features,
    }, indent=2, default=str))

    # model1_metrics_combined.json — nested long + short (for future use)
    (model_dir / "model1_metrics_combined.json").write_text(json.dumps({
        "config": CONFIG,
        "long": {
            "fold_metrics":    long_fold_metrics,
            "avg_metrics":     long_avg,
            "n_features_used": len(long_features),
            "feature_names":   long_features,
        },
        "short": {
            "fold_metrics":    short_fold_metrics,
            "avg_metrics":     short_avg,
            "n_features_used": len(short_features),
            "feature_names":   short_features,
        } if short_model else None,
    }, indent=2, default=str))

    elapsed = time.time() - t0
    print(f"\nSaved: {long_path}, {compat_path}")
    if short_model:
        print(f"Saved: {short_path}")
    print(f"Saved: {metrics_path}")
    print(f"\nDone in {elapsed:.0f}s")

    return long_model, long_features, long_avg


# ── Inference helper (used by live Rust ↔ Python bridge later) ───────────────

def load_model(model_dir: str = "ml/trained", direction: str = "long"):
    """
    Load a saved Model 1 for inference.

    Args:
        direction: "long" or "short"

    Returns (model, feature_names) ready for predict_proba().
    """
    d = Path(model_dir)
    model = xgb.XGBClassifier()

    if direction == "short" and (d / "model1_short.json").exists():
        model.load_model(str(d / "model1_short.json"))
        names_file = d / "model1_short_feature_names.txt"
    else:
        # Fall back to model1_technical.json (long model / backward compat)
        model.load_model(str(d / "model1_technical.json"))
        names_file = (d / "model1_long_feature_names.txt"
                      if (d / "model1_long_feature_names.txt").exists()
                      else d / "model1_feature_names.txt")

    feature_names = names_file.read_text().strip().splitlines()
    return model, feature_names


if __name__ == "__main__":
    train()
