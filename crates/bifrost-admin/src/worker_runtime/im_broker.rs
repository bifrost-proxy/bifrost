use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Semaphore};

use crate::im_gateway::external_cli::{
    ExternalCliProgressEvent, ExternalCliRunRequest, ExternalCliRunResult, ExternalCliRuntime,
};

pub(crate) const BROKER_ADDR_ENV: &str = "BIFROST_IM_AGENT_BROKER_ADDR";
pub(crate) const BROKER_TOKEN_ENV: &str = "BIFROST_IM_AGENT_BROKER_TOKEN";
const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
const MAX_CONNECTIONS: usize = 16;

static ENDPOINT: OnceLock<BrokerEndpoint> = OnceLock::new();

#[derive(Debug, Clone)]
pub(crate) struct BrokerEndpoint {
    pub addr: String,
    pub token: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BrokerRequest {
    Run {
        token: String,
        runs_root: String,
        request: Box<ExternalCliRunRequest>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BrokerResponse {
    Progress { event: ExternalCliProgressEvent },
    Result { result: ExternalCliRunResult },
    Error { error: String },
}

pub(crate) async fn ensure_main_broker() -> Result<BrokerEndpoint, String> {
    if let Some(endpoint) = ENDPOINT.get() {
        return Ok(endpoint.clone());
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
    tokio::spawn(async move {
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
    ENDPOINT
        .set(endpoint.clone())
        .map_err(|_| "IM Agent broker endpoint raced during initialization".to_string())?;
    Ok(endpoint)
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
            BrokerResponse::Result { result } => return Ok(result),
            BrokerResponse::Error { error } => return Err(error),
        }
    }
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
    let BrokerRequest::Run {
        token,
        runs_root,
        request,
    } = request;
    if token != expected_token {
        write_frame(
            &mut write_half,
            &BrokerResponse::Error {
                error: "invalid IM Agent broker capability token".to_string(),
            },
        )
        .await?;
        return Ok(());
    }
    let runs_root = std::path::PathBuf::from(runs_root);
    let runtime = ExternalCliRuntime::new(runs_root);
    let session_key = request.session_key.clone();
    let adapter = request.adapter.clone();
    let (progress_tx, mut progress_rx) = mpsc::channel(256);
    let mut run = Box::pin(tokio::spawn(async move {
        runtime.run_with_progress(*request, Some(progress_tx)).await
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
                        return Err("unexpected extra IM Agent broker request frame".to_string());
                    }
                    Err(error) => {
                        run.abort();
                        return Err(format!("read IM Agent broker client state: {error}"));
                    }
                }
            }
            result = &mut run => {
                let result = result.map_err(|error| format!("IM Agent broker task join failed: {error}"))?;
                match result {
                    Ok(result) => write_frame(&mut write_half, &BrokerResponse::Result { result }).await?,
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
