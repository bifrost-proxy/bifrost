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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    file_keys: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunDirectoryTaskResult {
    pub processed: usize,
    pub failed: usize,
    pub status: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub daily_agent_dates: Vec<String>,
}

pub(crate) fn is_asr_worker_process() -> bool {
    std::env::var(ASR_WORKER_ENV)
        .ok()
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

pub(crate) async fn run_directory_task(
    task_id: &str,
    recording_date: Option<NaiveDate>,
    file_keys: Vec<String>,
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
                file_keys,
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
                    crate::handlers::asr_jobs::run_directory_task_in_worker(
                        &request.task_id,
                        date,
                        request.file_keys,
                    )
                    .await
                    .map(|result| {
                        serde_json::json!(RunDirectoryTaskResult {
                            processed: result.processed,
                            failed: result.failed,
                            status: result.status,
                            daily_agent_dates: result.daily_agent_dates,
                        })
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
    use crate::worker_runtime::{WorkerFrame, WorkerRequest};

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn result_round_trip() {
        let value = serde_json::to_value(RunDirectoryTaskResult {
            processed: 3,
            failed: 1,
            status: "completed".to_string(),
            daily_agent_dates: vec!["2026-08-17".to_string()],
        })
        .unwrap();
        let parsed: RunDirectoryTaskResult = serde_json::from_value(value).unwrap();
        assert_eq!(parsed.processed, 3);
        assert_eq!(parsed.failed, 1);
        assert_eq!(parsed.daily_agent_dates, ["2026-08-17"]);
    }

    #[test]
    fn worker_mode_idle_timeout_and_spawn_contract_are_bounded() {
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::remove_var(ASR_WORKER_ENV);
        assert!(!is_asr_worker_process());
        std::env::set_var(ASR_WORKER_ENV, "TRUE");
        assert!(is_asr_worker_process());

        std::env::set_var("BIFROST_ASR_WORKER_IDLE_SECS", "29");
        assert_eq!(idle_secs(), DEFAULT_IDLE_SECS);
        std::env::set_var("BIFROST_ASR_WORKER_IDLE_SECS", "31");
        assert_eq!(idle_secs(), 31);
        std::env::remove_var("BIFROST_ASR_WORKER_IDLE_SECS");

        let spec = spawn_spec().unwrap();
        assert_eq!(spec.key, ASR_WORKER_KEY);
        assert_eq!(spec.kind, WorkerKind::Asr);
        assert_eq!(spec.max_concurrency, 1);
        assert_eq!(spec.env.get(ASR_WORKER_ENV).map(String::as_str), Some("1"));
        assert!(spec.args.iter().any(|value| value == "auxiliary-worker"));
        assert!(spec.stderr_path.unwrap().ends_with("asr-worker.log"));
        assert!(runtime_root().ends_with("runtime/asr-worker"));
        std::env::remove_var(ASR_WORKER_ENV);
    }

    #[tokio::test]
    async fn worker_frame_dispatch_reports_controls_and_validation_errors() {
        let (context, mut output) = WorkerStdioContext::test_context(WorkerKind::Asr);

        handle_worker_frame(
            ParentFrame::Request {
                request: WorkerRequest {
                    request_id: "unsupported".to_string(),
                    job_id: None,
                    deadline_unix_ms: None,
                    operation: "asr.unknown".to_string(),
                    payload: serde_json::json!({}),
                },
            },
            context.clone(),
        )
        .await
        .unwrap();
        let WorkerFrame::Response { response } = output.recv().await.unwrap() else {
            panic!("expected unsupported response")
        };
        assert_eq!(response.request_id, "unsupported");
        assert!(response.error.unwrap().contains("unsupported"));

        handle_worker_frame(
            ParentFrame::ConfigApply {
                request_id: "config".to_string(),
                generation: 7,
                payload: serde_json::json!({}),
            },
            context.clone(),
        )
        .await
        .unwrap();
        let WorkerFrame::Response { response } = output.recv().await.unwrap() else {
            panic!("expected config response")
        };
        assert!(response.ok);
        assert_eq!(response.payload["generation"], 7);

        let bad_payload = handle_worker_frame(
            ParentFrame::Request {
                request: WorkerRequest {
                    request_id: "bad-payload".to_string(),
                    job_id: None,
                    deadline_unix_ms: None,
                    operation: "asr.run_directory_task".to_string(),
                    payload: serde_json::json!({}),
                },
            },
            context.clone(),
        )
        .await
        .unwrap_err();
        assert!(bad_payload.contains("parse ASR worker task request"));

        let bad_date = handle_worker_frame(
            ParentFrame::Request {
                request: WorkerRequest {
                    request_id: "bad-date".to_string(),
                    job_id: None,
                    deadline_unix_ms: None,
                    operation: "asr.run_directory_task".to_string(),
                    payload: serde_json::json!({
                        "taskId": "missing",
                        "recordingDate": "not-a-date"
                    }),
                },
            },
            context.clone(),
        )
        .await
        .unwrap_err();
        assert!(bad_date.contains("parse ASR recording date"));

        let missing_compression = handle_worker_frame(
            ParentFrame::Request {
                request: WorkerRequest {
                    request_id: "bad-compression".to_string(),
                    job_id: None,
                    deadline_unix_ms: None,
                    operation: "asr.compress_source_audio".to_string(),
                    payload: serde_json::json!({"taskId": "  "}),
                },
            },
            context.clone(),
        )
        .await
        .unwrap_err();
        assert!(missing_compression.contains("taskId is required"));

        for frame in [
            ParentFrame::Cancel {
                request_id: "cancel-compression".to_string(),
                job_id: Some("compression:missing".to_string()),
            },
            ParentFrame::Cancel {
                request_id: "cancel-task".to_string(),
                job_id: Some("task:missing".to_string()),
            },
            ParentFrame::Cancel {
                request_id: "cancel-all".to_string(),
                job_id: None,
            },
            ParentFrame::Shutdown {
                request_id: "shutdown".to_string(),
            },
            ParentFrame::Ping {
                request_id: "ping".to_string(),
            },
        ] {
            handle_worker_frame(frame, context.clone()).await.unwrap();
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn parent_api_round_trips_results_and_controls_through_isolated_worker() {
        let _jobs_guard = crate::worker_runtime::worker_jobs_test_guard_async().await;
        crate::worker_runtime::clear_worker_jobs_for_tests();
        let script = r#"
printf '{"type":"hello","hello":{"protocolVersion":1,"workerKind":"asr","workerInstanceId":"asr-api-test","pid":%s,"buildVersion":"test","startupToken":"%s","capabilities":[]}}\n' "$$" "$BIFROST_WORKER_STARTUP_TOKEN"
printf '{"type":"ready","worker_instance_id":"asr-api-test"}\n'
while IFS= read -r line; do
  case "$line" in
    *'"type":"request"'*)
      request_id=$(printf '%s' "$line" | sed -n 's/.*"requestId":"\([^"]*\)".*/\1/p')
      case "$line" in
        *'asr.compress_source_audio'*)
          payload='{"compressed":true}'
          ;;
        *'"taskId":"failed"'*)
          payload='{"processed":1,"failed":1,"status":"failed"}'
          ;;
        *'"taskId":"paused"'*)
          payload='{"processed":1,"failed":0,"status":"paused"}'
          ;;
        *'"taskId":"daily-dates"'*)
          payload='{"processed":2,"failed":0,"status":"completed","dailyAgentDates":["2026-08-17","2026-08-18"]}'
          ;;
        *'"taskId":"invalid-result"'*)
          payload='{"unexpected":true}'
          ;;
        *'"fileKeys":["selected-key"]'*)
          payload='{"processed":3,"failed":0,"status":"completed"}'
          ;;
        *)
          payload='{"processed":2,"failed":0,"status":"completed"}'
          ;;
      esac
      printf '{"type":"event","event":{"requestId":"%s","jobId":null,"event":"progress","payload":{"step":1}}}\n' "$request_id"
      printf '{"type":"response","response":{"requestId":"%s","ok":true,"cancelled":false,"payload":%s,"error":null}}\n' "$request_id" "$payload"
      ;;
    *'"type":"shutdown"'*)
      printf '{"type":"goodbye","worker_instance_id":"asr-api-test","reason":"test complete"}\n'
      exit 0
      ;;
  esac
done
"#;
        let mut spec = WorkerSpawnSpec::new(
            ASR_WORKER_KEY,
            WorkerKind::Asr,
            "/bin/sh",
            vec!["-c".to_string(), script.to_string()],
        );
        spec.startup_timeout = Duration::from_secs(2);
        spec.request_timeout = Duration::from_secs(2);
        spec.heartbeat_timeout = Duration::from_secs(10);
        let supervisor = global_worker_supervisor();
        supervisor.get_or_start(spec).await.unwrap();

        let completed = run_directory_task(
            "completed",
            Some(NaiveDate::from_ymd_opt(2026, 8, 17).unwrap()),
            Vec::new(),
        )
        .await
        .unwrap();
        assert_eq!(
            completed,
            RunDirectoryTaskResult {
                processed: 2,
                failed: 0,
                status: "completed".to_string(),
                daily_agent_dates: Vec::new(),
            }
        );

        let failed = run_directory_task("failed", None, Vec::new())
            .await
            .unwrap();
        assert_eq!(failed.status, "failed");
        let daily_dates = run_directory_task("daily-dates", None, Vec::new())
            .await
            .unwrap();
        assert_eq!(daily_dates.daily_agent_dates, ["2026-08-17", "2026-08-18"]);
        let selected = run_directory_task("selected", None, vec!["selected-key".to_string()])
            .await
            .unwrap();
        assert_eq!(selected.processed, 3);
        let paused = run_directory_task("paused", None, Vec::new())
            .await
            .unwrap();
        assert_eq!(paused.status, "paused");
        assert!(run_directory_task("invalid-result", None, Vec::new())
            .await
            .unwrap_err()
            .contains("parse ASR worker result"));

        assert_eq!(
            run_source_compression("compression-task").await.unwrap(),
            serde_json::json!({"compressed": true})
        );
        assert!(stop_task("completed").await);
        assert!(stop_source_compression("compression-task").await);

        assert!(
            supervisor
                .unregister(ASR_WORKER_KEY, Duration::from_secs(1))
                .await
        );
        assert!(!stop_task("already-stopped").await);
        assert!(!stop_source_compression("already-stopped").await);
    }
}
