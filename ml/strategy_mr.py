"""
Mean-Reversion Strategy — EURUSD M1
====================================
Rule-based mean reversion with strict filtering.

Entry LONG:
    - M1 BB position < 0.10 (at/below lower Bollinger Band)
    - H1 trend is UP (h1_vs_ema21 > 0)
    - Time: 08:00-10:30 or 13:00-15:30 UTC
    - Max 8 trades/day, 15-bar cooldown

Entry SHORT:
    - M1 BB position > 0.90 (at/above upper Bollinger Band)
    - H1 trend is DOWN (h1_vs_ema21 < 0)
    - Same time/trade filters

Exit:
    - TP: 8 pips, SL: 5 pips (R:R 1.6:1)
    - Horizon: 60 bars (1 hour) timeout

Usage:
    python -m ml.strategy_mr --direction both
    python -m ml.strategy_mr --direction long
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

# ── Strategy Parameters ─────────────────────────────────────────────────────
# Entry filters — Price deviation from SMA20 + H1 trend
# dist_sma_20 measures how far M1 price is from its 20-bar SMA (as % of price)
# Negative = below SMA (oversold), Positive = above SMA (overbought)
DIST_SMA_THRESHOLD = 0.0012  # ~12 pips deviation on EURUSD

# Exit parameters — balanced R:R
TARGET_PIPS = 10
STOP_PIPS = 7
HORIZON_BARS = 90  # 1.5 hour timeout

# Risk management
MAX_TRADES_PER_DAY = 5
MIN_COOLDOWN_BARS = 30  # 30 min between trades

# Time windows (UTC hours) — London AM + London/NY overlap
TRADE_WINDOWS = [
    (8, 0, 10, 30),    # 08:00-10:30 UTC
    (13, 0, 15, 30),   # 13:00-15:30 UTC
]

# Pip constants
PIP_SIZE = 0.0001
PIP_VALUE_PER_MICROLOT = 0.10
SPREAD_PIPS = 1.0


def in_trade_window(ts):
    """Check if timestamp is within allowed trading windows."""
    h, m = ts.hour, ts.minute
    t = h * 60 + m
    for h1, m1, h2, m2 in TRADE_WINDOWS:
        start = h1 * 60 + m1
        end = h2 * 60 + m2
        if start <= t < end:
            return True
    return False


def resolve_trade(df, entry_idx, direction):
    """Simulate a trade with fixed pip TP/SL."""
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

    # Timeout
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
    print(f"Mean-Reversion Strategy [{dir_label}]")
    print("=" * 60)

    # Load data & compute features
    print(f"\n[1/2] Loading M1 candles...")
    df = load_candles(DB_PATH)

    print(f"\n[2/2] Computing features...")
    features = compute_features(df)

    start_ts = pd.Timestamp(start_date, tz="UTC")
    bt_indices = features.index[features.index >= start_ts]

    print(f"\nStrategy parameters:")
    print(f"  Entry:      dist_SMA20 < -{DIST_SMA_THRESHOLD} + H1 uptrend (long)")
    print(f"              dist_SMA20 > +{DIST_SMA_THRESHOLD} + H1 downtrend (short)")
    print(f"  TP/SL:      {TARGET_PIPS}p / {STOP_PIPS}p  (R:R = {TARGET_PIPS/STOP_PIPS:.1f}:1)")
    print(f"  Horizon:    {HORIZON_BARS} bars (1 hour)")
    print(f"  Windows:    08:00-10:30 + 13:00-15:30 UTC")
    print(f"  Max trades: {MAX_TRADES_PER_DAY}/day")
    print(f"  Cooldown:   {MIN_COOLDOWN_BARS} bars between trades")
    print(f"  Spread:     {SPREAD_PIPS} pip/trade")
    print(f"  Direction:  {dir_label}")
    print(f"  Period:     {start_date} -> present")
    print(f"  Balance:    ${initial_balance:.2f}")
    print(f"  Trade size: ${trade_size:,.0f}")
    print()

    # Map feature index to df position
    idx_map = {ts: i for i, ts in enumerate(df.index)}

    balance = initial_balance
    trades = []
    cooldown_until = 0
    daily_trades = {}

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

        # Cooldown
        if bar_pos < cooldown_until:
            continue

        # Time window filter
        if not in_trade_window(ts):
            continue

        # Daily trade limit
        day_key = ts.strftime("%Y-%m-%d")
        if daily_trades.get(day_key, 0) >= MAX_TRADES_PER_DAY:
            continue

        # Get feature values
        row = features.loc[ts]
        dist_sma = row.get("dist_sma_20", 0)
        h1_trend = row.get("h1_vs_ema21", 0)
        rsi = row.get("rsi_14", 50)
        m5_rsi = row.get("m5_rsi14", 50)

        if pd.isna(dist_sma) or pd.isna(h1_trend):
            continue

        # ── Entry logic ──────────────────────────────────────────────
        # Mean reversion: price overextended from SMA20 + H1 trend alignment
        # Buy when price is far below SMA20 in an H1 uptrend (snap back up)
        # Sell when price is far above SMA20 in an H1 downtrend (snap back down)
        direction = None

        # LONG: price well below SMA20 + H1 uptrend
        if (direction_filter in ("long", "both")
                and dist_sma < -DIST_SMA_THRESHOLD
                and h1_trend > 0):
            direction = "long"

        # SHORT: price well above SMA20 + H1 downtrend
        if (direction is None
                and direction_filter in ("short", "both")
                and dist_sma > DIST_SMA_THRESHOLD
                and h1_trend < 0):
            direction = "short"

        if direction is None:
            continue

        # ── Execute trade ────────────────────────────────────────────
        outcome, exit_bar, pnl_dist = resolve_trade(df, bar_pos, direction)

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

        cooldown_until = max(exit_bar + 1, bar_pos + MIN_COOLDOWN_BARS)
        daily_trades[day_key] = daily_trades.get(day_key, 0) + 1

        exit_time = df.index[exit_bar]
        trade_num = wins + losses + timeouts

        trades.append({
            "entry_time": ts,
            "exit_time": exit_time,
            "direction": direction,
            "rsi": rsi,
            "dist_sma": dist_sma,
            "h1_trend": h1_trend,
            "outcome": outcome,
            "pnl_pips": pnl_pips,
            "pnl_usd": pnl_usd,
            "balance": balance,
        })

        if trade_num <= 20 or trade_num % 25 == 0:
            print(f"  #{trade_num:>4d}  {ts.strftime('%Y-%m-%d %H:%M')}  "
                  f"{direction.upper():>5s}  "
                  f"dSMA={dist_sma:+.5f} RSI={rsi:.0f} H1={h1_trend:+.4f}  "
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
    print("BACKTEST RESULTS — Mean Reversion")
    print("=" * 60)
    print(f"  Strategy:         dist_SMA20 (>{DIST_SMA_THRESHOLD}) + H1 trend alignment")
    print(f"  TP/SL:            {TARGET_PIPS}p / {STOP_PIPS}p  (R:R = {TARGET_PIPS/STOP_PIPS:.1f}:1)")
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

        # Trades per month
        months = len(monthly_pnl) if monthly_pnl else 1
        print(f"  Trades/month:     {total_trades/months:.1f}")
    else:
        print("  No trades triggered.")
        print("\nBacktest complete.")
        return

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

    # Win rate by direction
    if direction_filter == "both" and total_trades > 0:
        long_trades = [t for t in trades if t["direction"] == "long"]
        short_trades = [t for t in trades if t["direction"] == "short"]
        if long_trades:
            lw = sum(1 for t in long_trades if t["outcome"] == "WIN")
            print(f"\n  LONG:   {len(long_trades)} trades, {lw/len(long_trades)*100:.0f}% win rate, "
                  f"${sum(t['pnl_usd'] for t in long_trades):+.2f}")
        if short_trades:
            sw = sum(1 for t in short_trades if t["outcome"] == "WIN")
            print(f"  SHORT:  {len(short_trades)} trades, {sw/len(short_trades)*100:.0f}% win rate, "
                  f"${sum(t['pnl_usd'] for t in short_trades):+.2f}")

    print()
    print("Backtest complete.")


def main():
    parser = argparse.ArgumentParser(description="Mean-Reversion Strategy Backtest")
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
