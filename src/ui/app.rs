use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

#[derive(Clone, Copy, Debug)]
pub struct Candle {
    pub time: i64, // Unix timestamp (start of the 1min period)
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
}

#[derive(Resource)]
pub struct AppState {
    pub btc_price: f64,
    pub btc_bid: f64,
    pub btc_ask: f64,
    pub btc_prev_bid: f64,      // Previous BID for color comparison
    pub btc_prev_ask: f64,      // Previous ASK for color comparison
    pub eurusd_bid: f64,
    pub eurusd_ask: f64,
    pub eurusd_prev_bid: f64,   // Previous BID for color comparison
    pub eurusd_prev_ask: f64,   // Previous ASK for color comparison
    pub btc_open: f64,
    pub eurusd_open: f64,
    pub candles: Vec<Candle>,
    pub connection_status: String,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            btc_price: 0.0,
            btc_bid: 0.0,
            btc_ask: 0.0,
            btc_prev_bid: 0.0,
            btc_prev_ask: 0.0,
            eurusd_bid: 0.0,
            eurusd_ask: 0.0,
            eurusd_prev_bid: 0.0,
            eurusd_prev_ask: 0.0,
            btc_open: 0.0,
            eurusd_open: 0.0,
            candles: Vec::new(),
            connection_status: "Init".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum ActiveView {
    #[default]
    Indicators,
    News,
    Analysis,
}

#[derive(Resource)]
pub struct UiState {
    pub active_view: ActiveView,
    pub sidebar_expanded: bool,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            active_view: ActiveView::Indicators,
            sidebar_expanded: true,
        }
    }
}

/// Main UI system that renders the egui interface
pub fn ui_system(
    mut contexts: EguiContexts,
    app_state: Res<AppState>,
    mut ui_state: ResMut<UiState>,
) {
    let ctx = contexts.ctx_mut();

    // Top Panel
    egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.heading("cTrader Rust Terminal");
            ui.separator();
            ui.label(format!("Status: {}", app_state.connection_status));
        });
    });

    // Narrow icon bar (leftmost)
    egui::SidePanel::left("icon_bar")
        .exact_width(50.0)
        .resizable(false)
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(10.0);

                // Indicators icon
                if ui.add_sized(
                    [40.0, 40.0],
                    egui::Button::new("📊")
                        .fill(if ui_state.active_view == ActiveView::Indicators {
                            egui::Color32::from_rgb(40, 40, 60)
                        } else {
                            egui::Color32::from_rgb(30, 30, 30)
                        })
                ).clicked() {
                    ui_state.active_view = ActiveView::Indicators;
                    ui_state.sidebar_expanded = !ui_state.sidebar_expanded;
                }
                ui.small("Indicators");

                ui.add_space(15.0);

                // News icon
                if ui.add_sized(
                    [40.0, 40.0],
                    egui::Button::new("📰")
                        .fill(if ui_state.active_view == ActiveView::News {
                            egui::Color32::from_rgb(40, 40, 60)
                        } else {
                            egui::Color32::from_rgb(30, 30, 30)
                        })
                ).clicked() {
                    ui_state.active_view = ActiveView::News;
                    ui_state.sidebar_expanded = true;
                }
                ui.small("News");

                ui.add_space(15.0);

                // Analysis icon
                if ui.add_sized(
                    [40.0, 40.0],
                    egui::Button::new("🔬")
                        .fill(if ui_state.active_view == ActiveView::Analysis {
                            egui::Color32::from_rgb(40, 40, 60)
                        } else {
                            egui::Color32::from_rgb(30, 30, 30)
                        })
                ).clicked() {
                    ui_state.active_view = ActiveView::Analysis;
                    ui_state.sidebar_expanded = true;
                }
                ui.small("Analysis");
            });
        });

    // Expandable sidebar (shows content based on active view)
    if ui_state.sidebar_expanded {
        egui::SidePanel::left("content_sidebar")
            .exact_width(280.0)
            .resizable(false)
            .show(ctx, |ui| {
                match ui_state.active_view {
                    ActiveView::Indicators => {
                        ui.vertical(|ui| {
                            ui.heading("📊 Indicators");
                            ui.separator();
                            ui.add_space(10.0);

                            // EUR/USD Section
                            ui.group(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label("✓");
                                    ui.label(egui::RichText::new("EURUSD").strong().size(14.0));
                                });

                                ui.add_space(5.0);

                                // Determine color for BID based on price change
                                let eurusd_bid_color = if app_state.eurusd_bid > app_state.eurusd_prev_bid {
                                    egui::Color32::from_rgb(100, 255, 100) // Green for up
                                } else {
                                    egui::Color32::from_rgb(255, 100, 100) // Red for down or equal
                                };

                                // Determine color for ASK based on price change
                                let eurusd_ask_color = if app_state.eurusd_ask > app_state.eurusd_prev_ask {
                                    egui::Color32::from_rgb(100, 255, 100) // Green for up
                                } else {
                                    egui::Color32::from_rgb(255, 100, 100) // Red for down or equal
                                };

                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(format!("{:.5}", app_state.eurusd_bid))
                                        .strong()
                                        .size(15.0)
                                        .color(eurusd_bid_color));
                                    ui.label(egui::RichText::new(format!("{:.5}", app_state.eurusd_ask))
                                        .size(15.0)
                                        .color(eurusd_ask_color));
                                });
                            });

                            ui.add_space(8.0);

                            // BTC/USD Section
                            ui.group(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label("✓");
                                    ui.label(egui::RichText::new("BTCUSD").strong().size(14.0));
                                });

                                ui.add_space(5.0);

                                // Determine color for BID based on price change
                                let btc_bid_color = if app_state.btc_bid > app_state.btc_prev_bid {
                                    egui::Color32::from_rgb(100, 255, 100) // Green for up
                                } else {
                                    egui::Color32::from_rgb(255, 100, 100) // Red for down or equal
                                };

                                // Determine color for ASK based on price change
                                let btc_ask_color = if app_state.btc_ask > app_state.btc_prev_ask {
                                    egui::Color32::from_rgb(100, 255, 100) // Green for up
                                } else {
                                    egui::Color32::from_rgb(255, 100, 100) // Red for down or equal
                                };

                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(format!("{:.2}", app_state.btc_bid))
                                        .strong()
                                        .size(15.0)
                                        .color(btc_bid_color));
                                    ui.label(egui::RichText::new(format!("{:.2}", app_state.btc_ask))
                                        .size(15.0)
                                        .color(btc_ask_color));
                                });
                            });
                        });
                    },
                    ActiveView::News => {
                        ui.heading("📰 News");
                        ui.separator();
                        ui.add_space(10.0);
                        ui.label("Latest financial news will appear here...");
                    },
                    ActiveView::Analysis => {
                        ui.heading("🔬 Analysis");
                        ui.separator();
                        ui.add_space(10.0);
                        ui.label("Market analysis tools will appear here...");
                    },
                }
            });
    }

    // Main Content Area (Central Panel)
    egui::CentralPanel::default().show(ctx, |ui| {
        ui.add_space(10.0);

        // Empty space for now - chart will go here
        ui.vertical_centered(|ui| {
            ui.add_space(200.0);
            ui.label(egui::RichText::new("Chart Area")
                .size(24.0)
                .color(egui::Color32::from_rgb(100, 100, 100)));
            ui.add_space(10.0);
            ui.label(egui::RichText::new("Chart will be built here in the next step")
                .size(14.0)
                .color(egui::Color32::from_rgb(120, 120, 120)));
        });
    });
}
