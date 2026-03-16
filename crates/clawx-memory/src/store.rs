use async_trait::async_trait;
use clawx_core::Result;
use serde::{Deserialize, Serialize};

/// A memory entry stored in the memory system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub content: String,
    pub metadata: serde_json::Value,
    pub created_at: String,
    #[serde(default)]
    pub relevance_score: f64,
}

/// Pluggable memory store trait.
///
/// Design: ZeroClaw's `Memory` trait with 6 backends as inspiration.
/// SQLite as default, trait allows Qdrant/PG/etc extensions.
#[async_trait]
pub trait MemoryStore: Send + Sync {
    /// Store a memory entry.
    async fn store(&self, content: &str, metadata: serde_json::Value) -> Result<String>;

    /// Recall relevant memories for a query.
    async fn recall(&self, query: &str, top_k: usize, threshold: f64) -> Result<Vec<MemoryEntry>>;

    /// Get a specific memory by ID.
    async fn get(&self, id: &str) -> Result<Option<MemoryEntry>>;

    /// List all memories with optional pagination.
    async fn list(&self, offset: usize, limit: usize) -> Result<Vec<MemoryEntry>>;

    /// Delete a memory by ID.
    async fn forget(&self, id: &str) -> Result<bool>;

    /// Count total memories.
    async fn count(&self) -> Result<usize>;

    /// Health check.
    async fn health_check(&self) -> Result<()>;
}
