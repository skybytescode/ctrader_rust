use super::Candle;
use duckdb::{Connection, Result, params};
use std::path::Path;

pub struct CandleDatabase {
    conn: Connection,
}

impl CandleDatabase {
    pub fn new<P: AsRef<Path>>(db_path: P) -> Result<Self> {
        let conn = Connection::open(db_path)?;
        Ok(Self { conn })
    }


    pub fn create_table_if_not_exists(&self, table_name: &str) -> Result<()> {
        // Create table if it doesn't exist
        self.conn.execute(
            &format!(
                "CREATE TABLE IF NOT EXISTS {} (
                    timestamp BIGINT PRIMARY KEY,
                    open DOUBLE,
                    high DOUBLE,
                    low DOUBLE,
                    close DOUBLE,
                    volume BIGINT
                )",
                table_name
            ),
            [],
        )?;

        // DuckDB automatically creates indexes for PRIMARY KEY
        // No need for explicit index creation like in SQLite
        Ok(())
    }

    /// Query candles in a time range
    pub fn get_candles(&self, table_name: &str, start_ts: i64, end_ts: i64) -> Result<Vec<Candle>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT timestamp, open, high, low, close, volume
             FROM {}
             WHERE timestamp >= ? AND timestamp <= ?
             ORDER BY timestamp ASC",
            table_name
        ))?;

        let candle_iter = stmt.query_map([start_ts, end_ts], |row| {
            Ok(Candle {
                timestamp: row.get(0)?,
                open: row.get(1)?,
                high: row.get(2)?,
                low: row.get(3)?,
                close: row.get(4)?,
                volume: row.get(5)?,
            })
        })?;

        candle_iter.collect()
    }

    /// Get last N candles
    pub fn get_last_candles(&self, table_name: &str, count: usize) -> Result<Vec<Candle>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT timestamp, open, high, low, close, volume
             FROM {}
             ORDER BY timestamp DESC
             LIMIT ?",
            table_name
        ))?;

        let candle_iter = stmt.query_map([count as i64], |row| {
            Ok(Candle {
                timestamp: row.get(0)?,
                open: row.get(1)?,
                high: row.get(2)?,
                low: row.get(3)?,
                close: row.get(4)?,
                volume: row.get(5)?,
            })
        })?;

        let mut candles: Vec<Candle> = candle_iter.collect::<Result<Vec<_>>>()?;
        candles.reverse(); // Return in chronological order
        Ok(candles)
    }

    /// Insert a single candle
    pub fn insert_candle(&self, table_name: &str, candle: &Candle) -> Result<()> {
        self.conn.execute(
            &format!(
                "INSERT OR REPLACE INTO {} (timestamp, open, high, low, close, volume)
                 VALUES (?, ?, ?, ?, ?, ?)",
                table_name
            ),
            params![candle.timestamp, candle.open, candle.high, candle.low, candle.close, candle.volume],
        )?;
        Ok(())
    }

    /// Batch insert candles (more efficient)
    pub fn insert_candles(&mut self, table_name: &str, candles: &[Candle]) -> Result<()> {
        let tx = self.conn.transaction()?;

        for candle in candles {
            tx.execute(
                &format!(
                    "INSERT OR REPLACE INTO {} (timestamp, open, high, low, close, volume)
                     VALUES (?, ?, ?, ?, ?, ?)",
                    table_name
                ),
                params![candle.timestamp, candle.open, candle.high, candle.low, candle.close, candle.volume],
            )?;
        }

        tx.commit()?;
        Ok(())
    }

    /// Get count of candles in database
    pub fn count_candles(&self, table_name: &str) -> Result<i64> {
        let count: i64 = self.conn.query_row(
            &format!("SELECT COUNT(*) FROM {}", table_name),
            [],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// Get ALL candles from the table (for loading complete historical data)
    pub fn get_all_candles(&self, table_name: &str) -> Result<Vec<Candle>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT timestamp, open, high, low, close, volume
             FROM {}
             ORDER BY timestamp ASC",
            table_name
        ))?;

        let candle_iter = stmt.query_map([], |row| {
            Ok(Candle {
                timestamp: row.get(0)?,
                open: row.get(1)?,
                high: row.get(2)?,
                low: row.get(3)?,
                close: row.get(4)?,
                volume: row.get(5)?,
            })
        })?;

        candle_iter.collect()
    }

    /// Get N candles before a specific timestamp (for lazy loading older data)
    pub fn get_candles_before(&self, table_name: &str, before_timestamp: i64, count: usize) -> Result<Vec<Candle>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT timestamp, open, high, low, close, volume
             FROM {}
             WHERE timestamp < ?
             ORDER BY timestamp DESC
             LIMIT ?",
            table_name
        ))?;

        let candle_iter = stmt.query_map([before_timestamp, count as i64], |row| {
            Ok(Candle {
                timestamp: row.get(0)?,
                open: row.get(1)?,
                high: row.get(2)?,
                low: row.get(3)?,
                close: row.get(4)?,
                volume: row.get(5)?,
            })
        })?;

        let mut candles: Vec<Candle> = candle_iter.collect::<Result<Vec<_>>>()?;
        candles.reverse(); // Return in chronological order (oldest first)
        Ok(candles)
    }

    // ========================================================================
    // Tick data methods
    // ========================================================================

    /// Create a tick data table (no PK — multiple ticks can share the same millisecond)
    pub fn create_tick_table_if_not_exists(&self, table_name: &str) -> Result<()> {
        self.conn.execute(
            &format!(
                "CREATE TABLE IF NOT EXISTS {} (
                    timestamp_ms BIGINT,
                    bid DOUBLE
                )",
                table_name
            ),
            [],
        )?;
        Ok(())
    }

    /// Batch insert ticks: (timestamp_ms, bid_price)
    pub fn insert_ticks(&mut self, table_name: &str, ticks: &[(i64, f64)]) -> Result<()> {
        let tx = self.conn.transaction()?;
        for (ts_ms, price) in ticks {
            tx.execute(
                &format!(
                    "INSERT OR REPLACE INTO {} (timestamp_ms, bid) VALUES (?, ?)",
                    table_name
                ),
                params![ts_ms, price],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Count ticks in a tick table
    pub fn count_ticks(&self, table_name: &str) -> Result<i64> {
        let count: i64 = self.conn.query_row(
            &format!("SELECT COUNT(*) FROM {}", table_name),
            [],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// Get (min, max) timestamp_ms from a tick table
    pub fn get_tick_date_range(&self, table_name: &str) -> Result<(i64, i64)> {
        self.conn.query_row(
            &format!("SELECT MIN(timestamp_ms), MAX(timestamp_ms) FROM {}", table_name),
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
    }

    /// Get (min, max) timestamp from a candle table
    pub fn get_candle_date_range(&self, table_name: &str) -> Result<(i64, i64)> {
        self.conn.query_row(
            &format!("SELECT MIN(timestamp), MAX(timestamp) FROM {}", table_name),
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
    }

    /// Get the first (oldest) candle in a table
    pub fn get_first_candle(&self, table_name: &str) -> Result<Option<Candle>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT timestamp, open, high, low, close, volume
             FROM {} ORDER BY timestamp ASC LIMIT 1",
            table_name
        ))?;
        let mut rows = stmt.query_map([], |row| {
            Ok(Candle {
                timestamp: row.get(0)?,
                open: row.get(1)?,
                high: row.get(2)?,
                low: row.get(3)?,
                close: row.get(4)?,
                volume: row.get(5)?,
            })
        })?;
        match rows.next() {
            Some(Ok(c)) => Ok(Some(c)),
            _ => Ok(None),
        }
    }

    /// Get the last (newest) candle in a table
    pub fn get_last_candle(&self, table_name: &str) -> Result<Option<Candle>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT timestamp, open, high, low, close, volume
             FROM {} ORDER BY timestamp DESC LIMIT 1",
            table_name
        ))?;
        let mut rows = stmt.query_map([], |row| {
            Ok(Candle {
                timestamp: row.get(0)?,
                open: row.get(1)?,
                high: row.get(2)?,
                low: row.get(3)?,
                close: row.get(4)?,
                volume: row.get(5)?,
            })
        })?;
        match rows.next() {
            Some(Ok(c)) => Ok(Some(c)),
            _ => Ok(None),
        }
    }

    /// Get the first (oldest) tick: (timestamp_ms, bid)
    pub fn get_first_tick(&self, table_name: &str) -> Result<Option<(i64, f64)>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT timestamp_ms, bid FROM {} ORDER BY timestamp_ms ASC LIMIT 1",
            table_name
        ))?;
        let mut rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
        })?;
        match rows.next() {
            Some(Ok(t)) => Ok(Some(t)),
            _ => Ok(None),
        }
    }

    /// Get the last (newest) tick: (timestamp_ms, bid)
    pub fn get_last_tick(&self, table_name: &str) -> Result<Option<(i64, f64)>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT timestamp_ms, bid FROM {} ORDER BY timestamp_ms DESC LIMIT 1",
            table_name
        ))?;
        let mut rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
        })?;
        match rows.next() {
            Some(Ok(t)) => Ok(Some(t)),
            _ => Ok(None),
        }
    }

    /// Check if a table exists in the database
    pub fn table_exists(&self, table_name: &str) -> bool {
        self.conn.query_row(
            "SELECT COUNT(*) FROM information_schema.tables WHERE table_name = ?",
            params![table_name],
            |row| row.get::<_, i64>(0),
        ).map(|c| c > 0).unwrap_or(false)
    }

    /// Bulk-load candles from a CSV file using DuckDB's native read_csv.
    /// Uses a staging table to avoid INSERT OR REPLACE deadlock with read_csv.
    pub fn bulk_load_candles_from_csv(&self, table_name: &str, csv_path: &str) -> Result<()> {
        let path = csv_path.replace('\\', "/");
        // 1. Load CSV into a temp staging table (no PK, no conflicts)
        self.conn.execute(
            &format!(
                "CREATE OR REPLACE TEMP TABLE _csv_staging AS \
                 SELECT * FROM read_csv('{path}', \
                 columns={{'timestamp': 'BIGINT', 'open': 'DOUBLE', 'high': 'DOUBLE', \
                 'low': 'DOUBLE', 'close': 'DOUBLE', 'volume': 'BIGINT'}}, header=true)"
            ),
            [],
        )?;
        // 2. Remove any overlapping rows from the target table
        self.conn.execute(
            &format!("DELETE FROM {table_name} WHERE timestamp IN (SELECT timestamp FROM _csv_staging)"),
            [],
        )?;
        // 3. Insert all rows from staging into target
        self.conn.execute(
            &format!("INSERT INTO {table_name} SELECT * FROM _csv_staging"),
            [],
        )?;
        // 4. Clean up
        self.conn.execute("DROP TABLE IF EXISTS _csv_staging", [])?;
        Ok(())
    }

    /// Bulk-load ticks from a CSV file using DuckDB's native read_csv.
    /// No PK on tick tables, so direct INSERT (no staging needed).
    pub fn bulk_load_ticks_from_csv(&self, table_name: &str, csv_path: &str) -> Result<()> {
        let path = csv_path.replace('\\', "/");
        self.conn.execute(
            &format!(
                "INSERT INTO {table_name} SELECT * FROM read_csv('{path}', \
                 columns={{'timestamp_ms': 'BIGINT', 'bid': 'DOUBLE'}}, header=true)"
            ),
            [],
        )?;
        Ok(())
    }

    // ========================================================================
    // Ask tick data methods
    // ========================================================================

    /// Create an ask tick data table (no PK — multiple ticks can share the same millisecond)
    pub fn create_ask_tick_table_if_not_exists(&self, table_name: &str) -> Result<()> {
        self.conn.execute(
            &format!(
                "CREATE TABLE IF NOT EXISTS {} (
                    timestamp_ms BIGINT,
                    ask DOUBLE
                )",
                table_name
            ),
            [],
        )?;
        Ok(())
    }

    /// Bulk-load ask ticks from a CSV file using DuckDB's native read_csv.
    pub fn bulk_load_ask_ticks_from_csv(&self, table_name: &str, csv_path: &str) -> Result<()> {
        let path = csv_path.replace('\\', "/");
        self.conn.execute(
            &format!(
                "INSERT INTO {table_name} SELECT * FROM read_csv('{path}', \
                 columns={{'timestamp_ms': 'BIGINT', 'ask': 'DOUBLE'}}, header=true)"
            ),
            [],
        )?;
        Ok(())
    }

    // ========================================================================
    // Merged tick data methods (bid + ask + spread)
    // ========================================================================

    /// Merge bid and ask tick tables via dual ASOF JOIN + UNION.
    /// - Bid-anchored: each bid tick with its current ask (ASOF JOIN, fast)
    /// - Ask-anchored: each ask tick with its current bid (ASOF JOIN, fast)
    /// - UNION deduplicates identical rows, ORDER BY produces final timeline
    /// Captures every market state change from both sides without slow window functions.
    /// Spread clipped to >= 0. Result schema: (timestamp_ms BIGINT, bid DOUBLE, ask DOUBLE, spread DOUBLE)
    pub fn merge_bid_ask_ticks(
        &self,
        bid_table: &str,
        ask_table: &str,
        merged_table: &str,
    ) -> Result<()> {
        self.conn.execute(
            &format!(
                "CREATE OR REPLACE TABLE {merged_table} AS \
                 WITH bid_anchored AS ( \
                     SELECT b.timestamp_ms, b.bid, a.ask, \
                            GREATEST(a.ask - b.bid, 0.0) AS spread \
                     FROM {bid_table} b \
                     ASOF JOIN {ask_table} a ON b.timestamp_ms >= a.timestamp_ms \
                 ), \
                 ask_anchored AS ( \
                     SELECT a.timestamp_ms, b.bid, a.ask, \
                            GREATEST(a.ask - b.bid, 0.0) AS spread \
                     FROM {ask_table} a \
                     ASOF JOIN {bid_table} b ON a.timestamp_ms >= b.timestamp_ms \
                     WHERE a.ask >= b.bid \
                 ) \
                 SELECT * FROM bid_anchored \
                 UNION \
                 SELECT * FROM ask_anchored \
                 ORDER BY timestamp_ms"
            ),
            [],
        )?;
        Ok(())
    }

    /// Incrementally merge new bid+ask ticks into an existing merged table.
    /// Only processes ticks strictly after the last timestamp already in the merged table.
    /// Uses the same ASOF JOIN logic as the full merge — orders of magnitude faster for small updates.
    /// Returns the number of newly inserted rows.
    /// Incrementally merge new bid+ask ticks into an existing merged table.
    /// Only reads the last row of the merged table (seed) + new ticks after the cutoff.
    /// Avoids scanning the full bid/ask tables entirely — runs in milliseconds.
    pub fn incremental_merge_bid_ask_ticks(
        &self,
        bid_table: &str,
        ask_table: &str,
        merged_table: &str,
    ) -> Result<i64> {
        // Get the last merged row: cutoff timestamp + last known bid/ask prices as seed
        let (cutoff_ts, seed_bid, seed_ask): (i64, f64, f64) = self.conn.query_row(
            &format!(
                "SELECT timestamp_ms, bid, ask FROM {} ORDER BY timestamp_ms DESC LIMIT 1",
                merged_table
            ),
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;

        // Strategy: union new bid + new ask ticks with two virtual seed rows at the cutoff.
        // The seed rows carry the last known bid and ask so the window forward-fill has
        // valid initial values.  Only the new ticks (after cutoff) are ever read from the
        // large bid/ask tables — no full-table scan.
        self.conn.execute(
            &format!(
                "INSERT INTO {merged_table} \
                 WITH new_combined AS ( \
                     SELECT {cutoff_ts} AS timestamp_ms, {seed_bid:.10} AS price, 1 AS is_bid \
                     UNION ALL \
                     SELECT {cutoff_ts} AS timestamp_ms, {seed_ask:.10} AS price, 0 AS is_bid \
                     UNION ALL \
                     SELECT timestamp_ms, bid AS price, 1 AS is_bid \
                     FROM {bid_table} WHERE timestamp_ms > {cutoff_ts} \
                     UNION ALL \
                     SELECT timestamp_ms, ask AS price, 0 AS is_bid \
                     FROM {ask_table} WHERE timestamp_ms > {cutoff_ts} \
                 ), \
                 new_filled AS ( \
                     SELECT timestamp_ms, \
                            LAST_VALUE(CASE WHEN is_bid = 1 THEN price END IGNORE NULLS) \
                                OVER (ORDER BY timestamp_ms, is_bid DESC \
                                      ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS bid, \
                            LAST_VALUE(CASE WHEN is_bid = 0 THEN price END IGNORE NULLS) \
                                OVER (ORDER BY timestamp_ms, is_bid DESC \
                                      ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS ask \
                     FROM new_combined \
                 ) \
                 SELECT timestamp_ms, bid, ask, GREATEST(ask - bid, 0.0) AS spread \
                 FROM new_filled \
                 WHERE bid IS NOT NULL AND ask IS NOT NULL \
                   AND timestamp_ms > {cutoff_ts} \
                 ORDER BY timestamp_ms"
            ),
            [],
        )?;

        let new_count: i64 = self.conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM {} WHERE timestamp_ms > {}",
                merged_table, cutoff_ts
            ),
            [],
            |row| row.get(0),
        )?;
        Ok(new_count)
    }

    /// Count rows in the merged ticks table
    pub fn count_merged_ticks(&self, table_name: &str) -> Result<i64> {
        let count: i64 = self.conn.query_row(
            &format!("SELECT COUNT(*) FROM {}", table_name),
            [],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// Get the first (oldest) merged tick: (timestamp_ms, bid, ask, spread)
    pub fn get_first_merged_tick(&self, table_name: &str) -> Result<Option<(i64, f64, f64, f64)>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT timestamp_ms, bid, ask, spread FROM {} ORDER BY timestamp_ms ASC LIMIT 1",
            table_name
        ))?;
        let mut rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, f64>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, f64>(3)?,
            ))
        })?;
        match rows.next() {
            Some(Ok(t)) => Ok(Some(t)),
            _ => Ok(None),
        }
    }

    /// Get the last (newest) merged tick: (timestamp_ms, bid, ask, spread)
    pub fn get_last_merged_tick(&self, table_name: &str) -> Result<Option<(i64, f64, f64, f64)>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT timestamp_ms, bid, ask, spread FROM {} ORDER BY timestamp_ms DESC LIMIT 1",
            table_name
        ))?;
        let mut rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, f64>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, f64>(3)?,
            ))
        })?;
        match rows.next() {
            Some(Ok(t)) => Ok(Some(t)),
            _ => Ok(None),
        }
    }

    // ========================================================================
    // ML feature table methods
    // ========================================================================

    /// Check if an ML features table exists and has data.
    /// Returns Some(row_count) if the table exists and is non-empty, None otherwise.
    pub fn check_ml_features_table(&self, table_name: &str) -> Option<i64> {
        if !self.table_exists(table_name) {
            return None;
        }
        let count: i64 = self.conn.query_row(
            &format!("SELECT COUNT(*) FROM {}", table_name),
            [],
            |row| row.get(0),
        ).unwrap_or(0);
        if count > 0 { Some(count) } else { None }
    }

    /// Incrementally update the ML features table with rows for new M1 candles.
    /// Only aggregates merged ticks and processes M1 candles after the last
    /// timestamp already in the features table — much faster than a full rebuild.
    /// Returns the number of newly inserted rows.
    pub fn incremental_build_ml_features(
        &self,
        m1_table: &str,
        merged_table: &str,
        features_table: &str,
    ) -> Result<i64> {
        let cutoff_ts: i64 = self.conn.query_row(
            &format!("SELECT MAX(timestamp) FROM {}", features_table),
            [],
            |row| row.get(0),
        )?;
        // Compute the ms cutoff in Rust to avoid INT32 overflow in SQL
        // (cutoff_ts ~1.77B seconds * 1000 overflows INT32 if done in SQL)
        let cutoff_ms: i64 = cutoff_ts * 1000;

        // Aggregate only merged ticks after the cutoff (timestamp_ms is in ms, cutoff is seconds)
        self.conn.execute(
            &format!(
                "INSERT INTO {features_table} \
                 WITH tick_agg AS ( \
                     SELECT \
                         CAST(EPOCH(DATE_TRUNC('minute', epoch_ms(timestamp_ms))) AS BIGINT) AS minute_ts, \
                         COUNT(*) AS tick_count, \
                         AVG(spread) * 10000.0 AS spread_mean_pips, \
                         MAX(spread) * 10000.0 AS spread_max_pips, \
                         STDDEV(spread) * 10000.0 AS spread_std_pips, \
                         SUM(CASE WHEN spread > 0.00003 THEN 1 ELSE 0 END) AS wide_spread_count \
                     FROM {merged_table} \
                     WHERE timestamp_ms > {cutoff_ms} \
                     GROUP BY minute_ts \
                 ) \
                 SELECT \
                     m.timestamp, m.open, m.high, m.low, m.close, \
                     m.volume AS m1_volume, \
                     COALESCE(t.tick_count, 0) AS tick_count, \
                     COALESCE(t.spread_mean_pips, 0.0) AS spread_mean_pips, \
                     COALESCE(t.spread_max_pips, 0.0) AS spread_max_pips, \
                     COALESCE(t.spread_std_pips, 0.0) AS spread_std_pips, \
                     COALESCE(t.wide_spread_count, 0) AS wide_spread_count \
                 FROM {m1_table} m \
                 LEFT JOIN tick_agg t ON t.minute_ts = m.timestamp \
                 WHERE m.timestamp > {cutoff_ts} \
                 ORDER BY m.timestamp"
            ),
            [],
        )?;

        let new_count: i64 = self.conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM {} WHERE timestamp > {}",
                features_table, cutoff_ts
            ),
            [],
            |row| row.get(0),
        )?;
        Ok(new_count)
    }

    /// Get the first (oldest) row from the ML features table: (timestamp, tick_count, spread_mean_pips)
    pub fn get_first_ml_feature(&self, table_name: &str) -> Result<Option<(i64, i64, f64)>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT timestamp, tick_count, spread_mean_pips FROM {} ORDER BY timestamp ASC LIMIT 1",
            table_name
        ))?;
        let mut rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, f64>(2)?))
        })?;
        match rows.next() {
            Some(Ok(t)) => Ok(Some(t)),
            _ => Ok(None),
        }
    }

    /// Get the last (newest) row from the ML features table: (timestamp, tick_count, spread_mean_pips)
    pub fn get_last_ml_feature(&self, table_name: &str) -> Result<Option<(i64, i64, f64)>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT timestamp, tick_count, spread_mean_pips FROM {} ORDER BY timestamp DESC LIMIT 1",
            table_name
        ))?;
        let mut rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, f64>(2)?))
        })?;
        match rows.next() {
            Some(Ok(t)) => Ok(Some(t)),
            _ => Ok(None),
        }
    }

    /// Build the ML tick-feature table by aggregating merged ticks per minute
    /// and joining with M1 candles.
    ///
    /// Schema: timestamp, open, high, low, close, m1_volume,
    ///         tick_count, spread_mean_pips, spread_max_pips,
    ///         spread_std_pips, wide_spread_count
    ///
    /// Returns the total row count of the created table.
    pub fn build_ml_features_table(
        &self,
        m1_table: &str,
        merged_table: &str,
        features_table: &str,
    ) -> Result<i64> {
        self.conn.execute(
            &format!(
                "CREATE OR REPLACE TABLE {features_table} AS \
                 WITH tick_agg AS ( \
                     SELECT \
                         CAST(EPOCH(DATE_TRUNC('minute', epoch_ms(timestamp_ms))) AS BIGINT) AS minute_ts, \
                         COUNT(*) AS tick_count, \
                         AVG(spread) * 10000.0 AS spread_mean_pips, \
                         MAX(spread) * 10000.0 AS spread_max_pips, \
                         STDDEV(spread) * 10000.0 AS spread_std_pips, \
                         SUM(CASE WHEN spread > 0.00003 THEN 1 ELSE 0 END) AS wide_spread_count \
                     FROM {merged_table} \
                     GROUP BY minute_ts \
                 ) \
                 SELECT \
                     m.timestamp, m.open, m.high, m.low, m.close, \
                     m.volume AS m1_volume, \
                     COALESCE(t.tick_count, 0) AS tick_count, \
                     COALESCE(t.spread_mean_pips, 0.0) AS spread_mean_pips, \
                     COALESCE(t.spread_max_pips, 0.0) AS spread_max_pips, \
                     COALESCE(t.spread_std_pips, 0.0) AS spread_std_pips, \
                     COALESCE(t.wide_spread_count, 0) AS wide_spread_count \
                 FROM {m1_table} m \
                 LEFT JOIN tick_agg t ON t.minute_ts = m.timestamp \
                 WHERE m.timestamp >= 1346112000 \
                 ORDER BY m.timestamp"
            ),
            [],
        )?;

        let count: i64 = self.conn.query_row(
            &format!("SELECT COUNT(*) FROM {features_table}"),
            [],
            |row| row.get(0),
        )?;
        Ok(count)
    }
}
