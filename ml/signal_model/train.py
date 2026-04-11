"""
Signal Model — LightGBM with Target Engineering
=================================================
Primary signal model replacing the old M4b unified ensemble.
Uses ATR-based targets with walk-forward validation.

Features: M1 technical (from model1) + session/time + regime state
Target:   "Price moves > 1.5 ATR in direction within 2 hours"

Run from the project root:
    python -m ml.signal_model.train

Outputs:
    ml/trained/signal_long.txt            <- LightGBM long model
    ml/trained/signal_short.txt           <- LightGBM short model
    ml/trained/signal_long_features.txt   <- feature names
    ml/trained/signal_short_features.txt  <- feature names
    ml/trained/signal_metrics.json        <- walk-forward CV results
"""

import gc
import json
import sys
import time
from pathlib import Path

import numpy as np
import pandas as pd
import lightgbm as lgb
from sklearn.metrics import (
    accuracy_score,
    log_loss,
    precision_score,
    recall_score,
    roc_auc_score,
)

PROJECT_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(PROJECT_ROOT))

from ml.model1_technical.features import load_candles, compute_features
from ml.signal_model.labels import build_labels, session_filter

# ── Configuration ────────────────────────────────────────────────────────────

CONFIG = {
    "db_path":       "Bots_db/Algo_EURUSD.duckdb",
    "model_dir":     "ml/trained",
    # Walk-forward: train on N years, test on 1 year, slide forward
    "train_years":   2,       # train on 2 years, test on next year
    "test_months":   12,
    "start_year":    2023,      # training data from 2023 onwards
    # Feature selection
    "top_features":  60,        # keep top N by gain
    # LightGBM hyperparameters
    "lgb_params": {
        "objective":       "binary",
        "metric":          "binary_logloss",
        "boosting_type":   "gbdt",
        "num_leaves":      31,
        "max_depth":       5,
        "learning_rate":   0.05,
        "n_estimators":    800,
        "min_child_samples": 50,
        "subsample":       0.8,
        "colsample_bytree": 0.8,
        "reg_alpha":       0.1,
        "reg_lambda":      1.0,
        "verbose":         -1,
        "n_jobs":          -1,
    },
    "early_stopping_rounds": 50,
}


def walk_forward_train(
    df_features: pd.DataFrame,
    labels: pd.Series,
    direction: str,
    config: dict,
) -> tuple:
    """
    Walk-forward training with expanding window.
    Returns (best_model, feature_names, metrics_list).
    """
    train_years = config["train_years"]
    test_months = config["test_months"]

    # Align features and labels — use intersection of indices
    common = df_features.index.intersection(labels.dropna().index)
    X = df_features.loc[common]
    y = labels.loc[common]

    print(f"\n  [{direction.upper()}] Total samples: {len(y):,}  "
          f"(win={y.sum():.0f}, loss={len(y)-y.sum():.0f}, "
          f"win_rate={y.mean():.1%})")

    # Determine fold boundaries
    years = sorted(X.index.year.unique())
    min_year = min(years)
    max_year = max(years)

    folds = []
    for test_start_year in range(min_year + train_years, max_year + 1):
        train_end = pd.Timestamp(f"{test_start_year}-01-01", tz="UTC")
        test_end = train_end + pd.DateOffset(months=test_months)

        train_mask = X.index < train_end
        test_mask = (X.index >= train_end) & (X.index < test_end)

        if train_mask.sum() < 1000 or test_mask.sum() < 500:
            continue

        folds.append((train_mask, test_mask, test_start_year))

    if not folds:
        print("  WARNING: No valid walk-forward folds!")
        return None, [], []

    print(f"  Walk-forward folds: {len(folds)}")

    metrics_list = []
    best_model = None
    best_auc = 0
    best_features = None

    for fold_idx, (train_mask, test_mask, test_year) in enumerate(folds):
        X_train, y_train = X[train_mask], y[train_mask]
        X_test, y_test = X[test_mask], y[test_mask]

        print(f"\n  Fold {fold_idx+1}: train={len(X_train):,} "
              f"test={len(X_test):,} (test_year={test_year})")

        # Feature selection on training data
        selector = lgb.LGBMClassifier(**config["lgb_params"])
        selector.fit(X_train, y_train)

        importances = pd.Series(
            selector.feature_importances_,
            index=X_train.columns
        ).sort_values(ascending=False)

        top_features = importances.head(config["top_features"]).index.tolist()
        print(f"    Selected {len(top_features)} features (top by gain)")

        # Retrain on selected features with early stopping
        X_tr_sel = X_train[top_features]
        X_te_sel = X_test[top_features]

        model = lgb.LGBMClassifier(**config["lgb_params"])
        model.fit(
            X_tr_sel, y_train,
            eval_set=[(X_te_sel, y_test)],
            callbacks=[
                lgb.early_stopping(config["early_stopping_rounds"]),
                lgb.log_evaluation(0),
            ],
        )

        # Evaluate
        y_pred_proba = model.predict_proba(X_te_sel)[:, 1]
        y_pred = (y_pred_proba >= 0.5).astype(int)

        try:
            auc = roc_auc_score(y_test, y_pred_proba)
        except ValueError:
            auc = 0.5

        acc = accuracy_score(y_test, y_pred)
        prec = precision_score(y_test, y_pred, zero_division=0)
        rec = recall_score(y_test, y_pred, zero_division=0)
        ll = log_loss(y_test, y_pred_proba)

        fold_metrics = {
            "fold": fold_idx + 1,
            "test_year": test_year,
            "train_size": len(X_train),
            "test_size": len(X_test),
            "accuracy": round(acc, 4),
            "precision": round(prec, 4),
            "recall": round(rec, 4),
            "auc": round(auc, 4),
            "log_loss": round(ll, 4),
            "n_features": len(top_features),
            "best_iteration": model.best_iteration_ if hasattr(model, 'best_iteration_') else -1,
        }
        metrics_list.append(fold_metrics)

        print(f"    ACC={acc:.3f} AUC={auc:.3f} PREC={prec:.3f} "
              f"REC={rec:.3f} LL={ll:.4f}")

        if auc > best_auc:
            best_auc = auc
            best_model = model
            best_features = top_features

    return best_model, best_features, metrics_list


def train():
    t0 = time.time()
    print("=" * 60)
    print("Signal Model — LightGBM with ATR Target Engineering")
    print("=" * 60)

    # 1. Load data
    print(f"\n[1/5] Loading M1 candles...")
    df = load_candles(CONFIG["db_path"])

    # Filter to training period
    start = pd.Timestamp(f"{CONFIG['start_year']}-01-01", tz="UTC")
    df = df[df.index >= start]
    print(f"  Filtered to {len(df):,} bars from {CONFIG['start_year']}")

    # 2. Compute features
    print("\n[2/5] Computing features...")
    features = compute_features(df)

    # 3. Build labels
    print("\n[3/5] Building ATR-based labels...")
    labels_long = build_labels(df, "long")
    labels_short = build_labels(df, "short")

    # Session filter
    session_mask = session_filter(df)
    labels_long[~session_mask] = np.nan
    labels_short[~session_mask] = np.nan

    valid_long = labels_long.notna().sum()
    valid_short = labels_short.notna().sum()
    print(f"  LONG labels: {valid_long:,} (win_rate={labels_long.dropna().mean():.1%})")
    print(f"  SHORT labels: {valid_short:,} (win_rate={labels_short.dropna().mean():.1%})")

    # Align features to labels
    common_idx = features.index.intersection(labels_long.dropna().index)
    features_aligned = features.loc[common_idx].dropna()

    # 4. Train
    print("\n[4/5] Training walk-forward...")

    model_long, feat_long, metrics_long = walk_forward_train(
        features_aligned, labels_long, "long", CONFIG
    )
    gc.collect()

    model_short, feat_short, metrics_short = walk_forward_train(
        features_aligned, labels_short, "short", CONFIG
    )
    gc.collect()

    # 5. Save
    print("\n[5/5] Saving models...")
    model_dir = Path(CONFIG["model_dir"])
    model_dir.mkdir(parents=True, exist_ok=True)

    if model_long is not None:
        model_long.booster_.save_model(str(model_dir / "signal_long.txt"))
        (model_dir / "signal_long_features.txt").write_text("\n".join(feat_long))
        print(f"  Saved signal_long.txt ({len(feat_long)} features)")

    if model_short is not None:
        model_short.booster_.save_model(str(model_dir / "signal_short.txt"))
        (model_dir / "signal_short_features.txt").write_text("\n".join(feat_short))
        print(f"  Saved signal_short.txt ({len(feat_short)} features)")

    # Combined metrics
    all_metrics = {
        "model_type": "LightGBM",
        "target": "ATR-based first-touch (1.5R target, 1.0R stop, 120-bar horizon)",
        "train_range": f"{CONFIG['start_year']}-present",
        "parameters": {
            k: v for k, v in CONFIG["lgb_params"].items()
            if k != "verbose"
        },
        "long": {
            "folds": metrics_long,
            "mean_auc": round(np.mean([m["auc"] for m in metrics_long]), 4) if metrics_long else None,
            "mean_accuracy": round(np.mean([m["accuracy"] for m in metrics_long]), 4) if metrics_long else None,
        },
        "short": {
            "folds": metrics_short,
            "mean_auc": round(np.mean([m["auc"] for m in metrics_short]), 4) if metrics_short else None,
            "mean_accuracy": round(np.mean([m["accuracy"] for m in metrics_short]), 4) if metrics_short else None,
        },
    }
    (model_dir / "signal_metrics.json").write_text(json.dumps(all_metrics, indent=2))

    elapsed = time.time() - t0
    print(f"\nDone in {elapsed:.0f}s")

    if metrics_long:
        print(f"\nLONG  mean AUC={all_metrics['long']['mean_auc']:.4f}  "
              f"ACC={all_metrics['long']['mean_accuracy']:.4f}")
    if metrics_short:
        print(f"SHORT mean AUC={all_metrics['short']['mean_auc']:.4f}  "
              f"ACC={all_metrics['short']['mean_accuracy']:.4f}")


# ── Inference helper ────────────────────────────────────────────────────────

def load_model(model_dir: str = "ml/trained", direction: str = "long"):
    """Load saved signal model for inference."""
    d = Path(model_dir)
    model_path = d / f"signal_{direction}.txt"
    feat_path = d / f"signal_{direction}_features.txt"

    if not model_path.exists():
        return None, []

    model = lgb.Booster(model_file=str(model_path))
    features = feat_path.read_text().strip().split("\n") if feat_path.exists() else []
    return model, features


if __name__ == "__main__":
    train()
