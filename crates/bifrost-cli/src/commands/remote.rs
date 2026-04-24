use std::collections::{BTreeMap, HashSet};
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::commands::caller_stream_frame::{
    parse_stream_frame_from_sse_data, CallerStreamState, StreamDecision,
};
use base64::Engine;
use bifrost_command::{
    CanonicalQueryCommand, FilterCondition, SearchArgs, SearchFilters, SearchScope,
    TrafficClearArgs, TrafficGetArgs, TrafficListArgs, TrafficListDirection,
};
use bifrost_core::{direct_reqwest_client_builder, BifrostError};
use colored::Colorize;
use dialoguer::{theme::ColorfulTheme, Select};
use futures::StreamExt;
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, CHACHA20_POLY1305, NONCE_LEN};
use ring::agreement::{self, UnparsedPublicKey, X25519};
use ring::digest::{digest, SHA256};
use ring::hkdf;
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, error, warn};

use super::search::SearchResultItem;
use super::{render_traffic_detail_body, render_traffic_list_body, OutputFormat};
use crate::cli::{
    RemoteCommandCommands, RemoteCommandExecArgs, RemoteCommands, RemoteSearchArgs,
    RemoteTrafficCommands,
};

const PAIRING_WATCH_TIMEOUT_SECS: u64 = 180;
// PR #5a: interpreted as an IDLE deadline (resets on every chunk/event),
// not an overall wall-clock deadline. 300s matches executor-side
// DEFAULT_IDLE_TIMEOUT_MS so client and server agree on liveness.
const CALL_EVENT_TIMEOUT_SECS: u64 = 300;
const CANCEL_SETTLE_TIMEOUT_SECS: u64 = 10;
const CANCEL_SETTLE_TOTAL_TIMEOUT_SECS: u64 = 15;
const CALLER_USER_AGENT: &str = "bifrost-cli-remote";
const CONNECTIONS_FILE: &str = "remote-connections.json";
const CONNECTIONS_KEY_FILE: &str = "remote-connections.key";
const START_PAIRING_OVERLOAD_RETRY_DELAYS_MS: [u64; 3] = [300, 700, 1500];
const CANCEL_SETTLE_RETRY_DELAYS_MS: [u64; 4] = [200, 500, 1000, 2000];
const SSH_CONNECT_TIMEOUT_SECS: u64 = 30;
const BIFROST_KEY_BEGIN: &str = "-----BEGIN BIFROST KEY-----";
const BIFROST_KEY_END: &str = "-----END BIFROST KEY-----";
const PKCS8_KEY_BEGIN: &str = "-----BEGIN PRIVATE KEY-----";
const PKCS8_KEY_END: &str = "-----END PRIVATE KEY-----";
const TRANSPORT_CONTEXT_VERSION: u32 = 2;
const ENCRYPTED_OPEN_CALL_VERSION: u32 = 2;
const LOCAL_SECRET_FORMAT_VERSION: u32 = 1;
const OPEN_CALL_HKDF_INFO_PREFIX: &[u8] = b"bifrost-open-call-v2";
const CALL_EVENT_HKDF_INFO_PREFIX: &[u8] = b"bifrost-e2e-v1";

fn should_print_remote_progress_banner(stdout_is_terminal: bool) -> bool {
    stdout_is_terminal
}

#[derive(Debug)]
pub struct RemoteOptions {
    pub relay_url: String,
    pub client_id: Option<String>,
    pub action: RemoteCommands,
}

#[derive(Debug, Clone)]
struct BuiltRemoteCommand {
    kind: CommandKind,
    label: String,
    command: Option<String>,
    args_json: Option<String>,
    query: Option<CanonicalQueryCommand>,
    shell_exec: Option<ShellExecPayload>,
    render: RemoteRenderMode,
    /// PR #5e-1: populated only for shell.exec; None otherwise.
    #[allow(dead_code)]
    streaming_prefs: Option<StreamingPrefs>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenCallCommandSummary {
    command_preview: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    masked_args_json: Option<String>,
}

#[derive(Debug, Clone)]
struct ShellExecPayload {
    exec_mode: String,
    argv: Option<Vec<String>>,
    command_text: Option<String>,
    cwd: Option<String>,
    env: Option<BTreeMap<String, String>>,
    timeout_ms: Option<u64>,
}

/// PR #5e-1: caller-side streaming preferences derived from CLI flags.
/// `stream=true` routes the call through `subscribe_call_events_streaming`.
/// `output_file=Some(..)` implies streaming and writes stdout to that file.
/// `resume_call_id=Some(..)` bypasses open_call and rejoins an existing
/// session (PR #5e-3).
#[derive(Debug, Clone, Default)]
struct StreamingPrefs {
    #[allow(dead_code)]
    pub stream: bool,
    #[allow(dead_code)]
    pub output_file: Option<std::path::PathBuf>,
    #[allow(dead_code)]
    pub resume_call_id: Option<String>,
    #[allow(dead_code)]
    pub no_verify_digest: bool,
}

impl StreamingPrefs {
    /// Effective streaming mode: explicit --stream, or implied by --output-file
    /// or --resume-call-id.
    #[allow(dead_code)]
    pub fn is_streaming(&self) -> bool {
        self.stream || self.output_file.is_some() || self.resume_call_id.is_some()
    }
}

#[derive(Debug, Clone)]
enum RemoteRenderMode {
    Raw,
    Search {
        format: OutputFormat,
        no_color: bool,
        keyword: String,
        max_scan: Option<usize>,
        max_results: Option<usize>,
    },
    TrafficList {
        format: OutputFormat,
        no_color: bool,
    },
    TrafficGet {
        format: OutputFormat,
        no_color: bool,
    },
}

#[derive(Debug, Clone, Deserialize)]
struct RemoteSearchProgressPayload {
    total_searched: usize,
    total_matched: usize,
    #[allow(dead_code)]
    next_cursor: Option<u64>,
    #[allow(dead_code)]
    has_more_hint: bool,
    #[allow(dead_code)]
    iterations: usize,
}

#[derive(Debug, Clone, Deserialize)]
struct RemoteSearchDonePayload {
    total_searched: usize,
    total_matched: usize,
    #[allow(dead_code)]
    next_cursor: Option<u64>,
    has_more: bool,
    #[allow(dead_code)]
    search_id: String,
}

#[derive(Debug, Clone, Deserialize)]
struct RemoteSearchErrorPayload {
    message: String,
}

enum RemoteSearchEvent {
    Result(Box<SearchResultItem>),
    Progress(RemoteSearchProgressPayload),
    Done(RemoteSearchDonePayload),
    Error(RemoteSearchErrorPayload),
}

#[derive(Default)]
struct RemoteSearchSseDecoder {
    partial_line: String,
    event_name: String,
    data_lines: Vec<String>,
}

struct RemoteSearchRenderer {
    format: OutputFormat,
    keyword: String,
    max_scan: Option<usize>,
    max_results: Option<usize>,
    use_color: bool,
    decoder: RemoteSearchSseDecoder,
    printed_header: bool,
    progress_visible: bool,
    total_matched: usize,
    total_searched: usize,
    has_more: bool,
    json_results: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LocalConnection {
    client_instance_id: String,
    device_name: String,
    platform: String,
    relay_url: String,
    grant_id: String,
    grant_mode: String,
    caller_fingerprint: String,
    connected_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    auth_method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ssh_key_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ssh_key_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    device_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transport_context_version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    caller_ephemeral_pub: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_ephemeral_pub: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    shared_secret_encrypted: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConnectionsFile {
    version: u32,
    connections: Vec<LocalConnection>,
}

fn connections_path() -> PathBuf {
    bifrost_storage::data_dir().join(CONNECTIONS_FILE)
}

fn connections_key_path() -> PathBuf {
    bifrost_storage::data_dir().join(CONNECTIONS_KEY_FILE)
}

fn load_connections() -> bifrost_core::Result<Vec<LocalConnection>> {
    let path = connections_path();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(&path).map_err(|e| {
        BifrostError::Io(std::io::Error::other(format!(
            "read {}: {e}",
            path.display()
        )))
    })?;
    let file: ConnectionsFile = serde_json::from_str(&content)
        .map_err(|e| BifrostError::Config(format!("parse {}: {e}", path.display())))?;
    Ok(file.connections)
}

fn save_connections(connections: &[LocalConnection]) -> bifrost_core::Result<()> {
    let path = connections_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "mkdir {}: {e}",
                parent.display()
            )))
        })?;
    }
    let file = ConnectionsFile {
        version: 2,
        connections: connections.to_vec(),
    };
    let content = serde_json::to_string_pretty(&file)
        .map_err(|e| BifrostError::Config(format!("serialize connections: {e}")))?;
    std::fs::write(&path, content).map_err(|e| {
        BifrostError::Io(std::io::Error::other(format!(
            "write {}: {e}",
            path.display()
        )))
    })?;
    Ok(())
}

fn upsert_local_connection(connections: &mut Vec<LocalConnection>, new_conn: LocalConnection) {
    connections.retain(|existing| {
        !(existing.client_instance_id == new_conn.client_instance_id
            && existing.relay_url == new_conn.relay_url)
    });
    connections.push(new_conn);
}

fn resolve_local_connection(
    connections: &[LocalConnection],
    explicit_id: Option<&str>,
) -> bifrost_core::Result<LocalConnection> {
    if let Some(prefix) = explicit_id {
        let matches: Vec<&LocalConnection> = connections
            .iter()
            .filter(|c| c.client_instance_id.starts_with(prefix))
            .collect();

        match matches.len() {
            0 => {
                return Err(BifrostError::Config(
                    "no saved connection matching that prefix, please run `bifrost remote connect <pair-code>` first".to_string(),
                ));
            }
            1 => {
                let conn = matches[0];
                if conn.client_instance_id != prefix {
                    debug!(prefix = %prefix, full_id = %conn.client_instance_id, "resolved short client id from local connections");
                }
                return Ok(conn.clone());
            }
            n => {
                if !std::io::stdin().is_terminal() {
                    return Err(BifrostError::Config(format!(
                        "ambiguous client id prefix '{prefix}' matches {n} saved connections, please be more specific"
                    )));
                }

                let items: Vec<String> = matches
                    .iter()
                    .map(|c| {
                        let short_id = &c.client_instance_id[..c.client_instance_id.len().min(12)];
                        format!("{} ({short_id})", c.device_name)
                    })
                    .collect();

                let selection = Select::with_theme(&ColorfulTheme::default())
                    .with_prompt(format!(
                        "Prefix '{prefix}' matches {n} connections, select one"
                    ))
                    .items(&items)
                    .default(0)
                    .interact()
                    .map_err(|e| BifrostError::Io(std::io::Error::other(e)))?;

                return Ok(matches[selection].clone());
            }
        }
    }

    match connections.len() {
        0 => Err(BifrostError::Config(
            "no saved connection, please run `bifrost remote connect <pair-code>` first"
                .to_string(),
        )),
        1 => {
            let conn = &connections[0];
            if should_print_remote_progress_banner(std::io::stdout().is_terminal()) {
                let short_id = &conn.client_instance_id[..conn.client_instance_id.len().min(12)];
                println!(
                    "{}",
                    format!(
                        "→ Using saved connection: {} ({short_id})",
                        conn.device_name
                    )
                    .dimmed()
                );
            }
            Ok(conn.clone())
        }
        n => {
            if !std::io::stdin().is_terminal() {
                return Err(BifrostError::Config(
                    "multiple saved connections, please specify --client-id".to_string(),
                ));
            }

            let items: Vec<String> = connections
                .iter()
                .map(|c| {
                    let short_id = &c.client_instance_id[..c.client_instance_id.len().min(12)];
                    format!("{} ({short_id})", c.device_name)
                })
                .collect();

            let selection = Select::with_theme(&ColorfulTheme::default())
                .with_prompt(format!("Found {n} saved connections, select one"))
                .items(&items)
                .default(0)
                .interact()
                .map_err(|e| BifrostError::Io(std::io::Error::other(e)))?;

            Ok(connections[selection].clone())
        }
    }
}

#[derive(Debug, Clone)]
struct StoredTransportContext {
    caller_ephemeral_pub: String,
    client_ephemeral_pub: String,
    shared_secret_encrypted: String,
}

#[derive(Debug)]
struct PendingCallerTransport {
    private_key: agreement::EphemeralPrivateKey,
    caller_ephemeral_pub: String,
}

impl PendingCallerTransport {
    fn generate() -> bifrost_core::Result<Self> {
        let rng = SystemRandom::new();
        let private_key =
            agreement::EphemeralPrivateKey::generate(&X25519, &rng).map_err(|_| {
                BifrostError::Config("generate caller ephemeral key failed".to_string())
            })?;
        let caller_ephemeral_pub = base64::engine::general_purpose::STANDARD.encode(
            private_key
                .compute_public_key()
                .map_err(|_| {
                    BifrostError::Config("compute caller ephemeral public key failed".to_string())
                })?
                .as_ref(),
        );
        Ok(Self {
            private_key,
            caller_ephemeral_pub,
        })
    }

    fn finalize(self, client_ephemeral_pub: &str) -> bifrost_core::Result<StoredTransportContext> {
        let client_pub_raw = base64::engine::general_purpose::STANDARD
            .decode(client_ephemeral_pub)
            .map_err(|e| {
                BifrostError::Config(format!("decode client_ephemeral_pub from relay: {e}"))
            })?;

        let peer_public = UnparsedPublicKey::new(&X25519, client_pub_raw);
        let shared_secret =
            agreement::agree_ephemeral(self.private_key, &peer_public, |secret| secret.to_vec())
                .map_err(|_| {
                    BifrostError::Config(
                        "derive encrypted transport shared secret failed".to_string(),
                    )
                })?;

        Ok(StoredTransportContext {
            caller_ephemeral_pub: self.caller_ephemeral_pub,
            client_ephemeral_pub: client_ephemeral_pub.to_string(),
            shared_secret_encrypted: encrypt_local_secret(&shared_secret)?,
        })
    }
}

#[derive(Debug, Clone)]
struct OpenCallTransportContext {
    caller_ephemeral_pub: String,
    client_ephemeral_pub: String,
    shared_secret: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum CommandKind {
    #[serde(rename = "query.readonly")]
    QueryReadonly,
    #[serde(rename = "shell.exec")]
    ShellExec,
}

impl CommandKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::QueryReadonly => "query.readonly",
            Self::ShellExec => "shell.exec",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EncryptedPayload {
    #[serde(default = "default_encrypted_open_call_version")]
    version: u32,
    nonce: String,
    ciphertext: String,
    tag: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    aad: Option<FrameEnvelopeAad>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CommandEnvelope {
    kind: String,
    command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    args_json: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    query: Option<CanonicalQueryCommand>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exec_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    argv: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    command_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    env: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FrameEnvelopeAad {
    version: u32,
    call_id: String,
    seq: u64,
    direction: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    frame_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    command_kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EncryptedFramePayload {
    chunk: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EncryptedExitPayload {
    exit_code: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stderr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stdout_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stderr_digest: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LocalSecretEnvelope {
    version: u32,
    nonce: String,
    ciphertext: String,
}

fn default_encrypted_open_call_version() -> u32 {
    ENCRYPTED_OPEN_CALL_VERSION
}

#[derive(Debug, Clone, Copy)]
struct HkdfLen(usize);

impl hkdf::KeyType for HkdfLen {
    fn len(&self) -> usize {
        self.0
    }
}

fn load_or_create_connections_key() -> bifrost_core::Result<[u8; 32]> {
    let path = connections_key_path();
    if path.exists() {
        let encoded = std::fs::read_to_string(&path).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "read {}: {e}",
                path.display()
            )))
        })?;
        let raw = base64::engine::general_purpose::STANDARD
            .decode(encoded.trim())
            .map_err(|e| BifrostError::Config(format!("parse {}: {e}", path.display())))?;
        let key: [u8; 32] = raw.try_into().map_err(|_| {
            BifrostError::Config(format!("{} must contain a 32-byte key", path.display()))
        })?;
        return Ok(key);
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "mkdir {}: {e}",
                parent.display()
            )))
        })?;
    }

    let mut key = [0u8; 32];
    SystemRandom::new()
        .fill(&mut key)
        .map_err(|_| BifrostError::Config("generate local transport key failed".to_string()))?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(key);
    std::fs::write(&path, format!("{encoded}\n")).map_err(|e| {
        BifrostError::Io(std::io::Error::other(format!(
            "write {}: {e}",
            path.display()
        )))
    })?;
    Ok(key)
}

fn encrypt_local_secret(plaintext: &[u8]) -> bifrost_core::Result<String> {
    let key_bytes = load_or_create_connections_key()?;
    let unbound = UnboundKey::new(&AES_256_GCM, &key_bytes).map_err(|_| {
        BifrostError::Config("initialize local transport secret encryption failed".to_string())
    })?;
    let key = LessSafeKey::new(unbound);
    let mut nonce = [0u8; NONCE_LEN];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| BifrostError::Config("generate local transport nonce failed".to_string()))?;
    let mut ciphertext = plaintext.to_vec();
    key.seal_in_place_append_tag(
        Nonce::assume_unique_for_key(nonce),
        Aad::empty(),
        &mut ciphertext,
    )
    .map_err(|_| BifrostError::Config("encrypt local transport secret failed".to_string()))?;

    serde_json::to_string(&LocalSecretEnvelope {
        version: LOCAL_SECRET_FORMAT_VERSION,
        nonce: base64::engine::general_purpose::STANDARD.encode(nonce),
        ciphertext: base64::engine::general_purpose::STANDARD.encode(ciphertext),
    })
    .map_err(|e| BifrostError::Config(format!("serialize local transport secret failed: {e}")))
}

fn decrypt_local_secret(encoded: &str) -> bifrost_core::Result<Vec<u8>> {
    let envelope: LocalSecretEnvelope = serde_json::from_str(encoded)
        .map_err(|e| BifrostError::Config(format!("parse local transport secret failed: {e}")))?;
    if envelope.version != LOCAL_SECRET_FORMAT_VERSION {
        return Err(BifrostError::Config(format!(
            "unsupported local transport secret version: {}",
            envelope.version
        )));
    }

    let key_bytes = load_or_create_connections_key()?;
    let unbound = UnboundKey::new(&AES_256_GCM, &key_bytes).map_err(|_| {
        BifrostError::Config("initialize local transport secret decryption failed".to_string())
    })?;
    let key = LessSafeKey::new(unbound);
    let nonce_raw = base64::engine::general_purpose::STANDARD
        .decode(envelope.nonce)
        .map_err(|e| BifrostError::Config(format!("decode local transport nonce failed: {e}")))?;
    let nonce: [u8; NONCE_LEN] = nonce_raw
        .try_into()
        .map_err(|_| BifrostError::Config("local transport nonce must be 12 bytes".to_string()))?;
    let mut ciphertext = base64::engine::general_purpose::STANDARD
        .decode(envelope.ciphertext)
        .map_err(|e| {
            BifrostError::Config(format!("decode local transport ciphertext failed: {e}"))
        })?;
    let plaintext = key
        .open_in_place(
            Nonce::assume_unique_for_key(nonce),
            Aad::empty(),
            &mut ciphertext,
        )
        .map_err(|_| BifrostError::Config("decrypt local transport secret failed".to_string()))?;
    Ok(plaintext.to_vec())
}

fn merge_transport_context(
    conn: &LocalConnection,
    grant: &GrantInfo,
) -> bifrost_core::Result<OpenCallTransportContext> {
    let caller_ephemeral_pub = conn
        .caller_ephemeral_pub
        .clone()
        .or_else(|| grant.caller_ephemeral_pub.clone())
        .ok_or_else(|| {
            BifrostError::Config(
                "saved connection is missing caller_ephemeral_pub; reconnect to refresh encrypted transport context".to_string(),
            )
        })?;
    let client_ephemeral_pub = conn
        .client_ephemeral_pub
        .clone()
        .or_else(|| grant.client_ephemeral_pub.clone())
        .ok_or_else(|| {
            BifrostError::Config(
                "saved connection is missing client_ephemeral_pub; reconnect to refresh encrypted transport context".to_string(),
            )
        })?;
    let shared_secret_encrypted = conn.shared_secret_encrypted.clone().ok_or_else(|| {
        BifrostError::Config(
            "saved connection is missing encrypted transport secret; reconnect to refresh encrypted transport context".to_string(),
        )
    })?;

    if let Some(grant_caller_pub) = &grant.caller_ephemeral_pub {
        if grant_caller_pub != &caller_ephemeral_pub {
            return Err(BifrostError::Config(
                "relay grant caller_ephemeral_pub does not match saved encrypted transport context; reconnect required".to_string(),
            ));
        }
    }
    if let Some(grant_client_pub) = &grant.client_ephemeral_pub {
        if grant_client_pub != &client_ephemeral_pub {
            return Err(BifrostError::Config(
                "relay grant client_ephemeral_pub does not match saved encrypted transport context; reconnect required".to_string(),
            ));
        }
    }

    Ok(OpenCallTransportContext {
        caller_ephemeral_pub,
        client_ephemeral_pub,
        shared_secret: decrypt_local_secret(&shared_secret_encrypted)?,
    })
}

fn reconcile_reusable_grant_for_transport(
    conn: &mut LocalConnection,
    grant: GrantInfo,
) -> bifrost_core::Result<GrantInfo> {
    if conn.grant_id.is_empty() || grant.grant_id == conn.grant_id {
        return Ok(grant);
    }

    let saved_caller_pub = conn.caller_ephemeral_pub.as_deref().unwrap_or_default();
    let saved_client_pub = conn.client_ephemeral_pub.as_deref().unwrap_or_default();
    let relay_caller_pub = grant
        .caller_ephemeral_pub
        .as_deref()
        .unwrap_or(saved_caller_pub);
    let relay_client_pub = grant
        .client_ephemeral_pub
        .as_deref()
        .unwrap_or(saved_client_pub);

    if !saved_caller_pub.is_empty()
        && !relay_caller_pub.is_empty()
        && relay_caller_pub != saved_caller_pub
    {
        return Err(BifrostError::Config(
            "saved connection transport no longer matches relay reusable authorization; reconnect required"
                .to_string(),
        ));
    }

    if !saved_client_pub.is_empty()
        && !relay_client_pub.is_empty()
        && relay_client_pub != saved_client_pub
    {
        return Err(BifrostError::Config(
            "saved connection transport no longer matches relay reusable authorization; reconnect required"
                .to_string(),
        ));
    }

    warn!(
        saved_grant_id = %conn.grant_id,
        relay_grant_id = %grant.grant_id,
        "relay returned a newer reusable grant with the same encrypted transport; updating saved grant_id"
    );
    conn.grant_id = grant.grant_id.clone();
    if grant.caller_ephemeral_pub.is_some() {
        conn.caller_ephemeral_pub = grant.caller_ephemeral_pub.clone();
    }
    if grant.client_ephemeral_pub.is_some() {
        conn.client_ephemeral_pub = grant.client_ephemeral_pub.clone();
    }
    Ok(grant)
}

fn is_grant_scope_mismatch_error(err: &BifrostError) -> bool {
    matches!(err, BifrostError::Network(msg)
        if msg.contains("open_call failed with status 403")
            && msg.contains("grant_scope_mismatch"))
}

fn is_stale_grant_crypto_error(result: &CallResult) -> bool {
    if let Some(ref stderr) = result.stderr {
        return stderr.contains("missing grant shared secret");
    }
    false
}

fn shell_scope_upgrade_error(conn: &LocalConnection) -> BifrostError {
    if conn.auth_method.as_deref() == Some("ssh_publickey")
        && conn.ssh_key_source.is_some()
        && conn.device_code.is_some()
    {
        return BifrostError::Config(
            "saved remote authorization does not allow shell.exec. Re-run the command to trigger a fresh SSH authorization prompt on the remote device."
                .to_string(),
        );
    }

    BifrostError::Config(
        "saved remote authorization is read-only and does not allow shell.exec. Run `bifrost remote connect <pair-code>` again and approve shell access on the remote device."
            .to_string(),
    )
}

fn derive_open_call_key(
    shared_secret: &[u8],
    grant_id: &str,
    caller_ephemeral_pub: &str,
    client_ephemeral_pub: &str,
    command_kind: CommandKind,
) -> bifrost_core::Result<[u8; 32]> {
    let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, grant_id.as_bytes());
    let prk = salt.extract(shared_secret);
    let caller_pub = decode_base64_or_raw(caller_ephemeral_pub);
    let client_pub = decode_base64_or_raw(client_ephemeral_pub);
    let info = [
        OPEN_CALL_HKDF_INFO_PREFIX,
        caller_pub.as_slice(),
        client_pub.as_slice(),
        command_kind.as_str().as_bytes(),
    ];
    let okm = prk
        .expand(&info, HkdfLen(32))
        .map_err(|_| BifrostError::Config("derive open_call session key failed".to_string()))?;
    let mut key = [0u8; 32];
    okm.fill(&mut key)
        .map_err(|_| BifrostError::Config("fill open_call session key failed".to_string()))?;
    Ok(key)
}

fn derive_call_event_key(
    shared_secret: &[u8],
    call_id: &str,
    caller_ephemeral_pub: &str,
    client_ephemeral_pub: &str,
) -> bifrost_core::Result<[u8; 32]> {
    let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, call_id.as_bytes());
    let prk = salt.extract(shared_secret);
    let caller_pub = decode_base64_or_raw(caller_ephemeral_pub);
    let client_pub = decode_base64_or_raw(client_ephemeral_pub);
    let info = [
        CALL_EVENT_HKDF_INFO_PREFIX,
        caller_pub.as_slice(),
        client_pub.as_slice(),
    ];
    let okm = prk
        .expand(&info, HkdfLen(32))
        .map_err(|_| BifrostError::Config("derive call event session key failed".to_string()))?;
    let mut key = [0u8; 32];
    okm.fill(&mut key)
        .map_err(|_| BifrostError::Config("fill call event session key failed".to_string()))?;
    Ok(key)
}

fn decode_base64_or_raw(value: &str) -> Vec<u8> {
    if value.is_empty() {
        return Vec::new();
    }

    match base64::engine::general_purpose::STANDARD.decode(value) {
        Ok(decoded) if !decoded.is_empty() => decoded,
        _ => value.as_bytes().to_vec(),
    }
}

fn short_fingerprint(bytes: &[u8]) -> String {
    digest(&SHA256, bytes).as_ref()[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn encrypt_remote_command(
    command_kind: CommandKind,
    command: Option<&str>,
    args_json: Option<&str>,
    query: Option<&CanonicalQueryCommand>,
    shell_exec: Option<&ShellExecPayload>,
    grant_id: &str,
    transport: &OpenCallTransportContext,
) -> bifrost_core::Result<EncryptedPayload> {
    let key_bytes = derive_open_call_key(
        &transport.shared_secret,
        grant_id,
        &transport.caller_ephemeral_pub,
        &transport.client_ephemeral_pub,
        command_kind,
    )?;
    debug!(
        grant_id = %grant_id,
        command_kind = %command_kind.as_str(),
        shared_secret_fp = %short_fingerprint(&transport.shared_secret),
        open_call_key_fp = %short_fingerprint(&key_bytes),
        "derived caller open_call encryption key"
    );
    let unbound = UnboundKey::new(&CHACHA20_POLY1305, &key_bytes).map_err(|_| {
        BifrostError::Config("initialize open_call command encryption failed".to_string())
    })?;
    let key = LessSafeKey::new(unbound);
    let mut nonce = [0u8; NONCE_LEN];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| BifrostError::Config("generate open_call nonce failed".to_string()))?;

    let plaintext = serde_json::to_vec(&CommandEnvelope {
        kind: command_kind.as_str().to_string(),
        command: command.unwrap_or_default().to_string(),
        args_json: args_json.map(ToOwned::to_owned),
        query: query.cloned(),
        exec_mode: shell_exec.map(|payload| payload.exec_mode.clone()),
        argv: shell_exec.and_then(|payload| payload.argv.clone()),
        command_text: shell_exec.and_then(|payload| payload.command_text.clone()),
        cwd: shell_exec.and_then(|payload| payload.cwd.clone()),
        env: shell_exec.and_then(|payload| payload.env.clone()),
        timeout_ms: shell_exec.and_then(|payload| payload.timeout_ms),
    })
    .map_err(|e| {
        BifrostError::Config(format!("serialize encrypted command payload failed: {e}"))
    })?;
    let mut sealed = plaintext;
    key.seal_in_place_append_tag(
        Nonce::assume_unique_for_key(nonce),
        Aad::empty(),
        &mut sealed,
    )
    .map_err(|_| BifrostError::Config("encrypt remote command payload failed".to_string()))?;
    let tag = sealed.split_off(sealed.len().saturating_sub(16));

    Ok(EncryptedPayload {
        version: ENCRYPTED_OPEN_CALL_VERSION,
        nonce: base64::engine::general_purpose::STANDARD.encode(nonce),
        ciphertext: base64::engine::general_purpose::STANDARD.encode(sealed),
        tag: base64::engine::general_purpose::STANDARD.encode(tag),
        aad: None,
    })
}

fn decrypt_payload_bytes(
    payload: &EncryptedPayload,
    key_bytes: &[u8; 32],
    aad_bytes: Option<&[u8]>,
) -> bifrost_core::Result<Vec<u8>> {
    let nonce_raw = base64::engine::general_purpose::STANDARD
        .decode(&payload.nonce)
        .map_err(|e| BifrostError::Config(format!("decode encrypted payload nonce failed: {e}")))?;
    let nonce: [u8; NONCE_LEN] = nonce_raw.try_into().map_err(|_| {
        BifrostError::Config("encrypted payload nonce must be 12 bytes".to_string())
    })?;

    let mut sealed = base64::engine::general_purpose::STANDARD
        .decode(&payload.ciphertext)
        .map_err(|e| {
            BifrostError::Config(format!("decode encrypted payload ciphertext failed: {e}"))
        })?;
    let tag = base64::engine::general_purpose::STANDARD
        .decode(&payload.tag)
        .map_err(|e| BifrostError::Config(format!("decode encrypted payload tag failed: {e}")))?;
    sealed.extend_from_slice(&tag);

    let unbound = UnboundKey::new(&CHACHA20_POLY1305, key_bytes).map_err(|_| {
        BifrostError::Config("initialize encrypted payload decrypt key failed".to_string())
    })?;
    let key = LessSafeKey::new(unbound);
    let plaintext = match aad_bytes {
        Some(aad_bytes) => key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(aad_bytes),
                &mut sealed,
            )
            .map_err(|_| {
                BifrostError::Config("encrypted payload authentication failed".to_string())
            })?,
        None => key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::empty(),
                &mut sealed,
            )
            .map_err(|_| {
                BifrostError::Config("encrypted payload authentication failed".to_string())
            })?,
    };
    Ok(plaintext.to_vec())
}

fn decrypt_frame_chunk(
    transport: &OpenCallTransportContext,
    call_id: &str,
    envelope: &Value,
) -> bifrost_core::Result<String> {
    let payload = EncryptedPayload {
        version: envelope
            .get("version")
            .and_then(|v| v.as_u64())
            .unwrap_or(2) as u32,
        nonce: envelope
            .get("nonce")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        ciphertext: envelope
            .get("ciphertext")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        tag: envelope
            .get("tag")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        aad: envelope
            .get("aad")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok()),
    };
    if payload.nonce.is_empty() || payload.tag.is_empty() {
        return Ok(payload.ciphertext);
    }

    let key = derive_call_event_key(
        &transport.shared_secret,
        call_id,
        &transport.caller_ephemeral_pub,
        &transport.client_ephemeral_pub,
    )?;
    let aad_bytes = if let Some(aad) = &payload.aad {
        Some(
            serde_json::to_vec(aad)
                .map_err(|e| BifrostError::Config(format!("serialize frame aad failed: {e}")))?,
        )
    } else {
        None
    };
    let plaintext = decrypt_payload_bytes(&payload, &key, aad_bytes.as_deref())?;
    let frame: EncryptedFramePayload = serde_json::from_slice(&plaintext)
        .map_err(|e| BifrostError::Config(format!("decode encrypted frame payload failed: {e}")))?;
    Ok(frame.chunk)
}

fn decrypt_exit_payload(
    transport: &OpenCallTransportContext,
    call_id: &str,
    payload: &EncryptedPayload,
) -> bifrost_core::Result<EncryptedExitPayload> {
    let key = derive_call_event_key(
        &transport.shared_secret,
        call_id,
        &transport.caller_ephemeral_pub,
        &transport.client_ephemeral_pub,
    )?;
    let aad_bytes = if let Some(aad) = &payload.aad {
        Some(
            serde_json::to_vec(aad)
                .map_err(|e| BifrostError::Config(format!("serialize exit aad failed: {e}")))?,
        )
    } else {
        None
    };
    let plaintext = decrypt_payload_bytes(payload, &key, aad_bytes.as_deref())?;
    serde_json::from_slice(&plaintext)
        .map_err(|e| BifrostError::Config(format!("decode encrypted exit payload failed: {e}")))
}

pub fn handle_remote_command(opts: RemoteOptions) -> bifrost_core::Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| BifrostError::Config(format!("failed to build tokio runtime: {e}")))?;

    rt.block_on(async_handle_remote_command(opts))
}

async fn async_handle_remote_command(opts: RemoteOptions) -> bifrost_core::Result<()> {
    let caller = CallerRelayClient::new(&opts.relay_url);
    let hostname = get_hostname();
    let username = get_username();
    let caller_fingerprint = generate_caller_fingerprint(&username, &hostname);
    let caller_info = CallerInfo {
        fingerprint: caller_fingerprint.clone(),
        display_name: Some(hostname.clone()),
        user_agent: Some(CALLER_USER_AGENT.to_string()),
        platform: Some(std::env::consts::OS.to_string()),
        hostname: Some(hostname),
        username: Some(username),
    };

    if let RemoteCommands::Connect {
        pair_code,
        ssh_key,
        device_code,
    } = &opts.action
    {
        if let Some(ssh_key) = ssh_key {
            return handle_connect_with_ssh(
                &caller,
                ssh_key,
                device_code.as_deref(),
                &caller_info,
                &opts.relay_url,
            )
            .await;
        }

        let pair_code = pair_code.as_deref().ok_or_else(|| {
            BifrostError::Config(
                "either <pair_code> or --ssh-key is required for `bifrost remote connect`"
                    .to_string(),
            )
        })?;
        return handle_connect(&caller, pair_code, &caller_info, &opts.relay_url).await;
    }

    let mut connections = load_connections()?;

    if let RemoteCommands::Disconnect { all, grant_id } = &opts.action {
        return handle_disconnect(
            &caller,
            &connections,
            opts.client_id.as_deref(),
            *all,
            grant_id.as_deref(),
        )
        .await;
    }

    let mut conn = resolve_local_connection(&connections, opts.client_id.as_deref())?;
    let caller_fingerprint = if conn.caller_fingerprint.is_empty() {
        caller_fingerprint
    } else {
        conn.caller_fingerprint.clone()
    };

    ensure_remote_command_confirmed(&opts.action)?;
    let command = build_remote_command(&opts.action);

    let grant = caller
        .find_reusable_grant(&conn.client_instance_id, &caller_fingerprint)
        .await?;

    let grant = match grant {
        Some(g) => g,
        None => {
            let conn_label = if conn.device_name.is_empty() {
                &conn.client_instance_id
            } else {
                &conn.device_name
            };
            eprintln!(
                "{}",
                format!(
                    "✗ Authorization for '{}' expired or revoked on the relay.",
                    conn_label
                )
                .bright_red()
            );
            connections.retain(|c| {
                !(c.client_instance_id == conn.client_instance_id && c.relay_url == conn.relay_url)
            });
            if let Err(error) = save_connections(&connections) {
                warn!(error = %error, "failed to remove stale connection");
            } else {
                eprintln!(
                    "  {} Stale connection removed from local cache.",
                    "→".bright_yellow()
                );
            }
            eprintln!(
                "  {} Run `bifrost remote connect <pair-code>` to re-authorize.",
                "→".bright_yellow()
            );
            std::process::exit(1);
        }
    };

    let grant = reconcile_reusable_grant_for_transport(&mut conn, grant)?;
    if let Some(existing) = connections.iter_mut().find(|existing| {
        existing.client_instance_id == conn.client_instance_id
            && existing.relay_url == conn.relay_url
    }) {
        *existing = conn.clone();
        save_connections(&connections)?;
    }

    debug!(grant_id = %grant.grant_id, "found reusable grant");
    if should_print_remote_progress_banner(std::io::stdout().is_terminal()) {
        println!(
            "{}",
            format!(
                "✓ Using authorization (grant: {})",
                &grant.grant_id[..grant.grant_id.len().min(8)]
            )
            .bright_green()
        );
    }

    let mut transport = merge_transport_context(&conn, &grant)?;
    let mut active_grant_id = grant.grant_id.clone();
    let command_summary = build_open_call_command_summary(&command);
    let command_encrypted = encrypt_remote_command(
        command.kind,
        command.command.as_deref(),
        command.args_json.as_deref(),
        command.query.as_ref(),
        command.shell_exec.as_ref(),
        &active_grant_id,
        &transport,
    )?;

    let call_result = caller
        .open_call(&OpenCallRequest {
            grant_id: grant.grant_id.clone(),
            client_instance_id: conn.client_instance_id.clone(),
            caller_fingerprint: caller_fingerprint.clone(),
            command_summary: command_summary.clone(),
            command_kind: command.kind,
            command_encrypted,
        })
        .await;

    let call_result = match call_result {
        Ok(result) => result,
        Err(err)
            if command.kind == CommandKind::ShellExec
                && is_grant_scope_mismatch_error(&err)
                && conn.auth_method.as_deref() == Some("ssh_publickey")
                && conn.ssh_key_source.is_some()
                && conn.device_code.is_some() =>
        {
            println!(
                "{}",
                "→ Saved authorization is missing shell.exec scope; requesting fresh SSH approval..."
                    .bright_yellow()
            );
            handle_connect_with_ssh(
                &caller,
                conn.ssh_key_source.as_deref().unwrap_or_default(),
                conn.device_code.as_deref(),
                &caller_info,
                &conn.relay_url,
            )
            .await?;

            let reloaded_connections = load_connections()?;
            conn = resolve_local_connection(&reloaded_connections, Some(&conn.client_instance_id))?;
            let refreshed_fingerprint = if conn.caller_fingerprint.is_empty() {
                caller_fingerprint.clone()
            } else {
                conn.caller_fingerprint.clone()
            };
            let refreshed_grant = caller
                .find_reusable_grant(&conn.client_instance_id, &refreshed_fingerprint)
                .await?
                .ok_or_else(|| {
                    BifrostError::Config(
                        "fresh SSH authorization was approved but no reusable grant was returned"
                            .to_string(),
                    )
                })?;
            let refreshed_grant =
                reconcile_reusable_grant_for_transport(&mut conn, refreshed_grant)?;
            let refreshed_transport = merge_transport_context(&conn, &refreshed_grant)?;
            let refreshed_command_encrypted = encrypt_remote_command(
                command.kind,
                command.command.as_deref(),
                command.args_json.as_deref(),
                command.query.as_ref(),
                command.shell_exec.as_ref(),
                &refreshed_grant.grant_id,
                &refreshed_transport,
            )?;
            let retried = caller
                .open_call(&OpenCallRequest {
                    grant_id: refreshed_grant.grant_id.clone(),
                    client_instance_id: conn.client_instance_id.clone(),
                    caller_fingerprint: refreshed_fingerprint,
                    command_summary: command_summary.clone(),
                    command_kind: command.kind,
                    command_encrypted: refreshed_command_encrypted,
                })
                .await?;
            active_grant_id = refreshed_grant.grant_id;
            transport = refreshed_transport;
            retried
        }
        Err(err)
            if command.kind == CommandKind::ShellExec && is_grant_scope_mismatch_error(&err) =>
        {
            return Err(shell_scope_upgrade_error(&conn));
        }
        Err(err) => return Err(err),
    };

    debug!(call_id = %call_result.call_id, grant_id = %active_grant_id, "call opened, subscribing to events");
    if should_print_remote_progress_banner(std::io::stdout().is_terminal()) {
        println!("{}", "→ Executing command on remote device...".dimmed());
    }

    let result = tokio::select! {
        result = caller.subscribe_call_events(
            &call_result.call_id,
            &call_result.relay_token,
            &transport,
            &command.render,
            CALL_EVENT_TIMEOUT_SECS,
        ) => result?,
        _ = wait_for_remote_call_cancel_signal() => {
            eprintln!("{}", "→ Cancellation requested, notifying remote device...".bright_yellow());
            let cancel_requested = match caller.cancel_call(&call_result.call_id, &call_result.relay_token).await {
                Ok(()) => true,
                Err(err) => {
                    warn!(call_id = %call_result.call_id, error = %err, "failed to send remote call cancel");
                    false
                }
            };
            match tokio::time::timeout(
                Duration::from_secs(CANCEL_SETTLE_TOTAL_TIMEOUT_SECS),
                caller.settle_cancelled_call(
                    &call_result.call_id,
                    &call_result.relay_token,
                    &transport,
                    &command.render,
                    cancel_requested,
                ),
            )
            .await
            {
                Ok(result) => result?,
                Err(_) => {
                    warn!(
                        call_id = %call_result.call_id,
                        "cancel settle exceeded total timeout, synthesizing cancelled result"
                    );
                    synthesized_cancelled_result()
                }
            }
        }
    };

    // If the remote client lost its grant crypto (e.g. data corruption / reinstall),
    // the call will fail with a "missing grant shared secret" error.
    // Treat this as an expired/revoked grant: clean up the stale local connection.
    if is_stale_grant_crypto_error(&result) {
        let conn_label = if conn.device_name.is_empty() {
            &conn.client_instance_id
        } else {
            &conn.device_name
        };
        eprintln!(
            "{}",
            format!(
                "✗ Authorization for '{}' expired or revoked on the relay.",
                conn_label
            )
            .bright_red()
        );
        connections.retain(|c| {
            !(c.client_instance_id == conn.client_instance_id && c.relay_url == conn.relay_url)
        });
        if let Err(error) = save_connections(&connections) {
            warn!(error = %error, "failed to remove stale connection");
        } else {
            eprintln!(
                "  {} Stale connection removed from local cache.",
                "→".bright_yellow()
            );
        }
        eprintln!(
            "  {} Run `bifrost remote connect <pair-code>` to re-authorize.",
            "→".bright_yellow()
        );
        std::process::exit(1);
    }

    print_remote_result(&command, &result);

    if result.exit_code != 0 {
        std::process::exit(result.exit_code);
    }

    Ok(())
}

fn ensure_remote_command_confirmed(action: &RemoteCommands) -> bifrost_core::Result<()> {
    if let RemoteCommands::Traffic {
        action:
            RemoteTrafficCommands::Clear {
                ids: None,
                yes: false,
            },
    } = action
    {
        if !std::io::stdin().is_terminal() {
            return Err(BifrostError::Config(
                "Use --yes to confirm clearing all remote traffic records in non-interactive mode"
                    .to_string(),
            ));
        }
        print!("Clear ALL remote traffic records? This cannot be undone. [y/N] ");
        std::io::stdout().flush().ok();
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).ok();
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Cancelled.");
            std::process::exit(0);
        }
    }

    Ok(())
}

async fn handle_connect(
    caller: &CallerRelayClient,
    pair_code: &str,
    caller_info: &CallerInfo,
    relay_url: &str,
) -> bifrost_core::Result<()> {
    let pending_transport = PendingCallerTransport::generate()?;
    println!(
        "{}",
        format!(
            "→ Initiating pairing with code {}...",
            pair_code.bright_cyan()
        )
        .dimmed()
    );

    let pairing_result = start_pairing_with_retry(
        caller,
        &StartPairingRequest {
            pair_code: pair_code.to_string(),
            caller_info: caller_info.clone(),
            caller_ephemeral_pub: pending_transport.caller_ephemeral_pub.clone(),
        },
    )
    .await?;

    println!(
        "{}",
        "⏳ Waiting for approval on the remote device...".bright_yellow()
    );

    let approval = caller.watch_pairing(&pairing_result.pairing_id).await?;

    match approval.status.as_str() {
        "approved" => {
            let grant_id = approval.grant_id.unwrap_or_else(|| "unknown".to_string());
            let client_instance_id = approval.client_instance_id.unwrap_or_default();
            let device_name = approval
                .device_name
                .unwrap_or_else(|| "unknown".to_string());
            let platform = approval.platform.unwrap_or_else(|| "unknown".to_string());
            let grant_mode = approval.grant_mode.unwrap_or_else(|| "unknown".to_string());
            let client_ephemeral_pub = approval.client_ephemeral_pub.as_deref().ok_or_else(|| {
                BifrostError::Config(
                    "pairing succeeded but relay did not return client_ephemeral_pub required for encrypted remote commands".to_string(),
                )
            })?;
            let transport = pending_transport.finalize(client_ephemeral_pub)?;

            let new_conn = LocalConnection {
                client_instance_id: client_instance_id.clone(),
                device_name: device_name.clone(),
                platform: platform.clone(),
                relay_url: relay_url.to_string(),
                grant_id: grant_id.clone(),
                grant_mode,
                caller_fingerprint: caller_info.fingerprint.clone(),
                connected_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64,
                auth_method: Some("pair_code".to_string()),
                ssh_key_fingerprint: None,
                ssh_key_source: None,
                device_code: None,
                transport_context_version: Some(TRANSPORT_CONTEXT_VERSION),
                caller_ephemeral_pub: Some(transport.caller_ephemeral_pub),
                client_ephemeral_pub: Some(transport.client_ephemeral_pub),
                shared_secret_encrypted: Some(transport.shared_secret_encrypted),
            };

            let mut connections = load_connections().unwrap_or_default();
            upsert_local_connection(&mut connections, new_conn);
            save_connections(&connections)?;

            let short_id = &client_instance_id[..client_instance_id.len().min(12)];
            println!(
                "{}",
                format!(
                    "✓ Connected! Authorization granted (grant: {})",
                    &grant_id[..grant_id.len().min(8)]
                )
                .bright_green()
            );
            println!(
                "{}",
                format!("  Device: {device_name} ({platform})").dimmed()
            );
            println!(
                "{}",
                format!(
                    "  You can now run commands like: bifrost remote status --client-id {short_id}"
                )
                .dimmed()
            );
            println!(
                "{}",
                "  Pair codes are one-time only. After connect succeeds, use remote status or other read-only remote commands instead of reconnecting with the same code."
                    .dimmed()
            );
            Ok(())
        }
        "rejected" => {
            println!("{}", "✗ Pairing was rejected.".bright_red());
            Err(BifrostError::Config("pairing rejected".to_string()))
        }
        other => {
            println!(
                "{}",
                format!("✗ Pairing ended with status: {other}").bright_red()
            );
            Err(BifrostError::Config(format!(
                "pairing failed with status: {other}"
            )))
        }
    }
}

async fn handle_connect_with_ssh(
    caller: &CallerRelayClient,
    ssh_key: &str,
    device_code_override: Option<&str>,
    caller_info: &CallerInfo,
    relay_url: &str,
) -> bifrost_core::Result<()> {
    let pending_transport = PendingCallerTransport::generate()?;
    let loaded_key = load_ssh_key(ssh_key, device_code_override)?;

    println!(
        "{}",
        format!(
            "→ Initiating SSH authorization with device code {}...",
            loaded_key.device_code.bright_cyan()
        )
        .dimmed()
    );

    let challenge = caller
        .request_ssh_challenge(&loaded_key.device_code)
        .await?;
    let timestamp = now_millis();
    let payload = build_ssh_connect_signature_payload(
        &challenge.challenge,
        &challenge.challenge_id,
        &loaded_key.device_code,
        timestamp,
    );
    let signature = base64::engine::general_purpose::STANDARD
        .encode(loaded_key.key_pair.sign(payload.as_bytes()).as_ref());

    let response = caller
        .ssh_connect(&SshConnectRequest {
            device_code: loaded_key.device_code.clone(),
            challenge_id: challenge.challenge_id,
            signature,
            timestamp,
            caller_info: Some(caller_info.clone()),
            caller_ephemeral_pub: Some(pending_transport.caller_ephemeral_pub.clone()),
        })
        .await?;

    println!(
        "{}",
        "⏳ Waiting for SSH authorization confirmation on the remote device...".bright_yellow()
    );

    let result = caller
        .watch_ssh_connect_result(&response.connect_id, &response.relay_token)
        .await?;

    match result.status.as_str() {
        "approved" => {
            let grant_id = result.grant_id.unwrap_or_else(|| "unknown".to_string());
            let client_instance_id = result.client_instance_id.ok_or_else(|| {
                BifrostError::Config(
                    "ssh connect succeeded but relay did not return client_instance_id".to_string(),
                )
            })?;
            let grant_mode = result.grant_mode.unwrap_or_else(|| "permanent".to_string());
            let caller_fingerprint = result
                .caller_fingerprint
                .unwrap_or_else(|| loaded_key.ssh_key_fingerprint.clone());
            let client_ephemeral_pub = result.client_ephemeral_pub.as_deref().ok_or_else(|| {
                BifrostError::Config(
                    "ssh connect succeeded but relay did not return client_ephemeral_pub required for encrypted remote commands".to_string(),
                )
            })?;
            let transport = pending_transport.finalize(client_ephemeral_pub)?;

            let new_conn = LocalConnection {
                client_instance_id: client_instance_id.clone(),
                device_name: format!("SSH {}", loaded_key.device_code),
                platform: "unknown".to_string(),
                relay_url: relay_url.to_string(),
                grant_id: grant_id.clone(),
                grant_mode,
                caller_fingerprint,
                connected_at: now_millis(),
                auth_method: Some("ssh_publickey".to_string()),
                ssh_key_fingerprint: Some(loaded_key.ssh_key_fingerprint.clone()),
                ssh_key_source: Some(loaded_key.source_label.clone()),
                device_code: Some(loaded_key.device_code.clone()),
                transport_context_version: Some(TRANSPORT_CONTEXT_VERSION),
                caller_ephemeral_pub: Some(transport.caller_ephemeral_pub),
                client_ephemeral_pub: Some(transport.client_ephemeral_pub),
                shared_secret_encrypted: Some(transport.shared_secret_encrypted),
            };

            let mut connections = load_connections().unwrap_or_default();
            upsert_local_connection(&mut connections, new_conn);
            save_connections(&connections)?;

            let short_id = &client_instance_id[..client_instance_id.len().min(12)];
            println!(
                "{}",
                format!(
                    "✓ Connected with SSH key (grant: {})",
                    &grant_id[..grant_id.len().min(8)]
                )
                .bright_green()
            );
            println!(
                "{}",
                format!("  Device Code: {}", loaded_key.device_code).dimmed()
            );
            println!(
                "{}",
                format!("  Key Source: {}", loaded_key.source_label).dimmed()
            );
            println!(
                "{}",
                format!(
                    "  You can now run commands like: bifrost remote status --client-id {short_id}"
                )
                .dimmed()
            );
            Ok(())
        }
        "rejected" => Err(BifrostError::Config(
            result
                .reason
                .unwrap_or_else(|| "ssh connect rejected".to_string()),
        )),
        other => Err(BifrostError::Config(format!(
            "ssh connect failed with status: {other}"
        ))),
    }
}

async fn start_pairing_with_retry(
    caller: &CallerRelayClient,
    req: &StartPairingRequest,
) -> bifrost_core::Result<StartPairingResponse> {
    let mut retry_idx = 0usize;

    loop {
        match caller.start_pairing(req).await {
            Ok(result) => return Ok(result),
            Err(err) if is_retryable_start_pairing_overload(&err) => {
                if retry_idx >= START_PAIRING_OVERLOAD_RETRY_DELAYS_MS.len() {
                    return Err(start_pairing_overload_error());
                }

                let delay_ms = START_PAIRING_OVERLOAD_RETRY_DELAYS_MS[retry_idx];
                retry_idx += 1;

                warn!(
                    attempt = retry_idx,
                    delay_ms, "relay overload protection triggered during start_pairing, retrying"
                );
                println!(
                    "{}",
                    format!("↻ Relay is temporarily busy, retrying pairing in {delay_ms}ms...")
                        .dimmed()
                );
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
            Err(err) => return Err(err),
        }
    }
}

fn is_retryable_start_pairing_overload(err: &BifrostError) -> bool {
    let BifrostError::Network(message) = err else {
        return false;
    };

    let lower = message.to_ascii_lowercase();
    lower.contains("start_pairing failed with status 503") && lower.contains("overload-protect")
}

fn start_pairing_overload_error() -> BifrostError {
    BifrostError::Network(
        "pairing service is temporarily busy (relay overload protection triggered). Please wait a few seconds and run `bifrost remote connect <pair-code>` again.".to_string(),
    )
}

async fn handle_disconnect(
    caller: &CallerRelayClient,
    connections: &[LocalConnection],
    client_id: Option<&str>,
    all: bool,
    grant_id: Option<&str>,
) -> bifrost_core::Result<()> {
    if let Some(gid) = grant_id {
        let caller_fingerprint = connections
            .iter()
            .find(|conn| conn.grant_id == gid)
            .map(|conn| conn.caller_fingerprint.as_str())
            .unwrap_or("");
        let outcome = caller.delete_grant(gid, caller_fingerprint).await?;
        let mut conns = connections.to_vec();
        conns.retain(|c| c.grant_id != gid);
        save_connections(&conns)?;
        let short_id = &gid[..gid.len().min(8)];
        match outcome {
            DeleteGrantOutcome::Deleted => {
                println!("{}", format!("✓ Grant {short_id} revoked.").bright_green())
            }
            DeleteGrantOutcome::AlreadyMissing => println!(
                "{}",
                format!("✓ Grant {short_id} was already missing on relay; local record removed.")
                    .bright_green()
            ),
        }
        return Ok(());
    }

    if all {
        if connections.is_empty() {
            println!("{}", "No saved connections.".dimmed());
            return Ok(());
        }

        println!(
            "{}",
            format!("Revoking {} connection(s)…", connections.len()).bright_yellow()
        );

        let mut remaining = connections.to_vec();
        let mut deleted = 0usize;
        let total = remaining.len();
        let mut to_remove = Vec::new();

        for (i, conn) in connections.iter().enumerate() {
            let short_id = &conn.grant_id[..conn.grant_id.len().min(12)];
            match revoke_all_matching_grants(
                caller,
                &conn.client_instance_id,
                &conn.caller_fingerprint,
            )
            .await
            {
                Ok(_revoked) => {
                    deleted += 1;
                    to_remove.push(i);
                    println!(
                        "  {} {} ({})",
                        "✓".bright_green(),
                        short_id,
                        conn.device_name
                    );
                }
                Err(e) => {
                    eprintln!(
                        "  {} {} ({}) — {}",
                        "✗".bright_red(),
                        short_id,
                        conn.device_name,
                        e
                    );
                }
            }
        }

        for i in to_remove.into_iter().rev() {
            remaining.remove(i);
        }
        save_connections(&remaining)?;

        println!(
            "{}",
            format!("Revoked {deleted}/{total} connection(s).").bright_green()
        );
        return Ok(());
    }

    let conn = resolve_local_connection(connections, client_id)?;
    let short_id = &conn.grant_id[..conn.grant_id.len().min(12)];

    revoke_all_matching_grants(caller, &conn.client_instance_id, &conn.caller_fingerprint).await?;

    let mut conns = connections.to_vec();
    conns.retain(|c| {
        !(c.client_instance_id == conn.client_instance_id && c.relay_url == conn.relay_url)
    });
    save_connections(&conns)?;

    println!(
        "{}",
        format!(
            "✓ Disconnected from {} (grant: {short_id})",
            conn.device_name
        )
        .bright_green()
    );
    Ok(())
}

async fn revoke_all_matching_grants(
    caller: &CallerRelayClient,
    client_instance_id: &str,
    caller_fingerprint: &str,
) -> bifrost_core::Result<usize> {
    let mut revoked = 0usize;
    let mut seen = HashSet::new();

    loop {
        let grant = caller
            .find_reusable_grant(client_instance_id, caller_fingerprint)
            .await?;
        let Some(grant) = grant else {
            break;
        };

        if !seen.insert(grant.grant_id.clone()) {
            return Err(BifrostError::Config(
                "relay reusable grant lookup returned the same grant repeatedly during disconnect cleanup"
                    .to_string(),
            ));
        }

        match caller
            .delete_grant(&grant.grant_id, caller_fingerprint)
            .await?
        {
            DeleteGrantOutcome::Deleted | DeleteGrantOutcome::AlreadyMissing => {
                revoked += 1;
            }
        }
    }

    Ok(revoked)
}

fn build_remote_command(action: &RemoteCommands) -> BuiltRemoteCommand {
    match action {
        RemoteCommands::Connect { .. } => unreachable!("connect handled separately"),
        RemoteCommands::Disconnect { .. } => unreachable!("disconnect handled separately"),
        RemoteCommands::Status => BuiltRemoteCommand {
            kind: CommandKind::QueryReadonly,
            label: "status".to_string(),
            command: Some("status".to_string()),
            args_json: None,
            query: None,
            shell_exec: None,
            render: RemoteRenderMode::Raw,
            streaming_prefs: None,
        },
        RemoteCommands::Command { action } => match action {
            RemoteCommandCommands::Exec(exec_args) => build_remote_shell_exec_command(exec_args),
        },
        RemoteCommands::Shell { .. } => unreachable!("shell handled separately"),
        RemoteCommands::Grant { .. } => unreachable!("grant handled separately"),
        RemoteCommands::Search(search_args) => {
            let query = CanonicalQueryCommand::Search(command_search_args(search_args));
            let command_id = query.command_id().to_string();
            BuiltRemoteCommand {
                kind: CommandKind::QueryReadonly,
                label: "search.stream".to_string(),
                command: Some(command_id),
                args_json: query_args_json(&query),
                query: Some(query),
                shell_exec: None,
                render: RemoteRenderMode::Search {
                    format: search_args.format.parse().unwrap_or(OutputFormat::Table),
                    no_color: search_args.no_color,
                    keyword: search_args.keyword.clone().unwrap_or_default(),
                    max_scan: search_args.max_scan,
                    max_results: search_args.max_results,
                },
                streaming_prefs: None,
            }
        }
        RemoteCommands::Traffic { action } => match action {
            RemoteTrafficCommands::List(list_args) => {
                let list_args = list_args.as_ref();
                let query = CanonicalQueryCommand::TrafficList(TrafficListArgs {
                    limit: Some(list_args.limit),
                    cursor: list_args.cursor,
                    direction: if list_args.direction == "forward" {
                        TrafficListDirection::Forward
                    } else {
                        TrafficListDirection::Backward
                    },
                    method: list_args.method.clone(),
                    status: list_args.status,
                    status_min: list_args.status_min,
                    status_max: list_args.status_max,
                    protocol: list_args.protocol.clone(),
                    host: list_args.host.clone(),
                    url: list_args.url.clone(),
                    path: list_args.path.clone(),
                    content_type: list_args.content_type.clone(),
                    client_ip: list_args.client_ip.clone(),
                    client_app: list_args.client_app.clone(),
                    has_rule_hit: list_args.has_rule_hit,
                    is_websocket: list_args.is_websocket,
                    is_sse: list_args.is_sse,
                    is_tunnel: list_args.is_tunnel,
                });
                let command_id = query.command_id().to_string();
                BuiltRemoteCommand {
                    kind: CommandKind::QueryReadonly,
                    label: "traffic.list".to_string(),
                    command: Some(command_id),
                    args_json: query_args_json(&query),
                    query: Some(query),
                    shell_exec: None,
                    render: RemoteRenderMode::TrafficList {
                        format: list_args.format.parse().unwrap_or(OutputFormat::Table),
                        no_color: list_args.no_color,
                    },
                    streaming_prefs: None,
                }
            }
            RemoteTrafficCommands::Get {
                id,
                request_body,
                response_body,
                format,
                no_color,
            } => {
                let query = CanonicalQueryCommand::TrafficGet(TrafficGetArgs {
                    id: id.clone(),
                    request_body: *request_body,
                    response_body: *response_body,
                });
                let command_id = query.command_id().to_string();
                BuiltRemoteCommand {
                    kind: CommandKind::QueryReadonly,
                    label: "traffic.get".to_string(),
                    command: Some(command_id),
                    args_json: query_args_json(&query),
                    query: Some(query),
                    shell_exec: None,
                    render: RemoteRenderMode::TrafficGet {
                        format: format.parse().unwrap_or(OutputFormat::JsonPretty),
                        no_color: *no_color,
                    },
                    streaming_prefs: None,
                }
            }
            RemoteTrafficCommands::Search(search_args) => {
                let query = CanonicalQueryCommand::Search(command_search_args(search_args));
                let command_id = query.command_id().to_string();
                BuiltRemoteCommand {
                    kind: CommandKind::QueryReadonly,
                    label: "search.stream".to_string(),
                    command: Some(command_id),
                    args_json: query_args_json(&query),
                    query: Some(query),
                    shell_exec: None,
                    render: RemoteRenderMode::Search {
                        format: search_args.format.parse().unwrap_or(OutputFormat::Table),
                        no_color: search_args.no_color,
                        keyword: search_args.keyword.clone().unwrap_or_default(),
                        max_scan: search_args.max_scan,
                        max_results: search_args.max_results,
                    },
                    streaming_prefs: None,
                }
            }
            RemoteTrafficCommands::Clear { ids, yes: _ } => {
                let query = CanonicalQueryCommand::TrafficClear(TrafficClearArgs {
                    ids: ids.as_ref().map(|value| {
                        value
                            .split(',')
                            .map(|item| item.trim().to_string())
                            .collect()
                    }),
                });
                let command_id = query.command_id().to_string();
                BuiltRemoteCommand {
                    kind: CommandKind::QueryReadonly,
                    label: "traffic.clear".to_string(),
                    command: Some(command_id),
                    args_json: query_args_json(&query),
                    query: Some(query),
                    shell_exec: None,
                    render: RemoteRenderMode::Raw,
                    streaming_prefs: None,
                }
            }
        },
    }
}

fn build_open_call_command_summary(command: &BuiltRemoteCommand) -> OpenCallCommandSummary {
    OpenCallCommandSummary {
        command_preview: command.label.clone(),
        masked_args_json: command.args_json.clone(),
    }
}

fn build_remote_shell_exec_command(exec_args: &RemoteCommandExecArgs) -> BuiltRemoteCommand {
    let shell_exec = if let Some(shell_text) = &exec_args.shell_text {
        ShellExecPayload {
            exec_mode: "shell_text".to_string(),
            argv: None,
            command_text: Some(shell_text.clone()),
            cwd: exec_args.cwd.clone(),
            env: remote_shell_exec_env(&exec_args.env),
            timeout_ms: exec_args.timeout_ms,
        }
    } else {
        ShellExecPayload {
            exec_mode: "argv_exec".to_string(),
            argv: Some(exec_args.argv.clone()),
            command_text: None,
            cwd: exec_args.cwd.clone(),
            env: remote_shell_exec_env(&exec_args.env),
            timeout_ms: exec_args.timeout_ms,
        }
    };

    let preview = exec_args
        .shell_text
        .clone()
        .or_else(|| exec_args.argv.first().cloned())
        .unwrap_or_else(|| "shell.exec".to_string());

    let streaming_prefs = StreamingPrefs {
        stream: exec_args.stream,
        output_file: exec_args.output_file.as_ref().map(std::path::PathBuf::from),
        resume_call_id: exec_args.resume_call_id.clone(),
        no_verify_digest: exec_args.no_verify_digest,
    };

    BuiltRemoteCommand {
        kind: CommandKind::ShellExec,
        label: "shell.exec".to_string(),
        command: Some(preview),
        args_json: None,
        query: None,
        shell_exec: Some(shell_exec),
        render: RemoteRenderMode::Raw,
        streaming_prefs: Some(streaming_prefs),
    }
}

fn remote_shell_exec_env(env_pairs: &[(String, String)]) -> Option<BTreeMap<String, String>> {
    if env_pairs.is_empty() {
        return None;
    }

    Some(env_pairs.iter().cloned().collect())
}

fn query_args_json(query: &CanonicalQueryCommand) -> Option<String> {
    match query {
        CanonicalQueryCommand::Search(args) => serde_json::to_string(args).ok(),
        CanonicalQueryCommand::TrafficList(args) => serde_json::to_string(args).ok(),
        CanonicalQueryCommand::TrafficGet(args) => serde_json::to_string(args).ok(),
        CanonicalQueryCommand::TrafficClear(args) => serde_json::to_string(args).ok(),
    }
}

fn command_search_args(args: &RemoteSearchArgs) -> SearchArgs {
    let scope_all = !(args.url
        || args.headers
        || args.body
        || args.req_header
        || args.res_header
        || args.req_body
        || args.res_body);

    let mut filters = SearchFilters::default();
    if let Some(status) = &args.status {
        filters.status_ranges.push(status.clone());
    }
    if let Some(protocol) = &args.protocol {
        filters.protocols.push(protocol.to_lowercase());
    }
    if let Some(content_type) = &args.content_type {
        filters.content_types.push(content_type.clone());
    }
    if let Some(domain) = &args.domain {
        filters.domains.push(domain.clone());
    }
    if let Some(method) = &args.method {
        filters.conditions.push(FilterCondition {
            field: "method".to_string(),
            operator: "equals".to_string(),
            value: method.clone(),
        });
    }
    if let Some(host) = &args.host {
        filters.conditions.push(FilterCondition {
            field: "host".to_string(),
            operator: "contains".to_string(),
            value: host.clone(),
        });
    }
    if let Some(path) = &args.path {
        filters.conditions.push(FilterCondition {
            field: "path".to_string(),
            operator: "contains".to_string(),
            value: path.clone(),
        });
    }

    SearchArgs {
        keyword: args.keyword.clone().unwrap_or_default(),
        scope: SearchScope {
            request_body: args.body || args.req_body,
            response_body: args.body || args.res_body,
            request_headers: args.headers || args.req_header,
            response_headers: args.headers || args.res_header,
            url: args.url,
            websocket_messages: false,
            sse_events: false,
            all: scope_all,
        },
        filters,
        limit: Some(args.limit),
        max_scan: args.max_scan,
        max_results: args.max_results,
        ..SearchArgs::default()
    }
}

impl RemoteSearchSseDecoder {
    fn push_chunk(&mut self, chunk: &str) -> bifrost_core::Result<Vec<RemoteSearchEvent>> {
        self.partial_line.push_str(chunk);
        let mut events = Vec::new();

        while let Some(pos) = self.partial_line.find('\n') {
            let line = self.partial_line[..pos].trim_end_matches('\r').to_string();
            self.partial_line = self.partial_line[pos + 1..].to_string();

            if line.is_empty() {
                if self.event_name.is_empty() || self.data_lines.is_empty() {
                    self.event_name.clear();
                    self.data_lines.clear();
                    continue;
                }

                let data_text = self.data_lines.join("\n");
                let event = match self.event_name.as_str() {
                    "result" => serde_json::from_str::<SearchResultItem>(&data_text)
                        .map(|item| RemoteSearchEvent::Result(Box::new(item)))
                        .map_err(|e| {
                            BifrostError::Parse(format!(
                                "failed to parse remote search result event: {e}"
                            ))
                        })?,
                    "progress" => serde_json::from_str::<RemoteSearchProgressPayload>(&data_text)
                        .map(RemoteSearchEvent::Progress)
                        .map_err(|e| {
                            BifrostError::Parse(format!(
                                "failed to parse remote search progress event: {e}"
                            ))
                        })?,
                    "done" => serde_json::from_str::<RemoteSearchDonePayload>(&data_text)
                        .map(RemoteSearchEvent::Done)
                        .map_err(|e| {
                            BifrostError::Parse(format!(
                                "failed to parse remote search done event: {e}"
                            ))
                        })?,
                    "error" => serde_json::from_str::<RemoteSearchErrorPayload>(&data_text)
                        .map(RemoteSearchEvent::Error)
                        .map_err(|e| {
                            BifrostError::Parse(format!(
                                "failed to parse remote search error event: {e}"
                            ))
                        })?,
                    _ => {
                        self.event_name.clear();
                        self.data_lines.clear();
                        continue;
                    }
                };

                events.push(event);
                self.event_name.clear();
                self.data_lines.clear();
            } else if let Some(rest) = line.strip_prefix("event:") {
                self.event_name = rest.trim().to_string();
            } else if let Some(rest) = line.strip_prefix("data:") {
                self.data_lines.push(rest.trim().to_string());
            }
        }

        Ok(events)
    }
}

impl RemoteSearchRenderer {
    fn from_render_mode(render: &RemoteRenderMode) -> Option<Self> {
        match render {
            RemoteRenderMode::Search {
                format,
                no_color,
                keyword,
                max_scan,
                max_results,
            } => Some(Self {
                format: *format,
                keyword: keyword.clone(),
                max_scan: *max_scan,
                max_results: *max_results,
                use_color: !*no_color && std::io::stdout().is_terminal(),
                decoder: RemoteSearchSseDecoder::default(),
                printed_header: false,
                progress_visible: false,
                total_matched: 0,
                total_searched: 0,
                has_more: false,
                json_results: Vec::new(),
            }),
            _ => None,
        }
    }

    fn consume_chunk(&mut self, chunk: &str) -> bifrost_core::Result<()> {
        for event in self.decoder.push_chunk(chunk)? {
            self.render_event(event)?;
        }
        Ok(())
    }

    fn render_event(&mut self, event: RemoteSearchEvent) -> bifrost_core::Result<()> {
        match event {
            RemoteSearchEvent::Result(item) => match self.format {
                OutputFormat::Table => self.render_table_result(&item),
                OutputFormat::Compact => self.render_compact_result(&item),
                OutputFormat::Json | OutputFormat::JsonPretty => {
                    self.json_results.push(serde_json::json!({
                        "id": item.record.id,
                        "seq": item.record.seq,
                        "method": item.record.m,
                        "host": item.record.h,
                        "path": item.record.p,
                        "status": item.record.s,
                        "protocol": item.record.proto,
                        "request_size": item.record.req_sz,
                        "response_size": item.record.res_sz,
                        "duration_ms": item.record.dur,
                        "timestamp": item.record.ts,
                        "matches": item.matches.iter().map(|m| {
                            serde_json::json!({
                                "field": m.field,
                                "preview": m.preview,
                            })
                        }).collect::<Vec<_>>(),
                    }));
                    Ok(())
                }
            },
            RemoteSearchEvent::Progress(progress) => {
                self.total_searched = progress.total_searched;
                self.total_matched = progress.total_matched;
                if self.format == OutputFormat::Table && self.use_color {
                    eprint!(
                        "\r\x1b[90m  ⏳ Searching... {} records scanned, {} matched\x1b[0m\x1b[K",
                        render_search_number(progress.total_searched),
                        progress.total_matched,
                    );
                    self.progress_visible = true;
                }
                Ok(())
            }
            RemoteSearchEvent::Done(done) => {
                self.total_matched = done.total_matched;
                self.total_searched = done.total_searched;
                self.has_more = done.has_more;
                self.finish_render();
                Ok(())
            }
            RemoteSearchEvent::Error(error) => {
                if self.progress_visible && self.use_color {
                    eprint!("\r\x1b[K");
                }
                match self.format {
                    OutputFormat::Json | OutputFormat::JsonPretty => {
                        let output = serde_json::json!({
                            "error": error.message,
                            "results": self.json_results,
                            "total_matched": self.total_matched,
                            "total_searched": self.total_searched,
                            "has_more": self.has_more,
                        });
                        if self.format == OutputFormat::JsonPretty {
                            println!("{}", serde_json::to_string_pretty(&output).unwrap());
                        } else {
                            println!("{}", output);
                        }
                    }
                    OutputFormat::Table | OutputFormat::Compact => {
                        eprintln!("\x1b[31m✗\x1b[0m Search failed: {}", error.message);
                    }
                }
                Ok(())
            }
        }
    }

    fn render_table_result(&mut self, item: &SearchResultItem) -> bifrost_core::Result<()> {
        if self.progress_visible && self.use_color {
            eprint!("\r\x1b[K");
            self.progress_visible = false;
        }
        if !self.printed_header {
            println!();
            if self.use_color {
                println!(
                    "\x1b[1;37m{:>10}  {:>6}  {:>6}  {:7}  {:40}  {:46}  {:>10}  {:>8}\x1b[0m",
                    "SEQ", "STATUS", "METHOD", "PROTO", "HOST", "PATH", "SIZE", "TIME"
                );
            } else {
                println!(
                    "{:>10}  {:>6}  {:>6}  {:7}  {:40}  {:46}  {:>10}  {:>8}",
                    "SEQ", "STATUS", "METHOD", "PROTO", "HOST", "PATH", "SIZE", "TIME"
                );
            }
            println!("{}", "─".repeat(150));
            self.printed_header = true;
        }

        self.total_matched += 1;
        print_remote_search_table_row(item, &self.keyword, self.use_color);
        Ok(())
    }

    fn render_compact_result(&mut self, item: &SearchResultItem) -> bifrost_core::Result<()> {
        let r = &item.record;
        let status = if r.s == 0 {
            "...".to_string()
        } else {
            r.s.to_string()
        };

        if self.use_color {
            let status_color = match r.s {
                0 => "\x1b[90m",
                200..=299 => "\x1b[32m",
                300..=399 => "\x1b[33m",
                400..=499 => "\x1b[31m",
                500..=599 => "\x1b[1;31m",
                _ => "\x1b[37m",
            };
            println!(
                "\x1b[90m{:>10}\x1b[0m {}{}\x1b[0m {} \x1b[36m{}\x1b[0m{}",
                r.seq, status_color, status, r.m, r.h, r.p
            );
        } else {
            println!("{:>10} {} {} {}{}", r.seq, status, r.m, r.h, r.p);
        }
        Ok(())
    }

    fn finish_render(&mut self) {
        if self.progress_visible && self.use_color {
            eprint!("\r\x1b[K");
            self.progress_visible = false;
        }

        match self.format {
            OutputFormat::Table => {
                if self.total_matched == 0 {
                    println!(
                        "\x1b[33m⚠\x1b[0m No results found for '\x1b[1m{}\x1b[0m'",
                        self.keyword
                    );
                } else {
                    println!();
                }
                print_remote_search_summary(
                    &self.keyword,
                    self.max_scan,
                    self.max_results,
                    self.total_searched,
                    self.total_matched,
                    self.has_more,
                    self.use_color,
                );
            }
            OutputFormat::Compact => {}
            OutputFormat::Json | OutputFormat::JsonPretty => {
                let output = serde_json::json!({
                    "results": self.json_results,
                    "total_matched": self.total_matched,
                    "total_searched": self.total_searched,
                    "has_more": self.has_more,
                });
                if self.format == OutputFormat::JsonPretty {
                    println!("{}", serde_json::to_string_pretty(&output).unwrap());
                } else {
                    println!("{}", output);
                }
            }
        }
    }
}

fn truncate_search_str(s: &str, max_len: usize) -> String {
    let len = s.chars().count();
    if len <= max_len {
        return s.to_string();
    }

    let keep = max_len.saturating_sub(3);
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i >= keep {
            break;
        }
        out.push(ch);
    }
    if keep < max_len {
        out.push_str("...");
    }
    out
}

fn highlight_remote_search_keyword(text: &str, keyword: &str, use_color: bool) -> String {
    if !use_color || keyword.is_empty() {
        return text.to_string();
    }

    let lower_text = text.to_lowercase();
    let lower_keyword = keyword.to_lowercase();
    if !lower_text.contains(&lower_keyword) {
        return text.to_string();
    }

    let mut result = String::new();
    let mut last_end = 0;
    for (start, _) in lower_text.match_indices(&lower_keyword) {
        let Some(prefix) = text.get(last_end..start) else {
            return text.to_string();
        };
        result.push_str(prefix);
        result.push_str("\x1b[1;33m");
        let end = start + lower_keyword.len();
        let Some(highlighted) = text.get(start..end) else {
            return text.to_string();
        };
        result.push_str(highlighted);
        result.push_str("\x1b[0m");
        last_end = end;
    }
    let Some(rest) = text.get(last_end..) else {
        return text.to_string();
    };
    result.push_str(rest);
    result
}

fn render_search_size(bytes: usize) -> String {
    const KB: usize = 1024;
    const MB: usize = KB * 1024;

    if bytes >= MB {
        format!("{:.1}MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1}KB", bytes as f64 / KB as f64)
    } else {
        format!("{}B", bytes)
    }
}

fn render_search_duration(ms: u64) -> String {
    if ms == 0 {
        "...".to_string()
    } else if ms >= 1000 {
        format!("{:.2}s", ms as f64 / 1000.0)
    } else {
        format!("{}ms", ms)
    }
}

fn render_search_number(value: usize) -> String {
    if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}K", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

fn print_remote_search_table_row(item: &SearchResultItem, keyword: &str, use_color: bool) {
    let r = &item.record;
    let status_str = if r.s == 0 {
        "...".to_string()
    } else {
        r.s.to_string()
    };
    let (status_color, status_display) = if use_color {
        match r.s {
            0 => ("\x1b[90m", format!("{:>6}", status_str)),
            200..=299 => ("\x1b[32m", format!("{:>6}", status_str)),
            300..=399 => ("\x1b[33m", format!("{:>6}", status_str)),
            400..=499 => ("\x1b[31m", format!("{:>6}", status_str)),
            500..=599 => ("\x1b[1;31m", format!("{:>6}", status_str)),
            _ => ("\x1b[37m", format!("{:>6}", status_str)),
        }
    } else {
        ("", format!("{:>6}", status_str))
    };

    let method_display = if use_color {
        match r.m.as_str() {
            "GET" => format!("\x1b[36m{:>6}\x1b[0m", r.m),
            "POST" => format!("\x1b[33m{:>6}\x1b[0m", r.m),
            "PUT" => format!("\x1b[35m{:>6}\x1b[0m", r.m),
            "DELETE" => format!("\x1b[31m{:>6}\x1b[0m", r.m),
            "PATCH" => format!("\x1b[34m{:>6}\x1b[0m", r.m),
            _ => format!("{:>6}", r.m),
        }
    } else {
        format!("{:>6}", r.m)
    };

    let proto = truncate_search_str(&r.proto, 7);
    let host = highlight_remote_search_keyword(&truncate_search_str(&r.h, 40), keyword, use_color);
    let path = highlight_remote_search_keyword(&truncate_search_str(&r.p, 46), keyword, use_color);
    let size = render_search_size(r.res_sz);
    let time = render_search_duration(r.dur);
    let seq = r.seq.to_string();

    if use_color {
        println!(
            "\x1b[90m{:>10}\x1b[0m  {}{}  {}  {:7}  {}  {}  {:>10}  {:>8}\x1b[0m",
            seq, status_color, status_display, method_display, proto, host, path, size, time
        );
    } else {
        println!(
            "{:>10}  {}  {}  {:7}  {}  {}  {:>10}  {:>8}",
            seq, status_display, method_display, proto, host, path, size, time
        );
    }

    if !item.matches.is_empty() && item.matches.iter().any(|m| m.field != "url") {
        for matched in &item.matches {
            if matched.field == "url" {
                continue;
            }
            let preview = highlight_remote_search_keyword(&matched.preview, keyword, use_color);
            if use_color {
                println!(
                    "        \x1b[90m└─ \x1b[34m{}\x1b[90m: {}\x1b[0m",
                    matched.field, preview
                );
            } else {
                println!("        └─ {}: {}", matched.field, preview);
            }
        }
    }
}

fn print_remote_search_summary(
    keyword: &str,
    max_scan: Option<usize>,
    max_results: Option<usize>,
    total_searched: usize,
    total_matched: usize,
    has_more: bool,
    use_color: bool,
) {
    let scan_range = max_scan.unwrap_or(0);
    let result_limit = max_results.unwrap_or(100);

    if use_color {
        if total_matched > 0 {
            println!(
                "\x1b[1;32m✓\x1b[0m Found \x1b[1m{}\x1b[0m matches (scanned {} records, scan range: {}, max results: {})",
                total_matched,
                render_search_number(total_searched),
                render_search_number(scan_range),
                render_search_number(result_limit),
            );
        } else {
            println!(
                "  Scanned {} records for '\x1b[1m{}\x1b[0m' (scan range: {}, max results: {})",
                render_search_number(total_searched),
                keyword,
                render_search_number(scan_range),
                render_search_number(result_limit),
            );
        }
        if has_more {
            println!("\x1b[33m  ⚡ Search stopped early — more data may match.\x1b[0m");
        }
    } else {
        if total_matched > 0 {
            println!(
                "Found {} matches (scanned {} records, scan range: {}, max results: {})",
                total_matched,
                render_search_number(total_searched),
                render_search_number(scan_range),
                render_search_number(result_limit),
            );
        } else {
            println!(
                "Scanned {} records for '{}' (scan range: {}, max results: {})",
                render_search_number(total_searched),
                keyword,
                render_search_number(scan_range),
                render_search_number(result_limit),
            );
        }
        if has_more {
            println!("  Search stopped early — more data may match.");
        }
    }
}

#[cfg(test)]
fn should_stream_remote_command(command: &BuiltRemoteCommand) -> bool {
    matches!(command.query, Some(CanonicalQueryCommand::Search(_)))
}

fn should_stream_remote_render(render: &RemoteRenderMode) -> bool {
    matches!(render, RemoteRenderMode::Raw)
}

async fn wait_for_remote_call_cancel_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        let mut sigterm =
            signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
        let mut sigint = signal(SignalKind::interrupt()).expect("failed to install SIGINT handler");
        let mut sighup = signal(SignalKind::hangup()).expect("failed to install SIGHUP handler");

        tokio::select! {
            _ = sigterm.recv() => {},
            _ = sigint.recv() => {},
            _ = sighup.recv() => {},
            _ = tokio::signal::ctrl_c() => {},
        }
    }

    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn generate_caller_fingerprint(user: &str, machine_id: &str) -> String {
    let raw = format!("bifrost-cli:{}:{}", user, machine_id);
    format!("{:x}", simple_hash(raw.as_bytes()))
}

fn simple_hash(data: &[u8]) -> u128 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    data.hash(&mut hasher);
    let h1 = hasher.finish();
    let mut hasher2 = DefaultHasher::new();
    h1.hash(&mut hasher2);
    let h2 = hasher2.finish();
    (h1 as u128) << 64 | (h2 as u128)
}

fn classify_delete_grant_failure(
    status: reqwest::StatusCode,
    body: &str,
) -> Option<DeleteGrantOutcome> {
    if status == reqwest::StatusCode::NOT_FOUND || body.contains("grant_not_found") {
        return Some(DeleteGrantOutcome::AlreadyMissing);
    }
    None
}

fn get_hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

fn get_username() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".into())
}

fn print_remote_result(command: &BuiltRemoteCommand, result: &CallResult) {
    if let Some(ref stdout) = result.stdout {
        if !stdout.is_empty() {
            match &command.render {
                RemoteRenderMode::Raw => {
                    print!("{stdout}");
                    if !stdout.ends_with('\n') {
                        println!();
                    }
                }
                RemoteRenderMode::Search { .. } => {}
                RemoteRenderMode::TrafficList { format, no_color } => {
                    if let Err(error) = render_traffic_list_body(stdout, *format, *no_color) {
                        eprintln!(
                            "{}",
                            format!("✗ Failed to render remote traffic list: {}", error)
                                .bright_red()
                        );
                    }
                }
                RemoteRenderMode::TrafficGet { format, no_color } => {
                    if let Err(error) = render_traffic_detail_body(stdout, *format, *no_color) {
                        eprintln!(
                            "{}",
                            format!("✗ Failed to render remote traffic detail: {}", error)
                                .bright_red()
                        );
                    }
                }
            }
        }
    }

    if let Some(ref stderr) = result.stderr {
        if !stderr.is_empty() {
            eprint!("{stderr}");
            if !stderr.ends_with('\n') {
                eprintln!();
            }
        }
    }

    if result.cancelled {
        eprintln!(
            "{}",
            format!("Remote command '{}' cancelled by caller.", command.label).bright_yellow()
        );
        return;
    }

    if result.exit_code != 0 {
        eprintln!(
            "{}",
            format!(
                "Remote command '{}' exited with code {}{}",
                command.label,
                result.exit_code,
                if result.stderr.is_none() {
                    " (no error details received from remote)"
                } else {
                    ""
                }
            )
            .bright_red()
        );
    }
}

struct CallerRelayClient {
    http: reqwest::Client,
    base_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeleteGrantOutcome {
    Deleted,
    AlreadyMissing,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CallerInfo {
    fingerprint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    platform: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hostname: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    username: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StartPairingRequest {
    pair_code: String,
    caller_info: CallerInfo,
    caller_ephemeral_pub: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StartPairingResponse {
    pairing_id: String,
    #[serde(default)]
    approval_sse_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PairingWatchResult {
    status: String,
    #[serde(default)]
    grant_id: Option<String>,
    #[serde(default)]
    client_instance_id: Option<String>,
    #[serde(default)]
    device_name: Option<String>,
    #[serde(default)]
    platform: Option<String>,
    #[serde(default)]
    grant_mode: Option<String>,
    #[serde(default)]
    caller_ephemeral_pub: Option<String>,
    #[serde(default)]
    client_ephemeral_pub: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SshChallengeRequest {
    device_code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SshChallengeResponse {
    challenge_id: String,
    challenge: String,
    #[serde(default)]
    expires_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SshConnectRequest {
    device_code: String,
    challenge_id: String,
    signature: String,
    timestamp: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    caller_info: Option<CallerInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    caller_ephemeral_pub: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SshConnectResponse {
    connect_id: String,
    relay_token: String,
    #[serde(default)]
    expires_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SshConnectResult {
    status: String,
    #[serde(default)]
    grant_id: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    caller_fingerprint: Option<String>,
    #[serde(default)]
    grant_mode: Option<String>,
    #[serde(default)]
    client_instance_id: Option<String>,
    #[serde(default)]
    caller_ephemeral_pub: Option<String>,
    #[serde(default)]
    client_ephemeral_pub: Option<String>,
}

struct LoadedSshKey {
    key_pair: Ed25519KeyPair,
    device_code: String,
    ssh_key_fingerprint: String,
    source_label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GrantInfo {
    grant_id: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    caller_ephemeral_pub: Option<String>,
    #[serde(default)]
    client_ephemeral_pub: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenCallRequest {
    grant_id: String,
    client_instance_id: String,
    caller_fingerprint: String,
    command_summary: OpenCallCommandSummary,
    command_kind: CommandKind,
    command_encrypted: EncryptedPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenCallResponse {
    call_id: String,
    relay_token: String,
}

#[derive(Debug)]
#[allow(dead_code)]
pub enum StreamingSubscriptionOutcome {
    /// The call finished with a terminal Done frame.
    Completed {
        exit_code: i32,
        duration_ms: Option<u64>,
        digest_ok: bool,
    },
    /// The executor emitted a Reconnect advisory or we observed a mid-stream
    /// disconnect that can be resumed; caller should reopen a new SSE
    /// subscription with these offsets.
    ReconnectNeeded {
        stdout_offset: u64,
        stderr_offset: u64,
        reason: String,
    },
    /// The SSE stream closed or errored without a terminal frame; caller
    /// should decide whether to resume or surface as a hard failure.
    Disconnected { reason: String },
    /// The executor emitted an Error frame; treat as a non-resumable failure.
    ErrorFrame { code: String, message: String },
    /// The status channel reported the call was cancelled.
    Cancelled,
}

#[derive(Debug, Clone, Default)]
struct CallResult {
    exit_code: i32,
    stdout: Option<String>,
    stderr: Option<String>,
    duration_ms: Option<u64>,
    cancelled: bool,
}

#[derive(Debug, Deserialize)]
struct RelayApiResponse<T> {
    code: i32,
    #[serde(default)]
    message: Option<String>,
    data: Option<T>,
}

impl CallerRelayClient {
    fn new(base_url: &str) -> Self {
        let http = direct_reqwest_client_builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .expect("failed to build caller http client");

        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    async fn delete_grant(
        &self,
        grant_id: &str,
        caller_fingerprint: &str,
    ) -> bifrost_core::Result<DeleteGrantOutcome> {
        let url = format!(
            "{}/v4/remote-invoke/grants/{}?caller_fingerprint={}",
            self.base_url,
            grant_id,
            urlencoding::encode(caller_fingerprint),
        );
        let response = self
            .http
            .delete(&url)
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("delete grant failed: {e}")))?;

        let status = response.status();
        if status.is_success() {
            return Ok(DeleteGrantOutcome::Deleted);
        }

        let body = response.text().await.unwrap_or_default();
        if classify_delete_grant_failure(status, &body) == Some(DeleteGrantOutcome::AlreadyMissing)
        {
            return Ok(DeleteGrantOutcome::AlreadyMissing);
        }

        Err(BifrostError::Network(format!(
            "delete grant failed with status {status}: {}",
            truncate(&body, 500)
        )))
    }

    async fn find_reusable_grant(
        &self,
        client_instance_id: &str,
        caller_fingerprint: &str,
    ) -> bifrost_core::Result<Option<GrantInfo>> {
        let url = format!(
            "{}/v4/remote-invoke/grants/reusable?client_instance_id={}&caller_fingerprint={}",
            self.base_url,
            urlencoding::encode(client_instance_id),
            urlencoding::encode(caller_fingerprint),
        );

        let response = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("find reusable grant failed: {e}")))?;

        let data: Value = self
            .parse_response_data(response, "find_reusable_grant")
            .await?;
        if data.is_null() {
            return Ok(None);
        }
        let grant: GrantInfo = serde_json::from_value(data)
            .map_err(|e| BifrostError::Network(format!("parse grant failed: {e}")))?;
        if grant.grant_id.is_empty() {
            return Ok(None);
        }
        Ok(Some(grant))
    }

    async fn start_pairing(
        &self,
        req: &StartPairingRequest,
    ) -> bifrost_core::Result<StartPairingResponse> {
        let url = format!("{}/v4/remote-invoke/pairings/start", self.base_url);
        let response = self
            .http
            .post(&url)
            .json(req)
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("start pairing failed: {e}")))?;

        self.parse_response_typed(response, "start_pairing").await
    }

    async fn watch_pairing(&self, pairing_id: &str) -> bifrost_core::Result<PairingWatchResult> {
        let url = format!(
            "{}/v4/remote-invoke/pairings/{}/watch",
            self.base_url, pairing_id
        );

        let sse_http = direct_reqwest_client_builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| BifrostError::Network(format!("build sse client: {e}")))?;

        let response = sse_http
            .get(&url)
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("watch pairing failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(BifrostError::Network(format!(
                "watch pairing returned {status}: {body}"
            )));
        }

        let mut stream = response.bytes_stream();
        let timeout = tokio::time::sleep(Duration::from_secs(PAIRING_WATCH_TIMEOUT_SECS));
        tokio::pin!(timeout);

        let mut event_name = String::new();
        let mut data_buf = String::new();
        let mut partial_line = String::new();

        loop {
            tokio::select! {
                _ = &mut timeout => {
                    return Err(BifrostError::Config("pairing approval timed out".to_string()));
                }
                chunk = stream.next() => {
                    match chunk {
                        Some(Ok(bytes)) => {
                            let text = String::from_utf8_lossy(&bytes);
                            partial_line.push_str(&text);

                            while let Some(pos) = partial_line.find('\n') {
                                let line = partial_line[..pos].trim_end_matches('\r').to_string();
                                partial_line = partial_line[pos + 1..].to_string();

                                if line.is_empty() {
                                    if !event_name.is_empty() && !data_buf.is_empty() {
                                        debug!(event = %event_name, "pairing SSE event");
                                        match event_name.as_str() {
                                            "decision" | "approved" | "rejected" | "status" => {
                                                if let Ok(v) = serde_json::from_str::<Value>(&data_buf) {
                                                    let status = v.get("status")
                                                        .or_else(|| v.get("decision"))
                                                        .and_then(|s| s.as_str())
                                                        .unwrap_or(&event_name)
                                                        .to_string();
                                                    let grant_id = v.get("grant_id")
                                                        .and_then(|g| g.as_str())
                                                        .map(|s| s.to_string());
                                                    let client_instance_id = v.get("client_instance_id")
                                                        .and_then(|g| g.as_str())
                                                        .map(|s| s.to_string());
                                                    let device_name = v.get("device_name")
                                                        .and_then(|g| g.as_str())
                                                        .map(|s| s.to_string());
                                                    let platform = v.get("platform")
                                                        .and_then(|g| g.as_str())
                                                        .map(|s| s.to_string());
                                                    let grant_mode = v.get("grant_mode")
                                                        .and_then(|g| g.as_str())
                                                        .map(|s| s.to_string());
                                                    let caller_ephemeral_pub = v.get("caller_ephemeral_pub")
                                                        .and_then(|g| g.as_str())
                                                        .map(|s| s.to_string());
                                                    let client_ephemeral_pub = v.get("client_ephemeral_pub")
                                                        .and_then(|g| g.as_str())
                                                        .map(|s| s.to_string());

                                                    if status == "approved" || status == "rejected" || status == "expired" || status == "cancelled" {
                                                        return Ok(PairingWatchResult {
                                                            status,
                                                            grant_id,
                                                            client_instance_id,
                                                            device_name,
                                                            platform,
                                                            grant_mode,
                                                            caller_ephemeral_pub,
                                                            client_ephemeral_pub,
                                                        });
                                                    }
                                                }
                                            }
                                            _ => {}
                                        }
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
                                }
                            }
                        }
                        Some(Err(e)) => {
                            return Err(BifrostError::Network(format!("pairing watch SSE error: {e}")));
                        }
                        None => {
                            return Err(BifrostError::Network("pairing watch stream closed unexpectedly".to_string()));
                        }
                    }
                }
            }
        }
    }

    async fn open_call(&self, req: &OpenCallRequest) -> bifrost_core::Result<OpenCallResponse> {
        let url = format!("{}/v4/remote-invoke/calls/open", self.base_url);
        let response = self
            .http
            .post(&url)
            .json(req)
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("open call failed: {e}")))?;

        self.parse_response_typed(response, "open_call").await
    }

    async fn cancel_call(&self, call_id: &str, relay_token: &str) -> bifrost_core::Result<()> {
        let url = format!(
            "{}/v4/remote-invoke/calls/{}/cancel",
            self.base_url, call_id
        );
        let response = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {relay_token}"))
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("cancel call failed: {e}")))?;

        let _ = self.parse_response_data(response, "cancel_call").await?;
        Ok(())
    }

    async fn request_ssh_challenge(
        &self,
        device_code: &str,
    ) -> bifrost_core::Result<SshChallengeResponse> {
        let url = format!("{}/v4/remote-invoke/ssh/challenge", self.base_url);
        let response = self
            .http
            .post(&url)
            .json(&SshChallengeRequest {
                device_code: device_code.to_string(),
            })
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("ssh challenge failed: {e}")))?;

        self.parse_response_typed(response, "ssh_challenge").await
    }

    async fn ssh_connect(
        &self,
        req: &SshConnectRequest,
    ) -> bifrost_core::Result<SshConnectResponse> {
        let url = format!("{}/v4/remote-invoke/ssh/connect", self.base_url);
        let response = self
            .http
            .post(&url)
            .json(req)
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("ssh connect failed: {e}")))?;

        self.parse_response_typed(response, "ssh_connect").await
    }

    async fn watch_ssh_connect_result(
        &self,
        connect_id: &str,
        relay_token: &str,
    ) -> bifrost_core::Result<SshConnectResult> {
        let url = format!(
            "{}/v4/remote-invoke/calls/{}/events",
            self.base_url, connect_id
        );

        let sse_http = direct_reqwest_client_builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| BifrostError::Network(format!("build ssh connect sse client: {e}")))?;

        let response = sse_http
            .get(&url)
            .header("Authorization", format!("Bearer {relay_token}"))
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("watch ssh connect failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(BifrostError::Network(format!(
                "watch_ssh_connect_result returned {status}: {body}"
            )));
        }

        let mut stream = response.bytes_stream();
        let timeout = tokio::time::sleep(Duration::from_secs(SSH_CONNECT_TIMEOUT_SECS));
        tokio::pin!(timeout);

        let mut event_name = String::new();
        let mut data_buf = String::new();
        let mut partial_line = String::new();

        loop {
            tokio::select! {
                _ = &mut timeout => {
                    return Err(BifrostError::Config("ssh connect approval timed out".to_string()));
                }
                chunk = stream.next() => {
                    match chunk {
                        Some(Ok(bytes)) => {
                            let text = String::from_utf8_lossy(&bytes);
                            partial_line.push_str(&text);

                            while let Some(pos) = partial_line.find('\n') {
                                let line = partial_line[..pos].trim_end_matches('\r').to_string();
                                partial_line = partial_line[pos + 1..].to_string();

                                if line.is_empty() {
                                    if event_name == "ssh_connect_result" && !data_buf.is_empty() {
                                        return serde_json::from_str(&data_buf).map_err(|e| {
                                            BifrostError::Network(format!(
                                                "parse ssh_connect_result failed: {e}"
                                            ))
                                        });
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
                                }
                            }
                        }
                        Some(Err(e)) => {
                            return Err(BifrostError::Network(format!("ssh connect SSE error: {e}")));
                        }
                        None => {
                            return Err(BifrostError::Network(
                                "ssh connect stream closed unexpectedly".to_string(),
                            ));
                        }
                    }
                }
            }
        }
    }

    async fn subscribe_call_events(
        &self,
        call_id: &str,
        relay_token: &str,
        transport: &OpenCallTransportContext,
        render_mode: &RemoteRenderMode,
        timeout_secs: u64,
    ) -> bifrost_core::Result<CallResult> {
        let url = format!(
            "{}/v4/remote-invoke/calls/{}/events",
            self.base_url, call_id
        );

        let sse_http = direct_reqwest_client_builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| BifrostError::Network(format!("build call events sse client: {e}")))?;

        let response = sse_http
            .get(&url)
            .header("Authorization", format!("Bearer {relay_token}"))
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("subscribe call events failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(BifrostError::Network(format!(
                "call events returned {status}: {body}"
            )));
        }

        let mut stream = response.bytes_stream();
        // PR #5a: idle deadline resets on each frame; see pulse() below.
        let idle = Duration::from_secs(timeout_secs);
        let mut timeout = Box::pin(tokio::time::sleep(idle));

        let mut event_name = String::new();
        let mut data_buf = String::new();
        let mut partial_line = String::new();
        let mut result = CallResult::default();
        let mut stdout_parts: Vec<String> = Vec::new();
        let mut seen_frame_seqs: HashSet<u64> = HashSet::new();
        let mut exit_received = false;
        let mut search_renderer = RemoteSearchRenderer::from_render_mode(render_mode);
        let stream_stdout = should_stream_remote_render(render_mode);

        loop {
            tokio::select! {
                _ = &mut timeout => {
                    if exit_received {
                        debug!("grace timeout after exit, no late frame arrived");
                        if !stream_stdout {
                            result.stdout = Some(stdout_parts.join(""));
                        }
                        return Ok(result);
                    }
                    warn!("call events timed out");
                    if !stream_stdout && stdout_parts.is_empty() {
                        return Err(BifrostError::Config("remote call timed out waiting for response".to_string()));
                    }
                    if !stream_stdout {
                        result.stdout = Some(stdout_parts.join(""));
                    }
                    return Ok(result);
                }
                chunk = stream.next() => {
                    match chunk {
                        Some(Ok(bytes)) => {
                            let text = String::from_utf8_lossy(&bytes);
                            partial_line.push_str(&text);
                            // PR #5a: reset idle deadline on each received chunk.
                            timeout.as_mut().reset(tokio::time::Instant::now() + idle);

                            while let Some(pos) = partial_line.find('\n') {
                                let line = partial_line[..pos].trim_end_matches('\r').to_string();
                                partial_line = partial_line[pos + 1..].to_string();

                                if line.is_empty() {
                                    if !event_name.is_empty() && !data_buf.is_empty() {
                                        debug!(event = %event_name, "call event");
                                        match event_name.as_str() {
                                            "frame" => {
                                                if let Ok(v) = serde_json::from_str::<Value>(&data_buf) {
                                                    if let Some(envelope_json) = v.get("envelope_json").and_then(|e| e.as_str()) {
                                                        if let Ok(envelope) = serde_json::from_str::<Value>(envelope_json) {
                                                            let seq = envelope.get("seq").and_then(|s| s.as_u64()).unwrap_or(0);
                                                            if seen_frame_seqs.insert(seq) {
                                                                let chunk = decrypt_frame_chunk(transport, call_id, &envelope)?;
                                                                if !chunk.is_empty() {
                                                                    if let Some(renderer) = search_renderer.as_mut() {
                                                                        renderer.consume_chunk(&chunk)?;
                                                                    } else if stream_stdout {
                                                                        print!("{chunk}");
                                                                        std::io::stdout().flush().ok();
                                                                    } else {
                                                                        stdout_parts.push(chunk);
                                                                    }
                                                                }
                                                            } else {
                                                                debug!(seq = seq, "skipping duplicate frame");
                                                            }
                                                        }
                                                    } else if let Some(ct) = v.get("ciphertext").and_then(|c| c.as_str()) {
                                                        if let Some(renderer) = search_renderer.as_mut() {
                                                            renderer.consume_chunk(ct)?;
                                                        } else if stream_stdout {
                                                            print!("{ct}");
                                                            std::io::stdout().flush().ok();
                                                        } else {
                                                            stdout_parts.push(ct.to_string());
                                                        }
                                                    }
                                                }
                                                if exit_received {
                                                    debug!("late frame received after exit");
                                                    timeout = Box::pin(tokio::time::sleep(Duration::from_secs(3)));
                                                }
                                            }
                                            "exit" => {
                                                if let Ok(v) = serde_json::from_str::<Value>(&data_buf) {
                                                    if let Some(exit_payload) = v
                                                        .get("exit_encrypted")
                                                        .cloned()
                                                        .and_then(|value| serde_json::from_value::<EncryptedPayload>(value).ok())
                                                    {
                                                        let decrypted = decrypt_exit_payload(transport, call_id, &exit_payload)?;
                                                        result.exit_code = decrypted.exit_code;
                                                        result.duration_ms = decrypted.duration_ms;
                                                        result.stderr = decrypted.stderr.filter(|s| !s.is_empty());
                                                    } else {
                                                        result.exit_code = v.get("exit_code")
                                                            .and_then(|c| c.as_i64())
                                                            .unwrap_or(0) as i32;
                                                        result.duration_ms = v.get("duration_ms")
                                                            .and_then(|d| d.as_u64());
                                                        result.stderr = v.get("stderr")
                                                            .and_then(|s| s.as_str())
                                                            .filter(|s| !s.is_empty())
                                                            .map(|s| s.to_string());
                                                    }
                                                }
                                                if !stream_stdout && !stdout_parts.is_empty() {
                                                    result.stdout = Some(stdout_parts.join(""));
                                                    return Ok(result);
                                                }
                                                debug!("exit received with empty stdout, waiting for delayed frame");
                                                exit_received = true;
                                                timeout = Box::pin(tokio::time::sleep(Duration::from_secs(3)));
                                            }
                                            "error" => {
                                                if let Ok(v) = serde_json::from_str::<Value>(&data_buf) {
                                                    let msg = v.get("message")
                                                        .or_else(|| v.get("error"))
                                                        .and_then(|m| m.as_str())
                                                        .unwrap_or("unknown error");
                                                    error!(error = %msg, "call error from relay");
                                                    result.exit_code = -1;
                                                    result.stderr = Some(msg.to_string());
                                                }
                                                if !stream_stdout {
                                                    result.stdout = Some(stdout_parts.join(""));
                                                }
                                                return Ok(result);
                                            }
                                            "status" => {
                                                if let Ok(v) = serde_json::from_str::<Value>(&data_buf) {
                                                    if let Some(status) = parse_call_terminal_status(&v) {
                                                        if status == "cancelled" {
                                                            result.exit_code = 130;
                                                            result.stderr = Some("remote call cancelled by caller".to_string());
                                                            result.cancelled = true;
                                                            if !stream_stdout {
                                                                result.stdout = Some(stdout_parts.join(""));
                                                            }
                                                            return Ok(result);
                                                        }
                                                    }
                                                }
                                            }
                                            _ => {
                                                debug!(event = %event_name, "unhandled call event");
                                            }
                                        }
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
                                }
                            }
                        }
                        Some(Err(e)) => {
                            return Err(BifrostError::Network(format!("call events SSE error: {e}")));
                        }
                        None => {
                            debug!("call events stream closed");
                            if !stream_stdout {
                                result.stdout = Some(stdout_parts.join(""));
                            }
                            return Ok(result);
                        }
                    }
                }
            }
        }
    }

    /// PR #5d-2: streaming variant of subscribe_call_events that routes
    /// recognised StreamFrame-shaped payloads through CallerStreamState
    /// and surfaces a structured outcome so the dispatcher can implement an
    /// outer reconnect-with-offset loop.
    ///
    /// Legacy envelope payloads (envelope_json / ciphertext) are NOT handled
    /// here; callers should fall back to `subscribe_call_events` when the
    /// remote does not yet emit StreamFrames. Detection is strict via
    /// `parse_stream_frame_from_sse_data`.
    ///
    /// The `resume_from` parameter, when Some, pre-positions the state
    /// machine heads so duplicate bytes across a reconnect are deduped.
    #[allow(dead_code)]
    async fn subscribe_call_events_streaming<W: std::io::Write, E: std::io::Write>(
        &self,
        call_id: &str,
        relay_token: &str,
        state: &mut CallerStreamState<W, E>,
        resume_from: Option<(u64, u64)>,
        timeout_secs: u64,
    ) -> bifrost_core::Result<StreamingSubscriptionOutcome> {
        if let Some((so, se)) = resume_from {
            state.set_heads(so, se);
        }

        let url = format!(
            "{}/v4/remote-invoke/calls/{}/events",
            self.base_url, call_id
        );
        let sse_http = direct_reqwest_client_builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| BifrostError::Network(format!("build streaming sse client: {e}")))?;
        let response = sse_http
            .get(&url)
            .header("Authorization", format!("Bearer {relay_token}"))
            .send()
            .await
            .map_err(|e| {
                BifrostError::Network(format!("subscribe streaming events failed: {e}"))
            })?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(BifrostError::Network(format!(
                "streaming events returned {status}: {body}"
            )));
        }

        let mut stream = response.bytes_stream();
        let idle = Duration::from_secs(timeout_secs);
        let mut timeout = Box::pin(tokio::time::sleep(idle));

        let mut event_name = String::new();
        let mut data_buf = String::new();
        let mut partial_line = String::new();

        loop {
            tokio::select! {
                _ = &mut timeout => {
                    warn!("streaming events idle timeout");
                    return Ok(StreamingSubscriptionOutcome::Disconnected {
                        reason: "idle_timeout".to_string(),
                    });
                }
                chunk = stream.next() => {
                    match chunk {
                        Some(Ok(bytes)) => {
                            let text = String::from_utf8_lossy(&bytes);
                            partial_line.push_str(&text);
                            timeout.as_mut().reset(tokio::time::Instant::now() + idle);

                            while let Some(pos) = partial_line.find('\n') {
                                let line = partial_line[..pos].trim_end_matches('\r').to_string();
                                partial_line = partial_line[pos + 1..].to_string();

                                if line.is_empty() {
                                    if !event_name.is_empty() && !data_buf.is_empty() {
                                        if event_name == "frame" {
                                            if let Some(frame) = parse_stream_frame_from_sse_data(&data_buf) {
                                                match state.feed(&frame).map_err(|e| BifrostError::Config(format!("stream ingest error: {e:?}")))? {
                                                    StreamDecision::Continue => {}
                                                    StreamDecision::ReconnectAt { stdout_offset, stderr_offset, reason } => {
                                                        return Ok(StreamingSubscriptionOutcome::ReconnectNeeded {
                                                            stdout_offset,
                                                            stderr_offset,
                                                            reason,
                                                        });
                                                    }
                                                    StreamDecision::Done { exit_code, duration_ms, digest_ok, .. } => {
                                                        return Ok(StreamingSubscriptionOutcome::Completed {
                                                            exit_code,
                                                            duration_ms: Some(duration_ms),
                                                            digest_ok,
                                                        });
                                                    }
                                                    StreamDecision::Error { code, message } => {
                                                        return Ok(StreamingSubscriptionOutcome::ErrorFrame { code, message });
                                                    }
                                                }
                                            } else {
                                                debug!("streaming path saw non-StreamFrame frame payload; ignoring");
                                            }
                                        } else if event_name == "status" {
                                            if let Ok(v) = serde_json::from_str::<Value>(&data_buf) {
                                                if let Some(status) = parse_call_terminal_status(&v) {
                                                    if status == "cancelled" {
                                                        return Ok(StreamingSubscriptionOutcome::Cancelled);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    event_name.clear();
                                    data_buf.clear();
                                } else if let Some(ev) = line.strip_prefix("event:") {
                                    event_name = ev.trim().to_string();
                                } else if let Some(d) = line.strip_prefix("data:") {
                                    if !data_buf.is_empty() { data_buf.push('\n'); }
                                    data_buf.push_str(d.trim());
                                }
                            }
                        }
                        Some(Err(e)) => {
                            return Ok(StreamingSubscriptionOutcome::Disconnected {
                                reason: format!("sse error: {e}"),
                            });
                        }
                        None => {
                            return Ok(StreamingSubscriptionOutcome::Disconnected {
                                reason: "sse stream closed".to_string(),
                            });
                        }
                    }
                }
            }
        }
    }

    async fn settle_cancelled_call(
        &self,
        call_id: &str,
        relay_token: &str,
        transport: &OpenCallTransportContext,
        render_mode: &RemoteRenderMode,
        cancel_requested: bool,
    ) -> bifrost_core::Result<CallResult> {
        if !cancel_requested {
            return self
                .subscribe_call_events(
                    call_id,
                    relay_token,
                    transport,
                    render_mode,
                    CALL_EVENT_TIMEOUT_SECS,
                )
                .await;
        }

        for (attempt, delay_ms) in std::iter::once(0)
            .chain(CANCEL_SETTLE_RETRY_DELAYS_MS)
            .enumerate()
        {
            if delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }

            match self
                .subscribe_call_events(
                    call_id,
                    relay_token,
                    transport,
                    render_mode,
                    CANCEL_SETTLE_TIMEOUT_SECS,
                )
                .await
            {
                Ok(result) if result.cancelled => return Ok(result),
                Ok(result) if is_ambiguous_cancel_settle_result(&result) => {
                    warn!(
                        call_id = %call_id,
                        attempt = attempt,
                        "cancel settle timed out without terminal event, synthesizing cancelled result"
                    );
                    return Ok(synthesized_cancelled_result());
                }
                Ok(result) => return Ok(result),
                Err(err) if is_retryable_cancel_settle_error(&err) => {
                    warn!(
                        call_id = %call_id,
                        attempt = attempt,
                        error = %err,
                        "cancel settle hit retryable relay throttling, retrying"
                    );
                }
                Err(err) => {
                    warn!(
                        call_id = %call_id,
                        attempt = attempt,
                        error = %err,
                        "cancel settle failed after relay accepted cancel, synthesizing cancelled result"
                    );
                    return Ok(synthesized_cancelled_result());
                }
            }
        }

        warn!(
            call_id = %call_id,
            "cancel settle exhausted retries, synthesizing cancelled result"
        );
        Ok(synthesized_cancelled_result())
    }

    async fn parse_response_data(
        &self,
        response: reqwest::Response,
        operation: &str,
    ) -> bifrost_core::Result<Value> {
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| BifrostError::Network(format!("{operation} response read failed: {e}")))?;

        if !status.is_success() {
            return Err(BifrostError::Network(format!(
                "{operation} failed with status {status}: {}",
                truncate(&body, 500)
            )));
        }

        let envelope: RelayApiResponse<Value> = serde_json::from_str(&body).map_err(|e| {
            BifrostError::Network(format!(
                "{operation} invalid JSON: {e} body={}",
                truncate(&body, 500)
            ))
        })?;

        if envelope.code != 0 {
            let msg = envelope.message.unwrap_or_default();
            return Err(BifrostError::Network(format!(
                "{operation} error code {}: {msg}",
                envelope.code
            )));
        }

        Ok(envelope.data.unwrap_or(Value::Null))
    }

    async fn parse_response_typed<T: serde::de::DeserializeOwned>(
        &self,
        response: reqwest::Response,
        operation: &str,
    ) -> bifrost_core::Result<T> {
        let data = self.parse_response_data(response, operation).await?;
        serde_json::from_value(data)
            .map_err(|e| BifrostError::Network(format!("{operation} parse failed: {e}")))
    }
}

fn load_ssh_key(
    spec: &str,
    device_code_override: Option<&str>,
) -> bifrost_core::Result<LoadedSshKey> {
    let (raw, source_label, file_path) = read_ssh_key_source(spec)?;
    if let Some(path) = file_path.as_deref() {
        warn_if_ssh_key_permissions_are_too_open(path);
    }

    let (pkcs8, embedded_device_code) = parse_ssh_private_key_bytes(&raw)?;
    let key_pair = Ed25519KeyPair::from_pkcs8(&pkcs8)
        .map_err(|_| BifrostError::Config("parse ed25519 private key failed".to_string()))?;
    let public_key_der = ed25519_public_key_to_spki_der(key_pair.public_key().as_ref());
    let device_code = device_code_override
        .map(ToOwned::to_owned)
        .or(embedded_device_code)
        .unwrap_or_else(|| derive_device_code(&public_key_der));
    let ssh_key_fingerprint = sha256_hex(&public_key_der);

    Ok(LoadedSshKey {
        key_pair,
        device_code,
        ssh_key_fingerprint,
        source_label,
    })
}

fn read_ssh_key_source(spec: &str) -> bifrost_core::Result<(Vec<u8>, String, Option<PathBuf>)> {
    if let Some(env_name) = spec.strip_prefix("env:") {
        let value = std::env::var(env_name).map_err(|_| {
            BifrostError::Config(format!("environment variable `{env_name}` is not set"))
        })?;
        return Ok((value.into_bytes(), format!("env:{env_name}"), None));
    }

    if spec == "-" {
        let mut buf = Vec::new();
        std::io::stdin().read_to_end(&mut buf).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "read ssh key from stdin: {e}"
            )))
        })?;
        return Ok((buf, "stdin".to_string(), None));
    }

    let path = PathBuf::from(spec);
    let bytes = std::fs::read(&path).map_err(|e| {
        BifrostError::Io(std::io::Error::other(format!(
            "read ssh key {}: {e}",
            path.display()
        )))
    })?;
    Ok((bytes, path.display().to_string(), Some(path)))
}

fn parse_ssh_private_key_bytes(raw: &[u8]) -> bifrost_core::Result<(Vec<u8>, Option<String>)> {
    if let Ok(text) = std::str::from_utf8(raw) {
        if let Some((pkcs8, device_code)) = parse_bifrost_key_file(text)? {
            return Ok((pkcs8, Some(device_code)));
        }
        if let Some(pkcs8) = parse_pem_block(text, PKCS8_KEY_BEGIN, PKCS8_KEY_END)? {
            return Ok((pkcs8, None));
        }
    }

    Ok((raw.to_vec(), None))
}

fn parse_bifrost_key_file(text: &str) -> bifrost_core::Result<Option<(Vec<u8>, String)>> {
    let Some(body) = text
        .split_once(BIFROST_KEY_BEGIN)
        .and_then(|(_, rest)| rest.split_once(BIFROST_KEY_END).map(|(body, _)| body))
    else {
        return Ok(None);
    };

    let mut device_code = None;
    let mut base64_lines = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("Device-Code:") {
            device_code = Some(value.trim().to_string());
            continue;
        }
        base64_lines.push(trimmed);
    }

    let device_code = device_code.ok_or_else(|| {
        BifrostError::Config("Bifrost SSH key file is missing `Device-Code:` header".to_string())
    })?;
    let pkcs8 = base64::engine::general_purpose::STANDARD
        .decode(base64_lines.join(""))
        .map_err(|e| BifrostError::Config(format!("decode Bifrost SSH key file failed: {e}")))?;
    Ok(Some((pkcs8, device_code)))
}

fn parse_pem_block(
    text: &str,
    begin_marker: &str,
    end_marker: &str,
) -> bifrost_core::Result<Option<Vec<u8>>> {
    let Some(body) = text
        .split_once(begin_marker)
        .and_then(|(_, rest)| rest.split_once(end_marker).map(|(body, _)| body))
    else {
        return Ok(None);
    };

    let encoded: String = body
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|e| BifrostError::Config(format!("decode PEM private key failed: {e}")))?;
    Ok(Some(decoded))
}

fn build_ssh_connect_signature_payload(
    challenge: &str,
    challenge_id: &str,
    device_code: &str,
    timestamp: u64,
) -> String {
    format!(
        "{{\"challenge\":{},\"challenge_id\":{},\"device_code\":{},\"timestamp\":{timestamp}}}",
        serde_json::to_string(challenge).unwrap_or_else(|_| "\"\"".to_string()),
        serde_json::to_string(challenge_id).unwrap_or_else(|_| "\"\"".to_string()),
        serde_json::to_string(device_code).unwrap_or_else(|_| "\"\"".to_string()),
    )
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn ed25519_public_key_to_spki_der(public_key: &[u8]) -> Vec<u8> {
    const ED25519_SPKI_PREFIX: [u8; 12] = [
        0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
    ];

    let mut der = Vec::with_capacity(ED25519_SPKI_PREFIX.len() + public_key.len());
    der.extend_from_slice(&ED25519_SPKI_PREFIX);
    der.extend_from_slice(public_key);
    der
}

fn derive_device_code(public_key_der: &[u8]) -> String {
    let digest = digest(&SHA256, public_key_der);
    let prefix = &digest.as_ref()[..8];
    let mut out = String::from("BF-");
    for byte in prefix {
        out.push_str(&format!("{byte:02X}"));
    }
    out
}

fn sha256_hex(data: &[u8]) -> String {
    let digest = digest(&SHA256, data);
    let mut out = String::with_capacity(digest.as_ref().len() * 2);
    for byte in digest.as_ref() {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn warn_if_ssh_key_permissions_are_too_open(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if let Ok(metadata) = std::fs::metadata(path) {
            let mode = metadata.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                eprintln!(
                    "{}",
                    format!(
                        "Warning: SSH key file {} has permissions {:o}; consider chmod 600.",
                        path.display(),
                        mode
                    )
                    .bright_yellow()
                );
            }
        }
    }
}

fn parse_call_terminal_status(payload: &Value) -> Option<&str> {
    payload
        .get("status")
        .and_then(|status| status.as_str())
        .filter(|status| matches!(*status, "cancelled" | "completed" | "failed" | "timeout"))
}

fn synthesized_cancelled_result() -> CallResult {
    CallResult {
        exit_code: 130,
        stdout: None,
        stderr: Some("remote call cancelled by caller".to_string()),
        duration_ms: None,
        cancelled: true,
    }
}

fn is_retryable_cancel_settle_error(err: &BifrostError) -> bool {
    match err {
        BifrostError::Network(message) => {
            message.contains("call events returned 429")
                || message.contains("cancel_call failed with status 429")
                || message.contains("too many requests, retry after")
        }
        _ => false,
    }
}

fn is_ambiguous_cancel_settle_result(result: &CallResult) -> bool {
    !result.cancelled
        && result.exit_code == 0
        && result.stderr.is_none()
        && result.duration_ms.is_none()
        && result.stdout.is_none()
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{t}...(truncated)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{
        RemoteCommandCommands, RemoteCommandExecArgs, RemoteSearchArgs, RemoteTrafficListArgs,
    };
    use std::sync::OnceLock;

    fn init_test_data_dir() {
        static TEST_DATA_DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
        let dir = TEST_DATA_DIR.get_or_init(|| tempfile::tempdir().expect("create temp dir"));
        bifrost_storage::set_data_dir(dir.path().to_path_buf());
    }

    fn decrypt_remote_command_for_test(
        payload: &EncryptedPayload,
        shared_secret: &[u8],
        grant_id: &str,
        caller_ephemeral_pub: &str,
        client_ephemeral_pub: &str,
        command_kind: CommandKind,
    ) -> CommandEnvelope {
        let key_bytes = derive_open_call_key(
            shared_secret,
            grant_id,
            caller_ephemeral_pub,
            client_ephemeral_pub,
            command_kind,
        )
        .expect("derive key");
        let unbound =
            UnboundKey::new(&CHACHA20_POLY1305, &key_bytes).expect("build decryption key");
        let key = LessSafeKey::new(unbound);
        let nonce_raw = base64::engine::general_purpose::STANDARD
            .decode(&payload.nonce)
            .expect("decode nonce");
        let nonce: [u8; NONCE_LEN] = nonce_raw.try_into().expect("nonce length");
        let mut sealed = base64::engine::general_purpose::STANDARD
            .decode(&payload.ciphertext)
            .expect("decode ciphertext");
        sealed.extend(
            base64::engine::general_purpose::STANDARD
                .decode(&payload.tag)
                .expect("decode tag"),
        );
        let plaintext = key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::empty(),
                &mut sealed,
            )
            .expect("decrypt payload");
        serde_json::from_slice(plaintext).expect("parse command envelope")
    }

    #[test]
    fn test_build_remote_command_for_search_uses_streaming_command() {
        let built = build_remote_command(&RemoteCommands::Search(Box::new(RemoteSearchArgs {
            keyword: Some("nextoncall".to_string()),
            limit: 11,
            format: "compact".to_string(),
            no_color: true,
            url: true,
            headers: false,
            body: false,
            req_header: false,
            res_header: false,
            req_body: false,
            res_body: false,
            status: Some("2xx".to_string()),
            method: Some("GET".to_string()),
            host: Some("api.example.com".to_string()),
            path: Some("/v1".to_string()),
            protocol: Some("HTTPS".to_string()),
            content_type: Some("json".to_string()),
            domain: Some("example.com".to_string()),
            max_scan: Some(12),
            max_results: Some(7),
        })));

        assert_eq!(built.kind, CommandKind::QueryReadonly);
        assert_eq!(built.label, "search.stream");
        assert_eq!(built.command.as_deref(), Some("search.stream"));
        let raw_args = built.args_json.as_deref().expect("args_json should exist");
        let args: SearchArgs = serde_json::from_str(raw_args).expect("search args json");
        assert_eq!(args.max_results, Some(7));
        assert_eq!(args.max_scan, Some(12));
        match &built.render {
            RemoteRenderMode::Search {
                format,
                no_color,
                keyword,
                max_scan,
                max_results,
            } => {
                assert_eq!(*format, OutputFormat::Compact);
                assert!(*no_color);
                assert_eq!(keyword, "nextoncall");
                assert_eq!(*max_scan, Some(12));
                assert_eq!(*max_results, Some(7));
            }
            other => panic!("unexpected render mode: {:?}", other),
        }
        match built.query.expect("query should exist") {
            CanonicalQueryCommand::Search(args) => {
                assert_eq!(args.keyword, "nextoncall");
                assert_eq!(args.limit, Some(11));
                assert_eq!(args.max_results, Some(7));
                assert_eq!(args.max_scan, Some(12));
                assert!(args.scope.url);
                assert!(!args.scope.all);
                assert_eq!(args.filters.status_ranges, vec!["2xx"]);
                assert_eq!(args.filters.protocols, vec!["https"]);
                assert_eq!(args.filters.content_types, vec!["json"]);
                assert_eq!(args.filters.domains, vec!["example.com"]);
                assert!(args.filters.conditions.iter().any(|condition| {
                    condition.field == "method"
                        && condition.operator == "equals"
                        && condition.value == "GET"
                }));
                assert!(args.filters.conditions.iter().any(|condition| {
                    condition.field == "host" && condition.value == "api.example.com"
                }));
                assert!(args
                    .filters
                    .conditions
                    .iter()
                    .any(|condition| { condition.field == "path" && condition.value == "/v1" }));
            }
            other => panic!("unexpected query: {:?}", other),
        }
    }

    #[test]
    fn test_build_remote_command_for_traffic_search_uses_streaming_command() {
        let built = build_remote_command(&RemoteCommands::Traffic {
            action: RemoteTrafficCommands::Search(Box::new(RemoteSearchArgs {
                keyword: Some("token".to_string()),
                limit: 5,
                format: "table".to_string(),
                no_color: false,
                url: false,
                headers: true,
                body: false,
                req_header: false,
                res_header: true,
                req_body: false,
                res_body: false,
                status: None,
                method: None,
                host: None,
                path: None,
                protocol: None,
                content_type: None,
                domain: None,
                max_scan: Some(9),
                max_results: Some(3),
            })),
        });

        assert_eq!(built.kind, CommandKind::QueryReadonly);
        assert_eq!(built.label, "search.stream");
        assert_eq!(built.command.as_deref(), Some("search.stream"));
        let raw_args = built.args_json.as_deref().expect("args_json should exist");
        let args: SearchArgs = serde_json::from_str(raw_args).expect("traffic search args json");
        assert_eq!(args.max_results, Some(3));
        assert_eq!(args.max_scan, Some(9));
        match &built.render {
            RemoteRenderMode::Search {
                format,
                no_color,
                keyword,
                max_scan,
                max_results,
            } => {
                assert_eq!(*format, OutputFormat::Table);
                assert!(!*no_color);
                assert_eq!(keyword, "token");
                assert_eq!(*max_scan, Some(9));
                assert_eq!(*max_results, Some(3));
            }
            other => panic!("unexpected render mode: {:?}", other),
        }
        match built.query.expect("query should exist") {
            CanonicalQueryCommand::Search(args) => {
                assert_eq!(args.keyword, "token");
                assert_eq!(args.limit, Some(5));
                assert_eq!(args.max_results, Some(3));
                assert_eq!(args.max_scan, Some(9));
                assert!(args.scope.request_headers);
                assert!(args.scope.response_headers);
                assert!(!args.scope.all);
            }
            other => panic!("unexpected query: {:?}", other),
        }
    }

    #[test]
    fn test_build_remote_command_for_search_supports_filter_only_query() {
        let built = build_remote_command(&RemoteCommands::Search(Box::new(RemoteSearchArgs {
            keyword: None,
            limit: 9,
            format: "json".to_string(),
            no_color: false,
            url: false,
            headers: false,
            body: false,
            req_header: false,
            res_header: false,
            req_body: false,
            res_body: false,
            status: Some("5xx".to_string()),
            method: None,
            host: Some("api.example.com".to_string()),
            path: None,
            protocol: None,
            content_type: None,
            domain: None,
            max_scan: Some(20),
            max_results: Some(9),
        })));

        match built.query.expect("query should exist") {
            CanonicalQueryCommand::Search(args) => {
                assert!(args.keyword.is_empty());
                assert_eq!(args.filters.status_ranges, vec!["5xx"]);
                assert!(args
                    .filters
                    .conditions
                    .iter()
                    .any(|condition| condition.field == "host"
                        && condition.value == "api.example.com"));
            }
            other => panic!("unexpected query: {:?}", other),
        }
    }

    #[test]
    fn test_build_remote_command_for_traffic_list_includes_all_filters() {
        let built = build_remote_command(&RemoteCommands::Traffic {
            action: RemoteTrafficCommands::List(Box::new(RemoteTrafficListArgs {
                limit: 7,
                cursor: Some(123),
                direction: "forward".to_string(),
                method: Some("POST".to_string()),
                status: Some(201),
                status_min: Some(200),
                status_max: Some(299),
                protocol: Some("https".to_string()),
                host: Some("api.example.com".to_string()),
                url: Some("/v1/chat".to_string()),
                path: Some("/v1".to_string()),
                content_type: Some("application/json".to_string()),
                client_ip: Some("127.0.0.1".to_string()),
                client_app: Some("curl".to_string()),
                has_rule_hit: Some(true),
                is_websocket: Some(false),
                is_sse: Some(true),
                is_tunnel: Some(false),
                format: "compact".to_string(),
                no_color: true,
            })),
        });

        assert_eq!(built.kind, CommandKind::QueryReadonly);
        assert_eq!(built.label, "traffic.list");
        assert_eq!(built.command.as_deref(), Some("traffic.list"));
        let raw_args = built.args_json.as_deref().expect("args_json should exist");
        let json: serde_json::Value = serde_json::from_str(raw_args).expect("traffic list json");
        assert_eq!(json.get("limit").and_then(|v| v.as_u64()), Some(7));
        assert_eq!(json.get("cursor").and_then(|v| v.as_u64()), Some(123));
        assert_eq!(
            json.get("direction").and_then(|v| v.as_str()),
            Some("forward")
        );
        assert_eq!(json.get("method").and_then(|v| v.as_str()), Some("POST"));
        assert_eq!(json.get("status_min").and_then(|v| v.as_u64()), Some(200));
        assert_eq!(json.get("status_max").and_then(|v| v.as_u64()), Some(299));
        assert_eq!(
            json.get("client_app").and_then(|v| v.as_str()),
            Some("curl")
        );
        match &built.render {
            RemoteRenderMode::TrafficList { format, no_color } => {
                assert_eq!(*format, OutputFormat::Compact);
                assert!(*no_color);
            }
            other => panic!("unexpected render mode: {:?}", other),
        }
        match built.query.expect("query should exist") {
            CanonicalQueryCommand::TrafficList(args) => {
                assert_eq!(args.limit, Some(7));
                assert_eq!(args.cursor, Some(123));
                assert_eq!(args.direction, TrafficListDirection::Forward);
                assert_eq!(args.method.as_deref(), Some("POST"));
                assert_eq!(args.status, Some(201));
                assert_eq!(args.status_min, Some(200));
                assert_eq!(args.status_max, Some(299));
                assert_eq!(args.protocol.as_deref(), Some("https"));
                assert_eq!(args.host.as_deref(), Some("api.example.com"));
                assert_eq!(args.url.as_deref(), Some("/v1/chat"));
                assert_eq!(args.path.as_deref(), Some("/v1"));
                assert_eq!(args.content_type.as_deref(), Some("application/json"));
                assert_eq!(args.client_ip.as_deref(), Some("127.0.0.1"));
                assert_eq!(args.client_app.as_deref(), Some("curl"));
                assert_eq!(args.has_rule_hit, Some(true));
                assert_eq!(args.is_websocket, Some(false));
                assert_eq!(args.is_sse, Some(true));
                assert_eq!(args.is_tunnel, Some(false));
            }
            other => panic!("unexpected query: {:?}", other),
        }
    }

    #[test]
    fn test_build_remote_command_for_traffic_get_includes_body_flags() {
        let built = build_remote_command(&RemoteCommands::Traffic {
            action: RemoteTrafficCommands::Get {
                id: "REQ-69e304e7-000033".to_string(),
                request_body: true,
                response_body: false,
                format: "table".to_string(),
                no_color: true,
            },
        });

        assert_eq!(built.kind, CommandKind::QueryReadonly);
        assert_eq!(built.label, "traffic.get");
        assert_eq!(built.command.as_deref(), Some("traffic.get"));
        let raw_args = built.args_json.as_deref().expect("args_json should exist");
        let json: serde_json::Value = serde_json::from_str(raw_args).expect("traffic get json");
        assert_eq!(
            json.get("id").and_then(|v| v.as_str()),
            Some("REQ-69e304e7-000033")
        );
        assert_eq!(
            json.get("request_body").and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            json.get("response_body").and_then(|v| v.as_bool()),
            Some(false)
        );
        match &built.render {
            RemoteRenderMode::TrafficGet { format, no_color } => {
                assert_eq!(*format, OutputFormat::Table);
                assert!(*no_color);
            }
            other => panic!("unexpected render mode: {:?}", other),
        }
        match built.query.expect("query should exist") {
            CanonicalQueryCommand::TrafficGet(args) => {
                assert_eq!(args.id, "REQ-69e304e7-000033");
                assert!(args.request_body);
                assert!(!args.response_body);
            }
            other => panic!("unexpected query: {:?}", other),
        }
    }

    #[test]
    fn test_build_remote_command_for_traffic_clear_uses_typed_ids() {
        let built = build_remote_command(&RemoteCommands::Traffic {
            action: RemoteTrafficCommands::Clear {
                ids: Some("REQ-1, REQ-2".to_string()),
                yes: true,
            },
        });

        assert_eq!(built.kind, CommandKind::QueryReadonly);
        assert_eq!(built.label, "traffic.clear");
        assert_eq!(built.command.as_deref(), Some("traffic.clear"));
        let raw_args = built.args_json.as_deref().expect("args_json should exist");
        let json: serde_json::Value = serde_json::from_str(raw_args).expect("traffic clear json");
        assert_eq!(
            json.get("ids")
                .and_then(|v| v.as_array())
                .map(|ids| ids.len()),
            Some(2)
        );
        match built.query.expect("query should exist") {
            CanonicalQueryCommand::TrafficClear(args) => {
                assert_eq!(
                    args.ids,
                    Some(vec!["REQ-1".to_string(), "REQ-2".to_string()])
                );
            }
            other => panic!("unexpected query: {:?}", other),
        }
    }

    #[test]
    fn test_build_remote_command_for_shell_exec_shell_text_uses_shell_exec_kind() {
        let built = build_remote_command(&RemoteCommands::Command {
            action: RemoteCommandCommands::Exec(Box::new(RemoteCommandExecArgs {
                cwd: Some("/srv/api".to_string()),
                env: vec![
                    ("NODE_ENV".to_string(), "production".to_string()),
                    ("REGION".to_string(), "sg".to_string()),
                ],
                timeout_ms: Some(600_000),
                shell_text: Some("printf hello".to_string()),
                argv: Vec::new(),
                stream: false,
                output_file: None,
                resume_call_id: None,
                no_verify_digest: false,
            })),
        });

        assert_eq!(built.kind, CommandKind::ShellExec);
        assert_eq!(built.label, "shell.exec");
        assert_eq!(built.command.as_deref(), Some("printf hello"));
        assert!(built.args_json.is_none());
        assert!(built.query.is_none());
        let shell_exec = built.shell_exec.expect("shell_exec payload should exist");
        assert_eq!(shell_exec.exec_mode, "shell_text");
        assert_eq!(shell_exec.command_text.as_deref(), Some("printf hello"));
        assert_eq!(shell_exec.cwd.as_deref(), Some("/srv/api"));
        assert_eq!(shell_exec.timeout_ms, Some(600_000));
        assert_eq!(
            shell_exec.env.as_ref().and_then(|env| env.get("NODE_ENV")),
            Some(&"production".to_string())
        );
    }

    #[test]
    fn test_build_remote_command_for_shell_exec_argv_uses_program_preview() {
        let built = build_remote_command(&RemoteCommands::Command {
            action: RemoteCommandCommands::Exec(Box::new(RemoteCommandExecArgs {
                cwd: None,
                env: vec![("A".to_string(), "1".to_string())],
                timeout_ms: Some(5_000),
                shell_text: None,
                argv: vec![
                    "/bin/echo".to_string(),
                    "hello".to_string(),
                    "--flag".to_string(),
                ],
                stream: false,
                output_file: None,
                resume_call_id: None,
                no_verify_digest: false,
            })),
        });

        assert_eq!(built.kind, CommandKind::ShellExec);
        assert_eq!(built.command.as_deref(), Some("/bin/echo"));
        let shell_exec = built.shell_exec.expect("shell_exec payload should exist");
        assert_eq!(shell_exec.exec_mode, "argv_exec");
        assert_eq!(
            shell_exec.argv,
            Some(vec![
                "/bin/echo".to_string(),
                "hello".to_string(),
                "--flag".to_string()
            ])
        );
        assert_eq!(shell_exec.timeout_ms, Some(5_000));
    }

    #[test]
    fn test_build_remote_command_populates_streaming_prefs_from_exec_args() {
        let built = build_remote_command(&RemoteCommands::Command {
            action: RemoteCommandCommands::Exec(Box::new(RemoteCommandExecArgs {
                cwd: None,
                env: vec![],
                timeout_ms: None,
                shell_text: Some("yes".to_string()),
                argv: Vec::new(),
                stream: true,
                output_file: Some("/tmp/out.bin".to_string()),
                resume_call_id: Some("ab12cd34".to_string()),
                no_verify_digest: true,
            })),
        });

        let prefs = built
            .streaming_prefs
            .expect("streaming_prefs should be populated for shell.exec");
        assert!(prefs.stream);
        assert_eq!(
            prefs.output_file,
            Some(std::path::PathBuf::from("/tmp/out.bin"))
        );
        assert_eq!(prefs.resume_call_id.as_deref(), Some("ab12cd34"));
        assert!(prefs.no_verify_digest);
        assert!(prefs.is_streaming());
    }

    #[test]
    fn test_streaming_prefs_is_streaming_implied_by_output_or_resume() {
        let mut p = StreamingPrefs::default();
        assert!(!p.is_streaming());
        p.output_file = Some(std::path::PathBuf::from("/tmp/x"));
        assert!(p.is_streaming());
        let mut p2 = StreamingPrefs::default();
        p2.resume_call_id = Some("abc".to_string());
        assert!(p2.is_streaming());
        let mut p3 = StreamingPrefs::default();
        p3.stream = true;
        assert!(p3.is_streaming());
    }

    #[test]
    fn test_build_remote_command_non_shell_exec_has_no_streaming_prefs() {
        let built = build_remote_command(&RemoteCommands::Status);
        assert!(built.streaming_prefs.is_none());
    }

    #[test]
    fn test_should_print_remote_progress_banner_for_terminal_output() {
        assert!(should_print_remote_progress_banner(true));
    }

    #[test]
    fn test_should_print_remote_progress_banner_skips_non_terminal_output() {
        assert!(!should_print_remote_progress_banner(false));
    }

    #[test]
    fn test_should_stream_remote_command_marks_search_variants_only() {
        let search = BuiltRemoteCommand {
            kind: CommandKind::QueryReadonly,
            label: "search.stream".to_string(),
            command: None,
            args_json: None,
            query: Some(CanonicalQueryCommand::Search(SearchArgs::default())),
            shell_exec: None,
            render: RemoteRenderMode::Search {
                format: OutputFormat::Table,
                no_color: false,
                keyword: String::new(),
                max_scan: None,
                max_results: None,
            },
            streaming_prefs: None,
        };
        let status = BuiltRemoteCommand {
            kind: CommandKind::QueryReadonly,
            label: "status".to_string(),
            command: Some("status".to_string()),
            args_json: None,
            query: None,
            shell_exec: None,
            render: RemoteRenderMode::Raw,
            streaming_prefs: None,
        };
        let get = BuiltRemoteCommand {
            kind: CommandKind::QueryReadonly,
            label: "traffic.get".to_string(),
            command: None,
            args_json: None,
            query: Some(CanonicalQueryCommand::TrafficGet(TrafficGetArgs {
                id: "REQ-1".to_string(),
                request_body: false,
                response_body: false,
            })),
            shell_exec: None,
            render: RemoteRenderMode::TrafficGet {
                format: OutputFormat::JsonPretty,
                no_color: false,
            },
            streaming_prefs: None,
        };
        assert!(should_stream_remote_command(&search));
        assert!(!should_stream_remote_command(&status));
        assert!(!should_stream_remote_command(&get));
    }

    #[test]
    fn test_retryable_start_pairing_overload_matches_503_overload_protect() {
        let err = BifrostError::Network(
            "start_pairing failed with status 503 Service Unavailable: overload-protect triggered"
                .to_string(),
        );

        assert!(is_retryable_start_pairing_overload(&err));
    }

    #[test]
    fn test_retryable_start_pairing_overload_rejects_non_overload_errors() {
        let invalid_code = BifrostError::Network(
            "start_pairing failed with status 400 Bad Request: pair_code_expired".to_string(),
        );
        let unrelated_503 = BifrostError::Network(
            "watch_pairing failed with status 503 Service Unavailable: upstream maintenance"
                .to_string(),
        );

        assert!(!is_retryable_start_pairing_overload(&invalid_code));
        assert!(!is_retryable_start_pairing_overload(&unrelated_503));
    }

    #[test]
    fn test_classify_delete_grant_failure_treats_404_as_already_missing() {
        let outcome = classify_delete_grant_failure(
            reqwest::StatusCode::NOT_FOUND,
            "{\"code\":404,\"message\":\"grant_not_found\"}",
        );

        assert_eq!(outcome, Some(DeleteGrantOutcome::AlreadyMissing));
    }

    #[test]
    fn test_classify_delete_grant_failure_rejects_other_errors() {
        let outcome = classify_delete_grant_failure(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            "{\"code\":500,\"message\":\"relay_internal_error\"}",
        );

        assert_eq!(outcome, None);
    }

    #[test]
    fn test_start_pairing_overload_error_is_actionable() {
        let err = start_pairing_overload_error();
        let BifrostError::Network(message) = err else {
            panic!("expected network error");
        };

        assert!(message.contains("pairing service is temporarily busy"));
        assert!(message.contains("bifrost remote connect <pair-code>"));
    }

    #[test]
    fn test_parse_call_terminal_status_accepts_cancelled() {
        let payload = serde_json::json!({
            "call_id": "call-1",
            "status": "cancelled"
        });

        assert_eq!(parse_call_terminal_status(&payload), Some("cancelled"));
    }

    #[test]
    fn test_parse_call_terminal_status_rejects_streaming() {
        let payload = serde_json::json!({
            "call_id": "call-1",
            "status": "streaming"
        });

        assert_eq!(parse_call_terminal_status(&payload), None);
    }

    #[test]
    fn test_retryable_cancel_settle_error_matches_429_call_events() {
        let err = BifrostError::Network(
            "call events returned 429 Too Many Requests: {\"code\":-1,\"message\":\"too many requests, retry after 15s\"}".to_string(),
        );

        assert!(is_retryable_cancel_settle_error(&err));
    }

    #[test]
    fn test_ambiguous_cancel_settle_result_requires_synthesis() {
        let result = CallResult::default();

        assert!(is_ambiguous_cancel_settle_result(&result));
    }

    #[test]
    fn test_build_ssh_connect_signature_payload_matches_relay_shape() {
        let payload = build_ssh_connect_signature_payload(
            "challenge-value",
            "challenge-id",
            "BF-0123456789ABCDEF",
            1700000000000,
        );

        assert_eq!(
            payload,
            "{\"challenge\":\"challenge-value\",\"challenge_id\":\"challenge-id\",\"device_code\":\"BF-0123456789ABCDEF\",\"timestamp\":1700000000000}"
        );
    }

    #[test]
    fn test_parse_bifrost_key_file_extracts_pkcs8_and_device_code() {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).expect("generate pkcs8");
        let rendered = format!(
            "{BIFROST_KEY_BEGIN}\nDevice-Code: BF-0123456789ABCDEF\n{}\n{BIFROST_KEY_END}\n",
            base64::engine::general_purpose::STANDARD.encode(pkcs8.as_ref())
        );

        let (parsed_pkcs8, device_code) = parse_bifrost_key_file(&rendered)
            .expect("parse should succeed")
            .expect("bifrost key should be detected");

        assert_eq!(device_code, "BF-0123456789ABCDEF");
        assert_eq!(parsed_pkcs8, pkcs8.as_ref());
    }

    #[test]
    fn test_encrypt_local_secret_roundtrip() {
        init_test_data_dir();
        let plaintext = b"shared-secret-test";

        let encrypted = encrypt_local_secret(plaintext).expect("encrypt local secret");
        let decrypted = decrypt_local_secret(&encrypted).expect("decrypt local secret");

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_encrypt_remote_command_uses_encrypted_only_payload() {
        let transport = OpenCallTransportContext {
            caller_ephemeral_pub: "caller-epk".to_string(),
            client_ephemeral_pub: "client-epk".to_string(),
            shared_secret: b"01234567890123456789012345678901".to_vec(),
        };

        let payload = encrypt_remote_command(
            CommandKind::QueryReadonly,
            Some("status"),
            None,
            None,
            None,
            "grant-123",
            &transport,
        )
        .expect("encrypt command");

        assert_eq!(payload.version, ENCRYPTED_OPEN_CALL_VERSION);
        let decrypted = decrypt_remote_command_for_test(
            &payload,
            &transport.shared_secret,
            "grant-123",
            &transport.caller_ephemeral_pub,
            &transport.client_ephemeral_pub,
            CommandKind::QueryReadonly,
        );
        assert_eq!(decrypted.kind, "query.readonly");
        assert_eq!(decrypted.command, "status");
        assert_eq!(decrypted.args_json, None);
        assert_eq!(decrypted.query, None);
    }

    #[test]
    fn test_encrypt_remote_command_includes_shell_exec_fields() {
        let transport = OpenCallTransportContext {
            caller_ephemeral_pub: "caller-epk".to_string(),
            client_ephemeral_pub: "client-epk".to_string(),
            shared_secret: b"01234567890123456789012345678901".to_vec(),
        };

        let shell_exec = ShellExecPayload {
            exec_mode: "argv_exec".to_string(),
            argv: Some(vec![
                "./deploy.sh".to_string(),
                "--env".to_string(),
                "prod".to_string(),
            ]),
            command_text: None,
            cwd: Some("/srv/api".to_string()),
            env: Some(BTreeMap::from([(
                "NODE_ENV".to_string(),
                "production".to_string(),
            )])),
            timeout_ms: Some(600_000),
        };

        let payload = encrypt_remote_command(
            CommandKind::ShellExec,
            Some("./deploy.sh"),
            None,
            None,
            Some(&shell_exec),
            "grant-123",
            &transport,
        )
        .expect("encrypt command");

        let decrypted = decrypt_remote_command_for_test(
            &payload,
            &transport.shared_secret,
            "grant-123",
            &transport.caller_ephemeral_pub,
            &transport.client_ephemeral_pub,
            CommandKind::ShellExec,
        );
        assert_eq!(decrypted.kind, "shell.exec");
        assert_eq!(decrypted.command, "./deploy.sh");
        assert_eq!(decrypted.exec_mode.as_deref(), Some("argv_exec"));
        assert_eq!(
            decrypted.argv,
            Some(vec![
                "./deploy.sh".to_string(),
                "--env".to_string(),
                "prod".to_string()
            ])
        );
        assert_eq!(decrypted.cwd.as_deref(), Some("/srv/api"));
        assert_eq!(decrypted.timeout_ms, Some(600_000));
        assert_eq!(
            decrypted.env.as_ref().and_then(|env| env.get("NODE_ENV")),
            Some(&"production".to_string())
        );
    }

    #[test]
    fn test_open_call_request_serialization_omits_plaintext_command_fields() {
        let request = OpenCallRequest {
            grant_id: "grant-1".to_string(),
            client_instance_id: "client-1".to_string(),
            caller_fingerprint: "fp-1".to_string(),
            command_summary: OpenCallCommandSummary {
                command_preview: "traffic.get".to_string(),
                masked_args_json: Some(
                    r#"{"id":"REQ-1","request_body":true,"response_body":true}"#.to_string(),
                ),
            },
            command_kind: CommandKind::QueryReadonly,
            command_encrypted: EncryptedPayload {
                version: ENCRYPTED_OPEN_CALL_VERSION,
                nonce: "nonce".to_string(),
                ciphertext: "ciphertext".to_string(),
                tag: "tag".to_string(),
                aad: None,
            },
        };

        let json = serde_json::to_value(&request).expect("serialize request");
        assert_eq!(json["command_kind"], "query.readonly");
        assert!(json.get("command_encrypted").is_some());
        assert!(json.get("command").is_none());
        assert_eq!(json["command_summary"]["command_preview"], "traffic.get");
        assert_eq!(
            json["command_summary"]["masked_args_json"],
            r#"{"id":"REQ-1","request_body":true,"response_body":true}"#
        );
    }

    #[test]
    fn test_build_open_call_command_summary_uses_label_and_args_json() {
        let command = BuiltRemoteCommand {
            kind: CommandKind::QueryReadonly,
            label: "traffic.list".to_string(),
            command: Some("traffic.list".to_string()),
            args_json: Some(r#"{"limit":7,"host":"127.0.0.1"}"#.to_string()),
            query: None,
            shell_exec: None,
            render: RemoteRenderMode::Raw,
            streaming_prefs: None,
        };

        let summary = build_open_call_command_summary(&command);
        assert_eq!(summary.command_preview, "traffic.list");
        assert_eq!(
            summary.masked_args_json.as_deref(),
            Some(r#"{"limit":7,"host":"127.0.0.1"}"#)
        );
    }

    #[test]
    fn test_merge_transport_context_requires_encrypted_context() {
        let conn = LocalConnection {
            client_instance_id: "client-1".to_string(),
            device_name: "device".to_string(),
            platform: "macos".to_string(),
            relay_url: "https://relay".to_string(),
            grant_id: "grant-1".to_string(),
            grant_mode: "permanent".to_string(),
            caller_fingerprint: "fp-1".to_string(),
            connected_at: 1,
            auth_method: Some("pair_code".to_string()),
            ssh_key_fingerprint: None,
            ssh_key_source: None,
            device_code: None,
            transport_context_version: Some(TRANSPORT_CONTEXT_VERSION),
            caller_ephemeral_pub: Some("caller-epk".to_string()),
            client_ephemeral_pub: Some("client-epk".to_string()),
            shared_secret_encrypted: None,
        };
        let grant = GrantInfo {
            grant_id: "grant-1".to_string(),
            status: "active".to_string(),
            caller_ephemeral_pub: None,
            client_ephemeral_pub: None,
        };

        let err = merge_transport_context(&conn, &grant).expect_err("should require shared secret");
        assert!(err
            .to_string()
            .contains("saved connection is missing encrypted transport secret"));
    }

    #[test]
    fn test_reconcile_reusable_grant_updates_saved_grant_when_transport_matches() {
        let mut conn = LocalConnection {
            client_instance_id: "client-1".to_string(),
            device_name: "device".to_string(),
            platform: "macos".to_string(),
            relay_url: "https://relay".to_string(),
            grant_id: "saved-grant".to_string(),
            grant_mode: "permanent".to_string(),
            caller_fingerprint: "fp-1".to_string(),
            connected_at: 1,
            auth_method: Some("pair_code".to_string()),
            ssh_key_fingerprint: None,
            ssh_key_source: None,
            device_code: None,
            transport_context_version: Some(TRANSPORT_CONTEXT_VERSION),
            caller_ephemeral_pub: Some("saved-caller-epk".to_string()),
            client_ephemeral_pub: Some("saved-client-epk".to_string()),
            shared_secret_encrypted: Some("secret".to_string()),
        };
        let relay_grant = GrantInfo {
            grant_id: "relay-grant".to_string(),
            status: "active".to_string(),
            caller_ephemeral_pub: Some("saved-caller-epk".to_string()),
            client_ephemeral_pub: Some("saved-client-epk".to_string()),
        };

        let preferred = reconcile_reusable_grant_for_transport(&mut conn, relay_grant)
            .expect("matching transport should reconcile");

        assert_eq!(preferred.grant_id, "relay-grant");
        assert_eq!(conn.grant_id, "relay-grant");
    }

    #[test]
    fn test_reconcile_reusable_grant_rejects_transport_mismatch() {
        let mut conn = LocalConnection {
            client_instance_id: "client-1".to_string(),
            device_name: "device".to_string(),
            platform: "macos".to_string(),
            relay_url: "https://relay".to_string(),
            grant_id: "saved-grant".to_string(),
            grant_mode: "permanent".to_string(),
            caller_fingerprint: "fp-1".to_string(),
            connected_at: 1,
            auth_method: Some("pair_code".to_string()),
            ssh_key_fingerprint: None,
            ssh_key_source: None,
            device_code: None,
            transport_context_version: Some(TRANSPORT_CONTEXT_VERSION),
            caller_ephemeral_pub: Some("saved-caller-epk".to_string()),
            client_ephemeral_pub: Some("saved-client-epk".to_string()),
            shared_secret_encrypted: Some("secret".to_string()),
        };
        let relay_grant = GrantInfo {
            grant_id: "relay-grant".to_string(),
            status: "active".to_string(),
            caller_ephemeral_pub: Some("relay-caller-epk".to_string()),
            client_ephemeral_pub: Some("saved-client-epk".to_string()),
        };

        let err = reconcile_reusable_grant_for_transport(&mut conn, relay_grant)
            .expect_err("mismatched transport should require reconnect");
        assert!(err
            .to_string()
            .contains("saved connection transport no longer matches relay reusable authorization"));
    }

    #[test]
    fn test_upsert_local_connection_replaces_duplicate_client_entries() {
        let mut connections = vec![
            LocalConnection {
                client_instance_id: "client-1".to_string(),
                device_name: "old".to_string(),
                platform: "macos".to_string(),
                relay_url: "https://relay".to_string(),
                grant_id: "grant-old".to_string(),
                grant_mode: "permanent".to_string(),
                caller_fingerprint: "fp-1".to_string(),
                connected_at: 1,
                auth_method: Some("pair_code".to_string()),
                ssh_key_fingerprint: None,
                ssh_key_source: None,
                device_code: None,
                transport_context_version: Some(TRANSPORT_CONTEXT_VERSION),
                caller_ephemeral_pub: Some("old-caller".to_string()),
                client_ephemeral_pub: Some("old-client".to_string()),
                shared_secret_encrypted: Some("secret-1".to_string()),
            },
            LocalConnection {
                client_instance_id: "client-2".to_string(),
                device_name: "other".to_string(),
                platform: "linux".to_string(),
                relay_url: "https://relay".to_string(),
                grant_id: "grant-other".to_string(),
                grant_mode: "permanent".to_string(),
                caller_fingerprint: "fp-2".to_string(),
                connected_at: 2,
                auth_method: Some("pair_code".to_string()),
                ssh_key_fingerprint: None,
                ssh_key_source: None,
                device_code: None,
                transport_context_version: Some(TRANSPORT_CONTEXT_VERSION),
                caller_ephemeral_pub: Some("other-caller".to_string()),
                client_ephemeral_pub: Some("other-client".to_string()),
                shared_secret_encrypted: Some("secret-2".to_string()),
            },
        ];

        upsert_local_connection(
            &mut connections,
            LocalConnection {
                client_instance_id: "client-1".to_string(),
                device_name: "new".to_string(),
                platform: "macos".to_string(),
                relay_url: "https://relay".to_string(),
                grant_id: "grant-new".to_string(),
                grant_mode: "permanent".to_string(),
                caller_fingerprint: "fp-1".to_string(),
                connected_at: 3,
                auth_method: Some("pair_code".to_string()),
                ssh_key_fingerprint: None,
                ssh_key_source: None,
                device_code: None,
                transport_context_version: Some(TRANSPORT_CONTEXT_VERSION),
                caller_ephemeral_pub: Some("new-caller".to_string()),
                client_ephemeral_pub: Some("new-client".to_string()),
                shared_secret_encrypted: Some("secret-3".to_string()),
            },
        );

        assert_eq!(connections.len(), 2);
        let replaced = connections
            .iter()
            .find(|conn| conn.client_instance_id == "client-1")
            .expect("replaced connection should remain");
        assert_eq!(replaced.device_name, "new");
        assert_eq!(replaced.grant_id, "grant-new");
    }

    #[test]
    fn test_is_grant_scope_mismatch_error_detects_open_call_403() {
        let err = BifrostError::Network(
            "open_call failed with status 403 Forbidden: {\"code\":403,\"message\":\"grant_scope_mismatch\",\"data\":null}".to_string(),
        );

        assert!(is_grant_scope_mismatch_error(&err));
    }
}
