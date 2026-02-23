//! Layout systems for UI visibility and sizing

use bevy::prelude::*;
use crate::ui::{UiState, AppState, SymbolCategory};
use crate::ui::bevy_ui::{
    Sidebar, VirtualizedScrollState, UiRebuildFlags,
};

/// Update sidebar visibility based on UiState
pub fn update_sidebar_visibility(
    ui_state: Res<UiState>,
    mut sidebar_query: Query<&mut Visibility, With<Sidebar>>,
    mut rebuild_flags: ResMut<UiRebuildFlags>,
) {
    if !ui_state.is_changed() {
        return;
    }

    for mut visibility in sidebar_query.iter_mut() {
        *visibility = if ui_state.sidebar_expanded {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }

    rebuild_flags.sidebar_visibility = true;
}

/// Update virtualized scroll state based on viewport and content
pub fn update_virtualized_scroll(
    ui_state: Res<UiState>,
    app_state: Res<AppState>,
    mut scroll_state: ResMut<VirtualizedScrollState>,
    viewport_query: Query<&ComputedNode, With<crate::ui::bevy_ui::InstrumentListViewport>>,
) {
    // Get viewport height
    let viewport_height = viewport_query
        .get_single()
        .map(|node| node.size().y)
        .unwrap_or(400.0);

    // Calculate visible count from viewport height
    scroll_state.visible_count = (viewport_height / scroll_state.item_height).ceil() as usize + 1;

    // Calculate total items (category headers + expanded instruments)
    scroll_state.total_items = calculate_expanded_item_count(&ui_state, &app_state);

    // Update first visible index from scroll offset
    scroll_state.update_from_scroll();
    scroll_state.clamp_scroll();
}

/// Calculate total number of visible items
fn calculate_expanded_item_count(ui_state: &UiState, app_state: &AppState) -> usize {
    let mut count = 0;

    for category in SymbolCategory::all() {
        // Category header always counts
        count += 1;

        // Add instruments if category is expanded
        if ui_state.expanded_categories.contains(category) {
            for symbol in category.instruments() {
                if app_state.instruments.contains_key(*symbol) {
                    count += 1;
                }
            }
        }
    }

    count
}

/// Get the item at a specific index in the virtualized list
pub fn get_item_at_index(
    index: usize,
    ui_state: &UiState,
    app_state: &AppState,
) -> VirtualizedListItem {
    let mut current_index = 0;

    for category in SymbolCategory::all() {
        // Check if this index is the category header
        if current_index == index {
            return VirtualizedListItem::CategoryHeader(*category);
        }
        current_index += 1;

        // If category is expanded, iterate through its instruments
        if ui_state.expanded_categories.contains(category) {
            for symbol in category.instruments() {
                if app_state.instruments.contains_key(*symbol) {
                    if current_index == index {
                        return VirtualizedListItem::Instrument((*symbol).to_string());
                    }
                    current_index += 1;
                }
            }
        }
    }

    VirtualizedListItem::Empty
}

/// Represents an item in the virtualized list
#[derive(Debug, Clone)]
pub enum VirtualizedListItem {
    CategoryHeader(crate::ui::SymbolCategory),
    Instrument(String),
    Empty,
}
