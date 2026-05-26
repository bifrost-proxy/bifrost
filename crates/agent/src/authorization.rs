//! Open-by-default tool authorization and orchestration for the built-in agent.
//!
//! Bifrost keeps the IM Agent runtime fully authorized by default. The policy
//! types exist so every tool path goes through one contract even though the
//! default decision is always allow and the sandbox is disabled.

use crate::types::ToolResult;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::path::{Path, PathBuf};
use tracing::debug;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PermissionProfile {
    /// Fully authorized profile used by the embedded Bifrost IM Agent.
    #[default]
    Open,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalPolicy {
    /// Never ask for approval; execute immediately.
    #[default]
    NeverAsk,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SandboxPolicy {
    /// No filesystem/process/network sandbox is applied.
    #[default]
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolAuthorizationConfig {
    #[serde(default)]
    pub permission_profile: PermissionProfile,
    #[serde(default)]
    pub approval_policy: ApprovalPolicy,
    #[serde(default)]
    pub sandbox_policy: SandboxPolicy,
}

impl ToolAuthorizationConfig {
    pub fn open() -> Self {
        Self::default()
    }

    pub fn allows_without_prompt(&self) -> bool {
        matches!(self.permission_profile, PermissionProfile::Open)
            && matches!(self.approval_policy, ApprovalPolicy::NeverAsk)
            && matches!(self.sandbox_policy, SandboxPolicy::Disabled)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolInvocationKind {
    Local,
    Mcp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolInvocationContext {
    pub kind: ToolInvocationKind,
    pub name: String,
    pub work_dir: Option<PathBuf>,
}

/// Shared dispatcher contract for local tools, file operations, patching,
/// terminal execution, and MCP calls.
#[derive(Debug, Clone)]
pub struct ToolOrchestrator {
    config: ToolAuthorizationConfig,
}

impl Default for ToolOrchestrator {
    fn default() -> Self {
        Self::open()
    }
}

impl ToolOrchestrator {
    pub fn open() -> Self {
        Self {
            config: ToolAuthorizationConfig::open(),
        }
    }

    pub fn new(config: ToolAuthorizationConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &ToolAuthorizationConfig {
        &self.config
    }

    pub fn decision_for(&self, context: &ToolInvocationContext) -> Result<(), ToolResult> {
        if self.config.allows_without_prompt() {
            debug!(
                tool = %context.name,
                kind = ?context.kind,
                work_dir = context.work_dir.as_ref().map(|p| p.display().to_string()),
                "tool authorization allowed by open Bifrost Agent policy"
            );
            Ok(())
        } else {
            Err(ToolResult {
                success: false,
                output: "tool authorization denied by policy".to_string(),
                runtime_events: Vec::new(),
            })
        }
    }

    pub async fn run_local<F, Fut>(&self, name: &str, work_dir: &Path, execute: F) -> ToolResult
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = ToolResult>,
    {
        let context = ToolInvocationContext {
            kind: ToolInvocationKind::Local,
            name: name.to_string(),
            work_dir: Some(work_dir.to_path_buf()),
        };
        match self.decision_for(&context) {
            Ok(()) => execute().await,
            Err(result) => result,
        }
    }

    pub async fn run_mcp<F, Fut>(&self, name: &str, execute: F) -> ToolResult
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = ToolResult>,
    {
        let context = ToolInvocationContext {
            kind: ToolInvocationKind::Mcp,
            name: name.to_string(),
            work_dir: None,
        };
        match self.decision_for(&context) {
            Ok(()) => execute().await,
            Err(result) => result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_is_open_without_sandbox_or_approval() {
        let config = ToolAuthorizationConfig::default();
        assert_eq!(config.permission_profile, PermissionProfile::Open);
        assert_eq!(config.approval_policy, ApprovalPolicy::NeverAsk);
        assert_eq!(config.sandbox_policy, SandboxPolicy::Disabled);
        assert!(config.allows_without_prompt());
    }

    #[tokio::test]
    async fn orchestrator_allows_local_and_mcp_tools() {
        let orchestrator = ToolOrchestrator::open();
        let work_dir = tempfile::tempdir().expect("work dir");
        let local = orchestrator
            .run_local("exec_command", work_dir.path(), || async {
                ToolResult {
                    success: true,
                    output: "local-ok".to_string(),
                    runtime_events: Vec::new(),
                }
            })
            .await;
        let mcp = orchestrator
            .run_mcp("server.tool", || async {
                ToolResult {
                    success: true,
                    output: "mcp-ok".to_string(),
                    runtime_events: Vec::new(),
                }
            })
            .await;

        assert!(local.success);
        assert_eq!(local.output, "local-ok");
        assert!(mcp.success);
        assert_eq!(mcp.output, "mcp-ok");
    }
}
