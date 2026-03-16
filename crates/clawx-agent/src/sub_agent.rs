use clawx_core::config::AgentConfig;
use clawx_core::message::Message;
use clawx_core::{Error, Result};
use clawx_llm::Provider;
use clawx_tools::ToolRegistry;
use crate::agent_loop::{run_agent_loop, LoopOutcome};
use crate::delegate::ChatDelegate;
use std::sync::Arc;
use tracing::info;

/// Sub-agent for delegated tasks.
///
/// Design: ZeroClaw's delegate tool with depth protection (OpenFang's MAX_AGENT_CALL_DEPTH).
/// Supports simple (one-shot) and agentic (loop) modes.
pub struct SubAgent {
    pub provider: Arc<dyn Provider>,
    pub tools: ToolRegistry,
    pub config: AgentConfig,
    pub depth: usize,
}

impl SubAgent {
    pub fn new(
        provider: Arc<dyn Provider>,
        tools: ToolRegistry,
        config: AgentConfig,
        parent_depth: usize,
    ) -> Self {
        Self {
            provider,
            tools,
            config,
            depth: parent_depth + 1,
        }
    }

    /// Execute a one-shot task: send prompt, get response (no tool loop).
    pub async fn simple(&self, system: &str, prompt: &str) -> Result<String> {
        self.check_depth()?;

        info!(depth = self.depth, "sub-agent simple execution");

        let messages = vec![
            Message::system(system),
            Message::user(prompt),
        ];

        let timeout = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            self.provider.complete(&messages, &[]),
        )
        .await
        .map_err(|_| Error::Timeout(120))?;

        Ok(timeout?.text())
    }

    /// Execute an agentic task: full agent loop with tools.
    pub async fn agentic(&self, system: &str, prompt: &str) -> Result<String> {
        self.check_depth()?;

        info!(depth = self.depth, "sub-agent agentic execution");

        let mut messages = vec![
            Message::system(system),
            Message::user(prompt),
        ];

        let delegate = ChatDelegate;

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(300),
            run_agent_loop(
                self.provider.clone(),
                &self.tools,
                &mut messages,
                &delegate,
                &self.config,
            ),
        )
        .await
        .map_err(|_| Error::Timeout(300))??;

        match outcome {
            LoopOutcome::Response(text) => Ok(text),
            LoopOutcome::Stopped(msg) => Ok(msg.unwrap_or_default()),
            LoopOutcome::MaxIterations => Ok(
                messages
                    .last()
                    .map(|m| m.text())
                    .unwrap_or_else(|| "[max iterations reached]".into()),
            ),
            LoopOutcome::NeedApproval { tool_name, .. } => {
                Err(Error::Other(format!("sub-agent cannot request approval for tool: {tool_name}")))
            }
        }
    }

    fn check_depth(&self) -> Result<()> {
        if self.depth > self.config.max_depth {
            return Err(Error::DepthLimitExceeded(self.config.max_depth));
        }
        Ok(())
    }
}
