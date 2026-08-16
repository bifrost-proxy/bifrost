use std::sync::{Arc, OnceLock};

use base64::Engine;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Semaphore};

use crate::remote_invoke::types::{
    scope_allows_command, GrantInfo, GrantStatus, RemoteCommand, RemoteInvokeResponse,
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

static SERVER: OnceLock<tokio::sync::Mutex<Option<BrokerServer>>> = OnceLock::new();
static RUNTIME_STATE: OnceLock<parking_lot::RwLock<Option<BrokerRuntimeState>>> = OnceLock::new();
static GRANT_AUTH_LOCK: OnceLock<parking_lot::Mutex<()>> = OnceLock::new();

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

struct BrokerServer {
    endpoint: BrokerEndpoint,
    task: tokio::task::JoinHandle<()>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BrokerRequest {
    Start {
        token: String,
        relay_url: String,
        envelope: Box<RemoteExecutionEnvelope>,
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

async fn forward_stdout_chunk(
    sender: &mpsc::Sender<Vec<u8>>,
    chunk: Vec<u8>,
) -> bifrost_core::Result<()> {
    sender.send(chunk).await.map_err(|_| {
        bifrost_core::BifrostError::Config(
            "Remote Execution broker stdout consumer closed".to_string(),
        )
    })
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
    let task = tokio::spawn(async move {
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
    *server = Some(BrokerServer {
        endpoint: endpoint.clone(),
        task,
    });
    Ok(endpoint)
}

fn broker_server() -> &'static tokio::sync::Mutex<Option<BrokerServer>> {
    SERVER.get_or_init(|| tokio::sync::Mutex::new(None))
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
            envelope: Box::new(RemoteExecutionEnvelope::from_command(command)),
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
    let command = match reauthorize_intent(&relay_url, *envelope) {
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
                async move { forward_stdout_chunk(&stdout_tx, chunk).await }
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
            let sink = move |chunk: Vec<u8>| {
                let stdout_tx = stdout_tx.clone();
                async move { forward_stdout_chunk(&stdout_tx, chunk).await }
            };
            executor
                .execute_with_stdout_sink(&command, Some(stdin_rx), sink)
                .await
                .map_err(|error| error.to_string())
        }
    }));

    let mut stdin_open = true;
    loop {
        tokio::select! {
            maybe_chunk = stdout_rx.recv() => {
                if let Some(chunk) = maybe_chunk {
                    if let Err(error) = write_response(
                        &mut write_half,
                        &BrokerResponse::Stdout {
                            data_base64: base64::engine::general_purpose::STANDARD.encode(chunk),
                        },
                    ).await {
                        execution.abort();
                        return Err(error);
                    }
                }
            }
            line = super::read_limited_async_line(&mut reader, BROKER_MAX_FRAME_BYTES), if stdin_open => {
                let line = match line {
                    Ok(line) => line,
                    Err(error) => {
                        execution.abort();
                        return Err(format!("read Remote Execution broker stdin: {error}"));
                    }
                };
                let Some(line) = line else {
                    execution.abort();
                    return Err("Remote Execution broker client disconnected".to_string());
                };
                let request = match serde_json::from_str::<BrokerRequest>(&line) {
                    Ok(request) => request,
                    Err(error) => {
                        execution.abort();
                        return Err(format!("parse Remote Execution broker stdin: {error}"));
                    }
                };
                match request {
                    BrokerRequest::Stdin { data_base64 } => {
                        let bytes = match base64::engine::general_purpose::STANDARD.decode(data_base64) {
                            Ok(bytes) => bytes,
                            Err(error) => {
                                execution.abort();
                                return Err(format!("decode Remote Execution broker stdin: {error}"));
                            }
                        };
                        if bytes.len() > BROKER_STDIN_CHUNK_BYTES {
                            execution.abort();
                            return Err("Remote Execution broker stdin chunk exceeds hard limit".to_string());
                        }
                        let Some(sender) = stdin_tx.as_ref() else {
                            execution.abort();
                            return Err("Remote Execution broker stdin is already closed".to_string());
                        };
                        if sender.send(bytes).await.is_err() {
                            execution.abort();
                            return Err("Remote Execution broker stdin is closed".to_string());
                        }
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
                // The command awaits every stdout send before it completes, but the
                // final send and the JoinHandle can become ready in the same select
                // turn. Drain those already-buffered chunks before publishing the
                // terminal result, otherwise the worker observes a successful call
                // with an empty final output.
                write_buffered_stdout(&mut write_half, &mut stdout_rx).await?;
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
    let _authorization = grant_auth_lock().lock();
    let grant_id = envelope
        .grant_id()
        .ok_or_else(|| "Remote Execution intent is missing grant_id".to_string())?;
    let store = GrantInfoStore::new(&bifrost_storage::data_dir());
    let mut grants = store
        .load_for_relay(relay_url)
        .map_err(|error| format!("reload Remote Invoke grant for broker authorization: {error}"))?;
    let grant = grants.get_mut(grant_id).ok_or_else(|| {
        format!("Remote Invoke grant '{grant_id}' is not authorized in main process")
    })?;
    let now = chrono::Utc::now().timestamp_millis().max(0) as u64;
    validate_and_consume_grant(grant_id, grant, &envelope, now)?;
    let updated = grant.clone();
    store
        .upsert(relay_url, grant_id, &updated)
        .map_err(|error| format!("persist Remote Invoke broker authorization: {error}"))?;
    Ok(envelope.into_command())
}

fn validate_and_consume_grant(
    grant_id: &str,
    grant: &mut GrantInfo,
    envelope: &RemoteExecutionEnvelope,
    now: u64,
) -> Result<(), String> {
    if grant.status != GrantStatus::Active {
        return Err(format!("Remote Invoke grant '{grant_id}' is not active"));
    }
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
    if let Some(remaining) = grant.remaining_calls {
        grant.remaining_calls = Some(remaining - 1);
        if remaining == 1 {
            grant.status = GrantStatus::Consumed;
        }
    }
    grant.use_count = grant.use_count.saturating_add(1);
    grant.last_command_at = Some(now);
    grant.last_used_at = Some(now);
    Ok(())
}

fn grant_auth_lock() -> &'static parking_lot::Mutex<()> {
    GRANT_AUTH_LOCK.get_or_init(|| parking_lot::Mutex::new(()))
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

async fn write_buffered_stdout<W>(
    writer: &mut W,
    receiver: &mut mpsc::Receiver<Vec<u8>>,
) -> Result<(), String>
where
    W: AsyncWrite + Unpin,
{
    while let Ok(chunk) = receiver.try_recv() {
        write_response(
            writer,
            &BrokerResponse::Stdout {
                data_base64: base64::engine::general_purpose::STANDARD.encode(chunk),
            },
        )
        .await?;
    }
    Ok(())
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

    static BROKER_TEST_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn active_grant() -> GrantInfo {
        use crate::remote_invoke::types::{AuthMethod, FileAccessScope, GrantMode, GrantScope};

        GrantInfo {
            grant_id: "grant-test".to_string(),
            client_instance_id: "client".to_string(),
            caller_fingerprint: "caller".to_string(),
            caller_display_name: None,
            label: None,
            grant_mode: GrantMode::Permanent,
            grant_scope: GrantScope::RemoteQuery,
            file_access: FileAccessScope::None,
            auth_method: AuthMethod::PairCode,
            status: GrantStatus::Active,
            first_authorized_at: 1,
            last_command_at: None,
            expires_at: None,
            last_used_at: None,
            max_calls: None,
            remaining_calls: None,
            use_count: 0,
            ssh_key_id: None,
            ssh_key_fingerprint: None,
            caller_ephemeral_pub: None,
            client_ephemeral_pub: None,
            policy_binding: None,
            shell_policy_set_version_snapshot: None,
            interactive_allowed: None,
            stdin_allowed: None,
            os_version: None,
            arch: None,
        }
    }

    fn envelope(command: RemoteCommand) -> RemoteExecutionEnvelope {
        RemoteExecutionEnvelope::from_command(&command)
    }

    #[test]
    fn broker_client_requires_all_capability_environment() {
        let _lock = BROKER_TEST_ENV_LOCK.blocking_lock();
        std::env::remove_var(BROKER_ADDR_ENV);
        std::env::remove_var(BROKER_TOKEN_ENV);
        std::env::remove_var(BROKER_RELAY_ENV);
        assert!(!broker_client_configured());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn broker_client_streams_chunked_stdin_stdout_and_terminal_frames() {
        let _lock = BROKER_TEST_ENV_LOCK.lock().await;

        async fn bind_listener() -> (TcpListener, String) {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap().to_string();
            std::env::set_var(BROKER_ADDR_ENV, &addr);
            std::env::set_var(BROKER_TOKEN_ENV, "token");
            std::env::set_var(BROKER_RELAY_ENV, "https://relay.example");
            (listener, addr)
        }

        let (listener, _) = bind_listener().await;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read_half, mut write_half) = stream.into_split();
            let mut reader = BufReader::new(read_half);
            let start = super::super::read_limited_async_line(&mut reader, BROKER_MAX_FRAME_BYTES)
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(
                serde_json::from_str::<BrokerRequest>(&start).unwrap(),
                BrokerRequest::Start { token, relay_url, .. }
                    if token == "token" && relay_url == "https://relay.example"
            ));
            let mut stdin = Vec::new();
            loop {
                let line =
                    super::super::read_limited_async_line(&mut reader, BROKER_MAX_FRAME_BYTES)
                        .await
                        .unwrap()
                        .unwrap();
                match serde_json::from_str::<BrokerRequest>(&line).unwrap() {
                    BrokerRequest::Stdin { data_base64 } => stdin.extend(
                        base64::engine::general_purpose::STANDARD
                            .decode(data_base64)
                            .unwrap(),
                    ),
                    BrokerRequest::StdinClose => break,
                    BrokerRequest::Start { .. } => panic!("unexpected duplicate start"),
                }
            }
            assert_eq!(stdin.len(), BROKER_STDIN_CHUNK_BYTES + 7);
            write_response(
                &mut write_half,
                &BrokerResponse::Stdout {
                    data_base64: base64::engine::general_purpose::STANDARD.encode(b"streamed"),
                },
            )
            .await
            .unwrap();
            write_response(
                &mut write_half,
                &BrokerResponse::Result {
                    response: RemoteInvokeResponse {
                        exit_code: 7,
                        ..Default::default()
                    },
                },
            )
            .await
            .unwrap();
        });
        let (stdin_tx, stdin_rx) = mpsc::channel(1);
        stdin_tx
            .send(vec![b'i'; BROKER_STDIN_CHUNK_BYTES + 7])
            .await
            .unwrap();
        drop(stdin_tx);
        let mut stdout = Vec::new();
        let response =
            execute_via_main_broker(&RemoteCommand::default(), Some(stdin_rx), &mut |chunk| {
                stdout.extend(chunk);
                std::future::ready(Ok(()))
            })
            .await
            .unwrap();
        assert_eq!(response.exit_code, 7);
        assert_eq!(stdout, b"streamed");
        server.await.unwrap();

        let (listener, _) = bind_listener().await;
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(&mut stream);
            let _ = super::super::read_limited_async_line(&mut reader, BROKER_MAX_FRAME_BYTES)
                .await
                .unwrap();
            drop(reader);
            write_response(
                &mut stream,
                &BrokerResponse::Error {
                    error: "rejected".to_string(),
                },
            )
            .await
            .unwrap();
        });
        let error = execute_via_main_broker(&RemoteCommand::default(), None, &mut |_| {
            std::future::ready(Ok(()))
        })
        .await
        .unwrap_err();
        assert_eq!(error, "rejected");
        server.await.unwrap();

        std::env::remove_var(BROKER_ADDR_ENV);
        assert!(
            execute_via_main_broker(
                &RemoteCommand::default(),
                None,
                &mut |_| std::future::ready(Ok(())),
            )
            .await
            .unwrap_err()
            .contains(BROKER_ADDR_ENV)
        );
        std::env::remove_var(BROKER_TOKEN_ENV);
        std::env::remove_var(BROKER_RELAY_ENV);
    }

    #[test]
    fn worker_environment_contains_only_the_broker_capabilities() {
        let endpoint = BrokerEndpoint {
            addr: "127.0.0.1:12345".to_string(),
            token: "capability".to_string(),
        };
        let mut spec = super::super::WorkerSpawnSpec::new(
            "remote",
            super::super::WorkerKind::RemoteInvoke,
            "bifrost",
            Vec::new(),
        );
        configure_worker_env(&mut spec, &endpoint, "https://relay.example");
        assert_eq!(spec.env.get(BROKER_ADDR_ENV), Some(&endpoint.addr));
        assert_eq!(spec.env.get(BROKER_TOKEN_ENV), Some(&endpoint.token));
        assert_eq!(
            spec.env.get(BROKER_RELAY_ENV).map(String::as_str),
            Some("https://relay.example")
        );
    }

    #[tokio::test]
    async fn main_broker_endpoint_is_loopback_stable_and_refreshes_runtime_state() {
        let _lock = BROKER_TEST_ENV_LOCK.lock().await;
        let state = Arc::new(crate::state::AdminState::new(0));
        let first = ensure_main_broker(state.clone(), "127.0.0.1".to_string(), 0)
            .await
            .unwrap();
        let second = ensure_main_broker(state, "localhost".to_string(), 1)
            .await
            .unwrap();
        assert_eq!(first.addr, second.addr);
        assert_eq!(first.token, second.token);
        assert!(first.addr.starts_with("127.0.0.1:"));
        let runtime = runtime_state().read().clone().unwrap();
        assert_eq!(runtime.admin_host, "localhost");
        assert_eq!(runtime.admin_port, 1);

        let stale = broker_server().lock().await.take().unwrap();
        stale.task.abort();
        let _ = stale.task.await;
        let recovered = ensure_main_broker(
            Arc::new(crate::state::AdminState::new(2)),
            "127.0.0.1".to_string(),
            2,
        )
        .await
        .unwrap();
        assert_ne!(recovered.token, first.token);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn main_broker_reauthorizes_and_executes_a_persisted_readonly_grant() {
        let _lock = BROKER_TEST_ENV_LOCK.lock().await;
        let temp = tempfile::tempdir().unwrap();
        let _data_dir = crate::test_env::BifrostDataDirGuard::set(temp.path());
        let relay_url = "https://relay.valid-broker.test";
        let grant = active_grant();
        GrantInfoStore::new(temp.path())
            .upsert(relay_url, &grant.grant_id, &grant)
            .unwrap();

        let admin_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let admin_port = admin_listener.local_addr().unwrap().port();
        let admin = tokio::spawn(async move {
            let paths = Arc::new(std::sync::Mutex::new(Vec::new()));
            for _ in 0..2 {
                let (stream, _) = admin_listener.accept().await.unwrap();
                let paths = paths.clone();
                let service = hyper::service::service_fn(
                    move |request: hyper::Request<hyper::body::Incoming>| {
                        let paths = paths.clone();
                        async move {
                            paths.lock().unwrap().push(request.uri().path().to_string());
                            Ok::<_, std::convert::Infallible>(
                                hyper::Response::builder()
                                    .header(hyper::header::CONNECTION, "close")
                                    .body(http_body_util::Full::new(bytes::Bytes::from_static(
                                        b"{}",
                                    )))
                                    .unwrap(),
                            )
                        }
                    },
                );
                hyper::server::conn::http1::Builder::new()
                    .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                    .await
                    .unwrap();
            }
            Arc::try_unwrap(paths).unwrap().into_inner().unwrap()
        });

        let endpoint = ensure_main_broker(
            Arc::new(crate::state::AdminState::new(admin_port)),
            "127.0.0.1".to_string(),
            admin_port,
        )
        .await
        .unwrap();
        let command = RemoteCommand {
            command: "status".to_string(),
            grant_id: Some(grant.grant_id.clone()),
            caller_fingerprint: Some(grant.caller_fingerprint.clone()),
            file_access: grant.file_access,
            ..Default::default()
        };
        let mut stream = TcpStream::connect(&endpoint.addr).await.unwrap();
        write_request(
            &mut stream,
            &BrokerRequest::Start {
                token: endpoint.token,
                relay_url: relay_url.to_string(),
                envelope: Box::new(envelope(command)),
            },
        )
        .await
        .unwrap();
        write_request(&mut stream, &BrokerRequest::StdinClose)
            .await
            .unwrap();

        let mut reader = BufReader::new(stream);
        let mut stdout = Vec::new();
        let response = loop {
            let line = super::super::read_limited_async_line(&mut reader, BROKER_MAX_FRAME_BYTES)
                .await
                .unwrap()
                .expect("broker terminal response");
            match serde_json::from_str::<BrokerResponse>(&line).unwrap() {
                BrokerResponse::Stdout { data_base64 } => stdout.extend(
                    base64::engine::general_purpose::STANDARD
                        .decode(data_base64)
                        .unwrap(),
                ),
                BrokerResponse::Result { response } => break response,
                BrokerResponse::Error { error } => panic!("valid broker call failed: {error}"),
            }
        };

        assert_eq!(
            response.exit_code, 0,
            "broker response stderr={:?} stdout={:?}",
            response.stderr, response.stdout
        );
        assert!(!stdout.is_empty());
        assert!(serde_json::from_slice::<serde_json::Value>(&stdout).is_ok());
        let persisted = GrantInfoStore::new(temp.path())
            .load_for_relay(relay_url)
            .unwrap();
        assert_eq!(persisted[&grant.grant_id].use_count, 1);
        assert!(persisted[&grant.grant_id].last_used_at.is_some());
        let paths = admin.await.unwrap();
        assert_eq!(
            paths,
            [
                "/_bifrost/api/power/remote-call".to_string(),
                "/_bifrost/api/system".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn stdout_forwarder_preserves_non_shell_command_output() {
        let (sender, mut receiver) = mpsc::channel(1);
        let expected = br#"{"content_b64":"cmVtb3RlLWZpbGU="}"#.to_vec();

        forward_stdout_chunk(&sender, expected.clone())
            .await
            .unwrap();

        assert_eq!(receiver.recv().await, Some(expected));
    }

    #[tokio::test]
    async fn buffered_stdout_is_written_before_terminal_result() {
        use tokio::io::AsyncReadExt;

        let (sender, mut receiver) = mpsc::channel(1);
        sender.send(b"final-frame".to_vec()).await.unwrap();
        drop(sender);
        let (mut client, mut server) = tokio::io::duplex(1024);

        write_buffered_stdout(&mut server, &mut receiver)
            .await
            .unwrap();
        drop(server);

        let mut encoded = String::new();
        client.read_to_string(&mut encoded).await.unwrap();
        let frame: BrokerResponse = serde_json::from_str(encoded.trim()).unwrap();
        match frame {
            BrokerResponse::Stdout { data_base64 } => assert_eq!(
                base64::engine::general_purpose::STANDARD
                    .decode(data_base64)
                    .unwrap(),
                b"final-frame"
            ),
            other => panic!("expected stdout frame, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn broker_frame_writers_round_trip_and_bound_oversized_frames() {
        use tokio::io::AsyncReadExt;

        let (mut client, mut server) = tokio::io::duplex(1024);
        write_request(&mut server, &BrokerRequest::StdinClose)
            .await
            .unwrap();
        write_response(
            &mut server,
            &BrokerResponse::Error {
                error: "bounded".to_string(),
            },
        )
        .await
        .unwrap();
        drop(server);
        let mut text = String::new();
        client.read_to_string(&mut text).await.unwrap();
        let mut lines = text.lines();
        assert!(matches!(
            serde_json::from_str::<BrokerRequest>(lines.next().unwrap()).unwrap(),
            BrokerRequest::StdinClose
        ));
        assert!(matches!(
            serde_json::from_str::<BrokerResponse>(lines.next().unwrap()).unwrap(),
            BrokerResponse::Error { error } if error == "bounded"
        ));

        let oversized = BrokerRequest::Stdin {
            data_base64: "x".repeat(BROKER_MAX_FRAME_BYTES),
        };
        assert!(write_request(&mut tokio::io::sink(), &oversized)
            .await
            .unwrap_err()
            .contains("exceeds"));
    }

    #[tokio::test]
    async fn broker_connection_rejects_missing_malformed_and_unauthorized_start() {
        let _lock = BROKER_TEST_ENV_LOCK.lock().await;
        let temp = tempfile::tempdir().unwrap();
        let _data_dir = crate::test_env::BifrostDataDirGuard::set(temp.path());

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
            .contains("closed before start"));

        let (mut client, server) = pair().await;
        client.write_all(b"not-json\n").await.unwrap();
        assert!(serve_connection(server, "token")
            .await
            .unwrap_err()
            .contains("parse Remote Execution broker start"));

        let (mut client, server) = pair().await;
        write_request(&mut client, &BrokerRequest::StdinClose)
            .await
            .unwrap();
        assert!(serve_connection(server, "token")
            .await
            .unwrap_err()
            .contains("requires start"));

        let (mut client, server) = pair().await;
        write_request(
            &mut client,
            &BrokerRequest::Start {
                token: "wrong".to_string(),
                relay_url: "https://relay.example".to_string(),
                envelope: Box::new(envelope(RemoteCommand::default())),
            },
        )
        .await
        .unwrap();
        serve_connection(server, "token").await.unwrap();
        let mut reader = BufReader::new(client);
        let line = super::super::read_limited_async_line(&mut reader, BROKER_MAX_FRAME_BYTES)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            serde_json::from_str::<BrokerResponse>(&line).unwrap(),
            BrokerResponse::Error { error } if error.contains("capability token")
        ));

        *runtime_state().write() = Some(BrokerRuntimeState {
            state: Arc::new(crate::state::AdminState::new(9)),
            admin_host: "127.0.0.1".to_string(),
            admin_port: 9,
        });
        let (mut client, server) = pair().await;
        write_request(
            &mut client,
            &BrokerRequest::Start {
                token: "token".to_string(),
                relay_url: "https://missing-grant.example.test".to_string(),
                envelope: Box::new(envelope(RemoteCommand {
                    grant_id: Some("missing-grant".to_string()),
                    ..Default::default()
                })),
            },
        )
        .await
        .unwrap();
        serve_connection(server, "token").await.unwrap();
        let mut reader = BufReader::new(client);
        let line = super::super::read_limited_async_line(&mut reader, BROKER_MAX_FRAME_BYTES)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            serde_json::from_str::<BrokerResponse>(&line).unwrap(),
            BrokerResponse::Error { error } if error.contains("not authorized")
        ));
        *runtime_state().write() = None;

        let (stdout_tx, stdout_rx) = mpsc::channel(1);
        drop(stdout_rx);
        assert!(forward_stdout_chunk(&stdout_tx, b"orphaned".to_vec())
            .await
            .unwrap_err()
            .to_string()
            .contains("consumer closed"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn broker_rejects_disconnected_invalid_oversized_and_duplicate_stdin_frames() {
        let _lock = BROKER_TEST_ENV_LOCK.lock().await;
        let temp = tempfile::tempdir().unwrap();
        let _data_dir = crate::test_env::BifrostDataDirGuard::set(temp.path());
        let relay_url = "https://relay.stdin-guards.test";
        let grant = active_grant();
        GrantInfoStore::new(temp.path())
            .upsert(relay_url, &grant.grant_id, &grant)
            .unwrap();
        *runtime_state().write() = Some(BrokerRuntimeState {
            state: Arc::new(crate::state::AdminState::new(9)),
            admin_host: "127.0.0.1".to_string(),
            admin_port: 9,
        });

        async fn pair() -> (TcpStream, TcpStream) {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let (client, accepted) = tokio::join!(TcpStream::connect(addr), listener.accept());
            (client.unwrap(), accepted.unwrap().0)
        }

        let start = || BrokerRequest::Start {
            token: "token".to_string(),
            relay_url: relay_url.to_string(),
            envelope: Box::new(envelope(RemoteCommand {
                command: "status".to_string(),
                grant_id: Some(grant.grant_id.clone()),
                caller_fingerprint: Some(grant.caller_fingerprint.clone()),
                file_access: grant.file_access,
                ..Default::default()
            })),
        };

        let (mut client, server) = pair().await;
        write_request(&mut client, &start()).await.unwrap();
        drop(client);
        assert!(serve_connection(server, "token")
            .await
            .unwrap_err()
            .contains("client disconnected"));

        let (mut client, server) = pair().await;
        write_request(&mut client, &start()).await.unwrap();
        client.write_all(b"not-json\n").await.unwrap();
        assert!(serve_connection(server, "token")
            .await
            .unwrap_err()
            .contains("parse Remote Execution broker stdin"));

        let (mut client, server) = pair().await;
        write_request(&mut client, &start()).await.unwrap();
        write_request(
            &mut client,
            &BrokerRequest::Stdin {
                data_base64: "%%%".to_string(),
            },
        )
        .await
        .unwrap();
        assert!(serve_connection(server, "token")
            .await
            .unwrap_err()
            .contains("decode Remote Execution broker stdin"));

        let (mut client, server) = pair().await;
        write_request(&mut client, &start()).await.unwrap();
        write_request(
            &mut client,
            &BrokerRequest::Stdin {
                data_base64: base64::engine::general_purpose::STANDARD.encode(vec![
                    0;
                    BROKER_STDIN_CHUNK_BYTES
                        + 1
                ]),
            },
        )
        .await
        .unwrap();
        assert!(serve_connection(server, "token")
            .await
            .unwrap_err()
            .contains("stdin chunk exceeds hard limit"));

        let (mut client, server) = pair().await;
        write_request(&mut client, &start()).await.unwrap();
        write_request(&mut client, &start()).await.unwrap();
        assert!(serve_connection(server, "token")
            .await
            .unwrap_err()
            .contains("duplicate Remote Execution broker start"));
        *runtime_state().write() = None;
    }

    #[test]
    fn persisted_grant_authorization_rejects_every_authority_mismatch() {
        use crate::remote_invoke::types::{FileAccessScope, GrantScope};

        let command = RemoteCommand {
            grant_id: Some("grant-test".to_string()),
            caller_fingerprint: Some("caller".to_string()),
            file_access: FileAccessScope::None,
            ..Default::default()
        };
        let valid = envelope(command.clone());

        let mut grant = active_grant();
        grant.status = GrantStatus::Revoked;
        assert!(
            validate_and_consume_grant("grant-test", &mut grant, &valid, 10)
                .unwrap_err()
                .contains("not active")
        );

        let mut grant = active_grant();
        grant.expires_at = Some(10);
        assert!(
            validate_and_consume_grant("grant-test", &mut grant, &valid, 10)
                .unwrap_err()
                .contains("expired")
        );

        let mut grant = active_grant();
        grant.remaining_calls = Some(0);
        assert!(
            validate_and_consume_grant("grant-test", &mut grant, &valid, 10)
                .unwrap_err()
                .contains("remaining calls")
        );

        let mut bad_command = command.clone();
        bad_command.caller_fingerprint = Some("other".to_string());
        assert!(validate_and_consume_grant(
            "grant-test",
            &mut active_grant(),
            &envelope(bad_command),
            10,
        )
        .unwrap_err()
        .contains("caller fingerprint"));

        let mut grant = active_grant();
        grant.ssh_key_fingerprint = Some("ssh".to_string());
        assert!(
            validate_and_consume_grant("grant-test", &mut grant, &valid, 10)
                .unwrap_err()
                .contains("SSH fingerprint")
        );

        let mut bad_command = command.clone();
        bad_command.file_access = FileAccessScope::Read;
        assert!(validate_and_consume_grant(
            "grant-test",
            &mut active_grant(),
            &envelope(bad_command),
            10,
        )
        .unwrap_err()
        .contains("file access"));

        let mut bad_command = command;
        bad_command.kind = crate::remote_invoke::types::CommandKind::PowerMgmt;
        let mut grant = active_grant();
        grant.grant_scope = GrantScope::RemoteQuery;
        assert!(
            validate_and_consume_grant("grant-test", &mut grant, &envelope(bad_command), 10,)
                .unwrap_err()
                .contains("does not allow")
        );

        let mut grant = active_grant();
        validate_and_consume_grant("grant-test", &mut grant, &valid, 42).unwrap();
        assert_eq!(grant.use_count, 1);
        assert_eq!(grant.last_command_at, Some(42));
        assert_eq!(grant.last_used_at, Some(42));
        assert_eq!(grant.remaining_calls, None);
        assert_eq!(grant.status, GrantStatus::Active);
    }

    #[test]
    fn main_broker_consumes_one_shot_grant_once() {
        use crate::remote_invoke::types::{FileAccessScope, GrantMode};

        let mut grant = active_grant();
        grant.grant_id = "grant-once".to_string();
        grant.grant_mode = GrantMode::Once;
        grant.max_calls = Some(1);
        grant.remaining_calls = Some(1);
        let command = RemoteCommand {
            grant_id: Some("grant-once".to_string()),
            caller_fingerprint: Some("caller".to_string()),
            file_access: FileAccessScope::None,
            ..Default::default()
        };
        let envelope = RemoteExecutionEnvelope::from_command(&command);

        validate_and_consume_grant("grant-once", &mut grant, &envelope, 42).unwrap();
        assert_eq!(grant.remaining_calls, Some(0));
        assert_eq!(grant.status, GrantStatus::Consumed);
        assert_eq!(grant.use_count, 1);
        assert!(validate_and_consume_grant("grant-once", &mut grant, &envelope, 43).is_err());
    }
}
