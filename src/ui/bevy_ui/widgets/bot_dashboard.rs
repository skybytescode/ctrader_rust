//! Bot dashboard widget — accordion-expand cards for the selected instrument

use bevy::prelude::*;
use crate::ui::ChartState;
use crate::ui::bevy_ui::{
    BotDashboard, BotDashboardTitle,
    TopCardType, CardsContainer, MainCard, MainCardContent,
    MainCardMaxBtn, MainCardMaxBtnIcon,
    DbSubCard, DbSubCardType,
    DbTimeframeBtn, DbStatusText, HistoryBotStatusText, UpdateHistoryStatusText, BotTimeframe,
    M1InfoText, TickInfoText, MlInfoText,
    BotDashboardState, TickWorkflowStep,
    theme::{colors, fonts},
};
use crate::data_retrieval::{
    DataKind, DataAction, DataRequest, DataResponse,
    DataRequestSender, DataResponseReceiver, SymbolIdMap,
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

    // Colored status text rows: M1 (light blue) | Ticks (yellow) | ML Features (green)
    commands.spawn((
        Text::new(""),
        TextFont { font_size: fonts::SIZE_SMALL, ..default() },
        TextColor(Color::srgb(0.45, 0.75, 1.0)),
        M1InfoText,
    )).set_parent(parent);
    commands.spawn((
        Text::new(""),
        TextFont { font_size: fonts::SIZE_SMALL, ..default() },
        TextColor(Color::srgb(1.0, 0.85, 0.25)),
        TickInfoText,
    )).set_parent(parent);
    commands.spawn((
        Text::new(""),
        TextFont { font_size: fonts::SIZE_SMALL, ..default() },
        TextColor(Color::srgb(0.30, 0.95, 0.55)),
        MlInfoText,
    )).set_parent(parent);
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

    // Sub-card header: title only (no expand button)
    commands.spawn((
        Text::new(title),
        TextFont { font_size: fonts::SIZE_NORMAL, ..default() },
        TextColor(colors::TEXT_PRIMARY),
    )).set_parent(card);

    // Sub-card content — always visible
    let content = commands.spawn((
        Node {
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(6.0),
            ..default()
        },
    )).id();

    if has_timeframe_btns {
        let btn_row = commands.spawn((
            Node {
                flex_direction: FlexDirection::Row,
                flex_wrap: FlexWrap::Wrap,
                column_gap: Val::Px(8.0),
                row_gap: Val::Px(6.0),
                ..default()
            },
        )).id();
        spawn_timeframe_btn(commands, btn_row, sub_type, BotTimeframe::M1Candles);
        spawn_timeframe_btn(commands, btn_row, sub_type, BotTimeframe::TickData);
        if sub_type == DbSubCardType::HistoryBot || sub_type == DbSubCardType::UpdateHistory {
            spawn_timeframe_btn(commands, btn_row, sub_type, BotTimeframe::MLFeatures);
        }
        commands.entity(content).add_child(btn_row);

        // Status text for each sub-card's progress messages
        if sub_type == DbSubCardType::HistoryBot {
            commands.spawn((
                Text::new(""),
                TextFont { font_size: fonts::SIZE_SMALL, ..default() },
                TextColor(colors::TEXT_MUTED),
                HistoryBotStatusText,
            )).set_parent(content);
        } else if sub_type == DbSubCardType::UpdateHistory {
            commands.spawn((
                Text::new(""),
                TextFont { font_size: fonts::SIZE_SMALL, ..default() },
                TextColor(colors::TEXT_MUTED),
                UpdateHistoryStatusText,
            )).set_parent(content);
        }
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
    let label = match timeframe {
        BotTimeframe::M1Candles => "M1 Candles",
        BotTimeframe::TickData  => "Tick Data",
        BotTimeframe::MLFeatures => "ML Features",
    };

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

/// Show/hide dashboard and update title.
/// Dashboard is visible only when an instrument is selected AND the sidebar is expanded.
pub fn update_dashboard_visibility(
    ui_state: Res<crate::ui::UiState>,
    chart_state: Res<ChartState>,
    mut dashboard_query: Query<&mut Visibility, With<BotDashboard>>,
    mut title_query: Query<&mut Text, With<BotDashboardTitle>>,
) {
    if !chart_state.is_changed() && !ui_state.is_changed() { return; }

    let show = chart_state.selected_instrument.is_some() && ui_state.sidebar_expanded;

    for mut vis in dashboard_query.iter_mut() {
        *vis = if show { Visibility::Visible } else { Visibility::Hidden };
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

/// Hover effect for M1 Candles / Tick Data buttons
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

/// Handle M1 Candles / Tick Data button clicks — sends data requests
pub fn handle_db_timeframe_btn_click(
    chart_state: Res<ChartState>,
    symbol_map: Res<SymbolIdMap>,
    mut dashboard_state: ResMut<BotDashboardState>,
    request_sender: Res<DataRequestSender>,
    query: Query<(&Interaction, &DbTimeframeBtn), Changed<Interaction>>,
) {
    for (interaction, btn) in query.iter() {
        if *interaction != Interaction::Pressed { continue; }

        let symbol = match chart_state.selected_instrument.as_deref() {
            Some(s) => s,
            None => {
                dashboard_state.download_message = Some("No instrument selected.".into());
                return;
            }
        };

        let symbol_id = match symbol_map.name_to_id.get(symbol) {
            Some(&id) => id,
            None => {
                dashboard_state.download_message =
                    Some(format!("Symbol ID not found for {}. Wait for connection.", symbol));
                return;
            }
        };

        // ML Features: check / build the tick_features_m1 table (DB-only, no API)
        if btn.timeframe == BotTimeframe::MLFeatures {
            match btn.parent_card {
                DbSubCardType::HistoryBot => {
                    // Check if already exists; build if not
                    let request = DataRequest {
                        symbol: symbol.to_string(),
                        symbol_id,
                        kind: DataKind::M1Candles,
                        action: DataAction::BuildMLFeatures,
                        force_rebuild: false,
                    };
                    if let Err(e) = request_sender.sender.try_send(request) {
                        dashboard_state.download_message = Some(format!("Failed: {}", e));
                    } else {
                        dashboard_state.download_message =
                            Some(format!("Checking ML features table for {}...", symbol));
                    }
                }
                DbSubCardType::UpdateHistory => {
                    // Incremental rebuild: only adds rows for new M1 candles/ticks
                    let request = DataRequest {
                        symbol: symbol.to_string(),
                        symbol_id,
                        kind: DataKind::M1Candles,
                        action: DataAction::BuildMLFeatures,
                        force_rebuild: true,
                    };
                    if let Err(e) = request_sender.sender.try_send(request) {
                        dashboard_state.update_history_message = Some(format!("Failed: {}", e));
                    } else {
                        dashboard_state.is_update_mode = true;
                        dashboard_state.update_history_message =
                            Some(format!("Updating ML features for {}...", symbol));
                    }
                }
                _ => {}
            }
            continue;
        }

        if dashboard_state.is_downloading {
            dashboard_state.download_message =
                Some("Download already in progress...".into());
            return;
        }

        // For Tick Data: start the bid→ask→merge workflow
        if btn.timeframe == BotTimeframe::TickData {
            let action = match btn.parent_card {
                DbSubCardType::HistoryBot => DataAction::CheckStatus,
                DbSubCardType::UpdateHistory => DataAction::UpdateLatest,
                _ => return,
            };

            if action == DataAction::CheckStatus {
                // History BoT: check bid ticks first, download if missing
                dashboard_state.tick_workflow = TickWorkflowStep::CheckingBid;
                let request = DataRequest {
                    symbol: symbol.to_string(),
                    symbol_id,
                    kind: DataKind::TickData,
                    action: DataAction::CheckStatus,
                    force_rebuild: false,
                };
                if let Err(e) = request_sender.sender.try_send(request) {
                    dashboard_state.download_message = Some(format!("Failed: {}", e));
                    dashboard_state.tick_workflow = TickWorkflowStep::Idle;
                    return;
                }
                dashboard_state.download_message =
                    Some(format!("Checking {} bid ticks in database...", symbol));
            } else {
                // Update History: check last bid tick timestamp before updating
                dashboard_state.tick_workflow = TickWorkflowStep::CheckingBidForUpdate;
                dashboard_state.is_update_mode = true;
                let request = DataRequest {
                    symbol: symbol.to_string(),
                    symbol_id,
                    kind: DataKind::TickData,
                    action: DataAction::CheckStatus,
                    force_rebuild: false,
                };
                if let Err(e) = request_sender.sender.try_send(request) {
                    dashboard_state.update_history_message = Some(format!("Failed: {}", e));
                    dashboard_state.tick_workflow = TickWorkflowStep::Idle;
                    dashboard_state.is_update_mode = false;
                    return;
                }
                dashboard_state.update_history_message =
                    Some(format!("Checking {} bid ticks in database...", symbol));
            }
            return;
        }

        // For M1 Candles: simple flow (unchanged)
        let kind = DataKind::M1Candles;
        let action = match btn.parent_card {
            DbSubCardType::HistoryBot => DataAction::CheckStatus,
            DbSubCardType::UpdateHistory => DataAction::UpdateLatest,
            _ => return,
        };

        let request = DataRequest {
            symbol: symbol.to_string(),
            symbol_id,
            kind,
            action,
            force_rebuild: false,
        };

        if let Err(e) = request_sender.sender.try_send(request) {
            println!("Failed to send data request: {}", e);
            dashboard_state.data_status_message =
                Some(format!("Failed to send request: {}", e));
            return;
        }

        match action {
            DataAction::CheckStatus => {
                dashboard_state.is_update_mode = false;
                dashboard_state.download_message =
                    Some(format!("Checking {} M1 Candles in database...", symbol));
            }
            DataAction::UpdateLatest => {
                dashboard_state.is_update_mode = true;
                dashboard_state.is_downloading = true;
                dashboard_state.download_progress = 0;
                dashboard_state.update_history_message =
                    Some(format!("Updating {} M1 history...", symbol));
            }
            _ => {}
        }
    }
}

/// Process data responses from the network task and update dashboard state.
/// For tick data, orchestrates the bid → ask → merge workflow via `TickWorkflowStep`.
pub fn process_data_responses(
    mut receiver: ResMut<DataResponseReceiver>,
    mut dashboard_state: ResMut<BotDashboardState>,
    symbol_map: Res<SymbolIdMap>,
    request_sender: Res<DataRequestSender>,
) {
    // Process one message per frame so each progress update is visible in the UI
    if let Ok(response) = receiver.receiver.try_recv() {
        let workflow = dashboard_state.tick_workflow;

        match response {
            DataResponse::StatusFound { symbol, kind, count, first_record, last_record, .. } => {
                match kind {
                    DataKind::M1Candles => {
                        let info = format!("{} M1 candles in DB\n{}\n{}", count, first_record, last_record);
                        dashboard_state.m1_info = Some(info);
                        dashboard_state.is_downloading = false;

                        if dashboard_state.is_update_mode {
                            // After Update History M1 complete — append last candle info
                            let rows = dashboard_state.pending_update_rows;
                            dashboard_state.update_history_message = Some(format!(
                                "Update successful! +{} new M1 candles.\n{}",
                                rows, last_record
                            ));
                            dashboard_state.is_update_mode = false;
                            dashboard_state.pending_update_rows = 0;
                        } else {
                            dashboard_state.download_message = Some(format!(
                                "{} M1 candles: {} found. Consider 'Update History'.", symbol, count
                            ));
                        }
                    }
                    DataKind::TickData if workflow == TickWorkflowStep::CheckingBid => {
                        // History BoT: bid exists → check ask
                        dashboard_state.download_message = Some(format!(
                            "Bid ticks in DB ({} rows). Checking ask ticks...", count
                        ));
                        dashboard_state.tick_workflow = TickWorkflowStep::CheckingAsk;
                        if let Some(&symbol_id) = symbol_map.name_to_id.get(&symbol) {
                            let _ = request_sender.sender.try_send(DataRequest {
                                symbol, symbol_id,
                                kind: DataKind::TickDataAsk,
                                action: DataAction::CheckStatus,
                                force_rebuild: false,
                            });
                        }
                    }
                    DataKind::TickDataAsk if workflow == TickWorkflowStep::CheckingAsk => {
                        // History BoT: both bid and ask exist → merge (skip if already merged)
                        dashboard_state.download_message = Some(
                            "Bid and ask ticks in DB. Merging in process...".into()
                        );
                        dashboard_state.tick_workflow = TickWorkflowStep::Merging;
                        if let Some(&symbol_id) = symbol_map.name_to_id.get(&symbol) {
                            let _ = request_sender.sender.try_send(DataRequest {
                                symbol, symbol_id,
                                kind: DataKind::TickData,
                                action: DataAction::MergeBidAsk,
                                force_rebuild: false,
                            });
                        }
                    }
                    // ── Update History: check bid last timestamp ──────────────────────────────
                    DataKind::TickData if workflow == TickWorkflowStep::CheckingBidForUpdate => {
                        // Show last bid tick, then check ask timestamp
                        dashboard_state.update_history_message = Some(format!(
                            "{}\nChecking ask ticks...", last_record
                        ));
                        dashboard_state.tick_workflow = TickWorkflowStep::CheckingAskForUpdate;
                        if let Some(&symbol_id) = symbol_map.name_to_id.get(&symbol) {
                            let _ = request_sender.sender.try_send(DataRequest {
                                symbol, symbol_id,
                                kind: DataKind::TickDataAsk,
                                action: DataAction::CheckStatus,
                                force_rebuild: false,
                            });
                        }
                    }
                    // ── Update History: check ask last timestamp ──────────────────────────────
                    DataKind::TickDataAsk if workflow == TickWorkflowStep::CheckingAskForUpdate => {
                        // Show last ask tick, then start UpdateLatest for bid
                        dashboard_state.update_history_message = Some(format!(
                            "{}\nFetching new bid ticks...", last_record
                        ));
                        dashboard_state.tick_workflow = TickWorkflowStep::UpdatingBid;
                        dashboard_state.is_downloading = true;
                        dashboard_state.download_progress = 0;
                        if let Some(&symbol_id) = symbol_map.name_to_id.get(&symbol) {
                            let _ = request_sender.sender.try_send(DataRequest {
                                symbol, symbol_id,
                                kind: DataKind::TickData,
                                action: DataAction::UpdateLatest,
                                force_rebuild: false,
                            });
                        }
                    }
                    _ => {
                        // Generic status found (non-workflow)
                        let kind_label = match kind {
                            DataKind::M1Candles => "M1 candles",
                            DataKind::TickData => "bid ticks",
                            DataKind::TickDataAsk => "ask ticks",
                        };
                        let info = format!("{} {} in DB\n{}\n{}", count, kind_label, first_record, last_record);
                        if kind == DataKind::TickData || kind == DataKind::TickDataAsk {
                            dashboard_state.tick_info = Some(info);
                        }
                        dashboard_state.download_message = Some(format!(
                            "{} {}: {} found.", symbol, kind_label, count
                        ));
                        dashboard_state.is_downloading = false;
                    }
                }
            }
            DataResponse::StatusEmpty { symbol, kind } => {
                match kind {
                    DataKind::M1Candles => {
                        dashboard_state.download_message = Some(format!(
                            "{} M1 candles: nothing in DB. Starting full download...", symbol
                        ));
                        if let Some(&symbol_id) = symbol_map.name_to_id.get(&symbol) {
                            let _ = request_sender.sender.try_send(DataRequest {
                                symbol, symbol_id, kind,
                                action: DataAction::RetrieveFull,
                                force_rebuild: false,
                            });
                            dashboard_state.is_downloading = true;
                            dashboard_state.download_progress = 0;
                        }
                    }
                    DataKind::TickData if workflow == TickWorkflowStep::CheckingBid => {
                        // No bid ticks → download full history
                        dashboard_state.download_message = Some(format!(
                            "No bid ticks found for {}. Starting bid download...", symbol
                        ));
                        dashboard_state.tick_workflow = TickWorkflowStep::DownloadingBid;
                        dashboard_state.is_downloading = true;
                        dashboard_state.download_progress = 0;
                        if let Some(&symbol_id) = symbol_map.name_to_id.get(&symbol) {
                            let _ = request_sender.sender.try_send(DataRequest {
                                symbol, symbol_id,
                                kind: DataKind::TickData,
                                action: DataAction::RetrieveFull,
                                force_rebuild: false,
                            });
                        }
                    }
                    DataKind::TickDataAsk if workflow == TickWorkflowStep::CheckingAsk => {
                        // No ask ticks → download full history
                        dashboard_state.download_message = Some(format!(
                            "No ask ticks found for {}. Starting ask download...", symbol
                        ));
                        dashboard_state.tick_workflow = TickWorkflowStep::DownloadingAsk;
                        dashboard_state.is_downloading = true;
                        dashboard_state.download_progress = 0;
                        if let Some(&symbol_id) = symbol_map.name_to_id.get(&symbol) {
                            let _ = request_sender.sender.try_send(DataRequest {
                                symbol, symbol_id,
                                kind: DataKind::TickDataAsk,
                                action: DataAction::RetrieveFull,
                                force_rebuild: false,
                            });
                        }
                    }
                    // ── Update History: no bid ticks in DB → abort ───────────────────────────
                    DataKind::TickData if workflow == TickWorkflowStep::CheckingBidForUpdate => {
                        dashboard_state.update_history_message = Some(format!(
                            "No bid ticks in DB for {}. Download from History BoT first.", symbol
                        ));
                        dashboard_state.tick_workflow = TickWorkflowStep::Idle;
                        dashboard_state.is_update_mode = false;
                    }
                    // ── Update History: no ask ticks in DB → abort ───────────────────────────
                    DataKind::TickDataAsk if workflow == TickWorkflowStep::CheckingAskForUpdate => {
                        dashboard_state.update_history_message = Some(format!(
                            "No ask ticks in DB for {}. Download from History BoT first.", symbol
                        ));
                        dashboard_state.tick_workflow = TickWorkflowStep::Idle;
                        dashboard_state.is_update_mode = false;
                    }
                    _ => {
                        let kind_label = match kind {
                            DataKind::M1Candles => "M1 candles",
                            DataKind::TickData => "bid ticks",
                            DataKind::TickDataAsk => "ask ticks",
                        };
                        dashboard_state.download_message = Some(format!(
                            "{} {}: nothing in DB.", symbol, kind_label
                        ));
                    }
                }
            }
            DataResponse::Progress { downloaded_rows, message, .. } => {
                dashboard_state.download_progress = downloaded_rows;
                if dashboard_state.is_update_mode {
                    dashboard_state.update_history_message = Some(message);
                } else {
                    dashboard_state.download_message = Some(message);
                }
            }
            DataResponse::Complete { symbol, kind, total_rows } => {
                match kind {
                    DataKind::M1Candles => {
                        dashboard_state.is_downloading = false;
                        dashboard_state.download_progress = 0;
                        if dashboard_state.is_update_mode {
                            // Update History: store row count, auto-check to get last candle info
                            dashboard_state.pending_update_rows = total_rows;
                            dashboard_state.update_history_message = Some(format!(
                                "Update successful! +{} new M1 candles.", total_rows
                            ));
                            // Keep is_update_mode = true so StatusFound routes here too
                            if let Some(&symbol_id) = symbol_map.name_to_id.get(&symbol) {
                                let _ = request_sender.sender.try_send(DataRequest {
                                    symbol, symbol_id,
                                    kind: DataKind::M1Candles,
                                    action: DataAction::CheckStatus,
                                    force_rebuild: false,
                                });
                            }
                        } else {
                            // History BoT full download: show info and auto-check first/last
                            dashboard_state.download_message = Some(format!(
                                "{} M1 candles downloaded. {} rows stored.", symbol, total_rows
                            ));
                            if let Some(&symbol_id) = symbol_map.name_to_id.get(&symbol) {
                                let _ = request_sender.sender.try_send(DataRequest {
                                    symbol, symbol_id, kind,
                                    action: DataAction::CheckStatus,
                                    force_rebuild: false,
                                });
                            }
                        }
                    }
                    // ── History BoT full-download paths ────────────────────────────────────
                    DataKind::TickData if workflow == TickWorkflowStep::DownloadingBid => {
                        // Full bid download complete → check if ask exists
                        dashboard_state.download_message = Some(format!(
                            "Bid ticks downloaded ({} rows). Checking ask ticks...", total_rows
                        ));
                        dashboard_state.is_downloading = false;
                        dashboard_state.download_progress = 0;
                        dashboard_state.tick_workflow = TickWorkflowStep::CheckingAsk;
                        if let Some(&symbol_id) = symbol_map.name_to_id.get(&symbol) {
                            let _ = request_sender.sender.try_send(DataRequest {
                                symbol, symbol_id,
                                kind: DataKind::TickDataAsk,
                                action: DataAction::CheckStatus,
                                force_rebuild: false,
                            });
                        }
                    }
                    DataKind::TickDataAsk if workflow == TickWorkflowStep::DownloadingAsk => {
                        // Full ask download complete → merge (no force, first-time build)
                        dashboard_state.download_message = Some(format!(
                            "Ask ticks downloaded ({} rows). Merging bid + ask...", total_rows
                        ));
                        dashboard_state.tick_workflow = TickWorkflowStep::Merging;
                        dashboard_state.is_downloading = true;
                        if let Some(&symbol_id) = symbol_map.name_to_id.get(&symbol) {
                            let _ = request_sender.sender.try_send(DataRequest {
                                symbol, symbol_id,
                                kind: DataKind::TickData,
                                action: DataAction::MergeBidAsk,
                                force_rebuild: false,
                            });
                        }
                    }
                    // ── Update History paths ────────────────────────────────────────────────
                    DataKind::TickData if workflow == TickWorkflowStep::UpdatingBid => {
                        // Bid update complete → now update ask too
                        dashboard_state.update_history_message = Some(format!(
                            "Bid ticks updated (+{} rows). Updating ask ticks...", total_rows
                        ));
                        dashboard_state.download_progress = 0;
                        dashboard_state.tick_workflow = TickWorkflowStep::UpdatingAsk;
                        if let Some(&symbol_id) = symbol_map.name_to_id.get(&symbol) {
                            let _ = request_sender.sender.try_send(DataRequest {
                                symbol, symbol_id,
                                kind: DataKind::TickDataAsk,
                                action: DataAction::UpdateLatest,
                                force_rebuild: false,
                            });
                        }
                    }
                    DataKind::TickDataAsk if workflow == TickWorkflowStep::UpdatingAsk => {
                        // Ask update complete → force-rebuild merged table with new ticks
                        dashboard_state.update_history_message = Some(format!(
                            "Ask ticks updated (+{} rows). Rebuilding merged table...", total_rows
                        ));
                        dashboard_state.tick_workflow = TickWorkflowStep::Merging;
                        dashboard_state.is_downloading = true;
                        if let Some(&symbol_id) = symbol_map.name_to_id.get(&symbol) {
                            let _ = request_sender.sender.try_send(DataRequest {
                                symbol, symbol_id,
                                kind: DataKind::TickData,
                                action: DataAction::MergeBidAsk,
                                force_rebuild: true,  // always rebuild after update
                            });
                        }
                    }
                    _ => {
                        // Generic complete (non-workflow)
                        let kind_label = match kind {
                            DataKind::M1Candles => "M1 candles",
                            DataKind::TickData => "bid ticks",
                            DataKind::TickDataAsk => "ask ticks",
                        };
                        dashboard_state.download_message = Some(format!(
                            "{} {} downloaded. {} rows stored.", symbol, kind_label, total_rows
                        ));
                        dashboard_state.is_downloading = false;
                        dashboard_state.download_progress = 0;
                    }
                }
            }
            DataResponse::MergeComplete { symbol, total_rows, new_rows, first_record, last_record, already_exists } => {
                let was_update = dashboard_state.is_update_mode;
                dashboard_state.tick_workflow = TickWorkflowStep::Idle;
                dashboard_state.is_downloading = false;
                dashboard_state.download_progress = 0;
                dashboard_state.is_update_mode = false;

                if was_update {
                    // Update History path: show success in Update History sub-card only
                    dashboard_state.update_history_message = Some(format!(
                        "Merge complete! +{} new rows ({} total).", new_rows, total_rows
                    ));
                } else {
                    // History BoT path: show in History BoT status
                    dashboard_state.download_message = Some(if already_exists {
                        format!(
                            "Already merged ({} rows). Consider 'Update History' to refresh.",
                            total_rows
                        )
                    } else {
                        format!(
                            "Merge complete! {} rows in {}_ticks_merged",
                            total_rows, symbol.to_lowercase()
                        )
                    });
                }
                dashboard_state.tick_info = Some(format!(
                    "{} merged ticks\n{}\n{}", total_rows, first_record, last_record
                ));
            }
            DataResponse::MLFeaturesComplete { symbol, total_rows, new_rows, already_exists, first_record, last_record } => {
                let was_update = dashboard_state.is_update_mode;
                dashboard_state.is_update_mode = false;

                // Always update the green ML info line in the status area
                dashboard_state.ml_features_info = Some(format!(
                    "ML Features: {} rows\n{}\n{}", total_rows, first_record, last_record
                ));

                if was_update {
                    dashboard_state.update_history_message = Some(format!(
                        "ML features updated! +{} new rows ({} total).", new_rows, total_rows
                    ));
                } else {
                    let msg = if already_exists {
                        format!(
                            "Already prepared for ML ({} rows).", total_rows
                        )
                    } else {
                        format!(
                            "{} ML features table built! {} rows ready for training.",
                            symbol, total_rows
                        )
                    };
                    dashboard_state.download_message = Some(msg);
                }
            }
            DataResponse::Error { symbol, message, .. } => {
                dashboard_state.download_message = Some(format!(
                    "Error for {}: {}", symbol, message
                ));
                dashboard_state.is_downloading = false;
                dashboard_state.tick_workflow = TickWorkflowStep::Idle;
            }
        }
    }
}

/// Update the History BoT sub-card status text (download progress)
pub fn update_history_bot_status(
    dashboard_state: Res<BotDashboardState>,
    mut query: Query<&mut Text, With<HistoryBotStatusText>>,
) {
    if !dashboard_state.is_changed() { return; }

    let msg = dashboard_state.download_message.as_deref().unwrap_or("");
    for mut text in query.iter_mut() {
        if text.0 != msg {
            text.0 = msg.to_string();
        }
    }
}

/// Update the M1 candle info line (light blue)
pub fn update_m1_info_text(
    dashboard_state: Res<BotDashboardState>,
    mut query: Query<&mut Text, With<M1InfoText>>,
) {
    if !dashboard_state.is_changed() { return; }
    let msg = dashboard_state.m1_info.as_deref().unwrap_or("");
    for mut text in query.iter_mut() {
        if text.0 != msg { text.0 = msg.to_string(); }
    }
}

/// Update the merged tick info line (yellow)
pub fn update_tick_info_text(
    dashboard_state: Res<BotDashboardState>,
    mut query: Query<&mut Text, With<TickInfoText>>,
) {
    if !dashboard_state.is_changed() { return; }
    let msg = dashboard_state.tick_info.as_deref().unwrap_or("");
    for mut text in query.iter_mut() {
        if text.0 != msg { text.0 = msg.to_string(); }
    }
}

/// Update the ML features info line (green)
pub fn update_ml_info_text(
    dashboard_state: Res<BotDashboardState>,
    mut query: Query<&mut Text, With<MlInfoText>>,
) {
    if !dashboard_state.is_changed() { return; }
    let msg = dashboard_state.ml_features_info.as_deref().unwrap_or("");
    for mut text in query.iter_mut() {
        if text.0 != msg { text.0 = msg.to_string(); }
    }
}

/// Update the Update History sub-card status text
pub fn update_update_history_status(
    dashboard_state: Res<BotDashboardState>,
    mut query: Query<&mut Text, With<UpdateHistoryStatusText>>,
) {
    if !dashboard_state.is_changed() { return; }

    let msg = dashboard_state.update_history_message.as_deref().unwrap_or("");
    for mut text in query.iter_mut() {
        if text.0 != msg {
            text.0 = msg.to_string();
        }
    }
}
