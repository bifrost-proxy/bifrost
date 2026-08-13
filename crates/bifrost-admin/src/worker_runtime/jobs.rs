use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::{worker_now_ms, WorkerEvent, WorkerKind};

const MAX_JOB_HISTORY: usize = 256;
const MAX_EVENTS_PER_JOB: usize = 32;
const MAX_EVENT_PAYLOAD_BYTES: usize = 8 * 1024;
const MAX_ARTIFACTS_PER_JOB: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerJobStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelling,
    Cancelled,
}

impl WorkerJobStatus {
    fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerJobEventRecord {
    pub timestamp_ms: u64,
    pub event: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerArtifactRecord {
    pub artifact_id: String,
    pub name: String,
    pub size_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    pub created_at_ms: u64,
    #[serde(skip)]
    path: PathBuf,
}

impl WorkerArtifactRecord {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerJobRecord {
    /// Stable registry identifier. For managed-worker requests this is the
    /// unique request id, while logical_job_id preserves the caller's task id.
    pub id: String,
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logical_job_id: Option<String>,
    pub worker_key: String,
    pub worker_kind: WorkerKind,
    pub operation: String,
    pub status: WorkerJobStatus,
    pub created_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub events: Vec<WorkerJobEventRecord>,
    #[serde(default)]
    pub artifacts: Vec<WorkerArtifactRecord>,
}

#[derive(Default)]
struct RegistryState {
    jobs: HashMap<String, WorkerJobRecord>,
    order: VecDeque<String>,
    request_to_job: HashMap<String, String>,
}

static REGISTRY: OnceLock<Mutex<RegistryState>> = OnceLock::new();

fn registry() -> &'static Mutex<RegistryState> {
    REGISTRY.get_or_init(|| Mutex::new(RegistryState::default()))
}

pub(crate) fn begin_request(
    worker_key: &str,
    worker_kind: WorkerKind,
    request_id: &str,
    logical_job_id: Option<&str>,
    operation: &str,
) {
    let mut state = registry().lock();
    if state.jobs.contains_key(request_id) {
        return;
    }
    prune_history(&mut state);
    let now = worker_now_ms();
    state.jobs.insert(
        request_id.to_string(),
        WorkerJobRecord {
            id: request_id.to_string(),
            request_id: request_id.to_string(),
            logical_job_id: logical_job_id.map(ToString::to_string),
            worker_key: worker_key.to_string(),
            worker_kind,
            operation: operation.to_string(),
            status: WorkerJobStatus::Queued,
            created_at_ms: now,
            started_at_ms: None,
            finished_at_ms: None,
            error: None,
            events: Vec::new(),
            artifacts: Vec::new(),
        },
    );
    state
        .request_to_job
        .insert(request_id.to_string(), request_id.to_string());
    state.order.push_back(request_id.to_string());
}

pub(crate) fn mark_running(request_id: &str) {
    update_status(request_id, WorkerJobStatus::Running, None);
}

pub(crate) fn mark_succeeded(request_id: &str) {
    update_status(request_id, WorkerJobStatus::Succeeded, None);
}

pub(crate) fn mark_failed(request_id: &str, error: impl Into<String>) {
    update_status(request_id, WorkerJobStatus::Failed, Some(error.into()));
}

pub(crate) fn mark_cancelled(request_id: &str, error: Option<String>) {
    update_status(request_id, WorkerJobStatus::Cancelled, error);
}

pub(crate) fn mark_logical_job_cancelling(worker_key: &str, logical_job_id: &str) -> usize {
    let mut state = registry().lock();
    let mut affected = 0;
    for job in state.jobs.values_mut() {
        if job.worker_key == worker_key
            && job.logical_job_id.as_deref() == Some(logical_job_id)
            && !job.status.is_terminal()
        {
            job.status = WorkerJobStatus::Cancelling;
            affected += 1;
        }
    }
    affected
}

pub(crate) fn fail_worker_jobs(worker_key: &str, error: &str) {
    let mut state = registry().lock();
    let now = worker_now_ms();
    for job in state.jobs.values_mut() {
        if job.worker_key == worker_key && !job.status.is_terminal() {
            job.status = WorkerJobStatus::Failed;
            job.error = Some(error.to_string());
            job.finished_at_ms = Some(now);
        }
    }
}

pub(crate) fn record_event(event: &WorkerEvent) {
    let mut state = registry().lock();
    let job_id = event
        .request_id
        .as_deref()
        .and_then(|request_id| state.request_to_job.get(request_id).cloned())
        .or_else(|| {
            event.job_id.as_deref().and_then(|logical_job_id| {
                state
                    .order
                    .iter()
                    .rev()
                    .filter_map(|id| state.jobs.get(id))
                    .find(|job| {
                        job.logical_job_id.as_deref() == Some(logical_job_id)
                            && !job.status.is_terminal()
                    })
                    .map(|job| job.id.clone())
            })
        });
    let Some(job_id) = job_id else {
        return;
    };
    let Some(job) = state.jobs.get_mut(&job_id) else {
        return;
    };
    append_event(job, event.event.clone(), event.payload.clone());
}

pub(crate) fn record_named_event(
    request_id: &str,
    event: impl Into<String>,
    payload: serde_json::Value,
) {
    record_event(&WorkerEvent {
        request_id: Some(request_id.to_string()),
        job_id: None,
        event: event.into(),
        payload,
    });
}

pub fn list_jobs() -> Vec<WorkerJobRecord> {
    let state = registry().lock();
    state
        .order
        .iter()
        .rev()
        .filter_map(|id| state.jobs.get(id).cloned())
        .collect()
}

pub fn get_job(id: &str) -> Option<WorkerJobRecord> {
    registry().lock().jobs.get(id).cloned()
}

pub fn cancel_target(id: &str) -> Option<(String, String)> {
    let mut state = registry().lock();
    let job = state.jobs.get_mut(id)?;
    if job.status.is_terminal() {
        return None;
    }
    job.status = WorkerJobStatus::Cancelling;
    Some((
        job.worker_key.clone(),
        job.logical_job_id
            .clone()
            .unwrap_or_else(|| job.request_id.clone()),
    ))
}

/// Restore an active job after a cancellation request could not be delivered.
///
/// Cancellation is a two-phase operation: the API first marks the job as
/// `cancelling`, then asks the owning runtime to stop it. A missing worker or
/// stale external CLI session must not leave the registry stuck in that
/// transitional state.
pub fn cancel_rejected(id: &str, error: impl Into<String>) -> bool {
    let mut state = registry().lock();
    let Some(job) = state.jobs.get_mut(id) else {
        return false;
    };
    if job.status != WorkerJobStatus::Cancelling {
        return false;
    }

    let error = error.into();
    job.status = if job.started_at_ms.is_some() {
        WorkerJobStatus::Running
    } else {
        WorkerJobStatus::Queued
    };
    job.finished_at_ms = None;
    job.error = None;
    append_event(
        job,
        "cancel_rejected".to_string(),
        serde_json::json!({"error": error}),
    );
    true
}

pub fn register_artifact(
    request_id: &str,
    name: impl Into<String>,
    path: impl AsRef<Path>,
    media_type: Option<String>,
) -> Result<WorkerArtifactRecord, String> {
    let canonical = path
        .as_ref()
        .canonicalize()
        .map_err(|error| format!("canonicalize worker artifact: {error}"))?;
    let metadata = std::fs::metadata(&canonical)
        .map_err(|error| format!("read worker artifact metadata: {error}"))?;
    if !metadata.is_file() {
        return Err("worker artifact must be a regular file".to_string());
    }
    let artifact = WorkerArtifactRecord {
        artifact_id: uuid::Uuid::new_v4().to_string(),
        name: name.into(),
        size_bytes: metadata.len(),
        media_type,
        created_at_ms: worker_now_ms(),
        path: canonical,
    };
    let mut state = registry().lock();
    let Some(job) = state.jobs.get_mut(request_id) else {
        return Err(format!("worker job '{request_id}' not found"));
    };
    if job.artifacts.len() >= MAX_ARTIFACTS_PER_JOB {
        return Err(format!(
            "worker job artifact limit reached ({MAX_ARTIFACTS_PER_JOB})"
        ));
    }
    job.artifacts.push(artifact.clone());
    Ok(artifact)
}

pub fn artifact(id: &str, artifact_id: &str) -> Option<WorkerArtifactRecord> {
    registry()
        .lock()
        .jobs
        .get(id)?
        .artifacts
        .iter()
        .find(|artifact| artifact.artifact_id == artifact_id)
        .cloned()
}

fn append_event(job: &mut WorkerJobRecord, event: String, payload: serde_json::Value) {
    if job.events.len() >= MAX_EVENTS_PER_JOB {
        job.events.remove(0);
    }
    job.events.push(WorkerJobEventRecord {
        timestamp_ms: worker_now_ms(),
        event,
        payload: bounded_payload(&payload),
    });
}

fn update_status(request_id: &str, status: WorkerJobStatus, error: Option<String>) {
    let mut state = registry().lock();
    let Some(job) = state.jobs.get_mut(request_id) else {
        return;
    };
    let now = worker_now_ms();
    if status == WorkerJobStatus::Running && job.started_at_ms.is_none() {
        job.started_at_ms = Some(now);
    }
    if status.is_terminal() {
        job.finished_at_ms = Some(now);
    }
    job.status = status;
    job.error = error;
}

fn prune_history(state: &mut RegistryState) {
    while state.jobs.len() >= MAX_JOB_HISTORY {
        let removable_index = state.order.iter().position(|id| {
            state
                .jobs
                .get(id)
                .is_none_or(|job| job.status.is_terminal())
        });
        let removed = removable_index
            .and_then(|index| state.order.remove(index))
            .or_else(|| state.order.pop_front());
        let Some(id) = removed else {
            break;
        };
        if let Some(job) = state.jobs.remove(&id) {
            state.request_to_job.remove(&job.request_id);
        }
    }
}

fn bounded_payload(payload: &serde_json::Value) -> serde_json::Value {
    match serde_json::to_vec(payload) {
        Ok(bytes) if bytes.len() <= MAX_EVENT_PAYLOAD_BYTES => payload.clone(),
        Ok(bytes) => serde_json::json!({
            "truncated": true,
            "originalBytes": bytes.len(),
        }),
        Err(error) => serde_json::json!({
            "truncated": true,
            "serializationError": error.to_string(),
        }),
    }
}

#[cfg(test)]
pub(crate) fn clear_for_tests() {
    *registry().lock() = RegistryState::default();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_lifecycle_events_and_bounded_payloads() {
        clear_for_tests();
        begin_request(
            "asr:offline-jobs",
            WorkerKind::Asr,
            "request-1",
            Some("task-1"),
            "asr.run_directory_task",
        );
        mark_running("request-1");
        record_event(&WorkerEvent {
            request_id: Some("request-1".to_string()),
            job_id: Some("task-1".to_string()),
            event: "progress".to_string(),
            payload: serde_json::json!({"content": "x".repeat(MAX_EVENT_PAYLOAD_BYTES)}),
        });
        mark_succeeded("request-1");

        let job = get_job("request-1").unwrap();
        assert_eq!(job.status, WorkerJobStatus::Succeeded);
        assert!(job.started_at_ms.is_some());
        assert!(job.finished_at_ms.is_some());
        assert_eq!(job.events.len(), 1);
        assert_eq!(job.events[0].payload["truncated"], true);
    }

    #[test]
    fn registers_only_explicit_regular_file_artifacts() {
        clear_for_tests();
        begin_request(
            "external_cli:request-3",
            WorkerKind::ExternalCli,
            "request-3",
            Some("session-3"),
            "external_cli.run",
        );
        let temp = tempfile::tempdir().unwrap();
        let artifact_path = temp.path().join("stdout.log");
        std::fs::write(&artifact_path, b"hello worker artifact").unwrap();
        let artifact = register_artifact(
            "request-3",
            "stdout",
            &artifact_path,
            Some("text/plain".to_string()),
        )
        .unwrap();
        assert_eq!(artifact.size_bytes, 21);
        assert_eq!(get_job("request-3").unwrap().artifacts.len(), 1);
        assert!(register_artifact("request-3", "directory", temp.path(), None).is_err());
    }

    #[test]
    fn cancel_target_uses_logical_job_id() {
        clear_for_tests();
        begin_request(
            "remote_execution:call-1",
            WorkerKind::RemoteExecution,
            "request-2",
            Some("call-1"),
            "remote.execute",
        );
        assert_eq!(
            cancel_target("request-2"),
            Some(("remote_execution:call-1".to_string(), "call-1".to_string()))
        );
        assert_eq!(
            get_job("request-2").unwrap().status,
            WorkerJobStatus::Cancelling
        );
    }

    #[test]
    fn rejected_cancel_restores_running_job_and_records_reason() {
        clear_for_tests();
        begin_request(
            "remote_execution:call-running",
            WorkerKind::RemoteExecution,
            "request-running",
            Some("call-running"),
            "remote.execute",
        );
        mark_running("request-running");
        assert!(cancel_target("request-running").is_some());
        assert!(cancel_rejected("request-running", "worker disappeared"));

        let job = get_job("request-running").unwrap();
        assert_eq!(job.status, WorkerJobStatus::Running);
        assert!(job.finished_at_ms.is_none());
        assert_eq!(job.events.last().unwrap().event, "cancel_rejected");
        assert_eq!(
            job.events.last().unwrap().payload["error"],
            "worker disappeared"
        );
    }

    #[test]
    fn rejected_cancel_restores_queued_job_but_never_revives_terminal_job() {
        clear_for_tests();
        begin_request(
            "asr:offline-jobs",
            WorkerKind::Asr,
            "request-queued",
            Some("task-queued"),
            "asr.run_directory_task",
        );
        assert!(cancel_target("request-queued").is_some());
        assert!(cancel_rejected("request-queued", "worker unavailable"));
        assert_eq!(
            get_job("request-queued").unwrap().status,
            WorkerJobStatus::Queued
        );

        mark_succeeded("request-queued");
        assert!(!cancel_rejected("request-queued", "late rejection"));
        assert_eq!(
            get_job("request-queued").unwrap().status,
            WorkerJobStatus::Succeeded
        );
    }
}
