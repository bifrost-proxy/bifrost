use bifrost_core::{Protocol, ResolvedRules};

pub(crate) fn apply_response_rules(
    resolved_rules: &ResolvedRules,
    status: u16,
    mut headers: Vec<(String, String)>,
    body: Option<String>,
) -> (u16, Vec<(String, String)>, Option<String>) {
    let mut final_status = status;
    let mut delete_res_headers = Vec::new();
    let mut res_headers = Vec::new();
    let mut res_cookies = Vec::new();
    let mut attachment = None;
    let mut res_type = None;
    let mut res_charset = None;
    let mut cache = None;
    let mut header_replace = Vec::new();
    let mut response_for = None;
    let mut res_cors = None;
    let mut trailers = Vec::new();

    for rule in &resolved_rules.rules {
        match rule.rule.protocol {
            Protocol::Delete => {
                delete_res_headers.extend(parse_delete_value(&rule.resolved_value).res_headers);
            }
            Protocol::ResHeaders => {
                res_headers.extend(rule.header_pairs().unwrap_or_default().iter().cloned());
            }
            Protocol::StatusCode | Protocol::ReplaceStatus => {
                if let Ok(code) = rule.resolved_value.parse::<u16>() {
                    final_status = code;
                }
            }
            Protocol::ResCookies => {
                if let Some((name, value)) = parse_cookie_value(&rule.resolved_value) {
                    res_cookies.push((name, value));
                }
            }
            Protocol::ResCors => {
                res_cors = Some(rule.resolved_value.clone());
            }
            Protocol::ResType => {
                res_type = Some(rule.resolved_value.clone());
            }
            Protocol::ResCharset => {
                res_charset = Some(rule.resolved_value.clone());
            }
            Protocol::Cache => {
                cache = Some(rule.resolved_value.clone());
            }
            Protocol::Attachment => {
                attachment = Some(rule.resolved_value.clone());
            }
            Protocol::ResponseFor => {
                response_for = Some(rule.resolved_value.clone());
            }
            Protocol::Trailers => {
                trailers.extend(parse_header_values(&rule.resolved_value));
            }
            Protocol::HeaderReplace => {
                header_replace.extend(parse_header_replace_value(&rule.resolved_value));
            }
            _ => {}
        }
    }

    for header_name in delete_res_headers {
        remove_header(&mut headers, &header_name);
    }
    for (key, value) in res_headers {
        set_header(&mut headers, key, value);
    }
    for (name, value) in res_cookies {
        headers.push(("Set-Cookie".to_string(), format!("{}={}", name, value)));
    }
    if let Some(attachment) = attachment {
        set_header(
            &mut headers,
            "Content-Disposition".to_string(),
            attachment_value(&attachment),
        );
    }
    if let Some(res_type) = res_type {
        set_header(
            &mut headers,
            "Content-Type".to_string(),
            expand_content_type_shortcut(&res_type).to_string(),
        );
    }
    if let Some(res_charset) = res_charset {
        apply_charset(&mut headers, &res_charset);
    }
    if let Some(cache) = cache {
        set_header(
            &mut headers,
            "Cache-Control".to_string(),
            cache_control_value(&cache),
        );
    }
    for rule in header_replace {
        if rule.target == HeaderReplaceTarget::Response {
            replace_header_value(
                &mut headers,
                &rule.header_name,
                &rule.pattern,
                &rule.replacement,
            );
        }
    }
    if let Some(response_for) = response_for {
        set_header(
            &mut headers,
            "x-bifrost-response-for".to_string(),
            response_for,
        );
    }
    if let Some(res_cors) = res_cors {
        apply_res_cors(&mut headers, &res_cors);
    }
    let trailer_names: Vec<String> = trailers.into_iter().map(|(name, _)| name).collect();
    if !trailer_names.is_empty() {
        remove_header(&mut headers, "Content-Length");
        set_header(
            &mut headers,
            "Trailer".to_string(),
            trailer_names.join(", "),
        );
    }

    let final_body =
        crate::replay_body_rules::apply_response_body_rules(resolved_rules, &headers, body);

    (final_status, headers, final_body)
}

pub(crate) fn apply_websocket_response_header_rules(
    resolved_rules: &ResolvedRules,
    mut headers: Vec<(String, String)>,
) -> Vec<(String, String)> {
    let mut delete_res_headers = Vec::new();
    let mut res_headers = Vec::new();
    let mut header_replace = Vec::new();

    for rule in &resolved_rules.rules {
        match rule.rule.protocol {
            Protocol::Delete => {
                delete_res_headers.extend(parse_delete_value(&rule.resolved_value).res_headers);
            }
            Protocol::ResHeaders => {
                res_headers.extend(rule.header_pairs().unwrap_or_default().iter().cloned());
            }
            Protocol::HeaderReplace => {
                header_replace.extend(parse_header_replace_value(&rule.resolved_value));
            }
            _ => {}
        }
    }

    for header_name in delete_res_headers {
        remove_header(&mut headers, &header_name);
    }
    for (key, value) in res_headers {
        set_header(&mut headers, key, value);
    }
    for rule in header_replace {
        if rule.target == HeaderReplaceTarget::Response {
            replace_header_value(
                &mut headers,
                &rule.header_name,
                &rule.pattern,
                &rule.replacement,
            );
        }
    }

    headers
}

fn set_header(headers: &mut Vec<(String, String)>, key: String, value: String) {
    remove_header(headers, &key);
    headers.push((key, value));
}

fn remove_header(headers: &mut Vec<(String, String)>, key: &str) {
    let key_lower = key.to_lowercase();
    headers.retain(|(existing, _)| existing.to_lowercase() != key_lower);
}

fn replace_header_value(
    headers: &mut [(String, String)],
    key: &str,
    pattern: &str,
    replacement: &str,
) {
    for (existing_key, value) in headers {
        if existing_key.eq_ignore_ascii_case(key) {
            *value = value.replace(pattern, replacement);
        }
    }
}

fn parse_header_values(value: &str) -> Vec<(String, String)> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let content = if trimmed.starts_with('(') && trimmed.ends_with(')') && trimmed.len() >= 2 {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    };

    if looks_like_json_header_object(content) {
        return parse_json_header_object(content).unwrap_or_default();
    }

    let delimiter = if content.contains('\n') { '\n' } else { ',' };
    content
        .split(delimiter)
        .filter_map(parse_header_value)
        .collect()
}

fn parse_header_value(value: &str) -> Option<(String, String)> {
    let trimmed = value.trim();
    let (key, val) = trimmed
        .split_once(':')
        .or_else(|| trimmed.split_once('='))?;
    let key = key.trim();
    if key.is_empty() {
        None
    } else {
        Some((key.to_string(), val.trim().to_string()))
    }
}

fn parse_json_header_object(content: &str) -> Option<Vec<(String, String)>> {
    let content = content.trim();
    if !looks_like_json_header_object(content) {
        return None;
    }

    let json_value = serde_json::from_str::<serde_json::Value>(content).ok()?;
    let obj = json_value.as_object()?;
    Some(
        obj.iter()
            .filter_map(|(key, value)| {
                if key.trim().is_empty() {
                    return None;
                }
                json_scalar_to_header_value(value).map(|value| (key.clone(), value))
            })
            .collect::<Vec<_>>(),
    )
}

fn looks_like_json_header_object(content: &str) -> bool {
    let content = content.trim();
    if !(content.starts_with('{') && content.ends_with('}')) {
        return false;
    }
    let inner = content[1..content.len() - 1].trim_start();
    inner.is_empty() || inner.starts_with('"') || inner.contains(':')
}

fn json_scalar_to_header_value(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(value) => Some(value.clone()),
        serde_json::Value::Number(value) => Some(value.to_string()),
        serde_json::Value::Bool(value) => Some(value.to_string()),
        serde_json::Value::Null => Some(String::new()),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => None,
    }
}

fn parse_cookie_value(value: &str) -> Option<(String, String)> {
    parse_header_value(value)
}

#[derive(Default)]
struct ParsedDeleteValue {
    res_headers: Vec<String>,
}

fn parse_delete_value(value: &str) -> ParsedDeleteValue {
    let mut result = ParsedDeleteValue::default();

    for part in value.split('|') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        if let Some(header) = part
            .strip_prefix("resHeaders.")
            .or_else(|| part.strip_prefix("res."))
        {
            result.res_headers.push(header.to_string());
        } else if part.starts_with("reqHeaders.")
            || part.starts_with("req.")
            || part.starts_with("urlParams.")
        {
            continue;
        } else {
            result.res_headers.push(part.to_string());
        }
    }

    result
}

#[derive(Debug, Eq, PartialEq)]
enum HeaderReplaceTarget {
    Request,
    Response,
}

struct HeaderReplaceRule {
    target: HeaderReplaceTarget,
    header_name: String,
    pattern: String,
    replacement: String,
}

fn parse_header_replace_value(value: &str) -> Vec<HeaderReplaceRule> {
    let mut rules = Vec::new();

    for part in value.split('|') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        let (target, rest) = if let Some(rest) = part.strip_prefix("req.") {
            (HeaderReplaceTarget::Request, rest)
        } else if let Some(rest) = part.strip_prefix("res.") {
            (HeaderReplaceTarget::Response, rest)
        } else {
            continue;
        };

        let Some((header_name, pattern_replacement)) = rest.split_once(':') else {
            continue;
        };
        let Some((pattern, replacement)) = pattern_replacement.split_once('=') else {
            continue;
        };

        rules.push(HeaderReplaceRule {
            target,
            header_name: header_name.to_string(),
            pattern: pattern.to_string(),
            replacement: replacement.to_string(),
        });
    }

    rules
}

fn apply_res_cors(headers: &mut Vec<(String, String)>, value: &str) {
    let cors = parse_cors_config(value);
    set_header(
        headers,
        "Access-Control-Allow-Origin".to_string(),
        cors.origin.unwrap_or_else(|| "*".to_string()),
    );
    set_header(
        headers,
        "Access-Control-Allow-Methods".to_string(),
        cors.methods
            .unwrap_or_else(|| "GET, POST, PUT, DELETE, OPTIONS, PATCH".to_string()),
    );
    set_header(
        headers,
        "Access-Control-Allow-Headers".to_string(),
        cors.headers.unwrap_or_else(|| "*".to_string()),
    );
    if cors.credentials.unwrap_or(true) {
        set_header(
            headers,
            "Access-Control-Allow-Credentials".to_string(),
            "true".to_string(),
        );
    }
}

#[derive(Default)]
struct CorsConfig {
    origin: Option<String>,
    methods: Option<String>,
    headers: Option<String>,
    credentials: Option<bool>,
}

fn parse_cors_config(value: &str) -> CorsConfig {
    let content = strip_wrapping_parens(value.trim());
    if content.is_empty() || content == "*" {
        return CorsConfig {
            origin: Some("*".to_string()),
            ..Default::default()
        };
    }

    let mut config = CorsConfig::default();
    for part in content.split(['&', ';']) {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim().to_string();
        match key.as_str() {
            "origin" => config.origin = Some(value),
            "methods" | "method" => config.methods = Some(value),
            "headers" | "header" => config.headers = Some(value),
            "credentials" | "credential" => {
                config.credentials = Some(matches!(value.as_str(), "1" | "true" | "yes"));
            }
            _ => {}
        }
    }

    config
}

fn strip_wrapping_parens(value: &str) -> &str {
    if value.starts_with('(') && value.ends_with(')') && value.len() >= 2 {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

fn expand_content_type_shortcut(input: &str) -> &str {
    match input.to_lowercase().as_str() {
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "javascript" | "mjs" => "application/javascript",
        "json" => "application/json",
        "xml" => "application/xml",
        "txt" | "text" => "text/plain",
        "form" => "application/x-www-form-urlencoded",
        _ => input,
    }
}

fn apply_charset(headers: &mut Vec<(String, String)>, charset: &str) {
    let current_ct = headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.as_str())
        .unwrap_or("text/plain");
    let base_ct = current_ct.split(';').next().unwrap_or(current_ct).trim();
    set_header(
        headers,
        "Content-Type".to_string(),
        format!("{}; charset={}", base_ct, charset),
    );
}

fn cache_control_value(value: &str) -> String {
    if let Ok(seconds) = value.parse::<u64>() {
        if seconds == 0 {
            "no-cache, no-store, must-revalidate".to_string()
        } else {
            format!("max-age={}", seconds)
        }
    } else {
        value.to_string()
    }
}

fn attachment_value(value: &str) -> String {
    if value.trim().is_empty() {
        "attachment; filename=\"download\"".to_string()
    } else {
        format!(
            "attachment; filename=\"{}\"",
            encode_attachment_filename(value)
        )
    }
}

fn encode_attachment_filename(filename: &str) -> String {
    filename
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c.to_string()
            } else {
                format!("%{:02X}", c as u32)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{apply_response_rules, apply_websocket_response_header_rules};
    use bifrost_core::{RequestContext, RuleParser, RulesResolver};

    fn resolve(rule_text: &str) -> bifrost_core::ResolvedRules {
        let parser = RuleParser::new();
        let rules = parser.parse_rules(rule_text).unwrap();
        let resolver = RulesResolver::new(rules);
        let ctx = RequestContext::from_url("https://example.test/api").with_method("GET");
        resolver.resolve(&ctx)
    }

    fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
        headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn replay_response_rules_apply_metadata_and_body_rules() {
        let rules = resolve(
            "https://example.test/api statusCode://201 resHeaders://X-Trace:old headerReplace://res.X-Trace:old=new resCookies://sid=abc resType://json resCharset://utf-8 cache://60 attachment://report.json responseFor://mobile resCors://(origin=https://app.example.test&methods=GET,POST&headers=X-Debug&credentials=false) trailers://X-Trailer:done resMerge://({\"test\":\"qwe\"})",
        );

        let (status, headers, body) = apply_response_rules(
            &rules,
            200,
            vec![
                ("content-type".to_string(), "text/plain".to_string()),
                ("content-length".to_string(), "15".to_string()),
            ],
            Some(r#"{"ok":true}"#.to_string()),
        );
        let value: serde_json::Value = serde_json::from_str(&body.unwrap()).unwrap();

        assert_eq!(status, 201);
        assert_eq!(header(&headers, "X-Trace"), Some("new"));
        assert_eq!(header(&headers, "Set-Cookie"), Some("sid=abc"));
        assert_eq!(
            header(&headers, "Content-Type"),
            Some("application/json; charset=utf-8")
        );
        assert_eq!(header(&headers, "Cache-Control"), Some("max-age=60"));
        assert_eq!(
            header(&headers, "Content-Disposition"),
            Some("attachment; filename=\"report.json\"")
        );
        assert_eq!(header(&headers, "x-bifrost-response-for"), Some("mobile"));
        assert_eq!(
            header(&headers, "Access-Control-Allow-Origin"),
            Some("https://app.example.test")
        );
        assert_eq!(header(&headers, "Trailer"), Some("X-Trailer"));
        assert!(header(&headers, "Content-Length").is_none());
        assert_eq!(value["ok"], true);
        assert_eq!(value["test"], "qwe");
    }

    #[test]
    fn replay_response_rules_delete_res_headers_only() {
        let rules = resolve(
            "https://example.test/api delete://resHeaders.X-Remove|reqHeaders.X-Keep|urlParams.debug resHeaders://X-Keep:yes",
        );

        let (_, headers, _) = apply_response_rules(
            &rules,
            200,
            vec![
                ("X-Remove".to_string(), "1".to_string()),
                ("X-Keep".to_string(), "old".to_string()),
            ],
            None,
        );

        assert!(header(&headers, "X-Remove").is_none());
        assert_eq!(header(&headers, "X-Keep"), Some("yes"));
    }

    #[test]
    fn replay_response_rules_apply_json_object_headers() {
        let rules =
            resolve(r#"https://example.test/api resHeaders://{"X-Env":"ppe","X-Flag":"1"}"#);

        let (_, headers, _) = apply_response_rules(&rules, 200, Vec::new(), None);

        assert_eq!(header(&headers, "X-Env"), Some("ppe"));
        assert_eq!(header(&headers, "X-Flag"), Some("1"));
    }

    #[test]
    fn replay_response_rules_apply_ampersand_separated_headers() {
        let rules = resolve(
            "https://example.test/api resHeaders://(X-Env=ppe&X-Flag=1&X-Query=a%3D1%26b%3D2)",
        );

        let (_, headers, _) = apply_response_rules(&rules, 200, Vec::new(), None);

        assert_eq!(header(&headers, "X-Env"), Some("ppe"));
        assert_eq!(header(&headers, "X-Flag"), Some("1"));
        assert_eq!(header(&headers, "X-Query"), Some("a%3D1%26b%3D2"));
    }

    #[test]
    fn replay_response_rules_ignore_malformed_json_object_headers() {
        let rules = resolve(r#"https://example.test/api resHeaders://{"X-Env":}"#);

        let (_, headers, _) = apply_response_rules(&rules, 200, Vec::new(), None);

        assert!(header(&headers, "{\"X-Env\"").is_none());
        assert!(headers.is_empty());
    }

    #[test]
    fn replay_websocket_response_rules_apply_headers_only() {
        let rules = resolve(
            "https://example.test/api statusCode://204 resHeaders://X-Replay-WS:injected delete://resHeaders.X-Remove headerReplace://res.X-Trace:old=new resBody://ignored",
        );

        let headers = apply_websocket_response_header_rules(
            &rules,
            vec![
                ("X-Remove".to_string(), "1".to_string()),
                ("X-Trace".to_string(), "old".to_string()),
                ("X-Upstream".to_string(), "seen".to_string()),
            ],
        );

        assert!(header(&headers, "X-Remove").is_none());
        assert_eq!(header(&headers, "X-Trace"), Some("new"));
        assert_eq!(header(&headers, "X-Replay-WS"), Some("injected"));
        assert_eq!(header(&headers, "X-Upstream"), Some("seen"));
    }
}
