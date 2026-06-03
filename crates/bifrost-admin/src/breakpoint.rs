use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::oneshot;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BreakpointSettings {
    pub enabled: bool,
    pub hook_request: bool,
    pub hook_response: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BreakpointEdit {
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

struct BreakpointHandle {
    request: Option<oneshot::Sender<BreakpointEdit>>,
    response: Option<oneshot::Sender<BreakpointEdit>>,
}

pub struct BreakpointManager {
    enabled: AtomicBool,
    hook_request: AtomicBool,
    hook_response: AtomicBool,
    pending: DashMap<String, BreakpointHandle>,
}

impl BreakpointManager {
    pub fn new() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            hook_request: AtomicBool::new(false),
            hook_response: AtomicBool::new(false),
            pending: DashMap::new(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn hook_request_enabled(&self) -> bool {
        self.is_enabled() && self.hook_request.load(Ordering::Relaxed)
    }

    pub fn hook_response_enabled(&self) -> bool {
        self.is_enabled() && self.hook_response.load(Ordering::Relaxed)
    }

    pub fn get_settings(&self) -> BreakpointSettings {
        BreakpointSettings {
            enabled: self.is_enabled(),
            hook_request: self.hook_request.load(Ordering::Relaxed),
            hook_response: self.hook_response.load(Ordering::Relaxed),
        }
    }

    pub fn update_settings(&self, settings: BreakpointSettings) {
        self.enabled.store(settings.enabled, Ordering::Relaxed);
        self.hook_request
            .store(settings.hook_request, Ordering::Relaxed);
        self.hook_response
            .store(settings.hook_response, Ordering::Relaxed);

        if !settings.enabled {
            self.cancel_all();
        }
    }

    pub fn pause_request(&self, request_id: String) -> oneshot::Receiver<BreakpointEdit> {
        let (tx, rx) = oneshot::channel();
        let handle = BreakpointHandle {
            request: Some(tx),
            response: None,
        };
        self.pending.insert(request_id, handle);
        rx
    }

    pub fn pause_response(&self, request_id: String) -> oneshot::Receiver<BreakpointEdit> {
        let (tx, rx) = oneshot::channel();

        if let Some(mut entry) = self.pending.get_mut(&request_id) {
            entry.response = Some(tx);
        } else {
            self.pending.insert(
                request_id,
                BreakpointHandle {
                    request: None,
                    response: Some(tx),
                },
            );
        }

        rx
    }

    pub fn resume(&self, request_id: &str, phase: &str, edit: BreakpointEdit) -> bool {
        if let Some((_, handle)) = self.pending.remove(request_id) {
            let tx = match phase {
                "request" => handle.request,
                "response" => handle.response,
                _ => handle.request.or(handle.response),
            };
            if let Some(tx) = tx {
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
