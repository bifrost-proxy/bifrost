use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use tempfile::TempDir;

use super::{BodyReadCache, SearchEngine};
use crate::body_store::{BodyRef, BodyStore};
use crate::search::types::FilterCondition;
use crate::search::{SearchFilters, SearchRequest, SearchScope, TimeRange};
use crate::traffic::TrafficRecord;
use crate::traffic_db::TrafficDbStore;

#[test]
fn targeted_record_ids_preserve_keyword_and_filter_intersection() {
    let dir = TempDir::new().expect("temp dir");
    let db = Arc::new(
        TrafficDbStore::new(dir.path().join("traffic"), 1024, 64 * 1024 * 1024, Some(24))
            .expect("traffic db"),
    );

    for (id, method, path) in [
        ("target-post", "POST", "/live-search-marker/target"),
        ("target-get", "GET", "/live-search-marker/wrong-method"),
        (
            "outside-post",
            "POST",
            "/live-search-marker/outside-id-scope",
        ),
    ] {
        db.record(TrafficRecord::new(
            id.to_string(),
            method.to_string(),
            format!("https://example.com{path}"),
        ));
    }

    let engine = SearchEngine::new(db, None);
    let response = engine.search(&SearchRequest {
        keyword: "live-search-marker".to_string(),
        scope: SearchScope {
            all: false,
            url: true,
            ..Default::default()
        },
        filters: SearchFilters {
            conditions: vec![FilterCondition {
                field: "method".to_string(),
                operator: "equals".to_string(),
                value: "POST".to_string(),
            }],
            ..Default::default()
        },
        record_ids: vec!["target-post".to_string(), "target-get".to_string()],
        limit: Some(500),
        max_scan: Some(500),
        max_results: Some(500),
        ..Default::default()
    });

    assert_eq!(response.total_matched, 1);
    assert_eq!(response.results[0].record.id, "target-post");
    assert!(response
        .results
        .iter()
        .all(|item| item.record.id != "outside-post"));
}

#[test]
fn response_body_search_prefers_derived_sse_body() {
    let dir = TempDir::new().expect("temp dir");
    let db = Arc::new(
        TrafficDbStore::new(dir.path().join("traffic"), 1024, 64 * 1024 * 1024, Some(24))
            .expect("traffic db"),
    );

    let mut record = TrafficRecord::new(
        "REQ-search-derived".to_string(),
        "GET".to_string(),
        "https://example.com/v1/chat/completions".to_string(),
    );
    record.set_sse();
    record.response_body_ref = Some(BodyRef::Inline {
        data: concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello \"}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"world\"}}]}\n\n",
            "data: [DONE]\n\n"
        )
        .to_string(),
    });
    record.derived_response_body_ref = Some(BodyRef::Inline {
        data: serde_json::json!({
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "hello world"
                },
                "finish_reason": "stop"
            }]
        })
        .to_string(),
    });
    db.record(record);

    let engine = SearchEngine::new(db, None);
    let response = engine.search(&SearchRequest {
        keyword: "hello world".to_string(),
        scope: SearchScope {
            all: false,
            response_body: true,
            ..Default::default()
        },
        filters: SearchFilters::default(),
        cursor: None,
        limit: Some(20),
        max_scan: None,
        max_results: None,
        time_range: None,
        ..Default::default()
    });

    assert_eq!(response.total_matched, 1);
    assert_eq!(response.results[0].matches[0].field, "response_body");
    assert!(response.results[0].matches[0]
        .preview
        .contains("hello world"));
}

#[test]
fn response_body_search_finds_decoded_bp_body() {
    let dir = TempDir::new().expect("temp dir");
    let db = Arc::new(
        TrafficDbStore::new(dir.path().join("traffic"), 1024, 64 * 1024 * 1024, Some(24))
            .expect("traffic db"),
    );

    let mut record = TrafficRecord::new(
        "REQ-search-bp".to_string(),
        "POST".to_string(),
        "https://example.com/bp".to_string(),
    );
    record.raw_response_body_ref = Some(BodyRef::Inline {
        data: "\u{0000}\u{0001}binary".to_string(),
    });
    record.response_body_ref = Some(BodyRef::Inline {
        data: serde_json::json!({
            "decoded": true,
            "marker": "bp-search-unique-needle"
        })
        .to_string(),
    });
    db.record(record);

    let engine = SearchEngine::new(db, None);
    let response = engine.search(&SearchRequest {
        keyword: "bp-search-unique-needle".to_string(),
        scope: SearchScope {
            all: false,
            response_body: true,
            ..Default::default()
        },
        filters: SearchFilters::default(),
        cursor: None,
        limit: Some(20),
        max_scan: None,
        max_results: None,
        record_ids: Vec::new(),
        include: Default::default(),
        time_range: None,
    });

    assert_eq!(response.total_matched, 1);
    assert_eq!(response.results[0].record.id, "REQ-search-bp");
    assert_eq!(response.results[0].matches[0].field, "response_body");
}

#[test]
fn request_body_scope_searches_the_primary_body_reference() {
    let db = make_db();
    let mut record = TrafficRecord::new(
        "REQ-search-request-body".to_string(),
        "POST".to_string(),
        "https://example.com/request-body".to_string(),
    );
    record.request_body_ref = Some(BodyRef::Inline {
        data: r#"{"marker":"request-body-unique-needle"}"#.to_string(),
    });
    db.record(record);

    let response = SearchEngine::new(db, None).search(&SearchRequest {
        keyword: "request-body-unique-needle".to_string(),
        scope: SearchScope {
            all: false,
            request_body: true,
            ..Default::default()
        },
        limit: Some(20),
        ..Default::default()
    });

    assert_eq!(response.total_matched, 1);
    assert_eq!(response.results[0].matches[0].field, "request_body");
}

#[test]
fn response_body_search_matches_ascii_file_body_without_lowercase_allocation() {
    let dir = TempDir::new().expect("temp dir");
    let db = Arc::new(
        TrafficDbStore::new(dir.path().join("traffic"), 1024, 64 * 1024 * 1024, Some(24))
            .expect("traffic db"),
    );
    let body_store = Arc::new(RwLock::new(BodyStore::new(
        dir.path().join("body_cache"),
        0,
        7,
        64 * 1024,
        Duration::from_millis(100),
    )));
    let body_ref = body_store
        .read()
        .store(
            "REQ-search-ascii-file",
            "res",
            br#"{"ok":true,"marker":"STORAGE-BODY-Needle-42","padding":"xxxxxxxx"}"#,
        )
        .expect("store body");

    let mut record = TrafficRecord::new(
        "REQ-search-ascii-file".to_string(),
        "GET".to_string(),
        "https://example.com/ascii-file".to_string(),
    );
    record.response_body_ref = Some(body_ref);
    db.record(record);

    let engine = SearchEngine::new(db, Some(body_store));
    let response = engine.search(&SearchRequest {
        keyword: "STORAGE-BODY-NEEDLE-42".to_string(),
        scope: SearchScope {
            all: false,
            response_body: true,
            ..Default::default()
        },
        filters: SearchFilters::default(),
        cursor: None,
        limit: Some(20),
        max_scan: None,
        max_results: None,
        record_ids: Vec::new(),
        include: Default::default(),
        time_range: None,
    });

    assert_eq!(response.total_matched, 1);
    assert_eq!(response.results[0].record.id, "REQ-search-ascii-file");
    assert_eq!(response.results[0].matches[0].field, "response_body");
    assert!(response.results[0].matches[0]
        .preview
        .contains("STORAGE-BODY-Needle-42"));
}

#[test]
fn encoded_file_body_is_decoded_for_search_json_filter_and_include() {
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};

    let dir = TempDir::new().expect("temp dir");
    let db = Arc::new(
        TrafficDbStore::new(dir.path().join("traffic"), 1024, 64 * 1024 * 1024, Some(24))
            .expect("traffic db"),
    );
    let body_store = Arc::new(RwLock::new(BodyStore::new(
        dir.path().join("body_cache"),
        0,
        7,
        64 * 1024,
        Duration::from_millis(100),
    )));
    let plaintext = br#"{"marker":"encoded-search-needle","ok":true}"#;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(plaintext).expect("compress body");
    let compressed = encoder.finish().expect("finish gzip body");
    let body_ref = body_store
        .read()
        .store("REQ-search-encoded", "res", &compressed)
        .expect("store compressed body")
        .with_content_encoding(Some("gzip"))
        .unwrap();
    let mut record = TrafficRecord::new(
        "REQ-search-encoded".to_string(),
        "GET".to_string(),
        "https://example.com/encoded".to_string(),
    );
    record.response_body_ref = Some(body_ref);
    db.record(record);
    let engine = SearchEngine::new(db, Some(body_store));

    let response = engine.search(&SearchRequest {
        keyword: "encoded-search-needle".to_string(),
        scope: SearchScope {
            all: false,
            response_body: true,
            ..Default::default()
        },
        filters: SearchFilters {
            conditions: vec![FilterCondition {
                field: "res.body.$.ok".to_string(),
                operator: "equals".to_string(),
                value: "true".to_string(),
            }],
            ..Default::default()
        },
        include: crate::search::SearchInclude {
            response_body: true,
            ..Default::default()
        },
        limit: Some(20),
        ..Default::default()
    });

    assert_eq!(response.total_matched, 1);
    assert!(response.results[0].matches[0]
        .preview
        .contains("encoded-search-needle"));
    let chunk = response.results[0]
        .bodies
        .as_ref()
        .and_then(|bodies| bodies.response.as_ref())
        .expect("included response body");
    assert_eq!(
        BASE64.decode(&chunk.bytes_b64).expect("decode base64"),
        plaintext
    );
    assert_eq!(chunk.size, plaintext.len());
}

#[test]
fn encoded_body_cache_honors_per_body_and_request_budgets() {
    let dir = TempDir::new().expect("temp dir");
    let db = Arc::new(
        TrafficDbStore::new(dir.path().join("traffic"), 1024, 64 * 1024 * 1024, Some(24))
            .expect("traffic db"),
    );
    let body_store = Arc::new(RwLock::new(BodyStore::new(
        dir.path().join("body_cache"),
        0,
        7,
        64 * 1024,
        Duration::from_millis(100),
    )));
    let plaintext = b"search-budget-plaintext";
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(plaintext).unwrap();
    let compressed = encoder.finish().unwrap();
    let first = body_store
        .read()
        .store("budget-first", "res", &compressed)
        .unwrap()
        .with_content_encoding(Some("gzip"))
        .unwrap();
    let second = body_store
        .read()
        .store("budget-second", "res", &compressed)
        .unwrap()
        .with_content_encoding(Some("gzip"))
        .unwrap();
    let custom = body_store
        .read()
        .store("budget-custom", "res", b"custom-wire-needle")
        .unwrap()
        .with_content_encoding(Some("x-company-codec"))
        .unwrap();
    let engine = SearchEngine::new(db, Some(body_store))
        .with_decompression_budget(plaintext.len(), plaintext.len());
    let mut cache = BodyReadCache::new(plaintext.len());

    assert_eq!(
        engine
            .load_body_bytes_cached("res:first", &first, &mut cache)
            .unwrap(),
        plaintext
    );
    assert_eq!(cache.remaining_decompressed_bytes, 0);
    assert_eq!(
        engine
            .load_body_bytes_cached("res:first", &first, &mut cache)
            .unwrap(),
        plaintext,
        "the second phase must reuse the first decoded value"
    );
    assert!(engine
        .load_body_bytes_cached("res:second", &second, &mut cache)
        .is_none());
    assert!(cache.decompression_budget_exhausted);

    let mut mid_body_cache = BodyReadCache::new(plaintext.len() - 1);
    assert!(engine
        .load_body_bytes_cached("res:mid-body", &first, &mut mid_body_cache)
        .is_none());
    assert!(mid_body_cache.decompression_budget_exhausted);
    assert!(mid_body_cache.current_record_exceeds_decompression_budget);
    assert_eq!(mid_body_cache.remaining_decompressed_bytes, 0);

    let limited_engine = engine.with_decompression_limit(plaintext.len() - 1);
    let mut limited_cache = BodyReadCache::new(plaintext.len() * 2);
    assert_eq!(
        limited_engine
            .load_body_bytes_cached("res:limited", &first, &mut limited_cache)
            .unwrap(),
        compressed,
        "the configured per-body limit must be honored"
    );
    assert_eq!(
        limited_engine
            .load_body_bytes_cached("res:custom", &custom, &mut limited_cache)
            .unwrap(),
        b"custom-wire-needle",
        "unknown codings must remain available to custom decoders"
    );
}

#[test]
fn search_reports_partial_results_when_decompression_budget_is_exhausted() {
    let dir = TempDir::new().expect("temp dir");
    let db = Arc::new(
        TrafficDbStore::new(dir.path().join("traffic"), 1024, 64 * 1024 * 1024, Some(24))
            .expect("traffic db"),
    );
    let body_store = Arc::new(RwLock::new(BodyStore::new(
        dir.path().join("body_cache"),
        0,
        7,
        64 * 1024,
        Duration::from_millis(100),
    )));
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(b"budget-exhaustion-needle").unwrap();
    let compressed = encoder.finish().unwrap();
    let body_ref = body_store
        .read()
        .store("budget-exhausted", "res", &compressed)
        .unwrap()
        .with_content_encoding(Some("gzip"))
        .unwrap();
    let mut record = TrafficRecord::new(
        "budget-exhausted".to_string(),
        "GET".to_string(),
        "https://example.com/budget".to_string(),
    );
    record.response_body_ref = Some(body_ref);
    db.record(record);

    let engine = SearchEngine::new(db, Some(body_store)).with_decompression_budget(1024, 0);
    let request = SearchRequest {
        keyword: "budget-exhaustion-needle".to_string(),
        scope: SearchScope {
            all: false,
            response_body: true,
            ..Default::default()
        },
        limit: Some(20),
        ..Default::default()
    };
    let response = engine.search(&request);

    assert!(response.results.is_empty());
    assert_eq!(response.total_searched, 1);
    assert!(response.next_cursor.is_some());
    assert!(response.has_more);
    assert_eq!(
        response.partial_reason.as_deref(),
        Some("decompression_budget_exhausted")
    );

    let continuation = engine.search(&SearchRequest {
        cursor: response.next_cursor,
        ..request
    });
    assert!(continuation.results.is_empty());
    assert_eq!(continuation.total_searched, 0);
    assert!(!continuation.has_more);
    assert!(continuation.partial_reason.is_none());
}

#[test]
fn search_advances_when_one_records_bodies_exceed_a_fresh_budget_together() {
    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    let dir = TempDir::new().expect("temp dir");
    let db = Arc::new(
        TrafficDbStore::new(dir.path().join("traffic"), 1024, 64 * 1024 * 1024, Some(24))
            .expect("traffic db"),
    );
    let body_store = Arc::new(RwLock::new(BodyStore::new(
        dir.path().join("body_cache"),
        0,
        7,
        64 * 1024,
        Duration::from_millis(100),
    )));
    let request_body_ref = body_store
        .read()
        .store("aggregate-budget", "req", &gzip(b"request1"))
        .unwrap()
        .with_content_encoding(Some("gzip"))
        .unwrap();
    let response_body_ref = body_store
        .read()
        .store("aggregate-budget", "res", &gzip(b"response-budget-needle"))
        .unwrap()
        .with_content_encoding(Some("gzip"))
        .unwrap();
    let mut record = TrafficRecord::new(
        "aggregate-budget".to_string(),
        "POST".to_string(),
        "https://example.com/aggregate".to_string(),
    );
    record.request_body_ref = Some(request_body_ref);
    record.response_body_ref = Some(response_body_ref);
    db.record(record);

    let engine = SearchEngine::new(db, Some(body_store)).with_decompression_budget(1024, 12);
    let request = SearchRequest {
        keyword: "response-budget-needle".to_string(),
        scope: SearchScope {
            all: false,
            request_body: true,
            response_body: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let response = engine.search(&request);
    assert_eq!(response.total_searched, 1);
    assert!(response.has_more);
    assert_eq!(
        response.partial_reason.as_deref(),
        Some("decompression_budget_exhausted")
    );

    let continuation = engine.search(&SearchRequest {
        cursor: response.next_cursor,
        ..request
    });
    assert!(!continuation.has_more);
    assert!(continuation.partial_reason.is_none());
}

#[test]
fn search_rolls_back_a_record_that_can_fit_the_next_fresh_budget() {
    fn setup(
        label: &str,
        consumer_url: &str,
        consumer_body: &[u8],
        target_url: &str,
        target_body: &[u8],
        budget: usize,
    ) -> (TempDir, SearchEngine) {
        fn gzip(bytes: &[u8]) -> Vec<u8> {
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(bytes).unwrap();
            encoder.finish().unwrap()
        }

        let dir = TempDir::new().expect("temp dir");
        let db = Arc::new(
            TrafficDbStore::new(dir.path().join("traffic"), 1024, 64 * 1024 * 1024, Some(24))
                .expect("traffic db"),
        );
        let body_store = Arc::new(RwLock::new(BodyStore::new(
            dir.path().join("body_cache"),
            0,
            7,
            64 * 1024,
            Duration::from_millis(100),
        )));
        for (suffix, url, body) in [
            ("target", target_url, target_body),
            ("consumer", consumer_url, consumer_body),
        ] {
            let id = format!("{label}-{suffix}");
            let body_ref = body_store
                .read()
                .store(&id, "res", &gzip(body))
                .unwrap()
                .with_content_encoding(Some("gzip"))
                .unwrap();
            let mut record = TrafficRecord::new(id, "GET".to_string(), url.to_string());
            record.response_body_ref = Some(body_ref);
            db.record(record);
        }
        (
            dir,
            SearchEngine::new(db, Some(body_store)).with_decompression_budget(1024, budget),
        )
    }

    let (_keyword_dir, keyword_engine) = setup(
        "keyword-rollback",
        "https://example.com/consumer",
        b"consumer",
        "https://example.com/target",
        b"target-needle",
        16,
    );
    let keyword_response = keyword_engine.search(&SearchRequest {
        keyword: "target-needle".to_string(),
        scope: SearchScope {
            all: false,
            response_body: true,
            ..Default::default()
        },
        ..Default::default()
    });
    assert_eq!(keyword_response.total_searched, 1);
    assert!(keyword_response.has_more);

    let (_condition_dir, condition_engine) = setup(
        "condition-rollback",
        "https://example.com/condition-consumer",
        br#"{"marker":"no"}"#,
        "https://example.com/condition-target",
        br#"{"marker":"yes"}"#,
        20,
    );
    let condition_response = condition_engine.search(&SearchRequest {
        keyword: "condition-target".to_string(),
        scope: SearchScope {
            all: false,
            url: true,
            ..Default::default()
        },
        filters: SearchFilters {
            conditions: vec![FilterCondition {
                field: "res.body.$.marker".to_string(),
                operator: "equals".to_string(),
                value: "yes".to_string(),
            }],
            ..Default::default()
        },
        ..Default::default()
    });
    assert_eq!(condition_response.total_searched, 1);
    assert!(condition_response.has_more);

    let (_hydration_dir, hydration_engine) = setup(
        "hydration-rollback",
        "https://example.com/hydration-consumer",
        b"consumer",
        "https://example.com/hydration-target",
        b"target response body",
        24,
    );
    let hydration_response = hydration_engine.search(&SearchRequest {
        keyword: "hydration".to_string(),
        scope: SearchScope {
            all: false,
            url: true,
            ..Default::default()
        },
        include: crate::search::SearchInclude {
            response_body: true,
            ..Default::default()
        },
        ..Default::default()
    });
    assert_eq!(hydration_response.total_searched, 1);
    assert_eq!(hydration_response.results.len(), 1);
    assert!(hydration_response.has_more);
}

#[test]
fn search_budget_exhaustion_rolls_back_filter_and_hydration_records() {
    let dir = TempDir::new().expect("temp dir");
    let db = Arc::new(
        TrafficDbStore::new(dir.path().join("traffic"), 1024, 64 * 1024 * 1024, Some(24))
            .expect("traffic db"),
    );
    let body_store = Arc::new(RwLock::new(BodyStore::new(
        dir.path().join("body_cache"),
        0,
        7,
        64 * 1024,
        Duration::from_millis(100),
    )));
    let plaintext = br#"{"marker":"filter-budget-needle"}"#;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(plaintext).unwrap();
    let compressed = encoder.finish().unwrap();
    let body_ref = body_store
        .read()
        .store("budget-rollback", "res", &compressed)
        .unwrap()
        .with_content_encoding(Some("gzip"))
        .unwrap();
    let mut record = TrafficRecord::new(
        "budget-rollback".to_string(),
        "GET".to_string(),
        "https://example.com/hydration-budget-needle".to_string(),
    );
    record.response_body_ref = Some(body_ref);
    db.record(record);
    let engine = SearchEngine::new(db, Some(body_store)).with_decompression_budget(1024, 0);

    let filter_response = engine.search(&SearchRequest {
        keyword: "hydration-budget-needle".to_string(),
        scope: SearchScope {
            all: false,
            url: true,
            ..Default::default()
        },
        filters: SearchFilters {
            conditions: vec![FilterCondition {
                field: "res.body.$.marker".to_string(),
                operator: "equals".to_string(),
                value: "filter-budget-needle".to_string(),
            }],
            ..Default::default()
        },
        ..Default::default()
    });
    assert_eq!(filter_response.total_searched, 1);
    assert!(filter_response.next_cursor.is_some());
    assert!(filter_response.has_more);
    assert_eq!(
        filter_response.partial_reason.as_deref(),
        Some("decompression_budget_exhausted")
    );

    let hydration_response = engine.search(&SearchRequest {
        keyword: "hydration-budget-needle".to_string(),
        scope: SearchScope {
            all: false,
            url: true,
            ..Default::default()
        },
        include: crate::search::SearchInclude {
            response_body: true,
            ..Default::default()
        },
        ..Default::default()
    });
    assert!(hydration_response.results.is_empty());
    assert_eq!(hydration_response.total_searched, 1);
    assert!(hydration_response.next_cursor.is_some());
    assert!(hydration_response.has_more);
    assert_eq!(
        hydration_response.partial_reason.as_deref(),
        Some("decompression_budget_exhausted")
    );
}

#[test]
fn file_body_json_filter_handles_non_json_and_missing_files() {
    let dir = TempDir::new().expect("temp dir");
    let db = Arc::new(
        TrafficDbStore::new(dir.path().join("traffic"), 1024, 64 * 1024 * 1024, Some(24))
            .expect("traffic db"),
    );
    let body_store = Arc::new(RwLock::new(BodyStore::new(
        dir.path().join("body_cache"),
        0,
        7,
        64 * 1024,
        Duration::from_millis(100),
    )));
    let mut non_json = TrafficRecord::new(
        "REQ-search-non-json".to_string(),
        "GET".to_string(),
        "https://example.com/non-json".to_string(),
    );
    non_json.response_body_ref =
        body_store
            .read()
            .store("REQ-search-non-json", "res", b"plain text is not JSON");
    db.record(non_json);
    let mut missing = TrafficRecord::new(
        "REQ-search-missing".to_string(),
        "GET".to_string(),
        "https://example.com/missing".to_string(),
    );
    missing.response_body_ref = Some(BodyRef::File {
        path: dir
            .path()
            .join("body_cache/missing")
            .to_string_lossy()
            .to_string(),
        size: 64,
    });
    db.record(missing);
    let engine = SearchEngine::new(db, Some(body_store));

    let response = engine.search(&SearchRequest {
        filters: SearchFilters {
            conditions: vec![FilterCondition {
                field: "res.body.$.ok".to_string(),
                operator: "equals".to_string(),
                value: "true".to_string(),
            }],
            ..Default::default()
        },
        limit: Some(20),
        ..Default::default()
    });

    assert_eq!(response.total_matched, 0);
}

fn make_db() -> Arc<TrafficDbStore> {
    let dir = TempDir::new().expect("temp dir");
    Arc::new(
        TrafficDbStore::new(dir.path().join("traffic"), 1024, 64 * 1024 * 1024, Some(24))
            .expect("traffic db"),
    )
    // NOTE: dir is dropped at end of test; data is in-memory or short-lived.
}

fn record_with_json_body(
    db: &Arc<TrafficDbStore>,
    id: &str,
    url: &str,
    req_body: Option<serde_json::Value>,
    res_body: Option<serde_json::Value>,
) {
    let mut rec = TrafficRecord::new(id.to_string(), "POST".to_string(), url.to_string());
    if let Some(b) = req_body {
        rec.request_body_ref = Some(BodyRef::Inline {
            data: b.to_string(),
        });
    }
    if let Some(b) = res_body {
        rec.response_body_ref = Some(BodyRef::Inline {
            data: b.to_string(),
        });
    }
    db.record(rec);
}

fn search_with(
    db: Arc<TrafficDbStore>,
    keyword: &str,
    scope: SearchScope,
    conditions: Vec<FilterCondition>,
    time_range: Option<TimeRange>,
) -> crate::search::SearchResponse {
    let engine = SearchEngine::new(db, None);
    let filters = SearchFilters {
        conditions,
        ..SearchFilters::default()
    };
    engine.search(&SearchRequest {
        keyword: keyword.to_string(),
        scope,
        filters,
        cursor: None,
        limit: Some(20),
        max_scan: None,
        max_results: None,
        time_range,
        ..Default::default()
    })
}

#[test]
fn json_path_req_body_filter_matches() {
    let db = make_db();
    record_with_json_body(
        &db,
        "REQ-jp-1",
        "https://api.example.com/v1/users",
        Some(serde_json::json!({"user":{"id":42,"name":"alice"}})),
        None,
    );
    record_with_json_body(
        &db,
        "REQ-jp-2",
        "https://api.example.com/v1/users",
        Some(serde_json::json!({"user":{"id":7,"name":"bob"}})),
        None,
    );
    let resp = search_with(
        db,
        "",
        SearchScope::default(),
        vec![FilterCondition {
            field: "req.body.$.user.name".to_string(),
            operator: "equals".to_string(),
            value: "alice".to_string(),
        }],
        None,
    );
    assert_eq!(resp.total_matched, 1);
    assert_eq!(resp.results[0].record.id, "REQ-jp-1");
}

#[test]
fn json_path_res_body_numeric_gt_filter() {
    let db = make_db();
    record_with_json_body(
        &db,
        "REQ-jp-3",
        "https://api.example.com/v1/foo",
        None,
        Some(serde_json::json!({"errno": 0, "data": {"score": 95}})),
    );
    record_with_json_body(
        &db,
        "REQ-jp-4",
        "https://api.example.com/v1/foo",
        None,
        Some(serde_json::json!({"errno": 0, "data": {"score": 50}})),
    );
    let resp = search_with(
        db,
        "",
        SearchScope::default(),
        vec![FilterCondition {
            field: "res.body.$.data.score".to_string(),
            operator: "gt".to_string(),
            value: "80".to_string(),
        }],
        None,
    );
    assert_eq!(resp.total_matched, 1);
    assert_eq!(resp.results[0].record.id, "REQ-jp-3");
}

#[test]
fn time_range_pre_filter_and_searched_range_population() {
    let db = make_db();
    for i in 0..5 {
        record_with_json_body(
            &db,
            &format!("REQ-ts-{}", i),
            "https://api.example.com/v1/x",
            None,
            Some(serde_json::json!({"i": i})),
        );
    }
    // Time range with a huge until value matches all (since=0).
    let resp = search_with(
        db,
        "",
        SearchScope::default(),
        vec![],
        Some(TimeRange {
            since_ms: Some(0),
            until_ms: Some(i64::MAX),
        }),
    );
    assert_eq!(resp.total_matched, 5);
    assert_eq!(resp.searched_range.scanned_count, 5);
    assert!(resp.searched_range.oldest_ts_ms.is_some());
    assert!(resp.searched_range.newest_ts_ms.is_some());
    assert!(resp.searched_range.oldest_ts_ms.unwrap() <= resp.searched_range.newest_ts_ms.unwrap());
}

#[test]
fn time_range_excludes_records_outside_window() {
    let db = make_db();
    for i in 0..3 {
        record_with_json_body(
            &db,
            &format!("REQ-ts2-{}", i),
            "https://api.example.com/v1/x",
            None,
            None,
        );
    }
    // until_ms = 1 forces all current records (with real ts in ms) to be excluded.
    let resp = search_with(
        db,
        "",
        SearchScope::default(),
        vec![],
        Some(TimeRange {
            since_ms: None,
            until_ms: Some(1),
        }),
    );
    assert_eq!(resp.total_matched, 0);
    assert_eq!(resp.searched_range.scanned_count, 0);
    assert!(resp.searched_range.oldest_ts_ms.is_none());
}

#[test]
fn header_condition_case_insensitive_contains() {
    let db = make_db();
    let mut rec = TrafficRecord::new(
        "REQ-hdr-1".to_string(),
        "GET".to_string(),
        "https://api.example.com/v1/x".to_string(),
    );
    rec.request_headers = Some(vec![("X-Trace-Id".to_string(), "abc-123".to_string())]);
    db.record(rec);
    let resp = search_with(
        db,
        "",
        SearchScope::default(),
        vec![FilterCondition {
            field: "req.header.x-trace-id".to_string(),
            operator: "contains".to_string(),
            value: "abc".to_string(),
        }],
        None,
    );
    assert_eq!(resp.total_matched, 1);
    assert_eq!(resp.results[0].record.id, "REQ-hdr-1");
}

#[test]
fn include_hydrates_response_body_and_headers() {
    use crate::search::SearchInclude;
    use base64::engine::general_purpose::STANDARD as BASE64;
    use base64::Engine as _;

    let db = make_db();
    let mut rec = TrafficRecord::new(
        "REQ-inc-1".to_string(),
        "GET".to_string(),
        "https://api.example.com/v1/items".to_string(),
    );
    rec.original_response_headers = Some(vec![(
        "Content-Type".to_string(),
        "application/json".to_string(),
    )]);
    rec.response_body_ref = Some(BodyRef::Inline {
        data: r#"{"name":"alpha","id":42}"#.to_string(),
    });
    db.record(rec);

    let engine = SearchEngine::new(db, None);
    let response = engine.search(&SearchRequest {
        keyword: "alpha".to_string(),
        scope: SearchScope {
            all: false,
            response_body: true,
            ..Default::default()
        },
        filters: SearchFilters::default(),
        cursor: None,
        limit: Some(20),
        max_scan: None,
        max_results: None,
        record_ids: Vec::new(),
        time_range: None,
        include: SearchInclude {
            response_body: true,
            response_headers: true,
            ..Default::default()
        },
    });

    assert_eq!(response.total_matched, 1);
    let item = &response.results[0];
    let bodies = item.bodies.as_ref().expect("bodies attached");
    let res_chunk = bodies.response.as_ref().expect("response chunk");
    let raw = BASE64.decode(&res_chunk.bytes_b64).expect("valid base64");
    assert!(String::from_utf8_lossy(&raw).contains("alpha"));
    assert!(!res_chunk.truncated);
    let headers = item.headers.as_ref().expect("headers attached");
    assert!(headers
        .response
        .iter()
        .any(|(k, v)| k == "Content-Type" && v.contains("application/json")));
    assert!(headers.request.is_empty());
}

#[test]
fn include_truncates_body_at_max_body_bytes() {
    use crate::search::SearchInclude;
    use base64::engine::general_purpose::STANDARD as BASE64;
    use base64::Engine as _;

    let db = make_db();
    let mut rec = TrafficRecord::new(
        "REQ-inc-trunc".to_string(),
        "GET".to_string(),
        "https://api.example.com/v1/big".to_string(),
    );
    // 10 KiB payload, keyword embedded near the start so search hits regardless.
    let mut payload = String::from("hit-needle-zzz ");
    payload.push_str(&"x".repeat(10 * 1024));
    rec.response_body_ref = Some(BodyRef::Inline { data: payload });
    db.record(rec);

    let engine = SearchEngine::new(db, None);
    let response = engine.search(&SearchRequest {
        keyword: "hit-needle-zzz".to_string(),
        scope: SearchScope {
            all: false,
            response_body: true,
            ..Default::default()
        },
        filters: SearchFilters::default(),
        cursor: None,
        limit: Some(20),
        max_scan: None,
        max_results: None,
        record_ids: Vec::new(),
        time_range: None,
        include: SearchInclude {
            response_body: true,
            max_body_bytes: Some(256),
            ..Default::default()
        },
    });

    assert_eq!(response.total_matched, 1);
    let bodies = response.results[0].bodies.as_ref().expect("bodies");
    let chunk = bodies.response.as_ref().expect("response chunk");
    assert!(chunk.truncated, "chunk should be truncated");
    let raw = BASE64.decode(&chunk.bytes_b64).expect("valid base64");
    assert_eq!(raw.len(), 256);
    assert!(chunk.size > 256);
}
