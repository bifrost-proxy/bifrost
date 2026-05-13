//! Tool system: ToolHandler trait, ToolRegistry, and built-in tools.
//!
//! Built-in tools:
//! - `write_file`: Write content to a file
//! - `read_file`: Read file contents (with offset/limit)
//! - `list_directory`: List directory entries
//! - `apply_patch`: Apply structured diff patches
//! - `exec_command`: Run shell commands with optional PTY-backed sessions
//! - `write_stdin`: Write to an exec_command session stdin
//! - `view_image`: Load local image files as data URLs
//! - `request_user_input`: Validate structured user input requests
//! - `tool_search`: turn-scoped deferred tool discovery (registered only when
//!   deferred tools exist)
//! - `get_goal`/`create_goal`/`update_goal`: Goal tracking system for task management

pub mod apply_patch_diff;
pub mod exec_command;
pub mod file_ops;
pub mod goal;
pub mod head_tail_buffer;
pub mod request_user_input;
pub mod research;
pub mod set_title;
pub mod switch_workdir;
pub mod tool_search;
pub mod update_plan;
pub mod view_image;

use crate::session_status::AgentTurnProgressSender;
use crate::types::{ToolDefinition, ToolResult};
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

    /// Execute the tool with an optional live progress channel.
    async fn execute_with_progress(
        &self,
        arguments: &str,
        work_dir: &Path,
        progress_sender: Option<AgentTurnProgressSender>,
    ) -> ToolResult {
        let _ = progress_sender;
        self.execute(arguments, work_dir).await
    }
}

/// Registry of available tools.
#[derive(Clone)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn ToolHandler>>,
    exec_session_manager: Option<Arc<exec_command::ExecSessionManager>>,
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
            exec_session_manager: None,
        }
    }

    /// Register a tool handler.
    pub fn register(&mut self, handler: Arc<dyn ToolHandler>) {
        self.tools.insert(handler.name().to_string(), handler);
    }

    /// Create a registry with all built-in tools.
    pub fn with_defaults() -> Self {
        let mut registry = Self::new();
        let exec_session_manager = Arc::new(exec_command::ExecSessionManager::new());
        registry.exec_session_manager = Some(exec_session_manager.clone());
        registry.register(Arc::new(exec_command::ExecCommandTool::new(
            exec_session_manager.clone(),
        )));
        registry.register(Arc::new(file_ops::WriteFileTool));
        registry.register(Arc::new(file_ops::ReadFileTool));
        registry.register(Arc::new(file_ops::ListDirectoryTool));
        registry.register(Arc::new(switch_workdir::SwitchWorkdirTool));
        registry.register(Arc::new(update_plan::UpdatePlanTool));
        registry.register(Arc::new(set_title::SetTitleTool));
        registry.register(Arc::new(view_image::ViewImageTool));
        registry.register(Arc::new(request_user_input::RequestUserInputTool));
        // Structured patch tool.
        registry.register(Arc::new(apply_patch_diff::ApplyDiffTool));
        registry.register(Arc::new(exec_command::WriteStdinTool::new(
            exec_session_manager,
        )));
        registry
    }

    /// Create a registry with built-ins plus config-gated tools.
    pub fn with_agent_config(config: &crate::config::AgentConfig) -> Self {
        Self::with_agent_config_and_home(config, crate::config::agent_home_dir())
    }

    /// Create a registry with built-ins plus config-gated tools using an explicit agent home.
    pub fn with_agent_config_and_home(
        config: &crate::config::AgentConfig,
        agent_home: impl Into<std::path::PathBuf>,
    ) -> Self {
        let mut registry = Self::with_defaults();
        let agent_home = agent_home.into();
        if config
            .research
            .as_ref()
            .is_some_and(|research| research.enabled)
        {
            match config
                .research
                .clone()
                .map(|research| {
                    crate::research::ResearchRuntime::from_config_with_home(
                        research,
                        agent_home.clone(),
                    )
                })
                .transpose()
            {
                Ok(Some(runtime)) => {
                    register_research_tools(&mut registry, runtime);
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(%error, "research tools disabled due to invalid config");
                }
            }
        }
        registry
    }

    /// Apply turn-level terminal runtime options to the shared exec session manager.
    pub fn configure_exec_sessions(&self, max_background_terminal_timeout_ms: u64) {
        if let Some(manager) = &self.exec_session_manager {
            manager.set_max_background_terminal_timeout(max_background_terminal_timeout_ms);
        }
    }

    /// Get tool definitions for the model API `tools` parameter.
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        let mut definitions: Vec<_> = self
            .tools
            .values()
            .map(|h| {
                if h.name() == apply_patch_diff::APPLY_PATCH_TOOL_NAME {
                    apply_patch_diff::apply_patch_tool_definition()
                } else {
                    ToolDefinition::function(
                        h.name().to_string(),
                        h.description().to_string(),
                        Some(h.parameters_schema()),
                    )
                }
            })
            .collect();
        definitions.sort_by(|left, right| {
            model_visible_tool_priority(left.name()).cmp(&model_visible_tool_priority(right.name()))
        });
        definitions.extend(goal::goal_tool_definitions());
        definitions
    }

    /// Execute a tool by name with the given arguments.
    pub async fn execute(&self, name: &str, arguments: &str, work_dir: &Path) -> ToolResult {
        match self.handler(name) {
            Some(handler) => handler.execute(arguments, work_dir).await,
            None => ToolResult {
                success: false,
                output: format!("unknown tool: {name}"),
            },
        }
    }

    /// Execute a tool by name with an optional live progress channel.
    pub async fn execute_with_progress(
        &self,
        name: &str,
        arguments: &str,
        work_dir: &Path,
        progress_sender: Option<AgentTurnProgressSender>,
    ) -> ToolResult {
        match self.handler(name) {
            Some(handler) => {
                handler
                    .execute_with_progress(arguments, work_dir, progress_sender)
                    .await
            }
            None => ToolResult {
                success: false,
                output: format!("unknown tool: {name}"),
            },
        }
    }

    /// Resolve a local tool handler by its registered tool name.
    pub fn handler(&self, name: &str) -> Option<Arc<dyn ToolHandler>> {
        self.tools.get(name).cloned()
    }

    /// Returns true when the name can be handled by this local registry.
    pub fn contains_tool(&self, name: &str) -> bool {
        self.handler(name).is_some()
    }

    /// List all registered tool names.
    pub fn tool_names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.tools.keys().cloned().collect();
        names.extend(goal::goal_tool_names().into_iter().map(str::to_string));
        names
    }
}

fn register_research_tools(registry: &mut ToolRegistry, runtime: crate::research::ResearchRuntime) {
    let runtime = Arc::new(runtime);
    registry.register(Arc::new(research::ResearchSearchTool::new(runtime.clone())));
    registry.register(Arc::new(research::ResearchFetchTool::new(runtime.clone())));
    registry.register(Arc::new(research::KnowledgeSearchTool::new(
        runtime.clone(),
    )));
    registry.register(Arc::new(research::KnowledgeSaveTool::new(runtime.clone())));
    registry.register(Arc::new(research::ResearchDigestTool::new(runtime)));
}

fn model_visible_tool_priority(name: &str) -> (u8, &str) {
    let priority = match name {
        "exec_command" => 0,
        "write_stdin" => 1,
        _ => 5,
    };
    (priority, name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_visible_tool_definitions_prefer_unified_exec_tools() {
        let definitions = ToolRegistry::with_defaults().definitions();
        let names = definitions
            .iter()
            .map(ToolDefinition::name)
            .collect::<Vec<_>>();
        let exec_command = names
            .iter()
            .position(|name| *name == "exec_command")
            .unwrap();
        let write_stdin = names
            .iter()
            .position(|name| *name == "write_stdin")
            .unwrap();
        assert!(!names.contains(&"shell"));
        assert!(!names.contains(&"shell_pty"));
        assert!(exec_command < names.len());
        assert!(write_stdin < names.len());
        assert!(exec_command < write_stdin);
        assert!(!ToolRegistry::with_defaults().contains_tool("shell"));
        assert!(!ToolRegistry::with_defaults().contains_tool("shell_pty"));
    }

    #[test]
    fn research_tools_are_config_gated() {
        let disabled = crate::config::AgentConfig::default();
        assert!(!ToolRegistry::with_agent_config(&disabled).contains_tool("research_search"));

        let enabled = crate::config::AgentConfig {
            research: Some(crate::research::default_enabled_config()),
            ..crate::config::AgentConfig::default()
        };
        assert!(ToolRegistry::with_agent_config(&enabled).contains_tool("research_search"));
        assert!(ToolRegistry::with_agent_config(&enabled).contains_tool("knowledge_save"));
    }

    #[tokio::test]
    async fn unknown_shell_aliases_are_rejected() {
        let registry = ToolRegistry::with_defaults();
        assert!(!registry.contains_tool("shell_command"));
        assert!(!registry.contains_tool("local_shell"));

        let work_dir = tempfile::tempdir().expect("work dir");
        let result = registry
            .execute(
                "shell_command",
                r#"{"command":"printf alias-ok"}"#,
                work_dir.path(),
            )
            .await;
        assert!(!result.success);
        assert!(result.output.contains("unknown tool: shell_command"));
    }
}
