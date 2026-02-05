use bevy::prelude::*;
use bevy_egui::EguiPlugin;
use prost::Message;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_rustls::rustls::{ClientConfig, RootCertStore, ServerName};
use tokio_rustls::TlsConnector;

pub mod openapi {
    include!("proto/generated/_.rs");
}

pub mod news;
pub mod ai;
pub mod ui;
pub mod db;

use db::{Candle, CandleDatabase};
use ui::{AppState, UiState, ChartState, Timeframe, ui_system, BevyChartPlugin};

/// Message types for communication between async tasks and Bevy
#[derive(Debug, Clone)]
pub enum PriceUpdate {
    BtcPrice {
        bid: f64,
        ask: f64,
    },
    EurusdPrice {
        bid: f64,
        ask: f64,
    },
    ConnectionStatus(String),
}

/// Resource to hold the receiver for price updates
#[derive(Resource)]
pub struct PriceUpdateReceiver {
    pub receiver: mpsc::Receiver<PriceUpdate>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum AuthState {
    NotAuthenticated,
    AppAuthenticated,
    AccountAuthenticated,
    SymbolsRequested,
    Subscribed,
}

fn main() {
    // Load environment variables from .env file
    dotenv::dotenv().ok();

    // Create channel for price updates
    let (tx, rx) = mpsc::channel::<PriceUpdate>(100);

    // Spawn the tokio runtime in a separate thread for async tasks
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create tokio runtime");

        rt.block_on(async move {
            // Start news scraper
            news::spawn_news_scraper();

            // Run price streaming with reconnection
            let mut backoff_seconds = 1;
            loop {
                println!("Starting cTrader price stream...");
                match run_session(tx.clone()).await {
                    Ok(_) => println!("Session ended gracefully."),
                    Err(e) => {
                        println!("Session error: {}", e);
                        let _ = tx.send(PriceUpdate::ConnectionStatus(format!("Error: {}", e))).await;
                    }
                }

                // Exponential backoff
                tokio::time::sleep(Duration::from_secs(backoff_seconds)).await;
                backoff_seconds = (backoff_seconds * 2).min(30);
            }
        });
    });

    // Run Bevy app on main thread
    App::new()
        .add_plugins(DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "cTrader Rust Terminal".into(),
                    resolution: (1200., 800.).into(),
                    ..default()
                }),
                ..default()
            })
            .set(bevy::log::LogPlugin {
                level: bevy::log::Level::INFO,
                filter: "wgpu=info,bevy_render=info".to_string(),
                ..default()
            })
        )
        .add_plugins(EguiPlugin)
        .add_plugins(BevyChartPlugin)
        .init_resource::<AppState>()
        .init_resource::<UiState>()
        .init_resource::<ChartState>()
        .insert_resource(PriceUpdateReceiver { receiver: rx })
        .add_systems(Startup, (print_gpu_info, load_historical_data).chain())
        .add_systems(Update, process_price_updates)
        .add_systems(Update, process_load_more_requests)
        .add_systems(Update, ui_system)
        .run();
}

/// System to print GPU/renderer information at startup
fn print_gpu_info(
    render_adapter: Option<Res<bevy::render::renderer::RenderAdapterInfo>>,
    render_device: Option<Res<bevy::render::renderer::RenderDevice>>,
) {
    println!("\n========== GPU INFORMATION ==========");

    // Print adapter info (GPU name, vendor, backend)
    if let Some(adapter) = render_adapter {
        println!("GPU Name: {}", adapter.name);
        println!("Vendor: {:?}", adapter.vendor);
        println!("Device Type: {:?}", adapter.device_type);
        println!("Backend: {:?}", adapter.backend);
        println!("Driver: {}", adapter.driver);
        println!("Driver Info: {}", adapter.driver_info);
    } else {
        println!("Adapter Info: Not available yet");
    }

    if let Some(device) = render_device {
        let limits = device.limits();
        println!("\nDevice Limits:");
        println!("  Max Texture 2D: {}x{}", limits.max_texture_dimension_2d, limits.max_texture_dimension_2d);
        println!("  Max Buffer Size: {} MB", limits.max_buffer_size / (1024 * 1024));
        println!("  Max Compute Workgroup: {}", limits.max_compute_workgroup_size_x);
    }

    // Print environment info
    if let Ok(backend) = std::env::var("WGPU_BACKEND") {
        println!("\nWGPU_BACKEND env: {}", backend);
    }

    println!("======================================\n");
}

/// System to load historical data from DuckDB on startup
fn load_historical_data(mut app_state: ResMut<AppState>, mut chart_state: ResMut<ChartState>) {
    println!("Loading ALL historical data from DuckDB...");

    let db_path = "instruments_db/all_instruments.duckdb";

    match CandleDatabase::new(db_path) {
        Ok(db) => {
            // Load ALL EURUSD H4 data (table name: eurusd_eurusd_hour4)
            match db.get_all_candles("eurusd_eurusd_hour4") {
                Ok(candles) => {
                    let count = candles.len();
                    println!("Loaded ALL {} EURUSD H4 candles from database", count);
                    if let Some(instrument) = app_state.instruments.get_mut("EURUSD") {
                        instrument.candles.insert(Timeframe::H4, candles);
                    }
                    chart_state.set_total_candles("EURUSD", Timeframe::H4, count);
                    chart_state.mark_all_data_loaded("EURUSD", Timeframe::H4);
                }
                Err(e) => println!("Failed to load EURUSD H4 data: {}", e),
            }

            // Load ALL BTCUSD H4 data (table name: btcusd_btcusd_hour4)
            match db.get_all_candles("btcusd_btcusd_hour4") {
                Ok(candles) => {
                    let count = candles.len();
                    println!("Loaded ALL {} BTCUSD H4 candles from database", count);
                    if let Some(instrument) = app_state.instruments.get_mut("BTCUSD") {
                        instrument.candles.insert(Timeframe::H4, candles);
                    }
                    chart_state.set_total_candles("BTCUSD", Timeframe::H4, count);
                    chart_state.mark_all_data_loaded("BTCUSD", Timeframe::H4);
                }
                Err(e) => println!("Failed to load BTCUSD H4 data: {}", e),
            }

            println!("Historical data loading complete.");
        }
        Err(e) => {
            println!("Warning: Could not open database {}: {}", db_path, e);
            println!("Charts will show live data only.");
        }
    }
}

/// Get DuckDB table name for a symbol/timeframe combination
fn get_table_name(symbol: &str, timeframe: Timeframe) -> Option<&'static str> {
    match (symbol, timeframe) {
        ("EURUSD", Timeframe::H4) => Some("eurusd_eurusd_hour4"),
        ("BTCUSD", Timeframe::H4) => Some("btcusd_btcusd_hour4"),
        // Add more timeframes as they become available in the database
        _ => None,
    }
}

/// System to process load more data requests (lazy loading from DuckDB)
fn process_load_more_requests(
    mut app_state: ResMut<AppState>,
    mut chart_state: ResMut<ChartState>,
) {
    // Check if there's a pending request and we're not already loading
    if chart_state.is_loading {
        return;
    }

    let request = match chart_state.load_more_request.take() {
        Some(r) => r,
        None => return,
    };

    // Get the table name for this symbol/timeframe
    let table_name = match get_table_name(&request.symbol, request.timeframe) {
        Some(name) => name,
        None => {
            println!("No database table for {}/{:?}", request.symbol, request.timeframe);
            return;
        }
    };

    // Mark as loading
    chart_state.is_loading = true;

    let db_path = "instruments_db/all_instruments.duckdb";

    match CandleDatabase::new(db_path) {
        Ok(db) => {
            match db.get_candles_before(table_name, request.before_timestamp, request.count) {
                Ok(older_candles) => {
                    if older_candles.is_empty() {
                        // No more data available - mark as all loaded
                        println!("All data loaded for {}/{:?}", request.symbol, request.timeframe);
                        chart_state.mark_all_data_loaded(&request.symbol, request.timeframe);
                    } else {
                        println!(
                            "Loaded {} older candles for {}/{:?}",
                            older_candles.len(),
                            request.symbol,
                            request.timeframe
                        );

                        // Prepend older candles to existing data
                        if let Some(instrument) = app_state.instruments.get_mut(&request.symbol) {
                            let candles = instrument
                                .candles
                                .entry(request.timeframe)
                                .or_insert_with(Vec::new);

                            // Prepend older candles (they should be in chronological order)
                            let mut new_candles = older_candles;
                            new_candles.append(candles);
                            *candles = new_candles;

                            // Update total candles count
                            chart_state.set_total_candles(
                                &request.symbol,
                                request.timeframe,
                                candles.len(),
                            );
                        }
                    }
                }
                Err(e) => {
                    println!(
                        "Failed to load older candles for {}/{:?}: {}",
                        request.symbol, request.timeframe, e
                    );
                }
            }

            // Update total count from database
            if let Ok(total) = db.count_candles(table_name) {
                chart_state.set_total_candles(&request.symbol, request.timeframe, total as usize);
            }
        }
        Err(e) => {
            println!("Failed to open database: {}", e);
        }
    }

    chart_state.is_loading = false;
}

/// Helper to update candles for a specific timeframe with a new price tick
/// Returns the completed candle if a new period started (the previous candle is now complete)
/// Creates live candles at proper timestamps - gaps are handled by the rendering system
fn update_candles_with_tick(candles: &mut Vec<Candle>, timeframe: Timeframe, bid: f64, now: i64) -> Option<Candle> {
    let interval = timeframe.seconds();
    let bucket_start = (now / interval) * interval;

    if let Some(last_candle) = candles.last_mut() {
        if last_candle.timestamp == bucket_start {
            // Update current candle
            last_candle.close = bid;
            if bid > last_candle.high { last_candle.high = bid; }
            if bid < last_candle.low { last_candle.low = bid; }
            last_candle.volume += 1;
            None // No completed candle
        } else if bucket_start > last_candle.timestamp {
            // Check if it's the immediate next period (for saving completed candles)
            let gap = bucket_start - last_candle.timestamp;
            let is_continuous = gap <= interval * 2; // Within 2 periods = continuous

            // Always create the new candle at its proper timestamp
            // The rendering system will show gaps with empty space and separators
            candles.push(Candle::new(bucket_start, bid, bid, bid, bid, 1));

            if is_continuous {
                // Previous candle completed normally - save it
                let completed_candle = candles.get(candles.len() - 2).cloned();
                completed_candle
            } else {
                // Gap exists - don't save the old candle as "completed"
                // (it was already complete, just not saved because app wasn't running)
                None
            }
        } else {
            // bucket_start < last_candle.timestamp - ignore (out of order tick)
            None
        }
    } else {
        // First candle - create it at proper timestamp
        candles.push(Candle::new(bucket_start, bid, bid, bid, bid, 1));
        None
    }
}

/// Helper to save a completed candle to DuckDB
fn save_candle_to_db(symbol: &str, timeframe: Timeframe, candle: &Candle) {
    let table_name = match get_table_name(symbol, timeframe) {
        Some(name) => name,
        None => {
            println!("No database table configured for {}/{:?}", symbol, timeframe);
            return;
        }
    };

    let db_path = "instruments_db/all_instruments.duckdb";
    match CandleDatabase::new(db_path) {
        Ok(db) => {
            match db.insert_candle(table_name, candle) {
                Ok(_) => {
                    let dt = chrono::DateTime::from_timestamp(candle.timestamp, 0)
                        .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
                        .unwrap_or_else(|| "unknown".to_string());
                    println!(
                        "✓ Saved completed {} {:?} candle to DB: {} O:{:.5} H:{:.5} L:{:.5} C:{:.5}",
                        symbol, timeframe, dt, candle.open, candle.high, candle.low, candle.close
                    );
                }
                Err(e) => println!("Failed to save candle to DB: {}", e),
            }
        }
        Err(e) => println!("Failed to open database for saving: {}", e),
    }
}

/// System to process price updates from the async tasks
fn process_price_updates(
    mut app_state: ResMut<AppState>,
    mut receiver: ResMut<PriceUpdateReceiver>,
) {
    // Process all pending updates
    while let Ok(update) = receiver.receiver.try_recv() {
        let now = chrono::Utc::now().timestamp();

        match update {
            PriceUpdate::BtcPrice { bid, ask } => {
                if let Some(instrument) = app_state.instruments.get_mut("BTCUSD") {
                    instrument.update_price(bid, ask);

                    // Update 4-hour candles with real-time price
                    let h4_candles = instrument.candles.entry(Timeframe::H4).or_insert_with(Vec::new);
                    if let Some(completed) = update_candles_with_tick(h4_candles, Timeframe::H4, bid, now) {
                        // A candle period just completed - save it to the database
                        save_candle_to_db("BTCUSD", Timeframe::H4, &completed);
                    }
                }
            }
            PriceUpdate::EurusdPrice { bid, ask } => {
                if let Some(instrument) = app_state.instruments.get_mut("EURUSD") {
                    instrument.update_price(bid, ask);

                    // Update 4-hour candles with real-time price
                    let h4_candles = instrument.candles.entry(Timeframe::H4).or_insert_with(Vec::new);
                    if let Some(completed) = update_candles_with_tick(h4_candles, Timeframe::H4, bid, now) {
                        // A candle period just completed - save it to the database
                        save_candle_to_db("EURUSD", Timeframe::H4, &completed);
                    }
                }
            }
            PriceUpdate::ConnectionStatus(status) => {
                app_state.connection_status = status;
            }
        }
    }
}

async fn run_session(tx: mpsc::Sender<PriceUpdate>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Load credentials from environment variables
    let app_client_id = std::env::var("CTRADER_CLIENT_ID")
        .expect("CTRADER_CLIENT_ID must be set in .env file");
    let app_secret = std::env::var("CTRADER_SECRET")
        .expect("CTRADER_SECRET must be set in .env file");
    let access_token = std::env::var("CTRADER_ACCESS_TOKEN")
        .expect("CTRADER_ACCESS_TOKEN must be set in .env file");
    let account_id: i64 = std::env::var("CTRADER_ACCOUNT_ID")
        .expect("CTRADER_ACCOUNT_ID must be set in .env file")
        .parse()
        .expect("CTRADER_ACCOUNT_ID must be a valid number");
    let target_symbol = std::env::var("CTRADER_SYMBOL")
        .unwrap_or_else(|_| "BTCUSD".to_string());

    // Debug: Show loaded config (masked for security)
    println!("✓ Loaded config: Client ID: {}..., Account ID: {}, Symbol: {}",
             &app_client_id.chars().take(10).collect::<String>(),
             account_id,
             target_symbol);

    let host = "live.ctraderapi.com";
    let port = 5035;

    // 1. Setup SSL/TLS
    let mut root_cert_store = RootCertStore::empty();
    root_cert_store.add_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.iter().map(|ta| {
        tokio_rustls::rustls::OwnedTrustAnchor::from_subject_spki_name_constraints(
            ta.subject, ta.spki, ta.name_constraints,
        )
    }));
    let config = ClientConfig::builder()
        .with_safe_defaults()
        .with_root_certificates(root_cert_store)
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(config));

    // 2. Connect
    let stream = TcpStream::connect(format!("{}:{}", host, port)).await?;
    let domain = ServerName::try_from(host)?;
    let mut tls_stream = connector.connect(domain, stream).await?;
    println!("Connected to IC Markets Live via Rust...");
    tx.send(PriceUpdate::ConnectionStatus("Connected".to_string())).await?;

    // 3. Send App Auth
    let app_auth = openapi::ProtoOaApplicationAuthReq {
        payload_type: Some(openapi::ProtoOaPayloadType::ProtoOaApplicationAuthReq as i32),
        client_id: app_client_id.clone(),
        client_secret: app_secret.clone(),
    };
    send_message(&mut tls_stream, openapi::ProtoOaPayloadType::ProtoOaApplicationAuthReq as u32, app_auth).await?;
    println!("App auth sent");

    // 4. Message Loop (Reading Prices)
    let mut _auth_state = AuthState::NotAuthenticated;
    let mut btc_symbol_id: Option<i64> = None;
    let mut eurusd_symbol_id: Option<i64> = None;

    let mut last_heartbeat = tokio::time::Instant::now();
    let mut heartbeat_interval = tokio::time::interval(Duration::from_secs(30));
    heartbeat_interval.tick().await; // skip first immediate tick

    loop {
        let mut header = [0u8; 4];
        tokio::select! {
            header_read = timeout(Duration::from_secs(30), tls_stream.read_exact(&mut header)) => {
                match header_read {
                    Ok(Ok(_)) => {},
                    Ok(Err(e)) => {
                        println!("Error reading header: {}", e);
                        break;
                    },
                    Err(_) => {
                        println!("Timeout reading header - connection may be dead");
                        break;
                    }
                }
                let len = u32::from_be_bytes(header) as usize;
                let mut buf = vec![0u8; len];
                let payload_read = timeout(Duration::from_secs(30), tls_stream.read_exact(&mut buf)).await;
                match payload_read {
                    Ok(Ok(_)) => {},
                    Ok(Err(e)) => {
                        println!("Error reading payload: {}", e);
                        break;
                    },
                    Err(_) => {
                        println!("Timeout reading payload - connection may be dead");
                        break;
                    }
                }
                let msg = match openapi::ProtoMessage::decode(&buf[..]) {
                    Ok(m) => m,
                    Err(e) => {
                        println!("Failed to decode message: {}", e);
                        continue;
                    }
                };
                println!("DEBUG recv: payload_type={}, payload_len={}", msg.payload_type, msg.payload.as_ref().map(|p| p.len()).unwrap_or(0));

                match msg.payload_type {
                    2101 => { // ProtoOAApplicationAuthRes
                        println!("App auth successful");
                        _auth_state = AuthState::AppAuthenticated;
                        let account_auth = openapi::ProtoOaAccountAuthReq {
                            payload_type: Some(openapi::ProtoOaPayloadType::ProtoOaAccountAuthReq as i32),
                            ctid_trader_account_id: account_id,
                            access_token: access_token.clone(),
                        };
                        send_message(&mut tls_stream, openapi::ProtoOaPayloadType::ProtoOaAccountAuthReq as u32, account_auth).await?;
                        println!("Account auth sent");
                    },
                    2103 => { // ProtoOAAccountAuthRes
                        println!("Account auth successful");
                        _auth_state = AuthState::AccountAuthenticated;
                        let symbols_req = openapi::ProtoOaSymbolsListReq {
                            payload_type: Some(openapi::ProtoOaPayloadType::ProtoOaSymbolsListReq as i32),
                            ctid_trader_account_id: account_id,
                            include_archived_symbols: None,
                        };
                        send_message(&mut tls_stream, openapi::ProtoOaPayloadType::ProtoOaSymbolsListReq as u32, symbols_req).await?;
                        println!("Symbols list requested");
                        _auth_state = AuthState::SymbolsRequested;
                    },
                    2115 => { // ProtoOASymbolsListRes
                        if let Some(payload) = &msg.payload {
                            let res = openapi::ProtoOaSymbolsListRes::decode(payload.as_slice())?;

                            for symbol in res.symbol {
                                if let Some(ref symbol_name) = symbol.symbol_name {
                                    if symbol_name == "BTCUSD" || symbol_name == &target_symbol {
                                        btc_symbol_id = Some(symbol.symbol_id);
                                        println!("Found {} (ID: {})", symbol_name, symbol.symbol_id);
                                    } else if symbol_name == "EURUSD" {
                                        eurusd_symbol_id = Some(symbol.symbol_id);
                                        println!("Found {} (ID: {})", symbol_name, symbol.symbol_id);
                                    }
                                }
                            }

                            let mut symbol_ids = Vec::new();
                            if let Some(id) = btc_symbol_id {
                                symbol_ids.push(id);
                            }
                            if let Some(id) = eurusd_symbol_id {
                                symbol_ids.push(id);
                            }

                            if !symbol_ids.is_empty() {
                                println!("Subscribing to {} symbols...", symbol_ids.len());
                                let subscribe = openapi::ProtoOaSubscribeSpotsReq {
                                    payload_type: Some(openapi::ProtoOaPayloadType::ProtoOaSubscribeSpotsReq as i32),
                                    ctid_trader_account_id: account_id,
                                    symbol_id: symbol_ids,
                                    subscribe_to_spot_timestamp: Some(true),
                                };
                                send_message(&mut tls_stream, openapi::ProtoOaPayloadType::ProtoOaSubscribeSpotsReq as u32, subscribe).await?;
                                println!("Subscribe sent for all symbols");
                                _auth_state = AuthState::Subscribed;
                            } else {
                                println!("Error: No symbols found in account symbol list.");
                                break;
                            }
                        }
                    },
                    2131 => { // ProtoOASpotEvent
                        if let Some(payload) = &msg.payload {
                            let event = openapi::ProtoOaSpotEvent::decode(payload.as_slice())?;
                            let symbol_id = event.symbol_id;

                            let bid = event.bid.unwrap_or(0) as f64 / 100_000.0;
                            let ask = event.ask.unwrap_or(0) as f64 / 100_000.0;

                            if Some(symbol_id) == btc_symbol_id {
                                if bid > 0.0 && ask > 0.0 {
                                    println!("LIVE BTCUSD | Bid: {:.2} | Ask: {:.2}", bid, ask);
                                    tx.send(PriceUpdate::BtcPrice { bid, ask }).await?;
                                }
                            } else if Some(symbol_id) == eurusd_symbol_id {
                                if bid > 0.0 && ask > 0.0 {
                                    println!("LIVE EURUSD | Bid: {:.5} | Ask: {:.5}", bid, ask);
                                    tx.send(PriceUpdate::EurusdPrice { bid, ask }).await?;
                                }
                            }
                        }
                    },
                    2142 => { // ProtoOAErrorRes
                        if let Some(payload) = &msg.payload {
                            let err = openapi::ProtoOaErrorRes::decode(payload.as_slice())?;
                            println!("ERROR: {} - {}", err.error_code, err.description.unwrap_or_default());
                            if err.error_code == "CH_CLIENT_AUTH_FAILURE" || err.error_code == "ACCOUNT_NOT_AUTHORIZED" {
                                break;
                            }
                            if err.error_code == "BLOCKED_PAYLOAD_TYPE" {
                                let retry_after = err.retry_after.unwrap_or(5);
                                if retry_after > 300 {
                                    println!("Rate limit too long ({} seconds). Treating as fatal error.", retry_after);
                                    break;
                                }
                                println!("Rate limited. Waiting {} seconds before retrying...", retry_after);
                                tokio::time::sleep(Duration::from_secs(retry_after)).await;
                                if _auth_state == AuthState::AppAuthenticated {
                                    let account_auth = openapi::ProtoOaAccountAuthReq {
                                        payload_type: Some(openapi::ProtoOaPayloadType::ProtoOaAccountAuthReq as i32),
                                        ctid_trader_account_id: account_id,
                                        access_token: access_token.clone(),
                                    };
                                    send_message(&mut tls_stream, openapi::ProtoOaPayloadType::ProtoOaAccountAuthReq as u32, account_auth).await?;
                                    println!("Account auth resent after rate limit");
                                }
                            }
                        }
                    },
                    2147 => { // ProtoOAAccountsTokenInvalidatedEvent
                        println!("Account token invalidated. Reconnecting...");
                        break;
                    },
                    2148 => { // ProtoOAClientDisconnectEvent
                        println!("Client disconnected by server. Reconnecting...");
                        break;
                    },
                    51 => {
                        println!("Heartbeat received");
                        last_heartbeat = tokio::time::Instant::now();
                        if let Err(e) = send_heartbeat(&mut tls_stream).await {
                            println!("Failed to send heartbeat: {}", e);
                        }
                    },
                    _ => println!("Received payload type: {}", msg.payload_type),
                }
            }
            _ = heartbeat_interval.tick() => {
                if let Err(e) = send_heartbeat(&mut tls_stream).await {
                    println!("Failed to send periodic heartbeat: {}", e);
                    break;
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(60)) => {
                if last_heartbeat.elapsed() > Duration::from_secs(60) {
                    println!("No heartbeat from server for 60 seconds, reconnecting...");
                    break;
                }
            }
        }
    }

    Ok(())
}

async fn send_message<T: Message>(
    stream: &mut tokio_rustls::client::TlsStream<TcpStream>,
    payload_type: u32,
    payload: T,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut body = Vec::new();
    payload.encode(&mut body)?;
    println!("DEBUG send: payload_type={}, body_len={}", payload_type, body.len());

    let proto_msg = openapi::ProtoMessage {
        payload_type,
        payload: Some(body),
        client_msg_id: Some("1".to_string()),
    };

    let mut full_msg = Vec::new();
    proto_msg.encode(&mut full_msg)?;
    println!("DEBUG send: full_msg_len={}", full_msg.len());

    let len = (full_msg.len() as u32).to_be_bytes();
    stream.write_all(&len).await?;
    stream.write_all(&full_msg).await?;
    Ok(())
}

async fn send_heartbeat(stream: &mut tokio_rustls::client::TlsStream<TcpStream>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let proto_msg = openapi::ProtoMessage {
        payload_type: 51,
        payload: None,
        client_msg_id: Some("heartbeat".to_string()),
    };
    let mut full_msg = Vec::new();
    proto_msg.encode(&mut full_msg)?;
    let len = (full_msg.len() as u32).to_be_bytes();
    stream.write_all(&len).await?;
    stream.write_all(&full_msg).await?;
    println!("Heartbeat sent");
    Ok(())
}
