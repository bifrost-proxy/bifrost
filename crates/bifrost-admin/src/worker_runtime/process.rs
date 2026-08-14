use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use dashmap::{mapref::entry::Entry, DashMap};
use once_cell::sync::Lazy;
use sysinfo::{Pid, ProcessesToUpdate, System};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, Command};
use tokio::sync::{broadcast, mpsc, oneshot, watch, Mutex, Semaphore};
use tracing::warn;

use super::jobs;
use super::protocol::{
    now_ms, parse_worker_frame, read_limited_async_line, serialize_frame, ParentFrame, WorkerEvent,
    WorkerFrame, WorkerHello, WorkerKind, WorkerLifecycleState, WorkerRequest, WorkerResponse,
    WORKER_MAX_FRAME_BYTES, WORKER_PROTOCOL_VERSION,
};

pub const WORKER_STARTUP_TOKEN_ENV: &str = "BIFROST_WORKER_STARTUP_TOKEN";
pub const WORKER_KIND_ENV: &str = "BIFROST_WORKER_KIND";
const WORKER_STDERR_MAX_BYTES: u64 = 32 * 1024 * 1024;
const WORKER_STDERR_READ_BYTES: usize = 16 * 1024;

static WORKER_PROCESS_SYSTEM: Lazy<parking_lot::Mutex<System>> =
    Lazy::new(|| parking_lot::Mutex::new(System::new_all()));

#[derive(Debug, Clone)]
pub struct WorkerSpawnSpec {
    pub key: String,
    pub kind: WorkerKind,
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub env_remove: Vec<String>,
    pub startup_timeout: Duration,
    pub request_timeout: Duration,
    pub heartbeat_timeout: Duration,
    pub max_concurrency: usize,
    pub max_queue_depth: usize,
    pub queue_wait_timeout: Duration,
    pub stderr_path: Option<PathBuf>,
}

impl WorkerSpawnSpec {
    pub fn new(
        key: impl Into<String>,
        kind: WorkerKind,
        executable: impl Into<PathBuf>,
        args: Vec<String>,
    ) -> Self {
        Self {
            key: key.into(),
            kind,
            executable: executable.into(),
            args,
            env: BTreeMap::new(),
            env_remove: Vec::new(),
            startup_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(60),
            heartbeat_timeout: Duration::from_secs(35),
            max_concurrency: 1,
            max_queue_depth: 32,
            queue_wait_timeout: Duration::from_secs(30),
            stderr_path: None,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ManagedWorkerSnapshot {
    pub key: String,
    pub worker_kind: WorkerKind,
    pub state: WorkerLifecycleState,
    pub pid: Option<u32>,
    pub worker_instance_id: Option<String>,
    pub last_heartbeat_ms: Option<u64>,
    pub worker_reported_heartbeat_ms: Option<u64>,
    pub heartbeat_age_ms: Option<u64>,
    pub started_at_ms: u64,
    pub restart_count: u64,
    pub active_jobs: usize,
    pub queued_jobs: usize,
    pub max_concurrency: usize,
    pub max_queue_depth: usize,
    pub rss_bytes: Option<u64>,
    pub virtual_memory_bytes: Option<u64>,
    pub cpu_usage_percent: Option<f32>,
    pub open_file_descriptors: Option<usize>,
    pub backoff_until_ms: Option<u64>,
    pub circuit_open_until_ms: Option<u64>,
    pub last_error: Option<String>,
}

pub struct ManagedWorker {
    key: String,
    kind: WorkerKind,
    hello: WorkerHello,
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    pending: DashMap<String, oneshot::Sender<WorkerResponse>>,
    request_cancellations: DashMap<String, watch::Sender<bool>>,
    dispatched_requests: DashMap<String, ()>,
    request_event_sinks: DashMap<String, mpsc::Sender<WorkerEvent>>,
    events: broadcast::Sender<WorkerEvent>,
    state: AtomicU8,
    last_heartbeat_ms: AtomicU64,
    worker_reported_heartbeat_ms: AtomicU64,
    active_jobs: AtomicUsize,
    queued_jobs: AtomicUsize,
    parent_queued_jobs: AtomicUsize,
    started_at_ms: u64,
    request_timeout: Duration,
    heartbeat_timeout: Duration,
    queue_wait_timeout: Duration,
    max_concurrency: usize,
    max_queue_depth: usize,
    request_slots: Arc<Semaphore>,
    control_slots: Arc<Semaphore>,
    last_error: parking_lot::RwLock<Option<String>>,
}

struct AtomicCounterGuard<'a> {
    counter: &'a AtomicUsize,
}
impl<'a> AtomicCounterGuard<'a> {
    fn try_increment_below(counter: &'a AtomicUsize, limit: usize) -> Option<Self> {
        let mut current = counter.load(Ordering::Acquire);
        loop {
            if current >= limit {
                return None;
            }
            match counter.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(Self { counter }),
                Err(actual) => current = actual,
            }
        }
    }
}
impl Drop for AtomicCounterGuard<'_> {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::AcqRel);
    }
}

struct RequestTrackingGuard<'a> {
    request_id: String,
    cancellations: &'a DashMap<String, watch::Sender<bool>>,
    dispatched: &'a DashMap<String, ()>,
}

impl Drop for RequestTrackingGuard<'_> {
    fn drop(&mut self) {
        self.cancellations.remove(&self.request_id);
        self.dispatched.remove(&self.request_id);
    }
}

impl ManagedWorker {
    pub async fn spawn(spec: WorkerSpawnSpec) -> Result<Arc<Self>, String> {
        let startup_token = uuid::Uuid::new_v4().to_string();
        let mut command = Command::new(&spec.executable);
        command
            .args(&spec.args)
            .env(WORKER_STARTUP_TOKEN_ENV, &startup_token)
            .env(WORKER_KIND_ENV, spec.kind.as_str())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true);
        for (key, value) in &spec.env {
            command.env(key, value);
        }
        for key in &spec.env_remove {
            command.env_remove(key);
        }
        configure_process_group(&mut command);
        command.stderr(if spec.stderr_path.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });

        let mut child = command.spawn().map_err(|error| {
            format!(
                "spawn {} worker '{}' failed: {error}",
                spec.kind.as_str(),
                spec.key
            )
        })?;
        let pid = child.id();
        if let (Some(stderr), Some(path)) = (child.stderr.take(), spec.stderr_path.clone()) {
            spawn_stderr_logger(stderr, path);
        }
        let startup_result = async {
            let stdin = child
                .stdin
                .take()
                .ok_or_else(|| "worker stdin unavailable".to_string())?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| "worker stdout unavailable".to_string())?;
            let mut stdout = BufReader::new(stdout);
            let deadline = tokio::time::Instant::now() + spec.startup_timeout;
            let hello_line = tokio::time::timeout_at(
                deadline,
                read_limited_async_line(&mut stdout, WORKER_MAX_FRAME_BYTES),
            )
            .await
            .map_err(|_| "worker hello timeout".to_string())?
            .map_err(|error| format!("read worker hello failed: {error}"))?
            .ok_or_else(|| "worker exited before hello".to_string())?;
            let hello = match parse_worker_frame(&hello_line)? {
                WorkerFrame::Hello { hello } => hello,
                other => return Err(format!("first worker frame must be hello, got {other:?}")),
            };
            validate_hello(&hello, &spec, &startup_token, pid)?;
            let ready_line = tokio::time::timeout_at(
                deadline,
                read_limited_async_line(&mut stdout, WORKER_MAX_FRAME_BYTES),
            )
            .await
            .map_err(|_| "worker ready timeout".to_string())?
            .map_err(|error| format!("read worker ready failed: {error}"))?
            .ok_or_else(|| "worker exited before ready".to_string())?;
            match parse_worker_frame(&ready_line)? {
                WorkerFrame::Ready { worker_instance_id }
                    if worker_instance_id == hello.worker_instance_id => {}
                other => return Err(format!("second worker frame must be ready, got {other:?}")),
            }
            Ok::<_, String>((stdin, stdout, hello))
        }
        .await;
        let (stdin, stdout, hello) = match startup_result {
            Ok(value) => value,
            Err(error) => {
                terminate_child_tree(&mut child).await;
                return Err(error);
            }
        };

        let (events, _) = broadcast::channel(128);
        let worker = Arc::new(Self {
            key: spec.key,
            kind: spec.kind,
            hello,
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            pending: DashMap::new(),
            request_cancellations: DashMap::new(),
            dispatched_requests: DashMap::new(),
            request_event_sinks: DashMap::new(),
            events,
            state: AtomicU8::new(state_to_u8(WorkerLifecycleState::Ready)),
            last_heartbeat_ms: AtomicU64::new(now_ms()),
            worker_reported_heartbeat_ms: AtomicU64::new(0),
            active_jobs: AtomicUsize::new(0),
            queued_jobs: AtomicUsize::new(0),
            parent_queued_jobs: AtomicUsize::new(0),
            started_at_ms: now_ms(),
            request_timeout: spec.request_timeout,
            heartbeat_timeout: spec.heartbeat_timeout,
            queue_wait_timeout: spec.queue_wait_timeout,
            max_concurrency: spec.max_concurrency.max(1),
            max_queue_depth: spec.max_queue_depth,
            request_slots: Arc::new(Semaphore::new(spec.max_concurrency.max(1))),
            control_slots: Arc::new(Semaphore::new(8)),
            last_error: parking_lot::RwLock::new(None),
        });
        Self::spawn_stdout_reader(&worker, stdout);
        Self::spawn_health_monitor(&worker);
        Ok(worker)
    }

    fn spawn_stdout_reader(worker: &Arc<Self>, mut stdout: BufReader<tokio::process::ChildStdout>) {
        let worker = Arc::downgrade(worker);
        tokio::spawn(async move {
            let mut unexpected = false;
            loop {
                let line = match read_limited_async_line(&mut stdout, WORKER_MAX_FRAME_BYTES).await
                {
                    Ok(Some(line)) => line,
                    Ok(None) => {
                        unexpected = true;
                        break;
                    }
                    Err(error) => {
                        if let Some(worker) = worker.upgrade() {
                            worker.mark_failed(format!("read worker stdout failed: {error}"));
                        }
                        unexpected = true;
                        break;
                    }
                };
                let Some(worker) = worker.upgrade() else {
                    break;
                };
                match parse_worker_frame(&line) {
                    Ok(WorkerFrame::Heartbeat { heartbeat }) => {
                        if heartbeat.worker_instance_id == worker.hello.worker_instance_id {
                            worker.last_heartbeat_ms.store(now_ms(), Ordering::Release);
                            worker
                                .worker_reported_heartbeat_ms
                                .store(heartbeat.timestamp_ms, Ordering::Release);
                            worker
                                .active_jobs
                                .store(heartbeat.active_jobs, Ordering::Release);
                            worker
                                .queued_jobs
                                .store(heartbeat.queued_jobs, Ordering::Release);
                            if !matches!(
                                worker.state(),
                                WorkerLifecycleState::Stopping | WorkerLifecycleState::Stopped
                            ) {
                                worker.state.store(
                                    state_to_u8(if heartbeat.active_jobs > 0 {
                                        WorkerLifecycleState::Busy
                                    } else {
                                        WorkerLifecycleState::Ready
                                    }),
                                    Ordering::Release,
                                );
                            }
                        }
                    }
                    Ok(WorkerFrame::Response { response }) => {
                        if let Some((_, sender)) = worker.pending.remove(&response.request_id) {
                            let _ = sender.send(response);
                        }
                    }
                    Ok(WorkerFrame::Event { event }) => {
                        if let Some(request_id) = event.request_id.as_deref() {
                            let sink = worker
                                .request_event_sinks
                                .get(request_id)
                                .map(|entry| entry.clone());
                            if let Some(sink) = sink {
                                let _ = sink.send(event.clone()).await;
                            }
                        }
                        jobs::record_event(&event);
                        let _ = worker.events.send(event);
                    }
                    Ok(WorkerFrame::Goodbye { reason, .. }) => {
                        worker.mark_stopped(reason);
                        break;
                    }
                    Ok(WorkerFrame::Ready { .. } | WorkerFrame::ConfigApplied { .. }) => {}
                    Ok(WorkerFrame::Hello { .. }) => {
                        worker.mark_failed("duplicate worker hello".to_string());
                        unexpected = true;
                        break;
                    }
                    Err(error) => {
                        worker.mark_failed(error);
                        unexpected = true;
                        break;
                    }
                }
            }
            if let Some(worker) = worker.upgrade() {
                if !matches!(
                    worker.state(),
                    WorkerLifecycleState::Stopping | WorkerLifecycleState::Stopped
                ) {
                    worker.mark_failed("worker exited unexpectedly".to_string());
                    unexpected = true;
                }
                if unexpected {
                    let mut child = worker.child.lock().await;
                    terminate_child_tree(&mut child).await;
                }
            }
        });
    }

    fn spawn_health_monitor(worker: &Arc<Self>) {
        let worker = Arc::downgrade(worker);
        tokio::spawn(async move {
            let Some(initial) = worker.upgrade() else {
                return;
            };
            let timeout = initial.heartbeat_timeout;
            drop(initial);
            let interval_duration = timeout
                .checked_div(3)
                .unwrap_or(Duration::from_secs(1))
                .clamp(Duration::from_millis(250), Duration::from_secs(5));
            let mut interval = tokio::time::interval(interval_duration);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            interval.tick().await;
            loop {
                interval.tick().await;
                let Some(worker) = worker.upgrade() else {
                    break;
                };
                if matches!(
                    worker.state(),
                    WorkerLifecycleState::Stopping | WorkerLifecycleState::Stopped
                ) {
                    break;
                }
                if worker.heartbeat_age() > timeout {
                    worker.mark_failed(format!(
                        "worker heartbeat timed out after {:?}",
                        worker.heartbeat_age()
                    ));
                    let mut child = worker.child.lock().await;
                    terminate_child_tree(&mut child).await;
                    break;
                }
                if let Ok(mut child) = worker.child.try_lock() {
                    if let Ok(Some(status)) = child.try_wait() {
                        worker.mark_failed(format!("worker exited with status {status}"));
                        break;
                    }
                };
            }
        });
    }

    pub fn key(&self) -> &str {
        &self.key
    }
    pub fn kind(&self) -> WorkerKind {
        self.kind
    }
    pub fn instance_id(&self) -> &str {
        &self.hello.worker_instance_id
    }
    pub fn pid(&self) -> Option<u32> {
        self.child.try_lock().ok().and_then(|child| child.id())
    }
    pub fn state(&self) -> WorkerLifecycleState {
        u8_to_state(self.state.load(Ordering::Acquire))
    }
    pub fn subscribe_events(&self) -> broadcast::Receiver<WorkerEvent> {
        self.events.subscribe()
    }

    pub fn subscribe_request_events(
        &self,
        request_id: impl Into<String>,
        capacity: usize,
    ) -> mpsc::Receiver<WorkerEvent> {
        let request_id = request_id.into();
        let (sender, receiver) = mpsc::channel(capacity.max(1));
        self.request_event_sinks.insert(request_id, sender);
        receiver
    }

    pub fn remove_request_event_sink(&self, request_id: &str) {
        self.request_event_sinks.remove(request_id);
    }

    pub async fn is_healthy(&self) -> bool {
        if !matches!(
            self.state(),
            WorkerLifecycleState::Ready | WorkerLifecycleState::Busy
        ) || self.heartbeat_age() > self.heartbeat_timeout
        {
            return false;
        }
        let mut child = self.child.lock().await;
        matches!(child.try_wait(), Ok(None))
    }

    pub async fn request(
        &self,
        operation: impl Into<String>,
        payload: serde_json::Value,
        timeout_override: Option<Duration>,
    ) -> Result<serde_json::Value, String> {
        self.request_with_id_inner(
            uuid::Uuid::new_v4().to_string(),
            None,
            operation.into(),
            payload,
            timeout_override,
            false,
        )
        .await
    }

    /// Send a low-volume control request without waiting behind the worker's
    /// primary execution semaphore. This is reserved for operations such as
    /// status/cancel controls that must remain reachable while a long-running
    /// request occupies the worker's normal slot.
    pub async fn request_control(
        &self,
        operation: impl Into<String>,
        payload: serde_json::Value,
        timeout_override: Option<Duration>,
    ) -> Result<serde_json::Value, String> {
        self.request_with_id_inner(
            uuid::Uuid::new_v4().to_string(),
            None,
            operation.into(),
            payload,
            timeout_override,
            true,
        )
        .await
    }

    pub async fn request_with_id(
        &self,
        request_id: String,
        job_id: Option<String>,
        operation: impl Into<String>,
        payload: serde_json::Value,
        timeout_override: Option<Duration>,
    ) -> Result<serde_json::Value, String> {
        self.request_with_id_inner(
            request_id,
            job_id,
            operation.into(),
            payload,
            timeout_override,
            false,
        )
        .await
    }

    async fn request_with_id_inner(
        &self,
        request_id: String,
        job_id: Option<String>,
        operation: String,
        payload: serde_json::Value,
        timeout_override: Option<Duration>,
        control_lane: bool,
    ) -> Result<serde_json::Value, String> {
        jobs::begin_request(
            &self.key,
            self.kind,
            &request_id,
            job_id.as_deref(),
            &operation,
        );
        if !matches!(
            self.state(),
            WorkerLifecycleState::Ready | WorkerLifecycleState::Busy
        ) {
            let error = format!("{} worker '{}' is not ready", self.kind.as_str(), self.key);
            jobs::mark_failed(&request_id, error.clone());
            return Err(error);
        }

        let (cancel_tx, mut cancel_rx) = watch::channel(false);
        self.request_cancellations
            .insert(request_id.clone(), cancel_tx);
        let _tracking = RequestTrackingGuard {
            request_id: request_id.clone(),
            cancellations: &self.request_cancellations,
            dispatched: &self.dispatched_requests,
        };

        let permit = if control_lane {
            let acquire = self.control_slots.clone().acquire_owned();
            tokio::pin!(acquire);
            let queue_timeout = tokio::time::sleep(Duration::from_secs(5));
            tokio::pin!(queue_timeout);
            tokio::select! {
                result = &mut acquire => result.map_err(|_| "worker control queue closed".to_string())?,
                _ = &mut queue_timeout => {
                    let error = format!("worker '{}' control queue timeout", self.key);
                    jobs::mark_failed(&request_id, error.clone());
                    return Err(error);
                },
                changed = cancel_rx.changed() => {
                    if changed.is_ok() && *cancel_rx.borrow() {
                        jobs::mark_cancelled(
                            &request_id,
                            Some("worker control request cancelled while queued".to_string()),
                        );
                        return Err("worker control request cancelled while queued".to_string());
                    }
                    let error = "worker control request cancellation channel closed".to_string();
                    jobs::mark_failed(&request_id, error.clone());
                    return Err(error);
                },
            }
        } else {
            match self.request_slots.clone().try_acquire_owned() {
                Ok(permit) => permit,
                Err(_) => {
                    let Some(queue_guard) = AtomicCounterGuard::try_increment_below(
                        &self.parent_queued_jobs,
                        self.max_queue_depth,
                    ) else {
                        let error = format!(
                            "worker '{}' request queue is full (limit {})",
                            self.key, self.max_queue_depth
                        );
                        jobs::mark_failed(&request_id, error.clone());
                        return Err(error);
                    };
                    let acquire = self.request_slots.clone().acquire_owned();
                    tokio::pin!(acquire);
                    let queue_timeout = tokio::time::sleep(self.queue_wait_timeout);
                    tokio::pin!(queue_timeout);
                    let permit = tokio::select! {
                        result = &mut acquire => match result {
                            Ok(permit) => permit,
                            Err(_) => {
                                let error = "worker request queue closed".to_string();
                                jobs::mark_failed(&request_id, error.clone());
                                return Err(error);
                            }
                        },
                        _ = &mut queue_timeout => {
                            let error = format!("worker '{}' request queue timeout", self.key);
                            jobs::mark_failed(&request_id, error.clone());
                            return Err(error);
                        },
                        changed = cancel_rx.changed() => {
                            if changed.is_ok() && *cancel_rx.borrow() {
                                jobs::mark_cancelled(
                                    &request_id,
                                    Some("worker request cancelled while queued".to_string()),
                                );
                                return Err("worker request cancelled while queued".to_string());
                            }
                            let error = "worker request cancellation channel closed".to_string();
                            jobs::mark_failed(&request_id, error.clone());
                            return Err(error);
                        },
                    };
                    drop(queue_guard);
                    permit
                }
            }
        };
        let _permit = permit;
        if *cancel_rx.borrow() {
            jobs::mark_cancelled(
                &request_id,
                Some("worker request cancelled before dispatch".to_string()),
            );
            return Err("worker request cancelled before dispatch".to_string());
        }

        jobs::mark_running(&request_id);
        let timeout = timeout_override.unwrap_or(self.request_timeout);
        let job_id_for_cancel = job_id.clone();
        let request = WorkerRequest {
            request_id: request_id.clone(),
            job_id,
            deadline_unix_ms: Some(now_ms().saturating_add(timeout.as_millis() as u64)),
            operation,
            payload,
        };
        let (sender, receiver) = oneshot::channel();
        match self.pending.entry(request_id.clone()) {
            Entry::Vacant(entry) => {
                entry.insert(sender);
            }
            Entry::Occupied(_) => {
                let error = format!("duplicate worker request id '{request_id}'");
                jobs::mark_failed(&request_id, error.clone());
                return Err(error);
            }
        }
        if *cancel_rx.borrow() {
            self.pending.remove(&request_id);
            jobs::mark_cancelled(
                &request_id,
                Some("worker request cancelled before dispatch".to_string()),
            );
            return Err("worker request cancelled before dispatch".to_string());
        }
        if let Err(error) = self
            .write_parent_frame(&ParentFrame::Request { request })
            .await
        {
            self.pending.remove(&request_id);
            jobs::mark_failed(&request_id, error.clone());
            self.mark_failed(error.clone());
            return Err(error);
        }
        self.dispatched_requests.insert(request_id.clone(), ());
        if *cancel_rx.borrow() {
            let _ = self
                .write_parent_frame(&ParentFrame::Cancel {
                    request_id: request_id.clone(),
                    job_id: job_id_for_cancel.clone(),
                })
                .await;
        }

        let response = match tokio::time::timeout(timeout, receiver).await {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => {
                let error = "worker response channel closed".to_string();
                jobs::mark_failed(&request_id, error.clone());
                return Err(error);
            }
            Err(_) => {
                self.pending.remove(&request_id);
                let _ = self
                    .write_parent_frame(&ParentFrame::Cancel {
                        request_id: request_id.clone(),
                        job_id: job_id_for_cancel,
                    })
                    .await;
                let error = format!("worker request '{request_id}' timed out after {timeout:?}");
                jobs::mark_failed(&request_id, error.clone());
                return Err(error);
            }
        };
        if response.cancelled {
            let error = response
                .error
                .unwrap_or_else(|| "worker request cancelled".to_string());
            jobs::mark_cancelled(&request_id, Some(error.clone()));
            return Err(error);
        }
        if response.ok {
            jobs::mark_succeeded(&request_id);
            Ok(response.payload)
        } else {
            let error = response
                .error
                .unwrap_or_else(|| "worker request failed".to_string());
            jobs::mark_failed(&request_id, error.clone());
            Err(error)
        }
    }

    pub async fn cancel_request(
        &self,
        request_id: &str,
        logical_job_id: &str,
    ) -> Result<bool, String> {
        let Some(sender) = self
            .request_cancellations
            .get(request_id)
            .map(|entry| entry.clone())
        else {
            return Ok(false);
        };
        let _ = sender.send(true);
        if self.dispatched_requests.contains_key(request_id) {
            self.write_parent_frame(&ParentFrame::Cancel {
                request_id: request_id.to_string(),
                job_id: Some(logical_job_id.to_string()),
            })
            .await?;
        }
        Ok(true)
    }

    pub async fn cancel_job(&self, job_id: impl Into<String>) -> Result<(), String> {
        let job_id = job_id.into();
        jobs::mark_logical_job_cancelling(&self.key, &job_id);
        let request_ids = jobs::active_request_ids(&self.key, &job_id);
        for request_id in request_ids {
            let _ = self.cancel_request(&request_id, &job_id).await?;
        }
        Ok(())
    }

    pub async fn shutdown(&self, grace: Duration) -> Result<(), String> {
        if self.state() == WorkerLifecycleState::Stopped {
            return Ok(());
        }
        self.state.store(
            state_to_u8(WorkerLifecycleState::Stopping),
            Ordering::Release,
        );
        let _ = self
            .write_parent_frame(&ParentFrame::Shutdown {
                request_id: uuid::Uuid::new_v4().to_string(),
            })
            .await;
        let mut child = self.child.lock().await;
        match tokio::time::timeout(grace, child.wait()).await {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => return Err(format!("wait worker failed: {error}")),
            Err(_) => terminate_child_tree(&mut child).await,
        }
        self.state.store(
            state_to_u8(WorkerLifecycleState::Stopped),
            Ordering::Release,
        );
        self.fail_pending("worker stopped");
        Ok(())
    }

    pub fn heartbeat_age(&self) -> Duration {
        Duration::from_millis(
            now_ms().saturating_sub(self.last_heartbeat_ms.load(Ordering::Acquire)),
        )
    }
    pub fn snapshot(
        &self,
        restart_count: u64,
        backoff_until_ms: Option<u64>,
        circuit_open_until_ms: Option<u64>,
    ) -> ManagedWorkerSnapshot {
        let metrics = self.pid().and_then(process_metrics);
        ManagedWorkerSnapshot {
            key: self.key.clone(),
            worker_kind: self.kind,
            state: self.state(),
            pid: self.pid(),
            worker_instance_id: Some(self.hello.worker_instance_id.clone()),
            last_heartbeat_ms: Some(self.last_heartbeat_ms.load(Ordering::Acquire)),
            worker_reported_heartbeat_ms: Some(
                self.worker_reported_heartbeat_ms.load(Ordering::Acquire),
            )
            .filter(|timestamp| *timestamp > 0),
            heartbeat_age_ms: Some(self.heartbeat_age().as_millis() as u64),
            started_at_ms: self.started_at_ms,
            restart_count,
            active_jobs: self.active_jobs.load(Ordering::Acquire),
            queued_jobs: self.queued_jobs.load(Ordering::Acquire)
                + self.parent_queued_jobs.load(Ordering::Acquire),
            max_concurrency: self.max_concurrency,
            max_queue_depth: self.max_queue_depth,
            rss_bytes: metrics.as_ref().map(|metrics| metrics.rss_bytes),
            virtual_memory_bytes: metrics.as_ref().map(|metrics| metrics.virtual_memory_bytes),
            cpu_usage_percent: metrics.as_ref().map(|metrics| metrics.cpu_usage_percent),
            open_file_descriptors: metrics.and_then(|metrics| metrics.open_file_descriptors),
            backoff_until_ms,
            circuit_open_until_ms,
            last_error: self.last_error.read().clone(),
        }
    }

    async fn write_parent_frame(&self, frame: &ParentFrame) -> Result<(), String> {
        let line = serialize_frame(frame)?;
        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        stdin.write_all(b"\n").await.map_err(|e| e.to_string())?;
        stdin.flush().await.map_err(|e| e.to_string())
    }
    fn mark_failed(&self, error: String) {
        jobs::fail_worker_jobs(&self.key, &error);
        *self.last_error.write() = Some(error);
        self.state.store(
            state_to_u8(WorkerLifecycleState::Degraded),
            Ordering::Release,
        );
        self.fail_pending("worker failed");
    }
    fn mark_stopped(&self, reason: Option<String>) {
        jobs::fail_worker_jobs(&self.key, reason.as_deref().unwrap_or("worker stopped"));
        *self.last_error.write() = reason;
        self.state.store(
            state_to_u8(WorkerLifecycleState::Stopped),
            Ordering::Release,
        );
        self.fail_pending("worker stopped");
    }
    fn fail_pending(&self, message: &str) {
        let keys = self
            .pending
            .iter()
            .map(|entry| entry.key().clone())
            .collect::<Vec<_>>();
        for key in keys {
            if let Some((_, sender)) = self.pending.remove(&key) {
                let _ = sender.send(WorkerResponse {
                    request_id: key,
                    ok: false,
                    cancelled: false,
                    payload: serde_json::Value::Null,
                    error: Some(message.to_string()),
                });
            }
        }
    }
}

fn validate_hello(
    hello: &WorkerHello,
    spec: &WorkerSpawnSpec,
    startup_token: &str,
    pid: Option<u32>,
) -> Result<(), String> {
    if hello.protocol_version != WORKER_PROTOCOL_VERSION
        || hello.worker_kind != spec.kind
        || hello.startup_token != startup_token
        || pid.is_some_and(|pid| pid != hello.pid)
        || hello.worker_instance_id.trim().is_empty()
    {
        return Err("worker hello validation failed".to_string());
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct WorkerProcessMetrics {
    rss_bytes: u64,
    virtual_memory_bytes: u64,
    cpu_usage_percent: f32,
    open_file_descriptors: Option<usize>,
}

fn process_metrics(pid: u32) -> Option<WorkerProcessMetrics> {
    let pid_value = Pid::from_u32(pid);
    let mut system = WORKER_PROCESS_SYSTEM.lock();
    system.refresh_processes(ProcessesToUpdate::Some(&[pid_value]));
    let process = system.process(pid_value)?;
    Some(WorkerProcessMetrics {
        rss_bytes: process.memory(),
        virtual_memory_bytes: process.virtual_memory(),
        cpu_usage_percent: process.cpu_usage(),
        open_file_descriptors: open_file_descriptor_count(pid),
    })
}

#[cfg(target_os = "linux")]
fn open_file_descriptor_count(pid: u32) -> Option<usize> {
    std::fs::read_dir(format!("/proc/{pid}/fd"))
        .ok()
        .map(|entries| entries.filter_map(Result::ok).count())
}

#[cfg(not(target_os = "linux"))]
fn open_file_descriptor_count(_: u32) -> Option<usize> {
    None
}

fn spawn_stderr_logger(stderr: ChildStderr, path: PathBuf) {
    tokio::spawn(async move {
        if let Err(error) = run_stderr_logger(stderr, &path, WORKER_STDERR_MAX_BYTES).await {
            warn!(path = %path.display(), error, "worker stderr logger stopped");
        }
    });
}

async fn run_stderr_logger(
    mut stderr: ChildStderr,
    path: &Path,
    max_bytes: u64,
) -> Result<(), String> {
    let (mut file, mut current_bytes) = open_bounded_log(path, max_bytes).await?;
    let mut buffer = vec![0u8; WORKER_STDERR_READ_BYTES];
    loop {
        let read = stderr
            .read(&mut buffer)
            .await
            .map_err(|error| format!("read worker stderr failed: {error}"))?;
        if read == 0 {
            file.flush().await.map_err(|error| error.to_string())?;
            return Ok(());
        }
        write_bounded_log_chunk(
            path,
            &mut file,
            &mut current_bytes,
            &buffer[..read],
            max_bytes,
        )
        .await?;
    }
}

async fn open_bounded_log(path: &Path, max_bytes: u64) -> Result<(tokio::fs::File, u64), String> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| error.to_string())?;
    }
    let mut current_bytes = tokio::fs::metadata(path)
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if current_bytes >= max_bytes {
        rotate_log(path).await?;
        current_bytes = 0;
    }
    let file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
        .map_err(|error| error.to_string())?;
    Ok((file, current_bytes))
}

async fn write_bounded_log_chunk(
    path: &Path,
    file: &mut tokio::fs::File,
    current_bytes: &mut u64,
    chunk: &[u8],
    max_bytes: u64,
) -> Result<(), String> {
    if max_bytes == 0 {
        return Ok(());
    }
    if current_bytes.saturating_add(chunk.len() as u64) > max_bytes {
        file.flush().await.map_err(|error| error.to_string())?;
        rotate_log(path).await?;
        *file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)
            .await
            .map_err(|error| error.to_string())?;
        *current_bytes = 0;
    }
    let retained = if chunk.len() as u64 > max_bytes {
        &chunk[chunk.len() - max_bytes as usize..]
    } else {
        chunk
    };
    file.write_all(retained)
        .await
        .map_err(|error| error.to_string())?;
    *current_bytes = current_bytes.saturating_add(retained.len() as u64);
    Ok(())
}

async fn rotate_log(path: &Path) -> Result<(), String> {
    let rotated = path.with_extension("log.1");
    match tokio::fs::remove_file(&rotated).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    match tokio::fs::rename(path, &rotated).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    command.process_group(0);
}
#[cfg(windows)]
fn configure_process_group(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0000_0200);
}
#[cfg(not(any(unix, windows)))]
fn configure_process_group(_: &mut Command) {}

#[cfg(unix)]
async fn terminate_child_tree(child: &mut Child) {
    if let Some(pid) = child.id() {
        let _ = std::process::Command::new("kill")
            .args(["-TERM", &format!("-{pid}")])
            .status();
        tokio::time::sleep(Duration::from_millis(250)).await;
        if child.try_wait().ok().flatten().is_none() {
            let _ = std::process::Command::new("kill")
                .args(["-KILL", &format!("-{pid}")])
                .status();
        }
    }
    let _ = child.start_kill();
    let _ = child.wait().await;
}
#[cfg(windows)]
async fn terminate_child_tree(child: &mut Child) {
    if let Some(pid) = child.id() {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .status();
    }
    let _ = child.start_kill();
    let _ = child.wait().await;
}
#[cfg(not(any(unix, windows)))]
async fn terminate_child_tree(child: &mut Child) {
    let _ = child.start_kill();
    let _ = child.wait().await;
}

fn state_to_u8(state: WorkerLifecycleState) -> u8 {
    state as u8
}
fn u8_to_state(value: u8) -> WorkerLifecycleState {
    match value {
        0 => WorkerLifecycleState::Stopped,
        1 => WorkerLifecycleState::Starting,
        2 => WorkerLifecycleState::Ready,
        3 => WorkerLifecycleState::Busy,
        4 => WorkerLifecycleState::Degraded,
        5 => WorkerLifecycleState::Stopping,
        6 => WorkerLifecycleState::Backoff,
        7 => WorkerLifecycleState::CircuitOpen,
        8 => WorkerLifecycleState::Disabled,
        _ => WorkerLifecycleState::Degraded,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_counter_respects_hard_limit() {
        let counter = AtomicUsize::new(0);
        let first = AtomicCounterGuard::try_increment_below(&counter, 1).unwrap();
        assert!(AtomicCounterGuard::try_increment_below(&counter, 1).is_none());
        drop(first);
        assert!(AtomicCounterGuard::try_increment_below(&counter, 1).is_some());
    }

    #[tokio::test]
    async fn bounded_log_rotates_while_process_is_running() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worker.stderr.log");
        let (mut file, mut current_bytes) = open_bounded_log(&path, 16).await.unwrap();
        write_bounded_log_chunk(&path, &mut file, &mut current_bytes, b"1234567890", 16)
            .await
            .unwrap();
        write_bounded_log_chunk(&path, &mut file, &mut current_bytes, b"abcdefghij", 16)
            .await
            .unwrap();
        file.flush().await.unwrap();

        assert_eq!(tokio::fs::read(&path).await.unwrap(), b"abcdefghij");
        assert_eq!(
            tokio::fs::read(path.with_extension("log.1")).await.unwrap(),
            b"1234567890"
        );
    }

    #[test]
    fn process_metrics_are_available_for_current_process() {
        let metrics = process_metrics(std::process::id()).expect("current process metrics");
        assert!(metrics.rss_bytes > 0);
        assert!(metrics.virtual_memory_bytes > 0);
    }
}
