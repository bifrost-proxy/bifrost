use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::oneshot;

pub use bifrost_storage::{
    DEFAULT_BREAKPOINT_TIMEOUT_MS, MAX_BREAKPOINT_TIMEOUT_MS, MIN_BREAKPOINT_TIMEOUT_MS,
};

pub const DEFAULT_BREAKPOINT_MAX_BODY_BYTES: usize = 1024 * 1024;
pub const MAX_BREAKPOINT_MAX_BODY_BYTES: usize = 10 * 1024 * 1024;

fn default_max_body_bytes() -> usize {
    DEFAULT_BREAKPOINT_MAX_BODY_BYTES
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BreakpointSettings {
    pub enabled: bool,
    #[serde(default = "default_max_body_bytes")]
    pub max_body_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BreakpointEdit {
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

struct BreakpointHandle {
    request: Option<oneshot::Sender<BreakpointEdit>>,
    request_body_editable: bool,
    response: Option<oneshot::Sender<BreakpointEdit>>,
    response_body_editable: bool,
}

pub struct BreakpointManager {
    enabled: AtomicBool,
    max_body_bytes: AtomicUsize,
    timeout_ms: AtomicU64,
    pending: DashMap<String, BreakpointHandle>,
}

impl BreakpointManager {
    pub fn new() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            max_body_bytes: AtomicUsize::new(DEFAULT_BREAKPOINT_MAX_BODY_BYTES),
            timeout_ms: AtomicU64::new(DEFAULT_BREAKPOINT_TIMEOUT_MS),
            pending: DashMap::new(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn get_settings(&self) -> BreakpointSettings {
        BreakpointSettings {
            enabled: self.is_enabled(),
            max_body_bytes: self.max_body_bytes(),
        }
    }

    pub fn update_settings(&self, settings: BreakpointSettings) {
        self.enabled.store(settings.enabled, Ordering::Relaxed);
        self.max_body_bytes.store(
            settings.max_body_bytes.min(MAX_BREAKPOINT_MAX_BODY_BYTES),
            Ordering::Relaxed,
        );
        if !settings.enabled {
            self.cancel_all();
        }
    }

    pub fn max_body_bytes(&self) -> usize {
        self.max_body_bytes.load(Ordering::Relaxed)
    }

    pub fn timeout_ms(&self) -> u64 {
        self.timeout_ms.load(Ordering::Relaxed)
    }

    pub fn set_timeout_ms(&self, timeout_ms: u64) {
        self.timeout_ms.store(
            timeout_ms.clamp(MIN_BREAKPOINT_TIMEOUT_MS, MAX_BREAKPOINT_TIMEOUT_MS),
            Ordering::Relaxed,
        );
    }

    pub fn body_within_capture_limit(&self, len: usize) -> bool {
        len <= self.max_body_bytes()
    }

    pub fn pause_request(
        &self,
        request_id: String,
        body_editable: bool,
    ) -> oneshot::Receiver<BreakpointEdit> {
        let (tx, rx) = oneshot::channel();
        let handle = BreakpointHandle {
            request: Some(tx),
            request_body_editable: body_editable,
            response: None,
            response_body_editable: false,
        };
        self.pending.insert(request_id, handle);
        rx
    }

    pub fn pause_response(
        &self,
        request_id: String,
        body_editable: bool,
    ) -> oneshot::Receiver<BreakpointEdit> {
        let (tx, rx) = oneshot::channel();

        if let Some(mut entry) = self.pending.get_mut(&request_id) {
            entry.response = Some(tx);
            entry.response_body_editable = body_editable;
        } else {
            self.pending.insert(
                request_id,
                BreakpointHandle {
                    request: None,
                    request_body_editable: false,
                    response: Some(tx),
                    response_body_editable: body_editable,
                },
            );
        }

        rx
    }

    pub fn resume(&self, request_id: &str, phase: &str, mut edit: BreakpointEdit) -> bool {
        if let Some((_, handle)) = self.pending.remove(request_id) {
            let (tx, body_editable) = match phase {
                "request" => (handle.request, handle.request_body_editable),
                "response" => (handle.response, handle.response_body_editable),
                _ => (
                    handle.request.or(handle.response),
                    handle.request_body_editable || handle.response_body_editable,
                ),
            };
            if let Some(tx) = tx {
                if edit
                    .body
                    .as_ref()
                    .map(|body| !body_editable || !self.body_within_capture_limit(body.len()))
                    .unwrap_or(false)
                {
                    edit.body = None;
                }
                let _ = tx.send(edit);
                return true;
            }
        }
        false
    }

    pub fn cancel_request(&self, request_id: &str) {
        self.pending.remove(request_id);
    }

    fn cancel_all(&self) {
        self.pending.clear();
    }

    pub fn has_pending(&self, request_id: &str) -> bool {
        self.pending.contains_key(request_id)
    }
}

impl Default for BreakpointManager {
    fn default() -> Self {
        Self::new()
    }
}

pub type SharedBreakpointManager = Arc<BreakpointManager>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_are_safe_for_normal_proxying() {
        let manager = BreakpointManager::new();

        let settings = manager.get_settings();
        assert!(!settings.enabled);
        assert_eq!(settings.max_body_bytes, DEFAULT_BREAKPOINT_MAX_BODY_BYTES);
        assert!(!manager.is_enabled());
        assert_eq!(manager.timeout_ms(), DEFAULT_BREAKPOINT_TIMEOUT_MS);
    }

    #[test]
    fn update_settings_clamps_expensive_limits() {
        let manager = BreakpointManager::new();

        manager.update_settings(BreakpointSettings {
            enabled: true,
            max_body_bytes: MAX_BREAKPOINT_MAX_BODY_BYTES + 1,
        });

        let settings = manager.get_settings();
        assert_eq!(settings.max_body_bytes, MAX_BREAKPOINT_MAX_BODY_BYTES);
    }

    #[test]
    fn timeout_is_runtime_performance_config() {
        let manager = BreakpointManager::new();

        manager.set_timeout_ms(MAX_BREAKPOINT_TIMEOUT_MS + 1);
        assert_eq!(manager.timeout_ms(), MAX_BREAKPOINT_TIMEOUT_MS);
    }

    #[tokio::test]
    async fn resume_drops_body_when_pause_was_header_only() {
        let manager = BreakpointManager::new();
        let rx = manager.pause_request("req-1".to_string(), false);

        assert!(manager.resume(
            "req-1",
            "request",
            BreakpointEdit {
                headers: vec![("x-test".to_string(), "1".to_string())],
                body: Some("blocked body edit".to_string()),
            },
        ));

        let edit = rx.await.unwrap();
        assert_eq!(edit.headers, vec![("x-test".to_string(), "1".to_string())]);
        assert!(edit.body.is_none());
    }
}
