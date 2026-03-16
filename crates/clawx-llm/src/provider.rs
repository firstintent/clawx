use async_trait::async_trait;
use clawx_core::{Message, Result, ToolDefinition};
use crate::stream::StreamEvent;
use tokio::sync::mpsc;

/// Core LLM provider trait.
///
/// Design: Inspired by IronClaw's `LlmProvider` elegance with OpenFang/ZeroClaw's
/// streaming support. Providers implement both blocking and streaming paths.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Non-streaming completion.
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<Message>;

    /// Streaming completion. Sends deltas via the channel.
    async fn stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        tx: mpsc::Sender<StreamEvent>,
    ) -> Result<Message>;

    /// Provider name for logging and routing.
    fn name(&self) -> &str;

    /// Model identifier.
    fn model(&self) -> &str;
}
