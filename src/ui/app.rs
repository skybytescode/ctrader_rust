use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use chrono::{Datelike, Timelike};
use std::collections::HashMap;

use super::chart::{
    ChartState, InstrumentData, LoadMoreDataRequest, TickDirection,
    render_timeframe_selector, render_chart_controls,
    render_scrollbar, handle_chart_interaction, colors,
};
use super::chart::bevy_chart::ChartViewport;

#[derive(Resource)]
pub struct AppState {
    pub instruments: HashMap<String, InstrumentData>,
    pub connection_status: String,
}

impl Default for AppState {
    fn default() -> Self {
        let mut instruments = HashMap::new();
        instruments.insert("EURUSD".to_string(), InstrumentData::new("EURUSD", 5));
        instruments.insert("BTCUSD".to_string(), InstrumentData::new("BTCUSD", 2));

        Self {
            instruments,
            connection_status: "Init".to_string(),
        }
    }
}

impl AppState {
    /// Helper to get EURUSD data (for backward compatibility)
    pub fn eurusd(&self) -> Option<&InstrumentData> {
        self.instruments.get("EURUSD")
    }

    /// Helper to get BTCUSD data (for backward compatibility)
    pub fn btcusd(&self) -> Option<&InstrumentData> {
        self.instruments.get("BTCUSD")
    }

    /// Helper to get mutable EURUSD data
    pub fn eurusd_mut(&mut self) -> Option<&mut InstrumentData> {
        self.instruments.get_mut("EURUSD")
    }

    /// Helper to get mutable BTCUSD data
    pub fn btcusd_mut(&mut self) -> Option<&mut InstrumentData> {
        self.instruments.get_mut("BTCUSD")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum ActiveView {
    #[default]
    Indicators,
    News,
    Analysis,
}

#[derive(Resource)]
pub struct UiState {
    pub active_view: ActiveView,
    pub sidebar_expanded: bool,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            active_view: ActiveView::Indicators,
            sidebar_expanded: true,
        }
    }
}

/// Main UI system that renders the egui interface
pub fn ui_system(
    mut contexts: EguiContexts,
    app_state: Res<AppState>,
    mut ui_state: ResMut<UiState>,
    mut chart_state: ResMut<ChartState>,
    chart_viewport: Res<ChartViewport>,
) {
    let ctx = contexts.ctx_mut();

    // Extract viewport data for aligned price axis rendering and tooltip
    let viewport_price_range = (chart_viewport.price_min, chart_viewport.price_max);
    let viewport_chart_size = chart_viewport.size;
    let viewport_chart_pos = chart_viewport.position;
    let viewport_time_start = chart_viewport.time_start;

    // Top Panel
    egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.heading("cTrader Rust Terminal");
            ui.separator();
            ui.label(format!("Status: {}", app_state.connection_status));

            // Show selected instrument in header
            if let Some(ref symbol) = chart_state.selected_instrument {
                ui.separator();
                ui.label(egui::RichText::new(symbol).strong().color(colors::BULLISH));
            }
        });
    });

    // Narrow icon bar (leftmost)
    egui::SidePanel::left("icon_bar")
        .exact_width(50.0)
        .resizable(false)
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(10.0);

                // Indicators icon
                if ui.add_sized(
                    [40.0, 40.0],
                    egui::Button::new("📊")
                        .fill(if ui_state.active_view == ActiveView::Indicators {
                            egui::Color32::from_rgb(40, 40, 60)
                        } else {
                            egui::Color32::from_rgb(30, 30, 30)
                        })
                ).clicked() {
                    ui_state.active_view = ActiveView::Indicators;
                    ui_state.sidebar_expanded = !ui_state.sidebar_expanded;
                }
                ui.small("Indicators");

                ui.add_space(15.0);

                // News icon
                if ui.add_sized(
                    [40.0, 40.0],
                    egui::Button::new("📰")
                        .fill(if ui_state.active_view == ActiveView::News {
                            egui::Color32::from_rgb(40, 40, 60)
                        } else {
                            egui::Color32::from_rgb(30, 30, 30)
                        })
                ).clicked() {
                    ui_state.active_view = ActiveView::News;
                    ui_state.sidebar_expanded = true;
                }
                ui.small("News");

                ui.add_space(15.0);

                // Analysis icon
                if ui.add_sized(
                    [40.0, 40.0],
                    egui::Button::new("🔬")
                        .fill(if ui_state.active_view == ActiveView::Analysis {
                            egui::Color32::from_rgb(40, 40, 60)
                        } else {
                            egui::Color32::from_rgb(30, 30, 30)
                        })
                ).clicked() {
                    ui_state.active_view = ActiveView::Analysis;
                    ui_state.sidebar_expanded = true;
                }
                ui.small("Analysis");
            });
        });

    // Expandable sidebar (shows content based on active view)
    if ui_state.sidebar_expanded {
        egui::SidePanel::left("content_sidebar")
            .exact_width(280.0)
            .resizable(false)
            .show(ctx, |ui| {
                match ui_state.active_view {
                    ActiveView::Indicators => {
                        render_indicators_sidebar(ui, &app_state, &mut chart_state);
                    },
                    ActiveView::News => {
                        ui.heading("📰 News");
                        ui.separator();
                        ui.add_space(10.0);
                        ui.label("Latest financial news will appear here...");
                    },
                    ActiveView::Analysis => {
                        ui.heading("🔬 Analysis");
                        ui.separator();
                        ui.add_space(10.0);
                        ui.label("Market analysis tools will appear here...");
                    },
                }
            });
    }

    // Main Content Area (Central Panel) - Chart
    // Use a transparent frame so Bevy renders the chart underneath
    egui::CentralPanel::default()
        .frame(egui::Frame::none().fill(egui::Color32::TRANSPARENT))
        .show(ctx, |ui| {
            render_chart_panel(ui, &app_state, &mut chart_state, viewport_price_range, viewport_chart_size, viewport_chart_pos, viewport_time_start);
        });
}

/// Render the indicators sidebar with clickable instruments
fn render_indicators_sidebar(
    ui: &mut egui::Ui,
    app_state: &AppState,
    chart_state: &mut ChartState,
) {
    ui.vertical(|ui| {
        ui.heading("📊 Instruments");
        ui.separator();
        ui.add_space(10.0);

        // EUR/USD Section
        if let Some(eurusd) = app_state.eurusd() {
            let is_selected = chart_state.selected_instrument.as_deref() == Some("EURUSD");
            render_instrument_card(ui, eurusd, is_selected, chart_state);
        }

        ui.add_space(8.0);

        // BTC/USD Section
        if let Some(btcusd) = app_state.btcusd() {
            let is_selected = chart_state.selected_instrument.as_deref() == Some("BTCUSD");
            render_instrument_card(ui, btcusd, is_selected, chart_state);
        }

        ui.add_space(20.0);
        ui.separator();
        ui.add_space(10.0);

        ui.label(egui::RichText::new("Click an instrument to view chart")
            .size(11.0)
            .color(egui::Color32::from_rgb(120, 120, 120)));
    });
}

/// Render a single instrument card (clickable)
fn render_instrument_card(
    ui: &mut egui::Ui,
    instrument: &InstrumentData,
    is_selected: bool,
    chart_state: &mut ChartState,
) {
    let bg_color = if is_selected {
        egui::Color32::from_rgb(45, 50, 65)
    } else {
        egui::Color32::from_rgb(35, 39, 50)
    };

    egui::Frame::none()
        .fill(bg_color)
        .inner_margin(egui::Margin::same(8.0))
        .outer_margin(egui::Margin::same(2.0))
        .rounding(4.0)
        .stroke(egui::Stroke::new(
            if is_selected { 2.0 } else { 0.0 },
            colors::BULLISH,
        ))
        .show(ui, |ui| {
            let response = ui.interact(
                ui.available_rect_before_wrap(),
                ui.id().with(&instrument.symbol),
                egui::Sense::click(),
            );

            ui.horizontal(|ui| {
                // Selection indicator
                if is_selected {
                    ui.label(egui::RichText::new("▶").color(colors::BULLISH));
                } else {
                    ui.label(egui::RichText::new("○").color(egui::Color32::from_rgb(80, 80, 80)));
                }

                ui.label(egui::RichText::new(&instrument.symbol).strong().size(14.0));
            });

            ui.add_space(5.0);

            // Price colors based on change
            let bid_color = if instrument.bid > instrument.prev_bid {
                colors::BULLISH
            } else {
                colors::BEARISH
            };

            let ask_color = if instrument.ask > instrument.prev_ask {
                colors::BULLISH
            } else {
                colors::BEARISH
            };

            let dp = instrument.decimal_places as usize;

            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Bid:").size(11.0).color(egui::Color32::from_rgb(150, 150, 150)));
                ui.label(egui::RichText::new(format!("{:.dp$}", instrument.bid))
                    .strong()
                    .size(14.0)
                    .color(bid_color));

                ui.separator();

                ui.label(egui::RichText::new("Ask:").size(11.0).color(egui::Color32::from_rgb(150, 150, 150)));
                ui.label(egui::RichText::new(format!("{:.dp$}", instrument.ask))
                    .size(14.0)
                    .color(ask_color));
            });

            // Handle click to select instrument
            if response.clicked() {
                chart_state.selected_instrument = Some(instrument.symbol.clone());
            }

            // Highlight on hover
            if response.hovered() {
                ui.painter().rect_stroke(
                    ui.available_rect_before_wrap(),
                    4.0,
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(80, 85, 100)),
                );
            }
        });
}

/// Render the chart panel - now using Bevy native rendering
/// This function renders the UI controls around the chart area,
/// while the actual chart is rendered by Bevy systems
fn render_chart_panel(
    ui: &mut egui::Ui,
    app_state: &AppState,
    chart_state: &mut ChartState,
    viewport_price_range: (f64, f64),  // (price_min, price_max) from ChartViewport
    viewport_chart_size: bevy::math::Vec2,  // Chart size from ChartViewport
    viewport_chart_pos: bevy::math::Vec2,   // Chart position from ChartViewport
    viewport_time_start: i64,               // Time start from ChartViewport for tooltip
) {
    let (vp_price_min, vp_price_max) = viewport_price_range;
    // Clone to avoid borrow issues
    let selected_symbol = chart_state.selected_instrument.clone();
    let selected_timeframe = chart_state.selected_timeframe;

    if let Some(symbol) = selected_symbol {
        if let Some(instrument) = app_state.instruments.get(&symbol) {
            // Toolbar area
            ui.horizontal(|ui| {
                // Instrument name
                ui.label(egui::RichText::new(&symbol).strong().size(16.0));
                ui.separator();

                // Timeframe selector
                render_timeframe_selector(ui, chart_state);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    render_chart_controls(ui, chart_state);
                });
            });

            ui.separator();

            // Get candles for selected timeframe
            let candles = instrument.candles
                .get(&selected_timeframe)
                .map(|v| v.as_slice())
                .unwrap_or(&[]);

            if candles.is_empty() {
                // No data for this timeframe - show message
                ui.vertical_centered(|ui| {
                    ui.add_space(100.0);
                    ui.label(egui::RichText::new(format!("No {} data available for {}",
                        selected_timeframe.as_str(), &symbol))
                        .size(16.0)
                        .color(egui::Color32::from_rgb(120, 120, 120)));
                    ui.add_space(10.0);
                    ui.label(egui::RichText::new("Historical data will be loaded from database")
                        .size(12.0)
                        .color(egui::Color32::from_rgb(100, 100, 100)));
                });
            } else {
                // Layout: chart area + price axis on the right
                let available_size = ui.available_size();
                let price_axis_width = 80.0;
                let time_axis_height = 30.0;
                let chart_width = available_size.x - price_axis_width;
                let chart_height = available_size.y - time_axis_height - 40.0; // Leave space for scrollbar

                // Calculate price range for axis labels
                let (start_idx, end_idx, _) = chart_state.visible_range(
                    candles.len(),
                    chart_width,
                    12.0 / chart_state.zoom_level as f32,
                );
                let visible_candles = &candles[start_idx..end_idx];
                let (min_price, max_price) = if !visible_candles.is_empty() {
                    let (min, max) = visible_candles.iter().fold(
                        (f64::MAX, f64::MIN),
                        |(min, max), c| (min.min(c.low), max.max(c.high)),
                    );
                    // Apply vertical zoom
                    let range = max - min;
                    let padding = range * 0.05;
                    let base_min = min - padding;
                    let base_max = max + padding;
                    let base_range = base_max - base_min;
                    let zoomed_range = base_range / chart_state.vertical_zoom;
                    let center = chart_state.price_center.unwrap_or((base_min + base_max) / 2.0);
                    (center - zoomed_range / 2.0, center + zoomed_range / 2.0)
                } else {
                    (0.0, 100.0)
                };

                ui.horizontal(|ui| {
                    // Chart area - transparent for Bevy rendering
                    let response = ui.allocate_response(
                        egui::Vec2::new(chart_width, chart_height),
                        egui::Sense::click_and_drag(),
                    );

                    // Handle chart interactions (scroll zoom, drag pan, keyboard, double-click reset)
                    let candle_width = 12.0 / chart_state.zoom_level as f32;
                    handle_chart_interaction(ui, &response, chart_state, candle_width, Some((min_price, max_price)));

                    // Show candle tooltip on hover - use timestamp-based lookup to handle gaps
                    if let Some(hover_pos) = response.hover_pos() {
                        // Calculate which slot (time position) is being hovered
                        let x_in_chart = hover_pos.x - response.rect.min.x;
                        let slot_index = (x_in_chart / candle_width) as i64;

                        // Convert slot to timestamp using the viewport's time_start
                        let interval = selected_timeframe.seconds();
                        let hovered_timestamp = viewport_time_start + (slot_index * interval);

                        // Find candle closest to this timestamp (within half interval)
                        let half_interval = interval / 2;
                        if let Some(candle) = candles.iter().find(|c| {
                            (c.timestamp - hovered_timestamp).abs() <= half_interval
                        }) {
                            // Format timestamp with full date and time
                            let dt: chrono::DateTime<chrono::Utc> = chrono::DateTime::from_timestamp(candle.timestamp, 0).unwrap_or_default();
                            let dp = instrument.decimal_places as usize;

                            // Determine if bullish or bearish
                            let is_bullish = candle.close >= candle.open;
                            let price_color = if is_bullish { colors::BULLISH } else { colors::BEARISH };

                            // Build tooltip text (using ASCII dashes for compatibility)
                            let tooltip_text = format!(
                                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}\n\
                                 -----------------\n\
                                 Open:   {:.dp$}\n\
                                 High:   {:.dp$}\n\
                                 Low:    {:.dp$}\n\
                                 Close:  {:.dp$}{}",
                                dt.year(), dt.month(), dt.day(),
                                dt.hour(), dt.minute(), dt.second(),
                                candle.open, candle.high, candle.low, candle.close,
                                if candle.volume > 0 { format!("\nVolume: {}", candle.volume) } else { String::new() },
                                dp = dp
                            );

                            // Show tooltip
                            egui::containers::popup::show_tooltip_at_pointer(
                                ui.ctx(),
                                ui.layer_id(),
                                egui::Id::new("candle_tooltip"),
                                |ui: &mut egui::Ui| {
                                    ui.style_mut().visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(25, 27, 35);
                                    ui.colored_label(price_color, &tooltip_text);
                                }
                            );
                        }
                    }

                    // Price axis (right side) - render with egui
                    let price_axis_response = ui.allocate_response(
                        egui::Vec2::new(price_axis_width, chart_height),
                        egui::Sense::click_and_drag(),
                    );

                    // Draw price axis background (cTrader style - darker)
                    let painter = ui.painter_at(price_axis_response.rect);
                    let axis_bg = if price_axis_response.hovered() || price_axis_response.dragged() {
                        egui::Color32::from_rgb(28, 30, 38)
                    } else {
                        egui::Color32::from_rgb(20, 22, 28)
                    };
                    painter.rect_filled(price_axis_response.rect, 0.0, axis_bg);

                    // Draw price labels with adaptive granularity
                    let price_range = max_price - min_price;
                    let dp = instrument.decimal_places as usize;

                    if price_range > 0.0 {
                        // Calculate nice interval - show many more price labels for professional look
                        // Target 15-25 labels for granular price axis like cTrader
                        let target_labels = (20.0 * chart_state.vertical_zoom).clamp(15.0, 30.0) as i32;
                        let raw_interval = price_range / target_labels as f64;
                        let magnitude = 10f64.powf(raw_interval.log10().floor());
                        let normalized = raw_interval / magnitude;
                        // Use finer steps: 1, 1.5, 2, 2.5, 5 for more granular pricing
                        let nice_interval = if normalized <= 1.0 { magnitude }
                            else if normalized <= 1.5 { 1.5 * magnitude }
                            else if normalized <= 2.0 { 2.0 * magnitude }
                            else if normalized <= 2.5 { 2.5 * magnitude }
                            else if normalized <= 5.0 { 5.0 * magnitude }
                            else { 10.0 * magnitude };

                        let first_label = (min_price / nice_interval).ceil() * nice_interval;
                        let mut price = first_label;

                        while price <= max_price {
                            let ratio = (price - min_price) / price_range;
                            let y = price_axis_response.rect.max.y - (ratio as f32 * price_axis_response.rect.height());

                            // Draw subtle tick mark
                            painter.line_segment(
                                [egui::pos2(price_axis_response.rect.min.x, y),
                                 egui::pos2(price_axis_response.rect.min.x + 4.0, y)],
                                egui::Stroke::new(1.0, egui::Color32::from_rgb(60, 62, 70)),
                            );

                            painter.text(
                                egui::pos2(price_axis_response.rect.min.x + 8.0, y),
                                egui::Align2::LEFT_CENTER,
                                format!("{:.dp$}", price),
                                egui::FontId::proportional(10.0),
                                colors::AXIS_TEXT,
                            );
                            price += nice_interval;
                        }
                    }

                    // Draw live price label box with mid-price and tick direction color (cTrader style)
                    // Use ChartViewport values for exact alignment with Bevy-rendered line
                    let mid_price = instrument.mid_price();
                    let vp_price_range = vp_price_max - vp_price_min;
                    let price_visible = mid_price >= vp_price_min && mid_price <= vp_price_max;
                    if mid_price > 0.0 && vp_price_range > 0.0 && price_visible {
                        // Calculate Y using exact same formula as Bevy chart for perfect alignment
                        // Bevy calculates: y = -height/2 + ratio * height (in centered coords)
                        // We need to map this to screen coords where the chart starts at viewport_chart_pos.y
                        let ratio = (mid_price - vp_price_min) / vp_price_range;
                        // Map ratio to screen Y: chart_top + (1-ratio) * chart_height
                        // (1-ratio because screen Y increases downward, but price increases upward)
                        let y = viewport_chart_pos.y + ((1.0 - ratio as f32) * viewport_chart_size.y);

                        // Color based on tick direction: green for up, orange for down
                        let tick_color = match instrument.tick_direction {
                            TickDirection::Up => colors::BULLISH,
                            TickDirection::Down => colors::BEARISH,
                        };

                        // Draw horizontal line extending from left edge to label (cTrader style)
                        painter.line_segment(
                            [egui::pos2(price_axis_response.rect.min.x - 20.0, y),
                             egui::pos2(price_axis_response.rect.min.x + 2.0, y)],
                            egui::Stroke::new(1.0, tick_color),
                        );

                        // Draw the price label box - centered on the line
                        let label_height = 18.0;
                        let label_rect = egui::Rect::from_min_size(
                            egui::pos2(price_axis_response.rect.min.x + 2.0, y - label_height / 2.0),
                            egui::vec2(price_axis_width - 4.0, label_height),
                        );
                        painter.rect_filled(label_rect, 2.0, tick_color);

                        // Draw price text centered in box
                        painter.text(
                            label_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            format!("{:.dp$}", mid_price),
                            egui::FontId::proportional(11.0),
                            egui::Color32::WHITE,
                        );
                    }

                    // Handle price axis drag for vertical zoom (faster rate than chart drag)
                    if price_axis_response.dragged() {
                        let delta = price_axis_response.drag_delta();
                        // Higher sensitivity: /35.0 instead of /100.0 for ~3x faster zoom
                        let zoom_factor = -delta.y as f64 / 35.0;
                        chart_state.adjust_vertical_zoom(zoom_factor);
                    }

                    // Change cursor on price axis hover
                    if price_axis_response.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
                    }
                });

                // Time axis (bottom) - cTrader style with dates
                ui.horizontal(|ui| {
                    let time_axis_response = ui.allocate_response(
                        egui::Vec2::new(chart_width, time_axis_height),
                        egui::Sense::click_and_drag(),
                    );

                    let painter = ui.painter_at(time_axis_response.rect);
                    let axis_bg = if time_axis_response.hovered() || time_axis_response.dragged() {
                        egui::Color32::from_rgb(28, 30, 38)
                    } else {
                        egui::Color32::from_rgb(20, 22, 28)
                    };
                    painter.rect_filled(time_axis_response.rect, 0.0, axis_bg);

                    // Draw time labels with date (cTrader style)
                    let candle_width = 12.0 / chart_state.zoom_level as f32;
                    let label_interval = (80.0 / candle_width).max(1.0) as usize;
                    let mut last_day = -1i32;

                    for (i, candle) in visible_candles.iter().enumerate() {
                        if i % label_interval != 0 { continue; }
                        let x = time_axis_response.rect.min.x + (i as f32 * candle_width) + candle_width / 2.0;
                        if x > time_axis_response.rect.max.x - 60.0 { break; }

                        let dt: chrono::DateTime<chrono::Utc> = chrono::DateTime::from_timestamp(candle.timestamp, 0).unwrap_or_default();
                        let day = dt.day() as i32;

                        // Format like cTrader: "5 Feb 01:00" when day changes, otherwise just time
                        let text = if day != last_day {
                            last_day = day;
                            format!("{} {} {:02}:{:02}",
                                dt.day(),
                                match dt.month() {
                                    1 => "Jan", 2 => "Feb", 3 => "Mar", 4 => "Apr",
                                    5 => "May", 6 => "Jun", 7 => "Jul", 8 => "Aug",
                                    9 => "Sep", 10 => "Oct", 11 => "Nov", _ => "Dec"
                                },
                                dt.hour(), dt.minute()
                            )
                        } else {
                            format!("{:02}:{:02}", dt.hour(), dt.minute())
                        };

                        // Draw tick mark
                        painter.line_segment(
                            [egui::pos2(x, time_axis_response.rect.min.y),
                             egui::pos2(x, time_axis_response.rect.min.y + 4.0)],
                            egui::Stroke::new(1.0, egui::Color32::from_rgb(60, 62, 70)),
                        );

                        painter.text(
                            egui::pos2(x, time_axis_response.rect.min.y + 16.0),
                            egui::Align2::CENTER_CENTER,
                            text,
                            egui::FontId::proportional(9.0),
                            colors::AXIS_TEXT,
                        );
                    }

                    // Handle time axis drag for horizontal zoom
                    if time_axis_response.dragged() {
                        let delta = time_axis_response.drag_delta();
                        let zoom_factor = delta.x as f64 / 100.0;
                        chart_state.zoom_level = (chart_state.zoom_level * (1.0 - zoom_factor)).clamp(0.3, 5.0);
                    }

                    if time_axis_response.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                    }
                });

                ui.add_space(4.0);

                // Render horizontal scrollbar (still using egui)
                let total_db_candles = chart_state.total_candles_in_db
                    .get(&symbol)
                    .and_then(|m| m.get(&selected_timeframe))
                    .copied()
                    .unwrap_or(candles.len());

                let needs_more_data = render_scrollbar(
                    ui,
                    chart_state,
                    candles.len(),
                    total_db_candles,
                    chart_state.is_loading,
                );

                // Request more data if needed and not already loading
                if needs_more_data && !chart_state.is_loading && chart_state.load_more_request.is_none() {
                    let all_loaded = chart_state.all_data_loaded
                        .get(&symbol)
                        .and_then(|m| m.get(&selected_timeframe))
                        .copied()
                        .unwrap_or(false);

                    if !all_loaded {
                        if let Some(oldest_candle) = candles.first() {
                            chart_state.load_more_request = Some(LoadMoreDataRequest {
                                symbol: symbol.clone(),
                                timeframe: selected_timeframe,
                                before_timestamp: oldest_candle.timestamp,
                                count: 500,
                            });
                        }
                    }
                }
            }
        }
    } else {
        // No instrument selected - show placeholder
        ui.vertical_centered(|ui| {
            ui.add_space(150.0);

            ui.label(egui::RichText::new("Select an Instrument")
                .size(20.0)
                .color(egui::Color32::from_rgb(150, 150, 150)));

            ui.add_space(10.0);

            ui.label(egui::RichText::new("Click on EURUSD or BTCUSD in the sidebar to view the chart")
                .size(13.0)
                .color(egui::Color32::from_rgb(100, 100, 100)));
        });
    }
}
