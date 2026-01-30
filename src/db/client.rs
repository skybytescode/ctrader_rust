use super::Candle;
use rusqlite::{Connection, Result, params};
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
        self.conn.execute(&format!(
            "CREATE TABLE IF NOT EXISTS {} (
                timestamp BIGINT PRIMARY KEY,
                open DOUBLE,
                high DOUBLE,
                low DOUBLE,
                close DOUBLE,
                volume BIGINT
            )",
            table_name
        ), [])?;

        // Create index for fast time-range queries
        self.conn.execute(&format!(
            "CREATE INDEX IF NOT EXISTS idx_timestamp_{} ON {}(timestamp)",
            table_name, table_name
        ), [])?;
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
    pub fn insert_candle(&self, table_name: &str, candle: &Candle) -> Result<()> {
        self.conn.execute(
            &format!(
                "INSERT OR REPLACE INTO {} (timestamp, open, high, low, close, volume)\n                         VALUES (?, ?, ?, ?, ?, ?)",
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
                    "INSERT OR REPLACE INTO {} (timestamp, open, high, low, close, volume)\n                     VALUES (?, ?, ?, ?, ?, ?)",
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
}
