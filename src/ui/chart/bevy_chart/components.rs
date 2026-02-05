//! Bevy ECS components for native chart rendering
//!
//! This module defines components for GPU-accelerated candlestick chart rendering
//! using Bevy's native 2D rendering system instead of egui's immediate mode.

use bevy::prelude::*;

/// Marker component for the chart camera
#[derive(Component)]
pub struct ChartCamera;

/// Marker component for chart-related entities (for easy cleanup)
#[derive(Component)]
pub struct ChartEntity;

/// Component for candlestick entities
#[derive(Component, Clone)]
pub struct CandleComponent {
    pub index: usize,
    pub timestamp: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: i64,
    pub is_bullish: bool,
}

impl CandleComponent {
    pub fn new(index: usize, timestamp: i64, open: f64, high: f64, low: f64, close: f64, volume: i64) -> Self {
        Self {
            index,
            timestamp,
            open,
            high,
            low,
            close,
            volume,
            is_bullish: close >= open,
        }
    }
}

/// Component for candlestick body (the rectangle part)
#[derive(Component)]
pub struct CandleBody {
    pub candle_index: usize,
}

/// Component for candlestick wick (the line part)
#[derive(Component)]
pub struct CandleWick {
    pub candle_index: usize,
}

/// Component for volume bars
#[derive(Component)]
pub struct VolumeBar {
    pub candle_index: usize,
    pub volume: i64,
}

/// Component for horizontal grid lines
#[derive(Component)]
pub struct GridLineH {
    pub price: f64,
}

/// Component for vertical grid lines
#[derive(Component)]
pub struct GridLineV {
    pub timestamp: i64,
}

/// Component for the live price line
#[derive(Component)]
pub struct LivePriceLine {
    pub price: f64,
}

/// Component for the crosshair
#[derive(Component)]
pub struct CrosshairLine {
    pub is_horizontal: bool,
}

/// Component for the "NOW" vertical line when showing future space
#[derive(Component)]
pub struct NowLine;

/// Component for day separator vertical line (yellow)
#[derive(Component)]
pub struct DaySeparator {
    pub timestamp: i64,
}

/// Component for week separator vertical lines (double gold)
#[derive(Component)]
pub struct WeekSeparator {
    pub timestamp: i64,
}

/// Component for price axis labels
#[derive(Component)]
pub struct PriceAxisLabel {
    pub price: f64,
}

/// Component for time axis labels
#[derive(Component)]
pub struct TimeAxisLabel {
    pub timestamp: i64,
}

/// Chart viewport information - stores the pixel coordinates and transformation data
#[derive(Resource, Clone)]
pub struct ChartViewport {
    /// Screen position of chart area (top-left corner)
    pub position: Vec2,
    /// Size of chart area in pixels
    pub size: Vec2,
    /// Minimum visible price
    pub price_min: f64,
    /// Maximum visible price
    pub price_max: f64,
    /// First visible candle timestamp
    pub time_start: i64,
    /// Last visible candle timestamp
    pub time_end: i64,
    /// Width of each candle in world units
    pub candle_width: f32,
    /// Number of future slots (empty space on right)
    pub future_slots: usize,
    /// Decimal places for price display
    pub decimal_places: u8,
    /// Timeframe interval in seconds (for timestamp-based positioning)
    pub timeframe_interval: i64,
    /// Total number of time slots in visible range (including gaps)
    pub total_slots: usize,
}

impl Default for ChartViewport {
    fn default() -> Self {
        Self {
            position: Vec2::ZERO,
            size: Vec2::new(800.0, 400.0),
            price_min: 0.0,
            price_max: 100.0,
            time_start: 0,
            time_end: 0,
            candle_width: 12.0,
            future_slots: 0,
            decimal_places: 5,
            timeframe_interval: 14400, // Default to H4 (4 hours)
            total_slots: 0,
        }
    }
}

impl ChartViewport {
    /// Convert price to Y coordinate in world space
    pub fn price_to_y(&self, price: f64) -> f32 {
        let range = self.price_max - self.price_min;
        if range == 0.0 {
            return 0.0;
        }
        let ratio = (price - self.price_min) / range;
        // Y increases upward in Bevy's coordinate system
        (ratio as f32 - 0.5) * self.size.y
    }

    /// Convert Y coordinate to price
    pub fn y_to_price(&self, y: f32) -> f64 {
        let ratio = (y / self.size.y) + 0.5;
        self.price_min + (ratio as f64 * (self.price_max - self.price_min))
    }

    /// Convert candle index to X coordinate in world space
    pub fn index_to_x(&self, index: usize) -> f32 {
        let x = (index as f32 * self.candle_width) - (self.size.x / 2.0);
        x + self.candle_width / 2.0
    }

    /// Convert timestamp to X coordinate in world space (handles gaps)
    pub fn timestamp_to_x(&self, timestamp: i64) -> f32 {
        if self.timeframe_interval == 0 || self.time_start == 0 {
            return 0.0;
        }
        // Calculate slot index based on timestamp
        let slot_index = (timestamp - self.time_start) / self.timeframe_interval;
        self.index_to_x(slot_index as usize)
    }

    /// Convert slot index (time-based) to timestamp
    pub fn slot_to_timestamp(&self, slot: usize) -> i64 {
        self.time_start + (slot as i64 * self.timeframe_interval)
    }

    /// Check if a price is within visible range
    pub fn is_price_visible(&self, price: f64) -> bool {
        price >= self.price_min && price <= self.price_max
    }
}

/// Bundle for spawning a complete candlestick (body + wick)
#[derive(Bundle)]
pub struct CandlestickBundle {
    pub candle: CandleComponent,
    pub chart_entity: ChartEntity,
    pub transform: Transform,
    pub global_transform: GlobalTransform,
    pub visibility: Visibility,
    pub inherited_visibility: InheritedVisibility,
    pub view_visibility: ViewVisibility,
}

impl CandlestickBundle {
    pub fn new(candle: CandleComponent, position: Vec3) -> Self {
        Self {
            candle,
            chart_entity: ChartEntity,
            transform: Transform::from_translation(position),
            global_transform: GlobalTransform::default(),
            visibility: Visibility::Visible,
            inherited_visibility: InheritedVisibility::default(),
            view_visibility: ViewVisibility::default(),
        }
    }
}

/// Professional color scheme for trading charts (cTrader style)
pub mod chart_colors {
    use bevy::prelude::*;

    // cTrader-style candlestick colors
    pub const BULLISH: Color = Color::srgb(0.0, 0.65, 0.45);           // Teal green
    pub const BEARISH: Color = Color::srgb(0.92, 0.45, 0.08);          // Orange
    pub const BULLISH_ALPHA: Color = Color::srgba(0.0, 0.65, 0.45, 0.5);
    pub const BEARISH_ALPHA: Color = Color::srgba(0.92, 0.45, 0.08, 0.5);

    // Chart elements
    pub const GRID: Color = Color::srgb(0.15, 0.16, 0.20);             // Subtle dark grid
    pub const CROSSHAIR: Color = Color::srgb(0.5, 0.5, 0.55);          // Medium gray
    pub const BACKGROUND: Color = Color::srgb(0.06, 0.07, 0.09);       // Very dark background

    // Time separators - solid colors for visibility
    pub const DAY_SEPARATOR: Color = Color::srgb(0.7, 0.7, 0.15);        // Yellow for day boundaries
    pub const WEEK_SEPARATOR: Color = Color::srgb(0.85, 0.65, 0.1);      // Gold for week boundaries

    // Live price line (orange like cTrader)
    pub const LIVE_PRICE: Color = Color::srgb(0.95, 0.50, 0.10);       // Orange for live price
    pub const NOW_LINE: Color = Color::srgb(0.4, 0.6, 0.9);            // Blue for NOW line
}
