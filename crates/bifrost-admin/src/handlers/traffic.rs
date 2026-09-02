use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use tokio_stream::StreamExt;

use base64::Engine as _;

use super::frames::{get_frame_detail, get_frames, subscribe_frames, unsubscribe_frames};
use super::network_body::{
    content_encoding_is_supported, decode_content_encoded_body_with_limit, decompress_with_limit,
    DEFAULT_MAX_DECOMPRESSED_BODY_BYTES,
};
use super::{
    error_response, full_body, json_response, method_not_allowed, success_response, BoxBody,
};
use crate::body_store::BodyRef;
use crate::push::{SharedPushManager, MAX_ID_LEN, MAX_SUBSCRIBED_IDS};
use crate::query_service::AdminQueryService;
use crate::state::{AdminState, SharedAdminState};
use crate::traffic_db::{QueryParams, TrafficSummaryCompact};

fn empty_query_result() -> crate::traffic_db::QueryResult {
    crate::traffic_db::QueryResult {
        records: Vec::new(),
        next_cursor: None,
        prev_cursor: None,
        has_more: false,
        total: 0,
        server_sequence: 0,
    }
}

async fn join_clear_task<T>(
    task: tokio::task::JoinHandle<std::result::Result<T, String>>,
    label: &str,
) -> std::result::Result<T, String> {
    task.await
        .map_err(|e| format!("{label} clear task join failed: {e}"))?
}

fn enrich_compact_frame_info(summary: &mut TrafficSummaryCompact, state: &AdminState) {
    state.reconcile_socket_summary(summary);
}

pub async fn handle_traffic(
    req: Request<Incoming>,
    state: SharedAdminState,
    push_manager: Option<SharedPushManager>,
    path: &str,
) -> Response<BoxBody> {
    let path = path.trim_end_matches('/');
    let method = req.method().clone();

    if path == "/api/traffic" {
        match method {
            Method::GET => list_traffic(req, state).await,
            Method::DELETE => clear_traffic(req, state, push_manager).await,
            _ => method_not_allowed(),
        }
    } else if path == "/api/traffic/query" {
        match method {
            Method::POST => query_traffic(req, state).await,
            _ => method_not_allowed(),
        }
    } else if path == "/api/traffic/updates" {
        match method {
            Method::GET => get_traffic_updates(req, state).await,
            _ => method_not_allowed(),
        }
    } else if path == "/api/traffic/statistics" {
        match method {
            Method::GET => get_traffic_statistics(state),
            _ => method_not_allowed(),
        }
    } else if path == "/api/traffic/batch" {
        match method {
            Method::GET => batch_traffic(req, state).await,
            _ => method_not_allowed(),
        }
    } else if let Some(rest) = path.strip_prefix("/api/traffic/") {
        let rest = rest.trim_end_matches('/');
        if let Some(id) = rest.strip_suffix("/request-body") {
            match method {
                Method::GET => get_request_body(state, id, req.uri().query()).await,
                _ => method_not_allowed(),
            }
        } else if let Some(id) = rest.strip_suffix("/response-body/content") {
            match method {
                Method::GET => get_response_body_content(state, id, req.uri().query()).await,
                _ => method_not_allowed(),
            }
        } else if let Some(id) = rest.strip_suffix("/response-body") {
            match method {
                Method::GET => get_response_body(state, id, req.uri().query()).await,
                _ => method_not_allowed(),
            }
        } else if let Some((id, after)) = rest.split_once("/sse/stream") {
            let after = after.trim().trim_matches('/');
            if !after.is_empty() {
                return error_response(StatusCode::BAD_REQUEST, "Invalid SSE stream path");
            }
            match method {
                Method::GET => subscribe_sse_stream(state, id, req.uri().query()).await,
                _ => method_not_allowed(),
            }
        } else if let Some(id) = rest.strip_suffix("/frames/stream") {
            match method {
                Method::GET => subscribe_frames(state, id).await,
                _ => method_not_allowed(),
            }
        } else if let Some(id) = rest.strip_suffix("/frames/unsubscribe") {
            match method {
                Method::DELETE => unsubscribe_frames(state, id).await,
                _ => method_not_allowed(),
            }
        } else if rest.contains("/frames/") {
            if let Some((id, frame_part)) = rest.split_once("/frames/") {
                if let Ok(frame_id) = frame_part.parse::<u64>() {
                    match method {
                        Method::GET => get_frame_detail(state, id, frame_id).await,
                        _ => method_not_allowed(),
                    }
                } else {
                    error_response(StatusCode::BAD_REQUEST, "Invalid frame ID")
                }
            } else {
                error_response(StatusCode::BAD_REQUEST, "Invalid path")
            }
        } else if let Some(id) = rest.strip_suffix("/frames") {
            match method {
                Method::GET => get_frames(state, id, req.uri().query()).await,
                _ => method_not_allowed(),
            }
        } else if let Some(id) = rest.strip_suffix("/auth-status") {
            match method {
                Method::GET => get_traffic_auth_status(state, id).await,
                _ => method_not_allowed(),
            }
        } else if let Some(id) = rest.strip_suffix("/export") {
            match method {
                Method::GET => get_traffic_export(state, id, req.uri().query()).await,
                _ => method_not_allowed(),
            }
        } else if let Some(id) = rest.strip_suffix("/replay") {
            match method {
                Method::POST => post_traffic_replay(req, state, id).await,
                _ => method_not_allowed(),
            }
        } else {
            match method {
                Method::GET => get_traffic_detail(state, rest).await,
                _ => method_not_allowed(),
            }
        }
    } else {
        error_response(StatusCode::NOT_FOUND, "Not Found")
    }
}

fn get_traffic_statistics(state: SharedAdminState) -> Response<BoxBody> {
    match state.traffic_db_store.as_ref() {
        Some(store) => json_response(&store.traffic_statistics()),
        None => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Traffic database not available",
        ),
    }
}

fn query_wants_raw(query: Option<&str>) -> bool {
    let Some(q) = query else {
        return false;
    };
    for part in q.split('&') {
        if let Some(v) = part.strip_prefix("raw=") {
            if v == "1" || v.eq_ignore_ascii_case("true") {
                return true;
            }
            return false;
        }
    }
    false
}

fn query_wants_base64(query: Option<&str>) -> bool {
    let Some(q) = query else {
        return false;
    };
    for part in q.split('&') {
        if let Some(v) = part
            .strip_prefix("encoding=")
            .or_else(|| part.strip_prefix("format="))
        {
            return v.eq_ignore_ascii_case("base64");
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{
        decode_query_value, parse_query_params_from_query_string, parse_updates_params,
        query_wants_base64, query_wants_raw,
    };
    use crate::push::{MAX_ID_LEN, MAX_SUBSCRIBED_IDS};
    use crate::traffic_db::{Direction, TextMatchMode};

    #[test]
    fn raw_body_query_flags_are_parsed_independently() {
        assert!(query_wants_raw(Some("raw=1&encoding=base64")));
        assert!(query_wants_raw(Some("raw=true")));
        assert!(!query_wants_raw(Some("raw=0&encoding=base64")));
        assert!(!query_wants_raw(Some("encoding=base64")));

        assert!(query_wants_base64(Some("raw=1&encoding=base64")));
        assert!(query_wants_base64(Some("format=base64")));
        assert!(!query_wants_base64(Some("raw=1&encoding=text")));
        assert!(!query_wants_base64(None));
    }

    #[test]
    fn decode_query_value_handles_plus_and_percent_encoding() {
        assert_eq!(decode_query_value("hello+world"), "hello world");
        assert_eq!(decode_query_value("a%2Bb+test"), "a+b test");
    }

    #[test]
    fn parse_updates_params_parses_fields_and_limits_pending_ids() {
        let long_id = "x".repeat(MAX_ID_LEN + 1);
        let mut parts = vec!["keep".to_string(), "".to_string(), long_id.clone()];
        for i in 0..(MAX_SUBSCRIBED_IDS + 2) {
            parts.push(format!("id-{i}"));
        }
        let query = format!(
            "after_id=req-1&after_seq=42&pending_ids={}&limit=200",
            parts.join(",")
        );

        let params = parse_updates_params(&query);

        assert_eq!(params.after_id.as_deref(), Some("req-1"));
        assert_eq!(params.after_seq, Some(42));
        assert_eq!(params.limit, Some(200));
        assert!(params.pending_ids.len() <= MAX_SUBSCRIBED_IDS);
        assert!(params.pending_ids.contains(&"keep".to_string()));
        assert!(!params.pending_ids.contains(&"".to_string()));
        assert!(!params.pending_ids.contains(&long_id));
    }

    #[test]
    fn parse_query_params_from_query_string_parses_basic_filters() {
        let query = "\
cursor=10\
&limit=200\
&direction=forward\
&method=GET\
&status=200\
&status_min=100\
&status_max=500\
&protocol=HTTP\
&has_rule_hit=true\
&is_websocket=false\
&is_sse=true\
&is_h3=false\
&is_tunnel=true\
&host=example.com\
&url_contains=%2Fapi%2Fv1\
&path_contains=%2Ffoo\
&client_app=Chrome\
&client_app_match=equals\
&client_app_empty=false\
&client_ip=1.2.3.4\
&client_ip_match=contains\
&client_ip_empty=true\
&listener_port=8080\
&content_type=text%2Fplain";

        let params = parse_query_params_from_query_string(query);

        assert_eq!(params.cursor, Some(10));
        assert_eq!(params.limit, Some(200));
        assert_eq!(params.direction, Direction::Forward);
        assert_eq!(params.method.as_deref(), Some("GET"));
        assert_eq!(params.status, Some(200));
        assert_eq!(params.status_min, Some(100));
        assert_eq!(params.status_max, Some(500));
        assert_eq!(params.protocol.as_deref(), Some("HTTP"));
        assert_eq!(params.has_rule_hit, Some(true));
        assert_eq!(params.is_websocket, Some(false));
        assert_eq!(params.is_sse, Some(true));
        assert_eq!(params.is_h3, Some(false));
        assert_eq!(params.is_tunnel, Some(true));
        assert_eq!(params.host_contains.as_deref(), Some("example.com"));
        assert_eq!(params.url_contains.as_deref(), Some("/api/v1"));
        assert_eq!(params.path_contains.as_deref(), Some("/foo"));
        assert_eq!(params.client_app.as_deref(), Some("Chrome"));
        assert_eq!(params.client_app_match, TextMatchMode::Equals);
        assert_eq!(params.client_app_empty, Some(false));
        assert_eq!(params.client_ip.as_deref(), Some("1.2.3.4"));
        assert_eq!(params.client_ip_match, TextMatchMode::Contains);
        assert_eq!(params.client_ip_empty, Some(true));
        assert_eq!(params.listener_port, Some(8080));
        assert_eq!(params.content_type.as_deref(), Some("text/plain"));
    }

    #[test]
    fn parse_query_params_from_query_string_defaults_limit_when_missing() {
        let params = parse_query_params_from_query_string("method=GET");
        assert_eq!(params.method.as_deref(), Some("GET"));
        assert_eq!(params.limit, Some(100));
    }

    #[test]
    fn raw_body_query_flags_use_first_raw_value() {
        // First raw flag wins and short-circuits.
        assert!(query_wants_raw(Some("raw=1&raw=0")));
        assert!(!query_wants_raw(Some("raw=0&raw=1")));
        assert!(!query_wants_raw(Some("raw=false&encoding=base64")));
    }

    #[test]
    fn parse_updates_params_uses_cursor_alias_and_ignores_invalid_numbers() {
        let params = parse_updates_params("after_seq=bogus&cursor=10&limit=abc");
        assert_eq!(params.after_seq, Some(10));
        assert!(params.limit.is_none());
    }

    #[test]
    fn parse_query_params_from_query_string_supports_host_url_and_path_synonyms() {
        let params = parse_query_params_from_query_string(
            "host_contains=example.com&host=final.test&url=/v1&path=/items",
        );

        // Last occurrence wins and all aliases map to the *_contains fields.
        assert_eq!(params.host_contains.as_deref(), Some("final.test"));
        assert_eq!(params.url_contains.as_deref(), Some("/v1"));
        assert_eq!(params.path_contains.as_deref(), Some("/items"));
    }
}

async fn subscribe_sse_stream(
    state: SharedAdminState,
    id: &str,
    query: Option<&str>,
) -> Response<BoxBody> {
    let record = if let Some(ref db_store) = state.traffic_db_store {
        let db_clone = db_store.clone();
        let id_owned = id.to_string();
        tokio::task::spawn_blocking(move || db_clone.get_by_id(&id_owned))
            .await
            .unwrap_or_default()
    } else {
        None
    };

    let Some(record) = record else {
        return error_response(
            StatusCode::NOT_FOUND,
            &format!("Traffic record '{}' not found", id),
        );
    };

    if !record.is_sse {
        return error_response(StatusCode::BAD_REQUEST, "Not a SSE traffic record");
    }

    if state.sse_hub.is_open(id) != Some(true) {
        return error_response(
            StatusCode::CONFLICT,
            "SSE connection already closed; use /response-body to load and render events",
        );
    }

    // 管理端主动拉取 SSE messages：
    // - 触发 proxy 侧对该连接的 sse_raw 写盘进行更激进的 flush（短时间内每个 chunk 都 flush）
    // - 避免出现 count 增长但详情页 messages 长时间空的情况
    state.sse_hub.request_force_flush(id, 30_000);

    let body_ref = match record.response_body_ref {
        Some(r) => r,
        None => {
            return error_response(
                StatusCode::NOT_FOUND,
                &format!("SSE response body for {} not found", id),
            );
        }
    };

    let mut opts = parse_sse_stream_options(query);
    // 前端详情页对 SSE messages 更关心“实时可见性”，而不是减少消息条数。
    // 这里强制每个事件都单独推送（batch_size=1），避免等待凑满 batch 才看到第一屏。
    opts.batch_size = 1;
    let max_body_size = state.get_max_body_buffer_size();
    let stream = build_sse_disk_stream(
        state.clone(),
        id.to_string(),
        body_ref,
        opts.from,
        opts.batch_size,
        max_body_size,
    );
    let body_stream = http_body_util::StreamBody::new(stream);

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/event-stream")
        .header("Cache-Control", "no-cache")
        .header("Connection", "keep-alive")
        .body(BoxBody::new(body_stream))
        .unwrap()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SseStreamFrom {
    Begin,
    Tail,
}

fn parse_sse_stream_from(query: Option<&str>) -> SseStreamFrom {
    let Some(q) = query else {
        return SseStreamFrom::Begin;
    };
    for part in q.split('&') {
        if let Some(v) = part.strip_prefix("from=") {
            if v.eq_ignore_ascii_case("tail") {
                return SseStreamFrom::Tail;
            }
            return SseStreamFrom::Begin;
        }
    }
    SseStreamFrom::Begin
}

#[derive(Debug, Clone, Copy)]
struct SseStreamOptions {
    from: SseStreamFrom,
    batch_size: usize,
}

fn parse_sse_stream_options(query: Option<&str>) -> SseStreamOptions {
    let from = parse_sse_stream_from(query);
    let mut batch_enabled = false;
    let mut batch_size_override: Option<usize> = None;

    let Some(q) = query else {
        return SseStreamOptions {
            from,
            batch_size: 1,
        };
    };

    for part in q.split('&') {
        if let Some(v) = part.strip_prefix("batch=") {
            if v == "0" || v.eq_ignore_ascii_case("false") {
                batch_enabled = false;
            } else if v == "1" || v.eq_ignore_ascii_case("true") {
                batch_enabled = true;
            }
            continue;
        }
        if let Some(v) = part.strip_prefix("batch_size=") {
            if let Ok(n) = v.parse::<usize>() {
                batch_size_override = Some(n.clamp(1, 1000));
            }
            continue;
        }
    }

    let batch_size = if let Some(n) = batch_size_override {
        n
    } else if batch_enabled && from == SseStreamFrom::Begin {
        200
    } else {
        1
    };

    SseStreamOptions { from, batch_size }
}

fn build_sse_disk_stream(
    state: SharedAdminState,
    connection_id: String,
    body_ref: BodyRef,
    from: SseStreamFrom,
    batch_size: usize,
    tail_bytes: usize,
) -> impl futures_util::Stream<Item = Result<hyper::body::Frame<bytes::Bytes>, hyper::Error>> {
    use tokio_stream::wrappers::ReceiverStream;

    let (tx, rx) = tokio::sync::mpsc::channel::<bytes::Bytes>(64);

    tokio::spawn(async move {
        let _ = stream_sse_events_from_body_ref(
            state,
            &connection_id,
            body_ref,
            from,
            batch_size,
            tail_bytes,
            tx,
        )
        .await;
    });

    ReceiverStream::new(rx).map(|b| Ok::<_, hyper::Error>(hyper::body::Frame::data(b)))
}

async fn stream_sse_events_from_body_ref(
    state: SharedAdminState,
    connection_id: &str,
    body_ref: BodyRef,
    from: SseStreamFrom,
    batch_size: usize,
    tail_bytes: usize,
    tx: tokio::sync::mpsc::Sender<bytes::Bytes>,
) -> Result<(), ()> {
    let mut seq: u64 = 0;
    let mut parser = SseIncrementalParser::new();

    match body_ref {
        BodyRef::Inline { data } => {
            let mut batch = Vec::new();
            let batch_size = batch_size.max(1);
            for raw in split_sse_events_text(&data) {
                seq = seq.saturating_add(1);
                let event = sse_event_from_raw(seq, now_ms(), raw);
                if batch_size <= 1 {
                    let s = sse_json_line(&event);
                    if tx.send(bytes::Bytes::from(s)).await.is_err() {
                        return Ok(());
                    }
                    continue;
                }

                batch.push(event);
                if batch.len() >= batch_size {
                    let s = sse_json_batch_line(&batch);
                    batch.clear();
                    if tx.send(bytes::Bytes::from(s)).await.is_err() {
                        return Ok(());
                    }
                }
            }

            if !batch.is_empty() {
                let s = sse_json_batch_line(&batch);
                let _ = tx.send(bytes::Bytes::from(s)).await;
            }
            Ok(())
        }
        BodyRef::File { path, .. } => {
            let cfg = SseFileStreamConfig {
                state,
                connection_id: connection_id.to_string(),
                path,
                start_offset: 0,
                fixed_end: None,
                from,
                batch_size,
                tail_bytes,
            };
            stream_sse_events_from_file(cfg, &mut seq, &mut parser, tx).await
        }
        BodyRef::FileRange { path, offset, size } => {
            let end = offset.saturating_add(size as u64);
            let cfg = SseFileStreamConfig {
                state,
                connection_id: connection_id.to_string(),
                path,
                start_offset: offset,
                fixed_end: Some(end),
                from,
                batch_size,
                tail_bytes,
            };
            stream_sse_events_from_file(cfg, &mut seq, &mut parser, tx).await
        }
        encoded_ref @ BodyRef::ContentEncoded { .. } => {
            let max_output_bytes = configured_decompress_output_bytes(&state).await;
            let Some(content_encoding) = encoded_ref.content_encoding().map(str::to_string) else {
                return Ok(());
            };
            if !content_encoding_is_supported(&content_encoding) {
                return Ok(());
            }
            let mut decoded = None;
            for _ in 0..80 {
                if let Some(bytes) = load_body_bytes_async(&state, &encoded_ref).await {
                    if let Ok(bytes) =
                        decompress_with_limit(&bytes, &content_encoding, max_output_bytes)
                    {
                        decoded = Some(bytes);
                        break;
                    }
                }
                if state.sse_hub.is_open(connection_id) != Some(true) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
            let Some(decoded) = decoded else {
                return Ok(());
            };
            Box::pin(stream_sse_events_from_body_ref(
                state,
                connection_id,
                BodyRef::Inline {
                    data: String::from_utf8_lossy(&decoded).to_string(),
                },
                from,
                batch_size,
                tail_bytes,
                tx,
            ))
            .await
        }
    }
}

struct SseFileStreamConfig {
    state: SharedAdminState,
    connection_id: String,
    path: String,
    start_offset: u64,
    fixed_end: Option<u64>,
    from: SseStreamFrom,
    batch_size: usize,
    tail_bytes: usize,
}

async fn stream_sse_events_from_file(
    cfg: SseFileStreamConfig,
    seq: &mut u64,
    parser: &mut SseIncrementalParser,
    tx: tokio::sync::mpsc::Sender<bytes::Bytes>,
) -> Result<(), ()> {
    use std::time::Instant;
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    use tokio::time::{sleep, Duration};

    let mut file = match tokio::fs::File::open(&cfg.path).await {
        Ok(f) => f,
        Err(_) => return Ok(()),
    };

    let mut offset = cfg.start_offset;
    if cfg.from == SseStreamFrom::Tail && cfg.fixed_end.is_none() && cfg.tail_bytes > 0 {
        if let Ok(meta) = file.metadata().await {
            let len = meta.len();
            offset = len.saturating_sub(cfg.tail_bytes as u64);
        }
    }

    if file.seek(std::io::SeekFrom::Start(offset)).await.is_err() {
        return Ok(());
    }

    let mut buf = vec![0u8; 8192];
    let mut last_force_flush_refresh = Instant::now();
    let mut closed_eof_retries = 0u8;
    let mut saw_closed = false;
    let mut last_event_was_finish = false;

    let mut batch = Vec::new();
    let batch_size = cfg.batch_size.max(1);

    loop {
        // 详情页 live SSE stream 打开期间，持续续租 force-flush 窗口。
        // 否则长连接超过初始 30s 后，proxy 侧会回到普通缓冲策略，
        // 导致尾部一批事件要等连接关闭 flush 后才在 response-body 里可见。
        if cfg.fixed_end.is_none() && last_force_flush_refresh.elapsed() >= Duration::from_secs(5) {
            cfg.state
                .sse_hub
                .request_force_flush(&cfg.connection_id, 30_000);
            last_force_flush_refresh = Instant::now();
        }

        let is_open = cfg
            .state
            .sse_hub
            .is_open(&cfg.connection_id)
            .unwrap_or(false);
        if !is_open {
            saw_closed = true;
        }
        let end = cfg.fixed_end;

        if let Some(end_pos) = end {
            if offset >= end_pos {
                break;
            }
        }

        let mut to_read = buf.len();
        if let Some(end_pos) = end {
            let remain = (end_pos - offset) as usize;
            to_read = to_read.min(remain);
            if to_read == 0 {
                break;
            }
        }

        let n = match file.read(&mut buf[..to_read]).await {
            Ok(n) => n,
            Err(_) => break,
        };

        if n == 0 {
            if !is_open {
                // 连接关闭时，proxy 侧会在 drop/finish 阶段把最后一批 buffered 数据 flush 到文件。
                // 这里若一看到 EOF + closed 就立即退出，会漏掉刚刚在 finish() 中落盘的尾数据。
                closed_eof_retries = closed_eof_retries.saturating_add(1);
                if closed_eof_retries >= 10 {
                    break;
                }
                sleep(Duration::from_millis(50)).await;
                continue;
            }
            closed_eof_retries = 0;
            sleep(Duration::from_millis(200)).await;
            continue;
        }

        closed_eof_retries = 0;
        offset = offset.saturating_add(n as u64);

        let mut produced = Vec::new();
        parser.push_bytes(&buf[..n], &mut produced);
        for raw in produced {
            *seq = seq.saturating_add(1);
            let event = sse_event_from_raw(*seq, now_ms(), raw);
            last_event_was_finish = event.event.as_deref() == Some("finish");
            if batch_size <= 1 {
                let s = sse_json_line(&event);
                if tx.send(bytes::Bytes::from(s)).await.is_err() {
                    return Ok(());
                }
                continue;
            }

            batch.push(event);
            if batch.len() >= batch_size {
                let s = sse_json_batch_line(&batch);
                batch.clear();
                if tx.send(bytes::Bytes::from(s)).await.is_err() {
                    return Ok(());
                }
            }
        }
    }

    if let Some(raw) = parser.finish() {
        *seq = seq.saturating_add(1);
        let event = sse_event_from_raw(*seq, now_ms(), raw);
        last_event_was_finish = event.event.as_deref() == Some("finish");
        if batch_size <= 1 {
            let s = sse_json_line(&event);
            let _ = tx.send(bytes::Bytes::from(s)).await;
        } else {
            batch.push(event);
        }
    }

    // live 详情流的完成边界应该由“连接真的关闭并且尾部追完”来决定，
    // 而不是依赖 upstream 一定发送特定的 finish 事件。
    if should_emit_synthetic_finish(cfg.fixed_end, saw_closed, last_event_was_finish) {
        *seq = seq.saturating_add(1);
        let finish_event = crate::sse::SseEvent {
            seq: *seq,
            ts: now_ms(),
            id: None,
            event: Some("finish".to_string()),
            retry: None,
            data: String::new(),
            raw: None,
            parse_error: false,
        };
        if batch_size <= 1 {
            let s = sse_json_line(&finish_event);
            let _ = tx.send(bytes::Bytes::from(s)).await;
        } else {
            batch.push(finish_event);
        }
    }

    if batch_size > 1 && !batch.is_empty() {
        let s = sse_json_batch_line(&batch);
        let _ = tx.send(bytes::Bytes::from(s)).await;
    }

    Ok(())
}

fn split_sse_events_text(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for line in input.lines() {
        if line.is_empty() {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            continue;
        }
        current.push_str(line);
        current.push('\n');
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

struct SseIncrementalParser {
    prev_nl: bool,
    buf: Vec<u8>,
}

impl SseIncrementalParser {
    fn new() -> Self {
        Self {
            prev_nl: false,
            buf: Vec::new(),
        }
    }

    fn push_bytes(&mut self, data: &[u8], out: &mut Vec<String>) {
        for &b in data {
            if b == b'\r' {
                continue;
            }
            if b == b'\n' {
                if self.prev_nl {
                    let mut chunk = std::mem::take(&mut self.buf);
                    while matches!(chunk.last(), Some(b'\n')) {
                        chunk.pop();
                    }
                    if !chunk.is_empty() {
                        out.push(String::from_utf8_lossy(&chunk).to_string());
                    }
                    self.prev_nl = false;
                    continue;
                }
                self.buf.push(b'\n');
                self.prev_nl = true;
                continue;
            }
            self.prev_nl = false;
            self.buf.push(b);
        }
    }

    fn finish(&mut self) -> Option<String> {
        let mut chunk = std::mem::take(&mut self.buf);
        while matches!(chunk.last(), Some(b'\n')) {
            chunk.pop();
        }
        if chunk.is_empty() {
            None
        } else {
            Some(String::from_utf8_lossy(&chunk).to_string())
        }
    }
}

fn sse_event_from_raw(seq: u64, ts: u64, raw: String) -> crate::sse::SseEvent {
    let mut event = crate::sse::parse_sse_event(&raw);
    event.seq = seq;
    event.ts = ts;
    event.raw = Some(raw);
    event
}

fn sse_json_line(event: &crate::sse::SseEvent) -> String {
    let data = serde_json::to_string(event)
        .unwrap_or_else(|_| format!(r#"{{"seq":{},"ts":{},"data":""}}"#, event.seq, event.ts));
    format!("id: {}\ndata: {}\n\n", event.seq, data)
}

fn sse_json_batch_line(events: &[crate::sse::SseEvent]) -> String {
    #[derive(serde::Serialize)]
    struct Payload<'a> {
        batch: bool,
        seq: u64,
        ts: u64,
        events: &'a [crate::sse::SseEvent],
    }
    let last_seq = events.last().map(|e| e.seq).unwrap_or(0);
    let data = serde_json::to_string(&Payload {
        batch: true,
        seq: last_seq,
        ts: now_ms(),
        events,
    })
    .unwrap_or_else(|_| "{\"batch\":true,\"seq\":0,\"ts\":0,\"events\":[]}".to_string());
    format!("id: {}\ndata: {}\n\n", last_seq, data)
}

fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

fn should_emit_synthetic_finish(
    fixed_end: Option<u64>,
    saw_closed: bool,
    last_event_was_finish: bool,
) -> bool {
    fixed_end.is_none() && saw_closed && !last_event_was_finish
}

#[cfg(test)]
mod sse_stream_tests {
    use std::io::Write;

    use flate2::{write::GzEncoder, Compression};

    use super::{
        parse_sse_stream_from, parse_sse_stream_options, should_emit_synthetic_finish,
        split_sse_events_text, stream_sse_events_from_body_ref, SseIncrementalParser,
        SseStreamFrom,
    };
    use crate::test_support::TestAdminState;

    #[test]
    fn test_parse_sse_stream_from_default_begin() {
        assert_eq!(parse_sse_stream_from(None), SseStreamFrom::Begin);
        assert_eq!(parse_sse_stream_from(Some("x=1")), SseStreamFrom::Begin);
        assert_eq!(
            parse_sse_stream_from(Some("from=begin")),
            SseStreamFrom::Begin
        );
        assert_eq!(
            parse_sse_stream_from(Some("from=tail")),
            SseStreamFrom::Tail
        );
        assert_eq!(
            parse_sse_stream_from(Some("a=b&from=tail&c=d")),
            SseStreamFrom::Tail
        );
    }

    #[test]
    fn test_parse_sse_stream_options_batch_size() {
        let o = parse_sse_stream_options(None);
        assert_eq!(o.from, SseStreamFrom::Begin);
        assert_eq!(o.batch_size, 1);

        let o = parse_sse_stream_options(Some("from=tail"));
        assert_eq!(o.from, SseStreamFrom::Tail);
        assert_eq!(o.batch_size, 1);

        let o = parse_sse_stream_options(Some("from=begin&batch=1"));
        assert_eq!(o.from, SseStreamFrom::Begin);
        assert_eq!(o.batch_size, 200);

        let o = parse_sse_stream_options(Some("from=begin&batch=0"));
        assert_eq!(o.from, SseStreamFrom::Begin);
        assert_eq!(o.batch_size, 1);

        let o = parse_sse_stream_options(Some("from=begin&batch_size=10"));
        assert_eq!(o.batch_size, 10);

        let o = parse_sse_stream_options(Some("from=begin&batch_size=99999"));
        assert_eq!(o.batch_size, 1000);
    }

    #[test]
    fn test_split_sse_events_text() {
        let input = "data: a\n\ndata: b\n\n";
        let out = split_sse_events_text(input);
        assert_eq!(out.len(), 2);
        assert!(out[0].contains("data: a"));
        assert!(out[1].contains("data: b"));
    }

    #[test]
    fn test_incremental_parser_boundary_and_finish() {
        let mut p = SseIncrementalParser::new();
        let mut out = Vec::new();
        p.push_bytes(b"data: a\n\n", &mut out);
        assert_eq!(out.len(), 1);
        assert!(out[0].contains("data: a"));
        let mut out2 = Vec::new();
        p.push_bytes(b"data: b\n", &mut out2);
        assert!(out2.is_empty());
        let tail = p.finish().unwrap();
        assert!(tail.contains("data: b"));
    }

    #[test]
    fn test_synthetic_finish_policy() {
        assert!(should_emit_synthetic_finish(None, true, false));
        assert!(!should_emit_synthetic_finish(None, true, true));
        assert!(!should_emit_synthetic_finish(None, false, false));
        assert!(!should_emit_synthetic_finish(Some(123), true, false));
    }

    #[tokio::test]
    async fn content_encoded_sse_body_is_decoded_before_event_parsing() {
        let harness = TestAdminState::builder().build();
        let plaintext = b"data: first\n\ndata: second\n\n";
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(plaintext).expect("compress SSE body");
        let compressed = encoder.finish().expect("finish gzip body");
        let body_ref = harness
            .body_store
            .read()
            .store("encoded-sse", "res", &compressed)
            .expect("store encoded SSE body")
            .with_content_encoding(Some("gzip"));
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);

        stream_sse_events_from_body_ref(
            harness.state(),
            "encoded-sse",
            body_ref,
            SseStreamFrom::Begin,
            1,
            0,
            tx,
        )
        .await
        .expect("stream decoded SSE events");

        let mut output = String::new();
        while let Some(chunk) = rx.recv().await {
            output.push_str(&String::from_utf8_lossy(&chunk));
        }
        assert!(output.contains("data: first"), "{output}");
        assert!(output.contains("data: second"), "{output}");
    }
}

async fn query_traffic(req: Request<Incoming>, state: SharedAdminState) -> Response<BoxBody> {
    if state.get_super_performance_mode() {
        return json_response(&empty_query_result());
    }

    let body_bytes = match req.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Failed to read request body: {}", e),
            );
        }
    };

    let params: QueryParams = match serde_json::from_slice(&body_bytes) {
        Ok(p) => p,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("Invalid JSON body: {}", e),
            );
        }
    };

    let service = AdminQueryService::new(state);
    match service.query_traffic_params(params).await {
        Ok(result) => json_response(&result),
        Err(e) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Query failed: {}", e),
        ),
    }
}

async fn list_traffic(req: Request<Incoming>, state: SharedAdminState) -> Response<BoxBody> {
    if state.get_super_performance_mode() {
        return json_response(&empty_query_result());
    }
    let query = req.uri().query().unwrap_or("");
    let params = parse_query_params_from_query_string(query);
    let service = AdminQueryService::new(state);
    match service.query_traffic_params(params).await {
        Ok(result) => json_response(&result),
        Err(e) => {
            tracing::error!("[TRAFFIC_API] Query task failed: {}", e);
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Query failed: {}", e),
            )
        }
    }
}

async fn get_traffic_updates(req: Request<Incoming>, state: SharedAdminState) -> Response<BoxBody> {
    if state.get_super_performance_mode() {
        let response = serde_json::json!({
            "new_records": [],
            "updated_records": [],
            "has_more": false,
            "server_total": 0,
            "server_sequence": 0
        });
        return json_response(&response);
    }
    let query = req.uri().query().unwrap_or("");
    let params = parse_updates_params(query);

    if let Some(ref db_store) = state.traffic_db_store {
        let limit = params.limit.unwrap_or(100);
        let cursor = params.after_seq;
        let pending_ids = params.pending_ids.clone();
        let db_clone = db_store.clone();
        let query_result = tokio::task::spawn_blocking(move || {
            if let Some(cursor) = cursor {
                let query_params = QueryParams {
                    cursor: Some(cursor),
                    limit: Some(limit),
                    direction: crate::traffic_db::Direction::Forward,
                    ..Default::default()
                };
                db_clone.query(&query_params)
            } else {
                db_clone.query_latest_window(limit)
            }
        })
        .await;

        let result = match query_result {
            Ok(r) => r,
            Err(e) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("Query failed: {}", e),
                );
            }
        };

        let mut new_records: Vec<TrafficSummaryCompact> = result.records;
        for record in &mut new_records {
            enrich_compact_frame_info(record, &state);
        }

        let updated_records: Vec<TrafficSummaryCompact> = if !pending_ids.is_empty() {
            let ids: Vec<String> = pending_ids;
            let db_clone = db_store.clone();
            let ids_result = tokio::task::spawn_blocking(move || {
                let id_refs: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
                db_clone.get_by_ids(&id_refs)
            })
            .await;

            match ids_result {
                Ok(mut summaries) => {
                    for summary in &mut summaries {
                        enrich_compact_frame_info(summary, &state);
                    }
                    summaries
                }
                Err(_) => Vec::new(),
            }
        } else {
            Vec::new()
        };

        let response = serde_json::json!({
            "new_records": new_records,
            "updated_records": updated_records,
            "has_more": result.has_more,
            "server_total": result.total,
            "server_sequence": result.server_sequence
        });

        json_response(&response)
    } else {
        error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Traffic database not available",
        )
    }
}

#[derive(Debug, Default)]
struct UpdatesParams {
    after_id: Option<String>,
    after_seq: Option<u64>,
    pending_ids: Vec<String>,
    limit: Option<usize>,
}

fn decode_query_value(value: &str) -> String {
    let value_with_spaces = value.replace('+', " ");
    urlencoding::decode(&value_with_spaces)
        .unwrap_or_default()
        .to_string()
}

fn parse_updates_params(query: &str) -> UpdatesParams {
    let mut params = UpdatesParams::default();

    for pair in query.split('&') {
        if let Some((key, value)) = pair.split_once('=') {
            let value = decode_query_value(value);
            match key {
                "after_id" if !value.is_empty() => {
                    params.after_id = Some(value.to_string());
                }
                "after_seq" | "cursor" => {
                    params.after_seq = value.parse().ok();
                }
                "pending_ids" if !value.is_empty() => {
                    params.pending_ids = value
                        .split(',')
                        .take(MAX_SUBSCRIBED_IDS)
                        .filter_map(|s: &str| {
                            let id = s.to_string();
                            if id.is_empty() || id.len() > MAX_ID_LEN {
                                None
                            } else {
                                Some(id)
                            }
                        })
                        .collect();
                }
                "limit" => {
                    params.limit = value.parse().ok();
                }
                _ => {}
            }
        }
    }

    params
}

fn parse_query_params_from_query_string(query: &str) -> QueryParams {
    let mut params = QueryParams::default();

    for pair in query.split('&') {
        if let Some((key, value)) = pair.split_once('=') {
            let value = decode_query_value(value);
            match key {
                "cursor" => params.cursor = value.parse().ok(),
                "limit" => params.limit = value.parse().ok(),
                "direction" if value == "forward" => {
                    params.direction = crate::traffic_db::Direction::Forward;
                }
                "method" => params.method = Some(value),
                "status" => params.status = value.parse().ok(),
                "status_min" => params.status_min = value.parse().ok(),
                "status_max" => params.status_max = value.parse().ok(),
                "protocol" => params.protocol = Some(value),
                "has_rule_hit" => params.has_rule_hit = value.parse().ok(),
                "is_websocket" => params.is_websocket = value.parse().ok(),
                "is_sse" => params.is_sse = value.parse().ok(),
                "is_h3" => params.is_h3 = value.parse().ok(),
                "is_tunnel" => params.is_tunnel = value.parse().ok(),
                "host" | "host_contains" => params.host_contains = Some(value),
                "url" | "url_contains" => params.url_contains = Some(value),
                "path" | "path_contains" => params.path_contains = Some(value),
                "client_app" => params.client_app = Some(value),
                "client_app_match" => {
                    params.client_app_match = if value.eq_ignore_ascii_case("equals") {
                        crate::traffic_db::TextMatchMode::Equals
                    } else {
                        crate::traffic_db::TextMatchMode::Contains
                    };
                }
                "client_app_empty" => params.client_app_empty = value.parse().ok(),
                "account_name" => params.account_name = Some(value),
                "account_name_match" => {
                    params.account_name_match = if value.eq_ignore_ascii_case("equals") {
                        crate::traffic_db::TextMatchMode::Equals
                    } else {
                        crate::traffic_db::TextMatchMode::Contains
                    };
                }
                "account_name_empty" => params.account_name_empty = value.parse().ok(),
                "client_ip" => params.client_ip = Some(value),
                "client_ip_match" => {
                    params.client_ip_match = if value.eq_ignore_ascii_case("equals") {
                        crate::traffic_db::TextMatchMode::Equals
                    } else {
                        crate::traffic_db::TextMatchMode::Contains
                    };
                }
                "client_ip_empty" => params.client_ip_empty = value.parse().ok(),
                "listener_port" | "port" => params.listener_port = value.parse().ok(),
                "content_type" => params.content_type = Some(value),
                _ => {}
            }
        }
    }

    if params.limit.is_none() {
        params.limit = Some(100);
    }

    params
}

/// Hard upper bound on `?ids=` count for `/api/traffic/batch`. Going higher
/// would let a single relay round-trip stream very large body payloads back to
/// the caller (each line can include base64-encoded request+response bodies).
const BATCH_GET_MAX_IDS: usize = 200;
const BATCH_GET_DEFAULT_MAX_BODY_BYTES: usize = 64 * 1024;

#[derive(Default, Debug)]
struct BatchTrafficParams {
    ids: Vec<String>,
    include_request_body: bool,
    include_response_body: bool,
    include_request_headers: bool,
    include_response_headers: bool,
    max_body_bytes: usize,
}

fn parse_batch_traffic_query(query: Option<&str>) -> Result<BatchTrafficParams, String> {
    let mut params = BatchTrafficParams {
        max_body_bytes: BATCH_GET_DEFAULT_MAX_BODY_BYTES,
        ..Default::default()
    };
    let Some(q) = query else {
        return Err("missing required `ids` query parameter".to_string());
    };
    let mut ids_seen = false;
    for part in q.split('&') {
        if part.is_empty() {
            continue;
        }
        if let Some(v) = part.strip_prefix("ids=") {
            ids_seen = true;
            let decoded =
                urlencoding::decode(v).map_err(|e| format!("invalid url-encoded `ids`: {e}"))?;
            for raw in decoded.split(',') {
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    continue;
                }
                params.ids.push(trimmed.to_string());
            }
        } else if let Some(v) = part.strip_prefix("include=") {
            let decoded = urlencoding::decode(v)
                .map_err(|e| format!("invalid url-encoded `include`: {e}"))?;
            for tok in decoded.split(',') {
                match tok.trim() {
                    "" => {}
                    "request-body" | "req-body" => params.include_request_body = true,
                    "response-body" | "res-body" => params.include_response_body = true,
                    "request-headers" | "req-headers" => params.include_request_headers = true,
                    "response-headers" | "res-headers" => params.include_response_headers = true,
                    "headers" => {
                        params.include_request_headers = true;
                        params.include_response_headers = true;
                    }
                    "bodies" => {
                        params.include_request_body = true;
                        params.include_response_body = true;
                    }
                    other => return Err(format!("unknown include token: {other}")),
                }
            }
        } else if let Some(v) = part.strip_prefix("max_body=") {
            params.max_body_bytes = v
                .parse::<usize>()
                .map_err(|e| format!("invalid max_body: {e}"))?;
        }
    }
    if !ids_seen || params.ids.is_empty() {
        return Err("missing required `ids` query parameter".to_string());
    }
    if params.ids.len() > BATCH_GET_MAX_IDS {
        return Err(format!(
            "`ids` length {} exceeds maximum of {}",
            params.ids.len(),
            BATCH_GET_MAX_IDS
        ));
    }
    Ok(params)
}

/// `GET /api/traffic/batch?ids=A,B,C&include=request-body,response-body,headers&max_body=N`
///
/// Streams `application/x-ndjson`. Each line is either:
///   `{"id":"A","ok":true,"record":{...},"bodies":{...},"headers":{...}}`
/// or, when an id cannot be resolved:
///   `{"id":"A","ok":false,"error":"not_found"}`
async fn batch_traffic(req: Request<Incoming>, state: SharedAdminState) -> Response<BoxBody> {
    let params = match parse_batch_traffic_query(req.uri().query()) {
        Ok(p) => p,
        Err(msg) => return error_response(StatusCode::BAD_REQUEST, &msg),
    };

    let service = AdminQueryService::new(state.clone());
    let mut out = Vec::<u8>::new();
    for id in &params.ids {
        let line = match service.get_traffic_record(id).await {
            Ok(record) => {
                let mut entry = serde_json::json!({
                    "id": id,
                    "ok": true,
                    "record": &record,
                });

                if params.include_request_body || params.include_response_body {
                    let mut bodies = serde_json::Map::new();
                    if params.include_request_body {
                        let body_ref = record
                            .request_body_ref
                            .as_ref()
                            .or(record.raw_request_body_ref.as_ref());
                        if let Some(chunk) = build_batch_body_chunk(
                            &state,
                            body_ref,
                            record.request_content_type.as_deref(),
                            params.max_body_bytes,
                        )
                        .await
                        {
                            bodies.insert("request".to_string(), chunk);
                        }
                    }
                    if params.include_response_body {
                        let body_ref = record
                            .response_body_ref
                            .as_ref()
                            .or(record.raw_response_body_ref.as_ref());
                        if let Some(chunk) = build_batch_body_chunk(
                            &state,
                            body_ref,
                            record.content_type.as_deref(),
                            params.max_body_bytes,
                        )
                        .await
                        {
                            bodies.insert("response".to_string(), chunk);
                        }
                    }
                    if !bodies.is_empty() {
                        entry["bodies"] = serde_json::Value::Object(bodies);
                    }
                }

                if params.include_request_headers || params.include_response_headers {
                    let mut headers = serde_json::Map::new();
                    if params.include_request_headers {
                        headers.insert(
                            "request".to_string(),
                            serde_json::to_value(&record.request_headers)
                                .unwrap_or(serde_json::Value::Null),
                        );
                    }
                    if params.include_response_headers {
                        headers.insert(
                            "response".to_string(),
                            serde_json::to_value(&record.response_headers)
                                .unwrap_or(serde_json::Value::Null),
                        );
                    }
                    if !headers.is_empty() {
                        entry["headers"] = serde_json::Value::Object(headers);
                    }
                }

                entry
            }
            Err(_) => serde_json::json!({
                "id": id,
                "ok": false,
                "error": "not_found",
            }),
        };
        let mut serialized = serde_json::to_vec(&line).unwrap_or_else(|_| b"{}".to_vec());
        serialized.push(b'\n');
        out.extend_from_slice(&serialized);
    }

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/x-ndjson")
        .header("Cache-Control", "no-store")
        .body(full_body(out))
        .unwrap()
}

async fn build_batch_body_chunk(
    state: &SharedAdminState,
    body_ref: Option<&BodyRef>,
    content_type: Option<&str>,
    max_body_bytes: usize,
) -> Option<serde_json::Value> {
    let body_ref = body_ref?;
    let bytes = load_body_bytes_async(state, body_ref).await?;
    let max_output_bytes = configured_decompress_output_bytes(state).await;
    let bytes = decode_stored_body(body_ref, bytes, true, max_output_bytes);
    let original_size = bytes.len();
    let (slice, truncated) = if original_size > max_body_bytes {
        (&bytes[..max_body_bytes], true)
    } else {
        (&bytes[..], false)
    };
    let encoded = base64::engine::general_purpose::STANDARD.encode(slice);
    let mut obj = serde_json::Map::new();
    obj.insert("bytes_b64".to_string(), serde_json::Value::String(encoded));
    obj.insert(
        "size".to_string(),
        serde_json::Value::Number(serde_json::Number::from(original_size)),
    );
    obj.insert("truncated".to_string(), serde_json::Value::Bool(truncated));
    if let Some(ct) = content_type {
        obj.insert(
            "content_type".to_string(),
            serde_json::Value::String(ct.to_string()),
        );
    }
    Some(serde_json::Value::Object(obj))
}

async fn get_traffic_detail(state: SharedAdminState, id: &str) -> Response<BoxBody> {
    let service = AdminQueryService::new(state.clone());
    match service.get_traffic_record(id).await {
        Ok(mut record) => {
            if let Some(ref socket_status) = record.socket_status {
                let total = socket_status.send_bytes + socket_status.receive_bytes;
                if !socket_status.is_open {
                    if record.response_size == 0 && total > 0 {
                        record.response_size = total as usize;
                    }
                    let status = socket_status.clone();
                    let frame_count = record.frame_count;
                    let last_frame_id = record.last_frame_id;
                    let response_size = record.response_size;
                    let record_id = record.id.clone();
                    state.update_traffic_by_id(&record_id, move |record| {
                        record.response_size = response_size;
                        record.socket_status = Some(status.clone());
                        record.frame_count = frame_count;
                        record.last_frame_id = last_frame_id;
                    });
                }
            }
            json_response(&record)
        }
        Err(_) => error_response(
            StatusCode::NOT_FOUND,
            &format!("Traffic record '{}' not found", id),
        ),
    }
}

#[derive(Debug, serde::Deserialize)]
struct ClearTrafficRequest {
    ids: Option<Vec<String>>,
}

fn parse_clear_traffic_request_body(
    body: &[u8],
) -> std::result::Result<ClearTrafficRequest, String> {
    if body.is_empty() {
        return Ok(ClearTrafficRequest { ids: None });
    }
    serde_json::from_slice(body).map_err(|e| format!("Invalid JSON clear traffic request: {e}"))
}

async fn clear_traffic(
    req: Request<Incoming>,
    state: SharedAdminState,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    let body = match req.collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => bytes::Bytes::new(),
    };

    let request = match parse_clear_traffic_request_body(&body) {
        Ok(request) => request,
        Err(e) => {
            tracing::warn!("[CLEAR_TRAFFIC] Failed to parse request body: {}", e);
            return error_response(StatusCode::BAD_REQUEST, &e);
        }
    };

    if let Some(ids) = request.ids {
        if !ids.is_empty() {
            return clear_traffic_by_ids(state, ids, push_manager).await;
        }
    }

    clear_all_traffic(state, push_manager).await
}

async fn clear_traffic_by_ids(
    state: SharedAdminState,
    ids: Vec<String>,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    let active_connection_ids = state.connection_monitor.active_connection_ids();
    let active_set: std::collections::HashSet<&String> = active_connection_ids.iter().collect();

    let ids_to_delete: Vec<String> = ids
        .into_iter()
        .filter(|id| !active_set.contains(id))
        .collect();

    if ids_to_delete.is_empty() {
        return success_response("No traffic records to clear (all are active connections)");
    }

    let count = ids_to_delete.len();

    if let Some(ref db_store) = state.traffic_db_store {
        let db_store_clone = db_store.clone();
        let ids_for_db = ids_to_delete.clone();
        let delete_task = tokio::task::spawn_blocking(move || {
            db_store_clone.delete_by_ids(&ids_for_db);
            Ok(())
        });
        if let Err(e) = join_clear_task(delete_task, "traffic db delete-by-ids").await {
            tracing::error!(error = %e, "[CLEAR_TRAFFIC] Failed to delete traffic records by ids");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e);
        }
    }

    if let Some(ref body_store) = state.body_store {
        let body_store_clone = body_store.clone();
        let ids_for_body = ids_to_delete.clone();
        let delete_task = tokio::task::spawn_blocking(move || {
            body_store_clone
                .write()
                .delete_by_ids(&ids_for_body)
                .map(|_| ())
                .map_err(|e| e.to_string())
        });
        if let Err(e) = join_clear_task(delete_task, "body store delete-by-ids").await {
            tracing::warn!(error = %e, "Failed to delete bodies");
        }
    }

    if let Some(ref frame_store) = state.frame_store {
        let frame_store_clone = frame_store.clone();
        let ids_for_frame = ids_to_delete.clone();
        let delete_task = tokio::task::spawn_blocking(move || {
            frame_store_clone
                .delete_by_ids(&ids_for_frame)
                .map(|_| ())
                .map_err(|e| e.to_string())
        });
        if let Err(e) = join_clear_task(delete_task, "frame store delete-by-ids").await {
            tracing::warn!(error = %e, "Failed to delete frames");
        }
    }

    if let Some(ref ws_payload_store) = state.ws_payload_store {
        let ws_payload_store_clone = ws_payload_store.clone();
        let ids_for_payload = ids_to_delete.clone();
        let delete_task = tokio::task::spawn_blocking(move || {
            ws_payload_store_clone
                .delete_by_ids(&ids_for_payload)
                .map(|_| ())
                .map_err(|e| e.to_string())
        });
        if let Err(e) = join_clear_task(delete_task, "ws payload store delete-by-ids").await {
            tracing::warn!(error = %e, "Failed to delete ws payloads");
        }
    }

    if let Some(pm) = push_manager {
        pm.invalidate_overview_cache();
        pm.broadcast_traffic_deleted(ids_to_delete.clone());
    }

    tracing::info!("[CLEAR_TRAFFIC] Deleted {} traffic records", count);
    success_response(&format!("{} traffic records cleared successfully", count))
}

#[cfg(test)]
mod clear_traffic_request_tests {
    use super::parse_clear_traffic_request_body;

    #[test]
    fn malformed_clear_request_is_rejected() {
        let err = parse_clear_traffic_request_body(br#"{"ids":["one""#)
            .expect_err("malformed JSON must not fall back to clear-all");
        assert!(err.contains("Invalid JSON clear traffic request"));
    }

    #[test]
    fn empty_clear_request_keeps_clear_all_semantics() {
        let request = parse_clear_traffic_request_body(b"").expect("empty body is clear-all");
        assert!(request.ids.is_none());
    }

    #[test]
    fn ids_clear_request_parses_ids() {
        let request =
            parse_clear_traffic_request_body(br#"{"ids":["a","b"]}"#).expect("valid ids body");
        assert_eq!(
            request.ids.as_deref(),
            Some(&["a".to_string(), "b".to_string()][..])
        );
    }
}

async fn clear_all_traffic(
    state: SharedAdminState,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    state.connection_monitor.clear();

    if let Some(ref db_store) = state.traffic_db_store {
        let db_store_clone = db_store.clone();
        let clear_task = tokio::task::spawn_blocking(move || {
            db_store_clone.clear();
            Ok(())
        });
        if let Err(e) = join_clear_task(clear_task, "traffic db clear-all").await {
            tracing::error!(error = %e, "[CLEAR_TRAFFIC] Failed to clear traffic records");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e);
        }
    }

    if let Some(ref body_store) = state.body_store {
        let body_store_clone = body_store.clone();
        let clear_task = tokio::task::spawn_blocking(move || {
            body_store_clone
                .write()
                .clear()
                .map(|_| ())
                .map_err(|e| e.to_string())
        });
        if let Err(e) = join_clear_task(clear_task, "body store clear-all").await {
            tracing::warn!(error = %e, "Failed to clear body store");
        }
    }

    if let Some(ref frame_store) = state.frame_store {
        let frame_store_clone = frame_store.clone();
        let clear_task = tokio::task::spawn_blocking(move || {
            frame_store_clone
                .clear()
                .map(|_| ())
                .map_err(|e| e.to_string())
        });
        if let Err(e) = join_clear_task(clear_task, "frame store clear-all").await {
            tracing::warn!(error = %e, "Failed to clear frame store");
        }
    }

    if let Some(ref ws_payload_store) = state.ws_payload_store {
        let ws_payload_store_clone = ws_payload_store.clone();
        let clear_task = tokio::task::spawn_blocking(move || {
            ws_payload_store_clone
                .clear()
                .map(|_| ())
                .map_err(|e| e.to_string())
        });
        if let Err(e) = join_clear_task(clear_task, "ws payload store clear-all").await {
            tracing::warn!(error = %e, "Failed to clear ws payload store");
        }
    }

    if let Some(pm) = push_manager {
        pm.invalidate_overview_cache();
        pm.notify_traffic_statistics_changed();
    }

    success_response("All traffic data cleared successfully")
}

async fn get_request_body(
    state: SharedAdminState,
    id: &str,
    query: Option<&str>,
) -> Response<BoxBody> {
    let record = if let Some(ref db_store) = state.traffic_db_store {
        let db_clone = db_store.clone();
        let id_owned = id.to_string();
        tokio::task::spawn_blocking(move || db_clone.get_by_id(&id_owned))
            .await
            .unwrap_or_default()
    } else {
        None
    };

    match record {
        Some(record) => {
            let want_raw = query_wants_raw(query);
            let body_ref = if want_raw {
                record
                    .raw_request_body_ref
                    .as_ref()
                    .or(record.request_body_ref.as_ref())
            } else {
                record.request_body_ref.as_ref()
            };

            if let Some(body_ref) = body_ref {
                get_body_content_async(&state, body_ref, query_wants_base64(query), !want_raw).await
            } else {
                json_response(&serde_json::json!({
                    "success": true,
                    "data": null
                }))
            }
        }
        None => error_response(
            StatusCode::NOT_FOUND,
            &format!("Traffic record '{}' not found", id),
        ),
    }
}

async fn get_response_body(
    state: SharedAdminState,
    id: &str,
    query: Option<&str>,
) -> Response<BoxBody> {
    let record = if let Some(ref db_store) = state.traffic_db_store {
        let db_clone = db_store.clone();
        let id_owned = id.to_string();
        tokio::task::spawn_blocking(move || db_clone.get_by_id(&id_owned))
            .await
            .unwrap_or_default()
    } else {
        None
    };

    match record {
        Some(record) => {
            let want_raw = query_wants_raw(query);
            let body_ref = if want_raw {
                record
                    .raw_response_body_ref
                    .as_ref()
                    .or(record.response_body_ref.as_ref())
            } else {
                record.response_body_ref.as_ref()
            };

            if let Some(body_ref) = body_ref {
                get_body_content_async(&state, body_ref, query_wants_base64(query), !want_raw).await
            } else {
                json_response(&serde_json::json!({
                    "success": true,
                    "data": null
                }))
            }
        }
        None => error_response(
            StatusCode::NOT_FOUND,
            &format!("Traffic record '{}' not found", id),
        ),
    }
}

async fn get_response_body_content(
    state: SharedAdminState,
    id: &str,
    query: Option<&str>,
) -> Response<BoxBody> {
    let record = if let Some(ref db_store) = state.traffic_db_store {
        let db_clone = db_store.clone();
        let id_owned = id.to_string();
        tokio::task::spawn_blocking(move || db_clone.get_by_id(&id_owned))
            .await
            .unwrap_or_default()
    } else {
        None
    };

    match record {
        Some(record) => {
            let want_raw = query_wants_raw(query);
            let body_ref = if want_raw {
                record
                    .raw_response_body_ref
                    .as_ref()
                    .or(record.response_body_ref.as_ref())
            } else {
                record.response_body_ref.as_ref()
            };

            if let Some(body_ref) = body_ref {
                get_body_bytes_async(
                    &state,
                    body_ref,
                    record
                        .content_type
                        .as_deref()
                        .unwrap_or("application/octet-stream"),
                    !want_raw,
                )
                .await
            } else {
                error_response(
                    StatusCode::NOT_FOUND,
                    &format!("Traffic response body '{}' not found", id),
                )
            }
        }
        None => error_response(
            StatusCode::NOT_FOUND,
            &format!("Traffic record '{}' not found", id),
        ),
    }
}

async fn load_body_bytes_async(state: &SharedAdminState, body_ref: &BodyRef) -> Option<Vec<u8>> {
    match body_ref.storage_ref() {
        BodyRef::Inline { data } => Some(data.as_bytes().to_vec()),
        BodyRef::File { .. } | BodyRef::FileRange { .. } => {
            if let Some(ref body_store) = state.body_store {
                let body_store_clone = body_store.clone();
                let body_ref_clone = body_ref.clone();
                tokio::task::spawn_blocking(move || {
                    let store = body_store_clone.read();
                    store.load_bytes(&body_ref_clone)
                })
                .await
                .ok()
                .flatten()
            } else {
                None
            }
        }
        BodyRef::ContentEncoded { .. } => unreachable!("storage_ref removes encoding wrappers"),
    }
}

async fn configured_decompress_output_bytes(state: &SharedAdminState) -> usize {
    match state.config_manager.as_ref() {
        Some(config_manager) => {
            config_manager
                .config()
                .await
                .sandbox
                .limits
                .max_decompress_output_bytes
        }
        None => DEFAULT_MAX_DECOMPRESSED_BODY_BYTES,
    }
}

fn decode_stored_body(
    body_ref: &BodyRef,
    bytes: Vec<u8>,
    decode_content_encoding: bool,
    max_output_bytes: usize,
) -> Vec<u8> {
    if decode_content_encoding {
        decode_content_encoded_body_with_limit(bytes, body_ref.content_encoding(), max_output_bytes)
    } else {
        bytes
    }
}

async fn get_body_content_async(
    state: &SharedAdminState,
    body_ref: &BodyRef,
    base64_output: bool,
    decode_content_encoding: bool,
) -> Response<BoxBody> {
    let max_output_bytes = configured_decompress_output_bytes(state).await;
    let decode =
        |bytes| decode_stored_body(body_ref, bytes, decode_content_encoding, max_output_bytes);
    if base64_output {
        return match load_body_bytes_async(state, body_ref).await {
            Some(bytes) => {
                let bytes = decode(bytes);
                json_response(&serde_json::json!({
                "success": true,
                "data": String::from_utf8_lossy(&bytes),
                "data_base64": base64::engine::general_purpose::STANDARD.encode(&bytes),
                "encoding": "base64",
                "size": bytes.len()
                }))
            }
            None => error_response(StatusCode::NOT_FOUND, "Body content not found"),
        };
    }

    match load_body_bytes_async(state, body_ref).await {
        Some(bytes) => {
            let bytes = decode(bytes);
            json_response(&serde_json::json!({
                "success": true,
                "data": String::from_utf8_lossy(&bytes),
                "encoding": "text",
                "size": bytes.len()
            }))
        }
        None => match body_ref.storage_ref() {
            BodyRef::File { path, size } | BodyRef::FileRange { path, size, .. }
                if state.body_store.is_none() =>
            {
                json_response(&serde_json::json!({
                    "success": false,
                    "error": "Body store not configured",
                    "path": path,
                    "size": size
                }))
            }
            BodyRef::File { path, .. } | BodyRef::FileRange { path, .. } => error_response(
                StatusCode::NOT_FOUND,
                &format!("Body file not found: {path}"),
            ),
            BodyRef::Inline { .. } => {
                error_response(StatusCode::NOT_FOUND, "Body content not found")
            }
            BodyRef::ContentEncoded { .. } => {
                unreachable!("storage_ref removes encoding wrappers")
            }
        },
    }
}

async fn get_body_bytes_async(
    state: &SharedAdminState,
    body_ref: &BodyRef,
    content_type: &str,
    decode_content_encoding: bool,
) -> Response<BoxBody> {
    let max_output_bytes = configured_decompress_output_bytes(state).await;
    match load_body_bytes_async(state, body_ref).await {
        Some(bytes) => {
            let bytes =
                decode_stored_body(body_ref, bytes, decode_content_encoding, max_output_bytes);
            Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", content_type)
                .header("Cache-Control", "no-store")
                .body(full_body(bytes))
                .unwrap()
        }
        None => error_response(StatusCode::NOT_FOUND, "Body content not found"),
    }
}

#[cfg(test)]
mod stored_body_tests {
    use std::io::Write;

    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use bifrost_storage::{SandboxConfigUpdate, SandboxLimitsConfigUpdate};
    use flate2::{write::GzEncoder, Compression};
    use http_body_util::BodyExt;

    use super::{decode_stored_body, get_body_content_async, BodyRef};
    use crate::test_support::TestAdminState;

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(bytes).expect("write gzip fixture");
        encoder.finish().expect("finish gzip fixture")
    }

    #[test]
    fn only_decodes_refs_marked_as_content_encoded() {
        let application_gzip = gzip(b"application payload");
        let wire_body = gzip(&application_gzip);
        let decoded_ref = BodyRef::File {
            path: "decoded-body".to_string(),
            size: application_gzip.len(),
        };
        let encoded_ref = decoded_ref.clone().with_content_encoding(Some("gzip"));

        assert_eq!(
            decode_stored_body(
                &decoded_ref,
                application_gzip.clone(),
                true,
                super::DEFAULT_MAX_DECOMPRESSED_BODY_BYTES,
            ),
            application_gzip,
            "an already-decoded HTTP representation must keep its application gzip layer"
        );
        assert_eq!(
            decode_stored_body(
                &encoded_ref,
                wire_body.clone(),
                true,
                super::DEFAULT_MAX_DECOMPRESSED_BODY_BYTES,
            ),
            application_gzip,
            "a marked wire body must lose exactly one HTTP gzip layer"
        );
        assert_eq!(
            decode_stored_body(
                &encoded_ref,
                wire_body.clone(),
                false,
                super::DEFAULT_MAX_DECOMPRESSED_BODY_BYTES,
            ),
            wire_body,
            "raw=1 must preserve the captured wire body"
        );
    }

    #[tokio::test]
    async fn configured_decompression_limit_is_honored_by_body_reads() {
        let harness = TestAdminState::builder().build();
        let plaintext = vec![b'a'; 1024];
        let compressed = gzip(&plaintext);
        let body_ref = harness
            .body_store
            .read()
            .store("configured-limit", "res", &compressed)
            .expect("store compressed body")
            .with_content_encoding(Some("gzip"));
        harness
            .config_manager
            .update_sandbox_config(SandboxConfigUpdate {
                limits: Some(SandboxLimitsConfigUpdate {
                    max_decompress_output_bytes: Some(plaintext.len() - 1),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .await
            .expect("lower decompression limit");

        let response = get_body_content_async(&harness.state(), &body_ref, true, true).await;
        let payload: serde_json::Value = serde_json::from_slice(
            &response
                .into_body()
                .collect()
                .await
                .expect("collect response")
                .to_bytes(),
        )
        .expect("parse body response");

        assert_eq!(
            STANDARD
                .decode(payload["data_base64"].as_str().expect("base64 body"))
                .expect("decode response body"),
            compressed,
            "an over-limit body must fall back to the original wire bytes"
        );
    }
}

#[cfg(test)]
mod batch_query_tests {
    use std::io::Write;

    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use flate2::{write::GzEncoder, Compression};

    use super::{
        build_batch_body_chunk, parse_batch_traffic_query, BATCH_GET_DEFAULT_MAX_BODY_BYTES,
        BATCH_GET_MAX_IDS,
    };
    use crate::test_support::TestAdminState;

    #[test]
    fn parse_basic_ids() {
        let p = parse_batch_traffic_query(Some("ids=a,b,c")).expect("ok");
        assert_eq!(p.ids, vec!["a", "b", "c"]);
        assert_eq!(p.max_body_bytes, BATCH_GET_DEFAULT_MAX_BODY_BYTES);
        assert!(!p.include_request_body);
        assert!(!p.include_response_body);
        assert!(!p.include_request_headers);
        assert!(!p.include_response_headers);
    }

    #[test]
    fn parse_include_aliases_and_shortcuts() {
        let p =
            parse_batch_traffic_query(Some("ids=x&include=req-body,res-body,headers")).expect("ok");
        assert!(p.include_request_body);
        assert!(p.include_response_body);
        assert!(p.include_request_headers);
        assert!(p.include_response_headers);

        let p2 = parse_batch_traffic_query(Some("ids=x&include=bodies")).expect("ok");
        assert!(p2.include_request_body);
        assert!(p2.include_response_body);
        assert!(!p2.include_request_headers);
    }

    #[test]
    fn parse_max_body() {
        let p = parse_batch_traffic_query(Some("ids=a&max_body=2048")).expect("ok");
        assert_eq!(p.max_body_bytes, 2048);
    }

    #[test]
    fn parse_missing_ids_is_error() {
        assert!(parse_batch_traffic_query(None).is_err());
        assert!(parse_batch_traffic_query(Some("foo=bar")).is_err());
        assert!(parse_batch_traffic_query(Some("ids=")).is_err());
    }

    #[test]
    fn parse_unknown_include_token_errors() {
        let err = parse_batch_traffic_query(Some("ids=a&include=mystery-token"))
            .expect_err("should reject unknown token");
        assert!(err.contains("unknown include token"));
    }

    #[test]
    fn parse_over_limit_ids_rejected() {
        // Build ids=1,2,...,N where N = BATCH_GET_MAX_IDS + 1.
        let n = BATCH_GET_MAX_IDS + 1;
        let ids: String = (0..n).map(|i| i.to_string()).collect::<Vec<_>>().join(",");
        let q = format!("ids={}", ids);
        let err = parse_batch_traffic_query(Some(&q)).expect_err("should reject");
        assert!(err.contains("exceeds maximum"));
    }

    #[test]
    fn parse_trims_empty_segments() {
        let p = parse_batch_traffic_query(Some("ids=a,,b,,c,")).expect("ok");
        assert_eq!(p.ids, vec!["a", "b", "c"]);
    }

    #[tokio::test]
    async fn batch_body_chunk_decodes_content_encoded_references() {
        let harness = TestAdminState::builder().build();
        let plaintext = br#"{"batch":"decoded plaintext"}"#;
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(plaintext).expect("compress batch body");
        let compressed = encoder.finish().expect("finish gzip body");
        let body_ref = harness
            .body_store
            .read()
            .store("encoded-batch", "req", &compressed)
            .expect("store encoded batch body")
            .with_content_encoding(Some("gzip"));

        let chunk = build_batch_body_chunk(
            &harness.state(),
            Some(&body_ref),
            Some("application/json"),
            usize::MAX,
        )
        .await
        .expect("build batch body chunk");

        assert_eq!(
            STANDARD
                .decode(chunk["bytes_b64"].as_str().expect("base64 body"))
                .expect("decode included body"),
            plaintext
        );
        assert_eq!(chunk["size"], plaintext.len());
        assert_eq!(chunk["truncated"], false);
    }
}

/// GET /api/traffic/{id}/auth-status — JWT/Cookie 登录态诊断。
async fn get_traffic_auth_status(state: SharedAdminState, id: &str) -> Response<BoxBody> {
    use crate::auth_inspect::build_auth_summary;
    let service = AdminQueryService::new(state.clone());
    let record = match service.get_traffic_record(id).await {
        Ok(r) => r,
        Err(_) => {
            return error_response(
                StatusCode::NOT_FOUND,
                &format!("Traffic record '{}' not found", id),
            )
        }
    };

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    let mut merged: Vec<(String, String)> = Vec::new();
    if let Some(h) = record.request_headers.as_ref() {
        merged.extend(h.iter().cloned());
    }
    if let Some(h) = record.original_request_headers.as_ref() {
        merged.extend(h.iter().cloned());
    }
    if let Some(h) = record.response_headers.as_ref() {
        merged.extend(h.iter().cloned());
    }
    if let Some(h) = record.original_response_headers.as_ref() {
        merged.extend(h.iter().cloned());
    }

    let host = if record.host.is_empty() {
        record.actual_host.clone().unwrap_or_default()
    } else {
        record.host.clone()
    };

    let summary = build_auth_summary(&merged, &host, now_ms);
    json_response(&summary)
}

#[cfg(test)]
mod auth_status_tests {
    use super::*;
    use crate::auth_inspect::AuthSummary;

    fn make_jwt(exp: i64, sub: &str) -> String {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = URL_SAFE_NO_PAD.encode(b"{\"alg\":\"none\"}");
        let payload = serde_json::json!({"exp": exp, "sub": sub});
        let payload_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
        format!("{header}.{payload_b64}.sig")
    }

    #[test]
    fn auth_summary_serializes_with_expected_fields() {
        // 验证 AuthSummary 的字段命名（防止意外重命名破坏 API 契约）。
        let jwt = make_jwt(9_999_999_999, "u-1");
        let headers = vec![("Authorization".to_string(), format!("Bearer {jwt}"))];
        let s = crate::auth_inspect::build_auth_summary(&headers, "example.com", 1_700_000_000_000);
        let v = serde_json::to_value(&s).unwrap();
        assert!(v.get("host").is_some());
        assert!(v.get("has_jwt").is_some());
        assert!(v.get("has_cookie").is_some());
        assert!(v.get("jwt_exp_ms").is_some());
        assert!(v.get("jwt_user_id").is_some());
        assert!(v.get("cookie_exp_ms").is_some());
        assert!(v.get("valid_at_ms").is_some());
        assert!(v.get("valid").is_some());

        // 反序列化往返。
        let parsed: AuthSummary = serde_json::from_value(v).unwrap();
        assert_eq!(parsed, s);
    }
}

// =============================================================================
// P2-6: traffic export / replay handlers
// =============================================================================

async fn get_traffic_record_async(
    state: &SharedAdminState,
    id: &str,
) -> Option<crate::traffic::TrafficRecord> {
    if let Some(ref db_store) = state.traffic_db_store {
        let db_clone = db_store.clone();
        let id_owned = id.to_string();
        tokio::task::spawn_blocking(move || db_clone.get_by_id(&id_owned))
            .await
            .ok()
            .flatten()
    } else {
        None
    }
}

fn parse_export_query(query: Option<&str>) -> crate::replay::ExportFormat {
    let mut format = crate::replay::ExportFormat::Curl;
    if let Some(q) = query {
        for part in q.split('&') {
            if let Some(v) = part.strip_prefix("format=") {
                if let Some(f) = crate::replay::ExportFormat::parse(v) {
                    format = f;
                }
            }
        }
    }
    format
}

async fn get_traffic_export(
    state: SharedAdminState,
    id: &str,
    query: Option<&str>,
) -> Response<BoxBody> {
    let format = parse_export_query(query);
    let record = match get_traffic_record_async(&state, id).await {
        Some(r) => r,
        None => {
            return error_response(
                StatusCode::NOT_FOUND,
                &format!("Traffic record '{}' not found", id),
            )
        }
    };
    let body = if let Some(body_ref) = record
        .raw_request_body_ref
        .as_ref()
        .or(record.request_body_ref.as_ref())
    {
        load_body_bytes_async(&state, body_ref).await
    } else {
        None
    };
    let headers = record.request_headers.clone().unwrap_or_default();
    let opts = crate::replay::ExportOptions { format };
    let text = crate::replay::export_request(
        &record.method,
        &record.url,
        &headers,
        body.as_deref(),
        &opts,
    );
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/plain; charset=utf-8")
        .header("Cache-Control", "no-store")
        .body(full_body(text.into_bytes()))
        .unwrap()
}

async fn post_traffic_replay(
    req: Request<Incoming>,
    state: SharedAdminState,
    id: &str,
) -> Response<BoxBody> {
    let body_bytes = match req.collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => {
            return error_response(StatusCode::BAD_REQUEST, "Failed to read request body");
        }
    };
    let opts: crate::replay::ReplayOptions = if body_bytes.is_empty() {
        crate::replay::ReplayOptions::default()
    } else {
        match serde_json::from_slice(&body_bytes) {
            Ok(v) => v,
            Err(e) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    &format!("Invalid JSON replay options: {e}"),
                );
            }
        }
    };
    let record = match get_traffic_record_async(&state, id).await {
        Some(r) => r,
        None => {
            return error_response(
                StatusCode::NOT_FOUND,
                &format!("Traffic record '{}' not found", id),
            );
        }
    };
    let body = if let Some(body_ref) = record
        .raw_request_body_ref
        .as_ref()
        .or(record.request_body_ref.as_ref())
    {
        load_body_bytes_async(&state, body_ref)
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let headers = record.request_headers.clone().unwrap_or_default();

    // refresh-auth：扫最近 N=200 个 traffic 记录，找同 host 的最新认证 header。
    let auth_candidates = if opts.refresh_auth_enabled() {
        let target_host = opts
            .auth_source_host
            .clone()
            .unwrap_or_else(|| record.host.clone())
            .to_ascii_lowercase();
        collect_auth_candidates(&state, &target_host, &record.id, 200).await
    } else {
        Vec::new()
    };

    let client = crate::replay::ReqwestClient;
    match crate::replay::replay_request(
        &client,
        &record.method,
        &record.url,
        &headers,
        &body,
        &opts,
        &auth_candidates,
    )
    .await
    {
        Ok(result) => json_response(&serde_json::json!({
            "success": true,
            "data": {
                "status": result.status,
                "duration_ms": result.duration_ms,
                "request": {
                    "method": record.method,
                    "url": record.url,
                },
                "response": {
                    "status": result.status,
                    "headers": result.headers,
                    "body_b64": result.body_b64,
                },
                "auth_refresh": result.auth_refresh,
                // 兼容旧响应字段：CLI human render 直接取这几个 key。
                "headers": result.headers,
                "body_b64": result.body_b64,
            },
        })),
        Err(e) => json_response(&serde_json::json!({
            "success": false,
            "error": e,
        })),
    }
}

async fn collect_auth_candidates(
    state: &SharedAdminState,
    target_host_lc: &str,
    exclude_id: &str,
    n: usize,
) -> Vec<crate::replay::AuthCandidate> {
    let Some(ref db_store) = state.traffic_db_store else {
        return Vec::new();
    };
    // 1. 取最近 N 条 compact 列表，按 host 粗筛。
    let db_clone = db_store.clone();
    let summaries = match tokio::task::spawn_blocking(move || db_clone.query_latest_window(n)).await
    {
        Ok(r) => r.records,
        Err(_) => return Vec::new(),
    };
    let target = target_host_lc.to_string();
    let exclude = exclude_id.to_string();
    let host_matches: Vec<(String, u64)> = summaries
        .into_iter()
        .filter(|s| s.id != exclude && s.h.to_ascii_lowercase() == target)
        .map(|s| (s.id, s.seq))
        .collect();

    // 2. 对每个粗筛命中拉完整记录，提取请求 header。
    let mut out: Vec<crate::replay::AuthCandidate> = Vec::new();
    for (id, seq) in host_matches {
        let db_clone = db_store.clone();
        let id_owned = id.clone();
        let full = match tokio::task::spawn_blocking(move || db_clone.get_by_id(&id_owned)).await {
            Ok(r) => r,
            Err(_) => continue,
        };
        let Some(rec) = full else { continue };
        let Some(headers) = rec.request_headers.clone() else {
            continue;
        };
        out.push(crate::replay::AuthCandidate {
            id,
            host: rec.host.clone(),
            headers,
            recency: seq,
        });
    }
    out
}
