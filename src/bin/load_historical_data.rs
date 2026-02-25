use db::{CandleDatabase, load_csv_to_duckdb};
use std::path::Path;
use std::fs;

#[path = "../db/mod.rs"]
mod db;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Starting historical data loading process...");

    let base_csv_dir = Path::new("Bots_db/csv_files");
    let main_db_path = Path::new("Bots_db/Algo_EURUSD.duckdb");

    // Ensure the Bots_db directory exists
    // The main_db_path ensures the parent directory exists
    fs::create_dir_all(main_db_path.parent().unwrap())?;

    // Create database connection once
    let mut db = CandleDatabase::new(&main_db_path)?;

    for entry in fs::read_dir(base_csv_dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            let instrument_name = path.file_name().ok_or("Invalid directory name")?.to_string_lossy().to_string();
            println!("\nProcessing instrument: {}", instrument_name);

            // Find CSV files within the instrument directory
            let mut csv_files = Vec::new();
            for csv_entry in fs::read_dir(&path)? {
                let csv_path = csv_entry?.path();
                if csv_path.is_file() && csv_path.extension().map_or(false, |ext| ext == "csv") {
                    csv_files.push(csv_path);
                }
            }

            if csv_files.is_empty() {
                println!("No CSV files found for instrument: {}. Skipping.", instrument_name);
                continue;
            }

            for csv_path in csv_files {
                let csv_file_stem = csv_path.file_stem().ok_or("Invalid CSV filename")?.to_string_lossy().to_string();
                
                // Construct a valid table name (e.g., eurusd_hour4)
                let table_name = format!("{}_{}",
                                         instrument_name.to_lowercase(),
                                         csv_file_stem.to_lowercase().replace("-", "_").replace(" ", "_"));
                
                println!("  Loading CSV: {:?}", csv_path);
                println!("  Into table: \"{}\" in database: {:?}", table_name, main_db_path);

                // Create table if it doesn't exist
                db.create_table_if_not_exists(&table_name)?;

                // Load CSV data
                let count = load_csv_to_duckdb(&csv_path, &table_name, &mut db)?;

              println!(" [x] Successfully loaded {} candles into table {}", count, table_name);
                // Verify
                let total = db.count_candles(&table_name)?;
               println!(" [x] Total candles in table {}: {}", table_name, total);

                // Show sample of last 5 candles
                let last_candles = db.get_last_candles(&table_name, 5)?;
               println!(" Last 5 candles from table {}:", table_name);
                if last_candles.is_empty() {
                    println!("    (No candles found)");
                } else {
                    for candle in &last_candles {
                        println!("    {} | O:{:.5} H:{:.5} L:{:.5} C:{:.5} V:{}",
                            candle.datetime().format("%Y-%m-%d %H:%M"),
                            candle.open,
                            candle.high,
                            candle.low,
                            candle.close,
                            candle.volume
                        );
                    }
                }
            }
        }
    }

    println!("\nHistorical data loading process completed.");
    Ok(())
}