use std::net::SocketAddr;

use hyper::{
    body::Incoming,
    header::{HeaderMap, HeaderValue},
    Method, Request, Response, StatusCode,
};
use serde::Serialize;
use tracing::debug;

use crate::cors::apply_cors_headers;
use crate::handlers::{
    app_icon::handle_app_icon,
    asr::handle_asr,
    audit::handle_audit,
    auth::{extract_bearer_token, handle_auth},
    bifrost_file::handle_bifrost_file,
    breakpoint::handle_breakpoint,
    capture::handle_capture,
    cert::{handle_cert, handle_cert_public, handle_proxy_public},
    config::handle_config,
    cors_preflight,
    devtools::handle_devtools,
    diagnostics::handle_diagnostics,
    env::handle_env,
    error_response, frames,
    group::handle_group,
    group_rules::handle_group_rules,
    im_gateway::handle_im_gateway,
    json_response, method_not_allowed,
    metrics::handle_metrics,
    mobile_devices::{handle_mobile_devices, handle_mobile_public},
    notification::handle_notification,
    ports::handle_ports,
    power::handle_power,
    proxy::handle_proxy,
    remote_invoke::handle_remote_invoke,
    replay::handle_replay,
    room::handle_room,
    rule_share_confirm::{handle_rule_share_confirm_api, handle_rule_share_confirm_page},
    rules::{handle_rules, share_env_exit_page},
    scripts::handle_scripts_request,
    search::handle_search,
    speech::handle_speech,
    swagger,
    sync::{handle_sync, handle_sync_public},
    syntax::handle_syntax,
    system::handle_system,
    traffic::handle_traffic,
    trust_probe::{handle_trust_probe_api, handle_trust_probe_public},
    user::handle_user,
    values::handle_values,
    voice::handle_voice,
    websocket::handle_websocket_upgrade,
    whitelist::handle_whitelist_request,
    worker_jobs::handle_worker_jobs,
    workers::handle_workers,
    BoxBody,
};
use crate::push::SharedPushManager;
use crate::state::SharedAdminState;
use crate::static_files::serve_static_file;
use crate::{is_remote_access_enabled, validate_admin_jwt, ADMIN_PATH_PREFIX};

const CSRF_HEADER_NAME: &str = "x-bifrost-csrf";

fn parse_query(query: &str) -> std::collections::HashMap<String, String> {
    serde_urlencoded::from_str(query).unwrap_or_default()
}

fn query_token(query: Option<&str>) -> Option<String> {
    query
        .and_then(|query| parse_query(query).remove("token"))
        .filter(|token| !token.trim().is_empty())
}

fn should_apply_cors(admin_path: &str) -> bool {
    admin_path != "/share-env/exit"
}

fn should_activate_asr_scheduler(admin_path: &str) -> bool {
    admin_path == "/api/asr/external-volumes"
        || path_is_or_below(admin_path, "/api/asr/tasks")
        || path_is_or_below(admin_path, "/api/asr/diarization")
        || path_is_or_below(admin_path, "/api/asr/speaker-profiles")
}

fn path_is_or_below(path: &str, root: &str) -> bool {
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn apply_share_env_exit_cors(resp: &mut Response<BoxBody>, origin: Option<&str>) {
    let headers = resp.headers_mut();
    headers.insert(
        "Access-Control-Allow-Origin",
        origin
            .and_then(|origin| HeaderValue::from_str(origin).ok())
            .unwrap_or_else(|| HeaderValue::from_static("*")),
    );
    headers.insert("Vary", HeaderValue::from_static("Origin"));
    headers.insert(
        "Access-Control-Allow-Methods",
        HeaderValue::from_static("POST, OPTIONS"),
    );
    headers.insert(
        "Access-Control-Allow-Headers",
        HeaderValue::from_static("Content-Type"),
    );
}

fn apply_admin_page_frame_protection(resp: &mut Response<BoxBody>) {
    let is_html = resp
        .headers()
        .get(hyper::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_ascii_lowercase().contains("text/html"))
        .unwrap_or(false);
    if !is_html {
        return;
    }

    let headers = resp.headers_mut();
    headers
        .entry("X-Frame-Options")
        .or_insert_with(|| HeaderValue::from_static("DENY"));

    let existing_csp = headers
        .get("Content-Security-Policy")
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    match existing_csp.as_deref() {
        Some(existing) if existing.to_ascii_lowercase().contains("frame-ancestors") => {}
        Some(existing) => {
            if let Ok(value) =
                HeaderValue::from_str(&format!("{}; frame-ancestors 'none'", existing.trim()))
            {
                headers.insert("Content-Security-Policy", value);
            }
        }
        None => {
            headers.insert(
                "Content-Security-Policy",
                HeaderValue::from_static("frame-ancestors 'none'"),
            );
        }
    }
}

pub struct AdminRouter;

impl AdminRouter {
    pub async fn handle(
        req: Request<Incoming>,
        state: SharedAdminState,
        push_manager: Option<SharedPushManager>,
        peer_addr: Option<SocketAddr>,
    ) -> Response<BoxBody> {
        let path = req.uri().path().to_string();

        let admin_path = match path.strip_prefix(ADMIN_PATH_PREFIX) {
            Some(p) => p.to_string(),
            None => return error_response(StatusCode::NOT_FOUND, "Not Found"),
        };

        let origin = req
            .headers()
            .get("Origin")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        if req.method() == Method::OPTIONS {
            let mut resp = cors_preflight();
            if admin_path == "/api/rules/share-env/exit" {
                apply_share_env_exit_cors(&mut resp, origin.as_deref());
            } else if should_apply_cors(&admin_path) {
                apply_cors_headers(&mut resp, origin.as_deref());
            }
            return resp;
        }

        let mut resp = if admin_path == "/swagger" {
            swagger::serve_swagger_ui()
        } else if admin_path.starts_with("/public/cert") {
            handle_cert_public(req, state, &admin_path).await
        } else if admin_path.starts_with("/public/mobile") {
            handle_mobile_public(req, state, &admin_path).await
        } else if admin_path.starts_with("/public/proxy") {
            handle_proxy_public(req, state, &admin_path).await
        } else if admin_path.starts_with("/public/trust-probe") || admin_path == "/tp" {
            handle_trust_probe_public(req, state, push_manager.clone(), &admin_path).await
        } else if admin_path.starts_with("/public/sync-login") {
            handle_sync_public(req, state, &admin_path).await
        } else if admin_path == "/share-env/exit" {
            if req.method() == Method::GET {
                if let Some(resp) =
                    Self::check_api_auth(&req, &state, "/api/rules/share-env/exit", peer_addr)
                {
                    resp
                } else {
                    share_env_exit_page(state)
                }
            } else {
                method_not_allowed()
            }
        } else if admin_path.starts_with("/share/rule") {
            handle_rule_share_confirm_page(req, state).await
        } else if admin_path.starts_with("/api/") {
            Self::handle_api(req, state, push_manager, &admin_path, peer_addr).await
        } else {
            serve_static_file(&admin_path, req.headers())
        };

        if admin_path == "/api/rules/share-env/exit" {
            apply_share_env_exit_cors(&mut resp, origin.as_deref());
        } else if should_apply_cors(&admin_path) {
            apply_cors_headers(&mut resp, origin.as_deref());
        }
        apply_admin_page_frame_protection(&mut resp);
        resp
    }

    async fn handle_api(
        req: Request<Incoming>,
        state: SharedAdminState,
        push_manager: Option<SharedPushManager>,
        path: &str,
        peer_addr: Option<SocketAddr>,
    ) -> Response<BoxBody> {
        if let Some(resp) = Self::check_api_auth(&req, &state, path, peer_addr) {
            return resp;
        }

        if let Some(resp) = Self::check_browser_write_guard(&req, &state, path) {
            return resp;
        }

        // ASR is deliberately lazy: ordinary admin polling after a fresh
        // install must not recover tasks, start watchers, or prepare model
        // assets. Activation also belongs after auth/write checks so rejected
        // requests cannot initialize the subsystem.
        if should_activate_asr_scheduler(path) {
            crate::handlers::asr_jobs::ensure_scheduler_started().await;
        }

        if path == "/api/security/csrf" {
            return match req.method() {
                &Method::GET => json_response(&CsrfResponse {
                    csrf_token: state.csrf_token(),
                    header_name: "X-Bifrost-CSRF",
                }),
                _ => method_not_allowed(),
            };
        }

        if path == "/api/docs" {
            return swagger::serve_swagger_ui();
        }

        if path == "/api/openapi.json" {
            return swagger::serve_openapi_spec();
        }

        if path == "/api/rules/share-confirm" {
            return handle_rule_share_confirm_api(req, state, push_manager).await;
        }

        if path.starts_with("/api/auth") {
            return handle_auth(req, state, path, peer_addr).await;
        }

        if path.starts_with("/api/admin/audit") {
            return handle_audit(req, path).await;
        }

        if path.starts_with("/api/asr") {
            handle_asr(req, path).await
        } else if path.starts_with("/api/speech") {
            handle_speech(req, path).await
        } else if path.starts_with("/api/voice") {
            handle_voice(req, state, path).await
        } else if path.starts_with("/api/breakpoint") {
            handle_breakpoint(req, state, push_manager.clone(), path).await
        } else if path.starts_with("/api/capture") {
            handle_capture(req, state, push_manager.clone(), path).await
        } else if path.starts_with("/api/rules") {
            handle_rules(req, state, push_manager.clone(), path).await
        } else if path.starts_with("/api/devtools") {
            handle_devtools(req, state, path).await
        } else if path.starts_with("/api/traffic") {
            handle_traffic(req, state, push_manager.clone(), path).await
        } else if path.starts_with("/api/metrics") {
            handle_metrics(req, state, path).await
        } else if path.starts_with("/api/diagnostics") {
            handle_diagnostics(req, state, path).await
        } else if path.starts_with("/api/worker-jobs") {
            handle_worker_jobs(req, path).await
        } else if path.starts_with("/api/workers") {
            handle_workers(req, path).await
        } else if path.starts_with("/api/mobile-devices") {
            handle_mobile_devices(req, state, path, peer_addr).await
        } else if path.starts_with("/api/trust-probe") {
            handle_trust_probe_api(req, state, push_manager.clone(), path).await
        } else if path.starts_with("/api/ports") {
            handle_ports(req, state, path).await
        } else if path.starts_with("/api/power") {
            handle_power(req, state, path).await
        } else if path.starts_with("/api/system") {
            handle_system(req, state, path).await
        } else if path.starts_with("/api/values") {
            let path_suffix = path.strip_prefix("/api/values").unwrap_or("");
            handle_values(req, state, path_suffix).await
        } else if path.starts_with("/api/whitelist") {
            if let Some(access_control) = &state.access_control {
                handle_whitelist_request(
                    req,
                    access_control.clone(),
                    state.config_manager.clone(),
                    push_manager.clone(),
                    path,
                )
                .await
            } else {
                error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Access control not configured",
                )
            }
        } else if path.starts_with("/api/cert") {
            handle_cert(req, state, path, peer_addr).await
        } else if path.starts_with("/api/proxy") {
            handle_proxy(req, state, path).await
        } else if path.starts_with("/api/config") {
            handle_config(req, state, push_manager, path).await
        } else if path.starts_with("/api/websocket/connections") {
            frames::list_websocket_connections(state).await
        } else if path.starts_with("/api/push") {
            if let Some(pm) = push_manager {
                handle_websocket_upgrade(req, pm).await
            } else {
                error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Push manager not configured",
                )
            }
        } else if path.starts_with("/api/app-icon/") {
            debug!(path = %path, "Routing to app_icon handler");
            handle_app_icon(req, state, path).await
        } else if path.starts_with("/api/search") {
            handle_search(req, state, path).await
        } else if path.starts_with("/api/sync") {
            handle_sync(req, state, path).await
        } else if path.starts_with("/api/group-rules") {
            handle_group_rules(req, state, path).await
        } else if path.starts_with("/api/group") {
            handle_group(req, state, path).await
        } else if path.starts_with("/api/env") {
            handle_env(req, state, path).await
        } else if path.starts_with("/api/room") {
            handle_room(req, state, path).await
        } else if path.starts_with("/api/user") {
            handle_user(req, state, path).await
        } else if path.starts_with("/api/scripts") {
            if let Some(script_manager) = &state.script_manager {
                handle_scripts_request(
                    req,
                    script_manager.clone(),
                    state.config_manager.clone(),
                    path,
                )
                .await
            } else {
                error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Script manager not configured",
                )
            }
        } else if path.starts_with("/api/replay") {
            handle_replay(req, state, push_manager, path).await
        } else if path.starts_with("/api/notifications") {
            handle_notification(req, state, path).await
        } else if path.starts_with("/api/syntax") {
            handle_syntax(req, state, path).await
        } else if path.starts_with("/api/bifrost-file") {
            let path_suffix = path.strip_prefix("/api/bifrost-file").unwrap_or("");
            handle_bifrost_file(req, path_suffix, state.clone()).await
        } else if path.starts_with("/api/im-gateway") {
            handle_im_gateway(req, state.im_gateway_service(), path).await
        } else if path.starts_with("/api/remote-invoke") {
            handle_remote_invoke(req, state.remote_invoke_worker(), path).await
        } else {
            error_response(StatusCode::NOT_FOUND, "API endpoint not found")
        }
    }

    const AUTH_PUBLIC_PATHS: &[&str] = &[
        "/api/auth/status",
        "/api/auth/login",
        "/api/auth/logout",
        "/api/security/csrf",
    ];

    fn is_auth_public_path(path: &str) -> bool {
        Self::AUTH_PUBLIC_PATHS.contains(&path)
    }

    fn is_internal_devtools_bridge_path(path: &str) -> bool {
        path.starts_with("/api/devtools/bridge/")
    }

    fn check_api_auth<T>(
        req: &Request<T>,
        state: &SharedAdminState,
        path: &str,
        peer_addr: Option<SocketAddr>,
    ) -> Option<Response<BoxBody>> {
        let is_loopback = peer_addr
            .map(|addr| addr.ip().is_loopback())
            .unwrap_or(false);
        let loopback_host_ok = is_loopback
            && req
                .headers()
                .get(hyper::header::HOST)
                .and_then(|v| v.to_str().ok())
                .map(crate::cors::is_allowed_host)
                .unwrap_or(false);

        // The injected DevTools page bridge is intentionally cross-origin and
        // reaches the admin router through the bifrost.local virtual host with
        // peer_addr=None. It authenticates with a page-specific bridge token in
        // the bridge protocol itself, so router-level bearer auth would break
        // the intended internal channel while not adding useful protection.
        if Self::is_internal_devtools_bridge_path(path) {
            return None;
        }

        if !is_remote_access_enabled(state) {
            if loopback_host_ok || Self::is_auth_public_path(path) {
                return None;
            }
            return Some(error_response(
                StatusCode::UNAUTHORIZED,
                "Admin API requires local loopback access or remote authentication",
            ));
        }

        if Self::is_auth_public_path(path) {
            return None;
        }

        if loopback_host_ok {
            // Anti-DNS-rebinding: a loopback peer is necessary but not
            // sufficient. A browser tricked by DNS rebinding connects to
            // 127.0.0.1 (peer looks loopback) yet sends the attacker's domain
            // in the `Host` header. Only grant the loopback bypass when the
            // `Host` header is a recognized local name, which the legitimate
            // desktop UI always sends. Requests without a Host header (or with
            // a foreign Host) fall through to bearer-token validation.
            return None;
        }

        let token = extract_bearer_token(req).or_else(|| {
            if path == "/api/asr/transcribe-ws" || path == "/api/voice/listen-ws" {
                query_token(req.uri().query())
            } else {
                None
            }
        });
        let Some(token) = token else {
            return Some(error_response(
                StatusCode::UNAUTHORIZED,
                "Missing bearer token",
            ));
        };
        if let Err(e) = validate_admin_jwt(state, &token) {
            return Some(error_response(
                StatusCode::UNAUTHORIZED,
                &format!("Unauthorized: {e}"),
            ));
        }
        None
    }

    fn check_browser_write_guard<T>(
        req: &Request<T>,
        state: &SharedAdminState,
        path: &str,
    ) -> Option<Response<BoxBody>> {
        if is_safe_method(req.method()) {
            return None;
        }

        if path == "/api/rules/share-env/exit" {
            return None;
        }

        let headers = req.headers();
        let has_browser_context = header_value(headers, "origin").is_some()
            || header_value(headers, "referer").is_some()
            || header_value(headers, "sec-fetch-site").is_some()
            || header_value(headers, "sec-fetch-mode").is_some()
            || header_value(headers, "sec-fetch-dest").is_some();
        if !has_browser_context {
            return None;
        }

        if matches!(
            header_value(headers, "sec-fetch-site").as_deref(),
            Some("cross-site")
        ) && !header_value(headers, "origin")
            .as_deref()
            .map(|origin| {
                crate::cors::is_allowed_admin_origin_for_host(
                    origin,
                    &header_value(headers, "host").unwrap_or_default(),
                )
            })
            .unwrap_or(false)
        {
            return Some(error_response(
                StatusCode::FORBIDDEN,
                "Cross-site admin write request rejected",
            ));
        }

        if let Some(origin) = header_value(headers, "origin") {
            let host = header_value(headers, "host").unwrap_or_default();
            if !crate::cors::is_allowed_admin_origin_for_host(&origin, &host) {
                return Some(error_response(
                    StatusCode::FORBIDDEN,
                    "Cross-origin admin write request rejected",
                ));
            }
        }

        let csrf_header = header_value(headers, CSRF_HEADER_NAME);
        if csrf_header.as_deref() != Some(state.csrf_token()) {
            return Some(error_response(
                StatusCode::FORBIDDEN,
                "Missing or invalid admin CSRF token",
            ));
        }

        None
    }
}

#[derive(Serialize)]
struct CsrfResponse<'a> {
    csrf_token: &'a str,
    header_name: &'static str,
}

fn is_safe_method(method: &Method) -> bool {
    matches!(method, &Method::GET | &Method::HEAD | &Method::OPTIONS)
}

fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin_auth_db::AuthDb;
    use crate::state::AdminState;
    use crate::test_support::TestAdminState;

    fn new_state_remote_enabled() -> (SharedAdminState, tempfile::TempDir) {
        let (state, tmp) = new_state_remote_disabled();
        state
            .auth_db
            .as_ref()
            .unwrap()
            .set_remote_access_enabled(true)
            .expect("enable remote access");
        (state, tmp)
    }

    fn new_state_remote_disabled() -> (SharedAdminState, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let auth_db_path = tmp.path().join("auth.db");
        let auth_db = AuthDb::open(&auth_db_path).expect("auth db");
        let rules_storage =
            bifrost_storage::RulesStorage::with_dir(tmp.path().join("rules")).expect("rules db");

        let state = AdminState::new_for_test(19998, rules_storage).with_auth_db(auth_db);
        let state = std::sync::Arc::new(state);
        (state, tmp)
    }

    fn remote_peer() -> Option<SocketAddr> {
        Some("192.168.1.100:12345".parse().unwrap())
    }

    fn loopback_peer() -> Option<SocketAddr> {
        Some("127.0.0.1:12345".parse().unwrap())
    }

    #[test]
    fn test_share_env_exit_page_suppresses_cors() {
        assert!(!should_apply_cors("/share-env/exit"));
        assert!(should_apply_cors("/api/rules/share-env/exit"));
        assert!(should_apply_cors("/api/rules/share-env/status"));
        assert!(should_apply_cors("/"));
    }

    #[test]
    fn test_share_env_exit_api_allows_business_origin_cors() {
        let mut resp = Response::builder()
            .status(200)
            .body(crate::handlers::empty_body())
            .unwrap();
        apply_share_env_exit_cors(&mut resp, Some("https://www.coze.cn"));

        assert_eq!(
            resp.headers()
                .get("Access-Control-Allow-Origin")
                .and_then(|value| value.to_str().ok()),
            Some("https://www.coze.cn")
        );
        assert_eq!(
            resp.headers()
                .get("Access-Control-Allow-Methods")
                .and_then(|value| value.to_str().ok()),
            Some("POST, OPTIONS")
        );
        assert_eq!(
            resp.headers()
                .get("Access-Control-Allow-Headers")
                .and_then(|value| value.to_str().ok()),
            Some("Content-Type")
        );
    }

    #[test]
    fn test_admin_page_frame_protection_applies_to_html() {
        let mut resp = Response::builder()
            .status(200)
            .header("Content-Type", "text/html; charset=utf-8")
            .body(crate::handlers::empty_body())
            .unwrap();

        apply_admin_page_frame_protection(&mut resp);

        assert_eq!(
            resp.headers()
                .get("X-Frame-Options")
                .and_then(|value| value.to_str().ok()),
            Some("DENY")
        );
        assert_eq!(
            resp.headers()
                .get("Content-Security-Policy")
                .and_then(|value| value.to_str().ok()),
            Some("frame-ancestors 'none'")
        );
    }

    #[test]
    fn test_admin_page_frame_protection_preserves_existing_csp() {
        let mut resp = Response::builder()
            .status(200)
            .header("Content-Type", "text/html")
            .header("Content-Security-Policy", "default-src 'none'")
            .body(crate::handlers::empty_body())
            .unwrap();

        apply_admin_page_frame_protection(&mut resp);

        let csp = resp
            .headers()
            .get("Content-Security-Policy")
            .and_then(|value| value.to_str().ok())
            .expect("csp");
        assert!(csp.contains("default-src 'none'"));
        assert!(csp.contains("frame-ancestors 'none'"));
    }

    #[test]
    fn test_admin_page_frame_protection_skips_non_html() {
        let mut resp = Response::builder()
            .status(200)
            .header("Content-Type", "application/json")
            .body(crate::handlers::empty_body())
            .unwrap();

        apply_admin_page_frame_protection(&mut resp);

        assert!(resp.headers().get("X-Frame-Options").is_none());
        assert!(resp.headers().get("Content-Security-Policy").is_none());
    }

    #[test]
    fn asr_scheduler_activation_requires_asr_task_workflow() {
        for path in [
            "/api/system/overview",
            "/api/proxy/address",
            "/api/asr/capabilities",
            "/api/asr/status",
            "/api/asr/moss/status",
            "/api/asr/init-stream",
            "/api/asr/service/start",
            "/api/asr/tasksmith",
            "/api/asr/diarization-preview",
            "/api/asr/speaker-profiles-backup",
        ] {
            assert!(
                !should_activate_asr_scheduler(path),
                "read-only or unrelated path must not activate ASR tasks: {path}"
            );
        }

        for path in [
            "/api/asr/tasks",
            "/api/asr/tasks/task-1",
            "/api/asr/tasks/-/watch",
            "/api/asr/external-volumes",
            "/api/asr/diarization/status",
            "/api/asr/speaker-profiles",
        ] {
            assert!(
                should_activate_asr_scheduler(path),
                "ASR task workflow should activate its scheduler: {path}"
            );
        }
    }

    #[tokio::test]
    async fn authenticated_asr_task_route_reaches_lazy_scheduler_activation() {
        use hyper::server::conn::http1;
        use hyper::service::service_fn;
        use hyper_util::rt::TokioIo;
        use tokio::net::TcpListener;

        let harness = TestAdminState::builder().build();
        let state = harness.state();
        let previous = crate::handlers::asr_jobs::set_scheduler_started_for_test(true).await;
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind router test listener");
        let addr = listener.local_addr().expect("router test listener addr");

        let server = tokio::spawn(async move {
            let (stream, peer_addr) = listener.accept().await.expect("accept router request");
            let io = TokioIo::new(stream);
            let service = service_fn(move |req: Request<Incoming>| {
                let state = state.clone();
                async move {
                    Ok::<_, hyper::Error>(
                        AdminRouter::handle(req, state, None, Some(peer_addr)).await,
                    )
                }
            });
            http1::Builder::new()
                .serve_connection(io, service)
                .await
                .expect("serve router request");
        });

        let response = reqwest::get(format!(
            "http://{addr}{ADMIN_PATH_PREFIX}/api/asr/tasks/missing-task"
        ))
        .await
        .expect("request ASR task route");
        assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);

        server.abort();
        crate::handlers::asr_jobs::set_scheduler_started_for_test(previous).await;
    }

    #[test]
    fn test_check_api_auth_requires_token_when_remote_enabled_for_remote_peer() {
        let (state, _tmp) = new_state_remote_enabled();
        let req = Request::builder()
            .uri("/_bifrost/api/system/status")
            .body(())
            .unwrap();
        let resp = AdminRouter::check_api_auth(&req, &state, "/api/system/status", remote_peer())
            .expect("should reject remote without token");
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn test_check_api_auth_skips_for_loopback_when_remote_enabled() {
        let (state, _tmp) = new_state_remote_enabled();
        let req = Request::builder()
            .uri("/_bifrost/api/system/status")
            .header(hyper::header::HOST, "127.0.0.1:9900")
            .body(())
            .unwrap();
        let resp = AdminRouter::check_api_auth(&req, &state, "/api/system/status", loopback_peer());
        assert!(resp.is_none(), "loopback should skip auth");
    }

    #[test]
    fn test_check_api_auth_skips_auth_endpoints() {
        let (state, _tmp) = new_state_remote_enabled();
        let req = Request::builder()
            .uri("/_bifrost/api/auth/status")
            .body(())
            .unwrap();
        let resp = AdminRouter::check_api_auth(&req, &state, "/api/auth/status", remote_peer());
        assert!(resp.is_none(), "auth/status should be public");

        let req = Request::builder()
            .uri("/_bifrost/api/auth/login")
            .body(())
            .unwrap();
        let resp = AdminRouter::check_api_auth(&req, &state, "/api/auth/login", remote_peer());
        assert!(resp.is_none(), "auth/login should be public");

        let req = Request::builder()
            .uri("/_bifrost/api/auth/logout")
            .body(())
            .unwrap();
        let resp = AdminRouter::check_api_auth(&req, &state, "/api/auth/logout", remote_peer());
        assert!(resp.is_none(), "auth/logout should be public");
    }

    #[test]
    fn test_check_api_auth_rejects_remote_passwd_without_token() {
        let (state, _tmp) = new_state_remote_enabled();
        let req = Request::builder()
            .uri("/_bifrost/api/auth/passwd")
            .body(())
            .unwrap();
        let resp = AdminRouter::check_api_auth(&req, &state, "/api/auth/passwd", remote_peer())
            .expect("should reject remote passwd without token");
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn test_check_api_auth_rejects_remote_remote_toggle_without_token() {
        let (state, _tmp) = new_state_remote_enabled();
        let req = Request::builder()
            .uri("/_bifrost/api/auth/remote")
            .body(())
            .unwrap();
        let resp = AdminRouter::check_api_auth(&req, &state, "/api/auth/remote", remote_peer())
            .expect("should reject remote toggle without token");
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn test_check_api_auth_rejects_remote_share_env_exit_without_token() {
        let (state, _tmp) = new_state_remote_enabled();
        let req = Request::builder()
            .uri("/_bifrost/share-env/exit")
            .body(())
            .unwrap();
        let resp =
            AdminRouter::check_api_auth(&req, &state, "/api/rules/share-env/exit", remote_peer())
                .expect("remote share env exit confirmation should require token");
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn test_check_api_auth_rejects_remote_revoke_all_without_token() {
        let (state, _tmp) = new_state_remote_enabled();
        let req = Request::builder()
            .uri("/_bifrost/api/auth/revoke-all")
            .body(())
            .unwrap();
        let resp = AdminRouter::check_api_auth(&req, &state, "/api/auth/revoke-all", remote_peer())
            .expect("should reject remote revoke-all without token");
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn test_check_api_auth_allows_loopback_passwd() {
        let (state, _tmp) = new_state_remote_enabled();
        let req = Request::builder()
            .uri("/_bifrost/api/auth/passwd")
            .header(hyper::header::HOST, "127.0.0.1:9900")
            .body(())
            .unwrap();
        let resp = AdminRouter::check_api_auth(&req, &state, "/api/auth/passwd", loopback_peer());
        assert!(resp.is_none(), "loopback should access passwd freely");
    }

    #[test]
    fn test_check_api_auth_allows_loopback_remote_toggle() {
        let (state, _tmp) = new_state_remote_enabled();
        let req = Request::builder()
            .uri("/_bifrost/api/auth/remote")
            .header(hyper::header::HOST, "127.0.0.1:9900")
            .body(())
            .unwrap();
        let resp = AdminRouter::check_api_auth(&req, &state, "/api/auth/remote", loopback_peer());
        assert!(
            resp.is_none(),
            "loopback should access remote toggle freely"
        );
    }

    #[test]
    fn test_check_api_auth_rejects_when_peer_addr_none() {
        let (state, _tmp) = new_state_remote_enabled();
        let req = Request::builder()
            .uri("/_bifrost/api/system/status")
            .body(())
            .unwrap();
        let resp = AdminRouter::check_api_auth(&req, &state, "/api/system/status", None)
            .expect("None peer_addr should default to non-local and require token");
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn test_check_api_auth_rejects_none_peer_when_remote_disabled() {
        let (state, _tmp) = new_state_remote_disabled();
        let req = Request::builder()
            .uri("/_bifrost/api/system/status")
            .header(hyper::header::HOST, "bifrost.local")
            .body(())
            .unwrap();
        let resp = AdminRouter::check_api_auth(&req, &state, "/api/system/status", None)
            .expect("None peer_addr should not get local bypass when remote is disabled");
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn test_check_api_auth_allows_internal_devtools_bridge_with_none_peer() {
        let (state, _tmp) = new_state_remote_disabled();
        let req = Request::builder()
            .uri("/_bifrost/api/devtools/bridge/pg_123/ws")
            .header(hyper::header::HOST, "bifrost.local")
            .body(())
            .unwrap();
        let resp =
            AdminRouter::check_api_auth(&req, &state, "/api/devtools/bridge/pg_123/ws", None);
        assert!(
            resp.is_none(),
            "internal DevTools bridge uses its own page token"
        );
    }

    #[test]
    fn test_check_api_auth_allows_loopback_when_remote_disabled() {
        let (state, _tmp) = new_state_remote_disabled();
        let req = Request::builder()
            .uri("/_bifrost/api/system/status")
            .header(hyper::header::HOST, "127.0.0.1:9900")
            .body(())
            .unwrap();
        let resp = AdminRouter::check_api_auth(&req, &state, "/api/system/status", loopback_peer());
        assert!(resp.is_none(), "local loopback UI remains available");
    }

    #[test]
    fn test_check_api_auth_accepts_query_token_for_browser_websocket() {
        let (state, _tmp) = new_state_remote_enabled();
        let (token, _) = crate::admin_auth::issue_admin_jwt(&state, "admin").expect("issue jwt");
        let req = Request::builder()
            .uri(format!("/_bifrost/api/asr/transcribe-ws?token={token}"))
            .body(())
            .unwrap();
        let resp =
            AdminRouter::check_api_auth(&req, &state, "/api/asr/transcribe-ws", remote_peer());
        assert!(
            resp.is_none(),
            "browser WebSocket should authenticate with query token"
        );
    }

    #[test]
    fn test_check_api_auth_rejects_query_token_for_regular_api() {
        let (state, _tmp) = new_state_remote_enabled();
        let (token, _) = crate::admin_auth::issue_admin_jwt(&state, "admin").expect("issue jwt");
        let req = Request::builder()
            .uri(format!("/_bifrost/api/asr/status?token={token}"))
            .body(())
            .unwrap();
        let resp = AdminRouter::check_api_auth(&req, &state, "/api/asr/status", remote_peer())
            .expect("regular API should still require Authorization header");
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn test_browser_write_guard_rejects_cross_site_fetch() {
        let harness = crate::test_support::TestAdminState::builder()
            .port(9900)
            .build();
        let state = harness.state();
        let req = Request::builder()
            .method(Method::POST)
            .uri("/_bifrost/api/rules")
            .header(hyper::header::HOST, "127.0.0.1:9900")
            .header("Origin", "http://evil.example")
            .header("Sec-Fetch-Site", "cross-site")
            .body(())
            .unwrap();

        let resp = AdminRouter::check_browser_write_guard(&req, &state, "/api/rules")
            .expect("cross-site browser write should be rejected");
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn test_browser_write_guard_allows_share_env_exit_cross_site_bridge() {
        let harness = crate::test_support::TestAdminState::builder()
            .port(9900)
            .build();
        let state = harness.state();
        let req = Request::builder()
            .method(Method::POST)
            .uri("/_bifrost/api/rules/share-env/exit")
            .header(hyper::header::HOST, "127.0.0.1:9900")
            .header("Origin", "https://www.coze.cn")
            .header("Sec-Fetch-Site", "cross-site")
            .body(())
            .unwrap();

        assert!(
            AdminRouter::check_browser_write_guard(&req, &state, "/api/rules/share-env/exit")
                .is_none()
        );
    }

    #[test]
    fn test_browser_write_guard_requires_csrf_for_local_origin() {
        let harness = crate::test_support::TestAdminState::builder()
            .port(9900)
            .build();
        let state = harness.state();
        let req = Request::builder()
            .method(Method::PUT)
            .uri("/_bifrost/api/rules/demo/enable")
            .header(hyper::header::HOST, "127.0.0.1:9900")
            .header("Origin", "http://127.0.0.1:9900")
            .header("Sec-Fetch-Site", "same-origin")
            .body(())
            .unwrap();

        let resp = AdminRouter::check_browser_write_guard(&req, &state, "/api/rules/demo/enable")
            .expect("local browser write without CSRF should be rejected");
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn test_browser_write_guard_accepts_local_origin_with_csrf() {
        let harness = crate::test_support::TestAdminState::builder()
            .port(9900)
            .build();
        let state = harness.state();
        let req = Request::builder()
            .method(Method::PUT)
            .uri("/_bifrost/api/rules/demo/enable")
            .header(hyper::header::HOST, "127.0.0.1:9900")
            .header("Origin", "http://127.0.0.1:9900")
            .header("Sec-Fetch-Site", "same-origin")
            .header("X-Bifrost-CSRF", state.csrf_token())
            .body(())
            .unwrap();

        assert!(
            AdminRouter::check_browser_write_guard(&req, &state, "/api/rules/demo/enable")
                .is_none()
        );
    }

    #[test]
    fn test_browser_write_guard_accepts_trusted_desktop_origin_when_sec_fetch_is_cross_site() {
        let harness = crate::test_support::TestAdminState::builder()
            .port(9900)
            .build();
        let state = harness.state();
        let req = Request::builder()
            .method(Method::PUT)
            .uri("/_bifrost/api/rules/demo")
            .header(hyper::header::HOST, "127.0.0.1:9900")
            .header("Origin", "tauri://localhost")
            .header("Sec-Fetch-Site", "cross-site")
            .header("X-Bifrost-CSRF", state.csrf_token())
            .body(())
            .unwrap();

        assert!(AdminRouter::check_browser_write_guard(&req, &state, "/api/rules/demo").is_none());
    }

    #[test]
    fn test_browser_write_guard_still_requires_csrf_for_trusted_cross_site_desktop_origin() {
        let harness = crate::test_support::TestAdminState::builder()
            .port(9900)
            .build();
        let state = harness.state();
        let req = Request::builder()
            .method(Method::PUT)
            .uri("/_bifrost/api/rules/demo")
            .header(hyper::header::HOST, "127.0.0.1:9900")
            .header("Origin", "tauri://localhost")
            .header("Sec-Fetch-Site", "cross-site")
            .body(())
            .unwrap();

        let resp = AdminRouter::check_browser_write_guard(&req, &state, "/api/rules/demo")
            .expect("trusted desktop-origin writes still require CSRF");
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn test_browser_write_guard_allows_cli_without_browser_context() {
        let harness = crate::test_support::TestAdminState::builder()
            .port(9900)
            .build();
        let state = harness.state();
        let req = Request::builder()
            .method(Method::POST)
            .uri("/_bifrost/api/rules")
            .header(hyper::header::HOST, "127.0.0.1:9900")
            .body(())
            .unwrap();

        assert!(AdminRouter::check_browser_write_guard(&req, &state, "/api/rules").is_none());
    }

    #[test]
    fn test_origin_matches_host_for_remote_admin_origin() {
        assert!(crate::cors::origin_matches_host(
            "http://192.168.1.25:9900",
            "192.168.1.25:9900"
        ));
        assert!(!crate::cors::origin_matches_host(
            "http://evil.example",
            "192.168.1.25:9900"
        ));
    }
}
