use std::collections::BTreeMap;

use base64::Engine;
use bifrost_command::CanonicalQueryCommand;
use bifrost_core::{BifrostError, Result};
use bifrost_storage::DEFAULT_REMOTE_BASE_URL;
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, CHACHA20_POLY1305};
use ring::hkdf::{Salt, HKDF_SHA256};
use ring::rand::SecureRandom;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) fn is_false(value: &bool) -> bool {
    !*value
}

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantMode {
    Once,
    #[serde(rename = "30m")]
    ThirtyMinutes,
    #[serde(rename = "1h")]
    OneHour,
    #[serde(rename = "1d")]
    OneDay,
    Permanent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantStatus {
    Active,
    Expired,
    Revoked,
    Consumed,
    Removed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    PairCode,
    SshPublickey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GrantScope {
    #[default]
    #[serde(rename = "remote_query")]
    RemoteQuery,
    #[serde(rename = "remote_shell_exec")]
    RemoteShellExec,
    #[serde(rename = "remote_shell_interactive")]
    RemoteShellInteractive,
    #[serde(rename = "remote_power_mgmt")]
    RemotePowerMgmt,
    #[serde(rename = "remote_im_gateway")]
    RemoteImGateway,
}

impl GrantScope {
    /// Check whether this shell-level scope allows the given command kind.
    /// File commands are handled by [`FileAccessScope`] via [`scope_allows_command`].
    pub fn allows_command(self, kind: CommandKind) -> bool {
        matches!(
            (self, kind),
            (_, CommandKind::QueryReadonly)
                | (Self::RemoteShellExec, CommandKind::ShellExec)
                | (Self::RemoteShellInteractive, CommandKind::ShellExec)
                | (Self::RemoteShellInteractive, CommandKind::PowerMgmt)
                | (Self::RemotePowerMgmt, CommandKind::PowerMgmt)
                | (Self::RemoteImGateway, CommandKind::ImGateway)
        )
    }
}

/// Independent file access level, orthogonal to [`GrantScope`].
/// This allows shell and file permissions to coexist on the same grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FileAccessScope {
    #[default]
    #[serde(rename = "none")]
    None,
    #[serde(rename = "read")]
    Read,
    #[serde(rename = "read_write")]
    ReadWrite,
}

impl FileAccessScope {
    /// Check whether this file access level allows the given command kind.
    pub fn allows_command(self, kind: CommandKind) -> bool {
        match kind {
            CommandKind::File => matches!(self, Self::Read | Self::ReadWrite),
            _ => false,
        }
    }

    /// Layered permission model default: shell-capable scopes implicitly grant
    /// file read/write, while query-only scopes grant no file access.
    ///
    /// Single source of truth for the "Shell* -> ReadWrite" migration rule;
    /// callers that materialize a `GrantInfo` from legacy/partial data should
    /// go through this helper instead of re-implementing the match.
    pub fn default_for(scope: GrantScope) -> Self {
        match scope {
            GrantScope::RemoteShellExec | GrantScope::RemoteShellInteractive => Self::ReadWrite,
            GrantScope::RemotePowerMgmt => Self::None,
            GrantScope::RemoteQuery => Self::None,
            GrantScope::RemoteImGateway => Self::None,
        }
    }
}

/// Combined permission check: grant_scope handles shell/query, file_access handles file.
pub fn scope_allows_command(
    grant_scope: GrantScope,
    file_access: FileAccessScope,
    kind: CommandKind,
) -> bool {
    match kind {
        CommandKind::File => file_access.allows_command(kind),
        _ => grant_scope.allows_command(kind),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CommandKind {
    #[default]
    #[serde(rename = "query.readonly")]
    QueryReadonly,
    #[serde(rename = "shell.exec")]
    ShellExec,
    #[serde(rename = "file")]
    File,
    #[serde(rename = "power.mgmt")]
    PowerMgmt,
    #[serde(rename = "im.gateway")]
    ImGateway,
}

impl CommandKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::QueryReadonly => "query.readonly",
            Self::ShellExec => "shell.exec",
            Self::File => "file",
            Self::PowerMgmt => "power.mgmt",
            Self::ImGateway => "im.gateway",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellExecMode {
    Template,
    ArgvExec,
    ShellText,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StdinMode {
    None,
    Inline,
    Stream,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputMode {
    SplitStreams,
    PtyMerged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallStatus {
    Pending,
    Authorized,
    KeyExchanged,
    Streaming,
    Completed,
    Failed,
    Cancelled,
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingStatus {
    Created,
    CodeVerified,
    PendingApproval,
    Approved,
    Rejected,
    Expired,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameDirection {
    #[default]
    CallerToClient,
    ClientToCaller,
}

impl FrameDirection {
    /// P1-A counter nonce: 4-byte direction prefix.
    /// Both peers share ONE session key, so the direction MUST be encoded into
    /// the nonce to guarantee caller->client and client->caller never collide
    /// on the same (key, nonce) pair.
    pub fn nonce_prefix(self) -> [u8; 4] {
        match self {
            FrameDirection::CallerToClient => [0, 0, 0, 0],
            FrameDirection::ClientToCaller => [0, 0, 0, 1],
        }
    }
}

/// P1-A deterministic counter nonce = 4-byte direction prefix || 8-byte seq (BE).
/// Replaces the previous random 12-byte nonce. Because both peers share the same
/// session key, uniqueness is guaranteed by (direction, seq) rather than by RNG,
/// eliminating the birthday-bound nonce-reuse risk of random nonces.
fn counter_nonce(direction: FrameDirection, seq: u64) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[..4].copy_from_slice(&direction.nonce_prefix());
    nonce[4..].copy_from_slice(&seq.to_be_bytes());
    nonce
}

pub fn verify_encrypted_payload_counter_nonce(
    payload: &EncryptedPayload,
    direction: FrameDirection,
    seq: u64,
) -> Result<()> {
    let engine = base64::engine::general_purpose::STANDARD;
    let nonce_bytes = engine.decode(&payload.nonce).map_err(|e| {
        BifrostError::Config(format!("invalid encrypted payload counter nonce: {e}"))
    })?;
    if nonce_bytes.len() != 12 {
        return Err(BifrostError::Config(format!(
            "invalid encrypted payload counter nonce length: {}",
            nonce_bytes.len()
        )));
    }
    let expected = counter_nonce(direction, seq);
    if nonce_bytes.as_slice() != expected {
        return Err(BifrostError::Config(
            "encrypted payload counter nonce does not match envelope direction/seq".to_string(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantDecision {
    Approve,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientSseEventKind {
    ClientHelloAck,
    PairingRequest,
    GrantCreated,
    CallOpen,
    CallFrame,
    CallCancel,
    GrantRevoked,
    SshConnect,
    Ping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallerSseEventKind {
    Status,
    Frame,
    Exit,
    Error,
    Heartbeat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerState {
    #[default]
    Disconnected,
    Registering,
    Connecting,
    Connected,
    Reconnecting,
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteInvokeConfig {
    #[serde(default)]
    pub enabled: bool,
    pub relay_url: String,
    #[serde(default = "default_sse_keepalive_ms")]
    pub sse_keepalive_ms: u64,
    #[serde(default = "default_pair_code_ttl_secs")]
    pub pair_code_ttl_secs: u64,
    #[serde(default = "default_max_active_calls")]
    pub max_active_calls: u32,
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
    #[serde(default = "default_max_records")]
    pub max_records: u32,
}

fn default_sse_keepalive_ms() -> u64 {
    30_000
}

fn default_pair_code_ttl_secs() -> u64 {
    120
}

fn default_max_active_calls() -> u32 {
    10
}

fn default_retention_days() -> u32 {
    90
}

fn default_max_records() -> u32 {
    1_000
}

impl Default for RemoteInvokeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            relay_url: DEFAULT_REMOTE_BASE_URL.to_string(),
            sse_keepalive_ms: default_sse_keepalive_ms(),
            pair_code_ttl_secs: default_pair_code_ttl_secs(),
            max_active_calls: default_max_active_calls(),
            retention_days: default_retention_days(),
            max_records: default_max_records(),
        }
    }
}

// ---------------------------------------------------------------------------
// Client Identity
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientIdentity {
    pub instance_id: String,
    pub device_name: String,
    pub platform: String,
    pub long_term_pubkey: String,
    pub long_term_pubkey_hash: String,
}

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CallerInfo {
    #[serde(default)]
    pub fingerprint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller_pubkey: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RemoteCommand {
    #[serde(default)]
    pub kind: CommandKind,
    #[serde(default)]
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args_json: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<CanonicalQueryCommand>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exec_mode: Option<ShellExecMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argv: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        alias = "shell_text",
        alias = "shell_command"
    )]
    pub command_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdin_mode: Option<StdinMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "crate::remote_invoke::types::is_false")]
    pub login: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pty: Option<RemotePtyRequest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_mode: Option<OutputMode>,
    #[serde(skip)]
    pub grant_id: Option<String>,
    /// Caller fingerprint snapshotted from the grant; used for file-policy matching.
    #[serde(skip)]
    pub caller_fingerprint: Option<String>,
    /// SSH key fingerprint snapshotted from the grant (if SSH-authenticated).
    #[serde(skip)]
    pub ssh_fingerprint: Option<String>,
    /// The grant's file_access scope, injected by the worker before execution.
    /// Used by the executor to reject write ops when the grant only allows read.
    #[serde(skip)]
    pub file_access: FileAccessScope,
}

impl RemoteCommand {
    pub fn summary_label(&self) -> &str {
        if let Some(query) = &self.query {
            return query.command_id();
        }

        if !self.command.is_empty() {
            return self.command.as_str();
        }

        match self.kind {
            CommandKind::QueryReadonly => "query.readonly",
            CommandKind::ShellExec => "shell.exec",
            CommandKind::File => "file",
            CommandKind::PowerMgmt => "power.mgmt",
            CommandKind::ImGateway => "im.gateway",
        }
    }

    pub fn summary_args_json(&self) -> Option<String> {
        if let Some(args_json) = self
            .args_json
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(args_json.to_string());
        }

        match &self.query {
            Some(CanonicalQueryCommand::Search(args)) => serde_json::to_string(args).ok(),
            Some(CanonicalQueryCommand::TrafficList(args)) => serde_json::to_string(args).ok(),
            Some(CanonicalQueryCommand::TrafficGet(args)) => serde_json::to_string(args).ok(),
            Some(CanonicalQueryCommand::TrafficClear(args)) => serde_json::to_string(args).ok(),
            None => None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CommandSummary {
    #[serde(default)]
    pub command_preview: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub masked_args_json: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload_size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedEnvelope {
    #[serde(default = "default_encrypted_envelope_version")]
    pub version: u32,
    pub call_id: String,
    pub seq: u64,
    #[serde(default)]
    pub direction: FrameDirection,
    pub nonce: String,
    pub ciphertext: String,
    pub tag: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aad: Option<EnvelopeAad>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvelopeAad {
    #[serde(default = "default_encrypted_envelope_version")]
    pub version: u32,
    #[serde(default)]
    pub call_id: String,
    #[serde(default)]
    pub seq: u64,
    pub direction: FrameDirection,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_kind: Option<CommandKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_scope: Option<GrantScope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender_key_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

impl EncryptedEnvelope {
    pub fn v2(
        call_id: String,
        seq: u64,
        direction: FrameDirection,
        ciphertext: String,
        frame_type: Option<String>,
        command_kind: Option<CommandKind>,
        grant_scope: Option<GrantScope>,
    ) -> Self {
        let aad = EnvelopeAad {
            version: default_encrypted_envelope_version(),
            call_id: call_id.clone(),
            seq,
            direction,
            token_hash: None,
            frame_type,
            command_kind,
            grant_scope,
            sender_key_id: None,
            metadata: None,
        };

        Self {
            version: default_encrypted_envelope_version(),
            call_id,
            seq,
            direction,
            nonce: String::new(),
            ciphertext,
            tag: String::new(),
            aad: Some(aad),
        }
    }
}

fn default_encrypted_envelope_version() -> u32 {
    2
}

const E2E_HKDF_INFO_PREFIX: &[u8] = b"bifrost-e2e-v1";
const OPEN_CALL_HKDF_INFO_PREFIX: &[u8] = b"bifrost-open-call-v2";

pub fn derive_open_call_session_key(
    shared_secret: &[u8],
    _grant_id: &str,
    caller_ephemeral_pub: Option<&str>,
    client_ephemeral_pub: Option<&str>,
    command_kind: CommandKind,
    // P0-A #4 channel binding: long-term key fingerprints of both peers.
    // Empty string == no binding (legacy). Caller and target MUST pass the
    // SAME values in the SAME order or the derived key will not match.
    caller_long_term_fp: &str,
    client_long_term_fp: &str,
) -> Result<[u8; 32]> {
    // v5 callers no longer see grant_id, so the open-call key schedule cannot
    // depend on it. The grant_id parameter is kept for source compatibility.
    let salt = Salt::new(HKDF_SHA256, b"bifrost-open-call-v5");
    let prk = salt.extract(shared_secret);

    let caller_pub = decode_base64_or_raw(caller_ephemeral_pub.unwrap_or_default());
    let client_pub = decode_base64_or_raw(client_ephemeral_pub.unwrap_or_default());
    let info_parts: [&[u8]; 6] = [
        OPEN_CALL_HKDF_INFO_PREFIX,
        caller_pub.as_slice(),
        client_pub.as_slice(),
        command_kind.as_str().as_bytes(),
        caller_long_term_fp.as_bytes(),
        client_long_term_fp.as_bytes(),
    ];

    let okm = prk.expand(&info_parts, SessionKeyLen).map_err(|_| {
        BifrostError::Config("expand remote invoke open_call key failed".to_string())
    })?;

    let mut session_key = [0u8; 32];
    okm.fill(&mut session_key)
        .map_err(|_| BifrostError::Config("fill remote invoke open_call key failed".to_string()))?;
    Ok(session_key)
}

pub fn derive_call_session_key(
    shared_secret: &[u8],
    call_id: &str,
    caller_ephemeral_pub: Option<&str>,
    client_ephemeral_pub: Option<&str>,
    // P0-A #4 channel binding: see derive_open_call_session_key.
    caller_long_term_fp: &str,
    client_long_term_fp: &str,
) -> Result<[u8; 32]> {
    let salt = Salt::new(HKDF_SHA256, call_id.as_bytes());
    let prk = salt.extract(shared_secret);

    let caller_pub = decode_base64_or_raw(caller_ephemeral_pub.unwrap_or_default());
    let client_pub = decode_base64_or_raw(client_ephemeral_pub.unwrap_or_default());
    let info_parts: [&[u8]; 5] = [
        E2E_HKDF_INFO_PREFIX,
        caller_pub.as_slice(),
        client_pub.as_slice(),
        caller_long_term_fp.as_bytes(),
        client_long_term_fp.as_bytes(),
    ];
    let okm = prk
        .expand(&info_parts, SessionKeyLen)
        .map_err(|_| BifrostError::Config("expand remote invoke session key failed".to_string()))?;

    let mut session_key = [0u8; 32];
    okm.fill(&mut session_key)
        .map_err(|_| BifrostError::Config("fill remote invoke session key failed".to_string()))?;
    Ok(session_key)
}

pub fn decrypt_remote_command_payload(
    payload: &EncryptedPayload,
    session_key: &[u8; 32],
    fallback_aad: EnvelopeAad,
) -> Result<RemoteCommand> {
    if payload.aad.is_none() {
        return Err(BifrostError::Config(
            "encrypted remote command payload requires authenticated AAD".to_string(),
        ));
    }
    decrypt_encrypted_payload(payload, session_key, fallback_aad)
}

pub fn encrypt_encrypted_payload<T>(
    payload: &T,
    session_key: &[u8; 32],
    aad: &EnvelopeAad,
) -> Result<EncryptedPayload>
where
    T: Serialize,
{
    let plaintext = serde_json::to_vec(payload)
        .map_err(|e| BifrostError::Config(format!("encode encrypted payload json: {}", e)))?;
    let aad_json = serde_json::to_vec(aad)
        .map_err(|e| BifrostError::Config(format!("serialize encrypted payload aad: {}", e)))?;

    let unbound = UnboundKey::new(&CHACHA20_POLY1305, session_key)
        .map_err(|_| BifrostError::Config("build remote invoke encrypt key failed".to_string()))?;
    let key = LessSafeKey::new(unbound);

    // P1-A: deterministic counter nonce (direction || seq) instead of random.
    // Still transmitted in payload.nonce so the peer decrypt path is unchanged.
    let nonce_bytes = counter_nonce(aad.direction, aad.seq);

    let mut in_out = plaintext;
    let tag = key
        .seal_in_place_separate_tag(
            Nonce::assume_unique_for_key(nonce_bytes),
            Aad::from(aad_json),
            &mut in_out,
        )
        .map_err(|_| BifrostError::Config("encrypt remote invoke payload failed".to_string()))?;

    let engine = base64::engine::general_purpose::STANDARD;
    Ok(EncryptedPayload {
        version: aad.version,
        nonce: engine.encode(nonce_bytes),
        ciphertext: engine.encode(in_out),
        tag: engine.encode(tag.as_ref()),
        aad: Some(aad.clone()),
    })
}

pub fn encrypt_encrypted_payload_without_aad<T>(
    payload: &T,
    session_key: &[u8; 32],
    version: u32,
) -> Result<EncryptedPayload>
where
    T: Serialize,
{
    let plaintext = serde_json::to_vec(payload)
        .map_err(|e| BifrostError::Config(format!("encode encrypted payload json: {}", e)))?;

    let unbound = UnboundKey::new(&CHACHA20_POLY1305, session_key)
        .map_err(|_| BifrostError::Config("build remote invoke encrypt key failed".to_string()))?;
    let key = LessSafeKey::new(unbound);

    let mut nonce_bytes = [0u8; 12];
    ring::rand::SystemRandom::new()
        .fill(&mut nonce_bytes)
        .map_err(|_| BifrostError::Config("generate encrypted payload nonce failed".to_string()))?;

    let mut in_out = plaintext;
    let tag = key
        .seal_in_place_separate_tag(
            Nonce::assume_unique_for_key(nonce_bytes),
            Aad::empty(),
            &mut in_out,
        )
        .map_err(|_| BifrostError::Config("encrypt remote invoke payload failed".to_string()))?;

    let engine = base64::engine::general_purpose::STANDARD;
    Ok(EncryptedPayload {
        version,
        nonce: engine.encode(nonce_bytes),
        ciphertext: engine.encode(in_out),
        tag: engine.encode(tag.as_ref()),
        aad: None,
    })
}

/// P1-A: counter-nonce variant of the no-AAD frame encryptor.
///
/// Identical wire format to `encrypt_encrypted_payload_without_aad` (empty AEAD
/// AAD, nonce transmitted in `payload.nonce`), but the nonce is a deterministic
/// `direction || seq` counter instead of a random value. Because both peers
/// share ONE call session key, encoding the direction in the nonce prefix keeps
/// the caller->client and client->caller nonce spaces disjoint, and the seq
/// keeps each direction collision-free. The peer decrypt path is unchanged: it
/// still reads the transmitted nonce, so this is fully wire-compatible.
///
/// CALLER CONTRACT: for a given (session_key, direction) the caller MUST pass a
/// strictly unique `seq`. Frames use seq = 1.. and call-exit uses seq = 0.
pub fn encrypt_encrypted_payload_counter<T>(
    payload: &T,
    session_key: &[u8; 32],
    version: u32,
    direction: FrameDirection,
    seq: u64,
) -> Result<EncryptedPayload>
where
    T: Serialize,
{
    let plaintext = serde_json::to_vec(payload)
        .map_err(|e| BifrostError::Config(format!("encode encrypted payload json: {}", e)))?;

    let unbound = UnboundKey::new(&CHACHA20_POLY1305, session_key)
        .map_err(|_| BifrostError::Config("build remote invoke encrypt key failed".to_string()))?;
    let key = LessSafeKey::new(unbound);

    let nonce_bytes = counter_nonce(direction, seq);

    let mut in_out = plaintext;
    let tag = key
        .seal_in_place_separate_tag(
            Nonce::assume_unique_for_key(nonce_bytes),
            Aad::empty(),
            &mut in_out,
        )
        .map_err(|_| BifrostError::Config("encrypt remote invoke payload failed".to_string()))?;

    let engine = base64::engine::general_purpose::STANDARD;
    Ok(EncryptedPayload {
        version,
        nonce: engine.encode(nonce_bytes),
        ciphertext: engine.encode(in_out),
        tag: engine.encode(tag.as_ref()),
        aad: None,
    })
}

fn decrypt_encrypted_payload<T>(
    payload: &EncryptedPayload,
    session_key: &[u8; 32],
    fallback_aad: EnvelopeAad,
) -> Result<T>
where
    T: DeserializeOwned,
{
    let engine = base64::engine::general_purpose::STANDARD;
    let nonce_bytes = engine
        .decode(&payload.nonce)
        .map_err(|e| BifrostError::Config(format!("invalid encrypted payload nonce: {}", e)))?;
    if nonce_bytes.len() != 12 {
        return Err(BifrostError::Config(format!(
            "invalid encrypted payload nonce length: {}",
            nonce_bytes.len()
        )));
    }

    let ciphertext = engine.decode(&payload.ciphertext).map_err(|e| {
        BifrostError::Config(format!("invalid encrypted payload ciphertext: {}", e))
    })?;
    let tag = engine
        .decode(&payload.tag)
        .map_err(|e| BifrostError::Config(format!("invalid encrypted payload tag: {}", e)))?;
    if tag.len() != 16 {
        return Err(BifrostError::Config(format!(
            "invalid encrypted payload tag length: {}",
            tag.len()
        )));
    }

    let aad_payload = payload.aad.clone().unwrap_or(fallback_aad);
    let aad_json = serde_json::to_vec(&aad_payload)
        .map_err(|e| BifrostError::Config(format!("serialize encrypted payload aad: {}", e)))?;

    let unbound = UnboundKey::new(&CHACHA20_POLY1305, session_key)
        .map_err(|_| BifrostError::Config("build remote invoke decrypt key failed".to_string()))?;
    let key = LessSafeKey::new(unbound);

    let mut in_out = ciphertext;
    in_out.extend_from_slice(&tag);

    let mut nonce_array = [0u8; 12];
    nonce_array.copy_from_slice(&nonce_bytes);
    let plaintext = key
        .open_in_place(
            Nonce::assume_unique_for_key(nonce_array),
            Aad::from(aad_json),
            &mut in_out,
        )
        .map_err(|_| BifrostError::Config("encrypted payload authentication failed".to_string()))?;

    serde_json::from_slice(plaintext)
        .map_err(|e| BifrostError::Config(format!("decode remote invoke payload json: {}", e)))
}

pub fn decrypt_encrypted_payload_without_aad<T>(
    payload: &EncryptedPayload,
    session_key: &[u8; 32],
) -> Result<T>
where
    T: DeserializeOwned,
{
    let engine = base64::engine::general_purpose::STANDARD;
    let nonce_bytes = engine
        .decode(&payload.nonce)
        .map_err(|e| BifrostError::Config(format!("invalid encrypted payload nonce: {}", e)))?;
    if nonce_bytes.len() != 12 {
        return Err(BifrostError::Config(format!(
            "invalid encrypted payload nonce length: {}",
            nonce_bytes.len()
        )));
    }

    let ciphertext = engine.decode(&payload.ciphertext).map_err(|e| {
        BifrostError::Config(format!("invalid encrypted payload ciphertext: {}", e))
    })?;
    let tag = engine
        .decode(&payload.tag)
        .map_err(|e| BifrostError::Config(format!("invalid encrypted payload tag: {}", e)))?;
    if tag.len() != 16 {
        return Err(BifrostError::Config(format!(
            "invalid encrypted payload tag length: {}",
            tag.len()
        )));
    }

    let unbound = UnboundKey::new(&CHACHA20_POLY1305, session_key)
        .map_err(|_| BifrostError::Config("build remote invoke decrypt key failed".to_string()))?;
    let key = LessSafeKey::new(unbound);

    let mut in_out = ciphertext;
    in_out.extend_from_slice(&tag);

    let mut nonce_array = [0u8; 12];
    nonce_array.copy_from_slice(&nonce_bytes);
    let plaintext = key
        .open_in_place(
            Nonce::assume_unique_for_key(nonce_array),
            Aad::empty(),
            &mut in_out,
        )
        .map_err(|_| BifrostError::Config("encrypted payload authentication failed".to_string()))?;

    serde_json::from_slice(plaintext)
        .map_err(|e| BifrostError::Config(format!("decode remote invoke payload json: {}", e)))
}

fn decode_base64_or_raw(value: &str) -> Vec<u8> {
    if value.is_empty() {
        return Vec::new();
    }

    let engine = base64::engine::general_purpose::STANDARD;
    match engine.decode(value) {
        Ok(decoded) if !decoded.is_empty() => decoded,
        _ => value.as_bytes().to_vec(),
    }
}

struct SessionKeyLen;

impl ring::hkdf::KeyType for SessionKeyLen {
    fn len(&self) -> usize {
        32
    }
}

// ---------------------------------------------------------------------------
// SSE event payloads (Relay -> Client)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientSseEvent {
    ClientHelloAck {
        stream_id: String,
        server_time: u64,
    },
    PairingRequest {
        pairing_id: String,
        caller_info: CallerInfo,
        command_summary: CommandSummary,
        command: Box<RemoteCommand>,
        caller_pubkey: String,
        client_ephemeral_pub: Option<String>,
        caller_ephemeral_pub: Option<String>,
    },
    GrantCreated {
        grant_id: String,
        call_id: String,
        relay_token: String,
        caller_ephemeral_pub: String,
        client_ephemeral_pub: Option<String>,
        grant_mode: GrantMode,
        #[serde(default)]
        grant_scope: GrantScope,
        #[serde(default)]
        file_access: FileAccessScope,
        #[serde(skip_serializing_if = "Option::is_none")]
        policy_binding: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        shell_policy_set_version_snapshot: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        interactive_allowed: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        stdin_allowed: Option<bool>,
        expires_at: Option<u64>,
    },
    CallOpen {
        call_id: String,
        grant_id: String,
        caller_ephemeral_pub: String,
        command_summary: CommandSummary,
        #[serde(default)]
        command_kind: CommandKind,
        command_encrypted: EncryptedPayload,
        #[serde(skip_serializing_if = "Option::is_none")]
        call_meta: Option<CallMeta>,
    },
    CallFrame {
        call_id: String,
        envelope: EncryptedEnvelope,
    },
    CallCancel {
        call_id: String,
        reason: Option<String>,
    },
    GrantRevoked {
        grant_id: String,
        reason: Option<String>,
    },
    SshConnect {
        connect_id: String,
        device_code: String,
        ssh_key_fingerprint: String,
        caller_info: Option<CallerInfo>,
        relay_verified: bool,
    },
    Ping {
        server_time: u64,
    },
}

// ---------------------------------------------------------------------------
// Pairing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingRequest {
    pub pairing_id: String,
    pub caller_info: CallerInfo,
    pub command_summary: CommandSummary,
    pub command: RemoteCommand,
    pub caller_pubkey: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_ephemeral_pub: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller_ephemeral_pub: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrantInfo {
    pub grant_id: String,
    pub client_instance_id: String,
    pub caller_fingerprint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller_display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub grant_mode: GrantMode,
    #[serde(default)]
    pub grant_scope: GrantScope,
    #[serde(default)]
    pub file_access: FileAccessScope,
    #[serde(default = "default_auth_method")]
    pub auth_method: AuthMethod,
    pub status: GrantStatus,
    pub first_authorized_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_command_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<u64>,
    pub max_calls: Option<u32>,
    pub remaining_calls: Option<u32>,
    #[serde(default)]
    pub use_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssh_key_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssh_key_fingerprint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller_ephemeral_pub: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_ephemeral_pub: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_binding: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell_policy_set_version_snapshot: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interactive_allowed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdin_allowed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallInfo {
    pub call_id: String,
    pub grant_id: String,
    pub pairing_id: Option<String>,
    pub client_instance_id: String,
    pub caller_fingerprint: String,
    #[serde(default = "default_auth_method")]
    pub auth_method: AuthMethod,
    #[serde(default)]
    pub command_kind: CommandKind,
    pub status: CallStatus,
    pub command_summary: CommandSummary,
    pub command: RemoteCommand,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller_display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub started_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_in: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_out: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssh_key_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssh_key_fingerprint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exec_mode: Option<ShellExecMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_mode: Option<OutputMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pty_enabled: Option<bool>,
}

// ---------------------------------------------------------------------------
// Discovery session
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverySession {
    pub session_id: String,
    pub pair_code: String,
    pub expires_at: u64,
    pub created_at: u64,
}

// ---------------------------------------------------------------------------
// Client -> Relay HTTP request bodies
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientRegistrationRequest {
    pub challenge_id: String,
    pub client_instance_id: String,
    pub client_long_term_pubkey: String,
    pub device_name: String,
    pub platform: String,
    pub bifrost_version: String,
    pub signature: String,
    pub timestamp: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_device_route: Option<Option<SshDeviceRoute>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientRegistrationResponse {
    pub client_auth_token: String,
    pub expires_at: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientRegistrationChallengeRequest {
    pub client_instance_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientRegistrationChallengeResponse {
    pub challenge_id: String,
    pub challenge: String,
    pub expires_at: u64,
    pub algorithm: String,
}

#[allow(clippy::too_many_arguments)]
pub fn build_registration_signature_payload(
    challenge_id: &str,
    challenge: &str,
    client_instance_id: &str,
    device_name: &str,
    platform: &str,
    bifrost_version: &str,
    client_long_term_pubkey: &str,
    timestamp: u64,
) -> String {
    serde_json::json!([
        "bifrost-remote-register-v1",
        challenge_id,
        challenge,
        client_instance_id,
        device_name,
        platform,
        bifrost_version,
        client_long_term_pubkey,
        timestamp,
    ])
    .to_string()
}

// ---------------------------------------------------------------------------
// P0-A: ephemeral-key handshake authentication + long-term key pinning.
//
// Both peers sign their own X25519 ephemeral public key with their Ed25519
// long-term private key. The peer verifies the signature with the long-term
// public key carried alongside it, then pins that long-term key by fingerprint.
// This defeats a relay-in-the-middle that swaps ephemeral keys: it cannot
// forge a signature without the long-term private key.
// ---------------------------------------------------------------------------

/// Domain-separation tag for the ephemeral-key authentication signature.
pub const EPHEMERAL_SIG_DOMAIN: &str = "bifrost-remote-ephemeral-v1";

/// Canonical JSON payload that each peer signs over its own ephemeral pubkey.
/// The field order is FIXED and MUST be identical on caller and target.
pub fn build_ephemeral_signature_payload(
    binding_id: &str,
    signer_instance_id: &str,
    peer_fingerprint: &str,
    ephemeral_pub_b64: &str,
    timestamp: u64,
) -> String {
    serde_json::json!([
        EPHEMERAL_SIG_DOMAIN,
        binding_id,
        signer_instance_id,
        peer_fingerprint,
        ephemeral_pub_b64,
        timestamp,
    ])
    .to_string()
}

/// SHA-256 fingerprint of a long-term public key (base64 SPKI DER input),
/// formatted identically to `identity::sha256_fingerprint` (`sha256:` prefix).
pub fn fingerprint_long_term_pubkey(long_term_pubkey_b64: &str) -> Result<String> {
    let engine = base64::engine::general_purpose::STANDARD;
    let der = engine
        .decode(long_term_pubkey_b64)
        .map_err(|e| BifrostError::Config(format!("invalid long-term pubkey encoding: {e}")))?;
    Ok(sha256_fingerprint_hex(&der))
}

fn sha256_fingerprint_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(data);
    let mut out = String::with_capacity(digest.len() * 2 + 7);
    out.push_str("sha256:");
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Verify an ephemeral-key signature and enforce long-term-key pinning.
///
/// P0-A is FAIL-CLOSED: if `signature`/`long_term_pubkey` are absent the caller
/// (see wiring in worker.rs / cli) decides policy, but whenever they ARE present
/// they MUST verify, and if `pinned_fingerprint` is Some it MUST match. Any
/// mismatch or malformed input returns Err and the session is refused.
pub fn verify_ephemeral_signature(
    long_term_pubkey_b64: &str,
    payload: &[u8],
    signature_b64: &str,
    pinned_fingerprint: Option<&str>,
) -> Result<()> {
    use ring::signature::{UnparsedPublicKey, ED25519};
    let engine = base64::engine::general_purpose::STANDARD;
    let der = engine
        .decode(long_term_pubkey_b64)
        .map_err(|e| BifrostError::Config(format!("invalid signer long-term pubkey: {e}")))?;
    if der.len() < 13 {
        return Err(BifrostError::Config(
            "signer long-term pubkey too short for SPKI DER".to_string(),
        ));
    }
    // Ed25519 SPKI DER prefix is 12 bytes; the raw key follows.
    let raw = &der[12..];
    let signature = engine
        .decode(signature_b64)
        .map_err(|e| BifrostError::Config(format!("invalid ephemeral signature: {e}")))?;
    UnparsedPublicKey::new(&ED25519, raw)
        .verify(payload, &signature)
        .map_err(|_| {
            BifrostError::Config("ephemeral key signature verification failed".to_string())
        })?;
    if let Some(pinned) = pinned_fingerprint {
        let actual = sha256_fingerprint_hex(&der);
        if actual != pinned {
            return Err(BifrostError::Config(format!(
                "long-term key fingerprint mismatch (pinned={pinned}, got={actual})"
            )));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishPairCodeRequest {
    pub client_instance_id: String,
    pub pair_code: String,
    pub expires_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discovery_session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientHeartbeatRequest {
    pub client_instance_id: String,
    pub stream_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub active_call_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_device_route: Option<Option<SshDeviceRoute>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrantDecisionRequest {
    pub pairing_id: String,
    pub client_instance_id: String,
    pub decision: GrantDecision,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_mode: Option<GrantMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_scope: Option<GrantScope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_access: Option<FileAccessScope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_ephemeral_pub: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateGrantRequest {
    pub client_instance_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_scope: Option<GrantScope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_access: Option<FileAccessScope>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientCallFrameRequest {
    pub call_id: String,
    pub client_instance_id: String,
    pub envelope_json: String,
}

/// PR#6c: wire payload for POST /v4/remote-invoke/client/calls/:id/stream-frame.
/// `frame_json` is the canonical JSON serialization of a `StreamFrame`
/// (see `stream_emit::frame_to_json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientCallStreamFrameRequest {
    pub call_id: String,
    pub client_instance_id: String,
    pub frame_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientCallExitRequest {
    pub call_id: String,
    pub client_instance_id: String,
    pub exit_code: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_in: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_out: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_encrypted: Option<EncryptedPayload>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshDeviceRoute {
    pub device_code: String,
    pub public_key_pem: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshConnectEvent {
    pub connect_id: String,
    pub device_code: String,
    pub ssh_key_fingerprint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller_ephemeral_pub: Option<String>,
    #[serde(default)]
    pub caller_info: Option<CallerInfo>,
    #[serde(default)]
    pub relay_verified: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SshConnectResultStatus {
    Approved,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshConnectResultRequest {
    pub connect_id: String,
    pub status: SshConnectResultStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller_fingerprint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssh_key_fingerprint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_mode: Option<GrantMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_scope: Option<GrantScope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_access: Option<FileAccessScope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller_ephemeral_pub: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_ephemeral_pub: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemotePtyRequest {
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cols: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedPayload {
    #[serde(default = "default_encrypted_envelope_version")]
    pub version: u32,
    pub nonce: String,
    pub ciphertext: String,
    pub tag: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aad: Option<EnvelopeAad>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallMeta {
    #[serde(default)]
    pub command_kind: CommandKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pty_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_mode: Option<OutputMode>,
}

// ---------------------------------------------------------------------------
// Relay API response helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteInvokeRequest {
    pub command: RemoteCommand,
    pub command_summary: CommandSummary,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RemoteInvokeResponse {
    #[serde(default)]
    pub exit_code: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
    /// SHA-1 digest of the (possibly truncated) inline  field.
    /// Kept for backward compatibility with legacy clients.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_digest: Option<String>,
    #[serde(default)]
    pub duration_ms: u64,
    // PR#2 large-output fields (all optional, additive, backward compatible):
    /// Total byte count of the full stdout stream as seen by the executor.
    /// When present and greater than , the
    /// inline field was truncated by the executor's output cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_total_bytes: Option<u64>,
    /// Same for stderr.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_total_bytes: Option<u64>,
    /// Hex-encoded SHA-256 of the **full** stdout bytes (not the truncated
    /// inline preview). Callers MUST use this for end-to-end verification
    /// when reassembling stdout from streamed chunks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_sha256_full: Option<String>,
    /// Same for stderr.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_sha256_full: Option<String>,
    /// True iff the executor truncated either stream when building the
    /// inline preview. Callers may use this as a hint to fall back to
    /// streamed chunks or a side-channel download.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_truncated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_truncated: Option<bool>,
}

// ---------------------------------------------------------------------------
// Allowed commands
// ---------------------------------------------------------------------------

pub const ALLOWED_COMMANDS: &[&str] = &["status", "search.stream", "traffic.list", "traffic.get"];

pub fn is_allowed_command(command: &str) -> bool {
    ALLOWED_COMMANDS.contains(&command)
}

pub fn grant_mode_ttl_ms(mode: GrantMode) -> Option<u64> {
    match mode {
        GrantMode::Once => None,
        GrantMode::ThirtyMinutes => Some(30 * 60 * 1000),
        GrantMode::OneHour => Some(60 * 60 * 1000),
        GrantMode::OneDay => Some(24 * 60 * 60 * 1000),
        GrantMode::Permanent => None,
    }
}

fn default_auth_method() -> AuthMethod {
    AuthMethod::PairCode
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, CHACHA20_POLY1305};

    #[test]
    fn test_remote_invoke_default_max_records_is_capped_at_1000() {
        assert_eq!(RemoteInvokeConfig::default().max_records, 1_000);
    }

    #[test]
    fn test_grant_scope_rejects_legacy_remote_invoke_string() {
        let scope = serde_json::from_value::<GrantScope>(serde_json::Value::String(
            "remote_invoke".to_string(),
        ));
        assert!(scope.is_err());
    }

    #[test]
    fn test_file_access_scope_allows_file_commands() {
        // FileAccessScope::None blocks file commands
        assert!(!FileAccessScope::None.allows_command(CommandKind::File));
        // FileAccessScope::Read allows file commands
        assert!(FileAccessScope::Read.allows_command(CommandKind::File));
        // FileAccessScope::ReadWrite allows file commands
        assert!(FileAccessScope::ReadWrite.allows_command(CommandKind::File));
        // File access does not affect non-file commands
        assert!(!FileAccessScope::ReadWrite.allows_command(CommandKind::ShellExec));
        assert!(!FileAccessScope::ReadWrite.allows_command(CommandKind::QueryReadonly));
    }

    #[test]
    fn test_grant_scope_allows_query_for_all_levels() {
        assert!(GrantScope::RemoteQuery.allows_command(CommandKind::QueryReadonly));
        assert!(GrantScope::RemoteShellExec.allows_command(CommandKind::QueryReadonly));
        assert!(GrantScope::RemoteShellInteractive.allows_command(CommandKind::QueryReadonly));
    }

    #[test]
    fn test_grant_scope_does_not_allow_file_for_shell_scopes() {
        // Shell scopes should NOT allow file commands (file_access handles that).
        assert!(!GrantScope::RemoteQuery.allows_command(CommandKind::File));
        assert!(!GrantScope::RemoteShellExec.allows_command(CommandKind::File));
        assert!(!GrantScope::RemoteShellInteractive.allows_command(CommandKind::File));
    }

    #[test]
    fn test_scope_allows_command_combined() {
        // Shell interactive + file read_write = full access
        assert!(scope_allows_command(
            GrantScope::RemoteShellInteractive,
            FileAccessScope::ReadWrite,
            CommandKind::QueryReadonly
        ));
        assert!(scope_allows_command(
            GrantScope::RemoteShellInteractive,
            FileAccessScope::ReadWrite,
            CommandKind::ShellExec
        ));
        assert!(scope_allows_command(
            GrantScope::RemoteShellInteractive,
            FileAccessScope::ReadWrite,
            CommandKind::File
        ));
        assert!(scope_allows_command(
            GrantScope::RemoteShellInteractive,
            FileAccessScope::ReadWrite,
            CommandKind::PowerMgmt
        ));

        // Query only + no file = minimal access
        assert!(scope_allows_command(
            GrantScope::RemoteQuery,
            FileAccessScope::None,
            CommandKind::QueryReadonly
        ));
        assert!(!scope_allows_command(
            GrantScope::RemoteQuery,
            FileAccessScope::None,
            CommandKind::ShellExec
        ));
        assert!(!scope_allows_command(
            GrantScope::RemoteQuery,
            FileAccessScope::None,
            CommandKind::File
        ));

        // Shell exec + file read = shell + read-only file
        assert!(scope_allows_command(
            GrantScope::RemoteShellExec,
            FileAccessScope::Read,
            CommandKind::File
        ));
        assert!(scope_allows_command(
            GrantScope::RemoteShellExec,
            FileAccessScope::Read,
            CommandKind::ShellExec
        ));
    }

    #[test]
    fn test_remote_command_defaults_to_query_readonly() {
        let command: RemoteCommand = serde_json::from_value(serde_json::json!({
            "command": "status"
        }))
        .expect("command should deserialize");
        assert_eq!(command.kind, CommandKind::QueryReadonly);
        assert_eq!(command.summary_label(), "status");
    }

    #[test]
    fn test_remote_command_supports_shell_exec_shape() {
        let command: RemoteCommand = serde_json::from_value(serde_json::json!({
            "kind": "shell.exec",
            "policy_id": "deploy-api",
            "exec_mode": "argv_exec",
            "argv": ["./scripts/deploy.sh", "--env", "prod"],
            "cwd": "/srv/api",
            "env": {
                "NODE_ENV": "production"
            },
            "stdin_mode": "none",
            "timeout_ms": 600000,
            "pty": {
                "enabled": false
            },
            "output_mode": "split_streams"
        }))
        .expect("shell command should deserialize");

        assert_eq!(command.kind, CommandKind::ShellExec);
        assert_eq!(command.exec_mode, Some(ShellExecMode::ArgvExec));
        assert_eq!(command.policy_id.as_deref(), Some("deploy-api"));
        assert_eq!(
            command.argv,
            Some(vec![
                "./scripts/deploy.sh".to_string(),
                "--env".to_string(),
                "prod".to_string()
            ])
        );
        assert_eq!(command.pty.as_ref().map(|pty| pty.enabled), Some(false));
    }

    #[test]
    fn test_encrypted_envelope_v2_populates_aad() {
        let envelope = EncryptedEnvelope::v2(
            "call-1".to_string(),
            7,
            FrameDirection::ClientToCaller,
            "cipher".to_string(),
            Some("stdout".to_string()),
            Some(CommandKind::ShellExec),
            Some(GrantScope::RemoteShellExec),
        );

        assert_eq!(envelope.version, 2);
        let aad = envelope.aad.expect("aad should exist");
        assert_eq!(aad.version, 2);
        assert_eq!(aad.call_id, "call-1");
        assert_eq!(aad.seq, 7);
        assert_eq!(aad.frame_type.as_deref(), Some("stdout"));
        assert_eq!(aad.command_kind, Some(CommandKind::ShellExec));
        assert_eq!(aad.grant_scope, Some(GrantScope::RemoteShellExec));
    }

    #[test]
    fn test_derive_call_session_key_is_stable() {
        let first = derive_call_session_key(
            b"shared-secret",
            "call-1",
            Some("Y2FsbGVyLXB1Yg=="),
            Some("Y2xpZW50LXB1Yg=="),
            "",
            "",
        )
        .expect("first key");
        let second = derive_call_session_key(
            b"shared-secret",
            "call-1",
            Some("Y2FsbGVyLXB1Yg=="),
            Some("Y2xpZW50LXB1Yg=="),
            "",
            "",
        )
        .expect("second key");
        let other = derive_call_session_key(
            b"shared-secret",
            "call-2",
            Some("Y2FsbGVyLXB1Yg=="),
            Some("Y2xpZW50LXB1Yg=="),
            "",
            "",
        )
        .expect("other key");

        assert_eq!(first, second);
        assert_ne!(first, other);
    }

    #[test]
    fn test_decrypt_remote_command_payload_roundtrip() {
        let session_key = derive_call_session_key(
            b"shared-secret",
            "call-crypto",
            Some("caller-ephemeral"),
            Some("client-ephemeral"),
            "",
            "",
        )
        .expect("session key");
        let aad = EnvelopeAad {
            version: 2,
            call_id: "call-crypto".to_string(),
            seq: 0,
            direction: FrameDirection::CallerToClient,
            token_hash: None,
            frame_type: Some("command".to_string()),
            command_kind: Some(CommandKind::QueryReadonly),
            grant_scope: Some(GrantScope::RemoteQuery),
            sender_key_id: None,
            metadata: None,
        };
        let command = RemoteCommand {
            kind: CommandKind::QueryReadonly,
            command: "status".to_string(),
            args_json: Some(r#"{"limit":5}"#.to_string()),
            ..Default::default()
        };
        let plaintext = serde_json::to_vec(&command).expect("serialize command");

        let engine = base64::engine::general_purpose::STANDARD;
        let nonce_bytes = [7u8; 12];
        let mut in_out = plaintext.clone();
        let unbound = UnboundKey::new(&CHACHA20_POLY1305, &session_key).expect("unbound key");
        let key = LessSafeKey::new(unbound);
        let tag = key
            .seal_in_place_separate_tag(
                Nonce::assume_unique_for_key(nonce_bytes),
                Aad::from(serde_json::to_vec(&aad).expect("serialize aad")),
                &mut in_out,
            )
            .expect("seal payload");

        let payload = EncryptedPayload {
            version: 2,
            nonce: engine.encode(nonce_bytes),
            ciphertext: engine.encode(in_out),
            tag: engine.encode(tag.as_ref()),
            aad: Some(aad.clone()),
        };

        let decrypted =
            decrypt_remote_command_payload(&payload, &session_key, aad).expect("decrypt payload");
        assert_eq!(decrypted.kind, CommandKind::QueryReadonly);
        assert_eq!(decrypted.command, "status");
        assert_eq!(decrypted.args_json.as_deref(), Some(r#"{"limit":5}"#));
    }

    #[test]
    fn test_decrypt_remote_command_payload_without_aad_rejected() {
        let session_key = derive_open_call_session_key(
            b"shared-secret",
            "grant-crypto",
            Some("caller-ephemeral"),
            Some("client-ephemeral"),
            CommandKind::QueryReadonly,
            "",
            "",
        )
        .expect("session key");
        let command = RemoteCommand {
            kind: CommandKind::QueryReadonly,
            command: "status".to_string(),
            args_json: Some(r#"{"limit":5}"#.to_string()),
            ..Default::default()
        };
        let payload = encrypt_encrypted_payload_without_aad(&command, &session_key, 2)
            .expect("encrypt payload");
        let fallback_aad = EnvelopeAad {
            version: 2,
            call_id: "ignored".to_string(),
            seq: 0,
            direction: FrameDirection::CallerToClient,
            token_hash: None,
            frame_type: Some("command".to_string()),
            command_kind: Some(CommandKind::QueryReadonly),
            grant_scope: Some(GrantScope::RemoteQuery),
            sender_key_id: None,
            metadata: None,
        };

        let err = decrypt_remote_command_payload(&payload, &session_key, fallback_aad)
            .expect_err("missing aad must be rejected");
        assert!(err.to_string().contains("requires authenticated AAD"));
    }

    #[test]
    fn test_client_registration_ssh_route_serializes_three_states() {
        let base = ClientRegistrationRequest {
            challenge_id: "challenge".to_string(),
            client_instance_id: "client".to_string(),
            client_long_term_pubkey: "pubkey".to_string(),
            device_name: "device".to_string(),
            platform: "macos".to_string(),
            bifrost_version: "0.0.0-test".to_string(),
            signature: "signature".to_string(),
            timestamp: 1_700_000_000,
            ssh_device_route: None,
        };

        let omitted = serde_json::to_value(&base).expect("serialize omitted route");
        assert!(omitted.get("ssh_device_route").is_none());

        let mut clear = base.clone();
        clear.ssh_device_route = Some(None);
        let clear = serde_json::to_value(&clear).expect("serialize clear route");
        assert!(clear
            .get("ssh_device_route")
            .is_some_and(|value| value.is_null()));

        let mut publish = base;
        publish.ssh_device_route = Some(Some(SshDeviceRoute {
            device_code: "BF-0123456789ABCDEF".to_string(),
            public_key_pem: "public-key".to_string(),
        }));
        let publish = serde_json::to_value(&publish).expect("serialize publish route");
        assert_eq!(
            publish
                .get("ssh_device_route")
                .and_then(|value| value.get("device_code"))
                .and_then(|value| value.as_str()),
            Some("BF-0123456789ABCDEF")
        );
    }
}

// BEGIN PR#1 large-output protocol extensions
//
// ADDITIVE types for the `large_output_v1` protocol revision. Older peers
// that do not understand the new variants negotiate down to
// `OutputTransport::Inline` via `ProtocolFeatures` during handshake. No
// existing on-wire form is broken by this block.

pub const PROTOCOL_VERSION: u32 = 2;
pub const PROTOCOL_FEATURE_LARGE_OUTPUT_V1: &str = "large_output_v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputTransport {
    /// Legacy: stdout/stderr returned inline inside `RemoteInvokeResponse`,
    /// bounded by `max_output_bytes`. Preserves pre-v1 behaviour.
    #[default]
    Inline,
    /// v1: executor emits StreamFrame::Stdout/Stderr chunks with monotonic
    /// seq and base64-encoded payload. Final response carries digest + size.
    Streaming,
    /// v1: executor uploads full stdout/stderr to object storage; final
    /// response carries an `ObjectRef`. SSE still emitted for live tailing.
    SideChannel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectRef {
    pub url: String,
    pub size: u64,
    pub sha256: String,
    pub expires_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
}

// StreamFrame variants have a large size spread (Done carries digests
// and optional ObjectRef, Ack/Heartbeat are tiny). Boxing the large
// variants would change the wire-format / serde layout and break
// cross-version compatibility, so we allow the lint on the enum itself.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StreamFrame {
    Stdout {
        seq: u64,
        /// PR#3b: absolute byte offset of the first byte in `data_b64` within
        /// the total stdout stream. Enables receivers to detect gaps, dedup
        /// on resume, and reassemble in order even across reconnects.
        #[serde(default)]
        offset: u64,
        data_b64: String,
    },
    Stderr {
        seq: u64,
        #[serde(default)]
        offset: u64,
        data_b64: String,
    },
    Heartbeat {
        ts: u64,
        /// PR#3b: last stdout/stderr offsets the executor has emitted so a
        /// disconnected receiver can tell whether it is behind.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stdout_offset: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stderr_offset: Option<u64>,
    },
    /// PR#3b: advisory signal from executor to receiver: "the current relay
    /// connection is approaching its wall-clock limit; please reconnect soon
    /// with Resume(call_id, from_offset)". Receivers that ignore this will
    /// still get correct data, but may hit a hard relay-side disconnect.
    Reconnect {
        reason: String,
        stdout_offset: u64,
        stderr_offset: u64,
    },
    Ack {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stdout_seq: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stderr_seq: Option<u64>,
    },
    Done {
        exit_code: i32,
        total_stdout: u64,
        total_stderr: u64,
        stdout_sha256: String,
        stderr_sha256: String,
        duration_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stdout_object: Option<ObjectRef>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stderr_object: Option<ObjectRef>,
    },
    Error {
        code: String,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolFeatures {
    pub version: u32,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_inline_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_frame_bytes: Option<u64>,
}

impl Default for ProtocolFeatures {
    fn default() -> Self {
        Self {
            version: PROTOCOL_VERSION,
            features: vec![PROTOCOL_FEATURE_LARGE_OUTPUT_V1.to_string()],
            max_inline_bytes: Some(4 * 1024 * 1024),
            max_frame_bytes: Some(256 * 1024),
        }
    }
}

impl ProtocolFeatures {
    pub fn negotiates_large_output(&self, peer: &ProtocolFeatures) -> bool {
        self.features
            .iter()
            .any(|f| f == PROTOCOL_FEATURE_LARGE_OUTPUT_V1)
            && peer
                .features
                .iter()
                .any(|f| f == PROTOCOL_FEATURE_LARGE_OUTPUT_V1)
    }
    pub fn effective_max_frame_bytes(&self, peer: &ProtocolFeatures) -> u64 {
        let a = self.max_frame_bytes.unwrap_or(256 * 1024);
        let b = peer.max_frame_bytes.unwrap_or(256 * 1024);
        a.min(b)
    }
    pub fn effective_max_inline_bytes(&self, peer: &ProtocolFeatures) -> u64 {
        let a = self.max_inline_bytes.unwrap_or(4 * 1024 * 1024);
        let b = peer.max_inline_bytes.unwrap_or(4 * 1024 * 1024);
        a.min(b)
    }
}

#[cfg(test)]
mod pr1_large_output_protocol_tests {
    use super::*;

    #[test]
    fn output_transport_roundtrip() {
        for t in [
            OutputTransport::Inline,
            OutputTransport::Streaming,
            OutputTransport::SideChannel,
        ] {
            let s = serde_json::to_string(&t).unwrap();
            let back: OutputTransport = serde_json::from_str(&s).unwrap();
            assert_eq!(t, back);
        }
    }

    #[test]
    fn stream_frame_roundtrip() {
        let frames = vec![
            StreamFrame::Stdout {
                seq: 0,
                offset: 0,
                data_b64: "YQ==".into(),
            },
            StreamFrame::Stderr {
                seq: 7,
                offset: 0,
                data_b64: "Yg==".into(),
            },
            StreamFrame::Heartbeat {
                ts: 1_700_000_000_000,
                stdout_offset: Some(65536),
                stderr_offset: Some(0),
            },
            StreamFrame::Reconnect {
                reason: "relay-wall-clock".into(),
                stdout_offset: 65536,
                stderr_offset: 0,
            },
            StreamFrame::Ack {
                stdout_seq: Some(5),
                stderr_seq: None,
            },
            StreamFrame::Done {
                exit_code: 0,
                total_stdout: 123,
                total_stderr: 0,
                stdout_sha256: "deadbeef".into(),
                stderr_sha256: "cafe".into(),
                duration_ms: 12,
                stdout_object: None,
                stderr_object: None,
            },
            StreamFrame::Error {
                code: "policy.denied".into(),
                message: "nope".into(),
            },
        ];
        for f in frames {
            let s = serde_json::to_string(&f).unwrap();
            let back: StreamFrame = serde_json::from_str(&s).unwrap();
            assert_eq!(s, serde_json::to_string(&back).unwrap());
        }
    }

    #[test]
    fn object_ref_roundtrip() {
        let r = ObjectRef {
            url: "https://store.example/abc".into(),
            size: 123,
            sha256: "beef".into(),
            expires_at: 1_700_000_000,
            content_type: Some("application/octet-stream".into()),
        };
        let s = serde_json::to_string(&r).unwrap();
        let back: ObjectRef = serde_json::from_str(&s).unwrap();
        assert_eq!(back.url, r.url);
        assert_eq!(back.size, r.size);
    }

    #[test]
    fn features_negotiation() {
        let a = ProtocolFeatures::default();
        let b = ProtocolFeatures::default();
        assert!(a.negotiates_large_output(&b));
        assert_eq!(a.effective_max_frame_bytes(&b), 256 * 1024);
    }

    // ---- P1-A / P0-A security regression tests ---------------------------

    const ED25519_SPKI_PREFIX: &[u8] = &[
        0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
    ];

    fn spki_der_b64(raw_pub: &[u8]) -> String {
        let mut der = Vec::with_capacity(ED25519_SPKI_PREFIX.len() + raw_pub.len());
        der.extend_from_slice(ED25519_SPKI_PREFIX);
        der.extend_from_slice(raw_pub);
        base64::engine::general_purpose::STANDARD.encode(der)
    }

    fn gen_ed25519() -> (ring::signature::Ed25519KeyPair, String) {
        use ring::signature::{Ed25519KeyPair, KeyPair};
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).expect("gen pkcs8");
        let kp = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).expect("parse pkcs8");
        let pub_b64 = spki_der_b64(kp.public_key().as_ref());
        (kp, pub_b64)
    }

    #[test]
    fn p1a_counter_nonce_is_deterministic_and_direction_separated() {
        // Determinism: same (direction, seq) -> identical nonce.
        assert_eq!(
            counter_nonce(FrameDirection::CallerToClient, 7),
            counter_nonce(FrameDirection::CallerToClient, 7)
        );
        // Seq is 8-byte big-endian in the low bytes.
        let n = counter_nonce(FrameDirection::CallerToClient, 0x0102_0304_0506_0708);
        assert_eq!(&n[..4], &[0, 0, 0, 0]);
        assert_eq!(&n[4..], &0x0102_0304_0506_0708u64.to_be_bytes());
        // Direction is encoded in the prefix so the two peers never collide.
        let c2s = counter_nonce(FrameDirection::CallerToClient, 42);
        let s2c = counter_nonce(FrameDirection::ClientToCaller, 42);
        assert_ne!(c2s, s2c);
        assert_eq!(&c2s[..4], &[0, 0, 0, 0]);
        assert_eq!(&s2c[..4], &[0, 0, 0, 1]);
        // Different seq -> different nonce within a direction.
        assert_ne!(
            counter_nonce(FrameDirection::ClientToCaller, 1),
            counter_nonce(FrameDirection::ClientToCaller, 2)
        );
    }

    #[test]
    fn p1a_counter_encryptor_roundtrips_and_uses_counter_nonce() {
        let key = [7u8; 32];
        let payload = serde_json::json!({ "chunk": "hello-p1a" });
        let sealed =
            encrypt_encrypted_payload_counter(&payload, &key, 2, FrameDirection::ClientToCaller, 5)
                .expect("encrypt");
        // Nonce on the wire is the deterministic counter nonce.
        let nonce = base64::engine::general_purpose::STANDARD
            .decode(&sealed.nonce)
            .expect("decode nonce");
        assert_eq!(
            nonce,
            counter_nonce(FrameDirection::ClientToCaller, 5).to_vec()
        );
        // Roundtrip via the matching no-AAD decryptor.
        let decoded: serde_json::Value =
            decrypt_encrypted_payload_without_aad(&sealed, &key).expect("decrypt");
        assert_eq!(decoded, payload);
    }

    #[test]
    fn p0a_ephemeral_signature_verifies_and_binds_key_roundtrip() {
        // Positive: caller signs its ephemeral pubkey; target verifies + pins fp,
        // then both derive the SAME channel-bound session key and roundtrip.
        let (caller_kp, caller_lt_b64) = gen_ed25519();
        let (_client_kp, client_lt_b64) = gen_ed25519();
        let caller_fp = fingerprint_long_term_pubkey(&caller_lt_b64).expect("caller fp");
        let client_fp = fingerprint_long_term_pubkey(&client_lt_b64).expect("client fp");

        let caller_eph = "oaNKFHB0HF2375h+0cywBEOqPbh1zUga5nvlpAaxhk0=";
        let payload = build_ephemeral_signature_payload(
            "binding-123",
            "caller-instance",
            &client_fp,
            caller_eph,
            1_700_000_000,
        );
        let sig = {
            let s = caller_kp.sign(payload.as_bytes());
            base64::engine::general_purpose::STANDARD.encode(s.as_ref())
        };
        // Verify succeeds and pinned fingerprint matches.
        verify_ephemeral_signature(&caller_lt_b64, payload.as_bytes(), &sig, Some(&caller_fp))
            .expect("verification should succeed");

        // Channel binding: identical fp order on both sides -> identical key.
        let shared = [0x11u8; 32];
        let call_id = "call-p0a";
        let key_target = derive_call_session_key(
            &shared,
            call_id,
            Some(caller_eph),
            Some("+28VjQSK28Z+0Ckn9lRCn/D1Lq84MyU9DZTO5X/oZHs="),
            &caller_fp,
            &client_fp,
        )
        .expect("target key");
        let key_caller = derive_call_session_key(
            &shared,
            call_id,
            Some(caller_eph),
            Some("+28VjQSK28Z+0Ckn9lRCn/D1Lq84MyU9DZTO5X/oZHs="),
            &caller_fp,
            &client_fp,
        )
        .expect("caller key");
        assert_eq!(key_target, key_caller);
        let msg = serde_json::json!({ "chunk": "bound" });
        let sealed = encrypt_encrypted_payload_counter(
            &msg,
            &key_caller,
            2,
            FrameDirection::ClientToCaller,
            1,
        )
        .expect("encrypt");
        let got: serde_json::Value =
            decrypt_encrypted_payload_without_aad(&sealed, &key_target).expect("decrypt");
        assert_eq!(got, msg);

        // Wrong ephemeral pub -> different key (binding is effective).
        let key_other = derive_call_session_key(
            &shared,
            call_id,
            Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="),
            Some("+28VjQSK28Z+0Ckn9lRCn/D1Lq84MyU9DZTO5X/oZHs="),
            &caller_fp,
            &client_fp,
        )
        .expect("other key");
        assert_ne!(key_other, key_caller);
    }

    #[test]
    fn p0a_ephemeral_signature_rejects_tamper_and_wrong_fingerprint() {
        let (caller_kp, caller_lt_b64) = gen_ed25519();
        let caller_fp = fingerprint_long_term_pubkey(&caller_lt_b64).expect("caller fp");
        let caller_eph = "oaNKFHB0HF2375h+0cywBEOqPbh1zUga5nvlpAaxhk0=";
        let payload = build_ephemeral_signature_payload(
            "binding-123",
            "caller-instance",
            "sha256:peerfp",
            caller_eph,
            1_700_000_000,
        );
        let sig = base64::engine::general_purpose::STANDARD
            .encode(caller_kp.sign(payload.as_bytes()).as_ref());

        // Negative 1: tampered payload (different ephemeral pub) fails.
        let tampered = build_ephemeral_signature_payload(
            "binding-123",
            "caller-instance",
            "sha256:peerfp",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
            1_700_000_000,
        );
        assert!(verify_ephemeral_signature(
            &caller_lt_b64,
            tampered.as_bytes(),
            &sig,
            Some(&caller_fp)
        )
        .is_err());

        // Negative 2: correct signature but wrong pinned fingerprint fails.
        assert!(verify_ephemeral_signature(
            &caller_lt_b64,
            payload.as_bytes(),
            &sig,
            Some("sha256:0000000000000000000000000000000000000000000000000000000000000000")
        )
        .is_err());

        // Negative 3: signature from a different key fails.
        let (_other_kp, other_lt_b64) = gen_ed25519();
        assert!(verify_ephemeral_signature(&other_lt_b64, payload.as_bytes(), &sig, None).is_err());
    }
}
// END PR#1 large-output protocol extensions
