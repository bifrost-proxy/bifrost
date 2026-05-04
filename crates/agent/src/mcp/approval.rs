//! MCP Tool Approval System and Deferred Loading Strategy.
//!
//! This module implements:
//! 1. **Tool Approval System**: Multi-layer approval logic for MCP tool calls
//!    with support for auto-approve, prompt, and explicit approve modes.
//! 2. **Deferred Loading Strategy**: When tool count exceeds threshold,
//!    only expose a subset directly and provide search for remaining tools.
//!
//! Design inspired by Codex's mcp_tool_call.rs and mcp_tool_exposure.rs.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Threshold for switching to deferred tool loading.
/// When total tools exceed this, enable deferred loading strategy.
pub const DEFERRED_LOADING_THRESHOLD: usize = 100;

/// Maximum size for tool call results (prevent oversized responses).
pub const MAX_RESULT_SIZE: usize = 512 * 1024; // 512KB

/// Maximum display length for argument values in approval messages.
const MAX_ARG_DISPLAY_LENGTH: usize = 200;

// ---------------------------------------------------------------------------
// ToolApprovalMode
// ---------------------------------------------------------------------------

/// Approval mode for MCP tool calls.
///
/// Controls whether tool calls require user confirmation before execution.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ToolApprovalMode {
    /// Always auto-approve, no user confirmation needed.
    /// Use with caution - tools will execute without any oversight.
    Auto,

    /// Prompt user for confirmation with a formatted message.
    /// Most common mode for interactive sessions.
    #[default]
    Prompt,

    /// Require explicit approval (stricter than Prompt).
    /// For high-security environments or destructive operations.
    Approve,
}

// ---------------------------------------------------------------------------
// ToolAnnotations
// ---------------------------------------------------------------------------

/// Metadata hints about a tool's behavior and safety characteristics.
///
/// These annotations are provided by MCP servers and help the approval engine
/// make smarter decisions about which tools need user confirmation.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ToolAnnotations {
    /// If true, the tool may perform destructive operations (delete, modify).
    #[serde(rename = "destructiveHint", skip_serializing_if = "Option::is_none")]
    pub destructive_hint: Option<bool>,

    /// If true, the tool only reads data without side effects.
    /// Read-only tools are generally safe to auto-approve.
    #[serde(rename = "readOnlyHint", skip_serializing_if = "Option::is_none")]
    pub read_only_hint: Option<bool>,

    /// If true, the tool interacts with external systems/services.
    #[serde(rename = "openWorldHint", skip_serializing_if = "Option::is_none")]
    pub open_world_hint: Option<bool>,

    /// If true, calling the tool multiple times with same args has same effect.
    #[serde(rename = "idempotentHint", skip_serializing_if = "Option::is_none")]
    pub idempotent_hint: Option<bool>,
}

impl ToolAnnotations {
    /// Check if this tool is safe to auto-approve.
    /// A tool is considered safe if it's read-only and not destructive.
    pub fn is_safe(&self) -> bool {
        self.read_only_hint == Some(true) && self.destructive_hint != Some(true)
    }

    /// Check if this tool might be destructive.
    pub fn is_destructive(&self) -> bool {
        self.destructive_hint == Some(true)
    }
}

// ---------------------------------------------------------------------------
// ApprovalDecision
// ---------------------------------------------------------------------------

/// Result of an approval check from the ApprovalEngine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalDecision {
    /// Auto-approved (no user interaction needed).
    Approved,

    /// Needs user confirmation with reason and formatted message.
    NeedsConfirmation {
        /// Technical reason for needing approval.
        reason: String,
        /// Human-readable message to display to user.
        message: String,
    },

    /// Tool call denied with reason.
    Denied {
        /// Reason for denial.
        reason: String,
    },
}

// ---------------------------------------------------------------------------
// ApprovalRequest
// ---------------------------------------------------------------------------

/// Request for user approval passed to the approval callback.
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    /// Name of the MCP server providing the tool.
    pub server_name: String,
    /// Name of the tool being called.
    pub tool_name: String,
    /// Arguments passed to the tool.
    pub arguments: serde_json::Value,
    /// Technical reason for needing approval.
    pub reason: String,
    /// Human-readable message to display.
    pub message: String,
}

// ---------------------------------------------------------------------------
// ApprovalCallback
// ---------------------------------------------------------------------------

/// Type alias for the approval callback future.
type ApprovalFuture = Pin<Box<dyn Future<Output = bool> + Send + 'static>>;

/// Approval callback type - return true to approve, false to deny.
pub type ApprovalCallback = Arc<dyn Fn(ApprovalRequest) -> ApprovalFuture + Send + Sync>;

// ---------------------------------------------------------------------------
// ApprovalEngine
// ---------------------------------------------------------------------------

/// Core approval engine that decides whether a tool call needs user approval.
///
/// Implements multi-layer approval logic:
/// 1. Check tool annotations - safe tools may be auto-approved
/// 2. Check per-tool override configuration
/// 3. Check per-server override configuration
/// 4. Fall back to default approval mode
/// 5. Check session memory (previously approved tools)
/// 6. If still needed, request interactive approval
pub struct ApprovalEngine {
    /// Default approval mode for tools without specific configuration.
    default_mode: ToolApprovalMode,

    /// Per-server approval mode overrides (key: server name).
    server_modes: HashMap<String, ToolApprovalMode>,

    /// Per-tool approval mode overrides (key: "server::tool").
    tool_modes: HashMap<String, ToolApprovalMode>,

    /// Session-level approval memory (tools approved during this session).
    /// Once approved, tools can be called again without re-approval.
    session_approvals: RwLock<HashSet<String>>,

    /// Optional callback for interactive approval requests.
    approval_callback: Option<ApprovalCallback>,
}

impl ApprovalEngine {
    /// Create a new approval engine with the given default mode.
    pub fn new(default_mode: ToolApprovalMode) -> Self {
        Self {
            default_mode,
            server_modes: HashMap::new(),
            tool_modes: HashMap::new(),
            session_approvals: RwLock::new(HashSet::new()),
            approval_callback: None,
        }
    }

    /// Create an approval engine with auto-approve as default.
    pub fn auto() -> Self {
        Self::new(ToolApprovalMode::Auto)
    }

    /// Create an approval engine with prompt as default.
    pub fn prompt() -> Self {
        Self::new(ToolApprovalMode::Prompt)
    }

    /// Configure server-level approval mode override.
    pub fn set_server_mode(&mut self, server_name: &str, mode: ToolApprovalMode) {
        self.server_modes.insert(server_name.to_string(), mode);
        debug!(
            server = %server_name,
            mode = ?mode,
            "Set server approval mode"
        );
    }

    /// Configure tool-level approval mode override.
    pub fn set_tool_mode(&mut self, server_name: &str, tool_name: &str, mode: ToolApprovalMode) {
        let key = format!("{}::{}", server_name, tool_name);
        self.tool_modes.insert(key, mode);
        debug!(
            server = %server_name,
            tool = %tool_name,
            mode = ?mode,
            "Set tool approval mode"
        );
    }

    /// Set the approval callback for interactive mode.
    pub fn set_callback(&mut self, callback: ApprovalCallback) {
        self.approval_callback = Some(callback);
    }

    /// Clear the approval callback.
    pub fn clear_callback(&mut self) {
        self.approval_callback = None;
    }

    /// Check if a tool call should be approved.
    ///
    /// This implements the multi-layer approval logic:
    /// 1. Check annotations - read-only tools may be auto-approved
    /// 2. Determine effective approval mode (tool > server > default)
    /// 3. For Prompt/Approve modes, check session memory
    /// 4. Build approval message if confirmation needed
    pub async fn check_approval(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: &serde_json::Value,
        annotations: Option<&ToolAnnotations>,
    ) -> ApprovalDecision {
        // 1. Check annotations - read-only tools may be auto-approved
        if let Some(ann) = annotations {
            if ann.is_safe() {
                debug!(
                    server = %server_name,
                    tool = %tool_name,
                    "Auto-approving read-only tool"
                );
                return ApprovalDecision::Approved;
            }
        }

        // 2. Determine effective approval mode (tool > server > default)
        let tool_key = format!("{}::{}", server_name, tool_name);
        let mode = self
            .tool_modes
            .get(&tool_key)
            .or_else(|| self.server_modes.get(server_name))
            .unwrap_or(&self.default_mode);

        match mode {
            ToolApprovalMode::Auto => {
                debug!(
                    server = %server_name,
                    tool = %tool_name,
                    mode = ?mode,
                    "Auto-approving due to mode"
                );
                ApprovalDecision::Approved
            }
            ToolApprovalMode::Prompt | ToolApprovalMode::Approve => {
                // 3. Check session memory
                {
                    let approvals = self.session_approvals.read().await;
                    if approvals.contains(&tool_key) {
                        debug!(
                            server = %server_name,
                            tool = %tool_name,
                            "Auto-approving (previously approved this session)"
                        );
                        return ApprovalDecision::Approved;
                    }
                }

                // 4. Build approval message
                let is_destructive = annotations
                    .as_ref()
                    .map(|a| a.is_destructive())
                    .unwrap_or(false);

                let reason = if is_destructive {
                    "This tool is marked as potentially destructive".to_string()
                } else {
                    format!(
                        "Tool '{}' from server '{}' requires approval",
                        tool_name, server_name
                    )
                };

                let message = format_approval_message(server_name, tool_name, arguments, &reason);

                ApprovalDecision::NeedsConfirmation { reason, message }
            }
        }
    }

    /// Record that a tool was approved in this session.
    ///
    /// After this, subsequent calls to the same tool won't require re-approval.
    pub async fn record_approval(&self, server_name: &str, tool_name: &str) {
        let tool_key = format!("{}::{}", server_name, tool_name);
        let mut approvals = self.session_approvals.write().await;
        approvals.insert(tool_key.clone());
        debug!(tool_key = %tool_key, "Recorded session approval");
    }

    /// Clear all session approvals.
    pub async fn clear_session_approvals(&self) {
        let mut approvals = self.session_approvals.write().await;
        approvals.clear();
        debug!("Cleared all session approvals");
    }

    /// Request interactive approval from user (via callback).
    ///
    /// Returns true if approved, false if denied or no callback set.
    pub async fn request_approval(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: &serde_json::Value,
        reason: &str,
        message: &str,
    ) -> bool {
        if let Some(callback) = &self.approval_callback {
            let request = ApprovalRequest {
                server_name: server_name.to_string(),
                tool_name: tool_name.to_string(),
                arguments: arguments.clone(),
                reason: reason.to_string(),
                message: message.to_string(),
            };
            callback(request).await
        } else {
            warn!(
                server = %server_name,
                tool = %tool_name,
                "No approval callback set, auto-declining"
            );
            false
        }
    }

    /// Full approval flow: check -> request if needed -> record.
    ///
    /// This is the primary entry point for tool approval.
    /// Returns Ok(true) if approved, Ok(false) if user declined, Err if denied.
    pub async fn approve_tool_call(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: &serde_json::Value,
        annotations: Option<&ToolAnnotations>,
    ) -> Result<bool, String> {
        match self
            .check_approval(server_name, tool_name, arguments, annotations)
            .await
        {
            ApprovalDecision::Approved => Ok(true),
            ApprovalDecision::Denied { reason } => {
                warn!(
                    server = %server_name,
                    tool = %tool_name,
                    reason = %reason,
                    "Tool call denied"
                );
                Err(reason)
            }
            ApprovalDecision::NeedsConfirmation { reason, message } => {
                let approved = self
                    .request_approval(server_name, tool_name, arguments, &reason, &message)
                    .await;
                if approved {
                    self.record_approval(server_name, tool_name).await;
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
        }
    }

    /// Get the effective approval mode for a tool.
    pub fn get_effective_mode(&self, server_name: &str, tool_name: &str) -> ToolApprovalMode {
        let tool_key = format!("{}::{}", server_name, tool_name);
        self.tool_modes
            .get(&tool_key)
            .or_else(|| self.server_modes.get(server_name))
            .copied()
            .unwrap_or(self.default_mode)
    }
}

impl Default for ApprovalEngine {
    fn default() -> Self {
        Self::prompt()
    }
}

// ---------------------------------------------------------------------------
// Approval Message Formatting
// ---------------------------------------------------------------------------

/// Format a human-readable approval message.
///
/// Includes server name, tool name, reason, and formatted arguments.
pub fn format_approval_message(
    server_name: &str,
    tool_name: &str,
    arguments: &serde_json::Value,
    reason: &str,
) -> String {
    let mut msg = format!(
        "MCP Tool Call Approval Required\n\
         ================================\n\
         Server: {}\n\
         Tool: {}\n\
         Reason: {}\n",
        server_name, tool_name, reason
    );

    // Format arguments
    if let Some(obj) = arguments.as_object() {
        if !obj.is_empty() {
            msg.push_str("\nArguments:\n");
            for (key, value) in obj {
                let display_value = format_arg_value(value);
                msg.push_str(&format!("  {}: {}\n", key, display_value));
            }
        }
    } else if !arguments.is_null() {
        // Non-object arguments (array, string, number, etc.)
        let display_value = format_arg_value(arguments);
        msg.push_str(&format!("\nArguments: {}\n", display_value));
    }

    msg
}

/// Format an argument value for display, with truncation for long values.
fn format_arg_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => {
            if s.len() > MAX_ARG_DISPLAY_LENGTH {
                format!("{}... (truncated)", &s[..MAX_ARG_DISPLAY_LENGTH])
            } else {
                s.clone()
            }
        }
        _ => {
            let s = serde_json::to_string_pretty(value).unwrap_or_default();
            if s.len() > MAX_ARG_DISPLAY_LENGTH {
                format!("{}... (truncated)", &s[..MAX_ARG_DISPLAY_LENGTH])
            } else {
                s
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Result Truncation
// ---------------------------------------------------------------------------

/// Truncate a tool call result if it exceeds the maximum size.
///
/// Prevents oversized responses from being returned to the model.
pub fn truncate_result(result: &str) -> String {
    if result.len() <= MAX_RESULT_SIZE {
        result.to_string()
    } else {
        let truncated = &result[..MAX_RESULT_SIZE];
        format!(
            "{}\n\n[Result truncated: {} bytes total, showing first {} bytes]",
            truncated,
            result.len(),
            MAX_RESULT_SIZE
        )
    }
}

// ---------------------------------------------------------------------------
// ToolInfo (for deferred loading)
// ---------------------------------------------------------------------------

/// Information about a tool for deferred loading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolInfo {
    /// Name of the MCP server providing the tool.
    pub server_name: String,
    /// Original tool name from the server.
    pub tool_name: String,
    /// Prefixed name exposed to the model (e.g., "server__tool").
    pub prefixed_name: String,
    /// Tool description.
    pub description: Option<String>,
}

impl ToolInfo {
    /// Create a new ToolInfo.
    pub fn new(server_name: &str, tool_name: &str, description: Option<String>) -> Self {
        let prefixed_name = format!("{}__{}", server_name, tool_name);
        Self {
            server_name: server_name.to_string(),
            tool_name: tool_name.to_string(),
            prefixed_name,
            description,
        }
    }
}

// ---------------------------------------------------------------------------
// ToolExposure (Deferred Loading Strategy)
// ---------------------------------------------------------------------------

/// Tool exposure strategy for deferred loading.
///
/// When the total number of tools exceeds a threshold, only a subset
/// is directly exposed to the model. The rest are available via search.
#[derive(Debug)]
pub struct ToolExposure {
    /// Tools immediately available to the model.
    pub direct_tools: Vec<ToolInfo>,
    /// Tools available via search (deferred loading).
    pub deferred_tools: Vec<ToolInfo>,
    /// Whether deferred loading is active.
    pub deferred_loading_active: bool,
}

impl ToolExposure {
    /// Calculate tool exposure from all available tools.
    ///
    /// If total tools <= threshold, all are direct.
    /// Otherwise, first N tools are direct, rest are deferred.
    pub fn calculate(all_tools: Vec<ToolInfo>) -> Self {
        let total = all_tools.len();

        if total <= DEFERRED_LOADING_THRESHOLD {
            // All tools directly available
            ToolExposure {
                direct_tools: all_tools,
                deferred_tools: Vec::new(),
                deferred_loading_active: false,
            }
        } else {
            info!(
                total = total,
                threshold = DEFERRED_LOADING_THRESHOLD,
                "MCP tool count exceeds threshold, enabling deferred loading"
            );

            // Split: first N tools are direct, rest are deferred
            let (direct, deferred): (Vec<_>, Vec<_>) = all_tools
                .into_iter()
                .enumerate()
                .partition(|(i, _)| *i < DEFERRED_LOADING_THRESHOLD);

            ToolExposure {
                direct_tools: direct.into_iter().map(|(_, t)| t).collect(),
                deferred_tools: deferred.into_iter().map(|(_, t)| t).collect(),
                deferred_loading_active: true,
            }
        }
    }

    /// Calculate tool exposure with prioritization.
    ///
    /// Tools from prioritized servers are placed in direct_tools first.
    pub fn calculate_with_priority(all_tools: Vec<ToolInfo>, priority_servers: &[String]) -> Self {
        let total = all_tools.len();

        if total <= DEFERRED_LOADING_THRESHOLD {
            return ToolExposure {
                direct_tools: all_tools,
                deferred_tools: Vec::new(),
                deferred_loading_active: false,
            };
        }

        let priority_set: HashSet<_> = priority_servers.iter().collect();

        // Separate priority and non-priority tools
        let (mut priority, non_priority): (Vec<_>, Vec<_>) = all_tools
            .into_iter()
            .partition(|t| priority_set.contains(&t.server_name));

        // Fill direct tools with priority first
        let direct_count = DEFERRED_LOADING_THRESHOLD.min(total);
        let mut direct_tools = Vec::with_capacity(direct_count);
        let mut deferred_tools = Vec::new();

        // Add priority tools first
        while direct_tools.len() < direct_count && !priority.is_empty() {
            direct_tools.push(priority.remove(0));
        }

        // Fill remaining with non-priority
        let remaining = direct_count - direct_tools.len();
        direct_tools.extend(non_priority.iter().take(remaining).cloned());

        // Remaining tools go to deferred
        deferred_tools.extend(priority);
        deferred_tools.extend(non_priority.into_iter().skip(remaining));

        info!(
            total = total,
            direct = direct_tools.len(),
            deferred = deferred_tools.len(),
            "MCP deferred loading enabled with priority servers"
        );

        ToolExposure {
            direct_tools,
            deferred_tools,
            deferred_loading_active: true,
        }
    }

    /// Search deferred tools by keyword.
    ///
    /// Searches in tool name and description (case-insensitive).
    pub fn search_deferred(&self, query: &str) -> Vec<&ToolInfo> {
        if !self.deferred_loading_active {
            return Vec::new();
        }

        let query_lower = query.to_lowercase();
        self.deferred_tools
            .iter()
            .filter(|t| {
                t.tool_name.to_lowercase().contains(&query_lower)
                    || t.description
                        .as_ref()
                        .map(|d| d.to_lowercase().contains(&query_lower))
                        .unwrap_or(false)
            })
            .collect()
    }

    /// Generate the search tool definition (when deferred loading is active).
    ///
    /// Returns None if deferred loading is not active.
    pub fn search_tool_definition(&self) -> Option<serde_json::Value> {
        if !self.deferred_loading_active {
            return None;
        }

        Some(serde_json::json!({
            "name": "mcp__search_tools",
            "description": format!(
                "Search for additional MCP tools. There are {} tools available that are not directly listed. Use this to find specific tools by name or description.",
                self.deferred_tools.len()
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search keyword to find tools"
                    }
                },
                "required": ["query"]
            }
        }))
    }

    /// Get total tool count.
    pub fn total_count(&self) -> usize {
        self.direct_tools.len() + self.deferred_tools.len()
    }
}

// ---------------------------------------------------------------------------
// Unit Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // ToolAnnotations Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_annotations_is_safe() {
        // Safe: read-only and not destructive
        let safe = ToolAnnotations {
            read_only_hint: Some(true),
            destructive_hint: Some(false),
            ..Default::default()
        };
        assert!(safe.is_safe());

        // Unsafe: destructive
        let destructive = ToolAnnotations {
            read_only_hint: Some(true),
            destructive_hint: Some(true),
            ..Default::default()
        };
        assert!(!destructive.is_safe());

        // Unsafe: not read-only
        let not_readonly = ToolAnnotations {
            read_only_hint: Some(false),
            destructive_hint: Some(false),
            ..Default::default()
        };
        assert!(!not_readonly.is_safe());

        // Unsafe: no hints
        let no_hints = ToolAnnotations::default();
        assert!(!no_hints.is_safe());
    }

    #[test]
    fn test_annotations_is_destructive() {
        let destructive = ToolAnnotations {
            destructive_hint: Some(true),
            ..Default::default()
        };
        assert!(destructive.is_destructive());

        let not_destructive = ToolAnnotations {
            destructive_hint: Some(false),
            ..Default::default()
        };
        assert!(!not_destructive.is_destructive());

        let unknown = ToolAnnotations::default();
        assert!(!unknown.is_destructive());
    }

    // -------------------------------------------------------------------------
    // ApprovalEngine Tests
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn test_check_approval_auto_mode() {
        let engine = ApprovalEngine::auto();

        // Should auto-approve any tool
        let decision = engine
            .check_approval(
                "test-server",
                "test-tool",
                &serde_json::json!({"arg": "value"}),
                None,
            )
            .await;
        assert_eq!(decision, ApprovalDecision::Approved);
    }

    #[tokio::test]
    async fn test_check_approval_safe_tool_auto_approves() {
        let engine = ApprovalEngine::prompt();

        // Read-only tool should auto-approve even in prompt mode
        let safe_annotations = ToolAnnotations {
            read_only_hint: Some(true),
            destructive_hint: Some(false),
            ..Default::default()
        };

        let decision = engine
            .check_approval(
                "test-server",
                "read-tool",
                &serde_json::json!({}),
                Some(&safe_annotations),
            )
            .await;
        assert_eq!(decision, ApprovalDecision::Approved);
    }

    #[tokio::test]
    async fn test_check_approval_destructive_needs_confirmation() {
        let engine = ApprovalEngine::prompt();

        let destructive_annotations = ToolAnnotations {
            destructive_hint: Some(true),
            ..Default::default()
        };

        let decision = engine
            .check_approval(
                "test-server",
                "delete-tool",
                &serde_json::json!({"file": "test.txt"}),
                Some(&destructive_annotations),
            )
            .await;

        match decision {
            ApprovalDecision::NeedsConfirmation { reason, .. } => {
                assert!(reason.contains("destructive"));
            }
            _ => panic!("Expected NeedsConfirmation for destructive tool"),
        }
    }

    #[tokio::test]
    async fn test_check_approval_session_memory() {
        let engine = ApprovalEngine::prompt();

        // First call needs confirmation
        let decision = engine
            .check_approval("test-server", "test-tool", &serde_json::json!({}), None)
            .await;
        assert!(matches!(
            decision,
            ApprovalDecision::NeedsConfirmation { .. }
        ));

        // Record approval
        engine.record_approval("test-server", "test-tool").await;

        // Second call should auto-approve (session memory)
        let decision = engine
            .check_approval("test-server", "test-tool", &serde_json::json!({}), None)
            .await;
        assert_eq!(decision, ApprovalDecision::Approved);

        // Different tool still needs confirmation
        let decision = engine
            .check_approval("test-server", "other-tool", &serde_json::json!({}), None)
            .await;
        assert!(matches!(
            decision,
            ApprovalDecision::NeedsConfirmation { .. }
        ));
    }

    #[tokio::test]
    async fn test_server_mode_override() {
        let mut engine = ApprovalEngine::prompt();
        engine.set_server_mode("trusted-server", ToolApprovalMode::Auto);

        // Tool from trusted server should auto-approve
        let decision = engine
            .check_approval("trusted-server", "any-tool", &serde_json::json!({}), None)
            .await;
        assert_eq!(decision, ApprovalDecision::Approved);

        // Tool from other server still needs confirmation
        let decision = engine
            .check_approval("other-server", "any-tool", &serde_json::json!({}), None)
            .await;
        assert!(matches!(
            decision,
            ApprovalDecision::NeedsConfirmation { .. }
        ));
    }

    #[tokio::test]
    async fn test_tool_mode_override() {
        let mut engine = ApprovalEngine::prompt();
        engine.set_tool_mode("test-server", "safe-tool", ToolApprovalMode::Auto);

        // Specific tool should auto-approve
        let decision = engine
            .check_approval("test-server", "safe-tool", &serde_json::json!({}), None)
            .await;
        assert_eq!(decision, ApprovalDecision::Approved);

        // Other tool from same server still needs confirmation
        let decision = engine
            .check_approval("test-server", "other-tool", &serde_json::json!({}), None)
            .await;
        assert!(matches!(
            decision,
            ApprovalDecision::NeedsConfirmation { .. }
        ));
    }

    #[tokio::test]
    async fn test_tool_mode_overrides_server_mode() {
        let mut engine = ApprovalEngine::prompt();
        engine.set_server_mode("test-server", ToolApprovalMode::Auto);
        engine.set_tool_mode("test-server", "dangerous-tool", ToolApprovalMode::Approve);

        // Tool-level override wins
        let decision = engine
            .check_approval(
                "test-server",
                "dangerous-tool",
                &serde_json::json!({}),
                None,
            )
            .await;
        assert!(matches!(
            decision,
            ApprovalDecision::NeedsConfirmation { .. }
        ));

        // Other tools use server mode
        let decision = engine
            .check_approval("test-server", "other-tool", &serde_json::json!({}), None)
            .await;
        assert_eq!(decision, ApprovalDecision::Approved);
    }

    #[tokio::test]
    async fn test_get_effective_mode() {
        let mut engine = ApprovalEngine::prompt();
        engine.set_server_mode("server-a", ToolApprovalMode::Auto);
        engine.set_tool_mode("server-a", "tool-x", ToolApprovalMode::Approve);

        // Tool override
        assert_eq!(
            engine.get_effective_mode("server-a", "tool-x"),
            ToolApprovalMode::Approve
        );

        // Server override
        assert_eq!(
            engine.get_effective_mode("server-a", "tool-y"),
            ToolApprovalMode::Auto
        );

        // Default
        assert_eq!(
            engine.get_effective_mode("server-b", "tool-z"),
            ToolApprovalMode::Prompt
        );
    }

    // -------------------------------------------------------------------------
    // Approval Message Formatting Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_format_approval_message_basic() {
        let msg = format_approval_message(
            "my-server",
            "my-tool",
            &serde_json::json!({}),
            "Test reason",
        );

        assert!(msg.contains("my-server"));
        assert!(msg.contains("my-tool"));
        assert!(msg.contains("Test reason"));
    }

    #[test]
    fn test_format_approval_message_with_args() {
        let msg = format_approval_message(
            "my-server",
            "my-tool",
            &serde_json::json!({
                "path": "/some/path",
                "content": "hello world"
            }),
            "Test reason",
        );

        assert!(msg.contains("path: /some/path"));
        assert!(msg.contains("content: hello world"));
    }

    #[test]
    fn test_format_approval_message_truncates_long_values() {
        let long_string = "x".repeat(300);
        let msg = format_approval_message(
            "my-server",
            "my-tool",
            &serde_json::json!({
                "long_arg": long_string.clone()
            }),
            "Test reason",
        );

        assert!(msg.contains("truncated"));
        assert!(!msg.contains(&long_string));
    }

    #[test]
    fn test_format_arg_value_string() {
        let value = serde_json::json!("hello");
        assert_eq!(format_arg_value(&value), "hello");
    }

    #[test]
    fn test_format_arg_value_truncates() {
        let long_string = "x".repeat(300);
        let value = serde_json::json!(long_string);
        let formatted = format_arg_value(&value);

        assert!(formatted.contains("truncated"));
        assert!(formatted.len() < long_string.len() + 50);
    }

    #[test]
    fn test_format_arg_value_json() {
        let value = serde_json::json!({"key": "value"});
        let formatted = format_arg_value(&value);
        assert!(formatted.contains("key"));
        assert!(formatted.contains("value"));
    }

    // -------------------------------------------------------------------------
    // Result Truncation Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_truncate_result_small() {
        let result = "hello world";
        assert_eq!(truncate_result(result), result);
    }

    #[test]
    fn test_truncate_result_large() {
        let large_result = "x".repeat(MAX_RESULT_SIZE + 1000);
        let truncated = truncate_result(&large_result);

        assert!(truncated.len() > MAX_RESULT_SIZE);
        assert!(truncated.len() < large_result.len());
        assert!(truncated.contains("truncated"));
        assert!(truncated.contains(&format!("{}", large_result.len())));
    }

    #[test]
    fn test_truncate_result_exactly_at_limit() {
        let result = "x".repeat(MAX_RESULT_SIZE);
        let truncated = truncate_result(&result);

        // Should not truncate if exactly at limit
        assert_eq!(truncated.len(), MAX_RESULT_SIZE);
        assert!(!truncated.contains("truncated"));
    }

    // -------------------------------------------------------------------------
    // ToolExposure Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_tool_exposure_below_threshold() {
        let tools: Vec<ToolInfo> = (0..50)
            .map(|i| ToolInfo::new("server", &format!("tool-{}", i), None))
            .collect();

        let exposure = ToolExposure::calculate(tools);

        assert!(!exposure.deferred_loading_active);
        assert_eq!(exposure.direct_tools.len(), 50);
        assert_eq!(exposure.deferred_tools.len(), 0);
        assert!(exposure.search_tool_definition().is_none());
    }

    #[test]
    fn test_tool_exposure_above_threshold() {
        let tools: Vec<ToolInfo> = (0..150)
            .map(|i| ToolInfo::new("server", &format!("tool-{}", i), None))
            .collect();

        let exposure = ToolExposure::calculate(tools);

        assert!(exposure.deferred_loading_active);
        assert_eq!(exposure.direct_tools.len(), DEFERRED_LOADING_THRESHOLD);
        assert_eq!(exposure.deferred_tools.len(), 50);
        assert_eq!(exposure.total_count(), 150);
        assert!(exposure.search_tool_definition().is_some());
    }

    #[test]
    fn test_tool_exposure_exactly_at_threshold() {
        let tools: Vec<ToolInfo> = (0..DEFERRED_LOADING_THRESHOLD)
            .map(|i| ToolInfo::new("server", &format!("tool-{}", i), None))
            .collect();

        let exposure = ToolExposure::calculate(tools);

        // Exactly at threshold = no deferred loading
        assert!(!exposure.deferred_loading_active);
        assert_eq!(exposure.direct_tools.len(), DEFERRED_LOADING_THRESHOLD);
    }

    #[test]
    fn test_tool_exposure_search_deferred() {
        let tools: Vec<ToolInfo> = vec![
            ToolInfo::new("server", "read_file", Some("Read a file".to_string())),
            ToolInfo::new("server", "write_file", Some("Write to a file".to_string())),
            ToolInfo::new("server", "delete_file", Some("Delete a file".to_string())),
        ];

        // Force deferred loading
        let mut exposure = ToolExposure::calculate(vec![]);
        exposure.deferred_loading_active = true;
        exposure.deferred_tools = tools;

        // Search for "file"
        let results = exposure.search_deferred("file");
        assert_eq!(results.len(), 3);

        // Search for "read"
        let results = exposure.search_deferred("read");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].tool_name, "read_file");

        // Search for "delete" (in description)
        let results = exposure.search_deferred("delete");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].tool_name, "delete_file");

        // Search for non-existent
        let results = exposure.search_deferred("nonexistent");
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_tool_exposure_search_case_insensitive() {
        let tools = vec![ToolInfo::new("server", "READ_File", None)];

        let mut exposure = ToolExposure::calculate(vec![]);
        exposure.deferred_loading_active = true;
        exposure.deferred_tools = tools;

        let results = exposure.search_deferred("read");
        assert_eq!(results.len(), 1);

        let results = exposure.search_deferred("FILE");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_tool_exposure_with_priority_servers() {
        let tools: Vec<ToolInfo> = (0..150)
            .map(|i| {
                let server = if i < 30 { "priority" } else { "normal" };
                ToolInfo::new(server, &format!("tool-{}", i), None)
            })
            .collect();

        let exposure = ToolExposure::calculate_with_priority(tools, &["priority".to_string()]);

        assert!(exposure.deferred_loading_active);

        // Check that priority server tools are in direct_tools
        let priority_in_direct = exposure
            .direct_tools
            .iter()
            .filter(|t| t.server_name == "priority")
            .count();
        assert_eq!(priority_in_direct, 30); // All priority tools should be direct
    }

    #[test]
    fn test_tool_exposure_search_tool_definition() {
        let mut exposure = ToolExposure::calculate(vec![]);
        exposure.deferred_loading_active = true;
        exposure.deferred_tools = vec![ToolInfo::new("server", "tool", None)];

        let def = exposure.search_tool_definition().unwrap();

        assert_eq!(def["name"], "mcp__search_tools");
        assert!(def["description"].as_str().unwrap().contains("1 tools"));
    }

    // -------------------------------------------------------------------------
    // ToolInfo Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_tool_info_prefixed_name() {
        let info = ToolInfo::new("my-server", "my-tool", Some("Description".to_string()));

        assert_eq!(info.server_name, "my-server");
        assert_eq!(info.tool_name, "my-tool");
        assert_eq!(info.prefixed_name, "my-server__my-tool");
        assert_eq!(info.description, Some("Description".to_string()));
    }

    #[test]
    fn test_tool_info_no_description() {
        let info = ToolInfo::new("server", "tool", None);

        assert_eq!(info.server_name, "server");
        assert_eq!(info.tool_name, "tool");
        assert_eq!(info.prefixed_name, "server__tool");
        assert_eq!(info.description, None);
    }

    // -------------------------------------------------------------------------
    // Integration Tests
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn test_full_approval_flow_approved() {
        let engine = ApprovalEngine::auto();

        let result = engine
            .approve_tool_call("server", "tool", &serde_json::json!({}), None)
            .await;

        assert!(result.is_ok());
        assert!(result.unwrap());
    }

    #[tokio::test]
    async fn test_full_approval_flow_no_callback() {
        let engine = ApprovalEngine::prompt();

        // No callback set, so approval request will fail
        let result = engine
            .approve_tool_call("server", "tool", &serde_json::json!({}), None)
            .await;

        assert!(result.is_ok());
        assert!(!result.unwrap()); // User declined (no callback)
    }

    #[tokio::test]
    async fn test_full_approval_flow_with_callback() {
        let mut engine = ApprovalEngine::prompt();
        engine.set_callback(Arc::new(|_req| Box::pin(async { true })));

        let result = engine
            .approve_tool_call("server", "tool", &serde_json::json!({}), None)
            .await;

        assert!(result.is_ok());
        assert!(result.unwrap());
    }

    #[tokio::test]
    async fn test_clear_session_approvals() {
        let engine = ApprovalEngine::prompt();

        engine.record_approval("server", "tool").await;

        // Should be approved
        {
            let approvals = engine.session_approvals.read().await;
            assert!(approvals.contains("server::tool"));
        }

        engine.clear_session_approvals().await;

        // Should be cleared
        {
            let approvals = engine.session_approvals.read().await;
            assert!(approvals.is_empty());
        }
    }
}
