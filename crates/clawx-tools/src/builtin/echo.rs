use async_trait::async_trait;
use clawx_core::{Result, ToolDefinition};
use crate::registry::Tool;
use serde_json::json;

pub struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "echo".into(),
            description: "Echo back a message. Useful for testing.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "message": {
                        "type": "string",
                        "description": "The message to echo back"
                    }
                },
                "required": ["message"]
            }),
        }
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<String> {
        let message = arguments["message"]
            .as_str()
            .unwrap_or("(no message)");
        Ok(message.to_string())
    }
}
