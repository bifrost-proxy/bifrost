use super::*;

#[test]
fn test_in_memory_statistics_follow_updates_deletes_and_restart() {
    let dir = create_test_dir();
    {
        let store = TrafficDbStore::new(dir.clone(), 1_000, 0, None).unwrap();
        let mut first = TrafficRecord::new(
            "stats-1".to_string(),
            "GET".to_string(),
            "https://one.test/a".to_string(),
        );
        first.client_ip = "127.0.0.1".to_string();
        first.listener_port = 9915;
        first.client_app = Some("Pending App".to_string());
        store.record(first);

        let mut second = TrafficRecord::new(
            "stats-2".to_string(),
            "GET".to_string(),
            "https://two.test/b".to_string(),
        );
        second.client_ip = "10.0.0.2".to_string();
        second.listener_port = 8800;
        second.client_app = Some("Codex".to_string());
        store.record(second);

        assert!(store.update_by_id("stats-1", |record| {
            record.client_app = Some("Codex".to_string());
            record.account_name = Some("eden".to_string());
        }));

        let snapshot = store.traffic_statistics();
        assert_eq!(snapshot.total_requests, 2);
        assert_eq!(snapshot.applications.get("Codex"), Some(&2));
        assert!(!snapshot.applications.contains_key("Pending App"));
        assert_eq!(snapshot.client_ips.get("127.0.0.1"), Some(&1));
        assert_eq!(snapshot.proxy_ports.get("9915"), Some(&1));
        assert_eq!(snapshot.account_names.get("eden"), Some(&1));
        assert_eq!(snapshot.domains.get("one.test"), Some(&1));

        store
            .delete_by_ids(&["stats-2".to_string(), "stats-2".to_string()])
            .unwrap();
        let snapshot = store.traffic_statistics();
        assert_eq!(snapshot.total_requests, 1);
        assert_eq!(snapshot.applications.get("Codex"), Some(&1));
        assert!(!snapshot.client_ips.contains_key("10.0.0.2"));
        store.checkpoint();
    }

    let reopened = TrafficDbStore::new(dir.clone(), 1_000, 0, None).unwrap();
    let snapshot = reopened.traffic_statistics();
    assert_eq!(snapshot.total_requests, 1);
    assert_eq!(snapshot.applications.get("Codex"), Some(&1));
    assert_eq!(snapshot.account_names.get("eden"), Some(&1));

    reopened.clear().unwrap();
    let snapshot = reopened.traffic_statistics();
    assert_eq!(snapshot.total_requests, 0);
    assert!(snapshot.applications.is_empty());
    assert!(snapshot.client_ips.is_empty());
    assert!(snapshot.domains.is_empty());

    cleanup_test_dir(&dir);
}

#[test]
fn test_statistics_stay_unchanged_when_single_insert_fails() {
    let dir = create_test_dir();
    let store = TrafficDbStore::new(dir.clone(), 1_000, 0, None).unwrap();
    store
        .write_conn
        .lock()
        .execute_batch("DROP TABLE traffic_records")
        .unwrap();

    let record = TrafficRecord::new(
        "stats-failed-insert".to_string(),
        "GET".to_string(),
        "https://failed.test/single".to_string(),
    );
    store.record(record);

    assert_eq!(store.count(), 0);
    assert_eq!(store.traffic_statistics().total_requests, 0);
    cleanup_test_dir(&dir);
}

#[test]
fn test_statistics_stay_unchanged_when_batch_insert_fails() {
    let dir = create_test_dir();
    let store = TrafficDbStore::new(dir.clone(), 1_000, 0, None).unwrap();
    store
        .write_conn
        .lock()
        .execute_batch("DROP TABLE traffic_records")
        .unwrap();

    let record = TrafficRecord::new(
        "stats-failed-batch".to_string(),
        "GET".to_string(),
        "https://failed.test/batch".to_string(),
    );
    store.record_batch(vec![record]);

    assert_eq!(store.count(), 0);
    assert_eq!(store.traffic_statistics().total_requests, 0);
    cleanup_test_dir(&dir);
}

#[test]
fn test_statistics_stay_unchanged_when_record_disappears_before_update() {
    let dir = create_test_dir();
    let store = TrafficDbStore::new(dir.clone(), 1_000, 0, None).unwrap();
    let record = TrafficRecord::new(
        "stats-disappearing-update".to_string(),
        "GET".to_string(),
        "https://failed.test/update".to_string(),
    );
    store.record(record);

    assert!(!store.update_by_id("stats-disappearing-update", |_| {
        store
            .write_conn
            .lock()
            .execute(
                "DELETE FROM traffic_records WHERE id = ?1",
                ["stats-disappearing-update"],
            )
            .unwrap();
    }));

    cleanup_test_dir(&dir);
}

#[test]
fn test_duplicate_record_id_replaces_statistics_without_incrementing_total() {
    let dir = create_test_dir();
    let store = TrafficDbStore::new(dir.clone(), 1_000, 0, None).unwrap();
    let mut first = TrafficRecord::new(
        "stats-duplicate".to_string(),
        "GET".to_string(),
        "https://one.test/a".to_string(),
    );
    first.client_app = Some("First App".to_string());
    store.record(first);

    let mut replacement = TrafficRecord::new(
        "stats-duplicate".to_string(),
        "GET".to_string(),
        "https://two.test/b".to_string(),
    );
    replacement.client_app = Some("Second App".to_string());
    store.record(replacement);

    let snapshot = store.traffic_statistics();
    assert_eq!(snapshot.total_requests, 1);
    assert_eq!(store.count(), 1);
    assert!(!snapshot.applications.contains_key("First App"));
    assert_eq!(snapshot.applications.get("Second App"), Some(&1));
    assert!(!snapshot.domains.contains_key("one.test"));
    assert_eq!(snapshot.domains.get("two.test"), Some(&1));

    cleanup_test_dir(&dir);
}

#[test]
fn test_metrics_aggregates_use_trusted_upload_download_bytes() {
    let dir = create_test_dir();
    let store = TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap();

    let mut record = TrafficRecord::new(
        "metric-1".to_string(),
        "POST".to_string(),
        "http://metrics.example.test/echo".to_string(),
    );
    record.client_app = Some("Metric Test App".to_string());
    record.request_size = 9_999;
    record.response_size = 8_888;
    record.upload_bytes = 37;
    record.download_bytes = 73;
    store.record(record);

    let host = store
        .aggregate_host_metrics()
        .into_iter()
        .find(|m| m.host == "metrics.example.test")
        .expect("host aggregate should exist");
    assert_eq!(host.requests, 1);
    assert_eq!(host.bytes_sent, 37);
    assert_eq!(host.bytes_received, 73);

    let app = store
        .aggregate_app_metrics()
        .into_iter()
        .find(|m| m.app_name == "Metric Test App")
        .expect("app aggregate should exist");
    assert_eq!(app.requests, 1);
    assert_eq!(app.bytes_sent, 37);
    assert_eq!(app.bytes_received, 73);

    let summary = store
        .get_by_ids(&["metric-1"])
        .into_iter()
        .next()
        .expect("summary should exist");
    assert_eq!(summary.req_sz, 9_999);
    assert_eq!(summary.res_sz, 8_888);
    assert_eq!(summary.up, 37);
    assert_eq!(summary.down, 73);

    assert!(store.update_by_id("metric-1", |record| {
        record.client_app = Some("Resolved Metric App".to_string());
        record.upload_bytes = 41;
        record.download_bytes = 79;
    }));
    assert!(store
        .aggregate_app_metrics()
        .iter()
        .all(|metrics| metrics.app_name != "Metric Test App"));
    let resolved = store
        .aggregate_app_metrics()
        .into_iter()
        .find(|metrics| metrics.app_name == "Resolved Metric App")
        .expect("resolved app aggregate should exist");
    assert_eq!(resolved.bytes_sent, 41);
    assert_eq!(resolved.bytes_received, 79);

    drop(store);
    let store = TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap();
    let reloaded = store
        .aggregate_app_metrics()
        .into_iter()
        .find(|metrics| metrics.app_name == "Resolved Metric App")
        .expect("app aggregate should initialize from persisted records");
    assert_eq!(reloaded.bytes_sent, 41);
    assert_eq!(reloaded.bytes_received, 79);

    store.delete_by_ids(&["metric-1".to_string()]).unwrap();
    assert!(store.aggregate_app_metrics().is_empty());
    assert!(store.aggregate_host_metrics().is_empty());

    cleanup_test_dir(&dir);
}
