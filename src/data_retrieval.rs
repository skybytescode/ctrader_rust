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
/// Examples: `eurusd_m1`, `eurusd_ticks`. The name is spliced into SQL, so
/// characters other than ASCII letters and digits (as in broker symbols like
/// `US30.cash` or `#EURUSD`) become `_`.
pub fn get_data_table_name(symbol: &str, kind: DataKind) -> String {
    let sym: String = symbol
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect();
    match kind {
        DataKind::M1Candles => format!("{}_m1", sym),
        DataKind::TickData => format!("{}_ticks", sym),
        DataKind::TickDataAsk => format!("{}_ticks_ask", sym),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trendbar_prices_are_deltas_from_low() {
        // EUR/USD bar: low 1.08000, open +12, high +40, close +25 (1/100,000 units).
        let c = trendbar_to_candle(321, Some(108_000), Some(12), Some(25), Some(40), Some(29_000_000)).unwrap();
        assert_eq!(c.timestamp, 29_000_000 * 60);
        assert!((c.low - 1.08).abs() < 1e-12);
        assert!((c.open - 1.08012).abs() < 1e-12);
        assert!((c.high - 1.0804).abs() < 1e-12);
        assert!((c.close - 1.08025).abs() < 1e-12);
        assert_eq!(c.volume, 321);
    }

    #[test]
    fn trendbar_without_low_or_time_is_skipped() {
        assert!(trendbar_to_candle(1, None, Some(1), Some(1), Some(1), Some(1)).is_none());
        assert!(trendbar_to_candle(1, Some(1), Some(1), Some(1), Some(1), None).is_none());
    }

    #[test]
    fn ticks_are_delta_decoded() {
        let got = decode_tick_data(&[1_700_000_000_000, 250, 1_000], &[108_000, 3, -5]);
        assert_eq!(got, vec![
            (1_700_000_000_000, 1.08),
            (1_700_000_000_250, 1.08003),
            (1_700_000_001_250, 1.07998),
        ]);
        assert!(decode_tick_data(&[], &[]).is_empty());
    }

    #[test]
    fn table_names() {
        assert_eq!(get_data_table_name("EURUSD", DataKind::M1Candles), "eurusd_m1");
        assert_eq!(get_data_table_name("EURUSD", DataKind::TickData), "eurusd_ticks");
        assert_eq!(get_data_table_name("EURUSD", DataKind::TickDataAsk), "eurusd_ticks_ask");
        assert_eq!(get_data_table_name("US30.cash", DataKind::M1Candles), "us30_cash_m1");
        assert_eq!(get_data_table_name("x; DROP TABLE y", DataKind::M1Candles), "x__drop_table_y_m1");
    }
}
