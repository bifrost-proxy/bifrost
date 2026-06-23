use std::net::SocketAddr;

use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use tracing::debug;

use crate::cors::apply_cors_headers;
use crate::handlers::{
    agent_chat::handle_agent_chat,
    agent_memories::handle_agent_memories,
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
    env::handle_env,
    error_response, frames,
    group::handle_group,
    group_rules::handle_group_rules,
    im_gateway::handle_im_gateway,
    method_not_allowed,
    metrics::handle_metrics,
    mobile_devices::{handle_mobile_devices, handle_mobile_public},
    notification::handle_notification,
    ports::handle_ports,
    power::handle_power,
    proxy::handle_proxy,
    remote_invoke::handle_remote_invoke,
    replay::handle_replay,
    room::handle_room,
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
    videos::handle_videos,
    voice::handle_voice,
    websocket::handle_websocket_upgrade,
    whitelist::handle_whitelist_request,
    BoxBody,
};
use crate::push::SharedPushManager;
use crate::state::SharedAdminState;
use crate::static_files::serve_static_file;
use crate::{is_remote_access_enabled, validate_admin_jwt, ADMIN_PATH_PREFIX};

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
            if should_apply_cors(&admin_path) {
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
        } else if admin_path.starts_with("/api/") {
            // Ensure the ASR scheduled-task scheduler is running on first API
            // request so tasks execute even if the ASR page is never visited.
            crate::handlers::asr_jobs::ensure_scheduler_started().await;
            Self::handle_api(req, state, push_manager, &admin_path, peer_addr).await
        } else {
            serve_static_file(&admin_path, req.headers())
        };

        if should_apply_cors(&admin_path) {
            apply_cors_headers(&mut resp, origin.as_deref());
        }
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

        if path == "/api/docs" {
            return swagger::serve_swagger_ui();
        }

        if path == "/api/openapi.json" {
            return swagger::serve_openapi_spec();
        }

        if path.starts_with("/api/auth") {
            return handle_auth(req, state, path, peer_addr).await;
        }

        if path.starts_with("/api/admin/audit") {
            return handle_audit(req, path).await;
        }

        if path.starts_with("/api/agent/chat") {
            handle_agent_chat(req, state, path).await
        } else if path.starts_with("/api/agent/memories") {
            handle_agent_memories(req, path).await
        } else if path.starts_with("/api/asr") {
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
        } else if path.starts_with("/api/videos") {
            handle_videos(req, path).await
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

    const AUTH_PUBLIC_PATHS: &[&str] = &["/api/auth/status", "/api/auth/login", "/api/auth/logout"];

    fn is_auth_public_path(path: &str) -> bool {
        Self::AUTH_PUBLIC_PATHS.contains(&path)
    }

    fn check_api_auth<T>(
        req: &Request<T>,
        state: &SharedAdminState,
        path: &str,
        peer_addr: Option<SocketAddr>,
    ) -> Option<Response<BoxBody>> {
        if !is_remote_access_enabled(state) {
            return None;
        }

        if Self::is_auth_public_path(path) {
            return None;
        }

        let is_loopback = peer_addr
            .map(|addr| addr.ip().is_loopback())
            .unwrap_or(false);

        if is_loopback {
            // Anti-DNS-rebinding: a loopback peer is necessary but not
            // sufficient. A browser tricked by DNS rebinding connects to
            // 127.0.0.1 (peer looks loopback) yet sends the attacker's domain
            // in the `Host` header. Only grant the loopback bypass when the
            // `Host` header is a recognized local name, which the legitimate
            // desktop UI always sends. Requests without a Host header (or with
            // a foreign Host) fall through to bearer-token validation.
            let host_ok = req
                .headers()
                .get(hyper::header::HOST)
                .and_then(|v| v.to_str().ok())
                .map(crate::cors::is_allowed_host)
                .unwrap_or(false);
            if host_ok {
                return None;
            }
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin_auth_db::AuthDb;
    use crate::state::AdminState;

    fn new_state_remote_enabled() -> (SharedAdminState, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let auth_db_path = tmp.path().join("auth.db");
        let auth_db = AuthDb::open(&auth_db_path).expect("auth db");
        let rules_storage =
            bifrost_storage::RulesStorage::with_dir(tmp.path().join("rules")).expect("rules db");

        let state = AdminState::new_for_test(19998, rules_storage).with_auth_db(auth_db);
        let state = std::sync::Arc::new(state);
        state
            .auth_db
            .as_ref()
            .unwrap()
            .set_remote_access_enabled(true)
            .expect("enable remote access");
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
}
