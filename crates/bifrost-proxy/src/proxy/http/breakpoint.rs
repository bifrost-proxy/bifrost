use bytes::Bytes;
use hyper::HeaderMap;
use tracing::{info, warn};

use bifrost_admin::push::BreakpointPausedPushData;
use bifrost_admin::{AdminState, SharedPushManager};
use bifrost_core::Protocol;
use std::sync::Arc;
use std::time::Duration;

use super::body_metadata::{normalize_req_headers, set_content_encoding_header, BodyMode};
use super::handler::headers_to_pairs;
use crate::server::ResolvedRules;
use crate::utils::tee::store_request_body;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BreakpointHookOutcome {
    pub body_replaced: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BreakpointPhase {
    Request,
    Response,
}

fn breakpoint_value_enables_phase(value: &str, phase: BreakpointPhase) -> bool {
    value
        .split(|c: char| c == ',' || c == '|' || c == '&' || c.is_whitespace())
        .map(|part| part.trim().to_ascii_lowercase())
        .any(|part| match part.as_str() {
            "request" | "req" => phase == BreakpointPhase::Request,
            "response" | "res" => phase == BreakpointPhase::Response,
            "both" | "all" => true,
            _ => false,
        })
}

fn breakpoint_rule_enabled(resolved_rules: &ResolvedRules, phase: BreakpointPhase) -> bool {
    resolved_rules.rules.iter().any(|rule| {
        rule.protocol == Protocol::Breakpoint && breakpoint_value_enables_phase(&rule.value, phase)
    })
}

pub fn breakpoint_request_rule_enabled(resolved_rules: &ResolvedRules) -> bool {
    breakpoint_rule_enabled(resolved_rules, BreakpointPhase::Request)
}

pub fn breakpoint_response_rule_enabled(resolved_rules: &ResolvedRules) -> bool {
    breakpoint_rule_enabled(resolved_rules, BreakpointPhase::Response)
}

#[derive(Debug)]
struct BreakpointBodyPayload {
    body: Option<String>,
    body_editable: bool,
    body_omitted: bool,
    body_size: Option<usize>,
    max_body_bytes: usize,
}

fn breakpoint_body_payload(
    state: &AdminState,
    body: &Bytes,
    body_size_hint: Option<usize>,
    force_body_omitted: bool,
) -> BreakpointBodyPayload {
    let max_body_bytes = state.breakpoint_manager.max_body_bytes();
    let body_size = if body.is_empty() {
        body_size_hint
    } else {
        Some(body.len())
    };

    if force_body_omitted {
        return BreakpointBodyPayload {
            body: None,
            body_editable: false,
            body_omitted: true,
            body_size,
            max_body_bytes,
        };
    }

    if body.is_empty() {
        return BreakpointBodyPayload {
            body: None,
            body_editable: true,
            body_omitted: false,
            body_size,
            max_body_bytes,
        };
    }

    if !state
        .breakpoint_manager
        .body_within_capture_limit(body.len())
    {
        return BreakpointBodyPayload {
            body: None,
            body_editable: false,
            body_omitted: true,
            body_size,
            max_body_bytes,
        };
    }

    match String::from_utf8(body.to_vec()) {
        Ok(body) => BreakpointBodyPayload {
            body: Some(body),
            body_editable: true,
            body_omitted: false,
            body_size,
            max_body_bytes,
        },
        Err(_) => BreakpointBodyPayload {
            body: None,
            body_editable: false,
            body_omitted: true,
            body_size,
            max_body_bytes,
        },
    }
}

fn body_edit_within_limit(state: &AdminState, body: &str) -> bool {
    state
        .breakpoint_manager
        .body_within_capture_limit(body.len())
}

#[allow(clippy::too_many_arguments)]
pub async fn breakpoint_request_hook(
    admin_state: &Option<Arc<AdminState>>,
    push_manager: &Option<SharedPushManager>,
    request_id: &str,
    method: &str,
    url: &str,
    parts_headers: &mut HeaderMap,
    body: Bytes,
    body_size_hint: Option<usize>,
    force_body_omitted: bool,
    final_body: &mut Bytes,
) -> BreakpointHookOutcome {
    let Some(ref state) = admin_state else {
        return BreakpointHookOutcome::default();
    };

    if !state.breakpoint_manager.is_enabled() {
        return BreakpointHookOutcome::default();
    }

    let body_payload = breakpoint_body_payload(state, &body, body_size_hint, force_body_omitted);

    info!(
        "[{}] Breakpoint: request hook triggered for {} {} | headers_count={} | body_size={:?} | body_omitted={}",
        request_id,
        method,
        url,
        parts_headers.len(),
        body_payload.body_size,
        body_payload.body_omitted,
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
            body: body_payload.body.clone(),
            body_omitted: body_payload.body_omitted,
            body_size: body_payload.body_size,
            max_body_bytes: body_payload.max_body_bytes,
        });
    }

    let rx = state
        .breakpoint_manager
        .pause_request(request_id.to_string(), body_payload.body_editable);

    let timeout_ms = state.breakpoint_manager.timeout_ms();
    match tokio::time::timeout(Duration::from_millis(timeout_ms), rx).await {
        Err(_) => {
            warn!(
                "[{}] Breakpoint: request hook timed out after {}ms; continuing without edits",
                request_id, timeout_ms
            );
            state.breakpoint_manager.cancel_request(request_id);
        }
        Ok(Err(_)) => {
            state.breakpoint_manager.cancel_request(request_id);
        }
        Ok(Ok(edit)) => {
            let mut body_replaced = false;
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
                if body_edit_within_limit(state, new_body) {
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
                    body_replaced = true;
                } else {
                    warn!(
                        "[{}] Breakpoint: ignored request body edit larger than {} bytes",
                        request_id,
                        state.breakpoint_manager.max_body_bytes()
                    );
                }
            }

            let updated_headers = headers_to_pairs(parts_headers);
            let updated_content_type = parts_headers
                .get(hyper::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let updated_request_size = final_body.len();
            let updated_body_ref = if body_replaced {
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
                if body_replaced {
                    record.request_body_ref = updated_body_ref.clone();
                }
            });
            return BreakpointHookOutcome { body_replaced };
        }
    }

    BreakpointHookOutcome::default()
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
    body_size_hint: Option<usize>,
    force_body_omitted: bool,
    final_body: &mut Bytes,
) -> BreakpointHookOutcome {
    let Some(ref state) = admin_state else {
        return BreakpointHookOutcome::default();
    };

    if !state.breakpoint_manager.is_enabled() {
        return BreakpointHookOutcome::default();
    }

    let body_payload = breakpoint_body_payload(state, &body, body_size_hint, force_body_omitted);

    info!(
        "[{}] Breakpoint: response hook triggered for {} {} (status {}) | headers_count={} | body_size={:?} | body_omitted={}",
        request_id,
        method,
        url,
        status,
        parts_headers.len(),
        body_payload.body_size,
        body_payload.body_omitted,
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
            body: body_payload.body.clone(),
            body_omitted: body_payload.body_omitted,
            body_size: body_payload.body_size,
            max_body_bytes: body_payload.max_body_bytes,
        });
    }

    let rx = state
        .breakpoint_manager
        .pause_response(request_id.to_string(), body_payload.body_editable);

    let timeout_ms = state.breakpoint_manager.timeout_ms();
    match tokio::time::timeout(Duration::from_millis(timeout_ms), rx).await {
        Err(_) => {
            warn!(
                "[{}] Breakpoint: response hook timed out after {}ms; continuing without edits",
                request_id, timeout_ms
            );
            state.breakpoint_manager.cancel_request(request_id);
        }
        Ok(Err(_)) => {
            state.breakpoint_manager.cancel_request(request_id);
        }
        Ok(Ok(edit)) => {
            let mut body_replaced = false;
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
                if body_edit_within_limit(state, new_body) {
                    *final_body = Bytes::from(new_body.clone());
                    set_content_encoding_header(parts_headers, None);
                    body_replaced = true;
                } else {
                    warn!(
                        "[{}] Breakpoint: ignored response body edit larger than {} bytes",
                        request_id,
                        state.breakpoint_manager.max_body_bytes()
                    );
                }
            }
            return BreakpointHookOutcome { body_replaced };
        }
    }

    BreakpointHookOutcome::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::RuleValue;
    use hyper::header::HeaderValue;
    use std::collections::HashMap;

    fn resolved_with_breakpoint(value: &str) -> ResolvedRules {
        ResolvedRules {
            rules: vec![RuleValue {
                pattern: "example.test".to_string(),
                protocol: Protocol::Breakpoint,
                value: value.to_string(),
                options: HashMap::new(),
                rule_name: None,
                raw: None,
                line: None,
                auto_tls_intercept: false,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn breakpoint_rule_phases_are_explicit() {
        let request = resolved_with_breakpoint("request");
        assert!(breakpoint_request_rule_enabled(&request));
        assert!(!breakpoint_response_rule_enabled(&request));

        let response = resolved_with_breakpoint("response");
        assert!(!breakpoint_request_rule_enabled(&response));
        assert!(breakpoint_response_rule_enabled(&response));

        let both = resolved_with_breakpoint("req,res");
        assert!(breakpoint_request_rule_enabled(&both));
        assert!(breakpoint_response_rule_enabled(&both));

        let empty = resolved_with_breakpoint("");
        assert!(!breakpoint_request_rule_enabled(&empty));
        assert!(!breakpoint_response_rule_enabled(&empty));
    }

    #[test]
    fn breakpoint_value_enables_phase_parses_aliases_and_delimiters() {
        assert!(breakpoint_value_enables_phase(
            "request",
            BreakpointPhase::Request
        ));
        assert!(breakpoint_value_enables_phase(
            "req",
            BreakpointPhase::Request
        ));
        assert!(breakpoint_value_enables_phase(
            "response",
            BreakpointPhase::Response
        ));
        assert!(breakpoint_value_enables_phase(
            "res",
            BreakpointPhase::Response
        ));
        assert!(breakpoint_value_enables_phase(
            "both",
            BreakpointPhase::Request
        ));
        assert!(breakpoint_value_enables_phase(
            "all",
            BreakpointPhase::Response
        ));

        // Mixed delimiters and whitespace.
        let value = " req | res , both & other";
        assert!(breakpoint_value_enables_phase(
            value,
            BreakpointPhase::Request
        ));
        assert!(breakpoint_value_enables_phase(
            value,
            BreakpointPhase::Response
        ));
        assert!(!breakpoint_value_enables_phase(
            "none",
            BreakpointPhase::Request
        ));
    }

    #[test]
    fn breakpoint_body_payload_handles_empty_and_forced_omit() {
        let state = AdminState::new(0);
        let body = Bytes::new();

        let payload = breakpoint_body_payload(&state, &body, Some(123), false);
        assert!(payload.body.is_none());
        assert!(payload.body_editable);
        assert!(!payload.body_omitted);
        assert_eq!(payload.body_size, Some(123));

        let payload = breakpoint_body_payload(&state, &body, Some(5), true);
        assert!(payload.body.is_none());
        assert!(!payload.body_editable);
        assert!(payload.body_omitted);
        assert_eq!(payload.body_size, Some(5));
    }

    #[test]
    fn breakpoint_body_payload_respects_capture_limit_and_utf8_validity() {
        let state = AdminState::new(0);
        state
            .breakpoint_manager
            .update_settings(bifrost_admin::breakpoint::BreakpointSettings {
                enabled: true,
                max_body_bytes: 4,
            });

        // Within limit and valid UTF-8
        let body = Bytes::from_static(b"abcd");
        let payload = breakpoint_body_payload(&state, &body, None, false);
        assert_eq!(payload.body.as_deref(), Some("abcd"));
        assert!(payload.body_editable);
        assert!(!payload.body_omitted);

        // Exceeds limit
        let body = Bytes::from_static(b"abcdef");
        let payload = breakpoint_body_payload(&state, &body, None, false);
        assert!(payload.body.is_none());
        assert!(!payload.body_editable);
        assert!(payload.body_omitted);

        // Invalid UTF-8 body is omitted even when small
        let body = Bytes::from_static(&[0xff, 0xfe]);
        let payload = breakpoint_body_payload(&state, &body, None, false);
        assert!(payload.body.is_none());
        assert!(!payload.body_editable);
        assert!(payload.body_omitted);
    }

    #[test]
    fn body_edit_within_limit_uses_breakpoint_capture_limit() {
        let state = AdminState::new(0);
        state
            .breakpoint_manager
            .update_settings(bifrost_admin::breakpoint::BreakpointSettings {
                enabled: true,
                max_body_bytes: 5,
            });

        assert!(body_edit_within_limit(&state, "12345"));
        assert!(!body_edit_within_limit(&state, "123456"));
    }

    async fn wait_until_pending(state: &AdminState, id: &str) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while !state.breakpoint_manager.has_pending(id) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn coverage_90_request_and_response_hooks_apply_resumed_edits() {
        use bifrost_admin::breakpoint::{BreakpointEdit, BreakpointSettings};
        let state = Arc::new(AdminState::new(0));
        state
            .breakpoint_manager
            .update_settings(BreakpointSettings {
                enabled: true,
                max_body_bytes: 64,
            });

        let task_state = state.clone();
        let request_task = tokio::spawn(async move {
            let mut headers = HeaderMap::new();
            headers.insert(hyper::header::CONTENT_LENGTH, HeaderValue::from_static("3"));
            let mut final_body = Bytes::from_static(b"old");
            let outcome = breakpoint_request_hook(
                &Some(task_state),
                &None,
                "request-covered",
                "POST",
                "http://example.test/",
                &mut headers,
                final_body.clone(),
                Some(3),
                false,
                &mut final_body,
            )
            .await;
            (outcome, headers, final_body)
        });
        wait_until_pending(&state, "request-covered").await;
        assert!(state.breakpoint_manager.resume(
            "request-covered",
            "request",
            BreakpointEdit {
                headers: vec![
                    ("content-type".into(), "text/plain".into()),
                    ("bad header".into(), "x".into())
                ],
                body: Some("new-body".into()),
            }
        ));
        let (outcome, headers, body) = request_task.await.unwrap();
        assert!(outcome.body_replaced);
        assert_eq!(body, Bytes::from_static(b"new-body"));
        assert_eq!(headers[hyper::header::CONTENT_LENGTH], "8");

        let task_state = state.clone();
        let response_task = tokio::spawn(async move {
            let mut headers = HeaderMap::new();
            headers.insert(
                hyper::header::CONTENT_ENCODING,
                HeaderValue::from_static("gzip"),
            );
            let mut final_body = Bytes::from_static(b"old");
            let outcome = breakpoint_response_hook(
                &Some(task_state),
                &None,
                "response-covered",
                "GET",
                "http://example.test/",
                200,
                &mut headers,
                final_body.clone(),
                Some(3),
                false,
                &mut final_body,
            )
            .await;
            (outcome, headers, final_body)
        });
        wait_until_pending(&state, "response-covered").await;
        assert!(state.breakpoint_manager.resume(
            "response-covered",
            "response",
            BreakpointEdit {
                headers: vec![("x-edited".into(), "yes".into())],
                body: Some("response-new".into()),
            }
        ));
        let (outcome, headers, body) = response_task.await.unwrap();
        assert!(outcome.body_replaced);
        assert_eq!(headers["x-edited"], "yes");
        assert!(!headers.contains_key(hyper::header::CONTENT_ENCODING));
        assert_eq!(body, Bytes::from_static(b"response-new"));
    }

    #[tokio::test]
    async fn coverage_90_hooks_cover_disabled_cancelled_and_oversized_edit_paths() {
        use bifrost_admin::breakpoint::{BreakpointEdit, BreakpointSettings};
        let mut headers = HeaderMap::new();
        let mut final_body = Bytes::new();
        assert!(
            !breakpoint_request_hook(
                &None,
                &None,
                "none",
                "GET",
                "/",
                &mut headers,
                Bytes::new(),
                None,
                false,
                &mut final_body,
            )
            .await
            .body_replaced
        );

        let state = Arc::new(AdminState::new(0));
        assert!(
            !breakpoint_response_hook(
                &Some(state.clone()),
                &None,
                "disabled",
                "GET",
                "/",
                200,
                &mut headers,
                Bytes::new(),
                None,
                false,
                &mut final_body,
            )
            .await
            .body_replaced
        );
        state
            .breakpoint_manager
            .update_settings(BreakpointSettings {
                enabled: true,
                max_body_bytes: 2,
            });
        let task_state = state.clone();
        let task = tokio::spawn(async move {
            let mut headers = HeaderMap::new();
            let mut final_body = Bytes::from_static(b"old");
            breakpoint_response_hook(
                &Some(task_state),
                &None,
                "oversized",
                "GET",
                "/",
                200,
                &mut headers,
                final_body.clone(),
                Some(3),
                true,
                &mut final_body,
            )
            .await
        });
        wait_until_pending(&state, "oversized").await;
        assert!(state.breakpoint_manager.resume(
            "oversized",
            "response",
            BreakpointEdit {
                headers: vec![],
                body: Some("way too large".into()),
            }
        ));
        assert!(!task.await.unwrap().body_replaced);
    }
}
