//! Tooltip widget for candle OHLC display

use bevy::prelude::*;
use crate::ui::bevy_ui::{
    TooltipContainer, TooltipContent, TooltipState,
    theme::{colors, fonts},
};

/// Spawn the tooltip container (hidden by default)
pub fn spawn_tooltip(commands: &mut Commands) {
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            top: Val::Px(0.0),
            padding: UiRect::all(Val::Px(8.0)),
            border: UiRect::all(Val::Px(1.0)),
            ..default()
        },
        BackgroundColor(colors::BG_PANEL),
        BorderColor(colors::BORDER),
        BorderRadius::all(Val::Px(4.0)),
        Visibility::Hidden,
        ZIndex(100),  // Ensure tooltip is on top
        TooltipContainer,
    )).with_children(|tooltip| {
        tooltip.spawn((
            Text::new(""),
            TextFont {
                font_size: fonts::SIZE_SMALL,
                ..default()
            },
            TextColor(colors::TEXT_PRIMARY),
            TooltipContent,
        ));
    });
}

/// Update tooltip visibility and position
pub fn update_tooltip(
    tooltip_state: Res<TooltipState>,
    mut container_query: Query<(&mut Node, &mut Visibility), With<TooltipContainer>>,
    mut content_query: Query<(&mut Text, &mut TextColor), With<TooltipContent>>,
) {
    if !tooltip_state.is_changed() {
        return;
    }

    let Ok((mut node, mut visibility)) = container_query.get_single_mut() else {
        return;
    };

    let Ok((mut text, mut text_color)) = content_query.get_single_mut() else {
        return;
    };

    if tooltip_state.visible {
        *visibility = Visibility::Visible;
        node.left = Val::Px(tooltip_state.position.x);
        node.top = Val::Px(tooltip_state.position.y);

        if let Some(ref data) = tooltip_state.candle_data {
            let dp = data.decimal_places as usize;
            let dt = chrono::DateTime::from_timestamp(data.timestamp, 0)
                .unwrap_or_default();

            let tooltip_text = format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}\n\
                 -----------------\n\
                 Open:   {:.dp$}\n\
                 High:   {:.dp$}\n\
                 Low:    {:.dp$}\n\
                 Close:  {:.dp$}{}",
                dt.year(), dt.month(), dt.day(),
                dt.hour(), dt.minute(), dt.second(),
                data.open, data.high, data.low, data.close,
                if data.volume > 0 { format!("\nVolume: {}", data.volume) } else { String::new() },
                dp = dp
            );

            text.0 = tooltip_text;

            // Color based on direction
            text_color.0 = if data.is_bullish {
                colors::BULLISH
            } else {
                colors::BEARISH
            };
        }
    } else {
        *visibility = Visibility::Hidden;
    }
}

use chrono::{Datelike, Timelike};
