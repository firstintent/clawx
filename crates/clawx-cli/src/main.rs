use anyhow::Result;
use clap::{Parser, Subcommand};
use clawx_agent::{run_agent_loop, LoopOutcome};
use clawx_agent::delegate::ChatDelegate;
use clawx_channels::channel::Channel;
use clawx_channels::telegram::{TelegramChannel, TelegramConfig};
use clawx_core::config::AgentConfig;
use clawx_core::message::Message;
use clawx_llm::provider::Provider;
use clawx_llm::providers::{AnthropicProvider, OpenAiProvider};
use clawx_llm::{RetryProvider, CircuitBreakerProvider};
use clawx_memory::{MemoryStore, SqliteMemory};
use clawx_tools::builtin::{EchoTool, ShellTool, ReadFileTool, WriteFileTool, ListDirTool};
use clawx_tools::ToolRegistry;
use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info};

#[derive(Parser)]
#[command(name = "clawx", about = "ClawX AI Agent")]
struct Cli {
    /// LLM provider: anthropic, openai, openrouter, ollama
    #[arg(long, global = true, default_value = "anthropic")]
    provider: String,

    /// Model name
    #[arg(long, global = true, default_value = "claude-sonnet-4-20250514")]
    model: String,

    /// API key (or set ANTHROPIC_API_KEY / OPENAI_API_KEY env var)
    #[arg(long, global = true)]
    api_key: Option<String>,

    /// Base URL override
    #[arg(long, global = true)]
    base_url: Option<String>,

    /// Maximum agent loop iterations
    #[arg(long, global = true, default_value = "50")]
    max_iterations: usize,

    /// System prompt
    #[arg(long, short = 's', global = true)]
    system: Option<String>,

    /// Memory database path
    #[arg(long, global = true, default_value = "clawx_memory.db")]
    memory_db: String,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Run as a Telegram bot
    Telegram {
        /// Telegram bot token (or set TELEGRAM_BOT_TOKEN env var)
        #[arg(long)]
        bot_token: Option<String>,

        /// Allowed Telegram user IDs (comma-separated). Empty = allow all.
        #[arg(long, default_value = "")]
        allowed_users: String,

        /// In groups, only respond when @mentioned
        #[arg(long, default_value = "true")]
        mention_only: bool,

        /// Custom Bot API base URL
        #[arg(long)]
        telegram_api_base: Option<String>,

        /// Minimum interval (ms) between draft edits
        #[arg(long, default_value = "750")]
        draft_interval_ms: u64,
    },

    /// Run a single prompt and exit
    Run {
        /// The prompt to execute
        prompt: String,
    },
}

fn build_provider(cli: &Cli) -> Result<Arc<dyn Provider>> {
    let api_key = cli.api_key.clone().or_else(|| {
        match cli.provider.as_str() {
            "anthropic" => std::env::var("ANTHROPIC_API_KEY").ok(),
            "openai" => std::env::var("OPENAI_API_KEY").ok(),
            "openrouter" => std::env::var("OPENROUTER_API_KEY").ok(),
            _ => std::env::var("API_KEY").ok(),
        }
    }).unwrap_or_default();

    let raw: Arc<dyn Provider> = match cli.provider.as_str() {
        "anthropic" => Arc::new(AnthropicProvider::new(
            api_key,
            cli.model.clone(),
            cli.base_url.clone(),
            4096,
            0.7,
        )),
        "openai" | "openrouter" | "ollama" => {
            let base_url = cli.base_url.clone().or_else(|| match cli.provider.as_str() {
                "openrouter" => Some("https://openrouter.ai/api".into()),
                "ollama" => Some("http://localhost:11434".into()),
                _ => None,
            });
            Arc::new(OpenAiProvider::new(
                api_key,
                cli.model.clone(),
                base_url,
                4096,
                0.7,
            ))
        }
        other => anyhow::bail!("unsupported provider: {other}"),
    };

    let with_retry = Arc::new(RetryProvider::new(raw, 3, 1000, 0.25));
    let with_cb = Arc::new(CircuitBreakerProvider::new(with_retry, 5, 30));
    Ok(with_cb)
}

fn build_tools() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(EchoTool));
    registry.register(Arc::new(ShellTool::default()));
    registry.register(Arc::new(ReadFileTool));
    registry.register(Arc::new(WriteFileTool));
    registry.register(Arc::new(ListDirTool));
    registry
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("clawx=info".parse().unwrap()),
        )
        .init();

    let cli = Cli::parse();
    let provider = build_provider(&cli)?;
    let tools = build_tools();
    let config = AgentConfig {
        max_iterations: cli.max_iterations,
        ..AgentConfig::default()
    };

    let memory = SqliteMemory::new(&cli.memory_db)?;
    memory.health_check().await?;
    info!(db = %cli.memory_db, "memory initialized");

    let system_prompt = cli.system.clone().unwrap_or_else(|| {
        "You are ClawX, a helpful AI assistant. Use the available tools to help the user accomplish their tasks. Respond concisely.".into()
    });

    match &cli.command {
        Some(Commands::Telegram {
            bot_token,
            allowed_users,
            mention_only,
            telegram_api_base,
            draft_interval_ms,
        }) => {
            run_telegram(
                provider,
                tools,
                config,
                system_prompt,
                bot_token.clone(),
                allowed_users.clone(),
                *mention_only,
                telegram_api_base.clone(),
                *draft_interval_ms,
            )
            .await
        }
        Some(Commands::Run { prompt }) => {
            run_oneshot(provider, tools, config, system_prompt, prompt).await
        }
        None => {
            run_repl(provider, tools, config, system_prompt).await
        }
    }
}

// ---------------------------------------------------------------------------
// Telegram bot mode
// ---------------------------------------------------------------------------

async fn run_telegram(
    provider: Arc<dyn Provider>,
    tools: ToolRegistry,
    config: AgentConfig,
    system_prompt: String,
    bot_token: Option<String>,
    allowed_users: String,
    mention_only: bool,
    api_base: Option<String>,
    draft_interval_ms: u64,
) -> Result<()> {
    let token = bot_token
        .or_else(|| std::env::var("TELEGRAM_BOT_TOKEN").ok())
        .ok_or_else(|| anyhow::anyhow!(
            "Telegram bot token required. Use --bot-token or set TELEGRAM_BOT_TOKEN env var"
        ))?;

    let allowed: Vec<String> = allowed_users
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let tg_config = TelegramConfig {
        bot_token: token,
        allowed_users: allowed.clone(),
        mention_only,
        api_base,
        draft_interval_ms,
    };

    let channel = Arc::new(TelegramChannel::new(tg_config));
    channel.init().await?;

    info!(
        allowed_users = ?allowed,
        mention_only,
        "Telegram bot started, polling for messages..."
    );

    // Per-chat conversation history
    let conversations: Arc<RwLock<HashMap<String, Vec<Message>>>> =
        Arc::new(RwLock::new(HashMap::new()));

    let (tx, mut rx) = tokio::sync::mpsc::channel(64);

    // Spawn polling task
    let poll_channel = channel.clone();
    tokio::spawn(async move {
        if let Err(e) = poll_channel.poll_loop(tx).await {
            error!(error = %e, "telegram poll loop exited");
        }
    });

    // Process incoming messages
    let delegate = ChatDelegate;
    let tools = Arc::new(tools);

    while let Some(msg) = rx.recv().await {
        let chat_id = msg.chat_id.clone();
        let provider = provider.clone();
        let tools = tools.clone();
        let config = config.clone();
        let system_prompt = system_prompt.clone();
        let channel = channel.clone();
        let conversations = conversations.clone();
        let delegate = ChatDelegate;

        tokio::spawn(async move {
            info!(
                chat_id,
                sender = msg.sender,
                text = msg.text,
                "received telegram message"
            );

            // Send typing indicator
            let _ = channel.send_typing(&chat_id).await;

            // Get or create conversation for this chat
            let mut convos = conversations.write().await;
            let messages = convos.entry(chat_id.clone()).or_insert_with(|| {
                vec![Message::system(&system_prompt)]
            });
            messages.push(Message::user(&msg.text));

            // Run agent loop
            match run_agent_loop(provider, &tools, messages, &delegate, &config).await {
                Ok(outcome) => {
                    let response = match outcome {
                        LoopOutcome::Response(text) => text,
                        LoopOutcome::Stopped(msg) => msg.unwrap_or_default(),
                        LoopOutcome::MaxIterations => "[max iterations reached]".into(),
                        LoopOutcome::NeedApproval { tool_name, .. } => {
                            format!("[tool '{tool_name}' needs approval — not available in Telegram mode]")
                        }
                    };

                    if !response.is_empty() {
                        if let Err(e) = channel.send(&chat_id, &response).await {
                            error!(error = %e, chat_id, "failed to send telegram response");
                        }
                    }
                }
                Err(e) => {
                    error!(error = %e, chat_id, "agent loop error");
                    let _ = channel.send(&chat_id, &format!("Error: {e}")).await;
                }
            }
        });
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// One-shot mode
// ---------------------------------------------------------------------------

async fn run_oneshot(
    provider: Arc<dyn Provider>,
    tools: ToolRegistry,
    config: AgentConfig,
    system_prompt: String,
    prompt: &str,
) -> Result<()> {
    let mut messages = vec![
        Message::system(&system_prompt),
        Message::user(prompt),
    ];

    let delegate = ChatDelegate;
    let outcome = run_agent_loop(provider, &tools, &mut messages, &delegate, &config).await?;

    match outcome {
        LoopOutcome::Response(text) => println!("{text}"),
        LoopOutcome::Stopped(msg) => {
            if let Some(m) = msg { println!("{m}"); }
        }
        LoopOutcome::MaxIterations => eprintln!("[max iterations reached]"),
        LoopOutcome::NeedApproval { tool_name, .. } => {
            eprintln!("[tool '{tool_name}' requires approval]");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Interactive REPL mode
// ---------------------------------------------------------------------------

async fn run_repl(
    provider: Arc<dyn Provider>,
    tools: ToolRegistry,
    config: AgentConfig,
    system_prompt: String,
) -> Result<()> {
    println!("ClawX AI Agent (type 'exit' to quit)");
    println!("Tools: {}", tools.definitions().iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", "));
    println!();

    let mut messages = vec![Message::system(&system_prompt)];
    let delegate = ChatDelegate;
    let stdin = io::stdin();

    loop {
        print!("you> ");
        io::stdout().flush()?;

        let mut input = String::new();
        if stdin.lock().read_line(&mut input)? == 0 {
            break;
        }
        let input = input.trim();
        if input.is_empty() { continue; }
        if input == "exit" || input == "quit" { break; }
        if input == "/compact" {
            let compressor = clawx_agent::ContextCompressor::new(
                config.context_window_tokens, 0.0,
            );
            compressor.compress_if_needed(&mut messages, &provider).await?;
            println!("[context compressed, {} messages remaining]", messages.len());
            continue;
        }

        messages.push(Message::user(input));

        match run_agent_loop(provider.clone(), &tools, &mut messages, &delegate, &config).await {
            Ok(LoopOutcome::Response(text)) => println!("\nclawx> {text}\n"),
            Ok(LoopOutcome::Stopped(Some(m))) => println!("\nclawx> {m}\n"),
            Ok(LoopOutcome::Stopped(None)) => {}
            Ok(LoopOutcome::MaxIterations) => println!("\n[max iterations reached]\n"),
            Ok(LoopOutcome::NeedApproval { tool_name, .. }) => {
                println!("\n[tool '{tool_name}' requires approval]\n");
            }
            Err(e) => eprintln!("\n[error: {e}]\n"),
        }
    }
    Ok(())
}
