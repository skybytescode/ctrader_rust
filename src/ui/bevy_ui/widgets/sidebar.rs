//! Sidebar widget (expandable content panel)

use bevy::prelude::*;
use crate::ui::SidebarTab;
use crate::ui::bevy_ui::{
    Sidebar, TabButton, SearchInput, InstrumentListContainer, InstrumentListViewport,
    theme::{colors, sizing, fonts},
};

/// Spawn the sidebar with tabs and content area
pub fn spawn_sidebar(commands: &mut Commands, parent: Entity) {
    let sidebar = commands.spawn((
        Node {
            width: Val::Px(sizing::SIDEBAR_WIDTH),
            height: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            border: UiRect::right(Val::Px(1.0)),
            ..default()
        },
        BackgroundColor(colors::BG_SIDEBAR),
        BorderColor(colors::BORDER),
        Sidebar,
    )).id();

    // Tab bar
    spawn_tab_bar(commands, sidebar);

    // Search bar
    spawn_search_bar(commands, sidebar);

    // Column headers (Bid / Ask)
    spawn_column_headers(commands, sidebar);

    // Scrollable instrument list
    spawn_instrument_list(commands, sidebar);

    commands.entity(parent).add_child(sidebar);
}

/// Spawn the tab bar (Watchlists / All Symbols)
fn spawn_tab_bar(commands: &mut Commands, parent: Entity) {
    let tab_bar = commands.spawn((
        Node {
            width: Val::Percent(100.0),
            height: Val::Px(sizing::TAB_HEIGHT),
            flex_direction: FlexDirection::Row,
            ..default()
        },
    )).id();

    // Watchlists tab
    spawn_tab_button(commands, tab_bar, SidebarTab::Watchlists, "Watchlists");

    // All Symbols tab
    spawn_tab_button(commands, tab_bar, SidebarTab::AllSymbols, "All symbols");

    commands.entity(parent).add_child(tab_bar);
}

/// Spawn a single tab button
fn spawn_tab_button(
    commands: &mut Commands,
    parent: Entity,
    tab: SidebarTab,
    label: &str,
) {
    commands.spawn((
        Node {
            width: Val::Percent(50.0),
            height: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        BackgroundColor(colors::BG_PANEL),
        Interaction::default(),
        TabButton { tab },
    )).with_children(|button| {
        button.spawn((
            Text::new(label),
            TextFont {
                font_size: fonts::SIZE_NORMAL,
                ..default()
            },
            TextColor(colors::TEXT_SECONDARY),
        ));
    }).set_parent(parent);
}

/// Spawn the search bar (disabled/placeholder for now)
fn spawn_search_bar(commands: &mut Commands, parent: Entity) {
    let search_container = commands.spawn((
        Node {
            width: Val::Percent(100.0),
            height: Val::Px(32.0),
            padding: UiRect::all(Val::Px(sizing::SPACING)),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(sizing::SPACING),
            ..default()
        },
    )).id();

    // Search input (placeholder)
    commands.spawn((
        Node {
            flex_grow: 1.0,
            height: Val::Percent(100.0),
            padding: UiRect::horizontal(Val::Px(sizing::SPACING)),
            align_items: AlignItems::Center,
            ..default()
        },
        BackgroundColor(colors::BG_DARK),
        BorderRadius::all(Val::Px(4.0)),
        SearchInput,
    )).with_children(|input| {
        input.spawn((
            Text::new("Search"),
            TextFont {
                font_size: fonts::SIZE_NORMAL,
                ..default()
            },
            TextColor(colors::TEXT_MUTED),
        ));
    }).set_parent(search_container);

    // Search icon
    commands.spawn((
        Text::new("🔍"),
        TextFont {
            font_size: fonts::SIZE_MEDIUM,
            ..default()
        },
        TextColor(colors::TEXT_MUTED),
    )).set_parent(search_container);

    commands.entity(parent).add_child(search_container);
}

/// Spawn column headers (Bid / Ask)
fn spawn_column_headers(commands: &mut Commands, parent: Entity) {
    let header_row = commands.spawn((
        Node {
            width: Val::Percent(100.0),
            height: Val::Px(20.0),
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::FlexEnd,
            padding: UiRect::horizontal(Val::Px(sizing::SPACING)),
            column_gap: Val::Px(30.0),
            ..default()
        },
    )).id();

    commands.spawn((
        Text::new("Bid"),
        TextFont {
            font_size: fonts::SIZE_SMALL,
            ..default()
        },
        TextColor(colors::TEXT_MUTED),
    )).set_parent(header_row);

    commands.spawn((
        Text::new("Ask"),
        TextFont {
            font_size: fonts::SIZE_SMALL,
            ..default()
        },
        TextColor(colors::TEXT_MUTED),
    )).set_parent(header_row);

    commands.entity(parent).add_child(header_row);
}

/// Spawn the scrollable instrument list container
fn spawn_instrument_list(commands: &mut Commands, parent: Entity) {
    let container = commands.spawn((
        Node {
            width: Val::Percent(100.0),
            flex_grow: 1.0,
            overflow: Overflow::clip(),
            ..default()
        },
        InstrumentListContainer,
    )).id();

    // Inner viewport for scroll content
    commands.spawn((
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            position_type: PositionType::Relative,
            ..default()
        },
        Interaction::default(),  // For scroll detection
        InstrumentListViewport,
    )).set_parent(container);

    commands.entity(parent).add_child(container);
}

/// Update tab button styles based on selected tab
pub fn update_tab_button_styles(
    ui_state: Res<crate::ui::UiState>,
    mut query: Query<(&TabButton, &mut BackgroundColor, &Children)>,
    mut text_query: Query<&mut TextColor>,
) {
    if !ui_state.is_changed() {
        return;
    }

    for (tab_button, mut bg_color, children) in query.iter_mut() {
        let is_selected = tab_button.tab == ui_state.sidebar_tab;

        let new_bg = if is_selected {
            colors::BG_BUTTON_ACTIVE
        } else {
            colors::BG_PANEL
        };

        if bg_color.0 != new_bg {
            bg_color.0 = new_bg;
        }

        // Update text color
        for child in children.iter() {
            if let Ok(mut text_color) = text_query.get_mut(*child) {
                let new_text_color = if is_selected {
                    colors::TEXT_PRIMARY
                } else {
                    colors::TEXT_SECONDARY
                };

                if text_color.0 != new_text_color {
                    text_color.0 = new_text_color;
                }
            }
        }
    }
}
