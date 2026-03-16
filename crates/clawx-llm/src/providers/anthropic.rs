use async_trait::async_trait;
use clawx_core::message::{ContentBlock, ToolCall};
use clawx_core::{Error, Message, Result, ToolDefinition};
use crate::provider::Provider;
use crate::stream::{FinishReason, StreamEvent};
use reqwest::Client;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tracing::debug;
use zeroize::Zeroizing;

/// Anthropic Messages API provider.
pub struct AnthropicProvider {
    client: Client,
    api_key: Zeroizing<String>,
    model: String,
    base_url: String,
    max_tokens: usize,
    temperature: f32,
}

impl AnthropicProvider {
    pub fn new(
        api_key: String,
        model: String,
        base_url: Option<String>,
        max_tokens: usize,
        temperature: f32,
    ) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .expect("failed to build HTTP client");
        Self {
            client,
            api_key: Zeroizing::new(api_key),
            model,
            base_url: base_url.unwrap_or_else(|| "https://api.anthropic.com".into()),
            max_tokens,
            temperature,
        }
    }

    fn build_request_body(&self, messages: &[Message], tools: &[ToolDefinition]) -> Value {
        let mut system_text = String::new();
        let mut api_messages = Vec::new();

        for msg in messages {
            match msg.role {
                clawx_core::message::MessageRole::System => {
                    system_text.push_str(&msg.text());
                    system_text.push('\n');
                }
                clawx_core::message::MessageRole::User => {
                    api_messages.push(json!({
                        "role": "user",
                        "content": msg.text(),
                    }));
                }
                clawx_core::message::MessageRole::Assistant => {
                    let mut content = Vec::new();
                    for block in &msg.content {
                        match block {
                            ContentBlock::Text { text } => {
                                content.push(json!({"type": "text", "text": text}));
                            }
                            ContentBlock::ToolUse(tc) => {
                                content.push(json!({
                                    "type": "tool_use",
                                    "id": tc.id,
                                    "name": tc.name,
                                    "input": tc.arguments,
                                }));
                            }
                            _ => {}
                        }
                    }
                    api_messages.push(json!({"role": "assistant", "content": content}));
                }
                clawx_core::message::MessageRole::Tool => {
                    let mut content = Vec::new();
                    for block in &msg.content {
                        if let ContentBlock::ToolResult(tr) = block {
                            content.push(json!({
                                "type": "tool_result",
                                "tool_use_id": tr.call_id,
                                "content": tr.output,
                                "is_error": tr.is_error,
                            }));
                        }
                    }
                    api_messages.push(json!({"role": "user", "content": content}));
                }
            }
        }

        let mut body = json!({
            "model": self.model,
            "max_tokens": self.max_tokens,
            "temperature": self.temperature,
            "messages": api_messages,
        });

        if !system_text.is_empty() {
            body["system"] = json!(system_text.trim());
        }

        if !tools.is_empty() {
            let tool_defs: Vec<Value> = tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.parameters,
                    })
                })
                .collect();
            body["tools"] = json!(tool_defs);
        }

        body
    }

    fn parse_response(body: &Value) -> Result<Message> {
        let content_blocks = body["content"]
            .as_array()
            .ok_or_else(|| Error::Provider("missing content array".into()))?;

        let mut content = Vec::new();
        for block in content_blocks {
            match block["type"].as_str() {
                Some("text") => {
                    let text = block["text"].as_str().unwrap_or_default().to_string();
                    content.push(ContentBlock::Text { text });
                }
                Some("tool_use") => {
                    content.push(ContentBlock::ToolUse(ToolCall {
                        id: block["id"].as_str().unwrap_or_default().to_string(),
                        name: block["name"].as_str().unwrap_or_default().to_string(),
                        arguments: block["input"].clone(),
                    }));
                }
                _ => {}
            }
        }

        Ok(Message {
            id: body["id"].as_str().unwrap_or_default().to_string(),
            role: clawx_core::message::MessageRole::Assistant,
            content,
            token_estimate: body["usage"]["output_tokens"].as_u64().map(|t| t as usize),
        })
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    async fn complete(&self, messages: &[Message], tools: &[ToolDefinition]) -> Result<Message> {
        let body = self.build_request_body(messages, tools);
        let url = format!("{}/v1/messages", self.base_url);

        debug!(url, model = %self.model, "calling Anthropic API");

        let resp = self
            .client
            .post(&url)
            .header("x-api-key", self.api_key.as_str())
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Provider(e.to_string()))?;

        let status = resp.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse().ok())
                .unwrap_or(30);
            return Err(Error::RateLimited { retry_after_secs: retry_after });
        }

        let response_body: Value = resp
            .json()
            .await
            .map_err(|e| Error::Provider(e.to_string()))?;

        if !status.is_success() {
            let err_msg = response_body["error"]["message"]
                .as_str()
                .unwrap_or("unknown error");
            return Err(Error::Provider(format!("{status}: {err_msg}")));
        }

        Self::parse_response(&response_body)
    }

    async fn stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        tx: mpsc::Sender<StreamEvent>,
    ) -> Result<Message> {
        let mut body = self.build_request_body(messages, tools);
        body["stream"] = json!(true);
        let url = format!("{}/v1/messages", self.base_url);

        let resp = self
            .client
            .post(&url)
            .header("x-api-key", self.api_key.as_str())
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Provider(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_body: Value = resp.json().await.unwrap_or_default();
            let err_msg = err_body["error"]["message"]
                .as_str()
                .unwrap_or("unknown error");
            return Err(Error::Provider(format!("{status}: {err_msg}")));
        }

        // Process SSE stream
        let mut full_text = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut current_tool_args = String::new();
        let mut finish_reason = FinishReason::Stop;
        let mut msg_id = String::new();

        let text = resp.text().await.map_err(|e| Error::Provider(e.to_string()))?;
        for line in text.lines() {
            let line = line.trim();
            if !line.starts_with("data: ") {
                continue;
            }
            let data = &line[6..];
            if data == "[DONE]" {
                break;
            }
            let event: Value = match serde_json::from_str(data) {
                Ok(v) => v,
                Err(_) => continue,
            };

            match event["type"].as_str() {
                Some("message_start") => {
                    msg_id = event["message"]["id"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                }
                Some("content_block_start") => {
                    if event["content_block"]["type"].as_str() == Some("tool_use") {
                        let id = event["content_block"]["id"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string();
                        let name = event["content_block"]["name"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string();
                        current_tool_args.clear();
                        let _ = tx.send(StreamEvent::ToolCallStart {
                            id: id.clone(),
                            name: name.clone(),
                        }).await;
                        tool_calls.push(ToolCall {
                            id,
                            name,
                            arguments: Value::Null,
                        });
                    }
                }
                Some("content_block_delta") => {
                    match event["delta"]["type"].as_str() {
                        Some("text_delta") => {
                            let text_delta = event["delta"]["text"]
                                .as_str()
                                .unwrap_or_default();
                            full_text.push_str(text_delta);
                            let _ = tx.send(StreamEvent::Delta(text_delta.to_string())).await;
                        }
                        Some("input_json_delta") => {
                            let partial = event["delta"]["partial_json"]
                                .as_str()
                                .unwrap_or_default();
                            current_tool_args.push_str(partial);
                            if let Some(tc) = tool_calls.last() {
                                let _ = tx.send(StreamEvent::ToolCallDelta {
                                    id: tc.id.clone(),
                                    arguments_delta: partial.to_string(),
                                }).await;
                            }
                        }
                        _ => {}
                    }
                }
                Some("content_block_stop") => {
                    if let Some(tc) = tool_calls.last_mut() {
                        if tc.arguments.is_null() && !current_tool_args.is_empty() {
                            tc.arguments = serde_json::from_str(&current_tool_args)
                                .unwrap_or(Value::String(current_tool_args.clone()));
                            current_tool_args.clear();
                        }
                    }
                }
                Some("message_delta") => {
                    match event["delta"]["stop_reason"].as_str() {
                        Some("tool_use") => finish_reason = FinishReason::ToolUse,
                        Some("max_tokens") => finish_reason = FinishReason::MaxTokens,
                        _ => finish_reason = FinishReason::Stop,
                    }
                }
                _ => {}
            }
        }

        let _ = tx.send(StreamEvent::Done(finish_reason)).await;

        let mut content = Vec::new();
        if !full_text.is_empty() {
            content.push(ContentBlock::Text { text: full_text });
        }
        for tc in tool_calls {
            content.push(ContentBlock::ToolUse(tc));
        }

        Ok(Message {
            id: msg_id,
            role: clawx_core::message::MessageRole::Assistant,
            content,
            token_estimate: None,
        })
    }

    fn name(&self) -> &str { "anthropic" }
    fn model(&self) -> &str { &self.model }
}
