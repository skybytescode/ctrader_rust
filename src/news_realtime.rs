//! Real-time News fetcher.
//!
//! Fetches news articles from FXStreet via the econcal proxy (localhost:6000)
//! and writes them to DuckDB tables:
//!   - `news_today`       — today's articles only (cleared daily, fast UI reads)
//!   - `news_historical`  — all articles (append-only, for ML training)
//!
//! Excludes crypto-tagged articles.

use chrono::{Datelike, NaiveDateTime, Timelike, Utc};
use serde::Deserialize;

// ── econcal proxy URL ────────────────────────────────────────────────────────

const PROXY_BASE: &str = "http://localhost:6000";
const NEWS_HOST: &str = "https://subscriptions.fxstreet.com";


/// Crypto-related tag names to filter out (case-insensitive substring match)
const CRYPTO_FILTER: &[&str] = &[
    "bitcoin", "btc", "ethereum", "eth", "crypto", "ripple", "xrp",
    "dogecoin", "doge", "solana", "sol", "cardano", "ada", "bnb",
    "litecoin", "ltc", "shiba", "altcoin", "defi", "nft", "blockchain",
];

// ── JSON deserialization structs (match FXStreet Post API) ───────────────────

/// Wrapper for the FXStreet API response: `{ "Values": [...] }`
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct NewsResponse {
    #[serde(default)]
    pub values: Vec<NewsArticle>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct NewsArticle {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub publication_date: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default, rename = "Url")]
    pub url: Option<String>,
    #[serde(default)]
    pub author: Option<NewsAuthor>,
    #[serde(default)]
    pub tags: Option<Vec<NewsTag>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct NewsAuthor {
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct NewsTag {
    #[serde(default)]
    pub name: Option<String>,
}

/// Parsed news row ready for DB insertion.
#[derive(Debug, Clone)]
pub struct NewsRow {
    pub article_id: String,
    pub title: String,
    pub published_utc: String,
    pub summary: String,
    pub url: String,
    pub author: String,
    pub tags: String, // comma-separated tag names
    pub hour_utc: i8,
    pub weekday: i8,
}

// ── Fetch & parse ────────────────────────────────────────────────────────────

/// Fetch news articles from FXStreet general feed via econcal proxy.
/// Fetches all articles (no tag filter), excludes crypto client-side.
/// `pages` controls how many pages to fetch (page 0 = newest).
/// `progress_tx` sends real-time status messages to the UI (optional).
/// Get the latest article timestamp from `news_historical`.
/// Returns None if the table doesn't exist or is empty.
pub fn get_latest_news_timestamp(db: &duckdb::Connection) -> Option<String> {
    db.query_row(
        "SELECT MAX(published_utc) FROM news_historical",
        [],
        |row| row.get::<_, Option<String>>(0),
    ).ok().flatten()
}

pub async fn fetch_news(
    take_per_page: u32,
    pages: u32,
    progress_tx: Option<&std::sync::mpsc::Sender<String>>,
) -> Result<Vec<NewsRow>, String> {
    fetch_news_since(take_per_page, pages, None, progress_tx).await
}

/// Fetch news, stopping when articles older than `since` are encountered.
/// If `since` is None, fetches all available pages.
pub async fn fetch_news_since(
    take_per_page: u32,
    pages: u32,
    since: Option<&str>,
    progress_tx: Option<&std::sync::mpsc::Sender<String>>,
) -> Result<Vec<NewsRow>, String> {
    let client = reqwest::Client::new();
    let mut all_rows: Vec<NewsRow> = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();
    let mut crypto_filtered = 0usize;

    let send = |msg: String| {
        if let Some(tx) = progress_tx {
            let _ = tx.send(msg);
        }
    };

    if let Some(cutoff) = since {
        send(format!("Fetching news newer than {} ...", &cutoff[..cutoff.len().min(19)]));
    } else {
        send(format!("Fetching all news, {} pages of {} ...", pages, take_per_page));
    }

    for page in 0..pages {
        send(format!("Page {}/{} ... ({} articles, {} crypto filtered)",
            page + 1, pages, all_rows.len(), crypto_filtered));

        let url = format!(
            "{}/v4/en/post/filter/GeneralFeed/{}/{}/PlainText",
            PROXY_BASE, page, take_per_page
        );

        let resp = match client
            .get(&url)
            .header("x-api-host", NEWS_HOST)
            .timeout(std::time::Duration::from_secs(20))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                let msg = format!("Page {} fetch failed: {}", page + 1, e);
                println!("News: {}", msg);
                send(msg);
                break;
            }
        };

        let text = match resp.text().await {
            Ok(t) => t,
            Err(e) => {
                let msg = format!("Page {} read error: {}", page + 1, e);
                println!("News: {}", msg);
                send(msg);
                break;
            }
        };

        let trimmed = text.trim();
        if trimmed.is_empty() || trimmed == "{}" {
            send(format!("Page {} empty — reached end of feed", page + 1));
            break;
        }

        // Response is {"Values": [...]}
        let articles = match serde_json::from_str::<NewsResponse>(trimmed) {
            Ok(resp) => resp.values,
            Err(_) => {
                let json_text = if let Some(pos) = trimmed.rfind(']') {
                    &trimmed[..=pos]
                } else {
                    trimmed
                };
                match serde_json::from_str::<Vec<NewsArticle>>(json_text) {
                    Ok(a) => a,
                    Err(e) => {
                        let msg = format!("Page {} parse error: {}", page + 1, e);
                        println!("News: {} (first 200 chars: {})", msg, &trimmed[..trimmed.len().min(200)]);
                        send(msg);
                        break;
                    }
                }
            }
        };

        let got = articles.len();
        let mut hit_cutoff = false;
        for a in articles {
            if is_crypto(&a) {
                crypto_filtered += 1;
                continue;
            }
            if let Some(row) = parse_article_no_crypto_check(a) {
                // Articles are newest-first; stop when we reach already-stored data
                if let Some(cutoff) = since {
                    if row.published_utc.as_str() <= cutoff {
                        hit_cutoff = true;
                        break;
                    }
                }
                if seen_ids.insert(row.article_id.clone()) {
                    all_rows.push(row);
                }
            }
        }

        if hit_cutoff {
            send(format!("Page {} — reached existing data, stopping", page + 1));
            break;
        }

        // If we got fewer than requested, no more pages
        if (got as u32) < take_per_page {
            send(format!("Page {} had {} articles (< {}), reached end",
                page + 1, got, take_per_page));
            break;
        }
    }

    if all_rows.is_empty() {
        return Err("No articles fetched".to_string());
    }

    // Sort by publication date descending (newest first)
    all_rows.sort_by(|a, b| b.published_utc.cmp(&a.published_utc));

    send(format!("Done: {} articles ({} crypto filtered out)", all_rows.len(), crypto_filtered));
    Ok(all_rows)
}

fn is_crypto(article: &NewsArticle) -> bool {
    // Check title
    if let Some(ref title) = article.title {
        let lower = title.to_lowercase();
        if CRYPTO_FILTER.iter().any(|kw| lower.contains(kw)) {
            return true;
        }
    }
    // Check tags
    if let Some(ref tags) = article.tags {
        for tag in tags {
            if let Some(ref name) = tag.name {
                let lower = name.to_lowercase();
                if CRYPTO_FILTER.iter().any(|kw| lower.contains(kw)) {
                    return true;
                }
            }
        }
    }
    false
}

fn parse_article_no_crypto_check(a: NewsArticle) -> Option<NewsRow> {
    let id = a.id.as_deref().unwrap_or("").to_string();
    if id.is_empty() {
        return None;
    }

    let title = a.title.as_deref().unwrap_or("").to_string();
    let pub_date = a.publication_date.as_deref().unwrap_or("").to_string();
    if pub_date.is_empty() {
        return None;
    }

    let summary = a.summary.as_deref().unwrap_or("").to_string();
    let url = a.url.as_deref().unwrap_or("").to_string();
    let author = a.author.as_ref()
        .and_then(|a| a.name.as_deref())
        .unwrap_or("").to_string();

    let tags_str = a.tags.as_ref()
        .map(|tags| tags.iter()
            .filter_map(|t| t.name.as_deref())
            .collect::<Vec<_>>()
            .join(","))
        .unwrap_or_default();

    // Parse timestamp
    let ts_str = pub_date.replace("Z", "");
    let ts = NaiveDateTime::parse_from_str(&ts_str, "%Y-%m-%dT%H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(&ts_str, "%Y-%m-%dT%H:%M:%S%.f"))
        .ok()?;

    let weekday = ts.weekday().num_days_from_monday() as i8;
    let hour = ts.hour() as i8;

    Some(NewsRow {
        article_id: id,
        title,
        published_utc: ts.format("%Y-%m-%dT%H:%M:%S").to_string(),
        summary,
        url,
        author,
        tags: tags_str,
        hour_utc: hour,
        weekday,
    })
}

// ── DuckDB write ─────────────────────────────────────────────────────────────

const CREATE_NEWS_TODAY: &str = "
CREATE TABLE IF NOT EXISTS news_today (
    article_id      VARCHAR PRIMARY KEY,
    title           VARCHAR NOT NULL,
    published_utc   VARCHAR NOT NULL,
    summary         VARCHAR,
    url             VARCHAR,
    author          VARCHAR,
    tags            VARCHAR,
    hour_utc        TINYINT NOT NULL,
    weekday         TINYINT NOT NULL
)
";

const CREATE_NEWS_HISTORICAL: &str = "
CREATE TABLE IF NOT EXISTS news_historical (
    article_id      VARCHAR PRIMARY KEY,
    title           VARCHAR NOT NULL,
    published_utc   VARCHAR NOT NULL,
    summary         VARCHAR,
    url             VARCHAR,
    author          VARCHAR,
    tags            VARCHAR,
    hour_utc        TINYINT NOT NULL,
    weekday         TINYINT NOT NULL
)
";

/// Write news to `news_today` (replace all) and upsert into `news_historical`.
pub fn write_news_to_db(db: &duckdb::Connection, rows: &[NewsRow]) -> Result<usize, String> {
    // Drop and recreate today table
    let _ = db.execute("DROP TABLE IF EXISTS news_today", []);
    db.execute_batch(CREATE_NEWS_TODAY).map_err(|e| format!("create news_today: {}", e))?;

    // Ensure historical table exists
    db.execute_batch(CREATE_NEWS_HISTORICAL).map_err(|e| format!("create news_historical: {}", e))?;

    let insert_today = "INSERT INTO news_today VALUES (?,?,?,?,?,?,?,?,?)";
    let mut inserted = 0usize;

    // Filter to today's articles only for the today table
    let today_str = Utc::now().format("%Y-%m-%d").to_string();

    for r in rows {
        let is_today = r.published_utc.starts_with(&today_str);

        if is_today {
            let params = duckdb::params![
                r.article_id,
                r.title,
                r.published_utc,
                r.summary,
                r.url,
                r.author,
                r.tags,
                r.hour_utc as i32,
                r.weekday as i32,
            ];
            match db.execute(insert_today, params) {
                Ok(_) => inserted += 1,
                Err(e) => {
                    if inserted == 0 {
                        println!("News: insert error (first row): {}", e);
                    }
                }
            }
        }

        // Upsert into historical (all articles)
        let upsert = "
            INSERT INTO news_historical VALUES (?,?,?,?,?,?,?,?,?)
            ON CONFLICT (article_id) DO UPDATE SET
                title         = EXCLUDED.title,
                summary       = EXCLUDED.summary,
                url           = EXCLUDED.url,
                author        = EXCLUDED.author,
                tags          = EXCLUDED.tags
        ";
        let params = duckdb::params![
            r.article_id,
            r.title,
            r.published_utc,
            r.summary,
            r.url,
            r.author,
            r.tags,
            r.hour_utc as i32,
            r.weekday as i32,
        ];
        let _ = db.execute(upsert, params);
    }

    println!("News: {} today, {} total fetched", inserted, rows.len());
    Ok(inserted)
}

/// Read today's news for UI display.
pub fn read_news_today(db: &duckdb::Connection) -> Vec<NewsRow> {
    let query = "
        SELECT article_id, title, published_utc, summary, url, author, tags, hour_utc, weekday
        FROM news_today
        ORDER BY published_utc DESC
    ";
    let mut stmt = match db.prepare(query) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map([], |row| {
        Ok(NewsRow {
            article_id: row.get::<_, String>(0)?,
            title: row.get::<_, String>(1)?,
            published_utc: row.get::<_, String>(2)?,
            summary: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
            url: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
            author: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
            tags: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
            hour_utc: row.get::<_, i32>(7)? as i8,
            weekday: row.get::<_, i32>(8)? as i8,
        })
    });
    match rows {
        Ok(r) => r.flatten().collect(),
        Err(_) => Vec::new(),
    }
}

/// Format news rows into display lines for the UI.
pub fn format_news_lines(rows: &[NewsRow]) -> Vec<String> {
    if rows.is_empty() {
        return vec!["No news articles for today".to_string()];
    }

    let now_utc = Utc::now().naive_utc();
    let mut lines = Vec::new();
    lines.push(format!("--- TODAY ({} articles) ---", rows.len()));

    for r in rows {
        // Convert UTC to local time for display
        let time_str = NaiveDateTime::parse_from_str(&r.published_utc, "%Y-%m-%dT%H:%M:%S")
            .ok()
            .and_then(|ndt| {
                Some(ndt.and_utc().with_timezone(&chrono::Local).format("%H:%M").to_string())
            })
            .unwrap_or_else(|| {
                if r.published_utc.len() >= 16 {
                    r.published_utc[11..16].to_string()
                } else {
                    r.published_utc.clone()
                }
            });

        // Time ago
        let ago = NaiveDateTime::parse_from_str(&r.published_utc, "%Y-%m-%dT%H:%M:%S")
            .ok()
            .map(|ndt| {
                let diff = now_utc - ndt;
                let mins = diff.num_minutes();
                if mins < 1 { "just now".to_string() }
                else if mins < 60 { format!("{}m ago", mins) }
                else { format!("{}h{}m ago", mins / 60, mins % 60) }
            })
            .unwrap_or_default();

        // Truncate title (char-aware so it doesn't panic on multibyte chars like '–')
        let short_title = if r.title.chars().count() > 60 {
            format!("{}...", r.title.chars().take(57).collect::<String>())
        } else {
            r.title.clone()
        };

        lines.push(format!("  {} ({})  {}", time_str, ago, short_title));
    }

    lines
}
