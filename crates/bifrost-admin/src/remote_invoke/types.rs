use std::collections::BTreeMap;

use base64::Engine;
use bifrost_command::CanonicalQueryCommand;
use bifrost_core::{BifrostError, Result};
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, CHACHA20_POLY1305};
use ring::hkdf::{Salt, HKDF_SHA256};
use ring::rand::SecureRandom;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    #[serde(rename = "remote_file_read")]
    RemoteFileRead,
    #[serde(rename = "remote_file_write")]
    RemoteFileWrite,
}

impl GrantScope {
    pub fn allows_command(self, kind: CommandKind) -> bool {
        matches!(
            (self, kind),
            (Self::RemoteQuery, CommandKind::QueryReadonly)
                | (Self::RemoteShellExec, CommandKind::QueryReadonly)
                | (Self::RemoteShellExec, CommandKind::ShellExec)
                | (Self::RemoteShellInteractive, CommandKind::QueryReadonly)
                | (Self::RemoteShellInteractive, CommandKind::ShellExec)
                | (Self::RemoteFileRead, CommandKind::File)
                | (Self::RemoteFileWrite, CommandKind::File)
        )
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
}

impl CommandKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::QueryReadonly => "query.readonly",
            Self::ShellExec => "shell.exec",
            Self::File => "file",
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
    5
}

fn default_retention_days() -> u32 {
    90
}

fn default_max_records() -> u32 {
    10_000
}

impl Default for RemoteInvokeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            relay_url: "https://bifrost.bytedance.net".to_string(),
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pty: Option<RemotePtyRequest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_mode: Option<OutputMode>,
    #[serde(skip)]
    pub grant_id: Option<String>,
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
    grant_id: &str,
    caller_ephemeral_pub: Option<&str>,
    client_ephemeral_pub: Option<&str>,
    command_kind: CommandKind,
) -> Result<[u8; 32]> {
    let salt = Salt::new(HKDF_SHA256, grant_id.as_bytes());
    let prk = salt.extract(shared_secret);

    let caller_pub = decode_base64_or_raw(caller_ephemeral_pub.unwrap_or_default());
    let client_pub = decode_base64_or_raw(client_ephemeral_pub.unwrap_or_default());
    let info_parts: [&[u8]; 4] = [
        OPEN_CALL_HKDF_INFO_PREFIX,
        caller_pub.as_slice(),
        client_pub.as_slice(),
        command_kind.as_str().as_bytes(),
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
) -> Result<[u8; 32]> {
    let salt = Salt::new(HKDF_SHA256, call_id.as_bytes());
    let prk = salt.extract(shared_secret);

    let caller_pub = decode_base64_or_raw(caller_ephemeral_pub.unwrap_or_default());
    let client_pub = decode_base64_or_raw(client_ephemeral_pub.unwrap_or_default());
    let info_parts: [&[u8]; 3] = [
        E2E_HKDF_INFO_PREFIX,
        caller_pub.as_slice(),
        client_pub.as_slice(),
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
        return decrypt_encrypted_payload_without_aad(payload, session_key);
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

    let mut nonce_bytes = [0u8; 12];
    ring::rand::SystemRandom::new()
        .fill(&mut nonce_bytes)
        .map_err(|_| BifrostError::Config("generate encrypted payload nonce failed".to_string()))?;

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

fn decrypt_encrypted_payload_without_aad<T>(
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
    pub grant_mode: GrantMode,
    #[serde(default)]
    pub grant_scope: GrantScope,
    #[serde(default = "default_auth_method")]
    pub auth_method: AuthMethod,
    pub status: GrantStatus,
    pub first_authorized_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<u64>,
    pub max_calls: Option<u32>,
    pub remaining_calls: Option<u32>,
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
    pub client_ephemeral_pub: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateGrantRequest {
    pub client_instance_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_scope: Option<GrantScope>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientCallFrameRequest {
    pub call_id: String,
    pub client_instance_id: String,
    pub envelope_json: String,
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
    pub grant_mode: Option<GrantMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_scope: Option<GrantScope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller_ephemeral_pub: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_ephemeral_pub: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemotePtyRequest {
    pub enabled: bool,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteInvokeResponse {
    pub exit_code: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr_digest: Option<String>,
    pub duration_ms: u64,
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
    fn test_grant_scope_rejects_legacy_remote_invoke_string() {
        let scope = serde_json::from_value::<GrantScope>(serde_json::Value::String(
            "remote_invoke".to_string(),
        ));
        assert!(scope.is_err());
    }

    #[test]
    fn test_file_scopes_are_separate_from_shell_scopes() {
        // With unified CommandKind::File, shell scopes should NOT allow file commands
        assert!(!GrantScope::RemoteShellExec.allows_command(CommandKind::File));
        assert!(!GrantScope::RemoteShellInteractive.allows_command(CommandKind::File));
        // Both file read and file write scopes allow the unified File kind
        assert!(GrantScope::RemoteFileRead.allows_command(CommandKind::File));
        assert!(GrantScope::RemoteFileWrite.allows_command(CommandKind::File));
        // File scopes should NOT allow shell/query commands
        assert!(!GrantScope::RemoteFileRead.allows_command(CommandKind::ShellExec));
        assert!(!GrantScope::RemoteFileWrite.allows_command(CommandKind::ShellExec));
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
        )
        .expect("first key");
        let second = derive_call_session_key(
            b"shared-secret",
            "call-1",
            Some("Y2FsbGVyLXB1Yg=="),
            Some("Y2xpZW50LXB1Yg=="),
        )
        .expect("second key");
        let other = derive_call_session_key(
            b"shared-secret",
            "call-2",
            Some("Y2FsbGVyLXB1Yg=="),
            Some("Y2xpZW50LXB1Yg=="),
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
    fn test_decrypt_remote_command_payload_without_aad_roundtrip() {
        let session_key = derive_open_call_session_key(
            b"shared-secret",
            "grant-crypto",
            Some("caller-ephemeral"),
            Some("client-ephemeral"),
            CommandKind::QueryReadonly,
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

        let decrypted = decrypt_remote_command_payload(&payload, &session_key, fallback_aad)
            .expect("decrypt payload");
        assert_eq!(decrypted.kind, CommandKind::QueryReadonly);
        assert_eq!(decrypted.command, "status");
        assert_eq!(decrypted.args_json.as_deref(), Some(r#"{"limit":5}"#));
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
