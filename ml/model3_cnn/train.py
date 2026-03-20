"""
Model 3 — 1D CNN Chart Pattern Training
=========================================
Run from the project root:

    python -m ml.model3_cnn.train

Outputs:
    ml/trained/model3_cnn_long.pt      ← long  model weights (state_dict)
    ml/trained/model3_cnn_short.pt     ← short model weights
    ml/trained/model3_cnn.pt           ← alias for long (backward compat)
    ml/trained/model3_metrics.json     ← walk-forward CV results
"""

import gc
import json
import sys
import time
from pathlib import Path

import numpy as np
import pandas as pd
import torch
import torch.nn as nn
from torch.utils.data import DataLoader, TensorDataset
from sklearn.metrics import (
    accuracy_score,
    log_loss,
    precision_score,
    recall_score,
    roc_auc_score,
)

PROJECT_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(PROJECT_ROOT))

from ml.model3_cnn.dataset import load_candles, build_cnn_dataset_single, WINDOW_SIZE
from ml.model3_cnn.model import CNNPatternModel, count_params

# ── Configuration ─────────────────────────────────────────────────────────────

CONFIG = {
    # Data
    "db_path":      "Bots_db/Algo_EURUSD.duckdb",
    "table":        "eurusd_m1",

    # Window
    "window_size":  WINDOW_SIZE,   # 60 M1 bars = 1 hour of price history
    "n_channels":   5,             # OHLCV

    # Label parameters (identical to Model 1)
    "target_pips":  15,
    "stop_pips":    10,
    "horizon_bars": 120,

    # Walk-forward: number of yearly folds
    "n_folds":      4,

    # Decision threshold
    "threshold":    0.50,

    # Model architecture
    "dropout":      0.3,

    # Training
    "batch_size":   2048,
    "max_epochs":   100,
    "lr":           1e-3,
    "weight_decay": 1e-4,
    "patience":     15,    # early stopping patience (epochs without val-AUC improvement)

    # Train short model too
    "train_short_model": True,

    "model_dir":    "ml/trained",
}

DEVICE = torch.device("cuda" if torch.cuda.is_available() else "cpu")


# ── Walk-forward splits ───────────────────────────────────────────────────────

def walk_forward_splits(timestamps: pd.DatetimeIndex, n_folds: int) -> list:
    """
    Expanding-window walk-forward splits identical to Model 1.

    Returns list of (train_idx, test_idx, test_year).
    """
    years = sorted(set(timestamps.year))
    if len(years) < n_folds + 1:
        raise ValueError(
            f"Not enough years ({len(years)}) for {n_folds} folds. "
            f"Need at least {n_folds + 1}."
        )
    test_years = years[-n_folds:]
    splits = []
    for test_year in test_years:
        train_mask = timestamps.year < test_year
        test_mask  = timestamps.year == test_year
        splits.append((
            np.where(train_mask)[0],
            np.where(test_mask)[0],
            test_year,
        ))
    return splits


# ── Training helpers ──────────────────────────────────────────────────────────

def _make_loader(
    X: np.ndarray,
    y: np.ndarray,
    batch_size: int,
    shuffle: bool,
) -> DataLoader:
    Xt = torch.from_numpy(X)
    yt = torch.from_numpy(y.astype(np.float32))
    return DataLoader(
        TensorDataset(Xt, yt),
        batch_size=batch_size,
        shuffle=shuffle,
        pin_memory=False,  # disabled: dataset too large, causes bad_alloc on CPU
        num_workers=0,     # safe on Windows
    )


def _evaluate(model: CNNPatternModel, loader: DataLoader) -> tuple[np.ndarray, np.ndarray]:
    """Run inference on loader. Returns (y_true, y_proba)."""
    model.eval()
    all_proba, all_true = [], []
    with torch.no_grad():
        for Xb, yb in loader:
            Xb = Xb.to(DEVICE, non_blocking=True)
            proba = model.predict_proba(Xb).cpu().numpy()
            all_proba.append(proba)
            all_true.append(yb.numpy())
    return np.concatenate(all_true), np.concatenate(all_proba)


def _fold_metrics(
    y_true: np.ndarray,
    y_proba: np.ndarray,
    threshold: float,
    fold_label: str,
) -> dict:
    y_pred = (y_proba >= threshold).astype(int)
    acc  = accuracy_score(y_true, y_pred)
    prec = precision_score(y_true, y_pred, zero_division=0)
    rec  = recall_score(y_true, y_pred, zero_division=0)
    auc  = roc_auc_score(y_true, y_proba)
    ll   = log_loss(y_true, y_proba)
    nsig = int(y_pred.sum())

    print(f"\n  {fold_label}")
    print(f"    ROC-AUC: {auc:.4f}  Log-loss: {ll:.4f}  Accuracy: {acc:.4f}")
    for t in [0.50, 0.52, 0.55, 0.60]:
        yp = (y_proba >= t).astype(int)
        p  = precision_score(y_true, yp, zero_division=0)
        r  = recall_score(y_true, yp, zero_division=0)
        print(f"      t={t:.2f}  prec={p:.3f}  recall={r:.3f}  signals={int(yp.sum()):,}")
    print(f"    At t={threshold}: Prec={prec:.4f}  Recall={rec:.4f}  "
          f"Signals={nsig:,}/{len(y_pred):,} ({nsig/len(y_pred)*100:.1f}%)")

    return {
        "accuracy":  round(acc,  4),
        "precision": round(prec, 4),
        "recall":    round(rec,  4),
        "roc_auc":   round(auc,  4),
        "log_loss":  round(ll,   4),
        "signals":   nsig,
        "test_bars": len(y_pred),
    }


# ── One-direction training ────────────────────────────────────────────────────

def _train_one_direction(
    X: np.ndarray,
    y: np.ndarray,
    timestamps: pd.DatetimeIndex,
    direction: str,
) -> tuple:
    """
    Walk-forward CV + final model for one direction.

    Returns (final_model, fold_metrics_list, avg_metrics_dict).
    """
    print(f"\n{'='*60}")
    print(f"  Training {direction.upper()} CNN  (device: {DEVICE})")
    print(f"  Params: {count_params(CNNPatternModel()):,}  |  "
          f"X.shape={X.shape}")
    print(f"{'='*60}")

    splits       = walk_forward_splits(timestamps, CONFIG["n_folds"])
    fold_metrics = []
    best_auc     = 0.0
    best_state   = None

    n = len(X)
    for train_idx, test_idx, test_year in splits:
        # Guard against off-by-one from duplicate timestamps in raw data
        train_idx = train_idx[train_idx < n]
        test_idx  = test_idx[test_idx < n]
        if len(test_idx) == 0:
            print(f"  Skipping fold {test_year}: no test samples after bounds check")
            continue
        X_tr, X_te = X[train_idx], X[test_idx]
        y_tr, y_te = y[train_idx], y[test_idx]

        # Class-balanced loss weight
        pos_frac   = float(y_tr.mean())
        pos_weight = torch.tensor(
            [(1.0 - pos_frac) / (pos_frac + 1e-10)], device=DEVICE
        )

        train_loader = _make_loader(X_tr, y_tr, CONFIG["batch_size"], shuffle=True)
        val_loader   = _make_loader(X_te, y_te, CONFIG["batch_size"], shuffle=False)

        model = CNNPatternModel(
            window_size=CONFIG["window_size"],
            n_channels=CONFIG["n_channels"],
            dropout=CONFIG["dropout"],
        ).to(DEVICE)

        criterion = nn.BCEWithLogitsLoss(pos_weight=pos_weight)
        optimizer = torch.optim.Adam(
            model.parameters(),
            lr=CONFIG["lr"],
            weight_decay=CONFIG["weight_decay"],
        )
        scheduler = torch.optim.lr_scheduler.ReduceLROnPlateau(
            optimizer, mode="max", factor=0.5, patience=5
        )

        best_val_auc  = 0.0
        patience_cnt  = 0
        best_ep_state = None

        for epoch in range(CONFIG["max_epochs"]):
            # ── Train ──────────────────────────────────────────────────────
            model.train()
            epoch_loss = 0.0
            for Xb, yb in train_loader:
                Xb, yb = Xb.to(DEVICE, non_blocking=True), yb.to(DEVICE, non_blocking=True)
                optimizer.zero_grad()
                logits = model(Xb)
                loss   = criterion(logits, yb)
                loss.backward()
                optimizer.step()
                epoch_loss += loss.item()

            # ── Validate ───────────────────────────────────────────────────
            y_true, y_proba = _evaluate(model, val_loader)
            val_auc = roc_auc_score(y_true, y_proba)
            scheduler.step(val_auc)

            if val_auc > best_val_auc:
                best_val_auc  = val_auc
                patience_cnt  = 0
                best_ep_state = {k: v.cpu().clone() for k, v in model.state_dict().items()}
            else:
                patience_cnt += 1
                if patience_cnt >= CONFIG["patience"]:
                    print(f"    Early stop at epoch {epoch + 1}  "
                          f"(best val AUC={best_val_auc:.4f})")
                    break

            if (epoch + 1) % 10 == 0:
                print(f"    Epoch {epoch+1:3d}  loss={epoch_loss/len(train_loader):.4f}  "
                      f"val_auc={val_auc:.4f}  best={best_val_auc:.4f}")

        if best_ep_state:
            model.load_state_dict(best_ep_state)

        y_true, y_proba = _evaluate(model, val_loader)
        metrics = _fold_metrics(
            y_true, y_proba, CONFIG["threshold"],
            f"[{direction}] Fold test_year={test_year} "
            f"(train {len(train_idx):,} / test {len(test_idx):,})",
        )
        metrics["test_year"] = test_year
        fold_metrics.append(metrics)

        if metrics["roc_auc"] > best_auc:
            best_auc  = metrics["roc_auc"]
            best_state = best_ep_state

    avg = {
        k: round(float(np.mean([m[k] for m in fold_metrics])), 4)
        for k in ["accuracy", "precision", "recall", "roc_auc", "log_loss"]
    }
    print(f"\n  [{direction}] Walk-forward averages:")
    for k, v in avg.items():
        print(f"    {k:<12}: {v:.4f}")

    # ── Final model on all data ───────────────────────────────────────────
    print(f"\n  [{direction}] Fine-tuning final model on all {len(X):,} samples "
          f"(10 epochs, lr×0.1)...")
    final_model = CNNPatternModel(
        window_size=CONFIG["window_size"],
        n_channels=CONFIG["n_channels"],
        dropout=CONFIG["dropout"],
    ).to(DEVICE)
    if best_state:
        final_model.load_state_dict(best_state)

    pos_frac_all   = float(y.mean())
    pos_weight_all = torch.tensor(
        [(1.0 - pos_frac_all) / (pos_frac_all + 1e-10)], device=DEVICE
    )
    full_loader  = _make_loader(X, y, CONFIG["batch_size"], shuffle=True)
    crit_all     = nn.BCEWithLogitsLoss(pos_weight=pos_weight_all)
    opt_all      = torch.optim.Adam(
        final_model.parameters(),
        lr=CONFIG["lr"] * 0.1,
        weight_decay=CONFIG["weight_decay"],
    )

    final_model.train()
    for _ in range(10):
        for Xb, yb in full_loader:
            Xb, yb = Xb.to(DEVICE, non_blocking=True), yb.to(DEVICE, non_blocking=True)
            opt_all.zero_grad()
            loss = crit_all(final_model(Xb), yb)
            loss.backward()
            opt_all.step()

    final_model.eval()
    return final_model, fold_metrics, avg


# ── Main training pipeline ────────────────────────────────────────────────────

def train():
    t0 = time.time()
    print("=" * 60)
    print("Model 3 — 1D CNN Chart Pattern Training")
    print(f"Device : {DEVICE}")
    print(f"Window : {CONFIG['window_size']} bars × {CONFIG['n_channels']} channels")
    print("=" * 60)

    # ── 1. Load candles ───────────────────────────────────────────────────
    print("\n[1/4] Loading candles...")
    df = load_candles(CONFIG["db_path"], CONFIG["table"])

    model_dir = Path(CONFIG["model_dir"])
    model_dir.mkdir(parents=True, exist_ok=True)

    kw = dict(
        window_size    = CONFIG["window_size"],
        target_pips    = CONFIG["target_pips"],
        stop_pips      = CONFIG["stop_pips"],
        horizon        = CONFIG["horizon_bars"],
        session_filter = True,
    )

    # ── 2. Train LONG ─────────────────────────────────────────────────────
    print("\n[2/4] Building LONG dataset + Walk-forward CV...")
    X_long, y_long, ts_long = build_cnn_dataset_single(df, "long", **kw)
    long_model, long_fold_metrics, long_avg = _train_one_direction(
        X_long, y_long, ts_long, "LONG"
    )
    del X_long, y_long, ts_long
    gc.collect()

    # ── 3. Train SHORT ────────────────────────────────────────────────────
    short_model, short_fold_metrics, short_avg = None, [], {}
    if CONFIG.get("train_short_model", True):
        print("\n[3/4] Building SHORT dataset + Walk-forward CV...")
        X_short, y_short, ts_short = build_cnn_dataset_single(df, "short", **kw)
        short_model, short_fold_metrics, short_avg = _train_one_direction(
            X_short, y_short, ts_short, "SHORT"
        )
        del X_short, y_short, ts_short
    else:
        print("\n[3/4] Skipped (train_short_model=False)")

    del df
    gc.collect()

    # ── 4. Save ───────────────────────────────────────────────────────────
    print("\n[4/4] Saving models and metrics...")

    long_path   = model_dir / "model3_cnn_long.pt"
    compat_path = model_dir / "model3_cnn.pt"
    torch.save(long_model.state_dict(), str(long_path))
    torch.save(long_model.state_dict(), str(compat_path))
    print(f"Saved: {long_path}")
    print(f"Saved: {compat_path}")

    if short_model is not None:
        short_path = model_dir / "model3_cnn_short.pt"
        torch.save(short_model.state_dict(), str(short_path))
        print(f"Saved: {short_path}")

    metrics_path = model_dir / "model3_metrics.json"
    metrics_path.write_text(json.dumps({
        "config": CONFIG,
        "long": {
            "fold_metrics": long_fold_metrics,
            "avg_metrics":  long_avg,
        },
        "short": {
            "fold_metrics": short_fold_metrics,
            "avg_metrics":  short_avg,
        } if short_model is not None else None,
    }, indent=2, default=str))
    print(f"Saved: {metrics_path}")

    elapsed = time.time() - t0
    print(f"\nDone in {elapsed:.0f}s")
    return long_model


# ── Inference helper ──────────────────────────────────────────────────────────

def load_model(
    model_dir: str = "ml/trained",
    direction: str = "long",
) -> CNNPatternModel:
    """
    Load a saved Model 3 for inference.

    Args:
        direction: "long" or "short"

    Returns model in eval mode, ready for predict_proba().
    """
    d    = Path(model_dir)
    name = f"model3_cnn_{direction}.pt"
    if not (d / name).exists():
        name = "model3_cnn.pt"   # fallback to compat path

    model = CNNPatternModel(
        window_size=CONFIG["window_size"],
        n_channels=CONFIG["n_channels"],
        dropout=CONFIG["dropout"],
    )
    model.load_state_dict(torch.load(str(d / name), map_location="cpu"))
    model.eval()
    return model


if __name__ == "__main__":
    train()
