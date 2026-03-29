use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use bifrost_admin::MatchedRule;

use crate::server::ResolvedRules;

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

static PROCESS_START_TS: LazyLock<u64> = LazyLock::new(|| {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
});

pub fn generate_request_id() -> String {
    let seq = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("REQ-{:x}-{:06}", *PROCESS_START_TS, seq)
}

#[derive(Debug, Clone)]
pub struct RequestContext {
    pub id: u64,
    pub id_string: String,
    pub start_time: Instant,
    pub req_headers: HashMap<String, String>,
    pub req_cookies: HashMap<String, String>,
    pub query_params: HashMap<String, String>,
    pub url: String,
    pub method: String,
    pub host: String,
    pub pathname: String,
    pub search: String,
    pub client_ip: String,
    pub client_app: Option<String>,
    pub client_pid: Option<u32>,
    pub client_path: Option<String>,
    pub port: u16,
}

impl RequestContext {
    pub fn new() -> Self {
        let id = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let id_string = format!("REQ-{:x}-{:06}", *PROCESS_START_TS, id);
        Self {
            id,
            id_string,
            start_time: Instant::now(),
            req_headers: HashMap::new(),
            req_cookies: HashMap::new(),
            query_params: HashMap::new(),
            url: String::new(),
            method: String::new(),
            host: String::new(),
            pathname: String::new(),
            search: String::new(),
            client_ip: String::new(),
            client_app: None,
            client_pid: None,
            client_path: None,
            port: 0,
        }
    }

    pub fn elapsed_ms(&self) -> u128 {
        self.start_time.elapsed().as_millis()
    }

    pub fn id_str(&self) -> &str {
        &self.id_string
    }

    pub fn with_request_info(
        mut self,
        url: String,
        method: String,
        host: String,
        pathname: String,
        search: String,
        client_ip: String,
    ) -> Self {
        self.url = url;
        self.method = method;
        self.host = host;
        self.pathname = pathname;
        self.search = search;
        self.client_ip = client_ip;
        self
    }

    pub fn with_client_process(
        mut self,
        app: Option<String>,
        pid: Option<u32>,
        path: Option<String>,
    ) -> Self {
        self.client_app = app;
        self.client_pid = pid;
        self.client_path = path;
        self
    }

    pub fn with_client_ip(mut self, client_ip: String) -> Self {
        self.client_ip = client_ip;
        self
    }

    pub fn with_headers(mut self, headers: HashMap<String, String>) -> Self {
        self.req_headers = headers;
        self
    }

    pub fn with_cookies(mut self, cookies: HashMap<String, String>) -> Self {
        self.req_cookies = cookies;
        self
    }

    pub fn with_query_params(mut self, params: HashMap<String, String>) -> Self {
        self.query_params = params;
        self
    }

    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }
}

impl Default for RequestContext {
    fn default() -> Self {
        Self::new()
    }
}

pub fn format_rules_summary(rules: &ResolvedRules) -> String {
    let mut parts = Vec::new();

    if rules.host.is_some() {
        parts.push("host");
    }
    if rules.proxy.is_some() {
        parts.push("proxy");
    }
    if !rules.req_headers.is_empty() {
        parts.push("req_headers");
    }
    if !rules.res_headers.is_empty() {
        parts.push("res_headers");
    }
    if rules.req_body.is_some() {
        parts.push("req_body");
    }
    if rules.res_body.is_some() {
        parts.push("res_body");
    }
    if !rules.req_cookies.is_empty() {
        parts.push("req_cookies");
    }
    if !rules.res_cookies.is_empty() {
        parts.push("res_cookies");
    }
    if rules.req_delay.is_some() {
        parts.push("req_delay");
    }
    if rules.res_delay.is_some() {
        parts.push("res_delay");
    }
    if rules.status_code.is_some() {
        parts.push("status_code");
    }
    if rules.method.is_some() {
        parts.push("method");
    }
    if rules.ua.is_some() {
        parts.push("ua");
    }
    if rules.referer.is_some() {
        parts.push("referer");
    }
    if rules.req_cors.is_enabled() {
        parts.push("req_cors");
    }
    if rules.res_cors.is_enabled() {
        parts.push("res_cors");
    }

    if parts.is_empty() {
        "none".to_string()
    } else {
        parts.join(", ")
    }
}

pub fn build_matched_rules(rules: &ResolvedRules) -> Option<Vec<MatchedRule>> {
    let mut matched = Vec::new();

    for rule in &rules.rules {
        matched.push(MatchedRule {
            pattern: rule.pattern.clone(),
            protocol: format!("{:?}", rule.protocol),
            value: rule.value.clone(),
            rule_name: rule.rule_name.clone(),
            raw: rule.raw.clone(),
            line: rule.line,
        });
    }

    if matched.is_empty() {
        None
    } else {
        Some(matched)
    }
}

pub fn format_rules_detail(rules: &ResolvedRules) -> String {
    let mut lines = Vec::new();

    if let Some(ref host) = rules.host {
        lines.push(format!("  host -> {}", host));
    }
    if let Some(ref proxy) = rules.proxy {
        lines.push(format!("  proxy -> {}", proxy));
    }
    if !rules.req_headers.is_empty() {
        for (name, value) in &rules.req_headers {
            lines.push(format!("  req_header: {} = {}", name, value));
        }
    }
    if !rules.res_headers.is_empty() {
        for (name, value) in &rules.res_headers {
            lines.push(format!("  res_header: {} = {}", name, value));
        }
    }
    if rules.req_body.is_some() {
        lines.push("  req_body: <modified>".to_string());
    }
    if rules.res_body.is_some() {
        lines.push("  res_body: <modified>".to_string());
    }
    if !rules.req_cookies.is_empty() {
        for (name, value) in &rules.req_cookies {
            lines.push(format!("  req_cookie: {} = {}", name, value));
        }
    }
    if !rules.res_cookies.is_empty() {
        for (name, value) in &rules.res_cookies {
            lines.push(format!("  res_cookie: {} = {}", name, value));
        }
    }
    if let Some(delay) = rules.req_delay {
        lines.push(format!("  req_delay: {}ms", delay));
    }
    if let Some(delay) = rules.res_delay {
        lines.push(format!("  res_delay: {}ms", delay));
    }
    if let Some(status) = rules.status_code {
        lines.push(format!("  status_code -> {}", status));
    }
    if let Some(ref method) = rules.method {
        lines.push(format!("  method -> {}", method));
    }
    if let Some(ref ua) = rules.ua {
        lines.push(format!("  user-agent -> {}", ua));
    }
    if let Some(ref referer) = rules.referer {
        lines.push(format!("  referer -> {}", referer));
    }
    if rules.req_cors.is_enabled() {
        lines.push("  req_cors: enabled".to_string());
    }
    if rules.res_cors.is_enabled() {
        lines.push("  res_cors: enabled".to_string());
    }

    if !rules.rules.is_empty() {
        lines.push(format!("  matched {} rule(s):", rules.rules.len()));
        for rule in &rules.rules {
            let source = rule
                .rule_name
                .as_ref()
                .map(|f| {
                    if let Some(line) = rule.line {
                        format!("{}:{}", f, line)
                    } else {
                        f.clone()
                    }
                })
                .unwrap_or_else(|| "<cli>".to_string());
            let raw_display = rule
                .raw
                .as_ref()
                .map(|r| format!(" (raw: {})", r))
                .unwrap_or_default();
            lines.push(format!(
                "    [{}] {} {:?}://{}{}",
                source, rule.pattern, rule.protocol, rule.value, raw_display
            ));
        }
    }

    if lines.is_empty() {
        "  (no rules applied)".to_string()
    } else {
        lines.join("\n")
    }
}

pub fn truncate_body(body: &[u8], max_len: usize) -> String {
    if body.is_empty() {
        return "<empty>".to_string();
    }

    let display_len = body.len().min(max_len);
    match std::str::from_utf8(&body[..display_len]) {
        Ok(s) => {
            if body.len() > max_len {
                format!("{}... ({} bytes total)", s, body.len())
            } else {
                s.to_string()
            }
        }
        Err(_) => format!("<binary {} bytes>", body.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    #[test]
    fn test_request_context_new() {
        let ctx1 = RequestContext::new();
        let ctx2 = RequestContext::new();
        assert!(ctx2.id > ctx1.id);
    }

    #[test]
    fn test_request_context_id_str() {
        let ctx = RequestContext::new();
        assert!(ctx.id_str().starts_with("REQ-"));
    }

    #[test]
    fn test_format_rules_summary_empty() {
        let rules = ResolvedRules::default();
        assert_eq!(format_rules_summary(&rules), "none");
    }

    #[test]
    fn test_format_rules_summary_with_rules() {
        use crate::server::CorsConfig;
        let rules = ResolvedRules {
            host: Some("example.com".to_string()),
            res_cors: CorsConfig::enable_all(),
            ..Default::default()
        };
        let summary = format_rules_summary(&rules);
        assert!(summary.contains("host"));
        assert!(summary.contains("res_cors"));
    }

    #[test]
    fn test_format_rules_detail_empty() {
        let rules = ResolvedRules::default();
        let detail = format_rules_detail(&rules);
        assert!(detail.contains("no rules applied"));
    }

    #[test]
    fn test_format_rules_detail_with_rules() {
        let rules = ResolvedRules {
            host: Some("example.com:8080".to_string()),
            req_headers: vec![("X-Custom".to_string(), "value".to_string())],
            status_code: Some(200),
            ..Default::default()
        };
        let detail = format_rules_detail(&rules);
        assert!(detail.contains("host -> example.com:8080"));
        assert!(detail.contains("req_header: X-Custom = value"));
        assert!(detail.contains("status_code -> 200"));
    }

    #[test]
    fn test_truncate_body_empty() {
        assert_eq!(truncate_body(&[], 100), "<empty>");
    }

    #[test]
    fn test_truncate_body_short() {
        let body = b"hello world";
        assert_eq!(truncate_body(body, 100), "hello world");
    }

    #[test]
    fn test_truncate_body_long() {
        let body = b"hello world this is a long string";
        let result = truncate_body(body, 10);
        assert!(result.contains("hello worl"));
        assert!(result.contains("33 bytes total"));
    }

    #[test]
    fn test_truncate_body_binary() {
        let body = vec![0xff, 0xfe, 0x00, 0x01];
        let result = truncate_body(&body, 100);
        assert!(result.contains("binary"));
        assert!(result.contains("4 bytes"));
    }

    #[test]
    fn test_format_rules_detail_with_cookies() {
        use crate::server::ResCookieValue;
        let rules = ResolvedRules {
            req_cookies: vec![("session".to_string(), "abc123".to_string())],
            res_cookies: vec![(
                "token".to_string(),
                ResCookieValue::simple("xyz789".to_string()),
            )],
            ..Default::default()
        };
        let detail = format_rules_detail(&rules);
        assert!(detail.contains("req_cookie: session = abc123"));
        assert!(detail.contains("res_cookie: token = xyz789"));
    }

    #[test]
    fn test_format_rules_detail_with_body() {
        let rules = ResolvedRules {
            req_body: Some(Bytes::from("request body")),
            res_body: Some(Bytes::from("response body")),
            ..Default::default()
        };
        let detail = format_rules_detail(&rules);
        assert!(detail.contains("req_body: <modified>"));
        assert!(detail.contains("res_body: <modified>"));
    }

    #[test]
    fn test_format_rules_detail_with_delays() {
        let rules = ResolvedRules {
            req_delay: Some(100),
            res_delay: Some(200),
            ..Default::default()
        };
        let detail = format_rules_detail(&rules);
        assert!(detail.contains("req_delay: 100ms"));
        assert!(detail.contains("res_delay: 200ms"));
    }
}
