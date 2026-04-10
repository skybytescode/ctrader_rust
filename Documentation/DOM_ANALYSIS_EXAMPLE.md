# DoM Analysis: Raw Data → Claude Prompt

This document shows the full pipeline from raw cTrader DoM data to what Claude CLI receives
for pattern analysis, using a real snapshot from 2026-04-08.

---

## 1. RAW DATA FROM cTRADER API (ProtoOaDepthEvent)

Each quote arrives as: `id`, `bid` (optional), `ask` (optional), `size`

- If `bid` is set → ask side (sellers, prices ABOVE market)
- If `ask` is set → bid side (buyers, prices BELOW market)
- Note: cTrader field naming is inverted from standard convention

```
RAW: id=1142291675 bid=None   ask=Some(116201) size=20000000   → BID side, price=1.16201
RAW: id=1142289813 bid=Some(115816) ask=None  size=20000000   → ASK side, price=1.15816
...
```

### Full Book Snapshot (85 entries)

**BID side (43 levels) — buyers, below/around market:**
```
Price       Size
1.16201     20,000,000
1.16190     10,000,000
1.16185      5,000,000
1.16183      3,000,000
1.16179      1,000,000
1.16178        500,000
1.16177        100,000
          ─── gap ~10 pips ───
1.16082     30,000,000
1.16061      5,000,000
1.16057      3,000,000
1.16049      1,000,000
1.16048        500,000
1.16046        100,000
1.16045     20,000,000
1.16042     10,000,000
1.16039      5,000,000
1.16037      3,000,000
1.16034      1,500,000
1.16032        100,000
1.16029     20,000,000
1.16025     10,000,000
1.16022      5,000,000
1.16020      3,000,000
1.16016      1,500,000
1.16014        100,000
1.16004     20,000,000
1.16001     10,000,000
1.16000      5,000,000
1.15999      3,000,000
1.15997     20,000,000
1.15996      1,500,000
1.15994        100,000
1.15990     10,000,000
1.15986      5,000,000
1.15984      3,000,000
1.15979      1,500,000
1.15977        100,000
          ─── gap ~14 pips ───
1.15837     20,000,000
1.15834     10,000,000
1.15832      5,000,000
1.15831      3,000,000
1.15828      1,500,000
1.15826        100,000
```

**ASK side (42 levels) — sellers, above/around market:**
```
Price       Size
1.15816     20,000,000
1.15818     10,000,000
1.15819      5,000,000
1.15820      3,000,000
1.15823      1,500,000
1.15825        100,000
          ─── gap ~11 pips ───
1.15932     30,000,000
1.15950      5,000,000
1.15959      3,000,000
1.15962     20,000,000
1.15968     10,000,000
1.15970      5,000,000
1.15971      3,000,000
1.15974      1,500,000
1.15974      1,000,000
1.15976        500,000
1.15976        100,000
1.15980        100,000
1.15982     20,000,000
1.15985     10,000,000
1.15987      5,000,000
1.15989      3,000,000
1.15992      1,500,000
1.15994        100,000
1.16002     20,000,000
1.16005     10,000,000
1.16007      5,000,000
1.16008      3,000,000
1.16011      1,500,000
1.16013        100,000
1.16017     20,000,000
1.16020     10,000,000
1.16024      5,000,000
1.16025      3,000,000
1.16029      1,500,000
1.16031        100,000
          ─── gap ~12 pips ───
1.16156     20,000,000
1.16167     10,000,000
1.16169      5,000,000
1.16170      3,000,000
1.16173      1,500,000
1.16175        100,000
```

### Key Observations from Raw Data
- **Total bid volume = 277,200,000** — **Total ask volume = 277,200,000** (exactly equal = synthetic/mirrored)
- Volumes follow repeating tiers: 20M, 10M, 5M, 3M, 1.5M, 100k (broker-generated pattern)
- Price ranges overlap: bids extend from 1.16201 to 1.15826, asks from 1.15816 to 1.16175
- **OBI is always 0.000** — useless for volume imbalance analysis

---

## 2. OUR ANALYSIS (computed in real-time, ~1/sec)

From the raw book, we extract **price level structure**:

### Level Counts & Trend
```
Bid levels: 43
Ask levels: 42
Levels trend: +0 (stable)
Levels dropping: false
```

### Density Near Price (levels per pip within 10 pips of best bid/ask)
```
Bid density near price: ~0.7/pip  (7 levels within 10 pips of best bid 1.16201)
Ask density near price: ~0.6/pip  (6 levels within 10 pips of best ask 1.15816)
Density imbalance: +0.1 (slightly more bid support)
```

### Clusters (groups of levels within 3 pips of each other)
```
Bid clusters:
  1. 1.16189 (7 levels) — top cluster near 1.16201
  2. 1.16063 (6 levels) — cluster around 1.16050
  3. 1.16040 (6 levels) — cluster around 1.16035
  4. 1.16022 (6 levels) — cluster around 1.16020
  5. 1.16000 (7 levels) — cluster around 1.16000 (round number!)
  6. 1.15985 (5 levels) — cluster around 1.15985
  7. 1.15832 (6 levels) — bottom cluster

Ask clusters:
  1. 1.15819 (6 levels) — bottom cluster near 1.15816
  2. 1.15952 (11 levels) — large cluster around 1.15960
  3. 1.15986 (6 levels) — cluster around 1.15985
  4. 1.16007 (6 levels) — cluster around 1.16005
  5. 1.16023 (6 levels) — cluster around 1.16020
  6. 1.16167 (6 levels) — top cluster
```

### Gaps (empty zones > 5 pips between clusters)
```
Bid gaps:
  1. 1.15977 → 1.16082  (gap = ~10 pips)  — price could drop fast through here
  2. 1.15837 → 1.15977  (gap = ~14 pips)  — large void below

Ask gaps:
  1. 1.15825 → 1.15932  (gap = ~11 pips)  — price could rally fast through here
  2. 1.16031 → 1.16156  (gap = ~12 pips)  — void above
```

---

## 3. WHAT CLAUDE CLI RECEIVES (the full prompt)

The DoM section is sent as part of a larger prompt that includes candlestick data
for all timeframes. Here is the **complete prompt structure**:

```
You are an expert EUR/USD forex trader. Analyze the raw OHLCV candlestick data
and Depth of Market data below across all timeframes. Identify patterns, trend
structure, support/resistance levels, order flow, and entry opportunities.

Time: 14:32 UTC. Your output will be used as input for a trading decision system.

M15:
  FORMING (73% complete): O=1.10040 H=1.10080 L=1.10030 C=1.10050 V=800
  COMPLETED BARS (oldest→newest):
    1: O=1.09990 H=1.10020 L=1.09980 C=1.10010 V=950
    ... (20 bars)

M5:
  FORMING (60% complete): O=1.10045 H=1.10060 L=1.10040 C=1.10050 V=280
  COMPLETED BARS (oldest→newest):
    1: O=1.10030 H=1.10045 L=1.10025 C=1.10040 V=310
    ... (20 bars)
  M1 BARS INSIDE FORMING M5:
    m1: O=1.10045 H=1.10055 L=1.10040 C=1.10050
    m2: O=1.10050 H=1.10060 L=1.10048 C=1.10055
    m3: O=1.10055 H=1.10058 L=1.10042 C=1.10050

M1 (last 20 bars):
    1: O=1.09990 H=1.10000 L=1.09985 C=1.09995
    2: O=1.09995 H=1.10005 L=1.09990 C=1.10000
    ... (20 bars)

DOM (price level structure):
  Levels: 43bid / 42ask (trend=+0)
  Density near price: bid=0.7/pip ask=0.6/pip imbalance=+0.1
  Bid clusters: 1.16189(7lvl), 1.16063(6lvl), 1.16040(6lvl), 1.16022(6lvl), 1.16000(7lvl)
  Ask clusters: 1.15819(6lvl), 1.15952(11lvl), 1.15986(6lvl), 1.16007(6lvl), 1.16023(6lvl)
  Bid gaps: 1.15977-1.16082(10p), 1.15837-1.15977(14p)
  Ask gaps: 1.15825-1.15932(11p), 1.16031-1.16156(12p)

Based on the raw OHLCV data, analyze independently. Output ONLY valid JSON:

{
  "m15_bias": "<bullish/bearish/neutral>",
  "m15_pattern": "<main pattern you identify, or 'none'>",
  "m5_bias": "<bullish/bearish/neutral>",
  "m5_pattern": "<main pattern you identify, or 'none'>",
  "timeframe_conflict": true/false,
  "conflict_detail": "<which timeframes disagree, or 'none'>",
  "dominant_bias": "<bullish/bearish/neutral>",
  "dominant_bias_confidence": 0.0-1.0,
  "session_quality": "<good/moderate/poor>",
  "forming_candle_signal": "<what the forming candles suggest, or 'none'>",
  "m1_entry_ready": true/false,
  "recommended_action": "<enter_long/enter_short/wait/no_trade>",
  "entry_timeframe": "<H1/M15/M5/none>",
  "entry_condition": "<specific condition to enter, or why not>",
  "key_resistance": price_level,
  "key_support": price_level,
  "target_pips": number,
  "stop_pips": number,
  "invalidation": "<what cancels this assessment>"
}
```

---

## 4. SCORING IMPACT (how DoM affects pattern score)

The DoM price level analysis contributes **-2 to +2** to the total pattern score:

| Condition | Score | Example |
|---|---|---|
| Density imbalance confirms pattern direction | +1 | Bullish pattern + more bid levels near price |
| Density imbalance contradicts pattern direction | -1 | Bullish pattern + more ask levels near price |
| Gap in trade direction > 10 pips | +1 | Going long + big gap above (room to run) |
| Levels dropping > 20% from average | -1 | Thinning book = low liquidity, risky |

Score >= 7 triggers the Claude CLI call (with 30s cooldown).

---

## 5. WHAT WE DON'T USE (and why)

| Metric | Why Unusable |
|---|---|
| OBI (Order Book Imbalance) | Always 0.000 — retail cTrader mirrors volumes exactly on both sides |
| Volume imbalance | Same reason — bid_vol always equals ask_vol |
| Wall detection | Walls are synthetic (broker preset tiers: 20M, 10M, 5M, 3M, 1.5M, 100k) |
| Bid/Ask pulling | Volume drops are symmetric, no real order flow signal |

These would work with ECN/STP accounts or CME futures data, but not with retail cTrader synthetic DoM.
