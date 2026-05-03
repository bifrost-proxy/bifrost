use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use base64::Engine;
use bifrost_core::{RequestContext, Rule, RuleParser, RulesResolver, ValueStore};
use bytes::{Bytes, BytesMut};
use futures_util::StreamExt;
use http_body_util::BodyExt;
use hyper::{body::Incoming, upgrade, Method, Request, Response, StatusCode, Uri};
use rustls::pki_types::ServerName;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Semaphore;
use tokio_rustls::TlsConnector;

use tracing::{debug, error, info, warn};

use super::replay_ws::{
    compute_accept_key, decompress_permessage_deflate_payload, generate_sec_websocket_key,
    header_values, negotiate_extensions, negotiate_protocol, parse_permessage_deflate,
    read_http1_response_with_leftover, HttpResponse, Opcode, WebSocketReader, WebSocketWriter,
};
use super::{error_response, json_response, method_not_allowed, success_response, BoxBody};
use crate::push::SharedPushManager;
use crate::replay_db::{
    KeyValueItem, ReplayBody, ReplayGroup, ReplayHistory, ReplayRequest, ReplayRequestSummary,
    RequestType, RuleConfig, RuleMode, MAX_CONCURRENT_REPLAYS, MAX_HISTORY, MAX_REQUESTS,
};
use crate::request_rules::{apply_all_request_rules, build_applied_rules, AppliedRequest};
use crate::state::SharedAdminState;
use crate::traffic::{FrameDirection, FrameType, MatchedRule, TrafficRecord};

static REPLAY_SEMAPHORE: once_cell::sync::Lazy<Arc<Semaphore>> =
    once_cell::sync::Lazy::new(|| Arc::new(Semaphore::new(MAX_CONCURRENT_REPLAYS)));

static REPLAY_SEQUENCE: AtomicU64 = AtomicU64::new(1);

fn get_header_value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn effective_authority(url: &str) -> Option<String> {
    let uri: Uri = url.parse().ok()?;
    let scheme = uri.scheme_str().unwrap_or("http");
    let host = uri.host()?.to_ascii_lowercase();
    let port = uri.port_u16().or(match scheme {
        "https" | "wss" => Some(443),
        "http" | "ws" => Some(80),
        _ => None,
    })?;
    Some(format!("{}:{}", host, port))
}

fn replay_target_authority_changed(original_url: &str, applied_url: &str) -> bool {
    match (
        effective_authority(original_url),
        effective_authority(applied_url),
    ) {
        (Some(original), Some(applied)) => original != applied,
        _ => original_url != applied_url,
    }
}

fn should_skip_http_forward_header(name: &str, target_authority_changed: bool) -> bool {
    let n = name.trim().to_ascii_lowercase();
    if n.is_empty() || n.starts_with(':') {
        return true;
    }

    matches!(
        n.as_str(),
        "content-length"
            | "transfer-encoding"
            | "connection"
            | "proxy-connection"
            | "keep-alive"
            | "te"
            | "trailer"
            | "upgrade"
    ) || (target_authority_changed && n == "host")
}

fn decode_replay_body(headers: &[(String, String)], body: &[u8]) -> Option<String> {
    if body.is_empty() {
        return None;
    }

    let encoding = get_header_value(headers, "content-encoding")
        .unwrap_or("")
        .split(',')
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase();

    let decoded = match encoding.as_str() {
        "" => body.to_vec(),
        "gzip" => {
            let mut d = flate2::read::GzDecoder::new(std::io::Cursor::new(body));
            let mut out = Vec::new();
            d.read_to_end(&mut out).ok()?;
            out
        }
        "deflate" => {
            let mut d = flate2::read::ZlibDecoder::new(std::io::Cursor::new(body));
            let mut out = Vec::new();
            d.read_to_end(&mut out).ok()?;
            out
        }
        "br" => {
            let mut d = brotli::Decompressor::new(std::io::Cursor::new(body), 4096);
            let mut out = Vec::new();
            d.read_to_end(&mut out).ok()?;
            out
        }
        "zstd" => zstd::stream::decode_all(std::io::Cursor::new(body)).ok()?,
        _ => body.to_vec(),
    };

    Some(String::from_utf8_lossy(&decoded).to_string())
}

#[cfg(test)]
mod replay_body_decode_tests {
    use super::{
        decode_replay_body, replay_target_authority_changed, should_skip_http_forward_header,
    };

    #[test]
    fn decode_gzip_response_body() {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;

        let raw = br#"{"ok":true,"msg":"hello"}"#;
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(raw).unwrap();
        let gz = enc.finish().unwrap();

        let headers = vec![("content-encoding".to_string(), "gzip".to_string())];
        let decoded = decode_replay_body(&headers, &gz).unwrap();
        assert_eq!(decoded, String::from_utf8_lossy(raw));
    }

    #[test]
    fn replay_forward_skips_stale_host_when_rule_changes_target() {
        assert!(replay_target_authority_changed(
            "https://bifrost.local/api/nextagent/v1/sessions",
            "http://localhost:5173/api/nextagent/v1/sessions"
        ));
        assert!(should_skip_http_forward_header("Host", true));
    }

    #[test]
    fn replay_forward_keeps_custom_host_when_authority_is_unchanged() {
        assert!(!replay_target_authority_changed(
            "https://bifrost.local/api/nextagent/v1/sessions",
            "https://bifrost.local/api/nextagent/v1/sessions"
        ));
        assert!(!should_skip_http_forward_header("Host", false));
    }

    #[test]
    fn replay_forward_skips_hop_by_hop_headers_and_pseudo_headers() {
        for name in [
            "Connection",
            "Content-Length",
            "Transfer-Encoding",
            "Proxy-Connection",
            "Keep-Alive",
            "TE",
            "Trailer",
            "Upgrade",
            ":authority",
        ] {
            assert!(
                should_skip_http_forward_header(name, false),
                "{name} should not be forwarded by unified replay"
            );
        }
    }
}

fn default_method() -> String {
    "GET".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamEvent {
    pub type_: String,
    pub data: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

pub async fn handle_replay(
    req: Request<Incoming>,
    state: SharedAdminState,
    push_manager: Option<SharedPushManager>,
    path: &str,
) -> Response<BoxBody> {
    let method = req.method().clone();

    if path == "/api/replay/execute" || path == "/api/replay/execute/unified" {
        match method {
            Method::POST => execute_replay_unified(req, state, push_manager).await,
            _ => method_not_allowed(),
        }
    } else if path == "/api/replay/execute/ws" {
        match method {
            Method::GET => execute_replay_websocket(req, state, push_manager).await,
            _ => method_not_allowed(),
        }
    } else if path == "/api/replay/groups" || path == "/api/replay/groups/" {
        match method {
            Method::GET => list_groups(state).await,
            Method::POST => create_group(req, state, push_manager).await,
            _ => method_not_allowed(),
        }
    } else if let Some(id) = path.strip_prefix("/api/replay/groups/") {
        match method {
            Method::GET => get_group(state, id).await,
            Method::PUT => update_group(req, state, push_manager, id).await,
            Method::DELETE => delete_group(state, push_manager, id).await,
            _ => method_not_allowed(),
        }
    } else if path == "/api/replay/requests" || path == "/api/replay/requests/" {
        match method {
            Method::GET => list_requests(req, state).await,
            Method::POST => create_request(req, state, push_manager).await,
            _ => method_not_allowed(),
        }
    } else if path == "/api/replay/requests/count" {
        match method {
            Method::GET => count_requests(state).await,
            _ => method_not_allowed(),
        }
    } else if let Some(rest) = path.strip_prefix("/api/replay/requests/") {
        if let Some(id) = rest.strip_suffix("/move") {
            match method {
                Method::PUT => move_request(req, state, push_manager, id).await,
                _ => method_not_allowed(),
            }
        } else {
            match method {
                Method::GET => get_request(state, rest).await,
                Method::PUT => update_request(req, state, push_manager, rest).await,
                Method::DELETE => delete_request(state, push_manager, rest).await,
                _ => method_not_allowed(),
            }
        }
    } else if path == "/api/replay/history" || path == "/api/replay/history/" {
        match method {
            Method::GET => list_history(req, state).await,
            Method::DELETE => clear_history(req, state).await,
            _ => method_not_allowed(),
        }
    } else if path == "/api/replay/history/count" {
        match method {
            Method::GET => count_history(req, state).await,
            _ => method_not_allowed(),
        }
    } else if let Some(id) = path.strip_prefix("/api/replay/history/") {
        match method {
            Method::DELETE => delete_history(state, id).await,
            _ => method_not_allowed(),
        }
    } else if path == "/api/replay/stats" {
        match method {
            Method::GET => get_stats(state).await,
            _ => method_not_allowed(),
        }
    } else {
        error_response(StatusCode::NOT_FOUND, "Not Found")
    }
}

#[derive(Debug, Clone, Deserialize)]
struct UnifiedReplayRequest {
    url: String,
    #[serde(default = "default_method")]
    method: String,
    headers: Vec<(String, String)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rule_config: Option<RuleConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout_ms: Option<u64>,
}

async fn execute_replay_unified(
    req: Request<Incoming>,
    state: SharedAdminState,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    let body = match req.into_body().collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid body: {}", e)),
    };

    let unified_req: UnifiedReplayRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)),
    };

    let permit = match REPLAY_SEMAPHORE.clone().try_acquire_owned() {
        Ok(p) => p,
        Err(_) => {
            return error_response(
                StatusCode::TOO_MANY_REQUESTS,
                "Too many concurrent replay requests",
            )
        }
    };

    let replay_id = format!("replay-{}", REPLAY_SEQUENCE.fetch_add(1, Ordering::SeqCst));
    let rule_config = unified_req.rule_config.clone().unwrap_or(RuleConfig {
        mode: RuleMode::Enabled,
        selected_rules: vec![],
        custom_rules: None,
    });

    let (resolved_rules, matched_rules, applied_request) = resolve_and_apply_rules(
        &state,
        &rule_config,
        &unified_req.url,
        &unified_req.method,
        &unified_req.headers,
        unified_req.body.as_ref().map(|s| s.as_bytes()),
    );

    info!(
        replay_id = %replay_id,
        original_url = %unified_req.url,
        applied_url = %applied_request.url,
        rules_count = matched_rules.len(),
        "[UNIFIED_REPLAY] Applied request rules"
    );

    let applied_url_lower = applied_request.url.to_lowercase();
    if applied_url_lower.starts_with("ws://") || applied_url_lower.starts_with("wss://") {
        drop(permit);
        return error_response(
            StatusCode::BAD_REQUEST,
            "WebSocket URLs are not supported via HTTP endpoint. Use the WebSocket endpoint (/api/replay/execute/ws) instead.",
        );
    }
    // timeout_ms 仅作为错误消息里的参考值，不再通过 reqwest connect_timeout 施加，
    // 因为历史上在 Linux CI 上观察到 SSE 长连接会在 timeout_ms 边界附近被断开，
    // 根因疑似 hyper/reqwest 在某些场景下将 connect_timeout 与 body 读取耦合。
    // SSE 这类流式响应必须允许任意长连接。如需超时控制，应由上游调用方在外部施加。
    let timeout_ms = unified_req
        .timeout_ms
        .unwrap_or(crate::replay_executor::DEFAULT_TIMEOUT_MS);

    let unsafe_ssl = state.runtime_config.read().await.unsafe_ssl;
    let client = bifrost_core::direct_reqwest_client_builder()
        .danger_accept_invalid_certs(unsafe_ssl)
        .build()
        .unwrap_or_default();

    let mut req_builder = match applied_request.method.to_uppercase().as_str() {
        "POST" => client.post(&applied_request.url),
        "PUT" => client.put(&applied_request.url),
        "PATCH" => client.patch(&applied_request.url),
        "DELETE" => client.delete(&applied_request.url),
        "HEAD" => client.head(&applied_request.url),
        "OPTIONS" => client.request(reqwest::Method::OPTIONS, &applied_request.url),
        _ => client.get(&applied_request.url),
    };

    let target_authority_changed =
        replay_target_authority_changed(&unified_req.url, &applied_request.url);
    for (key, value) in &applied_request.headers {
        if should_skip_http_forward_header(key, target_authority_changed) {
            continue;
        }
        req_builder = req_builder.header(key, value);
    }

    if let Some(ref body) = applied_request.body {
        req_builder = req_builder.body(body.clone());
    }

    let start_time = std::time::Instant::now();
    let response = match req_builder.send().await {
        Ok(r) => r,
        Err(e) => {
            drop(permit);
            if e.is_connect() || e.is_timeout() {
                let error_msg = format!("Request timeout after {}ms", timeout_ms);
                error!(replay_id = %replay_id, error = %error_msg, "[UNIFIED_REPLAY] Request timed out");
                return error_response(StatusCode::GATEWAY_TIMEOUT, &error_msg);
            }
            let error_msg = format!("Request failed: {}", e);
            error!(replay_id = %replay_id, error = %error_msg, "[UNIFIED_REPLAY] Request failed");
            return error_response(StatusCode::BAD_GATEWAY, &error_msg);
        }
    };

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();

    let is_sse = content_type.contains("text/event-stream");

    if is_sse {
        info!(replay_id = %replay_id, "[UNIFIED_REPLAY] Detected SSE response, switching to streaming mode");

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

        #[derive(Serialize)]
        struct ConnectionEvent {
            type_: String,
            traffic_id: String,
            url: String,
            applied_url: String,
            applied_rules: Vec<MatchedRule>,
        }

        let traffic_id =
            record_traffic_for_stream(&state, &replay_id, &applied_request, &matched_rules, true);

        let conn_event = ConnectionEvent {
            type_: "connection".to_string(),
            traffic_id: traffic_id.clone(),
            url: unified_req.url.clone(),
            applied_url: applied_request.url.clone(),
            applied_rules: matched_rules.clone(),
        };
        let _ = tx.send(serde_json::to_string(&conn_event).unwrap());

        record_history(
            &state,
            &push_manager,
            unified_req.request_id.as_deref(),
            &traffic_id,
            &applied_request.method,
            &applied_request.url,
            200,
            0,
            &rule_config,
        );

        let state_clone = state.clone();
        let replay_id_clone = replay_id.clone();
        let traffic_id_clone = traffic_id.clone();

        tokio::spawn(async move {
            let result = process_sse_response(
                &state_clone,
                &replay_id_clone,
                &traffic_id_clone,
                response,
                tx,
            )
            .await;
            drop(permit);
            if let Err(e) = result {
                error!(error = %e, replay_id = %replay_id_clone, "[UNIFIED_REPLAY] SSE stream processing failed");
            }
        });

        let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx).map(|event| {
            let mut sse_event = String::from("event: message\n");
            sse_event.push_str(&format!("data: {}\n", event));
            sse_event.push('\n');
            Ok::<_, hyper::Error>(hyper::body::Frame::data(Bytes::from(sse_event)))
        });

        let body = http_body_util::StreamBody::new(stream);

        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/event-stream")
            .header("Cache-Control", "no-cache")
            .header("Connection", "keep-alive")
            .body(BodyExt::boxed(body))
            .unwrap()
    } else {
        let status = response.status().as_u16();
        let response_headers: Vec<(String, String)> = response
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();

        let response_body = match response.bytes().await {
            Ok(b) => decode_replay_body(&response_headers, &b),
            Err(_) => None,
        };

        let (status, response_headers, response_body) =
            apply_response_rules(&resolved_rules, status, response_headers, response_body);

        let duration_ms = start_time.elapsed().as_millis() as u64;
        drop(permit);

        let traffic_id = record_traffic_for_unified(
            &state,
            &replay_id,
            &unified_req,
            &applied_request,
            status,
            &response_headers,
            response_body.as_deref(),
            duration_ms,
            &matched_rules,
        );

        record_history(
            &state,
            &push_manager,
            unified_req.request_id.as_deref(),
            &traffic_id,
            &unified_req.method,
            &unified_req.url,
            status,
            duration_ms,
            &rule_config,
        );

        info!(
            replay_id = %replay_id,
            traffic_id = %traffic_id,
            status = status,
            duration_ms = duration_ms,
            "[UNIFIED_REPLAY] Request completed"
        );

        #[derive(Serialize)]
        struct ExecuteResult {
            success: bool,
            data: UnifiedReplayResponse,
        }

        #[derive(Serialize)]
        struct UnifiedReplayResponse {
            traffic_id: String,
            status: u16,
            headers: Vec<(String, String)>,
            body: Option<String>,
            duration_ms: u64,
            applied_rules: Vec<MatchedRule>,
        }

        json_response(&ExecuteResult {
            success: true,
            data: UnifiedReplayResponse {
                traffic_id,
                status,
                headers: response_headers,
                body: response_body,
                duration_ms,
                applied_rules: matched_rules,
            },
        })
    }
}

async fn process_sse_response(
    state: &SharedAdminState,
    replay_id: &str,
    traffic_id: &str,
    response: reqwest::Response,
    tx: tokio::sync::mpsc::UnboundedSender<String>,
) -> Result<(), String> {
    let mut stream = response.bytes_stream();
    let mut buffer = Vec::new();
    let mut current_event = StreamEvent {
        type_: "message".to_string(),
        data: String::new(),
        id: None,
    };

    while let Some(chunk_result) = stream.next().await {
        if tx.is_closed() {
            info!(replay_id = %replay_id, traffic_id = %traffic_id, "[SSE] Client disconnected, stopping stream processing");
            return Ok(());
        }

        let chunk = chunk_result.map_err(|e| format!("Failed to read chunk: {}", e))?;
        buffer.extend_from_slice(&chunk);

        while buffer.contains(&b'\n') {
            if let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
                let line = buffer.drain(..=pos).collect::<Vec<_>>();
                let line_str = String::from_utf8_lossy(&line[..line.len() - 1]).to_string();

                if line_str.is_empty() {
                    if !current_event.data.is_empty() {
                        let event_json = serde_json::to_string(&current_event).unwrap();
                        if tx.send(event_json).is_err() {
                            info!(replay_id = %replay_id, traffic_id = %traffic_id, "[SSE] Client disconnected, stopping stream processing");
                            return Ok(());
                        }
                        record_sse_event(state, replay_id, traffic_id, &current_event);
                    }
                    current_event = StreamEvent {
                        type_: "message".to_string(),
                        data: String::new(),
                        id: None,
                    };
                } else if let Some(event_type) = line_str.strip_prefix("event:") {
                    current_event.type_ = event_type.trim().to_string();
                } else if let Some(data) = line_str.strip_prefix("data:") {
                    if !current_event.data.is_empty() {
                        current_event.data.push('\n');
                    }
                    current_event.data.push_str(data.trim());
                } else if let Some(id) = line_str.strip_prefix("id:") {
                    current_event.id = Some(id.trim().to_string());
                }
            }
        }
    }

    if !current_event.data.is_empty() && !tx.is_closed() {
        let event_json = serde_json::to_string(&current_event).unwrap();
        let _ = tx.send(event_json);
        record_sse_event(state, replay_id, traffic_id, &current_event);
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn record_traffic_for_unified(
    state: &SharedAdminState,
    replay_id: &str,
    unified_req: &UnifiedReplayRequest,
    applied_request: &AppliedRequest,
    status: u16,
    response_headers: &[(String, String)],
    response_body: Option<&str>,
    duration_ms: u64,
    matched_rules: &[MatchedRule],
) -> String {
    let traffic_id = format!("{}-{}", replay_id, uuid::Uuid::new_v4());
    let timestamp = chrono::Utc::now().timestamp_millis() as u64;

    let uri: Uri = applied_request.url.parse().unwrap_or_default();
    let host = uri.host().unwrap_or("unknown").to_string();
    let path = uri.path().to_string();
    let scheme = uri.scheme_str().unwrap_or("http");

    let request_content_type = applied_request
        .headers
        .iter()
        .find(|(k, _)| k.to_lowercase() == "content-type")
        .map(|(_, v)| v.clone());

    let response_content_type = response_headers
        .iter()
        .find(|(k, _)| k.to_lowercase() == "content-type")
        .map(|(_, v)| v.clone());

    let request_body_ref = if let Some(ref body) = unified_req.body {
        if let Some(ref body_store) = state.body_store {
            body_store.read().store(&traffic_id, "req", body.as_bytes())
        } else {
            None
        }
    } else {
        None
    };

    let response_body_ref = if let Some(body) = response_body {
        if let Some(ref body_store) = state.body_store {
            body_store.read().store(&traffic_id, "res", body.as_bytes())
        } else {
            None
        }
    } else {
        None
    };

    let record = TrafficRecord {
        id: traffic_id.clone(),
        sequence: 0,
        timestamp,
        host,
        method: applied_request.method.clone(),
        url: applied_request.url.clone(),
        path,
        status,
        protocol: scheme.to_string(),
        content_type: response_content_type,
        request_content_type,
        request_size: unified_req.body.as_ref().map(|b| b.len()).unwrap_or(0),
        response_size: response_body.map(|b| b.len()).unwrap_or(0),
        duration_ms,
        client_ip: "127.0.0.1".to_string(),
        client_app: Some("Bifrost Replay".to_string()),
        client_pid: None,
        client_path: None,
        is_tunnel: false,
        is_websocket: false,
        is_sse: false,
        is_h3: false,
        has_rule_hit: !matched_rules.is_empty(),
        is_replay: true,
        frame_count: 0,
        last_frame_id: 0,
        timing: None,
        request_headers: Some(applied_request.headers.clone()),
        original_response_headers: Some(response_headers.to_vec()),
        matched_rules: if matched_rules.is_empty() {
            None
        } else {
            Some(matched_rules.to_vec())
        },
        socket_status: None,
        request_body_ref,
        response_body_ref,
        derived_response_body_ref: None,
        raw_request_body_ref: None,
        raw_response_body_ref: None,
        actual_url: None,
        actual_host: None,
        original_request_headers: None,
        response_headers: None,
        error_message: None,
        req_script_results: None,
        res_script_results: None,
        decode_req_script_results: None,
        decode_res_script_results: None,
        devtools_client_req_id: None,
    };

    if let Some(ref traffic_db) = state.traffic_db_store {
        traffic_db.record(record);
    } else if let Some(ref async_writer) = state.async_traffic_writer {
        async_writer.record(record);
    }

    traffic_id
}

async fn list_groups(state: SharedAdminState) -> Response<BoxBody> {
    let store = match &state.replay_db_store {
        Some(s) => s,
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Replay store not available",
            )
        }
    };

    let groups = store.list_groups();

    #[derive(Serialize)]
    struct GroupsResponse {
        groups: Vec<ReplayGroup>,
    }
    json_response(&GroupsResponse { groups })
}

#[derive(Deserialize)]
struct CreateGroupRequest {
    name: String,
    parent_id: Option<String>,
}

async fn create_group(
    req: Request<Incoming>,
    state: SharedAdminState,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    let store = match &state.replay_db_store {
        Some(s) => s,
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Replay store not available",
            )
        }
    };

    let body = match req.into_body().collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid body: {}", e)),
    };

    let create_req: CreateGroupRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)),
    };

    let now = chrono::Utc::now().timestamp_millis() as u64;
    let group = ReplayGroup {
        id: uuid::Uuid::new_v4().to_string(),
        name: create_req.name,
        parent_id: create_req.parent_id,
        sort_order: 0,
        created_at: now,
        updated_at: now,
    };

    if let Err(e) = store.create_group(&group) {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to create group: {}", e),
        );
    }

    if let Some(pm) = push_manager {
        pm.broadcast_replay_request_updated("group_created", None, Some(&group.id));
        pm.broadcast_replay_groups_snapshot().await;
    }

    json_response(&group)
}

async fn get_group(state: SharedAdminState, id: &str) -> Response<BoxBody> {
    let store = match &state.replay_db_store {
        Some(s) => s,
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Replay store not available",
            )
        }
    };

    match store.get_group(id) {
        Some(group) => json_response(&group),
        None => error_response(StatusCode::NOT_FOUND, "Group not found"),
    }
}

#[derive(Deserialize)]
struct UpdateGroupRequest {
    name: Option<String>,
    parent_id: Option<String>,
    sort_order: Option<i32>,
}

async fn update_group(
    req: Request<Incoming>,
    state: SharedAdminState,
    push_manager: Option<SharedPushManager>,
    id: &str,
) -> Response<BoxBody> {
    let store = match &state.replay_db_store {
        Some(s) => s,
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Replay store not available",
            )
        }
    };

    let mut group = match store.get_group(id) {
        Some(g) => g,
        None => return error_response(StatusCode::NOT_FOUND, "Group not found"),
    };

    let body = match req.into_body().collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid body: {}", e)),
    };

    let update_req: UpdateGroupRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)),
    };

    if let Some(name) = update_req.name {
        group.name = name;
    }
    if let Some(parent_id) = update_req.parent_id {
        group.parent_id = Some(parent_id);
    }
    if let Some(sort_order) = update_req.sort_order {
        group.sort_order = sort_order;
    }
    group.updated_at = chrono::Utc::now().timestamp_millis() as u64;

    if let Err(e) = store.update_group(&group) {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to update group: {}", e),
        );
    }

    if let Some(pm) = push_manager {
        pm.broadcast_replay_request_updated("group_updated", None, Some(id));
        pm.broadcast_replay_groups_snapshot().await;
    }

    json_response(&group)
}

async fn delete_group(
    state: SharedAdminState,
    push_manager: Option<SharedPushManager>,
    id: &str,
) -> Response<BoxBody> {
    let store = match &state.replay_db_store {
        Some(s) => s,
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Replay store not available",
            )
        }
    };

    if let Err(e) = store.delete_group(id) {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to delete group: {}", e),
        );
    }

    if let Some(pm) = push_manager {
        pm.broadcast_replay_request_updated("group_deleted", None, Some(id));
        pm.broadcast_replay_groups_snapshot().await;
        pm.broadcast_replay_saved_requests_snapshot().await;
    }

    success_response("Group deleted")
}

async fn list_requests(req: Request<Incoming>, state: SharedAdminState) -> Response<BoxBody> {
    let store = match &state.replay_db_store {
        Some(s) => s,
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Replay store not available",
            )
        }
    };

    let query = req.uri().query().unwrap_or("");
    let params: std::collections::HashMap<_, _> = url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect();

    let saved_only = params.get("saved").map(|v| v == "true");
    let group_id = params.get("group_id").map(|s| s.as_str());
    let limit = params.get("limit").and_then(|v| v.parse().ok());
    let offset = params.get("offset").and_then(|v| v.parse().ok());

    let requests = store.list_requests(saved_only, group_id, limit, offset);
    let total = store.count_requests();

    #[derive(Serialize)]
    struct RequestsResponse {
        requests: Vec<ReplayRequestSummary>,
        total: usize,
        max_requests: usize,
    }
    json_response(&RequestsResponse {
        requests,
        total,
        max_requests: MAX_REQUESTS,
    })
}

async fn count_requests(state: SharedAdminState) -> Response<BoxBody> {
    let store = match &state.replay_db_store {
        Some(s) => s,
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Replay store not available",
            )
        }
    };

    let count = store.count_requests();

    #[derive(Serialize)]
    struct CountResponse {
        count: usize,
        max_requests: usize,
    }
    json_response(&CountResponse {
        count,
        max_requests: MAX_REQUESTS,
    })
}

#[derive(Deserialize)]
struct CreateRequestRequest {
    #[serde(default)]
    group_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    request_type: RequestType,
    method: String,
    url: String,
    #[serde(default)]
    headers: Vec<KeyValueItem>,
    #[serde(default)]
    body: Option<ReplayBody>,
    #[serde(default)]
    is_saved: bool,
}

async fn create_request(
    req: Request<Incoming>,
    state: SharedAdminState,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    let store = match &state.replay_db_store {
        Some(s) => s,
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Replay store not available",
            )
        }
    };

    let count = store.count_requests();
    if count >= MAX_REQUESTS {
        return error_response(
            StatusCode::CONFLICT,
            &format!(
                "Maximum request limit ({}) reached. Please delete some requests first.",
                MAX_REQUESTS
            ),
        );
    }

    let body = match req.into_body().collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid body: {}", e)),
    };

    let create_req: CreateRequestRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)),
    };

    let now = chrono::Utc::now().timestamp_millis() as u64;
    let request = ReplayRequest {
        id: uuid::Uuid::new_v4().to_string(),
        group_id: create_req.group_id,
        name: create_req.name,
        request_type: create_req.request_type,
        method: create_req.method,
        url: create_req.url,
        headers: create_req.headers,
        body: create_req.body,
        is_saved: create_req.is_saved,
        sort_order: 0,
        source: crate::replay_db::RequestSource::Internal,
        created_at: now,
        updated_at: now,
    };

    if let Err(e) = store.create_request(&request) {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to create request: {}", e),
        );
    }

    if let Some(pm) = push_manager {
        pm.broadcast_replay_request_updated(
            "request_created",
            Some(&request.id),
            request.group_id.as_deref(),
        );
        pm.broadcast_replay_saved_requests_snapshot().await;
        pm.broadcast_replay_groups_snapshot().await;
    }

    json_response(&request)
}

async fn get_request(state: SharedAdminState, id: &str) -> Response<BoxBody> {
    let store = match &state.replay_db_store {
        Some(s) => s,
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Replay store not available",
            )
        }
    };

    match store.get_request(id) {
        Some(request) => json_response(&request),
        None => error_response(StatusCode::NOT_FOUND, "Request not found"),
    }
}

#[derive(Deserialize)]
struct UpdateRequestRequest {
    #[serde(default)]
    group_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    request_type: Option<RequestType>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    headers: Option<Vec<KeyValueItem>>,
    #[serde(default)]
    body: Option<ReplayBody>,
    #[serde(default)]
    is_saved: Option<bool>,
    #[serde(default)]
    sort_order: Option<i32>,
}

async fn update_request(
    req: Request<Incoming>,
    state: SharedAdminState,
    push_manager: Option<SharedPushManager>,
    id: &str,
) -> Response<BoxBody> {
    let store = match &state.replay_db_store {
        Some(s) => s,
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Replay store not available",
            )
        }
    };

    let mut request = match store.get_request(id) {
        Some(r) => r,
        None => return error_response(StatusCode::NOT_FOUND, "Request not found"),
    };

    let body = match req.into_body().collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid body: {}", e)),
    };

    let update_req: UpdateRequestRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)),
    };

    if update_req.group_id.is_some() {
        request.group_id = update_req.group_id;
    }
    if let Some(name) = update_req.name {
        request.name = Some(name);
    }
    if let Some(request_type) = update_req.request_type {
        request.request_type = request_type;
    }
    if let Some(method) = update_req.method {
        request.method = method;
    }
    if let Some(url) = update_req.url {
        request.url = url;
    }
    if let Some(headers) = update_req.headers {
        request.headers = headers;
    }
    if update_req.body.is_some() {
        request.body = update_req.body;
    }
    if let Some(is_saved) = update_req.is_saved {
        request.is_saved = is_saved;
    }
    if let Some(sort_order) = update_req.sort_order {
        request.sort_order = sort_order;
    }
    request.updated_at = chrono::Utc::now().timestamp_millis() as u64;

    if let Err(e) = store.update_request(&request) {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to update request: {}", e),
        );
    }

    if let Some(pm) = push_manager {
        pm.broadcast_replay_request_updated(
            "request_updated",
            Some(&request.id),
            request.group_id.as_deref(),
        );
        pm.broadcast_replay_saved_requests_snapshot().await;
        pm.broadcast_replay_groups_snapshot().await;
    }

    json_response(&request)
}

async fn delete_request(
    state: SharedAdminState,
    push_manager: Option<SharedPushManager>,
    id: &str,
) -> Response<BoxBody> {
    let store = match &state.replay_db_store {
        Some(s) => s,
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Replay store not available",
            )
        }
    };

    if let Err(e) = store.delete_request(id) {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to delete request: {}", e),
        );
    }

    if let Some(pm) = push_manager {
        pm.broadcast_replay_request_updated("request_deleted", Some(id), None);
        pm.broadcast_replay_saved_requests_snapshot().await;
        pm.broadcast_replay_groups_snapshot().await;
    }

    success_response("Request deleted")
}

#[derive(Deserialize)]
struct MoveRequestRequest {
    group_id: Option<String>,
}

async fn move_request(
    req: Request<Incoming>,
    state: SharedAdminState,
    push_manager: Option<SharedPushManager>,
    id: &str,
) -> Response<BoxBody> {
    let store = match &state.replay_db_store {
        Some(s) => s,
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Replay store not available",
            )
        }
    };

    let body = match req.into_body().collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid body: {}", e)),
    };

    let move_req: MoveRequestRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("Invalid JSON: {}", e)),
    };

    if let Err(e) = store.move_request_to_group(id, move_req.group_id.as_deref()) {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to move request: {}", e),
        );
    }

    if let Some(pm) = push_manager {
        pm.broadcast_replay_request_updated(
            "request_updated",
            Some(id),
            move_req.group_id.as_deref(),
        );
        pm.broadcast_replay_saved_requests_snapshot().await;
        pm.broadcast_replay_groups_snapshot().await;
    }

    success_response("Request moved")
}

async fn list_history(req: Request<Incoming>, state: SharedAdminState) -> Response<BoxBody> {
    let store = match &state.replay_db_store {
        Some(s) => s,
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Replay store not available",
            )
        }
    };

    let query = req.uri().query().unwrap_or("");
    let params: std::collections::HashMap<_, _> = url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect();

    let request_id = params.get("request_id").map(|s| s.as_str());
    let request_unbound_only = matches!(params.get("binding").map(String::as_str), Some("unbound"));
    let limit = params.get("limit").and_then(|v| v.parse().ok());
    let offset = params.get("offset").and_then(|v| v.parse().ok());

    let history = store.list_history(request_id, request_unbound_only, limit, offset);
    let total = store.count_history(request_id, request_unbound_only);

    #[derive(Serialize)]
    struct HistoryResponse {
        history: Vec<ReplayHistory>,
        total: usize,
        max_history: usize,
    }
    json_response(&HistoryResponse {
        history,
        total,
        max_history: MAX_HISTORY,
    })
}

async fn count_history(req: Request<Incoming>, state: SharedAdminState) -> Response<BoxBody> {
    let store = match &state.replay_db_store {
        Some(s) => s,
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Replay store not available",
            )
        }
    };

    let query = req.uri().query().unwrap_or("");
    let params: std::collections::HashMap<_, _> = url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect();

    let request_id = params.get("request_id").map(|s| s.as_str());
    let request_unbound_only = matches!(params.get("binding").map(String::as_str), Some("unbound"));
    let count = store.count_history(request_id, request_unbound_only);

    #[derive(Serialize)]
    struct CountResponse {
        count: usize,
        max_history: usize,
    }
    json_response(&CountResponse {
        count,
        max_history: MAX_HISTORY,
    })
}

async fn delete_history(state: SharedAdminState, id: &str) -> Response<BoxBody> {
    let store = match &state.replay_db_store {
        Some(s) => s,
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Replay store not available",
            )
        }
    };

    if let Err(e) = store.delete_history(id) {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to delete history: {}", e),
        );
    }

    success_response("History deleted")
}

async fn clear_history(req: Request<Incoming>, state: SharedAdminState) -> Response<BoxBody> {
    let store = match &state.replay_db_store {
        Some(s) => s,
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Replay store not available",
            )
        }
    };

    let query = req.uri().query().unwrap_or("");
    let params: std::collections::HashMap<_, _> = url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect();

    let request_id = params.get("request_id").map(|s| s.as_str());

    match store.clear_history(request_id) {
        Ok(deleted) => {
            #[derive(Serialize)]
            struct ClearResponse {
                success: bool,
                deleted: usize,
            }
            json_response(&ClearResponse {
                success: true,
                deleted,
            })
        }
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to clear history: {}", e),
        ),
    }
}

async fn get_stats(state: SharedAdminState) -> Response<BoxBody> {
    let store = match &state.replay_db_store {
        Some(s) => s,
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Replay store not available",
            )
        }
    };

    let stats = store.stats();
    json_response(&stats)
}

async fn execute_replay_websocket(
    req: Request<Incoming>,
    state: SharedAdminState,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    if req.method() != Method::GET {
        return method_not_allowed();
    }

    let upgrade_header = req
        .headers()
        .get("Upgrade")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !upgrade_header.eq_ignore_ascii_case("websocket") {
        return error_response(StatusCode::BAD_REQUEST, "Invalid upgrade header");
    }

    let connection_header = req
        .headers()
        .get("Connection")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !header_contains_token(connection_header, "upgrade") {
        return error_response(StatusCode::BAD_REQUEST, "Invalid connection header");
    }

    let ws_version = req
        .headers()
        .get("Sec-WebSocket-Version")
        .and_then(|v| v.to_str().ok());
    if let Some(v) = ws_version {
        if v.trim() != "13" {
            return Response::builder()
                .status(StatusCode::UPGRADE_REQUIRED)
                .header("Sec-WebSocket-Version", "13")
                .body(BoxBody::default())
                .unwrap();
        }
    }

    let ws_key = match req.headers().get("Sec-WebSocket-Key") {
        Some(key) => key.to_str().unwrap_or("").to_string(),
        None => {
            return error_response(StatusCode::BAD_REQUEST, "Missing Sec-WebSocket-Key header");
        }
    };
    let key_ok = base64::engine::general_purpose::STANDARD
        .decode(ws_key.as_bytes())
        .map(|v| v.len() == 16)
        .unwrap_or(false);
    if !key_ok {
        return error_response(StatusCode::BAD_REQUEST, "Invalid Sec-WebSocket-Key header");
    }

    let client_protocol_offer = req
        .headers()
        .get("Sec-WebSocket-Protocol")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let client_extensions_offer = req
        .headers()
        .get("Sec-WebSocket-Extensions")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let upgrade_headers: Vec<(String, String)> = req
        .headers()
        .iter()
        .filter(|(k, _)| {
            let name = k.as_str().to_lowercase();
            !matches!(
                name.as_str(),
                "upgrade" | "connection" | "sec-websocket-key" | "sec-websocket-version" | "host"
            )
        })
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();

    let query = req.uri().query().unwrap_or("");
    let params: HashMap<_, _> = url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect();

    let url = match params.get("url") {
        Some(url) => url.clone(),
        None => return error_response(StatusCode::BAD_REQUEST, "Missing url parameter"),
    };

    let request_id = params.get("request_id").cloned();

    let rule_config = if let Some(rule_config_str) = params.get("rule_config") {
        serde_json::from_str(rule_config_str).unwrap_or(RuleConfig {
            mode: RuleMode::None,
            selected_rules: vec![],
            custom_rules: None,
        })
    } else {
        RuleConfig {
            mode: RuleMode::None,
            selected_rules: vec![],
            custom_rules: None,
        }
    };

    let replay_id = format!("replay-{}", REPLAY_SEQUENCE.fetch_add(1, Ordering::SeqCst));

    info!(replay_id = %replay_id, url = %url, "Starting WebSocket proxy");

    let (_resolved_rules, matched_rules, applied_request) =
        resolve_and_apply_rules(&state, &rule_config, &url, "GET", &upgrade_headers, None);

    info!(
        replay_id = %replay_id,
        original_url = %url,
        applied_url = %applied_request.url,
        rules_count = matched_rules.len(),
        "[WS_REPLAY] Applied request rules"
    );

    let traffic_id =
        record_traffic_for_stream(&state, &replay_id, &applied_request, &matched_rules, false);

    record_history(
        &state,
        &push_manager,
        request_id.as_deref(),
        &traffic_id,
        "GET",
        &applied_request.url,
        101,
        0,
        &rule_config,
    );

    state.connection_monitor.register_connection(&traffic_id);

    let unsafe_ssl = state.runtime_config.read().await.unsafe_ssl;
    let (upstream_stream, upstream_resp, upstream_leftover) = match connect_upstream_websocket(
        &applied_request.url,
        &applied_request.headers,
        unsafe_ssl,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                &format!("Failed to connect to target WebSocket: {}", e),
            )
        }
    };

    let upstream_protocol = upstream_resp.header("Sec-WebSocket-Protocol");
    let upstream_extensions = header_values(&upstream_resp, "Sec-WebSocket-Extensions");
    let negotiated_protocol =
        negotiate_protocol(client_protocol_offer.as_deref(), upstream_protocol);
    let negotiated_extensions =
        negotiate_extensions(client_extensions_offer.as_deref(), &upstream_extensions);
    let compression_enabled = negotiated_extensions
        .as_deref()
        .map(parse_permessage_deflate)
        .unwrap_or(false);

    let accept_key = compute_accept_key(&ws_key);
    let upstream_headers = upstream_resp.headers.clone();

    let replay_id_for_task = replay_id.clone();
    let traffic_id_for_task = traffic_id.clone();
    let state_for_task = state.clone();
    let frame_store = state.frame_store.clone();
    let ws_payload_store = state.ws_payload_store.clone();
    tokio::spawn(async move {
        let upgraded = match tokio::time::timeout(
            std::time::Duration::from_secs(15),
            upgrade::on(req),
        )
        .await
        {
            Ok(Ok(u)) => u,
            Ok(Err(e)) => {
                error!(error = %e, "WebSocket upgrade failed");
                return;
            }
            Err(_) => {
                error!("WebSocket upgrade timeout");
                return;
            }
        };

        if let Err(e) = websocket_bidirectional_generic_with_capture(
            upgraded,
            upstream_stream,
            upstream_leftover,
            &traffic_id_for_task,
            &state_for_task,
            compression_enabled,
        )
        .await
        {
            error!(error = %e, replay_id = %replay_id_for_task, traffic_id = %traffic_id_for_task, "WebSocket replay tunnel error");
        }

        let should_close = state_for_task
            .connection_monitor
            .get_connection_status(&traffic_id_for_task)
            .map(|s| s.is_open)
            .unwrap_or(false);
        if should_close {
            state_for_task.connection_monitor.set_connection_closed(
                &traffic_id_for_task,
                None,
                None,
                frame_store.as_ref(),
                ws_payload_store.as_ref(),
            );
        }
        persist_socket_summary(&state_for_task, &traffic_id_for_task);

        info!(replay_id = %replay_id_for_task, traffic_id = %traffic_id_for_task, "WebSocket proxy closed");
    });

    let mut response = Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header("Upgrade", "websocket")
        .header("Connection", "Upgrade")
        .header("Sec-WebSocket-Accept", accept_key);

    if let Some(protocol) = negotiated_protocol {
        response = response.header("Sec-WebSocket-Protocol", protocol);
    }
    if let Some(extensions) = negotiated_extensions {
        response = response.header("Sec-WebSocket-Extensions", extensions);
    }
    for (name, value) in upstream_headers {
        let lower = name.to_ascii_lowercase();
        if lower != "upgrade"
            && lower != "connection"
            && lower != "sec-websocket-accept"
            && lower != "sec-websocket-protocol"
            && lower != "sec-websocket-extensions"
        {
            response = response.header(name, value);
        }
    }

    response.body(BoxBody::default()).unwrap()
}

fn header_contains_token(header_value: &str, token: &str) -> bool {
    header_value
        .split(',')
        .flat_map(|s| s.split(';'))
        .map(|s| s.trim().to_ascii_lowercase())
        .any(|t| t == token)
}

trait WsIo: AsyncRead + AsyncWrite {}
impl<T: AsyncRead + AsyncWrite + ?Sized> WsIo for T {}
type BoxedWsStream = Box<dyn WsIo + Unpin + Send>;

async fn connect_upstream_websocket(
    url: &str,
    headers: &[(String, String)],
    unsafe_ssl: bool,
) -> Result<(BoxedWsStream, HttpResponse, BytesMut), String> {
    let parsed_url = url::Url::parse(url).map_err(|e| format!("Invalid URL: {}", e))?;
    let is_secure = match parsed_url.scheme() {
        "wss" | "https" => true,
        "ws" | "http" => false,
        scheme => return Err(format!("Unsupported scheme: {}", scheme)),
    };

    let ws_url = if parsed_url.scheme() == "http" || parsed_url.scheme() == "https" {
        let mut new = parsed_url.clone();
        let scheme = if is_secure { "wss" } else { "ws" };
        new.set_scheme(scheme)
            .map_err(|_| format!("Failed to set scheme to {}", scheme))?;
        new
    } else {
        parsed_url.clone()
    };

    let host = ws_url
        .host_str()
        .ok_or_else(|| "Missing host".to_string())?
        .to_string();
    let port = ws_url.port().unwrap_or(if is_secure { 443 } else { 80 });
    let addr = format!("{}:{}", host, port);

    debug!(
        url = %ws_url.as_str(),
        host = %host,
        port = %port,
        is_secure = %is_secure,
        "[WS_REPLAY] Connecting to upstream WebSocket"
    );

    let tcp_stream = TcpStream::connect(&addr)
        .await
        .map_err(|e| format!("Failed to connect to {}: {}", addr, e))?;

    let _ = tcp_stream.set_nodelay(true);

    let path = {
        let mut p = ws_url.path().to_string();
        if p.is_empty() {
            p.push('/');
        }
        if let Some(q) = ws_url.query() {
            p.push('?');
            p.push_str(q);
        }
        p
    };

    let host_header = if (is_secure && port == 443) || (!is_secure && port == 80) {
        host.clone()
    } else {
        format!("{}:{}", host, port)
    };

    let ws_key = generate_sec_websocket_key();
    let handshake = build_upstream_websocket_handshake(&path, &host_header, &ws_key, headers);

    if is_secure {
        let connector = TlsConnector::from(Arc::new(get_ws_tls_client_config(unsafe_ssl)));
        let server_name =
            ServerName::try_from(host).map_err(|e| format!("Invalid server name: {}", e))?;
        let mut tls_stream = connector
            .connect(server_name, tcp_stream)
            .await
            .map_err(|e| format!("TLS handshake failed: {}", e))?;

        tls_stream
            .write_all(handshake.as_bytes())
            .await
            .map_err(|e| format!("Failed to send handshake: {}", e))?;

        let (resp, leftover) = read_http1_response_with_leftover(&mut tls_stream).await?;
        validate_upstream_handshake(&resp, &ws_key)?;
        Ok((Box::new(tls_stream), resp, leftover))
    } else {
        let mut stream = tcp_stream;
        stream
            .write_all(handshake.as_bytes())
            .await
            .map_err(|e| format!("Failed to send handshake: {}", e))?;

        let (resp, leftover) = read_http1_response_with_leftover(&mut stream).await?;
        validate_upstream_handshake(&resp, &ws_key)?;
        Ok((Box::new(stream), resp, leftover))
    }
}

fn build_upstream_websocket_handshake(
    path: &str,
    host: &str,
    key: &str,
    headers: &[(String, String)],
) -> String {
    let mut handshake = format!(
        "GET {} HTTP/1.1\r\n\
         Host: {}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: {}\r\n\
         Sec-WebSocket-Version: 13\r\n",
        path, host, key
    );

    for (name, value) in headers {
        if should_skip_ws_forward_header(name) {
            continue;
        }
        handshake.push_str(&format!("{}: {}\r\n", name, value));
    }
    handshake.push_str("\r\n");
    handshake
}

fn should_skip_ws_forward_header(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    matches!(
        n.as_str(),
        "host"
            | "upgrade"
            | "connection"
            | "sec-websocket-key"
            | "sec-websocket-version"
            | "content-length"
            | "transfer-encoding"
            | "proxy-connection"
            | "keep-alive"
            | "te"
            | "trailer"
    )
}

fn validate_upstream_handshake(resp: &HttpResponse, ws_key: &str) -> Result<(), String> {
    if resp.status_code != 101 {
        return Err(format!(
            "WebSocket handshake failed: {} {}",
            resp.status_code, resp.status_text
        ));
    }
    let expected = compute_accept_key(ws_key);
    let got = resp
        .header("Sec-WebSocket-Accept")
        .unwrap_or("")
        .trim()
        .to_string();
    if got != expected {
        return Err("Invalid Sec-WebSocket-Accept from upstream".to_string());
    }
    Ok(())
}

fn get_ws_tls_client_config(unsafe_ssl: bool) -> rustls::ClientConfig {
    use rustls::{ClientConfig, RootCertStore};

    if unsafe_ssl {
        ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoCertificateVerification {}))
            .with_no_client_auth()
    } else {
        let mut root_store = RootCertStore::empty();
        let certs = rustls_native_certs::load_native_certs();
        for cert in certs.certs {
            let _ = root_store.add(cert);
        }

        ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth()
    }
}

#[derive(Debug)]
struct NoCertificateVerification;

impl rustls::client::danger::ServerCertVerifier for NoCertificateVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP521_SHA512,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::ED25519,
        ]
    }
}

fn opcode_to_frame_type(opcode: Opcode) -> FrameType {
    match opcode {
        Opcode::Continuation => FrameType::Continuation,
        Opcode::Text => FrameType::Text,
        Opcode::Binary => FrameType::Binary,
        Opcode::Close => FrameType::Close,
        Opcode::Ping => FrameType::Ping,
        Opcode::Pong => FrameType::Pong,
    }
}

async fn websocket_bidirectional_generic_with_capture(
    upgraded: upgrade::Upgraded,
    target: BoxedWsStream,
    upstream_leftover: BytesMut,
    connection_id: &str,
    state: &SharedAdminState,
    compression_enabled: bool,
) -> Result<(), String> {
    let client = hyper_util::rt::TokioIo::new(upgraded);
    let (target_read, target_write) = tokio::io::split(target);
    let (client_read, client_write) = tokio::io::split(client);

    let id_c2s = connection_id.to_string();
    let id_s2c = connection_id.to_string();
    let state_c2s = state.clone();
    let state_s2c = state.clone();

    let client_to_server = async move {
        let mut reader = WebSocketReader::new(client_read);
        let mut writer = WebSocketWriter::new(target_write, true);

        // permessage-deflate 允许“压缩消息 + fragmentation”。RSV1 通常只在首帧置位，后续 Continuation 帧不再置位。
        // 这里在同一条消息 FIN 结束时对整条消息解压，用于记录/展示；转发仍保持原始帧不变。
        let mut pending_compressed: Option<(FrameType, BytesMut)> = None;
        const MAX_COMPRESSED_MESSAGE_LEN: usize = 32 * 1024 * 1024;

        loop {
            let Some(frame) = reader
                .next_frame()
                .await
                .map_err(|e| format!("Client read error: {}", e))?
            else {
                break;
            };

            let frame_type = opcode_to_frame_type(frame.opcode);

            let mut raw_payload: Option<Bytes> = None;
            let mut payload_for_record = frame.payload.clone();
            let mut payload_is_text = matches!(frame_type, FrameType::Text | FrameType::Close);

            if compression_enabled {
                match frame.opcode {
                    Opcode::Text | Opcode::Binary => {
                        // 首帧：如果 rsv1=1 且被分片（fin=0），开始累计压缩碎片
                        if frame.rsv1 && !frame.fin {
                            let mut buf = BytesMut::with_capacity(frame.payload.len().min(8192));
                            buf.extend_from_slice(&frame.payload);
                            pending_compressed = Some((frame_type, buf));
                        } else if frame.rsv1 && frame.fin {
                            // 未分片：直接解压
                            raw_payload = Some(frame.payload.clone());
                            payload_for_record =
                                decompress_permessage_deflate_payload(&frame.payload);
                        }
                    }
                    Opcode::Continuation => {
                        if let Some((_, buf)) = pending_compressed.as_mut() {
                            if buf.len().saturating_add(frame.payload.len())
                                <= MAX_COMPRESSED_MESSAGE_LEN
                            {
                                buf.extend_from_slice(&frame.payload);
                            } else {
                                // 超出限制时放弃累计，避免内存压力
                                pending_compressed = None;
                            }

                            if frame.fin {
                                if let Some((start_type, buf)) = pending_compressed.take() {
                                    raw_payload = Some(buf.freeze());
                                    payload_for_record = decompress_permessage_deflate_payload(
                                        raw_payload.as_ref().unwrap(),
                                    );
                                    // Continuation 帧在 UI 侧仍希望按原始消息类型展示
                                    payload_is_text =
                                        matches!(start_type, FrameType::Text | FrameType::Close);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }

            state_c2s.connection_monitor.record_frame(
                &id_c2s,
                FrameDirection::Send,
                frame_type,
                payload_for_record.as_ref(),
                payload_is_text,
                raw_payload.as_ref().map(|b| b.as_ref()),
                frame.mask.is_some(),
                frame.fin,
                state_c2s.body_store.as_ref(),
                state_c2s.ws_payload_store.as_ref(),
                state_c2s.frame_store.as_ref(),
            );

            if frame.opcode == Opcode::Close {
                let code = frame.close_code();
                let reason = frame.close_reason().map(str::to_string);
                state_c2s.connection_monitor.set_connection_closed(
                    &id_c2s,
                    code,
                    reason,
                    state_c2s.frame_store.as_ref(),
                    state_c2s.ws_payload_store.as_ref(),
                );
                writer
                    .write_frame(frame)
                    .await
                    .map_err(|e| format!("Server write error: {}", e))?;
                break;
            }

            writer
                .write_frame(frame)
                .await
                .map_err(|e| format!("Server write error: {}", e))?;
        }

        Ok::<_, String>(())
    };

    let server_to_client = async move {
        let mut reader = WebSocketReader::with_initial_buffer(target_read, upstream_leftover);
        let mut writer = WebSocketWriter::new(client_write, false);

        let mut pending_compressed: Option<(FrameType, BytesMut)> = None;
        const MAX_COMPRESSED_MESSAGE_LEN: usize = 32 * 1024 * 1024;

        loop {
            let Some(frame) = reader
                .next_frame()
                .await
                .map_err(|e| format!("Server read error: {}", e))?
            else {
                break;
            };

            let frame_type = opcode_to_frame_type(frame.opcode);

            let mut raw_payload: Option<Bytes> = None;
            let mut payload_for_record = frame.payload.clone();
            let mut payload_is_text = matches!(frame_type, FrameType::Text | FrameType::Close);

            if compression_enabled {
                match frame.opcode {
                    Opcode::Text | Opcode::Binary => {
                        if frame.rsv1 && !frame.fin {
                            let mut buf = BytesMut::with_capacity(frame.payload.len().min(8192));
                            buf.extend_from_slice(&frame.payload);
                            pending_compressed = Some((frame_type, buf));
                        } else if frame.rsv1 && frame.fin {
                            raw_payload = Some(frame.payload.clone());
                            payload_for_record =
                                decompress_permessage_deflate_payload(&frame.payload);
                        }
                    }
                    Opcode::Continuation => {
                        if let Some((_, buf)) = pending_compressed.as_mut() {
                            if buf.len().saturating_add(frame.payload.len())
                                <= MAX_COMPRESSED_MESSAGE_LEN
                            {
                                buf.extend_from_slice(&frame.payload);
                            } else {
                                pending_compressed = None;
                            }

                            if frame.fin {
                                if let Some((start_type, buf)) = pending_compressed.take() {
                                    raw_payload = Some(buf.freeze());
                                    payload_for_record = decompress_permessage_deflate_payload(
                                        raw_payload.as_ref().unwrap(),
                                    );
                                    payload_is_text =
                                        matches!(start_type, FrameType::Text | FrameType::Close);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }

            state_s2c.connection_monitor.record_frame(
                &id_s2c,
                FrameDirection::Receive,
                frame_type,
                payload_for_record.as_ref(),
                payload_is_text,
                raw_payload.as_ref().map(|b| b.as_ref()),
                frame.mask.is_some(),
                frame.fin,
                state_s2c.body_store.as_ref(),
                state_s2c.ws_payload_store.as_ref(),
                state_s2c.frame_store.as_ref(),
            );

            if frame.opcode == Opcode::Close {
                let code = frame.close_code();
                let reason = frame.close_reason().map(str::to_string);
                state_s2c.connection_monitor.set_connection_closed(
                    &id_s2c,
                    code,
                    reason,
                    state_s2c.frame_store.as_ref(),
                    state_s2c.ws_payload_store.as_ref(),
                );
                writer
                    .write_frame(frame)
                    .await
                    .map_err(|e| format!("Client write error: {}", e))?;
                break;
            }

            writer
                .write_frame(frame)
                .await
                .map_err(|e| format!("Client write error: {}", e))?;
        }

        Ok::<_, String>(())
    };

    // 不能对整条 WebSocket 双向转发做固定超时：长连接（>30s）会被必断。
    // 这里用“任一方向结束即结束整体”的策略，并主动取消另一侧任务，避免半关闭导致的资源悬挂。
    let mut c2s = tokio::spawn(client_to_server);
    let mut s2c = tokio::spawn(server_to_client);

    let joined = tokio::select! {
        r = &mut c2s => {
            s2c.abort();
            r
        }
        r = &mut s2c => {
            c2s.abort();
            r
        }
    };

    match joined {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e),
        Err(e) => Err(format!("WebSocket task join error: {}", e)),
    }
}

fn persist_socket_summary(state: &SharedAdminState, record_id: &str) {
    let status = state.connection_monitor.get_connection_status(record_id);
    let last_frame_id = state
        .connection_monitor
        .get_last_frame_id(record_id)
        .unwrap_or(0);
    let frame_count = status.as_ref().map(|s| s.frame_count).unwrap_or(0);
    let status = status.map(|mut s| {
        s.is_open = false;
        s
    });
    let response_size = status
        .as_ref()
        .map(|s| s.send_bytes + s.receive_bytes)
        .unwrap_or(0) as usize;
    state.update_traffic_by_id(record_id, move |record| {
        record.response_size = response_size;
        record.frame_count = frame_count;
        record.last_frame_id = last_frame_id;
        if let Some(ref s) = status {
            record.socket_status = Some(s.clone());
        }
    });
}

fn resolve_and_apply_rules(
    state: &SharedAdminState,
    rule_config: &RuleConfig,
    url: &str,
    method: &str,
    headers: &[(String, String)],
    body: Option<&[u8]>,
) -> (
    bifrost_core::ResolvedRules,
    Vec<MatchedRule>,
    AppliedRequest,
) {
    let (resolved_rules, matched_rules) = match rule_config.mode {
        RuleMode::None => (bifrost_core::ResolvedRules::default(), vec![]),
        RuleMode::Custom => {
            if let Some(ref custom_rules) = rule_config.custom_rules {
                resolve_custom_rules(state, custom_rules, url, method)
            } else {
                (bifrost_core::ResolvedRules::default(), vec![])
            }
        }
        RuleMode::Enabled | RuleMode::Selected => {
            let selected = if rule_config.mode == RuleMode::Selected {
                Some(&rule_config.selected_rules)
            } else {
                None
            };
            resolve_from_storage(state, url, method, selected)
        }
    };

    let rules_to_apply = build_applied_rules(&resolved_rules);

    let applied_request =
        match apply_all_request_rules(url, method, headers, body, &rules_to_apply, true) {
            Ok(req) => req,
            Err(e) => {
                warn!(error = %e, "[REPLAY] Failed to apply rules, using original request");
                AppliedRequest {
                    url: url.to_string(),
                    method: method.to_string(),
                    headers: headers.to_vec(),
                    body: body.map(Bytes::copy_from_slice),
                }
            }
        };

    (resolved_rules, matched_rules, applied_request)
}

fn extract_inline_content(value: &str) -> String {
    if value.starts_with('{') && value.ends_with('}') && value.len() > 1 {
        value[1..value.len() - 1].to_string()
    } else {
        value.to_string()
    }
}

fn parse_headers(value: &str) -> Option<Vec<(String, String)>> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }

    let (content, use_colon) = if trimmed.starts_with('(') && trimmed.ends_with(')') {
        (&trimmed[1..trimmed.len() - 1], true)
    } else {
        (trimmed, trimmed.contains('\n') || trimmed.contains(':'))
    };

    let mut headers = Vec::new();
    let delimiter = if content.contains('\n') { '\n' } else { ',' };
    for part in content.split(delimiter) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let separator = if use_colon { ':' } else { '=' };
        if let Some(pos) = part.find(separator) {
            let key = part[..pos].trim().to_string();
            let val = part[pos + 1..].trim().to_string();
            if !key.is_empty() {
                headers.push((key, val));
            }
        }
    }

    if headers.is_empty() {
        None
    } else {
        Some(headers)
    }
}

fn apply_response_rules(
    resolved_rules: &bifrost_core::ResolvedRules,
    status: u16,
    mut headers: Vec<(String, String)>,
    body: Option<String>,
) -> (u16, Vec<(String, String)>, Option<String>) {
    use bifrost_core::Protocol;

    let mut final_status = status;
    let mut final_body = body;

    for rule in &resolved_rules.rules {
        match rule.rule.protocol {
            Protocol::ResHeaders => {
                if let Some(parsed) = parse_headers(&rule.resolved_value) {
                    for (key, value) in parsed {
                        let key_lower = key.to_lowercase();
                        headers.retain(|(k, _)| k.to_lowercase() != key_lower);
                        headers.push((key, value));
                    }
                }
            }
            Protocol::StatusCode | Protocol::ReplaceStatus => {
                if let Ok(code) = rule.resolved_value.parse::<u16>() {
                    final_status = code;
                }
            }
            Protocol::ResBody => {
                let content = extract_inline_content(&rule.resolved_value);
                final_body = Some(content);
            }
            _ => {}
        }
    }

    (final_status, headers, final_body)
}

fn resolve_custom_rules(
    state: &SharedAdminState,
    custom_rules: &str,
    url: &str,
    method: &str,
) -> (bifrost_core::ResolvedRules, Vec<MatchedRule>) {
    let mut values = load_values(state);
    let parser = RuleParser::with_values(values.clone());
    let parsed_rules = match parser.parse_rules_with_inline_values(custom_rules) {
        Ok((r, inline_values)) => {
            for (key, value) in inline_values {
                values.entry(key).or_insert(value);
            }
            r
        }
        Err(e) => {
            warn!(error = %e, "[REPLAY] Failed to parse custom rules");
            return (bifrost_core::ResolvedRules::default(), vec![]);
        }
    };
    let rules = parsed_rules
        .into_iter()
        .enumerate()
        .map(|(i, r)| r.with_source("custom".to_string(), i + 1))
        .collect::<Vec<_>>();

    if rules.is_empty() {
        return (bifrost_core::ResolvedRules::default(), vec![]);
    }

    let resolver = RulesResolver::new(rules).with_values(values);
    let ctx = RequestContext::from_url(url).with_method(method);
    let resolved = resolver.resolve(&ctx);

    let matched: Vec<MatchedRule> = resolved
        .rules
        .iter()
        .map(|r| MatchedRule {
            pattern: r.rule.pattern.clone(),
            protocol: r.rule.protocol.to_str().to_string(),
            value: r.resolved_value.clone(),
            rule_name: r.rule.file.clone(),
            raw: Some(r.rule.raw.clone()),
            line: r.rule.line,
        })
        .collect();

    (resolved, matched)
}

fn resolve_from_storage(
    state: &SharedAdminState,
    url: &str,
    method: &str,
    selected_rules: Option<&Vec<String>>,
) -> (bifrost_core::ResolvedRules, Vec<MatchedRule>) {
    let rules_storage = &state.rules_storage;
    let mut all_rules: Vec<Rule> = vec![];
    let mut values = load_values(state);

    let rule_files = match rules_storage.load_all() {
        Ok(files) => files,
        Err(e) => {
            warn!(error = %e, "[REPLAY] Failed to load rules");
            return (bifrost_core::ResolvedRules::default(), vec![]);
        }
    };

    for rule_file in rule_files {
        if !rule_file.enabled {
            continue;
        }

        if let Some(selected) = selected_rules {
            if !selected.contains(&rule_file.name) {
                continue;
            }
        }

        let parser = RuleParser::with_values(values.clone());
        if let Ok((parsed, inline_values)) =
            parser.parse_rules_with_inline_values(&rule_file.content)
        {
            for (key, value) in inline_values {
                values.entry(key).or_insert(value);
            }
            let rules_with_source: Vec<Rule> = parsed
                .into_iter()
                .enumerate()
                .map(|(i, r)| r.with_source(rule_file.name.clone(), i + 1))
                .collect();
            all_rules.extend(rules_with_source);
        }
    }

    if all_rules.is_empty() {
        return (bifrost_core::ResolvedRules::default(), vec![]);
    }

    let resolver = RulesResolver::new(all_rules).with_values(values);
    let ctx = RequestContext::from_url(url).with_method(method);
    let resolved = resolver.resolve(&ctx);

    let matched: Vec<MatchedRule> = resolved
        .rules
        .iter()
        .map(|r| MatchedRule {
            pattern: r.rule.pattern.clone(),
            protocol: r.rule.protocol.to_str().to_string(),
            value: r.resolved_value.clone(),
            rule_name: r.rule.file.clone(),
            raw: Some(r.rule.raw.clone()),
            line: r.rule.line,
        })
        .collect();

    (resolved, matched)
}

fn load_values(state: &SharedAdminState) -> HashMap<String, String> {
    if let Some(ref values_storage) = state.values_storage {
        let guard = values_storage.read();
        return guard.as_hashmap();
    }
    HashMap::new()
}

fn record_traffic_for_stream(
    state: &SharedAdminState,
    replay_id: &str,
    applied_request: &AppliedRequest,
    matched_rules: &[MatchedRule],
    is_sse: bool,
) -> String {
    let traffic_id = format!("{}-{}", replay_id, uuid::Uuid::new_v4());
    let timestamp = chrono::Utc::now().timestamp_millis() as u64;

    let uri: Uri = applied_request.url.parse().unwrap_or_default();
    let host = uri.host().unwrap_or("unknown").to_string();
    let path = uri.path().to_string();
    let scheme = uri.scheme_str().unwrap_or("http");
    let (recorded_url, protocol) = if is_sse {
        (applied_request.url.clone(), scheme.to_string())
    } else {
        let url = if scheme == "http" {
            applied_request.url.replacen("http://", "ws://", 1)
        } else if scheme == "https" {
            applied_request.url.replacen("https://", "wss://", 1)
        } else {
            applied_request.url.clone()
        };
        let protocol = if scheme == "https" || scheme == "wss" {
            "wss".to_string()
        } else {
            "ws".to_string()
        };
        (url, protocol)
    };

    let request_content_type = applied_request
        .headers
        .iter()
        .find(|(k, _)| k.to_lowercase() == "content-type")
        .map(|(_, v)| v.clone());

    let request_body_ref = if let Some(ref body) = applied_request.body {
        if let Some(ref body_store) = state.body_store {
            body_store.read().store(&traffic_id, "req", body)
        } else {
            None
        }
    } else {
        None
    };

    let record = TrafficRecord {
        id: traffic_id.clone(),
        sequence: 0,
        timestamp,
        host,
        method: applied_request.method.clone(),
        url: recorded_url,
        path,
        status: if is_sse { 200 } else { 101 },
        protocol,
        content_type: None,
        request_content_type,
        request_size: applied_request.body.as_ref().map(|b| b.len()).unwrap_or(0),
        response_size: 0,
        duration_ms: 0,
        client_ip: "127.0.0.1".to_string(),
        client_app: Some("Bifrost Replay".to_string()),
        client_pid: None,
        client_path: None,
        is_tunnel: false,
        is_websocket: !is_sse,
        is_sse,
        is_h3: false,
        has_rule_hit: !matched_rules.is_empty(),
        is_replay: true,
        frame_count: 0,
        last_frame_id: 0,
        timing: None,
        request_headers: Some(applied_request.headers.clone()),
        original_response_headers: None,
        matched_rules: if matched_rules.is_empty() {
            None
        } else {
            Some(matched_rules.to_vec())
        },
        socket_status: None,
        request_body_ref,
        response_body_ref: None,
        derived_response_body_ref: None,
        raw_request_body_ref: None,
        raw_response_body_ref: None,
        actual_url: None,
        actual_host: None,
        original_request_headers: None,
        response_headers: None,
        error_message: None,
        req_script_results: None,
        res_script_results: None,
        decode_req_script_results: None,
        decode_res_script_results: None,
        devtools_client_req_id: None,
    };

    if let Some(ref traffic_db) = state.traffic_db_store {
        traffic_db.record(record);
    } else if let Some(ref async_writer) = state.async_traffic_writer {
        async_writer.record(record);
    }

    traffic_id
}

fn record_sse_event(
    state: &SharedAdminState,
    replay_id: &str,
    traffic_id: &str,
    event: &StreamEvent,
) {
    let timestamp = chrono::Utc::now().timestamp_millis() as u64;

    // Store event in body store
    if let Some(ref body_store) = state.body_store {
        let event_json = serde_json::to_string(event).unwrap();
        let _ = body_store.read().store(
            traffic_id,
            &format!("sse_{}_{}", event.type_, timestamp),
            event_json.as_bytes(),
        );
    }

    info!(replay_id = %replay_id, traffic_id = %traffic_id, event_type = %event.type_, "Recorded SSE event");
}

#[allow(clippy::too_many_arguments)]
fn record_history(
    state: &SharedAdminState,
    push_manager: &Option<SharedPushManager>,
    request_id: Option<&str>,
    traffic_id: &str,
    method: &str,
    url: &str,
    status: u16,
    duration_ms: u64,
    rule_config: &RuleConfig,
) {
    if let Some(ref replay_db) = state.replay_db_store {
        info!(
            request_id = request_id.unwrap_or("<unbound>"),
            "[REPLAY] Recording history"
        );

        let history = ReplayHistory::new(
            request_id.map(|id| id.to_string()),
            traffic_id.to_string(),
            method.to_string(),
            url.to_string(),
            status,
            duration_ms,
            Some(rule_config.clone()),
        );

        info!(
            history_id = %history.id,
            "[REPLAY] Saving history to database"
        );
        if let Err(e) = replay_db.create_history(&history) {
            warn!(error = %e, "[REPLAY] Failed to record history");
        } else {
            info!(
                history_id = %history.id,
                "[REPLAY] History saved successfully"
            );
            if let Some(pm) = push_manager {
                pm.broadcast_replay_history_updated(
                    "history_created",
                    request_id,
                    Some(&history.id),
                );
            }
        }
    } else {
        warn!("[REPLAY] replay_db_store is None, cannot record history");
    }
}
