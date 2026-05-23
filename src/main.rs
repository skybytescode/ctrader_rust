use prost::Message;
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

const DB_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/Bots_db/Algo_EURUSD.duckdb");

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

/// PIDs of spawned child processes that should be killed when the app exits.
/// 0 = slot not in use.
static ECONCAL_PID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static VITE_PID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

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

/// Cache of symbol name → cTrader symbol_id, populated by the bridge task each
/// time the session emits a SymbolMapping. Tauri commands resolve names here.
static SYMBOL_MAP: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, i64>>>
    = std::sync::OnceLock::new();

fn symbol_map() -> &'static std::sync::Mutex<std::collections::HashMap<String, i64>> {
    SYMBOL_MAP.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

// ── Archive: per-day all-articles JSON files ────────────────────────────────

/// Tauri-managed state shared with command handlers.
struct AppState {
    db_mutex: SharedDb,
}

#[derive(serde::Serialize)]
struct ArchiveDayResult {
    status: String,           // "written" | "done" | "error"
    day: Option<String>,      // YYYY-MM-DD of the day just written
    count: usize,             // articles in that file
    path: Option<String>,     // absolute path to the file
    remaining_days: usize,    // days in DB still unwritten
    message: Option<String>,  // human-readable message
}

const ARCHIVE_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/news_data/all");

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

/// Compute the next day that still has articles without a body. Walks days
/// chronologically and returns the oldest day with at least one NULL body.
/// `year_prefix` (e.g. Some("2026")) limits the search to that year only.
fn find_next_day_to_write(
    db: &duckdb::Connection,
    year_prefix: Option<&str>,
) -> Result<Option<(String, usize)>, String> {
    let (sql, has_filter) = if year_prefix.is_some() {
        ("SELECT substr(published_utc, 1, 10) AS d, COUNT(*) AS n
          FROM news_historical
          WHERE length(published_utc) >= 10
            AND body IS NULL
            AND substr(published_utc, 1, 4) = ?
          GROUP BY d
          ORDER BY d ASC", true)
    } else {
        ("SELECT substr(published_utc, 1, 10) AS d, COUNT(*) AS n
          FROM news_historical
          WHERE length(published_utc) >= 10
            AND body IS NULL
          GROUP BY d
          ORDER BY d ASC", false)
    };
    let mut stmt = db.prepare(sql).map_err(|e| format!("prepare days: {}", e))?;
    let map_row = |row: &duckdb::Row<'_>| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize));
    let rows: Vec<(String, usize)> = if has_filter {
        stmt.query_map([year_prefix.unwrap()], map_row)
            .map_err(|e| format!("query days: {}", e))?
            .flatten().collect()
    } else {
        stmt.query_map([], map_row)
            .map_err(|e| format!("query days: {}", e))?
            .flatten().collect()
    };
    let total_remaining = rows.len();
    Ok(rows.into_iter().next().map(|(day, _)| (day, total_remaining)))
}

/// Build the JSON for a single day from news_historical and write it to disk.
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
            "article_id":   row.get::<_, String>(0)?,
            "title":        row.get::<_, String>(1)?,
            "published_utc": row.get::<_, String>(2)?,
            "summary":      row.get::<_, Option<String>>(3)?.unwrap_or_default(),
            "url":          row.get::<_, Option<String>>(4)?.unwrap_or_default(),
            "author":       row.get::<_, Option<String>>(5)?.unwrap_or_default(),
            "tags":         row.get::<_, Option<String>>(6)?.unwrap_or_default(),
            "hour_utc":     row.get::<_, i32>(7)?,
            "weekday":      row.get::<_, i32>(8)?,
            "body":         body_val,
        }))
    }).map_err(|e| format!("query archive: {}", e))?;

    let articles: Vec<serde_json::Value> = rows.flatten().collect();
    let count = articles.len();

    let month = &day[..7];
    let dir = std::path::Path::new(ARCHIVE_ROOT).join(month);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create dir {}: {}", dir.display(), e))?;
    let path = dir.join(format!("{}.json", day));

    let payload = serde_json::json!({
        "date": day,
        "count": count,
        "generated_at": chrono::Utc::now().to_rfc3339(),
        "articles": articles,
    });
    let pretty = serde_json::to_string_pretty(&payload).map_err(|e| format!("serialize: {}", e))?;
    std::fs::write(&path, pretty).map_err(|e| format!("write {}: {}", path.display(), e))?;

    Ok((path.to_string_lossy().to_string(), count))
}

/// Tauri command: pick the oldest un-archived day, fetch missing article bodies
/// in parallel (10 concurrent against fxstreet.com), update `news_historical.body`,
/// then write the day's archive file. Each call advances by one day.
#[tauri::command]
async fn write_next_archive_day(state: tauri::State<'_, AppState>) -> Result<ArchiveDayResult, String> {
    write_next_archive_day_inner(state, None).await
}

/// Same as `write_next_archive_day` but restricted to days in `year` (e.g. "2026").
#[tauri::command]
async fn write_next_archive_day_for_year(year: String, state: tauri::State<'_, AppState>) -> Result<ArchiveDayResult, String> {
    write_next_archive_day_inner(state, Some(year)).await
}

async fn write_next_archive_day_inner(
    state: tauri::State<'_, AppState>,
    year_filter: Option<String>,
) -> Result<ArchiveDayResult, String> {
    // Step 0: make sure econcal is reachable. The proxy is prone to puppeteer-
    // memory crashes over multi-hour sessions; this self-heals before each click.
    if !ensure_econcal_alive().await {
        return Ok(ArchiveDayResult {
            status: "error".into(),
            day: None, count: 0, path: None, remaining_days: 0,
            message: Some("Econcal proxy not reachable on :6000 and respawn failed. Restart the app.".into()),
        });
    }

    // Step 1: figure out which day to write + which article URLs need fetching.
    let db_mutex = state.db_mutex.clone();
    let year_owned = year_filter.clone();
    let plan = tokio::task::spawn_blocking(move || -> Result<Option<(String, usize, Vec<(String, String)>)>, String> {
        let _lock = db_mutex.lock().map_err(|e| format!("db lock poisoned: {}", e))?;
        let db = duckdb::Connection::open(DB_PATH).map_err(|e| format!("open db: {}", e))?;
        let next = find_next_day_to_write(&db, year_owned.as_deref())?;
        let Some((day, remaining)) = next else { return Ok(None); };

        // Never-attempted articles with a URL. Empty-string body means "tried
        // and got nothing usable" — leave those alone.
        let mut stmt = db.prepare(
            "SELECT article_id, url
             FROM news_historical
             WHERE published_utc LIKE ? || '%'
               AND url IS NOT NULL AND url <> ''
               AND body IS NULL"
        ).map_err(|e| format!("prepare fetch list: {}", e))?;
        let to_fetch: Vec<(String, String)> = stmt.query_map([day.as_str()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        }).map_err(|e| format!("query fetch list: {}", e))?
          .flatten().collect();
        Ok(Some((day, remaining, to_fetch)))
    })
    .await
    .map_err(|e| format!("task join: {}", e))??;

    let Some((day, remaining, to_fetch)) = plan else {
        return Ok(ArchiveDayResult {
            status: "done".into(),
            day: None, count: 0, path: None, remaining_days: 0,
            message: Some("All days in DB are archived.".into()),
        });
    };

    // Step 2: fetch bodies in parallel (10 concurrent).
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36")
        .build()
        .map_err(|e| format!("build http client: {}", e))?;
    // Concurrency 1 — the puppeteer proxy serialises requests internally and
    // anything above 1 returns 400 for a fraction of articles (the proxy's
    // shared page state gets confused). Sequential is reliable: ~500ms × 75
    // articles ≈ 38s/day. Slow but correct.
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
    let mut tasks = tokio::task::JoinSet::new();
    for (article_id, _url) in to_fetch {
        let permit = sem.clone().acquire_owned().await.map_err(|e| e.to_string())?;
        let client = client.clone();
        tasks.spawn(async move {
            let _permit = permit;
            let body = fetch_one_body(&client, &article_id).await;
            (article_id, body)
        });
    }
    let mut fetched: Vec<(String, String)> = Vec::new();
    let mut empty_ids: Vec<String> = Vec::new();
    let mut failed_ids: Vec<String> = Vec::new();
    let mut rate_limited = 0usize;
    let mut max_retry_after = 0u64;
    while let Some(joined) = tasks.join_next().await {
        if let Ok((aid, outcome)) = joined {
            match outcome {
                BodyFetch::Ok(b)         => fetched.push((aid, b)),
                BodyFetch::EmptyBody     => empty_ids.push(aid),
                BodyFetch::Failed        => failed_ids.push(aid),
                BodyFetch::RateLimited { retry_after_secs } => {
                    rate_limited += 1;
                    max_retry_after = max_retry_after.max(retry_after_secs);
                }
            }
        }
    }

    // If FXStreet is rate-limiting, abort the day. Don't write anything, don't
    // mark anything — these rows stay NULL so a later run after the cooldown
    // can pick them up. The Tauri command returns a clear "wait N seconds"
    // message and the React batch loop bails out.
    if rate_limited > 0 && fetched.is_empty() {
        return Ok(ArchiveDayResult {
            status: "rate_limited".into(),
            day: Some(day.clone()),
            count: 0,
            path: None,
            remaining_days: remaining,
            message: Some(format!(
                "FXStreet rate-limited us: Retry-After ~{}s. Stopping for now. \
                 Re-click in {} minutes.",
                max_retry_after,
                (max_retry_after + 59) / 60
            )),
        });
    }

    // If every fetch failed (e.g., the econcal proxy died), abort instead of
    // force-marking everything '' — a previous version of this code did that
    // and silently nuked ~33k articles when the proxy was down.
    if fetched.is_empty() && empty_ids.is_empty() && !failed_ids.is_empty() {
        return Ok(ArchiveDayResult {
            status: "stalled".into(),
            day: Some(day.clone()),
            count: 0,
            path: None,
            remaining_days: remaining,
            message: Some(format!(
                "All {} fetches failed for {} — proxy may be down. \
                 Check that econcal is listening on :6000 and retry.",
                failed_ids.len(), day
            )),
        });
    }

    // Step 3: update DB + write file.
    //   - fetched bodies → UPDATE body = '<text>'
    //   - genuinely empty (data flashes) → UPDATE body = ''  (don't retry)
    //   - HTTP/parse failures → leave NULL so a later run can retry
    let fetched_count = fetched.len();
    let empty_count   = empty_ids.len();
    let fail_count    = failed_ids.len();
    let day_clone = day.clone();
    let db_mutex = state.db_mutex.clone();
    let count = tokio::task::spawn_blocking(move || -> Result<usize, String> {
        let _lock = db_mutex.lock().map_err(|e| format!("db lock poisoned: {}", e))?;
        let db = duckdb::Connection::open(DB_PATH).map_err(|e| format!("open db: {}", e))?;
        for (aid, body) in &fetched {
            let _ = db.execute(
                "UPDATE news_historical SET body = ? WHERE article_id = ?",
                duckdb::params![body, aid],
            );
        }
        for aid in &empty_ids {
            let _ = db.execute(
                "UPDATE news_historical SET body = '' WHERE article_id = ?",
                duckdb::params![aid],
            );
        }
        let (_path, count) = write_archive_for_day(&db, &day_clone)?;
        Ok(count)
    })
    .await
    .map_err(|e| format!("task join: {}", e))??;

    let path = std::path::Path::new(ARCHIVE_ROOT).join(&day[..7]).join(format!("{}.json", day));
    Ok(ArchiveDayResult {
        status: "written".into(),
        day: Some(day),
        count,
        path: Some(path.to_string_lossy().to_string()),
        remaining_days: remaining.saturating_sub(1),
        message: Some({
            let mut parts = vec![format!("fetched: {}", fetched_count)];
            if empty_count > 0 { parts.push(format!("empty: {}", empty_count)); }
            if fail_count  > 0 { parts.push(format!("failed: {}", fail_count)); }
            if rate_limited > 0 { parts.push(format!("rate-limited: {}", rate_limited)); }
            parts.join(" | ")
        }),
    })
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
/// (M1/M5/M15/M30/H1/H4/D1). Goes through the cTrader session via the chart
/// request channel; resolves to the most recent `count` bars ending now.
/// `to_ms` (optional): fetch `count` bars ending at this unix-ms instant. If
/// absent, ends at "now". Used by the chart's lazy-load-on-pan to walk backward.
/// `force_refresh` (optional): skip the cache hit check and always go to the
/// cTrader API. Used by the Dashboard "Load Gold" button to ensure freshness.
///
/// Caching strategy:
/// 1. Check the per-(symbol, timeframe) DuckDB table for bars in [from, to].
/// 2. If the table already has data older than our `from` boundary, return the
///    cached slice — no API call.
/// 3. Otherwise hit cTrader, store the response in the cache table, return.
#[tauri::command]
async fn get_trendbars(
    state: tauri::State<'_, AppState>,
    symbol: String,
    timeframe: String,
    count: u32,
    to_ms: Option<i64>,
    force_refresh: Option<bool>,
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
    let from_sec = from_ms / 1000;
    let to_sec = end_ms / 1000;

    let table = format!("{}_{}", symbol.to_lowercase(), timeframe.to_lowercase());

    // Step 1: try the cache.
    let db_mutex = state.db_mutex.clone();
    let table_q = table.clone();
    let (cached, oldest_in_table, newest_in_cached): (Vec<CandleJson>, Option<i64>, Option<i64>) =
        tokio::task::spawn_blocking(move || -> Result<(Vec<CandleJson>, Option<i64>, Option<i64>), String> {
            let _lock = db_mutex.lock().map_err(|e| e.to_string())?;
            let db = duckdb::Connection::open(DB_PATH).map_err(|e| e.to_string())?;
            ensure_candle_table(&db, &table_q)?;
            let q = format!(
                "SELECT timestamp, open, high, low, close, volume FROM {}
                 WHERE timestamp >= ? AND timestamp < ? ORDER BY timestamp ASC",
                table_q
            );
            let mut stmt = db.prepare(&q).map_err(|e| e.to_string())?;
            let bars: Vec<CandleJson> = stmt.query_map(
                duckdb::params![from_sec, to_sec],
                |row| Ok(CandleJson {
                    time:   row.get(0)?, open:  row.get(1)?, high:   row.get(2)?,
                    low:    row.get(3)?, close: row.get(4)?, volume: row.get(5)?,
                })
            ).map_err(|e| e.to_string())?.flatten().collect();
            let oldest: Option<i64> = db.query_row(
                &format!("SELECT MIN(timestamp) FROM {}", table_q),
                [], |row| row.get::<_, Option<i64>>(0)
            ).unwrap_or(None);
            let newest_in_cached = bars.last().map(|b| b.time);
            Ok((bars, oldest, newest_in_cached))
        }).await.map_err(|e| e.to_string())??;

    // Coverage check:
    //   (a) oldest cached ts is at or before our `from` boundary
    //   (b) we have at least *some* bars in the requested window (the 70%-of-1000
    //       threshold used to fail on weekends/holidays when cTrader returns far
    //       fewer bars than `count` simply because the market was closed for half
    //       the window)
    //   (c) if the request was for the *current* window (to_ms unset → end_ms = now),
    //       also require the newest cached bar to be within ~2 buckets of now.
    //       For lazy-load (explicit older `to_ms`), skip this freshness check —
    //       the user is panning into the past and doesn't need an up-to-date tail.
    let asking_for_now = to_ms.is_none();
    let tail_tolerance = minutes_per_bar * 60 * 2;
    let tail_fresh = !asking_for_now
        || matches!(newest_in_cached, Some(n) if to_sec - n <= tail_tolerance);
    let has_coverage = matches!(oldest_in_table, Some(o) if o <= from_sec)
        && cached.len() >= 10;
    let cache_covers = has_coverage && tail_fresh;
    let force = force_refresh.unwrap_or(false);
    if cache_covers && !force {
        println!("[chart] cache hit: {} {} {} bars (range {}..{})",
                 symbol, timeframe, cached.len(), from_sec, to_sec);
        return Ok(cached);
    }
    // Tail-stale but coverage OK → serve cache instantly + refresh in the
    // background. The user sees the chart immediately; the next open or pan
    // sees fresh data. The live-tick handler in the frontend keeps the
    // in-progress bar tracking the current price.
    if has_coverage && !tail_fresh && !force {
        let stale_secs = newest_in_cached.map(|n| to_sec - n).unwrap_or(0);
        println!("[chart] cache hit (stale tail by {}s): {} {} {} bars — bg refresh",
                 symbol, timeframe, stale_secs, cached.len());
        if let Some(tx_bg) = CHART_REQ_TX.get().cloned() {
            let db_mutex_bg = state.db_mutex.clone();
            let table_bg = table.clone();
            let symbol_bg = symbol.clone();
            let tf_bg = timeframe.clone();
            tokio::spawn(async move {
                let (rt, rr) = tokio::sync::oneshot::channel();
                if tx_bg.send(ChartRequest {
                    symbol_id, period, from_ms, to_ms: end_ms, count, reply: rt,
                }).await.is_err() { return; }
                let timeout = tokio::time::timeout(std::time::Duration::from_secs(30), rr).await;
                if let Ok(Ok(Ok(fresh))) = timeout {
                    let n = fresh.len();
                    let _ = tokio::task::spawn_blocking(move || {
                        if let Ok(_lock) = db_mutex_bg.lock() {
                            if let Ok(db) = duckdb::Connection::open(DB_PATH) {
                                let _ = upsert_candles(&db, &table_bg, &fresh);
                            }
                        }
                    }).await;
                    println!("[chart] bg refresh done: {} {} {} bars", symbol_bg, tf_bg, n);
                }
            });
        }
        return Ok(cached);
    }
    if !cached.is_empty() {
        let stale_secs = newest_in_cached.map(|n| to_sec - n).unwrap_or(0);
        println!("[chart] cache partial: {} {} {} bars (tail stale by {}s) — refetching",
                 symbol, timeframe, cached.len(), stale_secs);
    }

    // Step 2: cache miss → fetch from cTrader.
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
    let fetched_count = fetched.len();

    // Step 3: persist (best-effort; don't fail the call if upsert fails).
    let db_mutex = state.db_mutex.clone();
    let table_w = table.clone();
    let candles_to_store = fetched.clone();
    tokio::task::spawn_blocking(move || {
        if let Ok(_lock) = db_mutex.lock() {
            if let Ok(db) = duckdb::Connection::open(DB_PATH) {
                let _ = upsert_candles(&db, &table_w, &candles_to_store);
            }
        }
    }).await.ok();

    println!("[chart] API fetch: {} {} {} bars stored to `{}`",
             symbol, timeframe, fetched_count, table);
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

#[derive(serde::Serialize)]
struct NewsBackfillResult {
    fetched: usize,
    inserted_estimate: usize,
    duration_ms: u128,
    error: Option<String>,
}

/// Tauri command: pull the newest ~10,000 articles from FXStreet (200 pages × 50)
/// and upsert into news_historical. Crypto is filtered out by the existing logic.
/// Used to fill gaps where the app wasn't running and missed the regular cycle.
#[tauri::command]
async fn backfill_news_from_api(state: tauri::State<'_, AppState>) -> Result<NewsBackfillResult, String> {
    let start = std::time::Instant::now();
    let db_mutex = state.db_mutex.clone();

    // Count existing rows so we can estimate how many got inserted.
    let before: usize = {
        let mutex_clone = db_mutex.clone();
        tokio::task::spawn_blocking(move || -> Result<usize, String> {
            let _lock = mutex_clone.lock().map_err(|e| e.to_string())?;
            let db = duckdb::Connection::open(DB_PATH).map_err(|e| e.to_string())?;
            db.query_row("SELECT COUNT(*) FROM news_historical", [], |row| row.get::<_, i64>(0))
                .map(|n| n as usize)
                .map_err(|e| e.to_string())
        }).await.map_err(|e| e.to_string())??
    };

    // Fetch all pages with no cutoff so we walk past our latest into the gap.
    let rows = match news_realtime::fetch_news_since(50, 200, None, None).await {
        Ok(r) => r,
        Err(e) => return Ok(NewsBackfillResult {
            fetched: 0,
            inserted_estimate: 0,
            duration_ms: start.elapsed().as_millis(),
            error: Some(e),
        }),
    };
    let fetched = rows.len();

    // Upsert into the DB.
    let mutex_clone = db_mutex.clone();
    let inserted_estimate = tokio::task::spawn_blocking(move || -> Result<usize, String> {
        let _lock = mutex_clone.lock().map_err(|e| e.to_string())?;
        let db = duckdb::Connection::open(DB_PATH).map_err(|e| e.to_string())?;
        // Surface DB errors instead of silently dropping them.
        if let Err(e) = news_realtime::write_news_to_db(&db, &rows) {
            return Err(format!("write_news_to_db: {}", e));
        }
        let after: i64 = db.query_row("SELECT COUNT(*) FROM news_historical", [], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        Ok((after as usize).saturating_sub(before))
    }).await.map_err(|e| e.to_string())??;

    Ok(NewsBackfillResult {
        fetched,
        inserted_estimate,
        duration_ms: start.elapsed().as_millis(),
        error: None,
    })
}

/// Tauri command: status without writing. Returns total days in DB and how many remain.
#[tauri::command]
async fn archive_status(state: tauri::State<'_, AppState>) -> Result<ArchiveDayResult, String> {
    archive_status_inner(state, None).await
}

#[tauri::command]
async fn archive_status_for_year(year: String, state: tauri::State<'_, AppState>) -> Result<ArchiveDayResult, String> {
    archive_status_inner(state, Some(year)).await
}

async fn archive_status_inner(
    state: tauri::State<'_, AppState>,
    year_filter: Option<String>,
) -> Result<ArchiveDayResult, String> {
    let db_mutex = state.db_mutex.clone();
    let year_owned = year_filter.clone();
    tokio::task::spawn_blocking(move || {
        let _lock = db_mutex.lock().map_err(|e| format!("db lock poisoned: {}", e))?;
        let db = duckdb::Connection::open(DB_PATH).map_err(|e| format!("open db: {}", e))?;
        let next = find_next_day_to_write(&db, year_owned.as_deref())?;
        Ok(match next {
            None => ArchiveDayResult {
                status: "done".into(),
                day: None, count: 0, path: None, remaining_days: 0,
                message: Some("All days archived.".into()),
            },
            Some((day, remaining)) => ArchiveDayResult {
                status: "pending".into(),
                day: Some(day),
                count: 0,
                path: None,
                remaining_days: remaining,
                message: None,
            },
        })
    })
    .await
    .map_err(|e| format!("task join: {}", e))?
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

    let result = std::process::Command::new(cmd)
        .args(["run", "dev"])
        .current_dir(frontend_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();

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
                        _ => continue,
                    };
                    let serialized = json.to_string();
                    // Cache the latest per-type snapshot so new WS clients can replay.
                    if let Some(t) = json.get("type").and_then(|v| v.as_str()) {
                        if let Ok(mut cache) = snapshot_cache().lock() {
                            cache.insert(t.to_string(), serialized.clone());
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

            // Run price streaming with reconnection
            // request_rx passed by &mut so pending requests survive reconnects
            let mut request_rx = data_req_rx;
            let mut chart_req_rx_holder = chart_req_rx;
            let mut backoff_seconds = 1;
            loop {
                println!("Starting cTrader price stream...");
                match run_session(tx.clone(), &mut request_rx, data_resp_tx.clone(), &mut chart_req_rx_holder, db_for_async.clone()).await {
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
            write_next_archive_day,
            write_next_archive_day_for_year,
            archive_status,
            archive_status_for_year,
            backfill_news_from_api,
            fetch_article_body_on_demand,
            get_trendbars,
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
    shared_db: SharedDb,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Local map: client_msg_id → oneshot reply for in-flight chart requests.
    let mut pending_chart_reqs: std::collections::HashMap<
        String,
        tokio::sync::oneshot::Sender<Result<Vec<Candle>, String>>,
    > = std::collections::HashMap::new();
    let mut chart_msg_id_counter: u64 = 0;
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

    // Symbols to subscribe to live spot prices (driven by CTRADER_SYMBOL env var)
    let instruments_to_subscribe: Vec<&str> = vec![target_symbol.as_str()];
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
    // EC + News default to always-on since the React dashboard surfaces them passively.
    let mut ec_capturing = true;

    // ── News capture state ───────────────────────────────────────────────
    let mut news_capturing = true;
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
                        let total = rows.len();

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

                        let now_str = chrono::Local::now().format("%H:%M:%S").to_string();
                        let _ = tx.send(PriceUpdate::NewsStatus(format!(
                            "{} articles | next: 5m | updated: {}",
                            total, now_str
                        ))).await;
                        let _ = tx.send(PriceUpdate::NewsTodayArticles(today_rows)).await;
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
