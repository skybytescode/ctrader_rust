//! MyFXBook economic calendar fetcher + store.
//!
//! Unlike FXStreet (JS-rendered → needs the puppeteer econcal proxy), MyFXBook
//! server-renders the calendar in plain HTML, so we fetch it directly with
//! reqwest and parse it with `scraper`. The page returns the current week's
//! events (incl. today). Mirrors the `ec_realtime` pattern:
//!   - `myfxbook_ec_today`            — today's events only (UI, rebuilt each fetch)
//!   - `myfxbook_economic_calendar`   — append/upsert archive (per-day JSON source)

use chrono::{Datelike, Timelike, Utc};

const CAL_URL: &str = "https://www.myfxbook.com/forex-economic-calendar";

/// One MyFXBook calendar event, ready for DB insertion / JSON export.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MfbEvent {
    pub row_id: String,        // MyFXBook's data-row-id (stable per event) — PK
    pub event_name: String,
    pub currency: String,
    pub country: String,
    pub importance: i8,        // 0 none/holiday · 1 low · 2 medium · 3 high
    pub timestamp_utc: String, // "YYYY-MM-DDTHH:MM:SS" (UTC)
    pub weekday: i8,
    pub hour_utc: i8,
    pub actual_raw: Option<String>,
    pub forecast_raw: Option<String>,
    pub previous_raw: Option<String>,
    pub actual: Option<f64>,
    pub forecast: Option<f64>,
    pub previous: Option<f64>,
    pub url: Option<String>,
}

/// Parse a display value like "2.67%", "1,234", "1.2K", "-563.30", "" into a
/// float. Returns None for blanks / dashes / unparseable text.
fn parse_num(s: &str) -> Option<f64> {
    let t = s.trim().replace(',', "");
    let t = t.trim_end_matches('%').trim();
    if t.is_empty() || t == "-" || t == "—" { return None; }
    let (body, mult): (&str, f64) = match t.chars().last() {
        Some('K') | Some('k') => (&t[..t.len() - 1], 1e3),
        Some('M') | Some('m') => (&t[..t.len() - 1], 1e6),
        Some('B') | Some('b') => (&t[..t.len() - 1], 1e9),
        Some('T') => (&t[..t.len() - 1], 1e12),
        _ => (t, 1.0),
    };
    body.trim().parse::<f64>().ok().map(|n| n * mult)
}

fn norm(s: String) -> Option<String> {
    let t = s.trim().to_string();
    if t.is_empty() || t == "-" || t == "—" { None } else { Some(t) }
}

/// Fetch the MyFXBook calendar HTML and parse it into events. Async fetch, then
/// a synchronous parse (scraper's `Html` is !Send, so it must not cross .await).
pub async fn fetch_calendar() -> Result<Vec<MfbEvent>, String> {
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36")
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| format!("build client: {}", e))?;
    let resp = client.get(CAL_URL).send().await.map_err(|e| format!("fetch: {}", e))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let html = resp.text().await.map_err(|e| format!("read body: {}", e))?;
    Ok(parse_calendar(&html))
}

/// Parse the MyFXBook calendar page HTML into events. Pure / synchronous so it's
/// unit-testable and never holds a non-Send `Html` across an await point.
pub fn parse_calendar(html: &str) -> Vec<MfbEvent> {
    use scraper::{Html, Selector};
    let doc = Html::parse_document(html);
    let row_sel = Selector::parse("tr.economicCalendarRow").unwrap();
    let left_sel = Selector::parse("span.calendarLeft").unwrap();
    let link_sel = Selector::parse("a.calendar-event-link").unwrap();
    let icon_sel = Selector::parse("i[title]").unwrap();
    let td_sel = Selector::parse("td").unwrap();
    let prev_sel = Selector::parse("span.previousCell").unwrap();
    let actual_sel = Selector::parse("span.actualCell").unwrap();
    let cons_sel = Selector::parse("td[data-concensus]").unwrap();

    let txt = |el: scraper::ElementRef| el.text().collect::<String>().trim().to_string();
    let mut out = Vec::new();

    for row in doc.select(&row_sel) {
        let row_id = row.value().attr("data-row-id")
            .map(|s| s.to_string())
            .or_else(|| row.value().attr("id").map(|s| s.trim_start_matches("calRow").to_string()))
            .unwrap_or_default();
        if row_id.is_empty() { continue; }

        // Timestamp + importance from the calendarLeft span (epoch ms is UTC).
        let left = row.select(&left_sel).next();
        let ts_ms: Option<i64> = left.and_then(|e| e.value().attr("time")).and_then(|s| s.parse::<i64>().ok());
        let importance: i8 = left.and_then(|e| e.value().attr("importance"))
            .and_then(|s| s.parse::<i8>().ok()).unwrap_or(0);
        let dt = match ts_ms.and_then(chrono::DateTime::<Utc>::from_timestamp_millis) {
            Some(d) => d,
            None => continue, // no usable time → skip
        };

        let event_name = row.select(&link_sel).next().map(txt).unwrap_or_default();
        if event_name.is_empty() { continue; }
        let url = row.select(&link_sel).next().and_then(|e| e.value().attr("href")).map(|s| s.to_string());
        let country = row.select(&icon_sel).next()
            .and_then(|e| e.value().attr("title"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default();

        // Currency: the bare 3-letter-uppercase cell among the row's td texts.
        let currency = row.select(&td_sel)
            .map(txt)
            .find(|t| t.len() == 3 && t.chars().all(|c| c.is_ascii_uppercase()))
            .unwrap_or_default();

        let previous_raw = row.select(&prev_sel).next().map(txt).and_then(norm);
        let actual_raw   = row.select(&actual_sel).next().map(txt).and_then(norm);
        let forecast_raw = row.select(&cons_sel).next().map(txt).and_then(norm);

        out.push(MfbEvent {
            row_id,
            event_name,
            currency,
            country,
            importance,
            timestamp_utc: dt.format("%Y-%m-%dT%H:%M:%S").to_string(),
            weekday: dt.weekday().num_days_from_sunday() as i8,
            hour_utc: dt.hour() as i8,
            actual:   actual_raw.as_deref().and_then(parse_num),
            forecast: forecast_raw.as_deref().and_then(parse_num),
            previous: previous_raw.as_deref().and_then(parse_num),
            actual_raw, forecast_raw, previous_raw,
            url,
        });
    }
    out
}

// ── DuckDB ────────────────────────────────────────────────────────────────────

pub const MFB_TABLE: &str = "myfxbook_economic_calendar";

const CREATE_MFB: &str = "
CREATE TABLE IF NOT EXISTS myfxbook_economic_calendar (
    row_id        VARCHAR PRIMARY KEY,
    event_name    VARCHAR NOT NULL,
    currency      VARCHAR NOT NULL,
    country       VARCHAR,
    importance    TINYINT NOT NULL,
    timestamp_utc VARCHAR NOT NULL,
    weekday       TINYINT NOT NULL,
    hour_utc      TINYINT NOT NULL,
    actual_raw    VARCHAR,
    forecast_raw  VARCHAR,
    previous_raw  VARCHAR,
    actual        DOUBLE,
    forecast      DOUBLE,
    previous      DOUBLE,
    url           VARCHAR
)";

const CREATE_MFB_TODAY: &str = "
CREATE TABLE IF NOT EXISTS myfxbook_ec_today (
    row_id        VARCHAR PRIMARY KEY,
    event_name    VARCHAR NOT NULL,
    currency      VARCHAR NOT NULL,
    country       VARCHAR,
    importance    TINYINT NOT NULL,
    timestamp_utc VARCHAR NOT NULL,
    weekday       TINYINT NOT NULL,
    hour_utc      TINYINT NOT NULL,
    actual_raw    VARCHAR,
    forecast_raw  VARCHAR,
    previous_raw  VARCHAR,
    actual        DOUBLE,
    forecast      DOUBLE,
    previous      DOUBLE,
    url           VARCHAR
)";

/// Create the archive table if missing. Idempotent.
pub fn create_mfb_table(db: &duckdb::Connection) -> Result<(), String> {
    db.execute_batch(CREATE_MFB).map_err(|e| format!("create {}: {}", MFB_TABLE, e))
}

/// Upsert events into the archive table. Updates the resolving fields (actual /
/// forecast / previous) on conflict so values fill in as releases land.
pub fn upsert_mfb(db: &duckdb::Connection, rows: &[MfbEvent]) -> Result<usize, String> {
    create_mfb_table(db)?;
    let upsert = "
        INSERT INTO myfxbook_economic_calendar
            (row_id, event_name, currency, country, importance, timestamp_utc,
             weekday, hour_utc, actual_raw, forecast_raw, previous_raw,
             actual, forecast, previous, url)
        VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
        ON CONFLICT (row_id) DO UPDATE SET
            event_name   = EXCLUDED.event_name,
            importance   = EXCLUDED.importance,
            timestamp_utc= EXCLUDED.timestamp_utc,
            actual_raw   = EXCLUDED.actual_raw,
            forecast_raw = EXCLUDED.forecast_raw,
            previous_raw = EXCLUDED.previous_raw,
            actual       = EXCLUDED.actual,
            forecast     = EXCLUDED.forecast,
            previous     = EXCLUDED.previous
    ";
    let mut n = 0usize;
    for r in rows {
        let params = duckdb::params![
            r.row_id, r.event_name, r.currency, r.country, r.importance as i32,
            r.timestamp_utc, r.weekday as i32, r.hour_utc as i32,
            r.actual_raw, r.forecast_raw, r.previous_raw,
            r.actual, r.forecast, r.previous, r.url,
        ];
        if db.execute(upsert, params).is_ok() { n += 1; }
    }
    Ok(n)
}

/// Rebuild `myfxbook_ec_today` from the archive (today's UTC rows) so it always
/// reflects the full day regardless of how many events a fetch returned.
pub fn write_today_from_archive(db: &duckdb::Connection) -> Result<usize, String> {
    create_mfb_table(db)?;
    db.execute_batch(CREATE_MFB_TODAY).map_err(|e| format!("create myfxbook_ec_today: {}", e))?;
    let _ = db.execute("DELETE FROM myfxbook_ec_today", []);
    let today = Utc::now().format("%Y-%m-%d").to_string();
    db.execute(
        "INSERT INTO myfxbook_ec_today
         SELECT * FROM myfxbook_economic_calendar WHERE timestamp_utc LIKE ? || '%'",
        duckdb::params![today],
    ).map_err(|e| format!("rebuild myfxbook_ec_today: {}", e))
}

/// Read today's events for the UI, ordered by time. Tuple shape mirrors the EC
/// `EcTodayRaw` payload plus country: (ts, currency, importance, name, country,
/// actual, forecast, previous).
pub type MfbTodayRow = (String, String, i32, String, String, Option<f64>, Option<f64>, Option<f64>);

pub fn read_today(db: &duckdb::Connection) -> Vec<MfbTodayRow> {
    let q = "SELECT timestamp_utc, currency, importance, event_name, country, actual, forecast, previous
             FROM myfxbook_ec_today ORDER BY timestamp_utc ASC";
    let Ok(mut stmt) = db.prepare(q) else { return Vec::new() };
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i32>(2)?,
            r.get::<_, String>(3)?, r.get::<_, Option<String>>(4)?.unwrap_or_default(),
            r.get::<_, Option<f64>>(5)?, r.get::<_, Option<f64>>(6)?, r.get::<_, Option<f64>>(7)?,
        ))
    });
    match rows { Ok(it) => it.flatten().collect(), Err(_) => Vec::new() }
}

/// Write one per-day JSON archive file under `<root>/YYYY-MM/YYYY-MM-DD.json`
/// from the archive table. Mirrors the EC/news archive file format.
pub fn write_archive_day(db: &duckdb::Connection, root: &str, day: &str) -> Result<(String, usize), String> {
    if day.len() < 7 { return Err("bad day".into()); }
    let mut stmt = db.prepare(
        "SELECT row_id, event_name, currency, country, importance, timestamp_utc, weekday, hour_utc,
                actual_raw, forecast_raw, previous_raw, actual, forecast, previous, url
         FROM myfxbook_economic_calendar
         WHERE timestamp_utc LIKE ? || '%'
         ORDER BY timestamp_utc ASC"
    ).map_err(|e| format!("prepare archive query: {}", e))?;
    let rows = stmt.query_map([day], |r| {
        Ok(serde_json::json!({
            "row_id":        r.get::<_, String>(0)?,
            "event_name":    r.get::<_, String>(1)?,
            "currency":      r.get::<_, String>(2)?,
            "country":       r.get::<_, Option<String>>(3)?,
            "importance":    r.get::<_, i32>(4)?,
            "timestamp_utc": r.get::<_, String>(5)?,
            "weekday":       r.get::<_, i32>(6)?,
            "hour_utc":      r.get::<_, i32>(7)?,
            "actual_raw":    r.get::<_, Option<String>>(8)?,
            "forecast_raw":  r.get::<_, Option<String>>(9)?,
            "previous_raw":  r.get::<_, Option<String>>(10)?,
            "actual":        r.get::<_, Option<f64>>(11)?,
            "forecast":      r.get::<_, Option<f64>>(12)?,
            "previous":      r.get::<_, Option<f64>>(13)?,
            "url":           r.get::<_, Option<String>>(14)?,
        }))
    }).map_err(|e| format!("query archive: {}", e))?;
    let events: Vec<serde_json::Value> = rows.flatten().collect();
    let count = events.len();

    let dir = std::path::Path::new(root).join(&day[..7]);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create dir {}: {}", dir.display(), e))?;
    let path = dir.join(format!("{}.json", day));
    let payload = serde_json::json!({
        "date": day, "count": count,
        "generated_at": Utc::now().to_rfc3339(),
        "events": events,
    });
    let pretty = serde_json::to_string_pretty(&payload).map_err(|e| format!("serialize: {}", e))?;
    std::fs::write(&path, pretty).map_err(|e| format!("write {}: {}", path.display(), e))?;
    Ok((path.to_string_lossy().to_string(), count))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_num_handles_common_formats() {
        assert_eq!(parse_num("2.67%"), Some(2.67));
        assert_eq!(parse_num("1,234"), Some(1234.0));
        assert_eq!(parse_num("1.2K"), Some(1200.0));
        assert_eq!(parse_num("-563.30"), Some(-563.30));
        assert_eq!(parse_num(""), None);
        assert_eq!(parse_num("—"), None);
    }

    #[test]
    fn parse_calendar_extracts_a_row() {
        let html = r#"
        <table><tr id="calRow305236" data-row-id="305236" class="economicCalendarRow">
          <td><span name="calendarLeft" class="calendarLeft text-center" importance="2" time="1780972500000"></span></td>
          <td><i class="New Zealand align-center" title="New Zealand"><span class="flag"></span></i></td>
          <td class="calendarToggleCell"> NZD </td>
          <td class="text-left"><a href="https://www.myfxbook.com/x" class="calendar-event-link">6-Month Bill Auction</a></td>
          <td><div class="impact_medium">Medium</div></td>
          <td data-previous="305236" previous-value="2.67"><span class="previousCell">2.67%</span></td>
          <td data-concensus="305236"><span class="consensusCell">3.00%</span></td>
          <td data-actual="305236"><span class="actualCell">2.80%</span></td>
        </tr></table>"#;
        let evs = parse_calendar(html);
        assert_eq!(evs.len(), 1);
        let e = &evs[0];
        assert_eq!(e.row_id, "305236");
        assert_eq!(e.currency, "NZD");
        assert_eq!(e.country, "New Zealand");
        assert_eq!(e.importance, 2);
        assert_eq!(e.event_name, "6-Month Bill Auction");
        assert_eq!(e.previous, Some(2.67));
        assert_eq!(e.forecast, Some(3.00));
        assert_eq!(e.actual, Some(2.80));
        assert!(e.timestamp_utc.starts_with("2026-06-09"));
    }
}
