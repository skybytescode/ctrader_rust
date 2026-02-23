//! Interaction handling systems for UI elements

use bevy::prelude::*;
use bevy::input::mouse::{MouseWheel, MouseScrollUnit};
use crate::ui::{UiState, ChartState};
use crate::ui::bevy_ui::{
    IconButton, InstrumentRow,
    TimeframeButton, ZoomButton, ResetButton, ToggleButton, ToggleType,
    InstrumentListViewport, ScrollbarThumb, PriceAxisPanel, TimeAxisPanel,
    VirtualizedScrollState, UiInteractionState,
};

/// Handle icon bar button clicks
pub fn handle_icon_button_click(
    mut ui_state: ResMut<UiState>,
    query: Query<(&Interaction, &IconButton), Changed<Interaction>>,
) {
    for (interaction, icon_button) in query.iter() {
        if *interaction == Interaction::Pressed {
            if ui_state.active_view == icon_button.view {
                // Toggle sidebar if clicking current view
                ui_state.sidebar_expanded = !ui_state.sidebar_expanded;
            } else {
                // Switch view and expand sidebar
                ui_state.active_view = icon_button.view;
                ui_state.sidebar_expanded = true;
            }
        }
    }
}

/// Handle instrument row clicks (selection)
pub fn handle_instrument_click(
    mut chart_state: ResMut<ChartState>,
    query: Query<(&Interaction, &InstrumentRow), Changed<Interaction>>,
) {
    for (interaction, instrument_row) in query.iter() {
        if *interaction == Interaction::Pressed {
            chart_state.selected_instrument = Some(instrument_row.symbol.clone());
        }
    }
}

/// Handle timeframe button clicks
pub fn handle_timeframe_click(
    mut chart_state: ResMut<ChartState>,
    query: Query<(&Interaction, &TimeframeButton), Changed<Interaction>>,
) {
    for (interaction, timeframe_button) in query.iter() {
        if *interaction == Interaction::Pressed {
            chart_state.selected_timeframe = timeframe_button.timeframe;
        }
    }
}

/// Handle zoom control button clicks
pub fn handle_zoom_controls(
    mut chart_state: ResMut<ChartState>,
    query: Query<(&Interaction, &ZoomButton), Changed<Interaction>>,
) {
    for (interaction, zoom_button) in query.iter() {
        if *interaction == Interaction::Pressed {
            let delta = if zoom_button.is_increase { 0.1 } else { -0.1 };

            if zoom_button.is_horizontal {
                chart_state.zoom_level = (chart_state.zoom_level * (1.0 + delta)).clamp(0.3, 5.0);
            } else {
                chart_state.vertical_zoom = (chart_state.vertical_zoom * (1.0 + delta)).clamp(0.1, 20.0);
            }
        }
    }
}

/// Handle reset button click
pub fn handle_reset_click(
    mut chart_state: ResMut<ChartState>,
    query: Query<&Interaction, (Changed<Interaction>, With<ResetButton>)>,
) {
    for interaction in query.iter() {
        if *interaction == Interaction::Pressed {
            chart_state.zoom_level = 1.0;
            chart_state.vertical_zoom = 1.0;
            chart_state.pan_offset = 0;
            chart_state.price_center = None;
        }
    }
}

/// Handle toggle button clicks (Volume, Grid, Crosshair)
pub fn handle_toggle_buttons(
    mut chart_state: ResMut<ChartState>,
    query: Query<(&Interaction, &ToggleButton), Changed<Interaction>>,
) {
    for (interaction, toggle_button) in query.iter() {
        if *interaction == Interaction::Pressed {
            match toggle_button.toggle_type {
                ToggleType::Volume => chart_state.show_volume = !chart_state.show_volume,
                ToggleType::Grid => chart_state.show_grid = !chart_state.show_grid,
                ToggleType::Crosshair => chart_state.crosshair_enabled = !chart_state.crosshair_enabled,
            }
        }
    }
}

/// Handle mouse wheel scrolling in instrument list
pub fn handle_list_scroll(
    mut scroll_state: ResMut<VirtualizedScrollState>,
    mut scroll_events: EventReader<MouseWheel>,
    viewport_query: Query<&Interaction, With<InstrumentListViewport>>,
) {
    // Only scroll if hovering over the viewport
    let is_hovering = viewport_query
        .get_single()
        .map(|i| *i == Interaction::Hovered)
        .unwrap_or(false);

    if !is_hovering {
        return;
    }

    for event in scroll_events.read() {
        let delta = match event.unit {
            MouseScrollUnit::Line => event.y * scroll_state.item_height,
            MouseScrollUnit::Pixel => event.y,
        };

        scroll_state.scroll_offset -= delta;
        scroll_state.clamp_scroll();
        scroll_state.update_from_scroll();
    }
}

/// Handle scrollbar thumb dragging
pub fn handle_scrollbar_drag(
    mut chart_state: ResMut<ChartState>,
    mut interaction_state: ResMut<UiInteractionState>,
    query: Query<&Interaction, (Changed<Interaction>, With<ScrollbarThumb>)>,
    buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
) {
    // Start/stop drag
    for interaction in query.iter() {
        match interaction {
            Interaction::Pressed => {
                interaction_state.dragging_scrollbar = true;
                if let Ok(window) = windows.get_single() {
                    interaction_state.drag_start = window.cursor_position();
                }
            }
            _ => {}
        }
    }

    // End drag when mouse released
    if buttons.just_released(MouseButton::Left) {
        interaction_state.dragging_scrollbar = false;
        interaction_state.drag_start = None;
    }

    // Handle ongoing drag
    if interaction_state.dragging_scrollbar {
        if let Ok(window) = windows.get_single() {
            if let (Some(current), Some(start)) = (window.cursor_position(), interaction_state.drag_start) {
                let delta = current.x - start.x;
                // Convert pixel delta to pan offset
                // This needs to be calibrated based on scrollbar/chart width ratio
                chart_state.pan_offset += (delta * 2.0) as i64;
                interaction_state.drag_start = Some(current);
            }
        }
    }
}

/// Handle price axis dragging (vertical zoom)
pub fn handle_price_axis_drag(
    mut chart_state: ResMut<ChartState>,
    mut interaction_state: ResMut<UiInteractionState>,
    query: Query<&Interaction, (Changed<Interaction>, With<PriceAxisPanel>)>,
    buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
) {
    // Start/stop drag
    for interaction in query.iter() {
        if *interaction == Interaction::Pressed {
            interaction_state.dragging_price_axis = true;
            if let Ok(window) = windows.get_single() {
                interaction_state.drag_start = window.cursor_position();
            }
        }
    }

    // End drag when mouse released
    if buttons.just_released(MouseButton::Left) {
        interaction_state.dragging_price_axis = false;
        interaction_state.drag_start = None;
    }

    // Handle ongoing drag
    if interaction_state.dragging_price_axis {
        if let Ok(window) = windows.get_single() {
            if let (Some(current), Some(start)) = (window.cursor_position(), interaction_state.drag_start) {
                let delta_y = current.y - start.y;
                // Faster zoom rate for price axis (matching egui behavior)
                let zoom_factor = -delta_y as f64 / 35.0;
                chart_state.adjust_vertical_zoom(zoom_factor);
                interaction_state.drag_start = Some(current);
            }
        }
    }
}

/// Handle time axis dragging (horizontal zoom)
pub fn handle_time_axis_drag(
    mut chart_state: ResMut<ChartState>,
    mut interaction_state: ResMut<UiInteractionState>,
    query: Query<&Interaction, (Changed<Interaction>, With<TimeAxisPanel>)>,
    buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
) {
    // Start/stop drag
    for interaction in query.iter() {
        if *interaction == Interaction::Pressed {
            interaction_state.dragging_time_axis = true;
            if let Ok(window) = windows.get_single() {
                interaction_state.drag_start = window.cursor_position();
            }
        }
    }

    // End drag when mouse released
    if buttons.just_released(MouseButton::Left) {
        interaction_state.dragging_time_axis = false;
        interaction_state.drag_start = None;
    }

    // Handle ongoing drag
    if interaction_state.dragging_time_axis {
        if let Ok(window) = windows.get_single() {
            if let (Some(current), Some(start)) = (window.cursor_position(), interaction_state.drag_start) {
                let delta_x = current.x - start.x;
                let zoom_factor = delta_x as f64 / 100.0;
                chart_state.zoom_level = (chart_state.zoom_level * (1.0 - zoom_factor)).clamp(0.3, 5.0);
                interaction_state.drag_start = Some(current);
            }
        }
    }
}

/// Update hover state for instrument rows
pub fn update_instrument_hover(
    mut interaction_state: ResMut<UiInteractionState>,
    query: Query<(&Interaction, &InstrumentRow), Changed<Interaction>>,
) {
    for (interaction, instrument_row) in query.iter() {
        match interaction {
            Interaction::Hovered => {
                interaction_state.hovered_instrument = Some(instrument_row.symbol.clone());
            }
            Interaction::None => {
                if interaction_state.hovered_instrument.as_ref() == Some(&instrument_row.symbol) {
                    interaction_state.hovered_instrument = None;
                }
            }
            _ => {}
        }
    }
}
