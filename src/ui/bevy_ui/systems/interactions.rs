//! Interaction handling systems for UI elements

use bevy::prelude::*;
use bevy::input::mouse::{MouseWheel, MouseScrollUnit};
use crate::ui::UiState;
use bevy::window::PrimaryWindow;
use crate::ui::bevy_ui::{
    IconButton, InstrumentRow,
    InstrumentListViewport,
    VirtualizedScrollState, UiInteractionState,
    MlInfoScrollArea, MlScrollbarThumb, MlModelInfoText,
    MlScrollbarDragState,
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

/// Handle mouse-wheel scrolling inside ML model info text areas
pub fn handle_ml_info_scroll(
    mut scroll_events: EventReader<MouseWheel>,
    mut query: Query<(&Interaction, &mut ScrollPosition), With<MlInfoScrollArea>>,
) {
    for event in scroll_events.read() {
        let delta = match event.unit {
            MouseScrollUnit::Line  => event.y * 16.0,
            MouseScrollUnit::Pixel => event.y,
        };
        for (interaction, mut scroll_pos) in query.iter_mut() {
            if *interaction != Interaction::None {
                scroll_pos.offset_y = (scroll_pos.offset_y - delta).max(0.0);
            }
        }
    }
}

/// Update scrollbar thumb size and position based on scroll state and content height
pub fn update_ml_scrollbar(
    mut params: ParamSet<(
        Query<(&MlInfoScrollArea, &ScrollPosition, &ComputedNode)>,
        Query<(&MlModelInfoText, &ComputedNode)>,
        Query<(&MlScrollbarThumb, &mut Node)>,
    )>,
) {
    const MIN_THUMB: f32 = 20.0;

    let areas: Vec<_> = params.p0().iter()
        .map(|(a, sp, cn)| (a.model, sp.offset_y, cn.size().y))
        .collect();

    let texts: Vec<_> = params.p1().iter()
        .map(|(t, cn)| (t.model, cn.size().y))
        .collect();

    for (model, scroll_offset, viewport_h) in &areas {
        let visible_h = viewport_h.max(1.0);
        let content_h = texts.iter()
            .find(|(m, _)| m == model)
            .map(|(_, h)| *h)
            .unwrap_or(visible_h);

        let thumb_h   = ((visible_h / content_h.max(visible_h)) * visible_h).max(MIN_THUMB);
        let max_scroll = (content_h - visible_h).max(0.0);
        let thumb_top  = if max_scroll > 0.0 {
            (scroll_offset / max_scroll) * (visible_h - thumb_h)
        } else {
            0.0
        };

        for (thumb, mut node) in params.p2().iter_mut() {
            if thumb.model == *model {
                node.height = Val::Px(thumb_h);
                node.top    = Val::Px(thumb_top);
            }
        }
    }
}

/// Handle click-and-drag on ML scrollbar thumbs
pub fn handle_ml_scrollbar_drag(
    mut drag:      ResMut<MlScrollbarDragState>,
    mouse:         Res<ButtonInput<MouseButton>>,
    windows:       Query<&Window, With<PrimaryWindow>>,
    thumb_q:       Query<(&MlScrollbarThumb, &Interaction)>,
    text_q:        Query<(&MlModelInfoText, &ComputedNode)>,
    mut scroll_q:  Query<(&MlInfoScrollArea, &mut ScrollPosition, &ComputedNode)>,
) {
    let cursor_y = windows.get_single()
        .ok()
        .and_then(|w| w.cursor_position())
        .map(|p| p.y)
        .unwrap_or(0.0);

    // Start drag
    if mouse.just_pressed(MouseButton::Left) {
        for (thumb, interaction) in thumb_q.iter() {
            if *interaction == Interaction::Pressed {
                for (area, scroll_pos, _) in scroll_q.iter() {
                    if area.model == thumb.model {
                        drag.model          = Some(thumb.model);
                        drag.cursor_y_start = cursor_y;
                        drag.scroll_start   = scroll_pos.offset_y;
                        break;
                    }
                }
            }
        }
    }

    // End drag
    if mouse.just_released(MouseButton::Left) {
        drag.model = None;
    }

    // Apply drag delta
    if let Some(model) = drag.model {
        let delta = cursor_y - drag.cursor_y_start;

        let content_h = text_q.iter()
            .find(|(t, _)| t.model == model)
            .map(|(_, cn)| cn.size().y)
            .unwrap_or(1.0);

        for (area, mut scroll_pos, area_cn) in scroll_q.iter_mut() {
            if area.model != model { continue; }

            let viewport_h    = area_cn.size().y.max(1.0);
            let max_scroll    = (content_h - viewport_h).max(0.0);
            let thumb_h       = ((viewport_h / content_h.max(viewport_h)) * viewport_h).max(20.0);
            let track_range   = viewport_h - thumb_h;

            if track_range > 0.0 {
                let scroll_per_px = max_scroll / track_range;
                scroll_pos.offset_y = (drag.scroll_start + delta * scroll_per_px)
                    .clamp(0.0, max_scroll);
            }
        }
    }
}
