use std::collections::HashSet;
use std::sync::Arc;

use bifrost_storage::{RemoteShellSet, RemoteShellStore};
use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};

use bifrost_core::BifrostError;

use crate::handlers::{error_response, json_response, method_not_allowed, BoxBody};
use crate::remote_invoke::types::{
    CommandKind, FileAccessScope, GrantMode, GrantScope, RemoteCommand, ShellExecMode,
};
use crate::remote_invoke::worker::RemoteInvokeWorker;

pub type SharedRemoteInvokeWorker = Arc<RemoteInvokeWorker>;

pub async fn handle_remote_invoke(
    req: Request<Incoming>,
    worker: Option<SharedRemoteInvokeWorker>,
    path: &str,
) -> Response<BoxBody> {
    let Some(worker) = worker else {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "Remote invoke not enabled");
    };

    let sub = path.strip_prefix("/api/remote-invoke").unwrap_or(path);

    if sub == "/status" || sub == "/status/" {
        return handle_status(&req, &worker);
    }
    if sub == "/identity" || sub == "/identity/" {
        return handle_identity(&req, &worker);
    }
    if sub == "/discovery/enter" || sub == "/discovery/enter/" {
        return handle_discovery_enter(req, &worker).await;
    }
    if sub == "/discovery/exit" || sub == "/discovery/exit/" {
        return handle_discovery_exit(req, &worker).await;
    }
    if sub == "/discovery/refresh" || sub == "/discovery/refresh/" {
        return handle_discovery_refresh(req, &worker).await;
    }
    if sub == "/pairings/pending" || sub == "/pairings/pending/" {
        return handle_pairings_pending(&req, &worker);
    }
    if let Some(rest) = sub.strip_prefix("/pairings/") {
        if let Some(pairing_id) = rest.strip_suffix("/approve") {
            return handle_pairing_approve(req, &worker, pairing_id).await;
        }
        if let Some(pairing_id) = rest.strip_suffix("/approve/") {
            return handle_pairing_approve(req, &worker, pairing_id).await;
        }
        if let Some(pairing_id) = rest.strip_suffix("/reject") {
            return handle_pairing_reject(req, &worker, pairing_id).await;
        }
        if let Some(pairing_id) = rest.strip_suffix("/reject/") {
            return handle_pairing_reject(req, &worker, pairing_id).await;
        }
    }
    if sub == "/grants" || sub == "/grants/" {
        return handle_grants_list(req, &worker).await;
    }
    if sub == "/shell-config" || sub == "/shell-config/" {
        return handle_shell_config(req).await;
    }
    if sub == "/shell-config/match" || sub == "/shell-config/match/" {
        return handle_shell_config_match(req, &worker).await;
    }
    if sub == "/file-access-config" || sub == "/file-access-config/" {
        return handle_file_access_config(req).await;
    }
    if sub == "/file-access/validate-path" || sub == "/file-access/validate-path/" {
        return handle_file_access_validate_path(req).await;
    }
    if let Some(rest) = sub.strip_prefix("/file-access/grants/") {
        let rest = rest.trim_end_matches('/');
        if let Some(grant_id) = rest.strip_suffix("/roots") {
            if !grant_id.is_empty() && !grant_id.contains('/') {
                return handle_file_access_grant_roots(req, grant_id).await;
            }
        }
    }
    if let Some(rest) = sub.strip_prefix("/grants/") {
        let grant_id = rest.trim_end_matches('/');
        if !grant_id.is_empty() && !grant_id.contains('/') {
            return handle_grant_action(req, &worker, grant_id).await;
        }
    }
    if sub == "/calls" || sub == "/calls/" {
        return handle_calls_list(req, &worker).await;
    }
    if sub == "/ssh-key"
        || sub == "/ssh-key/"
        || sub == "/ssh-key/private-key"
        || sub == "/ssh-key/private-key/"
        || sub == "/ssh-key/reset"
        || sub == "/ssh-key/reset/"
    {
        return handle_ssh_key(req, &worker, sub).await;
    }
    if let Some(rest) = sub.strip_prefix("/calls/") {
        let call_id = rest.trim_end_matches('/');
        if !call_id.is_empty() && !call_id.contains('/') {
            return handle_call_get(req, &worker, call_id).await;
        }
    }

    error_response(StatusCode::NOT_FOUND, "Remote invoke endpoint not found")
}

fn handle_status(req: &Request<Incoming>, worker: &RemoteInvokeWorker) -> Response<BoxBody> {
    if req.method() != Method::GET {
        return method_not_allowed();
    }

    let state = worker.state();
    let discovery = worker.discovery_session();
    let pending_count = worker.pending_pairings().len();
    let active_calls = worker.active_call_ids();

    json_response(&serde_json::json!({
        "state": format!("{:?}", state),
        "discovery_session": discovery,
        "pending_pairings_count": pending_count,
        "active_call_ids": active_calls,
    }))
}

fn handle_identity(req: &Request<Incoming>, worker: &RemoteInvokeWorker) -> Response<BoxBody> {
    if req.method() != Method::GET {
        return method_not_allowed();
    }

    let identity = worker.identity();
    json_response(&serde_json::json!({
        "instance_id": identity.instance_id,
        "device_name": identity.device_name,
        "platform": identity.platform,
    }))
}

async fn handle_discovery_enter(
    req: Request<Incoming>,
    worker: &RemoteInvokeWorker,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }

    match worker.enter_discovery_mode().await {
        Ok(session) => json_response(&serde_json::json!({
            "success": true,
            "session": session,
        })),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to enter discovery mode: {e}"),
        ),
    }
}

async fn handle_discovery_exit(
    req: Request<Incoming>,
    worker: &RemoteInvokeWorker,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }

    match worker.exit_discovery_mode().await {
        Ok(()) => json_response(&serde_json::json!({
            "success": true,
        })),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to exit discovery mode: {e}"),
        ),
    }
}

async fn handle_discovery_refresh(
    req: Request<Incoming>,
    worker: &RemoteInvokeWorker,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }

    match worker.refresh_pair_code().await {
        Ok(Some(session)) => json_response(&serde_json::json!({
            "success": true,
            "session": session,
        })),
        Ok(None) => error_response(StatusCode::BAD_REQUEST, "Not in discovery mode"),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to refresh pair code: {e}"),
        ),
    }
}

fn handle_pairings_pending(
    req: &Request<Incoming>,
    worker: &RemoteInvokeWorker,
) -> Response<BoxBody> {
    if req.method() != Method::GET {
        return method_not_allowed();
    }

    let pairings = worker.pending_pairings();
    json_response(&serde_json::json!({
        "pairings": pairings,
    }))
}

async fn handle_pairing_approve(
    req: Request<Incoming>,
    worker: &RemoteInvokeWorker,
    pairing_id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }

    let body = match req.collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read request body: {e}"),
            );
        }
    };

    #[derive(serde::Deserialize)]
    struct ApproveBody {
        grant_mode: GrantMode,
        #[serde(default)]
        grant_scope: Option<GrantScope>,
        #[serde(default)]
        file_access: Option<FileAccessScope>,
        #[serde(default)]
        policy_binding: Option<serde_json::Value>,
        #[serde(default)]
        interactive_allowed: Option<bool>,
        #[serde(default)]
        stdin_allowed: Option<bool>,
    }

    let parsed: ApproveBody = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Invalid request body: {e}"),
            );
        }
    };

    match worker
        .approve_pairing(
            pairing_id,
            parsed.grant_mode,
            parsed.grant_scope,
            parsed.file_access,
            parsed.policy_binding,
            parsed.interactive_allowed,
            parsed.stdin_allowed,
        )
        .await
    {
        Ok(result) => json_response(&serde_json::json!({
            "success": true,
            "data": result,
        })),
        Err(e) => error_response(
            pairing_action_status_code(&e),
            &format!("Failed to approve pairing: {e}"),
        ),
    }
}

async fn handle_pairing_reject(
    req: Request<Incoming>,
    worker: &RemoteInvokeWorker,
    pairing_id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }

    match worker.reject_pairing(pairing_id).await {
        Ok(result) => json_response(&serde_json::json!({
            "success": true,
            "data": result,
        })),
        Err(e) => error_response(
            pairing_action_status_code(&e),
            &format!("Failed to reject pairing: {e}"),
        ),
    }
}

fn pairing_action_status_code(error: &BifrostError) -> StatusCode {
    if let BifrostError::Network(message) = error {
        if message.contains("not found or expired")
            || message.contains("pairing_expired")
            || message.contains("pairing_not_found")
            || message.contains("pairing_not_pending")
        {
            return StatusCode::NOT_FOUND;
        }
    }
    StatusCode::INTERNAL_SERVER_ERROR
}

async fn handle_grants_list(
    req: Request<Incoming>,
    worker: &RemoteInvokeWorker,
) -> Response<BoxBody> {
    if req.method() != Method::GET {
        return method_not_allowed();
    }

    let grants = worker.list_grants_and_cleanup();
    json_response(&serde_json::json!({
        "grants": grants,
    }))
}

async fn handle_shell_config(req: Request<Incoming>) -> Response<BoxBody> {
    match *req.method() {
        Method::GET => {
            let store = match RemoteShellStore::new() {
                Ok(store) => store,
                Err(error) => {
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        &format!("Failed to open remote shell store: {error}"),
                    );
                }
            };

            match store.load() {
                Ok(set) => json_response(&set),
                Err(error) => error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("Failed to load remote shell config: {error}"),
                ),
            }
        }
        Method::PUT => {
            let body = match req.collect().await {
                Ok(body) => body.to_bytes(),
                Err(error) => {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        &format!("Failed to read request body: {error}"),
                    );
                }
            };

            let requested: RemoteShellSet = match serde_json::from_slice(&body) {
                Ok(value) => value,
                Err(error) => {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        &format!("Invalid request body: {error}"),
                    );
                }
            };

            if let Err(message) = validate_remote_shell_set(&requested) {
                return error_response(StatusCode::BAD_REQUEST, &message);
            }

            let store = match RemoteShellStore::new() {
                Ok(store) => store,
                Err(error) => {
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        &format!("Failed to open remote shell store: {error}"),
                    );
                }
            };
            let current = match store.load() {
                Ok(current) => current,
                Err(error) => {
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        &format!("Failed to load current remote shell config: {error}"),
                    );
                }
            };

            let next = prepare_remote_shell_set_for_save(current, requested);
            match store.save(&next) {
                Ok(()) => json_response(&next),
                Err(error) => error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("Failed to save remote shell config: {error}"),
                ),
            }
        }
        _ => method_not_allowed(),
    }
}

async fn handle_shell_config_match(
    req: Request<Incoming>,
    worker: &RemoteInvokeWorker,
) -> Response<BoxBody> {
    if req.method() != Method::POST {
        return method_not_allowed();
    }

    let body = match req.collect().await {
        Ok(body) => body.to_bytes(),
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read request body: {error}"),
            );
        }
    };

    #[derive(serde::Deserialize)]
    struct MatchRequest {
        #[serde(default)]
        command: String,
        exec_mode: ShellExecMode,
        #[serde(default)]
        argv: Option<Vec<String>>,
    }

    let input: MatchRequest = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Invalid request body: {error}"),
            );
        }
    };

    let mut cmd = RemoteCommand {
        kind: CommandKind::ShellExec,
        command: "shell.exec".to_string(),
        exec_mode: Some(input.exec_mode),
        ..Default::default()
    };
    match input.exec_mode {
        ShellExecMode::ArgvExec => {
            let argv = input.argv.filter(|v| !v.is_empty()).or_else(|| {
                let trimmed = input.command.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(
                        trimmed
                            .split_whitespace()
                            .map(str::to_owned)
                            .collect::<Vec<_>>(),
                    )
                }
            });
            cmd.argv = argv;
        }
        ShellExecMode::ShellText | ShellExecMode::Template => {
            cmd.command_text = Some(input.command.clone());
        }
    }

    let (matched, matched_policy_id, reason) =
        match worker.executor().select_policy_id_for_command(&cmd, None) {
            Ok(policy_id) => {
                let reason = format!("matched policy '{}'", policy_id);
                (true, Some(policy_id), reason)
            }
            Err(error) => (false, None, error.to_string()),
        };

    let response = serde_json::json!({
        "matched": matched,
        "matched_policy_id": matched_policy_id,
        "reason": reason,
    });
    json_response(&response)
}

async fn handle_file_access_config(req: Request<Incoming>) -> Response<BoxBody> {
    use crate::remote_invoke::file_policy_store::{load_raw_config, save_raw_config, RawConfig};

    match *req.method() {
        Method::GET => {
            let config = load_raw_config();
            json_response(&config)
        }
        Method::PUT => {
            let body = match req.collect().await {
                Ok(body) => body.to_bytes(),
                Err(error) => {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        &format!("Failed to read request body: {error}"),
                    );
                }
            };

            let config: RawConfig = match serde_json::from_slice(&body) {
                Ok(value) => value,
                Err(error) => {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        &format!("Invalid request body: {error}"),
                    );
                }
            };

            if let Err(msg) = save_raw_config(&config) {
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, &msg);
            }

            json_response(&config)
        }
        _ => method_not_allowed(),
    }
}

async fn handle_grant_action(
    req: Request<Incoming>,
    worker: &RemoteInvokeWorker,
    grant_id: &str,
) -> Response<BoxBody> {
    match *req.method() {
        Method::PATCH => {
            let body = match req.collect().await {
                Ok(b) => b.to_bytes(),
                Err(e) => {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        &format!("Failed to read request body: {e}"),
                    );
                }
            };

            #[derive(serde::Deserialize)]
            struct UpdateGrantBody {
                #[serde(default)]
                grant_scope: Option<GrantScope>,
                #[serde(default)]
                file_access: Option<FileAccessScope>,
                #[serde(default)]
                policy_binding: Option<serde_json::Value>,
                #[serde(default)]
                interactive_allowed: Option<bool>,
                #[serde(default)]
                stdin_allowed: Option<bool>,
            }

            let parsed: UpdateGrantBody = match serde_json::from_slice(&body) {
                Ok(v) => v,
                Err(e) => {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        &format!("Invalid request body: {e}"),
                    );
                }
            };

            match worker
                .update_grant(
                    grant_id,
                    parsed.grant_scope,
                    parsed.file_access,
                    parsed.policy_binding,
                    parsed.interactive_allowed,
                    parsed.stdin_allowed,
                )
                .await
            {
                Ok(result) => json_response(&serde_json::json!({
                    "success": true,
                    "data": result,
                })),
                Err(e) => {
                    let msg = e.to_string();
                    let status = match &e {
                        BifrostError::NotFound(_) => StatusCode::NOT_FOUND,
                        // Relay returned 403 grant_not_active / 404 — local cache is stale.
                        // Use 409 Conflict so the frontend can auto-refresh the grants list.
                        BifrostError::Network(inner)
                            if inner.contains("grant_not_active")
                                || inner.contains("403 Forbidden")
                                || inner.contains("404 Not Found") =>
                        {
                            StatusCode::CONFLICT
                        }
                        _ => StatusCode::BAD_REQUEST,
                    };
                    error_response(status, &format!("Failed to update grant: {msg}"))
                }
            }
        }
        Method::DELETE => match worker.delete_grant(grant_id).await {
            Ok(()) => json_response(&serde_json::json!({
                "success": true,
            })),
            Err(e) => error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to revoke grant: {e}"),
            ),
        },
        _ => method_not_allowed(),
    }
}

async fn handle_calls_list(
    req: Request<Incoming>,
    worker: &RemoteInvokeWorker,
) -> Response<BoxBody> {
    match *req.method() {
        Method::GET => {
            let calls = worker.list_calls();
            json_response(&serde_json::json!({
                "calls": calls,
            }))
        }
        Method::DELETE => {
            let removed = worker.clear_calls();
            json_response(&serde_json::json!({
                "success": true,
                "removed": removed,
            }))
        }
        _ => method_not_allowed(),
    }
}

async fn handle_call_get(
    req: Request<Incoming>,
    worker: &RemoteInvokeWorker,
    call_id: &str,
) -> Response<BoxBody> {
    if req.method() != Method::GET {
        return method_not_allowed();
    }

    match worker.get_call(call_id) {
        Some(call) => json_response(&serde_json::json!({
            "call": call,
        })),
        None => error_response(StatusCode::NOT_FOUND, "Call not found"),
    }
}

async fn handle_ssh_key(
    req: Request<Incoming>,
    worker: &RemoteInvokeWorker,
    sub: &str,
) -> Response<BoxBody> {
    match (req.method().clone(), sub) {
        (Method::GET, "/ssh-key/private-key" | "/ssh-key/private-key/") => {
            match worker.export_active_ssh_key() {
                Ok(Some(material)) => json_response(&material),
                Ok(None) => json_response(&serde_json::Value::Null),
                Err(e) => error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("Failed to load ssh key file: {e}"),
                ),
            }
        }
        (Method::POST, "/ssh-key/reset" | "/ssh-key/reset/") => match worker.reset_ssh_key() {
            Ok(Some(material)) => json_response(&material),
            Ok(None) => error_response(StatusCode::NOT_FOUND, "SSH key not found"),
            Err(e) => error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to reset ssh key: {e}"),
            ),
        },
        (Method::GET, "/ssh-key" | "/ssh-key/") => match worker.get_active_ssh_key() {
            Ok(Some(record)) => json_response(&record),
            Ok(None) => json_response(&serde_json::Value::Null),
            Err(e) => error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to load ssh key: {e}"),
            ),
        },
        (Method::POST, "/ssh-key" | "/ssh-key/") => {
            #[derive(serde::Deserialize)]
            struct CreateBody {
                label: String,
                grant_mode: GrantMode,
            }

            let parsed: CreateBody = match parse_json_body(req).await {
                Ok(body) => body,
                Err(response) => return response,
            };

            match worker.create_ssh_key(parsed.label, parsed.grant_mode) {
                Ok(result) => json_response(&result),
                Err(e) => error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("Failed to create ssh key: {e}"),
                ),
            }
        }
        (Method::PATCH, "/ssh-key" | "/ssh-key/") => {
            #[derive(serde::Deserialize)]
            struct UpdateBody {
                label: Option<String>,
                grant_mode: Option<GrantMode>,
            }

            let parsed: UpdateBody = match parse_json_body(req).await {
                Ok(body) => body,
                Err(response) => return response,
            };

            match worker.update_ssh_key(parsed.label, parsed.grant_mode) {
                Ok(Some(record)) => json_response(&record),
                Ok(None) => error_response(StatusCode::NOT_FOUND, "SSH key not found"),
                Err(e) => error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("Failed to update ssh key: {e}"),
                ),
            }
        }
        (Method::DELETE, "/ssh-key" | "/ssh-key/") => match worker.revoke_ssh_key() {
            Ok(_) => json_response(&serde_json::json!({
                "success": true,
            })),
            Err(e) => error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to revoke ssh key: {e}"),
            ),
        },
        _ => method_not_allowed(),
    }
}

async fn parse_json_body<T: serde::de::DeserializeOwned>(
    req: Request<Incoming>,
) -> std::result::Result<T, Response<BoxBody>> {
    let body = req.collect().await.map_err(|e| {
        error_response(
            StatusCode::BAD_REQUEST,
            &format!("Failed to read request body: {e}"),
        )
    })?;
    serde_json::from_slice(&body.to_bytes()).map_err(|e| {
        error_response(
            StatusCode::BAD_REQUEST,
            &format!("Invalid request body: {e}"),
        )
    })
}

fn validate_remote_shell_set(set: &RemoteShellSet) -> Result<(), String> {
    let mut policy_ids = HashSet::new();
    let mut profile_ids = HashSet::new();

    for profile in &set.profiles {
        let profile_id = profile.id.trim();
        if profile_id.is_empty() {
            return Err("remote shell profile id cannot be empty".to_string());
        }
        if profile.name.trim().is_empty() {
            return Err(format!(
                "remote shell profile '{profile_id}' name cannot be empty"
            ));
        }
        if !profile_ids.insert(profile_id.to_string()) {
            return Err(format!("duplicate remote shell profile id '{profile_id}'"));
        }
    }

    for policy in &set.policies {
        let policy_id = policy.id.trim();
        if policy_id.is_empty() {
            return Err("remote shell policy id cannot be empty".to_string());
        }
        if policy.name.trim().is_empty() {
            return Err(format!(
                "remote shell policy '{policy_id}' name cannot be empty"
            ));
        }
        if !policy_ids.insert(policy_id.to_string()) {
            return Err(format!("duplicate remote shell policy id '{policy_id}'"));
        }
        if let Some(profile_id) = policy.profile_id.as_deref().map(str::trim) {
            if !profile_id.is_empty() && !profile_ids.contains(profile_id) {
                return Err(format!(
                    "remote shell policy '{policy_id}' references missing profile '{profile_id}'"
                ));
            }
        }
    }

    Ok(())
}

fn prepare_remote_shell_set_for_save(
    current: RemoteShellSet,
    mut requested: RemoteShellSet,
) -> RemoteShellSet {
    const DEFAULT_SCHEMA_VERSION: u64 = 1;

    if requested.schema_version == 0 {
        requested.schema_version = current.schema_version.max(DEFAULT_SCHEMA_VERSION);
    }

    let same_payload = {
        let mut lhs = requested.clone();
        let mut rhs = current.clone();
        lhs.version = 0;
        rhs.version = 0;
        lhs == rhs
    };

    requested.version = if same_payload {
        current.version.max(requested.version)
    } else if requested.version <= current.version {
        current.version.saturating_add(1)
    } else {
        requested.version
    };

    requested
}

async fn handle_file_access_validate_path(req: Request<Incoming>) -> Response<BoxBody> {
    use crate::remote_invoke::file_access_roots;

    if req.method() != Method::POST {
        return method_not_allowed();
    }
    let body = match req.collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read request body: {e}"),
            );
        }
    };
    #[derive(serde::Deserialize)]
    struct Body {
        path: String,
    }
    let parsed: Body = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Invalid request body: {e}"),
            );
        }
    };
    let result = file_access_roots::validate_path(std::path::Path::new(&parsed.path));
    json_response(&result)
}

async fn handle_file_access_grant_roots(
    req: Request<Incoming>,
    grant_id: &str,
) -> Response<BoxBody> {
    use crate::remote_invoke::file_access_roots;

    #[derive(serde::Deserialize)]
    struct PathBody {
        path: String,
    }

    match *req.method() {
        Method::GET => match file_access_roots::list_roots(grant_id) {
            Ok(roots) => json_response(&serde_json::json!({
                "success": true,
                "grant_id": grant_id,
                "roots": roots,
            })),
            Err(e) => {
                let status = match &e {
                    file_access_roots::RootEditError::GrantNotFound(_) => StatusCode::NOT_FOUND,
                    _ => StatusCode::INTERNAL_SERVER_ERROR,
                };
                error_response(status, &e.to_string())
            }
        },
        Method::POST => {
            let body = match req.collect().await {
                Ok(b) => b.to_bytes(),
                Err(e) => {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        &format!("Failed to read request body: {e}"),
                    );
                }
            };
            let parsed: PathBody = match serde_json::from_slice(&body) {
                Ok(v) => v,
                Err(e) => {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        &format!("Invalid request body: {e}"),
                    );
                }
            };
            match file_access_roots::add_root(grant_id, std::path::Path::new(&parsed.path)) {
                Ok(outcome) => json_response(&serde_json::json!({
                    "success": true,
                    "data": outcome,
                })),
                Err(e) => {
                    let status = match &e {
                        file_access_roots::RootEditError::GrantNotFound(_) => StatusCode::NOT_FOUND,
                        file_access_roots::RootEditError::InvalidPath(_) => StatusCode::BAD_REQUEST,
                        _ => StatusCode::INTERNAL_SERVER_ERROR,
                    };
                    error_response(status, &e.to_string())
                }
            }
        }
        Method::DELETE => {
            let body = match req.collect().await {
                Ok(b) => b.to_bytes(),
                Err(e) => {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        &format!("Failed to read request body: {e}"),
                    );
                }
            };
            let parsed: PathBody = match serde_json::from_slice(&body) {
                Ok(v) => v,
                Err(e) => {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        &format!("Invalid request body: {e}"),
                    );
                }
            };
            match file_access_roots::remove_root(grant_id, std::path::Path::new(&parsed.path)) {
                Ok(removed) => json_response(&serde_json::json!({
                    "success": true,
                    "removed": removed,
                })),
                Err(e) => {
                    let status = match &e {
                        file_access_roots::RootEditError::GrantNotFound(_) => StatusCode::NOT_FOUND,
                        _ => StatusCode::INTERNAL_SERVER_ERROR,
                    };
                    error_response(status, &e.to_string())
                }
            }
        }
        _ => method_not_allowed(),
    }
}

#[cfg(test)]
mod tests {
    use bifrost_core::BifrostError;
    use bifrost_storage::{RemoteShellPolicy, RemoteShellProfile};
    use serde_json::json;

    use super::{
        pairing_action_status_code, prepare_remote_shell_set_for_save, validate_remote_shell_set,
    };
    use bifrost_storage::RemoteShellSet;
    use hyper::StatusCode;

    fn sample_set() -> RemoteShellSet {
        RemoteShellSet {
            schema_version: 1,
            version: 2,
            policies: vec![RemoteShellPolicy {
                id: "echo-argv".to_string(),
                name: "Echo argv".to_string(),
                description: None,
                enabled: true,
                profile_id: Some("safe".to_string()),
                metadata: json!({ "exec_mode": "argv_exec" }),
            }],
            profiles: vec![RemoteShellProfile {
                id: "safe".to_string(),
                name: "Safe".to_string(),
                description: None,
                enabled: true,
                metadata: json!({ "cwd_allowlist": ["/tmp"] }),
            }],
        }
    }

    #[test]
    fn validate_remote_shell_set_rejects_missing_profile_reference() {
        let mut set = sample_set();
        set.policies[0].profile_id = Some("missing".to_string());

        let error = validate_remote_shell_set(&set).expect_err("should reject missing profile");
        assert!(error.contains("references missing profile"));
    }

    #[test]
    fn prepare_remote_shell_set_for_save_bumps_version_when_payload_changes() {
        let current = sample_set();
        let mut requested = current.clone();
        requested.policies[0].name = "Echo argv updated".to_string();
        requested.version = current.version;

        let prepared = prepare_remote_shell_set_for_save(current, requested);
        assert_eq!(prepared.version, 3);
    }

    #[test]
    fn prepare_remote_shell_set_for_save_keeps_version_when_payload_is_same() {
        let current = sample_set();
        let requested = current.clone();

        let prepared = prepare_remote_shell_set_for_save(current, requested);
        assert_eq!(prepared.version, 2);
    }

    #[test]
    fn pairing_action_status_code_maps_stale_pairings_to_not_found() {
        let stale = BifrostError::Network("pairing abc not found or expired".to_string());
        let internal =
            BifrostError::Network("relay submit_grant_decision unauthorized".to_string());

        assert_eq!(pairing_action_status_code(&stale), StatusCode::NOT_FOUND);
        assert_eq!(
            pairing_action_status_code(&internal),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
