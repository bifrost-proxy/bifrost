use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bifrost_core::{BifrostError, Result};
use bifrost_sync::SyncManagerHandle;
use parking_lot::{Mutex, RwLock};
use rand::Rng;
use serde_json::Value;
use tokio::sync::Notify;
use tracing::{debug, error, info, warn};

use super::executor::RemoteInvokeExecutor;
use super::identity::Identity;
use super::relay_client::RelayClient;
use super::ssh_keys::{SshKeyMaterial, SshKeyRecord, SshKeyStore};
use super::types::{
    build_registration_signature_payload, grant_mode_ttl_ms, AuthMethod, CallInfo, CallStatus,
    CallerInfo, ClientCallExitRequest, ClientCallFrameRequest, ClientHeartbeatRequest,
    ClientRegistrationChallengeRequest, ClientRegistrationRequest, CommandSummary,
    DiscoverySession, EncryptedEnvelope, GrantDecision, GrantDecisionRequest, GrantInfo, GrantMode,
    GrantStatus, PairingRequest, PublishPairCodeRequest, RemoteCommand, RemoteInvokeConfig,
    SshConnectEvent, SshConnectResultRequest, SshConnectResultStatus, WorkerState,
};

const HEARTBEAT_INTERVAL_SECS: u64 = 25;
const INITIAL_RECONNECT_DELAY_MS: u64 = 1000;
const MAX_RECONNECT_DELAY_MS: u64 = 60000;
const PAIR_CODE_DIGITS: u32 = 6;
const PAIR_CODE_REFRESH_CHECK_SECS: u64 = 5;
const GRANT_CLEANUP_INTERVAL_SECS: u64 = 60;
const PENDING_PAIRING_POLL_SECS: u64 = 5;
const ACTIVE_CALL_RECONCILE_INTERVAL_MS: u64 = 1000;

struct TimestampedPairing {
    request: PairingRequest,
    received_at: u64,
}

struct ActiveCallControl {
    grant_id: String,
    started_at: u64,
    cancelled: AtomicBool,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl ActiveCallControl {
    fn new(grant_id: String, started_at: u64) -> Self {
        Self {
            grant_id,
            started_at,
            cancelled: AtomicBool::new(false),
            task: Mutex::new(None),
        }
    }

    fn mark_cancelled(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    fn abort_task(&self) {
        if let Some(handle) = self.task.lock().take() {
            handle.abort();
        }
    }
}

pub struct RemoteInvokeWorker {
    config: RemoteInvokeConfig,
    identity: Identity,
    sync_manager: Option<SyncManagerHandle>,
    relay_client: Arc<RelayClient>,
    executor: Arc<RemoteInvokeExecutor>,
    state: Arc<RwLock<WorkerState>>,
    pending_pairings: Arc<RwLock<HashMap<String, TimestampedPairing>>>,
    active_calls: Arc<RwLock<HashMap<String, Arc<ActiveCallControl>>>>,
    call_history: Arc<RwLock<VecDeque<CallInfo>>>,
    local_grants: Arc<RwLock<HashMap<String, GrantInfo>>>,
    discovery_session: Arc<RwLock<Option<DiscoverySession>>>,
    ssh_key_store: Arc<SshKeyStore>,
    shutdown: Arc<AtomicBool>,
    current_stream_id: Arc<RwLock<Option<String>>>,
    reconnect_notify: Arc<Notify>,
}

impl RemoteInvokeWorker {
    pub fn new(
        config: RemoteInvokeConfig,
        identity: Identity,
        sync_manager: Option<SyncManagerHandle>,
        admin_host: &str,
        admin_port: u16,
    ) -> Arc<Self> {
        let relay_client = Arc::new(RelayClient::new(
            &config.relay_url,
            &identity.instance_id,
            &identity.device_name,
            &identity.platform,
        ));
        let executor = Arc::new(RemoteInvokeExecutor::new(admin_host, admin_port));
        let ssh_key_store = Arc::new(SshKeyStore::new(&bifrost_storage::data_dir()));

        Arc::new(Self {
            config,
            identity,
            sync_manager,
            relay_client,
            executor,
            state: Arc::new(RwLock::new(WorkerState::Disconnected)),
            pending_pairings: Arc::new(RwLock::new(HashMap::new())),
            active_calls: Arc::new(RwLock::new(HashMap::new())),
            call_history: Arc::new(RwLock::new(VecDeque::new())),
            local_grants: Arc::new(RwLock::new(HashMap::new())),
            discovery_session: Arc::new(RwLock::new(None)),
            ssh_key_store,
            shutdown: Arc::new(AtomicBool::new(false)),
            current_stream_id: Arc::new(RwLock::new(None)),
            reconnect_notify: Arc::new(Notify::new()),
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
        for active_call in self.active_calls.read().values() {
            active_call.mark_cancelled();
            active_call.abort_task();
        }
        info!("remote invoke worker stop requested");
    }

    pub fn state(&self) -> WorkerState {
        *self.state.read()
    }

    pub fn discovery_session(&self) -> Option<DiscoverySession> {
        self.discovery_session.read().clone()
    }

    pub fn pending_pairings(&self) -> Vec<PairingRequest> {
        self.pending_pairings
            .read()
            .values()
            .map(|tp| tp.request.clone())
            .collect()
    }

    pub fn active_call_ids(&self) -> Vec<String> {
        self.active_calls.read().keys().cloned().collect()
    }

    pub fn list_calls(&self) -> Vec<CallInfo> {
        self.call_history.read().iter().rev().cloned().collect()
    }

    pub fn get_call(&self, call_id: &str) -> Option<CallInfo> {
        self.call_history
            .read()
            .iter()
            .find(|c| c.call_id == call_id)
            .cloned()
    }

    pub fn relay_client(&self) -> &Arc<RelayClient> {
        &self.relay_client
    }

    pub fn update_relay_url(&self, new_url: &str) {
        let old_url = self.relay_client.base_url();
        let new_normalized = new_url.trim_end_matches('/');
        if old_url == new_normalized {
            debug!(url = %new_normalized, "relay_url unchanged, skip reconnect");
            return;
        }
        info!(
            old_url = %old_url,
            new_url = %new_normalized,
            "relay_url changed, triggering reconnect"
        );
        self.relay_client.update_base_url(new_url);
        *self.discovery_session.write() = None;
        self.pending_pairings.write().clear();
        self.local_grants.write().clear();
        self.reconnect_notify.notify_waiters();
    }

    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    pub fn get_active_ssh_key(&self) -> Result<Option<SshKeyRecord>> {
        Ok(self.ssh_key_store.get_active_key()?.map(|key| key.record))
    }

    pub fn export_active_ssh_key(&self) -> Result<Option<SshKeyMaterial>> {
        self.ssh_key_store.export_active_key_material()
    }

    pub fn create_ssh_key(&self, label: String, grant_mode: GrantMode) -> Result<SshKeyMaterial> {
        self.revoke_local_ssh_grants(None);
        let result = self
            .ssh_key_store
            .create_or_replace_key(label, grant_mode)?;
        self.trigger_ssh_route_refresh();
        Ok(result)
    }

    pub fn update_ssh_key(
        &self,
        label: Option<String>,
        grant_mode: Option<GrantMode>,
    ) -> Result<Option<SshKeyRecord>> {
        let updated = self.ssh_key_store.update_active_key(label, grant_mode)?;
        if updated.is_some() {
            self.trigger_ssh_route_refresh();
        }
        Ok(updated)
    }

    pub fn reset_ssh_key(&self) -> Result<Option<SshKeyMaterial>> {
        let Some(active_key) = self.get_active_ssh_key()? else {
            return Ok(None);
        };

        self.revoke_local_ssh_grants(Some(&active_key.id));
        let reset = self
            .ssh_key_store
            .create_or_replace_key(active_key.label, GrantMode::Permanent)?;
        self.trigger_ssh_route_refresh();
        Ok(Some(reset))
    }

    pub fn revoke_ssh_key(&self) -> Result<Option<SshKeyRecord>> {
        let revoked = self.ssh_key_store.revoke_active_key()?;
        if revoked.is_some() {
            self.revoke_local_ssh_grants(revoked.as_ref().map(|record| record.id.as_str()));
            self.trigger_ssh_route_refresh();
        }
        Ok(revoked)
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

        if let Err(e) = self.relay_client.publish_pair_code(&req).await {
            if is_relay_unauthorized(&e) {
                warn!("publish_pair_code unauthorized, re-registering with relay");
                self.register_with_relay().await?;
                self.relay_client.publish_pair_code(&req).await?;
            } else {
                return Err(e);
            }
        }

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
            if let Err(e) = self
                .relay_client
                .close_discovery_session(&s.session_id)
                .await
            {
                if is_relay_unauthorized(&e) {
                    warn!("close_discovery_session unauthorized, re-registering with relay");
                    self.register_with_relay().await?;
                    self.relay_client
                        .close_discovery_session(&s.session_id)
                        .await?;
                } else {
                    return Err(e);
                }
            }
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

        if let Err(e) = self.relay_client.publish_pair_code(&req).await {
            if is_relay_unauthorized(&e) {
                warn!("refresh publish_pair_code unauthorized, re-registering with relay");
                self.register_with_relay().await?;
                self.relay_client.publish_pair_code(&req).await?;
            } else {
                return Err(e);
            }
        }

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
        let found = {
            let pairings = self.pending_pairings.read();
            pairings.contains_key(pairing_id)
        };

        if !found {
            return Err(BifrostError::Network(format!(
                "pairing {} not found or expired",
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

        let (caller_fingerprint_from_pairing, caller_display_name_from_pairing) = self
            .pending_pairings
            .read()
            .get(pairing_id)
            .map(|tp| {
                (
                    tp.request.caller_info.fingerprint.clone(),
                    tp.request.caller_info.display_name.clone(),
                )
            })
            .unwrap_or_default();

        self.pending_pairings.write().remove(pairing_id);

        let now = now_millis();
        let grant_id = result
            .get("grant_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if !grant_id.is_empty() {
            let caller_fingerprint = result
                .get("caller_fingerprint")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .unwrap_or(caller_fingerprint_from_pairing);
            let client_instance_id = result
                .get("client_instance_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| self.identity.instance_id.clone());
            let expires_at = result
                .get("expires_at")
                .and_then(|v| v.as_u64())
                .or_else(|| grant_mode_ttl_ms(grant_mode).map(|ttl| now + ttl));

            let caller_display_name = result
                .get("caller_display_name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or(caller_display_name_from_pairing);
            let grant_info = GrantInfo {
                grant_id: grant_id.clone(),
                client_instance_id,
                caller_fingerprint,
                caller_display_name,
                grant_mode,
                grant_scope: "remote_invoke".to_string(),
                auth_method: AuthMethod::PairCode,
                status: GrantStatus::Active,
                first_authorized_at: now,
                expires_at,
                last_used_at: None,
                max_calls: if grant_mode == GrantMode::Once {
                    Some(1)
                } else {
                    None
                },
                remaining_calls: if grant_mode == GrantMode::Once {
                    Some(1)
                } else {
                    None
                },
                ssh_key_id: None,
                ssh_key_fingerprint: None,
            };
            self.local_grants
                .write()
                .insert(grant_id.clone(), grant_info);
            debug!(grant_id = %grant_id, "inserted grant into local_grants from approve_pairing");
        }

        if let Some(ds) = self.discovery_session.read().as_ref() {
            if ds.expires_at <= now_millis() {
                *self.discovery_session.write() = None;
            }
        }

        info!(pairing_id = %pairing_id, "approved pairing");
        Ok(result)
    }

    pub async fn reject_pairing(&self, pairing_id: &str) -> Result<Value> {
        let found = {
            let pairings = self.pending_pairings.read();
            pairings.contains_key(pairing_id)
        };

        if !found {
            return Err(BifrostError::Network(format!(
                "pairing {} not found or expired",
                pairing_id
            )));
        }

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
        let user_auth_token = self
            .sync_manager
            .as_ref()
            .and_then(|manager| manager.session_token());
        let Some(user_auth_token) = user_auth_token else {
            return Err(BifrostError::Network(
                "remote invoke client registration requires a sync session token".to_string(),
            ));
        };
        let challenge = self
            .relay_client
            .request_registration_challenge(
                &ClientRegistrationChallengeRequest {
                    client_instance_id: self.identity.instance_id.clone(),
                },
                Some(user_auth_token.as_str()),
            )
            .await?;
        let payload = build_registration_signature_payload(
            &challenge.challenge_id,
            &challenge.challenge,
            &self.identity.instance_id,
            &self.identity.device_name,
            &self.identity.platform,
            env!("CARGO_PKG_VERSION"),
            &self.identity.long_term_pubkey,
            now,
        );
        let signature = self
            .identity
            .sign_registration_payload(payload.as_bytes())?;

        let req = ClientRegistrationRequest {
            challenge_id: challenge.challenge_id,
            client_instance_id: self.identity.instance_id.clone(),
            client_long_term_pubkey: self.identity.long_term_pubkey.clone(),
            device_name: self.identity.device_name.clone(),
            platform: self.identity.platform.clone(),
            bifrost_version: env!("CARGO_PKG_VERSION").to_string(),
            signature,
            timestamp: now,
            ssh_device_route: self.ssh_key_store.active_route().unwrap_or(None),
        };

        let resp = self
            .relay_client
            .register(&req, Some(user_auth_token.as_str()))
            .await?;
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

        if let Err(e) = self.relay_client.cancel_pending_pairings().await {
            debug!(error = %e, "cancel_pending_pairings on SSE connect (non-fatal)");
        } else {
            self.pending_pairings.write().clear();
            info!("cleared stale pending pairings on SSE reconnect");
        }

        match self.relay_client.fetch_active_grants().await {
            Ok(grants_data) => {
                let now = now_millis();
                let mut count = 0u32;
                for item in &grants_data {
                    if let Some(gi) =
                        build_grant_info_from_grant_created(item, &self.identity.instance_id, now)
                    {
                        let gid = gi.grant_id.clone();
                        self.local_grants.write().insert(gid, gi);
                        count += 1;
                    }
                }
                if count > 0 {
                    info!(
                        count = count,
                        "synced active grants from relay on SSE connect"
                    );
                }
            }
            Err(e) => {
                debug!(error = %e, "fetch_active_grants on SSE connect (non-fatal, relay may not support this endpoint)");
            }
        }

        let heartbeat_interval = Duration::from_secs(HEARTBEAT_INTERVAL_SECS);
        let mut heartbeat_ticker = tokio::time::interval(heartbeat_interval);
        heartbeat_ticker.tick().await;

        let pair_code_check_interval = Duration::from_secs(PAIR_CODE_REFRESH_CHECK_SECS);
        let mut pair_code_ticker = tokio::time::interval(pair_code_check_interval);
        pair_code_ticker.tick().await;

        let grant_cleanup_interval = Duration::from_secs(GRANT_CLEANUP_INTERVAL_SECS);
        let mut grant_cleanup_ticker = tokio::time::interval(grant_cleanup_interval);
        grant_cleanup_ticker.tick().await;

        let pending_poll_interval = Duration::from_secs(PENDING_PAIRING_POLL_SECS);
        let mut pending_poll_ticker = tokio::time::interval(pending_poll_interval);
        pending_poll_ticker.tick().await;

        let active_call_reconcile_interval =
            Duration::from_millis(ACTIVE_CALL_RECONCILE_INTERVAL_MS);
        let mut active_call_reconcile_ticker =
            tokio::time::interval(active_call_reconcile_interval);
        active_call_reconcile_ticker.tick().await;

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
                        if is_relay_unauthorized(&e) {
                            warn!(error = %e, "heartbeat auth rejected, triggering reconnect");
                            return Err(e);
                        }
                        warn!(error = %e, "heartbeat failed");
                    }
                }
                _ = pair_code_ticker.tick() => {
                    self.maybe_refresh_pair_code().await;
                }
                _ = grant_cleanup_ticker.tick() => {
                    self.periodic_grant_cleanup();
                }
                _ = pending_poll_ticker.tick() => {
                    self.poll_pending_pairings_from_relay().await;
                }
                _ = active_call_reconcile_ticker.tick() => {
                    self.reconcile_active_calls_with_relay().await;
                }
                _ = self.reconnect_notify.notified() => {
                    info!("reconnect signal received during SSE session, disconnecting");
                    return Ok(());
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
            ssh_device_route: self.ssh_key_store.active_route().unwrap_or(None),
        };
        self.relay_client.heartbeat(&req).await?;
        debug!("heartbeat sent");
        Ok(())
    }

    fn cleanup_expired_pairings(&self) {
        let ttl_ms = self.config.pair_code_ttl_secs * 1000;
        let now = now_millis();
        let mut pairings = self.pending_pairings.write();
        let before = pairings.len();
        pairings.retain(|id, tp| {
            let alive = now.saturating_sub(tp.received_at) < ttl_ms;
            if !alive {
                info!(pairing_id = %id, age_secs = (now - tp.received_at) / 1000, "removing expired pairing request");
            }
            alive
        });
        let removed = before - pairings.len();
        if removed > 0 {
            debug!(
                removed = removed,
                remaining = pairings.len(),
                "expired pairings cleanup done"
            );
        }
    }

    async fn poll_pending_pairings_from_relay(&self) {
        if self.discovery_session.read().is_none() {
            return;
        }

        let pairings = match self.relay_client.poll_pending_pairings().await {
            Ok(p) => p,
            Err(e) => {
                debug!(error = %e, "poll_pending_pairings_from_relay failed (non-fatal)");
                return;
            }
        };

        if pairings.is_empty() {
            return;
        }

        let now = now_millis();
        let mut added = 0u32;
        for p in pairings {
            let pairing_id = match p.get("pairing_id").and_then(|v| v.as_str()) {
                Some(id) => id.to_string(),
                None => continue,
            };

            if self.pending_pairings.read().contains_key(&pairing_id) {
                continue;
            }

            let caller_info = CallerInfo {
                fingerprint: p
                    .get("caller_fingerprint")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                display_name: p
                    .get("caller_display_name")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                user_agent: p
                    .get("user_agent")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                source_ip: p
                    .get("source_ip")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                platform: p
                    .get("platform")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                hostname: p
                    .get("hostname")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                username: p
                    .get("username")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
            };
            let caller_pubkey = p
                .get("caller_pubkey")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let request = PairingRequest {
                pairing_id: pairing_id.clone(),
                caller_info,
                command_summary: Default::default(),
                command: Default::default(),
                caller_pubkey,
            };

            info!(
                pairing_id = %pairing_id,
                "discovered pending pairing via relay polling (SSE push may have been missed)"
            );

            let timestamped = TimestampedPairing {
                request,
                received_at: now,
            };
            self.pending_pairings
                .write()
                .insert(pairing_id, timestamped);
            added += 1;
        }

        if added > 0 {
            info!(
                added = added,
                "added pending pairings from relay poll fallback"
            );
        }
    }

    async fn reconcile_active_calls_with_relay(&self) {
        let call_ids: Vec<String> = self.active_calls.read().keys().cloned().collect();
        if call_ids.is_empty() {
            return;
        }

        for call_id in call_ids {
            let call = match self.relay_client.fetch_client_call(&call_id).await {
                Ok(call) => call,
                Err(error) => {
                    debug!(
                        error = %error,
                        call_id = %call_id,
                        "fetch_client_call failed during active call reconcile"
                    );
                    continue;
                }
            };

            let Some(status) = parse_call_status_from_relay(&call) else {
                debug!(
                    call_id = %call_id,
                    "fetch_client_call returned payload without status"
                );
                continue;
            };

            if status != CallStatus::Cancelled {
                continue;
            }

            info!(
                call_id = %call_id,
                "relay already marked active call cancelled, reconciling local state"
            );
            self.apply_cancelled_call(&call_id);
        }
    }

    pub fn list_grants_and_cleanup(&self) -> Vec<serde_json::Value> {
        let now = now_millis();
        let mut grants = self.local_grants.write();
        let mut dead_ids = Vec::new();
        let mut live = Vec::new();

        for (id, info) in grants.iter() {
            if is_grant_info_dead(info, now) {
                dead_ids.push(id.clone());
            } else if let Ok(val) = serde_json::to_value(info) {
                live.push(val);
            }
        }

        if !dead_ids.is_empty() {
            let count = dead_ids.len();
            for id in &dead_ids {
                grants.remove(id);
            }
            debug!(
                removed = count,
                remaining = grants.len(),
                "cleaned up expired/dead grants from local_grants"
            );
        }

        live
    }

    fn periodic_grant_cleanup(&self) {
        let live = self.list_grants_and_cleanup();
        debug!(
            active_grants = live.len(),
            "periodic grant cleanup check done"
        );
    }

    pub async fn delete_grant(&self, grant_id: &str) -> Result<()> {
        let relay_result = self.relay_client.delete_grant(grant_id).await;
        self.local_grants.write().remove(grant_id);
        match relay_result {
            Ok(_) => {
                info!(grant_id = %grant_id, "grant deleted from local and relay");
            }
            Err(e) => {
                warn!(grant_id = %grant_id, error = %e, "grant deleted locally but failed to delete from relay");
            }
        }
        Ok(())
    }

    async fn maybe_refresh_pair_code(&self) {
        self.cleanup_expired_pairings();

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
                        self.apply_cancelled_call(call_id);
                    }
                }
                Err(e) => warn!(error = %e, "failed to parse call_cancel"),
            },
            "grant_revoked" => match serde_json::from_str::<Value>(data) {
                Ok(v) => {
                    if let Some(grant_id) = v.get("grant_id").and_then(|g| g.as_str()) {
                        info!(grant_id = %grant_id, "grant revoked, sending ack");
                        self.local_grants.write().remove(grant_id);
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
            "ssh_connect" => match serde_json::from_str::<SshConnectEvent>(data) {
                Ok(event) => self.handle_ssh_connect(event).await,
                Err(e) => warn!(error = %e, "failed to parse ssh_connect"),
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

    fn trigger_ssh_route_refresh(&self) {
        *self.state.write() = WorkerState::Reconnecting;
        self.reconnect_notify.notify_waiters();
    }

    fn revoke_local_ssh_grants(&self, ssh_key_id: Option<&str>) {
        self.local_grants.write().retain(|_, grant| {
            if grant.auth_method != AuthMethod::SshPublickey {
                return true;
            }

            match ssh_key_id {
                Some(id) => grant.ssh_key_id.as_deref() != Some(id),
                None => false,
            }
        });
    }

    async fn handle_grant_created(&self, data: Value) {
        let grant_info = match build_grant_info_from_grant_created(
            &data,
            &self.identity.instance_id,
            now_millis(),
        ) {
            Some(grant_info) => grant_info,
            None => {
                warn!("grant_created missing grant_id");
                return;
            }
        };
        let grant_id = grant_info.grant_id.clone();
        self.local_grants
            .write()
            .insert(grant_id.clone(), grant_info);
        info!(grant_id = %grant_id, "synchronized approved grant from relay");
    }

    async fn handle_ssh_connect(&self, event: SshConnectEvent) {
        let result = self.build_ssh_connect_result(&event);
        if let Err(error) = self.relay_client.post_ssh_connect_result(&result).await {
            warn!(
                error = %error,
                connect_id = %event.connect_id,
                "failed to post ssh connect result"
            );
        }
    }

    fn build_ssh_connect_result(&self, event: &SshConnectEvent) -> SshConnectResultRequest {
        if !event.relay_verified {
            return SshConnectResultRequest {
                connect_id: event.connect_id.clone(),
                status: SshConnectResultStatus::Rejected,
                grant_id: None,
                expires_at: None,
                reason: Some("relay_not_verified".to_string()),
                caller_fingerprint: None,
                grant_mode: None,
            };
        }

        let active_key = match self.ssh_key_store.get_active_key() {
            Ok(Some(key)) => key,
            Ok(None) => {
                return SshConnectResultRequest {
                    connect_id: event.connect_id.clone(),
                    status: SshConnectResultStatus::Rejected,
                    grant_id: None,
                    expires_at: None,
                    reason: Some("ssh_key_not_found".to_string()),
                    caller_fingerprint: None,
                    grant_mode: None,
                };
            }
            Err(error) => {
                warn!(error = %error, "load active ssh key failed");
                return SshConnectResultRequest {
                    connect_id: event.connect_id.clone(),
                    status: SshConnectResultStatus::Rejected,
                    grant_id: None,
                    expires_at: None,
                    reason: Some("ssh_key_store_unavailable".to_string()),
                    caller_fingerprint: None,
                    grant_mode: None,
                };
            }
        };

        if active_key.record.device_code != event.device_code {
            return SshConnectResultRequest {
                connect_id: event.connect_id.clone(),
                status: SshConnectResultStatus::Rejected,
                grant_id: None,
                expires_at: None,
                reason: Some("ssh_key_not_found".to_string()),
                caller_fingerprint: None,
                grant_mode: None,
            };
        }

        if active_key.record.ssh_key_fingerprint != event.ssh_key_fingerprint {
            return SshConnectResultRequest {
                connect_id: event.connect_id.clone(),
                status: SshConnectResultStatus::Rejected,
                grant_id: None,
                expires_at: None,
                reason: Some("ssh_key_fingerprint_mismatch".to_string()),
                caller_fingerprint: None,
                grant_mode: None,
            };
        }

        let now = now_millis();
        let grant_id = uuid::Uuid::new_v4().to_string();
        let grant_mode = GrantMode::Permanent;
        let expires_at = None;

        let grant_info = GrantInfo {
            grant_id: grant_id.clone(),
            client_instance_id: self.identity.instance_id.clone(),
            caller_fingerprint: active_key.record.ssh_key_fingerprint.clone(),
            caller_display_name: event
                .caller_info
                .as_ref()
                .and_then(|info| info.display_name.clone().or_else(|| info.hostname.clone())),
            grant_mode,
            grant_scope: "remote_invoke".to_string(),
            auth_method: AuthMethod::SshPublickey,
            status: GrantStatus::Active,
            first_authorized_at: now,
            expires_at,
            last_used_at: Some(now),
            max_calls: None,
            remaining_calls: None,
            ssh_key_id: Some(active_key.record.id.clone()),
            ssh_key_fingerprint: Some(active_key.record.ssh_key_fingerprint.clone()),
        };
        self.local_grants
            .write()
            .insert(grant_id.clone(), grant_info);

        if let Err(error) = self
            .ssh_key_store
            .mark_used(&event.device_code, event.caller_info.as_ref())
        {
            warn!(error = %error, "update ssh key usage info failed");
        }

        SshConnectResultRequest {
            connect_id: event.connect_id.clone(),
            status: SshConnectResultStatus::Approved,
            grant_id: Some(grant_id),
            expires_at,
            reason: None,
            caller_fingerprint: Some(active_key.record.ssh_key_fingerprint),
            grant_mode: Some(grant_mode),
        }
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
                hostname: data
                    .get("hostname")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                username: data
                    .get("username")
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

        let timestamped = TimestampedPairing {
            request,
            received_at: now_millis(),
        };
        self.pending_pairings
            .write()
            .insert(pairing_id, timestamped);
    }

    async fn handle_call_open(&self, data: Value) {
        debug!(
            data = %data,
            "handle_call_open received"
        );
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

        if grant_id.is_empty() {
            warn!(
                call_id = %call_id,
                "SECURITY: call_open rejected — empty grant_id"
            );
            self.send_call_exit(
                &call_id,
                -2,
                Some("empty grant_id".to_string()),
                None,
                None,
                0,
            )
            .await;
            return;
        }

        {
            let needs_insert = !self.local_grants.read().contains_key(&grant_id);
            if needs_insert {
                let caller_fp = data
                    .get("caller_fingerprint")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let grant_info = GrantInfo {
                    grant_id: grant_id.clone(),
                    client_instance_id: self.identity.instance_id.clone(),
                    caller_fingerprint: caller_fp,
                    caller_display_name: None,
                    grant_mode: GrantMode::Permanent,
                    grant_scope: "remote_invoke".to_string(),
                    auth_method: AuthMethod::PairCode,
                    status: GrantStatus::Active,
                    first_authorized_at: now_millis(),
                    expires_at: None,
                    last_used_at: None,
                    max_calls: None,
                    remaining_calls: None,
                    ssh_key_id: None,
                    ssh_key_fingerprint: None,
                };
                self.local_grants
                    .write()
                    .insert(grant_id.clone(), grant_info);
                info!(
                    call_id = %call_id,
                    grant_id = %grant_id,
                    "auto-recovered grant from call_open (relay pre-validated)"
                );
            }
        }

        let grant_reject_reason: Option<String> = {
            let now = now_millis();
            let mut grants = self.local_grants.write();
            validate_grant_for_call(&mut grants, &grant_id, now)
        };

        if let Some(reason) = grant_reject_reason {
            warn!(
                call_id = %call_id,
                grant_id = %grant_id,
                reason = %reason,
                "SECURITY: call_open rejected — {}", reason
            );
            self.send_call_exit(&call_id, -2, Some(reason), None, None, 0)
                .await;
            return;
        }

        info!(
            call_id = %call_id,
            grant_id = %grant_id,
            "grant validated for call_open"
        );

        let command: RemoteCommand = match data.get("command") {
            Some(v) => match serde_json::from_value(v.clone()) {
                Ok(c) => c,
                Err(e) => {
                    warn!(error = %e, call_id = %call_id, "failed to parse command in call_open");
                    let stderr = format!("failed to parse command: {}", e);
                    self.send_call_exit(&call_id, -1, Some(stderr), None, None, 0)
                        .await;
                    return;
                }
            },
            None => {
                warn!(call_id = %call_id, "call_open missing command field");
                self.send_call_exit(
                    &call_id,
                    -1,
                    Some("missing command field in call_open".to_string()),
                    None,
                    None,
                    0,
                )
                .await;
                return;
            }
        };

        let command_summary_for_call: CommandSummary = data
            .get("command_summary")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_else(|| CommandSummary {
                command_preview: command.command.clone(),
                ..Default::default()
            });

        let (caller_fingerprint, caller_display_name) = {
            let grants = self.local_grants.read();
            grants
                .get(&grant_id)
                .map(|g| (g.caller_fingerprint.clone(), g.caller_display_name.clone()))
                .unwrap_or_default()
        };

        info!(
            call_id = %call_id,
            grant_id = %grant_id,
            command = %command.command,
            args_json = ?command.args_json,
            "executing remote command via call_open"
        );

        let call_started_at = now_millis();
        let active_call = Arc::new(ActiveCallControl::new(grant_id.clone(), call_started_at));
        self.active_calls
            .write()
            .insert(call_id.clone(), Arc::clone(&active_call));

        {
            let call_info = CallInfo {
                call_id: call_id.clone(),
                grant_id: grant_id.clone(),
                pairing_id: None,
                client_instance_id: self.identity.instance_id.clone(),
                caller_fingerprint,
                auth_method: self
                    .local_grants
                    .read()
                    .get(&grant_id)
                    .map(|grant| grant.auth_method)
                    .unwrap_or(AuthMethod::PairCode),
                status: CallStatus::Streaming,
                command_summary: command_summary_for_call,
                command: command.clone(),
                source_ip: None,
                caller_display_name,
                payload_digest: None,
                stdout_digest: None,
                stderr_digest: None,
                exit_code: None,
                started_at: call_started_at,
                ended_at: None,
                duration_ms: None,
                bytes_in: None,
                bytes_out: None,
                ssh_key_id: self
                    .local_grants
                    .read()
                    .get(&grant_id)
                    .and_then(|grant| grant.ssh_key_id.clone()),
                ssh_key_fingerprint: self
                    .local_grants
                    .read()
                    .get(&grant_id)
                    .and_then(|grant| grant.ssh_key_fingerprint.clone()),
            };
            let max = self.config.max_records as usize;
            let mut history = self.call_history.write();
            history.push_back(call_info);
            while history.len() > max {
                history.pop_front();
            }
        }

        let executor = Arc::clone(&self.executor);
        let relay_client = Arc::clone(&self.relay_client);
        let instance_id = self.identity.instance_id.clone();
        let active_calls = Arc::clone(&self.active_calls);
        let active_call_for_task = Arc::clone(&active_call);
        let call_history = Arc::clone(&self.call_history);
        let cid = call_id.clone();

        let task = tokio::spawn(async move {
            // Give caller-initiated cancel a short window to arrive before we begin execution.
            tokio::time::sleep(Duration::from_millis(100)).await;
            if active_call_for_task.is_cancelled() {
                info!(call_id = %cid, "skip execution because call was cancelled before start");
                active_calls.write().remove(&cid);
                return;
            }

            let start = std::time::Instant::now();
            let mut next_seq = 1u64;
            let result = executor
                .execute_with_stdout_sink(&command, |chunk| {
                    let relay_client = Arc::clone(&relay_client);
                    let cid = cid.clone();
                    let instance_id = instance_id.clone();
                    let seq = next_seq;
                    next_seq += 1;

                    async move {
                        let envelope = EncryptedEnvelope {
                            version: 1,
                            call_id: cid.clone(),
                            seq,
                            direction: super::types::FrameDirection::ClientToCaller,
                            nonce: String::new(),
                            ciphertext: chunk,
                            tag: String::new(),
                            aad: None,
                        };

                        let envelope_json = serde_json::to_string(&envelope).unwrap_or_default();
                        let frame_req = ClientCallFrameRequest {
                            call_id: cid.clone(),
                            client_instance_id: instance_id,
                            envelope_json,
                        };

                        relay_client.post_call_frame(&cid, &frame_req).await
                    }
                })
                .await;
            let duration_ms = start.elapsed().as_millis() as u64;

            match result {
                Ok(response) => {
                    if active_call_for_task.is_cancelled() {
                        info!(call_id = %cid, "skip completion update because call was cancelled");
                        active_calls.write().remove(&cid);
                        return;
                    }
                    let exit_req = ClientCallExitRequest {
                        call_id: cid.clone(),
                        client_instance_id: instance_id,
                        exit_code: response.exit_code,
                        duration_ms: Some(duration_ms),
                        stderr: response.stderr.clone(),
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
                    let _ = update_call_in_history(
                        &call_history,
                        &cid,
                        CallResult {
                            status: if response.exit_code == 0 {
                                CallStatus::Completed
                            } else {
                                CallStatus::Failed
                            },
                            exit_code: response.exit_code,
                            duration_ms,
                            bytes_out: response.stdout.as_ref().map(|s| s.len() as u64),
                            stdout_digest: response.stdout_digest.clone(),
                            stderr_digest: response.stderr_digest.clone(),
                        },
                    );
                }
                Err(e) => {
                    if active_call_for_task.is_cancelled() {
                        info!(call_id = %cid, "skip failure update because call was cancelled");
                        active_calls.write().remove(&cid);
                        return;
                    }
                    error!(error = %e, call_id = %cid, "remote command execution failed");
                    let stderr = e.to_string();

                    let exit_req = ClientCallExitRequest {
                        call_id: cid.clone(),
                        client_instance_id: instance_id,
                        exit_code: -1,
                        duration_ms: Some(duration_ms),
                        stderr: Some(stderr.clone()),
                        stdout_digest: None,
                        stderr_digest: Some(crate::remote_invoke::executor::sha1_hex(&stderr)),
                        bytes_in: Some(0),
                        bytes_out: Some(0),
                    };

                    if let Err(e2) = relay_client.post_call_exit(&cid, &exit_req).await {
                        error!(error = %e2, call_id = %cid, "failed to post error call exit");
                    }
                    let _ = update_call_in_history(
                        &call_history,
                        &cid,
                        CallResult {
                            status: CallStatus::Failed,
                            exit_code: -1,
                            duration_ms,
                            bytes_out: None,
                            stdout_digest: None,
                            stderr_digest: None,
                        },
                    );
                }
            }

            active_calls.write().remove(&cid);
        });
        *active_call.task.lock() = Some(task);
    }

    async fn send_call_exit(
        &self,
        call_id: &str,
        exit_code: i32,
        stderr: Option<String>,
        stdout_digest: Option<String>,
        stderr_digest: Option<String>,
        duration_ms: u64,
    ) {
        let req = ClientCallExitRequest {
            call_id: call_id.to_string(),
            client_instance_id: self.identity.instance_id.clone(),
            exit_code,
            duration_ms: Some(duration_ms),
            stderr,
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
        let sleep_fut = tokio::time::sleep(Duration::from_millis(delay_ms));
        tokio::pin!(sleep_fut);

        tokio::select! {
            _ = &mut sleep_fut => {}
            _ = self.reconnect_notify.notified() => {
                info!("reconnect signal received, waking up immediately");
            }
        }
    }

    fn apply_cancelled_call(&self, call_id: &str) {
        let active_call = self.active_calls.write().remove(call_id);
        let started_at = active_call
            .as_ref()
            .map(|call| call.started_at)
            .or_else(|| find_call_started_at(&self.call_history, call_id));
        if let Some(active_call) = active_call {
            active_call.mark_cancelled();
            active_call.abort_task();
            info!(
                call_id = %call_id,
                grant_id = %active_call.grant_id,
                "remote call marked cancelled"
            );
        }
        if let Some(started_at) = started_at {
            let duration_ms = now_millis().saturating_sub(started_at);
            let updated = force_cancel_call_in_history(&self.call_history, call_id, duration_ms);
            if !updated {
                debug!(call_id = %call_id, "cancel reconcile did not change local history");
            }
        } else {
            debug!(call_id = %call_id, "cancel reconcile received before local history existed");
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

struct CallResult {
    status: CallStatus,
    exit_code: i32,
    duration_ms: u64,
    bytes_out: Option<u64>,
    stdout_digest: Option<String>,
    stderr_digest: Option<String>,
}

fn update_call_in_history(
    history: &RwLock<VecDeque<CallInfo>>,
    call_id: &str,
    result: CallResult,
) -> bool {
    let mut h = history.write();
    if let Some(call) = h.iter_mut().rev().find(|c| c.call_id == call_id) {
        if !should_apply_call_result(call.status, result.status) {
            return false;
        }
        call.status = result.status;
        call.exit_code = Some(result.exit_code);
        call.duration_ms = Some(result.duration_ms);
        call.ended_at = Some(now_millis());
        call.bytes_out = result.bytes_out;
        call.stdout_digest = result.stdout_digest;
        call.stderr_digest = result.stderr_digest;
        return true;
    }
    false
}

fn find_call_started_at(history: &RwLock<VecDeque<CallInfo>>, call_id: &str) -> Option<u64> {
    history
        .read()
        .iter()
        .rev()
        .find(|c| c.call_id == call_id)
        .map(|c| c.started_at)
}

fn force_cancel_call_in_history(
    history: &RwLock<VecDeque<CallInfo>>,
    call_id: &str,
    duration_ms: u64,
) -> bool {
    let mut h = history.write();
    if let Some(call) = h.iter_mut().rev().find(|c| c.call_id == call_id) {
        call.status = CallStatus::Cancelled;
        call.exit_code = Some(130);
        call.duration_ms = Some(duration_ms);
        call.ended_at = Some(now_millis());
        return true;
    }
    false
}

fn parse_call_status_from_relay(call: &Value) -> Option<CallStatus> {
    call.get("status")
        .and_then(|status| status.as_str())
        .and_then(|status| serde_json::from_value(Value::String(status.to_string())).ok())
}

fn should_apply_call_result(current: CallStatus, next: CallStatus) -> bool {
    if current == CallStatus::Cancelled && next != CallStatus::Cancelled {
        return false;
    }

    if matches!(
        current,
        CallStatus::Completed | CallStatus::Failed | CallStatus::Timeout
    ) && next == CallStatus::Cancelled
    {
        return false;
    }

    true
}

fn is_relay_unauthorized(err: &BifrostError) -> bool {
    matches!(err, BifrostError::Network(msg) if msg.contains("unauthorized"))
}

fn is_grant_info_dead(info: &GrantInfo, now_ms: u64) -> bool {
    if matches!(
        info.status,
        GrantStatus::Expired | GrantStatus::Consumed | GrantStatus::Removed | GrantStatus::Revoked
    ) {
        return true;
    }
    if let Some(expires_at) = info.expires_at {
        if expires_at > 0 && expires_at < now_ms {
            return true;
        }
    }
    false
}

fn validate_grant_for_call(
    grants: &mut HashMap<String, GrantInfo>,
    grant_id: &str,
    now: u64,
) -> Option<String> {
    match grants.get_mut(grant_id) {
        None => Some("grant not found in local_grants".to_string()),
        Some(grant) => {
            if is_grant_info_dead(grant, now) {
                Some(format!(
                    "grant is dead (status={:?}, expires_at={:?})",
                    grant.status, grant.expires_at
                ))
            } else if grant.status != GrantStatus::Active {
                Some(format!("grant status is {:?}, not Active", grant.status))
            } else if let Some(remaining) = grant.remaining_calls {
                if remaining == 0 {
                    grant.status = GrantStatus::Consumed;
                    Some("grant has no remaining calls".to_string())
                } else {
                    grant.remaining_calls = Some(remaining - 1);
                    if remaining - 1 == 0 {
                        grant.status = GrantStatus::Consumed;
                    }
                    grant.last_used_at = Some(now);
                    None
                }
            } else {
                grant.last_used_at = Some(now);
                None
            }
        }
    }
}

fn build_grant_info_from_grant_created(
    data: &Value,
    client_instance_id: &str,
    authorized_at: u64,
) -> Option<GrantInfo> {
    let grant_id = data.get("grant_id").and_then(|v| v.as_str())?.to_string();
    let caller_fingerprint = data
        .get("caller_info")
        .and_then(|ci| ci.get("fingerprint"))
        .and_then(|v| v.as_str())
        .or_else(|| data.get("caller_fingerprint").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string();
    let caller_display_name = data
        .get("caller_info")
        .and_then(|ci| ci.get("display_name"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let grant_mode: GrantMode = data
        .get("grant_mode")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or(GrantMode::Once);
    let expires_at = data.get("expires_at").and_then(|v| v.as_u64());
    let auth_method = data
        .get("auth_method")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or(AuthMethod::PairCode);
    let ssh_key_id = data
        .get("ssh_key_id")
        .and_then(|v| v.as_str())
        .map(|value| value.to_string());
    let ssh_key_fingerprint = data
        .get("ssh_key_fingerprint")
        .and_then(|v| v.as_str())
        .map(|value| value.to_string());

    Some(GrantInfo {
        grant_id,
        client_instance_id: client_instance_id.to_string(),
        caller_fingerprint,
        caller_display_name,
        grant_mode,
        grant_scope: "remote_invoke".to_string(),
        auth_method,
        status: GrantStatus::Active,
        first_authorized_at: authorized_at,
        expires_at,
        last_used_at: None,
        max_calls: if grant_mode == GrantMode::Once {
            Some(1)
        } else {
            None
        },
        remaining_calls: if grant_mode == GrantMode::Once {
            Some(1)
        } else {
            None
        },
        ssh_key_id,
        ssh_key_fingerprint,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_active_grant(grant_id: &str, mode: GrantMode) -> GrantInfo {
        GrantInfo {
            grant_id: grant_id.to_string(),
            client_instance_id: "test-instance".to_string(),
            caller_fingerprint: "test-fp".to_string(),
            caller_display_name: None,
            grant_mode: mode,
            grant_scope: "remote_invoke".to_string(),
            auth_method: AuthMethod::PairCode,
            status: GrantStatus::Active,
            first_authorized_at: 1000,
            expires_at: None,
            last_used_at: None,
            max_calls: if mode == GrantMode::Once {
                Some(1)
            } else {
                None
            },
            remaining_calls: if mode == GrantMode::Once {
                Some(1)
            } else {
                None
            },
            ssh_key_id: None,
            ssh_key_fingerprint: None,
        }
    }

    #[test]
    fn test_validate_grant_rejects_missing_grant() {
        let mut grants = HashMap::new();
        let result = validate_grant_for_call(&mut grants, "nonexistent", 5000);
        assert!(result.is_some());
        assert!(result.unwrap().contains("not found"));
    }

    #[test]
    fn test_validate_grant_accepts_active_permanent() {
        let mut grants = HashMap::new();
        grants.insert(
            "g1".to_string(),
            make_active_grant("g1", GrantMode::Permanent),
        );
        let result = validate_grant_for_call(&mut grants, "g1", 5000);
        assert!(result.is_none());
        assert_eq!(grants["g1"].last_used_at, Some(5000));
    }

    #[test]
    fn test_validate_grant_accepts_once_and_consumes() {
        let mut grants = HashMap::new();
        grants.insert("g1".to_string(), make_active_grant("g1", GrantMode::Once));
        assert_eq!(grants["g1"].remaining_calls, Some(1));

        let result = validate_grant_for_call(&mut grants, "g1", 5000);
        assert!(result.is_none());
        assert_eq!(grants["g1"].remaining_calls, Some(0));
        assert_eq!(grants["g1"].status, GrantStatus::Consumed);
        assert_eq!(grants["g1"].last_used_at, Some(5000));
    }

    #[test]
    fn test_validate_grant_rejects_consumed_once() {
        let mut grants = HashMap::new();
        let mut grant = make_active_grant("g1", GrantMode::Once);
        grant.remaining_calls = Some(0);
        grant.status = GrantStatus::Active;
        grants.insert("g1".to_string(), grant);

        let result = validate_grant_for_call(&mut grants, "g1", 5000);
        assert!(result.is_some());
        assert!(result.unwrap().contains("no remaining calls"));
        assert_eq!(grants["g1"].status, GrantStatus::Consumed);
    }

    #[test]
    fn test_validate_grant_rejects_expired() {
        let mut grants = HashMap::new();
        let mut grant = make_active_grant("g1", GrantMode::ThirtyMinutes);
        grant.expires_at = Some(3000);
        grants.insert("g1".to_string(), grant);

        let result = validate_grant_for_call(&mut grants, "g1", 5000);
        assert!(result.is_some());
        assert!(result.unwrap().contains("dead"));
    }

    #[test]
    fn test_validate_grant_rejects_revoked() {
        let mut grants = HashMap::new();
        let mut grant = make_active_grant("g1", GrantMode::Permanent);
        grant.status = GrantStatus::Revoked;
        grants.insert("g1".to_string(), grant);

        let result = validate_grant_for_call(&mut grants, "g1", 5000);
        assert!(result.is_some());
        assert!(result.unwrap().contains("dead"));
    }

    #[test]
    fn test_validate_grant_accepts_not_yet_expired() {
        let mut grants = HashMap::new();
        let mut grant = make_active_grant("g1", GrantMode::OneHour);
        grant.expires_at = Some(10000);
        grants.insert("g1".to_string(), grant);

        let result = validate_grant_for_call(&mut grants, "g1", 5000);
        assert!(result.is_none());
    }

    #[test]
    fn test_is_grant_info_dead_expired_status() {
        let grant = GrantInfo {
            status: GrantStatus::Expired,
            ..make_active_grant("g", GrantMode::Permanent)
        };
        assert!(is_grant_info_dead(&grant, 0));
    }

    #[test]
    fn test_is_grant_info_dead_time_expired() {
        let mut grant = make_active_grant("g", GrantMode::ThirtyMinutes);
        grant.expires_at = Some(1000);
        assert!(is_grant_info_dead(&grant, 2000));
        assert!(!is_grant_info_dead(&grant, 500));
    }

    #[test]
    fn test_build_grant_info_from_grant_created_accepts_minimal_payload() {
        let payload = serde_json::json!({
            "grant_id": "grant-1",
            "caller_fingerprint": "caller-fp",
            "grant_mode": "permanent",
        });

        let grant = build_grant_info_from_grant_created(&payload, "client-a", 1234)
            .expect("grant should parse");

        assert_eq!(grant.grant_id, "grant-1");
        assert_eq!(grant.client_instance_id, "client-a");
        assert_eq!(grant.caller_fingerprint, "caller-fp");
        assert_eq!(grant.grant_mode, GrantMode::Permanent);
        assert_eq!(grant.first_authorized_at, 1234);
        assert_eq!(grant.max_calls, None);
        assert_eq!(grant.remaining_calls, None);
    }

    #[test]
    fn test_build_grant_info_from_grant_created_prefers_nested_caller_info() {
        let payload = serde_json::json!({
            "grant_id": "grant-2",
            "caller_fingerprint": "outer-fp",
            "caller_info": {
                "fingerprint": "nested-fp",
                "display_name": "caller-name"
            },
            "grant_mode": "once",
            "expires_at": 9999
        });

        let grant = build_grant_info_from_grant_created(&payload, "client-b", 5678)
            .expect("grant should parse");

        assert_eq!(grant.caller_fingerprint, "nested-fp");
        assert_eq!(grant.caller_display_name.as_deref(), Some("caller-name"));
        assert_eq!(grant.grant_mode, GrantMode::Once);
        assert_eq!(grant.expires_at, Some(9999));
        assert_eq!(grant.max_calls, Some(1));
        assert_eq!(grant.remaining_calls, Some(1));
    }

    #[test]
    fn test_build_grant_info_from_grant_created_rejects_missing_grant_id() {
        let payload = serde_json::json!({
            "caller_fingerprint": "caller-fp",
            "grant_mode": "once"
        });

        assert!(build_grant_info_from_grant_created(&payload, "client-c", 42).is_none());
    }

    fn make_call_info(call_id: &str) -> CallInfo {
        CallInfo {
            call_id: call_id.to_string(),
            grant_id: "grant-1".to_string(),
            pairing_id: None,
            client_instance_id: "test-instance".to_string(),
            caller_fingerprint: "test-fp".to_string(),
            auth_method: AuthMethod::PairCode,
            status: CallStatus::Streaming,
            command_summary: CommandSummary {
                command_preview: "status".to_string(),
                ..Default::default()
            },
            command: RemoteCommand {
                command: "status".to_string(),
                args_json: None,
            },
            source_ip: None,
            caller_display_name: Some("TestCaller".to_string()),
            payload_digest: None,
            stdout_digest: None,
            stderr_digest: None,
            exit_code: None,
            started_at: 1000,
            ended_at: None,
            duration_ms: None,
            bytes_in: None,
            bytes_out: None,
            ssh_key_id: None,
            ssh_key_fingerprint: None,
        }
    }

    #[test]
    fn test_update_call_in_history_completed() {
        let history = RwLock::new(VecDeque::new());
        history.write().push_back(make_call_info("c1"));

        let updated = update_call_in_history(
            &history,
            "c1",
            CallResult {
                status: CallStatus::Completed,
                exit_code: 0,
                duration_ms: 150,
                bytes_out: Some(1024),
                stdout_digest: Some("digest-out".to_string()),
                stderr_digest: None,
            },
        );

        assert!(updated);
        let h = history.read();
        let call = h.front().unwrap();
        assert_eq!(call.status, CallStatus::Completed);
        assert_eq!(call.exit_code, Some(0));
        assert_eq!(call.duration_ms, Some(150));
        assert_eq!(call.bytes_out, Some(1024));
        assert_eq!(call.stdout_digest, Some("digest-out".to_string()));
        assert!(call.ended_at.is_some());
    }

    #[test]
    fn test_update_call_in_history_failed() {
        let history = RwLock::new(VecDeque::new());
        history.write().push_back(make_call_info("c1"));

        let updated = update_call_in_history(
            &history,
            "c1",
            CallResult {
                status: CallStatus::Failed,
                exit_code: -1,
                duration_ms: 50,
                bytes_out: None,
                stdout_digest: None,
                stderr_digest: None,
            },
        );

        assert!(updated);
        let h = history.read();
        let call = h.front().unwrap();
        assert_eq!(call.status, CallStatus::Failed);
        assert_eq!(call.exit_code, Some(-1));
        assert_eq!(call.duration_ms, Some(50));
        assert_eq!(call.bytes_out, None);
    }

    #[test]
    fn test_update_call_in_history_not_found() {
        let history = RwLock::new(VecDeque::new());
        history.write().push_back(make_call_info("c1"));

        let updated = update_call_in_history(
            &history,
            "nonexistent",
            CallResult {
                status: CallStatus::Failed,
                exit_code: -1,
                duration_ms: 0,
                bytes_out: None,
                stdout_digest: None,
                stderr_digest: None,
            },
        );

        assert!(!updated);
        let h = history.read();
        let call = h.front().unwrap();
        assert_eq!(call.status, CallStatus::Streaming);
        assert_eq!(call.exit_code, None);
    }

    #[test]
    fn test_update_call_in_history_multiple_calls() {
        let history = RwLock::new(VecDeque::new());
        history.write().push_back(make_call_info("c1"));
        history.write().push_back(make_call_info("c2"));
        history.write().push_back(make_call_info("c3"));

        let updated = update_call_in_history(
            &history,
            "c2",
            CallResult {
                status: CallStatus::Completed,
                exit_code: 0,
                duration_ms: 200,
                bytes_out: Some(512),
                stdout_digest: None,
                stderr_digest: None,
            },
        );

        assert!(updated);
        let h = history.read();
        assert_eq!(h[0].call_id, "c1");
        assert_eq!(h[0].status, CallStatus::Streaming);
        assert_eq!(h[1].call_id, "c2");
        assert_eq!(h[1].status, CallStatus::Completed);
        assert_eq!(h[1].exit_code, Some(0));
        assert_eq!(h[2].call_id, "c3");
        assert_eq!(h[2].status, CallStatus::Streaming);
    }

    #[test]
    fn test_update_call_in_history_cancelled_is_terminal() {
        let history = RwLock::new(VecDeque::new());
        history.write().push_back(make_call_info("c1"));

        let cancelled = update_call_in_history(
            &history,
            "c1",
            CallResult {
                status: CallStatus::Cancelled,
                exit_code: 130,
                duration_ms: 12,
                bytes_out: None,
                stdout_digest: None,
                stderr_digest: None,
            },
        );
        let late_completion = update_call_in_history(
            &history,
            "c1",
            CallResult {
                status: CallStatus::Completed,
                exit_code: 0,
                duration_ms: 99,
                bytes_out: Some(10),
                stdout_digest: Some("late".to_string()),
                stderr_digest: None,
            },
        );

        assert!(cancelled);
        assert!(!late_completion);
        let h = history.read();
        let call = h.front().unwrap();
        assert_eq!(call.status, CallStatus::Cancelled);
        assert_eq!(call.exit_code, Some(130));
        assert_eq!(call.duration_ms, Some(12));
    }

    #[test]
    fn test_update_call_in_history_does_not_override_completed_with_cancelled() {
        let history = RwLock::new(VecDeque::new());
        history.write().push_back(make_call_info("c1"));

        let completed = update_call_in_history(
            &history,
            "c1",
            CallResult {
                status: CallStatus::Completed,
                exit_code: 0,
                duration_ms: 30,
                bytes_out: Some(10),
                stdout_digest: Some("digest".to_string()),
                stderr_digest: None,
            },
        );
        let cancelled = update_call_in_history(
            &history,
            "c1",
            CallResult {
                status: CallStatus::Cancelled,
                exit_code: 130,
                duration_ms: 31,
                bytes_out: None,
                stdout_digest: None,
                stderr_digest: None,
            },
        );

        assert!(completed);
        assert!(!cancelled);
        let h = history.read();
        let call = h.front().unwrap();
        assert_eq!(call.status, CallStatus::Completed);
        assert_eq!(call.exit_code, Some(0));
    }

    #[test]
    fn test_find_call_started_at_returns_latest_matching_call() {
        let history = RwLock::new(VecDeque::new());
        let mut first = make_call_info("c1");
        first.started_at = 111;
        let mut second = make_call_info("c2");
        second.started_at = 222;
        let mut latest = make_call_info("c1");
        latest.started_at = 333;

        history.write().push_back(first);
        history.write().push_back(second);
        history.write().push_back(latest);

        assert_eq!(find_call_started_at(&history, "c1"), Some(333));
        assert_eq!(find_call_started_at(&history, "missing"), None);
    }

    #[test]
    fn test_parse_call_status_from_relay_reads_cancelled() {
        let call = serde_json::json!({
            "call_id": "c1",
            "status": "cancelled",
        });

        assert_eq!(
            parse_call_status_from_relay(&call),
            Some(CallStatus::Cancelled)
        );
    }

    #[test]
    fn test_parse_call_status_from_relay_rejects_unknown_status() {
        let call = serde_json::json!({
            "call_id": "c1",
            "status": "mystery",
        });

        assert_eq!(parse_call_status_from_relay(&call), None);
    }

    #[test]
    fn test_build_grant_info_from_grant_created_uses_grant_mode_from_data() {
        let payload = serde_json::json!({
            "grant_id": "g-session",
            "caller_fingerprint": "fp-1",
            "grant_mode": "permanent",
        });
        let grant =
            build_grant_info_from_grant_created(&payload, "client-x", 9999).expect("should build");
        assert_eq!(grant.grant_id, "g-session");
        assert_eq!(grant.grant_mode, GrantMode::Permanent);
        assert!(grant.max_calls.is_none());
        assert!(grant.remaining_calls.is_none());
    }

    #[test]
    fn test_auto_recovered_grant_passes_validation() {
        let mut grants = HashMap::new();
        let auto_grant = GrantInfo {
            grant_id: "auto-g".to_string(),
            client_instance_id: "inst".to_string(),
            caller_fingerprint: "fp".to_string(),
            caller_display_name: None,
            grant_mode: GrantMode::Permanent,
            grant_scope: "remote_invoke".to_string(),
            auth_method: AuthMethod::PairCode,
            status: GrantStatus::Active,
            first_authorized_at: 1000,
            expires_at: None,
            last_used_at: None,
            max_calls: None,
            remaining_calls: None,
            ssh_key_id: None,
            ssh_key_fingerprint: None,
        };
        grants.insert("auto-g".to_string(), auto_grant);

        let result = validate_grant_for_call(&mut grants, "auto-g", 5000);
        assert!(
            result.is_none(),
            "auto-recovered grant should pass validation"
        );
        assert_eq!(grants["auto-g"].last_used_at, Some(5000));
    }
}
