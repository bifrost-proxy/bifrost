use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use bifrost_core::Result as BifrostResult;
use dashmap::DashMap;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Mutex};

use crate::remote_invoke::types::{FileAccessScope, RemoteCommand};
use crate::remote_invoke::{RemoteInvokeExecutor, RemoteInvokeResponse};

use super::{
    run_worker_stdio, ManagedWorker, ParentFrame, WorkerEvent, WorkerKind, WorkerSpawnSpec,
    WorkerStdioContext,
};

const REMOTE_EXECUTION_PARENT_ENV: &str = "BIFROST_REMOTE_INVOKE_WORKER";
const REMOTE_EXECUTION_CHUNK_BYTES: usize = 48 * 1024;
const REMOTE_EXECUTION_INPUT_CHUNK_BYTES: usize = 64 * 1024;
const REMOTE_EXECUTION_INPUT_QUEUE: usize = 32;
const REMOTE_EXECUTION_EVENT: &str = "remote_execution.stdout";

static ACTIVE_EXECUTION_WORKERS: Lazy<DashMap<String, Arc<ManagedWorker>>> =
    Lazy::new(DashMap::new);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoteExecutionEnvelope {
    command: RemoteCommand,
    #[serde(default)]
    grant_id: Option<String>,
    #[serde(default)]
    caller_fingerprint: Option<String>,
    #[serde(default)]
    ssh_fingerprint: Option<String>,
    #[serde(default)]
    file_access: FileAccessScope,
}

impl RemoteExecutionEnvelope {
    pub(crate) fn from_command(command: &RemoteCommand) -> Self {
        Self {
            command: command.clone(),
            grant_id: command.grant_id.clone(),
            caller_fingerprint: command.caller_fingerprint.clone(),
            ssh_fingerprint: command.ssh_fingerprint.clone(),
            file_access: command.file_access,
        }
    }

    pub(crate) fn into_command(mut self) -> RemoteCommand {
        self.command.grant_id = self.grant_id;
        self.command.caller_fingerprint = self.caller_fingerprint;
        self.command.ssh_fingerprint = self.ssh_fingerprint;
        self.command.file_access = self.file_access;
        self.command
    }

    pub(crate) fn grant_id(&self) -> Option<&str> {
        self.grant_id.as_deref()
    }

    pub(crate) fn caller_fingerprint(&self) -> Option<&str> {
        self.caller_fingerprint.as_deref()
    }

    pub(crate) fn ssh_fingerprint(&self) -> Option<&str> {
        self.ssh_fingerprint.as_deref()
    }

    pub(crate) fn file_access(&self) -> FileAccessScope {
        self.file_access
    }

    pub(crate) fn command_kind(&self) -> crate::remote_invoke::types::CommandKind {
        self.command.kind
    }
}

#[derive(Default)]
struct RemoteExecutionRuntime {
    inputs: Mutex<HashMap<String, PreparedInput>>,
}

struct PreparedInput {
    sender: Option<mpsc::Sender<Vec<u8>>>,
    receiver: Option<mpsc::Receiver<Vec<u8>>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecutionIdPayload {
    execution_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecutionInputPayload {
    execution_id: String,
    data_base64: String,
}

struct WorkerShutdownGuard {
    worker: Option<Arc<ManagedWorker>>,
    worker_key: String,
    stderr_path: PathBuf,
}

impl WorkerShutdownGuard {
    fn new(worker: Arc<ManagedWorker>, stderr_path: PathBuf) -> Self {
        let worker_key = worker.key().to_string();
        ACTIVE_EXECUTION_WORKERS.insert(worker_key.clone(), worker.clone());
        Self {
            worker: Some(worker),
            worker_key,
            stderr_path,
        }
    }

    fn disarm(&mut self) {
        ACTIVE_EXECUTION_WORKERS.remove(&self.worker_key);
        self.worker = None;
    }
}

impl Drop for WorkerShutdownGuard {
    fn drop(&mut self) {
        ACTIVE_EXECUTION_WORKERS.remove(&self.worker_key);
        let Some(worker) = self.worker.take() else {
            return;
        };
        let stderr_path = self.stderr_path.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = worker.shutdown(Duration::from_secs(1)).await;
                let _ = tokio::fs::remove_file(stderr_path).await;
            });
        } else {
            let _ = std::fs::remove_file(stderr_path);
        }
    }
}

pub(crate) async fn cancel_registered_execution(
    worker_key: &str,
    request_id: &str,
    logical_job_id: &str,
) -> Result<bool, String> {
    let Some(worker) = ACTIVE_EXECUTION_WORKERS
        .get(worker_key)
        .map(|entry| entry.clone())
    else {
        return Ok(false);
    };
    worker.cancel_request(request_id, logical_job_id).await
}

pub(crate) fn should_isolate_remote_execution() -> bool {
    super::remote_invoke::is_remote_invoke_worker_process()
        && super::worker_execution_enabled(WorkerKind::RemoteExecution)
}

pub(crate) async fn execute_remote_command<F, Fut>(
    command: &RemoteCommand,
    admin_host: &str,
    admin_port: u16,
    stdin_rx: Option<mpsc::Receiver<Vec<u8>>>,
    on_stdout: &mut F,
) -> Result<RemoteInvokeResponse, String>
where
    F: FnMut(Vec<u8>) -> Fut,
    Fut: Future<Output = BifrostResult<()>>,
{
    let execution_id = uuid::Uuid::new_v4().to_string();
    let spec = spawn_spec(&execution_id, admin_host, admin_port, command.timeout_ms)?;
    let stderr_path = spec.stderr_path.clone().unwrap_or_else(|| {
        remote_execution_runtime_root().join(format!("{execution_id}.stderr.log"))
    });
    let worker = ManagedWorker::spawn(spec).await?;
    let mut shutdown_guard = WorkerShutdownGuard::new(worker.clone(), stderr_path.clone());

    worker
        .request(
            "remote_execution.prepare",
            serde_json::json!({ "executionId": execution_id }),
            Some(Duration::from_secs(10)),
        )
        .await?;

    let input_worker = worker.clone();
    let input_execution_id = execution_id.clone();
    let mut input_task =
        tokio::spawn(
            async move { forward_stdin(input_worker, input_execution_id, stdin_rx).await },
        );
    let mut input_done = false;

    let timeout = Duration::from_millis(
        command
            .timeout_ms
            .unwrap_or(120_000)
            .clamp(1_000, 24 * 60 * 60 * 1_000)
            .saturating_add(30_000),
    );
    let envelope = serde_json::to_value(RemoteExecutionEnvelope::from_command(command))
        .map_err(|error| format!("serialize remote execution request: {error}"))?;
    let run_request_id = format!("remote-execution-{execution_id}");
    let mut events = worker.subscribe_request_events(run_request_id.clone(), 32);
    let run_future = worker.request_with_id(
        run_request_id.clone(),
        Some(execution_id.clone()),
        "remote_execution.run",
        envelope,
        Some(timeout),
    );
    tokio::pin!(run_future);

    let run_value = loop {
        tokio::select! {
            result = &mut run_future => break result?,
            input_result = &mut input_task, if !input_done => {
                input_done = true;
                match input_result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        let _ = worker.cancel_request(&run_request_id, &execution_id).await;
                        return Err(error);
                    }
                    Err(error) => {
                        let _ = worker.cancel_request(&run_request_id, &execution_id).await;
                        return Err(format!("remote execution stdin task failed: {error}"));
                    }
                }
            }
            event = events.recv() => {
                handle_stdout_event(event, &execution_id, on_stdout).await?;
            }
        }
    };

    while let Ok(event) = events.try_recv() {
        handle_stdout_event(Some(event), &execution_id, on_stdout).await?;
    }
    worker.remove_request_event_sink(&run_request_id);
    if !input_done {
        input_task.abort();
        let _ = input_task.await;
    }

    let response: RemoteInvokeResponse = serde_json::from_value(run_value)
        .map_err(|error| format!("parse remote execution response: {error}"))?;
    worker.shutdown(Duration::from_secs(3)).await?;
    let _ = tokio::fs::remove_file(stderr_path).await;
    shutdown_guard.disarm();
    Ok(response)
}

async fn handle_stdout_event<F, Fut>(
    event: Option<WorkerEvent>,
    execution_id: &str,
    on_stdout: &mut F,
) -> Result<(), String>
where
    F: FnMut(Vec<u8>) -> Fut,
    Fut: Future<Output = BifrostResult<()>>,
{
    let event = event.ok_or_else(|| "remote execution event channel closed".to_string())?;
    if event.event != REMOTE_EXECUTION_EVENT || event.job_id.as_deref() != Some(execution_id) {
        return Ok(());
    }
    let encoded = event
        .payload
        .get("dataBase64")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "remote execution stdout event is missing dataBase64".to_string())?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| format!("decode remote execution stdout: {error}"))?;
    on_stdout(bytes).await.map_err(|error| error.to_string())
}

async fn forward_stdin(
    worker: Arc<ManagedWorker>,
    execution_id: String,
    mut stdin_rx: Option<mpsc::Receiver<Vec<u8>>>,
) -> Result<(), String> {
    if let Some(receiver) = stdin_rx.as_mut() {
        while let Some(chunk) = receiver.recv().await {
            for part in chunk.chunks(REMOTE_EXECUTION_INPUT_CHUNK_BYTES) {
                worker
                    .request(
                        "remote_execution.stdin",
                        serde_json::json!({
                            "executionId": execution_id,
                            "dataBase64": base64::engine::general_purpose::STANDARD.encode(part),
                        }),
                        Some(Duration::from_secs(10)),
                    )
                    .await?;
            }
        }
    }
    worker
        .request(
            "remote_execution.stdin_close",
            serde_json::json!({ "executionId": execution_id }),
            Some(Duration::from_secs(10)),
        )
        .await?;
    Ok(())
}

fn spawn_spec(
    execution_id: &str,
    admin_host: &str,
    admin_port: u16,
    timeout_ms: Option<u64>,
) -> Result<WorkerSpawnSpec, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("resolve Remote Execution worker executable: {error}"))?;
    let data_dir = bifrost_storage::data_dir();
    let mut spec = WorkerSpawnSpec::new(
        format!("remote_execution:{execution_id}"),
        WorkerKind::RemoteExecution,
        executable,
        vec![
            "auxiliary-worker".to_string(),
            "--kind".to_string(),
            "remote_execution".to_string(),
            "--data-dir".to_string(),
            data_dir.display().to_string(),
            "--admin-host".to_string(),
            admin_host.to_string(),
            "--admin-port".to_string(),
            admin_port.to_string(),
        ],
    );
    spec.env
        .insert(REMOTE_EXECUTION_PARENT_ENV.to_string(), "0".to_string());
    spec.env_remove.extend([
        "BIFROST_REMOTE_SESSION_TOKEN".to_string(),
        "BIFROST_REMOTE_WORKER_HTTP_TOKEN".to_string(),
        super::remote_broker::BROKER_ADDR_ENV.to_string(),
        super::remote_broker::BROKER_TOKEN_ENV.to_string(),
        super::remote_broker::BROKER_RELAY_ENV.to_string(),
    ]);
    spec.max_concurrency = 8;
    spec.max_queue_depth = 64;
    spec.startup_timeout = Duration::from_secs(15);
    spec.request_timeout = Duration::from_millis(
        timeout_ms
            .unwrap_or(120_000)
            .clamp(1_000, 24 * 60 * 60 * 1_000)
            .saturating_add(30_000),
    );
    spec.queue_wait_timeout = Duration::from_secs(10);
    spec.heartbeat_timeout = Duration::from_secs(45);
    spec.stderr_path =
        Some(remote_execution_runtime_root().join(format!("{execution_id}.stderr.log")));
    Ok(spec)
}

fn remote_execution_runtime_root() -> PathBuf {
    bifrost_storage::data_dir()
        .join("runtime")
        .join("workers")
        .join("remote-execution")
}

pub fn run_remote_execution_worker_stdio(admin_host: &str, admin_port: u16) -> Result<(), String> {
    let admin_host = admin_host.to_string();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("bifrost-remote-execution-worker")
        .build()
        .map_err(|error| format!("build Remote Execution worker runtime: {error}"))?;
    runtime.block_on(async move {
        let state = Arc::new(RemoteExecutionRuntime::default());
        run_worker_stdio(
            WorkerKind::RemoteExecution,
            vec![
                "remote_execution.prepare".to_string(),
                "remote_execution.run".to_string(),
                "remote_execution.stdin".to_string(),
                "remote_execution.stdin_close".to_string(),
            ],
            move |frame, context| {
                let state = state.clone();
                let admin_host = admin_host.clone();
                async move {
                    handle_worker_frame(frame, context, state, &admin_host, admin_port).await
                }
            },
        )
        .await
    })
}

async fn handle_worker_frame(
    frame: ParentFrame,
    context: Arc<WorkerStdioContext>,
    state: Arc<RemoteExecutionRuntime>,
    admin_host: &str,
    admin_port: u16,
) -> Result<(), String> {
    match frame {
        ParentFrame::Request { request } => match request.operation.as_str() {
            "remote_execution.prepare" => {
                let payload: ExecutionIdPayload = serde_json::from_value(request.payload)
                    .map_err(|error| format!("parse execution prepare payload: {error}"))?;
                validate_execution_id(&payload.execution_id)?;
                let (sender, receiver) = mpsc::channel(REMOTE_EXECUTION_INPUT_QUEUE);
                state.inputs.lock().await.insert(
                    payload.execution_id,
                    PreparedInput {
                        sender: Some(sender),
                        receiver: Some(receiver),
                    },
                );
                context
                    .response(
                        request.request_id,
                        Ok(serde_json::json!({ "prepared": true })),
                    )
                    .await;
            }
            "remote_execution.stdin" => {
                let payload: ExecutionInputPayload = serde_json::from_value(request.payload)
                    .map_err(|error| format!("parse execution stdin payload: {error}"))?;
                validate_execution_id(&payload.execution_id)?;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(payload.data_base64)
                    .map_err(|error| format!("decode execution stdin: {error}"))?;
                if bytes.len() > REMOTE_EXECUTION_INPUT_CHUNK_BYTES {
                    return Err("remote execution stdin chunk exceeds hard limit".to_string());
                }
                let sender = state
                    .inputs
                    .lock()
                    .await
                    .get(&payload.execution_id)
                    .and_then(|input| input.sender.clone())
                    .ok_or_else(|| "remote execution stdin is not prepared".to_string())?;
                tokio::time::timeout(Duration::from_secs(5), sender.send(bytes))
                    .await
                    .map_err(|_| "remote execution stdin queue timeout".to_string())?
                    .map_err(|_| "remote execution stdin is closed".to_string())?;
                context
                    .response(
                        request.request_id,
                        Ok(serde_json::json!({ "accepted": true })),
                    )
                    .await;
            }
            "remote_execution.stdin_close" => {
                let payload: ExecutionIdPayload = serde_json::from_value(request.payload)
                    .map_err(|error| format!("parse execution close payload: {error}"))?;
                validate_execution_id(&payload.execution_id)?;
                if let Some(input) = state.inputs.lock().await.get_mut(&payload.execution_id) {
                    input.sender = None;
                }
                context
                    .response(
                        request.request_id,
                        Ok(serde_json::json!({ "closed": true })),
                    )
                    .await;
            }
            "remote_execution.run" => {
                let execution_id = request
                    .job_id
                    .clone()
                    .ok_or_else(|| "remote execution run requires job id".to_string())?;
                validate_execution_id(&execution_id)?;
                let envelope: RemoteExecutionEnvelope = serde_json::from_value(request.payload)
                    .map_err(|error| format!("parse remote execution request: {error}"))?;
                let receiver = state
                    .inputs
                    .lock()
                    .await
                    .get_mut(&execution_id)
                    .and_then(|input| input.receiver.take())
                    .ok_or_else(|| "remote execution input is not prepared".to_string())?;
                let executor = RemoteInvokeExecutor::new(admin_host, admin_port);
                let event_context = context.clone();
                let event_request_id = request.request_id.clone();
                let event_execution_id = execution_id.clone();
                let result = executor
                    .execute_with_stdout_sink(&envelope.into_command(), Some(receiver), move |chunk| {
                        let context = event_context.clone();
                        let request_id = event_request_id.clone();
                        let execution_id = event_execution_id.clone();
                        async move {
                            for part in chunk.chunks(REMOTE_EXECUTION_CHUNK_BYTES) {
                                context
                                    .event(WorkerEvent {
                                        request_id: Some(request_id.clone()),
                                        job_id: Some(execution_id.clone()),
                                        event: REMOTE_EXECUTION_EVENT.to_string(),
                                        payload: serde_json::json!({
                                            "dataBase64": base64::engine::general_purpose::STANDARD.encode(part),
                                        }),
                                    })
                                    .await;
                            }
                            Ok(())
                        }
                    })
                    .await
                    .map_err(|error| error.to_string());
                state.inputs.lock().await.remove(&execution_id);
                context
                    .response(
                        request.request_id,
                        result.and_then(|response| {
                            serde_json::to_value(response).map_err(|error| {
                                format!("serialize remote execution response: {error}")
                            })
                        }),
                    )
                    .await;
            }
            other => return Err(format!("unsupported Remote Execution operation '{other}'")),
        },
        ParentFrame::Cancel { job_id, .. } => {
            if let Some(job_id) = job_id {
                state.inputs.lock().await.remove(&job_id);
            }
        }
        ParentFrame::Shutdown { .. } => {
            state.inputs.lock().await.clear();
        }
        ParentFrame::ConfigApply { request_id, .. } => {
            context
                .response(
                    request_id,
                    Err("Remote Execution worker has no mutable configuration".to_string()),
                )
                .await;
        }
        ParentFrame::Ping { .. } => {}
    }
    Ok(())
}

fn validate_execution_id(value: &str) -> Result<(), String> {
    if value.len() > 128
        || value.is_empty()
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
    {
        return Err("invalid remote execution id".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker_runtime::{WorkerFrame, WorkerLifecycleState, WorkerRequest};

    async fn dispatch_worker_frame(
        frame: ParentFrame,
        context: Arc<WorkerStdioContext>,
        state: Arc<RemoteExecutionRuntime>,
        admin_host: &str,
        admin_port: u16,
    ) -> Result<(), String> {
        let admin_host = admin_host.to_string();
        tokio::spawn(async move {
            handle_worker_frame(frame, context, state, &admin_host, admin_port).await
        })
        .await
        .expect("Remote Execution worker dispatch task")
    }

    #[test]
    fn execution_envelope_preserves_runtime_security_context() {
        let command = RemoteCommand {
            grant_id: Some("grant-1".to_string()),
            caller_fingerprint: Some("caller-1".to_string()),
            ssh_fingerprint: Some("ssh-1".to_string()),
            file_access: FileAccessScope::ReadWrite,
            ..Default::default()
        };
        let restored = RemoteExecutionEnvelope::from_command(&command).into_command();
        assert_eq!(restored.grant_id.as_deref(), Some("grant-1"));
        assert_eq!(restored.caller_fingerprint.as_deref(), Some("caller-1"));
        assert_eq!(restored.ssh_fingerprint.as_deref(), Some("ssh-1"));
        assert_eq!(restored.file_access, FileAccessScope::ReadWrite);
    }

    #[test]
    fn execution_id_rejects_path_like_values() {
        assert!(validate_execution_id("../../escape").is_err());
        assert!(validate_execution_id("valid-id-1").is_ok());
    }

    #[test]
    fn execution_id_spawn_and_isolation_contract_are_bounded() {
        for invalid in ["", "bad_value", "x/y"] {
            assert!(validate_execution_id(invalid).is_err());
        }
        assert!(validate_execution_id(&"x".repeat(129)).is_err());

        let spec = spawn_spec("execution-1", "127.0.0.1", 9876, Some(1)).unwrap();
        assert_eq!(spec.key, "remote_execution:execution-1");
        assert_eq!(spec.kind, WorkerKind::RemoteExecution);
        assert_eq!(spec.max_concurrency, 8);
        assert_eq!(spec.max_queue_depth, 64);
        assert_eq!(spec.request_timeout, Duration::from_secs(31));
        assert_eq!(
            spec.env
                .get(REMOTE_EXECUTION_PARENT_ENV)
                .map(String::as_str),
            Some("0")
        );
        assert!(spec
            .env_remove
            .iter()
            .any(|value| value == "BIFROST_REMOTE_SESSION_TOKEN"));
        assert!(spec
            .stderr_path
            .unwrap()
            .ends_with("execution-1.stderr.log"));
        assert!(remote_execution_runtime_root().ends_with("workers/remote-execution"));
        let _ = should_isolate_remote_execution();
    }

    #[tokio::test]
    async fn stdout_events_validate_identity_payload_and_consumer_errors() {
        let event = |job_id: Option<&str>, event: &str, payload: serde_json::Value| WorkerEvent {
            request_id: Some("request".to_string()),
            job_id: job_id.map(ToString::to_string),
            event: event.to_string(),
            payload,
        };
        let mut output = Vec::new();
        handle_stdout_event(
            Some(event(
                Some("execution"),
                REMOTE_EXECUTION_EVENT,
                serde_json::json!({
                    "dataBase64": base64::engine::general_purpose::STANDARD.encode(b"stdout")
                }),
            )),
            "execution",
            &mut |chunk| {
                output.extend(chunk);
                std::future::ready(Ok(()))
            },
        )
        .await
        .unwrap();
        assert_eq!(output, b"stdout");

        for ignored in [
            event(Some("other"), REMOTE_EXECUTION_EVENT, serde_json::json!({})),
            event(Some("execution"), "other", serde_json::json!({})),
        ] {
            handle_stdout_event(Some(ignored), "execution", &mut |_| {
                std::future::ready(Ok(()))
            })
            .await
            .unwrap();
        }
        assert!(
            handle_stdout_event(None, "execution", &mut |_| { std::future::ready(Ok(())) })
                .await
                .unwrap_err()
                .contains("channel closed")
        );
        assert!(handle_stdout_event(
            Some(event(
                Some("execution"),
                REMOTE_EXECUTION_EVENT,
                serde_json::json!({})
            )),
            "execution",
            &mut |_| std::future::ready(Ok(())),
        )
        .await
        .unwrap_err()
        .contains("missing dataBase64"));
        assert!(handle_stdout_event(
            Some(event(
                Some("execution"),
                REMOTE_EXECUTION_EVENT,
                serde_json::json!({"dataBase64": "%%%"})
            )),
            "execution",
            &mut |_| std::future::ready(Ok(())),
        )
        .await
        .unwrap_err()
        .contains("decode remote execution stdout"));
    }

    #[tokio::test]
    async fn worker_dispatch_covers_prepared_input_run_errors_and_controls() {
        let state = Arc::new(RemoteExecutionRuntime::default());
        let (context, mut output) = WorkerStdioContext::test_context(WorkerKind::RemoteExecution);
        let request =
            |id: &str, operation: &str, payload: serde_json::Value, job_id: Option<&str>| {
                ParentFrame::Request {
                    request: WorkerRequest {
                        request_id: id.to_string(),
                        job_id: job_id.map(ToString::to_string),
                        deadline_unix_ms: None,
                        operation: operation.to_string(),
                        payload,
                    },
                }
            };

        dispatch_worker_frame(
            request(
                "prepare",
                "remote_execution.prepare",
                serde_json::json!({"executionId": "exec-1"}),
                None,
            ),
            context.clone(),
            state.clone(),
            "127.0.0.1",
            0,
        )
        .await
        .unwrap();
        let WorkerFrame::Response { response } = output.recv().await.unwrap() else {
            panic!("expected prepare response")
        };
        assert_eq!(response.payload["prepared"], true);

        dispatch_worker_frame(
            request(
                "stdin",
                "remote_execution.stdin",
                serde_json::json!({
                    "executionId": "exec-1",
                    "dataBase64": base64::engine::general_purpose::STANDARD.encode(b"input")
                }),
                None,
            ),
            context.clone(),
            state.clone(),
            "127.0.0.1",
            0,
        )
        .await
        .unwrap();
        let WorkerFrame::Response { response } = output.recv().await.unwrap() else {
            panic!("expected stdin response")
        };
        assert_eq!(response.payload["accepted"], true);

        dispatch_worker_frame(
            request(
                "close",
                "remote_execution.stdin_close",
                serde_json::json!({"executionId": "exec-1"}),
                None,
            ),
            context.clone(),
            state.clone(),
            "127.0.0.1",
            0,
        )
        .await
        .unwrap();
        let WorkerFrame::Response { response } = output.recv().await.unwrap() else {
            panic!("expected close response")
        };
        assert_eq!(response.payload["closed"], true);

        for (frame, expected) in [
            (
                request(
                    "bad-base64",
                    "remote_execution.stdin",
                    serde_json::json!({"executionId": "exec-1", "dataBase64": "%%%"}),
                    None,
                ),
                "decode execution stdin",
            ),
            (
                request(
                    "unprepared",
                    "remote_execution.stdin",
                    serde_json::json!({"executionId": "missing", "dataBase64": ""}),
                    None,
                ),
                "not prepared",
            ),
            (
                request(
                    "run-no-job",
                    "remote_execution.run",
                    serde_json::json!({}),
                    None,
                ),
                "requires job id",
            ),
            (
                request(
                    "unknown",
                    "remote_execution.unknown",
                    serde_json::json!({}),
                    None,
                ),
                "unsupported",
            ),
        ] {
            let error =
                dispatch_worker_frame(frame, context.clone(), state.clone(), "127.0.0.1", 0)
                    .await
                    .unwrap_err();
            assert!(error.contains(expected), "{error}");
        }

        state.inputs.lock().await.insert(
            "cancel-me".to_string(),
            PreparedInput {
                sender: None,
                receiver: None,
            },
        );
        dispatch_worker_frame(
            ParentFrame::Cancel {
                request_id: "cancel".to_string(),
                job_id: Some("cancel-me".to_string()),
            },
            context.clone(),
            state.clone(),
            "127.0.0.1",
            0,
        )
        .await
        .unwrap();
        assert!(!state.inputs.lock().await.contains_key("cancel-me"));

        dispatch_worker_frame(
            ParentFrame::ConfigApply {
                request_id: "config".to_string(),
                generation: 1,
                payload: serde_json::Value::Null,
            },
            context.clone(),
            state.clone(),
            "127.0.0.1",
            0,
        )
        .await
        .unwrap();
        let WorkerFrame::Response { response } = output.recv().await.unwrap() else {
            panic!("expected config response")
        };
        assert!(response.error.unwrap().contains("no mutable configuration"));

        for frame in [
            ParentFrame::Ping {
                request_id: "ping".to_string(),
            },
            ParentFrame::Shutdown {
                request_id: "shutdown".to_string(),
            },
        ] {
            dispatch_worker_frame(frame, context.clone(), state.clone(), "127.0.0.1", 0)
                .await
                .unwrap();
        }
        assert!(state.inputs.lock().await.is_empty());
        assert!(!cancel_registered_execution("missing", "request", "job")
            .await
            .unwrap());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn parent_forwards_chunked_stdin_cancels_registered_runs_and_cleans_up() {
        let _jobs_guard = crate::worker_runtime::worker_jobs_test_guard_async().await;
        crate::worker_runtime::clear_worker_jobs_for_tests();
        let tail = r#"
run_id=''
while IFS= read -r line; do
  case "$line" in
    *'"type":"request"'*)
      request_id=$(printf '%s' "$line" | sed -n -e 's/.*"requestId":"\([^"]*\)".*/\1/p' -e 's/.*"request_id":"\([^"]*\)".*/\1/p')
      case "$line" in
        *'remote_execution.run'*) run_id="$request_id" ;;
        *) printf '{"type":"response","response":{"requestId":"%s","ok":true,"cancelled":false,"payload":{"accepted":true},"error":null}}\n' "$request_id" ;;
      esac
      ;;
    *'"type":"cancel"'*)
      printf '{"type":"response","response":{"requestId":"%s","ok":false,"cancelled":true,"payload":null,"error":"cancel acknowledged"}}\n' "$run_id"
      run_id=''
      ;;
    *'"type":"shutdown"'*)
      printf '{"type":"goodbye","worker_instance_id":"fake-instance","reason":"shutdown acknowledged"}\n'
      exit 0
      ;;
  esac
done
"#;
        let key = format!("remote_execution:test-{}", uuid::Uuid::new_v4());
        let mut spec =
            crate::worker_runtime::test_shell_worker_spec(&key, WorkerKind::RemoteExecution, tail);
        spec.request_timeout = Duration::from_secs(2);
        spec.heartbeat_timeout = Duration::from_secs(3);
        let worker = ManagedWorker::spawn(spec).await.unwrap();

        let (stdin_tx, stdin_rx) = mpsc::channel(1);
        stdin_tx
            .send(vec![b'x'; REMOTE_EXECUTION_INPUT_CHUNK_BYTES * 2 + 7])
            .await
            .unwrap();
        drop(stdin_tx);
        forward_stdin(
            worker.clone(),
            "execution-chunks".to_string(),
            Some(stdin_rx),
        )
        .await
        .unwrap();
        forward_stdin(worker.clone(), "execution-empty".to_string(), None)
            .await
            .unwrap();

        let temp = tempfile::tempdir().unwrap();
        let stderr_path = temp.path().join("worker.stderr.log");
        std::fs::write(&stderr_path, b"temporary worker log").unwrap();
        let shutdown_guard = WorkerShutdownGuard::new(worker.clone(), stderr_path.clone());
        let request_id = "registered-cancel-request".to_string();
        let request_worker = worker.clone();
        let pending = tokio::spawn(async move {
            request_worker
                .request_with_id(
                    request_id,
                    Some("registered-cancel-job".to_string()),
                    "remote_execution.run",
                    serde_json::Value::Null,
                    None,
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while crate::worker_runtime::worker_job("registered-cancel-request")
                .is_none_or(|job| job.status != crate::worker_runtime::WorkerJobStatus::Running)
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(cancel_registered_execution(
            &key,
            "registered-cancel-request",
            "registered-cancel-job"
        )
        .await
        .unwrap());
        assert_eq!(pending.await.unwrap().unwrap_err(), "cancel acknowledged");

        drop(shutdown_guard);
        tokio::time::timeout(Duration::from_secs(3), async {
            while stderr_path.exists() || worker.state() != WorkerLifecycleState::Stopped {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!ACTIVE_EXECUTION_WORKERS.contains_key(&key));
    }
}
