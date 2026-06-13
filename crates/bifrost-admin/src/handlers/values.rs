use bifrost_storage::ConfigChangeEvent;
use hyper::{Method, Request, Response, StatusCode};
use serde::{Deserialize, Serialize};

use super::{
    cors_preflight, error_response, json_response, method_not_allowed, success_response, BoxBody,
};
use crate::state::SharedAdminState;

#[derive(Debug, Serialize)]
pub struct ValueItem {
    pub name: String,
    pub value: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize)]
pub struct ValuesListResponse {
    pub values: Vec<ValueItem>,
    pub total: usize,
}

#[derive(Debug, Deserialize)]
pub struct CreateValueRequest {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateValueRequest {
    pub value: String,
}

pub async fn handle_values<B>(
    req: Request<B>,
    state: SharedAdminState,
    path_suffix: &str,
) -> Response<BoxBody>
where
    B: hyper::body::Body + Send + 'static,
    B::Data: Send,
    B::Error: std::error::Error + Send + Sync,
{
    if req.method() == Method::OPTIONS {
        return cors_preflight();
    }

    let storage = match &state.values_storage {
        Some(s) => s,
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Values storage not configured",
            )
        }
    };

    if path_suffix.is_empty() || path_suffix == "/" {
        match *req.method() {
            Method::GET => list_values(storage),
            Method::POST => create_value(req, storage, &state).await,
            _ => method_not_allowed(),
        }
    } else {
        let name = path_suffix.trim_start_matches('/');
        if name.is_empty() {
            return error_response(StatusCode::BAD_REQUEST, "Value name is required");
        }
        match *req.method() {
            Method::GET => get_value(name, storage),
            Method::PUT => update_value(req, name, storage, &state).await,
            Method::DELETE => delete_value(name, storage, &state),
            _ => method_not_allowed(),
        }
    }
}

fn list_values(storage: &crate::state::SharedValuesStorage) -> Response<BoxBody> {
    let guard = storage.read();
    match guard.list_entries() {
        Ok(entries) => {
            let values: Vec<ValueItem> = entries
                .into_iter()
                .map(|e| ValueItem {
                    name: e.name,
                    value: e.value,
                    created_at: e.created_at,
                    updated_at: e.updated_at,
                })
                .collect();
            let total = values.len();
            json_response(&ValuesListResponse { values, total })
        }
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to list values: {}", e),
        ),
    }
}

fn get_value(name: &str, storage: &crate::state::SharedValuesStorage) -> Response<BoxBody> {
    let guard = storage.read();
    match guard.get_entry(name) {
        Some(entry) => json_response(&ValueItem {
            name: entry.name,
            value: entry.value,
            created_at: entry.created_at,
            updated_at: entry.updated_at,
        }),
        None => error_response(
            StatusCode::NOT_FOUND,
            &format!("Value '{}' not found", name),
        ),
    }
}

async fn create_value<B>(
    req: Request<B>,
    storage: &crate::state::SharedValuesStorage,
    state: &SharedAdminState,
) -> Response<BoxBody>
where
    B: hyper::body::Body + Send + 'static,
    B::Data: Send,
    B::Error: std::error::Error + Send + Sync,
{
    let body = match http_body_util::BodyExt::collect(req.into_body()).await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "Failed to read request body"),
    };

    let request: CreateValueRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)),
    };

    if request.name.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "Value name cannot be empty");
    }

    let mut guard = storage.write();
    if guard.exists(&request.name) {
        return error_response(
            StatusCode::CONFLICT,
            &format!("Value '{}' already exists", request.name),
        );
    }

    match guard.set_value(&request.name, &request.value) {
        Ok(_) => {
            drop(guard);
            notify_values_changed(state, &request.name);
            success_response(&format!("Value '{}' created", request.name))
        }
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to create value: {}", e),
        ),
    }
}

async fn update_value<B>(
    req: Request<B>,
    name: &str,
    storage: &crate::state::SharedValuesStorage,
    state: &SharedAdminState,
) -> Response<BoxBody>
where
    B: hyper::body::Body + Send + 'static,
    B::Data: Send,
    B::Error: std::error::Error + Send + Sync,
{
    let body = match http_body_util::BodyExt::collect(req.into_body()).await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "Failed to read request body"),
    };

    let request: UpdateValueRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)),
    };

    let mut guard = storage.write();
    if !guard.exists(name) {
        return error_response(
            StatusCode::NOT_FOUND,
            &format!("Value '{}' not found", name),
        );
    }

    let name_owned = name.to_string();
    match guard.set_value(name, &request.value) {
        Ok(_) => {
            drop(guard);
            notify_values_changed(state, &name_owned);
            success_response(&format!("Value '{}' updated", name_owned))
        }
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to update value: {}", e),
        ),
    }
}

fn delete_value(
    name: &str,
    storage: &crate::state::SharedValuesStorage,
    state: &SharedAdminState,
) -> Response<BoxBody> {
    let mut guard = storage.write();
    if !guard.exists(name) {
        return error_response(
            StatusCode::NOT_FOUND,
            &format!("Value '{}' not found", name),
        );
    }

    let name_owned = name.to_string();
    match guard.remove_value(name) {
        Ok(_) => {
            drop(guard);
            notify_values_changed(state, &name_owned);
            success_response(&format!("Value '{}' deleted", name_owned))
        }
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to delete value: {}", e),
        ),
    }
}

fn notify_values_changed(state: &SharedAdminState, name: &str) {
    if let Some(ref config_manager) = state.config_manager {
        match config_manager.notify(ConfigChangeEvent::ValuesChanged(name.to_string())) {
            Ok(count) => {
                tracing::info!(
                    target: "bifrost_admin::values",
                    receivers = count,
                    name = name,
                    "notified values changed event"
                );
            }
            Err(e) => {
                tracing::warn!(
                    target: "bifrost_admin::values",
                    error = %e,
                    name = name,
                    "failed to notify values changed event (no receivers)"
                );
            }
        }
    } else {
        tracing::warn!(
            target: "bifrost_admin::values",
            "config_manager is not available, cannot notify values changed"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AdminState;
    use bytes::Bytes;
    use http_body_util::{BodyExt, Empty};
    use hyper::{Method, Request};

    #[tokio::test]
    async fn handle_values_returns_error_when_storage_not_configured() {
        let state = std::sync::Arc::new(AdminState::new(0));
        let req = Request::builder()
            .method(Method::GET)
            .uri("/api/values")
            .body(Empty::<Bytes>::new())
            .unwrap();

        let resp = handle_values(req, state, "").await;
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let msg = String::from_utf8_lossy(&body);
        assert!(msg.contains("Values storage not configured"));
    }
}
