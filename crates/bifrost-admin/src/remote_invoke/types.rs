use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameDirection {
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
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RemoteCommand {
    #[serde(default)]
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args_json: Option<String>,
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
    pub version: u32,
    pub call_id: String,
    pub seq: u64,
    pub direction: FrameDirection,
    pub nonce: String,
    pub ciphertext: String,
    pub tag: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aad: Option<EnvelopeAad>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvelopeAad {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_type: Option<String>,
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
        command: RemoteCommand,
        caller_pubkey: String,
    },
    GrantCreated {
        grant_id: String,
        call_id: String,
        relay_token: String,
        caller_ephemeral_pub: String,
        grant_mode: GrantMode,
        expires_at: Option<u64>,
    },
    CallOpen {
        call_id: String,
        grant_id: String,
        caller_ephemeral_pub: String,
        command_summary: CommandSummary,
        command: RemoteCommand,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrantInfo {
    pub grant_id: String,
    pub client_instance_id: String,
    pub caller_fingerprint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller_display_name: Option<String>,
    pub grant_mode: GrantMode,
    pub grant_scope: String,
    pub status: GrantStatus,
    pub first_authorized_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<u64>,
    pub max_calls: Option<u32>,
    pub remaining_calls: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallInfo {
    pub call_id: String,
    pub grant_id: String,
    pub pairing_id: Option<String>,
    pub client_instance_id: String,
    pub caller_fingerprint: String,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrantDecisionRequest {
    pub pairing_id: String,
    pub client_instance_id: String,
    pub decision: GrantDecision,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_mode: Option<GrantMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_ephemeral_pub: Option<String>,
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

pub const ALLOWED_COMMANDS: &[&str] = &[
    "status",
    "traffic.list",
    "traffic.get",
    "traffic.search",
    "search.get",
];

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
