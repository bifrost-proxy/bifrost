use base64::Engine;
use bifrost_core::text::truncate_chars;
use futures_util::{SinkExt, StreamExt};
use hyper::body::Incoming;
use hyper::{Request, Response, StatusCode};
use sha1::{Digest, Sha1};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::WebSocketStream;
use tracing::{debug, error, warn};

use super::{error_response, BoxBody};
use crate::push::{
    ClientSubscription, ConnectedData, PushMessage, SharedPushManager, MAX_ID_LEN,
    MAX_SETTINGS_SCOPES, MAX_SUBSCRIBED_IDS, METRICS_INTERVAL_MAX_MS, METRICS_INTERVAL_MIN_MS,
};

const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const WS_PING_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);
const WS_PONG_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const WS_TEXT_MAX_BYTES: usize = 64 * 1024;
const HISTORY_LIMIT_MAX: usize = 500;

pub async fn handle_websocket_upgrade(
    req: Request<Incoming>,
    push_manager: SharedPushManager,
) -> Response<BoxBody> {
    let upgrade_header = req
        .headers()
        .get("Upgrade")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if !upgrade_header.eq_ignore_ascii_case("websocket") {
        return error_response(StatusCode::BAD_REQUEST, "Invalid upgrade header");
    }

    if let Some(reason) = crate::cors::websocket_origin_rejection(req.headers()) {
        warn!(reason, "push WebSocket upgrade rejected (CSWSH guard)");
        return error_response(
            StatusCode::FORBIDDEN,
            "Cross-site WebSocket upgrade rejected",
        );
    }
    let ws_key = match req.headers().get("Sec-WebSocket-Key") {
        Some(key) => key.to_str().unwrap_or("").to_string(),
        None => {
            return error_response(StatusCode::BAD_REQUEST, "Missing Sec-WebSocket-Key header");
        }
    };

    let query = req.uri().query().unwrap_or("");
    let (client_key, subscription) = parse_subscription_from_query(query);

    let accept_key = generate_accept_key(&ws_key);

    tokio::spawn(async move {
        let upgraded = match hyper::upgrade::on(req).await {
            Ok(u) => u,
            Err(e) => {
                error!("WebSocket upgrade failed: {}", e);
                return;
            }
        };

        let ws_stream = WebSocketStream::from_raw_socket(
            hyper_util::rt::TokioIo::new(upgraded),
            tokio_tungstenite::tungstenite::protocol::Role::Server,
            None,
        )
        .await;

        handle_websocket_connection(ws_stream, push_manager, client_key, subscription).await;
    });

    Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header("Upgrade", "websocket")
        .header("Connection", "Upgrade")
        .header("Sec-WebSocket-Accept", accept_key)
        .body(BoxBody::default())
        .unwrap()
}

pub(crate) fn generate_accept_key(key: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(key.as_bytes());
    hasher.update(WS_GUID.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(hasher.finalize())
}

fn parse_subscription_from_query(query: &str) -> (String, ClientSubscription) {
    let mut subscription = ClientSubscription::default();
    let mut client_key: Option<String> = None;

    for pair in query.split('&') {
        if let Some((key, value)) = pair.split_once('=') {
            let value = urlencoding::decode(value).unwrap_or_default();
            match key {
                "x_client_id" if !value.is_empty() => {
                    client_key = Some(value.to_string());
                }
                "last_traffic_id" if !value.is_empty() => {
                    subscription.last_traffic_id = Some(value.to_string());
                }
                "last_sequence" => {
                    if let Ok(sequence) = value.parse() {
                        subscription.last_sequence = Some(sequence);
                    }
                }
                "pending_ids" if !value.is_empty() => {
                    subscription.pending_ids = value.split(',').map(|s| s.to_string()).collect();
                }
                "need_traffic" => {
                    subscription.need_traffic = value == "true" || value == "1";
                }
                "need_overview" => {
                    subscription.need_overview = value == "true" || value == "1";
                }
                "need_metrics" => {
                    subscription.need_metrics = value == "true" || value == "1";
                }
                "need_history" => {
                    subscription.need_history = value == "true" || value == "1";
                }
                "need_values" => {
                    subscription.need_values = value == "true" || value == "1";
                }
                "need_scripts" => {
                    subscription.need_scripts = value == "true" || value == "1";
                }
                "need_replay_saved_requests" => {
                    subscription.need_replay_saved_requests = value == "true" || value == "1";
                }
                "need_replay_groups" => {
                    subscription.need_replay_groups = value == "true" || value == "1";
                }
                "settings_scopes" if !value.is_empty() => {
                    subscription.settings_scopes =
                        value.split(',').map(|s| s.to_string()).collect();
                }
                "history_limit" => {
                    if let Ok(limit) = value.parse() {
                        subscription.history_limit = limit;
                    }
                }
                "metrics_interval_ms" => {
                    if let Ok(v) = value.parse() {
                        subscription.metrics_interval_ms = v;
                    }
                }
                _ => {}
            }
        }
    }

    let mut client_key = client_key.unwrap_or_else(|| "unknown".to_string());
    if client_key.chars().count() > MAX_ID_LEN {
        client_key = truncate_chars(&client_key, MAX_ID_LEN);
    }
    (client_key, sanitize_subscription(subscription))
}

async fn handle_websocket_connection<S>(
    ws_stream: WebSocketStream<S>,
    push_manager: SharedPushManager,
    client_key: String,
    subscription: ClientSubscription,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut ws_sender, mut ws_receiver) = ws_stream.split();

    let (client, mut msg_receiver) = push_manager.register_client(client_key, subscription);
    let client_id = client.id;

    let last_pong_ms = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(now_ms()));

    let connected_msg = PushMessage::Connected(ConnectedData {
        client_id,
        message: "WebSocket connection established".to_string(),
    });

    if let Ok(json) = serde_json::to_string(&connected_msg) {
        if let Err(e) = ws_sender.send(Message::Text(json.into())).await {
            error!(
                client_id = client_id,
                "Failed to send connected message: {}", e
            );
            push_manager.unregister_client(client_id);
            return;
        }
    }

    push_manager.send_initial_data(&client).await;

    let push_manager_unregister = push_manager.clone();
    let push_manager_receiver = push_manager.clone();
    let client_clone = client.clone();
    let last_pong_ms_sender = last_pong_ms.clone();

    let sender_task = tokio::spawn(async move {
        let mut ping_interval = tokio::time::interval(WS_PING_INTERVAL);
        loop {
            tokio::select! {
                _ = ping_interval.tick() => {
                    let now = now_ms();
                    let last_pong = last_pong_ms_sender.load(std::sync::atomic::Ordering::Relaxed);
                    if now.saturating_sub(last_pong) > WS_PONG_TIMEOUT.as_millis() as u64 {
                        let _ = ws_sender.send(Message::Close(None)).await;
                        break;
                    }
                    if let Err(e) = ws_sender.send(Message::Ping(bytes::Bytes::new())).await {
                        warn!(client_id = client_id, "Failed to send ping: {}", e);
                        break;
                    }
                }
                maybe_msg = msg_receiver.recv() => {
                    let Some(msg) = maybe_msg else {
                        break;
                    };
                    if let PushMessage::Disconnect(_) = msg {
                        if let Ok(json) = serde_json::to_string(&msg) {
                            let _ = ws_sender.send(Message::Text(json.into())).await;
                        }
                        let _ = ws_sender.send(Message::Close(None)).await;
                        break;
                    }
                    match serde_json::to_string(&msg) {
                        Ok(json) => {
                            if let Err(e) = ws_sender.send(Message::Text(json.into())).await {
                                warn!(client_id = client_id, "Failed to send message: {}", e);
                                break;
                            }
                        }
                        Err(e) => {
                            error!(client_id = client_id, "Failed to serialize message: {}", e);
                        }
                    }
                }
            }
        }
    });

    let last_pong_ms_receiver = last_pong_ms.clone();
    let receiver_task = tokio::spawn(async move {
        while let Some(result) = ws_receiver.next().await {
            match result {
                Ok(msg) => match msg {
                    Message::Text(text) => {
                        if text.len() > WS_TEXT_MAX_BYTES {
                            break;
                        }
                        if let Ok(subscription) = serde_json::from_str::<ClientSubscription>(&text)
                        {
                            let subscription = sanitize_subscription(subscription);
                            let previous = client_clone.get_subscription();
                            let needs_initial_traffic =
                                subscription.need_traffic && !previous.need_traffic;
                            let new_settings_scopes: Vec<String> = subscription
                                .settings_scopes
                                .iter()
                                .filter(|scope| !previous.settings_scopes.contains(*scope))
                                .cloned()
                                .collect();
                            client_clone.update_subscription(subscription);
                            if needs_initial_traffic {
                                push_manager_receiver.send_initial_traffic(&client_clone);
                            }
                            for scope in new_settings_scopes {
                                push_manager_receiver
                                    .send_settings_scope_to_client(&client_clone, &scope)
                                    .await;
                            }
                        }
                        last_pong_ms_receiver.store(now_ms(), std::sync::atomic::Ordering::Relaxed);
                    }
                    Message::Ping(_) => {
                        debug!(client_id = client_id, "Received ping");
                        last_pong_ms_receiver.store(now_ms(), std::sync::atomic::Ordering::Relaxed);
                    }
                    Message::Pong(_) => {
                        debug!(client_id = client_id, "Received pong");
                        last_pong_ms_receiver.store(now_ms(), std::sync::atomic::Ordering::Relaxed);
                    }
                    Message::Close(_) => {
                        debug!(client_id = client_id, "Client closed connection");
                        break;
                    }
                    _ => {}
                },
                Err(e) => {
                    warn!(client_id = client_id, "WebSocket error: {}", e);
                    break;
                }
            }
        }
    });

    let mut sender_task = sender_task;
    let mut receiver_task = receiver_task;
    tokio::select! {
        _ = &mut sender_task => {
            receiver_task.abort();
            let _ = receiver_task.await;
        }
        _ = &mut receiver_task => {
            sender_task.abort();
            let _ = sender_task.await;
        }
    };

    push_manager_unregister.unregister_client(client_id);
    debug!(client_id = client_id, "WebSocket connection closed");
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn sanitize_subscription(mut sub: ClientSubscription) -> ClientSubscription {
    if sub.history_limit > HISTORY_LIMIT_MAX {
        sub.history_limit = HISTORY_LIMIT_MAX;
    }
    sub.metrics_interval_ms = sub
        .metrics_interval_ms
        .clamp(METRICS_INTERVAL_MIN_MS, METRICS_INTERVAL_MAX_MS);
    if sub.pending_ids.len() > MAX_SUBSCRIBED_IDS {
        sub.pending_ids.truncate(MAX_SUBSCRIBED_IDS);
    }
    sub.pending_ids
        .retain(|id| !id.is_empty() && id.len() <= MAX_ID_LEN);
    if sub.settings_scopes.len() > MAX_SETTINGS_SCOPES {
        sub.settings_scopes.truncate(MAX_SETTINGS_SCOPES);
    }
    sub.settings_scopes.retain(|scope| !scope.is_empty());
    sub.settings_scopes.sort();
    sub.settings_scopes.dedup();
    sub
}
