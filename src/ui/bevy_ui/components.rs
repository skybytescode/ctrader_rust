//! UI marker components for Bevy UI elements

use bevy::prelude::*;
use crate::ui::{ActiveView, SymbolCategory};

// ============================================================================
// Root and Camera
// ============================================================================

#[derive(Component)]
pub struct UiRoot;

#[derive(Component)]
pub struct UiCamera;

// ============================================================================
// Panel Markers
// ============================================================================

#[derive(Component)]
pub struct TopPanel;

#[derive(Component)]
pub struct IconBar;

#[derive(Component)]
pub struct Sidebar;

// ============================================================================
// Icon Bar Components
// ============================================================================

#[derive(Component)]
pub struct IconButton {
    pub view: ActiveView,
}

#[derive(Component)]
pub struct IconLabel {
    pub view: ActiveView,
}

// ============================================================================
// Sidebar Components
// ============================================================================

#[derive(Component)]
pub struct InstrumentListContainer;

#[derive(Component)]
pub struct InstrumentListViewport;

#[derive(Component)]
pub struct CategoryHeader {
    pub category: SymbolCategory,
}

#[derive(Component)]
pub struct InstrumentRow {
    pub symbol: String,
    pub list_index: usize,
}

#[derive(Component)]
pub struct SymbolName {
    pub symbol: String,
}

#[derive(Component)]
pub struct BidPrice {
    pub symbol: String,
}

#[derive(Component)]
pub struct AskPrice {
    pub symbol: String,
}

#[derive(Component)]
pub struct SpreadPrice {
    pub symbol: String,
}

// ============================================================================
// Connection Status
// ============================================================================

#[derive(Component)]
pub struct ConnectionStatusLabel;

#[derive(Component)]
pub struct HeaderInstrumentLabel;

// ============================================================================
// Bot Dashboard Components
// ============================================================================

#[derive(Component)]
pub struct BotDashboard;

#[derive(Component)]
pub struct BotDashboardTitle;

// ============================================================================
// Bot Dashboard — Top-Level Card Components
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopCardType {
    Database,
    TrainModel,
    StartPause,
    CurrentStatus,
}

#[derive(Component)]
pub struct CardsContainer;

#[derive(Component)]
pub struct MainCardRow {
    pub row_index: u8,
}

#[derive(Component)]
pub struct MainCard {
    pub card_type: TopCardType,
}

#[derive(Component)]
pub struct MainCardContent {
    pub card_type: TopCardType,
}

#[derive(Component)]
pub struct MainCardMaxBtn {
    pub card_type: TopCardType,
}

#[derive(Component)]
pub struct MainCardMaxBtnIcon {
    pub card_type: TopCardType,
}

// ============================================================================
// Database Sub-Panel Components
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbSubCardType {
    HistoryBot,
    UpdateHistory,
    Status,
    Dom,
}

#[derive(Component)]
pub struct DbSubCard {
    pub sub_type: DbSubCardType,
}

#[derive(Component)]
pub struct DbSubCardContent {
    pub sub_type: DbSubCardType,
}

#[derive(Component)]
pub struct DbSubCardMaxBtn {
    pub sub_type: DbSubCardType,
}

#[derive(Component)]
pub struct DbSubCardMaxBtnIcon {
    pub sub_type: DbSubCardType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BotTimeframe {
    M1Candles,
    TickData,
    MLFeatures,
}

#[derive(Component)]
pub struct DbTimeframeBtn {
    pub parent_card: DbSubCardType,
    pub timeframe: BotTimeframe,
}

#[derive(Component)]
pub struct DbStatusText;

#[derive(Component)]
pub struct M1InfoText;

#[derive(Component)]
pub struct TickInfoText;

#[derive(Component)]
pub struct MlInfoText;

#[derive(Component)]
pub struct HistoryBotStatusText;

#[derive(Component)]
pub struct UpdateHistoryStatusText;
