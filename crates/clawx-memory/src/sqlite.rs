use async_trait::async_trait;
use clawx_core::{Error, Result};
use crate::store::{MemoryEntry, MemoryStore};
use rusqlite::Connection;
use std::sync::Mutex;
use tracing::debug;

/// SQLite-backed memory store with FTS5 full-text search.
pub struct SqliteMemory {
    conn: Mutex<Connection>,
}

impl SqliteMemory {
    pub fn new(path: &str) -> Result<Self> {
        let conn = if path == ":memory:" {
            Connection::open_in_memory()
        } else {
            Connection::open(path)
        }
        .map_err(|e| Error::Memory(e.to_string()))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS memories (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                metadata TEXT NOT NULL DEFAULT '{}',
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
                content,
                content='memories',
                content_rowid='rowid'
            );

            CREATE TRIGGER IF NOT EXISTS memories_ai AFTER INSERT ON memories BEGIN
                INSERT INTO memories_fts(rowid, content)
                VALUES (new.rowid, new.content);
            END;

            CREATE TRIGGER IF NOT EXISTS memories_ad AFTER DELETE ON memories BEGIN
                INSERT INTO memories_fts(memories_fts, rowid, content)
                VALUES ('delete', old.rowid, old.content);
            END;

            CREATE TRIGGER IF NOT EXISTS memories_au AFTER UPDATE ON memories BEGIN
                INSERT INTO memories_fts(memories_fts, rowid, content)
                VALUES ('delete', old.rowid, old.content);
                INSERT INTO memories_fts(rowid, content)
                VALUES (new.rowid, new.content);
            END;"
        )
        .map_err(|e| Error::Memory(e.to_string()))?;

        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn in_memory() -> Result<Self> {
        Self::new(":memory:")
    }
}

#[async_trait]
impl MemoryStore for SqliteMemory {
    async fn store(&self, content: &str, metadata: serde_json::Value) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let meta_str = serde_json::to_string(&metadata)?;
        let conn = self.conn.lock().map_err(|e| Error::Memory(e.to_string()))?;
        conn.execute(
            "INSERT INTO memories (id, content, metadata) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, content, meta_str],
        )
        .map_err(|e| Error::Memory(e.to_string()))?;
        debug!(id, "stored memory");
        Ok(id)
    }

    async fn recall(&self, query: &str, top_k: usize, _threshold: f64) -> Result<Vec<MemoryEntry>> {
        let conn = self.conn.lock().map_err(|e| Error::Memory(e.to_string()))?;

        // Use FTS5 for relevance-ranked search
        let mut stmt = conn
            .prepare(
                "SELECT m.id, m.content, m.metadata, m.created_at,
                        bm25(memories_fts) as rank
                 FROM memories_fts f
                 JOIN memories m ON m.rowid = f.rowid
                 WHERE memories_fts MATCH ?1
                 ORDER BY rank
                 LIMIT ?2"
            )
            .map_err(|e| Error::Memory(e.to_string()))?;

        // FTS5 query: escape special characters
        let fts_query = query
            .split_whitespace()
            .map(|w| format!("\"{w}\""))
            .collect::<Vec<_>>()
            .join(" OR ");

        let entries = stmt
            .query_map(rusqlite::params![fts_query, top_k], |row| {
                let metadata_str: String = row.get(2)?;
                Ok(MemoryEntry {
                    id: row.get(0)?,
                    content: row.get(1)?,
                    metadata: serde_json::from_str(&metadata_str).unwrap_or_default(),
                    created_at: row.get(3)?,
                    relevance_score: row.get::<_, f64>(4).unwrap_or(0.0).abs(),
                })
            })
            .map_err(|e| Error::Memory(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(entries)
    }

    async fn get(&self, id: &str) -> Result<Option<MemoryEntry>> {
        let conn = self.conn.lock().map_err(|e| Error::Memory(e.to_string()))?;
        let mut stmt = conn
            .prepare("SELECT id, content, metadata, created_at FROM memories WHERE id = ?1")
            .map_err(|e| Error::Memory(e.to_string()))?;

        let entry = stmt
            .query_row(rusqlite::params![id], |row| {
                let metadata_str: String = row.get(2)?;
                Ok(MemoryEntry {
                    id: row.get(0)?,
                    content: row.get(1)?,
                    metadata: serde_json::from_str(&metadata_str).unwrap_or_default(),
                    created_at: row.get(3)?,
                    relevance_score: 0.0,
                })
            })
            .ok();

        Ok(entry)
    }

    async fn list(&self, offset: usize, limit: usize) -> Result<Vec<MemoryEntry>> {
        let conn = self.conn.lock().map_err(|e| Error::Memory(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, content, metadata, created_at FROM memories
                 ORDER BY created_at DESC LIMIT ?1 OFFSET ?2"
            )
            .map_err(|e| Error::Memory(e.to_string()))?;

        let entries = stmt
            .query_map(rusqlite::params![limit, offset], |row| {
                let metadata_str: String = row.get(2)?;
                Ok(MemoryEntry {
                    id: row.get(0)?,
                    content: row.get(1)?,
                    metadata: serde_json::from_str(&metadata_str).unwrap_or_default(),
                    created_at: row.get(3)?,
                    relevance_score: 0.0,
                })
            })
            .map_err(|e| Error::Memory(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(entries)
    }

    async fn forget(&self, id: &str) -> Result<bool> {
        let conn = self.conn.lock().map_err(|e| Error::Memory(e.to_string()))?;
        let affected = conn
            .execute("DELETE FROM memories WHERE id = ?1", rusqlite::params![id])
            .map_err(|e| Error::Memory(e.to_string()))?;
        Ok(affected > 0)
    }

    async fn count(&self) -> Result<usize> {
        let conn = self.conn.lock().map_err(|e| Error::Memory(e.to_string()))?;
        let count: usize = conn
            .query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))
            .map_err(|e| Error::Memory(e.to_string()))?;
        Ok(count)
    }

    async fn health_check(&self) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| Error::Memory(e.to_string()))?;
        conn.execute_batch("SELECT 1")
            .map_err(|e| Error::Memory(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_store_and_recall() {
        let mem = SqliteMemory::in_memory().unwrap();
        let id = mem.store("Rust is a systems programming language", serde_json::json!({})).await.unwrap();
        assert!(!id.is_empty());

        let results = mem.recall("Rust programming", 5, 0.0).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("Rust"));
    }

    #[tokio::test]
    async fn test_forget() {
        let mem = SqliteMemory::in_memory().unwrap();
        let id = mem.store("temporary memory", serde_json::json!({})).await.unwrap();
        assert_eq!(mem.count().await.unwrap(), 1);
        mem.forget(&id).await.unwrap();
        assert_eq!(mem.count().await.unwrap(), 0);
    }
}
