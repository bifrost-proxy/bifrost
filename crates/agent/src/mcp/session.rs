//! MCP Session Recovery and Transport Enhancement.
//!
//! This module provides session management capabilities for MCP HTTP transport,
//! including:
//! - Session state management with recovery support
//! - HTTP session ID header management (Mcp-Session-Id)
//! - HTTP response error classification
//! - Active timeout tracking with pause support
//!
//! Based on Codex's rmcp-client implementation.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, Semaphore};

// ---------------------------------------------------------------------------
// Session State
// ---------------------------------------------------------------------------

/// MCP connection session state.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionState {
    /// Connecting to server.
    Connecting,
    /// Session established and ready.
    Ready {
        session_id: Option<String>,
        server_info: Option<ServerInfo>,
    },
    /// Session needs recovery (e.g., after HTTP 404).
    NeedsRecovery { reason: String },
    /// Session permanently closed.
    Closed { reason: String },
}

/// MCP server info from initialization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: Option<String>,
    pub version: Option<String>,
    pub protocol_version: Option<String>,
    #[serde(default)]
    pub capabilities: ServerCapabilities,
}

/// Server capabilities declared during initialization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ServerCapabilities {
    #[serde(default)]
    pub tools: Option<ToolsCapability>,
    #[serde(default)]
    pub resources: Option<ResourcesCapability>,
    #[serde(default)]
    pub elicitation: Option<ElicitationCapability>,
}

/// Tools capability.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolsCapability {
    #[serde(rename = "listChanged", default)]
    pub list_changed: bool,
}

/// Resources capability.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourcesCapability {
    #[serde(rename = "subscribe", default)]
    pub subscribe: bool,
    #[serde(rename = "listChanged", default)]
    pub list_changed: bool,
}

/// Elicitation capability.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ElicitationCapability {}

// ---------------------------------------------------------------------------
// Session Manager
// ---------------------------------------------------------------------------

/// Manages MCP session lifecycle including recovery.
pub struct SessionManager {
    state: Arc<RwLock<SessionState>>,
    /// Lock to prevent concurrent recovery attempts.
    recovery_lock: Arc<Semaphore>,
    /// Counter for recovery attempts.
    recovery_attempts: Arc<AtomicU32>,
    /// Maximum recovery attempts before giving up.
    max_recovery_attempts: u32,
}

impl SessionManager {
    /// Create a new session manager with the specified max recovery attempts.
    pub fn new(max_recovery_attempts: u32) -> Self {
        Self {
            state: Arc::new(RwLock::new(SessionState::Connecting)),
            recovery_lock: Arc::new(Semaphore::new(1)),
            recovery_attempts: Arc::new(AtomicU32::new(0)),
            max_recovery_attempts,
        }
    }

    /// Get current session state.
    pub async fn state(&self) -> SessionState {
        self.state.read().await.clone()
    }

    /// Get current session ID.
    pub async fn session_id(&self) -> Option<String> {
        match &*self.state.read().await {
            SessionState::Ready { session_id, .. } => session_id.clone(),
            _ => None,
        }
    }

    /// Transition to Ready state.
    pub async fn set_ready(&self, session_id: Option<String>, server_info: Option<ServerInfo>) {
        let mut state = self.state.write().await;
        *state = SessionState::Ready {
            session_id,
            server_info,
        };
        self.recovery_attempts.store(0, Ordering::SeqCst);
        let sid = match &*state {
            SessionState::Ready { session_id, .. } => session_id.as_deref(),
            _ => None,
        };
        tracing::info!("MCP session ready, session_id: {:?}", sid);
    }

    /// Handle HTTP 404 - session expired, trigger recovery.
    /// Returns false if max attempts exceeded.
    pub async fn handle_session_expired(&self) -> bool {
        let attempts = self.recovery_attempts.fetch_add(1, Ordering::SeqCst);
        if attempts >= self.max_recovery_attempts {
            let mut state = self.state.write().await;
            *state = SessionState::Closed {
                reason: format!(
                    "Max recovery attempts ({}) exceeded",
                    self.max_recovery_attempts
                ),
            };
            tracing::error!("Session recovery failed after {} attempts", attempts);
            return false;
        }

        let mut state = self.state.write().await;
        *state = SessionState::NeedsRecovery {
            reason: "HTTP 404 - session expired".to_string(),
        };
        tracing::warn!(
            "Session expired (attempt {}/{}), triggering recovery",
            attempts + 1,
            self.max_recovery_attempts
        );
        true
    }

    /// Try to perform session recovery.
    /// Returns true if recovery should be attempted.
    pub async fn try_recovery(&self) -> bool {
        // Use semaphore to prevent concurrent recovery
        let permit = match self.recovery_lock.clone().try_acquire_owned() {
            Ok(p) => p,
            Err(_) => {
                tracing::debug!("Recovery already in progress, waiting...");
                // Wait for ongoing recovery
                let _p = self.recovery_lock.clone().acquire_owned().await.unwrap();
                // Check if recovery succeeded
                return matches!(&*self.state.read().await, SessionState::Ready { .. });
            }
        };

        tracing::info!("Starting session recovery...");
        // Recovery logic will be performed by the caller
        // This just manages the lock
        drop(permit);
        true
    }

    /// Reset recovery attempts (after successful operation).
    pub fn reset_recovery_attempts(&self) {
        self.recovery_attempts.store(0, Ordering::SeqCst);
    }

    /// Close the session.
    pub async fn close(&self, reason: &str) {
        let mut state = self.state.write().await;
        *state = SessionState::Closed {
            reason: reason.to_string(),
        };
        tracing::info!("MCP session closed: {}", reason);
    }

    /// Check if the session is ready.
    pub async fn is_ready(&self) -> bool {
        matches!(&*self.state.read().await, SessionState::Ready { .. })
    }

    /// Check if the session is closed.
    pub async fn is_closed(&self) -> bool {
        matches!(&*self.state.read().await, SessionState::Closed { .. })
    }

    /// Check if the session needs recovery.
    pub async fn needs_recovery(&self) -> bool {
        matches!(
            &*self.state.read().await,
            SessionState::NeedsRecovery { .. }
        )
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new(3)
    }
}

// ---------------------------------------------------------------------------
// HTTP Session ID Manager
// ---------------------------------------------------------------------------

/// Manages MCP-Session-Id header for HTTP transport.
#[derive(Debug, Clone)]
pub struct HttpSessionIdManager {
    session_id: Arc<RwLock<Option<String>>>,
    last_event_id: Arc<RwLock<Option<String>>>,
}

impl HttpSessionIdManager {
    /// Create a new session ID manager.
    pub fn new() -> Self {
        Self {
            session_id: Arc::new(RwLock::new(None)),
            last_event_id: Arc::new(RwLock::new(None)),
        }
    }

    /// Update session ID from response header.
    pub async fn update_from_response(&self, headers: &reqwest::header::HeaderMap) {
        if let Some(id) = headers.get("mcp-session-id") {
            if let Ok(id_str) = id.to_str() {
                let mut session_id = self.session_id.write().await;
                let new_id = id_str.to_string();
                if session_id.as_deref() != Some(&new_id) {
                    tracing::debug!("Updated MCP session ID: {}", new_id);
                }
                *session_id = Some(new_id);
            }
        }
    }

    /// Get current session ID for request header.
    pub async fn get_session_id(&self) -> Option<String> {
        self.session_id.read().await.clone()
    }

    /// Update last event ID from SSE stream.
    pub async fn update_last_event_id(&self, event_id: &str) {
        let mut id = self.last_event_id.write().await;
        *id = Some(event_id.to_string());
    }

    /// Get last event ID for SSE reconnection.
    pub async fn get_last_event_id(&self) -> Option<String> {
        self.last_event_id.read().await.clone()
    }

    /// Clear all session state (for recovery).
    pub async fn clear(&self) {
        *self.session_id.write().await = None;
        *self.last_event_id.write().await = None;
    }

    /// Build headers for an HTTP request.
    pub async fn build_request_headers(&self) -> Vec<(String, String)> {
        let mut headers = Vec::new();
        if let Some(id) = self.get_session_id().await {
            headers.push(("Mcp-Session-Id".to_string(), id));
        }
        if let Some(id) = self.get_last_event_id().await {
            headers.push(("Last-Event-Id".to_string(), id));
        }
        headers
    }
}

impl Default for HttpSessionIdManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// HTTP Response Error Classification
// ---------------------------------------------------------------------------

/// HTTP response error classification.
#[derive(Debug, Clone, PartialEq)]
pub enum HttpResponseError {
    /// Session expired (HTTP 404 with session ID) - recoverable.
    SessionExpired,
    /// Authentication required (HTTP 401) - may need OAuth refresh.
    AuthRequired,
    /// Server error (5xx) - may be transient.
    ServerError(u16),
    /// Client error (4xx other than 401/404) - not recoverable.
    ClientError(u16),
    /// Network error.
    NetworkError(String),
}

impl std::fmt::Display for HttpResponseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionExpired => write!(f, "Session expired (HTTP 404)"),
            Self::AuthRequired => write!(f, "Authentication required (HTTP 401)"),
            Self::ServerError(code) => write!(f, "Server error (HTTP {})", code),
            Self::ClientError(code) => write!(f, "Client error (HTTP {})", code),
            Self::NetworkError(msg) => write!(f, "Network error: {}", msg),
        }
    }
}

/// Classify an HTTP response for error handling.
pub fn classify_http_error(status: u16, has_session_id: bool) -> Option<HttpResponseError> {
    match status {
        200..=299 => None, // Success
        401 => Some(HttpResponseError::AuthRequired),
        404 if has_session_id => Some(HttpResponseError::SessionExpired),
        400..=499 => Some(HttpResponseError::ClientError(status)),
        500..=599 => Some(HttpResponseError::ServerError(status)),
        _ => Some(HttpResponseError::ClientError(status)),
    }
}

/// Check if an HTTP error is recoverable.
pub fn is_recoverable_error(error: &HttpResponseError) -> bool {
    matches!(
        error,
        HttpResponseError::SessionExpired
            | HttpResponseError::AuthRequired
            | HttpResponseError::ServerError(_)
    )
}

// ---------------------------------------------------------------------------
// HTTP Headers Configuration
// ---------------------------------------------------------------------------

/// Custom HTTP headers configuration.
#[derive(Debug, Clone, Default)]
pub struct HttpHeadersConfig {
    /// Static headers.
    pub headers: Vec<(String, String)>,
    /// Headers from environment variables.
    pub env_headers: Vec<(String, String)>,
}

impl HttpHeadersConfig {
    /// Build from config maps.
    pub fn from_config(
        http_headers: Option<&HashMap<String, String>>,
        env_http_headers: Option<&HashMap<String, String>>,
    ) -> Self {
        let mut config = HttpHeadersConfig::default();

        if let Some(headers) = http_headers {
            for (key, value) in headers {
                config.headers.push((key.clone(), value.clone()));
            }
        }

        if let Some(env_headers) = env_http_headers {
            for (header_name, env_var_name) in env_headers {
                if let Ok(value) = std::env::var(env_var_name) {
                    if !value.trim().is_empty() {
                        config.env_headers.push((header_name.clone(), value));
                    }
                } else {
                    tracing::warn!(
                        "Environment variable '{}' not found for HTTP header '{}'",
                        env_var_name,
                        header_name
                    );
                }
            }
        }

        config
    }

    /// Get all headers as a flat list.
    pub fn all_headers(&self) -> Vec<(&str, &str)> {
        let mut result: Vec<(&str, &str)> = self
            .headers
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        result.extend(
            self.env_headers
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str())),
        );
        result
    }

    /// Build into a reqwest HeaderMap.
    pub fn to_header_map(&self) -> reqwest::header::HeaderMap {
        let mut map = reqwest::header::HeaderMap::new();
        for (name, value) in self.all_headers() {
            if let Ok(header_name) = reqwest::header::HeaderName::from_bytes(name.as_bytes()) {
                if let Ok(header_value) = reqwest::header::HeaderValue::from_str(value) {
                    map.insert(header_name, header_value);
                }
            }
        }
        map
    }
}

// ---------------------------------------------------------------------------
// DELETE Session Support
// ---------------------------------------------------------------------------

/// Send DELETE request to close MCP session (Streamable HTTP).
pub async fn delete_session(
    client: &reqwest::Client,
    url: &str,
    session_id: &str,
    auth_token: Option<&str>,
) -> Result<(), String> {
    let mut request = client.delete(url).header("Mcp-Session-Id", session_id);

    if let Some(token) = auth_token {
        request = request.bearer_auth(token);
    }

    let response = request
        .send()
        .await
        .map_err(|e| format!("DELETE session request failed: {e}"))?;

    if response.status().is_success() || response.status().as_u16() == 405 {
        // 405 means server doesn't support DELETE, which is fine
        tracing::debug!("Session deleted successfully");
        Ok(())
    } else {
        tracing::warn!("Failed to delete session: HTTP {}", response.status());
        Ok(()) // Non-critical error, don't propagate
    }
}

// ---------------------------------------------------------------------------
// Active Timeout Tracker
// ---------------------------------------------------------------------------

/// Pause-aware timeout tracker.
///
/// Pauses the timeout counter during elicitation or other blocking operations.
#[derive(Debug)]
pub struct ActiveTimeoutTracker {
    /// Total allowed active time.
    timeout: Duration,
    /// Accumulated active time.
    active_time: Arc<RwLock<Duration>>,
    /// When the current active period started (None if paused).
    active_since: Arc<RwLock<Option<Instant>>>,
}

impl ActiveTimeoutTracker {
    /// Create a new timeout tracker.
    pub fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            active_time: Arc::new(RwLock::new(Duration::ZERO)),
            active_since: Arc::new(RwLock::new(Some(Instant::now()))),
        }
    }

    /// Pause the timeout (e.g., during elicitation).
    pub async fn pause(&self) {
        let mut since = self.active_since.write().await;
        if let Some(start) = since.take() {
            let elapsed = start.elapsed();
            let mut total = self.active_time.write().await;
            *total += elapsed;
            tracing::trace!("Timeout paused, accumulated: {:?}", *total);
        }
    }

    /// Resume the timeout.
    pub async fn resume(&self) {
        let mut since = self.active_since.write().await;
        if since.is_none() {
            *since = Some(Instant::now());
            tracing::trace!("Timeout resumed");
        }
    }

    /// Check if timeout has been exceeded.
    pub async fn is_expired(&self) -> bool {
        let total = *self.active_time.read().await;
        let current = self
            .active_since
            .read()
            .await
            .map(|start| start.elapsed())
            .unwrap_or(Duration::ZERO);

        total + current >= self.timeout
    }

    /// Get remaining time.
    pub async fn remaining(&self) -> Duration {
        let total = *self.active_time.read().await;
        let current = self
            .active_since
            .read()
            .await
            .map(|start| start.elapsed())
            .unwrap_or(Duration::ZERO);

        self.timeout.saturating_sub(total + current)
    }

    /// Reset the tracker.
    pub async fn reset(&self) {
        *self.active_time.write().await = Duration::ZERO;
        *self.active_since.write().await = Some(Instant::now());
    }

    /// Get the total configured timeout.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

// ---------------------------------------------------------------------------
// Elicitation Pause State
// ---------------------------------------------------------------------------

/// Tracks whether elicitation is currently active for pause-aware timeouts.
#[derive(Debug, Clone)]
pub struct ElicitationPauseState {
    active_count: Arc<std::sync::atomic::AtomicUsize>,
    paused: Arc<tokio::sync::watch::Sender<bool>>,
}

impl ElicitationPauseState {
    /// Create a new pause state.
    pub fn new() -> Self {
        let (paused, _rx) = tokio::sync::watch::channel(false);
        Self {
            active_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            paused: Arc::new(paused),
        }
    }

    /// Enter elicitation mode (increments active count).
    /// Returns a guard that will exit elicitation mode on drop.
    pub fn enter(&self) -> ElicitationPauseGuard {
        if self.active_count.fetch_add(1, Ordering::AcqRel) == 0 {
            self.paused.send_replace(true);
        }
        ElicitationPauseGuard {
            pause_state: self.clone(),
        }
    }

    /// Subscribe to pause state changes.
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<bool> {
        self.paused.subscribe()
    }

    /// Check if currently paused.
    pub fn is_paused(&self) -> bool {
        *self.paused.borrow()
    }
}

impl Default for ElicitationPauseState {
    fn default() -> Self {
        Self::new()
    }
}

/// Guard that exits elicitation mode on drop.
#[derive(Debug)]
pub struct ElicitationPauseGuard {
    pause_state: ElicitationPauseState,
}

impl Drop for ElicitationPauseGuard {
    fn drop(&mut self) {
        if self.pause_state.active_count.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.pause_state.paused.send_replace(false);
        }
    }
}

// ---------------------------------------------------------------------------
// Active Time Timeout Function
// ---------------------------------------------------------------------------

/// Execute a future with pause-aware timeout.
///
/// This function pauses the timeout counter when the pause_state signals "paused".
/// It's useful for operations that may include long-running elicitation calls
/// that shouldn't count against the active timeout.
pub async fn active_time_timeout<T, Fut>(
    duration: Duration,
    mut pause_state: tokio::sync::watch::Receiver<bool>,
    operation: Fut,
) -> Result<T, ()>
where
    Fut: std::future::Future<Output = T>,
{
    let mut remaining = duration;
    tokio::pin!(operation);

    loop {
        if *pause_state.borrow_and_update() {
            tokio::select! {
                result = &mut operation => return Ok(result),
                changed = pause_state.changed() => {
                    if changed.is_err() {
                        return tokio::time::timeout(remaining, operation).await.map_err(|_| ());
                    }
                    let _paused = *pause_state.borrow_and_update();
                }
            }
            continue;
        }

        let active_start = Instant::now();
        tokio::select! {
            result = &mut operation => return Ok(result),
            _ = tokio::time::sleep(remaining) => {
                return Err(());
            }
            changed = pause_state.changed() => {
                if changed.is_err() {
                    return tokio::time::timeout(remaining, operation).await.map_err(|_| ());
                }
                if *pause_state.borrow_and_update() {
                    remaining = remaining.saturating_sub(active_start.elapsed());
                    if remaining.is_zero() {
                        return Err(());
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time;

    // SessionState tests

    #[test]
    fn test_session_state_transitions() {
        let connecting = SessionState::Connecting;
        assert!(matches!(connecting, SessionState::Connecting));

        let ready = SessionState::Ready {
            session_id: Some("test-id".to_string()),
            server_info: None,
        };
        assert!(matches!(ready, SessionState::Ready { .. }));

        let needs_recovery = SessionState::NeedsRecovery {
            reason: "test".to_string(),
        };
        assert!(matches!(needs_recovery, SessionState::NeedsRecovery { .. }));

        let closed = SessionState::Closed {
            reason: "done".to_string(),
        };
        assert!(matches!(closed, SessionState::Closed { .. }));
    }

    // SessionManager tests

    #[tokio::test]
    async fn test_session_manager_initial_state() {
        let manager = SessionManager::new(3);
        assert!(matches!(manager.state().await, SessionState::Connecting));
        assert!(!manager.is_ready().await);
        assert!(!manager.is_closed().await);
    }

    #[tokio::test]
    async fn test_session_manager_set_ready() {
        let manager = SessionManager::new(3);
        manager.set_ready(Some("sid-123".to_string()), None).await;
        assert!(manager.is_ready().await);
        assert_eq!(manager.session_id().await, Some("sid-123".to_string()));
    }

    #[tokio::test]
    async fn test_session_manager_handle_session_expired() {
        let manager = SessionManager::new(3);

        // First three attempts should succeed (max = 3)
        assert!(manager.handle_session_expired().await);
        assert!(manager.needs_recovery().await);

        assert!(manager.handle_session_expired().await);
        assert!(manager.handle_session_expired().await);

        // Fourth attempt should fail (counter reached max)
        assert!(!manager.handle_session_expired().await);
        assert!(manager.is_closed().await);
    }

    #[tokio::test]
    async fn test_session_manager_close() {
        let manager = SessionManager::new(3);
        manager.set_ready(Some("sid".to_string()), None).await;
        manager.close("test close").await;
        assert!(manager.is_closed().await);
    }

    #[tokio::test]
    async fn test_session_manager_recovery_attempts_reset() {
        let manager = SessionManager::new(3);
        manager.handle_session_expired().await;
        assert_eq!(manager.recovery_attempts.load(Ordering::SeqCst), 1);

        manager.set_ready(None, None).await;
        assert_eq!(manager.recovery_attempts.load(Ordering::SeqCst), 0);
    }

    // HttpSessionIdManager tests

    #[tokio::test]
    async fn test_http_session_id_manager_initial_state() {
        let manager = HttpSessionIdManager::new();
        assert!(manager.get_session_id().await.is_none());
        assert!(manager.get_last_event_id().await.is_none());
    }

    #[tokio::test]
    async fn test_http_session_id_manager_update_from_response() {
        let manager = HttpSessionIdManager::new();
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("mcp-session-id", "new-session-id".parse().unwrap());

        manager.update_from_response(&headers).await;
        assert_eq!(
            manager.get_session_id().await,
            Some("new-session-id".to_string())
        );
    }

    #[tokio::test]
    async fn test_http_session_id_manager_last_event_id() {
        let manager = HttpSessionIdManager::new();
        manager.update_last_event_id("event-123").await;
        assert_eq!(
            manager.get_last_event_id().await,
            Some("event-123".to_string())
        );
    }

    #[tokio::test]
    async fn test_http_session_id_manager_build_headers() {
        let manager = HttpSessionIdManager::new();
        manager.update_last_event_id("event-1").await;

        let headers = manager.build_request_headers().await;
        assert_eq!(headers.len(), 1);
        assert_eq!(
            headers[0],
            ("Last-Event-Id".to_string(), "event-1".to_string())
        );
    }

    #[tokio::test]
    async fn test_http_session_id_manager_clear() {
        let manager = HttpSessionIdManager::new();
        manager.update_last_event_id("event-1").await;
        manager.clear().await;
        assert!(manager.get_session_id().await.is_none());
        assert!(manager.get_last_event_id().await.is_none());
    }

    // classify_http_error tests

    #[test]
    fn test_classify_http_error_success() {
        assert!(classify_http_error(200, false).is_none());
        assert!(classify_http_error(201, true).is_none());
        assert!(classify_http_error(204, false).is_none());
    }

    #[test]
    fn test_classify_http_error_session_expired() {
        let error = classify_http_error(404, true);
        assert_eq!(error, Some(HttpResponseError::SessionExpired));

        // 404 without session_id should be client error
        let error = classify_http_error(404, false);
        assert_eq!(error, Some(HttpResponseError::ClientError(404)));
    }

    #[test]
    fn test_classify_http_error_auth_required() {
        let error = classify_http_error(401, false);
        assert_eq!(error, Some(HttpResponseError::AuthRequired));
    }

    #[test]
    fn test_classify_http_error_server_error() {
        let error = classify_http_error(500, false);
        assert_eq!(error, Some(HttpResponseError::ServerError(500)));

        let error = classify_http_error(503, true);
        assert_eq!(error, Some(HttpResponseError::ServerError(503)));
    }

    #[test]
    fn test_classify_http_error_client_error() {
        let error = classify_http_error(400, false);
        assert_eq!(error, Some(HttpResponseError::ClientError(400)));

        let error = classify_http_error(403, true);
        assert_eq!(error, Some(HttpResponseError::ClientError(403)));
    }

    #[test]
    fn test_is_recoverable_error() {
        assert!(is_recoverable_error(&HttpResponseError::SessionExpired));
        assert!(is_recoverable_error(&HttpResponseError::AuthRequired));
        assert!(is_recoverable_error(&HttpResponseError::ServerError(500)));
        assert!(!is_recoverable_error(&HttpResponseError::ClientError(400)));
        assert!(!is_recoverable_error(&HttpResponseError::NetworkError(
            "test".to_string()
        )));
    }

    // HttpHeadersConfig tests

    #[test]
    fn test_http_headers_config_empty() {
        let config = HttpHeadersConfig::from_config(None, None);
        assert!(config.all_headers().is_empty());
    }

    #[test]
    fn test_http_headers_config_static_headers() {
        let mut headers = HashMap::new();
        headers.insert("X-Custom".to_string(), "value".to_string());

        let config = HttpHeadersConfig::from_config(Some(&headers), None);
        let all = config.all_headers();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0], ("X-Custom", "value"));
    }

    #[test]
    fn test_http_headers_config_env_headers() {
        std::env::set_var("TEST_HEADER_VALUE", "from-env");
        let mut env_headers = HashMap::new();
        env_headers.insert("X-Env-Header".to_string(), "TEST_HEADER_VALUE".to_string());

        let config = HttpHeadersConfig::from_config(None, Some(&env_headers));
        let all = config.all_headers();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0], ("X-Env-Header", "from-env"));

        std::env::remove_var("TEST_HEADER_VALUE");
    }

    #[test]
    fn test_http_headers_config_to_header_map() {
        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), "Bearer token".to_string());

        let config = HttpHeadersConfig::from_config(Some(&headers), None);
        let map = config.to_header_map();

        assert_eq!(
            map.get("Authorization").unwrap().to_str().unwrap(),
            "Bearer token"
        );
    }

    // ActiveTimeoutTracker tests

    #[tokio::test]
    async fn test_active_timeout_tracker_not_expired() {
        let tracker = ActiveTimeoutTracker::new(Duration::from_secs(60));
        assert!(!tracker.is_expired().await);
    }

    #[tokio::test]
    async fn test_active_timeout_tracker_pause_resume() {
        let tracker = ActiveTimeoutTracker::new(Duration::from_millis(100));

        // Pause for a while
        tracker.pause().await;
        time::sleep(Duration::from_millis(150)).await;
        tracker.resume().await;

        // Should still have time remaining because timeout was paused
        let remaining = tracker.remaining().await;
        assert!(remaining > Duration::ZERO);
    }

    #[tokio::test]
    async fn test_active_timeout_tracker_reset() {
        let tracker = ActiveTimeoutTracker::new(Duration::from_secs(10));

        // Let some time pass
        time::sleep(Duration::from_millis(50)).await;

        // Reset
        tracker.reset().await;
        assert!(!tracker.is_expired().await);
    }

    // ElicitationPauseState tests

    #[test]
    fn test_elicitation_pause_state_initial() {
        let state = ElicitationPauseState::new();
        assert!(!state.is_paused());
    }

    #[test]
    fn test_elicitation_pause_state_enter_exit() {
        let state = ElicitationPauseState::new();
        assert!(!state.is_paused());

        let guard = state.enter();
        assert!(state.is_paused());

        drop(guard);
        assert!(!state.is_paused());
    }

    #[test]
    fn test_elicitation_pause_state_nested() {
        let state = ElicitationPauseState::new();

        let guard1 = state.enter();
        assert!(state.is_paused());

        let guard2 = state.enter();
        assert!(state.is_paused());

        drop(guard1);
        assert!(state.is_paused());

        drop(guard2);
        assert!(!state.is_paused());
    }

    // active_time_timeout tests

    #[tokio::test]
    async fn test_active_time_timeout_completes() {
        let pause_state = ElicitationPauseState::new();
        let result =
            active_time_timeout(Duration::from_millis(100), pause_state.subscribe(), async {
                "done"
            })
            .await;

        assert_eq!(result, Ok("done"));
    }

    #[tokio::test]
    async fn test_active_time_timeout_times_out() {
        let pause_state = ElicitationPauseState::new();
        let result =
            active_time_timeout(Duration::from_millis(10), pause_state.subscribe(), async {
                time::sleep(Duration::from_millis(100)).await;
                "done"
            })
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_active_time_timeout_pauses_during_elicitation() {
        let pause_state = ElicitationPauseState::new();
        let pause = pause_state.enter();

        tokio::spawn(async move {
            time::sleep(Duration::from_millis(75)).await;
            drop(pause);
        });

        let result =
            active_time_timeout(Duration::from_millis(50), pause_state.subscribe(), async {
                time::sleep(Duration::from_millis(90)).await;
                "done"
            })
            .await;

        // Should succeed because timeout was paused for ~75ms
        assert_eq!(result, Ok("done"));
    }
}
