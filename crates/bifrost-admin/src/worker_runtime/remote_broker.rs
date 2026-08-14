use std::sync::{Arc, OnceLock};

use base64::Engine;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Semaphore};

use crate::remote_invoke::types::{
    scope_allows_command, GrantStatus, RemoteCommand, RemoteInvokeResponse,
};
use crate::remote_invoke::{GrantInfoStore, RemoteInvokeExecutor};
use crate::state::SharedAdminState;

use super::remote_execution::RemoteExecutionEnvelope;

pub(crate) const BROKER_ADDR_ENV: &str = "BIFROST_REMOTE_EXECUTION_BROKER_ADDR";
pub(crate) const BROKER_TOKEN_ENV: &str = "BIFROST_REMOTE_EXECUTION_BROKER_TOKEN";
pub(crate) const BROKER_RELAY_ENV: &str = "BIFROST_REMOTE_EXECUTION_BROKER_RELAY";
const BROKER_MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
const BROKER_STDIN_CHUNK_BYTES: usize = 64 * 1024;
const BROKER_MAX_CONNECTIONS: usize = 32;

static ENDPOINT: OnceLock<BrokerEndpoint> = OnceLock::new();
static RUNTIME_STATE: OnceLock<parking_lot::RwLock<Option<BrokerRuntimeState>>> = OnceLock::new();

#[derive(Clone)]
struct BrokerRuntimeState {
    state: SharedAdminState,
    admin_host: String,
    admin_port: u16,
}

#[derive(Debug, Clone)]
pub(crate) struct BrokerEndpoint {
    pub addr: String,
    pub token: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BrokerRequest {
    Start {
        token: String,
        relay_url: String,
        envelope: RemoteExecutionEnvelope,
    },
    Stdin {
        data_base64: String,
    },
    StdinClose,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BrokerResponse {
    Stdout { data_base64: String },
    Result { response: RemoteInvokeResponse },
    Error { error: String },
}

pub(crate) async fn ensure_main_broker(
    state: SharedAdminState,
    admin_host: String,
    admin_port: u16,
) -> Result<BrokerEndpoint, String> {
    *runtime_state().write() = Some(BrokerRuntimeState {
        state,
        admin_host,
        admin_port,
    });
    if let Some(endpoint) = ENDPOINT.get() {
        return Ok(endpoint.clone());
    }

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|error| format!("bind Remote Execution main broker: {error}"))?;
    let addr = listener
        .local_addr()
        .map_err(|error| format!("read Remote Execution broker address: {error}"))?
        .to_string();
    let endpoint = BrokerEndpoint {
        addr,
        token: uuid::Uuid::new_v4().to_string(),
    };
    let endpoint_for_task = endpoint.clone();
    let connections = Arc::new(Semaphore::new(BROKER_MAX_CONNECTIONS));
    tokio::spawn(async move {
        loop {
            let (stream, peer) = match listener.accept().await {
                Ok(value) => value,
                Err(error) => {
                    tracing::warn!(error = %error, "Remote Execution broker accept failed");
                    break;
                }
            };
            if !peer.ip().is_loopback() {
                tracing::warn!(peer = %peer, "rejected non-loopback Remote Execution broker peer");
                continue;
            }
            let permit = match connections.clone().try_acquire_owned() {
                Ok(permit) => permit,
                Err(_) => {
                    tracing::warn!("Remote Execution broker connection limit reached");
                    continue;
                }
            };
            let token = endpoint_for_task.token.clone();
            tokio::spawn(async move {
                let _permit = permit;
                if let Err(error) = serve_connection(stream, &token).await {
                    tracing::debug!(error = %error, "Remote Execution broker connection closed");
                }
            });
        }
    });
    ENDPOINT
        .set(endpoint.clone())
        .map_err(|_| "Remote Execution broker endpoint raced during initialization".to_string())?;
    Ok(endpoint)
}

pub(crate) fn configure_worker_env(
    spec: &mut super::WorkerSpawnSpec,
    endpoint: &BrokerEndpoint,
    relay_url: &str,
) {
    spec.env
        .insert(BROKER_ADDR_ENV.to_string(), endpoint.addr.clone());
    spec.env
        .insert(BROKER_TOKEN_ENV.to_string(), endpoint.token.clone());
    spec.env
        .insert(BROKER_RELAY_ENV.to_string(), relay_url.to_string());
}

pub(crate) fn broker_client_configured() -> bool {
    std::env::var(BROKER_ADDR_ENV).is_ok()
        && std::env::var(BROKER_TOKEN_ENV).is_ok()
        && std::env::var(BROKER_RELAY_ENV).is_ok()
}

pub(crate) async fn execute_via_main_broker<F, Fut>(
    command: &RemoteCommand,
    mut stdin_rx: Option<mpsc::Receiver<Vec<u8>>>,
    on_stdout: &mut F,
) -> Result<RemoteInvokeResponse, String>
where
    F: FnMut(Vec<u8>) -> Fut,
    Fut: std::future::Future<Output = bifrost_core::Result<()>>,
{
    let addr = std::env::var(BROKER_ADDR_ENV)
        .map_err(|_| format!("{BROKER_ADDR_ENV} is required in Remote Invoke worker"))?;
    let token = std::env::var(BROKER_TOKEN_ENV)
        .map_err(|_| format!("{BROKER_TOKEN_ENV} is required in Remote Invoke worker"))?;
    let relay_url = std::env::var(BROKER_RELAY_ENV)
        .map_err(|_| format!("{BROKER_RELAY_ENV} is required in Remote Invoke worker"))?;
    let mut stream = TcpStream::connect(&addr)
        .await
        .map_err(|error| format!("connect Remote Execution main broker {addr}: {error}"))?;
    write_request(
        &mut stream,
        &BrokerRequest::Start {
            token,
            relay_url,
            envelope: RemoteExecutionEnvelope::from_command(command),
        },
    )
    .await?;
    let (read_half, mut write_half) = stream.into_split();
    let stdin_task = tokio::spawn(async move {
        if let Some(receiver) = stdin_rx.as_mut() {
            while let Some(chunk) = receiver.recv().await {
                for part in chunk.chunks(BROKER_STDIN_CHUNK_BYTES) {
                    write_request(
                        &mut write_half,
                        &BrokerRequest::Stdin {
                            data_base64: base64::engine::general_purpose::STANDARD.encode(part),
                        },
                    )
                    .await?;
                }
            }
        }
        write_request(&mut write_half, &BrokerRequest::StdinClose).await
    });

    let mut reader = BufReader::new(read_half);
    loop {
        let line = super::read_limited_async_line(&mut reader, BROKER_MAX_FRAME_BYTES)
            .await
            .map_err(|error| format!("read Remote Execution broker response: {error}"))?
            .ok_or_else(|| "Remote Execution broker closed before result".to_string())?;
        let frame: BrokerResponse = serde_json::from_str(&line)
            .map_err(|error| format!("parse Remote Execution broker response: {error}"))?;
        match frame {
            BrokerResponse::Stdout { data_base64 } => {
                let chunk = base64::engine::general_purpose::STANDARD
                    .decode(data_base64)
                    .map_err(|error| format!("decode Remote Execution broker stdout: {error}"))?;
                on_stdout(chunk)
                    .await
                    .map_err(|error| format!("forward Remote Execution broker stdout: {error}"))?;
            }
            BrokerResponse::Result { response } => {
                stdin_task.abort();
                return Ok(response);
            }
            BrokerResponse::Error { error } => {
                stdin_task.abort();
                return Err(error);
            }
        }
    }
}

async fn serve_connection(stream: TcpStream, expected_token: &str) -> Result<(), String> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let start_line = super::read_limited_async_line(&mut reader, BROKER_MAX_FRAME_BYTES)
        .await
        .map_err(|error| format!("read Remote Execution broker start: {error}"))?
        .ok_or_else(|| "Remote Execution broker peer closed before start".to_string())?;
    let start: BrokerRequest = serde_json::from_str(&start_line)
        .map_err(|error| format!("parse Remote Execution broker start: {error}"))?;
    let BrokerRequest::Start {
        token,
        relay_url,
        envelope,
    } = start
    else {
        return Err("Remote Execution broker requires start as first frame".to_string());
    };
    if token != expected_token {
        write_response(
            &mut write_half,
            &BrokerResponse::Error {
                error: "invalid Remote Execution broker capability token".to_string(),
            },
        )
        .await?;
        return Ok(());
    }

    let runtime = runtime_state()
        .read()
        .clone()
        .ok_or_else(|| "Remote Execution broker runtime is not configured".to_string())?;
    let command = match reauthorize_intent(&relay_url, envelope) {
        Ok(command) => command,
        Err(error) => {
            write_response(&mut write_half, &BrokerResponse::Error { error }).await?;
            return Ok(());
        }
    };

    let (stdin_tx, stdin_rx) = mpsc::channel::<Vec<u8>>(32);
    let mut stdin_tx = Some(stdin_tx);
    let (stdout_tx, mut stdout_rx) = mpsc::channel::<Vec<u8>>(64);
    let runtime_for_exec = runtime.clone();
    let mut execution = Box::pin(tokio::spawn(async move {
        if command.kind == crate::remote_invoke::types::CommandKind::ShellExec {
            let mut sink = move |chunk: Vec<u8>| {
                let stdout_tx = stdout_tx.clone();
                async move {
                    stdout_tx.send(chunk).await.map_err(|_| {
                        bifrost_core::BifrostError::Config(
                            "Remote Execution broker stdout consumer closed".to_string(),
                        )
                    })
                }
            };
            super::remote_execution::execute_remote_command(
                &command,
                &runtime_for_exec.admin_host,
                runtime_for_exec.admin_port,
                Some(stdin_rx),
                &mut sink,
            )
            .await
        } else {
            let executor = RemoteInvokeExecutor::new_with_state(
                &runtime_for_exec.admin_host,
                runtime_for_exec.admin_port,
                runtime_for_exec.state,
            );
            executor
                .execute_with_stdout_sink(&command, Some(stdin_rx), |_| async { Ok(()) })
                .await
                .map_err(|error| error.to_string())
        }
    }));

    let mut stdin_open = true;
    loop {
        tokio::select! {
            maybe_chunk = stdout_rx.recv() => {
                if let Some(chunk) = maybe_chunk {
                    write_response(
                        &mut write_half,
                        &BrokerResponse::Stdout {
                            data_base64: base64::engine::general_purpose::STANDARD.encode(chunk),
                        },
                    ).await?;
                }
            }
            line = super::read_limited_async_line(&mut reader, BROKER_MAX_FRAME_BYTES), if stdin_open => {
                let line = line.map_err(|error| format!("read Remote Execution broker stdin: {error}"))?;
                let Some(line) = line else {
                    execution.abort();
                    return Err("Remote Execution broker client disconnected".to_string());
                };
                match serde_json::from_str::<BrokerRequest>(&line)
                    .map_err(|error| format!("parse Remote Execution broker stdin: {error}"))? {
                    BrokerRequest::Stdin { data_base64 } => {
                        let bytes = base64::engine::general_purpose::STANDARD
                            .decode(data_base64)
                            .map_err(|error| format!("decode Remote Execution broker stdin: {error}"))?;
                        if bytes.len() > BROKER_STDIN_CHUNK_BYTES {
                            execution.abort();
                            return Err("Remote Execution broker stdin chunk exceeds hard limit".to_string());
                        }
                        let Some(sender) = stdin_tx.as_ref() else {
                            execution.abort();
                            return Err("Remote Execution broker stdin is already closed".to_string());
                        };
                        sender.send(bytes).await.map_err(|_| "Remote Execution broker stdin is closed".to_string())?;
                    }
                    BrokerRequest::StdinClose => {
                        stdin_open = false;
                        stdin_tx.take();
                    }
                    BrokerRequest::Start { .. } => {
                        execution.abort();
                        return Err("duplicate Remote Execution broker start frame".to_string());
                    }
                }
            }
            result = &mut execution => {
                let result = result
                    .map_err(|error| format!("Remote Execution broker task join failed: {error}"))?;
                match result {
                    Ok(response) => write_response(&mut write_half, &BrokerResponse::Result { response }).await?,
                    Err(error) => write_response(&mut write_half, &BrokerResponse::Error { error }).await?,
                }
                return Ok(());
            }
        }
    }
}

fn reauthorize_intent(
    relay_url: &str,
    envelope: RemoteExecutionEnvelope,
) -> Result<RemoteCommand, String> {
    let grant_id = envelope
        .grant_id()
        .ok_or_else(|| "Remote Execution intent is missing grant_id".to_string())?;
    let store = GrantInfoStore::new(&bifrost_storage::data_dir());
    let grants = store
        .load_for_relay(relay_url)
        .map_err(|error| format!("reload Remote Invoke grant for broker authorization: {error}"))?;
    let grant = grants.get(grant_id).ok_or_else(|| {
        format!("Remote Invoke grant '{grant_id}' is not authorized in main process")
    })?;
    if grant.status != GrantStatus::Active {
        return Err(format!("Remote Invoke grant '{grant_id}' is not active"));
    }
    let now = chrono::Utc::now().timestamp_millis().max(0) as u64;
    if grant.expires_at.is_some_and(|expires_at| expires_at <= now) {
        return Err(format!("Remote Invoke grant '{grant_id}' is expired"));
    }
    if grant.remaining_calls == Some(0) {
        return Err(format!(
            "Remote Invoke grant '{grant_id}' has no remaining calls"
        ));
    }
    if envelope.caller_fingerprint() != Some(grant.caller_fingerprint.as_str()) {
        return Err(
            "Remote Execution caller fingerprint does not match persisted grant".to_string(),
        );
    }
    if envelope.ssh_fingerprint() != grant.ssh_key_fingerprint.as_deref() {
        return Err("Remote Execution SSH fingerprint does not match persisted grant".to_string());
    }
    if envelope.file_access() != grant.file_access {
        return Err(
            "Remote Execution file access scope does not match persisted grant".to_string(),
        );
    }
    if !scope_allows_command(
        grant.grant_scope,
        grant.file_access,
        envelope.command_kind(),
    ) {
        return Err(format!(
            "Remote Invoke grant '{grant_id}' does not allow command kind {}",
            envelope.command_kind().as_str()
        ));
    }
    Ok(envelope.into_command())
}

async fn write_request<W>(writer: &mut W, frame: &BrokerRequest) -> Result<(), String>
where
    W: AsyncWrite + Unpin,
{
    write_json_line(writer, frame).await
}

async fn write_response<W>(writer: &mut W, frame: &BrokerResponse) -> Result<(), String>
where
    W: AsyncWrite + Unpin,
{
    write_json_line(writer, frame).await
}

async fn write_json_line<W, T>(writer: &mut W, value: &T) -> Result<(), String>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let mut bytes = serde_json::to_vec(value)
        .map_err(|error| format!("serialize Remote Execution broker frame: {error}"))?;
    if bytes.len() > BROKER_MAX_FRAME_BYTES {
        return Err(format!(
            "Remote Execution broker frame exceeds {BROKER_MAX_FRAME_BYTES} bytes"
        ));
    }
    bytes.push(b'\n');
    writer
        .write_all(&bytes)
        .await
        .map_err(|error| format!("write Remote Execution broker frame: {error}"))?;
    writer
        .flush()
        .await
        .map_err(|error| format!("flush Remote Execution broker frame: {error}"))
}

fn runtime_state() -> &'static parking_lot::RwLock<Option<BrokerRuntimeState>> {
    RUNTIME_STATE.get_or_init(|| parking_lot::RwLock::new(None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broker_client_requires_all_capability_environment() {
        std::env::remove_var(BROKER_ADDR_ENV);
        std::env::remove_var(BROKER_TOKEN_ENV);
        std::env::remove_var(BROKER_RELAY_ENV);
        assert!(!broker_client_configured());
    }
}
