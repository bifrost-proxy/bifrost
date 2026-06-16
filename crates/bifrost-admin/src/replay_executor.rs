use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use bifrost_core::{
    Protocol, RequestContext, ResolvedRules, Rule, RuleParser, RulesResolver, ValueStore,
};
use bifrost_script::ResponseData;
use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::client::conn::http1::Builder as ClientBuilder;
use hyper::{Request, Response, Uri};
use hyper_util::rt::TokioIo;
use rustls::pki_types::ServerName;
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio::sync::Semaphore;
use tokio_rustls::TlsConnector;
use tracing::{error, info, warn};

use crate::replay_db::{ReplayHistory, RuleConfig, RuleMode, MAX_CONCURRENT_REPLAYS};
use crate::replay_scripts::{
    apply_replay_decode_scripts, collect_script_rules, execute_replay_request_scripts,
    execute_replay_response_scripts, request_data_from_replay, response_data_from_replay,
    ReplayScriptRules,
};
use crate::request_rules::{apply_all_request_rules, build_applied_rules, AppliedRequest};
use crate::state::SharedAdminState;
use crate::traffic::{MatchedRule, RequestTiming, TrafficRecord};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayExecuteRequest {
    pub request: ReplayRequestData,
    pub rule_config: RuleConfig,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

pub const DEFAULT_TIMEOUT_MS: u64 = 10_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayRequestData {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayExecuteResponse {
    pub traffic_id: String,
    pub status: u16,
    pub headers: Vec<(String, String)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    pub duration_ms: u64,
    pub applied_rules: Vec<MatchedRule>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug)]
pub enum ReplayError {
    TooManyConcurrent,
    InvalidUrl(String),
    ConnectionFailed(String),
    RequestFailed(String),
    Internal(String),
}

impl std::fmt::Display for ReplayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReplayError::TooManyConcurrent => {
                write!(f, "Too many concurrent replay requests")
            }
            ReplayError::InvalidUrl(msg) => write!(f, "Invalid URL: {}", msg),
            ReplayError::ConnectionFailed(msg) => write!(f, "Connection failed: {}", msg),
            ReplayError::RequestFailed(msg) => write!(f, "Request failed: {}", msg),
            ReplayError::Internal(msg) => write!(f, "Internal error: {}", msg),
        }
    }
}

impl std::error::Error for ReplayError {}

pub type SharedReplayExecutor = Arc<ReplayExecutor>;

static REPLAY_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub struct ReplayExecutor {
    semaphore: Arc<Semaphore>,
    admin_state: SharedAdminState,
    unsafe_ssl: bool,
}

impl ReplayExecutor {
    pub fn new(admin_state: SharedAdminState, unsafe_ssl: bool) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(MAX_CONCURRENT_REPLAYS)),
            admin_state,
            unsafe_ssl,
        }
    }

    pub async fn execute(
        &self,
        request: ReplayExecuteRequest,
    ) -> Result<ReplayExecuteResponse, ReplayError> {
        let permit = self
            .semaphore
            .clone()
            .try_acquire_owned()
            .map_err(|_| ReplayError::TooManyConcurrent)?;

        let result = self.execute_inner(request).await;
        drop(permit);
        result
    }

    async fn execute_inner(
        &self,
        request: ReplayExecuteRequest,
    ) -> Result<ReplayExecuteResponse, ReplayError> {
        let start_time = Instant::now();
        let replay_id = format!("replay-{}", REPLAY_SEQUENCE.fetch_add(1, Ordering::SeqCst));

        let url = &request.request.url;
        let method = &request.request.method;

        info!(
            replay_id = %replay_id,
            method = %method,
            url = %url,
            "[REPLAY] Starting replay request"
        );

        let (resolved_rules, matched_rules, values) =
            self.resolve_rules(&request.rule_config, url, method);

        let rules_to_apply = build_applied_rules(&resolved_rules);
        let script_rules = collect_script_rules(&resolved_rules);

        let original_body = request.request.body.as_ref().map(|s| s.as_bytes());
        let mut applied_request = apply_all_request_rules(
            url,
            method,
            &request.request.headers,
            original_body,
            &rules_to_apply,
            true,
        )
        .map_err(|e| ReplayError::Internal(format!("Failed to apply rules: {}", e)))?;

        let mut req_script_results = Vec::new();
        if !script_rules.req_scripts.is_empty() {
            req_script_results = execute_replay_request_scripts(
                &self.admin_state,
                &script_rules.req_scripts,
                &resolved_rules,
                &replay_id,
                &applied_request.url,
                &mut applied_request.method,
                &mut applied_request.headers,
                &mut applied_request.body,
                &values,
            )
            .await;
        }

        info!(
            replay_id = %replay_id,
            original_url = %url,
            applied_url = %applied_request.url,
            original_method = %method,
            applied_method = %applied_request.method,
            "[REPLAY] Applied request rules"
        );

        let uri: Uri = applied_request
            .url
            .parse()
            .map_err(|e| ReplayError::InvalidUrl(format!("{}", e)))?;

        let is_https = uri.scheme_str() == Some("https");
        let host = uri
            .host()
            .ok_or_else(|| ReplayError::InvalidUrl("Missing host".to_string()))?
            .to_string();
        let port = uri.port_u16().unwrap_or(if is_https { 443 } else { 80 });
        let path = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");

        let mock_response = self.check_mock_response(&resolved_rules);
        let needs_real_request = if request.rule_config.mode == RuleMode::None {
            true
        } else {
            self.needs_real_request(&resolved_rules)
        };

        let mut timing = RequestTiming::default();

        // timeout_ms 只用于“连接建立/握手/首包(headers)获取”的超时控制。
        // 不用于整个请求生命周期，避免对长连接（如 SSE）造成错误断开。
        let establish_timeout =
            std::time::Duration::from_millis(request.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS));

        let (status, response_headers, response_body) = if let Some(ref mock) = mock_response {
            if !needs_real_request {
                info!(
                    replay_id = %replay_id,
                    status = mock.status,
                    rules_count = matched_rules.len(),
                    "[REPLAY] Returning mock response from rules (no real request needed)"
                );
                timing.dns_ms = Some(0);
                timing.connect_ms = Some(0);
                timing.tls_ms = Some(0);
                (mock.status, mock.headers.clone(), mock.body.clone())
            } else {
                let body_str = applied_request
                    .body
                    .as_ref()
                    .map(|b| String::from_utf8_lossy(b).to_string());
                match self
                    .try_real_request(
                        &host,
                        port,
                        is_https,
                        &applied_request.method,
                        path,
                        &applied_request.headers,
                        body_str.as_deref(),
                        &mut timing,
                        establish_timeout,
                    )
                    .await
                {
                    Ok((s, h, b)) => (s, h, b),
                    Err(e) => {
                        warn!(
                            replay_id = %replay_id,
                            error = %e,
                            "[REPLAY] Real request failed, using mock response as fallback"
                        );
                        timing.dns_ms = Some(0);
                        timing.connect_ms = Some(0);
                        timing.tls_ms = Some(0);
                        (mock.status, mock.headers.clone(), mock.body.clone())
                    }
                }
            }
        } else if needs_real_request {
            let body_str = applied_request
                .body
                .as_ref()
                .map(|b| String::from_utf8_lossy(b).to_string());
            match self
                .try_real_request(
                    &host,
                    port,
                    is_https,
                    &applied_request.method,
                    path,
                    &applied_request.headers,
                    body_str.as_deref(),
                    &mut timing,
                    establish_timeout,
                )
                .await
            {
                Ok((s, h, b)) => (s, h, b),
                Err(e) => {
                    let duration_ms = start_time.elapsed().as_millis() as u64;
                    error!(
                        replay_id = %replay_id,
                        error = %e,
                        "[REPLAY] Request failed with no fallback rules"
                    );
                    return Ok(ReplayExecuteResponse {
                        traffic_id: String::new(),
                        status: 0,
                        headers: vec![],
                        body: None,
                        duration_ms,
                        applied_rules: matched_rules,
                        error: Some(e.to_string()),
                    });
                }
            }
        } else {
            info!(
                replay_id = %replay_id,
                "[REPLAY] No mock response and no real request needed, returning empty response"
            );
            timing.dns_ms = Some(0);
            timing.connect_ms = Some(0);
            timing.tls_ms = Some(0);
            (
                200,
                vec![("content-type".to_string(), "text/plain".to_string())],
                None,
            )
        };

        let (mut status, mut response_headers, mut response_body) =
            self.apply_response_rules(&resolved_rules, status, response_headers, response_body);

        let mut res_script_results = Vec::new();
        if !script_rules.res_scripts.is_empty() {
            res_script_results = execute_replay_response_scripts(
                &self.admin_state,
                &script_rules.res_scripts,
                &resolved_rules,
                &replay_id,
                &applied_request.url,
                &applied_request.method,
                &applied_request.headers,
                &mut status,
                &mut response_headers,
                &mut response_body,
                &values,
            )
            .await;
        }

        let duration_ms = start_time.elapsed().as_millis() as u64;
        timing.total_ms = duration_ms;

        let traffic_id = self
            .record_traffic(
                &replay_id,
                &request,
                &applied_request,
                status,
                &response_headers,
                response_body.as_deref(),
                duration_ms,
                &matched_rules,
                &timing,
                &resolved_rules,
                &script_rules,
                &values,
                &req_script_results,
                &res_script_results,
            )
            .await;

        self.record_history(
            request.request_id.as_deref(),
            &traffic_id,
            method,
            url,
            status,
            duration_ms,
            &request.rule_config,
        )
        .await;

        info!(
            replay_id = %replay_id,
            traffic_id = %traffic_id,
            status = status,
            duration_ms = duration_ms,
            "[REPLAY] Completed replay request"
        );

        Ok(ReplayExecuteResponse {
            traffic_id,
            status,
            headers: response_headers,
            body: response_body,
            duration_ms,
            applied_rules: matched_rules,
            error: None,
        })
    }

    fn check_mock_response(&self, resolved_rules: &ResolvedRules) -> Option<MockResponse> {
        let mut status: Option<u16> = None;
        let mut body: Option<String> = None;
        let mut headers: Vec<(String, String)> = vec![];

        for rule in &resolved_rules.rules {
            match rule.rule.protocol {
                Protocol::StatusCode | Protocol::ReplaceStatus => {
                    if let Ok(code) = rule.resolved_value.parse::<u16>() {
                        status = Some(code);
                    }
                }
                Protocol::ResBody => {
                    let content = extract_inline_content(&rule.resolved_value);
                    body = Some(content);
                }
                Protocol::ResHeaders => {
                    if let Some(parsed) = parse_headers(&rule.resolved_value) {
                        headers.extend(parsed);
                    }
                }
                Protocol::Host | Protocol::XHost => {}
                _ => {}
            }
        }

        if status.is_some() || body.is_some() {
            let final_status = status.unwrap_or(200);
            let mut final_headers = headers;

            if !final_headers
                .iter()
                .any(|(k, _)| k.to_lowercase() == "content-type")
            {
                final_headers.push(("content-type".to_string(), "text/plain".to_string()));
            }

            Some(MockResponse {
                status: final_status,
                headers: final_headers,
                body,
            })
        } else {
            None
        }
    }

    fn needs_real_request(&self, resolved_rules: &ResolvedRules) -> bool {
        let mut has_mock_body_or_status = false;

        for rule in &resolved_rules.rules {
            match rule.rule.protocol {
                Protocol::Host | Protocol::XHost | Protocol::Proxy => {
                    return true;
                }
                Protocol::ResHeaders => {
                    return true;
                }
                Protocol::StatusCode | Protocol::ReplaceStatus | Protocol::ResBody => {
                    has_mock_body_or_status = true;
                }
                _ => {}
            }
        }

        !has_mock_body_or_status
    }

    fn apply_response_rules(
        &self,
        resolved_rules: &ResolvedRules,
        status: u16,
        headers: Vec<(String, String)>,
        body: Option<String>,
    ) -> (u16, Vec<(String, String)>, Option<String>) {
        crate::replay_response_rules::apply_response_rules(resolved_rules, status, headers, body)
    }

    #[allow(clippy::too_many_arguments)]
    async fn try_real_request(
        &self,
        host: &str,
        port: u16,
        is_https: bool,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: Option<&str>,
        timing: &mut RequestTiming,
        establish_timeout: std::time::Duration,
    ) -> Result<(u16, Vec<(String, String)>, Option<String>), ReplayError> {
        let deadline = Instant::now()
            .checked_add(establish_timeout)
            .unwrap_or_else(Instant::now);

        let dns_start = Instant::now();
        let connect_addr = format!("{}:{}", host, port);
        timing.dns_ms = Some(dns_start.elapsed().as_millis() as u64);

        let connect_start = Instant::now();
        let connect_remaining = deadline.saturating_duration_since(Instant::now());
        if connect_remaining.is_zero() {
            return Err(ReplayError::ConnectionFailed(format!(
                "Request timeout after {}ms",
                establish_timeout.as_millis()
            )));
        }
        let tcp_stream = match tokio::time::timeout(
            connect_remaining,
            TcpStream::connect(&connect_addr),
        )
        .await
        {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                return Err(ReplayError::ConnectionFailed(format!(
                    "Failed to connect to {}: {}",
                    connect_addr, e
                )))
            }
            Err(_) => {
                return Err(ReplayError::ConnectionFailed(format!(
                    "Connect timeout after {}ms",
                    establish_timeout.as_millis()
                )))
            }
        };
        timing.connect_ms = Some(connect_start.elapsed().as_millis() as u64);

        let (status, response_headers, response_body, tls_ms) = if is_https {
            let tls_start = Instant::now();
            let (s, h, b) = self
                .send_https_request(
                    tcp_stream,
                    host,
                    method,
                    path,
                    headers,
                    body,
                    establish_timeout,
                    deadline,
                )
                .await?;
            (s, h, b, Some(tls_start.elapsed().as_millis() as u64))
        } else {
            let (s, h, b) = self
                .send_http_request(
                    tcp_stream,
                    host,
                    method,
                    path,
                    headers,
                    body,
                    establish_timeout,
                    deadline,
                )
                .await?;
            (s, h, b, None)
        };
        timing.tls_ms = tls_ms;

        Ok((status, response_headers, response_body))
    }

    fn resolve_rules(
        &self,
        rule_config: &RuleConfig,
        url: &str,
        method: &str,
    ) -> (ResolvedRules, Vec<MatchedRule>, HashMap<String, String>) {
        match rule_config.mode {
            RuleMode::None => (ResolvedRules::default(), vec![], self.load_values()),
            RuleMode::Custom => {
                if let Some(ref custom_rules) = rule_config.custom_rules {
                    self.resolve_custom_rules(custom_rules, url, method)
                } else {
                    (ResolvedRules::default(), vec![], self.load_values())
                }
            }
            RuleMode::Enabled | RuleMode::Selected => {
                let rules_storage = &self.admin_state.rules_storage;
                let selected = if rule_config.mode == RuleMode::Selected {
                    Some(&rule_config.selected_rules)
                } else {
                    None
                };
                self.resolve_from_storage(rules_storage, url, method, selected)
            }
        }
    }

    fn resolve_custom_rules(
        &self,
        custom_rules: &str,
        url: &str,
        method: &str,
    ) -> (ResolvedRules, Vec<MatchedRule>, HashMap<String, String>) {
        let mut values = self.load_values();
        let parser = RuleParser::with_values(values.clone());
        let parsed_rules = match parser.parse_rules_with_inline_values(custom_rules) {
            Ok((r, inline_values)) => {
                for (key, value) in inline_values {
                    values.entry(key).or_insert(value);
                }
                r
            }
            Err(e) => {
                warn!(error = %e, "[REPLAY] Failed to parse custom rules");
                return (ResolvedRules::default(), vec![], values);
            }
        };
        let rules = parsed_rules
            .into_iter()
            .enumerate()
            .map(|(i, r)| r.with_source("custom".to_string(), i + 1))
            .collect::<Vec<_>>();

        if rules.is_empty() {
            return (ResolvedRules::default(), vec![], values);
        }

        let resolver = RulesResolver::new(rules).with_values(values.clone());
        let ctx = RequestContext::from_url(url).with_method(method);
        let resolved = resolver.resolve(&ctx);

        let matched: Vec<MatchedRule> = resolved
            .rules
            .iter()
            .map(|r| MatchedRule {
                pattern: r.rule.pattern.clone(),
                protocol: r.rule.protocol.to_str().to_string(),
                value: r.resolved_value.clone(),
                rule_name: r.rule.file.clone(),
                raw: Some(r.rule.raw.clone()),
                line: r.rule.line,
            })
            .collect();

        (resolved, matched, values)
    }

    fn resolve_from_storage(
        &self,
        rules_storage: &bifrost_storage::RulesStorage,
        url: &str,
        method: &str,
        selected_rules: Option<&Vec<String>>,
    ) -> (ResolvedRules, Vec<MatchedRule>, HashMap<String, String>) {
        let mut all_rules: Vec<Rule> = vec![];
        let mut values = self.load_values();

        let rule_files = match rules_storage.load_all() {
            Ok(files) => files,
            Err(e) => {
                warn!(error = %e, "[REPLAY] Failed to load rules");
                return (ResolvedRules::default(), vec![], values);
            }
        };

        info!(
            rule_files_count = rule_files.len(),
            "[REPLAY] Loading rules from storage"
        );
        let catalog_rule_files = rules_storage
            .load_all_with_subdirs()
            .unwrap_or_else(|_| rule_files.clone());
        let reference_catalog = bifrost_storage::build_rule_reference_catalog(&catalog_rule_files);

        for rule_file in rule_files {
            if !rule_file.enabled {
                info!(
                    rule_name = %rule_file.name,
                    "[REPLAY] Skipping disabled rule file"
                );
                continue;
            }

            if let Some(selected) = selected_rules {
                if !selected.contains(&rule_file.name) {
                    info!(
                        rule_name = %rule_file.name,
                        "[REPLAY] Skipping unselected rule file"
                    );
                    continue;
                }
            }

            let parser = RuleParser::with_values(values.clone());
            let source_name = bifrost_storage::rule_reference_key(&rule_file);
            let expanded_content = match bifrost_core::expand_rule_references(
                &source_name,
                &rule_file.content,
                &reference_catalog,
            ) {
                Ok(content) => content,
                Err(e) => {
                    warn!(
                        rule_name = %rule_file.name,
                        error = %e,
                        "[REPLAY] Failed to expand rule references"
                    );
                    continue;
                }
            };
            match parser.parse_rules_with_inline_values(&expanded_content) {
                Ok((parsed, inline_values)) => {
                    for (key, value) in inline_values {
                        values.entry(key).or_insert(value);
                    }
                    info!(
                        rule_name = %rule_file.name,
                        parsed_count = parsed.len(),
                        "[REPLAY] Parsed rule file"
                    );
                    let rules_with_source: Vec<Rule> = parsed
                        .into_iter()
                        .enumerate()
                        .map(|(i, r)| r.with_source(rule_file.name.clone(), i + 1))
                        .collect();
                    all_rules.extend(rules_with_source);
                }
                Err(e) => {
                    warn!(
                        rule_name = %rule_file.name,
                        error = %e,
                        "[REPLAY] Failed to parse rule file"
                    );
                }
            }
        }

        info!(total_rules = all_rules.len(), "[REPLAY] Total rules loaded");

        if all_rules.is_empty() {
            return (ResolvedRules::default(), vec![], values);
        }

        let resolver = RulesResolver::new(all_rules.clone()).with_values(values.clone());

        let ctx = RequestContext::from_url(url).with_method(method);
        info!(
            url = %url,
            host = %ctx.host,
            path = %ctx.path,
            "[REPLAY] Resolving rules for request"
        );

        let resolved = resolver.resolve(&ctx);
        info!(
            matched_count = resolved.rules.len(),
            "[REPLAY] Rules matching completed"
        );

        for rule in &resolved.rules {
            info!(
                pattern = %rule.rule.pattern,
                protocol = %rule.rule.protocol.to_str(),
                value = %rule.resolved_value,
                "[REPLAY] Matched rule"
            );
        }

        let applied_rules: Vec<MatchedRule> = resolved
            .rules
            .iter()
            .map(|r| MatchedRule {
                pattern: r.rule.pattern.clone(),
                protocol: r.rule.protocol.to_str().to_string(),
                value: r.resolved_value.clone(),
                rule_name: r.rule.file.clone(),
                raw: Some(r.rule.raw.clone()),
                line: r.rule.line,
            })
            .collect();

        info!(
            url = %url,
            matched_count = applied_rules.len(),
            "[REPLAY] Rules resolved"
        );

        (resolved, applied_rules, values)
    }

    fn load_values(&self) -> HashMap<String, String> {
        if let Some(ref values_storage) = self.admin_state.values_storage {
            let guard = values_storage.read();
            return guard.as_hashmap();
        }
        HashMap::new()
    }

    #[allow(clippy::too_many_arguments)]
    async fn send_http_request(
        &self,
        stream: TcpStream,
        host: &str,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: Option<&str>,
        establish_timeout: std::time::Duration,
        deadline: Instant,
    ) -> Result<(u16, Vec<(String, String)>, Option<String>), ReplayError> {
        let io = TokioIo::new(stream);

        let handshake_remaining = deadline.saturating_duration_since(Instant::now());
        if handshake_remaining.is_zero() {
            return Err(ReplayError::ConnectionFailed(format!(
                "Request timeout after {}ms",
                establish_timeout.as_millis()
            )));
        }
        let (mut sender, conn) =
            match tokio::time::timeout(handshake_remaining, ClientBuilder::new().handshake(io))
                .await
            {
                Ok(Ok(v)) => v,
                Ok(Err(e)) => {
                    return Err(ReplayError::ConnectionFailed(format!(
                        "HTTP handshake failed: {}",
                        e
                    )))
                }
                Err(_) => {
                    return Err(ReplayError::ConnectionFailed(format!(
                        "HTTP handshake timeout after {}ms",
                        establish_timeout.as_millis()
                    )))
                }
            };

        tokio::spawn(async move {
            if let Err(e) = conn.await {
                error!(error = %e, "[REPLAY] HTTP connection error");
            }
        });

        let mut req_builder = Request::builder()
            .method(method)
            .uri(path)
            .header("Host", host);

        for (key, value) in headers {
            let key_lower = key.to_lowercase();
            if key_lower == "host" || key_lower == "content-length" {
                continue;
            }
            req_builder = req_builder.header(key, value);
        }

        let body_bytes = body.map(|b| Bytes::from(b.to_string())).unwrap_or_default();
        if !body_bytes.is_empty() {
            req_builder = req_builder.header("Content-Length", body_bytes.len().to_string());
        }

        let request = req_builder
            .body(http_body_util::Full::new(body_bytes))
            .map_err(|e| ReplayError::RequestFailed(format!("Failed to build request: {}", e)))?;

        let send_remaining = deadline.saturating_duration_since(Instant::now());
        if send_remaining.is_zero() {
            return Err(ReplayError::RequestFailed(format!(
                "Request timeout after {}ms",
                establish_timeout.as_millis()
            )));
        }
        let response = match tokio::time::timeout(send_remaining, sender.send_request(request))
            .await
        {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return Err(ReplayError::RequestFailed(format!("Request failed: {}", e))),
            Err(_) => {
                return Err(ReplayError::RequestFailed(format!(
                    "Request timeout after {}ms",
                    establish_timeout.as_millis()
                )))
            }
        };

        self.parse_response(response).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn send_https_request(
        &self,
        stream: TcpStream,
        host: &str,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: Option<&str>,
        establish_timeout: std::time::Duration,
        deadline: Instant,
    ) -> Result<(u16, Vec<(String, String)>, Option<String>), ReplayError> {
        let tls_config = get_tls_client_config(self.unsafe_ssl);
        let connector = TlsConnector::from(Arc::new(tls_config));

        let server_name = ServerName::try_from(host.to_string())
            .map_err(|e| ReplayError::ConnectionFailed(format!("Invalid server name: {}", e)))?;

        let tls_remaining = deadline.saturating_duration_since(Instant::now());
        if tls_remaining.is_zero() {
            return Err(ReplayError::ConnectionFailed(format!(
                "Request timeout after {}ms",
                establish_timeout.as_millis()
            )));
        }
        let tls_stream =
            match tokio::time::timeout(tls_remaining, connector.connect(server_name, stream)).await
            {
                Ok(Ok(s)) => s,
                Ok(Err(e)) => {
                    return Err(ReplayError::ConnectionFailed(format!(
                        "TLS handshake failed: {}",
                        e
                    )))
                }
                Err(_) => {
                    return Err(ReplayError::ConnectionFailed(format!(
                        "TLS handshake timeout after {}ms",
                        establish_timeout.as_millis()
                    )))
                }
            };

        let io = TokioIo::new(tls_stream);

        let handshake_remaining = deadline.saturating_duration_since(Instant::now());
        if handshake_remaining.is_zero() {
            return Err(ReplayError::ConnectionFailed(format!(
                "Request timeout after {}ms",
                establish_timeout.as_millis()
            )));
        }
        let (mut sender, conn) =
            match tokio::time::timeout(handshake_remaining, ClientBuilder::new().handshake(io))
                .await
            {
                Ok(Ok(v)) => v,
                Ok(Err(e)) => {
                    return Err(ReplayError::ConnectionFailed(format!(
                        "HTTPS handshake failed: {}",
                        e
                    )))
                }
                Err(_) => {
                    return Err(ReplayError::ConnectionFailed(format!(
                        "HTTPS handshake timeout after {}ms",
                        establish_timeout.as_millis()
                    )))
                }
            };

        tokio::spawn(async move {
            if let Err(e) = conn.await {
                error!(error = %e, "[REPLAY] HTTPS connection error");
            }
        });

        let mut req_builder = Request::builder()
            .method(method)
            .uri(path)
            .header("Host", host);

        for (key, value) in headers {
            let key_lower = key.to_lowercase();
            if key_lower == "host" || key_lower == "content-length" {
                continue;
            }
            req_builder = req_builder.header(key, value);
        }

        let body_bytes = body.map(|b| Bytes::from(b.to_string())).unwrap_or_default();
        if !body_bytes.is_empty() {
            req_builder = req_builder.header("Content-Length", body_bytes.len().to_string());
        }

        let request = req_builder
            .body(http_body_util::Full::new(body_bytes))
            .map_err(|e| ReplayError::RequestFailed(format!("Failed to build request: {}", e)))?;

        let send_remaining = deadline.saturating_duration_since(Instant::now());
        if send_remaining.is_zero() {
            return Err(ReplayError::RequestFailed(format!(
                "Request timeout after {}ms",
                establish_timeout.as_millis()
            )));
        }
        let response = match tokio::time::timeout(send_remaining, sender.send_request(request))
            .await
        {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return Err(ReplayError::RequestFailed(format!("Request failed: {}", e))),
            Err(_) => {
                return Err(ReplayError::RequestFailed(format!(
                    "Request timeout after {}ms",
                    establish_timeout.as_millis()
                )))
            }
        };

        self.parse_response(response).await
    }

    async fn parse_response<B>(
        &self,
        response: Response<B>,
    ) -> Result<(u16, Vec<(String, String)>, Option<String>), ReplayError>
    where
        B: hyper::body::Body,
        B::Error: std::fmt::Display,
    {
        let status = response.status().as_u16();
        let headers: Vec<(String, String)> = response
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();

        let body_bytes = response
            .into_body()
            .collect()
            .await
            .map_err(|e| {
                ReplayError::RequestFailed(format!("Failed to read response body: {}", e))
            })?
            .to_bytes();

        let body = if body_bytes.is_empty() {
            None
        } else {
            Some(String::from_utf8_lossy(&body_bytes).to_string())
        };

        Ok((status, headers, body))
    }

    #[allow(clippy::too_many_arguments)]
    async fn record_traffic(
        &self,
        replay_id: &str,
        request: &ReplayExecuteRequest,
        applied_request: &AppliedRequest,
        status: u16,
        response_headers: &[(String, String)],
        response_body: Option<&str>,
        duration_ms: u64,
        applied_rules: &[MatchedRule],
        timing: &RequestTiming,
        resolved_rules: &ResolvedRules,
        script_rules: &ReplayScriptRules,
        values: &HashMap<String, String>,
        req_script_results: &[bifrost_script::ScriptExecutionResult],
        res_script_results: &[bifrost_script::ScriptExecutionResult],
    ) -> String {
        let traffic_id = format!("{}-{}", replay_id, uuid::Uuid::new_v4());
        let timestamp = chrono::Utc::now().timestamp_millis() as u64;

        let original_url = &request.request.url;
        let actual_url = &applied_request.url;
        let uri: Uri = actual_url.parse().unwrap_or_default();
        let host = uri.host().unwrap_or("unknown").to_string();
        let path = uri.path().to_string();
        let is_https = uri.scheme_str() == Some("https");

        let content_type = response_headers
            .iter()
            .find(|(k, _)| k.to_lowercase() == "content-type")
            .map(|(_, v)| v.clone());

        let request_content_type = applied_request
            .headers
            .iter()
            .find(|(k, _)| k.to_lowercase() == "content-type")
            .map(|(_, v)| v.clone());

        let request_size = applied_request.body.as_ref().map(|b| b.len()).unwrap_or(0);
        let response_size = response_body.map(|b| b.len()).unwrap_or(0);

        let mut request_body_for_storage = applied_request.body.clone();
        let mut response_body_for_storage = response_body.map(|body| Bytes::from(body.to_string()));
        let mut raw_request_body_ref = None;
        let mut raw_response_body_ref = None;
        let mut decode_req_script_results = Vec::new();
        let mut decode_res_script_results = Vec::new();

        if !script_rules.decode_scripts.is_empty() {
            let request_data = request_data_from_replay(
                &applied_request.url,
                &applied_request.method,
                &applied_request.headers,
                applied_request.body.as_ref(),
            );
            let response_data = response_data_from_replay(
                status,
                response_headers,
                response_body,
                request_data.clone(),
            );

            if let Some(body) = request_body_for_storage.clone() {
                if let Some(ref body_store) = self.admin_state.body_store {
                    raw_request_body_ref = body_store.read().store(&traffic_id, "req_raw", &body);
                }
                let (decoded, results) = apply_replay_decode_scripts(
                    &self.admin_state,
                    &script_rules.decode_scripts,
                    &script_rules.bp_scripts,
                    "request",
                    replay_id,
                    resolved_rules,
                    &request_data,
                    &ResponseData {
                        request: request_data.clone(),
                        ..Default::default()
                    },
                    values,
                    body,
                )
                .await;
                request_body_for_storage = Some(decoded);
                decode_req_script_results = results;
            }

            if let Some(body) = response_body_for_storage.clone() {
                if let Some(ref body_store) = self.admin_state.body_store {
                    raw_response_body_ref = body_store.read().store(&traffic_id, "res_raw", &body);
                }
                let (decoded, results) = apply_replay_decode_scripts(
                    &self.admin_state,
                    &script_rules.decode_scripts,
                    &script_rules.bp_scripts,
                    "response",
                    replay_id,
                    resolved_rules,
                    &response_data.request,
                    &response_data,
                    values,
                    body,
                )
                .await;
                response_body_for_storage = Some(decoded);
                decode_res_script_results = results;
            }
        }

        let request_body_ref = request_body_for_storage.as_ref().and_then(|body| {
            self.admin_state
                .body_store
                .as_ref()
                .and_then(|body_store| body_store.read().store(&traffic_id, "req", body))
        });

        let response_body_ref = response_body_for_storage.as_ref().and_then(|body| {
            self.admin_state
                .body_store
                .as_ref()
                .and_then(|body_store| body_store.read().store(&traffic_id, "res", body))
        });

        let has_changes = actual_url != original_url
            || applied_request.method != request.request.method
            || applied_request.headers != request.request.headers;

        let record = TrafficRecord {
            id: traffic_id.clone(),
            sequence: 0,
            timestamp,
            host: host.clone(),
            method: applied_request.method.clone(),
            url: actual_url.clone(),
            path,
            status,
            protocol: if is_https { "https" } else { "http" }.to_string(),
            content_type,
            request_content_type,
            request_size,
            response_size,
            duration_ms,
            listener_port: 0,
            client_ip: "127.0.0.1".to_string(),
            client_app: Some("Bifrost Replay".to_string()),
            client_pid: None,
            client_path: None,
            is_tunnel: false,
            is_websocket: false,
            is_sse: false,
            is_h3: false,
            has_rule_hit: !applied_rules.is_empty(),
            is_replay: true,
            frame_count: 0,
            last_frame_id: 0,
            timing: Some(timing.clone()),
            request_headers: Some(applied_request.headers.clone()),
            original_response_headers: Some(response_headers.to_vec()),
            matched_rules: if applied_rules.is_empty() {
                None
            } else {
                Some(applied_rules.to_vec())
            },
            socket_status: None,
            request_body_ref,
            response_body_ref,
            derived_response_body_ref: None,
            raw_request_body_ref,
            raw_response_body_ref,
            actual_url: if has_changes {
                Some(actual_url.clone())
            } else {
                None
            },
            actual_host: if has_changes {
                Some(host.clone())
            } else {
                None
            },
            original_request_headers: if has_changes {
                Some(request.request.headers.clone())
            } else {
                None
            },
            response_headers: None,
            error_message: None,
            req_script_results: if req_script_results.is_empty() {
                None
            } else {
                Some(req_script_results.to_vec())
            },
            res_script_results: if res_script_results.is_empty() {
                None
            } else {
                Some(res_script_results.to_vec())
            },
            decode_req_script_results: if decode_req_script_results.is_empty() {
                None
            } else {
                Some(decode_req_script_results)
            },
            decode_res_script_results: if decode_res_script_results.is_empty() {
                None
            } else {
                Some(decode_res_script_results)
            },
            devtools_client_req_id: None,
        };

        if let Some(ref traffic_db) = self.admin_state.traffic_db_store {
            traffic_db.record(record);
        } else if let Some(ref async_writer) = self.admin_state.async_traffic_writer {
            async_writer.record(record);
        }

        traffic_id
    }

    #[allow(clippy::too_many_arguments)]
    async fn record_history(
        &self,
        request_id: Option<&str>,
        traffic_id: &str,
        method: &str,
        url: &str,
        status: u16,
        duration_ms: u64,
        rule_config: &RuleConfig,
    ) {
        if let Some(ref replay_db) = self.admin_state.replay_db_store {
            let history = ReplayHistory::new(
                request_id.map(|id| id.to_string()),
                traffic_id.to_string(),
                method.to_string(),
                url.to_string(),
                status,
                duration_ms,
                Some(rule_config.clone()),
            );
            if let Err(e) = replay_db.create_history(&history) {
                warn!(error = %e, "[REPLAY] Failed to record history");
            }
        }
    }
}

#[derive(Debug, Clone)]
struct MockResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Option<String>,
}

fn extract_inline_content(value: &str) -> String {
    if value.starts_with('{') && value.ends_with('}') && value.len() > 1 {
        value[1..value.len() - 1].to_string()
    } else {
        value.to_string()
    }
}

fn parse_headers(value: &str) -> Option<Vec<(String, String)>> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }

    let (content, use_colon) = if trimmed.starts_with('(') && trimmed.ends_with(')') {
        (&trimmed[1..trimmed.len() - 1], true)
    } else {
        (trimmed, trimmed.contains('\n') || trimmed.contains(':'))
    };

    let mut headers = Vec::new();

    let delimiter = if content.contains('\n') { '\n' } else { ',' };
    for part in content.split(delimiter) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let separator = if use_colon { ':' } else { '=' };
        if let Some(pos) = part.find(separator) {
            let key = part[..pos].trim().to_string();
            let val = part[pos + 1..].trim().to_string();
            if !key.is_empty() {
                headers.push((key, val));
            }
        }
    }

    if headers.is_empty() {
        None
    } else {
        Some(headers)
    }
}

fn get_tls_client_config(unsafe_ssl: bool) -> rustls::ClientConfig {
    use rustls::{ClientConfig, RootCertStore};

    if unsafe_ssl {
        ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoCertificateVerification {}))
            .with_no_client_auth()
    } else {
        let mut root_store = RootCertStore::empty();
        let certs = rustls_native_certs::load_native_certs();
        for cert in certs.certs {
            let _ = root_store.add(cert);
        }

        ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth()
    }
}

#[derive(Debug)]
struct NoCertificateVerification;

impl rustls::client::danger::ServerCertVerifier for NoCertificateVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP521_SHA512,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::ED25519,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bifrost_core::{matcher::WildcardMatcher, Protocol, ResolvedRule, ResolvedRules, Rule};
    use serde_json::json;
    use std::sync::Arc;

    fn make_resolved_rule(protocol: Protocol, resolved_value: &str) -> ResolvedRule {
        let matcher =
            Arc::new(WildcardMatcher::new("*").expect("failed to create wildcard matcher"));
        let rule = Rule::new(
            "*".to_string(),
            matcher,
            protocol,
            resolved_value.to_string(),
            format!("* {}://{}", protocol.to_str(), resolved_value),
        );
        ResolvedRule {
            rule,
            captures: None,
            resolved_value: resolved_value.to_string(),
        }
    }

    fn build_resolved_rules(rules: &[(Protocol, &str)]) -> ResolvedRules {
        let mut resolved = ResolvedRules::new();
        for (protocol, value) in rules {
            resolved.add(make_resolved_rule(*protocol, value));
        }
        resolved
    }

    #[test]
    fn extract_inline_content_strips_wrapping_braces() {
        assert_eq!(extract_inline_content("{hello}"), "hello");
        assert_eq!(extract_inline_content("no-braces"), "no-braces");
        assert_eq!(extract_inline_content("{}"), "");
    }

    #[test]
    fn parse_headers_supports_parens_and_colon_and_equals() {
        let value = "(X-One: a, X-Two: b)";
        let headers = parse_headers(value).unwrap();
        assert_eq!(
            headers,
            vec![
                ("X-One".to_string(), "a".to_string()),
                ("X-Two".to_string(), "b".to_string()),
            ]
        );

        let value2 = "X-One=a,X-Two=b";
        let headers2 = parse_headers(value2).unwrap();
        assert_eq!(headers2, headers);
    }

    #[test]
    fn parse_headers_ignores_empty_and_invalid_parts() {
        assert!(parse_headers("   ").is_none());
        assert!(parse_headers(", , ").is_none());

        let headers = parse_headers("X-Ok=1,missing,Y-Ok=2").unwrap();
        assert_eq!(headers.len(), 2);
        assert_eq!(headers[0], ("X-Ok".to_string(), "1".to_string()));
        assert_eq!(headers[1], ("Y-Ok".to_string(), "2".to_string()));
    }

    #[test]
    fn replay_error_display_messages_are_human_readable() {
        assert_eq!(
            ReplayError::TooManyConcurrent.to_string(),
            "Too many concurrent replay requests"
        );
        assert_eq!(
            ReplayError::InvalidUrl("bad".into()).to_string(),
            "Invalid URL: bad"
        );
        assert_eq!(
            ReplayError::ConnectionFailed("oops".into()).to_string(),
            "Connection failed: oops"
        );
        assert_eq!(
            ReplayError::RequestFailed("timeout".into()).to_string(),
            "Request failed: timeout"
        );
        assert_eq!(
            ReplayError::Internal("bug".into()).to_string(),
            "Internal error: bug"
        );
    }

    #[test]
    fn check_mock_response_combines_status_body_and_headers() {
        let temp_dir = tempfile::tempdir().expect("temp rules dir");
        let rules_storage = bifrost_storage::RulesStorage::with_dir(temp_dir.path().join("rules"))
            .expect("rules storage");
        let admin = Arc::new(crate::state::AdminState::new_for_test(0, rules_storage));
        let executor = ReplayExecutor::new(admin, false);

        let mut resolved = ResolvedRules::new();
        resolved.add(make_resolved_rule(Protocol::StatusCode, "201"));
        resolved.add(make_resolved_rule(Protocol::ResBody, "inline-body"));
        resolved.add(make_resolved_rule(
            Protocol::ResHeaders,
            "(X-Test: one, Content-Type: application/json)",
        ));

        let mock = executor
            .check_mock_response(&resolved)
            .expect("expected mock response");
        assert_eq!(mock.status, 201);
        assert_eq!(mock.body.as_deref(), Some("inline-body"));
        assert!(mock
            .headers
            .iter()
            .any(|(k, v)| k == "X-Test" && v == "one"));
        assert!(mock
            .headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("content-type") && v == "application/json"));
    }

    #[test]
    fn check_mock_response_infers_default_content_type() {
        let temp_dir = tempfile::tempdir().expect("temp rules dir");
        let rules_storage = bifrost_storage::RulesStorage::with_dir(temp_dir.path().join("rules"))
            .expect("rules storage");
        let admin = Arc::new(crate::state::AdminState::new_for_test(0, rules_storage));
        let executor = ReplayExecutor::new(admin, false);

        let resolved = build_resolved_rules(&[(Protocol::ResBody, "hello")]);

        let mock = executor
            .check_mock_response(&resolved)
            .expect("expected mock response");
        assert_eq!(mock.status, 200);
        assert_eq!(mock.body.as_deref(), Some("hello"));
        assert!(mock
            .headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("content-type") && v == "text/plain"));
    }

    #[test]
    fn check_mock_response_returns_none_when_no_mock_rules() {
        let temp_dir = tempfile::tempdir().expect("temp rules dir");
        let rules_storage = bifrost_storage::RulesStorage::with_dir(temp_dir.path().join("rules"))
            .expect("rules storage");
        let admin = Arc::new(crate::state::AdminState::new_for_test(0, rules_storage));
        let executor = ReplayExecutor::new(admin, false);

        let resolved = build_resolved_rules(&[(Protocol::Host, "example.com")]);

        assert!(executor.check_mock_response(&resolved).is_none());
    }

    #[test]
    fn needs_real_request_depends_on_ruleset() {
        let temp_dir = tempfile::tempdir().expect("temp rules dir");
        let rules_storage = bifrost_storage::RulesStorage::with_dir(temp_dir.path().join("rules"))
            .expect("rules storage");
        let admin = Arc::new(crate::state::AdminState::new_for_test(0, rules_storage));
        let executor = ReplayExecutor::new(admin, false);

        let only_mock_status = build_resolved_rules(&[(Protocol::StatusCode, "200")]);
        assert!(!executor.needs_real_request(&only_mock_status));

        let only_host = build_resolved_rules(&[(Protocol::Host, "example.com")]);
        assert!(executor.needs_real_request(&only_host));

        let res_body_and_headers = build_resolved_rules(&[
            (Protocol::ResBody, "ok"),
            (Protocol::ResHeaders, "X-Test: v"),
        ]);
        assert!(executor.needs_real_request(&res_body_and_headers));

        let unrelated = build_resolved_rules(&[(Protocol::ReqHeaders, "X-Test=v")]);
        assert!(executor.needs_real_request(&unrelated));
    }

    #[test]
    fn replay_request_dto_serde_roundtrip_and_defaults() {
        let request_data = ReplayRequestData {
            method: "POST".into(),
            url: "http://example.test/".into(),
            headers: vec![("X-Test".into(), "1".into())],
            body: None,
        };
        let json = serde_json::to_value(&request_data).unwrap();
        assert!(json.get("body").is_none());

        let req = ReplayExecuteRequest {
            request: request_data,
            rule_config: crate::replay_db::RuleConfig {
                mode: crate::replay_db::RuleMode::Enabled,
                selected_rules: vec![],
                custom_rules: None,
            },
            request_id: None,
            timeout_ms: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert!(json.get("request_id").is_none());
        assert!(json.get("timeout_ms").is_none());

        let back: ReplayExecuteRequest = serde_json::from_value(json).unwrap();
        assert_eq!(back.request.method, "POST");
        assert_eq!(back.request.url, "http://example.test/");
    }

    #[test]
    fn replay_execute_response_omits_error_when_none() {
        let res = ReplayExecuteResponse {
            traffic_id: "t1".into(),
            status: 200,
            headers: vec![("Content-Type".into(), "text/plain".into())],
            body: Some("ok".into()),
            duration_ms: 42,
            applied_rules: vec![],
            error: None,
        };
        let json = serde_json::to_value(&res).unwrap();
        assert!(json.get("error").is_none());
        assert_eq!(json["traffic_id"], json!("t1"));
        assert_eq!(json["status"], json!(200));
    }
}

#[cfg(test)]
mod replay_executor_helper_tests {
    use super::*;
    use rustls::client::danger::ServerCertVerifier;

    #[test]
    fn parse_headers_supports_newline_delimited_format() {
        let headers = parse_headers("X-One: a\nX-Two: b").expect("headers");
        assert_eq!(headers.len(), 2);
        assert_eq!(headers[0], ("X-One".to_string(), "a".to_string()));
        assert_eq!(headers[1], ("X-Two".to_string(), "b".to_string()));
    }

    #[test]
    fn get_tls_client_config_builds_for_both_modes() {
        // Just ensure both branches construct without panicking.
        let _secure = get_tls_client_config(false);
        let _insecure = get_tls_client_config(true);
    }

    #[test]
    fn no_certificate_verification_reports_supported_schemes() {
        let verifier = NoCertificateVerification;
        let schemes = verifier.supported_verify_schemes();
        assert!(!schemes.is_empty());
    }
}
