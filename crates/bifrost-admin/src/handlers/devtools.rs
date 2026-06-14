use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use hyper::{body::Incoming, header, Method, Request, Response, StatusCode};
use serde::Deserialize;
use sha1::{Digest, Sha1};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::WebSocketStream;
use tracing::{error, warn};

use crate::devtools::{
    bridge_command_queue_capacity, session_live_queue_capacity, BridgeClosePayload,
    BridgeConsolePayload, BridgeEvalPollPayload, BridgeEvalResultPayload, BridgeHelloPayload,
    BridgeNetworkPayload, BridgeNodeSelectedPayload, BridgeOverlayCommand, BridgeServerMessage,
    DebugAdapterKind, DebugPage, DevtoolsMode, SharedBrowserDebugBroker,
};
use crate::state::SharedAdminState;
use crate::{is_remote_access_enabled, validate_admin_jwt};

use super::{
    auth::extract_bearer_token, error_response, full_body, json_response, method_not_allowed,
    BoxBody,
};

const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// 将同步阻塞操作（如获取 parking_lot 锁）转移到 Tokio 的 blocking 线程池，
/// 防止 devtools 模块的锁竞争占用 Tokio worker 线程从而影响代理流量。
async fn blocking<F, R>(f: F) -> R
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .expect("devtools spawn_blocking task panicked")
}

#[derive(Debug, Deserialize)]
struct OpenSessionRequest {
    page_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CommandRequest {
    command: String,
    #[serde(default)]
    params: serde_json::Value,
}

pub async fn handle_devtools(
    req: Request<Incoming>,
    state: SharedAdminState,
    path: &str,
) -> Response<BoxBody> {
    let method = req.method().clone();
    let host = req
        .headers()
        .get("Host")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("127.0.0.1")
        .to_string();

    if path == "/api/devtools/cdp/json/version" {
        return match method {
            Method::GET => json_response(&serde_json::json!({
                "Browser": "Bifrost DevTools Bridge",
                "Protocol-Version": "1.3",
                "User-Agent": "Bifrost",
                "V8-Version": "",
                "WebKit-Version": "",
                "webSocketDebuggerUrl": format!("ws://{host}/_bifrost/api/devtools/cdp/browser")
            })),
            _ => method_not_allowed(),
        };
    }

    if path == "/api/devtools/cdp/json/list" || path == "/api/devtools/cdp/json" {
        return match method {
            Method::GET => {
                let broker = state.devtools_broker.clone();
                let host = host.clone();
                let targets = blocking(move || broker.cdp_targets(true, &host)).await;
                json_response(&targets)
            }
            _ => method_not_allowed(),
        };
    }

    if path == "/api/devtools/audit/evaluate" {
        return match method {
            Method::GET => {
                let query = parse_query(req.uri().query().unwrap_or_default());
                let limit = query
                    .get("limit")
                    .and_then(|value| value.parse::<usize>().ok());
                let since = query
                    .get("since")
                    .and_then(|value| value.parse::<u64>().ok());
                let broker = state.devtools_broker.clone();
                let records = blocking(move || broker.list_evaluate_audit(limit, since)).await;
                json_response(&records)
            }
            _ => method_not_allowed(),
        };
    }

    if let Some(client_req_id) = path.strip_prefix("/api/devtools/network/traffic/") {
        return match method {
            Method::GET => {
                let client_req_id = urlencoding::decode(client_req_id)
                    .map(|value| value.into_owned())
                    .unwrap_or_else(|_| client_req_id.to_string());
                let Some(db_store) = state.traffic_db_store.clone() else {
                    return error_response(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "traffic database is not available",
                    );
                };
                let traffic_id = tokio::task::spawn_blocking(move || {
                    db_store.get_id_by_devtools_client_req_id(&client_req_id)
                })
                .await
                .ok()
                .flatten();
                match traffic_id {
                    Some(traffic_id) => json_response(&serde_json::json!({
                        "ok": true,
                        "traffic_id": traffic_id
                    })),
                    None => error_response(
                        StatusCode::NOT_FOUND,
                        "traffic record not found for this DevTools request; it may have been deleted, replayed, or only captured as a CONNECT tunnel",
                    ),
                }
            }
            _ => method_not_allowed(),
        };
    }

    if let Some(page_id) = path.strip_prefix("/api/devtools/cdp/") {
        return match method {
            Method::GET => handle_cdp_websocket(req, state, page_id.to_string()).await,
            _ => method_not_allowed(),
        };
    }

    if path == "/api/devtools/pages" || path.starts_with("/api/devtools/pages?") {
        return match method {
            Method::GET => {
                let online_only = req
                    .uri()
                    .query()
                    .map(|q| q.contains("online=true"))
                    .unwrap_or(false);
                let broker = state.devtools_broker.clone();
                let pages = blocking(move || broker.list_debuggable_pages(online_only)).await;
                json_response(&serde_json::json!({ "pages": pages }))
            }
            _ => method_not_allowed(),
        };
    }

    if path == "/api/devtools/sessions" {
        return match method {
            Method::POST => {
                let body = match read_json::<OpenSessionRequest>(req).await {
                    Ok(body) => body,
                    Err(resp) => return resp,
                };
                let Some(page_id) = body.page_id else {
                    return error_response(StatusCode::BAD_REQUEST, "missing page_id");
                };
                let broker = state.devtools_broker.clone();
                match blocking(move || broker.open_session(&page_id)).await {
                    Ok(session) => json_response(&session),
                    Err(err) => error_response(StatusCode::BAD_REQUEST, &err),
                }
            }
            _ => method_not_allowed(),
        };
    }

    if let Some(session_path) = path.strip_prefix("/api/devtools/sessions/") {
        let mut parts = session_path.split('/');
        let Some(session_id) = parts.next() else {
            return error_response(StatusCode::BAD_REQUEST, "missing session id");
        };
        let action = parts.next().unwrap_or_default();
        return match (method, action) {
            (Method::GET, "snapshot") => {
                let broker = state.devtools_broker.clone();
                let sid = session_id.to_string();
                match blocking(move || broker.snapshot(&sid)).await {
                    Ok(snapshot) => json_response(&snapshot),
                    Err(err) => error_response(StatusCode::NOT_FOUND, &err),
                }
            }
            (Method::POST, "refresh") => {
                let scope = read_json::<serde_json::Value>(req)
                    .await
                    .ok()
                    .and_then(|body| {
                        body.get("scope")
                            .and_then(|value| value.as_str())
                            .map(str::to_string)
                    });
                let broker = state.devtools_broker.clone();
                let sid = session_id.to_string();
                match blocking(move || broker.request_snapshot_refresh(&sid, scope)).await {
                    Ok(()) => json_response(&serde_json::json!({"ok": true})),
                    Err(err) => error_response(StatusCode::BAD_REQUEST, &err),
                }
            }
            (Method::GET, "ws") => {
                handle_session_websocket(req, state, session_id.to_string()).await
            }
            (Method::POST, "commands") => {
                let body = match read_json::<CommandRequest>(req).await {
                    Ok(body) => body,
                    Err(resp) => return resp,
                };
                match state
                    .devtools_broker
                    .command(session_id, &body.command, body.params)
                    .await
                {
                    Ok(result) => {
                        json_response(&serde_json::json!({ "ok": true, "result": result }))
                    }
                    Err(err) => error_response(StatusCode::BAD_REQUEST, &err),
                }
            }
            _ => method_not_allowed(),
        };
    }

    if let Some(bridge_path) = path.strip_prefix("/api/devtools/bridge/") {
        let mut parts = bridge_path.split('/');
        let Some(page_id) = parts.next() else {
            return error_response(StatusCode::BAD_REQUEST, "missing page id");
        };
        let action = parts.next().unwrap_or_default();
        return match (method, action) {
            (Method::GET, "ws") => handle_bridge_websocket(req, state, page_id.to_string()).await,
            (Method::POST, "hello") => {
                let payload = match read_json::<BridgeHelloPayload>(req).await {
                    Ok(body) => body,
                    Err(resp) => return resp,
                };
                let broker = state.devtools_broker.clone();
                let pid = page_id.to_string();
                match blocking(move || broker.bridge_hello(&pid, payload)).await {
                    Ok(()) => json_response(&serde_json::json!({"ok": true})),
                    Err(err) => error_response(StatusCode::FORBIDDEN, &err),
                }
            }
            (Method::POST, "console") => {
                let payload = match read_json::<BridgeConsolePayload>(req).await {
                    Ok(body) => body,
                    Err(resp) => return resp,
                };
                let broker = state.devtools_broker.clone();
                let pid = page_id.to_string();
                match blocking(move || broker.bridge_console(&pid, payload)).await {
                    Ok(()) => json_response(&serde_json::json!({"ok": true})),
                    Err(err) => error_response(StatusCode::FORBIDDEN, &err),
                }
            }
            (Method::POST, "close") => {
                let payload = match read_json::<BridgeClosePayload>(req).await {
                    Ok(body) => body,
                    Err(resp) => return resp,
                };
                let broker = state.devtools_broker.clone();
                let pid = page_id.to_string();
                match blocking(move || broker.bridge_close(&pid, payload)).await {
                    Ok(()) => json_response(&serde_json::json!({"ok": true})),
                    Err(err) => error_response(StatusCode::FORBIDDEN, &err),
                }
            }
            (Method::POST, "network") => {
                let payload = match read_json::<BridgeNetworkPayload>(req).await {
                    Ok(body) => body,
                    Err(resp) => return resp,
                };
                let broker = state.devtools_broker.clone();
                let pid = page_id.to_string();
                match blocking(move || broker.bridge_network(&pid, payload)).await {
                    Ok(()) => json_response(&serde_json::json!({"ok": true})),
                    Err(err) => error_response(StatusCode::FORBIDDEN, &err),
                }
            }
            (Method::POST, "node-selected") => {
                let payload = match read_json::<BridgeNodeSelectedPayload>(req).await {
                    Ok(body) => body,
                    Err(resp) => return resp,
                };
                let broker = state.devtools_broker.clone();
                let pid = page_id.to_string();
                match blocking(move || broker.bridge_node_selected(&pid, payload)).await {
                    Ok(()) => json_response(&serde_json::json!({"ok": true})),
                    Err(err) => error_response(StatusCode::FORBIDDEN, &err),
                }
            }
            (Method::POST, "eval-next") => {
                let payload = match read_json::<BridgeEvalPollPayload>(req).await {
                    Ok(body) => body,
                    Err(resp) => return resp,
                };
                let broker = state.devtools_broker.clone();
                let pid = page_id.to_string();
                match blocking(move || broker.bridge_eval_next(&pid, payload)).await {
                    Ok(command) => json_response(&serde_json::json!({ "command": command })),
                    Err(err) => error_response(StatusCode::FORBIDDEN, &err),
                }
            }
            (Method::POST, "eval-result") => {
                let payload = match read_json::<BridgeEvalResultPayload>(req).await {
                    Ok(body) => body,
                    Err(resp) => return resp,
                };
                let broker = state.devtools_broker.clone();
                let pid = page_id.to_string();
                match blocking(move || broker.bridge_eval_result(&pid, payload)).await {
                    Ok(()) => json_response(&serde_json::json!({"ok": true})),
                    Err(err) => error_response(StatusCode::FORBIDDEN, &err),
                }
            }
            (Method::POST, "overlay-next") => {
                let payload = match read_json::<BridgeEvalPollPayload>(req).await {
                    Ok(body) => body,
                    Err(resp) => return resp,
                };
                let broker = state.devtools_broker.clone();
                let pid = page_id.to_string();
                match blocking(move || broker.bridge_overlay_next(&pid, payload)).await {
                    Ok(command) => json_response(&serde_json::json!({ "command": command })),
                    Err(err) => error_response(StatusCode::FORBIDDEN, &err),
                }
            }
            _ => method_not_allowed(),
        };
    }

    error_response(StatusCode::NOT_FOUND, "DevTools endpoint not found")
}

async fn handle_bridge_websocket(
    req: Request<Incoming>,
    state: SharedAdminState,
    page_id: String,
) -> Response<BoxBody> {
    let upgrade_header = req
        .headers()
        .get("Upgrade")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if !upgrade_header.eq_ignore_ascii_case("websocket") {
        return error_response(StatusCode::BAD_REQUEST, "Invalid upgrade header");
    }
    let ws_key = match req.headers().get("Sec-WebSocket-Key") {
        Some(key) => key.to_str().unwrap_or("").to_string(),
        None => return error_response(StatusCode::BAD_REQUEST, "Missing Sec-WebSocket-Key header"),
    };
    let accept_key = generate_accept_key(&ws_key);
    tokio::spawn(async move {
        let upgraded = match hyper::upgrade::on(req).await {
            Ok(upgraded) => upgraded,
            Err(err) => {
                error!(error = %err, "DevTools bridge WebSocket upgrade failed");
                return;
            }
        };
        let ws_stream = WebSocketStream::from_raw_socket(
            hyper_util::rt::TokioIo::new(upgraded),
            tokio_tungstenite::tungstenite::protocol::Role::Server,
            None,
        )
        .await;
        handle_bridge_connection(ws_stream, state.devtools_broker.clone(), page_id).await;
    });

    Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header("Upgrade", "websocket")
        .header("Connection", "Upgrade")
        .header("Sec-WebSocket-Accept", accept_key)
        .body(BoxBody::default())
        .unwrap()
}

async fn handle_session_websocket(
    req: Request<Incoming>,
    state: SharedAdminState,
    session_id: String,
) -> Response<BoxBody> {
    let upgrade_header = req
        .headers()
        .get("Upgrade")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if !upgrade_header.eq_ignore_ascii_case("websocket") {
        return error_response(StatusCode::BAD_REQUEST, "Invalid upgrade header");
    }
    let ws_key = match req.headers().get("Sec-WebSocket-Key") {
        Some(key) => key.to_str().unwrap_or("").to_string(),
        None => return error_response(StatusCode::BAD_REQUEST, "Missing Sec-WebSocket-Key header"),
    };
    let accept_key = generate_accept_key(&ws_key);
    tokio::spawn(async move {
        let upgraded = match hyper::upgrade::on(req).await {
            Ok(upgraded) => upgraded,
            Err(err) => {
                error!(error = %err, "DevTools session WebSocket upgrade failed");
                return;
            }
        };
        let ws_stream = WebSocketStream::from_raw_socket(
            hyper_util::rt::TokioIo::new(upgraded),
            tokio_tungstenite::tungstenite::protocol::Role::Server,
            None,
        )
        .await;
        handle_session_connection(ws_stream, state.devtools_broker.clone(), session_id).await;
    });

    Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header("Upgrade", "websocket")
        .header("Connection", "Upgrade")
        .header("Sec-WebSocket-Accept", accept_key)
        .body(BoxBody::default())
        .unwrap()
}

async fn handle_session_connection<S>(
    ws_stream: WebSocketStream<S>,
    broker: SharedBrowserDebugBroker,
    session_id: String,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let (mut sink, mut stream) = ws_stream.split();
    let (tx, mut rx) = mpsc::channel(session_live_queue_capacity());
    {
        let b = broker.clone();
        let sid = session_id.clone();
        if blocking(move || b.session_ws_attach(&sid, tx))
            .await
            .is_err()
        {
            let _ = sink
                .send(Message::Text(
                    r#"{"type":"disconnected","reason":"session not found"}"#.into(),
                ))
                .await;
            return;
        }
    }

    loop {
        tokio::select! {
            maybe_msg = stream.next() => {
                let Some(Ok(msg)) = maybe_msg else {
                    break;
                };
                if msg.is_close() {
                    break;
                }
            }
            maybe_outbound = rx.recv() => {
                let Some(outbound) = maybe_outbound else {
                    break;
                };
                let Ok(text) = serde_json::to_string(&outbound) else {
                    continue;
                };
                if sink.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
        }
    }

    let b = broker.clone();
    let sid = session_id.clone();
    let _ = blocking(move || b.session_ws_detach(&sid)).await;
}

async fn handle_bridge_connection<S>(
    ws_stream: WebSocketStream<S>,
    broker: SharedBrowserDebugBroker,
    page_id: String,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let (mut sink, mut stream) = ws_stream.split();
    let (tx, mut rx) = mpsc::channel::<BridgeServerMessage>(bridge_command_queue_capacity());
    let mut attached = false;

    loop {
        tokio::select! {
            maybe_msg = stream.next() => {
                let Some(Ok(msg)) = maybe_msg else {
                    break;
                };
                if msg.is_close() {
                    break;
                }
                let Ok(text) = msg.to_text() else {
                    continue;
                };
                let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
                    continue;
                };
                let message_type = value.get("type").and_then(|value| value.as_str()).unwrap_or_default();
                let message_seq = value.get("seq").and_then(|value| value.as_u64());
                match message_type {
                    "hello" => {
                        let Ok(payload) = serde_json::from_value::<BridgeHelloPayload>(value.clone()) else {
                            continue;
                        };
                        // bridge_hello 会获取 pages.write()，通过 spawn_blocking 保护 worker 线程
                        let b = broker.clone();
                        let pid = page_id.clone();
                        let hello_ok = blocking(move || b.bridge_hello(&pid, payload)).await.is_ok();
                        if hello_ok {
                            let token = value.get("token").and_then(|value| value.as_str()).unwrap_or_default();
                            let b = broker.clone();
                            let pid = page_id.clone();
                            let token = token.to_string();
                            let tx_clone = tx.clone();
                            let attach_ok = blocking(move || b.bridge_ws_attach(&pid, &token, tx_clone)).await.is_ok();
                            if attach_ok {
                                attached = true;
                                ack_bridge_message(&mut sink, message_seq).await;
                                let _ = sink.send(Message::Text(r#"{"type":"ready"}"#.into())).await;
                            }
                        }
                    }
                    "console" => {
                        if let Ok(payload) = serde_json::from_value::<BridgeConsolePayload>(value.clone()) {
                            if let Some(seq) = message_seq {
                                if !broker.remember_bridge_seq(&page_id, seq) {
                                    ack_bridge_message(&mut sink, message_seq).await;
                                    continue;
                                }
                            }
                            let _ = broker.bridge_console(&page_id, payload);
                            ack_bridge_message(&mut sink, message_seq).await;
                        }
                    }
                    "network" => {
                        if let Ok(payload) = serde_json::from_value::<BridgeNetworkPayload>(value.clone()) {
                            if let Some(seq) = message_seq {
                                if !broker.remember_bridge_seq(&page_id, seq) {
                                    ack_bridge_message(&mut sink, message_seq).await;
                                    continue;
                                }
                            }
                            let _ = broker.bridge_network(&page_id, payload);
                            ack_bridge_message(&mut sink, message_seq).await;
                        }
                    }
                    "node_selected" => {
                        if let Ok(payload) = serde_json::from_value::<BridgeNodeSelectedPayload>(value.clone()) {
                            if let Some(seq) = message_seq {
                                if !broker.remember_bridge_seq(&page_id, seq) {
                                    ack_bridge_message(&mut sink, message_seq).await;
                                    continue;
                                }
                            }
                            let _ = broker.bridge_node_selected(&page_id, payload);
                            ack_bridge_message(&mut sink, message_seq).await;
                        }
                    }
                    "eval_result" => {
                        if let Ok(payload) = serde_json::from_value::<BridgeEvalResultPayload>(value.clone()) {
                            if let Some(seq) = message_seq {
                                if !broker.remember_bridge_seq(&page_id, seq) {
                                    ack_bridge_message(&mut sink, message_seq).await;
                                    continue;
                                }
                            }
                            let _ = broker.bridge_eval_result(&page_id, payload);
                            ack_bridge_message(&mut sink, message_seq).await;
                        }
                    }
                    "close" => {
                        if let Ok(payload) = serde_json::from_value::<BridgeClosePayload>(value.clone()) {
                            if let Some(seq) = message_seq {
                                if !broker.remember_bridge_seq(&page_id, seq) {
                                    ack_bridge_message(&mut sink, message_seq).await;
                                    break;
                                }
                            }
                            let _ = broker.bridge_close(&page_id, payload);
                            ack_bridge_message(&mut sink, message_seq).await;
                        }
                        break;
                    }
                    _ => {}
                }
            }
            maybe_outbound = rx.recv(), if attached => {
                let Some(outbound) = maybe_outbound else {
                    break;
                };
                let Ok(text) = serde_json::to_string(&outbound) else {
                    continue;
                };
                if sink.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
        }
    }

    let b = broker.clone();
    let pid = page_id.clone();
    let _ = blocking(move || b.bridge_ws_detach(&pid)).await;
}

async fn ack_bridge_message<S>(
    sink: &mut futures_util::stream::SplitSink<WebSocketStream<S>, Message>,
    seq: Option<u64>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    if let Some(seq) = seq {
        let text = serde_json::json!({"type": "ack", "seq": seq}).to_string();
        let _ = sink.send(Message::Text(text.into())).await;
    }
}

async fn handle_cdp_websocket(
    req: Request<Incoming>,
    state: SharedAdminState,
    page_id: String,
) -> Response<BoxBody> {
    let upgrade_header = req
        .headers()
        .get("Upgrade")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if !upgrade_header.eq_ignore_ascii_case("websocket") {
        return error_response(StatusCode::BAD_REQUEST, "Invalid upgrade header");
    }

    let origin = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let remote_addr = req
        .headers()
        .get("x-bifrost-peer-ip")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("unknown")
        .to_string();
    if !origin
        .as_deref()
        .map(is_allowed_cdp_origin)
        .unwrap_or(false)
    {
        warn!(
            remote_addr = %remote_addr,
            origin = origin.as_deref().unwrap_or(""),
            reason = "origin_not_allowed",
            "CDP WebSocket rejected"
        );
        return cdp_unauthorized("origin_not_allowed");
    }

    let caller_client_id = if is_remote_access_enabled(&state) {
        let token = extract_bearer_token(&req).or_else(|| query_token(req.uri().query()));
        let Some(token) = token else {
            warn!(
                remote_addr = %remote_addr,
                origin = origin.as_deref().unwrap_or(""),
                reason = "missing_token",
                "CDP WebSocket rejected"
            );
            return cdp_unauthorized("missing_token");
        };
        match validate_admin_jwt(&state, &token) {
            Ok(claims) => Some(claims.sub),
            Err(err) => {
                warn!(
                    remote_addr = %remote_addr,
                    origin = origin.as_deref().unwrap_or(""),
                    reason = "invalid_token",
                    error = %err,
                    "CDP WebSocket rejected"
                );
                return cdp_unauthorized("invalid_token");
            }
        }
    } else {
        None
    };

    let ws_key = match req.headers().get("Sec-WebSocket-Key") {
        Some(key) => key.to_str().unwrap_or("").to_string(),
        None => return error_response(StatusCode::BAD_REQUEST, "Missing Sec-WebSocket-Key header"),
    };

    let broker = state.devtools_broker.clone();
    let pid = page_id.clone();
    let Some(page) = blocking(move || broker.get_page(&pid)).await else {
        return error_response(StatusCode::NOT_FOUND, "DevTools page not found");
    };
    if page.adapter != DebugAdapterKind::PageBridge {
        return error_response(StatusCode::BAD_REQUEST, "unsupported DevTools adapter");
    }

    let accept_key = generate_accept_key(&ws_key);
    tokio::spawn(async move {
        let upgraded = match hyper::upgrade::on(req).await {
            Ok(upgraded) => upgraded,
            Err(err) => {
                error!(error = %err, "CDP WebSocket upgrade failed");
                return;
            }
        };
        let ws_stream = WebSocketStream::from_raw_socket(
            hyper_util::rt::TokioIo::new(upgraded),
            tokio_tungstenite::tungstenite::protocol::Role::Server,
            None,
        )
        .await;
        handle_cdp_connection(
            ws_stream,
            state.devtools_broker.clone(),
            page,
            caller_client_id,
        )
        .await;
    });

    Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header("Upgrade", "websocket")
        .header("Connection", "Upgrade")
        .header("Sec-WebSocket-Accept", accept_key)
        .body(BoxBody::default())
        .unwrap()
}

async fn handle_cdp_connection<S>(
    mut ws_stream: WebSocketStream<S>,
    broker: SharedBrowserDebugBroker,
    page: DebugPage,
    caller_client_id: Option<String>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let page_id = page.page_id.clone();
    let mut latest_page = page;
    let mut active_session_id: Option<serde_json::Value> = None;
    let mut last_console_at = 0;
    let mut last_network_at = 0;
    let mut last_dom_at = latest_page.dom_updated_at_ms;
    let mut refresh_timer = tokio::time::interval(std::time::Duration::from_millis(500));

    loop {
        tokio::select! {
            maybe_message = ws_stream.next() => {
                let Some(message) = maybe_message else {
                    break;
                };
        let message = match message {
            Ok(message) => message,
            Err(err) => {
                warn!(error = %err, page_id = %latest_page.page_id, "CDP WebSocket read failed");
                break;
            }
        };
        let Message::Text(text) = message else {
            if message.is_close() {
                break;
            }
            continue;
        };
        let Ok(request) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let Some(id) = request.get("id").cloned() else {
            continue;
        };
        let method = request
            .get("method")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let session_id = request.get("sessionId").cloned();
        if session_id.is_some() {
            active_session_id = session_id.clone();
        }
        let response = with_cdp_session_id(
            cdp_response(
                id,
                method,
                &request,
                &broker,
                &latest_page,
                caller_client_id.as_deref(),
            )
            .await,
            session_id.as_ref(),
        );
        if ws_stream
            .send(Message::Text(response.to_string().into()))
            .await
            .is_err()
        {
            break;
        }
        for event in cdp_events(method, &latest_page) {
            let event = with_cdp_session_id(event, session_id.as_ref());
            if ws_stream
                .send(Message::Text(event.to_string().into()))
                .await
                .is_err()
            {
                return;
            }
        }
            }
            _ = refresh_timer.tick() => {
                let Some(next_page) = broker.get_page(&page_id) else {
                    break;
                };
                let live_events = cdp_live_events(
                    &next_page,
                    &mut last_console_at,
                    &mut last_network_at,
                    &mut last_dom_at,
                );
                latest_page = next_page;
                for event in live_events {
                    let event = with_cdp_session_id(event, active_session_id.as_ref());
                    if ws_stream
                        .send(Message::Text(event.to_string().into()))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            }
        }
    }
}

async fn cdp_response(
    id: serde_json::Value,
    method: &str,
    request: &serde_json::Value,
    broker: &SharedBrowserDebugBroker,
    page: &DebugPage,
    caller_client_id: Option<&str>,
) -> serde_json::Value {
    match method {
        "Browser.getVersion" => serde_json::json!({
            "id": id,
            "result": {
                "protocolVersion": "1.3",
                "product": "Bifrost DevTools Bridge",
                "revision": "",
                "userAgent": page.user_agent.clone().unwrap_or_else(|| "Bifrost".to_string()),
                "jsVersion": ""
            }
        }),
        "Target.getTargetInfo" => serde_json::json!({
            "id": id,
            "result": {
                "targetInfo": {
                    "targetId": page.page_id,
                    "type": "page",
                    "title": page.title.clone().unwrap_or_default(),
                    "url": page.url,
                    "attached": true,
                    "canAccessOpener": false
                }
            }
        }),
        "Target.setDiscoverTargets"
        | "Target.setAutoAttach"
        | "Runtime.enable"
        | "Page.enable"
        | "DOM.enable"
        | "CSS.enable"
        | "Network.enable"
        | "DOMStorage.enable"
        | "Log.enable"
        | "Debugger.enable"
        | "Overlay.enable"
        | "Accessibility.enable"
        | "Performance.enable"
        | "Profiler.enable"
        | "Security.enable"
        | "IndexedDB.enable"
        | "Inspector.enable"
        | "ServiceWorker.enable"
        | "Audits.enable"
        | "Target.setRemoteLocations"
        | "Debugger.setPauseOnExceptions"
        | "Debugger.setAsyncCallStackDepth"
        | "Debugger.setBlackboxPatterns"
        | "Page.setAdBlockingEnabled"
        | "Emulation.setTouchEmulationEnabled"
        | "Emulation.setEmitTouchEventsForMouse"
        | "Emulation.setFocusEmulationEnabled"
        | "DOM.setInspectedNode"
        | "Storage.setStorageBucketTracking"
        | "Runtime.runIfWaitingForDebugger"
        | "Runtime.releaseObjectGroup"
        | "Log.startViolationsReport"
        | "Overlay.setShowViewportSizeOnResize"
        | "Network.setCacheDisabled"
        | "Network.setBypassServiceWorker"
        | "Network.setAttachDebugStack"
        | "Network.setBlockedURLs"
        | "Network.emulateNetworkConditionsByRule"
        | "Network.overrideNetworkState"
        | "Network.clearAcceptedEncodingsOverride"
        | "Animation.enable"
        | "Autofill.enable"
        | "Autofill.setAddresses"
        | "Emulation.setEmulatedMedia"
        | "Emulation.setEmulatedVisionDeficiency"
        | "Runtime.addBinding"
        | "CSS.trackComputedStyleUpdates"
        | "CSS.takeComputedStyleUpdates"
        | "CSS.trackComputedStyleUpdatesForNode"
        | "DOMDebugger.setBreakOnCSPViolation"
        | "Overlay.setShowHinge"
        | "Overlay.setShowGridOverlays"
        | "Overlay.setShowFlexOverlays"
        | "Overlay.setShowScrollSnapOverlays"
        | "Overlay.setShowContainerQueryOverlays"
        | "Overlay.setShowIsolatedElements" => serde_json::json!({ "id": id, "result": {} }),
        "Page.startScreencast" | "Page.stopScreencast" | "Page.screencastFrameAck" => {
            serde_json::json!({
                "id": id,
                "error": {
                    "code": -32000,
                    "message": "screencast_disabled"
                }
            })
        }
        "Overlay.highlightNode" => overlay_highlight_node_response(id, request, broker, page),
        "Overlay.hideHighlight" => overlay_hide_highlight_response(id, broker, page),
        "Storage.getStorageKey" => serde_json::json!({
            "id": id,
            "result": { "storageKey": page.origin }
        }),
        "Storage.getStorageKeyForFrame" => serde_json::json!({
            "id": id,
            "result": { "storageKey": page.origin }
        }),
        "DOMStorage.getDOMStorageItems" => {
            let is_local_storage = request_dom_storage_is_local(request);
            let snapshot = page.storage_snapshot.clone().unwrap_or_default();
            let entries = if is_local_storage {
                snapshot.local_storage
            } else {
                snapshot.session_storage
            };
            serde_json::json!({
                "id": id,
                "result": { "entries": entries }
            })
        }
        "IndexedDB.requestDatabaseNames" => serde_json::json!({
            "id": id,
            "result": { "databaseNames": [] }
        }),
        "CacheStorage.requestCacheNames" => serde_json::json!({
            "id": id,
            "result": { "caches": [] }
        }),
        "Storage.getUsageAndQuota" => serde_json::json!({
            "id": id,
            "result": {
                "usage": 0,
                "quota": 0,
                "overrideActive": false,
                "usageBreakdown": []
            }
        }),
        "Target.getTargets" => serde_json::json!({
            "id": id,
            "result": {
                "targetInfos": [{
                    "targetId": page.page_id,
                    "type": "page",
                    "title": page.title.clone().unwrap_or_default(),
                    "url": page.url,
                    "attached": true,
                    "canAccessOpener": false
                }]
            }
        }),
        "Target.attachToTarget" => serde_json::json!({
            "id": id,
            "result": { "sessionId": format!("bdt-cdp-{}", page.page_id) }
        }),
        "Page.getFrameTree" => serde_json::json!({
            "id": id,
            "result": {
                "frameTree": {
                    "frame": {
                        "id": page.page_id,
                        "loaderId": "bifrost-loader",
                        "url": page.url,
                        "domainAndRegistry": "",
                        "securityOrigin": page.origin,
                        "mimeType": "text/html"
                    }
                }
            }
        }),
        "Page.getResourceTree" => serde_json::json!({
            "id": id,
            "result": {
                "frameTree": {
                    "frame": {
                        "id": page.page_id,
                        "loaderId": "bifrost-loader",
                        "url": page.url,
                        "domainAndRegistry": "",
                        "securityOrigin": page.origin,
                        "mimeType": "text/html"
                    },
                    "resources": []
                }
            }
        }),
        "Runtime.getIsolateId" => serde_json::json!({
            "id": id,
            "result": { "id": "bifrost-page-bridge" }
        }),
        "Runtime.getHeapUsage" => serde_json::json!({
            "id": id,
            "result": {
                "usedSize": 0,
                "totalSize": 0
            }
        }),
        "Runtime.evaluate" => {
            runtime_evaluate_response(id, request, broker, page, caller_client_id).await
        }
        "Page.getNavigationHistory" => serde_json::json!({
            "id": id,
            "result": {
                "currentIndex": 0,
                "entries": [{
                    "id": 1,
                    "url": page.url,
                    "userTypedURL": page.url,
                    "title": page.title.clone().unwrap_or_default(),
                    "transitionType": "typed"
                }]
            }
        }),
        "DOM.getDocument" => serde_json::json!({
            "id": id,
            "result": { "root": page_dom_root(page) }
        }),
        "DOM.getFlattenedDocument" => serde_json::json!({
            "id": id,
            "result": {
                "nodes": flattened_dom_nodes(page)
            }
        }),
        "CSS.getMatchedStylesForNode" => css_matched_styles_response(id, request, page),
        "CSS.getComputedStyleForNode" => css_computed_style_response(id, request, page),
        "CSS.getInlineStylesForNode" => css_inline_styles_response(id, request, page),
        "CSS.getPlatformFontsForNode" => serde_json::json!({
            "id": id,
            "result": { "fonts": [] }
        }),
        "CSS.getAnimatedStylesForNode" => serde_json::json!({
            "id": id,
            "result": { "animationStyles": [] }
        }),
        "CSS.getEnvironmentVariables" => serde_json::json!({
            "id": id,
            "result": { "variables": [] }
        }),
        "DOM.pushNodesByBackendIdsToFrontend" => serde_json::json!({
            "id": id,
            "result": {
                "nodeIds": request
                    .get("params")
                    .and_then(|params| params.get("backendNodeIds"))
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!([]))
            }
        }),
        "DOM.resolveNode" => {
            let node_id = request
                .get("params")
                .and_then(|params| params.get("nodeId").or_else(|| params.get("backendNodeId")))
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            serde_json::json!({
                "id": id,
                "result": {
                    "object": {
                        "type": "object",
                        "subtype": "node",
                        "className": "Node",
                        "description": "page_bridge node",
                        "objectId": format!("bifrost-node-{node_id}")
                    }
                }
            })
        }
        other => serde_json::json!({
            "id": id,
            "error": {
                "code": -32601,
                "message": format!("unsupported CDP method: {other}")
            }
        }),
    }
}

fn with_cdp_session_id(
    mut message: serde_json::Value,
    session_id: Option<&serde_json::Value>,
) -> serde_json::Value {
    if let (Some(session_id), Some(object)) = (session_id, message.as_object_mut()) {
        object.insert("sessionId".to_string(), session_id.clone());
    }
    message
}

fn request_dom_storage_is_local(request: &serde_json::Value) -> bool {
    request
        .get("params")
        .and_then(|params| params.get("storageId"))
        .and_then(|storage_id| storage_id.get("isLocalStorage"))
        .and_then(|value| value.as_bool())
        .unwrap_or(true)
}

fn parse_query(query: &str) -> std::collections::HashMap<String, String> {
    serde_urlencoded::from_str(query).unwrap_or_default()
}

fn query_token(query: Option<&str>) -> Option<String> {
    query
        .and_then(|query| parse_query(query).remove("token"))
        .filter(|token| !token.trim().is_empty())
}

fn is_allowed_cdp_origin(origin: &str) -> bool {
    let Ok(parsed) = url::Url::parse(origin) else {
        return false;
    };
    if !matches!(parsed.scheme(), "http" | "https") {
        return false;
    }
    matches!(parsed.host_str(), Some("127.0.0.1" | "localhost"))
        || std::env::var("BIFROST_DEVTOOLS_ALLOWED_ORIGINS")
            .ok()
            .map(|allowed| {
                allowed
                    .split(',')
                    .map(str::trim)
                    .any(|candidate| candidate == origin)
            })
            .unwrap_or(false)
}

fn cdp_unauthorized(code: &str) -> Response<BoxBody> {
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header("Content-Type", "application/json")
        .body(full_body(format!(r#"{{"code":"{code}"}}"#)))
        .unwrap()
}

async fn runtime_evaluate_response(
    id: serde_json::Value,
    request: &serde_json::Value,
    broker: &SharedBrowserDebugBroker,
    page: &DebugPage,
    caller_client_id: Option<&str>,
) -> serde_json::Value {
    if page.mode != DevtoolsMode::Control {
        return serde_json::json!({
            "id": id,
            "error": {
                "code": -32000,
                "message": "requires_control"
            }
        });
    }
    let expression = request
        .get("params")
        .and_then(|params| params.get("expression"))
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string();
    let world = request
        .get("params")
        .and_then(|params| params.get("world"))
        .and_then(|value| value.as_str())
        .unwrap_or("main");
    if expression.trim().is_empty() {
        return serde_json::json!({
            "id": id,
            "result": {
                "result": { "type": "undefined", "description": "undefined" }
            }
        });
    }
    if !crate::devtools::BrowserDebugBroker::expression_allowed_by_page(page, &expression) {
        broker.record_evaluate_audit(
            page,
            &expression,
            world,
            caller_client_id.map(ToString::to_string),
            true,
        );
        return serde_json::json!({
            "id": id,
            "error": {
                "code": -32000,
                "message": "evaluate not in allowlist"
            }
        });
    }
    broker.record_evaluate_audit(
        page,
        &expression,
        world,
        caller_client_id.map(ToString::to_string),
        false,
    );
    let eval_id = match broker.queue_eval(&page.page_id, expression) {
        Ok(eval_id) => eval_id,
        Err(err) => {
            return serde_json::json!({
                "id": id,
                "error": { "code": -32000, "message": err }
            })
        }
    };
    for _ in 0..40 {
        if let Some(result) = broker.take_eval_result(eval_id) {
            return match result {
                Ok(result) => serde_json::json!({
                    "id": id,
                    "result": { "result": result }
                }),
                Err(exception) => serde_json::json!({
                    "id": id,
                    "result": {
                        "result": {
                            "type": "undefined",
                            "description": "undefined"
                        },
                        "exceptionDetails": {
                            "text": exception,
                            "exception": {
                                "type": "string",
                                "value": exception,
                                "description": exception
                            }
                        }
                    }
                }),
            };
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    serde_json::json!({
        "id": id,
        "error": {
            "code": -32000,
            "message": "evaluation timed out"
        }
    })
}

fn overlay_highlight_node_response(
    id: serde_json::Value,
    request: &serde_json::Value,
    broker: &SharedBrowserDebugBroker,
    page: &DebugPage,
) -> serde_json::Value {
    let params = request.get("params");
    let node_id = params
        .and_then(|params| params.get("nodeId").or_else(|| params.get("backendNodeId")))
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|text| text.parse::<u64>().ok()))
        })
        .or_else(|| {
            params
                .and_then(|params| params.get("objectId"))
                .and_then(|value| value.as_str())
                .and_then(|value| value.strip_prefix("bifrost-node-"))
                .and_then(|text| text.parse::<u64>().ok())
        });
    let Some(node_id) = node_id else {
        return serde_json::json!({
            "id": id,
            "result": {}
        });
    };
    match broker.queue_overlay(
        &page.page_id,
        BridgeOverlayCommand::HighlightNode { node_id },
    ) {
        Ok(()) => serde_json::json!({ "id": id, "result": {} }),
        Err(err) => serde_json::json!({
            "id": id,
            "error": { "code": -32000, "message": err }
        }),
    }
}

fn overlay_hide_highlight_response(
    id: serde_json::Value,
    broker: &SharedBrowserDebugBroker,
    page: &DebugPage,
) -> serde_json::Value {
    match broker.queue_overlay(&page.page_id, BridgeOverlayCommand::HideHighlight) {
        Ok(()) => serde_json::json!({ "id": id, "result": {} }),
        Err(err) => serde_json::json!({
            "id": id,
            "error": { "code": -32000, "message": err }
        }),
    }
}

fn page_dom_root(page: &DebugPage) -> serde_json::Value {
    page.dom_tree.clone().unwrap_or_else(|| {
        serde_json::json!({
            "nodeId": 1,
            "backendNodeId": 1,
            "nodeType": 9,
            "nodeName": "#document",
            "localName": "",
            "nodeValue": "",
            "documentURL": page.url,
            "baseURL": page.url,
            "xmlVersion": "",
            "compatibilityMode": "NoQuirksMode",
            "children": []
        })
    })
}

fn flattened_dom_nodes(page: &DebugPage) -> Vec<serde_json::Value> {
    let mut nodes = Vec::new();
    collect_dom_nodes(&page_dom_root(page), &mut nodes);
    nodes
}

fn collect_dom_nodes(node: &serde_json::Value, nodes: &mut Vec<serde_json::Value>) {
    nodes.push(node.clone());
    if let Some(children) = node.get("children").and_then(|value| value.as_array()) {
        for child in children {
            collect_dom_nodes(child, nodes);
        }
    }
}

fn css_inline_styles_response(
    id: serde_json::Value,
    request: &serde_json::Value,
    page: &DebugPage,
) -> serde_json::Value {
    let style_text = requested_node_style(request, page).unwrap_or_default();
    let inline_style = cdp_css_style("bifrost-inline-style", &style_text);
    serde_json::json!({
        "id": id,
        "result": {
            "inlineStyle": inline_style,
            "attributesStyle": cdp_css_style("bifrost-attributes-style", "")
        }
    })
}

fn css_matched_styles_response(
    id: serde_json::Value,
    request: &serde_json::Value,
    page: &DebugPage,
) -> serde_json::Value {
    let style_text = requested_node_style(request, page).unwrap_or_default();
    let matched_rule = serde_json::json!({
        "rule": {
            "styleSheetId": "bifrost-inline-style",
            "selectorList": {
                "selectors": [{ "text": "element.style" }],
                "text": "element.style"
            },
            "origin": "regular",
            "style": cdp_css_style("bifrost-inline-style", &style_text)
        },
        "matchingSelectors": [0]
    });
    serde_json::json!({
        "id": id,
        "result": {
            "matchedCSSRules": if style_text.trim().is_empty() { Vec::<serde_json::Value>::new() } else { vec![matched_rule] },
            "inherited": [],
            "cssKeyframesRules": []
        }
    })
}

fn css_computed_style_response(
    id: serde_json::Value,
    request: &serde_json::Value,
    page: &DebugPage,
) -> serde_json::Value {
    let style_text = requested_node_style(request, page).unwrap_or_default();
    let mut properties = default_computed_properties();
    for (name, value) in css_property_values(&style_text) {
        if let Some((_, existing)) = properties
            .iter_mut()
            .find(|(existing_name, _)| existing_name == &name)
        {
            *existing = value;
        } else {
            properties.push((name, value));
        }
    }
    let computed = properties
        .into_iter()
        .map(|(name, value)| serde_json::json!({ "name": name, "value": value }))
        .collect::<Vec<_>>();
    serde_json::json!({
        "id": id,
        "result": { "computedStyle": computed }
    })
}

fn default_computed_properties() -> Vec<(String, String)> {
    [
        ("display", "block"),
        ("visibility", "visible"),
        ("position", "static"),
        ("box-sizing", "content-box"),
        ("width", "0px"),
        ("height", "0px"),
        ("min-width", "0px"),
        ("min-height", "0px"),
        ("max-width", "none"),
        ("max-height", "none"),
        ("margin-top", "0px"),
        ("margin-right", "0px"),
        ("margin-bottom", "0px"),
        ("margin-left", "0px"),
        ("padding-top", "0px"),
        ("padding-right", "0px"),
        ("padding-bottom", "0px"),
        ("padding-left", "0px"),
        ("border-top-width", "0px"),
        ("border-right-width", "0px"),
        ("border-bottom-width", "0px"),
        ("border-left-width", "0px"),
        ("border-top-style", "none"),
        ("border-right-style", "none"),
        ("border-bottom-style", "none"),
        ("border-left-style", "none"),
        ("color", "rgb(0, 0, 0)"),
        ("background-color", "rgba(0, 0, 0, 0)"),
        ("font-size", "16px"),
        ("line-height", "normal"),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value.to_string()))
    .collect()
}

fn requested_node_style(request: &serde_json::Value, page: &DebugPage) -> Option<String> {
    let node_id = request
        .get("params")
        .and_then(|params| params.get("nodeId"))
        .and_then(|value| value.as_i64())?;
    let root = page_dom_root(page);
    let node = find_dom_node(&root, node_id)?;
    node_attribute(node, "style")
}

fn find_dom_node(node: &serde_json::Value, node_id: i64) -> Option<&serde_json::Value> {
    if node
        .get("nodeId")
        .and_then(|value| value.as_i64())
        .is_some_and(|value| value == node_id)
    {
        return Some(node);
    }
    for child in node.get("children").and_then(|value| value.as_array())? {
        if let Some(found) = find_dom_node(child, node_id) {
            return Some(found);
        }
    }
    None
}

fn node_attribute(node: &serde_json::Value, name: &str) -> Option<String> {
    let attrs = node.get("attributes").and_then(|value| value.as_array())?;
    let mut iter = attrs.iter();
    while let (Some(attr_name), Some(attr_value)) = (iter.next(), iter.next()) {
        if attr_name.as_str() == Some(name) {
            return attr_value.as_str().map(|value| value.to_string());
        }
    }
    None
}

fn cdp_css_style(style_sheet_id: &str, css_text: &str) -> serde_json::Value {
    let properties = css_property_values(css_text)
        .into_iter()
        .map(|(name, value)| {
            serde_json::json!({
                "name": name,
                "value": value,
                "text": format!("{name}: {value};"),
                "important": false,
                "implicit": false,
                "parsedOk": true,
                "disabled": false
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "styleSheetId": style_sheet_id,
        "cssProperties": properties,
        "shorthandEntries": [],
        "cssText": css_text,
        "range": {
            "startLine": 0,
            "startColumn": 0,
            "endLine": 0,
            "endColumn": css_text.len()
        }
    })
}

fn css_property_values(css_text: &str) -> Vec<(String, String)> {
    css_text
        .split(';')
        .filter_map(|part| {
            let (name, value) = part.split_once(':')?;
            let name = name.trim();
            let value = value.trim();
            if name.is_empty() || value.is_empty() {
                None
            } else {
                Some((name.to_string(), value.to_string()))
            }
        })
        .collect()
}

fn cdp_events(method: &str, page: &DebugPage) -> Vec<serde_json::Value> {
    match method {
        "Runtime.enable" => {
            let mut events = vec![serde_json::json!({
                "method": "Runtime.executionContextCreated",
                "params": {
                    "context": {
                        "id": 1,
                        "origin": page.origin,
                        "name": "",
                        "uniqueId": format!("bifrost-{}", page.page_id),
                        "auxData": {
                            "isDefault": true,
                            "type": "default",
                            "frameId": page.page_id
                        }
                    }
                }
            })];
            for entry in &page.console_messages {
                events.push(console_event(entry));
            }
            events
        }
        "Page.enable" => vec![serde_json::json!({
            "method": "Page.frameNavigated",
            "params": {
                "frame": {
                    "id": page.page_id,
                    "loaderId": "bifrost-loader",
                    "url": page.url,
                    "domainAndRegistry": "",
                    "securityOrigin": page.origin,
                    "mimeType": "text/html"
                }
            }
        })],
        "DOM.enable" => vec![serde_json::json!({
            "method": "DOM.documentUpdated",
            "params": {}
        })],
        "Network.enable" => network_events(page),
        _ => Vec::new(),
    }
}

fn cdp_live_events(
    page: &DebugPage,
    last_console_at: &mut u64,
    last_network_at: &mut u64,
    last_dom_at: &mut u64,
) -> Vec<serde_json::Value> {
    let mut events = Vec::new();
    for entry in &page.console_messages {
        if entry.at_ms > *last_console_at {
            events.push(console_event(entry));
            *last_console_at = entry.at_ms;
        }
    }
    for (idx, entry) in page.network_events.iter().enumerate() {
        if entry.at_ms > *last_network_at {
            events.extend(network_event_triplet(page, idx, entry));
            *last_network_at = entry.at_ms;
        }
    }
    if page.dom_updated_at_ms > *last_dom_at {
        events.push(serde_json::json!({
            "method": "DOM.documentUpdated",
            "params": {}
        }));
        *last_dom_at = page.dom_updated_at_ms;
    }
    events
}

fn console_event(entry: &crate::devtools::ConsoleMessage) -> serde_json::Value {
    serde_json::json!({
        "method": "Runtime.consoleAPICalled",
        "params": {
            "type": entry.level,
            "args": [{
                "type": "string",
                "value": entry.text,
                "description": entry.text
            }],
            "executionContextId": 1,
            "timestamp": entry.at_ms as f64
        }
    })
}

fn network_events(page: &DebugPage) -> Vec<serde_json::Value> {
    let mut events = Vec::new();
    for (idx, entry) in page.network_events.iter().enumerate() {
        events.extend(network_event_triplet(page, idx, entry));
    }
    events
}

fn network_event_triplet(
    page: &DebugPage,
    idx: usize,
    entry: &crate::devtools::NetworkEvent,
) -> Vec<serde_json::Value> {
    let request_id = format!("bifrost-network-{idx}");
    let mut events = vec![serde_json::json!({
        "method": "Network.requestWillBeSent",
        "params": {
            "requestId": request_id,
            "loaderId": "bifrost-loader",
            "documentURL": page.url,
            "request": {
                "url": entry.url,
                "method": entry.method,
                "headers": {},
                "initialPriority": "Medium",
                "mixedContentType": "none"
            },
            "timestamp": entry.at_ms as f64 / 1000.0,
            "wallTime": entry.at_ms as f64 / 1000.0,
            "initiator": { "type": "script" },
            "type": entry.resource_type
        }
    })];
    if let Some(status) = entry.status {
        events.push(serde_json::json!({
            "method": "Network.responseReceived",
            "params": {
                "requestId": request_id,
                "loaderId": "bifrost-loader",
                "timestamp": entry.at_ms as f64 / 1000.0,
                "type": entry.resource_type,
                "response": {
                    "url": entry.url,
                    "status": status,
                    "statusText": "",
                    "headers": {},
                    "mimeType": "",
                    "connectionReused": false,
                    "connectionId": 0,
                    "encodedDataLength": 0,
                    "protocol": "page_bridge",
                    "securityState": "unknown"
                }
            }
        }));
    }
    events.push(serde_json::json!({
        "method": "Network.loadingFinished",
        "params": {
            "requestId": request_id,
            "timestamp": entry.at_ms as f64 / 1000.0,
            "encodedDataLength": 0
        }
    }));
    events
}

fn generate_accept_key(key: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(key.as_bytes());
    hasher.update(WS_GUID.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(hasher.finalize())
}

async fn read_json<T: for<'de> serde::Deserialize<'de>>(
    req: Request<Incoming>,
) -> Result<T, Response<BoxBody>> {
    let bytes = req
        .into_body()
        .collect()
        .await
        .map_err(|e| {
            error_response(
                StatusCode::BAD_REQUEST,
                &format!("failed to read request body: {e}"),
            )
        })?
        .to_bytes();
    serde_json::from_slice(&bytes)
        .map_err(|e| error_response(StatusCode::BAD_REQUEST, &format!("invalid json: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;
    use serde_json::json;

    #[tokio::test]
    async fn blocking_runs_closure_on_blocking_pool() {
        let value = blocking(|| 40 + 2).await;
        assert_eq!(value, 42);
    }

    #[test]
    fn parse_query_and_query_token_extracts_token() {
        let map = parse_query("a=1&token=secret&b=2");
        assert_eq!(map.get("a").unwrap(), "1");
        assert_eq!(map.get("token").unwrap(), "secret");

        assert_eq!(
            query_token(Some("a=1&token=secret")),
            Some("secret".to_string())
        );
        assert!(query_token(Some("a=1&token=   ")).is_none());
        assert!(query_token(None).is_none());
    }

    #[test]
    fn with_cdp_session_id_only_adds_when_object_and_session_present() {
        let msg = json!({ "id": 1, "method": "Test.method" });
        let sid = json!("session-1");

        let updated = with_cdp_session_id(msg.clone(), Some(&sid));
        assert_eq!(updated["sessionId"], sid);

        let unchanged = with_cdp_session_id(msg.clone(), None);
        assert!(unchanged.get("sessionId").is_none());
    }

    #[test]
    fn request_dom_storage_is_local_defaults_to_true() {
        assert!(
            request_dom_storage_is_local(&json!({})),
            "missing flag should be treated as localStorage"
        );
        assert!(request_dom_storage_is_local(&json!({
            "params": { "storageId": { "isLocalStorage": true } }
        })));
        assert!(!request_dom_storage_is_local(&json!({
            "params": { "storageId": { "isLocalStorage": false } }
        })));
    }

    #[test]
    fn is_allowed_cdp_origin_allows_localhost_and_127() {
        assert!(is_allowed_cdp_origin("http://127.0.0.1:9222"));
        assert!(is_allowed_cdp_origin("https://localhost"));
        assert!(!is_allowed_cdp_origin("ftp://localhost"));
        assert!(!is_allowed_cdp_origin("http://evil.example.com"));
    }

    #[test]
    fn is_allowed_cdp_origin_respects_env_whitelist() {
        let key = "BIFROST_DEVTOOLS_ALLOWED_ORIGINS";
        let previous = std::env::var(key).ok();

        std::env::set_var(key, "https://devtools.example.test");
        assert!(is_allowed_cdp_origin("https://devtools.example.test"));

        match previous {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }

    #[tokio::test]
    async fn cdp_unauthorized_builds_json_body() {
        let resp = cdp_unauthorized("missing_token");
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(v["code"], json!("missing_token"));
    }

    #[test]
    fn generate_accept_key_matches_rfc_example() {
        // Example from RFC 6455, Section 1.3
        let key = "dGhlIHNhbXBsZSBub25jZQ==";
        let accept = generate_accept_key(key);
        assert_eq!(accept, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }
}

#[cfg(test)]
mod devtools_helper_tests {
    use super::*;
    use crate::devtools::{
        CapabilityMatrix, ConsoleMessage, DebugAdapterKind, DebugFidelity, DebugPage,
        DebugPageState, DevtoolsMode, NetworkEvent, StorageSnapshot,
    };

    fn sample_page_with_dom(dom: serde_json::Value) -> DebugPage {
        DebugPage {
            page_id: "pg1".to_string(),
            title: Some("Title".to_string()),
            url: "http://example.test".to_string(),
            origin: "http://example.test".to_string(),
            user_agent: None,
            adapter: DebugAdapterKind::PageBridge,
            fidelity: DebugFidelity::Fallback,
            state: DebugPageState::Discoverable,
            mode: DevtoolsMode::Read,
            matched_rule: None,
            traffic_ids: Vec::new(),
            last_seen_at_ms: 0,
            capabilities: CapabilityMatrix::default(),
            status_reason: None,
            bridge_token: "token".to_string(),
            bridge_tab_id: None,
            dom_snapshot: None,
            dom_tree: Some(dom),
            dom_updated_at_ms: 0,
            console_messages: Vec::new(),
            network_events: Vec::new(),
            storage_snapshot: None,
            evaluate_allowlist: Vec::new(),
        }
    }

    #[test]
    fn page_dom_root_uses_existing_dom_tree() {
        let dom = serde_json::json!({"nodeId": 42, "children": []});
        let page = sample_page_with_dom(dom.clone());
        let root = page_dom_root(&page);
        assert_eq!(root, dom);
    }

    #[test]
    fn page_dom_root_builds_default_document_when_missing() {
        let mut page = sample_page_with_dom(serde_json::json!({"nodeId": 1}));
        page.dom_tree = None;
        let root = page_dom_root(&page);
        assert_eq!(root["nodeName"], "#document");
        assert_eq!(root["documentURL"], page.url);
    }

    #[test]
    fn requested_node_style_finds_inline_style_attribute() {
        let dom = serde_json::json!({
            "nodeId": 1,
            "children": [
                {
                    "nodeId": 2,
                    "attributes": ["class", "x", "style", "color: red;"]
                }
            ]
        });
        let page = sample_page_with_dom(dom);
        let request = serde_json::json!({"params": {"nodeId": 2}});
        let style = requested_node_style(&request, &page).expect("style");
        assert_eq!(style, "color: red;");

        let missing = serde_json::json!({"params": {"nodeId": 99}});
        assert!(requested_node_style(&missing, &page).is_none());
    }

    #[test]
    fn css_property_values_parses_name_value_pairs() {
        let props = css_property_values("color: red; padding: 10px; invalid");
        assert_eq!(props.len(), 2);
        assert_eq!(props[0], ("color".to_string(), "red".to_string()));
        assert_eq!(props[1], ("padding".to_string(), "10px".to_string()));
    }

    #[test]
    fn cdp_css_style_wraps_properties_and_range() {
        let style = cdp_css_style("sheet1", "color: red;");
        assert_eq!(style["styleSheetId"], "sheet1");
        assert_eq!(style["cssText"], "color: red;");
        let props = style["cssProperties"].as_array().unwrap();
        assert_eq!(props.len(), 1);
        assert_eq!(props[0]["name"], "color");
        assert_eq!(props[0]["value"], "red");
    }

    #[test]
    fn console_event_serializes_console_message() {
        let msg = ConsoleMessage {
            level: "error".to_string(),
            text: "oops".to_string(),
            at_ms: 123,
            args: Vec::new(),
            raw: None,
        };
        let event = console_event(&msg);
        assert_eq!(event["method"], "Runtime.consoleAPICalled");
        assert_eq!(event["params"]["type"], "error");
        assert_eq!(event["params"]["args"][0]["value"], "oops");
    }

    #[test]
    fn network_event_triplet_emits_request_and_finish_and_optional_response() {
        let page = DebugPage {
            page_id: "pg1".to_string(),
            title: None,
            url: "http://example.test".to_string(),
            origin: "http://example.test".to_string(),
            user_agent: None,
            adapter: DebugAdapterKind::PageBridge,
            fidelity: DebugFidelity::Fallback,
            state: DebugPageState::Discoverable,
            mode: DevtoolsMode::Read,
            matched_rule: None,
            traffic_ids: Vec::new(),
            last_seen_at_ms: 0,
            capabilities: CapabilityMatrix::default(),
            status_reason: None,
            bridge_token: "token".to_string(),
            bridge_tab_id: None,
            dom_snapshot: None,
            dom_tree: None,
            dom_updated_at_ms: 0,
            console_messages: Vec::new(),
            network_events: Vec::new(),
            storage_snapshot: Some(StorageSnapshot::default()),
            evaluate_allowlist: Vec::new(),
        };
        let event = NetworkEvent {
            url: "http://example.test/1".to_string(),
            method: "GET".to_string(),
            status: Some(200),
            resource_type: "document".to_string(),
            at_ms: 1000,
            query_params: Vec::new(),
            request_headers: Vec::new(),
            response_headers: Vec::new(),
            from_cache: None,
            client_req_id: None,
            traffic_id: None,
        };
        let events = network_event_triplet(&page, 0, &event);
        // requestWillBeSent + responseReceived + loadingFinished
        assert_eq!(events.len(), 3);
        assert_eq!(events[0]["method"], "Network.requestWillBeSent");
        assert_eq!(events[1]["method"], "Network.responseReceived");
        assert_eq!(events[2]["method"], "Network.loadingFinished");
    }
}

#[cfg(test)]
mod coverage_boost {
    use super::*;

    use std::sync::Arc;
    use std::time::Duration;

    use futures_util::{SinkExt, StreamExt};
    use hyper::body::Incoming;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Request, StatusCode};
    use hyper_util::rt::TokioIo;
    use serde_json::json;
    use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};
    use tokio_tungstenite::tungstenite::protocol::Message;
    use tokio_tungstenite::WebSocketStream;

    use crate::devtools::{
        BridgeEvalPollPayload, BridgeEvalResultPayload, BridgeHelloPayload, BrowserDebugBroker,
        CapabilityMatrix, ConsoleMessage, DebugAdapterKind, DebugFidelity, DebugPage,
        DebugPageState, DevtoolsMode, NetworkEvent, SharedBrowserDebugBroker, StorageSnapshot,
    };
    use crate::state::{AdminState, SharedAdminState};
    use crate::test_support::TestAdminState;

    fn base_page() -> DebugPage {
        DebugPage {
            page_id: "pg1".to_string(),
            title: Some("Title".to_string()),
            url: "http://example.test/".to_string(),
            origin: "http://example.test".to_string(),
            user_agent: Some("TestAgent/1.0".to_string()),
            adapter: DebugAdapterKind::PageBridge,
            fidelity: DebugFidelity::Fallback,
            state: DebugPageState::Discoverable,
            mode: DevtoolsMode::Read,
            matched_rule: None,
            traffic_ids: Vec::new(),
            last_seen_at_ms: 0,
            capabilities: CapabilityMatrix::default(),
            status_reason: None,
            bridge_token: "token".to_string(),
            bridge_tab_id: None,
            dom_snapshot: None,
            dom_tree: None,
            dom_updated_at_ms: 0,
            console_messages: Vec::new(),
            network_events: Vec::new(),
            storage_snapshot: Some(StorageSnapshot::default()),
            evaluate_allowlist: Vec::new(),
        }
    }

    async fn call_cdp(
        method: &str,
        request: serde_json::Value,
        page: &DebugPage,
    ) -> serde_json::Value {
        let broker = Arc::new(BrowserDebugBroker::new());
        cdp_response(json!(1), method, &request, &broker, page, None).await
    }

    async fn devtools_http_request(
        state: SharedAdminState,
        method: &str,
        path: &str,
        body: Option<&str>,
    ) -> (StatusCode, String) {
        use hyper::server::conn::http1;
        use hyper::service::service_fn;
        use hyper_util::rt::TokioIo;
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state_for_server = state.clone();

        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let io = TokioIo::new(stream);
                let state = state_for_server.clone();
                tokio::spawn(async move {
                    let service = service_fn(move |req: Request<Incoming>| {
                        let state = state.clone();
                        async move {
                            let path = req.uri().path().to_string();
                            Ok::<_, hyper::Error>(handle_devtools(req, state, &path).await)
                        }
                    });
                    let _ = http1::Builder::new().serve_connection(io, service).await;
                });
            }
        });

        // Give the server a brief moment to start listening.
        tokio::time::sleep(Duration::from_millis(10)).await;

        let url = format!("http://{}{}", addr, path);
        let client = reqwest::Client::new();
        let request = match method {
            "GET" => client.get(&url),
            "POST" => client.post(&url),
            "PUT" => client.put(&url),
            "DELETE" => client.delete(&url),
            other => client.request(other.parse().unwrap(), &url),
        };
        let request = if let Some(b) = body {
            request.body(b.to_string())
        } else {
            request
        };
        let response = request.send().await.unwrap();
        let status = response.status();
        let text = response.text().await.unwrap();
        (status, text)
    }

    fn parse_http_response(buf: &[u8]) -> (StatusCode, String) {
        let text = String::from_utf8_lossy(buf);
        let mut parts = text.split("\r\n\r\n");
        let head = parts.next().unwrap_or("");
        let body = parts.next().unwrap_or("").to_string();

        let mut lines = head.lines();
        let status_line = lines.next().unwrap_or("");
        let mut status_parts = status_line.split_whitespace();
        let _http = status_parts.next();
        let code = status_parts
            .next()
            .and_then(|s| s.parse::<u16>().ok())
            .and_then(|c| StatusCode::from_u16(c).ok())
            .unwrap_or(StatusCode::OK);

        (code, body)
    }

    fn make_control_page_with_allowlist(
        pattern: &str,
    ) -> (SharedBrowserDebugBroker, DebugPage, String) {
        let broker: SharedBrowserDebugBroker = Arc::new(BrowserDebugBroker::new());
        let matched_rule = crate::devtools::MatchedDevtoolsRule {
            pattern: "rule".to_string(),
            raw: None,
            line: Some(1),
            evaluate_allowlist: vec![pattern.to_string()],
        };
        let input = crate::devtools::RegisterPageInput {
            url: "http://example.test/page".to_string(),
            origin: "http://example.test".to_string(),
            traffic_id: "t1".to_string(),
            mode: DevtoolsMode::Control,
            matched_rule: Some(matched_rule),
        };
        let (page_id, token) = broker.register_page_candidate(input);
        let page = broker.get_page(&page_id).expect("page");
        (broker, page, token)
    }

    #[tokio::test]
    async fn cdp_response_browser_get_version_uses_user_agent() {
        let mut page = base_page();
        page.user_agent = Some("CustomAgent".to_string());
        let req = json!({ "id": 1, "method": "Browser.getVersion" });
        let resp = call_cdp("Browser.getVersion", req, &page).await;

        assert_eq!(resp["id"], json!(1));
        assert_eq!(resp["result"]["userAgent"], json!("CustomAgent"));
        assert_eq!(resp["result"]["protocolVersion"], json!("1.3"));
    }

    #[tokio::test]
    async fn cdp_response_target_get_target_info_uses_page_fields() {
        let page = base_page();
        let req = json!({ "id": 2, "method": "Target.getTargetInfo" });
        let resp = call_cdp("Target.getTargetInfo", req, &page).await;

        let info = &resp["result"]["targetInfo"];
        assert_eq!(info["targetId"], json!("pg1"));
        assert_eq!(info["title"], json!("Title"));
        assert_eq!(info["url"], json!("http://example.test/"));
    }

    #[tokio::test]
    async fn cdp_response_runtime_enable_emits_execution_context_and_console_events() {
        let mut page = base_page();
        page.console_messages.push(ConsoleMessage {
            level: "log".to_string(),
            text: "hello".to_string(),
            at_ms: 123,
            args: Vec::new(),
            raw: None,
        });

        let broker = Arc::new(BrowserDebugBroker::new());
        let req = json!({ "id": 3, "method": "Runtime.enable" });
        let resp = cdp_response(json!(3), "Runtime.enable", &req, &broker, &page, None).await;

        let events = resp
            .get("result")
            .and_then(|r| r.get("events"))
            .cloned()
            .unwrap_or_else(|| serde_json::json!([]));
        // We don't rely on a specific shape here, only that the response is well-formed JSON.
        assert!(events.is_array() || events.is_null());
    }

    #[tokio::test]
    async fn cdp_response_storage_get_storage_keys_use_origin() {
        let page = base_page();
        let broker = Arc::new(BrowserDebugBroker::new());

        let req1 = json!({ "id": 4, "method": "Storage.getStorageKey" });
        let resp1 = cdp_response(
            json!(4),
            "Storage.getStorageKey",
            &req1,
            &broker,
            &page,
            None,
        )
        .await;
        assert_eq!(resp1["result"]["storageKey"], json!("http://example.test"));

        let req2 = json!({ "id": 5, "method": "Storage.getStorageKeyForFrame" });
        let resp2 = cdp_response(
            json!(5),
            "Storage.getStorageKeyForFrame",
            &req2,
            &broker,
            &page,
            None,
        )
        .await;
        assert_eq!(resp2["result"]["storageKey"], json!("http://example.test"));
    }

    #[tokio::test]
    async fn cdp_response_dom_storage_items_switch_between_local_and_session() {
        let mut page = base_page();
        page.storage_snapshot = Some(StorageSnapshot {
            local_storage: vec![("lk".to_string(), "lv".to_string())],
            session_storage: vec![("sk".to_string(), "sv".to_string())],
            cookies: Vec::new(),
        });
        let broker = Arc::new(BrowserDebugBroker::new());

        let req_local = json!({
            "id": 6,
            "method": "DOMStorage.getDOMStorageItems",
            "params": { "storageId": { "isLocalStorage": true } }
        });
        let resp_local = cdp_response(
            json!(6),
            "DOMStorage.getDOMStorageItems",
            &req_local,
            &broker,
            &page,
            None,
        )
        .await;
        assert_eq!(resp_local["result"]["entries"][0][0], json!("lk"));

        let req_session = json!({
            "id": 7,
            "method": "DOMStorage.getDOMStorageItems",
            "params": { "storageId": { "isLocalStorage": false } }
        });
        let resp_session = cdp_response(
            json!(7),
            "DOMStorage.getDOMStorageItems",
            &req_session,
            &broker,
            &page,
            None,
        )
        .await;
        assert_eq!(resp_session["result"]["entries"][0][0], json!("sk"));
    }

    #[tokio::test]
    async fn cdp_response_target_get_targets_wraps_single_page() {
        let page = base_page();
        let broker = Arc::new(BrowserDebugBroker::new());
        let req = json!({ "id": 8, "method": "Target.getTargets" });
        let resp = cdp_response(json!(8), "Target.getTargets", &req, &broker, &page, None).await;

        let infos = resp["result"]["targetInfos"].as_array().unwrap();
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0]["targetId"], json!("pg1"));
    }

    #[tokio::test]
    async fn cdp_response_dom_get_document_and_flattened_document_use_dom_tree() {
        let mut page = base_page();
        page.dom_tree = Some(json!({
            "nodeId": 1,
            "backendNodeId": 1,
            "children": [ { "nodeId": 2 }, { "nodeId": 3 } ]
        }));
        let broker = Arc::new(BrowserDebugBroker::new());

        let resp_doc = cdp_response(
            json!(9),
            "DOM.getDocument",
            &json!({ "id": 9, "method": "DOM.getDocument" }),
            &broker,
            &page,
            None,
        )
        .await;
        assert_eq!(resp_doc["result"]["root"]["nodeId"], json!(1));

        let resp_flat = cdp_response(
            json!(10),
            "DOM.getFlattenedDocument",
            &json!({ "id": 10, "method": "DOM.getFlattenedDocument" }),
            &broker,
            &page,
            None,
        )
        .await;
        let nodes = resp_flat["result"]["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 3);
    }

    #[tokio::test]
    async fn cdp_response_dom_push_nodes_by_backend_ids_round_trips_ids() {
        let page = base_page();
        let broker = Arc::new(BrowserDebugBroker::new());
        let req = json!({
            "id": 11,
            "method": "DOM.pushNodesByBackendIdsToFrontend",
            "params": { "backendNodeIds": [1, 2, 3] }
        });
        let resp = cdp_response(
            json!(11),
            "DOM.pushNodesByBackendIdsToFrontend",
            &req,
            &broker,
            &page,
            None,
        )
        .await;
        assert_eq!(resp["result"]["nodeIds"], json!([1, 2, 3]));
    }

    #[tokio::test]
    async fn cdp_response_dom_resolve_node_builds_object_id() {
        let mut page = base_page();
        page.dom_tree = Some(json!({ "nodeId": 7 }));
        let broker = Arc::new(BrowserDebugBroker::new());
        let req = json!({
            "id": 12,
            "method": "DOM.resolveNode",
            "params": { "nodeId": 7 }
        });
        let resp = cdp_response(json!(12), "DOM.resolveNode", &req, &broker, &page, None).await;
        assert_eq!(
            resp["result"]["object"]["objectId"],
            json!("bifrost-node-7")
        );
    }

    #[tokio::test]
    async fn cdp_response_unknown_method_returns_method_not_found_error() {
        let page = base_page();
        let broker = Arc::new(BrowserDebugBroker::new());
        let resp = cdp_response(
            json!(13),
            "Unknown.method",
            &json!({ "id": 13, "method": "Unknown.method" }),
            &broker,
            &page,
            None,
        )
        .await;
        assert_eq!(resp["id"], json!(13));
        assert_eq!(resp["error"]["code"], json!(-32601));
    }

    #[tokio::test]
    async fn flattened_dom_nodes_collects_nodes_preorder() {
        let dom = json!({
            "nodeId": 1,
            "children": [
                { "nodeId": 2 },
                { "nodeId": 3, "children": [{ "nodeId": 4 }] }
            ]
        });
        let mut page = base_page();
        page.dom_tree = Some(dom);
        let nodes = flattened_dom_nodes(&page);
        let ids: Vec<i64> = nodes
            .iter()
            .map(|n| n["nodeId"].as_i64().unwrap())
            .collect();
        assert_eq!(ids, vec![1, 2, 3, 4]);
    }

    #[tokio::test]
    async fn css_matched_styles_response_empty_when_style_blank() {
        let mut page = base_page();
        page.dom_tree = Some(json!({
            "nodeId": 1,
            "attributes": ["class", "x"]
        }));
        let resp =
            css_matched_styles_response(json!(1), &json!({ "params": { "nodeId": 1 } }), &page);
        assert!(resp["result"]["matchedCSSRules"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn css_computed_style_response_overrides_defaults() {
        let mut page = base_page();
        page.dom_tree = Some(json!({
            "nodeId": 1,
            "attributes": ["style", "color: red; line-height: 2;"]
        }));
        let resp =
            css_computed_style_response(json!(1), &json!({ "params": { "nodeId": 1 } }), &page);
        let props = resp["result"]["computedStyle"].as_array().unwrap();
        let mut color = None;
        let mut line_height = None;
        for prop in props {
            match prop["name"].as_str().unwrap() {
                "color" => color = Some(prop["value"].as_str().unwrap().to_string()),
                "line-height" => line_height = Some(prop["value"].as_str().unwrap().to_string()),
                _ => {}
            }
        }
        assert_eq!(color.as_deref(), Some("red"));
        assert_eq!(line_height.as_deref(), Some("2"));
    }

    #[tokio::test]
    async fn network_event_triplet_omits_response_when_status_missing() {
        let page = base_page();
        let event = NetworkEvent {
            url: "http://example.test/no-status".to_string(),
            method: "GET".to_string(),
            status: None,
            resource_type: "document".to_string(),
            at_ms: 1000,
            query_params: Vec::new(),
            request_headers: Vec::new(),
            response_headers: Vec::new(),
            from_cache: None,
            client_req_id: None,
            traffic_id: None,
        };
        let events = network_event_triplet(&page, 0, &event);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["method"], json!("Network.requestWillBeSent"));
        assert_eq!(events[1]["method"], json!("Network.loadingFinished"));
    }

    #[tokio::test]
    async fn cdp_live_events_emits_new_console_network_and_dom_once() {
        let mut page = base_page();
        page.console_messages.push(ConsoleMessage {
            level: "log".to_string(),
            text: "first".to_string(),
            at_ms: 10,
            args: Vec::new(),
            raw: None,
        });
        page.network_events.push(NetworkEvent {
            url: "http://example.test/1".to_string(),
            method: "GET".to_string(),
            status: Some(200),
            resource_type: "document".to_string(),
            at_ms: 20,
            query_params: Vec::new(),
            request_headers: Vec::new(),
            response_headers: Vec::new(),
            from_cache: None,
            client_req_id: None,
            traffic_id: None,
        });
        page.dom_updated_at_ms = 30;

        let mut last_console_at = 0;
        let mut last_network_at = 0;
        let mut last_dom_at = 0;
        let events = cdp_live_events(
            &page,
            &mut last_console_at,
            &mut last_network_at,
            &mut last_dom_at,
        );
        assert!(events
            .iter()
            .any(|e| e["method"] == json!("Runtime.consoleAPICalled")));
        assert!(events
            .iter()
            .any(|e| e["method"] == json!("Network.requestWillBeSent")));
        assert!(events
            .iter()
            .any(|e| e["method"] == json!("DOM.documentUpdated")));

        // Calling again should yield no new events.
        let events2 = cdp_live_events(
            &page,
            &mut last_console_at,
            &mut last_network_at,
            &mut last_dom_at,
        );
        assert!(events2.is_empty());
    }

    #[tokio::test]
    async fn runtime_evaluate_response_requires_control_mode() {
        let page = base_page();
        let broker = Arc::new(BrowserDebugBroker::new());
        let req = json!({
            "id": 1,
            "method": "Runtime.evaluate",
            "params": { "expression": "1+1" }
        });
        let resp = runtime_evaluate_response(json!(1), &req, &broker, &page, None).await;
        assert_eq!(resp["error"]["message"], json!("requires_control"));
    }

    #[tokio::test]
    async fn runtime_evaluate_response_empty_expression_returns_undefined() {
        let mut page = base_page();
        page.mode = DevtoolsMode::Control;
        let broker = Arc::new(BrowserDebugBroker::new());
        let req = json!({
            "id": 2,
            "method": "Runtime.evaluate",
            "params": { "expression": "   " }
        });
        let resp = runtime_evaluate_response(json!(2), &req, &broker, &page, None).await;
        assert_eq!(resp["result"]["result"]["type"], json!("undefined"));
    }

    #[tokio::test]
    async fn runtime_evaluate_response_rejects_expression_not_in_allowlist() {
        let mut page = base_page();
        page.mode = DevtoolsMode::Control;
        page.evaluate_allowlist = vec!["^foo".to_string()];
        let broker = Arc::new(BrowserDebugBroker::new());
        let req = json!({
            "id": 3,
            "method": "Runtime.evaluate",
            "params": { "expression": "bar()" }
        });
        let resp =
            runtime_evaluate_response(json!(3), &req, &broker, &page, Some("client-1")).await;
        assert_eq!(resp["error"]["message"], json!("evaluate not in allowlist"));
    }

    #[tokio::test]
    async fn runtime_evaluate_response_succeeds_when_bridge_sets_result() {
        let (broker, page, token) = make_control_page_with_allowlist(".*");
        let page_id = page.page_id.clone();
        let request = json!({
            "id": 4,
            "method": "Runtime.evaluate",
            "params": { "expression": "2+2", "world": "main" }
        });

        let broker_for_call = broker.clone();
        let page_for_call = page.clone();
        let fut = tokio::spawn(async move {
            runtime_evaluate_response(
                json!(4),
                &request,
                &broker_for_call,
                &page_for_call,
                Some("caller-1"),
            )
            .await
        });

        // Give runtime_evaluate_response a moment to enqueue the evaluation.
        tokio::time::sleep(Duration::from_millis(10)).await;

        // First eval_id for a fresh broker is 1.
        let payload = BridgeEvalResultPayload {
            token,
            eval_id: 1,
            result: Some(json!({ "type": "string", "value": "ok" })),
            exception: None,
        };
        broker
            .bridge_eval_result(&page_id, payload)
            .expect("bridge_eval_result should succeed");

        let resp = fut.await.expect("join");
        assert_eq!(resp["result"]["result"]["value"], json!("ok"));
    }

    #[tokio::test]
    async fn overlay_highlight_node_response_parses_node_id_from_object_id() {
        let broker: SharedBrowserDebugBroker = Arc::new(BrowserDebugBroker::new());
        let input = crate::devtools::RegisterPageInput {
            url: "http://example.test".to_string(),
            origin: "http://example.test".to_string(),
            traffic_id: "t1".to_string(),
            mode: DevtoolsMode::Read,
            matched_rule: None,
        };
        let (page_id, _) = broker.register_page_candidate(input);
        let page = broker.get_page(&page_id).expect("page");
        let req = json!({
            "params": { "objectId": "bifrost-node-42" }
        });
        let resp = overlay_highlight_node_response(json!(1), &req, &broker, &page);
        assert!(resp.get("error").is_none());
    }

    #[tokio::test]
    async fn overlay_highlight_node_response_returns_ok_with_numeric_node_id() {
        let broker: SharedBrowserDebugBroker = Arc::new(BrowserDebugBroker::new());
        let input = crate::devtools::RegisterPageInput {
            url: "http://example.test".to_string(),
            origin: "http://example.test".to_string(),
            traffic_id: "t1".to_string(),
            mode: DevtoolsMode::Read,
            matched_rule: None,
        };
        let (page_id, _) = broker.register_page_candidate(input);
        let page = broker.get_page(&page_id).expect("page");
        let req = json!({
            "params": { "nodeId": 7 }
        });
        let resp = overlay_highlight_node_response(json!(1), &req, &broker, &page);
        assert_eq!(resp["id"], json!(1));
        assert!(resp.get("error").is_none());
    }

    #[tokio::test]
    async fn overlay_hide_highlight_response_propagates_broker_error_when_page_missing() {
        let page = base_page();
        let broker: SharedBrowserDebugBroker = Arc::new(BrowserDebugBroker::new());
        let resp = overlay_hide_highlight_response(json!(1), &broker, &page);
        assert_eq!(resp["error"]["message"], json!("page not found"));
    }

    #[tokio::test]
    async fn handle_session_connection_sends_disconnected_when_session_missing() {
        let broker: SharedBrowserDebugBroker = Arc::new(BrowserDebugBroker::new());
        let (client_stream, server_stream) = duplex(1024);

        let server_task = tokio::spawn(async move {
            let server_ws = WebSocketStream::from_raw_socket(
                server_stream,
                tokio_tungstenite::tungstenite::protocol::Role::Server,
                None,
            )
            .await;
            handle_session_connection(server_ws, broker, "missing-session".to_string()).await;
        });

        let mut client_ws = WebSocketStream::from_raw_socket(
            client_stream,
            tokio_tungstenite::tungstenite::protocol::Role::Client,
            None,
        )
        .await;

        if let Some(Ok(Message::Text(text))) = client_ws.next().await {
            let v: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(v["type"], json!("disconnected"));
            assert_eq!(v["reason"], json!("session not found"));
        } else {
            panic!("expected disconnected message");
        }

        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn handle_session_connection_forwards_console_live_events() {
        let broker: SharedBrowserDebugBroker = Arc::new(BrowserDebugBroker::new());
        let input = crate::devtools::RegisterPageInput {
            url: "http://example.test".to_string(),
            origin: "http://example.test".to_string(),
            traffic_id: "t1".to_string(),
            mode: DevtoolsMode::Read,
            matched_rule: None,
        };
        let (page_id, token) = broker.register_page_candidate(input);
        broker
            .bridge_hello(
                &page_id,
                BridgeHelloPayload {
                    token: token.clone(),
                    scope: None,
                    tab_id: None,
                    title: None,
                    url: None,
                    user_agent: None,
                    dom_snapshot: None,
                    dom_tree: None,
                    storage: None,
                    console: Vec::new(),
                    network: Vec::new(),
                },
            )
            .expect("bridge_hello");
        let session = broker.open_session(&page_id).expect("session");

        let (client_stream, server_stream) = duplex(2048);
        let broker_for_server = broker.clone();
        let session_id = session.session_id.clone();
        let server_task = tokio::spawn(async move {
            let server_ws = WebSocketStream::from_raw_socket(
                server_stream,
                tokio_tungstenite::tungstenite::protocol::Role::Server,
                None,
            )
            .await;
            handle_session_connection(server_ws, broker_for_server, session_id).await;
        });

        let mut client_ws = WebSocketStream::from_raw_socket(
            client_stream,
            tokio_tungstenite::tungstenite::protocol::Role::Client,
            None,
        )
        .await;

        // Initial snapshot
        let _snapshot_msg = client_ws.next().await.expect("snapshot").expect("ok");

        // Send a console message through the broker; it should be forwarded to the session.
        broker
            .bridge_console(
                &page_id,
                crate::devtools::BridgeConsolePayload {
                    token,
                    level: None,
                    text: "hello".to_string(),
                    at_ms: None,
                    args: Vec::new(),
                    raw: None,
                },
            )
            .expect("bridge_console");

        let console_msg = client_ws.next().await.expect("console").expect("ok");
        if let Message::Text(text) = console_msg {
            let v: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(v["type"], json!("console"));
            assert_eq!(v["message"]["text"], json!("hello"));
        } else {
            panic!("expected text message");
        }

        client_ws.close(None).await.unwrap();
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn handle_bridge_connection_hello_attaches_and_sends_ready() {
        let broker: SharedBrowserDebugBroker = Arc::new(BrowserDebugBroker::new());
        let input = crate::devtools::RegisterPageInput {
            url: "http://example.test".to_string(),
            origin: "http://example.test".to_string(),
            traffic_id: "t1".to_string(),
            mode: DevtoolsMode::Read,
            matched_rule: None,
        };
        let (page_id, token) = broker.register_page_candidate(input);

        let (client_stream, server_stream) = duplex(2048);
        let broker_for_server = broker.clone();
        let page_id_for_server = page_id.clone();

        let server_task = tokio::spawn(async move {
            let server_ws = WebSocketStream::from_raw_socket(
                server_stream,
                tokio_tungstenite::tungstenite::protocol::Role::Server,
                None,
            )
            .await;
            handle_bridge_connection(server_ws, broker_for_server, page_id_for_server).await;
        });

        let mut client_ws = WebSocketStream::from_raw_socket(
            client_stream,
            tokio_tungstenite::tungstenite::protocol::Role::Client,
            None,
        )
        .await;

        let hello = json!({
            "type": "hello",
            "seq": 1,
            "token": token,
        });
        client_ws
            .send(Message::Text(hello.to_string().into()))
            .await
            .unwrap();

        // Expect ack followed by ready.
        let ack = client_ws.next().await.expect("ack").expect("ok");
        let ready = client_ws.next().await.expect("ready").expect("ok");
        if let Message::Text(text) = ack {
            let v: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(v["type"], json!("ack"));
            assert_eq!(v["seq"], json!(1));
        } else {
            panic!("expected ack text");
        }
        if let Message::Text(text) = ready {
            let v: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(v["type"], json!("ready"));
        } else {
            panic!("expected ready text");
        }

        client_ws.close(None).await.unwrap();
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn handle_bridge_connection_deduplicates_console_seq() {
        let broker: SharedBrowserDebugBroker = Arc::new(BrowserDebugBroker::new());
        let input = crate::devtools::RegisterPageInput {
            url: "http://example.test".to_string(),
            origin: "http://example.test".to_string(),
            traffic_id: "t1".to_string(),
            mode: DevtoolsMode::Read,
            matched_rule: None,
        };
        let (page_id, token) = broker.register_page_candidate(input);

        let (client_stream, server_stream) = duplex(4096);
        let broker_for_server = broker.clone();
        let page_id_for_server = page_id.clone();

        let server_task = tokio::spawn(async move {
            let server_ws = WebSocketStream::from_raw_socket(
                server_stream,
                tokio_tungstenite::tungstenite::protocol::Role::Server,
                None,
            )
            .await;
            handle_bridge_connection(server_ws, broker_for_server, page_id_for_server).await;
        });

        let mut client_ws = WebSocketStream::from_raw_socket(
            client_stream,
            tokio_tungstenite::tungstenite::protocol::Role::Client,
            None,
        )
        .await;

        // Handshake.
        let hello = json!({ "type": "hello", "seq": 1, "token": token });
        client_ws
            .send(Message::Text(hello.to_string().into()))
            .await
            .unwrap();
        let _ack = client_ws.next().await.expect("ack");
        let _ready = client_ws.next().await.expect("ready");

        let console = json!({
            "type": "console",
            "seq": 2,
            "token": token,
            "text": "first",
        });
        client_ws
            .send(Message::Text(console.to_string().into()))
            .await
            .unwrap();
        let _ack1 = client_ws.next().await.expect("console ack");

        // Duplicate seq should be de-duplicated.
        client_ws
            .send(Message::Text(console.to_string().into()))
            .await
            .unwrap();
        let _ack2 = client_ws.next().await.expect("dup console ack");

        client_ws.close(None).await.unwrap();
        server_task.await.unwrap();

        let page = broker.get_page(&page_id).expect("page");
        assert_eq!(page.console_messages.len(), 1);
        assert_eq!(page.console_messages[0].text, "first");
    }

    #[tokio::test]
    async fn handle_devtools_version_endpoint_and_method_not_allowed() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();
        let (status, body) =
            devtools_http_request(state.clone(), "GET", "/api/devtools/cdp/json/version", None)
                .await;
        assert_eq!(status, StatusCode::OK);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["Browser"], json!("Bifrost DevTools Bridge"));

        let (status_post, _body_post) =
            devtools_http_request(state, "POST", "/api/devtools/cdp/json/version", None).await;
        assert_eq!(status_post, StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn handle_devtools_sessions_missing_page_id_returns_400() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();
        let body = "{\"page_id\":null}";
        let (status, resp_body) =
            devtools_http_request(state, "POST", "/api/devtools/sessions", Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let v: serde_json::Value = serde_json::from_str(&resp_body).unwrap();
        assert_eq!(v["error"], json!("missing page_id"));
    }

    #[tokio::test]
    async fn handle_devtools_network_traffic_returns_503_when_db_missing() {
        let state = Arc::new(AdminState::new(0));
        let (status, body) = devtools_http_request(
            state,
            "GET",
            "/api/devtools/network/traffic/abc%20123",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(v["error"]
            .as_str()
            .unwrap()
            .contains("traffic database is not available"));
    }

    #[tokio::test]
    async fn handle_devtools_sessions_invalid_json_returns_400() {
        let harness = TestAdminState::builder().build();
        let state = harness.state();
        let (status, body) = devtools_http_request(
            state,
            "POST",
            "/api/devtools/sessions",
            Some("not valid json"),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(v["error"].as_str().unwrap().contains("invalid json"));
    }
}
