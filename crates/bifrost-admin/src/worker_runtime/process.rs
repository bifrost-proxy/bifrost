use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use dashmap::{mapref::entry::Entry, DashMap};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{broadcast, oneshot, Mutex, Semaphore};

use super::protocol::{
    now_ms, parse_worker_frame, read_limited_async_line, serialize_frame, ParentFrame, WorkerEvent,
    WorkerFrame, WorkerHello, WorkerKind, WorkerLifecycleState, WorkerRequest, WorkerResponse,
    WORKER_MAX_FRAME_BYTES, WORKER_PROTOCOL_VERSION,
};

pub const WORKER_STARTUP_TOKEN_ENV: &str = "BIFROST_WORKER_STARTUP_TOKEN";
pub const WORKER_KIND_ENV: &str = "BIFROST_WORKER_KIND";

#[derive(Debug, Clone)]
pub struct WorkerSpawnSpec {
    pub key: String,
    pub kind: WorkerKind,
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub startup_timeout: Duration,
    pub request_timeout: Duration,
    pub heartbeat_timeout: Duration,
    pub max_concurrency: usize,
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
            startup_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(60),
            heartbeat_timeout: Duration::from_secs(35),
            max_concurrency: 1,
            queue_wait_timeout: Duration::from_secs(30),
            stderr_path: None,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ManagedWorkerSnapshot {
    pub key: String,
    pub worker_kind: WorkerKind,
    pub state: WorkerLifecycleState,
    pub pid: Option<u32>,
    pub worker_instance_id: Option<String>,
    pub last_heartbeat_ms: Option<u64>,
    pub heartbeat_age_ms: Option<u64>,
    pub started_at_ms: u64,
    pub restart_count: u64,
    pub active_jobs: usize,
    pub queued_jobs: usize,
    pub max_concurrency: usize,
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
    events: broadcast::Sender<WorkerEvent>,
    state: AtomicU8,
    last_heartbeat_ms: AtomicU64,
    active_jobs: AtomicUsize,
    queued_jobs: AtomicUsize,
    parent_queued_jobs: AtomicUsize,
    started_at_ms: u64,
    request_timeout: Duration,
    heartbeat_timeout: Duration,
    queue_wait_timeout: Duration,
    max_concurrency: usize,
    request_slots: Arc<Semaphore>,
    last_error: parking_lot::RwLock<Option<String>>,
}

struct AtomicCounterGuard<'a> {
    counter: &'a AtomicUsize,
}
impl<'a> AtomicCounterGuard<'a> {
    fn increment(counter: &'a AtomicUsize) -> Self {
        counter.fetch_add(1, Ordering::AcqRel);
        Self { counter }
    }
}
impl Drop for AtomicCounterGuard<'_> {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::AcqRel);
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
            .stdout(Stdio::piped());
        for (key, value) in &spec.env {
            command.env(key, value);
        }
        configure_process_group(&mut command);
        command.stderr(match spec.stderr_path.as_deref() {
            Some(path) => stderr_file(path)?,
            None => Stdio::null(),
        });

        let mut child = command.spawn().map_err(|error| {
            format!(
                "spawn {} worker '{}' failed: {error}",
                spec.kind.as_str(),
                spec.key
            )
        })?;
        let pid = child.id();
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
            events,
            state: AtomicU8::new(state_to_u8(WorkerLifecycleState::Ready)),
            last_heartbeat_ms: AtomicU64::new(now_ms()),
            active_jobs: AtomicUsize::new(0),
            queued_jobs: AtomicUsize::new(0),
            parent_queued_jobs: AtomicUsize::new(0),
            started_at_ms: now_ms(),
            request_timeout: spec.request_timeout,
            heartbeat_timeout: spec.heartbeat_timeout,
            queue_wait_timeout: spec.queue_wait_timeout,
            max_concurrency: spec.max_concurrency.max(1),
            request_slots: Arc::new(Semaphore::new(spec.max_concurrency.max(1))),
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
                            worker
                                .last_heartbeat_ms
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
        self.request_with_id(
            uuid::Uuid::new_v4().to_string(),
            None,
            operation,
            payload,
            timeout_override,
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
        if !matches!(
            self.state(),
            WorkerLifecycleState::Ready | WorkerLifecycleState::Busy
        ) {
            return Err(format!(
                "{} worker '{}' is not ready",
                self.kind.as_str(),
                self.key
            ));
        }
        let queue_guard = AtomicCounterGuard::increment(&self.parent_queued_jobs);
        let permit = tokio::time::timeout(
            self.queue_wait_timeout,
            self.request_slots.clone().acquire_owned(),
        )
        .await
        .map_err(|_| format!("worker '{}' request queue timeout", self.key))?
        .map_err(|_| "worker request queue closed".to_string())?;
        drop(queue_guard);
        let _permit = permit;
        let timeout = timeout_override.unwrap_or(self.request_timeout);
        let job_id_for_cancel = job_id.clone();
        let request = WorkerRequest {
            request_id: request_id.clone(),
            job_id,
            deadline_unix_ms: Some(now_ms().saturating_add(timeout.as_millis() as u64)),
            operation: operation.into(),
            payload,
        };
        let (sender, receiver) = oneshot::channel();
        match self.pending.entry(request_id.clone()) {
            Entry::Vacant(entry) => {
                entry.insert(sender);
            }
            Entry::Occupied(_) => {
                return Err(format!("duplicate worker request id '{request_id}'"))
            }
        }
        if let Err(error) = self
            .write_parent_frame(&ParentFrame::Request { request })
            .await
        {
            self.pending.remove(&request_id);
            self.mark_failed(error.clone());
            return Err(error);
        }
        let response = match tokio::time::timeout(timeout, receiver).await {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => return Err("worker response channel closed".to_string()),
            Err(_) => {
                self.pending.remove(&request_id);
                let _ = self
                    .write_parent_frame(&ParentFrame::Cancel {
                        request_id: request_id.clone(),
                        job_id: job_id_for_cancel,
                    })
                    .await;
                return Err(format!(
                    "worker request '{request_id}' timed out after {timeout:?}"
                ));
            }
        };
        if response.ok {
            Ok(response.payload)
        } else {
            Err(response
                .error
                .unwrap_or_else(|| "worker request failed".to_string()))
        }
    }

    pub async fn cancel_job(&self, job_id: impl Into<String>) -> Result<(), String> {
        self.write_parent_frame(&ParentFrame::Cancel {
            request_id: uuid::Uuid::new_v4().to_string(),
            job_id: Some(job_id.into()),
        })
        .await
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
        ManagedWorkerSnapshot {
            key: self.key.clone(),
            worker_kind: self.kind,
            state: self.state(),
            pid: self.pid(),
            worker_instance_id: Some(self.hello.worker_instance_id.clone()),
            last_heartbeat_ms: Some(self.last_heartbeat_ms.load(Ordering::Acquire)),
            heartbeat_age_ms: Some(self.heartbeat_age().as_millis() as u64),
            started_at_ms: self.started_at_ms,
            restart_count,
            active_jobs: self.active_jobs.load(Ordering::Acquire),
            queued_jobs: self.queued_jobs.load(Ordering::Acquire)
                + self.parent_queued_jobs.load(Ordering::Acquire),
            max_concurrency: self.max_concurrency,
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
        *self.last_error.write() = Some(error);
        self.state.store(
            state_to_u8(WorkerLifecycleState::Degraded),
            Ordering::Release,
        );
        self.fail_pending("worker failed");
    }
    fn mark_stopped(&self, reason: Option<String>) {
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

fn stderr_file(path: &Path) -> Result<Stdio, String> {
    const MAX_BYTES: u64 = 32 * 1024 * 1024;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    if std::fs::metadata(path)
        .map(|m| m.len() >= MAX_BYTES)
        .unwrap_or(false)
    {
        let rotated = path.with_extension("log.1");
        let _ = std::fs::remove_file(&rotated);
        std::fs::rename(path, rotated).map_err(|e| e.to_string())?;
    }
    Ok(Stdio::from(
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| e.to_string())?,
    ))
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    if std::env::var_os(WORKER_KIND_ENV).is_none() {
        command.process_group(0);
    }
}
#[cfg(windows)]
fn configure_process_group(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    if std::env::var_os(WORKER_KIND_ENV).is_none() {
        command.creation_flags(0x0000_0200);
    }
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
