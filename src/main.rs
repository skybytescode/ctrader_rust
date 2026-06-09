use prost::Message;
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

const DB_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/Bots_db/xauusd.duckdb");

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
pub mod db;
pub mod data_retrieval;
pub mod ec_realtime;
pub mod news_realtime;
pub mod news_sentiment;
pub mod myfxbook_cal;
pub mod myfxbook_news;
pub mod forexfactory;
pub mod volume_profile;
pub mod strategy;

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
    /// News today's articles for UI display (structured, includes body when fetched).
    NewsTodayArticles(Vec<news_realtime::NewsRow>),
    /// News status message
    NewsStatus(String),
    /// MyFXBook calendar today's events for UI display.
    MfbTodayRaw(Vec<myfxbook_cal::MfbTodayRow>),
    /// MyFXBook calendar status message.
    MfbStatus(String),
    /// MyFXBook news/analysis/press-release items for UI display.
    MfbNewsToday(Vec<myfxbook_news::MfbNewsRow>),
    /// MyFXBook news status message.
    MfbNewsStatus(String),
    /// ForexFactory calendar today's events.
    FfCalToday(Vec<forexfactory::FfCalRow>),
    FfCalStatus(String),
    /// ForexFactory news items.
    FfNewsToday(Vec<forexfactory::FfNewsRow>),
    FfNewsStatus(String),
    /// News capture status (for the button)
    NewsCaptureActive(bool),
    /// Full snapshot of open positions + pending orders (pushed on reconcile and
    /// after every execution event). JSON: { positions: [...], orders: [...] }.
    PositionsSnapshot(serde_json::Value),
    /// A one-off trade notice (e.g. SL/TP hit, position closed) for a UI banner.
    TradeNotice(serde_json::Value),
    /// Auto-trade loop status for the UI: { enabled, oz, status }.
    AutoStatus(serde_json::Value),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum AuthState {
    NotAuthenticated,
    AppAuthenticated,
    AccountAuthenticated,
    SymbolsRequested,
    Subscribed,
}

/// PIDs of spawned child processes that should be killed when the app exits.
/// 0 = slot not in use.
static ECONCAL_PID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static VITE_PID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Latest live XAUUSD bid, stored as f64 bits (0 = none yet). Updated on every
/// tick in the WS bridge so commands like the Trade Idea can read a true
/// current price instead of relying on the last (up-to-5-min-old) M5 close.
static LATEST_XAUUSD_BID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Latest broadcast JSON message keyed by `type` field. Sent to every newly-connected
/// WS client so a client joining after a one-shot event (e.g. ec_today) still sees
/// the current state instead of waiting for the next fetch.
static SNAPSHOT_CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>>
    = std::sync::OnceLock::new();

fn snapshot_cache() -> &'static std::sync::Mutex<std::collections::HashMap<String, String>> {
    SNAPSHOT_CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

// ── Chart trendbar requests ─────────────────────────────────────────────────

/// A request from a Tauri command to the cTrader session for historical
/// trendbars. The session generates a unique `client_msg_id`, sends the
/// ProtoOAGetTrendbarsReq, and replies via the oneshot when the response arrives.
pub struct ChartRequest {
    pub symbol_id: i64,
    pub period: openapi::ProtoOaTrendbarPeriod,
    pub from_ms: i64,
    pub to_ms: i64,
    pub count: u32,
    pub reply: tokio::sync::oneshot::Sender<Result<Vec<db::Candle>, String>>,
}

/// Global sender, populated in `main()` so any Tauri command can push a
/// ChartRequest to the session loop.
static CHART_REQ_TX: std::sync::OnceLock<tokio::sync::mpsc::Sender<ChartRequest>>
    = std::sync::OnceLock::new();

// ── Order placement requests ────────────────────────────────────────────────

/// A request from a Tauri command to place a LIVE market order. The session
/// generates a unique `client_msg_id`, sends ProtoOANewOrderReq, and replies via
/// the oneshot when the ExecutionEvent / error arrives.
pub struct OrderRequest {
    pub symbol_id: i64,
    pub trade_side: openapi::ProtoOaTradeSide,
    pub order_type: openapi::ProtoOaOrderType,
    /// Volume in cTrader cents (0.01 of a unit). For XAUUSD: oz × 100.
    pub volume: i64,
    /// Entry price for a LIMIT order (None for MARKET / STOP).
    pub limit_price: Option<f64>,
    /// Entry price for a STOP order (None for MARKET / LIMIT).
    pub stop_price: Option<f64>,
    /// Absolute SL / TP prices — used by pending (LIMIT/STOP) orders.
    pub stop_loss: Option<f64>,
    pub take_profit: Option<f64>,
    /// Relative SL / TP in 1/100000 of a price unit — used by MARKET orders
    /// (which don't accept absolute SL/TP).
    pub rel_sl: Option<i64>,
    pub rel_tp: Option<i64>,
    pub label: String,
    pub reply: tokio::sync::oneshot::Sender<Result<String, String>>,
}

static ORDER_REQ_TX: std::sync::OnceLock<tokio::sync::mpsc::Sender<OrderRequest>>
    = std::sync::OnceLock::new();

/// A request to cancel a resting pending order. Reply carries the broker's
/// outcome string (e.g. "ORDER_CANCELLED") or an error.
pub struct CancelRequest {
    pub order_id: i64,
    pub reply: tokio::sync::oneshot::Sender<Result<String, String>>,
}

static CANCEL_REQ_TX: std::sync::OnceLock<tokio::sync::mpsc::Sender<CancelRequest>>
    = std::sync::OnceLock::new();

/// An action on an existing open position (close, or amend SL/TP).
pub enum PositionAction {
    Close { position_id: i64, volume: i64 },
    AmendSltp { position_id: i64, stop_loss: Option<f64>, take_profit: Option<f64> },
}
pub struct PositionActionRequest {
    pub action: PositionAction,
    pub reply: tokio::sync::oneshot::Sender<Result<String, String>>,
}

static POS_ACTION_TX: std::sync::OnceLock<tokio::sync::mpsc::Sender<PositionActionRequest>>
    = std::sync::OnceLock::new();

// ── Auto-trade ──────────────────────────────────────────────────────────────

/// Whether the auto-trade loop is active.
static AUTO_TRADE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Fixed size (oz) per auto trade.
static AUTO_OZ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

/// Current XAUUSD account state, refreshed by the session on every reconcile so
/// the auto-trade loop can tell whether we're flat / pending / in a position.
#[derive(Default, Clone)]
pub struct XauAccountState {
    pub positions: usize,
    pub pending_order_ids: Vec<i64>,
}
static XAU_ACCOUNT_STATE: std::sync::OnceLock<std::sync::Mutex<XauAccountState>>
    = std::sync::OnceLock::new();
fn xau_account_state() -> &'static std::sync::Mutex<XauAccountState> {
    XAU_ACCOUNT_STATE.get_or_init(|| std::sync::Mutex::new(XauAccountState::default()))
}

/// Cached trading spec for XAUUSD, fetched once via ProtoOASymbolByIdReq after
/// the symbols list arrives. Needed to size and price orders safely — orders are
/// refused if this is absent.
#[derive(Clone, Copy, Debug)]
pub struct SymbolSpec {
    pub symbol_id: i64,
    pub digits: i32,
    pub min_volume: i64,
    pub step_volume: i64,
    pub max_volume: i64,
}

static XAUUSD_SPEC: std::sync::OnceLock<std::sync::Mutex<Option<SymbolSpec>>>
    = std::sync::OnceLock::new();

fn xauusd_spec() -> &'static std::sync::Mutex<Option<SymbolSpec>> {
    XAUUSD_SPEC.get_or_init(|| std::sync::Mutex::new(None))
}

/// Cache of symbol name → cTrader symbol_id, populated by the bridge task each
/// time the session emits a SymbolMapping. Tauri commands resolve names here.
static SYMBOL_MAP: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, i64>>>
    = std::sync::OnceLock::new();

fn symbol_map() -> &'static std::sync::Mutex<std::collections::HashMap<String, i64>> {
    SYMBOL_MAP.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Snapshot of the background history-backfill loop's current progress.
/// The loop writes into this on every chunk so the Archives tab can poll it
/// and render a live status panel without needing Tauri events plumbed
/// through to the tokio thread that runs the loop.
#[derive(Clone, serde::Serialize, Default)]
pub struct BackfillState {
    /// "idle" | "running" | "complete" | "error"
    pub status: String,
    pub started_at_utc: Option<String>,
    pub completed_at_utc: Option<String>,
    /// Currently-active TF name (e.g. "m1"); None when idle / complete.
    pub current_tf: Option<String>,
    /// Mode for the current TF: "first-build" | "fill-forward" | "extend-back" | "full-walk".
    pub current_mode: Option<String>,
    /// Chunks fetched + bars upserted for the current TF only.
    pub chunks_this_tf: usize,
    pub bars_this_tf: usize,
    /// Lifetime totals across the whole backfill run.
    pub total_bars: usize,
    /// ISO timestamp of the oldest bar in the last chunk we wrote (so the UI
    /// can show "currently walking back through 2024-03-…" type context).
    pub last_chunk_oldest_utc: Option<String>,
    /// TFs the loop finished cleanly (in order of completion).
    pub tfs_completed: Vec<String>,
    /// TFs the loop skipped because they were already complete on startup.
    pub tfs_skipped: Vec<String>,
    /// Total expected TFs (currently always 14).
    pub tfs_total: usize,
    /// Last error message if status == "error".
    pub last_error: Option<String>,
}

static BACKFILL_STATE: std::sync::OnceLock<std::sync::Mutex<BackfillState>> = std::sync::OnceLock::new();

fn backfill_state() -> &'static std::sync::Mutex<BackfillState> {
    BACKFILL_STATE.get_or_init(|| std::sync::Mutex::new(BackfillState::default()))
}

/// Mutate the global backfill state. Cheap — only locks briefly.
fn with_backfill_state<F>(f: F) where F: FnOnce(&mut BackfillState) {
    if let Ok(mut s) = backfill_state().lock() { f(&mut s); }
}

/// Tauri command: snapshot of the current backfill state for the UI panel.
#[tauri::command]
fn get_history_backfill_state() -> BackfillState {
    backfill_state().lock().ok().map(|s| s.clone()).unwrap_or_default()
}

/// Tauri command behind the `XAUUSD_History_Update` button. Spawns the
/// history backfill in the background (fire-and-forget). The Current
/// fetching state panel polls progress in real time. Returns immediately
/// with a small status string so the UI can disable the button while busy.
///
/// Works for both first-time bootstrap and subsequent incremental updates —
/// the inner `run_history_backfill` already detects which TFs are complete,
/// stale-tail, or missing data and picks the right mode per TF.
#[tauri::command]
async fn start_history_backfill(state: tauri::State<'_, AppState>) -> Result<String, String> {
    // Refuse to start a second backfill if one is already running.
    if let Ok(s) = backfill_state().lock() {
        if s.status == "running" {
            return Ok("already running".to_string());
        }
    }
    // CHART_REQ_TX is set up by the cTrader session loop on startup. If it's
    // missing here, the session hasn't authenticated yet.
    if CHART_REQ_TX.get().is_none() {
        return Err("cTrader session not ready yet — wait a few seconds and try again.".into());
    }

    let symbol = std::env::var("CTRADER_SYMBOL").unwrap_or_else(|_| "XAUUSD".to_string());
    let db = state.db_mutex.clone();
    tokio::spawn(async move {
        run_history_backfill(symbol, db).await;
    });
    Ok("started".to_string())
}

// ── Archives tab: backend (ported from origin/web_gold) ──────────────────────

/// Root directory for per-day news archive JSON files.
/// Layout: news_data/all/YYYY-MM/YYYY-MM-DD.json
const ARCHIVE_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/news_data/all");

/// Walk `news_data/all/YYYY-MM/` folders to find the newest `YYYY-MM-DD.json`
/// file. Returns `(day, max_published_utc)` where `day` is "YYYY-MM-DD" and
/// `max_published_utc` is the latest article timestamp in that file ("YYYY-MM-DDTHH:MM:SS").
/// Returns `(None, None)` if no archive files exist.
fn find_newest_archive_cutoff() -> (Option<String>, Option<String>) {
    let root = std::path::Path::new(ARCHIVE_ROOT);
    if !root.exists() { return (None, None); }

    // Find the lexically-largest YYYY-MM-DD.json across all YYYY-MM subdirs.
    let mut newest_path: Option<std::path::PathBuf> = None;
    let mut newest_day: Option<String> = None;
    let entries = match std::fs::read_dir(root) { Ok(e) => e, Err(_) => return (None, None) };
    for month_entry in entries.flatten() {
        let month_path = month_entry.path();
        if !month_path.is_dir() { continue; }
        let day_entries = match std::fs::read_dir(&month_path) { Ok(e) => e, Err(_) => continue };
        for day_entry in day_entries.flatten() {
            let p = day_entry.path();
            let name = match p.file_name().and_then(|s| s.to_str()) { Some(s) => s, None => continue };
            // Expect "YYYY-MM-DD.json", 15 chars total.
            if name.len() != 15 || !name.ends_with(".json") { continue; }
            let day = &name[..10];
            if newest_day.as_deref().map(|d| day > d).unwrap_or(true) {
                newest_day = Some(day.to_string());
                newest_path = Some(p);
            }
        }
    }

    let (day, path) = match (newest_day, newest_path) {
        (Some(d), Some(p)) => (d, p),
        _ => return (None, None),
    };

    // Parse the file and pull max(article.published_utc).
    let max_pub = (|| -> Option<String> {
        let content = std::fs::read_to_string(&path).ok()?;
        let json: serde_json::Value = serde_json::from_str(&content).ok()?;
        let articles = json.get("articles")?.as_array()?;
        articles.iter()
            .filter_map(|a| a.get("published_utc")?.as_str().map(String::from))
            .max()
    })();

    (Some(day), max_pub)
}

/// Read all articles for `day` from `news_historical` and write the per-day
/// archive file at `news_data/all/YYYY-MM/YYYY-MM-DD.json`. Preserves existing
/// bodies stored in the DB. Returns `(absolute_path, article_count)`.
fn write_archive_for_day(db: &duckdb::Connection, day: &str) -> Result<(String, usize), String> {
    let mut stmt = db.prepare(
        "SELECT article_id, title, published_utc, summary, url, author, tags, hour_utc, weekday, body
         FROM news_historical
         WHERE published_utc LIKE ? || '%'
         ORDER BY published_utc ASC"
    ).map_err(|e| format!("prepare archive query: {}", e))?;

    let rows = stmt.query_map([day], |row| {
        let body_val: Option<String> = row.get(9).ok().flatten();
        Ok(serde_json::json!({
            "article_id":    row.get::<_, String>(0)?,
            "title":         row.get::<_, String>(1)?,
            "published_utc": row.get::<_, String>(2)?,
            "summary":       row.get::<_, Option<String>>(3)?.unwrap_or_default(),
            "url":           row.get::<_, Option<String>>(4)?.unwrap_or_default(),
            "author":        row.get::<_, Option<String>>(5)?.unwrap_or_default(),
            "tags":          row.get::<_, Option<String>>(6)?.unwrap_or_default(),
            "hour_utc":      row.get::<_, i32>(7)?,
            "weekday":       row.get::<_, i32>(8)?,
            "body":          body_val,
        }))
    }).map_err(|e| format!("query archive: {}", e))?;

    let articles: Vec<serde_json::Value> = rows.flatten().collect();
    let count = articles.len();

    let month = &day[..7];
    let dir = std::path::Path::new(ARCHIVE_ROOT).join(month);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create dir {}: {}", dir.display(), e))?;
    let path = dir.join(format!("{}.json", day));

    let payload = serde_json::json!({
        "date":         day,
        "count":        count,
        "generated_at": chrono::Utc::now().to_rfc3339(),
        "articles":     articles,
    });
    let pretty = serde_json::to_string_pretty(&payload).map_err(|e| format!("serialize: {}", e))?;
    std::fs::write(&path, pretty).map_err(|e| format!("write {}: {}", path.display(), e))?;

    Ok((path.to_string_lossy().to_string(), count))
}

#[derive(serde::Serialize)]
struct NewsUpdateResult {
    /// "YYYY-MM-DD" of the most recent archive file before the run, or null if
    /// no archive files existed.
    last_archive_day: Option<String>,
    /// ISO timestamp used as the lower bound when calling fetch_news_since
    /// (latest article time in the last archive file, or 7 days ago as fallback).
    cutoff: String,
    /// Number of new articles pulled from FXStreet.
    articles_fetched: usize,
    /// Article bodies successfully fetched and written to news_historical.body.
    bodies_fetched: usize,
    /// Articles where FXStreet's API returned an empty body (data flashes).
    /// Stored as '' so we don't retry.
    bodies_empty: usize,
    /// Body fetches that failed (timeout / parse error / proxy error).
    /// Left as NULL so a later run can retry.
    bodies_failed: usize,
    /// True if FXStreet returned HTTP 429 during the body fetch loop.
    /// Body fetching stops at the first 429 and the message field explains.
    rate_limited: bool,
    /// (day, article_count) for every day whose file was (re)written.
    days_written: Vec<(String, usize)>,
    /// Optional human-readable error or info message.
    message: Option<String>,
}

/// Tauri command: incremental archive update.
///
/// 1. Walks `news_data/all/` to find the newest YYYY-MM-DD.json
/// 2. Reads its latest `published_utc`
/// 3. Calls `fetch_news_since(cutoff)` to pull every newer article from FXStreet
/// 4. Upserts them into `news_historical`
/// 5. For every article in the affected days that still has `body IS NULL`,
///    fetches the body via the per-article proxy endpoint and updates
///    `news_historical.body` (concurrency 1 — the puppeteer proxy serialises
///    requests internally and >1 confuses its shared page state)
/// 6. (Re)writes one JSON file per affected day under `news_data/all/` —
///    pulling the now-populated bodies along with everything else
///
/// When no archive files exist yet, the cutoff defaults to 7 days before "now"
/// so the first run does a sensible bootstrap rather than crawling the full feed.
#[tauri::command]
async fn update_news_archive(state: tauri::State<'_, AppState>) -> Result<NewsUpdateResult, String> {
    run_news_archive_update(state.db_mutex.clone()).await
}

/// Guard so the auto-archive (driven by the 5-min news loop) never overlaps a
/// still-running cycle — body backfill is rate-limited and can outlast 5 min.
static NEWS_ARCHIVE_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

// Guards so the external (MyFXBook / ForexFactory) web fetches — which run OFF
// the cTrader session loop so a slow/hung fetch never stalls tick processing —
// never overlap themselves.
static MFB_CAL_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static MFB_NEWS_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static FF_CAL_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static FF_NEWS_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static FF_NEWS_BODY_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The News_Updates cycle as a reusable function. Driven both by the manual
/// button (`update_news_archive`) and automatically by the 5-min news loop:
/// find the archive cutoff, fetch newer FXStreet articles, upsert into
/// `news_historical`, backfill missing article bodies, and (re)write the
/// per-day JSON files under `news_data/all/`.
async fn run_news_archive_update(db_mutex: SharedDb) -> Result<NewsUpdateResult, String> {
    // Step 1: find newest archive day + its latest article time.
    let (last_archive_day, latest_in_file) =
        tokio::task::spawn_blocking(find_newest_archive_cutoff)
            .await
            .map_err(|e| format!("scan task join: {}", e))?;

    // Cutoff: the latest article time in the newest file, or 7 days ago if no
    // archive exists yet. fetch_news_since stops walking once it sees articles
    // <= cutoff, so this is also our "how far back do we crawl" bound.
    let cutoff = match latest_in_file.clone() {
        Some(t) => t,
        None => {
            let seven_days_ago = chrono::Utc::now() - chrono::Duration::days(7);
            seven_days_ago.format("%Y-%m-%dT%H:%M:%S").to_string()
        }
    };

    // Step 2: make sure econcal is reachable (we need it for both the news
    // listing fetch AND the per-article body fetch).
    if !ensure_econcal_alive().await {
        return Ok(NewsUpdateResult {
            last_archive_day, cutoff,
            articles_fetched: 0,
            bodies_fetched: 0, bodies_empty: 0, bodies_failed: 0,
            rate_limited: false,
            days_written: Vec::new(),
            message: Some("Econcal proxy not reachable on :6000. Restart the app and retry.".into()),
        });
    }

    // Step 3: fetch newer articles from FXStreet.
    let rows = match news_realtime::fetch_news_since(50, 200, Some(&cutoff), None).await {
        Ok(r) => r,
        Err(e) => return Ok(NewsUpdateResult {
            last_archive_day, cutoff,
            articles_fetched: 0,
            bodies_fetched: 0, bodies_empty: 0, bodies_failed: 0,
            rate_limited: false,
            days_written: Vec::new(),
            message: Some(format!("Fetch failed: {}", e)),
        }),
    };
    let articles_fetched = rows.len();

    // Step 4: upsert into news_historical + compute affected days.
    let affected_days: std::collections::BTreeSet<String> = rows.iter()
        .filter_map(|r| if r.published_utc.len() >= 10 { Some(r.published_utc[..10].to_string()) } else { None })
        .collect();

    {
        let db_mutex = db_mutex.clone();
        let rows_for_db = rows.clone();
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let _lock = db_mutex.lock().map_err(|e| format!("db lock poisoned: {}", e))?;
            let db = duckdb::Connection::open(DB_PATH).map_err(|e| format!("open db: {}", e))?;
            if !rows_for_db.is_empty() {
                news_realtime::write_news_to_db(&db, &rows_for_db)
                    .map_err(|e| format!("write_news_to_db: {}", e))?;
            }
            Ok(())
        })
        .await
        .map_err(|e| format!("upsert task join: {}", e))??;
    }

    // Step 5: list every article in the affected days that still has no body.
    // We include OLD articles (not just the ones we just inserted) so an
    // article that landed in DB via the live 5-min fetch — and never got a
    // body — gets one on this click too.
    let affected_for_query: Vec<String> = affected_days.iter().cloned().collect();
    let to_fetch: Vec<String> = {
        let db_mutex = db_mutex.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<String>, String> {
            let _lock = db_mutex.lock().map_err(|e| format!("db lock poisoned: {}", e))?;
            let db = duckdb::Connection::open(DB_PATH).map_err(|e| format!("open db: {}", e))?;
            let mut ids = Vec::new();
            for day in &affected_for_query {
                let mut stmt = db.prepare(
                    "SELECT article_id FROM news_historical
                     WHERE published_utc LIKE ? || '%'
                       AND url IS NOT NULL AND url <> ''
                       AND body IS NULL"
                ).map_err(|e| format!("prepare body-list: {}", e))?;
                let day_ids: Vec<String> = stmt.query_map([day.as_str()], |row| row.get::<_, String>(0))
                    .map_err(|e| format!("query body-list: {}", e))?
                    .flatten().collect();
                ids.extend(day_ids);
            }
            Ok(ids)
        })
        .await
        .map_err(|e| format!("body-list join: {}", e))??
    };

    // Step 6: fetch bodies sequentially. Bail at the first 429.
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36")
        .build()
        .map_err(|e| format!("build http client: {}", e))?;

    let mut fetched_bodies: Vec<(String, String)> = Vec::new();
    let mut empty_body_ids: Vec<String> = Vec::new();
    let mut failed_body_ids: Vec<String> = Vec::new();
    let mut rate_limited = false;
    let mut max_retry_after = 0u64;

    for aid in &to_fetch {
        if rate_limited { break; }
        match fetch_one_body(&client, aid).await {
            BodyFetch::Ok(b)         => fetched_bodies.push((aid.clone(), b)),
            BodyFetch::EmptyBody     => empty_body_ids.push(aid.clone()),
            BodyFetch::Failed        => failed_body_ids.push(aid.clone()),
            BodyFetch::RateLimited { retry_after_secs } => {
                rate_limited = true;
                max_retry_after = retry_after_secs;
            }
        }
    }

    let bodies_fetched = fetched_bodies.len();
    let bodies_empty   = empty_body_ids.len();
    let bodies_failed  = failed_body_ids.len();

    // Step 7: write fetched + empty body markers back to DB.
    if !fetched_bodies.is_empty() || !empty_body_ids.is_empty() {
        let db_mutex = db_mutex.clone();
        let fetched_for_db = fetched_bodies.clone();
        let empty_for_db = empty_body_ids.clone();
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let _lock = db_mutex.lock().map_err(|e| format!("db lock poisoned: {}", e))?;
            let db = duckdb::Connection::open(DB_PATH).map_err(|e| format!("open db: {}", e))?;
            for (aid, body) in &fetched_for_db {
                let _ = db.execute(
                    "UPDATE news_historical SET body = ? WHERE article_id = ?",
                    duckdb::params![body, aid],
                );
            }
            for aid in &empty_for_db {
                let _ = db.execute(
                    "UPDATE news_historical SET body = '' WHERE article_id = ?",
                    duckdb::params![aid],
                );
            }
            Ok(())
        })
        .await
        .map_err(|e| format!("body-write join: {}", e))??;
    }

    // Step 8: regenerate each affected day's JSON file from DB.
    let days_written: Vec<(String, usize)> = {
        let db_mutex = db_mutex.clone();
        let affected_for_write = affected_days.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<(String, usize)>, String> {
            let _lock = db_mutex.lock().map_err(|e| format!("db lock poisoned: {}", e))?;
            let db = duckdb::Connection::open(DB_PATH).map_err(|e| format!("open db: {}", e))?;
            let mut written = Vec::with_capacity(affected_for_write.len());
            for day in &affected_for_write {
                let (_path, count) = write_archive_for_day(&db, day)?;
                written.push((day.clone(), count));
            }
            Ok(written)
        })
        .await
        .map_err(|e| format!("write-days join: {}", e))??
    };

    let message = if rate_limited {
        Some(format!(
            "FXStreet rate-limited after {} bodies (Retry-After ~{}s). \
             {} bodies still missing — click News_Updates again in {} minute(s) to resume.",
            bodies_fetched + bodies_empty,
            max_retry_after,
            to_fetch.len() - bodies_fetched - bodies_empty,
            (max_retry_after + 59) / 60,
        ))
    } else {
        None
    };

    Ok(NewsUpdateResult {
        last_archive_day,
        cutoff,
        articles_fetched,
        bodies_fetched,
        bodies_empty,
        bodies_failed,
        rate_limited,
        days_written,
        message,
    })
}



// ── Gold EC events: one-shot table create + walk to today ──────────────────────

/// 30-day windows are large enough for one FXStreet API call but small
/// enough that the response stays well under a few MB. Each chunk commits
/// independently, so interrupting the walk never loses more than a chunk.
const EC_GOLD_CHUNK_DAYS: i64 = 30;

/// Earliest date FXStreet's `/v4/eventdate/mini` endpoint reliably returns
/// data for. Matches the floor of the existing eurusd_economic_calendar
/// (oldest row there is 2009-01-02).
const EC_GOLD_START_DATE: &str = "2009-01-01";

/// On a non-empty table, EC_Gold_Events_Update re-walks at least this many
/// trailing days (not just forward from MAX) so interior gaps left by days the
/// app was offline self-heal. Upserts are idempotent and also refresh
/// late-released `actual` values. ~4 months → a few extra 30-day chunks/click.
const EC_GOLD_REWALK_DAYS: i64 = 120;

/// Emitted via Tauri events after every 30-day chunk so the frontend can
/// render a live progress bar instead of staring at a frozen "Updating…"
/// label for two minutes.
#[derive(serde::Serialize, Clone)]
struct EcGoldProgress {
    chunks_done: usize,
    chunks_total: usize,
    rows_added_so_far: usize,
    /// YYYY-MM-DD of the chunk we just finished.
    current_chunk_start: String,
    current_chunk_end: String,
    /// Wallclock seconds elapsed since the click. Frontend uses this to
    /// compute an ETA = elapsed / chunks_done × (chunks_total - chunks_done).
    elapsed_secs: u64,
}

#[derive(serde::Serialize)]
struct EcGoldUpdateResult {
    /// True iff `xauusd_economic_calendar` already existed before this run.
    /// False on the very first click (we just created it from scratch).
    table_existed: bool,
    /// `MAX(timestamp_utc)` in the table before the run — null if fresh.
    cursor_before: Option<String>,
    /// First date the walk asked FXStreet for (YYYY-MM-DD).
    walk_start: String,
    /// End of the walk (always today's UTC date, YYYY-MM-DD).
    walk_end: String,
    /// 30-day chunks fetched + upserted this run.
    chunks_processed: usize,
    /// Sum of rows inserted/updated this run (after currency filter).
    rows_added_this_call: usize,
    /// `COUNT(*) FROM xauusd_economic_calendar` after the run.
    total_rows: usize,
    /// Final coverage window after the run.
    oldest_in_db: Option<String>,
    newest_in_db: Option<String>,
    /// Wallclock time spent on the walk.
    duration_ms: u128,
    /// Set if a chunk failed mid-walk; the partial progress is still
    /// committed and the next click will resume from `MAX(timestamp_utc)+1`.
    error: Option<String>,
}

/// Tauri command behind the `EC_Gold_Events_Update` button.
///
///  - If `xauusd_economic_calendar` is missing → create it and walk every
///    30-day window from 2009-01-01 → today, upserting all gold-relevant
///    events (USD/EUR/GBP/JPY/CHF/AUD/CNY, every impact level).
///  - If it exists → resume from `MAX(timestamp_utc) + 1 day` and walk
///    forward to today.
///
/// Per-chunk commit so a network/proxy hiccup never costs more than one
/// chunk of progress. The next click picks up from the last committed row.
#[tauri::command]
async fn update_ec_gold_events(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<EcGoldUpdateResult, String> {
    use chrono::{Duration, NaiveDate};
    use tauri::Emitter;
    let start_clock = std::time::Instant::now();

    // Step 1: proxy must be alive.
    if !ensure_econcal_alive().await {
        return Ok(EcGoldUpdateResult {
            table_existed: false,
            cursor_before: None,
            walk_start: String::new(),
            walk_end: chrono::Utc::now().format("%Y-%m-%d").to_string(),
            chunks_processed: 0,
            rows_added_this_call: 0,
            total_rows: 0,
            oldest_in_db: None,
            newest_in_db: None,
            duration_ms: start_clock.elapsed().as_millis(),
            error: Some("Econcal proxy not reachable on :6000.".into()),
        });
    }

    // Step 2: ensure table exists and find resume cursor.
    let (table_existed, cursor_before, cursor_start): (bool, Option<String>, NaiveDate) = {
        let db_mutex = state.db_mutex.clone();
        tokio::task::spawn_blocking(move || -> Result<(bool, Option<String>, NaiveDate), String> {
            let _lock = db_mutex.lock().map_err(|e| format!("db lock poisoned: {}", e))?;
            let db = duckdb::Connection::open(DB_PATH).map_err(|e| format!("open db: {}", e))?;
            let existed = db.query_row(
                "SELECT COUNT(*) FROM information_schema.tables WHERE table_name = 'xauusd_economic_calendar'",
                [], |row| row.get::<_, i64>(0)
            ).unwrap_or(0) > 0;
            ec_realtime::create_xauusd_ec_table(&db)?;
            let max_ts: Option<String> = db.query_row(
                "SELECT MAX(timestamp_utc) FROM xauusd_economic_calendar",
                [], |row| row.get::<_, Option<String>>(0)
            ).unwrap_or(None);
            let floor = NaiveDate::parse_from_str(EC_GOLD_START_DATE, "%Y-%m-%d")
                .map_err(|e| format!("parse start date: {}", e))?;
            let start = match &max_ts {
                Some(ts) if ts.len() >= 10 => {
                    let from_max = NaiveDate::parse_from_str(&ts[..10], "%Y-%m-%d")
                        .map_err(|e| format!("parse max ts: {}", e))?
                        + Duration::days(1);
                    // Re-walk a trailing window so interior gaps (days the app was
                    // offline, so the live upsert never advanced past them) self-heal
                    // even though MAX has since jumped ahead. Clamped to the floor.
                    let trailing = chrono::Utc::now().date_naive() - Duration::days(EC_GOLD_REWALK_DAYS);
                    from_max.min(trailing).max(floor)
                }
                _ => floor,
            };
            Ok((existed, max_ts, start))
        }).await.map_err(|e| format!("cursor task join: {}", e))??
    };

    let today: NaiveDate = chrono::Utc::now().date_naive();

    // Already current → no-op.
    if cursor_start > today {
        let (total, oldest, newest) = read_xauusd_ec_stats(&state).await?;
        return Ok(EcGoldUpdateResult {
            table_existed,
            cursor_before,
            walk_start: cursor_start.format("%Y-%m-%d").to_string(),
            walk_end: today.format("%Y-%m-%d").to_string(),
            chunks_processed: 0,
            rows_added_this_call: 0,
            total_rows: total,
            oldest_in_db: oldest,
            newest_in_db: newest,
            duration_ms: start_clock.elapsed().as_millis(),
            error: None,
        });
    }

    // Step 3: walk forward in 30-day chunks. No per-click cap — a single
    // click does the whole backfill (2009→today ≈ 200 chunks ≈ ~2 minutes).
    // Emit a Tauri event after every chunk so the UI can render progress.
    let walk_start_str = cursor_start.format("%Y-%m-%d").to_string();
    let total_days = (today - cursor_start).num_days().max(0);
    let chunks_total = ((total_days as usize + EC_GOLD_CHUNK_DAYS as usize - 1)
                       / EC_GOLD_CHUNK_DAYS as usize)
                       .max(1);
    let mut cursor = cursor_start;
    let mut rows_added_this_call = 0usize;
    let mut chunks_processed = 0usize;
    let mut last_error: Option<String> = None;
    let mut chunk_idx = 0usize;

    while cursor <= today {
        let chunk_end = (cursor + Duration::days(EC_GOLD_CHUNK_DAYS - 1)).min(today);
        let s = cursor.format("%Y%m%d").to_string();
        let e = chunk_end.format("%Y%m%d").to_string();
        chunk_idx += 1;

        let fetched = match ec_realtime::fetch_events_for_date(&s, &e).await {
            Ok(r) => r,
            Err(err) => {
                last_error = Some(format!("chunk {} ({}..{}) failed: {}", chunk_idx, s, e, err));
                break;
            }
        };

        let db_mutex = state.db_mutex.clone();
        let fetched_for_write = fetched.clone();
        let written = tokio::task::spawn_blocking(move || -> Result<usize, String> {
            let _lock = db_mutex.lock().map_err(|e| format!("db lock poisoned: {}", e))?;
            let db = duckdb::Connection::open(DB_PATH).map_err(|e| format!("open db: {}", e))?;
            ec_realtime::upsert_xauusd_ec(&db, &fetched_for_write)
        })
        .await
        .map_err(|e| format!("upsert task join: {}", e))??;

        rows_added_this_call += written;
        chunks_processed += 1;
        println!("[ec-gold] {}..{}: {} fetched, {} kept (cum {} / chunk {}/{})",
                 s, e, fetched.len(), written, rows_added_this_call, chunks_processed, chunks_total);

        // Push live progress to the frontend so the user sees the walk
        // happening instead of a frozen "Updating…" label.
        let progress = EcGoldProgress {
            chunks_done: chunks_processed,
            chunks_total,
            rows_added_so_far: rows_added_this_call,
            current_chunk_start: cursor.format("%Y-%m-%d").to_string(),
            current_chunk_end: chunk_end.format("%Y-%m-%d").to_string(),
            elapsed_secs: start_clock.elapsed().as_secs(),
        };
        let _ = app.emit("ec_gold_progress", &progress);

        cursor = chunk_end + Duration::days(1);

        // 120 ms pacing so the puppeteer proxy stays happy.
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    }

    let (total_rows, oldest_in_db, newest_in_db) = read_xauusd_ec_stats(&state).await?;

    Ok(EcGoldUpdateResult {
        table_existed,
        cursor_before,
        walk_start: walk_start_str,
        walk_end: today.format("%Y-%m-%d").to_string(),
        chunks_processed,
        rows_added_this_call,
        total_rows,
        oldest_in_db,
        newest_in_db,
        duration_ms: start_clock.elapsed().as_millis(),
        error: last_error,
    })
}

/// Helper for `update_ec_gold_events`: row count + coverage window snapshot.
async fn read_xauusd_ec_stats(state: &tauri::State<'_, AppState>) -> Result<(usize, Option<String>, Option<String>), String> {
    let db_mutex = state.db_mutex.clone();
    tokio::task::spawn_blocking(move || -> Result<(usize, Option<String>, Option<String>), String> {
        let _lock = db_mutex.lock().map_err(|e| format!("db lock poisoned: {}", e))?;
        let db = duckdb::Connection::open(DB_PATH).map_err(|e| format!("open db: {}", e))?;
        let count: i64 = db.query_row(
            "SELECT COUNT(*) FROM xauusd_economic_calendar",
            [], |row| row.get(0)
        ).unwrap_or(0);
        let oldest: Option<String> = db.query_row(
            "SELECT MIN(timestamp_utc) FROM xauusd_economic_calendar",
            [], |row| row.get::<_, Option<String>>(0)
        ).unwrap_or(None);
        let newest: Option<String> = db.query_row(
            "SELECT MAX(timestamp_utc) FROM xauusd_economic_calendar",
            [], |row| row.get::<_, Option<String>>(0)
        ).unwrap_or(None);
        Ok((count as usize, oldest, newest))
    })
    .await
    .map_err(|e| format!("stats task join: {}", e))?
}

// ── Per-TF stats panel for the Archives tab ──────────────────────────────────

#[derive(serde::Serialize, Clone)]
struct TfStats {
    /// Display name ("M1", "M5", ..., "MN1")
    timeframe: String,
    /// Backing table name ("xauusd_m1", etc.)
    table: String,
    /// Row count (0 if table missing).
    rows: u64,
    /// Oldest bar's timestamp as ISO 8601 (or null).
    oldest: Option<String>,
    /// Newest bar's timestamp as ISO 8601 (or null).
    newest: Option<String>,
    /// (newest - oldest) in days as a float, null if empty.
    coverage_days: Option<f64>,
    /// (now - newest) in seconds — how stale the tail is. Null if empty.
    tail_age_secs: Option<i64>,
    /// One bar width in seconds — used by the UI to decide "fresh" vs "stale".
    bar_secs: i64,
}

#[derive(serde::Serialize)]
struct XauusdStatsResult {
    timeframes: Vec<TfStats>,
    /// "live" / "weekend-closed" — gives the UI context for what "fresh" means.
    market_state: String,
    /// `now` at query time (so the UI's "fresh as of …" indicator stays honest).
    queried_at_utc: String,
}

/// Tauri command for the Archives tab's "Gold DB timeframe states" panel.
/// Returns one row per xauusd_{tf} table with row count + coverage + freshness.
#[tauri::command]
async fn get_xauusd_tf_stats(state: tauri::State<'_, AppState>) -> Result<XauusdStatsResult, String> {
    // (display name, table suffix, bar width seconds) — only the TFs the
    // broker actually exposes. M2/M4/M10/M30/H4 dropped because the broker
    // returns empty for them.
    let tfs: &[(&str, &str, i64)] = &[
        ("M1",  "m1",  60),
        ("M3",  "m3",  180),
        ("M5",  "m5",  300),
        ("M15", "m15", 900),
        ("H1",  "h1",  3600),
        ("H12", "h12", 43200),
        ("D1",  "d1",  86400),
        ("W1",  "w1",  604800),
        ("MN1", "mn1", 2592000),
    ];

    let now_secs = chrono::Utc::now().timestamp();
    let queried_at_utc = chrono::Utc::now().to_rfc3339();
    let market_state = if xauusd_is_market_closed() { "weekend-closed" } else { "live" }.to_string();

    let db_mutex = state.db_mutex.clone();
    let tfs_vec: Vec<(String, String, i64)> = tfs.iter()
        .map(|(n, s, b)| (n.to_string(), format!("xauusd_{}", s), *b))
        .collect();

    let timeframes = tokio::task::spawn_blocking(move || -> Vec<TfStats> {
        let _lock = match db_mutex.lock() { Ok(l) => l, Err(_) => return Vec::new() };
        let db = match duckdb::Connection::open(DB_PATH) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };

        tfs_vec.into_iter().map(|(tf, table, bar_secs)| {
            // Existence check — don't auto-create here; if a TF was never
            // touched, we want to report rows=0 honestly.
            let exists: i64 = db.query_row(
                "SELECT COUNT(*) FROM information_schema.tables WHERE table_name = ?",
                duckdb::params![table.as_str()],
                |row| row.get(0)
            ).unwrap_or(0);

            if exists == 0 {
                return TfStats {
                    timeframe: tf, table, rows: 0,
                    oldest: None, newest: None,
                    coverage_days: None, tail_age_secs: None, bar_secs,
                };
            }

            let count: i64 = db.query_row(
                &format!("SELECT COUNT(*) FROM {}", table),
                [], |row| row.get(0)
            ).unwrap_or(0);

            let oldest_ts: Option<i64> = db.query_row(
                &format!("SELECT MIN(timestamp) FROM {}", table),
                [], |row| row.get::<_, Option<i64>>(0)
            ).ok().flatten();
            let newest_ts: Option<i64> = db.query_row(
                &format!("SELECT MAX(timestamp) FROM {}", table),
                [], |row| row.get::<_, Option<i64>>(0)
            ).ok().flatten();

            let coverage_days = match (oldest_ts, newest_ts) {
                (Some(o), Some(n)) if n > o => Some((n - o) as f64 / 86400.0),
                _ => None,
            };
            let tail_age_secs = newest_ts.map(|n| now_secs - n);

            let to_iso = |ts: i64| chrono::DateTime::<chrono::Utc>::from_timestamp(ts, 0)
                .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S").to_string());

            TfStats {
                timeframe: tf,
                table,
                rows: count as u64,
                oldest: oldest_ts.and_then(to_iso),
                newest: newest_ts.and_then(to_iso),
                coverage_days,
                tail_age_secs,
                bar_secs,
            }
        }).collect()
    })
    .await
    .map_err(|e| format!("stats task join: {}", e))?;

    Ok(XauusdStatsResult { timeframes, market_state, queried_at_utc })
}

/// Root directory for the per-day EC events archive. Mirrors `news_data/all/`
/// in layout: `ec_events_data/all/YYYY-MM/YYYY-MM-DD.json`.
const EC_ARCHIVE_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/ec_events_data/all");

/// Per-day archive root for the MyFXBook calendar — same layout as the EC/news
/// archives: `myfxbook_events_data/all/YYYY-MM/YYYY-MM-DD.json`.
const MFB_ARCHIVE_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/myfxbook_events_data/all");

/// Per-day archive root for MyFXBook news/analysis/press-release items.
const MFB_NEWS_ARCHIVE_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/myfxbook_news_data/all");

/// Per-day archive roots for the ForexFactory calendar + news.
const FF_CAL_ARCHIVE_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/forexfactory_calendar_data/all");
const FF_NEWS_ARCHIVE_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/forexfactory_news_data/all");

/// Guard so the MyFXBook article-body backfill never overlaps itself (each cycle
/// fetches up to N full article pages, which can outlast the 5-min tick).
static MFB_NEWS_BODY_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Backfill full article bodies for MyFXBook news items that don't have one yet.
/// Runs OFF the session loop (each body is a separate page fetch). Bounded per
/// run; remaining items are picked up on later cycles. After writing bodies it
/// rewrites the affected per-day JSON files (and today's table) so the stored
/// archive carries the entire article, not just the URL.
async fn run_mfb_news_body_backfill(db_mutex: SharedDb) {
    // 1. List items still missing a body (newest first, bounded).
    let missing: Vec<(String, String, String)> = {
        let db_mutex = db_mutex.clone();
        match tokio::task::spawn_blocking(move || {
            let _lock = db_mutex.lock().ok()?;
            let db = duckdb::Connection::open(DB_PATH).ok()?;
            Some(myfxbook_news::list_missing_bodies(&db, 40))
        }).await {
            Ok(Some(v)) => v,
            _ => Vec::new(),
        }
    };
    if missing.is_empty() { return; }

    // 2. Fetch each article body (sequential — be gentle on the source).
    let mut fetched: Vec<(String, String)> = Vec::new();
    let mut days: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for (id, url, day) in &missing {
        if let Some(body) = myfxbook_news::fetch_body(url).await {
            fetched.push((id.clone(), body));
            days.insert(day.clone());
        }
    }
    if fetched.is_empty() { return; }
    let n = fetched.len();

    // 3. Persist bodies + rewrite the affected day-files and today's table.
    let _ = tokio::task::spawn_blocking(move || {
        let Ok(_lock) = db_mutex.lock() else { return };
        let Ok(db) = duckdb::Connection::open(DB_PATH) else { return };
        for (id, body) in &fetched { myfxbook_news::set_body(&db, id, body); }
        for day in &days { let _ = myfxbook_news::write_archive_day(&db, MFB_NEWS_ARCHIVE_ROOT, day); }
        let _ = myfxbook_news::write_today_from_archive(&db);
    }).await;
    println!("[mfb-news-body] backfilled {} article bodies", n);
}

/// Backfill ForexFactory news bodies by following each item's `/hit` redirect to
/// the original publisher and extracting the article text (best-effort — many FF
/// sources are social posts / JS-rendered, so some bodies stay empty). Off-loop,
/// bounded; rewrites the affected per-day JSON files + today's table.
async fn run_ff_news_body_backfill(db_mutex: SharedDb) {
    let missing: Vec<(String, String, String)> = {
        let db_mutex = db_mutex.clone();
        match tokio::task::spawn_blocking(move || {
            let _lock = db_mutex.lock().ok()?;
            let db = duckdb::Connection::open(DB_PATH).ok()?;
            Some(forexfactory::list_missing_news_bodies(&db, 25))
        }).await {
            Ok(Some(v)) => v,
            _ => Vec::new(),
        }
    };
    if missing.is_empty() { return; }

    let mut fetched: Vec<(String, String)> = Vec::new();
    let mut days: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for (id, hit_url, day) in &missing {
        if let Some(body) = forexfactory::fetch_body(hit_url).await {
            fetched.push((id.clone(), body));
            days.insert(day.clone());
        }
    }
    if fetched.is_empty() { return; }
    let n = fetched.len();

    let _ = tokio::task::spawn_blocking(move || {
        let Ok(_lock) = db_mutex.lock() else { return };
        let Ok(db) = duckdb::Connection::open(DB_PATH) else { return };
        for (id, body) in &fetched { forexfactory::set_news_body(&db, id, body); }
        for day in &days { let _ = forexfactory::write_news_archive_day(&db, FF_NEWS_ARCHIVE_ROOT, day); }
        let _ = forexfactory::write_news_today(&db);
    }).await;
    println!("[ff-news-body] backfilled {} article bodies", n);
}

#[derive(serde::Serialize, Clone)]
struct EcGoldStorageProgress {
    files_done: usize,
    files_total: usize,
    events_written_so_far: usize,
    /// YYYY-MM-DD of the file we just finished writing.
    current_day: String,
    elapsed_secs: u64,
}

#[derive(serde::Serialize)]
struct EcGoldStorageResult {
    /// True if any disk files existed before this run (i.e. an incremental
    /// update rather than a fresh bootstrap).
    incremental: bool,
    /// `MAX(YYYY-MM-DD)` of the disk archive before this run, null if empty.
    disk_latest_day_before: Option<String>,
    /// `MAX(YYYY-MM-DD)` in `xauusd_economic_calendar`, null if table empty.
    db_latest_day: Option<String>,
    /// Days inspected this run that already match the DB → no rewrite needed.
    days_already_current: usize,
    /// Days actually (re)written this run.
    files_written: usize,
    /// Sum of events in the (re)written files.
    events_written: usize,
    /// True iff nothing needed updating (disk already matched DB).
    up_to_date: bool,
    archive_root: String,
    duration_ms: u128,
    error: Option<String>,
}

/// Tauri command behind the `EC_Gold_events_storage` button.
///
/// Diff-aware: on first run, writes one JSON file per day under
/// `ec_events_data/all/YYYY-MM/YYYY-MM-DD.json` for every day in the DB.
/// On re-run, walks the existing archive, finds the latest day file, and:
///   - if the DB has newer days → writes those new days' files,
///   - if the DB has more events for the latest disk day → rewrites just that day,
///   - if everything already matches → returns `up_to_date = true` (no work done).
///
/// Per-day file format mirrors `news_data/all/YYYY-MM-DD.json`:
/// ```json
/// {
///   "date": "2026-05-26",
///   "count": 18,
///   "generated_at": "2026-05-26T12:34:56Z",
///   "events": [ { event row as object }, ... ]
/// }
/// ```
#[tauri::command]
async fn store_ec_gold_events(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<EcGoldStorageResult, String> {
    run_ec_gold_store(state.db_mutex.clone(), Some(app)).await
}

/// Guard so the auto EC-store (driven by the live EC fetch loop) never overlaps.
static EC_STORE_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The EC_Gold_events_storage cycle as a reusable function. Driven both by the
/// manual button (`store_ec_gold_events`) and automatically by the live EC fetch
/// loop. Diff-aware writer of per-day JSON under `ec_events_data/all/`. When
/// `app` is `None` (auto path) no progress events are emitted.
async fn run_ec_gold_store(
    db_mutex: SharedDb,
    app: Option<tauri::AppHandle>,
) -> Result<EcGoldStorageResult, String> {
    use tauri::Emitter;
    let start_clock = std::time::Instant::now();

    // Step 1: find the latest day already on disk.
    let disk_latest_day_before: Option<String> = tokio::task::spawn_blocking(find_latest_disk_day)
        .await
        .map_err(|e| format!("disk-scan task join: {}", e))?;
    let incremental = disk_latest_day_before.is_some();

    // Step 2: query (day, count) for every day in xauusd_economic_calendar.
    let db_days: Vec<(String, usize)> = {
        let db_mutex = db_mutex.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<(String, usize)>, String> {
            let _lock = db_mutex.lock().map_err(|e| format!("db lock poisoned: {}", e))?;
            let db = duckdb::Connection::open(DB_PATH).map_err(|e| format!("open db: {}", e))?;
            let mut stmt = db.prepare(
                "SELECT substr(timestamp_utc, 1, 10) AS day, COUNT(*) AS n
                 FROM xauusd_economic_calendar
                 WHERE length(timestamp_utc) >= 10
                 GROUP BY day
                 ORDER BY day ASC"
            ).map_err(|e| format!("prepare day-counts: {}", e))?;
            let rows: Vec<(String, usize)> = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
            }).map_err(|e| format!("query day-counts: {}", e))?
              .flatten().collect();
            Ok(rows)
        })
        .await
        .map_err(|e| format!("day-counts task join: {}", e))??
    };

    if db_days.is_empty() {
        return Ok(EcGoldStorageResult {
            incremental, disk_latest_day_before, db_latest_day: None,
            days_already_current: 0,
            files_written: 0, events_written: 0,
            up_to_date: false,
            archive_root: EC_ARCHIVE_ROOT.to_string(),
            duration_ms: start_clock.elapsed().as_millis(),
            error: Some("xauusd_economic_calendar is empty — run EC_Gold_Events_Update first.".into()),
        });
    }

    let db_latest_day = db_days.last().map(|(d, _)| d.clone());

    // Step 3: decide which days to (re)write.
    //
    //   First run (no disk):  every DB day.
    //   Re-run:               every DB day at/after the comparison horizon whose
    //                         DB count differs from the on-disk file's count
    //                         (missing-on-disk counts as count 0).
    //
    // Horizon = the EARLIER of the newest on-disk day and a trailing window
    // (today - EC_GOLD_REWALK_DAYS). Reaching back past disk-latest lets interior
    // gaps — days the walk just back-filled BELOW an already-present newer file —
    // get exported. Days older than the horizon are trusted as-is.
    let horizon: Option<String> = disk_latest_day_before.as_ref().map(|disk_latest| {
        let trailing = (chrono::Utc::now().date_naive() - chrono::Duration::days(EC_GOLD_REWALK_DAYS))
            .format("%Y-%m-%d").to_string();
        std::cmp::min(disk_latest.clone(), trailing)
    });
    let days_to_write: Vec<(String, usize)> = match &horizon {
        None => db_days.clone(),
        Some(h) => db_days.iter()
            .filter(|(day, db_count)| {
                if day.as_str() < h.as_str() { return false; }
                read_disk_event_count(day) != *db_count
            })
            .cloned()
            .collect(),
    };
    let days_already_current = if let Some(h) = &horizon {
        db_days.iter()
            .filter(|(day, _)| day.as_str() >= h.as_str())
            .count()
            .saturating_sub(days_to_write.len())
    } else { 0 };

    if days_to_write.is_empty() {
        return Ok(EcGoldStorageResult {
            incremental, disk_latest_day_before, db_latest_day,
            days_already_current,
            files_written: 0, events_written: 0,
            up_to_date: true,
            archive_root: EC_ARCHIVE_ROOT.to_string(),
            duration_ms: start_clock.elapsed().as_millis(),
            error: None,
        });
    }

    // Step 4: group the days-to-write by month so we can pull each month's
    // events in one DB query (cheaper than 1 query per day for first runs
    // where days_to_write can be ~5000 entries).
    let files_total = days_to_write.len();
    let mut by_month: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for (day, _) in &days_to_write {
        if day.len() >= 7 {
            by_month.entry(day[..7].to_string()).or_default().push(day.clone());
        }
    }
    let wanted_days: std::collections::HashSet<String> =
        days_to_write.iter().map(|(d, _)| d.clone()).collect();

    let mut files_written = 0usize;
    let mut events_written = 0usize;
    let mut last_error: Option<String> = None;

    // Fire an initial 0% progress so the UI shows the bar straight away
    // (without waiting for the first file to land). For fast re-runs of
    // 1-5 files, the bar would otherwise pop in for ~50 ms at the very end.
    let first_day = days_to_write.first().map(|(d, _)| d.clone()).unwrap_or_default();
    if let Some(app) = &app {
        let _ = app.emit("ec_gold_storage_progress", &EcGoldStorageProgress {
            files_done: 0,
            files_total,
            events_written_so_far: 0,
            current_day: first_day,
            elapsed_secs: start_clock.elapsed().as_secs(),
        });
    }

    'months: for (month, _) in &by_month {
        // Pull every event in this month, ordered by timestamp.
        let db_mutex = db_mutex.clone();
        let month_q = month.clone();
        let month_events: Vec<(String, serde_json::Value)> = match tokio::task::spawn_blocking(move || -> Result<Vec<(String, serde_json::Value)>, String> {
            let _lock = db_mutex.lock().map_err(|e| format!("db lock poisoned: {}", e))?;
            let db = duckdb::Connection::open(DB_PATH).map_err(|e| format!("open db: {}", e))?;
            let mut stmt = db.prepare(
                "SELECT event_date_id, event_id, event_name, currency, country_code,
                        volatility, timestamp_utc, weekday, hour_utc,
                        actual_raw, forecast_raw, previous_raw,
                        actual, forecast, previous, surprise, beats_forecast, unit
                 FROM xauusd_economic_calendar
                 WHERE substr(timestamp_utc, 1, 7) = ?
                 ORDER BY timestamp_utc ASC"
            ).map_err(|e| format!("prepare month query: {}", e))?;
            let rows = stmt.query_map([month_q.as_str()], |row| {
                let ts: String = row.get(6)?;
                let day = if ts.len() >= 10 { ts[..10].to_string() } else { ts.clone() };
                let event = serde_json::json!({
                    "event_date_id":  row.get::<_, String>(0)?,
                    "event_id":       row.get::<_, String>(1)?,
                    "event_name":     row.get::<_, String>(2)?,
                    "currency":       row.get::<_, String>(3)?,
                    "country_code":   row.get::<_, String>(4)?,
                    "volatility":     row.get::<_, i32>(5)?,
                    "timestamp_utc":  ts,
                    "weekday":        row.get::<_, i32>(7)?,
                    "hour_utc":       row.get::<_, i32>(8)?,
                    "actual_raw":     row.get::<_, Option<String>>(9)?,
                    "forecast_raw":   row.get::<_, Option<String>>(10)?,
                    "previous_raw":   row.get::<_, Option<String>>(11)?,
                    "actual":         row.get::<_, Option<f64>>(12)?,
                    "forecast":       row.get::<_, Option<f64>>(13)?,
                    "previous":       row.get::<_, Option<f64>>(14)?,
                    "surprise":       row.get::<_, Option<f64>>(15)?,
                    "beats_forecast": row.get::<_, Option<i32>>(16)?,
                    "unit":           row.get::<_, Option<String>>(17)?,
                });
                Ok((day, event))
            }).map_err(|e| format!("query month: {}", e))?;
            Ok(rows.flatten().collect())
        }).await.map_err(|e| format!("month task join: {}", e))? {
            Ok(v) => v,
            Err(e) => { last_error = Some(format!("{} read failed: {}", month, e)); break 'months; }
        };

        // Group this month's rows by day, keeping only days_to_write.
        let mut by_day: std::collections::BTreeMap<String, Vec<serde_json::Value>> =
            std::collections::BTreeMap::new();
        for (day, event) in month_events {
            if wanted_days.contains(&day) {
                by_day.entry(day).or_default().push(event);
            }
        }

        let month_dir = std::path::Path::new(EC_ARCHIVE_ROOT).join(month);
        if let Err(e) = std::fs::create_dir_all(&month_dir) {
            last_error = Some(format!("create dir {}: {}", month_dir.display(), e));
            break 'months;
        }

        let now_rfc = chrono::Utc::now().to_rfc3339();
        for (day, events) in &by_day {
            let path = month_dir.join(format!("{}.json", day));
            let count = events.len();
            let payload = serde_json::json!({
                "date":         day,
                "count":        count,
                "generated_at": now_rfc,
                "events":       events,
            });
            let pretty = match serde_json::to_string_pretty(&payload) {
                Ok(s) => s,
                Err(e) => { last_error = Some(format!("serialize {}: {}", day, e)); break 'months; }
            };
            if let Err(e) = std::fs::write(&path, pretty) {
                last_error = Some(format!("write {}: {}", path.display(), e));
                break 'months;
            }
            files_written += 1;
            events_written += count;

            if let Some(app) = &app {
                let _ = app.emit("ec_gold_storage_progress", &EcGoldStorageProgress {
                    files_done: files_written,
                    files_total,
                    events_written_so_far: events_written,
                    current_day: day.clone(),
                    elapsed_secs: start_clock.elapsed().as_secs(),
                });
            }
        }
    }

    Ok(EcGoldStorageResult {
        incremental, disk_latest_day_before, db_latest_day,
        days_already_current,
        files_written, events_written,
        up_to_date: false,
        archive_root: EC_ARCHIVE_ROOT.to_string(),
        duration_ms: start_clock.elapsed().as_millis(),
        error: last_error,
    })
}

/// Walk `ec_events_data/all/YYYY-MM/` and return the lexically-largest
/// `YYYY-MM-DD.json` filename (without the extension), or None if the
/// archive root doesn't exist or has no day files.
fn find_latest_disk_day() -> Option<String> {
    let root = std::path::Path::new(EC_ARCHIVE_ROOT);
    if !root.exists() { return None; }
    let mut newest: Option<String> = None;
    let month_iter = std::fs::read_dir(root).ok()?;
    for month_entry in month_iter.flatten() {
        let mp = month_entry.path();
        if !mp.is_dir() { continue; }
        let day_iter = match std::fs::read_dir(&mp) { Ok(d) => d, Err(_) => continue };
        for day_entry in day_iter.flatten() {
            let p = day_entry.path();
            let name = match p.file_name().and_then(|s| s.to_str()) { Some(s) => s, None => continue };
            if name.len() != 15 || !name.ends_with(".json") { continue; }
            let day = &name[..10];
            if newest.as_deref().map(|n| day > n).unwrap_or(true) {
                newest = Some(day.to_string());
            }
        }
    }
    newest
}

/// Return the `count` field from `ec_events_data/all/YYYY-MM/{day}.json`,
/// or 0 if the file is missing / unreadable / malformed.
fn read_disk_event_count(day: &str) -> usize {
    if day.len() < 7 { return 0; }
    let path = std::path::Path::new(EC_ARCHIVE_ROOT)
        .join(&day[..7])
        .join(format!("{}.json", day));
    let content = match std::fs::read_to_string(&path) { Ok(c) => c, Err(_) => return 0 };
    let v: serde_json::Value = match serde_json::from_str(&content) { Ok(v) => v, Err(_) => return 0 };
    v.get("count").and_then(|c| c.as_u64()).map(|n| n as usize).unwrap_or(0)
}

// ── Archives tab: import on-disk JSON archives back into DuckDB ───────────────
//
// The per-day JSON files under news_data/all/ and ec_events_data/all/ are EXPORTS
// of the DB. When a clone arrives with rich historical archive files but a near-
// empty DB (e.g. files copied from another branch), the app's News/Calendar and
// the EC "update" walk can't see that history because they read the tables, not
// the files. This command loads the files straight into the tables:
//   - ec_events_data/all/**.json → xauusd_economic_calendar (via upsert_xauusd_ec)
//   - news_data/all/**.json      → news_historical (body preserved via COALESCE)

/// Recursively collect every `*.json` under a `<root>/YYYY-MM/YYYY-MM-DD.json`
/// archive tree (two levels: month dir → day file).
fn walk_archive_json(root: &str) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let root = std::path::Path::new(root);
    let Ok(months) = std::fs::read_dir(root) else { return out };
    for m in months.flatten() {
        let mp = m.path();
        if !mp.is_dir() { continue; }
        let Ok(days) = std::fs::read_dir(&mp) else { continue };
        for d in days.flatten() {
            let p = d.path();
            if p.extension().and_then(|s| s.to_str()) == Some("json") {
                out.push(p);
            }
        }
    }
    out
}

/// Build an `EcRow` from one event object in an `ec_events_data` day file.
/// Returns None if a required field (event_date_id / currency / timestamp) is missing.
fn parse_ec_event(e: &serde_json::Value) -> Option<ec_realtime::EcRow> {
    let s   = |k: &str| e.get(k).and_then(|v| v.as_str()).map(String::from);
    let i   = |k: &str| e.get(k).and_then(|v| v.as_i64());
    let f   = |k: &str| e.get(k).and_then(|v| v.as_f64());
    Some(ec_realtime::EcRow {
        event_date_id: s("event_date_id")?,
        event_id:      s("event_id").unwrap_or_default(),
        event_name:    s("event_name").unwrap_or_default(),
        currency:      s("currency")?,
        country_code:  s("country_code").unwrap_or_default(),
        volatility:    i("volatility").unwrap_or(0) as i8,
        timestamp_utc: s("timestamp_utc")?,
        weekday:       i("weekday").unwrap_or(0) as i8,
        hour_utc:      i("hour_utc").unwrap_or(0) as i8,
        actual_raw:    s("actual_raw"),
        forecast_raw:  s("forecast_raw"),
        previous_raw:  s("previous_raw"),
        actual:        f("actual"),
        forecast:      f("forecast"),
        previous:      f("previous"),
        surprise:      f("surprise"),
        beats_forecast: i("beats_forecast").map(|v| v as i8),
    })
}

#[derive(serde::Serialize)]
struct ArchiveImportResult {
    ec_files: usize,
    ec_rows: usize,
    news_files: usize,
    news_rows: usize,
    /// Articles imported that carried a non-empty body.
    news_bodies: usize,
    message: Option<String>,
}

/// Tauri command behind the Archives "Import disk → DB" button. Loads every
/// on-disk per-day JSON archive into the DuckDB tables the app actually reads.
/// Idempotent: re-running upserts by primary key (event_date_id / article_id)
/// and never overwrites an existing news body with NULL.
#[tauri::command]
async fn import_disk_archives(state: tauri::State<'_, AppState>) -> Result<ArchiveImportResult, String> {
    let db_mutex = state.db_mutex.clone();
    tokio::task::spawn_blocking(move || -> Result<ArchiveImportResult, String> {
        let _lock = db_mutex.lock().map_err(|e| format!("db lock poisoned: {}", e))?;
        let db = duckdb::Connection::open(DB_PATH).map_err(|e| format!("open db: {}", e))?;

        // Ensure both tables exist before the transaction.
        ec_realtime::create_xauusd_ec_table(&db)?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS news_historical (
                article_id    VARCHAR PRIMARY KEY,
                title         VARCHAR NOT NULL,
                published_utc VARCHAR NOT NULL,
                summary       VARCHAR,
                url           VARCHAR,
                author        VARCHAR,
                tags          VARCHAR,
                hour_utc      TINYINT NOT NULL,
                weekday       TINYINT NOT NULL
            )"
        ).map_err(|e| format!("create news_historical: {}", e))?;
        let _ = db.execute("ALTER TABLE news_historical ADD COLUMN IF NOT EXISTS body VARCHAR", []);

        // One big transaction — DuckDB autocommit per-row would be very slow for
        // ~85k EC upserts.
        let _ = db.execute_batch("BEGIN TRANSACTION");

        // ── EC events ──
        let mut ec_files = 0usize;
        let mut ec_rows = 0usize;
        for path in walk_archive_json(EC_ARCHIVE_ROOT) {
            let Ok(content) = std::fs::read_to_string(&path) else { continue };
            let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else { continue };
            if let Some(events) = json.get("events").and_then(|v| v.as_array()) {
                let rows: Vec<ec_realtime::EcRow> = events.iter().filter_map(parse_ec_event).collect();
                if let Ok(n) = ec_realtime::upsert_xauusd_ec(&db, &rows) { ec_rows += n; }
            }
            ec_files += 1;
        }

        // ── News (body preserved) ──
        let news_upsert = "
            INSERT INTO news_historical
                (article_id, title, published_utc, summary, url, author, tags, hour_utc, weekday, body)
            VALUES (?,?,?,?,?,?,?,?,?,?)
            ON CONFLICT (article_id) DO UPDATE SET
                title   = EXCLUDED.title,
                summary = EXCLUDED.summary,
                url     = EXCLUDED.url,
                author  = EXCLUDED.author,
                tags    = EXCLUDED.tags,
                body    = COALESCE(EXCLUDED.body, news_historical.body)
        ";
        let mut news_files = 0usize;
        let mut news_rows = 0usize;
        let mut news_bodies = 0usize;
        for path in walk_archive_json(ARCHIVE_ROOT) {
            let Ok(content) = std::fs::read_to_string(&path) else { continue };
            let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else { continue };
            if let Some(articles) = json.get("articles").and_then(|v| v.as_array()) {
                for a in articles {
                    let Some(article_id) = a.get("article_id").and_then(|v| v.as_str()) else { continue };
                    let Some(published_utc) = a.get("published_utc").and_then(|v| v.as_str()) else { continue };
                    let title   = a.get("title").and_then(|v| v.as_str()).unwrap_or("");
                    let summary = a.get("summary").and_then(|v| v.as_str()).unwrap_or("");
                    let url     = a.get("url").and_then(|v| v.as_str()).unwrap_or("");
                    let author  = a.get("author").and_then(|v| v.as_str()).unwrap_or("");
                    let tags    = a.get("tags").and_then(|v| v.as_str()).unwrap_or("");
                    let hour_utc = a.get("hour_utc").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                    let weekday  = a.get("weekday").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                    let body: Option<String> = a.get("body").and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty()).map(String::from);
                    if body.is_some() { news_bodies += 1; }
                    let params = duckdb::params![
                        article_id, title, published_utc, summary, url, author, tags,
                        hour_utc, weekday, body
                    ];
                    if db.execute(news_upsert, params).is_ok() { news_rows += 1; }
                }
            }
            news_files += 1;
        }

        let _ = db.execute_batch("COMMIT");

        Ok(ArchiveImportResult {
            ec_files, ec_rows, news_files, news_rows, news_bodies,
            message: None,
        })
    })
    .await
    .map_err(|e| format!("import task join: {}", e))?
}

// ── End Archives tab backend ─────────────────────────────────────────────────



/// Tauri-managed state shared with command handlers.
struct AppState {
    db_mutex: SharedDb,
}

/// Extract the FXStreet article body from a JSON-LD `<script>` block in the page HTML.
/// FXStreet embeds the article content as a NewsArticle schema for SEO; we parse
/// `articleBody` directly — much more robust than scraping React-rendered DOM.
fn extract_article_body(html: &str) -> Option<String> {
    // Find every <script type="application/ld+json">...</script>
    let mut search = html;
    while let Some(start_idx) = search.find("application/ld+json") {
        let after_attr = &search[start_idx..];
        let json_start = after_attr.find('>')? + 1;
        let rest = &after_attr[json_start..];
        let end_idx = rest.find("</script>")?;
        let json_str = &rest[..end_idx];
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
            let type_field = val.get("@type").and_then(|v| v.as_str()).unwrap_or("");
            if matches!(type_field, "NewsArticle" | "Article" | "BlogPosting") {
                if let Some(body) = val.get("articleBody").and_then(|v| v.as_str()) {
                    return Some(body.to_string());
                }
            }
        }
        search = &rest[end_idx + "</script>".len()..];
    }
    None
}

/// Outcome of an attempt to fetch one article's body.
#[derive(Debug)]
enum BodyFetch {
    /// HTTP succeeded and the page's NewsArticle JSON-LD gave us a real body.
    Ok(String),
    /// HTTP succeeded but the article genuinely has no body (FXStreet data flashes
    /// like "US ISM PMI came in at..."). Safe to mark with '' so we don't retry.
    EmptyBody,
    /// FXStreet returned 429 — global throttle. The day should be aborted so we
    /// don't waste time and so these rows stay NULL for a later retry.
    RateLimited { retry_after_secs: u64 },
    /// Other fetch failure: timeout, non-2xx (not 429), or JSON-LD parse error.
    /// Leave the row at NULL so a later run can retry.
    Failed,
}

/// Make sure the econcal proxy is responding on :6000. If not, spawn it and
/// poll until it accepts connections (or give up after 30s). Returns true if
/// the proxy is ready, false otherwise.
async fn ensure_econcal_alive() -> bool {
    if std::net::TcpStream::connect("127.0.0.1:6000").is_ok() {
        return true;
    }
    println!("[econcal] proxy not responding on :6000 — attempting to (re)spawn...");
    start_econcal_server();
    // Poll until the proxy answers or we time out. We also pause briefly after
    // the port goes up so puppeteer has time to finish FXStreet auth — the
    // proxy queues requests internally, but skipping the pause sometimes shows
    // up as the first request hanging for ~20s.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if std::net::TcpStream::connect("127.0.0.1:6000").is_ok() {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    println!("[econcal] proxy failed to come up within 30s");
    false
}

/// Strip HTML tags to plain text. <p> boundaries become \n\n, other tags are
/// removed. <style>/<script> blocks are dropped wholesale (including content)
/// so CSS rules don't leak into the body. Whitespace is collapsed.
fn html_to_plain_text(html: &str) -> String {
    // Drop <style>...</style> and <script>...</script> blocks first (content too).
    fn drop_block(s: &str, open: &str, close: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let lower = s.to_ascii_lowercase();
        let mut i = 0;
        while i < s.len() {
            if let Some(off) = lower[i..].find(open) {
                out.push_str(&s[i..i + off]);
                let after_open = i + off;
                if let Some(end_off) = lower[after_open..].find(close) {
                    i = after_open + end_off + close.len();
                } else {
                    // No close tag — drop to end.
                    return out;
                }
            } else {
                out.push_str(&s[i..]);
                break;
            }
        }
        out
    }
    let stripped = drop_block(html, "<style", "</style>");
    let stripped = drop_block(&stripped, "<script", "</script>");

    // Insert paragraph breaks before paragraph-level tags before stripping.
    let normalized = stripped
        .replace("</p>", "</p>\n\n")
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("<br />", "\n");
    // Strip any tags.
    let mut out = String::with_capacity(normalized.len());
    let mut in_tag = false;
    for ch in normalized.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    // Decode a few common entities and collapse whitespace.
    let out = out.replace("&nbsp;", " ").replace("&amp;", "&")
        .replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"")
        .replace("&#39;", "'");
    // Collapse runs of whitespace but keep double newlines as paragraph separators.
    let mut result = String::with_capacity(out.len());
    let mut prev_blank = false;
    for line in out.lines() {
        let trimmed = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if trimmed.is_empty() {
            if !prev_blank && !result.is_empty() { result.push_str("\n\n"); }
            prev_blank = true;
        } else {
            if !result.is_empty() && !result.ends_with("\n\n") { result.push('\n'); }
            result.push_str(&trimmed);
            prev_blank = false;
        }
    }
    result.trim().to_string()
}

/// Fetch one article's body via the FXStreet API (through the local econcal proxy).
/// This endpoint isn't subject to the public-web rate limit (separate bucket) and
/// returns the body as an `HTML` field in JSON — no page-scraping needed.
async fn fetch_one_body(client: &reqwest::Client, article_id: &str) -> BodyFetch {
    let url = format!("http://127.0.0.1:6000/v4/en/post/{}", article_id);
    let resp = match client.get(&url)
        .header("x-api-host", "https://subscriptions.fxstreet.com")
        .timeout(std::time::Duration::from_secs(30))
        .send().await
    {
        Ok(r) => r,
        Err(_) => return BodyFetch::Failed,
    };
    let status = resp.status();
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let retry_after_secs = resp.headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(60);
        return BodyFetch::RateLimited { retry_after_secs };
    }
    if !status.is_success() { return BodyFetch::Failed; }
    let json: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(_) => return BodyFetch::Failed,
    };
    let html = match json.get("HTML").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return BodyFetch::Failed,
    };
    if html.is_empty() {
        return BodyFetch::EmptyBody;
    }
    let text = html_to_plain_text(html);
    if text.is_empty() { BodyFetch::EmptyBody } else { BodyFetch::Ok(text) }
}

/// Ensure a per-(symbol, timeframe) candle cache table exists. Schema matches
/// the legacy `{symbol}_m1` layout in `db::client` so we can share tables.
fn ensure_candle_table(db: &duckdb::Connection, table: &str) -> Result<(), String> {
    db.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS {} (
            timestamp BIGINT PRIMARY KEY,
            open DOUBLE,
            high DOUBLE,
            low DOUBLE,
            close DOUBLE,
            volume BIGINT
        )", table
    )).map_err(|e| format!("create {}: {}", table, e))
}

/// Upsert candles into a cache table. `ON CONFLICT (timestamp) DO UPDATE` so
/// re-fetching the "current" (still-open) candle overwrites with the latest snapshot.
fn upsert_candles(db: &duckdb::Connection, table: &str, candles: &[Candle]) -> Result<usize, String> {
    if candles.is_empty() { return Ok(0); }
    ensure_candle_table(db, table)?;
    let upsert = format!(
        "INSERT INTO {} (timestamp, open, high, low, close, volume) VALUES (?,?,?,?,?,?)
         ON CONFLICT (timestamp) DO UPDATE SET
             open = EXCLUDED.open, high = EXCLUDED.high,
             low  = EXCLUDED.low,  close = EXCLUDED.close,
             volume = EXCLUDED.volume",
        table
    );
    let mut inserted = 0;
    for c in candles {
        if db.execute(&upsert, duckdb::params![
            c.timestamp, c.open, c.high, c.low, c.close, c.volume,
        ]).is_ok() {
            inserted += 1;
        }
    }
    Ok(inserted)
}

#[derive(serde::Serialize)]
struct CandleJson {
    time: i64,     // Unix seconds (UTC) — Lightweight Charts wants this as `time`
    open: f64,
    high: f64,
    low: f64,
    close: f64,
    volume: i64,
}

/// Tauri command: fetch historical OHLC candles for `symbol` at `timeframe`
/// (M1/M5/M15/M30/H1/H4/D1). Always goes live to cTrader via the chart
/// request channel; resolves to the most recent `count` bars ending now.
/// `to_ms` (optional): fetch `count` bars ending at this unix-ms instant. If
/// absent, ends at "now". Used by the chart's lazy-load-on-pan to walk backward.
/// `_force_refresh` is accepted for back-compat but ignored — every call hits
/// the API.
#[tauri::command]
async fn get_trendbars(
    _state: tauri::State<'_, AppState>,
    symbol: String,
    timeframe: String,
    count: u32,
    to_ms: Option<i64>,
    _force_refresh: Option<bool>,
) -> Result<Vec<CandleJson>, String> {
    let symbol_id = {
        let map = symbol_map().lock().map_err(|e| e.to_string())?;
        match map.get(symbol.as_str()).copied() {
            Some(id) => id,
            None => return Err(format!("symbol '{}' not in symbol map (not yet subscribed?)", symbol)),
        }
    };

    let (period, minutes_per_bar): (openapi::ProtoOaTrendbarPeriod, i64) = match timeframe.as_str() {
        "M1"  => (openapi::ProtoOaTrendbarPeriod::M1,  1),
        "M2"  => (openapi::ProtoOaTrendbarPeriod::M2,  2),
        "M3"  => (openapi::ProtoOaTrendbarPeriod::M3,  3),
        "M4"  => (openapi::ProtoOaTrendbarPeriod::M4,  4),
        "M5"  => (openapi::ProtoOaTrendbarPeriod::M5,  5),
        "M10" => (openapi::ProtoOaTrendbarPeriod::M10, 10),
        "M15" => (openapi::ProtoOaTrendbarPeriod::M15, 15),
        "M30" => (openapi::ProtoOaTrendbarPeriod::M30, 30),
        "H1"  => (openapi::ProtoOaTrendbarPeriod::H1,  60),
        "H4"  => (openapi::ProtoOaTrendbarPeriod::H4,  240),
        "H12" => (openapi::ProtoOaTrendbarPeriod::H12, 720),
        "D1"  => (openapi::ProtoOaTrendbarPeriod::D1,  60 * 24),
        "W1"  => (openapi::ProtoOaTrendbarPeriod::W1,  60 * 24 * 7),
        "MN1" => (openapi::ProtoOaTrendbarPeriod::Mn1, 60 * 24 * 30),
        other => return Err(format!("unsupported timeframe '{}'", other)),
    };

    let end_ms = to_ms.unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
    let count_i64 = count as i64;
    let from_ms = end_ms - count_i64 * minutes_per_bar * 60 * 1000;

    let tx = CHART_REQ_TX.get().ok_or("chart channel not initialised")?;
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    tx.send(ChartRequest { symbol_id, period, from_ms, to_ms: end_ms, count, reply: reply_tx })
        .await
        .map_err(|e| format!("send chart req: {}", e))?;
    let timeout = tokio::time::timeout(std::time::Duration::from_secs(30), reply_rx).await;
    let fetched: Vec<Candle> = match timeout {
        Ok(Ok(Ok(c)))  => c,
        Ok(Ok(Err(e))) => return Err(e),
        Ok(Err(_))     => return Err("chart request was cancelled".into()),
        Err(_)         => return Err("cTrader response timeout".into()),
    };

    println!("[chart] live fetch: {} {} {} bars", symbol, timeframe, fetched.len());
    Ok(fetched.into_iter().map(|c| CandleJson {
        time: c.timestamp,
        open: c.open, high: c.high, low: c.low, close: c.close,
        volume: c.volume,
    }).collect())
}

#[derive(serde::Serialize)]
struct ArticleBodyResult {
    article_id: String,
    body: Option<String>,
    status: String, // "ok" | "empty" | "failed"
    message: Option<String>,
}

/// Tauri command: return a MyFXBook article's full body for the news card.
/// Uses the already-backfilled `myfxbook_news_historical.body` if present;
/// otherwise fetches it on demand from the article URL, stores it, and returns it.
#[tauri::command]
async fn get_mfb_news_body(
    article_id: String,
    url: String,
    state: tauri::State<'_, AppState>,
) -> Result<Option<String>, String> {
    // 1. Already stored?
    let existing: Option<String> = {
        let db_mutex = state.db_mutex.clone();
        let aid = article_id.clone();
        tokio::task::spawn_blocking(move || {
            let _lock = db_mutex.lock().ok()?;
            let db = duckdb::Connection::open(DB_PATH).ok()?;
            db.query_row(
                "SELECT body FROM myfxbook_news_historical WHERE article_id = ?",
                duckdb::params![aid], |r| r.get::<_, Option<String>>(0),
            ).ok().flatten()
        }).await.unwrap_or(None)
    };
    if let Some(b) = existing { if !b.is_empty() { return Ok(Some(b)); } }

    // 2. Fetch on demand, store, return.
    if url.is_empty() { return Ok(None); }
    match myfxbook_news::fetch_body(&url).await {
        Some(body) => {
            let db_mutex = state.db_mutex.clone();
            let (aid, b) = (article_id.clone(), body.clone());
            let _ = tokio::task::spawn_blocking(move || {
                if let Ok(_lock) = db_mutex.lock() {
                    if let Ok(db) = duckdb::Connection::open(DB_PATH) {
                        myfxbook_news::set_body(&db, &aid, &b);
                    }
                }
            }).await;
            Ok(Some(body))
        }
        None => Ok(None),
    }
}

/// Collect today's (UTC) news from all three sources as a compact text block for
/// the sentiment agent: "[Source] HH:MM | Title — summary". Bounded per source.
fn gather_todays_news(db: &duckdb::Connection) -> (String, usize) {
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let hhmm = |ts: &str| if ts.len() >= 16 { ts[11..16].to_string() } else { ts.to_string() };
    let trunc = |s: &str, n: usize| { let t = s.trim(); if t.chars().count() > n { t.chars().take(n).collect::<String>() + "…" } else { t.to_string() } };
    let mut lines: Vec<String> = Vec::new();

    if let Ok(mut s) = db.prepare("SELECT published_utc, title, COALESCE(summary,'') FROM news_historical WHERE published_utc LIKE ? || '%' ORDER BY published_utc DESC LIMIT 70") {
        if let Ok(rows) = s.query_map([&today], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))) {
            for (ts, title, sum) in rows.flatten() {
                let s2 = trunc(&sum, 180);
                lines.push(format!("[FXStreet] {} | {}{}", hhmm(&ts), title, if s2.is_empty() { String::new() } else { format!(" — {}", s2) }));
            }
        }
    }
    if let Ok(mut s) = db.prepare("SELECT published_utc, category, title, COALESCE(summary,'') FROM myfxbook_news_historical WHERE published_utc LIKE ? || '%' ORDER BY published_utc DESC LIMIT 60") {
        if let Ok(rows) = s.query_map([&today], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?))) {
            for (ts, cat, title, sum) in rows.flatten() {
                let s2 = trunc(&sum, 180);
                lines.push(format!("[MyFXBook/{}] {} | {}{}", cat, hhmm(&ts), title, if s2.is_empty() { String::new() } else { format!(" — {}", s2) }));
            }
        }
    }
    if let Ok(mut s) = db.prepare("SELECT published_utc, COALESCE(source,''), title, COALESCE(preview,'') FROM forexfactory_news WHERE published_utc LIKE ? || '%' ORDER BY published_utc DESC LIMIT 60") {
        if let Ok(rows) = s.query_map([&today], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?))) {
            for (ts, src, title, prev) in rows.flatten() {
                let p2 = trunc(&prev, 180);
                lines.push(format!("[ForexFactory{}] {} | {}{}", if src.is_empty() { String::new() } else { format!("/{}", src) }, hhmm(&ts), title, if p2.is_empty() { String::new() } else { format!(" — {}", p2) }));
            }
        }
    }
    let count = lines.len();
    (lines.join("\n"), count)
}

/// Pull the first balanced JSON object out of the model's raw text.
fn extract_sentiment_json(raw: &str) -> Option<serde_json::Value> {
    let t = raw.trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(t) { return Some(v); }
    let start = t.find('{')?;
    let end = t.rfind('}')?;
    if end > start { serde_json::from_str(&t[start..=end]).ok() } else { None }
}

/// Tauri command behind the Market Predictor "Today's Sentiment" button. Sends
/// today's news (all 3 sources) to the gold-sentiment agent, parses the JSON,
/// stamps it, persists it (so the panel survives until the next click), returns it.
#[tauri::command]
async fn get_gold_sentiment(state: tauri::State<'_, AppState>) -> Result<serde_json::Value, String> {
    // 1. Gather today's news.
    let (news, count) = {
        let db_mutex = state.db_mutex.clone();
        tokio::task::spawn_blocking(move || {
            let _lock = db_mutex.lock().ok()?;
            let db = duckdb::Connection::open(DB_PATH).ok()?;
            Some(gather_todays_news(&db))
        }).await.unwrap_or(None).unwrap_or((String::new(), 0))
    };
    if count == 0 {
        return Err("No news stored for today yet — let the news feeds run first.".into());
    }

    // 2. Ask the gold-sentiment agent.
    let now = chrono::Utc::now();
    let user = format!(
        "Today is {} (UTC). Below are today's market news items from FXStreet, MyFXBook and ForexFactory. \
         Decrypt them as a human gold trader would and return the XAUUSD market disposition now plus the \
         forward outlook for the current/next session. JSON only.\n\n{}",
        now.format("%Y-%m-%d"), news,
    );
    let model = std::env::var("CLAUDE_SENTIMENT_MODEL").unwrap_or_else(|_| "opus".to_string());
    // Reading a day of news is heavier than the trade-idea calls — give it room
    // (opus is slower still). 4 min cap.
    let raw = ai::traders::claude_cli_raw_timeout(
        &model, &ai::traders::gold_sentiment_prompt(), &user, std::time::Duration::from_secs(240),
    ).await?;
    let mut val = extract_sentiment_json(&raw)
        .ok_or_else(|| "could not parse sentiment JSON from the model output".to_string())?;

    // 3. Stamp with meta.
    if let Some(obj) = val.as_object_mut() {
        obj.insert("updated_utc".to_string(), serde_json::json!(now.to_rfc3339()));
        obj.insert("news_count".to_string(), serde_json::json!(count));
    }

    // 4. Persist (keeps a history; the panel loads the latest).
    {
        let db_mutex = state.db_mutex.clone();
        let updated = now.to_rfc3339();
        let payload = val.to_string();
        let _ = tokio::task::spawn_blocking(move || {
            if let Ok(_lock) = db_mutex.lock() {
                if let Ok(db) = duckdb::Connection::open(DB_PATH) {
                    let _ = db.execute_batch("CREATE TABLE IF NOT EXISTS gold_sentiment (updated_utc VARCHAR, payload VARCHAR)");
                    let _ = db.execute("INSERT INTO gold_sentiment VALUES (?, ?)", duckdb::params![updated, payload]);
                }
            }
        }).await;
    }
    Ok(val)
}

/// Load the most recent stored gold sentiment (so the panel is persistent across
/// restarts / tab switches). Returns null if none has been computed yet.
#[tauri::command]
async fn get_last_gold_sentiment(state: tauri::State<'_, AppState>) -> Result<Option<serde_json::Value>, String> {
    let db_mutex = state.db_mutex.clone();
    let payload: Option<String> = tokio::task::spawn_blocking(move || {
        let _lock = db_mutex.lock().ok()?;
        let db = duckdb::Connection::open(DB_PATH).ok()?;
        let _ = db.execute_batch("CREATE TABLE IF NOT EXISTS gold_sentiment (updated_utc VARCHAR, payload VARCHAR)");
        db.query_row("SELECT payload FROM gold_sentiment ORDER BY updated_utc DESC LIMIT 1", [], |r| r.get::<_, String>(0)).ok()
    }).await.unwrap_or(None);
    Ok(payload.and_then(|p| serde_json::from_str(&p).ok()))
}

/// Tauri command: fetch one article's body on demand (when a user opens it in the
/// modal). Hits the econcal proxy API, parses the HTML field, strips tags, and
/// updates `news_historical.body` so subsequent loads are free.
#[tauri::command]
async fn fetch_article_body_on_demand(
    article_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<ArticleBodyResult, String> {
    if !ensure_econcal_alive().await {
        return Ok(ArticleBodyResult {
            article_id,
            body: None,
            status: "failed".into(),
            message: Some("Econcal proxy not reachable.".into()),
        });
    }

    // Check if we already have it.
    let db_mutex = state.db_mutex.clone();
    let aid = article_id.clone();
    let existing: Option<String> = tokio::task::spawn_blocking(move || {
        let _lock = db_mutex.lock().ok()?;
        let db = duckdb::Connection::open(DB_PATH).ok()?;
        db.query_row(
            "SELECT body FROM news_historical WHERE article_id = ?",
            duckdb::params![aid],
            |row| row.get::<_, Option<String>>(0),
        ).ok().flatten()
    }).await.unwrap_or(None);

    if let Some(b) = existing.as_ref() {
        if !b.is_empty() {
            return Ok(ArticleBodyResult {
                article_id,
                body: Some(b.clone()),
                status: "ok".into(),
                message: None,
            });
        }
    }

    // Fetch from proxy.
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0")
        .build()
        .map_err(|e| e.to_string())?;
    let outcome = fetch_one_body(&client, &article_id).await;

    let (status, body_to_store): (&'static str, Option<String>) = match &outcome {
        BodyFetch::Ok(b) => ("ok", Some(b.clone())),
        BodyFetch::EmptyBody => ("empty", Some(String::new())),
        BodyFetch::Failed | BodyFetch::RateLimited { .. } => ("failed", None),
    };

    if let Some(b) = body_to_store.clone() {
        let db_mutex = state.db_mutex.clone();
        let aid = article_id.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let _lock = db_mutex.lock().ok()?;
            let db = duckdb::Connection::open(DB_PATH).ok()?;
            db.execute(
                "UPDATE news_historical SET body = ? WHERE article_id = ?",
                duckdb::params![b, aid],
            ).ok()
        }).await;
    }

    let returned_body = match outcome {
        BodyFetch::Ok(b) => Some(b),
        _ => None,
    };
    Ok(ArticleBodyResult { article_id, body: returned_body, status: status.into(), message: None })
}

// ── Background price refresh: keeps xauusd_m1/m5/h1/d1 caches fresh ──────────
//
// Spawned once after the cTrader subscription is confirmed. Polls fresh bars
// per timeframe on a sensible cadence (more frequent for lower TFs) so the
// Trade Ideas agent and any other DB reader always sees recent prices —
// without the user having to open the chart.

const REFRESH_TICK_SECS: u64 = 30;
const REFRESH_M1_INTERVAL_SECS: u64 = 60;        // every minute
const REFRESH_M5_INTERVAL_SECS: u64 = 5 * 60;    // every 5 min
const REFRESH_H1_INTERVAL_SECS: u64 = 30 * 60;   // every 30 min
const REFRESH_D1_INTERVAL_SECS: u64 = 6 * 3600;  // every 6 hours

/// Return true if the current UTC moment is inside the spot-gold market's
/// weekly closure (Friday 22:00 UTC → Sunday 22:00 UTC). During closure,
/// cTrader returns the same bars repeatedly, so we skip API calls.
fn xauusd_is_market_closed() -> bool {
    use chrono::{Datelike, Timelike, Weekday};
    let now = chrono::Utc::now();
    let h = now.hour();
    match now.weekday() {
        Weekday::Sat => true,
        Weekday::Sun => h < 22,
        Weekday::Fri => h >= 22,
        _ => false,
    }
}

/// One-shot history backfill. Spawned at session start; walks each TF
/// backwards in 1000-bar chunks until either the per-TF depth cap is reached
/// or cTrader's history runs out. Per-TF caps balance "useful coverage" vs.
/// "bandwidth and time": minute scales only go back a few months (rarely
/// useful for trading older than that), daily and above stretch to decades.
///
/// Smart skipping per TF:
///   - already complete (tail fresh + head at cap) → skip
///   - only tail stale → walk forward gap only (1-few chunks)
///   - head not at cap → walk all the way back
async fn run_history_backfill(symbol: String, shared_db: SharedDb) {
    // Wait for the symbol map.
    let mut symbol_id: Option<i64> = None;
    for _ in 0..40 {
        if let Ok(map) = symbol_map().lock() {
            if let Some(id) = map.get(symbol.as_str()).copied() {
                symbol_id = Some(id);
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    let symbol_id = match symbol_id {
        Some(id) => id,
        None => { println!("[backfill] '{}' not in symbol map — aborting", symbol); return; }
    };

    // (tf_name, period, minutes_per_bar, depth_days)
    //
    // depth_days is set to 50 years for every TF — effectively "as far back
    // as cTrader has data." The walk will stop when cTrader returns an empty
    // response (history exhausted) rather than hitting this artificial cap.
    // Practical result: each TF gets its true maximum history. For XAUUSD that
    // means D1/W1/MN1 to ~1998, H1/H4/H12 several years back, M1-M30 whatever
    // the broker retains (varies — typically months for M1, years for M30).
    // Only the 9 TFs our broker (IC Markets) actually exposes via the cTrader
    // Open API. M2/M4/M10/M30/H4 are dropped — the broker returned empty
    // for them, so they were always empty in the DB and just noise in the UI.
    const FULL_DEPTH_DAYS: i64 = 50 * 365;
    let targets: Vec<(&str, openapi::ProtoOaTrendbarPeriod, i64, i64)> = vec![
        ("m1",  openapi::ProtoOaTrendbarPeriod::M1,  1,            FULL_DEPTH_DAYS),
        ("m3",  openapi::ProtoOaTrendbarPeriod::M3,  3,            FULL_DEPTH_DAYS),
        ("m5",  openapi::ProtoOaTrendbarPeriod::M5,  5,            FULL_DEPTH_DAYS),
        ("m15", openapi::ProtoOaTrendbarPeriod::M15, 15,           FULL_DEPTH_DAYS),
        ("h1",  openapi::ProtoOaTrendbarPeriod::H1,  60,           FULL_DEPTH_DAYS),
        ("h12", openapi::ProtoOaTrendbarPeriod::H12, 720,          FULL_DEPTH_DAYS),
        ("d1",  openapi::ProtoOaTrendbarPeriod::D1,  60 * 24,      FULL_DEPTH_DAYS),
        ("w1",  openapi::ProtoOaTrendbarPeriod::W1,  60 * 24 * 7,  FULL_DEPTH_DAYS),
        ("mn1", openapi::ProtoOaTrendbarPeriod::Mn1, 60 * 24 * 30, FULL_DEPTH_DAYS),
    ];

    println!("[backfill] history backfill starting for {} ({} TFs)", symbol, targets.len());
    let overall_start = std::time::Instant::now();

    // Reset + initialize the global state the UI panel polls.
    let tfs_total = targets.len();
    with_backfill_state(|s| {
        *s = BackfillState {
            status: "running".into(),
            started_at_utc: Some(chrono::Utc::now().to_rfc3339()),
            tfs_total,
            ..BackfillState::default()
        };
    });

    for (tf_name, period, minutes_per_bar, depth_days) in targets {
        let table = format!("{}_{}", symbol.to_lowercase(), tf_name);

        // Read MIN + MAX from the existing table (or None if empty).
        let (existing_min, existing_max): (Option<i64>, Option<i64>) = {
            let db = shared_db.clone();
            let table_q = table.clone();
            match tokio::task::spawn_blocking(move || -> (Option<i64>, Option<i64>) {
                let _lock = match db.lock() { Ok(l) => l, Err(_) => return (None, None) };
                let conn = match duckdb::Connection::open(DB_PATH) { Ok(c) => c, Err(_) => return (None, None) };
                let _ = ensure_candle_table(&conn, &table_q);
                let mn: Option<i64> = conn.query_row(
                    &format!("SELECT MIN(timestamp) FROM {}", table_q),
                    [], |row| row.get::<_, Option<i64>>(0)
                ).ok().flatten();
                let mx: Option<i64> = conn.query_row(
                    &format!("SELECT MAX(timestamp) FROM {}", table_q),
                    [], |row| row.get::<_, Option<i64>>(0)
                ).ok().flatten();
                (mn, mx)
            }).await {
                Ok(t) => t,
                Err(_) => (None, None),
            }
        };

        let now_ms = chrono::Utc::now().timestamp_millis();
        let now_secs = now_ms / 1000;
        let cap_secs = now_secs - depth_days * 86400;
        let bucket_secs = minutes_per_bar * 60;
        let tail_fresh = existing_max.map(|m| (now_secs - m) < bucket_secs * 2).unwrap_or(false);
        let head_at_cap = existing_min.map(|m| m <= cap_secs).unwrap_or(false);

        // Already complete → skip entirely.
        if tail_fresh && head_at_cap {
            println!("[backfill] {} already complete (min {} / max age {}s) — skipping",
                     tf_name, existing_min.unwrap_or(0),
                     existing_max.map(|m| now_secs - m).unwrap_or(0));
            let tf_owned = tf_name.to_string();
            with_backfill_state(|s| s.tfs_skipped.push(tf_owned));
            continue;
        }

        // Reset per-TF state. Counters accumulate across BOTH passes
        // (forward + backward) so the UI shows total work per TF.
        {
            let tf_owned = tf_name.to_string();
            with_backfill_state(|s| {
                s.current_tf = Some(tf_owned);
                s.current_mode = None;
                s.chunks_this_tf = 0;
                s.bars_this_tf = 0;
                s.last_chunk_oldest_utc = None;
            });
        }

        let mut chunks = 0usize;
        let mut total_bars = 0usize;
        let tf_start = std::time::Instant::now();

        // Two-mode per-TF logic:
        //
        //   * Empty table (existing_max is None)
        //     → full first-build walk from `now` backwards to the cap,
        //       in parallel 10k×4 batches (multi-minute work for M1 etc.).
        //
        //   * Has data (existing_max is Some)
        //     → incremental ONLY: one small request asking cTrader for
        //       the gap between existing_max and now. No re-walking
        //       through bars we already have. This is what makes a
        //       re-click of `XAUUSD_History_Update` cheap (~1-2 sec
        //       total across all 9 TFs).
        //
        // If the user ever wants to RE-extend history deeper (e.g. cTrader
        // policy now allows older bars), they can drop the affected table
        // and re-click; the empty-table branch will do the deep walk again.
        match existing_max {
            Some(max_secs) => {
                let gap_secs = now_secs - max_secs;
                if gap_secs <= bucket_secs {
                    // Tail already within one bar of now — nothing to do.
                    println!("[backfill] {} already current (tail {}s old) — skipping", tf_name, gap_secs);
                } else {
                    println!("[backfill] {} incremental: gap {}s ({} bars)",
                             tf_name, gap_secs, gap_secs / bucket_secs);
                    with_backfill_state(|s| s.current_mode = Some("incremental".to_string()));
                    let (c, b) = fetch_incremental_gap(
                        tf_name.to_string(), symbol_id, period, bucket_secs, table.clone(),
                        shared_db.clone(), max_secs, now_secs,
                    ).await;
                    chunks += c;
                    total_bars += b;
                }
            }
            None => {
                println!("[backfill] {} first-build: walking from now to cap", tf_name);
                with_backfill_state(|s| s.current_mode = Some("first-build".to_string()));
                let (c, b) = walk_backwards_batches(
                    tf_name.to_string(), symbol_id, period, bucket_secs, table.clone(),
                    shared_db.clone(), now_ms, cap_secs,
                ).await;
                chunks += c;
                total_bars += b;
            }
        }
        // (Note: head_at_cap / existing_min are still used in the skip check at
        // the top to short-circuit fully-current TFs without entering this
        // match. Unused locals would lint but `_ = existing_min;` keeps clippy
        // happy without bringing back the deep-extend pass.)
        let _ = existing_min;

        let secs = tf_start.elapsed().as_secs();
        println!("[backfill] {} done: {} chunks, {} bars, {}s", tf_name, chunks, total_bars, secs);

        let tf_owned = tf_name.to_string();
        with_backfill_state(|s| {
            s.tfs_completed.push(tf_owned);
            s.current_tf = None;
            s.current_mode = None;
            s.chunks_this_tf = 0;
            s.bars_this_tf = 0;
        });
    }

    let total_secs = overall_start.elapsed().as_secs();
    println!("[backfill] complete: {}s wallclock", total_secs);
    with_backfill_state(|s| {
        s.status = "complete".into();
        s.completed_at_utc = Some(chrono::Utc::now().to_rfc3339());
        s.current_tf = None;
        s.current_mode = None;
    });
}

/// Incremental tail update: one cTrader request for the bars between
/// `from_secs` (the existing DB max) and `to_secs` (now). No batch walk —
/// just a single targeted fetch sized to the gap, ~1 RTT total.
/// Returns `(chunks_added, bars_added)`.
async fn fetch_incremental_gap(
    tf_name: String,
    symbol_id: i64,
    period: openapi::ProtoOaTrendbarPeriod,
    bucket_secs: i64,
    table: String,
    shared_db: SharedDb,
    from_secs: i64,
    to_secs: i64,
) -> (usize, usize) {
    let tx = match CHART_REQ_TX.get() { Some(t) => t.clone(), None => return (0, 0) };

    // Ask for `gap_bars + 5` to cover any clock drift / partial bar; cap to
    // cTrader's max (10_000) and floor to a sensible minimum of 5.
    let gap_secs = (to_secs - from_secs).max(0);
    let gap_bars = (gap_secs / bucket_secs) + 5;
    let count = (gap_bars as u32).clamp(5, 10_000);

    let from_ms = from_secs * 1000;
    let to_ms = to_secs * 1000;

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    if tx.send(ChartRequest {
        symbol_id, period, from_ms, to_ms, count, reply: reply_tx,
    }).await.is_err() {
        return (0, 0);
    }

    let bars = match tokio::time::timeout(std::time::Duration::from_secs(30), reply_rx).await {
        Ok(Ok(Ok(b))) => b,
        _ => {
            println!("[backfill] {} incremental fetch failed/timeout", tf_name);
            return (0, 0);
        }
    };

    if bars.is_empty() {
        println!("[backfill] {} incremental: 0 new bars (cTrader has nothing newer)", tf_name);
        return (0, 0);
    }

    let n = bars.len();
    let oldest_secs = bars.iter().map(|c| c.timestamp).min().unwrap_or(0);
    let oldest_iso = chrono::DateTime::<chrono::Utc>::from_timestamp(oldest_secs, 0)
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S").to_string());

    // Upsert.
    let db = shared_db.clone();
    let table_w = table.clone();
    let _ = tokio::task::spawn_blocking(move || {
        if let Ok(_lock) = db.lock() {
            if let Ok(conn) = duckdb::Connection::open(DB_PATH) {
                let _ = ensure_candle_table(&conn, &table_w);
                let _ = upsert_candles(&conn, &table_w, &bars);
            }
        }
    }).await;

    println!("[backfill] {} incremental: +{} bars", tf_name, n);

    with_backfill_state(|s| {
        s.chunks_this_tf += 1;
        s.bars_this_tf += n;
        s.total_bars += n;
        s.last_chunk_oldest_utc = oldest_iso;
    });

    (1, n)
}

/// One parallel-batch walk backwards from `start_cursor_ms` toward `floor_secs`.
/// Returns `(chunks_added, bars_added)`. Used by `run_history_backfill` for
/// both the forward-fill pass and the backward-extend pass.
async fn walk_backwards_batches(
    tf_name: String,
    symbol_id: i64,
    period: openapi::ProtoOaTrendbarPeriod,
    bucket_secs: i64,
    table: String,
    shared_db: SharedDb,
    start_cursor_ms: i64,
    floor_secs: i64,
) -> (usize, usize) {
    const BATCH_SIZE: usize = 4;
    let count = 10_000u32;
    let mut cursor_ms = start_cursor_ms;
    let mut chunks = 0usize;
    let mut total_bars = 0usize;

    'tf_walk: loop {
            // Pre-compute up to BATCH_SIZE adjacent windows, all reaching back
            // from `cursor_ms` toward the floor. Each window is
            // `count * bucket_secs` seconds wide.
            let chunk_span_ms = (count as i64) * bucket_secs * 1000;
            let mut windows: Vec<(i64, i64)> = Vec::with_capacity(BATCH_SIZE);
            let mut tentative_cursor = cursor_ms;
            for _ in 0..BATCH_SIZE {
                let to_ms = tentative_cursor;
                let from_ms = tentative_cursor - chunk_span_ms;
                windows.push((from_ms, to_ms));
                tentative_cursor = from_ms - 1000;
                if (tentative_cursor / 1000) <= floor_secs { break; }
            }
            if windows.is_empty() { break 'tf_walk; }

            // Fire all windows in parallel.
            let tx = match CHART_REQ_TX.get() { Some(t) => t.clone(), None => break 'tf_walk };
            let mut tasks = tokio::task::JoinSet::new();
            for (from_ms, to_ms) in windows {
                let tx = tx.clone();
                tasks.spawn(async move {
                    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                    if tx.send(ChartRequest {
                        symbol_id, period, from_ms, to_ms, count, reply: reply_tx,
                    }).await.is_err() {
                        return Err("send failed".to_string());
                    }
                    match tokio::time::timeout(std::time::Duration::from_secs(30), reply_rx).await {
                        Ok(Ok(Ok(b))) => Ok(b),
                        Ok(Ok(Err(e))) => Err(format!("chunk error: {}", e)),
                        _ => Err("chunk timeout".to_string()),
                    }
                });
            }

            // Collect all results. Track the overall oldest_secs for cursor advance.
            let mut batch_all_empty = true;
            let mut batch_overall_oldest_secs: Option<i64> = None;
            while let Some(joined) = tasks.join_next().await {
                let bars = match joined {
                    Ok(Ok(b)) => b,
                    Ok(Err(e)) => { println!("[backfill] {} batch chunk: {}", tf_name, e); continue; }
                    Err(e) => { println!("[backfill] {} join: {}", tf_name, e); continue; }
                };

                if bars.is_empty() { continue; }
                batch_all_empty = false;

                let n = bars.len();
                let oldest_secs = bars.iter().map(|c| c.timestamp).min().unwrap_or(0);
                batch_overall_oldest_secs = Some(match batch_overall_oldest_secs {
                    Some(prev) => prev.min(oldest_secs),
                    None => oldest_secs,
                });

                total_bars += n;
                chunks += 1;

                // Upsert this chunk's bars.
                let db = shared_db.clone();
                let table_w = table.clone();
                let bars_clone = bars.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    if let Ok(_lock) = db.lock() {
                        if let Ok(conn) = duckdb::Connection::open(DB_PATH) {
                            let _ = ensure_candle_table(&conn, &table_w);
                            let _ = upsert_candles(&conn, &table_w, &bars_clone);
                        }
                    }
                }).await;

                println!("[backfill] {} chunk {} ({} bars) oldest_ts={}", tf_name, chunks, n, oldest_secs);

                // Live progress for the UI panel.
                let oldest_iso = chrono::DateTime::<chrono::Utc>::from_timestamp(oldest_secs, 0)
                    .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S").to_string());
                let chunks_snap = chunks;
                let bars_this_tf_snap = total_bars;
                with_backfill_state(|s| {
                    s.chunks_this_tf = chunks_snap;
                    s.bars_this_tf = bars_this_tf_snap;
                    s.total_bars += n;
                    // Only update the displayed "oldest so far" if this chunk
                    // is actually older — keeps the UI's running min sensible.
                    if let (Some(curr), Some(new)) = (s.last_chunk_oldest_utc.clone(), oldest_iso.clone()) {
                        if new < curr { s.last_chunk_oldest_utc = oldest_iso.clone(); }
                    } else if s.last_chunk_oldest_utc.is_none() {
                        s.last_chunk_oldest_utc = oldest_iso.clone();
                    }
                });
            }

            // Decide next move.
            if batch_all_empty {
                println!("[backfill] {} cTrader returned empty — history exhausted ({} chunks, {} bars)",
                         tf_name, chunks, total_bars);
                break 'tf_walk;
            }

            // Advance cursor past the oldest bar we saw this batch.
            // If we never got an oldest_secs (shouldn't happen since !batch_all_empty),
            // bail to avoid infinite loop.
            let next_oldest_secs = match batch_overall_oldest_secs {
                Some(v) => v,
                None => break 'tf_walk,
            };

            if next_oldest_secs <= floor_secs { break 'tf_walk; }
            cursor_ms = next_oldest_secs * 1000 - 1000;

            // Tiny inter-batch gap so cTrader sees a small breath between
            // bursts. 50 ms is small enough that it doesn't dominate time
            // but prevents any spike-detection on their side.
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    (chunks, total_bars)
}

/// Background loop: every 30s checks which TFs are due for a refresh, then
/// requests a small tail window from cTrader via the existing chart channel
/// and upserts it into the per-TF table. Skips API calls during weekend
/// closure.
async fn run_price_refresh_loop(symbol: String, shared_db: SharedDb) {
    // Wait for the symbol map to populate (SymbolMapping arrives asynchronously
    // via the price channel after subscribe). Poll up to ~10s.
    let mut symbol_id: Option<i64> = None;
    for _ in 0..40 {
        if let Ok(map) = symbol_map().lock() {
            if let Some(id) = map.get(symbol.as_str()).copied() {
                symbol_id = Some(id);
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    let symbol_id = match symbol_id {
        Some(id) => id,
        None => { println!("[refresh] '{}' not in symbol map after wait — aborting loop", symbol); return; }
    };
    println!("[refresh] background price refresh loop started for {} (id {})", symbol, symbol_id);

    // (tf-name, period enum, minutes per bar, "last refreshed" cursor, interval secs, count to fetch)
    let mut targets: Vec<(&str, openapi::ProtoOaTrendbarPeriod, i64, tokio::time::Instant, u64, u32)> = vec![
        ("m1", openapi::ProtoOaTrendbarPeriod::M1, 1,        tokio::time::Instant::now() - std::time::Duration::from_secs(REFRESH_M1_INTERVAL_SECS), REFRESH_M1_INTERVAL_SECS, 120),
        ("m5", openapi::ProtoOaTrendbarPeriod::M5, 5,        tokio::time::Instant::now() - std::time::Duration::from_secs(REFRESH_M5_INTERVAL_SECS), REFRESH_M5_INTERVAL_SECS, 60),
        ("h1", openapi::ProtoOaTrendbarPeriod::H1, 60,       tokio::time::Instant::now() - std::time::Duration::from_secs(REFRESH_H1_INTERVAL_SECS), REFRESH_H1_INTERVAL_SECS, 60),
        ("d1", openapi::ProtoOaTrendbarPeriod::D1, 60 * 24,  tokio::time::Instant::now() - std::time::Duration::from_secs(REFRESH_D1_INTERVAL_SECS), REFRESH_D1_INTERVAL_SECS, 30),
    ];

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(REFRESH_TICK_SECS)).await;

        if xauusd_is_market_closed() {
            // Skip silently; check again next tick.
            continue;
        }

        let tx = match CHART_REQ_TX.get() { Some(t) => t.clone(), None => continue };

        let now = tokio::time::Instant::now();
        for entry in targets.iter_mut() {
            let (tf, period, minutes_per_bar, last, interval, count) = (entry.0, entry.1, entry.2, entry.3, entry.4, entry.5);
            if now.duration_since(last).as_secs() < interval { continue; }

            let end_ms = chrono::Utc::now().timestamp_millis();
            let from_ms = end_ms - (count as i64) * minutes_per_bar * 60 * 1000;
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            if tx.send(ChartRequest {
                symbol_id, period, from_ms, to_ms: end_ms, count, reply: reply_tx,
            }).await.is_err() {
                continue;
            }
            match tokio::time::timeout(std::time::Duration::from_secs(30), reply_rx).await {
                Ok(Ok(Ok(bars))) => {
                    let n = bars.len();
                    let table = format!("{}_{}", symbol.to_lowercase(), tf);
                    let db = shared_db.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        if let Ok(_lock) = db.lock() {
                            if let Ok(conn) = duckdb::Connection::open(DB_PATH) {
                                let _ = ensure_candle_table(&conn, &table);
                                let _ = upsert_candles(&conn, &table, &bars);
                            }
                        }
                    }).await;
                    entry.3 = now;
                    println!("[refresh] {} {} bars upserted", tf, n);
                }
                _ => {
                    // Don't update the cursor on failure — retry next tick.
                    println!("[refresh] {} fetch failed/timeout — will retry", tf);
                }
            }
        }
    }
}

// ── Live tick recorder ───────────────────────────────────────────────────────
// Persist every XAUUSD bid/ask/spread tick to DuckDB with a rolling 2-week
// window. Ticks are buffered in memory and flushed in small batches so we never
// take the DB lock per tick; old rows are pruned periodically.
const TICK_RETENTION_DAYS: i64 = 14;
const TICK_FLUSH_SECS: u64 = 2;
const TICK_PRUNE_SECS: u64 = 300;
const TICK_TABLE: &str = "xauusd_ticks_live";
const CREATE_TICK_TABLE: &str =
    "CREATE TABLE IF NOT EXISTS xauusd_ticks_live (timestamp_ms BIGINT, bid DOUBLE, ask DOUBLE, spread DOUBLE);";

static TICK_BUF: std::sync::OnceLock<std::sync::Mutex<Vec<(i64, f64, f64)>>> = std::sync::OnceLock::new();
fn tick_buf() -> &'static std::sync::Mutex<Vec<(i64, f64, f64)>> {
    TICK_BUF.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// Buffer one XAUUSD tick — called from the price bridge on every spot update.
/// Cheap (push under a short-lived lock); the writer task does the I/O.
fn record_xauusd_tick(bid: f64, ask: f64) {
    if let Ok(mut b) = tick_buf().lock() {
        if b.len() < 1_000_000 {  // safety cap if the writer ever stalls
            b.push((chrono::Utc::now().timestamp_millis(), bid, ask));
        }
    }
}

/// Background writer: flush buffered ticks into `xauusd_ticks_live` every few
/// seconds and prune rows older than the retention window. Runs for the life of
/// the process.
async fn run_tick_recorder(db_mutex: SharedDb) {
    use std::time::{Duration, Instant};
    let mut last_prune = Instant::now();
    loop {
        tokio::time::sleep(Duration::from_secs(TICK_FLUSH_SECS)).await;

        let batch: Vec<(i64, f64, f64)> = match tick_buf().lock() {
            Ok(mut b) if !b.is_empty() => std::mem::take(&mut *b),
            _ => Vec::new(),
        };
        let do_prune = last_prune.elapsed() >= Duration::from_secs(TICK_PRUNE_SECS);
        if batch.is_empty() && !do_prune { continue; }
        if do_prune { last_prune = Instant::now(); }

        let db = db_mutex.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let Ok(_lock) = db.lock() else { return };
            let Ok(conn) = duckdb::Connection::open(DB_PATH) else { return };
            let _ = conn.execute_batch(CREATE_TICK_TABLE);
            if !batch.is_empty() {
                if let Ok(mut stmt) = conn.prepare(&format!("INSERT INTO {TICK_TABLE} VALUES (?,?,?,?)")) {
                    for (ts, bid, ask) in &batch {
                        let _ = stmt.execute(duckdb::params![ts, bid, ask, ask - bid]);
                    }
                }
            }
            if do_prune {
                let cutoff = chrono::Utc::now().timestamp_millis()
                    - TICK_RETENTION_DAYS * 24 * 3600 * 1000;
                let _ = conn.execute(
                    &format!("DELETE FROM {TICK_TABLE} WHERE timestamp_ms < ?"),
                    duckdb::params![cutoff],
                );
            }
        }).await;
    }
}

// ── Trade Ideas: live snapshot helpers + 4-model fan-out ─────────────

/// Fetch `count` trendbars for `symbol_id` at `period` ending now, live from
/// cTrader via the chart request channel — the same path the chart uses. Bars
/// are returned oldest-first. Used to build the Trade Idea snapshot now that
/// the DuckDB candle tables are gone (charts are live-only).
async fn fetch_trendbars_for_snapshot(
    symbol_id: i64,
    period: openapi::ProtoOaTrendbarPeriod,
    minutes_per_bar: i64,
    count: u32,
) -> Result<Vec<Candle>, String> {
    let tx = CHART_REQ_TX.get().ok_or("chart channel not initialised")?;
    let end_ms = chrono::Utc::now().timestamp_millis();
    let from_ms = end_ms - (count as i64) * minutes_per_bar * 60 * 1000;
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    tx.send(ChartRequest { symbol_id, period, from_ms, to_ms: end_ms, count, reply: reply_tx })
        .await
        .map_err(|e| format!("send chart req: {}", e))?;
    match tokio::time::timeout(std::time::Duration::from_secs(30), reply_rx).await {
        Ok(Ok(Ok(mut bars))) => {
            bars.sort_by_key(|c| c.timestamp);
            Ok(bars)
        }
        Ok(Ok(Err(e))) => Err(e),
        Ok(Err(_))      => Err("chart request was cancelled".into()),
        Err(_)          => Err("cTrader response timeout".into()),
    }
}

/// Fetch trendbars for an explicit [from_ms, to_ms] window (oldest-first). Same
/// live cTrader path as the snapshot fetch, but with an explicit time range —
/// used to gather the multi-day M5 history for the volume profile.
async fn fetch_trendbars_range(
    symbol_id: i64,
    period: openapi::ProtoOaTrendbarPeriod,
    from_ms: i64,
    to_ms: i64,
    count: u32,
) -> Result<Vec<Candle>, String> {
    let tx = CHART_REQ_TX.get().ok_or("chart channel not initialised")?;
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    tx.send(ChartRequest { symbol_id, period, from_ms, to_ms, count, reply: reply_tx })
        .await
        .map_err(|e| format!("send chart req: {}", e))?;
    match tokio::time::timeout(std::time::Duration::from_secs(30), reply_rx).await {
        Ok(Ok(Ok(mut bars))) => { bars.sort_by_key(|c| c.timestamp); Ok(bars) }
        Ok(Ok(Err(e))) => Err(e),
        Ok(Err(_))      => Err("chart request was cancelled".into()),
        Err(_)          => Err("cTrader response timeout".into()),
    }
}

/// Fetch ~`days` of M5 candles live from cTrader for the volume profile, in
/// ~5-day chunks to stay under the broker's per-request trendbar cap. Returns
/// oldest-first, de-duplicated by timestamp.
async fn fetch_m5_window(symbol_id: i64, days: i64) -> Vec<Candle> {
    use openapi::ProtoOaTrendbarPeriod as P;
    let day_ms = 24 * 60 * 60 * 1000;
    let now = chrono::Utc::now().timestamp_millis();
    let start = now - days * day_ms;
    let mut all: Vec<Candle> = Vec::new();
    let mut end = now;
    while end > start {
        let chunk_start = (end - 5 * day_ms).max(start);
        let count = (((end - chunk_start) / (5 * 60 * 1000)) as u32) + 16;
        if let Ok(mut bars) = fetch_trendbars_range(symbol_id, P::M5, chunk_start, end, count).await {
            all.append(&mut bars);
        }
        end = chunk_start;
    }
    all.sort_by_key(|c| c.timestamp);
    all.dedup_by_key(|c| c.timestamp);
    all
}

/// Serialize a candle slice into compact `{ts,o,h,l,c}` JSON objects (oldest-first).
fn candles_to_json(bars: &[Candle]) -> Vec<serde_json::Value> {
    bars.iter().map(|c| serde_json::json!({
        "ts": c.timestamp,
        "o": c.open, "h": c.high, "l": c.low, "c": c.close,
    })).collect()
}

/// Session anchor for intraday VWAP — resets at 21:00 UTC (CME globex / spot-gold
/// daily reopen), matching the chart's `VWAP_SESSION_OFFSET_SEC`.
const VWAP_SESSION_OFFSET_SEC: i64 = 21 * 3600;
fn vwap_session_day(ts: i64) -> i64 { (ts - VWAP_SESSION_OFFSET_SEC).div_euclid(86400) }
fn round2(x: f64) -> f64 { (x * 100.0).round() / 100.0 }

/// Annotate candles with session-anchored VWAP (HLC3, resets 21:00 UTC) and an
/// 8-period EMA of close — the two indicators the day-trade strategy keys off.
/// Returns `(json_with_vwap_ema, last_vwap, last_ema8)` so the caller can also
/// surface the current values at the top of the snapshot.
fn candles_with_indicators(bars: &[Candle]) -> (Vec<serde_json::Value>, Option<f64>, Option<f64>) {
    let alpha = 2.0 / (8.0 + 1.0);
    let mut cum_pv = 0.0;
    let mut cum_v = 0.0;
    let mut cur_day = i64::MIN;
    let mut ema: Option<f64> = None;
    let mut last_vwap: Option<f64> = None;
    let mut out = Vec::with_capacity(bars.len());
    for c in bars {
        let day = vwap_session_day(c.timestamp);
        if day != cur_day { cum_pv = 0.0; cum_v = 0.0; cur_day = day; }
        let typical = (c.high + c.low + c.close) / 3.0;
        let vol = c.volume.max(0) as f64;
        cum_pv += typical * vol;
        cum_v += vol;
        let vwap = if cum_v > 0.0 { cum_pv / cum_v } else { typical };
        ema = Some(match ema {
            Some(prev) => alpha * c.close + (1.0 - alpha) * prev,
            None => c.close,
        });
        last_vwap = Some(vwap);
        out.push(serde_json::json!({
            "ts": c.timestamp, "o": c.open, "h": c.high, "l": c.low, "c": c.close,
            "vwap": round2(vwap),
            "ema8": ema.map(round2),
        }));
    }
    (out, last_vwap.map(round2), ema.map(round2))
}

/// High / low of the most recent 21:00-UTC session from a candle slice. Used as
/// the "session high / low" reference the strategy retests against.
fn session_high_low(bars: &[Candle]) -> (Option<f64>, Option<f64>) {
    let Some(last) = bars.last() else { return (None, None) };
    let day = vwap_session_day(last.timestamp);
    let mut hi = f64::MIN;
    let mut lo = f64::MAX;
    for c in bars {
        if vwap_session_day(c.timestamp) == day {
            hi = hi.max(c.high);
            lo = lo.min(c.low);
        }
    }
    if hi == f64::MIN { (None, None) } else { (Some(round2(hi)), Some(round2(lo))) }
}

/// ATR(n): simple average of (high − low) over the last `n` bars.
fn atr_simple(bars: &[Candle], n: usize) -> Option<f64> {
    if bars.is_empty() { return None; }
    let take = bars.len().min(n);
    let slice = &bars[bars.len() - take..];
    let sum: f64 = slice.iter().map(|c| c.high - c.low).sum();
    Some(round2(sum / take as f64))
}

// ── Trend-scalp indicators (Claude Blitz) ────────────────────────────────────

/// EMA(period) of a close series; returns the final value. Seeds with the SMA
/// of the first `period` closes for stability, then walks the standard EMA
/// recurrence. None if there aren't at least `period` closes.
fn ema_last(closes: &[f64], period: usize) -> Option<f64> {
    if period == 0 || closes.len() < period { return None; }
    let alpha = 2.0 / (period as f64 + 1.0);
    let mut ema = closes[..period].iter().sum::<f64>() / period as f64;
    for &c in &closes[period..] {
        ema = alpha * c + (1.0 - alpha) * ema;
    }
    Some(round2(ema))
}

/// Wilder's ADX(period) from candle bars — measures TREND STRENGTH (not
/// direction). Needs ~2×period+1 bars. Computed in Rust because LLMs estimate
/// the multi-step Wilder smoothing unreliably from raw OHLC.
fn adx_wilder(bars: &[Candle], period: usize) -> Option<f64> {
    let n = bars.len();
    if period == 0 || n < period * 2 + 1 { return None; }

    // Per-bar True Range and directional movement (indices 1..n).
    let mut tr = Vec::with_capacity(n - 1);
    let mut plus_dm = Vec::with_capacity(n - 1);
    let mut minus_dm = Vec::with_capacity(n - 1);
    for i in 1..n {
        let (h, l) = (bars[i].high, bars[i].low);
        let (ph, pl, pc) = (bars[i - 1].high, bars[i - 1].low, bars[i - 1].close);
        let up = h - ph;
        let down = pl - l;
        plus_dm.push(if up > down && up > 0.0 { up } else { 0.0 });
        minus_dm.push(if down > up && down > 0.0 { down } else { 0.0 });
        tr.push((h - l).max((h - pc).abs()).max((l - pc).abs()));
    }

    // Wilder smoothing: seed = sum of first `period`, then s = s - s/period + x.
    let smooth = |v: &[f64]| -> Vec<f64> {
        let mut out = Vec::new();
        if v.len() < period { return out; }
        let mut s: f64 = v[..period].iter().sum();
        out.push(s);
        for &x in &v[period..] {
            s = s - s / period as f64 + x;
            out.push(s);
        }
        out
    };
    let str_ = smooth(&tr);
    let sdm_p = smooth(&plus_dm);
    let sdm_m = smooth(&minus_dm);
    if str_.is_empty() { return None; }

    // DX per smoothed step, then Wilder-average DX into ADX.
    let mut dx = Vec::with_capacity(str_.len());
    for i in 0..str_.len() {
        if str_[i] == 0.0 { dx.push(0.0); continue; }
        let di_p = 100.0 * sdm_p[i] / str_[i];
        let di_m = 100.0 * sdm_m[i] / str_[i];
        let sum = di_p + di_m;
        dx.push(if sum == 0.0 { 0.0 } else { 100.0 * (di_p - di_m).abs() / sum });
    }
    if dx.len() < period { return None; }
    let mut adx = dx[..period].iter().sum::<f64>() / period as f64;
    for &d in &dx[period..] {
        adx = (adx * (period as f64 - 1.0) + d) / period as f64;
    }
    Some((adx * 10.0).round() / 10.0)
}

/// Short-term direction of a correlated instrument: returns (last_close,
/// pct_change_over_lookback_bars, "up"|"down"|"flat"). Used for the USDJPY /
/// XAGUSD context filter. ±0.05% dead-band keeps noise from reading as a trend.
fn series_dir(bars: &[Candle], lookback: usize) -> (Option<f64>, Option<f64>, &'static str) {
    let Some(last) = bars.last() else { return (None, None, "flat") };
    if bars.len() < 2 { return (Some(round2(last.close)), None, "flat"); }
    let k = bars.len().saturating_sub(lookback + 1);
    let ref_close = bars[k].close;
    if ref_close == 0.0 { return (Some(round2(last.close)), None, "flat"); }
    let chg = (last.close - ref_close) / ref_close * 100.0;
    let dir = if chg > 0.05 { "up" } else if chg < -0.05 { "down" } else { "flat" };
    (Some(round2(last.close)), Some((chg * 100.0).round() / 100.0), dir)
}

/// Coarse UTC session label for the scalp context filter.
fn session_label(now: chrono::DateTime<chrono::Utc>) -> &'static str {
    use chrono::Timelike;
    match now.hour() {
        22..=23 | 0..=5 => "Asia",
        6..=12          => "London",
        13..=15         => "London+NY overlap",
        16..=19         => "NY pm",
        _               => "late NY / pre-Asia",
    }
}

#[cfg(test)]
mod blitz_indicator_tests {
    use super::*;
    use db::Candle;

    fn c(close: f64, high: f64, low: f64) -> Candle {
        Candle::new(0, close, high, low, close, 100)
    }

    #[test]
    fn ema_constant_series_equals_constant() {
        let closes = vec![100.0; 60];
        assert!((ema_last(&closes, 50).unwrap() - 100.0).abs() < 0.01);
    }

    #[test]
    fn ema_none_when_too_few_bars() {
        assert!(ema_last(&[1.0, 2.0, 3.0], 50).is_none());
    }

    #[test]
    fn adx_high_on_strong_uptrend() {
        // Strictly rising highs/lows → all +DM, no -DM → ADX should be very high.
        let bars: Vec<Candle> = (0..40)
            .map(|i| { let b = 2000.0 + i as f64 * 5.0; c(b + 3.5, b + 4.0, b + 0.5) })
            .collect();
        let adx = adx_wilder(&bars, 14).expect("enough bars");
        assert!(adx > 50.0, "strong trend ADX should be high, got {}", adx);
    }

    #[test]
    fn adx_low_on_flat_range() {
        // Constant highs/lows → no directional movement → ADX ~0 (range).
        let bars: Vec<Candle> = (0..40).map(|_| c(2000.0, 2001.0, 1999.0)).collect();
        let adx = adx_wilder(&bars, 14).expect("enough bars");
        assert!(adx < 20.0, "range ADX should be low, got {}", adx);
    }

    #[test]
    fn adx_none_when_too_few_bars() {
        let bars: Vec<Candle> = (0..10).map(|_| c(2000.0, 2001.0, 1999.0)).collect();
        assert!(adx_wilder(&bars, 14).is_none());
    }

    #[test]
    fn series_dir_reads_direction() {
        let up: Vec<Candle> = (0..20).map(|i| c(2000.0 + i as f64, 2000.0, 2000.0)).collect();
        assert_eq!(series_dir(&up, 12).2, "up");
        let down: Vec<Candle> = (0..20).map(|i| c(2000.0 - i as f64, 2000.0, 2000.0)).collect();
        assert_eq!(series_dir(&down, 12).2, "down");
    }
}

/// Build two live snapshots in one pass:
///   - `compact` — tiny (latest M1 + M5 + scalar levels + a few headlines) for
///     the small/fast models (Gemini, DeepSeek, Qwen) so they stay under ~20 s.
///   - `full` — richer (M5+M15+H1 + PDH/PDL/PDC + more news/calendar) for the
///     Claude multi-strategy read.
/// VWAP/8EMA are computed over a full session of M5 bars so the values are
/// correct; only recent slices are shipped.
async fn build_trade_snapshots(
    db_mutex: &SharedDb,
    symbol_id: i64,
) -> Result<(serde_json::Value, serde_json::Value, serde_json::Value), String> {
    use openapi::ProtoOaTrendbarPeriod as P;
    // Correlated instruments for the Blitz context filter: USDJPY (inverse to
    // gold) and XAGUSD (silver, confirms). Resolve ids first so all fetches go
    // in one concurrent batch below.
    let (usdjpy_id, xagusd_id) = match symbol_map().lock() {
        Ok(m) => (m.get("USDJPY").copied(), m.get("XAGUSD").copied()),
        Err(_) => (None, None),
    };
    // ALL fetches in a single concurrent batch so the whole snapshot is ready in
    // ~one round-trip BEFORE any model is called — Claude never waits on a fetch,
    // it reads the finished JSON. M15 = 220 bars so EMA50/EMA200 + ADX(14) are
    // computable on the M15 trend frame for the Blitz 1-min scalper; M1 = 60 for
    // entry timing. Live from cTrader (no DB cache).
    let (m1, m5, m15, h1, d1, usdjpy, xagusd) = tokio::join!(
        fetch_trendbars_for_snapshot(symbol_id, P::M1,  1,  60),
        fetch_trendbars_for_snapshot(symbol_id, P::M5,  5,  288), // ~24h → correct session VWAP
        fetch_trendbars_for_snapshot(symbol_id, P::M15, 15, 220),
        fetch_trendbars_for_snapshot(symbol_id, P::H1,  60, 30),
        fetch_trendbars_for_snapshot(symbol_id, P::D1,  60 * 24, 5),
        async { match usdjpy_id {
            Some(id) => fetch_trendbars_for_snapshot(id, P::H1, 60, 48).await.unwrap_or_default(),
            None => Vec::new(),
        } },
        async { match xagusd_id {
            Some(id) => fetch_trendbars_for_snapshot(id, P::H1, 60, 48).await.unwrap_or_default(),
            None => Vec::new(),
        } },
    );
    let m1 = m1.unwrap_or_default();
    let m5 = m5.unwrap_or_default();
    let m15 = m15.unwrap_or_default();
    let h1 = h1.unwrap_or_default();
    let d1 = d1.unwrap_or_default();
    if m5.is_empty() && m1.is_empty() && h1.is_empty() {
        return Err("no candle data from cTrader (session reconnecting?)".into());
    }

    let (m5_full, vwap_now, ema8_m5) = candles_with_indicators(&m5);
    let (m15_full, _, ema8_m15) = candles_with_indicators(&m15);
    let (sess_hi, sess_lo) = session_high_low(&m5);
    let atr_h1 = atr_simple(&h1, 14);

    let slice_tail = |v: &[serde_json::Value], n: usize| -> Vec<serde_json::Value> {
        v.iter().skip(v.len().saturating_sub(n)).cloned().collect()
    };
    let m1_recent = if m1.len() > 20 { candles_to_json(&m1[m1.len() - 20..]) } else { candles_to_json(&m1) };
    let m5_recent_20 = slice_tail(&m5_full, 20);
    let m5_recent_24 = slice_tail(&m5_full, 24);
    let m15_recent = slice_tail(&m15_full, 32);
    let h1_recent = if h1.len() > 16 { candles_to_json(&h1[h1.len() - 16..]) } else { candles_to_json(&h1) };

    // Prior completed day (D1's last bar is today/in-progress, second-last is yesterday).
    let (pdh, pdl, pdc) = if d1.len() >= 2 {
        let p = &d1[d1.len() - 2];
        (Some(round2(p.high)), Some(round2(p.low)), Some(round2(p.close)))
    } else { (None, None, None) };

    let live_bid = {
        let bits = LATEST_XAUUSD_BID.load(std::sync::atomic::Ordering::Relaxed);
        if bits != 0 { Some(f64::from_bits(bits)) } else { None }
    };
    let last = m1.last().or_else(|| m5.last()).or_else(|| h1.last());
    let price = live_bid.or_else(|| last.map(|c| c.close));
    let last_ts_utc = last.and_then(|c|
        chrono::DateTime::<chrono::Utc>::from_timestamp(c.timestamp, 0).map(|d| d.to_rfc3339()));

    // Live calendar: today + tomorrow, vol≥2, gold currencies — names + times.
    const GOLD_CCYS: [&str; 7] = ["USD", "EUR", "GBP", "JPY", "CHF", "AUD", "CNY"];
    let calendar: Vec<serde_json::Value> = if ensure_econcal_alive().await {
        let start = chrono::Utc::now().format("%Y%m%d").to_string();
        let end = (chrono::Utc::now() + chrono::Duration::days(1)).format("%Y%m%d").to_string();
        match ec_realtime::fetch_events_for_date(&start, &end).await {
            Ok(rows) => rows.into_iter()
                .filter(|r| r.volatility >= 2 && GOLD_CCYS.contains(&r.currency.as_str()))
                .take(8)
                .map(|r| serde_json::json!({ "ts": r.timestamp_utc, "cur": r.currency, "name": r.event_name }))
                .collect(),
            Err(_) => Vec::new(),
        }
    } else { Vec::new() };

    // Recent gold/macro headlines (last 2 days, titles only).
    let cutoff = (chrono::Utc::now() - chrono::Duration::days(2)).format("%Y-%m-%d").to_string();
    let headlines: Vec<String> = {
        let db_mutex = db_mutex.clone();
        tokio::task::spawn_blocking(move || -> Vec<String> {
            let Ok(_lock) = db_mutex.lock() else { return Vec::new() };
            let Ok(db) = duckdb::Connection::open(DB_PATH) else { return Vec::new() };
            let q = "SELECT title FROM news_historical
                     WHERE published_utc >= ?
                       AND (lower(title) LIKE '%gold%' OR lower(title) LIKE '%fed%'
                         OR lower(title) LIKE '%powell%' OR lower(title) LIKE '%dollar%'
                         OR lower(title) LIKE '%yield%'  OR lower(title) LIKE '%cpi%'
                         OR lower(title) LIKE '%inflation%' OR lower(title) LIKE '%pce%')
                     ORDER BY published_utc DESC LIMIT 8";
            let Ok(mut stmt) = db.prepare(q) else { return Vec::new() };
            stmt.query_map([cutoff.as_str()], |row| row.get::<_, String>(0))
                .map(|rows| rows.flatten().collect())
                .unwrap_or_default()
        }).await.unwrap_or_default()
    };

    // ── Blitz (1-min scalp) indicators, computed in Rust ──────────────────
    // Triple-screen: trend = M15 (EMA50/200 + ADX), momentum = M5, entry = M1.
    let m15_closes: Vec<f64> = m15.iter().map(|c| c.close).collect();
    let ema50_m15 = ema_last(&m15_closes, 50);
    let ema200_m15 = ema_last(&m15_closes, 200);
    let adx_m15 = adx_wilder(&m15, 14);
    let atr_m15 = atr_simple(&m15, 14);
    let m1_closes: Vec<f64> = m1.iter().map(|c| c.close).collect();
    let ema9_m1 = ema_last(&m1_closes, 9);
    let ema20_m1 = ema_last(&m1_closes, 20);
    let (usdjpy_last, usdjpy_chg, usdjpy_dir) = series_dir(&usdjpy, 12);
    let (xagusd_last, xagusd_chg, xagusd_dir) = series_dir(&xagusd, 12);
    let price_vs = |e: Option<f64>| -> Option<&'static str> {
        match (price, e) { (Some(p), Some(e)) => Some(if p >= e { "above" } else { "below" }), _ => None }
    };
    let pvs_ema50 = price_vs(ema50_m15);
    let pvs_ema200 = price_vs(ema200_m15);
    let session = session_label(chrono::Utc::now());
    let m1_recent_30 = if m1.len() > 30 { candles_to_json(&m1[m1.len() - 30..]) } else { candles_to_json(&m1) };
    let m15_recent_48 = slice_tail(&m15_full, 48);

    let now_utc = chrono::Utc::now().to_rfc3339();
    let compact = serde_json::json!({
        "now_utc": now_utc,
        "price": price,
        "last_bar_ts_utc": last_ts_utc,
        "vwap": vwap_now,
        "ema8_m5": ema8_m5,
        "ema8_m15": ema8_m15,
        "session_high": sess_hi,
        "session_low": sess_lo,
        "atr_h1": atr_h1,
        "prior_day": { "high": pdh, "low": pdl, "close": pdc },
        "m1_recent": m1_recent,        // {ts,o,h,l,c}
        "m5_recent": m5_recent_20,     // {ts,o,h,l,c,vwap,ema8}
        "calendar_next": calendar.iter().take(5).cloned().collect::<Vec<_>>(),
        "news_headlines": headlines.iter().take(5).cloned().collect::<Vec<_>>(),
    });
    let full = serde_json::json!({
        "now_utc": now_utc,
        "price": price,
        "last_bar_ts_utc": last_ts_utc,
        "vwap": vwap_now,
        "ema8_m5": ema8_m5,
        "ema8_m15": ema8_m15,
        "session_high": sess_hi,
        "session_low": sess_lo,
        "atr_h1": atr_h1,
        "prior_day": { "high": pdh, "low": pdl, "close": pdc },
        "m5_recent": m5_recent_24,     // {ts,o,h,l,c,vwap,ema8}
        "m15_recent": m15_recent,      // {ts,o,h,l,c,vwap,ema8}
        "h1_recent": h1_recent,        // {ts,o,h,l,c}
        "calendar_next": calendar,
        "news_headlines": headlines,
    });
    // Blitz: the 1-min scalp snapshot — pre-computed M15 trend/strength + M5
    // momentum + M1 entry indicators + USDJPY/XAGUSD context (triple-screen).
    let blitz = serde_json::json!({
        "now_utc": now_utc,
        "price": price,
        "last_bar_ts_utc": last_ts_utc,
        "session": session,
        "m15_bias": {
            "ema50": ema50_m15,
            "ema200": ema200_m15,
            "adx14": adx_m15,
            "atr14": atr_m15,
            "price_vs_ema50": pvs_ema50,
            "price_vs_ema200": pvs_ema200,
        },
        "m5_momentum": { "ema8": ema8_m5, "vwap": vwap_now },
        "m1_entry": { "ema9": ema9_m1, "ema20": ema20_m1 },
        "session_high": sess_hi,
        "session_low": sess_lo,
        "prior_day": { "high": pdh, "low": pdl, "close": pdc },
        "correlations": {
            "usdjpy": { "last": usdjpy_last, "chg12h_pct": usdjpy_chg, "dir": usdjpy_dir },
            "xagusd": { "last": xagusd_last, "chg12h_pct": xagusd_chg, "dir": xagusd_dir },
            "note": "USDJPY moves inverse to gold; XAGUSD (silver) confirms gold direction",
        },
        "m1_recent": m1_recent_30,     // {ts,o,h,l,c} — entry timing
        "m5_recent": m5_recent_24,     // {ts,o,h,l,c,vwap,ema8} — momentum
        "m15_recent": m15_recent_48,   // {ts,o,h,l,c,vwap,ema8} — trend structure (HH/HL)
        "calendar_next": calendar,
        "news_headlines": headlines,
    });
    Ok((compact, full, blitz))
}

/// Tauri command behind `Gold_Trade_Ideas` — fan out to 4 models in parallel.
/// Claude gets the full snapshot + the multi-strategy playbook; Gemini/DeepSeek/
/// Qwen get the compact snapshot + the focused VWAP+8EMA prompt. Each is
/// single-shot and capped at ~18 s; a `trade_idea_model` event is emitted per
/// model as it finishes so the popups fill in live.
#[tauri::command]
async fn get_gold_trade_ideas_multi(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<Vec<ai::traders::ModelTradeIdea>, String> {
    use tauri::Emitter;

    let symbol_id = {
        let map = symbol_map().lock().map_err(|e| e.to_string())?;
        map.get("XAUUSD").copied()
            .ok_or_else(|| "XAUUSD not subscribed yet — wait a few seconds after connect.".to_string())?
    };

    let (compact, full, blitz) = build_trade_snapshots(&state.db_mutex, symbol_id).await?;
    // Claude runs the full multi-strategy playbook on the fuller snapshot;
    // Gemini gets a general day-trade prompt on the compact snapshot; Claude Blitz
    // runs the trend-scalp playbook on the pre-computed scalp snapshot.
    let user_compact = format!(
        "Compact live snapshot (seconds old):\n```json\n{}\n```\n\nGive ONE intraday setup for right now. JSON only.",
        serde_json::to_string(&compact).unwrap_or_default()
    );
    let user_full = format!(
        "Live snapshot (seconds old):\n```json\n{}\n```\n\nClassify the regime, pick the best-fitting strategy, and give ONE intraday setup for right now. JSON only.",
        serde_json::to_string(&full).unwrap_or_default()
    );
    let user_blitz = format!(
        "Live 1-min scalp snapshot (seconds old; indicators pre-computed):\n```json\n{}\n```\n\nRun the triple-screen (M15 trend → M5 momentum → M1 entry) and give ONE 1-minute scalp setup for right now (or FLAT). JSON only.",
        serde_json::to_string(&blitz).unwrap_or_default()
    );
    let system_claude = ai::traders::gold_day_trader_prompt();
    let system_simple = ai::traders::SIMPLE_TRADER_PROMPT.to_string();
    let system_blitz = ai::traders::blitz_prompt();

    let claude_model = std::env::var("CLAUDE_MODEL").unwrap_or_else(|_| "sonnet".to_string());
    let blitz_model = std::env::var("CLAUDE_BLITZ_MODEL").unwrap_or_else(|_| "haiku".to_string());
    let gemini_key = std::env::var("GOOGLE_API_KEY").ok();

    enum Kind { Claude(String), Gemini(String, Option<String>) }
    // (label, kind, system, user)
    let jobs: Vec<(&str, Kind, String, String)> = vec![
        ("Claude", Kind::Claude(claude_model),                             system_claude, user_full),
        ("Gemini", Kind::Gemini("gemini-flash-latest".into(), gemini_key), system_simple, user_compact),
        ("Claude Blitz", Kind::Claude(blitz_model),                        system_blitz,  user_blitz),
    ];

    let mut set = tokio::task::JoinSet::new();
    for (label, kind, system, user) in jobs {
        let app = app.clone();
        let label = label.to_string();
        set.spawn(async move {
            let res = match kind {
                Kind::Claude(m) => ai::traders::run_claude_cli(&label, &m, &system, &user).await,
                Kind::Gemini(m, key) => match key {
                    Some(k) => ai::traders::run_gemini(&label, &m, &k, &system, &user).await,
                    None => ai::traders::ModelTradeIdea::error(&label, &m, "GOOGLE_API_KEY not set in .env"),
                },
            };
            let _ = app.emit("trade_idea_model", &res);
            res
        });
    }

    let mut out = Vec::new();
    while let Some(joined) = set.join_next().await {
        if let Ok(r) = joined { out.push(r); }
    }
    Ok(out)
}

/// Per-idea Tauri command behind the three separate "idea" buttons. Builds the
/// live snapshot and runs exactly ONE model (Claude · Gemini · Claude Blitz),
/// returning its single setup. Each button runs alone on demand — no 3-way
/// concurrency, so a slow model never blocks the others.
#[tauri::command]
async fn get_gold_trade_idea(
    state: tauri::State<'_, AppState>,
    provider: String,
) -> Result<ai::traders::ModelTradeIdea, String> {
    let symbol_id = {
        let map = symbol_map().lock().map_err(|e| e.to_string())?;
        map.get("XAUUSD").copied()
            .ok_or_else(|| "XAUUSD not subscribed yet — wait a few seconds after connect.".to_string())?
    };
    let (compact, full, blitz) = build_trade_snapshots(&state.db_mutex, symbol_id).await?;

    let idea = match provider.as_str() {
        "Claude" => {
            let user = format!(
                "Live snapshot (seconds old):\n```json\n{}\n```\n\nClassify the regime, pick the best-fitting strategy, and give ONE intraday setup for right now. JSON only.",
                serde_json::to_string(&full).unwrap_or_default()
            );
            let model = std::env::var("CLAUDE_MODEL").unwrap_or_else(|_| "sonnet".to_string());
            ai::traders::run_claude_cli("Claude", &model, &ai::traders::gold_day_trader_prompt(), &user).await
        }
        "Gemini" => {
            let user = format!(
                "Compact live snapshot (seconds old):\n```json\n{}\n```\n\nGive ONE intraday setup for right now. JSON only.",
                serde_json::to_string(&compact).unwrap_or_default()
            );
            match std::env::var("GOOGLE_API_KEY").ok() {
                Some(k) => ai::traders::run_gemini("Gemini", "gemini-flash-latest", &k, ai::traders::SIMPLE_TRADER_PROMPT, &user).await,
                None => ai::traders::ModelTradeIdea::error("Gemini", "gemini-flash-latest", "GOOGLE_API_KEY not set in .env"),
            }
        }
        "Claude Blitz" => {
            let user = format!(
                "Live 1-min scalp snapshot (seconds old; indicators pre-computed):\n```json\n{}\n```\n\nRun the triple-screen (M15 trend → M5 momentum → M1 entry) and give ONE 1-minute scalp setup for right now (or FLAT). JSON only.",
                serde_json::to_string(&blitz).unwrap_or_default()
            );
            let model = std::env::var("CLAUDE_BLITZ_MODEL").unwrap_or_else(|_| "haiku".to_string());
            ai::traders::run_claude_cli("Claude Blitz", &model, &ai::traders::blitz_prompt(), &user).await
        }
        other => ai::traders::ModelTradeIdea::error(other, "", &format!("unknown provider '{}'", other)),
    };
    Ok(idea)
}

/// The XRP 5-minute agent (read-only): build the live XRPUSD snapshot, run it
/// through Claude with the `xrp-5m` playbook, and return ONE setup. Places
/// nothing — this is the preview the dashboard shows before any live loop runs.
#[tauri::command]
async fn get_xrp_trade_idea(
    state: tauri::State<'_, AppState>,
) -> Result<ai::traders::ModelTradeIdea, String> {
    let symbol_id = {
        let map = symbol_map().lock().map_err(|e| e.to_string())?;
        map.get("XRPUSD").copied()
            .ok_or_else(|| "XRPUSD not subscribed yet — restart the app to pick up the new subscription.".to_string())?
    };
    let (_compact, mut full, _blitz) = build_trade_snapshots(&state.db_mutex, symbol_id).await?;

    // BTC is the master filter for XRP — XRP rarely trends against Bitcoin. Attach
    // a `btc` context block (M5 + M15 direction/structure) so the agent can gate
    // its XRP bias on Bitcoin's tape. Degrades gracefully if BTCUSD isn't mapped.
    let btc_id = symbol_map().lock().ok().and_then(|m| m.get("BTCUSD").copied());
    if let Some(btc_id) = btc_id {
        use openapi::ProtoOaTrendbarPeriod as P;
        let (btc_m5, btc_m15) = tokio::join!(
            fetch_trendbars_for_snapshot(btc_id, P::M5, 5, 96),
            fetch_trendbars_for_snapshot(btc_id, P::M15, 15, 48),
        );
        let btc_m5 = btc_m5.unwrap_or_default();
        let btc_m15 = btc_m15.unwrap_or_default();
        if !btc_m5.is_empty() {
            let (m5_json, _vwap, ema8) = candles_with_indicators(&btc_m5);
            let (b5_last, b5_chg, b5_dir) = series_dir(&btc_m5, 12);
            let (_b15_last, b15_chg, b15_dir) = series_dir(&btc_m15, 12);
            let tail = |v: &[serde_json::Value], n: usize| -> Vec<serde_json::Value> {
                v.iter().skip(v.len().saturating_sub(n)).cloned().collect()
            };
            let m15_json = candles_to_json(&btc_m15);
            let btc = serde_json::json!({
                "price": b5_last,
                "m5": { "ema8": ema8, "dir12": b5_dir, "chg12_pct": b5_chg },
                "m15": { "dir12": b15_dir, "chg12_pct": b15_chg },
                "m5_recent": tail(&m5_json, 12),    // {ts,o,h,l,c,vwap,ema8}
                "m15_recent": tail(&m15_json, 12),  // {ts,o,h,l,c}
                "note": "BTC is the master filter: XRP rarely trends against Bitcoin. Trade XRP WITH BTC's direction; if BTC is flat/choppy demand a strong XRP-specific signal or stand aside; favor XRP catch-up when it lags a strong BTC move, and don't take an XRP bias that fights a clear BTC trend.",
            });
            if let Some(obj) = full.as_object_mut() {
                obj.insert("btc".to_string(), btc);
            }
        }
    }

    let user = format!(
        "Live XRPUSD snapshot (seconds old):\n```json\n{}\n```\n\nClassify the regime and the BTC tape, pick the best-fitting strategy, and give ONE 5-minute setup for right now (or FLAT). JSON only.",
        serde_json::to_string(&full).unwrap_or_default()
    );
    let model = std::env::var("CLAUDE_MODEL").unwrap_or_else(|_| "sonnet".to_string());
    Ok(ai::traders::run_claude_cli("XRP 5m", &model, &ai::traders::xrp_5m_prompt(), &user).await)
}

/// Compute the volume-by-price levels (per-day POC + today/yesterday + weekly
/// composites with provenance) over the last 2 weeks of M5, plus the current
/// price and recent M15 context. Returned to the chart to draw the lines AND
/// passed back into `get_volume_trade_idea` so Claude reasons on the same data.
#[tauri::command]
async fn get_volume_levels(
    _state: tauri::State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let symbol_id = {
        let map = symbol_map().lock().map_err(|e| e.to_string())?;
        map.get("XAUUSD").copied()
            .ok_or_else(|| "XAUUSD not subscribed yet — wait a few seconds after connect.".to_string())?
    };

    let m5 = fetch_m5_window(symbol_id, 14).await;
    if m5.is_empty() {
        return Err("no candle data from cTrader (session reconnecting?)".into());
    }
    let now = chrono::Utc::now();
    let levels = volume_profile::compute_levels(&m5, now, 0.5);

    let live_bid = {
        let bits = LATEST_XAUUSD_BID.load(std::sync::atomic::Ordering::Relaxed);
        if bits != 0 { Some(f64::from_bits(bits)) } else { None }
    };
    let price = live_bid.or_else(|| m5.last().map(|c| c.close));

    // Recent M15 context for the agent.
    let m15 = fetch_trendbars_for_snapshot(symbol_id, openapi::ProtoOaTrendbarPeriod::M15, 15, 64)
        .await.unwrap_or_default();
    let m15_recent = if m15.len() > 32 { candles_to_json(&m15[m15.len() - 32..]) } else { candles_to_json(&m15) };
    let atr_h1 = atr_simple(&m5, 12); // coarse intraday range gauge from M5

    Ok(serde_json::json!({
        "now_utc": now.to_rfc3339(),
        "price": price,
        "atr_intraday": atr_h1,
        "levels": levels,
        "m15_recent": m15_recent,
    }))
}

/// Ask the Claude Volume agent for a setup that references the volume levels.
/// Takes the snapshot returned by `get_volume_levels` (so the chart and Claude
/// see identical levels and there's no second fetch).
#[tauri::command]
async fn get_volume_trade_idea(
    snapshot: serde_json::Value,
) -> Result<ai::traders::ModelTradeIdea, String> {
    let user = format!(
        "Live volume snapshot (seconds old; levels pre-computed):\n```json\n{}\n```\n\nUsing these volume levels, give ONE setup for right now (or FLAT). JSON only.",
        serde_json::to_string(&snapshot).unwrap_or_default()
    );
    let model = std::env::var("CLAUDE_VOLUME_MODEL").unwrap_or_else(|_| "sonnet".to_string());
    Ok(ai::traders::run_claude_cli("Claude Volume", &model, &ai::traders::volume_prompt(), &user).await)
}

/// Result of a LIVE order placement. `sent` is true once the broker accepts the
/// order; otherwise `error` explains why.
#[derive(serde::Serialize)]
struct OrderResult {
    sent: bool,
    side: String,            // "BUY" | "SELL"
    symbol: String,
    oz: u32,
    ctrader_volume: i64,     // volume in cents, validated against symbol min/step
    order_type: String,      // "MARKET" | "LIMIT" | "STOP"
    entry: Option<f64>,      // pending entry price (None for MARKET)
    sl: Option<f64>,         // SL price attached (None = not attached)
    tp: Option<f64>,         // TP price attached
    status: Option<String>,  // execution type, e.g. ORDER_ACCEPTED / ORDER_FILLED
    error: Option<String>,
}

/// Place a REAL order for XAUUSD on the live cTrader account, sized from the
/// chosen ounces. When the idea has an entry zone, a PENDING order is placed at
/// that entry (LIMIT for a pullback, STOP for a breakout) with the idea's
/// absolute SL/TP — so the fill matches the idea instead of chasing the market.
/// With no entry zone it falls back to a MARKET order with relative SL/TP.
/// Refuses if the symbol spec isn't cached or there's no live price.
///
/// Internal core shared by the Tauri command and the auto-trade loop.
async fn submit_gold_order(
    side: String,
    oz: u32,
    entry: Option<f64>,
    stop: Option<f64>,
    target1: Option<f64>,
) -> Result<OrderResult, String> {
    let oz = oz.clamp(1, 10);
    let (trade_side, side_str) = match side.to_uppercase().as_str() {
        "LONG" | "BUY"  => (openapi::ProtoOaTradeSide::Buy, "BUY"),
        "SHORT" | "SELL" => (openapi::ProtoOaTradeSide::Sell, "SELL"),
        other => return Err(format!("can't place an order for bias '{}'", other)),
    };
    let is_buy = matches!(trade_side, openapi::ProtoOaTradeSide::Buy);

    let spec = xauusd_spec().lock().map_err(|e| e.to_string())?.ok_or(
        "XAUUSD trading spec not loaded yet — wait a few seconds after connect and retry."
    )?;
    let round = |x: f64| {
        let f = 10f64.powi(spec.digits.max(0));
        (x * f).round() / f
    };

    // Volume: oz × 100 (cents), clamped to [min, max] and floored to a step multiple.
    let mut volume = oz as i64 * 100;
    if spec.max_volume > 0 { volume = volume.min(spec.max_volume); }
    if spec.step_volume > 0 { volume = (volume / spec.step_volume) * spec.step_volume; }
    if spec.min_volume > 0 && volume < spec.min_volume { volume = spec.min_volume; }
    if volume <= 0 {
        return Err(format!("computed volume {} invalid for spec min={} step={}",
                           volume, spec.min_volume, spec.step_volume));
    }

    let price = {
        let bits = LATEST_XAUUSD_BID.load(std::sync::atomic::Ordering::Relaxed);
        if bits != 0 { f64::from_bits(bits) } else { 0.0 }
    };
    if price <= 0.0 {
        return Err("no live XAUUSD price yet — wait for a tick and retry.".into());
    }

    // Entry chosen in the popup (may be user-edited). None → market.
    let entry = entry.map(round);

    let mut order = OrderRequest {
        symbol_id: spec.symbol_id,
        trade_side,
        order_type: openapi::ProtoOaOrderType::Market,
        volume,
        limit_price: None,
        stop_price: None,
        stop_loss: None,
        take_profit: None,
        rel_sl: None,
        rel_tp: None,
        label: "GoldTradeIdea".into(),
        reply: { /* set below */ tokio::sync::oneshot::channel().0 },
    };

    let (out_type, out_entry, out_sl, out_tp);
    if let Some(e) = entry {
        // PENDING order at the idea's entry. Buy below market = LIMIT, above = STOP
        // (and the mirror for sells). Absolute SL/TP are allowed for pending orders.
        let ot = if is_buy {
            if e <= price { openapi::ProtoOaOrderType::Limit } else { openapi::ProtoOaOrderType::Stop }
        } else if e >= price { openapi::ProtoOaOrderType::Limit } else { openapi::ProtoOaOrderType::Stop };
        order.order_type = ot;
        match ot {
            openapi::ProtoOaOrderType::Limit => order.limit_price = Some(e),
            _ => order.stop_price = Some(e),
        }
        // SL/TP validated against the ENTRY (must straddle it correctly).
        let sl = stop.map(round).filter(|&s| if is_buy { s < e } else { s > e });
        let tp = target1.map(round).filter(|&t| if is_buy { t > e } else { t < e });
        order.stop_loss = sl;
        order.take_profit = tp;
        out_type = ot.as_str_name().to_string();
        out_entry = Some(e);
        out_sl = sl;
        out_tp = tp;
    } else {
        // No entry zone → MARKET now, with relative SL/TP from current price.
        let sl = stop.filter(|&s| if is_buy { s < price } else { s > price });
        let tp = target1.filter(|&t| if is_buy { t > price } else { t < price });
        order.rel_sl = sl.map(|s| ((price - s).abs() * 100_000.0).round() as i64);
        order.rel_tp = tp.map(|t| ((t - price).abs() * 100_000.0).round() as i64);
        out_type = "MARKET".to_string();
        out_entry = None;
        out_sl = sl.map(round);
        out_tp = tp.map(round);
    }

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    order.reply = reply_tx;
    let tx = ORDER_REQ_TX.get().ok_or("order channel not initialised")?;
    tx.send(order).await.map_err(|e| format!("send order: {}", e))?;

    let outcome = match tokio::time::timeout(std::time::Duration::from_secs(15), reply_rx).await {
        Ok(Ok(r)) => r,
        Ok(Err(_)) => Err("order request cancelled".into()),
        Err(_) => Err("broker response timeout (order may or may not have been placed — check cTrader)".into()),
    };

    match outcome {
        Ok(status) => Ok(OrderResult {
            sent: true, side: side_str.into(), symbol: "XAUUSD".into(), oz,
            ctrader_volume: volume, order_type: out_type, entry: out_entry,
            sl: out_sl, tp: out_tp, status: Some(status), error: None,
        }),
        Err(e) => Ok(OrderResult {
            sent: false, side: side_str.into(), symbol: "XAUUSD".into(), oz,
            ctrader_volume: volume, order_type: out_type, entry: out_entry,
            sl: out_sl, tp: out_tp, status: None, error: Some(e),
        }),
    }
}

/// Tauri command wrapper around [`submit_gold_order`].
#[tauri::command]
async fn place_gold_order(
    side: String,
    oz: u32,
    entry: Option<f64>,
    stop: Option<f64>,
    target1: Option<f64>,
) -> Result<OrderResult, String> {
    submit_gold_order(side, oz, entry, stop, target1).await
}

/// Enable/disable the auto-trade loop and set the per-trade size (oz).
#[tauri::command]
fn set_auto_trade(enabled: bool, oz: u32) {
    AUTO_OZ.store(oz.clamp(1, 10), std::sync::atomic::Ordering::Relaxed);
    AUTO_TRADE.store(enabled, std::sync::atomic::Ordering::Relaxed);
    println!("[auto] {} · {} oz", if enabled { "ENABLED" } else { "disabled" }, oz.clamp(1, 10));
}

/// Push the current auto-trade status to the UI over the WS bridge.
async fn push_auto_status(tx: &mpsc::Sender<PriceUpdate>, status: &str) {
    use std::sync::atomic::Ordering::Relaxed;
    let v = serde_json::json!({
        "enabled": AUTO_TRADE.load(Relaxed),
        "oz": AUTO_OZ.load(Relaxed),
        "status": status,
    });
    let _ = tx.send(PriceUpdate::AutoStatus(v)).await;
}

/// Generate a fresh Claude setup and, if it's a clean LONG/SHORT with an entry,
/// place an `AUTO_OZ`-sized pending order. Returns Ok(true) if an order was
/// placed, Ok(false) if FLAT/no setup, Err on a hard failure.
async fn auto_generate_and_place(db_mutex: &SharedDb) -> Result<bool, String> {
    let symbol_id = {
        let map = symbol_map().lock().map_err(|e| e.to_string())?;
        map.get("XAUUSD").copied().ok_or("XAUUSD not subscribed yet")?
    };
    let (_compact, full, _blitz) = build_trade_snapshots(db_mutex, symbol_id).await?;
    let system = ai::traders::gold_day_trader_prompt();
    let user = format!(
        "Live snapshot (seconds old):\n```json\n{}\n```\n\nClassify the regime, pick the best-fitting strategy, and give ONE intraday setup for right now. JSON only.",
        serde_json::to_string(&full).unwrap_or_default()
    );
    let model = std::env::var("CLAUDE_MODEL").unwrap_or_else(|_| "sonnet".into());
    let idea = ai::traders::run_claude_cli("Claude", &model, &system, &user).await;
    if !idea.ok { return Err(idea.error.unwrap_or_else(|| "model error".into())); }
    let bias = idea.bias.as_deref().unwrap_or("FLAT").to_uppercase();
    if bias != "LONG" && bias != "SHORT" { return Ok(false); }
    let entry = match (idea.entry_low, idea.entry_high) {
        (Some(a), Some(b)) => Some((a + b) / 2.0),
        (Some(a), None) | (None, Some(a)) => Some(a),
        (None, None) => None,
    };
    let oz = AUTO_OZ.load(std::sync::atomic::Ordering::Relaxed).clamp(1, 10);
    let res = submit_gold_order(bias, oz, entry, idea.stop, idea.target1).await?;
    if res.sent { Ok(true) } else { Err(res.error.unwrap_or_else(|| "order rejected".into())) }
}

/// Auto-trade state machine: keep exactly one XAUUSD position at a time. When
/// flat, ask Claude for a setup and place a pending order; cancel orders that
/// rest unfilled > 15 min; regenerate after each position closes. Idles when
/// `AUTO_TRADE` is off. Runs for the life of the process.
async fn run_auto_trade_loop(tx: mpsc::Sender<PriceUpdate>, db_mutex: SharedDb) {
    use std::time::{Duration, Instant};
    use std::sync::atomic::Ordering::Relaxed;
    let mut seen_orders: std::collections::HashMap<i64, Instant> = std::collections::HashMap::new();
    let mut next_gen = Instant::now();
    let mut last_place = Instant::now() - Duration::from_secs(120);
    let mut was_enabled = false;

    loop {
        tokio::time::sleep(Duration::from_secs(20)).await;
        if !AUTO_TRADE.load(Relaxed) {
            if was_enabled { push_auto_status(&tx, "off").await; was_enabled = false; }
            seen_orders.clear();
            continue;
        }
        if !was_enabled { push_auto_status(&tx, "enabled").await; was_enabled = true; }

        let (positions, pending) = match xau_account_state().lock() {
            Ok(s) => (s.positions, s.pending_order_ids.clone()),
            Err(_) => continue,
        };

        // In a position → wait for it to close.
        if positions > 0 {
            push_auto_status(&tx, "in position — waiting for it to close").await;
            seen_orders.clear();
            continue;
        }

        // Order(s) resting → wait for fill, or cancel if stale (>15 min).
        if !pending.is_empty() {
            let now = Instant::now();
            for &id in &pending { seen_orders.entry(id).or_insert(now); }
            seen_orders.retain(|id, _| pending.contains(id));
            let mut cancelled = false;
            for (id, t) in seen_orders.clone() {
                if now.duration_since(t) > Duration::from_secs(15 * 60) {
                    push_auto_status(&tx, &format!("cancelling stale order {} (>15m unfilled)", id)).await;
                    if let Some(ctx) = CANCEL_REQ_TX.get() {
                        let (rtx, rrx) = tokio::sync::oneshot::channel();
                        if ctx.send(CancelRequest { order_id: id, reply: rtx }).await.is_ok() {
                            let _ = tokio::time::timeout(Duration::from_secs(15), rrx).await;
                        }
                    }
                    seen_orders.remove(&id);
                    next_gen = Instant::now();
                    cancelled = true;
                }
            }
            if !cancelled { push_auto_status(&tx, "order resting — waiting for fill").await; }
            continue;
        }

        // Flat → generate the next idea (throttled).
        if Instant::now() < next_gen {
            push_auto_status(&tx, "flat — waiting before next idea").await;
            continue;
        }
        if last_place.elapsed() < Duration::from_secs(60) { continue; } // let reconcile settle
        push_auto_status(&tx, "flat — asking Claude for a setup…").await;
        match auto_generate_and_place(&db_mutex).await {
            Ok(true) => {
                last_place = Instant::now();
                next_gen = Instant::now() + Duration::from_secs(60);
                push_auto_status(&tx, "order placed — waiting for fill").await;
            }
            Ok(false) => {
                next_gen = Instant::now() + Duration::from_secs(5 * 60);
                push_auto_status(&tx, "no clean setup (FLAT) — retry in 5m").await;
            }
            Err(e) => {
                next_gen = Instant::now() + Duration::from_secs(60);
                push_auto_status(&tx, &format!("error: {} — retry in 1m", e)).await;
            }
        }
    }
}

/// Ask Claude whether a resting pending order is still worth keeping, given a
/// fresh live snapshot. Returns KEEP/CANCEL + reason; the UI shows it with
/// Keep / Cancel buttons. Read-only — places/cancels nothing itself.
#[tauri::command]
async fn review_pending_order(
    state: tauri::State<'_, AppState>,
    order_id: i64,
    side: String,
    entry: Option<f64>,
    stop: Option<f64>,
    target1: Option<f64>,
) -> Result<ai::traders::OrderReview, String> {
    let symbol_id = {
        let map = symbol_map().lock().map_err(|e| e.to_string())?;
        map.get("XAUUSD").copied()
            .ok_or_else(|| "XAUUSD not subscribed yet.".to_string())?
    };
    let (_compact, full, _blitz) = build_trade_snapshots(&state.db_mutex, symbol_id).await?;
    let user = format!(
        "Resting pending order #{}: {} XAUUSD @ entry {} · SL {} · TP {}.\n\n\
         Fresh live snapshot:\n```json\n{}\n```\n\n\
         Is this order still relevant? KEEP or CANCEL? JSON only.",
        order_id,
        side.to_uppercase(),
        entry.map(|v| format!("{:.2}", v)).unwrap_or_else(|| "n/a".into()),
        stop.map(|v| format!("{:.2}", v)).unwrap_or_else(|| "n/a".into()),
        target1.map(|v| format!("{:.2}", v)).unwrap_or_else(|| "n/a".into()),
        serde_json::to_string(&full).unwrap_or_default(),
    );
    let model = std::env::var("CLAUDE_MODEL").unwrap_or_else(|_| "sonnet".to_string());
    Ok(ai::traders::review_order_with_claude(&model, &user).await)
}

/// Cancel a resting pending order on the live account by id.
#[tauri::command]
async fn cancel_order(order_id: i64) -> Result<String, String> {
    let tx = CANCEL_REQ_TX.get().ok_or("cancel channel not initialised")?;
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    tx.send(CancelRequest { order_id, reply: reply_tx })
        .await
        .map_err(|e| format!("send cancel: {}", e))?;
    match tokio::time::timeout(std::time::Duration::from_secs(15), reply_rx).await {
        Ok(Ok(r)) => r,
        Ok(Err(_)) => Err("cancel request cancelled".into()),
        Err(_) => Err("broker response timeout (check cTrader)".into()),
    }
}

/// Ask Claude whether to HOLD / ADJUST (new SL/TP) / CLOSE an open position,
/// given a fresh live snapshot. Read-only — applies nothing itself.
#[tauri::command]
async fn review_position(
    state: tauri::State<'_, AppState>,
    position_id: i64,
    side: String,
    entry: Option<f64>,
    oz: f64,
    stop: Option<f64>,
    target1: Option<f64>,
) -> Result<ai::traders::PositionReview, String> {
    let symbol_id = {
        let map = symbol_map().lock().map_err(|e| e.to_string())?;
        map.get("XAUUSD").copied().ok_or_else(|| "XAUUSD not subscribed yet.".to_string())?
    };
    let (_compact, full, _blitz) = build_trade_snapshots(&state.db_mutex, symbol_id).await?;
    let f = |v: Option<f64>| v.map(|x| format!("{:.2}", x)).unwrap_or_else(|| "none".into());
    let user = format!(
        "Open position #{}: {} XAUUSD {} oz · entry {} · SL {} · TP {}.\n\n\
         Fresh live snapshot:\n```json\n{}\n```\n\n\
         HOLD, ADJUST (give new SL/TP), or CLOSE? JSON only.",
        position_id, side.to_uppercase(), oz, f(entry), f(stop), f(target1),
        serde_json::to_string(&full).unwrap_or_default(),
    );
    let model = std::env::var("CLAUDE_MODEL").unwrap_or_else(|_| "sonnet".to_string());
    Ok(ai::traders::review_position_with_claude(&model, &user).await)
}

/// Market-close an open position (full size) on the live account.
#[tauri::command]
async fn close_position(position_id: i64, oz: f64) -> Result<String, String> {
    let volume = (oz * 100.0).round() as i64;
    if volume <= 0 { return Err("invalid volume".into()); }
    let tx = POS_ACTION_TX.get().ok_or("position channel not initialised")?;
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    tx.send(PositionActionRequest {
        action: PositionAction::Close { position_id, volume },
        reply: reply_tx,
    }).await.map_err(|e| format!("send close: {}", e))?;
    match tokio::time::timeout(std::time::Duration::from_secs(15), reply_rx).await {
        Ok(Ok(r)) => r,
        Ok(Err(_)) => Err("close request cancelled".into()),
        Err(_) => Err("broker response timeout (check cTrader)".into()),
    }
}

/// Amend an open position's stop-loss and/or take-profit (absolute prices).
#[tauri::command]
async fn amend_position_sltp(
    position_id: i64,
    stop_loss: Option<f64>,
    take_profit: Option<f64>,
) -> Result<String, String> {
    if stop_loss.is_none() && take_profit.is_none() {
        return Err("nothing to amend".into());
    }
    let tx = POS_ACTION_TX.get().ok_or("position channel not initialised")?;
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    tx.send(PositionActionRequest {
        action: PositionAction::AmendSltp { position_id, stop_loss, take_profit },
        reply: reply_tx,
    }).await.map_err(|e| format!("send amend: {}", e))?;
    match tokio::time::timeout(std::time::Duration::from_secs(15), reply_rx).await {
        Ok(Ok(r)) => r,
        Ok(Err(_)) => Err("amend request cancelled".into()),
        Err(_) => Err("broker response timeout (check cTrader)".into()),
    }
}

/// Build the positions/orders snapshot JSON pushed to the UI panel. Volume is
/// shown in oz (cents/100). Pending protection/closing orders are excluded by
/// the caller; this just serializes whatever maps it's given.
fn build_positions_snapshot(
    positions: &std::collections::HashMap<i64, openapi::ProtoOaPosition>,
    orders: &std::collections::HashMap<i64, openapi::ProtoOaOrder>,
    names: &std::collections::HashMap<i64, String>,
) -> serde_json::Value {
    let side_str = |s: i32| if s == openapi::ProtoOaTradeSide::Buy as i32 { "BUY" } else { "SELL" };
    let sym = |id: i64| names.get(&id).cloned().unwrap_or_else(|| id.to_string());
    let pos: Vec<serde_json::Value> = positions.values().map(|p| {
        let td = &p.trade_data;
        serde_json::json!({
            "id": p.position_id,
            "symbol": sym(td.symbol_id),
            "side": side_str(td.trade_side),
            "oz": td.volume as f64 / 100.0,
            "entry": p.price,
            "sl": p.stop_loss,
            "tp": p.take_profit,
        })
    }).collect();
    let ord: Vec<serde_json::Value> = orders.values().map(|o| {
        let td = &o.trade_data;
        let ot = openapi::ProtoOaOrderType::try_from(o.order_type).map(|t| t.as_str_name()).unwrap_or("?");
        serde_json::json!({
            "id": o.order_id,
            "symbol": sym(td.symbol_id),
            "side": side_str(td.trade_side),
            "type": ot,
            "oz": td.volume as f64 / 100.0,
            "price": o.limit_price.or(o.stop_price),
            "sl": o.stop_loss,
            "tp": o.take_profit,
        })
    }).collect();
    serde_json::json!({ "positions": pos, "orders": ord })
}

/// If an execution event is a position close (a FILLED closing order), emit a
/// one-off TradeNotice (SL hit / TP hit / stop-out / closed + gross P/L). Uses
/// the pre-close tracked position to recover entry/SL/TP.
async fn maybe_emit_close_notice(
    ev: &openapi::ProtoOaExecutionEvent,
    open_positions: &std::collections::HashMap<i64, openapi::ProtoOaPosition>,
    names: &std::collections::HashMap<i64, String>,
    tx: &mpsc::Sender<PriceUpdate>,
) {
    let Some(order) = ev.order.as_ref() else { return };
    // 3 = ORDER_FILLED; closing_order marks it as reducing/closing a position.
    if ev.execution_type != 3 || order.closing_order != Some(true) { return; }

    let pos_id = order.position_id.unwrap_or(0);
    let close_px = order.execution_price.unwrap_or(0.0);
    let prev = open_positions.get(&pos_id);
    let (entry, sl, tp, side, oz, symbol) = match prev {
        Some(p) => (
            p.price.unwrap_or(0.0), p.stop_loss, p.take_profit,
            p.trade_data.trade_side, p.trade_data.volume as f64 / 100.0,
            names.get(&p.trade_data.symbol_id).cloned().unwrap_or_else(|| "XAUUSD".into()),
        ),
        None => (0.0, None, None, 0, 0.0, "XAUUSD".into()),
    };
    let reason = if order.is_stop_out == Some(true) {
        "stop-out"
    } else {
        match (sl, tp) {
            (Some(s), Some(t)) => if (close_px - s).abs() <= (close_px - t).abs() { "SL hit" } else { "TP hit" },
            (Some(_), None) => "SL hit",
            (None, Some(_)) => "TP hit",
            _ => "closed",
        }
    };
    let is_buy = side == openapi::ProtoOaTradeSide::Buy as i32;
    let pnl = if entry > 0.0 {
        let dir = if is_buy { 1.0 } else { -1.0 };
        ((close_px - entry) * oz * dir * 100.0).round() / 100.0
    } else { 0.0 };
    let notice = serde_json::json!({
        "reason": reason,
        "symbol": symbol,
        "side": if is_buy { "BUY" } else { "SELL" },
        "oz": oz,
        "close": close_px,
        "pnl": pnl,
        "position_id": pos_id,
    });
    let _ = tx.send(PriceUpdate::TradeNotice(notice)).await;
}

/// Best-effort: terminate a child process tree by PID. Used at shutdown.
fn kill_process_tree(pid: u32, label: &str) {
    if pid == 0 { return; }
    println!("[{}] Killing process tree (pid {}) on shutdown...", label, pid);
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .output();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("kill")
            .args(["-TERM", &format!("-{}", pid)])
            .output();
    }
}

/// Kills every spawned child (econcal, Vite dev server) before the process exits.
fn shutdown_children() {
    kill_process_tree(ECONCAL_PID.swap(0, std::sync::atomic::Ordering::Relaxed), "econcal");
    kill_process_tree(VITE_PID.swap(0, std::sync::atomic::Ordering::Relaxed), "vite");
}

/// In debug builds, spawns the Vite dev server (`npm run dev` in frontend/) and
/// blocks until it's responding on port 5173. In release builds this is a no-op;
/// the bundled `frontend/dist/` is served directly by Tauri.
#[cfg(debug_assertions)]
fn start_vite_dev_server() {
    let frontend_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/frontend");

    if std::net::TcpStream::connect("127.0.0.1:5173").is_ok()
        || std::net::TcpStream::connect("[::1]:5173").is_ok()
    {
        println!("[vite] Port 5173 already in use — assuming dev server is running.");
        return;
    }

    println!("[vite] Starting dev server (npm run dev)...");

    // On Windows, npm ships as npm.cmd; on Unix it's just `npm` on PATH.
    #[cfg(windows)]
    let cmd = "npm.cmd";
    #[cfg(not(windows))]
    let cmd = "npm";

    let mut command = std::process::Command::new(cmd);
    command
        .args(["run", "dev"])
        .current_dir(frontend_dir)
        // Detach stdin so the child can't lock the parent terminal after we
        // exit. Without this, npm + Vite's subprocess tree (esbuild, etc.)
        // inherit the parent stdin handle and leave the terminal unusable
        // once they're killed — the user has to close the terminal window.
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW (0x08000000) — keep the child completely detached
        // from our console. Otherwise Windows shares console handles down the
        // process tree and breaks the parent terminal on shutdown.
        command.creation_flags(0x08000000);
    }
    let result = command.spawn();

    let mut child = match result {
        Err(e) => {
            println!("[vite] Failed to start: {} (is npm on PATH?)", e);
            return;
        }
        Ok(c) => c,
    };
    VITE_PID.store(child.id(), std::sync::atomic::Ordering::Relaxed);

    // Forward Vite stdout in the background so its logs show up alongside ours.
    if let Some(stdout) = child.stdout.take() {
        std::thread::spawn(move || {
            use std::io::BufRead;
            let reader = std::io::BufReader::new(stdout);
            for line in reader.lines().flatten() {
                println!("[vite] {}", line);
            }
        });
    }
    if let Some(stderr) = child.stderr.take() {
        std::thread::spawn(move || {
            use std::io::BufRead;
            let reader = std::io::BufReader::new(stderr);
            for line in reader.lines().flatten() {
                println!("[vite] ERR: {}", line);
            }
        });
    }

    // Block until Vite is reachable (or 15s timeout), so the Tauri webview
    // doesn't open to an "ERR_CONNECTION_REFUSED" page.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while std::time::Instant::now() < deadline {
        if std::net::TcpStream::connect("127.0.0.1:5173").is_ok()
            || std::net::TcpStream::connect("[::1]:5173").is_ok()
        {
            println!("[vite] Ready on port 5173.");
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    println!("[vite] Timed out waiting for port 5173 (continuing anyway).");
}

#[cfg(not(debug_assertions))]
fn start_vite_dev_server() {} // Release builds use the bundled frontend/dist/.

/// If Node.js is not found or port 6000 is already in use, logs and continues.
fn start_econcal_server() {
    std::thread::spawn(|| {
        let econcal_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/econcal");

        // Skip if something is already listening on port 6000
        if std::net::TcpStream::connect("127.0.0.1:6000").is_ok() {
            println!("[econcal] Port 6000 already in use — skipping launch.");
            return;
        }

        println!("[econcal] Starting proxy server (node econcal.js)...");

        // Install dependencies if node_modules is missing
        if !std::path::Path::new(econcal_dir).join("node_modules").exists() {
            println!("[econcal] node_modules not found, running npm install...");
            let mut npm_install = std::process::Command::new("npm");
            npm_install
                .args(["install", "--prefer-offline"])
                .current_dir(econcal_dir)
                .stdin(std::process::Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                npm_install.creation_flags(0x08000000); // CREATE_NO_WINDOW
            }
            match npm_install.status() {
                Ok(s) if s.success() => println!("[econcal] npm install done."),
                Ok(s) => println!("[econcal] npm install exited: {}", s),
                Err(e) => println!("[econcal] npm install failed: {}", e),
            }
        }

        let mut command = std::process::Command::new("node");
        command
            .arg("econcal.js")
            .current_dir(econcal_dir)
            // Detach stdin (see start_vite_dev_server for the rationale). The
            // econcal proxy is even more important here because it spawns
            // puppeteer's Chromium, which leaks console handles aggressively.
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }
        let result = command.spawn();

        match result {
            Err(e) => {
                println!("[econcal] Failed to start: {} (is Node.js installed?)", e);
            }
            Ok(mut child) => {
                ECONCAL_PID.store(child.id(), std::sync::atomic::Ordering::Relaxed);
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

/// Accept WebSocket clients on 127.0.0.1:6001 and stream broadcast messages to each.
/// Every connected client gets its own subscriber to the broadcast channel.
async fn run_ws_server(tick_tx: tokio::sync::broadcast::Sender<String>) {
    // Retry bind: a freshly-killed previous instance can still hold the port briefly
    // (Windows TCP TIME_WAIT). Try every 250ms for up to 10 seconds.
    let listener = {
        let mut attempts: u32 = 0;
        loop {
            match tokio::net::TcpListener::bind("127.0.0.1:6001").await {
                Ok(l) => break l,
                Err(e) if attempts < 40 => {
                    if attempts == 0 {
                        println!("[ws] Port 6001 busy, waiting for it to free: {}", e);
                    }
                    attempts += 1;
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                }
                Err(e) => {
                    println!("[ws] Gave up binding 127.0.0.1:6001 after 10s: {}", e);
                    return;
                }
            }
        }
    };
    println!("[ws] Listening on ws://127.0.0.1:6001");

    loop {
        let (stream, addr) = match listener.accept().await {
            Ok(p) => p,
            Err(e) => {
                println!("[ws] Accept error: {}", e);
                continue;
            }
        };
        let mut sub = tick_tx.subscribe();
        tokio::spawn(async move {
            let ws = match tokio_tungstenite::accept_async(stream).await {
                Ok(ws) => ws,
                Err(e) => {
                    println!("[ws] Handshake failed for {}: {}", addr, e);
                    return;
                }
            };
            println!("[ws] Client connected: {}", addr);
            use futures_util::{SinkExt, StreamExt};
            use tokio_tungstenite::tungstenite::Message;
            let (mut write, mut read) = ws.split();

            // Replay the latest snapshot per message type so this client doesn't
            // wait for the next broadcast to populate state (esp. ec_today/news_today).
            let snapshot: Vec<String> = snapshot_cache().lock()
                .map(|c| c.values().cloned().collect())
                .unwrap_or_default();
            for msg in snapshot {
                if write.send(Message::Text(msg)).await.is_err() {
                    println!("[ws] Client {} dropped during snapshot replay", addr);
                    return;
                }
            }
            loop {
                tokio::select! {
                    msg = sub.recv() => {
                        match msg {
                            Ok(text) => {
                                if write.send(Message::Text(text)).await.is_err() { break; }
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                println!("[ws] {} lagged {} messages", addr, n);
                            }
                            Err(_) => break,
                        }
                    }
                    frame = read.next() => {
                        match frame {
                            Some(Ok(Message::Close(_))) | None => break,
                            Some(Err(_)) => break,
                            _ => {}
                        }
                    }
                }
            }
            println!("[ws] Client disconnected: {}", addr);
        });
    }
}

fn main() {
    // Load environment variables from .env file
    dotenv::dotenv().ok();

    // Ensure the DuckDB parent directory exists. DuckDB cannot create the
    // database file inside a missing directory, so without this every EC/News
    // DB write fails silently (the error is sent over WS, not stdout) and the
    // Calendar/News tabs stay empty while News re-backfills every cycle.
    if let Some(db_dir) = std::path::Path::new(DB_PATH).parent() {
        if let Err(e) = std::fs::create_dir_all(db_dir) {
            println!("[db] WARNING: could not create DB dir {}: {}", db_dir.display(), e);
        }
    }

    // Start the econcal FXStreet proxy server in the background
    start_econcal_server();

    // In dev builds, spawn `npm run dev` for the React frontend and wait until
    // it's listening on 5173 so the Tauri webview has something to load.
    start_vite_dev_server();

    // Create channel for price updates (network -> UI)
    let (tx, rx) = mpsc::channel::<PriceUpdate>(100);

    // Create channels for data retrieval (UI <-> network)
    let (data_req_tx, data_req_rx) = mpsc::channel::<DataRequest>(32);
    let (data_resp_tx, data_resp_rx) = mpsc::channel::<DataResponse>(64);

    // Chart trendbar requests: Tauri commands → session loop. Global sender so
    // any command can publish; receiver moves into the tokio thread.
    let (chart_req_tx, chart_req_rx) = mpsc::channel::<ChartRequest>(32);
    let _ = CHART_REQ_TX.set(chart_req_tx);
    let (order_req_tx, order_req_rx) = mpsc::channel::<OrderRequest>(16);
    let _ = ORDER_REQ_TX.set(order_req_tx);
    let (cancel_req_tx, cancel_req_rx) = mpsc::channel::<CancelRequest>(16);
    let _ = CANCEL_REQ_TX.set(cancel_req_tx);
    let (pos_action_tx, pos_action_rx) = mpsc::channel::<PositionActionRequest>(16);
    let _ = POS_ACTION_TX.set(pos_action_tx);

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
            // Broadcast channel that fans PriceUpdate JSON out to every WS client.
            let (tick_tx, _) = tokio::sync::broadcast::channel::<String>(256);

            // Bridge: pull PriceUpdate off the mpsc, encode as JSON, broadcast.
            let bridge_tx = tick_tx.clone();
            let mut price_rx = rx;
            tokio::spawn(async move {
                while let Some(msg) = price_rx.recv().await {
                    let json = match msg {
                        PriceUpdate::InstrumentPrice { symbol, bid, ask } => {
                            if symbol == "XAUUSD" {
                                LATEST_XAUUSD_BID.store(bid.to_bits(), std::sync::atomic::Ordering::Relaxed);
                                record_xauusd_tick(bid, ask);
                            }
                            serde_json::json!({"type":"tick","symbol":symbol,"bid":bid,"ask":ask})
                        }
                        PriceUpdate::ConnectionStatus(s) => {
                            serde_json::json!({"type":"status","value":s})
                        }
                        PriceUpdate::SymbolMapping(map) => {
                            // Cache for Tauri commands so they can resolve names → IDs.
                            if let Ok(mut cache) = symbol_map().lock() {
                                *cache = map.clone();
                            }
                            continue;  // not broadcast over WS
                        }
                        PriceUpdate::EcStatus(s) => {
                            serde_json::json!({"type":"ec_status","value":s})
                        }
                        PriceUpdate::EcTodayRaw(events) => {
                            let arr: Vec<serde_json::Value> = events.into_iter()
                                .map(|(ts, currency, volatility, name, actual, forecast, previous, surprise)| {
                                    serde_json::json!({
                                        "ts": ts,
                                        "currency": currency,
                                        "volatility": volatility,
                                        "name": name,
                                        "actual": actual,
                                        "forecast": forecast,
                                        "previous": previous,
                                        "surprise": surprise,
                                    })
                                })
                                .collect();
                            serde_json::json!({"type":"ec_today","events":arr})
                        }
                        PriceUpdate::NewsStatus(s) => {
                            serde_json::json!({"type":"news_status","value":s})
                        }
                        PriceUpdate::NewsTodayArticles(articles) => {
                            serde_json::json!({"type":"news_today","articles":articles})
                        }
                        PriceUpdate::MfbStatus(s) => {
                            serde_json::json!({"type":"mfb_status","value":s})
                        }
                        PriceUpdate::MfbTodayRaw(events) => {
                            let arr: Vec<serde_json::Value> = events.into_iter()
                                .map(|(ts, currency, importance, name, country, actual, forecast, previous)| {
                                    serde_json::json!({
                                        "ts": ts, "currency": currency, "volatility": importance,
                                        "name": name, "country": country,
                                        "actual": actual, "forecast": forecast, "previous": previous,
                                    })
                                })
                                .collect();
                            serde_json::json!({"type":"mfb_today","events":arr})
                        }
                        PriceUpdate::MfbNewsStatus(s) => {
                            serde_json::json!({"type":"mfb_news_status","value":s})
                        }
                        PriceUpdate::MfbNewsToday(items) => {
                            let arr: Vec<serde_json::Value> = items.into_iter()
                                .map(|(id, category, title, url, summary, source, published_utc)| {
                                    serde_json::json!({
                                        "article_id": id, "category": category, "title": title,
                                        "url": url, "summary": summary, "source": source,
                                        "published_utc": published_utc,
                                    })
                                })
                                .collect();
                            serde_json::json!({"type":"mfb_news_today","items":arr})
                        }
                        PriceUpdate::FfCalStatus(s) => serde_json::json!({"type":"ff_cal_status","value":s}),
                        PriceUpdate::FfCalToday(events) => {
                            let arr: Vec<serde_json::Value> = events.into_iter()
                                .map(|(ts, currency, impact, title, country, actual, forecast, previous)| {
                                    serde_json::json!({
                                        "ts": ts, "currency": currency, "volatility": impact,
                                        "name": title, "country": country,
                                        "actual": actual, "forecast": forecast, "previous": previous,
                                    })
                                }).collect();
                            serde_json::json!({"type":"ff_cal_today","events":arr})
                        }
                        PriceUpdate::FfNewsStatus(s) => serde_json::json!({"type":"ff_news_status","value":s}),
                        PriceUpdate::FfNewsToday(items) => {
                            let arr: Vec<serde_json::Value> = items.into_iter()
                                .map(|(id, title, url, source, preview, published_utc)| {
                                    serde_json::json!({
                                        "article_id": id, "title": title, "url": url,
                                        "source": source, "preview": preview, "published_utc": published_utc,
                                    })
                                }).collect();
                            serde_json::json!({"type":"ff_news_today","items":arr})
                        }
                        PriceUpdate::PositionsSnapshot(v) => {
                            serde_json::json!({"type":"positions","positions":v.get("positions").cloned().unwrap_or(serde_json::json!([])),"orders":v.get("orders").cloned().unwrap_or(serde_json::json!([]))})
                        }
                        PriceUpdate::TradeNotice(v) => {
                            serde_json::json!({"type":"trade_event","notice":v})
                        }
                        PriceUpdate::AutoStatus(v) => {
                            serde_json::json!({"type":"auto_status","auto":v})
                        }
                        _ => continue,
                    };
                    let serialized = json.to_string();
                    // Cache the latest per-type snapshot so new WS clients can replay.
                    if let Some(t) = json.get("type").and_then(|v| v.as_str()) {
                        // Ticks are per-symbol; key them by "tick:<symbol>" so each
                        // instrument's last price is replayed to new clients instead
                        // of one symbol clobbering another in the cache.
                        let key = match (t, json.get("symbol").and_then(|v| v.as_str())) {
                            ("tick", Some(sym)) => format!("tick:{}", sym),
                            _ => t.to_string(),
                        };
                        if let Ok(mut cache) = snapshot_cache().lock() {
                            cache.insert(key, serialized.clone());
                        }
                    }
                    let _ = bridge_tx.send(serialized);
                }
            });

            // Spawn the WS server (clients connect to ws://127.0.0.1:6001).
            let ws_tx = tick_tx.clone();
            tokio::spawn(async move { run_ws_server(ws_tx).await; });

            // Drain data_resp_rx — the React side will consume responses via the WS
            // once we add a command channel; for now nothing reads it.
            let mut data_resp_drain = data_resp_rx;
            tokio::spawn(async move {
                while data_resp_drain.recv().await.is_some() {}
            });

            // Auto-trade loop (idles until enabled via the UI toggle).
            {
                let auto_tx = tx.clone();
                let auto_db = db_for_async.clone();
                tokio::spawn(async move { run_auto_trade_loop(auto_tx, auto_db).await; });
            }

            // Live tick recorder → xauusd_ticks_live (rolling 2-week window).
            {
                let tick_db = db_for_async.clone();
                tokio::spawn(async move { run_tick_recorder(tick_db).await; });
            }

            // Run price streaming with reconnection
            // request_rx passed by &mut so pending requests survive reconnects
            let mut request_rx = data_req_rx;
            let mut chart_req_rx_holder = chart_req_rx;
            let mut order_req_rx_holder = order_req_rx;
            let mut cancel_req_rx_holder = cancel_req_rx;
            let mut pos_action_rx_holder = pos_action_rx;
            let mut backoff_seconds = 1;
            loop {
                println!("Starting cTrader price stream...");
                match run_session(tx.clone(), &mut request_rx, data_resp_tx.clone(), &mut chart_req_rx_holder, &mut order_req_rx_holder, &mut cancel_req_rx_holder, &mut pos_action_rx_holder, db_for_async.clone()).await {
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

    // data_req_tx is held by Tauri state — future commands will use it to request data.
    let _ = data_req_tx;

    let app_state = AppState { db_mutex: shared_db.clone() };
    tauri::Builder::default()
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            fetch_article_body_on_demand,
            get_mfb_news_body,
            get_gold_sentiment,
            get_last_gold_sentiment,
            get_trendbars,
            get_gold_trade_ideas_multi,
            get_gold_trade_idea,
            get_xrp_trade_idea,
            get_volume_levels,
            get_volume_trade_idea,
            place_gold_order,
            review_pending_order,
            cancel_order,
            review_position,
            close_position,
            amend_position_sltp,
            set_auto_trade,
            get_history_backfill_state,
            start_history_backfill,
            update_news_archive,
            update_ec_gold_events,
            store_ec_gold_events,
            get_xauusd_tf_stats,
            import_disk_archives,
        ])
        .setup(|_app| Ok(()))
        .run(tauri::generate_context!())
        .expect("error while running tauri application");

    // Window closed — clean up children (econcal + Vite dev server) before
    // exiting so their node + puppeteer Chrome processes don't get orphaned.
    shutdown_children();

    // Force exit to kill background threads (tokio runtime, WS server, etc.).
    std::process::exit(0);
}

async fn run_session(
    tx: mpsc::Sender<PriceUpdate>,
    request_rx: &mut mpsc::Receiver<DataRequest>,
    response_tx: mpsc::Sender<DataResponse>,
    chart_req_rx: &mut mpsc::Receiver<ChartRequest>,
    order_req_rx: &mut mpsc::Receiver<OrderRequest>,
    cancel_req_rx: &mut mpsc::Receiver<CancelRequest>,
    pos_action_rx: &mut mpsc::Receiver<PositionActionRequest>,
    shared_db: SharedDb,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Local map: client_msg_id → oneshot reply for in-flight chart requests.
    let mut pending_chart_reqs: std::collections::HashMap<
        String,
        tokio::sync::oneshot::Sender<Result<Vec<Candle>, String>>,
    > = std::collections::HashMap::new();
    let mut chart_msg_id_counter: u64 = 0;
    // Local map: client_msg_id → oneshot reply for in-flight order requests.
    let mut pending_order_reqs: std::collections::HashMap<
        String,
        tokio::sync::oneshot::Sender<Result<String, String>>,
    > = std::collections::HashMap::new();
    let mut order_msg_id_counter: u64 = 0;
    // Live account state for the Positions panel, kept in sync via reconcile +
    // execution events.
    let mut open_positions: std::collections::HashMap<i64, openapi::ProtoOaPosition> =
        std::collections::HashMap::new();
    let mut pending_orders: std::collections::HashMap<i64, openapi::ProtoOaOrder> =
        std::collections::HashMap::new();
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
        .unwrap_or_else(|_| "XAUUSD".to_string());

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

    // Symbols to subscribe to live spot prices (driven by CTRADER_SYMBOL env var).
    // EURUSD is always added so it shows in the sidebar alongside the primary symbol.
    let mut instruments_to_subscribe: Vec<&str> = vec![target_symbol.as_str()];
    for extra in ["EURUSD", "XRPUSD"] {
        if !instruments_to_subscribe.contains(&extra) {
            instruments_to_subscribe.push(extra);
        }
    }
    // All symbols whose IDs we need (cross-pairs for M1 data downloads, ID lookup only)
    let instruments_need_id: Vec<&str> = vec![
        "EURUSD", "XRPUSD", "BTCUSD", // BTCUSD: master-filter context for the XRP 5m agent
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
    // EC + News default to always-on since the React dashboard surfaces them passively.
    let mut ec_capturing = true;

    // ── News capture state ───────────────────────────────────────────────
    let mut news_capturing = true;
    let mut news_next_fetch: Option<tokio::time::Instant> = None;
    let mut news_today_date: Option<String> = None;

    // MyFXBook calendar: fetch the current week (incl. today) on a schedule,
    // upsert into the archive table, write per-day JSON, and push to the UI.
    let mut mfb_next_fetch: Option<tokio::time::Instant> =
        Some(tokio::time::Instant::now() + Duration::from_secs(12));
    // MyFXBook news/analysis/press-release fetch — staggered after the calendar.
    let mut mfb_news_next_fetch: Option<tokio::time::Instant> =
        Some(tokio::time::Instant::now() + Duration::from_secs(18));
    // ForexFactory calendar + news — staggered after the MyFXBook fetches.
    let mut ff_cal_next_fetch: Option<tokio::time::Instant> =
        Some(tokio::time::Instant::now() + Duration::from_secs(24));
    let mut ff_news_next_fetch: Option<tokio::time::Instant> =
        Some(tokio::time::Instant::now() + Duration::from_secs(30));

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

                                // Fetch the full XAUUSD symbol spec (digits / min /
                                // step / max volume) so live orders can be sized and
                                // priced safely. Cached on the 2117 response.
                                if let Some(xau_id) = symbol_id_to_name.iter()
                                    .find(|(_, n)| n.as_str() == "XAUUSD").map(|(&id, _)| id)
                                {
                                    let spec_req = openapi::ProtoOaSymbolByIdReq {
                                        payload_type: Some(openapi::ProtoOaPayloadType::ProtoOaSymbolByIdReq as i32),
                                        ctid_trader_account_id: account_id,
                                        symbol_id: vec![xau_id],
                                    };
                                    let _ = send_message(
                                        &mut tls_stream,
                                        openapi::ProtoOaPayloadType::ProtoOaSymbolByIdReq as u32,
                                        spec_req,
                                    ).await;
                                }

                                // Seed the Positions panel with current open positions
                                // + pending orders. Ongoing changes arrive as execution
                                // events, which trigger a fresh reconcile.
                                let recon = openapi::ProtoOaReconcileReq {
                                    payload_type: Some(openapi::ProtoOaPayloadType::ProtoOaReconcileReq as i32),
                                    ctid_trader_account_id: account_id,
                                    return_protection_orders: Some(false),
                                };
                                let _ = send_message(
                                    &mut tls_stream,
                                    openapi::ProtoOaPayloadType::ProtoOaReconcileReq as u32,
                                    recon,
                                ).await;

                                // (Note: the one-shot history backfill is NOT auto-spawned
                                // anymore — it's triggered manually by the
                                // `XAUUSD_History_Update` button in the Archives tab. The
                                // backfill function is the same; only the trigger changed.)

                                // Spawn the background price-refresh loop so the chart-cache
                                // tables (xauusd_m1/m5/h1/d1) stay fresh even when the user
                                // doesn't have the chart open. The Trade Ideas agent reads
                                // from these tables and needs current data on every click.
                                {
                                    let sym = target_symbol.clone();
                                    let db = shared_db.clone();
                                    tokio::spawn(async move {
                                        run_price_refresh_loop(sym, db).await;
                                    });
                                }
                            } else {
                                println!("Error: No symbols found in account symbol list.");
                                break;
                            }
                        }
                    },
                    2117 => { // ProtoOASymbolByIdRes — cache XAUUSD trading spec
                        if let Some(payload) = &msg.payload {
                            if let Ok(res) = openapi::ProtoOaSymbolByIdRes::decode(payload.as_slice()) {
                                if let Some(sym) = res.symbol.first() {
                                    let spec = SymbolSpec {
                                        symbol_id: sym.symbol_id,
                                        digits: sym.digits,
                                        min_volume: sym.min_volume.unwrap_or(0),
                                        step_volume: sym.step_volume.unwrap_or(0),
                                        max_volume: sym.max_volume.unwrap_or(i64::MAX),
                                    };
                                    println!("[order] XAUUSD spec: digits={} min_vol={} step_vol={} max_vol={}",
                                             spec.digits, spec.min_volume, spec.step_volume, spec.max_volume);
                                    if let Ok(mut g) = xauusd_spec().lock() { *g = Some(spec); }
                                }
                            }
                        }
                    },
                    2125 => { // ProtoOAReconcileRes — full positions + pending orders
                        if let Some(payload) = &msg.payload {
                            if let Ok(res) = openapi::ProtoOaReconcileRes::decode(payload.as_slice()) {
                                open_positions.clear();
                                for p in res.position { open_positions.insert(p.position_id, p); }
                                pending_orders.clear();
                                for o in res.order {
                                    // Skip protection/closing orders — show entry orders only.
                                    if o.closing_order != Some(true) {
                                        pending_orders.insert(o.order_id, o);
                                    }
                                }
                                let snap = build_positions_snapshot(&open_positions, &pending_orders, &symbol_id_to_name);
                                let _ = tx.send(PriceUpdate::PositionsSnapshot(snap)).await;

                                // Refresh XAUUSD-only account state for the auto-trade loop.
                                if let Some(xid) = symbol_id_to_name.iter()
                                    .find(|(_, n)| n.as_str() == "XAUUSD").map(|(&id, _)| id)
                                {
                                    let positions = open_positions.values()
                                        .filter(|p| p.trade_data.symbol_id == xid).count();
                                    let pending_order_ids: Vec<i64> = pending_orders.values()
                                        .filter(|o| o.trade_data.symbol_id == xid)
                                        .map(|o| o.order_id).collect();
                                    if let Ok(mut s) = xau_account_state().lock() {
                                        *s = XauAccountState { positions, pending_order_ids };
                                    }
                                }
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
                                    // Suppressed live-tick log (was: "LIVE XAUUSD | Bid: X | Ask: Y")
                                    // — ran on every spot event and flooded the terminal.
                                    // Decimals var still used below for formatting elsewhere
                                    // if needed; keep the computation in case future code wants it.
                                    let _ = decimals;
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
                            // Did this response come from a chart request? Match on client_msg_id.
                            let chart_reply = msg.client_msg_id.as_ref()
                                .and_then(|id| pending_chart_reqs.remove(id));
                            if let Some(reply_tx) = chart_reply {
                                match openapi::ProtoOaGetTrendbarsRes::decode(payload.as_slice()) {
                                    Ok(res) => {
                                        let candles: Vec<Candle> = res.trendbar.iter()
                                            .filter_map(|bar| trendbar_to_candle(
                                                bar.volume, bar.low, bar.delta_open,
                                                bar.delta_close, bar.delta_high,
                                                bar.utc_timestamp_in_minutes,
                                            ))
                                            .collect();
                                        let _ = reply_tx.send(Ok(candles));
                                    }
                                    Err(e) => { let _ = reply_tx.send(Err(format!("decode: {}", e))); }
                                }
                            } else {
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
                        }
                    },
                    2126 => { // ProtoOAExecutionEvent
                        let parsed = msg.payload.as_ref()
                            .and_then(|p| openapi::ProtoOaExecutionEvent::decode(p.as_slice()).ok());
                        // 1. If this answers our place_gold_order, reply to that command.
                        if let Some(reply) = msg.client_msg_id.as_ref()
                            .and_then(|id| pending_order_reqs.remove(id))
                        {
                            match &parsed {
                                Some(ev) => {
                                    let et = openapi::ProtoOaExecutionType::try_from(ev.execution_type)
                                        .map(|e| e.as_str_name()).unwrap_or("UNKNOWN");
                                    let rejected = matches!(ev.execution_type, 7 | 8); // REJECTED / CANCEL_REJECTED
                                    if rejected || ev.error_code.is_some() {
                                        let _ = reply.send(Err(format!(
                                            "{}{}", et,
                                            ev.error_code.clone().map(|c| format!(" ({})", c)).unwrap_or_default()
                                        )));
                                    } else {
                                        let pos = ev.position.as_ref().map(|p| p.position_id);
                                        let _ = reply.send(Ok(format!(
                                            "{}{}", et,
                                            pos.map(|id| format!(" · position {}", id)).unwrap_or_default()
                                        )));
                                    }
                                }
                                None => { let _ = reply.send(Err("empty execution event".into())); }
                            }
                        }
                        // 2. Lifecycle event: emit a close notice (using pre-close state),
                        //    then refresh the panel with a fresh reconcile.
                        if let Some(ev) = &parsed {
                            maybe_emit_close_notice(ev, &open_positions, &symbol_id_to_name, &tx).await;
                        }
                        let recon = openapi::ProtoOaReconcileReq {
                            payload_type: Some(openapi::ProtoOaPayloadType::ProtoOaReconcileReq as i32),
                            ctid_trader_account_id: account_id,
                            return_protection_orders: Some(false),
                        };
                        let _ = send_message(
                            &mut tls_stream,
                            openapi::ProtoOaPayloadType::ProtoOaReconcileReq as u32,
                            recon,
                        ).await;
                    },
                    2132 => { // ProtoOAOrderErrorEvent — resolves a pending order with failure
                        if let Some(reply) = msg.client_msg_id.as_ref()
                            .and_then(|id| pending_order_reqs.remove(id))
                        {
                            let detail = msg.payload.as_ref()
                                .and_then(|p| openapi::ProtoOaOrderErrorEvent::decode(p.as_slice()).ok())
                                .map(|e| format!("{}{}", e.error_code,
                                    e.description.map(|d| format!(" - {}", d)).unwrap_or_default()))
                                .unwrap_or_else(|| "order error".into());
                            let _ = reply.send(Err(detail));
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

                            // If this error answers a pending order, fail that order.
                            if let Some(reply) = msg.client_msg_id.as_ref()
                                .and_then(|id| pending_order_reqs.remove(id))
                            {
                                let _ = reply.send(Err(format!("{} - {}", err.error_code, desc)));
                            }

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
            // Chart-history request from a Tauri command: send ProtoOAGetTrendbarsReq
            // with a unique client_msg_id and remember the oneshot so we can reply.
            Some(chart_req) = chart_req_rx.recv() => {
                chart_msg_id_counter += 1;
                let cid = format!("chart-{}", chart_msg_id_counter);
                pending_chart_reqs.insert(cid.clone(), chart_req.reply);
                let req = openapi::ProtoOaGetTrendbarsReq {
                    payload_type: Some(openapi::ProtoOaPayloadType::ProtoOaGetTrendbarsReq as i32),
                    ctid_trader_account_id: account_id,
                    from_timestamp: Some(chart_req.from_ms),
                    to_timestamp: Some(chart_req.to_ms),
                    period: chart_req.period as i32,
                    symbol_id: chart_req.symbol_id,
                    count: Some(chart_req.count),
                };
                if let Err(e) = send_message_with_id(
                    &mut tls_stream,
                    openapi::ProtoOaPayloadType::ProtoOaGetTrendbarsReq as u32,
                    req,
                    &cid,
                ).await {
                    // Send failed: fail the pending request so the Tauri command unblocks.
                    if let Some(reply_tx) = pending_chart_reqs.remove(&cid) {
                        let _ = reply_tx.send(Err(format!("send failed: {}", e)));
                    }
                }
            }
            // LIVE order placement: build a ProtoOANewOrderReq with a unique
            // client_msg_id so the ExecutionEvent / error can be matched back.
            Some(order_req) = order_req_rx.recv() => {
                order_msg_id_counter += 1;
                let cid = format!("order-{}", order_msg_id_counter);
                pending_order_reqs.insert(cid.clone(), order_req.reply);
                let req = openapi::ProtoOaNewOrderReq {
                    payload_type: Some(openapi::ProtoOaPayloadType::ProtoOaNewOrderReq as i32),
                    ctid_trader_account_id: account_id,
                    symbol_id: order_req.symbol_id,
                    order_type: order_req.order_type as i32,
                    trade_side: order_req.trade_side as i32,
                    volume: order_req.volume,
                    limit_price: order_req.limit_price,
                    stop_price: order_req.stop_price,
                    stop_loss: order_req.stop_loss,
                    take_profit: order_req.take_profit,
                    relative_stop_loss: order_req.rel_sl,
                    relative_take_profit: order_req.rel_tp,
                    label: Some(order_req.label),
                    ..Default::default()
                };
                println!("[order] sending type={:?} {} vol={} limit={:?} stop={:?} sl={:?} tp={:?}",
                         order_req.order_type, req.trade_side, req.volume,
                         req.limit_price, req.stop_price, req.stop_loss, req.take_profit);
                if let Err(e) = send_message_with_id(
                    &mut tls_stream,
                    openapi::ProtoOaPayloadType::ProtoOaNewOrderReq as u32,
                    req,
                    &cid,
                ).await {
                    if let Some(reply_tx) = pending_order_reqs.remove(&cid) {
                        let _ = reply_tx.send(Err(format!("send failed: {}", e)));
                    }
                }
            }
            // Cancel a resting pending order. The ORDER_CANCELLED execution event
            // (matched by client_msg_id) resolves the reply via pending_order_reqs.
            Some(cancel_req) = cancel_req_rx.recv() => {
                order_msg_id_counter += 1;
                let cid = format!("cancel-{}", order_msg_id_counter);
                pending_order_reqs.insert(cid.clone(), cancel_req.reply);
                let req = openapi::ProtoOaCancelOrderReq {
                    payload_type: Some(openapi::ProtoOaPayloadType::ProtoOaCancelOrderReq as i32),
                    ctid_trader_account_id: account_id,
                    order_id: cancel_req.order_id,
                };
                println!("[order] cancelling order {}", cancel_req.order_id);
                if let Err(e) = send_message_with_id(
                    &mut tls_stream,
                    openapi::ProtoOaPayloadType::ProtoOaCancelOrderReq as u32,
                    req,
                    &cid,
                ).await {
                    if let Some(reply_tx) = pending_order_reqs.remove(&cid) {
                        let _ = reply_tx.send(Err(format!("send failed: {}", e)));
                    }
                }
            }
            // Close / amend an open position. The resulting ExecutionEvent
            // (matched by client_msg_id) resolves the reply via pending_order_reqs.
            Some(pa) = pos_action_rx.recv() => {
                order_msg_id_counter += 1;
                let cid = format!("posact-{}", order_msg_id_counter);
                pending_order_reqs.insert(cid.clone(), pa.reply);
                let send_res = match pa.action {
                    PositionAction::Close { position_id, volume } => {
                        let req = openapi::ProtoOaClosePositionReq {
                            payload_type: Some(openapi::ProtoOaPayloadType::ProtoOaClosePositionReq as i32),
                            ctid_trader_account_id: account_id,
                            position_id,
                            volume,
                        };
                        println!("[order] closing position {} vol={}", position_id, volume);
                        send_message_with_id(
                            &mut tls_stream,
                            openapi::ProtoOaPayloadType::ProtoOaClosePositionReq as u32,
                            req, &cid,
                        ).await
                    }
                    PositionAction::AmendSltp { position_id, stop_loss, take_profit } => {
                        let req = openapi::ProtoOaAmendPositionSltpReq {
                            payload_type: Some(openapi::ProtoOaPayloadType::ProtoOaAmendPositionSltpReq as i32),
                            ctid_trader_account_id: account_id,
                            position_id,
                            stop_loss,
                            take_profit,
                            ..Default::default()
                        };
                        println!("[order] amend SL/TP position {} sl={:?} tp={:?}", position_id, stop_loss, take_profit);
                        send_message_with_id(
                            &mut tls_stream,
                            openapi::ProtoOaPayloadType::ProtoOaAmendPositionSltpReq as u32,
                            req, &cid,
                        ).await
                    }
                };
                if let Err(e) = send_res {
                    if let Some(reply_tx) = pending_order_reqs.remove(&cid) {
                        let _ = reply_tx.send(Err(format!("send failed: {}", e)));
                    }
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
                                    // Also feed today's gold-relevant events into the
                                    // archive table so xauusd_economic_calendar (and
                                    // thus the auto JSON export below) stays current
                                    // without waiting for a manual EC_Gold_Events_Update.
                                    let _ = ec_realtime::upsert_xauusd_ec(&db, &rows);
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

                        // Auto-export the EC calendar JSON files (same work as the
                        // EC_Gold_events_storage button) off the session loop, so
                        // ec_events_data/all/ stays current as today's events fill
                        // in. Diff-aware (only rewrites changed days) + guarded so a
                        // slow run never overlaps the next EC fetch.
                        use std::sync::atomic::Ordering as EcOrd;
                        if !EC_STORE_BUSY.swap(true, EcOrd::AcqRel) {
                            let db = shared_db.clone();
                            tokio::spawn(async move {
                                match run_ec_gold_store(db, None).await {
                                    Ok(r) if !r.up_to_date => println!(
                                        "[ec-store] auto: {} file(s), {} event(s) written",
                                        r.files_written, r.events_written),
                                    Ok(_) => {}  // up-to-date: nothing to write, stay quiet
                                    Err(e) => println!("[ec-store] auto failed: {}", e),
                                }
                                EC_STORE_BUSY.store(false, EcOrd::Release);
                            });
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
                // Schedule next fetch in 5 minutes
                news_next_fetch = Some(
                    tokio::time::Instant::now() + Duration::from_secs(300)
                );

                // Look up the latest article we already have so fetch_news_since
                // can stop once it walks past our existing data. On a fresh DB or
                // after a long downtime, this lets the next fetch walk back many
                // pages to backfill the gap; on a hot cycle it stops after page 1.
                let db_clone = shared_db.clone();
                let since: Option<String> = tokio::task::spawn_blocking(move || {
                    let _lock = db_clone.lock().unwrap();
                    duckdb::Connection::open(DB_PATH)
                        .ok()
                        .and_then(|db| news_realtime::get_latest_news_timestamp(&db))
                })
                .await
                .unwrap_or(None);

                if since.is_some() {
                    println!("News: backfilling since {}", since.as_deref().unwrap_or(""));
                }

                match news_realtime::fetch_news_since(50, 200, since.as_deref(), None).await {
                    Ok(rows) => {
                        let fetched = rows.len();

                        let db_clone = shared_db.clone();
                        let today_rows = tokio::task::spawn_blocking(move || {
                            let _lock = db_clone.lock().unwrap();
                            match duckdb::Connection::open(DB_PATH) {
                                Ok(db) => {
                                    let _ = news_realtime::write_news_to_db(&db, &rows);
                                    news_realtime::read_news_today(&db)
                                }
                                Err(_) => Vec::new(),
                            }
                        })
                        .await
                        .unwrap_or_default();

                        // Status shows TODAY's article count (what the tab displays),
                        // with how many were newly fetched this cycle in parentheses.
                        let today_count = today_rows.len();
                        let now_str = chrono::Local::now().format("%H:%M:%S").to_string();
                        let _ = tx.send(PriceUpdate::NewsStatus(format!(
                            "{} articles (+{} new) | next: 5m | updated: {}",
                            today_count, fetched, now_str
                        ))).await;
                        let _ = tx.send(PriceUpdate::NewsTodayArticles(today_rows)).await;

                        // Auto-archive on the same cadence: backfill article bodies
                        // and (re)write the per-day JSON files — exactly what the
                        // News_Updates button does — but OFF the session loop so the
                        // rate-limited body fetches never block TLS reads / chart
                        // requests. Guarded so a slow cycle never overlaps the next.
                        use std::sync::atomic::Ordering;
                        if !NEWS_ARCHIVE_BUSY.swap(true, Ordering::AcqRel) {
                            let db = shared_db.clone();
                            tokio::spawn(async move {
                                match run_news_archive_update(db).await {
                                    Ok(r) => println!(
                                        "[news-archive] auto: {} fetched, {} bodies (+{} empty), {} day-file(s){}",
                                        r.articles_fetched, r.bodies_fetched, r.bodies_empty, r.days_written.len(),
                                        if r.rate_limited { " — rate-limited, resumes next cycle" } else { "" },
                                    ),
                                    Err(e) => println!("[news-archive] auto failed: {}", e),
                                }
                                NEWS_ARCHIVE_BUSY.store(false, Ordering::Release);
                            });
                        } else {
                            println!("[news-archive] auto: previous cycle still running — skipping");
                        }
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

        // ── External web fetches (MyFXBook / ForexFactory) ───────────────
        // These hit external (Cloudflare-fronted) sites via curl, which can be
        // slow or hang — so unlike the local EC/news fetches they run in SPAWNED
        // tasks, NEVER inline, so they can't stall cTrader tick processing. Each
        // is guarded by an AtomicBool so a slow run never overlaps the next.
        use std::sync::atomic::Ordering as ExtOrd;
        let now_inst = tokio::time::Instant::now();

        if _auth_state == AuthState::Subscribed
            && matches!(mfb_next_fetch, Some(t) if now_inst >= t)
            && !MFB_CAL_BUSY.swap(true, ExtOrd::AcqRel)
        {
            mfb_next_fetch = Some(now_inst + Duration::from_secs(300));
            let (tx, db) = (tx.clone(), shared_db.clone());
            tokio::spawn(async move {
                match myfxbook_cal::fetch_calendar().await {
                    Ok(events) => {
                        let week = events.len();
                        let days: std::collections::BTreeSet<String> = events.iter()
                            .filter_map(|e| if e.timestamp_utc.len() >= 10 { Some(e.timestamp_utc[..10].to_string()) } else { None }).collect();
                        let today_rows = tokio::task::spawn_blocking(move || {
                            let _lock = db.lock().unwrap();
                            match duckdb::Connection::open(DB_PATH) {
                                Ok(db) => {
                                    let _ = myfxbook_cal::upsert_mfb(&db, &events);
                                    let _ = myfxbook_cal::write_today_from_archive(&db);
                                    for day in &days { let _ = myfxbook_cal::write_archive_day(&db, MFB_ARCHIVE_ROOT, day); }
                                    myfxbook_cal::read_today(&db)
                                }
                                Err(_) => Vec::new(),
                            }
                        }).await.unwrap_or_default();
                        let now_str = chrono::Local::now().format("%H:%M:%S").to_string();
                        let _ = tx.send(PriceUpdate::MfbStatus(format!("{} events today ({} this week) | next: 5m | updated: {}", today_rows.len(), week, now_str))).await;
                        let _ = tx.send(PriceUpdate::MfbTodayRaw(today_rows)).await;
                    }
                    Err(e) => { let _ = tx.send(PriceUpdate::MfbStatus(format!("MyFXBook error: {}", e))).await; }
                }
                MFB_CAL_BUSY.store(false, ExtOrd::Release);
            });
        }

        if _auth_state == AuthState::Subscribed
            && matches!(mfb_news_next_fetch, Some(t) if now_inst >= t)
            && !MFB_NEWS_BUSY.swap(true, ExtOrd::AcqRel)
        {
            mfb_news_next_fetch = Some(now_inst + Duration::from_secs(300));
            let (tx, db) = (tx.clone(), shared_db.clone());
            tokio::spawn(async move {
                match myfxbook_news::fetch_all().await {
                    Ok(items) => {
                        let total = items.len();
                        let days: std::collections::BTreeSet<String> = items.iter()
                            .filter_map(|i| if i.published_utc.len() >= 10 { Some(i.published_utc[..10].to_string()) } else { None }).collect();
                        let db2 = db.clone();
                        let today_rows = tokio::task::spawn_blocking(move || {
                            let _lock = db2.lock().unwrap();
                            match duckdb::Connection::open(DB_PATH) {
                                Ok(db) => {
                                    let _ = myfxbook_news::upsert(&db, &items);
                                    let _ = myfxbook_news::write_today_from_archive(&db);
                                    for day in &days { let _ = myfxbook_news::write_archive_day(&db, MFB_NEWS_ARCHIVE_ROOT, day); }
                                    myfxbook_news::read_today(&db)
                                }
                                Err(_) => Vec::new(),
                            }
                        }).await.unwrap_or_default();
                        let now_str = chrono::Local::now().format("%H:%M:%S").to_string();
                        let _ = tx.send(PriceUpdate::MfbNewsStatus(format!("{} items today ({} fetched) | next: 5m | updated: {}", today_rows.len(), total, now_str))).await;
                        let _ = tx.send(PriceUpdate::MfbNewsToday(today_rows)).await;
                        if !MFB_NEWS_BODY_BUSY.swap(true, ExtOrd::AcqRel) {
                            run_mfb_news_body_backfill(db).await;
                            MFB_NEWS_BODY_BUSY.store(false, ExtOrd::Release);
                        }
                    }
                    Err(e) => { let _ = tx.send(PriceUpdate::MfbNewsStatus(format!("MyFXBook news error: {}", e))).await; }
                }
                MFB_NEWS_BUSY.store(false, ExtOrd::Release);
            });
        }

        if _auth_state == AuthState::Subscribed
            && matches!(ff_cal_next_fetch, Some(t) if now_inst >= t)
            && !FF_CAL_BUSY.swap(true, ExtOrd::AcqRel)
        {
            ff_cal_next_fetch = Some(now_inst + Duration::from_secs(300));
            let (tx, db) = (tx.clone(), shared_db.clone());
            tokio::spawn(async move {
                match forexfactory::fetch_calendar().await {
                    Ok(events) => {
                        let total = events.len();
                        let days: std::collections::BTreeSet<String> = events.iter()
                            .filter_map(|e| if e.timestamp_utc.len() >= 10 { Some(e.timestamp_utc[..10].to_string()) } else { None }).collect();
                        let today_rows = tokio::task::spawn_blocking(move || {
                            let _lock = db.lock().unwrap();
                            match duckdb::Connection::open(DB_PATH) {
                                Ok(db) => {
                                    let _ = forexfactory::upsert_cal(&db, &events);
                                    let _ = forexfactory::write_cal_today(&db);
                                    for day in &days { let _ = forexfactory::write_cal_archive_day(&db, FF_CAL_ARCHIVE_ROOT, day); }
                                    forexfactory::read_cal_today(&db)
                                }
                                Err(_) => Vec::new(),
                            }
                        }).await.unwrap_or_default();
                        let now_str = chrono::Local::now().format("%H:%M:%S").to_string();
                        let _ = tx.send(PriceUpdate::FfCalStatus(format!("{} events today ({} fetched) | next: 5m | updated: {}", today_rows.len(), total, now_str))).await;
                        let _ = tx.send(PriceUpdate::FfCalToday(today_rows)).await;
                    }
                    Err(e) => { let _ = tx.send(PriceUpdate::FfCalStatus(format!("ForexFactory calendar error: {}", e))).await; }
                }
                FF_CAL_BUSY.store(false, ExtOrd::Release);
            });
        }

        if _auth_state == AuthState::Subscribed
            && matches!(ff_news_next_fetch, Some(t) if now_inst >= t)
            && !FF_NEWS_BUSY.swap(true, ExtOrd::AcqRel)
        {
            ff_news_next_fetch = Some(now_inst + Duration::from_secs(300));
            let (tx, db) = (tx.clone(), shared_db.clone());
            tokio::spawn(async move {
                match forexfactory::fetch_news().await {
                    Ok(items) => {
                        let total = items.len();
                        let days: std::collections::BTreeSet<String> = items.iter()
                            .filter_map(|i| if i.published_utc.len() >= 10 { Some(i.published_utc[..10].to_string()) } else { None }).collect();
                        let db2 = db.clone();
                        let today_rows = tokio::task::spawn_blocking(move || {
                            let _lock = db2.lock().unwrap();
                            match duckdb::Connection::open(DB_PATH) {
                                Ok(db) => {
                                    let _ = forexfactory::upsert_news(&db, &items);
                                    let _ = forexfactory::write_news_today(&db);
                                    for day in &days { let _ = forexfactory::write_news_archive_day(&db, FF_NEWS_ARCHIVE_ROOT, day); }
                                    forexfactory::read_news_today(&db)
                                }
                                Err(_) => Vec::new(),
                            }
                        }).await.unwrap_or_default();
                        let now_str = chrono::Local::now().format("%H:%M:%S").to_string();
                        let _ = tx.send(PriceUpdate::FfNewsStatus(format!("{} items today ({} fetched) | next: 5m | updated: {}", today_rows.len(), total, now_str))).await;
                        let _ = tx.send(PriceUpdate::FfNewsToday(today_rows)).await;
                        if !FF_NEWS_BODY_BUSY.swap(true, ExtOrd::AcqRel) {
                            run_ff_news_body_backfill(db).await;
                            FF_NEWS_BODY_BUSY.store(false, ExtOrd::Release);
                        }
                    }
                    Err(e) => { let _ = tx.send(PriceUpdate::FfNewsStatus(format!("ForexFactory news error: {}", e))).await; }
                }
                FF_NEWS_BUSY.store(false, ExtOrd::Release);
            });
        }
    }

    Ok(())
}

async fn send_message<T: Message>(
    stream: &mut tokio_rustls::client::TlsStream<TcpStream>,
    payload_type: u32,
    payload: T,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    send_message_with_id(stream, payload_type, payload, "1").await
}

async fn send_message_with_id<T: Message>(
    stream: &mut tokio_rustls::client::TlsStream<TcpStream>,
    payload_type: u32,
    payload: T,
    client_msg_id: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut body = Vec::new();
    payload.encode(&mut body)?;
    println!("DEBUG send: payload_type={}, body_len={}", payload_type, body.len());

    let proto_msg = openapi::ProtoMessage {
        payload_type,
        payload: Some(body),
        client_msg_id: Some(client_msg_id.to_string()),
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
