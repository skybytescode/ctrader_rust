use bevy_egui::egui::{self, Response};
use super::state::ChartState;

/// Maximum future slots allowed (as percentage of visible candles)
const MAX_FUTURE_PERCENT: f32 = 0.5; // Allow panning up to 50% into the future

/// Handle chart interaction (zoom, pan, etc.)
pub fn handle_chart_interaction(
    ui: &egui::Ui,
    response: &Response,
    state: &mut ChartState,
    candle_width: f32,
    price_range: Option<(f64, f64)>,  // (price_min, price_max) for vertical pan
) {
    // Calculate max future slots based on visible area
    let chart_width = response.rect.width();
    let chart_height = response.rect.height();
    let visible_candles = ((chart_width / candle_width) * state.zoom_level as f32) as i64;
    let max_future_slots = (visible_candles as f32 * MAX_FUTURE_PERCENT) as i64;

    // Handle zoom with scroll wheel
    let scroll_delta = ui.input(|i| i.smooth_scroll_delta);
    if response.hovered() && scroll_delta.y != 0.0 {
        let zoom_factor = 1.0 + (scroll_delta.y * 0.002);
        state.zoom_level = (state.zoom_level * zoom_factor as f64).clamp(0.3, 5.0);
    }

    // Handle pan with drag
    // Horizontal: Drag right = show newer, Drag left = show older
    // Vertical: Drag up = show higher prices, Drag down = show lower prices
    if response.dragged() {
        let delta = response.drag_delta();

        // Horizontal pan (time axis)
        let candle_delta = (delta.x / candle_width) as i64;
        if candle_delta != 0 {
            // Allow panning into future (negative pan_offset) up to max_future_slots
            state.pan_offset = (state.pan_offset + candle_delta).max(-max_future_slots);
        }

        // Vertical pan (price axis) - drag to "grab" and move the chart
        // Drag up = chart moves up = shows lower prices
        // Drag down = chart moves down = shows higher prices
        if let Some((price_min, price_max)) = price_range {
            if delta.y.abs() > 0.5 && chart_height > 0.0 {
                let price_range = price_max - price_min;
                // Convert pixel delta to price delta (positive delta.y = drag down = show higher prices)
                let price_delta = (delta.y as f64 / chart_height as f64) * price_range;

                // Initialize price_center if not set
                if state.price_center.is_none() {
                    state.price_center = Some((price_min + price_max) / 2.0);
                }

                // Update price center
                if let Some(ref mut center) = state.price_center {
                    *center += price_delta;
                }
            }
        }
    }

    // Update crosshair position
    if response.hovered() {
        state.crosshair_pos = response.hover_pos().map(|p| (p.x, p.y));
    } else {
        state.crosshair_pos = None;
    }

    // Handle keyboard shortcuts when focused
    if response.has_focus() {
        ui.input(|i| {
            // Arrow keys for panning
            if i.key_pressed(egui::Key::ArrowLeft) {
                state.pan_left(5);
            }
            if i.key_pressed(egui::Key::ArrowRight) {
                state.pan_right(5, max_future_slots);
            }
            // +/- for zooming
            if i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals) {
                state.zoom_in();
            }
            if i.key_pressed(egui::Key::Minus) {
                state.zoom_out();
            }
            // Home key to reset view
            if i.key_pressed(egui::Key::Home) {
                state.reset_view();
            }
        });
    }

    // Double-click to reset all zoom and pan
    if response.double_clicked() {
        state.reset_view();
    }
}

/// Render timeframe selector toolbar
pub fn render_timeframe_selector(
    ui: &mut egui::Ui,
    state: &mut ChartState,
) {
    use super::state::Timeframe;

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;

        for tf in Timeframe::all() {
            let is_selected = state.selected_timeframe == *tf;
            let text = egui::RichText::new(tf.as_str())
                .size(12.0);

            let button = if is_selected {
                egui::Button::new(text)
                    .fill(egui::Color32::from_rgb(50, 54, 68))
            } else {
                egui::Button::new(text)
                    .fill(egui::Color32::from_rgb(35, 39, 50))
            };

            if ui.add(button).clicked() {
                state.selected_timeframe = *tf;
            }
        }

        ui.separator();

        // Volume toggle
        if ui.selectable_label(state.show_volume, "Vol").clicked() {
            state.show_volume = !state.show_volume;
        }

        // Grid toggle
        if ui.selectable_label(state.show_grid, "Grid").clicked() {
            state.show_grid = !state.show_grid;
        }

        // Crosshair toggle
        if ui.selectable_label(state.crosshair_enabled, "+").clicked() {
            state.crosshair_enabled = !state.crosshair_enabled;
        }
    });
}

/// Render chart controls (zoom buttons, etc.)
pub fn render_chart_controls(
    ui: &mut egui::Ui,
    state: &mut ChartState,
) {
    ui.horizontal(|ui| {
        // Horizontal zoom controls
        ui.label(egui::RichText::new("H:").size(10.0).color(egui::Color32::from_rgb(120, 120, 120)));
        if ui.small_button("−").on_hover_text("Zoom out horizontally (show more candles)").clicked() {
            state.zoom_out();
        }
        ui.label(format!("{:.0}%", 100.0 / state.zoom_level));
        if ui.small_button("+").on_hover_text("Zoom in horizontally (show fewer candles)").clicked() {
            state.zoom_in();
        }

        ui.separator();

        // Vertical zoom controls
        ui.label(egui::RichText::new("V:").size(10.0).color(egui::Color32::from_rgb(120, 120, 120)));
        if ui.small_button("−").on_hover_text("Zoom out vertically (show more price range)").clicked() {
            state.adjust_vertical_zoom(-0.1);
        }
        ui.label(format!("{:.0}%", state.vertical_zoom * 100.0));
        if ui.small_button("+").on_hover_text("Zoom in vertically (show less price range)").clicked() {
            state.adjust_vertical_zoom(0.1);
        }

        ui.separator();

        if ui.small_button("⟲").on_hover_text("Reset view (or double-click chart)").clicked() {
            state.reset_view();
        }
    });
}
