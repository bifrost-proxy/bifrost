use std::collections::HashMap;
use std::io::Write;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::FutureExt;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::process::WORKER_STARTUP_TOKEN_ENV;
use super::protocol::{
    now_ms, parse_parent_frame, read_limited_sync_line, serialize_frame, ParentFrame, WorkerEvent,
    WorkerFrame, WorkerHeartbeat, WorkerHello, WorkerKind, WorkerResponse,
    WORKER_HEARTBEAT_INTERVAL_SECS, WORKER_MAX_FRAME_BYTES, WORKER_PROTOCOL_VERSION,
};

const WORKER_MAX_IN_FLIGHT_REQUESTS: usize = 128;

struct RunningJob {
    job_id: Option<String>,
    handle: JoinHandle<()>,
}

struct ActiveJobGuard<'a> {
    counter: &'a AtomicUsize,
}

impl<'a> ActiveJobGuard<'a> {
    fn increment(counter: &'a AtomicUsize) -> Self {
        counter.fetch_add(1, Ordering::AcqRel);
        Self { counter }
    }
}

impl Drop for ActiveJobGuard<'_> {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::AcqRel);
    }
}

pub struct WorkerStdioContext {
    pub kind: WorkerKind,
    pub instance_id: String,
    pub shutdown: Arc<AtomicBool>,
    pub active_jobs: Arc<AtomicUsize>,
    pub queued_jobs: Arc<AtomicUsize>,
    output_tx: mpsc::Sender<WorkerFrame>,
}

impl WorkerStdioContext {
    pub async fn response(&self, request_id: String, result: Result<serde_json::Value, String>) {
        let mut response = match result {
            Ok(payload) => WorkerResponse {
                request_id,
                ok: true,
                payload,
                error: None,
            },
            Err(error) => WorkerResponse {
                request_id,
                ok: false,
                payload: serde_json::Value::Null,
                error: Some(error),
            },
        };
        let mut frame = WorkerFrame::Response {
            response: response.clone(),
        };
        if serialize_frame(&frame).is_err() {
            response.ok = false;
            response.payload = serde_json::Value::Null;
            response.error = Some("worker response exceeded IPC frame limit".to_string());
            frame = WorkerFrame::Response { response };
        }
        let _ = self.output_tx.send(frame).await;
    }

    pub async fn event(&self, event: WorkerEvent) {
        let frame = WorkerFrame::Event { event };
        if serialize_frame(&frame).is_ok() {
            let _ = self.output_tx.send(frame).await;
        }
    }

    /// Best-effort event delivery for progress-style notifications.
    ///
    /// Progress must never block final responses or heartbeats. Callers are
    /// expected to treat `false` as a dropped/coalesced progress event.
    pub fn try_event(&self, event: WorkerEvent) -> bool {
        let frame = WorkerFrame::Event { event };
        if serialize_frame(&frame).is_err() {
            return false;
        }
        self.output_tx.try_send(frame).is_ok()
    }
}

pub async fn run_worker_stdio<F, Fut>(
    kind: WorkerKind,
    capabilities: Vec<String>,
    handler: F,
) -> Result<(), String>
where
    F: Fn(ParentFrame, Arc<WorkerStdioContext>) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<(), String>> + Send + 'static,
{
    let startup_token = std::env::var(WORKER_STARTUP_TOKEN_ENV)
        .map_err(|_| format!("{WORKER_STARTUP_TOKEN_ENV} is required"))?;
    let instance_id = uuid::Uuid::new_v4().to_string();
    let (output_tx, mut output_rx) = mpsc::channel::<WorkerFrame>(128);
    let context = Arc::new(WorkerStdioContext {
        kind,
        instance_id: instance_id.clone(),
        shutdown: Arc::new(AtomicBool::new(false)),
        active_jobs: Arc::new(AtomicUsize::new(0)),
        queued_jobs: Arc::new(AtomicUsize::new(0)),
        output_tx: output_tx.clone(),
    });

    write_stdout_frame(&WorkerFrame::Hello {
        hello: WorkerHello {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_kind: kind,
            worker_instance_id: instance_id.clone(),
            pid: std::process::id(),
            build_version: env!("CARGO_PKG_VERSION").to_string(),
            startup_token,
            capabilities,
        },
    })?;

    let writer = std::thread::spawn(move || {
        while let Some(frame) = output_rx.blocking_recv() {
            let goodbye = matches!(frame, WorkerFrame::Goodbye { .. });
            if write_stdout_frame(&frame).is_err() || goodbye {
                break;
            }
        }
    });
    let _ = output_tx
        .send(WorkerFrame::Ready {
            worker_instance_id: instance_id.clone(),
        })
        .await;

    let heartbeat_context = context.clone();
    let heartbeat_tx = output_tx.clone();
    let heartbeat_task = tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(Duration::from_secs(WORKER_HEARTBEAT_INTERVAL_SECS));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if heartbeat_context.shutdown.load(Ordering::Acquire) {
                break;
            }
            match heartbeat_tx.try_send(WorkerFrame::Heartbeat {
                heartbeat: WorkerHeartbeat {
                    worker_instance_id: heartbeat_context.instance_id.clone(),
                    timestamp_ms: now_ms(),
                    active_jobs: heartbeat_context.active_jobs.load(Ordering::Acquire),
                    queued_jobs: heartbeat_context.queued_jobs.load(Ordering::Acquire),
                },
            }) {
                Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => {}
                Err(mpsc::error::TrySendError::Closed(_)) => break,
            }
        }
    });

    let (input_tx, mut input_rx) = mpsc::channel::<ParentFrame>(128);
    let shutdown = context.shutdown.clone();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut stdin = std::io::BufReader::new(stdin.lock());
        let mut explicit_shutdown = false;
        loop {
            let line = match read_limited_sync_line(&mut stdin, WORKER_MAX_FRAME_BYTES) {
                Ok(Some(line)) => line,
                Ok(None) => break,
                Err(error) => {
                    eprintln!("worker protocol input rejected: {error}");
                    break;
                }
            };
            match parse_parent_frame(&line) {
                Ok(frame) => {
                    explicit_shutdown = matches!(frame, ParentFrame::Shutdown { .. });
                    if input_tx.blocking_send(frame).is_err() || explicit_shutdown {
                        break;
                    }
                }
                Err(error) => {
                    eprintln!("worker protocol input rejected: {error}");
                    break;
                }
            }
        }
        if !explicit_shutdown {
            let _ = input_tx.blocking_send(ParentFrame::Shutdown {
                request_id: format!("stdin-closed-{}", now_ms()),
            });
        }
        shutdown.store(true, Ordering::Release);
    });

    let handler = Arc::new(handler);
    let mut jobs = HashMap::<String, RunningJob>::new();
    let mut shutdown_handled = false;
    while let Some(frame) = input_rx.recv().await {
        jobs.retain(|_, job| !job.handle.is_finished());
        match frame {
            ParentFrame::Ping { request_id } => {
                context
                    .response(request_id, Ok(serde_json::json!({ "pong": true })))
                    .await;
            }
            ParentFrame::Cancel { request_id, job_id } => {
                match job_id.as_deref() {
                    Some(job_id) => abort_job_by_id(&mut jobs, job_id).await,
                    None => abort_jobs(&mut jobs).await,
                }
                if let Err(error) =
                    handler(ParentFrame::Cancel { request_id, job_id }, context.clone()).await
                {
                    eprintln!("worker cancel handler failed: {error}");
                }
            }
            ParentFrame::Shutdown { request_id } => {
                context.shutdown.store(true, Ordering::Release);
                abort_jobs(&mut jobs).await;
                if let Err(error) = handler(
                    ParentFrame::Shutdown {
                        request_id: request_id.clone(),
                    },
                    context.clone(),
                )
                .await
                {
                    eprintln!("worker shutdown handler failed: {error}");
                }
                context
                    .response(request_id, Ok(serde_json::json!({ "stopping": true })))
                    .await;
                shutdown_handled = true;
                break;
            }
            frame => {
                if let ParentFrame::Request { request } = &frame {
                    if request
                        .deadline_unix_ms
                        .is_some_and(|deadline| deadline <= now_ms())
                    {
                        context
                            .response(
                                request.request_id.clone(),
                                Err("worker request deadline expired before execution".to_string()),
                            )
                            .await;
                        continue;
                    }
                }
                let request_id =
                    frame_request_id(&frame).unwrap_or_else(|| format!("frame-{}", now_ms()));
                let job_id = frame_job_id(&frame);
                if jobs.contains_key(&request_id) {
                    context
                        .response(
                            request_id,
                            Err("duplicate in-flight worker request id".to_string()),
                        )
                        .await;
                    continue;
                }
                if jobs.len() >= WORKER_MAX_IN_FLIGHT_REQUESTS {
                    context
                        .response(
                            request_id,
                            Err(format!(
                                "worker in-flight request limit reached ({WORKER_MAX_IN_FLIGHT_REQUESTS})"
                            )),
                        )
                        .await;
                    continue;
                }
                let handler = handler.clone();
                let context = context.clone();
                let failure_request_id = request_id.clone();
                let handle = tokio::spawn(async move {
                    let _active_guard = ActiveJobGuard::increment(&context.active_jobs);
                    let result = AssertUnwindSafe(handler(frame, context.clone()))
                        .catch_unwind()
                        .await;
                    match result {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => {
                            eprintln!("worker handler failed: {error}");
                            context.response(failure_request_id, Err(error)).await;
                        }
                        Err(payload) => {
                            let error = panic_message(payload);
                            eprintln!("worker handler panicked: {error}");
                            context.response(failure_request_id, Err(error)).await;
                        }
                    }
                });
                jobs.insert(request_id, RunningJob { job_id, handle });
            }
        }
    }

    context.shutdown.store(true, Ordering::Release);
    abort_jobs(&mut jobs).await;
    if !shutdown_handled {
        let _ = handler(
            ParentFrame::Shutdown {
                request_id: format!("input-channel-closed-{}", now_ms()),
            },
            context.clone(),
        )
        .await;
    }
    heartbeat_task.abort();
    let _ = output_tx
        .send(WorkerFrame::Goodbye {
            worker_instance_id: instance_id,
            reason: None,
        })
        .await;
    let _ = writer.join();
    Ok(())
}

fn frame_request_id(frame: &ParentFrame) -> Option<String> {
    match frame {
        ParentFrame::Request { request } => Some(request.request_id.clone()),
        ParentFrame::ConfigApply { request_id, .. } => Some(request_id.clone()),
        ParentFrame::Ping { request_id }
        | ParentFrame::Shutdown { request_id }
        | ParentFrame::Cancel { request_id, .. } => Some(request_id.clone()),
    }
}

fn frame_job_id(frame: &ParentFrame) -> Option<String> {
    match frame {
        ParentFrame::Request { request } => request.job_id.clone(),
        _ => None,
    }
}

async fn abort_job_by_id(jobs: &mut HashMap<String, RunningJob>, job_id: &str) {
    let request_ids = jobs
        .iter()
        .filter_map(|(request_id, job)| {
            (job.job_id.as_deref() == Some(job_id)).then(|| request_id.clone())
        })
        .collect::<Vec<_>>();
    for request_id in request_ids {
        if let Some(job) = jobs.remove(&request_id) {
            job.handle.abort();
            let _ = job.handle.await;
        }
    }
}

async fn abort_jobs(jobs: &mut HashMap<String, RunningJob>) {
    for (_, job) in jobs.drain() {
        job.handle.abort();
        let _ = job.handle.await;
    }
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        format!("worker handler panicked: {message}")
    } else if let Some(message) = payload.downcast_ref::<String>() {
        format!("worker handler panicked: {message}")
    } else {
        "worker handler panicked with a non-string payload".to_string()
    }
}

fn write_stdout_frame(frame: &WorkerFrame) -> Result<(), String> {
    let line = serialize_frame(frame)?;
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(line.as_bytes())
        .map_err(|error| format!("write worker stdout failed: {error}"))?;
    stdout
        .write_all(b"\n")
        .map_err(|error| format!("write worker stdout newline failed: {error}"))?;
    stdout
        .flush()
        .map_err(|error| format!("flush worker stdout failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancellation_matches_job_id_instead_of_cancel_request_id() {
        let mut jobs = HashMap::new();
        jobs.insert(
            "request-a".to_string(),
            RunningJob {
                job_id: Some("task:a".to_string()),
                handle: tokio::spawn(std::future::pending::<()>()),
            },
        );
        jobs.insert(
            "request-b".to_string(),
            RunningJob {
                job_id: Some("task:b".to_string()),
                handle: tokio::spawn(std::future::pending::<()>()),
            },
        );

        abort_job_by_id(&mut jobs, "task:a").await;

        assert!(!jobs.contains_key("request-a"));
        assert!(jobs.contains_key("request-b"));
        abort_jobs(&mut jobs).await;
        assert!(jobs.is_empty());
    }

    #[tokio::test]
    async fn oversized_response_is_replaced_with_bounded_error() {
        let (output_tx, mut output_rx) = mpsc::channel(1);
        let context = WorkerStdioContext {
            kind: WorkerKind::Browser,
            instance_id: "test".to_string(),
            shutdown: Arc::new(AtomicBool::new(false)),
            active_jobs: Arc::new(AtomicUsize::new(0)),
            queued_jobs: Arc::new(AtomicUsize::new(0)),
            output_tx,
        };

        context
            .response(
                "request-1".to_string(),
                Ok(serde_json::json!({"data": "x".repeat(WORKER_MAX_FRAME_BYTES)})),
            )
            .await;

        let WorkerFrame::Response { response } = output_rx.recv().await.unwrap() else {
            panic!("expected response frame");
        };
        assert!(!response.ok);
        assert_eq!(response.payload, serde_json::Value::Null);
        assert_eq!(
            response.error.as_deref(),
            Some("worker response exceeded IPC frame limit")
        );
    }

    #[test]
    fn progress_event_is_dropped_when_output_queue_is_full() {
        let (output_tx, _output_rx) = mpsc::channel(1);
        output_tx
            .try_send(WorkerFrame::Ready {
                worker_instance_id: "test".to_string(),
            })
            .unwrap();
        let context = WorkerStdioContext {
            kind: WorkerKind::Browser,
            instance_id: "test".to_string(),
            shutdown: Arc::new(AtomicBool::new(false)),
            active_jobs: Arc::new(AtomicUsize::new(0)),
            queued_jobs: Arc::new(AtomicUsize::new(0)),
            output_tx,
        };

        assert!(!context.try_event(WorkerEvent {
            request_id: Some("request-1".to_string()),
            job_id: None,
            event: "progress".to_string(),
            payload: serde_json::json!({"content": "progress"}),
        }));
    }

    #[test]
    fn worker_in_flight_request_limit_is_bounded() {
        assert_eq!(WORKER_MAX_IN_FLIGHT_REQUESTS, 128);
    }
}
