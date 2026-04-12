"""
Backtest — M1 XGBoost (Fixed Pip Targets)
==========================================
Simulates trading from a start date using the trained M1 model.

Usage:
    python -m ml.backtest --direction long
    python -m ml.backtest --direction short
    python -m ml.backtest --direction both

Parameters:
    --direction  "long", "short", or "both" (default: both)
    --start      Start date (default: 2025-03-01)
    --balance    Initial balance in USD (default: 672)
    --size       Trade notional in USD (default: 1000)
"""

import argparse
import sys
import time
from pathlib import Path

import numpy as np
import pandas as pd

PROJECT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PROJECT_ROOT))

from ml.model1_technical.features import load_candles, compute_features

DB_PATH = "Bots_db/Algo_EURUSD.duckdb"
MODEL_DIR = "ml/trained"

# M1 fixed-pip parameters
TARGET_PIPS = 15
STOP_PIPS = 10
HORIZON_BARS = 120

# Pip constants for EURUSD
PIP_SIZE = 0.0001
PIP_VALUE_PER_MICROLOT = 0.10  # $0.10 per pip per 1000 units
SPREAD_PIPS = 1.0  # typical EURUSD spread


def load_model():
    """Load trained M1 XGBoost long and short models."""
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
        model_short.load_model(str(model_dir / "model1_technical.json"))

    long_feats = (model_dir / "model1_long_feature_names.txt").read_text().strip().split("\n")
    short_feats = (model_dir / "model1_short_feature_names.txt").read_text().strip().split("\n")

    return model_long, model_short, long_feats, short_feats


def predict_all_bars(features, model_long, model_short, long_feats, short_feats):
    """Run XGBoost predictions on all feature rows at once."""
    X_long = features.reindex(columns=long_feats, fill_value=0.0)
    X_short = features.reindex(columns=short_feats, fill_value=0.0)

    prob_long = model_long.predict_proba(X_long)[:, 1]
    prob_short = model_short.predict_proba(X_short)[:, 1]

    return (
        pd.Series(prob_long, index=features.index, name="prob_long"),
        pd.Series(prob_short, index=features.index, name="prob_short"),
    )


def resolve_trade(df, entry_idx, direction):
    """
    Simulate a fixed-pip trade from entry bar.
    Returns (outcome, exit_bar_idx, pnl_pips).
    """
    entry_price = df["close"].iloc[entry_idx]
    target_dist = TARGET_PIPS * PIP_SIZE
    stop_dist = STOP_PIPS * PIP_SIZE

    if direction == "long":
        tp_level = entry_price + target_dist
        sl_level = entry_price - stop_dist
    else:
        tp_level = entry_price - target_dist
        sl_level = entry_price + stop_dist

    end_idx = min(entry_idx + HORIZON_BARS + 1, len(df))

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


def run_backtest(start_date, initial_balance, trade_size, direction_filter="both"):
    t0 = time.time()
    dir_label = direction_filter.upper() if direction_filter != "both" else "LONG+SHORT"
    print("=" * 60)
    print(f"Backtest — M1 XGBoost (Fixed Pip) [{dir_label}]")
    print("=" * 60)

    # 1. Load data
    print(f"\n[1/3] Loading M1 candles...")
    df = load_candles(DB_PATH)

    # 2. Load model
    print(f"\n[2/3] Loading M1 model...")
    model_long, model_short, long_feats, short_feats = load_model()
    print(f"  Long features: {len(long_feats)}, Short features: {len(short_feats)}")

    # 3. Compute features & predictions
    print(f"\n[3/3] Computing features & running predictions...")
    features = compute_features(df)
    prob_long, prob_short = predict_all_bars(
        features, model_long, model_short, long_feats, short_feats
    )

    # Simulate trades
    start_ts = pd.Timestamp(start_date, tz="UTC")
    bt_indices = features.index[features.index >= start_ts]

    print(f"\nSimulating trades from {start_date}...")
    print(f"  Bars in range: {len(bt_indices):,}")
    print(f"  Initial balance: ${initial_balance:.2f}")
    print(f"  Trade size: ${trade_size:,.0f} ({trade_size/1000:.0f} micro lot)")
    print(f"  Direction:  {dir_label}")
    print(f"  Target:     {TARGET_PIPS}p / Stop: {STOP_PIPS}p / Horizon: {HORIZON_BARS} bars")
    print(f"  Spread:     {SPREAD_PIPS} pip per trade")
    print(f"  Threshold:  0.50")
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

        # Session filter: 07:00-21:00 UTC
        hour = ts.hour
        if hour < 7 or hour >= 21:
            continue

        # Get predictions
        pl = prob_long.loc[ts] if ts in prob_long.index else 0.0
        ps = prob_short.loc[ts] if ts in prob_short.index else 0.0

        # Determine direction
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
        else:
            if pl > ps and pl > threshold:
                direction = "long"
                confidence = pl
            elif ps > pl and ps > threshold:
                direction = "short"
                confidence = ps

        if direction is None:
            continue

        # Resolve trade
        outcome, exit_bar, pnl_dist = resolve_trade(df, bar_pos, direction)

        # P&L calculation
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
        })

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
    print(f"  Model:            M1 XGBoost (Fixed Pip)")
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
    parser = argparse.ArgumentParser(description="Backtest M1 XGBoost model")
    parser.add_argument("--direction", default="both", choices=["long", "short", "both"],
                        help="Trade direction (default: both)")
    parser.add_argument("--start", default="2025-03-01",
                        help="Start date (default: 2025-03-01)")
    parser.add_argument("--balance", type=float, default=672.0,
                        help="Initial balance in USD (default: 672)")
    parser.add_argument("--size", type=float, default=1000.0,
                        help="Trade size in USD (default: 1000)")
    args = parser.parse_args()

    run_backtest(args.start, args.balance, args.size, args.direction)


if __name__ == "__main__":
    main()
