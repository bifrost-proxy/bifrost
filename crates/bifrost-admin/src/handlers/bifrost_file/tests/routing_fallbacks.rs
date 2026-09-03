use super::super::*;
use base64::Engine;
use flate2::{write::GzEncoder, Compression};
use std::io::Write;
use std::sync::Arc;

#[test]
fn network_record_import_falls_back_to_url_when_host_and_path_missing() {
    let record = NetworkRecord {
        id: "REQ-fallback".to_string(),
        method: "POST".to_string(),
        url: "https://example.test/api?x=1".to_string(),
        status: 201,
        host: None,
        path: None,
        protocol: None,
        actual_url: None,
        actual_host: None,
        listener_port: None,
        has_rule_hit: None,
        error_message: None,
        client_app: None,
        client_path: None,
        request_headers: None,
        response_headers: None,
        original_response_headers: None,
        request_body: None,
        request_body_base64: None,
        response_body: None,
        response_body_base64: None,
        duration_ms: 0,
        timestamp: 0,
        matched_rules: None,
        active_rules: None,
    };

    let traffic = network_record_to_traffic_record(&record);

    assert_eq!(traffic.host, "example.test");
    assert_eq!(traffic.path, "/api?x=1");
    assert_eq!(traffic.protocol, "HTTPS");
}

#[test]
fn network_record_import_uses_default_protocol_for_invalid_url() {
    let record = NetworkRecord {
        id: "REQ-invalid-url".to_string(),
        method: "GET".to_string(),
        url: "not a valid url".to_string(),
        status: 0,
        host: None,
        path: None,
        protocol: None,
        actual_url: None,
        actual_host: None,
        listener_port: None,
        has_rule_hit: None,
        error_message: None,
        client_app: None,
        client_path: None,
        request_headers: None,
        response_headers: None,
        original_response_headers: None,
        request_body: None,
        request_body_base64: None,
        response_body: None,
        response_body_base64: None,
        duration_ms: 0,
        timestamp: 0,
        matched_rules: None,
        active_rules: None,
    };

    let traffic = network_record_to_traffic_record(&record);

    assert_eq!(traffic.host, "");
    assert_eq!(traffic.path, "");
    assert_eq!(traffic.protocol, "HTTP");
}

#[test]
fn imported_compressed_bodies_defer_decoding_and_preserve_the_package_budget() {
    let dir = tempfile::tempdir().unwrap();
    let body_store = Arc::new(parking_lot::RwLock::new(crate::BodyStore::new(
        dir.path().join("bodies"),
        1024 * 1024,
        1,
        64 * 1024,
        std::time::Duration::from_secs(1),
    )));
    let plaintext = vec![b'a'; 128];
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&plaintext).unwrap();
    let compressed = encoder.finish().unwrap();
    let make_record = |id: &str| -> NetworkRecord {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "method": "POST",
            "url": "https://example.test/import-budget",
            "status": 200,
            "request_headers": [["content-encoding", "gzip"]],
            "request_body_base64": STANDARD.encode(&compressed),
            "duration_ms": 1,
            "timestamp": 1
        }))
        .unwrap()
    };
    let first = make_record("REQ-budget-first");
    let second = make_record("REQ-budget-second");
    let mut first_traffic = network_record_to_traffic_record(&first);
    let mut second_traffic = network_record_to_traffic_record(&second);
    let mut remaining_decompress_bytes = plaintext.len();

    persist_imported_bodies(
        &first,
        &mut first_traffic,
        &body_store,
        &mut remaining_decompress_bytes,
    )
    .unwrap();
    persist_imported_bodies(
        &second,
        &mut second_traffic,
        &body_store,
        &mut remaining_decompress_bytes,
    )
    .unwrap();

    assert_eq!(remaining_decompress_bytes, plaintext.len());
    let store = body_store.read();
    assert_eq!(
        store
            .load_bytes(first_traffic.request_body_ref.as_ref().unwrap())
            .as_deref(),
        Some(compressed.as_slice())
    );
    let second_ref = second_traffic.request_body_ref.as_ref().unwrap();
    assert_eq!(second_ref.content_encoding(), None);
    assert_eq!(
        first_traffic.request_body_content_encoding().as_deref(),
        Some("gzip")
    );
    assert_eq!(
        second_traffic.request_body_content_encoding().as_deref(),
        Some("gzip")
    );
    assert_eq!(
        store.load_bytes(second_ref).as_deref(),
        Some(compressed.as_slice())
    );
}
