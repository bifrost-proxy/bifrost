use std::sync::Arc;
use std::time::Instant;

use bifrost_admin::{AdminState, RequestTiming, TrafficRecord, TrafficType};
use bifrost_core::{BifrostError, Result};
use hyper::body::Incoming;
use hyper::{Request, Response};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tracing::{debug, error};

use super::super::ws_handshake::{
    header_values, negotiate_extensions, negotiate_protocol, read_http1_response_with_leftover,
};
use crate::server::{empty_body, BoxBody, RulesResolver};
use crate::utils::logging::RequestContext;
use crate::utils::process_info::resolve_client_process_async_for_connection;

pub async fn handle_websocket_upgrade(
    req: Request<Incoming>,
    rules: Arc<dyn RulesResolver>,
    admin_state: Option<Arc<AdminState>>,
    peer_addr: std::net::SocketAddr,
    local_addr: std::net::SocketAddr,
) -> Result<Response<BoxBody>> {
    let ctx = RequestContext::new().with_port(local_addr.port());
    let start_time = Instant::now();
    let uri = req.uri().clone();
    let url = uri.to_string();
    let method = req.method().to_string();

    let resolved_rules = rules.resolve(&url, "GET");
    let has_rules = !resolved_rules.rules.is_empty() || resolved_rules.host.is_some();

    let req_headers: Vec<(String, String)> = crate::proxy::http::headers_to_pairs(req.headers());

    let host = req
        .headers()
        .get(hyper::header::HOST)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| BifrostError::Network("Missing Host header".to_string()))?;

    let (target_host, target_port) = if let Some(ref host_rule) = resolved_rules.host {
        super::parse_host_port(host_rule)?
    } else {
        super::parse_host_port(host)?
    };

    debug!("WebSocket upgrade to {}:{}", target_host, target_port);

    let connect_start = Instant::now();
    let mut target_stream = TcpStream::connect(format!("{}:{}", target_host, target_port))
        .await
        .map_err(|e| {
            BifrostError::Network(format!(
                "Failed to connect to {}:{}: {}",
                target_host, target_port, e
            ))
        })?;
    let tcp_connect_ms = connect_start.elapsed().as_millis() as u64;

    if let Err(e) = target_stream.set_nodelay(true) {
        debug!("Failed to set TCP_NODELAY on WebSocket connection: {}", e);
    }

    let upgrade_request = build_websocket_handshake(&req)?;
    target_stream
        .write_all(upgrade_request.as_bytes())
        .await
        .map_err(|e| BifrostError::Network(format!("Failed to send handshake: {}", e)))?;

    let websocket_handshake_max_header_size = if let Some(ref state) = admin_state {
        if let Some(ref config_manager) = state.config_manager {
            config_manager
                .config()
                .await
                .server
                .websocket_handshake_max_header_size
        } else {
            64 * 1024
        }
    } else {
        64 * 1024
    };

    let (upstream_resp, upstream_leftover) =
        read_http1_response_with_leftover(&mut target_stream, websocket_handshake_max_header_size)
            .await?;
    if upstream_resp.status_code != 101 {
        return Err(BifrostError::Network(format!(
            "WebSocket handshake failed: {} {}",
            upstream_resp.status_code, upstream_resp.status_text
        )));
    }

    let sec_accept = upstream_resp
        .header("Sec-WebSocket-Accept")
        .map(|v| v.to_string());
    let upstream_protocol = upstream_resp.header("Sec-WebSocket-Protocol");
    let upstream_extensions = header_values(&upstream_resp, "Sec-WebSocket-Extensions");

    let client_protocol = req
        .headers()
        .get("Sec-WebSocket-Protocol")
        .and_then(|v| v.to_str().ok());
    let client_extensions = req
        .headers()
        .get("Sec-WebSocket-Extensions")
        .and_then(|v| v.to_str().ok());

    let negotiated_protocol = negotiate_protocol(client_protocol, upstream_protocol);
    let negotiated_extensions = negotiate_extensions(client_extensions, &upstream_extensions);
    let compression_cfg = negotiated_extensions
        .as_deref()
        .and_then(crate::protocol::parse_permessage_deflate_config);
    let compression_enabled = compression_cfg.is_some();
    let ws_meta = super::super::ws_decode::WsHandshakeMeta {
        negotiated_protocol: negotiated_protocol.clone(),
        negotiated_extensions: negotiated_extensions.clone(),
    };
    let upstream_headers = upstream_resp.headers.clone();

    let total_ms = start_time.elapsed().as_millis() as u64;
    let record_id = ctx.id_str().to_string();

    if compression_enabled {
        debug!(
            "[WS] permessage-deflate compression enabled for {}",
            record_id
        );
    }

    if let Some(ref state) = admin_state {
        state
            .metrics_collector
            .increment_requests_by_type(TrafficType::Ws);

        let ws_url = format!(
            "ws://{}{}",
            host,
            uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/")
        );

        let client_process =
            resolve_client_process_async_for_connection(&peer_addr, &local_addr).await;
        let (client_app, client_pid, client_path) = client_process
            .as_ref()
            .map(|p| (Some(p.name.clone()), Some(p.pid), p.path.clone()))
            .unwrap_or((None, None, None));

        let mut record = TrafficRecord::new(record_id.to_string(), method, ws_url);
        record.status = 101;
        record.protocol = "ws".to_string();
        record.duration_ms = total_ms;
        record.timing = Some(RequestTiming {
            dns_ms: None,
            connect_ms: Some(tcp_connect_ms),
            tls_ms: None,
            send_ms: None,
            wait_ms: Some(total_ms.saturating_sub(tcp_connect_ms)),
            first_byte_ms: Some(total_ms),
            receive_ms: None,
            total_ms,
        });
        record.request_headers = Some(req_headers.clone());
        record.has_rule_hit = has_rules;
        record.matched_rules = crate::utils::build_matched_rules(&resolved_rules);
        record.client_ip = peer_addr.ip().to_string();
        record.client_app = client_app;
        record.client_pid = client_pid;
        record.client_path = client_path;
        record.listener_port = ctx.port;
        record.set_websocket();

        state.connection_monitor.register_connection(&record_id);
        state.record_traffic(record);
    }

    let record_id_clone = record_id.clone();
    let ws_ctx = ctx.clone();
    let ws_rules = resolved_rules.clone();
    let ws_req_url = format!(
        "ws://{}{}",
        host,
        uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/")
    );
    let ws_req_method = "GET".to_string();
    let ws_req_headers = req_headers.clone();
    let ws_decode_scripts = ws_rules.decode_scripts.clone();
    let ws_compression_cfg = compression_cfg.clone();
    tokio::spawn(async move {
        match hyper::upgrade::on(req).await {
            Ok(upgraded) => {
                if let Err(e) = super::websocket_bidirectional_generic_with_capture(
                    upgraded,
                    target_stream,
                    &record_id_clone,
                    admin_state.clone(),
                    ws_compression_cfg,
                    upstream_leftover,
                    ws_ctx,
                    ws_rules,
                    ws_req_url,
                    ws_req_method,
                    ws_req_headers,
                    ws_meta,
                    ws_decode_scripts,
                )
                .await
                {
                    error!("WebSocket tunnel error: {}", e);
                }

                if let Some(ref state) = admin_state {
                    let frame_store = (!state.get_super_performance_mode())
                        .then_some(state.frame_store.as_ref())
                        .flatten();
                    let ws_payload_store = (!state.get_super_performance_mode())
                        .then_some(state.ws_payload_store.as_ref())
                        .flatten();
                    state.connection_monitor.set_connection_closed(
                        &record_id_clone,
                        None,
                        None,
                        frame_store,
                        ws_payload_store,
                    );
                }
            }
            Err(e) => {
                error!("WebSocket upgrade error: {}", e);
            }
        }
    });

    let mut response = Response::builder()
        .status(101)
        .header(hyper::header::UPGRADE, "websocket")
        .header(hyper::header::CONNECTION, "Upgrade");

    if let Some(accept) = sec_accept {
        response = response.header("Sec-WebSocket-Accept", accept);
    }

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

    Ok(response.body(empty_body()).unwrap())
}

fn build_websocket_handshake(req: &Request<Incoming>) -> Result<String> {
    let path = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");

    let host = req
        .headers()
        .get(hyper::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");

    let ws_key = req
        .headers()
        .get("Sec-WebSocket-Key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let ws_version = req
        .headers()
        .get("Sec-WebSocket-Version")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("13");

    let mut handshake = format!(
        "GET {} HTTP/1.1\r\n\
         Host: {}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: {}\r\n\
         Sec-WebSocket-Version: {}\r\n",
        path, host, ws_key, ws_version
    );

    for (name, value) in req.headers().iter() {
        let n = name.as_str();
        if n.eq_ignore_ascii_case("host")
            || n.eq_ignore_ascii_case("upgrade")
            || n.eq_ignore_ascii_case("connection")
            || n.eq_ignore_ascii_case("sec-websocket-key")
            || n.eq_ignore_ascii_case("sec-websocket-version")
            || n.eq_ignore_ascii_case("sec-websocket-protocol")
            || n.eq_ignore_ascii_case("sec-websocket-extensions")
            || n.eq_ignore_ascii_case("content-length")
            || n.eq_ignore_ascii_case("transfer-encoding")
            || n.eq_ignore_ascii_case("proxy-connection")
            || n.eq_ignore_ascii_case("keep-alive")
            || n.eq_ignore_ascii_case("te")
            || n.eq_ignore_ascii_case("trailer")
        {
            continue;
        }

        if let Ok(v) = value.to_str() {
            handshake.push_str(&format!("{}: {}\r\n", n, v));
        }
    }

    if let Some(protocol) = req
        .headers()
        .get("Sec-WebSocket-Protocol")
        .and_then(|v| v.to_str().ok())
    {
        handshake.push_str(&format!("Sec-WebSocket-Protocol: {}\r\n", protocol));
    }

    if let Some(extensions) = req
        .headers()
        .get("Sec-WebSocket-Extensions")
        .and_then(|v| v.to_str().ok())
    {
        handshake.push_str(&format!("Sec-WebSocket-Extensions: {}\r\n", extensions));
    }

    handshake.push_str("\r\n");
    Ok(handshake)
}
