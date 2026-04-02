-- Unified ML feature table: M5 candles + patterns + EC calendar + news sentiment + cross-pairs
-- All data aligned from 2025-02-19 (earliest news) onwards

DROP TABLE IF EXISTS ml_unified_features;
CREATE TABLE ml_unified_features AS
WITH
-- M5 with candle features and lagged context
m5_features AS (
    SELECT
        m.timestamp,
        m.open, m.high, m.low, m.close, m.volume,
        -- Body and wick ratios
        (m.close - m.open) * 10000 AS body_pips,
        ABS(m.close - m.open) / NULLIF(m.high - m.low, 0) AS body_ratio,
        (m.high - GREATEST(m.open, m.close)) / NULLIF(m.high - m.low, 0) AS upper_wick_ratio,
        (LEAST(m.open, m.close) - m.low) / NULLIF(m.high - m.low, 0) AS lower_wick_ratio,
        (m.high - m.low) * 10000 AS range_pips,
        -- Direction
        CASE WHEN m.close > m.open THEN 1 WHEN m.close < m.open THEN -1 ELSE 0 END AS direction,
        -- Candlestick patterns
        CASE WHEN (m.high - m.low) > 3 * ABS(m.close - m.open)
             AND (LEAST(m.open, m.close) - m.low) > 0.6 * (m.high - m.low)
             THEN 1 ELSE 0 END AS is_hammer,
        CASE WHEN (m.high - m.low) > 3 * ABS(m.close - m.open)
             AND (m.high - GREATEST(m.open, m.close)) > 0.6 * (m.high - m.low)
             THEN 1 ELSE 0 END AS is_shooting_star,
        CASE WHEN ABS(m.close - m.open) < 0.1 * (m.high - m.low)
             AND (m.high - m.low) * 10000 > 2
             THEN 1 ELSE 0 END AS is_doji,
        CASE WHEN ABS(m.close - m.open) / NULLIF(m.high - m.low, 0) > 0.8
             THEN 1 ELSE 0 END AS is_marubozu,
        -- Moving averages
        AVG(m.close) OVER (ORDER BY m.timestamp ROWS BETWEEN 4 PRECEDING AND CURRENT ROW) AS sma_5,
        AVG(m.close) OVER (ORDER BY m.timestamp ROWS BETWEEN 12 PRECEDING AND CURRENT ROW) AS sma_12,
        AVG(m.close) OVER (ORDER BY m.timestamp ROWS BETWEEN 24 PRECEDING AND CURRENT ROW) AS sma_24,
        -- ATR-like volatility
        AVG((m.high - m.low) * 10000) OVER (ORDER BY m.timestamp ROWS BETWEEN 12 PRECEDING AND CURRENT ROW) AS atr_12,
        -- Momentum
        m.close - LAG(m.close, 12) OVER (ORDER BY m.timestamp) AS momentum_12,
        -- Previous bar direction
        LAG(CASE WHEN m.close > m.open THEN 1 WHEN m.close < m.open THEN -1 ELSE 0 END)
            OVER (ORDER BY m.timestamp) AS prev_direction,
        -- 3-bar trend
        (LAG(CASE WHEN m.close > m.open THEN 1 WHEN m.close < m.open THEN -1 ELSE 0 END, 1) OVER (ORDER BY m.timestamp)
        + LAG(CASE WHEN m.close > m.open THEN 1 WHEN m.close < m.open THEN -1 ELSE 0 END, 2) OVER (ORDER BY m.timestamp)
        + LAG(CASE WHEN m.close > m.open THEN 1 WHEN m.close < m.open THEN -1 ELSE 0 END, 3) OVER (ORDER BY m.timestamp)
        ) AS trend_3bar,
        -- Engulfing pattern
        CASE WHEN m.close > m.open
             AND LAG(m.close) OVER (ORDER BY m.timestamp) < LAG(m.open) OVER (ORDER BY m.timestamp)
             AND m.close > LAG(m.open) OVER (ORDER BY m.timestamp)
             AND m.open < LAG(m.close) OVER (ORDER BY m.timestamp)
             THEN 1
             WHEN m.close < m.open
             AND LAG(m.close) OVER (ORDER BY m.timestamp) > LAG(m.open) OVER (ORDER BY m.timestamp)
             AND m.close < LAG(m.open) OVER (ORDER BY m.timestamp)
             AND m.open > LAG(m.close) OVER (ORDER BY m.timestamp)
             THEN -1
             ELSE 0 END AS engulfing,
        -- TARGET
        LEAD(m.close) OVER (ORDER BY m.timestamp) AS next_close,
        (LEAD(m.close) OVER (ORDER BY m.timestamp) - m.close) * 10000 AS target_move_pips,
        CASE WHEN LEAD(m.close) OVER (ORDER BY m.timestamp) > m.close THEN 1
             WHEN LEAD(m.close) OVER (ORDER BY m.timestamp) < m.close THEN -1
             ELSE 0 END AS target_direction
    FROM eurusd_m5 m
),
-- EC calendar features per M5 bar
ec_features AS (
    SELECT
        m.timestamp,
        MIN(CASE WHEN e.event_ts >= m.timestamp THEN e.event_ts - m.timestamp END) AS secs_to_next_ec,
        MIN(CASE WHEN e.event_ts <= m.timestamp THEN m.timestamp - e.event_ts END) AS secs_since_last_ec,
        LAST(CASE WHEN e.event_ts <= m.timestamp AND m.timestamp - e.event_ts <= 3600 THEN e.surprise END
             ORDER BY e.event_ts) AS last_ec_surprise_1h,
        LAST(CASE WHEN e.event_ts <= m.timestamp AND m.timestamp - e.event_ts <= 3600 THEN e.volatility END
             ORDER BY e.event_ts) AS last_ec_vol_1h,
        MAX(CASE WHEN e.event_ts >= m.timestamp AND e.event_ts - m.timestamp <= 1800 AND e.volatility = 3 THEN 1 ELSE 0 END) AS high_vol_ec_30m,
        SUM(CASE WHEN e.event_ts <= m.timestamp AND m.timestamp - e.event_ts <= 3600 THEN 1 ELSE 0 END) AS ec_count_1h,
        LAST(CASE WHEN e.event_ts <= m.timestamp AND m.timestamp - e.event_ts <= 3600 THEN e.beats_forecast END
             ORDER BY e.event_ts) AS last_ec_beats_1h
    FROM eurusd_m5 m
    LEFT JOIN ec_events_ts e ON e.event_ts BETWEEN m.timestamp - 86400 AND m.timestamp + 86400
    GROUP BY m.timestamp
),
-- News sentiment features per M5 bar
news_features AS (
    SELECT
        m.timestamp,
        SUM(CASE WHEN n.pub_ts >= m.timestamp - 300 AND n.pub_ts < m.timestamp THEN 1 ELSE 0 END) AS news_count_5m,
        AVG(CASE WHEN n.pub_ts >= m.timestamp - 300 AND n.pub_ts < m.timestamp THEN n.eurusd_impact END) AS news_impact_5m,
        SUM(CASE WHEN n.pub_ts >= m.timestamp - 3600 AND n.pub_ts < m.timestamp THEN 1 ELSE 0 END) AS news_count_1h,
        AVG(CASE WHEN n.pub_ts >= m.timestamp - 3600 AND n.pub_ts < m.timestamp THEN n.eurusd_impact END) AS news_impact_1h,
        MAX(CASE WHEN n.pub_ts >= m.timestamp - 3600 AND n.pub_ts < m.timestamp THEN n.vol_expected END) AS news_max_vol_1h,
        AVG(CASE WHEN n.pub_ts >= m.timestamp - 3600 AND n.pub_ts < m.timestamp THEN n.relevance END) AS news_avg_relevance_1h,
        SUM(CASE WHEN n.pub_ts >= m.timestamp - 14400 AND n.pub_ts < m.timestamp THEN 1 ELSE 0 END) AS news_count_4h,
        AVG(CASE WHEN n.pub_ts >= m.timestamp - 14400 AND n.pub_ts < m.timestamp THEN n.eurusd_impact END) AS news_impact_4h,
        AVG(CASE WHEN n.pub_ts >= m.timestamp - 3600 AND n.pub_ts < m.timestamp AND n.category = 'geopolitical' THEN n.eurusd_impact END) AS geo_impact_1h,
        AVG(CASE WHEN n.pub_ts >= m.timestamp - 3600 AND n.pub_ts < m.timestamp AND n.category = 'central_bank' THEN n.eurusd_impact END) AS cb_impact_1h
    FROM eurusd_m5 m
    LEFT JOIN news_ts n ON n.pub_ts BETWEEN m.timestamp - 14400 AND m.timestamp
    GROUP BY m.timestamp
)
-- FINAL JOIN
SELECT
    f.timestamp,
    f.open, f.high, f.low, f.close, f.volume,
    f.body_pips, f.body_ratio, f.upper_wick_ratio, f.lower_wick_ratio, f.range_pips,
    f.direction, f.prev_direction, f.trend_3bar,
    f.is_hammer, f.is_shooting_star, f.is_doji, f.is_marubozu, f.engulfing,
    (f.close - f.sma_5) * 10000 AS dist_sma5_pips,
    (f.close - f.sma_12) * 10000 AS dist_sma12_pips,
    (f.close - f.sma_24) * 10000 AS dist_sma24_pips,
    f.atr_12,
    f.momentum_12 * 10000 AS momentum_12_pips,
    -- Cross-pair moves
    (c.gbpusd_close - LAG(c.gbpusd_close) OVER (ORDER BY f.timestamp)) * 10000 AS gbpusd_move,
    (c.usdjpy_close - LAG(c.usdjpy_close) OVER (ORDER BY f.timestamp)) * 100 AS usdjpy_move,
    (c.usdchf_close - LAG(c.usdchf_close) OVER (ORDER BY f.timestamp)) * 10000 AS usdchf_move,
    (c.xauusd_close - LAG(c.xauusd_close) OVER (ORDER BY f.timestamp)) * 10 AS xauusd_move,
    -- EC Calendar
    LEAST(COALESCE(ec.secs_to_next_ec, 86400), 86400) AS secs_to_next_ec,
    LEAST(COALESCE(ec.secs_since_last_ec, 86400), 86400) AS secs_since_last_ec,
    COALESCE(ec.last_ec_surprise_1h, 0) AS last_ec_surprise_1h,
    COALESCE(ec.last_ec_vol_1h, 0) AS last_ec_vol_1h,
    COALESCE(ec.high_vol_ec_30m, 0) AS high_vol_ec_30m,
    COALESCE(ec.ec_count_1h, 0) AS ec_count_1h,
    COALESCE(ec.last_ec_beats_1h, 0) AS last_ec_beats_1h,
    -- News sentiment
    COALESCE(nf.news_count_5m, 0) AS news_count_5m,
    COALESCE(nf.news_impact_5m, 0) AS news_impact_5m,
    COALESCE(nf.news_count_1h, 0) AS news_count_1h,
    COALESCE(nf.news_impact_1h, 0) AS news_impact_1h,
    COALESCE(nf.news_max_vol_1h, 0) AS news_max_vol_1h,
    COALESCE(nf.news_avg_relevance_1h, 0) AS news_avg_relevance_1h,
    COALESCE(nf.news_count_4h, 0) AS news_count_4h,
    COALESCE(nf.news_impact_4h, 0) AS news_impact_4h,
    COALESCE(nf.geo_impact_1h, 0) AS geo_news_impact_1h,
    COALESCE(nf.cb_impact_1h, 0) AS cb_news_impact_1h,
    -- TARGET
    f.target_move_pips,
    f.target_direction
FROM m5_features f
LEFT JOIN cross_m5 c ON c.timestamp = f.timestamp
LEFT JOIN ec_features ec ON ec.timestamp = f.timestamp
LEFT JOIN news_features nf ON nf.timestamp = f.timestamp
WHERE f.target_move_pips IS NOT NULL
  AND f.momentum_12 IS NOT NULL
ORDER BY f.timestamp;
