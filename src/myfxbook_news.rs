//! MyFXBook news fetcher + store.
//!
//! Scrapes three server-rendered MyFXBook listing pages — /news, /analysis and
//! /press-release — with curl (Cloudflare blocks reqwest) + `scraper`. Mirrors
//! the calendar/news archive pattern:
//!   - `myfxbook_news_today`       — today's items (UI, rebuilt each fetch)
//!   - `myfxbook_news_historical`  — append/upsert archive (per-day JSON source)

use chrono::{DateTime, Datelike, Timelike, Utc};

/// The three MyFXBook listing categories we fetch.
pub const CATEGORIES: [&str; 3] = ["news", "analysis", "press-release"];

/// One MyFXBook news/analysis/press-release item.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MfbNewsItem {
    pub article_id: String,    // trailing numeric id from the URL — PK
    pub category: String,      // "news" | "analysis" | "press-release"
    pub title: String,
    pub url: String,
    pub summary: String,
    pub source: String,        // e.g. "RTTNews"
    pub published_utc: String, // "YYYY-MM-DDTHH:MM:SS" (derived from relative time)
    pub image_url: String,
    pub hour_utc: i8,
    pub weekday: i8,
}

/// Convert a MyFXBook relative time string ("28 minutes ago", "2 hours ago",
/// "3 days ago", "just now", "yesterday", or an absolute "Jun 9, 2026") into an
/// absolute UTC timestamp, relative to `now`.
fn relative_to_utc(s: &str, now: DateTime<Utc>) -> DateTime<Utc> {
    let t = s.trim().to_lowercase();
    if t.is_empty() || t.contains("just now") || t.contains("moment") {
        return now;
    }
    if t.contains("yesterday") { return now - chrono::Duration::days(1); }

    // "<n> <unit> ago"
    let mut it = t.split_whitespace();
    if let (Some(num), Some(unit)) = (it.next(), it.next()) {
        if let Ok(n) = num.parse::<i64>() {
            if unit.starts_with("sec")  { return now - chrono::Duration::seconds(n); }
            if unit.starts_with("min")  { return now - chrono::Duration::minutes(n); }
            if unit.starts_with("hour") { return now - chrono::Duration::hours(n); }
            if unit.starts_with("day")  { return now - chrono::Duration::days(n); }
            if unit.starts_with("week") { return now - chrono::Duration::weeks(n); }
            if unit.starts_with("month"){ return now - chrono::Duration::days(30 * n); }
        }
    }
    // Absolute "Mon D, YYYY" / "Mon D" fallback.
    for fmt in ["%b %d, %Y", "%b %d %Y", "%B %d, %Y"] {
        if let Ok(d) = chrono::NaiveDate::parse_from_str(s.trim(), fmt) {
            if let Some(dt) = d.and_hms_opt(0, 0, 0) {
                return DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc);
            }
        }
    }
    now // unparseable → treat as now so it still shows
}

/// Fetch and parse all three categories, concatenated. Async fetch per page,
/// then synchronous parse (scraper's Html is !Send).
pub async fn fetch_all() -> Result<Vec<MfbNewsItem>, String> {
    let now = Utc::now();
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for cat in CATEGORIES {
        let url = format!("https://www.myfxbook.com/{}", cat);
        match crate::myfxbook_cal::fetch_html_via_curl(&url).await {
            Ok(html) => out.extend(parse_news(&html, cat, now)),
            Err(e) => errors.push(format!("{}: {}", cat, e)),
        }
    }
    if out.is_empty() && !errors.is_empty() {
        return Err(errors.join("; "));
    }
    Ok(out)
}

/// Parse one MyFXBook listing page into items. `category` labels the source page;
/// `now` anchors the relative-time conversion (pass through for deterministic tests).
pub fn parse_news(html: &str, category: &str, now: DateTime<Utc>) -> Vec<MfbNewsItem> {
    use scraper::{Html, Selector};
    let doc = Html::parse_document(html);
    let row_sel = Selector::parse("div.news-row-text").unwrap();
    let h2a_sel = Selector::parse("h2 a").unwrap();
    let desc_sel = Selector::parse(".news-description, .news-summary").unwrap();
    let details_sel = Selector::parse(".news-details").unwrap();
    let img_sel = Selector::parse("img").unwrap();
    let txt = |el: scraper::ElementRef| el.text().collect::<String>().split_whitespace()
        .collect::<Vec<_>>().join(" ").trim().to_string();

    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for row in doc.select(&row_sel) {
        let Some(link) = row.select(&h2a_sel).next() else { continue };
        let href = link.value().attr("href").unwrap_or("");
        // id = trailing path segment ("/news/<slug>/<id>").
        let article_id = href.rsplit('/').next().unwrap_or("").to_string();
        if article_id.is_empty() || !seen.insert(article_id.clone()) { continue; }
        let title = txt(link);
        if title.is_empty() { continue; }

        let url = if href.starts_with("http") {
            href.to_string()
        } else {
            format!("https://www.myfxbook.com{}", href)
        };
        let summary = row.select(&desc_sel).next().map(txt).unwrap_or_default();
        let image_url = row.select(&img_sel).next()
            .and_then(|e| e.value().attr("src")).unwrap_or("").to_string();

        // ".news-details" = "Source | 28 minutes ago"
        let details = row.select(&details_sel).next().map(txt).unwrap_or_default();
        let (source, when) = match details.split_once('|') {
            Some((s, w)) => (s.trim().to_string(), w.trim().to_string()),
            None => (String::new(), details.clone()),
        };
        let dt = relative_to_utc(&when, now);

        out.push(MfbNewsItem {
            article_id,
            category: category.to_string(),
            title,
            url,
            summary,
            source,
            published_utc: dt.format("%Y-%m-%dT%H:%M:%S").to_string(),
            image_url,
            hour_utc: dt.hour() as i8,
            weekday: dt.weekday().num_days_from_sunday() as i8,
        });
    }
    out
}

// ── DuckDB ────────────────────────────────────────────────────────────────────

const CREATE_NEWS: &str = "
CREATE TABLE IF NOT EXISTS myfxbook_news_historical (
    article_id    VARCHAR PRIMARY KEY,
    category      VARCHAR NOT NULL,
    title         VARCHAR NOT NULL,
    url           VARCHAR,
    summary       VARCHAR,
    source        VARCHAR,
    published_utc VARCHAR NOT NULL,
    image_url     VARCHAR,
    hour_utc      TINYINT,
    weekday       TINYINT
)";

const CREATE_NEWS_TODAY: &str = "
CREATE TABLE IF NOT EXISTS myfxbook_news_today (
    article_id    VARCHAR PRIMARY KEY,
    category      VARCHAR NOT NULL,
    title         VARCHAR NOT NULL,
    url           VARCHAR,
    summary       VARCHAR,
    source        VARCHAR,
    published_utc VARCHAR NOT NULL,
    image_url     VARCHAR,
    hour_utc      TINYINT,
    weekday       TINYINT
)";

pub fn create_table(db: &duckdb::Connection) -> Result<(), String> {
    db.execute_batch(CREATE_NEWS).map_err(|e| format!("create myfxbook_news_historical: {}", e))
}

/// Upsert items into the archive. On conflict, refresh the mutable fields (title,
/// summary, source, published_utc can shift as the relative time re-anchors).
pub fn upsert(db: &duckdb::Connection, items: &[MfbNewsItem]) -> Result<usize, String> {
    create_table(db)?;
    let sql = "
        INSERT INTO myfxbook_news_historical
            (article_id, category, title, url, summary, source, published_utc, image_url, hour_utc, weekday)
        VALUES (?,?,?,?,?,?,?,?,?,?)
        ON CONFLICT (article_id) DO UPDATE SET
            title = EXCLUDED.title, summary = EXCLUDED.summary, source = EXCLUDED.source,
            url = EXCLUDED.url, image_url = EXCLUDED.image_url
    ";
    let mut n = 0usize;
    for it in items {
        let params = duckdb::params![
            it.article_id, it.category, it.title, it.url, it.summary, it.source,
            it.published_utc, it.image_url, it.hour_utc as i32, it.weekday as i32,
        ];
        if db.execute(sql, params).is_ok() { n += 1; }
    }
    Ok(n)
}

/// Rebuild `myfxbook_news_today` from the archive (today's UTC items).
pub fn write_today_from_archive(db: &duckdb::Connection) -> Result<usize, String> {
    create_table(db)?;
    db.execute_batch(CREATE_NEWS_TODAY).map_err(|e| format!("create myfxbook_news_today: {}", e))?;
    let _ = db.execute("DELETE FROM myfxbook_news_today", []);
    let today = Utc::now().format("%Y-%m-%d").to_string();
    db.execute(
        "INSERT INTO myfxbook_news_today
         SELECT * FROM myfxbook_news_historical WHERE published_utc LIKE ? || '%'",
        duckdb::params![today],
    ).map_err(|e| format!("rebuild myfxbook_news_today: {}", e))
}

/// Today's items for the UI, newest first: (id, category, title, url, summary, source, published_utc).
pub type MfbNewsRow = (String, String, String, String, String, String, String);

pub fn read_today(db: &duckdb::Connection) -> Vec<MfbNewsRow> {
    let q = "SELECT article_id, category, title, url, summary, source, published_utc
             FROM myfxbook_news_today ORDER BY published_utc DESC";
    let Ok(mut stmt) = db.prepare(q) else { return Vec::new() };
    let rows = stmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?,
            r.get::<_, Option<String>>(3)?.unwrap_or_default(),
            r.get::<_, Option<String>>(4)?.unwrap_or_default(),
            r.get::<_, Option<String>>(5)?.unwrap_or_default(),
            r.get::<_, String>(6)?))
    });
    match rows { Ok(it) => it.flatten().collect(), Err(_) => Vec::new() }
}

/// Write one per-day JSON archive file `<root>/YYYY-MM/YYYY-MM-DD.json`.
pub fn write_archive_day(db: &duckdb::Connection, root: &str, day: &str) -> Result<(String, usize), String> {
    if day.len() < 7 { return Err("bad day".into()); }
    let mut stmt = db.prepare(
        "SELECT article_id, category, title, url, summary, source, published_utc, image_url, hour_utc, weekday
         FROM myfxbook_news_historical WHERE published_utc LIKE ? || '%' ORDER BY published_utc DESC"
    ).map_err(|e| format!("prepare archive query: {}", e))?;
    let rows = stmt.query_map([day], |r| {
        Ok(serde_json::json!({
            "article_id": r.get::<_, String>(0)?, "category": r.get::<_, String>(1)?,
            "title": r.get::<_, String>(2)?, "url": r.get::<_, Option<String>>(3)?,
            "summary": r.get::<_, Option<String>>(4)?, "source": r.get::<_, Option<String>>(5)?,
            "published_utc": r.get::<_, String>(6)?, "image_url": r.get::<_, Option<String>>(7)?,
            "hour_utc": r.get::<_, Option<i32>>(8)?, "weekday": r.get::<_, Option<i32>>(9)?,
        }))
    }).map_err(|e| format!("query archive: {}", e))?;
    let items: Vec<serde_json::Value> = rows.flatten().collect();
    let count = items.len();
    let dir = std::path::Path::new(root).join(&day[..7]);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create dir {}: {}", dir.display(), e))?;
    let path = dir.join(format!("{}.json", day));
    let payload = serde_json::json!({
        "date": day, "count": count, "generated_at": Utc::now().to_rfc3339(), "items": items,
    });
    let pretty = serde_json::to_string_pretty(&payload).map_err(|e| format!("serialize: {}", e))?;
    std::fs::write(&path, pretty).map_err(|e| format!("write {}: {}", path.display(), e))?;
    Ok((path.to_string_lossy().to_string(), count))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_time_parses() {
        let now = DateTime::<Utc>::from_timestamp(1_780_000_000, 0).unwrap();
        assert_eq!(relative_to_utc("28 minutes ago", now), now - chrono::Duration::minutes(28));
        assert_eq!(relative_to_utc("2 hours ago", now), now - chrono::Duration::hours(2));
        assert_eq!(relative_to_utc("3 days ago", now), now - chrono::Duration::days(3));
        assert_eq!(relative_to_utc("just now", now), now);
        assert_eq!(relative_to_utc("yesterday", now), now - chrono::Duration::days(1));
    }

    #[test]
    fn parse_news_extracts_item() {
        let now = DateTime::<Utc>::from_timestamp(1_780_000_000, 0).unwrap();
        let html = r#"
        <article><div class="row news-row-text">
          <a href="/news/indian-shares-rebound/50277"><img src="https://x/desktop.webp"/></a>
          <div class="news-info">
            <h2><a href="/news/indian-shares-rebound/50277">Indian Shares Rebound</a></h2>
            <div class="news-description">Indian shares rebounded on Tuesday.</div>
            <div class="news-details">RTTNews <span class="news-separator">|</span> 28 minutes ago</div>
          </div>
        </div></article>"#;
        let items = parse_news(html, "news", now);
        assert_eq!(items.len(), 1);
        let it = &items[0];
        assert_eq!(it.article_id, "50277");
        assert_eq!(it.category, "news");
        assert_eq!(it.title, "Indian Shares Rebound");
        assert_eq!(it.source, "RTTNews");
        assert_eq!(it.url, "https://www.myfxbook.com/news/indian-shares-rebound/50277");
        assert!(it.summary.starts_with("Indian shares"));
    }
}
