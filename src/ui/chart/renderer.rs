use bevy_egui::egui::{self, Color32, Painter, Pos2, Rect, Response, Sense, Stroke, Vec2, pos2};
use crate::db::Candle;
use super::state::{ChartState, colors};
use chrono::{DateTime, Utc, Timelike};

/// Response from chart rendering
pub struct ChartResponse {
    pub response: Response,
    pub hovered_candle: Option<usize>,
    pub price_axis_dragged: Option<f32>,  // Y delta when dragging price axis
    pub time_axis_dragged: Option<f32>,   // X delta when dragging time axis
}

/// Price scale helper for converting prices to Y coordinates
struct PriceScale {
    min_price: f64,
    max_price: f64,
    chart_top: f32,
    chart_bottom: f32,
}

impl PriceScale {
    /// Create with vertical zoom applied
    fn with_vertical_zoom(
        min_price: f64,
        max_price: f64,
        chart_top: f32,
        chart_bottom: f32,
        vertical_zoom: f64,
        price_center: Option<f64>,
    ) -> Self {
        let range = max_price - min_price;
        let padding = range * 0.05;
        let base_min = min_price - padding;
        let base_max = max_price + padding;
        let base_range = base_max - base_min;

        // Apply vertical zoom: smaller range = more zoomed in
        let zoomed_range = base_range / vertical_zoom;

        // Determine center point
        let center = price_center.unwrap_or((base_min + base_max) / 2.0);

        // Calculate new min/max around center
        let new_min = center - zoomed_range / 2.0;
        let new_max = center + zoomed_range / 2.0;

        Self {
            min_price: new_min,
            max_price: new_max,
            chart_top,
            chart_bottom,
        }
    }

    fn y(&self, price: f64) -> f32 {
        let ratio = (price - self.min_price) / (self.max_price - self.min_price);
        self.chart_bottom - (ratio as f32 * (self.chart_bottom - self.chart_top))
    }

    fn price_at_y(&self, y: f32) -> f64 {
        let ratio = (self.chart_bottom - y) / (self.chart_bottom - self.chart_top);
        self.min_price + (ratio as f64 * (self.max_price - self.min_price))
    }
}

/// Render the complete candlestick chart
pub fn render_candlestick_chart(
    ui: &mut egui::Ui,
    candles: &[Candle],
    state: &ChartState,
    decimal_places: u8,
    current_bid: Option<f64>,
) -> ChartResponse {
    let available_size = ui.available_size();

    // Layout constants
    let price_axis_width = 80.0;
    let time_axis_height = 30.0;
    let volume_height = available_size.y * 0.15; // 15% for volume

    // We need to create separate interactive regions for chart, price axis, and time axis
    // First, allocate the full space
    let full_rect = ui.available_rect_before_wrap();

    let chart_rect = Rect::from_min_max(
        pos2(full_rect.min.x, full_rect.min.y),
        pos2(full_rect.max.x - price_axis_width, full_rect.max.y - time_axis_height - volume_height),
    );

    let volume_rect = Rect::from_min_max(
        pos2(full_rect.min.x, chart_rect.max.y),
        pos2(full_rect.max.x - price_axis_width, full_rect.max.y - time_axis_height),
    );

    let price_axis_rect = Rect::from_min_max(
        pos2(chart_rect.max.x, full_rect.min.y),
        pos2(full_rect.max.x, full_rect.max.y - time_axis_height),
    );

    let time_axis_rect = Rect::from_min_max(
        pos2(full_rect.min.x, full_rect.max.y - time_axis_height),
        pos2(full_rect.max.x - price_axis_width, full_rect.max.y),
    );

    // Create interactive responses for each region
    let chart_response = ui.allocate_rect(chart_rect, Sense::click_and_drag());
    let _volume_response = ui.allocate_rect(volume_rect, Sense::hover());
    let price_axis_response = ui.allocate_rect(price_axis_rect, Sense::click_and_drag());
    let time_axis_response = ui.allocate_rect(time_axis_rect, Sense::click_and_drag());

    // Get the painter for the full area
    let painter = ui.painter_at(full_rect);

    // Track axis drag deltas
    let mut price_axis_dragged: Option<f32> = None;
    let mut time_axis_dragged: Option<f32> = None;

    // Handle price axis dragging (vertical zoom)
    if price_axis_response.dragged() {
        let delta = price_axis_response.drag_delta();
        price_axis_dragged = Some(delta.y);
    }

    // Handle time axis dragging (horizontal zoom)
    if time_axis_response.dragged() {
        let delta = time_axis_response.drag_delta();
        time_axis_dragged = Some(delta.x);
    }

    // Change cursor when hovering over axes
    if price_axis_response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    if time_axis_response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }

    // Draw background
    painter.rect_filled(full_rect, 0.0, colors::BACKGROUND);

    if candles.is_empty() {
        // Show placeholder when no data
        painter.text(
            full_rect.center(),
            egui::Align2::CENTER_CENTER,
            "No candle data available",
            egui::FontId::proportional(16.0),
            colors::AXIS_TEXT,
        );
        return ChartResponse {
            response: chart_response,
            hovered_candle: None,
            price_axis_dragged,
            time_axis_dragged,
        };
    }

    // Calculate visible range (includes future slots for panning into future)
    let candle_base_width = 12.0;
    let candle_width = candle_base_width / state.zoom_level as f32;
    let (start_idx, end_idx, future_slots) = state.visible_range(candles.len(), chart_rect.width(), candle_width);
    let visible_candles = &candles[start_idx..end_idx];

    if visible_candles.is_empty() {
        return ChartResponse {
            response: chart_response,
            hovered_candle: None,
            price_axis_dragged,
            time_axis_dragged,
        };
    }

    // Calculate price range from visible candles
    let (min_price, max_price) = visible_candles.iter().fold(
        (f64::MAX, f64::MIN),
        |(min, max), c| (min.min(c.low), max.max(c.high)),
    );

    // Apply vertical zoom to price scale
    let price_scale = PriceScale::with_vertical_zoom(
        min_price,
        max_price,
        chart_rect.min.y,
        chart_rect.max.y,
        state.vertical_zoom,
        state.price_center,
    );

    // Calculate max volume for scaling
    let max_volume = visible_candles.iter().map(|c| c.volume).max().unwrap_or(1) as f64;

    // Draw grid if enabled
    if state.show_grid {
        draw_grid(&painter, &chart_rect, &price_scale, decimal_places);
    }

    // Draw candlesticks and volume
    let mut hovered_candle = None;
    let gap = 2.0;
    let body_width = (candle_width - gap).max(3.0);

    for (i, candle) in visible_candles.iter().enumerate() {
        let x = chart_rect.min.x + (i as f32 * candle_width) + gap / 2.0;

        // Draw candle
        draw_candle(&painter, candle, x, body_width, &price_scale);

        // Draw volume bar
        if state.show_volume {
            draw_volume_bar(&painter, candle, x, body_width, &volume_rect, max_volume);
        }

        // Check for hover
        let candle_hover_rect = Rect::from_min_max(
            pos2(x, chart_rect.min.y),
            pos2(x + body_width, chart_rect.max.y),
        );
        if let Some(hover_pos) = chart_response.hover_pos() {
            if candle_hover_rect.contains(hover_pos) {
                hovered_candle = Some(start_idx + i);
            }
        }
    }

    // Draw price axis (with highlight when being dragged)
    let price_axis_highlight = price_axis_response.dragged() || price_axis_response.hovered();
    draw_price_axis_interactive(&painter, &price_axis_rect, &price_scale, decimal_places, price_axis_highlight);

    // Draw time axis (with highlight when being dragged)
    let time_axis_highlight = time_axis_response.dragged() || time_axis_response.hovered();
    draw_time_axis_interactive(&painter, &time_axis_rect, visible_candles, candle_width, time_axis_highlight, future_slots);

    // Draw "now" line if showing future space (vertical line at the last candle position)
    if future_slots > 0 {
        let now_x = chart_rect.min.x + (visible_candles.len() as f32 * candle_width);
        // Dashed vertical line for "now"
        let now_color = Color32::from_rgb(100, 149, 237); // Cornflower blue
        let dash_length = 8.0;
        let gap_length = 4.0;
        let mut y = chart_rect.min.y;
        while y < chart_rect.max.y {
            let end_y = (y + dash_length).min(chart_rect.max.y);
            painter.line_segment(
                [pos2(now_x, y), pos2(now_x, end_y)],
                Stroke::new(1.5, now_color),
            );
            y += dash_length + gap_length;
        }
    }

    // Draw current live price line if available
    if let Some(bid) = current_bid {
        draw_live_price_line(&painter, bid, &chart_rect, &price_axis_rect, &price_scale, decimal_places);
    }

    // Draw crosshair if enabled and hovering
    if state.crosshair_enabled {
        if let Some(hover_pos) = chart_response.hover_pos() {
            if chart_rect.contains(hover_pos) {
                draw_crosshair(&painter, hover_pos, &chart_rect, &price_scale, decimal_places);
            }
        }
    }

    // Draw tooltip if hovering a candle
    if let Some(idx) = hovered_candle {
        let candle = &candles[idx];
        draw_tooltip(ui, &chart_response, candle, decimal_places);
    }

    // Draw separator line between chart and volume
    painter.line_segment(
        [pos2(chart_rect.min.x, chart_rect.max.y), pos2(chart_rect.max.x, chart_rect.max.y)],
        Stroke::new(1.0, colors::GRID),
    );

    ChartResponse {
        response: chart_response,
        hovered_candle,
        price_axis_dragged,
        time_axis_dragged,
    }
}

fn draw_candle(painter: &Painter, candle: &Candle, x: f32, width: f32, scale: &PriceScale) {
    let is_bullish = candle.close >= candle.open;
    let color = if is_bullish { colors::BULLISH } else { colors::BEARISH };

    let wick_x = x + width / 2.0;

    // Draw wick (high-low line)
    painter.line_segment(
        [pos2(wick_x, scale.y(candle.high)), pos2(wick_x, scale.y(candle.low))],
        Stroke::new(1.0, color),
    );

    // Draw body (open-close rect)
    let body_top = scale.y(candle.open.max(candle.close));
    let body_bottom = scale.y(candle.open.min(candle.close));
    let min_body_height = 1.0;

    let body_rect = Rect::from_min_max(
        pos2(x, body_top),
        pos2(x + width, body_bottom.max(body_top + min_body_height)),
    );

    if is_bullish {
        // Bullish: filled body
        painter.rect_filled(body_rect, 0.0, color);
    } else {
        // Bearish: filled body
        painter.rect_filled(body_rect, 0.0, color);
    }
}

fn draw_volume_bar(
    painter: &Painter,
    candle: &Candle,
    x: f32,
    width: f32,
    volume_rect: &Rect,
    max_volume: f64,
) {
    if max_volume <= 0.0 {
        return;
    }

    let is_bullish = candle.close >= candle.open;
    let color = if is_bullish { colors::bullish_alpha() } else { colors::bearish_alpha() };

    let height_ratio = (candle.volume as f64 / max_volume) as f32;
    let bar_height = height_ratio * volume_rect.height();

    let bar_rect = Rect::from_min_max(
        pos2(x, volume_rect.max.y - bar_height),
        pos2(x + width, volume_rect.max.y),
    );

    painter.rect_filled(bar_rect, 0.0, color);
}

fn draw_grid(painter: &Painter, chart_rect: &Rect, scale: &PriceScale, _decimal_places: u8) {
    let price_range = scale.max_price - scale.min_price;

    // Calculate nice grid intervals
    let target_lines = 6;
    let raw_interval = price_range / target_lines as f64;

    // Round to nice number
    let magnitude = 10f64.powf(raw_interval.log10().floor());
    let normalized = raw_interval / magnitude;
    let nice_interval = if normalized <= 1.5 {
        magnitude
    } else if normalized <= 3.0 {
        2.0 * magnitude
    } else if normalized <= 7.0 {
        5.0 * magnitude
    } else {
        10.0 * magnitude
    };

    // Draw horizontal grid lines
    let first_line = (scale.min_price / nice_interval).ceil() * nice_interval;
    let mut price = first_line;

    while price <= scale.max_price {
        let y = scale.y(price);
        painter.line_segment(
            [pos2(chart_rect.min.x, y), pos2(chart_rect.max.x, y)],
            Stroke::new(1.0, colors::GRID),
        );
        price += nice_interval;
    }
}

/// Draw price axis with optional highlight for interaction feedback
fn draw_price_axis_interactive(
    painter: &Painter,
    rect: &Rect,
    scale: &PriceScale,
    decimal_places: u8,
    highlighted: bool,
) {
    // Background for price axis (brighter when highlighted/dragging)
    let bg_color = if highlighted {
        Color32::from_rgb(38, 42, 58) // Slightly brighter when active
    } else {
        Color32::from_rgb(28, 32, 43)
    };
    painter.rect_filled(*rect, 0.0, bg_color);

    // Draw a subtle indicator line when highlighted
    if highlighted {
        painter.line_segment(
            [pos2(rect.min.x, rect.min.y), pos2(rect.min.x, rect.max.y)],
            Stroke::new(2.0, Color32::from_rgb(80, 120, 200)),
        );
    }

    let price_range = scale.max_price - scale.min_price;
    let target_labels = 6;
    let raw_interval = price_range / target_labels as f64;

    let magnitude = 10f64.powf(raw_interval.log10().floor());
    let normalized = raw_interval / magnitude;
    let nice_interval = if normalized <= 1.5 {
        magnitude
    } else if normalized <= 3.0 {
        2.0 * magnitude
    } else if normalized <= 7.0 {
        5.0 * magnitude
    } else {
        10.0 * magnitude
    };

    let first_label = (scale.min_price / nice_interval).ceil() * nice_interval;
    let mut price = first_label;

    let text_color = if highlighted {
        Color32::from_rgb(230, 235, 245)
    } else {
        colors::AXIS_TEXT
    };

    while price <= scale.max_price {
        let y = scale.y(price);
        let text = format!("{:.width$}", price, width = decimal_places as usize);

        painter.text(
            pos2(rect.min.x + 5.0, y),
            egui::Align2::LEFT_CENTER,
            text,
            egui::FontId::proportional(11.0),
            text_color,
        );

        price += nice_interval;
    }
}

/// Draw time axis with optional highlight for interaction feedback
fn draw_time_axis_interactive(
    painter: &Painter,
    rect: &Rect,
    candles: &[Candle],
    candle_width: f32,
    highlighted: bool,
    future_slots: usize,
) {
    // Background for time axis (brighter when highlighted/dragging)
    let bg_color = if highlighted {
        Color32::from_rgb(38, 42, 58) // Slightly brighter when active
    } else {
        Color32::from_rgb(28, 32, 43)
    };
    painter.rect_filled(*rect, 0.0, bg_color);

    // Draw a subtle indicator line when highlighted
    if highlighted {
        painter.line_segment(
            [pos2(rect.min.x, rect.min.y), pos2(rect.max.x, rect.min.y)],
            Stroke::new(2.0, Color32::from_rgb(80, 120, 200)),
        );
    }

    if candles.is_empty() {
        return;
    }

    // Draw time labels at regular intervals
    let label_interval = (60.0 / candle_width).max(1.0) as usize;

    let text_color = if highlighted {
        Color32::from_rgb(230, 235, 245)
    } else {
        colors::AXIS_TEXT
    };

    for (i, candle) in candles.iter().enumerate() {
        if i % label_interval != 0 {
            continue;
        }

        let x = rect.min.x + (i as f32 * candle_width) + candle_width / 2.0;
        if x > rect.max.x - 40.0 {
            break;
        }

        let dt: DateTime<Utc> = DateTime::from_timestamp(candle.timestamp, 0).unwrap_or_default();
        let text = format!("{:02}:{:02}", dt.hour(), dt.minute());

        painter.text(
            pos2(x, rect.center().y),
            egui::Align2::CENTER_CENTER,
            text,
            egui::FontId::proportional(10.0),
            text_color,
        );
    }

    // Draw "NOW" label in the future space
    if future_slots > 0 && !candles.is_empty() {
        let now_x = rect.min.x + (candles.len() as f32 * candle_width) + candle_width / 2.0;
        if now_x < rect.max.x - 20.0 {
            painter.text(
                pos2(now_x, rect.center().y),
                egui::Align2::CENTER_CENTER,
                "NOW",
                egui::FontId::proportional(9.0),
                Color32::from_rgb(100, 149, 237), // Cornflower blue
            );
        }
    }
}

fn draw_crosshair(
    painter: &Painter,
    pos: Pos2,
    chart_rect: &Rect,
    scale: &PriceScale,
    decimal_places: u8,
) {
    // Vertical line
    painter.line_segment(
        [pos2(pos.x, chart_rect.min.y), pos2(pos.x, chart_rect.max.y)],
        Stroke::new(1.0, colors::CROSSHAIR),
    );

    // Horizontal line
    painter.line_segment(
        [pos2(chart_rect.min.x, pos.y), pos2(chart_rect.max.x, pos.y)],
        Stroke::new(1.0, colors::CROSSHAIR),
    );

    // Price label on right
    let price = scale.price_at_y(pos.y);
    let label_rect = Rect::from_min_size(
        pos2(chart_rect.max.x + 2.0, pos.y - 10.0),
        Vec2::new(76.0, 20.0),
    );
    painter.rect_filled(label_rect, 2.0, colors::CROSSHAIR);

    let text = format!("{:.width$}", price, width = decimal_places as usize);
    painter.text(
        label_rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(11.0),
        Color32::WHITE,
    );
}

fn draw_tooltip(ui: &mut egui::Ui, _response: &Response, candle: &Candle, decimal_places: u8) {
    let dt: DateTime<Utc> = DateTime::from_timestamp(candle.timestamp, 0).unwrap_or_default();
    let is_bullish = candle.close >= candle.open;
    let change = candle.close - candle.open;
    let change_pct = if candle.open != 0.0 {
        (change / candle.open) * 100.0
    } else {
        0.0
    };

    let dp = decimal_places as usize;

    egui::show_tooltip(ui.ctx(), ui.layer_id(), egui::Id::new("candle_tooltip"), |ui| {
        ui.set_min_width(150.0);

        ui.label(egui::RichText::new(dt.format("%Y-%m-%d %H:%M").to_string())
            .color(colors::AXIS_TEXT)
            .size(11.0));

        ui.separator();

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("O:").color(colors::AXIS_TEXT));
            ui.label(egui::RichText::new(format!("{:.dp$}", candle.open)).strong());
        });
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("H:").color(colors::AXIS_TEXT));
            ui.label(egui::RichText::new(format!("{:.dp$}", candle.high)).strong());
        });
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("L:").color(colors::AXIS_TEXT));
            ui.label(egui::RichText::new(format!("{:.dp$}", candle.low)).strong());
        });
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("C:").color(colors::AXIS_TEXT));
            ui.label(egui::RichText::new(format!("{:.dp$}", candle.close)).strong());
        });
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Vol:").color(colors::AXIS_TEXT));
            ui.label(egui::RichText::new(format!("{}", candle.volume)).strong());
        });

        ui.separator();

        let color = if is_bullish { colors::BULLISH } else { colors::BEARISH };
        let sign = if change >= 0.0 { "+" } else { "" };
        ui.label(egui::RichText::new(format!("{}{:.dp$} ({}{:.2}%)", sign, change, sign, change_pct))
            .color(color)
            .strong());
    });
}

/// Draw a horizontal line showing the current live price
fn draw_live_price_line(
    painter: &Painter,
    price: f64,
    chart_rect: &Rect,
    price_axis_rect: &Rect,
    scale: &PriceScale,
    decimal_places: u8,
) {
    // Only draw if price is within visible range
    if price < scale.min_price || price > scale.max_price {
        return;
    }

    let y = scale.y(price);
    let live_color = Color32::from_rgb(255, 193, 7); // Amber/gold color for live price

    // Draw dashed line across the chart
    let dash_length = 5.0;
    let gap_length = 3.0;
    let mut x = chart_rect.min.x;
    while x < chart_rect.max.x {
        let end_x = (x + dash_length).min(chart_rect.max.x);
        painter.line_segment(
            [pos2(x, y), pos2(end_x, y)],
            Stroke::new(1.0, live_color),
        );
        x += dash_length + gap_length;
    }

    // Draw price label on the right side
    let label_width = 76.0;
    let label_height = 18.0;
    let label_rect = Rect::from_min_size(
        pos2(price_axis_rect.min.x + 2.0, y - label_height / 2.0),
        Vec2::new(label_width, label_height),
    );
    painter.rect_filled(label_rect, 2.0, live_color);

    let text = format!("{:.width$}", price, width = decimal_places as usize);
    painter.text(
        label_rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(11.0),
        Color32::BLACK,
    );
}

/// Render horizontal scrollbar for chart navigation
/// Returns true if user is scrolling towards older data (left edge)
pub fn render_scrollbar(
    ui: &mut egui::Ui,
    state: &mut ChartState,
    loaded_candles: usize,
    total_db_candles: usize,
    is_loading: bool,
) -> bool {
    let mut needs_more_data = false;
    let scrollbar_height = 16.0;

    ui.horizontal(|ui| {
        let available_width = ui.available_width() - 80.0; // Leave space for info

        // Calculate scroll ratio
        let visible_count = (100.0 / state.zoom_level) as usize;
        let max_offset = loaded_candles.saturating_sub(visible_count);

        // Scrollbar track
        let (response, painter) = ui.allocate_painter(
            Vec2::new(available_width, scrollbar_height),
            Sense::click_and_drag(),
        );
        let rect = response.rect;

        // Track background
        painter.rect_filled(
            rect,
            4.0,
            Color32::from_rgb(35, 39, 50),
        );

        // Calculate thumb position and size
        let thumb_ratio = if max_offset > 0 {
            (state.pan_offset as f32 / max_offset as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };

        // Thumb size based on visible portion
        let thumb_size_ratio = if loaded_candles > 0 {
            (visible_count as f32 / loaded_candles as f32).clamp(0.1, 1.0)
        } else {
            1.0
        };
        let thumb_width = (rect.width() * thumb_size_ratio).max(30.0);
        let max_thumb_x = rect.width() - thumb_width;

        // Thumb position (reversed: 0 offset = right side, max offset = left side)
        let thumb_x = rect.min.x + max_thumb_x * (1.0 - thumb_ratio);

        let thumb_rect = Rect::from_min_size(
            pos2(thumb_x, rect.min.y + 2.0),
            Vec2::new(thumb_width, scrollbar_height - 4.0),
        );

        // Thumb color (highlight when loading or hovering)
        let thumb_color = if is_loading {
            Color32::from_rgb(100, 100, 140) // Loading indicator
        } else if response.hovered() {
            Color32::from_rgb(80, 85, 100)
        } else {
            Color32::from_rgb(60, 65, 80)
        };

        painter.rect_filled(thumb_rect, 3.0, thumb_color);

        // Handle drag interaction
        if response.dragged() {
            let delta = response.drag_delta();
            // Reversed direction: drag left = show older data (increase offset)
            if max_offset > 0 {
                let scroll_delta = (-delta.x / max_thumb_x * max_offset as f32) as i64;
                state.pan_offset = (state.pan_offset + scroll_delta).clamp(0, max_offset as i64);
            }
        }

        // Handle click on track (jump to position)
        if response.clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                let click_ratio = 1.0 - ((pos.x - rect.min.x) / rect.width()).clamp(0.0, 1.0);
                state.pan_offset = (click_ratio * max_offset as f32) as i64;
            }
        }

        // Check if we need to load more data (near left edge = older data)
        if max_offset > 0 && state.pan_offset as usize >= max_offset.saturating_sub(10) {
            needs_more_data = true;
        }

        // Add left arrow button to load more data
        ui.add_space(5.0);
        if ui.add_enabled(!is_loading, egui::Button::new("◀◀").small())
            .on_hover_text("Load more historical data")
            .clicked()
        {
            needs_more_data = true;
        }

        // Show loaded/total info
        ui.add_space(5.0);
        let info_text = if total_db_candles > loaded_candles {
            format!("{}/{}", loaded_candles, total_db_candles)
        } else {
            format!("{}", loaded_candles)
        };
        ui.label(egui::RichText::new(info_text).size(10.0).color(colors::AXIS_TEXT));
    });

    needs_more_data
}
