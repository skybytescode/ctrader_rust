//! Real-time multi-timeframe candlestick pattern detection engine.
//!
//! Uses `candlestick-rs` for pattern recognition on completed and forming candles
//! across M5, M15, H1, and H4 timeframes simultaneously.
//!
//! M1 candles are the raw input — they build forming candles on higher timeframes
//! and provide entry timing signals. Pattern detection runs on M5 and above.

use candlestick_rs::{CandleStick, CandleStream};

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
            let fp = self.forming_patterns.summary();
            format!("Forming({}%): {}", pct, fp)
        } else {
            "Waiting...".to_string()
        };

        let completed_str = format!("Last: {} | Multi: {}",
            self.last_single.summary(),
            self.last_multi.summary()
        );

        format!("{}: {} | {}", self.name, forming_str, completed_str)
    }
}

// ── Pattern Engine (all timeframes) ──────────────────────────────────────────

/// The main pattern detection engine. Holds state for all timeframes.
pub struct PatternEngine {
    pub m5: TimeframeState,
    pub m15: TimeframeState,
    pub h1: TimeframeState,
    pub h4: TimeframeState,
    /// Latest M1 candle data (for entry timing).
    pub last_m1: Option<OhlcCandle>,
    /// Last 5 M1 candles (for momentum micro-read).
    m1_recent: Vec<OhlcCandle>,
}

impl PatternEngine {
    pub fn new() -> Self {
        Self {
            m5: TimeframeState::new("M5", 5),
            m15: TimeframeState::new("M15", 15),
            h1: TimeframeState::new("H1", 60),
            h4: TimeframeState::new("H4", 240),
            last_m1: None,
            m1_recent: Vec::with_capacity(10),
        }
    }

    /// Record a completed M1 candle. Used for M5 internal structure and entry timing.
    pub fn push_m1(&mut self, m1: OhlcCandle) {
        self.last_m1 = Some(m1);
        self.m1_recent.push(m1);
        if self.m1_recent.len() > 5 {
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
            60 => { self.h1.push_completed(candle); }
            240 => { self.h4.push_completed(candle); }
            _ => {}
        }
    }

    /// Update the forming candle from a LiveTrendbar update.
    pub fn update_forming(&mut self, period_minutes: u32, open: f64, high: f64, low: f64, close: f64, volume: f64, timestamp: i64) {
        match period_minutes {
            5 => self.m5.update_forming(open, high, low, close, volume, timestamp),
            15 => self.m15.update_forming(open, high, low, close, volume, timestamp),
            60 => self.h1.update_forming(open, high, low, close, volume, timestamp),
            240 => self.h4.update_forming(open, high, low, close, volume, timestamp),
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

        // ── H4 CONTEXT (0-2) ────────────────────────────────────────

        // H4 completed bar trend
        if self.h4.last_single.is_bullish { bull_signals += 1; score += 1; }
        else if self.h4.last_single.is_bearish { bear_signals += 1; score += 1; }

        // H4 multi-candle pattern (3BlackCrows, 3WhiteSoldiers, etc.)
        if self.h4.last_multi.is_bullish_signal() { bull_signals += 1; score += 1; }
        else if self.h4.last_multi.is_bearish_signal() { bear_signals += 1; score += 1; }

        // H4 forming pattern (only if >40% complete)
        let (h4_fb, h4_fr) = Self::forming_signal(&self.h4, 0.4);
        if h4_fb { bull_signals += 1; score += 1; }
        else if h4_fr { bear_signals += 1; score += 1; }

        // ── H1 PATTERNS (0-3) ───────────────────────────────────────

        // H1 completed reversal pattern
        if self.h1.last_single.is_bullish_reversal() { bull_signals += 1; score += 1; }
        else if self.h1.last_single.is_bearish_reversal() { bear_signals += 1; score += 1; }

        // H1 multi-candle pattern
        if self.h1.last_multi.is_bullish_signal() { bull_signals += 1; score += 1; }
        else if self.h1.last_multi.is_bearish_signal() { bear_signals += 1; score += 1; }

        // H1 forming pattern (>50% = +1, >75% = +1 extra)
        let (h1_fb, h1_fr) = Self::forming_signal(&self.h1, 0.5);
        if h1_fb { bull_signals += 1; score += 1; }
        else if h1_fr { bear_signals += 1; score += 1; }
        if self.h1.forming.completion_pct() > 0.75 {
            let (h1_fb75, h1_fr75) = Self::forming_signal(&self.h1, 0.75);
            if h1_fb75 { score += 1; } // bonus for high-confidence forming
            else if h1_fr75 { score += 1; }
        }

        // ── M15 PATTERNS (0-3) ──────────────────────────────────────

        // M15 completed reversal or multi-candle
        if self.m15.last_single.is_bullish_reversal() || self.m15.last_multi.is_bullish_signal() {
            bull_signals += 1; score += 1;
        } else if self.m15.last_single.is_bearish_reversal() || self.m15.last_multi.is_bearish_signal() {
            bear_signals += 1; score += 1;
        }

        // M15 forming pattern (>50% = counted, >70% = bonus)
        let (m15_fb, m15_fr) = Self::forming_signal(&self.m15, 0.5);
        if m15_fb { bull_signals += 1; score += 1; }
        else if m15_fr { bear_signals += 1; score += 1; }
        if self.m15.forming.completion_pct() > 0.7 {
            let (m15_fb70, m15_fr70) = Self::forming_signal(&self.m15, 0.7);
            if m15_fb70 { score += 1; }
            else if m15_fr70 { score += 1; }
        }

        // ── M5 PATTERNS (0-2) ───────────────────────────────────────

        // M5 completed reversal or multi-candle
        if self.m5.last_single.is_bullish_reversal() || self.m5.last_multi.is_bullish_signal() {
            bull_signals += 1; score += 1;
        } else if self.m5.last_single.is_bearish_reversal() || self.m5.last_multi.is_bearish_signal() {
            bear_signals += 1; score += 1;
        }

        // M5 forming pattern (>60% = counted)
        let (m5_fb, m5_fr) = Self::forming_signal(&self.m5, 0.6);
        if m5_fb { bull_signals += 1; score += 1; }
        else if m5_fr { bear_signals += 1; score += 1; }

        // ── SESSION (0-2) ────────────────────────────────────────────

        let hour = chrono::Utc::now().hour();
        if (8..12).contains(&hour) || (13..17).contains(&hour) {
            score += 2; // London or NY
        } else if (12..13).contains(&hour) {
            score += 1; // London/NY overlap
        }

        // ── M1 MOMENTUM (0-1) ───────────────────────────────────────

        let (momentum, higher_lows, lower_highs) = self.m1_momentum();
        if momentum >= 2 && higher_lows { bull_signals += 1; score += 1; }
        else if momentum <= -2 && lower_highs { bear_signals += 1; score += 1; }

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

        lines.push(self.h4.status_line());
        lines.push(self.h1.status_line());
        lines.push(self.m15.status_line());
        lines.push(self.m5.status_line());

        // M1 momentum
        let (momentum, hl, lh) = self.m1_momentum();
        let m1_str = format!("M1: momentum={} higher_lows={} lower_highs={}",
            momentum, hl, lh);
        lines.push(m1_str);

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
        // Any pattern forming > 50% on H4/H1/M15?
        if self.h4.forming.is_active() && self.h4.forming.completion_pct() > 0.5 {
            if self.h4.forming_patterns.is_bullish_reversal() || self.h4.forming_patterns.is_bearish_reversal() {
                return true;
            }
        }
        if self.h1.forming.is_active() && self.h1.forming.completion_pct() > 0.5 {
            if self.h1.forming_patterns.is_bullish_reversal() || self.h1.forming_patterns.is_bearish_reversal() {
                return true;
            }
        }
        if self.m15.forming.is_active() && self.m15.forming.completion_pct() > 0.5 {
            if self.m15.forming_patterns.is_bullish_reversal() || self.m15.forming_patterns.is_bearish_reversal() {
                return true;
            }
        }

        // Any completed reversal or multi-candle pattern on M5/M15/H1?
        if self.m5.last_single.is_bullish_reversal() || self.m5.last_single.is_bearish_reversal() { return true; }
        if self.m5.last_multi.is_bullish_signal() || self.m5.last_multi.is_bearish_signal() { return true; }
        if self.m15.last_single.is_bullish_reversal() || self.m15.last_single.is_bearish_reversal() { return true; }
        if self.m15.last_multi.is_bullish_signal() || self.m15.last_multi.is_bearish_signal() { return true; }
        if self.h1.last_single.is_bullish_reversal() || self.h1.last_single.is_bearish_reversal() { return true; }
        if self.h1.last_multi.is_bullish_signal() || self.h1.last_multi.is_bearish_signal() { return true; }

        false
    }

    /// Build a detailed description of one timeframe for the prompt.
    fn tf_description(tf: &TimeframeState) -> String {
        let forming = if tf.forming.is_active() {
            let pct = (tf.forming.completion_pct() * 100.0) as u32;
            format!(
                "Forming ({}% complete): O={:.5} H={:.5} L={:.5} C={:.5} range={:.1}pips\n  Forming pattern: {}",
                pct, tf.forming.open, tf.forming.high, tf.forming.low, tf.forming.close,
                tf.forming.range_pips(),
                tf.forming_patterns.summary()
            )
        } else {
            "No forming data yet".to_string()
        };

        let completed = format!("Last completed: {}", tf.last_single.summary());
        let multi = format!("Multi-candle: {}", tf.last_multi.summary());

        // Last completed bars directions
        let bar_dirs: String = tf.completed.iter()
            .rev()
            .take(5)
            .map(|c| if c.close > c.open { "Bull" } else { "Bear" })
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join(", ");
        let history = if bar_dirs.is_empty() {
            "No history yet".to_string()
        } else {
            format!("Last bars: {}", bar_dirs)
        };

        // Internal structure (for M5 only — has M1 inside)
        let internal = if tf.name == "M5" && !tf.m1_inside.is_empty() {
            let (bull, bear, hl, lh) = tf.internal_structure();
            format!("\n  M1 inside: {} bull, {} bear, higher_lows={}, lower_highs={}", bull, bear, hl, lh)
        } else {
            String::new()
        };

        format!("{}:\n  {}\n  {}\n  {}\n  {}{}", tf.name, forming, completed, multi, history, internal)
    }

    /// Build the Claude CLI prompt for pattern analysis.
    /// No pre-calculated score — let Claude form its own opinion from raw data.
    pub fn build_claude_prompt(&self) -> String {
        let (m1_mom, m1_hl, m1_lh) = self.m1_momentum();

        let now = chrono::Utc::now();
        let time_str = now.format("%H:%M UTC").to_string();
        let session = {
            let h = now.hour();
            if (8..12).contains(&h) { "London" }
            else if (12..13).contains(&h) { "London/NY overlap" }
            else if (13..17).contains(&h) { "New York" }
            else if (17..21).contains(&h) { "Late NY" }
            else { "Asian/Off-hours" }
        };

        format!(
r#"You are an expert EUR/USD forex trader analyzing candlestick patterns in real-time.
Time: {} ({} session).

Analyze the raw pattern data below. Your output will be used as input for a trading decision system that also considers spread, DoM, news, economic calendar, and ML model data.

{}

{}

{}

{}

M1 ENTRY TIMING:
  Momentum: {} ({})
  Higher lows: {}
  Lower highs: {}

Analyze the patterns independently. Output a structured assessment in this EXACT JSON format, no other text:

{{
  "h4_bias": "<bullish/bearish/neutral>",
  "h4_strength": "<strong/moderate/weak>",
  "h4_pattern": "<main pattern or 'none'>",
  "h1_bias": "<bullish/bearish/neutral>",
  "h1_strength": "<strong/moderate/weak>",
  "h1_pattern": "<main pattern or 'none'>",
  "m15_bias": "<bullish/bearish/neutral>",
  "m15_pattern": "<main pattern or 'none'>",
  "m5_bias": "<bullish/bearish/neutral>",
  "m5_pattern": "<main pattern or 'none'>",
  "timeframe_conflict": <true/false>,
  "conflict_detail": "<which timeframes disagree, or 'none'>",
  "dominant_bias": "<bullish/bearish/neutral>",
  "dominant_bias_confidence": <0.0-1.0>,
  "session_quality": "<good/moderate/poor>",
  "forming_candle_signal": "<strongest forming pattern and timeframe, or 'none'>",
  "m1_entry_ready": <true/false>,
  "recommended_action": "<enter_long/enter_short/wait/no_trade>",
  "entry_timeframe": "<H1/M15/M5/none>",
  "entry_condition": "<specific condition to enter, or why not>",
  "key_resistance": <price level or 0>,
  "key_support": <price level or 0>,
  "target_pips": <number or 0>,
  "stop_pips": <number or 0>,
  "invalidation": "<what cancels this assessment>"
}}"#,
            time_str, session,
            Self::tf_description(&self.h4),
            Self::tf_description(&self.h1),
            Self::tf_description(&self.m15),
            Self::tf_description(&self.m5),
            m1_mom,
            if m1_mom > 0 { "bullish" } else if m1_mom < 0 { "bearish" } else { "neutral" },
            m1_hl, m1_lh,
        )
    }
}

// ── Claude CLI integration ───────────────────────────────────────────────────

/// Structured response from Claude CLI pattern analysis.
/// This format is designed to be consumed by the main trading decision prompt.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct ClaudePatternResponse {
    pub h4_bias: Option<String>,
    pub h4_strength: Option<String>,
    pub h4_pattern: Option<String>,
    pub h1_bias: Option<String>,
    pub h1_strength: Option<String>,
    pub h1_pattern: Option<String>,
    pub m15_bias: Option<String>,
    pub m15_pattern: Option<String>,
    pub m5_bias: Option<String>,
    pub m5_pattern: Option<String>,
    pub timeframe_conflict: Option<bool>,
    pub conflict_detail: Option<String>,
    pub dominant_bias: Option<String>,
    pub dominant_bias_confidence: Option<f64>,
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
        lines.push(format!("Bias: {} ({:.0}%) | Action: {} | Session: {}",
            bias.to_uppercase(), conf * 100.0, action, session));

        // Per-timeframe
        lines.push(format!("H4: {} {} [{}]",
            self.h4_bias.as_deref().unwrap_or("?"),
            self.h4_strength.as_deref().unwrap_or(""),
            self.h4_pattern.as_deref().unwrap_or("none")));
        lines.push(format!("H1: {} {} [{}]",
            self.h1_bias.as_deref().unwrap_or("?"),
            self.h1_strength.as_deref().unwrap_or(""),
            self.h1_pattern.as_deref().unwrap_or("none")));
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
