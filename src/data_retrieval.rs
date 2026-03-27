//! Shared types and conversion functions for the data retrieval pipeline.
//!
//! Defines the request/response messages that flow between the Bevy UI
//! and the async network task for historical data downloads.

use std::collections::HashMap;
use crate::db::Candle;

// ============================================================================
// Enums
// ============================================================================

/// What kind of historical data to retrieve
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataKind {
    M1Candles,
    TickData,       // bid ticks (table: {symbol}_ticks)
    TickDataAsk,    // ask ticks (table: {symbol}_ticks_ask)
}

/// Action requested by the UI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataAction {
    /// Check if data exists in DB, report status
    CheckStatus,
    /// Retrieve full history from cTrader API
    RetrieveFull,
    /// Update: fetch only data newer than what's already in DB
    UpdateLatest,
    /// Merge bid+ask tick tables into a combined table via ASOF JOIN
    MergeBidAsk,
    /// Build ML feature table (tick features joined with M1 candles)
    BuildMLFeatures,
    /// Start capturing live DoM depth quotes to DuckDB
    DomCaptureStart,
    /// Pause/stop DoM depth quote capture
    DomCaptureStop,
    /// Start EC Calendar live capture
    EcCaptureStart,
    /// Stop EC Calendar live capture
    EcCaptureStop,
    /// Start News live capture
    NewsCaptureStart,
    /// Stop News live capture
    NewsCaptureStop,
}

// ============================================================================
// Request / Response
// ============================================================================

/// Request sent from Bevy UI -> async network task
#[derive(Debug, Clone)]
pub struct DataRequest {
    pub symbol: String,
    pub symbol_id: i64,
    pub kind: DataKind,
    pub action: DataAction,
    /// If true, bypass the "already exists" check in MergeBidAsk / BuildMLFeatures
    /// and always rebuild. Set to true by Update History after downloading new ticks.
    pub force_rebuild: bool,
}

/// Response sent from async network task -> Bevy UI
#[derive(Debug, Clone)]
pub enum DataResponse {
    /// DB check: data exists
    StatusFound {
        symbol: String,
        kind: DataKind,
        count: i64,
        oldest_ts: i64,
        newest_ts: i64,
        first_record: String,
        last_record: String,
    },
    /// DB check: no data
    StatusEmpty {
        symbol: String,
        kind: DataKind,
    },
    /// Download progress
    Progress {
        symbol: String,
        kind: DataKind,
        downloaded_rows: u64,
        message: String,
    },
    /// Download finished
    Complete {
        symbol: String,
        kind: DataKind,
        total_rows: u64,
    },
    /// Error
    Error {
        symbol: String,
        kind: DataKind,
        message: String,
    },
    /// Bid+Ask merge completed (or already existed)
    MergeComplete {
        symbol: String,
        total_rows: u64,
        /// Rows added during an incremental update (0 for first build / already_exists)
        new_rows: u64,
        first_record: String,
        last_record: String,
        /// true if the merged table already existed (no re-merge was done)
        already_exists: bool,
    },
    /// ML feature table build completed (or already existed)
    MLFeaturesComplete {
        symbol: String,
        total_rows: u64,
        /// Rows added during an incremental update (0 for first build / already_exists)
        new_rows: u64,
        already_exists: bool,
        first_record: String,
        last_record: String,
    },
}

// ============================================================================
// Bevy Resources
// ============================================================================

/// Send data requests from UI -> network task
#[derive(Clone)]
pub struct DataRequestSender {
    pub sender: tokio::sync::mpsc::Sender<DataRequest>,
}

/// Receive data responses in UI
pub struct DataResponseReceiver {
    pub receiver: tokio::sync::mpsc::Receiver<DataResponse>,
}

/// Maps symbol names to cTrader symbol IDs
#[derive(Default)]
pub struct SymbolIdMap {
    pub name_to_id: HashMap<String, i64>,
}

// ============================================================================
// Conversion Functions
// ============================================================================

/// Convert a ProtoOaTrendbar to a Candle.
///
/// Price encoding: all prices are integers in 1/100,000 units.
///   low is the base price; open = low + delta_open, etc.
///
/// Timestamp: `utc_timestamp_in_minutes * 60` → unix seconds.
pub fn trendbar_to_candle(
    volume: i64,
    low: Option<i64>,
    delta_open: Option<u64>,
    delta_close: Option<u64>,
    delta_high: Option<u64>,
    utc_timestamp_in_minutes: Option<u32>,
) -> Option<Candle> {
    let low_raw = low? as f64;
    let d_open = delta_open.unwrap_or(0) as f64;
    let d_close = delta_close.unwrap_or(0) as f64;
    let d_high = delta_high.unwrap_or(0) as f64;
    let ts_minutes = utc_timestamp_in_minutes? as i64;

    let divisor = 100_000.0;
    Some(Candle {
        timestamp: ts_minutes * 60,
        open: (low_raw + d_open) / divisor,
        high: (low_raw + d_high) / divisor,
        low: low_raw / divisor,
        close: (low_raw + d_close) / divisor,
        volume,
    })
}

/// Decode delta-encoded tick data from cTrader API.
///
/// Both timestamps and prices are delta-encoded:
///   - First entry: absolute timestamp (ms) and absolute price (in 1/100,000 units)
///   - Subsequent entries: delta from previous value
///
/// Returns `(timestamp_ms, price)` pairs.
pub fn decode_tick_data(
    timestamps: &[i64],
    ticks: &[i64],
) -> Vec<(i64, f64)> {
    let divisor = 100_000.0;
    let mut result = Vec::with_capacity(timestamps.len());

    if timestamps.is_empty() {
        return result;
    }

    let mut current_ts_ms = timestamps[0];
    let mut current_tick = ticks.first().copied().unwrap_or(0);
    result.push((current_ts_ms, current_tick as f64 / divisor));

    for i in 1..timestamps.len() {
        current_ts_ms += timestamps[i]; // delta timestamp
        current_tick += ticks.get(i).copied().unwrap_or(0); // delta price
        result.push((current_ts_ms, current_tick as f64 / divisor));
    }

    result
}

/// Generate a DuckDB table name from symbol + data kind.
///
/// Examples: `eurusd_m1`, `eurusd_ticks`
pub fn get_data_table_name(symbol: &str, kind: DataKind) -> String {
    let sym = symbol.to_lowercase();
    match kind {
        DataKind::M1Candles => format!("{}_m1", sym),
        DataKind::TickData => format!("{}_ticks", sym),
        DataKind::TickDataAsk => format!("{}_ticks_ask", sym),
    }
}
