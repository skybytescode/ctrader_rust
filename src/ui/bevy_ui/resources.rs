//! UI-specific resources for state management

use bevy::prelude::*;
use std::collections::HashSet;
use crate::ui::bevy_ui::components::{TopCardType, DbSubCardType};

/// State for the tooltip popup
#[derive(Resource, Default)]
pub struct TooltipState {
    /// Whether the tooltip is currently visible
    pub visible: bool,
    /// Screen position for the tooltip
    pub position: Vec2,
    /// Candle data to display
    pub candle_data: Option<TooltipData>,
}

/// Data for tooltip display
#[derive(Clone)]
pub struct TooltipData {
    pub timestamp: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: i64,
    pub decimal_places: u8,
    pub is_bullish: bool,
}

/// State for virtualized scrolling in instrument list
#[derive(Resource)]
pub struct VirtualizedScrollState {
    /// Current scroll offset in pixels
    pub scroll_offset: f32,
    /// Height of each row (fixed for virtualization)
    pub item_height: f32,
    /// Number of items visible in viewport
    pub visible_count: usize,
    /// Total number of items in the list
    pub total_items: usize,
    /// Index of first visible item
    pub first_visible_index: usize,
    /// Set of currently spawned row indices
    pub spawned_indices: HashSet<usize>,
}

impl Default for VirtualizedScrollState {
    fn default() -> Self {
        Self {
            scroll_offset: 0.0,
            item_height: 32.0,  // Fixed row height for performance
            visible_count: 0,
            total_items: 0,
            first_visible_index: 0,
            spawned_indices: HashSet::new(),
        }
    }
}

impl VirtualizedScrollState {
    /// Calculate which items should be visible
    pub fn calculate_visible_range(&self) -> std::ops::Range<usize> {
        let first = self.first_visible_index;
        let last = (first + self.visible_count + 1).min(self.total_items);
        first..last
    }

    /// Update first visible index based on scroll offset
    pub fn update_from_scroll(&mut self) {
        self.first_visible_index = (self.scroll_offset / self.item_height).floor() as usize;
    }

    /// Get the maximum scroll offset
    pub fn max_scroll(&self) -> f32 {
        let total_height = self.total_items as f32 * self.item_height;
        let viewport_height = self.visible_count as f32 * self.item_height;
        (total_height - viewport_height).max(0.0)
    }

    /// Clamp scroll offset to valid range
    pub fn clamp_scroll(&mut self) {
        self.scroll_offset = self.scroll_offset.clamp(0.0, self.max_scroll());
    }
}

/// State for UI interactions (dragging, hovering)
#[derive(Resource, Default)]
pub struct UiInteractionState {
    /// Currently hovered instrument symbol
    pub hovered_instrument: Option<String>,
    /// Whether price axis is being dragged
    pub dragging_price_axis: bool,
    /// Whether time axis is being dragged
    pub dragging_time_axis: bool,
    /// Whether scrollbar thumb is being dragged
    pub dragging_scrollbar: bool,
    /// Whether instrument list is being scrolled
    pub scrolling_list: bool,
    /// Drag start position for calculations
    pub drag_start: Option<Vec2>,
}

/// Cached formatted price strings to avoid repeated formatting
#[derive(Resource, Default)]
pub struct PriceFormatCache {
    /// Map of (price_micros, decimal_places) -> formatted string
    pub cache: std::collections::HashMap<(i64, u8), String>,
}

impl PriceFormatCache {
    /// Get or create formatted price string
    pub fn format(&mut self, price: f64, decimal_places: u8) -> String {
        // Convert to micros for stable hash key
        let price_micros = (price * 1_000_000.0) as i64;
        let key = (price_micros, decimal_places);

        self.cache.entry(key)
            .or_insert_with(|| format!("{:.width$}", price, width = decimal_places as usize))
            .clone()
    }

    /// Clear cache (call periodically to prevent unbounded growth)
    pub fn clear(&mut self) {
        self.cache.clear();
    }
}

/// Tracks the multi-step tick download/update workflow (bid → ask → merge)
///
/// History BoT flow:      CheckingBid → (DownloadingBid) → CheckingAsk → (DownloadingAsk) → Merging
/// Update History flow:   CheckingBidForUpdate → CheckingAskForUpdate → UpdatingBid → UpdatingAsk → Merging
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickWorkflowStep {
    #[default]
    Idle,
    /// History BoT: checking if bid ticks exist
    CheckingBid,
    /// History BoT: downloading full bid tick history
    DownloadingBid,
    /// History BoT: checking if ask ticks exist
    CheckingAsk,
    /// History BoT: downloading full ask tick history
    DownloadingAsk,
    /// Update History: checking last bid tick timestamp before updating
    CheckingBidForUpdate,
    /// Update History: checking last ask tick timestamp before updating
    CheckingAskForUpdate,
    /// Update History: downloading new bid ticks (UpdateLatest)
    UpdatingBid,
    /// Update History: downloading new ask ticks (UpdateLatest)
    UpdatingAsk,
    /// Both flows: running the merge
    Merging,
}

/// Tracks which bot dashboard card is currently expanded (accordion state)
#[derive(Resource, Default)]
pub struct BotDashboardState {
    /// Which top-level card is expanded (Database, TrainModel, etc.)
    pub expanded_top: Option<TopCardType>,
    /// Which Database sub-card is expanded (HistoryBot, UpdateHistory, etc.)
    pub expanded_db_sub: Option<DbSubCardType>,
    /// Download progress message shown inside the History BoT sub-card
    pub download_message: Option<String>,
    /// First/last record info shown in the bottom status area
    pub data_status_message: Option<String>,
    /// M1 candle info (first/last) — persisted across actions
    pub m1_info: Option<String>,
    /// Tick data info (first/last) — persisted across actions
    pub tick_info: Option<String>,
    /// Whether a download is currently in progress
    pub is_downloading: bool,
    /// Number of rows downloaded so far
    pub download_progress: u64,
    /// Current step in the tick bid+ask+merge workflow
    pub tick_workflow: TickWorkflowStep,
    /// Status message shown inside the Update History sub-card
    pub update_history_message: Option<String>,
    /// True when the current action was triggered from Update History (not History BoT)
    pub is_update_mode: bool,
    /// Rows added during an Update History M1 action (held until CheckStatus completes)
    pub pending_update_rows: u64,
    /// ML features table info (first/last row) — shown in green in the status area
    pub ml_features_info: Option<String>,
}

/// Tracks which UI elements need rebuilding
#[derive(Resource, Default)]
pub struct UiRebuildFlags {
    /// Sidebar visibility changed
    pub sidebar_visibility: bool,
    /// Tab selection changed
    pub tab_changed: bool,
    /// Category expand/collapse changed
    pub category_changed: bool,
    /// Instrument selection changed
    pub instrument_selection: bool,
    /// Timeframe changed
    pub timeframe_changed: bool,
}

impl UiRebuildFlags {
    /// Check if any flag is set
    pub fn any(&self) -> bool {
        self.sidebar_visibility
            || self.tab_changed
            || self.category_changed
            || self.instrument_selection
            || self.timeframe_changed
    }

    /// Reset all flags
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}
