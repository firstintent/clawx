use async_trait::async_trait;
use clawx_core::{Message, Result, ToolCall};

/// Signal returned by delegate methods to control the loop.
#[derive(Debug, Clone)]
pub enum LoopSignal {
    /// Continue the loop normally.
    Continue,
    /// Stop the loop with a final message.
    Stop(Option<String>),
    /// Inject a message into the conversation and continue.
    InjectMessage(Message),
}

/// Delegate trait for customizing the agent loop behavior.
///
/// Design: IronClaw's `LoopDelegate` pattern — a single loop engine serves
/// multiple scenarios (chat, background job, sandbox) through trait polymorphism.
#[async_trait]
pub trait LoopDelegate: Send + Sync {
    /// Called before each iteration. Return a signal to control flow.
    async fn before_iteration(&self, iteration: usize) -> Result<LoopSignal> {
        let _ = iteration;
        Ok(LoopSignal::Continue)
    }

    /// Called before calling the LLM. Can modify the system prompt.
    async fn before_llm_call(&self, messages: &[Message]) -> Result<Option<String>> {
        let _ = messages;
        Ok(None)
    }

    /// Called when the LLM returns a text-only response (no tool calls).
    async fn handle_text_response(&self, text: &str) -> Result<LoopSignal> {
        let _ = text;
        Ok(LoopSignal::Continue)
    }

    /// Called before executing a tool. Return false to block execution.
    async fn approve_tool(&self, tool_call: &ToolCall) -> Result<bool> {
        let _ = tool_call;
        Ok(true)
    }

    /// Called after all tool calls in an iteration have been executed.
    async fn after_tool_execution(&self, results: &[(ToolCall, String)]) -> Result<LoopSignal> {
        let _ = results;
        Ok(LoopSignal::Continue)
    }

    /// Called after each iteration completes.
    async fn after_iteration(&self, iteration: usize) -> Result<LoopSignal> {
        let _ = iteration;
        Ok(LoopSignal::Continue)
    }
}

/// Simple chat delegate: always continues, approves all tools.
pub struct ChatDelegate;

#[async_trait]
impl LoopDelegate for ChatDelegate {
    async fn handle_text_response(&self, _text: &str) -> Result<LoopSignal> {
        // Text response means the LLM is done; stop the loop.
        Ok(LoopSignal::Stop(None))
    }
}

/// Background job delegate: runs autonomously until completion.
pub struct JobDelegate {
    pub task_description: String,
}

#[async_trait]
impl LoopDelegate for JobDelegate {
    async fn before_llm_call(&self, _messages: &[Message]) -> Result<Option<String>> {
        Ok(Some(format!(
            "You are executing a background task: {}\nWork autonomously to complete it.",
            self.task_description
        )))
    }
}
