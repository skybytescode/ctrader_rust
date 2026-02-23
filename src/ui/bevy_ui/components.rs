//! UI marker components for Bevy UI elements
//!
//! These components mark entities for querying and interaction handling

use bevy::prelude::*;
use crate::ui::{ActiveView, SymbolCategory, Timeframe};

// ============================================================================
// Root and Camera
// ============================================================================

/// Marker for the root UI node
#[derive(Component)]
pub struct UiRoot;

/// Marker for the UI camera
#[derive(Component)]
pub struct UiCamera;

// ============================================================================
// Panel Markers
// ============================================================================

/// Marker for the top panel (title bar)
#[derive(Component)]
pub struct TopPanel;

/// Marker for the narrow icon bar (leftmost)
#[derive(Component)]
pub struct IconBar;

/// Marker for the expandable sidebar
#[derive(Component)]
pub struct Sidebar;

/// Marker for the main chart panel area
#[derive(Component)]
pub struct ChartPanel;

/// Marker for the chart toolbar
#[derive(Component)]
pub struct ChartToolbar;

/// Marker for the price axis panel
#[derive(Component)]
pub struct PriceAxisPanel;

/// Marker for the time axis panel
#[derive(Component)]
pub struct TimeAxisPanel;

// ============================================================================
// Icon Bar Components
// ============================================================================

/// Icon bar button that switches between views
#[derive(Component)]
pub struct IconButton {
    pub view: ActiveView,
}

/// Text label below an icon button
#[derive(Component)]
pub struct IconLabel {
    pub view: ActiveView,
}

// ============================================================================
// Sidebar Components
// ============================================================================

/// Container for the instrument list (scrollable viewport)
#[derive(Component)]
pub struct InstrumentListContainer;

/// The scrollable viewport for instruments
#[derive(Component)]
pub struct InstrumentListViewport;

/// Category header row
#[derive(Component)]
pub struct CategoryHeader {
    pub category: SymbolCategory,
}

/// Single instrument row in the watchlist
#[derive(Component)]
pub struct InstrumentRow {
    pub symbol: String,
    pub list_index: usize,
}

/// Symbol name text in an instrument row
#[derive(Component)]
pub struct SymbolName {
    pub symbol: String,
}

/// Bid price text in an instrument row
#[derive(Component)]
pub struct BidPrice {
    pub symbol: String,
}

/// Ask price text in an instrument row
#[derive(Component)]
pub struct AskPrice {
    pub symbol: String,
}

/// Spread display in an instrument row
#[derive(Component)]
pub struct SpreadPrice {
    pub symbol: String,
}

// ============================================================================
// Chart Toolbar Components
// ============================================================================

/// Displays the currently selected instrument name
#[derive(Component)]
pub struct SelectedInstrumentLabel;

/// Timeframe selector button
#[derive(Component)]
pub struct TimeframeButton {
    pub timeframe: Timeframe,
}

/// Zoom control button
#[derive(Component)]
pub struct ZoomButton {
    pub is_horizontal: bool,
    pub is_increase: bool,
}

/// Zoom level display text
#[derive(Component)]
pub struct ZoomDisplay {
    pub is_horizontal: bool,
}

/// Reset button to restore default zoom/pan
#[derive(Component)]
pub struct ResetButton;

/// Toggle button for chart features (Volume, Grid, Crosshair)
#[derive(Component)]
pub struct ToggleButton {
    pub toggle_type: ToggleType,
}

/// Types of toggle buttons
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToggleType {
    Volume,
    Grid,
    Crosshair,
}

// ============================================================================
// Price Axis Components
// ============================================================================

/// Individual price label on the price axis
#[derive(Component)]
pub struct PriceLabel {
    pub price: f64,
}

/// Live price marker showing current mid price
#[derive(Component)]
pub struct LivePriceMarker;

/// Live price text inside the marker
#[derive(Component)]
pub struct LivePriceText;

// ============================================================================
// Time Axis Components
// ============================================================================

/// Individual time label on the time axis
#[derive(Component)]
pub struct TimeLabel {
    pub timestamp: i64,
}

// ============================================================================
// Scrollbar Components
// ============================================================================

/// The scrollbar track (background)
#[derive(Component)]
pub struct ScrollbarTrack;

/// The scrollbar thumb (draggable)
#[derive(Component)]
pub struct ScrollbarThumb;

/// Button to load more historical data
#[derive(Component)]
pub struct LoadMoreButton;

/// Display showing loaded/total candles
#[derive(Component)]
pub struct LoadProgressDisplay;

// ============================================================================
// Tooltip Components
// ============================================================================

/// Container for the tooltip popup
#[derive(Component)]
pub struct TooltipContainer;

/// Text content inside the tooltip
#[derive(Component)]
pub struct TooltipContent;

// ============================================================================
// Connection Status
// ============================================================================

/// Displays connection status in top panel
#[derive(Component)]
pub struct ConnectionStatusLabel;

/// Displays selected instrument in top panel
#[derive(Component)]
pub struct HeaderInstrumentLabel;

// ============================================================================
// Bot Dashboard Components
// ============================================================================

/// Marker for the bot dashboard root panel
#[derive(Component)]
pub struct BotDashboard;

/// Title text in the dashboard header (shows instrument name)
#[derive(Component)]
pub struct BotDashboardTitle;

// ============================================================================
// Bot Dashboard — Top-Level Card Components
// ============================================================================

/// Identifies one of the 4 top-level dashboard cards
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopCardType {
    Database,
    TrainModel,
    StartPause,
    CurrentStatus,
}

/// Container holding all 4 main cards in a Column+Wrap layout
#[derive(Component)]
pub struct CardsContainer;

/// Marker for a flex row containing 2 main cards (row_index: 0 or 1)
#[derive(Component)]
pub struct MainCardRow {
    pub row_index: u8,
}

/// Marker for a top-level main card container
#[derive(Component)]
pub struct MainCard {
    pub card_type: TopCardType,
}

/// Expandable content area inside a main card (hidden when collapsed)
#[derive(Component)]
pub struct MainCardContent {
    pub card_type: TopCardType,
}

/// The [+]/[-] toggle button on a main card
#[derive(Component)]
pub struct MainCardMaxBtn {
    pub card_type: TopCardType,
}

/// The text inside the [+]/[-] button (so it can be updated)
#[derive(Component)]
pub struct MainCardMaxBtnIcon {
    pub card_type: TopCardType,
}

// ============================================================================
// Database Sub-Panel Components
// ============================================================================

/// Sub-card types inside the Database panel
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbSubCardType {
    HistoryBot,
    UpdateHistory,
    Status,
    Dom,
}

/// Marker for a sub-card inside the Database panel
#[derive(Component)]
pub struct DbSubCard {
    pub sub_type: DbSubCardType,
}

/// Expandable content area inside a DB sub-card (hidden when collapsed)
#[derive(Component)]
pub struct DbSubCardContent {
    pub sub_type: DbSubCardType,
}

/// The [+]/[-] toggle button on a DB sub-card
#[derive(Component)]
pub struct DbSubCardMaxBtn {
    pub sub_type: DbSubCardType,
}

/// The text inside the DB sub-card [+]/[-] button
#[derive(Component)]
pub struct DbSubCardMaxBtnIcon {
    pub sub_type: DbSubCardType,
}

/// Timeframe options for Database buttons
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BotTimeframe {
    M1,
    M5,
}

/// M1 / M5 button inside a Database sub-card
#[derive(Component)]
pub struct DbTimeframeBtn {
    pub parent_card: DbSubCardType,
    pub timeframe: BotTimeframe,
}
