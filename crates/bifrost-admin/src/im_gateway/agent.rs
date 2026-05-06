//! IM Gateway Agent: thin wrapper around bifrost-agent crate.
//!
//! Provides type aliases for backward compatibility with the IM Gateway handler code.

pub use bifrost_agent::mcp::McpManager as ImMcpManager;
pub use bifrost_agent::session::run_turn;
pub use bifrost_agent::session::run_turn_with_mcp;
pub use bifrost_agent::AgentClient as ImAgentClient;
pub use bifrost_agent::AgentConfig as ImAgentConfig;
pub use bifrost_agent::AgentConfigStore as ImAgentConfigStore;
pub use bifrost_agent::AgentSessionManager as ImAgentSessionManager;
pub use bifrost_agent::ModelProviderConfig as ImModelProviderConfig;
pub use bifrost_agent::ToolRegistry as ImAgentToolRegistry;
pub use bifrost_agent::TurnResult;
