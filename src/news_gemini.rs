//! Daily news analysis via Qwen 2.5 (Ollama).
//!
//! Reads raw articles from `news_today`, sends them to Qwen 2.5 in batches
//! via local Ollama, and stores structured analysis in `news_today_analyzed`.
//! This table is what gets sent to DeepSeek R1 for real-time trading decisions.

use serde::Deserialize;

const OLLAMA_URL: &str = "http://localhost:11434/api/generate";
const MODEL: &str = "qwen2.5:3b-instruct-q5_K_M";

/// Max articles per Ollama batch prompt
const BATCH_SIZE: usize = 10;

// ── DuckDB table ─────────────────────────────────────────────────────────────

const CREATE_TABLE: &str = "
CREATE TABLE IF NOT EXISTS news_today_analyzed (
    article_id          VARCHAR PRIMARY KEY,
    published_utc       VARCHAR NOT NULL,
    title               VARCHAR NOT NULL,
    eurusd_impact       FLOAT NOT NULL,
    volatility_expected FLOAT NOT NULL,
    relevance           FLOAT NOT NULL,
    category            VARCHAR NOT NULL,
    timeframe           VARCHAR NOT NULL,
    key_driver          VARCHAR NOT NULL
)
";

/// Article to analyze (from news_today).
#[derive(Debug, Clone)]
struct RawArticle {
    article_id: String,
    published_utc: String,
    title: String,
    summary: String,
}

/// Analysis of one article.
#[derive(Debug, Deserialize)]
struct ArticleAnalysis {
    #[serde(default)]
    article_id: String,
    #[serde(default)]
    eurusd_impact: f64,
    #[serde(default)]
    volatility_expected: f64,
    #[serde(default)]
    relevance: f64,
    #[serde(default)]
    category: String,
    #[serde(default)]
    timeframe: String,
    #[serde(default)]
    key_driver: String,
}

// ── Ollama API structs ──────────────────────────────────────────────────────

#[derive(serde::Serialize)]
struct OllamaRequest {
    model: String,
    prompt: String,
    stream: bool,
    keep_alive: String,
}

#[derive(Deserialize)]
struct OllamaResponse {
    #[serde(default)]
    response: String,
}

// ── Main function ───────────────────────────────────────────────────────────

/// Analyze today's news with Qwen 2.5 via Ollama. Fully synchronous (runs on std::thread).
/// Returns the number of articles analyzed.
pub fn analyze_today_news(
    shared_db: &std::sync::Arc<std::sync::Mutex<()>>,
    progress_tx: Option<&std::sync::mpsc::Sender<String>>,
) -> Result<usize, String> {
    let send = |msg: String| {
        if let Some(tx) = progress_tx {
            let _ = tx.send(msg);
        }
    };

    // Read raw articles from news_today
    let articles = {
        let _lock = shared_db.lock().unwrap_or_else(|e| e.into_inner());
        let db = duckdb::Connection::open("ctrader.duckdb")
            .map_err(|e| format!("DB open: {}", e))?;
        read_raw_articles(&db)?
    };

    if articles.is_empty() {
        send("No news articles to analyze".to_string());
        return Ok(0);
    }

    send(format!("Analyzing {} articles with Qwen 2.5...", articles.len()));

    // Ensure table exists (drop + recreate for fresh daily data)
    {
        let _lock = shared_db.lock().unwrap_or_else(|e| e.into_inner());
        let db = duckdb::Connection::open("ctrader.duckdb")
            .map_err(|e| format!("DB open: {}", e))?;
        let _ = db.execute("DROP TABLE IF EXISTS news_today_analyzed", []);
        db.execute_batch(CREATE_TABLE).map_err(|e| format!("create table: {}", e))?;
    }

    let client = reqwest::blocking::Client::new();
    let mut total_analyzed = 0usize;

    // Process in batches
    for (batch_idx, chunk) in articles.chunks(BATCH_SIZE).enumerate() {
        send(format!("Batch {}/{}: {} articles...",
            batch_idx + 1,
            (articles.len() + BATCH_SIZE - 1) / BATCH_SIZE,
            chunk.len()));

        let prompt = build_batch_prompt(chunk);

        let request = OllamaRequest {
            model: MODEL.to_string(),
            prompt,
            stream: false,
            keep_alive: "30m".to_string(),
        };

        let resp = client
            .post(OLLAMA_URL)
            .json(&request)
            .timeout(std::time::Duration::from_secs(120))
            .send()
            .map_err(|e| format!("Ollama request: {}", e))?;

        if !resp.status().is_success() {
            let err = resp.text().unwrap_or_default();
            send(format!("Ollama error: {}", &err[..err.len().min(200)]));
            continue;
        }

        let ollama_resp: OllamaResponse = resp.json()
            .map_err(|e| format!("Ollama parse: {}", e))?;

        let analyses = parse_analyses(&ollama_resp.response, chunk);

        // Write to DB
        {
            let _lock = shared_db.lock().unwrap_or_else(|e| e.into_inner());
            let db = duckdb::Connection::open("ctrader.duckdb")
                .map_err(|e| format!("DB open: {}", e))?;

            let insert = "INSERT OR REPLACE INTO news_today_analyzed VALUES (?,?,?,?,?,?,?,?,?)";
            for a in &analyses {
                let raw = chunk.iter().find(|r| r.article_id == a.article_id);
                if let Some(raw) = raw {
                    let _ = db.execute(insert, duckdb::params![
                        a.article_id,
                        raw.published_utc,
                        raw.title,
                        a.eurusd_impact,
                        a.volatility_expected,
                        a.relevance,
                        a.category,
                        a.timeframe,
                        a.key_driver,
                    ]);
                    total_analyzed += 1;
                }
            }
        }

        send(format!("Batch {}: {} articles stored", batch_idx + 1, analyses.len()));
    }

    send(format!("Done: {} articles analyzed with Qwen 2.5", total_analyzed));
    Ok(total_analyzed)
}

/// Read today's news for trading decision prompt (from analyzed table).
/// Returns compact lines sorted newest first.
pub fn read_for_claude(db: &duckdb::Connection) -> Vec<String> {
    let query = "
        SELECT published_utc, title, eurusd_impact, volatility_expected,
               relevance, category, timeframe, key_driver
        FROM news_today_analyzed
        ORDER BY published_utc DESC
    ";
    let mut stmt = match db.prepare(query) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map([], |row| {
        let pub_utc: String = row.get(0)?;
        let title: String = row.get(1)?;
        let impact: f64 = row.get(2)?;
        let vol: f64 = row.get(3)?;
        let rel: f64 = row.get(4)?;
        let cat: String = row.get(5)?;
        let tf: String = row.get(6)?;
        let driver: String = row.get(7)?;

        let time = if pub_utc.len() >= 16 { pub_utc[11..16].to_string() } else { pub_utc };
        let short_title = if title.chars().count() > 55 {
            let end: usize = title.char_indices().nth(52).map(|(i, _)| i).unwrap_or(title.len());
            format!("{}...", &title[..end])
        } else {
            title
        };

        Ok(format!("{} | imp={:+.1} vol={:.1} rel={:.1} | {} ({}) | {} | {}",
            time, impact, vol, rel, cat, tf, driver, short_title))
    });
    match rows {
        Ok(r) => r.flatten().collect(),
        Err(_) => Vec::new(),
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn read_raw_articles(db: &duckdb::Connection) -> Result<Vec<RawArticle>, String> {
    let query = "
        SELECT article_id, published_utc, title, COALESCE(summary, '') as summary
        FROM news_today
        ORDER BY published_utc DESC
    ";
    let mut stmt = db.prepare(query).map_err(|e| format!("prepare: {}", e))?;
    let rows = stmt.query_map([], |row| {
        Ok(RawArticle {
            article_id: row.get(0)?,
            published_utc: row.get(1)?,
            title: row.get(2)?,
            summary: row.get(3)?,
        })
    }).map_err(|e| format!("query: {}", e))?;

    Ok(rows.flatten().collect())
}

fn build_batch_prompt(articles: &[RawArticle]) -> String {
    let mut article_list = String::new();
    for (i, a) in articles.iter().enumerate() {
        let summary_part = if a.summary.is_empty() {
            String::new()
        } else {
            let short = if a.summary.chars().count() > 150 {
                let end: usize = a.summary.char_indices().nth(147).map(|(i, _)| i).unwrap_or(a.summary.len());
                format!("{}...", &a.summary[..end])
            } else {
                a.summary.clone()
            };
            format!("\n  Summary: {}", short)
        };
        article_list.push_str(&format!(
            "\n[{}] id=\"{}\" time=\"{}\" title=\"{}\"{}",
            i + 1, a.article_id, a.published_utc, a.title, summary_part
        ));
    }

    format!(
r#"Analyze these forex news articles for EUR/USD trading impact. For EACH article, provide:
- eurusd_impact: -1.0 (very bearish EUR/USD) to +1.0 (very bullish EUR/USD)
- volatility_expected: 0.0 (no impact) to 1.0 (major volatility)
- relevance: 0.0 (unrelated to EURUSD) to 1.0 (directly impacts EURUSD)
- category: one of: central_bank, economic_data, geopolitical, trade_war, market_sentiment, technical, other
- timeframe: immediate, short_term, or long_term
- key_driver: 3-8 word summary of why it matters

Articles:
{}

Respond ONLY with a valid JSON array, no markdown, no explanation:
[{{"article_id": "...", "eurusd_impact": 0.0, "volatility_expected": 0.0, "relevance": 0.0, "category": "...", "timeframe": "...", "key_driver": "..."}}, ...]"#,
        article_list
    )
}

fn parse_analyses(text: &str, articles: &[RawArticle]) -> Vec<ArticleAnalysis> {
    let trimmed = text.trim();

    // Strip markdown fences if present
    let json_str = if trimmed.starts_with("```") {
        trimmed.lines()
            .filter(|l| !l.trim().starts_with("```"))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        trimmed.to_string()
    };

    // Try to find JSON array in the response
    let json_to_parse = if let Some(start) = json_str.find('[') {
        if let Some(end) = json_str.rfind(']') {
            &json_str[start..=end]
        } else {
            &json_str
        }
    } else {
        &json_str
    };

    match serde_json::from_str::<Vec<ArticleAnalysis>>(json_to_parse) {
        Ok(analyses) => {
            let mut result: Vec<ArticleAnalysis> = Vec::new();
            for (i, mut a) in analyses.into_iter().enumerate() {
                if a.article_id.is_empty() {
                    if let Some(raw) = articles.get(i) {
                        a.article_id = raw.article_id.clone();
                    } else {
                        continue;
                    }
                }
                result.push(a);
            }
            result
        }
        Err(e) => {
            println!("Qwen news parse error: {} (first 200: {})",
                e, &json_to_parse[..json_to_parse.len().min(200)]);
            Vec::new()
        }
    }
}
