use super::*;

#[test]
fn test_clear_removes_pending_records_when_no_active_connections() {
    let dir = create_test_dir();
    let store = TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap();

    let record = TrafficRecord::new(
        "req-1".to_string(),
        "GET".to_string(),
        "https://a.com".to_string(),
    );
    store.record(record);
    assert_eq!(store.count(), 1);
    assert_eq!(store.oldest_sequence(), Some(1));

    store.clear_with_active_ids(&[]).unwrap();
    assert_eq!(store.count(), 0);
    assert_eq!(store.oldest_sequence(), None);

    cleanup_test_dir(&dir);
}

#[test]
fn test_cleanup_drops_to_target_after_trigger() {
    let dir = create_test_dir();
    let max_records = MIN_TRAFFIC_MAX_RECORDS;
    let store = TrafficDbStore::new(dir.clone(), max_records, 0, None).unwrap();

    let trigger = TrafficDbStore::cleanup_trigger_threshold(max_records);
    let total = trigger + 1;
    for i in 0..total {
        store.record(TrafficRecord::new(
            format!("req-{}", i),
            "GET".to_string(),
            format!("https://a.com/{}", i),
        ));
    }

    assert_eq!(store.count(), total);

    store.maybe_cleanup();

    let expected_count = TrafficDbStore::cleanup_target_count(max_records);
    assert_eq!(
        store.count(),
        expected_count,
        "after cleanup, count should be {}% of max ({}), got {}",
        CLEANUP_TARGET_PERCENT,
        expected_count,
        store.count()
    );
    assert_eq!(
        store.oldest_sequence(),
        Some((total - expected_count + 1) as u64),
    );
    assert_eq!(
        store.traffic_statistics().total_requests,
        expected_count as u64,
    );

    cleanup_test_dir(&dir);
}

#[test]
fn test_set_max_records_clamps_to_supported_range() {
    let dir = create_test_dir();
    let store = TrafficDbStore::new(dir.clone(), 10, 0, None).unwrap();

    store.set_max_records(10);
    assert_eq!(store.stats().max_records, MIN_TRAFFIC_MAX_RECORDS);

    store.set_max_records(MAX_TRAFFIC_MAX_RECORDS + 1);
    assert_eq!(store.stats().max_records, MAX_TRAFFIC_MAX_RECORDS);

    cleanup_test_dir(&dir);
}

#[test]
fn test_clear_preserves_active_connection_records() {
    let dir = create_test_dir();
    let store = TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap();

    let active = TrafficRecord::new(
        "active-1".to_string(),
        "GET".to_string(),
        "https://a.com".to_string(),
    );
    let inactive = TrafficRecord::new(
        "inactive-1".to_string(),
        "GET".to_string(),
        "https://b.com".to_string(),
    );
    store.record(active);
    store.record(inactive);
    assert_eq!(store.count(), 2);

    let active_ids = vec!["active-1".to_string()];
    store.clear_with_active_ids(&active_ids).unwrap();
    assert_eq!(store.count(), 1);
    assert!(store.get_by_id("active-1").is_some());

    cleanup_test_dir(&dir);
}

#[test]
fn test_no_cleanup_below_trigger_threshold() {
    let dir = create_test_dir();
    let max_records = MIN_TRAFFIC_MAX_RECORDS;
    let store = TrafficDbStore::new(dir.clone(), max_records, 0, None).unwrap();

    let trigger = TrafficDbStore::cleanup_trigger_threshold(max_records);
    for i in 0..trigger {
        store.record(TrafficRecord::new(
            format!("req-{}", i),
            "GET".to_string(),
            format!("https://a.com/{}", i),
        ));
    }

    assert_eq!(store.count(), trigger);

    store.maybe_cleanup();

    assert_eq!(
        store.count(),
        trigger,
        "no cleanup should happen below trigger threshold"
    );

    cleanup_test_dir(&dir);
}

#[test]
fn test_maybe_cleanup_record_count_only_deletes_to_target() {
    let dir = create_test_dir();
    let max_records = MIN_TRAFFIC_MAX_RECORDS;
    let store = TrafficDbStore::new(dir.clone(), max_records, 0, None).unwrap();

    let trigger = TrafficDbStore::cleanup_trigger_threshold(max_records);
    let target = TrafficDbStore::cleanup_target_count(max_records);
    let total = trigger + 1;
    for i in 0..total {
        store.record(TrafficRecord::new(
            format!("req-{}", i),
            "GET".to_string(),
            format!("https://a.com/{}", i),
        ));
    }

    store.maybe_cleanup();

    assert_eq!(
        store.count(),
        target,
        "record count cleanup should drop to target ({}% = {}), got {}",
        CLEANUP_TARGET_PERCENT,
        target,
        store.count()
    );

    cleanup_test_dir(&dir);
}

#[test]
fn test_maybe_cleanup_size_after_record_cleanup_skips_size_cleanup() {
    let dir = create_test_dir();
    let max_records = MIN_TRAFFIC_MAX_RECORDS;
    let store = TrafficDbStore::new(dir.clone(), max_records, 0, None).unwrap();

    let trigger = TrafficDbStore::cleanup_trigger_threshold(max_records);
    let target = TrafficDbStore::cleanup_target_count(max_records);
    let total = trigger + 1;
    for i in 0..total {
        store.record(TrafficRecord::new(
            format!("req-{}", i),
            "GET".to_string(),
            format!("https://a.com/{}", i),
        ));
    }

    store.max_db_size_bytes.store(1024, Ordering::SeqCst);

    store.maybe_cleanup();

    let remaining = store.count();
    assert_eq!(
        remaining, target,
        "when record cleanup triggers, size cleanup should be skipped. expected target={}, got {}",
        target, remaining,
    );

    cleanup_test_dir(&dir);
}

#[test]
fn test_maybe_cleanup_size_only_respects_max_delete_cap() {
    let dir = create_test_dir();
    let max_records = MIN_TRAFFIC_MAX_RECORDS;
    let store = TrafficDbStore::new(dir.clone(), max_records, 1, None).unwrap();

    for i in 0..max_records {
        store.record(TrafficRecord::new(
            format!("req-{}", i),
            "GET".to_string(),
            format!("https://a.com/{}", i),
        ));
    }

    let before = store.count();

    store.maybe_cleanup();

    let after = store.count();
    let max_delete = before * TrafficDbStore::SIZE_CLEANUP_MAX_DELETE_PERCENT / 100;
    let deleted = before.saturating_sub(after);
    assert!(
        deleted <= max_delete + 1,
        "size cleanup should not delete more than {}% of records: deleted {} out of {} (max allowed {})",
        TrafficDbStore::SIZE_CLEANUP_MAX_DELETE_PERCENT,
        deleted,
        before,
        max_delete,
    );
    assert!(after > 0, "size cleanup should never delete all records");

    cleanup_test_dir(&dir);
}

#[test]
fn test_record_batch_cleanup_preserves_reasonable_count() {
    let dir = create_test_dir();
    let max_records = MIN_TRAFFIC_MAX_RECORDS;
    let store = TrafficDbStore::new(dir.clone(), max_records, 0, None).unwrap();

    let trigger = TrafficDbStore::cleanup_trigger_threshold(max_records);
    let total_batches = (trigger + 500) / 64 + 1;
    for batch_idx in 0..total_batches {
        let batch: Vec<TrafficRecord> = (0..64)
            .map(|i| {
                let id = batch_idx * 64 + i;
                TrafficRecord::new(
                    format!("req-{}", id),
                    "GET".to_string(),
                    format!("https://a.com/{}", id),
                )
            })
            .collect();
        store.record_batch(batch);
    }

    let target = TrafficDbStore::cleanup_target_count(max_records);
    let count = store.count();
    assert!(
        count <= trigger + 128,
        "after batch inserts, count should not significantly exceed trigger ({}), got {}",
        trigger,
        count,
    );
    assert!(
        count >= target / 2,
        "after batch inserts with cleanup, count ({}) should not be below half of target ({})",
        count,
        target,
    );

    cleanup_test_dir(&dir);
}
