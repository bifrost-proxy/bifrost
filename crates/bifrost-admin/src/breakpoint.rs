use base64::Engine;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::UNIX_EPOCH;
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_encoding: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_representation: Option<String>,
}

pub type PendingBreakpoint = crate::push::BreakpointPausedPushData;

struct BreakpointHandle {
    sender: Option<oneshot::Sender<BreakpointEdit>>,
    body_editable: bool,
    snapshot: PendingBreakpoint,
}

type BreakpointReceiver = oneshot::Receiver<BreakpointEdit>;

pub enum BreakpointResumeError {
    NotFound,
    PhaseMismatch,
    InvalidEdit(String),
}

pub struct BreakpointManager {
    enabled: AtomicBool,
    max_body_bytes: AtomicUsize,
    timeout_ms: AtomicU64,
    pending: DashMap<String, BreakpointHandle>,
    lifecycle: std::sync::Mutex<()>,
}

impl BreakpointManager {
    pub fn new() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            max_body_bytes: AtomicUsize::new(DEFAULT_BREAKPOINT_MAX_BODY_BYTES),
            timeout_ms: AtomicU64::new(DEFAULT_BREAKPOINT_TIMEOUT_MS),
            pending: DashMap::new(),
            lifecycle: std::sync::Mutex::new(()),
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
        let _lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());
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

    pub fn pause(&self, snapshot: PendingBreakpoint, body_editable: bool) -> BreakpointReceiver {
        let (tx, rx) = oneshot::channel();
        let handle = BreakpointHandle {
            sender: Some(tx),
            body_editable,
            snapshot: snapshot.clone(),
        };
        self.pending.insert(snapshot.request_id.clone(), handle);
        rx
    }

    pub fn pause_if_enabled(
        &self,
        snapshot: PendingBreakpoint,
        body_editable: bool,
    ) -> Option<BreakpointReceiver> {
        let _lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.is_enabled()
            .then(|| self.pause(snapshot, body_editable))
    }

    pub fn resume(
        &self,
        request_id: &str,
        phase: &str,
        edit: BreakpointEdit,
    ) -> Result<(), BreakpointResumeError> {
        let mut entry = self
            .pending
            .get_mut(request_id)
            .ok_or(BreakpointResumeError::NotFound)?;
        if entry.snapshot.phase != phase {
            return Err(BreakpointResumeError::PhaseMismatch);
        }
        if edit
            .status
            .is_some_and(|status| !(200..=599).contains(&status))
        {
            return Err(BreakpointResumeError::InvalidEdit(
                "Final response status must be between 200 and 599".into(),
            ));
        }
        if edit.body.is_none() && entry.snapshot.body_size != Some(0) {
            if let Some(headers) = &edit.headers {
                let encodings = |headers: &[(String, String)]| {
                    headers
                        .iter()
                        .filter(|(name, _)| name.eq_ignore_ascii_case("content-encoding"))
                        .map(|(_, value)| value.trim().to_ascii_lowercase())
                        .collect::<Vec<_>>()
                        .join(",")
                };
                if encodings(headers) != encodings(&entry.snapshot.headers) {
                    return Err(BreakpointResumeError::InvalidEdit("Changing Content-Encoding requires a body edit to keep wire bytes consistent".into()));
                }
            }
        }
        if edit.body.is_some()
            && phase == "response"
            && entry
                .snapshot
                .method
                .as_deref()
                .is_some_and(|method| method.eq_ignore_ascii_case("HEAD"))
        {
            return Err(BreakpointResumeError::InvalidEdit(
                "HEAD responses cannot carry a body".into(),
            ));
        }
        if let Some(body) = edit.body.as_ref() {
            if !entry.body_editable {
                return Err(BreakpointResumeError::InvalidEdit(
                    "Body is unavailable or exceeds the capture limit".into(),
                ));
            }
            let encoding = edit.body_encoding.as_deref().unwrap_or("utf8");
            let len = match encoding {
                "utf8" => body.len(),
                "base64" => {
                    if body.len() > self.max_body_bytes().saturating_add(2) / 3 * 4 {
                        return Err(BreakpointResumeError::InvalidEdit(
                            "Body exceeds the capture limit".into(),
                        ));
                    }
                    base64::engine::general_purpose::STANDARD
                        .decode(body)
                        .map_err(|_| {
                            BreakpointResumeError::InvalidEdit("Invalid Base64 body".into())
                        })?
                        .len()
                }
                _ => {
                    return Err(BreakpointResumeError::InvalidEdit(
                        "body_encoding must be utf8 or base64".into(),
                    ))
                }
            };
            if !self.body_within_capture_limit(len) {
                return Err(BreakpointResumeError::InvalidEdit(
                    "Body exceeds the capture limit".into(),
                ));
            }
            let representation = edit.body_representation.as_deref().unwrap_or("decoded");
            if !matches!(representation, "decoded" | "raw") {
                return Err(BreakpointResumeError::InvalidEdit(
                    "body_representation must be decoded or raw".into(),
                ));
            }
            let effective_headers = edit.headers.as_ref().unwrap_or(&entry.snapshot.headers);
            if representation == "decoded" {
                for (_, encoding) in effective_headers
                    .iter()
                    .filter(|(name, _)| name.eq_ignore_ascii_case("content-encoding"))
                {
                    if encoding.split(',').any(|part| {
                        !matches!(
                            part.trim().to_ascii_lowercase().as_str(),
                            "" | "identity" | "gzip" | "x-gzip" | "deflate" | "br" | "zstd"
                        )
                    }) {
                        return Err(BreakpointResumeError::InvalidEdit(
                            "Unsupported content-encoding; edit raw bytes instead".into(),
                        ));
                    }
                }
            }
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
        let server_now_ms = UNIX_EPOCH.elapsed().unwrap_or_default().as_millis() as u64;
        let mut pending = self
            .pending
            .iter()
            .map(|entry| {
                let mut snapshot = entry.snapshot.clone();
                snapshot.server_now_ms = server_now_ms;
                snapshot
            })
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
    async fn invalid_body_edits_leave_pause_available_for_correction() {
        let manager = BreakpointManager::new();
        manager.update_settings(BreakpointSettings {
            enabled: true,
            max_body_bytes: 3,
        });
        let mut rx = manager.pause(pending("req-1", "request"), false);
        assert!(matches!(
            manager.resume(
                "req-1",
                "request",
                BreakpointEdit {
                    body: Some("x".into()),
                    ..Default::default()
                }
            ),
            Err(BreakpointResumeError::InvalidEdit(_))
        ));
        assert!(rx.try_recv().is_err());
        assert!(manager.has_pending("req-1"));
        manager
            .resume("req-1", "request", BreakpointEdit::default())
            .unwrap_or_else(|_| panic!("resume without edits"));
        rx.await.unwrap();

        let rx = manager.pause(pending("binary", "response"), true);
        for status in [100, 101, 103, 199, 600] {
            assert!(matches!(
                manager.resume(
                    "binary",
                    "response",
                    BreakpointEdit {
                        status: Some(status),
                        ..Default::default()
                    }
                ),
                Err(BreakpointResumeError::InvalidEdit(_))
            ));
            assert!(manager.has_pending("binary"));
        }
        for body in ["not-base64", "AQIDBA=="] {
            assert!(matches!(
                manager.resume(
                    "binary",
                    "response",
                    BreakpointEdit {
                        body: Some(body.into()),
                        body_encoding: Some("base64".into()),
                        ..Default::default()
                    }
                ),
                Err(BreakpointResumeError::InvalidEdit(_))
            ));
            assert!(manager.has_pending("binary"));
        }
        assert!(manager
            .resume(
                "binary",
                "response",
                BreakpointEdit {
                    body: Some("AP/+".into()),
                    body_encoding: Some("base64".into()),
                    body_representation: Some("raw".into()),
                    ..Default::default()
                }
            )
            .is_ok());
        assert_eq!(rx.await.unwrap().body.as_deref(), Some("AP/+"));
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
            body_encoding: "utf8".into(),
            body_representation: "decoded".into(),
            body_omitted: false,
            body_size: Some(0),
            max_body_bytes: DEFAULT_BREAKPOINT_MAX_BODY_BYTES,
            content_encoding: None,
            paused_at_ms: 10,
            deadline_at_ms: 20,
            server_now_ms: 10,
        }
    }

    #[tokio::test]
    async fn pending_snapshot_is_available_before_resume_and_phase_is_strict() {
        let manager = BreakpointManager::new();
        let rx = manager.pause(pending("req-2", "request"), true);

        let listed = manager.pending();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].request_id, "req-2");
        assert_eq!(listed[0].phase, "request");
        assert!(listed[0].server_now_ms >= listed[0].paused_at_ms);
        assert!(matches!(
            manager.resume("req-2", "response", BreakpointEdit::default()),
            Err(BreakpointResumeError::PhaseMismatch)
        ));
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
        let request_rx = manager.pause(pending("req-3", "request"), true);
        assert!(manager
            .resume("req-3", "request", BreakpointEdit::default())
            .is_ok());
        request_rx.await.unwrap();

        let response_rx = manager.pause(pending("req-3", "response"), true);
        assert_eq!(manager.pending()[0].phase, "response");
        assert!(manager
            .resume("req-3", "response", BreakpointEdit::default())
            .is_ok());
        response_rx.await.unwrap();
        assert!(manager.pending().is_empty());
    }

    #[tokio::test]
    async fn manager_rejects_missing_sender_and_oversized_body_edits() {
        let manager = BreakpointManager::new();
        manager.update_settings(BreakpointSettings {
            enabled: true,
            max_body_bytes: 3,
        });
        assert!(matches!(
            manager.resume("missing", "request", BreakpointEdit::default()),
            Err(BreakpointResumeError::NotFound)
        ));

        let rx = manager.pause(pending("oversized", "request"), true);
        assert!(manager
            .resume(
                "oversized",
                "request",
                BreakpointEdit {
                    body: Some("four".to_string()),
                    ..Default::default()
                },
            )
            .is_err());
        assert!(manager.has_pending("oversized"));
        assert!(manager
            .resume("oversized", "request", BreakpointEdit::default())
            .is_ok());
        assert!(rx.await.unwrap().body.is_none());

        let dropped = manager.pause(pending("dropped", "request"), true);
        drop(dropped);
        assert!(matches!(
            manager.resume("dropped", "request", BreakpointEdit::default()),
            Err(BreakpointResumeError::NotFound)
        ));

        let _missing_sender = manager.pause(pending("senderless", "request"), true);
        manager.pending.get_mut("senderless").unwrap().sender.take();
        assert!(matches!(
            manager.resume("senderless", "request", BreakpointEdit::default()),
            Err(BreakpointResumeError::NotFound)
        ));
    }

    #[test]
    fn production_pause_rechecks_gate_after_rules_are_resolved() {
        let manager = BreakpointManager::new();
        assert!(manager
            .pause_if_enabled(pending("disabled", "request"), true)
            .is_none());
        manager.update_settings(BreakpointSettings {
            enabled: true,
            max_body_bytes: 64,
        });
        let rx = manager
            .pause_if_enabled(pending("enabled", "request"), true)
            .unwrap();
        assert!(manager.has_pending("enabled"));
        manager.update_settings(BreakpointSettings {
            enabled: false,
            max_body_bytes: 64,
        });
        assert!(manager.pending().is_empty());
        assert!(manager
            .pause_if_enabled(pending("after-disable", "request"), true)
            .is_none());
        drop(rx);
    }

    #[test]
    fn cancel_is_phase_strict_and_disable_cancels_all() {
        let manager = BreakpointManager::new();
        let _first = manager.pause(pending("first", "request"), true);
        let _second = manager.pause(pending("second", "response"), true);
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
            body_encoding: "utf8".into(),
            body_representation: "decoded".into(),
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
            ..Default::default()
        };
        let encoded = serde_json::to_string(&edit).unwrap();
        assert_eq!(
            serde_json::from_str::<BreakpointEdit>(&encoded).unwrap(),
            edit
        );
    }
}
