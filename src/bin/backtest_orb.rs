//! EURUSD ORB backtest harness — Phase 1.
//!
//! Subcommands:
//!   download   Pull historical M1 bars from cTrader and save them to a CSV
//!              research file (offline; never touches the live app path).
//!
//! Usage:
//!   cargo run --bin backtest_orb -- download [--symbol EURUSD]
//!                                            [--from YYYY-MM-DD] [--to YYYY-MM-DD]
//!                                            [--out backtest_data/eurusd_m1.csv]
//!
//! Defaults: symbol EURUSD, the last 365 days, out = backtest_data/<symbol>_m1.csv.
//! Requires the same .env as the main app.
//!
//! The simulator (`run` subcommand) lands in the next increment and will read the
//! CSV produced here through the shared `strategy::orb` rule.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use chrono::{Datelike, NaiveDate, TimeZone, Utc};
use prost::Message;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::rustls::{ClientConfig, OwnedTrustAnchor, RootCertStore, ServerName};
use tokio_rustls::TlsConnector;

// Reuse the project's compiled proto types without going through lib.rs.
mod openapi {
    include!("../proto/generated/_.rs");
}

// The pure ORB rule — the SAME source file the live bot uses (via
// `strategy::orb` in main.rs) — plus the simulator built around it.
#[path = "../strategy/orb.rs"]
mod orb;
#[path = "../backtest/sim.rs"]
mod sim;

type Tls = tokio_rustls::client::TlsStream<TcpStream>;
type BoxErr = Box<dyn std::error::Error + Send + Sync>;

/// One decoded M1 bar (timestamp in UTC seconds, prices as floats).
#[derive(Clone, Copy)]
struct Bar {
    ts: i64,
    o: f64,
    h: f64,
    l: f64,
    c: f64,
    v: i64,
}

async fn write_proto<T: Message>(stream: &mut Tls, payload_type: u32, payload: T) -> Result<(), BoxErr> {
    let mut body = Vec::new();
    payload.encode(&mut body)?;
    let msg = openapi::ProtoMessage { payload_type, payload: Some(body), client_msg_id: None };
    let mut full = Vec::new();
    msg.encode(&mut full)?;
    let len = (full.len() as u32).to_be_bytes();
    stream.write_all(&len).await?;
    stream.write_all(&full).await?;
    Ok(())
}

async fn read_proto(stream: &mut Tls) -> Result<openapi::ProtoMessage, BoxErr> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;
    Ok(openapi::ProtoMessage::decode(buf.as_slice())?)
}

/// Read frames until one with `want` payload_type arrives (ignoring heartbeats
/// etc.), or a `ProtoOAErrorRes` (2142) is seen. 20s per-read timeout.
async fn read_until(stream: &mut Tls, want: u32) -> Result<openapi::ProtoMessage, BoxErr> {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(20), read_proto(stream)).await??;
        if msg.payload_type == want {
            return Ok(msg);
        }
        if msg.payload_type == 2142 {
            if let Some(p) = &msg.payload {
                let e = openapi::ProtoOaErrorRes::decode(p.as_slice())?;
                return Err(format!("broker error: {} - {}", e.error_code, e.description.unwrap_or_default()).into());
            }
        }
        // anything else (heartbeats, spot events, …) — keep reading.
    }
}

/// Connect + TLS to the live endpoint.
async fn connect() -> Result<Tls, BoxErr> {
    let host = "live.ctraderapi.com";
    let port = 5035;
    let mut roots = RootCertStore::empty();
    roots.add_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.iter().map(|ta| {
        OwnedTrustAnchor::from_subject_spki_name_constraints(ta.subject, ta.spki, ta.name_constraints)
    }));
    let cfg = ClientConfig::builder()
        .with_safe_defaults()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(cfg));
    let tcp = TcpStream::connect((host, port)).await?;
    let tls = connector.connect(ServerName::try_from(host)?, tcp).await?;
    eprintln!("[backtest] connected to {}:{}", host, port);
    Ok(tls)
}

/// App auth → account auth → symbols list; return the requested symbol's id.
async fn auth_and_resolve(
    tls: &mut Tls,
    client_id: String,
    client_secret: String,
    access_token: String,
    account_id: i64,
    symbol: &str,
) -> Result<i64, BoxErr> {
    write_proto(tls, openapi::ProtoOaPayloadType::ProtoOaApplicationAuthReq as u32,
        openapi::ProtoOaApplicationAuthReq {
            payload_type: Some(openapi::ProtoOaPayloadType::ProtoOaApplicationAuthReq as i32),
            client_id, client_secret,
        }).await?;
    read_until(tls, 2101).await?; // ApplicationAuthRes
    eprintln!("[backtest] app authed");

    write_proto(tls, openapi::ProtoOaPayloadType::ProtoOaAccountAuthReq as u32,
        openapi::ProtoOaAccountAuthReq {
            payload_type: Some(openapi::ProtoOaPayloadType::ProtoOaAccountAuthReq as i32),
            ctid_trader_account_id: account_id,
            access_token,
        }).await?;
    read_until(tls, 2103).await?; // AccountAuthRes
    eprintln!("[backtest] account authed");

    write_proto(tls, openapi::ProtoOaPayloadType::ProtoOaSymbolsListReq as u32,
        openapi::ProtoOaSymbolsListReq {
            payload_type: Some(openapi::ProtoOaPayloadType::ProtoOaSymbolsListReq as i32),
            ctid_trader_account_id: account_id,
            include_archived_symbols: Some(false),
        }).await?;
    let msg = read_until(tls, 2115).await?; // SymbolsListRes
    let res = openapi::ProtoOaSymbolsListRes::decode(msg.payload.unwrap_or_default().as_slice())?;
    let id = res.symbol.iter()
        .find(|s| s.symbol_name.as_deref() == Some(symbol))
        .map(|s| s.symbol_id)
        .ok_or_else(|| format!("symbol '{}' not found in account inventory", symbol))?;
    eprintln!("[backtest] {} symbol_id = {}", symbol, id);
    Ok(id)
}

/// Fetch one chunk of M1 trendbars in [from_ms, to_ms]. Returns decoded bars
/// (ascending order not guaranteed; the caller dedups + sorts).
async fn fetch_chunk(tls: &mut Tls, account_id: i64, symbol_id: i64, from_ms: i64, to_ms: i64) -> Result<Vec<Bar>, BoxErr> {
    write_proto(tls, openapi::ProtoOaPayloadType::ProtoOaGetTrendbarsReq as u32,
        openapi::ProtoOaGetTrendbarsReq {
            payload_type: Some(openapi::ProtoOaPayloadType::ProtoOaGetTrendbarsReq as i32),
            ctid_trader_account_id: account_id,
            from_timestamp: Some(from_ms),
            to_timestamp: Some(to_ms),
            period: openapi::ProtoOaTrendbarPeriod::M1 as i32,
            symbol_id,
            count: Some(10_000),
        }).await?;
    let msg = read_until(tls, 2138).await?; // GetTrendbarsRes
    let res = openapi::ProtoOaGetTrendbarsRes::decode(msg.payload.unwrap_or_default().as_slice())?;

    let div = 100_000.0;
    let mut bars = Vec::with_capacity(res.trendbar.len());
    for tb in &res.trendbar {
        let (Some(low_raw), Some(ts_min)) = (tb.low, tb.utc_timestamp_in_minutes) else { continue };
        let low = low_raw as f64;
        bars.push(Bar {
            ts: ts_min as i64 * 60,
            o: (low + tb.delta_open.unwrap_or(0) as f64) / div,
            h: (low + tb.delta_high.unwrap_or(0) as f64) / div,
            l: low / div,
            c: (low + tb.delta_close.unwrap_or(0) as f64) / div,
            v: tb.volume,
        });
    }
    Ok(bars)
}

/// Page M1 bars backward from `to_ms` to `from_ms`, robust to the server's
/// per-request cap: each step advances to just before the oldest bar received,
/// so partial responses are handled correctly.
async fn download_range(tls: &mut Tls, account_id: i64, symbol_id: i64, from_ms: i64, to_ms: i64)
    -> Result<BTreeMap<i64, Bar>, BoxErr>
{
    const WINDOW_MS: i64 = 5 * 24 * 3600 * 1000; // 5-day request span
    let mut out: BTreeMap<i64, Bar> = BTreeMap::new();
    let mut cursor_to = to_ms;

    while cursor_to > from_ms {
        let window_from = (cursor_to - WINDOW_MS).max(from_ms);
        let bars = fetch_chunk(tls, account_id, symbol_id, window_from, cursor_to).await?;

        if bars.is_empty() {
            // No data in this window (deep past / gap) — step back a full window.
            cursor_to = window_from - 60_000;
            continue;
        }

        let oldest_ms = bars.iter().map(|b| b.ts * 1000).min().unwrap();
        for b in bars {
            out.insert(b.ts, b);
        }

        let next = oldest_ms - 60_000;
        if next >= cursor_to {
            break; // no forward progress — avoid an infinite loop
        }
        cursor_to = next;

        if let Some(d) = chrono::DateTime::from_timestamp_millis(oldest_ms) {
            eprint!("\r[backtest] {} bars  (back to {})        ", out.len(), d.format("%Y-%m-%d %H:%M"));
            let _ = std::io::stderr().flush();
        }
        tokio::time::sleep(Duration::from_millis(250)).await; // be gentle on rate limits
    }
    eprintln!();
    Ok(out)
}

/// Write bars to CSV: `timestamp,open,high,low,close,volume`, ascending.
fn write_csv(bars: &BTreeMap<i64, Bar>, path: &str) -> Result<(), BoxErr> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let f = std::fs::File::create(path)?;
    let mut w = std::io::BufWriter::new(f);
    writeln!(w, "timestamp,open,high,low,close,volume")?;
    for b in bars.values() {
        writeln!(w, "{},{:.5},{:.5},{:.5},{:.5},{}", b.ts, b.o, b.h, b.l, b.c, b.v)?;
    }
    w.flush()?;
    Ok(())
}

/// Small flag reader: `--name value`.
fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn parse_date_to_ms(s: &str, end_of_day: bool) -> Result<i64, BoxErr> {
    let d = NaiveDate::parse_from_str(s, "%Y-%m-%d")?;
    let secs = if end_of_day { 23 * 3600 + 59 * 60 + 59 } else { 0 };
    let dt = Utc.with_ymd_and_hms(d.year(), d.month(), d.day(), 0, 0, 0).unwrap()
        + chrono::Duration::seconds(secs);
    Ok(dt.timestamp_millis())
}

async fn run_download(args: &[String]) -> Result<(), BoxErr> {
    dotenv::dotenv().ok();
    let client_id = std::env::var("CTRADER_CLIENT_ID")?;
    let client_secret = std::env::var("CTRADER_SECRET")?;
    let access_token = std::env::var("CTRADER_ACCESS_TOKEN")?;
    let account_id: i64 = std::env::var("CTRADER_ACCOUNT_ID")?.trim().parse()?;

    let symbol = flag(args, "--symbol").unwrap_or_else(|| "EURUSD".to_string());
    let now = Utc::now();
    let to_ms = match flag(args, "--to") {
        Some(s) => parse_date_to_ms(&s, true)?,
        None => now.timestamp_millis(),
    };
    let from_ms = match flag(args, "--from") {
        Some(s) => parse_date_to_ms(&s, false)?,
        None => (now - chrono::Duration::days(365)).timestamp_millis(),
    };
    let out = flag(args, "--out").unwrap_or_else(|| format!("backtest_data/{}_m1.csv", symbol.to_lowercase()));

    eprintln!("[backtest] downloading {} M1  {} → {}  → {}",
        symbol,
        chrono::DateTime::from_timestamp_millis(from_ms).unwrap().format("%Y-%m-%d"),
        chrono::DateTime::from_timestamp_millis(to_ms).unwrap().format("%Y-%m-%d"),
        out);

    let mut tls = connect().await?;
    let symbol_id = auth_and_resolve(&mut tls, client_id, client_secret, access_token, account_id, &symbol).await?;
    let bars = download_range(&mut tls, account_id, symbol_id, from_ms, to_ms).await?;

    if bars.is_empty() {
        return Err("no bars returned — check the date range / market data access".into());
    }
    write_csv(&bars, &out)?;
    let first = bars.values().next().unwrap();
    let last = bars.values().next_back().unwrap();
    eprintln!("[backtest] wrote {} bars to {}", bars.len(), out);
    eprintln!("[backtest] span: {} → {}",
        chrono::DateTime::from_timestamp(first.ts, 0).unwrap().format("%Y-%m-%d %H:%M"),
        chrono::DateTime::from_timestamp(last.ts, 0).unwrap().format("%Y-%m-%d %H:%M"));
    Ok(())
}

/// `run` — simulate the ORB rule over a downloaded CSV and print metrics.
fn run_backtest_cmd(args: &[String]) -> Result<(), BoxErr> {
    let inp = flag(args, "--in").unwrap_or_else(|| "backtest_data/eurusd_m1.csv".to_string());
    let bars = sim::load_csv(&inp)?;
    if bars.is_empty() {
        return Err(format!("no bars in {}", inp).into());
    }

    let mut cfg = orb::OrbConfig::default();
    if let Some(v) = flag(args, "--range-min") { cfg.range_minutes = v.parse()?; }
    if let Some(v) = flag(args, "--buffer") { cfg.buffer_pips = v.parse()?; }
    if let Some(v) = flag(args, "--target-r") { cfg.target_r = v.parse()?; }
    if let Some(v) = flag(args, "--min-range") { cfg.min_range_pips = v.parse()?; }
    if let Some(v) = flag(args, "--max-range") { cfg.max_range_pips = v.parse()?; }
    if let Some(v) = flag(args, "--be-r") { cfg.breakeven_at_r = v.parse()?; }

    let mut cost = sim::CostModel::default();
    if let Some(v) = flag(args, "--spread") { cost.spread_pips = v.parse()?; }
    if let Some(v) = flag(args, "--slippage") { cost.slippage_pips = v.parse()?; }

    let mut session = sim::SessionConfig::default();
    if args.iter().any(|a| a == "--no-blackout") { session.blackout = false; }
    if let Some(v) = flag(args, "--entry-window") { session.entry_window_min = v.parse()?; }

    let (trades, counts) = sim::run_backtest(&bars, &cfg, &cost, &session);
    let metrics = sim::compute_metrics(&trades);

    let out = flag(args, "--trades-out").unwrap_or_else(|| "backtest_data/orb_trades.csv".to_string());
    sim::write_trades_csv(&trades, &out)?;

    let span = (
        chrono::DateTime::from_timestamp(bars.first().unwrap().ts, 0).unwrap().date_naive(),
        chrono::DateTime::from_timestamp(bars.last().unwrap().ts, 0).unwrap().date_naive(),
    );
    sim::print_summary(&metrics, &counts, &cfg, &cost, &session, span);
    eprintln!("[backtest] trade log → {}", out);
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), BoxErr> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("download") => run_download(&args).await,
        Some("run") => run_backtest_cmd(&args),
        other => {
            eprintln!("unknown/missing subcommand: {:?}", other);
            eprintln!("usage:");
            eprintln!("  backtest_orb download [--symbol EURUSD] [--from YYYY-MM-DD] [--to YYYY-MM-DD] [--out PATH]");
            eprintln!("  backtest_orb run [--in CSV] [--spread P] [--target-r R] [--range-min M] [--buffer P] [--be-r R] [--no-blackout]");
            Ok(())
        }
    }
}
