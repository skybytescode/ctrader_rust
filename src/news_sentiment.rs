//! News sentiment analysis via local Ollama LLM.
//!
//! Processes news articles from `news_historical` through a local Llama model
//! and stores structured sentiment data in `news_sentiment`.
//! Incremental: only processes articles not yet analyzed.
//! Fully synchronous — runs on a plain std::thread, no async runtime needed.

use serde::Deserialize;

const OLLAMA_URL: &str = "http://localhost:11434/api/generate";
const DEFAULT_MODEL: &str = "qwen2.5:3b-instruct-q5_K_M";

const PROMPT_TEMPLATE: &str = r#"Analyze this forex news for EUR/USD impact. Respond ONLY with valid JSON, no explanation, no markdown.

Title: {TITLE}
Summary: {SUMMARY}

{"eur_sentiment": <-1.0 to 1.0>, "usd_sentiment": <-1.0 to 1.0>, "eurusd_impact": <-1.0 to 1.0>, "volatility_expected": <0.0 to 1.0>, "relevance": <0.0 to 1.0 how relevant to EURUSD>, "category": "<central_bank|economic_data|geopolitical|technical|market_sentiment|trade_war|other>", "timeframe": "<immediate|short_term|long_term>", "key_driver": "<5-10 words>"}"#;

// ── DuckDB table ─────────────────────────────────────────────────────────────

const CREATE_SENTIMENT_TABLE: &str = "
CREATE TABLE IF NOT EXISTS news_sentiment (
    article_id          VARCHAR PRIMARY KEY,
    published_utc       VARCHAR NOT NULL,
    eur_sentiment       FLOAT,
    usd_sentiment       FLOAT,
    eurusd_impact       FLOAT,
    volatility_expected FLOAT,
    relevance           FLOAT,
    category            VARCHAR,
    timeframe           VARCHAR,
    key_driver          VARCHAR,
    model_used          VARCHAR,
    analyzed_at         VARCHAR
)
";

// ── Ollama response structs ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct OllamaResponse {
    #[serde(default)]
    response: String,
}

#[derive(Debug, Deserialize, Default)]
struct SentimentResult {
    #[serde(default)]
    eur_sentiment: Option<f64>,
    #[serde(default)]
    usd_sentiment: Option<f64>,
    #[serde(default)]
    eurusd_impact: Option<f64>,
    #[serde(default)]
    volatility_expected: Option<f64>,
    #[serde(default)]
    relevance: Option<f64>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    timeframe: Option<String>,
    #[serde(default)]
    key_driver: Option<String>,
}

/// Article to be analyzed
#[derive(Debug)]
struct PendingArticle {
    article_id: String,
    published_utc: String,
    title: String,
    summary: String,
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Ensure the news_sentiment table exists.
pub fn ensure_table(db: &duckdb::Connection) -> Result<(), String> {
    db.execute_batch(CREATE_SENTIMENT_TABLE)
        .map_err(|e| format!("create news_sentiment: {}", e))
}

/// Count how many articles still need analysis.
pub fn count_pending(db: &duckdb::Connection) -> Result<(i64, i64), String> {
    let total: i64 = db.query_row(
        "SELECT COUNT(*) FROM news_historical", [], |r| r.get(0)
    ).unwrap_or(0);
    let analyzed: i64 = db.query_row(
        "SELECT COUNT(*) FROM news_sentiment", [], |r| r.get(0)
    ).unwrap_or(0);
    Ok((total, analyzed))
}

/// Run batch sentiment analysis (synchronous — call from std::thread).
/// Processes articles not yet in news_sentiment.
/// Sends real-time progress to `progress_tx`. Stops if receiver is dropped.
pub fn analyze_batch_sync(
    shared_db: &crate::SharedDb,
    progress_tx: &std::sync::mpsc::Sender<String>,
    batch_size: usize,
) -> Result<usize, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    // Fetch pending articles (lock DB briefly)
    println!("News sentiment: waiting for DB lock...");
    let (pending, already_done) = {
        let _lock = shared_db.lock().unwrap();
        println!("News sentiment: DB lock acquired, opening connection...");
        let db = duckdb::Connection::open(crate::DB_PATH)
            .map_err(|e| format!("DB open: {}", e))?;
        println!("News sentiment: querying pending articles...");
        ensure_table(&db)?;

        let already: i64 = db.query_row(
            "SELECT COUNT(*) FROM news_sentiment", [], |r| r.get(0)
        ).unwrap_or(0);

        let mut stmt = db.prepare(
            "SELECT h.article_id, h.published_utc, h.title, COALESCE(h.summary, '')
             FROM news_historical h
             LEFT JOIN news_sentiment s ON h.article_id = s.article_id
             WHERE s.article_id IS NULL
             ORDER BY h.published_utc DESC
             LIMIT ?"
        ).map_err(|e| format!("prepare: {}", e))?;

        let rows = stmt.query_map(duckdb::params![batch_size as i64], |row| {
            Ok(PendingArticle {
                article_id: row.get(0)?,
                published_utc: row.get(1)?,
                title: row.get(2)?,
                summary: row.get::<_, String>(3)?,
            })
        }).map_err(|e| format!("query: {}", e))?;

        let articles: Vec<PendingArticle> = rows.flatten().collect();
        (articles, already)
    }; // DB lock released here

    let total = pending.len();
    if total == 0 {
        let _ = progress_tx.send(format!("All {} articles already analyzed.", already_done));
        return Ok(0);
    }

    let send = |msg: String| -> bool {
        progress_tx.send(msg).is_ok()
    };

    send(format!("{} done, {} remaining. Starting {} ...",
        already_done, total, DEFAULT_MODEL));

    let mut analyzed = 0usize;
    let mut errors = 0usize;
    let mut consecutive_errors = 0usize;
    let start_time = std::time::Instant::now();

    for (i, article) in pending.iter().enumerate() {
        // Check if UI still listening (user clicked Stop)
        if !send(format!(
            "[{}/{}] {} ...", i + 1, total, truncate(&article.title, 55)
        )) {
            println!("News sentiment: UI stopped, halting at {}/{}", i, total);
            break;
        }

        // Build prompt
        let prompt = PROMPT_TEMPLATE
            .replace("{TITLE}", &article.title)
            .replace("{SUMMARY}", &article.summary);

        // Call Ollama (blocking HTTP — this thread is dedicated, so blocking is fine)
        if i == 0 {
            println!("News sentiment: sending first Ollama request...");
        }
        let body = serde_json::json!({
            "model": DEFAULT_MODEL,
            "prompt": prompt,
            "stream": false,
            "keep_alive": "30m"
        });

        let req_start = std::time::Instant::now();
        let result = client.post(OLLAMA_URL)
            .json(&body)
            .send();
        if i == 0 {
            println!("News sentiment: first Ollama response in {:.1}s", req_start.elapsed().as_secs_f64());
        }

        let sentiment = match result {
            Ok(resp) => {
                match resp.json::<OllamaResponse>() {
                    Ok(ollama_resp) => {
                        consecutive_errors = 0;
                        parse_sentiment(&ollama_resp.response)
                    }
                    Err(e) => {
                        errors += 1;
                        consecutive_errors += 1;
                        send(format!("[{}/{}] Parse error: {} — skipping", i + 1, total, e));
                        if consecutive_errors >= 5 {
                            send(format!("Stopped: {} consecutive errors", consecutive_errors));
                            return Err(format!("{} consecutive errors", consecutive_errors));
                        }
                        continue;
                    }
                }
            }
            Err(e) => {
                errors += 1;
                consecutive_errors += 1;
                let msg = format!("[{}/{}] Ollama error: {}", i + 1, total, e);
                println!("News sentiment: {}", msg);
                send(msg);
                if consecutive_errors >= 5 {
                    send(format!("Stopped: {} consecutive errors", consecutive_errors));
                    return Err(format!("{} consecutive errors, last: {}", consecutive_errors, e));
                }
                std::thread::sleep(std::time::Duration::from_secs(2));
                continue;
            }
        };

        // Write to DB (lock briefly, write, release)
        let now = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string();
        {
            let _lock = shared_db.lock().unwrap();
            if let Ok(db) = duckdb::Connection::open(crate::DB_PATH) {
                let _ = db.execute(
                    "INSERT INTO news_sentiment VALUES (?,?,?,?,?,?,?,?,?,?,?,?)
                     ON CONFLICT (article_id) DO NOTHING",
                    duckdb::params![
                        article.article_id,
                        article.published_utc,
                        sentiment.eur_sentiment,
                        sentiment.usd_sentiment,
                        sentiment.eurusd_impact,
                        sentiment.volatility_expected,
                        sentiment.relevance,
                        sentiment.category,
                        sentiment.timeframe,
                        sentiment.key_driver,
                        DEFAULT_MODEL,
                        now,
                    ],
                );
            }
        } // lock released

        analyzed += 1;

        // Progress
        let elapsed = start_time.elapsed().as_secs_f64();
        let per_article = if analyzed > 0 { elapsed / analyzed as f64 } else { 0.0 };
        let remaining = (total - i - 1) as f64 * per_article;
        let eta = format_duration(remaining);

        let progress_msg = format!(
            "[{}/{} total:{}] {:.1}s/art | {} err | ETA: {} | {}",
            i + 1, total, already_done + analyzed as i64, per_article, errors, eta,
            truncate(&article.title, 45)
        );
        if analyzed % 10 == 0 || analyzed <= 5 {
            println!("News sentiment: {}", progress_msg);
        }
        if !send(progress_msg) {
            println!("News sentiment: UI channel closed at {}/{}", i + 1, total);
            break;
        }
    }

    send(format!(
        "Done: {} analyzed, {} errors, {:.0}s total",
        analyzed, errors, start_time.elapsed().as_secs_f64()
    ));

    Ok(analyzed)
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn parse_sentiment(response: &str) -> SentimentResult {
    let text = response.trim();

    // Strip markdown code fences if present
    let json_str = if text.contains("```") {
        text.lines()
            .filter(|l| !l.trim().starts_with("```"))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        text.to_string()
    };

    // Find JSON object in the response
    let json_str = json_str.trim();
    let json_str = if let Some(start) = json_str.find('{') {
        if let Some(end) = json_str.rfind('}') {
            &json_str[start..=end]
        } else {
            json_str
        }
    } else {
        json_str
    };

    serde_json::from_str(json_str).unwrap_or_default()
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        let end: usize = s.char_indices().nth(max).map(|(i, _)| i).unwrap_or(s.len());
        format!("{}...", &s[..end])
    } else {
        s.to_string()
    }
}

/// Read today's news with sentiment scores for Claude prompt.
/// Returns compact lines like: "02:15 | impact=+0.8 vol=0.9 | trade_war | Trump tariff pause"
/// Sorted by time descending (newest first), limited to 30 most recent.
pub fn read_news_for_claude(db: &duckdb::Connection) -> Vec<String> {
    let query = "
        SELECT n.published_utc, n.title,
               s.eurusd_impact, s.volatility_expected, s.relevance,
               s.category, s.timeframe, s.key_driver
        FROM news_today n
        LEFT JOIN news_sentiment s ON n.article_id = s.article_id
        ORDER BY n.published_utc DESC
        LIMIT 30
    ";
    let mut stmt = match db.prepare(query) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map([], |row| {
        let pub_utc: String = row.get(0)?;
        let title: String = row.get(1)?;
        let impact: Option<f64> = row.get(2)?;
        let volatility: Option<f64> = row.get(3)?;
        let relevance: Option<f64> = row.get(4)?;
        let category: Option<String> = row.get(5)?;
        let timeframe: Option<String> = row.get(6)?;
        let key_driver: Option<String> = row.get(7)?;

        let time = if pub_utc.len() >= 16 { &pub_utc[11..16] } else { &pub_utc };

        let line = if let Some(imp) = impact {
            let rel = relevance.unwrap_or(0.0);
            let vol = volatility.unwrap_or(0.0);
            let cat = category.as_deref().unwrap_or("?");
            let tf = timeframe.as_deref().unwrap_or("?");
            let driver = key_driver.as_deref().unwrap_or("");
            let short_title = if title.chars().count() > 60 {
                let end: usize = title.char_indices().nth(57).map(|(i, _)| i).unwrap_or(title.len());
                format!("{}...", &title[..end])
            } else {
                title
            };
            format!("{} | impact={:+.1} vol={:.1} rel={:.1} | {} ({}) | {} | {}",
                time, imp, vol, rel, cat, tf, driver, short_title)
        } else {
            // Not yet analyzed by Ollama
            let short_title = if title.chars().count() > 70 {
                let end: usize = title.char_indices().nth(67).map(|(i, _)| i).unwrap_or(title.len());
                format!("{}...", &title[..end])
            } else {
                title
            };
            format!("{} | (not analyzed) | {}", time, short_title)
        };

        Ok(line)
    });
    match rows {
        Ok(r) => r.flatten().collect(),
        Err(_) => Vec::new(),
    }
}

fn format_duration(secs: f64) -> String {
    let s = secs as u64;
    if s >= 3600 {
        format!("{}h{}m", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{}m{}s", s / 60, s % 60)
    } else {
        format!("{}s", s)
    }
}
