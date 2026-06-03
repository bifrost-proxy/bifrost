use bytes::Bytes;
use hyper::HeaderMap;
use tracing::info;

use bifrost_admin::push::BreakpointPausedPushData;
use bifrost_admin::{AdminState, SharedPushManager};
use std::sync::Arc;

use super::body_metadata::{normalize_req_headers, set_content_encoding_header, BodyMode};
use super::handler::headers_to_pairs;
use crate::utils::tee::store_request_body;

#[allow(clippy::too_many_arguments)]
pub async fn breakpoint_request_hook(
    admin_state: &Option<Arc<AdminState>>,
    push_manager: &Option<SharedPushManager>,
    request_id: &str,
    method: &str,
    url: &str,
    parts_headers: &mut HeaderMap,
    body: Bytes,
    final_body: &mut Bytes,
) {
    let Some(ref state) = admin_state else {
        return;
    };

    if !state.breakpoint_manager.hook_request_enabled() {
        return;
    }

    let body_str = if !body.is_empty() {
        String::from_utf8(body.to_vec()).ok()
    } else {
        None
    };

    info!(
        "[{}] Breakpoint: request hook triggered for {} {} | headers_count={} | body={:?}",
        request_id,
        method,
        url,
        parts_headers.len(),
        body_str.as_deref().unwrap_or("(empty)")
    );

    let req_headers = headers_to_pairs(parts_headers);

    if let Some(ref pm) = push_manager {
        pm.broadcast_breakpoint_paused(BreakpointPausedPushData {
            phase: "request".to_string(),
            request_id: request_id.to_string(),
            method: Some(method.to_string()),
            url: Some(url.to_string()),
            status: None,
            headers: req_headers.clone(),
            body: body_str.clone(),
        });
    }

    let rx = state
        .breakpoint_manager
        .pause_request(request_id.to_string());

    match rx.await {
        Ok(edit) => {
            let has_edited_body = edit.body.is_some();
            let had_content_length = parts_headers.contains_key(hyper::header::CONTENT_LENGTH);
            let mut new_headers = HeaderMap::new();
            for (key, value) in &edit.headers {
                if let (Ok(name), Ok(val)) = (
                    hyper::header::HeaderName::from_bytes(key.as_bytes()),
                    hyper::header::HeaderValue::from_str(value),
                ) {
                    new_headers.insert(name, val);
                }
            }
            *parts_headers = new_headers;

            if let Some(ref new_body) = edit.body {
                *final_body = Bytes::from(new_body.clone());
                set_content_encoding_header(parts_headers, None);
                let mut request_parts = hyper::Request::new(()).into_parts().0;
                request_parts.headers = parts_headers.clone();
                normalize_req_headers(
                    &mut request_parts,
                    BodyMode::Known(final_body.len()),
                    had_content_length,
                );
                *parts_headers = request_parts.headers;
            }

            let updated_headers = headers_to_pairs(parts_headers);
            let updated_content_type = parts_headers
                .get(hyper::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let updated_request_size = final_body.len();
            let updated_body_ref = if has_edited_body {
                store_request_body(admin_state, request_id, final_body.as_ref(), None)
            } else {
                None
            };
            state.update_traffic_by_id(request_id, move |record| {
                let previous_headers = record.request_headers.clone();
                if record.original_request_headers.is_none() {
                    if let Some(ref previous) = previous_headers {
                        if previous != &updated_headers {
                            record.original_request_headers = Some(previous.clone());
                        }
                    }
                }
                record.request_headers = Some(updated_headers.clone());
                record.request_content_type = updated_content_type.clone();
                record.request_size = updated_request_size;
                if has_edited_body {
                    record.request_body_ref = updated_body_ref.clone();
                }
            });
        }
        Err(_) => {
            state.breakpoint_manager.cancel_request(request_id);
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn breakpoint_response_hook(
    admin_state: &Option<Arc<AdminState>>,
    push_manager: &Option<SharedPushManager>,
    request_id: &str,
    method: &str,
    url: &str,
    status: u16,
    parts_headers: &mut HeaderMap,
    body: Bytes,
    final_body: &mut Bytes,
) {
    let Some(ref state) = admin_state else {
        return;
    };

    if !state.breakpoint_manager.hook_response_enabled() {
        return;
    }

    let body_str = if !body.is_empty() {
        String::from_utf8(body.to_vec()).ok()
    } else {
        None
    };

    info!(
        "[{}] Breakpoint: response hook triggered for {} {} (status {}) | headers_count={}",
        request_id,
        method,
        url,
        status,
        parts_headers.len()
    );

    let res_headers = headers_to_pairs(parts_headers);

    if let Some(ref pm) = push_manager {
        pm.broadcast_breakpoint_paused(BreakpointPausedPushData {
            phase: "response".to_string(),
            request_id: request_id.to_string(),
            method: Some(method.to_string()),
            url: Some(url.to_string()),
            status: Some(status),
            headers: res_headers.clone(),
            body: body_str.clone(),
        });
    }

    let rx = state
        .breakpoint_manager
        .pause_response(request_id.to_string());

    match rx.await {
        Ok(edit) => {
            let mut new_headers = HeaderMap::new();
            for (key, value) in &edit.headers {
                if let (Ok(name), Ok(val)) = (
                    hyper::header::HeaderName::from_bytes(key.as_bytes()),
                    hyper::header::HeaderValue::from_str(value),
                ) {
                    new_headers.insert(name, val);
                }
            }
            *parts_headers = new_headers;

            if let Some(ref new_body) = edit.body {
                *final_body = Bytes::from(new_body.clone());
            }
        }
        Err(_) => {
            state.breakpoint_manager.cancel_request(request_id);
        }
    }
}
