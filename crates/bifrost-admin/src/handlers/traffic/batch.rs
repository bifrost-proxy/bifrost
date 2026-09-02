use super::body::{configured_decompress_output_bytes, load_body_bytes_async};
use super::*;
use crate::handlers::network_body::{
    content_encoding_is_supported, decompress_prefix_with_limit_metered,
};
use base64::Engine as _;

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
pub(super) async fn batch_traffic(
    req: Request<Incoming>,
    state: SharedAdminState,
) -> Response<BoxBody> {
    let params = match parse_batch_traffic_query(req.uri().query()) {
        Ok(p) => p,
        Err(msg) => return error_response(StatusCode::BAD_REQUEST, &msg),
    };

    let service = AdminQueryService::new(state.clone());
    let mut out = Vec::<u8>::new();
    let mut remaining_decompress_bytes = configured_decompress_output_bytes(&state).await;
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
                            &mut remaining_decompress_bytes,
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
                            &mut remaining_decompress_bytes,
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
    remaining_decompress_bytes: &mut usize,
) -> Option<serde_json::Value> {
    let body_ref = body_ref?;
    let bytes = load_body_bytes_async(state, body_ref).await?;
    let decoded = decode_batch_body(body_ref, bytes, max_body_bytes, remaining_decompress_bytes);
    let reported_size = if decoded.truncated {
        decoded.bytes.len().saturating_add(1)
    } else {
        decoded.bytes.len()
    };
    let (slice, truncated) = if decoded.bytes.len() > max_body_bytes {
        (&decoded.bytes[..max_body_bytes], true)
    } else {
        (&decoded.bytes[..], decoded.truncated)
    };
    let encoded = base64::engine::general_purpose::STANDARD.encode(slice);
    let mut obj = serde_json::Map::new();
    obj.insert("bytes_b64".to_string(), serde_json::Value::String(encoded));
    obj.insert(
        "size".to_string(),
        serde_json::Value::Number(serde_json::Number::from(reported_size)),
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

#[derive(Debug, PartialEq, Eq)]
struct DecodedBatchBody {
    bytes: Vec<u8>,
    truncated: bool,
}

fn decode_batch_body(
    body_ref: &BodyRef,
    bytes: Vec<u8>,
    max_body_bytes: usize,
    remaining_decompress_bytes: &mut usize,
) -> DecodedBatchBody {
    let Some(content_encoding) = body_ref.content_encoding() else {
        return DecodedBatchBody {
            bytes,
            truncated: false,
        };
    };
    if !content_encoding_is_supported(&content_encoding) {
        return DecodedBatchBody {
            bytes,
            truncated: false,
        };
    }

    // The batch endpoint only needs enough decoded data to fill its preview.
    // Share the configured decompression allowance across every requested body
    // so one request cannot multiply that work by the 200-ID batch limit.
    let decode_limit = (*remaining_decompress_bytes).min(max_body_bytes.saturating_add(1));
    if decode_limit == 0 {
        return DecodedBatchBody {
            bytes,
            truncated: false,
        };
    }
    match decompress_prefix_with_limit_metered(&bytes, &content_encoding, decode_limit) {
        Ok(decoded) => {
            *remaining_decompress_bytes =
                remaining_decompress_bytes.saturating_sub(decoded.consumed);
            DecodedBatchBody {
                bytes: decoded.bytes,
                truncated: decoded.truncated,
            }
        }
        Err(_) => {
            // A decoder may consume the entire allowance before reporting an
            // invalid or oversized stream, so account for the worst case.
            *remaining_decompress_bytes = remaining_decompress_bytes.saturating_sub(decode_limit);
            DecodedBatchBody {
                bytes,
                truncated: false,
            }
        }
    }
}

#[cfg(test)]
mod batch_query_tests {
    use std::io::Write;

    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use flate2::{write::GzEncoder, Compression};
    use hyper::{body::Incoming, Request};

    use super::{
        batch_traffic, build_batch_body_chunk, decode_batch_body, parse_batch_traffic_query,
        BATCH_GET_DEFAULT_MAX_BODY_BYTES, BATCH_GET_MAX_IDS,
    };
    use crate::state::SharedAdminState;
    use crate::test_support::TestAdminState;

    async fn spawn_batch_server(state: SharedAdminState) -> (String, tokio::task::JoinHandle<()>) {
        use hyper::server::conn::http1;
        use hyper::service::service_fn;
        use hyper_util::rt::TokioIo;
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind batch test listener");
        let addr = listener.local_addr().expect("batch test listener addr");
        let server = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let io = TokioIo::new(stream);
                let state = state.clone();
                tokio::spawn(async move {
                    let service = service_fn(move |req: Request<Incoming>| {
                        let state = state.clone();
                        async move { Ok::<_, hyper::Error>(batch_traffic(req, state).await) }
                    });
                    let _ = http1::Builder::new().serve_connection(io, service).await;
                });
            }
        });
        (format!("http://{addr}"), server)
    }

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
            .with_content_encoding(Some("gzip"))
            .unwrap();

        let mut remaining = usize::MAX;
        let chunk = build_batch_body_chunk(
            &harness.state(),
            Some(&body_ref),
            Some("application/json"),
            usize::MAX,
            &mut remaining,
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

    #[test]
    fn batch_body_decoding_shares_one_output_budget() {
        let harness = TestAdminState::builder().build();
        let plaintext = b"sixteen-byte-msg";
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(plaintext).unwrap();
        let compressed = encoder.finish().unwrap();
        let body_ref = harness
            .body_store
            .read()
            .store("batch-budget", "req", &compressed)
            .unwrap()
            .with_content_encoding(Some("gzip"))
            .unwrap();
        let mut remaining = plaintext.len();

        let first = decode_batch_body(&body_ref, compressed.clone(), usize::MAX, &mut remaining);
        let second = decode_batch_body(&body_ref, compressed.clone(), usize::MAX, &mut remaining);

        assert_eq!(first.bytes, plaintext);
        assert!(!first.truncated);
        assert_eq!(remaining, 0);
        assert_eq!(second.bytes, compressed);
        assert!(!second.truncated);
    }

    #[test]
    fn batch_body_decoding_preserves_unencoded_custom_and_invalid_bytes() {
        let harness = TestAdminState::builder().build();
        let raw = vec![0, 159, 146, 150];
        let plain_ref = harness
            .body_store
            .read()
            .store("batch-plain", "req", &raw)
            .unwrap();
        let custom_ref = harness
            .body_store
            .read()
            .store("batch-custom", "req", &raw)
            .unwrap()
            .with_content_encoding(Some("x-private-cipher"))
            .unwrap();
        let invalid_gzip_ref = harness
            .body_store
            .read()
            .store("batch-invalid", "req", &raw)
            .unwrap()
            .with_content_encoding(Some("gzip"))
            .unwrap();

        let mut remaining = 3;
        assert_eq!(
            decode_batch_body(&plain_ref, raw.clone(), 2, &mut remaining).bytes,
            raw
        );
        assert_eq!(remaining, 3);
        assert_eq!(
            decode_batch_body(&custom_ref, raw.clone(), 2, &mut remaining).bytes,
            raw
        );
        assert_eq!(remaining, 3);
        assert_eq!(
            decode_batch_body(&invalid_gzip_ref, raw.clone(), 2, &mut remaining).bytes,
            raw
        );
        assert_eq!(remaining, 0);
    }

    #[tokio::test]
    async fn oversized_compressed_batch_body_returns_decoded_prefix() {
        let harness = TestAdminState::builder().build();
        let plaintext = b"decoded body longer than preview";
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(plaintext).unwrap();
        let compressed = encoder.finish().unwrap();
        let body_ref = harness
            .body_store
            .read()
            .store("batch-truncated", "res", &compressed)
            .unwrap()
            .with_content_encoding(Some("gzip"))
            .unwrap();
        let mut remaining = usize::MAX;

        let chunk = build_batch_body_chunk(
            &harness.state(),
            Some(&body_ref),
            Some("text/plain"),
            7,
            &mut remaining,
        )
        .await
        .unwrap();

        assert_eq!(
            STANDARD
                .decode(chunk["bytes_b64"].as_str().unwrap())
                .unwrap(),
            &plaintext[..7]
        );
        assert_eq!(chunk["truncated"], true);
        assert!(chunk["size"].as_u64().unwrap() > 7);
    }

    #[tokio::test]
    async fn batch_endpoint_initializes_shared_budget_and_validates_query() {
        let harness = TestAdminState::builder().build();
        let (base, server) = spawn_batch_server(harness.state()).await;
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("build batch test client");

        let response = client
            .get(format!("{base}/api/traffic/batch?ids=missing"))
            .send()
            .await
            .expect("request batch endpoint");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let line: serde_json::Value =
            serde_json::from_str(response.text().await.expect("read batch response").trim())
                .expect("parse batch response");
        assert_eq!(line["id"], "missing");
        assert_eq!(line["ok"], false);

        let invalid = client
            .get(format!("{base}/api/traffic/batch"))
            .send()
            .await
            .expect("request invalid batch endpoint");
        assert_eq!(invalid.status(), reqwest::StatusCode::BAD_REQUEST);
        server.abort();
    }
}
