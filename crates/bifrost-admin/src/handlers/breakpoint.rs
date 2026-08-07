use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode, Uri};
use serde_json::json;

use crate::breakpoint::{BreakpointEdit, BreakpointResumeError, BreakpointSettings};
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
    } else if path == "/api/breakpoint/pending" {
        if method == Method::GET {
            json_response(&state.breakpoint_manager.pending())
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

    let value: serde_json::Value = match serde_json::from_slice(&body_bytes) {
        Ok(value) => value,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "Invalid JSON"),
    };
    let request_id = value
        .get("request_id")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    let phase = value
        .get("phase")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    let edit: BreakpointEdit = match serde_json::from_value(value) {
        Ok(edit) => edit,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "Invalid edit JSON"),
    };

    if request_id.trim().is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "Missing request_id");
    }
    if phase != "request" && phase != "response" {
        return error_response(StatusCode::BAD_REQUEST, "phase must be request or response");
    }
    if let Some(ref headers) = edit.headers {
        for (name, value) in headers {
            if hyper::header::HeaderName::from_bytes(name.as_bytes()).is_err() {
                return bad_request(format!("Invalid header name: {name}"));
            }
            if hyper::header::HeaderValue::from_str(value).is_err() {
                return bad_request(format!("Invalid header value for {name}"));
            }
        }
    }
    if phase == "request" {
        if edit.status.is_some() {
            return bad_request("status can only be edited during a response breakpoint");
        }
        if let Some(ref method) = edit.method {
            if Method::from_bytes(method.as_bytes()).is_err() {
                return error_response(StatusCode::BAD_REQUEST, "Invalid HTTP method");
            }
        }
        if let Some(ref url) = edit.url {
            let Ok(parsed) = url.parse::<Uri>() else {
                return bad_request("Invalid URL");
            };
            if !matches!(parsed.scheme_str(), Some("http" | "https"))
                || parsed.authority().is_none()
            {
                return bad_request("URL must be an absolute http or https URL");
            }
        }
    } else if edit.method.is_some() || edit.url.is_some() {
        return bad_request("method and url can only be edited during a request breakpoint");
    }
    if let Some(status) = edit.status {
        if StatusCode::from_u16(status).is_err() {
            return error_response(StatusCode::BAD_REQUEST, "Invalid HTTP status");
        }
    }

    if let Err(error) = state.breakpoint_manager.resume(&request_id, &phase, edit) {
        if matches!(error, BreakpointResumeError::PhaseMismatch) {
            return error_response(StatusCode::CONFLICT, "phase mismatch");
        }
        return error_response(StatusCode::NOT_FOUND, "not found");
    }
    if let Some(pm) = push_manager {
        pm.broadcast_breakpoint_resumed(request_id.clone(), phase.clone(), "resumed".into());
    }
    json_response(&json!({"resumed": true, "request_id": request_id, "phase": phase}))
}

fn bad_request(message: impl AsRef<str>) -> Response<BoxBody> {
    error_response(StatusCode::BAD_REQUEST, message.as_ref())
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
