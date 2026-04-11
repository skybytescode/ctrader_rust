"""
Real-time prediction script — 3-Layer Architecture
====================================================
Layer 1: FILTERS  — When NOT to trade
    - Session filter: only trade London/NY hours (07:00-21:00 UTC)
    - Spread filter: skip when spread > 1.5x session average

Layer 2: SIGNAL   — Two models blended
    - M1 XGBoost (technical indicators, fixed pip targets) — 40% weight
    - Signal LightGBM (ATR-based targets) — 60% weight

Layer 3: SIZING   — Confidence -> position size suggestion

Usage:
    py -3.12 ml/predict_all.py              # run once, print JSON
    py -3.12 ml/predict_all.py --loop 60    # run every 60s, write to DB
"""

import sys
import os
import json
import time
import warnings
warnings.filterwarnings("ignore")

import numpy as np
import pandas as pd

DB_PATH = "ctrader.duckdb"
MODEL_DIR = "ml/trained"
M1_BARS_NEEDED = 250  # enough for 200-bar indicators + warmup


# ═══════════════════════════════════════════════════════════════════════════
#  DATA LOADING
# ═══════════════════════════════════════════════════════════════════════════

def load_m1_data():
    """Load last N M1 bars from DuckDB."""
    import duckdb
    con = duckdb.connect(DB_PATH, read_only=True)
    df = con.execute(f"""
        SELECT timestamp, open, high, low, close, volume
        FROM eurusd_m1
        ORDER BY timestamp DESC
        LIMIT {M1_BARS_NEEDED}
    """).fetchdf()
    con.close()

    if df.empty:
        raise ValueError("No M1 data in DB")

    df = df.sort_values("timestamp").reset_index(drop=True)

    ts = df["timestamp"]
    if ts.iloc[0] > 1e12:
        df.index = pd.to_datetime(ts, unit="ms", utc=True)
    else:
        df.index = pd.to_datetime(ts, unit="s", utc=True)
    df.index.name = "datetime"

    return df


def load_tick_spread(df):
    """Add spread columns with defaults (tick table may not exist)."""
    df["spread_mean_pips"] = 1.0
    df["spread_max_pips"] = 2.0
    df["spread_std_pips"] = 0.3
    df["tick_count_ratio"] = 1.0
    df["wide_spread_ratio"] = 0.05
    df["spread_cost_pct"] = 0.001
    df["order_flow_delta"] = 0.0
    df["uptick_ratio"] = 0.5
    return df


# ═══════════════════════════════════════════════════════════════════════════
#  LAYER 1: FILTERS
# ═══════════════════════════════════════════════════════════════════════════

def check_session_filter(current_time):
    """
    Session filter: only trade during London + NY hours.
    Tradeable: 07:00-21:00 UTC (covers London open through NY close).
    """
    hour = current_time.hour
    is_tradeable = 7 <= hour < 21
    session = "Off-hours"
    if 7 <= hour < 12:
        session = "London"
    elif 12 <= hour < 13:
        session = "London (pre-NY)"
    elif 13 <= hour < 16:
        session = "London/NY Overlap"
    elif 16 <= hour < 21:
        session = "New York"

    return {
        "trade_allowed": is_tradeable,
        "session": session,
        "reason": None if is_tradeable else f"Outside trading hours ({session})",
    }


def check_spread_filter(df, session_avg_multiplier=1.5):
    """
    Spread filter: don't trade when current spread > 1.5x session average.
    Uses the last 60 bars (1 hour) as the session average baseline.
    """
    spread = df["spread_mean_pips"]
    current_spread = spread.iloc[-1]

    # Session average: last 60 bars
    session_avg = spread.iloc[-60:].mean() if len(spread) >= 60 else spread.mean()
    threshold = session_avg * session_avg_multiplier

    is_ok = current_spread <= threshold
    return {
        "trade_allowed": is_ok,
        "current_spread": round(float(current_spread), 2),
        "session_avg_spread": round(float(session_avg), 2),
        "threshold": round(float(threshold), 2),
        "reason": None if is_ok else f"Spread {current_spread:.1f} > {threshold:.1f} (1.5x avg)",
    }


def run_filters(df):
    """Run all Layer 1 filters. Returns combined filter result."""
    current_time = pd.Timestamp.utcnow()

    session = check_session_filter(current_time)
    spread = check_spread_filter(df)

    all_pass = session["trade_allowed"] and spread["trade_allowed"]

    reasons = []
    if not session["trade_allowed"]:
        reasons.append(session["reason"])
    if not spread["trade_allowed"]:
        reasons.append(spread["reason"])

    return {
        "trade_allowed": all_pass,
        "block_reasons": reasons if reasons else None,
        "session": session.get("session", "Unknown"),
        "spread_ok": spread["trade_allowed"],
        "current_spread": spread.get("current_spread"),
    }


# ═══════════════════════════════════════════════════════════════════════════
#  LAYER 2: SIGNAL (M1 XGBoost + LightGBM Signal)
# ═══════════════════════════════════════════════════════════════════════════

def predict_model1(df):
    """Model 1: Technical Indicators (XGBoost)."""
    try:
        if "ml/model1_technical" not in sys.path:
            sys.path.insert(0, "ml/model1_technical")
        from features import compute_features
        from train import load_model

        features = compute_features(df)
        if features.empty:
            return None, None

        model_long, feat_long = load_model(MODEL_DIR, "long")
        model_short, feat_short = load_model(MODEL_DIR, "short")

        row = features.iloc[[-1]]
        X_long = row.reindex(columns=feat_long, fill_value=0.0)
        X_short = row.reindex(columns=feat_short, fill_value=0.0)

        prob_long = float(model_long.predict_proba(X_long)[:, 1][0])
        prob_short = float(model_short.predict_proba(X_short)[:, 1][0])

        return prob_long, prob_short
    except Exception as e:
        print(f"Model1 error: {e}", file=sys.stderr)
        return None, None


def predict_signal(df):
    """Signal Model: LightGBM with ATR-based targets."""
    try:
        import lightgbm as lgb
        from pathlib import Path

        long_path = Path(MODEL_DIR) / "signal_long.txt"
        short_path = Path(MODEL_DIR) / "signal_short.txt"
        long_feat_path = Path(MODEL_DIR) / "signal_long_features.txt"
        short_feat_path = Path(MODEL_DIR) / "signal_short_features.txt"

        if not long_path.exists() or not short_path.exists():
            return None, None

        model_long = lgb.Booster(model_file=str(long_path))
        model_short = lgb.Booster(model_file=str(short_path))
        feat_long = long_feat_path.read_text().strip().split("\n")
        feat_short = short_feat_path.read_text().strip().split("\n")

        if "ml/model1_technical" not in sys.path:
            sys.path.insert(0, "ml/model1_technical")
        from features import compute_features

        features = compute_features(df)
        if features.empty:
            return None, None

        row = features.iloc[[-1]]
        X_long = row.reindex(columns=feat_long, fill_value=0.0)
        X_short = row.reindex(columns=feat_short, fill_value=0.0)

        prob_long = float(model_long.predict(X_long)[0])
        prob_short = float(model_short.predict(X_short)[0])

        return prob_long, prob_short
    except Exception as e:
        print(f"Signal model error: {e}", file=sys.stderr)
        return None, None


# ═══════════════════════════════════════════════════════════════════════════
#  LAYER 3: SIZING
# ═══════════════════════════════════════════════════════════════════════════

def compute_sizing(signal_long, signal_short, m1_long, m1_short):
    """
    Compute position sizing suggestion based on signal confidence.
    Returns a sizing multiplier (0.5 = half size, 1.0 = full, 1.5 = 1.5x).
    """
    if signal_long is None and m1_long is None:
        return 0.0, "neutral"

    # Combine signal model with M1 model (signal model gets 60% weight)
    def combine(sig, m1):
        if sig is not None and m1 is not None:
            return sig * 0.6 + m1 * 0.4
        return sig if sig is not None else m1

    combined_long = combine(signal_long, m1_long)
    combined_short = combine(signal_short, m1_short)

    if combined_long is None or combined_short is None:
        return 0.0, "neutral"

    # Direction
    if combined_long > combined_short and combined_long > 0.5:
        direction = "LONG"
        confidence = combined_long
    elif combined_short > combined_long and combined_short > 0.5:
        direction = "SHORT"
        confidence = combined_short
    else:
        return 0.5, "neutral"

    # Size multiplier based on confidence
    if confidence >= 0.65:
        size_mult = 1.5
    elif confidence >= 0.55:
        size_mult = 1.0
    elif confidence >= 0.50:
        size_mult = 0.75
    else:
        size_mult = 0.5

    return size_mult, direction


# ═══════════════════════════════════════════════════════════════════════════
#  MAIN PREDICTION LOOP
# ═══════════════════════════════════════════════════════════════════════════

def run_predictions():
    """Run all layers once, return result dict."""
    result = {
        # Layer 1: Filters
        "trade_allowed": True,
        "block_reasons": None,
        "session": None,
        "spread_ok": True,
        "current_spread": None,
        # Layer 2: Signal
        "model1_long": None,
        "model1_short": None,
        "signal_long": None,
        "signal_short": None,
        # Layer 3: Sizing
        "direction": "neutral",
        "size_multiplier": 0.0,
        # Meta
        "timestamp": pd.Timestamp.utcnow().isoformat(),
        "error": None,
    }

    try:
        df = load_m1_data()
        df = load_tick_spread(df)

        # Layer 1: Filters
        filters = run_filters(df)
        result.update({
            "trade_allowed": filters["trade_allowed"],
            "block_reasons": filters["block_reasons"],
            "session": filters["session"],
            "spread_ok": filters["spread_ok"],
            "current_spread": filters["current_spread"],
        })

        # Layer 2: Signal (only if filters pass)
        if filters["trade_allowed"]:
            m1_long, m1_short = predict_model1(df)
            result["model1_long"] = m1_long
            result["model1_short"] = m1_short

            sig_long, sig_short = predict_signal(df)
            result["signal_long"] = sig_long
            result["signal_short"] = sig_short

            # Layer 3: Sizing
            size_mult, direction = compute_sizing(sig_long, sig_short, m1_long, m1_short)
            result["size_multiplier"] = size_mult
            result["direction"] = direction
        else:
            result["direction"] = "BLOCKED"
            result["size_multiplier"] = 0.0

    except Exception as e:
        result["error"] = str(e)

    return result


def write_to_db(result):
    """Write predictions to ml_predictions_live table (single row, overwritten)."""
    import duckdb
    con = duckdb.connect(DB_PATH)
    con.execute("CREATE TABLE IF NOT EXISTS ml_predictions_live (data VARCHAR)")
    con.execute("DELETE FROM ml_predictions_live")
    con.execute("INSERT INTO ml_predictions_live VALUES (?)", [json.dumps(result)])
    con.close()


def main():
    loop_mode = False
    interval = 60

    if "--loop" in sys.argv:
        loop_mode = True
        idx = sys.argv.index("--loop")
        if idx + 1 < len(sys.argv):
            try:
                interval = int(sys.argv[idx + 1])
            except ValueError:
                pass

    if not loop_mode:
        result = run_predictions()
        print(json.dumps(result, indent=2))
        return

    print(f"ML Stack v2: Filters -> Signal -> Sizing")
    print(f"  Loop interval: {interval}s")
    print(f"  Layer 1: Session + Spread filters")
    print(f"  Layer 2: M1-XGBoost + LightGBM Signal")
    print(f"  Layer 3: Confidence-based sizing")
    print()

    cycle = 0
    while True:
        cycle += 1
        t0 = time.time()
        try:
            result = run_predictions()
            write_to_db(result)
            elapsed = time.time() - t0

            allowed = "+" if result["trade_allowed"] else "x"
            session = result.get("session", "?")
            direction = result.get("direction", "?")
            size = result.get("size_multiplier", 0)

            if result["trade_allowed"]:
                m1 = result.get("model1_long")
                sig = result.get("signal_long")
                m1_str = f"M1={m1:.0%}" if m1 else "M1=N/A"
                sig_str = f"Sig={sig:.0%}" if sig else "Sig=N/A"
                print(f"ML #{cycle}: {allowed} {session} | "
                      f"{m1_str} {sig_str} -> {direction} x{size:.1f} ({elapsed:.1f}s)")
            else:
                reasons = result.get("block_reasons", [])
                reason_str = "; ".join(reasons) if reasons else "Unknown"
                print(f"ML #{cycle}: {allowed} BLOCKED: {reason_str} ({elapsed:.1f}s)")

        except Exception as e:
            print(f"ML #{cycle}: error: {e}", file=sys.stderr)
            try:
                write_to_db({"error": str(e), "timestamp": pd.Timestamp.utcnow().isoformat()})
            except Exception:
                pass

        elapsed = time.time() - t0
        sleep_time = max(1, interval - elapsed)
        time.sleep(sleep_time)


if __name__ == "__main__":
    main()
