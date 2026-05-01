//! Tool system: ToolHandler trait, ToolRegistry, and built-in tools.
//!
//! Built-in tools:
//! - `shell`: Execute shell commands (zsh/bash, auto-detected)
//! - `write_file`: Write content to a file
//! - `read_file`: Read file contents (with offset/limit)
//! - `list_directory`: List directory entries
//! - `apply_patch`: Precise file editing via search-and-replace

pub mod file_ops;
pub mod head_tail_buffer;
pub mod patch;
pub mod shell;
pub mod switch_workdir;

use crate::types::{FunctionDefinition, ToolDefinition, ToolResult};
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// Trait for implementing agent tools.
#[async_trait]
pub trait ToolHandler: Send + Sync {
    /// The tool's unique name (used in function calls).
    fn name(&self) -> &str;

    /// A description of what the tool does (sent to the model).
    fn description(&self) -> &str;

    /// JSON Schema for the tool's parameters.
    fn parameters_schema(&self) -> serde_json::Value;

    /// Execute the tool with the given JSON arguments string.
    async fn execute(&self, arguments: &str, work_dir: &Path) -> ToolResult;
}

/// Registry of available tools.
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn ToolHandler>>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a tool handler.
    pub fn register(&mut self, handler: Arc<dyn ToolHandler>) {
        self.tools.insert(handler.name().to_string(), handler);
    }

    /// Create a registry with all built-in tools.
    pub fn with_defaults(shell_timeout_secs: u64) -> Self {
        let mut registry = Self::new();
        registry.register(Arc::new(shell::ShellTool::new(shell_timeout_secs)));
        registry.register(Arc::new(file_ops::WriteFileTool));
        registry.register(Arc::new(file_ops::ReadFileTool));
        registry.register(Arc::new(file_ops::ListDirectoryTool));
        registry.register(Arc::new(patch::ApplyPatchTool));
        registry.register(Arc::new(switch_workdir::SwitchWorkdirTool));
        registry
    }

    /// Get tool definitions for the model API `tools` parameter.
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .map(|h| ToolDefinition {
                tool_type: "function".to_string(),
                function: FunctionDefinition {
                    name: h.name().to_string(),
                    description: h.description().to_string(),
                    parameters: Some(h.parameters_schema()),
                },
            })
            .collect()
    }

    /// Execute a tool by name with the given arguments.
    pub async fn execute(&self, name: &str, arguments: &str, work_dir: &Path) -> ToolResult {
        match self.tools.get(name) {
            Some(handler) => handler.execute(arguments, work_dir).await,
            None => ToolResult {
                success: false,
                output: format!("unknown tool: {name}"),
            },
        }
    }

    /// List all registered tool names.
    pub fn tool_names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }
}
