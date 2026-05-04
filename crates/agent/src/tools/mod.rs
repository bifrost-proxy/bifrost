//! Tool system: ToolHandler trait, ToolRegistry, and built-in tools.
//!
//! Built-in tools:
//! - `shell`: Execute shell commands (zsh/bash, auto-detected)
//! - `write_file`: Write content to a file
//! - `read_file`: Read file contents (with offset/limit)
//! - `list_directory`: List directory entries
//! - `apply_patch`: Apply Codex-compatible structured diff patches
//! - `pty_shell`: PTY-backed persistent shell sessions with session management
//! - `write_stdin`: Write to PTY session stdin
//! - `get_goal`/`create_goal`/`update_goal`: Goal tracking system for task management

pub mod apply_patch_diff;
pub mod file_ops;
pub mod goal;
pub mod head_tail_buffer;
pub mod pty_shell;
pub mod set_title;
pub mod shell;
pub mod switch_workdir;
pub mod update_plan;

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
        // Existing tools
        registry.register(Arc::new(shell::ShellTool::new(shell_timeout_secs)));
        registry.register(Arc::new(file_ops::WriteFileTool));
        registry.register(Arc::new(file_ops::ReadFileTool));
        registry.register(Arc::new(file_ops::ListDirectoryTool));
        registry.register(Arc::new(switch_workdir::SwitchWorkdirTool));
        registry.register(Arc::new(update_plan::UpdatePlanTool));
        registry.register(Arc::new(set_title::SetTitleTool));
        // Goal tracking tools
        let goal_manager = Arc::new(goal::GoalManager::new());
        registry.register(Arc::new(goal::GetGoalTool::new(goal_manager.clone())));
        registry.register(Arc::new(goal::CreateGoalTool::new(goal_manager.clone())));
        registry.register(Arc::new(goal::UpdateGoalTool::new(goal_manager)));
        // Codex-compatible structured patch tool
        registry.register(Arc::new(apply_patch_diff::ApplyDiffTool));
        // PTY shell tools (persistent sessions)
        let session_manager = Arc::new(pty_shell::PtySessionManager::new());
        registry.register(Arc::new(pty_shell::PtyShellTool::new(
            shell_timeout_secs,
            session_manager.clone(),
        )));
        registry.register(Arc::new(pty_shell::WriteStdinTool::new(session_manager)));
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
