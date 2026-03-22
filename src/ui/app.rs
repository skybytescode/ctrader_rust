use std::collections::{HashMap, HashSet};

// ============================================================================
// Instrument Data
// ============================================================================

/// Tick direction for coloring the live price
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum TickDirection {
    #[default]
    Up,
    Down,
}

/// Data for a single instrument (live prices + metadata)
#[derive(Debug, Clone)]
pub struct InstrumentData {
    pub symbol: String,
    pub bid: f64,
    pub ask: f64,
    pub prev_bid: f64,
    pub prev_ask: f64,
    pub open: f64,
    pub decimal_places: u8,
    pub tick_direction: TickDirection,
    pub trades_weekends: bool,
}

impl InstrumentData {
    pub fn new(symbol: &str, decimal_places: u8, trades_weekends: bool) -> Self {
        Self {
            symbol: symbol.to_string(),
            bid: 0.0,
            ask: 0.0,
            prev_bid: 0.0,
            prev_ask: 0.0,
            open: 0.0,
            decimal_places,
            tick_direction: TickDirection::Up,
            trades_weekends,
        }
    }

    pub fn mid_price(&self) -> f64 {
        (self.bid + self.ask) / 2.0
    }

    pub fn prev_mid_price(&self) -> f64 {
        (self.prev_bid + self.prev_ask) / 2.0
    }

    pub fn update_price(&mut self, bid: f64, ask: f64) {
        self.prev_bid = self.bid;
        self.prev_ask = self.ask;
        self.bid = bid;
        self.ask = ask;

        let current_mid = self.mid_price();
        let prev_mid = self.prev_mid_price();
        if current_mid > prev_mid {
            self.tick_direction = TickDirection::Up;
        } else if current_mid < prev_mid {
            self.tick_direction = TickDirection::Down;
        }

        if self.open == 0.0 {
            self.open = bid;
        }
    }
}

// ============================================================================
// App State
// ============================================================================

pub struct AppState {
    pub instruments: HashMap<String, InstrumentData>,
    pub connection_status: String,
}

impl Default for AppState {
    fn default() -> Self {
        let mut instruments = HashMap::new();
        instruments.insert("EURUSD".to_string(), InstrumentData::new("EURUSD", 5, false));
        Self {
            instruments,
            connection_status: "Init".to_string(),
        }
    }
}

impl AppState {
    pub fn eurusd(&self) -> Option<&InstrumentData> {
        self.instruments.get("EURUSD")
    }

    pub fn eurusd_mut(&mut self) -> Option<&mut InstrumentData> {
        self.instruments.get_mut("EURUSD")
    }
}

// ============================================================================
// View / Category Enums
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum ActiveView {
    #[default]
    Bots,
}

/// Categories for the sidebar instrument list
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolCategory {
    TradingBots,
}

impl SymbolCategory {
    pub fn all() -> &'static [SymbolCategory] {
        &[SymbolCategory::TradingBots]
    }

    pub fn label(&self) -> &'static str {
        match self {
            SymbolCategory::TradingBots => "Trading Bots",
        }
    }

    pub fn instruments(&self) -> &'static [&'static str] {
        match self {
            SymbolCategory::TradingBots => &["EURUSD"],
        }
    }
}

// ============================================================================
// UI State
// ============================================================================

pub struct UiState {
    pub active_view: ActiveView,
    pub sidebar_expanded: bool,
    pub expanded_categories: HashSet<SymbolCategory>,
    /// Currently selected instrument (drives bot dashboard visibility)
    pub selected_instrument: Option<String>,
}

impl Default for UiState {
    fn default() -> Self {
        let mut expanded_categories = HashSet::new();
        expanded_categories.insert(SymbolCategory::TradingBots);

        Self {
            active_view: ActiveView::Bots,
            sidebar_expanded: true,
            expanded_categories,
            selected_instrument: None,
        }
    }
}

#[allow(dead_code)]
pub fn ui_system() {
    // No-op - using BevyUiPlugin instead
}
