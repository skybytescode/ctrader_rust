use std::sync::{Arc, Mutex};
use eframe::egui;

#[derive(Clone, Copy, Debug)]
pub struct Candle {
    pub time: i64, // Unix timestamp (start of the 1min period)
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
}

pub struct AppState {
    pub btc_price: f64,
    pub btc_bid: f64,
    pub btc_ask: f64,
    pub candles: Vec<Candle>,
    pub connection_status: String,
}

pub struct CtraderApp {
    pub state: Arc<Mutex<AppState>>,
}

impl CtraderApp {
    pub fn new(state: Arc<Mutex<AppState>>) -> Self {
        Self { state }
    }
}

impl eframe::App for CtraderApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let state = self.state.lock().unwrap();

        // Top Panel
        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("cTrader Rust Terminal");
                ui.separator();
                ui.label(format!("Status: {}", state.connection_status));
            });
        });

        // Left Sidebar
        egui::SidePanel::left("sidebar").show(ctx, |ui| {
            ui.heading("Menu");
            ui.separator();
            if ui.button("Dashboard").clicked() {
                // Handle switch
            }
            if ui.button("Settings").clicked() {
                // Handle switch
            }
            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                ui.label("Version 0.1.0");
            });
        });

        // Main Content
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Market Overview");
            
            ui.add_space(10.0);
            
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label("BTC/USD BID");
                        ui.heading(format!("{:.2}", state.btc_bid));
                    });
                    ui.add_space(40.0);
                    ui.vertical(|ui| {
                        ui.label("BTC/USD ASK");
                        ui.heading(format!("{:.2}", state.btc_ask));
                    });
                });
            });

            ui.add_space(20.0);
            ui.label("Price Chart");
            
            use egui_plot::{BoxElem, BoxPlot, BoxSpread, Plot};
            
            let mut boxes = Vec::new();
            for candle in &state.candles {
                let color = if candle.close >= candle.open {
                    egui::Color32::from_rgb(0, 255, 100) // Bullish Green
                } else {
                    egui::Color32::from_rgb(255, 50, 50) // Bearish Red
                };
                
                let box_elem = BoxElem::new(
                    candle.time as f64,
                    BoxSpread::new(
                        candle.low,
                        candle.open.min(candle.close),
                        candle.close, // median
                        candle.open.max(candle.close),
                        candle.high,
                    )
                )
                .box_width(45.0) // 45 seconds wide for 1 minute candle
                .fill(color)
                .stroke(egui::Stroke::new(1.0, color));
                
                boxes.push(box_elem);
            }
            
            Plot::new("btc_chart")
                .view_aspect(2.0)
                .show(ui, |plot_ui| {
                    if !boxes.is_empty() {
                        plot_ui.box_plot(BoxPlot::new(boxes));
                    }
                });

            ui.add_space(10.0);
            ui.label("Real-time 1-minute Candles from cTrader");
        });

        // Request a repaint to keep the price updating
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}
