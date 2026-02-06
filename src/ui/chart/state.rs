use bevy::prelude::*;
use std::collections::HashMap;
use crate::db::Candle;

/// Timeframe for candlestick chart
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Timeframe {
    M1,     // 1 minute
    M5,     // 5 minutes
    M15,    // 15 minutes
    #[default]
    H1,     // 1 hour
    H4,     // 4 hours
    D1,     // Daily
}

impl Timeframe {
    pub fn as_str(&self) -> &'static str {
        match self {
            Timeframe::M1 => "1m",
            Timeframe::M5 => "5m",
            Timeframe::M15 => "15m",
            Timeframe::H1 => "1H",
            Timeframe::H4 => "4H",
            Timeframe::D1 => "1D",
        }
    }

    pub fn all() -> &'static [Timeframe] {
        &[
            Timeframe::M1,
            Timeframe::M5,
            Timeframe::M15,
            Timeframe::H1,
            Timeframe::H4,
            Timeframe::D1,
        ]
    }

    /// Get the duration in seconds for this timeframe
    pub fn seconds(&self) -> i64 {
        match self {
            Timeframe::M1 => 60,
            Timeframe::M5 => 300,
            Timeframe::M15 => 900,
            Timeframe::H1 => 3600,
            Timeframe::H4 => 14400,
            Timeframe::D1 => 86400,
        }
    }
}

/// Tick direction for coloring the live price
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum TickDirection {
    #[default]
    Up,      // Price went up - green
    Down,    // Price went down - orange
}

/// Data for a single instrument
#[derive(Debug, Clone)]
pub struct InstrumentData {
    pub symbol: String,
    pub bid: f64,
    pub ask: f64,
    pub prev_bid: f64,
    pub prev_ask: f64,
    pub open: f64,
    pub decimal_places: u8,
    pub candles: HashMap<Timeframe, Vec<Candle>>,
    pub tick_direction: TickDirection,  // Direction of last price change
    pub trades_weekends: bool,          // True for crypto (24/7), false for forex (skip Sat/Sun)
}

impl InstrumentData {
    pub fn new(symbol: &str, decimal_places: u8, trades_weekends: bool) -> Self {
        Self {
            symbol: symbol.to_string(),
            bid: 0.0,
            ask: 0.0,
            prev_bid: 0.0,
            prev_ask: 0.0,
            open: 0.0,
            decimal_places,
            candles: HashMap::new(),
            tick_direction: TickDirection::Up,
            trades_weekends,
        }
    }

    /// Get the mid-price (average of bid and ask)
    pub fn mid_price(&self) -> f64 {
        (self.bid + self.ask) / 2.0
    }

    /// Get the previous mid-price
    pub fn prev_mid_price(&self) -> f64 {
        (self.prev_bid + self.prev_ask) / 2.0
    }

    /// Update prices from a tick
    pub fn update_price(&mut self, bid: f64, ask: f64) {
        self.prev_bid = self.bid;
        self.prev_ask = self.ask;
        self.bid = bid;
        self.ask = ask;

        // Update tick direction based on mid-price change
        let current_mid = self.mid_price();
        let prev_mid = self.prev_mid_price();

        if current_mid > prev_mid {
            self.tick_direction = TickDirection::Up;
        } else if current_mid < prev_mid {
            self.tick_direction = TickDirection::Down;
        }
        // If equal, keep the previous direction (no change)

        if self.open == 0.0 {
            self.open = bid;
        }
    }

    /// Auto-detect if this instrument trades on weekends by checking candle data
    /// Only counts "true weekend" candles (Saturday 04:00-23:59 and Sunday 00:00-16:00 UTC)
    /// to avoid false positives from Friday close / Sunday open edge cases
    pub fn detect_trades_weekends(&mut self) {
        use chrono::{Datelike, TimeZone, Timelike, Utc};

        let mut total_checked = 0usize;
        let mut true_weekend_count = 0usize;

        // Check all candles from all timeframes for better accuracy
        for candles in self.candles.values() {
            for candle in candles.iter() {
                if let Some(dt) = Utc.timestamp_opt(candle.timestamp, 0).single() {
                    total_checked += 1;
                    let weekday = dt.weekday();
                    let hour = dt.hour();

                    // Only count "true weekend" candles - middle of weekend, not edge cases
                    // Forex typically closes Friday ~22:00 UTC and opens Sunday ~22:00 UTC
                    // So we exclude:
                    // - Saturday 00:00-03:59 (could be Friday close spillover)
                    // - Sunday 17:00-23:59 (could be Sunday open)
                    let is_true_weekend = match weekday {
                        chrono::Weekday::Sat => hour >= 4,  // Saturday from 04:00 onwards
                        chrono::Weekday::Sun => hour < 17,  // Sunday until 17:00
                        _ => false,
                    };

                    if is_true_weekend {
                        true_weekend_count += 1;
                    }
                }
            }
        }

        // Crypto markets trade 24/7, so should have significant true weekend candles (~20%)
        // Forex should have ~0% true weekend candles
        // Use 10% threshold for true weekend candles
        let weekend_percentage = if total_checked > 0 {
            (true_weekend_count as f64 / total_checked as f64) * 100.0
        } else {
            0.0
        };

        // Log for debugging
        println!("  Weekend detection: {}/{} TRUE weekend candles ({:.1}%)",
            true_weekend_count, total_checked, weekend_percentage);

        // Crypto should have ~20%+ true weekend candles, forex should have <5%
        self.trades_weekends = weekend_percentage >= 10.0;
    }
}

/// Request to load more historical data
#[derive(Debug, Clone)]
pub struct LoadMoreDataRequest {
    pub symbol: String,
    pub timeframe: Timeframe,
    pub before_timestamp: i64,
    pub count: usize,
}

/// Chart UI state
#[derive(Resource)]
pub struct ChartState {
    pub selected_instrument: Option<String>,
    pub selected_timeframe: Timeframe,
    pub zoom_level: f64,           // Horizontal zoom (time axis)
    pub vertical_zoom: f64,        // Vertical zoom (price axis) - 1.0 = auto-fit
    pub price_center: Option<f64>, // Center price for vertical zoom (None = auto-center)
    pub pan_offset: i64,
    pub show_volume: bool,
    pub show_grid: bool,
    pub crosshair_enabled: bool,
    pub crosshair_pos: Option<(f32, f32)>,
    pub hovered_candle_idx: Option<usize>,
    // Scrollbar state
    pub scroll_position: f32,  // 0.0 = newest (right), 1.0 = oldest (left)
    pub total_candles_in_db: HashMap<String, HashMap<Timeframe, usize>>,
    pub all_data_loaded: HashMap<String, HashMap<Timeframe, bool>>,
    pub load_more_request: Option<LoadMoreDataRequest>,
    pub is_loading: bool,
}

impl Default for ChartState {
    fn default() -> Self {
        Self {
            selected_instrument: None,
            selected_timeframe: Timeframe::H4,
            zoom_level: 1.0,
            vertical_zoom: 1.0,
            price_center: None,
            pan_offset: 0,
            show_volume: true,
            show_grid: true,
            crosshair_enabled: true,
            crosshair_pos: None,
            hovered_candle_idx: None,
            scroll_position: 0.0,
            total_candles_in_db: HashMap::new(),
            all_data_loaded: HashMap::new(),
            load_more_request: None,
            is_loading: false,
        }
    }
}

impl ChartState {
    /// Calculate total time slots from candle timestamps (includes gaps)
    pub fn calculate_total_slots(candles: &[crate::db::Candle], interval: i64) -> usize {
        if candles.is_empty() || interval == 0 {
            return 0;
        }
        let first_ts = candles.first().map(|c| c.timestamp).unwrap_or(0);
        let last_ts = candles.last().map(|c| c.timestamp).unwrap_or(0);
        if first_ts == 0 || last_ts < first_ts {
            return candles.len();
        }
        ((last_ts - first_ts) / interval) as usize + 1
    }

    /// Calculate visible candle range based on zoom and pan
    /// Returns (start_idx, end_idx, future_candle_slots)
    /// - start_idx, end_idx: range of candles to draw (from the candles array)
    /// - future_candle_slots: how many empty candle slots to show on the right (for future space)
    pub fn visible_range(&self, total_candles: usize, chart_width: f32, candle_width: f32) -> (usize, usize, usize) {
        // For timestamp-based rendering, always return all candles
        // The viewport will handle visibility based on time range
        if total_candles == 0 {
            return (0, 0, 0);
        }

        // candle_width is already zoom-adjusted, so just divide
        let visible_count = (chart_width / candle_width) as usize;
        let _visible_count = visible_count.max(10);

        // For future space (panning right past the current candle)
        let future_slots = if self.pan_offset < 0 {
            (-self.pan_offset) as usize
        } else {
            0
        };

        // Return all candles - timestamp-based rendering handles positioning
        (0, total_candles, future_slots)
    }

    /// Get the visible slot count for calculating time range
    /// Note: candle_width is already zoom-adjusted (base_width / zoom_level)
    pub fn visible_slot_count(&self, chart_width: f32, candle_width: f32) -> usize {
        // candle_width is already adjusted for zoom, so just divide
        let visible_count = (chart_width / candle_width) as usize;
        visible_count.max(10)
    }

    /// Zoom in (decrease zoom_level to show fewer candles bigger)
    pub fn zoom_in(&mut self) {
        self.zoom_level = (self.zoom_level * 0.9).max(0.3);
    }

    /// Zoom out (increase zoom_level to show more candles smaller)
    pub fn zoom_out(&mut self) {
        self.zoom_level = (self.zoom_level * 1.1).min(5.0);
    }

    /// Pan left (show older candles)
    pub fn pan_left(&mut self, amount: i64) {
        self.pan_offset += amount;
    }

    /// Pan right (show newer candles / show future space)
    /// Allows negative pan_offset to show empty "future" space on the right
    pub fn pan_right(&mut self, amount: i64, max_future_slots: i64) {
        // Allow panning into future, but limit how far
        self.pan_offset = (self.pan_offset - amount).max(-max_future_slots);
    }

    /// Adjust vertical zoom (price axis scaling)
    pub fn adjust_vertical_zoom(&mut self, delta: f64) {
        // Allow much higher vertical zoom for detailed price analysis (up to 20x)
        self.vertical_zoom = (self.vertical_zoom * (1.0 + delta)).clamp(0.1, 20.0);
    }

    /// Reset vertical zoom to auto-fit
    pub fn reset_vertical_zoom(&mut self) {
        self.vertical_zoom = 1.0;
        self.price_center = None;
    }

    /// Reset all zoom and pan to defaults
    pub fn reset_view(&mut self) {
        self.zoom_level = 1.0;
        self.vertical_zoom = 1.0;
        self.price_center = None;
        self.pan_offset = 0;
    }

    /// Check if we need to load more data (user scrolled to left edge)
    pub fn check_load_more(&mut self, symbol: &str, timeframe: Timeframe, current_candles: usize, oldest_timestamp: i64) {
        // Don't request if already loading or all data loaded
        if self.is_loading || self.load_more_request.is_some() {
            return;
        }

        // Check if all data is already loaded for this symbol/timeframe
        if let Some(tf_map) = self.all_data_loaded.get(symbol) {
            if let Some(true) = tf_map.get(&timeframe) {
                return;
            }
        }

        // Calculate if we're near the left edge (older data)
        let visible_count = (100.0 / self.zoom_level) as usize;
        let threshold = visible_count.min(50); // Load when within 50 candles of edge

        if self.pan_offset as usize + visible_count + threshold >= current_candles {
            // Request more data
            self.load_more_request = Some(LoadMoreDataRequest {
                symbol: symbol.to_string(),
                timeframe,
                before_timestamp: oldest_timestamp,
                count: 500, // Load 500 more candles
            });
        }
    }

    /// Mark that all data has been loaded for a symbol/timeframe
    pub fn mark_all_data_loaded(&mut self, symbol: &str, timeframe: Timeframe) {
        self.all_data_loaded
            .entry(symbol.to_string())
            .or_insert_with(HashMap::new)
            .insert(timeframe, true);
    }

    /// Update total candles count for a symbol/timeframe
    pub fn set_total_candles(&mut self, symbol: &str, timeframe: Timeframe, count: usize) {
        self.total_candles_in_db
            .entry(symbol.to_string())
            .or_insert_with(HashMap::new)
            .insert(timeframe, count);
    }

    /// Get the scroll ratio (0 = newest, 1 = oldest)
    pub fn get_scroll_ratio(&self, loaded_candles: usize, total_candles: usize) -> f32 {
        if total_candles == 0 || loaded_candles == 0 {
            return 0.0;
        }
        let visible = (100.0 / self.zoom_level) as usize;
        let max_offset = loaded_candles.saturating_sub(visible);
        if max_offset == 0 {
            return 0.0;
        }
        (self.pan_offset as f32 / max_offset as f32).clamp(0.0, 1.0)
    }
}

/// Professional color scheme for trading charts (cTrader style)
pub mod colors {
    use bevy_egui::egui::Color32;

    // cTrader-style candlestick colors
    pub const BULLISH: Color32 = Color32::from_rgb(0, 166, 115);       // Teal green
    pub const BEARISH: Color32 = Color32::from_rgb(235, 115, 20);      // Orange

    // Chart elements
    pub const GRID: Color32 = Color32::from_rgb(38, 41, 51);           // Subtle dark grid
    pub const CROSSHAIR: Color32 = Color32::from_rgb(128, 128, 140);   // Medium gray
    pub const AXIS_TEXT: Color32 = Color32::from_rgb(153, 158, 166);   // Muted text color
    pub const BACKGROUND: Color32 = Color32::from_rgb(15, 18, 23);     // Very dark background
    pub const TOOLTIP_BG: Color32 = Color32::from_rgb(25, 28, 35);     // Tooltip background
    pub const AXIS_BG: Color32 = Color32::from_rgb(20, 22, 28);        // Axis background

    // Live price colors
    pub const LIVE_PRICE: Color32 = Color32::from_rgb(242, 128, 25);   // Orange for live price
    pub const LIVE_PRICE_TEXT: Color32 = Color32::from_rgb(255, 255, 255); // White text on price label

    // Alpha colors for volume bars
    pub fn bullish_alpha() -> Color32 {
        Color32::from_rgba_unmultiplied(0, 166, 115, 128)
    }

    pub fn bearish_alpha() -> Color32 {
        Color32::from_rgba_unmultiplied(235, 115, 20, 128)
    }
}
