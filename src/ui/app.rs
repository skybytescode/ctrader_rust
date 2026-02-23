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

        // Forex pairs - trades_weekends=false (skip Sat/Sun)
        instruments.insert("EURUSD".to_string(), InstrumentData::new("EURUSD", 5, false));
        instruments.insert("AUDUSD".to_string(), InstrumentData::new("AUDUSD", 5, false));
        instruments.insert("GBPUSD".to_string(), InstrumentData::new("GBPUSD", 5, false));
        instruments.insert("USDCHF".to_string(), InstrumentData::new("USDCHF", 5, false));
        instruments.insert("EURGBP".to_string(), InstrumentData::new("EURGBP", 5, false));
        instruments.insert("EURAUD".to_string(), InstrumentData::new("EURAUD", 5, false));

        // Metals - trades_weekends=false
        instruments.insert("XAUUSD".to_string(), InstrumentData::new("XAUUSD", 2, false));
        instruments.insert("XPDUSD".to_string(), InstrumentData::new("XPDUSD", 2, false));
        instruments.insert("XPTUSD".to_string(), InstrumentData::new("XPTUSD", 2, false));
        instruments.insert("XAUAUD".to_string(), InstrumentData::new("XAUAUD", 2, false));

        // Oil & Energy - trades_weekends=false
        instruments.insert("XTIUSD".to_string(), InstrumentData::new("XTIUSD", 3, false));
        instruments.insert("XNGUSD".to_string(), InstrumentData::new("XNGUSD", 4, false));

        // Indices - trades_weekends=false
        instruments.insert("CHINA50".to_string(), InstrumentData::new("CHINA50", 2, false));
        instruments.insert("SPXUSD".to_string(), InstrumentData::new("SPXUSD", 2, false));

        // Cryptocurrencies - trades_weekends=true (24/7)
        instruments.insert("BTCUSD".to_string(), InstrumentData::new("BTCUSD", 2, true));
        instruments.insert("BCHUSD".to_string(), InstrumentData::new("BCHUSD", 2, true));
        instruments.insert("ETHUSD".to_string(), InstrumentData::new("ETHUSD", 2, true));
        instruments.insert("LTCUSD".to_string(), InstrumentData::new("LTCUSD", 2, true));
        instruments.insert("AAVEUSD".to_string(), InstrumentData::new("AAVEUSD", 2, true));
        instruments.insert("AEROUSD".to_string(), InstrumentData::new("AEROUSD", 4, true));
        instruments.insert("ALGOUSD".to_string(), InstrumentData::new("ALGOUSD", 4, true));
        instruments.insert("APTUSD".to_string(), InstrumentData::new("APTUSD", 4, true));
        instruments.insert("ARBUSD".to_string(), InstrumentData::new("ARBUSD", 4, true));
        instruments.insert("ATOMUSD".to_string(), InstrumentData::new("ATOMUSD", 3, true));
        instruments.insert("AUSD".to_string(), InstrumentData::new("AUSD", 4, true));
        instruments.insert("CFXUSD".to_string(), InstrumentData::new("CFXUSD", 4, true));
        instruments.insert("CRVUSD".to_string(), InstrumentData::new("CRVUSD", 4, true));
        instruments.insert("ENSUSD".to_string(), InstrumentData::new("ENSUSD", 2, true));
        instruments.insert("ETCUSD".to_string(), InstrumentData::new("ETCUSD", 2, true));
        instruments.insert("FARTCOINUSD".to_string(), InstrumentData::new("FARTCOINUSD", 6, true));
        instruments.insert("FILUSD".to_string(), InstrumentData::new("FILUSD", 3, true));
        instruments.insert("FLOWUSD".to_string(), InstrumentData::new("FLOWUSD", 4, true));
        instruments.insert("GALAUSD".to_string(), InstrumentData::new("GALAUSD", 5, true));
        instruments.insert("GRTUSD".to_string(), InstrumentData::new("GRTUSD", 4, true));
        instruments.insert("HBARUSD".to_string(), InstrumentData::new("HBARUSD", 5, true));
        instruments.insert("HYPEUSD".to_string(), InstrumentData::new("HYPEUSD", 4, true));
        instruments.insert("ICPUSD".to_string(), InstrumentData::new("ICPUSD", 2, true));
        instruments.insert("IMXUSD".to_string(), InstrumentData::new("IMXUSD", 4, true));
        instruments.insert("INJUSD".to_string(), InstrumentData::new("INJUSD", 2, true));
        instruments.insert("IOTAUSD".to_string(), InstrumentData::new("IOTAUSD", 4, true));
        instruments.insert("IPUSD".to_string(), InstrumentData::new("IPUSD", 4, true));
        instruments.insert("JTOUSD".to_string(), InstrumentData::new("JTOUSD", 3, true));
        instruments.insert("JUPUSD".to_string(), InstrumentData::new("JUPUSD", 4, true));
        instruments.insert("LDOUSD".to_string(), InstrumentData::new("LDOUSD", 3, true));
        instruments.insert("MANAUSD".to_string(), InstrumentData::new("MANAUSD", 4, true));
        instruments.insert("MORPHOUSD".to_string(), InstrumentData::new("MORPHOUSD", 4, true));
        instruments.insert("NEARUSD".to_string(), InstrumentData::new("NEARUSD", 3, true));
        instruments.insert("ONDOUSD".to_string(), InstrumentData::new("ONDOUSD", 4, true));
        instruments.insert("OPUSD".to_string(), InstrumentData::new("OPUSD", 3, true));
        instruments.insert("PENGUUSD".to_string(), InstrumentData::new("PENGUUSD", 5, true));
        instruments.insert("PYTHUSD".to_string(), InstrumentData::new("PYTHUSD", 4, true));
        instruments.insert("RENDERUSD".to_string(), InstrumentData::new("RENDERUSD", 3, true));
        instruments.insert("SANDUSD".to_string(), InstrumentData::new("SANDUSD", 4, true));
        instruments.insert("STXUSD".to_string(), InstrumentData::new("STXUSD", 4, true));
        instruments.insert("SUIUSD".to_string(), InstrumentData::new("SUIUSD", 4, true));
        instruments.insert("SUSD".to_string(), InstrumentData::new("SUSD", 4, true));
        instruments.insert("SYRUPUSD".to_string(), InstrumentData::new("SYRUPUSD", 4, true));
        instruments.insert("TAOUSD".to_string(), InstrumentData::new("TAOUSD", 2, true));
        instruments.insert("THETAUSD".to_string(), InstrumentData::new("THETAUSD", 4, true));
        instruments.insert("TIAUSD".to_string(), InstrumentData::new("TIAUSD", 3, true));
        instruments.insert("TONUSD".to_string(), InstrumentData::new("TONUSD", 3, true));
        instruments.insert("TRUMPUSD".to_string(), InstrumentData::new("TRUMPUSD", 3, true));
        instruments.insert("VIRTUALUSD".to_string(), InstrumentData::new("VIRTUALUSD", 4, true));
        instruments.insert("WIFUSD".to_string(), InstrumentData::new("WIFUSD", 4, true));
        instruments.insert("WLDUSD".to_string(), InstrumentData::new("WLDUSD", 3, true));
        instruments.insert("ADAUSD".to_string(), InstrumentData::new("ADAUSD", 4, true));
        instruments.insert("AVXUSD".to_string(), InstrumentData::new("AVXUSD", 2, true));
        instruments.insert("DOGUSD".to_string(), InstrumentData::new("DOGUSD", 5, true));
        instruments.insert("KSMUSD".to_string(), InstrumentData::new("KSMUSD", 2, true));
        instruments.insert("UNIUSD".to_string(), InstrumentData::new("UNIUSD", 2, true));
        instruments.insert("XRPUSD".to_string(), InstrumentData::new("XRPUSD", 4, true));
        instruments.insert("XTZUSD".to_string(), InstrumentData::new("XTZUSD", 4, true));
        instruments.insert("BNBUSD".to_string(), InstrumentData::new("BNBUSD", 2, true));
        instruments.insert("DOTUSD".to_string(), InstrumentData::new("DOTUSD", 3, true));
        instruments.insert("LNKUSD".to_string(), InstrumentData::new("LNKUSD", 2, true));
        instruments.insert("POLUSD".to_string(), InstrumentData::new("POLUSD", 4, true));
        instruments.insert("SOLUSD".to_string(), InstrumentData::new("SOLUSD", 2, true));
        instruments.insert("XLMUSD".to_string(), InstrumentData::new("XLMUSD", 4, true));
        instruments.insert("XMRUSD".to_string(), InstrumentData::new("XMRUSD", 2, true));
        instruments.insert("GLMUSD".to_string(), InstrumentData::new("GLMUSD", 4, true));
        instruments.insert("VETUSD".to_string(), InstrumentData::new("VETUSD", 5, true));
        instruments.insert("ZECUSD".to_string(), InstrumentData::new("ZECUSD", 2, true));
        instruments.insert("KAIAUSD".to_string(), InstrumentData::new("KAIAUSD", 4, true));
        instruments.insert("SEIUSD".to_string(), InstrumentData::new("SEIUSD", 4, true));
        instruments.insert("MUSD".to_string(), InstrumentData::new("MUSD", 4, true));
        instruments.insert("ENAUSD".to_string(), InstrumentData::new("ENAUSD", 4, true));
        instruments.insert("FETUSD".to_string(), InstrumentData::new("FETUSD", 4, true));
        instruments.insert("CAKEUSD".to_string(), InstrumentData::new("CAKEUSD", 3, true));
        instruments.insert("PENDLEUSD".to_string(), InstrumentData::new("PENDLEUSD", 3, true));
        instruments.insert("DEXEUSD".to_string(), InstrumentData::new("DEXEUSD", 4, true));
        instruments.insert("QNTUSD".to_string(), InstrumentData::new("QNTUSD", 2, true));
        instruments.insert("COMPUSD".to_string(), InstrumentData::new("COMPUSD", 2, true));
        instruments.insert("DYDXUSD".to_string(), InstrumentData::new("DYDXUSD", 3, true));
        instruments.insert("XPLUSD".to_string(), InstrumentData::new("XPLUSD", 4, true));
        instruments.insert("STRKUSD".to_string(), InstrumentData::new("STRKUSD", 4, true));
        instruments.insert("1000xSHIB".to_string(), InstrumentData::new("1000xSHIB", 5, true));
        instruments.insert("1000xPEPE".to_string(), InstrumentData::new("1000xPEPE", 5, true));
        instruments.insert("1000xBONK".to_string(), InstrumentData::new("1000xBONK", 5, true));
        instruments.insert("1000xFLOKI".to_string(), InstrumentData::new("1000xFLOKI", 5, true));
        instruments.insert("WLFIUSD".to_string(), InstrumentData::new("WLFIUSD", 4, true));
        instruments.insert("ASTERUSD".to_string(), InstrumentData::new("ASTERUSD", 4, true));
        instruments.insert("TWTUSD".to_string(), InstrumentData::new("TWTUSD", 4, true));
        instruments.insert("COAIUSD".to_string(), InstrumentData::new("COAIUSD", 4, true));
        instruments.insert("MYXUSD".to_string(), InstrumentData::new("MYXUSD", 4, true));
        instruments.insert("2ZUSD".to_string(), InstrumentData::new("2ZUSD", 4, true));
        instruments.insert("1INCHUSD".to_string(), InstrumentData::new("1INCHUSD", 4, true));
        instruments.insert("TRXUSD".to_string(), InstrumentData::new("TRXUSD", 5, true));

        Self {
            instruments,
            connection_status: "Init".to_string(),
        }
    }
}

impl AppState {
    /// Helper to get EURUSD data (for backward compatibility)
    pub fn eurusd(&self) -> Option<&InstrumentData> {
        self.instruments.get("EURUSD")
    }

    /// Helper to get BTCUSD data (for backward compatibility)
    pub fn btcusd(&self) -> Option<&InstrumentData> {
        self.instruments.get("BTCUSD")
    }

    /// Helper to get mutable EURUSD data
    pub fn eurusd_mut(&mut self) -> Option<&mut InstrumentData> {
        self.instruments.get_mut("EURUSD")
    }

    /// Helper to get mutable BTCUSD data
    pub fn btcusd_mut(&mut self) -> Option<&mut InstrumentData> {
        self.instruments.get_mut("BTCUSD")
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

    /// Get instruments that belong to this category
    pub fn instruments(&self) -> &'static [&'static str] {
        match self {
            SymbolCategory::TradingBots => &["EURUSD", "GBPUSD", "USDCHF", "XAUUSD"],
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
/// This function does nothing now that we use pure Bevy UI
#[allow(dead_code)]
pub fn ui_system() {
    // No-op - using BevyUiPlugin instead
}
