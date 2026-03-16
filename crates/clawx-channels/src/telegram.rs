use async_trait::async_trait;
use clawx_core::{Error, Result};
use crate::channel::{Channel, ChannelMessage};
use reqwest::Client;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use zeroize::Zeroizing;

/// Telegram Bot API channel.
///
/// Features synthesized from IronClaw/OpenFang/ZeroClaw:
/// - Long polling with exponential backoff (OpenFang)
/// - Streaming draft edits (ZeroClaw 3-phase pattern)
/// - Zeroizing token storage (OpenFang)
/// - Markdown → Telegram HTML conversion (ZeroClaw)
/// - Smart message chunking at 4096 chars (ZeroClaw)
/// - Allowed user filtering (all three)
/// - Typing indicator on message receipt (all three)
pub struct TelegramChannel {
    client: Client,
    token: Zeroizing<String>,
    api_base: String,
    allowed_users: Vec<String>,
    mention_only: bool,
    bot_username: tokio::sync::RwLock<Option<String>>,
    poll_offset: AtomicI64,
    draft_interval_ms: u64,
}

/// Configuration for the Telegram channel.
#[derive(Debug, Clone)]
pub struct TelegramConfig {
    pub bot_token: String,
    /// Telegram user IDs or usernames allowed to interact. Empty = allow all.
    pub allowed_users: Vec<String>,
    /// In groups, only respond when @mentioned.
    pub mention_only: bool,
    /// Custom Bot API base URL (for self-hosted Bot API servers).
    pub api_base: Option<String>,
    /// Minimum interval between draft edits (ms). Prevents Telegram rate limits.
    pub draft_interval_ms: u64,
}

impl Default for TelegramConfig {
    fn default() -> Self {
        Self {
            bot_token: String::new(),
            allowed_users: Vec::new(),
            mention_only: true,
            api_base: None,
            draft_interval_ms: 750,
        }
    }
}

impl TelegramChannel {
    pub fn new(config: TelegramConfig) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .expect("failed to build HTTP client");

        let api_base = config
            .api_base
            .unwrap_or_else(|| "https://api.telegram.org".into());

        Self {
            client,
            token: Zeroizing::new(config.bot_token),
            api_base,
            allowed_users: config.allowed_users,
            mention_only: config.mention_only,
            bot_username: tokio::sync::RwLock::new(None),
            poll_offset: AtomicI64::new(0),
            draft_interval_ms: config.draft_interval_ms,
        }
    }

    fn api_url(&self, method: &str) -> String {
        format!("{}/bot{}/{}", self.api_base, self.token.as_str(), method)
    }

    /// Call Telegram Bot API and return the result.
    async fn api_call(&self, method: &str, body: &Value) -> Result<Value> {
        let url = self.api_url(method);
        let resp = self
            .client
            .post(&url)
            .json(body)
            .send()
            .await
            .map_err(|e| Error::Provider(format!("telegram api {method}: {e}")))?;

        let status = resp.status();
        let data: Value = resp
            .json()
            .await
            .map_err(|e| Error::Provider(format!("telegram parse: {e}")))?;

        if !data["ok"].as_bool().unwrap_or(false) {
            let desc = data["description"].as_str().unwrap_or("unknown error");
            let error_code = data["error_code"].as_u64().unwrap_or(0);

            // Rate limit handling
            if error_code == 429 {
                let retry_after = data["parameters"]["retry_after"].as_u64().unwrap_or(5);
                warn!(retry_after, "telegram rate limited");
                tokio::time::sleep(Duration::from_secs(retry_after)).await;
                return Err(Error::RateLimited { retry_after_secs: retry_after });
            }

            return Err(Error::Provider(format!("telegram {method} {status}: {desc}")));
        }

        Ok(data["result"].clone())
    }

    /// Fetch bot info and cache username.
    pub async fn init(&self) -> Result<()> {
        let me = self.api_call("getMe", &json!({})).await?;
        let username = me["username"].as_str().unwrap_or_default().to_string();
        info!(username, "telegram bot initialized");
        *self.bot_username.write().await = Some(username);
        Ok(())
    }

    /// Start long-polling and emit messages to the channel.
    pub async fn poll_loop(&self, tx: mpsc::Sender<ChannelMessage>) -> Result<()> {
        let mut backoff_ms: u64 = 0;

        loop {
            if backoff_ms > 0 {
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
            }

            let offset = self.poll_offset.load(Ordering::Relaxed);
            let body = json!({
                "offset": offset,
                "timeout": 30,
                "allowed_updates": ["message"],
            });

            match self.api_call("getUpdates", &body).await {
                Ok(updates) => {
                    backoff_ms = 0;

                    if let Some(arr) = updates.as_array() {
                        for update in arr {
                            let update_id = update["update_id"].as_i64().unwrap_or(0);
                            self.poll_offset.store(update_id + 1, Ordering::Relaxed);

                            if let Some(msg) = self.parse_update(update).await {
                                if tx.send(msg).await.is_err() {
                                    info!("telegram poll loop: receiver dropped, stopping");
                                    return Ok(());
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    // Exponential backoff: 1s → 2s → 4s → ... → 60s max
                    backoff_ms = if backoff_ms == 0 { 1000 } else { (backoff_ms * 2).min(60_000) };
                    error!(error = %e, backoff_ms, "telegram poll error");
                }
            }
        }
    }

    /// Parse a Telegram update into a ChannelMessage, filtering by allowed users.
    async fn parse_update(&self, update: &Value) -> Option<ChannelMessage> {
        let msg = update.get("message")?;
        let text = msg["text"].as_str().unwrap_or_default();
        let chat_id = msg["chat"]["id"].as_i64()?;
        let message_id = msg["message_id"].as_i64()?;
        let from = msg.get("from")?;
        let user_id = from["id"].as_i64()?.to_string();
        let username = from["username"].as_str().unwrap_or_default();
        let first_name = from["first_name"].as_str().unwrap_or("unknown");

        // Filter allowed users
        if !self.allowed_users.is_empty()
            && !self.allowed_users.contains(&user_id)
            && !self.allowed_users.iter().any(|u| u == username)
        {
            debug!(user_id, username, "ignoring message from non-allowed user");
            return None;
        }

        // Group mention filter
        let is_group = msg["chat"]["type"].as_str().map_or(false, |t| t != "private");
        if is_group && self.mention_only {
            let bot_username = self.bot_username.read().await;
            if let Some(ref bot) = *bot_username {
                let mention = format!("@{bot}");
                if !text.contains(&mention) {
                    return None;
                }
            }
        }

        // Strip @mention prefix from text
        let clean_text = {
            let bot_username = self.bot_username.read().await;
            let mut t = text.to_string();
            if let Some(ref bot) = *bot_username {
                t = t.replace(&format!("@{bot}"), "").trim().to_string();
            }
            t
        };

        if clean_text.is_empty() {
            return None;
        }

        let sender = if username.is_empty() {
            first_name.to_string()
        } else {
            format!("{first_name} (@{username})")
        };

        Some(ChannelMessage {
            sender,
            text: clean_text,
            chat_id: chat_id.to_string(),
            message_id: message_id.to_string(),
            metadata: json!({
                "user_id": user_id,
                "username": username,
                "chat_type": msg["chat"]["type"],
            }),
        })
    }
}

// ---------------------------------------------------------------------------
// Channel trait implementation — streaming draft support
// ---------------------------------------------------------------------------

#[async_trait]
impl Channel for TelegramChannel {
    async fn send_draft(&self, chat_id: &str, text: &str) -> Result<Option<String>> {
        let html = markdown_to_telegram_html(text);
        let chunks = chunk_message(&html, 4096);
        let first = chunks.first().map_or("...", |c| c.as_str());

        let result = self
            .api_call("sendMessage", &json!({
                "chat_id": chat_id,
                "text": first,
                "parse_mode": "HTML",
            }))
            .await?;

        let msg_id = result["message_id"].as_i64().map(|id| id.to_string());
        Ok(msg_id)
    }

    async fn update_draft(&self, chat_id: &str, message_id: &str, text: &str) -> Result<()> {
        let html = markdown_to_telegram_html(text);
        // Truncate to 4096 for edit (don't chunk edits)
        let html = if html.len() > 4096 { &html[..4096] } else { &html };

        let msg_id: i64 = message_id.parse().map_err(|_| Error::Other("invalid message_id".into()))?;

        // Telegram returns error if text is unchanged — ignore it
        match self
            .api_call("editMessageText", &json!({
                "chat_id": chat_id,
                "message_id": msg_id,
                "text": html,
                "parse_mode": "HTML",
            }))
            .await
        {
            Ok(_) => Ok(()),
            Err(Error::Provider(msg)) if msg.contains("message is not modified") => Ok(()),
            Err(e) => Err(e),
        }
    }

    async fn finalize(&self, chat_id: &str, message_id: &str, text: &str) -> Result<()> {
        let html = markdown_to_telegram_html(text);
        let chunks = chunk_message(&html, 4096);

        if let Some(first) = chunks.first() {
            // Edit the draft with the first chunk
            let _ = self.update_draft(chat_id, message_id, first).await;
        }

        // Send remaining chunks as new messages
        for chunk in chunks.iter().skip(1) {
            self.api_call("sendMessage", &json!({
                "chat_id": chat_id,
                "text": chunk,
                "parse_mode": "HTML",
            }))
            .await?;
        }

        Ok(())
    }

    async fn send(&self, chat_id: &str, text: &str) -> Result<()> {
        let html = markdown_to_telegram_html(text);
        let chunks = chunk_message(&html, 4096);

        for chunk in &chunks {
            self.api_call("sendMessage", &json!({
                "chat_id": chat_id,
                "text": chunk,
                "parse_mode": "HTML",
            }))
            .await?;
        }

        Ok(())
    }

    async fn send_typing(&self, chat_id: &str) -> Result<()> {
        let _ = self
            .api_call("sendChatAction", &json!({
                "chat_id": chat_id,
                "action": "typing",
            }))
            .await;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Markdown → Telegram HTML conversion (ZeroClaw pattern)
// ---------------------------------------------------------------------------

/// Convert Markdown-ish text to Telegram-safe HTML.
pub fn markdown_to_telegram_html(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut in_code_block = false;
    let mut code_lang = String::new();

    for line in input.lines() {
        if line.starts_with("```") {
            if in_code_block {
                output.push_str("</code></pre>\n");
                in_code_block = false;
                code_lang.clear();
            } else {
                code_lang = line.trim_start_matches('`').trim().to_string();
                if code_lang.is_empty() {
                    output.push_str("<pre><code>");
                } else {
                    output.push_str(&format!(r#"<pre><code class="language-{code_lang}">"#));
                }
                in_code_block = true;
            }
            continue;
        }

        if in_code_block {
            output.push_str(&escape_html(line));
            output.push('\n');
            continue;
        }

        let processed = process_inline_markdown(line);
        output.push_str(&processed);
        output.push('\n');
    }

    if in_code_block {
        output.push_str("</code></pre>\n");
    }

    output.trim_end().to_string()
}

fn process_inline_markdown(line: &str) -> String {
    // Headers → bold
    if let Some(stripped) = line.strip_prefix("### ") {
        return format!("<b>{}</b>", escape_html(stripped));
    }
    if let Some(stripped) = line.strip_prefix("## ") {
        return format!("<b>{}</b>", escape_html(stripped));
    }
    if let Some(stripped) = line.strip_prefix("# ") {
        return format!("<b>{}</b>", escape_html(stripped));
    }

    let mut result = String::new();
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // Inline code: `text`
        if chars[i] == '`' {
            if let Some(end) = find_closing(&chars, i + 1, '`') {
                let code_text: String = chars[i + 1..end].iter().collect();
                result.push_str(&format!("<code>{}</code>", escape_html(&code_text)));
                i = end + 1;
                continue;
            }
        }

        // Bold: **text**
        if i + 1 < len && chars[i] == '*' && chars[i + 1] == '*' {
            if let Some(end) = find_double_closing(&chars, i + 2, '*') {
                let inner: String = chars[i + 2..end].iter().collect();
                result.push_str(&format!("<b>{}</b>", escape_html(&inner)));
                i = end + 2;
                continue;
            }
        }

        // Italic: *text*
        if chars[i] == '*' {
            if let Some(end) = find_closing(&chars, i + 1, '*') {
                let inner: String = chars[i + 1..end].iter().collect();
                result.push_str(&format!("<i>{}</i>", escape_html(&inner)));
                i = end + 1;
                continue;
            }
        }

        // Strikethrough: ~~text~~
        if i + 1 < len && chars[i] == '~' && chars[i + 1] == '~' {
            if let Some(end) = find_double_closing(&chars, i + 2, '~') {
                let inner: String = chars[i + 2..end].iter().collect();
                result.push_str(&format!("<s>{}</s>", escape_html(&inner)));
                i = end + 2;
                continue;
            }
        }

        // Links: [text](url)
        if chars[i] == '[' {
            if let Some((link_text, url, end_pos)) = parse_link(&chars, i) {
                result.push_str(&format!(
                    r#"<a href="{}">{}</a>"#,
                    escape_html(&url),
                    escape_html(&link_text)
                ));
                i = end_pos;
                continue;
            }
        }

        // Escape HTML entities for plain text
        match chars[i] {
            '&' => result.push_str("&amp;"),
            '<' => result.push_str("&lt;"),
            '>' => result.push_str("&gt;"),
            c => result.push(c),
        }
        i += 1;
    }

    result
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn find_closing(chars: &[char], start: usize, marker: char) -> Option<usize> {
    for i in start..chars.len() {
        if chars[i] == marker {
            return Some(i);
        }
    }
    None
}

fn find_double_closing(chars: &[char], start: usize, marker: char) -> Option<usize> {
    for i in start..chars.len().saturating_sub(1) {
        if chars[i] == marker && chars[i + 1] == marker {
            return Some(i);
        }
    }
    None
}

fn parse_link(chars: &[char], start: usize) -> Option<(String, String, usize)> {
    // [text](url)
    let text_end = find_closing(chars, start + 1, ']')?;
    if text_end + 1 >= chars.len() || chars[text_end + 1] != '(' {
        return None;
    }
    let url_end = find_closing(chars, text_end + 2, ')')?;
    let text: String = chars[start + 1..text_end].iter().collect();
    let url: String = chars[text_end + 2..url_end].iter().collect();
    Some((text, url, url_end + 1))
}

// ---------------------------------------------------------------------------
// Smart message chunking (ZeroClaw pattern)
// ---------------------------------------------------------------------------

/// Split a message into chunks at word boundaries, respecting Telegram's limit.
fn chunk_message(text: &str, max_len: usize) -> Vec<String> {
    if text.len() <= max_len {
        return vec![text.to_string()];
    }

    let overhead = 30; // "(continues...)" and "(continued)" markers
    let effective_max = max_len - overhead;
    let mut chunks = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        if remaining.len() <= max_len {
            if chunks.is_empty() {
                chunks.push(remaining.to_string());
            } else {
                chunks.push(format!("(continued)\n{remaining}"));
            }
            break;
        }

        let slice = &remaining[..effective_max];
        // Find best split point: newline > space > hard cut
        let split_at = slice
            .rfind('\n')
            .or_else(|| slice.rfind(' '))
            .unwrap_or(effective_max);

        let chunk = &remaining[..split_at];
        if chunks.is_empty() {
            chunks.push(format!("{chunk}\n(continues...)"));
        } else {
            chunks.push(format!("(continued)\n{chunk}\n(continues...)"));
        }

        remaining = remaining[split_at..].trim_start();
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_markdown_to_html_bold() {
        let input = "This is **bold** text";
        let output = markdown_to_telegram_html(input);
        assert!(output.contains("<b>bold</b>"));
    }

    #[test]
    fn test_markdown_to_html_code_block() {
        let input = "```rust\nfn main() {}\n```";
        let output = markdown_to_telegram_html(input);
        assert!(output.contains(r#"<pre><code class="language-rust">"#));
        assert!(output.contains("fn main() {}"));
    }

    #[test]
    fn test_markdown_to_html_inline_code() {
        let input = "Use `cargo build` to compile";
        let output = markdown_to_telegram_html(input);
        assert!(output.contains("<code>cargo build</code>"));
    }

    #[test]
    fn test_chunk_short_message() {
        let chunks = chunk_message("Hello", 4096);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "Hello");
    }

    #[test]
    fn test_chunk_long_message() {
        let long_text = "word ".repeat(1000); // ~5000 chars
        let chunks = chunk_message(&long_text, 100);
        assert!(chunks.len() > 1);
        assert!(chunks[0].contains("(continues...)"));
        assert!(chunks[1].contains("(continued)"));
    }

    #[test]
    fn test_html_escaping() {
        let input = "Use <script> & \"quotes\"";
        let output = markdown_to_telegram_html(input);
        assert!(output.contains("&lt;script&gt;"));
        assert!(output.contains("&amp;"));
    }
}
