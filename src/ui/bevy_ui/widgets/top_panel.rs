//! Top panel widget (title bar)

use bevy::prelude::*;
use crate::ui::bevy_ui::{
    TopPanel, ConnectionStatusLabel, HeaderInstrumentLabel,
    theme::{colors, sizing, fonts},
};

/// Spawn the top panel with title and status
pub fn spawn_top_panel(commands: &mut Commands, parent: Entity) {
    let panel = commands.spawn((
        Node {
            width: Val::Percent(100.0),
            height: Val::Px(sizing::TOP_PANEL_HEIGHT),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            padding: UiRect::horizontal(Val::Px(sizing::SPACING_LARGE)),
            column_gap: Val::Px(sizing::SPACING_LARGE),
            border: UiRect::bottom(Val::Px(1.0)),
            ..default()
        },
        BackgroundColor(colors::BG_PANEL),
        BorderColor(colors::BORDER),
        TopPanel,
    )).id();

    // App title
    commands.spawn((
        Text::new("cTrader Rust Terminal"),
        TextFont {
            font_size: fonts::SIZE_LARGE,
            ..default()
        },
        TextColor(colors::TEXT_PRIMARY),
    )).set_parent(panel);

    // Separator
    commands.spawn((
        Node {
            width: Val::Px(1.0),
            height: Val::Px(20.0),
            ..default()
        },
        BackgroundColor(colors::SEPARATOR),
    )).set_parent(panel);

    // Connection status
    commands.spawn((
        Text::new("Status: Init"),
        TextFont {
            font_size: fonts::SIZE_NORMAL,
            ..default()
        },
        TextColor(colors::TEXT_SECONDARY),
        ConnectionStatusLabel,
    )).set_parent(panel);

    // Separator (only visible when instrument selected)
    commands.spawn((
        Node {
            width: Val::Px(1.0),
            height: Val::Px(20.0),
            ..default()
        },
        BackgroundColor(colors::SEPARATOR),
    )).set_parent(panel);

    // Selected instrument (hidden initially)
    commands.spawn((
        Text::new(""),
        TextFont {
            font_size: fonts::SIZE_NORMAL,
            ..default()
        },
        TextColor(colors::BULLISH),
        Visibility::Hidden,
        HeaderInstrumentLabel,
    )).set_parent(panel);

    commands.entity(parent).add_child(panel);
}
