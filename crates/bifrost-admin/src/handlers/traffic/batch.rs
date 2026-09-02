use super::body::{configured_decompress_output_bytes, decode_stored_body, load_body_bytes_async};
use super::*;
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
            .with_content_encoding(Some("gzip"))
            .unwrap();

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
