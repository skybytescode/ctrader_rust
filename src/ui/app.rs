use bevy::prelude::*;
use std::collections::{HashMap, HashSet};

use super::chart::InstrumentData;

#[derive(Resource)]
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

#[derive(Resource)]
pub struct UiState {
    pub active_view: ActiveView,
    pub sidebar_expanded: bool,
    pub expanded_categories: HashSet<SymbolCategory>,
}

impl Default for UiState {
    fn default() -> Self {
        let mut expanded_categories = HashSet::new();
        expanded_categories.insert(SymbolCategory::TradingBots);

        Self {
            active_view: ActiveView::Bots,
            sidebar_expanded: true,
            expanded_categories,
        }
    }
}

/// Placeholder for the old ui_system - kept for backward compatibility
#[allow(dead_code)]
pub fn ui_system() {
    // No-op - using BevyUiPlugin instead
}
