use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use super::{
    global_worker_supervisor, run_worker_stdio, ParentFrame, WorkerKind, WorkerSpawnSpec,
    WorkerStdioContext,
};

const ASR_WORKER_ENV: &str = "BIFROST_ASR_WORKER";
const ASR_WORKER_KEY: &str = "asr:offline-jobs";
const DEFAULT_IDLE_SECS: u64 = 5 * 60;
const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 24 * 60 * 60;

static LAST_USED_MS: AtomicU64 = AtomicU64::new(0);
static IDLE_REAPER_STARTED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunDirectoryTaskRequest {
    task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recording_date: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunDirectoryTaskResult {
    pub processed: usize,
    pub failed: usize,
    pub status: String,
}

pub(crate) fn is_asr_worker_process() -> bool {
    std::env::var(ASR_WORKER_ENV)
        .ok()
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

pub(crate) async fn run_directory_task(
    task_id: &str,
    recording_date: Option<NaiveDate>,
) -> Result<RunDirectoryTaskResult, String> {
    let worker = ensure_worker().await?;
    let request_id = uuid::Uuid::new_v4().to_string();
    let result = worker
        .request_with_id(
            request_id.clone(),
            Some(format!("task:{task_id}")),
            "asr.run_directory_task",
            serde_json::to_value(RunDirectoryTaskRequest {
                task_id: task_id.to_string(),
                recording_date: recording_date.map(|date| date.format("%Y-%m-%d").to_string()),
            })
            .map_err(|error| format!("serialize ASR worker task request: {error}"))?,
            Some(Duration::from_secs(DEFAULT_REQUEST_TIMEOUT_SECS)),
        )
        .await;
    touch();
    let result = result?;
    let parsed: RunDirectoryTaskResult = serde_json::from_value(result)
        .map_err(|error| format!("parse ASR worker result: {error}"))?;
    if parsed.status == "failed" {
        super::mark_worker_job_failed(
            &request_id,
            format!("ASR directory task '{task_id}' failed"),
        );
    } else if parsed.status == "paused" {
        super::mark_worker_job_cancelled(
            &request_id,
            Some(format!("ASR directory task '{task_id}' paused")),
        );
    }
    Ok(parsed)
}

pub(crate) async fn run_source_compression(task_id: &str) -> Result<serde_json::Value, String> {
    let worker = ensure_worker().await?;
    let result = worker
        .request_with_id(
            uuid::Uuid::new_v4().to_string(),
            Some(format!("compression:{task_id}")),
            "asr.compress_source_audio",
            serde_json::json!({ "taskId": task_id }),
            Some(Duration::from_secs(DEFAULT_REQUEST_TIMEOUT_SECS)),
        )
        .await;
    touch();
    result
}

pub(crate) async fn stop_source_compression(task_id: &str) -> bool {
    crate::handlers::asr_jobs::set_worker_source_compression_cancel(task_id, true);
    let Some(worker) = global_worker_supervisor().get(ASR_WORKER_KEY).await else {
        return false;
    };
    if let Err(error) = worker.cancel_job(format!("compression:{task_id}")).await {
        tracing::warn!(task_id, error = %error, "failed to cancel ASR compression worker job");
    }
    true
}

pub(crate) async fn stop_task(task_id: &str) -> bool {
    crate::handlers::asr_jobs::set_worker_force_pause(task_id, true);
    let Some(worker) = global_worker_supervisor().get(ASR_WORKER_KEY).await else {
        return false;
    };
    if let Err(error) = worker.cancel_job(format!("task:{task_id}")).await {
        tracing::warn!(task_id, error = %error, "failed to cancel ASR worker task");
    }
    true
}

async fn ensure_worker() -> Result<Arc<super::ManagedWorker>, String> {
    touch();
    start_idle_reaper();
    global_worker_supervisor().get_or_start(spawn_spec()?).await
}

fn spawn_spec() -> Result<WorkerSpawnSpec, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("resolve ASR worker executable: {error}"))?;
    let executable = labeled_worker_executable(&executable, "bifrost-asr-worker");
    let data_dir = bifrost_storage::data_dir();
    let mut spec = WorkerSpawnSpec::new(
        ASR_WORKER_KEY,
        WorkerKind::Asr,
        executable,
        vec![
            "auxiliary-worker".to_string(),
            "--kind".to_string(),
            "asr".to_string(),
            "--data-dir".to_string(),
            data_dir.display().to_string(),
            "--admin-host".to_string(),
            "127.0.0.1".to_string(),
            "--admin-port".to_string(),
            "0".to_string(),
        ],
    );
    spec.env.insert(ASR_WORKER_ENV.to_string(), "1".to_string());
    spec.max_concurrency = 1;
    spec.startup_timeout = Duration::from_secs(20);
    spec.request_timeout = Duration::from_secs(DEFAULT_REQUEST_TIMEOUT_SECS);
    spec.queue_wait_timeout = Duration::from_secs(60);
    spec.heartbeat_timeout = Duration::from_secs(45);
    spec.stderr_path = Some(runtime_root().join("asr-worker.log"));
    Ok(spec)
}

fn start_idle_reaper() {
    if IDLE_REAPER_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    tokio::spawn(async move {
        let idle = Duration::from_secs(idle_secs());
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        loop {
            interval.tick().await;
            let last = LAST_USED_MS.load(Ordering::Acquire);
            if last == 0 || super::worker_now_ms().saturating_sub(last) < idle.as_millis() as u64 {
                continue;
            }
            if let Some(worker) = global_worker_supervisor().get(ASR_WORKER_KEY).await {
                let snapshot = worker.snapshot(0, None, None);
                if snapshot.active_jobs == 0 && snapshot.queued_jobs == 0 {
                    global_worker_supervisor()
                        .stop(ASR_WORKER_KEY, Duration::from_secs(10))
                        .await;
                    LAST_USED_MS.store(0, Ordering::Release);
                }
            }
        }
    });
}

fn touch() {
    LAST_USED_MS.store(super::worker_now_ms(), Ordering::Release);
}

fn idle_secs() -> u64 {
    std::env::var("BIFROST_ASR_WORKER_IDLE_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value >= 30)
        .unwrap_or(DEFAULT_IDLE_SECS)
}

pub fn run_asr_worker_stdio() -> Result<(), String> {
    std::env::set_var(ASR_WORKER_ENV, "1");
    lower_process_priority();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("bifrost-asr-worker")
        .build()
        .map_err(|error| format!("build ASR worker runtime: {error}"))?;
    runtime.block_on(run_worker_stdio(
        WorkerKind::Asr,
        vec![
            "asr.run_directory_task".to_string(),
            "asr.compress_source_audio".to_string(),
        ],
        handle_worker_frame,
    ))
}

async fn handle_worker_frame(
    frame: ParentFrame,
    context: Arc<WorkerStdioContext>,
) -> Result<(), String> {
    match frame {
        ParentFrame::Request { request } => {
            let request_id = request.request_id.clone();
            let result = match request.operation.as_str() {
                "asr.run_directory_task" => {
                    let request: RunDirectoryTaskRequest = serde_json::from_value(request.payload)
                        .map_err(|error| format!("parse ASR worker task request: {error}"))?;
                    let date = request
                        .recording_date
                        .as_deref()
                        .map(|value| {
                            NaiveDate::parse_from_str(value, "%Y-%m-%d")
                                .map_err(|error| format!("parse ASR recording date: {error}"))
                        })
                        .transpose()?;
                    crate::handlers::asr_jobs::run_directory_task_in_worker(&request.task_id, date)
                        .await
                        .and_then(|result| {
                            serde_json::to_value(RunDirectoryTaskResult {
                                processed: result.processed,
                                failed: result.failed,
                                status: result.status,
                            })
                            .map_err(|error| format!("serialize ASR worker result: {error}"))
                        })
                }
                "asr.compress_source_audio" => {
                    let task_id = request
                        .payload
                        .get("taskId")
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .ok_or_else(|| "ASR compression taskId is required".to_string())?;
                    crate::handlers::asr_jobs::run_source_compression_in_worker(task_id).await
                }
                other => Err(format!("unsupported ASR worker operation '{other}'")),
            };
            context.response(request_id, result).await;
        }
        ParentFrame::Cancel { job_id, .. } => {
            match job_id.as_deref() {
                Some(value) if value.starts_with("compression:") => {
                    crate::handlers::asr_jobs::set_worker_source_compression_cancel(
                        value.trim_start_matches("compression:"),
                        true,
                    );
                }
                Some(value) => {
                    crate::handlers::asr_jobs::set_worker_force_pause(
                        value.trim_start_matches("task:"),
                        true,
                    );
                }
                None => {
                    crate::handlers::asr_jobs::cancel_all_worker_tasks();
                    crate::handlers::asr_jobs::cancel_all_worker_source_compressions();
                }
            }
            crate::shutdown_managed_asr_service().await;
        }
        ParentFrame::Shutdown { .. } => {
            crate::handlers::asr_jobs::cancel_all_worker_tasks();
            crate::handlers::asr_jobs::cancel_all_worker_source_compressions();
            crate::shutdown_managed_asr_service().await;
        }
        ParentFrame::ConfigApply {
            request_id,
            generation,
            ..
        } => {
            context
                .response(
                    request_id,
                    Ok(serde_json::json!({"generation": generation, "applied": true})),
                )
                .await;
        }
        ParentFrame::Ping { .. } => {}
    }
    Ok(())
}

fn runtime_root() -> PathBuf {
    bifrost_storage::data_dir().join("runtime/asr-worker")
}

fn labeled_worker_executable(executable: &Path, alias_name: &str) -> PathBuf {
    let alias_dir = bifrost_storage::data_dir().join("runtime/process-aliases");
    bifrost_core::process_alias_executable(executable, &alias_dir, alias_name)
        .unwrap_or_else(|_| executable.to_path_buf())
}

#[cfg(unix)]
fn lower_process_priority() {
    let _ = std::process::Command::new("renice")
        .args(["10", "-p", &std::process::id().to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(not(unix))]
fn lower_process_priority() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_round_trip() {
        let value = serde_json::to_value(RunDirectoryTaskResult {
            processed: 3,
            failed: 1,
            status: "completed".to_string(),
        })
        .unwrap();
        let parsed: RunDirectoryTaskResult = serde_json::from_value(value).unwrap();
        assert_eq!(parsed.processed, 3);
        assert_eq!(parsed.failed, 1);
    }
}
