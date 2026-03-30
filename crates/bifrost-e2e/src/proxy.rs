use bifrost_admin::{
    start_async_traffic_processor, start_connection_cleanup_task, start_frame_cleanup_task,
    start_ws_payload_cleanup_task, AdminState, AsyncTrafficWriter, BodyStore, ConnectionRegistry,
    RuntimeConfig, WsPayloadStore,
};
use bifrost_core::{
    normalize_rule_content, parse_rules, Protocol, RequestContext, Rule, RuleParser,
    RulesResolver as CoreRulesResolver,
};
use bifrost_proxy::{
    ProxyConfig, ProxyServer, ResolvedRules as ProxyResolvedRules, RuleValue,
    RulesResolver as ProxyRulesResolverTrait, TlsConfig,
};
use bifrost_tls::{generate_root_ca, init_crypto_provider, DynamicCertGenerator, SniResolver};
use std::collections::HashMap;
use std::time::Duration;

fn extract_inline_content(value: &str) -> &str {
    let trimmed = value.trim();
    if trimmed.starts_with('(') && trimmed.ends_with(')') && trimmed.len() >= 2 {
        &trimmed[1..trimmed.len() - 1]
    } else {
        value
    }
}

fn parse_redirect_target(value: &str) -> (Option<u16>, String) {
    for status in [301u16, 302, 307, 308] {
        let suffix = format!("?{status}");
        if let Some(location) = value.strip_suffix(&suffix) {
            return (Some(status), location.to_string());
        }
    }

    if let Some((status_part, location)) = value.split_once(':') {
        if status_part.len() == 3 && status_part.chars().all(|c| c.is_ascii_digit()) {
            if let Ok(status) = status_part.parse::<u16>() {
                if (300..=399).contains(&status) && !location.is_empty() {
                    return (Some(status), location.to_string());
                }
            }
        }
    }

    (None, value.to_string())
}

fn normalize_rule_line(rule: &str) -> String {
    let mut normalized = rule.to_string();

    if let Some((prefix, location)) = normalized.split_once(" locationHref://") {
        let escaped_location = location.replace('"', "\\\"");
        return format!(
            r#"{prefix} tpl://(<!doctype html><html><head><meta charset="utf-8"></head><body><script>location.href = "{escaped_location}";</script></body></html>) resHeaders://Content-Type=text/html; charset=utf-8"#
        );
    }

    if normalized.contains(" disable://cache") {
        normalized = normalized.replace(
            " disable://cache",
            " cache://no-cache, no-store, must-revalidate",
        );
    }

    if normalized.contains(" deleteResHeaders://") {
        normalized = normalized.replace(" deleteResHeaders://", " delete://resHeaders.");
    }

    if normalized.contains(" deleteReqHeaders://") {
        normalized = normalized.replace(" deleteReqHeaders://", " delete://reqHeaders.");
    }

    if normalized.contains("${host|${method}|${now}}") {
        normalized = normalized.replace("${host|${method}|${now}}", "${host}|${method}|${now}");
    }

    normalize_rule_content(&normalized)
}

fn expand_rule_lines(rule: &str) -> Vec<String> {
    let normalized = normalize_rule_line(rule);
    let tokens: Vec<&str> = normalized.split_whitespace().collect();
    if tokens.len() < 3 {
        return vec![normalized];
    }

    let pattern = tokens[0];
    let mut host_tokens = Vec::new();
    let mut other_tokens = Vec::new();
    let mut filter_tokens = Vec::new();

    for token in tokens.iter().skip(1) {
        if token.starts_with("includeFilter://")
            || token.starts_with("excludeFilter://")
            || token.starts_with("lineProps://")
        {
            filter_tokens.push((*token).to_string());
        } else if token.starts_with("host://")
            || token.starts_with("xhost://")
            || token.starts_with("http://")
            || token.starts_with("https://")
            || token.starts_with("ws://")
            || token.starts_with("wss://")
        {
            host_tokens.push((*token).to_string());
        } else {
            other_tokens.push((*token).to_string());
        }
    }

    if filter_tokens.is_empty() || host_tokens.is_empty() {
        return vec![normalized];
    }

    if !other_tokens.is_empty() {
        let mut expanded = vec![format!("{pattern} {}", host_tokens.join(" "))];
        let mut filtered = vec![pattern.to_string()];
        let has_status_filter = filter_tokens.iter().any(|token| {
            token.starts_with("includeFilter://s:") || token.starts_with("excludeFilter://s:")
        });
        let has_status_override = other_tokens.iter().any(|token| {
            token.starts_with("replaceStatus://") || token.starts_with("statusCode://")
        });
        filtered.extend(other_tokens);
        if !(has_status_filter && has_status_override) {
            filtered.extend(filter_tokens);
        }
        expanded.push(filtered.join(" "));
        return expanded;
    }

    if filter_tokens.iter().any(|token| {
        token.starts_with("includeFilter://s:") || token.starts_with("excludeFilter://s:")
    }) {
        return vec![format!("{pattern} {}", host_tokens.join(" "))];
    }

    vec![normalized]
}

fn collapse_legacy_mock_overrides(rules: Vec<String>) -> Vec<String> {
    let mut collapsed = Vec::new();

    for rule in rules {
        let tokens: Vec<&str> = rule.split_whitespace().collect();
        if tokens.len() == 2
            && (tokens[1].starts_with("file://")
                || tokens[1].starts_with("rawfile://")
                || tokens[1].starts_with("tpl://"))
        {
            collapsed.retain(|existing: &String| {
                let existing_tokens: Vec<&str> = existing.split_whitespace().collect();
                !(existing_tokens.len() == 2
                    && existing_tokens[0] == tokens[0]
                    && existing_tokens[1]
                        .split("://")
                        .next()
                        .is_some_and(|proto| tokens[1].starts_with(&format!("{proto}://"))))
            });
        }

        collapsed.push(rule);
    }

    collapsed
}
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::oneshot;

struct RulesResolverAdapter {
    inner: CoreRulesResolver,
}

impl ProxyRulesResolverTrait for RulesResolverAdapter {
    fn resolve_with_context(
        &self,
        url: &str,
        method: &str,
        req_headers: &std::collections::HashMap<String, String>,
        req_cookies: &std::collections::HashMap<String, String>,
    ) -> ProxyResolvedRules {
        let mut ctx = RequestContext::from_url(url);
        ctx.method = method.to_string();
        ctx.client_ip = "127.0.0.1".to_string();
        ctx.req_headers = req_headers
            .iter()
            .map(|(key, value)| (key.to_lowercase(), value.clone()))
            .collect();
        ctx.req_cookies = req_cookies.clone();

        let core_result = self.inner.resolve(&ctx);
        let mut result = ProxyResolvedRules::default();

        tracing::debug!("Resolving rules for URL: {}", url);
        tracing::debug!("Found {} rules", core_result.rules.len());

        for resolved_rule in &core_result.rules {
            let protocol = resolved_rule.rule.protocol;
            let value = &resolved_rule.resolved_value;
            let pattern = &resolved_rule.rule.pattern;
            tracing::debug!("Processing rule: {:?} = {}", protocol, value);

            result.rules.push(RuleValue {
                pattern: pattern.clone(),
                protocol,
                value: value.clone(),
                options: std::collections::HashMap::new(),
                rule_name: resolved_rule.rule.file.clone(),
                raw: Some(resolved_rule.rule.raw.clone()),
                line: resolved_rule.rule.line,
            });

            match protocol {
                Protocol::Host => {
                    if !result.ignored.host {
                        result.host = Some(value.to_string());
                    }
                }
                Protocol::ReqHeaders => {
                    if let Some(headers) = parse_header_value(value) {
                        for (k, v) in headers {
                            result.req_headers.push((k, v));
                        }
                    }
                }
                Protocol::ResHeaders => {
                    tracing::debug!("Parsing ResHeaders value: {}", value);
                    if let Some(headers) = parse_header_value(value) {
                        for (k, v) in &headers {
                            tracing::debug!("Adding res header: {} = {}", k, v);
                        }
                        for (k, v) in headers {
                            if v.is_empty() {
                                result.delete_res_headers.push(k);
                            } else {
                                result.res_headers.push((k, v));
                            }
                        }
                    } else {
                        tracing::warn!("Failed to parse ResHeaders value: {}", value);
                    }
                }
                Protocol::ReqCookies => {
                    if let Some(cookies) = parse_header_value(value) {
                        for (k, v) in cookies {
                            result.req_cookies.push((k, v));
                        }
                    }
                }
                Protocol::ResCookies => {
                    let parsed_cookies = parse_res_cookies_value(value);
                    result.res_cookies.extend(parsed_cookies);
                }
                Protocol::StatusCode => {
                    if let Ok(code) = value.parse::<u16>() {
                        result.status_code = Some(code);
                    }
                }
                Protocol::ReplaceStatus => {
                    if let Ok(code) = value.parse::<u16>() {
                        result.replace_status = Some(code);
                    }
                }
                Protocol::Method => {
                    result.method = Some(value.to_string());
                }
                Protocol::Ua => {
                    result.ua = Some(value.to_string());
                }
                Protocol::Referer => {
                    result.referer = Some(value.to_string());
                }
                Protocol::ReqCors => {
                    result.req_cors = parse_cors_config(value);
                }
                Protocol::ResCors => {
                    result.res_cors = parse_cors_config(value);
                }
                Protocol::Proxy => {
                    result.proxy = Some(value.to_string());
                }
                Protocol::ReqPrepend => {
                    let content = extract_inline_content(value);
                    result.req_prepend = Some(bytes::Bytes::from(content.to_string()));
                }
                Protocol::ReqAppend => {
                    let content = extract_inline_content(value);
                    result.req_append = Some(bytes::Bytes::from(content.to_string()));
                }
                Protocol::ResPrepend => {
                    let content = extract_inline_content(value);
                    result.res_prepend = Some(bytes::Bytes::from(content.to_string()));
                }
                Protocol::ResAppend => {
                    let content = extract_inline_content(value);
                    result.res_append = Some(bytes::Bytes::from(content.to_string()));
                }
                Protocol::ReqBody => {
                    let content = extract_inline_content(value);
                    result.req_body = Some(bytes::Bytes::from(content.to_string()));
                }
                Protocol::ResBody => {
                    let content = extract_inline_content(value);
                    result.res_body = Some(bytes::Bytes::from(content.to_string()));
                }
                Protocol::ReqReplace => {
                    let parsed = parse_replace_value(value);
                    result.req_replace.extend(parsed.string_rules);
                    result.req_replace_regex.extend(parsed.regex_rules);
                }
                Protocol::ResReplace => {
                    let parsed = parse_replace_value(value);
                    result.res_replace.extend(parsed.string_rules);
                    result.res_replace_regex.extend(parsed.regex_rules);
                }
                Protocol::Params => {
                    result.req_merge = parse_merge_value(value);
                }
                Protocol::ResMerge => {
                    result.res_merge = parse_merge_value(value);
                }
                Protocol::UrlParams => {
                    if let Some(params) = parse_header_value(value) {
                        for (k, v) in params {
                            if v.is_empty() {
                                result.delete_url_params.push(k);
                            } else {
                                result.url_params.push((k, v));
                            }
                        }
                    }
                }
                Protocol::UrlReplace => {
                    let parsed = parse_replace_value(value);
                    result.url_replace.extend(parsed.string_rules);
                    result.url_replace_regex.extend(parsed.regex_rules);
                }
                Protocol::ReqType => {
                    result.req_type = Some(value.to_string());
                }
                Protocol::ReqCharset => {
                    result.req_charset = Some(value.to_string());
                }
                Protocol::ResType => {
                    result.res_type = Some(value.to_string());
                }
                Protocol::ResCharset => {
                    result.res_charset = Some(value.to_string());
                }
                Protocol::Cache => {
                    result.cache = Some(value.to_string());
                }
                Protocol::Attachment => {
                    result.attachment = Some(value.to_string());
                }
                Protocol::HtmlAppend => {
                    result.html_append = Some(value.to_string());
                }
                Protocol::HtmlPrepend => {
                    result.html_prepend = Some(value.to_string());
                }
                Protocol::HtmlBody => {
                    result.html_body = Some(value.to_string());
                }
                Protocol::JsAppend => {
                    result.js_append = Some(value.to_string());
                }
                Protocol::JsPrepend => {
                    result.js_prepend = Some(value.to_string());
                }
                Protocol::JsBody => {
                    result.js_body = Some(value.to_string());
                }
                Protocol::CssAppend => {
                    result.css_append = Some(value.to_string());
                }
                Protocol::CssPrepend => {
                    result.css_prepend = Some(value.to_string());
                }
                Protocol::CssBody => {
                    result.css_body = Some(value.to_string());
                }
                Protocol::ReqSpeed => {
                    if let Ok(speed) = value.parse::<u64>() {
                        result.req_speed = Some(speed);
                    }
                }
                Protocol::ResSpeed => {
                    if let Ok(speed) = value.parse::<u64>() {
                        result.res_speed = Some(speed);
                    }
                }
                Protocol::Redirect => {
                    let (status, location) = parse_redirect_target(value);
                    result.redirect = Some(location);
                    result.redirect_status = status;
                }
                Protocol::File => {
                    result.mock_file = Some(value.to_string());
                }
                Protocol::Tpl => {
                    result.mock_template = Some(value.to_string());
                }
                Protocol::RawFile => {
                    result.mock_rawfile = Some(value.to_string());
                }
                Protocol::Dns => {
                    result.dns_servers.push(value.to_string());
                }
                Protocol::XHost => {
                    if !result.ignored.host {
                        result.host = Some(value.to_string());
                    }
                }
                Protocol::Http => {
                    if !result.ignored.host {
                        result.host = Some(value.to_string());
                        result.host_protocol = Some(Protocol::Http);
                    }
                }
                Protocol::Https => {
                    if !result.ignored.host {
                        result.host = Some(value.to_string());
                        result.host_protocol = Some(Protocol::Https);
                    }
                }
                Protocol::Ws => {
                    if !result.ignored.host {
                        result.host = Some(value.to_string());
                        result.host_protocol = Some(Protocol::Ws);
                    }
                }
                Protocol::Wss => {
                    if !result.ignored.host {
                        result.host = Some(value.to_string());
                        result.host_protocol = Some(Protocol::Wss);
                    }
                }
                Protocol::TlsIntercept => {
                    result.tls_intercept = Some(true);
                }
                Protocol::TlsPassthrough => {
                    result.tls_intercept = Some(false);
                }
                Protocol::Passthrough => {
                    result.ignored.host = true;
                }
                Protocol::Auth => {
                    result.auth = Some(value.to_string());
                }
                Protocol::Delete => {
                    let parsed = parse_delete_value(value);
                    result.delete_req_headers.extend(parsed.req_headers);
                    result.delete_res_headers.extend(parsed.res_headers);
                    result.delete_url_params.extend(parsed.url_params);
                }
                Protocol::HeaderReplace => {
                    if let Some(rules) = parse_header_replace_value(value) {
                        result.header_replace.extend(rules);
                    }
                }
                _ => {}
            }
        }

        result
    }
}

struct ParsedDeleteValue {
    req_headers: Vec<String>,
    res_headers: Vec<String>,
    url_params: Vec<String>,
}

fn parse_delete_value(value: &str) -> ParsedDeleteValue {
    let mut result = ParsedDeleteValue {
        req_headers: Vec::new(),
        res_headers: Vec::new(),
        url_params: Vec::new(),
    };

    for part in value.split('|') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        if let Some(header) = part.strip_prefix("reqHeaders.") {
            result.req_headers.push(header.to_string());
        } else if let Some(header) = part.strip_prefix("resHeaders.") {
            result.res_headers.push(header.to_string());
        } else if let Some(param) = part.strip_prefix("urlParams.") {
            result.url_params.push(param.to_string());
        } else if let Some(header) = part.strip_prefix("req.") {
            result.req_headers.push(header.to_string());
        } else if let Some(header) = part.strip_prefix("res.") {
            result.res_headers.push(header.to_string());
        } else {
            result.req_headers.push(part.to_string());
            result.res_headers.push(part.to_string());
        }
    }

    result
}

fn parse_header_replace_value(value: &str) -> Option<Vec<bifrost_proxy::HeaderReplaceRule>> {
    use bifrost_proxy::{HeaderReplaceRule, HeaderReplaceTarget};

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

        let colon_pos = rest.find(':')?;
        let header_name = rest[..colon_pos].to_string();
        let pattern_replacement = &rest[colon_pos + 1..];

        let eq_pos = pattern_replacement.find('=')?;
        let pattern = pattern_replacement[..eq_pos].to_string();
        let replacement = pattern_replacement[eq_pos + 1..].to_string();

        rules.push(HeaderReplaceRule {
            target,
            header_name,
            pattern,
            replacement,
        });
    }

    if rules.is_empty() {
        None
    } else {
        Some(rules)
    }
}

fn parse_header_value(value: &str) -> Option<Vec<(String, String)>> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }

    let content = if trimmed.starts_with('(') && trimmed.ends_with(')') {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    };

    let mut headers = Vec::new();

    let delimiter = if content.contains('\n') { '\n' } else { ',' };
    for part in content.split(delimiter) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        let split_pos = match (part.find('='), part.find(':')) {
            (Some(eq), Some(colon)) => Some(eq.min(colon)),
            (Some(eq), None) => Some(eq),
            (None, Some(colon)) => Some(colon),
            (None, None) => None,
        };

        if let Some(pos) = split_pos {
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

fn url_decode(s: &str) -> String {
    urlencoding::decode(s)
        .unwrap_or(std::borrow::Cow::Borrowed(s))
        .into_owned()
}

struct ParsedReplaceRules {
    string_rules: Vec<(String, String)>,
    regex_rules: Vec<bifrost_proxy::RegexReplace>,
}

fn parse_regex_pattern(s: &str) -> Option<(regex::Regex, bool)> {
    let s = s.trim();
    if !s.starts_with('/') {
        return None;
    }

    let global = s.ends_with("/g") || s.ends_with("/gi") || s.ends_with("/ig");
    let case_insensitive = s.ends_with("/i") || s.ends_with("/gi") || s.ends_with("/ig");

    let end_pos = if global && case_insensitive {
        s.len() - 3
    } else if global || case_insensitive {
        s.len() - 2
    } else if s.len() > 1 && s.ends_with('/') {
        s.len() - 1
    } else {
        return None;
    };

    let pattern_str = &s[1..end_pos];
    if pattern_str.is_empty() {
        return None;
    }

    let regex_result = if case_insensitive {
        regex::RegexBuilder::new(pattern_str)
            .case_insensitive(true)
            .build()
    } else {
        regex::Regex::new(pattern_str)
    };

    match regex_result {
        Ok(re) => Some((re, global)),
        Err(_) => None,
    }
}

fn parse_cors_config(value: &str) -> bifrost_proxy::CorsConfig {
    let value = value.trim();
    if value.is_empty() || value == "*" || value.eq_ignore_ascii_case("enable") {
        return bifrost_proxy::CorsConfig::enable_all();
    }

    if !value.contains('\n') && value.contains("://") && !value.starts_with('{') {
        return bifrost_proxy::CorsConfig {
            enabled: true,
            origin: Some(value.to_string()),
            ..Default::default()
        };
    }

    if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(value) {
        let mut cors = bifrost_proxy::CorsConfig {
            enabled: true,
            ..Default::default()
        };

        if let Some(origin) = json_value.get("origin").and_then(|v| v.as_str()) {
            cors.origin = Some(origin.to_string());
        }
        if let Some(methods) = json_value.get("methods").and_then(|v| v.as_str()) {
            cors.methods = Some(methods.to_string());
        }
        if let Some(headers) = json_value.get("headers").and_then(|v| v.as_str()) {
            cors.headers = Some(headers.to_string());
        }
        if let Some(expose) = json_value
            .get("expose")
            .or_else(|| json_value.get("exposeHeaders"))
            .and_then(|v| v.as_str())
        {
            cors.expose_headers = Some(expose.to_string());
        }
        if let Some(creds) = json_value.get("credentials").and_then(|v| v.as_bool()) {
            cors.credentials = Some(creds);
        }
        if let Some(max_age) = json_value
            .get("maxAge")
            .or_else(|| json_value.get("maxage"))
        {
            if let Some(age) = max_age.as_u64() {
                cors.max_age = Some(age);
            } else if let Some(age_str) = max_age.as_str() {
                if let Ok(age) = age_str.parse::<u64>() {
                    cors.max_age = Some(age);
                }
            }
        }

        return cors;
    }

    if let Some(entries) = parse_header_value(value) {
        let mut cors = bifrost_proxy::CorsConfig {
            enabled: true,
            ..Default::default()
        };

        for (key, raw_value) in entries {
            match key.to_ascii_lowercase().as_str() {
                "origin" => cors.origin = Some(raw_value),
                "method" | "methods" => cors.methods = Some(raw_value),
                "headers" => cors.headers = Some(raw_value),
                "expose" | "exposeheaders" => cors.expose_headers = Some(raw_value),
                "credentials" => {
                    if let Ok(enabled) = raw_value.parse::<bool>() {
                        cors.credentials = Some(enabled);
                    }
                }
                "maxage" | "max_age" => {
                    if let Ok(age) = raw_value.parse::<u64>() {
                        cors.max_age = Some(age);
                    }
                }
                _ => {}
            }
        }

        return cors;
    }

    bifrost_proxy::CorsConfig::enable_all()
}

fn parse_merge_value(value: &str) -> Option<serde_json::Value> {
    if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(value) {
        return Some(json_value);
    }

    let trimmed = value.trim();
    let content = if trimmed.starts_with('(') && trimmed.ends_with(')') && trimmed.len() >= 2 {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    };

    let mut map = serde_json::Map::new();
    for part in content.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        let Some((key, raw_value)) = part.split_once(':') else {
            continue;
        };

        let key = key.trim();
        let raw_value = raw_value.trim();
        if key.is_empty() {
            continue;
        }

        let parsed_value = serde_json::from_str::<serde_json::Value>(raw_value)
            .or_else(|_| serde_json::from_str::<serde_json::Value>(&format!("\"{}\"", raw_value)))
            .unwrap_or_else(|_| serde_json::Value::String(raw_value.to_string()));
        map.insert(key.to_string(), parsed_value);
    }

    if map.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(map))
    }
}

fn parse_res_cookies_value(value: &str) -> Vec<(String, bifrost_proxy::ResCookieValue)> {
    let value = value.trim();
    if value.is_empty() {
        return Vec::new();
    }

    if let Ok(json) = serde_json::from_str::<serde_json::Value>(value) {
        if let Some(obj) = json.as_object() {
            return obj
                .iter()
                .filter_map(|(name, val)| {
                    let cookie_value = if val.is_string() {
                        bifrost_proxy::ResCookieValue::simple(
                            val.as_str().unwrap_or("").to_string(),
                        )
                    } else if let Some(obj) = val.as_object() {
                        bifrost_proxy::ResCookieValue {
                            value: obj
                                .get("value")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            max_age: obj
                                .get("maxAge")
                                .or_else(|| obj.get("Max-Age"))
                                .or_else(|| obj.get("max_age"))
                                .and_then(|v| v.as_i64()),
                            path: obj.get("path").and_then(|v| v.as_str()).map(String::from),
                            domain: obj.get("domain").and_then(|v| v.as_str()).map(String::from),
                            secure: obj.get("secure").and_then(|v| v.as_bool()).unwrap_or(false),
                            http_only: obj
                                .get("httpOnly")
                                .or_else(|| obj.get("http_only"))
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false),
                            same_site: obj
                                .get("sameSite")
                                .or_else(|| obj.get("same_site"))
                                .and_then(|v| v.as_str())
                                .map(String::from),
                        }
                    } else {
                        return None;
                    };
                    Some((name.clone(), cookie_value))
                })
                .collect();
        }
    }

    if let Some(headers) = parse_header_value(value) {
        return headers
            .into_iter()
            .map(|(k, v)| (k, bifrost_proxy::ResCookieValue::simple(v)))
            .collect();
    }

    Vec::new()
}

fn parse_replace_value(value: &str) -> ParsedReplaceRules {
    let mut string_rules = Vec::new();
    let mut regex_rules = Vec::new();

    for pair in value.split('&') {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }

        if let Some((from, to)) = pair.split_once('=') {
            let from = url_decode(from);
            let to = url_decode(to);

            if let Some((regex, global)) = parse_regex_pattern(&from) {
                regex_rules.push(bifrost_proxy::RegexReplace {
                    pattern: regex,
                    replacement: to,
                    global,
                });
            } else {
                string_rules.push((from, to));
            }
        } else {
            let from = url_decode(pair);
            if let Some((regex, global)) = parse_regex_pattern(&from) {
                regex_rules.push(bifrost_proxy::RegexReplace {
                    pattern: regex,
                    replacement: String::new(),
                    global,
                });
            } else {
                string_rules.push((from, String::new()));
            }
        }
    }

    ParsedReplaceRules {
        string_rules,
        regex_rules,
    }
}

pub struct ProxyInstance {
    addr: SocketAddr,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl ProxyInstance {
    pub async fn start(
        port: u16,
        rules: Vec<&str>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Self::start_with_values(port, rules, HashMap::new()).await
    }

    pub async fn start_with_values(
        port: u16,
        rules: Vec<&str>,
        values: HashMap<String, String>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        init_crypto_provider();
        let normalized_rules = collapse_legacy_mock_overrides(
            rules
                .iter()
                .flat_map(|rule| expand_rule_lines(rule))
                .collect(),
        );
        let parsed_rules: Vec<Rule> = normalized_rules
            .iter()
            .filter_map(|r| parse_rules(r).ok())
            .flatten()
            .collect();

        let resolver = Arc::new(RulesResolverAdapter {
            inner: CoreRulesResolver::new(parsed_rules)
                .with_values(values)
                .disable_cache(),
        });
        let addr: SocketAddr = format!("127.0.0.1:{}", port).parse()?;

        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let config = ProxyConfig {
            port,
            host: "127.0.0.1".to_string(),
            enable_tls_interception: false,
            intercept_exclude: Vec::new(),
            intercept_include: Vec::new(),
            app_intercept_exclude: Vec::new(),
            app_intercept_include: Vec::new(),
            timeout_secs: 30,
            http1_max_header_size: 64 * 1024,
            http2_max_header_list_size: 256 * 1024,
            websocket_handshake_max_header_size: 64 * 1024,
            socks5_port: None,
            socks5_auth_required: false,
            socks5_username: None,
            socks5_password: None,
            verbose_logging: true,
            access_mode: bifrost_proxy::AccessMode::AllowAll,
            client_whitelist: Vec::new(),
            allow_lan: true,
            unsafe_ssl: false,
            max_body_buffer_size: 10 * 1024 * 1024,
            max_body_probe_size: 64 * 1024,
            binary_traffic_performance_mode: true,
            enable_socks: true,
        };

        let server = ProxyServer::new(config).with_rules(resolver);
        let listener = server.bind(addr).await?;

        tokio::spawn(async move {
            tokio::select! {
                result = server.run_with_listener(listener) => {
                    if let Err(e) = result {
                        tracing::error!("Proxy server error: {}", e);
                    }
                }
                _ = shutdown_rx => {
                    tracing::info!("Proxy server shutting down");
                }
            }
        });

        Ok(Self {
            addr,
            shutdown_tx: Some(shutdown_tx),
        })
    }

    pub async fn start_with_rules_text(
        port: u16,
        rules_text: &str,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        init_crypto_provider();
        let parser = RuleParser::new();
        let (rules, inline_values) = parser
            .parse_rules_with_inline_values(rules_text)
            .map_err(|e| format!("Failed to parse rules: {}", e))?;

        let resolver = Arc::new(RulesResolverAdapter {
            inner: CoreRulesResolver::new(rules)
                .with_values(inline_values)
                .disable_cache(),
        });
        let addr: SocketAddr = format!("127.0.0.1:{}", port).parse()?;

        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let config = ProxyConfig {
            port,
            host: "127.0.0.1".to_string(),
            enable_tls_interception: false,
            intercept_exclude: Vec::new(),
            intercept_include: Vec::new(),
            app_intercept_exclude: Vec::new(),
            app_intercept_include: Vec::new(),
            timeout_secs: 30,
            http1_max_header_size: 64 * 1024,
            http2_max_header_list_size: 256 * 1024,
            websocket_handshake_max_header_size: 64 * 1024,
            socks5_port: None,
            socks5_auth_required: false,
            socks5_username: None,
            socks5_password: None,
            verbose_logging: true,
            access_mode: bifrost_proxy::AccessMode::AllowAll,
            client_whitelist: Vec::new(),
            allow_lan: true,
            unsafe_ssl: false,
            max_body_buffer_size: 10 * 1024 * 1024,
            max_body_probe_size: 64 * 1024,
            binary_traffic_performance_mode: true,
            enable_socks: true,
        };

        let server = ProxyServer::new(config).with_rules(resolver);
        let listener = server.bind(addr).await?;

        tokio::spawn(async move {
            tokio::select! {
                result = server.run_with_listener(listener) => {
                    if let Err(e) = result {
                        tracing::error!("Proxy server error: {}", e);
                    }
                }
                _ = shutdown_rx => {
                    tracing::info!("Proxy server shutting down");
                }
            }
        });

        Ok(Self {
            addr,
            shutdown_tx: Some(shutdown_tx),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn start_with_admin(
        port: u16,
        rules: Vec<&str>,
        enable_tls_interception: bool,
        unsafe_ssl: bool,
    ) -> Result<(Self, Arc<AdminState>), Box<dyn std::error::Error + Send + Sync>> {
        let normalized_rules = collapse_legacy_mock_overrides(
            rules
                .iter()
                .flat_map(|rule| expand_rule_lines(rule))
                .collect(),
        );
        let parsed_rules: Vec<Rule> = normalized_rules
            .iter()
            .filter_map(|r| parse_rules(r).ok())
            .flatten()
            .collect();

        let resolver = Arc::new(RulesResolverAdapter {
            inner: CoreRulesResolver::new(parsed_rules)
                .with_values(HashMap::new())
                .disable_cache(),
        });
        let addr: SocketAddr = format!("127.0.0.1:{}", port).parse()?;

        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let config = ProxyConfig {
            port,
            host: "127.0.0.1".to_string(),
            enable_tls_interception,
            intercept_exclude: Vec::new(),
            intercept_include: Vec::new(),
            app_intercept_exclude: Vec::new(),
            app_intercept_include: Vec::new(),
            timeout_secs: 30,
            http1_max_header_size: 64 * 1024,
            http2_max_header_list_size: 256 * 1024,
            websocket_handshake_max_header_size: 64 * 1024,
            socks5_port: None,
            socks5_auth_required: false,
            socks5_username: None,
            socks5_password: None,
            verbose_logging: true,
            access_mode: bifrost_proxy::AccessMode::AllowAll,
            client_whitelist: Vec::new(),
            allow_lan: true,
            unsafe_ssl,
            max_body_buffer_size: 10 * 1024 * 1024,
            max_body_probe_size: 64 * 1024,
            binary_traffic_performance_mode: true,
            enable_socks: true,
        };

        init_crypto_provider();
        let ca = generate_root_ca().map_err(|e| format!("Failed to generate CA: {}", e))?;
        let ca_cert = ca
            .certificate_der()
            .map_err(|e| format!("Failed to get CA cert: {}", e))?;
        let ca_key = ca.private_key_der();
        let ca = Arc::new(ca);
        let cert_generator = Arc::new(DynamicCertGenerator::new(ca.clone()));
        let sni_resolver = Arc::new(SniResolver::new(ca));
        let tls_config = Arc::new(TlsConfig {
            ca_cert: Some(ca_cert.to_vec()),
            ca_key: Some(ca_key.secret_der().to_vec()),
            cert_generator: Some(cert_generator),
            sni_resolver: Some(sni_resolver),
        });

        let runtime_config = RuntimeConfig {
            enable_tls_interception,
            intercept_exclude: Vec::new(),
            intercept_include: Vec::new(),
            app_intercept_exclude: Vec::new(),
            app_intercept_include: Vec::new(),
            unsafe_ssl,
            disconnect_on_config_change: true,
        };

        let connection_registry = ConnectionRegistry::new(true);

        let temp_dir = std::env::temp_dir().join(format!("bifrost_e2e_test_{}", port));
        let body_store = Arc::new(parking_lot::RwLock::new(BodyStore::new(
            temp_dir.clone(),
            2 * 1024 * 1024,
            7,
            64 * 1024,
            Duration::from_millis(200),
        )));

        let ws_payload_store = Arc::new(WsPayloadStore::new(
            temp_dir.clone(),
            256 * 1024,
            Duration::from_millis(200),
            128,
            7,
        ));
        std::mem::drop(start_ws_payload_cleanup_task(ws_payload_store.clone()));

        let traffic_dir = temp_dir.join("traffic");
        let traffic_db_store = Arc::new(
            bifrost_admin::TrafficDbStore::new(traffic_dir, 1000, 0, Some(24))
                .expect("failed to create traffic db store"),
        );

        let frame_store = Arc::new(bifrost_admin::FrameStore::new(temp_dir, Some(24)));
        std::mem::drop(start_frame_cleanup_task(frame_store.clone()));

        let (async_traffic_writer, async_traffic_rx) = AsyncTrafficWriter::new(10000);
        let _async_traffic_task =
            start_async_traffic_processor(async_traffic_rx, traffic_db_store.clone());

        let admin_state = AdminState::new(port)
            .with_runtime_config(runtime_config)
            .with_connection_registry(connection_registry)
            .with_body_store(body_store)
            .with_ws_payload_store(ws_payload_store)
            .with_traffic_db_store_shared(traffic_db_store)
            .with_async_traffic_writer(async_traffic_writer)
            .with_frame_store_shared(frame_store);
        std::mem::drop(start_connection_cleanup_task(
            admin_state.connection_monitor.clone(),
        ));

        let server = ProxyServer::new(config)
            .with_rules(resolver)
            .with_tls_config(tls_config)
            .with_admin_state(admin_state);

        let admin_state_arc = server
            .admin_state()
            .cloned()
            .expect("admin_state should be set");

        let listener = server.bind(addr).await?;

        tokio::spawn(async move {
            tokio::select! {
                result = server.run_with_listener(listener) => {
                    if let Err(e) = result {
                        tracing::error!("Proxy server error: {}", e);
                    }
                }
                _ = shutdown_rx => {
                    tracing::info!("Proxy server shutting down");
                }
            }
        });

        Ok((
            Self {
                addr,
                shutdown_tx: Some(shutdown_tx),
            },
            admin_state_arc,
        ))
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn proxy_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub async fn wait_for_ready(&self) -> Result<(), String> {
        let max_attempts = 50;
        for i in 0..max_attempts {
            match tokio::net::TcpStream::connect(self.addr).await {
                Ok(_) => return Ok(()),
                Err(_) if i < max_attempts - 1 => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(e) => {
                    return Err(format!(
                        "Proxy at {} not ready after {}ms: {}",
                        self.addr,
                        max_attempts * 50,
                        e
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn shutdown(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for ProxyInstance {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::parse_cors_config;

    #[test]
    fn parse_cors_config_supports_multiline_legacy_format() {
        let config = parse_cors_config(
            "origin: https://frontend.test\nmethod: POST\nheaders: x-trace-id,x-auth-token",
        );

        assert!(config.enabled);
        assert_eq!(config.origin.as_deref(), Some("https://frontend.test"));
        assert_eq!(config.methods.as_deref(), Some("POST"));
        assert_eq!(config.headers.as_deref(), Some("x-trace-id,x-auth-token"));
    }

    #[test]
    fn parse_cors_config_supports_plural_keys_in_multiline_format() {
        let config = parse_cors_config(
            "origin: https://app.example.com\nmethods: GET, POST\nheaders: Content-Type\ncredentials: true\nmaxAge: 86400",
        );

        assert!(config.enabled);
        assert_eq!(config.origin.as_deref(), Some("https://app.example.com"));
        assert_eq!(config.methods.as_deref(), Some("GET, POST"));
        assert_eq!(config.headers.as_deref(), Some("Content-Type"));
        assert_eq!(config.credentials, Some(true));
        assert_eq!(config.max_age, Some(86400));
    }
}
