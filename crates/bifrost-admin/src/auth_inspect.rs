//! JWT / Cookie 登录态诊断模块。
//!
//! 该模块从已捕获的请求/响应头中识别用户登录态（JWT、Cookie），
//! 并解析其过期时间和用户标识，给出 [`AuthSummary`] 报告，供 HTTP API 和
//! CLI 命令使用。
//!
//! 隐私：模块只解析过期时间与 user_id；**永远不会**回显原始 token 字符串。

use base64::Engine as _;
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

/// 单个请求级别的登录态诊断结果。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthSummary {
    pub host: String,
    pub has_jwt: bool,
    pub has_cookie: bool,
    pub jwt_exp_ms: Option<i64>,
    pub jwt_user_id: Option<String>,
    pub cookie_exp_ms: Option<i64>,
    pub valid_at_ms: Option<i64>,
    /// 综合 jwt/cookie 过期信息得出的“是否仍有效”。
    /// 没有任何过期信息时为 `None`。
    pub valid: Option<bool>,
}

impl AuthSummary {
    fn empty(host: &str, now_ms: i64) -> Self {
        Self {
            host: host.to_string(),
            has_jwt: false,
            has_cookie: false,
            jwt_exp_ms: None,
            jwt_user_id: None,
            cookie_exp_ms: None,
            valid_at_ms: Some(now_ms),
            valid: None,
        }
    }
}

/// 解析 JWT 字符串的 payload，返回 `(exp_ms, user_id)`。失败返回 `None`。
///
/// - 不验证签名；只关心过期与 user_id；任何错误都安全返回 None。
/// - base64 使用 URL-safe，且容忍无 padding。
pub fn parse_jwt_exp(token: &str) -> Option<(i64, Option<String>)> {
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload_b64 = parts.next()?;
    let _signature = parts.next();
    // 标准 JWT 是三段，如果没有第三段也尝试继续解析（容错）。
    let payload_bytes = decode_b64_url(payload_b64)?;
    let value: serde_json::Value = serde_json::from_slice(&payload_bytes).ok()?;
    let exp = value.get("exp").and_then(|v| v.as_i64());
    let user_id = value
        .get("sub")
        .or_else(|| value.get("user_id"))
        .or_else(|| value.get("uid"))
        .and_then(|v| match v {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Number(n) => Some(n.to_string()),
            _ => None,
        });
    let exp_ms = exp.and_then(|s| s.checked_mul(1000))?;
    Some((exp_ms, user_id))
}

fn decode_b64_url(s: &str) -> Option<Vec<u8>> {
    // 容忍无 padding 的 URL-safe base64。
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    if let Ok(v) = engine.decode(s) {
        return Some(v);
    }
    // 再容忍带 padding 的版本。
    let trimmed = s.trim_end_matches('=');
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(trimmed)
        .ok()
}

/// 解析单个 `Set-Cookie` header 值的过期时间（ms epoch）。
/// 支持 `Expires=<RFC1123>` 与 `Max-Age=<sec>`（基于 `now_ms`）。
/// 同时存在时取较早者（更保守）。
pub fn parse_cookie_expiry(set_cookie: &str) -> Option<i64> {
    parse_cookie_expiry_at(set_cookie, None)
}

/// 同 [`parse_cookie_expiry`]，但允许指定参考时间，用于 Max-Age 转换。
pub fn parse_cookie_expiry_at(set_cookie: &str, now_ms: Option<i64>) -> Option<i64> {
    let mut expires_ms: Option<i64> = None;
    let mut max_age_ms: Option<i64> = None;

    for attr in set_cookie.split(';') {
        let attr = attr.trim();
        if let Some(v) = strip_prefix_ci(attr, "Expires=") {
            if let Some(ts) = parse_http_date(v.trim()) {
                expires_ms = Some(ts);
            }
        } else if let Some(v) = strip_prefix_ci(attr, "Max-Age=") {
            if let Ok(secs) = v.trim().parse::<i64>() {
                let base = now_ms.unwrap_or_else(current_now_ms);
                if let Some(extra_ms) = secs.checked_mul(1000) {
                    if let Some(sum) = base.checked_add(extra_ms) {
                        max_age_ms = Some(sum);
                    }
                }
            }
        }
    }

    match (expires_ms, max_age_ms) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() < prefix.len() {
        return None;
    }
    let (head, tail) = s.split_at(prefix.len());
    if head.eq_ignore_ascii_case(prefix) {
        Some(tail)
    } else {
        None
    }
}

fn parse_http_date(s: &str) -> Option<i64> {
    // 优先 RFC2822 (与 RFC1123 兼容)，回退到几种常见 cookie 日期格式。
    if let Ok(dt) = DateTime::parse_from_rfc2822(s) {
        return Some(dt.with_timezone(&Utc).timestamp_millis());
    }
    let candidates = [
        "%a, %d %b %Y %H:%M:%S GMT",
        "%a, %d-%b-%Y %H:%M:%S GMT",
        "%A, %d-%b-%y %H:%M:%S GMT",
        "%a %b %e %H:%M:%S %Y",
    ];
    for fmt in candidates {
        if let Ok(ndt) = NaiveDateTime::parse_from_str(s, fmt) {
            return Utc
                .from_local_datetime(&ndt)
                .single()
                .map(|d| d.timestamp_millis());
        }
    }
    None
}

fn current_now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 从请求/响应 headers 综合构造 [`AuthSummary`]。
///
/// - `headers` 应是请求头与响应头合并后的列表（key 不区分大小写）。
/// - `now_ms` 用于评估 valid（毫秒 epoch）。
pub fn build_auth_summary(headers: &[(String, String)], host: &str, now_ms: i64) -> AuthSummary {
    let mut summary = AuthSummary::empty(host, now_ms);

    // 1. JWT 来源：Authorization / X-Jwt-Token / X-Auth-Token / Cookie
    let mut jwt_candidates: Vec<&str> = Vec::new();
    for (k, v) in headers {
        let key = k.to_ascii_lowercase();
        match key.as_str() {
            "authorization" => {
                if let Some(rest) = v
                    .strip_prefix("Bearer ")
                    .or_else(|| v.strip_prefix("bearer "))
                {
                    jwt_candidates.push(rest.trim());
                }
            }
            "x-jwt-token" | "x-auth-token" | "jwt-token" => {
                jwt_candidates.push(v.trim());
            }
            "cookie" => {
                for c in v.split(';') {
                    let c = c.trim();
                    if let Some((name, val)) = c.split_once('=') {
                        let name_l = name.to_ascii_lowercase();
                        if name_l.contains("jwt") || name_l.contains("token") {
                            jwt_candidates.push(val);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    for cand in jwt_candidates {
        if let Some((exp_ms, user_id)) = parse_jwt_exp(cand) {
            summary.has_jwt = true;
            // 选最早过期时间（最保守）；任一拿到 user_id 即记下。
            summary.jwt_exp_ms = Some(match summary.jwt_exp_ms {
                Some(prev) => prev.min(exp_ms),
                None => exp_ms,
            });
            if summary.jwt_user_id.is_none() {
                summary.jwt_user_id = user_id;
            }
        } else if !cand.is_empty() && cand.split('.').count() >= 2 {
            // 看起来像 JWT 但解析失败：只标记 has_jwt，不写 exp。
            summary.has_jwt = true;
        }
    }

    // 2. Cookie：Set-Cookie 响应头里 Expires/Max-Age，或 Cookie 头存在即标记 has_cookie。
    for (k, v) in headers {
        let key = k.to_ascii_lowercase();
        match key.as_str() {
            "set-cookie" => {
                summary.has_cookie = true;
                if let Some(exp) = parse_cookie_expiry_at(v, Some(now_ms)) {
                    summary.cookie_exp_ms = Some(match summary.cookie_exp_ms {
                        Some(prev) => prev.min(exp),
                        None => exp,
                    });
                }
            }
            "cookie" if !v.trim().is_empty() => {
                summary.has_cookie = true;
            }
            _ => {}
        }
    }

    // 3. 综合 valid：所有出现过的 exp 都必须 > now_ms 才算有效；若全无 exp，valid = None。
    let mut all_valid = true;
    let mut any_exp = false;
    if let Some(exp) = summary.jwt_exp_ms {
        any_exp = true;
        if exp <= now_ms {
            all_valid = false;
        }
    }
    if let Some(exp) = summary.cookie_exp_ms {
        any_exp = true;
        if exp <= now_ms {
            all_valid = false;
        }
    }
    summary.valid = if any_exp { Some(all_valid) } else { None };

    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    fn make_jwt(payload: &serde_json::Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(b"{\"alg\":\"HS256\",\"typ\":\"JWT\"}");
        let payload_bytes = serde_json::to_vec(payload).unwrap();
        let payload_b64 = URL_SAFE_NO_PAD.encode(payload_bytes);
        format!("{header}.{payload_b64}.fakesig")
    }

    #[test]
    fn parse_jwt_exp_basic() {
        let tok = make_jwt(&serde_json::json!({"exp": 1_700_000_000, "sub": "u1"}));
        let (exp_ms, uid) = parse_jwt_exp(&tok).unwrap();
        assert_eq!(exp_ms, 1_700_000_000_000);
        assert_eq!(uid.as_deref(), Some("u1"));
    }

    #[test]
    fn parse_jwt_exp_handles_padded_base64() {
        // 手动构造带 = padding 的 token
        let payload = b"{\"exp\":1700000000,\"sub\":\"abc\"}";
        let mut encoded = base64::engine::general_purpose::URL_SAFE.encode(payload);
        if !encoded.contains('=') {
            encoded.push('=');
        }
        let header = URL_SAFE_NO_PAD.encode(b"{\"alg\":\"none\"}");
        let token = format!("{header}.{encoded}.sig");
        let (exp_ms, _) = parse_jwt_exp(&token).unwrap();
        assert_eq!(exp_ms, 1_700_000_000_000);
    }

    #[test]
    fn parse_jwt_exp_returns_none_for_missing_exp() {
        let tok = make_jwt(&serde_json::json!({"sub": "u1"}));
        assert!(parse_jwt_exp(&tok).is_none());
    }

    #[test]
    fn parse_jwt_exp_returns_none_for_malformed_token() {
        assert!(parse_jwt_exp("not.a.jwt").is_none());
        assert!(parse_jwt_exp("").is_none());
        assert!(parse_jwt_exp("only-one-segment").is_none());
    }

    #[test]
    fn parse_jwt_exp_supports_numeric_user_id() {
        let tok = make_jwt(&serde_json::json!({"exp": 100, "user_id": 12345}));
        let (_, uid) = parse_jwt_exp(&tok).unwrap();
        assert_eq!(uid.as_deref(), Some("12345"));
    }

    #[test]
    fn parse_jwt_exp_supports_uid_field() {
        let tok = make_jwt(&serde_json::json!({"exp": 100, "uid": "u42"}));
        let (_, uid) = parse_jwt_exp(&tok).unwrap();
        assert_eq!(uid.as_deref(), Some("u42"));
    }

    #[test]
    fn parse_cookie_expiry_rfc1123() {
        let v = "session=abc; Expires=Wed, 21 Oct 2099 07:28:00 GMT; HttpOnly";
        let ts = parse_cookie_expiry(v).unwrap();
        // 2099-10-21T07:28:00Z
        let dt = Utc.with_ymd_and_hms(2099, 10, 21, 7, 28, 0).unwrap();
        assert_eq!(ts, dt.timestamp_millis());
    }

    #[test]
    fn parse_cookie_expiry_max_age() {
        let v = "x=1; Max-Age=3600";
        let now = 1_700_000_000_000_i64;
        let ts = parse_cookie_expiry_at(v, Some(now)).unwrap();
        assert_eq!(ts, now + 3_600_000);
    }

    #[test]
    fn parse_cookie_expiry_takes_earlier_when_both_present() {
        let v = "x=1; Expires=Wed, 21 Oct 2099 07:28:00 GMT; Max-Age=10";
        let now = 1_700_000_000_000_i64;
        let ts = parse_cookie_expiry_at(v, Some(now)).unwrap();
        // Max-Age=10 sec → much earlier than 2099。
        assert_eq!(ts, now + 10_000);
    }

    #[test]
    fn parse_cookie_expiry_none_when_session_cookie() {
        assert!(parse_cookie_expiry("session=abc; HttpOnly").is_none());
    }

    #[test]
    fn build_auth_summary_bearer_jwt() {
        let tok = make_jwt(&serde_json::json!({"exp": 9_999_999_999_i64, "sub": "user-9"}));
        let headers = vec![("Authorization".into(), format!("Bearer {tok}"))];
        let s = build_auth_summary(&headers, "api.example.com", 1_700_000_000_000);
        assert!(s.has_jwt);
        assert_eq!(s.jwt_user_id.as_deref(), Some("user-9"));
        assert_eq!(s.valid, Some(true));
        assert!(!s.has_cookie);
    }

    #[test]
    fn build_auth_summary_expired_jwt() {
        let tok = make_jwt(&serde_json::json!({"exp": 1, "sub": "u"}));
        let headers = vec![("Authorization".into(), format!("Bearer {tok}"))];
        let s = build_auth_summary(&headers, "h", 1_700_000_000_000);
        assert!(s.has_jwt);
        assert_eq!(s.valid, Some(false));
    }

    #[test]
    fn build_auth_summary_cookie_jwt_name() {
        let tok = make_jwt(&serde_json::json!({"exp": 9_999_999_999_i64, "sub": "ck-u"}));
        let headers = vec![("Cookie".into(), format!("session=x; jwt={tok}"))];
        let s = build_auth_summary(&headers, "h", 1_700_000_000_000);
        assert!(s.has_jwt);
        assert!(s.has_cookie);
        assert_eq!(s.jwt_user_id.as_deref(), Some("ck-u"));
    }

    #[test]
    fn build_auth_summary_set_cookie_response() {
        let headers = vec![(
            "Set-Cookie".into(),
            "sid=abc; Expires=Wed, 21 Oct 2099 07:28:00 GMT".to_string(),
        )];
        let s = build_auth_summary(&headers, "h", 1_700_000_000_000);
        assert!(s.has_cookie);
        assert!(s.cookie_exp_ms.is_some());
        assert_eq!(s.valid, Some(true));
    }

    #[test]
    fn build_auth_summary_empty_headers() {
        let s = build_auth_summary(&[], "h", 1_700_000_000_000);
        assert!(!s.has_jwt);
        assert!(!s.has_cookie);
        assert_eq!(s.valid, None);
    }

    #[test]
    fn build_auth_summary_multiple_set_cookie_picks_earliest() {
        let now = 1_700_000_000_000_i64;
        let headers = vec![
            (
                "Set-Cookie".into(),
                "a=1; Expires=Wed, 21 Oct 2099 07:28:00 GMT".to_string(),
            ),
            ("Set-Cookie".into(), "b=2; Max-Age=60".to_string()),
        ];
        let s = build_auth_summary(&headers, "h", now);
        assert_eq!(s.cookie_exp_ms, Some(now + 60_000));
    }

    #[test]
    fn build_auth_summary_invalid_jwt_string_marks_has_jwt_without_exp() {
        let headers = vec![("Authorization".into(), "Bearer foo.bar.baz".to_string())];
        let s = build_auth_summary(&headers, "h", 1_700_000_000_000);
        assert!(s.has_jwt);
        assert!(s.jwt_exp_ms.is_none());
        assert_eq!(s.valid, None);
    }
}
