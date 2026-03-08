use bevy::prelude::*;
use prost::Message;
use std::io::Write;
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
pub mod data_retrieval;

use db::{Candle, CandleDatabase};
use data_retrieval::{
    DataKind, DataAction, DataRequest, DataResponse,
    DataRequestSender, DataResponseReceiver, SymbolIdMap,
    trendbar_to_candle, decode_tick_data, get_data_table_name,
};
use ui::{AppState, UiState, BevyUiPlugin};

/// Message types for communication between async tasks and Bevy
#[derive(Debug, Clone)]
pub enum PriceUpdate {
    /// Generic price update for any instrument
    InstrumentPrice {
        symbol: String,
        bid: f64,
        ask: f64,
    },
    ConnectionStatus(String),
    /// Symbol name → cTrader symbol_id mapping (sent once after auth)
    SymbolMapping(std::collections::HashMap<String, i64>),
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

    // Create channel for price updates (network -> Bevy)
    let (tx, rx) = mpsc::channel::<PriceUpdate>(100);

    // Create channels for data retrieval (Bevy <-> network)
    let (data_req_tx, data_req_rx) = mpsc::channel::<DataRequest>(32);
    let (data_resp_tx, data_resp_rx) = mpsc::channel::<DataResponse>(64);

    // Spawn the tokio runtime in a separate thread for async tasks
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create tokio runtime");

        rt.block_on(async move {
            // News scraper disabled (no UI integration)

            // Run price streaming with reconnection
            // request_rx passed by &mut so pending requests survive reconnects
            let mut request_rx = data_req_rx;
            let mut backoff_seconds = 1;
            loop {
                println!("Starting cTrader price stream...");
                match run_session(tx.clone(), &mut request_rx, data_resp_tx.clone()).await {
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
        .add_plugins(BevyUiPlugin)
        .init_resource::<AppState>()
        .init_resource::<UiState>()
        .init_resource::<SymbolIdMap>()
        .insert_resource(PriceUpdateReceiver { receiver: rx })
        .insert_resource(DataRequestSender { sender: data_req_tx })
        .insert_resource(DataResponseReceiver { receiver: data_resp_rx })
        .add_systems(Startup, print_gpu_info)
        .add_systems(Update, process_price_updates)
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

/// System to process price updates from the async tasks
fn process_price_updates(
    mut app_state: ResMut<AppState>,
    mut receiver: ResMut<PriceUpdateReceiver>,
    mut symbol_map: ResMut<SymbolIdMap>,
) {
    // Process all pending updates
    while let Ok(update) = receiver.receiver.try_recv() {
        match update {
            PriceUpdate::InstrumentPrice { symbol, bid, ask } => {
                if let Some(instrument) = app_state.instruments.get_mut(&symbol) {
                    instrument.update_price(bid, ask);
                }
            }
            PriceUpdate::ConnectionStatus(status) => {
                app_state.connection_status = status;
            }
            PriceUpdate::SymbolMapping(mapping) => {
                println!("Received symbol mapping: {} symbols", mapping.len());
                symbol_map.name_to_id = mapping;
            }
        }
    }
}

async fn run_session(
    tx: mpsc::Sender<PriceUpdate>,
    request_rx: &mut mpsc::Receiver<DataRequest>,
    response_tx: mpsc::Sender<DataResponse>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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

    // Map symbol_id -> symbol_name for all subscribed instruments
    let mut symbol_id_to_name: std::collections::HashMap<i64, String> = std::collections::HashMap::new();

    // Symbols to subscribe to live spot prices
    let instruments_to_subscribe: Vec<&str> = vec!["EURUSD"];
    // All symbols whose IDs we need (cross-pairs for M1 data downloads, ID lookup only)
    let instruments_need_id: Vec<&str> = vec![
        "EURUSD",
        "GBPUSD", "USDJPY", "USDCHF", "AUDUSD", "EURJPY",
    ];

    let mut last_heartbeat = tokio::time::Instant::now();
    let mut heartbeat_interval = tokio::time::interval(Duration::from_secs(30));
    heartbeat_interval.tick().await; // skip first immediate tick

    // Active download state for historical data retrieval
    let mut active_download: Option<ActiveDownload> = None;

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

                            // Build mapping from symbol_id to symbol_name.
                            // Capture IDs for all needed symbols; only subscribe to spot prices for EURUSD.
                            let mut spot_subscribe_ids = Vec::new();
                            for symbol in res.symbol {
                                if let Some(ref symbol_name) = symbol.symbol_name {
                                    if instruments_need_id.contains(&symbol_name.as_str()) {
                                        symbol_id_to_name.insert(symbol.symbol_id, symbol_name.clone());
                                        println!("Found {} (ID: {})", symbol_name, symbol.symbol_id);
                                        if instruments_to_subscribe.contains(&symbol_name.as_str()) {
                                            spot_subscribe_ids.push(symbol.symbol_id);
                                        }
                                    }
                                }
                            }

                            if !spot_subscribe_ids.is_empty() {
                                println!("Subscribing to {} spot symbol(s)...", spot_subscribe_ids.len());
                                let subscribe = openapi::ProtoOaSubscribeSpotsReq {
                                    payload_type: Some(openapi::ProtoOaPayloadType::ProtoOaSubscribeSpotsReq as i32),
                                    ctid_trader_account_id: account_id,
                                    symbol_id: spot_subscribe_ids,
                                    subscribe_to_spot_timestamp: Some(true),
                                };
                                send_message(&mut tls_stream, openapi::ProtoOaPayloadType::ProtoOaSubscribeSpotsReq as u32, subscribe).await?;
                                println!("Subscribe sent. ID map has {} symbols", symbol_id_to_name.len());
                                _auth_state = AuthState::Subscribed;

                                // Send reverse mapping (name -> id) to Bevy for data retrieval
                                let reverse_map: std::collections::HashMap<String, i64> = symbol_id_to_name
                                    .iter()
                                    .map(|(&id, name)| (name.clone(), id))
                                    .collect();
                                let _ = tx.send(PriceUpdate::SymbolMapping(reverse_map)).await;
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

                            // Look up the symbol name from our mapping
                            if let Some(symbol_name) = symbol_id_to_name.get(&symbol_id) {
                                if bid > 0.0 && ask > 0.0 {
                                    // Use appropriate decimal places for display
                                    let decimals = match symbol_name.as_str() {
                                        "BTCUSD" | "ETHUSD" | "BCHUSD" | "LTCUSD" | "XAUUSD" |
                                        "XPDUSD" | "XPTUSD" | "XAUAUD" | "CHINA50" | "SPXUSD" |
                                        "AAVEUSD" | "ENSUSD" | "ETCUSD" | "ICPUSD" | "INJUSD" | "TAOUSD" |
                                        "AVXUSD" | "KSMUSD" | "UNIUSD" | "BNBUSD" | "LNKUSD" |
                                        "SOLUSD" | "XMRUSD" | "ZECUSD" | "QNTUSD" | "COMPUSD" => 2,
                                        "ATOMUSD" | "FILUSD" | "XTIUSD" | "JTOUSD" | "LDOUSD" |
                                        "NEARUSD" | "OPUSD" | "RENDERUSD" | "TIAUSD" | "TONUSD" | "TRUMPUSD" |
                                        "WLDUSD" | "DOTUSD" | "CAKEUSD" | "PENDLEUSD" | "DYDXUSD" => 3,
                                        "AEROUSD" | "ALGOUSD" | "APTUSD" | "ARBUSD" | "AUSD" |
                                        "CFXUSD" | "CRVUSD" | "FLOWUSD" | "XNGUSD" | "GRTUSD" |
                                        "HYPEUSD" | "IMXUSD" | "IOTAUSD" | "IPUSD" | "JUPUSD" |
                                        "MANAUSD" | "MORPHOUSD" | "ONDOUSD" | "PYTHUSD" | "SANDUSD" |
                                        "STXUSD" | "SUIUSD" | "SUSD" | "SYRUPUSD" | "THETAUSD" |
                                        "VIRTUALUSD" | "WIFUSD" | "ADAUSD" | "XRPUSD" | "XTZUSD" |
                                        "POLUSD" | "XLMUSD" | "GLMUSD" | "KAIAUSD" | "SEIUSD" |
                                        "MUSD" | "ENAUSD" | "FETUSD" | "DEXEUSD" |
                                        "XPLUSD" | "STRKUSD" | "WLFIUSD" | "ASTERUSD" |
                                        "TWTUSD" | "COAIUSD" | "MYXUSD" | "2ZUSD" | "1INCHUSD" => 4,
                                        "GALAUSD" | "EURUSD" | "AUDUSD" | "GBPUSD" | "USDCHF" | "EURGBP" |
                                        "EURAUD" | "HBARUSD" | "PENGUUSD" | "DOGUSD" | "VETUSD" | "TRXUSD" |
                                        "1000xSHIB" | "1000xPEPE" | "1000xBONK" | "1000xFLOKI" => 5,
                                        "FARTCOINUSD" => 6,
                                        _ => 5, // Default to 5 decimals
                                    };
                                    println!("LIVE {} | Bid: {:.prec$} | Ask: {:.prec$}", symbol_name, bid, ask, prec = decimals);
                                    tx.send(PriceUpdate::InstrumentPrice {
                                        symbol: symbol_name.clone(),
                                        bid,
                                        ask,
                                    }).await?;
                                }
                            }
                        }
                    },
                    2138 => { // ProtoOAGetTrendbarsRes
                        if let Some(payload) = &msg.payload {
                            let res = openapi::ProtoOaGetTrendbarsRes::decode(payload.as_slice())?;
                            handle_trendbars_response(
                                &mut tls_stream,
                                &res,
                                account_id,
                                &response_tx,
                                &mut active_download,
                            ).await?;
                        }
                    },
                    2146 => { // ProtoOAGetTickDataRes
                        if let Some(payload) = &msg.payload {
                            let res = openapi::ProtoOaGetTickDataRes::decode(payload.as_slice())?;
                            handle_tick_data_response(
                                &mut tls_stream,
                                &res,
                                account_id,
                                &response_tx,
                                &mut active_download,
                            ).await?;
                        }
                    },
                    2142 => { // ProtoOAErrorRes
                        if let Some(payload) = &msg.payload {
                            let err = openapi::ProtoOaErrorRes::decode(payload.as_slice())?;
                            let desc = err.description.clone().unwrap_or_default();
                            println!("ERROR: {} - {}", err.error_code, desc);

                            if err.error_code == "CH_CLIENT_AUTH_FAILURE" || err.error_code == "ACCOUNT_NOT_AUTHORIZED" {
                                // Notify UI if download was active
                                if let Some(dl) = active_download.take() {
                                    let _ = response_tx.send(DataResponse::Error {
                                        symbol: dl.symbol,
                                        kind: dl.kind,
                                        message: format!("{} - {}", err.error_code, desc),
                                    }).await;
                                }
                                break;
                            }
                            if err.error_code == "BLOCKED_PAYLOAD_TYPE" {
                                let retry_after = err.retry_after.unwrap_or(5);
                                if retry_after > 300 {
                                    println!("Rate limit too long ({} seconds). Treating as fatal error.", retry_after);
                                    if let Some(dl) = active_download.take() {
                                        let _ = response_tx.send(DataResponse::Error {
                                            symbol: dl.symbol,
                                            kind: dl.kind,
                                            message: format!("Rate limited for {} seconds, aborting.", retry_after),
                                        }).await;
                                    }
                                    break;
                                }
                                println!("Rate limited. Waiting {} seconds before retrying...", retry_after);
                                // Notify UI about the wait
                                if let Some(ref dl) = active_download {
                                    let _ = response_tx.send(DataResponse::Progress {
                                        symbol: dl.symbol.clone(),
                                        kind: dl.kind,
                                        downloaded_rows: dl.total_downloaded,
                                        message: format!("Rate limited, waiting {} seconds...", retry_after),
                                    }).await;
                                }
                                tokio::time::sleep(Duration::from_secs(retry_after as u64)).await;
                                // Resend the last API request after rate limit
                                if let Some(ref dl) = active_download {
                                    let now_ms = chrono::Utc::now().timestamp_millis();
                                    let dummy_req = DataRequest {
                                        symbol: dl.symbol.clone(),
                                        symbol_id: dl.symbol_id,
                                        kind: dl.kind,
                                        action: DataAction::RetrieveFull,
                                        force_rebuild: false,
                                    };
                                    send_first_api_request(&mut tls_stream, &dummy_req, account_id, dl.from_timestamp_ms, now_ms).await?;
                                    println!("Resent download request after rate limit");
                                }
                            } else {
                                // Any other API error during active download → notify UI
                                if let Some(dl) = active_download.take() {
                                    let _ = response_tx.send(DataResponse::Error {
                                        symbol: dl.symbol,
                                        kind: dl.kind,
                                        message: format!("{} - {}", err.error_code, desc),
                                    }).await;
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
            // Receive data requests from Bevy UI
            Some(request) = request_rx.recv() => {
                println!("DEBUG: Received data request: {} {:?} {:?}", request.symbol, request.kind, request.action);
                handle_data_request(
                    &mut tls_stream,
                    &request,
                    account_id,
                    &response_tx,
                    &mut active_download,
                ).await?;
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

// ============================================================================
// Historical Data Retrieval
// ============================================================================

/// Tracks an ongoing download operation
struct ActiveDownload {
    symbol: String,
    symbol_id: i64,
    kind: DataKind,
    total_downloaded: u64,
    /// For pagination: the original from_timestamp (ms)
    from_timestamp_ms: i64,
    /// The initial to_timestamp (ms) — used for progress percentage
    to_timestamp_ms: i64,
    /// DuckDB table name
    table_name: String,
    /// Path to temp CSV file for accumulating data before bulk load
    csv_path: String,
}

/// Handle a data request from the UI
async fn handle_data_request(
    stream: &mut tokio_rustls::client::TlsStream<TcpStream>,
    request: &DataRequest,
    account_id: i64,
    response_tx: &mpsc::Sender<DataResponse>,
    active_download: &mut Option<ActiveDownload>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let table_name = get_data_table_name(&request.symbol, request.kind);

    match request.action {
        DataAction::CheckStatus => {
            check_db_status(request, &table_name, response_tx).await;
        }
        DataAction::RetrieveFull => {
            println!("Starting full retrieval: {} {:?}", request.symbol, request.kind);
            // Fetch ALL available history (from Jan 1, 2010)
            let now_ms = chrono::Utc::now().timestamp_millis();
            let from_ms = 1_262_304_000_000_i64; // 2010-01-01 00:00:00 UTC in ms

            // Create temp CSV file with header for accumulating data
            let csv_suffix = match request.kind {
                DataKind::M1Candles => "m1",
                DataKind::TickData => "ticks_bid",
                DataKind::TickDataAsk => "ticks_ask",
            };
            let csv_path = format!("Bots_db/temp_{}_{}.csv",
                request.symbol.to_lowercase(), csv_suffix);
            {
                let header = match request.kind {
                    DataKind::M1Candles => "timestamp,open,high,low,close,volume",
                    DataKind::TickData => "timestamp_ms,bid",
                    DataKind::TickDataAsk => "timestamp_ms,ask",
                };
                let mut f = std::fs::File::create(&csv_path)?;
                writeln!(f, "{}", header)?;
            }

            *active_download = Some(ActiveDownload {
                symbol: request.symbol.clone(),
                symbol_id: request.symbol_id,
                kind: request.kind,
                total_downloaded: 0,
                from_timestamp_ms: from_ms,
                to_timestamp_ms: now_ms,
                table_name: table_name.clone(),
                csv_path,
            });

            send_first_api_request(stream, request, account_id, from_ms, now_ms).await?;
        }
        DataAction::UpdateLatest => {
            println!("Starting update: {} {:?}", request.symbol, request.kind);
            let now_ms = chrono::Utc::now().timestamp_millis();

            // Check what we have in DB, fetch from the newest timestamp onward
            let from_ms = get_newest_timestamp_in_db(&table_name, request.kind);

            // Create temp CSV file with header for accumulating data
            let csv_suffix = match request.kind {
                DataKind::M1Candles => "m1",
                DataKind::TickData => "ticks_bid",
                DataKind::TickDataAsk => "ticks_ask",
            };
            let csv_path = format!("Bots_db/temp_{}_{}.csv",
                request.symbol.to_lowercase(), csv_suffix);
            {
                let header = match request.kind {
                    DataKind::M1Candles => "timestamp,open,high,low,close,volume",
                    DataKind::TickData => "timestamp_ms,bid",
                    DataKind::TickDataAsk => "timestamp_ms,ask",
                };
                let mut f = std::fs::File::create(&csv_path)?;
                writeln!(f, "{}", header)?;
            }

            *active_download = Some(ActiveDownload {
                symbol: request.symbol.clone(),
                symbol_id: request.symbol_id,
                kind: request.kind,
                total_downloaded: 0,
                from_timestamp_ms: from_ms,
                to_timestamp_ms: now_ms,
                table_name: table_name.clone(),
                csv_path,
            });

            send_first_api_request(stream, request, account_id, from_ms, now_ms).await?;
        }
        DataAction::MergeBidAsk => {
            println!("Checking merged table for {}", request.symbol);
            let symbol = request.symbol.clone();
            let bid_table = get_data_table_name(&symbol, DataKind::TickData);
            let ask_table = get_data_table_name(&symbol, DataKind::TickDataAsk);
            let merged_table = format!("{}_ticks_merged", symbol.to_lowercase());

            let force_rebuild = request.force_rebuild;
            let merge_result = tokio::task::spawn_blocking(move || {
                let db_path = "Bots_db/Algo_EURUSD.duckdb";
                let db = CandleDatabase::new(db_path)?;

                let already_exists = db.check_ml_features_table(&merged_table).is_some();

                let incremental_new_rows: i64;
                if already_exists && force_rebuild {
                    // Update History path: table exists, only add the new ticks (fast incremental)
                    println!("[MERGE] Incremental merge: appending new ticks to existing {} rows...", merged_table);
                    let nr = db.incremental_merge_bid_ask_ticks(&bid_table, &ask_table, &merged_table)?;
                    println!("[MERGE] Incremental merge complete. +{} new rows.", nr);
                    incremental_new_rows = nr;
                } else if !already_exists {
                    // First-time build: full merge (slow, but only done once)
                    println!("[MERGE] Full merge (first build)...");
                    db.merge_bid_ask_ticks(&bid_table, &ask_table, &merged_table)?;
                    println!("[MERGE] Full merge complete.");
                    incremental_new_rows = 0;
                } else {
                    println!("[MERGE] Merged table already exists — skipping re-merge.");
                    incremental_new_rows = 0;
                }

                let total = db.count_merged_ticks(&merged_table)?;
                let first = db.get_first_merged_tick(&merged_table)?;
                let last = db.get_last_merged_tick(&merged_table)?;
                // already_exists=true only when we skipped the merge (existed and no force)
                Ok::<(i64, i64, Option<(i64, f64, f64, f64)>, Option<(i64, f64, f64, f64)>, bool), duckdb::Error>((total, incremental_new_rows, first, last, already_exists && !force_rebuild))
            }).await;

            match merge_result {
                Ok(Ok((total, new_rows, first, last, already_exists))) => {
                    let first_record = match first {
                        Some((ts, bid, ask, spread)) => {
                            let dt = chrono::DateTime::from_timestamp_millis(ts)
                                .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
                                .unwrap_or_else(|| "?".into());
                            let spread_pips = spread * 10_000.0;
                            format!("First: {} Bid:{:.5} Ask:{:.5} Spread:{:.1} pips", dt, bid, ask, spread_pips)
                        }
                        None => "First: N/A".into(),
                    };
                    let last_record = match last {
                        Some((ts, bid, ask, spread)) => {
                            let dt = chrono::DateTime::from_timestamp_millis(ts)
                                .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
                                .unwrap_or_else(|| "?".into());
                            let spread_pips = spread * 10_000.0;
                            format!("Last: {} Bid:{:.5} Ask:{:.5} Spread:{:.1} pips", dt, bid, ask, spread_pips)
                        }
                        None => "Last: N/A".into(),
                    };
                    println!("[MERGE] Reporting {} rows (already_exists={})", total, already_exists);
                    let _ = response_tx.send(DataResponse::MergeComplete {
                        symbol: request.symbol.clone(),
                        total_rows: total as u64,
                        new_rows: new_rows as u64,
                        first_record,
                        last_record,
                        already_exists,
                    }).await;
                }
                Ok(Err(e)) => {
                    println!("[MERGE] DB error: {}", e);
                    let _ = response_tx.send(DataResponse::Error {
                        symbol: request.symbol.clone(),
                        kind: DataKind::TickData,
                        message: format!("Merge error: {}", e),
                    }).await;
                }
                Err(e) => {
                    println!("[MERGE] Task error: {}", e);
                    let _ = response_tx.send(DataResponse::Error {
                        symbol: request.symbol.clone(),
                        kind: DataKind::TickData,
                        message: format!("Merge task error: {}", e),
                    }).await;
                }
            }
        }
        DataAction::BuildMLFeatures => {
            let symbol = request.symbol.clone();
            let m1_table = get_data_table_name(&symbol, DataKind::M1Candles);
            let merged_table = format!("{}_ticks_merged", symbol.to_lowercase());
            let features_table = format!("{}_tick_features_m1", symbol.to_lowercase());

            let force_rebuild = request.force_rebuild;
            let ml_result = tokio::task::spawn_blocking(move || {
                let db_path = "Bots_db/Algo_EURUSD.duckdb";
                let db = CandleDatabase::new(db_path)?;

                let force_rebuild = force_rebuild;
                let already_exists = db.check_ml_features_table(&features_table).is_some();

                if already_exists && force_rebuild {
                    println!("[ML] Incremental ML features update...");
                    let new_rows = db.incremental_build_ml_features(&m1_table, &merged_table, &features_table)?;
                    let total = db.check_ml_features_table(&features_table).unwrap_or(0);
                    println!("[ML] Incremental update complete. +{} new rows, {} total.", new_rows, total);
                    Ok::<(i64, i64, bool), duckdb::Error>((total, new_rows, false))
                } else if !already_exists {
                    println!("[ML] Building tick features table (first build, may take a few minutes)...");
                    let count = db.build_ml_features_table(&m1_table, &merged_table, &features_table)?;
                    println!("[ML] Complete! {} rows in {}", count, features_table);
                    Ok::<(i64, i64, bool), duckdb::Error>((count, 0, false))
                } else {
                    let count = db.check_ml_features_table(&features_table).unwrap_or(0);
                    println!("[ML] Features table already exists with {} rows", count);
                    Ok::<(i64, i64, bool), duckdb::Error>((count, 0, true))
                }
            }).await;

            // Helper to format a (timestamp, tick_count, spread_mean_pips) row
            let fmt_ml_row = |label: &str, row: Option<(i64, i64, f64)>| -> String {
                match row {
                    Some((ts, ticks, spread)) => {
                        let dt = chrono::DateTime::from_timestamp(ts, 0)
                            .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
                            .unwrap_or_else(|| "?".into());
                        format!("{}: {} ticks:{} spread:{:.1}pips", label, dt, ticks, spread)
                    }
                    None => format!("{}: N/A", label),
                }
            };

            match ml_result {
                Ok(Ok((total, new_rows, already_exists))) => {
                    let features_table2 = format!("{}_tick_features_m1", request.symbol.to_lowercase());
                    let db2 = CandleDatabase::new("Bots_db/Algo_EURUSD.duckdb");
                    let (first_record, last_record) = match db2 {
                        Ok(db) => {
                            let first = db.get_first_ml_feature(&features_table2).ok().flatten();
                            let last = db.get_last_ml_feature(&features_table2).ok().flatten();
                            (fmt_ml_row("First ML", first), fmt_ml_row("Last ML", last))
                        }
                        Err(_) => ("First ML: N/A".into(), "Last ML: N/A".into()),
                    };
                    let _ = response_tx.send(DataResponse::MLFeaturesComplete {
                        symbol: request.symbol.clone(),
                        total_rows: total as u64,
                        new_rows: new_rows as u64,
                        already_exists,
                        first_record,
                        last_record,
                    }).await;
                }
                Ok(Err(e)) => {
                    println!("[ML] DB error: {}", e);
                    let _ = response_tx.send(DataResponse::Error {
                        symbol: request.symbol.clone(),
                        kind: DataKind::M1Candles,
                        message: format!("ML features error: {}", e),
                    }).await;
                }
                Err(e) => {
                    println!("[ML] Task error: {}", e);
                    let _ = response_tx.send(DataResponse::Error {
                        symbol: request.symbol.clone(),
                        kind: DataKind::M1Candles,
                        message: format!("ML features task error: {}", e),
                    }).await;
                }
            }
        }
    }
    Ok(())
}

/// Check DuckDB for existing data and send status response
async fn check_db_status(
    request: &DataRequest,
    table_name: &str,
    response_tx: &mpsc::Sender<DataResponse>,
) {
    let db_path = "Bots_db/Algo_EURUSD.duckdb";
    match CandleDatabase::new(db_path) {
        Ok(db) => {
            if !db.table_exists(table_name) {
                let _ = response_tx.send(DataResponse::StatusEmpty {
                    symbol: request.symbol.clone(),
                    kind: request.kind,
                }).await;
                return;
            }
            match request.kind {
                DataKind::M1Candles => {
                    match db.count_candles(table_name) {
                        Ok(count) if count > 0 => {
                            let (oldest, newest) = db.get_candle_date_range(table_name).unwrap_or((0, 0));
                            let first_record = match db.get_first_candle(table_name) {
                                Ok(Some(c)) => {
                                    let dt = chrono::DateTime::from_timestamp(c.timestamp, 0)
                                        .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
                                        .unwrap_or_else(|| "?".into());
                                    format!("First M1: {} O:{:.5} H:{:.5} L:{:.5} C:{:.5} V:{}", dt, c.open, c.high, c.low, c.close, c.volume)
                                }
                                _ => "First M1: N/A".into(),
                            };
                            let last_record = match db.get_last_candle(table_name) {
                                Ok(Some(c)) => {
                                    let dt = chrono::DateTime::from_timestamp(c.timestamp, 0)
                                        .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
                                        .unwrap_or_else(|| "?".into());
                                    format!("Last M1: {} O:{:.5} H:{:.5} L:{:.5} C:{:.5} V:{}", dt, c.open, c.high, c.low, c.close, c.volume)
                                }
                                _ => "Last M1: N/A".into(),
                            };
                            let _ = response_tx.send(DataResponse::StatusFound {
                                symbol: request.symbol.clone(),
                                kind: request.kind,
                                count,
                                oldest_ts: oldest,
                                newest_ts: newest,
                                first_record,
                                last_record,
                            }).await;
                        }
                        _ => {
                            let _ = response_tx.send(DataResponse::StatusEmpty {
                                symbol: request.symbol.clone(),
                                kind: request.kind,
                            }).await;
                        }
                    }
                }
                DataKind::TickData | DataKind::TickDataAsk => {
                    let label = if request.kind == DataKind::TickDataAsk { "ask tick" } else { "bid tick" };
                    match db.count_ticks(table_name) {
                        Ok(count) if count > 0 => {
                            let (oldest_ms, newest_ms) = db.get_tick_date_range(table_name).unwrap_or((0, 0));
                            let first_record = match db.get_first_tick(table_name) {
                                Ok(Some((ts_ms, price))) => {
                                    let dt = chrono::DateTime::from_timestamp_millis(ts_ms)
                                        .map(|d| d.format("%Y-%m-%d %H:%M:%S%.3f").to_string())
                                        .unwrap_or_else(|| "?".into());
                                    format!("First {}: {} Price:{:.5}", label, dt, price)
                                }
                                _ => format!("First {}: N/A", label),
                            };
                            let last_record = match db.get_last_tick(table_name) {
                                Ok(Some((ts_ms, price))) => {
                                    let dt = chrono::DateTime::from_timestamp_millis(ts_ms)
                                        .map(|d| d.format("%Y-%m-%d %H:%M:%S%.3f").to_string())
                                        .unwrap_or_else(|| "?".into());
                                    format!("Last {}: {} Price:{:.5}", label, dt, price)
                                }
                                _ => format!("Last {}: N/A", label),
                            };
                            let _ = response_tx.send(DataResponse::StatusFound {
                                symbol: request.symbol.clone(),
                                kind: request.kind,
                                count,
                                oldest_ts: oldest_ms / 1000,
                                newest_ts: newest_ms / 1000,
                                first_record,
                                last_record,
                            }).await;
                        }
                        _ => {
                            let _ = response_tx.send(DataResponse::StatusEmpty {
                                symbol: request.symbol.clone(),
                                kind: request.kind,
                            }).await;
                        }
                    }
                }
            }
        }
        Err(_) => {
            let _ = response_tx.send(DataResponse::StatusEmpty {
                symbol: request.symbol.clone(),
                kind: request.kind,
            }).await;
        }
    }
}

/// Get the newest timestamp currently in DB (returns ms)
fn get_newest_timestamp_in_db(table_name: &str, kind: DataKind) -> i64 {
    let db_path = "Bots_db/Algo_EURUSD.duckdb";
    match CandleDatabase::new(db_path) {
        Ok(db) => {
            if !db.table_exists(table_name) {
                // No table → start from earliest available (Jan 1, 2010)
                return 1_262_304_000_000_i64;
            }
            match kind {
                DataKind::M1Candles => {
                    db.get_candle_date_range(table_name)
                        .map(|(_, newest)| newest * 1000) // seconds → ms
                        .unwrap_or(1_262_304_000_000_i64) // fallback: Jan 1, 2010
                }
                DataKind::TickData | DataKind::TickDataAsk => {
                    db.get_tick_date_range(table_name)
                        .map(|(_, newest_ms)| newest_ms)
                        .unwrap_or(1_262_304_000_000_i64) // fallback: Jan 1, 2010
                }
            }
        }
        Err(_) => {
            1_262_304_000_000_i64 // Jan 1, 2010
        }
    }
}

/// Send the first API request for historical data
async fn send_first_api_request(
    stream: &mut tokio_rustls::client::TlsStream<TcpStream>,
    request: &DataRequest,
    account_id: i64,
    from_ms: i64,
    to_ms: i64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match request.kind {
        DataKind::M1Candles => {
            let req = openapi::ProtoOaGetTrendbarsReq {
                payload_type: Some(openapi::ProtoOaPayloadType::ProtoOaGetTrendbarsReq as i32),
                ctid_trader_account_id: account_id,
                from_timestamp: Some(from_ms),
                to_timestamp: Some(to_ms),
                period: 1, // M1
                symbol_id: request.symbol_id,
                count: None,
            };
            println!("Sending GetTrendbarsReq M1 for {} (from {} to {})", request.symbol, from_ms, to_ms);
            send_message(stream, 2137, req).await?;
        }
        DataKind::TickData => {
            let req = openapi::ProtoOaGetTickDataReq {
                payload_type: Some(openapi::ProtoOaPayloadType::ProtoOaGetTickdataReq as i32),
                ctid_trader_account_id: account_id,
                symbol_id: request.symbol_id,
                r#type: 1, // Bid
                from_timestamp: Some(from_ms),
                to_timestamp: Some(to_ms),
            };
            println!("Sending GetTickDataReq (Bid) for {} (from {} to {})", request.symbol, from_ms, to_ms);
            send_message(stream, 2145, req).await?;
        }
        DataKind::TickDataAsk => {
            let req = openapi::ProtoOaGetTickDataReq {
                payload_type: Some(openapi::ProtoOaPayloadType::ProtoOaGetTickdataReq as i32),
                ctid_trader_account_id: account_id,
                symbol_id: request.symbol_id,
                r#type: 2, // Ask
                from_timestamp: Some(from_ms),
                to_timestamp: Some(to_ms),
            };
            println!("Sending GetTickDataReq (Ask) for {} (from {} to {})", request.symbol, from_ms, to_ms);
            send_message(stream, 2145, req).await?;
        }
    }
    Ok(())
}

/// Handle GetTrendbarsRes — convert, store, paginate
async fn handle_trendbars_response(
    stream: &mut tokio_rustls::client::TlsStream<TcpStream>,
    res: &openapi::ProtoOaGetTrendbarsRes,
    account_id: i64,
    response_tx: &mpsc::Sender<DataResponse>,
    active_download: &mut Option<ActiveDownload>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let dl = match active_download.as_mut() {
        Some(d) => d,
        None => {
            println!("[TRENDBAR] No active download, ignoring");
            return Ok(());
        }
    };

    println!("[TRENDBAR] Converting {} bars...", res.trendbar.len());

    // Convert trendbars to candles
    let mut candles: Vec<Candle> = res.trendbar.iter()
        .filter_map(|bar| {
            trendbar_to_candle(
                bar.volume,
                bar.low,
                bar.delta_open,
                bar.delta_close,
                bar.delta_high,
                bar.utc_timestamp_in_minutes,
            )
        })
        .collect();

    candles.sort_by_key(|c| c.timestamp);

    let batch_count = candles.len() as u64;
    dl.total_downloaded += batch_count;

    // Calculate progress percentage FIRST (before DB insert)
    let current_pos_ms = candles.first().map(|c| c.timestamp * 1000).unwrap_or(dl.to_timestamp_ms);
    let total_range = dl.to_timestamp_ms - dl.from_timestamp_ms;
    let covered = dl.to_timestamp_ms - current_pos_ms;
    let pct = if total_range > 0 { ((covered as f64 / total_range as f64) * 100.0).min(100.0) } else { 0.0 };

    println!("[TRENDBAR] {} candles, total={}, {:.0}%, has_more={:?}",
        batch_count, dl.total_downloaded, pct, res.has_more);

    // Send progress to UI immediately (before slow DB insert)
    let _ = response_tx.send(DataResponse::Progress {
        symbol: dl.symbol.clone(),
        kind: dl.kind,
        downloaded_rows: dl.total_downloaded,
        message: format!("Downloading M1 candles... {:.0}% ({} rows)", pct, dl.total_downloaded),
    }).await;

    // Append candles to temp CSV file (fast file I/O, no DB involved)
    if !candles.is_empty() {
        let mut file = std::fs::OpenOptions::new().append(true).open(&dl.csv_path)?;
        let mut buf = std::io::BufWriter::new(&mut file);
        for c in &candles {
            writeln!(buf, "{},{},{},{},{},{}", c.timestamp, c.open, c.high, c.low, c.close, c.volume)?;
        }
    }

    // Paginate if more data available
    if res.has_more == Some(true) && batch_count > 0 {
        let oldest_ts_sec = current_pos_ms / 1000;
        let new_to_ms = oldest_ts_sec * 1000 - 1;

        // Rate limit
        tokio::time::sleep(Duration::from_millis(500)).await;

        let req = openapi::ProtoOaGetTrendbarsReq {
            payload_type: Some(openapi::ProtoOaPayloadType::ProtoOaGetTrendbarsReq as i32),
            ctid_trader_account_id: account_id,
            from_timestamp: Some(dl.from_timestamp_ms),
            to_timestamp: Some(new_to_ms),
            period: 1, // M1
            symbol_id: dl.symbol_id,
            count: None,
        };
        send_message(stream, 2137, req).await?;
    } else {
        // All batches received — bulk load CSV into DuckDB
        let total = dl.total_downloaded;
        let symbol = dl.symbol.clone();
        let kind = dl.kind;
        let csv_path = dl.csv_path.clone();
        let table_name = dl.table_name.clone();

        // Tell UI we're loading into DB
        let _ = response_tx.send(DataResponse::Progress {
            symbol: symbol.clone(),
            kind,
            downloaded_rows: total,
            message: format!("Loading {} M1 candles into database...", total),
        }).await;

        // Bulk load CSV → DuckDB (await — only happens once, read_csv is fast)
        let load_result = tokio::task::spawn_blocking(move || {
            let db_path = "Bots_db/Algo_EURUSD.duckdb";
            let db = CandleDatabase::new(db_path)?;
            db.create_table_if_not_exists(&table_name)?;
            db.bulk_load_candles_from_csv(&table_name, &csv_path)?;
            let _ = std::fs::remove_file(&csv_path);
            Ok::<(), duckdb::Error>(())
        }).await;

        match load_result {
            Ok(Ok(())) => {
                println!("[TRENDBAR] Bulk CSV load complete! {} rows", total);
                let _ = response_tx.send(DataResponse::Complete {
                    symbol, kind, total_rows: total,
                }).await;
            }
            Ok(Err(e)) => {
                println!("[TRENDBAR] Bulk load DB error: {}", e);
                let _ = response_tx.send(DataResponse::Error {
                    symbol, kind, message: format!("DB bulk load error: {}", e),
                }).await;
            }
            Err(e) => {
                println!("[TRENDBAR] Bulk load task error: {}", e);
                let _ = response_tx.send(DataResponse::Error {
                    symbol, kind, message: format!("Bulk load task error: {}", e),
                }).await;
            }
        }
        *active_download = None;
    }

    Ok(())
}

/// Handle GetTickDataRes — decode, store, paginate
async fn handle_tick_data_response(
    stream: &mut tokio_rustls::client::TlsStream<TcpStream>,
    res: &openapi::ProtoOaGetTickDataRes,
    account_id: i64,
    response_tx: &mpsc::Sender<DataResponse>,
    active_download: &mut Option<ActiveDownload>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let dl = match active_download.as_mut() {
        Some(d) => d,
        None => {
            println!("Received tick data response but no active download");
            return Ok(());
        }
    };

    // Extract timestamps and ticks from the proto structs
    let timestamps: Vec<i64> = res.tick_data.iter().map(|t| t.timestamp).collect();
    let ticks: Vec<i64> = res.tick_data.iter().map(|t| t.tick).collect();
    let decoded = decode_tick_data(&timestamps, &ticks);

    let batch_count = decoded.len() as u64;
    dl.total_downloaded += batch_count;

    // Sort decoded ticks
    let mut sorted = decoded;
    sorted.sort_by_key(|(ts, _)| *ts);

    // Calculate progress percentage FIRST (before DB insert)
    let current_pos_ms = sorted.first().map(|(ts, _)| *ts).unwrap_or(dl.to_timestamp_ms);
    let total_range = dl.to_timestamp_ms - dl.from_timestamp_ms;
    let covered = dl.to_timestamp_ms - current_pos_ms;
    let pct = if total_range > 0 { ((covered as f64 / total_range as f64) * 100.0).min(100.0) } else { 0.0 };

    let tick_label = if dl.kind == DataKind::TickDataAsk { "ask ticks" } else { "bid ticks" };
    println!("[TICK] {} {}, total={}, {:.0}%, has_more={}",
        batch_count, tick_label, dl.total_downloaded, pct, res.has_more);

    // Send progress to UI immediately (before slow DB insert)
    let _ = response_tx.send(DataResponse::Progress {
        symbol: dl.symbol.clone(),
        kind: dl.kind,
        downloaded_rows: dl.total_downloaded,
        message: format!("Downloading {}... {:.0}% ({} rows)", tick_label, pct, dl.total_downloaded),
    }).await;

    // Append ticks to temp CSV file (fast file I/O, no DB involved)
    if !sorted.is_empty() {
        let mut file = std::fs::OpenOptions::new().append(true).open(&dl.csv_path)?;
        let mut buf = std::io::BufWriter::new(&mut file);
        for (ts_ms, price) in &sorted {
            writeln!(buf, "{},{}", ts_ms, price)?;
        }
    }

    // Paginate — use correct tick type based on DataKind
    if res.has_more && !sorted.is_empty() {
        let oldest_ms = sorted.first().unwrap().0;
        let new_to_ms = oldest_ms - 1;

        let tick_type = match dl.kind {
            DataKind::TickDataAsk => 2, // Ask
            _ => 1, // Bid
        };

        tokio::time::sleep(Duration::from_millis(500)).await;

        let req = openapi::ProtoOaGetTickDataReq {
            payload_type: Some(openapi::ProtoOaPayloadType::ProtoOaGetTickdataReq as i32),
            ctid_trader_account_id: account_id,
            symbol_id: dl.symbol_id,
            r#type: tick_type,
            from_timestamp: Some(dl.from_timestamp_ms),
            to_timestamp: Some(new_to_ms),
        };
        send_message(stream, 2145, req).await?;
    } else {
        // All batches received — bulk load CSV into DuckDB
        let total = dl.total_downloaded;
        let symbol = dl.symbol.clone();
        let kind = dl.kind;
        let csv_path = dl.csv_path.clone();
        let table_name = dl.table_name.clone();
        let is_ask = kind == DataKind::TickDataAsk;

        let tick_label = if is_ask { "ask ticks" } else { "bid ticks" };

        // Tell UI we're loading into DB
        let _ = response_tx.send(DataResponse::Progress {
            symbol: symbol.clone(),
            kind,
            downloaded_rows: total,
            message: format!("Loading {} {} into database...", total, tick_label),
        }).await;

        // Bulk load CSV → DuckDB (await — only happens once, read_csv is fast)
        let load_result = tokio::task::spawn_blocking(move || {
            let db_path = "Bots_db/Algo_EURUSD.duckdb";
            let db = CandleDatabase::new(db_path)?;
            if is_ask {
                db.create_ask_tick_table_if_not_exists(&table_name)?;
                db.bulk_load_ask_ticks_from_csv(&table_name, &csv_path)?;
            } else {
                db.create_tick_table_if_not_exists(&table_name)?;
                db.bulk_load_ticks_from_csv(&table_name, &csv_path)?;
            }
            let _ = std::fs::remove_file(&csv_path);
            Ok::<(), duckdb::Error>(())
        }).await;

        match load_result {
            Ok(Ok(())) => {
                println!("[TICK] Bulk CSV load complete! {} {} rows", tick_label, total);
                let _ = response_tx.send(DataResponse::Complete {
                    symbol, kind, total_rows: total,
                }).await;
            }
            Ok(Err(e)) => {
                println!("[TICK] Bulk load DB error: {}", e);
                let _ = response_tx.send(DataResponse::Error {
                    symbol, kind, message: format!("DB bulk load error: {}", e),
                }).await;
            }
            Err(e) => {
                println!("[TICK] Bulk load task error: {}", e);
                let _ = response_tx.send(DataResponse::Error {
                    symbol, kind, message: format!("Bulk load task error: {}", e),
                }).await;
            }
        }
        *active_download = None;
    }

    Ok(())
}
