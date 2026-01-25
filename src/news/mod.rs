use rss::Channel;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::time::Duration;
use tokio::time::sleep;
use scraper::{Html, Selector};
use std::collections::HashSet;
use std::process::Command;

fn log_to_file(msg: &str) {
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open("scraper_log.txt")
    {
        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let _ = writeln!(file, "[{}] {}", timestamp, msg);
    }
}

pub fn spawn_news_scraper() {
    tokio::spawn(async move {
        log_to_file("Starting news scraper thread...");
        let rss_url = "https://www.myfxbook.com/rss/latest-forex-news";
        let news_page_url = "https://www.myfxbook.com/news";
        let output_dir = "news_data";
        
        if let Err(e) = std::fs::create_dir_all(output_dir) {
            log_to_file(&format!("Failed to create news_data directory: {}", e));
            return;
        }

        loop {
            log_to_file("Fetching latest news...");
            match fetch_and_save_all_news(rss_url, news_page_url, output_dir).await {
                Ok(_) => log_to_file("News updated successfully."),
                Err(e) => log_to_file(&format!("Error fetching news: {}", e)),
            }

            sleep(Duration::from_secs(15 * 60)).await;
        }
    });
}

// Fallback to curl.exe for Cloudflare-protected pages
fn fetch_with_curl(url: &str) -> Result<String, Box<dyn std::error::Error>> {
    let output = Command::new("curl.exe")
        .arg("-s")
        .arg("-L")
        .arg("-H")
        .arg("User-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/121.0.0.0 Safari/537.36")
        .arg(url)
        .output()?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(format!("Curl failed with status: {}", output.status).into())
    }
}

async fn fetch_and_save_all_news(rss_url: &str, news_page_url: &str, output_dir: &str) -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/121.0.0.0 Safari/537.36")
        .timeout(Duration::from_secs(15))
        .build()?;

    let mut articles = Vec::new();
    let mut seen_links = HashSet::new();

    // 1. Fetch from News Page using curl fallback
    log_to_file(&format!("Fetching news list from {} using curl...", news_page_url));
    match fetch_with_curl(news_page_url) {
        Ok(html_content) => {
            if html_content.contains("Just a moment...") {
                log_to_file("Cloudflare challenge detected on news page even with curl.");
            } else {
                let document = Html::parse_document(&html_content);
                let selector = Selector::parse("h2 a").unwrap();
                let mut count = 0;
                for element in document.select(&selector) {
                    if let Some(href) = element.value().attr("href") {
                        let full_link = if href.starts_with('/') {
                            format!("https://www.myfxbook.com{}", href)
                        } else {
                            href.to_string()
                        };
                        
                        if !seen_links.contains(&full_link) {
                            let title = element.text().collect::<Vec<_>>().join(" ");
                            articles.push((title, full_link.clone(), "Recently".to_string(), String::new()));
                            seen_links.insert(full_link);
                            count += 1;
                        }
                    }
                }
                log_to_file(&format!("Found {} articles from news page.", count));
            }
        }
        Err(e) => log_to_file(&format!("Curl failed for news page: {}", e)),
    }

    // 2. Fetch from RSS
    log_to_file(&format!("Fetching RSS from {}...", rss_url));
    if let Ok(response) = client.get(rss_url).send().await {
        if let Ok(bytes) = response.bytes().await {
            if let Ok(channel) = Channel::read_from(&bytes[..]) {
                let mut count = 0;
                for item in channel.items() {
                    let link = item.link().unwrap_or("").to_string();
                    if !link.is_empty() && !seen_links.contains(&link) {
                        articles.push((
                            item.title().unwrap_or("No Title").to_string(),
                            link.clone(),
                            item.pub_date().unwrap_or("No Date").to_string(),
                            item.description().unwrap_or("").to_string()
                        ));
                        seen_links.insert(link);
                        count += 1;
                    }
                }
                log_to_file(&format!("Found {} new articles from RSS.", count));
            }
        }
    }

    if articles.is_empty() {
        log_to_file("No articles found.");
        return Ok(());
    }

    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
    let file_path = format!("{}/news_{}.txt", output_dir, timestamp);
    let mut file = File::create(&file_path)?;

    log_to_file(&format!("Processing {} articles...", articles.len()));

    for (title, link, pub_date, description) in articles {
        let mut content = String::new();

        if link.contains("myfxbook.com/news/") {
            log_to_file(&format!("Fetching full content for: {}", title));
            match fetch_full_article_content(&link) {
                Ok(text) => content = text,
                Err(e) => {
                    log_to_file(&format!("Failed to fetch full article for {}: {}", title, e));
                    content = format!("(Error fetching full content: {})", e);
                }
            }
        }

        if content.is_empty() && !description.is_empty() {
            content = strip_html_tags(&description);
        }

        if content.is_empty() || content == "None" {
            if link.contains("marketwatch.com") {
                content = "(External MarketWatch link - full content requires subscription)".to_string();
            } else {
                content = "(No content available)".to_string();
            }
        }
        
        writeln!(file, "# TITLE: {}", title)?;
        writeln!(file, "DATE: {}", pub_date)?;
        writeln!(file, "LINK: {}", link)?;
        writeln!(file, "CONTENT:")?;
        writeln!(file, "{}", content)?;
        writeln!(file, "\n---\n")?;
    }

    log_to_file("Extraction complete.");
    Ok(())
}

fn fetch_full_article_content(url: &str) -> Result<String, Box<dyn std::error::Error>> {
    let html_content = fetch_with_curl(url)?;
    if html_content.contains("Just a moment...") {
        return Err("Cloudflare Block".into());
    }
    let document = Html::parse_document(&html_content);
    let selector = Selector::parse("#news-body").unwrap();

    if let Some(news_body) = document.select(&selector).next() {
        let text = news_body.text().collect::<Vec<_>>().join(" ");
        let cleaned = clean_whitespace(&text);
        if !cleaned.is_empty() {
            return Ok(cleaned);
        }
    }

    Ok(String::new())
}

fn strip_html_tags(input: &str) -> String {
    let mut output = String::new();
    let mut in_tag = false;
    
    for c in input.chars() {
        if c == '<' {
            in_tag = true;
        } else if c == '>' {
            in_tag = false;
        } else if !in_tag {
            output.push(c);
        }
    }
    
    clean_entities(&output)
}

fn clean_entities(input: &str) -> String {
    let cleaned = input
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("\r\n", "\n")
        .replace("\r", "\n");

    clean_whitespace(&cleaned)
}

fn clean_whitespace(input: &str) -> String {
    let mut final_output = String::new();
    let mut last_was_ws = false;
    for c in input.chars() {
        if c.is_whitespace() {
            if !last_was_ws {
                final_output.push(' ');
                last_was_ws = true;
            }
        } else {
            final_output.push(c);
            last_was_ws = false;
        }
    }
    final_output.trim().to_string()
}
