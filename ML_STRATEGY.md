# EURUSD ML Trading Bot — Strategy Plan

## Architecture Overview

```
Market data (live tick / M5 close)
        ↓
  Models 1–5  (specialist signal generators)
        ↓
  Model 6 / Ensemble  (combines all signals)
        ↓
  Trade decision: Buy / Sell / Flat
        +  position size (Kelly)
        +  SL / TP levels
```

Models 1–5 are the **eyes** of the bot. Model 6 is the **brain** that decides when and how much to trade. No single model trades alone.

---

## Final Model Summary

| # | Model | Type | What it learns | Input data |
|---|---|---|---|---|
| 1 | Technical Indicators | XGBoost | Price momentum, trend strength, overbought/oversold | M5 candles + features from M1, H1, H4, D1 |
| 2 | Regime Detection | HMM | Market state: trending, ranging, volatile, quiet | Volatility, returns distribution, spread patterns |
| 3 | Chart Patterns | 1D CNN | Visual price shapes: double tops, H&S, channels | Raw OHLC sliding windows (50–200 bars) |
| 4 | News & Economic Calendar | XGBoost | Price reaction to NFP, CPI, rate decisions, sentiment | Forex Factory events, FRED macro data, Gemini sentiment |
| 5 | Depth of Market (DoM) | XGBoost | Order flow imbalance, tick velocity, spread patterns | Tick-derived approximation (cTrader has no real L2) |
| 6 | Ensemble Meta-Model | XGBoost | How to combine all 5 signals optimally | Outputs of models 1–5 + portfolio state |

---

## Trading Style

- **Both scalper AND intraday simultaneously** — two independent position types
- Multiple open positions allowed at the same time
- Decision point: **every M5 close** (~288 decisions/day)

| Style | Horizon | Target | Stop |
|---|---|---|---|
| Scalp | 3–6 M5 bars (15–30 min) | 5–8 pips | 3–5 pips |
| Intraday | 12–24 M5 bars (1–2 hours) | 15–25 pips | 10–15 pips |

---

## Target Variable (Label)

**Ternary classification** — applied at M5 bar `t`, using future data from bars `t+1` onward:

```
+1  →  price moves UP   ≥ threshold pips within prediction horizon
-1  →  price moves DOWN ≥ threshold pips within prediction horizon
 0  →  price stays flat (noise zone — no trade)
```

Separate models for each style:
- **Scalp model**: horizon = 3–6 M5 bars, threshold = 5–8 pips
- **Intraday model**: horizon = 12–24 M5 bars, threshold = 15–25 pips

The `0` (flat) class filters noise and maps directly to "don't trade."

---

## Model 1 — Technical Indicators (XGBoost) — ~86 Features

### Data Requirements

| Data | Timeframe | History | Rows | Source |
|---|---|---|---|---|
| M1 candles | 1-min OHLCV | 2–3 years | ~1.5M | cTrader GetTrendbars |
| Tick data | Every bid/ask tick | 1–2 years | ~50–100M | cTrader GetTickData |
| GBPUSD M1 | Cross-pair | 2 years | ~1.5M | cTrader GetTrendbars |
| USDJPY M1 | Cross-pair | 2 years | ~1.5M | cTrader GetTrendbars |
| USDCHF M1 | Cross-pair | 2 years | ~1.5M | cTrader GetTrendbars |
| XAUUSD M1 | Cross-pair | 2 years | ~1.5M | cTrader GetTrendbars |

All higher timeframes (M5, M15, H1, H4, D1) are **computed from M1** — not downloaded separately.

### Feature Categories

| Category | Features | Count | Status |
|---|---|---|---|
| Trend | SMA(20,50,200), EMA(12,26), MACD, MACD histogram | ~8 | Original |
| Momentum | RSI(14), Stochastic(14,3), Williams %R, CCI, ROC | ~8 | Original |
| Volatility | ATR(14), Bollinger Bands width, Keltner width, true range | ~6 | Original |
| Volume | Tick volume MA, volume ratio, volume spike detection | ~4 | Original |
| VWAP | VWAP deviation, VWAP slope, VWAP band position | ~5 | Added |
| Tick microstructure | Avg spread, tick velocity, uptick ratio, spread volatility | ~6 | Original |
| Time / session | Hour of day, day of week, session (London/NY/Asian), is_session_open | ~5 | Original |
| Multi-timeframe | H1 trend direction, H4 RSI, D1 SMA slope | ~10 | Original |
| Ichimoku | Cloud position, Tenkan/Kijun cross, Chikou span | ~5 | New |
| ADX / DI | ADX(14), +DI, -DI | ~3 | New |
| Fibonacci distance | Distance to 38.2%, 50%, 61.8% of last swing | ~3 | New |
| Candle patterns | Doji, engulfing, hammer, morning star (encoded as flags) | ~8 | New |
| Cross-pair correlation | GBPUSD/USDJPY/USDCHF return_5m, return_1h, corr_20 | ~8 | New |
| Pivot / fractal levels | Williams fractals, daily/weekly pivot points | ~4 | New |
| Order flow delta | Upticks − downticks per bar | ~3 | New |
| **Total** | | **~86** | |

### Cross-Pair Feature Detail

For each correlated pair, compute per M5 row:
```
gbpusd_return_5m    # GBPUSD % change over same 5 min
gbpusd_return_1h    # GBPUSD % change over same 1 hour
gbpusd_corr_20      # Rolling 20-bar correlation with EURUSD
usdjpy_return_5m
usdjpy_return_1h
usdjpy_corr_20
usdchf_return_5m
usdchf_corr_20
xauusd_return_1h    # Gold direction as risk signal
```

**Only M1 candles needed for cross-pairs** — no tick data required.

---

## Model 2 — Regime Detection (HMM)

- **Unsupervised** — no ground truth labels
- Discovers 4 hidden states: trending up, trending down, ranging, volatile
- Input: returns series, realized volatility, spread patterns from M1 data
- Output: current regime label + probability → used as **filter** for Model 1
- Rule: "Only take Model 1 signal when regime = trending OR breakout"
- Retrain: weekly (regime structure changes slowly)

---

## Model 3 — Chart Patterns (1D CNN)

- Input: raw OHLC sliding windows (50–200 bars)
- Requires **labeled training data** (examples of double top, H&S, channels)
- Challenge: labeling historical patterns is time-consuming
- Approach: use rule-based pattern detector to generate noisy labels, then train CNN to generalize
- Lower priority — add after Models 1+2 are working

---

## Model 4 — News & Economic Calendar (XGBoost)

- Data sources: Forex Factory (scraping), FRED API, Gemini/news sentiment
- Training labels: historical news event → price reaction (pips moved in next 1h)
- Key events: NFP, CPI, rate decisions, PMI
- **Critical rule**: suppress all signals 15 min before and 30 min after high-impact news
- Also provides: upcoming event countdown feature for Models 1 and 6
- Requires ongoing data collection before training

---

## Model 5 — Depth of Market / Order Flow (XGBoost)

- **cTrader limitation**: no real Level 2 order book via OpenAPI
- Approximated entirely from tick data:
  - Tick velocity (ticks per minute)
  - Uptick/downtick ratio per M5 bar
  - Spread behavior (widening = institutional activity, narrowing = calm)
  - Bid/ask imbalance patterns
  - Volume spike detection
- Already partially computed in the ML features table

---

## Model 6 — Ensemble Meta-Model (XGBoost)

### Input Features (from Models 1–5 + portfolio state)
```
model1_prob_up, model1_prob_flat, model1_prob_down
model2_regime (one-hot: trending/ranging/volatile/quiet)
model2_regime_confidence
model3_pattern_type, model3_pattern_confidence
model4_event_impact, model4_expected_direction
model5_order_flow_imbalance, model5_tick_velocity
portfolio_scalp_open (bool), portfolio_scalp_unrealized_pnl
portfolio_intraday_open (bool), portfolio_intraday_unrealized_pnl
current_drawdown_pct
```

### Output
```
action: Buy_Scalp / Sell_Scalp / Buy_Intraday / Sell_Intraday / Close / Hold
confidence: 0.0 – 1.0  (used for Kelly position sizing)
```

### Training (Out-of-Fold Stacking)
```
Full dataset split by time:
  [===== Train Models 1–5 =====][== Val: generate predictions ==][= Test =]
                                          ↓
                              Model 6 trains on Val predictions only
                                          ↓
                              Final evaluation on Test only (never seen)
```
This prevents Model 6 from learning to trust overfit predictions from training data.

---

## Position Sizing — Kelly Criterion

```
bet_fraction = (win_rate * avg_win - loss_rate * avg_loss) / avg_win

Practical Kelly: use fractional Kelly (25–50%) to reduce variance
  position_size = account_equity * (kelly_fraction * model6_confidence)
  max_position  = 2% of account per trade (hard cap)
```

---

## Reward / Learning Mechanism (No RL — Supervised + Daily Retraining)

The bot learns from wins and losses through **daily incremental retraining**:

```
Monday:   Model trained on 2yr history → makes trades
Tuesday:  Add Monday's closed trades to training data → retrain
Wednesday: Add Tuesday's trades → retrain
...
```

Each loss becomes a negative training example. Each win gets reinforced. The equity curve trends upward through:
1. Positive expected value per trade (signal accuracy > 55%)
2. Kelly compounding (position size grows with account equity)
3. Daily retraining (model stays current with market regime)

Loss aversion in the label definition: the **flat zone (label=0)** acts as an automatic filter — the model is only rewarded for predicting real moves, not noise.

---

## Training Pipeline (Python)

```
DuckDB (M1 candles + tick features)
    ↓ read via Python (polars/pandas)
Compute all 86 indicators (pandas-ta, ta-lib)
    ↓
Add target labels (forward-looking, no look-ahead bias)
    ↓
Train/val/test split  ← TIME-BASED ONLY (never random shuffle)
    ↓
XGBoost fit  (objective: multi:softprob, eval_metric: mlogloss)
    ↓
Walk-forward backtest  (expanding window, 30-day test windows)
    ↓
Export model.json → loaded by Rust at inference time
```

**Critical**: features at bar `t` use only data from bars `t-1` and earlier. Labels use `t+1` onward.

---

## Inference in Production (Rust)

```
Every M5 close:
  1. Read last N M1 bars from DuckDB
  2. Compute features in Rust (or call Python subprocess)
  3. Load model.json → XGBoost inference
  4. Get signal probabilities
  5. Apply regime filter (Model 2)
  6. Check news calendar (Model 4) — suppress if high-impact event near
  7. Model 6 decision → trade action
  8. Send order via cTrader OpenAPI
  9. Log trade for next retraining cycle
```

---

## Data Status

| Dataset | Status |
|---|---|
| EURUSD M1 candles | ✅ Downloaded |
| EURUSD ticks (bid + ask) | ✅ Downloaded |
| EURUSD merged ticks | ✅ Built |
| EURUSD ML features table | ✅ Built in DuckDB |
| GBPUSD M1 | ❌ Not yet downloaded |
| USDJPY M1 | ❌ Not yet downloaded |
| USDCHF M1 | ❌ Not yet downloaded |
| XAUUSD M1 | ❌ Not yet downloaded |
| News / calendar data | ❌ Not yet collected |

---

## Implementation Phases

### Phase 1 — Model 1 Baseline (Start Here)
- Train XGBoost on EURUSD-only technical features (~52 features, no cross-pairs)
- Walk-forward backtest: validate signal quality > 55% accuracy
- Establish baseline before adding complexity

### Phase 2 — Add Regime Filter (Model 2)
- Train HMM on EURUSD returns + volatility
- Use regime as filter: only trade when regime = trending or breakout
- Expected result: fewer trades, higher win rate

### Phase 3 — Add Scalp Model
- Train second XGBoost with shorter horizon (3–6 M5 bars, 5–8 pip threshold)
- Two models running simultaneously: scalp + intraday
- Independent SL/TP and position sizing per style

### Phase 4 — Cross-Pair Features (Models 1 enhancement)
- Download GBPUSD, USDJPY, USDCHF, XAUUSD M1 data
- Add 8–10 correlation features to Model 1
- Retrain and measure accuracy improvement

### Phase 5 — News Model (Model 4)
- Set up Forex Factory scraper + FRED API
- Build historical news → price reaction dataset
- Train Model 4, add news filter to execution rules

### Phase 6 — Kelly Position Sizing
- Implement fractional Kelly based on Model 6 confidence
- Position size grows with account equity (compounding)

### Phase 7 — Paper Trading
- Run full pipeline on live data, paper account
- Monitor signal quality, regime filter effectiveness, drawdown

### Phase 8 — Live Trading (small size)
- Start with minimum lot size
- Daily retraining activated

### Phase 9 — RL (Optional, only if Phase 8 is profitable)
- Add RL layer for execution timing optimization only
- Models 1–5 remain supervised (stable foundation)
- RL optimizes: when within the M5 bar to actually submit the order

---

## Key Decisions Made

| Decision | Choice | Reason |
|---|---|---|
| Primary approach | Supervised XGBoost (not RL) | More data-efficient, interpretable, stable |
| Label type | Ternary (+1/0/-1) | Flat zone filters noise, reduces false signals |
| Scalp horizon | 3–6 M5 bars | 15–30 min, 5–8 pip target |
| Intraday horizon | 12–24 M5 bars | 1–2 hour, 15–25 pip target |
| Decision clock | Every M5 close | Natural alignment with feature timeframe |
| Train/val split | Time-based only | Prevent data leakage |
| RL | Deferred to Phase 9 | Needs proven profitable base first |
| DoM | Tick-derived approx | cTrader has no real L2 order book |

---

## Execution Steps (Immediate Roadmap)

### Step 1 — Download Cross-Pair M1 Data (Now)
- Download GBPUSD, USDJPY, USDCHF, XAUUSD M1 candles via the app
- Already supported — just select each symbol and click "M1 Candles" in History BoT
- These correlation features are among the most predictive for EURUSD

### Step 2 — Python Training Script
- Read M1 candles + merged ticks from DuckDB
- Compute all technical indicators (pandas-ta / ta-lib)
- First pass: EURUSD only, ~52 features (no cross-pairs yet)
- Define **first-touch label**: +15 pip target / −10 pip stop, 2-hour max horizon
- Restrict to London + NY session hours only (08:00–17:00 UTC)

### Step 3 — Train XGBoost + Walk-Forward Validate
- Train on expanding window, test on rolling 3-month windows
- If accuracy > 50% on out-of-sample windows → continue
- Check feature importance → drop bottom 50% of features → retrain
- Target: 52–56% accuracy on filtered signals

### Step 4 — Add Layers
- Add cross-pair correlation features (GBPUSD, USDJPY, USDCHF, XAUUSD) → measure improvement
- Add HMM regime filter (Model 2) → measure improvement in filtered win rate
- Add news suppression (Forex Factory events) → measure drawdown reduction

### Step 5 — Paper Trading (2–3 months)
- Run full pipeline on live data, paper account
- Monitor: signal accuracy, regime filter effectiveness, drawdown, trade count
- This is the real validation — backtest results mean little without this step

### Step 6 — Go Live (Small Size)
- Start at minimum lot size (0.01 — $0.10/pip)
- Activate daily retraining
- Scale up only after 2+ months of profitable live results

---

## Notes

- **Look-ahead bias** is the #1 risk. Always verify: features at bar `t` use only `t-1` and earlier data.
- **News suppression** is mandatory. Never hold open positions through high-impact events.
- **Model 3 (CNN)** requires labeled pattern data — deferred until labeling strategy is decided.
- **Daily retraining** is what gives the bot adaptive learning — not RL.
- **Kelly sizing** is what makes the equity curve compound toward infinity — not RL.
- **No scalping** — intraday only (15 pip target / 10 pip stop). Spread costs make scalping unviable.
- **London + NY hours only** — Asian session signals are noise, avoid entirely.
