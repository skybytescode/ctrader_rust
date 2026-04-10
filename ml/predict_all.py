"""
Real-time prediction script for all ML models.
Runs continuously (every 60s) writing predictions to DuckDB table
`ml_predictions_live`, which the Rust app reads for the Claude prompt.

Usage:
    py -3.12 ml/predict_all.py              # run once, print JSON to stdout
    py -3.12 ml/predict_all.py --loop 60    # run every 60s, write to DB

Output table: ml_predictions_live (single row, overwritten each cycle)
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


def predict_model2(df):
    """Model 2: Regime Detection (HMM)."""
    try:
        if "ml/model2_regime" not in sys.path:
            sys.path.insert(0, "ml/model2_regime")
        from features import compute_features
        from train import load_model

        features = compute_features(df)
        if features.empty:
            return None, None

        model, scaler, state_map = load_model(MODEL_DIR)
        X = scaler.transform(features.iloc[[-1]])
        state = int(model.predict(X)[0])
        regime_name = state_map.get(state, f"Unknown({state})")

        return regime_name, state
    except Exception as e:
        print(f"Model2 error: {e}", file=sys.stderr)
        return None, None


def predict_model3(df):
    """Model 3: CNN Chart Pattern (PyTorch)."""
    try:
        import torch
        if "ml/model3_cnn" not in sys.path:
            sys.path.insert(0, "ml/model3_cnn")
        from train import load_model

        WINDOW = 60
        if len(df) < WINDOW:
            return None, None

        last_w = df.iloc[-WINDOW:]
        o = last_w["open"].values.astype(np.float32)
        h = last_w["high"].values.astype(np.float32)
        l = last_w["low"].values.astype(np.float32)
        c = last_w["close"].values.astype(np.float32)
        v = last_w["volume"].values.astype(np.float32)

        X = np.stack([o, h, l, c, v])

        entry_close = c[-1]
        win_range = h.max() - l.min() + 1e-8
        for ch in range(4):
            X[ch] = (X[ch] - entry_close) / win_range
        vol_mean = X[4].mean() + 1e-8
        X[4] = X[4] / vol_mean

        X_tensor = torch.from_numpy(X).unsqueeze(0).float()

        model_long = load_model(MODEL_DIR, "long")
        model_short = load_model(MODEL_DIR, "short")

        with torch.no_grad():
            prob_long = float(torch.sigmoid(model_long(X_tensor)).item())
            prob_short = float(torch.sigmoid(model_short(X_tensor)).item())

        return prob_long, prob_short
    except Exception as e:
        print(f"Model3 error: {e}", file=sys.stderr)
        return None, None


def predict_model4(df):
    """Model 4: Economic Calendar (XGBoost)."""
    try:
        if "ml/model4_econcal" not in sys.path:
            sys.path.insert(0, "ml/model4_econcal")
        from features import compute_ec_features
        from train import load_model

        import duckdb
        con = duckdb.connect(DB_PATH, read_only=True)

        try:
            df_ec = con.execute("""
                SELECT timestamp_utc, currency, volatility, event_name,
                       actual, forecast, previous
                FROM eurusd_ec_historical
                ORDER BY timestamp_utc
            """).fetchdf()
        except Exception:
            con.close()
            return None, None

        con.close()

        if df_ec.empty:
            return None, None

        df_ec["timestamp_utc"] = pd.to_datetime(df_ec["timestamp_utc"])
        df_ec["surprise"] = df_ec["actual"] - df_ec["forecast"]
        max_val = df_ec["surprise"].abs().quantile(0.99)
        df_ec["surprise_norm"] = (df_ec["surprise"] / (max_val + 1e-10)).clip(-1, 1)

        features = compute_ec_features(df.index, df_ec)
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
        print(f"Model4 error: {e}", file=sys.stderr)
        return None, None


def predict_unified(df):
    """Unified model (XGBoost on combined features)."""
    try:
        import xgboost as xgb
        from pathlib import Path

        model_path = Path(MODEL_DIR) / "unified_xgb.json"
        if not model_path.exists():
            return None

        feat_imp_path = Path(MODEL_DIR) / "unified_feature_importance.csv"
        if feat_imp_path.exists():
            fi = pd.read_csv(feat_imp_path)
            feature_names = fi["feature"].tolist()
        else:
            return None

        model = xgb.XGBClassifier()
        model.load_model(str(model_path))

        if "ml/model1_technical" not in sys.path:
            sys.path.insert(0, "ml/model1_technical")
        from features import compute_features
        features = compute_features(df)
        if features.empty:
            return None

        row = features.iloc[[-1]]
        X = row.reindex(columns=feature_names, fill_value=0.0)

        prob = float(model.predict_proba(X)[:, 1][0])
        return prob
    except Exception as e:
        print(f"Unified error: {e}", file=sys.stderr)
        return None


def run_predictions():
    """Run all models once, return result dict."""
    result = {
        "model1_long": None,
        "model1_short": None,
        "model2_regime": None,
        "model2_regime_id": None,
        "model3_long": None,
        "model3_short": None,
        "model4_long": None,
        "model4_short": None,
        "unified_prob": None,
        "timestamp": pd.Timestamp.utcnow().isoformat(),
        "error": None,
    }

    try:
        df = load_m1_data()
        df = load_tick_spread(df)

        m1_long, m1_short = predict_model1(df)
        result["model1_long"] = m1_long
        result["model1_short"] = m1_short

        regime, regime_id = predict_model2(df)
        result["model2_regime"] = regime
        result["model2_regime_id"] = regime_id

        m3_long, m3_short = predict_model3(df)
        result["model3_long"] = m3_long
        result["model3_short"] = m3_short

        m4_long, m4_short = predict_model4(df)
        result["model4_long"] = m4_long
        result["model4_short"] = m4_short

        unified = predict_unified(df)
        result["unified_prob"] = unified

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
    # Parse args
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
        # Single run: print JSON to stdout
        result = run_predictions()
        print(json.dumps(result))
        return

    # Loop mode: run predictions every N seconds, write to DB
    print(f"ML Predictions: starting loop mode (every {interval}s)")
    cycle = 0
    while True:
        cycle += 1
        t0 = time.time()
        try:
            result = run_predictions()
            write_to_db(result)
            elapsed = time.time() - t0

            # Summary for console
            m1_str = f"long={result['model1_long']:.0%}" if result["model1_long"] else "N/A"
            regime = result["model2_regime"] or "N/A"
            m3_str = f"long={result['model3_long']:.0%}" if result["model3_long"] else "N/A"
            print(f"ML #{cycle}: M1={m1_str} Regime={regime} CNN={m3_str} ({elapsed:.1f}s)")

        except Exception as e:
            print(f"ML #{cycle}: error: {e}", file=sys.stderr)
            try:
                write_to_db({"error": str(e), "timestamp": pd.Timestamp.utcnow().isoformat()})
            except Exception:
                pass

        # Sleep until next cycle
        elapsed = time.time() - t0
        sleep_time = max(1, interval - elapsed)
        time.sleep(sleep_time)


if __name__ == "__main__":
    main()
