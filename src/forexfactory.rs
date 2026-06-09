//! ForexFactory fetcher + store — calendar and news.
//!
//! Both pages are reachable with curl (Cloudflare-safe, via the shared helper).
//!   - Calendar: the events are embedded in a `window.calendarComponentStates[N]`
//!     JSON object (days[] → events[]) — we extract and parse that JSON.
//!   - News: server-rendered `.news-block` HTML — parsed with `scraper`.
//!
//! Stored per-day like the EC / MyFXBook archives:
//!   forexfactory_calendar_data/all/YYYY-MM/YYYY-MM-DD.json
//!   forexfactory_news_data/all/YYYY-MM/YYYY-MM-DD.json

use chrono::{DateTime, Datelike, Timelike, Utc};

const CAL_URL: &str = "https://www.forexfactory.com/calendar";
const NEWS_URL: &str = "https://www.forexfactory.com/news";

// ── Calendar ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize)]
pub struct FfCalEvent {
    pub event_id: String,      // FF event id — PK
    pub title: String,
    pub country: String,
    pub currency: String,
    pub impact: i8,            // 0 holiday/none · 1 low · 2 medium · 3 high
    pub impact_label: String,  // "high" | "medium" | "low" | "holiday"
    pub timestamp_utc: String, // "YYYY-MM-DDTHH:MM:SS" (UTC, from dateline)
    pub time_label: String,    // e.g. "All Day", "8:30am"
    pub weekday: i8,
    pub hour_utc: i8,
    pub actual_raw: Option<String>,
    pub forecast_raw: Option<String>,
    pub previous_raw: Option<String>,
}

fn impact_to_num(name: &str) -> i8 {
    match name.to_lowercase().as_str() {
        "high" => 3,
        "medium" => 2,
        "low" => 1,
        _ => 0, // holiday / none / grey
    }
}

/// A calendar value field may be a plain string or `{ "value": "...", ... }`.
fn val_str(v: &serde_json::Value) -> Option<String> {
    let s = match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(o) => o.get("value").or_else(|| o.get("text"))
            .and_then(|x| x.as_str()).map(String::from).unwrap_or_default(),
        serde_json::Value::Number(n) => n.to_string(),
        _ => String::new(),
    };
    let t = s.trim().to_string();
    if t.is_empty() || t == "&nbsp;" { None } else { Some(t) }
}

/// Extract every `days: [ ... ]` array from the
/// `window.calendarComponentStates[N] = { days: [...] }` assignments.
///
/// The wrapping object is a JS literal (the `days` key is UNQUOTED), so it isn't
/// valid JSON — but the `[...]` array value itself is (all inner keys quoted), so
/// we bracket-match just the array and parse that.
fn extract_days_arrays(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let needle = "calendarComponentStates[";
    let bytes = html.as_bytes();
    let mut search = 0usize;
    while let Some(rel) = html[search..].find(needle) {
        let i = search + rel;
        let after = &html[i..];
        // Locate `days:` within this assignment, then its opening '['.
        let Some(d) = after.find("days:") else { search = i + needle.len(); continue };
        let Some(br) = after[d..].find('[') else { search = i + needle.len(); continue };
        let start = i + d + br;
        // bracket-match (skipping brackets inside strings).
        let mut depth = 0i32; let mut in_str = false; let mut esc = false;
        let mut end = None;
        for j in start..bytes.len() {
            let c = bytes[j];
            if in_str {
                if esc { esc = false; }
                else if c == b'\\' { esc = true; }
                else if c == b'"' { in_str = false; }
            } else {
                match c {
                    b'"' => in_str = true,
                    b'[' => depth += 1,
                    b']' => { depth -= 1; if depth == 0 { end = Some(j); break; } }
                    _ => {}
                }
            }
        }
        match end {
            Some(e) => { out.push(html[start..=e].to_string()); search = e + 1; }
            None => break,
        }
    }
    out
}

/// Parse the ForexFactory calendar page into events (dedup by id across states).
pub fn parse_calendar(html: &str) -> Vec<FfCalEvent> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for arr in extract_days_arrays(html) {
        let Ok(days) = serde_json::from_str::<Vec<serde_json::Value>>(&arr) else { continue };
        for day in &days {
            let Some(events) = day.get("events").and_then(|e| e.as_array()) else { continue };
            for ev in events {
                let id = ev.get("id").map(|x| match x {
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::String(s) => s.clone(),
                    _ => String::new(),
                }).unwrap_or_default();
                if id.is_empty() || !seen.insert(id.clone()) { continue; }
                let dateline = ev.get("dateline").and_then(|d| d.as_i64());
                let Some(dt) = dateline.and_then(|s| DateTime::<Utc>::from_timestamp(s, 0)) else { continue };
                let title = ev.get("name").and_then(|n| n.as_str()).unwrap_or("").trim().to_string();
                if title.is_empty() { continue; }
                let impact_label = ev.get("impactName").and_then(|n| n.as_str()).unwrap_or("").to_string();
                out.push(FfCalEvent {
                    event_id: id,
                    title,
                    country: ev.get("country").and_then(|n| n.as_str()).unwrap_or("").to_string(),
                    currency: ev.get("currency").and_then(|n| n.as_str()).unwrap_or("").to_string(),
                    impact: impact_to_num(&impact_label),
                    impact_label,
                    timestamp_utc: dt.format("%Y-%m-%dT%H:%M:%S").to_string(),
                    time_label: ev.get("timeLabel").and_then(|n| n.as_str()).unwrap_or("").to_string(),
                    weekday: dt.weekday().num_days_from_sunday() as i8,
                    hour_utc: dt.hour() as i8,
                    actual_raw:   ev.get("actual").and_then(val_str),
                    forecast_raw: ev.get("forecast").and_then(val_str),
                    previous_raw: ev.get("previous").and_then(val_str),
                });
            }
        }
    }
    out
}

pub async fn fetch_calendar() -> Result<Vec<FfCalEvent>, String> {
    let html = crate::myfxbook_cal::fetch_html_via_curl(CAL_URL).await?;
    let evs = parse_calendar(&html);
    if evs.is_empty() { return Err("no events parsed (page layout changed?)".into()); }
    Ok(evs)
}

// ── News ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize)]
pub struct FfNewsItem {
    pub article_id: String,    // FF news id — PK
    pub title: String,
    pub url: String,
    pub source: String,        // e.g. "advisorperspectives.com"
    pub preview: String,
    pub published_utc: String, // derived from relative time
    pub image_url: String,
    pub hour_utc: i8,
    pub weekday: i8,
}

fn relative_to_utc(s: &str, now: DateTime<Utc>) -> DateTime<Utc> {
    let t = s.trim().to_lowercase();
    if t.is_empty() || t.contains("now") || t.contains("moment") { return now; }
    let mut it = t.split_whitespace();
    if let (Some(num), Some(unit)) = (it.next(), it.next()) {
        if let Ok(n) = num.parse::<i64>() {
            if unit.starts_with("sec")  { return now - chrono::Duration::seconds(n); }
            if unit.starts_with("min")  { return now - chrono::Duration::minutes(n); }
            if unit.starts_with("hour") || unit.starts_with("hr") { return now - chrono::Duration::hours(n); }
            if unit.starts_with("day")  { return now - chrono::Duration::days(n); }
            if unit.starts_with("week") { return now - chrono::Duration::weeks(n); }
            if unit.starts_with("month"){ return now - chrono::Duration::days(30 * n); }
        }
    }
    now
}

pub fn parse_news(html: &str, now: DateTime<Utc>) -> Vec<FfNewsItem> {
    use scraper::{Html, Selector};
    let doc = Html::parse_document(html);
    // The /news page has a small featured set (.news-block) plus the full list
    // (.news-block__item) — match both; dedup by id handles any overlap.
    let block_sel = Selector::parse("div.news-block__item, div.news-block").unwrap();
    let title_sel = Selector::parse(".news-block__title a, a.news-block__title").unwrap();
    let any_title_a = Selector::parse("a[href^='/news/']").unwrap();
    let src_sel = Selector::parse(".news-block__details a.darklink").unwrap();
    let time_sel = Selector::parse(".news-block__details .nowrap, .news-block__details span").unwrap();
    let prev_sel = Selector::parse(".news-block__preview").unwrap();
    let img_sel = Selector::parse(".news-block__image img").unwrap();
    let txt = |el: scraper::ElementRef| el.text().collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ").trim().to_string();

    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for block in doc.select(&block_sel) {
        // Title link: prefer the explicit title anchor, else the first /news/ link
        // that is NOT the "/hit" external-redirect source link.
        let link = block.select(&title_sel).next()
            .or_else(|| block.select(&any_title_a).find(|a| {
                a.value().attr("href").map(|h| !h.ends_with("/hit")).unwrap_or(false)
            }));
        let Some(link) = link else { continue };
        let href = link.value().attr("href").unwrap_or("");
        // id = leading number of "/news/<id>-<slug>".
        let article_id = href.trim_start_matches("/news/").split('-').next().unwrap_or("").to_string();
        if article_id.is_empty() || !article_id.chars().all(|c| c.is_ascii_digit()) { continue; }
        if !seen.insert(article_id.clone()) { continue; }
        let title = txt(link);
        if title.is_empty() { continue; }

        let url = if href.starts_with("http") { href.to_string() } else { format!("https://www.forexfactory.com{}", href) };
        let source = block.select(&src_sel).next().map(txt)
            .map(|s| s.trim_start_matches("From ").to_string()).unwrap_or_default();
        let when = block.select(&time_sel).map(txt).find(|t| t.contains("ago") || t.contains("now")).unwrap_or_default();
        let dt = relative_to_utc(&when, now);
        let preview = block.select(&prev_sel).next().map(txt).unwrap_or_default();
        let image_url = block.select(&img_sel).next().and_then(|e| e.value().attr("src")).unwrap_or("").to_string();

        out.push(FfNewsItem {
            article_id, title, url, source, preview,
            published_utc: dt.format("%Y-%m-%dT%H:%M:%S").to_string(),
            image_url,
            hour_utc: dt.hour() as i8,
            weekday: dt.weekday().num_days_from_sunday() as i8,
        });
    }
    out
}

pub async fn fetch_news() -> Result<Vec<FfNewsItem>, String> {
    let html = crate::myfxbook_cal::fetch_html_via_curl(NEWS_URL).await?;
    let items = parse_news(&html, Utc::now());
    if items.is_empty() { return Err("no news parsed (page layout changed?)".into()); }
    Ok(items)
}

// ── DuckDB: calendar ──────────────────────────────────────────────────────────

const CREATE_CAL: &str = "
CREATE TABLE IF NOT EXISTS forexfactory_calendar (
    event_id VARCHAR PRIMARY KEY, title VARCHAR NOT NULL, country VARCHAR, currency VARCHAR,
    impact TINYINT NOT NULL, impact_label VARCHAR, timestamp_utc VARCHAR NOT NULL,
    time_label VARCHAR, weekday TINYINT, hour_utc TINYINT,
    actual_raw VARCHAR, forecast_raw VARCHAR, previous_raw VARCHAR )";
const CREATE_CAL_TODAY: &str = "
CREATE TABLE IF NOT EXISTS forexfactory_calendar_today (
    event_id VARCHAR PRIMARY KEY, title VARCHAR NOT NULL, country VARCHAR, currency VARCHAR,
    impact TINYINT NOT NULL, impact_label VARCHAR, timestamp_utc VARCHAR NOT NULL,
    time_label VARCHAR, weekday TINYINT, hour_utc TINYINT,
    actual_raw VARCHAR, forecast_raw VARCHAR, previous_raw VARCHAR )";

pub fn create_cal(db: &duckdb::Connection) -> Result<(), String> {
    db.execute_batch(CREATE_CAL).map_err(|e| format!("create forexfactory_calendar: {}", e))
}

pub fn upsert_cal(db: &duckdb::Connection, rows: &[FfCalEvent]) -> Result<usize, String> {
    create_cal(db)?;
    let sql = "INSERT INTO forexfactory_calendar
        (event_id,title,country,currency,impact,impact_label,timestamp_utc,time_label,weekday,hour_utc,actual_raw,forecast_raw,previous_raw)
        VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)
        ON CONFLICT (event_id) DO UPDATE SET
          title=EXCLUDED.title, impact=EXCLUDED.impact, impact_label=EXCLUDED.impact_label,
          timestamp_utc=EXCLUDED.timestamp_utc, time_label=EXCLUDED.time_label,
          actual_raw=EXCLUDED.actual_raw, forecast_raw=EXCLUDED.forecast_raw, previous_raw=EXCLUDED.previous_raw";
    let mut n = 0;
    for r in rows {
        let p = duckdb::params![r.event_id,r.title,r.country,r.currency,r.impact as i32,r.impact_label,
            r.timestamp_utc,r.time_label,r.weekday as i32,r.hour_utc as i32,r.actual_raw,r.forecast_raw,r.previous_raw];
        if db.execute(sql, p).is_ok() { n += 1; }
    }
    Ok(n)
}

pub fn write_cal_today(db: &duckdb::Connection) -> Result<usize, String> {
    create_cal(db)?;
    db.execute_batch(CREATE_CAL_TODAY).map_err(|e| format!("create forexfactory_calendar_today: {}", e))?;
    let _ = db.execute("DELETE FROM forexfactory_calendar_today", []);
    let today = Utc::now().format("%Y-%m-%d").to_string();
    db.execute("INSERT INTO forexfactory_calendar_today SELECT * FROM forexfactory_calendar WHERE timestamp_utc LIKE ? || '%'",
        duckdb::params![today]).map_err(|e| format!("rebuild ff cal today: {}", e))
}

/// (ts, currency, impact, title, country, actual, forecast, previous)
pub type FfCalRow = (String, String, i32, String, String, Option<String>, Option<String>, Option<String>);
pub fn read_cal_today(db: &duckdb::Connection) -> Vec<FfCalRow> {
    let q = "SELECT timestamp_utc,currency,impact,title,country,actual_raw,forecast_raw,previous_raw
             FROM forexfactory_calendar_today ORDER BY timestamp_utc ASC";
    let Ok(mut s) = db.prepare(q) else { return Vec::new() };
    let r = s.query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i32>(2)?,r.get::<_,String>(3)?,
        r.get::<_,Option<String>>(4)?.unwrap_or_default(),r.get::<_,Option<String>>(5)?,r.get::<_,Option<String>>(6)?,r.get::<_,Option<String>>(7)?)));
    match r { Ok(it) => it.flatten().collect(), Err(_) => Vec::new() }
}

pub fn write_cal_archive_day(db: &duckdb::Connection, root: &str, day: &str) -> Result<(String, usize), String> {
    if day.len() < 7 { return Err("bad day".into()); }
    let mut s = db.prepare("SELECT event_id,title,country,currency,impact,impact_label,timestamp_utc,time_label,actual_raw,forecast_raw,previous_raw
        FROM forexfactory_calendar WHERE timestamp_utc LIKE ? || '%' ORDER BY timestamp_utc ASC").map_err(|e| format!("prep: {}", e))?;
    let rows = s.query_map([day], |r| Ok(serde_json::json!({
        "event_id":r.get::<_,String>(0)?,"title":r.get::<_,String>(1)?,"country":r.get::<_,Option<String>>(2)?,
        "currency":r.get::<_,Option<String>>(3)?,"impact":r.get::<_,i32>(4)?,"impact_label":r.get::<_,Option<String>>(5)?,
        "timestamp_utc":r.get::<_,String>(6)?,"time_label":r.get::<_,Option<String>>(7)?,
        "actual":r.get::<_,Option<String>>(8)?,"forecast":r.get::<_,Option<String>>(9)?,"previous":r.get::<_,Option<String>>(10)?,
    }))).map_err(|e| format!("query: {}", e))?;
    let events: Vec<serde_json::Value> = rows.flatten().collect();
    write_day_file(root, day, "events", events)
}

// ── DuckDB: news ──────────────────────────────────────────────────────────────

const CREATE_NEWS: &str = "
CREATE TABLE IF NOT EXISTS forexfactory_news (
    article_id VARCHAR PRIMARY KEY, title VARCHAR NOT NULL, url VARCHAR, source VARCHAR,
    preview VARCHAR, published_utc VARCHAR NOT NULL, image_url VARCHAR, hour_utc TINYINT, weekday TINYINT, body VARCHAR )";
const CREATE_NEWS_TODAY: &str = "
CREATE TABLE IF NOT EXISTS forexfactory_news_today (
    article_id VARCHAR PRIMARY KEY, title VARCHAR NOT NULL, url VARCHAR, source VARCHAR,
    preview VARCHAR, published_utc VARCHAR NOT NULL, image_url VARCHAR, hour_utc TINYINT, weekday TINYINT, body VARCHAR )";

pub fn create_news(db: &duckdb::Connection) -> Result<(), String> {
    db.execute_batch(CREATE_NEWS).map_err(|e| format!("create forexfactory_news: {}", e))?;
    let _ = db.execute("ALTER TABLE forexfactory_news ADD COLUMN IF NOT EXISTS body VARCHAR", []);
    Ok(())
}

/// Recursively search a JSON-LD value for an `articleBody` string.
fn find_article_body(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Object(o) => {
            if let Some(b) = o.get("articleBody").and_then(|x| x.as_str()) {
                if !b.trim().is_empty() { return Some(b.to_string()); }
            }
            for val in o.values() { if let Some(b) = find_article_body(val) { return Some(b); } }
            None
        }
        serde_json::Value::Array(a) => a.iter().find_map(find_article_body),
        _ => None,
    }
}

fn collapse_ws(s: &str) -> String { s.split_whitespace().collect::<Vec<_>>().join(" ").trim().to_string() }

/// Best-effort extraction of an article body from an arbitrary publisher page:
/// JSON-LD `articleBody` → article/content paragraphs → og:description (captures
/// social-post / summary text). Returns None if nothing usable is found.
pub fn extract_body(html: &str) -> Option<String> {
    use scraper::{Html, Selector};
    let doc = Html::parse_document(html);

    if let Ok(sel) = Selector::parse(r#"script[type="application/ld+json"]"#) {
        for s in doc.select(&sel) {
            let raw = s.text().collect::<String>();
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw.trim()) {
                if let Some(b) = find_article_body(&v) {
                    let b = collapse_ws(&b);
                    if b.len() > 120 { return Some(b); }
                }
            }
        }
    }
    for sel_str in ["article p", ".article-body p", ".entry-content p", ".post-content p",
                    ".story-body p", "main p"] {
        if let Ok(sel) = Selector::parse(sel_str) {
            let paras: Vec<String> = doc.select(&sel)
                .map(|p| collapse_ws(&p.text().collect::<String>()))
                .filter(|t| t.len() > 40)
                .collect();
            let joined = paras.join("\n\n");
            if joined.len() > 200 { return Some(joined); }
        }
    }
    if let Ok(sel) = Selector::parse(r#"meta[property="og:description"], meta[name="description"]"#) {
        if let Some(m) = doc.select(&sel).next() {
            if let Some(c) = m.value().attr("content") {
                let c = collapse_ws(c);
                if c.len() > 40 { return Some(c); }
            }
        }
    }
    None
}

/// Follow the FF `/hit` redirect to the original publisher and extract the body.
pub async fn fetch_body(hit_url: &str) -> Option<String> {
    let html = crate::myfxbook_cal::fetch_html_via_curl(hit_url).await.ok()?;
    extract_body(&html)
}

/// (article_id, hit_url, day) for items still missing a body, newest first.
pub fn list_missing_news_bodies(db: &duckdb::Connection, limit: usize) -> Vec<(String, String, String)> {
    let q = "SELECT article_id, url, substr(published_utc,1,10) FROM forexfactory_news
             WHERE body IS NULL AND url IS NOT NULL AND url <> ''
             ORDER BY published_utc DESC LIMIT ?";
    let Ok(mut s) = db.prepare(q) else { return Vec::new() };
    let r = s.query_map([limit as i64], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)));
    match r {
        Ok(it) => it.flatten().map(|(id, url, day)| (id, format!("{}/hit", url), day)).collect(),
        Err(_) => Vec::new(),
    }
}

pub fn set_news_body(db: &duckdb::Connection, article_id: &str, body: &str) {
    let _ = db.execute("UPDATE forexfactory_news SET body = ? WHERE article_id = ?",
        duckdb::params![body, article_id]);
}

pub fn upsert_news(db: &duckdb::Connection, items: &[FfNewsItem]) -> Result<usize, String> {
    create_news(db)?;
    let sql = "INSERT INTO forexfactory_news
        (article_id,title,url,source,preview,published_utc,image_url,hour_utc,weekday)
        VALUES (?,?,?,?,?,?,?,?,?)
        ON CONFLICT (article_id) DO UPDATE SET
          title=EXCLUDED.title, url=EXCLUDED.url, source=EXCLUDED.source,
          preview=EXCLUDED.preview, image_url=EXCLUDED.image_url";
    let mut n = 0;
    for it in items {
        let p = duckdb::params![it.article_id,it.title,it.url,it.source,it.preview,it.published_utc,it.image_url,it.hour_utc as i32,it.weekday as i32];
        if db.execute(sql, p).is_ok() { n += 1; }
    }
    Ok(n)
}

pub fn write_news_today(db: &duckdb::Connection) -> Result<usize, String> {
    create_news(db)?;
    db.execute_batch(CREATE_NEWS_TODAY).map_err(|e| format!("create forexfactory_news_today: {}", e))?;
    let _ = db.execute("ALTER TABLE forexfactory_news_today ADD COLUMN IF NOT EXISTS body VARCHAR", []);
    let _ = db.execute("DELETE FROM forexfactory_news_today", []);
    let today = Utc::now().format("%Y-%m-%d").to_string();
    db.execute("INSERT INTO forexfactory_news_today
        (article_id,title,url,source,preview,published_utc,image_url,hour_utc,weekday,body)
        SELECT article_id,title,url,source,preview,published_utc,image_url,hour_utc,weekday,body
        FROM forexfactory_news WHERE published_utc LIKE ? || '%'",
        duckdb::params![today]).map_err(|e| format!("rebuild ff news today: {}", e))
}

/// (id, title, url, source, preview, published_utc)
pub type FfNewsRow = (String, String, String, String, String, String);
pub fn read_news_today(db: &duckdb::Connection) -> Vec<FfNewsRow> {
    let q = "SELECT article_id,title,url,source,preview,published_utc FROM forexfactory_news_today ORDER BY published_utc DESC";
    let Ok(mut s) = db.prepare(q) else { return Vec::new() };
    let r = s.query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?.unwrap_or_default(),
        r.get::<_,Option<String>>(3)?.unwrap_or_default(),r.get::<_,Option<String>>(4)?.unwrap_or_default(),r.get::<_,String>(5)?)));
    match r { Ok(it) => it.flatten().collect(), Err(_) => Vec::new() }
}

pub fn write_news_archive_day(db: &duckdb::Connection, root: &str, day: &str) -> Result<(String, usize), String> {
    if day.len() < 7 { return Err("bad day".into()); }
    let mut s = db.prepare("SELECT article_id,title,url,source,preview,published_utc,image_url,body
        FROM forexfactory_news WHERE published_utc LIKE ? || '%' ORDER BY published_utc DESC").map_err(|e| format!("prep: {}", e))?;
    let rows = s.query_map([day], |r| Ok(serde_json::json!({
        "article_id":r.get::<_,String>(0)?,"title":r.get::<_,String>(1)?,"url":r.get::<_,Option<String>>(2)?,
        "source":r.get::<_,Option<String>>(3)?,"preview":r.get::<_,Option<String>>(4)?,
        "published_utc":r.get::<_,String>(5)?,"image_url":r.get::<_,Option<String>>(6)?,
        "body":r.get::<_,Option<String>>(7)?,
    }))).map_err(|e| format!("query: {}", e))?;
    let items: Vec<serde_json::Value> = rows.flatten().collect();
    write_day_file(root, day, "items", items)
}

// ── Shared per-day JSON writer ────────────────────────────────────────────────

fn write_day_file(root: &str, day: &str, key: &str, items: Vec<serde_json::Value>) -> Result<(String, usize), String> {
    let count = items.len();
    let dir = std::path::Path::new(root).join(&day[..7]);
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {}", dir.display(), e))?;
    let path = dir.join(format!("{}.json", day));
    let payload = serde_json::json!({ "date": day, "count": count, "generated_at": Utc::now().to_rfc3339(), key: items });
    std::fs::write(&path, serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?)
        .map_err(|e| format!("write {}: {}", path.display(), e))?;
    Ok((path.to_string_lossy().to_string(), count))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_json_parses() {
        // Real FF format: the `days` key is UNQUOTED (JS literal); inner keys quoted.
        let html = r#"<script>window.calendarComponentStates[1] = {
            days: [{"date":"x","dateline":1780779600,"events":[
            {"id":12345,"name":"Non-Farm Employment Change","country":"US","currency":"USD",
             "impactName":"high","impactTitle":"High Impact Expected","timeLabel":"8:30am",
             "dateline":1780823700,"actual":"150K","forecast":"160K","previous":"140K"}]}]};</script>"#;
        let evs = parse_calendar(html);
        assert_eq!(evs.len(), 1);
        let e = &evs[0];
        assert_eq!(e.event_id, "12345");
        assert_eq!(e.currency, "USD");
        assert_eq!(e.impact, 3);
        assert_eq!(e.title, "Non-Farm Employment Change");
        assert_eq!(e.actual_raw.as_deref(), Some("150K"));
        assert!(e.timestamp_utc.starts_with("2026-"));
    }

    #[test]
    fn news_parses() {
        let now = DateTime::<Utc>::from_timestamp(1_780_000_000, 0).unwrap();
        let html = r#"<div class="news-block">
          <div class="news-block__title"><a href="/news/1402710-us-trade-gap-narrows">US Trade Gap Narrows</a></div>
          <div class="news-block__content"><div class="news-block__details">
            <a href="/news/1402710/hit" class="darklink">From advisorperspectives.com</a>
            <span class="nowrap">5 min ago</span></div>
            <div class="news-block__preview">Some preview text.</div></div></div>"#;
        let items = parse_news(html, now);
        assert_eq!(items.len(), 1);
        let it = &items[0];
        assert_eq!(it.article_id, "1402710");
        assert_eq!(it.title, "US Trade Gap Narrows");
        assert_eq!(it.source, "advisorperspectives.com");
        assert_eq!(it.published_utc, (now - chrono::Duration::minutes(5)).format("%Y-%m-%dT%H:%M:%S").to_string());
    }
}
