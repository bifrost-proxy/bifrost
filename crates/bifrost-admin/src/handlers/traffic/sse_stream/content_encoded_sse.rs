use super::super::body::{configured_decompress_output_bytes, load_body_bytes_async};
use super::*;
use crate::handlers::network_body::{
    content_encoding_is_supported, decompress_partial_with_limit, decompress_with_limit,
};

pub(super) async fn stream_content_encoded_sse_events(
    state: SharedAdminState,
    connection_id: &str,
    body_ref: BodyRef,
    from: SseStreamFrom,
    batch_size: usize,
    tail_bytes: usize,
    tx: tokio::sync::mpsc::Sender<bytes::Bytes>,
) -> Result<(), ()> {
    use std::time::Instant;
    use tokio::time::{sleep, Duration};

    let max_output_bytes = configured_decompress_output_bytes(&state).await;
    let Some(content_encoding) = body_ref.content_encoding() else {
        return Ok(());
    };
    if !content_encoding_is_supported(&content_encoding) {
        return Ok(());
    }

    let mut parser = SseIncrementalParser::new();
    let mut decoded_offset = 0usize;
    let mut initialized_offset = false;
    let mut seq = 0u64;
    let mut batch = Vec::new();
    let batch_size = batch_size.max(1);
    let mut last_force_flush_refresh = Instant::now();
    let mut closed_idle_retries = 0u8;
    let mut saw_closed = false;
    let mut last_event_was_finish = false;

    loop {
        if last_force_flush_refresh.elapsed() >= Duration::from_secs(5) {
            state.sse_hub.request_force_flush(connection_id, 30_000);
            last_force_flush_refresh = Instant::now();
        }

        let is_open = state.sse_hub.is_open(connection_id).unwrap_or(false);
        if !is_open {
            saw_closed = true;
        }
        let mut made_progress = false;
        let mut member_is_complete = false;
        if let Some(wire_bytes) = load_body_bytes_async(&state, &body_ref).await {
            let decoded =
                match decompress_with_limit(&wire_bytes, &content_encoding, max_output_bytes) {
                    Ok(decoded) => {
                        member_is_complete = true;
                        Ok(decoded)
                    }
                    Err(complete_error) => decompress_partial_with_limit(
                        &wire_bytes,
                        &content_encoding,
                        max_output_bytes,
                    )
                    .map_err(|partial_error| (complete_error, partial_error)),
                };
            if let Ok(decoded) = decoded {
                if !initialized_offset && (!decoded.is_empty() || !is_open) {
                    decoded_offset = if from == SseStreamFrom::Tail && tail_bytes > 0 {
                        decoded.len().saturating_sub(tail_bytes)
                    } else {
                        0
                    };
                    initialized_offset = true;
                }
                if decoded.len() >= decoded_offset {
                    let new_bytes = &decoded[decoded_offset..];
                    made_progress = !new_bytes.is_empty();
                    decoded_offset = decoded.len();
                    let mut produced = Vec::new();
                    parser.push_bytes(new_bytes, &mut produced);
                    for raw in produced {
                        seq = seq.saturating_add(1);
                        let event = sse_event_from_raw(seq, now_ms(), raw);
                        last_event_was_finish = event.event.as_deref() == Some("finish");
                        if batch_size <= 1 {
                            if tx
                                .send(bytes::Bytes::from(sse_json_line(&event)))
                                .await
                                .is_err()
                            {
                                return Ok(());
                            }
                        } else {
                            batch.push(event);
                            if batch.len() >= batch_size {
                                let payload = sse_json_batch_line(&batch);
                                batch.clear();
                                if tx.send(bytes::Bytes::from(payload)).await.is_err() {
                                    return Ok(());
                                }
                            }
                        }
                    }
                }
            } else if let Err((complete_error, partial_error)) = decoded {
                let hit_limit = [&complete_error, &partial_error].iter().any(|error| {
                    error
                        .to_string()
                        .contains("decompressed body exceeds the preview limit")
                });
                if hit_limit {
                    seq = seq.saturating_add(1);
                    let error_event = crate::sse::SseEvent {
                        seq,
                        ts: now_ms(),
                        id: None,
                        event: Some("error".to_string()),
                        retry: None,
                        data: format!(
                            "decoded SSE body exceeds the configured {} byte limit",
                            max_output_bytes
                        ),
                        raw: None,
                        parse_error: true,
                    };
                    if batch_size <= 1 {
                        let _ = tx
                            .send(bytes::Bytes::from(sse_json_line(&error_event)))
                            .await;
                    } else {
                        batch.push(error_event);
                        let _ = tx
                            .send(bytes::Bytes::from(sse_json_batch_line(&batch)))
                            .await;
                    }
                    return Ok(());
                }
            }
        }

        // A complete prefix may still receive another gzip member (and
        // `identity` is complete for every prefix), so only finish after the
        // upstream connection itself has closed.
        if member_is_complete && !is_open {
            break;
        }

        if !is_open {
            if made_progress {
                closed_idle_retries = 0;
            } else {
                closed_idle_retries = closed_idle_retries.saturating_add(1);
                if closed_idle_retries >= 10 {
                    break;
                }
            }
            sleep(Duration::from_millis(50)).await;
        } else {
            closed_idle_retries = 0;
            sleep(Duration::from_millis(100)).await;
        }
    }

    if let Some(raw) = parser.finish() {
        seq = seq.saturating_add(1);
        let event = sse_event_from_raw(seq, now_ms(), raw);
        last_event_was_finish = event.event.as_deref() == Some("finish");
        if batch_size <= 1 {
            let _ = tx.send(bytes::Bytes::from(sse_json_line(&event))).await;
        } else {
            batch.push(event);
        }
    }

    if should_emit_synthetic_finish(None, saw_closed, last_event_was_finish) {
        seq = seq.saturating_add(1);
        let finish_event = crate::sse::SseEvent {
            seq,
            ts: now_ms(),
            id: None,
            event: Some("finish".to_string()),
            retry: None,
            data: String::new(),
            raw: None,
            parse_error: false,
        };
        if batch_size <= 1 {
            let _ = tx
                .send(bytes::Bytes::from(sse_json_line(&finish_event)))
                .await;
        } else {
            batch.push(finish_event);
        }
    }
    if batch_size > 1 && !batch.is_empty() {
        let _ = tx
            .send(bytes::Bytes::from(sse_json_batch_line(&batch)))
            .await;
    }
    Ok(())
}
