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
