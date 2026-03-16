use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

/// A tool call requested by the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// The result of executing a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub call_id: String,
    pub output: String,
    pub is_error: bool,
}

/// Content block within a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
    ToolUse(ToolCall),
    ToolResult(ToolResult),
}

/// A conversation message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub role: MessageRole,
    pub content: Vec<ContentBlock>,
    #[serde(default)]
    pub token_estimate: Option<usize>,
}

impl Message {
    pub fn system(text: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            role: MessageRole::System,
            content: vec![ContentBlock::Text { text: text.into() }],
            token_estimate: None,
        }
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            role: MessageRole::User,
            content: vec![ContentBlock::Text { text: text.into() }],
            token_estimate: None,
        }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            role: MessageRole::Assistant,
            content: vec![ContentBlock::Text { text: text.into() }],
            token_estimate: None,
        }
    }

    pub fn tool_result(call_id: impl Into<String>, output: impl Into<String>, is_error: bool) -> Self {
        let call_id = call_id.into();
        Self {
            id: Uuid::new_v4().to_string(),
            role: MessageRole::Tool,
            content: vec![ContentBlock::ToolResult(ToolResult {
                call_id,
                output: output.into(),
                is_error,
            })],
            token_estimate: None,
        }
    }

    /// Extract text content from the message.
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|c| match c {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    /// Extract tool calls from the message.
    pub fn tool_calls(&self) -> Vec<&ToolCall> {
        self.content
            .iter()
            .filter_map(|c| match c {
                ContentBlock::ToolUse(tc) => Some(tc),
                _ => None,
            })
            .collect()
    }

    /// Estimate token count: word_count * 1.3 + 4 overhead per message.
    pub fn estimate_tokens(&self) -> usize {
        if let Some(est) = self.token_estimate {
            return est;
        }
        let text = self.text();
        let words = text.split_whitespace().count();
        (words as f64 * 1.3) as usize + 4
    }
}
