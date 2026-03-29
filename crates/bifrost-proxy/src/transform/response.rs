use hyper::header::{HeaderName, HeaderValue};
use hyper::http::response::Parts;
use hyper::StatusCode;
use tracing::info;

use crate::server::{CorsConfig, HeaderReplaceTarget, ResolvedRules};
use crate::utils::logging::RequestContext;

pub fn apply_res_rules(
    parts: &mut Parts,
    rules: &ResolvedRules,
    verbose_logging: bool,
    ctx: &RequestContext,
) {
    apply_res_status(parts, rules, verbose_logging, ctx);
    apply_res_delete_headers(parts, rules, verbose_logging, ctx);
    apply_res_headers(parts, rules, verbose_logging, ctx);
    apply_res_cookies(parts, rules, verbose_logging, ctx);
    apply_res_attachment(parts, rules, verbose_logging, ctx);
    apply_res_type(parts, rules, verbose_logging, ctx);
    apply_res_charset(parts, rules, verbose_logging, ctx);
    apply_res_cache(parts, rules, verbose_logging, ctx);
    apply_res_header_replace(parts, rules, verbose_logging, ctx);

    if rules.res_cors.is_enabled() {
        apply_res_cors(parts, &rules.res_cors, ctx, verbose_logging);
    }

    apply_res_trailers(parts, rules, verbose_logging, ctx);
}

fn apply_res_delete_headers(
    parts: &mut Parts,
    rules: &ResolvedRules,
    verbose_logging: bool,
    ctx: &RequestContext,
) {
    for header_name in &rules.delete_res_headers {
        if let Ok(name) = header_name.parse::<HeaderName>() {
            let old_value = parts
                .headers
                .get(&name)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());

            if parts.headers.remove(&name).is_some() && verbose_logging {
                info!(
                    "[{}] [RES_DELETE_HEADER] {} : \"{}\" -> (deleted)",
                    ctx.id_str(),
                    header_name,
                    old_value.unwrap_or_default()
                );
            }
        }
    }
}

fn apply_res_header_replace(
    parts: &mut Parts,
    rules: &ResolvedRules,
    verbose_logging: bool,
    ctx: &RequestContext,
) {
    for rule in &rules.header_replace {
        if rule.target != HeaderReplaceTarget::Response {
            continue;
        }

        if let Ok(header_name) = rule.header_name.parse::<HeaderName>() {
            if let Some(current_value) = parts.headers.get(&header_name) {
                if let Ok(current_str) = current_value.to_str() {
                    let new_value = current_str.replace(&rule.pattern, &rule.replacement);

                    if let Ok(new_header_value) = new_value.parse::<HeaderValue>() {
                        if verbose_logging {
                            info!(
                                "[{}] [RES_HEADER_REPLACE] {} : \"{}\" -> \"{}\"",
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

fn apply_res_status(
    parts: &mut Parts,
    rules: &ResolvedRules,
    verbose_logging: bool,
    ctx: &RequestContext,
) {
    let target_status = rules.replace_status.or(rules.status_code);
    if let Some(status_code) = target_status {
        if let Ok(status) = StatusCode::from_u16(status_code) {
            if verbose_logging {
                info!(
                    "[{}] [RES_STATUS] {} -> {}",
                    ctx.id_str(),
                    parts.status.as_u16(),
                    status_code
                );
            }
            parts.status = status;
        }
    }
}

fn apply_res_headers(
    parts: &mut Parts,
    rules: &ResolvedRules,
    verbose_logging: bool,
    ctx: &RequestContext,
) {
    for (name, value) in &rules.res_headers {
        let processed_value = process_template_value(value, ctx);
        if let (Ok(header_name), Ok(header_value)) = (
            name.parse::<HeaderName>(),
            processed_value.parse::<HeaderValue>(),
        ) {
            if verbose_logging {
                let old_value = parts
                    .headers
                    .get(&header_name)
                    .and_then(|v| v.to_str().ok())
                    .map(|s| format!("\"{}\"", s))
                    .unwrap_or_else(|| "(none)".to_string());
                info!(
                    "[{}] [RES_HEADER] {} : {} -> \"{}\"",
                    ctx.id_str(),
                    name,
                    old_value,
                    processed_value
                );
            }
            parts.headers.insert(header_name, header_value);
        }
    }
}

fn process_template_value(value: &str, ctx: &RequestContext) -> String {
    use regex::Regex;
    use std::sync::LazyLock;

    static RE_REQ_HEADERS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\$\{reqHeaders\.([^}]+)\}").unwrap());
    static RE_REQ_COOKIES: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\$\{reqCookies\.([^}]+)\}").unwrap());
    static RE_QUERY: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\$\{query\.([^}]+)\}").unwrap());
    static RE_RANDOM_INT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\$\{randomInt\((\d+)(?:-(\d+))?\)\}").unwrap());
    static RE_ENV: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\$\{env\.([^}]+)\}").unwrap());

    let mut result = value.to_string();

    result = result.replace("$${", "\x00ESCAPED_DOLLAR\x00");

    result = RE_REQ_HEADERS
        .replace_all(&result, |caps: &regex::Captures| {
            let header_name = &caps[1];
            ctx.req_headers
                .get(&header_name.to_lowercase())
                .cloned()
                .unwrap_or_default()
        })
        .to_string();

    result = RE_REQ_COOKIES
        .replace_all(&result, |caps: &regex::Captures| {
            let cookie_name = &caps[1];
            ctx.req_cookies
                .get(cookie_name)
                .cloned()
                .unwrap_or_default()
        })
        .to_string();

    result = RE_QUERY
        .replace_all(&result, |caps: &regex::Captures| {
            let param_name = &caps[1];
            ctx.query_params
                .get(param_name)
                .cloned()
                .unwrap_or_default()
        })
        .to_string();

    result = RE_RANDOM_INT
        .replace_all(&result, |caps: &regex::Captures| {
            let first: u64 = caps[1].parse().unwrap_or(100);
            if let Some(second) = caps.get(2) {
                let min = first;
                let max: u64 = second.as_str().parse().unwrap_or(100);
                if max > min {
                    (rand::random::<u64>() % (max - min + 1) + min).to_string()
                } else {
                    min.to_string()
                }
            } else {
                (rand::random::<u64>() % (first + 1)).to_string()
            }
        })
        .to_string();

    result = RE_ENV
        .replace_all(&result, |caps: &regex::Captures| {
            let var_name = &caps[1];
            std::env::var(var_name)
                .or_else(|_| match var_name {
                    "USER" => std::env::var("USERNAME"),
                    "USERNAME" => std::env::var("USER"),
                    "HOME" => std::env::var("USERPROFILE"),
                    "USERPROFILE" => std::env::var("HOME"),
                    _ => Err(std::env::VarError::NotPresent),
                })
                .unwrap_or_default()
        })
        .to_string();

    let url_port = url::Url::parse(&ctx.url)
        .ok()
        .and_then(|u| u.port())
        .map(|p| p.to_string())
        .unwrap_or_default();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_default();
    let random: u64 = rand::random();

    result = result
        .replace("${url}", &ctx.url)
        .replace("${method}", &ctx.method)
        .replace("${host}", &ctx.host)
        .replace("${url.host}", &ctx.host)
        .replace("${url.hostname}", &ctx.host)
        .replace("${url.port}", &url_port)
        .replace("${pathname}", &ctx.pathname)
        .replace("${path}", &ctx.pathname)
        .replace("${url.path}", &ctx.pathname)
        .replace("${url.pathname}", &ctx.pathname)
        .replace("${search}", &ctx.search)
        .replace("${query}", &ctx.search)
        .replace("${url.search}", &ctx.search)
        .replace("${clientIp}", &ctx.client_ip)
        .replace("${reqId}", ctx.id_str())
        .replace("${now}", &now)
        .replace("${timestamp}", &now)
        .replace("${randomUUID}", &uuid::Uuid::new_v4().to_string())
        .replace("${random}", &random.to_string())
        .replace("${version}", env!("CARGO_PKG_VERSION"))
        .replace("${port}", &ctx.port.to_string());

    result = result.replace("\x00ESCAPED_DOLLAR\x00", "${");

    result
}

fn apply_res_cookies(
    parts: &mut Parts,
    rules: &ResolvedRules,
    verbose_logging: bool,
    ctx: &RequestContext,
) {
    for del_name in &rules.res_del_cookies {
        let prefix = format!("{}=", del_name);
        let cookie_str = format!(
            "{}=; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT",
            del_name
        );
        if let Ok(header_value) = cookie_str.parse::<HeaderValue>() {
            if verbose_logging {
                info!("[{}] [RES_COOKIE_DEL] {} : deleted", ctx.id_str(), del_name);
            }
            let mut to_remove = Vec::new();
            for (idx, val) in parts
                .headers
                .get_all(hyper::header::SET_COOKIE)
                .iter()
                .enumerate()
            {
                if let Ok(s) = val.to_str() {
                    if s.starts_with(&prefix) {
                        to_remove.push(idx);
                    }
                }
            }
            parts
                .headers
                .append(hyper::header::SET_COOKIE, header_value);
        }
    }

    for (name, cookie_value) in &rules.res_cookies {
        let cookie_str = cookie_value.to_set_cookie_string(name);
        if let Ok(header_value) = cookie_str.parse::<HeaderValue>() {
            if verbose_logging {
                info!(
                    "[{}] [RES_COOKIE] {} = \"{}\" ({})",
                    ctx.id_str(),
                    name,
                    cookie_value.value,
                    cookie_str
                );
            }
            parts
                .headers
                .append(hyper::header::SET_COOKIE, header_value);
        }
    }
}

fn apply_res_attachment(
    parts: &mut Parts,
    rules: &ResolvedRules,
    verbose_logging: bool,
    ctx: &RequestContext,
) {
    if let Some(ref attachment) = rules.attachment {
        let filename = if attachment.is_empty() {
            extract_filename_from_url(&ctx.pathname)
        } else {
            attachment.clone()
        };

        let filename = encode_content_disposition_filename(&filename);
        let header_value = format!("attachment; filename=\"{}\"", filename);

        if let Ok(value) = header_value.parse::<HeaderValue>() {
            if verbose_logging {
                info!(
                    "[{}] [RES_ATTACHMENT] Content-Disposition: {}",
                    ctx.id_str(),
                    header_value
                );
            }
            parts
                .headers
                .insert(hyper::header::CONTENT_DISPOSITION, value);
        }
    }
}

fn apply_res_cache(
    parts: &mut Parts,
    rules: &ResolvedRules,
    verbose_logging: bool,
    ctx: &RequestContext,
) {
    if let Some(ref cache_value) = rules.cache {
        let cache_control = if let Ok(seconds) = cache_value.parse::<u64>() {
            if seconds == 0 {
                "no-cache, no-store, must-revalidate".to_string()
            } else {
                format!("max-age={}", seconds)
            }
        } else {
            cache_value.clone()
        };

        if let Ok(value) = cache_control.parse::<HeaderValue>() {
            if verbose_logging {
                let old_value = parts
                    .headers
                    .get(hyper::header::CACHE_CONTROL)
                    .and_then(|v| v.to_str().ok())
                    .map(|s| format!("\"{}\"", s))
                    .unwrap_or_else(|| "(none)".to_string());
                info!(
                    "[{}] [RES_CACHE] Cache-Control : {} -> \"{}\"",
                    ctx.id_str(),
                    old_value,
                    cache_control
                );
            }
            parts.headers.insert(hyper::header::CACHE_CONTROL, value);
        }
    }
}

pub(crate) fn expand_content_type_shortcut(input: &str) -> &str {
    match input.to_lowercase().as_str() {
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "javascript" | "mjs" => "application/javascript",
        "json" => "application/json",
        "xml" => "application/xml",
        "txt" | "text" => "text/plain",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "eot" => "application/vnd.ms-fontobject",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" | "gzip" => "application/gzip",
        "mp3" => "audio/mpeg",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "wasm" => "application/wasm",
        "form" => "application/x-www-form-urlencoded",
        "multipart" => "multipart/form-data",
        _ => input,
    }
}

fn apply_res_type(
    parts: &mut Parts,
    rules: &ResolvedRules,
    verbose_logging: bool,
    ctx: &RequestContext,
) {
    if let Some(ref content_type) = rules.res_type {
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
                    "[{}] [RES_TYPE] Content-Type : {} -> \"{}\"",
                    ctx.id_str(),
                    old_value,
                    expanded
                );
            }
            parts.headers.insert(hyper::header::CONTENT_TYPE, value);
        }
    }
}

fn apply_res_charset(
    parts: &mut Parts,
    rules: &ResolvedRules,
    verbose_logging: bool,
    ctx: &RequestContext,
) {
    if let Some(ref charset) = rules.res_charset {
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
                    "[{}] [RES_CHARSET] Content-Type : \"{}\" -> \"{}\"",
                    ctx.id_str(),
                    current_ct,
                    new_ct
                );
            }
            parts.headers.insert(hyper::header::CONTENT_TYPE, value);
        }
    }
}

fn extract_filename_from_url(pathname: &str) -> String {
    pathname
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty() && s.contains('.'))
        .unwrap_or("download")
        .to_string()
}

fn encode_content_disposition_filename(filename: &str) -> String {
    filename
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c.to_string()
            } else if c.is_ascii() {
                format!("%{:02X}", c as u32)
            } else {
                format!("%{:02X}", c as u32 & 0xFF)
            }
        })
        .collect()
}

fn apply_res_trailers(
    parts: &mut Parts,
    rules: &ResolvedRules,
    verbose_logging: bool,
    ctx: &RequestContext,
) {
    if rules.trailers.is_empty() {
        return;
    }

    let mut trailer_names: Vec<String> = Vec::new();

    for (name, value) in &rules.trailers {
        if let (Ok(_header_name), Ok(_header_value)) =
            (name.parse::<HeaderName>(), value.parse::<HeaderValue>())
        {
            trailer_names.push(name.clone());
            if verbose_logging {
                info!("[{}] [RES_TRAILER] {} : \"{}\"", ctx.id_str(), name, value);
            }
        }
    }

    if !trailer_names.is_empty() {
        parts.headers.remove(hyper::header::CONTENT_LENGTH);
        let trailer_header = trailer_names.join(", ");
        if let Ok(value) = trailer_header.parse::<HeaderValue>() {
            parts.headers.insert(hyper::header::TRAILER, value);
            if verbose_logging {
                info!(
                    "[{}] [RES_TRAILER] Trailer header: {}",
                    ctx.id_str(),
                    trailer_header
                );
            }
        }
    }
}

fn apply_res_cors(
    parts: &mut Parts,
    cors: &CorsConfig,
    ctx: &RequestContext,
    verbose_logging: bool,
) {
    if verbose_logging {
        info!(
            "[{}] [RES_CORS] enabled with config: {:?}",
            ctx.id_str(),
            cors
        );
    }

    let origin = cors.origin.as_deref().unwrap_or("*");
    if let Ok(header_value) = origin.parse::<HeaderValue>() {
        parts
            .headers
            .insert(hyper::header::ACCESS_CONTROL_ALLOW_ORIGIN, header_value);
    }

    let methods = cors
        .methods
        .as_deref()
        .unwrap_or("GET, POST, PUT, DELETE, OPTIONS, PATCH");
    if let Ok(header_value) = methods.parse::<HeaderValue>() {
        parts
            .headers
            .insert(hyper::header::ACCESS_CONTROL_ALLOW_METHODS, header_value);
    }

    let headers = cors.headers.as_deref().unwrap_or("*");
    if let Ok(header_value) = headers.parse::<HeaderValue>() {
        parts
            .headers
            .insert(hyper::header::ACCESS_CONTROL_ALLOW_HEADERS, header_value);
    }

    let credentials = cors.credentials.unwrap_or(true);
    if credentials {
        parts.headers.insert(
            hyper::header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
            HeaderValue::from_static("true"),
        );
    }

    let expose_headers = cors.expose_headers.as_deref().unwrap_or("*");
    if let Ok(header_value) = expose_headers.parse::<HeaderValue>() {
        parts
            .headers
            .insert(hyper::header::ACCESS_CONTROL_EXPOSE_HEADERS, header_value);
    }

    if let Some(max_age) = cors.max_age {
        if let Ok(header_value) = max_age.to_string().parse::<HeaderValue>() {
            parts
                .headers
                .insert(hyper::header::ACCESS_CONTROL_MAX_AGE, header_value);
        }
    }
}

pub fn parse_set_cookie(cookie_str: &str) -> Option<(String, String, SetCookieOptions)> {
    let mut parts = cookie_str.split(';');
    let name_value = parts.next()?;
    let mut nv_parts = name_value.splitn(2, '=');
    let name = nv_parts.next()?.trim().to_string();
    let value = nv_parts.next().unwrap_or("").trim().to_string();

    if name.is_empty() {
        return None;
    }

    let mut options = SetCookieOptions::default();

    for part in parts {
        let part = part.trim();
        let lower = part.to_lowercase();

        if lower.starts_with("path=") {
            options.path = Some(part[5..].to_string());
        } else if lower.starts_with("domain=") {
            options.domain = Some(part[7..].to_string());
        } else if lower.starts_with("max-age=") {
            if let Ok(max_age) = part[8..].parse() {
                options.max_age = Some(max_age);
            }
        } else if lower.starts_with("expires=") {
            options.expires = Some(part[8..].to_string());
        } else if lower == "secure" {
            options.secure = true;
        } else if lower == "httponly" {
            options.http_only = true;
        } else if lower.starts_with("samesite=") {
            options.same_site = Some(part[9..].to_string());
        }
    }

    Some((name, value, options))
}

#[derive(Debug, Clone, Default)]
pub struct SetCookieOptions {
    pub path: Option<String>,
    pub domain: Option<String>,
    pub max_age: Option<i64>,
    pub expires: Option<String>,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: Option<String>,
}

impl SetCookieOptions {
    pub fn to_cookie_string(&self, name: &str, value: &str) -> String {
        let mut cookie = format!("{}={}", name, value);

        if let Some(ref path) = self.path {
            cookie.push_str(&format!("; Path={}", path));
        }
        if let Some(ref domain) = self.domain {
            cookie.push_str(&format!("; Domain={}", domain));
        }
        if let Some(max_age) = self.max_age {
            cookie.push_str(&format!("; Max-Age={}", max_age));
        }
        if let Some(ref expires) = self.expires {
            cookie.push_str(&format!("; Expires={}", expires));
        }
        if self.secure {
            cookie.push_str("; Secure");
        }
        if self.http_only {
            cookie.push_str("; HttpOnly");
        }
        if let Some(ref same_site) = self.same_site {
            cookie.push_str(&format!("; SameSite={}", same_site));
        }

        cookie
    }
}

pub fn format_set_cookie(name: &str, value: &str, options: &SetCookieOptions) -> String {
    options.to_cookie_string(name, value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::Response;

    fn create_test_parts() -> Parts {
        let (parts, _) = Response::builder()
            .status(200)
            .body(())
            .unwrap()
            .into_parts();
        parts
    }

    #[test]
    fn test_apply_res_status() {
        let mut parts = create_test_parts();
        let mut rules = ResolvedRules::default();
        let ctx = RequestContext::new();
        rules.status_code = Some(404);

        apply_res_rules(&mut parts, &rules, false, &ctx);

        assert_eq!(parts.status, StatusCode::NOT_FOUND);
    }

    #[test]
    fn test_apply_res_headers() {
        let mut parts = create_test_parts();
        let mut rules = ResolvedRules::default();
        let ctx = RequestContext::new();
        rules
            .res_headers
            .push(("X-Custom-Header".to_string(), "custom-value".to_string()));
        rules
            .res_headers
            .push(("Content-Type".to_string(), "application/json".to_string()));

        apply_res_rules(&mut parts, &rules, false, &ctx);

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
            parts.headers.get("Content-Type").unwrap().to_str().unwrap(),
            "application/json"
        );
    }

    #[test]
    fn test_apply_res_cookies() {
        let mut parts = create_test_parts();
        let mut rules = ResolvedRules::default();
        let ctx = RequestContext::new();
        rules.res_cookies.push((
            "session".to_string(),
            crate::server::ResCookieValue::simple("abc123".to_string()),
        ));
        rules.res_cookies.push((
            "user".to_string(),
            crate::server::ResCookieValue::simple("test".to_string()),
        ));

        apply_res_rules(&mut parts, &rules, false, &ctx);

        let cookies: Vec<_> = parts
            .headers
            .get_all(hyper::header::SET_COOKIE)
            .iter()
            .collect();
        assert_eq!(cookies.len(), 2);
    }

    #[test]
    fn test_apply_res_cors() {
        let mut parts = create_test_parts();
        let mut rules = ResolvedRules::default();
        let ctx = RequestContext::new();
        rules.res_cors = CorsConfig::enable_all();

        apply_res_rules(&mut parts, &rules, false, &ctx);

        assert!(parts
            .headers
            .contains_key(hyper::header::ACCESS_CONTROL_ALLOW_ORIGIN));
        assert!(parts
            .headers
            .contains_key(hyper::header::ACCESS_CONTROL_ALLOW_METHODS));
        assert!(parts
            .headers
            .contains_key(hyper::header::ACCESS_CONTROL_ALLOW_HEADERS));
        assert!(parts
            .headers
            .contains_key(hyper::header::ACCESS_CONTROL_EXPOSE_HEADERS));
    }

    #[test]
    fn test_parse_set_cookie_simple() {
        let (name, value, options) = parse_set_cookie("session=abc123").unwrap();
        assert_eq!(name, "session");
        assert_eq!(value, "abc123");
        assert!(options.path.is_none());
    }

    #[test]
    fn test_parse_set_cookie_with_options() {
        let cookie =
            "session=abc123; Path=/; Domain=example.com; Secure; HttpOnly; SameSite=Strict";
        let (name, value, options) = parse_set_cookie(cookie).unwrap();
        assert_eq!(name, "session");
        assert_eq!(value, "abc123");
        assert_eq!(options.path, Some("/".to_string()));
        assert_eq!(options.domain, Some("example.com".to_string()));
        assert!(options.secure);
        assert!(options.http_only);
        assert_eq!(options.same_site, Some("Strict".to_string()));
    }

    #[test]
    fn test_parse_set_cookie_with_max_age() {
        let cookie = "session=abc123; Max-Age=3600";
        let (_, _, options) = parse_set_cookie(cookie).unwrap();
        assert_eq!(options.max_age, Some(3600));
    }

    #[test]
    fn test_parse_set_cookie_with_expires() {
        let cookie = "session=abc123; Expires=Wed, 09 Jun 2021 10:18:14 GMT";
        let (_, _, options) = parse_set_cookie(cookie).unwrap();
        assert_eq!(
            options.expires,
            Some("Wed, 09 Jun 2021 10:18:14 GMT".to_string())
        );
    }

    #[test]
    fn test_parse_set_cookie_empty_value() {
        let (name, value, _) = parse_set_cookie("session=").unwrap();
        assert_eq!(name, "session");
        assert_eq!(value, "");
    }

    #[test]
    fn test_parse_set_cookie_invalid() {
        let result = parse_set_cookie("=value");
        assert!(result.is_none());
    }

    #[test]
    fn test_set_cookie_options_to_string() {
        let options = SetCookieOptions {
            path: Some("/api".to_string()),
            domain: Some("example.com".to_string()),
            max_age: Some(3600),
            expires: None,
            secure: true,
            http_only: true,
            same_site: Some("Lax".to_string()),
        };

        let cookie_str = options.to_cookie_string("session", "abc123");
        assert!(cookie_str.contains("session=abc123"));
        assert!(cookie_str.contains("Path=/api"));
        assert!(cookie_str.contains("Domain=example.com"));
        assert!(cookie_str.contains("Max-Age=3600"));
        assert!(cookie_str.contains("Secure"));
        assert!(cookie_str.contains("HttpOnly"));
        assert!(cookie_str.contains("SameSite=Lax"));
    }

    #[test]
    fn test_set_cookie_options_default() {
        let options = SetCookieOptions::default();
        assert!(options.path.is_none());
        assert!(options.domain.is_none());
        assert!(options.max_age.is_none());
        assert!(!options.secure);
        assert!(!options.http_only);
    }

    #[test]
    fn test_format_set_cookie() {
        let options = SetCookieOptions {
            path: Some("/".to_string()),
            secure: true,
            ..Default::default()
        };

        let cookie = format_set_cookie("test", "value", &options);
        assert_eq!(cookie, "test=value; Path=/; Secure");
    }
}
