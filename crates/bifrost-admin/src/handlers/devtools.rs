use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use hyper::{body::Incoming, header, Method, Request, Response, StatusCode};
use serde::Deserialize;
use sha1::{Digest, Sha1};
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::WebSocketStream;
use tracing::{error, warn};

use crate::devtools::{
    BridgeClosePayload, BridgeConsolePayload, BridgeEvalPollPayload, BridgeEvalResultPayload,
    BridgeHelloPayload, BridgeNetworkPayload, BridgeOverlayCommand, DebugAdapterKind, DebugPage,
    DevtoolsMode, SharedBrowserDebugBroker,
};
use crate::state::SharedAdminState;
use crate::{is_remote_access_enabled, validate_admin_jwt};

use super::{
    auth::extract_bearer_token, error_response, full_body, json_response, method_not_allowed,
    BoxBody,
};

const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

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
            Method::GET => json_response(&state.devtools_broker.cdp_targets(true, &host)),
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
                json_response(&state.devtools_broker.list_evaluate_audit(limit, since))
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
                json_response(&serde_json::json!({
                    "pages": state.devtools_broker.list_debuggable_pages(online_only)
                }))
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
                match state.devtools_broker.open_session(&page_id) {
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
            (Method::GET, "snapshot") => match state.devtools_broker.snapshot(session_id) {
                Ok(snapshot) => json_response(&snapshot),
                Err(err) => error_response(StatusCode::NOT_FOUND, &err),
            },
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
            (Method::POST, "hello") => {
                let payload = match read_json::<BridgeHelloPayload>(req).await {
                    Ok(body) => body,
                    Err(resp) => return resp,
                };
                match state.devtools_broker.bridge_hello(page_id, payload) {
                    Ok(()) => json_response(&serde_json::json!({"ok": true})),
                    Err(err) => error_response(StatusCode::FORBIDDEN, &err),
                }
            }
            (Method::POST, "console") => {
                let payload = match read_json::<BridgeConsolePayload>(req).await {
                    Ok(body) => body,
                    Err(resp) => return resp,
                };
                match state.devtools_broker.bridge_console(page_id, payload) {
                    Ok(()) => json_response(&serde_json::json!({"ok": true})),
                    Err(err) => error_response(StatusCode::FORBIDDEN, &err),
                }
            }
            (Method::POST, "close") => {
                let payload = match read_json::<BridgeClosePayload>(req).await {
                    Ok(body) => body,
                    Err(resp) => return resp,
                };
                match state.devtools_broker.bridge_close(page_id, payload) {
                    Ok(()) => json_response(&serde_json::json!({"ok": true})),
                    Err(err) => error_response(StatusCode::FORBIDDEN, &err),
                }
            }
            (Method::POST, "network") => {
                let payload = match read_json::<BridgeNetworkPayload>(req).await {
                    Ok(body) => body,
                    Err(resp) => return resp,
                };
                match state.devtools_broker.bridge_network(page_id, payload) {
                    Ok(()) => json_response(&serde_json::json!({"ok": true})),
                    Err(err) => error_response(StatusCode::FORBIDDEN, &err),
                }
            }
            (Method::POST, "eval-next") => {
                let payload = match read_json::<BridgeEvalPollPayload>(req).await {
                    Ok(body) => body,
                    Err(resp) => return resp,
                };
                match state.devtools_broker.bridge_eval_next(page_id, payload) {
                    Ok(command) => json_response(&serde_json::json!({ "command": command })),
                    Err(err) => error_response(StatusCode::FORBIDDEN, &err),
                }
            }
            (Method::POST, "eval-result") => {
                let payload = match read_json::<BridgeEvalResultPayload>(req).await {
                    Ok(body) => body,
                    Err(resp) => return resp,
                };
                match state.devtools_broker.bridge_eval_result(page_id, payload) {
                    Ok(()) => json_response(&serde_json::json!({"ok": true})),
                    Err(err) => error_response(StatusCode::FORBIDDEN, &err),
                }
            }
            (Method::POST, "overlay-next") => {
                let payload = match read_json::<BridgeEvalPollPayload>(req).await {
                    Ok(body) => body,
                    Err(resp) => return resp,
                };
                match state.devtools_broker.bridge_overlay_next(page_id, payload) {
                    Ok(command) => json_response(&serde_json::json!({ "command": command })),
                    Err(err) => error_response(StatusCode::FORBIDDEN, &err),
                }
            }
            _ => method_not_allowed(),
        };
    }

    error_response(StatusCode::NOT_FOUND, "DevTools endpoint not found")
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

    let Some(page) = state.devtools_broker.get_page(&page_id) else {
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
