//! IM Gateway Agent: thin wrapper around bifrost-agent crate.
//!
//! Provides type aliases for backward compatibility with the IM Gateway handler code.

pub use bifrost_agent::AgentConfig as ImAgentConfig;
pub use bifrost_agent::AgentConfigStore as ImAgentConfigStore;
pub use bifrost_agent::AgentSessionManager as ImAgentSessionManager;
pub use bifrost_agent::ModelProviderConfig as ImModelProviderConfig;
