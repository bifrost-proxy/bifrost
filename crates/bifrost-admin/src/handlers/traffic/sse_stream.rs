use super::*;

mod content_encoded_sse;
use content_encoded_sse::stream_content_encoded_sse_events;

pub(super) async fn subscribe_sse_stream(
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
        encoded_ref @ (BodyRef::File { .. } | BodyRef::FileRange { .. })
            if encoded_ref.content_encoding().is_some() =>
        {
            stream_content_encoded_sse_events(
                state,
                connection_id,
                encoded_ref,
                from,
                batch_size,
                tail_bytes,
                tx,
            )
            .await
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
        BodyRef::FileRange {
            path, offset, size, ..
        } => {
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
    use std::io::Write;

    use flate2::{write::GzEncoder, Compression};

    use super::{
        parse_sse_stream_from, parse_sse_stream_options, should_emit_synthetic_finish,
        split_sse_events_text, stream_content_encoded_sse_events, stream_sse_events_from_body_ref,
        SseIncrementalParser, SseStreamFrom,
    };
    use crate::body_store::BodyRef;
    use crate::test_support::TestAdminState;
    use bifrost_storage::{SandboxConfigUpdate, SandboxLimitsConfigUpdate};

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
            .with_content_encoding(Some("gzip"))
            .unwrap();
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

    #[tokio::test]
    async fn content_encoded_sse_file_range_decodes_only_the_selected_wire_bytes() {
        let harness = TestAdminState::builder().build();
        let plaintext = b"data: selected range\n\n";
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(plaintext).unwrap();
        let compressed = encoder.finish().unwrap();
        let path = harness.data_dir().join("encoded-sse-range.bin");
        let prefix = b"ignored-prefix";
        let mut file = prefix.to_vec();
        file.extend_from_slice(&compressed);
        file.extend_from_slice(b"ignored-suffix");
        std::fs::write(&path, file).unwrap();
        let body_ref = BodyRef::FileRange {
            path: path.to_string_lossy().to_string(),
            offset: prefix.len() as u64,
            size: compressed.len(),
        }
        .with_content_encoding(Some("gzip"))
        .unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);

        stream_content_encoded_sse_events(
            harness.state(),
            "encoded-sse-range",
            body_ref,
            SseStreamFrom::Begin,
            1,
            0,
            tx,
        )
        .await
        .unwrap();

        let mut output = String::new();
        while let Some(chunk) = rx.recv().await {
            output.push_str(&String::from_utf8_lossy(&chunk));
        }
        assert!(output.contains("data: selected range"), "{output}");
        assert!(!output.contains("ignored"), "{output}");
    }

    #[tokio::test]
    async fn content_encoded_sse_missing_file_finishes_without_events() {
        let harness = TestAdminState::builder().build();
        let path = harness.data_dir().join("missing-encoded-sse.gz");
        let body_ref = BodyRef::File {
            path: path.to_string_lossy().to_string(),
            size: 1,
        }
        .with_content_encoding(Some("gzip"))
        .unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);

        stream_content_encoded_sse_events(
            harness.state(),
            "missing-encoded-sse",
            body_ref,
            SseStreamFrom::Begin,
            1,
            0,
            tx,
        )
        .await
        .unwrap();

        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn content_encoded_sse_batches_complete_and_trailing_events() {
        let harness = TestAdminState::builder().build();
        let plaintext = b"data: first\n\ndata: trailing";
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(plaintext).expect("compress SSE body");
        let compressed = encoder.finish().expect("finish gzip body");
        let body_ref = harness
            .body_store
            .read()
            .store("encoded-sse-batch", "res", &compressed)
            .expect("store encoded SSE body")
            .with_content_encoding(Some("gzip"))
            .unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);

        stream_content_encoded_sse_events(
            harness.state(),
            "encoded-sse-batch",
            body_ref,
            SseStreamFrom::Begin,
            10,
            0,
            tx,
        )
        .await
        .expect("stream decoded SSE batch");

        let output = rx.recv().await.expect("batched stream output");
        let output = String::from_utf8(output.to_vec()).unwrap();
        let data = output
            .lines()
            .find_map(|line| line.strip_prefix("data: "))
            .expect("SSE batch data line");
        let batch: serde_json::Value = serde_json::from_str(data).unwrap();
        assert_eq!(batch["batch"], true);
        assert_eq!(batch["events"].as_array().unwrap().len(), 3);
        assert!(batch["events"][0]["raw"]
            .as_str()
            .unwrap()
            .contains("data: first"));
        assert!(batch["events"][1]["raw"]
            .as_str()
            .unwrap()
            .contains("data: trailing"));
        assert_eq!(batch["events"][2]["event"], "finish");
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn encoded_sse_helper_ignores_unmarked_and_custom_coded_bodies() {
        let harness = TestAdminState::builder().build();
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        stream_content_encoded_sse_events(
            harness.state(),
            "unmarked-sse",
            BodyRef::Inline {
                data: "data: plain\n\n".to_string(),
            },
            SseStreamFrom::Begin,
            1,
            0,
            tx,
        )
        .await
        .expect("unmarked body is handled by the normal SSE path");
        assert!(rx.recv().await.is_none());

        let custom_ref = harness
            .body_store
            .read()
            .store("custom-sse", "res", b"custom wire bytes")
            .unwrap()
            .with_content_encoding(Some("x-company-codec"))
            .unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        stream_content_encoded_sse_events(
            harness.state(),
            "custom-sse",
            custom_ref,
            SseStreamFrom::Begin,
            1,
            0,
            tx,
        )
        .await
        .expect("custom coding is left for the custom decoder");
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn unfinished_gzip_sse_stream_remains_live_past_two_seconds() {
        let harness = TestAdminState::builder().build();
        let connection_id = "encoded-sse-growing";
        let path = harness.data_dir().join("encoded-sse-growing.gz");
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(b"data: first\n\n").unwrap();
        encoder.flush().unwrap();
        std::fs::write(&path, encoder.get_ref()).unwrap();
        let body_ref = BodyRef::File {
            path: path.to_string_lossy().to_string(),
            size: encoder.get_ref().len(),
        }
        .with_content_encoding(Some("gzip"))
        .unwrap();
        harness.state().sse_hub.register(connection_id);
        let state = harness.state();
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let stream = tokio::spawn(async move {
            stream_sse_events_from_body_ref(
                state,
                connection_id,
                body_ref,
                SseStreamFrom::Begin,
                1,
                0,
                tx,
            )
            .await
        });

        let first = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("first event should be decoded before gzip finalization")
            .expect("stream output");
        assert!(String::from_utf8_lossy(&first).contains("data: first"));
        tokio::time::sleep(std::time::Duration::from_millis(2_100)).await;
        assert!(
            !stream.is_finished(),
            "an open SSE must not stop after two seconds"
        );

        encoder.write_all(b"data: second\n\n").unwrap();
        encoder.flush().unwrap();
        std::fs::write(&path, encoder.get_ref()).unwrap();
        let second = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("second event should arrive while the gzip member is open")
            .expect("stream output");
        assert!(String::from_utf8_lossy(&second).contains("data: second"));

        let final_wire = encoder.finish().unwrap();
        std::fs::write(&path, final_wire).unwrap();
        harness.state().sse_hub.set_closed(connection_id);
        tokio::time::timeout(std::time::Duration::from_secs(2), stream)
            .await
            .expect("closed stream should finish")
            .expect("stream task")
            .expect("stream result");
    }

    #[tokio::test]
    async fn identity_coded_sse_stays_open_for_appended_events() {
        let harness = TestAdminState::builder().build();
        let connection_id = "identity-sse-growing";
        let path = harness.data_dir().join("identity-sse-growing.txt");
        std::fs::write(&path, b"data: first\n\n").unwrap();
        let body_ref = BodyRef::File {
            path: path.to_string_lossy().to_string(),
            size: 13,
        }
        .with_content_encoding(Some("identity"))
        .unwrap();
        harness.state().sse_hub.register(connection_id);
        let state = harness.state();
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let stream = tokio::spawn(async move {
            stream_sse_events_from_body_ref(
                state,
                connection_id,
                body_ref,
                SseStreamFrom::Begin,
                1,
                0,
                tx,
            )
            .await
        });

        let first = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(String::from_utf8_lossy(&first).contains("data: first"));
        assert!(!stream.is_finished());

        std::fs::write(&path, b"data: first\n\ndata: second\n\n").unwrap();
        let second = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(String::from_utf8_lossy(&second).contains("data: second"));
        harness.state().sse_hub.set_closed(connection_id);
        tokio::time::timeout(std::time::Duration::from_secs(1), stream)
            .await
            .expect("identity stream should close")
            .expect("stream task")
            .expect("stream result");
    }

    #[tokio::test]
    async fn complete_gzip_member_stays_open_for_an_appended_member() {
        fn gzip_member(data: &[u8]) -> Vec<u8> {
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(data).unwrap();
            encoder.finish().unwrap()
        }

        let harness = TestAdminState::builder().build();
        let connection_id = "gzip-member-sse-growing";
        let path = harness.data_dir().join("gzip-member-sse-growing.gz");
        let first_member = gzip_member(b"data: first\n\n");
        std::fs::write(&path, &first_member).unwrap();
        let body_ref = BodyRef::File {
            path: path.to_string_lossy().to_string(),
            size: first_member.len(),
        }
        .with_content_encoding(Some("gzip"))
        .unwrap();
        harness.state().sse_hub.register(connection_id);
        let state = harness.state();
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let stream = tokio::spawn(async move {
            stream_sse_events_from_body_ref(
                state,
                connection_id,
                body_ref,
                SseStreamFrom::Begin,
                1,
                0,
                tx,
            )
            .await
        });

        // Coverage instrumentation can delay the first filesystem poll; this
        // remains bounded while avoiding a scheduler-sensitive one-second cap.
        let first = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(String::from_utf8_lossy(&first).contains("data: first"));
        assert!(!stream.is_finished());

        let second_member = gzip_member(b"data: second\n\n");
        let mut wire = first_member;
        wire.extend_from_slice(&second_member);
        std::fs::write(&path, wire).unwrap();
        let second = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(String::from_utf8_lossy(&second).contains("data: second"));
        harness.state().sse_hub.set_closed(connection_id);
        tokio::time::timeout(std::time::Duration::from_secs(5), stream)
            .await
            .expect("gzip stream should close")
            .expect("stream task")
            .expect("stream result");
    }

    #[tokio::test]
    async fn encoded_sse_tail_mode_skips_older_decoded_events() {
        let harness = TestAdminState::builder().build();
        let last_event = b"data: third\n\n";
        let plaintext = b"data: first\n\ndata: second\n\ndata: third\n\n";
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(plaintext).unwrap();
        let compressed = encoder.finish().unwrap();
        let body_ref = harness
            .body_store
            .read()
            .store("encoded-sse-tail", "res", &compressed)
            .unwrap()
            .with_content_encoding(Some("gzip"))
            .unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);

        stream_sse_events_from_body_ref(
            harness.state(),
            "encoded-sse-tail",
            body_ref,
            SseStreamFrom::Tail,
            1,
            last_event.len(),
            tx,
        )
        .await
        .unwrap();

        let mut output = String::new();
        while let Some(chunk) = rx.recv().await {
            output.push_str(&String::from_utf8_lossy(&chunk));
        }
        assert!(output.contains("data: third"), "{output}");
        assert!(!output.contains("data: first"), "{output}");
        assert!(!output.contains("data: second"), "{output}");
    }

    #[tokio::test]
    async fn oversized_encoded_sse_emits_error_and_stops() {
        let harness = TestAdminState::builder().build();
        harness
            .config_manager
            .update_sandbox_config(SandboxConfigUpdate {
                limits: Some(SandboxLimitsConfigUpdate {
                    max_decompress_output_bytes: Some(32),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .await
            .unwrap();
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&vec![b'a'; 1024]).unwrap();
        let compressed = encoder.finish().unwrap();
        let body_ref = harness
            .body_store
            .read()
            .store("encoded-sse-over-limit", "res", &compressed)
            .unwrap()
            .with_content_encoding(Some("gzip"))
            .unwrap();
        harness.state().sse_hub.register("encoded-sse-over-limit");
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);

        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            stream_content_encoded_sse_events(
                harness.state(),
                "encoded-sse-over-limit",
                body_ref,
                SseStreamFrom::Begin,
                1,
                0,
                tx,
            ),
        )
        .await
        .expect("over-limit stream must stop instead of retrying")
        .expect("stream result");

        let error = rx.recv().await.expect("explicit limit error");
        let error = String::from_utf8_lossy(&error);
        assert!(error.contains("\"event\":\"error\""), "{error}");
        assert!(error.contains("configured 32 byte limit"), "{error}");
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn encoded_sse_flushes_full_batches_and_single_trailing_events() {
        let harness = TestAdminState::builder().build();
        let plaintext = b"data: first\n\ndata: second\n\ndata: trailing";
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(plaintext).unwrap();
        let compressed = encoder.finish().unwrap();
        let body_ref = harness
            .body_store
            .read()
            .store("encoded-sse-full-batch", "res", &compressed)
            .unwrap()
            .with_content_encoding(Some("gzip"))
            .unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        stream_content_encoded_sse_events(
            harness.state(),
            "encoded-sse-full-batch",
            body_ref.clone(),
            SseStreamFrom::Begin,
            2,
            0,
            tx,
        )
        .await
        .unwrap();
        let first_batch = String::from_utf8(rx.recv().await.unwrap().to_vec()).unwrap();
        assert!(first_batch.contains("data: first"), "{first_batch}");
        assert!(first_batch.contains("data: second"), "{first_batch}");

        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        stream_content_encoded_sse_events(
            harness.state(),
            "encoded-sse-full-batch",
            body_ref,
            SseStreamFrom::Begin,
            1,
            0,
            tx,
        )
        .await
        .unwrap();
        let mut output = String::new();
        while let Some(chunk) = rx.recv().await {
            output.push_str(&String::from_utf8_lossy(&chunk));
        }
        assert!(output.contains("data: trailing"), "{output}");
    }

    #[tokio::test]
    async fn malformed_encoded_sse_emits_terminal_error() {
        let harness = TestAdminState::builder().build();
        let body_ref = harness
            .body_store
            .read()
            .store("malformed-closed-sse", "res", b"not gzip")
            .unwrap()
            .with_content_encoding(Some("gzip"))
            .unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);

        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            stream_content_encoded_sse_events(
                harness.state(),
                "malformed-closed-sse",
                body_ref,
                SseStreamFrom::Begin,
                1,
                0,
                tx,
            ),
        )
        .await
        .expect("closed malformed stream must stop")
        .unwrap();

        let error = String::from_utf8(rx.recv().await.unwrap().to_vec()).unwrap();
        assert!(error.contains("\"event\":\"error\""), "{error}");
        assert!(error.contains("failed to decode"), "{error}");
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn active_malformed_encoded_sse_does_not_hang() {
        let harness = TestAdminState::builder().build();
        let connection_id = "malformed-active-sse";
        let body_ref = harness
            .body_store
            .read()
            .store(connection_id, "res", b"not gzip")
            .unwrap()
            .with_content_encoding(Some("gzip"))
            .unwrap();
        harness.state().sse_hub.register(connection_id);
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);

        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            stream_content_encoded_sse_events(
                harness.state(),
                connection_id,
                body_ref,
                SseStreamFrom::Begin,
                1,
                0,
                tx,
            ),
        )
        .await
        .expect("active malformed stream must terminate")
        .unwrap();

        let error = String::from_utf8(rx.recv().await.unwrap().to_vec()).unwrap();
        assert!(error.contains("\"event\":\"error\""), "{error}");
        assert!(rx.recv().await.is_none());
        harness.state().sse_hub.unregister(connection_id);
    }

    #[tokio::test]
    async fn oversized_encoded_sse_error_uses_requested_batch_shape() {
        let harness = TestAdminState::builder().build();
        harness
            .config_manager
            .update_sandbox_config(SandboxConfigUpdate {
                limits: Some(SandboxLimitsConfigUpdate {
                    max_decompress_output_bytes: Some(16),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .await
            .unwrap();
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&[b'a'; 128]).unwrap();
        let compressed = encoder.finish().unwrap();
        let body_ref = harness
            .body_store
            .read()
            .store("encoded-sse-over-limit-batch", "res", &compressed)
            .unwrap()
            .with_content_encoding(Some("gzip"))
            .unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);

        stream_content_encoded_sse_events(
            harness.state(),
            "encoded-sse-over-limit-batch",
            body_ref,
            SseStreamFrom::Begin,
            10,
            0,
            tx,
        )
        .await
        .unwrap();

        let error = String::from_utf8(rx.recv().await.unwrap().to_vec()).unwrap();
        assert!(error.contains("\"batch\":true"), "{error}");
        assert!(error.contains("\"event\":\"error\""), "{error}");
    }
}
