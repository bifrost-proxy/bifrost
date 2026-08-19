use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Semaphore};

use crate::im_gateway::external_cli::{
    ExternalCliGuideResult, ExternalCliModelUpdateResult, ExternalCliProgressEvent,
    ExternalCliRunRequest, ExternalCliRunResult, ExternalCliRuntime,
};

pub(crate) const BROKER_ADDR_ENV: &str = "BIFROST_IM_AGENT_BROKER_ADDR";
pub(crate) const BROKER_TOKEN_ENV: &str = "BIFROST_IM_AGENT_BROKER_TOKEN";
const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
const MAX_CONNECTIONS: usize = 16;

static SERVER: OnceLock<tokio::sync::Mutex<Option<BrokerServer>>> = OnceLock::new();

#[derive(Debug, Clone)]
pub(crate) struct BrokerEndpoint {
    pub addr: String,
    pub token: String,
}

struct BrokerServer {
    endpoint: BrokerEndpoint,
    task: tokio::task::JoinHandle<()>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BrokerRequest {
    Run {
        token: String,
        runs_root: String,
        request: Box<ExternalCliRunRequest>,
    },
    Guide {
        token: String,
        session_key: String,
        guide_id: String,
        message: String,
    },
    ModelUpdate {
        token: String,
        session_key: String,
        model: Option<String>,
    },
    Stop {
        token: String,
        runs_root: String,
        session_key: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BrokerResponse {
    Progress {
        event: ExternalCliProgressEvent,
    },
    Result {
        result: Box<ExternalCliRunResult>,
    },
    GuideResult {
        result: ExternalCliGuideResult,
    },
    ModelUpdateResult {
        result: ExternalCliModelUpdateResult,
    },
    StopResult {
        stopped: bool,
    },
    Error {
        error: String,
    },
}

/// Returns the terminal result that is safe to send over the broker wire.
///
/// Progress events are already delivered one frame at a time while the run is
/// active. Keeping the same in-memory event stream on the terminal result
/// duplicates potentially unbounded raw runner output and can make the final
/// frame exceed `MAX_FRAME_BYTES`. The full event history remains available to
/// the main process while the durable run artifact keeps its compact event
/// summaries.
fn terminal_result_for_broker(mut result: ExternalCliRunResult) -> ExternalCliRunResult {
    result.events.clear();
    result
}

impl BrokerRequest {
    fn token(&self) -> &str {
        match self {
            Self::Run { token, .. }
            | Self::Guide { token, .. }
            | Self::ModelUpdate { token, .. }
            | Self::Stop { token, .. } => token,
        }
    }
}

pub(crate) async fn ensure_main_broker() -> Result<BrokerEndpoint, String> {
    let mut server = broker_server().lock().await;
    if let Some(existing) = server.as_ref() {
        if !existing.task.is_finished() {
            return Ok(existing.endpoint.clone());
        }
    }
    if let Some(stale) = server.take() {
        stale.task.abort();
    }
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|error| format!("bind IM Agent main broker: {error}"))?;
    let endpoint = BrokerEndpoint {
        addr: listener
            .local_addr()
            .map_err(|error| format!("read IM Agent broker address: {error}"))?
            .to_string(),
        token: uuid::Uuid::new_v4().to_string(),
    };
    let endpoint_for_task = endpoint.clone();
    let connections = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let task = tokio::spawn(async move {
        loop {
            let (stream, peer) = match listener.accept().await {
                Ok(value) => value,
                Err(error) => {
                    tracing::warn!(error = %error, "IM Agent broker accept failed");
                    break;
                }
            };
            if !peer.ip().is_loopback() {
                tracing::warn!(peer = %peer, "rejected non-loopback IM Agent broker peer");
                continue;
            }
            let permit = match connections.clone().try_acquire_owned() {
                Ok(permit) => permit,
                Err(_) => {
                    tracing::warn!("IM Agent broker connection limit reached");
                    continue;
                }
            };
            let token = endpoint_for_task.token.clone();
            tokio::spawn(async move {
                let _permit = permit;
                if let Err(error) = serve_connection(stream, &token).await {
                    tracing::debug!(error = %error, "IM Agent broker connection closed");
                }
            });
        }
    });
    *server = Some(BrokerServer {
        endpoint: endpoint.clone(),
        task,
    });
    Ok(endpoint)
}

fn broker_server() -> &'static tokio::sync::Mutex<Option<BrokerServer>> {
    SERVER.get_or_init(|| tokio::sync::Mutex::new(None))
}

pub(crate) fn configure_worker_env(spec: &mut super::WorkerSpawnSpec, endpoint: &BrokerEndpoint) {
    spec.env
        .insert(BROKER_ADDR_ENV.to_string(), endpoint.addr.clone());
    spec.env
        .insert(BROKER_TOKEN_ENV.to_string(), endpoint.token.clone());
}

pub(crate) fn client_configured() -> bool {
    std::env::var(BROKER_ADDR_ENV).is_ok() && std::env::var(BROKER_TOKEN_ENV).is_ok()
}

pub(crate) async fn run_via_main_broker(
    runs_root: std::path::PathBuf,
    request: ExternalCliRunRequest,
    progress_tx: Option<mpsc::Sender<ExternalCliProgressEvent>>,
) -> Result<ExternalCliRunResult, String> {
    let addr = std::env::var(BROKER_ADDR_ENV)
        .map_err(|_| format!("{BROKER_ADDR_ENV} is required in IM Gateway worker"))?;
    let token = std::env::var(BROKER_TOKEN_ENV)
        .map_err(|_| format!("{BROKER_TOKEN_ENV} is required in IM Gateway worker"))?;
    let mut stream = TcpStream::connect(&addr)
        .await
        .map_err(|error| format!("connect IM Agent main broker {addr}: {error}"))?;
    write_frame(
        &mut stream,
        &BrokerRequest::Run {
            token,
            runs_root: runs_root.display().to_string(),
            request: Box::new(request),
        },
    )
    .await?;
    let mut reader = BufReader::new(stream);
    loop {
        let line = super::read_limited_async_line(&mut reader, MAX_FRAME_BYTES)
            .await
            .map_err(|error| format!("read IM Agent broker response: {error}"))?
            .ok_or_else(|| "IM Agent broker closed before result".to_string())?;
        match serde_json::from_str::<BrokerResponse>(&line)
            .map_err(|error| format!("parse IM Agent broker response: {error}"))?
        {
            BrokerResponse::Progress { event } => {
                if let Some(progress_tx) = progress_tx.as_ref() {
                    let _ = progress_tx.try_send(event);
                }
            }
            BrokerResponse::Result { result } => return Ok(*result),
            BrokerResponse::GuideResult { .. }
            | BrokerResponse::ModelUpdateResult { .. }
            | BrokerResponse::StopResult { .. } => {
                return Err("IM Agent broker returned an unexpected control response".to_string())
            }
            BrokerResponse::Error { error } => return Err(error),
        }
    }
}

pub(crate) async fn guide_via_main_broker(
    session_key: &str,
    guide_id: String,
    message: String,
) -> Result<ExternalCliGuideResult, String> {
    let response = request_control_via_main_broker(|token| BrokerRequest::Guide {
        token,
        session_key: session_key.to_string(),
        guide_id,
        message,
    })
    .await?;
    match response {
        BrokerResponse::GuideResult { result } => Ok(result),
        BrokerResponse::Error { error } => Err(error),
        _ => Err("IM Agent broker returned an unexpected guide response".to_string()),
    }
}

pub(crate) async fn model_update_via_main_broker(
    session_key: &str,
    model: Option<String>,
) -> Result<ExternalCliModelUpdateResult, String> {
    let response = request_control_via_main_broker(|token| BrokerRequest::ModelUpdate {
        token,
        session_key: session_key.to_string(),
        model,
    })
    .await?;
    match response {
        BrokerResponse::ModelUpdateResult { result } => Ok(result),
        BrokerResponse::Error { error } => Err(error),
        _ => Err("IM Agent broker returned an unexpected model update response".to_string()),
    }
}

pub(crate) async fn stop_via_main_broker(
    runs_root: &std::path::Path,
    session_key: &str,
) -> Result<bool, String> {
    let response = request_control_via_main_broker(|token| BrokerRequest::Stop {
        token,
        runs_root: runs_root.display().to_string(),
        session_key: session_key.to_string(),
    })
    .await?;
    match response {
        BrokerResponse::StopResult { stopped } => Ok(stopped),
        BrokerResponse::Error { error } => Err(error),
        _ => Err("IM Agent broker returned an unexpected stop response".to_string()),
    }
}

async fn request_control_via_main_broker(
    build_request: impl FnOnce(String) -> BrokerRequest,
) -> Result<BrokerResponse, String> {
    let addr = std::env::var(BROKER_ADDR_ENV)
        .map_err(|_| format!("{BROKER_ADDR_ENV} is required in IM Gateway worker"))?;
    let token = std::env::var(BROKER_TOKEN_ENV)
        .map_err(|_| format!("{BROKER_TOKEN_ENV} is required in IM Gateway worker"))?;
    let mut stream = TcpStream::connect(&addr)
        .await
        .map_err(|error| format!("connect IM Agent main broker {addr}: {error}"))?;
    write_frame(&mut stream, &build_request(token)).await?;
    let mut reader = BufReader::new(stream);
    let line = super::read_limited_async_line(&mut reader, MAX_FRAME_BYTES)
        .await
        .map_err(|error| format!("read IM Agent broker control response: {error}"))?
        .ok_or_else(|| "IM Agent broker closed before control response".to_string())?;
    serde_json::from_str::<BrokerResponse>(&line)
        .map_err(|error| format!("parse IM Agent broker control response: {error}"))
}

async fn serve_connection(stream: TcpStream, expected_token: &str) -> Result<(), String> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let line = super::read_limited_async_line(&mut reader, MAX_FRAME_BYTES)
        .await
        .map_err(|error| format!("read IM Agent broker request: {error}"))?
        .ok_or_else(|| "IM Agent broker peer closed before request".to_string())?;
    let request: BrokerRequest = serde_json::from_str(&line)
        .map_err(|error| format!("parse IM Agent broker request: {error}"))?;
    if request.token() != expected_token {
        write_frame(
            &mut write_half,
            &BrokerResponse::Error {
                error: "invalid IM Agent broker capability token".to_string(),
            },
        )
        .await?;
        return Ok(());
    }
    match request {
        BrokerRequest::Run {
            runs_root, request, ..
        } => serve_run_connection(reader, write_half, runs_root, *request).await,
        BrokerRequest::Guide {
            session_key,
            guide_id,
            message,
            ..
        } => {
            let rejected_guide_id = guide_id.clone();
            let response = match crate::im_gateway::external_cli::request_worker_session_guide(
                &session_key,
                guide_id,
                message,
            )
            .await
            {
                Ok(result) => BrokerResponse::GuideResult { result },
                Err(reason) => BrokerResponse::GuideResult {
                    result: ExternalCliGuideResult {
                        guide_id: rejected_guide_id,
                        accepted: false,
                        thread_id: None,
                        turn_id: None,
                        reason: Some(reason),
                    },
                },
            };
            write_frame(&mut write_half, &response).await
        }
        BrokerRequest::ModelUpdate {
            session_key, model, ..
        } => {
            let response =
                match crate::im_gateway::external_cli::request_worker_session_model_update(
                    &session_key,
                    model,
                )
                .await
                {
                    Ok(result) => BrokerResponse::ModelUpdateResult { result },
                    Err(error) => BrokerResponse::Error { error },
                };
            write_frame(&mut write_half, &response).await
        }
        BrokerRequest::Stop {
            runs_root,
            session_key,
            ..
        } => {
            let stopped = crate::im_gateway::external_cli::request_local_managed_session_stop(
                std::path::Path::new(&runs_root),
                &session_key,
            )
            .await;
            write_frame(&mut write_half, &BrokerResponse::StopResult { stopped }).await
        }
    }
}

async fn serve_run_connection(
    mut reader: BufReader<tokio::net::tcp::OwnedReadHalf>,
    mut write_half: tokio::net::tcp::OwnedWriteHalf,
    runs_root: String,
    request: ExternalCliRunRequest,
) -> Result<(), String> {
    let runs_root = std::path::PathBuf::from(runs_root);
    let runtime = ExternalCliRuntime::new(runs_root);
    let session_key = request.session_key.clone();
    let adapter = request.adapter.clone();
    let (progress_tx, mut progress_rx) = mpsc::channel(256);
    let mut run = Box::pin(tokio::spawn(async move {
        runtime.run_with_progress(request, Some(progress_tx)).await
    }));
    loop {
        tokio::select! {
            progress = progress_rx.recv() => {
                if let Some(event) = progress {
                    if let Err(error) = write_frame(&mut write_half, &BrokerResponse::Progress { event }).await {
                        run.abort();
                        cancel_brokered_session(session_key.as_deref(), &adapter).await;
                        return Err(error);
                    }
                }
            }
            peer_frame = super::read_limited_async_line(&mut reader, MAX_FRAME_BYTES) => {
                match peer_frame {
                    Ok(None) => {
                        run.abort();
                        cancel_brokered_session(session_key.as_deref(), &adapter).await;
                        return Err("IM Agent broker client disconnected".to_string());
                    }
                    Ok(Some(_)) => {
                        run.abort();
                        cancel_brokered_session(session_key.as_deref(), &adapter).await;
                        return Err("unexpected extra IM Agent broker request frame".to_string());
                    }
                    Err(error) => {
                        run.abort();
                        cancel_brokered_session(session_key.as_deref(), &adapter).await;
                        return Err(format!("read IM Agent broker client state: {error}"));
                    }
                }
            }
            result = &mut run => {
                let result = result.map_err(|error| format!("IM Agent broker task join failed: {error}"))?;
                // The command can finish in the same scheduler turn that its
                // final progress events reach this queue. Preserve wire order
                // by flushing those events before the terminal result frame.
                while let Ok(event) = progress_rx.try_recv() {
                    write_frame(&mut write_half, &BrokerResponse::Progress { event }).await?;
                }
                match result {
                    Ok(result) => write_frame(
                        &mut write_half,
                        &BrokerResponse::Result {
                            result: Box::new(terminal_result_for_broker(result)),
                        },
                    )
                    .await?,
                    Err(error) => write_frame(&mut write_half, &BrokerResponse::Error { error }).await?,
                }
                return Ok(());
            }
        }
    }
}

async fn cancel_brokered_session(session_key: Option<&str>, adapter: &str) {
    let Some(session_key) = session_key else {
        return;
    };
    if adapter == crate::im_gateway::chatgpt_web::ADAPTER_ID {
        let _ = crate::im_gateway::chatgpt_web::worker::stop_session_run(session_key).await;
    } else {
        let _ = crate::im_gateway::external_cli::request_worker_session_stop(session_key).await;
    }
}

async fn write_frame<W, T>(writer: &mut W, frame: &T) -> Result<(), String>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let mut bytes = serde_json::to_vec(frame)
        .map_err(|error| format!("serialize IM Agent broker frame: {error}"))?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(format!(
            "IM Agent broker frame exceeds {MAX_FRAME_BYTES} bytes"
        ));
    }
    bytes.push(b'\n');
    writer
        .write_all(&bytes)
        .await
        .map_err(|error| format!("write IM Agent broker frame: {error}"))?;
    writer
        .flush()
        .await
        .map_err(|error| format!("flush IM Agent broker frame: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn request() -> ExternalCliRunRequest {
        serde_json::from_value(serde_json::json!({ "message": "hello" })).unwrap()
    }

    fn result() -> ExternalCliRunResult {
        serde_json::from_value(serde_json::json!({
            "runId": "run-1",
            "sessionKey": "session-1",
            "runtime": "external_cli",
            "adapter": "codex",
            "status": "succeeded",
            "exitCode": 0,
            "response": "done",
            "startedAt": 1,
            "finishedAt": 2,
            "durationMs": 1,
            "artifacts": {
                "runDir": "", "prompt": "", "commandSnapshot": "",
                "stdout": "", "stderr": "", "normalizedEvents": "", "lastMessage": ""
            },
            "events": []
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn broker_terminal_result_discards_duplicate_live_events_and_preserves_delivery_fields() {
        let mut full_result = result();
        full_result.response = "final response".to_string();
        full_result.responses = vec!["thinking".to_string(), "final response".to_string()];
        full_result
            .metadata
            .insert("threadId".to_string(), "thread-1".to_string());
        full_result.events = (0..512)
            .map(|index| ExternalCliProgressEvent {
                event_type:
                    crate::im_gateway::external_cli::ExternalCliProgressEventType::AssistantDelta,
                content: "x".repeat(40 * 1024),
                title: Some(format!("event-{index}")),
                raw: serde_json::json!({ "payload": "x".repeat(1024) }),
            })
            .collect();

        let oversized = BrokerResponse::Result {
            result: Box::new(full_result.clone()),
        };
        assert!(serde_json::to_vec(&oversized).unwrap().len() > MAX_FRAME_BYTES);

        let terminal_result = terminal_result_for_broker(full_result);
        assert!(terminal_result.events.is_empty());
        assert_eq!(terminal_result.response, "final response");
        assert_eq!(terminal_result.responses, ["thinking", "final response"]);
        assert_eq!(
            terminal_result.metadata.get("threadId"),
            Some(&"thread-1".to_string())
        );
        assert_eq!(terminal_result.artifacts.run_dir, "");

        write_frame(
            &mut tokio::io::sink(),
            &BrokerResponse::Result {
                result: Box::new(terminal_result),
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn endpoint_and_worker_environment_are_stable() {
        let _lock = ENV_LOCK.lock().await;
        let endpoint = ensure_main_broker().await.unwrap();
        assert!(endpoint.addr.starts_with("127.0.0.1:"));
        assert!(!endpoint.token.is_empty());
        assert_eq!(ensure_main_broker().await.unwrap().addr, endpoint.addr);

        let mut spec = super::super::WorkerSpawnSpec::new(
            "im",
            super::super::WorkerKind::ImGateway,
            "bifrost",
            Vec::new(),
        );
        configure_worker_env(&mut spec, &endpoint);
        assert_eq!(spec.env.get(BROKER_ADDR_ENV), Some(&endpoint.addr));
        assert_eq!(spec.env.get(BROKER_TOKEN_ENV), Some(&endpoint.token));

        let stale = broker_server().lock().await.take().unwrap();
        stale.task.abort();
        let _ = stale.task.await;
        let recovered = ensure_main_broker().await.unwrap();
        assert_ne!(recovered.token, endpoint.token);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn main_broker_runs_external_cli_in_main_process_and_streams_progress() {
        let _lock = ENV_LOCK.lock().await;
        std::env::remove_var("BIFROST_IM_GATEWAY_WORKER");
        let temp = tempfile::tempdir().unwrap();
        let _data_dir = crate::test_env::BifrostDataDirGuard::set(temp.path());
        let endpoint = ensure_main_broker().await.unwrap();
        std::env::set_var(BROKER_ADDR_ENV, &endpoint.addr);
        std::env::set_var(BROKER_TOKEN_ENV, &endpoint.token);
        let request: ExternalCliRunRequest = serde_json::from_value(serde_json::json!({
            "message": "run through the main-process broker",
            "adapter": "mock",
            "sessionKey": "brokered-im-session",
            "adapterConfig": {
                "executable": "sh",
                "args": [
                    "-c",
                    "cat >/dev/null; printf '%s\\n' '{\"type\":\"assistant_delta\",\"delta\":\"broker working\"}' '{\"type\":\"assistant_final\",\"content\":\"broker done\"}'"
                ],
                "timeoutSecs": 10
            }
        }))
        .unwrap();
        let (progress_tx, mut progress_rx) = mpsc::channel(8);

        let result = run_via_main_broker(temp.path().join("runs"), request, Some(progress_tx))
            .await
            .unwrap();

        assert!(matches!(
            result.status,
            crate::im_gateway::external_cli::ExternalCliRunStatus::Succeeded
        ));
        assert_eq!(result.response, "broker done");
        let mut contents = Vec::new();
        while let Ok(event) = progress_rx.try_recv() {
            contents.push(event.content);
        }
        assert!(contents
            .iter()
            .any(|content| content.contains("broker working")));
        assert!(std::path::Path::new(&result.artifacts.run_dir).is_dir());

        std::env::remove_var(BROKER_ADDR_ENV);
        std::env::remove_var(BROKER_TOKEN_ENV);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn client_forwards_progress_and_handles_result_error_and_missing_config() {
        let _lock = ENV_LOCK.lock().await;

        async fn bind() -> TcpListener {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            std::env::set_var(BROKER_ADDR_ENV, listener.local_addr().unwrap().to_string());
            std::env::set_var(BROKER_TOKEN_ENV, "token");
            listener
        }

        let listener = bind().await;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read_half, mut write_half) = stream.into_split();
            let mut reader = BufReader::new(read_half);
            let line = super::super::read_limited_async_line(&mut reader, MAX_FRAME_BYTES)
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(
                serde_json::from_str::<BrokerRequest>(&line).unwrap(),
                BrokerRequest::Run { token, runs_root, .. }
                    if token == "token" && runs_root.ends_with("runs")
            ));
            write_frame(
                &mut write_half,
                &BrokerResponse::Progress {
                    event: ExternalCliProgressEvent {
                        event_type:
                            crate::im_gateway::external_cli::ExternalCliProgressEventType::Status,
                        content: "working".to_string(),
                        title: None,
                        raw: serde_json::Value::Null,
                    },
                },
            )
            .await
            .unwrap();
            write_frame(
                &mut write_half,
                &BrokerResponse::Result {
                    result: Box::new(result()),
                },
            )
            .await
            .unwrap();
        });
        let (progress_tx, mut progress_rx) = mpsc::channel(1);
        let value = run_via_main_broker(
            std::path::PathBuf::from("runs"),
            request(),
            Some(progress_tx),
        )
        .await
        .unwrap();
        assert_eq!(value.run_id, "run-1");
        assert_eq!(progress_rx.recv().await.unwrap().content, "working");
        server.await.unwrap();

        let listener = bind().await;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read_half, mut write_half) = stream.into_split();
            let mut reader = BufReader::new(read_half);
            let _ = super::super::read_limited_async_line(&mut reader, MAX_FRAME_BYTES)
                .await
                .unwrap();
            write_frame(
                &mut write_half,
                &BrokerResponse::Error {
                    error: "rejected".to_string(),
                },
            )
            .await
            .unwrap();
        });
        assert_eq!(
            run_via_main_broker(std::path::PathBuf::new(), request(), None)
                .await
                .unwrap_err(),
            "rejected"
        );
        server.await.unwrap();

        std::env::remove_var(BROKER_ADDR_ENV);
        assert!(
            run_via_main_broker(std::path::PathBuf::new(), request(), None)
                .await
                .unwrap_err()
                .contains(BROKER_ADDR_ENV)
        );
        std::env::remove_var(BROKER_TOKEN_ENV);
        assert!(!client_configured());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn clients_reject_unexpected_broker_response_types() {
        let _lock = ENV_LOCK.lock().await;

        async fn serve_once(response: BrokerResponse) -> tokio::task::JoinHandle<()> {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            std::env::set_var(BROKER_ADDR_ENV, listener.local_addr().unwrap().to_string());
            std::env::set_var(BROKER_TOKEN_ENV, "token");
            tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                let (read_half, mut write_half) = stream.into_split();
                let mut reader = BufReader::new(read_half);
                super::super::read_limited_async_line(&mut reader, MAX_FRAME_BYTES)
                    .await
                    .unwrap()
                    .unwrap();
                write_frame(&mut write_half, &response).await.unwrap();
            })
        }

        let server = serve_once(BrokerResponse::StopResult { stopped: false }).await;
        assert_eq!(
            run_via_main_broker(std::path::PathBuf::new(), request(), None)
                .await
                .unwrap_err(),
            "IM Agent broker returned an unexpected control response"
        );
        server.await.unwrap();

        let server = serve_once(BrokerResponse::StopResult { stopped: false }).await;
        assert_eq!(
            guide_via_main_broker("session", "guide".to_string(), "continue".to_string())
                .await
                .unwrap_err(),
            "IM Agent broker returned an unexpected guide response"
        );
        server.await.unwrap();

        let server = serve_once(BrokerResponse::StopResult { stopped: false }).await;
        assert_eq!(
            model_update_via_main_broker("session", Some("gpt-5.6".to_string()))
                .await
                .unwrap_err(),
            "IM Agent broker returned an unexpected model update response"
        );
        server.await.unwrap();

        let server = serve_once(BrokerResponse::GuideResult {
            result: ExternalCliGuideResult {
                guide_id: "guide".to_string(),
                accepted: false,
                thread_id: None,
                turn_id: None,
                reason: Some("not running".to_string()),
            },
        })
        .await;
        assert_eq!(
            stop_via_main_broker(std::path::Path::new("runs"), "session")
                .await
                .unwrap_err(),
            "IM Agent broker returned an unexpected stop response"
        );
        server.await.unwrap();

        std::env::remove_var(BROKER_ADDR_ENV);
        std::env::remove_var(BROKER_TOKEN_ENV);
    }

    #[tokio::test]
    async fn server_rejects_closed_malformed_and_bad_token_clients() {
        async fn pair() -> (TcpStream, TcpStream) {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let (client, accepted) = tokio::join!(TcpStream::connect(addr), listener.accept());
            (client.unwrap(), accepted.unwrap().0)
        }

        let (client, server) = pair().await;
        drop(client);
        assert!(serve_connection(server, "token")
            .await
            .unwrap_err()
            .contains("closed before request"));

        let (mut client, server) = pair().await;
        client.write_all(b"bad-json\n").await.unwrap();
        assert!(serve_connection(server, "token")
            .await
            .unwrap_err()
            .contains("parse IM Agent"));

        let (mut client, server) = pair().await;
        write_frame(
            &mut client,
            &BrokerRequest::Run {
                token: "wrong".to_string(),
                runs_root: String::new(),
                request: Box::new(request()),
            },
        )
        .await
        .unwrap();
        serve_connection(server, "token").await.unwrap();
        let mut reader = BufReader::new(client);
        let line = super::super::read_limited_async_line(&mut reader, MAX_FRAME_BYTES)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            serde_json::from_str::<BrokerResponse>(&line).unwrap(),
            BrokerResponse::Error { error } if error.contains("capability token")
        ));

        cancel_brokered_session(None, "codex").await;
        let oversized = BrokerResponse::Error {
            error: "x".repeat(MAX_FRAME_BYTES),
        };
        assert!(write_frame(&mut tokio::io::sink(), &oversized)
            .await
            .unwrap_err()
            .contains("exceeds"));
    }

    #[tokio::test]
    async fn server_routes_control_requests_and_returns_bounded_results() {
        async fn exchange(request: BrokerRequest) -> BrokerResponse {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let (client, accepted) = tokio::join!(TcpStream::connect(addr), listener.accept());
            let mut client = client.unwrap();
            write_frame(&mut client, &request).await.unwrap();
            let server = tokio::spawn(async move {
                serve_connection(accepted.unwrap().0, "token")
                    .await
                    .unwrap();
            });
            let mut reader = BufReader::new(client);
            let line = super::super::read_limited_async_line(&mut reader, MAX_FRAME_BYTES)
                .await
                .unwrap()
                .unwrap();
            server.await.unwrap();
            serde_json::from_str(&line).unwrap()
        }

        let guide = exchange(BrokerRequest::Guide {
            token: "token".to_string(),
            session_key: "missing-session".to_string(),
            guide_id: "guide-1".to_string(),
            message: "continue".to_string(),
        })
        .await;
        assert!(matches!(
            guide,
            BrokerResponse::GuideResult { result }
                if result.guide_id == "guide-1" && !result.accepted
        ));

        let model = exchange(BrokerRequest::ModelUpdate {
            token: "token".to_string(),
            session_key: "missing-session".to_string(),
            model: Some("gpt-5.6".to_string()),
        })
        .await;
        assert!(matches!(
            model,
            BrokerResponse::Error { error } if error.contains("no active external runner")
        ));

        let temp = tempfile::tempdir().unwrap();
        let stopped = exchange(BrokerRequest::Stop {
            token: "token".to_string(),
            runs_root: temp.path().display().to_string(),
            session_key: "missing-session".to_string(),
        })
        .await;
        assert!(matches!(
            stopped,
            BrokerResponse::StopResult { stopped: false }
        ));
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn server_cancels_runs_when_clients_disconnect_or_send_extra_frames() {
        let _lock = ENV_LOCK.lock().await;
        let temp = tempfile::tempdir().unwrap();
        let _data_dir = crate::test_env::BifrostDataDirGuard::set(temp.path());

        async fn pair() -> (TcpStream, TcpStream) {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let (client, accepted) = tokio::join!(TcpStream::connect(addr), listener.accept());
            (client.unwrap(), accepted.unwrap().0)
        }

        fn holding_request(session_key: &str) -> ExternalCliRunRequest {
            serde_json::from_value(serde_json::json!({
                "message": "hold until broker cancellation",
                "adapter": "mock",
                "sessionKey": session_key,
                "adapterConfig": {
                    "executable": "sh",
                    "args": ["-c", "cat >/dev/null; sleep 10"],
                    "timeoutSecs": 15
                }
            }))
            .unwrap()
        }

        let (mut client, server) = pair().await;
        write_frame(
            &mut client,
            &BrokerRequest::Run {
                token: "token".to_string(),
                runs_root: temp.path().join("runs-disconnect").display().to_string(),
                request: Box::new(holding_request("broker-disconnect")),
            },
        )
        .await
        .unwrap();
        drop(client);
        let error = serve_connection(server, "token").await.unwrap_err();
        assert!(error.contains("client disconnected"), "{error}");

        let (mut client, server) = pair().await;
        write_frame(
            &mut client,
            &BrokerRequest::Run {
                token: "token".to_string(),
                runs_root: temp.path().join("runs-extra").display().to_string(),
                request: Box::new(holding_request("broker-extra-frame")),
            },
        )
        .await
        .unwrap();
        client.write_all(b"{}\n").await.unwrap();
        let error = serve_connection(server, "token").await.unwrap_err();
        assert!(error.contains("unexpected extra"), "{error}");

        cancel_brokered_session(Some("missing-chatgpt-session"), "chatgpt-web").await;
        cancel_brokered_session(Some("missing-cli-session"), "codex").await;
    }
}
