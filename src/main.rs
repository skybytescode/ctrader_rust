use prost::Message;
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

const DB_PATH: &str = "Bots_db/Algo_EURUSD.duckdb";

/// Mutex that serializes ALL DuckDB file access. Each caller opens/closes its own
/// connection while holding the lock, ensuring only one connection exists at a time.
/// This allows Python scripts to access the DB between Rust operations.
pub type SharedDb = Arc<std::sync::Mutex<()>>;
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
pub mod ec_realtime;
pub mod news_realtime;
pub mod news_sentiment;

use db::{Candle, CandleDatabase};
use data_retrieval::{
    DataKind, DataAction, DataRequest, DataResponse,
    trendbar_to_candle, decode_tick_data, get_data_table_name,
};

/// Message types for communication between async tasks and UI
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
    /// DoM capture status update
    DomCaptureStatus(String),
    /// EC Calendar today's events for UI display
    EcTodayEvents(Vec<String>),
    /// EC Calendar raw event data for client-side countdown
    EcTodayRaw(Vec<(String, String, i32, String, Option<f64>, Option<f64>, Option<f64>, Option<f64>)>),
    /// EC Calendar status message
    EcStatus(String),
    /// EC Calendar capture status (for the button)
    EcCaptureActive(bool),
    /// News today's articles for UI display
    NewsTodayLines(Vec<String>),
    /// News status message
    NewsStatus(String),
    /// News capture status (for the button)
    NewsCaptureActive(bool),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum AuthState {
    NotAuthenticated,
    AppAuthenticated,
    AccountAuthenticated,
    SymbolsRequested,
    Subscribed,
}

/// Spawn the econcal Node.js proxy server as a background process.
/// Streams its stdout/stderr to our stdout so startup status is visible.
/// If Node.js is not found or port 6000 is already in use, logs and continues.
fn start_econcal_server() {
    std::thread::spawn(|| {
        let econcal_dir = "D:/RustProjects/ctrader_rust/econcal";

        // Skip if something is already listening on port 6000
        if std::net::TcpStream::connect("127.0.0.1:6000").is_ok() {
            println!("[econcal] Port 6000 already in use — skipping launch.");
            return;
        }

        println!("[econcal] Starting proxy server (node econcal.js)...");

        // Install dependencies if node_modules is missing
        if !std::path::Path::new(econcal_dir).join("node_modules").exists() {
            println!("[econcal] node_modules not found, running npm install...");
            match std::process::Command::new("npm")
                .args(["install", "--prefer-offline"])
                .current_dir(econcal_dir)
                .status()
            {
                Ok(s) if s.success() => println!("[econcal] npm install done."),
                Ok(s) => println!("[econcal] npm install exited: {}", s),
                Err(e) => println!("[econcal] npm install failed: {}", e),
            }
        }

        let result = std::process::Command::new("node")
            .arg("econcal.js")
            .current_dir(econcal_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn();

        match result {
            Err(e) => {
                println!("[econcal] Failed to start: {} (is Node.js installed?)", e);
            }
            Ok(mut child) => {
                use std::io::BufRead;
                // Stream stdout
                if let Some(stdout) = child.stdout.take() {
                    let reader = std::io::BufReader::new(stdout);
                    for line in reader.lines().flatten() {
                        println!("[econcal] {}", line);
                    }
                }
                // Capture stderr
                if let Some(stderr) = child.stderr.take() {
                    let reader = std::io::BufReader::new(stderr);
                    for line in reader.lines().flatten() {
                        println!("[econcal] ERR: {}", line);
                    }
                }
                let _ = child.wait();
                println!("[econcal] Server process exited.");
            }
        }
    });
}

fn main() {
    // Load environment variables from .env file
    dotenv::dotenv().ok();

    // Start the econcal FXStreet proxy server in the background
    start_econcal_server();

    // Create channel for price updates (network -> UI)
    let (tx, rx) = mpsc::channel::<PriceUpdate>(100);

    // Create channels for data retrieval (UI <-> network)
    let (data_req_tx, data_req_rx) = mpsc::channel::<DataRequest>(32);
    let (data_resp_tx, data_resp_rx) = mpsc::channel::<DataResponse>(64);

    // Mutex serializes all DuckDB file access (only one connection at a time on Windows)
    let shared_db: SharedDb = Arc::new(std::sync::Mutex::new(()));
    let db_for_async = shared_db.clone();

    // Spawn the tokio runtime in a separate thread for async tasks
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create tokio runtime");

        rt.block_on(async move {
            // Run price streaming with reconnection
            // request_rx passed by &mut so pending requests survive reconnects
            let mut request_rx = data_req_rx;
            let mut backoff_seconds = 1;
            loop {
                println!("Starting cTrader price stream...");
                match run_session(tx.clone(), &mut request_rx, data_resp_tx.clone(), db_for_async.clone()).await {
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

    // Run egui/eframe app on main thread
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 800.0])
            .with_title("cTrader Rust Terminal"),
        ..Default::default()
    };
    eframe::run_native(
        "cTrader Rust Terminal",
        options,
        Box::new(|_cc| Ok(Box::new(ui::CTraderApp::new(rx, data_req_tx, data_resp_rx, shared_db)))),
    ).expect("Failed to start eframe");

    // Window closed — force exit to kill background threads and child processes
    std::process::exit(0);
}

async fn run_session(
    tx: mpsc::Sender<PriceUpdate>,
    request_rx: &mut mpsc::Receiver<DataRequest>,
    response_tx: mpsc::Sender<DataResponse>,
    shared_db: SharedDb,
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
        "GBPUSD", "USDJPY", "USDCHF", "AUDUSD", "EURJPY", "XAUUSD",
    ];

    let mut last_heartbeat = tokio::time::Instant::now();
    let mut heartbeat_interval = tokio::time::interval(Duration::from_secs(30));
    heartbeat_interval.tick().await; // skip first immediate tick

    // Active download state for historical data retrieval
    let mut active_download: Option<ActiveDownload> = None;

    // ── DoM capture state ────────────────────────────────────────────────
    let mut dom_capturing = false;
    // In-memory order book: quote_id → (side, price, size)
    // side: 0=bid, 1=ask
    let mut dom_book: std::collections::HashMap<u64, (u8, i32, i64)> = std::collections::HashMap::new();
    // Running book totals (updated incrementally on each quote change)
    let mut dom_total_bid_vol: f64 = 0.0;
    let mut dom_total_ask_vol: f64 = 0.0;
    let mut dom_bid_levels: u32 = 0;
    let mut dom_ask_levels: u32 = 0;
    // Batch buffer for DuckDB writes (flushed periodically)
    let mut dom_batch: Vec<(i64, u8, u64, u8, i32, i64)> = Vec::with_capacity(10_000);
    let mut dom_last_flush = tokio::time::Instant::now();
    let mut dom_total_rows: u64 = 0;
    let mut dom_last_status_update = tokio::time::Instant::now();
    let mut dom_rows_since_status: u64 = 0;

    // ── DoM DB writer channel (offloads blocking writes from async loop) ──
    enum DomDbCommand {
        /// Open DB, create tables, cleanup old data. Returns (existing_rows, last_ts_str).
        Init { reply: tokio::sync::oneshot::Sender<(u64, String)> },
        /// Write a batch of raw DoM rows via appender
        RawBatch { rows: Vec<(i64, u8, u64, u8, i32, i64)> },
        /// Write one M1 feature row
        M1Features { params: [f64; 18] },
        /// Final flush + close DB. Returns total rows written by writer.
        Close { reply: tokio::sync::oneshot::Sender<u64> },
    }
    let (dom_db_tx, mut dom_db_rx) = mpsc::channel::<DomDbCommand>(64);

    // Spawn the DoM DB writer on a blocking thread (lives until channel closes)
    let dom_shared_db = shared_db.clone();
    tokio::task::spawn_blocking(move || {
        let mut writer_rows: u64 = 0;

        while let Some(cmd) = dom_db_rx.blocking_recv() {
            match cmd {
                DomDbCommand::Init { reply } => {
                    let _lock = dom_shared_db.lock().unwrap();
                    let conn = duckdb::Connection::open(DB_PATH)
                        .expect("Failed to open DuckDB for DoM");
                    conn.execute_batch("
                        CREATE TABLE IF NOT EXISTS eurusd_dom_raw (
                            ts_ms        BIGINT NOT NULL,
                            event_type   TINYINT NOT NULL,
                            quote_id     BIGINT NOT NULL,
                            side         TINYINT,
                            price        INTEGER,
                            size         BIGINT
                        );
                        CREATE TABLE IF NOT EXISTS eurusd_dom_features_m1 (
                            timestamp    BIGINT PRIMARY KEY,
                            obi_mean     FLOAT,
                            obi_std      FLOAT,
                            spread_mean  FLOAT,
                            spread_max   FLOAT,
                            bid_vol_mean FLOAT,
                            ask_vol_mean FLOAT,
                            bid_vol_min  FLOAT,
                            ask_vol_min  FLOAT,
                            bid_ask_ratio FLOAT,
                            bid_levels   FLOAT,
                            ask_levels   FLOAT,
                            churn_rate   FLOAT,
                            quotes_added INTEGER,
                            quotes_deleted INTEGER,
                            best_bid_max BIGINT,
                            best_ask_max BIGINT,
                            snapshots    INTEGER
                        );
                    ").expect("Failed to create DoM tables");
                    let cutoff_ms = chrono::Utc::now().timestamp_millis()
                        - (8 * 7 * 24 * 3600 * 1000_i64);
                    let _ = conn.execute(
                        "DELETE FROM eurusd_dom_raw WHERE ts_ms < ?",
                        duckdb::params![cutoff_ms],
                    );
                    let existing: (u64, String) = match conn.query_row(
                        "SELECT COUNT(*), COALESCE(MAX(ts_ms)::VARCHAR, '0') FROM eurusd_dom_raw",
                        [],
                        |row| Ok((row.get::<_, i64>(0).unwrap_or(0) as u64, row.get::<_, String>(1).unwrap_or_default())),
                    ) {
                        Ok(v) => v,
                        Err(_) => (0, "0".to_string()),
                    };
                    writer_rows = existing.0;
                    let _ = reply.send(existing);
                }
                DomDbCommand::RawBatch { rows } => {
                    let _lock = dom_shared_db.lock().unwrap();
                    if let Ok(conn) = duckdb::Connection::open(DB_PATH) {
                        if let Ok(mut appender) = conn.appender("eurusd_dom_raw") {
                            for &(ts, etype, qid, side, price, size) in &rows {
                                let _ = appender.append_row(duckdb::params![
                                    ts, etype as i8, qid as i64, side as i8, price, size
                                ]);
                            }
                            let _ = appender.flush();
                            writer_rows += rows.len() as u64;
                        }
                    }
                }
                DomDbCommand::M1Features { params } => {
                    let _lock = dom_shared_db.lock().unwrap();
                    if let Ok(conn) = duckdb::Connection::open(DB_PATH) {
                        let _ = conn.execute(
                            "INSERT OR REPLACE INTO eurusd_dom_features_m1 VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                            duckdb::params![
                                params[0] as i64, params[1], params[2], params[3], params[4],
                                params[5], params[6], params[7], params[8], params[9],
                                params[10], params[11], params[12],
                                params[13] as i32, params[14] as i32,
                                params[15] as i64, params[16] as i64, params[17] as i32,
                            ],
                        );
                    }
                }
                DomDbCommand::Close { reply } => {
                    let _ = reply.send(writer_rows);
                }
            }
        }
    });

    // ── DoM M1 aggregation state ─────────────────────────────────────────
    // Accumulates per-minute features from live book snapshots
    struct DomM1Accum {
        minute_ts: i64,           // start of current minute (unix seconds)
        snapshots: u32,           // number of book snapshots this minute
        obi_sum: f64,             // sum of OBI values for mean
        obi_sq_sum: f64,          // sum of OBI^2 for std
        spread_sum: f64,          // sum of spread values
        spread_max: f64,          // max spread seen
        total_bid_vol_sum: f64,   // sum of total bid volume
        total_ask_vol_sum: f64,   // sum of total ask volume
        total_bid_vol_min: f64,   // min total bid vol (liquidity vacuum)
        total_ask_vol_min: f64,   // min total ask vol
        bid_levels_sum: u32,      // sum of bid level counts
        ask_levels_sum: u32,      // sum of ask level counts
        quotes_added: u32,        // new/update events this minute
        quotes_deleted: u32,      // delete events this minute
        best_bid_max_size: i64,   // max size seen at best bid
        best_ask_max_size: i64,   // max size seen at best ask
    }
    impl DomM1Accum {
        fn new(ts: i64) -> Self {
            Self {
                minute_ts: ts,
                snapshots: 0,
                obi_sum: 0.0, obi_sq_sum: 0.0,
                spread_sum: 0.0, spread_max: 0.0,
                total_bid_vol_sum: 0.0, total_ask_vol_sum: 0.0,
                total_bid_vol_min: f64::MAX, total_ask_vol_min: f64::MAX,
                bid_levels_sum: 0, ask_levels_sum: 0,
                quotes_added: 0, quotes_deleted: 0,
                best_bid_max_size: 0, best_ask_max_size: 0,
            }
        }
    }
    let mut dom_m1: Option<DomM1Accum> = None;
    // EURUSD symbol_id for DoM subscription (resolved after symbol list)
    let mut dom_eurusd_id: Option<i64> = None;

    // ── EC Calendar smart scheduling state ─────────────────────────────
    let mut ec_today_date: Option<String> = None;
    // Scheduled fetch times (UTC timestamps in seconds) for today's events.
    // For each event: event_time+0, +60, +120, +300 seconds.
    let mut ec_scheduled_fetches: Vec<i64> = Vec::new();
    let mut ec_schedule_loaded = false;
    // Force initial schedule fetch 10 seconds after auth
    let mut ec_next_schedule_fetch: Option<tokio::time::Instant> = None;
    let mut ec_last_fetch_ts: i64 = 0; // last UTC timestamp we fetched at
    let mut ec_capturing = false; // toggled by UI button

    // ── News capture state ───────────────────────────────────────────────
    let mut news_capturing = false;
    let mut news_next_fetch: Option<tokio::time::Instant> = None;
    let mut news_today_date: Option<String> = None;

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

                                // Schedule first EC calendar fetch in 10 seconds
                                ec_next_schedule_fetch = Some(
                                    tokio::time::Instant::now() + Duration::from_secs(10)
                                );

                                // Send reverse mapping (name -> id) to Bevy for data retrieval
                                let reverse_map: std::collections::HashMap<String, i64> = symbol_id_to_name
                                    .iter()
                                    .map(|(&id, name)| (name.clone(), id))
                                    .collect();
                                let _ = tx.send(PriceUpdate::SymbolMapping(reverse_map)).await;

                                // Capture EURUSD symbol_id for DoM subscription
                                for (&id, name) in &symbol_id_to_name {
                                    if name == "EURUSD" {
                                        dom_eurusd_id = Some(id);
                                        println!("DoM: EURUSD symbol_id = {}", id);
                                        break;
                                    }
                                }
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
                                &shared_db,
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
                                &shared_db,
                            ).await?;
                        }
                    },
                    2155 => { // ProtoOADepthEvent
                        if dom_capturing {
                            if let Some(payload) = &msg.payload {
                                let event = openapi::ProtoOaDepthEvent::decode(payload.as_slice())?;
                                let now_ms = chrono::Utc::now().timestamp_millis();

                                // Process new/updated quotes (maintain running totals)
                                for q in &event.new_quotes {
                                    let (side, price) = if let Some(bid) = q.bid {
                                        (0u8, bid as i32)
                                    } else if let Some(ask) = q.ask {
                                        (1u8, ask as i32)
                                    } else {
                                        continue;
                                    };
                                    let size = q.size as i64;
                                    // Remove old entry from running totals if updating
                                    if let Some(&(old_side, _, old_size)) = dom_book.get(&q.id) {
                                        if old_side == 0 { dom_total_bid_vol -= old_size as f64; dom_bid_levels -= 1; }
                                        else { dom_total_ask_vol -= old_size as f64; dom_ask_levels -= 1; }
                                    }
                                    // Add new entry to running totals
                                    if side == 0 { dom_total_bid_vol += size as f64; dom_bid_levels += 1; }
                                    else { dom_total_ask_vol += size as f64; dom_ask_levels += 1; }
                                    dom_book.insert(q.id, (side, price, size));
                                    dom_batch.push((now_ms, 0, q.id, side, price, size));
                                }

                                // Process deleted quotes (maintain running totals)
                                for &qid in &event.deleted_quotes {
                                    if let Some((side, price, old_size)) = dom_book.remove(&qid) {
                                        if side == 0 { dom_total_bid_vol -= old_size as f64; dom_bid_levels -= 1; }
                                        else { dom_total_ask_vol -= old_size as f64; dom_ask_levels -= 1; }
                                        dom_batch.push((now_ms, 1, qid, side, price, 0));
                                    }
                                }

                                // ── M1 aggregation: snapshot current book state ──
                                let current_minute = now_ms / 60_000 * 60; // truncate to minute (unix seconds)
                                let n_added = event.new_quotes.len() as u32;
                                let n_deleted = event.deleted_quotes.len() as u32;

                                // Check for minute rollover → flush previous minute to DB writer
                                if let Some(ref acc) = dom_m1 {
                                    if current_minute != acc.minute_ts && acc.snapshots > 0 {
                                        let n = acc.snapshots as f64;
                                        let obi_mean = acc.obi_sum / n;
                                        let obi_std = ((acc.obi_sq_sum / n) - obi_mean * obi_mean)
                                            .max(0.0).sqrt();
                                        let bid_mean = acc.total_bid_vol_sum / n;
                                        let ask_mean = acc.total_ask_vol_sum / n;
                                        let ratio = if ask_mean > 0.0 { bid_mean / ask_mean } else { 1.0 };
                                        let total_events = (acc.quotes_added + acc.quotes_deleted) as f64;
                                        let churn = if n > 0.0 { total_events / n } else { 0.0 };

                                        let _ = dom_db_tx.try_send(DomDbCommand::M1Features {
                                            params: [
                                                acc.minute_ts as f64,
                                                obi_mean, obi_std,
                                                acc.spread_sum / n, acc.spread_max,
                                                bid_mean, ask_mean,
                                                if acc.total_bid_vol_min < f64::MAX { acc.total_bid_vol_min } else { 0.0 },
                                                if acc.total_ask_vol_min < f64::MAX { acc.total_ask_vol_min } else { 0.0 },
                                                ratio,
                                                acc.bid_levels_sum as f64 / n, acc.ask_levels_sum as f64 / n,
                                                churn,
                                                acc.quotes_added as f64, acc.quotes_deleted as f64,
                                                acc.best_bid_max_size as f64, acc.best_ask_max_size as f64,
                                                acc.snapshots as f64,
                                            ],
                                        });
                                        dom_m1 = None;
                                    }
                                }

                                // Initialize new minute accumulator if needed
                                if dom_m1.is_none() {
                                    dom_m1 = Some(DomM1Accum::new(current_minute));
                                }

                                // Snapshot current book into accumulator
                                if let Some(ref mut acc) = dom_m1 {
                                    acc.quotes_added += n_added;
                                    acc.quotes_deleted += n_deleted;

                                    // Use running totals for volume/levels (O(1))
                                    let total_bid = dom_total_bid_vol;
                                    let total_ask = dom_total_ask_vol;

                                    // Best bid/ask still need a scan — but only when quotes changed
                                    // This is unavoidable without a BTreeMap, but runs less often
                                    // than the old code (only when accumulator is active)
                                    let mut best_bid: i32 = 0;
                                    let mut best_ask: i32 = i32::MAX;
                                    let mut best_bid_size: i64 = 0;
                                    let mut best_ask_size: i64 = 0;
                                    for &(side, price, size) in dom_book.values() {
                                        if side == 0 {
                                            if price > best_bid { best_bid = price; best_bid_size = size; }
                                        } else {
                                            if price < best_ask { best_ask = price; best_ask_size = size; }
                                        }
                                    }

                                    // OBI = (bid_vol - ask_vol) / (bid_vol + ask_vol)
                                    let total = total_bid + total_ask;
                                    let obi = if total > 0.0 { (total_bid - total_ask) / total } else { 0.0 };

                                    // Spread in pips (price units are /100000)
                                    let spread = if best_ask < i32::MAX && best_bid > 0 {
                                        (best_ask - best_bid) as f64 / 10.0 // in pips
                                    } else {
                                        0.0
                                    };

                                    acc.snapshots += 1;
                                    acc.obi_sum += obi;
                                    acc.obi_sq_sum += obi * obi;
                                    acc.spread_sum += spread;
                                    if spread > acc.spread_max { acc.spread_max = spread; }
                                    acc.total_bid_vol_sum += total_bid;
                                    acc.total_ask_vol_sum += total_ask;
                                    if total_bid < acc.total_bid_vol_min { acc.total_bid_vol_min = total_bid; }
                                    if total_ask < acc.total_ask_vol_min { acc.total_ask_vol_min = total_ask; }
                                    acc.bid_levels_sum += dom_bid_levels;
                                    acc.ask_levels_sum += dom_ask_levels;
                                    if best_bid_size > acc.best_bid_max_size { acc.best_bid_max_size = best_bid_size; }
                                    if best_ask_size > acc.best_ask_max_size { acc.best_ask_max_size = best_ask_size; }
                                }

                                // Flush batch to DB writer every 2 seconds or when buffer is large
                                if dom_batch.len() >= 5_000
                                    || dom_last_flush.elapsed() > Duration::from_secs(2)
                                {
                                    let flushed = dom_batch.len() as u64;
                                    // Send batch to writer thread (non-blocking)
                                    let batch = std::mem::replace(
                                        &mut dom_batch,
                                        Vec::with_capacity(10_000),
                                    );
                                    let _ = dom_db_tx.try_send(DomDbCommand::RawBatch { rows: batch });
                                    dom_total_rows += flushed;
                                    dom_rows_since_status += flushed;
                                    dom_last_flush = tokio::time::Instant::now();

                                    // Send status update to UI every 5 seconds
                                    if dom_last_status_update.elapsed() > Duration::from_secs(5) {
                                        let elapsed_secs = dom_last_status_update.elapsed().as_secs_f64();
                                        let rows_per_sec = (dom_rows_since_status as f64 / elapsed_secs) as u64;
                                        let now_local = chrono::Local::now();
                                        let status = format!(
                                            "Active | {} rows/s | {} total | DB updated: {}",
                                            rows_per_sec,
                                            dom_total_rows,
                                            now_local.format("%H:%M:%S"),
                                        );
                                        let _ = tx.send(PriceUpdate::DomCaptureStatus(status)).await;
                                        dom_rows_since_status = 0;
                                        dom_last_status_update = tokio::time::Instant::now();
                                    }
                                }
                            }
                        }
                    },
                    2157 => { // ProtoOASubscribeDepthQuotesRes
                        println!("DoM: subscription confirmed");
                        let _ = tx.send(PriceUpdate::DomCaptureStatus(
                            "DoM capture active".to_string()
                        )).await;
                    },
                    2159 => { // ProtoOAUnsubscribeDepthQuotesRes
                        println!("DoM: unsubscribed");
                        let _ = tx.send(PriceUpdate::DomCaptureStatus(
                            "DoM capture paused".to_string()
                        )).await;
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
                // Handle DoM capture start/stop separately
                match request.action {
                    DataAction::DomCaptureStart => {
                        if let Some(eurusd_id) = dom_eurusd_id {
                            if !dom_capturing {
                                // Initialize DB writer (non-blocking: awaits reply from writer thread)
                                let (init_tx, init_rx) = tokio::sync::oneshot::channel();
                                let _ = dom_db_tx.send(DomDbCommand::Init { reply: init_tx }).await;
                                let (existing_rows, last_ts_str) = init_rx.await.unwrap_or((0, "0".to_string()));
                                dom_total_rows = existing_rows;
                                let last_ts_ms: i64 = last_ts_str.parse().unwrap_or(0);
                                let last_str = if last_ts_ms > 0 {
                                    chrono::DateTime::from_timestamp_millis(last_ts_ms)
                                        .map(|dt| dt.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S").to_string())
                                        .unwrap_or_default()
                                } else {
                                    "none".to_string()
                                };
                                println!("DoM: DB has {} existing rows, last: {}", dom_total_rows, last_str);

                                // Subscribe to DoM
                                let subscribe = openapi::ProtoOaSubscribeDepthQuotesReq {
                                    payload_type: Some(
                                        openapi::ProtoOaPayloadType::ProtoOaSubscribeDepthQuotesReq as i32
                                    ),
                                    ctid_trader_account_id: account_id,
                                    symbol_id: vec![eurusd_id],
                                };
                                send_message(
                                    &mut tls_stream,
                                    openapi::ProtoOaPayloadType::ProtoOaSubscribeDepthQuotesReq as u32,
                                    subscribe,
                                ).await?;
                                dom_capturing = true;
                                dom_book.clear();
                                dom_total_bid_vol = 0.0;
                                dom_total_ask_vol = 0.0;
                                dom_bid_levels = 0;
                                dom_ask_levels = 0;
                                dom_batch.clear();
                                dom_rows_since_status = 0;
                                dom_last_status_update = tokio::time::Instant::now();
                                let dom_start_msg = if last_ts_ms > 0 {
                                    format!("Starting... ({} existing, last: {})", dom_total_rows, last_str)
                                } else {
                                    format!("Starting... ({} existing)", dom_total_rows)
                                };
                                println!("DoM: subscribing to EURUSD depth (id={})", eurusd_id);
                                let _ = tx.send(PriceUpdate::DomCaptureStatus(dom_start_msg)).await;
                            }
                        } else {
                            let _ = tx.send(PriceUpdate::DomCaptureStatus(
                                "Error: EURUSD symbol_id not found".to_string()
                            )).await;
                        }
                        continue;
                    }
                    DataAction::EcCaptureStart => {
                        if !ec_capturing {
                            ec_capturing = true;
                            // Trigger initial fetch in 2s
                            ec_next_schedule_fetch = Some(
                                tokio::time::Instant::now() + Duration::from_secs(2)
                            );
                            ec_schedule_loaded = false;
                            ec_today_date = None; // force day-change detection
                            println!("EC: capture started by user");
                            let _ = tx.send(PriceUpdate::EcCaptureActive(true)).await;
                            let _ = tx.send(PriceUpdate::EcStatus("Starting...".to_string())).await;
                        }
                        continue;
                    }
                    DataAction::EcCaptureStop => {
                        if ec_capturing {
                            ec_capturing = false;
                            ec_scheduled_fetches.clear();
                            ec_schedule_loaded = false;
                            println!("EC: capture stopped by user");
                            let _ = tx.send(PriceUpdate::EcCaptureActive(false)).await;
                            let _ = tx.send(PriceUpdate::EcStatus("Stopped".to_string())).await;
                        }
                        continue;
                    }
                    DataAction::NewsCaptureStart => {
                        if !news_capturing {
                            news_capturing = true;
                            news_next_fetch = Some(
                                tokio::time::Instant::now() + Duration::from_secs(2)
                            );
                            news_today_date = None;
                            println!("News: capture started by user");
                            let _ = tx.send(PriceUpdate::NewsCaptureActive(true)).await;
                            let _ = tx.send(PriceUpdate::NewsStatus("Starting...".to_string())).await;
                        }
                        continue;
                    }
                    DataAction::NewsCaptureStop => {
                        if news_capturing {
                            news_capturing = false;
                            news_next_fetch = None;
                            println!("News: capture stopped by user");
                            let _ = tx.send(PriceUpdate::NewsCaptureActive(false)).await;
                            let _ = tx.send(PriceUpdate::NewsStatus("Stopped".to_string())).await;
                        }
                        continue;
                    }
                    DataAction::DomCaptureStop => {
                        if dom_capturing {
                            if let Some(eurusd_id) = dom_eurusd_id {
                                let unsubscribe = openapi::ProtoOaUnsubscribeDepthQuotesReq {
                                    payload_type: Some(
                                        openapi::ProtoOaPayloadType::ProtoOaUnsubscribeDepthQuotesReq as i32
                                    ),
                                    ctid_trader_account_id: account_id,
                                    symbol_id: vec![eurusd_id],
                                };
                                send_message(
                                    &mut tls_stream,
                                    openapi::ProtoOaPayloadType::ProtoOaUnsubscribeDepthQuotesReq as u32,
                                    unsubscribe,
                                ).await?;
                            }
                            // Flush remaining batch to writer
                            if !dom_batch.is_empty() {
                                let batch = std::mem::take(&mut dom_batch);
                                dom_total_rows += batch.len() as u64;
                                let _ = dom_db_tx.send(DomDbCommand::RawBatch { rows: batch }).await;
                            }
                            // Flush last M1 accumulator to writer
                            if let Some(ref acc) = dom_m1 {
                                if acc.snapshots > 0 {
                                    let n = acc.snapshots as f64;
                                    let obi_mean = acc.obi_sum / n;
                                    let obi_std = ((acc.obi_sq_sum / n) - obi_mean * obi_mean).max(0.0).sqrt();
                                    let bid_mean = acc.total_bid_vol_sum / n;
                                    let ask_mean = acc.total_ask_vol_sum / n;
                                    let ratio = if ask_mean > 0.0 { bid_mean / ask_mean } else { 1.0 };
                                    let churn = (acc.quotes_added + acc.quotes_deleted) as f64 / n;
                                    let _ = dom_db_tx.send(DomDbCommand::M1Features {
                                        params: [
                                            acc.minute_ts as f64, obi_mean, obi_std,
                                            acc.spread_sum / n, acc.spread_max,
                                            bid_mean, ask_mean,
                                            if acc.total_bid_vol_min < f64::MAX { acc.total_bid_vol_min } else { 0.0 },
                                            if acc.total_ask_vol_min < f64::MAX { acc.total_ask_vol_min } else { 0.0 },
                                            ratio, acc.bid_levels_sum as f64 / n, acc.ask_levels_sum as f64 / n,
                                            churn, acc.quotes_added as f64, acc.quotes_deleted as f64,
                                            acc.best_bid_max_size as f64, acc.best_ask_max_size as f64,
                                            acc.snapshots as f64,
                                        ],
                                    }).await;
                                }
                            }
                            // Close DB connection via writer (awaits final flush)
                            let (close_tx, close_rx) = tokio::sync::oneshot::channel();
                            let _ = dom_db_tx.send(DomDbCommand::Close { reply: close_tx }).await;
                            if let Ok(total) = close_rx.await {
                                dom_total_rows = total;
                            }
                            dom_m1 = None;
                            dom_capturing = false;
                            dom_book.clear();
                            dom_total_bid_vol = 0.0;
                            dom_total_ask_vol = 0.0;
                            dom_bid_levels = 0;
                            dom_ask_levels = 0;
                            println!("DoM: stopped. {} total rows written", dom_total_rows);
                            let _ = tx.send(PriceUpdate::DomCaptureStatus(
                                format!("DoM paused ({} rows saved)", dom_total_rows)
                            )).await;
                        }
                        continue;
                    }
                    _ => {}
                }
                println!("DEBUG: Received data request: {} {:?} {:?}", request.symbol, request.kind, request.action);
                handle_data_request(
                    &mut tls_stream,
                    &request,
                    account_id,
                    &response_tx,
                    &mut active_download,
                    &shared_db,
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

        // ── EC Calendar smart scheduling ─────────────────────────────────
        if _auth_state == AuthState::Subscribed && ec_capturing {
            let now_utc = chrono::Utc::now();
            let now_ts = now_utc.timestamp();
            let today_str = now_utc.format("%Y%m%d").to_string();

            // Detect day change → reset schedule
            if ec_today_date.as_deref() != Some(&today_str) {
                println!("EC: new day ({}), resetting schedule", today_str);
                ec_today_date = Some(today_str.clone());
                ec_scheduled_fetches.clear();
                ec_schedule_loaded = false;
                ec_next_schedule_fetch = Some(
                    tokio::time::Instant::now() + Duration::from_secs(5)
                );
            }

            // Check if it's time for the schedule fetch (day start)
            let should_fetch_schedule = match ec_next_schedule_fetch {
                Some(t) if tokio::time::Instant::now() >= t => {
                    ec_next_schedule_fetch = None;
                    true
                }
                _ => false,
            };

            // Check if any scheduled event fetch is due
            let should_fetch_event = !ec_scheduled_fetches.is_empty()
                && now_ts >= ec_scheduled_fetches[0]
                && now_ts != ec_last_fetch_ts;

            if should_fetch_schedule || should_fetch_event {
                // Remove past scheduled fetches
                while !ec_scheduled_fetches.is_empty() && ec_scheduled_fetches[0] <= now_ts {
                    ec_scheduled_fetches.remove(0);
                }
                ec_last_fetch_ts = now_ts;

                // For the schedule fetch, do it inline (awaited) so we can build the schedule
                // For event fetches, also inline — it's one quick HTTP call
                match ec_realtime::fetch_events_for_date(&today_str, &today_str).await {
                    Ok(rows) => {
                        let count = rows.len();

                        // Build schedule on first fetch of the day
                        if should_fetch_schedule && !ec_schedule_loaded {
                            let mut unique_times = std::collections::BTreeSet::new();
                            for r in &rows {
                                if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(
                                    &r.timestamp_utc, "%Y-%m-%dT%H:%M:%S"
                                ) {
                                    let event_ts = dt.and_utc().timestamp();
                                    // First 3 min: every 30s, then final at 5 min
                                    for offset in [0i64, 30, 60, 90, 120, 150, 180, 300] {
                                        let t = event_ts + offset;
                                        if t > now_ts {
                                            unique_times.insert(t);
                                        }
                                    }
                                }
                            }
                            ec_scheduled_fetches = unique_times.into_iter().collect();
                            ec_schedule_loaded = true;
                            println!(
                                "EC: scheduled {} fetches for {} events today",
                                ec_scheduled_fetches.len(), count
                            );
                            if let Some(&next_ts) = ec_scheduled_fetches.first() {
                                if let Some(dt) = chrono::DateTime::from_timestamp(next_ts, 0) {
                                    println!("EC: next fetch at {}",
                                        dt.with_timezone(&chrono::Local).format("%H:%M:%S"));
                                }
                            }
                        }

                        // Write to DB and read raw data (blocking)
                        let db_clone = shared_db.clone();
                        let (lines, raw) = tokio::task::spawn_blocking(move || {
                            let _lock = db_clone.lock().unwrap();
                            match duckdb::Connection::open(DB_PATH) {
                                Ok(db) => {
                                    let _ = ec_realtime::write_ec_to_db(&db, &rows);
                                    let raw = ec_realtime::read_ec_today_raw(&db);
                                    let lines = ec_realtime::format_ec_lines(&raw);
                                    (lines, raw)
                                }
                                Err(e) => (vec![format!("DB error: {}", e)], Vec::new()),
                            }
                        })
                        .await
                        .unwrap_or_else(|e| (vec![format!("Task error: {}", e)], Vec::new()));

                        let upcoming = lines.iter().filter(|l| l.contains('>')).count();
                        let next_fetch = ec_scheduled_fetches.first().and_then(|&t| {
                            chrono::DateTime::from_timestamp(t, 0).map(|dt|
                                dt.with_timezone(&chrono::Local).format("%H:%M").to_string()
                            )
                        }).unwrap_or_else(|| "done".to_string());
                        let now_str = chrono::Local::now().format("%H:%M:%S").to_string();

                        let _ = tx.send(PriceUpdate::EcStatus(format!(
                            "{} events ({} upcoming) | next: {} | updated: {}",
                            count, upcoming, next_fetch, now_str
                        ))).await;
                        let _ = tx.send(PriceUpdate::EcTodayEvents(lines)).await;
                        if !raw.is_empty() {
                            let _ = tx.send(PriceUpdate::EcTodayRaw(raw)).await;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(PriceUpdate::EcStatus(format!("EC error: {}", e))).await;
                        // Retry in 15s if the proxy isn't ready yet
                        if !ec_schedule_loaded {
                            ec_next_schedule_fetch = Some(
                                tokio::time::Instant::now() + Duration::from_secs(15)
                            );
                            println!("EC: fetch failed ({}), retrying in 15s...", e);
                        }
                    }
                }
            }
        }

        // ── News periodic fetching ───────────────────────────────────────
        if _auth_state == AuthState::Subscribed && news_capturing {
            let now_utc = chrono::Utc::now();
            let today_str = now_utc.format("%Y%m%d").to_string();

            // Detect day change → force immediate fetch
            if news_today_date.as_deref() != Some(&today_str) {
                println!("News: new day ({}), resetting", today_str);
                news_today_date = Some(today_str.clone());
                news_next_fetch = Some(
                    tokio::time::Instant::now() + Duration::from_secs(3)
                );
            }

            let should_fetch = match news_next_fetch {
                Some(t) if tokio::time::Instant::now() >= t => true,
                _ => false,
            };

            if should_fetch {
                // Schedule next fetch in 10 minutes
                news_next_fetch = Some(
                    tokio::time::Instant::now() + Duration::from_secs(600)
                );

                match news_realtime::fetch_news(50, 1, None).await {
                    Ok(rows) => {
                        let total = rows.len();

                        let db_clone = shared_db.clone();
                        let lines = tokio::task::spawn_blocking(move || {
                            let _lock = db_clone.lock().unwrap();
                            match duckdb::Connection::open(DB_PATH) {
                                Ok(db) => {
                                    let _ = news_realtime::write_news_to_db(&db, &rows);
                                    let today_rows = news_realtime::read_news_today(&db);
                                    news_realtime::format_news_lines(&today_rows)
                                }
                                Err(e) => vec![format!("DB error: {}", e)],
                            }
                        })
                        .await
                        .unwrap_or_else(|e| vec![format!("Task error: {}", e)]);

                        let now_str = chrono::Local::now().format("%H:%M:%S").to_string();
                        let _ = tx.send(PriceUpdate::NewsStatus(format!(
                            "{} articles | next: 10m | updated: {}",
                            total, now_str
                        ))).await;
                        let _ = tx.send(PriceUpdate::NewsTodayLines(lines)).await;
                    }
                    Err(e) => {
                        let _ = tx.send(PriceUpdate::NewsStatus(format!("News error: {}", e))).await;
                        // Retry sooner if proxy isn't ready
                        news_next_fetch = Some(
                            tokio::time::Instant::now() + Duration::from_secs(15)
                        );
                        println!("News: fetch failed ({}), retrying in 15s...", e);
                    }
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
    shared_db: &SharedDb,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let table_name = get_data_table_name(&request.symbol, request.kind);

    match request.action {
        DataAction::CheckStatus => {
            check_db_status(request, &table_name, response_tx, shared_db).await;
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
            let from_ms = get_newest_timestamp_in_db(&table_name, request.kind, shared_db);

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
            let db_for_merge = shared_db.clone();
            let merge_result = tokio::task::spawn_blocking(move || {
                let _lock = db_for_merge.lock().unwrap();
                let db = CandleDatabase::new(DB_PATH)?;

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
            let db_for_ml = shared_db.clone();
            let ml_result = tokio::task::spawn_blocking(move || {
                let _lock = db_for_ml.lock().unwrap();
                let db = CandleDatabase::new(DB_PATH)?;

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
                    let _lock = shared_db.lock().unwrap();
                    let db2 = CandleDatabase::new(DB_PATH);
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
        // DoM capture actions are handled inline in the select! loop, not here
        DataAction::DomCaptureStart | DataAction::DomCaptureStop
        | DataAction::EcCaptureStart | DataAction::EcCaptureStop
        | DataAction::NewsCaptureStart | DataAction::NewsCaptureStop => {}
    }
    Ok(())
}

/// Check DuckDB for existing data and send status response
async fn check_db_status(
    request: &DataRequest,
    table_name: &str,
    response_tx: &mpsc::Sender<DataResponse>,
    shared_db: &SharedDb,
) {
    let _lock = shared_db.lock().unwrap();
    match CandleDatabase::new(DB_PATH) {
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
fn get_newest_timestamp_in_db(table_name: &str, kind: DataKind, shared_db: &SharedDb) -> i64 {
    let _lock = shared_db.lock().unwrap();
    match CandleDatabase::new(DB_PATH) {
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
    shared_db: &SharedDb,
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
        let db_for_load = shared_db.clone();
        let load_result = tokio::task::spawn_blocking(move || {
            let _lock = db_for_load.lock().unwrap();
            let db = CandleDatabase::new(DB_PATH)?;
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
    shared_db: &SharedDb,
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
        let db_for_load = shared_db.clone();
        let load_result = tokio::task::spawn_blocking(move || {
            let _lock = db_for_load.lock().unwrap();
            let db = CandleDatabase::new(DB_PATH)?;
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
