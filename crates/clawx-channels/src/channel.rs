use async_trait::async_trait;
use clawx_core::Result;

/// An incoming message from a channel.
#[derive(Debug, Clone)]
pub struct ChannelMessage {
    pub sender: String,
    pub text: String,
    pub chat_id: String,
    pub message_id: String,
    /// Platform-specific metadata.
    pub metadata: serde_json::Value,
}

/// A channel bridges external messaging platforms to the agent.
#[async_trait]
pub trait Channel: Send + Sync {
    /// Send an initial draft response (for streaming updates).
    /// Returns a message ID that can be used for `update_draft`.
    async fn send_draft(&self, chat_id: &str, text: &str) -> Result<Option<String>>;

    /// Update an existing draft message (edit in place).
    async fn update_draft(&self, chat_id: &str, message_id: &str, text: &str) -> Result<()>;

    /// Finalize a draft — send the final version.
    async fn finalize(&self, chat_id: &str, message_id: &str, text: &str) -> Result<()>;

    /// Send a complete message (non-streaming).
    async fn send(&self, chat_id: &str, text: &str) -> Result<()>;

    /// Send a typing indicator.
    async fn send_typing(&self, chat_id: &str) -> Result<()>;
}
