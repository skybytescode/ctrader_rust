//! Real-time multi-timeframe candlestick pattern detection engine.
//!
//! Uses `candlestick-rs` for pattern recognition on completed and forming candles
//! across M5 and M15 timeframes simultaneously.
//!
//! M1 candles are the raw input — they build forming candles on higher timeframes
//! and provide entry timing signals. Pattern detection runs on M5 and above.

use candlestick_rs::{CandleStick, CandleStream};

// ── DoM real-time analytics ───────────────────────────────────────────────────

/// A price level cluster in the DoM — a group of nearby orders.
#[derive(Debug, Clone)]
pub struct DomCluster {
    pub center_price: f64,      // volume-weighted center of the cluster
    pub total_size: i64,        // sum of all order sizes in cluster
    pub level_count: u32,       // number of individual levels in cluster
    pub price_min: f64,         // lowest price in cluster
    pub price_max: f64,         // highest price in cluster
}

/// A gap between adjacent clusters — empty zone in the book.
#[derive(Debug, Clone)]
pub struct DomGap {
    pub price_from: f64,        // lower edge of gap
    pub price_to: f64,          // upper edge of gap
    pub gap_pips: f64,          // gap width in pips
}

/// Live Depth of Market analytics for trading decisions.
/// Focused on price level structure (clusters, gaps, density) rather than
/// volume imbalance (OBI), since retail cTrader DoM has mirrored volumes.
#[derive(Debug, Clone)]
pub struct DomSnapshot {
    // ── Current state ──
    pub bid_levels: u32,
    pub ask_levels: u32,
    pub best_bid: f64,          // highest bid price (below market)
    pub best_ask: f64,          // lowest ask price (above market)
    pub book_spread_pips: f64,  // DoM book depth range (not trading spread)
    pub active: bool,

    // ── Level count tracking ──
    level_history: Vec<u32>,           // last 60 total level counts
    pub levels_trend: i32,             // level count change (negative = thinning)
    pub levels_dropping: bool,         // levels dropped >20% from average

    // ── Price level clusters ──
    pub bid_clusters: Vec<DomCluster>, // clusters on bid side (sorted by price desc)
    pub ask_clusters: Vec<DomCluster>, // clusters on ask side (sorted by price asc)

    // ── Gaps (empty zones) ──
    pub bid_gaps: Vec<DomGap>,         // gaps on bid side (sorted by size desc)
    pub ask_gaps: Vec<DomGap>,         // gaps on ask side (sorted by size desc)

    // ── Density (how tightly packed levels are near current price) ──
    pub bid_density_near: f64,         // levels per pip in closest 10 pips on bid side
    pub ask_density_near: f64,         // levels per pip in closest 10 pips on ask side
    pub density_imbalance: f64,        // bid_density - ask_density (positive = more bid support)

    // ── Churn tracking (event rate) ──
    churn_history: Vec<u32>,           // events per update cycle (last 60)
    pub churn_rate: f64,               // average events per cycle
    pub churn_spike: bool,             // current churn > 2x average

    update_count: u64,
}

impl Default for DomSnapshot {
    fn default() -> Self {
        Self {
            bid_levels: 0, ask_levels: 0,
            best_bid: 0.0, best_ask: 0.0,
            book_spread_pips: 0.0, active: false,
            level_history: Vec::with_capacity(62),
            levels_trend: 0, levels_dropping: false,
            bid_clusters: Vec::new(), ask_clusters: Vec::new(),
            bid_gaps: Vec::new(), ask_gaps: Vec::new(),
            bid_density_near: 0.0, ask_density_near: 0.0, density_imbalance: 0.0,
            churn_history: Vec::with_capacity(62),
            churn_rate: 0.0, churn_spike: false,
            update_count: 0,
        }
    }
}

impl DomSnapshot {
    /// Update with new book state. Call on every DoM event (~31/sec).
    pub fn update(&mut self, _total_bid_vol: f64, _total_ask_vol: f64,
                   bid_levels: u32, ask_levels: u32,
                   best_bid: f64, best_ask: f64,
                   book: &std::collections::HashMap<u64, (u8, i32, i64)>) {
        self.bid_levels = bid_levels;
        self.ask_levels = ask_levels;
        self.best_bid = best_bid;
        self.best_ask = best_ask;
        self.book_spread_pips = if best_ask > best_bid && best_bid > 0.0 {
            (best_ask - best_bid) * 10000.0
        } else {
            0.0
        };
        self.active = true;
        self.update_count += 1;

        // ── Level count tracking ──
        let total_levels = bid_levels + ask_levels;
        self.level_history.push(total_levels);
        if self.level_history.len() > 60 { self.level_history.remove(0); }
        if self.level_history.len() >= 10 {
            let avg = self.level_history.iter().sum::<u32>() as f64 / self.level_history.len() as f64;
            self.levels_trend = total_levels as i32 - avg as i32;
            self.levels_dropping = (total_levels as f64) < avg * 0.8;
        }

        // ── Cluster & gap & density analysis (every 30th update, ~1/sec) ──
        if self.update_count % 30 == 0 && book.len() > 4 {
            self.analyze_clusters(book);
        }
    }

    /// Find clusters of nearby price levels, gaps between them, and density near price.
    fn analyze_clusters(&mut self, book: &std::collections::HashMap<u64, (u8, i32, i64)>) {
        // Separate and sort by price
        let mut bids: Vec<(f64, i64)> = Vec::new(); // (price, size) sorted desc
        let mut asks: Vec<(f64, i64)> = Vec::new(); // (price, size) sorted asc
        for &(side, price, size) in book.values() {
            let p = price as f64 / 100_000.0;
            if side == 0 { bids.push((p, size)); }
            else { asks.push((p, size)); }
        }
        bids.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        asks.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        // Cluster detection: group levels within 3 pips of each other
        const CLUSTER_GAP_PIPS: f64 = 3.0;
        self.bid_clusters = Self::find_clusters(&bids, CLUSTER_GAP_PIPS);
        self.ask_clusters = Self::find_clusters(&asks, CLUSTER_GAP_PIPS);

        // Gap detection: find spaces between clusters > 5 pips
        const MIN_GAP_PIPS: f64 = 5.0;
        self.bid_gaps = Self::find_gaps(&self.bid_clusters, MIN_GAP_PIPS, true);
        self.ask_gaps = Self::find_gaps(&self.ask_clusters, MIN_GAP_PIPS, false);

        // Density near market: count levels within 10 pips of best bid/ask
        if self.best_bid > 0.0 {
            let near_count = bids.iter()
                .filter(|(p, _)| (self.best_bid - p) * 10000.0 < 10.0)
                .count() as f64;
            self.bid_density_near = near_count / 10.0; // levels per pip
        }
        if self.best_ask > 0.0 {
            let near_count = asks.iter()
                .filter(|(p, _)| (p - self.best_ask) * 10000.0 < 10.0)
                .count() as f64;
            self.ask_density_near = near_count / 10.0;
        }
        self.density_imbalance = self.bid_density_near - self.ask_density_near;
    }

    /// Group sorted price levels into clusters. Levels within `gap_pips` of each other form a cluster.
    fn find_clusters(levels: &[(f64, i64)], gap_pips: f64) -> Vec<DomCluster> {
        if levels.is_empty() { return Vec::new(); }
        let mut clusters: Vec<DomCluster> = Vec::new();
        let mut cur_prices: Vec<(f64, i64)> = vec![levels[0]];

        for &(price, size) in &levels[1..] {
            let last_price = cur_prices.last().unwrap().0;
            if (last_price - price).abs() * 10000.0 <= gap_pips {
                cur_prices.push((price, size));
            } else {
                clusters.push(Self::make_cluster(&cur_prices));
                cur_prices = vec![(price, size)];
            }
        }
        clusters.push(Self::make_cluster(&cur_prices));
        clusters
    }

    fn make_cluster(levels: &[(f64, i64)]) -> DomCluster {
        let total_size: i64 = levels.iter().map(|(_, s)| s).sum();
        let weighted_price: f64 = levels.iter()
            .map(|(p, s)| p * (*s as f64))
            .sum::<f64>() / total_size.max(1) as f64;
        let prices: Vec<f64> = levels.iter().map(|(p, _)| *p).collect();
        DomCluster {
            center_price: weighted_price,
            total_size,
            level_count: levels.len() as u32,
            price_min: prices.iter().cloned().fold(f64::MAX, f64::min),
            price_max: prices.iter().cloned().fold(f64::MIN, f64::max),
        }
    }

    /// Find gaps between clusters larger than min_gap_pips.
    fn find_gaps(clusters: &[DomCluster], min_gap_pips: f64, descending: bool) -> Vec<DomGap> {
        if clusters.len() < 2 { return Vec::new(); }
        let mut gaps: Vec<DomGap> = Vec::new();
        for i in 0..clusters.len() - 1 {
            let (from, to) = if descending {
                // Bids sorted desc: gap between low of cluster[i] and high of cluster[i+1]
                (clusters[i + 1].price_max, clusters[i].price_min)
            } else {
                // Asks sorted asc: gap between high of cluster[i] and low of cluster[i+1]
                (clusters[i].price_max, clusters[i + 1].price_min)
            };
            let gap_pips = (to - from) * 10000.0;
            if gap_pips > min_gap_pips {
                gaps.push(DomGap { price_from: from, price_to: to, gap_pips });
            }
        }
        gaps.sort_by(|a, b| b.gap_pips.partial_cmp(&a.gap_pips).unwrap_or(std::cmp::Ordering::Equal));
        gaps.truncate(3);
        gaps
    }

    /// Format for Claude prompt (raw data, no interpretation).
    pub fn to_prompt_section(&self) -> String {
        if !self.active {
            return "DOM:\n  Not active".to_string();
        }

        let mut lines = Vec::new();
        lines.push("DOM (price level structure):".to_string());
        lines.push(format!("  Levels: {}bid / {}ask (trend={:+}{})",
            self.bid_levels, self.ask_levels, self.levels_trend,
            if self.levels_dropping { " THINNING" } else { "" }));
        lines.push(format!("  Density near price: bid={:.1}/pip ask={:.1}/pip imbalance={:+.1}",
            self.bid_density_near, self.ask_density_near, self.density_imbalance));

        // Clusters
        if !self.bid_clusters.is_empty() {
            let cl: Vec<String> = self.bid_clusters.iter().take(5)
                .map(|c| format!("{:.5}({}lvl)", c.center_price, c.level_count))
                .collect();
            lines.push(format!("  Bid clusters: {}", cl.join(", ")));
        }
        if !self.ask_clusters.is_empty() {
            let cl: Vec<String> = self.ask_clusters.iter().take(5)
                .map(|c| format!("{:.5}({}lvl)", c.center_price, c.level_count))
                .collect();
            lines.push(format!("  Ask clusters: {}", cl.join(", ")));
        }

        // Gaps
        if !self.bid_gaps.is_empty() {
            let g: Vec<String> = self.bid_gaps.iter()
                .map(|g| format!("{:.5}-{:.5}({:.0}p)", g.price_from, g.price_to, g.gap_pips))
                .collect();
            lines.push(format!("  Bid gaps: {}", g.join(", ")));
        }
        if !self.ask_gaps.is_empty() {
            let g: Vec<String> = self.ask_gaps.iter()
                .map(|g| format!("{:.5}-{:.5}({:.0}p)", g.price_from, g.price_to, g.gap_pips))
                .collect();
            lines.push(format!("  Ask gaps: {}", g.join(", ")));
        }

        lines.join("\n")
    }

    /// For display in UI.
    pub fn status_line(&self) -> String {
        if !self.active {
            return "DoM: not active".to_string();
        }
        let lvl_status = if self.levels_dropping { "THIN" }
            else if self.levels_trend > 5 { "thick" }
            else { "ok" };
        let density_dir = if self.density_imbalance > 0.3 { "bid+" }
            else if self.density_imbalance < -0.3 { "ask+" }
            else { "even" };
        let bid_gaps_count = self.bid_gaps.len();
        let ask_gaps_count = self.ask_gaps.len();
        let largest_gap = self.bid_gaps.iter().chain(self.ask_gaps.iter())
            .map(|g| g.gap_pips)
            .fold(0.0f64, f64::max);
        let mut extra = Vec::new();
        if self.levels_dropping { extra.push("LEVELS-DROP"); }
        if self.churn_spike { extra.push("CHURN-SPIKE"); }
        if largest_gap > 15.0 { extra.push("BIG-GAP"); }
        let extra_str = if extra.is_empty() { String::new() } else { format!(" | {}", extra.join(" ")) };
        format!("DoM: {}b/{}a({}) density={} gaps={}b/{}a max={:.0}p{}",
            self.bid_levels, self.ask_levels, lvl_status,
            density_dir, bid_gaps_count, ask_gaps_count, largest_gap, extra_str)
    }
}

// ── OHLCV candle for candlestick-rs ──────────────────────────────────────────

/// Simple OHLCV tuple that implements the CandleStick trait.
#[derive(Debug, Clone, Copy)]
pub struct OhlcCandle {
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    pub timestamp: i64,
}

impl OhlcCandle {
    pub fn new(open: f64, high: f64, low: f64, close: f64, volume: f64, timestamp: i64) -> Self {
        Self { open, high, low, close, volume, timestamp }
    }
}

impl CandleStick for OhlcCandle {
    fn open(&self) -> f64 { self.open }
    fn high(&self) -> f64 { self.high }
    fn low(&self) -> f64 { self.low }
    fn close(&self) -> f64 { self.close }
    fn volume(&self) -> f64 { self.volume }
}

impl CandleStick for &OhlcCandle {
    fn open(&self) -> f64 { self.open }
    fn high(&self) -> f64 { self.high }
    fn low(&self) -> f64 { self.low }
    fn close(&self) -> f64 { self.close }
    fn volume(&self) -> f64 { self.volume }
}

// ── Forming candle ───────────────────────────────────────────────────────────

/// A candle that is still forming (current period not yet closed).
/// Updated every M1 bar. Can be analyzed for pattern prediction.
#[derive(Debug, Clone)]
pub struct FormingCandle {
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    pub timestamp: i64,      // period start timestamp (seconds)
    pub m1_count: u32,       // how many M1 bars received
    pub period_minutes: u32, // 5, 15, 60, 240
    active: bool,            // has received at least one bar
}

impl FormingCandle {
    pub fn new(period_minutes: u32) -> Self {
        Self {
            open: 0.0, high: 0.0, low: 0.0, close: 0.0, volume: 0.0,
            timestamp: 0, m1_count: 0, period_minutes, active: false,
        }
    }

    /// Reset for a new period. Called when the previous period closes.
    pub fn reset(&mut self, open_price: f64, timestamp: i64) {
        self.open = open_price;
        self.high = open_price;
        self.low = open_price;
        self.close = open_price;
        self.volume = 0.0;
        self.timestamp = timestamp;
        self.m1_count = 0;
        self.active = true;
    }

    /// Update with a new M1 candle.
    pub fn update_from_m1(&mut self, m1_high: f64, m1_low: f64, m1_close: f64, m1_volume: f64) {
        if !self.active {
            return;
        }
        if m1_high > self.high { self.high = m1_high; }
        if m1_low < self.low { self.low = m1_low; }
        self.close = m1_close;
        self.volume += m1_volume;
        self.m1_count += 1;
    }

    /// Update directly from cTrader LiveTrendbar data.
    pub fn update_from_trendbar(&mut self, open: f64, high: f64, low: f64, close: f64, volume: f64, timestamp: i64) {
        self.open = open;
        self.high = high;
        self.low = low;
        self.close = close;
        self.volume = volume;
        self.timestamp = timestamp;
        self.active = true;
    }

    /// How complete is this bar? (0.0 = just started, 1.0 = about to close)
    pub fn completion_pct(&self) -> f64 {
        if self.period_minutes == 0 { return 0.0; }
        (self.m1_count as f64 / self.period_minutes as f64).min(1.0)
    }

    /// Is this candle active (has data)?
    pub fn is_active(&self) -> bool {
        self.active && self.high > 0.0
    }

    /// Convert to OhlcCandle for candlestick-rs pattern detection.
    pub fn as_ohlc(&self) -> OhlcCandle {
        OhlcCandle::new(self.open, self.high, self.low, self.close, self.volume, self.timestamp)
    }

    /// Range in pips (for EUR/USD).
    pub fn range_pips(&self) -> f64 {
        (self.high - self.low) * 10000.0
    }
}

// ── Pattern detection result ─────────────────────────────────────────────────

/// Detected patterns on a single candle.
#[derive(Debug, Clone, Default)]
pub struct SinglePatterns {
    pub is_bullish: bool,
    pub is_bearish: bool,
    pub is_hammer: bool,
    pub is_inverted_hammer: bool,
    pub is_shooting_star: bool,
    pub is_hanging_man: bool,
    pub is_doji: bool,
    pub is_long_legged_doji: bool,
    pub is_dragonfly_doji: bool,
    pub is_gravestone_doji: bool,
    pub is_spinning_top: bool,
    pub is_marubozu: bool,
    pub is_bullish_marubozu: bool,
    pub is_bearish_marubozu: bool,
}

impl SinglePatterns {
    pub fn detect(candle: &OhlcCandle) -> Self {
        // Skip detection if range is too small (< 0.5 pips for EURUSD)
        if (candle.high - candle.low) < 0.00005 {
            return Self::default();
        }
        Self {
            is_bullish: candle.is_bullish(),
            is_bearish: candle.is_bearish(),
            is_hammer: candle.is_hammer(),
            is_inverted_hammer: candle.is_inverted_hammer(),
            is_shooting_star: candle.is_shooting_star(),
            is_hanging_man: candle.is_hanging_man(),
            is_doji: candle.is_doji(),
            is_long_legged_doji: candle.is_long_legged_doji(),
            is_dragonfly_doji: candle.is_dragonfly_doji(),
            is_gravestone_doji: candle.is_gravestone_doji(),
            is_spinning_top: candle.is_spinning_top(),
            is_marubozu: candle.is_marubozu(),
            is_bullish_marubozu: candle.is_bullish_marubozu(),
            is_bearish_marubozu: candle.is_bearish_marubozu(),
        }
    }

    /// Returns a short string summary of detected patterns.
    pub fn summary(&self) -> String {
        let mut patterns = Vec::new();
        if self.is_hammer { patterns.push("Hammer"); }
        if self.is_inverted_hammer { patterns.push("InvHammer"); }
        if self.is_shooting_star { patterns.push("ShootingStar"); }
        if self.is_hanging_man && !self.is_hammer { patterns.push("HangingMan"); }
        if self.is_dragonfly_doji { patterns.push("DragonflyDoji"); }
        else if self.is_gravestone_doji { patterns.push("GravestoneDoji"); }
        else if self.is_long_legged_doji { patterns.push("LongLegDoji"); }
        else if self.is_doji { patterns.push("Doji"); }
        if self.is_spinning_top && !self.is_doji { patterns.push("SpinTop"); }
        if self.is_bullish_marubozu { patterns.push("BullMarubozu"); }
        if self.is_bearish_marubozu { patterns.push("BearMarubozu"); }
        if patterns.is_empty() {
            if self.is_bullish { "Bullish".to_string() }
            else if self.is_bearish { "Bearish".to_string() }
            else { "Flat".to_string() }
        } else {
            patterns.join(", ")
        }
    }

    /// Does this pattern suggest a bullish reversal?
    pub fn is_bullish_reversal(&self) -> bool {
        self.is_hammer || self.is_dragonfly_doji || self.is_bullish_marubozu
    }

    /// Does this pattern suggest a bearish reversal?
    pub fn is_bearish_reversal(&self) -> bool {
        self.is_shooting_star || self.is_gravestone_doji || self.is_bearish_marubozu
    }
}

/// Detected multi-candle patterns from a CandleStream.
#[derive(Debug, Clone, Default)]
pub struct MultiPatterns {
    pub bullish_engulfing: bool,
    pub bearish_engulfing: bool,
    pub bullish_harami: bool,
    pub bearish_harami: bool,
    pub bullish_doji_star: bool,
    pub bearish_doji_star: bool,
    pub morning_star: bool,
    pub morning_star_doji: bool,
    pub evening_star: bool,
    pub evening_star_doji: bool,
    pub three_white_soldiers: bool,
    pub three_black_crows: bool,
    pub three_inside_up: bool,
    pub three_inside_down: bool,
    pub dark_cloud_cover: bool,
}

impl MultiPatterns {
    pub fn detect(stream: &CandleStream<OhlcCandle>) -> Self {
        Self {
            bullish_engulfing: stream.is_bullish_engulfing(),
            bearish_engulfing: stream.is_bearish_engulfing(),
            bullish_harami: stream.is_bullish_harami(),
            bearish_harami: stream.is_bearish_harami(),
            bullish_doji_star: stream.is_bullish_doji_star(),
            bearish_doji_star: stream.is_bearish_doji_star(),
            morning_star: stream.is_morning_star(),
            morning_star_doji: stream.is_morning_star_doji(),
            evening_star: stream.is_evening_star(),
            evening_star_doji: stream.is_evening_star_doji(),
            three_white_soldiers: stream.is_three_white_soldiers(),
            three_black_crows: stream.is_three_black_crows(),
            three_inside_up: stream.is_three_inside_up(),
            three_inside_down: stream.is_three_inside_down(),
            dark_cloud_cover: stream.is_dark_cloud_cover(),
        }
    }

    /// Returns a short string summary of detected patterns.
    pub fn summary(&self) -> String {
        let mut patterns = Vec::new();
        if self.bullish_engulfing { patterns.push("BullEngulfing"); }
        if self.bearish_engulfing { patterns.push("BearEngulfing"); }
        if self.bullish_harami { patterns.push("BullHarami"); }
        if self.bearish_harami { patterns.push("BearHarami"); }
        if self.bullish_doji_star { patterns.push("BullDojiStar"); }
        if self.bearish_doji_star { patterns.push("BearDojiStar"); }
        if self.morning_star { patterns.push("MorningStar"); }
        if self.morning_star_doji { patterns.push("MorningStarDoji"); }
        if self.evening_star { patterns.push("EveningStar"); }
        if self.evening_star_doji { patterns.push("EveningStarDoji"); }
        if self.three_white_soldiers { patterns.push("3WhiteSoldiers"); }
        if self.three_black_crows { patterns.push("3BlackCrows"); }
        if self.three_inside_up { patterns.push("3InsideUp"); }
        if self.three_inside_down { patterns.push("3InsideDown"); }
        if self.dark_cloud_cover { patterns.push("DarkCloud"); }
        if patterns.is_empty() {
            "None".to_string()
        } else {
            patterns.join(", ")
        }
    }

    /// Does any multi-candle pattern suggest bullish reversal?
    pub fn is_bullish_signal(&self) -> bool {
        self.bullish_engulfing || self.bullish_harami || self.bullish_doji_star
        || self.morning_star || self.morning_star_doji
        || self.three_white_soldiers || self.three_inside_up
    }

    /// Does any multi-candle pattern suggest bearish reversal?
    pub fn is_bearish_signal(&self) -> bool {
        self.bearish_engulfing || self.bearish_harami || self.bearish_doji_star
        || self.evening_star || self.evening_star_doji
        || self.three_black_crows || self.three_inside_down || self.dark_cloud_cover
    }
}

// ── Swing Point Detection ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SwingType {
    High,
    Low,
}

#[derive(Debug, Clone, Copy)]
pub struct SwingPoint {
    pub swing_type: SwingType,
    pub price: f64,
    pub index: usize,
    pub timestamp: i64,
}

/// Swing structure analysis from completed bars.
#[derive(Debug, Clone, Default)]
pub struct SwingAnalysis {
    pub swing_highs: Vec<SwingPoint>,
    pub swing_lows: Vec<SwingPoint>,
    /// 1 = uptrend (HH+HL), -1 = downtrend (LH+LL), 0 = mixed/insufficient
    pub trend_direction: i32,
    pub last_swing_high: Option<f64>,
    pub last_swing_low: Option<f64>,
}

impl SwingAnalysis {
    /// Detect swing highs/lows using 3-bar pivot logic.
    /// `min_swing_pips` filters out noise (e.g., 3.0 for M5, 5.0 for M15).
    pub fn detect(completed: &[OhlcCandle], min_swing_pips: f64) -> Self {
        let mut highs = Vec::new();
        let mut lows = Vec::new();
        let min_diff = min_swing_pips * 0.0001; // convert pips to price

        if completed.len() < 3 {
            return Self::default();
        }

        // 3-bar pivot detection
        for i in 1..completed.len() - 1 {
            let prev = &completed[i - 1];
            let curr = &completed[i];
            let next = &completed[i + 1];

            // Swing high: current high > both neighbors by min_diff
            if curr.high > prev.high + min_diff && curr.high > next.high + min_diff {
                highs.push(SwingPoint {
                    swing_type: SwingType::High,
                    price: curr.high,
                    index: i,
                    timestamp: curr.timestamp,
                });
            }

            // Swing low: current low < both neighbors by min_diff
            if curr.low < prev.low - min_diff && curr.low < next.low - min_diff {
                lows.push(SwingPoint {
                    swing_type: SwingType::Low,
                    price: curr.low,
                    index: i,
                    timestamp: curr.timestamp,
                });
            }
        }

        // Keep last 5 of each
        if highs.len() > 5 { highs.drain(..highs.len() - 5); }
        if lows.len() > 5 { lows.drain(..lows.len() - 5); }

        let last_high = highs.last().map(|s| s.price);
        let last_low = lows.last().map(|s| s.price);

        // Determine trend from swing structure
        let trend_direction = Self::classify_trend(&highs, &lows);

        Self {
            swing_highs: highs,
            swing_lows: lows,
            trend_direction,
            last_swing_high: last_high,
            last_swing_low: last_low,
        }
    }

    fn classify_trend(highs: &[SwingPoint], lows: &[SwingPoint]) -> i32 {
        if highs.len() < 2 || lows.len() < 2 {
            return 0;
        }
        let h = &highs[highs.len() - 2..];
        let l = &lows[lows.len() - 2..];

        let higher_highs = h[1].price > h[0].price;
        let higher_lows = l[1].price > l[0].price;
        let lower_highs = h[1].price < h[0].price;
        let lower_lows = l[1].price < l[0].price;

        if higher_highs && higher_lows { 1 }       // uptrend
        else if lower_highs && lower_lows { -1 }    // downtrend
        else { 0 }                                    // mixed
    }

    pub fn summary(&self) -> String {
        let trend = match self.trend_direction {
            1 => "UP(HH+HL)",
            -1 => "DN(LH+LL)",
            _ => "mixed",
        };
        format!("Swing:{} H={} L={}", trend, self.swing_highs.len(), self.swing_lows.len())
    }
}

// ── Fibonacci Retracement ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum RetracementClass {
    #[default]
    None,
    ShallowPullback,   // < 38.2%
    NormalPullback,    // 38.2% - 61.8%
    DeepPullback,      // 61.8% - 78.6%
    Reversal,          // > 78.6%
}

impl RetracementClass {
    pub fn label(&self) -> &'static str {
        match self {
            RetracementClass::None => "none",
            RetracementClass::ShallowPullback => "shallow",
            RetracementClass::NormalPullback => "pullback",
            RetracementClass::DeepPullback => "deep",
            RetracementClass::Reversal => "reversal",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct FibonacciLevels {
    pub active: bool,
    pub swing_high: f64,
    pub swing_low: f64,
    pub is_upswing: bool,
    pub level_236: f64,
    pub level_382: f64,
    pub level_500: f64,
    pub level_618: f64,
    pub level_786: f64,
    /// Where current price sits as retracement ratio (0.0 = no retrace, 1.0 = full retrace)
    pub current_retracement: f64,
    pub retracement_class: RetracementClass,
    /// Price proximity to nearest fib level (in pips)
    pub nearest_fib_pips: f64,
    pub nearest_fib_name: &'static str,
}

impl FibonacciLevels {
    /// Calculate fib levels from the most recent swing high and low.
    /// `current_price` is the latest close or forming close.
    pub fn calculate(swings: &SwingAnalysis, current_price: f64) -> Self {
        let (sh, sl) = match (swings.last_swing_high, swings.last_swing_low) {
            (Some(h), Some(l)) if (h - l).abs() > 0.0003 => (h, l), // need at least 3 pips range
            _ => return Self::default(),
        };

        let range = sh - sl;

        // Determine if we're measuring from an upswing or downswing
        // by checking which swing point is more recent
        let last_high_idx = swings.swing_highs.last().map(|s| s.index).unwrap_or(0);
        let last_low_idx = swings.swing_lows.last().map(|s| s.index).unwrap_or(0);
        let is_upswing = last_high_idx > last_low_idx; // most recent swing is a high

        // Fib retracement levels
        let (l236, l382, l500, l618, l786) = if is_upswing {
            // Retracing down from high
            (sh - range * 0.236, sh - range * 0.382, sh - range * 0.5,
             sh - range * 0.618, sh - range * 0.786)
        } else {
            // Retracing up from low
            (sl + range * 0.236, sl + range * 0.382, sl + range * 0.5,
             sl + range * 0.618, sl + range * 0.786)
        };

        // Current retracement ratio
        let retrace = if is_upswing {
            if range > 0.0 { (sh - current_price) / range } else { 0.0 }
        } else {
            if range > 0.0 { (current_price - sl) / range } else { 0.0 }
        };
        let retrace_clamped = retrace.max(0.0);

        let class = if retrace_clamped < 0.0 || retrace_clamped > 1.5 {
            RetracementClass::None
        } else if retrace_clamped < 0.382 {
            RetracementClass::ShallowPullback
        } else if retrace_clamped < 0.618 {
            RetracementClass::NormalPullback
        } else if retrace_clamped < 0.786 {
            RetracementClass::DeepPullback
        } else {
            RetracementClass::Reversal
        };

        // Find nearest fib level
        let fib_levels = [
            (l236, "23.6%"), (l382, "38.2%"), (l500, "50.0%"),
            (l618, "61.8%"), (l786, "78.6%"),
        ];
        let (nearest_dist, nearest_name) = fib_levels.iter()
            .map(|(lvl, name)| (((current_price - lvl) * 10000.0).abs(), *name))
            .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap_or((999.0, "none"));

        Self {
            active: true,
            swing_high: sh,
            swing_low: sl,
            is_upswing,
            level_236: l236,
            level_382: l382,
            level_500: l500,
            level_618: l618,
            level_786: l786,
            current_retracement: retrace_clamped,
            retracement_class: class,
            nearest_fib_pips: nearest_dist,
            nearest_fib_name: nearest_name,
        }
    }

    pub fn summary(&self) -> String {
        if !self.active { return "Fib:inactive".to_string(); }
        let dir = if self.is_upswing { "up" } else { "dn" };
        format!("Fib({}): {:.0}% {} @{:.1}p from {}",
            dir, self.current_retracement * 100.0,
            self.retracement_class.label(),
            self.nearest_fib_pips, self.nearest_fib_name)
    }

    pub fn to_prompt_section(&self, tf_name: &str) -> String {
        if !self.active { return String::new(); }
        let dir = if self.is_upswing { "upswing" } else { "downswing" };
        format!(
            "  FIBONACCI ({} {:.5}→{:.5}): 23.6%={:.5} 38.2%={:.5} 50%={:.5} 61.8%={:.5} 78.6%={:.5}\n    Current retracement: {:.0}% ({}) | nearest fib: {} ({:.1}p away)",
            dir, self.swing_low, self.swing_high,
            self.level_236, self.level_382, self.level_500, self.level_618, self.level_786,
            self.current_retracement * 100.0, self.retracement_class.label(),
            self.nearest_fib_name, self.nearest_fib_pips
        )
    }
}

// ── Chart Pattern Detection ─────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ChartPatternType {
    AscendingChannel,
    DescendingChannel,
    HorizontalChannel,
    AscendingTriangle,
    DescendingTriangle,
    SymmetricTriangle,
    RisingWedge,
    FallingWedge,
}

impl ChartPatternType {
    pub fn label(&self) -> &'static str {
        match self {
            ChartPatternType::AscendingChannel => "AscChannel",
            ChartPatternType::DescendingChannel => "DescChannel",
            ChartPatternType::HorizontalChannel => "HorizChannel",
            ChartPatternType::AscendingTriangle => "AscTriangle",
            ChartPatternType::DescendingTriangle => "DescTriangle",
            ChartPatternType::SymmetricTriangle => "SymTriangle",
            ChartPatternType::RisingWedge => "RisingWedge",
            ChartPatternType::FallingWedge => "FallingWedge",
        }
    }

    /// Expected breakout direction: 1=up, -1=down
    pub fn breakout_bias(&self) -> i32 {
        match self {
            ChartPatternType::AscendingChannel => 1,
            ChartPatternType::DescendingChannel => -1,
            ChartPatternType::HorizontalChannel => 0,
            ChartPatternType::AscendingTriangle => 1,
            ChartPatternType::DescendingTriangle => -1,
            ChartPatternType::SymmetricTriangle => 0,
            ChartPatternType::RisingWedge => -1,   // wedges break opposite
            ChartPatternType::FallingWedge => 1,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChartPattern {
    pub pattern_type: ChartPatternType,
    pub confidence: f64,
    pub upper_slope: f64,   // pips per bar
    pub lower_slope: f64,   // pips per bar
    pub upper_at_current: f64,  // projected upper trendline at last bar
    pub lower_at_current: f64,  // projected lower trendline at last bar
}

/// Chart pattern analysis result.
#[derive(Debug, Clone, Default)]
pub struct ChartPatternAnalysis {
    pub detected: Option<ChartPattern>,
}

impl ChartPatternAnalysis {
    /// Detect chart patterns from swing points using linear regression on swing highs/lows.
    pub fn detect(swings: &SwingAnalysis) -> Self {
        if swings.swing_highs.len() < 2 || swings.swing_lows.len() < 2 {
            return Self::default();
        }

        // Linear regression on swing highs (upper trendline)
        let (upper_slope, upper_intercept, upper_r2) = Self::linear_regression(
            &swings.swing_highs.iter().map(|s| (s.index as f64, s.price)).collect::<Vec<_>>()
        );
        // Linear regression on swing lows (lower trendline)
        let (lower_slope, lower_intercept, lower_r2) = Self::linear_regression(
            &swings.swing_lows.iter().map(|s| (s.index as f64, s.price)).collect::<Vec<_>>()
        );

        // Convert slopes to pips/bar
        let upper_pips = upper_slope * 10000.0;
        let lower_pips = lower_slope * 10000.0;

        // Confidence based on R-squared (need at least 3 points for meaningful R2)
        let min_points = swings.swing_highs.len().min(swings.swing_lows.len());
        let base_confidence = if min_points >= 3 {
            (upper_r2 + lower_r2) / 2.0
        } else {
            // With only 2 points R2=1.0 always, so discount
            0.5
        };

        // Classify pattern based on slopes
        let flat_threshold = 0.5; // pips/bar — slopes smaller than this are "flat"
        let parallel_threshold = 0.8; // slopes within this range are "parallel"

        let upper_flat = upper_pips.abs() < flat_threshold;
        let lower_flat = lower_pips.abs() < flat_threshold;
        let both_up = upper_pips > flat_threshold && lower_pips > flat_threshold;
        let both_down = upper_pips < -flat_threshold && lower_pips < -flat_threshold;
        let slopes_converge = upper_pips < lower_pips; // upper coming down or lower going up relative

        let pattern_type = if upper_flat && lower_flat {
            Some(ChartPatternType::HorizontalChannel)
        } else if upper_flat && lower_pips > flat_threshold {
            Some(ChartPatternType::AscendingTriangle)
        } else if lower_flat && upper_pips < -flat_threshold {
            Some(ChartPatternType::DescendingTriangle)
        } else if both_up && (upper_pips - lower_pips).abs() < parallel_threshold {
            Some(ChartPatternType::AscendingChannel)
        } else if both_down && (upper_pips - lower_pips).abs() < parallel_threshold {
            Some(ChartPatternType::DescendingChannel)
        } else if both_up && upper_pips < lower_pips {
            Some(ChartPatternType::RisingWedge) // converging upward
        } else if both_down && upper_pips > lower_pips {
            Some(ChartPatternType::FallingWedge) // converging downward
        } else if slopes_converge && upper_pips < 0.0 && lower_pips > 0.0 {
            Some(ChartPatternType::SymmetricTriangle)
        } else {
            None
        };

        let detected = pattern_type.map(|pt| {
            // Project trendlines to the last known index
            let last_idx = swings.swing_highs.last().unwrap().index
                .max(swings.swing_lows.last().unwrap().index) as f64;
            ChartPattern {
                pattern_type: pt,
                confidence: base_confidence.max(0.0).min(1.0),
                upper_slope: upper_pips,
                lower_slope: lower_pips,
                upper_at_current: upper_intercept + upper_slope * last_idx,
                lower_at_current: lower_intercept + lower_slope * last_idx,
            }
        });

        // Filter low confidence
        let detected = detected.filter(|p| p.confidence >= 0.4);

        Self { detected }
    }

    /// Simple least-squares linear regression.
    /// Returns (slope, intercept, r_squared).
    fn linear_regression(points: &[(f64, f64)]) -> (f64, f64, f64) {
        let n = points.len() as f64;
        if n < 2.0 { return (0.0, 0.0, 0.0); }

        let sum_x: f64 = points.iter().map(|p| p.0).sum();
        let sum_y: f64 = points.iter().map(|p| p.1).sum();
        let sum_xy: f64 = points.iter().map(|p| p.0 * p.1).sum();
        let sum_x2: f64 = points.iter().map(|p| p.0 * p.0).sum();
        let sum_y2: f64 = points.iter().map(|p| p.1 * p.1).sum();

        let denom = n * sum_x2 - sum_x * sum_x;
        if denom.abs() < 1e-15 { return (0.0, sum_y / n, 0.0); }

        let slope = (n * sum_xy - sum_x * sum_y) / denom;
        let intercept = (sum_y - slope * sum_x) / n;

        // R-squared
        let ss_tot = sum_y2 - sum_y * sum_y / n;
        let ss_res: f64 = points.iter()
            .map(|p| { let pred = intercept + slope * p.0; (p.1 - pred).powi(2) })
            .sum();
        let r2 = if ss_tot > 1e-15 { 1.0 - ss_res / ss_tot } else { 1.0 };

        (slope, intercept, r2.max(0.0))
    }

    pub fn summary(&self) -> String {
        match &self.detected {
            Some(p) => format!("{}({:.0}%)", p.pattern_type.label(), p.confidence * 100.0),
            None => "NoPat".to_string(),
        }
    }

    pub fn to_prompt_section(&self) -> String {
        match &self.detected {
            Some(p) => format!(
                "  CHART PATTERN: {} (conf={:.0}%) upper_slope={:+.1}p/bar lower_slope={:+.1}p/bar breakout_bias={}",
                p.pattern_type.label(), p.confidence * 100.0,
                p.upper_slope, p.lower_slope,
                match p.pattern_type.breakout_bias() { 1 => "UP", -1 => "DOWN", _ => "NEUTRAL" }
            ),
            None => String::new(),
        }
    }
}

// ── Timeframe state ──────────────────────────────────────────────────────────

/// State for one timeframe: completed bar stream + forming candle + detected patterns.
pub struct TimeframeState {
    pub name: &'static str,
    pub period_minutes: u32,
    pub stream: CandleStream<'static, OhlcCandle>,
    pub forming: FormingCandle,
    /// Last 5 completed candles (owned, for CandleStream references).
    completed: Vec<OhlcCandle>,
    /// Patterns on the last completed bar.
    pub last_single: SinglePatterns,
    /// Multi-candle patterns across last few completed bars.
    pub last_multi: MultiPatterns,
    /// Patterns detected on the forming (current) candle.
    pub forming_patterns: SinglePatterns,
    /// Internal M1 bars within the current forming candle (for structure analysis).
    pub m1_inside: Vec<OhlcCandle>,
    /// Last completed bar timestamp (to detect new bar close).
    pub last_bar_ts: i64,
    /// Swing point analysis from completed bars.
    pub swing_analysis: SwingAnalysis,
    /// Fibonacci retracement levels from swing points.
    pub fib_levels: FibonacciLevels,
    /// Chart pattern detection (channels, triangles, wedges).
    pub chart_pattern: ChartPatternAnalysis,
}

impl TimeframeState {
    pub fn new(name: &'static str, period_minutes: u32) -> Self {
        Self {
            name,
            period_minutes,
            stream: CandleStream::new(),
            forming: FormingCandle::new(period_minutes),
            completed: Vec::with_capacity(22),
            last_single: SinglePatterns::default(),
            last_multi: MultiPatterns::default(),
            forming_patterns: SinglePatterns::default(),
            m1_inside: Vec::with_capacity(period_minutes as usize + 1),
            last_bar_ts: 0,
            swing_analysis: SwingAnalysis::default(),
            fib_levels: FibonacciLevels::default(),
            chart_pattern: ChartPatternAnalysis::default(),
        }
    }

    /// Process a completed bar for this timeframe.
    /// Returns true if this is a NEW bar (not seen before).
    pub fn push_completed(&mut self, candle: OhlcCandle) -> bool {
        if candle.timestamp <= self.last_bar_ts {
            return false; // already processed
        }
        self.last_bar_ts = candle.timestamp;

        // Store completed candle (keep last 20 for trend analysis)
        self.completed.push(candle);
        if self.completed.len() > 20 {
            self.completed.remove(0);
        }

        // Detect single-candle patterns on the just-closed bar
        self.last_single = SinglePatterns::detect(&candle);

        // Rebuild CandleStream from completed candles (need owned references)
        // CandleStream borrows, so we rebuild each time
        self.last_multi = self.detect_multi_patterns();

        // Swing / Fibonacci / Chart pattern detection
        let min_pips = if self.period_minutes >= 15 { 5.0 } else { 3.0 };
        self.swing_analysis = SwingAnalysis::detect(&self.completed, min_pips);
        self.fib_levels = FibonacciLevels::calculate(&self.swing_analysis, candle.close);
        self.chart_pattern = ChartPatternAnalysis::detect(&self.swing_analysis);

        // Reset forming candle for next period
        self.forming = FormingCandle::new(self.period_minutes);
        self.m1_inside.clear();

        true
    }

    /// Update the forming candle from a LiveTrendbar update.
    pub fn update_forming(&mut self, open: f64, high: f64, low: f64, close: f64, volume: f64, timestamp: i64) {
        self.forming.update_from_trendbar(open, high, low, close, volume, timestamp);

        // Detect patterns on the forming candle
        if self.forming.is_active() && self.forming.range_pips() > 0.5 {
            let ohlc = self.forming.as_ohlc();
            self.forming_patterns = SinglePatterns::detect(&ohlc);
        }

        // Update fib retracement with current price
        if self.swing_analysis.last_swing_high.is_some() {
            self.fib_levels = FibonacciLevels::calculate(&self.swing_analysis, close);
        }
    }

    /// Record a completed M1 bar inside the current forming candle (for structure analysis).
    pub fn push_m1_inside(&mut self, m1: OhlcCandle) {
        self.m1_inside.push(m1);
    }

    /// Detect multi-candle patterns from the completed bars.
    fn detect_multi_patterns(&self) -> MultiPatterns {
        if self.completed.len() < 2 {
            return MultiPatterns::default();
        }
        // CandleStream needs references with a lifetime.
        // We create a local stream and push the last 5 completed candles.
        let mut stream = CandleStream::new();
        let start = if self.completed.len() > 5 { self.completed.len() - 5 } else { 0 };
        for candle in &self.completed[start..] {
            stream.push(candle);
        }
        MultiPatterns::detect(&stream)
    }

    /// How strong is the last completed pattern relative to recent candle sizes?
    /// Returns (bull_strength, bear_strength) each 0-3:
    ///   0 = no pattern, 1 = normal, 2 = strong (big candle), 3 = very strong (huge + reversal)
    pub fn pattern_strength(&self) -> (i32, i32) {
        if self.completed.len() < 3 { return (0, 0); }

        let last = self.completed.last().unwrap();
        let last_range = last.high - last.low;

        // Average range of previous bars (excluding last)
        let prev_bars = &self.completed[self.completed.len().saturating_sub(6)..self.completed.len() - 1];
        let avg_range = prev_bars.iter().map(|c| c.high - c.low).sum::<f64>() / prev_bars.len().max(1) as f64;

        let size_ratio = if avg_range > 0.0 { last_range / avg_range } else { 1.0 };

        let mut bull = 0i32;
        let mut bear = 0i32;

        // Single-candle pattern strength
        if self.last_single.is_bullish_reversal() {
            bull = if size_ratio > 2.5 { 3 } else if size_ratio > 1.5 { 2 } else { 1 };
        }
        if self.last_single.is_bearish_reversal() {
            bear = if size_ratio > 2.5 { 3 } else if size_ratio > 1.5 { 2 } else { 1 };
        }

        // Multi-candle patterns are inherently stronger
        if self.last_multi.is_bullish_signal() {
            let base = if size_ratio > 2.0 { 3 } else if size_ratio > 1.3 { 2 } else { 1 };
            bull = bull.max(base);
        }
        if self.last_multi.is_bearish_signal() {
            let base = if size_ratio > 2.0 { 3 } else if size_ratio > 1.3 { 2 } else { 1 };
            bear = bear.max(base);
        }

        // Marubozu (full body, no wicks) = strong conviction regardless of size
        if self.last_single.is_bullish_marubozu && size_ratio > 1.2 { bull = bull.max(2); }
        if self.last_single.is_bearish_marubozu && size_ratio > 1.2 { bear = bear.max(2); }

        (bull, bear)
    }

    /// Analyze internal M1 structure of the forming candle.
    /// Returns: (bullish_count, bearish_count, higher_lows, lower_highs)
    pub fn internal_structure(&self) -> (u32, u32, bool, bool) {
        if self.m1_inside.len() < 3 {
            return (0, 0, false, false);
        }
        let mut bull = 0u32;
        let mut bear = 0u32;
        let mut higher_lows = true;
        let mut lower_highs = true;

        for i in 0..self.m1_inside.len() {
            let c = &self.m1_inside[i];
            if c.close > c.open { bull += 1; } else { bear += 1; }
            if i > 0 {
                if c.low <= self.m1_inside[i - 1].low { higher_lows = false; }
                if c.high >= self.m1_inside[i - 1].high { lower_highs = false; }
            }
        }
        (bull, bear, higher_lows, lower_highs)
    }

    /// Get a display line for this timeframe's current state.
    pub fn status_line(&self) -> String {
        let forming_str = if self.forming.is_active() {
            let pct = (self.forming.completion_pct() * 100.0) as u32;
            if pct < 20 || self.forming.range_pips() < 1.0 {
                format!("Forming({}%) ...", pct)
            } else {
                let fp = self.forming_patterns.summary();
                format!("Forming({}%): {}", pct, fp)
            }
        } else {
            "Waiting...".to_string()
        };

        // Trend summary from completed bars (last 5)
        let trend_str = if self.completed.len() >= 3 {
            let last5 = &self.completed[self.completed.len().saturating_sub(5)..];
            let bull_count = last5.iter().filter(|c| c.close > c.open).count();
            let bear_count = last5.len() - bull_count;
            let net_move = last5.last().unwrap().close - last5.first().unwrap().open;
            let net_pips = net_move * 10000.0;
            let dir = if bull_count >= 4 { "UP" }
                else if bear_count >= 4 { "DOWN" }
                else if bull_count >= 3 { "up" }
                else if bear_count >= 3 { "dn" }
                else { "mix" };
            format!("{}({}/{}) {:+.0}p", dir, bull_count, last5.len(), net_pips)
        } else {
            "...".to_string()
        };

        // Pattern strength (size-weighted)
        let (ps_bull, ps_bear) = self.pattern_strength();
        let strength_str = if ps_bull >= 3 || ps_bear >= 3 { "STRONG" }
            else if ps_bull >= 2 || ps_bear >= 2 { "solid" }
            else if ps_bull >= 1 || ps_bear >= 1 { "weak" }
            else { "" };

        let multi_str = self.last_multi.summary();
        let multi_part = if multi_str == "None" { String::new() } else { format!(" | Multi: {}", multi_str) };
        let strength_part = if strength_str.is_empty() { String::new() } else { format!(" ({})", strength_str) };

        // Swing / Fib / Chart pattern
        let swing_str = self.swing_analysis.summary();
        let fib_str = self.fib_levels.summary();
        let chart_str = self.chart_pattern.summary();
        let structure_part = if self.swing_analysis.swing_highs.is_empty() {
            String::new()
        } else {
            format!("\n  {} | {} | {}", swing_str, fib_str, chart_str)
        };

        format!("{}: {} | Trend: {} | Last: {}{}{}{}",
            self.name, forming_str, trend_str,
            self.last_single.summary(), strength_part, multi_part, structure_part)
    }
}

// ── Pattern Engine (all timeframes) ──────────────────────────────────────────

/// The main pattern detection engine. Holds state for all timeframes.
pub struct PatternEngine {
    pub m5: TimeframeState,
    pub m15: TimeframeState,
    /// Latest M1 candle data (for entry timing).
    pub last_m1: Option<OhlcCandle>,
    /// Last 20 M1 candles (for momentum micro-read and Claude prompt).
    m1_recent: Vec<OhlcCandle>,
    /// Live DoM snapshot.
    pub dom: DomSnapshot,
}

impl PatternEngine {
    pub fn new() -> Self {
        Self {
            m5: TimeframeState::new("M5", 5),
            m15: TimeframeState::new("M15", 15),
            last_m1: None,
            m1_recent: Vec::with_capacity(22),
            dom: DomSnapshot::default(),
        }
    }

    /// Update the live DoM analytics. Pass the full book for wall detection.
    pub fn update_dom(&mut self, total_bid_vol: f64, total_ask_vol: f64,
                       bid_levels: u32, ask_levels: u32,
                       best_bid: f64, best_ask: f64,
                       book: &std::collections::HashMap<u64, (u8, i32, i64)>) {
        self.dom.update(total_bid_vol, total_ask_vol, bid_levels, ask_levels,
                        best_bid, best_ask, book);
    }

    /// Record a completed M1 candle. Used for M5 internal structure and entry timing.
    pub fn push_m1(&mut self, m1: OhlcCandle) {
        self.last_m1 = Some(m1);
        self.m1_recent.push(m1);
        if self.m1_recent.len() > 20 {
            self.m1_recent.remove(0);
        }

        // Record M1 inside M5 forming candle for entry timing analysis
        self.m5.push_m1_inside(m1);
    }

    /// Process a completed bar for the given timeframe period.
    /// `period_minutes`: 5, 15, 60, or 240.
    pub fn push_completed_bar(&mut self, period_minutes: u32, candle: OhlcCandle) {
        match period_minutes {
            5 => { self.m5.push_completed(candle); }
            15 => { self.m15.push_completed(candle); }
            _ => {}
        }
    }

    /// Update the forming candle from a LiveTrendbar update.
    pub fn update_forming(&mut self, period_minutes: u32, open: f64, high: f64, low: f64, close: f64, volume: f64, timestamp: i64) {
        match period_minutes {
            5 => self.m5.update_forming(open, high, low, close, volume, timestamp),
            15 => self.m15.update_forming(open, high, low, close, volume, timestamp),
            _ => {}
        }
    }

    /// M1 momentum: are the last N M1 bars bullish?
    pub fn m1_momentum(&self) -> (i32, bool, bool) {
        if self.m1_recent.len() < 3 {
            return (0, false, false);
        }
        let last3 = &self.m1_recent[self.m1_recent.len() - 3..];
        let bullish_count = last3.iter().filter(|c| c.close > c.open).count() as i32;
        let bearish_count = 3 - bullish_count;

        let higher_lows = last3[1].low > last3[0].low && last3[2].low > last3[1].low;
        let lower_highs = last3[1].high < last3[0].high && last3[2].high < last3[1].high;

        let momentum = bullish_count - bearish_count; // -3 to +3
        (momentum, higher_lows, lower_highs)
    }

    /// Analyze completed bars across all timeframes for trend context.
    /// Returns (bull_score, bear_score) where each is 0-4+.
    /// Checks: consecutive direction, large candle moves, net close-vs-open across bars.
    fn completed_bar_trend(&self) -> (i32, i32) {
        let mut bull = 0i32;
        let mut bear = 0i32;

        for tf in [&self.m15, &self.m5] {
            if tf.completed.len() < 3 { continue; }
            let bars = &tf.completed;
            let last5 = &bars[bars.len().saturating_sub(5)..];

            // Count bullish vs bearish bars in window (majority vote)
            let bull_bars = last5.iter().filter(|c| c.close > c.open).count();
            let bear_bars = last5.len() - bull_bars;
            // 4+ out of 5 bars in same direction = strong signal
            if bull_bars >= 4 { bull += 1; }
            if bear_bars >= 4 { bear += 1; }

            // Count consecutive bullish/bearish from most recent
            let mut consec_bull = 0u32;
            let mut consec_bear = 0u32;
            for c in last5.iter().rev() {
                if c.close > c.open { consec_bull += 1; } else { break; }
            }
            for c in last5.iter().rev() {
                if c.close < c.open { consec_bear += 1; } else { break; }
            }
            if consec_bull >= 3 { bull += 1; }
            if consec_bear >= 3 { bear += 1; }

            // Large candle detection: any bar in last 3 with range > 2x average
            if last5.len() >= 3 {
                let avg_range: f64 = last5.iter()
                    .map(|c| c.high - c.low)
                    .sum::<f64>() / last5.len() as f64;
                for c in last5.iter().rev().take(3) {
                    let r = c.high - c.low;
                    if r > avg_range * 2.0 && avg_range > 0.0 {
                        if c.close > c.open { bull += 1; } else { bear += 1; }
                        break; // count once per TF
                    }
                }
            }

            // Net movement: close of last bar vs open of first bar in window
            let net_move = last5.last().unwrap().close - last5.first().unwrap().open;
            let total_range: f64 = last5.iter().map(|c| c.high - c.low).sum::<f64>();
            if total_range > 0.0 && net_move.abs() > total_range * 0.3 {
                if net_move > 0.0 { bull += 1; } else { bear += 1; }
            }
        }

        (bull, bear)
    }

    /// Helper: check forming pattern with completion-based confidence.
    /// Returns (is_bull_reversal, is_bear_reversal) only if completion >= min_pct.
    fn forming_signal(tf: &TimeframeState, min_pct: f64) -> (bool, bool) {
        if !tf.forming.is_active() || tf.forming.completion_pct() < min_pct || tf.forming.range_pips() < 1.0 {
            return (false, false);
        }
        (tf.forming_patterns.is_bullish_reversal(), tf.forming_patterns.is_bearish_reversal())
    }

    /// Calculate the pattern score based on the PDF framework.
    /// Includes completed patterns, forming patterns (with completion confidence),
    /// multi-candle patterns, session, and M1 momentum across all timeframes.
    /// Returns (score, direction): direction = 1 (long), -1 (short), 0 (no signal).
    pub fn calculate_score(&self) -> (i32, i32) {
        let mut score: i32 = 0;
        let mut bull_signals = 0i32;
        let mut bear_signals = 0i32;

        // ── M15 PATTERNS (0-4) — size-weighted ────────────────────
        {
            let (m15_bull, m15_bear) = self.m15.pattern_strength();
            if m15_bull > 0 { bull_signals += 1; score += m15_bull; }
            if m15_bear > 0 { bear_signals += 1; score += m15_bear; }

            // M15 forming pattern (>50% = counted, >70% = bonus)
            let (m15_fb, m15_fr) = Self::forming_signal(&self.m15, 0.5);
            if m15_fb { bull_signals += 1; score += 1; }
            else if m15_fr { bear_signals += 1; score += 1; }
            if self.m15.forming.completion_pct() > 0.7 {
                let (m15_fb70, m15_fr70) = Self::forming_signal(&self.m15, 0.7);
                if m15_fb70 { score += 1; }
                else if m15_fr70 { score += 1; }
            }
        }

        // ── M5 PATTERNS (0-4) — size-weighted ─────────────────────
        {
            let (m5_bull, m5_bear) = self.m5.pattern_strength();
            if m5_bull > 0 { bull_signals += 1; score += m5_bull; }
            if m5_bear > 0 { bear_signals += 1; score += m5_bear; }

            // M5 forming pattern (>60% = counted)
            let (m5_fb, m5_fr) = Self::forming_signal(&self.m5, 0.6);
            if m5_fb { bull_signals += 1; score += 1; }
            else if m5_fr { bear_signals += 1; score += 1; }
        }

        // ── SESSION (0-2) ────────────────────────────────────────────

        let hour = chrono::Utc::now().hour();
        if (8..12).contains(&hour) || (13..17).contains(&hour) {
            score += 2; // London or NY
        } else if (12..13).contains(&hour) {
            score += 1; // London/NY overlap
        }

        // ── DOM PRICE LEVEL STRUCTURE (−2 to +2) ─────────────────────
        // Retail DoM has mirrored volumes (OBI always 0), so we analyze:
        //   - Level density near price (more levels = more support)
        //   - Gaps in the book (price can move fast through gaps)
        //   - Level count changes (thinning = volatility incoming)
        if self.dom.active {
            // Density imbalance: more bid levels near price = support, ask = resistance
            let pattern_dir = if bull_signals > bear_signals { 1i32 }
                else if bear_signals > bull_signals { -1 }
                else { 0 };

            if self.dom.density_imbalance > 0.3 && pattern_dir > 0 {
                score += 1; // more bid density confirms bullish
            } else if self.dom.density_imbalance < -0.3 && pattern_dir < 0 {
                score += 1; // more ask density confirms bearish
            } else if self.dom.density_imbalance > 0.3 && pattern_dir < 0 {
                score -= 1; // bid density contradicts bearish
            } else if self.dom.density_imbalance < -0.3 && pattern_dir > 0 {
                score -= 1; // ask density contradicts bullish
            }

            // Levels thinning = low liquidity, risky
            if self.dom.levels_dropping { score -= 1; }

            // Large gaps on the side we're trading toward = price could accelerate
            if pattern_dir > 0 && !self.dom.ask_gaps.is_empty() {
                if self.dom.ask_gaps[0].gap_pips > 10.0 { score += 1; } // gap above = room to run up
            } else if pattern_dir < 0 && !self.dom.bid_gaps.is_empty() {
                if self.dom.bid_gaps[0].gap_pips > 10.0 { score += 1; } // gap below = room to run down
            }
        }

        // ── SWING / FIBONACCI / CHART PATTERN (−2 to +4) ────────────
        // Swing structure: trend direction from higher-highs/higher-lows
        // Fibonacci: pullback depth classification
        // Chart patterns: channels, triangles, wedges with breakout bias

        // M15 swing trend alignment
        if self.m15.swing_analysis.trend_direction == 1 {
            bull_signals += 1; score += 1;
        } else if self.m15.swing_analysis.trend_direction == -1 {
            bear_signals += 1; score += 1;
        }
        // M5 swing trend alignment (lighter weight)
        if self.m5.swing_analysis.trend_direction == 1 { bull_signals += 1; }
        else if self.m5.swing_analysis.trend_direction == -1 { bear_signals += 1; }

        // Fibonacci retracement — ideal pullback entry or reversal warning
        // Use M15 as primary, M5 as secondary
        for (tf, weight) in [(&self.m15, 2i32), (&self.m5, 1i32)] {
            if !tf.fib_levels.active { continue; }
            let near_fib = tf.fib_levels.nearest_fib_pips < 3.0; // within 3 pips of a fib level
            match tf.fib_levels.retracement_class {
                RetracementClass::NormalPullback if near_fib => {
                    // Price at 38.2-61.8% retracement near fib level = ideal pullback entry
                    if tf.swing_analysis.trend_direction == 1 {
                        bull_signals += 1; score += weight;
                    } else if tf.swing_analysis.trend_direction == -1 {
                        bear_signals += 1; score += weight;
                    }
                }
                RetracementClass::DeepPullback => {
                    // Trend weakening — discount trend-following by 1
                    if tf.swing_analysis.trend_direction == 1 && bull_signals > 0 {
                        score -= 1;
                    } else if tf.swing_analysis.trend_direction == -1 && bear_signals > 0 {
                        score -= 1;
                    }
                }
                RetracementClass::Reversal => {
                    // Trend likely broken — flip bias
                    if tf.swing_analysis.trend_direction == 1 {
                        bear_signals += 1; score += 1;
                        if bull_signals > 0 { bull_signals -= 1; score -= 1; }
                    } else if tf.swing_analysis.trend_direction == -1 {
                        bull_signals += 1; score += 1;
                        if bear_signals > 0 { bear_signals -= 1; score -= 1; }
                    }
                }
                _ => {}
            }
        }

        // Chart pattern breakout bias
        if let Some(ref pat) = self.m15.chart_pattern.detected {
            let bias = pat.pattern_type.breakout_bias();
            let pts = if pat.confidence > 0.7 { 2 } else { 1 };
            if bias > 0 { bull_signals += 1; score += pts; }
            else if bias < 0 { bear_signals += 1; score += pts; }
        }
        if let Some(ref pat) = self.m5.chart_pattern.detected {
            let bias = pat.pattern_type.breakout_bias();
            if bias > 0 { bull_signals += 1; }
            else if bias < 0 { bear_signals += 1; }
        }

        // ── M1 MOMENTUM (0-3) ───────────────────────────────────────

        let (momentum, higher_lows, lower_highs) = self.m1_momentum();
        // Basic momentum alignment
        if momentum >= 2 { bull_signals += 1; score += 1; }
        else if momentum <= -2 { bear_signals += 1; score += 1; }
        // Structure confirmation (higher lows for bull, lower highs for bear)
        if higher_lows { bull_signals += 1; score += 1; }
        if lower_highs { bear_signals += 1; score += 1; }
        // Strong M1 confirmation: momentum + structure aligned with higher TF direction
        if momentum == 3 && higher_lows && bull_signals > bear_signals { score += 1; }
        else if momentum == -3 && lower_highs && bear_signals > bull_signals { score += 1; }

        // ── COMPLETED BAR TREND CONTEXT (−2 to +4) ────────────────
        // Prevents false reversal signals after strong directional moves.
        // Looks at the last 5 completed bars on each timeframe for:
        //   - Consecutive bullish/bearish bars (trend strength)
        //   - Large candles (strong momentum)
        //   - Net direction across all timeframes

        let (trend_bull, trend_bear) = self.completed_bar_trend();

        // Strong trend should dominate over single-candle reversal patterns.
        // A single red candle in a strong uptrend shouldn't flip signal to SHORT.
        if trend_bull >= 4 {
            bull_signals += 3; score += 3;
            // Discount opposing signals heavily (likely noise/pullback)
            let discount = bear_signals.min(2);
            bear_signals -= discount; score -= discount;
        } else if trend_bull >= 3 {
            bull_signals += 2; score += 2;
            if bear_signals > 0 { bear_signals -= 1; score -= 1; }
        } else if trend_bull >= 2 {
            bull_signals += 1; score += 1;
        }

        if trend_bear >= 4 {
            bear_signals += 3; score += 3;
            let discount = bull_signals.min(2);
            bull_signals -= discount; score -= discount;
        } else if trend_bear >= 3 {
            bear_signals += 2; score += 2;
            if bull_signals > 0 { bull_signals -= 1; score -= 1; }
        } else if trend_bear >= 2 {
            bear_signals += 1; score += 1;
        }

        // ── DETERMINE DIRECTION ──────────────────────────────────────

        let direction = if bull_signals > bear_signals && bull_signals >= 2 {
            1 // LONG
        } else if bear_signals > bull_signals && bear_signals >= 2 {
            -1 // SHORT
        } else {
            0 // NO SIGNAL (conflicting or weak)
        };

        // If no clear direction, reduce score
        if direction == 0 {
            score = score.min(3);
        }

        (score, direction)
    }

    /// Generate a multi-line status display for the UI.
    pub fn status_display(&self) -> Vec<String> {
        let mut lines = Vec::new();

        lines.push(self.m15.status_line());
        lines.push(self.m5.status_line());

        // DoM
        lines.push(self.dom.status_line());

        // Trend context from completed bars
        let (trend_bull, trend_bear) = self.completed_bar_trend();
        let trend_str = if trend_bull > trend_bear && trend_bull >= 2 { format!("BULL({})", trend_bull) }
            else if trend_bear > trend_bull && trend_bear >= 2 { format!("BEAR({})", trend_bear) }
            else if trend_bull > 0 || trend_bear > 0 { format!("mixed(b{}:s{})", trend_bull, trend_bear) }
            else { "flat".to_string() };
        lines.push(format!("Trend: {} | M1: mom={} hl={} lh={}",
            trend_str,
            self.m1_momentum().0, self.m1_momentum().1, self.m1_momentum().2));

        // Overall score
        let (score, direction) = self.calculate_score();
        let dir_str = match direction {
            1 => "LONG",
            -1 => "SHORT",
            _ => "NEUTRAL",
        };
        let strength = if score >= 10 { "STRONG" }
            else if score >= 7 { "VALID" }
            else if score >= 4 { "WEAK" }
            else { "NONE" };

        lines.push(format!("Score: {}/13 {} {} signal", score, dir_str, strength));

        lines
    }

    /// Check if anything interesting is happening that warrants Claude analysis.
    /// Returns true if there's a potential setup worth analyzing.
    pub fn has_interesting_signal(&self) -> bool {
        // Any pattern forming > 50% on M15?
        if self.m15.forming.is_active() && self.m15.forming.completion_pct() > 0.5 {
            if self.m15.forming_patterns.is_bullish_reversal() || self.m15.forming_patterns.is_bearish_reversal() {
                return true;
            }
        }

        // Any completed reversal or multi-candle pattern on M5/M15?
        if self.m5.last_single.is_bullish_reversal() || self.m5.last_single.is_bearish_reversal() { return true; }
        if self.m5.last_multi.is_bullish_signal() || self.m5.last_multi.is_bearish_signal() { return true; }
        if self.m15.last_single.is_bullish_reversal() || self.m15.last_single.is_bearish_reversal() { return true; }
        if self.m15.last_multi.is_bullish_signal() || self.m15.last_multi.is_bearish_signal() { return true; }

        // Price near a Fibonacci level (within 3 pips) on M15?
        if self.m15.fib_levels.active && self.m15.fib_levels.nearest_fib_pips < 3.0 {
            return true;
        }

        // Chart pattern detected with decent confidence on M15?
        if let Some(ref pat) = self.m15.chart_pattern.detected {
            if pat.confidence > 0.6 { return true; }
        }

        // Fib retracement at reversal level on M15? (trend breaking)
        if self.m15.fib_levels.retracement_class == RetracementClass::Reversal {
            return true;
        }

        false
    }

    /// Build raw OHLCV data description of one timeframe for Claude prompt.
    /// No pattern labels, no trend analysis — let Claude interpret the raw data.
    fn tf_description(tf: &TimeframeState) -> String {
        let mut lines = Vec::new();
        lines.push(format!("{}:", tf.name));

        // Forming candle (raw OHLCV only)
        if tf.forming.is_active() {
            let pct = (tf.forming.completion_pct() * 100.0) as u32;
            lines.push(format!("  FORMING ({}% complete): O={:.5} H={:.5} L={:.5} C={:.5} V={:.0}",
                pct, tf.forming.open, tf.forming.high, tf.forming.low, tf.forming.close,
                tf.forming.volume));
        }

        // Last completed bars (raw OHLCV only — all 20)
        let n_bars = tf.completed.len();
        if n_bars > 0 {
            lines.push(format!("  COMPLETED BARS (oldest→newest):"));
            let start = 0;
            for (i, c) in tf.completed[start..].iter().enumerate() {
                lines.push(format!("    {}: O={:.5} H={:.5} L={:.5} C={:.5} V={:.0}",
                    i + 1, c.open, c.high, c.low, c.close, c.volume));
            }
        }

        // M1 bars inside forming M5 (raw OHLCV only)
        if tf.name == "M5" && !tf.m1_inside.is_empty() {
            lines.push(format!("  M1 BARS INSIDE FORMING M5:"));
            for (i, c) in tf.m1_inside.iter().enumerate() {
                lines.push(format!("    m{}: O={:.5} H={:.5} L={:.5} C={:.5}",
                    i + 1, c.open, c.high, c.low, c.close));
            }
        }

        // Swing points
        if !tf.swing_analysis.swing_highs.is_empty() || !tf.swing_analysis.swing_lows.is_empty() {
            let highs: Vec<String> = tf.swing_analysis.swing_highs.iter().map(|s| format!("{:.5}", s.price)).collect();
            let lows: Vec<String> = tf.swing_analysis.swing_lows.iter().map(|s| format!("{:.5}", s.price)).collect();
            let trend = match tf.swing_analysis.trend_direction {
                1 => "uptrend(HH+HL)", -1 => "downtrend(LH+LL)", _ => "mixed",
            };
            lines.push(format!("  SWING POINTS: highs=[{}] lows=[{}] trend={}",
                highs.join(", "), lows.join(", "), trend));
        }

        // Fibonacci levels
        let fib_section = tf.fib_levels.to_prompt_section(tf.name);
        if !fib_section.is_empty() {
            lines.push(fib_section);
        }

        // Chart pattern
        let chart_section = tf.chart_pattern.to_prompt_section();
        if !chart_section.is_empty() {
            lines.push(chart_section);
        }

        lines.join("\n")
    }

    /// Build the Claude CLI prompt for pattern analysis.
    /// Sends raw OHLCV data only — no pattern labels, no trend analysis.
    /// Let Claude interpret the data independently.
    pub fn build_claude_prompt(&self, news_lines: &[String], ec_lines: &[String], model_predictions: &str) -> String {
        // Last 20 M1 bars as raw OHLC
        let m1_bars = self.m1_recent.iter()
            .enumerate()
            .map(|(i, c)| format!("    {}: O={:.5} H={:.5} L={:.5} C={:.5}", i + 1, c.open, c.high, c.low, c.close))
            .collect::<Vec<_>>()
            .join("\n");
        let m1_section = if m1_bars.is_empty() {
            "M1 (last 20 bars):\n  No data yet".to_string()
        } else {
            format!("M1 (last {} bars):\n{}", self.m1_recent.len(), m1_bars)
        };

        let dom_section = self.dom.to_prompt_section();

        // News section
        let news_section = if news_lines.is_empty() {
            "NEWS TODAY:\n  No news data available".to_string()
        } else {
            format!("NEWS TODAY ({} articles):\n{}", news_lines.len(),
                news_lines.iter().map(|l| format!("  {}", l)).collect::<Vec<_>>().join("\n"))
        };

        // EC Calendar section
        let ec_section = if ec_lines.is_empty() {
            "ECONOMIC CALENDAR TODAY:\n  No events".to_string()
        } else {
            format!("ECONOMIC CALENDAR TODAY:\n{}",
                ec_lines.iter().map(|l| format!("  {}", l)).collect::<Vec<_>>().join("\n"))
        };

        // ML Model predictions section
        let model_section = if model_predictions.is_empty() {
            "ML MODELS:\n  Not available".to_string()
        } else {
            format!("ML MODELS (probability of profitable trade):\n{}", model_predictions)
        };

        let now = chrono::Utc::now();
        let time_str = now.format("%H:%M UTC").to_string();

        format!(
r#"You are an expert EUR/USD forex trader. Analyze the raw OHLCV candlestick data, Depth of Market, News, Economic Calendar, and ML model predictions below. Identify patterns, trend structure, support/resistance levels, news-driven moves, and entry opportunities.

IMPORTANT: If a major news event just occurred and price made a large move, this is a NEWS-DRIVEN breakout. Do not dismiss it due to session quality. News moves can continue and extend significantly.

The ML models provide independent probability estimates. Consider them as additional signals — they are NOT authoritative. Weight them alongside your own analysis of the raw data.

Time: {}. Your output will be used as input for a trading decision system.

{}

{}

{}

{}

{}

{}

{}

Based on all data (price action + news + EC calendar + DoM + ML models), analyze and output ONLY valid JSON:

{{
  "m15_bias": "<bullish/bearish/neutral>",
  "m15_pattern": "<main pattern you identify, or 'none'>",
  "m5_bias": "<bullish/bearish/neutral>",
  "m5_pattern": "<main pattern you identify, or 'none'>",
  "timeframe_conflict": <true/false>,
  "conflict_detail": "<which timeframes disagree, or 'none'>",
  "dominant_bias": "<bullish/bearish/neutral>",
  "dominant_bias_confidence": <0.0-1.0>,
  "news_driven": <true/false>,
  "news_impact": "<brief description of which news is driving price, or 'none'>",
  "session_quality": "<good/moderate/poor>",
  "forming_candle_signal": "<what the forming candles suggest, or 'none'>",
  "m1_entry_ready": <true/false>,
  "recommended_action": "<enter_long/enter_short/wait/no_trade>",
  "entry_timeframe": "<M15/M5/none>",
  "entry_condition": "<specific condition to enter, or why not>",
  "key_resistance": <price level or 0>,
  "key_support": <price level or 0>,
  "target_pips": <number or 0>,
  "stop_pips": <number or 0>,
  "invalidation": "<what cancels this assessment>"
}}"#,
            time_str,
            Self::tf_description(&self.m15),
            Self::tf_description(&self.m5),
            m1_section,
            dom_section,
            news_section,
            ec_section,
            model_section,
        )
    }
}

// ── Claude CLI integration ───────────────────────────────────────────────────

/// Structured response from Claude CLI pattern analysis.
/// This format is designed to be consumed by the main trading decision prompt.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct ClaudePatternResponse {
    pub m15_bias: Option<String>,
    pub m15_pattern: Option<String>,
    pub m5_bias: Option<String>,
    pub m5_pattern: Option<String>,
    pub timeframe_conflict: Option<bool>,
    pub conflict_detail: Option<String>,
    pub dominant_bias: Option<String>,
    pub dominant_bias_confidence: Option<f64>,
    pub news_driven: Option<bool>,
    pub news_impact: Option<String>,
    pub session_quality: Option<String>,
    pub forming_candle_signal: Option<String>,
    pub m1_entry_ready: Option<bool>,
    pub recommended_action: Option<String>,
    pub entry_timeframe: Option<String>,
    pub entry_condition: Option<String>,
    pub key_resistance: Option<f64>,
    pub key_support: Option<f64>,
    pub target_pips: Option<f64>,
    pub stop_pips: Option<f64>,
    pub invalidation: Option<String>,
}

impl ClaudePatternResponse {
    /// Format for UI display.
    pub fn display_summary(&self) -> String {
        let bias = self.dominant_bias.as_deref().unwrap_or("?");
        let conf = self.dominant_bias_confidence.unwrap_or(0.0);
        let action = self.recommended_action.as_deref().unwrap_or("?");
        let session = self.session_quality.as_deref().unwrap_or("?");
        let conflict = self.timeframe_conflict.unwrap_or(false);

        let mut lines = Vec::new();

        // Header
        let news_flag = if self.news_driven.unwrap_or(false) { " | NEWS-DRIVEN" } else { "" };
        lines.push(format!("Bias: {} ({:.0}%) | Action: {} | Session: {}{}",
            bias.to_uppercase(), conf * 100.0, action, session, news_flag));

        // News impact
        if self.news_driven.unwrap_or(false) {
            if let Some(ref impact) = self.news_impact {
                if impact != "none" {
                    lines.push(format!("News: {}", impact));
                }
            }
        }

        // Per-timeframe
        lines.push(format!("M15: {} [{}]",
            self.m15_bias.as_deref().unwrap_or("?"),
            self.m15_pattern.as_deref().unwrap_or("none")));
        lines.push(format!("M5: {} [{}]",
            self.m5_bias.as_deref().unwrap_or("?"),
            self.m5_pattern.as_deref().unwrap_or("none")));

        // Conflict
        if conflict {
            lines.push(format!("CONFLICT: {}",
                self.conflict_detail.as_deref().unwrap_or("?")));
        }

        // Forming signal
        if let Some(ref sig) = self.forming_candle_signal {
            if sig != "none" {
                lines.push(format!("Forming: {}", sig));
            }
        }

        // Entry
        if let Some(ref cond) = self.entry_condition {
            lines.push(format!("Entry: {}", cond));
        }

        // Levels
        let res = self.key_resistance.unwrap_or(0.0);
        let sup = self.key_support.unwrap_or(0.0);
        if res > 0.0 || sup > 0.0 {
            lines.push(format!("Levels: R={:.5} S={:.5}", res, sup));
        }

        // Target/Stop
        let tp = self.target_pips.unwrap_or(0.0);
        let sl = self.stop_pips.unwrap_or(0.0);
        if tp > 0.0 || sl > 0.0 {
            lines.push(format!("Target: {}p | Stop: {}p", tp, sl));
        }

        // Invalidation
        if let Some(ref inv) = self.invalidation {
            lines.push(format!("Invalid: {}", inv));
        }

        lines.join("\n")
    }
}

/// Call Claude CLI with a pattern analysis prompt.
/// Runs synchronously (blocking) — call from a dedicated thread.
/// Returns the parsed response or an error string.
pub fn call_claude_pattern_analysis(prompt: &str) -> Result<ClaudePatternResponse, String> {
    // Pipe prompt via stdin to Claude CLI (avoids Windows arg escaping issues)
    use std::io::Write;
    let mut child = std::process::Command::new("C:/Users/kushn/AppData/Roaming/npm/claude.cmd")
        .args(["-p", "-", "--output-format", "text"])
        .env("CLAUDE_CODE_MAX_TURNS", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn claude CLI: {}", e))?;

    // Write prompt to stdin
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(prompt.as_bytes());
        // stdin drops here, closing the pipe
    }

    let output = child.wait_with_output()
        .map_err(|e| format!("Failed to wait for claude CLI: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Claude CLI error: {}", stderr));
    }

    let response_text = String::from_utf8_lossy(&output.stdout).to_string();

    // Find JSON in the response (Claude may add text around it)
    let json_str = response_text.trim();
    let json_str = if let Some(start) = json_str.find('{') {
        if let Some(end) = json_str.rfind('}') {
            &json_str[start..=end]
        } else {
            json_str
        }
    } else {
        json_str
    };

    serde_json::from_str(json_str)
        .map_err(|e| format!("JSON parse error: {} — response: {}", e, &response_text[..response_text.len().min(300)]))
}

// ── Helper: convert cTrader trendbar period to minutes ───────────────────────

pub fn trendbar_period_to_minutes(period: i32) -> u32 {
    match period {
        1 => 1,      // M1
        2 => 2,      // M2
        3 => 3,      // M3
        4 => 4,      // M4
        5 => 5,      // M5
        6 => 10,     // M10
        7 => 15,     // M15
        8 => 30,     // M30
        9 => 60,     // H1
        10 => 240,   // H4
        11 => 720,   // H12
        12 => 1440,  // D1
        _ => 0,
    }
}

/// Convert cTrader trendbar delta-encoded OHLCV to an OhlcCandle.
/// cTrader encodes: low is absolute, open/close/high are deltas from low.
/// Prices are in 1/100000 units.
pub fn decode_trendbar(
    low: i64,
    delta_open: u64,
    delta_close: u64,
    delta_high: u64,
    volume: i64,
    timestamp_minutes: u32,
) -> OhlcCandle {
    let low_f = low as f64 / 100_000.0;
    let open_f = (low as f64 + delta_open as f64) / 100_000.0;
    let close_f = (low as f64 + delta_close as f64) / 100_000.0;
    let high_f = (low as f64 + delta_high as f64) / 100_000.0;
    let ts = timestamp_minutes as i64 * 60;
    OhlcCandle::new(open_f, high_f, low_f, close_f, volume as f64, ts)
}

use chrono::Timelike;
