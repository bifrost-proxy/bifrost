//! Bifrost Agent: AI agent runtime with tool calling, multi-turn dialogue, and memory compaction.
//!
//! Core capabilities (inspired by Codex agent architecture):
//! - **Turn loop**: prompt → model → tool_calls → execute → repeat
//! - **Tool system**: Shell execution, file read/write/patch, directory listing
//! - **Memory compaction**: Automatic history summarization when context grows too long
//! - **Token tracking**: Real API token usage tracking + coarse estimates
//! - **Context management**: History limits, compaction summary preservation
//! - **Chat Completions API**: Standard format with function calling
//! - **AGENTS.md**: Project instruction loading from hierarchy
//! - **Skills**: Extensible skill system with YAML frontmatter
//! - **MCP**: Model Context Protocol server integration
//! - **Persistence**: Conversation recording and replay
//!
//! # Example
//! ```rust,no_run
//! use bifrost_agent::{AgentClient, AgentConfig, ToolRegistry};
//! use bifrost_agent::session::{AgentSession, AgentSessionManager, run_turn};
//!
//! async fn example() {
//!     let config = AgentConfig::default();
//!     let client = AgentClient::new();
//!     let tools = ToolRegistry::with_defaults(config.get_shell_timeout_secs());
//!     let mut session = AgentSession::new("user-123");
//!
//!     let result = run_turn(&client, &config, &mut session, &tools, "echo hello", None)
//!         .await
//!         .unwrap();
//!     println!("Response: {}", result.response);
//! }
//! ```

pub mod agents_md;
pub mod client;
pub mod compact;
pub mod config;
pub mod history;
pub mod mcp;
pub mod memory_runtime;
pub mod persistence;
pub mod prompt;
pub mod session;
pub mod skills;
pub mod tools;
pub mod types;

// Re-export main types for convenience
pub use client::AgentClient;
pub use compact::CompactionResult;
pub use config::{
    list_builtin_providers, AgentConfig, AgentConfigStore, HistoryConfig, HistoryPersistence,
    McpServerConfig, MemoriesConfig, ModelProviderConfig, ProviderInfo,
};
pub use session::{AgentSession, AgentSessionManager, SessionDetail, SessionInfo};
pub use skills::{SkillMetadata, SkillScope, SkillsManager};
pub use tools::ToolRegistry;
pub use types::{ChatMessage, ToolCallLog, TurnResult};
