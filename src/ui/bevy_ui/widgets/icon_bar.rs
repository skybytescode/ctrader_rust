//! Icon bar widget (narrow left sidebar)

use bevy::prelude::*;
use crate::ui::ActiveView;
use crate::ui::bevy_ui::{
    IconBar, IconButton, IconLabel,
    theme::{colors, sizing, fonts},
};

/// Spawn the icon bar with view buttons
pub fn spawn_icon_bar(commands: &mut Commands, parent: Entity) {
    let bar = commands.spawn((
        Node {
            width: Val::Px(sizing::ICON_BAR_WIDTH),
            height: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            padding: UiRect::top(Val::Px(sizing::SPACING_LARGE)),
            row_gap: Val::Px(sizing::SPACING_LARGE),
            border: UiRect::right(Val::Px(1.0)),
            ..default()
        },
        BackgroundColor(colors::BG_DARK),
        BorderColor(colors::BORDER),
        IconBar,
    )).id();

    // Bot button (using ASCII — Bevy default font doesn't render emojis)
    spawn_icon_button(commands, bar, ActiveView::Bots, "[B]", "Bots");

    commands.entity(parent).add_child(bar);
}

/// Spawn a single icon button with label
fn spawn_icon_button(
    commands: &mut Commands,
    parent: Entity,
    view: ActiveView,
    icon: &str,
    label: &str,
) {
    let container = commands.spawn((
        Node {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            row_gap: Val::Px(2.0),
            ..default()
        },
    )).id();

    // Icon button
    commands.spawn((
        Node {
            width: Val::Px(sizing::ICON_BUTTON_SIZE),
            height: Val::Px(sizing::ICON_BUTTON_SIZE),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        BackgroundColor(colors::BG_BUTTON),
        BorderRadius::all(Val::Px(4.0)),
        Interaction::default(),
        IconButton { view },
    )).with_children(|button| {
        button.spawn((
            Text::new(icon),
            TextFont {
                font_size: fonts::SIZE_HEADING,
                ..default()
            },
            TextColor(colors::TEXT_PRIMARY),
        ));
    }).set_parent(container);

    // Label below button
    commands.spawn((
        Text::new(label),
        TextFont {
            font_size: fonts::SIZE_TINY,
            ..default()
        },
        TextColor(colors::TEXT_MUTED),
        IconLabel { view },
    )).set_parent(container);

    commands.entity(parent).add_child(container);
}

/// Update icon button backgrounds based on active view
pub fn update_icon_button_styles(
    ui_state: Res<crate::ui::UiState>,
    mut query: Query<(&IconButton, &mut BackgroundColor)>,
) {
    if !ui_state.is_changed() {
        return;
    }

    for (icon_button, mut bg_color) in query.iter_mut() {
        let new_color = if icon_button.view == ui_state.active_view {
            colors::BG_BUTTON_ACTIVE
        } else {
            colors::BG_BUTTON
        };

        if bg_color.0 != new_color {
            bg_color.0 = new_color;
        }
    }
}
