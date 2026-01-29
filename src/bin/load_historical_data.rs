#[path = "../db/mod.rs"]
mod db;

use db::{CandleDatabase, load_csv_to_duckdb};
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Loading historical EURUSD 4H data...");
    
    // Paths
    let csv_path = Path::new("instruments_db/csv_files/eurusd/EURUSD_Hour4.csv");
    let db_path = Path::new("instruments_db/eurusd_4h.duckdb");

    // Create database
    let mut db = CandleDatabase::new(db_path)?;
    
    // Load CSV data
    let count = load_csv_to_duckdb(csv_path, &mut db)?;
    
    println!("✓ Successfully loaded {} candles", count);
    println!("✓ Database created at: {:?}", db_path);
    
    // Verify
    let total = db.count_candles()?;
    println!("✓ Total candles in database: {}", total);
    
    // Show sample of last 5 candles
    let last_candles = db.get_last_candles(5)?;
    println!("\nLast 5 candles:");
    for candle in &last_candles {
        println!("  {} | O:{:.5} H:{:.5} L:{:.5} C:{:.5} V:{}",
            candle.datetime().format("%Y-%m-%d %H:%M"),
            candle.open,
            candle.high,
            candle.low,
            candle.close,
            candle.volume
        );
    }
    
    Ok(())
}
