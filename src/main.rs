use prost::Message;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::rustls::{ClientConfig, RootCertStore, ServerName};
use tokio_rustls::TlsConnector;

// Note: You must compile the .proto files (OpenApiMessages.proto, etc.)
// to generate these Rust structs. For this example, we assume they are in 'openapi' module.
pub mod openapi {
    include!("proto/generated/_.rs");
}

pub mod news;
pub mod ai;
pub mod ui;

use ui::app::{AppState, CtraderApp};
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq)]
enum AuthState {
    NotAuthenticated,
    AppAuthenticated,
    AccountAuthenticated,
    SymbolsRequested,
    Subscribed,
}

async fn run_session(app_state: Arc<Mutex<AppState>>) -> Result<(), Box<dyn std::error::Error>> {
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
    {
        let mut state = app_state.lock().unwrap();
        state.connection_status = "Connected".to_string();
    }

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
    let mut heartbeat_interval = tokio::time::interval(Duration::from_secs(30)); // send heartbeat every 30 seconds
    heartbeat_interval.tick().await; // skip first immediate tick
    
    loop {
        let mut header = [0u8; 4];
        tokio::select! {
            // Read 4-byte header (Big Endian length) with timeout
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
                // Read payload with timeout
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
                // Deserialize ProtoMessage
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
                        // Send Account Auth
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
                        // Send Symbols List Request
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
                            
                            // Find both BTCUSD and EURUSD symbol IDs
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
                            
                            // Subscribe to both symbols if found
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
                            
                            // Price scaling: divide by 100_000 as per proto spec (1/100000 of unit of price)
                            let bid = event.bid.unwrap_or(0) as f64 / 100_000.0;
                            let ask = event.ask.unwrap_or(0) as f64 / 100_000.0;
                            
                            // Check which symbol this update is for
                            let mut state = app_state.lock().unwrap();
                            
                            if Some(symbol_id) == btc_symbol_id {
                                // Only update if we received valid non-zero prices
                                if bid > 0.0 && ask > 0.0 {
                                    println!("LIVE BTCUSD | Bid: {:.2} | Ask: {:.2}", bid, ask);
                                    
                                    // Set opening price on first update
                                    if state.btc_open == 0.0 {
                                        state.btc_open = bid;
                                    }
                                    
                                    // Store previous prices before updating
                                    state.btc_prev_bid = state.btc_bid;
                                    state.btc_prev_ask = state.btc_ask;
                                    
                                    state.btc_bid = bid;
                                    state.btc_ask = ask;
                                    state.btc_price = bid;
                                }
                                
                                // 1-minute candle aggregation (60 seconds)
                                let interval = 60;
                                let now = chrono::Utc::now().timestamp();
                                let bucket_start = (now / interval) * interval;
                                
                                use crate::ui::app::Candle;
                                if let Some(last_candle) = state.candles.last_mut() {
                                    if last_candle.time == bucket_start {
                                        // Update current candle
                                        last_candle.close = bid;
                                        if bid > last_candle.high { last_candle.high = bid; }
                                        if bid < last_candle.low { last_candle.low = bid; }
                                    } else {
                                        // Start new candle
                                        state.candles.push(Candle {
                                            time: bucket_start,
                                            open: bid,
                                            high: bid,
                                            low: bid,
                                            close: bid,
                                        });
                                    }
                                } else {
                                    // First candle
                                    state.candles.push(Candle {
                                        time: bucket_start,
                                        open: bid,
                                        high: bid,
                                        low: bid,
                                        close: bid,
                                    });
                                }
                                
                                if state.candles.len() > 200 {
                                    state.candles.remove(0);
                                }
                            } else if Some(symbol_id) == eurusd_symbol_id {
                                // Only update if we received valid non-zero prices
                                if bid > 0.0 && ask > 0.0 {
                                    println!("LIVE EURUSD | Bid: {:.5} | Ask: {:.5}", bid, ask);
                                    
                                    // Set opening price on first update
                                    if state.eurusd_open == 0.0 {
                                        state.eurusd_open = bid;
                                    }
                                    
                                    // Store previous prices before updating
                                    state.eurusd_prev_bid = state.eurusd_bid;
                                    state.eurusd_prev_ask = state.eurusd_ask;
                                    
                                    state.eurusd_bid = bid;
                                    state.eurusd_ask = ask;
                                }
                            }
                        }
                    },
                    2142 => { // ProtoOAErrorRes
                        if let Some(payload) = &msg.payload {
                            let err = openapi::ProtoOaErrorRes::decode(payload.as_slice())?;
                            println!("ERROR: {} - {}", err.error_code, err.description.unwrap_or_default());
                            // If error is fatal, break
                            if err.error_code == "CH_CLIENT_AUTH_FAILURE" || err.error_code == "ACCOUNT_NOT_AUTHORIZED" {
                                break;
                            }
                            // Handle rate‑limit error
                            if err.error_code == "BLOCKED_PAYLOAD_TYPE" {
                                let retry_after = err.retry_after.unwrap_or(5); // default 5 seconds
                                if retry_after > 300 {
                                    println!("Rate limit too long ({} seconds). Treating as fatal error.", retry_after);
                                    break;
                                }
                                println!("Rate limited. Waiting {} seconds before retrying...", retry_after);
                                tokio::time::sleep(Duration::from_secs(retry_after)).await;
                                // After waiting, we can try to resend the account auth if we are in AppAuthenticated state
                                if _auth_state == AuthState::AppAuthenticated {
                                    let account_auth = openapi::ProtoOaAccountAuthReq {
                                        payload_type: Some(openapi::ProtoOaPayloadType::ProtoOaAccountAuthReq as i32),
                                        ctid_trader_account_id: account_id,
                                        access_token: access_token.clone(),
                                    };
                                    send_message(&mut tls_stream, openapi::ProtoOaPayloadType::ProtoOaAccountAuthReq as u32, account_auth).await?;
                                    println!("Account auth resent after rate limit");
                                }
                                // Do not break; continue loop
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
                // Send periodic heartbeat
                if let Err(e) = send_heartbeat(&mut tls_stream).await {
                    println!("Failed to send periodic heartbeat: {}", e);
                    break;
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(60)) => {
                // Check if we haven't received a heartbeat from server in 60 seconds
                if last_heartbeat.elapsed() > Duration::from_secs(60) {
                    println!("No heartbeat from server for 60 seconds, reconnecting...");
                    break;
                }
            }
        }
    }

    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load environment variables from .env file
    dotenv::dotenv().ok();

    let app_state = Arc::new(Mutex::new(AppState {
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
    }));

    // Create tokio runtime manually since we need to run eframe on main thread
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    let state_for_task = Arc::clone(&app_state);
    rt.spawn(async move {
        // Start news scraper in a separate task
        news::spawn_news_scraper();
        
        let mut backoff_seconds = 1;
        loop {
            println!("Starting cTrader price stream...");
            match run_session(state_for_task.clone()).await {
                Ok(_) => println!("Session ended gracefully."),
                Err(e) => {
                    println!("Session error: {}", e);
                    {
                        let mut state = state_for_task.lock().unwrap();
                        state.connection_status = format!("Error: {}", e);
                    }
                }
            }
            
            // Exponential backoff
            tokio::time::sleep(Duration::from_secs(backoff_seconds)).await;
            backoff_seconds = (backoff_seconds * 2).min(30);
        }
    });

    // Run UI on the main thread
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([800.0, 600.0]),
        ..Default::default()
    };

    eframe::run_native(
        "cTrader Rust Terminal",
        options,
        Box::new(|_cc| {
            Box::new(CtraderApp::new(app_state))
        }),
    ).map_err(|e| e.to_string().into())
}

async fn send_message<T: Message>(
    stream: &mut tokio_rustls::client::TlsStream<TcpStream>,
    payload_type: u32,
    payload: T,
) -> Result<(), Box<dyn std::error::Error>> {
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

    // Prefix with 4-byte length in Big Endian
    let len = (full_msg.len() as u32).to_be_bytes();
    stream.write_all(&len).await?;
    stream.write_all(&full_msg).await?;
    Ok(())
}

async fn send_heartbeat(stream: &mut tokio_rustls::client::TlsStream<TcpStream>) -> Result<(), Box<dyn std::error::Error>> {
    let proto_msg = openapi::ProtoMessage {
        payload_type: 51, // heartbeat payload type
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