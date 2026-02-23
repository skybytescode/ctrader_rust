//! Virtualized scrolling systems for instrument list
//!
//! Only spawns entities for visible rows, dramatically reducing
//! the number of entities when there are 100+ instruments.

use bevy::prelude::*;
use std::collections::HashSet;
use crate::ui::{UiState, AppState, ChartState};
use crate::ui::bevy_ui::{
    InstrumentRow, InstrumentListViewport, BidPrice, AskPrice, SpreadPrice, SymbolName,
    CategoryHeader,
    VirtualizedScrollState,
    theme::{colors, sizing, fonts},
    systems::layout::{get_item_at_index, VirtualizedListItem},
};

/// Spawn visible instrument rows based on scroll position
pub fn spawn_visible_rows(
    mut commands: Commands,
    scroll_state: Res<VirtualizedScrollState>,
    ui_state: Res<UiState>,
    app_state: Res<AppState>,
    chart_state: Res<ChartState>,
    existing_rows: Query<(Entity, &InstrumentRow)>,
    viewport_query: Query<Entity, With<InstrumentListViewport>>,
) {
    // Only update when scroll state or ui state changes
    if !scroll_state.is_changed() && !ui_state.is_changed() {
        return;
    }

    let Ok(viewport_entity) = viewport_query.get_single() else {
        return;
    };

    let visible_range = scroll_state.calculate_visible_range();

    // Collect existing row indices
    let existing_indices: HashSet<usize> = existing_rows
        .iter()
        .map(|(_, row)| row.list_index)
        .collect();

    // Spawn missing rows
    for idx in visible_range.clone() {
        if existing_indices.contains(&idx) {
            continue;
        }

        let item = get_item_at_index(idx, &ui_state, &app_state);

        match item {
            VirtualizedListItem::CategoryHeader(category) => {
                spawn_category_header(
                    &mut commands,
                    viewport_entity,
                    idx,
                    category,
                    &scroll_state,
                );
            }
            VirtualizedListItem::Instrument(symbol) => {
                if let Some(instrument) = app_state.instruments.get(&symbol) {
                    let is_selected = chart_state.selected_instrument.as_ref() == Some(&symbol);
                    spawn_instrument_row(
                        &mut commands,
                        viewport_entity,
                        idx,
                        &symbol,
                        instrument.decimal_places,
                        instrument.bid,
                        instrument.ask,
                        is_selected,
                        &scroll_state,
                    );
                }
            }
            VirtualizedListItem::Empty => {}
        }
    }
}

/// Despawn rows that are no longer visible
pub fn despawn_invisible_rows(
    mut commands: Commands,
    scroll_state: Res<VirtualizedScrollState>,
    existing_rows: Query<(Entity, &InstrumentRow)>,
) {
    if !scroll_state.is_changed() {
        return;
    }

    let visible_range = scroll_state.calculate_visible_range();

    for (entity, row) in existing_rows.iter() {
        if !visible_range.contains(&row.list_index) {
            commands.entity(entity).despawn_recursive();
        }
    }
}

/// Update row positions when scrolling
pub fn update_row_positions(
    scroll_state: Res<VirtualizedScrollState>,
    mut row_query: Query<(&InstrumentRow, &mut Node)>,
) {
    if !scroll_state.is_changed() {
        return;
    }

    for (row, mut node) in row_query.iter_mut() {
        let y_position = (row.list_index as f32 * scroll_state.item_height) - scroll_state.scroll_offset;
        node.top = Val::Px(y_position);
    }
}

/// Update selection highlighting
pub fn update_row_selection(
    chart_state: Res<ChartState>,
    mut row_query: Query<(&InstrumentRow, &mut BackgroundColor)>,
) {
    if !chart_state.is_changed() {
        return;
    }

    for (row, mut bg_color) in row_query.iter_mut() {
        let is_selected = chart_state.selected_instrument.as_ref() == Some(&row.symbol);
        let new_color = if is_selected {
            colors::BG_SELECTED
        } else {
            Color::NONE
        };

        if bg_color.0 != new_color {
            bg_color.0 = new_color;
        }
    }
}

/// Spawn a category header row (no arrow — always expanded)
fn spawn_category_header(
    commands: &mut Commands,
    viewport: Entity,
    list_index: usize,
    category: crate::ui::SymbolCategory,
    scroll_state: &VirtualizedScrollState,
) {
    let y_position = (list_index as f32 * scroll_state.item_height) - scroll_state.scroll_offset;

    let row_entity = commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(y_position),
            width: Val::Percent(100.0),
            height: Val::Px(scroll_state.item_height),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            padding: UiRect::horizontal(Val::Px(sizing::SPACING)),
            ..default()
        },
        BackgroundColor(Color::NONE),
        InstrumentRow {
            symbol: format!("__category_{:?}", category),
            list_index,
        },
        CategoryHeader { category },
    )).id();

    // Category label (no arrow)
    commands.spawn((
        Text::new(category.label()),
        TextFont {
            font_size: fonts::SIZE_NORMAL,
            ..default()
        },
        TextColor(colors::TEXT_SECONDARY),
    )).set_parent(row_entity);

    commands.entity(viewport).add_child(row_entity);
}

/// Spawn an instrument row (symbol + bid + ask + spread)
fn spawn_instrument_row(
    commands: &mut Commands,
    viewport: Entity,
    list_index: usize,
    symbol: &str,
    decimal_places: u8,
    bid: f64,
    ask: f64,
    is_selected: bool,
    scroll_state: &VirtualizedScrollState,
) {
    let y_position = (list_index as f32 * scroll_state.item_height) - scroll_state.scroll_offset;
    let dp = decimal_places as usize;

    let bg_color = if is_selected {
        colors::BG_SELECTED
    } else {
        Color::NONE
    };

    let row_entity = commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(y_position),
            width: Val::Percent(100.0),
            height: Val::Px(scroll_state.item_height),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            padding: UiRect::horizontal(Val::Px(sizing::SPACING)),
            ..default()
        },
        BackgroundColor(bg_color),
        Interaction::default(),
        InstrumentRow {
            symbol: symbol.to_string(),
            list_index,
        },
    )).id();

    // Symbol name
    commands.spawn((
        Text::new(symbol),
        TextFont {
            font_size: fonts::SIZE_NORMAL,
            ..default()
        },
        TextColor(colors::TEXT_PRIMARY),
        SymbolName { symbol: symbol.to_string() },
    )).set_parent(row_entity);

    // Right side: bid + ask + spread
    let right_container = commands.spawn((
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(sizing::SPACING_LARGE),
            ..default()
        },
    )).id();

    // Bid price
    commands.spawn((
        Text::new(format!("{:.dp$}", bid)),
        TextFont {
            font_size: fonts::SIZE_SMALL,
            ..default()
        },
        TextColor(colors::TEXT_PRIMARY),
        BidPrice { symbol: symbol.to_string() },
    )).set_parent(right_container);

    // Ask price
    commands.spawn((
        Text::new(format!("{:.dp$}", ask)),
        TextFont {
            font_size: fonts::SIZE_SMALL,
            ..default()
        },
        TextColor(colors::TEXT_PRIMARY),
        AskPrice { symbol: symbol.to_string() },
    )).set_parent(right_container);

    // Spread: points for 2dp instruments (gold), pips with 1 decimal for 3+dp
    let pip_mult = if decimal_places <= 2 {
        10f64.powi(decimal_places as i32)
    } else {
        10f64.powi(decimal_places as i32 - 1)
    };
    let spread = (ask - bid) * pip_mult;
    let spread_dp = if decimal_places <= 2 { 0 } else { 1 };
    commands.spawn((
        Text::new(format!("{:.dp$}", spread, dp = spread_dp)),
        TextFont {
            font_size: fonts::SIZE_SMALL,
            ..default()
        },
        TextColor(colors::TEXT_MUTED),
        SpreadPrice { symbol: symbol.to_string() },
    )).set_parent(right_container);

    commands.entity(row_entity).add_child(right_container);
    commands.entity(viewport).add_child(row_entity);
}
