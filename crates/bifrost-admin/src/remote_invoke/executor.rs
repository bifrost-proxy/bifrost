use std::future::Future;
use std::time::{Duration, Instant};

use bifrost_core::{direct_reqwest_client_builder, BifrostError, Result};
use futures_util::StreamExt;
use sha1::{Digest, Sha1};
use tokio::process::Command as TokioCommand;
use tracing::{debug, warn};

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

const ALLOWED_METHODS: &[&str] = &[
    "GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS", "CONNECT", "TRACE",
];
const ALLOWED_PROTOCOLS: &[&str] = &["http", "https", "ws", "wss", "h3"];
const ALLOWED_DIRECTIONS: &[&str] = &["backward", "forward"];

pub struct RemoteInvokeExecutor {
    admin_host: String,
    admin_port: u16,
    http: reqwest::Client,
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
    max_scan: Option<usize>,
    #[serde(default)]
    max_results: Option<usize>,
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

#[derive(Debug, Clone, serde::Deserialize)]
struct SearchResultRow {
    record: TrafficSummaryRow,
    matches: Vec<SearchMatchRow>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct SearchMatchRow {
    field: String,
    preview: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct SearchProgressPayload {
    total_searched: usize,
    total_matched: usize,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct SearchDonePayload {
    total_searched: usize,
    total_matched: usize,
    has_more: bool,
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
        }
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
        let args = self.parse_and_validate_args(command)?;

        let start = Instant::now();
        let result = match command.kind {
            super::types::CommandKind::QueryReadonly => {
                if !is_allowed_command(&command.command) {
                    let stderr = format!("command '{}' is not allowed", command.command);
                    warn!(
                        command = %command.command,
                        "remote invoke rejected: command not in whitelist"
                    );
                    Err(BifrostError::Config(stderr))
                } else {
                    self.dispatch_with_stdout_sink(&command.command, &args, &mut on_stdout)
                        .await
                }
            }
            super::types::CommandKind::ShellExec => {
                self.execute_shell_exec(command, &mut on_stdout).await
            }
        };
        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(body) => {
                let stdout_digest = (!body.is_empty()).then(|| sha1_hex(&body));
                debug!(
                    command = %command.command,
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
                    command = %command.command,
                    duration_ms,
                    error = %stderr,
                    "remote invoke failed"
                );
                Ok(RemoteInvokeResponse {
                    exit_code: -1,
                    stdout: None,
                    stderr: Some(stderr),
                    stdout_digest: None,
                    stderr_digest: Some(sha1_hex(&e.to_string())),
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

    async fn execute_shell_exec<F, Fut>(
        &self,
        command: &RemoteCommand,
        on_stdout: &mut F,
    ) -> Result<String>
    where
        F: FnMut(String) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        if command.pty.as_ref().map(|pty| pty.enabled).unwrap_or(false) {
            return Err(BifrostError::Config(
                "shell.exec with PTY is not implemented on client yet".to_string(),
            ));
        }

        let timeout_ms = command.timeout_ms.unwrap_or(REQUEST_TIMEOUT_SECS * 1000);
        let mut process = self.build_shell_exec_process(command)?;
        let output = tokio::time::timeout(Duration::from_millis(timeout_ms), process.output())
            .await
            .map_err(|_| BifrostError::Network("shell.exec timed out".to_string()))?
            .map_err(|e| BifrostError::Network(format!("spawn shell.exec failed: {}", e)))?;

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        if !stdout.is_empty() {
            on_stdout(stdout.clone()).await?;
        }

        if output.status.success() {
            Ok(stdout)
        } else {
            Err(BifrostError::Network(if stderr.is_empty() {
                format!(
                    "shell.exec exited with status {}",
                    output.status.code().unwrap_or(-1)
                )
            } else {
                stderr
            }))
        }
    }

    fn build_shell_exec_process(&self, command: &RemoteCommand) -> Result<TokioCommand> {
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
                build_shell_text_process(command.shell.as_deref(), shell_text)
            }
        };

        if let Some(cwd) = &command.cwd {
            process.current_dir(cwd);
        }
        if let Some(env) = &command.env {
            process.envs(env);
        }

        Ok(process)
    }

    async fn dispatch_with_stdout_sink<F, Fut>(
        &self,
        command: &str,
        args: &CommandArgs,
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
            "traffic.list" => {
                let body = self.list_traffic(args).await?;
                self.emit_stdout(on_stdout, body).await
            }
            "traffic.get" => {
                let id = args.id.as_deref().ok_or_else(|| {
                    BifrostError::Config("traffic.get requires 'id' arg".to_string())
                })?;
                let body = self.get_traffic(id, args).await?;
                self.emit_stdout(on_stdout, body).await
            }
            "traffic.search" | "search.get" => {
                let query = args.query.as_deref().ok_or_else(|| {
                    BifrostError::Config(format!("{} requires 'query' arg", command))
                })?;
                self.search_stream(
                    query,
                    args.limit,
                    args.max_results,
                    args.max_scan,
                    on_stdout,
                )
                .await
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
        let mut partial_line = String::new();
        let mut event_name = String::new();
        let mut data_buf = String::new();
        let mut printed_header = false;
        let mut progress_visible = false;
        let mut total_searched = 0usize;
        let mut total_matched = 0usize;
        let mut has_more = false;
        let mut saw_done = false;

        while let Some(chunk) = stream.next().await {
            let bytes = chunk
                .map_err(|e| BifrostError::Network(format!("search stream read failed: {}", e)))?;
            partial_line.push_str(&String::from_utf8_lossy(&bytes));

            while let Some(pos) = partial_line.find('\n') {
                let line = partial_line[..pos].trim_end_matches('\r').to_string();
                partial_line = partial_line[pos + 1..].to_string();

                if line.is_empty() {
                    if event_name.is_empty() || data_buf.is_empty() {
                        continue;
                    }

                    match event_name.as_str() {
                        "result" => {
                            let item: SearchResultRow =
                                serde_json::from_str(&data_buf).map_err(|e| {
                                    BifrostError::Parse(format!(
                                        "failed to parse search result event: {}",
                                        e
                                    ))
                                })?;
                            if progress_visible {
                                emit_search_chunk(
                                    &mut full_output,
                                    on_stdout,
                                    "\r\x1b[K".to_string(),
                                )
                                .await?;
                                progress_visible = false;
                            }
                            if !printed_header {
                                emit_search_chunk(
                                    &mut full_output,
                                    on_stdout,
                                    format!(
                                        "\n{:>10}  {:>6}  {:>6}  {:7}  {:40}  {:46}  {:>10}  {:>8}\n{}\n",
                                        "SEQ",
                                        "STATUS",
                                        "METHOD",
                                        "PROTO",
                                        "HOST",
                                        "PATH",
                                        "SIZE",
                                        "TIME",
                                        "─".repeat(150)
                                    ),
                                )
                                .await?;
                                printed_header = true;
                            }

                            emit_search_chunk(
                                &mut full_output,
                                on_stdout,
                                render_search_result(&item, query),
                            )
                            .await?;
                        }
                        "progress" => {
                            let progress: SearchProgressPayload = serde_json::from_str(&data_buf)
                                .map_err(|e| {
                                BifrostError::Parse(format!(
                                    "failed to parse search progress event: {}",
                                    e
                                ))
                            })?;
                            total_searched = progress.total_searched;
                            total_matched = progress.total_matched;
                            emit_search_chunk(
                                &mut full_output,
                                on_stdout,
                                format!(
                                    "\r  Searching... {} records scanned, {} matched",
                                    format_number(progress.total_searched),
                                    progress.total_matched
                                ),
                            )
                            .await?;
                            progress_visible = true;
                        }
                        "done" => {
                            let done: SearchDonePayload =
                                serde_json::from_str(&data_buf).map_err(|e| {
                                    BifrostError::Parse(format!(
                                        "failed to parse search done event: {}",
                                        e
                                    ))
                                })?;
                            total_searched = done.total_searched;
                            total_matched = done.total_matched;
                            has_more = done.has_more;
                            saw_done = true;

                            if progress_visible {
                                emit_search_chunk(
                                    &mut full_output,
                                    on_stdout,
                                    "\r\x1b[K".to_string(),
                                )
                                .await?;
                                progress_visible = false;
                            }

                            emit_search_chunk(
                                &mut full_output,
                                on_stdout,
                                render_search_summary(
                                    query,
                                    search_limit,
                                    total_searched,
                                    total_matched,
                                    has_more,
                                ),
                            )
                            .await?;
                        }
                        _ => {}
                    }

                    event_name.clear();
                    data_buf.clear();
                } else if let Some(rest) = line.strip_prefix("event:") {
                    event_name = rest.trim().to_string();
                } else if let Some(rest) = line.strip_prefix("data:") {
                    if !data_buf.is_empty() {
                        data_buf.push('\n');
                    }
                    data_buf.push_str(rest.trim());
                }
            }
        }

        if progress_visible {
            emit_search_chunk(&mut full_output, on_stdout, "\r\x1b[K".to_string()).await?;
        }

        if !saw_done {
            emit_search_chunk(
                &mut full_output,
                on_stdout,
                render_search_summary(query, search_limit, total_searched, total_matched, has_more),
            )
            .await?;
        }

        Ok(full_output)
    }
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

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
    fn test_validate_args_search_accepts_max_scan_and_max_results() {
        let executor = RemoteInvokeExecutor::new("127.0.0.1", 8800);
        let cmd = RemoteCommand {
            command: "traffic.search".to_string(),
            args_json: Some(r#"{"query":"hello","max_results":7,"max_scan":23}"#.to_string()),
            ..Default::default()
        };
        let args = executor
            .parse_and_validate_args(&cmd)
            .expect("args should parse");
        assert_eq!(args.query.as_deref(), Some("hello"));
        assert_eq!(args.max_results, Some(7));
        assert_eq!(args.max_scan, Some(23));
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
        assert_eq!(args.max_results, None);
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
    async fn test_execute_shell_exec_shell_text() {
        let executor = RemoteInvokeExecutor::new("127.0.0.1", 8800);
        let cmd = RemoteCommand {
            kind: super::super::types::CommandKind::ShellExec,
            exec_mode: Some(ShellExecMode::ShellText),
            command_text: Some("printf hello".to_string()),
            ..Default::default()
        };

        let resp = executor.execute(&cmd).await.expect("shell exec response");
        assert_eq!(resp.exit_code, 0);
        assert_eq!(resp.stdout.as_deref(), Some("hello"));
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
        assert!(joined_chunks.contains("Searching... 12 records scanned, 0 matched"));
        assert!(joined_chunks.contains("566961"));
        assert!(joined_chunks.contains("/nextoncall/profile"));
        assert!(joined_chunks.contains("Found 1 matches"));
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

        assert!(stdout.contains("Scanned 20 records"));
        assert!(stdout.contains("limit: 2"));
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
}
