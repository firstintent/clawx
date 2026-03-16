use async_trait::async_trait;
use clawx_core::{Error, Result, ToolDefinition};
use crate::registry::Tool;
use serde_json::json;

pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "read_file".into(),
            description: "Read the contents of a file.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute path to the file"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Line number to start reading from (0-based)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of lines to read"
                    }
                },
                "required": ["path"]
            }),
        }
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String> {
        let path = arguments["path"]
            .as_str()
            .ok_or_else(|| Error::ToolExecution {
                tool: "read_file".into(),
                message: "missing 'path' argument".into(),
            })?;

        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| Error::ToolExecution {
                tool: "read_file".into(),
                message: format!("{path}: {e}"),
            })?;

        let offset = arguments["offset"].as_u64().unwrap_or(0) as usize;
        let limit = arguments["limit"].as_u64().map(|l| l as usize);

        let lines: Vec<&str> = content.lines().collect();
        let end = limit.map_or(lines.len(), |l| (offset + l).min(lines.len()));
        let selected: Vec<String> = lines[offset.min(lines.len())..end]
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{:>4}\t{}", offset + i + 1, line))
            .collect();

        Ok(selected.join("\n"))
    }
}

pub struct WriteFileTool;

#[async_trait]
impl Tool for WriteFileTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "write_file".into(),
            description: "Write content to a file, creating it if it doesn't exist.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute path to the file"
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to write"
                    }
                },
                "required": ["path", "content"]
            }),
        }
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String> {
        let path = arguments["path"]
            .as_str()
            .ok_or_else(|| Error::ToolExecution {
                tool: "write_file".into(),
                message: "missing 'path' argument".into(),
            })?;
        let content = arguments["content"]
            .as_str()
            .ok_or_else(|| Error::ToolExecution {
                tool: "write_file".into(),
                message: "missing 'content' argument".into(),
            })?;

        // Ensure parent directory exists
        if let Some(parent) = std::path::Path::new(path).parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| Error::ToolExecution {
                    tool: "write_file".into(),
                    message: format!("create parent dirs: {e}"),
                })?;
        }

        tokio::fs::write(path, content)
            .await
            .map_err(|e| Error::ToolExecution {
                tool: "write_file".into(),
                message: format!("{path}: {e}"),
            })?;

        Ok(format!("Wrote {} bytes to {path}", content.len()))
    }

    fn requires_approval(&self) -> bool { true }
}

pub struct ListDirTool;

#[async_trait]
impl Tool for ListDirTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "list_dir".into(),
            description: "List contents of a directory.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the directory"
                    }
                },
                "required": ["path"]
            }),
        }
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String> {
        let path = arguments["path"]
            .as_str()
            .ok_or_else(|| Error::ToolExecution {
                tool: "list_dir".into(),
                message: "missing 'path' argument".into(),
            })?;

        let mut entries = tokio::fs::read_dir(path)
            .await
            .map_err(|e| Error::ToolExecution {
                tool: "list_dir".into(),
                message: format!("{path}: {e}"),
            })?;

        let mut items = Vec::new();
        while let Some(entry) = entries.next_entry().await.map_err(|e| Error::ToolExecution {
            tool: "list_dir".into(),
            message: e.to_string(),
        })? {
            let name = entry.file_name().to_string_lossy().to_string();
            let ft = entry.file_type().await.ok();
            let suffix = if ft.as_ref().is_some_and(|f| f.is_dir()) { "/" } else { "" };
            items.push(format!("{name}{suffix}"));
        }

        items.sort();
        Ok(items.join("\n"))
    }
}
