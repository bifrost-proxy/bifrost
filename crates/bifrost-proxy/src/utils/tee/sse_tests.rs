use std::io::Write;

use bytes::Bytes;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use super::{SseEventBodyDecoder, SseTeeBody, SseTeeOptions};

#[tokio::test]
async fn sse_event_decoder_uses_the_limit_resolved_by_the_async_handler() {
    let plaintext = b"data: exceeds tiny configured limit\n\n";
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(plaintext).unwrap();
    let wire = Bytes::from(encoder.finish().unwrap());
    let body = crate::server::full_body(Bytes::new());
    let mut tee = SseTeeBody::new(
        body,
        Some(Arc::new(bifrost_admin::AdminState::new(0))),
        "configured-limit".to_string(),
        SseTeeOptions {
            traffic_type: None,
            file_writer: None,
            content_encoding: Some("gzip".to_string()),
            max_buffer_size: 1024,
            max_decompress_output_bytes: 4,
        },
    );

    tee.process_sse_wire_chunk(&wire);

    for _ in 0..50 {
        let partial = match &tee.event_decoder {
            SseEventBodyDecoder::Encoded(observer) => observer.partial.load(Ordering::Relaxed),
            _ => false,
        };
        if partial {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("bounded SSE observer did not reject output over the configured limit");
}

#[tokio::test]
async fn sse_event_decoder_finalization_rejects_a_truncated_trailer() {
    let plaintext = b"data: complete event\n\n";
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(plaintext).unwrap();
    let mut wire = encoder.finish().unwrap();
    wire.truncate(wire.len() - 4);
    let wire = Bytes::from(wire);
    let body = crate::server::full_body(Bytes::new());
    let mut tee = SseTeeBody::new(
        body,
        Some(Arc::new(bifrost_admin::AdminState::new(0))),
        "truncated-trailer".to_string(),
        SseTeeOptions {
            traffic_type: None,
            file_writer: None,
            content_encoding: Some("gzip".to_string()),
            max_buffer_size: 1024,
            max_decompress_output_bytes: 1024,
        },
    );

    tee.process_sse_wire_chunk(&wire);
    assert!(matches!(tee.event_decoder, SseEventBodyDecoder::Encoded(_)));

    tee.finish_sse_event_decoder();

    assert!(matches!(tee.event_decoder, SseEventBodyDecoder::Encoded(_)));
    for _ in 0..50 {
        let partial = match &tee.event_decoder {
            SseEventBodyDecoder::Encoded(observer) => observer.partial.load(Ordering::Relaxed),
            _ => false,
        };
        if partial {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("truncated SSE trailer was not rejected during finalization");
}
