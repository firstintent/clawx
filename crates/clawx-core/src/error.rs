use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("LLM provider error: {0}")]
    Provider(String),

    #[error("rate limited, retry after {retry_after_secs}s")]
    RateLimited { retry_after_secs: u64 },

    #[error("context window exceeded: used {used} of {limit} tokens")]
    ContextOverflow { used: usize, limit: usize },

    #[error("tool execution error: {tool}: {message}")]
    ToolExecution { tool: String, message: String },

    #[error("tool not found: {0}")]
    ToolNotFound(String),

    #[error("memory error: {0}")]
    Memory(String),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("max iterations ({0}) reached")]
    MaxIterations(usize),

    #[error("agent depth limit ({0}) exceeded")]
    DepthLimitExceeded(usize),

    #[error("timeout after {0}s")]
    Timeout(u64),

    #[error("cancelled")]
    Cancelled,

    #[error("{0}")]
    Other(String),
}

impl Error {
    /// Whether this error is transient and the operation can be retried.
    pub fn is_transient(&self) -> bool {
        matches!(self, Error::RateLimited { .. } | Error::Provider(_))
    }
}
