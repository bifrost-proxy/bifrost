use std::collections::BTreeMap;
use std::future::Future;
use std::process::Stdio;
use std::time::{Duration, Instant};

use bifrost_command::{
    CanonicalQueryCommand, SearchArgs, TrafficGetArgs, TrafficListArgs, TrafficListDirection,
};
use bifrost_core::{direct_reqwest_client_builder, BifrostError, Result};
use bifrost_storage::{RemoteShellSet, RemoteShellStore};
use futures_util::StreamExt;
use regex::Regex;
use sha1::{Digest, Sha1};
use tokio::io::AsyncReadExt;
use tokio::process::Command as TokioCommand;
use tracing::{debug, info, warn};

use crate::query_service::AdminQueryService;
use crate::state::SharedAdminState;

use super::types::{is_allowed_command, RemoteCommand, RemoteInvokeResponse, ShellExecMode};

const MAX_ID_LEN: usize = 20;
const MAX_QUERY_LEN: usize = 500;
const MAX_HOST_LEN: usize = 200;
const MAX_URL_LEN: usize = 500;
const MAX_PATH_LEN: usize = 500;
const MAX_CONTENT_TYPE_LEN: usize = 100;
const MAX_CLIENT_IP_LEN: usize = 45;
const MAX_CLIENT_APP_LEN: usize = 200;
const MAX_TRAFFIC_LIST_LIMIT: usize = 100;
const REQUEST_TIMEOUT_SECS: u64 = 30;
const SEARCH_STREAM_TIMEOUT_SECS: u64 = 600;
const DEFAULT_SHELL_OUTPUT_MAX_BYTES: usize = 4 * 1024 * 1024; // PR#2: bumped 64 KiB -> 4 MiB; binary-safe stream path lands in PR#3
#[allow(dead_code)]
// Retained for backward-compat reference; PR#3a switched to idle-timeout primary liveness.
const DEFAULT_SHELL_TIMEOUT_MS: u64 = 30_000;
/// PR#3a: default idle timeout for shell.exec.
const DEFAULT_IDLE_TIMEOUT_MS: u64 = 300_000;
/// PR#3a: heartbeat tick interval (floor of idle-timeout granularity).
const HEARTBEAT_INTERVAL_MS: u64 = 10_000;
/// PR#3b: elapsed wall-clock after which the executor emits a single
/// StreamFrame::Reconnect advisory so receivers can swap relay
/// connections BEFORE the relay's 30-minute hard limit hits. 27 min
/// gives receivers ~3 min to tear down + re-establish.
const RELAY_RECONNECT_HINT_MS: u64 = 27 * 60 * 1000;

const ALLOWED_METHODS: &[&str] = &[
    "GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS", "CONNECT", "TRACE",
];
const ALLOWED_PROTOCOLS: &[&str] = &["http", "https", "ws", "wss", "h3"];
const ALLOWED_DIRECTIONS: &[&str] = &["backward", "forward"];

pub struct RemoteInvokeExecutor {
    admin_host: String,
    admin_port: u16,
    http: reqwest::Client,
    query_service: Option<AdminQueryService>,
    keepawake_manager: Option<bifrost_power::SharedKeepAwakeManager>,
}

#[derive(Debug, Clone)]
struct ResolvedShellPolicy {
    policy_id: String,
    allowed_exec_modes: Vec<ShellExecMode>,
    reject_reason: Option<String>,
    allow_any_executable: bool,
    allowed_executables: Vec<String>,
    allowed_shell_patterns: Vec<String>,
    cwd_allowlist: Vec<String>,
    env_allowlist: Vec<String>,
    default_cwd: Option<String>,
    shell: Option<String>,
    max_timeout_ms: Option<u64>,
    /// PR#3a: idle timeout (None => DEFAULT_IDLE_TIMEOUT_MS)
    max_idle_ms: Option<u64>,
    max_output_bytes: usize,
    /// PR#7: wall-clock ceiling independent of idle timeout. `None` means unbounded.
    /// PR#7b: enforced via independent select arm in both streaming and legacy paths.
    max_wall_clock_ms: Option<u64>,
    /// PR#7: hard cap on cumulative streamed stdout+stderr bytes. `None` means
    /// fall back to legacy `max_output_bytes` (which only bounds the inline preview).
    /// PR#7b: enforced in both streaming and legacy exec paths.
    max_output_bytes_total: Option<u64>,
    /// PR#7: whether this policy permits `Resume(call_id, from_offset)` subscriptions
    /// on the streaming endpoint. Defaults to false until operators opt in.
    /// PR#7b: enforced via enforce_allow_resume() at subscription dispatch.
    allow_resume: bool,
    stdin_allowed: bool,
    interactive_allowed: bool,
    inherit_env: bool,
    default_env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, serde::Deserialize, Default)]
#[serde(default)]
struct ShellPolicyMetadata {
    exec_mode: Option<ShellExecMode>,
    allowed_exec_modes: Vec<ShellExecMode>,
    reject_reason: Option<String>,
    allow_any_executable: Option<bool>,
    allowed_executables: Vec<String>,
    allowed_shell_patterns: Vec<String>,
    cwd_allowlist: Vec<String>,
    env_allowlist: Vec<String>,
    default_cwd: Option<String>,
    shell: Option<String>,
    max_timeout_ms: Option<u64>,
    max_idle_ms: Option<u64>,
    max_output_bytes: Option<usize>,
    max_wall_clock_ms: Option<u64>,
    max_output_bytes_total: Option<u64>,
    allow_resume: Option<bool>,
    stdin_allowed: Option<bool>,
    interactive_allowed: Option<bool>,
    inherit_env: Option<bool>,
    default_env: BTreeMap<String, String>,
}

#[derive(Debug, serde::Deserialize, Default)]
struct CommandArgs {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    request_body: Option<bool>,
    #[serde(default)]
    response_body: Option<bool>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    cursor: Option<u64>,
    #[serde(default)]
    direction: Option<String>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    status: Option<u16>,
    #[serde(default)]
    status_min: Option<u16>,
    #[serde(default)]
    status_max: Option<u16>,
    #[serde(default)]
    protocol: Option<String>,
    #[serde(default)]
    host: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    content_type: Option<String>,
    #[serde(default)]
    client_ip: Option<String>,
    #[serde(default)]
    client_app: Option<String>,
    #[serde(default)]
    has_rule_hit: Option<bool>,
    #[serde(default)]
    is_websocket: Option<bool>,
    #[serde(default)]
    is_sse: Option<bool>,
    #[serde(default)]
    is_tunnel: Option<bool>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct TrafficQueryResult {
    records: Vec<TrafficSummaryRow>,
    server_sequence: u64,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct TrafficSummaryRow {
    id: String,
    seq: u64,
    m: String,
    h: String,
    p: String,
    s: u16,
    res_sz: usize,
    dur: u64,
    proto: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone, serde::Deserialize)]
struct SearchResultRow {
    record: TrafficSummaryRow,
    matches: Vec<SearchMatchRow>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, serde::Deserialize)]
struct SearchMatchRow {
    field: String,
    preview: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone, serde::Deserialize)]
struct SearchProgressPayload {
    total_searched: usize,
    total_matched: usize,
}

#[allow(dead_code)]
#[derive(Debug, Clone, serde::Deserialize)]
struct SearchDonePayload {
    total_searched: usize,
    total_matched: usize,
    has_more: bool,
}

enum SearchServiceEvent {
    Result(Box<crate::search::SearchResultItem>),
    Progress(crate::search::SearchProgress),
}

impl RemoteInvokeExecutor {
    pub fn new(admin_host: &str, admin_port: u16) -> Self {
        let http = direct_reqwest_client_builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()
            .expect("failed to build reqwest client for RemoteInvokeExecutor");
        Self {
            admin_host: admin_host.to_string(),
            admin_port,
            http,
            query_service: None,
            keepawake_manager: None,
        }
    }

    pub fn new_with_state(admin_host: &str, admin_port: u16, state: SharedAdminState) -> Self {
        let mut executor = Self::new(admin_host, admin_port);
        executor.query_service = Some(AdminQueryService::new(state.clone()));
        executor.keepawake_manager = state.keepawake_manager.clone();
        executor
    }

    pub async fn execute(&self, command: &RemoteCommand) -> Result<RemoteInvokeResponse> {
        self.execute_with_stdout_sink(command, |_| async { Ok(()) })
            .await
    }

    pub async fn execute_with_stdout_sink<F, Fut>(
        &self,
        command: &RemoteCommand,
        mut on_stdout: F,
    ) -> Result<RemoteInvokeResponse>
    where
        F: FnMut(Vec<u8>) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        // Auto-trigger keep-awake on every remote call (all kinds, including
        // read-only). Manager internally decides whether to acquire based
        // on mode; on non-macOS this is a cheap no-op.
        if let Some(mgr) = &self.keepawake_manager {
            mgr.on_remote_call();
        }
        let start = Instant::now();
        let transport_validation = match command.query.as_ref() {
            Some(query) => self.validate_query_transport_kind(command.kind, query),
            None => Ok(()),
        };
        let query_result = match transport_validation {
            Err(error) => Err(error),
            Ok(()) => match command.kind {
                super::types::CommandKind::QueryReadonly => {
                    let command_label = command.summary_label().to_string();
                    if !is_allowed_command(&command_label) {
                        let stderr = format!("command '{}' is not allowed", command_label);
                        warn!(
                            command = %command_label,
                            "remote invoke rejected: command not in whitelist"
                        );
                        Err(BifrostError::Config(stderr))
                    } else if let Some(query) = &command.query {
                        self.validate_query(query)?;
                        self.dispatch_query_with_stdout_sink(query, &mut on_stdout)
                            .await
                    } else if command.command == "status" {
                        let args = self.parse_and_validate_args(command)?;
                        self.dispatch_with_stdout_sink(&command.command, &args, &mut on_stdout)
                            .await
                    } else {
                        Err(BifrostError::Config(
                            "legacy remote query commands are not supported".to_string(),
                        ))
                    }
                }
                super::types::CommandKind::ShellExec => {
                    return self.execute_shell_exec(command, &mut on_stdout).await;
                }
                super::types::CommandKind::File => {
                    let body = self.execute_file_op(command).await?;
                    self.emit_stdout(&mut on_stdout, body).await
                }
                super::types::CommandKind::PowerMgmt => {
                    let body = self.execute_power_op(command).await?;
                    self.emit_stdout(&mut on_stdout, body).await
                }
            },
        };
        let duration_ms = start.elapsed().as_millis() as u64;

        match query_result {
            Ok(body) => {
                let stdout_digest = (!body.is_empty()).then(|| sha1_hex(&body));
                debug!(
                    command = %command.summary_label(),
                    duration_ms,
                    body_len = body.len(),
                    "remote invoke completed"
                );
                Ok(RemoteInvokeResponse {
                    exit_code: 0,
                    stdout: (!body.is_empty()).then_some(body),
                    stderr: None,
                    stdout_digest,
                    stderr_digest: None,
                    duration_ms,
                    ..Default::default()
                })
            }
            Err(e) => {
                let stderr = e.to_string();
                warn!(
                    command = %command.summary_label(),
                    duration_ms,
                    error = %stderr,
                    "remote invoke failed"
                );
                Ok(RemoteInvokeResponse {
                    exit_code: -1,
                    stdout: None,
                    stderr: Some(stderr.clone()),
                    stdout_digest: None,
                    stderr_digest: Some(sha1_hex(&stderr)),
                    duration_ms,
                    ..Default::default()
                })
            }
        }
    }

    /// Dispatch a remote `power.mgmt` command against this executor's
    /// keep-awake manager. Returns a JSON string for the response body.
    ///
    /// Supported commands:
    /// - `status`                    — return full Status
    /// - `on`                        — activate assertion (one-shot)
    /// - `off`                       — release assertion (one-shot)
    /// - `set_mode` (args: {"mode":".."}) — change mode
    async fn execute_power_op(&self, command: &RemoteCommand) -> Result<String> {
        let Some(mgr) = self.keepawake_manager.clone() else {
            return Err(BifrostError::Config(
                "keepawake manager not initialized on this host".to_string(),
            ));
        };
        let cmd = command.command.as_str();
        match cmd {
            "status" => {
                let st = mgr.status();
                serde_json::to_string(&st)
                    .map_err(|e| BifrostError::Config(format!("serialize status: {e}")))
            }
            "on" => {
                mgr.activate_once()
                    .map_err(|e| BifrostError::Config(format!("activate: {e}")))?;
                let st = mgr.status();
                serde_json::to_string(&st)
                    .map_err(|e| BifrostError::Config(format!("serialize status: {e}")))
            }
            "off" => {
                mgr.release_once()
                    .map_err(|e| BifrostError::Config(format!("release: {e}")))?;
                let st = mgr.status();
                serde_json::to_string(&st)
                    .map_err(|e| BifrostError::Config(format!("serialize status: {e}")))
            }
            "set_mode" => {
                #[derive(serde::Deserialize)]
                struct Args {
                    mode: String,
                }
                let args: Args = command
                    .args_json
                    .as_deref()
                    .ok_or_else(|| BifrostError::Config("missing args".to_string()))
                    .and_then(|s| {
                        serde_json::from_str(s).map_err(|e| BifrostError::Config(e.to_string()))
                    })?;
                let mode: bifrost_power::Mode =
                    args.mode
                        .parse()
                        .map_err(|e: bifrost_power::ParseModeError| {
                            BifrostError::Config(e.to_string())
                        })?;
                mgr.set_mode(mode)
                    .map_err(|e| BifrostError::Config(format!("set_mode: {e}")))?;
                let st = mgr.status();
                serde_json::to_string(&st)
                    .map_err(|e| BifrostError::Config(format!("serialize status: {e}")))
            }
            other => Err(BifrostError::Config(format!(
                "unknown power command: {other}"
            ))),
        }
    }

    /// Public entry point for file.* operations.
    ///
    /// Wraps [`execute_file_op_inner`] to emit a single `audit.file` tracing
    /// event per request, recording `grant_id`, `method`, `path_hash`
    /// (sha256 prefix), `op`, `duration_ms`, and — when the inner handler
    /// returns JSON with `bytes_written` / `sha256` fields — those too.
    ///
    /// On error, logs the file error code extracted from the `[file.xxx]`
    /// prefix (falls back to `"file.internal"`).
    async fn execute_file_op(&self, command: &RemoteCommand) -> Result<String> {
        let started = Instant::now();

        // Best-effort extraction of grant_id / path / method *before* dispatch,
        // so we can log even when the inner fn returns early with an error.
        let method = command.command.clone();
        let (path_for_hash, grant_for_log) = {
            #[derive(serde::Deserialize, Default)]
            #[serde(default)]
            struct Peek {
                path: Option<String>,
                grant_id: Option<String>,
            }
            let peek: Peek = command
                .args_json
                .as_deref()
                .and_then(|raw| serde_json::from_str(raw).ok())
                .unwrap_or_default();
            let p = peek.path.unwrap_or_default();
            let g = command
                .grant_id
                .clone()
                .or(peek.grant_id)
                .unwrap_or_default();
            (p, g)
        };
        let path_hash = audit_path_hash(&path_for_hash);

        let outcome = self.execute_file_op_inner(command).await;
        let duration_ms = started.elapsed().as_millis() as u64;

        match &outcome {
            Ok(json) => {
                // Try to surface bytes_written / sha256 from the handler's JSON.
                let (bytes, sha) = extract_audit_fields(json);
                info!(
                    target = "audit.file",
                    grant_id = %grant_for_log,
                    method = %method,
                    path_hash = %path_hash,
                    duration_ms,
                    result = "ok",
                    bytes = bytes.unwrap_or(0),
                    sha256 = sha.as_deref().unwrap_or(""),
                    "file op succeeded"
                );
            }
            Err(err) => {
                let code = extract_file_error_code(&err.to_string());
                warn!(
                    target = "audit.file",
                    grant_id = %grant_for_log,
                    method = %method,
                    path_hash = %path_hash,
                    duration_ms,
                    result = "err",
                    error_code = %code,
                    "file op failed"
                );
            }
        }

        outcome
    }

    async fn execute_file_op_inner(&self, command: &RemoteCommand) -> Result<String> {
        use bifrost_core::file_access::FileOp;
        use std::path::Path;

        // The specific file operation is carried in command.command (e.g. "file.read")
        let file_op_name = command.command.as_str();
        let op = match file_op_name {
            "file.read" => FileOp::Read,
            "file.list" => FileOp::List,
            "file.stat" => FileOp::Stat,
            "file.glob" => FileOp::Glob,
            "file.search" => FileOp::Search,
            "file.hash" => FileOp::Hash,
            "file.write" => FileOp::Write,
            "file.edit" => FileOp::Edit,
            "file.mkdir" => FileOp::Mkdir,
            "file.move" => FileOp::Move,
            "file.delete" => FileOp::Delete,
            "file.apply_patch" => FileOp::ApplyPatch,
            _ => {
                return Err(BifrostError::Config(format!(
                    "unknown file operation: '{}'",
                    file_op_name
                )))
            }
        };

        // Enforce grant-level file_access: if the grant only allows read,
        // reject any write operation before we even load the file policy.
        if op.is_write()
            && !matches!(
                command.file_access,
                super::types::FileAccessScope::ReadWrite
            )
        {
            return Err(BifrostError::Config(format!(
                "[file.permission_denied] grant file_access={:?} does not allow write operation '{}'",
                command.file_access, file_op_name
            )));
        }

        #[derive(serde::Deserialize, Default)]
        #[serde(default)]
        struct FileParams {
            path: Option<String>,
            max_bytes: Option<u64>,
            allow_binary: Option<bool>,
            depth: Option<u32>,
            pattern: Option<String>,
            max_matches: Option<usize>,
            max_scan_bytes: Option<u64>,
            algo: Option<String>,
            grant_id: Option<String>,
            cwd: Option<String>,
            content_b64: Option<String>,
            base_sha256: Option<String>,
            #[serde(default)]
            if_match_sha256: Option<String>,
            allow_overwrite: Option<bool>,
            edits: Option<Vec<super::file_ops::EditRange>>,
            to_path: Option<String>,
            recursive: Option<bool>,
            parents: Option<bool>,
            create_parents: Option<bool>,
            patch_text: Option<String>,
            exclude_patterns: Option<Vec<String>>,
            context_before: Option<u32>,
            context_after: Option<u32>,
            offset: Option<u32>,
            limit: Option<u32>,
            case_insensitive: Option<bool>,
            glob: Option<String>,
            respect_gitignore: Option<bool>,
        }

        let params: FileParams = match command.args_json.as_deref() {
            Some(raw) if !raw.trim().is_empty() => serde_json::from_str(raw).map_err(|e| {
                BifrostError::Config(format!("[file.invalid_args] bad args_json: {}", e))
            })?,
            _ => FileParams::default(),
        };

        let cwd_str = params
            .cwd
            .clone()
            .or_else(|| command.cwd.clone())
            .unwrap_or_else(|| {
                std::env::current_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| "/".to_string())
            });
        let cwd = Path::new(&cwd_str);

        let grant_id = command
            .grant_id
            .clone()
            .or(params.grant_id.clone())
            .unwrap_or_default();
        let store = super::file_policy_store::FileAccessPolicyStore::load_default();
        let policy = store.resolve(
            &grant_id,
            command.caller_fingerprint.as_deref(),
            command.ssh_fingerprint.as_deref(),
            cwd,
        );

        let default_path = ".".to_string();
        let requested_path = match file_op_name {
            "file.glob" | "file.search" | "file.list" | "file.apply_patch" => {
                params.path.clone().unwrap_or(default_path)
            }
            _ => params.path.clone().ok_or_else(|| {
                BifrostError::Config("[file.invalid_args] 'path' is required".to_string())
            })?,
        };

        let decision = policy
            .check(Path::new(&requested_path), cwd, op)
            .map_err(|e| BifrostError::Config(format!("[{}] {}", e.code(), e)))?;

        let value = match file_op_name {
            "file.read" => {
                super::file_ops::handle_file_read(
                    &decision,
                    params.max_bytes,
                    params.allow_binary.unwrap_or(false),
                    params.offset,
                    params.limit,
                )
                .await?
            }
            "file.list" => {
                let excludes = params.exclude_patterns.clone().unwrap_or_default();
                super::file_ops::handle_file_list(
                    &decision,
                    params.depth,
                    &excludes,
                    params.respect_gitignore.unwrap_or(true),
                )
                .await?
            }
            "file.stat" => super::file_ops::handle_file_stat(&decision).await?,
            "file.glob" => {
                let pattern = params.pattern.clone().ok_or_else(|| {
                    BifrostError::Config(
                        "[file.invalid_args] 'pattern' is required for file.glob".to_string(),
                    )
                })?;
                let excludes = params.exclude_patterns.clone().unwrap_or_default();
                super::file_ops::handle_file_glob(
                    &decision,
                    &pattern,
                    params.max_matches,
                    &excludes,
                    params.respect_gitignore.unwrap_or(true),
                )
                .await?
            }
            "file.search" => {
                let pattern = params.pattern.clone().ok_or_else(|| {
                    BifrostError::Config(
                        "[file.invalid_args] 'pattern' is required for file.search".to_string(),
                    )
                })?;
                let excludes = params.exclude_patterns.clone().unwrap_or_default();
                super::file_ops::handle_file_search(
                    &decision,
                    &pattern,
                    params.max_matches,
                    params.max_scan_bytes,
                    &excludes,
                    params.context_before,
                    params.context_after,
                    params.case_insensitive.unwrap_or(false),
                    params.glob.as_deref(),
                    params.respect_gitignore.unwrap_or(true),
                    &policy.denies,
                )
                .await?
            }
            "file.hash" => {
                super::file_ops::handle_file_hash(&decision, params.algo.as_deref()).await?
            }
            "file.write" => {
                let content = params.content_b64.as_deref().ok_or_else(|| {
                    BifrostError::Config(
                        "[file.invalid_args] 'content_b64' is required for file.write".to_string(),
                    )
                })?;
                super::file_ops::handle_file_write(
                    &decision,
                    content,
                    params.base_sha256.as_deref(),
                    params.allow_overwrite,
                    params.create_parents.unwrap_or(false),
                )
                .await?
            }
            "file.edit" => {
                let edits = params.edits.as_ref().ok_or_else(|| {
                    BifrostError::Config(
                        "[file.invalid_args] 'edits' is required for file.edit".to_string(),
                    )
                })?;
                super::file_ops::handle_file_edit(&decision, params.base_sha256.as_deref(), edits)
                    .await?
            }
            "file.mkdir" => {
                super::file_ops::handle_file_mkdir(&decision, params.parents.unwrap_or(false))
                    .await?
            }
            "file.move" => {
                let to = params.to_path.as_deref().ok_or_else(|| {
                    BifrostError::Config(
                        "[file.invalid_args] 'to_path' is required for file.move".to_string(),
                    )
                })?;
                let to_decision = policy
                    .check(Path::new(to), cwd, FileOp::Move)
                    .map_err(|e| BifrostError::Config(format!("[{}] {}", e.code(), e)))?;
                super::file_ops::handle_file_move(
                    &decision,
                    &to_decision,
                    params.base_sha256.as_deref(),
                )
                .await?
            }
            "file.delete" => {
                super::file_ops::handle_file_delete(
                    &decision,
                    params.recursive.unwrap_or(false),
                    params.if_match_sha256.as_deref(),
                )
                .await?
            }
            "file.apply_patch" => {
                let patch_text = params.patch_text.as_deref().ok_or_else(|| {
                    BifrostError::Config(
                        "[file.invalid_args] 'patch_text' is required for file.apply_patch"
                            .to_string(),
                    )
                })?;
                // Use the shared parser so the decision map always matches
                // what handle_file_apply_patch will iterate over, including
                // git-extended headers (diff --git, rename, copy, index,
                // mode changes, binary markers).
                let entries = super::file_ops::parse_patch(patch_text)?;
                let mut decisions = std::collections::HashMap::new();
                for entry in &entries {
                    // Only content-bearing entries need a decision. The
                    // handler itself will reject binary/rename/copy/mode
                    // entries with an explicit error code, but we avoid
                    // running policy.check on synthetic keys here.
                    match entry.kind {
                        super::file_ops::PatchKind::Modify
                        | super::file_ops::PatchKind::Create
                        | super::file_ops::PatchKind::Delete => {}
                        _ => continue,
                    }
                    let key = entry.decision_key();
                    if key.is_empty() || key == "/dev/null" {
                        continue;
                    }
                    if decisions.contains_key(&key) {
                        continue;
                    }
                    let d = policy
                        .check(Path::new(&key), cwd, FileOp::ApplyPatch)
                        .map_err(|err| BifrostError::Config(format!("[{}] {}", err.code(), err)))?;
                    decisions.insert(key, d);
                }
                super::file_ops::handle_file_apply_patch(&decisions, patch_text).await?
            }
            _ => unreachable!(),
        };

        serde_json::to_string(&value)
            .map_err(|e| BifrostError::Config(format!("serialize file op result: {}", e)))
    }

    fn parse_and_validate_args(&self, command: &RemoteCommand) -> Result<CommandArgs> {
        if matches!(
            command.kind,
            super::types::CommandKind::ShellExec | super::types::CommandKind::File
        ) {
            return Ok(CommandArgs::default());
        }

        let args: CommandArgs = match &command.args_json {
            Some(json_str) => serde_json::from_str(json_str)
                .map_err(|e| BifrostError::Config(format!("invalid args_json: {}", e)))?,
            None => CommandArgs::default(),
        };

        if let Some(ref id) = args.id {
            if id.len() > MAX_ID_LEN {
                return Err(BifrostError::Config(format!(
                    "id param too long: {} > {}",
                    id.len(),
                    MAX_ID_LEN
                )));
            }
            if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                return Err(BifrostError::Config(
                    "id param must contain only alphanumeric characters and hyphens".to_string(),
                ));
            }
        }

        if let Some(ref query) = args.query {
            if query.len() > MAX_QUERY_LEN {
                return Err(BifrostError::Config(format!(
                    "query param too long: {} > {}",
                    query.len(),
                    MAX_QUERY_LEN
                )));
            }
            if query.chars().any(|c| c.is_ascii_control()) {
                return Err(BifrostError::Config(
                    "query param must not contain ASCII control characters".to_string(),
                ));
            }
        }

        if let Some(ref d) = args.direction {
            if !ALLOWED_DIRECTIONS.contains(&d.as_str()) {
                return Err(BifrostError::Config(format!(
                    "direction must be one of: {}",
                    ALLOWED_DIRECTIONS.join(", ")
                )));
            }
        }

        if let Some(ref m) = args.method {
            if !ALLOWED_METHODS.contains(&m.as_str()) {
                return Err(BifrostError::Config(format!(
                    "method must be one of: {}",
                    ALLOWED_METHODS.join(", ")
                )));
            }
        }

        if let Some(ref p) = args.protocol {
            if !ALLOWED_PROTOCOLS.contains(&p.as_str()) {
                return Err(BifrostError::Config(format!(
                    "protocol must be one of: {}",
                    ALLOWED_PROTOCOLS.join(", ")
                )));
            }
        }

        validate_string_param(&args.host, "host", MAX_HOST_LEN, is_host_char)?;
        validate_string_param(&args.url, "url", MAX_URL_LEN, is_url_char)?;
        validate_string_param(&args.path, "path", MAX_PATH_LEN, is_path_char)?;
        validate_string_param(
            &args.content_type,
            "content_type",
            MAX_CONTENT_TYPE_LEN,
            is_content_type_char,
        )?;
        validate_string_param(
            &args.client_ip,
            "client_ip",
            MAX_CLIENT_IP_LEN,
            is_client_ip_char,
        )?;
        validate_string_param(
            &args.client_app,
            "client_app",
            MAX_CLIENT_APP_LEN,
            is_client_app_char,
        )?;

        for (name, val) in [
            ("status", args.status),
            ("status_min", args.status_min),
            ("status_max", args.status_max),
        ] {
            if let Some(v) = val {
                if !(100..=599).contains(&v) {
                    return Err(BifrostError::Config(format!(
                        "{} must be between 100 and 599, got {}",
                        name, v
                    )));
                }
            }
        }

        Ok(args)
    }

    fn validate_query(&self, query: &CanonicalQueryCommand) -> Result<()> {
        match query {
            CanonicalQueryCommand::Search(args) => self.validate_search_args(args),
            CanonicalQueryCommand::TrafficList(args) => self.validate_traffic_list_args(args),
            CanonicalQueryCommand::TrafficGet(args) => self.validate_traffic_get_args(args),
            CanonicalQueryCommand::TrafficClear(_) => Err(BifrostError::Config(
                "traffic.clear is not enabled for remote invoke".to_string(),
            )),
        }
    }

    fn validate_query_transport_kind(
        &self,
        kind: super::types::CommandKind,
        query: &CanonicalQueryCommand,
    ) -> Result<()> {
        if kind == super::types::CommandKind::QueryReadonly
            && query.capability() == bifrost_command::CommandCapability::Mutating
        {
            return Err(BifrostError::Config(format!(
                "mutating query '{}' cannot be sent as query.readonly",
                query.command_id()
            )));
        }
        Ok(())
    }

    fn validate_search_args(&self, args: &SearchArgs) -> Result<()> {
        if args.keyword.len() > MAX_QUERY_LEN {
            return Err(BifrostError::Config(format!(
                "query param too long: {} > {}",
                args.keyword.len(),
                MAX_QUERY_LEN
            )));
        }
        if args.keyword.chars().any(|c| c.is_ascii_control()) {
            return Err(BifrostError::Config(
                "query param must not contain ASCII control characters".to_string(),
            ));
        }
        for domain in &args.filters.domains {
            validate_string_param(&Some(domain.clone()), "domain", MAX_HOST_LEN, is_host_char)?;
        }
        for content_type in &args.filters.content_types {
            validate_string_param(
                &Some(content_type.clone()),
                "content_type",
                MAX_CONTENT_TYPE_LEN,
                is_content_type_char,
            )?;
        }
        for protocol in &args.filters.protocols {
            let protocol = protocol.to_lowercase();
            if !ALLOWED_PROTOCOLS.contains(&protocol.as_str()) {
                return Err(BifrostError::Config(format!(
                    "unsupported protocol filter '{}'",
                    protocol
                )));
            }
        }
        for status_range in &args.filters.status_ranges {
            if !matches!(
                status_range.as_str(),
                "2xx" | "3xx" | "4xx" | "5xx" | "error"
            ) {
                return Err(BifrostError::Config(format!(
                    "unsupported status filter '{}'",
                    status_range
                )));
            }
        }
        for condition in &args.filters.conditions {
            match condition.field.as_str() {
                "method" if !ALLOWED_METHODS.contains(&condition.value.as_str()) => {
                    return Err(BifrostError::Config(format!(
                        "unsupported method filter '{}'",
                        condition.value
                    )));
                }
                "method" => {}
                "host" => validate_string_param(
                    &Some(condition.value.clone()),
                    "host",
                    MAX_HOST_LEN,
                    is_host_char,
                )?,
                "path" => validate_string_param(
                    &Some(condition.value.clone()),
                    "path",
                    MAX_PATH_LEN,
                    is_path_char,
                )?,
                _ => {}
            }
        }
        Ok(())
    }

    fn validate_traffic_list_args(&self, args: &TrafficListArgs) -> Result<()> {
        validate_string_param(&args.method, "method", 16, |c| c.is_ascii_uppercase())?;
        validate_string_param(&args.protocol, "protocol", 8, |c| c.is_ascii_lowercase())?;
        validate_string_param(&args.host, "host", MAX_HOST_LEN, is_host_char)?;
        validate_string_param(&args.url, "url", MAX_URL_LEN, is_url_char)?;
        validate_string_param(&args.path, "path", MAX_PATH_LEN, is_path_char)?;
        validate_string_param(
            &args.content_type,
            "content_type",
            MAX_CONTENT_TYPE_LEN,
            is_content_type_char,
        )?;
        validate_string_param(
            &args.client_ip,
            "client_ip",
            MAX_CLIENT_IP_LEN,
            is_client_ip_char,
        )?;
        validate_string_param(
            &args.client_app,
            "client_app",
            MAX_CLIENT_APP_LEN,
            is_client_app_char,
        )?;

        if let Some(method) = &args.method {
            if !ALLOWED_METHODS.contains(&method.as_str()) {
                return Err(BifrostError::Config(format!(
                    "unsupported method '{}'",
                    method
                )));
            }
        }
        if let Some(protocol) = &args.protocol {
            if !ALLOWED_PROTOCOLS.contains(&protocol.as_str()) {
                return Err(BifrostError::Config(format!(
                    "unsupported protocol '{}'",
                    protocol
                )));
            }
        }
        Ok(())
    }

    fn validate_traffic_get_args(&self, args: &TrafficGetArgs) -> Result<()> {
        if args.id.len() > MAX_ID_LEN {
            return Err(BifrostError::Config(format!(
                "id param too long: {} > {}",
                args.id.len(),
                MAX_ID_LEN
            )));
        }
        if !args
            .id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return Err(BifrostError::Config(
                "id param must contain only alphanumeric characters and hyphens".to_string(),
            ));
        }
        Ok(())
    }

    async fn dispatch_query_with_stdout_sink<F, Fut>(
        &self,
        query: &CanonicalQueryCommand,
        on_stdout: &mut F,
    ) -> Result<String>
    where
        F: FnMut(Vec<u8>) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        match query {
            CanonicalQueryCommand::Search(args) => {
                if self.query_service.is_some() {
                    self.search_stream_via_service(args, on_stdout).await
                } else {
                    self.search_stream(
                        &args.keyword,
                        args.limit,
                        args.max_results,
                        args.max_scan,
                        on_stdout,
                    )
                    .await
                }
            }
            CanonicalQueryCommand::TrafficList(args) => {
                if let Some(service) = &self.query_service {
                    let body =
                        serde_json::to_string(&service.list_traffic(args).await?).map_err(|e| {
                            BifrostError::Config(format!("serialize traffic list: {e}"))
                        })?;
                    self.emit_stdout(on_stdout, body).await
                } else {
                    let legacy_args = self.command_args_from_traffic_list(args);
                    let body = self.list_traffic(&legacy_args).await?;
                    self.emit_stdout(on_stdout, body).await
                }
            }
            CanonicalQueryCommand::TrafficGet(args) => {
                if let Some(service) = &self.query_service {
                    let body = serde_json::to_string(&service.get_traffic_json(args).await?)
                        .map_err(|e| {
                            BifrostError::Config(format!("serialize traffic detail: {e}"))
                        })?;
                    self.emit_stdout(on_stdout, body).await
                } else {
                    let legacy_args = self.command_args_from_traffic_get(args);
                    let body = self.get_traffic(&args.id, &legacy_args).await?;
                    self.emit_stdout(on_stdout, body).await
                }
            }
            CanonicalQueryCommand::TrafficClear(_) => Err(BifrostError::Config(
                "traffic.clear is not enabled for remote invoke".to_string(),
            )),
        }
    }

    fn command_args_from_traffic_list(&self, args: &TrafficListArgs) -> CommandArgs {
        CommandArgs {
            limit: args.limit,
            cursor: args.cursor,
            direction: Some(match args.direction {
                TrafficListDirection::Backward => "backward".to_string(),
                TrafficListDirection::Forward => "forward".to_string(),
            }),
            method: args.method.clone(),
            status: args.status,
            status_min: args.status_min,
            status_max: args.status_max,
            protocol: args.protocol.clone(),
            host: args.host.clone(),
            url: args.url.clone(),
            path: args.path.clone(),
            content_type: args.content_type.clone(),
            client_ip: args.client_ip.clone(),
            client_app: args.client_app.clone(),
            has_rule_hit: args.has_rule_hit,
            is_websocket: args.is_websocket,
            is_sse: args.is_sse,
            is_tunnel: args.is_tunnel,
            ..CommandArgs::default()
        }
    }

    fn command_args_from_traffic_get(&self, args: &TrafficGetArgs) -> CommandArgs {
        CommandArgs {
            id: Some(args.id.clone()),
            request_body: Some(args.request_body),
            response_body: Some(args.response_body),
            ..CommandArgs::default()
        }
    }

    /// PR#3b: streaming variant of shell.exec. Instead of returning a
    /// RemoteInvokeResponse with buffered stdout/stderr, this method pushes
    /// `StreamFrame`s into `frame_tx` as the child produces output. The
    /// caller is responsible for pulling frames off the receiver; if the
    /// caller is slow, `frame_tx.send().await` blocks the read loop, giving
    /// natural back-pressure without unbounded in-process buffering.
    ///
    /// Lifecycle (exactly one of the terminal variants is sent):
    ///   - Stdout{seq, offset, data_b64}* / Stderr{...}*
    ///   - Heartbeat{ts, stdout_offset, stderr_offset}*   every 10s
    ///   - Reconnect{reason, stdout_offset, stderr_offset} once at ~27min
    ///   - Done{...} on normal exit
    ///   - Error{code, message} on any failure
    ///
    /// When `frame_tx` is closed by the receiver (downstream gone), the
    /// method aborts the child and returns Ok(()) — the assumption is the
    /// receiver no longer cares about this call.
    pub async fn execute_shell_exec_streaming(
        &self,
        command: &RemoteCommand,
        frame_tx: tokio::sync::mpsc::Sender<crate::remote_invoke::types::StreamFrame>,
    ) -> Result<()> {
        use crate::remote_invoke::types::StreamFrame;
        use base64::Engine as _;

        // Helper: best-effort send; if receiver is gone we abort.
        async fn send_frame(
            tx: &tokio::sync::mpsc::Sender<StreamFrame>,
            frame: StreamFrame,
        ) -> std::result::Result<(), ()> {
            tx.send(frame).await.map_err(|_| ())
        }

        async fn send_error(
            tx: &tokio::sync::mpsc::Sender<StreamFrame>,
            code: &str,
            message: String,
        ) {
            let _ = tx
                .send(StreamFrame::Error {
                    code: code.to_string(),
                    message,
                })
                .await;
        }

        let policy = match self.resolve_shell_policy(command) {
            Ok(p) => p,
            Err(e) => {
                send_error(&frame_tx, "policy_rejected", format!("{e}")).await;
                return Ok(());
            }
        };

        if command.pty.as_ref().map(|pty| pty.enabled).unwrap_or(false)
            && !policy.interactive_allowed
        {
            send_error(
                &frame_tx,
                "policy_rejected",
                format!(
                    "policy '{}' does not allow PTY/interactive shell execution",
                    policy.policy_id
                ),
            )
            .await;
            return Ok(());
        }

        // Timeout split identical to PR#3a.
        let wall_clock_timeout_ms: Option<u64> = match (command.timeout_ms, policy.max_timeout_ms) {
            (Some(c), Some(p)) => Some(c.min(p)),
            (Some(c), None) => Some(c),
            (None, Some(p)) => Some(p),
            (None, None) => None,
        };
        let idle_timeout_ms: u64 = policy.max_idle_ms.unwrap_or(DEFAULT_IDLE_TIMEOUT_MS);
        let timeout_ms = wall_clock_timeout_ms.unwrap_or(u64::MAX);

        // PR#7b Gate 3: exercise the helper at streaming entry.
        // The subscribe-with-from_offset wiring lives on a future handler;
        // here we pass None so this always succeeds but consumes the field.
        if let Err(e) = enforce_allow_resume(policy.allow_resume, None, &policy.policy_id) {
            send_error(&frame_tx, "allow_resume_rejected", format!("{e}")).await;
            return Ok(());
        }

        let start = Instant::now();
        let mut process = match self.build_shell_exec_process(command, &policy) {
            Ok(p) => p,
            Err(e) => {
                send_error(&frame_tx, "spawn_failed", format!("{e}")).await;
                return Ok(());
            }
        };
        process.stdout(Stdio::piped());
        process.stderr(Stdio::piped());
        process.stdin(Stdio::null());
        process.kill_on_drop(true);

        let mut child = match process.spawn() {
            Ok(c) => c,
            Err(e) => {
                send_error(
                    &frame_tx,
                    "spawn_failed",
                    format!("spawn shell.exec failed: {e}"),
                )
                .await;
                return Ok(());
            }
        };
        let mut stdout_reader = match child.stdout.take() {
            Some(r) => r,
            None => {
                send_error(
                    &frame_tx,
                    "spawn_failed",
                    "shell.exec stdout pipe unavailable".to_string(),
                )
                .await;
                let _ = child.kill().await;
                return Ok(());
            }
        };
        let mut stderr_reader = match child.stderr.take() {
            Some(r) => r,
            None => {
                send_error(
                    &frame_tx,
                    "spawn_failed",
                    "shell.exec stderr pipe unavailable".to_string(),
                )
                .await;
                let _ = child.kill().await;
                return Ok(());
            }
        };

        let mut stdout_buf = [0u8; 65536];
        let mut stderr_buf = [0u8; 65536];
        let mut stdout_open = true;
        let mut stderr_open = true;
        let mut exit_status: Option<std::process::ExitStatus> = None;
        let mut stdout_total_bytes: u64 = 0;
        let mut stderr_total_bytes: u64 = 0;
        let mut stdout_hasher = ring::digest::Context::new(&ring::digest::SHA256);
        let mut stderr_hasher = ring::digest::Context::new(&ring::digest::SHA256);
        let mut stdout_seq: u64 = 0;
        let mut stderr_seq: u64 = 0;
        let mut reconnect_sent = false;

        let wall_clock = async {
            match wall_clock_timeout_ms {
                Some(ms) => tokio::time::sleep(Duration::from_millis(ms)).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::pin!(wall_clock);
        // PR#7b Gate 1: independent policy-level wall-clock ceiling.
        let policy_wall_clock = async {
            match policy.max_wall_clock_ms {
                Some(ms) => tokio::time::sleep(Duration::from_millis(ms)).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::pin!(policy_wall_clock);
        let mut idle_deadline =
            tokio::time::Instant::now() + Duration::from_millis(idle_timeout_ms);
        let idle_sleep = tokio::time::sleep_until(idle_deadline);
        tokio::pin!(idle_sleep);
        let reconnect_hint = tokio::time::sleep(Duration::from_millis(RELAY_RECONNECT_HINT_MS));
        tokio::pin!(reconnect_hint);
        let mut heartbeat = tokio::time::interval(Duration::from_millis(HEARTBEAT_INTERVAL_MS));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let _ = heartbeat.tick().await;

        loop {
            tokio::select! {
                _ = &mut wall_clock => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    send_error(
                        &frame_tx,
                        "wall_clock_timeout",
                        format!(
                            "shell.exec wall-clock timeout after {timeout_ms} ms (policy '{}')",
                            policy.policy_id
                        ),
                    ).await;
                    return Ok(());
                }
                // PR#7b Gate 1: policy-level wall-clock ceiling, independent of request timeout.
                _ = &mut policy_wall_clock => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    let policy_ms = policy.max_wall_clock_ms.unwrap_or(0);
                    send_error(
                        &frame_tx,
                        "wall_clock_exceeded",
                        format!(
                            "shell.exec exceeded policy max_wall_clock_ms = {policy_ms} ms (policy '{}')",
                            policy.policy_id
                        ),
                    ).await;
                    return Ok(());
                }
                _ = &mut idle_sleep => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    send_error(
                        &frame_tx,
                        "idle_timeout",
                        format!(
                            "shell.exec idle timeout after {idle_timeout_ms} ms of no output (policy '{}')",
                            policy.policy_id
                        ),
                    ).await;
                    return Ok(());
                }
                _ = &mut reconnect_hint, if !reconnect_sent => {
                    reconnect_sent = true;
                    if send_frame(&frame_tx, StreamFrame::Reconnect {
                        reason: "relay-wall-clock".to_string(),
                        stdout_offset: stdout_total_bytes,
                        stderr_offset: stderr_total_bytes,
                    }).await.is_err() {
                        let _ = child.kill().await;
                        return Ok(());
                    }
                }
                _ = heartbeat.tick() => {
                    if send_frame(&frame_tx, StreamFrame::Heartbeat {
                        ts: chrono::Utc::now().timestamp_millis() as u64,
                        stdout_offset: Some(stdout_total_bytes),
                        stderr_offset: Some(stderr_total_bytes),
                    }).await.is_err() {
                        let _ = child.kill().await;
                        return Ok(());
                    }
                }
                wait_result = child.wait(), if exit_status.is_none() => {
                    match wait_result {
                        Ok(status) => {
                            exit_status = Some(status);
                            if !stdout_open && !stderr_open { break; }
                        }
                        Err(e) => {
                            send_error(&frame_tx, "wait_failed", format!("wait shell.exec failed: {e}")).await;
                            return Ok(());
                        }
                    }
                }
                read = stdout_reader.read(&mut stdout_buf), if stdout_open => {
                    match read {
                        Ok(0) => {
                            stdout_open = false;
                            if exit_status.is_some() && !stderr_open { break; }
                        }
                        Ok(n) => {
                            idle_deadline = tokio::time::Instant::now()
                                + Duration::from_millis(idle_timeout_ms);
                            idle_sleep.as_mut().reset(idle_deadline);
                            let offset = stdout_total_bytes;
                            stdout_total_bytes += n as u64;
                            stdout_hasher.update(&stdout_buf[..n]);
                            // PR#7b Gate 2: cumulative stdout+stderr byte cap.
                            if let Some(cap) = policy.max_output_bytes_total {
                                if stdout_total_bytes + stderr_total_bytes > cap {
                                    let _ = child.kill().await;
                                    let _ = child.wait().await;
                                    send_error(
                                        &frame_tx,
                                        "output_bytes_exceeded",
                                        format!(
                                            "shell.exec exceeded policy max_output_bytes_total = {cap} bytes (policy '{}')",
                                            policy.policy_id
                                        ),
                                    ).await;
                                    return Ok(());
                                }
                            }
                            let data_b64 = base64::engine::general_purpose::STANDARD
                                .encode(&stdout_buf[..n]);
                            let seq = stdout_seq;
                            stdout_seq += 1;
                            // back-pressure: this await blocks on slow consumer.
                            if send_frame(&frame_tx, StreamFrame::Stdout {
                                seq, offset, data_b64,
                            }).await.is_err() {
                                let _ = child.kill().await;
                                return Ok(());
                            }
                        }
                        Err(e) => {
                            send_error(&frame_tx, "read_failed",
                                format!("read shell.exec stdout failed: {e}")).await;
                            let _ = child.kill().await;
                            return Ok(());
                        }
                    }
                }
                read = stderr_reader.read(&mut stderr_buf), if stderr_open => {
                    match read {
                        Ok(0) => {
                            stderr_open = false;
                            if exit_status.is_some() && !stdout_open { break; }
                        }
                        Ok(n) => {
                            idle_deadline = tokio::time::Instant::now()
                                + Duration::from_millis(idle_timeout_ms);
                            idle_sleep.as_mut().reset(idle_deadline);
                            let offset = stderr_total_bytes;
                            stderr_total_bytes += n as u64;
                            stderr_hasher.update(&stderr_buf[..n]);
                            // PR#7b Gate 2: cumulative stdout+stderr byte cap.
                            if let Some(cap) = policy.max_output_bytes_total {
                                if stdout_total_bytes + stderr_total_bytes > cap {
                                    let _ = child.kill().await;
                                    let _ = child.wait().await;
                                    send_error(
                                        &frame_tx,
                                        "output_bytes_exceeded",
                                        format!(
                                            "shell.exec exceeded policy max_output_bytes_total = {cap} bytes (policy '{}')",
                                            policy.policy_id
                                        ),
                                    ).await;
                                    return Ok(());
                                }
                            }
                            let data_b64 = base64::engine::general_purpose::STANDARD
                                .encode(&stderr_buf[..n]);
                            let seq = stderr_seq;
                            stderr_seq += 1;
                            if send_frame(&frame_tx, StreamFrame::Stderr {
                                seq, offset, data_b64,
                            }).await.is_err() {
                                let _ = child.kill().await;
                                return Ok(());
                            }
                        }
                        Err(e) => {
                            send_error(&frame_tx, "read_failed",
                                format!("read shell.exec stderr failed: {e}")).await;
                            let _ = child.kill().await;
                            return Ok(());
                        }
                    }
                }
            }
        }

        let status = match exit_status {
            Some(s) => s,
            None => match child.wait().await {
                Ok(s) => s,
                Err(e) => {
                    send_error(
                        &frame_tx,
                        "wait_failed",
                        format!("wait shell.exec failed: {e}"),
                    )
                    .await;
                    return Ok(());
                }
            },
        };

        let stdout_sha256 = {
            let d = stdout_hasher.finish();
            let mut out = String::with_capacity(64);
            for b in d.as_ref() {
                out.push_str(&format!("{b:02x}"));
            }
            out
        };
        let stderr_sha256 = {
            let d = stderr_hasher.finish();
            let mut out = String::with_capacity(64);
            for b in d.as_ref() {
                out.push_str(&format!("{b:02x}"));
            }
            out
        };

        let _ = send_frame(
            &frame_tx,
            StreamFrame::Done {
                exit_code: status.code().unwrap_or(-1),
                total_stdout: stdout_total_bytes,
                total_stderr: stderr_total_bytes,
                stdout_sha256,
                stderr_sha256,
                duration_ms: start.elapsed().as_millis() as u64,
                stdout_object: None,
                stderr_object: None,
            },
        )
        .await;

        Ok(())
    }

    async fn execute_shell_exec<F, Fut>(
        &self,
        command: &RemoteCommand,
        on_stdout: &mut F,
    ) -> Result<RemoteInvokeResponse>
    where
        F: FnMut(Vec<u8>) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        let policy = self.resolve_shell_policy(command)?;

        if command.pty.as_ref().map(|pty| pty.enabled).unwrap_or(false)
            && !policy.interactive_allowed
        {
            return Err(BifrostError::Config(format!(
                "policy '{}' does not allow PTY/interactive shell execution",
                policy.policy_id
            )));
        }

        if command
            .stdin_mode
            .is_some_and(|mode| mode != super::types::StdinMode::None)
            && !policy.stdin_allowed
        {
            return Err(BifrostError::Config(format!(
                "policy '{}' does not allow stdin for shell.exec",
                policy.policy_id
            )));
        }

        let start = Instant::now();
        // PR#3a: split single wall-clock timeout into (wall_clock, idle).
        let wall_clock_timeout_ms: Option<u64> = match (command.timeout_ms, policy.max_timeout_ms) {
            (Some(c), Some(p)) => Some(c.min(p)),
            (Some(c), None) => Some(c),
            (None, Some(p)) => Some(p),
            (None, None) => None,
        };
        let idle_timeout_ms: u64 = policy.max_idle_ms.unwrap_or(DEFAULT_IDLE_TIMEOUT_MS);
        let timeout_ms = wall_clock_timeout_ms.unwrap_or(u64::MAX);
        let mut process = self.build_shell_exec_process(command, &policy)?;
        process.stdout(Stdio::piped());
        process.stderr(Stdio::piped());
        process.stdin(Stdio::null());
        process.kill_on_drop(true);

        let mut child = process
            .spawn()
            .map_err(|e| BifrostError::Network(format!("spawn shell.exec failed: {}", e)))?;
        let mut stdout_reader = child.stdout.take().ok_or_else(|| {
            BifrostError::Network("shell.exec stdout pipe unavailable".to_string())
        })?;
        let mut stderr_reader = child.stderr.take().ok_or_else(|| {
            BifrostError::Network("shell.exec stderr pipe unavailable".to_string())
        })?;

        let mut stdout_buf = [0u8; 65536]; // PR#2: 4 KiB -> 64 KiB to reduce syscall / SSE-event overhead on large outputs
        let mut stderr_buf = [0u8; 65536]; // PR#2: 4 KiB -> 64 KiB, see stdout_buf rationale
        let mut stdout_open = true;
        let mut stderr_open = true;
        let mut exit_status = None;
        let mut stdout_preview = Vec::new();
        let mut stderr_preview = Vec::new();
        // // PR#2 source-of-truth hashing: track full byte totals + SHA-256 of
        // the *complete* stdout/stderr streams, independent of the capped
        // `stdout_preview` / `stderr_preview` inline buffers. This lets
        // callers detect truncation and verify reassembled streams even
        // when the inline preview was clipped by `max_output_bytes`.
        let mut stdout_total_bytes: u64 = 0;
        let mut stderr_total_bytes: u64 = 0;
        let mut stdout_hasher = ring::digest::Context::new(&ring::digest::SHA256);
        let mut stderr_hasher = ring::digest::Context::new(&ring::digest::SHA256);
        // PR#3a: wall-clock future (optional) + idle future + heartbeat tick.
        let wall_clock = async {
            match wall_clock_timeout_ms {
                Some(ms) => tokio::time::sleep(Duration::from_millis(ms)).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::pin!(wall_clock);
        // PR#7b Gate 1 (legacy path): independent policy-level wall-clock ceiling.
        let policy_wall_clock = async {
            match policy.max_wall_clock_ms {
                Some(ms) => tokio::time::sleep(Duration::from_millis(ms)).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::pin!(policy_wall_clock);
        let mut idle_deadline =
            tokio::time::Instant::now() + Duration::from_millis(idle_timeout_ms);
        let idle_sleep = tokio::time::sleep_until(idle_deadline);
        tokio::pin!(idle_sleep);
        let mut heartbeat = tokio::time::interval(Duration::from_millis(HEARTBEAT_INTERVAL_MS));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let _ = heartbeat.tick().await; // swallow immediate tick

        loop {
            tokio::select! {
                _ = &mut wall_clock => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    return Err(BifrostError::Network(format!(
                        "shell.exec wall-clock timeout after {} ms (policy '{}')",
                        timeout_ms, policy.policy_id
                    )));
                }
                // PR#7b Gate 1 (legacy path): policy-level wall-clock ceiling.
                _ = &mut policy_wall_clock => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    let policy_ms = policy.max_wall_clock_ms.unwrap_or(0);
                    return Err(BifrostError::Network(format!(
                        "shell.exec exceeded policy max_wall_clock_ms = {} ms (policy '{}')",
                        policy_ms, policy.policy_id
                    )));
                }
                _ = &mut idle_sleep => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    return Err(BifrostError::Network(format!(
                        "shell.exec idle timeout after {} ms of no output (policy '{}')",
                        idle_timeout_ms, policy.policy_id
                    )));
                }
                _ = heartbeat.tick() => {
                    // PR#3a: hook for PR#3b StreamFrame::Heartbeat. Does
                    // NOT reset idle_deadline — liveness must come from
                    // actual process output, not from our own tick.
                }
                wait_result = child.wait(), if exit_status.is_none() => {
                    exit_status = Some(wait_result.map_err(|e| {
                        BifrostError::Network(format!("wait shell.exec failed: {}", e))
                    })?);
                    if !stdout_open && !stderr_open {
                        break;
                    }
                }
                read = stdout_reader.read(&mut stdout_buf), if stdout_open => {
                    let read = read.map_err(|e| {
                        BifrostError::Network(format!("read shell.exec stdout failed: {}", e))
                    })?;
                    if read == 0 {
                        stdout_open = false;
                        if exit_status.is_some() && !stderr_open {
                            break;
                        }
                    } else {
                        // PR#3a: real stdout read resets the idle deadline.
                        idle_deadline = tokio::time::Instant::now()
                            + Duration::from_millis(idle_timeout_ms);
                        idle_sleep.as_mut().reset(idle_deadline);
                        // PR#2: hash + total BEFORE the cap
                        stdout_total_bytes += read as u64;
                        stdout_hasher.update(&stdout_buf[..read]);
                        // PR#7b Gate 2 (legacy path): cumulative stdout+stderr byte cap.
                        if let Some(cap) = policy.max_output_bytes_total {
                            if stdout_total_bytes + stderr_total_bytes > cap {
                                let _ = child.kill().await;
                                let _ = child.wait().await;
                                return Err(BifrostError::Network(format!(
                                    "shell.exec exceeded policy max_output_bytes_total = {} bytes (policy '{}')",
                                    cap, policy.policy_id
                                )));
                            }
                        }
                        append_truncated_bytes(
                            &mut stdout_preview,
                            &stdout_buf[..read],
                            policy.max_output_bytes,
                        );
                        on_stdout(stdout_buf[..read].to_vec()).await?;
                    }
                }
                read = stderr_reader.read(&mut stderr_buf), if stderr_open => {
                    let read = read.map_err(|e| {
                        BifrostError::Network(format!("read shell.exec stderr failed: {}", e))
                    })?;
                    if read == 0 {
                        stderr_open = false;
                        if exit_status.is_some() && !stdout_open {
                            break;
                        }
                    } else {
                        // PR#3a: real stderr read resets the idle deadline.
                        idle_deadline = tokio::time::Instant::now()
                            + Duration::from_millis(idle_timeout_ms);
                        idle_sleep.as_mut().reset(idle_deadline);
                        // PR#2: hash + total BEFORE the cap
                        stderr_total_bytes += read as u64;
                        stderr_hasher.update(&stderr_buf[..read]);
                        // PR#7b Gate 2 (legacy path): cumulative stdout+stderr byte cap.
                        if let Some(cap) = policy.max_output_bytes_total {
                            if stdout_total_bytes + stderr_total_bytes > cap {
                                let _ = child.kill().await;
                                let _ = child.wait().await;
                                return Err(BifrostError::Network(format!(
                                    "shell.exec exceeded policy max_output_bytes_total = {} bytes (policy '{}')",
                                    cap, policy.policy_id
                                )));
                            }
                        }
                        append_truncated_bytes(
                            &mut stderr_preview,
                            &stderr_buf[..read],
                            policy.max_output_bytes,
                        );
                    }
                }
            }
        }

        let status = if let Some(status) = exit_status {
            status
        } else {
            child
                .wait()
                .await
                .map_err(|e| BifrostError::Network(format!("wait shell.exec failed: {}", e)))?
        };
        let stdout = String::from_utf8_lossy(&stdout_preview).into_owned();
        let stderr = String::from_utf8_lossy(&stderr_preview).into_owned();

        // PR#2: source-of-truth fields independent of the inline preview.
        let stdout_sha256_full_hex = {
            let digest = stdout_hasher.finish();
            if stdout_total_bytes > 0 {
                let mut out = String::with_capacity(64);
                for b in digest.as_ref() {
                    out.push_str(&format!("{b:02x}"));
                }
                Some(out)
            } else {
                None
            }
        };
        let stderr_sha256_full_hex = {
            let digest = stderr_hasher.finish();
            if stderr_total_bytes > 0 {
                let mut out = String::with_capacity(64);
                for b in digest.as_ref() {
                    out.push_str(&format!("{b:02x}"));
                }
                Some(out)
            } else {
                None
            }
        };
        let stdout_truncated_flag = (stdout_total_bytes as usize) > stdout_preview.len();
        let stderr_truncated_flag = (stderr_total_bytes as usize) > stderr_preview.len();
        Ok(RemoteInvokeResponse {
            exit_code: status.code().unwrap_or(-1),
            stdout: (!stdout.is_empty()).then_some(stdout.clone()),
            stderr: (!stderr.is_empty()).then_some(stderr.clone()),
            stdout_digest: (!stdout.is_empty()).then(|| sha1_hex(&stdout)),
            stderr_digest: (!stderr.is_empty()).then(|| sha1_hex(&stderr)),
            duration_ms: start.elapsed().as_millis() as u64,
            stdout_total_bytes: (stdout_total_bytes > 0).then_some(stdout_total_bytes),
            stderr_total_bytes: (stderr_total_bytes > 0).then_some(stderr_total_bytes),
            stdout_sha256_full: stdout_sha256_full_hex,
            stderr_sha256_full: stderr_sha256_full_hex,
            stdout_truncated: Some(stdout_truncated_flag),
            stderr_truncated: Some(stderr_truncated_flag),
        })
    }

    fn build_shell_exec_process(
        &self,
        command: &RemoteCommand,
        policy: &ResolvedShellPolicy,
    ) -> Result<TokioCommand> {
        Self::validate_shell_command_against_policy(policy, command)?;
        if let Some(reason) = &policy.reject_reason {
            return Err(BifrostError::Config(format!(
                "policy '{}' is not executable: {}",
                policy.policy_id, reason
            )));
        }

        let exec_mode = command
            .exec_mode
            .ok_or_else(|| BifrostError::Config("shell.exec requires exec_mode".to_string()))?;

        let mut process = match exec_mode {
            ShellExecMode::ArgvExec => {
                let argv = command.argv.as_ref().ok_or_else(|| {
                    BifrostError::Config("shell.exec argv_exec requires argv".to_string())
                })?;
                let (program, args) = argv.split_first().ok_or_else(|| {
                    BifrostError::Config("shell.exec argv_exec requires non-empty argv".to_string())
                })?;
                let mut process = TokioCommand::new(program);
                process.args(args);
                process
            }
            ShellExecMode::ShellText | ShellExecMode::Template => {
                let shell_text = command.command_text.as_deref().ok_or_else(|| {
                    BifrostError::Config("shell.exec requires command_text".to_string())
                })?;
                build_shell_text_process(
                    command.shell.as_deref().or(policy.shell.as_deref()),
                    shell_text,
                )
            }
        };

        let effective_cwd = command.cwd.as_ref().or(policy.default_cwd.as_ref());
        if let Some(cwd) = effective_cwd {
            process.current_dir(cwd);
        }
        if !policy.inherit_env {
            process.env_clear();
            // On Windows, processes (especially .NET apps like PowerShell) need
            // critical system env vars to start. Preserve them after env_clear().
            #[cfg(windows)]
            {
                for key in &["SystemRoot", "SystemDrive"] {
                    if let Ok(val) = std::env::var(key) {
                        process.env(key, &val);
                    }
                }
            }
        }
        process.envs(policy.default_env.clone());
        if let Some(env) = &command.env {
            process.envs(env);
        }

        Ok(process)
    }

    pub fn select_policy_id_for_command(
        &self,
        command: &RemoteCommand,
        binding: Option<&serde_json::Value>,
    ) -> Result<String> {
        let store = RemoteShellStore::new()?;
        let set = store.load()?;
        let candidate_ids = Self::candidate_policy_ids(&set, binding)?;
        let mut matching_ids = Vec::new();
        let mut candidate_errors: Vec<(String, BifrostError)> = Vec::new();

        for policy_id in candidate_ids {
            let policy = Self::resolve_shell_policy_from_set(&set, &policy_id)?;
            match Self::validate_shell_command_against_policy(&policy, command) {
                Ok(()) => matching_ids.push(policy.policy_id),
                Err(error) => candidate_errors.push((policy_id, error)),
            }
        }

        match matching_ids.len() {
            1 => Ok(matching_ids.remove(0)),
            0 => {
                if candidate_errors.len() == 1 {
                    return Err(candidate_errors.remove(0).1);
                }
                let candidate_list = candidate_errors
                    .iter()
                    .map(|(policy_id, _)| policy_id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                Err(BifrostError::Config(format!(
                    "target Shell Access did not find a unique policy for this command among [{}]",
                    candidate_list
                )))
            }
            _ => Err(BifrostError::Config(format!(
                "target Shell Access matched multiple policies for this command: {}",
                matching_ids.join(", ")
            ))),
        }
    }

    fn resolve_shell_policy(&self, command: &RemoteCommand) -> Result<ResolvedShellPolicy> {
        let policy_id = command
            .policy_id
            .as_deref()
            .ok_or_else(|| BifrostError::Config("shell.exec requires policy_id".to_string()))?;
        let store = RemoteShellStore::new()?;
        let set = store.load()?;
        Self::resolve_shell_policy_from_set(&set, policy_id)
    }

    fn candidate_policy_ids(
        set: &RemoteShellSet,
        binding: Option<&serde_json::Value>,
    ) -> Result<Vec<String>> {
        let enabled_policy_ids = set
            .policies
            .iter()
            .filter(|policy| policy.enabled)
            .map(|policy| policy.id.clone())
            .collect::<Vec<_>>();
        if enabled_policy_ids.is_empty() {
            return Err(BifrostError::Config(
                "no enabled shell policy exists on this device".to_string(),
            ));
        }

        let Some(binding) = binding else {
            return Ok(enabled_policy_ids);
        };

        let mode = binding
            .get("mode")
            .and_then(|value| value.as_str())
            .unwrap_or("all");
        if mode == "all" {
            return Ok(enabled_policy_ids);
        }

        let Some(policy_ids) = binding.get("policy_ids").and_then(|value| value.as_array()) else {
            return Err(BifrostError::Config(
                "shell policy binding requires a non-empty policy_ids array".to_string(),
            ));
        };

        let selected = policy_ids
            .iter()
            .filter_map(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        if selected.is_empty() {
            return Err(BifrostError::Config(
                "shell policy binding requires at least one policy id".to_string(),
            ));
        }

        Ok(selected)
    }

    fn resolve_shell_policy_from_set(
        set: &RemoteShellSet,
        policy_id: &str,
    ) -> Result<ResolvedShellPolicy> {
        let policy = set.find_policy(policy_id).ok_or_else(|| {
            BifrostError::Config(format!("remote shell policy '{}' not found", policy_id))
        })?;
        if !policy.enabled {
            return Err(BifrostError::Config(format!(
                "remote shell policy '{}' is disabled",
                policy_id
            )));
        }

        let policy_meta: ShellPolicyMetadata = serde_json::from_value(policy.metadata.clone())
            .map_err(|error| {
                BifrostError::Config(format!(
                    "parse metadata for remote shell policy '{}': {}",
                    policy_id, error
                ))
            })?;
        let profile_meta = if let Some(profile_id) = policy.profile_id.as_deref() {
            let profile = set.find_profile(profile_id).ok_or_else(|| {
                BifrostError::Config(format!(
                    "remote shell profile '{}' referenced by policy '{}' was not found",
                    profile_id, policy_id
                ))
            })?;
            if !profile.enabled {
                return Err(BifrostError::Config(format!(
                    "remote shell profile '{}' is disabled",
                    profile_id
                )));
            }
            serde_json::from_value::<ShellPolicyMetadata>(profile.metadata.clone()).map_err(
                |error| {
                    BifrostError::Config(format!(
                        "parse metadata for remote shell profile '{}': {}",
                        profile_id, error
                    ))
                },
            )?
        } else {
            ShellPolicyMetadata::default()
        };

        let exec_mode = policy_meta
            .exec_mode
            .or(profile_meta.exec_mode)
            .ok_or_else(|| {
                BifrostError::Config(format!(
                    "remote shell policy '{}' is missing metadata.exec_mode",
                    policy_id
                ))
            })?;

        let mut allowed_exec_modes = if policy_meta.allowed_exec_modes.is_empty() {
            profile_meta.allowed_exec_modes
        } else {
            policy_meta.allowed_exec_modes
        };
        if allowed_exec_modes.is_empty() {
            let can_argv = policy_meta
                .allow_any_executable
                .or(profile_meta.allow_any_executable)
                .unwrap_or(false)
                || !policy_meta.allowed_executables.is_empty()
                || !profile_meta.allowed_executables.is_empty();
            let can_shell = !policy_meta.allowed_shell_patterns.is_empty()
                || !profile_meta.allowed_shell_patterns.is_empty();
            if can_argv {
                allowed_exec_modes.push(ShellExecMode::ArgvExec);
            }
            if can_shell {
                allowed_exec_modes.push(ShellExecMode::ShellText);
            }
            if !allowed_exec_modes.contains(&exec_mode) {
                allowed_exec_modes.push(exec_mode);
            }
        } else {
            dedupe_shell_exec_modes(&mut allowed_exec_modes);
        }

        let resolved = ResolvedShellPolicy {
            policy_id: policy_id.to_string(),
            allowed_exec_modes,
            reject_reason: policy_meta.reject_reason.or(profile_meta.reject_reason),
            allow_any_executable: policy_meta
                .allow_any_executable
                .or(profile_meta.allow_any_executable)
                .unwrap_or(false),
            allowed_executables: if policy_meta.allowed_executables.is_empty() {
                profile_meta.allowed_executables
            } else {
                policy_meta.allowed_executables
            },
            allowed_shell_patterns: if policy_meta.allowed_shell_patterns.is_empty() {
                profile_meta.allowed_shell_patterns
            } else {
                policy_meta.allowed_shell_patterns
            },
            cwd_allowlist: if policy_meta.cwd_allowlist.is_empty() {
                profile_meta.cwd_allowlist
            } else {
                policy_meta.cwd_allowlist
            },
            env_allowlist: if policy_meta.env_allowlist.is_empty() {
                profile_meta.env_allowlist
            } else {
                policy_meta.env_allowlist
            },
            default_cwd: policy_meta.default_cwd.or(profile_meta.default_cwd),
            shell: policy_meta.shell.or(profile_meta.shell),
            max_timeout_ms: policy_meta.max_timeout_ms.or(profile_meta.max_timeout_ms),
            max_idle_ms: policy_meta.max_idle_ms.or(profile_meta.max_idle_ms),
            max_output_bytes: policy_meta
                .max_output_bytes
                .or(profile_meta.max_output_bytes)
                .unwrap_or(DEFAULT_SHELL_OUTPUT_MAX_BYTES),
            max_wall_clock_ms: policy_meta
                .max_wall_clock_ms
                .or(profile_meta.max_wall_clock_ms),
            max_output_bytes_total: policy_meta
                .max_output_bytes_total
                .or(profile_meta.max_output_bytes_total),
            allow_resume: policy_meta
                .allow_resume
                .or(profile_meta.allow_resume)
                .unwrap_or(false),
            stdin_allowed: policy_meta
                .stdin_allowed
                .unwrap_or(profile_meta.stdin_allowed.unwrap_or(false)),
            interactive_allowed: policy_meta
                .interactive_allowed
                .unwrap_or(profile_meta.interactive_allowed.unwrap_or(false)),
            inherit_env: policy_meta
                .inherit_env
                .unwrap_or(profile_meta.inherit_env.unwrap_or(false)),
            default_env: if policy_meta.default_env.is_empty() {
                profile_meta.default_env
            } else {
                policy_meta.default_env
            },
        };

        Ok(resolved)
    }

    fn validate_shell_command_against_policy(
        policy: &ResolvedShellPolicy,
        command: &RemoteCommand,
    ) -> Result<()> {
        let exec_mode = command
            .exec_mode
            .ok_or_else(|| BifrostError::Config("shell.exec requires exec_mode".to_string()))?;
        if !policy.allowed_exec_modes.contains(&exec_mode) {
            let allowed_modes = policy
                .allowed_exec_modes
                .iter()
                .map(shell_exec_mode_label)
                .collect::<Vec<_>>()
                .join(", ");
            return Err(BifrostError::Config(format!(
                "policy '{}' allows exec_mode [{}], got {}. Use '--shell-text <cmd>' for shell text, or 'bifrost remote exec -- <program> [args...]' for argv execution.",
                policy.policy_id,
                allowed_modes,
                shell_exec_mode_label(&exec_mode)
            )));
        }
        if command.pty.as_ref().map(|pty| pty.enabled).unwrap_or(false)
            && !policy.interactive_allowed
        {
            return Err(BifrostError::Config(format!(
                "policy '{}' does not allow PTY/interactive shell execution",
                policy.policy_id
            )));
        }
        if command
            .stdin_mode
            .is_some_and(|mode| mode != super::types::StdinMode::None)
            && !policy.stdin_allowed
        {
            return Err(BifrostError::Config(format!(
                "policy '{}' does not allow stdin for shell.exec",
                policy.policy_id
            )));
        }

        match exec_mode {
            ShellExecMode::ArgvExec => {
                let argv = command.argv.as_ref().ok_or_else(|| {
                    BifrostError::Config("shell.exec argv_exec requires argv".to_string())
                })?;
                let (program, _) = argv.split_first().ok_or_else(|| {
                    BifrostError::Config("shell.exec argv_exec requires non-empty argv".to_string())
                })?;
                if !policy.allow_any_executable
                    && !policy
                        .allowed_executables
                        .iter()
                        .any(|allowed| allowed == program)
                {
                    return Err(BifrostError::Config(format!(
                        "program '{}' is not allowed by policy '{}'",
                        program, policy.policy_id
                    )));
                }
            }
            ShellExecMode::ShellText | ShellExecMode::Template => {
                let shell_text = command.command_text.as_deref().ok_or_else(|| {
                    BifrostError::Config("shell.exec requires command_text".to_string())
                })?;
                let allowed = policy
                    .allowed_shell_patterns
                    .iter()
                    .any(|pattern| Regex::new(pattern).is_ok_and(|re| re.is_match(shell_text)));
                if !allowed {
                    return Err(BifrostError::Config(format!(
                        "shell_text does not match any allowlist rule in policy '{}'",
                        policy.policy_id
                    )));
                }
            }
        }

        let effective_cwd = command.cwd.as_ref().or(policy.default_cwd.as_ref());
        if let Some(cwd) = effective_cwd {
            if !policy.cwd_allowlist.is_empty()
                && !policy
                    .cwd_allowlist
                    .iter()
                    .any(|allowed| path_is_within(cwd, allowed))
            {
                return Err(BifrostError::Config(format!(
                    "cwd '{}' is not allowed by policy '{}'",
                    cwd, policy.policy_id
                )));
            }
        }
        if let Some(env) = &command.env {
            for key in env.keys() {
                if !policy.env_allowlist.is_empty()
                    && !policy.env_allowlist.iter().any(|allowed| allowed == key)
                {
                    return Err(BifrostError::Config(format!(
                        "environment key '{}' is not allowed by policy '{}'",
                        key, policy.policy_id
                    )));
                }
            }
        }

        Ok(())
    }

    async fn dispatch_with_stdout_sink<F, Fut>(
        &self,
        command: &str,
        _args: &CommandArgs,
        on_stdout: &mut F,
    ) -> Result<String>
    where
        F: FnMut(Vec<u8>) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        match command {
            "status" => {
                let body = self.get_status().await?;
                self.emit_stdout(on_stdout, body).await
            }
            _ => Err(BifrostError::Config(format!(
                "unhandled command: {}",
                command
            ))),
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}:{}", self.admin_host, self.admin_port)
    }

    async fn get_status(&self) -> Result<String> {
        let url = format!("{}/_bifrost/api/system", self.base_url());
        self.http_get(&url, "status").await
    }

    async fn list_traffic(&self, args: &CommandArgs) -> Result<String> {
        let mut params: Vec<(String, String)> = Vec::new();

        let limit = args.limit.unwrap_or(50).min(MAX_TRAFFIC_LIST_LIMIT);
        params.push(("limit".to_string(), limit.to_string()));

        if let Some(cursor) = args.cursor {
            params.push(("cursor".to_string(), cursor.to_string()));
        }
        if let Some(ref d) = args.direction {
            params.push(("direction".to_string(), d.clone()));
        }
        if let Some(ref m) = args.method {
            params.push(("method".to_string(), m.clone()));
        }
        if let Some(v) = args.status {
            params.push(("status".to_string(), v.to_string()));
        }
        if let Some(v) = args.status_min {
            params.push(("status_min".to_string(), v.to_string()));
        }
        if let Some(v) = args.status_max {
            params.push(("status_max".to_string(), v.to_string()));
        }
        if let Some(ref p) = args.protocol {
            params.push(("protocol".to_string(), p.clone()));
        }
        if let Some(ref h) = args.host {
            params.push(("host_contains".to_string(), h.clone()));
        }
        if let Some(ref u) = args.url {
            params.push(("url_contains".to_string(), u.clone()));
        }
        if let Some(ref p) = args.path {
            params.push(("path_contains".to_string(), p.clone()));
        }
        if let Some(ref ct) = args.content_type {
            params.push(("content_type".to_string(), ct.clone()));
        }
        if let Some(ref ip) = args.client_ip {
            params.push(("client_ip".to_string(), ip.clone()));
        }
        if let Some(ref app) = args.client_app {
            params.push(("client_app".to_string(), app.clone()));
        }
        if let Some(v) = args.has_rule_hit {
            params.push(("has_rule_hit".to_string(), v.to_string()));
        }
        if let Some(v) = args.is_websocket {
            params.push(("is_websocket".to_string(), v.to_string()));
        }
        if let Some(v) = args.is_sse {
            params.push(("is_sse".to_string(), v.to_string()));
        }
        if let Some(v) = args.is_tunnel {
            params.push(("is_tunnel".to_string(), v.to_string()));
        }

        let qs = params
            .iter()
            .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&");

        let url = format!("{}/_bifrost/api/traffic?{}", self.base_url(), qs);
        self.http_get(&url, "traffic.list").await
    }

    async fn get_traffic(&self, id: &str, args: &CommandArgs) -> Result<String> {
        let resolved_id = self.resolve_traffic_id(id).await?;
        let url = format!(
            "{}/_bifrost/api/traffic/{}",
            self.base_url(),
            urlencoding::encode(&resolved_id)
        );
        let detail_body = self.http_get(&url, "traffic.get").await?;

        let include_req_body = args.request_body.unwrap_or(false);
        let include_res_body = args.response_body.unwrap_or(false);

        if !include_req_body && !include_res_body {
            return Ok(detail_body);
        }

        let mut detail: serde_json::Value = serde_json::from_str(&detail_body)
            .map_err(|e| BifrostError::Config(format!("failed to parse traffic detail: {}", e)))?;

        if include_req_body {
            let req_url = format!(
                "{}/_bifrost/api/traffic/{}/request-body",
                self.base_url(),
                urlencoding::encode(&resolved_id)
            );
            if let Ok(body_text) = self.http_get(&req_url, "traffic.get/request-body").await {
                if let Ok(body_val) = serde_json::from_str::<serde_json::Value>(&body_text) {
                    detail["request_body"] = body_val;
                }
            }
        }

        if include_res_body {
            let res_url = format!(
                "{}/_bifrost/api/traffic/{}/response-body",
                self.base_url(),
                urlencoding::encode(&resolved_id)
            );
            if let Ok(body_text) = self.http_get(&res_url, "traffic.get/response-body").await {
                if let Ok(body_val) = serde_json::from_str::<serde_json::Value>(&body_text) {
                    detail["response_body"] = body_val;
                }
            }
        }

        serde_json::to_string(&detail)
            .map_err(|e| BifrostError::Config(format!("failed to serialize traffic detail: {}", e)))
    }

    async fn http_get(&self, url: &str, label: &str) -> Result<String> {
        let resp = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("{} request failed: {}", label, e)))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| BifrostError::Network(format!("failed to read {} body: {}", label, e)))?;
        if !status.is_success() {
            return Err(BifrostError::Network(format!(
                "{} returned HTTP {}: {}",
                label, status, body
            )));
        }
        Ok(body)
    }

    async fn emit_stdout<F, Fut>(&self, on_stdout: &mut F, body: String) -> Result<String>
    where
        F: FnMut(Vec<u8>) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        if !body.is_empty() {
            on_stdout(body.clone().into_bytes()).await?;
        }
        Ok(body)
    }

    async fn resolve_traffic_id(&self, id: &str) -> Result<String> {
        if !id.chars().all(|c| c.is_ascii_digit()) {
            return Ok(id.to_string());
        }

        let seq: u64 = id
            .parse()
            .map_err(|_| BifrostError::Parse(format!("invalid traffic sequence: {}", id)))?;

        if let Some(exact) = self.find_id_by_exact_sequence(seq).await? {
            return Ok(exact);
        }

        let server_seq = self.fetch_server_sequence().await?;
        let Some(modulus) = 10u64.checked_pow(id.len() as u32) else {
            return Err(BifrostError::NotFound(format!(
                "No traffic record with sequence suffix '{}' found",
                id
            )));
        };

        let suffix = seq % modulus;
        let mut candidate = suffix;
        let mut candidates = Vec::new();
        while candidate <= server_seq {
            candidates.push(candidate);
            match candidate.checked_add(modulus) {
                Some(next) => candidate = next,
                None => break,
            }
        }
        candidates.reverse();

        for candidate_seq in candidates {
            if let Some(found) = self.find_id_by_exact_sequence(candidate_seq).await? {
                return Ok(found);
            }
        }

        Err(BifrostError::NotFound(format!(
            "No traffic record with sequence suffix '{}' found",
            id
        )))
    }

    async fn fetch_server_sequence(&self) -> Result<u64> {
        let url = format!("{}/_bifrost/api/traffic?limit=1", self.base_url());
        let body = self.http_get(&url, "traffic.get/server-sequence").await?;
        let parsed: TrafficQueryResult = serde_json::from_str(&body)
            .map_err(|e| BifrostError::Parse(format!("failed to parse traffic list: {}", e)))?;
        Ok(parsed.server_sequence)
    }

    async fn find_id_by_exact_sequence(&self, seq: u64) -> Result<Option<String>> {
        let cursor = seq.saturating_add(1);
        let url = format!(
            "{}/_bifrost/api/traffic?limit=1&cursor={}&direction=backward",
            self.base_url(),
            cursor
        );
        let body = self.http_get(&url, "traffic.get/sequence-lookup").await?;
        let parsed: TrafficQueryResult = serde_json::from_str(&body)
            .map_err(|e| BifrostError::Parse(format!("failed to parse traffic lookup: {}", e)))?;
        Ok(parsed
            .records
            .into_iter()
            .next()
            .filter(|record| record.seq == seq)
            .map(|record| record.id))
    }

    async fn search_stream_via_service<F, Fut>(
        &self,
        args: &SearchArgs,
        on_stdout: &mut F,
    ) -> Result<String>
    where
        F: FnMut(Vec<u8>) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        let service = self
            .query_service
            .as_ref()
            .ok_or_else(|| BifrostError::Config("query service not available".to_string()))?
            .clone();

        let (tx, mut rx) = tokio::sync::mpsc::channel::<SearchServiceEvent>(64);
        let service_args = args.clone();
        let tx_results = tx.clone();
        let tx_progress = tx.clone();

        let worker = tokio::spawn(async move {
            service
                .search_stream(
                    &service_args,
                    move |item| {
                        let _ = tx_results
                            .blocking_send(SearchServiceEvent::Result(Box::new(item.clone())));
                    },
                    move |progress| {
                        let _ = tx_progress
                            .blocking_send(SearchServiceEvent::Progress(progress.clone()));
                    },
                )
                .await
        });
        drop(tx);

        let mut full_output = String::new();
        while let Some(event) = rx.recv().await {
            match event {
                SearchServiceEvent::Result(item) => {
                    let payload = serde_json::to_string(&item).map_err(|e| {
                        BifrostError::Config(format!("serialize search result event: {e}"))
                    })?;
                    emit_search_chunk(&mut full_output, on_stdout, sse_event("result", &payload))
                        .await?;
                }
                SearchServiceEvent::Progress(progress) => {
                    let payload = serde_json::json!({
                        "total_searched": progress.total_searched,
                        "total_matched": progress.total_matched,
                        "next_cursor": progress.cursor,
                        "has_more_hint": progress.has_more_hint,
                        "iterations": progress.iterations,
                    });
                    emit_search_chunk(
                        &mut full_output,
                        on_stdout,
                        sse_event("progress", &payload.to_string()),
                    )
                    .await?;
                }
            }
        }

        let response = worker
            .await
            .map_err(|e| BifrostError::Config(format!("search stream worker join failed: {e}")))?;

        match response {
            Ok(response) => {
                let payload = serde_json::json!({
                    "total_searched": response.total_searched,
                    "total_matched": response.total_matched,
                    "next_cursor": response.next_cursor,
                    "has_more": response.has_more,
                    "search_id": response.search_id,
                });
                emit_search_chunk(
                    &mut full_output,
                    on_stdout,
                    sse_event("done", &payload.to_string()),
                )
                .await?;
            }
            Err(error) => {
                let payload = serde_json::json!({ "message": error.to_string() });
                emit_search_chunk(
                    &mut full_output,
                    on_stdout,
                    sse_event("error", &payload.to_string()),
                )
                .await?;
            }
        }

        Ok(full_output)
    }

    async fn search_stream<F, Fut>(
        &self,
        query: &str,
        legacy_limit: Option<usize>,
        max_results: Option<usize>,
        max_scan: Option<usize>,
        on_stdout: &mut F,
    ) -> Result<String>
    where
        F: FnMut(Vec<u8>) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        let url = format!("{}/_bifrost/api/search/stream", self.base_url());
        let search_limit = max_results
            .or(legacy_limit)
            .unwrap_or(50)
            .min(MAX_TRAFFIC_LIST_LIMIT);
        let payload = serde_json::json!({
            "keyword": query,
            "max_results": search_limit,
            "max_scan": max_scan,
        });
        let resp = self
            .http
            .post(&url)
            .timeout(Duration::from_secs(SEARCH_STREAM_TIMEOUT_SECS))
            .json(&payload)
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("search stream request failed: {}", e)))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(BifrostError::Network(format!(
                "search stream returned HTTP {}: {}",
                status, body
            )));
        }

        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        if !content_type.contains("text/event-stream") {
            return Err(BifrostError::Network(format!(
                "search stream returned unexpected content-type: {}",
                content_type
            )));
        }

        let mut full_output = String::new();
        let mut stream = resp.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let bytes = chunk
                .map_err(|e| BifrostError::Network(format!("search stream read failed: {}", e)))?;
            emit_search_chunk(
                &mut full_output,
                on_stdout,
                String::from_utf8_lossy(&bytes).into_owned(),
            )
            .await?;
        }

        Ok(full_output)
    }
}

fn shell_exec_mode_label(mode: &ShellExecMode) -> &'static str {
    match mode {
        ShellExecMode::ArgvExec => "argv_exec",
        ShellExecMode::ShellText => "shell_text",
        ShellExecMode::Template => "template",
    }
}

fn dedupe_shell_exec_modes(modes: &mut Vec<ShellExecMode>) {
    let mut deduped = Vec::new();
    for mode in modes.drain(..) {
        if !deduped.contains(&mode) {
            deduped.push(mode);
        }
    }
    *modes = deduped;
}

fn build_shell_text_process(shell: Option<&str>, shell_text: &str) -> TokioCommand {
    #[cfg(windows)]
    {
        // Resolve the effective shell: skip Unix-style paths that don't exist on Windows
        // (e.g. "/bin/bash" from a cross-platform default) and fall back to cmd.
        let effective_shell = shell.filter(|s| !is_unix_only_shell_path(s));

        if let Some(shell) = effective_shell {
            let mut command = TokioCommand::new(shell);
            if windows_shell_uses_command_flag(shell) {
                // Force UTF-8 output encoding for PowerShell to avoid garbled
                // non-ASCII text (e.g. Chinese chars from ipconfig).
                let utf8_shell_text = format!(
                    "[Console]::OutputEncoding = [Text.Encoding]::UTF8; {}",
                    shell_text
                );
                command
                    .arg("-NoLogo")
                    .arg("-NoProfile")
                    .arg("-Command")
                    .arg(&utf8_shell_text);
            } else if windows_shell_uses_login_flag(shell) {
                command.arg("-lc").arg(shell_text);
            } else {
                // cmd.exe: prepend chcp 65001 to switch console to UTF-8
                let utf8_shell_text = format!("chcp 65001 > nul && {}", shell_text);
                command.arg("/C").arg(&utf8_shell_text);
            }
            return command;
        }

        // Default: cmd with UTF-8 code page
        let utf8_shell_text = format!("chcp 65001 > nul && {}", shell_text);
        let mut command = TokioCommand::new("cmd");
        command.arg("/C").arg(&utf8_shell_text);
        command
    }

    #[cfg(not(windows))]
    {
        let shell = shell.unwrap_or("/bin/sh");
        let mut command = TokioCommand::new(shell);
        command.arg("-lc").arg(shell_text);
        command
    }
}

/// Returns true if the shell path looks like a Unix-only absolute path
/// (e.g. "/bin/bash", "/usr/bin/zsh") that would never exist on Windows.
#[cfg(windows)]
fn is_unix_only_shell_path(shell: &str) -> bool {
    shell.starts_with('/')
}

#[cfg(windows)]
fn windows_shell_uses_command_flag(shell: &str) -> bool {
    windows_shell_file_name(shell).is_some_and(|name| {
        matches!(
            name.as_str(),
            "powershell" | "powershell.exe" | "pwsh" | "pwsh.exe"
        )
    })
}

#[cfg(windows)]
fn windows_shell_uses_login_flag(shell: &str) -> bool {
    windows_shell_file_name(shell).is_some_and(|name| {
        matches!(
            name.as_str(),
            "bash" | "bash.exe" | "sh" | "sh.exe" | "zsh" | "zsh.exe"
        )
    })
}

#[cfg(windows)]
fn windows_shell_file_name(shell: &str) -> Option<String> {
    std::path::Path::new(shell)
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_ascii_lowercase())
}

async fn emit_search_chunk<F, Fut>(
    full_output: &mut String,
    on_stdout: &mut F,
    chunk: String,
) -> Result<()>
where
    F: FnMut(Vec<u8>) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    full_output.push_str(&chunk);
    on_stdout(chunk.into_bytes()).await
}

fn validate_string_param(
    val: &Option<String>,
    name: &str,
    max_len: usize,
    char_check: fn(char) -> bool,
) -> Result<()> {
    if let Some(ref s) = val {
        if s.len() > max_len {
            return Err(BifrostError::Config(format!(
                "{} param too long: {} > {}",
                name,
                s.len(),
                max_len
            )));
        }
        if !s.chars().all(char_check) {
            return Err(BifrostError::Config(format!(
                "{} param contains invalid characters",
                name
            )));
        }
    }
    Ok(())
}

fn append_truncated_bytes(target: &mut Vec<u8>, chunk: &[u8], max_bytes: usize) {
    if target.len() >= max_bytes {
        return;
    }
    let remaining = max_bytes.saturating_sub(target.len());
    target.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
}

fn path_is_within(path: &str, allowed_prefix: &str) -> bool {
    path == allowed_prefix
        || path
            .strip_prefix(allowed_prefix)
            .is_some_and(|rest| rest.starts_with('/') || rest.starts_with('\\'))
}

fn sse_event(event: &str, json_data: &str) -> String {
    let data = json_data.replace('\n', "\\n");
    format!("event: {event}\ndata: {data}\n\n")
}

fn is_host_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':' | '*')
}

fn is_url_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            '.' | '_'
                | '-'
                | ':'
                | '/'
                | '?'
                | '&'
                | '='
                | '%'
                | '+'
                | '~'
                | '#'
                | '@'
                | '!'
                | '$'
                | ','
                | ';'
        )
}

fn is_path_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':' | '/')
}

fn is_content_type_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '.' | '+' | ';' | '=' | ' ')
}

fn is_client_ip_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | ':')
}

fn is_client_app_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ' ' | '(' | ')' | '/')
}

pub(crate) fn sha1_hex(input: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(input.as_bytes());
    let result = hasher.finalize();
    result.iter().fold(String::with_capacity(40), |mut acc, b| {
        use std::fmt::Write;
        let _ = write!(acc, "{:02x}", b);
        acc
    })
}

#[allow(dead_code)]
fn render_search_result(item: &SearchResultRow, query: &str) -> String {
    let record = &item.record;
    let mut out = format!(
        "{:>10}  {:>6}  {:>6}  {:7}  {:40}  {:46}  {:>10}  {:>8}\n",
        record.seq,
        format_status(record.s),
        truncate_str(&record.m, 6),
        truncate_str(&record.proto, 7),
        truncate_str(&record.h, 40),
        highlight_keyword(&truncate_str(&record.p, 46), query),
        format_size(record.res_sz),
        format_duration(record.dur),
    );

    for m in item.matches.iter().filter(|m| m.field != "url") {
        out.push_str(&format!(
            "        └─ {}: {}\n",
            m.field,
            highlight_keyword(&truncate_str(&m.preview, 80), query)
        ));
    }

    out
}

#[allow(dead_code)]
fn render_search_summary(
    query: &str,
    limit: usize,
    total_searched: usize,
    total_matched: usize,
    has_more: bool,
) -> String {
    let mut out = String::new();
    if total_matched == 0 {
        out.push_str(&format!(
            "No results found for '{}'\n  Scanned {} records (limit: {})\n",
            query,
            format_number(total_searched),
            format_number(limit),
        ));
    } else {
        out.push_str(&format!(
            "\nFound {} matches (scanned {} records, limit: {})\n",
            total_matched,
            format_number(total_searched),
            format_number(limit),
        ));
    }

    if has_more {
        out.push_str("  Search stopped early — more data may match.\n");
    }

    out
}

fn format_status(status: u16) -> String {
    if status == 0 {
        "...".to_string()
    } else {
        status.to_string()
    }
}

fn format_size(bytes: usize) -> String {
    const KB: usize = 1024;
    const MB: usize = 1024 * KB;

    if bytes >= MB {
        format!("{:.1}MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1}KB", bytes as f64 / KB as f64)
    } else {
        format!("{}B", bytes)
    }
}

fn format_duration(duration_ms: u64) -> String {
    if duration_ms >= 1000 {
        format!("{:.2}s", duration_ms as f64 / 1000.0)
    } else {
        format!("{}ms", duration_ms)
    }
}

#[allow(dead_code)]
fn format_number(value: usize) -> String {
    let s = value.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (index, ch) in s.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

fn truncate_str(input: &str, max_len: usize) -> String {
    let count = input.chars().count();
    if count <= max_len {
        return input.to_string();
    }

    let keep = max_len.saturating_sub(3);
    let mut out = String::new();
    for (index, ch) in input.chars().enumerate() {
        if index >= keep {
            break;
        }
        out.push(ch);
    }
    out.push_str("...");
    out
}

fn highlight_keyword(text: &str, keyword: &str) -> String {
    let _ = keyword;
    text.to_string()
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use super::*;
    use bifrost_storage::{RemoteShellPolicy, RemoteShellSet, RemoteShellStore};
    use tempfile::TempDir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn shell_policy_metadata_parses_pr7_fields() {
        let json = serde_json::json!({
            "max_wall_clock_ms": 7_200_000u64,
            "max_output_bytes_total": 1_073_741_824u64,
            "allow_resume": true
        });
        let meta: ShellPolicyMetadata =
            serde_json::from_value(json).expect("parse PR#7 policy metadata");
        assert_eq!(meta.max_wall_clock_ms, Some(7_200_000));
        assert_eq!(meta.max_output_bytes_total, Some(1_073_741_824));
        assert_eq!(meta.allow_resume, Some(true));
    }

    #[test]
    fn shell_policy_metadata_defaults_pr7_fields_to_none() {
        let json = serde_json::json!({});
        let meta: ShellPolicyMetadata = serde_json::from_value(json).expect("parse empty metadata");
        assert!(meta.max_wall_clock_ms.is_none());
        assert!(meta.max_output_bytes_total.is_none());
        assert!(meta.allow_resume.is_none());
    }

    fn setup_remote_shell_store() -> (std::sync::MutexGuard<'static, ()>, TempDir) {
        let guard = crate::remote_invoke::remote_shell_test_guard();
        let dir = TempDir::new().expect("tempdir");
        let data_dir = dir.path().join("bifrost-data");
        std::fs::create_dir_all(&data_dir).expect("create data dir");
        bifrost_storage::set_data_dir(data_dir);
        let store = RemoteShellStore::new().expect("remote shell store");
        store
            .save(&RemoteShellSet {
                schema_version: 1,
                version: 1,
                policies: vec![RemoteShellPolicy {
                    id: "test-shell".to_string(),
                    name: "test-shell".to_string(),
                    description: None,
                    enabled: true,
                    profile_id: None,
                    metadata: serde_json::json!({
                        "exec_mode": "shell_text",
                        "allowed_shell_patterns": ["^printf hello$"],
                        "max_timeout_ms": 5000
                    }),
                }],
                profiles: vec![],
            })
            .expect("save remote shell store");
        (guard, dir)
    }

    fn streaming_shell_command() -> (String, [&'static str; 2]) {
        if cfg!(target_os = "windows") {
            (
                "[Console]::Write('first'); Start-Sleep -Milliseconds 350; [Console]::Write('second')".to_string(),
                ["first", "second"],
            )
        } else {
            (
                "printf first; sleep 0.35; printf second".to_string(),
                ["first", "second"],
            )
        }
    }

    fn streaming_shell_program() -> &'static str {
        if cfg!(target_os = "windows") {
            r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"
        } else {
            "/bin/sh"
        }
    }

    #[test]
    fn test_sha1_hex_known_value() {
        let digest = sha1_hex("hello");
        assert_eq!(digest, "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d");
    }

    #[test]
    fn test_sha1_hex_empty() {
        let digest = sha1_hex("");
        assert_eq!(digest, "da39a3ee5e6b4b0d3255bfef95601890afd80709");
    }

    #[test]
    fn test_validate_args_id_alphanumeric_with_hyphens() {
        let executor = RemoteInvokeExecutor::new("127.0.0.1", 8800);
        let cmd = RemoteCommand {
            command: "traffic.get".to_string(),
            args_json: Some(r#"{"id":"REQ-69e304e7-000033"}"#.to_string()),
            ..Default::default()
        };
        let args = executor.parse_and_validate_args(&cmd);
        assert!(args.is_ok());
        assert_eq!(args.unwrap().id.as_deref(), Some("REQ-69e304e7-000033"));
    }

    #[test]
    fn test_validate_args_id_rejects_special_chars() {
        let executor = RemoteInvokeExecutor::new("127.0.0.1", 8800);
        let cmd = RemoteCommand {
            command: "traffic.get".to_string(),
            args_json: Some(r#"{"id":"abc;DROP"}"#.to_string()),
            ..Default::default()
        };
        let args = executor.parse_and_validate_args(&cmd);
        assert!(args.is_err());
    }

    #[test]
    fn test_validate_args_id_rejects_too_long() {
        let executor = RemoteInvokeExecutor::new("127.0.0.1", 8800);
        let long_id = "1".repeat(21);
        let cmd = RemoteCommand {
            command: "traffic.get".to_string(),
            args_json: Some(format!(r#"{{"id":"{}"}}"#, long_id)),
            ..Default::default()
        };
        let args = executor.parse_and_validate_args(&cmd);
        assert!(args.is_err());
    }

    #[test]
    fn test_validate_args_query_valid_chars() {
        let executor = RemoteInvokeExecutor::new("127.0.0.1", 8800);
        let cmd = RemoteCommand {
            command: "traffic.search".to_string(),
            args_json: Some(r#"{"query":"example.com/api-v2:443"}"#.to_string()),
            ..Default::default()
        };
        let args = executor.parse_and_validate_args(&cmd);
        assert!(args.is_ok());
        assert_eq!(
            args.unwrap().query.as_deref(),
            Some("example.com/api-v2:443")
        );
    }

    #[test]
    fn test_validate_args_query_rejects_control_chars() {
        let executor = RemoteInvokeExecutor::new("127.0.0.1", 8800);
        let cmd = RemoteCommand {
            command: "traffic.search".to_string(),
            args_json: Some(r#"{"query":"hello\u0000world"}"#.to_string()),
            ..Default::default()
        };
        let args = executor.parse_and_validate_args(&cmd);
        assert!(args.is_err());
    }

    #[test]
    fn test_validate_args_query_accepts_chinese() {
        let executor = RemoteInvokeExecutor::new("127.0.0.1", 8800);
        let cmd = RemoteCommand {
            command: "traffic.search".to_string(),
            args_json: Some(r#"{"query":"测试中文搜索"}"#.to_string()),
            ..Default::default()
        };
        let args = executor.parse_and_validate_args(&cmd);
        assert!(args.is_ok());
        assert_eq!(args.unwrap().query.as_deref(), Some("测试中文搜索"));
    }

    #[test]
    fn test_validate_args_query_accepts_special_url_chars() {
        let executor = RemoteInvokeExecutor::new("127.0.0.1", 8800);
        let cmd = RemoteCommand {
            command: "traffic.search".to_string(),
            args_json: Some(r#"{"query":"example.com/path?key=value&foo=bar"}"#.to_string()),
            ..Default::default()
        };
        let args = executor.parse_and_validate_args(&cmd);
        assert!(args.is_ok());
    }

    #[test]
    fn test_validate_args_query_rejects_too_long() {
        let executor = RemoteInvokeExecutor::new("127.0.0.1", 8800);
        let long_query = "a".repeat(501);
        let cmd = RemoteCommand {
            command: "traffic.search".to_string(),
            args_json: Some(format!(r#"{{"query":"{}"}}"#, long_query)),
            ..Default::default()
        };
        let args = executor.parse_and_validate_args(&cmd);
        assert!(args.is_err());
    }

    #[test]
    fn test_validate_args_search_keeps_legacy_limit_compatibility() {
        let executor = RemoteInvokeExecutor::new("127.0.0.1", 8800);
        let cmd = RemoteCommand {
            command: "search.get".to_string(),
            args_json: Some(r#"{"query":"hello","limit":5}"#.to_string()),
            ..Default::default()
        };
        let args = executor
            .parse_and_validate_args(&cmd)
            .expect("args should parse");
        assert_eq!(args.query.as_deref(), Some("hello"));
        assert_eq!(args.limit, Some(5));
    }

    #[test]
    fn test_validate_args_no_args() {
        let executor = RemoteInvokeExecutor::new("127.0.0.1", 8800);
        let cmd = RemoteCommand {
            command: "status".to_string(),
            args_json: None,
            ..Default::default()
        };
        let args = executor.parse_and_validate_args(&cmd);
        assert!(args.is_ok());
    }

    #[tokio::test]
    async fn test_execute_rejects_unknown_command() {
        let executor = RemoteInvokeExecutor::new("127.0.0.1", 8800);
        let cmd = RemoteCommand {
            command: "rm -rf".to_string(),
            args_json: None,
            ..Default::default()
        };
        let resp = executor.execute(&cmd).await.unwrap();
        assert_eq!(resp.exit_code, -1);
        assert!(resp.stderr.unwrap().contains("not allowed"));
    }

    #[tokio::test]
    async fn test_execute_rejects_legacy_query_command_at_allowlist_boundary() {
        let executor = RemoteInvokeExecutor::new("127.0.0.1", 8800);
        let cmd = RemoteCommand {
            command: "traffic.search".to_string(),
            args_json: Some(r#"{"query":"example.com"}"#.to_string()),
            ..Default::default()
        };

        let resp = executor
            .execute(&cmd)
            .await
            .expect("legacy command response");
        assert_eq!(resp.exit_code, -1);
        assert_eq!(
            resp.stderr.as_deref(),
            Some("Config error: command 'traffic.search' is not allowed")
        );
    }

    #[tokio::test]
    async fn test_execute_rejects_mutating_query_declared_as_readonly() {
        let executor = RemoteInvokeExecutor::new("127.0.0.1", 8800);
        let cmd = RemoteCommand {
            query: Some(CanonicalQueryCommand::TrafficClear(
                bifrost_command::TrafficClearArgs {
                    ids: Some(vec!["REQ-1".to_string()]),
                },
            )),
            ..Default::default()
        };

        let resp = executor
            .execute(&cmd)
            .await
            .expect("traffic clear response");
        assert_eq!(resp.exit_code, -1);
        assert_eq!(
            resp.stderr.as_deref(),
            Some("Config error: mutating query 'traffic.clear' cannot be sent as query.readonly")
        );
    }

    #[test]
    fn test_execute_shell_exec_shell_text() {
        let (_guard, _data_dir) = setup_remote_shell_store();
        let executor = RemoteInvokeExecutor::new("127.0.0.1", 8800);
        let cmd = RemoteCommand {
            kind: super::super::types::CommandKind::ShellExec,
            policy_id: Some("test-shell".to_string()),
            exec_mode: Some(ShellExecMode::ShellText),
            command_text: Some("printf hello".to_string()),
            ..Default::default()
        };

        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let resp = runtime
            .block_on(executor.execute(&cmd))
            .expect("shell exec response");
        assert_eq!(resp.exit_code, 0);
        assert_eq!(resp.stdout.as_deref(), Some("hello"));
    }

    #[cfg(windows)]
    #[test]
    fn test_windows_shell_detection_uses_powershell_command_flag_for_paths() {
        assert!(windows_shell_uses_command_flag("powershell"));
        assert!(windows_shell_uses_command_flag("pwsh.exe"));
        assert!(windows_shell_uses_command_flag(
            r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"
        ));
        assert!(!windows_shell_uses_command_flag("cmd.exe"));
    }

    #[cfg(windows)]
    #[test]
    fn test_windows_shell_detection_uses_login_flag_for_posix_shells() {
        assert!(windows_shell_uses_login_flag("bash"));
        assert!(windows_shell_uses_login_flag(
            r"C:\Program Files\Git\bin\bash.exe"
        ));
        assert!(!windows_shell_uses_login_flag("powershell.exe"));
        assert!(!windows_shell_uses_login_flag("cmd.exe"));
    }

    #[test]
    fn test_execute_shell_exec_streams_stdout_before_exit() {
        let _guard = crate::remote_invoke::remote_shell_test_guard();
        let dir = TempDir::new().expect("tempdir");
        let data_dir = dir.path().join("bifrost-data");
        std::fs::create_dir_all(&data_dir).expect("create data dir");
        bifrost_storage::set_data_dir(data_dir);
        let (shell_text, expected_chunks) = streaming_shell_command();
        RemoteShellStore::new()
            .expect("store")
            .save(&RemoteShellSet {
                schema_version: 1,
                version: 1,
                policies: vec![RemoteShellPolicy {
                    id: "stream-test".to_string(),
                    name: "stream-test".to_string(),
                    description: None,
                    enabled: true,
                    profile_id: None,
                    metadata: serde_json::json!({
                        "exec_mode": "shell_text",
                        "allowed_shell_patterns": ["^(?s:.*)$"],
                        "shell": streaming_shell_program(),
                        "max_timeout_ms": 5000
                    }),
                }],
                profiles: vec![],
            })
            .expect("save");

        let executor = RemoteInvokeExecutor::new("127.0.0.1", 8800);
        let cmd = RemoteCommand {
            kind: super::super::types::CommandKind::ShellExec,
            policy_id: Some("stream-test".to_string()),
            exec_mode: Some(ShellExecMode::ShellText),
            command_text: Some(shell_text.to_string()),
            ..Default::default()
        };

        #[allow(clippy::type_complexity)]
        let received_at: Arc<Mutex<Vec<(Vec<u8>, Duration)>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&received_at);
        let started = Instant::now();
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let resp = runtime
            .block_on(executor.execute_with_stdout_sink(&cmd, |chunk| {
                let sink = Arc::clone(&sink);
                let elapsed = started.elapsed();
                async move {
                    sink.lock().expect("sink lock").push((chunk, elapsed));
                    Ok(())
                }
            }))
            .expect("streaming shell exec response");

        assert_eq!(resp.exit_code, 0);
        assert_eq!(resp.stdout.as_deref(), Some("firstsecond"));
        let received = received_at.lock().expect("received lock");
        assert!(
            received.len() >= 2,
            "expected multiple streamed chunks, got {:?}",
            *received
        );
        assert_eq!(received[0].0.as_slice(), expected_chunks[0].as_bytes());
        assert_eq!(received[1].0.as_slice(), expected_chunks[1].as_bytes());
        assert!(
            received[0].1 < Duration::from_millis(250),
            "first chunk should arrive before the command exits, got {:?}",
            received[0].1
        );
        assert!(
            received[1].1 >= Duration::from_millis(250),
            "second chunk should arrive after the intentional delay, got {:?}",
            received[1].1
        );
    }

    #[test]
    fn test_execute_shell_exec_rejects_unimplemented_sandbox_policy() {
        let _guard = crate::remote_invoke::remote_shell_test_guard();
        let dir = TempDir::new().expect("tempdir");
        let data_dir = dir.path().join("bifrost-data");
        std::fs::create_dir_all(&data_dir).expect("create data dir");
        bifrost_storage::set_data_dir(data_dir);
        let store = RemoteShellStore::new().expect("remote shell store");
        store
            .save(&RemoteShellSet {
                schema_version: 1,
                version: 1,
                policies: vec![RemoteShellPolicy {
                    id: "default-sandbox".to_string(),
                    name: "Default Sandbox".to_string(),
                    description: None,
                    enabled: true,
                    profile_id: None,
                    metadata: serde_json::json!({
                        "exec_mode": "shell_text",
                        "allowed_shell_patterns": ["^(?s:.*)$"],
                        "reject_reason": "sandbox execution is not implemented yet on this target; choose Full Access or Custom Policies"
                    }),
                }],
                profiles: vec![],
            })
            .expect("save remote shell store");
        let executor = RemoteInvokeExecutor::new("127.0.0.1", 8800);
        let cmd = RemoteCommand {
            kind: super::super::types::CommandKind::ShellExec,
            policy_id: Some("default-sandbox".to_string()),
            exec_mode: Some(ShellExecMode::ShellText),
            command_text: Some("printf hello".to_string()),
            ..Default::default()
        };

        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let error = runtime
            .block_on(executor.execute(&cmd))
            .expect_err("shell exec should reject");
        assert_eq!(
            error.to_string(),
            "Config error: policy 'default-sandbox' is not executable: sandbox execution is not implemented yet on this target; choose Full Access or Custom Policies"
        );
    }

    #[test]
    fn test_select_policy_id_for_command_rejects_ambiguous_target_match() {
        let _guard = crate::remote_invoke::remote_shell_test_guard();
        let dir = TempDir::new().expect("tempdir");
        let data_dir = dir.path().join("bifrost-data");
        std::fs::create_dir_all(&data_dir).expect("create data dir");
        bifrost_storage::set_data_dir(data_dir);
        RemoteShellStore::new()
            .expect("store")
            .save(&RemoteShellSet {
                schema_version: 1,
                version: 1,
                policies: vec![
                    RemoteShellPolicy {
                        id: "full-a".to_string(),
                        name: "full-a".to_string(),
                        description: None,
                        enabled: true,
                        profile_id: None,
                        metadata: serde_json::json!({
                            "exec_mode": "shell_text",
                            "allowed_shell_patterns": ["^(?s:.*)$"]
                        }),
                    },
                    RemoteShellPolicy {
                        id: "full-b".to_string(),
                        name: "full-b".to_string(),
                        description: None,
                        enabled: true,
                        profile_id: None,
                        metadata: serde_json::json!({
                            "exec_mode": "shell_text",
                            "allowed_shell_patterns": ["^(?s:.*)$"]
                        }),
                    },
                ],
                profiles: vec![],
            })
            .expect("save remote shell store");

        let executor = RemoteInvokeExecutor::new("127.0.0.1", 8800);
        let command = RemoteCommand {
            kind: super::super::types::CommandKind::ShellExec,
            exec_mode: Some(ShellExecMode::ShellText),
            command_text: Some("printf hello".to_string()),
            ..Default::default()
        };

        let error = executor
            .select_policy_id_for_command(&command, Some(&serde_json::json!({ "mode": "all" })))
            .expect_err("ambiguous match should fail");
        assert!(error.to_string().contains("matched multiple policies"));
    }

    #[tokio::test]
    async fn test_resolve_traffic_id_accepts_numeric_sequence() {
        let (_server, host, port) = spawn_mock_http_server(vec![MockResponse {
            path_contains: "/_bifrost/api/traffic?limit=1&cursor=567&direction=backward",
            body_contains: None,
            content_type: "application/json".to_string(),
            body: serde_json::json!({
                "records": [{
                    "id": "REQ-69e304e7-000566",
                    "seq": 566,
                    "m": "GET",
                    "h": "example.com",
                    "p": "/nextoncall",
                    "s": 200,
                    "res_sz": 128,
                    "dur": 12,
                    "proto": "https"
                }],
                "server_sequence": 566
            })
            .to_string(),
        }])
        .await;

        let executor = RemoteInvokeExecutor::new(&host, port);
        let resolved = executor.resolve_traffic_id("566").await.unwrap();
        assert_eq!(resolved, "REQ-69e304e7-000566");
    }

    #[tokio::test]
    async fn test_list_traffic_forwards_all_supported_query_params() {
        let (_server, host, port) = spawn_mock_http_server(vec![MockResponse {
            path_contains: "/_bifrost/api/traffic?limit=7&cursor=123&direction=forward&method=POST&status=201&status_min=200&status_max=299&protocol=https&host_contains=api.example.com&url_contains=%2Fv1%2Fchat&path_contains=%2Fv1&content_type=application%2Fjson&client_ip=127.0.0.1&client_app=curl&has_rule_hit=true&is_websocket=false&is_sse=true&is_tunnel=false",
            body_contains: None,
            content_type: "application/json".to_string(),
            body: r#"{"records":[],"server_sequence":0}"#.to_string(),
        }])
        .await;

        let executor = RemoteInvokeExecutor::new(&host, port);
        let args = CommandArgs {
            limit: Some(7),
            cursor: Some(123),
            direction: Some("forward".to_string()),
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
            ..Default::default()
        };

        let body = executor.list_traffic(&args).await.unwrap();
        assert!(body.contains("\"records\":[]"));
    }

    #[tokio::test]
    async fn test_get_traffic_fetches_request_and_response_bodies_when_requested() {
        let (_server, host, port) = spawn_mock_http_server(vec![
            MockResponse {
                path_contains: "/_bifrost/api/traffic/REQ-69e304e7-000033",
                body_contains: None,
                content_type: "application/json".to_string(),
                body: r#"{"id":"REQ-69e304e7-000033","seq":33}"#.to_string(),
            },
            MockResponse {
                path_contains: "/_bifrost/api/traffic/REQ-69e304e7-000033/request-body",
                body_contains: None,
                content_type: "application/json".to_string(),
                body: r#"{"text":"request body"}"#.to_string(),
            },
            MockResponse {
                path_contains: "/_bifrost/api/traffic/REQ-69e304e7-000033/response-body",
                body_contains: None,
                content_type: "application/json".to_string(),
                body: r#"{"text":"response body"}"#.to_string(),
            },
        ])
        .await;

        let executor = RemoteInvokeExecutor::new(&host, port);
        let args = CommandArgs {
            id: Some("REQ-69e304e7-000033".to_string()),
            request_body: Some(true),
            response_body: Some(true),
            ..Default::default()
        };

        let body = executor
            .get_traffic("REQ-69e304e7-000033", &args)
            .await
            .unwrap();
        assert!(body.contains("\"request_body\":{\"text\":\"request body\"}"));
        assert!(body.contains("\"response_body\":{\"text\":\"response body\"}"));
    }

    #[tokio::test]
    async fn test_search_stream_formats_incremental_output() {
        let sse_body = concat!(
            "event: progress\n",
            "data: {\"total_searched\":12,\"total_matched\":0}\n\n",
            "event: result\n",
            "data: {\"record\":{\"id\":\"REQ-1\",\"seq\":566961,\"m\":\"GET\",\"h\":\"api.example.com\",\"p\":\"/nextoncall/profile\",\"s\":200,\"res_sz\":256,\"dur\":18,\"proto\":\"https\"},\"matches\":[{\"field\":\"url\",\"preview\":\"/nextoncall/profile\"},{\"field\":\"response_body\",\"preview\":\"hello nextoncall\"}]}\n\n",
            "event: done\n",
            "data: {\"total_searched\":12,\"total_matched\":1,\"has_more\":false}\n\n"
        );
        let (_server, host, port) = spawn_mock_http_server(vec![MockResponse {
            path_contains: "/_bifrost/api/search/stream",
            body_contains: Some("\"max_results\":5"),
            content_type: "text/event-stream".to_string(),
            body: sse_body.to_string(),
        }])
        .await;

        let executor = RemoteInvokeExecutor::new(&host, port);
        let chunks: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&chunks);
        let stdout = executor
            .search_stream("nextoncall", None, Some(5), None, &mut |chunk| {
                let sink = Arc::clone(&sink);
                async move {
                    sink.lock().unwrap().push(chunk);
                    Ok(())
                }
            })
            .await
            .unwrap();

        let joined_chunks: String = {
            let guard = chunks.lock().unwrap();
            let mut buf: Vec<u8> = Vec::new();
            for c in guard.iter() {
                buf.extend_from_slice(c);
            }
            String::from_utf8(buf).expect("utf8 chunks")
        };
        assert!(joined_chunks.contains("event: progress"));
        assert!(joined_chunks.contains("\"total_searched\":12"));
        assert!(joined_chunks.contains("event: result"));
        assert!(joined_chunks.contains("\"seq\":566961"));
        assert!(joined_chunks.contains("/nextoncall/profile"));
        assert!(joined_chunks.contains("event: done"));
        assert!(joined_chunks.contains("\"total_matched\":1"));
        assert_eq!(stdout, joined_chunks);
    }

    #[tokio::test]
    async fn test_search_stream_forwards_max_scan_to_executor() {
        let sse_body = concat!(
            "event: done\n",
            "data: {\"total_searched\":20,\"total_matched\":0,\"has_more\":true}\n\n"
        );
        let (_server, host, port) = spawn_mock_http_server(vec![MockResponse {
            path_contains: "/_bifrost/api/search/stream",
            body_contains: Some("\"max_scan\":20"),
            content_type: "text/event-stream".to_string(),
            body: sse_body.to_string(),
        }])
        .await;

        let executor = RemoteInvokeExecutor::new(&host, port);
        let stdout = executor
            .search_stream("nextoncall", None, Some(2), Some(20), &mut |_chunk| async {
                Ok(())
            })
            .await
            .unwrap();

        assert!(stdout.contains("event: done"));
        assert!(stdout.contains("\"total_searched\":20"));
        assert!(stdout.contains("\"has_more\":true"));
    }

    struct MockResponse {
        path_contains: &'static str,
        body_contains: Option<&'static str>,
        content_type: String,
        body: String,
    }

    async fn spawn_mock_http_server(
        responses: Vec<MockResponse>,
    ) -> (tokio::task::JoinHandle<()>, String, u16) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let queue = Arc::new(Mutex::new(VecDeque::from(responses)));
        let queue_for_task = Arc::clone(&queue);

        let handle = tokio::spawn(async move {
            loop {
                let response = {
                    let mut guard = queue_for_task.lock().unwrap();
                    guard.pop_front()
                };
                let Some(response) = response else {
                    break;
                };

                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 8192];
                let read = socket.read(&mut buf).await.unwrap();
                let request_text = String::from_utf8_lossy(&buf[..read]);
                assert!(
                    request_text.contains(response.path_contains),
                    "expected request to contain {:?}, got {:?}",
                    response.path_contains,
                    request_text.lines().next().unwrap_or_default()
                );
                if let Some(body_contains) = response.body_contains {
                    assert!(
                        request_text.contains(body_contains),
                        "expected request body to contain {:?}, got {:?}",
                        body_contains,
                        request_text
                    );
                }

                let http_response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response.content_type,
                    response.body.len(),
                    response.body
                );
                socket.write_all(http_response.as_bytes()).await.unwrap();
            }
        });

        (handle, addr.ip().to_string(), addr.port())
    }

    #[test]
    fn test_legacy_full_access_argv_exec_actually_runs() {
        let _guard = crate::remote_invoke::remote_shell_test_guard();
        let dir = TempDir::new().expect("tempdir");
        let data_dir = dir.path().join("bifrost-data");
        std::fs::create_dir_all(&data_dir).expect("create data dir");
        bifrost_storage::set_data_dir(data_dir);
        RemoteShellStore::new()
            .expect("store")
            .save(&RemoteShellSet {
                schema_version: 1,
                version: 1,
                policies: vec![RemoteShellPolicy {
                    id: "full-access".to_string(),
                    name: "Full Access".to_string(),
                    description: None,
                    enabled: true,
                    profile_id: None,
                    metadata: serde_json::json!({
                        "exec_mode": "shell_text",
                        "allowed_shell_patterns": ["^(?s:.*)$"],
                        "allow_any_executable": true,
                        "shell": "/bin/bash",
                        "inherit_env": true
                    }),
                }],
                profiles: vec![],
            })
            .expect("save");

        let executor = RemoteInvokeExecutor::new("127.0.0.1", 8800);
        let cmd = RemoteCommand {
            kind: super::super::types::CommandKind::ShellExec,
            policy_id: Some("full-access".to_string()),
            exec_mode: Some(ShellExecMode::ArgvExec),
            argv: Some(vec!["/bin/pwd".to_string()]),
            ..Default::default()
        };

        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let resp = runtime
            .block_on(executor.execute(&cmd))
            .expect("old-format full-access should execute argv_exec /bin/pwd");
        assert_eq!(resp.exit_code, 0, "exit_code should be 0");
        assert!(
            !resp.stdout.as_deref().unwrap_or("").trim().is_empty(),
            "stdout should contain cwd path"
        );
    }

    #[test]
    fn test_legacy_full_access_without_allowed_exec_modes_permits_argv() {
        let _guard = crate::remote_invoke::remote_shell_test_guard();
        let dir = TempDir::new().expect("tempdir");
        let data_dir = dir.path().join("bifrost-data");
        std::fs::create_dir_all(&data_dir).expect("create data dir");
        bifrost_storage::set_data_dir(data_dir);
        RemoteShellStore::new()
            .expect("store")
            .save(&RemoteShellSet {
                schema_version: 1,
                version: 1,
                policies: vec![RemoteShellPolicy {
                    id: "full-access".to_string(),
                    name: "Full Access".to_string(),
                    description: None,
                    enabled: true,
                    profile_id: None,
                    metadata: serde_json::json!({
                        "exec_mode": "shell_text",
                        "allowed_shell_patterns": ["^(?s:.*)$"],
                        "allow_any_executable": true,
                        "shell": "/bin/bash",
                        "inherit_env": true,
                        "stdin_allowed": true,
                        "interactive_allowed": true
                    }),
                }],
                profiles: vec![],
            })
            .expect("save");

        let executor = RemoteInvokeExecutor::new("127.0.0.1", 8800);
        let argv_command = RemoteCommand {
            kind: super::super::types::CommandKind::ShellExec,
            exec_mode: Some(ShellExecMode::ArgvExec),
            argv: Some(vec!["/bin/pwd".to_string()]),
            ..Default::default()
        };
        let result = executor
            .select_policy_id_for_command(
                &argv_command,
                Some(&serde_json::json!({ "mode": "all" })),
            )
            .expect("legacy full-access should accept argv_exec");
        assert_eq!(result, "full-access");

        let shell_command = RemoteCommand {
            kind: super::super::types::CommandKind::ShellExec,
            exec_mode: Some(ShellExecMode::ShellText),
            command_text: Some("pwd".to_string()),
            ..Default::default()
        };
        let result = executor
            .select_policy_id_for_command(
                &shell_command,
                Some(&serde_json::json!({ "mode": "all" })),
            )
            .expect("legacy full-access should accept shell_text");
        assert_eq!(result, "full-access");
    }

    #[test]
    fn test_build_shell_text_process_default_shell() {
        // When shell is None, should default to /bin/sh on non-windows
        let cmd = build_shell_text_process(None, "echo hi");
        let prog = cmd.as_std().get_program().to_string_lossy().to_string();
        #[cfg(not(windows))]
        assert_eq!(prog, "/bin/sh", "default shell should be /bin/sh");
        #[cfg(windows)]
        assert_eq!(prog, "cmd", "default shell on windows should be cmd");
    }

    #[test]
    fn test_build_shell_text_process_explicit_shell() {
        // When an explicit shell is given, it should be used directly
        #[cfg(not(windows))]
        {
            let cmd = build_shell_text_process(Some("/bin/bash"), "echo test");
            let prog = cmd.as_std().get_program().to_string_lossy().to_string();
            assert_eq!(prog, "/bin/bash");
            let args: Vec<_> = cmd
                .as_std()
                .get_args()
                .map(|a| a.to_string_lossy().to_string())
                .collect();
            assert_eq!(args, vec!["-lc", "echo test"]);
        }
    }

    #[cfg(windows)]
    #[test]
    fn test_build_shell_text_process_unix_path_fallback_on_windows() {
        // Unix-style paths like /bin/bash should be ignored on Windows, falling back to cmd
        let cmd = build_shell_text_process(Some("/bin/bash"), "echo hi");
        let prog = cmd.as_std().get_program().to_string_lossy().to_string();
        assert_eq!(
            prog, "cmd",
            "unix shell path should fallback to cmd on windows"
        );
        let args: Vec<_> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(args[0], "/C");
        assert!(args[1].contains("chcp 65001"), "should prepend chcp 65001");
        assert!(
            args[1].contains("echo hi"),
            "should contain original command"
        );
    }

    #[cfg(windows)]
    #[test]
    fn test_build_shell_text_process_powershell_utf8_prefix() {
        let cmd = build_shell_text_process(Some("powershell.exe"), "ipconfig");
        let prog = cmd.as_std().get_program().to_string_lossy().to_string();
        assert_eq!(prog, "powershell.exe");
        let args: Vec<_> = cmd
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert!(args.contains(&"-Command".to_string()));
        let cmd_arg = args.last().expect("should have command arg");
        assert!(
            cmd_arg.contains("[Console]::OutputEncoding"),
            "should set UTF-8 encoding"
        );
        assert!(cmd_arg.contains("ipconfig"));
    }

    #[cfg(windows)]
    #[test]
    fn test_is_unix_only_shell_path() {
        assert!(is_unix_only_shell_path("/bin/bash"));
        assert!(is_unix_only_shell_path("/usr/bin/zsh"));
        assert!(is_unix_only_shell_path("/bin/sh"));
        assert!(!is_unix_only_shell_path("cmd.exe"));
        assert!(!is_unix_only_shell_path("powershell.exe"));
        assert!(!is_unix_only_shell_path(r"C:\Windows\System32\cmd.exe"));
        assert!(!is_unix_only_shell_path("bash"));
    }

    #[test]
    fn test_select_policy_single_rejection_has_no_double_error_prefix() {
        let _guard = crate::remote_invoke::remote_shell_test_guard();
        let dir = TempDir::new().expect("tempdir");
        let data_dir = dir.path().join("bifrost-data");
        std::fs::create_dir_all(&data_dir).expect("create data dir");
        bifrost_storage::set_data_dir(data_dir);
        RemoteShellStore::new()
            .expect("store")
            .save(&RemoteShellSet {
                schema_version: 1,
                version: 1,
                policies: vec![RemoteShellPolicy {
                    id: "shell-only".to_string(),
                    name: "Shell Only".to_string(),
                    description: None,
                    enabled: true,
                    profile_id: None,
                    metadata: serde_json::json!({
                        "exec_mode": "shell_text",
                        "allowed_exec_modes": ["shell_text"],
                        "allowed_shell_patterns": ["^(?s:.*)$"]
                    }),
                }],
                profiles: vec![],
            })
            .expect("save");

        let executor = RemoteInvokeExecutor::new("127.0.0.1", 8800);
        let command = RemoteCommand {
            kind: super::super::types::CommandKind::ShellExec,
            exec_mode: Some(ShellExecMode::ArgvExec),
            argv: Some(vec!["/bin/pwd".to_string()]),
            ..Default::default()
        };

        let error = executor
            .select_policy_id_for_command(&command, Some(&serde_json::json!({ "mode": "all" })))
            .expect_err("should reject argv_exec for shell-only policy");
        let msg = error.to_string();
        assert!(
            !msg.contains("Config error: Config error:"),
            "error message must not double-wrap: {msg}"
        );
        assert!(
            msg.starts_with("Config error: "),
            "must have one prefix: {msg}"
        );
        assert!(
            msg.contains("policy 'shell-only'"),
            "must mention policy: {msg}"
        );
    }

    #[test]
    fn test_execute_shell_exec_idle_timeout_kills_stuck_child() {
        // PR#3a: child sleeps longer than idle timeout without emitting output
        //        -> must be killed with "idle timeout" error.
        let _guard = crate::remote_invoke::remote_shell_test_guard();
        let dir = TempDir::new().expect("tempdir");
        let data_dir = dir.path().join("bifrost-data");
        std::fs::create_dir_all(&data_dir).expect("create data dir");
        bifrost_storage::set_data_dir(data_dir);

        RemoteShellStore::new()
            .expect("store")
            .save(&RemoteShellSet {
                schema_version: 1,
                version: 1,
                policies: vec![RemoteShellPolicy {
                    id: "idle-test".to_string(),
                    name: "idle-test".to_string(),
                    description: None,
                    enabled: true,
                    profile_id: None,
                    metadata: serde_json::json!({
                        "exec_mode": "shell_text",
                        "allowed_shell_patterns": ["^(?s:.*)$"],
                        "shell": streaming_shell_program(),
                        // No wall-clock timeout; idle should kick in first.
                        "max_idle_ms": 300u64
                    }),
                }],
                profiles: vec![],
            })
            .expect("save");

        let executor = RemoteInvokeExecutor::new("127.0.0.1", 8800);
        let cmd = RemoteCommand {
            kind: super::super::types::CommandKind::ShellExec,
            policy_id: Some("idle-test".to_string()),
            exec_mode: Some(ShellExecMode::ShellText),
            // Sleep ~5s without printing anything. Idle timeout 300ms must kill.
            command_text: Some("sleep 5".to_string()),
            ..Default::default()
        };

        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let started = Instant::now();
        let err = runtime
            .block_on(executor.execute(&cmd))
            .expect_err("idle timeout must produce an error");
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_millis(2_000),
            "idle timeout should have killed within a second or two, elapsed={:?}",
            elapsed
        );
        let msg = format!("{err}");
        assert!(
            msg.contains("idle timeout"),
            "expected idle timeout error, got: {msg}"
        );
    }

    #[test]
    fn test_execute_shell_exec_wall_clock_timeout_still_enforced() {
        // PR#3a: when command.timeout_ms is set, wall-clock must still kill
        //        even if output keeps streaming (idle deadline refreshed).
        let _guard = crate::remote_invoke::remote_shell_test_guard();
        let dir = TempDir::new().expect("tempdir");
        let data_dir = dir.path().join("bifrost-data");
        std::fs::create_dir_all(&data_dir).expect("create data dir");
        bifrost_storage::set_data_dir(data_dir);

        RemoteShellStore::new()
            .expect("store")
            .save(&RemoteShellSet {
                schema_version: 1,
                version: 1,
                policies: vec![RemoteShellPolicy {
                    id: "wall-test".to_string(),
                    name: "wall-test".to_string(),
                    description: None,
                    enabled: true,
                    profile_id: None,
                    metadata: serde_json::json!({
                        "exec_mode": "shell_text",
                        "allowed_shell_patterns": ["^(?s:.*)$"],
                        "shell": streaming_shell_program(),
                        // Large idle; small wall-clock should win.
                        "max_idle_ms": 60_000u64
                    }),
                }],
                profiles: vec![],
            })
            .expect("save");

        let executor = RemoteInvokeExecutor::new("127.0.0.1", 8800);
        let cmd = RemoteCommand {
            kind: super::super::types::CommandKind::ShellExec,
            policy_id: Some("wall-test".to_string()),
            exec_mode: Some(ShellExecMode::ShellText),
            // Print a dot every 50ms forever -> never idle; wall-clock must trip.
            command_text: Some(
                "i=0; while [ $i -lt 200 ]; do printf .; sleep 0.05; i=$((i+1)); done".to_string(),
            ),
            timeout_ms: Some(400),
            ..Default::default()
        };

        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let started = Instant::now();
        let err = runtime
            .block_on(executor.execute(&cmd))
            .expect_err("wall-clock timeout must produce an error");
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_millis(2_000),
            "wall-clock should have killed within 2s, elapsed={:?}",
            elapsed
        );
        let msg = format!("{err}");
        assert!(
            msg.contains("wall-clock timeout"),
            "expected wall-clock timeout error, got: {msg}"
        );
    }

    fn drain_frames_sync(
        runtime: &tokio::runtime::Runtime,
        mut rx: tokio::sync::mpsc::Receiver<crate::remote_invoke::types::StreamFrame>,
    ) -> Vec<crate::remote_invoke::types::StreamFrame> {
        runtime.block_on(async move {
            let mut v = Vec::new();
            while let Some(f) = rx.recv().await {
                v.push(f);
            }
            v
        })
    }

    #[test]
    fn test_streaming_emits_stdout_with_monotonic_offsets_and_matching_sha() {
        use crate::remote_invoke::types::StreamFrame;
        use ring::digest::{Context, SHA256};

        let _guard = crate::remote_invoke::remote_shell_test_guard();
        let dir = TempDir::new().expect("tempdir");
        let data_dir = dir.path().join("bifrost-data");
        std::fs::create_dir_all(&data_dir).expect("create data dir");
        bifrost_storage::set_data_dir(data_dir);

        RemoteShellStore::new()
            .expect("store")
            .save(&RemoteShellSet {
                schema_version: 1,
                version: 1,
                policies: vec![RemoteShellPolicy {
                    id: "stream3b".to_string(),
                    name: "stream3b".to_string(),
                    description: None,
                    enabled: true,
                    profile_id: None,
                    metadata: serde_json::json!({
                        "exec_mode": "shell_text",
                        "allowed_shell_patterns": ["^(?s:.*)$"],
                        "shell": streaming_shell_program(),
                    }),
                }],
                profiles: vec![],
            })
            .expect("save");

        let executor = RemoteInvokeExecutor::new("127.0.0.1", 8800);
        // Produce 200 KiB of stdout so we get multiple 64 KiB frames.
        let cmd = RemoteCommand {
            kind: super::super::types::CommandKind::ShellExec,
            policy_id: Some("stream3b".to_string()),
            exec_mode: Some(ShellExecMode::ShellText),
            command_text: Some(
                // printf a 64-byte pattern 3200 times => 204_800 bytes
                "i=0; while [ $i -lt 3200 ]; do                     printf '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef';                     i=$((i+1));                 done".to_string(),
            ),
            ..Default::default()
        };

        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let (tx, rx) = tokio::sync::mpsc::channel::<StreamFrame>(32);
        let exec_arc = std::sync::Arc::new(executor);
        let exec_clone = std::sync::Arc::clone(&exec_arc);
        let cmd_arc = std::sync::Arc::new(cmd);
        let cmd_clone = std::sync::Arc::clone(&cmd_arc);
        runtime.spawn(async move {
            exec_clone
                .execute_shell_exec_streaming(&cmd_clone, tx)
                .await
                .expect("streaming ok");
        });
        let frames = drain_frames_sync(&runtime, rx);

        // Expectations: at least one Stdout frame, monotonic offsets, final Done
        // with matching total + sha.
        let mut reconstructed: Vec<u8> = Vec::new();
        let mut expected_offset: u64 = 0;
        let mut seq_count: u64 = 0;
        let mut done_frame: Option<StreamFrame> = None;
        for f in &frames {
            match f {
                StreamFrame::Stdout {
                    seq,
                    offset,
                    data_b64,
                } => {
                    assert_eq!(*seq, seq_count, "seq must be monotonic");
                    assert_eq!(*offset, expected_offset, "offset must be monotonic");
                    seq_count += 1;
                    let bytes = base64::Engine::decode(
                        &base64::engine::general_purpose::STANDARD,
                        data_b64,
                    )
                    .expect("b64 decode");
                    expected_offset += bytes.len() as u64;
                    reconstructed.extend_from_slice(&bytes);
                }
                StreamFrame::Stderr { .. } => {}
                StreamFrame::Heartbeat { .. } => {}
                StreamFrame::Reconnect { .. } => {}
                StreamFrame::Done { .. } => {
                    done_frame = Some(f.clone());
                }
                StreamFrame::Error { code, message } => {
                    panic!("unexpected Error frame: {code} {message}");
                }
                StreamFrame::Ack { .. } => {}
            }
        }
        let done = done_frame.expect("Done frame required");
        match done {
            StreamFrame::Done {
                exit_code,
                total_stdout,
                stdout_sha256,
                ..
            } => {
                assert_eq!(exit_code, 0);
                assert_eq!(total_stdout, reconstructed.len() as u64);
                assert_eq!(total_stdout, 3200 * 64);
                // Compute SHA over reconstructed, compare.
                let mut ctx = Context::new(&SHA256);
                ctx.update(&reconstructed);
                let d = ctx.finish();
                let mut expected = String::with_capacity(64);
                for b in d.as_ref() {
                    expected.push_str(&format!("{b:02x}"));
                }
                assert_eq!(stdout_sha256, expected);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn test_streaming_backpressure_blocks_on_slow_consumer() {
        // With mpsc capacity=1 and a producer that emits many chunks, the
        // executor must wait for us to recv() before emitting the next frame.
        // We verify by measuring that between two consecutive recv()s the
        // producer could not run away and flood memory.
        use crate::remote_invoke::types::StreamFrame;

        let _guard = crate::remote_invoke::remote_shell_test_guard();
        let dir = TempDir::new().expect("tempdir");
        let data_dir = dir.path().join("bifrost-data");
        std::fs::create_dir_all(&data_dir).expect("create data dir");
        bifrost_storage::set_data_dir(data_dir);

        RemoteShellStore::new()
            .expect("store")
            .save(&RemoteShellSet {
                schema_version: 1,
                version: 1,
                policies: vec![RemoteShellPolicy {
                    id: "bp-test".to_string(),
                    name: "bp-test".to_string(),
                    description: None,
                    enabled: true,
                    profile_id: None,
                    metadata: serde_json::json!({
                        "exec_mode": "shell_text",
                        "allowed_shell_patterns": ["^(?s:.*)$"],
                        "shell": streaming_shell_program(),
                    }),
                }],
                profiles: vec![],
            })
            .expect("save");

        let executor = std::sync::Arc::new(RemoteInvokeExecutor::new("127.0.0.1", 8800));
        let cmd = std::sync::Arc::new(RemoteCommand {
            kind: super::super::types::CommandKind::ShellExec,
            policy_id: Some("bp-test".to_string()),
            exec_mode: Some(ShellExecMode::ShellText),
            command_text: Some(
                "i=0; while [ $i -lt 64 ]; do                     printf '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef';                     i=$((i+1));                 done".to_string(),
            ),
            ..Default::default()
        });

        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let (tx, mut rx) = tokio::sync::mpsc::channel::<StreamFrame>(1);
        let exec_clone = std::sync::Arc::clone(&executor);
        let cmd_clone = std::sync::Arc::clone(&cmd);
        let join = runtime.spawn(async move {
            exec_clone
                .execute_shell_exec_streaming(&cmd_clone, tx)
                .await
        });

        // Consume slowly. Total stdout is 4096 bytes -> likely 1 Stdout frame +
        // Done. Accept a flexible assertion: first frame pulled must be either
        // Stdout or an early Heartbeat; drain; terminate normally.
        let result = runtime.block_on(async move {
            let mut total = 0u64;
            let mut done_seen = false;
            while let Some(f) = rx.recv().await {
                // slow consumer: 5 ms per frame
                tokio::time::sleep(Duration::from_millis(5)).await;
                match f {
                    StreamFrame::Stdout { data_b64, .. } => {
                        let bytes = base64::Engine::decode(
                            &base64::engine::general_purpose::STANDARD,
                            &data_b64,
                        )
                        .expect("b64");
                        total += bytes.len() as u64;
                    }
                    StreamFrame::Done { total_stdout, .. } => {
                        done_seen = true;
                        assert_eq!(total, total_stdout);
                    }
                    StreamFrame::Error { code, message } => {
                        panic!("Error frame: {code} {message}");
                    }
                    _ => {}
                }
            }
            assert!(done_seen, "Done frame must be observed");
            total
        });
        assert_eq!(result, 64 * 64);
        runtime
            .block_on(join)
            .expect("join")
            .expect("streaming result");
    }

    #[test]
    fn test_streaming_reports_policy_rejection_via_error_frame() {
        use crate::remote_invoke::types::StreamFrame;

        let _guard = crate::remote_invoke::remote_shell_test_guard();
        let dir = TempDir::new().expect("tempdir");
        let data_dir = dir.path().join("bifrost-data");
        std::fs::create_dir_all(&data_dir).expect("create data dir");
        bifrost_storage::set_data_dir(data_dir);

        // No policy saved -> resolve_shell_policy should reject.
        let executor = RemoteInvokeExecutor::new("127.0.0.1", 8800);
        let cmd = RemoteCommand {
            kind: super::super::types::CommandKind::ShellExec,
            policy_id: Some("missing-policy".to_string()),
            exec_mode: Some(ShellExecMode::ShellText),
            command_text: Some("true".to_string()),
            ..Default::default()
        };

        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let (tx, rx) = tokio::sync::mpsc::channel::<StreamFrame>(8);
        let exec_arc = std::sync::Arc::new(executor);
        let exec_clone = std::sync::Arc::clone(&exec_arc);
        let cmd_arc = std::sync::Arc::new(cmd);
        let cmd_clone = std::sync::Arc::clone(&cmd_arc);
        runtime.spawn(async move {
            exec_clone
                .execute_shell_exec_streaming(&cmd_clone, tx)
                .await
                .expect("streaming ok");
        });
        let frames = drain_frames_sync(&runtime, rx);
        assert_eq!(
            frames.len(),
            1,
            "expected exactly one Error frame, got {:?}",
            frames
        );
        match &frames[0] {
            StreamFrame::Error { code, .. } => {
                assert_eq!(code, "policy_rejected");
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }
}

// PR#7b Gate 3: helper used at the streaming subscription dispatch point.
// Wiring into the actual subscribe handler is tracked in remote-large-output-plan.md.
#[allow(dead_code)]
pub(crate) fn enforce_allow_resume(
    allow_resume: bool,
    requested_from_offset: Option<u64>,
    policy_id: &str,
) -> Result<()> {
    match requested_from_offset {
        None => Ok(()),
        Some(_) if allow_resume => Ok(()),
        Some(off) => Err(BifrostError::Config(format!(
            "policy '{}' does not allow resume subscription (requested from_offset={})",
            policy_id, off
        ))),
    }
}

#[cfg(test)]
mod pr7b_policy_enforcement_tests {
    use super::*;

    #[test]
    fn enforce_allow_resume_none_offset_passes() {
        assert!(enforce_allow_resume(false, None, "p1").is_ok());
        assert!(enforce_allow_resume(true, None, "p2").is_ok());
    }

    #[test]
    fn enforce_allow_resume_enabled_passes() {
        assert!(enforce_allow_resume(true, Some(0), "p").is_ok());
        assert!(enforce_allow_resume(true, Some(1_048_576), "p").is_ok());
    }

    #[test]
    fn enforce_allow_resume_disabled_rejects_any_offset() {
        let err = enforce_allow_resume(false, Some(0), "strict").unwrap_err();
        match err {
            BifrostError::Config(msg) => {
                assert!(msg.contains("strict"), "msg={msg}");
                assert!(msg.contains("from_offset=0"), "msg={msg}");
            }
            other => panic!("expected Config, got {other:?}"),
        }

        let err2 = enforce_allow_resume(false, Some(42_000), "strict").unwrap_err();
        match err2 {
            BifrostError::Config(msg) => assert!(msg.contains("from_offset=42000")),
            other => panic!("expected Config, got {other:?}"),
        }
    }
}

/// First 16 hex chars of sha256(path). Empty input => "?".
fn audit_path_hash(path: &str) -> String {
    if path.is_empty() {
        return "?".to_string();
    }
    use ring::digest::{Context, SHA256};
    let mut ctx = Context::new(&SHA256);
    ctx.update(path.as_bytes());
    let digest = ctx.finish();
    digest
        .as_ref()
        .iter()
        .take(8)
        .map(|b| format!("{:02x}", b))
        .collect()
}

/// Extract `[file.xxx]` error code from a BifrostError display string.
/// Falls back to `"file.internal"`.
fn extract_file_error_code(msg: &str) -> String {
    if let Some(start) = msg.find("[file.") {
        let rest = &msg[start + 1..];
        if let Some(end) = rest.find(']') {
            return rest[..end].to_string();
        }
    }
    "file.internal".to_string()
}

/// Pull `bytes_written` (u64) and `sha256` (string) from a handler's JSON
/// result, if present. Both fields are optional across ops.
fn extract_audit_fields(json: &str) -> (Option<u64>, Option<String>) {
    let v: serde_json::Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(_) => return (None, None),
    };
    let bytes = v
        .get("bytes_written")
        .and_then(|x| x.as_u64())
        .or_else(|| v.get("size").and_then(|x| x.as_u64()))
        .or_else(|| v.get("bytes").and_then(|x| x.as_u64()));
    let sha = v
        .get("sha256")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    (bytes, sha)
}

#[cfg(test)]
mod audit_helpers_tests {
    use super::*;

    #[test]
    fn path_hash_is_stable_and_prefix() {
        let a = audit_path_hash("/tmp/foo.txt");
        let b = audit_path_hash("/tmp/foo.txt");
        assert_eq!(a, b);
        assert_eq!(a.len(), 16);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn path_hash_empty_marker() {
        assert_eq!(audit_path_hash(""), "?");
    }

    #[test]
    fn extracts_file_code() {
        assert_eq!(
            extract_file_error_code("[file.sha_mismatch] base does not match"),
            "file.sha_mismatch"
        );
        assert_eq!(
            extract_file_error_code("something wrapped: [file.permission_denied] grant"),
            "file.permission_denied"
        );
        assert_eq!(extract_file_error_code("bare text"), "file.internal");
    }

    #[test]
    fn extracts_audit_fields() {
        let (b, s) = extract_audit_fields(r#"{"bytes_written":42,"sha256":"deadbeef"}"#);
        assert_eq!(b, Some(42));
        assert_eq!(s.as_deref(), Some("deadbeef"));
        let (b, s) = extract_audit_fields("not json");
        assert!(b.is_none() && s.is_none());
        let (b, _) = extract_audit_fields(r#"{"size":7}"#);
        assert_eq!(b, Some(7));
    }
}
