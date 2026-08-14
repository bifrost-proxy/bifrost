use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use parking_lot::RwLock;
use tokio::sync::Mutex;

use super::process::{ManagedWorker, ManagedWorkerSnapshot, WorkerSpawnSpec};
use super::protocol::{now_ms, WorkerKind, WorkerLifecycleState};

const FAILURE_WINDOW_MS: u64 = 5 * 60 * 1_000;
const FAILURES_BEFORE_CIRCUIT: usize = 5;
const CIRCUIT_OPEN_MS: u64 = 60 * 1_000;
const BACKOFF_BASE_MS: u64 = 1_000;
const BACKOFF_MAX_MS: u64 = 30 * 1_000;

struct WorkerRecord {
    kind: WorkerKind,
    worker: Option<Arc<ManagedWorker>>,
    last_spec: Option<WorkerSpawnSpec>,
    restart_count: u64,
    last_error: Option<String>,
    failures: VecDeque<u64>,
    backoff_until_ms: Option<u64>,
    circuit_open_until_ms: Option<u64>,
}

impl WorkerRecord {
    fn new(kind: WorkerKind) -> Self {
        Self {
            kind,
            worker: None,
            last_spec: None,
            restart_count: 0,
            last_error: None,
            failures: VecDeque::new(),
            backoff_until_ms: None,
            circuit_open_until_ms: None,
        }
    }
    fn refresh(&mut self, now: u64) {
        while self
            .failures
            .front()
            .is_some_and(|value| now.saturating_sub(*value) > FAILURE_WINDOW_MS)
        {
            self.failures.pop_front();
        }
        if self.backoff_until_ms.is_some_and(|value| value <= now) {
            self.backoff_until_ms = None;
        }
        if self.circuit_open_until_ms.is_some_and(|value| value <= now) {
            self.circuit_open_until_ms = None;
            self.failures.clear();
        }
    }
    fn failure(&mut self, error: String) {
        let now = now_ms();
        self.refresh(now);
        self.restart_count = self.restart_count.saturating_add(1);
        self.last_error = Some(error);
        self.failures.push_back(now);
        if self.failures.len() >= FAILURES_BEFORE_CIRCUIT {
            self.circuit_open_until_ms = Some(now + CIRCUIT_OPEN_MS);
            self.backoff_until_ms = None;
        } else {
            let exponent = self.failures.len().saturating_sub(1).min(5) as u32;
            self.backoff_until_ms = Some(
                now + BACKOFF_BASE_MS
                    .saturating_mul(1_u64 << exponent)
                    .min(BACKOFF_MAX_MS),
            );
        }
    }
    fn gate(&mut self, key: &str) -> Option<String> {
        let now = now_ms();
        self.refresh(now);
        self.circuit_open_until_ms
            .map(|until| format!("worker '{key}' restart circuit is open until {until}"))
            .or_else(|| {
                self.backoff_until_ms
                    .map(|until| format!("worker '{key}' is in restart backoff until {until}"))
            })
    }
    fn clear_gate(&mut self) {
        self.failures.clear();
        self.backoff_until_ms = None;
        self.circuit_open_until_ms = None;
        self.last_error = None;
    }
}

#[derive(Default)]
pub struct WorkerSupervisor {
    workers: RwLock<HashMap<String, Arc<Mutex<WorkerRecord>>>>,
}
pub type SharedWorkerSupervisor = Arc<WorkerSupervisor>;
static GLOBAL: OnceLock<SharedWorkerSupervisor> = OnceLock::new();
pub fn global_worker_supervisor() -> SharedWorkerSupervisor {
    GLOBAL
        .get_or_init(|| Arc::new(WorkerSupervisor::default()))
        .clone()
}

impl WorkerSupervisor {
    pub fn new() -> Self {
        Self::default()
    }
    pub async fn get(&self, key: &str) -> Option<Arc<ManagedWorker>> {
        let record = self.workers.read().get(key).cloned()?;
        let worker = record.lock().await.worker.clone()?;
        worker.is_healthy().await.then_some(worker)
    }
    pub async fn get_or_start(&self, spec: WorkerSpawnSpec) -> Result<Arc<ManagedWorker>, String> {
        let record = self
            .workers
            .write()
            .entry(spec.key.clone())
            .or_insert_with(|| Arc::new(Mutex::new(WorkerRecord::new(spec.kind))))
            .clone();
        let mut record = record.lock().await;
        record.kind = spec.kind;
        record.last_spec = Some(spec.clone());
        if let Some(worker) = record.worker.as_ref() {
            if worker.is_healthy().await {
                return Ok(worker.clone());
            }
        }
        if let Some(stale) = record.worker.take() {
            let state = stale.state();
            let error = stale.snapshot(0, None, None).last_error;
            let _ = stale.shutdown(Duration::from_secs(1)).await;
            if !matches!(
                state,
                WorkerLifecycleState::Stopped | WorkerLifecycleState::Stopping
            ) {
                record.failure(error.unwrap_or_else(|| "worker became unhealthy".to_string()));
            }
        }
        if let Some(error) = record.gate(&spec.key) {
            return Err(error);
        }
        match ManagedWorker::spawn(spec).await {
            Ok(worker) => {
                record.worker = Some(worker.clone());
                record.backoff_until_ms = None;
                record.last_error = None;
                Ok(worker)
            }
            Err(error) => {
                record.failure(error.clone());
                Err(error)
            }
        }
    }
    pub async fn restart(&self, spec: WorkerSpawnSpec) -> Result<Arc<ManagedWorker>, String> {
        self.stop(&spec.key, Duration::from_secs(3)).await;
        let record = self
            .workers
            .write()
            .entry(spec.key.clone())
            .or_insert_with(|| Arc::new(Mutex::new(WorkerRecord::new(spec.kind))))
            .clone();
        {
            let mut record = record.lock().await;
            record.restart_count = record.restart_count.saturating_add(1);
            record.clear_gate();
        }
        self.get_or_start(spec).await
    }
    pub async fn stop(&self, key: &str, grace: Duration) -> bool {
        let Some(record) = self.workers.read().get(key).cloned() else {
            return false;
        };
        let worker = record.lock().await.worker.take();
        if let Some(worker) = worker {
            if let Err(error) = worker.shutdown(grace).await {
                record.lock().await.last_error = Some(error);
            }
            true
        } else {
            false
        }
    }
    pub async fn start_kind(&self, kind: WorkerKind) -> Vec<(String, Result<(), String>)> {
        let specs = self.specs_for_kind(kind).await;
        let mut results = Vec::with_capacity(specs.len());
        for spec in specs {
            let key = spec.key.clone();
            let result = self.get_or_start(spec).await.map(|_| ());
            results.push((key, result));
        }
        results
    }

    pub async fn restart_key(&self, key: &str) -> Result<Arc<ManagedWorker>, String> {
        let record = self
            .workers
            .read()
            .get(key)
            .cloned()
            .ok_or_else(|| format!("worker '{key}' is not registered"))?;
        let spec = record
            .lock()
            .await
            .last_spec
            .clone()
            .ok_or_else(|| format!("worker '{key}' has no restart specification"))?;
        self.restart(spec).await
    }

    pub async fn restart_kind(&self, kind: WorkerKind) -> Vec<(String, Result<(), String>)> {
        let specs = self.specs_for_kind(kind).await;
        let mut results = Vec::with_capacity(specs.len());
        for spec in specs {
            let key = spec.key.clone();
            let result = self.restart(spec).await.map(|_| ());
            results.push((key, result));
        }
        results
    }

    pub async fn reset_circuit(&self, key: &str) -> bool {
        let Some(record) = self.workers.read().get(key).cloned() else {
            return false;
        };
        record.lock().await.clear_gate();
        true
    }

    pub async fn reset_circuit_kind(&self, kind: WorkerKind) -> usize {
        let records = self.records_for_kind(kind).await;
        for (_, record) in &records {
            record.lock().await.clear_gate();
        }
        records.len()
    }

    async fn specs_for_kind(&self, kind: WorkerKind) -> Vec<WorkerSpawnSpec> {
        let records = self.records_for_kind(kind).await;
        let mut specs = Vec::new();
        for (_, record) in records {
            if let Some(spec) = record.lock().await.last_spec.clone() {
                specs.push(spec);
            }
        }
        specs
    }

    async fn records_for_kind(&self, kind: WorkerKind) -> Vec<(String, Arc<Mutex<WorkerRecord>>)> {
        let records = self
            .workers
            .read()
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<Vec<_>>();
        let mut matches = Vec::new();
        for (key, record) in records {
            if record.lock().await.kind == kind {
                matches.push((key, record));
            }
        }
        matches
    }

    pub async fn unregister(&self, key: &str, grace: Duration) -> bool {
        let record = self.workers.write().remove(key);
        let Some(record) = record else {
            return false;
        };
        let worker = record.lock().await.worker.take();
        if let Some(worker) = worker {
            if let Err(error) = worker.shutdown(grace).await {
                tracing::warn!(worker_key = key, error = %error, "failed to stop unregistered worker");
            }
        }
        true
    }

    pub async fn stop_kind(&self, kind: WorkerKind, grace: Duration) -> usize {
        let records = self
            .workers
            .read()
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<Vec<_>>();
        let mut count = 0;
        for (key, record) in records {
            let matches_kind = { record.lock().await.kind == kind };
            if matches_kind && self.stop(&key, grace).await {
                count += 1;
            }
        }
        count
    }
    pub async fn stop_all(&self, grace: Duration) {
        let keys = self.workers.read().keys().cloned().collect::<Vec<_>>();
        for key in keys {
            self.stop(&key, grace).await;
        }
    }
    pub async fn snapshots(&self) -> Vec<ManagedWorkerSnapshot> {
        let records = self
            .workers
            .read()
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<Vec<_>>();
        let now = now_ms();
        let mut result = Vec::new();
        for (key, record) in records {
            let mut record = record.lock().await;
            record.refresh(now);
            if let Some(worker) = record.worker.as_ref() {
                result.push(worker.snapshot(
                    record.restart_count,
                    record.backoff_until_ms,
                    record.circuit_open_until_ms,
                ));
            } else {
                let state = if record.circuit_open_until_ms.is_some() {
                    WorkerLifecycleState::CircuitOpen
                } else if record.backoff_until_ms.is_some() {
                    WorkerLifecycleState::Backoff
                } else {
                    WorkerLifecycleState::Stopped
                };
                result.push(ManagedWorkerSnapshot {
                    key,
                    worker_kind: record.kind,
                    state,
                    pid: None,
                    worker_instance_id: None,
                    last_heartbeat_ms: None,
                    worker_reported_heartbeat_ms: None,
                    heartbeat_age_ms: None,
                    started_at_ms: 0,
                    restart_count: record.restart_count,
                    active_jobs: 0,
                    queued_jobs: 0,
                    max_concurrency: 0,
                    max_queue_depth: 0,
                    rss_bytes: None,
                    virtual_memory_bytes: None,
                    cpu_usage_percent: None,
                    open_file_descriptors: None,
                    backoff_until_ms: record.backoff_until_ms,
                    circuit_open_until_ms: record.circuit_open_until_ms,
                    last_error: record.last_error.clone(),
                });
            }
        }
        result.sort_by(|a, b| a.key.cmp(&b.key));
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn empty_supervisor_is_empty() {
        let supervisor = WorkerSupervisor::new();
        assert!(supervisor.snapshots().await.is_empty());
    }
    #[test]
    fn repeated_failure_opens_circuit() {
        let mut record = WorkerRecord::new(WorkerKind::Browser);
        for index in 0..FAILURES_BEFORE_CIRCUIT {
            record.failure(format!("failure-{index}"));
        }
        assert!(record.circuit_open_until_ms.is_some());
        record.clear_gate();
        assert!(record.circuit_open_until_ms.is_none());
        assert!(record.failures.is_empty());
    }
}
