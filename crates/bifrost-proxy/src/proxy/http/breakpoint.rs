use bytes::Bytes;
use hyper::HeaderMap;
use tracing::{info, warn};

use bifrost_admin::breakpoint::PendingBreakpoint;
use bifrost_admin::{AdminState, SharedPushManager};
use bifrost_core::Protocol;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::body_metadata::{
    header_content_encoding, normalize_req_headers, set_content_encoding_header, BodyMode,
};
use super::handler::headers_to_pairs;
use crate::server::{full_body, BoxBody};
use crate::server::{ResolvedRules, RulesResolver};
use crate::transform::{compress_body, try_decompress_body_with_limit};
use crate::utils::tee::store_response_body;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BreakpointHookOutcome {
    pub body_replaced: bool,
    pub method: Option<String>,
    pub url: Option<String>,
    pub status: Option<u16>,
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

pub fn breakpoint_rules_require_tls_interception(
    admin_state: &Option<Arc<AdminState>>,
    resolved_rules: &ResolvedRules,
) -> bool {
    admin_state.as_ref().is_some_and(|state| {
        state.breakpoint_manager.is_enabled()
            && (breakpoint_request_rule_enabled(resolved_rules)
                || breakpoint_response_rule_enabled(resolved_rules))
    })
}

pub fn breakpoint_host_rules_require_tls_interception(
    admin_state: &Option<Arc<AdminState>>,
    rules: Option<&dyn RulesResolver>,
    authority: &str,
) -> bool {
    admin_state.as_ref().is_some_and(|state| {
        state.breakpoint_manager.is_enabled()
            && rules.is_some_and(|rules| rules.has_breakpoint_rules_for_host(authority))
    })
}

pub fn breakpoint_tls_interception_required(
    admin_state: &Option<Arc<AdminState>>,
    resolved_rules: &ResolvedRules,
    rules: Option<&dyn RulesResolver>,
    host: &str,
    port: u16,
) -> bool {
    let authority = super::handler::format_connection_endpoint(host, port);
    breakpoint_rules_require_tls_interception(admin_state, resolved_rules)
        || breakpoint_host_rules_require_tls_interception(admin_state, rules, &authority)
}

pub fn body_limit(state: &Option<Arc<AdminState>>, enabled: bool, fallback: usize) -> usize {
    state
        .as_ref()
        .filter(|_| enabled)
        .map_or(fallback, |state| {
            fallback.min(state.breakpoint_manager.max_body_bytes())
        })
}

#[derive(Debug)]
struct BreakpointBodyPayload {
    body: Option<String>,
    body_editable: bool,
    body_omitted: bool,
    body_size: Option<usize>,
    max_body_bytes: usize,
    content_encoding: Option<String>,
}

fn breakpoint_body_payload(
    state: &AdminState,
    headers: &HeaderMap,
    body: &Bytes,
    body_size_hint: Option<usize>,
    force_body_omitted: bool,
) -> BreakpointBodyPayload {
    let max_body_bytes = state.breakpoint_manager.max_body_bytes();
    let source_body_empty = body.is_empty();
    let content_encoding = header_content_encoding(headers)
        .filter(|encoding| !encoding.eq_ignore_ascii_case("identity"));
    let body_size = if source_body_empty {
        body_size_hint
    } else {
        Some(body.len())
    };

    let decoded = if force_body_omitted {
        None
    } else if source_body_empty {
        Some(Bytes::new())
    } else if let Some(ref encoding) = content_encoding {
        try_decompress_body_with_limit(body, encoding, max_body_bytes)
            .ok()
            .map(Bytes::from)
    } else {
        Some(body.clone())
    };
    let body = (!source_body_empty)
        .then(|| decoded.as_ref())
        .flatten()
        .filter(|bytes| {
            state
                .breakpoint_manager
                .body_within_capture_limit(bytes.len())
        })
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .map(str::to_owned);
    let body_editable = !force_body_omitted && (source_body_empty || body.is_some());
    BreakpointBodyPayload {
        body,
        body_editable,
        body_omitted: !body_editable,
        body_size,
        max_body_bytes,
        content_encoding,
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[allow(clippy::too_many_arguments)]
fn pending_snapshot(
    state: &AdminState,
    phase: &str,
    request_id: &str,
    method: Option<String>,
    url: Option<String>,
    status: Option<u16>,
    headers: Vec<(String, String)>,
    body: &BreakpointBodyPayload,
) -> PendingBreakpoint {
    let paused_at_ms = now_ms();
    PendingBreakpoint {
        request_id: request_id.to_string(),
        phase: phase.to_string(),
        method,
        url,
        status,
        headers,
        body: body.body.clone(),
        body_omitted: body.body_omitted,
        body_size: body.body_size,
        max_body_bytes: body.max_body_bytes,
        content_encoding: body.content_encoding.clone(),
        paused_at_ms,
        deadline_at_ms: paused_at_ms.saturating_add(state.breakpoint_manager.timeout_ms()),
    }
}

fn apply_edited_headers(target: &mut HeaderMap, headers: Option<Vec<(String, String)>>) {
    let Some(headers) = headers else {
        return;
    };
    let mut edited = HeaderMap::new();
    for (key, value) in headers {
        if let (Ok(name), Ok(value)) = (
            hyper::header::HeaderName::from_bytes(key.as_bytes()),
            hyper::header::HeaderValue::from_str(&value),
        ) {
            edited.append(name, value);
        }
    }
    *target = edited;
}

fn encode_edited_body(headers: &HeaderMap, body: &str) -> Option<Bytes> {
    let encoding = header_content_encoding(headers)
        .filter(|encoding| !encoding.eq_ignore_ascii_case("identity"));
    match encoding {
        Some(encoding) => compress_body(body.as_bytes(), &encoding)
            .ok()
            .map(Bytes::from),
        None => Some(Bytes::copy_from_slice(body.as_bytes())),
    }
}

pub fn apply_edited_status(target: &mut hyper::StatusCode, edited: Option<u16>) {
    if let Some(status) = edited.and_then(|value| hyper::StatusCode::from_u16(value).ok()) {
        *target = status;
    }
}

pub fn body_read_error_response(error: impl std::fmt::Display) -> hyper::Response<BoxBody> {
    hyper::Response::builder()
        .status(hyper::StatusCode::BAD_GATEWAY)
        .body(full_body(format!(
            "Failed to read request body for breakpoint: {error}"
        )))
        .expect("static breakpoint error response")
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

    let body_payload = breakpoint_body_payload(
        state,
        parts_headers,
        &body,
        body_size_hint,
        force_body_omitted,
    );

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

    let snapshot = pending_snapshot(
        state,
        "request",
        request_id,
        Some(method.to_string()),
        Some(url.to_string()),
        None,
        req_headers,
        &body_payload,
    );
    let rx = state
        .breakpoint_manager
        .pause_request(snapshot.clone(), body_payload.body_editable);
    if let Some(ref pm) = push_manager {
        pm.broadcast_breakpoint_paused(snapshot.clone());
    }

    let timeout_ms = state.breakpoint_manager.timeout_ms();
    match tokio::time::timeout(Duration::from_millis(timeout_ms), rx).await {
        Err(_) => {
            warn!(
                "[{}] Breakpoint: request hook timed out after {}ms; continuing without edits",
                request_id, timeout_ms
            );
            state.breakpoint_manager.cancel(request_id, "request");
            if let Some(ref pm) = push_manager {
                pm.broadcast_breakpoint_resumed(
                    request_id.to_string(),
                    "request".to_string(),
                    "timeout".to_string(),
                );
            }
        }
        Ok(Err(_)) => {
            state.breakpoint_manager.cancel(request_id, "request");
        }
        Ok(Ok(mut edit)) => {
            let mut body_replaced = false;
            let had_content_length = parts_headers.contains_key(hyper::header::CONTENT_LENGTH);
            let edited_method = edit.method.take();
            let edited_url = edit.url.take();
            let original_content_encoding = header_content_encoding(parts_headers);
            apply_edited_headers(parts_headers, edit.headers.take());

            if let Some(ref new_body) = edit.body {
                if let Some(encoded) = encode_edited_body(parts_headers, new_body) {
                    *final_body = encoded;
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
                    set_content_encoding_header(
                        parts_headers,
                        original_content_encoding.as_deref(),
                    );
                    warn!(
                        request_id,
                        "Breakpoint request body edit ignored: unsupported encoding"
                    );
                }
            }

            return BreakpointHookOutcome {
                body_replaced,
                method: edited_method,
                url: edited_url,
                status: None,
            };
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

    let body_payload = breakpoint_body_payload(
        state,
        parts_headers,
        &body,
        body_size_hint,
        force_body_omitted,
    );

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

    let snapshot = pending_snapshot(
        state,
        "response",
        request_id,
        Some(method.to_string()),
        Some(url.to_string()),
        Some(status),
        res_headers,
        &body_payload,
    );
    let rx = state
        .breakpoint_manager
        .pause_response(snapshot.clone(), body_payload.body_editable);
    if let Some(ref pm) = push_manager {
        pm.broadcast_breakpoint_paused(snapshot.clone());
    }

    let timeout_ms = state.breakpoint_manager.timeout_ms();
    match tokio::time::timeout(Duration::from_millis(timeout_ms), rx).await {
        Err(_) => {
            warn!(
                "[{}] Breakpoint: response hook timed out after {}ms; continuing without edits",
                request_id, timeout_ms
            );
            state.breakpoint_manager.cancel(request_id, "response");
            if let Some(ref pm) = push_manager {
                pm.broadcast_breakpoint_resumed(
                    request_id.to_string(),
                    "response".to_string(),
                    "timeout".to_string(),
                );
            }
        }
        Ok(Err(_)) => {
            state.breakpoint_manager.cancel(request_id, "response");
        }
        Ok(Ok(mut edit)) => {
            let mut body_replaced = false;
            let edited_status = edit.status.take();
            let original_content_encoding = header_content_encoding(parts_headers);
            apply_edited_headers(parts_headers, edit.headers.take());

            if let Some(ref new_body) = edit.body {
                if let Some(encoded) = encode_edited_body(parts_headers, new_body) {
                    *final_body = encoded;
                    body_replaced = true;
                } else {
                    set_content_encoding_header(
                        parts_headers,
                        original_content_encoding.as_deref(),
                    );
                    warn!(
                        request_id,
                        "Breakpoint response body edit ignored: unsupported encoding"
                    );
                }
            }
            let updated_headers = headers_to_pairs(parts_headers);
            let updated_content_type = parts_headers
                .get(hyper::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            let updated_body_ref = body_replaced
                .then(|| store_response_body(admin_state, request_id, final_body.as_ref()))
                .flatten();
            let updated_response_size = final_body.len();
            state.update_traffic_by_id(request_id, move |record| {
                record.response_headers = Some(updated_headers.clone());
                record.content_type = updated_content_type.clone();
                if let Some(status) = edited_status {
                    record.status = status;
                }
                if body_replaced {
                    record.response_body_ref = updated_body_ref.clone();
                    record.response_size = updated_response_size;
                    record.download_bytes = updated_response_size;
                }
            });
            return BreakpointHookOutcome {
                body_replaced,
                method: None,
                url: None,
                status: edited_status,
            };
        }
    }

    BreakpointHookOutcome::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::{RuleValue, RulesResolver};
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
    fn enabled_breakpoint_rule_requires_scoped_tls_interception() {
        let state = Arc::new(AdminState::new(0));
        let request = resolved_with_breakpoint("request");
        assert!(!breakpoint_rules_require_tls_interception(
            &Some(state.clone()),
            &request,
        ));
        state
            .breakpoint_manager
            .update_settings(bifrost_admin::breakpoint::BreakpointSettings {
                enabled: true,
                max_body_bytes: 1024,
            });
        assert!(breakpoint_rules_require_tls_interception(
            &Some(state),
            &request,
        ));
        assert!(!breakpoint_rules_require_tls_interception(&None, &request));
    }

    struct HostBreakpointResolver;

    impl RulesResolver for HostBreakpointResolver {
        fn resolve_with_context(
            &self,
            _url: &str,
            _method: &str,
            _req_headers: &HashMap<String, String>,
            _req_cookies: &HashMap<String, String>,
        ) -> ResolvedRules {
            ResolvedRules::default()
        }

        fn has_breakpoint_rules_for_host(&self, host: &str) -> bool {
            matches!(host, "example.test:443" | "[::1]:443")
        }
    }

    #[test]
    fn breakpoint_host_and_body_limit_helpers_follow_runtime_gate() {
        let state = Arc::new(AdminState::new(0));
        let resolver = HostBreakpointResolver;
        assert!(!breakpoint_host_rules_require_tls_interception(
            &Some(state.clone()),
            Some(&resolver),
            "example.test:443",
        ));
        assert_eq!(body_limit(&Some(state.clone()), true, 4096), 4096);

        state
            .breakpoint_manager
            .update_settings(bifrost_admin::breakpoint::BreakpointSettings {
                enabled: true,
                max_body_bytes: 64,
            });
        assert!(breakpoint_host_rules_require_tls_interception(
            &Some(state.clone()),
            Some(&resolver),
            "example.test:443",
        ));
        assert!(!breakpoint_host_rules_require_tls_interception(
            &Some(state.clone()),
            Some(&resolver),
            "other.test:443",
        ));
        assert!(!breakpoint_host_rules_require_tls_interception(
            &Some(state.clone()),
            None,
            "example.test:443",
        ));
        assert!(!breakpoint_host_rules_require_tls_interception(
            &None,
            Some(&resolver),
            "example.test:443",
        ));
        let no_op = crate::server::NoOpRulesResolver;
        assert!(!breakpoint_host_rules_require_tls_interception(
            &Some(state.clone()),
            Some(&no_op),
            "example.test:443",
        ));
        assert_eq!(body_limit(&Some(state.clone()), true, 4096), 64);
        assert_eq!(body_limit(&Some(state), false, 4096), 4096);
        assert_eq!(body_limit(&None, true, 4096), 4096);

        let resolved = ResolvedRules::default();
        assert!(breakpoint_tls_interception_required(
            &Some(Arc::new({
                let state = AdminState::new(0);
                state.breakpoint_manager.update_settings(
                    bifrost_admin::breakpoint::BreakpointSettings {
                        enabled: true,
                        max_body_bytes: 64,
                    },
                );
                state
            })),
            &resolved,
            Some(&resolver),
            "::1",
            443,
        ));
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
        let headers = HeaderMap::new();
        let body = Bytes::new();

        let payload = breakpoint_body_payload(&state, &headers, &body, Some(123), false);
        assert!(payload.body.is_none());
        assert!(payload.body_editable);
        assert!(!payload.body_omitted);
        assert_eq!(payload.body_size, Some(123));

        let payload = breakpoint_body_payload(&state, &headers, &body, Some(5), true);
        assert!(payload.body.is_none());
        assert!(!payload.body_editable);
        assert!(payload.body_omitted);
        assert_eq!(payload.body_size, Some(5));
        assert!(format!("{payload:?}").contains("body_omitted"));

        let outcome = BreakpointHookOutcome::default();
        assert_eq!(<BreakpointHookOutcome as Clone>::clone(&outcome), outcome);
        assert!(format!("{outcome:?}").contains("body_replaced"));
    }

    #[test]
    fn breakpoint_body_payload_respects_capture_limit_and_utf8_validity() {
        let state = AdminState::new(0);
        let headers = HeaderMap::new();
        state
            .breakpoint_manager
            .update_settings(bifrost_admin::breakpoint::BreakpointSettings {
                enabled: true,
                max_body_bytes: 4,
            });

        // Within limit and valid UTF-8
        let body = Bytes::from_static(b"abcd");
        let payload = breakpoint_body_payload(&state, &headers, &body, None, false);
        assert_eq!(payload.body.as_deref(), Some("abcd"));
        assert!(payload.body_editable);
        assert!(!payload.body_omitted);

        // Exceeds limit
        let body = Bytes::from_static(b"abcdef");
        let payload = breakpoint_body_payload(&state, &headers, &body, None, false);
        assert!(payload.body.is_none());
        assert!(!payload.body_editable);
        assert!(payload.body_omitted);

        // Invalid UTF-8 body is omitted even when small
        let body = Bytes::from_static(&[0xff, 0xfe]);
        let payload = breakpoint_body_payload(&state, &headers, &body, None, false);
        assert!(payload.body.is_none());
        assert!(!payload.body_editable);
        assert!(payload.body_omitted);

        let mut gzip_headers = HeaderMap::new();
        gzip_headers.insert(
            hyper::header::CONTENT_ENCODING,
            HeaderValue::from_static("gzip"),
        );
        let invalid_gzip = breakpoint_body_payload(
            &state,
            &gzip_headers,
            &Bytes::from_static(b"not-gzip"),
            None,
            false,
        );
        assert!(invalid_gzip.body.is_none());
        assert!(invalid_gzip.body_omitted);
    }

    #[test]
    fn snapshot_header_and_encoding_helpers_cover_optional_paths() {
        let state = AdminState::new(0);
        let payload = BreakpointBodyPayload {
            body: Some("body".into()),
            body_editable: true,
            body_omitted: false,
            body_size: Some(4),
            max_body_bytes: 10,
            content_encoding: None,
        };
        let snapshot = pending_snapshot(
            &state,
            "response",
            "snapshot",
            Some("GET".into()),
            Some("http://example.test/".into()),
            Some(201),
            vec![("x-test".into(), "yes".into())],
            &payload,
        );
        assert_eq!(snapshot.phase, "response");
        assert!(snapshot.deadline_at_ms >= snapshot.paused_at_ms);
        let push = snapshot.clone();
        assert_eq!(push.request_id, "snapshot");
        assert_eq!(push.status, Some(201));
        assert_eq!(push.body.as_deref(), Some("body"));

        let mut headers = HeaderMap::new();
        apply_edited_headers(&mut headers, None);
        assert!(headers.is_empty());
        apply_edited_headers(
            &mut headers,
            Some(vec![
                ("x-one".into(), "1".into()),
                ("x-one".into(), "2".into()),
                ("bad header".into(), "ignored".into()),
            ]),
        );
        assert_eq!(headers.get_all("x-one").iter().count(), 2);
        assert_eq!(
            encode_edited_body(&headers, "plain").unwrap(),
            Bytes::from_static(b"plain")
        );
        headers.insert(
            hyper::header::CONTENT_ENCODING,
            HeaderValue::from_static("unsupported"),
        );
        assert!(encode_edited_body(&headers, "plain").is_none());

        let mut status = hyper::StatusCode::OK;
        apply_edited_status(&mut status, Some(218));
        assert_eq!(status.as_u16(), 218);
        apply_edited_status(&mut status, Some(99));
        assert_eq!(status.as_u16(), 218);
        apply_edited_status(&mut status, None);
        assert_eq!(status.as_u16(), 218);
        let error = body_read_error_response("broken");
        assert_eq!(error.status(), hyper::StatusCode::BAD_GATEWAY);
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
        let harness = bifrost_admin::test_support::TestAdminState::builder().build();
        let state = harness.state();
        state
            .breakpoint_manager
            .update_settings(BreakpointSettings {
                enabled: true,
                max_body_bytes: 64,
            });
        state.breakpoint_manager.set_timeout_ms(5_000);
        let push = Arc::new(bifrost_admin::push::PushManager::new(state.clone()));

        let task_state = state.clone();
        let task_push = push.clone();
        let request_task = tokio::spawn(async move {
            let mut headers = HeaderMap::new();
            headers.insert(hyper::header::CONTENT_LENGTH, HeaderValue::from_static("3"));
            let mut final_body = Bytes::from_static(b"old");
            let outcome = breakpoint_request_hook(
                &Some(task_state),
                &Some(task_push),
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
        assert!(state
            .breakpoint_manager
            .resume(
                "request-covered",
                "request",
                BreakpointEdit {
                    headers: vec![
                        ("content-type".into(), "text/plain".into()),
                        ("bad header".into(), "x".into())
                    ]
                    .into(),
                    body: Some("new-body".into()),
                    method: Some("PUT".into()),
                    url: Some("http://example.test/edited".into()),
                    ..Default::default()
                }
            )
            .is_ok());
        let (outcome, headers, body) = request_task.await.unwrap();
        assert!(outcome.body_replaced);
        assert_eq!(outcome.method.as_deref(), Some("PUT"));
        assert_eq!(outcome.url.as_deref(), Some("http://example.test/edited"));
        assert_eq!(body, Bytes::from_static(b"new-body"));
        assert_eq!(headers[hyper::header::CONTENT_LENGTH], "8");

        let task_state = state.clone();
        let task_push = push;
        let gzip_old = Bytes::from(compress_body(b"old", "gzip").unwrap());
        state.record_traffic(bifrost_admin::TrafficRecord::new(
            "response-covered".into(),
            "GET".into(),
            "http://example.test/".into(),
        ));
        let response_task = tokio::spawn(async move {
            let mut headers = HeaderMap::new();
            headers.insert(
                hyper::header::CONTENT_ENCODING,
                HeaderValue::from_static("gzip"),
            );
            let mut final_body = gzip_old;
            let outcome = breakpoint_response_hook(
                &Some(task_state),
                &Some(task_push),
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
        let pending = state.breakpoint_manager.pending();
        assert_eq!(pending[0].body.as_deref(), Some("old"));
        assert_eq!(pending[0].content_encoding.as_deref(), Some("gzip"));
        assert!(state
            .breakpoint_manager
            .resume(
                "response-covered",
                "response",
                BreakpointEdit {
                    headers: Some(vec![
                        ("content-encoding".into(), "gzip".into()),
                        ("x-edited".into(), "yes".into()),
                        ("set-cookie".into(), "a=1".into()),
                        ("set-cookie".into(), "b=2".into()),
                    ]),
                    body: Some("response-new".into()),
                    status: Some(201),
                    ..Default::default()
                }
            )
            .is_ok());
        let (outcome, headers, body) = response_task.await.unwrap();
        assert!(outcome.body_replaced);
        assert_eq!(outcome.status, Some(201));
        assert_eq!(headers["x-edited"], "yes");
        assert_eq!(headers[hyper::header::CONTENT_ENCODING], "gzip");
        assert_eq!(headers.get_all("set-cookie").iter().count(), 2);
        assert_eq!(
            try_decompress_body_with_limit(&body, "gzip", 64).unwrap(),
            b"response-new"
        );
        let record = harness
            .traffic_db
            .get_by_id("response-covered")
            .expect("edited response record");
        assert_eq!(record.status, 201);
        assert_eq!(record.response_size, body.len());
    }

    #[tokio::test]
    async fn unsupported_edited_content_encoding_preserves_original_body_encoding() {
        use bifrost_admin::breakpoint::{BreakpointEdit, BreakpointSettings};
        let state = Arc::new(AdminState::new(0));
        state
            .breakpoint_manager
            .update_settings(BreakpointSettings {
                enabled: true,
                max_body_bytes: 64,
            });

        let task_state = state.clone();
        let original = Bytes::from(compress_body(b"old", "gzip").unwrap());
        let expected = original.clone();
        let task = tokio::spawn(async move {
            let mut headers = HeaderMap::new();
            headers.insert(
                hyper::header::CONTENT_ENCODING,
                HeaderValue::from_static("gzip"),
            );
            let mut final_body = original;
            let outcome = breakpoint_response_hook(
                &Some(task_state),
                &None,
                "unsupported-encoding",
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
        wait_until_pending(&state, "unsupported-encoding").await;
        state
            .breakpoint_manager
            .resume(
                "unsupported-encoding",
                "response",
                BreakpointEdit {
                    headers: Some(vec![
                        ("content-encoding".into(), "unsupported".into()),
                        ("x-edited".into(), "yes".into()),
                    ]),
                    body: Some("new-body".into()),
                    ..Default::default()
                },
            )
            .unwrap();

        let (outcome, headers, body) = task.await.unwrap();
        assert!(!outcome.body_replaced);
        assert_eq!(headers[hyper::header::CONTENT_ENCODING], "gzip");
        assert_eq!(headers["x-edited"], "yes");
        assert_eq!(body, expected);

        let task_state = state.clone();
        let original = Bytes::from(compress_body(b"old-request", "gzip").unwrap());
        let expected = original.clone();
        let request_task = tokio::spawn(async move {
            let mut headers = HeaderMap::new();
            headers.insert(
                hyper::header::CONTENT_ENCODING,
                HeaderValue::from_static("gzip"),
            );
            let mut final_body = original;
            let outcome = breakpoint_request_hook(
                &Some(task_state),
                &None,
                "unsupported-request-encoding",
                "POST",
                "http://example.test/",
                &mut headers,
                final_body.clone(),
                Some(11),
                false,
                &mut final_body,
            )
            .await;
            (outcome, headers, final_body)
        });
        wait_until_pending(&state, "unsupported-request-encoding").await;
        state
            .breakpoint_manager
            .resume(
                "unsupported-request-encoding",
                "request",
                BreakpointEdit {
                    headers: Some(vec![("content-encoding".into(), "unsupported".into())]),
                    body: Some("new-request".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        let (outcome, headers, body) = request_task.await.unwrap();
        assert!(!outcome.body_replaced);
        assert_eq!(headers[hyper::header::CONTENT_ENCODING], "gzip");
        assert_eq!(body, expected);
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
        assert!(
            !breakpoint_response_hook(
                &None,
                &None,
                "none-response",
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
        assert!(state
            .breakpoint_manager
            .resume(
                "oversized",
                "response",
                BreakpointEdit {
                    headers: Some(vec![]),
                    body: Some("way too large".into()),
                    ..Default::default()
                }
            )
            .is_ok());
        assert!(!task.await.unwrap().body_replaced);
    }

    #[tokio::test]
    async fn breakpoint_hooks_timeout_and_cancel_without_leaking_pending_state() {
        use bifrost_admin::breakpoint::BreakpointSettings;
        let state = Arc::new(AdminState::new(0));
        state
            .breakpoint_manager
            .update_settings(BreakpointSettings {
                enabled: true,
                max_body_bytes: 64,
            });
        state.breakpoint_manager.set_timeout_ms(5_000);
        let push = Arc::new(bifrost_admin::push::PushManager::new(state.clone()));

        let request_state = state.clone();
        let request_push = push.clone();
        let request = tokio::spawn(async move {
            let mut headers = HeaderMap::new();
            let mut final_body = Bytes::new();
            breakpoint_request_hook(
                &Some(request_state),
                &Some(request_push),
                "request-timeout",
                "GET",
                "http://example.test/timeout",
                &mut headers,
                Bytes::new(),
                None,
                false,
                &mut final_body,
            )
            .await
        });
        let response_state = state.clone();
        let response_push = push;
        let response = tokio::spawn(async move {
            let mut headers = HeaderMap::new();
            let mut final_body = Bytes::new();
            breakpoint_response_hook(
                &Some(response_state),
                &Some(response_push),
                "response-timeout",
                "GET",
                "http://example.test/timeout",
                200,
                &mut headers,
                Bytes::new(),
                None,
                false,
                &mut final_body,
            )
            .await
        });
        wait_until_pending(&state, "request-timeout").await;
        wait_until_pending(&state, "response-timeout").await;
        let (request, response) = tokio::join!(request, response);
        assert_eq!(request.unwrap(), BreakpointHookOutcome::default());
        assert_eq!(response.unwrap(), BreakpointHookOutcome::default());
        assert!(state.breakpoint_manager.pending().is_empty());

        let cancelled_state = state.clone();
        let cancelled = tokio::spawn(async move {
            let mut headers = HeaderMap::new();
            let mut final_body = Bytes::new();
            breakpoint_request_hook(
                &Some(cancelled_state),
                &None,
                "request-cancelled",
                "GET",
                "http://example.test/cancelled",
                &mut headers,
                Bytes::new(),
                None,
                false,
                &mut final_body,
            )
            .await
        });
        wait_until_pending(&state, "request-cancelled").await;
        assert!(state
            .breakpoint_manager
            .cancel("request-cancelled", "request"));
        assert_eq!(cancelled.await.unwrap(), BreakpointHookOutcome::default());

        let cancelled_state = state.clone();
        let cancelled = tokio::spawn(async move {
            let mut headers = HeaderMap::new();
            let mut final_body = Bytes::new();
            breakpoint_response_hook(
                &Some(cancelled_state),
                &None,
                "response-cancelled",
                "GET",
                "http://example.test/cancelled",
                200,
                &mut headers,
                Bytes::new(),
                None,
                false,
                &mut final_body,
            )
            .await
        });
        wait_until_pending(&state, "response-cancelled").await;
        assert!(state
            .breakpoint_manager
            .cancel("response-cancelled", "response"));
        assert_eq!(cancelled.await.unwrap(), BreakpointHookOutcome::default());
    }
}
