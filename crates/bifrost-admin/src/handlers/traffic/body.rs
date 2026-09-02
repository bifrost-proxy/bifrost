use super::*;
use crate::handlers::network_body::{
    decode_content_encoded_body_with_limit, DEFAULT_MAX_DECOMPRESSED_BODY_BYTES,
};
use base64::Engine as _;

pub(super) async fn get_request_body(
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

pub(super) async fn get_response_body(
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

pub(super) async fn get_response_body_content(
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

pub(super) async fn load_body_bytes_async(
    state: &SharedAdminState,
    body_ref: &BodyRef,
) -> Option<Vec<u8>> {
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
    }
}

pub(super) async fn configured_decompress_output_bytes(state: &SharedAdminState) -> usize {
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

pub(super) fn decode_stored_body(
    body_ref: &BodyRef,
    bytes: Vec<u8>,
    decode_content_encoding: bool,
    max_output_bytes: usize,
) -> Vec<u8> {
    if decode_content_encoding {
        decode_content_encoded_body_with_limit(
            bytes,
            body_ref.content_encoding().as_deref(),
            max_output_bytes,
        )
    } else {
        bytes
    }
}

pub(super) async fn get_body_content_async(
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
            BodyRef::File { path, size, .. } | BodyRef::FileRange { path, size, .. }
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
        },
    }
}

pub(super) async fn get_body_bytes_async(
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
    use std::sync::Arc;

    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use bifrost_storage::{SandboxConfigUpdate, SandboxLimitsConfigUpdate};
    use flate2::{write::GzEncoder, Compression};
    use http_body_util::BodyExt;
    use hyper::{Response, StatusCode};

    use super::{
        configured_decompress_output_bytes, decode_stored_body, get_body_bytes_async,
        get_body_content_async, get_request_body, get_response_body, get_response_body_content,
    };
    use crate::body_store::BodyRef;
    use crate::handlers::BoxBody;
    use crate::test_support::TestAdminState;
    use crate::{AdminState, TrafficRecord};

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(bytes).expect("write gzip fixture");
        encoder.finish().expect("finish gzip fixture")
    }

    #[test]
    fn only_decodes_refs_marked_as_content_encoded() {
        let harness = TestAdminState::builder().build();
        let application_gzip = gzip(b"application payload");
        let wire_body = gzip(&application_gzip);
        let decoded_ref = harness
            .body_store
            .read()
            .store("decoded-body", "res", &application_gzip)
            .expect("store decoded body");
        let encoded_ref = harness
            .body_store
            .read()
            .store("encoded-body", "res", &wire_body)
            .expect("store encoded body")
            .with_content_encoding(Some("gzip"))
            .unwrap();

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
            .with_content_encoding(Some("gzip"))
            .unwrap();
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

    async fn response_json(response: Response<BoxBody>) -> serde_json::Value {
        serde_json::from_slice(
            &response
                .into_body()
                .collect()
                .await
                .expect("collect response")
                .to_bytes(),
        )
        .expect("parse response JSON")
    }

    #[tokio::test]
    async fn body_routes_serve_decoded_raw_and_binary_content() {
        let harness = TestAdminState::builder().build();
        let mut record = TrafficRecord::new(
            "body-routes".to_string(),
            "POST".to_string(),
            "https://example.test/body".to_string(),
        );
        record.request_body_ref = Some(BodyRef::Inline {
            data: "decoded request".to_string(),
        });
        record.raw_request_body_ref = Some(BodyRef::Inline {
            data: "raw request".to_string(),
        });
        record.response_body_ref = Some(BodyRef::Inline {
            data: "decoded response".to_string(),
        });
        record.raw_response_body_ref = Some(BodyRef::Inline {
            data: "raw response".to_string(),
        });
        record.content_type = Some("text/plain".to_string());
        harness.traffic_db.record(record);
        let state = harness.state();

        let request =
            response_json(get_request_body(state.clone(), "body-routes", None).await).await;
        assert_eq!(request["data"], "decoded request");
        let raw_request =
            response_json(get_request_body(state.clone(), "body-routes", Some("raw=1")).await)
                .await;
        assert_eq!(raw_request["data"], "raw request");

        let response =
            response_json(get_response_body(state.clone(), "body-routes", None).await).await;
        assert_eq!(response["data"], "decoded response");
        let raw_response = response_json(
            get_response_body(state.clone(), "body-routes", Some("raw=1&encoding=base64")).await,
        )
        .await;
        assert_eq!(raw_response["data"], "raw response");
        assert_eq!(raw_response["encoding"], "base64");

        let binary = get_response_body_content(state, "body-routes", None).await;
        assert_eq!(binary.status(), StatusCode::OK);
        assert_eq!(binary.headers()["Content-Type"], "text/plain");
        assert_eq!(
            binary.into_body().collect().await.unwrap().to_bytes(),
            "decoded response"
        );
    }

    #[tokio::test]
    async fn body_helpers_report_missing_storage_and_use_default_limit() {
        let harness = TestAdminState::builder().build();
        let state_without_body_store =
            Arc::new(AdminState::new(0).with_traffic_db_store_shared(harness.traffic_db.clone()));
        assert_eq!(
            configured_decompress_output_bytes(&state_without_body_store).await,
            super::DEFAULT_MAX_DECOMPRESSED_BODY_BYTES
        );

        let missing_ref = BodyRef::File {
            path: harness
                .data_dir()
                .join("missing-body")
                .to_string_lossy()
                .to_string(),
            size: 42,
        };
        let no_store = response_json(
            get_body_content_async(&state_without_body_store, &missing_ref, false, true).await,
        )
        .await;
        assert_eq!(no_store["error"], "Body store not configured");
        assert_eq!(no_store["size"], 42);

        let missing = get_body_content_async(&harness.state(), &missing_ref, false, true).await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        let missing_bytes = get_body_bytes_async(
            &harness.state(),
            &missing_ref,
            "application/octet-stream",
            true,
        )
        .await;
        assert_eq!(missing_bytes.status(), StatusCode::NOT_FOUND);
    }
}
