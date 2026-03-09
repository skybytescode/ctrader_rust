"""
Model 1 — Technical Indicators Feature Engineering
===================================================
Computes ~97 features from EURUSD M1 candles stored in DuckDB.

All features are computed using only past data (shift(1) where needed).
No look-ahead bias.

Usage:
    from ml.model1_technical.features import load_candles, compute_features
    df_raw  = load_candles()
    df_feat = compute_features(df_raw)
"""

import numpy as np
import pandas as pd
import pandas_ta as ta
import duckdb

# ── Default paths ────────────────────────────────────────────────────────────
DB_PATH = "Bots_db/Algo_EURUSD.duckdb"
TABLE   = "eurusd_m1"


# ── Data loading ─────────────────────────────────────────────────────────────

TICK_FEATURES_TABLE = "eurusd_tick_features_m1"
# Bars with mean spread above this threshold are news/illiquid events — drop from training
WIDE_SPREAD_FILTER_PIPS = 1.5


def load_candles(db_path: str = DB_PATH, table: str = TABLE) -> pd.DataFrame:
    """
    Load M1 candles from DuckDB.
    Returns a DataFrame with columns [open, high, low, close, volume]
    indexed by a tz-aware UTC DatetimeIndex.
    """
    con = duckdb.connect(db_path, read_only=True)
    df  = con.execute(
        f"SELECT timestamp, open, high, low, close, volume "
        f"FROM {table} ORDER BY timestamp"
    ).df()
    con.close()

    df["timestamp"] = pd.to_datetime(df["timestamp"], unit="s", utc=True)
    df = df.set_index("timestamp")
    print(f"Loaded {len(df):,} M1 candles from {table}  "
          f"({df.index[0]} -> {df.index[-1]})")
    return df


def load_tick_features(
    db_path: str = DB_PATH,
    table: str   = TICK_FEATURES_TABLE,
) -> pd.DataFrame:
    """
    Load the pre-computed tick-level features from DuckDB.

    Columns returned:
        tick_count        — number of ticks in the M1 bar
        spread_mean_pips  — mean bid-ask spread during the bar (pips)
        spread_max_pips   — peak spread during the bar
        spread_std_pips   — spread volatility during the bar
        wide_spread_count — number of ticks with abnormally wide spread

    These are keyed to the same M1 timestamps as eurusd_m1.
    Bars with tick_count == 0 have no tick data and will be NaN after merge.
    """
    con = duckdb.connect(db_path, read_only=True)
    df  = con.execute(
        f"SELECT timestamp, tick_count, spread_mean_pips, "
        f"spread_max_pips, spread_std_pips, wide_spread_count "
        f"FROM {table} WHERE tick_count > 0 ORDER BY timestamp"
    ).df()
    con.close()

    df["timestamp"] = pd.to_datetime(df["timestamp"], unit="s", utc=True)
    df = df.set_index("timestamp")
    print(f"Loaded {len(df):,} tick-feature rows from {table}")
    return df


def add_tick_features(
    df_features: pd.DataFrame,
    df_ticks: pd.DataFrame,
    target_pips: int = 15,
) -> pd.DataFrame:
    """
    Join tick-level features onto the feature DataFrame and compute
    derived spread/activity metrics.

    New features added:
        tick_count_ratio   — tick_count / 20-bar rolling avg (relative activity)
        spread_mean_pips   — mean spread during bar (raw pips)
        spread_max_pips    — peak spread during bar
        spread_std_pips    — spread volatility
        wide_spread_ratio  — wide_spread_count / tick_count  (fraction of spiky ticks)
        spread_cost_pct    — spread_mean / target_pips  (spread as % of profit target)

    Bars with no tick data (NaN after join) are left as NaN and will be
    dropped by the existing dropna() call in build_dataset().
    """
    pip_cols = ["tick_count", "spread_mean_pips", "spread_max_pips",
                "spread_std_pips", "wide_spread_count"]

    # Left-join: preserve all feature rows, NaN where no tick data
    merged = df_features.join(df_ticks[pip_cols], how="left")

    # Relative tick activity (how busy this bar vs recent avg)
    tc = merged["tick_count"]
    tc_ma = tc.rolling(20, min_periods=5).mean()
    merged["tick_count_ratio"] = tc / (tc_ma + 1e-6)

    # Wide-spread fraction
    merged["wide_spread_ratio"] = (
        merged["wide_spread_count"] / (tc + 1e-6)
    )

    # Spread as a fraction of the profit target
    merged["spread_cost_pct"] = merged["spread_mean_pips"] / target_pips

    # Drop the raw wide_spread_count (replaced by ratio)
    merged = merged.drop(columns=["wide_spread_count", "tick_count"])

    n_added = len([c for c in merged.columns if c not in df_features.columns])
    print(f"Added {n_added} tick features  "
          f"(coverage: {merged['spread_mean_pips'].notna().sum():,} / {len(merged):,} bars "
          f"= {merged['spread_mean_pips'].notna().mean()*100:.1f}%)")
    return merged


# ── Helper ───────────────────────────────────────────────────────────────────

def _count_consecutive(bool_series: pd.Series) -> np.ndarray:
    """Count consecutive True values; resets to 0 on False."""
    arr    = bool_series.to_numpy(dtype=bool)
    result = np.zeros(len(arr), dtype=np.float32)
    count  = 0
    for i, v in enumerate(arr):
        count = count + 1 if v else 0
        result[i] = count
    return result


# ── Feature computation ───────────────────────────────────────────────────────

def compute_features(df: pd.DataFrame) -> pd.DataFrame:
    """
    Compute all ~86 technical indicator features.

    Input : raw OHLCV DataFrame (from load_candles)
    Output: feature-only DataFrame, same index, no OHLCV columns.
            Rows with NaN from warm-up periods are NOT dropped here —
            that happens in labels.py after merging with labels.
    """
    out   = pd.DataFrame(index=df.index)
    close = df["close"]
    high  = df["high"]
    low   = df["low"]
    open_ = df["open"]
    vol   = df["volume"]

    # ── 1. Price / Returns ───────────────────────────────────────────────────
    for n in [1, 5, 15, 30, 60]:
        out[f"return_{n}m"] = close.pct_change(n)

    out["hl_range"]   = (high - low) / close                          # normalized bar range
    out["body_size"]  = (close - open_).abs() / (close + 1e-10)       # normalized body
    out["upper_wick"] = (high  - np.maximum(open_, close)) / (close + 1e-10)
    out["lower_wick"] = (np.minimum(open_, close) - low)   / (close + 1e-10)

    # ── 2. Moving Averages ───────────────────────────────────────────────────
    emas = {}
    for n in [5, 10, 21, 50, 100, 200]:
        ema            = close.ewm(span=n, adjust=False).mean()
        emas[n]        = ema
        out[f"dist_ema_{n}"] = (close - ema) / (close + 1e-10)  # distance to EMA (normalized)

    out["dist_sma_20"]    = (close - close.rolling(20).mean()) / (close + 1e-10)
    out["ema5_vs_ema21"]  = (emas[5]  - emas[21])  / (close + 1e-10)
    out["ema21_vs_ema50"] = (emas[21] - emas[50])  / (close + 1e-10)
    out["ema50_vs_ema200"]= (emas[50] - emas[200]) / (close + 1e-10)

    # ── 3. Momentum ──────────────────────────────────────────────────────────
    out["rsi_14"] = ta.rsi(close, 14)
    out["rsi_5"]  = ta.rsi(close, 5)

    macd = ta.macd(close, fast=12, slow=26, signal=9)
    out["macd_line"]      = macd.iloc[:, 0] / (close + 1e-10)  # normalized
    out["macd_signal"]    = macd.iloc[:, 2] / (close + 1e-10)
    out["macd_histogram"] = macd.iloc[:, 1] / (close + 1e-10)

    stoch = ta.stoch(high, low, close, k=14, d=3)
    out["stoch_k"] = stoch.iloc[:, 0]
    out["stoch_d"] = stoch.iloc[:, 1]

    out["cci_14"]      = ta.cci(high, low, close, 14)
    out["williams_r"]  = ta.willr(high, low, close, 14)
    out["roc_10"]      = ta.roc(close, 10)
    out["momentum_10"] = (close - close.shift(10)) / (close + 1e-10)
    adx = ta.adx(high, low, close, 14)
    out["adx_14"] = adx.iloc[:, 0]
    out["dmp_14"] = adx.iloc[:, 1]   # +DI (bullish trend strength)
    out["dmn_14"] = adx.iloc[:, 2]   # -DI (bearish trend strength)

    # ── 4. Volatility ────────────────────────────────────────────────────────
    atr = ta.atr(high, low, close, 14)
    out["atr_14"]    = atr
    out["atr_ratio"] = atr / (close + 1e-10)

    bb = ta.bbands(close, length=20, std=2.0)
    bb_upper = bb.iloc[:, 0]
    bb_mid   = bb.iloc[:, 1]
    bb_lower = bb.iloc[:, 2]
    out["bb_width"]    = (bb_upper - bb_lower) / (bb_mid + 1e-10)
    out["bb_position"] = (close - bb_lower) / (bb_upper - bb_lower + 1e-10)

    out["std_20"] = close.rolling(20).std() / (close + 1e-10)

    kc = ta.kc(high, low, close, length=20)
    out["kc_width"] = (kc.iloc[:, 0] - kc.iloc[:, 2]) / (close + 1e-10)

    # Squeeze: BB inside KC (low volatility, breakout pending)
    out["squeeze"] = (
        (bb_upper < kc.iloc[:, 0]) & (bb_lower > kc.iloc[:, 2])
    ).astype(float)

    # ── 5. Volume ────────────────────────────────────────────────────────────
    vol_ma = vol.rolling(20).mean()
    out["volume_ratio"] = vol / (vol_ma + 1e-10)

    obv = ta.obv(close, vol)
    out["obv_change_5"]  = obv.pct_change(5)
    out["obv_change_20"] = obv.pct_change(20)

    out["mfi_14"] = ta.mfi(high, low, close, vol, 14)

    # ── 6. Candlestick Patterns ──────────────────────────────────────────────
    body    = (close - open_).abs()
    hl      = high - low + 1e-10
    upper_w = high - np.maximum(open_, close)
    lower_w = np.minimum(open_, close) - low
    is_bull = close > open_

    out["is_doji"]          = (body < hl * 0.10).astype(float)
    out["is_hammer"]        = (
        (lower_w > body * 2.0) & (upper_w < body * 0.5) & is_bull
    ).astype(float)
    out["is_shooting_star"] = (
        (upper_w > body * 2.0) & (lower_w < body * 0.5) & ~is_bull
    ).astype(float)

    out["is_engulfing_bull"] = (
        is_bull &
        (open_ < close.shift(1)) &
        (close > open_.shift(1))
    ).astype(float)
    out["is_engulfing_bear"] = (
        ~is_bull &
        (open_ > close.shift(1)) &
        (close < open_.shift(1))
    ).astype(float)

    out["is_inside_bar"]   = ((high < high.shift(1)) & (low > low.shift(1))).astype(float)
    out["is_pin_bar_bull"] = ((lower_w > hl * 0.60) & is_bull).astype(float)
    out["is_pin_bar_bear"] = ((upper_w > hl * 0.60) & ~is_bull).astype(float)

    # ── 7. Time / Session ────────────────────────────────────────────────────
    hour_frac = df.index.hour + df.index.minute / 60.0
    out["hour_sin"] = np.sin(2 * np.pi * hour_frac / 24)
    out["hour_cos"] = np.cos(2 * np.pi * hour_frac / 24)

    dow = df.index.dayofweek.astype(float)  # Mon=0 … Fri=4
    out["dow_sin"] = np.sin(2 * np.pi * dow / 5)
    out["dow_cos"] = np.cos(2 * np.pi * dow / 5)

    hour = df.index.hour
    out["is_london"]   = ((hour >= 8)  & (hour < 12)).astype(float)
    out["is_new_york"] = ((hour >= 13) & (hour < 17)).astype(float)
    out["is_overlap"]  = ((hour >= 13) & (hour < 16)).astype(float)

    # ── 8. Support / Resistance ──────────────────────────────────────────────
    for n in [10, 20, 50]:
        # Distance from recent swing high/low (shifted to avoid look-ahead)
        out[f"dist_swing_high_{n}"] = (
            high.rolling(n).max().shift(1) - close
        ) / (close + 1e-10)
        out[f"dist_swing_low_{n}"] = (
            close - low.rolling(n).min().shift(1)
        ) / (close + 1e-10)

    # Simple intraday pivot point (prior bar H+L+C / 3)
    pivot = (high.shift(1) + low.shift(1) + close.shift(1)) / 3
    out["dist_pivot"] = (close - pivot) / (close + 1e-10)

    # ── 9. Lookback Ranges & Slopes ──────────────────────────────────────────
    for n in [5, 15, 30, 60]:
        range_n = high.rolling(n).max() - low.rolling(n).min()
        out[f"range_{n}m"] = range_n / (close + 1e-10)
        out[f"slope_{n}m"] = (close - close.shift(n)) / (n * close + 1e-10)

    # ── 10. Consecutive Direction ────────────────────────────────────────────
    out["consec_bull"] = _count_consecutive(close > open_)
    out["consec_bear"] = _count_consecutive(close < open_)

    # Bars since swing high/low (within last 20)
    out["bars_since_swing_high"] = high.rolling(20).apply(
        lambda x: len(x) - 1 - int(np.argmax(x)), raw=True
    )
    out["bars_since_swing_low"] = low.rolling(20).apply(
        lambda x: len(x) - 1 - int(np.argmin(x)), raw=True
    )

    # ── 11. VWAP (session-anchored, resets at 00:00 UTC each day) ────────────
    typical_price = (high + low + close) / 3
    tp_vol        = typical_price * vol
    day_key       = df.index.normalize()          # midnight UTC — group key
    tp_vol_cum    = tp_vol.groupby(day_key).cumsum()
    vol_cum       = vol.groupby(day_key).cumsum()
    vwap          = tp_vol_cum / (vol_cum + 1e-10)

    out["dist_vwap"]    = (close - vwap) / (close + 1e-10)   # signed distance
    out["vwap_slope_5"] = vwap.diff(5)  / (close + 1e-10)    # trend of VWAP
    out["above_vwap"]   = (close > vwap).astype(float)        # binary: 1 above

    # ── 12. Ichimoku Cloud ────────────────────────────────────────────────────
    # Tenkan (9) and Kijun (26) lines
    tenkan = (high.rolling(9).max()  + low.rolling(9).min())  / 2
    kijun  = (high.rolling(26).max() + low.rolling(26).min()) / 2

    # Cloud at current bar = Senkou spans calculated 26 bars ago (projected fwd)
    # This is the correct no-look-ahead form: cloud values visible now were
    # computed 26 bars ago and projected forward to today.
    cloud_a = ((tenkan + kijun) / 2).shift(26)
    cloud_b = ((high.rolling(52).max() + low.rolling(52).min()) / 2).shift(26)

    cloud_top = np.maximum(cloud_a, cloud_b)
    cloud_bot = np.minimum(cloud_a, cloud_b)

    out["dist_cloud_top"]  = (close - cloud_top) / (close + 1e-10)
    out["dist_cloud_bot"]  = (close - cloud_bot) / (close + 1e-10)
    out["cloud_thickness"] = (cloud_top - cloud_bot) / (close + 1e-10)
    out["above_cloud"]     = (                          # +1 above, -1 below, 0 inside
        (close > cloud_top).astype(float) - (close < cloud_bot).astype(float)
    )
    out["tenkan_vs_kijun"] = (tenkan - kijun) / (close + 1e-10)

    # ── 13. Fibonacci Retracements ────────────────────────────────────────────
    # 50-bar swing high/low shifted 1 to avoid look-ahead
    swing_h   = high.rolling(50).max().shift(1)
    swing_l   = low.rolling(50).min().shift(1)
    fib_range = swing_h - swing_l + 1e-10

    for ratio, tag in [(0.236, "236"), (0.382, "382"), (0.500, "500"), (0.618, "618")]:
        level = swing_l + ratio * fib_range
        out[f"dist_fib_{tag}"] = (close - level) / (close + 1e-10)

    # Normalized position within the full swing range [0 = at low, 1 = at high]
    out["fib_position"] = (close - swing_l) / fib_range

    print(f"Computed {out.shape[1]} features over {len(out):,} bars")
    return out


# ── Cross-pair features ────────────────────────────────────────────────────────

CROSS_PAIRS_SYMS = ["gbpusd", "usdjpy", "usdchf", "audusd", "eurjpy"]


def load_cross_pair_candles(db_path: str = DB_PATH) -> dict:
    """
    Load M1 close prices for all 5 cross-pairs from DuckDB.
    Returns {symbol: pd.Series(close, index=DatetimeIndex)}.
    """
    con = duckdb.connect(db_path, read_only=True)
    result = {}
    for sym in CROSS_PAIRS_SYMS:
        table = f"{sym}_m1"
        try:
            df = con.execute(
                f"SELECT timestamp, close FROM {table} ORDER BY timestamp"
            ).df()
            df["timestamp"] = pd.to_datetime(df["timestamp"], unit="s", utc=True)
            df = df.set_index("timestamp")
            result[sym] = df["close"]
            print(f"  {sym.upper()}: {len(df):,} bars")
        except Exception as e:
            print(f"  WARNING: could not load {table}: {e}")
    con.close()
    return result


def compute_cross_pair_features(main_df: pd.DataFrame, cross_closes: dict) -> pd.DataFrame:
    """
    Compute cross-pair correlation features aligned to EURUSD M1 timestamps.

    For each of the 5 cross-pairs (25 features total):
        {SYM}_return_1m    — 1-bar percentage return
        {SYM}_return_5m    — 5-bar percentage return
        {SYM}_rsi14        — RSI(14)
        {SYM}_vs_ema21     — distance from EMA(21) normalised by close
        {SYM}_momentum_10  — 10-bar price momentum

    Composite features (3 additional):
        usd_strength_5m    — avg 5m return of USD-long pairs minus USD-short pairs
                             (proxy for DXY momentum; USDJPY+USDCHF-AUDUSD)
        risk_sentiment_5m  — AUDUSD 5m return (positive = risk-on)
        eur_divergence_5m  — EURUSD 5m return minus EURJPY 5m return
                             (positive = EUR strengthening vs JPY but not USD)
    """
    out = pd.DataFrame(index=main_df.index)

    for sym, close_series in cross_closes.items():
        # Forward-fill to handle any gaps (e.g., different tick times)
        close = close_series.reindex(main_df.index, method="ffill")
        s = sym.upper()

        out[f"{s}_return_1m"]   = close.pct_change(1)
        out[f"{s}_return_5m"]   = close.pct_change(5)
        out[f"{s}_rsi14"]       = ta.rsi(close, 14)
        ema21 = close.ewm(span=21, adjust=False).mean()
        out[f"{s}_vs_ema21"]    = (close - ema21) / (close + 1e-10)
        out[f"{s}_momentum_10"] = (close - close.shift(10)) / (close + 1e-10)

    # USD strength: average of USD-long minus USD-short 5m momentum
    usd_components = []
    for sym in ["usdjpy", "usdchf"]:
        if sym in cross_closes:
            usd_components.append(
                cross_closes[sym].reindex(main_df.index, method="ffill").pct_change(5)
            )
    if "audusd" in cross_closes:
        usd_components.append(
            -cross_closes["audusd"].reindex(main_df.index, method="ffill").pct_change(5)
        )
    if usd_components:
        out["usd_strength_5m"] = pd.concat(usd_components, axis=1).mean(axis=1)

    # Risk-on/risk-off: AUDUSD rises in risk-on environments
    if "audusd" in cross_closes:
        out["risk_sentiment_5m"] = (
            cross_closes["audusd"].reindex(main_df.index, method="ffill").pct_change(5)
        )

    # EUR divergence: EURUSD 5m momentum minus EURJPY 5m momentum
    # Positive → EUR strengthening vs JPY but not vs USD → JPY weakness
    if "eurjpy" in cross_closes:
        eurusd_ret = main_df["close"].pct_change(5)
        eurjpy_ret = cross_closes["eurjpy"].reindex(main_df.index, method="ffill").pct_change(5)
        out["eur_divergence_5m"] = eurusd_ret - eurjpy_ret

    print(f"Computed {out.shape[1]} cross-pair features "
          f"({len(cross_closes)} pairs x 5 + {out.shape[1] - len(cross_closes) * 5} composite)")
    return out


# ── Market regime feature (from Model 2 HMM) ─────────────────────────────────

def add_regime_feature(df_features: pd.DataFrame, main_df: pd.DataFrame) -> pd.DataFrame:
    """
    Load the trained Model 2 GaussianHMM and predict market regime for each bar.

    Adds two features:
        regime_state     — integer 0-3 (Viterbi decoded state)
        regime_prob_max  — posterior probability of the most-likely state

    The HMM is unsupervised (trained without labels) so using it as a feature
    does not introduce look-ahead bias from future trade outcomes.
    Falls back gracefully if Model 2 is not available.
    """
    model2_path = "ml/trained/model2_regime.pkl"
    try:
        import pickle
        with open(model2_path, "rb") as f:
            hmm_model = pickle.load(f)
    except FileNotFoundError:
        print(f"  Model 2 not found at {model2_path} — skipping regime feature")
        return df_features
    except Exception as e:
        print(f"  Could not load Model 2: {e} — skipping regime feature")
        return df_features

    # Recompute Model 2's 8 features (same as model2/features.py)
    close   = main_df["close"]
    log_ret = np.log(close / close.shift(1))
    vol20   = log_ret.rolling(20).std()
    vol5    = log_ret.rolling(5).std()
    atr14   = ta.atr(main_df["high"], main_df["low"], close, 14)
    hl_rng  = (main_df["high"] - main_df["low"]) / (close + 1e-10)
    ret_abs = log_ret.abs().rolling(20).mean()
    vol_rat = vol5 / (vol20 + 1e-10)

    spread = (
        df_features["spread_mean_pips"].fillna(0.0)
        if "spread_mean_pips" in df_features.columns
        else pd.Series(0.0, index=main_df.index)
    )

    hmm_feats = pd.DataFrame({
        "log_return":       log_ret,
        "realized_vol_20":  vol20,
        "realized_vol_5":   vol5,
        "atr_ratio":        atr14 / (close + 1e-10),
        "hl_range":         hl_rng,
        "spread_mean_pips": spread,
        "return_abs_20":    ret_abs,
        "vol_ratio":        vol_rat,
    }, index=main_df.index).fillna(0.0)

    X_hmm = hmm_feats.to_numpy(dtype=np.float64)

    try:
        # Viterbi decoding: best state sequence
        states = hmm_model.predict(X_hmm)
        # Posterior state probabilities (T × n_components)
        _, posteriors = hmm_model.score_samples(X_hmm)
        prob_max = posteriors.max(axis=1)

        df_features = df_features.copy()
        df_features["regime_state"]    = states.astype(np.float32)
        df_features["regime_prob_max"] = prob_max.astype(np.float32)
        unique_states, counts = np.unique(states, return_counts=True)
        state_pcts = {int(s): f"{c/len(states)*100:.1f}%" for s, c in zip(unique_states, counts)}
        print(f"  Regime feature added — state distribution: {state_pcts}")
    except Exception as e:
        print(f"  Warning: regime prediction failed: {e}")

    return df_features


def get_feature_names() -> list[str]:
    """Return the ordered list of all feature column names."""
    # Build a tiny dummy DataFrame to get column order
    idx = pd.date_range("2020-01-02 08:00", periods=300, freq="1min", tz="UTC")
    rng = np.random.default_rng(0)
    base = 1.10000
    dummy = pd.DataFrame({
        "open":   base + rng.normal(0, 0.0002, 300).cumsum(),
        "high":   0.0,
        "low":    0.0,
        "close":  0.0,
        "volume": rng.integers(100, 1000, 300).astype(float),
    }, index=idx)
    dummy["close"] = dummy["open"] + rng.normal(0, 0.0001, 300)
    dummy["high"]  = np.maximum(dummy["open"], dummy["close"]) + rng.uniform(0, 0.0002, 300)
    dummy["low"]   = np.minimum(dummy["open"], dummy["close"]) - rng.uniform(0, 0.0002, 300)
    feat = compute_features(dummy)
    return list(feat.columns)
