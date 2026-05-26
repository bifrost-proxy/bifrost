//! Memory pollution detection mechanism.
//!
//! Tracks whether a session has been "polluted" by external context (MCP tool
//! results, web search results, untrusted third-party data). Polluted sessions
//! should not write their content into long-term memory to avoid contaminating
//! the memory store with potentially unreliable information.
//!
//! Aligned with Codex's thread memory mode concept.

use std::sync::{Arc, RwLock};
use tracing::debug;

// ─────────────────────────────────────────────────────────────────────────────
// Thread Memory Mode
// ─────────────────────────────────────────────────────────────────────────────

/// Represents the memory generation eligibility of a thread/session.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ThreadMemoryMode {
    /// Memory generation is enabled — session content may be written to
    /// long-term memory after extraction.
    #[default]
    Enabled,
    /// Memory generation is explicitly disabled for this thread (e.g., by
    /// config or ephemeral session flag).
    Disabled,
    /// The session has been polluted by external/untrusted context.
    /// Content from this session will NOT be written to long-term memory.
    Polluted { reason: String },
}

impl ThreadMemoryMode {
    /// Returns `true` if memory generation should proceed.
    pub fn allows_memory_write(&self) -> bool {
        matches!(self, Self::Enabled)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Pollution Sources
// ─────────────────────────────────────────────────────────────────────────────

/// Categories of external context that can pollute a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollutionSource {
    /// MCP tool call result (Chrome DevTools, external APIs, etc.)
    McpToolResult,
    /// Web search or fetch result
    WebContent,
    /// Untrusted third-party plugin output
    UntrustedPlugin,
    /// External API response injected into context
    ExternalApi,
    /// User explicitly injected unverified content
    #[allow(dead_code)]
    UserInjectedExternal,
}

impl std::fmt::Display for PollutionSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::McpToolResult => write!(f, "mcp_tool_result"),
            Self::WebContent => write!(f, "web_content"),
            Self::UntrustedPlugin => write!(f, "untrusted_plugin"),
            Self::ExternalApi => write!(f, "external_api"),
            Self::UserInjectedExternal => write!(f, "user_injected_external"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Pollution Detector
// ─────────────────────────────────────────────────────────────────────────────

/// Source markers that indicate external/untrusted context injection.
const POLLUTION_MARKERS: &[(&str, PollutionSource)] = &[
    ("mcp_", PollutionSource::McpToolResult),
    ("mcp://", PollutionSource::McpToolResult),
    ("<invoke", PollutionSource::McpToolResult),
    ("WebSearch", PollutionSource::WebContent),
    ("WebFetch", PollutionSource::WebContent),
    ("web_search", PollutionSource::WebContent),
    ("external_api", PollutionSource::ExternalApi),
    ("plugin_output", PollutionSource::UntrustedPlugin),
];

/// Thread-safe pollution detector that manages memory mode transitions.
///
/// Once a session is marked as polluted, it stays polluted for the remainder
/// of the session lifetime. This is a one-way transition.
#[derive(Debug, Clone)]
pub struct PollutionDetector {
    mode: Arc<RwLock<ThreadMemoryMode>>,
}

impl Default for PollutionDetector {
    fn default() -> Self {
        Self::new(ThreadMemoryMode::Enabled)
    }
}

impl PollutionDetector {
    /// Create a new detector with the given initial mode.
    pub fn new(initial_mode: ThreadMemoryMode) -> Self {
        Self {
            mode: Arc::new(RwLock::new(initial_mode)),
        }
    }

    /// Create a detector that starts in disabled mode.
    pub fn disabled() -> Self {
        Self::new(ThreadMemoryMode::Disabled)
    }

    /// Check if external context indicators are present in `source` text and
    /// mark the session as polluted if so.
    ///
    /// Returns `true` if pollution was newly detected (transition from
    /// Enabled → Polluted), `false` otherwise (already polluted, disabled,
    /// or no pollution found).
    pub fn mark_polluted_if_external_context(&self, source: &str) -> bool {
        // Fast path: if already polluted or disabled, no-op.
        {
            let mode = self.mode.read().unwrap_or_else(|e| e.into_inner());
            if !matches!(*mode, ThreadMemoryMode::Enabled) {
                return false;
            }
        }

        let source_lower = source.to_lowercase();
        for (marker, pollution_source) in POLLUTION_MARKERS {
            if source_lower.contains(&marker.to_lowercase()) {
                let reason = format!(
                    "external context detected: {} (marker: {})",
                    pollution_source, marker
                );
                debug!(
                    pollution_source = %pollution_source,
                    marker = marker,
                    "marking session as polluted"
                );
                let mut mode = self.mode.write().unwrap_or_else(|e| e.into_inner());
                // Double-check: might have been set by a concurrent writer.
                if matches!(*mode, ThreadMemoryMode::Enabled) {
                    *mode = ThreadMemoryMode::Polluted { reason };
                    return true;
                }
                return false;
            }
        }

        false
    }

    /// Explicitly mark the session as polluted with a given reason.
    pub fn mark_polluted(&self, reason: String) {
        let mut mode = self.mode.write().unwrap_or_else(|e| e.into_inner());
        if matches!(*mode, ThreadMemoryMode::Enabled) {
            debug!(reason = %reason, "explicitly marking session as polluted");
            *mode = ThreadMemoryMode::Polluted { reason };
        }
    }

    /// Returns `true` if the session is currently polluted.
    pub fn is_polluted(&self) -> bool {
        let mode = self.mode.read().unwrap_or_else(|e| e.into_inner());
        matches!(*mode, ThreadMemoryMode::Polluted { .. })
    }

    /// Returns `true` if memory writes are allowed (mode is Enabled).
    pub fn allows_memory_write(&self) -> bool {
        let mode = self.mode.read().unwrap_or_else(|e| e.into_inner());
        mode.allows_memory_write()
    }

    /// Get the current mode snapshot.
    pub fn current_mode(&self) -> ThreadMemoryMode {
        let mode = self.mode.read().unwrap_or_else(|e| e.into_inner());
        mode.clone()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_mode_is_enabled() {
        let detector = PollutionDetector::default();
        assert_eq!(detector.current_mode(), ThreadMemoryMode::Enabled);
        assert!(detector.allows_memory_write());
        assert!(!detector.is_polluted());
    }

    #[test]
    fn detects_mcp_pollution() {
        let detector = PollutionDetector::default();
        let result = detector
            .mark_polluted_if_external_context("Called mcp_Chrome_DevTools_MCP_click on element");
        assert!(result);
        assert!(detector.is_polluted());
        assert!(!detector.allows_memory_write());
        match detector.current_mode() {
            ThreadMemoryMode::Polluted { reason } => {
                assert!(reason.contains("mcp_tool_result"));
            }
            _ => panic!("expected Polluted mode"),
        }
    }

    #[test]
    fn detects_web_search_pollution() {
        let detector = PollutionDetector::default();
        let result =
            detector.mark_polluted_if_external_context("Results from WebSearch: latest docs");
        assert!(result);
        assert!(detector.is_polluted());
    }

    #[test]
    fn detects_web_fetch_pollution() {
        let detector = PollutionDetector::default();
        let result = detector.mark_polluted_if_external_context("WebFetch returned HTML content");
        assert!(result);
        assert!(detector.is_polluted());
    }

    #[test]
    fn clean_content_does_not_pollute() {
        let detector = PollutionDetector::default();
        let result = detector
            .mark_polluted_if_external_context("How do I implement a binary search in Rust?");
        assert!(!result);
        assert!(!detector.is_polluted());
        assert!(detector.allows_memory_write());
    }

    #[test]
    fn pollution_is_one_way_transition() {
        let detector = PollutionDetector::default();
        detector.mark_polluted_if_external_context("mcp_tool invoked");
        assert!(detector.is_polluted());

        // Second detection returns false (already polluted).
        let second = detector.mark_polluted_if_external_context("WebSearch results");
        assert!(!second);
        assert!(detector.is_polluted());
    }

    #[test]
    fn disabled_mode_prevents_pollution_transition() {
        let detector = PollutionDetector::disabled();
        assert!(!detector.allows_memory_write());
        assert!(!detector.is_polluted());

        let result = detector.mark_polluted_if_external_context("mcp_tool_call");
        assert!(!result);
        // Mode stays Disabled, does not become Polluted.
        assert_eq!(detector.current_mode(), ThreadMemoryMode::Disabled);
    }

    #[test]
    fn explicit_mark_polluted() {
        let detector = PollutionDetector::default();
        detector.mark_polluted("user injected unverified content".to_string());
        assert!(detector.is_polluted());
        assert!(!detector.allows_memory_write());
    }

    #[test]
    fn thread_memory_mode_allows_write() {
        assert!(ThreadMemoryMode::Enabled.allows_memory_write());
        assert!(!ThreadMemoryMode::Disabled.allows_memory_write());
        assert!(!ThreadMemoryMode::Polluted {
            reason: "test".to_string()
        }
        .allows_memory_write());
    }
}
