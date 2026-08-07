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

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BreakpointEdit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<Vec<(String, String)>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

pub type PendingBreakpoint = crate::push::BreakpointPausedPushData;

struct BreakpointHandle {
    sender: Option<oneshot::Sender<BreakpointEdit>>,
    body_editable: bool,
    snapshot: PendingBreakpoint,
}

type BreakpointReceiver = oneshot::Receiver<BreakpointEdit>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakpointResumeError {
    NotFound,
    PhaseMismatch,
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

    fn pause(&self, snapshot: PendingBreakpoint, body_editable: bool) -> BreakpointReceiver {
        let (tx, rx) = oneshot::channel();
        let handle = BreakpointHandle {
            sender: Some(tx),
            body_editable,
            snapshot: snapshot.clone(),
        };
        self.pending.insert(snapshot.request_id.clone(), handle);
        rx
    }

    pub fn pause_request(&self, snapshot: PendingBreakpoint, editable: bool) -> BreakpointReceiver {
        debug_assert_eq!(snapshot.phase, "request");
        self.pause(snapshot, editable)
    }

    pub fn pause_response(
        &self,
        snapshot: PendingBreakpoint,
        editable: bool,
    ) -> BreakpointReceiver {
        debug_assert_eq!(snapshot.phase, "response");
        self.pause(snapshot, editable)
    }

    pub fn resume(
        &self,
        request_id: &str,
        phase: &str,
        mut edit: BreakpointEdit,
    ) -> Result<(), BreakpointResumeError> {
        let mut entry = self
            .pending
            .get_mut(request_id)
            .ok_or(BreakpointResumeError::NotFound)?;
        if entry.snapshot.phase != phase {
            return Err(BreakpointResumeError::PhaseMismatch);
        }
        if edit
            .body
            .as_ref()
            .map(|body| !entry.body_editable || !self.body_within_capture_limit(body.len()))
            .unwrap_or(false)
        {
            edit.body = None;
        }
        let sender = entry.sender.take();
        drop(entry);
        self.pending.remove(request_id);
        sender
            .ok_or(BreakpointResumeError::NotFound)?
            .send(edit)
            .map_err(|_| BreakpointResumeError::NotFound)
    }

    pub fn cancel(&self, request_id: &str, phase: &str) -> bool {
        let phase_matches = self
            .pending
            .get(request_id)
            .is_some_and(|entry| entry.snapshot.phase == phase);
        if phase_matches {
            self.pending.remove(request_id);
            return true;
        }
        false
    }

    pub fn cancel_all(&self) {
        self.pending.clear();
    }

    pub fn has_pending(&self, request_id: &str) -> bool {
        self.pending.contains_key(request_id)
    }

    pub fn pending(&self) -> Vec<PendingBreakpoint> {
        let mut pending = self
            .pending
            .iter()
            .map(|entry| entry.snapshot.clone())
            .collect::<Vec<_>>();
        pending.sort_by_key(|item| (item.paused_at_ms, item.request_id.clone()));
        pending
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
        let rx = manager.pause_request(pending("req-1", "request"), false);

        assert!(manager
            .resume(
                "req-1",
                "request",
                BreakpointEdit {
                    headers: Some(vec![("x-test".to_string(), "1".to_string())]),
                    body: Some("blocked body edit".to_string()),
                    ..Default::default()
                },
            )
            .is_ok());

        let edit = rx.await.unwrap();
        assert_eq!(
            edit.headers,
            Some(vec![("x-test".to_string(), "1".to_string())])
        );
        assert!(edit.body.is_none());
    }

    fn pending(id: &str, phase: &str) -> PendingBreakpoint {
        PendingBreakpoint {
            request_id: id.to_string(),
            phase: phase.to_string(),
            method: Some("GET".to_string()),
            url: Some("http://example.test/".to_string()),
            status: None,
            headers: Vec::new(),
            body: None,
            body_omitted: false,
            body_size: Some(0),
            max_body_bytes: DEFAULT_BREAKPOINT_MAX_BODY_BYTES,
            content_encoding: None,
            paused_at_ms: 10,
            deadline_at_ms: 20,
        }
    }

    #[tokio::test]
    async fn pending_snapshot_is_available_before_resume_and_phase_is_strict() {
        let manager = BreakpointManager::new();
        let rx = manager.pause_request(pending("req-2", "request"), true);

        assert_eq!(manager.pending(), vec![pending("req-2", "request")]);
        assert_eq!(
            manager.resume("req-2", "response", BreakpointEdit::default()),
            Err(BreakpointResumeError::PhaseMismatch)
        );
        assert!(manager.has_pending("req-2"));
        assert!(manager
            .resume("req-2", "request", BreakpointEdit::default())
            .is_ok());
        assert_eq!(rx.await.unwrap(), BreakpointEdit::default());
        assert!(manager.pending().is_empty());
    }

    #[tokio::test]
    async fn request_and_response_snapshots_replace_each_other_sequentially() {
        let manager = BreakpointManager::new();
        let request_rx = manager.pause_request(pending("req-3", "request"), true);
        manager
            .resume("req-3", "request", BreakpointEdit::default())
            .unwrap();
        request_rx.await.unwrap();

        let response_rx = manager.pause_response(pending("req-3", "response"), true);
        assert_eq!(manager.pending()[0].phase, "response");
        manager
            .resume("req-3", "response", BreakpointEdit::default())
            .unwrap();
        response_rx.await.unwrap();
        assert!(manager.pending().is_empty());
    }

    #[tokio::test]
    async fn manager_rejects_missing_sender_and_oversized_body_edits() {
        let error = BreakpointResumeError::NotFound;
        assert_eq!(format!("{error:?}"), "NotFound");
        assert_eq!(<BreakpointResumeError as Clone>::clone(&error), error);
        let copied = error;
        assert_eq!(copied, error);

        let manager = BreakpointManager::new();
        manager.update_settings(BreakpointSettings {
            enabled: true,
            max_body_bytes: 3,
        });
        assert_eq!(
            manager.resume("missing", "request", BreakpointEdit::default()),
            Err(BreakpointResumeError::NotFound)
        );

        let rx = manager.pause_request(pending("oversized", "request"), true);
        manager
            .resume(
                "oversized",
                "request",
                BreakpointEdit {
                    body: Some("four".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(rx.await.unwrap().body.is_none());

        let dropped = manager.pause_request(pending("dropped", "request"), true);
        drop(dropped);
        assert_eq!(
            manager.resume("dropped", "request", BreakpointEdit::default()),
            Err(BreakpointResumeError::NotFound)
        );

        let _missing_sender = manager.pause_request(pending("senderless", "request"), true);
        manager.pending.get_mut("senderless").unwrap().sender.take();
        assert_eq!(
            manager.resume("senderless", "request", BreakpointEdit::default()),
            Err(BreakpointResumeError::NotFound)
        );
    }

    #[test]
    fn cancel_is_phase_strict_and_disable_cancels_all() {
        let manager = BreakpointManager::new();
        let _first = manager.pause_request(pending("first", "request"), true);
        let _second = manager.pause_response(pending("second", "response"), true);
        assert!(!manager.cancel("first", "response"));
        assert!(manager.has_pending("first"));
        assert!(manager.cancel("first", "request"));
        assert!(!manager.cancel("missing", "request"));

        manager.update_settings(BreakpointSettings {
            enabled: false,
            max_body_bytes: 9,
        });
        assert!(manager.pending().is_empty());
    }

    #[test]
    fn breakpoint_payloads_round_trip_through_json() {
        let snapshot = PendingBreakpoint {
            method: None,
            url: None,
            status: Some(418),
            headers: vec![("set-cookie".to_string(), "a=1".to_string())],
            body: Some("teapot".to_string()),
            body_omitted: false,
            body_size: Some(6),
            max_body_bytes: 10,
            content_encoding: Some("gzip".to_string()),
            ..pending("serde", "response")
        };
        let encoded = serde_json::to_string(&snapshot).unwrap();
        assert_eq!(
            serde_json::from_str::<PendingBreakpoint>(&encoded).unwrap(),
            snapshot
        );
        assert!(format!("{snapshot:?}").contains("serde"));

        let edit = BreakpointEdit {
            method: Some("PUT".to_string()),
            url: Some("https://example.test/".to_string()),
            status: Some(201),
            headers: Some(vec![("x-test".to_string(), "yes".to_string())]),
            body: Some("body".to_string()),
        };
        let encoded = serde_json::to_string(&edit).unwrap();
        assert_eq!(
            serde_json::from_str::<BreakpointEdit>(&encoded).unwrap(),
            edit
        );
    }
}
