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

// ============================================================================
// Train Model Sub-Panel Components
// ============================================================================

/// Which model the sub-card represents (0-indexed repr for array indexing)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum MlSubCardType {
    Model1 = 0,
    Model2 = 1,
    Model3 = 2,
    Model4 = 3,
    Model5 = 4,
    Model6 = 5,
}

impl MlSubCardType {
    pub fn all() -> [MlSubCardType; 6] {
        [
            MlSubCardType::Model1,
            MlSubCardType::Model2,
            MlSubCardType::Model3,
            MlSubCardType::Model4,
            MlSubCardType::Model5,
            MlSubCardType::Model6,
        ]
    }
}

#[derive(Component)]
pub struct MlSubCard {
    pub sub_type: MlSubCardType,
}

/// Which action button inside an ML sub-card was clicked
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MlBtnType {
    Status,
    Train,
    FeatureCount,
}

/// Button inside an ML model sub-card (Status or Update Training)
#[derive(Component)]
pub struct MlModelBtn {
    pub model: MlSubCardType,
    pub btn_type: MlBtnType,
}

/// Status / training-output text area inside an ML model sub-card
#[derive(Component)]
pub struct MlModelInfoText {
    pub model: MlSubCardType,
}

/// Scroll container wrapping the MlModelInfoText — used for mouse-wheel scrolling
#[derive(Component)]
pub struct MlInfoScrollArea {
    pub model: MlSubCardType,
}

/// Draggable thumb inside the scrollbar track for an ML model info area
#[derive(Component)]
pub struct MlScrollbarThumb {
    pub model: MlSubCardType,
}

// ============================================================================
// DoM Capture (Model 6 — Realtime Algorithms)
// ============================================================================

/// Button to start/pause DoM depth capture
#[derive(Component)]
pub struct DomCaptureBtn;

/// Status text for DoM capture
#[derive(Component)]
pub struct DomCaptureStatusText;

// ============================================================================
// Cross-Pair Data Buttons (History BoT)
// ============================================================================

/// The 6 cross-pairs downloaded as M1 correlation features for Model 1.
pub const CROSS_PAIRS: [&str; 6] = ["GBPUSD", "USDJPY", "USDCHF", "AUDUSD", "EURJPY", "XAUUSD"];

/// Button for one cross-pair M1 download inside the History Bot sub-card.
#[derive(Component)]
pub struct CrossPairBtn {
    pub symbol: &'static str,
}

/// Status text next to a cross-pair button (shows candle count / date range).
#[derive(Component)]
pub struct CrossPairStatusText {
    pub symbol: &'static str,
}

// ============================================================================
// Cross-Pair Update Buttons (Update History)
// ============================================================================

/// Button for one cross-pair M1 update inside the Update History sub-card.
#[derive(Component)]
pub struct CrossPairUpdateBtn {
    pub symbol: &'static str,
}

/// Status text next to a cross-pair update button (shows rows added / last date).
#[derive(Component)]
pub struct CrossPairUpdateStatusText {
    pub symbol: &'static str,
}

// ============================================================================
// Economic Calendar Button (History BoT)
// ============================================================================

/// Button that triggers economic calendar scrape or shows last record.
#[derive(Component)]
pub struct EconCalBtn;

/// Status text shown next to the Economic Calendar button.
#[derive(Component)]
pub struct EconCalStatusText;

// ============================================================================
// Economic Calendar Update Button (Update History)
// ============================================================================

/// Button that appends new EC events since the last stored record.
#[derive(Component)]
pub struct EconCalUpdateBtn;

/// Status text shown next to the EC update button.
#[derive(Component)]
pub struct EconCalUpdateStatusText;
