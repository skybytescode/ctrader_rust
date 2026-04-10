//! egui-based UI for cTrader Rust Terminal.
//! Replaces the Bevy ECS UI with immediate-mode egui via eframe.

use egui::{self, Color32, RichText, ScrollArea, Vec2};
use tokio::sync::mpsc;
use std::collections::{HashMap, HashSet};
use std::time::Instant;

use crate::PriceUpdate;
use crate::data_retrieval::{
    DataKind, DataAction, DataRequest, DataResponse, SymbolIdMap,
};
use crate::ui::app::{AppState, UiState, SymbolCategory, TickDirection};
use crate::ui::theme::colors;

// ============================================================================
// Enums & state types (moved from bevy_ui/components.rs and resources.rs)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopCardType { Database, TrainModel, StartPause, CurrentStatus }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbSubCardType { HistoryBot, UpdateHistory, Status, Dom }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum MlSubCardType { Model1 = 0, Model2 = 1, Model3 = 2, Model4 = 3, Model5 = 4, Model6 = 5 }
impl MlSubCardType {
    pub fn all() -> [MlSubCardType; 6] {
        [Self::Model1, Self::Model2, Self::Model3, Self::Model4, Self::Model5, Self::Model6]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MlBtnType { Status, Train, FeatureCount }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BotTimeframe { M1Candles, TickData, MLFeatures }

pub const CROSS_PAIRS: [&str; 6] = ["GBPUSD", "USDJPY", "USDCHF", "AUDUSD", "EURJPY", "XAUUSD"];

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickWorkflowStep {
    #[default] Idle,
    CheckingBid, DownloadingBid,
    CheckingAsk, DownloadingAsk,
    CheckingBidForUpdate, CheckingAskForUpdate,
    UpdatingBid, UpdatingAsk,
    Merging,
}

pub struct MlModelState {
    pub status_text: String,
    pub is_training: bool,
    pub last_trained: Option<String>,
    pub training_rx: Option<std::sync::mpsc::Receiver<String>>,
}
impl Default for MlModelState {
    fn default() -> Self {
        Self { status_text: String::new(), is_training: false, last_trained: None, training_rx: None }
    }
}

pub struct MlTrainState { pub states: [MlModelState; 6] }
impl Default for MlTrainState {
    fn default() -> Self { Self { states: std::array::from_fn(|_| MlModelState::default()) } }
}
impl MlTrainState {
    pub fn get(&self, model: MlSubCardType) -> &MlModelState { &self.states[model as usize] }
    pub fn get_mut(&mut self, model: MlSubCardType) -> &mut MlModelState { &mut self.states[model as usize] }
}

#[derive(Default)]
pub struct BotDashboardState {
    pub expanded_top: Option<TopCardType>,
    pub expanded_db_sub: Option<DbSubCardType>,
    pub download_message: Option<String>,
    pub data_status_message: Option<String>,
    pub m1_info: Option<String>,
    pub tick_info: Option<String>,
    pub ml_features_info: Option<String>,
    pub is_downloading: bool,
    pub download_progress: u64,
    pub tick_workflow: TickWorkflowStep,
    pub update_history_message: Option<String>,
    pub is_update_mode: bool,
    pub pending_update_rows: u64,
    pub cross_pair_status: HashMap<String, String>,
    pub cross_pair_update_status: HashMap<String, String>,
    pub updating_cross_pairs: HashSet<String>,
    pub econ_cal_status: String,
    pub econ_cal_is_running: bool,
    pub econ_cal_rx: Option<std::sync::mpsc::Receiver<String>>,
    pub econ_cal_update_status: String,
    pub econ_cal_update_is_running: bool,
    pub econ_cal_update_rx: Option<std::sync::mpsc::Receiver<String>>,
    pub dom_capture_active: bool,
    pub dom_capture_status: String,
    pub ec_capture_active: bool,
    pub ec_today_lines: Vec<String>,
    pub ec_today_raw: Vec<(String, String, i32, String, Option<f64>, Option<f64>, Option<f64>, Option<f64>)>,
    pub ec_status: String,
    // News capture
    pub news_capture_active: bool,
    pub news_today_lines: Vec<String>,
    pub news_status: String,
    pub news_update_status: String,
    pub news_update_is_running: bool,
    pub news_update_rx: Option<std::sync::mpsc::Receiver<String>>,
    // News sentiment analysis (Ollama)
    pub news_analyze_status: String,
    pub news_analyze_is_running: bool,
    pub news_analyze_rx: Option<std::sync::mpsc::Receiver<String>>,
    // News daily analysis (Gemini)
    pub news_gemini_status: String,
    pub news_gemini_is_running: bool,
    pub news_gemini_rx: Option<std::sync::mpsc::Receiver<String>>,
    // ML unified features build
    pub ml_unified_status: String,
    pub ml_unified_is_running: bool,
    pub ml_unified_rx: Option<std::sync::mpsc::Receiver<String>>,
    // Model 4b training
    pub model4b_status: String,
    pub model4b_is_running: bool,
    pub model4b_rx: Option<std::sync::mpsc::Receiver<String>>,
    // Pattern engine display
    pub pattern_lines: Vec<String>,
    // AI analysis display (used by auto-timer DeepSeek)
    pub claude_analysis: String,
    // Manual AI decision buttons
    pub ai_deepseek_status: String,
    pub ai_deepseek_busy: bool,
    pub ai_claude_status: String,
    pub ai_claude_busy: bool,
    pub ai_qwen_status: String,
    pub ai_qwen_busy: bool,
}

// ============================================================================
// Main App
// ============================================================================

pub struct CTraderApp {
    // State
    pub app_state: AppState,
    pub ui_state: UiState,
    pub symbol_map: SymbolIdMap,
    pub dashboard: BotDashboardState,
    pub ml_train: MlTrainState,
    // Channels
    pub price_rx: mpsc::Receiver<PriceUpdate>,
    pub data_req_tx: mpsc::Sender<DataRequest>,
    pub data_resp_rx: mpsc::Receiver<DataResponse>,
    // Shared DB
    pub shared_db: crate::SharedDb,
    // AI decision pending results: (model_name, receiver)
    ai_pending: Vec<(String, std::sync::mpsc::Receiver<String>)>,
    // Timers
    ec_countdown_last: Instant,
    theme_applied: bool,
}

impl CTraderApp {
    pub fn new(
        price_rx: mpsc::Receiver<PriceUpdate>,
        data_req_tx: mpsc::Sender<DataRequest>,
        data_resp_rx: mpsc::Receiver<DataResponse>,
        shared_db: crate::SharedDb,
    ) -> Self {
        Self {
            app_state: AppState::default(),
            ui_state: UiState::default(),
            symbol_map: SymbolIdMap::default(),
            dashboard: BotDashboardState::default(),
            ml_train: MlTrainState::default(),
            price_rx,
            data_req_tx,
            data_resp_rx,
            shared_db,
            ai_pending: Vec::new(),
            ec_countdown_last: Instant::now(),
            theme_applied: false,
        }
    }

    // ── Channel polling ──────────────────────────────────────────────────

    fn poll_price_updates(&mut self) -> bool {
        let mut received = false;
        while let Ok(update) = self.price_rx.try_recv() {
            received = true;
            match update {
                PriceUpdate::InstrumentPrice { symbol, bid, ask } => {
                    if let Some(inst) = self.app_state.instruments.get_mut(&symbol) {
                        inst.update_price(bid, ask);
                    }
                }
                PriceUpdate::ConnectionStatus(s) => self.app_state.connection_status = s,
                PriceUpdate::SymbolMapping(m) => {
                    println!("Received symbol mapping: {} symbols", m.len());
                    self.symbol_map.name_to_id = m;
                }
                PriceUpdate::DomCaptureStatus(s) => self.dashboard.dom_capture_status = s,
                PriceUpdate::EcTodayEvents(lines) => self.dashboard.ec_today_lines = lines,
                PriceUpdate::EcTodayRaw(raw) => self.dashboard.ec_today_raw = raw,
                PriceUpdate::EcStatus(s) => self.dashboard.ec_status = s,
                PriceUpdate::EcCaptureActive(active) => self.dashboard.ec_capture_active = active,
                PriceUpdate::NewsTodayLines(lines) => self.dashboard.news_today_lines = lines,
                PriceUpdate::NewsStatus(s) => self.dashboard.news_status = s,
                PriceUpdate::NewsCaptureActive(active) => self.dashboard.news_capture_active = active,
                PriceUpdate::PatternStatus(lines) => self.dashboard.pattern_lines = lines,
                PriceUpdate::ClaudeAnalysis(text) => self.dashboard.claude_analysis = text,
            }
        }
        received
    }

    fn poll_data_responses(&mut self) -> bool {
        // Process up to 5 responses per frame (balances throughput vs visible progress)
        let mut received = false;
        for _ in 0..5 {
            match self.data_resp_rx.try_recv() {
                Ok(response) => {
                    received = true;
                    process_data_response(
                        response,
                        &mut self.dashboard,
                        &self.symbol_map,
                        &self.data_req_tx,
                    );
                }
                Err(_) => break,
            }
        }
        received
    }

    fn poll_ml_training(&mut self) {
        for i in 0..6 {
            if !self.ml_train.states[i].is_training { continue; }
            let mut lines: Vec<String> = Vec::new();
            let mut done = false;
            if let Some(ref rx) = self.ml_train.states[i].training_rx {
                loop {
                    match rx.try_recv() {
                        Ok(line) if line == "__DONE__" => { done = true; break; }
                        Ok(line) => lines.push(line),
                        Err(_) => break,
                    }
                }
            }
            if !lines.is_empty() {
                self.ml_train.states[i].status_text.push('\n');
                self.ml_train.states[i].status_text.push_str(&lines.join("\n"));
                let count = self.ml_train.states[i].status_text.lines().count();
                if count > 60 {
                    let new_text = self.ml_train.states[i].status_text
                        .lines().skip(count - 60).collect::<Vec<_>>().join("\n");
                    self.ml_train.states[i].status_text = new_text;
                }
            }
            if done {
                let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
                self.ml_train.states[i].last_trained = Some(ts.clone());
                self.ml_train.states[i].status_text.push_str(&format!("\n\nLast trained: {}", ts));
                self.ml_train.states[i].is_training = false;
                self.ml_train.states[i].training_rx = None;
            }
        }
    }

    fn poll_econ_cal(&mut self) {
        poll_background_thread(
            &mut self.dashboard.econ_cal_is_running,
            &mut self.dashboard.econ_cal_status,
            &mut self.dashboard.econ_cal_rx,
        );
        poll_background_thread(
            &mut self.dashboard.econ_cal_update_is_running,
            &mut self.dashboard.econ_cal_update_status,
            &mut self.dashboard.econ_cal_update_rx,
        );
        poll_background_thread(
            &mut self.dashboard.news_update_is_running,
            &mut self.dashboard.news_update_status,
            &mut self.dashboard.news_update_rx,
        );
        poll_background_thread(
            &mut self.dashboard.news_analyze_is_running,
            &mut self.dashboard.news_analyze_status,
            &mut self.dashboard.news_analyze_rx,
        );
        poll_background_thread(
            &mut self.dashboard.news_gemini_is_running,
            &mut self.dashboard.news_gemini_status,
            &mut self.dashboard.news_gemini_rx,
        );
        poll_background_thread(
            &mut self.dashboard.ml_unified_is_running,
            &mut self.dashboard.ml_unified_status,
            &mut self.dashboard.ml_unified_rx,
        );
        poll_background_thread(
            &mut self.dashboard.model4b_is_running,
            &mut self.dashboard.model4b_status,
            &mut self.dashboard.model4b_rx,
        );

        // Poll AI decision results
        self.ai_pending.retain_mut(|(model, rx)| {
            match rx.try_recv() {
                Ok(result) => {
                    let ts = chrono::Local::now().format("%H:%M:%S").to_string();
                    let display = format!("[{}] {}", ts, result);
                    match model.as_str() {
                        "deepseek" => { self.dashboard.ai_deepseek_status = display; self.dashboard.ai_deepseek_busy = false; }
                        "claude" => { self.dashboard.ai_claude_status = display; self.dashboard.ai_claude_busy = false; }
                        "qwen" => { self.dashboard.ai_qwen_status = display; self.dashboard.ai_qwen_busy = false; }
                        _ => {}
                    }
                    false // remove from pending
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => true, // keep waiting
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    match model.as_str() {
                        "deepseek" => { self.dashboard.ai_deepseek_status = "Disconnected".to_string(); self.dashboard.ai_deepseek_busy = false; }
                        "claude" => { self.dashboard.ai_claude_status = "Disconnected".to_string(); self.dashboard.ai_claude_busy = false; }
                        "qwen" => { self.dashboard.ai_qwen_status = "Disconnected".to_string(); self.dashboard.ai_qwen_busy = false; }
                        _ => {}
                    }
                    false
                }
            }
        });
    }

    fn refresh_ec_countdown(&mut self) {
        if self.ec_countdown_last.elapsed().as_secs() < 30 { return; }
        self.ec_countdown_last = Instant::now();
        if self.dashboard.ec_today_raw.is_empty() { return; }
        let lines = crate::ec_realtime::format_ec_lines(&self.dashboard.ec_today_raw);
        if lines != self.dashboard.ec_today_lines {
            self.dashboard.ec_today_lines = lines;
        }
    }

    // ── UI Drawing ───────────────────────────────────────────────────────

    /// Reusable frame for card backgrounds (matches Bevy BG_SIDEBAR + rounded corners)
    fn card_frame() -> egui::Frame {
        egui::Frame::new()
            .fill(colors::BG_SIDEBAR)
            .corner_radius(6.0)
            .inner_margin(egui::Margin::same(10))
    }

    /// Styled button matching Bevy theme: dark bg, white text, rounded
    fn themed_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
        let btn = egui::Button::new(RichText::new(label).size(10.0).color(colors::TEXT_PRIMARY))
            .fill(colors::BG_BUTTON)
            .corner_radius(4.0);
        ui.add(btn)
    }

    /// Small themed button for cross-pair rows (fixed min width)
    fn small_themed_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
        let btn = egui::Button::new(RichText::new(label).size(10.0).color(colors::TEXT_PRIMARY))
            .fill(colors::BG_BUTTON)
            .corner_radius(4.0)
            .min_size(Vec2::new(72.0, 0.0));
        ui.add(btn)
    }

    fn draw_top_panel(&self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top_panel")
            .exact_height(36.0)
            .frame(egui::Frame::new()
                .fill(colors::BG_PANEL)
                .inner_margin(egui::Margin::symmetric(10, 0))
                .stroke(egui::Stroke::new(1.0, colors::BORDER)))
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(RichText::new("cTrader Rust Terminal").size(16.0).color(colors::TEXT_PRIMARY));

                    // Vertical separator
                    let (rect, _) = ui.allocate_exact_size(Vec2::new(1.0, 20.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 0.0, colors::SEPARATOR);

                    ui.label(RichText::new(format!("Status: {}", self.app_state.connection_status))
                        .size(12.0).color(colors::TEXT_SECONDARY));

                    if let Some(symbol) = &self.ui_state.selected_instrument {
                        let (rect, _) = ui.allocate_exact_size(Vec2::new(1.0, 20.0), egui::Sense::hover());
                        ui.painter().rect_filled(rect, 0.0, colors::SEPARATOR);
                        ui.label(RichText::new(symbol).size(12.0).color(colors::BULLISH));
                    }
                });
            });
    }

    fn draw_icon_bar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("icon_bar")
            .exact_width(50.0)
            .frame(egui::Frame::new()
                .fill(colors::BG_DARK)
                .inner_margin(egui::Margin { left: 5, right: 5, top: 10, bottom: 5 })
                .stroke(egui::Stroke::new(1.0, colors::BORDER)))
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    let is_active = true; // Only one view: Bots
                    let bg = if is_active { colors::BG_BUTTON_ACTIVE } else { colors::BG_BUTTON };
                    let btn = egui::Button::new(
                        RichText::new("[B]").size(20.0).color(colors::TEXT_PRIMARY)
                    ).fill(bg).corner_radius(4.0).min_size(Vec2::new(40.0, 40.0));
                    ui.add(btn);
                    ui.label(RichText::new("Bots").size(9.0).color(colors::TEXT_MUTED));
                });
            });
    }

    fn draw_sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("sidebar")
            .exact_width(280.0)
            .frame(egui::Frame::new()
                .fill(colors::BG_SIDEBAR)
                .inner_margin(egui::Margin::same(4))
                .stroke(egui::Stroke::new(1.0, colors::BORDER)))
            .show(ctx, |ui| {
                // Column headers: right-aligned Bid | Ask | Spread
                ui.allocate_ui_with_layout(
                    Vec2::new(ui.available_width(), 20.0),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        ui.add_space(4.0);
                        ui.label(RichText::new("Spread").size(10.0).color(colors::TEXT_MUTED));
                        ui.add_space(16.0);
                        ui.label(RichText::new("Ask").size(10.0).color(colors::TEXT_MUTED));
                        ui.add_space(16.0);
                        ui.label(RichText::new("Bid").size(10.0).color(colors::TEXT_MUTED));
                    },
                );

                // Separator line
                let rect = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), egui::Sense::hover()).0;
                ui.painter().rect_filled(rect, 0.0, colors::SEPARATOR);

                ui.add_space(4.0);

                // Instrument list
                for cat in SymbolCategory::all() {
                    ui.add_space(4.0);
                    ui.label(RichText::new(cat.label()).size(10.0).color(colors::TEXT_MUTED));
                    ui.add_space(2.0);

                    for &symbol in cat.instruments() {
                        let is_selected = self.ui_state.selected_instrument.as_deref() == Some(symbol);
                        let inst = self.app_state.instruments.get(symbol);

                        // Allocate a fixed-height row
                        let row_height = 32.0;
                        let available_width = ui.available_width();
                        let (row_rect, response) = ui.allocate_exact_size(
                            Vec2::new(available_width, row_height),
                            egui::Sense::click(),
                        );

                        // Background
                        let bg = if is_selected {
                            colors::BG_SELECTED
                        } else if response.hovered() {
                            colors::BG_HOVER
                        } else {
                            Color32::TRANSPARENT
                        };
                        ui.painter().rect_filled(row_rect, 0.0, bg);

                        // Symbol name on left
                        ui.painter().text(
                            egui::pos2(row_rect.left() + 8.0, row_rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            symbol,
                            egui::FontId::proportional(12.0),
                            colors::TEXT_PRIMARY,
                        );

                        if let Some(inst) = inst {
                            let tick_color = match inst.tick_direction {
                                TickDirection::Up => colors::BULLISH,
                                TickDirection::Down => colors::BEARISH,
                            };
                            let spread = if inst.bid > 0.0 && inst.ask > 0.0 {
                                (inst.ask - inst.bid) * 100_000.0
                            } else { 0.0 };
                            let dp = inst.decimal_places as usize;

                            // Bid
                            ui.painter().text(
                                egui::pos2(row_rect.right() - 130.0, row_rect.center().y),
                                egui::Align2::RIGHT_CENTER,
                                format!("{:.width$}", inst.bid, width = dp),
                                egui::FontId::monospace(11.0),
                                tick_color,
                            );
                            // Ask
                            ui.painter().text(
                                egui::pos2(row_rect.right() - 60.0, row_rect.center().y),
                                egui::Align2::RIGHT_CENTER,
                                format!("{:.width$}", inst.ask, width = dp),
                                egui::FontId::monospace(11.0),
                                tick_color,
                            );
                            // Spread
                            ui.painter().text(
                                egui::pos2(row_rect.right() - 4.0, row_rect.center().y),
                                egui::Align2::RIGHT_CENTER,
                                format!("{:.1}", spread),
                                egui::FontId::monospace(10.0),
                                colors::TEXT_MUTED,
                            );
                        }

                        if response.clicked() {
                            self.ui_state.selected_instrument = Some(symbol.to_string());
                        }
                    }
                }
            });
    }

    fn draw_dashboard(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(colors::BG_DARKEST).inner_margin(egui::Margin::same(20)))
            .show(ctx, |ui| {
                if self.ui_state.selected_instrument.is_none() {
                    ui.centered_and_justified(|ui| {
                        ui.label(RichText::new("Select an instrument from the sidebar")
                            .size(14.0).color(colors::TEXT_MUTED));
                    });
                    return;
                }

                let symbol = self.ui_state.selected_instrument.clone().unwrap();

                // Dashboard header with border bottom
                ui.label(RichText::new(format!("{} Bot Dashboard", symbol))
                    .size(20.0).color(colors::TEXT_PRIMARY));
                let rect = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), egui::Sense::hover()).0;
                ui.painter().rect_filled(rect, 0.0, colors::BORDER);
                ui.add_space(8.0);

                // 2x2 card grid (or 75/25 accordion when expanded)
                ScrollArea::vertical().show(ui, |ui| {
                    let expanded = self.dashboard.expanded_top;
                    let total_width = ui.available_width().max(100.0);
                    let gap = 12.0;

                    if let Some(exp) = expanded {
                        // Expanded layout: main card ~74% | stacked sidebar ~24%
                        let main_w = (total_width * 0.74 - gap).max(50.0);
                        let side_w = (total_width * 0.24).max(50.0);

                        ui.horizontal_top(|ui| {
                            ui.vertical(|ui| {
                                ui.set_width(main_w);
                                self.draw_card(ui, exp, &symbol, true);
                            });
                            ui.add_space(gap);
                            ui.vertical(|ui| {
                                ui.set_width(side_w);
                                let others = [TopCardType::Database, TopCardType::TrainModel,
                                              TopCardType::StartPause, TopCardType::CurrentStatus];
                                for ct in others {
                                    if ct != exp {
                                        self.draw_card(ui, ct, &symbol, false);
                                        ui.add_space(6.0);
                                    }
                                }
                            });
                        });
                    } else {
                        // Default 2x2 grid
                        let half = ((total_width - gap) / 2.0).max(50.0);
                        // Row 1: Database | Train Model
                        ui.horizontal_top(|ui| {
                            ui.vertical(|ui| {
                                ui.set_width(half);
                                self.draw_card(ui, TopCardType::Database, &symbol, false);
                            });
                            ui.add_space(gap);
                            ui.vertical(|ui| {
                                ui.set_width(half);
                                self.draw_card(ui, TopCardType::TrainModel, &symbol, false);
                            });
                        });
                        ui.add_space(gap);
                        // Row 2: Start/Pause | Real-Time Data
                        ui.horizontal_top(|ui| {
                            ui.vertical(|ui| {
                                ui.set_width(half);
                                self.draw_card(ui, TopCardType::StartPause, &symbol, false);
                            });
                            ui.add_space(gap);
                            ui.vertical(|ui| {
                                ui.set_width(half);
                                self.draw_card(ui, TopCardType::CurrentStatus, &symbol, false);
                            });
                        });
                    }
                });
            });
    }

    /// Draw a single dashboard card with header [title ... [+]/[-]] and content
    fn draw_card(&mut self, ui: &mut egui::Ui, card_type: TopCardType, symbol: &str, _is_expanded: bool) {
        let title = match card_type {
            TopCardType::Database => "Database",
            TopCardType::TrainModel => "Train Model",
            TopCardType::StartPause => "Start / Pause",
            TopCardType::CurrentStatus => "Real-Time Data",
        };
        let expanded = self.dashboard.expanded_top;
        let is_this_expanded = expanded == Some(card_type);
        let icon = if is_this_expanded { "[-]" } else { "[+]" };

        // Always show content for Database, TrainModel, CurrentStatus;
        // others only when expanded
        let always_show = matches!(card_type,
            TopCardType::Database | TopCardType::TrainModel | TopCardType::CurrentStatus);
        let show_content = always_show || is_this_expanded;

        Self::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());

            // Header row: title + expand button
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).size(14.0).color(colors::TEXT_PRIMARY).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let btn = egui::Button::new(
                        RichText::new(icon).size(12.0).color(colors::TEXT_PRIMARY)
                    ).fill(colors::BG_BUTTON).corner_radius(4.0);
                    if ui.add(btn).clicked() {
                        if is_this_expanded {
                            self.dashboard.expanded_top = None;
                        } else {
                            self.dashboard.expanded_top = Some(card_type);
                        }
                    }
                });
            });

            if show_content {
                ui.add_space(6.0);
                match card_type {
                    TopCardType::Database => self.draw_database_content(ui, symbol),
                    TopCardType::TrainModel => self.draw_train_model_content(ui),
                    TopCardType::StartPause => self.draw_start_pause_content(ui),
                    TopCardType::CurrentStatus => self.draw_realtime_data_content(ui),
                }
            }
        });
    }

    // ── Status Info Bar ──────────────────────────────────────────────────

    fn draw_status_bar(&self, ui: &mut egui::Ui) {
        if let Some(ref info) = self.dashboard.m1_info {
            ui.label(RichText::new(info).size(10.0).color(colors::ACCENT_BLUE));
        }
        if let Some(ref info) = self.dashboard.tick_info {
            ui.label(RichText::new(info).size(10.0).color(Color32::from_rgb(200, 200, 80)));
        }
        if let Some(ref info) = self.dashboard.ml_features_info {
            ui.label(RichText::new(info).size(10.0).color(colors::BULLISH));
        }
    }

    // ── Database Card Content ────────────────────────────────────────────

    fn draw_database_content(&mut self, ui: &mut egui::Ui, symbol: &str) {
        // Status info bar inside card
        self.draw_status_bar(ui);

        ui.add_space(4.0);

        // 2x2 sub-card grid
        ui.columns(2, |cols| {
            self.draw_history_bot_subcard(&mut cols[0], symbol);
            self.draw_update_history_subcard(&mut cols[1], symbol);
        });
    }

    fn draw_history_bot_subcard(&mut self, ui: &mut egui::Ui, symbol: &str) {
        egui::Frame::new()
            .fill(colors::BG_SIDEBAR)
            .corner_radius(6.0)
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.label(RichText::new("History BoT").size(12.0).color(colors::TEXT_PRIMARY));
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    if Self::themed_button(ui, "M1 Candles").clicked() {
                        self.handle_timeframe_click(symbol, DbSubCardType::HistoryBot, BotTimeframe::M1Candles);
                    }
                    if Self::themed_button(ui, "Tick Data").clicked() {
                        self.handle_timeframe_click(symbol, DbSubCardType::HistoryBot, BotTimeframe::TickData);
                    }
                    if Self::themed_button(ui, "ML Features").clicked() {
                        self.handle_timeframe_click(symbol, DbSubCardType::HistoryBot, BotTimeframe::MLFeatures);
                    }
                });
                if let Some(ref msg) = self.dashboard.download_message {
                    ui.add_space(2.0);
                    ui.label(RichText::new(msg).size(10.0).color(colors::TEXT_MUTED));
                }

                // Cross-pair section
                ui.add_space(6.0);
                ui.label(RichText::new("Cross-Pair M1 Data:").size(10.0).color(colors::TEXT_MUTED));
                for &sym in CROSS_PAIRS.iter() {
                    ui.horizontal(|ui| {
                        if Self::small_themed_button(ui, sym).clicked() {
                            self.handle_cross_pair_click(sym);
                        }
                        let status = self.dashboard.cross_pair_status.get(sym)
                            .map(|s| s.as_str()).unwrap_or("--");
                        ui.label(RichText::new(status).size(10.0).color(colors::TEXT_MUTED));
                    });
                }

                // Economic Calendar section
                ui.add_space(6.0);
                ui.label(RichText::new("Economic Calendar:").size(10.0).color(colors::TEXT_MUTED));
                ui.horizontal(|ui| {
                    if Self::themed_button(ui, "Economic Calendar").clicked() {
                        self.handle_econ_cal_click();
                    }
                    ui.label(RichText::new(&self.dashboard.econ_cal_status).size(10.0).color(colors::TEXT_MUTED));
                });

                // News section
                ui.add_space(6.0);
                ui.label(RichText::new("News:").size(10.0).color(colors::TEXT_MUTED));
                ui.horizontal(|ui| {
                    if Self::themed_button(ui, "News").clicked() {
                        self.handle_news_db_click();
                    }
                    ui.label(RichText::new(&self.dashboard.news_status).size(10.0).color(colors::TEXT_MUTED));
                });
            });
    }

    fn draw_update_history_subcard(&mut self, ui: &mut egui::Ui, symbol: &str) {
        egui::Frame::new()
            .fill(colors::BG_SIDEBAR)
            .corner_radius(6.0)
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.label(RichText::new("Update History").size(12.0).color(colors::TEXT_PRIMARY));
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    if Self::themed_button(ui, "M1 Candles").clicked() {
                        self.handle_timeframe_click(symbol, DbSubCardType::UpdateHistory, BotTimeframe::M1Candles);
                    }
                    if Self::themed_button(ui, "Tick Data").clicked() {
                        self.handle_timeframe_click(symbol, DbSubCardType::UpdateHistory, BotTimeframe::TickData);
                    }
                    if Self::themed_button(ui, "ML Features").clicked() {
                        self.handle_timeframe_click(symbol, DbSubCardType::UpdateHistory, BotTimeframe::MLFeatures);
                    }
                });
                if let Some(ref msg) = self.dashboard.update_history_message {
                    ui.add_space(2.0);
                    ui.label(RichText::new(msg).size(10.0).color(colors::TEXT_MUTED));
                }

                // Cross-pair update section
                ui.add_space(6.0);
                ui.label(RichText::new("Cross-Pair M1 Update:").size(10.0).color(colors::TEXT_MUTED));
                for &sym in CROSS_PAIRS.iter() {
                    ui.horizontal(|ui| {
                        if Self::small_themed_button(ui, sym).clicked() {
                            self.handle_cross_pair_update_click(sym);
                        }
                        let status = self.dashboard.cross_pair_update_status.get(sym)
                            .map(|s| s.as_str()).unwrap_or("--");
                        ui.label(RichText::new(status).size(10.0).color(colors::TEXT_MUTED));
                    });
                }

                // EC Update section
                ui.add_space(6.0);
                ui.label(RichText::new("Economic Calendar:").size(10.0).color(colors::TEXT_MUTED));
                ui.horizontal(|ui| {
                    if Self::themed_button(ui, "Update EC").clicked() {
                        self.handle_econ_cal_update_click();
                    }
                    ui.label(RichText::new(&self.dashboard.econ_cal_update_status).size(10.0).color(colors::TEXT_MUTED));
                });

                // News Update section
                ui.add_space(6.0);
                ui.label(RichText::new("News:").size(10.0).color(colors::TEXT_MUTED));
                ui.horizontal(|ui| {
                    if Self::themed_button(ui, "Update News").clicked() {
                        self.handle_news_update_click();
                    }
                    ui.label(RichText::new(&self.dashboard.news_update_status).size(10.0).color(colors::TEXT_MUTED));
                });

                // News Sentiment Analysis section
                ui.add_space(6.0);
                ui.label(RichText::new("News Sentiment (Ollama AI):").size(10.0).color(colors::TEXT_MUTED));
                ui.horizontal(|ui| {
                    if Self::themed_button(ui, "Analyze News").clicked() {
                        self.handle_news_analyze_click();
                    }
                    if self.dashboard.news_analyze_is_running {
                        if Self::themed_button(ui, "Stop").clicked() {
                            // Signal stop by dropping the receiver
                            self.dashboard.news_analyze_rx = None;
                            self.dashboard.news_analyze_is_running = false;
                            self.dashboard.news_analyze_status = "Stopped by user".to_string();
                        }
                    }
                });
                if !self.dashboard.news_analyze_status.is_empty() {
                    ui.label(RichText::new(&self.dashboard.news_analyze_status).size(10.0).color(colors::TEXT_MUTED));
                }

                // News Daily Analysis (Gemini)
                ui.add_space(6.0);
                ui.label(RichText::new("News Daily (Gemini Flash):").size(10.0).color(colors::TEXT_MUTED));
                ui.horizontal(|ui| {
                    if Self::themed_button(ui, "Analyze Today").clicked() {
                        self.handle_news_gemini_click();
                    }
                });
                if !self.dashboard.news_gemini_status.is_empty() {
                    ui.label(RichText::new(&self.dashboard.news_gemini_status).size(10.0).color(colors::TEXT_MUTED));
                }

                // ML Unified Features Build (Model 4b)
                ui.add_space(6.0);
                ui.label(RichText::new("ML Features (Model 4b):").size(10.0).color(colors::TEXT_MUTED));
                ui.horizontal(|ui| {
                    if Self::themed_button(ui, "ml_unified_features_4b").clicked() {
                        self.handle_ml_unified_build_click();
                    }
                });
                if !self.dashboard.ml_unified_status.is_empty() {
                    ui.label(RichText::new(&self.dashboard.ml_unified_status).size(10.0).color(colors::TEXT_MUTED));
                }
            });
    }

    // ── Train Model Card Content ─────────────────────────────────────────

    fn draw_train_model_content(&mut self, ui: &mut egui::Ui) {
        let titles = [
            "Model 1 -- Technical Indicators",
            "Model 2 -- Regime Detection HMM",
            "Model 3 -- Chart Patterns CNN",
            "Model 4 -- Economic Calendar",
            "Model 5 -- Depth of Market",
            "Model 6 -- Ensemble",
        ];
        for model in MlSubCardType::all() {
            let idx = model as usize;
            egui::Frame::new()
                .fill(colors::BG_SIDEBAR)
                .corner_radius(6.0)
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.label(RichText::new(titles[idx]).size(12.0).color(colors::TEXT_PRIMARY));
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        if Self::themed_button(ui, "Status").clicked() {
                            self.handle_ml_btn(model, MlBtnType::Status);
                        }
                        if Self::themed_button(ui, "Update Training").clicked() {
                            self.handle_ml_btn(model, MlBtnType::Train);
                        }
                        if Self::themed_button(ui, "Feature Count").clicked() {
                            self.handle_ml_btn(model, MlBtnType::FeatureCount);
                        }
                    });
                    let text = &self.ml_train.states[idx].status_text;
                    if !text.is_empty() {
                        ui.add_space(4.0);
                        egui::Frame::new()
                            .fill(Color32::from_rgba_premultiplied(0, 0, 0, 50))
                            .corner_radius(4.0)
                            .inner_margin(egui::Margin::same(4))
                            .show(ui, |ui| {
                                ScrollArea::vertical()
                                    .id_salt(format!("ml_scroll_{}", idx))
                                    .max_height(220.0)
                                    .show(ui, |ui| {
                                        ui.label(RichText::new(text).size(10.0)
                                            .color(colors::TEXT_MUTED).font(egui::FontId::monospace(10.0)));
                                    });
                            });
                    }
                });
            ui.add_space(4.0);
        }

        // Model 4b — Unified (EC + News + Candle Patterns + Cross-Pairs)
        egui::Frame::new()
            .fill(colors::BG_SIDEBAR)
            .corner_radius(6.0)
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.label(RichText::new("Model 4b -- EC + News + Price (XGBoost)").size(12.0).color(colors::TEXT_PRIMARY));
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    if Self::themed_button(ui, "Train Model 4b").clicked() {
                        self.handle_train_model4b_click();
                    }
                    if Self::themed_button(ui, "Status").clicked() {
                        self.handle_model4b_status_click();
                    }
                });
                if !self.dashboard.model4b_status.is_empty() {
                    ui.add_space(4.0);
                    egui::Frame::new()
                        .fill(Color32::from_rgba_premultiplied(0, 0, 0, 50))
                        .corner_radius(4.0)
                        .inner_margin(egui::Margin::same(4))
                        .show(ui, |ui| {
                            ScrollArea::vertical()
                                .id_salt("model4b_scroll")
                                .max_height(220.0)
                                .show(ui, |ui| {
                                    ui.label(RichText::new(&self.dashboard.model4b_status).size(10.0)
                                        .color(colors::TEXT_MUTED).font(egui::FontId::monospace(10.0)));
                                });
                        });
                }
            });
        ui.add_space(4.0);
    }

    // ── Start / Pause Card Content ───────────────────────────────────────

    fn draw_start_pause_content(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("AI Trading Decisions:").size(11.0).color(colors::TEXT_SECONDARY));
        ui.add_space(4.0);

        // DeepSeek R1 button
        ui.horizontal(|ui| {
            let btn = if self.dashboard.ai_deepseek_busy {
                ui.add_enabled(false, egui::Button::new("DeepSeek R1"))
            } else {
                Self::themed_button(ui, "DeepSeek R1")
            };
            if btn.clicked() && !self.dashboard.ai_deepseek_busy {
                self.trigger_ai_decision("deepseek");
            }
            if !self.dashboard.ai_deepseek_status.is_empty() {
                ui.label(RichText::new(&self.dashboard.ai_deepseek_status).size(9.0).color(colors::TEXT_MUTED));
            }
        });

        // Claude CLI button
        ui.horizontal(|ui| {
            let btn = if self.dashboard.ai_claude_busy {
                ui.add_enabled(false, egui::Button::new("Claude"))
            } else {
                Self::themed_button(ui, "Claude")
            };
            if btn.clicked() && !self.dashboard.ai_claude_busy {
                self.trigger_ai_decision("claude");
            }
            if !self.dashboard.ai_claude_status.is_empty() {
                ui.label(RichText::new(&self.dashboard.ai_claude_status).size(9.0).color(colors::TEXT_MUTED));
            }
        });

        // Qwen 2.5 button
        ui.horizontal(|ui| {
            let btn = if self.dashboard.ai_qwen_busy {
                ui.add_enabled(false, egui::Button::new("Qwen 2.5"))
            } else {
                Self::themed_button(ui, "Qwen 2.5")
            };
            if btn.clicked() && !self.dashboard.ai_qwen_busy {
                self.trigger_ai_decision("qwen");
            }
            if !self.dashboard.ai_qwen_status.is_empty() {
                ui.label(RichText::new(&self.dashboard.ai_qwen_status).size(9.0).color(colors::TEXT_MUTED));
            }
        });

        // Results display
        for (label, status) in [
            ("DeepSeek", &self.dashboard.ai_deepseek_status),
            ("Claude", &self.dashboard.ai_claude_status),
            ("Qwen", &self.dashboard.ai_qwen_status),
        ] {
            if status.contains('\n') {
                ui.add_space(4.0);
                egui::Frame::new()
                    .fill(Color32::from_rgba_premultiplied(0, 40, 0, 50))
                    .corner_radius(4.0)
                    .inner_margin(egui::Margin::same(4))
                    .show(ui, |ui| {
                        ui.label(RichText::new(format!("[{}]", label)).size(9.0).color(colors::TEXT_SECONDARY));
                        ScrollArea::vertical()
                            .id_salt(format!("ai_{}_scroll", label))
                            .max_height(100.0)
                            .show(ui, |ui| {
                                ui.label(RichText::new(status).size(9.0)
                                    .color(colors::TEXT_PRIMARY)
                                    .font(egui::FontId::monospace(9.0)));
                            });
                    });
            }
        }
    }

    // ── Real-Time Data Card Content ──────────────────────────────────────

    fn draw_realtime_data_content(&mut self, ui: &mut egui::Ui) {
        // DoM Section
        ui.horizontal(|ui| {
            ui.label(RichText::new("Price DoM:").size(10.0).color(colors::TEXT_SECONDARY));
            let btn_label = if self.dashboard.dom_capture_active { "Pause Capture" } else { "Start Capture" };
            if Self::themed_button(ui, btn_label).clicked() {
                self.handle_dom_capture_click();
            }
            ui.label(RichText::new(&self.dashboard.dom_capture_status).size(10.0).color(colors::TEXT_SECONDARY));
        });

        ui.add_space(8.0);

        // EC Calendar Section
        ui.horizontal(|ui| {
            ui.label(RichText::new("EC Calendar:").size(10.0).color(colors::TEXT_SECONDARY));
            let ec_btn_label = if self.dashboard.ec_capture_active { "Stop Capture" } else { "Start Capture" };
            if Self::themed_button(ui, ec_btn_label).clicked() {
                self.handle_ec_capture_click();
            }
            if !self.dashboard.ec_status.is_empty() {
                ui.label(RichText::new(&self.dashboard.ec_status).size(10.0).color(colors::TEXT_SECONDARY));
            } else {
                ui.label(RichText::new("Stopped").size(10.0).color(colors::TEXT_SECONDARY));
            }
        });

        egui::Frame::new()
            .fill(Color32::from_rgba_premultiplied(0, 0, 0, 50))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::same(4))
            .show(ui, |ui| {
                if !self.dashboard.ec_today_lines.is_empty() {
                    let text = self.dashboard.ec_today_lines.join("\n");
                    ScrollArea::vertical()
                        .id_salt("ec_scroll")
                        .max_height(180.0)
                        .show(ui, |ui| {
                            ui.label(RichText::new(&text).size(11.0).color(colors::TEXT_SECONDARY));
                        });
                } else {
                    ui.label(RichText::new("Waiting for econcal proxy...").size(11.0).color(colors::TEXT_SECONDARY));
                }
            });

        ui.add_space(8.0);

        // News Section
        ui.horizontal(|ui| {
            ui.label(RichText::new("News:").size(10.0).color(colors::TEXT_SECONDARY));
            let news_btn_label = if self.dashboard.news_capture_active { "Stop News Capture" } else { "Start News Capture" };
            if Self::themed_button(ui, news_btn_label).clicked() {
                self.handle_news_capture_click();
            }
            if !self.dashboard.news_status.is_empty() {
                ui.label(RichText::new(&self.dashboard.news_status).size(10.0).color(colors::TEXT_SECONDARY));
            } else {
                ui.label(RichText::new("Stopped").size(10.0).color(colors::TEXT_SECONDARY));
            }
        });

        egui::Frame::new()
            .fill(Color32::from_rgba_premultiplied(0, 0, 0, 50))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::same(4))
            .show(ui, |ui| {
                if !self.dashboard.news_today_lines.is_empty() {
                    let text = self.dashboard.news_today_lines.join("\n");
                    ScrollArea::vertical()
                        .id_salt("news_scroll")
                        .max_height(180.0)
                        .show(ui, |ui| {
                            ui.label(RichText::new(&text).size(11.0).color(colors::TEXT_SECONDARY));
                        });
                } else {
                    ui.label(RichText::new("Waiting for news capture...").size(11.0).color(colors::TEXT_SECONDARY));
                }
            });

        ui.add_space(8.0);

        // Pattern Detection Section
        ui.horizontal(|ui| {
            ui.label(RichText::new("Pattern Detection:").size(10.0).color(colors::TEXT_SECONDARY));
        });

        egui::Frame::new()
            .fill(Color32::from_rgba_premultiplied(0, 0, 0, 50))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::same(4))
            .show(ui, |ui| {
                if !self.dashboard.pattern_lines.is_empty() {
                    let text = self.dashboard.pattern_lines.join("\n");
                    ScrollArea::vertical()
                        .id_salt("pattern_scroll")
                        .max_height(160.0)
                        .show(ui, |ui| {
                            ui.label(RichText::new(&text).size(10.0)
                                .color(colors::TEXT_SECONDARY)
                                .font(egui::FontId::monospace(10.0)));
                        });
                } else {
                    ui.label(RichText::new("Waiting for pattern data...").size(11.0).color(colors::TEXT_SECONDARY));
                }
            });

        // Claude Analysis Section
        if !self.dashboard.claude_analysis.is_empty() {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Claude Analysis:").size(10.0).color(colors::TEXT_SECONDARY));
            });
            egui::Frame::new()
                .fill(Color32::from_rgba_premultiplied(0, 40, 0, 50))
                .corner_radius(4.0)
                .inner_margin(egui::Margin::same(4))
                .show(ui, |ui| {
                    ScrollArea::vertical()
                        .id_salt("claude_analysis_scroll")
                        .max_height(120.0)
                        .show(ui, |ui| {
                            ui.label(RichText::new(&self.dashboard.claude_analysis).size(10.0)
                                .color(colors::TEXT_PRIMARY)
                                .font(egui::FontId::monospace(10.0)));
                        });
                });
        }
    }

    // ── Event Handlers ───────────────────────────────────────────────────

    fn handle_timeframe_click(&mut self, symbol: &str, parent_card: DbSubCardType, timeframe: BotTimeframe) {
        let symbol_id = match self.symbol_map.name_to_id.get(symbol) {
            Some(&id) => id,
            None => {
                let msg = format!("Symbol ID not found for {}. Wait for connection.", symbol);
                if parent_card == DbSubCardType::UpdateHistory {
                    self.dashboard.update_history_message = Some(msg);
                } else {
                    self.dashboard.download_message = Some(msg);
                }
                return;
            }
        };

        // ML Features
        if timeframe == BotTimeframe::MLFeatures {
            let force = parent_card == DbSubCardType::UpdateHistory;
            let req = DataRequest {
                symbol: symbol.to_string(), symbol_id,
                kind: DataKind::M1Candles,
                action: DataAction::BuildMLFeatures,
                force_rebuild: force,
            };
            if let Err(e) = self.data_req_tx.try_send(req) {
                let msg = format!("Failed: {}", e);
                if force { self.dashboard.update_history_message = Some(msg); }
                else { self.dashboard.download_message = Some(msg); }
            } else {
                let msg = if force {
                    self.dashboard.is_update_mode = true;
                    format!("Updating ML features for {}...", symbol)
                } else {
                    format!("Checking ML features table for {}...", symbol)
                };
                if force { self.dashboard.update_history_message = Some(msg); }
                else { self.dashboard.download_message = Some(msg); }
            }
            return;
        }

        if self.dashboard.is_downloading {
            let msg = "Download already in progress...".to_string();
            self.dashboard.download_message = Some(msg);
            return;
        }

        // Tick Data
        if timeframe == BotTimeframe::TickData {
            if parent_card == DbSubCardType::HistoryBot {
                self.dashboard.tick_workflow = TickWorkflowStep::CheckingBid;
                let req = DataRequest {
                    symbol: symbol.to_string(), symbol_id,
                    kind: DataKind::TickData, action: DataAction::CheckStatus,
                    force_rebuild: false,
                };
                if let Err(e) = self.data_req_tx.try_send(req) {
                    self.dashboard.download_message = Some(format!("Failed: {}", e));
                    self.dashboard.tick_workflow = TickWorkflowStep::Idle;
                } else {
                    self.dashboard.download_message = Some(format!("Checking {} bid ticks in database...", symbol));
                }
            } else {
                self.dashboard.tick_workflow = TickWorkflowStep::CheckingBidForUpdate;
                self.dashboard.is_update_mode = true;
                let req = DataRequest {
                    symbol: symbol.to_string(), symbol_id,
                    kind: DataKind::TickData, action: DataAction::CheckStatus,
                    force_rebuild: false,
                };
                if let Err(e) = self.data_req_tx.try_send(req) {
                    self.dashboard.update_history_message = Some(format!("Failed: {}", e));
                    self.dashboard.tick_workflow = TickWorkflowStep::Idle;
                    self.dashboard.is_update_mode = false;
                } else {
                    self.dashboard.update_history_message = Some(format!("Checking {} bid ticks in database...", symbol));
                }
            }
            return;
        }

        // M1 Candles
        let action = match parent_card {
            DbSubCardType::HistoryBot => DataAction::CheckStatus,
            DbSubCardType::UpdateHistory => DataAction::UpdateLatest,
            _ => return,
        };
        let req = DataRequest {
            symbol: symbol.to_string(), symbol_id,
            kind: DataKind::M1Candles, action, force_rebuild: false,
        };
        if let Err(e) = self.data_req_tx.try_send(req) {
            self.dashboard.data_status_message = Some(format!("Failed to send request: {}", e));
            return;
        }
        match action {
            DataAction::CheckStatus => {
                self.dashboard.is_update_mode = false;
                self.dashboard.download_message = Some(format!("Checking {} M1 Candles in database...", symbol));
            }
            DataAction::UpdateLatest => {
                self.dashboard.is_update_mode = true;
                self.dashboard.is_downloading = true;
                self.dashboard.download_progress = 0;
                self.dashboard.update_history_message = Some(format!("Updating {} M1 history...", symbol));
            }
            _ => {}
        }
    }

    fn handle_cross_pair_click(&mut self, symbol: &str) {
        let symbol_id = match self.symbol_map.name_to_id.get(symbol) {
            Some(&id) => id,
            None => {
                self.dashboard.cross_pair_status.insert(symbol.to_string(), "Symbol ID not found. Wait for connection.".to_string());
                return;
            }
        };
        self.dashboard.cross_pair_status.insert(symbol.to_string(), "Checking...".to_string());
        let _ = self.data_req_tx.try_send(DataRequest {
            symbol: symbol.to_string(), symbol_id,
            kind: DataKind::M1Candles, action: DataAction::CheckStatus, force_rebuild: false,
        });
    }

    fn handle_cross_pair_update_click(&mut self, symbol: &str) {
        let symbol_id = match self.symbol_map.name_to_id.get(symbol) {
            Some(&id) => id,
            None => {
                self.dashboard.cross_pair_update_status.insert(symbol.to_string(), "Symbol ID not found. Wait for connection.".to_string());
                return;
            }
        };
        self.dashboard.updating_cross_pairs.insert(symbol.to_string());
        self.dashboard.cross_pair_update_status.insert(symbol.to_string(), "Updating...".to_string());
        let _ = self.data_req_tx.try_send(DataRequest {
            symbol: symbol.to_string(), symbol_id,
            kind: DataKind::M1Candles, action: DataAction::UpdateLatest, force_rebuild: false,
        });
    }

    fn handle_econ_cal_click(&mut self) {
        if self.dashboard.econ_cal_is_running {
            self.dashboard.econ_cal_status = "Already running...".to_string();
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        self.dashboard.econ_cal_is_running = true;
        self.dashboard.econ_cal_status = "Starting...".to_string();
        self.dashboard.econ_cal_rx = Some(rx);
        spawn_python_thread(tx, "ml.model4_news.scrape_econcal");
    }

    fn handle_econ_cal_update_click(&mut self) {
        if self.dashboard.econ_cal_update_is_running {
            self.dashboard.econ_cal_update_status = "Already running...".to_string();
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        self.dashboard.econ_cal_update_is_running = true;
        self.dashboard.econ_cal_update_status = "Updating...".to_string();
        self.dashboard.econ_cal_update_rx = Some(rx);
        spawn_python_thread(tx, "ml.model4_news.update_econcal");
    }

    fn handle_dom_capture_click(&mut self) {
        let action = if self.dashboard.dom_capture_active {
            DataAction::DomCaptureStop
        } else {
            DataAction::DomCaptureStart
        };
        let req = DataRequest {
            symbol: "EURUSD".to_string(), symbol_id: 0,
            kind: DataKind::M1Candles, action, force_rebuild: false,
        };
        if let Err(e) = self.data_req_tx.try_send(req) {
            self.dashboard.dom_capture_status = format!("Send error: {}", e);
        } else {
            self.dashboard.dom_capture_active = !self.dashboard.dom_capture_active;
            self.dashboard.dom_capture_status = if self.dashboard.dom_capture_active {
                "Starting...".to_string()
            } else {
                "Stopping...".to_string()
            };
        }
    }

    fn handle_ec_capture_click(&mut self) {
        let action = if self.dashboard.ec_capture_active {
            DataAction::EcCaptureStop
        } else {
            DataAction::EcCaptureStart
        };
        let req = DataRequest {
            symbol: "EURUSD".to_string(), symbol_id: 0,
            kind: DataKind::M1Candles, action, force_rebuild: false,
        };
        if let Err(e) = self.data_req_tx.try_send(req) {
            self.dashboard.ec_status = format!("Send error: {}", e);
        } else {
            self.dashboard.ec_capture_active = !self.dashboard.ec_capture_active;
            self.dashboard.ec_status = if self.dashboard.ec_capture_active {
                "Starting...".to_string()
            } else {
                "Stopping...".to_string()
            };
        }
    }

    fn handle_news_capture_click(&mut self) {
        let action = if self.dashboard.news_capture_active {
            DataAction::NewsCaptureStop
        } else {
            DataAction::NewsCaptureStart
        };
        let req = DataRequest {
            symbol: "EURUSD".to_string(), symbol_id: 0,
            kind: DataKind::M1Candles, action, force_rebuild: false,
        };
        if let Err(e) = self.data_req_tx.try_send(req) {
            self.dashboard.news_status = format!("Send error: {}", e);
        } else {
            self.dashboard.news_capture_active = !self.dashboard.news_capture_active;
            self.dashboard.news_status = if self.dashboard.news_capture_active {
                "Starting...".to_string()
            } else {
                "Stopping...".to_string()
            };
        }
    }

    fn handle_news_db_click(&mut self) {
        let _lock = self.shared_db.lock().unwrap_or_else(|e| e.into_inner());
        let status = match duckdb::Connection::open(crate::DB_PATH) {
            Ok(db) => {
                let today_count: i64 = db.query_row(
                    "SELECT COUNT(*) FROM news_today", [], |r| r.get(0)
                ).unwrap_or(0);
                let hist_count: i64 = db.query_row(
                    "SELECT COUNT(*) FROM news_historical", [], |r| r.get(0)
                ).unwrap_or(0);
                format!("Today: {} | Historical: {}", today_count, hist_count)
            }
            Err(e) => format!("DB error: {}", e),
        };
        self.dashboard.news_status = status;
    }

    fn handle_news_update_click(&mut self) {
        if self.dashboard.news_update_is_running {
            self.dashboard.news_update_status = "Already running...".to_string();
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        self.dashboard.news_update_is_running = true;
        self.dashboard.news_update_status = "Fetching news...".to_string();
        self.dashboard.news_update_rx = Some(rx);

        let db_clone = self.shared_db.clone();
        std::thread::spawn(move || {
            // Check DB for latest stored article
            let latest = {
                let _lock = db_clone.lock().unwrap_or_else(|e| e.into_inner());
                duckdb::Connection::open(crate::DB_PATH).ok()
                    .and_then(|db| crate::news_realtime::get_latest_news_timestamp(&db))
            };

            if let Some(ref ts) = latest {
                let _ = tx.send(format!("Latest in DB: {} — fetching newer...", ts));
            } else {
                let _ = tx.send("No existing news — full fetch...".to_string());
            }

            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            let db_clone2 = db_clone.clone();
            let result = match rt {
                Ok(rt) => rt.block_on(async {
                    match crate::news_realtime::fetch_news_since(
                        100, 500, latest.as_deref(), Some(&tx)
                    ).await {
                        Ok(rows) => {
                            let count = rows.len();
                            let _ = tx.send(format!("Writing {} new articles to DB...", count));
                            {
                                let _lock = db_clone2.lock().unwrap_or_else(|e| e.into_inner());
                                match duckdb::Connection::open(crate::DB_PATH) {
                                    Ok(db) => {
                                        let _ = crate::news_realtime::write_news_to_db(&db, &rows);
                                        format!("Done: {} new articles stored", count)
                                    }
                                    Err(e) => format!("DB error: {}", e),
                                }
                            }
                        }
                        Err(e) => {
                            if latest.is_some() && e.contains("No articles") {
                                "Already up to date".to_string()
                            } else {
                                format!("Fetch error: {}", e)
                            }
                        }
                    }
                }),
                Err(e) => format!("Runtime error: {}", e),
            };
            let _ = tx.send(result);
            let _ = tx.send("__DONE__".to_string());
        });
    }

    fn handle_news_analyze_click(&mut self) {
        if self.dashboard.news_analyze_is_running {
            self.dashboard.news_analyze_status = "Already running...".to_string();
            return;
        }

        // Check pending count first
        let pending_info = {
            let _lock = self.shared_db.lock().unwrap_or_else(|e| e.into_inner());
            match duckdb::Connection::open(crate::DB_PATH) {
                Ok(db) => {
                    let _ = crate::news_sentiment::ensure_table(&db);
                    crate::news_sentiment::count_pending(&db).ok()
                }
                Err(_) => None,
            }
        };

        if let Some((total, analyzed)) = pending_info {
            let pending = total - analyzed;
            if pending == 0 {
                self.dashboard.news_analyze_status = format!(
                    "All {} articles already analyzed.", total
                );
                return;
            }
            self.dashboard.news_analyze_status = format!(
                "Starting: {} pending of {} total...", pending, total
            );
        }

        let (tx, rx) = std::sync::mpsc::channel::<String>();
        self.dashboard.news_analyze_is_running = true;
        self.dashboard.news_analyze_rx = Some(rx);
        let db_clone = self.shared_db.clone();

        std::thread::spawn(move || {
            let result = match crate::news_sentiment::analyze_batch_sync(
                &db_clone, &tx, 50_000
            ) {
                Ok(n) => format!("Done: {} articles analyzed", n),
                Err(e) => format!("Error: {}", e),
            };
            let _ = tx.send(result);
            let _ = tx.send("__DONE__".to_string());
        });
    }

    fn handle_news_gemini_click(&mut self) {
        if self.dashboard.news_gemini_is_running {
            self.dashboard.news_gemini_status = "Already running...".to_string();
            return;
        }

        self.dashboard.news_gemini_status = "Starting Gemini analysis...".to_string();
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        self.dashboard.news_gemini_is_running = true;
        self.dashboard.news_gemini_rx = Some(rx);
        let db_clone = self.shared_db.clone();

        std::thread::spawn(move || {
            let result = match crate::news_gemini::analyze_today_news(&db_clone, Some(&tx)) {
                Ok(n) => format!("Done: {} articles analyzed with Gemini", n),
                Err(e) => format!("Error: {}", e),
            };
            let _ = tx.send(result);
            let _ = tx.send("__DONE__".to_string());
        });
    }

    fn trigger_ai_decision(&mut self, model: &str) {
        let model = model.to_string();

        // Read all data for the prompt
        let (news_lines, ec_lines, model_preds) = {
            let _lock = self.shared_db.lock().unwrap_or_else(|e| e.into_inner());
            let db = duckdb::Connection::open(crate::DB_PATH).ok();
            let news = db.as_ref()
                .map(|d| crate::news_gemini::read_for_claude(d))
                .unwrap_or_default();
            let ec = db.as_ref()
                .map(|d| crate::ec_realtime::read_ec_today(d))
                .unwrap_or_default();
            let models = db.and_then(|d| {
                d.query_row(
                    "SELECT data FROM ml_predictions_live LIMIT 1",
                    [],
                    |row| row.get::<_, String>(0),
                ).ok()
            })
            .map(|json_str| crate::format_model_predictions(&json_str))
            .unwrap_or_else(|| "  Models not available".to_string());
            (news, ec, models)
        };

        let pattern_status = self.dashboard.pattern_lines.join("\n");
        let prompt = build_decisive_prompt(&pattern_status, &news_lines, &ec_lines, &model_preds);

        // Set busy state
        match model.as_str() {
            "deepseek" => { self.dashboard.ai_deepseek_busy = true; self.dashboard.ai_deepseek_status = "Asking DeepSeek R1...".to_string(); }
            "claude" => { self.dashboard.ai_claude_busy = true; self.dashboard.ai_claude_status = "Asking Claude...".to_string(); }
            "qwen" => { self.dashboard.ai_qwen_busy = true; self.dashboard.ai_qwen_status = "Asking Qwen 2.5...".to_string(); }
            _ => {}
        }

        let (tx, rx) = std::sync::mpsc::channel::<String>();

        let model_clone = model.clone();
        std::thread::spawn(move || {
            let result = match model_clone.as_str() {
                "deepseek" => call_ollama_decision(&prompt, "deepseek-r1:8b-llama-distill-q4_K_M"),
                "claude" => {
                    match crate::pattern_engine::call_claude_pattern_analysis(&prompt) {
                        Ok(resp) => resp.display_summary(),
                        Err(e) => format!("Error: {}", e),
                    }
                }
                "qwen" => call_ollama_decision(&prompt, "qwen2.5:3b-instruct-q5_K_M"),
                _ => "Unknown model".to_string(),
            };
            let _ = tx.send(result);
        });

        self.ai_pending.push((model, rx));
    }

    fn handle_train_model4b_click(&mut self) {
        if self.dashboard.model4b_is_running {
            self.dashboard.model4b_status = "Already running...".to_string();
            return;
        }

        // Check if feature table exists
        let has_features = {
            let _lock = self.shared_db.lock().unwrap_or_else(|e| e.into_inner());
            duckdb::Connection::open(crate::DB_PATH).ok()
                .and_then(|db| db.query_row(
                    "SELECT COUNT(*) FROM ml_unified_features", [], |r| r.get::<_, i64>(0)
                ).ok())
                .unwrap_or(0) > 0
        };

        if !has_features {
            self.dashboard.model4b_status = "No feature table found. Click 'ml_unified_features_4b' in Database tab first.".to_string();
            return;
        }

        let (tx, rx) = std::sync::mpsc::channel::<String>();
        self.dashboard.model4b_is_running = true;
        self.dashboard.model4b_status = "Starting training...".to_string();
        self.dashboard.model4b_rx = Some(rx);

        std::thread::spawn(move || {
            let result = std::process::Command::new("C:/Windows/py.exe")
                .args(["-3.12", "-u", "ml/train_unified.py"])
                .current_dir("D:/RustProjects/ctrader_rust")
                .env("PYTHONIOENCODING", "utf-8")
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn();

            match result {
                Err(e) => {
                    let _ = tx.send(format!("Failed to start Python: {}", e));
                    let _ = tx.send("__DONE__".to_string());
                }
                Ok(mut child) => {
                    use std::io::BufRead;

                    // Read stderr on separate thread
                    let stderr_tx = tx.clone();
                    let stderr_handle = child.stderr.take().map(|stderr| {
                        std::thread::spawn(move || {
                            let reader = std::io::BufReader::new(stderr);
                            for line in reader.lines().flatten() {
                                if !line.trim().is_empty() {
                                    let _ = stderr_tx.send(format!("ERR: {}", line));
                                }
                            }
                        })
                    });

                    // Read stdout
                    if let Some(stdout) = child.stdout.take() {
                        let reader = std::io::BufReader::new(stdout);
                        for line in reader.lines() {
                            match line {
                                Ok(l) => { let _ = tx.send(l); }
                                Err(_) => break,
                            }
                        }
                    }

                    if let Some(handle) = stderr_handle {
                        let _ = handle.join();
                    }

                    let status = child.wait().unwrap_or_else(|_| std::process::ExitStatus::default());
                    let code = status.code().unwrap_or(-1);
                    if code != 0 {
                        let _ = tx.send(format!("Training exited with code: {}", code));
                    }
                    let _ = tx.send("__DONE__".to_string());
                }
            }
        });
    }

    fn handle_model4b_status_click(&mut self) {
        // Read metrics from saved file
        let metrics_path = "ml/trained/unified_metrics.json";
        match std::fs::read_to_string(metrics_path) {
            Ok(contents) => {
                if let Ok(metrics) = serde_json::from_str::<serde_json::Value>(&contents) {
                    let acc = metrics.get("accuracy").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let baseline = metrics.get("baseline").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let train_rows = metrics.get("train_rows").and_then(|v| v.as_i64()).unwrap_or(0);
                    let test_rows = metrics.get("test_rows").and_then(|v| v.as_i64()).unwrap_or(0);
                    let features = metrics.get("features").and_then(|v| v.as_i64()).unwrap_or(0);
                    let best_iter = metrics.get("best_iteration").and_then(|v| v.as_i64()).unwrap_or(0);

                    let mut text = format!(
                        "Model 4b — EC + News + Price (XGBoost)\n\
                         ────────────────────────────────────────\n\
                         Accuracy:       {:.1}%\n\
                         Baseline:       {:.1}%\n\
                         Edge:           +{:.1}%\n\
                         Train rows:     {}\n\
                         Test rows:      {}\n\
                         Features:       {}\n\
                         Best iteration: {}\n",
                        acc * 100.0, baseline * 100.0, (acc - baseline) * 100.0,
                        train_rows, test_rows, features, best_iter
                    );

                    if let Some(top) = metrics.get("top_features").and_then(|v| v.as_object()) {
                        text.push_str("\nTop Features:\n");
                        let mut sorted: Vec<_> = top.iter().collect();
                        sorted.sort_by(|a, b| b.1.as_f64().unwrap_or(0.0)
                            .partial_cmp(&a.1.as_f64().unwrap_or(0.0)).unwrap());
                        for (name, val) in sorted.iter().take(15) {
                            text.push_str(&format!("  {:<28} {:.4}\n", name, val.as_f64().unwrap_or(0.0)));
                        }
                    }

                    self.dashboard.model4b_status = text;
                } else {
                    self.dashboard.model4b_status = "Error parsing metrics JSON".to_string();
                }
            }
            Err(_) => {
                self.dashboard.model4b_status = "Model 4b not yet trained.\nClick 'Train Model 4b' to start.".to_string();
            }
        }
    }

    fn handle_ml_unified_build_click(&mut self) {
        if self.dashboard.ml_unified_is_running {
            self.dashboard.ml_unified_status = "Already running...".to_string();
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        self.dashboard.ml_unified_is_running = true;
        self.dashboard.ml_unified_status = "Building features...".to_string();
        self.dashboard.ml_unified_rx = Some(rx);
        let db_clone = self.shared_db.clone();

        std::thread::spawn(move || {
            let start = std::time::Instant::now();

            let result = (|| -> Result<String, String> {
                // Step 1: Build M5 candles
                let _ = tx.send("Step 1/4: Building M5 candles from M1...".to_string());
                {
                    let _lock = db_clone.lock().unwrap_or_else(|e| e.into_inner());
                    let db = duckdb::Connection::open(crate::DB_PATH)
                        .map_err(|e| format!("DB open: {}", e))?;
                    db.execute_batch("
                        DROP TABLE IF EXISTS eurusd_m5;
                        CREATE TABLE eurusd_m5 AS
                        SELECT
                            (timestamp // 300) * 300 AS timestamp,
                            FIRST(open ORDER BY timestamp) AS open,
                            MAX(high) AS high,
                            MIN(low) AS low,
                            LAST(close ORDER BY timestamp) AS close,
                            SUM(volume) AS volume,
                            COUNT(*) AS m1_count
                        FROM eurusd_m1
                        WHERE timestamp >= 1739923200
                        GROUP BY (timestamp // 300) * 300
                        HAVING COUNT(*) >= 3
                        ORDER BY timestamp;
                    ").map_err(|e| format!("M5 build: {}", e))?;
                    let m5_count: i64 = db.query_row("SELECT COUNT(*) FROM eurusd_m5", [], |r| r.get(0)).unwrap_or(0);
                    let _ = tx.send(format!("Step 1/4: {} M5 candles built", m5_count));
                }

                // Step 2: Build cross-pair M5
                let _ = tx.send("Step 2/4: Building cross-pair M5...".to_string());
                {
                    let _lock = db_clone.lock().unwrap_or_else(|e| e.into_inner());
                    let db = duckdb::Connection::open(crate::DB_PATH)
                        .map_err(|e| format!("DB open: {}", e))?;
                    db.execute_batch("
                        DROP TABLE IF EXISTS cross_m5;
                        CREATE TABLE cross_m5 AS
                        WITH pairs AS (
                            SELECT (timestamp // 300) * 300 AS ts, 'gbpusd' AS pair, LAST(close ORDER BY timestamp) AS close FROM gbpusd_m1 WHERE timestamp >= 1739923200 GROUP BY (timestamp // 300) * 300
                            UNION ALL SELECT (timestamp // 300) * 300, 'usdjpy', LAST(close ORDER BY timestamp) FROM usdjpy_m1 WHERE timestamp >= 1739923200 GROUP BY (timestamp // 300) * 300
                            UNION ALL SELECT (timestamp // 300) * 300, 'usdchf', LAST(close ORDER BY timestamp) FROM usdchf_m1 WHERE timestamp >= 1739923200 GROUP BY (timestamp // 300) * 300
                            UNION ALL SELECT (timestamp // 300) * 300, 'eurjpy', LAST(close ORDER BY timestamp) FROM eurjpy_m1 WHERE timestamp >= 1739923200 GROUP BY (timestamp // 300) * 300
                            UNION ALL SELECT (timestamp // 300) * 300, 'audusd', LAST(close ORDER BY timestamp) FROM audusd_m1 WHERE timestamp >= 1739923200 GROUP BY (timestamp // 300) * 300
                            UNION ALL SELECT (timestamp // 300) * 300, 'xauusd', LAST(close ORDER BY timestamp) FROM xauusd_m1 WHERE timestamp >= 1739923200 GROUP BY (timestamp // 300) * 300
                        )
                        SELECT ts as timestamp,
                            MAX(CASE WHEN pair='gbpusd' THEN close END) AS gbpusd_close,
                            MAX(CASE WHEN pair='usdjpy' THEN close END) AS usdjpy_close,
                            MAX(CASE WHEN pair='usdchf' THEN close END) AS usdchf_close,
                            MAX(CASE WHEN pair='eurjpy' THEN close END) AS eurjpy_close,
                            MAX(CASE WHEN pair='audusd' THEN close END) AS audusd_close,
                            MAX(CASE WHEN pair='xauusd' THEN close END) AS xauusd_close
                        FROM pairs GROUP BY ts ORDER BY ts;
                    ").map_err(|e| format!("Cross M5 build: {}", e))?;
                    let _ = tx.send("Step 2/4: Cross-pair M5 done".to_string());
                }

                // Step 3: Prepare EC and News timestamps
                let _ = tx.send("Step 3/4: Preparing EC calendar and News timestamps...".to_string());
                {
                    let _lock = db_clone.lock().unwrap_or_else(|e| e.into_inner());
                    let db = duckdb::Connection::open(crate::DB_PATH)
                        .map_err(|e| format!("DB open: {}", e))?;
                    db.execute_batch("
                        DROP TABLE IF EXISTS ec_events_ts;
                        CREATE TABLE ec_events_ts AS
                        SELECT
                            CAST(EPOCH(timestamp_utc) AS BIGINT) AS event_ts,
                            event_name, currency, volatility,
                            COALESCE(actual, 0) AS actual,
                            COALESCE(forecast, 0) AS forecast,
                            COALESCE(surprise, 0) AS surprise,
                            COALESCE(beats_forecast, 0) AS beats_forecast
                        FROM eurusd_economic_calendar
                        WHERE CAST(EPOCH(timestamp_utc) AS BIGINT) >= 1739923200
                        ORDER BY event_ts;

                        DROP TABLE IF EXISTS news_ts;
                        CREATE TABLE news_ts AS
                        SELECT
                            CAST(EPOCH(STRPTIME(published_utc, '%Y-%m-%dT%H:%M:%S')) AS BIGINT) AS pub_ts,
                            COALESCE(eur_sentiment, 0) AS eur_sentiment,
                            COALESCE(usd_sentiment, 0) AS usd_sentiment,
                            COALESCE(eurusd_impact, 0) AS eurusd_impact,
                            COALESCE(volatility_expected, 0) AS vol_expected,
                            COALESCE(relevance, 0) AS relevance,
                            COALESCE(category, 'other') AS category
                        FROM news_sentiment
                        WHERE published_utc IS NOT NULL AND eurusd_impact IS NOT NULL
                        ORDER BY pub_ts;
                    ").map_err(|e| format!("EC/News prep: {}", e))?;
                    let _ = tx.send("Step 3/4: EC and News timestamps ready".to_string());
                }

                // Step 4: Build unified feature table
                let _ = tx.send("Step 4/4: Building unified feature table (this may take a minute)...".to_string());
                {
                    let _lock = db_clone.lock().unwrap_or_else(|e| e.into_inner());
                    let db = duckdb::Connection::open(crate::DB_PATH)
                        .map_err(|e| format!("DB open: {}", e))?;

                    // Read and execute the SQL file
                    let sql = std::fs::read_to_string("build_features.sql")
                        .map_err(|e| format!("Read build_features.sql: {}", e))?;
                    db.execute_batch(&sql)
                        .map_err(|e| format!("Build features: {}", e))?;

                    let row_count: i64 = db.query_row(
                        "SELECT COUNT(*) FROM ml_unified_features", [], |r| r.get(0)
                    ).unwrap_or(0);
                    let col_count: i64 = db.query_row(
                        "SELECT COUNT(*) FROM information_schema.columns WHERE table_name='ml_unified_features'",
                        [], |r| r.get(0)
                    ).unwrap_or(0);

                    // Export CSV
                    let _ = db.execute_batch(
                        "COPY ml_unified_features TO 'ml/ml_unified_features.csv' (HEADER, DELIMITER ',');"
                    );

                    let elapsed = start.elapsed().as_secs();
                    Ok(format!("Done: {} rows x {} cols | CSV exported | {}s",
                        row_count, col_count, elapsed))
                }
            })();

            let msg = match result {
                Ok(s) => s,
                Err(e) => format!("Error: {}", e),
            };
            let _ = tx.send(msg);
            let _ = tx.send("__DONE__".to_string());
        });
    }

    fn handle_ml_btn(&mut self, model: MlSubCardType, btn_type: MlBtnType) {
        match btn_type {
            MlBtnType::Status => {
                let last = self.ml_train.get(model).last_trained.clone();
                let mut text = read_ml_model_status(model);
                if let Some(ts) = last {
                    text.push_str(&format!("\n\nLast trained: {}", ts));
                }
                self.ml_train.get_mut(model).status_text = text;
            }
            MlBtnType::FeatureCount => {
                self.ml_train.get_mut(model).status_text = read_ml_model_features(model);
            }
            MlBtnType::Train => {
                let ms = self.ml_train.get_mut(model);
                if ms.is_training {
                    ms.status_text = "Training already in progress...".to_string();
                    return;
                }
                let module = match model {
                    MlSubCardType::Model1 => Some("ml.model1_technical.train"),
                    MlSubCardType::Model2 => Some("ml.model2_regime.train"),
                    MlSubCardType::Model3 => Some("ml.model3_cnn.train"),
                    MlSubCardType::Model4 => Some("ml.model4_econcal.train"),
                    _ => None,
                };
                if let Some(module_path) = module {
                    let (tx, rx) = std::sync::mpsc::channel::<String>();
                    ms.is_training = true;
                    ms.status_text = "Starting training...\n".to_string();
                    ms.training_rx = Some(rx);
                    spawn_ml_training_thread(tx, module_path);
                } else {
                    ms.status_text = "Training script not yet implemented for this model.".to_string();
                }
            }
        }
    }
}

impl eframe::App for CTraderApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.theme_applied {
            crate::ui::theme::apply_theme(ctx);
            self.theme_applied = true;
        }

        // Poll all channels — track if any data arrived
        let mut has_data = false;
        has_data |= self.poll_price_updates();
        has_data |= self.poll_data_responses();
        self.poll_ml_training();
        self.poll_econ_cal();
        self.refresh_ec_countdown();

        // Draw UI
        self.draw_top_panel(ctx);
        self.draw_icon_bar(ctx);
        self.draw_sidebar(ctx);
        self.draw_dashboard(ctx);

        // Smart repaint: fast when data arrives, slow when idle
        if has_data {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
    }
}

// ============================================================================
// Free functions (helpers)
// ============================================================================

fn is_cross_pair(symbol: &str) -> bool {
    CROSS_PAIRS.contains(&symbol)
}

fn poll_background_thread(
    is_running: &mut bool,
    status: &mut String,
    rx_opt: &mut Option<std::sync::mpsc::Receiver<String>>,
) {
    if !*is_running { return; }
    let mut messages: Vec<String> = Vec::new();
    let mut done = false;
    if let Some(rx) = rx_opt.as_ref() {
        loop {
            match rx.try_recv() {
                Ok(line) if line == "__DONE__" => { done = true; break; }
                Ok(line) => messages.push(line),
                Err(_) => break,
            }
        }
    }
    if let Some(last) = messages.into_iter().last() {
        *status = last;
    }
    if done {
        *is_running = false;
        *rx_opt = None;
    }
}

/// Build a decisive trading prompt from current state.
fn build_decisive_prompt(
    pattern_status: &str,
    news_lines: &[String],
    ec_lines: &[String],
    model_preds: &str,
) -> String {
    let news_section = if news_lines.is_empty() {
        "NEWS TODAY:\n  No news data".to_string()
    } else {
        format!("NEWS TODAY ({} articles):\n{}", news_lines.len(),
            news_lines.iter().map(|l| format!("  {}", l)).collect::<Vec<_>>().join("\n"))
    };

    let ec_section = if ec_lines.is_empty() {
        "ECONOMIC CALENDAR:\n  No events".to_string()
    } else {
        format!("ECONOMIC CALENDAR:\n{}",
            ec_lines.iter().map(|l| format!("  {}", l)).collect::<Vec<_>>().join("\n"))
    };

    let now = chrono::Utc::now();
    let time_str = now.format("%H:%M UTC").to_string();

    format!(
r#"You are an expert EUR/USD forex trader making real-time trading decisions.

CRITICAL RULES:
- You MUST choose enter_long or enter_short if ANY valid setup exists. Only say "wait" if there is genuinely NO pattern and NO directional bias.
- Be DECISIVE. Traders lose money by waiting too long. If the data shows a direction, commit to it.
- A strong trend with a pullback is an ENTRY opportunity, not a reason to wait.
- News-driven moves can extend — do NOT dismiss them due to session quality.

Time: {time_str}

PATTERN DETECTION (real-time):
{pattern_status}

{news_section}

{ec_section}

ML MODELS:
{model_preds}

Based on ALL data above, output ONLY valid JSON:

{{
  "m15_bias": "<bullish/bearish/neutral>",
  "m15_pattern": "<what you see>",
  "m5_bias": "<bullish/bearish/neutral>",
  "m5_pattern": "<what you see>",
  "dominant_bias": "<bullish/bearish/neutral>",
  "dominant_bias_confidence": <0.0-1.0>,
  "news_driven": <true/false>,
  "news_impact": "<which news matters or none>",
  "session_quality": "<good/moderate/poor>",
  "recommended_action": "<enter_long/enter_short/wait>",
  "entry_timeframe": "<M15/M5/none>",
  "entry_condition": "<specific reason for your decision>",
  "key_resistance": <price or 0>,
  "key_support": <price or 0>,
  "target_pips": <number or 0>,
  "stop_pips": <number or 0>,
  "invalidation": "<what cancels this>"
}}"#)
}

/// Call Ollama with a prompt and return formatted text result.
fn call_ollama_decision(prompt: &str, model: &str) -> String {
    #[derive(serde::Serialize)]
    struct Req { model: String, prompt: String, stream: bool, keep_alive: String }
    #[derive(serde::Deserialize)]
    struct Resp { #[serde(default)] response: String }

    let client = reqwest::blocking::Client::new();
    let req = Req {
        model: model.to_string(),
        prompt: prompt.to_string(),
        stream: false,
        keep_alive: "30m".to_string(),
    };

    let resp = match client
        .post("http://localhost:11434/api/generate")
        .json(&req)
        .timeout(std::time::Duration::from_secs(120))
        .send()
    {
        Ok(r) => r,
        Err(e) => return format!("Error: {}", e),
    };

    if !resp.status().is_success() {
        return format!("Error: HTTP {}", resp.status());
    }

    let ollama: Resp = match resp.json() {
        Ok(r) => r,
        Err(e) => return format!("Error: parse {}", e),
    };

    let text = ollama.response.trim();

    // DeepSeek R1: strip <think>...</think> block
    let text = if let Some(end) = text.find("</think>") {
        text[end + 8..].trim()
    } else {
        text
    };

    // Try to parse as TradingDecision for nice display
    let json_clean = if text.contains("```") {
        text.lines().filter(|l| !l.trim().starts_with("```")).collect::<Vec<_>>().join("\n")
    } else {
        text.to_string()
    };

    let json_str = if let Some(start) = json_clean.find('{') {
        if let Some(end) = json_clean.rfind('}') {
            &json_clean[start..=end]
        } else { &json_clean }
    } else { &json_clean };

    match serde_json::from_str::<crate::decision_engine::TradingDecision>(json_str) {
        Ok(d) => d.display_summary(),
        Err(_) => {
            // Return raw text if can't parse JSON (truncated)
            if text.len() > 500 { format!("{}...", &text[..500]) } else { text.to_string() }
        }
    }
}

/// Spawn a Python subprocess with concurrent stdout/stderr reading.
/// `always_report_exit`: if true, always send exit code; if false, only on error.
fn spawn_python_subprocess(
    tx: std::sync::mpsc::Sender<String>,
    module_path: &str,
    always_report_exit: bool,
) {
    let module_path = module_path.to_string();
    std::thread::spawn(move || {
        let result = std::process::Command::new("C:/Windows/py.exe")
            .args(["-3.12", "-u", "-m", &module_path])
            .current_dir("D:/RustProjects/ctrader_rust")
            .env("PYTHONIOENCODING", "utf-8")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn();

        match result {
            Err(e) => {
                let msg = if always_report_exit {
                    format!("ERROR: failed to start Python: {}", e)
                } else {
                    format!("ERROR: {}", e)
                };
                let _ = tx.send(msg);
                let _ = tx.send("__DONE__".to_string());
            }
            Ok(mut child) => {
                use std::io::BufRead;

                // Read stderr concurrently on a separate thread
                let stderr_tx = tx.clone();
                let stderr_handle = child.stderr.take().map(|stderr| {
                    std::thread::spawn(move || {
                        let reader = std::io::BufReader::new(stderr);
                        for line in reader.lines().flatten() {
                            if !line.trim().is_empty() {
                                let _ = stderr_tx.send(format!("ERR: {}", line));
                            }
                        }
                    })
                });

                // Read stdout on current thread
                if let Some(stdout) = child.stdout.take() {
                    let reader = std::io::BufReader::new(stdout);
                    for line in reader.lines() {
                        match line {
                            Ok(l) => { let _ = tx.send(l); }
                            Err(_) => break,
                        }
                    }
                }

                // Wait for stderr thread to finish
                if let Some(handle) = stderr_handle {
                    let _ = handle.join();
                }

                let status = child.wait().unwrap_or_else(|_| std::process::ExitStatus::default());
                let code = status.code().unwrap_or(-1);
                if always_report_exit {
                    let _ = tx.send(format!("Training finished (exit code: {})", code));
                } else if code != 0 {
                    let _ = tx.send(format!("Error (exit code: {})", code));
                }
                let _ = tx.send("__DONE__".to_string());
            }
        }
    });
}

/// Spawn ML training thread — always reports exit code on completion.
fn spawn_ml_training_thread(tx: std::sync::mpsc::Sender<String>, module_path: &str) {
    spawn_python_subprocess(tx, module_path, true);
}

/// Spawn Python thread — only reports exit code on error.
fn spawn_python_thread(tx: std::sync::mpsc::Sender<String>, module_path: &str) {
    spawn_python_subprocess(tx, module_path, false);
}

/// Process a single DataResponse and update dashboard state.
fn process_data_response(
    response: DataResponse,
    ds: &mut BotDashboardState,
    symbol_map: &SymbolIdMap,
    req_tx: &mpsc::Sender<DataRequest>,
) {
    let workflow = ds.tick_workflow;

    match response {
        DataResponse::StatusFound { symbol, kind, count, first_record, last_record, .. } => {
            match kind {
                DataKind::M1Candles if is_cross_pair(&symbol) => {
                    ds.cross_pair_status.insert(symbol, format!(
                        "{} M1 bars\n{}\n{}\nConsider updating", count, first_record, last_record
                    ));
                }
                DataKind::M1Candles => {
                    ds.m1_info = Some(format!("{} M1 candles in DB\n{}\n{}", count, first_record, last_record));
                    ds.is_downloading = false;
                    if ds.is_update_mode {
                        let rows = ds.pending_update_rows;
                        ds.update_history_message = Some(format!(
                            "Update successful! +{} new M1 candles.\n{}", rows, last_record
                        ));
                        ds.is_update_mode = false;
                        ds.pending_update_rows = 0;
                    } else {
                        ds.download_message = Some(format!("{} M1 candles: {} found. Consider 'Update History'.", symbol, count));
                    }
                }
                DataKind::TickData if workflow == TickWorkflowStep::CheckingBid => {
                    ds.download_message = Some(format!("Bid ticks in DB ({} rows). Checking ask ticks...", count));
                    ds.tick_workflow = TickWorkflowStep::CheckingAsk;
                    if let Some(&sid) = symbol_map.name_to_id.get(&symbol) {
                        let _ = req_tx.try_send(DataRequest { symbol, symbol_id: sid, kind: DataKind::TickDataAsk, action: DataAction::CheckStatus, force_rebuild: false });
                    }
                }
                DataKind::TickDataAsk if workflow == TickWorkflowStep::CheckingAsk => {
                    ds.download_message = Some("Bid and ask ticks in DB. Merging in process...".into());
                    ds.tick_workflow = TickWorkflowStep::Merging;
                    if let Some(&sid) = symbol_map.name_to_id.get(&symbol) {
                        let _ = req_tx.try_send(DataRequest { symbol, symbol_id: sid, kind: DataKind::TickData, action: DataAction::MergeBidAsk, force_rebuild: false });
                    }
                }
                DataKind::TickData if workflow == TickWorkflowStep::CheckingBidForUpdate => {
                    ds.update_history_message = Some(format!("{}\nChecking ask ticks...", last_record));
                    ds.tick_workflow = TickWorkflowStep::CheckingAskForUpdate;
                    if let Some(&sid) = symbol_map.name_to_id.get(&symbol) {
                        let _ = req_tx.try_send(DataRequest { symbol, symbol_id: sid, kind: DataKind::TickDataAsk, action: DataAction::CheckStatus, force_rebuild: false });
                    }
                }
                DataKind::TickDataAsk if workflow == TickWorkflowStep::CheckingAskForUpdate => {
                    ds.update_history_message = Some(format!("{}\nFetching new bid ticks...", last_record));
                    ds.tick_workflow = TickWorkflowStep::UpdatingBid;
                    ds.is_downloading = true;
                    ds.download_progress = 0;
                    if let Some(&sid) = symbol_map.name_to_id.get(&symbol) {
                        let _ = req_tx.try_send(DataRequest { symbol, symbol_id: sid, kind: DataKind::TickData, action: DataAction::UpdateLatest, force_rebuild: false });
                    }
                }
                _ => {
                    let kind_label = match kind { DataKind::M1Candles => "M1 candles", DataKind::TickData => "bid ticks", DataKind::TickDataAsk => "ask ticks" };
                    let info = format!("{} {} in DB\n{}\n{}", count, kind_label, first_record, last_record);
                    if kind == DataKind::TickData || kind == DataKind::TickDataAsk { ds.tick_info = Some(info); }
                    ds.download_message = Some(format!("{} {}: {} found.", symbol, kind_label, count));
                    ds.is_downloading = false;
                }
            }
        }
        DataResponse::StatusEmpty { symbol, kind } => {
            match kind {
                DataKind::M1Candles if is_cross_pair(&symbol) => {
                    ds.cross_pair_status.insert(symbol.clone(), "Downloading...".to_string());
                    if let Some(&sid) = symbol_map.name_to_id.get(&symbol) {
                        let _ = req_tx.try_send(DataRequest { symbol, symbol_id: sid, kind, action: DataAction::RetrieveFull, force_rebuild: false });
                    }
                }
                DataKind::M1Candles => {
                    ds.download_message = Some(format!("{} M1 candles: nothing in DB. Starting full download...", symbol));
                    if let Some(&sid) = symbol_map.name_to_id.get(&symbol) {
                        let _ = req_tx.try_send(DataRequest { symbol, symbol_id: sid, kind, action: DataAction::RetrieveFull, force_rebuild: false });
                        ds.is_downloading = true;
                        ds.download_progress = 0;
                    }
                }
                DataKind::TickData if workflow == TickWorkflowStep::CheckingBid => {
                    ds.download_message = Some(format!("No bid ticks found for {}. Starting bid download...", symbol));
                    ds.tick_workflow = TickWorkflowStep::DownloadingBid;
                    ds.is_downloading = true;
                    ds.download_progress = 0;
                    if let Some(&sid) = symbol_map.name_to_id.get(&symbol) {
                        let _ = req_tx.try_send(DataRequest { symbol, symbol_id: sid, kind: DataKind::TickData, action: DataAction::RetrieveFull, force_rebuild: false });
                    }
                }
                DataKind::TickDataAsk if workflow == TickWorkflowStep::CheckingAsk => {
                    ds.download_message = Some(format!("No ask ticks found for {}. Starting ask download...", symbol));
                    ds.tick_workflow = TickWorkflowStep::DownloadingAsk;
                    ds.is_downloading = true;
                    ds.download_progress = 0;
                    if let Some(&sid) = symbol_map.name_to_id.get(&symbol) {
                        let _ = req_tx.try_send(DataRequest { symbol, symbol_id: sid, kind: DataKind::TickDataAsk, action: DataAction::RetrieveFull, force_rebuild: false });
                    }
                }
                DataKind::TickData if workflow == TickWorkflowStep::CheckingBidForUpdate => {
                    ds.update_history_message = Some(format!("No bid ticks in DB for {}. Download from History BoT first.", symbol));
                    ds.tick_workflow = TickWorkflowStep::Idle;
                    ds.is_update_mode = false;
                }
                DataKind::TickDataAsk if workflow == TickWorkflowStep::CheckingAskForUpdate => {
                    ds.update_history_message = Some(format!("No ask ticks in DB for {}. Download from History BoT first.", symbol));
                    ds.tick_workflow = TickWorkflowStep::Idle;
                    ds.is_update_mode = false;
                }
                _ => {
                    let kind_label = match kind { DataKind::M1Candles => "M1 candles", DataKind::TickData => "bid ticks", DataKind::TickDataAsk => "ask ticks" };
                    ds.download_message = Some(format!("{} {}: nothing in DB.", symbol, kind_label));
                }
            }
        }
        DataResponse::Progress { symbol, downloaded_rows, message, .. } => {
            ds.download_progress = downloaded_rows;
            if ds.updating_cross_pairs.contains(&symbol) {
                ds.cross_pair_update_status.insert(symbol, message);
            } else if is_cross_pair(&symbol) {
                ds.cross_pair_status.insert(symbol, message);
            } else if ds.is_update_mode {
                ds.update_history_message = Some(message);
            } else {
                ds.download_message = Some(message);
            }
        }
        DataResponse::Complete { symbol, kind, total_rows } => {
            match kind {
                DataKind::M1Candles if ds.updating_cross_pairs.contains(&symbol) => {
                    ds.cross_pair_update_status.insert(symbol.clone(), format!("+{} new M1 bars", total_rows));
                    ds.updating_cross_pairs.remove(&symbol);
                }
                DataKind::M1Candles if is_cross_pair(&symbol) => {
                    ds.cross_pair_status.insert(symbol.clone(), format!("Done ({} rows). Loading info...", total_rows));
                    if let Some(&sid) = symbol_map.name_to_id.get(&symbol) {
                        let _ = req_tx.try_send(DataRequest { symbol, symbol_id: sid, kind: DataKind::M1Candles, action: DataAction::CheckStatus, force_rebuild: false });
                    }
                }
                DataKind::M1Candles => {
                    ds.is_downloading = false;
                    ds.download_progress = 0;
                    if ds.is_update_mode {
                        ds.pending_update_rows = total_rows;
                        ds.update_history_message = Some(format!("Update successful! +{} new M1 candles.", total_rows));
                        if let Some(&sid) = symbol_map.name_to_id.get(&symbol) {
                            let _ = req_tx.try_send(DataRequest { symbol, symbol_id: sid, kind: DataKind::M1Candles, action: DataAction::CheckStatus, force_rebuild: false });
                        }
                    } else {
                        ds.download_message = Some(format!("{} M1 candles downloaded. {} rows stored.", symbol, total_rows));
                        if let Some(&sid) = symbol_map.name_to_id.get(&symbol) {
                            let _ = req_tx.try_send(DataRequest { symbol, symbol_id: sid, kind, action: DataAction::CheckStatus, force_rebuild: false });
                        }
                    }
                }
                DataKind::TickData if workflow == TickWorkflowStep::DownloadingBid => {
                    ds.download_message = Some(format!("Bid ticks downloaded ({} rows). Checking ask ticks...", total_rows));
                    ds.is_downloading = false;
                    ds.download_progress = 0;
                    ds.tick_workflow = TickWorkflowStep::CheckingAsk;
                    if let Some(&sid) = symbol_map.name_to_id.get(&symbol) {
                        let _ = req_tx.try_send(DataRequest { symbol, symbol_id: sid, kind: DataKind::TickDataAsk, action: DataAction::CheckStatus, force_rebuild: false });
                    }
                }
                DataKind::TickDataAsk if workflow == TickWorkflowStep::DownloadingAsk => {
                    ds.download_message = Some(format!("Ask ticks downloaded ({} rows). Merging bid + ask...", total_rows));
                    ds.tick_workflow = TickWorkflowStep::Merging;
                    ds.is_downloading = true;
                    if let Some(&sid) = symbol_map.name_to_id.get(&symbol) {
                        let _ = req_tx.try_send(DataRequest { symbol, symbol_id: sid, kind: DataKind::TickData, action: DataAction::MergeBidAsk, force_rebuild: false });
                    }
                }
                DataKind::TickData if workflow == TickWorkflowStep::UpdatingBid => {
                    ds.update_history_message = Some(format!("Bid ticks updated (+{} rows). Updating ask ticks...", total_rows));
                    ds.download_progress = 0;
                    ds.tick_workflow = TickWorkflowStep::UpdatingAsk;
                    if let Some(&sid) = symbol_map.name_to_id.get(&symbol) {
                        let _ = req_tx.try_send(DataRequest { symbol, symbol_id: sid, kind: DataKind::TickDataAsk, action: DataAction::UpdateLatest, force_rebuild: false });
                    }
                }
                DataKind::TickDataAsk if workflow == TickWorkflowStep::UpdatingAsk => {
                    ds.update_history_message = Some(format!("Ask ticks updated (+{} rows). Rebuilding merged table...", total_rows));
                    ds.tick_workflow = TickWorkflowStep::Merging;
                    ds.is_downloading = true;
                    if let Some(&sid) = symbol_map.name_to_id.get(&symbol) {
                        let _ = req_tx.try_send(DataRequest { symbol, symbol_id: sid, kind: DataKind::TickData, action: DataAction::MergeBidAsk, force_rebuild: true });
                    }
                }
                _ => {
                    let kind_label = match kind { DataKind::M1Candles => "M1 candles", DataKind::TickData => "bid ticks", DataKind::TickDataAsk => "ask ticks" };
                    ds.download_message = Some(format!("{} {} downloaded. {} rows stored.", symbol, kind_label, total_rows));
                    ds.is_downloading = false;
                    ds.download_progress = 0;
                }
            }
        }
        DataResponse::MergeComplete { symbol, total_rows, new_rows, first_record, last_record, already_exists } => {
            let was_update = ds.is_update_mode;
            ds.tick_workflow = TickWorkflowStep::Idle;
            ds.is_downloading = false;
            ds.download_progress = 0;
            ds.is_update_mode = false;
            if was_update {
                ds.update_history_message = Some(format!("Merge complete! +{} new rows ({} total).", new_rows, total_rows));
            } else {
                ds.download_message = Some(if already_exists {
                    format!("Already merged ({} rows). Consider 'Update History' to refresh.", total_rows)
                } else {
                    format!("Merge complete! {} rows in {}_ticks_merged", total_rows, symbol.to_lowercase())
                });
            }
            ds.tick_info = Some(format!("{} merged ticks\n{}\n{}", total_rows, first_record, last_record));
        }
        DataResponse::MLFeaturesComplete { symbol, total_rows, new_rows, already_exists, first_record, last_record } => {
            let was_update = ds.is_update_mode;
            ds.is_update_mode = false;
            ds.ml_features_info = Some(format!("ML Features: {} rows\n{}\n{}", total_rows, first_record, last_record));
            if was_update {
                ds.update_history_message = Some(format!("ML features updated! +{} new rows ({} total).", new_rows, total_rows));
            } else {
                ds.download_message = Some(if already_exists {
                    format!("Already prepared for ML ({} rows).", total_rows)
                } else {
                    format!("{} ML features table built! {} rows ready for training.", symbol, total_rows)
                });
            }
        }
        DataResponse::Error { symbol, message, .. } => {
            ds.download_message = Some(format!("Error for {}: {}", symbol, message));
            ds.is_downloading = false;
            ds.tick_workflow = TickWorkflowStep::Idle;
        }
    }
}

// ============================================================================
// ML Model status/feature helpers
// ============================================================================

fn read_ml_model_status(model: MlSubCardType) -> String {
    let (metrics_path, model_path, model_name) = match model {
        MlSubCardType::Model1 => ("ml/trained/model1_metrics.json", "ml/trained/model1_technical.json", "Model 1  Technical Indicators (XGBoost)"),
        MlSubCardType::Model2 => return read_model2_status(),
        MlSubCardType::Model3 => return read_model3_status(),
        MlSubCardType::Model4 => ("ml/trained/model4_metrics.json", "ml/trained/model4_long.json", "Model 4  Economic Calendar (XGBoost)"),
        MlSubCardType::Model5 => return "Model 5 (Order Flow XGBoost) --not yet trained.".to_string(),
        MlSubCardType::Model6 => return "Model 6 (Ensemble XGBoost) --not yet trained.".to_string(),
    };

    let raw = match std::fs::read_to_string(metrics_path) {
        Ok(s) => s,
        Err(e) => return format!("Could not read {}: {}", metrics_path, e),
    };
    let v: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => return format!("Could not parse metrics JSON: {}", e),
    };

    let avg = &v["avg_metrics"];
    let cfg = &v["config"];
    let folds = v["fold_metrics"].as_array();

    let roc_auc   = avg["roc_auc"].as_f64().unwrap_or(0.0);
    let accuracy  = avg["accuracy"].as_f64().unwrap_or(0.0);
    let precision = avg["precision"].as_f64().unwrap_or(0.0);
    let log_loss  = avg["log_loss"].as_f64().unwrap_or(0.0);
    let n_feat    = v["n_features_used"].as_u64().unwrap_or(0);
    let target_p  = cfg["target_pips"].as_u64().unwrap_or(0);
    let stop_p    = cfg["stop_pips"].as_u64().unwrap_or(0);
    let horizon   = cfg["horizon_bars"].as_u64().unwrap_or(0);
    let threshold = cfg["threshold"].as_f64().unwrap_or(0.50);
    let n_folds   = folds.map(|f| f.len()).unwrap_or(0);

    let test_years: Vec<String> = folds
        .map(|f| f.iter().filter_map(|m| m["test_year"].as_u64().map(|y| y.to_string())).collect())
        .unwrap_or_default();

    let mut lines = Vec::new();
    lines.push(model_name.to_string());
    lines.push("--------------------------------------".to_string());
    lines.push(format!("Label: +{}p target / -{}p stop / {}m horizon", target_p, stop_p, horizon));
    lines.push(format!("Features used  : {}", n_feat));
    lines.push(format!("Walk-fwd folds : {} (test years: {})", n_folds, test_years.join(", ")));
    lines.push(String::new());
    lines.push("--- Avg walk-forward metrics ---".to_string());
    lines.push(format!("ROC-AUC  : {:.4}", roc_auc));
    lines.push(format!("Accuracy : {:.4}", accuracy));
    lines.push(format!("Precision: {:.4}  (at threshold {:.2})", precision, threshold));
    lines.push(format!("Log-loss : {:.4}", log_loss));

    if let Some(folds_arr) = folds {
        lines.push(String::new());
        lines.push("--- Per-fold ---".to_string());
        for fold in folds_arr {
            let year = fold["test_year"].as_u64().unwrap_or(0);
            let auc  = fold["roc_auc"].as_f64().unwrap_or(0.0);
            let prec = fold["precision"].as_f64().unwrap_or(0.0);
            let sigs = fold["signals"].as_u64().unwrap_or(0);
            lines.push(format!("  {}: AUC={:.3}  Prec={:.3}  Signals={}", year, auc, prec, sigs));
        }
    }

    if let Ok(meta) = std::fs::metadata(model_path) {
        if let Ok(modified) = meta.modified() {
            let datetime = chrono::DateTime::<chrono::Local>::from(modified);
            lines.push(String::new());
            lines.push(format!("Last trained: {}", datetime.format("%Y-%m-%d %H:%M")));
        }
    }

    lines.join("\n")
}

fn read_model2_status() -> String {
    let metrics_path = "ml/trained/model2_metrics.json";
    let model_path   = "ml/trained/model2_regime.pkl";

    let raw = match std::fs::read_to_string(metrics_path) {
        Ok(s)  => s,
        Err(_) => return "Model 2 (Regime HMM) --not yet trained.\nPress 'Update Training' to train.".to_string(),
    };
    let v: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => return format!("Could not parse model2_metrics.json: {}", e),
    };

    let n_states  = v["n_states"].as_u64().unwrap_or(0);
    let n_feat    = v["n_features"].as_u64().unwrap_or(0);
    let n_bars    = v["train_bars"].as_u64().unwrap_or(0);
    let stride    = v["stride"].as_u64().unwrap_or(1);
    let ll        = v["log_likelihood"].as_f64().unwrap_or(0.0);
    let aic       = v["aic"].as_f64().unwrap_or(0.0);
    let bic       = v["bic"].as_f64().unwrap_or(0.0);
    let n_params  = v["n_params"].as_u64().unwrap_or(0);
    let n_restart = v["n_restarts"].as_u64().unwrap_or(0);

    let range_start = v["train_range"][0].as_str().unwrap_or("?");
    let range_end   = v["train_range"][1].as_str().unwrap_or("?");
    let start_short = &range_start[..10.min(range_start.len())];
    let end_short   = &range_end[..10.min(range_end.len())];

    let mut lines = Vec::new();
    lines.push("Model 2  Regime Detection (GaussianHMM)".to_string());
    lines.push("--------------------------------------".to_string());
    lines.push(format!("States: {}  |  Features: {}  |  Restarts: {}", n_states, n_feat, n_restart));
    lines.push(format!("Train obs : {} (every {}nd bar)", n_bars, stride));
    lines.push(format!("Train range: {} -> {}", start_short, end_short));
    lines.push(String::new());
    lines.push("--- Fit quality ---".to_string());
    lines.push(format!("Log-likelihood: {:.2}", ll));
    lines.push(format!("AIC    : {:.2}", aic));
    lines.push(format!("BIC    : {:.2}", bic));
    lines.push(format!("Params : {}", n_params));

    if let Some(stats) = v["state_stats"].as_array() {
        lines.push(String::new());
        lines.push("--- Regime states ---".to_string());
        for s in stats {
            let label    = s["label"].as_str().unwrap_or("?");
            let pct      = s["pct_bars"].as_f64().unwrap_or(0.0);
            let avg_dur  = s["avg_duration_bars"].as_f64().unwrap_or(0.0);
            let ret_pips = s["mean_return_pips"].as_f64().unwrap_or(0.0);
            let vol      = s["mean_vol_20"].as_f64().unwrap_or(0.0);
            lines.push(format!("  {:<16}: {:5.1}%  dur={:.0}m  ret={:+.4}p  vol={:.6}", label, pct, avg_dur, ret_pips, vol));
        }
    }

    if let Ok(meta) = std::fs::metadata(model_path) {
        if let Ok(modified) = meta.modified() {
            let datetime = chrono::DateTime::<chrono::Local>::from(modified);
            lines.push(String::new());
            lines.push(format!("Last trained: {}", datetime.format("%Y-%m-%d %H:%M")));
        }
    }

    lines.join("\n")
}

fn read_model3_status() -> String {
    let metrics_path = "ml/trained/model3_metrics.json";
    let model_path   = "ml/trained/model3_cnn_long.pt";

    let raw = match std::fs::read_to_string(metrics_path) {
        Ok(s)  => s,
        Err(_) => return "Model 3 (Chart Patterns CNN) --not yet trained.\nPress 'Update Training' to train.".to_string(),
    };
    let v: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => return format!("Could not parse model3_metrics.json: {}", e),
    };

    let cfg         = &v["config"];
    let window_size = cfg["window_size"].as_u64().unwrap_or(60);
    let target_p    = cfg["target_pips"].as_u64().unwrap_or(0);
    let stop_p      = cfg["stop_pips"].as_u64().unwrap_or(0);
    let horizon     = cfg["horizon"].as_u64().unwrap_or(0);
    let n_folds     = cfg["n_folds"].as_u64().unwrap_or(0);

    let mut lines = Vec::new();
    lines.push("Model 3  Chart Patterns (1D CNN)".to_string());
    lines.push("--------------------------------".to_string());
    lines.push(format!("Window: {} bars  |  Label: +{}p / -{}p / {}m", window_size, target_p, stop_p, horizon));
    lines.push(format!("Walk-fwd folds: {}", n_folds));

    for direction in &["long", "short"] {
        let dir_v = &v[direction];
        if dir_v.is_null() { continue; }
        let avg     = &dir_v["avg_metrics"];
        let roc_auc = avg["roc_auc"].as_f64().unwrap_or(0.0);
        let acc     = avg["accuracy"].as_f64().unwrap_or(0.0);
        let prec    = avg["precision"].as_f64().unwrap_or(0.0);
        let ll      = avg["log_loss"].as_f64().unwrap_or(0.0);
        lines.push(String::new());
        lines.push(format!("--- {} model (avg walk-forward) ---", direction.to_uppercase()));
        lines.push(format!("ROC-AUC  : {:.4}", roc_auc));
        lines.push(format!("Accuracy : {:.4}", acc));
        lines.push(format!("Precision: {:.4}", prec));
        lines.push(format!("Log-loss : {:.4}", ll));
        if let Some(folds_arr) = dir_v["fold_metrics"].as_array() {
            lines.push("Per-fold:".to_string());
            for fold in folds_arr {
                let year = fold["test_year"].as_u64().unwrap_or(0);
                let auc  = fold["roc_auc"].as_f64().unwrap_or(0.0);
                let p    = fold["precision"].as_f64().unwrap_or(0.0);
                let sigs = fold["signals"].as_u64().unwrap_or(0);
                lines.push(format!("  {}: AUC={:.3}  Prec={:.3}  Signals={}", year, auc, p, sigs));
            }
        }
    }

    if let Ok(meta) = std::fs::metadata(model_path) {
        if let Ok(modified) = meta.modified() {
            let datetime = chrono::DateTime::<chrono::Local>::from(modified);
            lines.push(String::new());
            lines.push(format!("Last trained: {}", datetime.format("%Y-%m-%d %H:%M")));
        }
    }

    lines.join("\n")
}

fn read_ml_model_features(model: MlSubCardType) -> String {
    match model {
        MlSubCardType::Model1 => concat!(
            "~125 computed -> top 70 selected by XGBoost gain\n\n",
            "Trend (MA)        10\n  EMA 5/10/21/50/100/200, SMA 20\n  ema5/21, ema21/50, ema50/200 cross\n\n",
            "Momentum          14\n  RSI 14/5, MACD line/signal/hist\n  Stoch K/D, CCI, WilliamsR\n  ROC 10, Momentum, ADX / +DI / -DI\n\n",
            "Volatility         7\n  ATR 14 + ratio, BB width/pos\n  StdDev 20, KC width, Squeeze\n\n",
            "Price / Returns    9\n  return 1/5/15/30/60m\n  hl_range, body, upper/lower wick\n\n",
            "S/R Levels         7\n  SwingHigh/Low x3 periods, Pivot\n\n",
            "Ranges / Slopes    8\n  Range + Slope for 5/15/30/60m\n\n",
            "Candlesticks       8\n  Doji, Hammer, ShootingStar\n  Engulf x2, InsideBar, PinBar x2\n\n",
            "Time / Session     7\n  Hour sin/cos, DOW sin/cos\n  London, NewYork, Overlap\n\n",
            "Volume             4\n  VolRatio, OBV x2, MFI 14\n\n",
            "Consecutive        4\n  BullStreak, BearStreak\n  SinceSwingHigh, SinceSwingLow\n\n",
            "Tick Features      6\n  TickRatio, SpreadMean/Max/Std\n  WideRatio, SpreadCost%\n\n",
            "VWAP               3\n  dist_vwap, vwap_slope_5, above_vwap\n\n",
            "Ichimoku Cloud     5\n  dist_cloud_top, dist_cloud_bot\n  cloud_thickness, above_cloud\n  tenkan_vs_kijun\n\n",
            "Fibonacci          5\n  dist 23.6 / 38.2 / 50.0 / 61.8\n  fib_position (0=low, 1=high)\n\n",
            "Cross-Pair        38\n  GBPUSD/USDJPY/USDCHF/AUDUSD/EURJPY/XAUUSD\n  return_1m/5m/60m, RSI14, vs_EMA21, mom10\n  corr_20 (rolling correlation vs EURUSD)\n  usd_strength_5m, risk_sentiment_5m\n  eur_divergence_5m\n\n",
            "Market Regime      2\n  regime_state (Model 2 HMM 0-3)\n  regime_prob_max (confidence)"
        ).to_string(),
        MlSubCardType::Model2 => concat!(
            "8 features  (unsupervised --no labels)\n\n",
            "log_return          bar log-return\nrealized_vol_20     20-bar rolling std of log-returns\n",
            "realized_vol_5      5-bar rolling std (fast vol)\natr_ratio           ATR(14) / close\n",
            "hl_range            (high - low) / close\nspread_mean_pips    mean bid-ask spread in bar\n",
            "return_abs_20       20-bar mean of |log_return|\nvol_ratio           realized_vol_5 / realized_vol_20\n\n",
            "Model: GaussianHMM  covariance=full\nStates: 4  (Trending Up / Down / Ranging / Volatile)\n",
            "Data: all 24h bars from 2013  (no session filter)\nStride: every 3rd bar -> ~1.6M observations\n",
            "Restarts: 5  (best log-likelihood selected)"
        ).to_string(),
        MlSubCardType::Model3 => concat!(
            "Input: 60-bar OHLCV window (1 hour of M1 data)\n",
            "Channels: 5  (Open, High, Low, Close, Volume)\n",
            "Normalised per window (OHLC relative to entry close\n",
            "and window range; Volume by window mean)\n",
            "\n",
            "Architecture: 1D CNN  (~58K parameters)\n",
            "  Conv1d(5\u{2192}32) + BN + ReLU + MaxPool(2)\n",
            "  Conv1d(32\u{2192}64) + BN + ReLU + MaxPool(2)\n",
            "  Conv1d(64\u{2192}128) + BN + ReLU + MaxPool(2)\n",
            "  AdaptiveAvgPool1d(1)  [global avg pool]\n",
            "  Linear(128\u{2192}64) + ReLU + Dropout(0.3)\n",
            "  Linear(64\u{2192}1) \u{2192} sigmoid probability\n",
            "\n",
            "Labels: same as Model 1\n  +15p target / -10p stop / 120m horizon\n  Long + Short models trained separately\n\n",
            "Session filter: London (08-12) + NY (13-17) UTC\nWalk-forward: 4 yearly expanding folds\n",
            "Optimizer: Adam  lr=1e-3  ReduceLROnPlateau\nLoss: BCEWithLogitsLoss (pos_weight balanced)\n",
            "Early stopping: patience=15 epochs\nFine-tune: 10 extra epochs on full data after CV"
        ).to_string(),
        MlSubCardType::Model4 => concat!(
            "~70 computed -> top 50 selected by XGBoost gain\n\n",
            "Last Event (per vol/currency)   24\n  surprise_norm, abs_surprise, beats, hours_ago\n  x 3 vol tiers (>=1, >=2, >=3)\n  x 2 currencies (EUR, USD)\n\n",
            "Rolling Event Counts            24\n  count by vol tier x currency: 1h/4h/24h/1w\n  EUR vol>=1/2/3 + USD vol>=1/2/3\n\n",
            "Surprise Differential           12\n  EUR avg surprise: 1h/4h/24h/1w\n  USD avg surprise: 1h/4h/24h/1w\n  net (EUR-USD): 1h/4h/24h/1w\n\n",
            "Named Event Flags                8\n  is_nfp_day, is_fomc_day, is_ecb_day, is_cpi_day\n  hours_since_nfp/fomc/ecb/cpi\n\n",
            "High-Vol Proximity               3\n  bars_since_last_high_vol (capped 1440)\n  high_vol_in_last_1h (binary)\n  high_vol_in_last_4h (binary)\n\n",
            "Labels: same as Model 1\n  +15p target / -10p stop / 120m horizon\n  Long + Short models, CUDA RTX 3070\n",
            "  Session filter: London + NY only\n  Walk-forward: 4 yearly expanding folds"
        ).to_string(),
        MlSubCardType::Model5 => "Model 5 (Order Flow) - features not yet defined.".to_string(),
        MlSubCardType::Model6 => "Model 6 (Ensemble) - features not yet defined.".to_string(),
    }
}
