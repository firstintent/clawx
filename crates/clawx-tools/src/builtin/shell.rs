use async_trait::async_trait;
use clawx_core::{Error, Result, ToolDefinition};
use crate::registry::Tool;
use serde_json::json;

pub struct ShellTool {
    timeout_secs: u64,
}

impl ShellTool {
    pub fn new(timeout_secs: u64) -> Self {
        Self { timeout_secs }
    }
}

impl Default for ShellTool {
    fn default() -> Self {
        Self::new(120)
    }
}

#[async_trait]
impl Tool for ShellTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "shell".into(),
            description: "Execute a shell command and return its output.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The shell command to execute"
                    },
                    "working_dir": {
                        "type": "string",
                        "description": "Working directory for the command (optional)"
                    }
                },
                "required": ["command"]
            }),
        }
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String> {
        let command = arguments["command"]
            .as_str()
            .ok_or_else(|| Error::ToolExecution {
                tool: "shell".into(),
                message: "missing 'command' argument".into(),
            })?;

        let working_dir = arguments["working_dir"].as_str();

        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(command);
        cmd.env_clear();
        // Preserve minimal safe env
        if let Ok(path) = std::env::var("PATH") {
            cmd.env("PATH", path);
        }
        if let Ok(home) = std::env::var("HOME") {
            cmd.env("HOME", home);
        }

        if let Some(dir) = working_dir {
            cmd.current_dir(dir);
        }

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(self.timeout_secs),
            cmd.output(),
        )
        .await
        .map_err(|_| Error::Timeout(self.timeout_secs))?
        .map_err(|e| Error::ToolExecution {
            tool: "shell".into(),
            message: e.to_string(),
        })?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        let mut result = String::new();
        if !stdout.is_empty() {
            result.push_str(&stdout);
        }
        if !stderr.is_empty() {
            if !result.is_empty() {
                result.push_str("\n--- stderr ---\n");
            }
            result.push_str(&stderr);
        }
        if !output.status.success() {
            result.push_str(&format!("\n[exit code: {}]", output.status.code().unwrap_or(-1)));
        }

        // Sanitize potential credential leaks
        let result = crate::security::redact_credentials(&result);

        Ok(result)
    }

    fn requires_approval(&self) -> bool { true }
}
