use super::*;

#[test]
fn test_database_epoch_survives_reopen_and_changes_for_rebuilt_database() {
    let dir = tempfile::tempdir().unwrap();
    let store = TrafficDbStore::new(dir.path().to_path_buf(), 100, 0, None).unwrap();
    let epoch = store.database_epoch().to_string();
    assert!(uuid::Uuid::parse_str(&epoch).is_ok());
    assert_eq!(
        store.traffic_statistics().database_epoch.as_deref(),
        Some(epoch.as_str())
    );
    store.record(epoch_test_record("preserved"));
    drop(store);

    let reopened = TrafficDbStore::new(dir.path().to_path_buf(), 100, 0, None).unwrap();
    assert_eq!(reopened.database_epoch(), epoch);
    assert_eq!(reopened.get_by_id("preserved").unwrap().sequence, 1);
    assert_eq!(reopened.current_sequence(), 2);
    drop(reopened);

    let other_dir = tempfile::tempdir().unwrap();
    let other = TrafficDbStore::new(other_dir.path().to_path_buf(), 100, 0, None).unwrap();
    assert_ne!(other.database_epoch(), epoch);
    fs::remove_file(dir.path().join("traffic.db")).unwrap();
    let rebuilt = TrafficDbStore::new(dir.path().to_path_buf(), 100, 0, None).unwrap();
    assert_ne!(rebuilt.database_epoch(), epoch);
    assert_eq!(rebuilt.current_sequence(), 1);
}

#[test]
fn test_database_epoch_backfills_existing_database_without_losing_rows() {
    let dir = tempfile::tempdir().unwrap();
    let store = TrafficDbStore::new(dir.path().to_path_buf(), 100, 0, None).unwrap();
    store.record(epoch_test_record("legacy-preserved"));
    store
        .write_conn
        .lock()
        .execute("DELETE FROM metadata WHERE key != 'schema_version'", [])
        .unwrap();
    drop(store);

    let store = TrafficDbStore::new(dir.path().to_path_buf(), 100, 0, None).unwrap();
    let epoch = store.database_epoch().to_string();
    assert!(!epoch.is_empty());
    assert_eq!(store.get_by_id("legacy-preserved").unwrap().sequence, 1);
    store.record(epoch_test_record("new-record"));
    assert_eq!(store.get_by_id("new-record").unwrap().sequence, 2);
    drop(store);
    let reopened = TrafficDbStore::new(dir.path().to_path_buf(), 100, 0, None).unwrap();
    assert_eq!(reopened.database_epoch(), epoch);
    assert_eq!(reopened.traffic_statistics().total_requests, 2);
}

#[test]
fn test_database_epoch_metadata_error_does_not_reset_database() {
    let dir = tempfile::tempdir().unwrap();
    let store = TrafficDbStore::new(dir.path().to_path_buf(), 100, 0, None).unwrap();
    store.record(epoch_test_record("must-survive"));
    store
        .write_conn
        .lock()
        .execute_batch(
            "DELETE FROM metadata WHERE key = 'database_epoch';
         CREATE TRIGGER reject_epoch BEFORE INSERT ON metadata
         WHEN NEW.key = 'database_epoch'
         BEGIN SELECT RAISE(ABORT, 'metadata unavailable'); END;",
        )
        .unwrap();
    drop(store);

    assert!(TrafficDbStore::new(dir.path().to_path_buf(), 100, 0, None).is_err());
    let conn = Connection::open(dir.path().join("traffic.db")).unwrap();
    assert_eq!(
        conn.query_row("SELECT id FROM traffic_records", [], |row| row
            .get::<_, String>(0))
            .unwrap(),
        "must-survive"
    );
    conn.execute_batch("DROP TRIGGER reject_epoch").unwrap();
    let reopened = TrafficDbStore::new(dir.path().to_path_buf(), 100, 0, None).unwrap();
    assert!(reopened.get_by_id("must-survive").is_some());
}

#[test]
fn test_database_epoch_rejects_invalid_metadata_without_reset() {
    for (key, value) in [
        ("database_epoch", ""),
        ("sequence_high_water", "invalid"),
        ("sequence_high_water", "18446744073709551615"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let store = TrafficDbStore::new(dir.path().to_path_buf(), 100, 0, None).unwrap();
        store.record(epoch_test_record("must-survive"));
        store
            .write_conn
            .lock()
            .execute(
                "INSERT OR REPLACE INTO metadata (key, value) VALUES (?1, ?2)",
                [key, value],
            )
            .unwrap();
        drop(store);
        assert!(TrafficDbStore::new(dir.path().to_path_buf(), 100, 0, None).is_err());
        let conn = Connection::open(dir.path().join("traffic.db")).unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM traffic_records", [], |row| row
                .get::<_, usize>(0))
                .unwrap(),
            1
        );
    }
}

#[test]
fn test_database_epoch_concurrent_metadata_initialization_uses_one_identity() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("traffic.db");
    let mut conn = Connection::open(&db_path).unwrap();
    init_database(&mut conn).unwrap();
    drop(conn);
    let barrier = Arc::new(std::sync::Barrier::new(4));
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let db_path = db_path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut conn = Connection::open(db_path).unwrap();
                barrier.wait();
                TrafficDbStore::initialize_sequence_metadata(&mut conn).unwrap()
            })
        })
        .collect();
    let identities: Vec<_> = handles
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    assert!(identities.iter().all(|identity| identity == &identities[0]));
}

#[test]
fn test_database_epoch_clear_and_tail_deletion_preserve_sequence_on_restart() {
    for mode in ["tail", "clear", "active", "expired", "oldest"] {
        let dir = tempfile::tempdir().unwrap();
        let store = TrafficDbStore::new(dir.path().to_path_buf(), 100, 0, None).unwrap();
        store.record_batch(
            (0..3)
                .map(|i| epoch_test_record(&format!("record-{i}")))
                .collect(),
        );
        let epoch = store.database_epoch().to_string();
        let next_sequence = store.current_sequence();
        match mode {
            "tail" => {
                store.delete_by_ids(&["record-2".to_string()]).unwrap();
            }
            "clear" => store.clear().unwrap(),
            "active" => store
                .clear_with_active_ids(&["record-0".to_string()])
                .unwrap(),
            "expired" => assert_eq!(
                store.delete_expired_by_cutoff(&store.write_conn.lock(), i64::MAX as u64),
                3
            ),
            "oldest" => {
                assert_eq!(store.delete_oldest_by_limit(&store.write_conn.lock(), 3), 3)
            }
            _ => unreachable!(),
        }
        drop(store);
        let store = TrafficDbStore::new(dir.path().to_path_buf(), 100, 0, None).unwrap();
        assert_eq!(store.database_epoch(), epoch, "{mode}");
        assert_eq!(store.current_sequence(), next_sequence, "{mode}");
        store.record(epoch_test_record("after-restart"));
        assert_eq!(
            store.get_by_id("after-restart").unwrap().sequence,
            next_sequence,
            "{mode}"
        );
        store.clear().unwrap();
        drop(store);
        let reopened = TrafficDbStore::new(dir.path().to_path_buf(), 100, 0, None).unwrap();
        assert_eq!(reopened.database_epoch(), epoch, "{mode}");
        assert_eq!(reopened.current_sequence(), next_sequence + 1, "{mode}");
    }
}

#[test]
fn test_database_epoch_failed_insert_does_not_change_identity_on_restart() {
    let dir = tempfile::tempdir().unwrap();
    let store = TrafficDbStore::new(dir.path().to_path_buf(), 100, 0, None).unwrap();
    store.record(epoch_test_record("preserved"));
    let epoch = store.database_epoch().to_string();
    store.set_write_query_only(true).unwrap();
    assert!(store
        .try_record_batch(vec![epoch_test_record("failed")])
        .is_err());
    assert_eq!(store.current_sequence(), 3);
    drop(store);
    let reopened = TrafficDbStore::new(dir.path().to_path_buf(), 100, 0, None).unwrap();
    assert_eq!(reopened.database_epoch(), epoch);
    assert_eq!(reopened.current_sequence(), 2);
    assert!(reopened.get_by_id("failed").is_none());
    assert_eq!(reopened.get_by_id("preserved").unwrap().sequence, 1);
}

#[test]
fn test_delete_by_ids_rolls_back_all_chunks_before_updating_memory() {
    let dir = tempfile::tempdir().unwrap();
    let store = TrafficDbStore::new(dir.path().to_path_buf(), 2_000, 0, None).unwrap();
    let ids: Vec<String> = (0..501).map(|index| format!("chunk-{index}")).collect();
    store.record_batch(ids.iter().map(|id| epoch_test_record(id)).collect());
    let before = store.traffic_statistics();
    let cached: Vec<String> = store
        .recent_cache
        .read()
        .iter()
        .map(|(id, _)| id.clone())
        .collect();
    store
        .write_conn
        .lock()
        .execute_batch(
            "CREATE TRIGGER reject_last_chunk BEFORE DELETE ON traffic_records
         WHEN OLD.id = 'chunk-500'
         BEGIN SELECT RAISE(ABORT, 'delete unavailable'); END;",
        )
        .unwrap();

    assert!(store.delete_by_ids(&ids).is_err());
    assert_eq!(store.count(), 501);
    assert_eq!(store.all_ids_set().len(), 501);
    assert_eq!(store.traffic_statistics(), before);
    assert_eq!(
        store
            .recent_cache
            .read()
            .iter()
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>(),
        cached
    );
    store
        .write_conn
        .lock()
        .execute_batch("DROP TRIGGER reject_last_chunk")
        .unwrap();
    assert_eq!(store.delete_by_ids(&ids).unwrap(), 501);
    assert_eq!(store.count(), 0);
    assert_eq!(store.traffic_statistics().total_requests, 0);
    assert_eq!(store.recent_cache_stats().len, 0);
    assert_eq!(store.delete_by_ids(&[]).unwrap(), 0);
    assert_eq!(store.delete_by_ids(&ids).unwrap(), 0);
}

#[test]
fn test_clear_sql_failure_preserves_rows_statistics_and_cache() {
    let dir = tempfile::tempdir().unwrap();
    let store = TrafficDbStore::new(dir.path().to_path_buf(), 100, 0, None).unwrap();
    store.record(epoch_test_record("must-survive"));
    store.record(epoch_test_record("fails-later"));
    let before = store.traffic_statistics();
    store
        .write_conn
        .lock()
        .execute_batch(
            "CREATE TRIGGER reject_clear BEFORE DELETE ON traffic_records
         WHEN OLD.id = 'fails-later'
         BEGIN SELECT RAISE(FAIL, 'delete unavailable'); END;",
        )
        .unwrap();
    assert!(store.clear().is_err());
    assert!(store.clear_with_active_ids(&["other".to_string()]).is_err());
    assert_eq!(store.count(), 2);
    assert_eq!(store.traffic_statistics(), before);
    assert_eq!(store.recent_cache_stats().len, 2);
    assert!(store.get_by_id("must-survive").is_some());
}

#[test]
fn test_database_epoch_high_water_failure_preserves_records_and_stops_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let store = TrafficDbStore::new(dir.path().to_path_buf(), 100, 0, None).unwrap();
    store.record(epoch_test_record("must-survive"));
    let epoch = store.database_epoch().to_string();
    store
        .write_conn
        .lock()
        .execute_batch(
            "CREATE TRIGGER reject_high_water BEFORE INSERT ON metadata
         WHEN NEW.key = 'sequence_high_water'
         BEGIN SELECT RAISE(ABORT, 'metadata unavailable'); END;",
        )
        .unwrap();
    assert!(store.clear().is_err());
    assert!(store.clear_with_active_ids(&["other".to_string()]).is_err());
    assert!(store.delete_by_ids(&["must-survive".to_string()]).is_err());
    {
        let conn = store.write_conn.lock();
        assert_eq!(store.delete_expired_by_cutoff(&conn, i64::MAX as u64), 0);
        assert_eq!(store.delete_oldest_by_limit(&conn, 1), 0);
    }
    assert!(store.get_by_id("must-survive").is_some());
    assert_eq!(store.traffic_statistics().total_requests, 1);
    assert_eq!(store.database_epoch(), epoch);
    drop(store);
    let reopened = TrafficDbStore::new(dir.path().to_path_buf(), 100, 0, None).unwrap();
    assert!(reopened.get_by_id("must-survive").is_some());
    assert_eq!(reopened.database_epoch(), epoch);
    assert_eq!(reopened.current_sequence(), 2);
}
