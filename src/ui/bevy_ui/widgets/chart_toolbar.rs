//! Chart toolbar widget

use bevy::prelude::*;
use crate::ui::Timeframe;
use crate::ui::bevy_ui::{
    ChartToolbar, SelectedInstrumentLabel, TimeframeButton,
    ZoomButton, ZoomDisplay, ResetButton,
    theme::{colors, sizing, fonts},
};

/// Spawn the chart toolbar
pub fn spawn_chart_toolbar(commands: &mut Commands, parent: Entity) {
    let toolbar = commands.spawn((
        Node {
            width: Val::Percent(100.0),
            height: Val::Px(sizing::TOOLBAR_HEIGHT),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            padding: UiRect::horizontal(Val::Px(sizing::SPACING)),
            border: UiRect::bottom(Val::Px(1.0)),
            ..default()
        },
        BackgroundColor(colors::BG_PANEL),
        BorderColor(colors::BORDER),
        ChartToolbar,
    )).id();

    // Left side: instrument name + timeframe selector
    let left_container = commands.spawn((
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(sizing::SPACING_LARGE),
            ..default()
        },
    )).id();

    // Selected instrument label
    commands.spawn((
        Text::new(""),
        TextFont {
            font_size: fonts::SIZE_LARGE,
            ..default()
        },
        TextColor(colors::TEXT_PRIMARY),
        SelectedInstrumentLabel,
    )).set_parent(left_container);

    // Separator
    commands.spawn((
        Node {
            width: Val::Px(1.0),
            height: Val::Px(20.0),
            ..default()
        },
        BackgroundColor(colors::SEPARATOR),
    )).set_parent(left_container);

    // Timeframe buttons
    spawn_timeframe_buttons(commands, left_container);

    commands.entity(toolbar).add_child(left_container);

    // Right side: zoom controls
    let right_container = commands.spawn((
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(sizing::SPACING_LARGE),
            ..default()
        },
    )).id();

    // Horizontal zoom
    spawn_zoom_control(commands, right_container, true, "H:");

    // Vertical zoom
    spawn_zoom_control(commands, right_container, false, "V:");

    // Reset button
    commands.spawn((
        Node {
            width: Val::Px(24.0),
            height: Val::Px(24.0),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        BackgroundColor(colors::BG_BUTTON),
        BorderRadius::all(Val::Px(4.0)),
        Interaction::default(),
        ResetButton,
    )).with_children(|button| {
        button.spawn((
            Text::new("⟲"),
            TextFont {
                font_size: fonts::SIZE_MEDIUM,
                ..default()
            },
            TextColor(colors::TEXT_SECONDARY),
        ));
    }).set_parent(right_container);

    commands.entity(toolbar).add_child(right_container);
    commands.entity(parent).add_child(toolbar);
}

/// Spawn timeframe selector buttons
fn spawn_timeframe_buttons(commands: &mut Commands, parent: Entity) {
    let container = commands.spawn((
        Node {
            flex_direction: FlexDirection::Row,
            column_gap: Val::Px(2.0),
            ..default()
        },
    )).id();

    let timeframes = [
        (Timeframe::M1, "1m"),
        (Timeframe::M5, "5m"),
        (Timeframe::M15, "15m"),
        (Timeframe::H1, "1H"),
        (Timeframe::H4, "4H"),
        (Timeframe::D1, "1D"),
    ];

    for (tf, label) in timeframes {
        commands.spawn((
            Node {
                padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(colors::BG_BUTTON),
            BorderRadius::all(Val::Px(4.0)),
            Interaction::default(),
            TimeframeButton { timeframe: tf },
        )).with_children(|button| {
            button.spawn((
                Text::new(label),
                TextFont {
                    font_size: fonts::SIZE_SMALL,
                    ..default()
                },
                TextColor(colors::TEXT_SECONDARY),
            ));
        }).set_parent(container);
    }

    commands.entity(parent).add_child(container);
}

/// Spawn a zoom control (label + minus + display + plus)
fn spawn_zoom_control(
    commands: &mut Commands,
    parent: Entity,
    is_horizontal: bool,
    label: &str,
) {
    let container = commands.spawn((
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(2.0),
            ..default()
        },
    )).id();

    // Label
    commands.spawn((
        Text::new(label),
        TextFont {
            font_size: fonts::SIZE_SMALL,
            ..default()
        },
        TextColor(colors::TEXT_MUTED),
    )).set_parent(container);

    // Minus button
    spawn_zoom_button(commands, container, is_horizontal, false);

    // Display
    commands.spawn((
        Node {
            min_width: Val::Px(40.0),
            justify_content: JustifyContent::Center,
            ..default()
        },
    )).with_children(|display| {
        display.spawn((
            Text::new("100%"),
            TextFont {
                font_size: fonts::SIZE_SMALL,
                ..default()
            },
            TextColor(colors::TEXT_SECONDARY),
            ZoomDisplay { is_horizontal },
        ));
    }).set_parent(container);

    // Plus button
    spawn_zoom_button(commands, container, is_horizontal, true);

    commands.entity(parent).add_child(container);
}

/// Spawn a zoom button (+ or -)
fn spawn_zoom_button(
    commands: &mut Commands,
    parent: Entity,
    is_horizontal: bool,
    is_increase: bool,
) {
    let label = if is_increase { "+" } else { "−" };

    commands.spawn((
        Node {
            width: Val::Px(20.0),
            height: Val::Px(20.0),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        BackgroundColor(colors::BG_BUTTON),
        BorderRadius::all(Val::Px(2.0)),
        Interaction::default(),
        ZoomButton { is_horizontal, is_increase },
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

/// Update timeframe button styles based on selection
pub fn update_timeframe_button_styles(
    chart_state: Res<crate::ui::ChartState>,
    mut query: Query<(&TimeframeButton, &mut BackgroundColor, &Children)>,
    mut text_query: Query<&mut TextColor>,
) {
    if !chart_state.is_changed() {
        return;
    }

    for (tf_button, mut bg_color, children) in query.iter_mut() {
        let is_selected = tf_button.timeframe == chart_state.selected_timeframe;

        let new_bg = if is_selected {
            colors::BG_BUTTON_ACTIVE
        } else {
            colors::BG_BUTTON
        };

        if bg_color.0 != new_bg {
            bg_color.0 = new_bg;
        }

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

/// Update zoom display text
pub fn update_zoom_displays(
    chart_state: Res<crate::ui::ChartState>,
    mut query: Query<(&ZoomDisplay, &mut Text)>,
) {
    if !chart_state.is_changed() {
        return;
    }

    for (zoom_display, mut text) in query.iter_mut() {
        let zoom = if zoom_display.is_horizontal {
            chart_state.zoom_level
        } else {
            chart_state.vertical_zoom
        };

        let new_text = format!("{:.0}%", zoom * 100.0);
        if text.0 != new_text {
            text.0 = new_text;
        }
    }
}

/// Update selected instrument label
pub fn update_selected_instrument_label(
    chart_state: Res<crate::ui::ChartState>,
    mut query: Query<&mut Text, With<SelectedInstrumentLabel>>,
) {
    if !chart_state.is_changed() {
        return;
    }

    for mut text in query.iter_mut() {
        let new_text = chart_state.selected_instrument.clone().unwrap_or_default();
        if text.0 != new_text {
            text.0 = new_text;
        }
    }
}
