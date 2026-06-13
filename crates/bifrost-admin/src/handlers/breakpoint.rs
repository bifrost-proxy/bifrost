use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use serde_json::json;

use crate::breakpoint::{BreakpointEdit, BreakpointSettings};
use crate::handlers::{error_response, json_response, BoxBody};
use crate::push::SharedPushManager;
use crate::state::SharedAdminState;

pub async fn handle_breakpoint(
    req: Request<Incoming>,
    state: SharedAdminState,
    push_manager: Option<SharedPushManager>,
    path: &str,
) -> Response<BoxBody> {
    let method = req.method().clone();

    if path == "/api/breakpoint/settings" {
        match method {
            Method::GET => get_settings(&state),
            Method::POST => update_settings(req, &state, push_manager).await,
            _ => error_response(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed"),
        }
    } else if path == "/api/breakpoint/resume" {
        if method == Method::POST {
            resume(req, &state, push_manager).await
        } else {
            error_response(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed")
        }
    } else {
        error_response(StatusCode::NOT_FOUND, "Not found")
    }
}

fn get_settings(state: &SharedAdminState) -> Response<BoxBody> {
    let settings = state.breakpoint_manager.get_settings();
    json_response(&settings)
}

async fn update_settings(
    req: Request<Incoming>,
    state: &SharedAdminState,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    let body_bytes = match req.into_body().collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "Failed to read body"),
    };

    let settings: BreakpointSettings = match serde_json::from_slice(&body_bytes) {
        Ok(s) => s,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "Invalid JSON"),
    };

    state.breakpoint_manager.update_settings(settings);
    let effective_settings = state.breakpoint_manager.get_settings();

    if let Some(pm) = push_manager {
        pm.broadcast_breakpoint_settings_updated(crate::push::BreakpointSettingsPushData {
            enabled: effective_settings.enabled,
            max_body_bytes: effective_settings.max_body_bytes,
        });
    }

    json_response(&effective_settings)
}

async fn resume(
    req: Request<Incoming>,
    state: &SharedAdminState,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    let body_bytes = match req.into_body().collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "Failed to read body"),
    };

    let body: serde_json::Value = match serde_json::from_slice(&body_bytes) {
        Ok(v) => v,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "Invalid JSON"),
    };

    let request_id = match body.get("request_id").and_then(|v| v.as_str()) {
        Some(id) => id.to_string(),
        None => return error_response(StatusCode::BAD_REQUEST, "Missing request_id"),
    };

    let phase = body
        .get("phase")
        .and_then(|v| v.as_str())
        .unwrap_or("request")
        .to_string();

    let headers: Vec<(String, String)> = body
        .get("headers")
        .and_then(|v| {
            v.as_array().map(|arr| {
                arr.iter()
                    .filter_map(|item| {
                        let pair = item.as_array()?;
                        Some((
                            pair.first()?.as_str()?.to_string(),
                            pair.get(1)?.as_str()?.to_string(),
                        ))
                    })
                    .collect()
            })
        })
        .unwrap_or_default();

    let edit_body: Option<String> = body
        .get("body")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let edit = BreakpointEdit {
        headers,
        body: edit_body,
    };

    let resumed = state.breakpoint_manager.resume(&request_id, &phase, edit);

    if resumed {
        if let Some(pm) = push_manager {
            pm.broadcast_breakpoint_resumed(request_id.clone());
        }
        json_response(&json!({"resumed": true, "request_id": request_id}))
    } else {
        error_response(
            StatusCode::NOT_FOUND,
            "No pending breakpoint for this request",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AdminState;
    use http_body_util::BodyExt;

    #[tokio::test]
    async fn get_settings_reflects_breakpoint_manager_state() {
        let state = std::sync::Arc::new(AdminState::new(0));
        state
            .breakpoint_manager
            .update_settings(BreakpointSettings {
                enabled: true,
                max_body_bytes: 42,
            });

        let resp = get_settings(&state);
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let settings: BreakpointSettings = serde_json::from_slice(&body).unwrap();
        assert!(settings.enabled);
        assert_eq!(settings.max_body_bytes, 42);
    }
}
