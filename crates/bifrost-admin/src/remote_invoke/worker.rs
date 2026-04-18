use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bifrost_core::{BifrostError, Result};
use parking_lot::RwLock;
use rand::Rng;
use serde_json::Value;
use tracing::{debug, error, info, warn};

use super::executor::RemoteInvokeExecutor;
use super::identity::Identity;
use super::relay_client::RelayClient;
use super::types::{
    CallerInfo, ClientCallExitRequest, ClientCallFrameRequest, ClientHeartbeatRequest,
    ClientRegistrationRequest, DiscoverySession, EncryptedEnvelope, GrantDecision,
    GrantDecisionRequest, GrantMode, PairingRequest, PublishPairCodeRequest, RemoteCommand,
    RemoteInvokeConfig, WorkerState,
};

const HEARTBEAT_INTERVAL_SECS: u64 = 25;
const INITIAL_RECONNECT_DELAY_MS: u64 = 1000;
const MAX_RECONNECT_DELAY_MS: u64 = 60000;
const PAIR_CODE_DIGITS: u32 = 6;
const PAIR_CODE_REFRESH_CHECK_SECS: u64 = 5;

pub struct RemoteInvokeWorker {
    config: RemoteInvokeConfig,
    identity: Identity,
    relay_client: Arc<RelayClient>,
    executor: Arc<RemoteInvokeExecutor>,
    state: Arc<RwLock<WorkerState>>,
    pending_pairings: Arc<RwLock<HashMap<String, PairingRequest>>>,
    active_calls: Arc<RwLock<HashMap<String, String>>>,
    discovery_session: Arc<RwLock<Option<DiscoverySession>>>,
    shutdown: Arc<AtomicBool>,
    current_stream_id: Arc<RwLock<Option<String>>>,
}

impl RemoteInvokeWorker {
    pub fn new(
        config: RemoteInvokeConfig,
        identity: Identity,
        admin_host: &str,
        admin_port: u16,
    ) -> Arc<Self> {
        let relay_client = Arc::new(RelayClient::new(&config.relay_url, &identity.instance_id));
        let executor = Arc::new(RemoteInvokeExecutor::new(admin_host, admin_port));

        Arc::new(Self {
            config,
            identity,
            relay_client,
            executor,
            state: Arc::new(RwLock::new(WorkerState::Disconnected)),
            pending_pairings: Arc::new(RwLock::new(HashMap::new())),
            active_calls: Arc::new(RwLock::new(HashMap::new())),
            discovery_session: Arc::new(RwLock::new(None)),
            shutdown: Arc::new(AtomicBool::new(false)),
            current_stream_id: Arc::new(RwLock::new(None)),
        })
    }

    pub fn start(self: &Arc<Self>) {
        let worker = Arc::clone(self);
        tokio::spawn(async move {
            worker.run_loop().await;
        });
        info!("remote invoke worker started");
    }

    pub fn stop(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        info!("remote invoke worker stop requested");
    }

    pub fn state(&self) -> WorkerState {
        *self.state.read()
    }

    pub fn discovery_session(&self) -> Option<DiscoverySession> {
        self.discovery_session.read().clone()
    }

    pub fn pending_pairings(&self) -> Vec<PairingRequest> {
        self.pending_pairings.read().values().cloned().collect()
    }

    pub fn active_call_ids(&self) -> Vec<String> {
        self.active_calls.read().keys().cloned().collect()
    }

    pub fn relay_client(&self) -> &Arc<RelayClient> {
        &self.relay_client
    }

    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    pub async fn enter_discovery_mode(&self) -> Result<DiscoverySession> {
        let pair_code = generate_pair_code();
        let session_id = uuid::Uuid::new_v4().to_string();
        let now_ms = now_millis();
        let expires_at = now_ms + self.config.pair_code_ttl_secs * 1000;

        let req = PublishPairCodeRequest {
            client_instance_id: self.identity.instance_id.clone(),
            pair_code: pair_code.clone(),
            expires_at,
            discovery_session_id: Some(session_id.clone()),
        };

        self.relay_client.publish_pair_code(&req).await?;

        let session = DiscoverySession {
            session_id,
            pair_code,
            expires_at,
            created_at: now_ms,
        };

        *self.discovery_session.write() = Some(session.clone());
        info!(
            session_id = %session.session_id,
            pair_code = %session.pair_code,
            "entered discovery mode"
        );
        Ok(session)
    }

    pub async fn exit_discovery_mode(&self) -> Result<()> {
        let session = self.discovery_session.read().clone();
        if let Some(s) = session {
            self.relay_client
                .close_discovery_session(&s.session_id)
                .await?;
            *self.discovery_session.write() = None;
            info!(session_id = %s.session_id, "exited discovery mode");
        }
        Ok(())
    }

    pub async fn refresh_pair_code(&self) -> Result<Option<DiscoverySession>> {
        let current = self.discovery_session.read().clone();
        if current.is_none() {
            return Ok(None);
        }
        let old = current.unwrap();

        let pair_code = generate_pair_code();
        let now_ms = now_millis();
        let expires_at = now_ms + self.config.pair_code_ttl_secs * 1000;

        let req = PublishPairCodeRequest {
            client_instance_id: self.identity.instance_id.clone(),
            pair_code: pair_code.clone(),
            expires_at,
            discovery_session_id: Some(old.session_id.clone()),
        };

        self.relay_client.publish_pair_code(&req).await?;

        let session = DiscoverySession {
            session_id: old.session_id,
            pair_code,
            expires_at,
            created_at: now_ms,
        };

        *self.discovery_session.write() = Some(session.clone());
        info!(pair_code = %session.pair_code, "refreshed pair code");
        Ok(Some(session))
    }

    pub async fn approve_pairing(&self, pairing_id: &str, grant_mode: GrantMode) -> Result<Value> {
        let pairing = {
            let pairings = self.pending_pairings.read();
            pairings.get(pairing_id).cloned()
        };

        if pairing.is_none() {
            return Err(BifrostError::Network(format!(
                "pairing {} not found in pending list",
                pairing_id
            )));
        }

        let req = GrantDecisionRequest {
            pairing_id: pairing_id.to_string(),
            client_instance_id: self.identity.instance_id.clone(),
            decision: GrantDecision::Approve,
            grant_mode: Some(grant_mode),
            client_ephemeral_pub: None,
        };

        let result = self
            .relay_client
            .submit_grant_decision(pairing_id, &req)
            .await?;
        self.pending_pairings.write().remove(pairing_id);

        if let Some(ds) = self.discovery_session.read().as_ref() {
            if ds.expires_at <= now_millis() {
                *self.discovery_session.write() = None;
            }
        }

        info!(pairing_id = %pairing_id, "approved pairing");
        Ok(result)
    }

    pub async fn reject_pairing(&self, pairing_id: &str) -> Result<Value> {
        let req = GrantDecisionRequest {
            pairing_id: pairing_id.to_string(),
            client_instance_id: self.identity.instance_id.clone(),
            decision: GrantDecision::Reject,
            grant_mode: None,
            client_ephemeral_pub: None,
        };

        let result = self
            .relay_client
            .submit_grant_decision(pairing_id, &req)
            .await?;
        self.pending_pairings.write().remove(pairing_id);
        info!(pairing_id = %pairing_id, "rejected pairing");
        Ok(result)
    }

    async fn run_loop(&self) {
        let mut reconnect_delay = INITIAL_RECONNECT_DELAY_MS;

        loop {
            if self.shutdown.load(Ordering::SeqCst) {
                info!("remote invoke worker shutting down");
                *self.state.write() = WorkerState::Disconnected;
                return;
            }

            *self.state.write() = WorkerState::Registering;

            match self.register_with_relay().await {
                Ok(_) => {
                    info!("registered with relay server");
                    reconnect_delay = INITIAL_RECONNECT_DELAY_MS;
                }
                Err(e) => {
                    error!(error = %e, "failed to register with relay");
                    *self.state.write() = WorkerState::Disconnected;
                    self.sleep_with_shutdown_check(reconnect_delay).await;
                    reconnect_delay = (reconnect_delay * 2).min(MAX_RECONNECT_DELAY_MS);
                    continue;
                }
            }

            *self.state.write() = WorkerState::Connecting;

            match self.run_sse_session().await {
                Ok(_) => {
                    info!("SSE session ended normally");
                    reconnect_delay = INITIAL_RECONNECT_DELAY_MS;
                }
                Err(e) => {
                    warn!(error = %e, "SSE session ended with error");
                }
            }

            if self.shutdown.load(Ordering::SeqCst) {
                return;
            }

            *self.state.write() = WorkerState::Reconnecting;
            info!(delay_ms = reconnect_delay, "reconnecting to relay");
            self.sleep_with_shutdown_check(reconnect_delay).await;
            reconnect_delay = (reconnect_delay * 2).min(MAX_RECONNECT_DELAY_MS);
        }
    }

    async fn register_with_relay(&self) -> Result<()> {
        let now = now_millis() / 1000;
        let signature = format!("{}:{}", self.identity.instance_id, now);

        let req = ClientRegistrationRequest {
            client_instance_id: self.identity.instance_id.clone(),
            client_long_term_pubkey: self.identity.long_term_pubkey.clone(),
            device_name: self.identity.device_name.clone(),
            platform: self.identity.platform.clone(),
            bifrost_version: env!("CARGO_PKG_VERSION").to_string(),
            signature,
            timestamp: now,
        };

        let resp = self.relay_client.register(&req).await?;
        self.relay_client.set_auth_token(resp.client_auth_token);
        info!("relay auth token acquired, expires_at={}", resp.expires_at);
        Ok(())
    }

    async fn run_sse_session(&self) -> Result<()> {
        let stream_id = uuid::Uuid::new_v4().to_string();
        *self.current_stream_id.write() = Some(stream_id.clone());

        let url = self.relay_client.build_stream_url(&stream_id);
        info!(stream_id = %stream_id, "connecting SSE stream");

        let http = bifrost_core::direct_reqwest_client_builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| BifrostError::Network(format!("build sse client: {}", e)))?;

        let response = http
            .get(&url)
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("SSE connect failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(BifrostError::Network(format!(
                "SSE connect returned status {}",
                response.status()
            )));
        }

        *self.state.write() = WorkerState::Connected;
        info!(stream_id = %stream_id, "SSE stream connected");

        let heartbeat_interval = Duration::from_secs(HEARTBEAT_INTERVAL_SECS);
        let mut heartbeat_ticker = tokio::time::interval(heartbeat_interval);
        heartbeat_ticker.tick().await;

        let pair_code_check_interval = Duration::from_secs(PAIR_CODE_REFRESH_CHECK_SECS);
        let mut pair_code_ticker = tokio::time::interval(pair_code_check_interval);
        pair_code_ticker.tick().await;

        let mut event_name = String::new();
        let mut data_buf = String::new();

        let mut stream = response.bytes_stream();
        use futures_util::StreamExt;

        let mut partial_line = String::new();

        loop {
            tokio::select! {
                _ = heartbeat_ticker.tick() => {
                    if self.shutdown.load(Ordering::SeqCst) {
                        return Ok(());
                    }
                    if let Err(e) = self.send_heartbeat(&stream_id).await {
                        warn!(error = %e, "heartbeat failed");
                    }
                }
                _ = pair_code_ticker.tick() => {
                    self.maybe_refresh_pair_code().await;
                }
                chunk = stream.next() => {
                    match chunk {
                        Some(Ok(bytes)) => {
                            let text = String::from_utf8_lossy(&bytes);
                            debug!(
                                bytes_len = bytes.len(),
                                "SSE chunk received"
                            );
                            partial_line.push_str(&text);

                            while let Some(newline_pos) = partial_line.find('\n') {
                                let line = partial_line[..newline_pos].trim_end_matches('\r').to_string();
                                partial_line = partial_line[newline_pos + 1..].to_string();

                                if line.is_empty() {
                                    if !event_name.is_empty() && !data_buf.is_empty() {
                                        self.dispatch_sse_event(&event_name, &data_buf).await;
                                    }
                                    event_name.clear();
                                    data_buf.clear();
                                } else if let Some(ev) = line.strip_prefix("event:") {
                                    event_name = ev.trim().to_string();
                                } else if let Some(d) = line.strip_prefix("data:") {
                                    if !data_buf.is_empty() {
                                        data_buf.push('\n');
                                    }
                                    data_buf.push_str(d.trim());
                                } else if line.starts_with(':') {
                                    debug!(comment = %line, "SSE comment");
                                }
                            }
                        }
                        Some(Err(e)) => {
                            return Err(BifrostError::Network(format!("SSE read error: {}", e)));
                        }
                        None => {
                            info!("SSE stream closed by server");
                            return Ok(());
                        }
                    }
                }
            }
        }
    }

    async fn send_heartbeat(&self, stream_id: &str) -> Result<()> {
        let active_ids = self.active_call_ids();
        let req = ClientHeartbeatRequest {
            client_instance_id: self.identity.instance_id.clone(),
            stream_id: stream_id.to_string(),
            active_call_ids: active_ids,
        };
        self.relay_client.heartbeat(&req).await?;
        debug!("heartbeat sent");
        Ok(())
    }

    async fn maybe_refresh_pair_code(&self) {
        let needs_refresh = {
            let session = self.discovery_session.read();
            match session.as_ref() {
                Some(ds) => ds.expires_at <= now_millis(),
                None => false,
            }
        };

        if needs_refresh {
            match self.refresh_pair_code().await {
                Ok(Some(new_session)) => {
                    info!(
                        pair_code = %new_session.pair_code,
                        expires_at = new_session.expires_at,
                        "auto-refreshed expired pair code"
                    );
                }
                Ok(None) => {}
                Err(e) => {
                    warn!(error = %e, "failed to auto-refresh pair code");
                }
            }
        }
    }

    async fn dispatch_sse_event(&self, event_name: &str, data: &str) {
        info!(
            event = %event_name,
            data_len = data.len(),
            "dispatching SSE event"
        );

        match event_name {
            "client_hello_ack" => {
                info!("received client_hello_ack from relay");
                *self.state.write() = WorkerState::Connected;
            }
            "pairing_request" => match serde_json::from_str::<Value>(data) {
                Ok(v) => self.handle_pairing_request(v).await,
                Err(e) => warn!(error = %e, "failed to parse pairing_request"),
            },
            "grant_created" => match serde_json::from_str::<Value>(data) {
                Ok(v) => self.handle_grant_created(v).await,
                Err(e) => warn!(error = %e, "failed to parse grant_created"),
            },
            "call_open" => match serde_json::from_str::<Value>(data) {
                Ok(v) => self.handle_call_open(v).await,
                Err(e) => warn!(error = %e, "failed to parse call_open"),
            },
            "call_frame" => {
                debug!(
                    data_len = data.len(),
                    "call_frame received (stdin forwarding not yet implemented)"
                );
            }
            "call_cancel" => match serde_json::from_str::<Value>(data) {
                Ok(v) => {
                    if let Some(call_id) = v.get("call_id").and_then(|c| c.as_str()) {
                        info!(call_id = %call_id, "call cancelled by caller");
                        self.active_calls.write().remove(call_id);
                    }
                }
                Err(e) => warn!(error = %e, "failed to parse call_cancel"),
            },
            "grant_revoked" => match serde_json::from_str::<Value>(data) {
                Ok(v) => {
                    if let Some(grant_id) = v.get("grant_id").and_then(|g| g.as_str()) {
                        info!(grant_id = %grant_id, "grant revoked, sending ack");
                        let rc = Arc::clone(&self.relay_client);
                        let gid = grant_id.to_string();
                        tokio::spawn(async move {
                            if let Err(e) = rc.revoke_ack(&gid).await {
                                warn!(error = %e, grant_id = %gid, "revoke_ack failed");
                            }
                        });
                    }
                }
                Err(e) => warn!(error = %e, "failed to parse grant_revoked"),
            },
            "ping" => {
                debug!("ping from relay");
            }
            "replaced" => {
                info!("SSE stream replaced by newer connection");
                *self.state.write() = WorkerState::Disconnected;
            }
            _ => {
                debug!(event = %event_name, "unknown SSE event");
            }
        }
    }

    async fn handle_grant_created(&self, data: Value) {
        let call_id = match data.get("call_id").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(),
            None => {
                warn!("grant_created missing call_id");
                return;
            }
        };
        let grant_id = data
            .get("grant_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let command: RemoteCommand = match data.get("command") {
            Some(v) => match serde_json::from_value(v.clone()) {
                Ok(c) => c,
                Err(e) => {
                    warn!(error = %e, call_id = %call_id, "failed to parse command in grant_created");
                    self.send_call_exit(&call_id, -1, None, None, 0).await;
                    return;
                }
            },
            None => {
                warn!(call_id = %call_id, "grant_created missing command");
                self.send_call_exit(&call_id, -1, None, None, 0).await;
                return;
            }
        };

        info!(
            call_id = %call_id,
            grant_id = %grant_id,
            command = %command.command,
            "executing remote command from grant_created"
        );

        self.active_calls
            .write()
            .insert(call_id.clone(), grant_id.clone());

        let executor = Arc::clone(&self.executor);
        let relay_client = Arc::clone(&self.relay_client);
        let instance_id = self.identity.instance_id.clone();
        let active_calls = Arc::clone(&self.active_calls);
        let cid = call_id.clone();

        tokio::spawn(async move {
            let start = std::time::Instant::now();
            let result = executor.execute(&command).await;
            let duration_ms = start.elapsed().as_millis() as u64;

            match result {
                Ok(response) => {
                    if let Some(ref stdout) = response.stdout {
                        let envelope = EncryptedEnvelope {
                            version: 1,
                            call_id: cid.clone(),
                            seq: 1,
                            direction: super::types::FrameDirection::ClientToCaller,
                            nonce: String::new(),
                            ciphertext: stdout.clone(),
                            tag: String::new(),
                            aad: None,
                        };

                        let envelope_json = serde_json::to_string(&envelope).unwrap_or_default();
                        let frame_req = ClientCallFrameRequest {
                            call_id: cid.clone(),
                            client_instance_id: instance_id.clone(),
                            envelope_json,
                        };

                        if let Err(e) = relay_client.post_call_frame(&cid, &frame_req).await {
                            error!(error = %e, call_id = %cid, "failed to post call frame");
                        }
                    }

                    let exit_req = ClientCallExitRequest {
                        call_id: cid.clone(),
                        client_instance_id: instance_id,
                        exit_code: response.exit_code,
                        duration_ms: Some(duration_ms),
                        stdout_digest: response.stdout_digest.clone(),
                        stderr_digest: response.stderr_digest.clone(),
                        bytes_in: Some(0),
                        bytes_out: response.stdout.as_ref().map(|s| s.len() as u64),
                    };

                    if let Err(e) = relay_client.post_call_exit(&cid, &exit_req).await {
                        error!(error = %e, call_id = %cid, "failed to post call exit");
                    }

                    info!(
                        call_id = %cid,
                        exit_code = response.exit_code,
                        duration_ms = duration_ms,
                        "remote command execution completed (grant_created)"
                    );
                }
                Err(e) => {
                    error!(error = %e, call_id = %cid, "remote command execution failed (grant_created)");

                    let exit_req = ClientCallExitRequest {
                        call_id: cid.clone(),
                        client_instance_id: instance_id,
                        exit_code: -1,
                        duration_ms: Some(duration_ms),
                        stdout_digest: None,
                        stderr_digest: None,
                        bytes_in: Some(0),
                        bytes_out: Some(0),
                    };

                    if let Err(e2) = relay_client.post_call_exit(&cid, &exit_req).await {
                        error!(error = %e2, call_id = %cid, "failed to post error call exit");
                    }
                }
            }

            active_calls.write().remove(&cid);
        });
    }

    async fn handle_pairing_request(&self, data: Value) {
        let pairing_id = match data.get("pairing_id").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(),
            None => {
                warn!("pairing_request missing pairing_id");
                return;
            }
        };

        let caller_info = if let Some(ci) = data.get("caller_info") {
            serde_json::from_value(ci.clone()).unwrap_or_default()
        } else {
            CallerInfo {
                fingerprint: data
                    .get("caller_fingerprint")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                display_name: data
                    .get("caller_display_name")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                user_agent: data
                    .get("user_agent")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                source_ip: data
                    .get("source_ip")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                platform: data
                    .get("platform")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
            }
        };

        let command_summary_val = data.get("command_summary").cloned().unwrap_or(Value::Null);
        let command_val = data.get("command").cloned().unwrap_or(Value::Null);
        let caller_pubkey = data
            .get("caller_pubkey")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let command_summary = serde_json::from_value(command_summary_val).unwrap_or_default();
        let command = serde_json::from_value(command_val).unwrap_or_default();

        let request = PairingRequest {
            pairing_id: pairing_id.clone(),
            caller_info,
            command_summary,
            command,
            caller_pubkey,
        };

        info!(
            pairing_id = %pairing_id,
            "received pairing request, awaiting user decision"
        );

        self.pending_pairings.write().insert(pairing_id, request);
    }

    async fn handle_call_open(&self, data: Value) {
        let call_id = match data.get("call_id").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(),
            None => {
                warn!("call_open missing call_id");
                return;
            }
        };
        let grant_id = data
            .get("grant_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let command: RemoteCommand = match data.get("command") {
            Some(v) => match serde_json::from_value(v.clone()) {
                Ok(c) => c,
                Err(e) => {
                    warn!(error = %e, call_id = %call_id, "failed to parse command in call_open");
                    self.send_call_exit(&call_id, -1, None, None, 0).await;
                    return;
                }
            },
            None => {
                warn!(call_id = %call_id, "call_open missing command");
                self.send_call_exit(&call_id, -1, None, None, 0).await;
                return;
            }
        };

        info!(
            call_id = %call_id,
            grant_id = %grant_id,
            command = %command.command,
            "executing remote command"
        );

        self.active_calls
            .write()
            .insert(call_id.clone(), grant_id.clone());

        let executor = Arc::clone(&self.executor);
        let relay_client = Arc::clone(&self.relay_client);
        let instance_id = self.identity.instance_id.clone();
        let active_calls = Arc::clone(&self.active_calls);
        let cid = call_id.clone();

        tokio::spawn(async move {
            let start = std::time::Instant::now();
            let result = executor.execute(&command).await;
            let duration_ms = start.elapsed().as_millis() as u64;

            match result {
                Ok(response) => {
                    if let Some(ref stdout) = response.stdout {
                        let envelope = EncryptedEnvelope {
                            version: 1,
                            call_id: cid.clone(),
                            seq: 1,
                            direction: super::types::FrameDirection::ClientToCaller,
                            nonce: String::new(),
                            ciphertext: stdout.clone(),
                            tag: String::new(),
                            aad: None,
                        };

                        let envelope_json = serde_json::to_string(&envelope).unwrap_or_default();
                        let frame_req = ClientCallFrameRequest {
                            call_id: cid.clone(),
                            client_instance_id: instance_id.clone(),
                            envelope_json,
                        };

                        if let Err(e) = relay_client.post_call_frame(&cid, &frame_req).await {
                            error!(error = %e, call_id = %cid, "failed to post call frame");
                        }
                    }

                    let exit_req = ClientCallExitRequest {
                        call_id: cid.clone(),
                        client_instance_id: instance_id,
                        exit_code: response.exit_code,
                        duration_ms: Some(duration_ms),
                        stdout_digest: response.stdout_digest.clone(),
                        stderr_digest: response.stderr_digest.clone(),
                        bytes_in: Some(0),
                        bytes_out: response.stdout.as_ref().map(|s| s.len() as u64),
                    };

                    if let Err(e) = relay_client.post_call_exit(&cid, &exit_req).await {
                        error!(error = %e, call_id = %cid, "failed to post call exit");
                    }

                    info!(
                        call_id = %cid,
                        exit_code = response.exit_code,
                        duration_ms = duration_ms,
                        "remote command execution completed"
                    );
                }
                Err(e) => {
                    error!(error = %e, call_id = %cid, "remote command execution failed");

                    let exit_req = ClientCallExitRequest {
                        call_id: cid.clone(),
                        client_instance_id: instance_id,
                        exit_code: -1,
                        duration_ms: Some(duration_ms),
                        stdout_digest: None,
                        stderr_digest: None,
                        bytes_in: Some(0),
                        bytes_out: Some(0),
                    };

                    if let Err(e2) = relay_client.post_call_exit(&cid, &exit_req).await {
                        error!(error = %e2, call_id = %cid, "failed to post error call exit");
                    }
                }
            }

            active_calls.write().remove(&cid);
        });
    }

    async fn send_call_exit(
        &self,
        call_id: &str,
        exit_code: i32,
        stdout_digest: Option<String>,
        stderr_digest: Option<String>,
        duration_ms: u64,
    ) {
        let req = ClientCallExitRequest {
            call_id: call_id.to_string(),
            client_instance_id: self.identity.instance_id.clone(),
            exit_code,
            duration_ms: Some(duration_ms),
            stdout_digest,
            stderr_digest,
            bytes_in: Some(0),
            bytes_out: Some(0),
        };

        if let Err(e) = self.relay_client.post_call_exit(call_id, &req).await {
            error!(error = %e, call_id = %call_id, "failed to send call exit");
        }
    }

    async fn sleep_with_shutdown_check(&self, delay_ms: u64) {
        let mut remaining = delay_ms;

        while remaining > 0 && !self.shutdown.load(Ordering::SeqCst) {
            let sleep_time = remaining.min(500);
            tokio::time::sleep(Duration::from_millis(sleep_time)).await;
            remaining = remaining.saturating_sub(sleep_time);
        }
    }
}

fn generate_pair_code() -> String {
    let mut rng = rand::thread_rng();
    let code: u32 = rng.gen_range(0..10u32.pow(PAIR_CODE_DIGITS));
    format!("{:0>width$}", code, width = PAIR_CODE_DIGITS as usize)
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
