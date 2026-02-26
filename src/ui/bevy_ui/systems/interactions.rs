//! Interaction handling systems for UI elements

use bevy::prelude::*;
use bevy::input::mouse::{MouseWheel, MouseScrollUnit};
use crate::ui::UiState;
use crate::ui::bevy_ui::{
    IconButton, InstrumentRow,
    InstrumentListViewport,
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
                ui_state.sidebar_expanded = !ui_state.sidebar_expanded;
            } else {
                ui_state.active_view = icon_button.view;
                ui_state.sidebar_expanded = true;
            }
        }
    }
}

/// Handle instrument row clicks — sets selected_instrument in UiState
pub fn handle_instrument_click(
    mut ui_state: ResMut<UiState>,
    query: Query<(&Interaction, &InstrumentRow), Changed<Interaction>>,
) {
    for (interaction, instrument_row) in query.iter() {
        if *interaction == Interaction::Pressed {
            ui_state.selected_instrument = Some(instrument_row.symbol.clone());
        }
    }
}

/// Handle mouse wheel scrolling in instrument list
pub fn handle_list_scroll(
    mut scroll_state: ResMut<VirtualizedScrollState>,
    mut scroll_events: EventReader<MouseWheel>,
    viewport_query: Query<&Interaction, With<InstrumentListViewport>>,
) {
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
