use serde::{Deserialize, Serialize};

/// A chunk of streamed content from the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamChunk {
    pub delta: String,
    pub finish_reason: Option<FinishReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    ToolUse,
    MaxTokens,
}

/// Events emitted during streaming.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// Text delta.
    Delta(String),
    /// Tool call started.
    ToolCallStart { id: String, name: String },
    /// Tool call argument delta.
    ToolCallDelta { id: String, arguments_delta: String },
    /// Stream complete.
    Done(FinishReason),
    /// Error during streaming.
    Error(String),
}
