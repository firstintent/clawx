use clawx_core::message::{Message, MessageRole};
use clawx_core::Result;
use clawx_llm::Provider;
use std::sync::Arc;
use tracing::{debug, info};

/// Tiered context compression.
///
/// Design: IronClaw's three-strategy graduated compression:
///   80-85% usage → Summarize old messages (keep recent 10 turns)
///   85-95% usage → Aggressive summarize (keep recent 5 turns)
///   >95% usage  → Truncate (drop oldest messages, no summary)
///
/// Combined with OpenFang's tool result truncation (applied in the loop).
pub struct ContextCompressor {
    context_window: usize,
    threshold_pct: f64,
}

impl ContextCompressor {
    pub fn new(context_window: usize, threshold_pct: f64) -> Self {
        Self { context_window, threshold_pct }
    }

    /// Estimate total token usage of the conversation.
    fn estimate_tokens(messages: &[Message]) -> usize {
        messages.iter().map(|m| m.estimate_tokens()).sum()
    }

    /// Calculate usage ratio.
    fn usage_ratio(&self, messages: &[Message]) -> f64 {
        let used = Self::estimate_tokens(messages);
        used as f64 / self.context_window as f64
    }

    /// Compress conversation if token usage exceeds threshold.
    pub async fn compress_if_needed(
        &self,
        messages: &mut Vec<Message>,
        provider: &Arc<dyn Provider>,
    ) -> Result<()> {
        let ratio = self.usage_ratio(messages);
        if ratio < self.threshold_pct {
            return Ok(());
        }

        info!(ratio, threshold = self.threshold_pct, "context compression triggered");

        if ratio > 0.95 {
            self.truncate(messages);
        } else if ratio > 0.85 {
            self.summarize(messages, provider, 5).await?;
        } else {
            self.summarize(messages, provider, 10).await?;
        }

        Ok(())
    }

    /// Truncate: drop oldest non-system messages.
    fn truncate(&self, messages: &mut Vec<Message>) {
        let keep_recent = 5;
        if messages.len() <= keep_recent + 1 {
            return;
        }

        // Keep system messages and the last N messages
        let system_msgs: Vec<Message> = messages
            .iter()
            .filter(|m| m.role == MessageRole::System)
            .cloned()
            .collect();

        let recent: Vec<Message> = messages
            .iter()
            .rev()
            .take(keep_recent)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();

        let dropped = messages.len() - system_msgs.len() - recent.len();
        info!(dropped, "truncated old messages");

        messages.clear();
        messages.extend(system_msgs);
        messages.push(Message::system(format!(
            "[Context truncated: {dropped} earlier messages were removed to stay within limits]"
        )));
        messages.extend(recent);
    }

    /// Summarize: use LLM to compress old messages, keep recent N turns.
    async fn summarize(
        &self,
        messages: &mut Vec<Message>,
        provider: &Arc<dyn Provider>,
        keep_recent: usize,
    ) -> Result<()> {
        if messages.len() <= keep_recent + 1 {
            return Ok(());
        }

        // Split into system, old, and recent
        let mut system_msgs: Vec<Message> = Vec::new();
        let mut old_msgs = Vec::new();
        let non_system: Vec<Message> = messages
            .iter()
            .filter(|m| {
                if m.role == MessageRole::System {
                    system_msgs.push((*m).clone());
                    false
                } else {
                    true
                }
            })
            .cloned()
            .collect();

        let split_point = non_system.len().saturating_sub(keep_recent);
        for (i, msg) in non_system.iter().enumerate() {
            if i < split_point {
                old_msgs.push(msg.clone());
            }
        }

        if old_msgs.is_empty() {
            return Ok(());
        }

        // Build summary request
        let old_text: String = old_msgs
            .iter()
            .map(|m| format!("[{:?}] {}", m.role, m.text()))
            .collect::<Vec<_>>()
            .join("\n");

        // Cap input to 12000 chars (ZeroClaw pattern)
        let summary_input = if old_text.len() > 12000 {
            old_text[..12000].to_string()
        } else {
            old_text
        };

        let summary_prompt = vec![
            Message::system("Summarize the following conversation concisely, preserving key decisions, tool results, and context. Output only the summary, max 500 words."),
            Message::user(summary_input),
        ];

        debug!("generating conversation summary");
        let summary_response = provider.complete(&summary_prompt, &[]).await?;
        let summary_text = summary_response.text();

        // Rebuild messages
        let recent: Vec<Message> = messages
            .iter()
            .filter(|m| m.role != MessageRole::System)
            .rev()
            .take(keep_recent)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();

        let old_count = old_msgs.len();
        messages.clear();
        messages.extend(system_msgs);
        messages.push(Message::system(format!(
            "[Compaction summary of {old_count} earlier messages]\n{summary_text}"
        )));
        messages.extend(recent);

        info!(old_count, "summarized old messages");
        Ok(())
    }
}
