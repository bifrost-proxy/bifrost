use bifrost_core::{Protocol, ResolvedRules};

pub(crate) fn apply_response_rules(
    resolved_rules: &ResolvedRules,
    status: u16,
    mut headers: Vec<(String, String)>,
    body: Option<String>,
) -> (u16, Vec<(String, String)>, Option<String>) {
    let mut final_status = status;

    for rule in &resolved_rules.rules {
        match rule.rule.protocol {
            Protocol::Delete => {
                for header_name in parse_delete_value(&rule.resolved_value).res_headers {
                    remove_header(&mut headers, &header_name);
                }
            }
            Protocol::ResHeaders => {
                for (key, value) in parse_header_values(&rule.resolved_value) {
                    set_header(&mut headers, key, value);
                }
            }
            Protocol::StatusCode | Protocol::ReplaceStatus => {
                if let Ok(code) = rule.resolved_value.parse::<u16>() {
                    final_status = code;
                }
            }
            Protocol::ResCookies => {
                if let Some((name, value)) = parse_cookie_value(&rule.resolved_value) {
                    headers.push(("Set-Cookie".to_string(), format!("{}={}", name, value)));
                }
            }
            Protocol::ResCors => {
                apply_res_cors(&mut headers, &rule.resolved_value);
            }
            Protocol::ResType => {
                set_header(
                    &mut headers,
                    "Content-Type".to_string(),
                    expand_content_type_shortcut(&rule.resolved_value).to_string(),
                );
            }
            Protocol::ResCharset => {
                apply_charset(&mut headers, &rule.resolved_value);
            }
            Protocol::Cache => {
                set_header(
                    &mut headers,
                    "Cache-Control".to_string(),
                    cache_control_value(&rule.resolved_value),
                );
            }
            Protocol::Attachment => {
                set_header(
                    &mut headers,
                    "Content-Disposition".to_string(),
                    attachment_value(&rule.resolved_value),
                );
            }
            Protocol::ResponseFor => {
                set_header(
                    &mut headers,
                    "x-bifrost-response-for".to_string(),
                    rule.resolved_value.clone(),
                );
            }
            Protocol::Trailers => {
                let trailer_names: Vec<String> = parse_header_values(&rule.resolved_value)
                    .into_iter()
                    .map(|(name, _)| name)
                    .collect();
                if !trailer_names.is_empty() {
                    remove_header(&mut headers, "Content-Length");
                    set_header(
                        &mut headers,
                        "Trailer".to_string(),
                        trailer_names.join(", "),
                    );
                }
            }
            Protocol::HeaderReplace => {
                for rule in parse_header_replace_value(&rule.resolved_value) {
                    if rule.target == HeaderReplaceTarget::Response {
                        replace_header_value(
                            &mut headers,
                            &rule.header_name,
                            &rule.pattern,
                            &rule.replacement,
                        );
                    }
                }
            }
            _ => {}
        }
    }

    let final_body =
        crate::replay_body_rules::apply_response_body_rules(resolved_rules, &headers, body);

    (final_status, headers, final_body)
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
    use super::apply_response_rules;
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
}
