//! Bot dashboard widget — accordion-expand cards for the selected instrument

use bevy::prelude::*;
use crate::ui::bevy_ui::{
    BotDashboard, BotDashboardTitle,
    TopCardType, CardsContainer, MainCard, MainCardContent,
    MainCardMaxBtn, MainCardMaxBtnIcon,
    DbSubCard, DbSubCardType,
    DbTimeframeBtn, HistoryBotStatusText, UpdateHistoryStatusText, BotTimeframe,
    M1InfoText, TickInfoText, MlInfoText,
    MlSubCardType, MlSubCard, MlBtnType, MlModelBtn, MlModelInfoText,
    MlInfoScrollArea, MlScrollbarThumb,
    CrossPairBtn, CrossPairStatusText, CROSS_PAIRS,
    CrossPairUpdateBtn, CrossPairUpdateStatusText,
    BotDashboardState, TickWorkflowStep, MlTrainState,
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
    spawn_main_card(commands, cards, TopCardType::TrainModel,    "Train Model",     true);
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

    if card_type == TopCardType::Database {
        spawn_database_content(commands, content);
    } else if card_type == TopCardType::TrainModel {
        spawn_train_model_content(commands, content);
    } else {
        // Placeholder content for other cards
        let placeholder = match card_type {
            TopCardType::StartPause    => "Bot is stopped.",
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

/// Spawn the content inside the Train Model accordion card.
/// Mirrors the Database card layout: a 2-column wrapping grid of model sub-cards.
fn spawn_train_model_content(commands: &mut Commands, parent: Entity) {
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

    let model_titles = [
        "Model 1",
        "Model 2",
        "Model 3",
        "Model 4",
        "Model 5",
        "Model 6",
    ];

    for model in MlSubCardType::all() {
        spawn_ml_subcard(commands, grid, model, model_titles[model as usize]);
    }

    commands.entity(parent).add_child(grid);
}

/// Spawn one ML model sub-card (mirrors spawn_db_subcard layout).
fn spawn_ml_subcard(
    commands: &mut Commands,
    parent: Entity,
    model: MlSubCardType,
    title: &str,
) {
    let card = commands.spawn((
        Node {
            flex_basis: Val::Percent(47.0),
            flex_grow: 1.0,
            min_height: Val::Px(90.0),
            flex_direction: FlexDirection::Column,
            padding: UiRect::all(Val::Px(10.0)),
            row_gap: Val::Px(6.0),
            ..default()
        },
        BackgroundColor(colors::BG_SIDEBAR),
        BorderRadius::all(Val::Px(6.0)),
        MlSubCard { sub_type: model },
    )).id();

    // Title
    commands.spawn((
        Text::new(title),
        TextFont { font_size: fonts::SIZE_NORMAL, ..default() },
        TextColor(colors::TEXT_PRIMARY),
    )).set_parent(card);

    // Button row: [Status] [Update Training] [*]
    let btn_row = commands.spawn((
        Node {
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            column_gap: Val::Px(6.0),
            row_gap: Val::Px(4.0),
            ..default()
        },
    )).id();

    let status_btn = commands.spawn((
        Node {
            padding: UiRect::axes(Val::Px(10.0), Val::Px(4.0)),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        BackgroundColor(colors::BG_BUTTON),
        BorderRadius::all(Val::Px(4.0)),
        Interaction::default(),
        MlModelBtn { model, btn_type: MlBtnType::Status },
    )).id();
    commands.spawn((
        Text::new("Status"),
        TextFont { font_size: fonts::SIZE_SMALL, ..default() },
        TextColor(colors::TEXT_PRIMARY),
    )).set_parent(status_btn);

    let train_btn = commands.spawn((
        Node {
            padding: UiRect::axes(Val::Px(10.0), Val::Px(4.0)),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        BackgroundColor(colors::BG_BUTTON),
        BorderRadius::all(Val::Px(4.0)),
        Interaction::default(),
        MlModelBtn { model, btn_type: MlBtnType::Train },
    )).id();
    commands.spawn((
        Text::new("Update Training"),
        TextFont { font_size: fonts::SIZE_SMALL, ..default() },
        TextColor(colors::TEXT_PRIMARY),
    )).set_parent(train_btn);

    let feat_btn = commands.spawn((
        Node {
            padding: UiRect::axes(Val::Px(10.0), Val::Px(4.0)),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        BackgroundColor(colors::BG_BUTTON),
        BorderRadius::all(Val::Px(4.0)),
        Interaction::default(),
        MlModelBtn { model, btn_type: MlBtnType::FeatureCount },
    )).id();
    commands.spawn((
        Text::new("Feature Count"),
        TextFont { font_size: fonts::SIZE_SMALL, ..default() },
        TextColor(colors::TEXT_PRIMARY),
    )).set_parent(feat_btn);

    commands.entity(btn_row).add_children(&[status_btn, train_btn, feat_btn]);
    commands.entity(card).add_child(btn_row);

    // Scrollable info area: [scroll_content | scrollbar_track]
    let scroll_wrapper = commands.spawn((
        Node {
            flex_direction: FlexDirection::Row,
            width: Val::Percent(100.0),
            max_height: Val::Px(220.0),
            column_gap: Val::Px(3.0),
            ..default()
        },
    )).id();

    let scroll_area = commands.spawn((
        Node {
            flex_direction: FlexDirection::Column,
            overflow: Overflow::scroll_y(),
            flex_grow: 1.0,
            ..default()
        },
        ScrollPosition::default(),
        Interaction::default(),
        MlInfoScrollArea { model },
    )).id();
    commands.spawn((
        Text::new("Press Status to check model info, or Update Training to retrain."),
        TextFont { font_size: fonts::SIZE_SMALL, ..default() },
        TextColor(colors::TEXT_MUTED),
        MlModelInfoText { model },
    )).set_parent(scroll_area);

    // Scrollbar track
    let track = commands.spawn((
        Node {
            width: Val::Px(4.0),
            height: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            overflow: Overflow::clip(),
            ..default()
        },
        BackgroundColor(colors::BG_DARK),
        BorderRadius::all(Val::Px(2.0)),
    )).id();

    // Scrollbar thumb (absolute-positioned inside track)
    let thumb = commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Px(40.0),
            top: Val::Px(0.0),
            ..default()
        },
        BackgroundColor(colors::BG_BUTTON_ACTIVE),
        BorderRadius::all(Val::Px(2.0)),
        Interaction::default(),
        MlScrollbarThumb { model },
    )).id();

    commands.entity(track).add_child(thumb);
    commands.entity(scroll_wrapper).add_children(&[scroll_area, track]);
    commands.entity(card).add_child(scroll_wrapper);

    commands.entity(parent).add_child(card);
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
            spawn_cross_pair_section(commands, content);
        } else if sub_type == DbSubCardType::UpdateHistory {
            commands.spawn((
                Text::new(""),
                TextFont { font_size: fonts::SIZE_SMALL, ..default() },
                TextColor(colors::TEXT_MUTED),
                UpdateHistoryStatusText,
            )).set_parent(content);
            spawn_cross_pair_update_section(commands, content);
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

/// Spawn the cross-pair M1 section inside the History BoT sub-card content.
/// Adds a label + one row per pair in CROSS_PAIRS.
fn spawn_cross_pair_section(commands: &mut Commands, parent: Entity) {
    commands.spawn((
        Text::new("Cross-Pair M1 Data:"),
        TextFont { font_size: fonts::SIZE_SMALL, ..default() },
        TextColor(colors::TEXT_MUTED),
    )).set_parent(parent);

    for &symbol in CROSS_PAIRS.iter() {
        spawn_cross_pair_row(commands, parent, symbol);
    }
}

/// Spawn one cross-pair row: [SYMBOL btn] [status text]
fn spawn_cross_pair_row(commands: &mut Commands, parent: Entity, symbol: &'static str) {
    let row = commands.spawn((
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(8.0),
            ..default()
        },
    )).id();

    let btn = commands.spawn((
        Node {
            padding: UiRect::axes(Val::Px(8.0), Val::Px(3.0)),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            min_width: Val::Px(72.0),
            ..default()
        },
        BackgroundColor(colors::BG_BUTTON),
        BorderRadius::all(Val::Px(4.0)),
        Interaction::default(),
        CrossPairBtn { symbol },
    )).id();

    commands.spawn((
        Text::new(symbol),
        TextFont { font_size: fonts::SIZE_SMALL, ..default() },
        TextColor(colors::TEXT_PRIMARY),
    )).set_parent(btn);

    let status = commands.spawn((
        Text::new("--"),
        TextFont { font_size: fonts::SIZE_SMALL, ..default() },
        TextColor(colors::TEXT_MUTED),
        CrossPairStatusText { symbol },
    )).id();

    commands.entity(row).add_children(&[btn, status]);
    commands.entity(parent).add_child(row);
}

/// Spawn the cross-pair update section inside the Update History sub-card content.
fn spawn_cross_pair_update_section(commands: &mut Commands, parent: Entity) {
    commands.spawn((
        Text::new("Cross-Pair M1 Update:"),
        TextFont { font_size: fonts::SIZE_SMALL, ..default() },
        TextColor(colors::TEXT_MUTED),
    )).set_parent(parent);

    for &symbol in CROSS_PAIRS.iter() {
        spawn_cross_pair_update_row(commands, parent, symbol);
    }
}

/// Spawn one cross-pair update row: [SYMBOL btn] [status text]
fn spawn_cross_pair_update_row(commands: &mut Commands, parent: Entity, symbol: &'static str) {
    let row = commands.spawn((
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(8.0),
            ..default()
        },
    )).id();

    let btn = commands.spawn((
        Node {
            padding: UiRect::axes(Val::Px(8.0), Val::Px(3.0)),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            min_width: Val::Px(72.0),
            ..default()
        },
        BackgroundColor(colors::BG_BUTTON),
        BorderRadius::all(Val::Px(4.0)),
        Interaction::default(),
        CrossPairUpdateBtn { symbol },
    )).id();

    commands.spawn((
        Text::new(symbol),
        TextFont { font_size: fonts::SIZE_SMALL, ..default() },
        TextColor(colors::TEXT_PRIMARY),
    )).set_parent(btn);

    let status = commands.spawn((
        Text::new("--"),
        TextFont { font_size: fonts::SIZE_SMALL, ..default() },
        TextColor(colors::TEXT_MUTED),
        CrossPairUpdateStatusText { symbol },
    )).id();

    commands.entity(row).add_children(&[btn, status]);
    commands.entity(parent).add_child(row);
}

// ============================================================================
// Systems
// ============================================================================

/// Show/hide dashboard and update title.
/// Dashboard is visible only when an instrument is selected AND the sidebar is expanded.
pub fn update_dashboard_visibility(
    ui_state: Res<crate::ui::UiState>,
    mut dashboard_query: Query<&mut Visibility, With<BotDashboard>>,
    mut title_query: Query<&mut Text, With<BotDashboardTitle>>,
) {
    if !ui_state.is_changed() { return; }

    let show = ui_state.selected_instrument.is_some() && ui_state.sidebar_expanded;

    for mut vis in dashboard_query.iter_mut() {
        *vis = if show { Visibility::Visible } else { Visibility::Hidden };
    }

    if let Some(ref symbol) = ui_state.selected_instrument {
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

    // Database and Train Model content always visible; others show only when expanded
    for (content, mut node) in content_query.iter_mut() {
        node.display = if content.card_type == TopCardType::Database
            || content.card_type == TopCardType::TrainModel
        {
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
    ui_state: Res<crate::ui::UiState>,
    symbol_map: Res<SymbolIdMap>,
    mut dashboard_state: ResMut<BotDashboardState>,
    request_sender: Res<DataRequestSender>,
    query: Query<(&Interaction, &DbTimeframeBtn), Changed<Interaction>>,
) {
    for (interaction, btn) in query.iter() {
        if *interaction != Interaction::Pressed { continue; }

        let symbol = match ui_state.selected_instrument.as_deref() {
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

fn is_cross_pair(symbol: &str) -> bool {
    CROSS_PAIRS.contains(&symbol)
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
                    DataKind::M1Candles if is_cross_pair(&symbol) => {
                        // Cross-pair: route to the per-symbol status map
                        let msg = format!(
                            "{} M1 bars\n{}\n{}\nConsider updating",
                            count, first_record, last_record
                        );
                        dashboard_state.cross_pair_status.insert(symbol, msg);
                    }
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
                    DataKind::M1Candles if is_cross_pair(&symbol) => {
                        // Cross-pair: auto-start download
                        dashboard_state.cross_pair_status.insert(
                            symbol.clone(),
                            "Downloading...".to_string(),
                        );
                        if let Some(&symbol_id) = symbol_map.name_to_id.get(&symbol) {
                            let _ = request_sender.sender.try_send(DataRequest {
                                symbol, symbol_id, kind,
                                action: DataAction::RetrieveFull,
                                force_rebuild: false,
                            });
                        }
                    }
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
            DataResponse::Progress { symbol, downloaded_rows, message, .. } => {
                dashboard_state.download_progress = downloaded_rows;
                if dashboard_state.updating_cross_pairs.contains(&symbol) {
                    dashboard_state.cross_pair_update_status.insert(symbol, message);
                } else if is_cross_pair(&symbol) {
                    dashboard_state.cross_pair_status.insert(symbol, message);
                } else if dashboard_state.is_update_mode {
                    dashboard_state.update_history_message = Some(message);
                } else {
                    dashboard_state.download_message = Some(message);
                }
            }
            DataResponse::Complete { symbol, kind, total_rows } => {
                match kind {
                    DataKind::M1Candles if dashboard_state.updating_cross_pairs.contains(&symbol) => {
                        // Cross-pair update complete: show row count, remove from active set
                        dashboard_state.cross_pair_update_status.insert(
                            symbol.clone(),
                            format!("+{} new M1 bars", total_rows),
                        );
                        dashboard_state.updating_cross_pairs.remove(&symbol);
                    }
                    DataKind::M1Candles if is_cross_pair(&symbol) => {
                        // Cross-pair full download complete: auto-check to populate first/last dates
                        dashboard_state.cross_pair_status.insert(
                            symbol.clone(),
                            format!("Done ({} rows). Loading info...", total_rows),
                        );
                        if let Some(&symbol_id) = symbol_map.name_to_id.get(&symbol) {
                            let _ = request_sender.sender.try_send(DataRequest {
                                symbol, symbol_id,
                                kind: DataKind::M1Candles,
                                action: DataAction::CheckStatus,
                                force_rebuild: false,
                            });
                        }
                    }
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

// ============================================================================
// Model 1 systems
// ============================================================================

/// Handle Status and Update Training button clicks for Model 1.
/// Handle Status / Update Training button clicks for any ML model sub-card.
pub fn handle_ml_model_btn_click(
    mut state: ResMut<MlTrainState>,
    query: Query<(&Interaction, &MlModelBtn), Changed<Interaction>>,
) {
    for (interaction, btn) in query.iter() {
        if *interaction != Interaction::Pressed { continue; }

        match btn.btn_type {
            MlBtnType::Status => {
                let last = state.get(btn.model).last_trained.clone();
                let mut text = read_ml_model_status(btn.model);
                if let Some(ts) = last {
                    text.push_str(&format!("\n\nLast trained: {}", ts));
                }
                state.get_mut(btn.model).status_text = text;
            }
            MlBtnType::FeatureCount => {
                let text = read_ml_model_features(btn.model);
                state.get_mut(btn.model).status_text = text;
            }
            MlBtnType::Train => {
                let model_state = state.get_mut(btn.model);
                if model_state.is_training {
                    model_state.status_text = "Training already in progress...".to_string();
                    continue;
                }

                let module = match btn.model {
                    MlSubCardType::Model1 => Some("ml.model1_technical.train"),
                    MlSubCardType::Model2 => Some("ml.model2_regime.train"),
                    _ => None,
                };

                if let Some(module_path) = module {
                    let (tx, rx) = std::sync::mpsc::channel::<String>();
                    model_state.is_training = true;
                    model_state.status_text = "Starting training...\n".to_string();
                    model_state.training_rx = Some(std::sync::Mutex::new(rx));

                    let module_path = module_path.to_string();
                    std::thread::spawn(move || {
                        let python = "C:/Users/kushn/AppData/Local/Programs/Python/Python314/python.exe";
                        let result = std::process::Command::new(python)
                            .args(["-u", "-m", &module_path])   // -u = unbuffered stdout
                            .current_dir("C:/Users/kushn/RustProjects/ctrader_rust")
                            .env("PYTHONIOENCODING", "utf-8")   // fix Windows pipe encoding (Errno 22)
                            .stdout(std::process::Stdio::piped())
                            .stderr(std::process::Stdio::piped())
                            .spawn();

                        match result {
                            Err(e) => {
                                let _ = tx.send(format!("ERROR: failed to start Python: {}", e));
                                let _ = tx.send("__DONE__".to_string());
                            }
                            Ok(mut child) => {
                                use std::io::BufRead;
                                // Stream stdout live
                                if let Some(stdout) = child.stdout.take() {
                                    let reader = std::io::BufReader::new(stdout);
                                    for line in reader.lines() {
                                        match line {
                                            Ok(l)  => { let _ = tx.send(l); }
                                            Err(_) => break,
                                        }
                                    }
                                }
                                // Capture stderr (errors / tracebacks)
                                if let Some(stderr) = child.stderr.take() {
                                    let reader = std::io::BufReader::new(stderr);
                                    for line in reader.lines().flatten() {
                                        if !line.trim().is_empty() {
                                            let _ = tx.send(format!("ERR: {}", line));
                                        }
                                    }
                                }
                                let status = child.wait().unwrap_or_else(|_| {
                                    std::process::ExitStatus::default()
                                });
                                let _ = tx.send(format!(
                                    "Training finished (exit code: {})",
                                    status.code().unwrap_or(-1)
                                ));
                                let _ = tx.send("__DONE__".to_string());
                            }
                        }
                    });
                } else {
                    model_state.status_text =
                        "Training script not yet implemented for this model.".to_string();
                }
            }
        }
    }
}

/// Poll all model training threads each frame; append stdout to status_text.
pub fn poll_ml_training(mut state: ResMut<MlTrainState>) {
    for i in 0..6 {
        if !state.states[i].is_training { continue; }

        let mut lines: Vec<String> = Vec::new();
        let mut done = false;

        if let Some(ref mutex) = state.states[i].training_rx {
            if let Ok(rx) = mutex.try_lock() {
                loop {
                    match rx.try_recv() {
                        Ok(line) if line == "__DONE__" => { done = true; break; }
                        Ok(line)                       => lines.push(line),
                        Err(_)                         => break,
                    }
                }
            }
        }

        if !lines.is_empty() {
            state.states[i].status_text.push('\n');
            state.states[i].status_text.push_str(&lines.join("\n"));
            let count = state.states[i].status_text.lines().count();
            if count > 60 {
                let new_text = state.states[i].status_text
                    .lines()
                    .skip(count - 60)
                    .collect::<Vec<_>>()
                    .join("\n");
                state.states[i].status_text = new_text;
            }
        }

        if done {
            let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
            state.states[i].last_trained = Some(timestamp.clone());
            state.states[i].status_text
                .push_str(&format!("\n\nLast trained: {}", timestamp));
            state.states[i].is_training = false;
            state.states[i].training_rx = None;
        }
    }
}

/// Sync each MlModelInfoText entity with the corresponding model's status_text.
pub fn update_ml_model_info_text(
    state: Res<MlTrainState>,
    mut query: Query<(&MlModelInfoText, &mut Text)>,
) {
    if !state.is_changed() { return; }
    for (info, mut text) in query.iter_mut() {
        let model_text = &state.get(info.model).status_text;
        if text.0 != *model_text {
            text.0 = model_text.clone();
        }
    }
}

/// Hover effect for ML model buttons
pub fn update_ml_model_btn_hover(
    mut query: Query<(&Interaction, &mut BackgroundColor), (Changed<Interaction>, With<MlModelBtn>)>,
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

// ============================================================================
// Cross-pair systems
// ============================================================================

/// Handle click on a cross-pair button — CheckStatus if data exists, else auto-download.
pub fn handle_cross_pair_btn_click(
    symbol_map: Res<SymbolIdMap>,
    mut dashboard_state: ResMut<BotDashboardState>,
    request_sender: Res<DataRequestSender>,
    query: Query<(&Interaction, &CrossPairBtn), Changed<Interaction>>,
) {
    for (interaction, btn) in query.iter() {
        if *interaction != Interaction::Pressed { continue; }

        let symbol_id = match symbol_map.name_to_id.get(btn.symbol) {
            Some(&id) => id,
            None => {
                dashboard_state.cross_pair_status.insert(
                    btn.symbol.to_string(),
                    "Symbol ID not found. Wait for connection.".to_string(),
                );
                continue;
            }
        };

        dashboard_state.cross_pair_status.insert(
            btn.symbol.to_string(),
            "Checking...".to_string(),
        );

        let _ = request_sender.sender.try_send(DataRequest {
            symbol: btn.symbol.to_string(),
            symbol_id,
            kind: DataKind::M1Candles,
            action: DataAction::CheckStatus,
            force_rebuild: false,
        });
    }
}

/// Sync CrossPairStatusText entities from the cross_pair_status HashMap.
pub fn update_cross_pair_status_text(
    dashboard_state: Res<BotDashboardState>,
    mut query: Query<(&CrossPairStatusText, &mut Text)>,
) {
    if !dashboard_state.is_changed() { return; }
    for (status_text, mut text) in query.iter_mut() {
        let msg = dashboard_state.cross_pair_status
            .get(status_text.symbol)
            .map(|s| s.as_str())
            .unwrap_or("--");
        if text.0 != msg {
            text.0 = msg.to_string();
        }
    }
}

/// Hover effect for cross-pair buttons
pub fn update_cross_pair_btn_hover(
    mut query: Query<(&Interaction, &mut BackgroundColor), (Changed<Interaction>, With<CrossPairBtn>)>,
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

// ============================================================================
// Cross-pair Update systems (Update History)
// ============================================================================

/// Handle click on a cross-pair update button — send UpdateLatest for M1 candles.
pub fn handle_cross_pair_update_btn_click(
    symbol_map: Res<SymbolIdMap>,
    mut dashboard_state: ResMut<BotDashboardState>,
    request_sender: Res<DataRequestSender>,
    query: Query<(&Interaction, &CrossPairUpdateBtn), Changed<Interaction>>,
) {
    for (interaction, btn) in query.iter() {
        if *interaction != Interaction::Pressed { continue; }

        let symbol_id = match symbol_map.name_to_id.get(btn.symbol) {
            Some(&id) => id,
            None => {
                dashboard_state.cross_pair_update_status.insert(
                    btn.symbol.to_string(),
                    "Symbol ID not found. Wait for connection.".to_string(),
                );
                continue;
            }
        };

        dashboard_state.updating_cross_pairs.insert(btn.symbol.to_string());
        dashboard_state.cross_pair_update_status.insert(
            btn.symbol.to_string(),
            "Updating...".to_string(),
        );

        let _ = request_sender.sender.try_send(DataRequest {
            symbol: btn.symbol.to_string(),
            symbol_id,
            kind: DataKind::M1Candles,
            action: DataAction::UpdateLatest,
            force_rebuild: false,
        });
    }
}

/// Sync CrossPairUpdateStatusText entities from the cross_pair_update_status HashMap.
pub fn update_cross_pair_update_status_text(
    dashboard_state: Res<BotDashboardState>,
    mut query: Query<(&CrossPairUpdateStatusText, &mut Text)>,
) {
    if !dashboard_state.is_changed() { return; }
    for (status_text, mut text) in query.iter_mut() {
        let msg = dashboard_state.cross_pair_update_status
            .get(status_text.symbol)
            .map(|s| s.as_str())
            .unwrap_or("--");
        if text.0 != msg {
            text.0 = msg.to_string();
        }
    }
}

/// Hover effect for cross-pair update buttons
pub fn update_cross_pair_update_btn_hover(
    mut query: Query<(&Interaction, &mut BackgroundColor), (Changed<Interaction>, With<CrossPairUpdateBtn>)>,
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

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Read metrics JSON for a given ML model and format a human-readable summary.
fn read_ml_model_status(model: MlSubCardType) -> String {
    // Only Model 1 has metrics yet; others show a placeholder
    let (metrics_path, model_path, model_name) = match model {
        MlSubCardType::Model1 => (
            "ml/trained/model1_metrics.json",
            "ml/trained/model1_technical.json",
            "Model 1  Technical Indicators (XGBoost)",
        ),
        MlSubCardType::Model2 => return read_model2_status(),
        MlSubCardType::Model3 => return "Model 3 (Chart Patterns CNN) — not yet trained.".to_string(),
        MlSubCardType::Model4 => return "Model 4 (News & Calendar) — not yet trained.".to_string(),
        MlSubCardType::Model5 => return "Model 5 (Order Flow XGBoost) — not yet trained.".to_string(),
        MlSubCardType::Model6 => return "Model 6 (Ensemble XGBoost) — not yet trained.".to_string(),
    };

    let raw = match std::fs::read_to_string(metrics_path) {
        Ok(s)  => s,
        Err(e) => return format!("Could not read {}: {}", metrics_path, e),
    };
    let v: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v)  => v,
        Err(e) => return format!("Could not parse metrics JSON: {}", e),
    };

    let avg   = &v["avg_metrics"];
    let cfg   = &v["config"];
    let folds = v["fold_metrics"].as_array();

    let roc_auc   = avg["roc_auc"].as_f64().unwrap_or(0.0);
    let accuracy  = avg["accuracy"].as_f64().unwrap_or(0.0);
    let precision = avg["precision"].as_f64().unwrap_or(0.0);
    let log_loss  = avg["log_loss"].as_f64().unwrap_or(0.0);
    let n_feat    = v["n_features_used"].as_u64().unwrap_or(0);
    let target_p  = cfg["target_pips"].as_u64().unwrap_or(0);
    let stop_p    = cfg["stop_pips"].as_u64().unwrap_or(0);
    let horizon   = cfg["horizon_bars"].as_u64().unwrap_or(0);
    let n_folds   = folds.map(|f| f.len()).unwrap_or(0);

    let test_years: Vec<String> = folds
        .map(|f| f.iter()
            .filter_map(|m| m["test_year"].as_u64().map(|y| y.to_string()))
            .collect())
        .unwrap_or_default();

    let mut lines = Vec::new();
    lines.push(model_name.to_string());
    lines.push("--------------------------------------".to_string());
    lines.push(format!("Label: +{}p target / -{}p stop / {}m horizon", target_p, stop_p, horizon));
    lines.push(format!("Features used  : {}", n_feat));
    lines.push(format!("Walk-fwd folds : {} (test years: {})", n_folds, test_years.join(", ")));
    lines.push(String::new());
    lines.push("--- Avg walk-forward metrics ---".to_string());
    lines.push(format!("ROC-AUC  : {:.4}", roc_auc));
    lines.push(format!("Accuracy : {:.4}", accuracy));
    lines.push(format!("Precision: {:.4}  (at threshold 0.55)", precision));
    lines.push(format!("Log-loss : {:.4}", log_loss));

    if let Some(folds_arr) = folds {
        lines.push(String::new());
        lines.push("--- Per-fold ---".to_string());
        for fold in folds_arr {
            let year = fold["test_year"].as_u64().unwrap_or(0);
            let auc  = fold["roc_auc"].as_f64().unwrap_or(0.0);
            let prec = fold["precision"].as_f64().unwrap_or(0.0);
            let sigs = fold["signals"].as_u64().unwrap_or(0);
            lines.push(format!("  {}: AUC={:.3}  Prec={:.3}  Signals={}", year, auc, prec, sigs));
        }
    }

    if let Ok(meta) = std::fs::metadata(model_path) {
        if let Ok(modified) = meta.modified() {
            let datetime = chrono::DateTime::<chrono::Local>::from(modified);
            lines.push(String::new());
            lines.push(format!("Last trained: {}", datetime.format("%Y-%m-%d %H:%M")));
        }
    }

    lines.join("\n")
}

/// Parse model2_metrics.json and format a human-readable status string.
fn read_model2_status() -> String {
    let metrics_path = "ml/trained/model2_metrics.json";
    let model_path   = "ml/trained/model2_regime.pkl";

    let raw = match std::fs::read_to_string(metrics_path) {
        Ok(s)  => s,
        Err(_) => return "Model 2 (Regime HMM) — not yet trained.\nPress 'Update Training' to train.".to_string(),
    };
    let v: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v)  => v,
        Err(e) => return format!("Could not parse model2_metrics.json: {}", e),
    };

    let n_states  = v["n_states"].as_u64().unwrap_or(0);
    let n_feat    = v["n_features"].as_u64().unwrap_or(0);
    let n_bars    = v["train_bars"].as_u64().unwrap_or(0);
    let stride    = v["stride"].as_u64().unwrap_or(1);
    let ll        = v["log_likelihood"].as_f64().unwrap_or(0.0);
    let aic       = v["aic"].as_f64().unwrap_or(0.0);
    let bic       = v["bic"].as_f64().unwrap_or(0.0);
    let n_params  = v["n_params"].as_u64().unwrap_or(0);
    let n_restart = v["n_restarts"].as_u64().unwrap_or(0);

    let range_start = v["train_range"][0].as_str().unwrap_or("?");
    let range_end   = v["train_range"][1].as_str().unwrap_or("?");
    let start_short = &range_start[..10.min(range_start.len())];
    let end_short   = &range_end[..10.min(range_end.len())];

    let mut lines = Vec::new();
    lines.push("Model 2  Regime Detection (GaussianHMM)".to_string());
    lines.push("--------------------------------------".to_string());
    lines.push(format!("States: {}  |  Features: {}  |  Restarts: {}", n_states, n_feat, n_restart));
    lines.push(format!("Train obs : {} (every {}nd bar)", n_bars, stride));
    lines.push(format!("Train range: {} -> {}", start_short, end_short));
    lines.push(String::new());
    lines.push("--- Fit quality ---".to_string());
    lines.push(format!("Log-likelihood: {:.2}", ll));
    lines.push(format!("AIC    : {:.2}", aic));
    lines.push(format!("BIC    : {:.2}", bic));
    lines.push(format!("Params : {}", n_params));

    if let Some(stats) = v["state_stats"].as_array() {
        lines.push(String::new());
        lines.push("--- Regime states ---".to_string());
        for s in stats {
            let label    = s["label"].as_str().unwrap_or("?");
            let pct      = s["pct_bars"].as_f64().unwrap_or(0.0);
            let avg_dur  = s["avg_duration_bars"].as_f64().unwrap_or(0.0);
            let ret_pips = s["mean_return_pips"].as_f64().unwrap_or(0.0);
            let vol      = s["mean_vol_20"].as_f64().unwrap_or(0.0);
            lines.push(format!(
                "  {:<16}: {:5.1}%  dur={:.0}m  ret={:+.4}p  vol={:.6}",
                label, pct, avg_dur, ret_pips, vol
            ));
        }
    }

    if let Ok(meta) = std::fs::metadata(model_path) {
        if let Ok(modified) = meta.modified() {
            let datetime = chrono::DateTime::<chrono::Local>::from(modified);
            lines.push(String::new());
            lines.push(format!("Last trained: {}", datetime.format("%Y-%m-%d %H:%M")));
        }
    }

    lines.join("\n")
}

/// Return a feature-category breakdown for a given ML model.
fn read_ml_model_features(model: MlSubCardType) -> String {
    match model {
        MlSubCardType::Model1 => concat!(
            "~125 computed -> top 70 selected by XGBoost gain\n",
            "\n",
            "Trend (MA)        10\n",
            "  EMA 5/10/21/50/100/200, SMA 20\n",
            "  ema5/21, ema21/50, ema50/200 cross\n",
            "\n",
            "Momentum          14\n",
            "  RSI 14/5, MACD line/signal/hist\n",
            "  Stoch K/D, CCI, WilliamsR\n",
            "  ROC 10, Momentum, ADX / +DI / -DI\n",
            "\n",
            "Volatility         7\n",
            "  ATR 14 + ratio, BB width/pos\n",
            "  StdDev 20, KC width, Squeeze\n",
            "\n",
            "Price / Returns    9\n",
            "  return 1/5/15/30/60m\n",
            "  hl_range, body, upper/lower wick\n",
            "\n",
            "S/R Levels         7\n",
            "  SwingHigh/Low x3 periods, Pivot\n",
            "\n",
            "Ranges / Slopes    8\n",
            "  Range + Slope for 5/15/30/60m\n",
            "\n",
            "Candlesticks       8\n",
            "  Doji, Hammer, ShootingStar\n",
            "  Engulf x2, InsideBar, PinBar x2\n",
            "\n",
            "Time / Session     7\n",
            "  Hour sin/cos, DOW sin/cos\n",
            "  London, NewYork, Overlap\n",
            "\n",
            "Volume             4\n",
            "  VolRatio, OBV x2, MFI 14\n",
            "\n",
            "Consecutive        4\n",
            "  BullStreak, BearStreak\n",
            "  SinceSwingHigh, SinceSwingLow\n",
            "\n",
            "Tick Features      6\n",
            "  TickRatio, SpreadMean/Max/Std\n",
            "  WideRatio, SpreadCost%\n",
            "\n",
            "VWAP               3\n",
            "  dist_vwap, vwap_slope_5, above_vwap\n",
            "\n",
            "Ichimoku Cloud     5\n",
            "  dist_cloud_top, dist_cloud_bot\n",
            "  cloud_thickness, above_cloud\n",
            "  tenkan_vs_kijun\n",
            "\n",
            "Fibonacci          5\n",
            "  dist 23.6 / 38.2 / 50.0 / 61.8\n",
            "  fib_position (0=low, 1=high)\n",
            "\n",
            "Cross-Pair        28\n",
            "  GBPUSD/USDJPY/USDCHF/AUDUSD/EURJPY\n",
            "  return_1m/5m, RSI14, vs_EMA21, mom10\n",
            "  usd_strength_5m, risk_sentiment_5m\n",
            "  eur_divergence_5m\n",
            "\n",
            "Market Regime      2\n",
            "  regime_state (Model 2 HMM 0-3)\n",
            "  regime_prob_max (confidence)",
        ).to_string(),
        MlSubCardType::Model2 => concat!(
            "8 features  (unsupervised — no labels)\n",
            "\n",
            "log_return          bar log-return\n",
            "realized_vol_20     20-bar rolling std of log-returns\n",
            "realized_vol_5      5-bar rolling std (fast vol)\n",
            "atr_ratio           ATR(14) / close\n",
            "hl_range            (high - low) / close\n",
            "spread_mean_pips    mean bid-ask spread in bar\n",
            "return_abs_20       20-bar mean of |log_return|\n",
            "vol_ratio           realized_vol_5 / realized_vol_20\n",
            "\n",
            "Model: GaussianHMM  covariance=full\n",
            "States: 4  (Trending Up / Down / Ranging / Volatile)\n",
            "Data: all 24h bars from 2013  (no session filter)\n",
            "Stride: every 3rd bar -> ~1.6M observations\n",
            "Restarts: 5  (best log-likelihood selected)",
        ).to_string(),
        MlSubCardType::Model3 => "Model 3 (Chart Patterns CNN) - features not yet defined.".to_string(),
        MlSubCardType::Model4 => "Model 4 (News & Calendar) - features not yet defined.".to_string(),
        MlSubCardType::Model5 => "Model 5 (Order Flow) - features not yet defined.".to_string(),
        MlSubCardType::Model6 => "Model 6 (Ensemble) - features not yet defined.".to_string(),
    }
}
