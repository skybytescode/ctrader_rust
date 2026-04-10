//! Trading decision engine via DeepSeek R1 (Ollama).
//!
//! Runs every 2 minutes on a timer. Sends all available data
//! (candles, DoM, news, EC, ML models) to DeepSeek R1 for analysis.
//! Returns a structured trading decision.

use serde::Deserialize;

const OLLAMA_URL: &str = "http://localhost:11434/api/generate";
const MODEL: &str = "deepseek-r1:8b-llama-distill-q4_K_M";

// ── Ollama API ──────────────────────────────────────────────────────────────

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

// ── Decision response ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, serde::Serialize, Default)]
pub struct TradingDecision {
    pub m15_bias: Option<String>,
    pub m15_pattern: Option<String>,
    pub m5_bias: Option<String>,
    pub m5_pattern: Option<String>,
    pub dominant_bias: Option<String>,
    pub dominant_bias_confidence: Option<f64>,
    pub news_driven: Option<bool>,
    pub news_impact: Option<String>,
    pub session_quality: Option<String>,
    pub recommended_action: Option<String>,
    pub entry_timeframe: Option<String>,
    pub entry_condition: Option<String>,
    pub key_resistance: Option<f64>,
    pub key_support: Option<f64>,
    pub target_pips: Option<f64>,
    pub stop_pips: Option<f64>,
    pub invalidation: Option<String>,
}

impl TradingDecision {
    /// Format for UI display.
    pub fn display_summary(&self) -> String {
        let bias = self.dominant_bias.as_deref().unwrap_or("?");
        let conf = self.dominant_bias_confidence.unwrap_or(0.0);
        let action = self.recommended_action.as_deref().unwrap_or("?");
        let session = self.session_quality.as_deref().unwrap_or("?");

        let mut lines = Vec::new();

        // Header
        let news_flag = if self.news_driven.unwrap_or(false) { " | NEWS-DRIVEN" } else { "" };
        lines.push(format!("Bias: {} ({:.0}%) | Action: {} | Session: {}{}",
            bias.to_uppercase(), conf * 100.0, action, session, news_flag));

        // News impact
        if self.news_driven.unwrap_or(false) {
            if let Some(ref impact) = self.news_impact {
                if impact != "none" {
                    lines.push(format!("News: {}", impact));
                }
            }
        }

        // Per-timeframe
        lines.push(format!("M15: {} [{}]",
            self.m15_bias.as_deref().unwrap_or("?"),
            self.m15_pattern.as_deref().unwrap_or("none")));
        lines.push(format!("M5: {} [{}]",
            self.m5_bias.as_deref().unwrap_or("?"),
            self.m5_pattern.as_deref().unwrap_or("none")));

        // Entry
        if let Some(ref cond) = self.entry_condition {
            lines.push(format!("Entry: {}", cond));
        }

        // Levels
        let res = self.key_resistance.unwrap_or(0.0);
        let sup = self.key_support.unwrap_or(0.0);
        if res > 0.0 || sup > 0.0 {
            lines.push(format!("Levels: R={:.5} S={:.5}", res, sup));
        }

        // Target/Stop
        let tp = self.target_pips.unwrap_or(0.0);
        let sl = self.stop_pips.unwrap_or(0.0);
        if tp > 0.0 || sl > 0.0 {
            lines.push(format!("TP: {:.0} pips | SL: {:.0} pips", tp, sl));
        }

        // Invalidation
        if let Some(ref inv) = self.invalidation {
            if inv != "none" && !inv.is_empty() {
                lines.push(format!("Invalid: {}", inv));
            }
        }

        lines.join("\n")
    }
}

// ── Call DeepSeek R1 ────────────────────────────────────────────────────────

/// Call DeepSeek R1 via Ollama with the full trading prompt.
/// Returns a parsed TradingDecision or error string.
/// This is synchronous (blocking) — call from std::thread or spawn_blocking.
pub fn call_decision(prompt: &str) -> Result<TradingDecision, String> {
    let client = reqwest::blocking::Client::new();

    let request = OllamaRequest {
        model: MODEL.to_string(),
        prompt: prompt.to_string(),
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
        return Err(format!("Ollama error: {}", &err[..err.len().min(200)]));
    }

    let ollama_resp: OllamaResponse = resp.json()
        .map_err(|e| format!("Ollama parse: {}", e))?;

    let text = ollama_resp.response.trim();

    // DeepSeek R1 outputs <think>...</think> reasoning then the JSON answer
    let json_text = if let Some(end_think) = text.find("</think>") {
        text[end_think + 8..].trim()
    } else {
        text
    };

    // Strip markdown fences
    let json_clean = if json_text.contains("```") {
        json_text.lines()
            .filter(|l| !l.trim().starts_with("```"))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        json_text.to_string()
    };

    // Find JSON object
    let json_str = if let Some(start) = json_clean.find('{') {
        if let Some(end) = json_clean.rfind('}') {
            &json_clean[start..=end]
        } else {
            &json_clean
        }
    } else {
        &json_clean
    };

    serde_json::from_str::<TradingDecision>(json_str)
        .map_err(|e| format!("JSON parse: {} (first 300: {})", e, &json_str[..json_str.len().min(300)]))
}
