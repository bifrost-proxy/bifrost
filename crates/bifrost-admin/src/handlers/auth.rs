use std::net::SocketAddr;

use hyper::{body::Incoming, header, Method, Request, Response, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::time::sleep;
use tracing::{info, warn};

use super::{
    cors_preflight, error_response, json_response, json_response_with_status, method_not_allowed,
    BoxBody,
};
use crate::admin_audit;
use crate::admin_auth::{
    get_admin_username, get_failed_login_count, has_admin_password, is_remote_access_enabled,
    issue_admin_jwt, record_failed_login, reset_failed_login_count, revoke_all_admin_sessions,
    set_admin_password_hash, set_admin_username, set_remote_access_enabled,
    validate_password_strength, verify_admin_credentials, MAX_LOGIN_ATTEMPTS, MIN_PASSWORD_LENGTH,
};
use crate::state::SharedAdminState;
use crate::ADMIN_PATH_PREFIX;

pub const ADMIN_SESSION_COOKIE_NAME: &str = "bifrost_admin_session";
const ADMIN_SESSION_COOKIE_MAX_AGE_SECS: i64 = 7 * 24 * 60 * 60;

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub token: String,
    pub expires_at: String,
    pub username: String,
}

#[derive(Debug, Serialize)]
pub struct AuthStatusResponse {
    pub remote_access_enabled: bool,
    pub auth_required: bool,
    pub username: String,
    pub has_password: bool,
    pub locked_out: bool,
    pub failed_attempts: u32,
    pub max_attempts: u32,
    pub min_password_length: usize,
}

#[derive(Debug, Deserialize)]
pub struct ChangePasswordRequest {
    pub username: Option<String>,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct RemoteAccessRequest {
    pub enabled: bool,
}

pub async fn handle_auth(
    req: Request<Incoming>,
    state: SharedAdminState,
    path: &str,
    peer_addr: Option<SocketAddr>,
) -> Response<BoxBody> {
    if req.method() == Method::OPTIONS {
        return cors_preflight();
    }

    let is_loopback = peer_addr
        .map(|addr| addr.ip().is_loopback())
        .unwrap_or(false);

    if path == "/api/auth/status" {
        if *req.method() != Method::GET {
            return method_not_allowed();
        }
        let remote_enabled = is_remote_access_enabled(&state);
        let username = get_admin_username(&state).unwrap_or_else(|| "admin".to_string());
        let password_set = has_admin_password(&state);
        let failed_attempts = if is_loopback {
            get_failed_login_count(&state)
        } else {
            0
        };
        let locked_out = false;
        return json_response(&AuthStatusResponse {
            remote_access_enabled: remote_enabled,
            auth_required: remote_enabled && !is_loopback,
            username,
            has_password: password_set,
            locked_out,
            failed_attempts,
            max_attempts: MAX_LOGIN_ATTEMPTS,
            min_password_length: MIN_PASSWORD_LENGTH,
        });
    }

    if path == "/api/auth/login" {
        if *req.method() != Method::POST {
            return method_not_allowed();
        }

        if !require_json_content_type(&req) {
            return error_response(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "Content-Type must be application/json",
            );
        }

        if !is_remote_access_enabled(&state) {
            // 默认本地管理端不需要登录；仅在开启远程访问时启用鉴权。
            return error_response(StatusCode::FORBIDDEN, "Remote admin access is not enabled");
        }

        let peer_ip = req
            .headers()
            .get("x-bifrost-peer-ip")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown")
            .to_string();
        let user_agent = req
            .headers()
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let body = match http_body_util::BodyExt::collect(req.into_body()).await {
            Ok(collected) => collected.to_bytes(),
            Err(_) => {
                return error_response(StatusCode::BAD_REQUEST, "Failed to read request body")
            }
        };

        let login: LoginRequest = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => {
                return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {e}"))
            }
        };

        let ok = match verify_admin_credentials(&state, &login.username, &login.password) {
            Ok(v) => v,
            Err(e) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("Auth error: {e}"),
                )
            }
        };
        if !ok {
            let failed_count = match record_failed_login(&state) {
                Ok(c) => c,
                Err(e) => {
                    warn!(error = %e, "Failed to record login failure");
                    0
                }
            };

            if let Err(e) =
                admin_audit::record_failed_login_attempt(&login.username, &peer_ip, &user_agent)
            {
                warn!(error = %e, "Failed to write failed login audit");
            }

            // Progressive delay: increase response time with each failure
            let delay_secs = std::cmp::min(failed_count as u64, 10);
            if delay_secs > 0 {
                sleep(std::time::Duration::from_secs(delay_secs)).await;
            }

            if failed_count >= MAX_LOGIN_ATTEMPTS {
                return json_response_with_status(
                    StatusCode::FORBIDDEN,
                    &serde_json::json!({
                        "error": "Too many failed login attempts. Try again later or use the correct administrator password.",
                        "locked_out": true,
                    }),
                );
            }

            let remaining = MAX_LOGIN_ATTEMPTS.saturating_sub(failed_count);
            let error_msg = if remaining <= 3 {
                "Invalid credentials. Few attempts remaining before lockout."
            } else {
                "Invalid credentials"
            };
            return json_response_with_status(
                StatusCode::UNAUTHORIZED,
                &serde_json::json!({
                    "error": error_msg,
                }),
            );
        }

        if let Err(e) = reset_failed_login_count(&state) {
            warn!(error = %e, "Failed to reset login failure count");
        }

        let (token, claims) = match issue_admin_jwt(&state, &login.username) {
            Ok(v) => v,
            Err(e) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("Failed to issue token: {e}"),
                )
            }
        };

        if let Err(e) = admin_audit::record_login(&login.username, &peer_ip, &user_agent) {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to write audit log: {e}"),
            );
        }
        let expires_at = chrono::DateTime::<chrono::Utc>::from_timestamp(claims.exp, 0)
            .map(|t| t.to_rfc3339())
            .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());

        // CLI 和普通浏览器 API 继续使用显式 token；HttpOnly Cookie 让无法
        // 自定义 Authorization header 的同源 EventSource/WebSocket 也能鉴权。
        // SameSite=Strict 配合 router 的 CSRF/Origin 检查限制浏览器跨站使用。
        let mut resp = json_response(&LoginResponse {
            token: token.clone(),
            expires_at,
            username: login.username,
        });
        resp.headers_mut().insert(
            header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        resp.headers_mut().insert(
            header::SET_COOKIE,
            admin_session_cookie(&token, ADMIN_SESSION_COOKIE_MAX_AGE_SECS)
                .parse()
                .expect("admin session cookie must be a valid header"),
        );
        return resp;
    }

    if path == "/api/auth/logout" {
        if *req.method() != Method::POST {
            return method_not_allowed();
        }
        return Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .header(header::SET_COOKIE, clear_admin_session_cookie())
            .body(super::full_body("{\"success\":true}"))
            .unwrap();
    }

    if path == "/api/auth/session" {
        return handle_session_request(&req, &state);
    }

    if path == "/api/auth/passwd" {
        if *req.method() != Method::POST {
            return method_not_allowed();
        }

        if !require_json_content_type(&req) {
            return error_response(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "Content-Type must be application/json",
            );
        }

        let body = match http_body_util::BodyExt::collect(req.into_body()).await {
            Ok(collected) => collected.to_bytes(),
            Err(_) => {
                return error_response(StatusCode::BAD_REQUEST, "Failed to read request body")
            }
        };

        let payload: ChangePasswordRequest = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => {
                return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {e}"))
            }
        };

        if payload.password.is_empty() {
            return error_response(StatusCode::BAD_REQUEST, "Password cannot be empty");
        }

        if let Err(e) = validate_password_strength(&payload.password) {
            return error_response(StatusCode::BAD_REQUEST, &e.to_string());
        }

        if let Some(ref username) = payload.username {
            if let Err(e) = set_admin_username(&state, username) {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("Failed to set username: {e}"),
                );
            }
        }

        if let Err(e) = set_admin_password_hash(&state, &payload.password) {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to set password: {e}"),
            );
        }

        let username = payload
            .username
            .unwrap_or_else(|| get_admin_username(&state).unwrap_or_else(|| "admin".to_string()));
        info!(username = %username, "Admin password updated via API");
        return json_response(&serde_json::json!({
            "success": true,
            "message": "Password updated"
        }));
    }

    if path == "/api/auth/remote" {
        if *req.method() != Method::POST {
            return method_not_allowed();
        }

        if !require_json_content_type(&req) {
            return error_response(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "Content-Type must be application/json",
            );
        }

        let body = match http_body_util::BodyExt::collect(req.into_body()).await {
            Ok(collected) => collected.to_bytes(),
            Err(_) => {
                return error_response(StatusCode::BAD_REQUEST, "Failed to read request body")
            }
        };

        let payload: RemoteAccessRequest = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => {
                return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {e}"))
            }
        };

        if payload.enabled && !has_admin_password(&state) {
            return error_response(
                StatusCode::BAD_REQUEST,
                "Cannot enable remote access without setting a password first",
            );
        }

        if let Err(e) = set_remote_access_enabled(&state, payload.enabled) {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to update remote access: {e}"),
            );
        }

        info!(enabled = payload.enabled, "Remote access toggled via API");
        let username = get_admin_username(&state).unwrap_or_else(|| "admin".to_string());
        let password_set = has_admin_password(&state);
        let failed_attempts = get_failed_login_count(&state);
        return json_response(&AuthStatusResponse {
            remote_access_enabled: payload.enabled,
            auth_required: payload.enabled,
            username,
            has_password: password_set,
            locked_out: false,
            failed_attempts,
            max_attempts: MAX_LOGIN_ATTEMPTS,
            min_password_length: MIN_PASSWORD_LENGTH,
        });
    }

    if path == "/api/auth/revoke-all" {
        if *req.method() != Method::POST {
            return method_not_allowed();
        }

        if let Err(e) = revoke_all_admin_sessions(&state) {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to revoke sessions: {e}"),
            );
        }

        info!("All admin sessions revoked via API");
        let mut resp = json_response(&serde_json::json!({
            "success": true,
            "message": "All sessions revoked"
        }));
        resp.headers_mut().insert(
            header::SET_COOKIE,
            clear_admin_session_cookie()
                .parse()
                .expect("admin session clear cookie must be a valid header"),
        );
        return resp;
    }

    error_response(StatusCode::NOT_FOUND, "API endpoint not found")
}

fn handle_session_request<B>(req: &Request<B>, state: &SharedAdminState) -> Response<BoxBody> {
    if *req.method() != Method::GET {
        return method_not_allowed();
    }
    let tokens = [extract_bearer_token(req), extract_admin_session_cookie(req)];
    if tokens.iter().all(Option::is_none) {
        return error_response(StatusCode::UNAUTHORIZED, "Missing authenticated session");
    }
    let mut authenticated = None;
    let mut last_error = None;
    for token in tokens.into_iter().flatten() {
        match crate::admin_auth::validate_admin_jwt(state, &token) {
            Ok(claims) => {
                authenticated = Some((token, claims));
                break;
            }
            Err(error) => last_error = Some(error),
        }
    }
    let Some((token, claims)) = authenticated else {
        return error_response(
            StatusCode::UNAUTHORIZED,
            &format!(
                "Unauthorized: {}",
                last_error.expect("at least one credential was validated")
            ),
        );
    };
    let max_age_secs = (claims.exp - chrono::Utc::now().timestamp()).max(0);
    let mut resp = json_response(&serde_json::json!({ "success": true }));
    resp.headers_mut().insert(
        header::SET_COOKIE,
        admin_session_cookie(&token, max_age_secs)
            .parse()
            .expect("admin session cookie must be a valid header"),
    );
    resp
}

fn require_json_content_type<B>(req: &Request<B>) -> bool {
    req.headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|ct| ct.to_ascii_lowercase().contains("application/json"))
        .unwrap_or(false)
}

pub fn extract_bearer_token<T>(req: &Request<T>) -> Option<String> {
    let header_val = req.headers().get(header::AUTHORIZATION)?.to_str().ok()?;
    let v = header_val.trim();
    let token = v
        .strip_prefix("Bearer ")
        .or_else(|| v.strip_prefix("bearer "))?;
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    Some(token.to_string())
}

pub fn extract_admin_session_cookie<T>(req: &Request<T>) -> Option<String> {
    req.headers()
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|cookie| cookie.trim().split_once('='))
        .find_map(|(name, value)| {
            if name.trim() != ADMIN_SESSION_COOKIE_NAME {
                return None;
            }
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_string())
        })
}

fn admin_session_cookie(token: &str, max_age_secs: i64) -> String {
    format!(
        "{ADMIN_SESSION_COOKIE_NAME}={token}; Path={ADMIN_PATH_PREFIX}; Max-Age={max_age_secs}; HttpOnly; SameSite=Strict"
    )
}

fn clear_admin_session_cookie() -> String {
    format!(
        "{ADMIN_SESSION_COOKIE_NAME}=; Path={ADMIN_PATH_PREFIX}; Max-Age=0; HttpOnly; SameSite=Strict"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin_auth_db::AuthDb;
    use hyper::Request;
    use std::sync::Arc;

    fn authenticated_state() -> (SharedAdminState, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let auth_db = AuthDb::open(&tmp.path().join("auth.db")).expect("auth db");
        (
            Arc::new(crate::AdminState::new(0).with_auth_db(auth_db)),
            tmp,
        )
    }

    #[test]
    fn session_endpoint_requires_get_and_a_credential() {
        let (state, _tmp) = authenticated_state();
        let post = Request::builder().method(Method::POST).body(()).unwrap();
        assert_eq!(
            handle_session_request(&post, &state).status(),
            StatusCode::METHOD_NOT_ALLOWED
        );

        let missing = Request::builder().method(Method::GET).body(()).unwrap();
        assert_eq!(
            handle_session_request(&missing, &state).status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn session_endpoint_falls_back_from_invalid_bearer_to_valid_cookie() {
        let (state, _tmp) = authenticated_state();
        let (token, _) = issue_admin_jwt(&state, "admin").expect("issue jwt");
        let req = Request::builder()
            .method(Method::GET)
            .header(header::AUTHORIZATION, "Bearer invalid")
            .header(
                header::COOKIE,
                format!("{ADMIN_SESSION_COOKIE_NAME}={token}"),
            )
            .body(())
            .unwrap();

        let response = handle_session_request(&req, &state);
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(
                |value| value.starts_with(&format!("{ADMIN_SESSION_COOKIE_NAME}={token};"))
            ));
    }

    #[test]
    fn session_endpoint_rejects_invalid_credentials() {
        let (state, _tmp) = authenticated_state();
        let req = Request::builder()
            .method(Method::GET)
            .header(header::AUTHORIZATION, "Bearer invalid")
            .body(())
            .unwrap();
        assert_eq!(
            handle_session_request(&req, &state).status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn test_extract_bearer_token_accepts_bearer_and_lowercase() {
        let req = Request::builder()
            .uri("/")
            .header(header::AUTHORIZATION, "Bearer abc")
            .body(())
            .unwrap();
        assert_eq!(extract_bearer_token(&req), Some("abc".to_string()));

        let req = Request::builder()
            .uri("/")
            .header(header::AUTHORIZATION, "bearer def")
            .body(())
            .unwrap();
        assert_eq!(extract_bearer_token(&req), Some("def".to_string()));
    }

    #[test]
    fn test_extract_bearer_token_rejects_empty_token() {
        let req = Request::builder()
            .uri("/")
            .header(header::AUTHORIZATION, "Bearer   ")
            .body(())
            .unwrap();
        assert_eq!(extract_bearer_token(&req), None);
    }

    #[test]
    fn test_extract_admin_session_cookie_finds_exact_cookie_name() {
        let req = Request::builder()
            .uri("/")
            .header(
                header::COOKIE,
                "theme=dark; bifrost_admin_session=header.payload.signature; other=1",
            )
            .body(())
            .unwrap();
        assert_eq!(
            extract_admin_session_cookie(&req),
            Some("header.payload.signature".to_string())
        );

        let req = Request::builder()
            .uri("/")
            .header(header::COOKIE, "not_bifrost_admin_session=wrong")
            .body(())
            .unwrap();
        assert_eq!(extract_admin_session_cookie(&req), None);
    }

    #[test]
    fn test_extract_admin_session_cookie_rejects_empty_or_malformed_value() {
        for cookie in [
            "bifrost_admin_session=",
            "bifrost_admin_session",
            "theme=dark",
        ] {
            let req = Request::builder()
                .uri("/")
                .header(header::COOKIE, cookie)
                .body(())
                .unwrap();
            assert_eq!(extract_admin_session_cookie(&req), None, "{cookie}");
        }
    }

    #[test]
    fn test_admin_session_cookie_is_scoped_and_not_script_readable() {
        let cookie = admin_session_cookie("token", ADMIN_SESSION_COOKIE_MAX_AGE_SECS);
        assert!(cookie.starts_with("bifrost_admin_session=token;"));
        assert!(cookie.contains("Path=/_bifrost"));
        assert!(cookie.contains("Max-Age=604800"));
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Strict"));
    }

    #[test]
    fn test_clear_admin_session_cookie_expires_cookie_at_same_path() {
        let cookie = clear_admin_session_cookie();
        assert!(cookie.starts_with("bifrost_admin_session=;"));
        assert!(cookie.contains("Path=/_bifrost"));
        assert!(cookie.contains("Max-Age=0"));
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Strict"));
    }
}
