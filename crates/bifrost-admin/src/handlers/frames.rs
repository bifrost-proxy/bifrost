use std::sync::Arc;

use bytes::Bytes;
use hyper::{Response, StatusCode};
use serde::{Deserialize, Serialize};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use super::{error_response, full_body, json_response, BoxBody};
use crate::body_store::BodyRef;
use crate::connection_monitor::WebSocketFrameRecord;
use crate::state::AdminState;
use crate::traffic::{FrameType, SocketStatus, TrafficRecord};

#[derive(Debug, Serialize)]
struct FramesResponse {
    frames: Vec<WebSocketFrameRecord>,
    socket_status: Option<SocketStatus>,
    last_frame_id: u64,
    has_more: bool,
    is_monitored: bool,
}

#[derive(Debug, Deserialize)]
pub struct FramesQuery {
    #[serde(default)]
    pub after: Option<u64>,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

impl Default for FramesQuery {
    fn default() -> Self {
        Self {
            after: None,
            limit: 100,
        }
    }
}

fn default_limit() -> usize {
    100
}

fn parse_frames_query(query: Option<&str>) -> FramesQuery {
    query
        .and_then(|q| serde_urlencoded::from_str(q).ok())
        .unwrap_or_default()
}

async fn get_traffic_record(state: Arc<AdminState>, id: &str) -> Option<TrafficRecord> {
    if let Some(ref db_store) = state.traffic_db_store {
        let db_clone = db_store.clone();
        let id_owned = id.to_string();
        tokio::task::spawn_blocking(move || db_clone.get_by_id(&id_owned))
            .await
            .unwrap_or_default()
    } else {
        None
    }
}

pub async fn get_frames(
    state: Arc<AdminState>,
    connection_id: &str,
    query_str: Option<&str>,
) -> Response<BoxBody> {
    tracing::debug!(
        "[FRAMES API] get_frames called for connection_id: {}",
        connection_id
    );
    let query = parse_frames_query(query_str);
    let monitor = &state.connection_monitor;

    let conn_id = connection_id.to_string();
    let frame_store = state.frame_store.clone();
    let after = query.after;
    let limit = query.limit;

    if let Some(fs) = frame_store.as_ref() {
        fs.flush();
    }

    let (file_frames, file_has_more, pending_frames) = if let Some(fs) = frame_store.as_ref() {
        let fs = fs.clone();
        let fs_for_file = fs.clone();
        let conn_id_clone = conn_id.clone();
        let (file_frames, file_has_more) = match tokio::task::spawn_blocking(move || {
            fs_for_file.load_frames(&conn_id_clone, after, limit)
        })
        .await
        {
            Ok(Ok((frames, has_more))) => (frames, has_more),
            Ok(Err(e)) => {
                tracing::warn!("[FRAMES API] Failed to load frames from file: {}", e);
                (Vec::new(), false)
            }
            Err(e) => {
                tracing::warn!("[FRAMES API] spawn_blocking failed: {}", e);
                (Vec::new(), false)
            }
        };
        let pending_frames = fs.load_pending_frames(&conn_id, after, limit);
        (file_frames, file_has_more, pending_frames)
    } else {
        (Vec::new(), false, Vec::new())
    };

    let (mem_frames, is_active) = {
        let connections = monitor.connections.read();
        if let Some(store) = connections.get(&conn_id) {
            let frames: Vec<_> = if let Some(after_id) = after {
                store
                    .frames
                    .iter()
                    .filter(|f| f.frame_id > after_id)
                    .cloned()
                    .collect()
            } else {
                store.frames.iter().cloned().collect()
            };
            (frames, true)
        } else {
            (Vec::new(), false)
        }
    };

    use std::collections::HashSet;
    let mut seen_ids: HashSet<u64> = HashSet::new();
    let mut all_frames = Vec::new();

    for frame in file_frames {
        if !seen_ids.contains(&frame.frame_id) {
            seen_ids.insert(frame.frame_id);
            all_frames.push(frame);
        }
    }

    for frame in pending_frames {
        if !seen_ids.contains(&frame.frame_id) {
            seen_ids.insert(frame.frame_id);
            all_frames.push(frame);
        }
    }

    for frame in mem_frames {
        if !seen_ids.contains(&frame.frame_id) {
            seen_ids.insert(frame.frame_id);
            all_frames.push(frame);
        }
    }

    all_frames.sort_by_key(|f| f.frame_id);
    let has_more = file_has_more || all_frames.len() > limit;
    let frames: Vec<_> = all_frames.into_iter().take(limit).collect();

    let has_data = !frames.is_empty()
        || is_active
        || monitor.has_persisted_frames(&conn_id, state.frame_store.as_ref());

    if has_data {
        let socket_status = if let Some(status) = monitor.get_status(&conn_id) {
            Some(status)
        } else {
            get_traffic_record(state.clone(), connection_id)
                .await
                .and_then(|record| {
                    if record.is_sse {
                        state
                            .sse_hub
                            .get_socket_status(&conn_id)
                            .or(record.socket_status)
                    } else {
                        record.socket_status
                    }
                })
        };
        let last_frame_id = frames
            .last()
            .map(|f| f.frame_id)
            .or_else(|| monitor.get_last_frame_id(&conn_id))
            .or_else(|| {
                state
                    .frame_store
                    .as_ref()
                    .and_then(|fs| fs.get_last_frame_id(&conn_id))
            })
            .unwrap_or(0);
        let is_monitored = monitor.is_monitored(&conn_id);

        let response = FramesResponse {
            frames,
            socket_status,
            last_frame_id,
            has_more,
            is_monitored,
        };

        json_response(&response)
    } else {
        if let Some(record) = get_traffic_record(state.clone(), connection_id).await {
            let socket_status = if record.is_sse {
                state
                    .sse_hub
                    .get_socket_status(&conn_id)
                    .or(record.socket_status)
            } else {
                record.socket_status
            };
            if socket_status.is_some() {
                let response = FramesResponse {
                    frames: Vec::new(),
                    socket_status,
                    last_frame_id: record.last_frame_id,
                    has_more: false,
                    is_monitored: false,
                };
                return json_response(&response);
            }
        }

        error_response(
            StatusCode::NOT_FOUND,
            &format!("Connection {} not found", connection_id),
        )
    }
}

pub async fn subscribe_frames(state: Arc<AdminState>, connection_id: &str) -> Response<BoxBody> {
    let monitor = &state.connection_monitor;

    if !monitor.start_monitoring(connection_id) {
        return error_response(
            StatusCode::NOT_FOUND,
            &format!("Connection {} not found", connection_id),
        );
    }

    let receiver = match monitor.subscribe_connection(connection_id) {
        Some(rx) => rx,
        None => {
            return error_response(
                StatusCode::NOT_FOUND,
                &format!("Connection {} not found", connection_id),
            );
        }
    };

    let connection_id_owned = connection_id.to_string();
    let stream = BroadcastStream::new(receiver).filter_map(move |result| match result {
        Ok(event) if event.connection_id == connection_id_owned => {
            let data = serde_json::to_string(&event.frame).ok()?;
            let sse_data = format!("data: {}\n\n", data);
            Some(sse_data)
        }
        _ => None,
    });

    let body_stream = http_body_util::StreamBody::new(
        stream.map(|s| Ok::<_, hyper::Error>(hyper::body::Frame::data(Bytes::from(s)))),
    );

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/event-stream")
        .header("Cache-Control", "no-cache")
        .header("Connection", "keep-alive")
        .body(BoxBody::new(body_stream))
        .unwrap()
}

pub async fn unsubscribe_frames(state: Arc<AdminState>, connection_id: &str) -> Response<BoxBody> {
    let monitor = &state.connection_monitor;

    if monitor.stop_monitoring(connection_id) {
        let body = serde_json::json!({
            "success": true,
            "message": format!("Stopped monitoring connection {}", connection_id)
        });
        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(full_body(body.to_string()))
            .unwrap()
    } else {
        error_response(
            StatusCode::NOT_FOUND,
            &format!("Connection {} not found", connection_id),
        )
    }
}

#[derive(Debug, Serialize)]
struct WebSocketConnectionsResponse {
    connections: Vec<WebSocketConnectionInfo>,
    total: usize,
}

#[derive(Debug, Serialize)]
struct WebSocketConnectionInfo {
    id: String,
    frame_count: usize,
    socket_status: Option<SocketStatus>,
    is_monitored: bool,
}

pub async fn list_websocket_connections(state: Arc<AdminState>) -> Response<BoxBody> {
    let monitor = &state.connection_monitor;
    let connection_ids = monitor.active_connection_ids();

    let connections: Vec<WebSocketConnectionInfo> = connection_ids
        .iter()
        .filter_map(|id| {
            Some(WebSocketConnectionInfo {
                id: id.clone(),
                frame_count: monitor.get_frame_count(id)?,
                socket_status: monitor.get_status(id),
                is_monitored: monitor.is_monitored(id),
            })
        })
        .collect();

    let total = connections.len();
    let response = WebSocketConnectionsResponse { connections, total };

    json_response(&response)
}

pub async fn get_frame_detail(
    state: Arc<AdminState>,
    connection_id: &str,
    frame_id: u64,
) -> Response<BoxBody> {
    let monitor = &state.connection_monitor;

    let frame = {
        let connections = monitor.connections.read();
        connections.get(connection_id).and_then(|store| {
            store
                .frames
                .iter()
                .find(|f| f.frame_id == frame_id)
                .cloned()
        })
    }
    .or_else(|| {
        state.frame_store.as_ref().and_then(|fs| {
            fs.flush();
            let pending = fs.load_pending_frames(connection_id, None, usize::MAX);
            pending
                .into_iter()
                .find(|f| f.frame_id == frame_id)
                .or_else(|| fs.load_frame_by_id(connection_id, frame_id).ok().flatten())
        })
    });

    if let Some(frame) = frame {
        let mut full_payload: Option<String> = None;
        let mut raw_full_payload: Option<String> = None;

        if let Some(ref body_ref) = frame.payload_ref {
            if let BodyRef::Inline { data } = body_ref {
                full_payload = Some(data.clone());
            } else if let Some(ref ws_payload_store) = state.ws_payload_store {
                if ws_payload_store.is_ws_payload_ref(body_ref) {
                    let body_ref_clone = body_ref.clone();
                    let store_clone = ws_payload_store.clone();
                    let frame_clone = frame.clone();
                    let data = tokio::task::spawn_blocking(move || {
                        store_clone.read_range(&body_ref_clone)
                    })
                    .await
                    .ok()
                    .flatten();

                    if let Some(payload_bytes) = data {
                        full_payload = Some(if frame_clone.payload_is_text {
                            String::from_utf8_lossy(&payload_bytes).to_string()
                        } else {
                            base64::Engine::encode(
                                &base64::engine::general_purpose::STANDARD,
                                payload_bytes,
                            )
                        });
                    }
                }
            } else if let Some(ref body_store) = state.body_store {
                let body_ref_clone = body_ref.clone();
                let body_store_clone = body_store.clone();

                let data = tokio::task::spawn_blocking(move || {
                    let store = body_store_clone.read();
                    store.load(&body_ref_clone)
                })
                .await
                .ok()
                .flatten();

                if let Some(payload_data) = data {
                    full_payload = Some(payload_data);
                }
            }
        }

        if let Some(ref body_ref) = frame.raw_payload_ref {
            if let BodyRef::Inline { data } = body_ref {
                raw_full_payload = Some(data.clone());
            } else if let Some(ref ws_payload_store) = state.ws_payload_store {
                if ws_payload_store.is_ws_payload_ref(body_ref) {
                    let body_ref_clone = body_ref.clone();
                    let store_clone = ws_payload_store.clone();
                    let frame_clone = frame.clone();
                    let data = tokio::task::spawn_blocking(move || {
                        store_clone.read_range(&body_ref_clone)
                    })
                    .await
                    .ok()
                    .flatten();

                    if let Some(payload_bytes) = data {
                        let raw_is_text = frame_clone.raw_payload_is_text.unwrap_or(false);
                        raw_full_payload = Some(if raw_is_text {
                            String::from_utf8_lossy(&payload_bytes).to_string()
                        } else {
                            base64::Engine::encode(
                                &base64::engine::general_purpose::STANDARD,
                                payload_bytes,
                            )
                        });
                    }
                }
            } else if let Some(ref body_store) = state.body_store {
                let body_ref_clone = body_ref.clone();
                let body_store_clone = body_store.clone();

                let data = tokio::task::spawn_blocking(move || {
                    let store = body_store_clone.read();
                    store.load(&body_ref_clone)
                })
                .await
                .ok()
                .flatten();

                if let Some(payload_data) = data {
                    raw_full_payload = Some(payload_data);
                }
            }
        }

        if full_payload.is_some() || raw_full_payload.is_some() {
            let body = serde_json::json!({
                "frame": frame.clone(),
                "full_payload": full_payload.unwrap_or_default(),
                "raw_full_payload": raw_full_payload,
            });
            return json_response(&body);
        }

        if frame.frame_type == FrameType::Sse {
            let record = if let Some(ref db_store) = state.traffic_db_store {
                let db_store = db_store.clone();
                let id = connection_id.to_string();
                tokio::task::spawn_blocking(move || db_store.get_by_id(&id))
                    .await
                    .ok()
                    .flatten()
            } else {
                None
            };

            if let Some(record) = record {
                if let (Some(body_store), Some(body_ref)) =
                    (state.body_store.clone(), record.response_body_ref.clone())
                {
                    let data =
                        tokio::task::spawn_blocking(move || body_store.read().load(&body_ref))
                            .await
                            .ok()
                            .flatten();

                    if let Some(raw) = data {
                        let mut events = Vec::new();
                        let mut current = String::new();
                        for line in raw.lines() {
                            if line.is_empty() {
                                if !current.is_empty() {
                                    events.push(std::mem::take(&mut current));
                                }
                                continue;
                            }
                            current.push_str(line);
                            current.push('\n');
                        }
                        if !current.is_empty() {
                            events.push(current);
                        }

                        if let Some(payload) = events.get(frame_id as usize) {
                            let body = serde_json::json!({
                                "frame": frame.clone(),
                                "full_payload": payload
                            });
                            return json_response(&body);
                        }
                    }
                }
            }
        }

        json_response(&frame)
    } else {
        error_response(
            StatusCode::NOT_FOUND,
            &format!(
                "Frame {} not found in connection {}",
                frame_id, connection_id
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;

    use http_body_util::BodyExt;
    use serde::Deserialize;

    use super::get_frames;
    use crate::connection_monitor::WebSocketFrameRecord;
    use crate::frame_store::FrameStore;
    use crate::state::AdminState;
    use crate::traffic::{FrameDirection, FrameType, SocketStatus, TrafficRecord};
    use crate::traffic_db::TrafficDbStore;

    #[derive(Debug, Deserialize)]
    struct FramesResponseForTest {
        frames: Vec<WebSocketFrameRecord>,
        socket_status: Option<SocketStatus>,
        last_frame_id: u64,
        has_more: bool,
    }

    #[tokio::test]
    async fn frames_has_more_true_when_file_has_more() {
        let dir = std::env::temp_dir().join(format!("bifrost-admin-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();

        let frame_store = Arc::new(FrameStore::new(dir.clone(), None));
        let connection_id = "test-connection";

        for frame_id in 1..=101u64 {
            let frame = WebSocketFrameRecord::new(
                frame_id,
                FrameDirection::Receive,
                FrameType::Text,
                b"hi",
                false,
                true,
                256,
                true,
            );
            frame_store.append_frame(connection_id, &frame).unwrap();
        }
        frame_store.flush();

        let mut state = AdminState::new(0);
        state.frame_store = Some(frame_store);
        let state = Arc::new(state);

        let resp = get_frames(state, connection_id, Some("limit=100")).await;
        assert!(resp.status().is_success());

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let parsed: FramesResponseForTest = serde_json::from_slice(&body).unwrap();

        assert_eq!(parsed.frames.len(), 100);
        assert_eq!(parsed.last_frame_id, 100);
        assert!(parsed.has_more);

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn frames_falls_back_to_persisted_socket_status_after_monitor_cleanup() {
        let dir = std::env::temp_dir().join(format!("bifrost-admin-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();

        let store = Arc::new(TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap());
        let state = Arc::new(AdminState::new(0).with_traffic_db_store_shared(store));
        let connection_id = "streaming-cleaned";
        let mut record = TrafficRecord::new(
            connection_id.to_string(),
            "GET".to_string(),
            "http://example.test/stream".to_string(),
        );
        record.socket_status = Some(SocketStatus {
            is_open: false,
            receive_bytes: 4096,
            ..Default::default()
        });
        state.record_traffic(record);

        let resp = get_frames(state, connection_id, None).await;
        assert!(resp.status().is_success());

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let parsed: FramesResponseForTest = serde_json::from_slice(&body).unwrap();
        let socket_status = parsed
            .socket_status
            .expect("persisted socket status should be returned");
        assert!(!socket_status.is_open);
        assert_eq!(socket_status.receive_bytes, 4096);

        let _ = fs::remove_dir_all(&dir);
    }
}
