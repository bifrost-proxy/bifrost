use super::*;

#[test]
fn test_devtools_client_req_id_lookup_uses_first_non_replay_record() {
    let dir = create_test_dir();
    let store = TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap();

    let mut first = TrafficRecord::new(
        "traffic-first".to_string(),
        "GET".to_string(),
        "https://example.test/api".to_string(),
    );
    first.devtools_client_req_id = Some("client-req-1".to_string());
    store.record(first);

    let mut duplicate = TrafficRecord::new(
        "traffic-duplicate".to_string(),
        "GET".to_string(),
        "https://example.test/api".to_string(),
    );
    duplicate.devtools_client_req_id = Some("client-req-1".to_string());
    store.record(duplicate);

    let mut replay = TrafficRecord::new(
        "traffic-replay".to_string(),
        "GET".to_string(),
        "https://example.test/api".to_string(),
    );
    replay.is_replay = true;
    replay.devtools_client_req_id = Some("client-req-replay".to_string());
    store.record(replay);

    assert_eq!(
        store.get_id_by_devtools_client_req_id("client-req-1"),
        Some("traffic-first".to_string())
    );
    assert_eq!(
        store.get_id_by_devtools_client_req_id("client-req-replay"),
        None
    );

    cleanup_test_dir(&dir);
}

#[test]
fn test_query_for_search_skips_total_count() {
    let dir = create_test_dir();
    let store = TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap();

    store.record(TrafficRecord::new(
        "req-1".to_string(),
        "GET".to_string(),
        "https://a.com/p1".to_string(),
    ));
    store.record(TrafficRecord::new(
        "req-2".to_string(),
        "GET".to_string(),
        "https://a.com/p2".to_string(),
    ));
    store.record(TrafficRecord::new(
        "req-3".to_string(),
        "GET".to_string(),
        "https://a.com/p3".to_string(),
    ));

    let params = QueryParams {
        limit: Some(2),
        direction: Direction::Backward,
        ..Default::default()
    };

    let normal = store.query(&params);
    let fast = store.query_for_search(&params);

    assert_eq!(normal.records.len(), fast.records.len());
    assert_eq!(normal.has_more, fast.has_more);
    assert!(normal.total >= 3);
    assert_eq!(fast.total, 0);

    cleanup_test_dir(&dir);
}

#[test]
fn test_query_latest_window_returns_latest_records_in_ascending_order() {
    let dir = create_test_dir();
    let store = TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap();

    for i in 0..5 {
        store.record(TrafficRecord::new(
            format!("req-{}", i),
            "GET".to_string(),
            format!("https://a.com/{}", i),
        ));
    }

    let result = store.query_latest_window(2);
    let sequences: Vec<u64> = result.records.iter().map(|r| r.seq).collect();
    let ids: Vec<&str> = result.records.iter().map(|r| r.id.as_str()).collect();

    assert_eq!(sequences.len(), 2);
    assert!(sequences[0] < sequences[1]);
    assert_eq!(ids, vec!["req-3", "req-4"]);
    assert_eq!(result.prev_cursor, Some(sequences[0]));
    assert_eq!(result.next_cursor, Some(sequences[1]));

    cleanup_test_dir(&dir);
}

#[test]
fn test_get_search_fields_by_ids_respects_field_flags() {
    let dir = create_test_dir();
    let store = TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap();

    let mut record = TrafficRecord::new(
        "req-1".to_string(),
        "POST".to_string(),
        "https://a.com/p1".to_string(),
    );
    record.request_headers = Some(vec![("X-Test".to_string(), "1".to_string())]);
    record.original_response_headers = Some(vec![("Y-Test".to_string(), "2".to_string())]);
    record.request_body_ref = Some(BodyRef::Inline {
        data: "hello".to_string(),
    });
    record.response_body_ref = Some(BodyRef::Inline {
        data: "world".to_string(),
    });
    record.set_request_body_content_encoding(Some("gzip"));
    record.set_response_body_content_encoding(Some("br"));
    store.record(record);

    let ids = ["req-1" as &str];

    let m = store.get_search_fields_by_ids(&ids, true, true, true, true, true);
    let f = m.get("req-1").expect("missing fields");
    assert!(f.url.as_deref().unwrap_or("").contains("https://a.com/p1"));
    assert!(f
        .request_headers
        .as_ref()
        .is_some_and(|h| h.iter().any(|(k, v)| k == "X-Test" && v == "1")));
    assert!(f
        .response_headers
        .as_ref()
        .is_some_and(|h| h.iter().any(|(k, v)| k == "Y-Test" && v == "2")));
    assert!(matches!(f.request_body_ref, Some(BodyRef::Inline { .. })));
    assert!(matches!(f.response_body_ref, Some(BodyRef::Inline { .. })));
    assert_eq!(f.request_body_content_encoding.as_deref(), Some("gzip"));
    assert_eq!(f.response_body_content_encoding.as_deref(), Some("br"));

    let m2 = store.get_search_fields_by_ids(&ids, false, false, false, false, false);
    let f2 = m2.get("req-1").expect("missing fields");
    assert!(f2.url.is_none());
    assert!(f2.request_headers.is_none());
    assert!(f2.response_headers.is_none());
    assert!(f2.request_body_ref.is_none());
    assert!(f2.response_body_ref.is_none());
    assert!(f2.request_body_content_encoding.is_none());
    assert!(f2.response_body_content_encoding.is_none());

    cleanup_test_dir(&dir);
}

#[test]
fn test_get_by_id_loads_detail_fields_from_split_table() {
    let dir = create_test_dir();
    let store = TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap();

    let mut record = TrafficRecord::new(
        "detail-1".to_string(),
        "POST".to_string(),
        "https://a.com/p1".to_string(),
    );
    record.actual_url = Some("https://upstream.example.com/p1".to_string());
    record.actual_host = Some("upstream.example.com".to_string());
    record.request_headers = Some(vec![(
        "Content-Type".to_string(),
        "application/json".to_string(),
    )]);
    record.original_response_headers = Some(vec![("X-Trace".to_string(), "trace-1".to_string())]);
    record.request_body_ref = Some(BodyRef::Inline {
        data: "{\"hello\":1}".to_string(),
    });
    record.response_body_ref = Some(BodyRef::File {
        path: dir
            .join("encoded-response-body")
            .to_string_lossy()
            .to_string(),
        size: 11,
    });
    record.set_request_body_content_encoding(Some("br"));
    record.set_response_body_content_encoding(Some("gzip"));
    record.mark_response_body_observation_partial();
    record.socket_status = Some(crate::traffic::SocketStatus {
        is_open: false,
        send_count: 3,
        receive_count: 5,
        send_bytes: 120,
        receive_bytes: 240,
        frame_count: 8,
        close_code: Some(1000),
        close_reason: Some("Normal closure".to_string()),
    });
    record.error_message = Some("upstream timeout".to_string());
    store.record(record);

    let loaded = store.get_by_id("detail-1").expect("record should exist");
    assert_eq!(loaded.actual_host.as_deref(), Some("upstream.example.com"));
    assert!(loaded.request_headers.as_ref().is_some_and(|h| h
        .iter()
        .any(|(k, v)| k == "Content-Type" && v == "application/json")));
    assert!(matches!(
        loaded.request_body_ref,
        Some(BodyRef::Inline { .. })
    ));
    assert_eq!(
        loaded.response_body_content_encoding().as_deref(),
        Some("gzip")
    );
    assert_eq!(
        loaded.request_body_content_encoding().as_deref(),
        Some("br")
    );
    assert!(loaded
        .body_metadata
        .as_ref()
        .and_then(|metadata| metadata.response.as_ref())
        .is_some_and(|metadata| metadata.observation_partial));
    assert_eq!(loaded.error_message.as_deref(), Some("upstream timeout"));
    assert_eq!(
        loaded.socket_status.as_ref().and_then(|s| s.close_code),
        Some(1000)
    );
    assert_eq!(
        loaded
            .socket_status
            .as_ref()
            .and_then(|s| s.close_reason.as_deref()),
        Some("Normal closure")
    );

    cleanup_test_dir(&dir);
}

#[test]
fn test_get_by_ids_keeps_request_content_type_and_rule_summary() {
    let dir = create_test_dir();
    let store = TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap();

    let mut record = TrafficRecord::new(
        "summary-1".to_string(),
        "POST".to_string(),
        "https://a.com/p1".to_string(),
    );
    record.request_content_type = Some("application/json".to_string());
    record.has_rule_hit = true;
    record.matched_rules = Some(vec![
        crate::traffic::MatchedRule {
            pattern: "/p1".to_string(),
            protocol: "reqHeaders".to_string(),
            value: "X-Test: 1".to_string(),
            rule_name: Some("rule-1".to_string()),
            raw: None,
            line: Some(1),
        },
        crate::traffic::MatchedRule {
            pattern: "/p1".to_string(),
            protocol: "resBody".to_string(),
            value: "ok".to_string(),
            rule_name: Some("rule-2".to_string()),
            raw: None,
            line: Some(2),
        },
    ]);
    store.record(record);

    let rows = store.get_by_ids(&["summary-1"]);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].req_ct.as_deref(), Some("application/json"));
    assert_eq!(rows[0].rc, 2);
    assert!(rows[0].rp.iter().any(|p| p == "reqHeaders"));
    assert!(rows[0].rp.iter().any(|p| p == "resBody"));

    cleanup_test_dir(&dir);
}
