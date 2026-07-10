use std::sync::Arc;

use bifrost_admin::{AdminState, FrameDirection, FrameType, TrafficType};
use bifrost_core::{BifrostError, Result};
use futures_util::StreamExt;
use hyper::upgrade::Upgraded;
use hyper_util::rt::TokioIo;
use tracing::{debug, trace};

use crate::protocol::{WebSocketReader, WebSocketWriter};
use crate::utils::logging::RequestContext;

use super::super::ws_decode::{decode_ws_payload_for_storage, WsHandshakeMeta};

#[allow(clippy::too_many_arguments)]
pub async fn websocket_bidirectional_generic_with_capture<S>(
    upgraded: Upgraded,
    target: S,
    record_id: &str,
    admin_state: Option<Arc<AdminState>>,
    compression_cfg: Option<crate::protocol::PerMessageDeflateConfig>,
    upstream_leftover: bytes::BytesMut,
    ctx: RequestContext,
    resolved_rules: crate::server::ResolvedRules,
    request_url: String,
    request_method: String,
    request_headers: Vec<(String, String)>,
    ws_meta: WsHandshakeMeta,
    decode_scripts: Vec<String>,
) -> Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let client = TokioIo::new(upgraded);
    let (target_read, target_write) = tokio::io::split(target);
    let (client_read, client_write) = tokio::io::split(client);

    let record_id_owned = record_id.to_string();
    let admin_state_c2s = admin_state.clone();
    let admin_state_s2c = admin_state.clone();
    let ctx_c2s = ctx.clone();
    let ctx_s2c = ctx;
    let rules_c2s = resolved_rules.clone();
    let rules_s2c = resolved_rules;
    let url_c2s = request_url.clone();
    let url_s2c = request_url;
    let method_c2s = request_method.clone();
    let method_s2c = request_method;
    let headers_c2s = request_headers.clone();
    let headers_s2c = request_headers;
    let ws_meta_c2s = ws_meta.clone();
    let ws_meta_s2c = ws_meta;
    let scripts_c2s = decode_scripts.clone();
    let scripts_s2c = decode_scripts;

    let compression_cfg_c2s = compression_cfg.clone();
    let compression_cfg_s2c = compression_cfg;

    let client_to_server = async move {
        let mut reader = WebSocketReader::new(client_read);
        let mut writer = WebSocketWriter::new(target_write, true);
        let mut inflater = compression_cfg_c2s
            .as_ref()
            .map(|_| crate::protocol::PerMessageDeflateInflater::new());
        let takeover = compression_cfg_c2s
            .as_ref()
            .map(|cfg| !cfg.client_no_context_takeover)
            .unwrap_or(false);

        while let Some(result) = reader.next().await {
            let frame = match result {
                Ok(f) => f,
                Err(e) => {
                    trace!("Client read error: {}", e);
                    break;
                }
            };

            if let Some(ref state) = admin_state_c2s {
                state
                    .metrics_collector
                    .add_bytes_sent_by_type(TrafficType::Ws, frame.payload.len() as u64);

                let frame_type = super::opcode_to_frame_type(frame.opcode);
                let payload_for_record = if frame.is_compressed() {
                    if let Some(inflater) = inflater.as_mut() {
                        if !takeover {
                            inflater.reset();
                        }
                        match inflater.decompress_message(frame.payload.as_ref()) {
                            Ok(bytes) => bytes,
                            Err(e) => {
                                trace!("[WS] decompress failed (c2s): {}", e);
                                frame.payload.clone()
                            }
                        }
                    } else {
                        frame.payload.clone()
                    }
                } else {
                    frame.payload.clone()
                };

                let decoded = if matches!(
                    frame_type,
                    FrameType::Text | FrameType::Binary | FrameType::Continuation
                ) {
                    decode_ws_payload_for_storage(
                        &admin_state_c2s,
                        &scripts_c2s,
                        &ctx_c2s,
                        &rules_c2s,
                        &url_c2s,
                        &method_c2s,
                        &headers_c2s,
                        &ws_meta_c2s,
                        FrameDirection::Send,
                        frame_type,
                        payload_for_record.as_ref(),
                    )
                    .await
                } else {
                    None
                };

                let payload_is_text = decoded.is_some()
                    || matches!(
                        frame_type,
                        FrameType::Text | FrameType::Close | FrameType::Sse
                    );

                if !state.get_super_performance_mode() {
                    state.connection_monitor.record_frame(
                        &record_id_owned,
                        FrameDirection::Send,
                        frame_type,
                        decoded
                            .as_deref()
                            .unwrap_or_else(|| payload_for_record.as_ref()),
                        payload_is_text,
                        decoded.as_ref().map(|_| payload_for_record.as_ref()),
                        frame.mask.is_some(),
                        frame.fin,
                        state.body_store.as_ref(),
                        state.ws_payload_store.as_ref(),
                        state.frame_store.as_ref(),
                    );
                }

                if frame.opcode == crate::protocol::Opcode::Close {
                    let close_code = frame.close_code();
                    let close_reason = frame.close_reason().map(str::to_string);
                    let frame_store = (!state.get_super_performance_mode())
                        .then_some(state.frame_store.as_ref())
                        .flatten();
                    let ws_payload_store = (!state.get_super_performance_mode())
                        .then_some(state.ws_payload_store.as_ref())
                        .flatten();
                    state.connection_monitor.set_connection_closed(
                        &record_id_owned,
                        close_code,
                        close_reason,
                        frame_store,
                        ws_payload_store,
                    );
                    super::persist_socket_summary(state, &record_id_owned);
                }
            }

            if let Err(e) = writer.write_frame(frame).await {
                trace!("Server write error: {}", e);
                break;
            }
        }

        Ok::<_, std::io::Error>(())
    };

    let record_id_owned2 = record_id.to_string();
    let server_to_client = async move {
        let mut reader = WebSocketReader::with_initial_buffer(target_read, upstream_leftover);
        let mut writer = WebSocketWriter::new(client_write, false);
        let mut inflater = compression_cfg_s2c
            .as_ref()
            .map(|_| crate::protocol::PerMessageDeflateInflater::new());
        let takeover = compression_cfg_s2c
            .as_ref()
            .map(|cfg| !cfg.server_no_context_takeover)
            .unwrap_or(false);

        while let Some(result) = reader.next().await {
            let frame = match result {
                Ok(f) => f,
                Err(e) => {
                    trace!("Server read error: {}", e);
                    break;
                }
            };

            if let Some(ref state) = admin_state_s2c {
                state
                    .metrics_collector
                    .add_bytes_received_by_type(TrafficType::Ws, frame.payload.len() as u64);

                let frame_type = super::opcode_to_frame_type(frame.opcode);
                let payload_for_record = if frame.is_compressed() {
                    if let Some(inflater) = inflater.as_mut() {
                        if !takeover {
                            inflater.reset();
                        }
                        match inflater.decompress_message(frame.payload.as_ref()) {
                            Ok(bytes) => bytes,
                            Err(e) => {
                                trace!("[WS] decompress failed (s2c): {}", e);
                                frame.payload.clone()
                            }
                        }
                    } else {
                        frame.payload.clone()
                    }
                } else {
                    frame.payload.clone()
                };

                let decoded = if matches!(
                    frame_type,
                    FrameType::Text | FrameType::Binary | FrameType::Continuation
                ) {
                    decode_ws_payload_for_storage(
                        &admin_state_s2c,
                        &scripts_s2c,
                        &ctx_s2c,
                        &rules_s2c,
                        &url_s2c,
                        &method_s2c,
                        &headers_s2c,
                        &ws_meta_s2c,
                        FrameDirection::Receive,
                        frame_type,
                        payload_for_record.as_ref(),
                    )
                    .await
                } else {
                    None
                };

                let payload_is_text = decoded.is_some()
                    || matches!(
                        frame_type,
                        FrameType::Text | FrameType::Close | FrameType::Sse
                    );

                if !state.get_super_performance_mode() {
                    state.connection_monitor.record_frame(
                        &record_id_owned2,
                        FrameDirection::Receive,
                        frame_type,
                        decoded
                            .as_deref()
                            .unwrap_or_else(|| payload_for_record.as_ref()),
                        payload_is_text,
                        decoded.as_ref().map(|_| payload_for_record.as_ref()),
                        frame.mask.is_some(),
                        frame.fin,
                        state.body_store.as_ref(),
                        state.ws_payload_store.as_ref(),
                        state.frame_store.as_ref(),
                    );
                }

                if frame.opcode == crate::protocol::Opcode::Close {
                    let close_code = frame.close_code();
                    let close_reason = frame.close_reason().map(str::to_string);
                    let frame_store = (!state.get_super_performance_mode())
                        .then_some(state.frame_store.as_ref())
                        .flatten();
                    let ws_payload_store = (!state.get_super_performance_mode())
                        .then_some(state.ws_payload_store.as_ref())
                        .flatten();
                    state.connection_monitor.set_connection_closed(
                        &record_id_owned2,
                        close_code,
                        close_reason,
                        frame_store,
                        ws_payload_store,
                    );
                    super::persist_socket_summary(state, &record_id_owned2);
                }
            }

            if let Err(e) = writer.write_frame(frame).await {
                trace!("Client write error: {}", e);
                break;
            }
        }

        Ok::<_, std::io::Error>(())
    };

    let result = tokio::try_join!(client_to_server, server_to_client);

    if let Some(ref state) = admin_state {
        let should_close = state
            .connection_monitor
            .get_connection_status(record_id)
            .map(|s| s.is_open)
            .unwrap_or(false);
        if should_close {
            let frame_store = (!state.get_super_performance_mode())
                .then_some(state.frame_store.as_ref())
                .flatten();
            let ws_payload_store = (!state.get_super_performance_mode())
                .then_some(state.ws_payload_store.as_ref())
                .flatten();
            state.connection_monitor.set_connection_closed(
                record_id,
                None,
                None,
                frame_store,
                ws_payload_store,
            );
        }
        super::persist_socket_summary(state, record_id);
    }

    match result {
        Ok(_) => {
            debug!("WebSocket connection closed normally");
            Ok(())
        }
        Err(e) => {
            if e.kind() == std::io::ErrorKind::ConnectionReset
                || e.kind() == std::io::ErrorKind::BrokenPipe
            {
                debug!("WebSocket connection closed: {}", e);
                Ok(())
            } else {
                Err(BifrostError::Network(format!("WebSocket error: {}", e)))
            }
        }
    }
}
