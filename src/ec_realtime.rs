//! Real-time Economic Calendar fetcher.
//!
//! Fetches EUR/USD economic events from the econcal proxy (localhost:6000)
//! and writes them to DuckDB tables:
//!   - `eurusd_ec_today`            — today's events only (fast UI reads, cleared daily)
//!   - `eurusd_economic_calendar`   — historical (append new actuals)

use chrono::{Datelike, NaiveDateTime, Timelike, Utc};
use serde::Deserialize;

// ── econcal proxy URL ────────────────────────────────────────────────────────

const PROXY_BASE: &str = "http://localhost:6000";

// ── JSON deserialization structs (match FXStreet API) ────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct EcEvent {
    #[serde(default)]
    pub event: Option<EcEventInner>,
    #[serde(default)]
    pub date_utc: Option<String>,
    #[serde(default)]
    pub volatility: Option<i32>,
    #[serde(default)]
    pub actual: Option<f64>,
    #[serde(default)]
    pub consensus: Option<f64>,
    #[serde(default)]
    pub previous: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct EcEventInner {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub currency_id: Option<String>,
    #[serde(default)]
    pub international_country_code: Option<String>,
}

/// Parsed EC row ready for DB insertion.
#[derive(Debug, Clone)]
pub struct EcRow {
    pub event_date_id: String,
    pub event_id: String,
    pub event_name: String,
    pub currency: String,
    pub country_code: String,
    pub volatility: i8,
    pub timestamp_utc: String, // ISO 8601
    pub weekday: i8,
    pub hour_utc: i8,
    pub actual_raw: Option<String>,
    pub forecast_raw: Option<String>,
    pub previous_raw: Option<String>,
    pub actual: Option<f64>,
    pub forecast: Option<f64>,
    pub previous: Option<f64>,
    pub surprise: Option<f64>,
    pub beats_forecast: Option<i8>,
}

// ── Fetch & parse ────────────────────────────────────────────────────────────

/// Fetch today's EUR/USD events from the econcal proxy.
pub async fn fetch_today_events() -> Result<Vec<EcRow>, String> {
    let today = Utc::now().format("%Y%m%d").to_string();
    fetch_events_for_date(&today, &today).await
}

/// Fetch EUR/USD events for a date range (YYYYMMDD format).
pub async fn fetch_events_for_date(start: &str, end: &str) -> Result<Vec<EcRow>, String> {
    let url = format!(
        "{}/v4/eventdate/mini?view=1&start={}&end={}",
        PROXY_BASE, start, end
    );

    let resp = reqwest::Client::new()
        .get(&url)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("EC fetch failed: {}", e))?;

    let text = resp.text().await.map_err(|e| format!("EC read body: {}", e))?;

    // econcal sometimes appends stray `{}` after the JSON array — strip it
    let trimmed = text.trim();
    let json_text = if let Some(pos) = trimmed.rfind(']') {
        &trimmed[..=pos]
    } else {
        trimmed
    };

    let events: Vec<EcEvent> =
        serde_json::from_str(json_text).map_err(|e| format!("EC parse JSON: {}", e))?;

    let rows: Vec<EcRow> = events
        .into_iter()
        .filter_map(|ev| parse_event(ev))
        .collect();

    Ok(rows)
}

fn parse_event(ev: EcEvent) -> Option<EcRow> {
    let inner = ev.event.as_ref()?;
    let currency = inner.currency_id.as_deref().unwrap_or("");
    // No source-side currency filter — the parser returns every event the
    // proxy gives us. Downstream writers apply their own filters:
    //   - `write_ec_to_db` keeps only EUR + USD (legacy eurusd_* tables,
    //     Calendar tab behavior unchanged)
    //   - `upsert_xauusd_ec` keeps USD/EUR/GBP/JPY/CHF/AUD/CNY (gold-focused
    //     xauusd_economic_calendar)
    if currency.is_empty() {
        return None;
    }

    let eid = inner.id.as_deref().unwrap_or("").to_string();
    let date_utc = ev.date_utc.as_deref().unwrap_or("");
    if date_utc.is_empty() {
        return None;
    }

    let edid = format!("{}__{}", eid, date_utc);
    let name = inner.name.as_deref().unwrap_or("").to_string();
    let cc = inner.international_country_code.as_deref().unwrap_or("").to_string();
    let vol = ev.volatility.unwrap_or(0) as i8;

    // Parse timestamp — FXStreet "DateUtc" is actual UTC (despite the name)
    let ts_str = date_utc.replace("Z", "");
    let ts = NaiveDateTime::parse_from_str(&ts_str, "%Y-%m-%dT%H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(&ts_str, "%Y-%m-%dT%H:%M:%S%.f"))
        .ok()?;
    let weekday = ts.weekday().num_days_from_monday() as i8;
    let hour = ts.hour() as i8;

    let actual = ev.actual;
    let forecast = ev.consensus;
    let previous = ev.previous;

    let a_raw = actual.map(|v| v.to_string());
    let f_raw = forecast.map(|v| v.to_string());
    let p_raw = previous.map(|v| v.to_string());

    let (surprise, beats) = match (actual, forecast) {
        (Some(a), Some(f)) => {
            let s = a - f;
            let b = if s > 0.0 { 1i8 } else if s < 0.0 { -1 } else { 0 };
            (Some(s), Some(b))
        }
        _ => (None, None),
    };

    Some(EcRow {
        event_date_id: edid,
        event_id: eid,
        event_name: name,
        currency: currency.to_string(),
        country_code: cc,
        volatility: vol,
        timestamp_utc: ts.format("%Y-%m-%dT%H:%M:%S").to_string(),
        weekday,
        hour_utc: hour,
        actual_raw: a_raw,
        forecast_raw: f_raw,
        previous_raw: p_raw,
        actual,
        forecast,
        previous,
        surprise,
        beats_forecast: beats,
    })
}

// ── DuckDB write ─────────────────────────────────────────────────────────────

const CREATE_EC_TODAY: &str = "
CREATE TABLE IF NOT EXISTS eurusd_ec_today (
    event_date_id   VARCHAR PRIMARY KEY,
    event_id        VARCHAR NOT NULL,
    event_name      VARCHAR NOT NULL,
    currency        VARCHAR NOT NULL,
    country_code    VARCHAR NOT NULL,
    volatility      TINYINT NOT NULL,
    timestamp_utc   VARCHAR NOT NULL,
    weekday         TINYINT NOT NULL,
    hour_utc        TINYINT NOT NULL,
    actual_raw      VARCHAR,
    forecast_raw    VARCHAR,
    previous_raw    VARCHAR,
    actual          DOUBLE,
    forecast        DOUBLE,
    previous        DOUBLE,
    surprise        DOUBLE,
    beats_forecast  TINYINT,
    unit            VARCHAR
)
";

/// Write today's events to `eurusd_ec_today` (replace all) and upsert into
/// `eurusd_economic_calendar` (historical).
pub fn write_ec_to_db(db: &duckdb::Connection, rows: &[EcRow]) -> Result<usize, String> {
    // Drop and recreate today table (schema might have changed)
    let _ = db.execute("DROP TABLE IF EXISTS eurusd_ec_today", []);
    db.execute_batch(CREATE_EC_TODAY).map_err(|e| format!("create ec_today: {}", e))?;

    let insert_today = "INSERT INTO eurusd_ec_today VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)";
    let mut inserted = 0usize;

    // The parser used to drop non-EUR/USD events at source. It now returns
    // every currency (so the gold bootstrap can see GBP/JPY/CHF/AUD/CNY)
    // and the EUR/USD filter moved here — the Calendar tab's source tables
    // stay EUR/USD-only.
    for r in rows.iter().filter(|r| r.currency == "EUR" || r.currency == "USD") {
        let params = duckdb::params![
            r.event_date_id,
            r.event_id,
            r.event_name,
            r.currency,
            r.country_code,
            r.volatility as i32,
            r.timestamp_utc,
            r.weekday as i32,
            r.hour_utc as i32,
            r.actual_raw,
            r.forecast_raw,
            r.previous_raw,
            r.actual,
            r.forecast,
            r.previous,
            r.surprise,
            r.beats_forecast.map(|v| v as i32),
            None::<String>,  // unit
        ];

        match db.execute(insert_today, params) {
            Ok(_) => inserted += 1,
            Err(e) => {
                if inserted == 0 {
                    println!("EC: insert error (first row): {}", e);
                }
            }
        }
    }

    // Upsert into historical table (skip if table doesn't exist)
    let hist_exists = db
        .query_row(
            "SELECT COUNT(*) FROM information_schema.tables WHERE table_name = 'eurusd_economic_calendar'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0) > 0;

    let mut historical_updated = 0usize;
    if !hist_exists {
        println!("EC: historical table not found, skipping upsert");
        println!("EC: {} events in today table", inserted);
        return Ok(inserted);
    }
    for r in rows.iter().filter(|r| r.currency == "EUR" || r.currency == "USD") {
        // Try INSERT, on conflict UPDATE actual/forecast/previous/surprise/beats
        let upsert = "
            INSERT INTO eurusd_economic_calendar
                (event_date_id, event_id, event_name, currency, country_code,
                 volatility, timestamp_utc, weekday, hour_utc,
                 actual_raw, forecast_raw, previous_raw,
                 actual, forecast, previous, surprise, beats_forecast, unit)
            VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
            ON CONFLICT (event_date_id) DO UPDATE SET
                actual_raw     = EXCLUDED.actual_raw,
                forecast_raw   = EXCLUDED.forecast_raw,
                previous_raw   = EXCLUDED.previous_raw,
                actual         = EXCLUDED.actual,
                forecast       = EXCLUDED.forecast,
                previous       = EXCLUDED.previous,
                surprise       = EXCLUDED.surprise,
                beats_forecast = EXCLUDED.beats_forecast
        ";

        let params = duckdb::params![
            r.event_date_id,
            r.event_id,
            r.event_name,
            r.currency,
            r.country_code,
            r.volatility as i32,
            r.timestamp_utc,
            r.weekday as i32,
            r.hour_utc as i32,
            r.actual_raw,
            r.forecast_raw,
            r.previous_raw,
            r.actual,
            r.forecast,
            r.previous,
            r.surprise,
            r.beats_forecast.map(|v| v as i32),
            None::<String>,
        ];

        if db.execute(upsert, params).is_ok() {
            historical_updated += 1;
        }
    }

    // Also keep `xauusd_economic_calendar` current with today's gold-relevant
    // events. The Calendar tab reads from this table now (filtered to today),
    // so it must be refreshed on every live EC cycle alongside the legacy
    // EUR/USD tables above. Filters to the gold currency set internally.
    let xau_written = upsert_xauusd_ec(db, rows).unwrap_or(0);

    println!("EC: {} events in today table, {} upserted to historical, {} to xauusd_economic_calendar",
             inserted, historical_updated, xau_written);
    Ok(inserted)
}

/// Read today's event data from `xauusd_economic_calendar` for the Calendar
/// tab. The tab now sources from the gold-focused historical table (filtered
/// to today's UTC date), which covers all 7 gold-relevant currencies
/// (USD/EUR/GBP/JPY/CHF/AUD/CNY) instead of just EUR + USD. The wire format
/// is unchanged — same 8-tuple per row, same WS message type, same UI.
pub fn read_ec_today_raw(db: &duckdb::Connection)
    -> Vec<(String, String, i32, String, Option<f64>, Option<f64>, Option<f64>, Option<f64>)>
{
    let today = Utc::now().format("%Y-%m-%d").to_string();
    let query = "
        SELECT timestamp_utc, currency, volatility, event_name,
               actual, forecast, previous, surprise
        FROM xauusd_economic_calendar
        WHERE substr(timestamp_utc, 1, 10) = ?
        ORDER BY timestamp_utc
    ";
    let mut stmt = match db.prepare(query) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map([today.as_str()], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i32>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<f64>>(4)?,
            row.get::<_, Option<f64>>(5)?,
            row.get::<_, Option<f64>>(6)?,
            row.get::<_, Option<f64>>(7)?,
        ))
    });
    match rows {
        Ok(r) => r.flatten().collect(),
        Err(_) => Vec::new(),
    }
}

/// Format raw EC data into display lines with live countdowns.
pub fn format_ec_lines(
    raw: &[(String, String, i32, String, Option<f64>, Option<f64>, Option<f64>, Option<f64>)],
) -> Vec<String> {
    let now_utc = Utc::now().naive_utc();
    let thirty_min_ago = now_utc - chrono::Duration::minutes(30);

    let mut past_lines = Vec::new();
    let mut present_lines = Vec::new();
    let mut future_lines = Vec::new();

    for (ts, cur, vol, name, actual, forecast, previous, surprise) in raw {
        // Convert UTC timestamp to local time for display
        let time_str = NaiveDateTime::parse_from_str(ts, "%Y-%m-%dT%H:%M:%S")
            .ok()
            .and_then(|ndt| ndt.and_utc().with_timezone(&chrono::Local).format("%H:%M").to_string().into())
            .unwrap_or_else(|| if ts.len() >= 16 { ts[11..16].to_string() } else { ts.to_string() });
        let vol_stars = match vol {
            3 => "***",
            2 => "** ",
            _ => "*  ",
        };
        let event_time = NaiveDateTime::parse_from_str(ts, "%Y-%m-%dT%H:%M:%S").ok();
        let short_name = if name.len() > 38 { &name[..38] } else { name.as_str() };

        let is_past = event_time.map(|et| et < now_utc).unwrap_or(false);
        let is_recent = event_time.map(|et| et >= thirty_min_ago && et <= now_utc).unwrap_or(false);

        let a_str = actual.map(|v| format!("{:.2}", v)).unwrap_or_else(|| "--".to_string());
        let f_str = forecast.map(|v| format!("{:.2}", v)).unwrap_or_else(|| "--".to_string());
        let p_str = previous.map(|v| format!("{:.2}", v)).unwrap_or_else(|| "--".to_string());
        let s_str = if let Some(s) = surprise {
            if *s > 0.0 { format!("+{:.2}", s) }
            else if *s < 0.0 { format!("{:.2}", s) }
            else { "0".to_string() }
        } else {
            "--".to_string()
        };

        if is_past {
            let line = format!(
                "  {} {} {} A:{} F:{} P:{} S:{}  {}",
                time_str, cur, vol_stars, a_str, f_str, p_str, s_str, short_name
            );
            if is_recent { present_lines.push(line); } else { past_lines.push(line); }
        } else {
            let countdown = if let Some(et) = event_time {
                let diff = et - now_utc;
                let total_secs = diff.num_seconds();
                if total_secs > 3600 {
                    format!(" (in {}h{}m)", total_secs / 3600, (total_secs % 3600) / 60)
                } else if total_secs > 60 {
                    format!(" (in {}m)", total_secs / 60)
                } else if total_secs > 0 {
                    format!(" (in {}s)", total_secs)
                } else {
                    " (NOW)".to_string()
                }
            } else {
                String::new()
            };
            let line = format!(
                "  {} {} {} F:{} P:{}  {}{}",
                time_str, cur, vol_stars, f_str, p_str, short_name, countdown
            );
            future_lines.push(line);
        }
    }

    let mut lines = Vec::new();
    if !future_lines.is_empty() {
        lines.push(format!("--- UPCOMING ({}) ---", future_lines.len()));
        lines.extend(future_lines);
    }
    if !present_lines.is_empty() {
        lines.push(format!("--- JUST RELEASED ({}) ---", present_lines.len()));
        lines.extend(present_lines);
    }
    if !past_lines.is_empty() {
        lines.push(format!("--- PAST ({}) ---", past_lines.len()));
        lines.extend(past_lines);
    }
    if lines.is_empty() {
        lines.push("No EC events for today".to_string());
    }
    lines
}

/// Read today's events from `eurusd_ec_today` for UI display.
/// Returns formatted lines sorted by time, grouped: PAST / PRESENT / UPCOMING.
pub fn read_ec_today(db: &duckdb::Connection) -> Vec<String> {
    let query = "
        SELECT timestamp_utc, currency, volatility, event_name,
               actual, forecast, previous, surprise
        FROM eurusd_ec_today
        ORDER BY timestamp_utc
    ";

    let mut stmt = match db.prepare(query) {
        Ok(s) => s,
        Err(_) => return vec!["EC today table not found".to_string()],
    };

    let now_utc = Utc::now().naive_utc();
    let thirty_min_ago = now_utc - chrono::Duration::minutes(30);

    let mut past_lines = Vec::new();
    let mut present_lines = Vec::new();
    let mut future_lines = Vec::new();

    let rows = stmt.query_map([], |row| {
        let ts: String = row.get(0)?;
        let cur: String = row.get(1)?;
        let vol: i32 = row.get(2)?;
        let name: String = row.get(3)?;
        let actual: Option<f64> = row.get(4)?;
        let forecast: Option<f64> = row.get(5)?;
        let previous: Option<f64> = row.get(6)?;
        let surprise: Option<f64> = row.get(7)?;
        Ok((ts, cur, vol, name, actual, forecast, previous, surprise))
    });

    if let Ok(rows) = rows {
        for row in rows.flatten() {
            let (ts, cur, vol, name, actual, forecast, previous, surprise) = row;

            let time_str = if ts.len() >= 16 { &ts[11..16] } else { &ts };
            let vol_stars = match vol {
                3 => "***",
                2 => "** ",
                _ => "*  ",
            };

            let event_time = NaiveDateTime::parse_from_str(&ts, "%Y-%m-%dT%H:%M:%S").ok();
            let short_name = if name.len() > 38 { &name[..38] } else { &name };

            let is_past = event_time.map(|et| et < now_utc).unwrap_or(false);
            let is_recent = event_time.map(|et| et >= thirty_min_ago && et <= now_utc).unwrap_or(false);

            if is_past {
                // Past or just released
                let a_str = actual.map(|v| format!("{:.2}", v)).unwrap_or_else(|| "--".to_string());
                let f_str = forecast.map(|v| format!("{:.2}", v)).unwrap_or_else(|| "--".to_string());

                let surprise_str = if let Some(s) = surprise {
                    if s > 0.0 { format!(" [+{:.2}]", s) }
                    else if s < 0.0 { format!(" [{:.2}]", s) }
                    else { " [=]".to_string() }
                } else {
                    String::new()
                };

                let line = format!(
                    "  {} {} {} A:{} F:{}{}  {}",
                    time_str, cur, vol_stars, a_str, f_str, surprise_str, short_name
                );

                if is_recent {
                    present_lines.push(line);
                } else {
                    past_lines.push(line);
                }
            } else {
                // Upcoming event
                let f_str = forecast.map(|v| format!("{:.2}", v)).unwrap_or_else(|| "--".to_string());
                let p_str = previous.map(|v| format!("{:.2}", v)).unwrap_or_else(|| "--".to_string());

                let countdown = if let Some(et) = event_time {
                    let diff = et - now_utc;
                    let mins = diff.num_minutes();
                    if mins > 60 {
                        format!(" (in {}h{}m)", mins / 60, mins % 60)
                    } else if mins > 0 {
                        format!(" (in {}m)", mins)
                    } else {
                        " (NOW)".to_string()
                    }
                } else {
                    String::new()
                };

                let line = format!(
                    "  {} {} {} F:{} P:{}  {}{}",
                    time_str, cur, vol_stars, f_str, p_str, short_name, countdown
                );
                future_lines.push(line);
            }
        }
    }

    let mut lines = Vec::new();

    if !future_lines.is_empty() {
        lines.push(format!("--- UPCOMING ({}) ---", future_lines.len()));
        lines.extend(future_lines);
    }

    if !present_lines.is_empty() {
        lines.push(format!("--- JUST RELEASED ({}) ---", present_lines.len()));
        lines.extend(present_lines);
    }

    if !past_lines.is_empty() {
        lines.push(format!("--- PAST ({}) ---", past_lines.len()));
        lines.extend(past_lines);
    }

    if lines.is_empty() {
        lines.push("No EC events for today".to_string());
    }
    lines
}

// ── Gold-focused historical EC table ──────────────────────────────────────────
//
// Sibling of `eurusd_economic_calendar` filtered to currencies that move
// XAUUSD: USD (Fed/inflation/jobs), the major DXY components
// (EUR/GBP/JPY/CHF), AUD (#2 gold producer, AUD/gold correlation), and CNY
// (#1 physical gold consumer). All impact levels (low + medium + high) are
// kept — even low-impact prints sometimes shift gold via flow effects, and
// the full set is useful for downstream feature engineering.

pub const XAUUSD_EC_TABLE: &str = "xauusd_economic_calendar";
pub const XAUUSD_EC_CURRENCIES: &[&str] = &["USD", "EUR", "GBP", "JPY", "CHF", "AUD", "CNY"];

const CREATE_XAUUSD_EC: &str = "
CREATE TABLE IF NOT EXISTS xauusd_economic_calendar (
    event_date_id   VARCHAR PRIMARY KEY,
    event_id        VARCHAR NOT NULL,
    event_name      VARCHAR NOT NULL,
    currency        VARCHAR NOT NULL,
    country_code    VARCHAR NOT NULL,
    volatility      TINYINT NOT NULL,
    timestamp_utc   VARCHAR NOT NULL,
    weekday         TINYINT NOT NULL,
    hour_utc        TINYINT NOT NULL,
    actual_raw      VARCHAR,
    forecast_raw    VARCHAR,
    previous_raw    VARCHAR,
    actual          DOUBLE,
    forecast        DOUBLE,
    previous        DOUBLE,
    surprise        DOUBLE,
    beats_forecast  TINYINT,
    unit            VARCHAR
)
";

/// Create the gold-EC table if it doesn't exist. Idempotent.
pub fn create_xauusd_ec_table(db: &duckdb::Connection) -> Result<(), String> {
    db.execute_batch(CREATE_XAUUSD_EC).map_err(|e| format!("create xauusd_economic_calendar: {}", e))
}

/// Filter a batch of EcRows down to the gold-relevant currency set
/// (USD/EUR/GBP/JPY/CHF/AUD/CNY — every impact level kept) and upsert
/// them into `xauusd_economic_calendar`. Returns rows actually written.
pub fn upsert_xauusd_ec(db: &duckdb::Connection, rows: &[EcRow]) -> Result<usize, String> {
    create_xauusd_ec_table(db)?;

    let upsert = "
        INSERT INTO xauusd_economic_calendar
            (event_date_id, event_id, event_name, currency, country_code,
             volatility, timestamp_utc, weekday, hour_utc,
             actual_raw, forecast_raw, previous_raw,
             actual, forecast, previous, surprise, beats_forecast, unit)
        VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
        ON CONFLICT (event_date_id) DO UPDATE SET
            actual_raw     = EXCLUDED.actual_raw,
            forecast_raw   = EXCLUDED.forecast_raw,
            previous_raw   = EXCLUDED.previous_raw,
            actual         = EXCLUDED.actual,
            forecast       = EXCLUDED.forecast,
            previous       = EXCLUDED.previous,
            surprise       = EXCLUDED.surprise,
            beats_forecast = EXCLUDED.beats_forecast
    ";

    let mut written = 0usize;
    for r in rows.iter().filter(|r| XAUUSD_EC_CURRENCIES.contains(&r.currency.as_str())) {
        let params = duckdb::params![
            r.event_date_id,
            r.event_id,
            r.event_name,
            r.currency,
            r.country_code,
            r.volatility as i32,
            r.timestamp_utc,
            r.weekday as i32,
            r.hour_utc as i32,
            r.actual_raw,
            r.forecast_raw,
            r.previous_raw,
            r.actual,
            r.forecast,
            r.previous,
            r.surprise,
            r.beats_forecast.map(|v| v as i32),
            None::<String>,
        ];
        if db.execute(upsert, params).is_ok() {
            written += 1;
        }
    }
    Ok(written)
}
