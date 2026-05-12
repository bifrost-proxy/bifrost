use std::collections::HashMap;

use base64::Engine;
use bifrost_core::{Protocol, ResolvedRules as CoreResolvedRules};
use bytes::Bytes;
use serde_json::Value;
use tracing::info;
use url::Url;

#[derive(Debug, Clone, Default)]
pub struct AppliedRules {
    pub forward_url: Option<String>,
    pub forward_source_path: Option<String>,
    pub forward_target_path_exact: bool,
    pub forwarding_passthrough: bool,
    pub host: Option<String>,
    pub method: Option<String>,
    pub ua: Option<String>,
    pub referer: Option<String>,
    pub auth: Option<String>,
    pub req_headers: Vec<(String, String)>,
    pub req_cookies: Vec<(String, String)>,
    pub req_del_cookies: Vec<String>,
    pub delete_req_headers: Vec<String>,
    pub delete_url_params: Vec<String>,
    pub url_params: Vec<(String, String)>,
    pub url_replace: Vec<(String, String)>,
    pub url_replace_regex: Vec<ReplayRegexReplace>,
    pub req_body: Option<Bytes>,
    pub req_prepend: Option<Bytes>,
    pub req_append: Option<Bytes>,
    pub req_replace: Vec<(String, String)>,
    pub req_replace_regex: Vec<ReplayRegexReplace>,
    pub req_merge: Option<Value>,
    pub req_type: Option<String>,
    pub req_charset: Option<String>,
    pub forwarded_for: Option<String>,
    pub header_replace: Vec<HeaderReplaceRule>,
    pub req_cors: Option<CorsConfig>,
}

#[derive(Debug, Clone)]
pub struct ReplayRegexReplace {
    pattern: regex::Regex,
    replacement: String,
    global: bool,
}

#[derive(Debug, Clone, Eq, PartialEq)]
enum HeaderReplaceTarget {
    Request,
    Response,
}

#[derive(Debug, Clone)]
pub struct HeaderReplaceRule {
    target: HeaderReplaceTarget,
    header_name: String,
    pattern: String,
    replacement: String,
}

#[derive(Debug, Clone, Default)]
pub struct CorsConfig {
    origin: Option<String>,
    methods: Option<String>,
    headers: Option<String>,
}

pub struct AppliedRequest {
    pub url: String,
    pub method: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Bytes>,
}

pub fn build_applied_rules(core_rules: &CoreResolvedRules) -> AppliedRules {
    let mut applied = AppliedRules::default();

    for rule in &core_rules.rules {
        match rule.rule.protocol {
            Protocol::Http | Protocol::Https | Protocol::Ws | Protocol::Wss
                if !applied.forwarding_passthrough && applied.forward_url.is_none() =>
            {
                let scheme = match rule.rule.protocol {
                    Protocol::Http => "http",
                    Protocol::Https => "https",
                    Protocol::Ws => "ws",
                    Protocol::Wss => "wss",
                    _ => continue,
                };
                let value = rule.resolved_value.trim_end_matches('/');
                let forward_url = if value.contains("://") {
                    value.to_string()
                } else {
                    format!("{}://{}", scheme, value)
                };
                applied.forward_source_path = extract_path_from_pattern(&rule.rule.pattern);
                applied.forward_target_path_exact =
                    rule.rule.pattern.trim_start_matches('!').starts_with('^');
                applied.forward_url = Some(forward_url);
            }
            Protocol::Host | Protocol::XHost
                if !applied.forwarding_passthrough && applied.host.is_none() =>
            {
                applied.host = Some(rule.resolved_value.clone());
            }
            Protocol::Passthrough => {
                applied.forwarding_passthrough = true;
                applied.forward_url = None;
                applied.forward_source_path = None;
                applied.forward_target_path_exact = false;
                applied.host = None;
            }
            Protocol::Method if applied.method.is_none() => {
                applied.method = Some(rule.resolved_value.clone());
            }
            Protocol::Ua if applied.ua.is_none() => {
                applied.ua = Some(rule.resolved_value.clone());
            }
            Protocol::Referer if applied.referer.is_none() => {
                applied.referer = Some(rule.resolved_value.clone());
            }
            Protocol::Auth if applied.auth.is_none() => {
                applied.auth = Some(rule.resolved_value.clone());
            }
            Protocol::ReqHeaders => {
                applied
                    .req_headers
                    .extend(parse_header_values(&rule.resolved_value));
            }
            Protocol::ReqCookies => {
                applied
                    .req_cookies
                    .extend(parse_header_values(&rule.resolved_value));
            }
            Protocol::Delete => {
                let parsed = parse_delete_value(&rule.resolved_value);
                applied.delete_req_headers.extend(parsed.req_headers);
                applied.delete_url_params.extend(parsed.url_params);
            }
            Protocol::UrlParams => {
                for (key, value) in parse_url_params_value(&rule.resolved_value) {
                    if value.is_empty() {
                        applied.delete_url_params.push(key);
                    } else {
                        applied.url_params.push((key, value));
                    }
                }
            }
            Protocol::UrlReplace => {
                let parsed = parse_replace_value(&rule.resolved_value);
                applied.url_replace.extend(parsed.string_rules);
                applied.url_replace_regex.extend(parsed.regex_rules);
            }
            Protocol::ReqBody if applied.req_body.is_none() => {
                let content = extract_inline_content(&rule.resolved_value);
                applied.req_body = Some(Bytes::from(content));
            }
            Protocol::ReqPrepend if applied.req_prepend.is_none() => {
                let content = extract_inline_content(&rule.resolved_value);
                applied.req_prepend = Some(Bytes::from(content));
            }
            Protocol::ReqAppend if applied.req_append.is_none() => {
                let content = extract_inline_content(&rule.resolved_value);
                applied.req_append = Some(Bytes::from(content));
            }
            Protocol::ReqReplace => {
                let parsed = parse_replace_value(&rule.resolved_value);
                applied.req_replace.extend(parsed.string_rules);
                applied.req_replace_regex.extend(parsed.regex_rules);
            }
            Protocol::Params => {
                if let Some(value) = parse_merge_value(&rule.resolved_value) {
                    applied.req_merge = Some(value);
                }
            }
            Protocol::ReqType => {
                applied.req_type = Some(rule.resolved_value.clone());
            }
            Protocol::ReqCharset => {
                applied.req_charset = Some(rule.resolved_value.clone());
            }
            Protocol::ForwardedFor => {
                applied.forwarded_for = Some(rule.resolved_value.clone());
            }
            Protocol::HeaderReplace => {
                applied
                    .header_replace
                    .extend(parse_header_replace_value(&rule.resolved_value));
            }
            Protocol::ReqCors => {
                applied.req_cors = Some(parse_cors_config(&rule.resolved_value));
            }
            _ => {}
        }
    }

    applied
}

pub fn apply_host_rule(original_url: &str, host: Option<&str>) -> Result<String, String> {
    let Some(new_host) = host else {
        return Ok(original_url.to_string());
    };

    let mut url = Url::parse(original_url).map_err(|e| format!("Invalid URL: {}", e))?;

    if let Some((host_part, port_part)) = new_host.split_once(':') {
        url.set_host(Some(host_part)).map_err(|e| e.to_string())?;
        if let Ok(port) = port_part.parse::<u16>() {
            url.set_port(Some(port)).map_err(|_| "Invalid port")?;
        }
    } else {
        url.set_host(Some(new_host)).map_err(|e| e.to_string())?;
        let _ = url.set_port(None);
    }

    Ok(url.to_string())
}

pub fn apply_url_rules(url: &str, rules: &AppliedRules) -> String {
    let mut result = url.to_string();

    for (from, to) in &rules.url_replace {
        result = result.replace(from, to);
    }

    for regex_rule in &rules.url_replace_regex {
        result = if regex_rule.global {
            regex_rule
                .pattern
                .replace_all(&result, regex_rule.replacement.as_str())
                .into_owned()
        } else {
            regex_rule
                .pattern
                .replace(&result, regex_rule.replacement.as_str())
                .into_owned()
        };
    }

    if !rules.url_params.is_empty() || !rules.delete_url_params.is_empty() {
        let mut parsed = match Url::parse(&result) {
            Ok(u) => u,
            Err(_) => return result,
        };

        let mut query_pairs: Vec<(String, String)> = parsed
            .query_pairs()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();

        if !rules.delete_url_params.is_empty() {
            query_pairs.retain(|(key, _)| {
                !rules
                    .delete_url_params
                    .iter()
                    .any(|delete_key| delete_key == key)
            });
        }

        for (key, value) in &rules.url_params {
            if let Some(existing) = query_pairs.iter_mut().find(|(k, _)| k == key) {
                existing.1 = value.clone();
            } else {
                query_pairs.push((key.clone(), value.clone()));
            }
        }

        if query_pairs.is_empty() {
            parsed.set_query(None);
        } else {
            let query_string: String = query_pairs
                .iter()
                .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            parsed.set_query(Some(&query_string));
        }

        result = parsed.to_string();
    }

    result
}

pub fn apply_all_request_rules(
    original_url: &str,
    original_method: &str,
    original_headers: &[(String, String)],
    original_body: Option<&[u8]>,
    applied_rules: &AppliedRules,
    verbose_logging: bool,
) -> Result<AppliedRequest, String> {
    let final_method = applied_rules
        .method
        .clone()
        .unwrap_or_else(|| original_method.to_string());

    if verbose_logging && applied_rules.method.is_some() {
        info!(
            "[REPLAY_RULES] Method: {} -> {}",
            original_method, final_method
        );
    }

    let base_url = if let Some(ref forward_url) = applied_rules.forward_url {
        let original_parsed =
            Url::parse(original_url).map_err(|e| format!("Invalid original URL: {}", e))?;
        let forward_parsed =
            Url::parse(forward_url).map_err(|e| format!("Invalid forward URL: {}", e))?;

        let mut new_url = forward_parsed.clone();
        let original_path = original_parsed.path().to_string()
            + original_parsed
                .query()
                .map(|query| format!("?{}", query))
                .unwrap_or_default()
                .as_str();
        if let Some(target_path) = extract_target_path_from_forward_url(forward_url) {
            let rewritten_path = if applied_rules.forward_target_path_exact {
                target_path
            } else {
                rewrite_path_with_prefix(
                    &original_path,
                    applied_rules.forward_source_path.as_deref(),
                    &target_path,
                )
            };
            let (path, query) = split_path_and_query(&rewritten_path);
            new_url.set_path(path);
            new_url.set_query(query);
        } else {
            new_url.set_path(original_parsed.path());
            new_url.set_query(original_parsed.query());
        }

        if verbose_logging {
            info!(
                "[REPLAY_RULES] Forward URL: {} -> {}",
                original_url, new_url
            );
        }
        new_url.to_string()
    } else {
        original_url.to_string()
    };

    let url_after_host = apply_host_rule(&base_url, applied_rules.host.as_deref())?;

    if verbose_logging && applied_rules.host.is_some() {
        info!("[REPLAY_RULES] Host: {} -> {}", base_url, url_after_host);
    }

    let final_url = apply_url_rules(&url_after_host, applied_rules);

    if verbose_logging && final_url != url_after_host {
        info!("[REPLAY_RULES] URL: {} -> {}", url_after_host, final_url);
    }

    let mut final_headers = apply_header_rules(original_headers, applied_rules, verbose_logging);

    if let Some(ref ua) = applied_rules.ua {
        apply_single_header(&mut final_headers, "user-agent", ua, verbose_logging);
    }

    if let Some(ref referer) = applied_rules.referer {
        apply_single_header(&mut final_headers, "referer", referer, verbose_logging);
    }

    if let Some(ref auth) = applied_rules.auth {
        let encoded = base64::engine::general_purpose::STANDARD.encode(auth);
        let header_value = format!("Basic {}", encoded);
        apply_single_header(
            &mut final_headers,
            "authorization",
            &header_value,
            verbose_logging,
        );
    }

    apply_additional_header_rules(&mut final_headers, applied_rules, verbose_logging);

    let final_body = apply_body_rules(
        original_body,
        &final_headers,
        applied_rules,
        verbose_logging,
    );

    Ok(AppliedRequest {
        url: final_url,
        method: final_method,
        headers: final_headers,
        body: final_body,
    })
}

fn extract_path_from_pattern(pattern: &str) -> Option<String> {
    let s = pattern
        .strip_prefix("http://")
        .or_else(|| pattern.strip_prefix("https://"))
        .or_else(|| pattern.strip_prefix("ws://"))
        .or_else(|| pattern.strip_prefix("wss://"))
        .unwrap_or(pattern);

    s.find('/')
        .map(|pos| s[pos..].to_string())
        .filter(|p| p != "/")
}

fn extract_target_path_from_forward_url(forward_url: &str) -> Option<String> {
    Url::parse(forward_url)
        .ok()
        .map(|url| {
            let mut path = url.path().to_string();
            if let Some(query) = url.query() {
                path.push('?');
                path.push_str(query);
            }
            path
        })
        .filter(|path| path != "/")
}

fn split_path_and_query(path_and_query: &str) -> (&str, Option<&str>) {
    match path_and_query.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (path_and_query, None),
    }
}

fn rewrite_path_with_prefix(
    request_path: &str,
    source_path: Option<&str>,
    target_path: &str,
) -> String {
    if let Some(source) = source_path {
        let source_trimmed = source.trim_end_matches('/');
        if let Some(remaining) = request_path.strip_prefix(source_trimmed) {
            let target = target_path.trim_end_matches('/');
            if remaining.is_empty() {
                format!("{}/", target)
            } else if remaining.starts_with('/') || remaining.starts_with('?') {
                format!("{}{}", target, remaining)
            } else {
                format!("{}/{}", target, remaining)
            }
        } else {
            request_path.to_string()
        }
    } else {
        let target = target_path.trim_end_matches('/');
        if request_path == "/" {
            format!("{}/", target)
        } else if request_path.starts_with('/') {
            format!("{}{}", target, request_path)
        } else {
            format!("{}/{}", target, request_path)
        }
    }
}

fn apply_header_rules(
    original_headers: &[(String, String)],
    rules: &AppliedRules,
    verbose_logging: bool,
) -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> = original_headers.to_vec();

    for header_name in &rules.delete_req_headers {
        let lower_name = header_name.to_lowercase();
        let before_len = headers.len();
        headers.retain(|(k, _)| k.to_lowercase() != lower_name);
        if verbose_logging && headers.len() < before_len {
            info!("[REPLAY_RULES] Deleted header: {}", header_name);
        }
    }

    for (name, value) in &rules.req_headers {
        let lower_name = name.to_lowercase();
        let mut found = false;
        for (k, v) in headers.iter_mut() {
            if k.to_lowercase() == lower_name {
                if verbose_logging {
                    info!(
                        "[REPLAY_RULES] Header {} : \"{}\" -> \"{}\"",
                        name, v, value
                    );
                }
                *v = value.clone();
                found = true;
                break;
            }
        }
        if !found {
            if verbose_logging {
                info!("[REPLAY_RULES] Header {} : (none) -> \"{}\"", name, value);
            }
            headers.push((name.clone(), value.clone()));
        }
    }

    if !rules.req_cookies.is_empty() || !rules.req_del_cookies.is_empty() {
        apply_cookie_rules(&mut headers, rules, verbose_logging);
    }

    headers
}

fn apply_additional_header_rules(
    headers: &mut Vec<(String, String)>,
    rules: &AppliedRules,
    verbose_logging: bool,
) {
    for rule in &rules.header_replace {
        if rule.target != HeaderReplaceTarget::Request {
            continue;
        }
        for (key, value) in headers.iter_mut() {
            if key.eq_ignore_ascii_case(&rule.header_name) {
                let next = value.replace(&rule.pattern, &rule.replacement);
                if verbose_logging {
                    info!(
                        "[REPLAY_RULES] Header replace {} : \"{}\" -> \"{}\"",
                        rule.header_name, value, next
                    );
                }
                *value = next;
            }
        }
    }

    if let Some(ref content_type) = rules.req_type {
        apply_single_header(
            headers,
            "content-type",
            expand_content_type_shortcut(content_type),
            verbose_logging,
        );
    }

    if let Some(ref charset) = rules.req_charset {
        let current = headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("content-type"))
            .map(|(_, value)| value.as_str())
            .unwrap_or("text/plain");
        let base = current.split(';').next().unwrap_or(current).trim();
        apply_single_header(
            headers,
            "content-type",
            &format!("{}; charset={}", base, charset),
            verbose_logging,
        );
    }

    if let Some(ref forwarded_for) = rules.forwarded_for {
        let current = headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("x-forwarded-for"))
            .map(|(_, value)| value.clone());
        let next = current
            .filter(|value| !value.is_empty())
            .map(|value| format!("{}, {}", value, forwarded_for))
            .unwrap_or_else(|| forwarded_for.clone());
        apply_single_header(headers, "x-forwarded-for", &next, verbose_logging);
    }

    if let Some(ref cors) = rules.req_cors {
        if let Some(ref origin) = cors.origin {
            apply_single_header(headers, "origin", origin, verbose_logging);
        }
        if let Some(ref methods) = cors.methods {
            apply_single_header(
                headers,
                "access-control-request-method",
                methods,
                verbose_logging,
            );
        }
        if let Some(ref cors_headers) = cors.headers {
            apply_single_header(
                headers,
                "access-control-request-headers",
                cors_headers,
                verbose_logging,
            );
        }
    }
}

fn apply_single_header(
    headers: &mut Vec<(String, String)>,
    header_name: &str,
    value: &str,
    verbose_logging: bool,
) {
    let lower_name = header_name.to_lowercase();
    for (k, v) in headers.iter_mut() {
        if k.to_lowercase() == lower_name {
            if verbose_logging {
                info!(
                    "[REPLAY_RULES] {} : \"{}\" -> \"{}\"",
                    header_name, v, value
                );
            }
            *v = value.to_string();
            return;
        }
    }
    if verbose_logging {
        info!("[REPLAY_RULES] {} : (none) -> \"{}\"", header_name, value);
    }
    headers.push((header_name.to_string(), value.to_string()));
}

fn apply_cookie_rules(
    headers: &mut Vec<(String, String)>,
    rules: &AppliedRules,
    verbose_logging: bool,
) {
    let cookie_header = headers
        .iter()
        .find(|(k, _)| k.to_lowercase() == "cookie")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();

    let mut cookies: HashMap<String, String> = cookie_header
        .split(';')
        .filter_map(|pair| {
            let mut parts = pair.trim().splitn(2, '=');
            let name = parts.next()?.trim().to_string();
            let value = parts.next().unwrap_or("").trim().to_string();
            if name.is_empty() {
                None
            } else {
                Some((name, value))
            }
        })
        .collect();

    for del_name in &rules.req_del_cookies {
        if cookies.remove(del_name).is_some() && verbose_logging {
            info!("[REPLAY_RULES] Cookie {} : deleted", del_name);
        }
    }

    for (name, value) in &rules.req_cookies {
        let old_value = cookies.insert(name.clone(), value.clone());
        if verbose_logging {
            if let Some(old) = old_value {
                info!(
                    "[REPLAY_RULES] Cookie {} : \"{}\" -> \"{}\"",
                    name, old, value
                );
            } else {
                info!("[REPLAY_RULES] Cookie {} : (none) -> \"{}\"", name, value);
            }
        }
    }

    let new_cookie_value: String = cookies
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join("; ");

    headers.retain(|(k, _)| k.to_lowercase() != "cookie");

    if !new_cookie_value.is_empty() {
        headers.push(("Cookie".to_string(), new_cookie_value));
    }
}

fn apply_body_rules(
    original_body: Option<&[u8]>,
    headers: &[(String, String)],
    rules: &AppliedRules,
    verbose_logging: bool,
) -> Option<Bytes> {
    if rules.req_body.is_some() {
        if verbose_logging {
            info!("[REPLAY_RULES] Request body replaced by rule");
        }
        return rules.req_body.clone();
    }

    let mut body = original_body.map(|b| b.to_vec()).unwrap_or_default();

    if let Some(ref prepend) = rules.req_prepend {
        if verbose_logging {
            info!("[REPLAY_RULES] Prepending {} bytes to body", prepend.len());
        }
        let mut new_body = prepend.to_vec();
        new_body.extend_from_slice(&body);
        body = new_body;
    }

    if let Some(ref append) = rules.req_append {
        if verbose_logging {
            info!("[REPLAY_RULES] Appending {} bytes to body", append.len());
        }
        body.extend_from_slice(append);
    }

    if !rules.req_replace.is_empty() {
        let mut body_str = String::from_utf8_lossy(&body).to_string();
        for (from, to) in &rules.req_replace {
            if body_str.contains(from) {
                body_str = body_str.replace(from, to);
                if verbose_logging {
                    info!("[REPLAY_RULES] Body replace: \"{}\" -> \"{}\"", from, to);
                }
            }
        }
        body = body_str.into_bytes();
    }

    if !rules.req_replace_regex.is_empty() {
        let mut body_str = String::from_utf8_lossy(&body).to_string();
        for regex_rule in &rules.req_replace_regex {
            body_str = if regex_rule.global {
                regex_rule
                    .pattern
                    .replace_all(&body_str, regex_rule.replacement.as_str())
                    .into_owned()
            } else {
                regex_rule
                    .pattern
                    .replace(&body_str, regex_rule.replacement.as_str())
                    .into_owned()
            };
        }
        body = body_str.into_bytes();
    }

    if let Some(ref merge_value) = rules.req_merge {
        let content_type = headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("content-type"))
            .map(|(_, value)| value.to_ascii_lowercase())
            .unwrap_or_default();
        if content_type.starts_with("application/x-www-form-urlencoded") {
            if let Some(merged) = merge_form_urlencoded(&body, merge_value) {
                body = merged.to_vec();
            }
        } else if let Ok(original) = serde_json::from_slice::<Value>(&body) {
            let merged = merge_json(original, merge_value.clone());
            if let Ok(encoded) = serde_json::to_vec(&merged) {
                body = encoded;
            }
        }
    }

    if body.is_empty() {
        None
    } else {
        Some(Bytes::from(body))
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
        .map(str::trim)
        .filter(|part| !part.is_empty() && !part.starts_with('#'))
        .filter_map(parse_header_value)
        .collect::<Vec<_>>()
}

fn parse_header_value(value: &str) -> Option<(String, String)> {
    let trimmed = value.trim();

    if let Some(pos) = trimmed.find(':') {
        let key = trimmed[..pos].trim().to_string();
        let val = trimmed[pos + 1..].trim().to_string();
        if !key.is_empty() {
            return Some((key, val));
        }
    }

    if let Some(pos) = trimmed.find('=') {
        let key = trimmed[..pos].trim().to_string();
        let val = trimmed[pos + 1..].trim().to_string();
        if !key.is_empty() {
            return Some((key, val));
        }
    }

    None
}

#[derive(Default)]
struct ParsedDeleteValue {
    req_headers: Vec<String>,
    url_params: Vec<String>,
}

fn parse_delete_value(value: &str) -> ParsedDeleteValue {
    let mut result = ParsedDeleteValue::default();

    for part in value.split('|') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        if let Some(header) = part
            .strip_prefix("reqHeaders.")
            .or_else(|| part.strip_prefix("req."))
        {
            result.req_headers.push(header.to_string());
        } else if let Some(param) = part.strip_prefix("urlParams.") {
            result.url_params.push(param.to_string());
        } else if part.starts_with("resHeaders.") || part.starts_with("res.") {
            continue;
        } else {
            result.req_headers.push(part.to_string());
        }
    }

    result
}

fn parse_url_params_value(value: &str) -> Vec<(String, String)> {
    let content = strip_wrapping_parens(value.trim());
    let mut params = Vec::new();

    for part in content.split(['&', ',', '\n']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        let (key, value) = part
            .split_once('=')
            .or_else(|| part.split_once(':'))
            .unwrap_or((part, ""));
        let key = key.trim();
        if !key.is_empty() {
            params.push((key.to_string(), value.trim().to_string()));
        }
    }

    params
}

struct ParsedReplaceRules {
    string_rules: Vec<(String, String)>,
    regex_rules: Vec<ReplayRegexReplace>,
}

fn parse_replace_value(value: &str) -> ParsedReplaceRules {
    let mut string_rules = Vec::new();
    let mut regex_rules = Vec::new();

    for pair in value.split('&') {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }

        let (from, to) = pair
            .split_once('=')
            .or_else(|| pair.split_once(' '))
            .unwrap_or((pair, ""));
        let from = url_decode(from);
        let to = url_decode(to);
        if let Some((pattern, global)) = parse_regex_pattern(&from) {
            regex_rules.push(ReplayRegexReplace {
                pattern,
                replacement: to,
                global,
            });
        } else {
            string_rules.push((from, to));
        }
    }

    ParsedReplaceRules {
        string_rules,
        regex_rules,
    }
}

fn parse_merge_value(value: &str) -> Option<Value> {
    if let Ok(json_value) = serde_json::from_str(value) {
        return Some(json_value);
    }

    let content = strip_wrapping_parens(value.trim());
    if content.contains('=') {
        let mut object = serde_json::Map::new();
        for pair in content.split('&') {
            if let Some((key, value)) = pair.split_once('=') {
                object.insert(
                    key.trim().to_string(),
                    Value::String(value.trim().to_string()),
                );
            }
        }
        if !object.is_empty() {
            return Some(Value::Object(object));
        }
    }

    None
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

fn url_decode(value: &str) -> String {
    urlencoding::decode(value)
        .map(|decoded| decoded.into_owned())
        .unwrap_or_else(|_| value.to_string())
}

fn parse_regex_pattern(value: &str) -> Option<(regex::Regex, bool)> {
    let value = value.trim();
    if !value.starts_with('/') {
        return None;
    }

    let global = value.ends_with("/g") || value.ends_with("/gi") || value.ends_with("/ig");
    let case_insensitive =
        value.ends_with("/i") || value.ends_with("/gi") || value.ends_with("/ig");

    let end_pos = if global && case_insensitive {
        value.len().checked_sub(3)?
    } else if global || case_insensitive {
        value.len().checked_sub(2)?
    } else if value.len() > 1 && value.ends_with('/') {
        value.len() - 1
    } else {
        return None;
    };

    let pattern = &value[1..end_pos];
    if pattern.is_empty() {
        return None;
    }

    let regex = if case_insensitive {
        regex::RegexBuilder::new(pattern)
            .case_insensitive(true)
            .build()
            .ok()?
    } else {
        regex::Regex::new(pattern).ok()?
    };

    Some((regex, global))
}

fn merge_json(base: Value, patch: Value) -> Value {
    match (base, patch) {
        (Value::Object(mut base_map), Value::Object(patch_map)) => {
            for (key, value) in patch_map {
                let base_value = base_map.remove(&key).unwrap_or(Value::Null);
                base_map.insert(key, merge_json(base_value, value));
            }
            Value::Object(base_map)
        }
        (_, patch) => patch,
    }
}

fn merge_form_urlencoded(body: &[u8], patch: &Value) -> Option<Bytes> {
    let Value::Object(patch_map) = patch else {
        return None;
    };

    let mut pairs: Vec<(String, String)> = url::form_urlencoded::parse(body).into_owned().collect();
    for (key, value) in patch_map {
        let value_string = match value {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Null => String::new(),
            other => other.to_string(),
        };

        if let Some(existing) = pairs
            .iter_mut()
            .find(|(existing_key, _)| existing_key == key)
        {
            existing.1 = value_string;
        } else {
            pairs.push((key.clone(), value_string));
        }
    }

    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in pairs {
        serializer.append_pair(&key, &value);
    }
    Some(Bytes::from(serializer.finish()))
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

fn extract_inline_content(value: &str) -> String {
    if value.starts_with('{') && value.ends_with('}') && value.len() > 1 {
        value[1..value.len() - 1].to_string()
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bifrost_core::{parse_rules, RequestContext, RuleParser, RulesResolver};

    #[test]
    fn test_apply_host_rule_basic() {
        let result = apply_host_rule("http://example.com/path", Some("newhost.com")).unwrap();
        assert_eq!(result, "http://newhost.com/path");
    }

    #[test]
    fn test_apply_host_rule_with_port() {
        let result = apply_host_rule("http://example.com/path", Some("newhost.com:8080")).unwrap();
        assert_eq!(result, "http://newhost.com:8080/path");
    }

    #[test]
    fn test_apply_host_rule_https() {
        let result = apply_host_rule("https://example.com/path", Some("newhost.com")).unwrap();
        assert_eq!(result, "https://newhost.com/path");
    }

    #[test]
    fn test_apply_host_rule_none() {
        let result = apply_host_rule("http://example.com/path", None).unwrap();
        assert_eq!(result, "http://example.com/path");
    }

    #[test]
    fn test_apply_url_rules_replace() {
        let rules = AppliedRules {
            url_replace: vec![("/old/".to_string(), "/new/".to_string())],
            ..Default::default()
        };
        let result = apply_url_rules("http://example.com/old/path", &rules);
        assert_eq!(result, "http://example.com/new/path");
    }

    #[test]
    fn test_apply_url_rules_params() {
        let rules = AppliedRules {
            url_params: vec![("foo".to_string(), "bar".to_string())],
            ..Default::default()
        };
        let result = apply_url_rules("http://example.com/path", &rules);
        assert!(result.contains("foo=bar"));
    }

    #[test]
    fn test_apply_forward_rule_uses_suffix_after_matched_source_path() {
        let rules = AppliedRules {
            forward_url: Some("http://localhost:9000/labor_cost/static".to_string()),
            forward_source_path: Some("/labor_cost/static/".to_string()),
            ..Default::default()
        };

        let result = apply_all_request_rules(
            "https://internal.example.test/labor_cost/static/07c1d7e1fb3e13436b958af5f90ec9c8.svg",
            "GET",
            &[],
            None,
            &rules,
            false,
        )
        .unwrap();

        assert_eq!(
            result.url,
            "http://localhost:9000/labor_cost/static/07c1d7e1fb3e13436b958af5f90ec9c8.svg"
        );
    }

    #[test]
    fn test_apply_forward_rule_preserves_query_after_prefix_rewrite() {
        let rules = AppliedRules {
            forward_url: Some("http://localhost:9000/labor_cost/static".to_string()),
            forward_source_path: Some("/labor_cost/static/".to_string()),
            ..Default::default()
        };

        let result = apply_all_request_rules(
            "https://internal.example.test/labor_cost/static/app.js?v=1",
            "GET",
            &[],
            None,
            &rules,
            false,
        )
        .unwrap();

        assert_eq!(
            result.url,
            "http://localhost:9000/labor_cost/static/app.js?v=1"
        );
    }

    #[test]
    fn test_apply_forward_rule_without_target_path_preserves_original_path() {
        let rules = AppliedRules {
            forward_url: Some("http://127.0.0.1:13000".to_string()),
            forward_source_path: None,
            ..Default::default()
        };

        let result = apply_all_request_rules(
            "http://fake-host.local/test-host",
            "GET",
            &[],
            None,
            &rules,
            false,
        )
        .unwrap();

        assert_eq!(result.url, "http://127.0.0.1:13000/test-host");
    }

    #[test]
    fn test_apply_forward_rule_from_resolved_custom_rule_uses_source_path_suffix() {
        let rules = parse_rules(
            "https://internal.example.test/labor_cost/static/ http://127.0.0.1:13000/labor_cost/static/",
        )
        .unwrap();
        let resolver = RulesResolver::new(rules);
        let ctx = RequestContext::from_url(
            "https://internal.example.test/labor_cost/static/07c1d7e1fb3e13436b958af5f90ec9c8.svg?v=1",
        )
        .with_method("GET");
        let resolved_rules = resolver.resolve(&ctx);
        let applied_rules = build_applied_rules(&resolved_rules);

        let result =
            apply_all_request_rules(&ctx.url, "GET", &[], None, &applied_rules, false).unwrap();

        assert_eq!(
            result.url,
            "http://127.0.0.1:13000/labor_cost/static/07c1d7e1fb3e13436b958af5f90ec9c8.svg?v=1"
        );
    }

    #[test]
    fn test_apply_forward_rule_from_path_wildcard_uses_exact_target_path() {
        let rules = parse_rules(
            "^https://cdn-tos-cn.bytedance.net/obj/archi/obj/okrx-web/approvals-web/1.0.0.*/index.html http://127.0.0.1:8999/approvals",
        )
        .unwrap();
        let resolver = RulesResolver::new(rules);
        let ctx = RequestContext::from_url(
            "https://cdn-tos-cn.bytedance.net/obj/archi/obj/okrx-web/approvals-web/1.0.0.3505/index.html",
        )
        .with_method("GET");
        let resolved_rules = resolver.resolve(&ctx);
        let applied_rules = build_applied_rules(&resolved_rules);

        let result =
            apply_all_request_rules(&ctx.url, "GET", &[], None, &applied_rules, false).unwrap();

        assert_eq!(result.url, "http://127.0.0.1:8999/approvals");
    }

    #[test]
    fn test_passthrough_blocks_later_forward_rule_from_resolved_rules() {
        let rules_text = r#"
https://nextoncall-bd.byteintl.net/ reqHeaders://{ppe_i18n}
```ppe_i18n
x-tt-env: ppe_next_agent_new
x-use-ppe: 1
```

https://bifrost.local/ reqHeaders://{ppe2}
```ppe2
x-tt-env: ppe_next_agent_new
x-use-ppe: 1
```
https://bifrost.local/app/ passthrough://

https://bifrost.local/api/v1 passthrough://
https://bifrost.local/api/nextagent/ passthrough://
bifrost.local http://localhost:5173/
a.com status://200 resBody://{data}

```data
abc
```

b.com status://200 resBody://(value)
"#;
        let parser = RuleParser::default();
        let (rules, values) = parser.parse_rules_with_inline_values(rules_text).unwrap();
        let resolver = RulesResolver::new(rules).with_values(values);
        let ctx = RequestContext::from_url("https://bifrost.local/api/nextagent/v1/sessions")
            .with_method("POST");
        let resolved_rules = resolver.resolve(&ctx);
        let applied_rules = build_applied_rules(&resolved_rules);

        assert!(applied_rules.forwarding_passthrough);
        assert_eq!(applied_rules.forward_url, None);
        assert_eq!(applied_rules.host, None);
        assert_eq!(
            applied_rules.req_headers,
            vec![
                ("x-tt-env".to_string(), "ppe_next_agent_new".to_string()),
                ("x-use-ppe".to_string(), "1".to_string())
            ]
        );

        let result =
            apply_all_request_rules(&ctx.url, "POST", &[], Some(b"{}"), &applied_rules, false)
                .unwrap();

        assert_eq!(
            result.url,
            "https://bifrost.local/api/nextagent/v1/sessions"
        );
    }

    #[test]
    fn test_parse_header_value_colon() {
        let result = parse_header_value("Content-Type: application/json");
        assert_eq!(
            result,
            Some(("Content-Type".to_string(), "application/json".to_string()))
        );
    }

    #[test]
    fn test_parse_header_value_equals() {
        let result = parse_header_value("X-Custom=value");
        assert_eq!(result, Some(("X-Custom".to_string(), "value".to_string())));
    }

    #[test]
    fn test_parse_header_values_multiline() {
        let result = parse_header_values("x-tt-env: ppe_next_agent_new\nx-use-ppe: 1");
        assert_eq!(
            result,
            vec![
                ("x-tt-env".to_string(), "ppe_next_agent_new".to_string()),
                ("x-use-ppe".to_string(), "1".to_string())
            ]
        );
    }

    #[test]
    fn test_parse_header_values_skips_hash_comment_lines() {
        let result = parse_header_values(
            "X-Test-Env:test_env\n#comment_marker\nX-Test-Flag:1\n## X-Ignored-Env: ignored comment",
        );
        assert_eq!(
            result,
            vec![
                ("X-Test-Env".to_string(), "test_env".to_string()),
                ("X-Test-Flag".to_string(), "1".to_string())
            ]
        );
    }

    #[test]
    fn test_replay_request_rules_apply_extended_request_protocols() {
        let rules = parse_rules(
            r#"https://example.test/api reqHeaders://(X-Trace: old) reqCookies://(sid=abc,theme=dark) delete://reqHeaders.X-Remove|urlParams.drop urlParams://(keep=2&drop=) reqType://json reqCharset://utf-8 forwardedFor://1.2.3.4 headerReplace://req.X-Trace:old=new reqCors://(origin=https://app.example.test&methods=POST&headers=X-Test) reqMerge://({"b":2,"c":3})"#,
        )
        .unwrap();
        let resolver = RulesResolver::new(rules);
        let ctx =
            RequestContext::from_url("https://example.test/api?drop=1&old=1").with_method("POST");
        let resolved_rules = resolver.resolve(&ctx);
        let applied_rules = build_applied_rules(&resolved_rules);

        let result = apply_all_request_rules(
            &ctx.url,
            "POST",
            &[
                ("X-Remove".to_string(), "1".to_string()),
                ("X-Trace".to_string(), "old".to_string()),
                ("Content-Type".to_string(), "text/plain".to_string()),
                ("X-Forwarded-For".to_string(), "9.9.9.9".to_string()),
            ],
            Some(br#"{"a":1,"b":0}"#),
            &applied_rules,
            false,
        )
        .unwrap();
        let body = result.body.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let header = |name: &str| {
            result
                .headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.as_str())
        };

        assert!(!result.url.contains("drop="));
        assert!(result.url.contains("old=1"));
        assert!(result.url.contains("keep=2"));
        assert!(header("X-Remove").is_none());
        assert_eq!(header("X-Trace"), Some("new"));
        assert_eq!(
            header("Content-Type"),
            Some("application/json; charset=utf-8")
        );
        assert_eq!(header("X-Forwarded-For"), Some("9.9.9.9, 1.2.3.4"));
        assert_eq!(header("Origin"), Some("https://app.example.test"));
        assert_eq!(header("Access-Control-Request-Method"), Some("POST"));
        assert_eq!(header("Access-Control-Request-Headers"), Some("X-Test"));
        assert!(header("Cookie").unwrap_or_default().contains("sid=abc"));
        assert!(header("Cookie").unwrap_or_default().contains("theme=dark"));
        assert_eq!(value["a"], 1);
        assert_eq!(value["b"], 2);
        assert_eq!(value["c"], 3);
    }

    #[test]
    fn test_replay_request_rules_apply_form_req_merge_and_regex_replace() {
        let rules = AppliedRules {
            req_replace_regex: vec![ReplayRegexReplace {
                pattern: regex::Regex::new(r"item=\d+").unwrap(),
                replacement: "item=9".to_string(),
                global: true,
            }],
            req_merge: Some(serde_json::json!({"b": "2"})),
            ..Default::default()
        };

        let result = apply_all_request_rules(
            "https://example.test/api",
            "POST",
            &[(
                "Content-Type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            )],
            Some(b"item=1&a=old"),
            &rules,
            false,
        )
        .unwrap();
        let body = String::from_utf8(result.body.unwrap().to_vec()).unwrap();

        assert_eq!(body, "item=9&a=old&b=2");
    }

    #[test]
    fn test_extract_inline_content() {
        assert_eq!(extract_inline_content("{hello}"), "hello");
        assert_eq!(extract_inline_content("hello"), "hello");
        assert_eq!(extract_inline_content("{}"), "");
    }
}
