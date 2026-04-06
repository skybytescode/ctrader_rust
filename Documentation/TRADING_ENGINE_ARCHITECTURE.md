# Trading Engine Architecture

## Overview

The trading engine combines **historical knowledge** (trained ML models) with **real-time intelligence** (live market data) to make trading decisions on EUR/USD.

---

## Two Layers

### Layer 1: KNOWLEDGE (trained models - loaded at startup)

The "memory" of the trading bot. Learned patterns from historical data. Static during trading, retrained periodically.

| Model | What It Learned | Training Data | Period |
|-------|----------------|---------------|--------|
| **Model 1** — Technical Indicators (XGBoost) | "When RSI=62, MACD positive, London session → 54% chance +15p target hits before -10p stop" | 70 features from M1 candles + tick spread + cross-pairs | 2011-2026 (15 years) |
| **Model 2** — Regime Detection (HMM) | "Market is currently in: trending / ranging / volatile / quiet" | Price volatility, trend persistence from M1 | 2011-2026 |
| **Model 3** — Chart Patterns (CNN) | "This 120-bar chart shape has 58% long win probability" | Raw OHLCV windows from M1 | 2011-2026 |
| **Model 4** — Economic Calendar (XGBoost) | "NFP surprise of +0.3 → EUR/USD drops 8 pips avg in 5 min" | EC events + surprise values + price reactions | 2009-2026 |
| **Model 4b** — Unified EC+News+Price (XGBoost) | "EC surprise + bearish news + gold dropping + technical sell → 63% confidence SHORT" | 47 features: M5 candles, patterns, cross-pairs, EC, news sentiment | 2025-2026 (13 months) |
| **Model 5** — Depth of Market (XGBoost) | "Order book imbalance of +0.4 with high churn → buy pressure" | DoM features: OBI, spread, volume, walls, churn | ~1 week (NOT READY) |
| **Model 6** — Ensemble (XGBoost) | "When Model 1 says BUY but Model 3 says SELL in volatile regime → historically loses" | Outputs of Models 1-5 as features | NOT YET TRAINED |

### Layer 2: REAL-TIME INTELLIGENCE (live data - changes every second)

What's happening RIGHT NOW in the market. This is what actually drives the trading decision.

| Data Source | Update Frequency | What It Provides |
|-------------|-----------------|-------------------|
| **Live price tick** | Every tick (~30/sec) | Current bid/ask, spread, last price |
| **M1 candle close** | Every 1 minute | Latest completed candle for micro-structure analysis |
| **DoM (Depth of Market)** | Continuous (~31 events/sec) | Order book pressure, bid/ask walls, churn rate |
| **EC Calendar** | Scheduled around events | Next event time, last surprise, volatility rating |
| **News feed** | Every 10 minutes | Recent article count, AI sentiment scores, categories |
| **Cross-pairs** | Every tick | GBPUSD, USDJPY, USDCHF, XAUUSD current prices |

---

## Decision Flow

```
STEP 1: REAL-TIME DATA COLLECTION (continuous)
│
├── Live price tick → current bid 1.0850, ask 1.0851, spread 0.1 pip
├── M1 candle just closed → bullish, +0.3 pips body
├── DoM snapshot → OBI = +0.5, heavy buying, no walls
├── EC calendar → NFP released 20 min ago, surprise = +0.3 (USD positive)
├── News feed → 3 articles in last hour, avg sentiment = -0.6 (EUR bearish)
├── Cross-pairs → XAUUSD dropping (risk-on), USDJPY rising (USD strength)
│
        ↓
│
STEP 2: REAL-TIME ANALYSIS ENGINE (every M1 close)
│
├── Multi-timeframe structure:
│   ├── M1 inside M5: "3 bullish bars of 5, momentum strong"
│   ├── M5 inside H1: "higher lows forming, ascending structure"
│   ├── H1 inside H4: "at support level, hammer forming"
│   └── Assessment: "All timeframes aligned bullish"
│
├── Anomaly detection:
│   ├── Spread: 0.1 pip = normal → OK
│   ├── DoM: no sudden wall changes → OK
│   ├── Volume: 1.3× average → slightly elevated, OK
│   └── Assessment: "No anomalies detected"
│
├── Event proximity:
│   ├── NFP released 20 min ago → "Post-event, initial move settled"
│   ├── Next event: FOMC in 6 hours → "Safe window to trade"
│   └── Assessment: "Clear to trade"
│
├── News momentum:
│   ├── 3 articles, avg impact = -0.6 → "Bearish EUR sentiment"
│   ├── Category: geopolitical (strongest signal from decay analysis)
│   └── Assessment: "News supports SHORT EUR/USD"
│
        ↓
│
STEP 3: MODEL CONSULTATION (every M5 close, or on significant events)
│
│   The models are asked: "Given the current technical state, what's the probability?"
│
├── Model 1 (Technical):  "P(short win) = 0.59" (RSI, MACD, EMAs all bearish)
├── Model 2 (Regime):     "Trending regime" (allows trend-following trades)
├── Model 3 (CNN):        "P(short win) = 0.62" (sees breakdown pattern)
├── Model 4 (EC):         "Post-NFP USD strength signal"
├── Model 4b (Unified):   "SHORT confidence = 0.61" (EC + News + Price aligned)
│
│   Agreement: 4 out of 4 directional models say SHORT
│   Regime: trending (Model 2 allows trend trades)
│
        ↓
│
STEP 4: RISK CHECK (before every trade)
│
├── Already in a position? → NO (can enter)
├── Daily loss limit hit (3%)? → NO (can enter)
├── Spread normal? → YES (0.1 pip, OK)
├── Enough margin? → YES
├── Session hours? → YES (London session)
├── Max trades today reached? → NO
│
        ↓
│
STEP 5: EXECUTE
│
│   DECISION: OPEN SHORT EUR/USD
│   Position size: calculated from account balance, 1% risk
│   Stop loss: +10 pips above entry (based on Model 1's trained R:R)
│   Take profit: -15 pips below entry
│   Max hold time: 2 hours
│   
│   → Send ProtoOaNewOrderReq to cTrader API
│
        ↓
│
STEP 6: POSITION MANAGEMENT (continuous while position is open)
│
├── Every tick:
│   ├── Check if TP or SL hit → close automatically (broker-side)
│   ├── Check spread → if spikes > 3× normal → emergency close
│   ├── Check DoM → if OBI reverses strongly → tighten stop
│
├── Every M1 close:
│   ├── Update trailing stop if in profit
│   ├── Check if model signals reversed → early close
│   ├── Check micro-structure → momentum dying? → tighten stop
│
├── Every M5 close:
│   ├── Re-run models → still SHORT? → hold
│   ├── Model now says LONG? → close position
│
├── Time check:
│   ├── 2 hours reached? → close at market
│   ├── High-vol EC event approaching? → tighten stop
│
└── Result: CLOSED at -15 pips (take profit hit) → WIN
```

---

## Real-Time Components to Build

| Component | Purpose | Update Frequency | Priority |
|-----------|---------|-----------------|----------|
| **Real-time analyzer** | M1 micro-structure, spread monitoring, DoM pressure analysis | Every tick / every second | HIGH |
| **Multi-timeframe engine** | M1→M5→H1→H4 structure, forming candle patterns, internal momentum | Every M1 close | HIGH |
| **Signal combiner** | Collects real-time analysis + all model outputs into one decision | Every M1 close | HIGH |
| **Event manager** | EC proximity blocker (don't trade 15 min before high-vol events), news cluster detection | Continuous | HIGH |
| **Position manager** | Trailing stops, detect reversals, time-based exits, spread spike protection | Every tick | HIGH |
| **Risk manager** | Position sizing (1% account risk), daily loss limit (3%), margin check | Before each trade | HIGH |
| **Execution engine** | Send orders to cTrader OpenAPI (open/close/amend positions) | On decision | HIGH |
| **Trade journal** | Log every signal, decision, entry, exit, P&L to DuckDB | On every event | MEDIUM |
| **Paper trading mode** | Run everything but log instead of executing real trades | Always first | CRITICAL |

---

## Model Output Interpretation

Each model produces a different type of output. Here's how they translate to a trading signal:

### Model 1 — Technical Indicators
```
Input:  70 features from last 200 M1 candles
Output: P(long_win) = 0.62, P(short_win) = 0.41
Meaning: "62% chance a LONG trade with +15p target / -10p stop wins within 2 hours"
Signal:  LONG if P(long_win) ≥ 0.55, SHORT if P(short_win) ≥ 0.55
```

### Model 2 — Regime Detection
```
Input:  Price volatility and trend metrics
Output: Regime state (0=quiet, 1=trending, 2=ranging, 3=volatile)
Meaning: "Market is currently in a trending state"
Signal:  FILTER — allows/blocks certain trade types
         Trending → allow trend-following (Models 1, 4b)
         Ranging  → allow mean-reversion, CNN patterns (Model 3)
         Volatile → reduce size or block trading
         Quiet    → reduce size, tighter stops
```

### Model 3 — Chart Patterns CNN
```
Input:  Raw OHLCV window (120 M1 bars = 2 hours of data)
Output: P(long_win) = 0.71, P(short_win) = 0.29
Meaning: "The visual chart pattern looks like a bullish setup"
Signal:  LONG if P(long_win) ≥ 0.55, SHORT if P(short_win) ≥ 0.55
```

### Model 4 — Economic Calendar
```
Input:  EC event type, surprise value, volatility rating, time since release
Output: Direction signal + blocker
Meaning: "NFP surprised positive for USD → EUR/USD should drop"
Signal:  DIRECTIONAL after events, BLOCKER before events
```

### Model 4b — Unified (EC + News + Price)
```
Input:  47 features (M5 candles, patterns, cross-pairs, EC, news sentiment)
Output: P(up) = 0.39, confidence = 0.61 (toward SHORT)
Meaning: "All data combined points to EUR/USD going down"
Signal:  LONG if confidence ≥ 0.58 toward up, SHORT if ≥ 0.58 toward down
Note:    At 0.58 confidence → ~63% accuracy (from backtest)
         At 0.60 confidence → ~67% accuracy (fewer trades)
```

---

## Risk Management Rules

| Rule | Value | Rationale |
|------|-------|-----------|
| Max position size | 1% of account equity at risk | Survive 10+ consecutive losses |
| Stop loss | 10 pips (Model 1 trained R:R) | Based on trained model parameters |
| Take profit | 15 pips (1.5:1 R:R) | Positive expected value at >40% win rate |
| Max hold time | 2 hours (120 M1 bars) | Model trained on this horizon |
| Max daily loss | 3% of account | Circuit breaker, stop trading for the day |
| Max open positions | 1 | No correlation risk, simplicity |
| Spread limit | Don't trade if spread > 2× average | Avoid illiquid periods |
| Session filter | London 08:00-12:00, New York 13:00-17:00 UTC | Most liquid, models trained on these |
| EC event buffer | Don't enter 15 min before high-vol EC event | Extreme volatility, unpredictable |
| Min model agreement | At least 2 directional models agree | Avoid conflicting signals |
| Min confidence | Model 4b ≥ 0.58 OR Model 1 ≥ 0.55 | Only high-conviction trades |

---

## Expected Performance (Realistic)

### Conservative (2 trades/day, high confidence only)
```
Win rate (adjusted for spread + slippage): ~58-60%
Average net per trade: +3.5 pips
Trades per month: ~44
Monthly pips: ~+156
Monthly return: ~1.5-2.5%
```

### Key Success Factors
1. Paper trade for 2-4 weeks minimum before going live
2. Retrain models monthly (market evolves)
3. Never override the risk manager
4. Monitor model decay (track accuracy weekly)
5. Start with minimum position size

---

## Implementation Order

| Phase | What | Description |
|-------|------|-------------|
| **Phase 1** | Paper Trading Engine | Build the full decision pipeline but LOG trades instead of executing. Run for 2+ weeks. |
| **Phase 2** | Trade Journal & Review | DuckDB table for all signals/decisions/outcomes. UI to review past trades. |
| **Phase 3** | Live Execution | Connect to cTrader order API. Start with minimum size ($1,000 positions). |
| **Phase 4** | Position Management | Trailing stops, partial exits, time-based closes. |
| **Phase 5** | Model 6 Ensemble | Train on 2+ weeks of paper trading data (all model outputs + actual outcomes). |
| **Phase 6** | Scale Up | Increase position size based on proven track record. |

---

## Data Sources Summary

### Already Available (no code changes needed)
- Live EURUSD bid/ask (cTrader price feed)
- M1 candles (constructed from price feed)
- DoM depth events (cTrader ProtoOaDepthEvent)
- EC Calendar events (FXStreet via econcal proxy)
- News articles + AI sentiment (FXStreet + Ollama)
- Cross-pair prices (cTrader price feed for GBPUSD, USDJPY, etc.)
- 5 trained ML models (XGBoost JSON, HMM params, CNN weights)

### Needs Building
- Real-time feature computation (rolling buffers, no DB reads)
- XGBoost inference in Rust (load model, ~1ms per prediction)
- CNN inference in Rust (or call Python, ~10ms per prediction)
- cTrader order management (ProtoOaNewOrderReq, ProtoOaClosePositionReq)
- Trade state machine (IDLE → POSITION_OPEN → managing → CLOSED)
- Position sizing calculator
- Trade journal (DuckDB table + UI display)


