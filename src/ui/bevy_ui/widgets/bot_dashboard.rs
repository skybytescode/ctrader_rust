//! Bot dashboard widget — accordion-expand cards for the selected instrument

use bevy::prelude::*;
use crate::ui::ChartState;
use crate::ui::bevy_ui::{
    BotDashboard, BotDashboardTitle,
    TopCardType, CardsContainer, MainCard, MainCardContent,
    MainCardMaxBtn, MainCardMaxBtnIcon,
    DbSubCard, DbSubCardType, DbSubCardContent,
    DbSubCardMaxBtn, DbSubCardMaxBtnIcon,
    DbTimeframeBtn, BotTimeframe,
    BotDashboardState,
    theme::{colors, fonts},
};

// ============================================================================
// Spawn helpers
// ============================================================================

/// Spawn the bot dashboard panel (hidden until an instrument is selected)
pub fn spawn_bot_dashboard(commands: &mut Commands, parent: Entity) {
    let dashboard = commands.spawn((
        Node {
            flex_grow: 1.0,
            flex_direction: FlexDirection::Column,
            padding: UiRect::all(Val::Px(20.0)),
            row_gap: Val::Px(12.0),
            ..default()
        },
        BackgroundColor(colors::BG_DARKEST),
        Visibility::Hidden,
        BotDashboard,
    )).id();

    // Header
    let header = commands.spawn((
        Node {
            width: Val::Percent(100.0),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            padding: UiRect::bottom(Val::Px(8.0)),
            border: UiRect::bottom(Val::Px(1.0)),
            ..default()
        },
        BorderColor(colors::BORDER),
    )).id();

    commands.spawn((
        Text::new("Bot Dashboard"),
        TextFont { font_size: fonts::SIZE_HEADING, ..default() },
        TextColor(colors::TEXT_PRIMARY),
        BotDashboardTitle,
    )).set_parent(header);
    commands.entity(dashboard).add_child(header);

    // Cards container — Column + Wrap creates a 2x2 grid that reshapes on expand.
    // Items flow top-to-bottom, then wrap to a new column to the right.
    // Spawn order determines grid positions:
    //   Col 1: Database, Start/Pause  |  Col 2: Train Model, Current Status
    let cards = commands.spawn((
        Node {
            width: Val::Percent(100.0),
            flex_grow: 1.0,
            flex_direction: FlexDirection::Column,
            flex_wrap: FlexWrap::Wrap,
            column_gap: Val::Px(12.0),
            row_gap: Val::Px(12.0),
            ..default()
        },
        CardsContainer,
    )).id();

    spawn_main_card(commands, cards, TopCardType::Database,      "Database",        true);
    spawn_main_card(commands, cards, TopCardType::StartPause,    "Start / Pause",   false);
    spawn_main_card(commands, cards, TopCardType::TrainModel,    "Train Model",     false);
    spawn_main_card(commands, cards, TopCardType::CurrentStatus, "Current Status",  false);

    commands.entity(dashboard).add_child(cards);
    commands.entity(parent).add_child(dashboard);
}

/// Spawn a single main card with header + expandable content area
fn spawn_main_card(
    commands: &mut Commands,
    parent: Entity,
    card_type: TopCardType,
    title: &str,
    is_database: bool,
) {
    // In a Column+Wrap container: flex_basis controls height (main axis),
    // width controls which column items land in.
    // Default 2x2 grid: ~45% height (2 per column), ~48% width (2 columns).
    let card = commands.spawn((
        Node {
            width: Val::Percent(48.0),
            flex_basis: Val::Percent(45.0),
            flex_grow: 1.0,
            min_height: Val::Px(0.0),
            flex_direction: FlexDirection::Column,
            padding: UiRect::all(Val::Px(14.0)),
            row_gap: Val::Px(10.0),
            overflow: Overflow::clip(),
            ..default()
        },
        BackgroundColor(colors::BG_PANEL),
        BorderRadius::all(Val::Px(8.0)),
        MainCard { card_type },
    )).id();

    // Header row: title + [+] button
    let header_row = commands.spawn((
        Node {
            width: Val::Percent(100.0),
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::SpaceBetween,
            align_items: AlignItems::Center,
            ..default()
        },
    )).id();

    commands.spawn((
        Text::new(title),
        TextFont { font_size: fonts::SIZE_MEDIUM, ..default() },
        TextColor(colors::TEXT_PRIMARY),
    )).set_parent(header_row);

    // [+] maximize button
    let max_btn = commands.spawn((
        Node {
            padding: UiRect::axes(Val::Px(8.0), Val::Px(3.0)),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        BackgroundColor(colors::BG_BUTTON),
        BorderRadius::all(Val::Px(4.0)),
        Interaction::default(),
        MainCardMaxBtn { card_type },
    )).id();

    commands.spawn((
        Text::new("[+]"),
        TextFont { font_size: fonts::SIZE_SMALL, ..default() },
        TextColor(colors::TEXT_PRIMARY),
        MainCardMaxBtnIcon { card_type },
    )).set_parent(max_btn);

    commands.entity(header_row).add_child(max_btn);
    commands.entity(card).add_child(header_row);

    // Database content always visible; other cards hidden until [+] is clicked.
    let initial_display = if is_database { Display::Flex } else { Display::None };
    let content = commands.spawn((
        Node {
            display: initial_display,
            width: Val::Percent(100.0),
            flex_grow: 1.0,
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(10.0),
            ..default()
        },
        MainCardContent { card_type },
    )).id();

    if is_database {
        spawn_database_content(commands, content);
    } else {
        // Placeholder content for non-Database cards
        let placeholder = match card_type {
            TopCardType::TrainModel  => "No model trained yet.",
            TopCardType::StartPause  => "Bot is stopped.",
            TopCardType::CurrentStatus => "P&L: --\nTrades: 0",
            _ => "",
        };
        commands.spawn((
            Text::new(placeholder),
            TextFont { font_size: fonts::SIZE_NORMAL, ..default() },
            TextColor(colors::TEXT_MUTED),
        )).set_parent(content);
    }

    commands.entity(card).add_child(content);
    commands.entity(parent).add_child(card);
}

/// Spawn the 4 sub-cards inside the Database card content area
fn spawn_database_content(commands: &mut Commands, parent: Entity) {
    let grid = commands.spawn((
        Node {
            width: Val::Percent(100.0),
            flex_grow: 1.0,
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            column_gap: Val::Px(10.0),
            row_gap: Val::Px(10.0),
            ..default()
        },
    )).id();

    spawn_db_subcard(commands, grid, DbSubCardType::HistoryBot,    "History BoT",    true);
    spawn_db_subcard(commands, grid, DbSubCardType::UpdateHistory, "Update History", true);
    spawn_db_subcard(commands, grid, DbSubCardType::Status,        "Status",         false);
    spawn_db_subcard(commands, grid, DbSubCardType::Dom,           "DoM",            false);

    commands.entity(parent).add_child(grid);
}

/// Spawn one sub-card inside Database
fn spawn_db_subcard(
    commands: &mut Commands,
    parent: Entity,
    sub_type: DbSubCardType,
    title: &str,
    has_timeframe_btns: bool,
) {
    let card = commands.spawn((
        Node {
            flex_basis: Val::Percent(47.0),
            flex_grow: 1.0,
            min_height: Val::Px(70.0),
            flex_direction: FlexDirection::Column,
            padding: UiRect::all(Val::Px(10.0)),
            row_gap: Val::Px(6.0),
            ..default()
        },
        BackgroundColor(colors::BG_SIDEBAR),
        BorderRadius::all(Val::Px(6.0)),
        DbSubCard { sub_type },
    )).id();

    // Sub-card header: title + [+] button
    let header = commands.spawn((
        Node {
            width: Val::Percent(100.0),
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::SpaceBetween,
            align_items: AlignItems::Center,
            ..default()
        },
    )).id();

    commands.spawn((
        Text::new(title),
        TextFont { font_size: fonts::SIZE_NORMAL, ..default() },
        TextColor(colors::TEXT_PRIMARY),
    )).set_parent(header);

    let sub_btn = commands.spawn((
        Node {
            padding: UiRect::axes(Val::Px(6.0), Val::Px(2.0)),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        BackgroundColor(colors::BG_BUTTON),
        BorderRadius::all(Val::Px(3.0)),
        Interaction::default(),
        DbSubCardMaxBtn { sub_type },
    )).id();

    commands.spawn((
        Text::new("[+]"),
        TextFont { font_size: fonts::SIZE_TINY, ..default() },
        TextColor(colors::TEXT_PRIMARY),
        DbSubCardMaxBtnIcon { sub_type },
    )).set_parent(sub_btn);

    commands.entity(header).add_child(sub_btn);
    commands.entity(card).add_child(header);

    // Sub-card content — Display::None removes it from layout
    let content = commands.spawn((
        Node {
            display: Display::None,
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(6.0),
            ..default()
        },
        DbSubCardContent { sub_type },
    )).id();

    if has_timeframe_btns {
        let btn_row = commands.spawn((
            Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(8.0),
                ..default()
            },
        )).id();
        spawn_timeframe_btn(commands, btn_row, sub_type, BotTimeframe::M1);
        spawn_timeframe_btn(commands, btn_row, sub_type, BotTimeframe::M5);
        commands.entity(content).add_child(btn_row);
    } else {
        let detail = match sub_type {
            DbSubCardType::Status => "Ready",
            DbSubCardType::Dom    => "Inactive",
            _                     => "",
        };
        commands.spawn((
            Text::new(detail),
            TextFont { font_size: fonts::SIZE_SMALL, ..default() },
            TextColor(colors::TEXT_MUTED),
        )).set_parent(content);
    }

    commands.entity(card).add_child(content);
    commands.entity(parent).add_child(card);
}

/// Spawn an M1 or M5 action button
fn spawn_timeframe_btn(
    commands: &mut Commands,
    parent: Entity,
    parent_card: DbSubCardType,
    timeframe: BotTimeframe,
) {
    let label = match timeframe { BotTimeframe::M1 => "M1", BotTimeframe::M5 => "M5" };

    let btn = commands.spawn((
        Node {
            padding: UiRect::axes(Val::Px(10.0), Val::Px(5.0)),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        BackgroundColor(colors::BG_BUTTON),
        BorderRadius::all(Val::Px(4.0)),
        Interaction::default(),
        DbTimeframeBtn { parent_card, timeframe },
    )).id();

    commands.spawn((
        Text::new(label),
        TextFont { font_size: fonts::SIZE_SMALL, ..default() },
        TextColor(colors::TEXT_PRIMARY),
    )).set_parent(btn);

    commands.entity(parent).add_child(btn);
}

// ============================================================================
// Systems
// ============================================================================

/// Show/hide dashboard and update title when selected instrument changes
pub fn update_dashboard_visibility(
    chart_state: Res<ChartState>,
    mut dashboard_query: Query<&mut Visibility, With<BotDashboard>>,
    mut title_query: Query<&mut Text, With<BotDashboardTitle>>,
) {
    if !chart_state.is_changed() { return; }

    for mut vis in dashboard_query.iter_mut() {
        *vis = if chart_state.selected_instrument.is_some() {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }

    if let Some(ref symbol) = chart_state.selected_instrument {
        for mut text in title_query.iter_mut() {
            let new_title = format!("{} Bot Dashboard", symbol);
            if text.0 != new_title { text.0 = new_title; }
        }
    }
}

/// Handle click on a main card's [+]/[-] button — toggle accordion
pub fn handle_main_card_max_btn(
    mut state: ResMut<BotDashboardState>,
    query: Query<(&Interaction, &MainCardMaxBtn), Changed<Interaction>>,
) {
    for (interaction, btn) in query.iter() {
        if *interaction == Interaction::Pressed {
            if state.expanded_top == Some(btn.card_type) {
                state.expanded_top = None;            // collapse
            } else {
                state.expanded_top = Some(btn.card_type); // expand (collapses others)
                state.expanded_db_sub = None;         // reset DB sub-accordion on card change
            }
        }
    }
}

/// Canonical spawn order for the default 2x2 grid (Column+Wrap).
/// Col 1: Database, StartPause | Col 2: TrainModel, CurrentStatus
const DEFAULT_ORDER: [TopCardType; 4] = [
    TopCardType::Database,
    TopCardType::StartPause,
    TopCardType::TrainModel,
    TopCardType::CurrentStatus,
];

/// Update main card sizes/visibility based on accordion state.
/// Uses Column+Wrap: expanded card is reordered to position 0 (fills left column at 75%),
/// the other 3 stack on the right (25% each).
pub fn update_main_card_expand(
    state: Res<BotDashboardState>,
    mut commands: Commands,
    container_query: Query<Entity, With<CardsContainer>>,
    mut card_query: Query<(Entity, &MainCard, &mut Node), Without<MainCardContent>>,
    mut content_query: Query<(&MainCardContent, &mut Node), Without<MainCard>>,
    mut icon_query: Query<(&MainCardMaxBtnIcon, &mut Text)>,
) {
    if !state.is_changed() { return; }

    let expanded = state.expanded_top;
    let Ok(container) = container_query.get_single() else { return };

    // Collect card entities by type
    let card_map: Vec<(Entity, TopCardType)> = card_query
        .iter()
        .map(|(e, mc, _)| (e, mc.card_type))
        .collect();

    // Build desired child order: expanded card first, then others
    let ordered: Vec<Entity> = if let Some(exp) = expanded {
        let mut order = Vec::with_capacity(4);
        // Expanded card first
        if let Some(&(e, _)) = card_map.iter().find(|(_, t)| *t == exp) {
            order.push(e);
        }
        // Remaining cards in canonical order
        for ct in DEFAULT_ORDER {
            if ct != exp {
                if let Some(&(e, _)) = card_map.iter().find(|(_, t)| *t == ct) {
                    order.push(e);
                }
            }
        }
        order
    } else {
        // Default 2x2 grid order
        let mut order = Vec::with_capacity(4);
        for ct in DEFAULT_ORDER {
            if let Some(&(e, _)) = card_map.iter().find(|(_, t)| *t == ct) {
                order.push(e);
            }
        }
        order
    };

    // Reorder children of the container
    commands.entity(container).replace_children(&ordered);

    // Update card sizes
    for (_entity, mc, mut node) in card_query.iter_mut() {
        if let Some(exp) = expanded {
            if mc.card_type == exp {
                // Expanded card: full height, 3/4 width (left column)
                node.width = Val::Percent(74.0);
                node.flex_basis = Val::Percent(100.0);
                node.flex_grow = 0.0;
            } else {
                // Sidebar cards: 1/4 width, ~31% height (3 per column)
                node.width = Val::Percent(24.0);
                node.flex_basis = Val::Percent(30.0);
                node.flex_grow = 1.0;
            }
        } else {
            // Default 2x2 grid: ~48% width, ~45% height
            node.width = Val::Percent(48.0);
            node.flex_basis = Val::Percent(45.0);
            node.flex_grow = 1.0;
        }
    }

    // Database content always visible; other cards show only when expanded
    for (content, mut node) in content_query.iter_mut() {
        node.display = if content.card_type == TopCardType::Database {
            Display::Flex
        } else {
            match expanded {
                Some(t) if t == content.card_type => Display::Flex,
                _ => Display::None,
            }
        };
    }

    // Update button icons
    for (icon, mut text) in icon_query.iter_mut() {
        let new_label = match expanded {
            Some(t) if t == icon.card_type => "[-]",
            _ => "[+]",
        };
        if text.0 != new_label { text.0 = new_label.to_string(); }
    }
}

/// Handle click on a DB sub-card's [+]/[-] button
pub fn handle_db_subcard_max_btn(
    mut state: ResMut<BotDashboardState>,
    query: Query<(&Interaction, &DbSubCardMaxBtn), Changed<Interaction>>,
) {
    for (interaction, btn) in query.iter() {
        if *interaction == Interaction::Pressed {
            if state.expanded_db_sub == Some(btn.sub_type) {
                state.expanded_db_sub = None;
            } else {
                state.expanded_db_sub = Some(btn.sub_type);
            }
        }
    }
}

/// Update DB sub-card content display based on accordion state
pub fn update_db_subcard_expand(
    state: Res<BotDashboardState>,
    mut content_query: Query<(&DbSubCardContent, &mut Node)>,
    mut icon_query: Query<(&DbSubCardMaxBtnIcon, &mut Text)>,
) {
    if !state.is_changed() { return; }

    let expanded = state.expanded_db_sub;

    for (content, mut node) in content_query.iter_mut() {
        node.display = match expanded {
            Some(t) if t == content.sub_type => Display::Flex,
            _ => Display::None,
        };
    }

    for (icon, mut text) in icon_query.iter_mut() {
        let new_label = match expanded {
            Some(t) if t == icon.sub_type => "[-]",
            _ => "[+]",
        };
        if text.0 != new_label { text.0 = new_label.to_string(); }
    }
}

/// Hover effect for main card [+]/[-] buttons
pub fn update_main_card_hover(
    mut query: Query<(&Interaction, &mut BackgroundColor), (Changed<Interaction>, With<MainCardMaxBtn>)>,
) {
    for (interaction, mut bg) in query.iter_mut() {
        let new_color = match interaction {
            Interaction::Hovered => colors::BG_BUTTON_ACTIVE,
            Interaction::Pressed => colors::ACCENT_BLUE,
            Interaction::None    => colors::BG_BUTTON,
        };
        if bg.0 != new_color { bg.0 = new_color; }
    }
}

/// Hover effect for DB sub-card [+]/[-] buttons
pub fn update_db_subcard_btn_hover(
    mut query: Query<(&Interaction, &mut BackgroundColor), (Changed<Interaction>, With<DbSubCardMaxBtn>)>,
) {
    for (interaction, mut bg) in query.iter_mut() {
        let new_color = match interaction {
            Interaction::Hovered => colors::BG_BUTTON_ACTIVE,
            Interaction::Pressed => colors::ACCENT_BLUE,
            Interaction::None    => colors::BG_BUTTON,
        };
        if bg.0 != new_color { bg.0 = new_color; }
    }
}

/// Hover effect for M1/M5 timeframe buttons
pub fn update_db_timeframe_btn_hover(
    mut query: Query<(&Interaction, &mut BackgroundColor), (Changed<Interaction>, With<DbTimeframeBtn>)>,
) {
    for (interaction, mut bg) in query.iter_mut() {
        let new_color = match interaction {
            Interaction::Hovered => colors::BG_BUTTON_ACTIVE,
            Interaction::Pressed => colors::ACCENT_BLUE,
            Interaction::None    => colors::BG_BUTTON,
        };
        if bg.0 != new_color { bg.0 = new_color; }
    }
}

/// Handle M1/M5 button clicks
pub fn handle_db_timeframe_btn_click(
    chart_state: Res<ChartState>,
    query: Query<(&Interaction, &DbTimeframeBtn), Changed<Interaction>>,
) {
    for (interaction, btn) in query.iter() {
        if *interaction == Interaction::Pressed {
            let symbol = chart_state.selected_instrument.as_deref().unwrap_or("None");
            let card_label = match btn.parent_card {
                DbSubCardType::HistoryBot    => "History BoT",
                DbSubCardType::UpdateHistory => "Update History",
                DbSubCardType::Status        => "Status",
                DbSubCardType::Dom           => "DoM",
            };
            let tf = match btn.timeframe { BotTimeframe::M1 => "M1", BotTimeframe::M5 => "M5" };
            println!("[{}] {} {} clicked", symbol, card_label, tf);
        }
    }
}
