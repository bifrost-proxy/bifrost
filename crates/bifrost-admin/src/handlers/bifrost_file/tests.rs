use super::*;
use async_trait::async_trait;
use base64::Engine;
use flate2::{write::GzEncoder, Compression};
use std::io::Write;
use std::sync::Arc;

const TEST_ADMIN_PORT: u16 = 19_900;

#[tokio::test]
async fn network_export_preserves_gzip_request_and_exposes_plaintext() {
    let dir = tempfile::tempdir().unwrap();
    let storage = RulesStorage::with_dir(dir.path().to_path_buf()).unwrap();
    let state = Arc::new(
        crate::state::AdminState::new_for_test(TEST_ADMIN_PORT, storage).with_body_store(Arc::new(
            parking_lot::RwLock::new(crate::BodyStore::new(
                dir.path().join("bodies"),
                1024 * 1024,
                1,
                64 * 1024,
                std::time::Duration::from_secs(1),
            )),
        )),
    );
    let plaintext = br#"{"message":"hello from gzip"}"#;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(plaintext).unwrap();
    let compressed = encoder.finish().unwrap();

    let mut traffic = TrafficRecord::new(
        "REQ-gzip-export".to_string(),
        "POST".to_string(),
        "https://example.test/gzip".to_string(),
    );
    traffic.request_headers = Some(vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("content-encoding".to_string(), "gzip".to_string()),
    ]);
    traffic.request_body_ref = state
        .body_store
        .as_ref()
        .and_then(|store| store.read().store(&traffic.id, "req", &compressed))
        .map(|body_ref| body_ref.with_content_encoding(Some("gzip")));
    traffic.response_headers = Some(vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("content-encoding".to_string(), "gzip".to_string()),
    ]);
    traffic.response_body_ref = state
        .body_store
        .as_ref()
        .and_then(|store| store.read().store(&traffic.id, "res", &compressed))
        .map(|body_ref| body_ref.with_content_encoding(Some("gzip")));

    let record = traffic_to_network_record(&traffic, true, &state).await;

    assert_eq!(
        record.request_body.as_deref(),
        std::str::from_utf8(plaintext).ok()
    );
    let encoded = record
        .request_body_base64
        .as_deref()
        .expect("compressed request should remain recoverable");
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap(),
        compressed
    );
    assert_eq!(
        record.response_body.as_deref(),
        std::str::from_utf8(plaintext).ok()
    );
    let encoded = record
        .response_body_base64
        .as_deref()
        .expect("compressed response should remain recoverable");
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap(),
        compressed
    );
}

#[test]
fn preview_hides_legacy_lossy_compressed_body_and_warns() {
    let content = r#"01 network

[meta]
name = "legacy-lossy"
version = "1.0.0"
created_at = "2026-09-02T04:04:10Z"

[options]
count = 1

---
[{"id":"REQ-lossy","method":"POST","url":"https://example.test/gzip","status":200,"request_headers":[["content-type","application/json"],["content-encoding","gzip"]],"request_body":"\u001f��garbled","duration_ms":1,"timestamp":1}]
"#;

    let preview = build_preview(content).expect("legacy network preview");
    let network = preview.network.expect("network preview");
    let detail = network.single_record.expect("single record detail");

    assert!(detail.request_body.is_none());
    assert!(network
        .warnings
        .iter()
        .any(|warning| warning.contains("older Bifrost version")));
}

#[test]
fn multi_record_preview_surfaces_legacy_lossy_body_warnings() {
    let legacy: NetworkRecord = serde_json::from_value(serde_json::json!({
        "id": "REQ-lossy-multi",
        "method": "POST",
        "url": "https://example.test/lossy",
        "status": 200,
        "request_headers": [["content-encoding", "gzip"]],
        "request_body": "\u{1f}��garbled",
        "duration_ms": 1,
        "timestamp": 1
    }))
    .unwrap();
    let clean: NetworkRecord = serde_json::from_value(serde_json::json!({
        "id": "REQ-clean-multi",
        "method": "GET",
        "url": "https://example.test/clean",
        "status": 200,
        "response_body": "plain text",
        "duration_ms": 1,
        "timestamp": 2
    }))
    .unwrap();

    let preview = build_network_preview(&[legacy, clean]);

    assert!(preview.single_record.is_none());
    assert!(preview.warnings.iter().any(|warning| {
        warning.contains("REQ-lossy-multi") && warning.contains("older Bifrost version")
    }));
}

#[test]
fn multi_record_preview_does_not_decompress_lossless_body_fields() {
    let encoded_binary = STANDARD.encode([0, 1, 2, 3]);
    let encoded_record: NetworkRecord = serde_json::from_value(serde_json::json!({
        "id": "REQ-encoded-multi",
        "method": "GET",
        "url": "https://example.test/encoded",
        "status": 200,
        "response_headers": [["content-encoding", "gzip"]],
        "response_body_base64": encoded_binary,
        "duration_ms": 1,
        "timestamp": 1
    }))
    .unwrap();
    let clean_record: NetworkRecord = serde_json::from_value(serde_json::json!({
        "id": "REQ-clean-multi",
        "method": "GET",
        "url": "https://example.test/clean",
        "status": 200,
        "duration_ms": 1,
        "timestamp": 2
    }))
    .unwrap();

    let preview = build_network_preview(&[encoded_record, clean_record]);

    assert!(preview.single_record.is_none());
    assert!(preview.warnings.is_empty());
}

#[test]
fn malformed_lossless_body_fields_are_rejected() {
    for (request_body_base64, response_body_base64, expected_label) in [
        (Some("%%%"), None, "request_body_base64"),
        (None, Some("%%%"), "response_body_base64"),
    ] {
        let record: NetworkRecord = serde_json::from_value(serde_json::json!({
            "id": "REQ-invalid-base64",
            "method": "POST",
            "url": "https://example.test/invalid-base64",
            "status": 200,
            "request_body_base64": request_body_base64,
            "response_body_base64": response_body_base64,
            "duration_ms": 1,
            "timestamp": 1
        }))
        .unwrap();

        let error = validate_network_body_base64(std::slice::from_ref(&record))
            .expect_err("invalid lossless body must be rejected before import");
        assert!(error.contains("REQ-invalid-base64"), "{error}");
        assert!(error.contains(expected_label), "{error}");

        let content = BifrostFileWriter::write_network("invalid-base64", None, &[record])
            .expect("serialize invalid package fixture");
        let error = build_preview(&content).expect_err("preview must reject invalid base64");
        assert!(error.contains(expected_label), "{error}");
    }
}

#[test]
fn imported_network_bodies_persist_plaintext_and_raw_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let body_store = Arc::new(parking_lot::RwLock::new(crate::BodyStore::new(
        dir.path().join("bodies"),
        1024 * 1024,
        1,
        64 * 1024,
        std::time::Duration::from_secs(1),
    )));
    let plaintext = br#"{"message":"imported body"}"#;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(plaintext).unwrap();
    let compressed = encoder.finish().unwrap();
    let encoded = STANDARD.encode(&compressed);
    let record: NetworkRecord = serde_json::from_value(serde_json::json!({
        "id": "REQ-import-body",
        "method": "POST",
        "url": "https://example.test/import",
        "status": 200,
        "request_headers": [["content-encoding", "gzip"]],
        "response_headers": [["content-encoding", "gzip"]],
        "request_body": std::str::from_utf8(plaintext).unwrap(),
        "request_body_base64": encoded,
        "response_body_base64": STANDARD.encode(&compressed),
        "duration_ms": 1,
        "timestamp": 1
    }))
    .unwrap();
    let mut traffic = network_record_to_traffic_record(&record);

    persist_imported_bodies(&record, &mut traffic, &body_store)
        .expect("persist valid imported bodies");

    let store = body_store.read();

    assert_eq!(
        traffic
            .request_body_ref
            .as_ref()
            .and_then(|body_ref| store.load_bytes(body_ref))
            .as_deref(),
        Some(plaintext.as_slice())
    );
    assert_eq!(
        traffic
            .response_body_ref
            .as_ref()
            .and_then(|body_ref| store.load_bytes(body_ref))
            .as_deref(),
        Some(plaintext.as_slice())
    );
    assert_eq!(
        traffic
            .raw_request_body_ref
            .as_ref()
            .and_then(|body_ref| store.load_bytes(body_ref))
            .as_deref(),
        Some(compressed.as_slice())
    );
    assert_eq!(
        traffic
            .raw_response_body_ref
            .as_ref()
            .and_then(|body_ref| store.load_bytes(body_ref))
            .as_deref(),
        Some(compressed.as_slice())
    );
}

#[test]
fn imported_oversized_compressed_body_preserves_its_encoding_marker() {
    let dir = tempfile::tempdir().unwrap();
    let body_store = Arc::new(parking_lot::RwLock::new(crate::BodyStore::new(
        dir.path().join("bodies"),
        1024 * 1024,
        1,
        64 * 1024,
        std::time::Duration::from_secs(1),
    )));
    let plaintext = vec![b'a'; DEFAULT_MAX_DECOMPRESSED_BODY_BYTES + 1];
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&plaintext).unwrap();
    let compressed = encoder.finish().unwrap();
    let record: NetworkRecord = serde_json::from_value(serde_json::json!({
        "id": "REQ-import-oversized-gzip",
        "method": "GET",
        "url": "https://example.test/import-oversized-gzip",
        "status": 200,
        "response_headers": [["content-encoding", "gzip"]],
        "response_body_base64": STANDARD.encode(&compressed),
        "duration_ms": 1,
        "timestamp": 1
    }))
    .unwrap();
    let mut traffic = network_record_to_traffic_record(&record);

    persist_imported_bodies(&record, &mut traffic, &body_store)
        .expect("persist oversized compressed body as wire bytes");

    let store = body_store.read();
    let response_ref = traffic.response_body_ref.as_ref().unwrap();
    assert_eq!(response_ref.content_encoding().as_deref(), Some("gzip"));
    assert_eq!(
        store.load_bytes(response_ref).as_deref(),
        Some(compressed.as_slice())
    );
    assert_eq!(
        decompress_with_limit(
            &store.load_bytes(response_ref).unwrap(),
            response_ref.content_encoding().as_deref().unwrap(),
            plaintext.len(),
        )
        .unwrap(),
        plaintext
    );
}

#[tokio::test]
async fn network_import_handler_rejects_body_when_persistence_is_paused() {
    const CHILD: &str = "BIFROST_NETWORK_IMPORT_PRESSURE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "handlers::bifrost_file::tests::network_import_handler_rejects_body_when_persistence_is_paused",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let traffic_store =
        Arc::new(crate::TrafficDbStore::new(dir.path().join("traffic"), 100, 0, None).unwrap());
    let body_store = Arc::new(parking_lot::RwLock::new(crate::BodyStore::new(
        dir.path().join("bodies"),
        1024 * 1024,
        1,
        64 * 1024,
        std::time::Duration::from_secs(1),
    )));
    let storage = RulesStorage::with_dir(dir.path().join("rules")).unwrap();
    let state = Arc::new(
        crate::state::AdminState::new_for_test(TEST_ADMIN_PORT, storage)
            .with_traffic_db_store_shared(traffic_store.clone())
            .with_body_store(body_store),
    );
    let record: NetworkRecord = serde_json::from_value(serde_json::json!({
        "id": "REQ-import-pressure",
        "method": "POST",
        "url": "https://example.test/import-pressure",
        "status": 200,
        "request_body": "must not be dropped",
        "duration_ms": 1,
        "timestamp": 1
    }))
    .unwrap();
    let content = BifrostFileWriter::write_network("import-pressure", None, &[record]).unwrap();

    bifrost_core::publish_resource_pressure(bifrost_core::ResourcePressureLevel::Critical);
    let response = import_network(&content, &state).await;
    bifrost_core::publish_resource_pressure(bifrost_core::ResourcePressureLevel::Normal);

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(traffic_store.get_by_id("OUT-REQ-import-pressure").is_none());
}

#[tokio::test]
async fn network_import_handler_persists_bodies_in_recorded_traffic() {
    let dir = tempfile::tempdir().unwrap();
    let traffic_store =
        Arc::new(crate::TrafficDbStore::new(dir.path().join("traffic"), 100, 0, None).unwrap());
    let body_store = Arc::new(parking_lot::RwLock::new(crate::BodyStore::new(
        dir.path().join("bodies"),
        1024 * 1024,
        1,
        64 * 1024,
        std::time::Duration::from_secs(1),
    )));
    let storage = RulesStorage::with_dir(dir.path().join("rules")).unwrap();
    let state = Arc::new(
        crate::state::AdminState::new_for_test(TEST_ADMIN_PORT, storage)
            .with_traffic_db_store_shared(traffic_store.clone())
            .with_body_store(body_store.clone()),
    );
    let record: NetworkRecord = serde_json::from_value(serde_json::json!({
        "id": "REQ-import-handler",
        "method": "POST",
        "url": "https://example.test/import-handler",
        "status": 200,
        "request_body": "request plaintext",
        "response_body": "response plaintext",
        "duration_ms": 1,
        "timestamp": 1
    }))
    .unwrap();
    let content = BifrostFileWriter::write_network("import-handler", None, &[record]).unwrap();

    let response = import_network(&content, &state).await;
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let result: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let imported = traffic_store
        .get_by_id("OUT-REQ-import-handler")
        .expect("imported traffic record");
    let store = body_store.read();

    assert_eq!(result["success"], true);
    assert_eq!(result["data"]["record_count"], 1);
    assert_eq!(
        imported
            .request_body_ref
            .as_ref()
            .and_then(|body_ref| store.load_bytes(body_ref))
            .as_deref(),
        Some(b"request plaintext".as_slice())
    );
    assert_eq!(
        imported
            .response_body_ref
            .as_ref()
            .and_then(|body_ref| store.load_bytes(body_ref))
            .as_deref(),
        Some(b"response plaintext".as_slice())
    );
}

#[tokio::test]
async fn network_import_rejects_invalid_base64_before_recording_any_rows() {
    let dir = tempfile::tempdir().unwrap();
    let traffic_store =
        Arc::new(crate::TrafficDbStore::new(dir.path().join("traffic"), 100, 0, None).unwrap());
    let storage = RulesStorage::with_dir(dir.path().join("rules")).unwrap();
    let state = Arc::new(
        crate::state::AdminState::new_for_test(TEST_ADMIN_PORT, storage)
            .with_traffic_db_store_shared(traffic_store.clone()),
    );
    let valid: NetworkRecord = serde_json::from_value(serde_json::json!({
        "id": "REQ-valid-before-invalid",
        "method": "GET",
        "url": "https://example.test/valid",
        "status": 200,
        "duration_ms": 1,
        "timestamp": 1
    }))
    .unwrap();
    let invalid: NetworkRecord = serde_json::from_value(serde_json::json!({
        "id": "REQ-invalid-import",
        "method": "POST",
        "url": "https://example.test/invalid",
        "status": 200,
        "response_body_base64": "%%%",
        "duration_ms": 1,
        "timestamp": 2
    }))
    .unwrap();
    let content =
        BifrostFileWriter::write_network("invalid-import", None, &[valid, invalid]).unwrap();

    let response = import_network(&content, &state).await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(traffic_store
        .get_by_id("OUT-REQ-valid-before-invalid")
        .is_none());
    assert!(traffic_store.get_by_id("OUT-REQ-invalid-import").is_none());
}

#[tokio::test]
async fn network_import_handler_warns_when_body_store_is_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let traffic_store =
        Arc::new(crate::TrafficDbStore::new(dir.path().join("traffic"), 100, 0, None).unwrap());
    let storage = RulesStorage::with_dir(dir.path().join("rules")).unwrap();
    let state = Arc::new(
        crate::state::AdminState::new_for_test(TEST_ADMIN_PORT, storage)
            .with_traffic_db_store_shared(traffic_store.clone()),
    );
    let record: NetworkRecord = serde_json::from_value(serde_json::json!({
        "id": "REQ-import-no-body-store",
        "method": "POST",
        "url": "https://example.test/import-no-body-store",
        "status": 200,
        "request_body": "request plaintext",
        "duration_ms": 1,
        "timestamp": 1
    }))
    .unwrap();
    let content =
        BifrostFileWriter::write_network("import-no-body-store", None, &[record]).unwrap();

    let response = import_network(&content, &state).await;
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let result: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let imported = traffic_store
        .get_by_id("OUT-REQ-import-no-body-store")
        .expect("imported traffic record");

    assert!(imported.request_body_ref.is_none());
    assert!(result["warnings"].as_array().is_some_and(|warnings| {
        warnings.iter().any(|warning| {
            warning
                .as_str()
                .is_some_and(|text| text.contains("body store is unavailable"))
        })
    }));
}

#[test]
fn imported_record_does_not_restore_removed_response_content_type() {
    let record: NetworkRecord = serde_json::from_value(serde_json::json!({
        "id": "REQ-content-type-removed",
        "method": "GET",
        "url": "https://example.test/content-type",
        "status": 200,
        "response_headers": [["x-delivered", "yes"]],
        "original_response_headers": [["content-type", "application/json"]],
        "duration_ms": 1,
        "timestamp": 1
    }))
    .unwrap();

    let traffic = network_record_to_traffic_record(&record);

    assert!(traffic.content_type.is_none());
    assert_eq!(
        traffic.original_response_headers,
        record.original_response_headers
    );
    assert_eq!(traffic.response_headers, record.response_headers);
}

#[test]
fn preview_decodes_lossless_gzip_body_field() {
    let plaintext = br#"{"message":"hello from package"}"#;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(plaintext).unwrap();
    let compressed = encoder.finish().unwrap();
    let record: NetworkRecord = serde_json::from_value(serde_json::json!({
        "id": "REQ-base64-preview",
        "method": "POST",
        "url": "https://example.test/gzip",
        "status": 200,
        "request_headers": [
            ["content-type", "application/json"],
            ["content-encoding", "gzip"]
        ],
        "request_body_base64": base64::engine::general_purpose::STANDARD.encode(compressed),
        "duration_ms": 1,
        "timestamp": 1
    }))
    .unwrap();

    let preview = build_network_preview(&[record]);
    let detail = preview.single_record.expect("single record detail");

    assert_eq!(
        detail.request_body.as_deref(),
        std::str::from_utf8(plaintext).ok()
    );
    assert_eq!(
        detail.record.request_content_type.as_deref(),
        Some("application/json")
    );
    assert!(preview.warnings.is_empty());
}

#[test]
fn network_record_import_preserves_routing_diagnostics() {
    let record = NetworkRecord {
        id: "REQ-exported".to_string(),
        method: "GET".to_string(),
        url: "https://lf6-cdn2-tos.bytegoofy.com/index.html".to_string(),
        status: 502,
        host: Some("lf6-cdn2-tos.bytegoofy.com".to_string()),
        path: Some("/index.html".to_string()),
        protocol: Some("https".to_string()),
        actual_url: Some("http://10.37.102.138:8081/index.html".to_string()),
        actual_host: Some("10.37.102.138".to_string()),
        listener_port: Some(TEST_ADMIN_PORT),
        has_rule_hit: Some(true),
        error_message: Some("Request Failed".to_string()),
        client_app: Some("Google Chrome".to_string()),
        client_path: Some("/Applications/Google Chrome.app".to_string()),
        request_headers: None,
        response_headers: None,
        original_response_headers: None,
        request_body: None,
        request_body_base64: None,
        response_body: None,
        response_body_base64: None,
        duration_ms: 78,
        timestamp: 1779283635053,
        matched_rules: Some(vec![MatchedRuleExport {
            pattern: "lf6-cdn2-tos.bytegoofy.com/index.html".to_string(),
            protocol: "Host".to_string(),
            value: "10.37.102.138:8081".to_string(),
        }]),
        active_rules: None,
    };

    let traffic = network_record_to_traffic_record(&record);

    assert_eq!(traffic.id, "OUT-REQ-exported");
    assert_eq!(traffic.host, "lf6-cdn2-tos.bytegoofy.com");
    assert_eq!(traffic.path, "/index.html");
    assert_eq!(traffic.protocol, "https");
    assert_eq!(
        traffic.actual_url.as_deref(),
        Some("http://10.37.102.138:8081/index.html")
    );
    assert_eq!(traffic.actual_host.as_deref(), Some("10.37.102.138"));
    assert_eq!(traffic.listener_port, TEST_ADMIN_PORT);
    assert!(traffic.has_rule_hit);
    assert_eq!(traffic.error_message.as_deref(), Some("Request Failed"));
    assert_eq!(traffic.client_app.as_deref(), Some("Google Chrome"));
    assert_eq!(
        traffic
            .matched_rules
            .as_ref()
            .and_then(|rules| rules.first())
            .map(|r| { (r.pattern.as_str(), r.protocol.as_str(), r.value.as_str(),) }),
        Some((
            "lf6-cdn2-tos.bytegoofy.com/index.html",
            "Host",
            "10.37.102.138:8081"
        ))
    );
}

#[test]
fn network_import_rejects_empty_package() {
    let err = validate_network_import_records(0).unwrap_err();
    assert_eq!(err, EMPTY_NETWORK_IMPORT_ERROR);
}

#[test]
fn preview_rules_includes_rule_details() {
    let content = r#"01 rules

[meta]
name = "preview-rules"
enabled = false
description = "preview before import"

---
example.com proxy://127.0.0.1:8080
"#;

    let preview = build_preview(content).expect("rules preview");

    assert_eq!(preview.file_type, BifrostFileType::Rules);
    assert_eq!(preview.item_count, Some(1));
    let rules = preview.rules.expect("rules preview payload");
    assert_eq!(rules.name, "preview-rules");
    assert!(!rules.enabled);
    assert_eq!(rules.description.as_deref(), Some("preview before import"));
    assert_eq!(rules.line_count, 1);
    assert_eq!(rules.content.trim(), "example.com proxy://127.0.0.1:8080");
}

#[test]
fn preview_single_network_record_includes_detail_payload() {
    let records = vec![NetworkRecord {
        id: "REQ-preview".to_string(),
        method: "POST".to_string(),
        url: "https://api.example.test/v1/items?limit=1".to_string(),
        status: 201,
        host: Some("api.example.test".to_string()),
        path: Some("/v1/items?limit=1".to_string()),
        protocol: Some("https".to_string()),
        actual_url: None,
        actual_host: None,
        listener_port: Some(TEST_ADMIN_PORT),
        has_rule_hit: Some(false),
        error_message: None,
        client_app: Some("Preview Client".to_string()),
        client_path: None,
        request_headers: Some(vec![(
            "content-type".to_string(),
            "application/json".to_string(),
        )]),
        response_headers: Some(vec![(
            "content-type".to_string(),
            "application/json".to_string(),
        )]),
        original_response_headers: None,
        request_body: Some(r#"{"name":"preview"}"#.to_string()),
        request_body_base64: None,
        response_body: Some(r#"{"ok":true}"#.to_string()),
        response_body_base64: None,
        duration_ms: 42,
        timestamp: 1779283635053,
        matched_rules: None,
        active_rules: None,
    }];
    let content = BifrostFileWriter::write_network("preview-network", None, &records)
        .expect("network package");

    let preview = build_preview(&content).expect("network preview");

    assert_eq!(preview.file_type, BifrostFileType::Network);
    assert_eq!(preview.item_count, Some(1));
    let network = preview.network.expect("network preview payload");
    assert_eq!(network.record_count, 1);
    assert_eq!(network.hosts, vec!["api.example.test".to_string()]);
    assert_eq!(network.records.len(), 1);
    assert_eq!(network.records[0].method, "POST");
    let detail = network.single_record.expect("single record detail");
    assert_eq!(detail.record.id, "OUT-REQ-preview");
    assert_eq!(detail.record.host, "api.example.test");
    assert_eq!(
        detail.record.content_type.as_deref(),
        Some("application/json")
    );
    assert_eq!(
        detail.record.request_content_type.as_deref(),
        Some("application/json")
    );
    assert_eq!(
        detail.record.original_response_headers,
        Some(vec![(
            "content-type".to_string(),
            "application/json".to_string(),
        )])
    );
    assert!(detail.record.response_headers.is_none());
    assert_eq!(
        detail.request_body.as_deref(),
        Some(r#"{"name":"preview"}"#)
    );
    assert_eq!(detail.response_body.as_deref(), Some(r#"{"ok":true}"#));
}

#[test]
fn network_export_rejects_empty_selection() {
    let err = validate_network_export_records(&[], &[], 0).unwrap_err();
    assert!(err.contains("Select at least one Network record"));
}

#[test]
fn network_export_rejects_missing_selected_records() {
    let requested = vec!["A".to_string(), "B".to_string()];
    let missing = vec!["B".to_string()];

    let err = validate_network_export_records(&requested, &missing, 1).unwrap_err();

    assert!(err.contains("1 selected record(s) no longer exist"));
    assert!(err.contains("B"));
}

#[test]
fn network_export_allows_resolved_records() {
    let requested = vec!["A".to_string()];

    validate_network_export_records(&requested, &[], 1).unwrap();
}

#[test]
fn network_import_preserves_original_and_delivered_response_headers() {
    let original = vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("connection".to_string(), "keep-alive".to_string()),
    ];
    let delivered = vec![("content-type".to_string(), "application/json".to_string())];
    let record = NetworkRecord {
        id: "REQ-header-snapshots".to_string(),
        method: "GET".to_string(),
        url: "https://example.test/headers".to_string(),
        status: 200,
        host: None,
        path: None,
        protocol: None,
        actual_url: None,
        actual_host: None,
        listener_port: None,
        has_rule_hit: Some(false),
        error_message: None,
        client_app: None,
        client_path: None,
        request_headers: None,
        response_headers: Some(delivered.clone()),
        original_response_headers: Some(original.clone()),
        request_body: None,
        request_body_base64: None,
        response_body: None,
        response_body_base64: None,
        duration_ms: 1,
        timestamp: 1,
        matched_rules: None,
        active_rules: None,
    };

    let traffic = network_record_to_traffic_record(&record);

    assert_eq!(traffic.original_response_headers, Some(original));
    assert_eq!(traffic.response_headers, Some(delivered));
    assert!(!traffic.has_rule_hit);
}

#[tokio::test]
async fn network_export_writes_original_and_delivered_response_headers() {
    let dir = tempfile::tempdir().unwrap();
    let storage = RulesStorage::with_dir(dir.path().to_path_buf()).unwrap();
    let state = Arc::new(crate::state::AdminState::new_for_test(
        TEST_ADMIN_PORT,
        storage,
    ));
    let original = vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("connection".to_string(), "keep-alive".to_string()),
    ];
    let delivered = vec![("content-type".to_string(), "application/json".to_string())];
    let mut traffic = TrafficRecord::new(
        "REQ-header-export".to_string(),
        "GET".to_string(),
        "https://example.test/headers".to_string(),
    );
    traffic.original_response_headers = Some(original.clone());
    traffic.response_headers = Some(delivered.clone());

    let record = traffic_to_network_record(&traffic, false, &state).await;

    assert_eq!(record.original_response_headers, Some(original));
    assert_eq!(record.response_headers, Some(delivered));
}

#[tokio::test]
async fn network_export_attaches_default_port_active_rules() {
    let dir = tempfile::tempdir().unwrap();
    let storage = RulesStorage::with_dir(dir.path().to_path_buf()).unwrap();
    storage
        .save(&RuleFile::new(
            "default-rule",
            "default.example.test status://209",
        ))
        .unwrap();
    storage
        .save(
            &RuleFile::new("disabled-rule", "disabled.example.test status://500")
                .with_enabled(false),
        )
        .unwrap();
    let state = Arc::new(crate::state::AdminState::new_for_test(
        TEST_ADMIN_PORT,
        storage,
    ));
    let mut traffic = TrafficRecord::new(
        "REQ-1".to_string(),
        "GET".to_string(),
        "http://default.example.test/".to_string(),
    );
    traffic.listener_port = TEST_ADMIN_PORT;

    let record = traffic_to_network_record(&traffic, false, &state).await;
    let active_rules = record.active_rules.expect("active rules snapshot");

    assert_eq!(record.listener_port, Some(TEST_ADMIN_PORT));
    assert_eq!(active_rules.source, ActiveRuleSource::DefaultPort);
    assert_eq!(active_rules.listener_port, TEST_ADMIN_PORT);
    assert_eq!(active_rules.total, 1);
    assert_eq!(active_rules.rules[0].name, "default-rule");
    assert_eq!(
        active_rules.rules[0].content.as_deref(),
        Some("default.example.test status://209")
    );
    assert!(active_rules
        .merged_content
        .contains("default.example.test status://209"));
    assert!(!active_rules
        .merged_content
        .contains("disabled.example.test"));
}

#[tokio::test]
async fn network_export_reports_empty_default_rules_when_rule_dir_missing() {
    let dir = tempfile::tempdir().unwrap();
    let storage = RulesStorage::with_dir(dir.path().join("missing-rules")).unwrap();
    let state = Arc::new(crate::state::AdminState::new_for_test(
        TEST_ADMIN_PORT,
        storage,
    ));
    let mut traffic = TrafficRecord::new(
        "REQ-empty".to_string(),
        "GET".to_string(),
        "http://empty.example.test/".to_string(),
    );
    traffic.listener_port = TEST_ADMIN_PORT;

    let record = traffic_to_network_record(&traffic, false, &state).await;
    let active_rules = record.active_rules.expect("active rules snapshot");

    assert_eq!(active_rules.source, ActiveRuleSource::DefaultPort);
    assert_eq!(active_rules.total, 0);
    assert!(active_rules.rules.is_empty());
    assert!(active_rules.merged_content.is_empty());
    assert!(active_rules.unavailable_reason.is_none());
}

#[tokio::test]
async fn network_export_uses_custom_port_active_rules_for_request_port() {
    let dir = tempfile::tempdir().unwrap();
    let storage = RulesStorage::with_dir(dir.path().to_path_buf()).unwrap();
    storage
        .save(&RuleFile::new(
            "default-rule",
            "default.example.test status://209",
        ))
        .unwrap();
    let state = Arc::new(crate::state::AdminState::new_for_test(
        TEST_ADMIN_PORT,
        storage,
    ));
    state.set_temporary_port_manager(Arc::new(FakeTemporaryPortManager));
    let mut traffic = TrafficRecord::new(
        "REQ-2".to_string(),
        "GET".to_string(),
        "http://custom.example.test/".to_string(),
    );
    traffic.listener_port = 19090;

    let record = traffic_to_network_record(&traffic, false, &state).await;
    let active_rules = record.active_rules.expect("active rules snapshot");

    assert_eq!(record.listener_port, Some(19090));
    assert_eq!(active_rules.source, ActiveRuleSource::CustomPort);
    assert_eq!(active_rules.admin_port, TEST_ADMIN_PORT);
    assert_eq!(active_rules.listener_port, 19090);
    assert_eq!(active_rules.total, 1);
    assert_eq!(active_rules.rules[0].name, "custom-port-rule");
    assert_eq!(
        active_rules.rules[0].content.as_deref(),
        Some("custom.example.test status://210")
    );
    assert!(active_rules
        .merged_content
        .contains("custom.example.test status://210"));
    assert!(!active_rules.merged_content.contains("default.example.test"));
}

struct FakeTemporaryPortManager;

#[async_trait]
impl crate::temp_ports::TemporaryPortManager for FakeTemporaryPortManager {
    async fn bind(
        &self,
        _req: crate::temp_ports::TemporaryPortBindRequest,
    ) -> Result<crate::temp_ports::TemporaryPortBinding, crate::temp_ports::TemporaryPortError>
    {
        unreachable!("bind is not used by this test")
    }

    async fn update(
        &self,
        _port: u16,
        _req: crate::temp_ports::TemporaryPortUpdateRequest,
    ) -> Result<crate::temp_ports::TemporaryPortBinding, crate::temp_ports::TemporaryPortError>
    {
        unreachable!("update is not used by this test")
    }

    async fn destroy(
        &self,
        _port: u16,
    ) -> Result<crate::temp_ports::TemporaryPortBinding, crate::temp_ports::TemporaryPortError>
    {
        unreachable!("destroy is not used by this test")
    }

    async fn list(&self) -> Vec<crate::temp_ports::TemporaryPortBinding> {
        Vec::new()
    }

    async fn show(
        &self,
        _port: u16,
    ) -> Result<crate::temp_ports::TemporaryPortBinding, crate::temp_ports::TemporaryPortError>
    {
        unreachable!("show is not used by this test")
    }

    async fn active_summary(
        &self,
        port: u16,
    ) -> Result<crate::temp_ports::TemporaryPortActiveSummary, crate::temp_ports::TemporaryPortError>
    {
        Ok(crate::temp_ports::TemporaryPortActiveSummary {
            port,
            total: 1,
            rules: vec![crate::temp_ports::TemporaryPortRuleItem {
                name: "custom-port-rule".to_string(),
                rule_count: 1,
                group_id: None,
                group_name: None,
                content: Some("custom.example.test status://210".to_string()),
            }],
            merged_content: "custom.example.test status://210".to_string(),
        })
    }
}

#[test]
fn count_rules_ignores_blank_and_comment_lines() {
    let content = "# comment\n\n rule-one  \n   # another comment\nrule-two\n   \n";
    assert_eq!(count_rules(content), 2);
}

#[test]
fn toml_to_json_converts_common_value_types() {
    use toml::Value;

    let mut table = toml::map::Map::new();
    table.insert("s".to_string(), Value::String("v".to_string()));
    table.insert("i".to_string(), Value::Integer(1));
    table.insert("b".to_string(), Value::Boolean(true));
    table.insert(
        "arr".to_string(),
        Value::Array(vec![Value::Integer(2), Value::Integer(3)]),
    );
    let outer = Value::Table(table);

    let json = toml_to_json(outer);
    assert_eq!(json["s"], "v");
    assert_eq!(json["i"], 1);
    assert_eq!(json["b"], true);
    assert_eq!(json["arr"][0], 2);
    assert_eq!(json["arr"][1], 3);
}

#[test]
fn convert_to_replay_request_maps_request_and_body_types() {
    use crate::replay_db::{BodyType, RawType, RequestSource, RequestType};

    let headers = vec![KeyValueItemExport {
        id: "h1".to_string(),
        key: "X-Test".to_string(),
        value: "v".to_string(),
        enabled: true,
        description: Some("header".to_string()),
    }];

    let body_export = ReplayBodyExport {
        body_type: "form-data".to_string(),
        raw_type: Some("json".to_string()),
        content: Some("body".to_string()),
        form_data: vec![KeyValueItemExport {
            id: "f1".to_string(),
            key: "k".to_string(),
            value: "v".to_string(),
            enabled: true,
            description: None,
        }],
        binary_file: Some("file.bin".to_string()),
    };

    let export = ReplayRequestExport {
        id: "rid".to_string(),
        group_id: Some("gid".to_string()),
        name: Some("name".to_string()),
        request_type: "sse".to_string(),
        method: "GET".to_string(),
        url: "http://example.test/".to_string(),
        headers: headers.clone(),
        body: Some(body_export),
        is_saved: true,
        sort_order: 42,
        created_at: 100,
        updated_at: 200,
    };

    let replay = convert_to_replay_request(&export, 7);

    assert_eq!(replay.id, "OUT-007");
    assert_eq!(replay.group_id.as_deref(), Some("gid"));
    assert_eq!(replay.name.as_deref(), Some("name"));
    assert_eq!(replay.request_type, RequestType::Sse);
    assert_eq!(replay.method, "GET");
    assert_eq!(replay.url, "http://example.test/");
    assert!(replay.is_saved);
    assert_eq!(replay.sort_order, 42);
    assert_eq!(replay.source, RequestSource::Imported);
    assert_eq!(replay.created_at, 100);
    assert_eq!(replay.updated_at, 200);
    assert_eq!(replay.headers.len(), 1);
    assert_eq!(replay.headers[0].key, "X-Test");
    assert_eq!(replay.headers[0].value, "v");
    assert_eq!(replay.headers[0].description.as_deref(), Some("header"));

    let body = replay.body.expect("body");
    assert_eq!(body.body_type, BodyType::FormData);
    assert!(matches!(body.raw_type, Some(RawType::Json)));
    assert_eq!(body.content.as_deref(), Some("body"));
    assert_eq!(body.form_data.len(), 1);
    assert_eq!(body.form_data[0].key, "k");
    assert_eq!(body.binary_file.as_deref(), Some("file.bin"));
}

#[test]
fn convert_from_replay_request_maps_request_and_body_types() {
    use crate::replay_db::{
        BodyType, KeyValueItem, RawType, ReplayBody, ReplayRequest, RequestSource, RequestType,
    };

    let headers = vec![KeyValueItem {
        id: "h1".to_string(),
        key: "X-Test".to_string(),
        value: "v".to_string(),
        enabled: true,
        description: Some("header".to_string()),
    }];

    let body = ReplayBody {
        body_type: BodyType::Raw,
        raw_type: Some(RawType::Xml),
        content: Some("body".to_string()),
        form_data: vec![KeyValueItem {
            id: "f1".to_string(),
            key: "k".to_string(),
            value: "v".to_string(),
            enabled: true,
            description: None,
        }],
        binary_file: Some("file.bin".to_string()),
    };

    let request = ReplayRequest {
        id: "rid".to_string(),
        group_id: Some("gid".to_string()),
        name: Some("name".to_string()),
        request_type: RequestType::WebSocket,
        method: "POST".to_string(),
        url: "ws://example.test/".to_string(),
        headers,
        body: Some(body),
        is_saved: true,
        sort_order: 11,
        source: RequestSource::Imported,
        created_at: 123,
        updated_at: 456,
    };

    let export = convert_from_replay_request(&request);

    assert_eq!(export.id, "rid");
    assert_eq!(export.group_id.as_deref(), Some("gid"));
    assert_eq!(export.name.as_deref(), Some("name"));
    assert_eq!(export.request_type, "websocket");
    assert_eq!(export.method, "POST");
    assert_eq!(export.url, "ws://example.test/");
    assert!(export.is_saved);
    assert_eq!(export.sort_order, 11);
    assert_eq!(export.created_at, 123);
    assert_eq!(export.updated_at, 456);
    assert_eq!(export.headers.len(), 1);
    assert_eq!(export.headers[0].key, "X-Test");
    assert_eq!(export.headers[0].value, "v");
    assert_eq!(export.headers[0].description.as_deref(), Some("header"));

    let body_export = export.body.expect("body");
    assert_eq!(body_export.body_type, "raw");
    assert_eq!(body_export.raw_type.as_deref(), Some("xml"));
    assert_eq!(body_export.content.as_deref(), Some("body"));
    assert_eq!(body_export.form_data.len(), 1);
    assert_eq!(body_export.form_data[0].key, "k");
    assert_eq!(body_export.form_data[0].value, "v");
    assert_eq!(body_export.binary_file.as_deref(), Some("file.bin"));
}

#[test]
fn unavailable_active_rules_export_is_empty_with_reason() {
    let export = unavailable_active_rules_export(
        ActiveRuleSource::CustomPort,
        TEST_ADMIN_PORT,
        19090,
        "missing manager",
    );

    assert_eq!(export.source, ActiveRuleSource::CustomPort);
    assert_eq!(export.admin_port, TEST_ADMIN_PORT);
    assert_eq!(export.listener_port, 19090);
    assert_eq!(export.total, 0);
    assert!(export.rules.is_empty());
    assert!(export.merged_content.is_empty());
    assert_eq!(
        export.unavailable_reason.as_deref(),
        Some("missing manager")
    );
}

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
