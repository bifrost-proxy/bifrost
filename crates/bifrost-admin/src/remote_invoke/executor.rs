use std::collections::BTreeMap;
use std::future::Future;
use std::time::{Duration, Instant};

use bifrost_command::{
    CanonicalQueryCommand, SearchArgs, TrafficGetArgs, TrafficListArgs, TrafficListDirection,
};
use bifrost_core::{direct_reqwest_client_builder, BifrostError, Result};
use bifrost_storage::{RemoteShellSet, RemoteShellStore};
use futures_util::StreamExt;
use regex::Regex;
use sha1::{Digest, Sha1};
use tokio::process::Command as TokioCommand;
use tracing::{debug, warn};

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
const DEFAULT_SHELL_OUTPUT_MAX_BYTES: usize = 64 * 1024;
const DEFAULT_SHELL_TIMEOUT_MS: u64 = 30_000;

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
    max_output_bytes: usize,
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
    max_output_bytes: Option<usize>,
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
        }
    }

    pub fn new_with_state(admin_host: &str, admin_port: u16, state: SharedAdminState) -> Self {
        let mut executor = Self::new(admin_host, admin_port);
        executor.query_service = Some(AdminQueryService::new(state));
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
        F: FnMut(String) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
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
                })
            }
        }
    }

    fn parse_and_validate_args(&self, command: &RemoteCommand) -> Result<CommandArgs> {
        if command.kind == super::types::CommandKind::ShellExec {
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
        F: FnMut(String) -> Fut,
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

    async fn execute_shell_exec<F, Fut>(
        &self,
        command: &RemoteCommand,
        on_stdout: &mut F,
    ) -> Result<RemoteInvokeResponse>
    where
        F: FnMut(String) -> Fut,
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
        let timeout_ms = command
            .timeout_ms
            .unwrap_or(policy.max_timeout_ms.unwrap_or(DEFAULT_SHELL_TIMEOUT_MS))
            .min(policy.max_timeout_ms.unwrap_or(u64::MAX));
        let mut process = self.build_shell_exec_process(command, &policy)?;
        let output = tokio::time::timeout(Duration::from_millis(timeout_ms), process.output())
            .await
            .map_err(|_| {
                BifrostError::Network(format!(
                    "shell.exec timed out after {} ms (policy '{}')",
                    timeout_ms, policy.policy_id
                ))
            })?
            .map_err(|e| BifrostError::Network(format!("spawn shell.exec failed: {}", e)))?;

        let stdout = truncate_utf8_bytes(&output.stdout, policy.max_output_bytes);
        let stderr = truncate_utf8_bytes(&output.stderr, policy.max_output_bytes);
        if !stdout.is_empty() {
            on_stdout(stdout.clone()).await?;
        }

        Ok(RemoteInvokeResponse {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: (!stdout.is_empty()).then_some(stdout.clone()),
            stderr: (!stderr.is_empty()).then_some(stderr.clone()),
            stdout_digest: (!stdout.is_empty()).then(|| sha1_hex(&stdout)),
            stderr_digest: (!stderr.is_empty()).then(|| sha1_hex(&stderr)),
            duration_ms: start.elapsed().as_millis() as u64,
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
            max_output_bytes: policy_meta
                .max_output_bytes
                .or(profile_meta.max_output_bytes)
                .unwrap_or(DEFAULT_SHELL_OUTPUT_MAX_BYTES),
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
                "policy '{}' allows exec_mode [{}], got {}. Use '--shell-text <cmd>' for shell text, or 'bifrost remote command exec -- <program> [args...]' for argv execution.",
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
        F: FnMut(String) -> Fut,
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
        F: FnMut(String) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        if !body.is_empty() {
            on_stdout(body.clone()).await?;
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
        F: FnMut(String) -> Fut,
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
        F: FnMut(String) -> Fut,
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
        if let Some(shell) = shell {
            let mut command = TokioCommand::new(shell);
            command.arg("/C").arg(shell_text);
            return command;
        }

        let mut command = TokioCommand::new("cmd");
        command.arg("/C").arg(shell_text);
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

async fn emit_search_chunk<F, Fut>(
    full_output: &mut String,
    on_stdout: &mut F,
    chunk: String,
) -> Result<()>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    full_output.push_str(&chunk);
    on_stdout(chunk).await
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

fn truncate_utf8_bytes(bytes: &[u8], max_bytes: usize) -> String {
    if bytes.len() <= max_bytes {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    String::from_utf8_lossy(&bytes[..max_bytes]).into_owned()
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

    use super::*;
    use bifrost_storage::{RemoteShellPolicy, RemoteShellSet, RemoteShellStore};
    use tempfile::TempDir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

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
        let chunks: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
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

        let joined_chunks = chunks.lock().unwrap().join("");
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
}
