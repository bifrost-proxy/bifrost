use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use serde::Deserialize;

use super::{error_response, full_body, BoxBody};
use crate::state::SharedAdminState;

pub async fn handle_power(
    req: Request<Incoming>,
    state: SharedAdminState,
    path: &str,
) -> Response<BoxBody> {
    let method = req.method().clone();
    match (path, method) {
        ("/api/power/status", Method::GET) | ("/api/power", Method::GET) => get_status(state).await,
        ("/api/power/on", Method::POST) => post_on(state).await,
        ("/api/power/off", Method::POST) => post_off(state).await,
        ("/api/power/mode", Method::POST) => post_mode(req, state).await,
        ("/api/power/remote-call", Method::POST) => post_remote_call(state).await,
        _ => error_response(StatusCode::NOT_FOUND, "Not Found"),
    }
}

async fn post_remote_call(state: SharedAdminState) -> Response<BoxBody> {
    let mgr = match require_manager(&state) {
        Some(m) => m,
        None => return manager_unavailable_response(),
    };
    mgr.on_remote_call();
    json_ok(&mgr.status())
}

fn require_manager(state: &SharedAdminState) -> Option<bifrost_power::SharedKeepAwakeManager> {
    state.keepawake_manager.clone()
}

fn manager_unavailable_response() -> Response<BoxBody> {
    error_response(
        StatusCode::SERVICE_UNAVAILABLE,
        "Keep-awake manager not configured",
    )
}

async fn get_status(state: SharedAdminState) -> Response<BoxBody> {
    let mgr = match require_manager(&state) {
        Some(m) => m,
        None => return manager_unavailable_response(),
    };
    let status = mgr.status();
    json_ok(&status)
}

async fn post_on(state: SharedAdminState) -> Response<BoxBody> {
    let mgr = match require_manager(&state) {
        Some(m) => m,
        None => return manager_unavailable_response(),
    };
    match mgr.set_mode(bifrost_power::Mode::ForceOn) {
        Ok(_) => json_ok(&mgr.status()),
        Err(e) => error_response(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

async fn post_off(state: SharedAdminState) -> Response<BoxBody> {
    let mgr = match require_manager(&state) {
        Some(m) => m,
        None => return manager_unavailable_response(),
    };
    match mgr.set_mode(bifrost_power::Mode::Off) {
        Ok(_) => json_ok(&mgr.status()),
        Err(e) => error_response(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

#[derive(Deserialize)]
struct ModeBody {
    mode: String,
}

async fn post_mode(req: Request<Incoming>, state: SharedAdminState) -> Response<BoxBody> {
    let mgr = match require_manager(&state) {
        Some(m) => m,
        None => return manager_unavailable_response(),
    };
    let bytes = match req.collect().await {
        Ok(c) => c.to_bytes(),
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read body: {e}"),
            )
        }
    };
    let body: ModeBody = match serde_json::from_slice(&bytes) {
        Ok(b) => b,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {e}")),
    };
    let mode: bifrost_power::Mode = match body.mode.parse() {
        Ok(m) => m,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid mode: {e}")),
    };
    match mgr.set_mode(mode) {
        Ok(_) => json_ok(&mgr.status()),
        Err(e) => error_response(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

fn json_ok<T: serde::Serialize>(v: &T) -> Response<BoxBody> {
    match serde_json::to_vec(v) {
        Ok(body) => Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(full_body(body))
            .unwrap(),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("serialize error: {e}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use http_body_util::BodyExt;

    use super::*;

    #[tokio::test]
    async fn remote_call_endpoint_returns_keepawake_status() {
        let manager = bifrost_power::KeepAwakeManager::new(
            bifrost_power::Mode::Auto,
            Arc::new(bifrost_power::NoopPersister),
        );
        let state = Arc::new(crate::state::AdminState::new(0).with_keepawake_manager(manager));

        let response = post_remote_call(state).await;

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            value.get("mode").and_then(|mode| mode.as_str()),
            Some("auto")
        );
    }

    #[tokio::test]
    async fn remote_call_endpoint_requires_manager() {
        let state = Arc::new(crate::state::AdminState::new(0));

        let response = post_remote_call(state).await;

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
