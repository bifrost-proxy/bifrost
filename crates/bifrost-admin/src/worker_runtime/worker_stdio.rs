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
    now_ms, parse_parent_frame, read_limited_sync_line, serialize_frame, truncate_utf8_bytes,
    ParentFrame, WorkerEvent, WorkerFrame, WorkerHeartbeat, WorkerHello, WorkerKind,
    WorkerResponse, WORKER_HEARTBEAT_INTERVAL_SECS, WORKER_MAX_ERROR_BYTES, WORKER_MAX_FRAME_BYTES,
    WORKER_PROTOCOL_VERSION,
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
    #[cfg(test)]
    pub(crate) fn test_context(kind: WorkerKind) -> (Arc<Self>, mpsc::Receiver<WorkerFrame>) {
        let (output_tx, output_rx) = mpsc::channel(128);
        (
            Arc::new(Self {
                kind,
                instance_id: "test-worker-instance".to_string(),
                shutdown: Arc::new(AtomicBool::new(false)),
                active_jobs: Arc::new(AtomicUsize::new(0)),
                queued_jobs: Arc::new(AtomicUsize::new(0)),
                output_tx,
            }),
            output_rx,
        )
    }

    pub async fn response(&self, request_id: String, result: Result<serde_json::Value, String>) {
        let mut response = match result {
            Ok(payload) => WorkerResponse {
                request_id,
                ok: true,
                cancelled: false,
                payload,
                error: None,
            },
            Err(error) => WorkerResponse {
                request_id,
                ok: false,
                cancelled: false,
                payload: serde_json::Value::Null,
                error: Some(truncate_utf8_bytes(&error, WORKER_MAX_ERROR_BYTES)),
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

    pub async fn cancelled_response(&self, request_id: String) {
        let response = WorkerResponse {
            request_id,
            ok: false,
            cancelled: true,
            payload: serde_json::Value::Null,
            error: Some("worker request cancelled".to_string()),
        };
        let _ = self
            .output_tx
            .send(WorkerFrame::Response { response })
            .await;
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

    let (input_tx, input_rx) = mpsc::channel::<ParentFrame>(128);
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
    process_worker_input(input_rx, context.clone(), handler).await;
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

async fn process_worker_input<F, Fut>(
    mut input_rx: mpsc::Receiver<ParentFrame>,
    context: Arc<WorkerStdioContext>,
    handler: Arc<F>,
) where
    F: Fn(ParentFrame, Arc<WorkerStdioContext>) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<(), String>> + Send + 'static,
{
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
                let aborted_request_ids = match job_id.as_deref() {
                    Some(job_id) => abort_job_by_id(&mut jobs, job_id).await,
                    None => abort_request(&mut jobs, &request_id).await,
                };
                if !aborted_request_ids.is_empty() {
                    if let Err(error) =
                        handler(ParentFrame::Cancel { request_id, job_id }, context.clone()).await
                    {
                        eprintln!("worker cancel handler failed: {error}");
                    }
                }
                for aborted_request_id in aborted_request_ids {
                    context.cancelled_response(aborted_request_id).await;
                }
            }
            ParentFrame::Shutdown { request_id } => {
                context.shutdown.store(true, Ordering::Release);
                let _ = abort_jobs(&mut jobs).await;
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
    let _ = abort_jobs(&mut jobs).await;
    if !shutdown_handled {
        let _ = handler(
            ParentFrame::Shutdown {
                request_id: format!("input-channel-closed-{}", now_ms()),
            },
            context.clone(),
        )
        .await;
    }
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

async fn abort_request(jobs: &mut HashMap<String, RunningJob>, request_id: &str) -> Vec<String> {
    let Some(job) = jobs.remove(request_id) else {
        return Vec::new();
    };
    job.handle.abort();
    let _ = job.handle.await;
    vec![request_id.to_string()]
}

async fn abort_job_by_id(jobs: &mut HashMap<String, RunningJob>, job_id: &str) -> Vec<String> {
    let request_ids = jobs
        .iter()
        .filter(|(_, job)| job.job_id.as_deref() == Some(job_id))
        .map(|(request_id, _)| request_id.clone())
        .collect::<Vec<_>>();
    for request_id in &request_ids {
        if let Some(job) = jobs.remove(request_id) {
            job.handle.abort();
            let _ = job.handle.await;
        }
    }
    request_ids
}

async fn abort_jobs(jobs: &mut HashMap<String, RunningJob>) -> Vec<String> {
    let mut request_ids = Vec::with_capacity(jobs.len());
    for (request_id, job) in jobs.drain() {
        request_ids.push(request_id);
        job.handle.abort();
        let _ = job.handle.await;
    }
    request_ids
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

    fn request_frame(request_id: &str, job_id: Option<&str>, operation: &str) -> ParentFrame {
        ParentFrame::Request {
            request: super::super::protocol::WorkerRequest {
                request_id: request_id.to_string(),
                job_id: job_id.map(str::to_string),
                deadline_unix_ms: None,
                operation: operation.to_string(),
                payload: serde_json::Value::Null,
            },
        }
    }

    async fn response_for(
        output: &mut mpsc::Receiver<WorkerFrame>,
        expected_request_id: &str,
    ) -> WorkerResponse {
        loop {
            let frame = tokio::time::timeout(Duration::from_secs(5), output.recv())
                .await
                .unwrap_or_else(|_| {
                    panic!("worker response timeout for request {expected_request_id}")
                })
                .expect("worker output closed");
            if let WorkerFrame::Response { response } = frame {
                if response.request_id == expected_request_id {
                    return response;
                }
            }
        }
    }

    async fn responses_for(
        output: &mut mpsc::Receiver<WorkerFrame>,
        expected_request_ids: &[&str],
    ) -> HashMap<String, WorkerResponse> {
        let mut responses = HashMap::new();
        tokio::time::timeout(Duration::from_secs(5), async {
            while responses.len() < expected_request_ids.len() {
                let frame = output.recv().await.expect("worker output closed");
                if let WorkerFrame::Response { response } = frame {
                    if expected_request_ids.contains(&response.request_id.as_str()) {
                        responses.insert(response.request_id.clone(), response);
                    }
                }
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "worker response timeout: expected {expected_request_ids:?}, received {:?}",
                responses.keys().collect::<Vec<_>>()
            )
        });
        responses
    }

    async fn wait_for_active(context: &WorkerStdioContext, expected: usize) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while context.active_jobs.load(Ordering::Acquire) != expected {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("active worker jobs did not reach expected count");
    }

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

        let aborted = abort_job_by_id(&mut jobs, "task:a").await;

        assert_eq!(aborted, vec!["request-a".to_string()]);
        assert!(!jobs.contains_key("request-a"));
        assert!(jobs.contains_key("request-b"));
        let _ = abort_jobs(&mut jobs).await;
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

        context
            .event(WorkerEvent {
                request_id: Some("request-event".to_string()),
                job_id: Some("job-event".to_string()),
                event: "progress".to_string(),
                payload: serde_json::json!({"step": 1}),
            })
            .await;
        assert!(matches!(
            output_rx.recv().await.unwrap(),
            WorkerFrame::Event { .. }
        ));
        context
            .cancelled_response("request-cancelled".to_string())
            .await;
        let WorkerFrame::Response { response } = output_rx.recv().await.unwrap() else {
            panic!("expected cancelled response frame");
        };
        assert!(response.cancelled);
    }

    #[tokio::test]
    async fn oversized_error_is_truncated_before_enqueue() {
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
                Err("é".repeat(WORKER_MAX_ERROR_BYTES)),
            )
            .await;

        let WorkerFrame::Response { response } = output_rx.recv().await.unwrap() else {
            panic!("expected response frame");
        };
        let error = response.error.as_deref().unwrap();
        assert!(error.len() <= WORKER_MAX_ERROR_BYTES);
        assert!(error.ends_with("..."));
        assert!(serialize_frame(&WorkerFrame::Response { response }).is_ok());
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

    #[tokio::test]
    async fn context_delivers_success_cancel_and_bounded_events() {
        let (context, mut output_rx) = WorkerStdioContext::test_context(WorkerKind::Asr);
        assert_eq!(context.kind, WorkerKind::Asr);
        assert_eq!(context.instance_id, "test-worker-instance");

        context
            .response("success".to_string(), Ok(serde_json::json!({"ok": true})))
            .await;
        context.cancelled_response("cancelled".to_string()).await;
        context
            .event(WorkerEvent {
                request_id: Some("event".to_string()),
                job_id: Some("job".to_string()),
                event: "progress".to_string(),
                payload: serde_json::json!({"step": 1}),
            })
            .await;
        assert!(context.try_event(WorkerEvent {
            request_id: Some("try-event".to_string()),
            job_id: None,
            event: "progress".to_string(),
            payload: serde_json::json!({"step": 2}),
        }));
        assert!(!context.try_event(WorkerEvent {
            request_id: Some("oversized".to_string()),
            job_id: None,
            event: "progress".to_string(),
            payload: serde_json::json!({"data": "x".repeat(WORKER_MAX_FRAME_BYTES)}),
        }));

        let WorkerFrame::Response { response } = output_rx.recv().await.unwrap() else {
            panic!("expected success response");
        };
        assert!(response.ok);
        assert_eq!(response.payload["ok"], true);
        let WorkerFrame::Response { response } = output_rx.recv().await.unwrap() else {
            panic!("expected cancelled response");
        };
        assert!(response.cancelled);
        for expected_request_id in ["event", "try-event"] {
            let WorkerFrame::Event { event } = output_rx.recv().await.unwrap() else {
                panic!("expected event");
            };
            assert_eq!(event.request_id.as_deref(), Some(expected_request_id));
        }
    }

    #[tokio::test]
    async fn abort_helpers_and_frame_identity_cover_empty_and_populated_jobs() {
        let request = ParentFrame::Request {
            request: super::super::protocol::WorkerRequest {
                request_id: "request".to_string(),
                job_id: Some("logical-job".to_string()),
                deadline_unix_ms: None,
                operation: "test".to_string(),
                payload: serde_json::Value::Null,
            },
        };
        assert_eq!(frame_request_id(&request).as_deref(), Some("request"));
        assert_eq!(frame_job_id(&request).as_deref(), Some("logical-job"));
        for frame in [
            ParentFrame::ConfigApply {
                request_id: "config".to_string(),
                generation: 1,
                payload: serde_json::Value::Null,
            },
            ParentFrame::Ping {
                request_id: "ping".to_string(),
            },
            ParentFrame::Shutdown {
                request_id: "shutdown".to_string(),
            },
            ParentFrame::Cancel {
                request_id: "cancel".to_string(),
                job_id: None,
            },
        ] {
            assert!(frame_request_id(&frame).is_some());
            assert!(frame_job_id(&frame).is_none());
        }

        let mut jobs = HashMap::new();
        assert!(abort_request(&mut jobs, "missing").await.is_empty());
        assert!(abort_job_by_id(&mut jobs, "missing").await.is_empty());
        jobs.insert(
            "request".to_string(),
            RunningJob {
                job_id: None,
                handle: tokio::spawn(std::future::pending::<()>()),
            },
        );
        assert_eq!(abort_request(&mut jobs, "request").await, ["request"]);
        jobs.insert(
            "request-a".to_string(),
            RunningJob {
                job_id: Some("shared".to_string()),
                handle: tokio::spawn(std::future::pending::<()>()),
            },
        );
        jobs.insert(
            "request-b".to_string(),
            RunningJob {
                job_id: Some("shared".to_string()),
                handle: tokio::spawn(std::future::pending::<()>()),
            },
        );
        let mut aborted = abort_job_by_id(&mut jobs, "shared").await;
        aborted.sort();
        assert_eq!(aborted, ["request-a", "request-b"]);
        assert!(jobs.is_empty());
    }

    #[test]
    fn active_guard_and_panic_messages_cover_all_payload_shapes() {
        let counter = AtomicUsize::new(0);
        let guard = ActiveJobGuard::increment(&counter);
        assert_eq!(counter.load(Ordering::Acquire), 1);
        drop(guard);
        assert_eq!(counter.load(Ordering::Acquire), 0);

        assert!(panic_message(Box::new("borrowed panic")).contains("borrowed panic"));
        assert!(panic_message(Box::new("owned panic".to_string())).contains("owned panic"));
        assert!(panic_message(Box::new(7_u8)).contains("non-string"));
    }

    #[tokio::test]
    async fn input_loop_covers_deadlines_failures_panics_duplicates_cancellation_and_limits() {
        let (context, mut output) = WorkerStdioContext::test_context(WorkerKind::Browser);
        let (input_tx, input_rx) = mpsc::channel(256);
        let handler = Arc::new(
            |frame: ParentFrame, context: Arc<WorkerStdioContext>| async move {
                match frame {
                    ParentFrame::Request { request } => match request.operation.as_str() {
                        "ok" => {
                            context
                                .response(
                                    request.request_id,
                                    Ok(serde_json::json!({"handled": true})),
                                )
                                .await;
                            Ok(())
                        }
                        "fail" => Err("injected handler failure".to_string()),
                        "panic" => panic!("injected handler panic"),
                        "pending" => std::future::pending::<Result<(), String>>().await,
                        operation => Err(format!("unexpected operation {operation}")),
                    },
                    ParentFrame::Cancel { .. } => Err("injected cancel failure".to_string()),
                    ParentFrame::Shutdown { .. } => Err("injected shutdown failure".to_string()),
                    ParentFrame::Ping { request_id } => {
                        context
                            .response(request_id, Ok(serde_json::json!({"pong": true})))
                            .await;
                        Ok(())
                    }
                    ParentFrame::ConfigApply { request_id, .. } => {
                        context
                            .response(request_id, Ok(serde_json::json!({"applied": true})))
                            .await;
                        Ok(())
                    }
                }
            },
        );
        let loop_task = tokio::spawn(process_worker_input(input_rx, context.clone(), handler));

        input_tx
            .send(ParentFrame::Ping {
                request_id: "ping".to_string(),
            })
            .await
            .unwrap();
        assert!(response_for(&mut output, "ping").await.ok);

        let mut expired = request_frame("expired", None, "ok");
        let ParentFrame::Request { request } = &mut expired else {
            unreachable!();
        };
        request.deadline_unix_ms = Some(now_ms());
        input_tx.send(expired).await.unwrap();
        let expired = response_for(&mut output, "expired").await;
        assert!(expired.error.unwrap().contains("deadline expired"));

        for (request_id, operation, expected_error) in [
            ("ok", "ok", None),
            ("failed", "fail", Some("injected handler failure")),
            ("panicked", "panic", Some("injected handler panic")),
        ] {
            input_tx
                .send(request_frame(request_id, None, operation))
                .await
                .unwrap();
            let response = response_for(&mut output, request_id).await;
            if let Some(expected_error) = expected_error {
                assert!(response.error.unwrap().contains(expected_error));
            } else {
                assert!(response.ok);
            }
        }

        input_tx
            .send(request_frame("duplicate", Some("duplicate-job"), "pending"))
            .await
            .unwrap();
        wait_for_active(&context, 1).await;
        input_tx
            .send(request_frame("duplicate", Some("duplicate-job"), "pending"))
            .await
            .unwrap();
        assert!(response_for(&mut output, "duplicate")
            .await
            .error
            .unwrap()
            .contains("duplicate"));
        input_tx
            .send(ParentFrame::Cancel {
                request_id: "duplicate".to_string(),
                job_id: None,
            })
            .await
            .unwrap();
        assert!(response_for(&mut output, "duplicate").await.cancelled);

        for request_id in ["shared-a", "shared-b"] {
            input_tx
                .send(request_frame(request_id, Some("shared-job"), "pending"))
                .await
                .unwrap();
        }
        wait_for_active(&context, 2).await;
        input_tx
            .send(ParentFrame::Cancel {
                request_id: "cancel-shared".to_string(),
                job_id: Some("shared-job".to_string()),
            })
            .await
            .unwrap();
        let shared_responses = responses_for(&mut output, &["shared-a", "shared-b"]).await;
        assert!(shared_responses["shared-a"].cancelled);
        assert!(shared_responses["shared-b"].cancelled);

        for index in 0..WORKER_MAX_IN_FLIGHT_REQUESTS {
            input_tx
                .send(request_frame(&format!("limit-{index}"), None, "pending"))
                .await
                .unwrap();
        }
        wait_for_active(&context, WORKER_MAX_IN_FLIGHT_REQUESTS).await;
        input_tx
            .send(request_frame("over-limit", None, "pending"))
            .await
            .unwrap();
        assert!(response_for(&mut output, "over-limit")
            .await
            .error
            .unwrap()
            .contains("in-flight request limit"));

        input_tx
            .send(ParentFrame::Shutdown {
                request_id: "shutdown".to_string(),
            })
            .await
            .unwrap();
        let shutdown = response_for(&mut output, "shutdown").await;
        assert!(shutdown.ok);
        assert_eq!(shutdown.payload["stopping"], true);
        loop_task.await.unwrap();
        assert!(context.shutdown.load(Ordering::Acquire));
        assert_eq!(context.active_jobs.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn input_loop_treats_channel_close_as_implicit_shutdown() {
        let (context, _output) = WorkerStdioContext::test_context(WorkerKind::Asr);
        let (input_tx, input_rx) = mpsc::channel(1);
        let shutdown_seen = Arc::new(AtomicBool::new(false));
        let seen = shutdown_seen.clone();
        let handler = Arc::new(
            move |frame: ParentFrame, _context: Arc<WorkerStdioContext>| {
                let seen = seen.clone();
                async move {
                    if matches!(frame, ParentFrame::Shutdown { .. }) {
                        seen.store(true, Ordering::Release);
                    }
                    Err("implicit shutdown failure is best effort".to_string())
                }
            },
        );
        drop(input_tx);

        process_worker_input(input_rx, context.clone(), handler).await;

        assert!(shutdown_seen.load(Ordering::Acquire));
        assert!(context.shutdown.load(Ordering::Acquire));
    }
}
