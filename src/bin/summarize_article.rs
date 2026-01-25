#[path = "../ai/mod.rs"]
mod ai;

use std::fs;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv::dotenv().ok();
    let api_key = std::env::var("GOOGLE_API_KEY")
        .expect("GOOGLE_API_KEY must be set in .env file");
    
    let summary = ai::summarizer::processor::run_article_summary(api_key).await?;

    println!("\n--- SUMMARY ---");
    println!("{}", summary);
    println!("----------------\n");

    fs::write("summary_result.txt", summary)?;
    println!("Summary successfully saved to summary_result.txt");

    Ok(())
}
