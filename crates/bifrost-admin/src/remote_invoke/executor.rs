use std::time::Instant;

use bifrost_core::{direct_reqwest_client_builder, BifrostError, Result};
use sha1::{Digest, Sha1};
use tracing::{debug, warn};

use super::types::{is_allowed_command, RemoteCommand, RemoteInvokeResponse};

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
        if !is_allowed_command(&command.command) {
            warn!(
                command = %command.command,
                "remote invoke rejected: command not in whitelist"
            );
            return Ok(RemoteInvokeResponse {
                exit_code: -1,
                stdout: None,
                stderr: Some(format!("command '{}' is not allowed", command.command)),
                stdout_digest: None,
                stderr_digest: None,
                duration_ms: 0,
            });
        }

        let args = self.parse_and_validate_args(command)?;

        let start = Instant::now();
        let result = self.dispatch(&command.command, &args).await;
        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(body) => {
                let stdout_digest = Some(sha1_hex(&body));
                debug!(
                    command = %command.command,
                    duration_ms,
                    body_len = body.len(),
                    "remote invoke completed"
                );
                Ok(RemoteInvokeResponse {
                    exit_code: 0,
                    stdout: Some(body),
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
                    stderr_digest: None,
                    duration_ms,
                })
            }
        }
    }

    fn parse_and_validate_args(&self, command: &RemoteCommand) -> Result<CommandArgs> {
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

    async fn dispatch(&self, command: &str, args: &CommandArgs) -> Result<String> {
        match command {
            "status" => self.get_status().await,
            "traffic.list" => self.list_traffic(args).await,
            "traffic.get" => {
                let id = args.id.as_deref().ok_or_else(|| {
                    BifrostError::Config("traffic.get requires 'id' arg".to_string())
                })?;
                self.get_traffic(id, args).await
            }
            "traffic.search" | "search.get" => {
                let query = args.query.as_deref().ok_or_else(|| {
                    BifrostError::Config(format!("{} requires 'query' arg", command))
                })?;
                self.search(query).await
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
        let url = format!("{}/_bifrost/api/traffic/{}", self.base_url(), id);
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
                id
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
                id
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

    async fn search(&self, query: &str) -> Result<String> {
        let url = format!("{}/_bifrost/api/search", self.base_url());
        let payload = serde_json::json!({
            "keyword": query,
        });
        let resp = self
            .http
            .post(&url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| BifrostError::Network(format!("search request failed: {}", e)))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| BifrostError::Network(format!("failed to read search body: {}", e)))?;
        if !status.is_success() {
            return Err(BifrostError::Network(format!(
                "search returned HTTP {}: {}",
                status, body
            )));
        }
        Ok(body)
    }
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

fn sha1_hex(input: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(input.as_bytes());
    let result = hasher.finalize();
    result.iter().fold(String::with_capacity(40), |mut acc, b| {
        use std::fmt::Write;
        let _ = write!(acc, "{:02x}", b);
        acc
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        };
        let args = executor.parse_and_validate_args(&cmd);
        assert!(args.is_err());
    }

    #[test]
    fn test_validate_args_no_args() {
        let executor = RemoteInvokeExecutor::new("127.0.0.1", 8800);
        let cmd = RemoteCommand {
            command: "status".to_string(),
            args_json: None,
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
        };
        let resp = executor.execute(&cmd).await.unwrap();
        assert_eq!(resp.exit_code, -1);
        assert!(resp.stderr.unwrap().contains("not allowed"));
    }
}
