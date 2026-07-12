use base64::Engine;
use hyper::header::{HeaderName, HeaderValue, AUTHORIZATION};
use hyper::http::request::Parts;
use tracing::info;

use crate::server::{CorsConfig, HeaderReplaceTarget, ResolvedRules};
use crate::transform::response::expand_content_type_shortcut;
use crate::utils::logging::RequestContext;

pub fn apply_req_rules(
    parts: &mut Parts,
    rules: &ResolvedRules,
    verbose_logging: bool,
    ctx: &RequestContext,
) {
    apply_req_delete_headers(parts, rules, verbose_logging, ctx);
    apply_req_headers(parts, rules, verbose_logging, ctx);
    apply_req_cookies(parts, rules, verbose_logging, ctx);
    apply_req_method(parts, rules, verbose_logging, ctx);
    apply_req_ua(parts, rules, verbose_logging, ctx);
    apply_req_referer(parts, rules, verbose_logging, ctx);
    apply_req_auth(parts, rules, verbose_logging, ctx);
    apply_req_header_replace(parts, rules, verbose_logging, ctx);
    apply_req_type(parts, rules, verbose_logging, ctx);
    apply_req_charset(parts, rules, verbose_logging, ctx);
    apply_forwarded_for(parts, rules, verbose_logging, ctx);

    if rules.req_cors.is_enabled() {
        apply_req_cors(parts, &rules.req_cors, verbose_logging, ctx);
    }
}

fn apply_req_delete_headers(
    parts: &mut Parts,
    rules: &ResolvedRules,
    verbose_logging: bool,
    ctx: &RequestContext,
) {
    for header_name in &rules.delete_req_headers {
        if let Ok(name) = header_name.parse::<HeaderName>() {
            let old_value = parts
                .headers
                .get(&name)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());

            if parts.headers.remove(&name).is_some() && verbose_logging {
                info!(
                    "[{}] [REQ_DELETE_HEADER] {} : \"{}\" -> (deleted)",
                    ctx.id_str(),
                    header_name,
                    old_value.unwrap_or_default()
                );
            }
        }
    }
}

fn apply_req_auth(
    parts: &mut Parts,
    rules: &ResolvedRules,
    verbose_logging: bool,
    ctx: &RequestContext,
) {
    if let Some(ref auth) = rules.auth {
        let encoded = base64::engine::general_purpose::STANDARD.encode(auth);
        let header_value = format!("Basic {}", encoded);

        if let Ok(value) = header_value.parse::<HeaderValue>() {
            if verbose_logging {
                let old_value = parts
                    .headers
                    .get(AUTHORIZATION)
                    .and_then(|v| v.to_str().ok())
                    .map(|s| format!("\"{}\"", s))
                    .unwrap_or_else(|| "(none)".to_string());
                info!(
                    "[{}] [REQ_AUTH] Authorization : {} -> \"{}\"",
                    ctx.id_str(),
                    old_value,
                    header_value
                );
            }
            parts.headers.insert(AUTHORIZATION, value);
        }
    }
}

fn apply_req_header_replace(
    parts: &mut Parts,
    rules: &ResolvedRules,
    verbose_logging: bool,
    ctx: &RequestContext,
) {
    for rule in &rules.header_replace {
        if rule.target != HeaderReplaceTarget::Request {
            continue;
        }

        if let Ok(header_name) = rule.header_name.parse::<HeaderName>() {
            if let Some(current_value) = parts.headers.get(&header_name) {
                if let Ok(current_str) = current_value.to_str() {
                    let new_value = current_str.replace(&rule.pattern, &rule.replacement);

                    if let Ok(new_header_value) = new_value.parse::<HeaderValue>() {
                        if verbose_logging {
                            info!(
                                "[{}] [REQ_HEADER_REPLACE] {} : \"{}\" -> \"{}\"",
                                ctx.id_str(),
                                rule.header_name,
                                current_str,
                                new_value
                            );
                        }
                        parts.headers.insert(header_name, new_header_value);
                    }
                }
            }
        }
    }
}

fn apply_req_headers(
    parts: &mut Parts,
    rules: &ResolvedRules,
    verbose_logging: bool,
    ctx: &RequestContext,
) {
    for (name, value) in &rules.req_headers {
        if let (Ok(header_name), Ok(header_value)) =
            (name.parse::<HeaderName>(), value.parse::<HeaderValue>())
        {
            if verbose_logging {
                let old_value = parts
                    .headers
                    .get(&header_name)
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string());
                if let Some(old) = old_value {
                    info!(
                        "[{}] [REQ_HEADER] {} : \"{}\" -> \"{}\"",
                        ctx.id_str(),
                        name,
                        old,
                        value
                    );
                } else {
                    info!(
                        "[{}] [REQ_HEADER] {} : (none) -> \"{}\"",
                        ctx.id_str(),
                        name,
                        value
                    );
                }
            }
            parts.headers.insert(header_name, header_value);
        }
    }
}

fn escape_cookie_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.~!$&'()*+,;=".contains(c) {
                c.to_string()
            } else {
                format!("%{:02X}", c as u32)
            }
        })
        .collect()
}

fn escape_cookie_value(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii()
                && !c.is_ascii_control()
                && c != '"'
                && c != ','
                && c != ';'
                && c != '\\'
            {
                c.to_string()
            } else {
                format!("%{:02X}", c as u32)
            }
        })
        .collect()
}

fn apply_req_cookies(
    parts: &mut Parts,
    rules: &ResolvedRules,
    verbose_logging: bool,
    ctx: &RequestContext,
) {
    if rules.req_cookies.is_empty() && rules.req_del_cookies.is_empty() {
        return;
    }

    let existing_cookies = merge_cookie_header_values(&parts.headers);

    let mut cookies: Vec<String> = if existing_cookies.is_empty() {
        Vec::new()
    } else {
        existing_cookies
            .split(';')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    };

    for del_name in &rules.req_del_cookies {
        let before_len = cookies.len();
        cookies.retain(|c| {
            let parts: Vec<&str> = c.splitn(2, '=').collect();
            parts.first().map(|n| *n != del_name).unwrap_or(true)
        });
        if verbose_logging && cookies.len() < before_len {
            info!("[{}] [REQ_COOKIE_DEL] {} : deleted", ctx.id_str(), del_name);
        }
    }

    for (name, value) in &rules.req_cookies {
        let escaped_name = escape_cookie_name(name);
        let escaped_value = escape_cookie_value(value);
        let cookie_str = format!("{}={}", escaped_name, escaped_value);

        let found = cookies
            .iter()
            .position(|c| c.starts_with(&format!("{}=", escaped_name)));
        if let Some(idx) = found {
            let old_cookie = &cookies[idx];
            let old_value = old_cookie.split('=').nth(1).unwrap_or("").to_string();
            if verbose_logging {
                info!(
                    "[{}] [REQ_COOKIE] {} : \"{}\" -> \"{}\"",
                    ctx.id_str(),
                    name,
                    old_value,
                    escaped_value
                );
            }
            cookies[idx] = cookie_str;
        } else {
            if verbose_logging {
                info!(
                    "[{}] [REQ_COOKIE] {} : (none) -> \"{}\"",
                    ctx.id_str(),
                    name,
                    escaped_value
                );
            }
            cookies.push(cookie_str);
        }
    }

    let cookie_header = cookies.join("; ");
    if let Ok(header_value) = cookie_header.parse::<HeaderValue>() {
        parts.headers.insert(hyper::header::COOKIE, header_value);
    }
}

fn apply_req_method(
    parts: &mut Parts,
    rules: &ResolvedRules,
    verbose_logging: bool,
    ctx: &RequestContext,
) {
    if let Some(ref method) = rules.method {
        if let Ok(m) = method.parse() {
            if verbose_logging {
                info!(
                    "[{}] [REQ_METHOD] {} -> {}",
                    ctx.id_str(),
                    parts.method,
                    method
                );
            }
            parts.method = m;
        }
    }
}

fn apply_req_ua(
    parts: &mut Parts,
    rules: &ResolvedRules,
    verbose_logging: bool,
    ctx: &RequestContext,
) {
    if let Some(ref ua) = rules.ua {
        if let Ok(header_value) = ua.parse::<HeaderValue>() {
            if verbose_logging {
                let old_ua = parts
                    .headers
                    .get(hyper::header::USER_AGENT)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("(none)");
                info!("[{}] [REQ_UA] \"{}\" -> \"{}\"", ctx.id_str(), old_ua, ua);
            }
            parts
                .headers
                .insert(hyper::header::USER_AGENT, header_value);
        }
    }
}

fn apply_req_referer(
    parts: &mut Parts,
    rules: &ResolvedRules,
    verbose_logging: bool,
    ctx: &RequestContext,
) {
    if let Some(ref referer) = rules.referer {
        if let Ok(header_value) = referer.parse::<HeaderValue>() {
            if verbose_logging {
                let old_referer = parts
                    .headers
                    .get(hyper::header::REFERER)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("(none)");
                info!(
                    "[{}] [REQ_REFERER] \"{}\" -> \"{}\"",
                    ctx.id_str(),
                    old_referer,
                    referer
                );
            }
            parts.headers.insert(hyper::header::REFERER, header_value);
        }
    }
}

fn apply_req_type(
    parts: &mut Parts,
    rules: &ResolvedRules,
    verbose_logging: bool,
    ctx: &RequestContext,
) {
    if let Some(ref content_type) = rules.req_type {
        let expanded = expand_content_type_shortcut(content_type);
        if let Ok(value) = expanded.parse::<HeaderValue>() {
            if verbose_logging {
                let old_value = parts
                    .headers
                    .get(hyper::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .map(|s| format!("\"{}\"", s))
                    .unwrap_or_else(|| "(none)".to_string());
                info!(
                    "[{}] [REQ_TYPE] Content-Type : {} -> \"{}\"",
                    ctx.id_str(),
                    old_value,
                    expanded
                );
            }
            parts.headers.insert(hyper::header::CONTENT_TYPE, value);
        }
    }
}

fn apply_req_charset(
    parts: &mut Parts,
    rules: &ResolvedRules,
    verbose_logging: bool,
    ctx: &RequestContext,
) {
    if let Some(ref charset) = rules.req_charset {
        let current_ct = parts
            .headers
            .get(hyper::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("text/plain");

        let base_ct = current_ct.split(';').next().unwrap_or(current_ct).trim();
        let new_ct = format!("{}; charset={}", base_ct, charset);

        if let Ok(value) = new_ct.parse::<HeaderValue>() {
            if verbose_logging {
                info!(
                    "[{}] [REQ_CHARSET] Content-Type : \"{}\" -> \"{}\"",
                    ctx.id_str(),
                    current_ct,
                    new_ct
                );
            }
            parts.headers.insert(hyper::header::CONTENT_TYPE, value);
        }
    }
}

fn apply_forwarded_for(
    parts: &mut Parts,
    rules: &ResolvedRules,
    verbose_logging: bool,
    ctx: &RequestContext,
) {
    if let Some(ref forwarded_for) = rules.forwarded_for {
        let header_name: HeaderName = "x-forwarded-for".parse().unwrap();
        let new_value = if let Some(existing) = parts
            .headers
            .get(&header_name)
            .and_then(|v| v.to_str().ok())
        {
            format!("{}, {}", existing, forwarded_for)
        } else {
            forwarded_for.clone()
        };

        if let Ok(header_value) = new_value.parse::<HeaderValue>() {
            if verbose_logging {
                let old_value = parts
                    .headers
                    .get(&header_name)
                    .and_then(|v| v.to_str().ok())
                    .map(|s| format!("\"{}\"", s))
                    .unwrap_or_else(|| "(none)".to_string());
                info!(
                    "[{}] [REQ_FORWARDED_FOR] X-Forwarded-For : {} -> \"{}\"",
                    ctx.id_str(),
                    old_value,
                    new_value
                );
            }
            parts.headers.insert(header_name, header_value);
        }
    }
}

fn apply_req_cors(
    parts: &mut Parts,
    cors: &CorsConfig,
    verbose_logging: bool,
    ctx: &RequestContext,
) {
    if verbose_logging {
        info!(
            "[{}] [REQ_CORS] enabled with config: {:?}",
            ctx.id_str(),
            cors
        );
    }

    if let Some(ref origin) = cors.origin {
        if origin == "*" {
            parts.headers.insert(
                HeaderName::from_static("origin"),
                HeaderValue::from_static("*"),
            );
        } else if let Ok(header_value) = origin.parse::<HeaderValue>() {
            parts
                .headers
                .insert(HeaderName::from_static("origin"), header_value);
        }
    }

    if let Some(ref method) = cors.methods {
        if let Ok(header_value) = method.parse::<HeaderValue>() {
            parts.headers.insert(
                HeaderName::from_static("access-control-request-method"),
                header_value,
            );
        }
    }

    if let Some(ref headers) = cors.headers {
        if let Ok(header_value) = headers.parse::<HeaderValue>() {
            parts.headers.insert(
                HeaderName::from_static("access-control-request-headers"),
                header_value,
            );
        }
    }
}

pub fn parse_cookie_string(cookie_str: &str) -> Vec<(String, String)> {
    cookie_str
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
        .collect()
}

pub fn format_cookie_header(cookies: &[(String, String)]) -> String {
    cookies
        .iter()
        .map(|(name, value)| format!("{}={}", name, value))
        .collect::<Vec<_>>()
        .join("; ")
}

pub fn collect_all_cookies_from_headers(
    headers: &hyper::HeaderMap,
) -> std::collections::HashMap<String, String> {
    let mut cookies = std::collections::HashMap::new();
    for value in headers.get_all(hyper::header::COOKIE).iter() {
        if let Ok(s) = value.to_str() {
            for pair in s.split(';') {
                let mut iter = pair.trim().splitn(2, '=');
                if let (Some(k), Some(v)) = (iter.next(), iter.next()) {
                    let k = k.trim();
                    if !k.is_empty() {
                        cookies.insert(k.to_string(), v.to_string());
                    }
                }
            }
        }
    }
    cookies
}

pub fn merge_cookie_header_values(headers: &hyper::HeaderMap) -> String {
    let mut parts = Vec::new();
    for value in headers.get_all(hyper::header::COOKIE).iter() {
        if let Ok(s) = value.to_str() {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                parts.push(trimmed.to_string());
            }
        }
    }
    parts.join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::Method;

    fn create_test_parts() -> Parts {
        let (parts, _) = hyper::Request::builder()
            .method(Method::GET)
            .uri("http://example.com/path")
            .body(())
            .unwrap()
            .into_parts();
        parts
    }

    #[test]
    fn test_apply_req_headers() {
        let mut parts = create_test_parts();
        let mut rules = ResolvedRules::default();
        let ctx = RequestContext::new();
        rules
            .req_headers
            .push(("X-Custom-Header".to_string(), "custom-value".to_string()));
        rules
            .req_headers
            .push(("X-Another".to_string(), "another-value".to_string()));

        apply_req_rules(&mut parts, &rules, false, &ctx);

        assert_eq!(
            parts
                .headers
                .get("X-Custom-Header")
                .unwrap()
                .to_str()
                .unwrap(),
            "custom-value"
        );
        assert_eq!(
            parts.headers.get("X-Another").unwrap().to_str().unwrap(),
            "another-value"
        );
    }

    #[test]
    fn test_apply_req_cookies_new() {
        let mut parts = create_test_parts();
        let mut rules = ResolvedRules::default();
        let ctx = RequestContext::new();
        rules
            .req_cookies
            .push(("session".to_string(), "abc123".to_string()));
        rules
            .req_cookies
            .push(("user".to_string(), "test".to_string()));

        apply_req_rules(&mut parts, &rules, false, &ctx);

        let cookie = parts
            .headers
            .get(hyper::header::COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(cookie.contains("session=abc123"));
        assert!(cookie.contains("user=test"));
    }

    #[test]
    fn test_apply_req_cookies_merge() {
        let mut parts = create_test_parts();
        let ctx = RequestContext::new();
        parts.headers.insert(
            hyper::header::COOKIE,
            HeaderValue::from_static("existing=value; session=old"),
        );

        let mut rules = ResolvedRules::default();
        rules
            .req_cookies
            .push(("session".to_string(), "new".to_string()));
        rules
            .req_cookies
            .push(("added".to_string(), "cookie".to_string()));

        apply_req_rules(&mut parts, &rules, false, &ctx);

        let cookie = parts
            .headers
            .get(hyper::header::COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(cookie.contains("existing=value"));
        assert!(cookie.contains("session=new"));
        assert!(cookie.contains("added=cookie"));
        assert!(!cookie.contains("session=old"));
    }

    #[test]
    fn test_apply_req_method() {
        let mut parts = create_test_parts();
        let mut rules = ResolvedRules::default();
        let ctx = RequestContext::new();
        rules.method = Some("POST".to_string());

        apply_req_rules(&mut parts, &rules, false, &ctx);

        assert_eq!(parts.method, Method::POST);
    }

    #[test]
    fn test_apply_req_ua() {
        let mut parts = create_test_parts();
        let mut rules = ResolvedRules::default();
        let ctx = RequestContext::new();
        rules.ua = Some("Custom-Agent/1.0".to_string());

        apply_req_rules(&mut parts, &rules, false, &ctx);

        assert_eq!(
            parts
                .headers
                .get(hyper::header::USER_AGENT)
                .unwrap()
                .to_str()
                .unwrap(),
            "Custom-Agent/1.0"
        );
    }

    #[test]
    fn test_apply_req_referer() {
        let mut parts = create_test_parts();
        let mut rules = ResolvedRules::default();
        let ctx = RequestContext::new();
        rules.referer = Some("http://referrer.com".to_string());

        apply_req_rules(&mut parts, &rules, false, &ctx);

        assert_eq!(
            parts
                .headers
                .get(hyper::header::REFERER)
                .unwrap()
                .to_str()
                .unwrap(),
            "http://referrer.com"
        );
    }

    #[test]
    fn test_apply_req_cors() {
        let mut parts = create_test_parts();
        let mut rules = ResolvedRules::default();
        let ctx = RequestContext::new();
        rules.req_cors = CorsConfig::enable_all();

        apply_req_rules(&mut parts, &rules, false, &ctx);

        assert!(parts
            .headers
            .contains_key(HeaderName::from_static("origin")));
    }

    #[test]
    fn test_parse_cookie_string() {
        let cookies = parse_cookie_string("name=value; session=abc123; empty=");
        assert_eq!(cookies.len(), 3);
        assert_eq!(cookies[0], ("name".to_string(), "value".to_string()));
        assert_eq!(cookies[1], ("session".to_string(), "abc123".to_string()));
        assert_eq!(cookies[2], ("empty".to_string(), "".to_string()));
    }

    #[test]
    fn test_parse_cookie_string_empty() {
        let cookies = parse_cookie_string("");
        assert!(cookies.is_empty());
    }

    #[test]
    fn test_format_cookie_header() {
        let cookies = vec![
            ("name".to_string(), "value".to_string()),
            ("session".to_string(), "abc123".to_string()),
        ];
        let header = format_cookie_header(&cookies);
        assert_eq!(header, "name=value; session=abc123");
    }

    #[test]
    fn test_format_cookie_header_empty() {
        let cookies: Vec<(String, String)> = vec![];
        let header = format_cookie_header(&cookies);
        assert_eq!(header, "");
    }

    #[test]
    fn test_collect_all_cookies_single_header() {
        let mut headers = hyper::HeaderMap::new();
        headers.insert(
            hyper::header::COOKIE,
            HeaderValue::from_static("session=abc; user=test"),
        );
        let cookies = collect_all_cookies_from_headers(&headers);
        assert_eq!(cookies.get("session").unwrap(), "abc");
        assert_eq!(cookies.get("user").unwrap(), "test");
    }

    #[test]
    fn test_collect_all_cookies_multiple_headers() {
        let mut headers = hyper::HeaderMap::new();
        headers.append(
            hyper::header::COOKIE,
            HeaderValue::from_static("monitor_web_id=123456"),
        );
        headers.append(
            hyper::header::COOKIE,
            HeaderValue::from_static("session_flag=1"),
        );
        headers.append(
            hyper::header::COOKIE,
            HeaderValue::from_static("x-token=abc-def-ghi"),
        );
        let cookies = collect_all_cookies_from_headers(&headers);
        assert_eq!(cookies.len(), 3);
        assert_eq!(cookies.get("monitor_web_id").unwrap(), "123456");
        assert_eq!(cookies.get("session_flag").unwrap(), "1");
        assert_eq!(cookies.get("x-token").unwrap(), "abc-def-ghi");
    }

    #[test]
    fn test_collect_all_cookies_empty() {
        let headers = hyper::HeaderMap::new();
        let cookies = collect_all_cookies_from_headers(&headers);
        assert!(cookies.is_empty());
    }

    #[test]
    fn test_merge_cookie_header_values_single() {
        let mut headers = hyper::HeaderMap::new();
        headers.insert(
            hyper::header::COOKIE,
            HeaderValue::from_static("session=abc; user=test"),
        );
        let merged = merge_cookie_header_values(&headers);
        assert_eq!(merged, "session=abc; user=test");
    }

    #[test]
    fn test_merge_cookie_header_values_multiple() {
        let mut headers = hyper::HeaderMap::new();
        headers.append(
            hyper::header::COOKIE,
            HeaderValue::from_static("monitor_web_id=123456"),
        );
        headers.append(
            hyper::header::COOKIE,
            HeaderValue::from_static("session_flag=1"),
        );
        headers.append(
            hyper::header::COOKIE,
            HeaderValue::from_static("x-token=abc-def-ghi"),
        );
        let merged = merge_cookie_header_values(&headers);
        assert!(merged.contains("monitor_web_id=123456"));
        assert!(merged.contains("session_flag=1"));
        assert!(merged.contains("x-token=abc-def-ghi"));
        assert_eq!(merged.matches("; ").count(), 2);
    }

    #[test]
    fn test_merge_cookie_header_values_empty() {
        let headers = hyper::HeaderMap::new();
        let merged = merge_cookie_header_values(&headers);
        assert!(merged.is_empty());
    }

    #[test]
    fn test_collect_all_cookies_with_jwt() {
        let jwt = "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.eyJleHAiOjE3NzYzMjk0NjksImlhdCI6MTc3NTcyNDY2OX0.JT4aDh4cRQnz0sEccvclUk1nXm2na3ZJarI3UJlN_FPg";
        let mut headers = hyper::HeaderMap::new();
        headers.append(
            hyper::header::COOKIE,
            HeaderValue::from_static("people-lang=zh"),
        );
        headers.append(
            hyper::header::COOKIE,
            HeaderValue::from_str(&format!("bd_sso_3b6da9={}", jwt)).unwrap(),
        );
        headers.append(
            hyper::header::COOKIE,
            HeaderValue::from_static("x-token=16289f27-f342-4f5b-b95a-e5291cfe1577"),
        );
        let cookies = collect_all_cookies_from_headers(&headers);
        assert_eq!(cookies.len(), 3);
        assert_eq!(cookies.get("people-lang").unwrap(), "zh");
        assert_eq!(cookies.get("bd_sso_3b6da9").unwrap(), jwt);
        assert_eq!(
            cookies.get("x-token").unwrap(),
            "16289f27-f342-4f5b-b95a-e5291cfe1577"
        );
    }

    #[test]
    fn test_apply_req_cookies_merge_multiple_cookie_headers() {
        let mut parts = create_test_parts();
        let ctx = RequestContext::new();
        parts.headers.append(
            hyper::header::COOKIE,
            HeaderValue::from_static("session=abc"),
        );
        parts
            .headers
            .append(hyper::header::COOKIE, HeaderValue::from_static("token=xyz"));

        let mut rules = ResolvedRules::default();
        rules
            .req_cookies
            .push(("added".to_string(), "new".to_string()));

        apply_req_rules(&mut parts, &rules, false, &ctx);

        let cookie = parts
            .headers
            .get(hyper::header::COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(cookie.contains("session=abc"));
        assert!(cookie.contains("token=xyz"));
        assert!(cookie.contains("added=new"));
        assert_eq!(
            parts.headers.get_all(hyper::header::COOKIE).iter().count(),
            1
        );
    }

    #[test]
    fn test_apply_forwarded_for_new() {
        let mut parts = create_test_parts();
        let rules = ResolvedRules {
            forwarded_for: Some("1.2.3.4".to_string()),
            ..Default::default()
        };
        let ctx = RequestContext::new();

        apply_req_rules(&mut parts, &rules, false, &ctx);

        assert_eq!(
            parts
                .headers
                .get("x-forwarded-for")
                .unwrap()
                .to_str()
                .unwrap(),
            "1.2.3.4"
        );
    }

    #[test]
    fn test_apply_forwarded_for_chain() {
        let mut parts = create_test_parts();
        let ctx = RequestContext::new();
        parts.headers.insert(
            "x-forwarded-for".parse::<HeaderName>().unwrap(),
            HeaderValue::from_static("10.0.0.1"),
        );

        let rules = ResolvedRules {
            forwarded_for: Some("1.2.3.4".to_string()),
            ..Default::default()
        };

        apply_req_rules(&mut parts, &rules, false, &ctx);

        assert_eq!(
            parts
                .headers
                .get("x-forwarded-for")
                .unwrap()
                .to_str()
                .unwrap(),
            "10.0.0.1, 1.2.3.4"
        );
    }

    #[test]
    fn coverage_90_full_verbose_request_pipeline_covers_remaining_rules() {
        let request = hyper::Request::builder()
            .method("POST")
            .uri("http://example.test/")
            .header("x-delete", "old")
            .header("x-replace", "before value")
            .header(hyper::header::AUTHORIZATION, "Bearer old")
            .header(hyper::header::CONTENT_TYPE, "text/plain; charset=ascii")
            .header(hyper::header::COOKIE, "remove=1; keep=old")
            .body(())
            .unwrap();
        let (mut parts, _) = request.into_parts();
        let rules = ResolvedRules {
            delete_req_headers: vec!["x-delete".into(), "invalid header".into()],
            req_headers: vec![
                ("x-new".into(), "new".into()),
                ("bad header".into(), "x".into()),
            ],
            req_cookies: vec![
                ("keep".into(), "new;quoted\"".into()),
                ("space name".into(), "ok".into()),
            ],
            req_del_cookies: vec!["remove".into()],
            method: Some("PATCH".into()),
            ua: Some("coverage-agent".into()),
            referer: Some("https://referer.test/".into()),
            auth: Some("user:pass".into()),
            header_replace: vec![crate::server::HeaderReplaceRule {
                target: HeaderReplaceTarget::Request,
                header_name: "x-replace".into(),
                pattern: "before".into(),
                replacement: "after".into(),
            }],
            req_type: Some("json".into()),
            req_charset: Some("utf-8".into()),
            forwarded_for: Some("203.0.113.8".into()),
            req_cors: CorsConfig {
                enabled: true,
                origin: Some("https://origin.test".into()),
                methods: Some("PATCH".into()),
                headers: Some("x-new".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        apply_req_rules(&mut parts, &rules, true, &RequestContext::new());
        assert_eq!(parts.method, hyper::Method::PATCH);
        assert!(!parts.headers.contains_key("x-delete"));
        assert_eq!(parts.headers["x-replace"], "after value");
        assert_eq!(
            parts.headers[hyper::header::CONTENT_TYPE],
            "application/json; charset=utf-8"
        );
        assert!(parts.headers[hyper::header::COOKIE]
            .to_str()
            .unwrap()
            .contains("space%20name=ok"));
        assert_eq!(parts.headers["origin"], "https://origin.test");
    }

    #[test]
    fn coverage_90_cookie_escape_and_wildcard_cors_paths() {
        assert_eq!(escape_cookie_name("a b"), "a%20b");
        assert_eq!(escape_cookie_value("a,b;c\\d\n"), "a%2Cb%3Bc%5Cd%0A");
        let request = hyper::Request::builder().uri("/").body(()).unwrap();
        let (mut parts, _) = request.into_parts();
        let rules = ResolvedRules {
            req_cors: CorsConfig {
                enabled: true,
                origin: Some("*".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        apply_req_rules(&mut parts, &rules, true, &RequestContext::new());
        assert_eq!(parts.headers["origin"], "*");
    }
}
