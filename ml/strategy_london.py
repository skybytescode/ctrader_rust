"""
London Breakout Strategy — Multi-Symbol
=========================================
Trade the breakout of the Asian session range during London open.
Supports EURUSD and XAUUSD with symbol-specific parameters.

Logic:
    1. Compute Asian range: high/low of 00:00-06:59 UTC
    2. Wait for London open (07:00-12:00 UTC)
    3. Enter LONG on first close above Asian high + buffer
       Enter SHORT on first close below Asian low - buffer
    4. TP = 1.5x range size, SL = opposite side of range
    5. Max 1 trade per day, daily bias filter from previous day

Usage:
    python -m ml.strategy_london --symbol EURUSD --direction both
    python -m ml.strategy_london --symbol XAUUSD --direction long
"""

import argparse
import sys
import time
from pathlib import Path

import numpy as np
import pandas as pd

PROJECT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PROJECT_ROOT))

DB_PATH = "Bots_db/Algo_EURUSD.duckdb"

# ── Session Parameters (shared) ────────────────────────────────────────────
ASIAN_START_HOUR = 0
ASIAN_END_HOUR = 7  # exclusive
ENTRY_START_HOUR = 7
ENTRY_END_HOUR = 12
SESSION_CLOSE_HOUR = 20

# TP/SL as multiples of Asian range (shared)
TP_RANGE_MULT = 1.5
SL_RANGE_MULT = 1.0

# ── Symbol-specific parameters ──────────────────────────────────────────────
SYMBOL_CONFIG = {
    "EURUSD": {
        "table": "eurusd_m1",
        "pip_size": 0.0001,         # 1 pip = 0.0001
        "pip_value": 0.10,          # $0.10 per pip per micro lot (1000 units)
        "spread_pips": 1.0,         # typical spread
        "buffer_pips": 2,           # breakout buffer
        "min_range_pips": 15,       # min Asian range
        "max_range_pips": 55,       # max Asian range
    },
    "XAUUSD": {
        "table": "xauusd_m1",
        "pip_size": 0.10,           # 1 pip = $0.10 for gold
        "pip_value": 0.01,          # $0.01 per pip per micro lot (1 oz)
        "spread_pips": 15,          # ~$1.50 ECN gold spread
        "buffer_pips": 20,          # $2.00 buffer
        "min_range_pips": 80,       # ~$8 min range
        "max_range_pips": 800,      # ~$80 max range (gold is volatile)
    },
}


def load_symbol_candles(symbol):
    """Load M1 candles for any symbol from DB."""
    import duckdb
    table = SYMBOL_CONFIG[symbol]["table"]
    con = duckdb.connect(DB_PATH, read_only=True)
    df = con.execute(
        f"SELECT timestamp, open, high, low, close, volume "
        f"FROM {table} ORDER BY timestamp"
    ).df()
    con.close()

    df["timestamp"] = pd.to_datetime(df["timestamp"], unit="s", utc=True)
    df = df.set_index("timestamp")
    print(f"Loaded {len(df):,} M1 candles from {table}  "
          f"({df.index[0]} -> {df.index[-1]})")
    return df


def run_backtest(start_date, initial_balance, trade_size, direction_filter="both",
                 symbol="EURUSD"):
    t0 = time.time()
    dir_label = direction_filter.upper() if direction_filter != "both" else "LONG+SHORT"
    cfg = SYMBOL_CONFIG[symbol]
    pip_size = cfg["pip_size"]
    pip_value = cfg["pip_value"]
    spread_pips = cfg["spread_pips"]
    buffer_pips = cfg["buffer_pips"]
    min_range_pips = cfg["min_range_pips"]
    max_range_pips = cfg["max_range_pips"]

    print("=" * 60)
    print(f"London Breakout — {symbol} [{dir_label}]")
    print("=" * 60)

    # Load data
    print(f"\n[1/1] Loading {symbol} M1 candles...")
    df = load_symbol_candles(symbol)

    start_ts = pd.Timestamp(start_date, tz="UTC")
    df_bt = df[df.index >= start_ts]

    # Group by day
    df_bt = df_bt.copy()
    df_bt["date"] = df_bt.index.date
    days = sorted(df_bt["date"].unique())

    print(f"\nStrategy parameters:")
    print(f"  Asian range:   {ASIAN_START_HOUR:02d}:00-{ASIAN_END_HOUR:02d}:00 UTC")
    print(f"  Entry window:  {ENTRY_START_HOUR:02d}:00-{ENTRY_END_HOUR:02d}:00 UTC")
    print(f"  Buffer:        {buffer_pips} pips above/below range")
    print(f"  TP:            {TP_RANGE_MULT}x Asian range")
    print(f"  SL:            Opposite side of range ({SL_RANGE_MULT}x)")
    print(f"  Range filter:  {min_range_pips}-{max_range_pips} pips")
    print(f"  Spread:        {spread_pips} pip/trade")
    print(f"  Direction:     {dir_label}")
    print(f"  Period:        {start_date} -> {days[-1] if days else 'N/A'}")
    print(f"  Days:          {len(days)}")
    print(f"  Balance:       ${initial_balance:.2f}")
    print(f"  Trade size:    ${trade_size:,.0f}")
    print()

    balance = initial_balance
    trades = []
    peak_balance = balance
    max_drawdown = 0.0
    monthly_pnl = {}
    skipped_reasons = {"weekend": 0, "no_range": 0, "range_too_small": 0,
                       "range_too_big": 0, "no_breakout": 0}

    for day in days:
        day_bars = df_bt[df_bt["date"] == day]

        # Skip weekends (shouldn't have data but just in case)
        if pd.Timestamp(day).weekday() >= 5:
            skipped_reasons["weekend"] += 1
            continue

        # ── 1. Compute Asian session range ───────────────────────────
        asian = day_bars[(day_bars.index.hour >= ASIAN_START_HOUR) &
                         (day_bars.index.hour < ASIAN_END_HOUR)]

        if len(asian) < 30:  # need at least 30 bars in Asian session
            skipped_reasons["no_range"] += 1
            continue

        asian_high = asian["high"].max()
        asian_low = asian["low"].min()
        asian_range = asian_high - asian_low
        range_pips = asian_range / pip_size

        if range_pips < min_range_pips:
            skipped_reasons["range_too_small"] += 1
            continue
        if range_pips > max_range_pips:
            skipped_reasons["range_too_big"] += 1
            continue

        # ── 2. Determine daily bias from previous day ────────────────
        day_idx = list(days).index(day)
        prev_day_bias = None
        if day_idx > 0:
            prev_day = days[day_idx - 1]
            prev_bars = df_bt[df_bt["date"] == prev_day]
            if len(prev_bars) > 0:
                prev_open = prev_bars["open"].iloc[0]
                prev_close = prev_bars["close"].iloc[-1]
                if prev_close > prev_open:
                    prev_day_bias = "long"
                else:
                    prev_day_bias = "short"

        # ── 3. Look for breakout in London session ───────────────────
        london = day_bars[(day_bars.index.hour >= ENTRY_START_HOUR) &
                          (day_bars.index.hour < ENTRY_END_HOUR)]

        if len(london) == 0:
            skipped_reasons["no_breakout"] += 1
            continue

        buffer = buffer_pips * pip_size
        breakout_high = asian_high + buffer
        breakout_low = asian_low - buffer

        # Find first breakout bar — only trade in direction of daily bias
        entry_bar = None
        direction = None

        for ts, row in london.iterrows():
            # LONG: only if bias is long (or no bias filter)
            if (direction_filter in ("long", "both")
                    and row["close"] > breakout_high
                    and (prev_day_bias != "short" or direction_filter == "long")):
                direction = "long"
                entry_bar = ts
                entry_price = row["close"]
                break
            # SHORT: only if bias is short (or no bias filter)
            if (direction_filter in ("short", "both")
                    and row["close"] < breakout_low
                    and (prev_day_bias != "long" or direction_filter == "short")):
                direction = "short"
                entry_bar = ts
                entry_price = row["close"]
                break

        if entry_bar is None:
            skipped_reasons["no_breakout"] += 1
            continue

        # ── 3. Compute TP/SL ─────────────────────────────────────────
        tp_dist = asian_range * TP_RANGE_MULT
        sl_dist = asian_range * SL_RANGE_MULT

        if direction == "long":
            tp_level = entry_price + tp_dist
            sl_level = entry_price - sl_dist
        else:
            tp_level = entry_price - tp_dist
            sl_level = entry_price + sl_dist

        tp_pips = tp_dist / pip_size
        sl_pips = sl_dist / pip_size

        # ── 4. Resolve trade using remaining bars of the day ─────────
        remaining = day_bars[day_bars.index > entry_bar]
        # Also include next day's early bars up to SESSION_CLOSE_HOUR
        close_mask = remaining.index.hour < SESSION_CLOSE_HOUR
        remaining = remaining[close_mask] if len(remaining[close_mask]) > 0 else remaining

        outcome = "TIMEOUT"
        exit_price = entry_price
        exit_time = entry_bar

        for ts, row in remaining.iterrows():
            if direction == "long":
                if row["high"] >= tp_level:
                    outcome = "WIN"
                    exit_price = tp_level
                    exit_time = ts
                    break
                if row["low"] <= sl_level:
                    outcome = "LOSS"
                    exit_price = sl_level
                    exit_time = ts
                    break
            else:
                if row["low"] <= tp_level:
                    outcome = "WIN"
                    exit_price = tp_level
                    exit_time = ts
                    break
                if row["high"] >= sl_level:
                    outcome = "LOSS"
                    exit_price = sl_level
                    exit_time = ts
                    break
        else:
            # Timeout — close at last bar
            if len(remaining) > 0:
                exit_price = remaining["close"].iloc[-1]
                exit_time = remaining.index[-1]

        # ── 5. Calculate P&L ─────────────────────────────────────────
        if direction == "long":
            pnl_dist = exit_price - entry_price
        else:
            pnl_dist = entry_price - exit_price

        pnl_pips = pnl_dist / pip_size
        microlots = trade_size / 1000.0
        spread_cost = spread_pips * pip_value * microlots
        pnl_usd = pnl_pips * pip_value * microlots - spread_cost

        balance += pnl_usd
        if balance > peak_balance:
            peak_balance = balance
        dd = (peak_balance - balance) / peak_balance * 100
        if dd > max_drawdown:
            max_drawdown = dd

        month_key = entry_bar.strftime("%Y-%m")
        monthly_pnl[month_key] = monthly_pnl.get(month_key, 0.0) + pnl_usd

        trade_num = len(trades) + 1
        trades.append({
            "entry_time": entry_bar,
            "exit_time": exit_time,
            "direction": direction,
            "asian_range_pips": range_pips,
            "tp_pips": tp_pips,
            "sl_pips": sl_pips,
            "outcome": outcome,
            "pnl_pips": pnl_pips,
            "pnl_usd": pnl_usd,
            "balance": balance,
        })

        if trade_num <= 20 or trade_num % 25 == 0:
            print(f"  #{trade_num:>4d}  {entry_bar.strftime('%Y-%m-%d %H:%M')}  "
                  f"{direction.upper():>5s}  "
                  f"range={range_pips:.0f}p  "
                  f"TP={tp_pips:.0f}p  SL={sl_pips:.0f}p  "
                  f"{outcome:>7s}  {pnl_pips:+.1f}p  P&L=${pnl_usd:+.2f}  "
                  f"bal=${balance:.2f}")

    elapsed = time.time() - t0

    # ── Results ──────────────────────────────────────────────────────────────
    total_trades = len(trades)
    wins = sum(1 for t in trades if t["outcome"] == "WIN")
    losses = sum(1 for t in trades if t["outcome"] == "LOSS")
    timeouts = sum(1 for t in trades if t["outcome"] == "TIMEOUT")
    pips_won = sum(t["pnl_pips"] for t in trades if t["pnl_pips"] > 0)
    pips_lost = sum(t["pnl_pips"] for t in trades if t["pnl_pips"] < 0)
    pips_net = pips_won + pips_lost
    total_spread_pips = total_trades * spread_pips
    total_pnl = balance - initial_balance

    print()
    print("=" * 60)
    print("BACKTEST RESULTS — London Breakout")
    print("=" * 60)
    print(f"  Symbol:           {symbol}")
    print(f"  Strategy:         Asian range breakout ({ENTRY_START_HOUR:02d}:00-{ENTRY_END_HOUR:02d}:00)")
    print(f"  TP/SL:            {TP_RANGE_MULT}x / {SL_RANGE_MULT}x Asian range")
    print(f"  Direction:        {dir_label}")
    print(f"  Period:           {start_date} -> {days[-1] if days else 'N/A'}")
    print(f"  Spread cost:      {spread_pips} pip/trade")
    print(f"  Duration:         {elapsed:.1f}s")
    print()
    print(f"  Total days:       {len(days)}")
    print(f"  Total trades:     {total_trades}")
    if total_trades > 0:
        print(f"  Wins:             {wins}  ({wins/total_trades*100:.1f}%)")
        print(f"  Losses:           {losses}  ({losses/total_trades*100:.1f}%)")
        print(f"  Timeouts:         {timeouts}  ({timeouts/total_trades*100:.1f}%)")
        months = len(monthly_pnl) if monthly_pnl else 1
        print(f"  Trades/month:     {total_trades/months:.1f}")

        avg_range = np.mean([t["asian_range_pips"] for t in trades])
        avg_tp = np.mean([t["tp_pips"] for t in trades])
        avg_sl = np.mean([t["sl_pips"] for t in trades])
        print(f"  Avg range:        {avg_range:.1f} pips")
        print(f"  Avg TP target:    {avg_tp:.1f} pips")
        print(f"  Avg SL target:    {avg_sl:.1f} pips")
    else:
        print("  No trades triggered.")
        print(f"\n  Skipped: {skipped_reasons}")
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

    # Skipped days breakdown
    print()
    print(f"  Skipped days:")
    for reason, count in sorted(skipped_reasons.items()):
        if count > 0:
            print(f"    {reason}: {count}")

    if monthly_pnl:
        print()
        print("  Monthly P&L:")
        for month, pnl in sorted(monthly_pnl.items()):
            bar = "+" * max(1, int(abs(pnl) / 2)) if pnl > 0 else "-" * max(1, int(abs(pnl) / 2))
            print(f"    {month}:  ${pnl:+8.2f}  {bar}")

    # Direction breakdown
    if direction_filter == "both" and total_trades > 0:
        long_trades = [t for t in trades if t["direction"] == "long"]
        short_trades = [t for t in trades if t["direction"] == "short"]
        if long_trades:
            lw = sum(1 for t in long_trades if t["outcome"] == "WIN")
            lp = sum(t["pnl_pips"] for t in long_trades)
            print(f"\n  LONG:   {len(long_trades)} trades, {lw/len(long_trades)*100:.0f}% win rate, "
                  f"{lp:+.0f} pips, ${sum(t['pnl_usd'] for t in long_trades):+.2f}")
        if short_trades:
            sw = sum(1 for t in short_trades if t["outcome"] == "WIN")
            sp = sum(t["pnl_pips"] for t in short_trades)
            print(f"  SHORT:  {len(short_trades)} trades, {sw/len(short_trades)*100:.0f}% win rate, "
                  f"{sp:+.0f} pips, ${sum(t['pnl_usd'] for t in short_trades):+.2f}")

    print()
    print("Backtest complete.")


def main():
    parser = argparse.ArgumentParser(description="London Breakout Strategy Backtest")
    parser.add_argument("--symbol", default="EURUSD", choices=["EURUSD", "XAUUSD"],
                        help="Symbol to backtest (default: EURUSD)")
    parser.add_argument("--direction", default="both", choices=["long", "short", "both"],
                        help="Trade direction (default: both)")
    parser.add_argument("--start", default="2025-03-01",
                        help="Start date (default: 2025-03-01)")
    parser.add_argument("--balance", type=float, default=672.0,
                        help="Initial balance in USD (default: 672)")
    parser.add_argument("--size", type=float, default=1000.0,
                        help="Trade size in USD (default: 1000)")
    args = parser.parse_args()

    run_backtest(args.start, args.balance, args.size, args.direction, args.symbol)


if __name__ == "__main__":
    main()
