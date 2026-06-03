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
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
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
                    return Ok(());
                }
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
