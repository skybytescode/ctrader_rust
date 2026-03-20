//! Bevy UI Plugin - combines all UI systems and widgets
//! Focused on watchlist/sidebar functionality only (no chart)

use bevy::prelude::*;

use super::resources::*;
use super::systems::*;
use super::widgets::*;

/// System sets for ordering UI updates
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum UiSystemSet {
    /// Calculate layout sizes, visibility
    Layout,
    /// Update text, colors (with change detection)
    Update,
    /// Handle mouse/keyboard interactions
    Interaction,
    /// Spawn/despawn virtualized rows
    Render,
}

/// Pure Bevy UI Plugin - replaces egui for better performance
/// Currently handles: top panel, icon bar, sidebar with instrument list
pub struct BevyUiPlugin;

impl Plugin for BevyUiPlugin {
    fn build(&self, app: &mut App) {
        app
            // Resources
            .init_resource::<VirtualizedScrollState>()
            .init_resource::<UiInteractionState>()
            .init_resource::<PriceFormatCache>()
            .init_resource::<UiRebuildFlags>()
            .init_resource::<BotDashboardState>()
            .init_resource::<MlTrainState>()
            .init_resource::<MlScrollbarDragState>()

            // System set ordering
            .configure_sets(Update, (
                UiSystemSet::Layout,
                UiSystemSet::Update,
                UiSystemSet::Interaction,
                UiSystemSet::Render,
            ).chain())

            // Startup systems
            .add_systems(Startup, setup_ui)

            // Layout systems
            .add_systems(Update, (
                layout::update_sidebar_visibility,
                layout::update_virtualized_scroll,
            ).in_set(UiSystemSet::Layout))

            // Update systems (with change detection)
            .add_systems(Update, (
                price_updates::update_bid_prices,
                price_updates::update_ask_prices,
                price_updates::update_bid_colors,
                price_updates::update_ask_colors,
                price_updates::update_spread_prices,
                price_updates::update_spread_colors,
                price_updates::update_connection_status,
                price_updates::clear_price_cache,
                icon_bar::update_icon_button_styles,
                bot_dashboard::update_dashboard_visibility,
                bot_dashboard::process_data_responses,
                // Must run AFTER process_data_responses so they see the updated state
                bot_dashboard::update_history_bot_status
                    .after(bot_dashboard::process_data_responses),
                bot_dashboard::update_update_history_status
                    .after(bot_dashboard::process_data_responses),
                bot_dashboard::update_m1_info_text
                    .after(bot_dashboard::process_data_responses),
                bot_dashboard::update_tick_info_text
                    .after(bot_dashboard::process_data_responses),
                bot_dashboard::update_ml_info_text
                    .after(bot_dashboard::process_data_responses),
                bot_dashboard::poll_ml_training,
                bot_dashboard::update_ml_model_info_text
                    .after(bot_dashboard::poll_ml_training),
                bot_dashboard::update_cross_pair_status_text
                    .after(bot_dashboard::process_data_responses),
                bot_dashboard::update_cross_pair_update_status_text
                    .after(bot_dashboard::process_data_responses),
            ).in_set(UiSystemSet::Update))

            // Econ-cal systems split out to stay within Bevy's tuple limit
            .add_systems(Update, (
                bot_dashboard::poll_econ_cal,
                bot_dashboard::update_econ_cal_status_text
                    .after(bot_dashboard::poll_econ_cal),
                bot_dashboard::poll_econ_cal_update,
                bot_dashboard::update_econ_cal_update_status_text
                    .after(bot_dashboard::poll_econ_cal_update),
            ).in_set(UiSystemSet::Update))
            .add_systems(Update, (
                bot_dashboard::handle_econ_cal_update_btn_click,
                bot_dashboard::update_econ_cal_update_btn_hover,
            ).in_set(UiSystemSet::Interaction))

            // Interaction systems (split to stay within Bevy tuple limit)
            .add_systems(Update, (
                interactions::handle_icon_button_click,
                interactions::handle_instrument_click,
                interactions::handle_list_scroll,
                interactions::handle_ml_info_scroll,
                interactions::update_ml_scrollbar,
                interactions::handle_ml_scrollbar_drag,
                interactions::update_instrument_hover,
                bot_dashboard::handle_main_card_max_btn,
                bot_dashboard::update_main_card_expand,
                bot_dashboard::update_main_card_hover,
                bot_dashboard::handle_db_timeframe_btn_click,
                bot_dashboard::update_db_timeframe_btn_hover,
            ).in_set(UiSystemSet::Interaction))
            .add_systems(Update, (
                bot_dashboard::handle_ml_model_btn_click,
                bot_dashboard::update_ml_model_btn_hover,
                bot_dashboard::handle_cross_pair_btn_click,
                bot_dashboard::update_cross_pair_btn_hover,
                bot_dashboard::handle_cross_pair_update_btn_click,
                bot_dashboard::update_cross_pair_update_btn_hover,
                bot_dashboard::handle_econ_cal_btn_click,
                bot_dashboard::update_econ_cal_btn_hover,
                bot_dashboard::handle_dom_capture_btn_click,
                bot_dashboard::update_dom_capture_btn_hover,
            ).in_set(UiSystemSet::Interaction))
            .add_systems(Update, (
                bot_dashboard::update_dom_capture_btn_text,
                bot_dashboard::update_dom_capture_status_text,
            ).in_set(UiSystemSet::Update))

            // Render systems (virtualization)
            .add_systems(Update, (
                virtualization::spawn_visible_rows,
                virtualization::despawn_invisible_rows,
                virtualization::update_row_positions,
                virtualization::update_row_selection,
            ).in_set(UiSystemSet::Render));
    }
}

/// Setup the UI root and spawn initial widgets
fn setup_ui(mut commands: Commands) {
    // Spawn UI camera
    commands.spawn((
        Camera2d,
        Camera {
            order: 1,  // Render above other cameras
            ..default()
        },
        super::UiCamera,
    ));

    // Root UI node (full screen)
    let root = commands.spawn((
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            ..default()
        },
        super::UiRoot,
    )).id();

    // Top panel
    top_panel::spawn_top_panel(&mut commands, root);

    // Main content area (horizontal: icon bar + sidebar)
    let main_content = commands.spawn((
        Node {
            width: Val::Percent(100.0),
            flex_grow: 1.0,
            flex_direction: FlexDirection::Row,
            ..default()
        },
    )).id();

    // Icon bar
    icon_bar::spawn_icon_bar(&mut commands, main_content);

    // Sidebar
    sidebar::spawn_sidebar(&mut commands, main_content);

    // Bot dashboard (main content area, right of sidebar)
    bot_dashboard::spawn_bot_dashboard(&mut commands, main_content);

    commands.entity(root).add_child(main_content);

    println!("Bevy UI initialized (watchlist + bot dashboard)");
}
