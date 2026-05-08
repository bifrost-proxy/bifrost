use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use tokio_stream::StreamExt;

use base64::Engine as _;

use super::frames::{get_frame_detail, get_frames, subscribe_frames, unsubscribe_frames};
use super::{
    error_response, full_body, json_response, method_not_allowed, success_response, BoxBody,
};
use crate::body_store::BodyRef;
use crate::push::{SharedPushManager, MAX_ID_LEN, MAX_SUBSCRIBED_IDS};
use crate::query_service::AdminQueryService;
use crate::state::{AdminState, SharedAdminState};
use crate::traffic_db::{QueryParams, TrafficSummaryCompact};

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
    use super::{query_wants_base64, query_wants_raw};

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
    use super::{
        parse_sse_stream_from, parse_sse_stream_options, should_emit_synthetic_finish,
        split_sse_events_text, SseIncrementalParser, SseStreamFrom,
    };

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
}

async fn query_traffic(req: Request<Incoming>, state: SharedAdminState) -> Response<BoxBody> {
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

async fn clear_traffic(
    req: Request<Incoming>,
    state: SharedAdminState,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    let body = match req.collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => bytes::Bytes::new(),
    };

    let request: ClearTrafficRequest = if body.is_empty() {
        ClearTrafficRequest { ids: None }
    } else {
        match serde_json::from_slice(&body) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("[CLEAR_TRAFFIC] Failed to parse request body: {}", e);
                ClearTrafficRequest { ids: None }
            }
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
        let _delete_task = tokio::task::spawn_blocking(move || {
            db_store_clone.delete_by_ids(&ids_for_db);
        });
    }

    if let Some(ref body_store) = state.body_store {
        let body_store_clone = body_store.clone();
        let ids_for_body = ids_to_delete.clone();
        let _delete_task = tokio::task::spawn_blocking(move || {
            if let Err(e) = body_store_clone.write().delete_by_ids(&ids_for_body) {
                tracing::warn!("Failed to delete bodies: {}", e);
            }
        });
    }

    if let Some(ref frame_store) = state.frame_store {
        let frame_store_clone = frame_store.clone();
        let ids_for_frame = ids_to_delete.clone();
        let _delete_task = tokio::task::spawn_blocking(move || {
            if let Err(e) = frame_store_clone.delete_by_ids(&ids_for_frame) {
                tracing::warn!("Failed to delete frames: {}", e);
            }
        });
    }

    if let Some(ref ws_payload_store) = state.ws_payload_store {
        let ws_payload_store_clone = ws_payload_store.clone();
        let ids_for_payload = ids_to_delete.clone();
        let _delete_task = tokio::task::spawn_blocking(move || {
            if let Err(e) = ws_payload_store_clone.delete_by_ids(&ids_for_payload) {
                tracing::warn!("Failed to delete ws payloads: {}", e);
            }
        });
    }

    if let Some(pm) = push_manager {
        pm.invalidate_overview_cache();
        pm.broadcast_traffic_deleted(ids_to_delete.clone());
    }

    tracing::info!("[CLEAR_TRAFFIC] Deleted {} traffic records", count);
    success_response(&format!("{} traffic records cleared successfully", count))
}

async fn clear_all_traffic(
    state: SharedAdminState,
    push_manager: Option<SharedPushManager>,
) -> Response<BoxBody> {
    let active_connection_ids = state.connection_monitor.active_connection_ids();

    if let Some(ref db_store) = state.traffic_db_store {
        let db_store_clone = db_store.clone();
        let active_ids_for_db = active_connection_ids.clone();
        let _clear_task = tokio::task::spawn_blocking(move || {
            db_store_clone.clear_with_active_ids(&active_ids_for_db);
        });
    }

    if let Some(ref body_store) = state.body_store {
        let body_store_clone = body_store.clone();
        let _clear_task = tokio::task::spawn_blocking(move || {
            if let Err(e) = body_store_clone.write().clear() {
                tracing::warn!("Failed to clear body store: {}", e);
            }
        });
    }

    if let Some(ref frame_store) = state.frame_store {
        let frame_store_clone = frame_store.clone();
        let _clear_task = tokio::task::spawn_blocking(move || {
            if let Err(e) = frame_store_clone.clear() {
                tracing::warn!("Failed to clear frame store: {}", e);
            }
        });
    }

    if let Some(ref ws_payload_store) = state.ws_payload_store {
        let ws_payload_store_clone = ws_payload_store.clone();
        let _clear_task = tokio::task::spawn_blocking(move || {
            if let Err(e) = ws_payload_store_clone.clear() {
                tracing::warn!("Failed to clear ws payload store: {}", e);
            }
        });
    }

    state.connection_monitor.clear();

    if let Some(pm) = push_manager {
        pm.invalidate_overview_cache();
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
                get_body_content_async(&state, body_ref, query_wants_base64(query)).await
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
                get_body_content_async(&state, body_ref, query_wants_base64(query)).await
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
    match body_ref {
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
    }
}

async fn get_body_content_async(
    state: &SharedAdminState,
    body_ref: &BodyRef,
    base64_output: bool,
) -> Response<BoxBody> {
    if base64_output {
        return match load_body_bytes_async(state, body_ref).await {
            Some(bytes) => json_response(&serde_json::json!({
                "success": true,
                "data": String::from_utf8_lossy(&bytes),
                "data_base64": base64::engine::general_purpose::STANDARD.encode(&bytes),
                "encoding": "base64",
                "size": bytes.len()
            })),
            None => error_response(StatusCode::NOT_FOUND, "Body content not found"),
        };
    }

    match body_ref {
        BodyRef::Inline { data } => json_response(&serde_json::json!({
            "success": true,
            "data": data,
            "encoding": "text",
            "size": data.len()
        })),
        BodyRef::File { path, size } | BodyRef::FileRange { path, size, .. } => {
            if let Some(ref body_store) = state.body_store {
                let body_store_clone = body_store.clone();
                let body_ref_clone = body_ref.clone();
                let path_clone = path.clone();

                let data = tokio::task::spawn_blocking(move || {
                    let store = body_store_clone.read();
                    store.load(&body_ref_clone)
                })
                .await
                .ok()
                .flatten();

                match data {
                    Some(content) => json_response(&serde_json::json!({
                        "success": true,
                        "data": content,
                        "encoding": "text",
                        "size": size
                    })),
                    None => error_response(
                        StatusCode::NOT_FOUND,
                        &format!("Body file not found: {}", path_clone),
                    ),
                }
            } else {
                json_response(&serde_json::json!({
                    "success": false,
                    "error": "Body store not configured",
                    "path": path,
                    "size": size
                }))
            }
        }
    }
}

async fn get_body_bytes_async(
    state: &SharedAdminState,
    body_ref: &BodyRef,
    content_type: &str,
) -> Response<BoxBody> {
    match load_body_bytes_async(state, body_ref).await {
        Some(bytes) => Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", content_type)
            .header("Cache-Control", "no-store")
            .body(full_body(bytes))
            .unwrap(),
        None => error_response(StatusCode::NOT_FOUND, "Body content not found"),
    }
}
