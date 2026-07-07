use bifrost_storage::{SyncConfigUpdate, DEFAULT_REMOTE_BASE_URL};
use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use super::{
    empty_body, error_response, full_body, json_response, method_not_allowed,
    public_response_builder, BoxBody,
};
use crate::remote_invoke::{Identity, RemoteInvokeConfig, RemoteInvokeWorker};
use crate::state::SharedAdminState;

#[derive(Debug, Serialize)]
struct SyncLoginUrlResponse {
    login_url: String,
}

#[derive(Debug, Deserialize)]
struct UpdateSyncConfigRequest {
    enabled: Option<bool>,
    auto_sync: Option<bool>,
    remote_base_url: Option<String>,
    probe_interval_secs: Option<u64>,
    connect_timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct SaveSessionRequest {
    token: String,
}

#[derive(Debug, Deserialize, Default)]
struct LoginRequest {
    token: Option<String>,
    provider_id: Option<String>,
    remote_base_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LoginUrlQuery {
    callback_url: String,
}

#[derive(Debug, Deserialize, Default)]
struct RemoteSampleQuery {
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, Default)]
struct LoginCallbackQuery {
    token: Option<String>,
}

pub async fn handle_sync(
    req: Request<Incoming>,
    state: SharedAdminState,
    path: &str,
) -> Response<BoxBody> {
    let Some(sync_manager) = state.sync_manager.clone() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Sync manager not available",
        );
    };

    if path == "/api/sync/status" || path == "/api/sync/status/" {
        return match req.method() {
            &Method::GET => json_response(&sync_manager.status().await),
            _ => method_not_allowed(),
        };
    }

    if path == "/api/sync/config" || path == "/api/sync/config/" {
        return match req.method() {
            &Method::PUT => update_sync_config(req, state).await,
            _ => method_not_allowed(),
        };
    }

    if path.starts_with("/api/sync/login-url") {
        return match req.method() {
            &Method::GET => get_login_url(req, sync_manager).await,
            _ => method_not_allowed(),
        };
    }

    if path == "/api/sync/remote-sample" || path == "/api/sync/remote-sample/" {
        return match req.method() {
            &Method::GET => get_remote_sample(req, sync_manager).await,
            _ => method_not_allowed(),
        };
    }

    if path == "/api/sync/login" || path == "/api/sync/login/" {
        return match req.method() {
            &Method::POST => login(req, sync_manager, state).await,
            _ => method_not_allowed(),
        };
    }

    if path == "/api/sync/session" || path == "/api/sync/session/" {
        return match req.method() {
            &Method::POST => save_session(req, sync_manager, state).await,
            _ => method_not_allowed(),
        };
    }

    if let Some(provider_id) = provider_logout_id(path) {
        return match req.method() {
            &Method::POST => match sync_manager.logout_provider(provider_id).await {
                Ok(status) => {
                    reconcile_remote_invoke_workers(&state).await;
                    super::rules::notify_rules_changed_pub(&state);
                    json_response(&status)
                }
                Err(error) => error_response(
                    StatusCode::BAD_REQUEST,
                    &format!("Failed to clear sync provider session: {error}"),
                ),
            },
            _ => method_not_allowed(),
        };
    }

    if path == "/api/sync/logout" || path == "/api/sync/logout/" {
        return match req.method() {
            &Method::POST => match sync_manager.logout().await {
                Ok(status) => {
                    // The group id/name cache is a local navigation index for persisted
                    // group rule directories, not an auth credential. Keep it across
                    // logout so Badge and Rules deep links can still target the real
                    // group id after a logout/login cycle.
                    state.clear_group_cache_resolved();
                    reconcile_remote_invoke_workers(&state).await;
                    super::rules::notify_rules_changed_pub(&state);
                    json_response(&status)
                }
                Err(error) => error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("Failed to clear sync session: {error}"),
                ),
            },
            _ => method_not_allowed(),
        };
    }

    if path == "/api/sync/run" || path == "/api/sync/run/" {
        return match req.method() {
            &Method::POST => {
                sync_manager.trigger_sync();
                json_response(&sync_manager.status().await)
            }
            _ => method_not_allowed(),
        };
    }

    error_response(StatusCode::NOT_FOUND, "Not Found")
}

fn provider_logout_id(path: &str) -> Option<&str> {
    let path = path.trim_end_matches('/');
    let rest = path.strip_prefix("/api/sync/providers/")?;
    let provider_id = rest.strip_suffix("/logout")?;
    (!provider_id.is_empty()).then_some(provider_id)
}

pub async fn handle_sync_public(
    req: Request<Incoming>,
    state: SharedAdminState,
    path: &str,
) -> Response<BoxBody> {
    let Some(sync_manager) = state.sync_manager.clone() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Sync manager not available",
        );
    };

    match (req.method(), path) {
        (&Method::GET, "/public/sync-login") | (&Method::GET, "/public/sync-login/") => {
            let query = req.uri().query().unwrap_or_default();
            let parsed =
                serde_urlencoded::from_str::<LoginCallbackQuery>(query).unwrap_or_default();
            let html = if let Some(token) = parsed.token.filter(|value| !value.trim().is_empty()) {
                match sync_manager.save_token(token).await {
                    Ok(()) => {
                        notify_sync_session_saved(&state);
                        reconcile_remote_invoke_workers(&state).await;
                        let status = sync_manager.status().await;
                        render_sync_login_result_html(
                            true,
                            "Login completed. You can close this window now.",
                            status.user.as_ref().map(|user| user.user_id.as_str()),
                        )
                    }
                    Err(error) => render_sync_login_result_html(
                        false,
                        &format!("Failed to save sync session: {error}"),
                        None,
                    ),
                }
            } else {
                render_sync_login_result_html(
                    false,
                    "Missing login token from remote callback.",
                    None,
                )
            };
            public_response_builder(StatusCode::OK)
                .header("Content-Type", "text/html; charset=utf-8")
                .body(full_body(html))
                .unwrap()
        }
        (&Method::OPTIONS, "/public/sync-login") | (&Method::OPTIONS, "/public/sync-login/") => {
            public_response_builder(StatusCode::NO_CONTENT)
                .body(empty_body())
                .unwrap()
        }
        _ => error_response(StatusCode::NOT_FOUND, "Not Found"),
    }
}

pub async fn handle_sync_login_callback(
    req: Request<Incoming>,
    state: SharedAdminState,
) -> Response<BoxBody> {
    let Some(sync_manager) = state.sync_manager.clone() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Sync manager not available",
        );
    };

    let query = req.uri().query().unwrap_or_default();
    let parsed = serde_urlencoded::from_str::<LoginCallbackQuery>(query).unwrap_or_default();
    let html = if let Some(token) = parsed.token.filter(|value| !value.trim().is_empty()) {
        match sync_manager.save_token(token).await {
            Ok(()) => {
                notify_sync_session_saved(&state);
                reconcile_remote_invoke_workers(&state).await;
                let status = sync_manager.status().await;
                render_sync_login_result_html(
                    true,
                    "Login completed. You can close this window now.",
                    status.user.as_ref().map(|user| user.user_id.as_str()),
                )
            }
            Err(error) => render_sync_login_result_html(
                false,
                &format!("Failed to save sync session: {error}"),
                None,
            ),
        }
    } else {
        render_sync_login_result_html(false, "Missing login token from remote callback.", None)
    };

    public_response_builder(StatusCode::OK)
        .header("Content-Type", "text/html; charset=utf-8")
        .body(full_body(html))
        .unwrap()
}

async fn update_sync_config(req: Request<Incoming>, state: SharedAdminState) -> Response<BoxBody> {
    let Some(config_manager) = state.config_manager.clone() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Config manager not available",
        );
    };
    let body = match req.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read body: {error}"),
            )
        }
    };
    let request: UpdateSyncConfigRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(error) => {
            return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {error}"))
        }
    };

    if let Some(remote_base_url) = &request.remote_base_url {
        if remote_base_url.trim().is_empty() {
            return error_response(StatusCode::BAD_REQUEST, "remote_base_url cannot be empty");
        }
        if url::Url::parse(remote_base_url).is_err() {
            return error_response(
                StatusCode::BAD_REQUEST,
                "remote_base_url must be a valid URL",
            );
        }
    }

    match config_manager
        .update_sync_config(SyncConfigUpdate {
            enabled: request.enabled,
            auto_sync: request.auto_sync,
            remote_base_url: request.remote_base_url,
            probe_interval_secs: request.probe_interval_secs,
            connect_timeout_ms: request.connect_timeout_ms,
        })
        .await
    {
        Ok(_) => {
            reconcile_remote_invoke_workers(&state).await;

            if let Some(sync_manager) = state.sync_manager.clone() {
                sync_manager.trigger_sync();
                json_response(&sync_manager.status().await)
            } else {
                error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Sync manager not available",
                )
            }
        }
        Err(error) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to update sync config: {error}"),
        ),
    }
}

async fn get_login_url(
    req: Request<Incoming>,
    sync_manager: bifrost_sync::SharedSyncManager,
) -> Response<BoxBody> {
    let query = req.uri().query().unwrap_or_default();
    let parsed = match serde_urlencoded::from_str::<LoginUrlQuery>(query) {
        Ok(parsed) => parsed,
        Err(error) => {
            return error_response(StatusCode::BAD_REQUEST, &format!("Invalid query: {error}"))
        }
    };
    match sync_manager.login_url(&parsed.callback_url).await {
        Ok(login_url) => json_response(&SyncLoginUrlResponse { login_url }),
        Err(error) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to build sync login url: {error}"),
        ),
    }
}

async fn save_session(
    req: Request<Incoming>,
    sync_manager: bifrost_sync::SharedSyncManager,
    state: SharedAdminState,
) -> Response<BoxBody> {
    let body = match req.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read body: {error}"),
            )
        }
    };
    let request: SaveSessionRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(error) => {
            return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {error}"))
        }
    };
    if request.token.trim().is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "token is required");
    }
    match sync_manager.save_token(request.token).await {
        Ok(status) => {
            notify_sync_session_saved(&state);
            reconcile_remote_invoke_workers(&state).await;
            json_response(&status)
        }
        Err(error) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to save sync session: {error}"),
        ),
    }
}

async fn login(
    req: Request<Incoming>,
    sync_manager: bifrost_sync::SharedSyncManager,
    state: SharedAdminState,
) -> Response<BoxBody> {
    let body = match req.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read body: {error}"),
            )
        }
    };
    let request: LoginRequest = if body.is_empty() {
        LoginRequest::default()
    } else {
        match serde_json::from_slice(&body) {
            Ok(request) => request,
            Err(error) => {
                return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {error}"))
            }
        }
    };

    match (request.token, request.remote_base_url) {
        (Some(token), remote_base_url) => {
            if request.provider_id.as_deref() == Some("github_gist") {
                return match sync_manager.save_github_gist_session(token).await {
                    Ok(()) => {
                        notify_sync_session_saved(&state);
                        json_response(&sync_manager.status().await)
                    }
                    Err(error) => error_response(
                        StatusCode::BAD_REQUEST,
                        &format!("Failed to save GitHub Gist session: {error}"),
                    ),
                };
            }
            let remote_base_url = match remote_base_url {
                Some(remote_base_url) => remote_base_url,
                None => {
                    let configured = sync_manager.status().await.remote_base_url;
                    if configured.trim().is_empty() {
                        DEFAULT_REMOTE_BASE_URL.to_string()
                    } else {
                        configured
                    }
                }
            };
            match sync_manager
                .save_login_session(token, remote_base_url)
                .await
            {
                Ok(()) => {
                    notify_sync_session_saved(&state);
                    reconcile_remote_invoke_workers(&state).await;
                    json_response(&sync_manager.status().await)
                }
                Err(error) => error_response(
                    StatusCode::BAD_REQUEST,
                    &format!("Failed to save sync login session: {error}"),
                ),
            }
        }
        (None, None) => match sync_manager.request_login().await {
            Ok(()) => json_response(&sync_manager.status().await),
            Err(error) => error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to open sync login page: {error}"),
            ),
        },
        (None, Some(_)) => error_response(StatusCode::BAD_REQUEST, "token is required"),
    }
}

fn notify_sync_session_saved(state: &SharedAdminState) {
    state.clear_group_cache_resolved();
    super::rules::notify_rules_changed_pub(state);
}

async fn reconcile_remote_invoke_workers(state: &SharedAdminState) {
    let Some(sync_manager) = state.sync_manager.clone() else {
        return;
    };
    let targets = bifrost_sync::SyncManagerHandle::new(sync_manager.clone())
        .remote_invoke_registration_targets()
        .await;
    let desired_urls: Vec<String> = targets
        .iter()
        .map(|target| target.remote_base_url.trim_end_matches('/').to_string())
        .collect();

    state.stop_remote_invoke_workers_except(&desired_urls);
    if desired_urls.is_empty() {
        info!("remote invoke workers stopped because no provider session supports registration");
        return;
    }

    let Some((admin_host, admin_port)) = state.remote_invoke_admin_endpoint() else {
        warn!("remote invoke admin endpoint missing; skip provider worker reconcile");
        return;
    };
    let Some(config_manager) = state.config_manager.clone() else {
        warn!("config manager missing; skip provider worker reconcile");
        return;
    };
    let identity = match Identity::load_or_create(config_manager.data_dir()) {
        Ok(identity) => identity,
        Err(error) => {
            warn!(error = %error, "remote invoke identity init failed during provider reconcile");
            return;
        }
    };

    for target in targets {
        let relay_url = target.remote_base_url.trim_end_matches('/').to_string();
        if state
            .remote_invoke_worker_for_relay_url(&relay_url)
            .is_some()
        {
            continue;
        }
        let worker = RemoteInvokeWorker::new(
            RemoteInvokeConfig {
                relay_url: relay_url.clone(),
                ..Default::default()
            },
            identity.clone(),
            Some(bifrost_sync::SyncManagerHandle::new(sync_manager.clone())),
            state.clone(),
            &admin_host,
            admin_port,
        );
        state.upsert_remote_invoke_worker(worker.clone());
        worker.start();
        info!(
            provider_id = %target.provider_id,
            relay_url = %relay_url,
            "remote invoke provider worker started"
        );
    }
}

async fn get_remote_sample(
    req: Request<Incoming>,
    sync_manager: bifrost_sync::SharedSyncManager,
) -> Response<BoxBody> {
    let query = req.uri().query().unwrap_or_default();
    let parsed = serde_urlencoded::from_str::<RemoteSampleQuery>(query).unwrap_or_default();
    match sync_manager.remote_sample(parsed.limit.unwrap_or(10)).await {
        Ok(envs) => json_response(&envs),
        Err(error) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to fetch remote sync sample: {error}"),
        ),
    }
}

fn render_sync_login_result_html(success: bool, message: &str, user_id: Option<&str>) -> String {
    let title = if success {
        "Remote Sign-In Completed"
    } else {
        "Remote Sign-In Failed"
    };
    let status_class = if success { "success" } else { "error" };
    let detail = user_id
        .map(|value| format!(r#"<p class="detail">Signed in as <strong>{value}</strong>.</p>"#))
        .unwrap_or_default();
    let success_script = if success {
        r#"
      <script>
        window.opener?.postMessage(
          { type: "bifrost-sync-login-complete", redirect_to: "/" },
          window.location.origin,
        );
        window.setTimeout(() => {
          window.location.replace("/");
        }, 300);
      </script>"#
    } else {
        ""
    };
    format!(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>Bifrost Remote Sign-In</title>
    <style>
      :root {{
        color-scheme: light;
        font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
      }}
      body {{
        margin: 0;
        min-height: 100vh;
        display: grid;
        place-items: center;
        background: linear-gradient(180deg, #f4f7fb 0%, #eef2f8 100%);
        color: #1f2329;
      }}
      .card {{
        width: min(460px, calc(100vw - 32px));
        background: #fff;
        border-radius: 18px;
        padding: 28px;
        box-shadow: 0 18px 48px rgba(15, 23, 42, 0.12);
      }}
      h1 {{
        margin: 0 0 18px;
        font-size: 22px;
      }}
      .status {{
        border-radius: 14px;
        padding: 16px 18px;
        line-height: 1.6;
        background: #f7f9fc;
      }}
      .status.success {{
        background: #edf9f0;
        color: #166534;
      }}
      .status.error {{
        background: #fff1f0;
        color: #b42318;
      }}
      .detail {{
        margin: 12px 0 0;
        color: #475467;
      }}
    </style>
  </head>
  <body>
    <main class="card">
      <h1>{title}</h1>
      <div class="status {status_class}">{message}</div>
      {detail}
    </main>
    {success_script}
  </body>
</html>"#
    )
}

#[cfg(test)]
mod tests {
    use super::render_sync_login_result_html;

    #[test]
    fn sync_login_success_page_redirects_to_home() {
        let html = render_sync_login_result_html(true, "Login completed.", Some("tester"));

        assert!(html.contains("window.location.replace(\"/\")"));
        assert!(html.contains("bifrost-sync-login-complete"));
        assert!(html.contains("Signed in as <strong>tester</strong>."));
    }

    #[test]
    fn sync_login_error_page_does_not_redirect() {
        let html = render_sync_login_result_html(false, "Login failed.", None);

        assert!(!html.contains("window.location.replace(\"/\")"));
        assert!(html.contains("Login failed."));
    }
}
