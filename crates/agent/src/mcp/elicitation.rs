//! MCP Elicitation protocol handling.
//!
//! Implements the Elicitation flow where an MCP server requests user input
//! during tool execution. This module provides:
//! - **ElicitationPolicy**: Determines how elicitation requests are handled
//!   (auto-decline for non-interactive, or forward to callback).
//! - **ElicitationHandler**: Processes incoming elicitation requests according
//!   to the configured policy.
//!
//! Design inspired by Codex's `elicitation.rs` and `elicitation_client_service.rs`.

use serde_json::{Map, Value};
use tokio::sync::oneshot;
use tracing::{debug, warn};

use super::types::{ElicitationAction, ElicitationRequest, ElicitationResponse};

// ---------------------------------------------------------------------------
// ElicitationPolicy
// ---------------------------------------------------------------------------

/// Policy for handling elicitation requests from MCP servers.
///
/// Determines whether elicitation requests are automatically declined (suitable
/// for non-interactive / batch modes) or forwarded to a user-facing callback.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ElicitationPolicy {
    /// Auto-decline all elicitation requests without user interaction.
    /// Suitable for non-interactive / CI / batch processing modes.
    #[default]
    AutoDecline,

    /// Forward elicitation requests to a callback for user decision.
    /// Used when a human operator is available to respond.
    Interactive,
}

impl ElicitationPolicy {
    /// Derive policy from an approval mode string.
    ///
    /// - `"auto"` or `"never"` → `AutoDecline`
    /// - Anything else → `Interactive`
    pub fn from_approval_mode(mode: &str) -> Self {
        match mode {
            "auto" | "never" => ElicitationPolicy::AutoDecline,
            _ => ElicitationPolicy::Interactive,
        }
    }
}

// ---------------------------------------------------------------------------
// ElicitationCallback
// ---------------------------------------------------------------------------

/// Callback type for elicitation requests.
///
/// When invoked with an `ElicitationRequest`, it returns a oneshot receiver
/// that will eventually produce the user's `ElicitationResponse`.
pub type ElicitationCallback =
    Box<dyn Fn(ElicitationRequest) -> oneshot::Receiver<ElicitationResponse> + Send + Sync>;

// ---------------------------------------------------------------------------
// ElicitationHandler
// ---------------------------------------------------------------------------

/// Handles elicitation requests from MCP servers.
///
/// Combines a policy with an optional callback to determine how each
/// elicitation request is processed.
pub struct ElicitationHandler {
    policy: ElicitationPolicy,
    callback: Option<ElicitationCallback>,
}

impl ElicitationHandler {
    /// Create a new handler with the given policy and no callback.
    pub fn new(policy: ElicitationPolicy) -> Self {
        Self {
            policy,
            callback: None,
        }
    }

    /// Create a new handler with the given policy and callback.
    pub fn with_callback(policy: ElicitationPolicy, callback: ElicitationCallback) -> Self {
        Self {
            policy,
            callback: Some(callback),
        }
    }

    /// Get the current policy.
    pub fn policy(&self) -> &ElicitationPolicy {
        &self.policy
    }

    /// Handle an incoming elicitation request, returning a response.
    ///
    /// Behavior depends on the configured policy:
    /// - `AutoDecline`: Immediately returns a Decline response.
    /// - `Interactive`: Forwards to callback if available; otherwise declines.
    pub async fn handle(&self, request: ElicitationRequest) -> ElicitationResponse {
        match &self.policy {
            ElicitationPolicy::AutoDecline => {
                debug!(
                    message = %request.message(),
                    "Auto-declining elicitation request (policy=AutoDecline)"
                );
                ElicitationResponse {
                    action: ElicitationAction::Decline,
                    content: None,
                    meta: None,
                }
            }
            ElicitationPolicy::Interactive => {
                if let Some(callback) = &self.callback {
                    debug!(
                        message = %request.message(),
                        "Forwarding elicitation to interactive callback"
                    );
                    let rx = callback(request);
                    match rx.await {
                        Ok(response) => response,
                        Err(_) => {
                            warn!("Elicitation callback channel closed; returning Cancel");
                            ElicitationResponse {
                                action: ElicitationAction::Cancel,
                                content: None,
                                meta: None,
                            }
                        }
                    }
                } else {
                    debug!(
                        message = %request.message(),
                        "No callback set for Interactive policy; declining"
                    );
                    ElicitationResponse {
                        action: ElicitationAction::Decline,
                        content: None,
                        meta: None,
                    }
                }
            }
        }
    }
}

impl std::fmt::Debug for ElicitationHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ElicitationHandler")
            .field("policy", &self.policy)
            .field("has_callback", &self.callback.is_some())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create a simple auto-decline response for a given request message.
/// Utility for tests and non-interactive environments.
pub fn auto_decline_response() -> ElicitationResponse {
    ElicitationResponse {
        action: ElicitationAction::Decline,
        content: None,
        meta: None,
    }
}

/// Create an accept response with optional content.
pub fn accept_response(content: Option<Value>) -> ElicitationResponse {
    ElicitationResponse {
        action: ElicitationAction::Accept,
        content,
        meta: None,
    }
}

/// Restore context metadata into an elicitation request.
///
/// When processing JSON-RPC requests, the `_meta` field may be lifted out
/// of the request into a separate context. This function merges it back,
/// excluding the `progressToken` key which is handled separately.
///
/// Mirrors Codex's `restore_context_meta` in `elicitation_client_service.rs`.
pub fn restore_context_meta(
    request: &mut ElicitationRequest,
    mut context_meta: Map<String, Value>,
) {
    // Remove progressToken — it's handled by the transport layer
    context_meta.remove("progressToken");
    if context_meta.is_empty() {
        return;
    }

    let meta_value = Value::Object(context_meta);
    match request {
        ElicitationRequest::Form { ref mut meta, .. }
        | ElicitationRequest::Url { ref mut meta, .. } => match meta {
            Some(Value::Object(existing)) => {
                if let Value::Object(new_map) = meta_value {
                    existing.extend(new_map);
                }
            }
            _ => {
                *meta = Some(meta_value);
            }
        },
    }
}

// ---------------------------------------------------------------------------
// Unit Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -------------------------------------------------------------------------
    // ElicitationPolicy tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_policy_from_approval_mode_auto() {
        assert_eq!(
            ElicitationPolicy::from_approval_mode("auto"),
            ElicitationPolicy::AutoDecline
        );
    }

    #[test]
    fn test_policy_from_approval_mode_never() {
        assert_eq!(
            ElicitationPolicy::from_approval_mode("never"),
            ElicitationPolicy::AutoDecline
        );
    }

    #[test]
    fn test_policy_from_approval_mode_prompt() {
        assert_eq!(
            ElicitationPolicy::from_approval_mode("prompt"),
            ElicitationPolicy::Interactive
        );
    }

    #[test]
    fn test_policy_from_approval_mode_other() {
        assert_eq!(
            ElicitationPolicy::from_approval_mode("interactive"),
            ElicitationPolicy::Interactive
        );
        assert_eq!(
            ElicitationPolicy::from_approval_mode(""),
            ElicitationPolicy::Interactive
        );
    }

    #[test]
    fn test_policy_default_is_auto_decline() {
        assert_eq!(ElicitationPolicy::default(), ElicitationPolicy::AutoDecline);
    }

    // -------------------------------------------------------------------------
    // ElicitationHandler tests
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn test_handler_auto_decline_returns_decline() {
        let handler = ElicitationHandler::new(ElicitationPolicy::AutoDecline);
        let request = ElicitationRequest::Form {
            meta: None,
            message: "Please confirm action".to_string(),
            requested_schema: None,
        };

        let response = handler.handle(request).await;
        assert_eq!(response.action, ElicitationAction::Decline);
        assert!(response.content.is_none());
    }

    #[tokio::test]
    async fn test_handler_interactive_no_callback_declines() {
        let handler = ElicitationHandler::new(ElicitationPolicy::Interactive);
        let request = ElicitationRequest::Form {
            meta: None,
            message: "Confirm?".to_string(),
            requested_schema: None,
        };

        let response = handler.handle(request).await;
        assert_eq!(response.action, ElicitationAction::Decline);
    }

    #[tokio::test]
    async fn test_handler_interactive_with_callback_accept() {
        let callback: ElicitationCallback = Box::new(|_req| {
            let (tx, rx) = oneshot::channel();
            tx.send(ElicitationResponse {
                action: ElicitationAction::Accept,
                content: Some(json!({"confirmed": true})),
                meta: None,
            })
            .unwrap();
            rx
        });

        let handler = ElicitationHandler::with_callback(ElicitationPolicy::Interactive, callback);
        let request = ElicitationRequest::Form {
            meta: None,
            message: "Approve operation?".to_string(),
            requested_schema: Some(json!({"type": "object"})),
        };

        let response = handler.handle(request).await;
        assert_eq!(response.action, ElicitationAction::Accept);
        assert_eq!(response.content, Some(json!({"confirmed": true})));
    }

    #[tokio::test]
    async fn test_handler_interactive_callback_dropped_returns_cancel() {
        let callback: ElicitationCallback = Box::new(|_req| {
            let (tx, rx) = oneshot::channel::<ElicitationResponse>();
            // Drop sender without sending → receiver gets RecvError
            drop(tx);
            rx
        });

        let handler = ElicitationHandler::with_callback(ElicitationPolicy::Interactive, callback);
        let request = ElicitationRequest::Form {
            meta: None,
            message: "Will fail".to_string(),
            requested_schema: None,
        };

        let response = handler.handle(request).await;
        assert_eq!(response.action, ElicitationAction::Cancel);
        assert!(response.content.is_none());
    }

    // -------------------------------------------------------------------------
    // Helper function tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_auto_decline_response() {
        let resp = auto_decline_response();
        assert_eq!(resp.action, ElicitationAction::Decline);
        assert!(resp.content.is_none());
    }

    #[test]
    fn test_accept_response_with_content() {
        let content = json!({"answer": 42});
        let resp = accept_response(Some(content.clone()));
        assert_eq!(resp.action, ElicitationAction::Accept);
        assert_eq!(resp.content, Some(content));
    }

    #[test]
    fn test_accept_response_without_content() {
        let resp = accept_response(None);
        assert_eq!(resp.action, ElicitationAction::Accept);
        assert!(resp.content.is_none());
    }

    // -------------------------------------------------------------------------
    // Serialization round-trip tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_elicitation_request_serialization_roundtrip() {
        let request = ElicitationRequest::Form {
            meta: None,
            message: "Confirm deployment?".to_string(),
            requested_schema: Some(
                json!({"type": "object", "properties": {"confirmed": {"type": "boolean"}}}),
            ),
        };

        let serialized = serde_json::to_string(&request).unwrap();
        let deserialized: ElicitationRequest = serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized.message(), request.message());
        assert_eq!(deserialized, request);
    }

    #[test]
    fn test_elicitation_response_serialization_roundtrip() {
        let response = ElicitationResponse {
            action: ElicitationAction::Accept,
            content: Some(json!({"name": "test", "value": 123})),
            meta: None,
        };

        let serialized = serde_json::to_string(&response).unwrap();
        let deserialized: ElicitationResponse = serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized.action, ElicitationAction::Accept);
        assert_eq!(deserialized.content, response.content);
    }

    #[test]
    fn test_elicitation_action_all_variants_serialize() {
        for (action, expected) in [
            (ElicitationAction::Accept, "\"accept\""),
            (ElicitationAction::Decline, "\"decline\""),
            (ElicitationAction::Cancel, "\"cancel\""),
        ] {
            let serialized = serde_json::to_string(&action).unwrap();
            assert_eq!(serialized, expected);
        }
    }

    // -------------------------------------------------------------------------
    // restore_context_meta tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_restore_context_meta_merges_into_form() {
        let mut request = ElicitationRequest::Form {
            meta: None,
            message: "test".to_string(),
            requested_schema: None,
        };
        let mut context_meta = Map::new();
        context_meta.insert("key".to_string(), json!("value"));
        context_meta.insert("progressToken".to_string(), json!("tok-123"));

        restore_context_meta(&mut request, context_meta);

        match &request {
            ElicitationRequest::Form { meta, .. } => {
                let meta = meta.as_ref().unwrap();
                assert_eq!(meta.get("key"), Some(&json!("value")));
                // progressToken should be removed
                assert_eq!(meta.get("progressToken"), None);
            }
            _ => panic!("expected Form variant"),
        }
    }

    #[test]
    fn test_restore_context_meta_empty_is_noop() {
        let mut request = ElicitationRequest::Form {
            meta: None,
            message: "test".to_string(),
            requested_schema: None,
        };
        let mut context_meta = Map::new();
        context_meta.insert("progressToken".to_string(), json!("tok"));

        restore_context_meta(&mut request, context_meta);

        match &request {
            ElicitationRequest::Form { meta, .. } => {
                assert!(meta.is_none());
            }
            _ => panic!("expected Form variant"),
        }
    }

    #[test]
    fn test_restore_context_meta_merges_into_existing_meta() {
        let mut request = ElicitationRequest::Form {
            meta: Some(json!({"existing": "data"})),
            message: "test".to_string(),
            requested_schema: None,
        };
        let mut context_meta = Map::new();
        context_meta.insert("new_key".to_string(), json!("new_value"));

        restore_context_meta(&mut request, context_meta);

        match &request {
            ElicitationRequest::Form { meta, .. } => {
                let meta = meta.as_ref().unwrap();
                assert_eq!(meta.get("existing"), Some(&json!("data")));
                assert_eq!(meta.get("new_key"), Some(&json!("new_value")));
            }
            _ => panic!("expected Form variant"),
        }
    }

    #[test]
    fn test_restore_context_meta_url_variant() {
        let mut request = ElicitationRequest::Url {
            meta: None,
            message: "Visit URL".to_string(),
            url: "https://example.com/oauth".to_string(),
            elicitation_id: "elicit-001".to_string(),
        };
        let mut context_meta = Map::new();
        context_meta.insert("trace_id".to_string(), json!("abc123"));

        restore_context_meta(&mut request, context_meta);

        match &request {
            ElicitationRequest::Url { meta, .. } => {
                let meta = meta.as_ref().unwrap();
                assert_eq!(meta.get("trace_id"), Some(&json!("abc123")));
            }
            _ => panic!("expected Url variant"),
        }
    }
}
