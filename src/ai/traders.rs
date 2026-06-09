//! Multi-provider "gold day trader" inference.
//!
//! Each provider is asked the SAME thing from the SAME compact live snapshot:
//! produce ONE intraday VWAP + 8 EMA setup (bias + entry / stop / targets).
//! Every call is single-shot — no tools, no agent loop — so the whole 4-model
//! fan-out returns in well under 20 s. Used by the Trade Ideas 4-popup feature.

use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// General gold day-trader prompt for the small/fast models (Gemini, DeepSeek,
/// Qwen). Not locked to one method and not the full strategy library — the model
/// just reads the tape, names the obvious pattern, and gives one setup. The rich
/// multi-strategy playbook is reserved for Claude.
pub const SIMPLE_TRADER_PROMPT: &str = r#"You are a professional XAUUSD (gold) intraday DAY TRADER. Read the live snapshot (price, VWAP, 8 EMA, session high/low, prior-day high/low/close, ATR(H1), latest M1 & M5 candles, calendar, headlines) and give ONE intraday setup for right now — whichever direction and pattern the data supports.

- Identify the obvious intraday pattern (trend pullback, breakout, range fade, support/resistance bounce, liquidity sweep, news reaction) and trade with it; don't force any single indicator.
- Place the stop beyond your invalidation, within ~2x ATR(H1); prefer R:R >= 1.5.
- Stand aside (bias FLAT, null levels) if the tape is choppy/directionless, you're within ~30 min of a tier-1 release (FOMC / CPI / NFP / PCE), or candles are stale (weekend).
- Let the headlines tilt your long-vs-short lean.

Output ONLY a single JSON object — no prose, no markdown, no code fences, no <think> tags. Exact schema:
{"bias":"LONG|SHORT|FLAT","strategy":"short label of the pattern you traded","entry_low":number|null,"entry_high":number|null,"stop":number|null,"target1":number|null,"target2":number|null,"rationale":"<=160 chars"}
All prices are USD floats. For FLAT set every level to null and use rationale to state the wait condition."#;

/// The multi-strategy professional gold day-trader playbook — Claude's system
/// prompt only. Embedded from the reusable skill file at compile time; the YAML
/// frontmatter is stripped at runtime by [`gold_day_trader_prompt`].
const GOLD_DAY_TRADER_SKILL: &str =
    include_str!("../../.claude/skills/gold-day-trader/SKILL.md");

/// The trend-scalping playbook — "Claude Blitz" system prompt only. Embedded
/// from the reusable skill file at compile time; frontmatter stripped at runtime.
const CLAUDE_BLITZ_SKILL: &str =
    include_str!("../../.claude/skills/claude-blitz/SKILL.md");

/// The volume-profile playbook — "Claude Volume" system prompt only.
const CLAUDE_VOLUME_SKILL: &str =
    include_str!("../../.claude/skills/claude-volume/SKILL.md");

/// The professional XRPUSD 5-minute playbook — "XRP 5m" agent system prompt.
const XRP_5M_SKILL: &str =
    include_str!("../../.claude/skills/xrp-5m/SKILL.md");

/// The gold news-sentiment agent — reads the day's news like a human trader and
/// returns the crowd's XAUUSD disposition + forward outlook. Market Predictor.
const GOLD_SENTIMENT_SKILL: &str =
    include_str!("../../.claude/skills/gold-sentiment/SKILL.md");

/// Strip a leading `---\n … \n---\n` YAML frontmatter block from a skill body.
fn strip_frontmatter(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            return rest[end + 4..].trim_start().to_string();
        }
    }
    s.to_string()
}

/// Claude's system prompt: the skill body with its YAML frontmatter removed.
pub fn gold_day_trader_prompt() -> String {
    strip_frontmatter(GOLD_DAY_TRADER_SKILL)
}

/// Claude Blitz's system prompt: the trend-scalping skill body (frontmatter removed).
pub fn blitz_prompt() -> String {
    strip_frontmatter(CLAUDE_BLITZ_SKILL)
}

/// Claude Volume's system prompt: the volume-profile skill body (frontmatter removed).
pub fn volume_prompt() -> String {
    strip_frontmatter(CLAUDE_VOLUME_SKILL)
}

/// XRP 5m agent's system prompt: the XRP 5-minute skill body (frontmatter removed).
pub fn xrp_5m_prompt() -> String {
    strip_frontmatter(XRP_5M_SKILL)
}

/// Gold news-sentiment agent's system prompt (frontmatter removed).
pub fn gold_sentiment_prompt() -> String {
    strip_frontmatter(GOLD_SENTIMENT_SKILL)
}

/// One model's answer, ready to serialize to the frontend popup.
#[derive(Serialize, Clone, Debug)]
pub struct ModelTradeIdea {
    pub provider: String,
    pub model: String,
    pub ok: bool,
    pub bias: Option<String>,
    /// Name of the strategy the model chose (Claude picks from a library; the
    /// focused models report "VWAP+8EMA").
    pub strategy: Option<String>,
    pub entry_low: Option<f64>,
    pub entry_high: Option<f64>,
    pub stop: Option<f64>,
    pub target1: Option<f64>,
    pub target2: Option<f64>,
    pub rationale: Option<String>,
    pub error: Option<String>,
    pub duration_ms: u128,
}

#[derive(Deserialize)]
struct SetupJson {
    bias: Option<String>,
    #[serde(default)]
    strategy: Option<String>,
    entry_low: Option<f64>,
    entry_high: Option<f64>,
    stop: Option<f64>,
    target1: Option<f64>,
    target2: Option<f64>,
    rationale: Option<String>,
}

impl ModelTradeIdea {
    /// Public constructor for an errored result (e.g. a missing API key) built
    /// without making a call.
    pub fn error(provider: &str, model: &str, msg: impl Into<String>) -> Self {
        Self::err(provider, model, msg.into(), 0)
    }
    fn err(provider: &str, model: &str, msg: String, ms: u128) -> Self {
        Self {
            provider: provider.into(), model: model.into(), ok: false,
            bias: None, strategy: None, entry_low: None, entry_high: None, stop: None,
            target1: None, target2: None, rationale: None,
            error: Some(msg), duration_ms: ms,
        }
    }
}

/// Strip any `<think>…</think>` reasoning trace and pull the FIRST balanced JSON
/// object out of a model's raw text (handles bare objects, ```json fences, and
/// models that emit a second object or trailing prose after the first).
fn extract_json(raw: &str) -> Option<String> {
    let mut s = raw;
    if let Some(end) = s.rfind("</think>") {
        s = &s[end + "</think>".len()..];
    }
    // Prefer the contents of a fenced ```json … ``` block if present.
    let lower = s.to_ascii_lowercase();
    let body = if let Some(i) = lower.find("```json") {
        let after = &s[i + "```json".len()..];
        match after.find("```") { Some(j) => &after[..j], None => after }
    } else {
        s
    };
    // Brace-match from the first '{' to its matching '}', skipping braces inside
    // strings. Returns the first complete object only — ignores anything after.
    let bytes = body.as_bytes();
    let start = body.find('{')?;
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    for i in start..bytes.len() {
        let c = bytes[i];
        if in_str {
            if esc { esc = false; }
            else if c == b'\\' { esc = true; }
            else if c == b'"' { in_str = false; }
        } else {
            match c {
                b'"' => in_str = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 { return Some(body[start..=i].to_string()); }
                }
                _ => {}
            }
        }
    }
    None
}

/// Turn raw model text into a structured `ModelTradeIdea`.
pub fn parse_model_output(provider: &str, model: &str, raw: &str, ms: u128) -> ModelTradeIdea {
    let Some(json) = extract_json(raw) else {
        return ModelTradeIdea::err(provider, model,
            format!("no JSON in model output (got {} chars)", raw.trim().len()), ms);
    };
    match serde_json::from_str::<SetupJson>(&json) {
        Ok(s) => ModelTradeIdea {
            provider: provider.into(), model: model.into(), ok: true,
            bias: s.bias.map(|b| b.to_uppercase()),
            strategy: s.strategy,
            entry_low: s.entry_low, entry_high: s.entry_high, stop: s.stop,
            target1: s.target1, target2: s.target2,
            rationale: s.rationale, error: None, duration_ms: ms,
        },
        Err(e) => ModelTradeIdea::err(provider, model, format!("JSON parse: {}", e), ms),
    }
}

/// Timeout for the Gemini HTTP call (fast).
const PER_MODEL_TIMEOUT: Duration = Duration::from_secs(18);
/// Timeout for the local `claude` CLI path. A COLD single-shot (no prompt cache
/// yet) can take ~30s+; the old 18s cap killed it before it could complete and
/// warm its cache, so it would time out forever. Give it real headroom.
const CLAUDE_CLI_TIMEOUT: Duration = Duration::from_secs(60);

/// Call Google Gemini via the generateContent REST endpoint. `responseMimeType`
/// asks Gemini for raw JSON so there's nothing to unfence.
pub async fn run_gemini(provider: &str, model: &str, api_key: &str, system: &str, user: &str) -> ModelTradeIdea {
    let t = Instant::now();
    let client = reqwest::Client::new();
    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
        model, api_key
    );
    let body = serde_json::json!({
        "system_instruction": { "parts": [{ "text": system }] },
        "contents": [{ "parts": [{ "text": user }] }],
        "generationConfig": { "temperature": 0.3, "responseMimeType": "application/json" },
    });
    let resp = client.post(&url).json(&body).timeout(PER_MODEL_TIMEOUT).send().await;
    match resp {
        Ok(r) if r.status().is_success() => match r.json::<serde_json::Value>().await {
            Ok(v) => {
                let text = v.get("candidates")
                    .and_then(|c| c.get(0))
                    .and_then(|c| c.get("content"))
                    .and_then(|c| c.get("parts"))
                    .and_then(|p| p.get(0))
                    .and_then(|p| p.get("text"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("");
                parse_model_output(provider, model, text, t.elapsed().as_millis())
            }
            Err(e) => ModelTradeIdea::err(provider, model, format!("Gemini decode: {}", e), t.elapsed().as_millis()),
        },
        Ok(r) => {
            let code = r.status();
            let detail = r.text().await.unwrap_or_default();
            let detail = detail.chars().take(160).collect::<String>();
            ModelTradeIdea::err(provider, model, format!("Gemini HTTP {} {}", code, detail), t.elapsed().as_millis())
        }
        Err(e) => {
            let msg = if e.is_timeout() { "timed out".to_string() } else { format!("Gemini: {}", e) };
            ModelTradeIdea::err(provider, model, msg, t.elapsed().as_millis())
        }
    }
}

/// Call the local `claude` CLI in single-shot print mode (no agent, no tools)
/// and return the model's raw text. System + user are concatenated and piped via
/// stdin — passing them as args trips Rust's .cmd "unsafe argument" guard on
/// Windows because of `{}"`.
pub async fn claude_cli_raw(model: &str, system: &str, user: &str) -> Result<String, String> {
    use tokio::io::AsyncWriteExt;
    // Resolve the claude binary. The npm install ships `claude.cmd`; the native
    // installer ships `claude.exe` (e.g. ~/.local/bin/claude.exe). Try each in
    // turn so we work regardless of how the user installed the CLI.
    let candidates: &[&str] = if cfg!(windows) {
        &["claude.cmd", "claude.exe", "claude"]
    } else {
        &["claude"]
    };
    let prompt = format!("{}\n\n---\n\n{}", system, user);

    let mut child = None;
    let mut last_err = String::new();
    for bin in candidates {
        let mut cmd = tokio::process::Command::new(bin);
        cmd.args(["-p", "--model", model, "--output-format", "json"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        { cmd.creation_flags(0x08000000); }

        match cmd.spawn() {
            Ok(c) => { child = Some(c); break; }
            Err(e) => { last_err = format!("{} ({})", e, bin); }
        }
    }
    let mut child = child.ok_or_else(|| format!("claude CLI spawn: {}", last_err))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(prompt.as_bytes()).await;
        let _ = stdin.shutdown().await;
    }
    let out = match tokio::time::timeout(CLAUDE_CLI_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(format!("claude CLI: {}", e)),
        Err(_) => return Err("timed out".into()),
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    // `--output-format json` wraps the answer: {"type":"result","result":"…"}.
    let inner = serde_json::from_str::<serde_json::Value>(stdout.trim())
        .ok()
        .and_then(|v| v.get("result").and_then(|r| r.as_str()).map(String::from))
        .unwrap_or_else(|| stdout.to_string());
    Ok(inner)
}

/// Single-shot Claude trade-idea call → structured `ModelTradeIdea`.
pub async fn run_claude_cli(provider: &str, model: &str, system: &str, user: &str) -> ModelTradeIdea {
    let t = Instant::now();
    match claude_cli_raw(model, system, user).await {
        Ok(text) => parse_model_output(provider, model, &text, t.elapsed().as_millis()),
        Err(e) => ModelTradeIdea::err(provider, model, e, t.elapsed().as_millis()),
    }
}

/// System prompt for reviewing an existing resting pending order.
pub const ORDER_REVIEW_PROMPT: &str = r#"You are a professional XAUUSD (gold) intraday day trader reviewing a RESTING pending order you placed earlier, to decide whether it is still worth keeping.

You are given the order (side, entry, stop, target) and a fresh live snapshot (price, VWAP, 8 EMA, session high/low, prior-day levels, recent M1/M5 candles, calendar, headlines).

Decide KEEP or CANCEL. Lean CANCEL when: price has run far from the entry and the pullback/breakout premise is stale; price has flipped to the wrong side of VWAP for this direction; the trend or news has turned against the trade; a tier-1 event is imminent; or the original setup clearly no longer applies. Lean KEEP when the premise is intact and price is still working toward a valid entry with the R:R preserved.

Output ONLY a JSON object — no prose, no markdown, no code fences, no <think>:
{"recommendation":"KEEP|CANCEL","confidence":"low|medium|high","reason":"<=240 chars: what changed and why keep/cancel"}"#;

/// Parsed order-review verdict.
#[derive(Serialize, Clone, Debug)]
pub struct OrderReview {
    pub ok: bool,
    pub recommendation: Option<String>,
    pub confidence: Option<String>,
    pub reason: Option<String>,
    pub error: Option<String>,
    pub duration_ms: u128,
}

#[derive(Deserialize)]
struct ReviewJson {
    recommendation: Option<String>,
    #[serde(default)]
    confidence: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

/// System prompt for managing an OPEN position.
pub const POSITION_REVIEW_PROMPT: &str = r#"You are a professional XAUUSD (gold) intraday day trader managing an OPEN position. Given the position (side, entry, size, current SL, current TP) and a fresh live snapshot (price, VWAP, 8 EMA, session/prior-day levels, recent candles, calendar, headlines), decide: HOLD as-is, ADJUST the stop/target, or CLOSE now.

- CLOSE when the thesis is invalidated, structure/momentum has turned against the position, the move looks exhausted at a logical level, or a tier-1 event poses outsized risk to an open trade.
- ADJUST when the trade is working and you should trail/raise the stop (lock gains or move to breakeven) and/or extend/trim the target.
- HOLD when the trade is progressing and the current SL/TP remain appropriate.

Output ONLY a JSON object — no prose, no markdown, no code fences, no <think>:
{"action":"HOLD|ADJUST|CLOSE","new_sl":number|null,"new_tp":number|null,"confidence":"low|medium|high","reason":"<=240 chars"}
For HOLD and CLOSE set new_sl and new_tp to null. For ADJUST give the new absolute SL and/or TP (null for the one you don't change)."#;

/// Parsed open-position management verdict.
#[derive(Serialize, Clone, Debug)]
pub struct PositionReview {
    pub ok: bool,
    pub action: Option<String>,
    pub new_sl: Option<f64>,
    pub new_tp: Option<f64>,
    pub confidence: Option<String>,
    pub reason: Option<String>,
    pub error: Option<String>,
    pub duration_ms: u128,
}

#[derive(Deserialize)]
struct PositionReviewJson {
    action: Option<String>,
    #[serde(default)] new_sl: Option<f64>,
    #[serde(default)] new_tp: Option<f64>,
    #[serde(default)] confidence: Option<String>,
    #[serde(default)] reason: Option<String>,
}

/// Review an open position with Claude (single-shot). Returns HOLD/ADJUST/CLOSE.
pub async fn review_position_with_claude(model: &str, user: &str) -> PositionReview {
    let t = Instant::now();
    match claude_cli_raw(model, POSITION_REVIEW_PROMPT, user).await {
        Ok(text) => {
            let ms = t.elapsed().as_millis();
            match extract_json(&text).and_then(|j| serde_json::from_str::<PositionReviewJson>(&j).ok()) {
                Some(r) => PositionReview {
                    ok: true,
                    action: r.action.map(|s| s.to_uppercase()),
                    new_sl: r.new_sl,
                    new_tp: r.new_tp,
                    confidence: r.confidence,
                    reason: r.reason,
                    error: None,
                    duration_ms: ms,
                },
                None => PositionReview {
                    ok: false, action: None, new_sl: None, new_tp: None, confidence: None,
                    reason: None, error: Some("couldn't parse review JSON".into()), duration_ms: ms,
                },
            }
        }
        Err(e) => PositionReview {
            ok: false, action: None, new_sl: None, new_tp: None, confidence: None,
            reason: None, error: Some(e), duration_ms: t.elapsed().as_millis(),
        },
    }
}

/// Review a pending order with Claude (single-shot). Returns KEEP/CANCEL + reason.
pub async fn review_order_with_claude(model: &str, user: &str) -> OrderReview {
    let t = Instant::now();
    match claude_cli_raw(model, ORDER_REVIEW_PROMPT, user).await {
        Ok(text) => {
            let ms = t.elapsed().as_millis();
            match extract_json(&text).and_then(|j| serde_json::from_str::<ReviewJson>(&j).ok()) {
                Some(r) => OrderReview {
                    ok: true,
                    recommendation: r.recommendation.map(|s| s.to_uppercase()),
                    confidence: r.confidence,
                    reason: r.reason,
                    error: None,
                    duration_ms: ms,
                },
                None => OrderReview {
                    ok: false, recommendation: None, confidence: None, reason: None,
                    error: Some("couldn't parse review JSON".into()), duration_ms: ms,
                },
            }
        }
        Err(e) => OrderReview {
            ok: false, recommendation: None, confidence: None, reason: None,
            error: Some(e), duration_ms: t.elapsed().as_millis(),
        },
    }
}
