"""
Train XGBoost model on the unified feature table.
Walk-forward validation: train on past, test on future.
Uses GPU (CUDA) for fast training on RTX 3070.
"""

import sys
import json
import os

print("Importing packages (Python 3.14 first import may be slow)...", flush=True)
print("  pandas...", end="", flush=True)
import pandas as pd
print(" ok", flush=True)
print("  numpy...", end="", flush=True)
import numpy as np
print(" ok", flush=True)
print("  sklearn...", end="", flush=True)
from sklearn.metrics import classification_report, accuracy_score
from sklearn.model_selection import TimeSeriesSplit
print(" ok", flush=True)
print("  xgboost...", end="", flush=True)
import xgboost as xgb
print(" ok", flush=True)

# ── Load data ────────────────────────────────────────────────────────────────

CSV_PATH = os.path.join(os.path.dirname(__file__), "ml_unified_features.csv")
print(f"Loading {CSV_PATH}...")
df = pd.read_csv(CSV_PATH)
print(f"Loaded {len(df)} rows, {len(df.columns)} columns")
print(f"Period: {df['timestamp'].min()} to {df['timestamp'].max()}")

# ── Feature / target split ───────────────────────────────────────────────────

# Drop raw OHLCV (we have derived features instead) and target columns
DROP_COLS = ['timestamp', 'open', 'high', 'low', 'close', 'next_close',
             'target_move_pips', 'target_direction']

# Features = everything except drops
feature_cols = [c for c in df.columns if c not in DROP_COLS]
X = df[feature_cols].copy()
y_direction = df['target_direction'].copy()  # -1, 0, +1
y_move = df['target_move_pips'].copy()

# For classification: map -1,0,1 to 0,1,2 (XGBoost needs non-negative labels)
# Or simplify to binary: up (1) vs down (0), drop 0s
binary_mask = y_direction != 0
X_bin = X[binary_mask].reset_index(drop=True)
y_bin = (y_direction[binary_mask] == 1).astype(int).reset_index(drop=True)

print(f"\nFeatures: {len(feature_cols)}")
print(f"Binary classification: {len(X_bin)} rows ({y_bin.mean():.3f} up rate)")
print(f"Features: {feature_cols}")

# ── Handle NaN/inf ───────────────────────────────────────────────────────────

X_bin = X_bin.replace([np.inf, -np.inf], np.nan)
# XGBoost handles NaN natively, but let's check
nan_pct = X_bin.isna().mean()
high_nan = nan_pct[nan_pct > 0.5]
if len(high_nan) > 0:
    print(f"\nWARNING: High NaN columns (>50%): {dict(high_nan)}")

# ── Walk-forward split ───────────────────────────────────────────────────────

# Use last 20% as test (most recent data)
split_idx = int(len(X_bin) * 0.8)
X_train, X_test = X_bin[:split_idx], X_bin[split_idx:]
y_train, y_test = y_bin[:split_idx], y_bin[split_idx:]

print(f"\nTrain: {len(X_train)} rows")
print(f"Test:  {len(X_test)} rows (walk-forward, most recent 20%)")

# ── Train XGBoost ────────────────────────────────────────────────────────────

print("\nTraining XGBoost with CUDA...")
model = xgb.XGBClassifier(
    n_estimators=500,
    max_depth=6,
    learning_rate=0.05,
    subsample=0.8,
    colsample_bytree=0.8,
    min_child_weight=10,
    gamma=1,
    reg_alpha=0.1,
    reg_lambda=1.0,
    tree_method='hist',
    device='cuda',
    eval_metric='logloss',
    early_stopping_rounds=50,
    random_state=42,
)

model.fit(
    X_train, y_train,
    eval_set=[(X_test, y_test)],
    verbose=50,
)

# ── Evaluate ─────────────────────────────────────────────────────────────────

y_pred = model.predict(X_test)
y_prob = model.predict_proba(X_test)[:, 1]

acc = accuracy_score(y_test, y_pred)
print(f"\n{'='*60}")
print(f"Test Accuracy: {acc:.4f} ({acc*100:.1f}%)")
print(f"Baseline (always predict majority): {max(y_test.mean(), 1-y_test.mean()):.4f}")
print(f"{'='*60}")
print(f"\nClassification Report:")
print(classification_report(y_test, y_pred, target_names=['DOWN', 'UP']))

# ── Feature Importance ───────────────────────────────────────────────────────

importance = model.feature_importances_
imp_df = pd.DataFrame({
    'feature': feature_cols,
    'importance': importance
}).sort_values('importance', ascending=False)

print(f"\nTop 20 Feature Importances:")
print(f"{'Feature':<30} {'Importance':>10}")
print("-" * 42)
for _, row in imp_df.head(20).iterrows():
    print(f"{row['feature']:<30} {row['importance']:>10.4f}")

# ── Confidence-filtered accuracy ─────────────────────────────────────────────

print(f"\nAccuracy by confidence threshold:")
print(f"{'Threshold':<12} {'Accuracy':>10} {'Trades':>8} {'% of total':>12}")
print("-" * 44)
for threshold in [0.50, 0.52, 0.55, 0.58, 0.60, 0.65, 0.70]:
    confident = (y_prob >= threshold) | (y_prob <= 1 - threshold)
    if confident.sum() > 0:
        conf_acc = accuracy_score(y_test[confident], y_pred[confident])
        print(f"{threshold:<12.2f} {conf_acc:>10.4f} {confident.sum():>8} {confident.mean()*100:>11.1f}%")

# ── Save model and results ───────────────────────────────────────────────────

MODEL_DIR = os.path.join(os.path.dirname(__file__), "trained")
os.makedirs(MODEL_DIR, exist_ok=True)

model.save_model(os.path.join(MODEL_DIR, "unified_xgb.json"))
print(f"\nModel saved to {MODEL_DIR}/unified_xgb.json")

# Save metrics
metrics = {
    "accuracy": float(acc),
    "baseline": float(max(y_test.mean(), 1-y_test.mean())),
    "train_rows": int(len(X_train)),
    "test_rows": int(len(X_test)),
    "features": len(feature_cols),
    "best_iteration": int(model.best_iteration) if hasattr(model, 'best_iteration') else -1,
    "top_features": {row['feature']: float(row['importance']) for _, row in imp_df.head(20).iterrows()},
}
with open(os.path.join(MODEL_DIR, "unified_metrics.json"), "w") as f:
    json.dump(metrics, f, indent=2)

imp_df.to_csv(os.path.join(MODEL_DIR, "unified_feature_importance.csv"), index=False)
print("Model 4b successfully trained. Metrics and feature importance saved.")
