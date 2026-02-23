//! Sidebar widget (Trading Bots panel)

use bevy::prelude::*;
use crate::ui::bevy_ui::{
    Sidebar, InstrumentListContainer, InstrumentListViewport,
    theme::{colors, sizing, fonts},
};

/// Spawn the sidebar with column headers and instrument list
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

    // Column headers (Bid / Ask / Spread)
    spawn_column_headers(commands, sidebar);

    // Scrollable instrument list
    spawn_instrument_list(commands, sidebar);

    commands.entity(parent).add_child(sidebar);
}

/// Spawn column headers (Bid / Ask / Spread)
fn spawn_column_headers(commands: &mut Commands, parent: Entity) {
    let header_row = commands.spawn((
        Node {
            width: Val::Percent(100.0),
            height: Val::Px(20.0),
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::FlexEnd,
            padding: UiRect::horizontal(Val::Px(sizing::SPACING)),
            column_gap: Val::Px(sizing::SPACING_LARGE),
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

    commands.spawn((
        Text::new("Spread"),
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
        Interaction::default(),
        InstrumentListViewport,
    )).set_parent(container);

    commands.entity(parent).add_child(container);
}
