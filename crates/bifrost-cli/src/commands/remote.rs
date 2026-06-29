use std::collections::{BTreeMap, HashSet};
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::commands::caller_stream_frame::{
    parse_stream_frame_from_sse_data, CallerStreamState, StreamDecision,
};
use base64::Engine;
use bifrost_command::{
    CanonicalQueryCommand, FilterCondition, SearchArgs, SearchFilters, SearchScope, TrafficGetArgs,
    TrafficListArgs, TrafficListDirection,
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
    RemoteCommandExecArgs, RemoteCommands, RemoteConnCommands, RemoteFileCommands,
    RemoteJobCommands, RemoteRunArgs, RemoteSearchArgs, RemoteTrafficCommands,
};

mod pop;

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
const CALLER_IDENTITY_FILE: &str = "remote-caller-identity.json";
const REMOTE_JOBS_FILE: &str = "remote-jobs.json";
const START_PAIRING_OVERLOAD_RETRY_DELAYS_MS: [u64; 3] = [300, 700, 1500];
const CANCEL_SETTLE_RETRY_DELAYS_MS: [u64; 4] = [200, 500, 1000, 2000];
const SSH_CONNECT_TIMEOUT_SECS: u64 = 30;
const DEFAULT_REMOTE_SSH_KEY_ENV_VAR: &str = "BIFROST_REMOTE_SSH_KEY";
const DEFAULT_REMOTE_CLIENT_ID_ENV_VAR: &str = "BIFROST_REMOTE_CLIENT_ID";
const DEFAULT_REMOTE_SSH_KEY_ENV_SPEC: &str = "env:BIFROST_REMOTE_SSH_KEY";
const BIFROST_KEY_BEGIN: &str = "-----BEGIN BIFROST KEY-----";
const BIFROST_KEY_END: &str = "-----END BIFROST KEY-----";
const PKCS8_KEY_BEGIN: &str = "-----BEGIN PRIVATE KEY-----";
const PKCS8_KEY_END: &str = "-----END PRIVATE KEY-----";
const TRANSPORT_CONTEXT_VERSION: u32 = 2;
const ENCRYPTED_OPEN_CALL_VERSION: u32 = 2;
const LOCAL_SECRET_FORMAT_VERSION: u32 = 1;
const OPEN_CALL_HKDF_INFO_PREFIX: &[u8] = b"bifrost-open-call-v2";
const CALL_EVENT_HKDF_INFO_PREFIX: &[u8] = b"bifrost-e2e-v1";
const GRANT_SESSION_REFRESH_SKEW_SECS: i64 = 60;

#[cfg(test)]
static REMOTE_TEST_DATA_DIR_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn is_false(value: &bool) -> bool {
    !*value
}

fn should_print_remote_progress_banner(stdout_is_terminal: bool) -> bool {
    stdout_is_terminal
}

fn remote_client_short_id(client_id: &str) -> &str {
    &client_id[..client_id.len().min(12)]
}

fn saved_connection_selector_matches(conn: &LocalConnection, selector: &str) -> bool {
    conn.client_instance_id.starts_with(selector)
        || (!conn.device_name.is_empty() && conn.device_name.starts_with(selector))
}

fn saved_connection_choices(connections: &[LocalConnection]) -> String {
    connections
        .iter()
        .map(|conn| {
            let short_id = remote_client_short_id(&conn.client_instance_id);
            let label = if conn.device_name.is_empty() {
                "(unnamed)"
            } else {
                conn.device_name.as_str()
            };
            format!("  --client-id {short_id}    {label}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn multiple_connections_error(connections: &[LocalConnection]) -> String {
    format!(
        "multiple saved connections, please specify a target after `remote`, before the subcommand:\n{}\n→ Example: bifrost remote --client-id <id-or-label-prefix> conn status\n→ Or set {DEFAULT_REMOTE_CLIENT_ID_ENV_VAR}=<id-or-label-prefix>",
        saved_connection_choices(connections)
    )
}

fn remote_call_idle_timeout_message(timeout_secs: u64) -> String {
    format!(
        "remote call received no events for {timeout_secs}s (idle timeout, not total process timeout)\n→ Quiet or long-running commands should use `bifrost remote exec --detach ...` and then `bifrost remote job watch <call_id> --output-file <log>`"
    )
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
    login: bool,
    stdin_mode: Option<String>,
    pty: Option<PtyRequestPayload>,
    output_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PtyRequestPayload {
    enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    rows: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cols: Option<u16>,
}

/// PR #5e-1: caller-side streaming preferences derived from CLI flags.
/// `stream=true` routes the call through `subscribe_call_events_streaming`.
/// `output_file=Some(..)` implies streaming and writes stdout to that file.
/// `resume_call_id=Some(..)` bypasses open_call and rejoins an existing
/// session (PR #5e-3).
#[derive(Debug, Clone, Default)]
struct StreamingPrefs {
    pub stream: bool,
    pub output_file: Option<std::path::PathBuf>,
    pub resume_call_id: Option<String>,
    pub resume_relay_token: Option<String>,
    pub no_verify_digest: bool,
    pub interactive: bool,
    pub stdin: bool,
}

impl StreamingPrefs {
    /// Effective streaming mode: explicit --stream, or implied by --output-file
    /// or --resume-call-id.
    pub fn is_streaming(&self) -> bool {
        self.stream || self.output_file.is_some() || self.resume_call_id.is_some()
    }
}

/// PR #5e-3: cross-field validation for resume flags.
/// `--resume-call-id` and `--resume-relay-token` must be provided together; if
/// either appears without the other, return a helpful error string.
fn validate_streaming_prefs(prefs: &StreamingPrefs) -> Result<(), String> {
    match (
        prefs.resume_call_id.as_ref(),
        prefs.resume_relay_token.as_ref(),
    ) {
        (Some(_), None) => Err(
            "--resume-call-id requires --resume-relay-token to rejoin an existing call".to_string(),
        ),
        (None, Some(_)) => Err(
            "--resume-relay-token requires --resume-call-id to rejoin an existing call".to_string(),
        ),
        _ => Ok(()),
    }
}

/// Identifies which `file.*` operation produced a result, so the renderer can
/// turn the server's JSON response into human-friendly output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteFileOp {
    Read,
    ReadMany,
    List,
    Stat,
    Glob,
    Find,
    Hash,
    Outline,
    Write,
    Edit,
    Mkdir,
    Move,
    Delete,
    Patch,
}

#[derive(Debug, Clone)]
enum RemoteRenderMode {
    Raw,
    File {
        op: RemoteFileOp,
        json: bool,
    },
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    grant_session_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    grant_session_expires_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConnectionsFile {
    version: u32,
    connections: Vec<LocalConnection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RemoteJobRecord {
    call_id: String,
    relay_token_encrypted: String,
    relay_url: String,
    client_instance_id: Option<String>,
    device_name: Option<String>,
    command_preview: Option<String>,
    status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    exit_code: Option<i32>,
    created_at: u64,
    updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RemoteJobsFile {
    version: u32,
    jobs: Vec<RemoteJobRecord>,
}

fn connections_path() -> PathBuf {
    bifrost_storage::data_dir().join(CONNECTIONS_FILE)
}

fn connections_key_path() -> PathBuf {
    bifrost_storage::data_dir().join(CONNECTIONS_KEY_FILE)
}

fn caller_identity_path() -> PathBuf {
    bifrost_storage::data_dir().join(CALLER_IDENTITY_FILE)
}

fn remote_jobs_path() -> PathBuf {
    bifrost_storage::data_dir().join(REMOTE_JOBS_FILE)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CallerIdentityFile {
    version: u32,
    caller_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    caller_pubkey_b64: Option<String>,
}

struct CallerPopIdentity {
    caller_fingerprint: String,
    key_pair: Ed25519KeyPair,
}

fn default_remote_device_key_path() -> PathBuf {
    bifrost_storage::data_dir().join("remote-device.key")
}

fn load_or_create_caller_identity() -> bifrost_core::Result<CallerPopIdentity> {
    load_or_create_caller_identity_at(&caller_identity_path(), &default_remote_device_key_path())
}

#[cfg(test)]
fn load_or_create_caller_fingerprint_at(path: &Path) -> bifrost_core::Result<String> {
    let key_path = path
        .parent()
        .map(|parent| parent.join("remote-device.key"))
        .unwrap_or_else(default_remote_device_key_path);
    Ok(load_or_create_caller_identity_at(path, &key_path)?.caller_fingerprint)
}

fn load_or_create_caller_identity_at(
    identity_path: &Path,
    key_path: &Path,
) -> bifrost_core::Result<CallerPopIdentity> {
    let key_pair = load_or_create_caller_pop_key(key_path)?;
    let caller_pubkey_b64 = pop::caller_pubkey_b64(&key_pair);
    let caller_fingerprint = load_or_create_caller_fingerprint_value(identity_path)?;

    write_caller_identity_file(identity_path, &caller_fingerprint, Some(&caller_pubkey_b64))?;
    Ok(CallerPopIdentity {
        caller_fingerprint,
        key_pair,
    })
}

fn load_or_create_caller_fingerprint_value(path: &Path) -> bifrost_core::Result<String> {
    if path.exists() {
        let content = std::fs::read_to_string(path).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "read {}: {e}",
                path.display()
            )))
        })?;
        let file: CallerIdentityFile = serde_json::from_str(&content)
            .map_err(|e| BifrostError::Config(format!("parse {}: {e}", path.display())))?;
        if is_valid_caller_fingerprint(&file.caller_fingerprint) {
            return Ok(file.caller_fingerprint);
        }
        warn!(
            path = %path.display(),
            "remote caller identity file contained an invalid caller_fingerprint; rotating it"
        );
    }

    let caller_fingerprint = generate_random_caller_fingerprint()?;
    write_caller_identity_file(path, &caller_fingerprint, None)?;
    Ok(caller_fingerprint)
}

fn write_caller_identity_file(
    path: &Path,
    caller_fingerprint: &str,
    caller_pubkey_b64: Option<&str>,
) -> bifrost_core::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "mkdir {}: {e}",
                parent.display()
            )))
        })?;
    }
    let content = serde_json::to_string_pretty(&CallerIdentityFile {
        version: 1,
        caller_fingerprint: caller_fingerprint.to_string(),
        caller_pubkey_b64: caller_pubkey_b64.map(str::to_string),
    })
    .map_err(|e| BifrostError::Config(format!("serialize caller identity: {e}")))?;
    std::fs::write(path, content).map_err(|e| {
        BifrostError::Io(std::io::Error::other(format!(
            "write {}: {e}",
            path.display()
        )))
    })?;
    Ok(())
}

fn load_or_create_caller_pop_key(path: &Path) -> bifrost_core::Result<Ed25519KeyPair> {
    if path.exists() {
        return Ok(load_ssh_key(path.to_string_lossy().as_ref(), None)?.key_pair);
    }

    let rng = SystemRandom::new();
    let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng)
        .map_err(|_| BifrostError::Config("generate caller PoP key failed".to_string()))?;
    let key_pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref())
        .map_err(|_| BifrostError::Config("parse generated caller PoP key failed".to_string()))?;
    let public_key_der = ed25519_public_key_to_spki_der(key_pair.public_key().as_ref());
    let device_code = derive_device_code(&public_key_der);
    let rendered = render_bifrost_key_file(&device_code, pkcs8.as_ref());

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "mkdir {}: {e}",
                parent.display()
            )))
        })?;
    }
    std::fs::write(path, rendered).map_err(|e| {
        BifrostError::Io(std::io::Error::other(format!(
            "write {}: {e}",
            path.display()
        )))
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
            tracing::warn!("failed to set 0600 permissions on {}: {e}", path.display());
        }
    }
    Ok(key_pair)
}

fn render_bifrost_key_file(device_code: &str, private_key_pkcs8: &[u8]) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(private_key_pkcs8);
    format!("{BIFROST_KEY_BEGIN}\nDevice-Code: {device_code}\n{encoded}\n{BIFROST_KEY_END}\n")
}

fn generate_random_caller_fingerprint() -> bifrost_core::Result<String> {
    let rng = SystemRandom::new();
    let mut bytes = [0u8; 16];
    rng.fill(&mut bytes)
        .map_err(|_| BifrostError::Config("generate remote caller identity failed".to_string()))?;
    Ok(format!("caller-{}", hex_lower(&bytes)))
}

fn is_valid_caller_fingerprint(value: &str) -> bool {
    value
        .strip_prefix("caller-")
        .is_some_and(|rest| rest.len() == 32 && rest.bytes().all(|b| b.is_ascii_hexdigit()))
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

fn load_remote_jobs() -> bifrost_core::Result<Vec<RemoteJobRecord>> {
    let path = remote_jobs_path();
    load_remote_jobs_from_path(&path)
}

fn load_remote_jobs_from_path(path: &Path) -> bifrost_core::Result<Vec<RemoteJobRecord>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(path).map_err(|e| {
        BifrostError::Io(std::io::Error::other(format!(
            "read {}: {e}",
            path.display()
        )))
    })?;
    let file: RemoteJobsFile = serde_json::from_str(&content)
        .map_err(|e| BifrostError::Config(format!("parse {}: {e}", path.display())))?;
    Ok(file.jobs)
}

fn save_remote_jobs_to_path(path: &Path, jobs: &[RemoteJobRecord]) -> bifrost_core::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "mkdir {}: {e}",
                parent.display()
            )))
        })?;
    }
    let file = RemoteJobsFile {
        version: 1,
        jobs: jobs.to_vec(),
    };
    let content = serde_json::to_string_pretty(&file)
        .map_err(|e| BifrostError::Config(format!("serialize remote jobs: {e}")))?;
    std::fs::write(path, content).map_err(|e| {
        BifrostError::Io(std::io::Error::other(format!(
            "write {}: {e}",
            path.display()
        )))
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
            tracing::warn!("failed to set 0600 permissions on {}: {e}", path.display());
        }
    }
    Ok(())
}

fn remember_remote_job(record: RemoteJobRecord) -> bifrost_core::Result<()> {
    let path = remote_jobs_path();
    remember_remote_job_at(&path, record)
}

fn remember_remote_job_at(path: &Path, record: RemoteJobRecord) -> bifrost_core::Result<()> {
    let mut jobs = load_remote_jobs_from_path(path)?;
    jobs.retain(|existing| existing.call_id != record.call_id);
    jobs.push(record);
    jobs.sort_by_key(|job| std::cmp::Reverse(job.updated_at));
    save_remote_jobs_to_path(path, &jobs)
}

fn resolve_remote_job(call_id: &str) -> bifrost_core::Result<(String, String)> {
    let jobs = load_remote_jobs()?;
    let job = jobs
        .iter()
        .find(|job| job.call_id == call_id)
        .ok_or_else(|| {
            BifrostError::Config(format!(
                "no local remote job cache for call_id {call_id}; run `bifrost remote job list` to see known jobs, or start the task again with `bifrost remote exec --detach`"
            ))
        })?;
    let token = decrypt_local_secret(&job.relay_token_encrypted).and_then(|bytes| {
        String::from_utf8(bytes)
            .map_err(|e| BifrostError::Config(format!("decode cached relay token failed: {e}")))
    })?;
    Ok((token, job.relay_url.clone()))
}

fn update_remote_job_status(
    call_id: &str,
    status: &str,
    exit_code: Option<i32>,
) -> bifrost_core::Result<()> {
    let path = remote_jobs_path();
    update_remote_job_status_at(&path, call_id, status, exit_code)
}

fn update_remote_job_status_at(
    path: &Path,
    call_id: &str,
    status: &str,
    exit_code: Option<i32>,
) -> bifrost_core::Result<()> {
    let mut jobs = load_remote_jobs_from_path(path)?;
    let Some(job) = jobs.iter_mut().find(|job| job.call_id == call_id) else {
        return Ok(());
    };
    job.status = status.to_string();
    job.exit_code = exit_code;
    job.updated_at = now_millis();
    save_remote_jobs_to_path(path, &jobs)
}

fn render_remote_job_list(jobs: &[RemoteJobRecord]) {
    if jobs.is_empty() {
        println!("No detached remote jobs remembered locally.");
        return;
    }
    println!(
        "{:<36} {:<10} {:<9} {:<18} COMMAND",
        "CALL_ID", "STATUS", "EXIT", "DEVICE"
    );
    for job in jobs {
        let exit = job
            .exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "-".to_string());
        println!(
            "{:<36} {:<10} {:<9} {:<18} {}",
            truncate(&job.call_id, 36),
            truncate(&job.status, 10),
            truncate(&exit, 9),
            truncate(job.device_name.as_deref().unwrap_or("-"), 18),
            job.command_preview.as_deref().unwrap_or("-")
        );
    }
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
    resolve_local_connection_with_terminal(connections, explicit_id, std::io::stdin().is_terminal())
}

fn resolve_local_connection_with_terminal(
    connections: &[LocalConnection],
    explicit_id: Option<&str>,
    stdin_is_terminal: bool,
) -> bifrost_core::Result<LocalConnection> {
    if let Some(prefix) = explicit_id {
        let matches: Vec<&LocalConnection> = connections
            .iter()
            .filter(|c| saved_connection_selector_matches(c, prefix))
            .collect();

        match matches.len() {
            0 => {
                let detail = if connections.is_empty() {
                    "no saved connections are available".to_string()
                } else {
                    format!(
                        "available targets:\n{}\n→ Put --client-id after `remote`, before the subcommand.",
                        saved_connection_choices(connections)
                    )
                };
                return Err(BifrostError::Config(format!(
                    "no saved connection matching '{prefix}'. {detail}\n→ If this is a new target, run `bifrost remote conn up <pair-code>` first"
                )));
            }
            1 => {
                let conn = matches[0];
                if conn.client_instance_id != prefix {
                    debug!(prefix = %prefix, full_id = %conn.client_instance_id, "resolved short client id from local connections");
                }
                return Ok(conn.clone());
            }
            n => {
                if !stdin_is_terminal {
                    return Err(BifrostError::Config(format!(
                        "ambiguous remote target selector '{prefix}' matches {n} saved connections, please be more specific:\n{}",
                        saved_connection_choices(&matches.into_iter().cloned().collect::<Vec<_>>())
                    )));
                }

                let items: Vec<String> = matches
                    .iter()
                    .map(|c| {
                        let short_id = remote_client_short_id(&c.client_instance_id);
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
            "no saved connection, please run `bifrost remote conn up <pair-code>` first"
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
            if !stdin_is_terminal {
                return Err(BifrostError::Config(multiple_connections_error(
                    connections,
                )));
            }

            let items: Vec<String> = connections
                .iter()
                .map(|c| {
                    let short_id = remote_client_short_id(&c.client_instance_id);
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
    #[serde(rename = "file")]
    File,
    #[serde(rename = "power.mgmt")]
    PowerMgmt,
}

impl CommandKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::QueryReadonly => "query.readonly",
            Self::ShellExec => "shell.exec",
            Self::File => "file",
            Self::PowerMgmt => "power.mgmt",
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
    #[serde(default, skip_serializing_if = "is_false")]
    login: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    stdin_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pty: Option<PtyRequestPayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_mode: Option<String>,
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
struct CallerInputFramePayload {
    data_b64: String,
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
        // The data dir holds the transport master key; restrict it to the
        // owner on Unix. Best-effort: log a warning on failure.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(e) = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            {
                tracing::warn!(
                    "failed to set 0700 permissions on {}: {e}",
                    parent.display()
                );
            }
        }
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
    // The transport master key must only be readable by its owner.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)) {
            tracing::warn!("failed to set 0600 permissions on {}: {e}", path.display());
        }
    }
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

fn encrypt_grant_session_token(token: &str) -> bifrost_core::Result<String> {
    encrypt_local_secret(token.as_bytes())
}

fn decrypt_grant_session_token(conn: &LocalConnection) -> bifrost_core::Result<String> {
    let encoded = conn.grant_session_token.as_deref().ok_or_else(|| {
        BifrostError::Config(
            "saved connection is missing grant_session_token; reconnect to refresh authorization"
                .to_string(),
        )
    })?;
    let decrypted = decrypt_local_secret(encoded)?;
    String::from_utf8(decrypted).map_err(|e| {
        BifrostError::Config(format!(
            "decrypt saved grant_session_token as utf-8 failed: {e}"
        ))
    })
}

fn store_grant_session_token(conn: &mut LocalConnection, token: &str) -> bifrost_core::Result<()> {
    conn.grant_session_token = Some(encrypt_grant_session_token(token)?);
    Ok(())
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
    let grant_id_changed =
        !grant.grant_id.is_empty() && !conn.grant_id.is_empty() && grant.grant_id != conn.grant_id;
    if grant_id_changed {
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
    }

    if let Some(token) = &grant.grant_session_token {
        store_grant_session_token(conn, token)?;
    }
    if let Some(expires_at) = &grant.expires_at {
        conn.grant_session_expires_at = Some(expires_at.clone());
    }
    if grant.caller_ephemeral_pub.is_some() {
        conn.caller_ephemeral_pub = grant.caller_ephemeral_pub.clone();
    }
    if grant.client_ephemeral_pub.is_some() {
        conn.client_ephemeral_pub = grant.client_ephemeral_pub.clone();
    }
    if grant.shared_secret_encrypted.is_some() {
        conn.shared_secret_encrypted = grant.shared_secret_encrypted.clone();
    }
    if !grant_id_changed {
        return Ok(grant);
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

fn local_grant_from_saved_session(conn: &LocalConnection) -> Option<GrantInfo> {
    if conn.grant_session_token.is_none()
        || conn.caller_ephemeral_pub.is_none()
        || conn.client_ephemeral_pub.is_none()
        || conn.shared_secret_encrypted.is_none()
    {
        return None;
    }

    Some(GrantInfo {
        grant_id: conn.grant_id.clone(),
        status: "active".to_string(),
        grant_session_token: None,
        expires_at: conn.grant_session_expires_at.clone(),
        grant_summary: None,
        caller_ephemeral_pub: conn.caller_ephemeral_pub.clone(),
        client_ephemeral_pub: conn.client_ephemeral_pub.clone(),
        shared_secret_encrypted: conn.shared_secret_encrypted.clone(),
    })
}

fn grant_session_needs_refresh(conn: &LocalConnection) -> bool {
    if conn.grant_session_token.is_none() {
        return true;
    }
    let Some(expires_at) = conn.grant_session_expires_at.as_deref() else {
        return true;
    };
    let Ok(expires_at) = chrono::DateTime::parse_from_rfc3339(expires_at) else {
        return true;
    };
    expires_at.timestamp() <= chrono::Utc::now().timestamp() + GRANT_SESSION_REFRESH_SKEW_SECS
}

fn is_grant_scope_mismatch_error(err: &BifrostError) -> bool {
    matches!(err, BifrostError::Network(msg)
        if msg.contains("open_call failed with status 403")
            && msg.contains("grant_scope_mismatch"))
}

fn is_grant_session_token_invalid_error(err: &BifrostError) -> bool {
    matches!(err, BifrostError::Network(msg)
        if msg.contains("open_call failed with status 401")
            && msg.contains("grant_session_token_invalid"))
}

fn is_stale_remote_grant_error(err: &BifrostError) -> bool {
    let BifrostError::Network(msg) = err else {
        return false;
    };
    if !(msg.contains("open_call failed with status 403")
        || msg.contains("open_call error code")
        || msg.contains("find_reusable_grant error code"))
    {
        return false;
    }
    msg.contains("grant_not_active")
        || msg.contains("grant_revoked")
        || msg.contains("grant_missing_shared_secret")
        || msg.contains("grant_not_found")
}

fn is_stale_grant_crypto_error(result: &CallResult) -> bool {
    if let Some(ref stderr) = result.stderr {
        return stderr.contains("missing grant shared secret");
    }
    false
}

fn remove_matching_local_connection(
    connections: &mut Vec<LocalConnection>,
    stale: &LocalConnection,
) -> bool {
    let before = connections.len();
    connections.retain(|c| {
        !(c.client_instance_id == stale.client_instance_id && c.relay_url == stale.relay_url)
    });
    connections.len() != before
}

fn remove_stale_local_connection(
    connections: &mut Vec<LocalConnection>,
    stale: &LocalConnection,
) -> bifrost_core::Result<bool> {
    let removed = remove_matching_local_connection(connections, stale);
    if removed {
        save_connections(connections)?;
    }
    Ok(removed)
}

fn stale_remote_grant_error(conn: &LocalConnection) -> BifrostError {
    let conn_label = if conn.device_name.is_empty() {
        &conn.client_instance_id
    } else {
        &conn.device_name
    };
    BifrostError::Config(format!(
        "authorization for '{conn_label}' expired or was revoked; local stale connection removed. Run `bifrost remote conn up <pair-code>` to re-connect."
    ))
}

fn shell_scope_upgrade_error(conn: &LocalConnection) -> BifrostError {
    if conn.auth_method.as_deref() == Some("ssh_publickey")
        && conn.ssh_key_source.is_some()
        && conn.device_code.is_some()
    {
        return BifrostError::Config(
            "saved remote authorization does not allow shell.exec. Ask the target device to raise this grant with `bifrost setting grant update --level shell` or `--level full`."
                .to_string(),
        );
    }

    BifrostError::Config(
        "saved remote authorization is read-only and does not allow shell.exec. Run `bifrost remote conn up <pair-code>` again and approve shell access on the remote device."
            .to_string(),
    )
}

fn derive_open_call_key(
    shared_secret: &[u8],
    _grant_id: &str,
    caller_ephemeral_pub: &str,
    client_ephemeral_pub: &str,
    command_kind: CommandKind,
) -> bifrost_core::Result<[u8; 32]> {
    // v5 keeps grant_id server-side, so caller and client derive this key from
    // the ECDH context and command kind only.
    let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, b"bifrost-open-call-v5");
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
        login: shell_exec.map(|payload| payload.login).unwrap_or(false),
        stdin_mode: shell_exec.and_then(|payload| payload.stdin_mode.clone()),
        pty: shell_exec.and_then(|payload| payload.pty.clone()),
        output_mode: shell_exec.and_then(|payload| payload.output_mode.clone()),
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

fn encrypt_caller_input_frame(
    transport: &OpenCallTransportContext,
    call_id: &str,
    seq: u64,
    bytes: &[u8],
) -> bifrost_core::Result<String> {
    let key_bytes = derive_call_event_key(
        &transport.shared_secret,
        call_id,
        &transport.caller_ephemeral_pub,
        &transport.client_ephemeral_pub,
    )?;
    let unbound = UnboundKey::new(&CHACHA20_POLY1305, &key_bytes).map_err(|_| {
        BifrostError::Config("initialize caller input frame encryption failed".to_string())
    })?;
    let key = LessSafeKey::new(unbound);
    let mut nonce = [0u8; NONCE_LEN];
    SystemRandom::new().fill(&mut nonce).map_err(|_| {
        BifrostError::Config("generate caller input frame nonce failed".to_string())
    })?;
    let mut sealed = serde_json::to_vec(&CallerInputFramePayload {
        data_b64: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
    .map_err(|e| BifrostError::Config(format!("serialize caller input frame failed: {e}")))?;
    key.seal_in_place_append_tag(
        Nonce::assume_unique_for_key(nonce),
        Aad::empty(),
        &mut sealed,
    )
    .map_err(|_| BifrostError::Config("encrypt caller input frame failed".to_string()))?;
    let tag = sealed.split_off(sealed.len().saturating_sub(16));
    serde_json::to_string(&serde_json::json!({
        "version": ENCRYPTED_OPEN_CALL_VERSION,
        "call_id": call_id,
        "seq": seq,
        "direction": "caller_to_client",
        "nonce": base64::engine::general_purpose::STANDARD.encode(nonce),
        "ciphertext": base64::engine::general_purpose::STANDARD.encode(sealed),
        "tag": base64::engine::general_purpose::STANDARD.encode(tag),
    }))
    .map_err(|e| BifrostError::Config(format!("serialize caller input envelope failed: {e}")))
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
    let caller_identity = load_or_create_caller_identity()?;
    let caller_fingerprint = caller_identity.caller_fingerprint.clone();

    // Extract --label from conn up if present
    let label = if let RemoteCommands::Conn {
        action: RemoteConnCommands::Up { label, .. },
    } = &opts.action
    {
        label.clone()
    } else {
        None
    };
    let caller_info = CallerInfo {
        fingerprint: caller_fingerprint.clone(),
        caller_pubkey: Some(pop::caller_pubkey_b64(&caller_identity.key_pair)),
        display_name: Some(label.clone().unwrap_or_else(|| hostname.clone())),
        user_agent: Some(CALLER_USER_AGENT.to_string()),
        platform: Some(std::env::consts::OS.to_string()),
        hostname: Some(hostname),
        username: Some(username),
        label,
        os_version: get_os_version(),
        arch: Some(std::env::consts::ARCH.to_string()),
    };

    if let RemoteCommands::Conn {
        action:
            RemoteConnCommands::Up {
                pair_code,
                ssh_key,
                device_code,
                ..
            },
    } = &opts.action
    {
        if let Some(ssh_key) = ssh_key {
            return handle_connect_with_ssh(
                &caller,
                ssh_key,
                device_code.as_deref(),
                &caller_info,
                &caller_identity,
                &opts.relay_url,
            )
            .await;
        }

        let pair_code = pair_code.as_deref().ok_or_else(|| {
            BifrostError::Config(
                "either <pair_code> or --ssh-key is required for `bifrost remote conn up`"
                    .to_string(),
            )
        })?;
        return handle_connect(
            &caller,
            pair_code,
            &caller_info,
            &caller_identity,
            &opts.relay_url,
        )
        .await;
    }

    let mut connections = load_connections()?;

    if let RemoteCommands::Conn {
        action: RemoteConnCommands::Down { all, grant_id },
    } = &opts.action
    {
        return handle_disconnect(
            &caller,
            &connections,
            &caller_identity,
            opts.client_id.as_deref(),
            *all,
            grant_id.as_deref(),
        )
        .await;
    }

    if let RemoteCommands::Job { action } = &opts.action {
        return handle_remote_job_command(&caller, action, &opts.relay_url).await;
    }

    let mut conn = resolve_local_connection(&connections, opts.client_id.as_deref())?;
    if let RemoteCommands::Exec(exec_args) = &opts.action {
        if let Err(msg) = validate_remote_command_exec_flags(exec_args) {
            return Err(BifrostError::Config(msg));
        }
    }
    let mut command = if matches!(&opts.action, RemoteCommands::Run(_)) {
        None
    } else {
        Some(build_remote_command_checked(&opts.action)?)
    };

    let grant = if !grant_session_needs_refresh(&conn) {
        local_grant_from_saved_session(&conn)
    } else {
        debug!("saved v5 grant session is expired or near expiry; refreshing before open_call");
        None
    };
    let grant = if let Some(local_grant) = grant {
        debug!("using saved v5 grant session");
        local_grant
    } else {
        let grant = caller
            .find_reusable_grant(&conn.client_instance_id, &caller_identity, Some(&conn))
            .await?;

        match grant {
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
                    !(c.client_instance_id == conn.client_instance_id
                        && c.relay_url == conn.relay_url)
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
                    "  {} Run `bifrost remote conn up <pair-code>` to re-authorize.",
                    "→".bright_yellow()
                );
                std::process::exit(1);
            }
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

    debug!("found reusable v5 grant session");
    if should_print_remote_progress_banner(std::io::stdout().is_terminal()) {
        println!(
            "{}",
            format!(
                "✓ Using authorization for {}",
                remote_client_short_id(&conn.client_instance_id)
            )
            .bright_green()
        );
    }

    // PR #5e-3: if the caller passed `--resume-call-id` together with
    // `--resume-relay-token`, skip open_call entirely and dive straight into
    // the streaming subscriber, seeded with offsets (0, 0). Validation of the
    // pair is performed via `validate_streaming_prefs`.
    if let Some(prefs) = command
        .as_ref()
        .and_then(|command| command.streaming_prefs.as_ref())
    {
        if let Err(msg) = validate_streaming_prefs(prefs) {
            return Err(BifrostError::Config(msg));
        }
        if let (Some(resume_id), Some(resume_token)) = (
            prefs.resume_call_id.as_ref(),
            prefs.resume_relay_token.as_ref(),
        ) {
            debug!(call_id = %resume_id, "resume-call-id bypass: skipping open_call and rejoining existing call");
            if should_print_remote_progress_banner(std::io::stdout().is_terminal()) {
                println!(
                    "{}",
                    format!(
                        "→ Rejoining existing remote call (call_id: {})...",
                        &resume_id[..resume_id.len().min(8)]
                    )
                    .dimmed()
                );
            }
            let stream_result = tokio::select! {
                result = run_streaming_dispatch(
                    &caller,
                    resume_id,
                    resume_token,
                    prefs,
                ) => result?,
                _ = wait_for_remote_call_cancel_signal() => {
                    eprintln!("{}", "→ Cancellation requested, notifying remote device...".bright_yellow());
                    let _ = caller
                        .cancel_call(resume_id, resume_token)
                        .await;
                    synthesized_cancelled_result()
                }
            };
            if stream_result.exit_code != 0 {
                std::process::exit(stream_result.exit_code);
            }
            return Ok(());
        }
    }

    let transport = merge_transport_context(&conn, &grant)?;
    let mut remote_run_cleanup: Option<(String, Option<String>)> = None;
    if let RemoteCommands::Run(run_args) = &opts.action {
        let remote_script_path = prepare_remote_run_script(
            &caller,
            &conn,
            &grant,
            &caller_identity,
            &transport,
            run_args,
        )
        .await?;
        if should_print_remote_progress_banner(std::io::stdout().is_terminal()) {
            println!(
                "{}",
                format!("→ Uploaded script to {remote_script_path}").dimmed()
            );
        }
        if run_args.detach {
            eprintln!(
                "  {} Detached `remote run` keeps the uploaded script for inspection: {}",
                "→".bright_yellow(),
                remote_script_path
            );
        } else if !run_args.keep {
            remote_run_cleanup = Some((remote_script_path.clone(), run_args.cwd.clone()));
        }
        command = Some(build_remote_run_shell_exec_command(
            run_args,
            remote_script_path,
        ));
    }
    let command = command.expect("remote command should be built before open_call");
    let active_grant_id = grant.grant_id.clone();
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
    let open_request = OpenCallRequest {
        client_instance_id: conn.client_instance_id.clone(),
        command_summary: command_summary.clone(),
        command_kind: command.kind,
        command_encrypted,
        pty_enabled: command
            .shell_exec
            .as_ref()
            .and_then(|payload| payload.pty.as_ref())
            .map(|pty| pty.enabled),
    };
    let mut grant_session_token = decrypt_grant_session_token(&conn)?;

    let mut call_result = caller
        .open_call(&open_request, &grant_session_token, &caller_identity)
        .await;
    if let Err(err) = &call_result {
        if is_grant_session_token_invalid_error(err) {
            debug!(
                "saved v5 grant session token was rejected; refreshing and retrying open_call once"
            );
            let refreshed = caller
                .find_reusable_grant(&conn.client_instance_id, &caller_identity, Some(&conn))
                .await?;
            if let Some(refreshed) = refreshed {
                let refreshed_grant = reconcile_reusable_grant_for_transport(&mut conn, refreshed)?;
                if let Some(existing) = connections.iter_mut().find(|existing| {
                    existing.client_instance_id == conn.client_instance_id
                        && existing.relay_url == conn.relay_url
                }) {
                    *existing = conn.clone();
                    save_connections(&connections)?;
                }
                grant_session_token = decrypt_grant_session_token(&conn)?;
                call_result = caller
                    .open_call(&open_request, &grant_session_token, &caller_identity)
                    .await;
                if call_result.is_ok() {
                    debug!(
                        grant_id = %refreshed_grant.grant_id,
                        "open_call succeeded after refreshing v5 grant session token"
                    );
                }
            }
        }
    }

    let call_result = match call_result {
        Ok(result) => result,
        Err(err)
            if command.kind == CommandKind::ShellExec && is_grant_scope_mismatch_error(&err) =>
        {
            return Err(shell_scope_upgrade_error(&conn));
        }
        Err(err) if is_stale_remote_grant_error(&err) => {
            match remove_stale_local_connection(&mut connections, &conn) {
                Ok(true) => eprintln!(
                    "  {} Stale connection removed from local cache.",
                    "→".bright_yellow()
                ),
                Ok(false) => {}
                Err(error) => warn!(error = %error, "failed to remove stale connection"),
            }
            return Err(stale_remote_grant_error(&conn));
        }
        Err(err) => return Err(err),
    };

    debug!(call_id = %call_result.call_id, grant_id = %active_grant_id, "call opened, subscribing to events");
    let should_detach = match &opts.action {
        RemoteCommands::Exec(exec_args) => exec_args.detach,
        RemoteCommands::Run(run_args) => run_args.detach,
        _ => false,
    };
    if should_detach {
        let now = now_millis();
        remember_remote_job(RemoteJobRecord {
            call_id: call_result.call_id.clone(),
            relay_token_encrypted: encrypt_local_secret(call_result.relay_token.as_bytes())?,
            relay_url: opts.relay_url.clone(),
            client_instance_id: Some(conn.client_instance_id.clone()),
            device_name: Some(conn.device_name.clone()),
            command_preview: Some(command_summary.command_preview.clone()),
            status: "running".to_string(),
            exit_code: None,
            created_at: now,
            updated_at: now,
        })?;
        println!("Detached remote job started");
        println!("call_id={}", call_result.call_id);
        println!("watch: bifrost remote job watch {}", call_result.call_id);
        return Ok(());
    }
    if should_print_remote_progress_banner(std::io::stdout().is_terminal()) {
        println!("{}", "→ Executing command on remote device...".dimmed());
    }

    let streaming_enabled = command
        .streaming_prefs
        .as_ref()
        .map(|p| p.is_streaming())
        .unwrap_or(false);

    if streaming_enabled {
        let prefs = command
            .streaming_prefs
            .as_ref()
            .expect("streaming_prefs present when streaming_enabled");
        let stdin_forwarder = prefs.stdin.then(|| {
            spawn_remote_stdin_forwarder(
                Arc::new(caller.clone()),
                call_result.call_id.clone(),
                call_result.relay_token.clone(),
                transport.clone(),
                prefs.interactive,
            )
        });
        let stream_result = tokio::select! {
            result = run_streaming_dispatch(
                &caller,
                &call_result.call_id,
                &call_result.relay_token,
                prefs,
            ) => result?,
            _ = wait_for_remote_call_cancel_signal() => {
                eprintln!("{}", "→ Cancellation requested, notifying remote device...".bright_yellow());
                let _ = caller
                    .cancel_call(&call_result.call_id, &call_result.relay_token)
                    .await;
                synthesized_cancelled_result()
            }
        };
        if let Some(handle) = stdin_forwarder {
            handle.abort();
        }
        if let Some((remote_path, cwd)) = remote_run_cleanup.as_ref() {
            cleanup_remote_run_script(
                &caller,
                &conn,
                &grant,
                &caller_identity,
                &transport,
                cwd.clone(),
                remote_path,
            )
            .await;
        }
        if stream_result.exit_code != 0 {
            std::process::exit(stream_result.exit_code);
        }
        return Ok(());
    }

    let stdin_forwarder = command
        .streaming_prefs
        .as_ref()
        .filter(|prefs| prefs.stdin)
        .map(|prefs| {
            spawn_remote_stdin_forwarder(
                Arc::new(caller.clone()),
                call_result.call_id.clone(),
                call_result.relay_token.clone(),
                transport.clone(),
                prefs.interactive,
            )
        });
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
    if let Some(handle) = stdin_forwarder {
        handle.abort();
    }

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
            "  {} Run `bifrost remote conn up <pair-code>` to re-authorize.",
            "→".bright_yellow()
        );
        std::process::exit(1);
    }

    if let Some((remote_path, cwd)) = remote_run_cleanup.as_ref() {
        cleanup_remote_run_script(
            &caller,
            &conn,
            &grant,
            &caller_identity,
            &transport,
            cwd.clone(),
            remote_path,
        )
        .await;
    }

    print_remote_result(&command, &result);

    if result.exit_code != 0 {
        std::process::exit(result.exit_code);
    }

    Ok(())
}

async fn handle_connect(
    caller: &CallerRelayClient,
    pair_code: &str,
    caller_info: &CallerInfo,
    caller_identity: &CallerPopIdentity,
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
            caller_pubkey: pop::caller_pubkey_b64(&caller_identity.key_pair),
            caller_ephemeral_pub: pending_transport.caller_ephemeral_pub.clone(),
        },
    )
    .await?;

    println!(
        "{}",
        "⏳ Waiting for approval on the remote device...".bright_yellow()
    );

    let approval = caller
        .watch_pairing(&pairing_result.pairing_id, &pairing_result.watch_token)
        .await?;

    match approval.status.as_str() {
        "approved" => {
            let claim_token = approval.claim_token.as_deref().ok_or_else(|| {
                BifrostError::Config("pairing approved without claim_token".to_string())
            })?;
            let grant = caller
                .claim_grant(
                    &pairing_result.client_instance_id,
                    pair_code,
                    claim_token,
                    pending_transport,
                    caller_identity,
                )
                .await?;
            let device_name =
                remote_client_short_id(&pairing_result.client_instance_id).to_string();
            let short_id = remote_client_short_id(&pairing_result.client_instance_id);
            let platform = "unknown".to_string();
            let grant_mode = grant
                .grant_summary
                .as_ref()
                .and_then(|summary| summary.mode.clone())
                .unwrap_or_else(|| "unknown".to_string());
            let grant_session_token = grant
                .grant_session_token
                .as_deref()
                .map(encrypt_grant_session_token)
                .transpose()?;

            let new_conn = LocalConnection {
                client_instance_id: pairing_result.client_instance_id.clone(),
                device_name: device_name.clone(),
                platform: platform.clone(),
                relay_url: relay_url.to_string(),
                grant_id: String::new(),
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
                caller_ephemeral_pub: grant.caller_ephemeral_pub,
                client_ephemeral_pub: grant.client_ephemeral_pub,
                shared_secret_encrypted: grant.shared_secret_encrypted,
                grant_session_token,
                grant_session_expires_at: grant.expires_at,
            };

            let mut connections = load_connections().unwrap_or_default();
            upsert_local_connection(&mut connections, new_conn);
            save_connections(&connections)?;

            println!(
                "{}",
                "✓ Connected! Authorization granted."
                    .to_string()
                    .bright_green()
            );
            println!(
                "{}",
                format!("  Device: {device_name} ({platform})").dimmed()
            );
            println!(
                "{}",
                format!(
                    "  You can now run commands like: bifrost remote conn status --client-id {short_id}"
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
    caller_identity: &CallerPopIdentity,
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
            let grant_id = result
                .grant_id
                .clone()
                .unwrap_or_else(|| "unknown".to_string());
            let client_instance_id = result.client_instance_id.clone().ok_or_else(|| {
                BifrostError::Config(
                    "ssh connect succeeded but relay did not return client_instance_id".to_string(),
                )
            })?;
            let grant_mode = result
                .grant_mode
                .clone()
                .unwrap_or_else(|| "once".to_string());
            let caller_fingerprint = result
                .caller_fingerprint
                .clone()
                .unwrap_or_else(|| caller_info.fingerprint.clone());

            // P0-3: prefer single-use claim_token exchanged for a real grant_session_token via PoP.
            let (grant_session_token_plain, grant_session_expires_at, transport) =
                if let Some(claim_token) = result.claim_token.as_deref() {
                    let redeemed = caller
                        .redeem_ssh_claim(
                            &client_instance_id,
                            claim_token,
                            pending_transport,
                            caller_identity,
                        )
                        .await?;
                    let transport_finalized = redeemed
                        .client_ephemeral_pub
                        .as_deref()
                        .and_then(|client_eph| {
                            redeemed.caller_ephemeral_pub.as_deref().map(|caller_eph| {
                                StoredTransportContext {
                                    caller_ephemeral_pub: caller_eph.to_string(),
                                    client_ephemeral_pub: client_eph.to_string(),
                                    shared_secret_encrypted: redeemed
                                        .shared_secret_encrypted
                                        .clone()
                                        .unwrap_or_default(),
                                }
                            })
                        })
                        .ok_or_else(|| {
                            BifrostError::Network(
                                "redeem_ssh_claim response missing transport context".to_string(),
                            )
                        })?;
                    (
                        redeemed.grant_session_token,
                        redeemed.expires_at,
                        transport_finalized,
                    )
                } else {
                    // Legacy path (should not occur after P0-3 server changes).
                    let client_ephemeral_pub =
                        result.client_ephemeral_pub.as_deref().ok_or_else(|| {
                            BifrostError::Config(
                            "ssh connect succeeded but relay did not return client_ephemeral_pub"
                                .to_string(),
                        )
                        })?;
                    let t = pending_transport.finalize(client_ephemeral_pub)?;
                    (
                        result.grant_session_token.clone(),
                        result.grant_session_expires_at.clone(),
                        t,
                    )
                };

            let grant_session_token = grant_session_token_plain
                .as_deref()
                .map(encrypt_grant_session_token)
                .transpose()?;

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
                grant_session_token,
                grant_session_expires_at,
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
                    "  You can now run commands like: bifrost remote conn status --client-id {short_id}"
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
        "pairing service is temporarily busy (relay overload protection triggered). Please wait a few seconds and run `bifrost remote conn up <pair-code>` again.".to_string(),
    )
}

async fn handle_disconnect(
    caller: &CallerRelayClient,
    connections: &[LocalConnection],
    caller_identity: &CallerPopIdentity,
    client_id: Option<&str>,
    all: bool,
    grant_id: Option<&str>,
) -> bifrost_core::Result<()> {
    if let Some(gid) = grant_id {
        let conn = connections
            .iter()
            .find(|conn| conn.grant_id == gid)
            .ok_or_else(|| BifrostError::Config(format!("no saved connection for grant {gid}")))?;
        let outcome = caller.delete_grant(conn, caller_identity).await?;
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
            match revoke_saved_connection(caller, conn, caller_identity).await {
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

    revoke_saved_connection(caller, &conn, caller_identity).await?;

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

async fn revoke_saved_connection(
    caller: &CallerRelayClient,
    conn: &LocalConnection,
    caller_identity: &CallerPopIdentity,
) -> bifrost_core::Result<usize> {
    match caller.delete_grant(conn, caller_identity).await? {
        DeleteGrantOutcome::Deleted | DeleteGrantOutcome::AlreadyMissing => Ok(1),
    }
}

#[cfg(test)]
fn build_remote_command(action: &RemoteCommands) -> BuiltRemoteCommand {
    build_remote_command_checked(action).expect("remote command should build")
}

fn build_remote_command_checked(
    action: &RemoteCommands,
) -> bifrost_core::Result<BuiltRemoteCommand> {
    match action {
        RemoteCommands::Conn {
            action: RemoteConnCommands::Up { .. },
        } => unreachable!("conn up handled separately"),
        RemoteCommands::Conn {
            action: RemoteConnCommands::Down { .. },
        } => unreachable!("conn down handled separately"),
        RemoteCommands::Conn {
            action: RemoteConnCommands::Status,
        } => Ok(BuiltRemoteCommand {
            kind: CommandKind::QueryReadonly,
            label: "status".to_string(),
            command: Some("status".to_string()),
            args_json: None,
            query: None,
            shell_exec: None,
            render: RemoteRenderMode::Raw,
            streaming_prefs: None,
        }),
        RemoteCommands::Job { .. } => Err(BifrostError::Config(
            "`remote job` is handled locally and should not build an encrypted command".to_string(),
        )),
        RemoteCommands::Exec(exec_args) => Ok(build_remote_shell_exec_command(exec_args)),
        RemoteCommands::Run(_) => Err(BifrostError::Config(
            "`remote run` must upload its script before building the final remote exec command"
                .to_string(),
        )),
        RemoteCommands::File { action } => build_remote_file_command(action.as_ref()),
        RemoteCommands::KeepAwake { action } => {
            let (op, args) = crate::commands::keepawake::build_remote_args(action);
            Ok(BuiltRemoteCommand {
                kind: CommandKind::PowerMgmt,
                label: format!("keepawake.{op}"),
                command: Some(op),
                args_json: Some(args.to_string()),
                query: None,
                shell_exec: None,
                render: RemoteRenderMode::Raw,
                streaming_prefs: None,
            })
        }
        RemoteCommands::Traffic {
            action: RemoteTrafficCommands::Search(search_args),
        } => {
            let query = CanonicalQueryCommand::Search(command_search_args(search_args));
            let command_id = query.command_id().to_string();
            Ok(BuiltRemoteCommand {
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
            })
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
                    listener_port: list_args.listener_port,
                    has_rule_hit: list_args.has_rule_hit,
                    is_websocket: list_args.is_websocket,
                    is_sse: list_args.is_sse,
                    is_tunnel: list_args.is_tunnel,
                });
                let command_id = query.command_id().to_string();
                Ok(BuiltRemoteCommand {
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
                })
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
                    ..Default::default()
                });
                let command_id = query.command_id().to_string();
                Ok(BuiltRemoteCommand {
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
                })
            }
            RemoteTrafficCommands::Search(_) => unreachable!("traffic search handled above"),
        },
    }
}

fn build_open_call_command_summary(command: &BuiltRemoteCommand) -> OpenCallCommandSummary {
    OpenCallCommandSummary {
        command_preview: command.label.clone(),
        masked_args_json: command.args_json.clone(),
    }
}

async fn open_and_wait_remote_command(
    caller: &CallerRelayClient,
    conn: &LocalConnection,
    grant: &GrantInfo,
    caller_identity: &CallerPopIdentity,
    transport: &OpenCallTransportContext,
    command: &BuiltRemoteCommand,
    timeout_secs: u64,
) -> bifrost_core::Result<CallResult> {
    let command_summary = build_open_call_command_summary(command);
    let command_encrypted = encrypt_remote_command(
        command.kind,
        command.command.as_deref(),
        command.args_json.as_deref(),
        command.query.as_ref(),
        command.shell_exec.as_ref(),
        &grant.grant_id,
        transport,
    )?;
    let open_request = OpenCallRequest {
        client_instance_id: conn.client_instance_id.clone(),
        command_summary,
        command_kind: command.kind,
        command_encrypted,
        pty_enabled: command
            .shell_exec
            .as_ref()
            .and_then(|payload| payload.pty.as_ref())
            .map(|pty| pty.enabled),
    };
    let grant_session_token = decrypt_grant_session_token(conn)?;
    let mut call_result = caller
        .open_call(&open_request, &grant_session_token, caller_identity)
        .await;
    if let Err(err) = &call_result {
        if is_grant_session_token_invalid_error(err) {
            debug!(
                "saved v5 grant session token was rejected in helper path; refreshing and retrying open_call once"
            );
            let refreshed = caller
                .find_reusable_grant(&conn.client_instance_id, caller_identity, Some(conn))
                .await?;
            if let Some(refreshed) = refreshed {
                let mut refreshed_conn = conn.clone();
                reconcile_reusable_grant_for_transport(&mut refreshed_conn, refreshed)?;
                let refreshed_token = decrypt_grant_session_token(&refreshed_conn)?;
                call_result = caller
                    .open_call(&open_request, &refreshed_token, caller_identity)
                    .await;
            }
        }
    }
    let call_result = call_result?;
    caller
        .subscribe_call_events(
            &call_result.call_id,
            &call_result.relay_token,
            transport,
            &command.render,
            timeout_secs,
        )
        .await
}

async fn run_remote_file_command_or_fail(
    caller: &CallerRelayClient,
    conn: &LocalConnection,
    grant: &GrantInfo,
    caller_identity: &CallerPopIdentity,
    transport: &OpenCallTransportContext,
    command: &BuiltRemoteCommand,
    phase: &str,
) -> bifrost_core::Result<()> {
    let result = open_and_wait_remote_command(
        caller,
        conn,
        grant,
        caller_identity,
        transport,
        command,
        CALL_EVENT_TIMEOUT_SECS,
    )
    .await?;
    if result.exit_code == 0 {
        return Ok(());
    }
    print_remote_result(command, &result);
    Err(BifrostError::Config(format!(
        "remote run failed while {phase}; see remote file error above"
    )))
}

async fn prepare_remote_run_script(
    caller: &CallerRelayClient,
    conn: &LocalConnection,
    grant: &GrantInfo,
    caller_identity: &CallerPopIdentity,
    transport: &OpenCallTransportContext,
    run_args: &RemoteRunArgs,
) -> bifrost_core::Result<String> {
    let script_file = run_args.script_file.as_ref().ok_or_else(|| {
        BifrostError::Config("`remote run --script-file <path>` is required".to_string())
    })?;
    let remote_path = remote_run_script_remote_path(run_args)?;
    let scratch_command = build_remote_file_command(&RemoteFileCommands::ScratchDir {
        cwd: run_args.cwd.clone(),
        name: run_args.scratch_name.clone(),
        output: "json".to_string(),
    })?;
    run_remote_file_command_or_fail(
        caller,
        conn,
        grant,
        caller_identity,
        transport,
        &scratch_command,
        "creating the remote scratch directory",
    )
    .await?;
    let write_command = build_remote_file_command(&RemoteFileCommands::Write {
        path: Some(remote_path.clone()),
        path_flag: None,
        content: None,
        content_file: Some(script_file.to_string_lossy().to_string()),
        content_b64: None,
        base_sha256: None,
        allow_overwrite: Some(false),
        create_parents: true,
        cwd: run_args.cwd.clone(),
        output: "json".to_string(),
    })?;
    run_remote_file_command_or_fail(
        caller,
        conn,
        grant,
        caller_identity,
        transport,
        &write_command,
        "uploading the local script",
    )
    .await?;
    Ok(remote_path)
}

async fn cleanup_remote_run_script(
    caller: &CallerRelayClient,
    conn: &LocalConnection,
    grant: &GrantInfo,
    caller_identity: &CallerPopIdentity,
    transport: &OpenCallTransportContext,
    cwd: Option<String>,
    remote_path: &str,
) {
    let Ok(delete_command) = build_remote_file_command(&RemoteFileCommands::Delete {
        path: remote_path.to_string(),
        recursive: false,
        cwd,
        output: "json".to_string(),
    }) else {
        return;
    };
    match open_and_wait_remote_command(
        caller,
        conn,
        grant,
        caller_identity,
        transport,
        &delete_command,
        CALL_EVENT_TIMEOUT_SECS,
    )
    .await
    {
        Ok(result) if result.exit_code == 0 => {}
        Ok(result) => {
            eprintln!(
                "  {} remote run cleanup did not delete {} (exit_code={})",
                "→".bright_yellow(),
                remote_path,
                result.exit_code
            );
        }
        Err(error) => {
            eprintln!(
                "  {} remote run cleanup did not delete {}: {}",
                "→".bright_yellow(),
                remote_path,
                error
            );
        }
    }
}

/// Phase 6 (review fix): pure helper that encodes the post-exit scheduling
/// contract. Exposed for unit testing so that regression coverage does not
/// have to spin up a full SSE stream.
///
/// Returns `true` if the outer subscribe loop should reset its idle-deadline
/// timer on this incoming chunk, `false` if the current timer (typically the
/// 3-second post-exit grace) must be left untouched.
///
/// The rule is simple: once `exit_received` is true, a late frame or
/// keep-alive MUST NOT push the deadline further out, otherwise a post-exit
/// `stream_frame` cadence tighter than the grace window keeps the loop
/// alive forever.
pub(crate) fn should_reset_idle_deadline_on_chunk(exit_received: bool) -> bool {
    !exit_received
}

/// Phase 6 (review fix): reject semantically conflicting CLI flags that clap
/// cannot express directly. `--resume-call-id` rejoins an existing remote
/// call (bypassing open_call), so a fresh command body (`--shell-text` /
/// trailing `argv`) would be silently ignored, confusing operators. Fail
/// fast at arg-parse time with an actionable message.
pub(crate) fn validate_remote_command_exec_flags(
    exec_args: &RemoteCommandExecArgs,
) -> Result<(), String> {
    if exec_args.resume_call_id.is_some() {
        if exec_args.shell_text.is_some() {
            return Err(
                "--resume-call-id cannot be combined with --shell-text:                  resume rejoins an existing call and ignores any new command"
                    .to_string(),
            );
        }
        if !exec_args.argv.is_empty() {
            return Err(
                "--resume-call-id cannot be combined with a trailing program/argv:                  resume rejoins an existing call and ignores any new command"
                    .to_string(),
            );
        }
    }
    Ok(())
}

fn build_remote_shell_exec_command(exec_args: &RemoteCommandExecArgs) -> BuiltRemoteCommand {
    let interactive = exec_args.interactive;
    let stdin_enabled = exec_args.stdin || interactive;
    let pty_enabled = exec_args.pty || interactive;
    let stdin_mode = stdin_enabled.then(|| "stream".to_string());
    let pty = pty_enabled.then(|| {
        let (cols, rows) = current_terminal_size();
        PtyRequestPayload {
            enabled: true,
            rows: Some(rows),
            cols: Some(cols),
        }
    });
    let output_mode = pty_enabled.then(|| "pty_merged".to_string());
    let shell_exec = if let Some(shell_text) = &exec_args.shell_text {
        ShellExecPayload {
            exec_mode: "shell_text".to_string(),
            argv: None,
            command_text: Some(shell_text.clone()),
            cwd: exec_args.cwd.clone(),
            env: remote_shell_exec_env(&exec_args.env),
            timeout_ms: exec_args.timeout_ms,
            login: exec_args.login,
            stdin_mode: stdin_mode.clone(),
            pty: pty.clone(),
            output_mode: output_mode.clone(),
        }
    } else {
        ShellExecPayload {
            exec_mode: "argv_exec".to_string(),
            argv: Some(exec_args.argv.clone()),
            command_text: None,
            cwd: exec_args.cwd.clone(),
            env: remote_shell_exec_env(&exec_args.env),
            timeout_ms: exec_args.timeout_ms,
            login: false,
            stdin_mode,
            pty,
            output_mode,
        }
    };

    let preview = exec_args
        .shell_text
        .clone()
        .or_else(|| exec_args.argv.first().cloned())
        .unwrap_or_else(|| "shell.exec".to_string());

    let streaming_prefs = StreamingPrefs {
        stream: exec_args.stream || interactive,
        output_file: exec_args.output_file.as_ref().map(std::path::PathBuf::from),
        resume_call_id: exec_args.resume_call_id.clone(),
        resume_relay_token: exec_args.resume_relay_token.clone(),
        no_verify_digest: exec_args.no_verify_digest || interactive,
        interactive,
        stdin: stdin_enabled,
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

fn build_remote_run_shell_exec_command(
    run_args: &RemoteRunArgs,
    remote_script_path: String,
) -> BuiltRemoteCommand {
    let mut argv = vec![run_args.interpreter.clone(), remote_script_path.clone()];
    argv.extend(run_args.args.clone());
    let shell_exec = ShellExecPayload {
        exec_mode: "argv_exec".to_string(),
        argv: Some(argv),
        command_text: None,
        cwd: run_args.cwd.clone(),
        env: remote_shell_exec_env(&run_args.env),
        timeout_ms: run_args.timeout_ms,
        login: false,
        stdin_mode: None,
        pty: None,
        output_mode: None,
    };
    let streaming_prefs = StreamingPrefs {
        stream: run_args.stream,
        output_file: run_args.output_file.as_ref().map(PathBuf::from),
        resume_call_id: None,
        resume_relay_token: None,
        no_verify_digest: run_args.no_verify_digest,
        interactive: false,
        stdin: false,
    };

    BuiltRemoteCommand {
        kind: CommandKind::ShellExec,
        label: "remote.run".to_string(),
        command: Some(format!(
            "run {} {}",
            run_args.interpreter, remote_script_path
        )),
        args_json: None,
        query: None,
        shell_exec: Some(shell_exec),
        render: RemoteRenderMode::Raw,
        streaming_prefs: Some(streaming_prefs),
    }
}

fn sanitize_remote_run_filename(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches('_');
    if trimmed.is_empty() {
        "script".to_string()
    } else {
        trimmed.to_string()
    }
}

fn remote_run_script_remote_path(run_args: &RemoteRunArgs) -> bifrost_core::Result<String> {
    let scratch_name = run_args.scratch_name.trim_matches('/');
    if scratch_name.is_empty() || scratch_name.contains("..") {
        return Err(BifrostError::Config(
            "`remote run --scratch-name` must be a non-empty relative directory name".to_string(),
        ));
    }
    let script_file = run_args.script_file.as_ref().ok_or_else(|| {
        BifrostError::Config("`remote run --script-file <path>` is required".to_string())
    })?;
    let local_name = script_file
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("script");
    let remote_name = run_args
        .remote_name
        .as_deref()
        .map(sanitize_remote_run_filename)
        .unwrap_or_else(|| {
            format!(
                "run-{}-{}",
                now_millis(),
                sanitize_remote_run_filename(local_name)
            )
        });
    Ok(format!("{scratch_name}/{remote_name}"))
}

fn current_terminal_size() -> (u16, u16) {
    crossterm::terminal::size().unwrap_or((80, 24))
}

fn build_remote_file_command(
    action: &RemoteFileCommands,
) -> bifrost_core::Result<BuiltRemoteCommand> {
    use serde_json::json;
    let (file_op, label, args_value, output) = match action {
        RemoteFileCommands::Read {
            path,
            max_bytes,
            allow_binary,
            offset,
            limit,
            cwd,
            output,
        } => (
            "file.read",
            format!("file.read {}", path),
            json!({
                "path": path,
                "max_bytes": max_bytes,
                "allow_binary": allow_binary,
                "offset": offset,
                "limit": limit,
                "cwd": cwd,
            }),
            output.clone(),
        ),
        RemoteFileCommands::ReadMany {
            paths,
            max_bytes,
            allow_binary,
            cwd,
            output,
        } => (
            "file.read_many",
            format!("file.read_many ({} files)", paths.len()),
            json!({
                "paths": paths,
                "max_bytes": max_bytes,
                "allow_binary": allow_binary,
                "cwd": cwd,
            }),
            output.clone(),
        ),
        RemoteFileCommands::ScratchDir { cwd, name, output } => (
            "file.mkdir",
            format!("file.scratch_dir {}", name),
            json!({
                "path": name,
                "parents": true,
                "cwd": cwd,
            }),
            output.clone(),
        ),
        RemoteFileCommands::List {
            path,
            depth,
            max_matches,
            cursor,
            no_ignore,
            exclude_patterns,
            cwd,
            output,
        } => (
            "file.list",
            format!("file.list {}", path.clone().unwrap_or_else(|| ".".into())),
            json!({
                "path": path,
                "depth": depth,
                "max_matches": max_matches,
                "cursor": cursor,
                "respect_gitignore": !no_ignore,
                "exclude_patterns": if exclude_patterns.is_empty() { None } else { Some(exclude_patterns) },
                "cwd": cwd,
            }),
            output.clone(),
        ),
        RemoteFileCommands::Stat { path, cwd, output } => (
            "file.stat",
            format!("file.stat {}", path),
            json!({ "path": path, "cwd": cwd }),
            output.clone(),
        ),
        RemoteFileCommands::Glob {
            pattern,
            max_matches,
            cursor,
            no_ignore,
            exclude_patterns,
            cwd,
            output,
        } => (
            "file.glob",
            format!("file.glob {}", pattern),
            json!({
                "pattern": pattern,
                "max_matches": max_matches,
                "cursor": cursor,
                "respect_gitignore": !no_ignore,
                "exclude_patterns": if exclude_patterns.is_empty() { None } else { Some(exclude_patterns) },
                "cwd": cwd,
            }),
            output.clone(),
        ),
        RemoteFileCommands::Find {
            pattern,
            regex,
            path,
            max_matches,
            max_scan,
            cursor,
            context_before,
            context_after,
            around,
            case_insensitive,
            fixed_strings,
            word,
            glob,
            no_ignore,
            exclude_patterns,
            cwd,
            output,
        } => {
            // P1-3: gather positional `pattern` plus every repeated `--regex`
            // into one OR-list. `patterns` is the canonical wire field; the
            // legacy single `pattern` is kept populated for back-compat with
            // older servers.
            let mut patterns: Vec<String> = Vec::new();
            if let Some(p) = pattern.clone() {
                patterns.push(p);
            }
            patterns.extend(regex.clone());
            if patterns.is_empty() {
                return Err(BifrostError::Config(
                    "[file.invalid_args] file find requires a positional <PATTERN> or at least one --regex".to_string(),
                ));
            }
            let label = format!("file.search {}", patterns.join("|"));
            (
                "file.search",
                label,
                json!({
                    // Back-compat: first pattern also sent as the scalar field.
                    "pattern": patterns.first().cloned(),
                    "patterns": patterns,
                    "path": path,
                    "max_matches": max_matches,
                    "max_scan_bytes": max_scan,
                    "cursor": cursor,
                    "case_insensitive": case_insensitive,
                    "fixed_strings": fixed_strings,
                    "word": word,
                    "glob": glob,
                    "respect_gitignore": !no_ignore,
                    "exclude_patterns": if exclude_patterns.is_empty() { None } else { Some(exclude_patterns) },
                    "context_before": context_before,
                    "context_after": context_after,
                    "around": around,
                    "cwd": cwd,
                }),
                output.clone(),
            )
        }
        RemoteFileCommands::Hash {
            path,
            algo,
            cwd,
            output,
        } => (
            "file.hash",
            format!("file.hash {}", path),
            json!({ "path": path, "algo": algo, "cwd": cwd }),
            output.clone(),
        ),
        RemoteFileCommands::Outline {
            path,
            max_symbols,
            max_bytes,
            cwd,
            output,
        } => (
            "file.outline",
            format!("file.outline {}", path),
            json!({
                "path": path,
                "max_symbols": max_symbols,
                "max_bytes": max_bytes,
                "cwd": cwd,
            }),
            output.clone(),
        ),
        RemoteFileCommands::Write {
            path,
            path_flag,
            content: cli_content,
            content_file,
            content_b64: cli_content_b64,
            base_sha256,
            allow_overwrite,
            create_parents,
            cwd,
            output,
        } => {
            let path = resolve_file_write_path(path, path_flag)?;
            let content_b64 = if let Some(b64) = cli_content_b64.as_deref() {
                b64.to_string()
            } else if let Some(text) = cli_content.as_deref() {
                base64::engine::general_purpose::STANDARD.encode(text.as_bytes())
            } else {
                match content_file.as_deref() {
                    Some("-") => {
                        use std::io::Read;
                        let mut buf = Vec::new();
                        std::io::stdin().read_to_end(&mut buf).map_err(|e| {
                            BifrostError::Io(std::io::Error::other(format!(
                                "read stdin for file.write: {e}"
                            )))
                        })?;
                        base64::engine::general_purpose::STANDARD.encode(&buf)
                    }
                    None => {
                        return Err(BifrostError::Config(
                            "missing content source for file.write: use --content, --from-local/--content-file, --content-b64, or --from-local - to read stdin".to_string(),
                        ));
                    }
                    Some(p) => std::fs::read(p)
                        .map(|b| base64::engine::general_purpose::STANDARD.encode(&b))
                        .map_err(|e| {
                            BifrostError::Io(std::io::Error::other(format!(
                                "read content file {p}: {e}"
                            )))
                        })?,
                }
            };
            (
                "file.write",
                format!("file.write {}", path),
                json!({
                    "path": path,
                    "content_b64": content_b64,
                    "base_sha256": base_sha256,
                    "allow_overwrite": allow_overwrite,
                    "create_parents": create_parents,
                    "cwd": cwd,
                }),
                output.clone(),
            )
        }
        RemoteFileCommands::Edit {
            path,
            edits,
            edits_file,
            base_sha256,
            cwd,
            output,
        } => {
            let edits_text = read_remote_file_edit_source(edits, edits_file)?;
            let edits_val: serde_json::Value = serde_json::from_str(&edits_text).map_err(|e| {
                BifrostError::Config(format!(
                    "invalid --edits JSON: {e}. Expected a JSON array like \
                     '[{{\"start_line\":10,\"end_line\":12,\"replacement\":\"new text\\n\"}}]' \
                     (1-based inclusive line numbers)."
                ))
            })?;
            if !edits_val.is_array() {
                return Err(BifrostError::Config(
                    "invalid --edits: expected a JSON array of \
                     {start_line,end_line,replacement} objects."
                        .to_string(),
                ));
            }
            (
                "file.edit",
                format!("file.edit {}", path),
                json!({
                    "path": path,
                    "edits": edits_val,
                    "base_sha256": base_sha256,
                    "cwd": cwd,
                }),
                output.clone(),
            )
        }
        RemoteFileCommands::Mkdir {
            path,
            parents,
            cwd,
            output,
        } => (
            "file.mkdir",
            format!("file.mkdir {}", path),
            json!({ "path": path, "parents": parents, "cwd": cwd }),
            output.clone(),
        ),
        RemoteFileCommands::Move {
            from,
            to,
            base_sha256,
            allow_overwrite,
            cwd,
            output,
        } => (
            "file.move",
            format!("file.move {} -> {}", from, to),
            json!({
                "path": from,
                "to_path": to,
                "base_sha256": base_sha256,
                "allow_overwrite": allow_overwrite,
                "cwd": cwd,
            }),
            output.clone(),
        ),
        RemoteFileCommands::Delete {
            path,
            recursive,
            cwd,
            output,
        } => (
            "file.delete",
            format!("file.delete {}", path),
            json!({ "path": path, "recursive": recursive, "cwd": cwd }),
            output.clone(),
        ),
        RemoteFileCommands::Patch {
            patch_file,
            patch_b64,
            base_sha,
            cwd,
            output,
        } => {
            let patch_text = if let Some(b64) = patch_b64.as_deref() {
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(b64.trim())
                    .map_err(|e| {
                        BifrostError::Io(std::io::Error::other(format!("decode --patch-b64: {e}")))
                    })?;
                String::from_utf8(decoded).map_err(|e| {
                    BifrostError::Io(std::io::Error::other(format!(
                        "patch-b64 is not valid utf-8: {e}"
                    )))
                })?
            } else if let Some(pf) = patch_file.as_deref() {
                if pf == "-" {
                    use std::io::Read;
                    let mut buf = String::new();
                    std::io::stdin().read_to_string(&mut buf).map_err(|e| {
                        BifrostError::Io(std::io::Error::other(format!(
                            "read stdin for file.apply-patch: {e}"
                        )))
                    })?;
                    buf
                } else {
                    std::fs::read_to_string(pf).map_err(|e| {
                        BifrostError::Io(std::io::Error::other(format!(
                            "read patch file {pf}: {e}"
                        )))
                    })?
                }
            } else {
                return Err(BifrostError::Config(
                    "apply-patch requires --from-local/--patch-file or --patch-b64".to_string(),
                ));
            };
            // Parse `--base-sha PATH=SHA` pairs into a map for the server-side
            // per-file optimistic lock. An empty SHA asserts the target does
            // not yet exist (creation guard).
            let mut base_shas = serde_json::Map::new();
            for pair in base_sha {
                let (p, sha) = pair.split_once('=').ok_or_else(|| {
                    BifrostError::Config(format!(
                        "invalid --base-sha '{pair}': expected PATH=SHA256 (SHA may be empty)"
                    ))
                })?;
                base_shas.insert(p.to_string(), Value::String(sha.to_string()));
            }
            let base_shas_val = if base_shas.is_empty() {
                Value::Null
            } else {
                Value::Object(base_shas)
            };
            (
                "file.apply_patch",
                format!("file.apply_patch ({} bytes)", patch_text.len()),
                json!({ "patch_text": patch_text, "base_shas": base_shas_val, "cwd": cwd }),
                output.clone(),
            )
        }
    };

    let render_op = match action {
        RemoteFileCommands::Read { .. } => RemoteFileOp::Read,
        RemoteFileCommands::ReadMany { .. } => RemoteFileOp::ReadMany,
        RemoteFileCommands::List { .. } => RemoteFileOp::List,
        RemoteFileCommands::Stat { .. } => RemoteFileOp::Stat,
        RemoteFileCommands::Glob { .. } => RemoteFileOp::Glob,
        RemoteFileCommands::Find { .. } => RemoteFileOp::Find,
        RemoteFileCommands::Hash { .. } => RemoteFileOp::Hash,
        RemoteFileCommands::Outline { .. } => RemoteFileOp::Outline,
        RemoteFileCommands::Write { .. } => RemoteFileOp::Write,
        RemoteFileCommands::Edit { .. } => RemoteFileOp::Edit,
        RemoteFileCommands::ScratchDir { .. } => RemoteFileOp::Mkdir,
        RemoteFileCommands::Mkdir { .. } => RemoteFileOp::Mkdir,
        RemoteFileCommands::Move { .. } => RemoteFileOp::Move,
        RemoteFileCommands::Delete { .. } => RemoteFileOp::Delete,
        RemoteFileCommands::Patch { .. } => RemoteFileOp::Patch,
    };
    let render_json = output.eq_ignore_ascii_case("json");

    Ok(BuiltRemoteCommand {
        kind: CommandKind::File,
        label,
        command: Some(file_op.to_string()),
        args_json: Some(args_value.to_string()),
        query: None,
        shell_exec: None,
        render: RemoteRenderMode::File {
            op: render_op,
            json: render_json,
        },
        streaming_prefs: None,
    })
}

async fn handle_remote_job_command(
    caller: &CallerRelayClient,
    action: &RemoteJobCommands,
    default_relay_url: &str,
) -> bifrost_core::Result<()> {
    match action {
        RemoteJobCommands::List => {
            let jobs = load_remote_jobs()?;
            render_remote_job_list(&jobs);
            Ok(())
        }
        RemoteJobCommands::Status { call_id, wait_ms } => {
            let (resolved_token, relay_url) = resolve_remote_job(call_id)?;
            let job_caller;
            let caller = if !relay_url.is_empty() && relay_url != default_relay_url {
                job_caller = CallerRelayClient::new(&relay_url);
                &job_caller
            } else {
                caller
            };
            match caller
                .poll_call_terminal_status(
                    call_id,
                    &resolved_token,
                    Duration::from_millis(*wait_ms),
                )
                .await?
            {
                Some(status) => {
                    println!("status={}", status.status);
                    if let Some(exit_code) = status.exit_code {
                        println!("exit_code={exit_code}");
                    }
                    if let Some(duration_ms) = status.duration_ms {
                        println!("duration_ms={duration_ms}");
                    }
                    update_remote_job_status(call_id, &status.status, status.exit_code)?;
                }
                None => {
                    println!("status=running");
                    println!("call_id={call_id}");
                    update_remote_job_status(call_id, "running", None)?;
                }
            }
            Ok(())
        }
        RemoteJobCommands::Logs {
            call_id,
            output_file,
            no_verify_digest,
        }
        | RemoteJobCommands::Watch {
            call_id,
            output_file,
            no_verify_digest,
        } => {
            let (resolved_token, relay_url) = resolve_remote_job(call_id)?;
            let job_caller;
            let caller = if !relay_url.is_empty() && relay_url != default_relay_url {
                job_caller = CallerRelayClient::new(&relay_url);
                &job_caller
            } else {
                caller
            };
            let prefs = StreamingPrefs {
                stream: true,
                output_file: output_file.as_ref().map(PathBuf::from),
                resume_call_id: Some(call_id.clone()),
                resume_relay_token: Some(resolved_token.clone()),
                no_verify_digest: *no_verify_digest,
                interactive: false,
                stdin: false,
            };
            let result = run_streaming_dispatch(caller, call_id, &resolved_token, &prefs).await?;
            if matches!(action, RemoteJobCommands::Watch { .. }) {
                update_remote_job_status(call_id, "exited", Some(result.exit_code))?;
                if result.exit_code != 0 {
                    std::process::exit(result.exit_code);
                }
            }
            Ok(())
        }
    }
}

fn remote_shell_exec_env(env_pairs: &[(String, String)]) -> Option<BTreeMap<String, String>> {
    if env_pairs.is_empty() {
        return None;
    }

    Some(env_pairs.iter().cloned().collect())
}

fn read_remote_file_edit_source(
    edits: &Option<String>,
    edits_file: &Option<String>,
) -> Result<String, BifrostError> {
    match (edits.as_deref(), edits_file.as_deref()) {
        (Some(text), None) => Ok(text.to_string()),
        (None, Some("-")) => {
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf).map_err(|e| {
                BifrostError::Io(std::io::Error::other(format!(
                    "read stdin for file.edit --from-local: {e}"
                )))
            })?;
            Ok(buf)
        }
        (None, Some(path)) => std::fs::read_to_string(path).map_err(|e| {
            BifrostError::Io(std::io::Error::other(format!(
                "read edits file {path}: {e}"
            )))
        }),
        (Some(_), Some(_)) => Err(BifrostError::Config(
            "file.edit accepts either --edits or --from-local, not both".to_string(),
        )),
        (None, None) => Err(BifrostError::Config(
            "file.edit requires --edits JSON or --from-local <local-json-file>".to_string(),
        )),
    }
}

fn resolve_file_write_path(
    positional: &Option<String>,
    path_flag: &Option<String>,
) -> bifrost_core::Result<String> {
    match (positional.as_ref(), path_flag.as_ref()) {
        (Some(path), None) | (None, Some(path)) => Ok(path.clone()),
        (Some(_), Some(_)) => Err(BifrostError::Config(
            "[file.invalid_args] pass the target path only once: use `bifrost remote file write <path> ...` or the compatibility form `--path <path>`, not both".to_string(),
        )),
        (None, None) => Err(BifrostError::Config(
            "[file.invalid_args] missing target path for file write\n→ Use: bifrost remote file write <path> --from-local <local-file>".to_string(),
        )),
    }
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
    if let Some(listener_port) = args.listener_port {
        filters.conditions.push(FilterCondition {
            field: "listener_port".to_string(),
            operator: "equals".to_string(),
            value: listener_port.to_string(),
        });
    }

    for entry in &args.req_json {
        if let Some((path, value)) = entry.split_once('=') {
            let p = path.trim();
            if p.is_empty() {
                continue;
            }
            let field = if p.starts_with('$') {
                format!(
                    "req.body.{}",
                    p.trim_start_matches('$').trim_start_matches('.')
                )
            } else {
                format!("req.body.{}", p)
            };
            filters.conditions.push(FilterCondition {
                field,
                operator: "equals".to_string(),
                value: value.trim().to_string(),
            });
        }
    }
    for entry in &args.res_json {
        if let Some((path, value)) = entry.split_once('=') {
            let p = path.trim();
            if p.is_empty() {
                continue;
            }
            let field = if p.starts_with('$') {
                format!(
                    "res.body.{}",
                    p.trim_start_matches('$').trim_start_matches('.')
                )
            } else {
                format!("res.body.{}", p)
            };
            filters.conditions.push(FilterCondition {
                field,
                operator: "equals".to_string(),
                value: value.trim().to_string(),
            });
        }
    }
    for entry in &args.req_header_eq {
        if let Some((name, value)) = entry.split_once('=') {
            let n = name.trim();
            if n.is_empty() {
                continue;
            }
            filters.conditions.push(FilterCondition {
                field: format!("req.header.{}", n),
                operator: "equals".to_string(),
                value: value.trim().to_string(),
            });
        }
    }
    for entry in &args.res_header_eq {
        if let Some((name, value)) = entry.split_once('=') {
            let n = name.trim();
            if n.is_empty() {
                continue;
            }
            filters.conditions.push(FilterCondition {
                field: format!("res.header.{}", n),
                operator: "equals".to_string(),
                value: value.trim().to_string(),
            });
        }
    }

    let now_ms = chrono::Utc::now().timestamp_millis();
    let mut time_range: Option<bifrost_command::TimeRange> = None;
    let parse_arg = |s: &str| -> Option<i64> {
        let t = s.trim();
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(t) {
            return Some(dt.timestamp_millis());
        }
        if let Ok(n) = t.parse::<i64>() {
            return Some(n);
        }
        crate::commands::search::parse_duration_ms(t).map(|ms| now_ms - ms)
    };
    if let Some(s) = args.latest.as_deref() {
        if let Some(ms) = crate::commands::search::parse_duration_ms(s) {
            let tr = time_range.get_or_insert_with(bifrost_command::TimeRange::default);
            tr.since_ms = Some(now_ms - ms);
        }
    }
    if let Some(s) = args.since.as_deref() {
        if let Some(v) = parse_arg(s) {
            let tr = time_range.get_or_insert_with(bifrost_command::TimeRange::default);
            tr.since_ms = Some(v);
        }
    }
    if let Some(s) = args.until.as_deref() {
        if let Some(v) = parse_arg(s) {
            let tr = time_range.get_or_insert_with(bifrost_command::TimeRange::default);
            tr.until_ms = Some(v);
        }
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
        time_range,
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
                OutputFormat::Ndjson => {
                    let line = serde_json::json!({
                        "type": "result",
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
                    });
                    println!("{}", line);
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
                    OutputFormat::Ndjson => {
                        let line = serde_json::json!({
                            "type": "error",
                            "error": error.message,
                        });
                        println!("{}", line);
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
            OutputFormat::Ndjson => {
                let line = serde_json::json!({
                    "type": "done",
                    "total_matched": self.total_matched,
                    "total_searched": self.total_searched,
                    "has_more": self.has_more,
                });
                println!("{}", line);
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

struct RawModeGuard {
    enabled: bool,
}

impl RawModeGuard {
    fn enable(enabled: bool) -> Self {
        if enabled {
            let _ = crossterm::terminal::enable_raw_mode();
        }
        Self { enabled }
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if self.enabled {
            let _ = crossterm::terminal::disable_raw_mode();
        }
    }
}

struct RemoteStdinForwarder {
    handle: tokio::task::JoinHandle<()>,
    _raw_mode: RawModeGuard,
}

impl RemoteStdinForwarder {
    fn abort(self) {
        self.handle.abort();
    }
}

fn spawn_remote_stdin_forwarder(
    caller: Arc<CallerRelayClient>,
    call_id: String,
    relay_token: String,
    transport: OpenCallTransportContext,
    raw_mode: bool,
) -> RemoteStdinForwarder {
    let raw_mode_guard = RawModeGuard::enable(raw_mode);
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
    std::thread::Builder::new()
        .name("bifrost-remote-stdin-reader".to_string())
        .spawn(move || {
            let mut stdin = std::io::stdin();
            let mut buf = [0_u8; 4096];
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.blocking_send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        })
        .ok();

    let handle = tokio::spawn(async move {
        let mut seq = 1_u64;
        while let Some(bytes) = rx.recv().await {
            match encrypt_caller_input_frame(&transport, &call_id, seq, &bytes) {
                Ok(envelope_json) => {
                    if let Err(error) = caller
                        .post_call_input(&call_id, &relay_token, envelope_json)
                        .await
                    {
                        warn!(error = %error, call_id = %call_id, "failed to forward local stdin to remote call");
                        break;
                    }
                }
                Err(error) => {
                    warn!(error = %error, call_id = %call_id, "failed to encrypt remote stdin frame");
                    break;
                }
            }
            seq = seq.saturating_add(1);
        }
    });
    RemoteStdinForwarder {
        handle,
        _raw_mode: raw_mode_guard,
    }
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
                RemoteRenderMode::File { op, json } => {
                    if *json {
                        print!("{stdout}");
                        if !stdout.ends_with('\n') {
                            println!();
                        }
                    } else {
                        render_remote_file_human(*op, stdout);
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
            // For file ops, surface an actionable remediation hint based on the
            // structured `[file.xxx]` error code the server embeds in stderr.
            if matches!(command.render, RemoteRenderMode::File { .. }) {
                if let Some(hint) = remote_file_error_hint(stderr) {
                    eprintln!("  {} {}", "→".bright_yellow(), hint.bright_yellow());
                }
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

/// Map a structured `[file.xxx]` error code (embedded by the server in stderr)
/// to a one-line, actionable remediation hint for coding agents.
fn remote_file_error_hint(stderr: &str) -> Option<&'static str> {
    let code = stderr
        .split_once('[')
        .and_then(|(_, rest)| rest.split_once(']'))
        .map(|(code, _)| code.trim())?;
    let hint = match code {
        "file.out_of_scope" => {
            "Path is outside the granted roots. If this was /tmp, macOS may resolve it to \
             /private/tmp outside the policy; use `remote file scratch-dir` under the repo/cwd \
             or ask the target to add the path to FileAccessPolicy."
        }
        "file.op_not_permitted" => {
            "The active file policy does not allow this operation. For `read-many`, fall back to \
             repeated `remote file read` calls or ask the target to add the read_many op."
        }
        "file.anchor_not_found" => {
            "Anchor text not in file — likely changed between your read and write. \
             Re-`read` the file and re-anchor to the latest content."
        }
        "file.anchor_not_unique" => {
            "Anchor matched multiple times. Pass `expected_count` matching the actual \
             count, or use a more specific old_string."
        }
        "file.deny_pattern" => {
            "Path matched a policy `denies` rule (e.g. .ssh, .env, target/, *.pfx). The owner \
             deliberately excluded it; pick a different path — re-authorizing will NOT unblock it."
        }
        "file.permission_denied" => {
            "Write blocked because the grant is read-only (or recursive delete is disabled). \
             Ask the target to re-authorize with read-write File Access."
        }
        "file.already_exists" => {
            "Target already exists. For `mkdir`, drop it or add --parents (idempotent on existing \
             dirs); for `move`, pass --overwrite or choose a new destination."
        }
        "file.cross_device" => {
            "Source and destination are on different filesystems, so an atomic rename is impossible. \
             Use `file read` + `file write` to copy across the boundary instead of `move`."
        }
        "file.binary_not_allowed" => {
            "File looks binary. Re-run `read` with --allow-binary, or use `file hash` + chunked reads."
        }
        "file.sha_mismatch" => {
            "Optimistic-lock failed: the file changed since you read it. Re-run `read`, take the \
             fresh sha256/file_sha256, and retry with --base-sha256."
        }
        "file.not_found" => {
            "Path does not exist. Use `file write --create-parents` or `file mkdir --parents` first."
        }
        "file.is_a_directory" => "Target is a directory; switch to `file list`/`file delete --recursive`.",
        "file.not_a_directory" => "A path component is a file, not a directory; check the path.",
        "file.invalid_args" => {
            "Invalid arguments. For `edit`, line numbers are 1-based inclusive and must be within \
             the file; re-`read` to get current line numbers."
        }
        "file.invalid_regex" => "The search regex is invalid; check the pattern syntax.",
        "file.invalid_glob" => "The glob pattern is invalid; e.g. use '*.rs' or 'src/**/*.ts'.",
        "file.precondition_failed" => {
            "Patch context did not match the current file. Re-`read` the target and regenerate \
             the unified diff against the latest content."
        }
        _ => return None,
    };
    Some(hint)
}

/// Render a `file.*` JSON response into compact, human-friendly text for
/// terminals and coding agents. Falls back to printing the raw JSON if the
/// payload cannot be parsed, so no information is ever lost.
fn render_remote_file_human(op: RemoteFileOp, stdout: &str) {
    let trimmed = stdout.trim();
    let value: Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(_) => {
            print!("{stdout}");
            if !stdout.ends_with('\n') {
                println!();
            }
            return;
        }
    };

    match op {
        RemoteFileOp::Read => render_file_read_human(&value),
        RemoteFileOp::ReadMany => render_file_read_many_human(&value),
        RemoteFileOp::List => render_file_list_human(&value),
        RemoteFileOp::Stat => render_file_stat_human(&value),
        RemoteFileOp::Glob => render_file_glob_human(&value),
        RemoteFileOp::Find => render_file_find_human(&value),
        RemoteFileOp::Hash => render_file_hash_human(&value),
        RemoteFileOp::Outline => render_file_outline_human(&value),
        RemoteFileOp::Write | RemoteFileOp::Edit => render_file_write_human(op, &value),
        RemoteFileOp::Mkdir => render_file_mkdir_human(&value),
        RemoteFileOp::Move => render_file_move_human(&value),
        RemoteFileOp::Delete => render_file_delete_human(&value),
        RemoteFileOp::Patch => render_file_patch_human(&value),
    }
}

fn truncation_notice(op: RemoteFileOp) -> &'static str {
    match op {
        RemoteFileOp::Read => {
            "output truncated — fetch more with --offset/--limit or raise --max-bytes"
        }
        RemoteFileOp::List => "listing truncated — narrow the path or reduce --depth",
        RemoteFileOp::Glob => "results truncated — tighten the pattern or raise --max-matches",
        RemoteFileOp::Find => {
            "results truncated — tighten the regex/--glob/--path or raise --max-matches"
        }
        _ => "result truncated",
    }
}

fn print_truncation_hint(op: RemoteFileOp, value: &Value) {
    if value.get("truncated").and_then(Value::as_bool) == Some(true) {
        eprintln!(
            "  {} {}",
            "⚠".bright_yellow(),
            truncation_notice(op).bright_yellow()
        );
        // P1: when the server returned a resumable cursor, surface the exact
        // flag to fetch the next page rather than leaving the agent guessing.
        if let Some(nc) = value.get("next_cursor").and_then(Value::as_u64) {
            eprintln!(
                "  {} {}",
                "→".bright_cyan(),
                format!("more results available — re-run with --cursor {nc}").bright_cyan()
            );
        }
    }
}

fn render_file_read_human(value: &Value) {
    let decoded = value
        .get("content_b64")
        .and_then(Value::as_str)
        .and_then(|b64| base64::engine::general_purpose::STANDARD.decode(b64).ok());

    match decoded {
        Some(bytes) => match String::from_utf8(bytes) {
            Ok(text) => {
                // Print content verbatim so it round-trips and pipes cleanly.
                print!("{text}");
                if !text.ends_with('\n') {
                    println!();
                }
            }
            Err(e) => {
                let len = e.as_bytes().len();
                eprintln!(
                    "{}",
                    format!(
                        "[binary content, {len} bytes — use --output json + decode content_b64, \
                         or `file hash`]"
                    )
                    .dimmed()
                );
            }
        },
        None => {
            print!("{value}");
            println!();
            return;
        }
    }

    // Footer with sha256 (for optimistic-lock edits) and size, on stderr so it
    // does not pollute piped file content on stdout.
    let sha = value
        .get("file_sha256")
        .and_then(Value::as_str)
        .or_else(|| value.get("sha256").and_then(Value::as_str));
    let total_lines = value.get("total_lines").and_then(Value::as_u64);
    let total_size = value.get("total_size").and_then(Value::as_u64);
    let mtime = value.get("mtime_unix").and_then(Value::as_u64);
    let mut parts: Vec<String> = Vec::new();
    if let Some(n) = total_lines {
        parts.push(format!("{n} lines"));
    }
    if let Some(n) = total_size {
        parts.push(format!("{n} bytes"));
    }
    if let Some(s) = sha {
        parts.push(format!("sha256={s}"));
    }
    if let Some(ts) = mtime {
        parts.push(format!("mtime_unix={ts}"));
    }
    if !parts.is_empty() {
        eprintln!("{}", format!("— {}", parts.join("  ")).dimmed());
    }
    // P1: surface mtime as RFC3339 on a dedicated stderr line so concurrent
    // editors can spot "this file was modified N seconds ago" without diffing
    // the structured JSON output.
    if let Some(mtime) = value.get("mtime_unix").and_then(Value::as_i64) {
        if let Some(rfc) = format_mtime_rfc3339(mtime) {
            eprintln!("{}", format!("— mtime={rfc}").dimmed());
        }
    }
    print_truncation_hint(RemoteFileOp::Read, value);
}

/// Format a Unix timestamp (seconds) as an RFC3339 UTC string. Returns `None`
/// if the timestamp is out of `chrono`'s representable range.
fn format_mtime_rfc3339(mtime_unix: i64) -> Option<String> {
    chrono::DateTime::<chrono::Utc>::from_timestamp(mtime_unix, 0)
        .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

fn render_file_read_many_human(value: &Value) {
    let files = match value.get("files").and_then(Value::as_array) {
        Some(f) => f,
        None => {
            println!("{value}");
            return;
        }
    };
    for f in files {
        let path = f.get("path").and_then(Value::as_str).unwrap_or("?");
        let ok = f.get("ok").and_then(Value::as_bool).unwrap_or(false);
        if !ok {
            let err = f
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("read failed");
            eprintln!("{}", format!("✗ {path}: {err}").red());
            continue;
        }
        let size = f.get("total_size").and_then(Value::as_u64).unwrap_or(0);
        let sha = f
            .get("file_sha256")
            .and_then(Value::as_str)
            .or_else(|| f.get("sha256").and_then(Value::as_str))
            .unwrap_or("");
        println!(
            "{}",
            format!("✓ {path}  ({size} bytes  sha256={sha})").green()
        );
    }
    let ok_count = value.get("ok_count").and_then(Value::as_u64).unwrap_or(0);
    let count = value.get("count").and_then(Value::as_u64).unwrap_or(0);
    eprintln!("{}", format!("— {ok_count}/{count} files read").dimmed());
}

fn render_file_list_human(value: &Value) {
    if let Some(entries) = value.get("entries").and_then(Value::as_array) {
        for e in entries {
            let kind = e.get("type").and_then(Value::as_str).unwrap_or("?");
            let path = e
                .get("path")
                .and_then(Value::as_str)
                .or_else(|| e.get("name").and_then(Value::as_str))
                .unwrap_or("");
            let marker = match kind {
                "dir" => "/",
                "symlink" => "@",
                _ => "",
            };
            if let Some(tgt) = e.get("symlink_target").and_then(Value::as_str) {
                println!("{path}{marker} -> {tgt}");
            } else {
                println!("{path}{marker}");
            }
        }
        eprintln!("{}", format!("— {} entries", entries.len()).dimmed());
    } else {
        print!("{value}");
        println!();
    }
    print_truncation_hint(RemoteFileOp::List, value);
}

fn render_file_stat_human(value: &Value) {
    let kind = value.get("kind").and_then(Value::as_str).unwrap_or("?");
    let size = value.get("size").and_then(Value::as_u64).unwrap_or(0);
    println!("type: {kind}");
    println!("size: {size} bytes");
    if let Some(mode) = value.get("mode").and_then(Value::as_u64) {
        println!("mode: {:o}", mode);
    }
    if let Some(mtime) = value.get("mtime_unix").and_then(Value::as_u64) {
        println!("mtime_unix: {mtime}");
    }
    if let Some(tgt) = value.get("symlink_target").and_then(Value::as_str) {
        println!("symlink_target: {tgt}");
    }
    if let Some(sha) = value.get("sha256").and_then(Value::as_str) {
        println!("sha256: {sha}");
    }
}

fn render_file_glob_human(value: &Value) {
    if let Some(matches) = value.get("matches").and_then(Value::as_array) {
        for m in matches {
            if let Some(p) = m.as_str() {
                println!("{p}");
            }
        }
        eprintln!("{}", format!("— {} matches", matches.len()).dimmed());
    } else {
        print!("{value}");
        println!();
    }
    print_truncation_hint(RemoteFileOp::Glob, value);
}

fn render_file_find_human(value: &Value) {
    if let Some(matches) = value.get("matches").and_then(Value::as_array) {
        for m in matches {
            let path = m.get("path").and_then(Value::as_str).unwrap_or("");
            let line = m.get("line").and_then(Value::as_u64).unwrap_or(0);
            let col = m.get("column").and_then(Value::as_u64).unwrap_or(0);
            let preview = m.get("preview").and_then(Value::as_str).unwrap_or("");
            println!("{path}:{line}:{col}: {preview}");
            if let Some(ctx) = m.get("context").and_then(Value::as_array) {
                for c in ctx {
                    let cl = c.get("line").and_then(Value::as_u64).unwrap_or(0);
                    let content = c.get("content").and_then(Value::as_str).unwrap_or("");
                    println!("  {cl}| {content}");
                }
            }
        }
        eprintln!("{}", format!("— {} matches", matches.len()).dimmed());
    } else {
        print!("{value}");
        println!();
    }
    print_truncation_hint(RemoteFileOp::Find, value);
}

fn render_file_hash_human(value: &Value) {
    let algo = value
        .get("algo")
        .and_then(Value::as_str)
        .unwrap_or("sha256");
    let hex = value.get("hex").and_then(Value::as_str).unwrap_or("");
    println!("{algo}: {hex}");
    if let Some(size) = value.get("size").and_then(Value::as_u64) {
        eprintln!("{}", format!("— {size} bytes").dimmed());
    }
}

fn render_file_outline_human(value: &Value) {
    let lang = value
        .get("language")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    if let Some(symbols) = value.get("symbols").and_then(Value::as_array) {
        for s in symbols {
            let kind = s.get("kind").and_then(Value::as_str).unwrap_or("");
            let line = s.get("line").and_then(Value::as_u64).unwrap_or(0);
            let sig = s
                .get("signature")
                .and_then(Value::as_str)
                .or_else(|| s.get("name").and_then(Value::as_str))
                .unwrap_or("");
            println!("{:>5}  {:<10} {}", line, kind, sig);
        }
        let truncated = value
            .get("truncated")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let note = if truncated {
            format!("— {} symbols ({}, truncated)", symbols.len(), lang)
        } else {
            format!("— {} symbols ({})", symbols.len(), lang)
        };
        eprintln!("{}", note.dimmed());
    } else {
        print!("{value}");
        println!();
    }
}

fn render_file_write_human(op: RemoteFileOp, value: &Value) {
    let verb = if op == RemoteFileOp::Edit {
        "Edited"
    } else {
        "Wrote"
    };
    let path = value.get("path").and_then(Value::as_str).unwrap_or("");
    let bytes = value
        .get("bytes_written")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    println!("{} {} ({} bytes)", verb.green(), path, bytes);
    if let Some(n) = value.get("applied_edits").and_then(Value::as_u64) {
        println!("  {n} edit(s) applied");
    }
    if let Some(sha) = value.get("sha256").and_then(Value::as_str) {
        eprintln!("{}", format!("— sha256={sha}").dimmed());
    }
}

fn render_file_mkdir_human(value: &Value) {
    let path = value.get("path").and_then(Value::as_str).unwrap_or("");
    println!("{} {}", "Created".green(), path);
}

fn render_file_move_human(value: &Value) {
    let from = value.get("from").and_then(Value::as_str).unwrap_or("");
    let to = value.get("to").and_then(Value::as_str).unwrap_or("");
    println!("{} {} -> {}", "Moved".green(), from, to);
}

fn render_file_delete_human(value: &Value) {
    let path = value.get("path").and_then(Value::as_str).unwrap_or("");
    println!("{} {}", "Deleted".green(), path);
}

fn render_file_patch_human(value: &Value) {
    if let Some(files) = value.get("files").and_then(Value::as_array) {
        for f in files {
            let path = f.get("path").and_then(Value::as_str).unwrap_or("");
            let from = f.get("from").and_then(Value::as_str).unwrap_or("");
            let to = f.get("to").and_then(Value::as_str).unwrap_or("");
            if f.get("deleted").and_then(Value::as_bool) == Some(true) {
                println!("{} {}", "deleted".red(), path);
            } else if f.get("renamed").and_then(Value::as_bool) == Some(true) {
                println!("{} {} -> {}", "renamed".green(), from, to);
            } else if f.get("copied").and_then(Value::as_bool) == Some(true) {
                let bytes = f.get("bytes_written").and_then(Value::as_u64).unwrap_or(0);
                println!("{} {} -> {} ({} bytes)", "copied".green(), from, to, bytes);
            } else {
                let bytes = f.get("bytes_written").and_then(Value::as_u64).unwrap_or(0);
                println!("{} {} ({} bytes)", "patched".green(), path, bytes);
            }
        }
        eprintln!("{}", format!("— {} file(s)", files.len()).dimmed());
    } else {
        print!("{value}");
        println!();
    }
}

#[derive(Clone)]
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
    caller_pubkey: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    os_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    arch: Option<String>,
}

fn get_os_version() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
                } else {
                    None
                }
            })
    }
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|content| {
                content
                    .lines()
                    .find(|line| line.starts_with("PRETTY_NAME="))
                    .map(|line| {
                        line.trim_start_matches("PRETTY_NAME=")
                            .trim_matches('"')
                            .to_string()
                    })
            })
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StartPairingRequest {
    pair_code: String,
    caller_info: CallerInfo,
    caller_pubkey: String,
    caller_ephemeral_pub: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StartPairingResponse {
    pairing_id: String,
    watch_token: String,
    client_instance_id: String,
    #[serde(default)]
    approval_sse_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PairingWatchResult {
    status: String,
    #[serde(default)]
    claim_token: Option<String>,
    #[serde(default)]
    claim_expires_at: Option<String>,
    #[serde(default)]
    grant_summary: Option<GrantSummary>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    shared_secret_encrypted: Option<String>,
    #[serde(default)]
    grant_session_token: Option<String>,
    #[serde(default)]
    grant_session_expires_at: Option<String>,
    #[serde(default)]
    grant_summary: Option<GrantSummary>,
    #[serde(default)]
    claim_token: Option<String>,
    #[serde(default)]
    claim_expires_at: Option<String>,
}
struct LoadedSshKey {
    key_pair: Ed25519KeyPair,
    device_code: String,
    ssh_key_fingerprint: String,
    source_label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GrantInfo {
    #[serde(default)]
    grant_id: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    grant_session_token: Option<String>,
    #[serde(default)]
    expires_at: Option<String>,
    #[serde(default)]
    grant_summary: Option<GrantSummary>,
    #[serde(default)]
    caller_ephemeral_pub: Option<String>,
    #[serde(default)]
    client_ephemeral_pub: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    shared_secret_encrypted: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GrantSummary {
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    file_access: Option<String>,
    #[serde(default)]
    client_ephemeral_pub: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenCallRequest {
    client_instance_id: String,
    command_summary: OpenCallCommandSummary,
    command_kind: CommandKind,
    command_encrypted: EncryptedPayload,
    #[serde(skip_serializing_if = "Option::is_none")]
    pty_enabled: Option<bool>,
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

#[derive(Debug, Clone)]
struct JobTerminalStatus {
    status: String,
    exit_code: Option<i32>,
    duration_ms: Option<u64>,
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
        conn: &LocalConnection,
        caller_identity: &CallerPopIdentity,
    ) -> bifrost_core::Result<DeleteGrantOutcome> {
        let token = decrypt_grant_session_token(conn)?;
        let url = format!("{}/v5/remote-invoke/grants/revoke", self.base_url);
        let body = pop::sign_envelope(
            serde_json::json!({
                "client_instance_id": conn.client_instance_id,
            }),
            &caller_identity.key_pair,
        )?;
        let response = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .json(&body)
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
        caller_identity: &CallerPopIdentity,
        saved_connection: Option<&LocalConnection>,
    ) -> bifrost_core::Result<Option<GrantInfo>> {
        let pending_transport = if saved_connection.is_some() {
            None
        } else {
            Some(PendingCallerTransport::generate()?)
        };
        let caller_ephemeral_pub = saved_connection
            .and_then(|conn| conn.caller_ephemeral_pub.clone())
            .or_else(|| pending_transport.as_ref().map(|t| t.caller_ephemeral_pub.clone()))
            .ok_or_else(|| {
                BifrostError::Config(
                    "saved connection is missing caller_ephemeral_pub; reconnect to refresh authorization"
                        .to_string(),
                )
            })?;
        let url = format!("{}/v5/remote-invoke/grants/lookup", self.base_url);
        let body = pop::sign_envelope(
            serde_json::json!({
                "client_instance_id": client_instance_id,
                "caller_ephemeral_pub": caller_ephemeral_pub,
            }),
            &caller_identity.key_pair,
        )?;

        let response = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("find reusable grant failed: {e}")))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            let body = response.text().await.unwrap_or_default();
            if body.contains("grant_not_found") {
                return Ok(None);
            }
            return Err(BifrostError::Network(format!(
                "find_reusable_grant failed with status 404: {}",
                truncate(&body, 500)
            )));
        }

        let data: Value = self
            .parse_response_data(response, "find_reusable_grant")
            .await?;
        if data.is_null() {
            return Ok(None);
        }
        let session: GrantInfo = serde_json::from_value(data)
            .map_err(|e| BifrostError::Network(format!("parse grant session failed: {e}")))?;
        let client_ephemeral_pub = session
            .grant_summary
            .as_ref()
            .and_then(|summary| summary.client_ephemeral_pub.as_deref())
            .or(session.client_ephemeral_pub.as_deref())
            .ok_or_else(|| {
                BifrostError::Network(
                    "find_reusable_grant response missing client_ephemeral_pub".to_string(),
                )
            })?;
        let transport = if let Some(conn) = saved_connection {
            let saved_client_pub = conn.client_ephemeral_pub.as_deref().ok_or_else(|| {
                BifrostError::Config(
                    "saved connection is missing client_ephemeral_pub; reconnect to refresh authorization"
                        .to_string(),
                )
            })?;
            if saved_client_pub != client_ephemeral_pub {
                return Err(BifrostError::Config(
                    "relay grant client_ephemeral_pub does not match saved encrypted transport context; reconnect required"
                        .to_string(),
                ));
            }
            StoredTransportContext {
                caller_ephemeral_pub: conn.caller_ephemeral_pub.clone().ok_or_else(|| {
                    BifrostError::Config(
                        "saved connection is missing caller_ephemeral_pub; reconnect to refresh authorization"
                            .to_string(),
                    )
                })?,
                client_ephemeral_pub: saved_client_pub.to_string(),
                shared_secret_encrypted: conn.shared_secret_encrypted.clone().ok_or_else(|| {
                    BifrostError::Config(
                        "saved connection is missing encrypted transport secret; reconnect to refresh authorization"
                            .to_string(),
                    )
                })?,
            }
        } else {
            pending_transport
                .expect("pending transport exists without saved connection")
                .finalize(client_ephemeral_pub)?
        };
        let grant = GrantInfo {
            grant_id: String::new(),
            status: "active".to_string(),
            grant_session_token: session.grant_session_token,
            expires_at: session.expires_at,
            grant_summary: session.grant_summary,
            caller_ephemeral_pub: Some(transport.caller_ephemeral_pub),
            client_ephemeral_pub: Some(transport.client_ephemeral_pub),
            shared_secret_encrypted: Some(transport.shared_secret_encrypted),
        };
        Ok(Some(grant))
    }

    async fn start_pairing(
        &self,
        req: &StartPairingRequest,
    ) -> bifrost_core::Result<StartPairingResponse> {
        let url = format!("{}/v5/remote-invoke/pairings/start", self.base_url);
        let response = self
            .http
            .post(&url)
            .json(req)
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("start pairing failed: {e}")))?;

        self.parse_response_typed(response, "start_pairing").await
    }

    async fn watch_pairing(
        &self,
        pairing_id: &str,
        watch_token: &str,
    ) -> bifrost_core::Result<PairingWatchResult> {
        let url = format!(
            "{}/v5/remote-invoke/pairings/{}/watch?watch_token={}",
            self.base_url,
            pairing_id,
            urlencoding::encode(watch_token),
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
                                                        .or_else(|| v.get("type"))
                                                        .and_then(|s| s.as_str())
                                                        .unwrap_or(&event_name)
                                                        .to_string();
                                                    let claim_token = v.get("claim_token")
                                                        .and_then(|g| g.as_str())
                                                        .map(|s| s.to_string());
                                                    let claim_expires_at = v.get("claim_expires_at")
                                                        .and_then(|g| g.as_str())
                                                        .map(|s| s.to_string());
                                                    let grant_summary = v.get("grant_summary")
                                                        .cloned()
                                                        .and_then(|summary| serde_json::from_value(summary).ok());

                                                    if status == "approved" || status == "rejected" || status == "expired" || status == "cancelled" {
                                                        return Ok(PairingWatchResult {
                                                            status,
                                                            claim_token,
                                                            claim_expires_at,
                                                            grant_summary,
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

    async fn claim_grant(
        &self,
        client_instance_id: &str,
        pair_code: &str,
        claim_token: &str,
        pending_transport: PendingCallerTransport,
        caller_identity: &CallerPopIdentity,
    ) -> bifrost_core::Result<GrantInfo> {
        let url = format!("{}/v5/remote-invoke/grants/claim", self.base_url);
        let body = pop::sign_envelope(
            serde_json::json!({
                "client_instance_id": client_instance_id,
                "pair_code": pair_code,
                "claim_token": claim_token,
                "caller_ephemeral_pub": pending_transport.caller_ephemeral_pub.clone(),
            }),
            &caller_identity.key_pair,
        )?;
        let response = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("claim grant failed: {e}")))?;
        let data: Value = self.parse_response_data(response, "claim_grant").await?;
        let session: GrantInfo = serde_json::from_value(data)
            .map_err(|e| BifrostError::Network(format!("parse grant claim failed: {e}")))?;
        let client_ephemeral_pub = session
            .grant_summary
            .as_ref()
            .and_then(|summary| summary.client_ephemeral_pub.as_deref())
            .ok_or_else(|| {
                BifrostError::Network(
                    "claim_grant response missing client_ephemeral_pub".to_string(),
                )
            })?;
        let transport = pending_transport.finalize(client_ephemeral_pub)?;
        Ok(GrantInfo {
            grant_id: String::new(),
            status: "active".to_string(),
            grant_session_token: session.grant_session_token,
            expires_at: session.expires_at,
            grant_summary: session.grant_summary,
            caller_ephemeral_pub: Some(transport.caller_ephemeral_pub),
            client_ephemeral_pub: Some(transport.client_ephemeral_pub),
            shared_secret_encrypted: Some(transport.shared_secret_encrypted),
        })
    }

    async fn redeem_ssh_claim(
        &self,
        client_instance_id: &str,
        claim_token: &str,
        pending_transport: PendingCallerTransport,
        caller_identity: &CallerPopIdentity,
    ) -> bifrost_core::Result<GrantInfo> {
        let url = format!("{}/v5/remote-invoke/grants/ssh-claim", self.base_url);
        let body = pop::sign_envelope(
            serde_json::json!({
                "client_instance_id": client_instance_id,
                "claim_token": claim_token,
                "caller_ephemeral_pub": pending_transport.caller_ephemeral_pub.clone(),
            }),
            &caller_identity.key_pair,
        )?;
        let response = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("redeem ssh claim failed: {e}")))?;
        let data: Value = self
            .parse_response_data(response, "redeem_ssh_claim")
            .await?;
        let session: GrantInfo = serde_json::from_value(data)
            .map_err(|e| BifrostError::Network(format!("parse ssh claim response failed: {e}")))?;
        let client_ephemeral_pub = session
            .grant_summary
            .as_ref()
            .and_then(|summary| summary.client_ephemeral_pub.as_deref())
            .ok_or_else(|| {
                BifrostError::Network(
                    "redeem_ssh_claim response missing client_ephemeral_pub".to_string(),
                )
            })?;
        let transport = pending_transport.finalize(client_ephemeral_pub)?;
        Ok(GrantInfo {
            grant_id: String::new(),
            status: "active".to_string(),
            grant_session_token: session.grant_session_token,
            expires_at: session.expires_at,
            grant_summary: session.grant_summary,
            caller_ephemeral_pub: Some(transport.caller_ephemeral_pub),
            client_ephemeral_pub: Some(transport.client_ephemeral_pub),
            shared_secret_encrypted: Some(transport.shared_secret_encrypted),
        })
    }

    async fn open_call(
        &self,
        req: &OpenCallRequest,
        grant_session_token: &str,
        caller_identity: &CallerPopIdentity,
    ) -> bifrost_core::Result<OpenCallResponse> {
        let url = format!("{}/v5/remote-invoke/calls/open", self.base_url);
        let body = pop::sign_envelope(
            serde_json::to_value(req).map_err(|e| {
                BifrostError::Config(format!("serialize open_call body failed: {e}"))
            })?,
            &caller_identity.key_pair,
        )?;
        let response = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {grant_session_token}"))
            .json(&body)
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

    async fn post_call_input(
        &self,
        call_id: &str,
        relay_token: &str,
        envelope_json: String,
    ) -> bifrost_core::Result<()> {
        let url = format!("{}/v4/remote-invoke/calls/{}/input", self.base_url, call_id);
        let response = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {relay_token}"))
            .json(&serde_json::json!({ "envelope_json": envelope_json }))
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("post call input failed: {e}")))?;

        let _ = self
            .parse_response_data(response, "post_call_input")
            .await?;
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
                        return Err(BifrostError::Config(remote_call_idle_timeout_message(timeout_secs)));
                    }
                    eprintln!(
                        "  {} {}",
                        "→".bright_yellow(),
                        remote_call_idle_timeout_message(timeout_secs).bright_yellow()
                    );
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
                            // Phase 6 (review fix): the should_reset_idle_deadline_on_chunk
                            // helper expresses the post-exit-grace contract as a pure fn
                            // that is unit-testable; keep the 3s grace authoritative and
                            // do NOT let late stream_frame chunks / keep-alives seed it
                            // back to the 300s idle window.
                            if should_reset_idle_deadline_on_chunk(exit_received) {
                                timeout.as_mut().reset(tokio::time::Instant::now() + idle);
                            }

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
                                                    // Phase 6 (review fix): do NOT re-arm the 3s grace
                                                    // sleep on every late frame. The grace deadline is
                                                    // set exactly once when `exit_received` flips true;
                                                    // otherwise a post-exit stream_frame cadence tighter
                                                    // than 3s keeps the loop alive indefinitely and the
                                                    // caller never returns after the remote has finished.
                                                    debug!("late frame after exit; ignoring grace extension");
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
                                                // Consuming renderer (search/traffic) drains frames
                                                // directly into renderer state, so stdout_parts is
                                                // empty at exit-time. Treat exit as terminal here —
                                                // the "delayed frame" grace path is for the raw
                                                // stdout-buffering case and risks holding the idle
                                                // timer open forever under post-exit stream_frame
                                                // races + 30s relay keep-alives.
                                                if !stream_stdout && search_renderer.is_some() {
                                                    return Ok(result);
                                                }
                                                debug!("exit received with empty stdout, waiting for delayed frame");
                                                exit_received = true;
                                                timeout = Box::pin(tokio::time::sleep(Duration::from_secs(3)));
                                            }
                                            "error" => {
                                                if exit_received {
                                                    // Phase 6 (review fix): after a terminal exit,
                                                    // a trailing `error` event from the relay (most
                                                    // commonly a harmless race against the exit
                                                    // finalize) must not overwrite the captured exit
                                                    // code back to -1. Ignore and keep waiting for
                                                    // the grace deadline or stream close.
                                                    debug!("ignoring post-exit error event");
                                                } else {
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
                                            }
                                            "status" => {
                                                if exit_received {
                                                    // Phase 6 (review fix): a trailing status event
                                                    // after the exit has been observed must not flip
                                                    // exit_code to 130 / cancelled=true. This can
                                                    // happen when the caller cancels late and the
                                                    // relay finalizes the status after exit has
                                                    // already landed.
                                                    debug!("ignoring post-exit status event");
                                                } else if let Ok(v) = serde_json::from_str::<Value>(&data_buf) {
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
                    eprintln!(
                        "  {} {}",
                        "→".bright_yellow(),
                        remote_call_idle_timeout_message(timeout_secs).bright_yellow()
                    );
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
                                        if event_name == "stream_frame" {
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
                                        } else if event_name == "exit" {
                                            // PR E: relay pushes the terminal `exit` event when the target
                                            // finishes executing. Treat it as a Completed outcome so the
                                            // streaming caller loop can terminate rather than entering an
                                            // infinite reconnect loop waiting for a Done frame that worker
                                            // never emits today.
                                            //
                                            // PR review fix: mark digest_ok=false so the outer dispatcher
                                            // does not silently mask a missing digest check when
                                            // --no-verify-digest is not set. A warn! documents the
                                            // downgrade for operators.
                                            if let Ok(v) = serde_json::from_str::<Value>(&data_buf) {
                                                let exit_code = v.get("exit_code").and_then(|x| x.as_i64()).unwrap_or(0) as i32;
                                                let duration_ms = v.get("duration_ms").and_then(|x| x.as_u64());
                                                if state.digest_verification_enabled() {
                                                    warn!("streaming subscribe ended on legacy exit event; digest not verified");
                                                }
                                                return Ok(StreamingSubscriptionOutcome::Completed {
                                                    exit_code,
                                                    duration_ms,
                                                    digest_ok: !state.digest_verification_enabled(),
                                                });
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

    async fn poll_call_terminal_status(
        &self,
        call_id: &str,
        relay_token: &str,
        wait: Duration,
    ) -> bifrost_core::Result<Option<JobTerminalStatus>> {
        let url = format!(
            "{}/v4/remote-invoke/calls/{}/status",
            self.base_url, call_id
        );
        let http = direct_reqwest_client_builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| BifrostError::Network(format!("build job status client: {e}")))?;
        let deadline = tokio::time::Instant::now() + wait;

        loop {
            let response = http
                .get(&url)
                .header("Authorization", format!("Bearer {relay_token}"))
                .send()
                .await
                .map_err(|e| BifrostError::Network(format!("poll job status failed: {e}")))?;
            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                return Err(BifrostError::Network(format!(
                    "job status returned {status}: {}",
                    truncate(&body, 500)
                )));
            }

            let body: Value = response
                .json()
                .await
                .map_err(|e| BifrostError::Network(format!("decode job status response: {e}")))?;
            if let Some(status) = job_terminal_status_from_call_payload(&body) {
                return Ok(Some(status));
            }

            let now = tokio::time::Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            tokio::time::sleep(std::cmp::min(
                Duration::from_millis(200),
                deadline.saturating_duration_since(now),
            ))
            .await;
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
        if env_name != DEFAULT_REMOTE_SSH_KEY_ENV_VAR {
            return Err(BifrostError::Config(format!(
                "remote SSH key env source only supports `{DEFAULT_REMOTE_SSH_KEY_ENV_SPEC}`; pass `--ssh-key` without a value or pass a key file path"
            )));
        }
        let value = std::env::var(env_name).map_err(|_| {
            BifrostError::Config(format!(
                "{DEFAULT_REMOTE_SSH_KEY_ENV_VAR} is not set; pass `--ssh-key <path>` or set {DEFAULT_REMOTE_SSH_KEY_ENV_VAR}"
            ))
        })?;
        if value.trim().is_empty() {
            return Err(BifrostError::Config(format!(
                "{DEFAULT_REMOTE_SSH_KEY_ENV_VAR} is empty; pass `--ssh-key <path>` or set a non-empty {DEFAULT_REMOTE_SSH_KEY_ENV_VAR}"
            )));
        }
        let value = normalize_env_ssh_key_value(value);
        return Ok((
            value.into_bytes(),
            DEFAULT_REMOTE_SSH_KEY_ENV_SPEC.to_string(),
            None,
        ));
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

fn normalize_env_ssh_key_value(value: String) -> String {
    if value.contains("\\n") && !value.contains('\n') {
        value.replace("\\n", "\n")
    } else {
        value
    }
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

fn hex_lower(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len() * 2);
    for byte in data {
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
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

fn parse_call_terminal_status(payload: &Value) -> Option<&str> {
    payload
        .get("status")
        .and_then(|status| status.as_str())
        .filter(|status| matches!(*status, "cancelled" | "completed" | "failed" | "timeout"))
}

fn job_terminal_status_from_call_payload(payload: &Value) -> Option<JobTerminalStatus> {
    let call = payload.get("data").unwrap_or(payload);
    let status = call.get("status").and_then(Value::as_str)?;
    match status {
        "completed" => Some(JobTerminalStatus {
            status: "exited".to_string(),
            exit_code: call
                .get("exit_code")
                .and_then(Value::as_i64)
                .filter(|code| *code >= 0)
                .map(|code| code as i32),
            duration_ms: call.get("duration_ms").and_then(Value::as_u64),
        }),
        "cancelled" | "failed" | "timeout" => Some(JobTerminalStatus {
            status: status.to_string(),
            exit_code: call
                .get("exit_code")
                .and_then(Value::as_i64)
                .filter(|code| *code >= 0)
                .map(|code| code as i32),
            duration_ms: call.get("duration_ms").and_then(Value::as_u64),
        }),
        _ => None,
    }
}

/// PR #5e-2: pure helper that converts a [`StreamingSubscriptionOutcome`]
/// into the next dispatch step taken by [`run_streaming_dispatch`].
#[derive(Debug)]
enum StreamingDispatchStep {
    Complete(CallResult),
    Reconnect((u64, u64), String),
    DisconnectResume((u64, u64), String),
    Error(String),
    Cancelled,
}

/// Hint shown to operators when a streaming dispatch completes with an exit
/// code that almost certainly means "the child was killed by a signal", so the
/// user has a chance to recognize that the failure was external rather than
/// caused by their command. Returns `None` for non-signal exits.
///
/// Kept as a pure helper so it can be unit-tested without spinning up the
/// streaming dispatch loop.
fn streaming_completion_hint(exit_code: i32) -> Option<&'static str> {
    match exit_code {
        143 => Some(
            "hint: exit 143 = SIGTERM (128+15). The remote child was terminated by a signal \
             before it could exit normally. Common causes: caller / shell-wrapper sent SIGTERM, \
             the host's OOM killer, or the policy's max_wall_clock_ms cap. \
             If you expect a long-running task, pass a larger --timeout-ms and confirm the \
             remote policy's max_wall_clock_ms is generous enough.",
        ),
        137 => Some(
            "hint: exit 137 = SIGKILL (128+9). The remote child was force-killed. This is \
             usually the OS OOM killer or a hard wall-clock cap. Check `dmesg` / Console.app on \
             the remote and consider lowering memory pressure or raising the policy ceiling.",
        ),
        _ => None,
    }
}

fn streaming_result_from_outcome(
    outcome: StreamingSubscriptionOutcome,
    verify: bool,
    current_heads: (u64, u64),
    reconnected: bool,
) -> StreamingDispatchStep {
    match outcome {
        StreamingSubscriptionOutcome::Completed {
            exit_code,
            duration_ms,
            digest_ok,
        } => {
            if verify && !digest_ok {
                // Root cause: once the dispatcher has reconnected mid-stream,
                // the running SHA-256 in `CallerStreamState` no longer
                // necessarily lines up with the executor's digest -- the server
                // may resume from a slightly different offset, replay bytes the
                // hasher has already consumed, or compute its digest only over
                // the post-resume tail. Failing the whole call here turns every
                // network blip on a long task into a hard error, which is worse
                // than the integrity signal is worth. Downgrade to a warning so
                // the run still completes; callers that need strict integrity
                // can pass `--no-verify-digest` to silence the warning entirely
                // or can re-run without reconnects.
                if reconnected {
                    warn!(
                        exit_code,
                        stdout_head = current_heads.0,
                        stderr_head = current_heads.1,
                        "stream digest mismatch after reconnect; digest check downgraded to warning"
                    );
                    eprintln!(
                        "warning: stream digest could not be verified after reconnect \
                         (exit_code={exit_code}); continuing"
                    );
                    return StreamingDispatchStep::Complete(CallResult {
                        exit_code,
                        stdout: None,
                        stderr: None,
                        duration_ms,
                        cancelled: false,
                    });
                }
                StreamingDispatchStep::Error(
                    "stream digest mismatch; output may be incomplete. Re-run with `remote exec --detach` and follow with `remote job watch`, or retry with --no-verify-digest only after separately validating the remote log/output.".to_string(),
                )
            } else {
                StreamingDispatchStep::Complete(CallResult {
                    exit_code,
                    stdout: None,
                    stderr: None,
                    duration_ms,
                    cancelled: false,
                })
            }
        }
        StreamingSubscriptionOutcome::ReconnectNeeded {
            stdout_offset,
            stderr_offset,
            reason,
        } => StreamingDispatchStep::Reconnect((stdout_offset, stderr_offset), reason),
        StreamingSubscriptionOutcome::Disconnected { reason } => {
            StreamingDispatchStep::DisconnectResume(current_heads, reason)
        }
        StreamingSubscriptionOutcome::ErrorFrame { code, message } => {
            StreamingDispatchStep::Error(format!("remote error frame {code}: {message}"))
        }
        StreamingSubscriptionOutcome::Cancelled => StreamingDispatchStep::Cancelled,
    }
}

/// PR #5e-2: route a call through [`CallerRelayClient::subscribe_call_events_streaming`]
/// with an outer reconnect-with-offset loop. stdout is written to either the
/// caller-provided `--output-file` or to this process's stdout. stderr is
/// always forwarded to this process's stderr.
async fn run_streaming_dispatch(
    caller: &CallerRelayClient,
    call_id: &str,
    relay_token: &str,
    prefs: &StreamingPrefs,
) -> bifrost_core::Result<CallResult> {
    let stdout_sink: Box<dyn std::io::Write + Send> = match prefs.output_file.as_ref() {
        // PR review fix: use the shared append-mode BufWriter sink so resume
        // semantics are preserved. File::create truncates the output file on
        // every reconnect, defeating --resume-call-id.
        Some(path) => Box::new(
            crate::commands::caller_stream_frame::open_stdout_file_sink(path).map_err(|e| {
                BifrostError::Network(format!("open --output-file {}: {}", path.display(), e))
            })?,
        ),
        None => Box::new(std::io::stdout()),
    };
    let stderr_sink: Box<dyn std::io::Write + Send> = Box::new(std::io::stderr());
    let verify_digest = !prefs.no_verify_digest;
    let mut state = CallerStreamState::new(stdout_sink, stderr_sink, verify_digest);

    let max_reconnects: u32 = 16;
    let mut attempt: u32 = 0;
    let mut resume_from: Option<(u64, u64)> = None;
    // Tracks whether we have entered Reconnect / DisconnectResume at least once.
    // A reconnected stream cannot reliably re-verify its rolling SHA-256, so the
    // dispatcher tells `streaming_result_from_outcome` to downgrade a final
    // digest mismatch to a warning instead of failing the whole run.
    let mut reconnected = false;

    loop {
        let outcome = caller
            .subscribe_call_events_streaming(
                call_id,
                relay_token,
                &mut state,
                resume_from,
                CALL_EVENT_TIMEOUT_SECS,
            )
            .await?;

        let current_heads = (state.stdout_head(), state.stderr_head());
        match streaming_result_from_outcome(outcome, verify_digest, current_heads, reconnected) {
            StreamingDispatchStep::Complete(result) => {
                // Phase 5 (review C9): flush buffered sinks (e.g. the
                // BufWriter<File> used for --output-file) before returning
                // success. A flush failure here surfaces as a Network error
                // rather than a silent data loss.
                if let Err(e) = state.finish() {
                    return Err(BifrostError::Network(format!(
                        "flush output sinks on completion: {e}"
                    )));
                }
                // Friendly diagnostic for the SIGTERM/SIGKILL exit codes that
                // routinely confuse callers running long tasks (see
                // `streaming_completion_hint`). We only emit a hint -- the
                // exit_code itself is propagated verbatim so scripts continue
                // to observe the real status.
                if let Some(hint) = streaming_completion_hint(result.exit_code) {
                    eprintln!("{hint}");
                }
                return Ok(result);
            }
            StreamingDispatchStep::Reconnect(offsets, reason) => {
                warn!(
                    call_id = %call_id,
                    reason = %reason,
                    stdout_offset = offsets.0,
                    stderr_offset = offsets.1,
                    "streaming dispatch reconnect requested"
                );
                attempt += 1;
                if attempt > max_reconnects {
                    return Err(BifrostError::Network(
                        "exceeded max reconnect attempts".to_string(),
                    ));
                }
                resume_from = Some(offsets);
                reconnected = true;
            }
            StreamingDispatchStep::DisconnectResume(offsets, reason) => {
                warn!(
                    call_id = %call_id,
                    reason = %reason,
                    stdout_head = offsets.0,
                    stderr_head = offsets.1,
                    "streaming dispatch disconnected, resuming"
                );
                attempt += 1;
                if attempt > max_reconnects {
                    return Err(BifrostError::Network(
                        "exceeded max reconnect attempts".to_string(),
                    ));
                }
                resume_from = Some(offsets);
                reconnected = true;
            }
            StreamingDispatchStep::Error(message) => {
                return Err(BifrostError::Network(message));
            }
            StreamingDispatchStep::Cancelled => return Ok(synthesized_cancelled_result()),
        }
    }
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
    use crate::cli::{RemoteCommandExecArgs, RemoteSearchArgs, RemoteTrafficListArgs};
    use std::sync::{Mutex, OnceLock};

    fn init_test_data_dir() {
        static TEST_DATA_DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
        let dir = TEST_DATA_DIR.get_or_init(|| tempfile::tempdir().expect("create temp dir"));
        bifrost_storage::set_data_dir(dir.path().to_path_buf());
    }

    fn with_test_data_dir_lock<T>(f: impl FnOnce() -> T) -> T {
        let _guard = REMOTE_TEST_DATA_DIR_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f()
    }

    fn with_remote_ssh_key_env<T>(value: Option<&str>, f: impl FnOnce() -> T) -> T {
        static ENV_LOCK: Mutex<()> = Mutex::new(());
        let _guard = ENV_LOCK.lock().expect("lock env");
        let old = std::env::var(DEFAULT_REMOTE_SSH_KEY_ENV_VAR).ok();
        match value {
            Some(value) => unsafe {
                std::env::set_var(DEFAULT_REMOTE_SSH_KEY_ENV_VAR, value);
            },
            None => unsafe {
                std::env::remove_var(DEFAULT_REMOTE_SSH_KEY_ENV_VAR);
            },
        }
        let result = f();
        match old {
            Some(value) => unsafe {
                std::env::set_var(DEFAULT_REMOTE_SSH_KEY_ENV_VAR, value);
            },
            None => unsafe {
                std::env::remove_var(DEFAULT_REMOTE_SSH_KEY_ENV_VAR);
            },
        }
        result
    }

    fn sample_local_connection(client_id: &str, relay_url: &str) -> LocalConnection {
        LocalConnection {
            client_instance_id: client_id.to_string(),
            device_name: "device".to_string(),
            platform: "macos".to_string(),
            relay_url: relay_url.to_string(),
            grant_id: format!("grant-{client_id}"),
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
            grant_session_token: None,
            grant_session_expires_at: Some("2099-01-01T00:00:00Z".to_string()),
        }
    }

    #[test]
    fn test_random_caller_fingerprint_has_expected_shape() {
        let fingerprint = generate_random_caller_fingerprint().expect("generate caller id");

        assert!(is_valid_caller_fingerprint(&fingerprint));
        assert_eq!(fingerprint.len(), "caller-".len() + 32);
    }

    #[test]
    fn test_load_or_create_caller_fingerprint_persists_per_data_dir() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let path = tempdir.path().join(CALLER_IDENTITY_FILE);

        let first = load_or_create_caller_fingerprint_at(&path).expect("create caller id");
        let second = load_or_create_caller_fingerprint_at(&path).expect("reload caller id");

        assert_eq!(first, second);
        assert!(is_valid_caller_fingerprint(&first));
    }

    #[test]
    fn test_load_or_create_caller_fingerprint_differs_across_data_dirs() {
        let first_dir = tempfile::tempdir().expect("first tempdir");
        let second_dir = tempfile::tempdir().expect("second tempdir");

        let first =
            load_or_create_caller_fingerprint_at(&first_dir.path().join(CALLER_IDENTITY_FILE))
                .expect("create first caller id");
        let second =
            load_or_create_caller_fingerprint_at(&second_dir.path().join(CALLER_IDENTITY_FILE))
                .expect("create second caller id");

        assert_ne!(first, second);
        assert!(is_valid_caller_fingerprint(&first));
        assert!(is_valid_caller_fingerprint(&second));
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
        let built = build_remote_command(&RemoteCommands::Traffic {
            action: RemoteTrafficCommands::Search(Box::new(RemoteSearchArgs {
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
                listener_port: Some(50831),
                protocol: Some("HTTPS".to_string()),
                content_type: Some("json".to_string()),
                domain: Some("example.com".to_string()),
                max_scan: Some(12),
                max_results: Some(7),
                req_json: Vec::new(),
                res_json: Vec::new(),
                req_header_eq: Vec::new(),
                res_header_eq: Vec::new(),
                since: None,
                until: None,
                latest: None,
            })),
        });

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
                assert!(args.filters.conditions.iter().any(|condition| {
                    condition.field == "listener_port"
                        && condition.operator == "equals"
                        && condition.value == "50831"
                }));
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
                listener_port: None,
                protocol: None,
                content_type: None,
                domain: None,
                max_scan: Some(9),
                max_results: Some(3),
                req_json: Vec::new(),
                res_json: Vec::new(),
                req_header_eq: Vec::new(),
                res_header_eq: Vec::new(),
                since: None,
                until: None,
                latest: None,
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
        let built = build_remote_command(&RemoteCommands::Traffic {
            action: RemoteTrafficCommands::Search(Box::new(RemoteSearchArgs {
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
                listener_port: Some(50831),
                protocol: None,
                content_type: None,
                domain: None,
                max_scan: Some(20),
                max_results: Some(9),
                req_json: Vec::new(),
                res_json: Vec::new(),
                req_header_eq: Vec::new(),
                res_header_eq: Vec::new(),
                since: None,
                until: None,
                latest: None,
            })),
        });

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
                assert!(args
                    .filters
                    .conditions
                    .iter()
                    .any(|condition| condition.field == "listener_port"
                        && condition.operator == "equals"
                        && condition.value == "50831"));
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
                listener_port: Some(50831),
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
        assert_eq!(
            json.get("listener_port").and_then(|v| v.as_u64()),
            Some(50831)
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
                assert_eq!(args.listener_port, Some(50831));
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
    fn test_build_remote_command_for_shell_exec_shell_text_uses_shell_exec_kind() {
        let built = build_remote_command(&RemoteCommands::Exec(Box::new(RemoteCommandExecArgs {
            cwd: Some("/srv/api".to_string()),
            env: vec![
                ("NODE_ENV".to_string(), "production".to_string()),
                ("REGION".to_string(), "sg".to_string()),
            ],
            timeout_ms: Some(600_000),
            detach: false,
            login: false,
            shell_text: Some("printf hello".to_string()),
            argv: Vec::new(),
            stream: false,
            output_file: None,
            resume_call_id: None,
            resume_relay_token: None,
            no_verify_digest: false,
            stdin: false,
            pty: false,
            interactive: false,
        })));

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
    fn test_build_remote_command_shell_exec_login_flag_reaches_payload() {
        let built = build_remote_command(&RemoteCommands::Exec(Box::new(RemoteCommandExecArgs {
            cwd: Some("/srv/api".to_string()),
            env: vec![],
            timeout_ms: Some(900_000),
            detach: true,
            login: true,
            shell_text: Some("cargo test".to_string()),
            argv: Vec::new(),
            stream: false,
            output_file: None,
            resume_call_id: None,
            resume_relay_token: None,
            no_verify_digest: false,
            stdin: false,
            pty: false,
            interactive: false,
        })));

        let shell_exec = built.shell_exec.expect("shell_exec payload should exist");
        assert!(shell_exec.login);
        assert_eq!(shell_exec.timeout_ms, Some(900_000));
    }

    #[test]
    fn test_build_remote_command_for_shell_exec_argv_uses_program_preview() {
        let built = build_remote_command(&RemoteCommands::Exec(Box::new(RemoteCommandExecArgs {
            cwd: None,
            env: vec![("A".to_string(), "1".to_string())],
            timeout_ms: Some(5_000),
            detach: false,
            login: false,
            shell_text: None,
            argv: vec![
                "/bin/echo".to_string(),
                "hello".to_string(),
                "--flag".to_string(),
            ],
            stream: false,
            output_file: None,
            resume_call_id: None,
            resume_relay_token: None,
            no_verify_digest: false,
            stdin: false,
            pty: false,
            interactive: false,
        })));

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
        let built = build_remote_command(&RemoteCommands::Exec(Box::new(RemoteCommandExecArgs {
            cwd: None,
            env: vec![],
            timeout_ms: None,
            detach: false,
            login: false,
            shell_text: Some("yes".to_string()),
            argv: Vec::new(),
            stream: true,
            output_file: Some("/tmp/out.bin".to_string()),
            resume_call_id: Some("ab12cd34".to_string()),
            resume_relay_token: Some("tok-xyz".to_string()),
            no_verify_digest: true,
            stdin: false,
            pty: false,
            interactive: false,
        })));

        let prefs = built
            .streaming_prefs
            .expect("streaming_prefs should be populated for shell.exec");
        assert!(prefs.stream);
        assert_eq!(
            prefs.output_file,
            Some(std::path::PathBuf::from("/tmp/out.bin"))
        );
        assert_eq!(prefs.resume_call_id.as_deref(), Some("ab12cd34"));
        assert_eq!(prefs.resume_relay_token.as_deref(), Some("tok-xyz"));
        assert!(prefs.no_verify_digest);
        assert!(prefs.is_streaming());
    }

    #[test]
    fn test_build_remote_command_interactive_disables_digest_verification() {
        let built = build_remote_command(&RemoteCommands::Exec(Box::new(RemoteCommandExecArgs {
            cwd: None,
            env: vec![],
            timeout_ms: None,
            detach: false,
            login: false,
            shell_text: Some("python3 -i".to_string()),
            argv: Vec::new(),
            stream: false,
            output_file: None,
            resume_call_id: None,
            resume_relay_token: None,
            no_verify_digest: false,
            stdin: false,
            pty: false,
            interactive: true,
        })));

        let prefs = built
            .streaming_prefs
            .expect("interactive shell.exec should have streaming prefs");
        let shell_exec = built
            .shell_exec
            .expect("interactive shell.exec should have payload");

        assert!(prefs.stream);
        assert!(prefs.stdin);
        assert!(prefs.interactive);
        assert!(prefs.no_verify_digest);
        assert_eq!(shell_exec.stdin_mode.as_deref(), Some("stream"));
        assert_eq!(shell_exec.output_mode.as_deref(), Some("pty_merged"));
        assert_eq!(shell_exec.pty.as_ref().map(|pty| pty.enabled), Some(true));
        assert!(shell_exec
            .pty
            .as_ref()
            .and_then(|pty| pty.rows)
            .is_some_and(|rows| rows > 0));
        assert!(shell_exec
            .pty
            .as_ref()
            .and_then(|pty| pty.cols)
            .is_some_and(|cols| cols > 0));
    }

    #[test]
    fn test_remote_stdin_forwarder_owns_raw_mode_guard() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let forwarder = RemoteStdinForwarder {
            handle: runtime.spawn(async {}),
            _raw_mode: RawModeGuard::enable(false),
        };
        forwarder.abort();
    }

    #[allow(clippy::field_reassign_with_default)]
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
    fn test_streaming_prefs_resume_requires_token() {
        let prefs = StreamingPrefs {
            stream: false,
            output_file: None,
            resume_call_id: Some("abc".to_string()),
            resume_relay_token: None,
            no_verify_digest: false,
            interactive: false,
            stdin: false,
        };
        let err = validate_streaming_prefs(&prefs).expect_err("should reject missing token");
        assert!(err.contains("--resume-call-id"), "got: {err}");
        assert!(err.contains("--resume-relay-token"), "got: {err}");
    }

    #[test]
    fn test_streaming_prefs_resume_token_without_call_id_rejected() {
        let prefs = StreamingPrefs {
            stream: false,
            output_file: None,
            resume_call_id: None,
            resume_relay_token: Some("tok".to_string()),
            no_verify_digest: false,
            interactive: false,
            stdin: false,
        };
        let err = validate_streaming_prefs(&prefs).expect_err("should reject lone token");
        assert!(err.contains("--resume-relay-token"), "got: {err}");
    }

    #[test]
    fn test_streaming_prefs_resume_with_token_ok() {
        let prefs = StreamingPrefs {
            stream: false,
            output_file: None,
            resume_call_id: Some("abc".to_string()),
            resume_relay_token: Some("tok".to_string()),
            no_verify_digest: false,
            interactive: false,
            stdin: false,
        };
        assert!(validate_streaming_prefs(&prefs).is_ok());
    }

    #[test]
    fn test_streaming_prefs_resume_both_none_ok() {
        let prefs = StreamingPrefs::default();
        assert!(validate_streaming_prefs(&prefs).is_ok());
    }

    #[test]
    fn test_build_remote_command_populates_resume_fields() {
        let built = build_remote_command(&RemoteCommands::Exec(Box::new(RemoteCommandExecArgs {
            cwd: None,
            env: vec![],
            timeout_ms: None,
            detach: false,
            login: false,
            shell_text: Some("tail -f /var/log/x".to_string()),
            argv: Vec::new(),
            stream: false,
            output_file: None,
            resume_call_id: Some("resume-xyz".to_string()),
            resume_relay_token: Some("relay-tok-42".to_string()),
            no_verify_digest: false,
            stdin: false,
            pty: false,
            interactive: false,
        })));

        let prefs = built
            .streaming_prefs
            .expect("streaming_prefs should be populated");
        assert_eq!(prefs.resume_call_id.as_deref(), Some("resume-xyz"));
        assert_eq!(prefs.resume_relay_token.as_deref(), Some("relay-tok-42"));
        assert!(prefs.is_streaming());
        assert!(validate_streaming_prefs(&prefs).is_ok());
    }

    #[test]
    fn test_build_remote_file_scratch_dir_uses_policy_checked_mkdir() {
        let built = build_remote_command(&RemoteCommands::File {
            action: Box::new(RemoteFileCommands::ScratchDir {
                cwd: Some("/repo".to_string()),
                name: ".bifrost-tmp".to_string(),
                output: "json".to_string(),
            }),
        });

        assert_eq!(built.kind, CommandKind::File);
        assert_eq!(built.command.as_deref(), Some("file.mkdir"));
        let args: Value = serde_json::from_str(built.args_json.as_deref().unwrap()).unwrap();
        assert_eq!(
            args.get("path").and_then(Value::as_str),
            Some(".bifrost-tmp")
        );
        assert_eq!(args.get("cwd").and_then(Value::as_str), Some("/repo"));
        assert_eq!(args.get("parents").and_then(Value::as_bool), Some(true));
    }

    #[test]
    fn test_build_remote_command_non_shell_exec_has_no_streaming_prefs() {
        let built = build_remote_command(&RemoteCommands::Conn {
            action: RemoteConnCommands::Status,
        });
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
                ..Default::default()
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
        assert!(message.contains("bifrost remote conn up <pair-code>"));
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
    fn job_terminal_status_from_call_payload_maps_completed_to_exited() {
        let payload = serde_json::json!({
            "code": 0,
            "data": {
                "call_id": "call-1",
                "status": "completed",
                "exit_code": 7,
                "duration_ms": 1234
            }
        });

        let status = job_terminal_status_from_call_payload(&payload).expect("terminal status");
        assert_eq!(status.status, "exited");
        assert_eq!(status.exit_code, Some(7));
        assert_eq!(status.duration_ms, Some(1234));
    }

    #[test]
    fn job_terminal_status_from_call_payload_rejects_streaming() {
        let payload = serde_json::json!({
            "code": 0,
            "data": {
                "call_id": "call-1",
                "status": "streaming",
                "exit_code": -1
            }
        });

        assert!(job_terminal_status_from_call_payload(&payload).is_none());
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
    fn test_load_ssh_key_reads_default_environment_source() {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).expect("generate pkcs8");
        let rendered = format!(
            "{BIFROST_KEY_BEGIN}\nDevice-Code: BF-0123456789ABCDEF\n{}\n{BIFROST_KEY_END}\n",
            base64::engine::general_purpose::STANDARD.encode(pkcs8.as_ref())
        );

        with_remote_ssh_key_env(Some(&rendered), || {
            let loaded = load_ssh_key(DEFAULT_REMOTE_SSH_KEY_ENV_SPEC, None)
                .expect("env source should load");

            assert_eq!(loaded.device_code, "BF-0123456789ABCDEF");
            assert_eq!(loaded.source_label, DEFAULT_REMOTE_SSH_KEY_ENV_SPEC);
        });
    }

    #[test]
    fn test_read_ssh_key_env_source_restores_escaped_newlines() {
        let escaped = format!(
            "{BIFROST_KEY_BEGIN}\\nDevice-Code: BF-0123456789ABCDEF\\nabc\\n{BIFROST_KEY_END}\\n"
        );

        with_remote_ssh_key_env(Some(&escaped), || {
            let (raw, source_label, file_path) =
                read_ssh_key_source(DEFAULT_REMOTE_SSH_KEY_ENV_SPEC).expect("env should read");
            let text = String::from_utf8(raw).expect("env source is utf8");

            assert_eq!(source_label, DEFAULT_REMOTE_SSH_KEY_ENV_SPEC);
            assert!(file_path.is_none());
            assert!(text.contains('\n'));
            assert!(!text.contains("\\n"));
        });
    }

    #[test]
    fn test_read_ssh_key_env_source_requires_fixed_name() {
        let err = read_ssh_key_source("env:OTHER_REMOTE_SSH_KEY")
            .expect_err("arbitrary env var names should be rejected");

        assert!(format!("{err}").contains(DEFAULT_REMOTE_SSH_KEY_ENV_SPEC));
    }

    #[test]
    fn test_read_ssh_key_env_source_reports_missing_fixed_env() {
        with_remote_ssh_key_env(None, || {
            let err = read_ssh_key_source(DEFAULT_REMOTE_SSH_KEY_ENV_SPEC)
                .expect_err("missing env should fail");

            assert!(format!("{err}").contains("BIFROST_REMOTE_SSH_KEY is not set"));
        });
    }

    #[test]
    fn test_read_ssh_key_env_source_rejects_empty_value() {
        with_remote_ssh_key_env(Some(" \n\t"), || {
            let err = read_ssh_key_source(DEFAULT_REMOTE_SSH_KEY_ENV_SPEC)
                .expect_err("empty env should fail");

            assert!(format!("{err}").contains("BIFROST_REMOTE_SSH_KEY is empty"));
        });
    }

    #[test]
    fn test_encrypt_local_secret_roundtrip() {
        with_test_data_dir_lock(|| {
            init_test_data_dir();
            let plaintext = b"shared-secret-test";

            let encrypted = encrypt_local_secret(plaintext).expect("encrypt local secret");
            let decrypted = decrypt_local_secret(&encrypted).expect("decrypt local secret");

            assert_eq!(decrypted, plaintext);
        });
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
            login: false,
            stdin_mode: None,
            pty: None,
            output_mode: None,
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
            client_instance_id: "client-1".to_string(),
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
            pty_enabled: None,
        };

        let json = serde_json::to_value(&request).expect("serialize request");
        assert_eq!(json["command_kind"], "query.readonly");
        assert!(json.get("command_encrypted").is_some());
        assert!(json.get("grant_id").is_none());
        assert!(json.get("caller_fingerprint").is_none());
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
            grant_session_token: None,
            grant_session_expires_at: None,
        };
        let grant = GrantInfo {
            grant_id: "grant-1".to_string(),
            status: "active".to_string(),
            grant_session_token: None,
            expires_at: None,
            grant_summary: None,
            caller_ephemeral_pub: None,
            client_ephemeral_pub: None,
            shared_secret_encrypted: None,
        };

        let err = merge_transport_context(&conn, &grant).expect_err("should require shared secret");
        assert!(err
            .to_string()
            .contains("saved connection is missing encrypted transport secret"));
    }

    #[test]
    fn test_reconcile_reusable_grant_updates_saved_grant_when_transport_matches() {
        with_test_data_dir_lock(|| {
            init_test_data_dir();
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
                grant_session_token: Some(
                    encrypt_grant_session_token("session-token").expect("encrypt grant session"),
                ),
                grant_session_expires_at: Some("2099-01-01T00:00:00Z".to_string()),
            };
            let relay_grant = GrantInfo {
                grant_id: "relay-grant".to_string(),
                status: "active".to_string(),
                grant_session_token: Some("session-new".to_string()),
                expires_at: None,
                grant_summary: None,
                caller_ephemeral_pub: Some("saved-caller-epk".to_string()),
                client_ephemeral_pub: Some("saved-client-epk".to_string()),
                shared_secret_encrypted: None,
            };

            let preferred = reconcile_reusable_grant_for_transport(&mut conn, relay_grant)
                .expect("matching transport should reconcile");

            assert_eq!(preferred.grant_id, "relay-grant");
            assert_eq!(conn.grant_id, "relay-grant");
            assert_ne!(conn.grant_session_token.as_deref(), Some("session-new"));
            assert_eq!(
                decrypt_grant_session_token(&conn).expect("decrypt saved grant session"),
                "session-new"
            );
        });
    }

    #[test]
    fn test_reconcile_reusable_grant_rejects_transport_mismatch() {
        with_test_data_dir_lock(|| {
            init_test_data_dir();
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
                grant_session_token: Some(
                    encrypt_grant_session_token("session-token").expect("encrypt grant session"),
                ),
                grant_session_expires_at: Some("2099-01-01T00:00:00Z".to_string()),
            };
            let relay_grant = GrantInfo {
                grant_id: "relay-grant".to_string(),
                status: "active".to_string(),
                grant_session_token: None,
                expires_at: None,
                grant_summary: None,
                caller_ephemeral_pub: Some("relay-caller-epk".to_string()),
                client_ephemeral_pub: Some("saved-client-epk".to_string()),
                shared_secret_encrypted: None,
            };

            let err = reconcile_reusable_grant_for_transport(&mut conn, relay_grant)
                .expect_err("mismatched transport should require reconnect");
            assert!(err.to_string().contains(
                "saved connection transport no longer matches relay reusable authorization"
            ));
        });
    }

    #[test]
    fn test_upsert_local_connection_replaces_duplicate_client_entries() {
        with_test_data_dir_lock(|| {
            init_test_data_dir();
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
                    grant_session_token: Some(
                        encrypt_grant_session_token("session-old").expect("encrypt grant session"),
                    ),
                    grant_session_expires_at: Some("2099-01-01T00:00:00Z".to_string()),
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
                    grant_session_token: Some(
                        encrypt_grant_session_token("session-other")
                            .expect("encrypt grant session"),
                    ),
                    grant_session_expires_at: Some("2099-01-01T00:00:00Z".to_string()),
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
                    grant_session_token: Some(
                        encrypt_grant_session_token("session-new").expect("encrypt grant session"),
                    ),
                    grant_session_expires_at: Some("2099-01-01T00:00:00Z".to_string()),
                },
            );

            assert_eq!(connections.len(), 2);
            let replaced = connections
                .iter()
                .find(|conn| conn.client_instance_id == "client-1")
                .expect("replaced connection should remain");
            assert_eq!(replaced.device_name, "new");
            assert_eq!(replaced.grant_id, "grant-new");
        });
    }

    #[test]
    fn test_is_grant_scope_mismatch_error_detects_open_call_403() {
        let err = BifrostError::Network(
            "open_call failed with status 403 Forbidden: {\"code\":403,\"message\":\"grant_scope_mismatch\",\"data\":null}".to_string(),
        );

        assert!(is_grant_scope_mismatch_error(&err));
    }

    #[test]
    fn test_is_stale_remote_grant_error_detects_open_call_403() {
        let stale_messages = [
            "grant_not_active",
            "grant_revoked",
            "grant_missing_shared_secret",
            "grant_not_found",
        ];

        for message in stale_messages {
            let err = BifrostError::Network(format!(
                "open_call failed with status 403 Forbidden: {{\"code\":-1,\"message\":\"{message}\"}}"
            ));
            assert!(
                is_stale_remote_grant_error(&err),
                "expected stale grant classification for {message}"
            );
        }
    }

    #[test]
    fn test_is_stale_remote_grant_error_rejects_scope_mismatch() {
        let err = BifrostError::Network(
            "open_call failed with status 403 Forbidden: {\"code\":403,\"message\":\"grant_scope_mismatch\",\"data\":null}".to_string(),
        );

        assert!(!is_stale_remote_grant_error(&err));
    }

    #[test]
    fn test_is_grant_session_token_invalid_error_detects_open_call_401() {
        let err = BifrostError::Network(
            "open_call failed with status 401 Unauthorized: {\"code\":-1,\"message\":\"grant_session_token_invalid\"}".to_string(),
        );

        assert!(is_grant_session_token_invalid_error(&err));
    }

    #[test]
    fn test_is_grant_session_token_invalid_error_rejects_other_401() {
        let err = BifrostError::Network(
            "open_call failed with status 401 Unauthorized: {\"code\":-1,\"message\":\"caller_pubkey_mismatch\"}".to_string(),
        );

        assert!(!is_grant_session_token_invalid_error(&err));
    }

    #[test]
    fn test_shell_scope_upgrade_error_for_ssh_key_requires_explicit_grant_update() {
        let mut conn = sample_local_connection("client-ssh", "https://relay.example");
        conn.auth_method = Some("ssh_publickey".to_string());
        conn.ssh_key_source = Some("/tmp/test.bifrost".to_string());
        conn.device_code = Some("BF-TEST".to_string());

        let err = shell_scope_upgrade_error(&conn).to_string();

        assert!(err.contains("does not allow shell.exec"));
        assert!(err.contains("bifrost setting grant update --level shell"));
        assert!(!err.contains("fresh SSH authorization"));
    }

    #[test]
    fn test_remove_matching_local_connection_removes_only_same_client_and_relay() {
        let stale = sample_local_connection("client-1", "https://relay-a");
        let mut connections = vec![
            stale.clone(),
            sample_local_connection("client-1", "https://relay-b"),
            sample_local_connection("client-2", "https://relay-a"),
        ];

        assert!(remove_matching_local_connection(&mut connections, &stale));

        assert_eq!(connections.len(), 2);
        assert!(connections.iter().any(
            |conn| conn.client_instance_id == "client-1" && conn.relay_url == "https://relay-b"
        ));
        assert!(connections.iter().any(
            |conn| conn.client_instance_id == "client-2" && conn.relay_url == "https://relay-a"
        ));
    }

    #[test]
    fn test_stale_remote_grant_error_is_actionable_without_shared_secret_text() {
        let conn = sample_local_connection("client-1", "https://relay-a");

        let message = stale_remote_grant_error(&conn).to_string();

        assert!(message.contains("expired") || message.contains("revoked"));
        assert!(message.contains("re-connect"));
        assert!(!message.contains("missing grant shared secret"));
    }

    #[test]
    fn streaming_result_completed_ok() {
        let step = streaming_result_from_outcome(
            StreamingSubscriptionOutcome::Completed {
                exit_code: 0,
                duration_ms: Some(42),
                digest_ok: true,
            },
            true,
            (10, 20),
            false,
        );
        match step {
            StreamingDispatchStep::Complete(r) => {
                assert_eq!(r.exit_code, 0);
                assert_eq!(r.duration_ms, Some(42));
                assert!(!r.cancelled);
                assert!(r.stdout.is_none());
                assert!(r.stderr.is_none());
            }
            other => panic!("expected Complete, got {:?}", other),
        }
    }

    #[test]
    fn streaming_result_completed_digest_mismatch_with_verify_errs() {
        let step = streaming_result_from_outcome(
            StreamingSubscriptionOutcome::Completed {
                exit_code: 0,
                duration_ms: Some(1),
                digest_ok: false,
            },
            true,
            (0, 0),
            false,
        );
        match step {
            StreamingDispatchStep::Error(msg) => assert!(msg.contains("digest")),
            other => panic!("expected Error, got {:?}", other),
        }
    }

    #[test]
    fn streaming_result_completed_digest_mismatch_without_verify_completes() {
        let step = streaming_result_from_outcome(
            StreamingSubscriptionOutcome::Completed {
                exit_code: 7,
                duration_ms: Some(2),
                digest_ok: false,
            },
            false,
            (0, 0),
            false,
        );
        match step {
            StreamingDispatchStep::Complete(r) => assert_eq!(r.exit_code, 7),
            other => panic!("expected Complete, got {:?}", other),
        }
    }

    /// Regression: once the streaming dispatcher has reconnected, the running
    /// SHA-256 in `CallerStreamState` cannot be trusted (see
    /// `streaming_result_from_outcome` doc comment). A digest mismatch after a
    /// reconnect must therefore complete the call (with a warning), not surface
    /// as a hard `Error`, so long-running tasks survive a single network blip.
    #[test]
    fn streaming_result_completed_digest_mismatch_after_reconnect_completes() {
        let step = streaming_result_from_outcome(
            StreamingSubscriptionOutcome::Completed {
                exit_code: 9,
                duration_ms: Some(11),
                digest_ok: false,
            },
            /* verify */ true,
            (12_345, 6_789),
            /* reconnected */ true,
        );
        match step {
            StreamingDispatchStep::Complete(r) => {
                assert_eq!(r.exit_code, 9);
                assert_eq!(r.duration_ms, Some(11));
                assert!(!r.cancelled);
            }
            other => panic!("expected Complete after reconnect, got {:?}", other),
        }
    }

    #[test]
    fn streaming_result_reconnect_propagates_offsets() {
        let step = streaming_result_from_outcome(
            StreamingSubscriptionOutcome::ReconnectNeeded {
                stdout_offset: 123,
                stderr_offset: 456,
                reason: "gap".to_string(),
            },
            true,
            (0, 0),
            false,
        );
        match step {
            StreamingDispatchStep::Reconnect((so, se), reason) => {
                assert_eq!(so, 123);
                assert_eq!(se, 456);
                assert_eq!(reason, "gap");
            }
            other => panic!("expected Reconnect, got {:?}", other),
        }
    }

    #[test]
    fn streaming_result_disconnected_uses_current_heads() {
        let step = streaming_result_from_outcome(
            StreamingSubscriptionOutcome::Disconnected {
                reason: "sse closed".to_string(),
            },
            true,
            (11, 22),
            false,
        );
        match step {
            StreamingDispatchStep::DisconnectResume((so, se), reason) => {
                assert_eq!(so, 11);
                assert_eq!(se, 22);
                assert_eq!(reason, "sse closed");
            }
            other => panic!("expected DisconnectResume, got {:?}", other),
        }
    }

    #[test]
    fn streaming_result_error_frame_maps_to_error() {
        let step = streaming_result_from_outcome(
            StreamingSubscriptionOutcome::ErrorFrame {
                code: "E_FOO".to_string(),
                message: "boom".to_string(),
            },
            true,
            (0, 0),
            false,
        );
        match step {
            StreamingDispatchStep::Error(msg) => {
                assert!(msg.contains("E_FOO"));
                assert!(msg.contains("boom"));
            }
            other => panic!("expected Error, got {:?}", other),
        }
    }

    #[test]
    fn streaming_result_cancelled_maps_to_cancelled() {
        let step = streaming_result_from_outcome(
            StreamingSubscriptionOutcome::Cancelled,
            true,
            (0, 0),
            false,
        );
        match step {
            StreamingDispatchStep::Cancelled => {}
            other => panic!("expected Cancelled, got {:?}", other),
        }
    }

    /// Pure-helper test for the SIGTERM/SIGKILL exit hint surfaced by the
    /// streaming dispatcher when the remote child terminates with a signal.
    /// Keeps the assertion narrow (presence of the exit-code number + the
    /// `--timeout-ms` workaround hint) so future copy edits do not break it.
    #[test]
    fn streaming_completion_hint_explains_signal_exits() {
        let hint_143 = streaming_completion_hint(143).expect("143 should produce a SIGTERM hint");
        assert!(hint_143.contains("143"), "got: {hint_143}");
        assert!(hint_143.contains("SIGTERM"), "got: {hint_143}");
        assert!(hint_143.contains("--timeout-ms"), "got: {hint_143}");

        let hint_137 = streaming_completion_hint(137).expect("137 should produce a SIGKILL hint");
        assert!(hint_137.contains("137"), "got: {hint_137}");
        assert!(hint_137.contains("SIGKILL"), "got: {hint_137}");

        assert!(streaming_completion_hint(0).is_none());
        assert!(streaming_completion_hint(1).is_none());
        assert!(streaming_completion_hint(130).is_none());
    }

    #[test]
    fn validate_remote_command_exec_flags_rejects_resume_with_shell_text() {
        let args = RemoteCommandExecArgs {
            cwd: None,
            env: Vec::new(),
            timeout_ms: None,
            detach: false,
            login: false,
            shell_text: Some("echo hi".to_string()),
            argv: Vec::new(),
            stream: false,
            output_file: None,
            resume_call_id: Some("abc".to_string()),
            resume_relay_token: Some("tok".to_string()),
            no_verify_digest: false,
            stdin: false,
            pty: false,
            interactive: false,
        };
        let err = validate_remote_command_exec_flags(&args).unwrap_err();
        assert!(
            err.contains("--resume-call-id cannot be combined with --shell-text"),
            "got: {err}"
        );
    }

    #[test]
    fn validate_remote_command_exec_flags_rejects_resume_with_argv() {
        let args = RemoteCommandExecArgs {
            cwd: None,
            env: Vec::new(),
            timeout_ms: None,
            detach: false,
            login: false,
            shell_text: None,
            argv: vec!["ls".to_string(), "-la".to_string()],
            stream: false,
            output_file: None,
            resume_call_id: Some("abc".to_string()),
            resume_relay_token: Some("tok".to_string()),
            no_verify_digest: false,
            stdin: false,
            pty: false,
            interactive: false,
        };
        let err = validate_remote_command_exec_flags(&args).unwrap_err();
        assert!(
            err.contains("--resume-call-id cannot be combined with a trailing program"),
            "got: {err}"
        );
    }

    #[test]
    fn validate_remote_command_exec_flags_allows_plain_resume() {
        let args = RemoteCommandExecArgs {
            cwd: None,
            env: Vec::new(),
            timeout_ms: None,
            detach: false,
            login: false,
            shell_text: None,
            argv: Vec::new(),
            stream: false,
            output_file: None,
            resume_call_id: Some("abc".to_string()),
            resume_relay_token: Some("tok".to_string()),
            no_verify_digest: false,
            stdin: false,
            pty: false,
            interactive: false,
        };
        assert!(validate_remote_command_exec_flags(&args).is_ok());
    }

    #[test]
    fn validate_remote_command_exec_flags_allows_fresh_shell_text() {
        let args = RemoteCommandExecArgs {
            cwd: None,
            env: Vec::new(),
            timeout_ms: None,
            detach: false,
            login: false,
            shell_text: Some("echo hi".to_string()),
            argv: Vec::new(),
            stream: false,
            output_file: None,
            resume_call_id: None,
            resume_relay_token: None,
            no_verify_digest: false,
            stdin: false,
            pty: false,
            interactive: false,
        };
        assert!(validate_remote_command_exec_flags(&args).is_ok());
    }

    #[test]
    fn post_exit_guard_prevents_idle_deadline_reset() {
        // Phase 6: once exit has been observed, the idle-deadline reset
        // on incoming SSE chunks MUST be suppressed so the 3s grace is
        // authoritative. A cadence of post-exit frames tighter than the
        // grace must not extend the timer indefinitely.
        assert!(should_reset_idle_deadline_on_chunk(false));
        assert!(!should_reset_idle_deadline_on_chunk(true));
    }

    #[test]
    fn sse_stream_frame_with_forward_gap_bubbles_up_offset_ahead() {
        // Phase 6: integration check that combines the SSE decoder
        // (parse_stream_frame_from_sse_data) with the state machine
        // (CallerStreamState::feed). A relay-shaped
        // {call_id, frame_json: "<Stdout offset=10 ...>"} payload whose
        // offset starts strictly past head must produce
        // StreamIngestError::OffsetAhead, not a silent digest mismatch.
        use crate::commands::caller_stream_frame::{
            parse_stream_frame_from_sse_data, CallerStreamState, StreamIngestError,
        };

        // Build the stringified StreamFrame then wrap it in the relay envelope.
        // Using serde_json::json! avoids fragile manual backslash-escaping.
        let first_inner = serde_json::json!({
            "kind": "stdout",
            "seq": 0u64,
            "offset": 0u64,
            "data_b64": "YWJj", // "abc"
        })
        .to_string();
        let first = serde_json::json!({
            "call_id": "c1",
            "frame_json": first_inner,
        })
        .to_string();

        let mut state = CallerStreamState::new(Vec::new(), Vec::new(), false);
        let f = parse_stream_frame_from_sse_data(&first).expect("parse first");
        state.feed(&f).expect("feed first");
        assert_eq!(state.stdout_head(), 3);

        let gap_inner = serde_json::json!({
            "kind": "stdout",
            "seq": 1u64,
            "offset": 10u64,
            "data_b64": "WFla", // "XYZ"
        })
        .to_string();
        let gap = serde_json::json!({
            "call_id": "c1",
            "frame_json": gap_inner,
        })
        .to_string();
        let f = parse_stream_frame_from_sse_data(&gap).expect("parse gap");
        let err = state.feed(&f).unwrap_err();
        match err {
            StreamIngestError::OffsetAhead {
                kind,
                got,
                expected,
            } => {
                assert_eq!(kind, "stdout");
                assert_eq!(got, 10);
                assert_eq!(expected, 3);
            }
            other => panic!("expected OffsetAhead, got {other:?}"),
        }
    }

    #[test]
    fn sha256_hex_empty_input_matches_known_digest() {
        let hex = sha256_hex(b"");
        assert_eq!(
            hex,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_hex_abc_matches_known_digest() {
        let hex = sha256_hex(b"abc");
        assert_eq!(
            hex,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn hex_lower_formats_bytes_as_lowercase_hex() {
        let bytes = [0x00u8, 0x01, 0xAB, 0xCD, 0xFF];
        assert_eq!(hex_lower(&bytes), "0001abcdff".to_string());
    }

    #[test]
    fn ed25519_public_key_to_spki_der_prefix_and_key() {
        let public_key = [0x11u8; 32];
        let der = ed25519_public_key_to_spki_der(&public_key);
        assert_eq!(der.len(), 12 + public_key.len());
        assert_eq!(
            &der[..12],
            &[0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00]
        );
        assert_eq!(&der[12..], &public_key);
    }

    #[test]
    fn derive_device_code_is_deterministic_and_prefixed() {
        let der = vec![0x42u8; 64];
        let code1 = derive_device_code(&der);
        let code2 = derive_device_code(&der);
        assert_eq!(code1, code2);
        assert!(code1.starts_with("BF-"));
        assert_eq!(code1.len(), 3 + 16);
        let suffix = &code1[3..];
        assert!(suffix.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn derive_device_code_varies_with_input() {
        let der1 = vec![0x01u8; 64];
        let der2 = vec![0x02u8; 64];
        let code1 = derive_device_code(&der1);
        let code2 = derive_device_code(&der2);
        assert_ne!(code1, code2);
    }

    #[test]
    fn truncate_returns_original_when_shorter_than_max() {
        let s = "hello";
        assert_eq!(truncate(s, 10), "hello".to_string());
    }

    #[test]
    fn truncate_returns_original_when_equal_to_max() {
        let s = "hello";
        assert_eq!(truncate(s, 5), "hello".to_string());
    }

    #[test]
    fn truncate_appends_suffix_when_truncated() {
        let s = "abcdef";
        let t = truncate(s, 3);
        assert!(t.starts_with("abc"));
        assert!(t.ends_with("...(truncated)"));
    }

    #[test]
    fn synthesized_cancelled_result_has_expected_shape() {
        let r = synthesized_cancelled_result();
        assert_eq!(r.exit_code, 130);
        assert!(r.cancelled);
        assert_eq!(r.stderr.as_deref(), Some("remote call cancelled by caller"));
        assert!(r.stdout.is_none());
    }

    #[test]
    fn retryable_cancel_settle_error_matches_cancel_call_429() {
        let err = BifrostError::Network(
            "cancel_call failed with status 429 Too Many Requests".to_string(),
        );
        assert!(is_retryable_cancel_settle_error(&err));
    }

    #[test]
    fn retryable_cancel_settle_error_matches_too_many_requests_message() {
        let err = BifrostError::Network("too many requests, retry after 10s".to_string());
        assert!(is_retryable_cancel_settle_error(&err));
    }

    #[test]
    fn retryable_cancel_settle_error_rejects_other_network_errors() {
        let err = BifrostError::Network("some other network error".to_string());
        assert!(!is_retryable_cancel_settle_error(&err));
    }

    #[test]
    fn ambiguous_cancel_settle_result_false_when_cancelled_flag_set() {
        let result = CallResult {
            exit_code: 0,
            stdout: None,
            stderr: None,
            duration_ms: None,
            cancelled: true,
        };
        assert!(!is_ambiguous_cancel_settle_result(&result));
    }

    #[test]
    fn ambiguous_cancel_settle_result_false_when_non_zero_exit_code() {
        let result = CallResult {
            exit_code: 1,
            stdout: None,
            stderr: None,
            duration_ms: None,
            cancelled: false,
        };
        assert!(!is_ambiguous_cancel_settle_result(&result));
    }

    #[test]
    fn parse_call_terminal_status_accepts_all_terminal_states() {
        for status in ["cancelled", "completed", "failed", "timeout"] {
            let payload = serde_json::json!({
                "call_id": "c1",
                "status": status,
            });
            assert_eq!(parse_call_terminal_status(&payload), Some(status));
        }
    }

    #[test]
    fn parse_call_terminal_status_missing_or_non_string_yields_none() {
        let payload = serde_json::json!({ "call_id": "c1" });
        assert_eq!(parse_call_terminal_status(&payload), None);

        let payload = serde_json::json!({
            "call_id": "c1",
            "status": 123,
        });
        assert_eq!(parse_call_terminal_status(&payload), None);
    }

    #[test]
    fn render_search_size_formats_units() {
        assert_eq!(render_search_size(0), "0B");
        assert_eq!(render_search_size(999), "999B");
        assert_eq!(render_search_size(1024), "1.0KB");
        assert_eq!(render_search_size(10 * 1024 * 1024), "10.0MB");
    }

    #[test]
    fn render_search_duration_formats_ms_and_seconds() {
        assert_eq!(render_search_duration(0), "...".to_string());
        assert_eq!(render_search_duration(1), "1ms".to_string());
        assert_eq!(render_search_duration(1500), "1.50s".to_string());
    }

    #[test]
    fn render_search_number_formats_k_and_m_suffixes() {
        assert_eq!(render_search_number(42), "42".to_string());
        assert_eq!(render_search_number(1_200), "1.2K".to_string());
        assert_eq!(render_search_number(2_500_000), "2.5M".to_string());
    }

    #[test]
    fn should_stream_remote_render_only_for_raw_mode() {
        let raw = RemoteRenderMode::Raw;
        assert!(should_stream_remote_render(&raw));

        let search = RemoteRenderMode::Search {
            format: OutputFormat::Table,
            no_color: false,
            keyword: String::new(),
            max_scan: None,
            max_results: None,
        };
        assert!(!should_stream_remote_render(&search));

        let traffic = RemoteRenderMode::TrafficList {
            format: OutputFormat::Compact,
            no_color: true,
        };
        assert!(!should_stream_remote_render(&traffic));
    }

    #[cfg(unix)]
    #[test]
    fn warn_if_ssh_key_permissions_are_too_open_does_not_panic() {
        use std::fs::{self, File};
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("key");
        File::create(&path).expect("create key");

        // Mode 0o600 should not trigger any warning logic that panics.
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o600);
        fs::set_permissions(&path, perms).unwrap();
        warn_if_ssh_key_permissions_are_too_open(&path);

        // Mode 0o777 should emit a warning but must not panic.
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o777);
        fs::set_permissions(&path, perms).unwrap();
        warn_if_ssh_key_permissions_are_too_open(&path);
    }

    // ---------- remote_file_error_hint coverage for new branches ----------

    #[test]
    fn test_remote_file_error_hint_op_not_permitted() {
        let stderr =
            "[file.op_not_permitted] requested op read_many is not permitted by the active policy";
        let hint = remote_file_error_hint(stderr).expect("hint should be present");
        assert!(
            hint.contains("`remote file read`"),
            "hint should point at the per-file read fallback, got: {hint}"
        );
        assert!(
            hint.contains("read_many"),
            "hint should name the read_many op so callers know what to enable, got: {hint}"
        );
    }

    #[test]
    fn test_remote_file_error_hint_anchor_not_found() {
        let stderr = "[file.anchor_not_found] old_string not found in 'foo.rs'";
        let hint = remote_file_error_hint(stderr).expect("hint should be present");
        assert!(
            hint.contains("Re-`read`") && hint.contains("re-anchor"),
            "hint should tell agent to re-read and re-anchor, got: {hint}"
        );
    }

    #[test]
    fn test_remote_file_error_hint_anchor_not_unique() {
        let stderr = "[file.anchor_not_unique] old_string found 3 time(s) in 'foo.rs', expected 1";
        let hint = remote_file_error_hint(stderr).expect("hint should be present");
        assert!(
            hint.contains("expected_count"),
            "hint should mention expected_count, got: {hint}"
        );
        assert!(
            hint.contains("more specific old_string"),
            "hint should suggest a more specific old_string, got: {hint}"
        );
    }

    #[test]
    fn test_remote_file_error_hint_out_of_scope_mentions_repo_tmp() {
        let stderr = "[file.out_of_scope] path /tmp/foo is outside the granted roots";
        let hint = remote_file_error_hint(stderr).expect("hint should be present");
        assert!(
            hint.contains("scratch-dir"),
            "hint should point ephemeral writes at `remote file scratch-dir`, got: {hint}"
        );
        assert!(
            hint.contains("FileAccessPolicy"),
            "hint should keep the FileAccessPolicy guidance, got: {hint}"
        );
    }

    // ---------- format_mtime_rfc3339 snapshot ----------

    #[test]
    fn test_format_mtime_rfc3339_renders_utc_z() {
        // 2026-06-16T12:34:56Z
        let ts = 1_781_613_296_i64;
        let formatted = format_mtime_rfc3339(ts).expect("timestamp should be representable");
        assert_eq!(formatted, "2026-06-16T12:34:56Z");
    }

    #[test]
    fn test_format_mtime_rfc3339_handles_epoch() {
        let formatted = format_mtime_rfc3339(0).expect("epoch should format");
        assert_eq!(formatted, "1970-01-01T00:00:00Z");
    }
}

#[cfg(test)]
mod coverage_boost {
    use super::*;
    use crate::commands::search::SearchResultItem;
    use bifrost_command::{SearchArgs, TrafficClearArgs};
    use serde_json::{json, Value};
    use std::sync::OnceLock;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn with_connections_lock<T>(f: impl FnOnce() -> T) -> T {
        let _guard = REMOTE_TEST_DATA_DIR_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f()
    }

    fn init_test_data_dir() {
        static TEST_DATA_DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
        let dir = TEST_DATA_DIR.get_or_init(|| tempfile::tempdir().expect("create temp dir"));
        bifrost_storage::set_data_dir(dir.path().to_path_buf());
    }

    fn sample_local_connection(client_id: &str, relay_url: &str) -> LocalConnection {
        LocalConnection {
            client_instance_id: client_id.to_string(),
            device_name: "device".to_string(),
            platform: "macos".to_string(),
            relay_url: relay_url.to_string(),
            grant_id: format!("grant-{client_id}"),
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
            grant_session_token: None,
            grant_session_expires_at: Some("2099-01-01T00:00:00Z".to_string()),
        }
    }

    fn sample_authorized_connection(client_id: &str, relay_url: &str) -> LocalConnection {
        let mut conn = sample_local_connection(client_id, relay_url);
        conn.shared_secret_encrypted =
            Some(encrypt_local_secret(b"secret").expect("encrypt shared secret"));
        conn.grant_session_token =
            Some(encrypt_grant_session_token("session-token").expect("encrypt grant session"));
        conn
    }

    fn sample_remote_job(call_id: &str, relay_url: &str, token: &str) -> RemoteJobRecord {
        RemoteJobRecord {
            call_id: call_id.to_string(),
            relay_token_encrypted: encrypt_local_secret(token.as_bytes()).expect("encrypt token"),
            relay_url: relay_url.to_string(),
            client_instance_id: Some("client-1".to_string()),
            device_name: Some("device".to_string()),
            command_preview: Some("cargo test".to_string()),
            status: "running".to_string(),
            exit_code: None,
            created_at: 10,
            updated_at: 10,
        }
    }

    // --- Base64 helpers and fingerprints ---

    #[test]
    fn decode_base64_or_raw_empty_returns_empty_vec() {
        assert!(decode_base64_or_raw("").is_empty());
    }

    #[test]
    fn decode_base64_or_raw_valid_base64_decodes() {
        let bytes = decode_base64_or_raw("aGVsbG8=");
        assert_eq!(bytes, b"hello");
    }

    #[test]
    fn decode_base64_or_raw_invalid_returns_raw_bytes() {
        let input = "not-base64??";
        let output = decode_base64_or_raw(input);
        assert_eq!(output, input.as_bytes());
    }

    #[test]
    fn short_fingerprint_uses_first_six_sha256_bytes() {
        let fp = short_fingerprint(b"abc");
        assert_eq!(fp, "ba7816bf8f01");
    }

    // --- Shell exec env and query args JSON ---

    #[test]
    fn remote_shell_exec_env_empty_returns_none() {
        assert!(remote_shell_exec_env(&[]).is_none());
    }

    #[test]
    fn remote_shell_exec_env_collects_pairs_into_map() {
        let env = remote_shell_exec_env(&[
            ("A".to_string(), "1".to_string()),
            ("B".to_string(), "2".to_string()),
        ])
        .expect("env map should be populated");
        assert_eq!(env.get("A").map(String::as_str), Some("1"));
        assert_eq!(env.get("B").map(String::as_str), Some("2"));
    }

    #[test]
    fn query_args_json_handles_search_variant() {
        let query = CanonicalQueryCommand::Search(SearchArgs {
            keyword: "needle".to_string(),
            limit: Some(10),
            ..SearchArgs::default()
        });

        let json_str = query_args_json(&query).expect("search args_json");
        let v: Value = serde_json::from_str(&json_str).expect("valid json");
        assert_eq!(v.get("keyword").and_then(|k| k.as_str()), Some("needle"));
    }

    #[test]
    fn query_args_json_handles_traffic_list_variant() {
        let args = TrafficListArgs {
            limit: Some(5),
            cursor: Some(42),
            direction: TrafficListDirection::Backward,
            host: Some("example.com".to_string()),
            ..TrafficListArgs::default()
        };
        let query = CanonicalQueryCommand::TrafficList(args.clone());

        let json_str = query_args_json(&query).expect("traffic list args_json");
        let v: Value = serde_json::from_str(&json_str).expect("valid json");
        assert_eq!(v.get("limit").and_then(|k| k.as_u64()), Some(5));
        assert_eq!(v.get("cursor").and_then(|k| k.as_u64()), Some(42));
        assert_eq!(v.get("host").and_then(|k| k.as_str()), Some("example.com"));
    }

    #[test]
    fn query_args_json_handles_traffic_get_variant() {
        let query = CanonicalQueryCommand::TrafficGet(TrafficGetArgs {
            id: "REQ-1".to_string(),
            request_body: true,
            response_body: false,
            ..Default::default()
        });

        let json_str = query_args_json(&query).expect("traffic get args_json");
        let v: Value = serde_json::from_str(&json_str).expect("valid json");
        assert_eq!(v.get("id").and_then(|k| k.as_str()), Some("REQ-1"));
        assert_eq!(v.get("request_body").and_then(|k| k.as_bool()), Some(true));
        assert_eq!(
            v.get("response_body").and_then(|k| k.as_bool()),
            Some(false)
        );
    }

    #[test]
    fn query_args_json_handles_traffic_clear_variant() {
        let query = CanonicalQueryCommand::TrafficClear(TrafficClearArgs {
            ids: Some(vec!["REQ-1".to_string(), "REQ-2".to_string()]),
        });

        let json_str = query_args_json(&query).expect("traffic clear args_json");
        let v: Value = serde_json::from_str(&json_str).expect("valid json");
        let ids = v
            .get("ids")
            .and_then(|ids| ids.as_array())
            .expect("ids array");
        assert_eq!(ids.len(), 2);
    }

    // --- Remote search string helpers ---

    #[test]
    fn truncate_search_str_returns_original_for_short_input() {
        let s = "hello";
        assert_eq!(truncate_search_str(s, 10), "hello".to_string());
    }

    #[test]
    fn truncate_search_str_truncates_and_appends_ellipsis() {
        let s = "abcdef";
        let t = truncate_search_str(s, 4);
        assert!(t.starts_with("a"));
        assert!(t.ends_with("..."));
    }

    #[test]
    fn highlight_remote_search_keyword_no_color_or_empty_keyword_is_passthrough() {
        let s = "Hello World";
        assert_eq!(highlight_remote_search_keyword(s, "", true), s);
        assert_eq!(highlight_remote_search_keyword(s, "world", false), s);
    }

    #[test]
    fn highlight_remote_search_keyword_wraps_matches_in_escape_sequences() {
        let s = "hello hello";
        let highlighted = highlight_remote_search_keyword(s, "he", true);
        assert!(highlighted.contains("\x1b[1;33m"));
        assert!(highlighted.contains("\x1b[0m"));
        assert!(highlighted.to_lowercase().contains("he"));
    }

    // --- Remote search SSE decoder & renderer ---

    #[test]
    fn remote_search_sse_decoder_parses_result_progress_done_error() {
        let mut decoder = RemoteSearchSseDecoder::default();

        let result_payload = json!({
            "record": {
                "id": "REQ-1",
                "seq": 1u64,
                "ts": 123u64,
                "m": "GET",
                "h": "example.com",
                "p": "/",
                "s": 200u16,
                "ct": null,
                "req_ct": null,
                "req_sz": 10usize,
                "res_sz": 20usize,
                "dur": 30u64,
                "proto": "HTTP/1.1",
                "cip": "127.0.0.1",
                "capp": "curl",
                "flags": 0u32,
                "fc": 0usize,
                "st": "ok",
                "et": null,
            },
            "matches": [],
        });

        let progress_payload = json!({
            "total_searched": 10usize,
            "total_matched": 2usize,
            "next_cursor": null,
            "has_more_hint": false,
            "iterations": 1usize,
        });

        let done_payload = json!({
            "total_searched": 10usize,
            "total_matched": 2usize,
            "next_cursor": 5u64,
            "has_more": true,
            "search_id": "sid",
        });

        let error_payload = json!({ "message": "boom" });

        let sse = format!(
            "event: result\n\
             data: {}\n\
             \n\
             event: progress\n\
             data: {}\n\
             \n\
             event: done\n\
             data: {}\n\
             \n\
             event: error\n\
             data: {}\n\
             \n",
            result_payload, progress_payload, done_payload, error_payload,
        );

        let events = decoder
            .push_chunk(&sse)
            .expect("decoder should produce events");
        assert_eq!(events.len(), 4);
    }

    #[test]
    fn remote_search_sse_decoder_handles_partial_chunks() {
        let mut decoder = RemoteSearchSseDecoder::default();
        let progress_payload = json!({
            "total_searched": 1usize,
            "total_matched": 0usize,
            "next_cursor": null,
            "has_more_hint": false,
            "iterations": 0usize,
        })
        .to_string();

        let part1 = format!("event: progress\ndata: {}", progress_payload);
        let part2 = "\n\n";

        let ev1 = decoder.push_chunk(&part1).expect("no error");
        assert!(ev1.is_empty());
        let ev2 = decoder.push_chunk(part2).expect("no error");
        assert_eq!(ev2.len(), 1);
    }

    #[test]
    fn remote_search_renderer_table_accumulates_results_and_done_state() {
        let render_mode = RemoteRenderMode::Search {
            format: OutputFormat::Table,
            no_color: true,
            keyword: "needle".to_string(),
            max_scan: Some(50),
            max_results: Some(3),
        };
        let mut renderer = RemoteSearchRenderer::from_render_mode(&render_mode)
            .expect("renderer should be constructed");

        let item: SearchResultItem = serde_json::from_value(json!({
            "record": {
                "id": "REQ-1",
                "seq": 1u64,
                "ts": 123u64,
                "m": "GET",
                "h": "example.com",
                "p": "/v1",
                "s": 200u16,
                "ct": null,
                "req_ct": null,
                "req_sz": 10usize,
                "res_sz": 20usize,
                "dur": 30u64,
                "proto": "HTTP/1.1",
                "cip": "127.0.0.1",
                "capp": "curl",
                "flags": 0u32,
                "fc": 0usize,
                "st": "ok",
                "et": null,
            },
            "matches": [{
                "field": "host",
                "preview": "example.com",
                "offset": 0usize,
            }],
        }))
        .expect("decode search result");

        renderer
            .render_event(RemoteSearchEvent::Result(Box::new(item)))
            .expect("render result");

        renderer
            .render_event(RemoteSearchEvent::Progress(RemoteSearchProgressPayload {
                total_searched: 5,
                total_matched: 1,
                next_cursor: None,
                has_more_hint: false,
                iterations: 1,
            }))
            .expect("render progress");

        renderer
            .render_event(RemoteSearchEvent::Done(RemoteSearchDonePayload {
                total_searched: 10,
                total_matched: 2,
                next_cursor: Some(5),
                has_more: true,
                search_id: "sid".to_string(),
            }))
            .expect("render done");

        assert_eq!(renderer.total_searched, 10);
        assert_eq!(renderer.total_matched, 2);
        assert!(renderer.has_more);
    }

    #[test]
    fn remote_search_renderer_json_collects_results() {
        let render_mode = RemoteRenderMode::Search {
            format: OutputFormat::Json,
            no_color: true,
            keyword: String::new(),
            max_scan: None,
            max_results: None,
        };
        let mut renderer = RemoteSearchRenderer::from_render_mode(&render_mode)
            .expect("renderer should be constructed");

        let item: SearchResultItem = serde_json::from_value(json!({
            "record": {
                "id": "REQ-2",
                "seq": 2u64,
                "ts": 1u64,
                "m": "POST",
                "h": "example.com",
                "p": "/v2",
                "s": 201u16,
                "ct": null,
                "req_ct": null,
                "req_sz": 5usize,
                "res_sz": 15usize,
                "dur": 5u64,
                "proto": "HTTP/2",
                "cip": "127.0.0.1",
                "capp": null,
                "flags": 0u32,
                "fc": 0usize,
                "st": "ok",
                "et": null,
            },
            "matches": [],
        }))
        .expect("decode search result");

        renderer
            .render_event(RemoteSearchEvent::Result(Box::new(item)))
            .expect("render result");
        renderer
            .render_event(RemoteSearchEvent::Done(RemoteSearchDonePayload {
                total_searched: 1,
                total_matched: 1,
                next_cursor: None,
                has_more: false,
                search_id: "s".to_string(),
            }))
            .expect("render done");

        assert_eq!(renderer.json_results.len(), 1);
        assert_eq!(renderer.total_matched, 1);
        assert_eq!(renderer.total_searched, 1);
    }

    #[test]
    fn remote_search_renderer_from_render_mode_non_search_returns_none() {
        let render_mode = RemoteRenderMode::Raw;
        assert!(RemoteSearchRenderer::from_render_mode(&render_mode).is_none());
    }

    // --- Connection file helpers and resolver ---

    #[test]
    fn load_connections_missing_file_returns_empty_vec() {
        with_connections_lock(|| {
            init_test_data_dir();
            let path = connections_path();
            let _ = std::fs::remove_file(&path);
            let loaded = load_connections().expect("load should succeed");
            assert!(loaded.is_empty());
        });
    }

    #[test]
    fn save_and_load_connections_roundtrip() {
        with_connections_lock(|| {
            init_test_data_dir();
            let connections = vec![sample_local_connection("client-1", "https://relay-a")];
            save_connections(&connections).expect("save");
            let _loaded = load_connections().expect("load");
        });
    }

    #[test]
    fn load_remote_jobs_missing_file_returns_empty_vec() {
        with_connections_lock(|| {
            init_test_data_dir();
            let _ = std::fs::remove_file(remote_jobs_path());
            let loaded = load_remote_jobs().expect("load remote jobs");
            assert!(loaded.is_empty());
        });
    }

    #[test]
    fn remember_remote_job_persists_encrypted_token_and_resolves_by_call_id() {
        with_connections_lock(|| {
            init_test_data_dir();
            let _ = std::fs::remove_file(remote_jobs_path());
            remember_remote_job(sample_remote_job("call-1", "https://relay-a", "tok-1"))
                .expect("remember job");

            let jobs = load_remote_jobs().expect("load remote jobs");
            assert_eq!(jobs.len(), 1);
            assert_ne!(jobs[0].relay_token_encrypted, "tok-1");

            let (token, relay_url) = resolve_remote_job("call-1").expect("resolve job");
            assert_eq!(token, "tok-1");
            assert_eq!(relay_url, "https://relay-a");
        });
    }

    #[test]
    fn resolve_remote_job_without_cache_returns_actionable_error() {
        with_connections_lock(|| {
            init_test_data_dir();
            let _ = std::fs::remove_file(remote_jobs_path());
            let err = resolve_remote_job("missing-call").expect_err("resolve should fail");
            let msg = err.to_string();
            assert!(msg.contains("no local remote job cache"));
            assert!(msg.contains("bifrost remote job list"));
        });
    }

    #[test]
    fn update_remote_job_status_persists_exit_code() {
        with_connections_lock(|| {
            init_test_data_dir();
            let path = remote_jobs_path();
            let _ = std::fs::remove_file(&path);
            remember_remote_job_at(
                &path,
                sample_remote_job("call-2", "https://relay-a", "tok-2"),
            )
            .expect("remember job");

            update_remote_job_status_at(&path, "call-2", "exited", Some(7)).expect("update status");

            let jobs = load_remote_jobs_from_path(&path).expect("load remote jobs");
            let job = jobs
                .iter()
                .find(|job| job.call_id == "call-2")
                .expect("updated job");
            assert_eq!(job.status, "exited");
            assert_eq!(job.exit_code, Some(7));
            assert!(job.updated_at >= job.created_at);
        });
    }

    #[test]
    fn remove_stale_local_connection_persists_changes() {
        with_connections_lock(|| {
            init_test_data_dir();
            let mut connections = vec![
                sample_local_connection("client-1", "https://relay-a"),
                sample_local_connection("client-2", "https://relay-a"),
            ];
            save_connections(&connections).expect("save initial");
            let stale = connections[0].clone();
            let removed =
                remove_stale_local_connection(&mut connections, &stale).expect("remove stale");
            assert!(removed);
            assert_eq!(connections.len(), 1);
            assert_eq!(connections[0].client_instance_id, "client-2");
        });
    }

    #[test]
    fn resolve_local_connection_errors_when_no_saved_connections() {
        let err = resolve_local_connection(&[], None).expect_err("should fail");
        assert!(err
            .to_string()
            .contains("no saved connection, please run `bifrost remote conn up"));
    }

    #[test]
    fn resolve_local_connection_single_connection_without_client_id() {
        let conn = sample_local_connection("client-1", "https://relay-a");
        let resolved =
            resolve_local_connection(std::slice::from_ref(&conn), None).expect("resolve");
        assert_eq!(resolved.client_instance_id, conn.client_instance_id);
    }

    #[test]
    fn resolve_local_connection_explicit_full_client_id_matches() {
        let conn = sample_local_connection("client-full", "https://relay-a");
        let resolved = resolve_local_connection(std::slice::from_ref(&conn), Some("client-full"))
            .expect("resolve");
        assert_eq!(resolved.client_instance_id, "client-full");
    }

    #[test]
    fn resolve_local_connection_explicit_device_label_prefix_matches() {
        let mut conn = sample_local_connection("client-full", "https://relay-a");
        conn.device_name = "devbox-mac".to_string();
        let resolved =
            resolve_local_connection(std::slice::from_ref(&conn), Some("devbox")).expect("resolve");
        assert_eq!(resolved.client_instance_id, "client-full");
    }

    #[test]
    fn resolve_local_connection_multiple_noninteractive_lists_choices() {
        let conn_a = sample_local_connection("client-aaaaaaaa", "https://relay-a");
        let mut conn_b = sample_local_connection("client-bbbbbbbb", "https://relay-a");
        conn_b.device_name = "macbook".to_string();
        let err = resolve_local_connection_with_terminal(&[conn_a, conn_b], None, false)
            .expect_err("should fail");
        let msg = err.to_string();
        assert!(msg.contains("multiple saved connections"));
        assert!(msg.contains("--client-id client-aaaa"));
        assert!(msg.contains("--client-id client-bbbb"));
        assert!(msg.contains(DEFAULT_REMOTE_CLIENT_ID_ENV_VAR));
        assert!(msg.contains("bifrost remote --client-id"));
    }

    #[test]
    fn resolve_local_connection_explicit_prefix_with_no_match_errors() {
        let conn = sample_local_connection("client-1", "https://relay-a");
        let err =
            resolve_local_connection(&[conn], Some("does-not-exist")).expect_err("should fail");
        assert!(err.to_string().contains("no saved connection matching"));
    }

    #[test]
    fn idle_timeout_message_mentions_seconds_and_job_watch() {
        let msg = remote_call_idle_timeout_message(CALL_EVENT_TIMEOUT_SECS);
        assert!(msg.contains("300s"));
        assert!(msg.contains("idle timeout"));
        assert!(msg.contains("remote exec --detach"));
        assert!(msg.contains("remote job watch"));
    }

    // --- Shell scope upgrade error variants ---

    #[test]
    fn shell_scope_upgrade_error_for_ssh_key_mentions_ssh_prompt() {
        let mut conn = sample_local_connection("client-1", "https://relay-a");
        conn.auth_method = Some("ssh_publickey".to_string());
        conn.ssh_key_source = Some("env".to_string());
        conn.device_code = Some("BF-0123".to_string());

        let msg = shell_scope_upgrade_error(&conn).to_string();
        assert!(msg.contains("does not allow shell.exec"));
        assert!(msg.contains("--level shell"));
    }

    #[test]
    fn shell_scope_upgrade_error_readonly_suggests_pair_code() {
        let conn = sample_local_connection("client-1", "https://relay-a");
        let msg = shell_scope_upgrade_error(&conn).to_string();
        assert!(msg.contains("read-only"));
        assert!(msg.contains("bifrost remote conn up <pair-code>"));
    }

    // --- Env SSH key normalization and key parsing helpers ---

    #[test]
    fn normalize_env_ssh_key_value_passthrough_for_real_newlines() {
        let input = format!("{BIFROST_KEY_BEGIN}\nline1\n{BIFROST_KEY_END}\n");
        let normalized = normalize_env_ssh_key_value(input.clone());
        assert_eq!(normalized, input);
    }

    #[test]
    fn normalize_env_ssh_key_value_unescapes_backslash_n_sequences() {
        let encoded = format!("{BIFROST_KEY_BEGIN}\\nline1\\n{BIFROST_KEY_END}\\n");
        let normalized = normalize_env_ssh_key_value(encoded);
        assert!(normalized.contains('\n'));
        assert!(!normalized.contains("\\n"));
    }

    #[test]
    fn parse_bifrost_key_file_returns_none_for_non_bifrost_file() {
        let text = "no magic header here";
        let parsed = parse_bifrost_key_file(text).expect("parse ok");
        assert!(parsed.is_none());
    }

    #[test]
    fn parse_bifrost_key_file_requires_device_code_header() {
        let pkcs8_b64 = base64::engine::general_purpose::STANDARD.encode(b"dummy-pkcs8-data");
        let text = format!("{BIFROST_KEY_BEGIN}\n{}\n{BIFROST_KEY_END}\n", pkcs8_b64);
        let err = parse_bifrost_key_file(&text).expect_err("missing Device-Code should error");
        assert!(err
            .to_string()
            .contains("Bifrost SSH key file is missing `Device-Code:` header"));
    }

    #[test]
    fn parse_pem_block_decodes_base64_body() {
        let pkcs8_bytes = vec![1u8, 2, 3, 4];
        let encoded = base64::engine::general_purpose::STANDARD.encode(&pkcs8_bytes);
        let text = format!("{PKCS8_KEY_BEGIN}\n{}\n{PKCS8_KEY_END}\n", encoded);
        let decoded = parse_pem_block(&text, PKCS8_KEY_BEGIN, PKCS8_KEY_END)
            .expect("no error")
            .expect("pem block found");
        assert_eq!(decoded, pkcs8_bytes);
    }

    #[test]
    fn parse_pem_block_missing_markers_returns_none() {
        let text = "-----BEGIN OTHER-----\nABC\n-----END OTHER-----";
        let decoded = parse_pem_block(text, PKCS8_KEY_BEGIN, PKCS8_KEY_END).expect("no error");
        assert!(decoded.is_none());
    }

    #[test]
    fn parse_ssh_private_key_bytes_binary_passes_through() {
        let raw = [0u8, 1, 2, 3];
        let (pkcs8, device_code) = parse_ssh_private_key_bytes(&raw).expect("parse");
        assert_eq!(pkcs8, raw);
        assert!(device_code.is_none());
    }

    #[test]
    fn parse_ssh_private_key_bytes_prefers_bifrost_format() {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).expect("pkcs8");
        let rendered = format!(
            "{BIFROST_KEY_BEGIN}\nDevice-Code: BF-0000000000000000\n{}\n{BIFROST_KEY_END}\n",
            base64::engine::general_purpose::STANDARD.encode(pkcs8.as_ref())
        );
        let (parsed_pkcs8, device_code) =
            parse_ssh_private_key_bytes(rendered.as_bytes()).expect("parse");
        assert_eq!(parsed_pkcs8, pkcs8.as_ref());
        assert_eq!(device_code.as_deref(), Some("BF-0000000000000000"));
    }

    #[test]
    fn load_ssh_key_respects_device_code_override() {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).expect("pkcs8");
        let rendered = format!(
            "{BIFROST_KEY_BEGIN}\nDevice-Code: BF-AAAAAAAAAAAAAAAA\n{}\n{BIFROST_KEY_END}\n",
            base64::engine::general_purpose::STANDARD.encode(pkcs8.as_ref())
        );

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("key");
        std::fs::write(&path, rendered).expect("write key");

        let loaded =
            load_ssh_key(path.to_str().unwrap(), Some("BF-OVERRIDE-1234")).expect("load ssh key");
        assert_eq!(loaded.device_code, "BF-OVERRIDE-1234");
    }

    #[test]
    fn read_ssh_key_source_reads_from_file_spec() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("k");
        std::fs::write(&path, "key-content").expect("write key");

        let (raw, label, file_path) = read_ssh_key_source(path.to_str().unwrap()).expect("read");
        assert_eq!(raw, b"key-content");
        assert!(label.contains("k"));
        assert_eq!(file_path.as_deref(), Some(path.as_path()));
    }

    // --- Encrypted frame and exit payload helpers ---

    #[test]
    fn encrypt_caller_input_frame_roundtrip_decrypts_payload() {
        let transport = OpenCallTransportContext {
            caller_ephemeral_pub: "caller-epk".to_string(),
            client_ephemeral_pub: "client-epk".to_string(),
            shared_secret: b"01234567890123456789012345678901".to_vec(),
        };
        let call_id = "call-1";
        let seq = 5u64;
        let plaintext = b"hello-input";

        let envelope_json = encrypt_caller_input_frame(&transport, call_id, seq, plaintext)
            .expect("encrypt input frame");
        let envelope: Value = serde_json::from_str(&envelope_json).expect("parse envelope");

        let payload = EncryptedPayload {
            version: envelope
                .get("version")
                .and_then(|v| v.as_u64())
                .unwrap_or(ENCRYPTED_OPEN_CALL_VERSION as u64) as u32,
            nonce: envelope
                .get("nonce")
                .and_then(|v| v.as_str())
                .unwrap()
                .to_string(),
            ciphertext: envelope
                .get("ciphertext")
                .and_then(|v| v.as_str())
                .unwrap()
                .to_string(),
            tag: envelope
                .get("tag")
                .and_then(|v| v.as_str())
                .unwrap()
                .to_string(),
            aad: None,
        };

        let key_bytes = derive_call_event_key(
            &transport.shared_secret,
            call_id,
            &transport.caller_ephemeral_pub,
            &transport.client_ephemeral_pub,
        )
        .expect("derive key");
        let plaintext_bytes =
            decrypt_payload_bytes(&payload, &key_bytes, None).expect("decrypt payload");
        let frame: CallerInputFramePayload =
            serde_json::from_slice(&plaintext_bytes).expect("decode frame");
        assert_eq!(
            frame.data_b64,
            base64::engine::general_purpose::STANDARD.encode(plaintext)
        );
    }

    #[test]
    fn decrypt_exit_payload_with_aad_roundtrip() {
        let transport = OpenCallTransportContext {
            caller_ephemeral_pub: "caller-epk".to_string(),
            client_ephemeral_pub: "client-epk".to_string(),
            shared_secret: b"01234567890123456789012345678901".to_vec(),
        };
        let call_id = "call-exit";
        let exit = EncryptedExitPayload {
            exit_code: 7,
            duration_ms: Some(42),
            stderr: Some("boom".to_string()),
            stdout_digest: None,
            stderr_digest: None,
        };

        let aad = FrameEnvelopeAad {
            version: ENCRYPTED_OPEN_CALL_VERSION,
            call_id: call_id.to_string(),
            seq: 1,
            direction: "client_to_caller".to_string(),
            frame_type: Some("exit".to_string()),
            command_kind: None,
        };
        let aad_bytes = serde_json::to_vec(&aad).expect("encode aad");

        let key_bytes = derive_call_event_key(
            &transport.shared_secret,
            call_id,
            &transport.caller_ephemeral_pub,
            &transport.client_ephemeral_pub,
        )
        .expect("derive key");
        let unbound =
            UnboundKey::new(&CHACHA20_POLY1305, &key_bytes).expect("build encryption key");
        let key = LessSafeKey::new(unbound);
        let mut nonce = [0u8; NONCE_LEN];
        SystemRandom::new().fill(&mut nonce).expect("nonce");
        let mut sealed = serde_json::to_vec(&exit).expect("encode exit");
        key.seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(&aad_bytes[..]),
            &mut sealed,
        )
        .expect("seal exit");
        let tag = sealed.split_off(sealed.len().saturating_sub(16));
        let payload = EncryptedPayload {
            version: ENCRYPTED_OPEN_CALL_VERSION,
            nonce: base64::engine::general_purpose::STANDARD.encode(nonce),
            ciphertext: base64::engine::general_purpose::STANDARD.encode(&sealed),
            tag: base64::engine::general_purpose::STANDARD.encode(tag),
            aad: Some(aad),
        };

        let decrypted =
            decrypt_exit_payload(&transport, call_id, &payload).expect("decrypt exit payload");
        assert_eq!(decrypted.exit_code, 7);
        assert_eq!(decrypted.duration_ms, Some(42));
        assert_eq!(decrypted.stderr.as_deref(), Some("boom"));
    }

    // --- Hostname / username / os version helpers ---

    #[test]
    fn get_hostname_returns_non_empty_string() {
        let hostname = get_hostname();
        assert!(!hostname.is_empty());
    }

    #[test]
    fn get_username_returns_non_empty_string() {
        let username = get_username();
        assert!(!username.is_empty());
    }

    #[test]
    fn get_os_version_smoke_test() {
        let version = get_os_version();
        if let Some(v) = version {
            assert!(!v.is_empty());
        }
    }

    // --- HTTP helpers using wiremock ---

    fn test_caller_identity() -> CallerPopIdentity {
        let rng = SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).expect("generate pkcs8");
        let key_pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).expect("key pair");
        CallerPopIdentity {
            caller_fingerprint: "caller-test".to_string(),
            key_pair,
        }
    }

    fn test_x25519_pub_b64() -> String {
        let rng = SystemRandom::new();
        let private_key =
            agreement::EphemeralPrivateKey::generate(&X25519, &rng).expect("x25519 private");
        base64::engine::general_purpose::STANDARD.encode(
            private_key
                .compute_public_key()
                .expect("x25519 public")
                .as_ref(),
        )
    }

    #[tokio::test]
    async fn caller_delete_grant_treats_2xx_as_deleted() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v5/remote-invoke/grants/revoke"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&mock_server)
            .await;

        let client = CallerRelayClient::new(&mock_server.uri());
        init_test_data_dir();
        let conn = sample_authorized_connection("client-1", &mock_server.uri());
        let identity = test_caller_identity();
        let outcome = client
            .delete_grant(&conn, &identity)
            .await
            .expect("delete should succeed");
        assert_eq!(outcome, DeleteGrantOutcome::Deleted);
    }

    #[tokio::test]
    async fn caller_delete_grant_treats_404_grant_not_found_as_already_missing() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v5/remote-invoke/grants/revoke"))
            .respond_with(ResponseTemplate::new(404).set_body_raw(
                "{\"code\":404,\"message\":\"grant_not_found\"}",
                "application/json",
            ))
            .mount(&mock_server)
            .await;

        let client = CallerRelayClient::new(&mock_server.uri());
        init_test_data_dir();
        let conn = sample_authorized_connection("client-1", &mock_server.uri());
        let identity = test_caller_identity();
        let outcome = client
            .delete_grant(&conn, &identity)
            .await
            .expect("delete should succeed");
        assert_eq!(outcome, DeleteGrantOutcome::AlreadyMissing);
    }

    #[tokio::test]
    async fn find_reusable_grant_returns_none_for_null_data() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v5/remote-invoke/grants/lookup"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "message": null,
                "data": null
            })))
            .mount(&mock_server)
            .await;

        let client = CallerRelayClient::new(&mock_server.uri());
        let identity = test_caller_identity();
        let grant = client
            .find_reusable_grant("client-1", &identity, None)
            .await
            .expect("request should succeed");
        assert!(grant.is_none());
    }

    #[tokio::test]
    async fn find_reusable_grant_returns_some_for_valid_grant() {
        let mock_server = MockServer::start().await;
        let client_ephemeral_pub = test_x25519_pub_b64();
        Mock::given(method("POST"))
            .and(path("/v5/remote-invoke/grants/lookup"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "message": null,
                "data": {
                    "grant_session_token": "session-token",
                    "expires_at": "2099-01-01T00:00:00Z",
                    "grant_summary": {
                        "scope": "remote_shell_exec",
                        "mode": "permanent",
                        "file_access": "read_write",
                        "client_ephemeral_pub": client_ephemeral_pub
                    }
                }
            })))
            .mount(&mock_server)
            .await;

        let client = CallerRelayClient::new(&mock_server.uri());
        let identity = test_caller_identity();
        let grant = client
            .find_reusable_grant("client-1", &identity, None)
            .await
            .expect("request should succeed")
            .expect("grant should be present");
        assert_eq!(grant.grant_session_token.as_deref(), Some("session-token"));
        assert_eq!(grant.status, "active");
        assert!(grant.shared_secret_encrypted.is_some());
    }

    #[tokio::test]
    async fn find_reusable_grant_reuses_saved_transport_for_session_refresh() {
        let mock_server = MockServer::start().await;
        let saved_conn = sample_authorized_connection("client-1", &mock_server.uri());
        let saved_caller_ephemeral_pub = saved_conn.caller_ephemeral_pub.clone().unwrap();
        let saved_client_ephemeral_pub = saved_conn.client_ephemeral_pub.clone().unwrap();
        let saved_secret = saved_conn.shared_secret_encrypted.clone().unwrap();

        Mock::given(method("POST"))
            .and(path("/v5/remote-invoke/grants/lookup"))
            .and(body_partial_json(serde_json::json!({
                "caller_ephemeral_pub": saved_caller_ephemeral_pub
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "message": null,
                "data": {
                    "grant_session_token": "session-refreshed",
                    "expires_at": "2099-01-01T00:00:00Z",
                    "grant_summary": {
                        "scope": "remote_shell_exec",
                        "mode": "permanent",
                        "file_access": "read_write",
                        "client_ephemeral_pub": saved_client_ephemeral_pub
                    }
                }
            })))
            .mount(&mock_server)
            .await;

        let client = CallerRelayClient::new(&mock_server.uri());
        let identity = test_caller_identity();
        let grant = client
            .find_reusable_grant("client-1", &identity, Some(&saved_conn))
            .await
            .expect("request should succeed")
            .expect("grant should be present");

        assert_eq!(
            grant.grant_session_token.as_deref(),
            Some("session-refreshed")
        );
        assert_eq!(
            grant.caller_ephemeral_pub.as_deref(),
            saved_conn.caller_ephemeral_pub.as_deref()
        );
        assert_eq!(
            grant.client_ephemeral_pub.as_deref(),
            saved_conn.client_ephemeral_pub.as_deref()
        );
        assert_eq!(
            grant.shared_secret_encrypted.as_deref(),
            Some(saved_secret.as_str())
        );
    }

    #[tokio::test]
    async fn start_pairing_nonzero_code_maps_to_error() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v5/remote-invoke/pairings/start"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 123,
                "message": "bad things",
                "data": null
            })))
            .mount(&mock_server)
            .await;

        let client = CallerRelayClient::new(&mock_server.uri());
        let req = StartPairingRequest {
            pair_code: "CODE".to_string(),
            caller_info: CallerInfo {
                fingerprint: "fp-1".to_string(),
                caller_pubkey: None,
                display_name: None,
                user_agent: None,
                platform: None,
                hostname: None,
                username: None,
                label: None,
                os_version: None,
                arch: None,
            },
            caller_pubkey: "pubkey".to_string(),
            caller_ephemeral_pub: "epk".to_string(),
        };

        let err = client.start_pairing(&req).await.expect_err("should fail");
        let msg = err.to_string();
        assert!(msg.contains("start_pairing error code 123"));
    }

    #[tokio::test]
    async fn subscribe_call_events_processes_frame_and_exit_plaintext() {
        let mock_server = MockServer::start().await;
        let call_id = "call-1";
        let exit_json = json!({
            "exit_code": 7,
            "duration_ms": 42u64,
            "stderr": "boom",
        });

        let envelope = json!({
            "version": ENCRYPTED_OPEN_CALL_VERSION,
            "nonce": "",
            "ciphertext": "hello-chunk",
            "tag": "",
        });
        let frame_data = json!({
            "envelope_json": envelope.to_string(),
        })
        .to_string();

        let sse_body = format!(
            "event: frame\n\
             data: {}\n\
             \n\
             event: exit\n\
             data: {}\n\
             \n",
            frame_data, exit_json,
        );

        Mock::given(method("GET"))
            .and(path(format!("/v4/remote-invoke/calls/{call_id}/events")))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse_body, "text/event-stream"))
            .mount(&mock_server)
            .await;

        let client = CallerRelayClient::new(&mock_server.uri());
        let transport = OpenCallTransportContext {
            caller_ephemeral_pub: "caller-epk".to_string(),
            client_ephemeral_pub: "client-epk".to_string(),
            shared_secret: Vec::new(),
        };

        let result = client
            .subscribe_call_events(
                call_id,
                "relay-token",
                &transport,
                &RemoteRenderMode::Raw,
                5,
            )
            .await
            .expect("subscribe should succeed");

        assert_eq!(result.exit_code, 7);
        assert_eq!(result.duration_ms, Some(42));
        assert_eq!(result.stderr.as_deref(), Some("boom"));
        assert!(result.stdout.is_none());
        assert!(!result.cancelled);
    }

    #[tokio::test]
    async fn subscribe_call_events_status_cancelled_sets_cancelled_result() {
        let mock_server = MockServer::start().await;
        let call_id = "call-cancel";
        let status_payload = json!({
            "call_id": call_id,
            "status": "cancelled",
        })
        .to_string();

        let sse_body = format!(
            "event: status\n\
             data: {}\n\
             \n",
            status_payload,
        );

        Mock::given(method("GET"))
            .and(path(format!("/v4/remote-invoke/calls/{call_id}/events")))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse_body, "text/event-stream"))
            .mount(&mock_server)
            .await;

        let client = CallerRelayClient::new(&mock_server.uri());
        let transport = OpenCallTransportContext {
            caller_ephemeral_pub: "caller-epk".to_string(),
            client_ephemeral_pub: "client-epk".to_string(),
            shared_secret: Vec::new(),
        };

        let result = client
            .subscribe_call_events(
                call_id,
                "relay-token",
                &transport,
                &RemoteRenderMode::TrafficList {
                    format: OutputFormat::Compact,
                    no_color: true,
                },
                5,
            )
            .await
            .expect("subscribe should succeed");

        assert_eq!(result.exit_code, 130);
        assert!(result.cancelled);
        assert_eq!(
            result.stderr.as_deref(),
            Some("remote call cancelled by caller")
        );
    }
}

#[cfg(test)]
mod coverage_boost_v2 {
    use super::*;
    use crate::cli::RemoteSearchArgs;
    use crate::commands::search::SearchResultItem;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn base_search_args() -> RemoteSearchArgs {
        RemoteSearchArgs {
            keyword: Some("needle".to_string()),
            limit: 50,
            format: "table".to_string(),
            no_color: false,
            url: false,
            headers: false,
            body: false,
            req_header: false,
            res_header: false,
            req_body: false,
            res_body: false,
            status: None,
            method: None,
            host: None,
            path: None,
            listener_port: None,
            protocol: None,
            content_type: None,
            domain: None,
            max_scan: Some(100),
            max_results: Some(25),
            req_json: Vec::new(),
            res_json: Vec::new(),
            req_header_eq: Vec::new(),
            res_header_eq: Vec::new(),
            since: None,
            until: None,
            latest: None,
        }
    }

    fn sample_search_result_item() -> SearchResultItem {
        serde_json::from_value(json!({
            "record": {
                "id": "REQ-1",
                "seq": 1u64,
                "ts": 123u64,
                "m": "GET",
                "h": "example.com",
                "p": "/v1",
                "s": 200u16,
                "ct": null,
                "req_ct": null,
                "req_sz": 10usize,
                "res_sz": 20usize,
                "dur": 30u64,
                "proto": "HTTP/1.1",
                "cip": "127.0.0.1",
                "capp": "curl",
                "flags": 0u32,
                "fc": 0usize,
                "st": "ok",
                "et": null,
            },
            "matches": [{
                "field": "host",
                "preview": "example.com",
                "offset": 0usize,
            }],
        }))
        .expect("search result item")
    }

    #[test]
    fn command_search_args_all_scope_when_no_location_flags() {
        let args = base_search_args();
        let search = command_search_args(&args);
        assert_eq!(search.keyword, "needle");
        assert!(search.scope.all);
        assert!(!search.scope.url);
        assert!(!search.scope.request_body);
        assert!(!search.scope.response_body);
        assert!(!search.scope.request_headers);
        assert!(!search.scope.response_headers);
    }

    #[test]
    fn command_search_args_url_scope_disables_all() {
        let mut args = base_search_args();
        args.url = true;
        let search = command_search_args(&args);
        assert!(!search.scope.all);
        assert!(search.scope.url);
    }

    #[test]
    fn command_search_args_builds_filters_for_all_supported_fields() {
        let mut args = base_search_args();
        args.status = Some("4xx".to_string());
        args.protocol = Some("HTTPS".to_string());
        args.content_type = Some("json".to_string());
        args.domain = Some("example.com".to_string());
        args.method = Some("POST".to_string());
        args.host = Some("api.example.com".to_string());
        args.path = Some("/v1".to_string());
        args.listener_port = Some(18080);

        let search = command_search_args(&args);
        assert_eq!(search.filters.status_ranges, vec!["4xx"]);
        assert_eq!(search.filters.protocols, vec!["https"]);
        assert_eq!(search.filters.content_types, vec!["json"]);
        assert_eq!(search.filters.domains, vec!["example.com"]);
        assert!(search
            .filters
            .conditions
            .iter()
            .any(|c| c.field == "method" && c.operator == "equals" && c.value == "POST"));
        assert!(search.filters.conditions.iter().any(|c| c.field == "host"
            && c.operator == "contains"
            && c.value == "api.example.com"));
        assert!(search
            .filters
            .conditions
            .iter()
            .any(|c| c.field == "path" && c.operator == "contains" && c.value == "/v1"));
        assert!(search
            .filters
            .conditions
            .iter()
            .any(|c| c.field == "listener_port" && c.operator == "equals" && c.value == "18080"));
    }

    #[test]
    fn render_search_size_formats_bytes_kb_and_mb() {
        assert_eq!(render_search_size(512), "512B");
        assert_eq!(render_search_size(2048), "2.0KB");
        assert_eq!(render_search_size(3 * 1024 * 1024), "3.0MB");
    }

    #[test]
    fn render_search_duration_formats_zero_and_seconds() {
        assert_eq!(render_search_duration(0), "...");
        assert_eq!(render_search_duration(250), "250ms");
        assert_eq!(render_search_duration(1500), "1.50s");
    }

    #[test]
    fn render_search_number_formats_thousands_and_millions() {
        assert_eq!(render_search_number(999), "999");
        assert_eq!(render_search_number(2_000), "2.0K");
        assert_eq!(render_search_number(2_000_000), "2.0M");
    }

    #[test]
    fn print_remote_search_table_row_runs_without_color() {
        let item = sample_search_result_item();
        print_remote_search_table_row(&item, "example", false);
    }

    #[test]
    fn print_remote_search_table_row_runs_with_color() {
        let item = sample_search_result_item();
        print_remote_search_table_row(&item, "example", true);
    }

    #[test]
    fn print_remote_search_summary_plain_with_matches_and_more_flag() {
        print_remote_search_summary("needle", Some(200), Some(10), 500, 5, true, false);
    }

    #[test]
    fn print_remote_search_summary_color_without_matches() {
        print_remote_search_summary("needle", Some(50), Some(5), 100, 0, false, true);
    }

    #[test]
    fn should_stream_remote_render_only_for_raw_mode() {
        assert!(should_stream_remote_render(&RemoteRenderMode::Raw));
        let render = RemoteRenderMode::TrafficGet {
            format: OutputFormat::JsonPretty,
            no_color: false,
        };
        assert!(!should_stream_remote_render(&render));
    }

    #[test]
    fn remote_search_renderer_handles_error_in_json_mode() {
        let render_mode = RemoteRenderMode::Search {
            format: OutputFormat::JsonPretty,
            no_color: true,
            keyword: String::new(),
            max_scan: None,
            max_results: None,
        };
        let mut renderer = RemoteSearchRenderer::from_render_mode(&render_mode)
            .expect("renderer should construct");
        renderer
            .render_event(RemoteSearchEvent::Error(RemoteSearchErrorPayload {
                message: "boom".to_string(),
            }))
            .expect("error event should be handled");
    }

    #[test]
    fn remote_search_renderer_handles_error_in_table_mode() {
        let render_mode = RemoteRenderMode::Search {
            format: OutputFormat::Table,
            no_color: true,
            keyword: "k".to_string(),
            max_scan: Some(10),
            max_results: Some(5),
        };
        let mut renderer = RemoteSearchRenderer::from_render_mode(&render_mode)
            .expect("renderer should construct");
        renderer
            .render_event(RemoteSearchEvent::Error(RemoteSearchErrorPayload {
                message: "boom".to_string(),
            }))
            .expect("error event should be handled");
    }

    #[tokio::test]
    async fn watch_pairing_returns_approved_result() {
        let mock_server = MockServer::start().await;
        let body = "event: approved\n\
                    data: {\"type\":\"approved\",\"claim_token\":\"claim-1\",\"claim_expires_at\":\"2099-01-01T00:00:00Z\",\"grant_summary\":{\"scope\":\"remote_shell_exec\",\"mode\":\"permanent\",\"file_access\":\"read_write\"}}\n\n";
        Mock::given(method("GET"))
            .and(path("/v5/remote-invoke/pairings/p1/watch"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
            .mount(&mock_server)
            .await;

        let client = CallerRelayClient::new(&mock_server.uri());
        let result = client
            .watch_pairing("p1", "watch-token")
            .await
            .expect("watch_pairing");
        assert_eq!(result.status, "approved");
        assert_eq!(result.claim_token.as_deref(), Some("claim-1"));
        assert_eq!(
            result
                .grant_summary
                .as_ref()
                .and_then(|s| s.mode.as_deref()),
            Some("permanent")
        );
    }

    #[tokio::test]
    async fn watch_pairing_non_success_status_maps_to_error() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v5/remote-invoke/pairings/p1/watch"))
            .respond_with(ResponseTemplate::new(500).set_body_string("oops"))
            .mount(&mock_server)
            .await;

        let client = CallerRelayClient::new(&mock_server.uri());
        let err = client
            .watch_pairing("p1", "watch-token")
            .await
            .expect_err("should fail");
        assert!(err.to_string().contains("watch pairing returned"));
    }

    #[tokio::test]
    async fn request_ssh_challenge_parses_success() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v4/remote-invoke/ssh/challenge"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "message": null,
                "data": {
                    "challenge_id": "cid",
                    "challenge": "ch",
                    "expires_at": 123u64
                }
            })))
            .mount(&mock_server)
            .await;

        let client = CallerRelayClient::new(&mock_server.uri());
        let resp = client
            .request_ssh_challenge("BF-0001")
            .await
            .expect("ssh challenge");
        assert_eq!(resp.challenge_id, "cid");
        assert_eq!(resp.challenge, "ch");
        assert_eq!(resp.expires_at, Some(123));
    }

    #[tokio::test]
    async fn request_ssh_challenge_non_zero_code_maps_to_error() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v4/remote-invoke/ssh/challenge"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 42,
                "message": "bad",
                "data": null
            })))
            .mount(&mock_server)
            .await;

        let client = CallerRelayClient::new(&mock_server.uri());
        let err = client
            .request_ssh_challenge("BF-0001")
            .await
            .expect_err("should fail");
        assert!(err.to_string().contains("ssh_challenge error code"));
    }

    #[tokio::test]
    async fn ssh_connect_parses_success() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v4/remote-invoke/ssh/connect"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "message": null,
                "data": {
                    "connect_id": "cid",
                    "relay_token": "tok",
                    "expires_at": 42u64
                }
            })))
            .mount(&mock_server)
            .await;

        let client = CallerRelayClient::new(&mock_server.uri());
        let req = SshConnectRequest {
            device_code: "BF-1".to_string(),
            challenge_id: "ch".to_string(),
            signature: "sig".to_string(),
            timestamp: 1,
            caller_info: None,
            caller_ephemeral_pub: None,
        };
        let resp = client.ssh_connect(&req).await.expect("ssh_connect");
        assert_eq!(resp.connect_id, "cid");
        assert_eq!(resp.relay_token, "tok");
        assert_eq!(resp.expires_at, Some(42));
    }

    #[tokio::test]
    async fn ssh_connect_non_zero_code_maps_to_error() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v4/remote-invoke/ssh/connect"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 7,
                "message": "denied",
                "data": null
            })))
            .mount(&mock_server)
            .await;

        let client = CallerRelayClient::new(&mock_server.uri());
        let req = SshConnectRequest {
            device_code: "BF-1".to_string(),
            challenge_id: "ch".to_string(),
            signature: "sig".to_string(),
            timestamp: 1,
            caller_info: None,
            caller_ephemeral_pub: None,
        };
        let err = client.ssh_connect(&req).await.expect_err("should fail");
        assert!(err.to_string().contains("ssh_connect error code"));
    }

    #[tokio::test]
    async fn cancel_call_non_success_status_maps_to_error() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v4/remote-invoke/calls/abc/cancel"))
            .respond_with(
                ResponseTemplate::new(500).set_body_string("{\"code\":1,\"message\":\"bad\"}"),
            )
            .mount(&mock_server)
            .await;

        let client = CallerRelayClient::new(&mock_server.uri());
        let err = client
            .cancel_call("abc", "tok")
            .await
            .expect_err("should fail");
        assert!(err.to_string().contains("cancel_call failed with status"));
    }

    #[tokio::test]
    async fn post_call_input_successful_request() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v4/remote-invoke/calls/abc/input"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "code": 0,
                "message": null,
                "data": null
            })))
            .mount(&mock_server)
            .await;

        let client = CallerRelayClient::new(&mock_server.uri());
        client
            .post_call_input("abc", "tok", "{}".to_string())
            .await
            .expect("post_call_input");
    }

    #[tokio::test]
    async fn subscribe_call_events_non_success_status_maps_to_error() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v4/remote-invoke/calls/call-err/events"))
            .respond_with(ResponseTemplate::new(500).set_body_string("failure"))
            .mount(&mock_server)
            .await;

        let client = CallerRelayClient::new(&mock_server.uri());
        let transport = OpenCallTransportContext {
            caller_ephemeral_pub: "caller".to_string(),
            client_ephemeral_pub: "client".to_string(),
            shared_secret: Vec::new(),
        };
        let err = client
            .subscribe_call_events("call-err", "tok", &transport, &RemoteRenderMode::Raw, 1)
            .await
            .expect_err("should fail");
        assert!(err.to_string().contains("call events returned"));
    }
}

#[cfg(test)]
mod coverage_boost_v3 {
    use super::*;
    use serde_json::Value;
    use tempfile::NamedTempFile;

    fn build_file_command(action: RemoteFileCommands) -> BuiltRemoteCommand {
        build_remote_file_command(&action).expect("build file command")
    }

    fn args_json(built: &BuiltRemoteCommand) -> Value {
        let args = built
            .args_json
            .as_deref()
            .expect("file command must have args_json");
        serde_json::from_str(args).expect("args_json must be valid JSON")
    }

    #[test]
    fn file_read_command_populates_all_fields() {
        let cmd = RemoteFileCommands::Read {
            path: "foo.txt".to_string(),
            max_bytes: Some(4096),
            allow_binary: true,
            offset: Some(10),
            limit: Some(5),
            cwd: Some("/tmp".to_string()),
            output: "human".to_string(),
        };

        let built = build_file_command(cmd);
        assert_eq!(built.kind, CommandKind::File);
        assert_eq!(built.command.as_deref(), Some("file.read"));

        let v = args_json(&built);
        assert_eq!(v["path"], "foo.txt");
        assert_eq!(v["max_bytes"].as_u64(), Some(4096));
        assert_eq!(v["allow_binary"].as_bool(), Some(true));
        assert_eq!(v["offset"].as_u64(), Some(10));
        assert_eq!(v["limit"].as_u64(), Some(5));
        assert_eq!(v["cwd"].as_str(), Some("/tmp"));
    }

    #[test]
    fn file_list_respect_gitignore_and_exclude_patterns() {
        let cmd = RemoteFileCommands::List {
            path: None,
            depth: 3,
            max_matches: None,
            cursor: None,
            no_ignore: false,
            exclude_patterns: Vec::new(),
            cwd: Some("/work".to_string()),
            output: "human".to_string(),
        };

        let built = build_file_command(cmd);
        assert_eq!(built.command.as_deref(), Some("file.list"));
        let v = args_json(&built);
        assert_eq!(v["path"], serde_json::Value::Null);
        assert_eq!(v["depth"].as_u64(), Some(3));
        assert_eq!(v["respect_gitignore"].as_bool(), Some(true));
        assert!(v.get("exclude_patterns").unwrap().is_null());

        let cmd2 = RemoteFileCommands::List {
            path: Some(".".to_string()),
            depth: 1,
            max_matches: None,
            cursor: None,
            no_ignore: true,
            exclude_patterns: vec!["*.log".to_string()],
            cwd: None,
            output: "human".to_string(),
        };
        let built2 = build_file_command(cmd2);
        let v2 = args_json(&built2);
        assert_eq!(v2["respect_gitignore"].as_bool(), Some(false));
        let ex = v2["exclude_patterns"].as_array().expect("array");
        assert_eq!(ex.len(), 1);
        assert_eq!(ex[0], "*.log");
    }

    #[test]
    fn file_stat_and_hash_commands_encode_paths() {
        let stat_cmd = RemoteFileCommands::Stat {
            path: "foo.txt".to_string(),
            cwd: Some("/tmp".to_string()),
            output: "human".to_string(),
        };
        let stat_built = build_file_command(stat_cmd);
        assert_eq!(stat_built.command.as_deref(), Some("file.stat"));
        let sv = args_json(&stat_built);
        assert_eq!(sv["path"], "foo.txt");
        assert_eq!(sv["cwd"].as_str(), Some("/tmp"));

        let hash_cmd = RemoteFileCommands::Hash {
            path: "foo.txt".to_string(),
            algo: "sha256".to_string(),
            cwd: None,
            output: "human".to_string(),
        };
        let hash_built = build_file_command(hash_cmd);
        assert_eq!(hash_built.command.as_deref(), Some("file.hash"));
        let hv = args_json(&hash_built);
        assert_eq!(hv["path"], "foo.txt");
        assert_eq!(hv["algo"], "sha256");
    }

    #[test]
    fn file_glob_and_find_include_search_options() {
        let glob_cmd = RemoteFileCommands::Glob {
            pattern: "src/**/*.rs".to_string(),
            max_matches: Some(50),
            cursor: None,
            no_ignore: false,
            exclude_patterns: vec!["target/**".to_string()],
            cwd: Some("/repo".to_string()),
            output: "human".to_string(),
        };
        let glob_built = build_file_command(glob_cmd);
        assert_eq!(glob_built.command.as_deref(), Some("file.glob"));
        let gv = args_json(&glob_built);
        assert_eq!(gv["pattern"], "src/**/*.rs");
        assert_eq!(gv["max_matches"].as_u64(), Some(50));
        assert_eq!(gv["respect_gitignore"].as_bool(), Some(true));

        let find_cmd = RemoteFileCommands::Find {
            pattern: Some("needle".to_string()),
            regex: Vec::new(),
            path: Some(".".to_string()),
            max_matches: Some(5),
            max_scan: Some(1024),
            cursor: None,
            case_insensitive: true,
            fixed_strings: false,
            word: false,
            around: None,
            glob: Some("*.rs".to_string()),
            no_ignore: true,
            exclude_patterns: vec!["target/**".to_string()],
            context_before: Some(2),
            context_after: Some(3),
            cwd: None,
            output: "human".to_string(),
        };
        let find_built = build_file_command(find_cmd);
        assert_eq!(find_built.command.as_deref(), Some("file.search"));
        let fv = args_json(&find_built);
        assert_eq!(fv["pattern"], "needle");
        assert_eq!(fv["path"], ".");
        assert_eq!(fv["max_matches"].as_u64(), Some(5));
        assert_eq!(fv["max_scan_bytes"].as_u64(), Some(1024));
        assert_eq!(fv["case_insensitive"].as_bool(), Some(true));
        assert_eq!(fv["glob"], "*.rs");
        assert_eq!(fv["respect_gitignore"].as_bool(), Some(false));
        assert_eq!(fv["context_before"].as_u64(), Some(2));
        assert_eq!(fv["context_after"].as_u64(), Some(3));
    }

    #[test]
    fn file_mkdir_move_and_delete_map_simple_fields() {
        let mkdir_cmd = RemoteFileCommands::Mkdir {
            path: "dir".to_string(),
            parents: true,
            cwd: Some("/root".to_string()),
            output: "human".to_string(),
        };
        let mkdir_built = build_file_command(mkdir_cmd);
        assert_eq!(mkdir_built.command.as_deref(), Some("file.mkdir"));
        let mv = args_json(&mkdir_built);
        assert_eq!(mv["path"], "dir");
        assert_eq!(mv["parents"].as_bool(), Some(true));

        let move_cmd = RemoteFileCommands::Move {
            from: "a.txt".to_string(),
            to: "b.txt".to_string(),
            base_sha256: None,
            allow_overwrite: None,
            cwd: None,
            output: "human".to_string(),
        };
        let move_built = build_file_command(move_cmd);
        assert_eq!(move_built.command.as_deref(), Some("file.move"));
        let mv2 = args_json(&move_built);
        assert_eq!(mv2["path"], "a.txt");
        assert_eq!(mv2["to_path"], "b.txt");

        let delete_cmd = RemoteFileCommands::Delete {
            path: "tmp.txt".to_string(),
            recursive: true,
            cwd: Some("/tmp".to_string()),
            output: "human".to_string(),
        };
        let delete_built = build_file_command(delete_cmd);
        assert_eq!(delete_built.command.as_deref(), Some("file.delete"));
        let dv = args_json(&delete_built);
        assert_eq!(dv["path"], "tmp.txt");
        assert_eq!(dv["recursive"].as_bool(), Some(true));
    }

    #[test]
    fn file_write_prefers_cli_b64_over_file() {
        let cmd = RemoteFileCommands::Write {
            path: Some("out.txt".to_string()),
            path_flag: None,
            content: None,
            content_file: Some("ignored".to_string()),
            content_b64: Some("Zm9vYmFy".to_string()),
            base_sha256: Some("abc".to_string()),
            allow_overwrite: Some(true),
            create_parents: false,
            cwd: None,
            output: "human".to_string(),
        };
        let built = build_file_command(cmd);
        assert_eq!(built.command.as_deref(), Some("file.write"));
        let v = args_json(&built);
        assert_eq!(v["path"], "out.txt");
        assert_eq!(v["content_b64"], "Zm9vYmFy");
        assert_eq!(v["base_sha256"], "abc");
        assert_eq!(v["allow_overwrite"].as_bool(), Some(true));
        assert_eq!(v["create_parents"].as_bool(), Some(false));
    }

    #[test]
    fn file_write_reads_bytes_from_content_file() {
        let mut tmp = NamedTempFile::new().expect("temp file");
        write!(tmp, "hello-file").expect("write temp content");
        let path = tmp.path().to_path_buf();

        let cmd = RemoteFileCommands::Write {
            path: Some("from-file.txt".to_string()),
            path_flag: None,
            content: None,
            content_file: Some(path.display().to_string()),
            content_b64: None,
            base_sha256: None,
            allow_overwrite: Some(false),
            create_parents: true,
            cwd: Some("/cwd".to_string()),
            output: "human".to_string(),
        };
        let built = build_file_command(cmd);
        let v = args_json(&built);
        assert_eq!(v["path"], "from-file.txt");
        assert_eq!(v["cwd"], "/cwd");
        let b64 = v["content_b64"].as_str().expect("b64 string");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .expect("decode base64");
        assert_eq!(decoded, b"hello-file");
    }

    #[test]
    fn file_write_accepts_path_flag_compatibility_alias() {
        let cmd = RemoteFileCommands::Write {
            path: None,
            path_flag: Some("flag-path.txt".to_string()),
            content: Some("hello".to_string()),
            content_file: None,
            content_b64: None,
            base_sha256: None,
            allow_overwrite: None,
            create_parents: true,
            cwd: None,
            output: "human".to_string(),
        };
        let built = build_file_command(cmd);
        let v = args_json(&built);
        assert_eq!(v["path"], "flag-path.txt");
        assert_eq!(v["create_parents"].as_bool(), Some(true));
    }

    #[test]
    fn file_write_rejects_duplicate_positional_and_path_flag() {
        let cmd = RemoteFileCommands::Write {
            path: Some("pos.txt".to_string()),
            path_flag: Some("flag.txt".to_string()),
            content: Some("hello".to_string()),
            content_file: None,
            content_b64: None,
            base_sha256: None,
            allow_overwrite: None,
            create_parents: false,
            cwd: None,
            output: "human".to_string(),
        };
        let err = build_remote_file_command(&cmd).expect_err("expected duplicate path error");
        assert!(err.to_string().contains("target path only once"));
    }

    #[test]
    fn file_write_reports_io_error_when_file_missing() {
        let cmd = RemoteFileCommands::Write {
            path: Some("missing.txt".to_string()),
            path_flag: None,
            content: None,
            content_file: Some("/definitely/not/present".to_string()),
            content_b64: None,
            base_sha256: None,
            allow_overwrite: Some(false),
            create_parents: false,
            cwd: None,
            output: "human".to_string(),
        };
        let err = build_remote_file_command(&cmd).expect_err("expected error");
        match err {
            BifrostError::Io(e) => {
                let msg = e.to_string();
                assert!(msg.contains("read content file"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn file_write_rejects_missing_content_source() {
        let cmd = RemoteFileCommands::Write {
            path: Some("missing-content.txt".to_string()),
            path_flag: None,
            content: None,
            content_file: None,
            content_b64: None,
            base_sha256: None,
            allow_overwrite: Some(false),
            create_parents: false,
            cwd: None,
            output: "human".to_string(),
        };
        let err = build_remote_file_command(&cmd).expect_err("expected missing content source");
        match err {
            BifrostError::Config(msg) => {
                assert!(msg.contains("missing content source for file.write"));
                assert!(msg.contains("--from-local -"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn remote_run_builds_argv_exec_payload_for_uploaded_script() {
        let args = RemoteRunArgs {
            script_file: Some(PathBuf::from("./q.py")),
            interpreter: "python3".to_string(),
            cwd: Some("/repo".to_string()),
            env: vec![("TOKEN".to_string(), "redacted".to_string())],
            timeout_ms: Some(600000),
            scratch_name: ".bifrost-tmp".to_string(),
            remote_name: None,
            keep: false,
            detach: false,
            stream: true,
            output_file: Some("run.log".to_string()),
            no_verify_digest: true,
            args: vec!["--limit".to_string(), "5".to_string()],
        };
        let built = build_remote_run_shell_exec_command(&args, ".bifrost-tmp/run-q.py".to_string());
        assert_eq!(built.kind, CommandKind::ShellExec);
        assert_eq!(built.label, "remote.run");
        let payload = built.shell_exec.as_ref().expect("shell payload");
        assert_eq!(payload.exec_mode, "argv_exec");
        assert_eq!(
            payload.argv.as_ref().expect("argv"),
            &vec![
                "python3".to_string(),
                ".bifrost-tmp/run-q.py".to_string(),
                "--limit".to_string(),
                "5".to_string(),
            ]
        );
        assert_eq!(payload.cwd.as_deref(), Some("/repo"));
        assert_eq!(payload.timeout_ms, Some(600000));
        assert_eq!(
            payload
                .env
                .as_ref()
                .and_then(|env| env.get("TOKEN"))
                .map(String::as_str),
            Some("redacted")
        );
        let prefs = built.streaming_prefs.as_ref().expect("streaming prefs");
        assert!(prefs.is_streaming());
        assert!(prefs.no_verify_digest);
        assert_eq!(prefs.output_file.as_deref(), Some(Path::new("run.log")));
    }

    #[test]
    fn remote_run_remote_path_uses_scratch_dir_and_sanitized_filename() {
        let args = RemoteRunArgs {
            script_file: Some(PathBuf::from("./weird name$.py")),
            interpreter: "python3".to_string(),
            cwd: None,
            env: vec![],
            timeout_ms: None,
            scratch_name: ".bifrost-tmp".to_string(),
            remote_name: Some("custom name$.py".to_string()),
            keep: false,
            detach: false,
            stream: false,
            output_file: None,
            no_verify_digest: false,
            args: vec![],
        };
        let remote_path = remote_run_script_remote_path(&args).expect("remote path");
        assert_eq!(remote_path, ".bifrost-tmp/custom_name_.py");
    }

    #[test]
    fn file_edit_parses_valid_edits_json() {
        let cmd = RemoteFileCommands::Edit {
            path: "file.txt".to_string(),
            edits: Some("[{\"op\":\"replace\"}]".to_string()),
            edits_file: None,
            base_sha256: Some("abc".to_string()),
            cwd: None,
            output: "human".to_string(),
        };
        let built = build_file_command(cmd);
        let v = args_json(&built);
        assert!(v["edits"].is_array());
        assert_eq!(v["base_sha256"], "abc");
    }

    #[test]
    fn file_edit_reads_edits_from_local_file() {
        let mut tmp = NamedTempFile::new().expect("temp edits");
        write!(
            tmp,
            "[{{\"old_string\":\"before\",\"new_string\":\"after\"}}]"
        )
        .expect("write edits");
        let path = tmp.path().to_path_buf();

        let cmd = RemoteFileCommands::Edit {
            path: "file.txt".to_string(),
            edits: None,
            edits_file: Some(path.display().to_string()),
            base_sha256: None,
            cwd: Some("/repo".to_string()),
            output: "human".to_string(),
        };
        let built = build_file_command(cmd);
        let v = args_json(&built);
        assert_eq!(v["path"], "file.txt");
        assert_eq!(v["cwd"], "/repo");
        assert_eq!(v["edits"][0]["old_string"], "before");
        assert_eq!(v["edits"][0]["new_string"], "after");
    }

    #[test]
    fn file_edit_rejects_invalid_edits_json() {
        let cmd = RemoteFileCommands::Edit {
            path: "file.txt".to_string(),
            edits: Some("not-json".to_string()),
            edits_file: None,
            base_sha256: None,
            cwd: None,
            output: "human".to_string(),
        };
        let err = build_remote_file_command(&cmd).expect_err("expected config error");
        match err {
            BifrostError::Config(msg) => {
                assert!(msg.contains("invalid --edits JSON"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn file_patch_from_base64_populates_patch_text() {
        let patch_text = "diff --git a b";
        let b64 = base64::engine::general_purpose::STANDARD.encode(patch_text);
        let cmd = RemoteFileCommands::Patch {
            patch_file: None,
            patch_b64: Some(b64),
            base_sha: Vec::new(),
            cwd: Some("/repo".to_string()),
            output: "human".to_string(),
        };
        let built = build_file_command(cmd);
        let v = args_json(&built);
        assert_eq!(v["patch_text"], patch_text);
        assert_eq!(v["cwd"], "/repo");
    }

    #[test]
    fn file_patch_reports_decode_error_for_invalid_base64() {
        let cmd = RemoteFileCommands::Patch {
            patch_file: None,
            patch_b64: Some("%%%".to_string()),
            base_sha: Vec::new(),
            cwd: None,
            output: "human".to_string(),
        };
        let err = build_remote_file_command(&cmd).expect_err("expected error");
        match err {
            BifrostError::Io(e) => {
                assert!(e.to_string().contains("decode --patch-b64"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn file_patch_requires_source_when_neither_file_nor_b64_provided() {
        let cmd = RemoteFileCommands::Patch {
            patch_file: None,
            patch_b64: None,
            base_sha: Vec::new(),
            cwd: None,
            output: "human".to_string(),
        };
        let err = build_remote_file_command(&cmd).expect_err("expected config error");
        match err {
            BifrostError::Config(msg) => {
                assert!(msg.contains("apply-patch requires"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn file_patch_reads_patch_file_when_provided() {
        let mut tmp = NamedTempFile::new().expect("temp patch");
        write!(tmp, "line1\nline2\n").expect("write patch");
        let path = tmp.path().to_path_buf();

        let cmd = RemoteFileCommands::Patch {
            patch_file: Some(path.display().to_string()),
            patch_b64: None,
            base_sha: Vec::new(),
            cwd: None,
            output: "human".to_string(),
        };
        let built = build_file_command(cmd);
        let v = args_json(&built);
        assert_eq!(v["patch_text"], "line1\nline2\n");
    }

    #[test]
    fn build_remote_command_for_file_variant_wraps_file_command() {
        let action = RemoteCommands::File {
            action: Box::new(RemoteFileCommands::Stat {
                path: "foo.txt".to_string(),
                cwd: None,
                output: "human".to_string(),
            }),
        };

        let built = build_remote_command_checked(&action).expect("build file command");
        assert_eq!(built.kind, CommandKind::File);
        assert!(matches!(built.render, RemoteRenderMode::File { .. }));
        assert_eq!(built.command.as_deref(), Some("file.stat"));
    }
}
