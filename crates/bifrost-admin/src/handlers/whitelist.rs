use std::net::IpAddr;
use std::sync::Arc;

use bytes::Bytes;
use chrono::{DateTime, Utc};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use tracing::info;

use super::{error_response, full_body, json_response, method_not_allowed, BoxBody};
use crate::push::SharedPushManager;
use bifrost_core::{AccessMode, ClientAccessControl, UserPassAccountConfig, UserPassAuthConfig};
use bifrost_storage::{AccessConfigUpdate, SharedConfigManager};

#[derive(Serialize)]
struct WhitelistResponse {
    mode: String,
    allow_lan: bool,
    whitelist: Vec<String>,
    temporary_whitelist: Vec<String>,
    session_denied: Vec<String>,
    userpass: UserPassAuthResponse,
}

#[derive(Serialize)]
struct UserPassAuthResponse {
    enabled: bool,
    accounts: Vec<UserPassAccountResponse>,
    loopback_requires_auth: bool,
}

#[derive(Serialize)]
struct UserPassAccountResponse {
    username: String,
    enabled: bool,
    has_password: bool,
    last_connected_at: Option<String>,
}

#[derive(Deserialize)]
struct AddWhitelistRequest {
    ip_or_cidr: String,
}

#[derive(Deserialize)]
struct UpdateModeRequest {
    mode: String,
}

#[derive(Deserialize)]
struct UpdateAllowLanRequest {
    allow_lan: bool,
}

#[derive(Deserialize)]
struct UpdateUserPassRequest {
    enabled: bool,
    accounts: Vec<UpdateUserPassAccountRequest>,
    #[serde(default)]
    loopback_requires_auth: bool,
}

#[derive(Deserialize)]
struct UpdateUserPassAccountRequest {
    username: String,
    password: Option<String>,
    enabled: bool,
}

#[derive(Deserialize)]
struct TemporaryWhitelistRequest {
    ip: String,
}

pub async fn handle_whitelist_request(
    req: Request<Incoming>,
    access_control: Arc<RwLock<ClientAccessControl>>,
    config_manager: Option<SharedConfigManager>,
    push_manager: Option<SharedPushManager>,
    path: &str,
) -> Response<BoxBody> {
    match (req.method(), path) {
        (&Method::GET, "/api/whitelist") => handle_list(access_control).await,
        (&Method::POST, "/api/whitelist") => {
            handle_add(req, access_control, config_manager, push_manager).await
        }
        (&Method::DELETE, "/api/whitelist") => {
            handle_remove(req, access_control, config_manager, push_manager).await
        }
        (&Method::GET, "/api/whitelist/mode") => handle_get_mode(access_control).await,
        (&Method::PUT, "/api/whitelist/mode") => {
            handle_set_mode(req, access_control, config_manager, push_manager).await
        }
        (&Method::GET, "/api/whitelist/allow-lan") => handle_get_allow_lan(access_control).await,
        (&Method::PUT, "/api/whitelist/allow-lan") => {
            handle_set_allow_lan(req, access_control, config_manager, push_manager).await
        }
        (&Method::PUT, "/api/whitelist/userpass") => {
            handle_set_userpass(req, access_control, config_manager, push_manager).await
        }
        (&Method::POST, "/api/whitelist/temporary") => {
            handle_add_temporary(req, access_control, push_manager).await
        }
        (&Method::DELETE, "/api/whitelist/temporary") => {
            handle_remove_temporary(req, access_control, push_manager).await
        }
        (&Method::GET, "/api/whitelist/pending") => handle_get_pending(access_control).await,
        (&Method::GET, "/api/whitelist/pending/stream") => {
            handle_pending_stream(access_control).await
        }
        (&Method::POST, "/api/whitelist/pending/approve") => {
            handle_approve_pending(req, access_control, push_manager).await
        }
        (&Method::POST, "/api/whitelist/pending/reject") => {
            handle_reject_pending(req, access_control, push_manager).await
        }
        (&Method::DELETE, "/api/whitelist/pending") => {
            handle_clear_pending(access_control, push_manager).await
        }
        (&Method::GET, "/api/whitelist/session-denied") => {
            handle_get_session_denied(access_control).await
        }
        (&Method::DELETE, "/api/whitelist/session-denied") => {
            handle_remove_session_denied(req, access_control, push_manager).await
        }
        _ => method_not_allowed(),
    }
}

async fn handle_list(access_control: Arc<RwLock<ClientAccessControl>>) -> Response<BoxBody> {
    let ac = access_control.read().await;
    let response = WhitelistResponse {
        mode: ac.mode().to_string(),
        allow_lan: ac.allow_lan(),
        whitelist: ac.whitelist_entries(),
        temporary_whitelist: ac
            .temporary_whitelist_entries()
            .iter()
            .map(|ip| ip.to_string())
            .collect(),
        session_denied: ac
            .session_denied_entries()
            .iter()
            .map(|ip| ip.to_string())
            .collect(),
        userpass: build_userpass_response(&ac),
    };
    json_response(&response)
}

async fn broadcast_access_snapshots(
    push_manager: &Option<SharedPushManager>,
    include_pending: bool,
) {
    if let Some(pm) = push_manager {
        pm.invalidate_overview_cache();
        pm.broadcast_settings_scope(crate::push::SETTINGS_SCOPE_WHITELIST_STATUS)
            .await;
        if include_pending {
            pm.broadcast_settings_scope(crate::push::SETTINGS_SCOPE_PENDING_AUTHORIZATIONS)
                .await;
        }
    }
}

async fn handle_add(
    req: Request<Incoming>,
    access_control: Arc<RwLock<ClientAccessControl>>,
    config_manager: Option<SharedConfigManager>,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    let body = match req.collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "Failed to read request body"),
    };

    let request: AddWhitelistRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)),
    };

    let mut ac = access_control.write().await;
    match ac.add_to_whitelist(&request.ip_or_cidr) {
        Ok(_) => {
            info!("Added {} to whitelist via API", request.ip_or_cidr);
            let whitelist = ac.whitelist_entries();
            drop(ac);

            if let Some(ref cm) = config_manager {
                let update = AccessConfigUpdate {
                    mode: None,
                    whitelist: Some(whitelist),
                    allow_lan: None,
                    userpass: None,
                };
                if let Err(e) = cm.update_access_config(update).await {
                    tracing::error!("Failed to persist whitelist: {}", e);
                }
            }
            broadcast_access_snapshots(&push_manager, false).await;

            let response = serde_json::json!({
                "success": true,
                "message": format!("Added {} to whitelist", request.ip_or_cidr)
            });
            Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "application/json")
                .body(full_body(response.to_string()))
                .unwrap()
        }
        Err(e) => error_response(StatusCode::BAD_REQUEST, &e),
    }
}

async fn handle_remove(
    req: Request<Incoming>,
    access_control: Arc<RwLock<ClientAccessControl>>,
    config_manager: Option<SharedConfigManager>,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    let body = match req.collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "Failed to read request body"),
    };

    let request: AddWhitelistRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)),
    };

    let mut ac = access_control.write().await;
    match ac.remove_from_whitelist(&request.ip_or_cidr) {
        Ok(removed) => {
            if removed {
                info!("Removed {} from whitelist via API", request.ip_or_cidr);
                let whitelist = ac.whitelist_entries();
                drop(ac);

                if let Some(ref cm) = config_manager {
                    let update = AccessConfigUpdate {
                        mode: None,
                        whitelist: Some(whitelist),
                        allow_lan: None,
                        userpass: None,
                    };
                    if let Err(e) = cm.update_access_config(update).await {
                        tracing::error!("Failed to persist whitelist removal: {}", e);
                    }
                }
                broadcast_access_snapshots(&push_manager, false).await;

                let response = serde_json::json!({
                    "success": true,
                    "message": format!("Removed {} from whitelist", request.ip_or_cidr)
                });
                Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "application/json")
                    .body(full_body(response.to_string()))
                    .unwrap()
            } else {
                error_response(
                    StatusCode::NOT_FOUND,
                    &format!("{} not found in whitelist", request.ip_or_cidr),
                )
            }
        }
        Err(e) => error_response(StatusCode::BAD_REQUEST, &e),
    }
}

async fn handle_get_mode(access_control: Arc<RwLock<ClientAccessControl>>) -> Response<BoxBody> {
    let ac = access_control.read().await;
    let response = serde_json::json!({
        "mode": ac.mode().to_string()
    });
    json_response(&response)
}

async fn handle_set_mode(
    req: Request<Incoming>,
    access_control: Arc<RwLock<ClientAccessControl>>,
    config_manager: Option<SharedConfigManager>,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    let body = match req.collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "Failed to read request body"),
    };

    let request: UpdateModeRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)),
    };

    let mode: AccessMode = match request.mode.parse() {
        Ok(m) => m,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &e),
    };

    let mut ac = access_control.write().await;
    ac.set_mode(mode);
    let mode_value = ac.mode().to_string();
    drop(ac);

    if let Some(ref cm) = config_manager {
        let update = AccessConfigUpdate {
            mode: Some(mode),
            whitelist: None,
            allow_lan: None,
            userpass: None,
        };
        if let Err(e) = cm.update_access_config(update).await {
            tracing::error!("Failed to persist access mode: {}", e);
        } else {
            info!("Access mode {} persisted to config", mode);
        }
    }
    broadcast_access_snapshots(&push_manager, true).await;

    let response = serde_json::json!({
        "success": true,
        "mode": mode_value
    });
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/json")
        .body(full_body(response.to_string()))
        .unwrap()
}

async fn handle_get_allow_lan(
    access_control: Arc<RwLock<ClientAccessControl>>,
) -> Response<BoxBody> {
    let ac = access_control.read().await;
    let response = serde_json::json!({
        "allow_lan": ac.allow_lan()
    });
    json_response(&response)
}

async fn handle_set_allow_lan(
    req: Request<Incoming>,
    access_control: Arc<RwLock<ClientAccessControl>>,
    config_manager: Option<SharedConfigManager>,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    let body = match req.collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "Failed to read request body"),
    };

    let request: UpdateAllowLanRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)),
    };

    let mut ac = access_control.write().await;
    ac.set_allow_lan(request.allow_lan);
    let allow_lan = ac.allow_lan();
    drop(ac);

    if let Some(ref cm) = config_manager {
        let update = AccessConfigUpdate {
            mode: None,
            whitelist: None,
            allow_lan: Some(request.allow_lan),
            userpass: None,
        };
        if let Err(e) = cm.update_access_config(update).await {
            tracing::error!("Failed to persist allow_lan setting: {}", e);
        } else {
            info!(
                "Allow LAN setting {} persisted to config",
                request.allow_lan
            );
        }
    }
    broadcast_access_snapshots(&push_manager, false).await;

    let response = serde_json::json!({
        "success": true,
        "allow_lan": allow_lan
    });
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/json")
        .body(full_body(response.to_string()))
        .unwrap()
}

async fn handle_set_userpass(
    req: Request<Incoming>,
    access_control: Arc<RwLock<ClientAccessControl>>,
    config_manager: Option<SharedConfigManager>,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    let body = match req.collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "Failed to read request body"),
    };

    let request: UpdateUserPassRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)),
    };

    // Serialize the read/validate/persist/apply sequence so concurrent CLI and
    // WebUI updates cannot both derive from the same stale account snapshot.
    // Persist before switching the runtime policy: a disk failure must not
    // leave the current process using credentials that disappear on restart.
    let ac = access_control.write().await;
    let existing_userpass = ac.userpass_config();
    let userpass = match validate_userpass_request(request, existing_userpass.as_ref()) {
        Ok(userpass) => userpass,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };

    if let Some(ref cm) = config_manager {
        let update = AccessConfigUpdate {
            mode: None,
            whitelist: None,
            allow_lan: None,
            userpass: Some(Some(userpass.clone())),
        };
        if let Err(e) = cm.update_access_config(update).await {
            tracing::error!("Failed to persist userpass config: {}", e);
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to persist userpass config",
            );
        }
    }

    ac.set_userpass_config(Some(userpass.clone()));
    drop(ac);

    if let Some(ref cm) = config_manager {
        let valid_usernames = userpass
            .accounts
            .iter()
            .map(|account| account.username.clone())
            .collect::<std::collections::HashSet<_>>();
        let filtered_timestamps = cm
            .userpass_last_connected_at()
            .await
            .into_iter()
            .filter(|(username, _)| valid_usernames.contains(username))
            .collect();
        if let Err(e) = cm
            .replace_userpass_last_connected_at(filtered_timestamps)
            .await
        {
            tracing::error!("Failed to persist userpass timestamps cleanup: {}", e);
        }
    }

    broadcast_access_snapshots(&push_manager, false).await;
    json_response(&serde_json::json!({
        "success": true
    }))
}

fn validate_userpass_request(
    request: UpdateUserPassRequest,
    existing_userpass: Option<&UserPassAuthConfig>,
) -> Result<UserPassAuthConfig, String> {
    let mut usernames = std::collections::HashSet::new();
    let mut accounts = Vec::new();

    for account in request.accounts {
        if account.username.trim().is_empty() {
            return Err("username cannot be empty".to_string());
        }
        if !usernames.insert(account.username.clone()) {
            return Err(format!("duplicate username '{}'", account.username));
        }
        let password = account.password.or_else(|| {
            existing_userpass.and_then(|current| {
                current
                    .accounts
                    .iter()
                    .find(|existing| existing.username == account.username)
                    .and_then(|existing| existing.password.clone())
            })
        });
        if password.as_deref().unwrap_or_default().is_empty() {
            return Err(format!(
                "password is required for account '{}'",
                account.username
            ));
        }
        accounts.push(UserPassAccountConfig {
            username: account.username,
            password,
            enabled: account.enabled,
        });
    }

    Ok(UserPassAuthConfig {
        enabled: request.enabled,
        accounts,
        loopback_requires_auth: request.loopback_requires_auth,
    })
}

fn build_userpass_response(access_control: &ClientAccessControl) -> UserPassAuthResponse {
    let status = access_control.userpass_status();
    UserPassAuthResponse {
        enabled: status.enabled,
        accounts: status
            .accounts
            .into_iter()
            .map(|account| UserPassAccountResponse {
                username: account.username,
                enabled: account.enabled,
                has_password: account.has_password,
                last_connected_at: account.last_connected_at.and_then(format_timestamp_rfc3339),
            })
            .collect(),
        loopback_requires_auth: status.loopback_requires_auth,
    }
}

fn format_timestamp_rfc3339(timestamp: u64) -> Option<String> {
    DateTime::<Utc>::from_timestamp(timestamp as i64, 0).map(|dt| dt.to_rfc3339())
}

async fn handle_add_temporary(
    req: Request<Incoming>,
    access_control: Arc<RwLock<ClientAccessControl>>,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    let body = match req.collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "Failed to read request body"),
    };

    let request: TemporaryWhitelistRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)),
    };

    let ip: IpAddr = match request.ip.parse() {
        Ok(ip) => ip,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Invalid IP address: {}", e),
            )
        }
    };

    let ac = access_control.read().await;
    ac.add_temporary(ip);
    drop(ac);
    broadcast_access_snapshots(&push_manager, false).await;

    let response = serde_json::json!({
        "success": true,
        "message": format!("Added {} to temporary whitelist", ip)
    });
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/json")
        .body(full_body(response.to_string()))
        .unwrap()
}

async fn handle_remove_temporary(
    req: Request<Incoming>,
    access_control: Arc<RwLock<ClientAccessControl>>,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    let body = match req.collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "Failed to read request body"),
    };

    let request: TemporaryWhitelistRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)),
    };

    let ip: IpAddr = match request.ip.parse() {
        Ok(ip) => ip,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Invalid IP address: {}", e),
            )
        }
    };

    let ac = access_control.read().await;
    let removed = ac.remove_temporary(&ip);
    drop(ac);

    if removed {
        broadcast_access_snapshots(&push_manager, false).await;
        let response = serde_json::json!({
            "success": true,
            "message": format!("Removed {} from temporary whitelist", ip)
        });
        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(full_body(response.to_string()))
            .unwrap()
    } else {
        error_response(
            StatusCode::NOT_FOUND,
            &format!("{} not found in temporary whitelist", ip),
        )
    }
}

async fn handle_get_pending(access_control: Arc<RwLock<ClientAccessControl>>) -> Response<BoxBody> {
    let ac = access_control.read().await;
    let pending = ac.get_pending_authorizations();
    json_response(&pending)
}

async fn handle_pending_stream(
    access_control: Arc<RwLock<ClientAccessControl>>,
) -> Response<BoxBody> {
    let ac = access_control.read().await;
    let receiver = ac.subscribe();
    drop(ac);

    let stream = BroadcastStream::new(receiver).filter_map(|result| match result {
        Ok(event) => {
            let data = serde_json::to_string(&event).ok()?;
            let sse_data = format!("data: {}\n\n", data);
            Some(sse_data)
        }
        Err(_) => None,
    });

    let body_stream = http_body_util::StreamBody::new(
        stream.map(|s| Ok::<_, hyper::Error>(hyper::body::Frame::data(Bytes::from(s)))),
    );

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/event-stream")
        .header("Cache-Control", "no-cache")
        .header("Connection", "keep-alive")
        .body(BoxBody::new(body_stream))
        .unwrap()
}

async fn handle_approve_pending(
    req: Request<Incoming>,
    access_control: Arc<RwLock<ClientAccessControl>>,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    let body = match req.collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "Failed to read request body"),
    };

    let request: TemporaryWhitelistRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)),
    };

    let ip: IpAddr = match request.ip.parse() {
        Ok(ip) => ip,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Invalid IP address: {}", e),
            )
        }
    };

    let ac = access_control.read().await;
    if ac.approve_pending(&ip) {
        drop(ac);
        broadcast_access_snapshots(&push_manager, true).await;
        info!("Approved pending authorization for {} via API", ip);
        let response = serde_json::json!({
            "success": true,
            "message": format!("Approved {} and added to temporary whitelist", ip)
        });
        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(full_body(response.to_string()))
            .unwrap()
    } else {
        drop(ac);
        error_response(
            StatusCode::NOT_FOUND,
            &format!("{} not found in pending authorizations", ip),
        )
    }
}

async fn handle_reject_pending(
    req: Request<Incoming>,
    access_control: Arc<RwLock<ClientAccessControl>>,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    let body = match req.collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "Failed to read request body"),
    };

    let request: TemporaryWhitelistRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)),
    };

    let ip: IpAddr = match request.ip.parse() {
        Ok(ip) => ip,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Invalid IP address: {}", e),
            )
        }
    };

    let ac = access_control.read().await;
    if ac.reject_pending(&ip) {
        drop(ac);
        broadcast_access_snapshots(&push_manager, true).await;
        info!("Rejected pending authorization for {} via API", ip);
        let response = serde_json::json!({
            "success": true,
            "message": format!("Rejected {} and added to session denied list", ip)
        });
        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(full_body(response.to_string()))
            .unwrap()
    } else {
        drop(ac);
        error_response(
            StatusCode::NOT_FOUND,
            &format!("{} not found in pending authorizations", ip),
        )
    }
}

async fn handle_clear_pending(
    access_control: Arc<RwLock<ClientAccessControl>>,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    let ac = access_control.read().await;
    ac.clear_pending_authorizations();
    drop(ac);
    broadcast_access_snapshots(&push_manager, true).await;
    info!("Cleared all pending authorizations via API");
    let response = serde_json::json!({
        "success": true,
        "message": "Cleared all pending authorizations"
    });
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/json")
        .body(full_body(response.to_string()))
        .unwrap()
}

async fn handle_get_session_denied(
    access_control: Arc<RwLock<ClientAccessControl>>,
) -> Response<BoxBody> {
    let ac = access_control.read().await;
    let denied: Vec<String> = ac
        .session_denied_entries()
        .iter()
        .map(|ip| ip.to_string())
        .collect();
    json_response(&denied)
}

async fn handle_remove_session_denied(
    req: Request<Incoming>,
    access_control: Arc<RwLock<ClientAccessControl>>,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    let body = match req.collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "Failed to read request body"),
    };

    #[derive(Deserialize)]
    struct RemoveSessionDeniedRequest {
        ip: Option<String>,
    }

    let request: RemoveSessionDeniedRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(_) => RemoveSessionDeniedRequest { ip: None },
    };

    let ac = access_control.read().await;
    if let Some(ip_str) = request.ip {
        let ip: IpAddr = match ip_str.parse() {
            Ok(ip) => ip,
            Err(e) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    &format!("Invalid IP address: {}", e),
                )
            }
        };
        let removed = ac.remove_session_denied(&ip);
        drop(ac);
        if removed {
            broadcast_access_snapshots(&push_manager, false).await;
            info!("Removed {} from session denied list via API", ip);
            let response = serde_json::json!({
                "success": true,
                "message": format!("Removed {} from session denied list", ip)
            });
            Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "application/json")
                .body(full_body(response.to_string()))
                .unwrap()
        } else {
            error_response(
                StatusCode::NOT_FOUND,
                &format!("{} not found in session denied list", ip),
            )
        }
    } else {
        ac.clear_session_denied();
        drop(ac);
        broadcast_access_snapshots(&push_manager, false).await;
        info!("Cleared all session denied entries via API");
        let response = serde_json::json!({
            "success": true,
            "message": "Cleared all session denied entries"
        });
        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(full_body(response.to_string()))
            .unwrap()
    }
}
