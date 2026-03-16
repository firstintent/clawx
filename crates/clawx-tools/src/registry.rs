use async_trait::async_trait;
use clawx_core::{Error, Result, ToolDefinition};
use std::collections::HashMap;
use std::sync::Arc;

/// A tool that can be executed by the agent.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool definition for the LLM.
    fn definition(&self) -> ToolDefinition;

    /// Execute the tool with the given arguments.
    async fn execute(&self, arguments: serde_json::Value) -> Result<String>;

    /// Whether this tool requires user approval before execution.
    fn requires_approval(&self) -> bool { false }
}

/// Registry of available tools.
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a tool.
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        let name = tool.definition().name.clone();
        self.tools.insert(name, tool);
    }

    /// Get a tool by name.
    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.get(name)
    }

    /// Get all tool definitions for the LLM.
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().map(|t| t.definition()).collect()
    }

    /// Execute a tool by name.
    pub async fn execute(&self, name: &str, arguments: serde_json::Value) -> Result<String> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| Error::ToolNotFound(name.to_string()))?;
        tool.execute(arguments).await
    }

    /// Check if a tool requires approval.
    pub fn requires_approval(&self, name: &str) -> bool {
        self.tools
            .get(name)
            .is_some_and(|t| t.requires_approval())
    }

    /// Number of registered tools.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Create a filtered copy with only the named tools.
    pub fn filter(&self, names: &[&str]) -> Self {
        let tools = names
            .iter()
            .filter_map(|n| self.tools.get(*n).map(|t| (n.to_string(), t.clone())))
            .collect();
        Self { tools }
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
