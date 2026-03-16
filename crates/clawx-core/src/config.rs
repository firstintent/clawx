use serde::{Deserialize, Serialize};

/// Top-level configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub llm: LlmConfig,
    #[serde(default)]
    pub agent: AgentConfig,
    #[serde(default)]
    pub memory: MemoryConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: usize,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    #[serde(default)]
    pub retry: RetryConfig,
    #[serde(default)]
    pub failover: Option<FailoverConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    #[serde(default = "default_max_retries")]
    pub max_retries: usize,
    #[serde(default = "default_base_delay_ms")]
    pub base_delay_ms: u64,
    #[serde(default = "default_jitter_pct")]
    pub jitter_pct: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: default_max_retries(),
            base_delay_ms: default_base_delay_ms(),
            jitter_pct: default_jitter_pct(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailoverConfig {
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default = "default_failure_threshold")]
    pub failure_threshold: usize,
    #[serde(default = "default_cooldown_secs")]
    pub cooldown_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    #[serde(default = "default_max_iterations")]
    pub max_iterations: usize,
    #[serde(default = "default_max_depth")]
    pub max_depth: usize,
    #[serde(default = "default_context_window")]
    pub context_window_tokens: usize,
    #[serde(default = "default_compress_threshold")]
    pub compress_threshold_pct: f64,
    #[serde(default = "default_tool_nudge_max")]
    pub tool_nudge_max: usize,
    #[serde(default)]
    pub parallel_tools: bool,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_iterations: default_max_iterations(),
            max_depth: default_max_depth(),
            context_window_tokens: default_context_window(),
            compress_threshold_pct: default_compress_threshold(),
            tool_nudge_max: default_tool_nudge_max(),
            parallel_tools: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    #[serde(default = "default_memory_backend")]
    pub backend: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default = "default_recall_top_k")]
    pub recall_top_k: usize,
    #[serde(default = "default_relevance_threshold")]
    pub relevance_threshold: f64,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            backend: default_memory_backend(),
            path: None,
            recall_top_k: default_recall_top_k(),
            relevance_threshold: default_relevance_threshold(),
        }
    }
}

fn default_max_tokens() -> usize { 4096 }
fn default_temperature() -> f32 { 0.7 }
fn default_max_retries() -> usize { 3 }
fn default_base_delay_ms() -> u64 { 1000 }
fn default_jitter_pct() -> f64 { 0.25 }
fn default_failure_threshold() -> usize { 3 }
fn default_cooldown_secs() -> u64 { 300 }
fn default_max_iterations() -> usize { 50 }
fn default_max_depth() -> usize { 5 }
fn default_context_window() -> usize { 128_000 }
fn default_compress_threshold() -> f64 { 0.80 }
fn default_tool_nudge_max() -> usize { 2 }
fn default_memory_backend() -> String { "sqlite".into() }
fn default_recall_top_k() -> usize { 5 }
fn default_relevance_threshold() -> f64 { 0.4 }
