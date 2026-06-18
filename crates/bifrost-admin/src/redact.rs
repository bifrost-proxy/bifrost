//! Secrets redaction layer for traffic outputs.
//!
//! Default-on protection for `bifrost traffic get` (and other CLI surfaces that
//! opt-in) so that headers and JSON bodies do not leak `Authorization`,
//! `Cookie`, JWTs, API keys, etc. into terminals, log files or AI assistant
//! transcripts.
//!
//! Two escape hatches:
//!   * `--show-secrets` (per-invocation, prints original value).
//!   * `BIFROST_REDACT=off` env (kill switch for upgrade periods).
//!
//! See `design/redact.md` for the full rule table and rationale.

use base64::Engine;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{Map, Value};

/// Options controlling redaction behaviour.
#[derive(Clone, Debug)]
pub struct RedactOptions {
    /// When true, return original values untouched. Equivalent to disabling
    /// redaction. The CLI sets this from `--show-secrets`.
    pub show_secrets: bool,
    /// When true, callers want a short structured summary of detected auth
    /// information (presence of JWT/Cookie, JWT subject, expiry, host) **in
    /// addition** to the redacted payload. Redaction is still applied.
    pub extract_auth_summary: bool,
}

impl Default for RedactOptions {
    fn default() -> Self {
        default_options()
    }
}

/// Default options: redact aggressively, no auth summary.
pub fn default_options() -> RedactOptions {
    RedactOptions {
        show_secrets: false,
        extract_auth_summary: false,
    }
}

/// Structured summary of authentication-related headers. Useful for human
/// triage without revealing the raw secret material.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuthSummary {
    pub has_jwt: bool,
    pub has_cookie: bool,
    pub jwt_user_id: Option<String>,
    pub jwt_exp_unix: Option<i64>,
    pub cookie_names: Vec<String>,
    pub host: Option<String>,
}

const REDACTED_HEADER_NAMES: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
];

static SENSITIVE_HEADER_RE: Lazy<Regex> = Lazy::new(|| {
    // Matches names like `token`, `x-api-key`, `x-some-secret`, `jwt`,
    // `session-id`, `csrf`, etc. (case insensitive).
    Regex::new(r"(?i)^(x-)?(.*-)?(token|secret|jwt|api[-_]?key|session[-_]?id|csrf)$").unwrap()
});

static SENSITIVE_JSON_KEY_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(password|token|secret|jwt|api_?key|access_?key|sk-|refresh_?token)").unwrap()
});

/// Whether a header name is considered sensitive and its value must be hidden.
fn is_sensitive_header_name(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    if REDACTED_HEADER_NAMES.iter().any(|n| *n == lower) {
        return true;
    }
    if lower.starts_with("x-tt-passport-") {
        return true;
    }
    SENSITIVE_HEADER_RE.is_match(&lower)
}

fn redacted_marker(value: &str) -> String {
    format!("<REDACTED:len={}>", value.chars().count())
}

/// Redact a list of (name, value) headers in-place.
pub fn redact_headers(headers: &mut [(String, String)], opts: &RedactOptions) {
    if opts.show_secrets {
        return;
    }
    for (name, value) in headers.iter_mut() {
        if is_sensitive_header_name(name) {
            *value = redacted_marker(value);
        }
    }
}

/// Redact a JSON body provided as text. For non-JSON content the body is
/// returned unchanged (avoiding heuristic damage to other formats).
pub fn redact_body_text(content_type: Option<&str>, body: &str, opts: &RedactOptions) -> String {
    if opts.show_secrets {
        return body.to_string();
    }
    if !content_type_is_json(content_type) {
        return body.to_string();
    }
    match serde_json::from_str::<Value>(body) {
        Ok(mut value) => {
            redact_json_in_place(&mut value);
            serde_json::to_string(&value).unwrap_or_else(|_| body.to_string())
        }
        Err(_) => body.to_string(),
    }
}

/// Redact a JSON body provided as bytes. Returns redacted bytes; non-JSON
/// content passes through.
pub fn redact_body_bytes(content_type: Option<&str>, body: &[u8], opts: &RedactOptions) -> Vec<u8> {
    if opts.show_secrets {
        return body.to_vec();
    }
    if !content_type_is_json(content_type) {
        return body.to_vec();
    }
    match std::str::from_utf8(body) {
        Ok(text) => redact_body_text(content_type, text, opts).into_bytes(),
        Err(_) => body.to_vec(),
    }
}

fn content_type_is_json(content_type: Option<&str>) -> bool {
    content_type
        .map(|ct| ct.to_ascii_lowercase().contains("json"))
        .unwrap_or(false)
}

fn redact_json_in_place(value: &mut Value) {
    match value {
        Value::Object(map) => {
            redact_json_object(map);
        }
        Value::Array(items) => {
            for item in items {
                redact_json_in_place(item);
            }
        }
        _ => {}
    }
}

fn redact_json_object(map: &mut Map<String, Value>) {
    for (key, val) in map.iter_mut() {
        if SENSITIVE_JSON_KEY_RE.is_match(&key.to_ascii_lowercase()) {
            if let Some(s) = val.as_str() {
                *val = Value::String(redacted_marker(s));
                continue;
            }
            if val.is_number() || val.is_boolean() {
                let original = val.to_string();
                *val = Value::String(redacted_marker(&original));
                continue;
            }
            // For nested objects/arrays under a sensitive key, still recurse:
            redact_json_in_place(val);
        } else {
            redact_json_in_place(val);
        }
    }
}

/// Extract a high-signal auth summary from a header list (without leaking the
/// raw secret values). Headers can be provided in their original form.
pub fn auth_summary_from_headers(headers: &[(String, String)]) -> AuthSummary {
    let mut summary = AuthSummary::default();
    for (name, value) in headers {
        let lower = name.trim().to_ascii_lowercase();
        match lower.as_str() {
            "authorization" => {
                let trimmed = value.trim();
                if let Some(token) = trimmed.strip_prefix("Bearer ") {
                    summary.has_jwt = true;
                    if let Some((sub, exp)) = decode_jwt_claims(token.trim()) {
                        if summary.jwt_user_id.is_none() {
                            summary.jwt_user_id = sub;
                        }
                        if summary.jwt_exp_unix.is_none() {
                            summary.jwt_exp_unix = exp;
                        }
                    }
                } else if !trimmed.is_empty() {
                    summary.has_jwt = true;
                }
            }
            "cookie" => {
                summary.has_cookie = true;
                summary.cookie_names.extend(parse_cookie_names(value));
            }
            "set-cookie" => {
                summary.has_cookie = true;
                if let Some(name) = value.split(';').next().and_then(|kv| kv.split('=').next()) {
                    let trimmed = name.trim().to_string();
                    if !trimmed.is_empty() {
                        summary.cookie_names.push(trimmed);
                    }
                }
            }
            "host" | ":authority" if summary.host.is_none() => {
                summary.host = Some(value.trim().to_string());
            }
            _ => {}
        }
    }
    summary.cookie_names.sort();
    summary.cookie_names.dedup();
    summary
}

fn parse_cookie_names(value: &str) -> Vec<String> {
    value
        .split(';')
        .filter_map(|kv| {
            let name = kv.split('=').next()?.trim();
            if name.is_empty() {
                None
            } else {
                Some(name.to_string())
            }
        })
        .collect()
}

fn decode_jwt_claims(token: &str) -> Option<(Option<String>, Option<i64>)> {
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.as_bytes())
        .ok()
        .or_else(|| {
            base64::engine::general_purpose::URL_SAFE
                .decode(payload.as_bytes())
                .ok()
        })?;
    let json: Value = serde_json::from_slice(&raw).ok()?;
    let sub = json
        .get("sub")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            json.get("user_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .or_else(|| {
            json.get("uid")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .or_else(|| {
            json.get("user_id")
                .and_then(|v| v.as_i64())
                .map(|i| i.to_string())
        })
        .or_else(|| {
            json.get("uid")
                .and_then(|v| v.as_i64())
                .map(|i| i.to_string())
        });
    let exp = json.get("exp").and_then(|v| v.as_i64());
    Some((sub, exp))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> RedactOptions {
        default_options()
    }

    #[test]
    fn redact_headers_hides_authorization_and_cookie() {
        let mut headers = vec![
            ("Authorization".to_string(), "Bearer abcdef".to_string()),
            ("Cookie".to_string(), "sid=hello; uid=42".to_string()),
            ("Content-Type".to_string(), "application/json".to_string()),
        ];
        redact_headers(&mut headers, &opts());
        assert_eq!(headers[0].1, "<REDACTED:len=13>");
        assert!(headers[1].1.starts_with("<REDACTED:len="));
        assert_eq!(headers[2].1, "application/json");
    }

    #[test]
    fn redact_headers_matches_regex_token_secret_key() {
        let mut headers = vec![
            ("x-api-key".to_string(), "secretvalue".to_string()),
            ("X-Csrf-Token".to_string(), "abc123".to_string()),
            ("X-Session-Id".to_string(), "ses-001".to_string()),
            ("X-JWT".to_string(), "tokval".to_string()),
            ("X-Tt-Passport-Cookie".to_string(), "lol".to_string()),
            ("X-Request-Id".to_string(), "rid-42".to_string()),
        ];
        redact_headers(&mut headers, &opts());
        assert!(headers[0].1.starts_with("<REDACTED:"));
        assert!(headers[1].1.starts_with("<REDACTED:"));
        assert!(headers[2].1.starts_with("<REDACTED:"));
        assert!(headers[3].1.starts_with("<REDACTED:"));
        assert!(headers[4].1.starts_with("<REDACTED:"));
        assert_eq!(headers[5].1, "rid-42", "non-sensitive header preserved");
    }

    #[test]
    fn redact_headers_show_secrets_bypasses() {
        let mut headers = vec![("Authorization".to_string(), "Bearer abc".to_string())];
        let mut o = opts();
        o.show_secrets = true;
        redact_headers(&mut headers, &o);
        assert_eq!(headers[0].1, "Bearer abc");
    }

    #[test]
    fn redact_body_text_json_nested_keys_redacted() {
        let body = r#"{"user":{"name":"alice","password":"p1"},"tokens":["t1"],"api_key":"k","Token":"x","data":[{"refreshToken":"rt"}]}"#;
        let out = redact_body_text(Some("application/json"), body, &opts());
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["user"]["name"], "alice");
        assert!(v["user"]["password"]
            .as_str()
            .unwrap()
            .starts_with("<REDACTED:"));
        assert!(v["api_key"].as_str().unwrap().starts_with("<REDACTED:"));
        assert!(v["Token"].as_str().unwrap().starts_with("<REDACTED:"));
        assert!(v["data"][0]["refreshToken"]
            .as_str()
            .unwrap()
            .starts_with("<REDACTED:"));
    }

    #[test]
    fn redact_body_text_non_json_passthrough() {
        let body = "password=hunter2";
        let out = redact_body_text(Some("text/plain"), body, &opts());
        assert_eq!(out, body);
    }

    #[test]
    fn redact_body_text_invalid_json_passthrough() {
        let body = "not json {{{";
        let out = redact_body_text(Some("application/json"), body, &opts());
        assert_eq!(out, body);
    }

    #[test]
    fn redact_body_bytes_round_trips() {
        let body = br#"{"token":"abc"}"#;
        let out = redact_body_bytes(Some("application/json; charset=utf-8"), body, &opts());
        let s = String::from_utf8(out).unwrap();
        let v: Value = serde_json::from_str(&s).unwrap();
        assert!(v["token"].as_str().unwrap().starts_with("<REDACTED:"));
    }

    #[test]
    fn auth_summary_handles_bearer_jwt() {
        // Header: {"alg":"none","typ":"JWT"}  Payload: {"sub":"u-42","exp":99}
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"alg":"none","typ":"JWT"}"#);
        let payload =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"sub":"u-42","exp":99}"#);
        let token = format!("{}.{}.sig", header, payload);
        let headers = vec![
            ("Authorization".to_string(), format!("Bearer {}", token)),
            ("Host".to_string(), "api.example.com".to_string()),
        ];
        let s = auth_summary_from_headers(&headers);
        assert!(s.has_jwt);
        assert_eq!(s.jwt_user_id.as_deref(), Some("u-42"));
        assert_eq!(s.jwt_exp_unix, Some(99));
        assert_eq!(s.host.as_deref(), Some("api.example.com"));
        assert!(!s.has_cookie);
    }

    #[test]
    fn auth_summary_handles_cookies_only() {
        let headers = vec![("Cookie".to_string(), "sid=abc; uid=1; sid=abc".to_string())];
        let s = auth_summary_from_headers(&headers);
        assert!(!s.has_jwt);
        assert!(s.has_cookie);
        assert_eq!(s.cookie_names, vec!["sid".to_string(), "uid".to_string()]);
    }

    #[test]
    fn auth_summary_handles_no_auth() {
        let headers = vec![("Content-Type".to_string(), "application/json".to_string())];
        let s = auth_summary_from_headers(&headers);
        assert!(!s.has_jwt);
        assert!(!s.has_cookie);
        assert!(s.jwt_user_id.is_none());
    }

    #[test]
    fn auth_summary_handles_broken_jwt() {
        let headers = vec![(
            "Authorization".to_string(),
            "Bearer not-a-valid-jwt".to_string(),
        )];
        let s = auth_summary_from_headers(&headers);
        assert!(s.has_jwt);
        assert!(s.jwt_user_id.is_none());
        assert!(s.jwt_exp_unix.is_none());
    }

    #[test]
    fn is_sensitive_header_name_examples() {
        assert!(is_sensitive_header_name("Authorization"));
        assert!(is_sensitive_header_name("authorization"));
        assert!(is_sensitive_header_name("Proxy-Authorization"));
        assert!(is_sensitive_header_name("Cookie"));
        assert!(is_sensitive_header_name("Set-Cookie"));
        assert!(is_sensitive_header_name("X-Api-Key"));
        assert!(is_sensitive_header_name("X-Tt-Passport-Cookie"));
        assert!(!is_sensitive_header_name("Content-Type"));
        assert!(!is_sensitive_header_name("Accept"));
    }
}
