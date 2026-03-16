use clawx_core::config::AgentConfig;
use clawx_core::message::{ContentBlock, Message};
use clawx_core::Result;
use clawx_llm::Provider;
use clawx_tools::ToolRegistry;
use crate::compression::ContextCompressor;
use crate::delegate::{LoopDelegate, LoopSignal};
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Outcome of the agent loop.
#[derive(Debug)]
pub enum LoopOutcome {
    /// LLM returned a final text response.
    Response(String),
    /// Loop was stopped by delegate signal.
    Stopped(Option<String>),
    /// Maximum iterations reached.
    MaxIterations,
    /// A tool requires approval and execution was paused.
    NeedApproval {
        tool_name: String,
        arguments: serde_json::Value,
        iteration: usize,
    },
}

/// Run the agent loop.
///
/// Design: IronClaw's unified `run_agentic_loop()` with LoopDelegate trait.
/// Single loop engine, customizable through delegate polymorphism.
///
/// Flow: check_signals → before_llm_call → LLM call → handle response
///   → tool calls → execute → after_iteration → repeat
pub async fn run_agent_loop(
    provider: Arc<dyn Provider>,
    tools: &ToolRegistry,
    messages: &mut Vec<Message>,
    delegate: &dyn LoopDelegate,
    config: &AgentConfig,
) -> Result<LoopOutcome> {
    let compressor = ContextCompressor::new(config.context_window_tokens, config.compress_threshold_pct);
    let tool_defs = tools.definitions();
    let mut nudge_count = 0;

    for iteration in 0..config.max_iterations {
        debug!(iteration, "agent loop iteration");

        // --- Pre-iteration signal check ---
        match delegate.before_iteration(iteration).await? {
            LoopSignal::Continue => {}
            LoopSignal::Stop(msg) => return Ok(LoopOutcome::Stopped(msg)),
            LoopSignal::InjectMessage(m) => {
                messages.push(m);
            }
        }

        // --- Context compression if needed ---
        compressor.compress_if_needed(messages, &provider).await?;

        // --- Optional system prompt override from delegate ---
        if let Some(system_override) = delegate.before_llm_call(messages).await? {
            // Replace or prepend system message
            if let Some(first) = messages.first_mut() {
                if first.role == clawx_core::message::MessageRole::System {
                    first.content = vec![ContentBlock::Text { text: system_override }];
                } else {
                    messages.insert(0, Message::system(system_override));
                }
            } else {
                messages.push(Message::system(system_override));
            }
        }

        // --- LLM call ---
        let response = provider.complete(messages, &tool_defs).await?;
        let tool_calls = response.tool_calls().into_iter().cloned().collect::<Vec<_>>();
        let text = response.text();
        messages.push(response);

        // --- Handle response ---
        if tool_calls.is_empty() {
            // Text-only response
            if text.is_empty() {
                warn!(iteration, "LLM returned empty response");
            }

            // Tool intent nudge: if we expected tools but got text, nudge (IronClaw pattern)
            if !tool_defs.is_empty() && nudge_count < config.tool_nudge_max && text.len() < 200 {
                nudge_count += 1;
                debug!(nudge_count, "nudging LLM to use tools");
                messages.push(Message::user(
                    "Please use the available tools to complete the task rather than just describing what to do."
                ));
                continue;
            }

            match delegate.handle_text_response(&text).await? {
                LoopSignal::Continue => continue,
                LoopSignal::Stop(_) => return Ok(LoopOutcome::Response(text)),
                LoopSignal::InjectMessage(m) => {
                    messages.push(m);
                    continue;
                }
            }
        }

        // --- Execute tool calls ---
        nudge_count = 0; // Reset nudge counter on tool use
        let mut results = Vec::new();

        for tc in &tool_calls {
            // Check approval
            if tools.requires_approval(&tc.name) {
                if !delegate.approve_tool(tc).await? {
                    return Ok(LoopOutcome::NeedApproval {
                        tool_name: tc.name.clone(),
                        arguments: tc.arguments.clone(),
                        iteration,
                    });
                }
            }

            // Execute
            let output = match tools.execute(&tc.name, tc.arguments.clone()).await {
                Ok(output) => {
                    // Truncate large outputs (OpenFang pattern: 30% of context window)
                    let max_chars = config.context_window_tokens * 2 * 30 / 100; // ~2 chars/token, 30%
                    if output.len() > max_chars {
                        let truncated = &output[..max_chars];
                        // Find last newline boundary
                        let cut = truncated.rfind('\n').unwrap_or(max_chars);
                        format!("{}\n\n[output truncated: {} of {} chars shown]", &output[..cut], cut, output.len())
                    } else {
                        output
                    }
                }
                Err(e) => format!("Error: {e}"),
            };

            let is_error = output.starts_with("Error:");
            messages.push(Message::tool_result(&tc.id, &output, is_error));
            results.push((tc.clone(), output));
        }

        // --- Post-tool signal ---
        match delegate.after_tool_execution(&results).await? {
            LoopSignal::Continue => {}
            LoopSignal::Stop(msg) => return Ok(LoopOutcome::Stopped(msg)),
            LoopSignal::InjectMessage(m) => {
                messages.push(m);
            }
        }

        // --- Post-iteration signal ---
        match delegate.after_iteration(iteration).await? {
            LoopSignal::Continue => {}
            LoopSignal::Stop(msg) => return Ok(LoopOutcome::Stopped(msg)),
            LoopSignal::InjectMessage(m) => {
                messages.push(m);
            }
        }
    }

    info!(max = config.max_iterations, "agent loop reached max iterations");
    Ok(LoopOutcome::MaxIterations)
}
