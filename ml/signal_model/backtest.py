"""
Backtest — Signal LightGBM or M1 XGBoost
==========================================
Simulates trading from a start date using trained models.

Usage:
    python -m ml.signal_model.backtest --model signal
    python -m ml.signal_model.backtest --model m1

Parameters:
    --model     "signal" (LightGBM ATR-based) or "m1" (XGBoost fixed-pip)
    --start     Start date (default: 2025-03-01)
    --balance   Initial balance in USD (default: 672)
    --size      Trade notional in USD (default: 1000)
"""

import argparse
import sys
import time
from pathlib import Path

import numpy as np
import pandas as pd

PROJECT_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(PROJECT_ROOT))

from ml.model1_technical.features import load_candles, compute_features
from ml.signal_model.labels import (
    _compute_atr, ATR_PERIOD, TARGET_ATR_MULT, STOP_ATR_MULT, HORIZON_BARS,
    session_filter,
)

DB_PATH = "Bots_db/Algo_EURUSD.duckdb"
MODEL_DIR = "ml/trained"

# Model1 fixed-pip parameters (from model1 CONFIG)
M1_TARGET_PIPS = 15
M1_STOP_PIPS = 10
M1_HORIZON = 120

# Pip value for 1 micro lot (1000 units) EURUSD
PIP_SIZE = 0.0001
PIP_VALUE_PER_MICROLOT = 0.10  # $0.10 per pip per 1000 units


def load_signal_model():
    """Load trained Signal LightGBM models."""
    import lightgbm as lgb
    model_dir = Path(MODEL_DIR)

    long_model = lgb.Booster(model_file=str(model_dir / "signal_long.txt"))
    short_model = lgb.Booster(model_file=str(model_dir / "signal_short.txt"))
    long_feats = (model_dir / "signal_long_features.txt").read_text().strip().split("\n")
    short_feats = (model_dir / "signal_short_features.txt").read_text().strip().split("\n")

    return long_model, short_model, long_feats, short_feats


def load_m1_model():
    """Load trained M1 XGBoost models."""
    import xgboost as xgb
    model_dir = Path(MODEL_DIR)

    model_long = xgb.XGBClassifier()
    model_short = xgb.XGBClassifier()

    if (model_dir / "model1_long.json").exists():
        model_long.load_model(str(model_dir / "model1_long.json"))
    else:
        model_long.load_model(str(model_dir / "model1_technical.json"))

    if (model_dir / "model1_short.json").exists():
        model_short.load_model(str(model_dir / "model1_short.json"))
    else:
        model_long.load_model(str(model_dir / "model1_technical.json"))

    long_feat_file = model_dir / "model1_long_feature_names.txt"
    short_feat_file = model_dir / "model1_short_feature_names.txt"
    long_feats = long_feat_file.read_text().strip().split("\n")
    short_feats = short_feat_file.read_text().strip().split("\n")

    return model_long, model_short, long_feats, short_feats


def predict_all_bars(features, model_long, model_short, long_feats, short_feats,
                     is_signal_model):
    """Run predictions on all feature rows at once (vectorized)."""
    X_long = features.reindex(columns=long_feats, fill_value=0.0)
    X_short = features.reindex(columns=short_feats, fill_value=0.0)

    if is_signal_model:
        # LightGBM Booster: predict() returns probabilities directly
        prob_long = model_long.predict(X_long)
        prob_short = model_short.predict(X_short)
    else:
        # XGBoost: predict_proba()[:, 1]
        prob_long = model_long.predict_proba(X_long)[:, 1]
        prob_short = model_short.predict_proba(X_short)[:, 1]

    return (
        pd.Series(prob_long, index=features.index, name="prob_long"),
        pd.Series(prob_short, index=features.index, name="prob_short"),
    )


def resolve_trade(df, entry_idx, direction, atr_val, is_signal_model):
    """
    Simulate a trade from entry bar.
    Returns (outcome, exit_bar_idx, pnl_pips).
    """
    entry_price = df["close"].iloc[entry_idx]

    if is_signal_model:
        # ATR-based targets
        target_dist = atr_val * TARGET_ATR_MULT
        stop_dist = atr_val * STOP_ATR_MULT
        horizon = HORIZON_BARS
    else:
        # Fixed pip targets
        target_dist = M1_TARGET_PIPS * PIP_SIZE
        stop_dist = M1_STOP_PIPS * PIP_SIZE
        horizon = M1_HORIZON

    if direction == "long":
        tp_level = entry_price + target_dist
        sl_level = entry_price - stop_dist
    else:
        tp_level = entry_price - target_dist
        sl_level = entry_price + stop_dist

    end_idx = min(entry_idx + horizon + 1, len(df))

    for j in range(entry_idx + 1, end_idx):
        high = df["high"].iloc[j]
        low = df["low"].iloc[j]

        if direction == "long":
            if high >= tp_level:
                return "WIN", j, target_dist
            if low <= sl_level:
                return "LOSS", j, -stop_dist
        else:
            if low <= tp_level:
                return "WIN", j, target_dist
            if high >= sl_level:
                return "LOSS", j, -stop_dist

    # Timeout — close at current price
    exit_price = df["close"].iloc[end_idx - 1]
    if direction == "long":
        pnl_dist = exit_price - entry_price
    else:
        pnl_dist = entry_price - exit_price

    return "TIMEOUT", end_idx - 1, pnl_dist


SPREAD_PIPS = 1.0  # typical EURUSD spread


def run_backtest(model_type, start_date, initial_balance, trade_size,
                 direction_filter="both"):
    t0 = time.time()
    model_label = 'Signal LightGBM (ATR)' if model_type == 'signal' else 'M1 XGBoost (Fixed Pip)'
    dir_label = direction_filter.upper() if direction_filter != "both" else "LONG+SHORT"
    print("=" * 60)
    print(f"Backtest — {model_label} [{dir_label}]")
    print("=" * 60)

    is_signal = model_type == "signal"

    # 1. Load data
    print(f"\n[1/4] Loading M1 candles...")
    df = load_candles(DB_PATH)

    # 2. Load model
    print(f"\n[2/4] Loading {'Signal' if is_signal else 'M1'} model...")
    if is_signal:
        model_long, model_short, long_feats, short_feats = load_signal_model()
    else:
        model_long, model_short, long_feats, short_feats = load_m1_model()
    print(f"  Long features: {len(long_feats)}, Short features: {len(short_feats)}")

    # 3. Compute features on full dataset
    print(f"\n[3/4] Computing features...")
    features = compute_features(df)

    # Compute ATR for signal model
    atr = _compute_atr(df["high"], df["low"], df["close"], ATR_PERIOD)

    # ATR values indexed same as df

    # Run all predictions at once
    print("  Running predictions on all bars...")
    prob_long, prob_short = predict_all_bars(
        features, model_long, model_short, long_feats, short_feats, is_signal
    )

    # 4. Simulate trades
    start_ts = pd.Timestamp(start_date, tz="UTC")
    bt_mask = features.index >= start_ts
    bt_indices = features.index[bt_mask]

    print(f"\n[4/4] Simulating trades from {start_date}...")
    print(f"  Bars in range: {len(bt_indices):,}")
    print(f"  Initial balance: ${initial_balance:.2f}")
    print(f"  Trade size: ${trade_size:,} (1 micro lot)")
    print(f"  Direction:  {dir_label}")
    print(f"  Spread:     {SPREAD_PIPS} pip per trade")
    print(f"  Threshold:  0.50")
    print()

    balance = initial_balance
    trades = []
    cooldown_until = 0  # bar index, skip until trade resolves
    threshold = 0.50

    # Map datetime index to integer position for fast lookups
    idx_map = {ts: i for i, ts in enumerate(df.index)}

    wins = 0
    losses = 0
    timeouts = 0
    total_pnl = 0.0
    peak_balance = balance
    max_drawdown = 0.0
    monthly_pnl = {}

    for ts in bt_indices:
        bar_pos = idx_map.get(ts)
        if bar_pos is None:
            continue

        # Skip if in cooldown (active trade)
        if bar_pos < cooldown_until:
            continue

        # Session filter
        hour = ts.hour
        if hour < 7 or hour >= 21:
            continue

        # ATR check for signal model
        atr_val = atr.iloc[bar_pos] if bar_pos < len(atr) else np.nan
        if is_signal and (np.isnan(atr_val) or atr_val < 1e-10):
            continue

        # Get predictions
        pl = prob_long.loc[ts] if ts in prob_long.index else 0.0
        ps = prob_short.loc[ts] if ts in prob_short.index else 0.0

        # Determine direction (respecting direction_filter)
        direction = None
        confidence = 0.0
        if direction_filter == "long":
            if pl > threshold:
                direction = "long"
                confidence = pl
        elif direction_filter == "short":
            if ps > threshold:
                direction = "short"
                confidence = ps
        else:  # both
            if pl > ps and pl > threshold:
                direction = "long"
                confidence = pl
            elif ps > pl and ps > threshold:
                direction = "short"
                confidence = ps

        if direction is None:
            continue

        # Resolve trade
        outcome, exit_bar, pnl_dist = resolve_trade(
            df, bar_pos, direction, atr_val, is_signal
        )

        # Convert price distance to USD P&L (minus spread cost)
        pnl_pips = pnl_dist / PIP_SIZE
        # For $1000 = 1 micro lot: $0.10 per pip
        microlots = trade_size / 1000.0
        spread_cost = SPREAD_PIPS * PIP_VALUE_PER_MICROLOT * microlots
        pnl_usd = pnl_pips * PIP_VALUE_PER_MICROLOT * microlots - spread_cost

        balance += pnl_usd
        total_pnl += pnl_usd

        # Track peak and drawdown
        if balance > peak_balance:
            peak_balance = balance
        dd = (peak_balance - balance) / peak_balance * 100
        if dd > max_drawdown:
            max_drawdown = dd

        # Monthly tracking
        month_key = ts.strftime("%Y-%m")
        monthly_pnl[month_key] = monthly_pnl.get(month_key, 0.0) + pnl_usd

        if outcome == "WIN":
            wins += 1
        elif outcome == "LOSS":
            losses += 1
        else:
            timeouts += 1

        # Set cooldown until trade resolves
        cooldown_until = exit_bar + 1

        entry_price = df["close"].iloc[bar_pos]
        exit_time = df.index[exit_bar]
        trade_num = wins + losses + timeouts

        trades.append({
            "entry_time": ts,
            "exit_time": exit_time,
            "direction": direction,
            "confidence": confidence,
            "outcome": outcome,
            "pnl_pips": pnl_pips,
            "pnl_usd": pnl_usd,
            "balance": balance,
        })

        # Print periodic updates
        if trade_num <= 10 or trade_num % 50 == 0:
            print(f"  #{trade_num:>4d}  {ts.strftime('%Y-%m-%d %H:%M')}  "
                  f"{direction.upper():>5s}  conf={confidence:.3f}  "
                  f"{outcome:>7s}  {pnl_pips:+.1f}p  P&L=${pnl_usd:+.2f}  "
                  f"bal=${balance:.2f}")

    elapsed = time.time() - t0

    # ── Results ──────────────────────────────────────────────────────────────
    total_trades = wins + losses + timeouts
    pips_won = sum(t["pnl_pips"] for t in trades if t["pnl_pips"] > 0)
    pips_lost = sum(t["pnl_pips"] for t in trades if t["pnl_pips"] < 0)
    pips_net = pips_won + pips_lost
    total_spread_pips = total_trades * SPREAD_PIPS

    print()
    print("=" * 60)
    print("BACKTEST RESULTS")
    print("=" * 60)
    print(f"  Model:            {model_label}")
    print(f"  Direction:        {dir_label}")
    print(f"  Period:           {start_date} -> {bt_indices[-1].strftime('%Y-%m-%d') if len(bt_indices) > 0 else 'N/A'}")
    print(f"  Spread cost:      {SPREAD_PIPS} pip/trade")
    print(f"  Duration:         {elapsed:.1f}s")
    print()
    print(f"  Total trades:     {total_trades}")
    if total_trades > 0:
        print(f"  Wins:             {wins}  ({wins/total_trades*100:.1f}%)")
        print(f"  Losses:           {losses}  ({losses/total_trades*100:.1f}%)")
        print(f"  Timeouts:         {timeouts}  ({timeouts/total_trades*100:.1f}%)")
    else:
        print("  Wins: 0")
        print("  Losses: 0")
        print("  Timeouts: 0")
    print()
    print(f"  Pips won:         {pips_won:+.1f}")
    print(f"  Pips lost:        {pips_lost:+.1f}")
    print(f"  Pips net:         {pips_net:+.1f}")
    print(f"  Spread paid:      {total_spread_pips:.1f} pips ({total_trades} trades)")
    print(f"  Net after spread: {pips_net - total_spread_pips:+.1f} pips")
    print()
    print(f"  Initial balance:  ${initial_balance:.2f}")
    print(f"  Final balance:    ${balance:.2f}")
    print(f"  Total P&L:        ${total_pnl:+.2f}  ({total_pnl/initial_balance*100:+.1f}%)")
    print(f"  Max drawdown:     {max_drawdown:.1f}%")

    if total_trades > 0:
        avg_win = np.mean([t["pnl_usd"] for t in trades if t["outcome"] == "WIN"]) if wins > 0 else 0
        avg_loss = np.mean([t["pnl_usd"] for t in trades if t["outcome"] == "LOSS"]) if losses > 0 else 0
        print(f"  Avg win:          ${avg_win:+.2f}")
        print(f"  Avg loss:         ${avg_loss:+.2f}")
        if losses > 0 and avg_loss != 0:
            print(f"  Profit factor:    {abs(avg_win * wins / (avg_loss * losses)):.2f}")
        elif wins > 0:
            print(f"  Profit factor:    inf")

    # Monthly breakdown
    if monthly_pnl:
        print()
        print("  Monthly P&L:")
        for month, pnl in sorted(monthly_pnl.items()):
            bar = "+" * int(abs(pnl) / 2) if pnl >= 0 else "-" * int(abs(pnl) / 2)
            print(f"    {month}:  ${pnl:+8.2f}  {bar}")

    print()
    print("Backtest complete.")


def run_combined_backtest(start_date, initial_balance, trade_size,
                         direction_filter="both"):
    """
    Combined backtest: only trade when BOTH Signal LightGBM and M1 XGBoost
    agree on direction. Uses Signal ATR-based targets for trade resolution.
    """
    t0 = time.time()
    dir_label = direction_filter.upper() if direction_filter != "both" else "LONG+SHORT"
    print("=" * 60)
    print(f"Backtest — COMBINED (Signal + M1 Agreement) [{dir_label}]")
    print("=" * 60)

    # 1. Load data
    print(f"\n[1/5] Loading M1 candles...")
    df = load_candles(DB_PATH)

    # 2. Load both models
    print(f"\n[2/5] Loading Signal LightGBM model...")
    sig_long, sig_short, sig_long_f, sig_short_f = load_signal_model()
    print(f"  Signal features: long={len(sig_long_f)}, short={len(sig_short_f)}")

    print(f"\n[3/5] Loading M1 XGBoost model...")
    m1_long, m1_short, m1_long_f, m1_short_f = load_m1_model()
    print(f"  M1 features: long={len(m1_long_f)}, short={len(m1_short_f)}")

    # 3. Compute features
    print(f"\n[4/5] Computing features & predictions...")
    features = compute_features(df)
    atr = _compute_atr(df["high"], df["low"], df["close"], ATR_PERIOD)

    # Predictions from both models
    print("  Signal predictions...")
    sig_pl, sig_ps = predict_all_bars(features, sig_long, sig_short,
                                      sig_long_f, sig_short_f, True)
    print("  M1 predictions...")
    m1_pl, m1_ps = predict_all_bars(features, m1_long, m1_short,
                                     m1_long_f, m1_short_f, False)

    # 4. Simulate
    start_ts = pd.Timestamp(start_date, tz="UTC")
    bt_mask = features.index >= start_ts
    bt_indices = features.index[bt_mask]

    print(f"\n[5/5] Simulating trades from {start_date}...")
    print(f"  Bars in range: {len(bt_indices):,}")
    print(f"  Initial balance: ${initial_balance:.2f}")
    print(f"  Trade size: ${trade_size:,}")
    print(f"  Direction:  {dir_label}")
    print(f"  Spread:     {SPREAD_PIPS} pip per trade")
    print(f"  Rule:       Both models must agree (>0.50) on same direction")
    print(f"  Targets:    Signal ATR-based (1.5R target, 1.0R stop)")
    print()

    balance = initial_balance
    trades = []
    cooldown_until = 0
    threshold = 0.50
    idx_map = {ts: i for i, ts in enumerate(df.index)}

    wins = 0
    losses = 0
    timeouts = 0
    total_pnl = 0.0
    peak_balance = balance
    max_drawdown = 0.0
    monthly_pnl = {}

    for ts in bt_indices:
        bar_pos = idx_map.get(ts)
        if bar_pos is None:
            continue

        if bar_pos < cooldown_until:
            continue

        # Session filter
        hour = ts.hour
        if hour < 7 or hour >= 21:
            continue

        # ATR check
        atr_val = atr.iloc[bar_pos] if bar_pos < len(atr) else np.nan
        if np.isnan(atr_val) or atr_val < 1e-10:
            continue

        # Get predictions from BOTH models
        s_long = sig_pl.loc[ts] if ts in sig_pl.index else 0.0
        s_short = sig_ps.loc[ts] if ts in sig_ps.index else 0.0
        m_long = m1_pl.loc[ts] if ts in m1_pl.index else 0.0
        m_short = m1_ps.loc[ts] if ts in m1_ps.index else 0.0

        # Both models must agree on direction above threshold
        direction = None
        confidence = 0.0

        long_agree = s_long > threshold and m_long > threshold
        short_agree = s_short > threshold and m_short > threshold

        if direction_filter == "long":
            if long_agree:
                direction = "long"
                confidence = s_long  # report signal confidence
        elif direction_filter == "short":
            if short_agree:
                direction = "short"
                confidence = s_short
        else:  # both
            if long_agree and short_agree:
                # Both agree on both — pick higher combined confidence
                combined_long = s_long * 0.6 + m_long * 0.4
                combined_short = s_short * 0.6 + m_short * 0.4
                if combined_long >= combined_short:
                    direction = "long"
                    confidence = combined_long
                else:
                    direction = "short"
                    confidence = combined_short
            elif long_agree:
                direction = "long"
                confidence = s_long * 0.6 + m_long * 0.4
            elif short_agree:
                direction = "short"
                confidence = s_short * 0.6 + m_short * 0.4

        if direction is None:
            continue

        # Resolve using Signal ATR-based targets
        outcome, exit_bar, pnl_dist = resolve_trade(
            df, bar_pos, direction, atr_val, True  # ATR-based
        )

        pnl_pips = pnl_dist / PIP_SIZE
        microlots = trade_size / 1000.0
        spread_cost = SPREAD_PIPS * PIP_VALUE_PER_MICROLOT * microlots
        pnl_usd = pnl_pips * PIP_VALUE_PER_MICROLOT * microlots - spread_cost

        balance += pnl_usd
        total_pnl += pnl_usd

        if balance > peak_balance:
            peak_balance = balance
        dd = (peak_balance - balance) / peak_balance * 100
        if dd > max_drawdown:
            max_drawdown = dd

        month_key = ts.strftime("%Y-%m")
        monthly_pnl[month_key] = monthly_pnl.get(month_key, 0.0) + pnl_usd

        if outcome == "WIN":
            wins += 1
        elif outcome == "LOSS":
            losses += 1
        else:
            timeouts += 1

        cooldown_until = exit_bar + 1
        exit_time = df.index[exit_bar]
        trade_num = wins + losses + timeouts

        trades.append({
            "entry_time": ts,
            "exit_time": exit_time,
            "direction": direction,
            "confidence": confidence,
            "outcome": outcome,
            "pnl_pips": pnl_pips,
            "pnl_usd": pnl_usd,
            "balance": balance,
            "sig_long": s_long, "sig_short": s_short,
            "m1_long": m_long, "m1_short": m_short,
        })

        if trade_num <= 20 or trade_num % 25 == 0:
            print(f"  #{trade_num:>4d}  {ts.strftime('%Y-%m-%d %H:%M')}  "
                  f"{direction.upper():>5s}  "
                  f"sig={s_long:.3f}/{s_short:.3f}  "
                  f"m1={m_long:.3f}/{m_short:.3f}  "
                  f"{outcome:>7s}  {pnl_pips:+.1f}p  P&L=${pnl_usd:+.2f}  "
                  f"bal=${balance:.2f}")

    elapsed = time.time() - t0

    total_trades = wins + losses + timeouts
    pips_won = sum(t["pnl_pips"] for t in trades if t["pnl_pips"] > 0)
    pips_lost = sum(t["pnl_pips"] for t in trades if t["pnl_pips"] < 0)
    pips_net = pips_won + pips_lost
    total_spread_pips = total_trades * SPREAD_PIPS

    print()
    print("=" * 60)
    print("BACKTEST RESULTS — COMBINED (Signal + M1)")
    print("=" * 60)
    print(f"  Strategy:         Both models agree (>0.50) on direction")
    print(f"  Targets:          Signal ATR (1.5R target, 1.0R stop)")
    print(f"  Direction:        {dir_label}")
    print(f"  Period:           {start_date} -> {bt_indices[-1].strftime('%Y-%m-%d') if len(bt_indices) > 0 else 'N/A'}")
    print(f"  Spread cost:      {SPREAD_PIPS} pip/trade")
    print(f"  Duration:         {elapsed:.1f}s")
    print()
    print(f"  Total trades:     {total_trades}")
    if total_trades > 0:
        print(f"  Wins:             {wins}  ({wins/total_trades*100:.1f}%)")
        print(f"  Losses:           {losses}  ({losses/total_trades*100:.1f}%)")
        print(f"  Timeouts:         {timeouts}  ({timeouts/total_trades*100:.1f}%)")
    else:
        print("  Wins: 0")
        print("  Losses: 0")
        print("  Timeouts: 0")
    print()
    print(f"  Pips won:         {pips_won:+.1f}")
    print(f"  Pips lost:        {pips_lost:+.1f}")
    print(f"  Pips net:         {pips_net:+.1f}")
    print(f"  Spread paid:      {total_spread_pips:.1f} pips ({total_trades} trades)")
    print(f"  Net after spread: {pips_net - total_spread_pips:+.1f} pips")
    print()
    print(f"  Initial balance:  ${initial_balance:.2f}")
    print(f"  Final balance:    ${balance:.2f}")
    print(f"  Total P&L:        ${total_pnl:+.2f}  ({total_pnl/initial_balance*100:+.1f}%)")
    print(f"  Max drawdown:     {max_drawdown:.1f}%")

    if total_trades > 0:
        avg_win = np.mean([t["pnl_usd"] for t in trades if t["outcome"] == "WIN"]) if wins > 0 else 0
        avg_loss = np.mean([t["pnl_usd"] for t in trades if t["outcome"] == "LOSS"]) if losses > 0 else 0
        print(f"  Avg win:          ${avg_win:+.2f}")
        print(f"  Avg loss:         ${avg_loss:+.2f}")
        if losses > 0 and avg_loss != 0:
            print(f"  Profit factor:    {abs(avg_win * wins / (avg_loss * losses)):.2f}")
        elif wins > 0:
            print(f"  Profit factor:    inf")

    if monthly_pnl:
        print()
        print("  Monthly P&L:")
        for month, pnl in sorted(monthly_pnl.items()):
            bar = "+" * int(abs(pnl) / 2) if pnl >= 0 else "-" * int(abs(pnl) / 2)
            print(f"    {month}:  ${pnl:+8.2f}  {bar}")

    print()
    print("Backtest complete.")


def main():
    parser = argparse.ArgumentParser(description="Backtest signal or M1 model")
    parser.add_argument("--model", required=True, choices=["signal", "m1", "combined"],
                        help="Model to backtest: 'signal', 'm1', or 'combined'")
    parser.add_argument("--direction", default="both", choices=["long", "short", "both"],
                        help="Trade direction: 'long', 'short', or 'both' (default: both)")
    parser.add_argument("--start", default="2025-03-01",
                        help="Start date (default: 2025-03-01)")
    parser.add_argument("--balance", type=float, default=672.0,
                        help="Initial balance in USD (default: 672)")
    parser.add_argument("--size", type=float, default=1000.0,
                        help="Trade size in USD (default: 1000)")
    args = parser.parse_args()

    if args.model == "combined":
        run_combined_backtest(args.start, args.balance, args.size, args.direction)
    else:
        run_backtest(args.model, args.start, args.balance, args.size, args.direction)


if __name__ == "__main__":
    main()
