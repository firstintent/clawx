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

/// OpenAI-compatible provider. Works with OpenAI, OpenRouter, Ollama, etc.
pub struct OpenAiProvider {
    client: Client,
    api_key: Zeroizing<String>,
    model: String,
    base_url: String,
    max_tokens: usize,
    temperature: f32,
}

impl OpenAiProvider {
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
            base_url: base_url.unwrap_or_else(|| "https://api.openai.com".into()),
            max_tokens,
            temperature,
        }
    }

    fn build_request_body(&self, messages: &[Message], tools: &[ToolDefinition]) -> Value {
        let api_messages: Vec<Value> = messages
            .iter()
            .map(|msg| {
                let role = match msg.role {
                    clawx_core::message::MessageRole::System => "system",
                    clawx_core::message::MessageRole::User => "user",
                    clawx_core::message::MessageRole::Assistant => "assistant",
                    clawx_core::message::MessageRole::Tool => "tool",
                };

                match msg.role {
                    clawx_core::message::MessageRole::Assistant => {
                        let mut obj = json!({"role": role});
                        let text = msg.text();
                        if !text.is_empty() {
                            obj["content"] = json!(text);
                        }
                        let tcs = msg.tool_calls();
                        if !tcs.is_empty() {
                            obj["tool_calls"] = json!(tcs.iter().map(|tc| json!({
                                "id": tc.id,
                                "type": "function",
                                "function": {
                                    "name": tc.name,
                                    "arguments": tc.arguments.to_string(),
                                }
                            })).collect::<Vec<_>>());
                        }
                        obj
                    }
                    clawx_core::message::MessageRole::Tool => {
                        let tool_results: Vec<&clawx_core::message::ToolResult> = msg.content.iter()
                            .filter_map(|c| match c {
                                ContentBlock::ToolResult(tr) => Some(tr),
                                _ => None,
                            })
                            .collect();
                        if let Some(tr) = tool_results.first() {
                            json!({
                                "role": "tool",
                                "tool_call_id": tr.call_id,
                                "content": tr.output,
                            })
                        } else {
                            json!({"role": role, "content": msg.text()})
                        }
                    }
                    _ => json!({"role": role, "content": msg.text()}),
                }
            })
            .collect();

        let mut body = json!({
            "model": self.model,
            "max_tokens": self.max_tokens,
            "temperature": self.temperature,
            "messages": api_messages,
        });

        if !tools.is_empty() {
            let tool_defs: Vec<Value> = tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        }
                    })
                })
                .collect();
            body["tools"] = json!(tool_defs);
        }

        body
    }

    fn parse_response(body: &Value) -> Result<Message> {
        let choice = &body["choices"][0];
        let msg = &choice["message"];
        let mut content = Vec::new();

        if let Some(text) = msg["content"].as_str() {
            if !text.is_empty() {
                content.push(ContentBlock::Text { text: text.to_string() });
            }
        }

        if let Some(tool_calls) = msg["tool_calls"].as_array() {
            for tc in tool_calls {
                let args_str = tc["function"]["arguments"].as_str().unwrap_or("{}");
                let arguments: Value = serde_json::from_str(args_str).unwrap_or(json!({}));
                content.push(ContentBlock::ToolUse(ToolCall {
                    id: tc["id"].as_str().unwrap_or_default().to_string(),
                    name: tc["function"]["name"].as_str().unwrap_or_default().to_string(),
                    arguments,
                }));
            }
        }

        Ok(Message {
            id: body["id"].as_str().unwrap_or_default().to_string(),
            role: clawx_core::message::MessageRole::Assistant,
            content,
            token_estimate: body["usage"]["completion_tokens"].as_u64().map(|t| t as usize),
        })
    }
}

#[async_trait]
impl Provider for OpenAiProvider {
    async fn complete(&self, messages: &[Message], tools: &[ToolDefinition]) -> Result<Message> {
        let body = self.build_request_body(messages, tools);
        let url = format!("{}/v1/chat/completions", self.base_url);

        let key: &str = &self.api_key;
        debug!(url, model = %self.model, key_len = key.len(), "calling OpenAI-compatible API");

        let resp = self
            .client
            .post(&url)
            .header("authorization", format!("Bearer {key}"))
            .header("content-type", "application/json")
            .body(serde_json::to_vec(&body).map_err(|e| Error::Provider(e.to_string()))?)
            .send()
            .await
            .map_err(|e| {
                Error::Provider(format!("reqwest send: {e:?}"))
            })?;

        let status = resp.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(Error::RateLimited { retry_after_secs: 30 });
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
        let url = format!("{}/v1/chat/completions", self.base_url);

        let resp = self
            .client
            .post(&url)
            .bearer_auth(self.api_key.as_str())
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Provider(format!("{e:#}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_body: Value = resp.json().await.unwrap_or_default();
            return Err(Error::Provider(format!("{status}: {}", err_body)));
        }

        let mut full_text = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut tool_args_buffer: std::collections::HashMap<usize, String> = std::collections::HashMap::new();
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

            if msg_id.is_empty() {
                if let Some(id) = event["id"].as_str() {
                    msg_id = id.to_string();
                }
            }

            let delta = &event["choices"][0]["delta"];

            // Text delta
            if let Some(content) = delta["content"].as_str() {
                full_text.push_str(content);
                let _ = tx.send(StreamEvent::Delta(content.to_string())).await;
            }

            // Tool call deltas
            if let Some(tcs) = delta["tool_calls"].as_array() {
                for tc_delta in tcs {
                    let idx = tc_delta["index"].as_u64().unwrap_or(0) as usize;

                    if let Some(id) = tc_delta["id"].as_str() {
                        let name = tc_delta["function"]["name"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string();
                        let _ = tx.send(StreamEvent::ToolCallStart {
                            id: id.to_string(),
                            name: name.clone(),
                        }).await;
                        while tool_calls.len() <= idx {
                            tool_calls.push(ToolCall {
                                id: String::new(),
                                name: String::new(),
                                arguments: Value::Null,
                            });
                        }
                        tool_calls[idx].id = id.to_string();
                        tool_calls[idx].name = name;
                    }

                    if let Some(args) = tc_delta["function"]["arguments"].as_str() {
                        tool_args_buffer
                            .entry(idx)
                            .or_default()
                            .push_str(args);
                        if let Some(tc) = tool_calls.get(idx) {
                            let _ = tx.send(StreamEvent::ToolCallDelta {
                                id: tc.id.clone(),
                                arguments_delta: args.to_string(),
                            }).await;
                        }
                    }
                }
            }

            // Finish reason
            if let Some(reason) = event["choices"][0]["finish_reason"].as_str() {
                finish_reason = match reason {
                    "tool_calls" => FinishReason::ToolUse,
                    "length" => FinishReason::MaxTokens,
                    _ => FinishReason::Stop,
                };
            }
        }

        // Finalize tool call arguments
        for (idx, args_str) in tool_args_buffer {
            if let Some(tc) = tool_calls.get_mut(idx) {
                tc.arguments = serde_json::from_str(&args_str).unwrap_or(json!({}));
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

    fn name(&self) -> &str { "openai" }
    fn model(&self) -> &str { &self.model }
}
