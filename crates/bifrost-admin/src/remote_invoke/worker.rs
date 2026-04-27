use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use bifrost_core::{BifrostError, Result};
use bifrost_storage::RemoteShellStore;
use bifrost_sync::SyncManagerHandle;
use chrono::{DateTime, Utc};
use parking_lot::{Mutex, RwLock};
use rand::Rng;
use ring::agreement::{agree_ephemeral, EphemeralPrivateKey, UnparsedPublicKey, X25519};
use ring::digest::{digest, SHA256};
use ring::rand::SystemRandom;
use serde_json::{json, Value};
use tokio::sync::Notify;
use tracing::{debug, error, info, warn};

use super::call_history_store::{
    finalize_non_terminal_restored_calls, now_millis, sanitize_call_for_history, CallHistoryStore,
};
use super::executor::RemoteInvokeExecutor;
use super::file_policy_store::{
    full_file_ops as file_policy_full_ops, has_ssh_fingerprint_grant as file_policy_has_ssh_grant,
    rekey_ssh_fingerprint_grant as file_policy_rekey_ssh_grant,
    remove_grant_id_grant as file_policy_remove_grant_id_grant,
    upsert_ssh_fingerprint_grant as file_policy_upsert_ssh_grant,
};
use super::grant_crypto_store::{GrantCryptoStore, StoredGrantCryptoMaterial};
use super::grant_info_store::GrantInfoStore;
use super::grant_policy_store::{GrantPolicyStore, StoredGrantPolicy};
use super::identity::Identity;
use super::relay_client::RelayClient;
use super::session_ring;
use super::ssh_keys::{SshKeyMaterial, SshKeyRecord, SshKeyStore};
use super::stream_emit;
use super::types::{
    build_registration_signature_payload, decrypt_remote_command_payload, derive_call_session_key,
    derive_open_call_session_key, encrypt_encrypted_payload_without_aad, grant_mode_ttl_ms,
    scope_allows_command, AuthMethod, CallInfo, CallStatus, CallerInfo, ClientCallExitRequest,
    ClientCallFrameRequest, ClientCallStreamFrameRequest, ClientHeartbeatRequest,
    ClientRegistrationChallengeRequest, ClientRegistrationRequest, CommandKind, CommandSummary,
    DiscoverySession, EncryptedEnvelope, EncryptedPayload, EnvelopeAad, FileAccessScope,
    FrameDirection, GrantDecision, GrantDecisionRequest, GrantInfo, GrantMode, GrantScope,
    GrantStatus, PairingRequest, PublishPairCodeRequest, RemoteCommand, RemoteInvokeConfig,
    SshConnectEvent, SshConnectResultRequest, SshConnectResultStatus, UpdateGrantRequest,
    WorkerState,
};
use crate::state::SharedAdminState;

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

fn short_fingerprint(bytes: &[u8]) -> String {
    digest(&SHA256, bytes).as_ref()[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Clone)]
struct GrantCryptoMaterial {
    shared_secret: Vec<u8>,
    caller_ephemeral_pub: String,
    client_ephemeral_pub: String,
}

#[derive(Debug, Clone)]
struct ShellGrantProvision {
    grant_scope: GrantScope,
    file_access: FileAccessScope,
    policy_binding: Option<Value>,
    shell_policy_set_version_snapshot: Option<u64>,
    interactive_allowed: Option<bool>,
    stdin_allowed: Option<bool>,
}

/// User-supplied override for the file-access grant auto-seeded when an SSH
/// key is created. `None` + defaults means: `$HOME` as the single root and
/// all 12 file ops. UI / HTTP API can narrow this.
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct SshKeySeedPolicy {
    /// Absolute paths. Empty → resolver falls back to `$HOME`.
    pub roots: Vec<std::path::PathBuf>,
    /// Explicit ops. Empty → resolver falls back to [`file_policy_full_ops`].
    pub ops: Vec<bifrost_core::file_access::FileOp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_overwrite: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_recursive_delete: Option<bool>,
}

impl SshKeySeedPolicy {
    /// `roots` if non-empty, else `[$HOME]`, else `[]` (fallback tier handles it).
    pub fn resolved_roots(&self) -> Vec<std::path::PathBuf> {
        if !self.roots.is_empty() {
            return self.roots.clone();
        }
        match std::env::var_os("HOME") {
            Some(home) => vec![std::path::PathBuf::from(home)],
            None => Vec::new(),
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
    call_history_store: Arc<CallHistoryStore>,
    local_grants: Arc<RwLock<HashMap<String, GrantInfo>>>,
    grant_crypto: Arc<RwLock<HashMap<String, GrantCryptoMaterial>>>,
    grant_crypto_store: Arc<GrantCryptoStore>,
    grant_policy_store: Arc<GrantPolicyStore>,
    grant_info_store: Arc<GrantInfoStore>,
    grant_policy: Arc<RwLock<HashMap<String, StoredGrantPolicy>>>,
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
        state: SharedAdminState,
        admin_host: &str,
        admin_port: u16,
    ) -> Arc<Self> {
        let relay_client = Arc::new(RelayClient::new(
            &config.relay_url,
            &identity.instance_id,
            &identity.device_name,
            &identity.platform,
        ));
        let executor = Arc::new(RemoteInvokeExecutor::new_with_state(
            admin_host, admin_port, state,
        ));
        let data_dir = bifrost_storage::data_dir();
        let ssh_key_store = Arc::new(SshKeyStore::new(&data_dir));
        let grant_crypto_store = Arc::new(GrantCryptoStore::new(&data_dir));
        let grant_policy_store = Arc::new(GrantPolicyStore::new(&data_dir));
        let grant_info_store = Arc::new(GrantInfoStore::new(&data_dir));
        let call_history_store = Arc::new(CallHistoryStore::new(&data_dir));
        let restored_grant_crypto =
            match grant_crypto_store.load_for_relay(&relay_client.base_url()) {
                Ok(restored) => restored
                    .into_iter()
                    .map(|(grant_id, material)| {
                        (
                            grant_id,
                            GrantCryptoMaterial {
                                shared_secret: material.shared_secret,
                                caller_ephemeral_pub: material.caller_ephemeral_pub,
                                client_ephemeral_pub: material.client_ephemeral_pub,
                            },
                        )
                    })
                    .collect(),
                Err(error) => {
                    warn!(error = %error, "load persisted grant crypto failed");
                    HashMap::new()
                }
            };
        let restored_grant_policy =
            match grant_policy_store.load_for_relay(&relay_client.base_url()) {
                Ok(restored) => restored,
                Err(error) => {
                    warn!(error = %error, "load persisted grant policy failed");
                    HashMap::new()
                }
            };
        let restored_grant_info = match grant_info_store.load_for_relay(&relay_client.base_url()) {
            Ok(mut restored) => {
                // Remove grants whose crypto material is missing (e.g. crypto files were deleted).
                // These orphaned grants cannot decrypt any incoming commands and will be revoked
                // on the relay during the next SSE reconciliation.
                let before = restored.len();
                restored.retain(|grant_id, _| restored_grant_crypto.contains_key(grant_id));
                let removed = before - restored.len();
                if removed > 0 {
                    warn!(
                        removed = removed,
                        remaining = restored.len(),
                        "removed orphaned grants with missing crypto material on startup"
                    );
                    if let Err(error) = grant_info_store.retain_only(
                        &relay_client.base_url(),
                        &restored.keys().cloned().collect(),
                    ) {
                        warn!(error = %error, "failed to persist orphaned grant cleanup");
                    }
                }
                if !restored.is_empty() {
                    info!(
                        count = restored.len(),
                        "restored persisted grant info from disk"
                    );
                }
                restored
            }
            Err(error) => {
                warn!(error = %error, "load persisted grant info failed");
                HashMap::new()
            }
        };
        let mut restored_call_history = match call_history_store.load_for_client(
            &relay_client.base_url(),
            &identity.instance_id,
            config.max_records as usize,
            config.retention_days,
        ) {
            Ok(restored) => restored,
            Err(error) => {
                warn!(error = %error, "load persisted call history failed");
                VecDeque::new()
            }
        };
        if finalize_non_terminal_restored_calls(&mut restored_call_history, now_millis()) > 0 {
            for call in &restored_call_history {
                if let Err(error) = call_history_store.upsert(
                    &relay_client.base_url(),
                    &identity.instance_id,
                    call,
                    config.max_records as usize,
                    config.retention_days,
                ) {
                    warn!(error = %error, call_id = %call.call_id, "persist finalized restored call failed");
                }
            }
        }

        Arc::new(Self {
            config,
            identity,
            sync_manager,
            relay_client,
            executor,
            state: Arc::new(RwLock::new(WorkerState::Disconnected)),
            pending_pairings: Arc::new(RwLock::new(HashMap::new())),
            active_calls: Arc::new(RwLock::new(HashMap::new())),
            call_history: Arc::new(RwLock::new(restored_call_history)),
            call_history_store,
            local_grants: Arc::new(RwLock::new(restored_grant_info)),
            grant_crypto: Arc::new(RwLock::new(restored_grant_crypto)),
            grant_crypto_store,
            grant_policy_store,
            grant_info_store,
            grant_policy: Arc::new(RwLock::new(restored_grant_policy)),
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
        self.cleanup_expired_pairings();
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

    pub fn clear_calls(&self) -> usize {
        let removed = {
            let mut history = self.call_history.write();
            let removed = history.len();
            history.clear();
            removed
        };
        if let Err(error) = self
            .call_history_store
            .clear_for_client(&self.relay_client.base_url(), &self.identity.instance_id)
        {
            warn!(error = %error, "clear persisted remote invoke call history failed");
        }
        removed
    }

    pub fn relay_client(&self) -> &Arc<RelayClient> {
        &self.relay_client
    }

    pub fn executor(&self) -> &Arc<RemoteInvokeExecutor> {
        &self.executor
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
        let restored_grant_crypto = match self.grant_crypto_store.load_for_relay(new_normalized) {
            Ok(restored) => restored
                .into_iter()
                .map(|(grant_id, material)| {
                    (
                        grant_id,
                        GrantCryptoMaterial {
                            shared_secret: material.shared_secret,
                            caller_ephemeral_pub: material.caller_ephemeral_pub,
                            client_ephemeral_pub: material.client_ephemeral_pub,
                        },
                    )
                })
                .collect(),
            Err(error) => {
                warn!(error = %error, relay_url = %new_normalized, "reload persisted grant crypto after relay switch failed");
                HashMap::new()
            }
        };
        *self.grant_crypto.write() = restored_grant_crypto.clone();
        let restored_grant_policy = match self.grant_policy_store.load_for_relay(new_normalized) {
            Ok(restored) => restored,
            Err(error) => {
                warn!(error = %error, relay_url = %new_normalized, "reload persisted grant policy after relay switch failed");
                HashMap::new()
            }
        };
        *self.grant_policy.write() = restored_grant_policy;
        let restored_grant_info = match self.grant_info_store.load_for_relay(new_normalized) {
            Ok(mut restored) => {
                let before = restored.len();
                restored.retain(|grant_id, _| restored_grant_crypto.contains_key(grant_id));
                let removed = before - restored.len();
                if removed > 0 {
                    warn!(
                        removed = removed,
                        remaining = restored.len(),
                        "removed orphaned grants with missing crypto material on relay switch"
                    );
                    if let Err(error) = self
                        .grant_info_store
                        .retain_only(new_normalized, &restored.keys().cloned().collect())
                    {
                        warn!(error = %error, "failed to persist orphaned grant cleanup on relay switch");
                    }
                }
                restored
            }
            Err(error) => {
                warn!(error = %error, relay_url = %new_normalized, "reload persisted grant info after relay switch failed");
                HashMap::new()
            }
        };
        *self.local_grants.write() = restored_grant_info;
        let mut restored_call_history = match self.call_history_store.load_for_client(
            new_normalized,
            &self.identity.instance_id,
            self.config.max_records as usize,
            self.config.retention_days,
        ) {
            Ok(restored) => restored,
            Err(error) => {
                warn!(error = %error, relay_url = %new_normalized, "reload persisted call history after relay switch failed");
                VecDeque::new()
            }
        };
        if finalize_non_terminal_restored_calls(&mut restored_call_history, now_millis()) > 0 {
            for call in &restored_call_history {
                if let Err(error) = self.call_history_store.upsert(
                    new_normalized,
                    &self.identity.instance_id,
                    call,
                    self.config.max_records as usize,
                    self.config.retention_days,
                ) {
                    warn!(error = %error, call_id = %call.call_id, "persist finalized restored call after relay switch failed");
                }
            }
        }
        *self.call_history.write() = restored_call_history;
        self.reconnect_notify.notify_waiters();
    }

    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    pub fn get_active_ssh_key(&self) -> Result<Option<SshKeyRecord>> {
        Ok(self.ssh_key_store.get_active_key()?.map(|key| key.record))
    }

    pub fn ensure_active_ssh_file_access_policy(&self) -> Result<Option<SshKeyRecord>> {
        let Some(record) = self.get_active_ssh_key()? else {
            return Ok(None);
        };
        if !file_policy_has_ssh_grant(&record.ssh_key_fingerprint) {
            self.seed_ssh_file_access_grant(&record, None);
        }
        Ok(Some(record))
    }

    pub fn export_active_ssh_key(&self) -> Result<Option<SshKeyMaterial>> {
        self.ssh_key_store.export_active_key_material()
    }

    pub fn create_ssh_key(
        &self,
        label: String,
        grant_mode: GrantMode,
        seed_policy: Option<SshKeySeedPolicy>,
    ) -> Result<SshKeyMaterial> {
        self.revoke_local_ssh_grants(None);
        let result = self
            .ssh_key_store
            .create_or_replace_key(label, grant_mode)?;
        self.seed_ssh_file_access_grant(&result.record, seed_policy);
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
        // Reuse the operator's previous SSH file policy across rotation.
        // If the old key never had an explicit policy, seed the default.
        let moved = file_policy_rekey_ssh_grant(
            &active_key.ssh_key_fingerprint,
            &reset.record.ssh_key_fingerprint,
            Some(format!("ssh-key:{}", reset.record.label)),
        )
        .unwrap_or_else(|err| {
            warn!(
                error = %err,
                "failed to migrate SSH key file-access policy during reset"
            );
            false
        });
        if !moved {
            self.seed_ssh_file_access_grant(&reset.record, None);
        }
        self.trigger_ssh_route_refresh();
        Ok(Some(reset))
    }

    /// Best-effort: write a `[[grant]]` entry into `file-access.toml` keyed
    /// by the SSH fingerprint so the SSH-key flow is "full permissions by
    /// default" (matches the absence of a pair-code scope dialog). Failure
    /// is logged but non-fatal: the key is still created and can still be
    /// used via the hardcoded read-only cwd fallback.
    fn seed_ssh_file_access_grant(&self, record: &SshKeyRecord, policy: Option<SshKeySeedPolicy>) {
        let policy = policy.unwrap_or_default();
        let roots = policy.resolved_roots();
        let ops = if policy.ops.is_empty() {
            file_policy_full_ops()
        } else {
            policy.ops
        };
        let name = Some(format!("ssh-key:{}", record.label));
        if let Err(err) = file_policy_upsert_ssh_grant(
            &record.ssh_key_fingerprint,
            name,
            roots,
            ops,
            policy.allow_overwrite,
            policy.allow_recursive_delete,
        ) {
            warn!(
                fingerprint = %record.ssh_key_fingerprint,
                error = %err,
                "failed to seed SSH key file-access grant; falling back to manual config"
            );
        } else {
            info!(
                fingerprint = %record.ssh_key_fingerprint,
                "seeded SSH key file-access grant"
            );
        }
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

    #[allow(clippy::too_many_arguments)]
    pub async fn approve_pairing(
        &self,
        pairing_id: &str,
        grant_mode: GrantMode,
        requested_grant_scope: Option<GrantScope>,
        requested_file_access: Option<FileAccessScope>,
        requested_policy_binding: Option<Value>,
        requested_interactive_allowed: Option<bool>,
        requested_stdin_allowed: Option<bool>,
    ) -> Result<Value> {
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

        let caller_ephemeral_pub = {
            self.pending_pairings
                .read()
                .get(pairing_id)
                .and_then(|tp| tp.request.caller_ephemeral_pub.clone())
        }
        .ok_or_else(|| {
            BifrostError::Config(
                "pairing request is missing caller_ephemeral_pub required for encrypted remote commands"
                    .to_string(),
            )
        })?;
        let crypto_material = build_grant_crypto_material(&caller_ephemeral_pub)?;
        let shell_grant = shell_grant_provision(
            requested_grant_scope,
            requested_file_access,
            requested_policy_binding,
            requested_interactive_allowed,
            requested_stdin_allowed,
        )
        .unwrap_or_else(|error| {
            warn!(error = %error, "load remote shell grant defaults failed, fallback to remote_query");
            let mut provision = default_query_grant_provision();
            provision.file_access = requested_file_access.unwrap_or_default();
            provision
        });

        let req = GrantDecisionRequest {
            pairing_id: pairing_id.to_string(),
            client_instance_id: self.identity.instance_id.clone(),
            decision: GrantDecision::Approve,
            grant_mode: Some(grant_mode),
            grant_scope: Some(shell_grant.grant_scope),
            file_access: Some(shell_grant.file_access),
            client_ephemeral_pub: Some(crypto_material.client_ephemeral_pub.clone()),
        };

        let result = match self
            .relay_client
            .submit_grant_decision(pairing_id, &req)
            .await
        {
            Ok(result) => result,
            Err(error) if is_relay_stale_pairing_error(&error) => {
                self.pending_pairings.write().remove(pairing_id);
                return Err(pairing_not_found_or_expired_error(pairing_id));
            }
            Err(error) => return Err(error),
        };

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
            self.grant_crypto
                .write()
                .insert(grant_id.clone(), crypto_material.clone());
            self.persist_grant_crypto(&grant_id, &crypto_material);
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
                grant_scope: shell_grant.grant_scope,
                file_access: shell_grant.file_access,
                auth_method: AuthMethod::PairCode,
                status: GrantStatus::Active,
                first_authorized_at: now,
                last_command_at: None,
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
                caller_ephemeral_pub: None,
                client_ephemeral_pub: None,
                policy_binding: shell_grant.policy_binding.clone(),
                shell_policy_set_version_snapshot: shell_grant.shell_policy_set_version_snapshot,
                interactive_allowed: shell_grant.interactive_allowed,
                stdin_allowed: shell_grant.stdin_allowed,
            };
            self.local_grants
                .write()
                .insert(grant_id.clone(), grant_info.clone());
            self.persist_grant_info(&grant_id, &grant_info);
            self.persist_grant_policy(
                &grant_id,
                &StoredGrantPolicy {
                    grant_scope: shell_grant.grant_scope,
                    file_access: shell_grant.file_access,
                    policy_binding: shell_grant.policy_binding.clone(),
                    shell_policy_set_version_snapshot: shell_grant
                        .shell_policy_set_version_snapshot,
                    interactive_allowed: shell_grant.interactive_allowed,
                    stdin_allowed: shell_grant.stdin_allowed,
                },
            );
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
            grant_scope: None,
            file_access: None,
            client_ephemeral_pub: None,
        };

        let result = match self
            .relay_client
            .submit_grant_decision(pairing_id, &req)
            .await
        {
            Ok(result) => result,
            Err(error) if is_relay_stale_pairing_error(&error) => {
                self.pending_pairings.write().remove(pairing_id);
                return Err(pairing_not_found_or_expired_error(pairing_id));
            }
            Err(error) => return Err(error),
        };
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
            ssh_device_route: self.ssh_key_store.active_route().ok(),
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
                let synced_transport = self.grant_crypto.read().clone();
                let synced_policy = self.grant_policy.read().clone();
                let mut stale_grant_ids = Vec::new();
                let mut relay_active_ids = HashSet::new();
                for item in &grants_data {
                    if let Some(gi) =
                        build_grant_info_from_grant_created(item, &self.identity.instance_id, now)
                    {
                        let gid = gi.grant_id.clone();
                        relay_active_ids.insert(gid.clone());
                        let gi = apply_stored_grant_policy(gi, synced_policy.get(&gid));
                        if !has_usable_grant_crypto(&synced_transport, &gi) {
                            warn!(
                                grant_id = %gid,
                                "active relay grant is missing usable encrypted transport context locally; deleting stale grant"
                            );
                            stale_grant_ids.push(gid);
                            continue;
                        }
                        self.persist_grant_info(&gid, &gi);
                        self.local_grants.write().insert(gid, gi);
                        count += 1;
                    }
                }

                let local_orphans: Vec<String> = {
                    let grants = self.local_grants.read();
                    grants
                        .keys()
                        .filter(|id| !relay_active_ids.contains(id.as_str()))
                        .cloned()
                        .collect()
                };
                if !local_orphans.is_empty() {
                    info!(
                        count = local_orphans.len(),
                        "purging local grants not present in relay active set (SSE reconciliation)"
                    );
                    for orphan_id in &local_orphans {
                        self.local_grants.write().remove(orphan_id);
                        self.remove_grant_crypto(orphan_id);
                        self.remove_grant_policy(orphan_id);
                        self.remove_grant_info(orphan_id);
                    }
                }

                if let Err(error) = self
                    .grant_info_store
                    .retain_only(&self.relay_client.base_url(), &relay_active_ids)
                {
                    warn!(error = %error, "retain_only grant info store failed during SSE reconciliation");
                }

                if count > 0 {
                    info!(
                        count = count,
                        "synced active grants from relay on SSE connect"
                    );
                }
                for grant_id in stale_grant_ids {
                    // Re-check after brief wait: approve_pairing may be storing crypto concurrently
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    if self.grant_crypto.read().contains_key(&grant_id) {
                        info!(grant_id = %grant_id, "grant crypto arrived during stale check; keeping grant");
                        continue;
                    }
                    self.local_grants.write().remove(&grant_id);
                    self.remove_grant_crypto(&grant_id);
                    self.remove_grant_policy(&grant_id);
                    self.remove_grant_info(&grant_id);
                    match self.relay_client.delete_grant(&grant_id).await {
                        Ok(_) => info!(
                            grant_id = %grant_id,
                            "deleted stale relay grant without local encrypted transport context"
                        ),
                        Err(error) => warn!(
                            error = %error,
                            grant_id = %grant_id,
                            "failed to delete stale relay grant without local encrypted transport context"
                        ),
                    }
                }
            }
            Err(e) => {
                debug!(error = %e, "fetch_active_grants on SSE connect (non-fatal, relay may not support this endpoint)");
            }
        }

        *self.state.write() = WorkerState::Connected;
        info!(stream_id = %stream_id, "SSE stream connected");

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
        // This is intentionally three-state: omitted means the local key store
        // could not be read, null means no active SSH key and the relay should
        // clear the route, and an object publishes the active key route.
        let ssh_device_route = match self.ssh_key_store.active_route() {
            Ok(route) => Some(route),
            Err(e) => {
                warn!(error = %e, "failed to read ssh active_route for heartbeat, omitting field");
                None
            }
        };
        let req = ClientHeartbeatRequest {
            client_instance_id: self.identity.instance_id.clone(),
            stream_id: stream_id.to_string(),
            active_call_ids: active_ids,
            ssh_device_route,
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
            let alive = pairing_request_is_alive(tp, now, ttl_ms);
            if !alive {
                info!(
                    pairing_id = %id,
                    age_secs = (now - tp.received_at) / 1000,
                    expires_at = tp.request.expires_at,
                    "removing expired pairing request"
                );
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
            let caller_ephemeral_pub = p
                .get("caller_ephemeral_pub")
                .and_then(|v| v.as_str())
                .map(|value| value.to_string());
            let client_ephemeral_pub = p
                .get("client_ephemeral_pub")
                .and_then(|v| v.as_str())
                .map(|value| value.to_string());

            let request = PairingRequest {
                pairing_id: pairing_id.clone(),
                caller_info,
                command_summary: Default::default(),
                command: Default::default(),
                caller_pubkey,
                expires_at: p.get("expires_at").and_then(parse_relay_timestamp_millis),
                client_ephemeral_pub,
                caller_ephemeral_pub,
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
            } else if let Ok(mut val) = serde_json::to_value(info) {
                if let Some(obj) = val.as_object_mut() {
                    obj.insert(
                        "first_connected_at".to_string(),
                        json!(info.first_authorized_at),
                    );
                    obj.insert("created_at".to_string(), json!(info.first_authorized_at));
                }
                live.push(val);
            }
        }

        if !dead_ids.is_empty() {
            let count = dead_ids.len();
            for id in &dead_ids {
                grants.remove(id);
            }
            drop(grants);
            for id in &dead_ids {
                self.remove_grant_crypto(id);
                self.remove_grant_policy(id);
                self.remove_grant_info(id);
            }
            debug!(
                removed = count,
                remaining = self.local_grants.read().len(),
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
        self.remove_grant_crypto(grant_id);
        self.remove_grant_policy(grant_id);
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

    pub async fn update_grant(
        &self,
        grant_id: &str,
        requested_grant_scope: Option<GrantScope>,
        requested_file_access: Option<FileAccessScope>,
        requested_policy_binding: Option<Value>,
        requested_interactive_allowed: Option<bool>,
        requested_stdin_allowed: Option<bool>,
    ) -> Result<Value> {
        let existing = self
            .local_grants
            .read()
            .get(grant_id)
            .cloned()
            .ok_or_else(|| BifrostError::NotFound(format!("grant '{}' not found", grant_id)))?;

        let updated_shell_grant = updated_shell_grant_provision(
            &existing,
            requested_grant_scope,
            requested_file_access,
            requested_policy_binding,
            requested_interactive_allowed,
            requested_stdin_allowed,
        )?;

        let req = UpdateGrantRequest {
            client_instance_id: self.identity.instance_id.clone(),
            grant_scope: Some(updated_shell_grant.grant_scope),
            file_access: Some(updated_shell_grant.file_access),
        };

        let result = self.relay_client.update_grant(grant_id, &req).await?;
        let mut updated_info = build_grant_info_from_grant_created(
            &result,
            &existing.client_instance_id,
            existing.first_authorized_at,
        )
        .unwrap_or_else(|| GrantInfo {
            grant_id: existing.grant_id.clone(),
            client_instance_id: existing.client_instance_id.clone(),
            caller_fingerprint: existing.caller_fingerprint.clone(),
            caller_display_name: existing.caller_display_name.clone(),
            grant_mode: existing.grant_mode,
            grant_scope: updated_shell_grant.grant_scope,
            file_access: updated_shell_grant.file_access,
            auth_method: existing.auth_method,
            status: existing.status,
            first_authorized_at: existing.first_authorized_at,
            last_command_at: existing.last_command_at,
            expires_at: existing.expires_at,
            last_used_at: existing.last_used_at,
            max_calls: existing.max_calls,
            remaining_calls: existing.remaining_calls,
            ssh_key_id: existing.ssh_key_id.clone(),
            ssh_key_fingerprint: existing.ssh_key_fingerprint.clone(),
            caller_ephemeral_pub: existing.caller_ephemeral_pub.clone(),
            client_ephemeral_pub: existing.client_ephemeral_pub.clone(),
            policy_binding: updated_shell_grant.policy_binding.clone(),
            shell_policy_set_version_snapshot: updated_shell_grant
                .shell_policy_set_version_snapshot,
            interactive_allowed: updated_shell_grant.interactive_allowed,
            stdin_allowed: updated_shell_grant.stdin_allowed,
        });
        updated_info.policy_binding = updated_shell_grant.policy_binding.clone();
        updated_info.file_access = updated_shell_grant.file_access;
        updated_info.shell_policy_set_version_snapshot =
            updated_shell_grant.shell_policy_set_version_snapshot;
        updated_info.interactive_allowed = updated_shell_grant.interactive_allowed;
        updated_info.stdin_allowed = updated_shell_grant.stdin_allowed;

        self.local_grants
            .write()
            .insert(grant_id.to_string(), updated_info.clone());
        self.persist_grant_policy(
            grant_id,
            &StoredGrantPolicy {
                grant_scope: updated_shell_grant.grant_scope,
                file_access: updated_shell_grant.file_access,
                policy_binding: updated_shell_grant.policy_binding.clone(),
                shell_policy_set_version_snapshot: updated_shell_grant
                    .shell_policy_set_version_snapshot,
                interactive_allowed: updated_shell_grant.interactive_allowed,
                stdin_allowed: updated_shell_grant.stdin_allowed,
            },
        );
        info!(grant_id = %grant_id, "grant updated locally and on relay");
        serde_json::to_value(updated_info).map_err(|error| {
            BifrostError::Config(format!("serialize grant update result: {error}"))
        })
    }

    fn persist_grant_crypto(&self, grant_id: &str, material: &GrantCryptoMaterial) {
        let stored = StoredGrantCryptoMaterial {
            shared_secret: material.shared_secret.clone(),
            caller_ephemeral_pub: material.caller_ephemeral_pub.clone(),
            client_ephemeral_pub: material.client_ephemeral_pub.clone(),
        };
        if let Err(error) =
            self.grant_crypto_store
                .upsert(&self.relay_client.base_url(), grant_id, &stored)
        {
            warn!(error = %error, grant_id = %grant_id, "persist grant crypto failed");
        }
    }

    fn persist_grant_policy(&self, grant_id: &str, policy: &StoredGrantPolicy) {
        self.grant_policy
            .write()
            .insert(grant_id.to_string(), policy.clone());
        if let Err(error) =
            self.grant_policy_store
                .upsert(&self.relay_client.base_url(), grant_id, policy)
        {
            warn!(error = %error, grant_id = %grant_id, "persist grant policy failed");
        }
    }

    fn remove_grant_crypto(&self, grant_id: &str) {
        self.grant_crypto.write().remove(grant_id);
        if let Err(error) = self.grant_crypto_store.remove(grant_id) {
            warn!(error = %error, grant_id = %grant_id, "remove persisted grant crypto failed");
        }
    }

    fn remove_grant_policy(&self, grant_id: &str) {
        self.grant_policy.write().remove(grant_id);
        if let Err(error) = self.grant_policy_store.remove(grant_id) {
            warn!(error = %error, grant_id = %grant_id, "remove persisted grant policy failed");
        }
        match file_policy_remove_grant_id_grant(grant_id) {
            Ok(true) => {
                info!(grant_id = %grant_id, "removed per-grant file access policy");
            }
            Ok(false) => {}
            Err(error) => {
                warn!(error = %error, grant_id = %grant_id, "remove per-grant file access policy failed");
            }
        }
    }

    fn persist_grant_info(&self, grant_id: &str, info: &GrantInfo) {
        if let Err(error) =
            self.grant_info_store
                .upsert(&self.relay_client.base_url(), grant_id, info)
        {
            warn!(error = %error, grant_id = %grant_id, "persist grant info failed");
        }
    }

    fn remove_grant_info(&self, grant_id: &str) {
        if let Err(error) = self.grant_info_store.remove(grant_id) {
            warn!(error = %error, grant_id = %grant_id, "remove persisted grant info failed");
        }
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
                        self.remove_grant_crypto(grant_id);
                        self.remove_grant_policy(grant_id);
                        self.remove_grant_info(grant_id);
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
        let mut revoked_grant_ids = Vec::new();
        self.local_grants.write().retain(|grant_id, grant| {
            if grant.auth_method != AuthMethod::SshPublickey {
                return true;
            }

            let keep = match ssh_key_id {
                Some(id) => grant.ssh_key_id.as_deref() != Some(id),
                None => false,
            };
            if !keep {
                revoked_grant_ids.push(grant_id.clone());
            }
            keep
        });
        for grant_id in revoked_grant_ids {
            self.remove_grant_crypto(&grant_id);
            self.remove_grant_policy(&grant_id);
            self.remove_grant_info(&grant_id);
        }
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
        let grant_info = {
            let stored = self.grant_policy.read();
            apply_stored_grant_policy(grant_info, stored.get(&grant_id))
        };
        if !has_usable_grant_crypto(&self.grant_crypto.read(), &grant_info) {
            // Race condition: The relay sends the SSE grant_created event before the HTTP
            // response to submit_grant_decision. If approve_pairing hasn't stored the crypto
            // yet, we might mistakenly consider this grant stale. Wait briefly and retry.
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            if has_usable_grant_crypto(&self.grant_crypto.read(), &grant_info) {
                info!(grant_id = %grant_id, "grant crypto arrived after brief wait; accepting grant");
                self.persist_grant_info(&grant_id, &grant_info);
                self.local_grants
                    .write()
                    .insert(grant_id.clone(), grant_info);
                return;
            }
            warn!(
                grant_id = %grant_id,
                "grant_created is missing usable encrypted transport context locally; deleting stale grant"
            );
            self.local_grants.write().remove(&grant_id);
            self.remove_grant_crypto(&grant_id);
            self.remove_grant_policy(&grant_id);
            self.remove_grant_info(&grant_id);
            match self.relay_client.delete_grant(&grant_id).await {
                Ok(_) => info!(
                    grant_id = %grant_id,
                    "deleted stale grant from relay after grant_created without local encrypted transport context"
                ),
                Err(error) => warn!(
                    error = %error,
                    grant_id = %grant_id,
                    "failed to delete stale grant from relay after grant_created without local encrypted transport context"
                ),
            }
            return;
        }
        self.persist_grant_info(&grant_id, &grant_info);
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
                ssh_key_fingerprint: None,
                grant_mode: None,
                grant_scope: None,
                file_access: None,
                caller_ephemeral_pub: None,
                client_ephemeral_pub: None,
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
                    ssh_key_fingerprint: None,
                    grant_mode: None,
                    grant_scope: None,
                    file_access: None,
                    caller_ephemeral_pub: None,
                    client_ephemeral_pub: None,
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
                    ssh_key_fingerprint: None,
                    grant_mode: None,
                    grant_scope: None,
                    file_access: None,
                    caller_ephemeral_pub: None,
                    client_ephemeral_pub: None,
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
                ssh_key_fingerprint: None,
                grant_mode: None,
                grant_scope: None,
                file_access: None,
                caller_ephemeral_pub: None,
                client_ephemeral_pub: None,
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
                ssh_key_fingerprint: None,
                grant_mode: None,
                grant_scope: None,
                file_access: None,
                caller_ephemeral_pub: None,
                client_ephemeral_pub: None,
            };
        }

        let now = now_millis();
        let crypto_material = match event.caller_ephemeral_pub.as_deref() {
            Some(caller_ephemeral_pub) => match build_grant_crypto_material(caller_ephemeral_pub) {
                Ok(material) => material,
                Err(error) => {
                    warn!(error = %error, "build ssh connect encrypted transport failed");
                    return SshConnectResultRequest {
                        connect_id: event.connect_id.clone(),
                        status: SshConnectResultStatus::Rejected,
                        grant_id: None,
                        expires_at: None,
                        reason: Some("invalid_caller_ephemeral_pub".to_string()),
                        caller_fingerprint: None,
                        ssh_key_fingerprint: None,
                        grant_mode: None,
                        grant_scope: None,
                        file_access: None,
                        caller_ephemeral_pub: None,
                        client_ephemeral_pub: None,
                    };
                }
            },
            None => {
                return SshConnectResultRequest {
                    connect_id: event.connect_id.clone(),
                    status: SshConnectResultStatus::Rejected,
                    grant_id: None,
                    expires_at: None,
                    reason: Some(
                        "caller_ephemeral_pub is required for encrypted ssh remote commands"
                            .to_string(),
                    ),
                    caller_fingerprint: None,
                    ssh_key_fingerprint: None,
                    grant_mode: None,
                    grant_scope: None,
                    file_access: None,
                    caller_ephemeral_pub: None,
                    client_ephemeral_pub: None,
                };
            }
        };
        let grant_id = uuid::Uuid::new_v4().to_string();
        let grant_mode = GrantMode::Permanent;
        let expires_at = None;
        let caller_fingerprint = event
            .caller_info
            .as_ref()
            .map(|info| info.fingerprint.trim())
            .filter(|fingerprint| !fingerprint.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| active_key.record.ssh_key_fingerprint.clone());
        let shell_grant = shell_grant_provision(None, None, None, None, None).unwrap_or_else(|error| {
            warn!(error = %error, "load remote shell grant defaults failed for ssh connect, fallback to remote_query");
            default_query_grant_provision()
        });

        let grant_info = GrantInfo {
            grant_id: grant_id.clone(),
            client_instance_id: self.identity.instance_id.clone(),
            caller_fingerprint: caller_fingerprint.clone(),
            caller_display_name: event
                .caller_info
                .as_ref()
                .and_then(|info| info.display_name.clone().or_else(|| info.hostname.clone())),
            grant_mode,
            grant_scope: shell_grant.grant_scope,
            file_access: shell_grant.file_access,
            auth_method: AuthMethod::SshPublickey,
            status: GrantStatus::Active,
            first_authorized_at: now,
            last_command_at: None,
            expires_at,
            last_used_at: Some(now),
            max_calls: None,
            remaining_calls: None,
            ssh_key_id: Some(active_key.record.id.clone()),
            ssh_key_fingerprint: Some(active_key.record.ssh_key_fingerprint.clone()),
            caller_ephemeral_pub: Some(crypto_material.caller_ephemeral_pub.clone()),
            client_ephemeral_pub: Some(crypto_material.client_ephemeral_pub.clone()),
            policy_binding: shell_grant.policy_binding.clone(),
            shell_policy_set_version_snapshot: shell_grant.shell_policy_set_version_snapshot,
            interactive_allowed: shell_grant.interactive_allowed,
            stdin_allowed: shell_grant.stdin_allowed,
        };
        self.local_grants
            .write()
            .insert(grant_id.clone(), grant_info);
        self.persist_grant_policy(
            &grant_id,
            &StoredGrantPolicy {
                grant_scope: shell_grant.grant_scope,
                file_access: shell_grant.file_access,
                policy_binding: shell_grant.policy_binding.clone(),
                shell_policy_set_version_snapshot: shell_grant.shell_policy_set_version_snapshot,
                interactive_allowed: shell_grant.interactive_allowed,
                stdin_allowed: shell_grant.stdin_allowed,
            },
        );
        self.grant_crypto
            .write()
            .insert(grant_id.clone(), crypto_material.clone());
        self.persist_grant_crypto(&grant_id, &crypto_material);

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
            caller_fingerprint: Some(caller_fingerprint),
            ssh_key_fingerprint: Some(active_key.record.ssh_key_fingerprint),
            grant_mode: Some(grant_mode),
            grant_scope: Some(shell_grant.grant_scope),
            file_access: Some(shell_grant.file_access),
            caller_ephemeral_pub: Some(crypto_material.caller_ephemeral_pub),
            client_ephemeral_pub: Some(crypto_material.client_ephemeral_pub),
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
        let caller_ephemeral_pub = data
            .get("caller_ephemeral_pub")
            .and_then(|v| v.as_str())
            .map(|value| value.to_string());
        let client_ephemeral_pub = data
            .get("client_ephemeral_pub")
            .and_then(|v| v.as_str())
            .map(|value| value.to_string());

        let command_summary = serde_json::from_value(command_summary_val).unwrap_or_default();
        let command = serde_json::from_value(command_val).unwrap_or_default();

        let request = PairingRequest {
            pairing_id: pairing_id.clone(),
            caller_info,
            command_summary,
            command,
            caller_pubkey,
            expires_at: data
                .get("expires_at")
                .and_then(parse_relay_timestamp_millis),
            client_ephemeral_pub,
            caller_ephemeral_pub,
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

        let command_kind = data
            .get("command_kind")
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default();

        {
            let needs_insert = !self.local_grants.read().contains_key(&grant_id);
            if needs_insert {
                let active_ssh_key = self.get_active_ssh_key().ok().flatten();
                let grant_info = recover_grant_info_from_call_open(
                    &data,
                    grant_id.clone(),
                    &self.identity.instance_id,
                    command_kind,
                    active_ssh_key.as_ref(),
                );
                if grant_info.ssh_key_fingerprint.is_some() {
                    if let Err(error) = self.ensure_active_ssh_file_access_policy() {
                        warn!(
                            error = %error,
                            grant_id = %grant_id,
                            "auto-recovered SSH grant but failed to restore default file policy"
                        );
                    }
                }
                self.persist_grant_info(&grant_id, &grant_info);
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
            let result = validate_grant_for_call(&mut grants, &grant_id, now);
            if result.is_none() {
                if let Some(grant) = grants.get(&grant_id) {
                    self.persist_grant_info(&grant_id, grant);
                }
            }
            result
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

        let (grant_scope, file_access, caller_fp, ssh_fp) = self
            .local_grants
            .read()
            .get(&grant_id)
            .map(|grant| {
                (
                    grant.grant_scope,
                    grant.file_access,
                    Some(grant.caller_fingerprint.clone()),
                    grant.ssh_key_fingerprint.clone(),
                )
            })
            .unwrap_or_default();

        if data.get("command").is_some() {
            warn!(
                call_id = %call_id,
                grant_id = %grant_id,
                "legacy plaintext command payload is no longer accepted"
            );
            self.send_call_exit(
                &call_id,
                -2,
                Some("plaintext command payload is no longer supported".to_string()),
                None,
                None,
                0,
            )
            .await;
            return;
        }

        if data.get("command_encrypted").is_none() {
            warn!(call_id = %call_id, "call_open missing command_encrypted field");
            self.send_call_exit(
                &call_id,
                -1,
                Some("missing command_encrypted field in call_open".to_string()),
                None,
                None,
                0,
            )
            .await;
            return;
        }

        if !scope_allows_command(grant_scope, file_access, command_kind) {
            let reason = format!(
                "grant scope {:?} / file_access {:?} does not allow command kind {:?}",
                grant_scope, file_access, command_kind
            );
            warn!(
                call_id = %call_id,
                grant_id = %grant_id,
                %reason,
                "SECURITY: call_open rejected due to grant scope mismatch"
            );
            self.send_call_exit(&call_id, -2, Some(reason), None, None, 0)
                .await;
            return;
        }

        let encrypted_payload: EncryptedPayload = match data
            .get("command_encrypted")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok())
        {
            Some(payload) => payload,
            None => {
                self.send_call_exit(
                    &call_id,
                    -1,
                    Some("invalid command_encrypted payload in call_open".to_string()),
                    None,
                    None,
                    0,
                )
                .await;
                return;
            }
        };

        let mut command = match self.decrypt_call_command(
            &grant_id,
            &call_id,
            command_kind,
            grant_scope,
            &encrypted_payload,
        ) {
            Ok(command) => command,
            Err(error) => {
                warn!(
                    call_id = %call_id,
                    grant_id = %grant_id,
                    error = %error,
                    "failed to decrypt call_open command payload"
                );
                self.send_call_exit(&call_id, -2, Some(error.to_string()), None, None, 0)
                    .await;
                return;
            }
        };

        if command.kind != command_kind {
            let reason = format!(
                "decrypted command kind {:?} does not match transport kind {:?}",
                command.kind, command_kind
            );
            warn!(call_id = %call_id, grant_id = %grant_id, %reason, "SECURITY: command kind mismatch");
            self.send_call_exit(&call_id, -2, Some(reason), None, None, 0)
                .await;
            return;
        }
        command.grant_id = Some(grant_id.clone());
        command.caller_fingerprint = caller_fp.clone();
        command.ssh_fingerprint = ssh_fp.clone();
        command.file_access = file_access;

        if let Some(query) = &command.query {
            if command_kind == CommandKind::QueryReadonly
                && query.capability() == bifrost_command::CommandCapability::Mutating
            {
                let reason = format!(
                    "query '{}' requires a mutating transport kind, but caller declared query.readonly",
                    query.command_id()
                );
                warn!(call_id = %call_id, grant_id = %grant_id, %reason, "SECURITY: mutating query kind mismatch");
                self.send_call_exit(&call_id, -2, Some(reason), None, None, 0)
                    .await;
                return;
            }
        }

        if let Some(reason) = resolve_shell_command_policy_for_grant(
            &mut command,
            &self.local_grants,
            &grant_id,
            &self.executor,
        ) {
            warn!(
                call_id = %call_id,
                grant_id = %grant_id,
                %reason,
                "SECURITY: shell command rejected by grant policy binding"
            );
            self.send_call_exit(&call_id, -2, Some(reason), None, None, 0)
                .await;
            return;
        }

        let command_summary_for_call =
            build_call_command_summary(data.get("command_summary"), &command, command_kind);

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
            command = %command.summary_label(),
            args_json = ?command.args_json,
            query = ?command.query,
            "executing remote command via call_open"
        );

        let call_started_at = now_millis();
        let active_call = Arc::new(ActiveCallControl::new(grant_id.clone(), call_started_at));
        self.active_calls
            .write()
            .insert(call_id.clone(), Arc::clone(&active_call));

        {
            let mut call_info = CallInfo {
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
                command_kind,
                status: CallStatus::Streaming,
                command_summary: command_summary_for_call,
                command: RemoteCommand {
                    kind: command.kind,
                    command: command.command.clone(),
                    args_json: command.args_json.clone(),
                    query: command.query.clone(),
                    policy_id: command.policy_id.clone(),
                    exec_mode: command.exec_mode,
                    argv: command.argv.clone(),
                    shell: command.shell.clone(),
                    command_text: command.command_text.clone(),
                    cwd: command.cwd.clone(),
                    env: command.env.clone(),
                    stdin_mode: command.stdin_mode,
                    timeout_ms: command.timeout_ms,
                    pty: command.pty.clone(),
                    output_mode: command.output_mode,
                    grant_id: None,
                    caller_fingerprint: None,
                    ssh_fingerprint: None,
                    file_access: Default::default(),
                },
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
                policy_id: command.policy_id.clone(),
                exec_mode: command.exec_mode,
                output_mode: command.output_mode,
                pty_enabled: command.pty.as_ref().map(|pty| pty.enabled),
            };
            sanitize_call_for_history(&mut call_info);
            let max = self.config.max_records as usize;
            self.persist_call_history_entry(&call_info);
            let mut history = self.call_history.write();
            history.push_back(call_info);
            while history.len() > max {
                history.pop_front();
            }
        }

        let grant_crypto_lookup = { self.grant_crypto.read().get(&grant_id).cloned() };
        let grant_crypto = match grant_crypto_lookup {
            Some(crypto) => crypto,
            None => {
                self.send_call_exit(
                    &call_id,
                    -2,
                    Some(
                        "missing grant shared secret for encrypted remote command stream; reconnect is required"
                            .to_string(),
                    ),
                    None,
                    None,
                    0,
                )
                .await;
                return;
            }
        };

        let executor = Arc::clone(&self.executor);
        let relay_client = Arc::clone(&self.relay_client);
        let instance_id = self.identity.instance_id.clone();
        let active_calls = Arc::clone(&self.active_calls);
        let active_call_for_task = Arc::clone(&active_call);
        let call_history = Arc::clone(&self.call_history);
        let call_history_store = Arc::clone(&self.call_history_store);
        let call_history_relay_url = self.relay_client.base_url();
        let call_history_client_instance_id = self.identity.instance_id.clone();
        let call_history_max_records = self.config.max_records as usize;
        let call_history_retention_days = self.config.retention_days;
        let cid = call_id.clone();
        let command_kind_for_stream = command.kind;
        let grant_scope_for_stream = grant_scope;

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
            // PR B: track absolute stdout byte offset so stream_frame emission
            // carries the real head. CLI's CallerStreamState verifies contiguity
            // against this offset; a stuck 0u64 causes all frames after the first
            // to be treated as reconnect/dedup candidates.
            let mut next_stdout_offset: u64 = 0;
            // PR #4c-2: register a session in the global ring so tee/resume can work.
            let _session_registered = session_ring::register_session_str(&cid);

            let result = executor
                .execute_with_stdout_sink(&command, |chunk| {
                    let relay_client = Arc::clone(&relay_client);
                    let cid = cid.clone();
                    let instance_id = instance_id.clone();
                    let grant_crypto = grant_crypto.clone();
                    let seq = next_seq;
                    next_seq += 1;
                    let offset_for_stream = next_stdout_offset;
                    next_stdout_offset = next_stdout_offset.saturating_add(chunk.len() as u64);

                    async move {
                        // PR #4c-2: mirror stdout bytes into the session ring
                        // for resume. Silent no-op if cid is not a UUID.
                        session_ring::tee_stdout_str(&cid, &chunk);
                        // PR#6c: clone before chunk/instance_id get moved into legacy path
                        let chunk_bytes_for_stream = chunk.clone();
                        let instance_id_for_stream = instance_id.clone();
                        let envelope = Self::encrypt_call_frame(
                            &grant_crypto,
                            &cid,
                            seq,
                            String::from_utf8_lossy(&chunk).into_owned(),
                            command_kind_for_stream,
                            grant_scope_for_stream,
                        )?;

                        let envelope_json = serde_json::to_string(&envelope).unwrap_or_default();
                        let frame_req = ClientCallFrameRequest {
                            call_id: cid.clone(),
                            client_instance_id: instance_id,
                            envelope_json,
                        };

                        // PR#6c-followup: run the legacy /frame POST and the
                        // new /stream-frame POST concurrently with tokio::join!
                        // so neither RTT serializes the stdout chunk sink. The
                        // stream_frame result is intentionally best-effort (the
                        // legacy envelope path remains the source of truth
                        // until PR #5e flips the caller default), but running
                        // both in parallel prevents either slow leg from
                        // blocking the executor read loop — the root cause of
                        // the CI hang reverted in f1e2f88.
                        let legacy_fut = relay_client.post_call_frame(&cid, &frame_req);
                        let stream_frame = stream_emit::build_stdout_frame(
                            seq,
                            offset_for_stream,
                            &chunk_bytes_for_stream,
                        );
                        let stream_frame_json = stream_emit::frame_to_json(&stream_frame);
                        let cid_for_stream = cid.clone();
                        let relay_for_stream = Arc::clone(&relay_client);
                        let stream_fut = async move {
                            if let Some(frame_json) = stream_frame_json {
                                let stream_req = ClientCallStreamFrameRequest {
                                    call_id: cid_for_stream.clone(),
                                    client_instance_id: instance_id_for_stream,
                                    frame_json,
                                };
                                if let Err(err) = relay_for_stream
                                    .post_call_stream_frame(&cid_for_stream, &stream_req)
                                    .await
                                {
                                    tracing::debug!(
                                        call_id = %cid_for_stream,
                                        ?err,
                                        "parallel stream_frame post failed"
                                    );
                                }
                            }
                        };
                        let (legacy_result, ()) = tokio::join!(legacy_fut, stream_fut);
                        legacy_result
                    }
                })
                .await;
            let duration_ms = start.elapsed().as_millis() as u64;

            match result {
                Ok(response) => {
                    // PR #4c-2: finalize the session ring for successful exit.
                    session_ring::finalize_session_str(
                        &cid,
                        super::session_ring::SessionStatus::Done {
                            exit_code: response.exit_code,
                        },
                    );
                    if active_call_for_task.is_cancelled() {
                        info!(call_id = %cid, "skip completion update because call was cancelled");
                        active_calls.write().remove(&cid);
                        return;
                    }
                    let exit_encrypted = match Self::encrypt_call_exit(
                        &grant_crypto,
                        &cid,
                        response.exit_code,
                        Some(duration_ms),
                        response.stderr.clone(),
                        response.stdout_digest.clone(),
                        response.stderr_digest.clone(),
                    ) {
                        Ok(payload) => Some(payload),
                        Err(error) => {
                            // PR #4c-2: finalize the session ring for failure.
                            session_ring::finalize_session_str(
                                &cid,
                                super::session_ring::SessionStatus::Failed {
                                    code: error.to_string(),
                                },
                            );
                            error!(
                                error = %error,
                                call_id = %cid,
                                "failed to encrypt call exit payload"
                            );
                            None
                        }
                    };
                    let exit_req = ClientCallExitRequest {
                        call_id: cid.clone(),
                        client_instance_id: instance_id,
                        exit_code: response.exit_code,
                        duration_ms: Some(duration_ms),
                        stderr: None,
                        stdout_digest: response.stdout_digest.clone(),
                        stderr_digest: response.stderr_digest.clone(),
                        bytes_in: Some(0),
                        bytes_out: response.stdout.as_ref().map(|s| s.len() as u64),
                        exit_encrypted,
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
                    if update_call_in_history(
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
                    ) {
                        persist_call_history_entry_from_lock(
                            &call_history_store,
                            &call_history_relay_url,
                            &call_history_client_instance_id,
                            &call_history,
                            &cid,
                            call_history_max_records,
                            call_history_retention_days,
                        );
                    }
                }
                Err(e) => {
                    if active_call_for_task.is_cancelled() {
                        info!(call_id = %cid, "skip failure update because call was cancelled");
                        active_calls.write().remove(&cid);
                        return;
                    }
                    error!(error = %e, call_id = %cid, "remote command execution failed");
                    // PR#8-followup: the success branch already finalizes the
                    // session ring on Done; the error branch previously skipped
                    // finalize, leaking the 128 MiB ring forever. Close it now
                    // so the global registry does not grow unbounded for
                    // failed calls.
                    session_ring::finalize_session_str(
                        &cid,
                        super::session_ring::SessionStatus::Failed {
                            code: e.to_string(),
                        },
                    );
                    let stderr = e.to_string();
                    let exit_encrypted = match Self::encrypt_call_exit(
                        &grant_crypto,
                        &cid,
                        -1,
                        Some(duration_ms),
                        Some(stderr.clone()),
                        None,
                        Some(crate::remote_invoke::executor::sha1_hex(&stderr)),
                    ) {
                        Ok(payload) => Some(payload),
                        Err(error) => {
                            error!(
                                error = %error,
                                call_id = %cid,
                                "failed to encrypt error exit payload"
                            );
                            None
                        }
                    };

                    let exit_req = ClientCallExitRequest {
                        call_id: cid.clone(),
                        client_instance_id: instance_id,
                        exit_code: -1,
                        duration_ms: Some(duration_ms),
                        stderr: None,
                        stdout_digest: None,
                        stderr_digest: Some(crate::remote_invoke::executor::sha1_hex(&stderr)),
                        bytes_in: Some(0),
                        bytes_out: Some(0),
                        exit_encrypted,
                    };

                    if let Err(e2) = relay_client.post_call_exit(&cid, &exit_req).await {
                        error!(error = %e2, call_id = %cid, "failed to post error call exit");
                    }
                    if update_call_in_history(
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
                    ) {
                        persist_call_history_entry_from_lock(
                            &call_history_store,
                            &call_history_relay_url,
                            &call_history_client_instance_id,
                            &call_history,
                            &cid,
                            call_history_max_records,
                            call_history_retention_days,
                        );
                    }
                }
            }

            active_calls.write().remove(&cid);
        });
        *active_call.task.lock() = Some(task);
    }

    fn decrypt_call_command(
        &self,
        grant_id: &str,
        call_id: &str,
        command_kind: CommandKind,
        grant_scope: GrantScope,
        payload: &EncryptedPayload,
    ) -> Result<RemoteCommand> {
        let crypto = self
            .grant_crypto
            .read()
            .get(grant_id)
            .cloned()
            .ok_or_else(|| {
                BifrostError::Config(
                    "missing grant shared secret for encrypted remote command; reconnect is required"
                        .to_string(),
                )
            })?;

        let session_key = derive_open_call_session_key(
            &crypto.shared_secret,
            grant_id,
            Some(&crypto.caller_ephemeral_pub),
            Some(&crypto.client_ephemeral_pub),
            command_kind,
        )?;
        debug!(
            grant_id = %grant_id,
            command_kind = %command_kind.as_str(),
            shared_secret_fp = %short_fingerprint(&crypto.shared_secret),
            open_call_key_fp = %short_fingerprint(&session_key),
            "derived client open_call decryption key"
        );

        let fallback_aad = EnvelopeAad {
            version: payload.version,
            call_id: call_id.to_string(),
            seq: 0,
            direction: super::types::FrameDirection::CallerToClient,
            token_hash: None,
            frame_type: Some("command".to_string()),
            command_kind: Some(command_kind),
            grant_scope: Some(grant_scope),
            sender_key_id: None,
            metadata: None,
        };

        let mut command = decrypt_remote_command_payload(payload, &session_key, fallback_aad)?;
        if command.command.is_empty() {
            command.command = command.kind.as_str().to_string();
        }
        Ok(command)
    }

    fn encrypt_call_frame(
        crypto: &GrantCryptoMaterial,
        call_id: &str,
        seq: u64,
        chunk: String,
        _command_kind: CommandKind,
        _grant_scope: GrantScope,
    ) -> Result<EncryptedEnvelope> {
        let session_key = derive_call_session_key(
            &crypto.shared_secret,
            call_id,
            Some(&crypto.caller_ephemeral_pub),
            Some(&crypto.client_ephemeral_pub),
        )?;
        let payload = encrypt_encrypted_payload_without_aad(
            &serde_json::json!({ "chunk": chunk }),
            &session_key,
            2,
        )?;

        Ok(EncryptedEnvelope {
            version: payload.version,
            call_id: call_id.to_string(),
            seq,
            direction: FrameDirection::ClientToCaller,
            nonce: payload.nonce,
            ciphertext: payload.ciphertext,
            tag: payload.tag,
            aad: payload.aad,
        })
    }

    fn encrypt_call_exit(
        crypto: &GrantCryptoMaterial,
        call_id: &str,
        exit_code: i32,
        duration_ms: Option<u64>,
        stderr: Option<String>,
        stdout_digest: Option<String>,
        stderr_digest: Option<String>,
    ) -> Result<EncryptedPayload> {
        let session_key = derive_call_session_key(
            &crypto.shared_secret,
            call_id,
            Some(&crypto.caller_ephemeral_pub),
            Some(&crypto.client_ephemeral_pub),
        )?;
        encrypt_encrypted_payload_without_aad(
            &serde_json::json!({
                "exit_code": exit_code,
                "duration_ms": duration_ms,
                "stderr": stderr,
                "stdout_digest": stdout_digest,
                "stderr_digest": stderr_digest,
            }),
            &session_key,
            2,
        )
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
            exit_encrypted: None,
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

    fn persist_call_history_entry(&self, call: &CallInfo) {
        if let Err(error) = self.call_history_store.upsert(
            &self.relay_client.base_url(),
            &self.identity.instance_id,
            call,
            self.config.max_records as usize,
            self.config.retention_days,
        ) {
            warn!(error = %error, call_id = %call.call_id, "persist remote invoke call history failed");
        }
    }

    fn persist_call_history_by_id(&self, call_id: &str) {
        if let Some(call) = find_call_in_history(&self.call_history, call_id) {
            self.persist_call_history_entry(&call);
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
            if updated {
                self.persist_call_history_by_id(call_id);
            } else {
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

fn pairing_request_is_alive(pairing: &TimestampedPairing, now: u64, ttl_ms: u64) -> bool {
    match pairing.request.expires_at {
        Some(expires_at) if expires_at > 0 => now < expires_at,
        _ => now.saturating_sub(pairing.received_at) < ttl_ms,
    }
}

fn parse_relay_timestamp_millis(value: &Value) -> Option<u64> {
    if let Some(ms) = value.as_u64() {
        return Some(ms);
    }
    let timestamp = value.as_str()?;
    let parsed = DateTime::parse_from_rfc3339(timestamp).ok()?;
    Some(parsed.with_timezone(&Utc).timestamp_millis().max(0) as u64)
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

fn find_call_in_history(history: &RwLock<VecDeque<CallInfo>>, call_id: &str) -> Option<CallInfo> {
    history
        .read()
        .iter()
        .rev()
        .find(|c| c.call_id == call_id)
        .cloned()
}

fn persist_call_history_entry_from_lock(
    store: &CallHistoryStore,
    relay_url: &str,
    client_instance_id: &str,
    history: &RwLock<VecDeque<CallInfo>>,
    call_id: &str,
    max_records: usize,
    retention_days: u32,
) {
    let Some(call) = find_call_in_history(history, call_id) else {
        debug!(call_id = %call_id, "skip persisting missing call history entry");
        return;
    };
    if let Err(error) = store.upsert(
        relay_url,
        client_instance_id,
        &call,
        max_records,
        retention_days,
    ) {
        warn!(error = %error, call_id = %call_id, "persist remote invoke call history failed");
    }
}

fn build_call_command_summary(
    summary_value: Option<&Value>,
    command: &RemoteCommand,
    command_kind: CommandKind,
) -> CommandSummary {
    let mut summary: CommandSummary = summary_value
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default();

    let preview = summary.command_preview.trim();
    let fallback = command.summary_label().trim();
    if preview.is_empty() || (preview == command_kind.as_str() && !fallback.is_empty()) {
        summary.command_preview = if fallback.is_empty() {
            command_kind.as_str().to_string()
        } else {
            fallback.to_string()
        };
    }

    let has_masked_args = summary
        .masked_args_json
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    if !has_masked_args {
        summary.masked_args_json = command.summary_args_json();
    }

    summary
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

fn is_relay_stale_pairing_error(err: &BifrostError) -> bool {
    matches!(
        err,
        BifrostError::Network(msg)
            if msg.contains("pairing_expired")
                || msg.contains("pairing_not_found")
                || msg.contains("pairing_not_pending")
                || msg.contains("not found or expired")
    )
}

fn pairing_not_found_or_expired_error(pairing_id: &str) -> BifrostError {
    BifrostError::Network(format!("pairing {} not found or expired", pairing_id))
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
                    grant.last_command_at = Some(now);
                    grant.last_used_at = Some(now);
                    None
                }
            } else {
                grant.last_command_at = Some(now);
                grant.last_used_at = Some(now);
                None
            }
        }
    }
}

fn has_usable_grant_crypto(
    grant_crypto: &HashMap<String, GrantCryptoMaterial>,
    grant: &GrantInfo,
) -> bool {
    let Some(material) = grant_crypto.get(&grant.grant_id) else {
        return false;
    };

    if let Some(caller_ephemeral_pub) = &grant.caller_ephemeral_pub {
        if !caller_ephemeral_pub.is_empty()
            && material.caller_ephemeral_pub != *caller_ephemeral_pub
        {
            return false;
        }
    }

    if let Some(client_ephemeral_pub) = &grant.client_ephemeral_pub {
        if !client_ephemeral_pub.is_empty()
            && material.client_ephemeral_pub != *client_ephemeral_pub
        {
            return false;
        }
    }

    !material.shared_secret.is_empty()
}

fn default_query_grant_provision() -> ShellGrantProvision {
    ShellGrantProvision {
        grant_scope: GrantScope::RemoteQuery,
        file_access: FileAccessScope::None,
        policy_binding: None,
        shell_policy_set_version_snapshot: None,
        interactive_allowed: None,
        stdin_allowed: None,
    }
}

fn shell_grant_provision(
    requested_grant_scope: Option<GrantScope>,
    requested_file_access: Option<FileAccessScope>,
    requested_policy_binding: Option<Value>,
    requested_interactive_allowed: Option<bool>,
    requested_stdin_allowed: Option<bool>,
) -> Result<ShellGrantProvision> {
    let grant_scope = requested_grant_scope.unwrap_or(GrantScope::RemoteShellExec);
    // 层级模型：Shell 自动包含 File(read_write) + Query，File 包含 Query
    let file_access = requested_file_access.unwrap_or(match grant_scope {
        GrantScope::RemoteShellExec | GrantScope::RemoteShellInteractive => {
            FileAccessScope::ReadWrite
        }
        GrantScope::RemoteQuery => FileAccessScope::None,
        GrantScope::RemotePowerMgmt => FileAccessScope::None,
    });

    let store = RemoteShellStore::new()?;
    let set = store.load()?;
    let has_enabled_policy = set.policies.iter().any(|policy| policy.enabled);
    if !has_enabled_policy {
        // 无 shell policy 降级为 query，file_access 也应按 query 层级决定
        let mut provision = default_query_grant_provision();
        provision.file_access = requested_file_access.unwrap_or(FileAccessScope::None);
        return Ok(provision);
    }

    if grant_scope == GrantScope::RemoteQuery {
        let mut provision = default_query_grant_provision();
        provision.file_access = file_access;
        return Ok(provision);
    }

    let policy_binding = normalize_shell_policy_binding(&set, requested_policy_binding)?;
    let interactive_allowed = requested_interactive_allowed.unwrap_or(false);
    if interactive_allowed && grant_scope != GrantScope::RemoteShellInteractive {
        return Err(BifrostError::Config(
            "interactive shell access requires grant_scope=remote_shell_interactive".to_string(),
        ));
    }

    Ok(ShellGrantProvision {
        grant_scope,
        file_access,
        policy_binding: Some(policy_binding),
        shell_policy_set_version_snapshot: Some(set.current_version()),
        interactive_allowed: Some(interactive_allowed),
        stdin_allowed: Some(requested_stdin_allowed.unwrap_or(false)),
    })
}

fn updated_shell_grant_provision(
    existing: &GrantInfo,
    requested_grant_scope: Option<GrantScope>,
    requested_file_access: Option<FileAccessScope>,
    requested_policy_binding: Option<Value>,
    requested_interactive_allowed: Option<bool>,
    requested_stdin_allowed: Option<bool>,
) -> Result<ShellGrantProvision> {
    let desired_scope = requested_grant_scope.unwrap_or(existing.grant_scope);
    // 层级模型：shell scope 自动包含 file read_write
    let file_access = requested_file_access.unwrap_or(match desired_scope {
        GrantScope::RemoteShellExec | GrantScope::RemoteShellInteractive => {
            FileAccessScope::ReadWrite
        }
        GrantScope::RemoteQuery => existing.file_access,
        GrantScope::RemotePowerMgmt => FileAccessScope::None,
    });
    if desired_scope == GrantScope::RemoteQuery {
        let mut provision = default_query_grant_provision();
        provision.file_access = file_access;
        return Ok(provision);
    }

    let store = RemoteShellStore::new()?;
    let set = store.load()?;
    if !set.policies.iter().any(|policy| policy.enabled) {
        return Err(BifrostError::Config(
            "no enabled shell policy exists on this device".to_string(),
        ));
    }

    let desired_policy_binding =
        requested_policy_binding.or_else(|| existing.policy_binding.clone());
    let policy_binding = normalize_shell_policy_binding(&set, desired_policy_binding)?;
    let interactive_allowed = requested_interactive_allowed
        .or(existing.interactive_allowed)
        .unwrap_or(false);
    if interactive_allowed && desired_scope != GrantScope::RemoteShellInteractive {
        return Err(BifrostError::Config(
            "interactive shell access requires grant_scope=remote_shell_interactive".to_string(),
        ));
    }

    Ok(ShellGrantProvision {
        grant_scope: desired_scope,
        file_access,
        policy_binding: Some(policy_binding),
        shell_policy_set_version_snapshot: Some(set.current_version()),
        interactive_allowed: Some(interactive_allowed),
        stdin_allowed: Some(
            requested_stdin_allowed
                .or(existing.stdin_allowed)
                .unwrap_or(false),
        ),
    })
}

fn normalize_shell_policy_binding(
    set: &bifrost_storage::RemoteShellSet,
    binding: Option<Value>,
) -> Result<Value> {
    let enabled_policy_ids = set
        .policies
        .iter()
        .filter(|policy| policy.enabled)
        .map(|policy| policy.id.as_str())
        .collect::<Vec<_>>();

    if enabled_policy_ids.is_empty() {
        return Err(BifrostError::Config(
            "no enabled shell policy exists on this device".to_string(),
        ));
    }

    let Some(binding) = binding else {
        return Ok(json!({ "mode": "all" }));
    };

    let mode = binding
        .get("mode")
        .and_then(|value| value.as_str())
        .unwrap_or("all");
    if mode == "all" {
        return Ok(json!({ "mode": "all" }));
    }

    let Some(policy_ids) = binding.get("policy_ids").and_then(|value| value.as_array()) else {
        return Err(BifrostError::Config(
            "shell policy binding requires a non-empty policy_ids array".to_string(),
        ));
    };

    let normalized_policy_ids = policy_ids
        .iter()
        .filter_map(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();

    if normalized_policy_ids.is_empty() {
        return Err(BifrostError::Config(
            "shell policy binding requires at least one policy id".to_string(),
        ));
    }

    for policy_id in &normalized_policy_ids {
        if !enabled_policy_ids
            .iter()
            .any(|enabled| enabled == policy_id)
        {
            return Err(BifrostError::Config(format!(
                "shell policy '{}' is not enabled on this device",
                policy_id
            )));
        }
    }

    Ok(json!({
        "mode": "selected",
        "policy_ids": normalized_policy_ids,
    }))
}

fn resolve_shell_command_policy_for_grant(
    command: &mut RemoteCommand,
    grants: &Arc<RwLock<HashMap<String, GrantInfo>>>,
    grant_id: &str,
    executor: &Arc<RemoteInvokeExecutor>,
) -> Option<String> {
    if command.kind != CommandKind::ShellExec {
        return None;
    }

    let grant = grants.read().get(grant_id).cloned()?;
    if grant.grant_scope == GrantScope::RemoteQuery {
        return Some("grant scope does not allow shell.exec".to_string());
    }
    if command
        .policy_id
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Some(
            "shell.exec caller must not specify policy_id; the target device selects policy"
                .to_string(),
        );
    }

    if let Some(snapshot) = grant.shell_policy_set_version_snapshot {
        match RemoteShellStore::new().and_then(|store| store.current_version()) {
            Ok(current_version) if current_version != snapshot => {
                return Some(format!(
                    "shell policy set version changed (grant={}, current={}), reconnect is required",
                    snapshot, current_version
                ));
            }
            Err(error) => {
                return Some(format!("load shell policy set version failed: {}", error));
            }
            _ => {}
        }
    }

    match executor.select_policy_id_for_command(command, grant.policy_binding.as_ref()) {
        Ok(policy_id) => {
            command.policy_id = Some(policy_id);
            None
        }
        Err(error) => Some(error.to_string()),
    }
}

fn build_grant_crypto_material(caller_ephemeral_pub: &str) -> Result<GrantCryptoMaterial> {
    let engine = base64::engine::general_purpose::STANDARD;
    let caller_public_key = engine.decode(caller_ephemeral_pub).map_err(|e| {
        BifrostError::Config(format!("invalid caller_ephemeral_pub encoding: {}", e))
    })?;

    let rng = SystemRandom::new();
    let my_private = EphemeralPrivateKey::generate(&X25519, &rng)
        .map_err(|_| BifrostError::Config("generate client ephemeral key failed".to_string()))?;
    let my_public = my_private
        .compute_public_key()
        .map_err(|_| BifrostError::Config("compute client ephemeral pubkey failed".to_string()))?;
    let client_ephemeral_pub = engine.encode(my_public.as_ref());

    let peer = UnparsedPublicKey::new(&X25519, caller_public_key);
    let shared_secret = agree_ephemeral(my_private, &peer, |shared_secret| shared_secret.to_vec())
        .map_err(|_| BifrostError::Config("derive grant shared secret failed".to_string()))?;

    Ok(GrantCryptoMaterial {
        shared_secret,
        caller_ephemeral_pub: caller_ephemeral_pub.to_string(),
        client_ephemeral_pub,
    })
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

    let grant_scope: GrantScope = data
        .get("grant_scope")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let file_access: FileAccessScope = data
        .get("file_access")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_else(|| FileAccessScope::default_for(grant_scope));

    Some(GrantInfo {
        grant_id,
        client_instance_id: client_instance_id.to_string(),
        caller_fingerprint,
        caller_display_name,
        grant_mode,
        grant_scope,
        file_access,
        auth_method,
        status: GrantStatus::Active,
        first_authorized_at: authorized_at,
        last_command_at: data.get("last_command_at").and_then(|v| v.as_u64()),
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
        caller_ephemeral_pub: data
            .get("caller_ephemeral_pub")
            .and_then(|v| v.as_str())
            .map(|value| value.to_string()),
        client_ephemeral_pub: data
            .get("client_ephemeral_pub")
            .and_then(|v| v.as_str())
            .map(|value| value.to_string()),
        policy_binding: data.get("policy_binding").cloned(),
        shell_policy_set_version_snapshot: data
            .get("shell_policy_set_version_snapshot")
            .and_then(|v| v.as_u64()),
        interactive_allowed: data.get("interactive_allowed").and_then(|v| v.as_bool()),
        stdin_allowed: data.get("stdin_allowed").and_then(|v| v.as_bool()),
    })
}

fn recover_grant_info_from_call_open(
    data: &Value,
    grant_id: String,
    client_instance_id: &str,
    command_kind: CommandKind,
    active_ssh_key: Option<&SshKeyRecord>,
) -> GrantInfo {
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
    let ssh_key_fingerprint = data.get("ssh_key_fingerprint").and_then(|v| v.as_str());
    let active_ssh_key = active_ssh_key.filter(|record| {
        ssh_key_fingerprint
            .map(|fingerprint| record.ssh_key_fingerprint == fingerprint)
            .unwrap_or_else(|| record.ssh_key_fingerprint == caller_fingerprint)
    });
    let grant_scope: GrantScope = data
        .get("grant_scope")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or(GrantScope::RemoteQuery);
    let file_access: FileAccessScope = data
        .get("file_access")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_else(|| {
            if active_ssh_key.is_some() && command_kind == CommandKind::File {
                FileAccessScope::ReadWrite
            } else {
                FileAccessScope::default_for(grant_scope)
            }
        });

    GrantInfo {
        grant_id,
        client_instance_id: client_instance_id.to_string(),
        caller_fingerprint,
        caller_display_name,
        grant_mode: GrantMode::Permanent,
        grant_scope,
        file_access,
        auth_method: if active_ssh_key.is_some() {
            AuthMethod::SshPublickey
        } else {
            AuthMethod::PairCode
        },
        status: GrantStatus::Active,
        first_authorized_at: now_millis(),
        last_command_at: data.get("last_command_at").and_then(|v| v.as_u64()),
        expires_at: None,
        last_used_at: None,
        max_calls: None,
        remaining_calls: None,
        ssh_key_id: active_ssh_key.map(|record| record.id.clone()),
        ssh_key_fingerprint: active_ssh_key.map(|record| record.ssh_key_fingerprint.clone()),
        caller_ephemeral_pub: data
            .get("caller_ephemeral_pub")
            .and_then(|v| v.as_str())
            .map(|value| value.to_string()),
        client_ephemeral_pub: data
            .get("client_ephemeral_pub")
            .and_then(|v| v.as_str())
            .map(|value| value.to_string()),
        policy_binding: data.get("policy_binding").cloned(),
        shell_policy_set_version_snapshot: data
            .get("shell_policy_set_version_snapshot")
            .and_then(|v| v.as_u64()),
        interactive_allowed: data.get("interactive_allowed").and_then(|v| v.as_bool()),
        stdin_allowed: data.get("stdin_allowed").and_then(|v| v.as_bool()),
    }
}

fn apply_stored_grant_policy(
    mut grant: GrantInfo,
    stored: Option<&StoredGrantPolicy>,
) -> GrantInfo {
    if let Some(stored) = stored {
        grant.grant_scope = stored.grant_scope;
        // Layered model migration: old persisted policies may lack file_access
        // (deserialized as None via #[serde(default)]). Route through the
        // single-source-of-truth helper so Shell* scopes auto-include file
        // read_write per the layered permission model.
        grant.file_access = if stored.file_access == FileAccessScope::None {
            FileAccessScope::default_for(stored.grant_scope)
        } else {
            stored.file_access
        };
        grant.policy_binding = stored.policy_binding.clone();
        grant.shell_policy_set_version_snapshot = stored.shell_policy_set_version_snapshot;
        grant.interactive_allowed = stored.interactive_allowed;
        grant.stdin_allowed = stored.stdin_allowed;
    }
    grant
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_invoke::file_policy_store::{
        load_raw_config, save_raw_config, GrantMatch, RawConfig, RawGrantPolicy,
    };
    use crate::remote_invoke::ssh_keys::SshKeyStatus;
    use crate::remote_invoke::types::ShellExecMode;
    use crate::state::AdminState;
    use base64::Engine;
    use bifrost_command::{CanonicalQueryCommand, SearchArgs};
    use bifrost_core::file_access::FileOp;
    use bifrost_storage::{RemoteShellPolicy, RemoteShellSet, RemoteShellStore};
    use ring::agreement::EphemeralPrivateKey;
    use ring::rand::SystemRandom;
    use tempfile::TempDir;

    fn make_active_grant(grant_id: &str, mode: GrantMode) -> GrantInfo {
        GrantInfo {
            grant_id: grant_id.to_string(),
            client_instance_id: "test-instance".to_string(),
            caller_fingerprint: "test-fp".to_string(),
            caller_display_name: None,
            grant_mode: mode,
            grant_scope: GrantScope::RemoteQuery,
            file_access: Default::default(),
            auth_method: AuthMethod::PairCode,
            status: GrantStatus::Active,
            first_authorized_at: 1000,
            last_command_at: None,
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
            caller_ephemeral_pub: None,
            client_ephemeral_pub: None,
            policy_binding: None,
            shell_policy_set_version_snapshot: None,
            interactive_allowed: None,
            stdin_allowed: None,
        }
    }

    fn setup_remote_shell_store(version: u64) -> (std::sync::MutexGuard<'static, ()>, TempDir) {
        let guard = crate::remote_invoke::remote_shell_test_guard();
        let dir = TempDir::new().expect("tempdir");
        let data_dir = dir.path().join("bifrost-data");
        std::fs::create_dir_all(&data_dir).expect("create data dir");
        bifrost_storage::set_data_dir(data_dir);
        let store = RemoteShellStore::new().expect("remote shell store");
        store
            .save(&RemoteShellSet {
                schema_version: 1,
                version,
                policies: vec![RemoteShellPolicy {
                    id: "echo-argv".to_string(),
                    name: "echo-argv".to_string(),
                    description: None,
                    enabled: true,
                    profile_id: None,
                    metadata: serde_json::json!({
                        "exec_mode": "argv_exec"
                    }),
                }],
                profiles: vec![],
            })
            .expect("save store");
        (guard, dir)
    }

    #[test]
    fn remove_grant_policy_removes_exact_file_access_policy_but_keeps_fingerprint_policy() {
        let _guard = crate::remote_invoke::remote_shell_test_guard();
        let dir = TempDir::new().expect("tempdir");
        let data_dir = dir.path().join("bifrost-data");
        std::fs::create_dir_all(&data_dir).expect("create data dir");
        bifrost_storage::set_data_dir(data_dir.clone());

        save_raw_config(&RawConfig {
            grants: vec![
                RawGrantPolicy {
                    match_: GrantMatch {
                        grant_id: Some("grant-deleted".to_string()),
                        ..Default::default()
                    },
                    name: Some("exact-grant".to_string()),
                    roots: vec![data_dir.clone()],
                    ops: vec![FileOp::Read],
                    ..Default::default()
                },
                RawGrantPolicy {
                    match_: GrantMatch {
                        ssh_fingerprint: Some("ssh-fp".to_string()),
                        ..Default::default()
                    },
                    name: Some("ssh-default".to_string()),
                    roots: vec![data_dir],
                    ops: vec![FileOp::Read],
                    ..Default::default()
                },
            ],
            default: None,
        })
        .expect("save file access config");

        let worker = RemoteInvokeWorker::new(
            RemoteInvokeConfig::default(),
            Identity::load_or_create(dir.path()).expect("identity"),
            None,
            Arc::new(AdminState::new(0)),
            "127.0.0.1",
            0,
        );

        worker.remove_grant_policy("grant-deleted");

        let cfg = load_raw_config();
        assert_eq!(cfg.grants.len(), 1);
        assert_eq!(cfg.grants[0].name.as_deref(), Some("ssh-default"));
        assert_eq!(
            cfg.grants[0].match_.ssh_fingerprint.as_deref(),
            Some("ssh-fp")
        );
        assert!(cfg
            .grants
            .iter()
            .all(
                |policy| policy.match_.grant_id.as_deref() != Some("grant-deleted")
                    && policy.grant_id.as_deref() != Some("grant-deleted")
            ));
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
        assert_eq!(grants["g1"].last_command_at, Some(5000));
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
        assert_eq!(grants["g1"].last_command_at, Some(5000));
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

    #[test]
    fn test_is_relay_stale_pairing_error_matches_expired_and_not_pending() {
        let expired = BifrostError::Network(
            "relay submit_grant_decision failed with status 410 Gone: pairing_expired".to_string(),
        );
        let not_pending = BifrostError::Network(
            "relay submit_grant_decision failed with status 400 Bad Request: pairing_not_pending"
                .to_string(),
        );
        let unrelated =
            BifrostError::Network("relay submit_grant_decision unauthorized".to_string());

        assert!(is_relay_stale_pairing_error(&expired));
        assert!(is_relay_stale_pairing_error(&not_pending));
        assert!(!is_relay_stale_pairing_error(&unrelated));
    }

    #[test]
    fn test_pairing_request_is_alive_prefers_relay_expires_at() {
        let pairing = TimestampedPairing {
            request: PairingRequest {
                pairing_id: "pairing-1".to_string(),
                caller_info: CallerInfo::default(),
                command_summary: CommandSummary::default(),
                command: RemoteCommand::default(),
                caller_pubkey: String::new(),
                expires_at: Some(3_000),
                client_ephemeral_pub: None,
                caller_ephemeral_pub: None,
            },
            received_at: 1_000,
        };

        assert!(!pairing_request_is_alive(&pairing, 3_500, 120_000));
        assert!(pairing_request_is_alive(&pairing, 2_500, 120_000));
    }

    #[test]
    fn test_parse_relay_timestamp_millis_accepts_rfc3339() {
        let millis =
            parse_relay_timestamp_millis(&Value::String("2026-04-23T10:20:30.123Z".to_string()));

        assert_eq!(millis, Some(1_776_939_630_123));
    }

    #[test]
    fn test_shell_grant_provision_promotes_to_remote_shell_exec_when_policy_exists() {
        let (_guard, _dir) = setup_remote_shell_store(7);

        let provision =
            shell_grant_provision(None, None, None, None, None).expect("shell grant provision");

        assert_eq!(provision.grant_scope, GrantScope::RemoteShellExec);
        // Shell scope 层级模型：默认自动带 file_access read_write
        assert_eq!(provision.file_access, FileAccessScope::ReadWrite);
        assert_eq!(provision.shell_policy_set_version_snapshot, Some(7));
        assert_eq!(
            provision
                .policy_binding
                .as_ref()
                .and_then(|value| value.get("mode"))
                .and_then(|value| value.as_str()),
            Some("all")
        );
    }

    #[test]
    fn test_shell_grant_provision_accepts_selected_policy_binding() {
        let (_guard, _dir) = setup_remote_shell_store(7);

        let provision = shell_grant_provision(
            Some(GrantScope::RemoteShellExec),
            None,
            Some(serde_json::json!({
                "mode": "selected",
                "policy_ids": ["echo-argv"],
            })),
            Some(false),
            Some(true),
        )
        .expect("shell grant provision");

        assert_eq!(provision.grant_scope, GrantScope::RemoteShellExec);
        // Shell scope 自动带 file_access read_write
        assert_eq!(provision.file_access, FileAccessScope::ReadWrite);
        assert_eq!(provision.stdin_allowed, Some(true));
        assert_eq!(
            provision
                .policy_binding
                .as_ref()
                .and_then(|value| value.get("policy_ids"))
                .and_then(|value| value.as_array())
                .map(|values| values
                    .iter()
                    .filter_map(|value| value.as_str())
                    .collect::<Vec<_>>()),
            Some(vec!["echo-argv"])
        );
    }

    #[test]
    fn test_shell_grant_provision_with_file_access_without_shell_policy() {
        let _guard = crate::remote_invoke::remote_shell_test_guard();
        let dir = TempDir::new().expect("tempdir");
        let data_dir = dir.path().join("bifrost-data");
        std::fs::create_dir_all(&data_dir).expect("create data dir");
        bifrost_storage::set_data_dir(data_dir);
        RemoteShellStore::new()
            .expect("remote shell store")
            .save(&RemoteShellSet {
                schema_version: 1,
                version: 1,
                policies: vec![],
                profiles: vec![],
            })
            .expect("save empty remote shell store");

        let provision =
            shell_grant_provision(None, Some(FileAccessScope::ReadWrite), None, None, None)
                .expect("file access should work without shell policy");

        // No enabled shell policy => falls back to RemoteQuery for shell scope
        assert_eq!(provision.grant_scope, GrantScope::RemoteQuery);
        // But file_access is preserved
        assert_eq!(provision.file_access, FileAccessScope::ReadWrite);
        assert!(provision.policy_binding.is_none());
        assert!(provision.shell_policy_set_version_snapshot.is_none());
    }

    #[test]
    fn test_shell_grant_provision_with_file_access_and_shell_scope() {
        let _guard = crate::remote_invoke::remote_shell_test_guard();
        let dir = TempDir::new().expect("tempdir");
        let data_dir = dir.path().join("bifrost-data");
        std::fs::create_dir_all(&data_dir).expect("create data dir");
        bifrost_storage::set_data_dir(data_dir);
        RemoteShellStore::new()
            .expect("remote shell store")
            .save(&RemoteShellSet {
                schema_version: 1,
                version: 1,
                policies: vec![],
                profiles: vec![],
            })
            .expect("save empty remote shell store");

        // When no enabled shell policy, grant_scope falls back to RemoteQuery
        // but file_access is preserved from the request.
        let provision = shell_grant_provision(
            Some(GrantScope::RemoteShellExec),
            Some(FileAccessScope::ReadWrite),
            None,
            None,
            None,
        )
        .expect("file access should work even without shell policy");

        assert_eq!(provision.grant_scope, GrantScope::RemoteQuery);
        assert_eq!(provision.file_access, FileAccessScope::ReadWrite);
        assert!(provision.policy_binding.is_none());
        assert!(provision.shell_policy_set_version_snapshot.is_none());
    }

    #[test]
    fn test_shell_grant_provision_rejects_unknown_selected_policy() {
        let (_guard, _dir) = setup_remote_shell_store(7);

        let error = shell_grant_provision(
            Some(GrantScope::RemoteShellExec),
            None,
            Some(serde_json::json!({
                "mode": "selected",
                "policy_ids": ["missing-policy"],
            })),
            None,
            None,
        )
        .expect_err("missing policy should fail");

        assert!(error.to_string().contains("missing-policy"));
    }

    #[test]
    fn test_resolve_shell_command_policy_for_grant_rejects_version_mismatch() {
        let (_guard, _dir) = setup_remote_shell_store(9);
        let grants = Arc::new(RwLock::new(HashMap::new()));
        let mut grant = make_active_grant("g1", GrantMode::Permanent);
        grant.grant_scope = GrantScope::RemoteShellExec;
        grant.policy_binding = Some(serde_json::json!({ "mode": "all" }));
        grant.shell_policy_set_version_snapshot = Some(8);
        grants.write().insert("g1".to_string(), grant);

        let mut command = RemoteCommand {
            kind: CommandKind::ShellExec,
            exec_mode: Some(ShellExecMode::ArgvExec),
            argv: Some(vec!["/bin/echo".to_string(), "hello".to_string()]),
            ..Default::default()
        };
        let executor = Arc::new(RemoteInvokeExecutor::new("127.0.0.1", 18080));

        let reason = resolve_shell_command_policy_for_grant(&mut command, &grants, "g1", &executor)
            .expect("reason");
        assert!(reason.contains("version changed"));
    }

    #[test]
    fn test_resolve_shell_command_policy_for_grant_rejects_caller_selected_policy() {
        let (_guard, _dir) = setup_remote_shell_store(7);
        let grants = Arc::new(RwLock::new(HashMap::new()));
        let mut grant = make_active_grant("g1", GrantMode::Permanent);
        grant.grant_scope = GrantScope::RemoteShellExec;
        grant.policy_binding = Some(serde_json::json!({ "mode": "all" }));
        grant.shell_policy_set_version_snapshot = Some(7);
        grants.write().insert("g1".to_string(), grant);

        let mut command = RemoteCommand {
            kind: CommandKind::ShellExec,
            policy_id: Some("echo-argv".to_string()),
            exec_mode: Some(ShellExecMode::ArgvExec),
            argv: Some(vec!["/bin/echo".to_string(), "hello".to_string()]),
            ..Default::default()
        };
        let executor = Arc::new(RemoteInvokeExecutor::new("127.0.0.1", 18080));

        let reason = resolve_shell_command_policy_for_grant(&mut command, &grants, "g1", &executor)
            .expect("caller policy should be rejected");
        assert!(reason.contains("must not specify policy_id"));
    }

    #[test]
    fn test_resolve_shell_command_policy_for_grant_selects_target_policy() {
        let (_guard, dir) = setup_remote_shell_store(7);
        let data_dir = dir.path().join("bifrost-data");
        bifrost_storage::set_data_dir(data_dir);
        RemoteShellStore::new()
            .expect("store")
            .save(&RemoteShellSet {
                schema_version: 1,
                version: 7,
                policies: vec![
                    RemoteShellPolicy {
                        id: "echo-argv".to_string(),
                        name: "echo-argv".to_string(),
                        description: None,
                        enabled: true,
                        profile_id: None,
                        metadata: serde_json::json!({
                            "exec_mode": "argv_exec",
                            "allowed_executables": ["/bin/echo"]
                        }),
                    },
                    RemoteShellPolicy {
                        id: "pwd-argv".to_string(),
                        name: "pwd-argv".to_string(),
                        description: None,
                        enabled: true,
                        profile_id: None,
                        metadata: serde_json::json!({
                            "exec_mode": "argv_exec",
                            "allowed_executables": ["/bin/pwd"]
                        }),
                    },
                ],
                profiles: vec![],
            })
            .expect("save store");

        let grants = Arc::new(RwLock::new(HashMap::new()));
        let mut grant = make_active_grant("g1", GrantMode::Permanent);
        grant.grant_scope = GrantScope::RemoteShellExec;
        grant.policy_binding = Some(serde_json::json!({
            "mode": "selected",
            "policy_ids": ["pwd-argv"]
        }));
        grant.shell_policy_set_version_snapshot = Some(7);
        grants.write().insert("g1".to_string(), grant);

        let mut command = RemoteCommand {
            kind: CommandKind::ShellExec,
            exec_mode: Some(ShellExecMode::ArgvExec),
            argv: Some(vec!["/bin/pwd".to_string()]),
            ..Default::default()
        };
        let executor = Arc::new(RemoteInvokeExecutor::new("127.0.0.1", 18080));

        let reason = resolve_shell_command_policy_for_grant(&mut command, &grants, "g1", &executor);
        assert!(reason.is_none());
        assert_eq!(command.policy_id.as_deref(), Some("pwd-argv"));
    }

    #[test]
    fn test_has_usable_grant_crypto_requires_matching_local_material() {
        let grant = GrantInfo {
            grant_id: "grant-1".to_string(),
            caller_ephemeral_pub: Some("caller-epk".to_string()),
            client_ephemeral_pub: Some("client-epk".to_string()),
            ..make_active_grant("grant-1", GrantMode::Permanent)
        };
        let mut grant_crypto = HashMap::new();

        assert!(!has_usable_grant_crypto(&grant_crypto, &grant));

        grant_crypto.insert(
            "grant-1".to_string(),
            GrantCryptoMaterial {
                shared_secret: vec![1, 2, 3],
                caller_ephemeral_pub: "caller-epk".to_string(),
                client_ephemeral_pub: "client-epk".to_string(),
            },
        );
        assert!(has_usable_grant_crypto(&grant_crypto, &grant));

        grant_crypto.insert(
            "grant-1".to_string(),
            GrantCryptoMaterial {
                shared_secret: vec![1, 2, 3],
                caller_ephemeral_pub: "other-caller".to_string(),
                client_ephemeral_pub: "client-epk".to_string(),
            },
        );
        assert!(!has_usable_grant_crypto(&grant_crypto, &grant));
    }

    #[test]
    fn test_build_call_command_summary_falls_back_to_decrypted_command_when_preview_blank() {
        let command = RemoteCommand {
            kind: CommandKind::QueryReadonly,
            command: "status".to_string(),
            args_json: None,
            query: None,
            policy_id: None,
            exec_mode: None,
            argv: None,
            shell: None,
            command_text: None,
            cwd: None,
            env: None,
            stdin_mode: None,
            timeout_ms: None,
            pty: None,
            output_mode: None,
            grant_id: None,
            caller_fingerprint: None,
            ssh_fingerprint: None,
            file_access: Default::default(),
        };

        let summary = build_call_command_summary(
            Some(&serde_json::json!({
                "command_preview": "",
                "masked_args_json": "[\"--json\"]"
            })),
            &command,
            CommandKind::QueryReadonly,
        );

        assert_eq!(summary.command_preview, "status");
        assert_eq!(summary.masked_args_json.as_deref(), Some("[\"--json\"]"));
    }

    #[test]
    fn test_build_call_command_summary_replaces_route_level_preview_with_decrypted_command() {
        let command = RemoteCommand {
            kind: CommandKind::QueryReadonly,
            command: "status".to_string(),
            args_json: None,
            query: None,
            policy_id: None,
            exec_mode: None,
            argv: None,
            shell: None,
            command_text: None,
            cwd: None,
            env: None,
            stdin_mode: None,
            timeout_ms: None,
            pty: None,
            output_mode: None,
            grant_id: None,
            caller_fingerprint: None,
            ssh_fingerprint: None,
            file_access: Default::default(),
        };

        let summary = build_call_command_summary(
            Some(&serde_json::json!({
                "command_preview": "query.readonly"
            })),
            &command,
            CommandKind::QueryReadonly,
        );

        assert_eq!(summary.command_preview, "status");
    }

    #[test]
    fn test_build_call_command_summary_falls_back_to_decrypted_args_json_when_masked_args_missing()
    {
        let command = RemoteCommand {
            kind: CommandKind::QueryReadonly,
            command: "search.get".to_string(),
            args_json: Some(r#"{"query":"needle","max_results":5,"max_scan":50}"#.to_string()),
            query: None,
            policy_id: None,
            exec_mode: None,
            argv: None,
            shell: None,
            command_text: None,
            cwd: None,
            env: None,
            stdin_mode: None,
            timeout_ms: None,
            pty: None,
            output_mode: None,
            grant_id: None,
            caller_fingerprint: None,
            ssh_fingerprint: None,
            file_access: Default::default(),
        };

        let summary = build_call_command_summary(
            Some(&serde_json::json!({
                "command_preview": "search.get"
            })),
            &command,
            CommandKind::QueryReadonly,
        );

        assert_eq!(
            summary.masked_args_json.as_deref(),
            Some(r#"{"query":"needle","max_results":5,"max_scan":50}"#)
        );
    }

    #[test]
    fn test_build_call_command_summary_preserves_existing_masked_args_json() {
        let command = RemoteCommand {
            kind: CommandKind::QueryReadonly,
            command: "search.get".to_string(),
            args_json: Some(r#"{"query":"needle","max_results":5}"#.to_string()),
            query: None,
            policy_id: None,
            exec_mode: None,
            argv: None,
            shell: None,
            command_text: None,
            cwd: None,
            env: None,
            stdin_mode: None,
            timeout_ms: None,
            pty: None,
            output_mode: None,
            grant_id: None,
            caller_fingerprint: None,
            ssh_fingerprint: None,
            file_access: Default::default(),
        };

        let summary = build_call_command_summary(
            Some(&serde_json::json!({
                "command_preview": "search.get",
                "masked_args_json": "{\"query\":\"***\"}"
            })),
            &command,
            CommandKind::QueryReadonly,
        );

        assert_eq!(
            summary.masked_args_json.as_deref(),
            Some("{\"query\":\"***\"}")
        );
    }

    #[test]
    fn test_build_call_command_summary_falls_back_to_query_args_when_args_json_missing() {
        let command = RemoteCommand {
            kind: CommandKind::QueryReadonly,
            command: String::new(),
            args_json: None,
            query: Some(CanonicalQueryCommand::Search(SearchArgs {
                keyword: "needle".to_string(),
                limit: Some(5),
                max_scan: Some(50),
                max_results: Some(5),
                ..SearchArgs::default()
            })),
            policy_id: None,
            exec_mode: None,
            argv: None,
            shell: None,
            command_text: None,
            cwd: None,
            env: None,
            stdin_mode: None,
            timeout_ms: None,
            pty: None,
            output_mode: None,
            grant_id: None,
            caller_fingerprint: None,
            ssh_fingerprint: None,
            file_access: Default::default(),
        };

        let summary = build_call_command_summary(None, &command, CommandKind::QueryReadonly);

        let masked = summary
            .masked_args_json
            .as_deref()
            .expect("query args should be serialized");
        assert!(masked.contains("\"keyword\":\"needle\""));
        assert!(masked.contains("\"limit\":5"));
        assert!(masked.contains("\"max_scan\":50"));
        assert!(masked.contains("\"max_results\":5"));
    }

    fn make_call_info(call_id: &str) -> CallInfo {
        CallInfo {
            call_id: call_id.to_string(),
            grant_id: "grant-1".to_string(),
            pairing_id: None,
            client_instance_id: "test-instance".to_string(),
            caller_fingerprint: "test-fp".to_string(),
            auth_method: AuthMethod::PairCode,
            command_kind: CommandKind::QueryReadonly,
            status: CallStatus::Streaming,
            command_summary: CommandSummary {
                command_preview: "status".to_string(),
                ..Default::default()
            },
            command: RemoteCommand {
                kind: CommandKind::QueryReadonly,
                command: "status".to_string(),
                args_json: None,
                query: None,
                policy_id: None,
                exec_mode: None,
                argv: None,
                shell: None,
                command_text: None,
                cwd: None,
                env: None,
                stdin_mode: None,
                timeout_ms: None,
                pty: None,
                output_mode: None,
                grant_id: None,
                caller_fingerprint: None,
                ssh_fingerprint: None,
                file_access: Default::default(),
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
            policy_id: None,
            exec_mode: None,
            output_mode: None,
            pty_enabled: None,
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
            grant_scope: GrantScope::RemoteQuery,
            file_access: Default::default(),
            auth_method: AuthMethod::PairCode,
            status: GrantStatus::Active,
            first_authorized_at: 1000,
            last_command_at: None,
            expires_at: None,
            last_used_at: None,
            max_calls: None,
            remaining_calls: None,
            ssh_key_id: None,
            ssh_key_fingerprint: None,
            caller_ephemeral_pub: None,
            client_ephemeral_pub: None,
            policy_binding: None,
            shell_policy_set_version_snapshot: None,
            interactive_allowed: None,
            stdin_allowed: None,
        };
        grants.insert("auto-g".to_string(), auto_grant);

        let result = validate_grant_for_call(&mut grants, "auto-g", 5000);
        assert!(
            result.is_none(),
            "auto-recovered grant should pass validation"
        );
        assert_eq!(grants["auto-g"].last_used_at, Some(5000));
    }

    #[test]
    fn test_recover_call_open_ssh_file_grant_preserves_file_access() {
        let active_key = SshKeyRecord {
            id: "ssh-key-1".to_string(),
            device_code: "BF-TEST".to_string(),
            label: "mira".to_string(),
            public_key_pem: "pub".to_string(),
            ssh_key_fingerprint: "ssh-fp".to_string(),
            grant_mode: GrantMode::Permanent,
            status: SshKeyStatus::Active,
            created_at: 1,
            last_used_at: None,
            last_caller_info: None,
        };
        let payload = serde_json::json!({
            "caller_fingerprint": "caller-random-fp",
            "ssh_key_fingerprint": "ssh-fp",
            "caller_ephemeral_pub": "caller-epk",
            "client_ephemeral_pub": "client-epk"
        });

        let grant = recover_grant_info_from_call_open(
            &payload,
            "ghost-file-grant".to_string(),
            "client-x",
            CommandKind::File,
            Some(&active_key),
        );

        assert_eq!(grant.auth_method, AuthMethod::SshPublickey);
        assert_eq!(grant.caller_fingerprint, "caller-random-fp");
        assert_eq!(grant.file_access, FileAccessScope::ReadWrite);
        assert_eq!(grant.ssh_key_fingerprint.as_deref(), Some("ssh-fp"));
        assert!(scope_allows_command(
            grant.grant_scope,
            grant.file_access,
            CommandKind::File
        ));
    }

    #[test]
    fn test_recover_call_open_uses_payload_file_access_for_non_ssh_grant() {
        let payload = serde_json::json!({
            "caller_fingerprint": "pair-fp",
            "grant_scope": "remote_query",
            "file_access": "read"
        });

        let grant = recover_grant_info_from_call_open(
            &payload,
            "relay-prevalidated-file-grant".to_string(),
            "client-x",
            CommandKind::File,
            None,
        );

        assert_eq!(grant.auth_method, AuthMethod::PairCode);
        assert_eq!(grant.file_access, FileAccessScope::Read);
        assert!(scope_allows_command(
            grant.grant_scope,
            grant.file_access,
            CommandKind::File
        ));
    }

    #[test]
    fn test_build_grant_info_from_grant_created_parses_shell_exec_scope_and_keys() {
        let payload = serde_json::json!({
            "grant_id": "grant-shell",
            "caller_fingerprint": "caller-shell",
            "grant_mode": "permanent",
            "grant_scope": "remote_shell_exec",
            "caller_ephemeral_pub": "caller-epk",
            "client_ephemeral_pub": "client-epk",
            "interactive_allowed": false,
            "stdin_allowed": true,
            "shell_policy_set_version_snapshot": 3
        });

        let grant = build_grant_info_from_grant_created(&payload, "client-shell", 777)
            .expect("grant should parse");

        assert_eq!(grant.grant_scope, GrantScope::RemoteShellExec);
        assert_eq!(grant.caller_ephemeral_pub.as_deref(), Some("caller-epk"));
        assert_eq!(grant.client_ephemeral_pub.as_deref(), Some("client-epk"));
        assert_eq!(grant.interactive_allowed, Some(false));
        assert_eq!(grant.stdin_allowed, Some(true));
        assert_eq!(grant.shell_policy_set_version_snapshot, Some(3));
    }

    #[test]
    fn test_grant_scope_rejects_shell_exec_for_query_scope() {
        assert!(!GrantScope::RemoteQuery.allows_command(CommandKind::ShellExec));
        assert!(GrantScope::RemoteShellExec.allows_command(CommandKind::ShellExec));
    }

    #[test]
    fn test_build_grant_crypto_material_derives_x25519_secret() {
        let rng = SystemRandom::new();
        let caller_private =
            EphemeralPrivateKey::generate(&X25519, &rng).expect("caller private key");
        let caller_public = caller_private
            .compute_public_key()
            .expect("caller public key");
        let caller_public_b64 =
            base64::engine::general_purpose::STANDARD.encode(caller_public.as_ref());

        let material = build_grant_crypto_material(&caller_public_b64).expect("grant crypto");
        assert_eq!(material.caller_ephemeral_pub, caller_public_b64);
        assert!(!material.client_ephemeral_pub.is_empty());
        assert_eq!(material.shared_secret.len(), 32);
    }
}
