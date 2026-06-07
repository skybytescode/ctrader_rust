//! One-shot: connect to the live cTrader OpenAPI, authenticate, request the
//! account's full symbol inventory (`ProtoOASymbolsListReq` → `…Res`), print
//! every instrument name sorted, plus a USD-index-candidates filter, and exit.
//!
//! Run with:  `cargo run --bin list_symbols`
//! Requires the same .env as the main app.

use std::sync::Arc;
use std::time::Duration;

use prost::Message;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::rustls::{ClientConfig, OwnedTrustAnchor, RootCertStore, ServerName};
use tokio_rustls::TlsConnector;

// Reuse the project's compiled proto types without going through lib.rs.
mod openapi {
    include!("../proto/generated/_.rs");
}

async fn write_proto<T: Message>(
    stream: &mut tokio_rustls::client::TlsStream<TcpStream>,
    payload_type: u32,
    payload: T,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut body = Vec::new();
    payload.encode(&mut body)?;
    let msg = openapi::ProtoMessage {
        payload_type,
        payload: Some(body),
        client_msg_id: None,
    };
    let mut full = Vec::new();
    msg.encode(&mut full)?;
    let len = (full.len() as u32).to_be_bytes();
    stream.write_all(&len).await?;
    stream.write_all(&full).await?;
    Ok(())
}

async fn read_proto(
    stream: &mut tokio_rustls::client::TlsStream<TcpStream>,
) -> Result<openapi::ProtoMessage, Box<dyn std::error::Error + Send + Sync>> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;
    Ok(openapi::ProtoMessage::decode(buf.as_slice())?)
}

/// Pretty-print a full symbol spec, focused on the cost model (commission vs
/// spread-only). Commission precise rates are scaled by 10^8 (10^5 for percent).
fn print_symbol_spec(name: &str, sym: &openapi::ProtoOaSymbol) {
    let ct = sym.commission_type;
    let ct_name = match ct {
        Some(1) => "USD_PER_MILLION_USD".to_string(),
        Some(2) => "USD_PER_LOT".to_string(),
        Some(3) => "PERCENTAGE_OF_VALUE".to_string(),
        Some(4) => "QUOTE_CCY_PER_LOT".to_string(),
        Some(o) => format!("unknown({})", o),
        None => "none".to_string(),
    };
    let precise = sym.precise_trading_commission_rate;
    let interpreted = match (ct, precise) {
        (Some(3), Some(p)) => format!("{}% of notional", p as f64 / 1e5),
        (Some(1), Some(p)) => format!("{:.2} USD per 1M USD volume", p as f64 / 1e8),
        (Some(2), Some(p)) => format!("{:.2} USD per lot", p as f64 / 1e8),
        (Some(4), Some(p)) => format!("{:.2} quote-ccy per lot", p as f64 / 1e8),
        _ => "n/a".to_string(),
    };
    let has_commission = sym.commission.unwrap_or(0) != 0
        || sym.precise_trading_commission_rate.unwrap_or(0) != 0;
    let verdict = if has_commission { "COMMISSION + (raw) SPREAD" } else { "SPREAD-ONLY (no commission)" };
    let min_c = sym.precise_min_commission.map(|p| p as f64 / 1e8);

    println!("\n  {name}");
    println!("    digits={}  pipPosition={}  lotSize={:?}  measurementUnits={:?}",
        sym.digits, sym.pip_position, sym.lot_size, sym.measurement_units);
    println!("    volume: min={:?} step={:?} max={:?}",
        sym.min_volume, sym.step_volume, sym.max_volume);
    println!("    commission: type={} raw={:?} precise={:?} -> {}",
        ct_name, sym.commission, precise, interpreted);
    println!("    minCommission: precise={:?} (~{:?}) typeId={:?} asset={:?}",
        sym.precise_min_commission, min_c, sym.min_commission_type, sym.min_commission_asset);
    println!("    >>> COST MODEL: {}", verdict);
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    dotenv::dotenv().ok();

    let client_id = std::env::var("CTRADER_CLIENT_ID")?;
    let client_secret = std::env::var("CTRADER_SECRET")?;
    let access_token = std::env::var("CTRADER_ACCESS_TOKEN")?;
    let account_id: i64 = std::env::var("CTRADER_ACCOUNT_ID")?.trim().parse()?;

    let host = "live.ctraderapi.com";
    let port = 5035;

    // TLS setup mirroring the main session.
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
    let mut tls = connector.connect(ServerName::try_from(host)?, tcp).await?;
    eprintln!("[list_symbols] connected to {}:{}", host, port);

    // 1) ApplicationAuthReq
    write_proto(
        &mut tls,
        openapi::ProtoOaPayloadType::ProtoOaApplicationAuthReq as u32,
        openapi::ProtoOaApplicationAuthReq {
            payload_type: Some(openapi::ProtoOaPayloadType::ProtoOaApplicationAuthReq as i32),
            client_id, client_secret,
        },
    ).await?;

    // 2) Read until we get the SymbolsListRes; send the chained requests inline.
    // id -> name for the symbols we later fetch full specs for (the SymbolByIdRes
    // carries no symbol_name, only the id).
    let mut id_to_name: std::collections::HashMap<i64, String> = std::collections::HashMap::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
    loop {
        let msg = tokio::time::timeout(
            deadline.saturating_duration_since(tokio::time::Instant::now()),
            read_proto(&mut tls),
        ).await??;
        match msg.payload_type {
            2101 /* ApplicationAuthRes */ => {
                eprintln!("[list_symbols] app authed; sending account auth");
                write_proto(
                    &mut tls,
                    openapi::ProtoOaPayloadType::ProtoOaAccountAuthReq as u32,
                    openapi::ProtoOaAccountAuthReq {
                        payload_type: Some(openapi::ProtoOaPayloadType::ProtoOaAccountAuthReq as i32),
                        ctid_trader_account_id: account_id,
                        access_token: access_token.clone(),
                    },
                ).await?;
            }
            2103 /* AccountAuthRes */ => {
                eprintln!("[list_symbols] account authed; requesting symbols list");
                write_proto(
                    &mut tls,
                    openapi::ProtoOaPayloadType::ProtoOaSymbolsListReq as u32,
                    openapi::ProtoOaSymbolsListReq {
                        payload_type: Some(openapi::ProtoOaPayloadType::ProtoOaSymbolsListReq as i32),
                        ctid_trader_account_id: account_id,
                        include_archived_symbols: Some(false),
                    },
                ).await?;
            }
            2115 /* SymbolsListRes */ => {
                if let Some(payload) = &msg.payload {
                    let res = openapi::ProtoOaSymbolsListRes::decode(payload.as_slice())?;
                    let total = res.symbol.len();
                    let enabled_count = res.symbol.iter().filter(|s| s.enabled == Some(true)).count();
                    println!("\n=== Broker carries {} instruments ({} enabled / visible to trader) ===", total, enabled_count);

                    // Filter enabled-only for the alphabetical dump (matches UI behavior).
                    let mut enabled_names: Vec<String> = res.symbol.iter()
                        .filter(|s| s.enabled == Some(true))
                        .filter_map(|s| s.symbol_name.clone())
                        .collect();
                    enabled_names.sort();
                    println!("\n-- Enabled (visible) symbols --");
                    for chunk in enabled_names.chunks(6) {
                        println!("  {}", chunk.join(", "));
                    }

                    // Highlight DXY / USD-index candidates with their enabled flag + description.
                    let usd_keyword = |n: &str| {
                        let u = n.to_uppercase();
                        u.contains("DXY") || u.contains("USDX") || u.contains("USDIDX")
                            || u.contains("US.DLR") || u.contains("DLR") || u.contains("DOLLAR")
                            || (u.contains("USD") && (u.contains("IDX") || u.contains("INDEX") || u.contains("BSKT")))
                    };
                    println!("\n-- USD-index candidates (full detail) --");
                    let mut found_any = false;
                    for s in &res.symbol {
                        if let Some(name) = &s.symbol_name {
                            if usd_keyword(name) {
                                found_any = true;
                                println!("  {:14}  id={:<6}  enabled={:?}  category={:?}  desc={:?}",
                                    name, s.symbol_id, s.enabled, s.symbol_category_id, s.description);
                            }
                        }
                    }
                    if !found_any { println!("  (none matched)"); }

                    // Same for the gold futures, for comparison.
                    println!("\n-- Gold futures GC* (full detail) --");
                    for s in &res.symbol {
                        if let Some(name) = &s.symbol_name {
                            if name.starts_with("GC") && name.len() <= 6 {
                                println!("  {:14}  id={:<6}  enabled={:?}  category={:?}  desc={:?}",
                                    name, s.symbol_id, s.enabled, s.symbol_category_id, s.description);
                            }
                        }
                    }

                    // List how many are returned but disabled (UI-hidden).
                    let disabled: Vec<&String> = res.symbol.iter()
                        .filter(|s| s.enabled != Some(true))
                        .filter_map(|s| s.symbol_name.as_ref()).collect();
                    println!("\n-- {} symbols returned by API but NOT enabled (hidden in UI) --", disabled.len());
                    if disabled.len() <= 60 {
                        for chunk in disabled.chunks(6) {
                            let chunk_strs: Vec<&str> = chunk.iter().map(|s| s.as_str()).collect();
                            println!("  {}", chunk_strs.join(", "));
                        }
                    } else {
                        println!("  (too many to list; first 30: {:?})",
                            disabled.iter().take(30).collect::<Vec<_>>());
                    }

                    // Fetch the FULL spec (incl. commission) for the pair we trade,
                    // to determine the cost model (commission + spread vs spread-only).
                    let wanted = ["EURUSD", "XAUUSD", "XRPUSD"];
                    let mut ids = Vec::new();
                    for s in &res.symbol {
                        if let Some(n) = &s.symbol_name {
                            if wanted.contains(&n.as_str()) {
                                id_to_name.insert(s.symbol_id, n.clone());
                                ids.push(s.symbol_id);
                            }
                        }
                    }
                    if ids.is_empty() {
                        println!("\n(EURUSD / XAUUSD not found for spec lookup)");
                        return Ok(());
                    }
                    println!("\n-- Requesting full spec (commission) for {:?} --", wanted);
                    write_proto(
                        &mut tls,
                        openapi::ProtoOaPayloadType::ProtoOaSymbolByIdReq as u32,
                        openapi::ProtoOaSymbolByIdReq {
                            payload_type: Some(openapi::ProtoOaPayloadType::ProtoOaSymbolByIdReq as i32),
                            ctid_trader_account_id: account_id,
                            symbol_id: ids,
                        },
                    ).await?;
                    // Don't return — wait for the SymbolByIdRes (2117) handler below.
                }
            }
            2117 /* SymbolByIdRes */ => {
                if let Some(payload) = &msg.payload {
                    let res = openapi::ProtoOaSymbolByIdRes::decode(payload.as_slice())?;
                    println!("\n=== Full symbol specs — cost model ===");
                    for sym in &res.symbol {
                        let name = id_to_name.get(&sym.symbol_id).cloned()
                            .unwrap_or_else(|| format!("id {}", sym.symbol_id));
                        print_symbol_spec(&name, sym);
                    }
                }
                return Ok(());
            }
            2142 /* ErrorRes */ => {
                if let Some(payload) = &msg.payload {
                    let err = openapi::ProtoOaErrorRes::decode(payload.as_slice())?;
                    eprintln!("[list_symbols] ErrorRes: {} - {}",
                              err.error_code, err.description.unwrap_or_default());
                }
                return Err("broker returned an error before SymbolsListRes".into());
            }
            other => {
                eprintln!("[list_symbols] (ignoring payload_type {})", other);
            }
        }
    }
}
