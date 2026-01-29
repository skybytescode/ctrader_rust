use super::Candle;
use rusqlite::{Connection, Result, params};
use std::path::Path;

pub struct CandleDatabase {
    conn: Connection,
}

impl CandleDatabase {
    pub fn new<P: AsRef<Path>>(db_path: P) -> Result<Self> {
        let conn = Connection::open(db_path)?;
        
        // Create candles table if it doesn't exist
        conn.execute(
            "CREATE TABLE IF NOT EXISTS candles (
                timestamp BIGINT PRIMARY KEY,
                open DOUBLE,
                high DOUBLE,
                low DOUBLE,
                close DOUBLE,
                volume BIGINT
            )",
            [],
        )?;

        // Create index for fast time-range queries
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_timestamp ON candles(timestamp)",
            [],
        )?;

        Ok(Self { conn })
    }

    /// Query candles in a time range
    pub fn get_candles(&self, start_ts: i64, end_ts: i64) -> Result<Vec<Candle>> {
        let mut stmt = self.conn.prepare(
            "SELECT timestamp, open, high, low, close, volume 
             FROM candles 
             WHERE timestamp >= ? AND timestamp <= ?
             ORDER BY timestamp ASC"
        )?;

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
    pub fn get_last_candles(&self, count: usize) -> Result<Vec<Candle>> {
        let mut stmt = self.conn.prepare(
            "SELECT timestamp, open, high, low, close, volume 
             FROM candles 
             ORDER BY timestamp DESC
             LIMIT ?"
        )?;

        let candle_iter = stmt.query_map([count], |row| {
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
    pub fn insert_candle(&self, candle: &Candle) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO candles (timestamp, open, high, low, close, volume)
             VALUES (?, ?, ?, ?, ?, ?)",
            params![candle.timestamp, candle.open, candle.high, candle.low, candle.close, candle.volume],
        )?;
        Ok(())
    }

    /// Batch insert candles (more efficient)
    pub fn insert_candles(&mut self, candles: &[Candle]) -> Result<()> {
        let tx = self.conn.transaction()?;
        
        for candle in candles {
            tx.execute(
                "INSERT OR REPLACE INTO candles (timestamp, open, high, low, close, volume)
                 VALUES (?, ?, ?, ?, ?, ?)",
                params![candle.timestamp, candle.open, candle.high, candle.low, candle.close, candle.volume],
            )?;
        }
        
        tx.commit()?;
        Ok(())
    }

    /// Get count of candles in database
    pub fn count_candles(&self) -> Result<i64> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM candles",
            [],
            |row| row.get(0),
        )?;
        Ok(count)
    }
}
