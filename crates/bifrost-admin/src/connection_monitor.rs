use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bifrost_core::text::floor_char_boundary;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::body_store::{BodyRef, SharedBodyStore};
use crate::frame_store::SharedFrameStore;
use crate::traffic::{FrameDirection, FrameType, SocketStatus};
use crate::ws_payload_store::SharedWsPayloadStore;

const DEFAULT_PREVIEW_LIMIT: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSocketFrameRecord {
    pub frame_id: u64,
    pub timestamp: u64,
    pub direction: FrameDirection,
    pub frame_type: FrameType,
    pub payload_size: usize,
    pub payload_is_text: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload_preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload_ref: Option<BodyRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_payload_size: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_payload_is_text: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_payload_preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_payload_ref: Option<BodyRef>,
    pub is_masked: bool,
    pub is_fin: bool,
}

impl WebSocketFrameRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        frame_id: u64,
        direction: FrameDirection,
        frame_type: FrameType,
        payload: &[u8],
        is_masked: bool,
        is_fin: bool,
        preview_limit: usize,
        payload_is_text: bool,
    ) -> Self {
        let payload_preview = if preview_limit == 0 || payload.is_empty() {
            None
        } else if payload_is_text {
            let text = String::from_utf8_lossy(payload);
            let end = floor_char_boundary(&text, preview_limit);
            Some(text[..end].to_string())
        } else if payload.len() <= preview_limit {
            Some(base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                payload,
            ))
        } else {
            Some(base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                &payload[..preview_limit],
            ))
        };

        Self {
            frame_id,
            timestamp: chrono::Utc::now().timestamp_millis() as u64,
            direction,
            frame_type,
            payload_size: payload.len(),
            payload_is_text,
            payload_preview,
            payload_ref: None,
            raw_payload_size: None,
            raw_payload_is_text: None,
            raw_payload_preview: None,
            raw_payload_ref: None,
            is_masked,
            is_fin,
        }
    }

    pub fn new_sse_event(frame_id: u64, payload: &[u8], preview_limit: usize) -> Self {
        let payload_preview = if preview_limit == 0 || payload.is_empty() {
            None
        } else {
            let preview_bytes = &payload[..payload.len().min(preview_limit)];
            Some(String::from_utf8_lossy(preview_bytes).to_string())
        };

        Self {
            frame_id,
            timestamp: chrono::Utc::now().timestamp_millis() as u64,
            direction: FrameDirection::Receive,
            frame_type: FrameType::Sse,
            payload_size: payload.len(),
            payload_is_text: true,
            payload_preview,
            payload_ref: None,
            raw_payload_size: None,
            raw_payload_is_text: None,
            raw_payload_preview: None,
            raw_payload_ref: None,
            is_masked: false,
            is_fin: true,
        }
    }

    pub fn new_sse_event_with_preview(
        frame_id: u64,
        payload_size: usize,
        payload_preview: Option<String>,
        payload_ref: Option<BodyRef>,
    ) -> Self {
        Self {
            frame_id,
            timestamp: chrono::Utc::now().timestamp_millis() as u64,
            direction: FrameDirection::Receive,
            frame_type: FrameType::Sse,
            payload_size,
            payload_is_text: true,
            payload_preview,
            payload_ref,
            raw_payload_size: None,
            raw_payload_is_text: None,
            raw_payload_preview: None,
            raw_payload_ref: None,
            is_masked: false,
            is_fin: true,
        }
    }
}
const DEFAULT_MAX_FRAMES_PER_CONNECTION: usize = 1000;
const BROADCAST_CHANNEL_SIZE: usize = 256;
const CLOSED_CONNECTION_RETENTION: Duration = Duration::from_secs(30);
const CONNECTION_CLEANUP_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub struct FrameEvent {
    pub connection_id: String,
    pub frame: WebSocketFrameRecord,
}

struct SseEventInput {
    payload_size: usize,
    payload_preview: Option<String>,
    payload_ref: Option<BodyRef>,
}

pub struct ConnectionFrameStore {
    pub(crate) frames: VecDeque<WebSocketFrameRecord>,
    max_frames: usize,
    frame_id_counter: AtomicU64,
    status: SocketStatus,
    closed_at: Option<Instant>,
    is_monitored: bool,
    is_tunnel: bool, // 标记是否为隧道连接
    tx: broadcast::Sender<FrameEvent>,
}

impl ConnectionFrameStore {
    pub fn new(max_frames: usize) -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CHANNEL_SIZE);
        let initial_capacity = max_frames.min(32);
        Self {
            frames: VecDeque::with_capacity(initial_capacity),
            max_frames,
            frame_id_counter: AtomicU64::new(0),
            status: SocketStatus::default(),
            closed_at: None,
            is_monitored: false,
            is_tunnel: false,
            tx,
        }
    }

    pub fn new_tunnel(max_frames: usize) -> Self {
        let mut store = Self::new(max_frames);
        store.is_tunnel = true;
        store
    }

    pub fn next_frame_id(&self) -> u64 {
        self.frame_id_counter.fetch_add(1, Ordering::SeqCst)
    }

    pub fn add_frame(&mut self, frame: WebSocketFrameRecord) {
        tracing::debug!(
            "[WS_MONITOR] add_frame: frame_id={}, frame_type={:?}, direction={:?}, payload_size={}",
            frame.frame_id,
            frame.frame_type,
            frame.direction,
            frame.payload_size
        );
        match frame.direction {
            FrameDirection::Send => {
                self.status.send_count += 1;
                self.status.send_bytes += frame.payload_size as u64;
            }
            FrameDirection::Receive => {
                self.status.receive_count += 1;
                self.status.receive_bytes += frame.payload_size as u64;
            }
        }

        if self.frames.len() >= self.max_frames {
            self.frames.pop_front();
        }
        self.frames.push_back(frame.clone());
        tracing::debug!(
            "[WS_MONITOR] after add_frame: frames.len()={}",
            self.frames.len()
        );
    }

    pub fn set_closed(&mut self, code: Option<u16>, reason: Option<String>) {
        self.status.is_open = false;
        self.status.close_code = code;
        self.status.close_reason = reason;
        self.closed_at = Some(Instant::now());
    }
}

pub struct ConnectionMonitor {
    pub(crate) connections: RwLock<HashMap<String, ConnectionFrameStore>>,
    preview_limit: usize,
    max_frames_per_connection: usize,
    global_tx: broadcast::Sender<FrameEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConnectionMonitorMemoryStats {
    pub connection_count: usize,
    pub open_connection_count: usize,
    pub closed_connection_count: usize,
    pub tunnel_connection_count: usize,
    pub total_frames_in_memory: usize,
    pub total_preview_bytes: usize,
    pub total_inline_ref_bytes: usize,
    pub monitored_connection_count: usize,
}

impl ConnectionMonitor {
    pub fn new() -> Self {
        Self::with_config(DEFAULT_PREVIEW_LIMIT, DEFAULT_MAX_FRAMES_PER_CONNECTION)
    }

    pub fn with_config(preview_limit: usize, max_frames_per_connection: usize) -> Self {
        let (global_tx, _) = broadcast::channel(BROADCAST_CHANNEL_SIZE * 4);
        Self {
            connections: RwLock::new(HashMap::new()),
            preview_limit,
            max_frames_per_connection,
            global_tx,
        }
    }

    pub fn register_connection(&self, connection_id: &str) {
        let mut connections = self.connections.write();
        if !connections.contains_key(connection_id) {
            connections.insert(
                connection_id.to_string(),
                ConnectionFrameStore::new(self.max_frames_per_connection),
            );
        }
    }

    pub fn register_tunnel_connection(&self, connection_id: &str) {
        let mut connections = self.connections.write();
        if !connections.contains_key(connection_id) {
            // 为隧道连接设置最小的最大帧数量，进一步减少内存使用
            let tunnel_max_frames = 5; // 只保留最基本的统计信息
            let mut store = ConnectionFrameStore::new(tunnel_max_frames);
            store.is_tunnel = true;
            // 隧道连接不需要监控，默认设置为非监控状态
            store.is_monitored = false;
            connections.insert(connection_id.to_string(), store);
        }
    }

    pub fn unregister_connection(&self, connection_id: &str) {
        self.connections.write().remove(connection_id);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_frame(
        &self,
        connection_id: &str,
        direction: FrameDirection,
        frame_type: FrameType,
        payload: &[u8],
        payload_is_text: bool,
        raw_payload: Option<&[u8]>,
        is_masked: bool,
        is_fin: bool,
        body_store: Option<&SharedBodyStore>,
        ws_payload_store: Option<&SharedWsPayloadStore>,
        frame_store: Option<&SharedFrameStore>,
    ) -> Option<WebSocketFrameRecord> {
        let mut connections = self.connections.write();
        let store = connections.get_mut(connection_id)?;

        // 对于隧道连接，只记录基本统计信息，不记录详细帧数据
        if store.is_tunnel {
            // 只更新流量统计
            match direction {
                FrameDirection::Send => {
                    store.status.send_count += 1;
                    store.status.send_bytes += payload.len() as u64;
                }
                FrameDirection::Receive => {
                    store.status.receive_count += 1;
                    store.status.receive_bytes += payload.len() as u64;
                }
            }
            return None;
        }

        let frame_id = store.next_frame_id();
        let mut frame = WebSocketFrameRecord::new(
            frame_id,
            direction,
            frame_type,
            payload,
            is_masked,
            is_fin,
            self.preview_limit,
            payload_is_text,
        );
        if !payload.is_empty() {
            let direction_str = match direction {
                FrameDirection::Send => "send",
                FrameDirection::Receive => "recv",
            };

            if raw_payload.is_some() {
                let raw = raw_payload.unwrap_or(payload);
                let raw_is_text = frame_type == FrameType::Text
                    || frame_type == FrameType::Close
                    || frame_type == FrameType::Sse;
                frame.raw_payload_size = Some(raw.len());
                frame.raw_payload_is_text = Some(raw_is_text);
                frame.raw_payload_preview = if self.preview_limit == 0 || raw.is_empty() {
                    None
                } else if raw_is_text {
                    let preview_bytes = &raw[..raw.len().min(self.preview_limit)];
                    Some(String::from_utf8_lossy(preview_bytes).to_string())
                } else if raw.len() <= self.preview_limit {
                    Some(base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        raw,
                    ))
                } else {
                    Some(base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        &raw[..self.preview_limit],
                    ))
                };

                let raw_ref = if let Some(ws_payload_store) = ws_payload_store {
                    ws_payload_store.append_bytes(connection_id, raw)
                } else {
                    None
                };

                if raw_ref.is_some() {
                    frame.raw_payload_ref = raw_ref;
                } else if let Some(body_store) = body_store {
                    let ref_key =
                        format!("{}_frame_{}_{}_raw", connection_id, frame_id, direction_str);
                    let raw_for_store: Vec<u8> = if raw_is_text {
                        raw.to_vec()
                    } else {
                        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, raw)
                            .into_bytes()
                    };
                    frame.raw_payload_ref =
                        body_store
                            .read()
                            .store_force_file(&ref_key, "frame", &raw_for_store);
                }
            }

            if let Some(ws_payload_store) = ws_payload_store {
                frame.payload_ref = ws_payload_store.append_bytes(connection_id, payload);
            } else if let Some(body_store) = body_store {
                let ref_key = format!("{}_frame_{}_{}", connection_id, frame_id, direction_str);
                let payload_for_store: Vec<u8> = if payload_is_text {
                    payload.to_vec()
                } else {
                    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, payload)
                        .into_bytes()
                };
                frame.payload_ref = body_store
                    .read()
                    .store(&ref_key, "frame", &payload_for_store);
            } else {
                frame.payload_ref = Some(BodyRef::Inline {
                    data: String::from_utf8_lossy(payload).to_string(),
                });
            }
        }

        let frame_for_store = frame.clone();
        let frame_for_memory = frame;
        store.add_frame(frame_for_memory.clone());

        if let Some(fs) = frame_store {
            if let Err(e) = fs.append_frame(connection_id, &frame_for_store) {
                tracing::warn!(
                    "[WS_MONITOR] Failed to persist frame {} for {}: {}",
                    frame_id,
                    connection_id,
                    e
                );
            }
        }

        let event = FrameEvent {
            connection_id: connection_id.to_string(),
            frame: frame_for_memory.clone(),
        };
        let _ = store.tx.send(event.clone());
        let _ = self.global_tx.send(event);

        Some(frame_for_memory)
    }

    pub fn record_sse_event(
        &self,
        connection_id: &str,
        payload: &[u8],
        body_store: Option<&SharedBodyStore>,
        frame_store: Option<&SharedFrameStore>,
    ) -> Option<WebSocketFrameRecord> {
        let mut connections = self.connections.write();
        let store = connections.get_mut(connection_id)?;
        let frame_id = store.next_frame_id();
        let payload_ref = if payload.is_empty() {
            None
        } else {
            let inline = BodyRef::Inline {
                data: String::from_utf8_lossy(payload).to_string(),
            };
            if let Some(body_store) = body_store {
                let ref_key = format!("{}_sse_event_{}", connection_id, frame_id);
                body_store
                    .read()
                    .store_force_file(&ref_key, "sse", payload)
                    .or(Some(inline))
            } else {
                Some(inline)
            }
        };
        let input = SseEventInput {
            payload_size: payload.len(),
            payload_preview: None,
            payload_ref,
        };
        self.record_sse_event_inner(store, connection_id, frame_id, input, frame_store)
    }

    pub fn record_sse_event_streamed(
        &self,
        connection_id: &str,
        payload_size: usize,
        payload_ref: Option<BodyRef>,
        frame_store: Option<&SharedFrameStore>,
    ) -> Option<WebSocketFrameRecord> {
        let mut connections = self.connections.write();
        let store = connections.get_mut(connection_id)?;
        let frame_id = store.next_frame_id();
        let input = SseEventInput {
            payload_size,
            payload_preview: None,
            payload_ref,
        };

        self.record_sse_event_inner(store, connection_id, frame_id, input, frame_store)
    }

    fn record_sse_event_inner(
        &self,
        store: &mut ConnectionFrameStore,
        connection_id: &str,
        frame_id: u64,
        input: SseEventInput,
        frame_store: Option<&SharedFrameStore>,
    ) -> Option<WebSocketFrameRecord> {
        let frame = WebSocketFrameRecord::new_sse_event_with_preview(
            frame_id,
            input.payload_size,
            input.payload_preview,
            input.payload_ref,
        );

        let frame_for_store = frame.clone();
        let frame_for_memory = frame;
        store.add_frame(frame_for_memory.clone());

        if let Some(fs) = frame_store {
            if let Err(e) = fs.append_frame(connection_id, &frame_for_store) {
                tracing::warn!(
                    "[WS_MONITOR] Failed to persist SSE event {} for {}: {}",
                    frame_id,
                    connection_id,
                    e
                );
            }
        }

        let event = FrameEvent {
            connection_id: connection_id.to_string(),
            frame: frame_for_memory.clone(),
        };
        let _ = store.tx.send(event.clone());
        let _ = self.global_tx.send(event);

        Some(frame_for_memory)
    }

    pub fn set_connection_closed(
        &self,
        connection_id: &str,
        code: Option<u16>,
        reason: Option<String>,
        frame_store: Option<&SharedFrameStore>,
        ws_payload_store: Option<&SharedWsPayloadStore>,
    ) {
        self.close_connection(connection_id, code, reason, frame_store, ws_payload_store);
    }

    pub fn close_connection(
        &self,
        connection_id: &str,
        code: Option<u16>,
        reason: Option<String>,
        frame_store: Option<&SharedFrameStore>,
        ws_payload_store: Option<&SharedWsPayloadStore>,
    ) -> Option<SocketStatus> {
        let mut connections = self.connections.write();
        let mut final_status = None;
        if let Some(store) = connections.get_mut(connection_id) {
            store.set_closed(code, reason);
            final_status = Some(store.status.clone());

            // 对于隧道连接，关闭后自动清理，释放内存
            if store.is_tunnel {
                connections.remove(connection_id);
                return final_status;
            }
        }
        if let Some(fs) = frame_store {
            fs.mark_connection_closed(connection_id);
        }
        if let Some(ws_payload_store) = ws_payload_store {
            ws_payload_store.close(connection_id);
        }
        final_status
    }

    pub fn start_monitoring(&self, connection_id: &str) -> bool {
        let mut connections = self.connections.write();
        if let Some(store) = connections.get_mut(connection_id) {
            store.is_monitored = true;
            true
        } else {
            false
        }
    }

    pub fn stop_monitoring(&self, connection_id: &str) -> bool {
        let mut connections = self.connections.write();
        if let Some(store) = connections.get_mut(connection_id) {
            store.is_monitored = false;
            true
        } else {
            false
        }
    }

    pub fn is_monitored(&self, connection_id: &str) -> bool {
        self.connections
            .read()
            .get(connection_id)
            .map(|s| s.is_monitored)
            .unwrap_or(false)
    }

    pub fn get_connection_status(&self, connection_id: &str) -> Option<SocketStatus> {
        let connections = self.connections.read();
        let store = connections.get(connection_id)?;
        let mut status = store.status.clone();
        status.frame_count = store.frames.len();
        Some(status)
    }

    pub fn get_frames(
        &self,
        connection_id: &str,
        after_frame_id: Option<u64>,
        limit: usize,
    ) -> Option<(Vec<WebSocketFrameRecord>, bool)> {
        let connections = self.connections.read();
        let store = connections.get(connection_id)?;

        tracing::debug!(
            "[WS_MONITOR] get_frames for {}: store.frames.len()={}, after_frame_id={:?}, limit={}",
            connection_id,
            store.frames.len(),
            after_frame_id,
            limit
        );

        let frames: Vec<_> = if let Some(after_id) = after_frame_id {
            store
                .frames
                .iter()
                .filter(|f| f.frame_id > after_id)
                .cloned()
                .collect()
        } else {
            store.frames.iter().cloned().collect()
        };

        let has_more = frames.len() > limit;
        let result = frames.into_iter().take(limit).collect();

        Some((result, has_more))
    }

    pub fn get_frames_with_persistence(
        &self,
        connection_id: &str,
        after_frame_id: Option<u64>,
        limit: usize,
        frame_store: Option<&SharedFrameStore>,
    ) -> (Vec<WebSocketFrameRecord>, bool, bool) {
        use std::collections::HashSet;

        let mut all_frames = Vec::new();
        let mut seen_ids: HashSet<u64> = HashSet::new();
        let is_active = self.connections.read().contains_key(connection_id);

        if let Some(fs) = frame_store {
            match fs.load_frames(connection_id, after_frame_id, usize::MAX) {
                Ok((file_frames, _)) => {
                    for frame in file_frames {
                        if !seen_ids.contains(&frame.frame_id) {
                            seen_ids.insert(frame.frame_id);
                            all_frames.push(frame);
                        }
                    }
                    tracing::debug!(
                        "[WS_MONITOR] Loaded {} frames from file for {}",
                        all_frames.len(),
                        connection_id
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        "[WS_MONITOR] Failed to load frames from file for {}: {}",
                        connection_id,
                        e
                    );
                }
            }
        }

        if let Some(store) = self.connections.read().get(connection_id) {
            let mem_frames: Vec<_> = if let Some(after_id) = after_frame_id {
                store
                    .frames
                    .iter()
                    .filter(|f| f.frame_id > after_id)
                    .cloned()
                    .collect()
            } else {
                store.frames.iter().cloned().collect()
            };

            for frame in mem_frames {
                if !seen_ids.contains(&frame.frame_id) {
                    seen_ids.insert(frame.frame_id);
                    all_frames.push(frame);
                }
            }
        }

        all_frames.sort_by_key(|f| f.frame_id);

        let has_more = all_frames.len() > limit;
        let result: Vec<_> = all_frames.into_iter().take(limit).collect();

        (result, has_more, is_active)
    }

    pub fn has_persisted_frames(
        &self,
        connection_id: &str,
        frame_store: Option<&SharedFrameStore>,
    ) -> bool {
        if let Some(fs) = frame_store {
            fs.connection_exists(connection_id)
        } else {
            false
        }
    }

    pub fn get_status(&self, connection_id: &str) -> Option<SocketStatus> {
        self.connections
            .read()
            .get(connection_id)
            .map(|s| s.status.clone())
    }

    pub fn update_traffic(
        &self,
        connection_id: &str,
        direction: FrameDirection,
        bytes: u64,
    ) -> bool {
        let mut connections = self.connections.write();
        if let Some(store) = connections.get_mut(connection_id) {
            match direction {
                FrameDirection::Send => {
                    store.status.send_bytes += bytes;
                }
                FrameDirection::Receive => {
                    store.status.receive_bytes += bytes;
                }
            }
            true
        } else {
            false
        }
    }

    pub fn get_last_frame_id(&self, connection_id: &str) -> Option<u64> {
        self.connections
            .read()
            .get(connection_id)
            .map(|s| s.frame_id_counter.load(Ordering::SeqCst).saturating_sub(1))
    }

    pub fn get_frame_count(&self, connection_id: &str) -> Option<usize> {
        self.connections
            .read()
            .get(connection_id)
            .map(|s| s.frames.len())
    }

    pub fn subscribe_connection(
        &self,
        connection_id: &str,
    ) -> Option<broadcast::Receiver<FrameEvent>> {
        self.connections
            .read()
            .get(connection_id)
            .map(|s| s.tx.subscribe())
    }

    pub fn subscribe_all(&self) -> broadcast::Receiver<FrameEvent> {
        self.global_tx.subscribe()
    }

    pub fn cleanup_closed_connections(&self) {
        self.cleanup_closed_connections_older_than(CLOSED_CONNECTION_RETENTION);
    }

    fn cleanup_closed_connections_older_than(&self, retention: Duration) {
        let now = Instant::now();
        let mut connections = self.connections.write();
        connections.retain(|_, store| {
            if store.status.is_open || store.is_monitored {
                return true;
            }

            store
                .closed_at
                .is_none_or(|closed_at| now.duration_since(closed_at) < retention)
        });
    }

    pub fn clear(&self) {
        self.connections.write().clear();
        tracing::info!("[WEBSOCKET_MONITOR] Cleared all connections");
    }

    pub fn connection_count(&self) -> usize {
        self.connections.read().len()
    }

    pub fn memory_stats(&self) -> ConnectionMonitorMemoryStats {
        let connections = self.connections.read();

        let mut stats = ConnectionMonitorMemoryStats {
            connection_count: connections.len(),
            ..Default::default()
        };

        for (_id, store) in connections.iter() {
            if store.status.is_open {
                stats.open_connection_count += 1;
            } else {
                stats.closed_connection_count += 1;
            }
            if store.is_tunnel {
                stats.tunnel_connection_count += 1;
            }
            if store.is_monitored {
                stats.monitored_connection_count += 1;
            }

            stats.total_frames_in_memory = stats
                .total_frames_in_memory
                .saturating_add(store.frames.len());

            for f in store.frames.iter() {
                if let Some(p) = &f.payload_preview {
                    stats.total_preview_bytes = stats.total_preview_bytes.saturating_add(p.len());
                }
                if let Some(p) = &f.raw_payload_preview {
                    stats.total_preview_bytes = stats.total_preview_bytes.saturating_add(p.len());
                }
                if let Some(BodyRef::Inline { data }) = &f.payload_ref {
                    stats.total_inline_ref_bytes =
                        stats.total_inline_ref_bytes.saturating_add(data.len());
                }
                if let Some(BodyRef::Inline { data }) = &f.raw_payload_ref {
                    stats.total_inline_ref_bytes =
                        stats.total_inline_ref_bytes.saturating_add(data.len());
                }
            }
        }

        stats
    }

    pub fn active_connection_ids(&self) -> Vec<String> {
        self.connections
            .read()
            .iter()
            .filter(|(_, store)| store.status.is_open)
            .map(|(id, _)| id.clone())
            .collect()
    }
}

impl Default for ConnectionMonitor {
    fn default() -> Self {
        Self::new()
    }
}

pub type SharedConnectionMonitor = Arc<ConnectionMonitor>;

pub fn start_connection_cleanup_task(
    monitor: SharedConnectionMonitor,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(CONNECTION_CLEANUP_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            monitor.cleanup_closed_connections();
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_connection() {
        let monitor = ConnectionMonitor::new();
        monitor.register_connection("conn-1");
        assert_eq!(monitor.connection_count(), 1);
    }

    #[test]
    fn test_record_frame() {
        let monitor = ConnectionMonitor::new();
        monitor.register_connection("conn-1");

        let frame = monitor.record_frame(
            "conn-1",
            FrameDirection::Send,
            FrameType::Text,
            b"Hello, World!",
            true,
            None,
            true,
            true,
            None,
            None,
            None,
        );

        assert!(frame.is_some());
        let frame = frame.unwrap();
        assert_eq!(frame.frame_id, 0);
        assert_eq!(frame.direction, FrameDirection::Send);
        assert_eq!(frame.frame_type, FrameType::Text);
        assert_eq!(frame.payload_size, 13);
    }

    #[test]
    fn test_get_frames() {
        let monitor = ConnectionMonitor::new();
        monitor.register_connection("conn-1");

        for i in 0..5 {
            monitor.record_frame(
                "conn-1",
                FrameDirection::Send,
                FrameType::Text,
                format!("Message {}", i).as_bytes(),
                true,
                None,
                true,
                true,
                None,
                None,
                None,
            );
        }

        let (frames, has_more) = monitor.get_frames("conn-1", None, 10).unwrap();
        assert_eq!(frames.len(), 5);
        assert!(!has_more);

        let (frames, has_more) = monitor.get_frames("conn-1", Some(2), 10).unwrap();
        assert_eq!(frames.len(), 2);
        assert!(!has_more);
    }

    #[test]
    fn test_monitoring_state() {
        let monitor = ConnectionMonitor::new();
        monitor.register_connection("conn-1");

        assert!(!monitor.is_monitored("conn-1"));
        monitor.start_monitoring("conn-1");
        assert!(monitor.is_monitored("conn-1"));
        monitor.stop_monitoring("conn-1");
        assert!(!monitor.is_monitored("conn-1"));
    }

    #[test]
    fn test_socket_status() {
        let monitor = ConnectionMonitor::new();
        monitor.register_connection("conn-1");

        monitor.record_frame(
            "conn-1",
            FrameDirection::Send,
            FrameType::Text,
            b"test",
            true,
            None,
            true,
            true,
            None,
            None,
            None,
        );
        monitor.record_frame(
            "conn-1",
            FrameDirection::Receive,
            FrameType::Text,
            b"response",
            true,
            None,
            false,
            true,
            None,
            None,
            None,
        );

        let status = monitor.get_status("conn-1").unwrap();
        assert!(status.is_open);
        assert_eq!(status.send_count, 1);
        assert_eq!(status.receive_count, 1);
        assert_eq!(status.send_bytes, 4);
        assert_eq!(status.receive_bytes, 8);
    }

    #[test]
    fn test_connection_closed() {
        let monitor = ConnectionMonitor::new();
        monitor.register_connection("conn-1");

        monitor.set_connection_closed(
            "conn-1",
            Some(1000),
            Some("Normal closure".to_string()),
            None,
            None,
        );

        let status = monitor.get_status("conn-1").unwrap();
        assert!(!status.is_open);
        assert_eq!(status.close_code, Some(1000));
        assert_eq!(status.close_reason, Some("Normal closure".to_string()));
    }

    #[test]
    fn cleanup_closed_connections_respects_retention_and_monitoring() {
        let monitor = ConnectionMonitor::new();
        monitor.register_connection("closed");
        monitor.register_connection("monitored");
        monitor.start_monitoring("monitored");

        monitor.set_connection_closed("closed", None, None, None, None);
        monitor.set_connection_closed("monitored", None, None, None, None);

        monitor.cleanup_closed_connections_older_than(Duration::from_secs(30));
        assert!(monitor.get_status("closed").is_some());
        assert!(monitor.get_status("monitored").is_some());

        monitor.cleanup_closed_connections_older_than(Duration::ZERO);
        assert!(monitor.get_status("closed").is_none());
        assert!(monitor.get_status("monitored").is_some());
    }

    #[test]
    fn memory_stats_counts_open_and_closed_connections() {
        let monitor = ConnectionMonitor::new();
        monitor.register_connection("open");
        monitor.register_connection("closed");
        monitor.set_connection_closed("closed", None, None, None, None);

        let stats = monitor.memory_stats();
        assert_eq!(stats.connection_count, 2);
        assert_eq!(stats.open_connection_count, 1);
        assert_eq!(stats.closed_connection_count, 1);
    }
}
