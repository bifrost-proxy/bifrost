use super::*;

#[test]
fn test_incompatible_traffic_db_is_reset_instead_of_failing_startup() {
    let dir = create_test_dir();
    let db_path = dir.join("traffic.db");
    create_legacy_v10_traffic_db(&db_path);

    let store = TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap();

    assert_eq!(store.record_count.load(Ordering::Relaxed), 0);
    assert!(sqlite_column_exists(
        &db_path,
        "traffic_records",
        "devtools_client_req_id"
    ));
    assert!(sqlite_object_exists(
        &db_path,
        "index",
        "idx_devtools_client_req_id"
    ));

    cleanup_test_dir(&dir);
}

#[test]
fn test_v14_database_migrates_body_metadata_without_losing_records() {
    let dir = create_test_dir();
    let db_path = dir.join("traffic.db");
    {
        let store = TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap();
        store.record(TrafficRecord::new(
            "v14-preserved".to_string(),
            "GET".to_string(),
            "https://example.test/preserved".to_string(),
        ));
    }
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute(
            "ALTER TABLE traffic_record_details DROP COLUMN body_metadata_blob",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE metadata SET value = '14' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
    }

    let store = TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap();

    assert!(store.get_by_id("v14-preserved").is_some());
    assert!(sqlite_column_exists(
        &db_path,
        "traffic_record_details",
        "body_metadata_blob"
    ));
    let conn = Connection::open(&db_path).unwrap();
    let version: String = conn
        .query_row(
            "SELECT value FROM metadata WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        version,
        crate::traffic_db::schema::SCHEMA_VERSION.to_string()
    );

    cleanup_test_dir(&dir);
}

#[test]
fn test_broken_current_traffic_db_is_reset_instead_of_failing_startup() {
    let dir = create_test_dir();
    let db_path = dir.join("traffic.db");
    create_current_version_broken_traffic_db(&db_path);

    let store = TrafficDbStore::new(dir.clone(), 100, 0, None).unwrap();

    assert_eq!(store.record_count.load(Ordering::Relaxed), 0);
    assert!(sqlite_column_exists(
        &db_path,
        "traffic_records",
        "devtools_client_req_id"
    ));
    assert!(sqlite_object_exists(
        &db_path,
        "index",
        "idx_devtools_client_req_id"
    ));

    cleanup_test_dir(&dir);
}

#[test]
fn test_reset_database_reports_remove_failure_for_directory_path() {
    let dir = create_test_dir();
    let db_path = dir.join("traffic.db");
    fs::create_dir(&db_path).unwrap();

    assert!(TrafficDbStore::reset_database(&db_path).is_err());

    cleanup_test_dir(&dir);
}

#[cfg(unix)]
#[test]
fn test_reset_database_reports_sqlite_init_failure_when_old_db_cannot_be_removed() {
    use std::os::unix::fs::PermissionsExt;

    let dir = create_test_dir();
    let db_path = dir.join("traffic.db");
    create_current_version_broken_traffic_db(&db_path);

    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
    let result = TrafficDbStore::reset_database(&db_path);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();

    assert!(result.is_err());

    cleanup_test_dir(&dir);
}

#[test]
fn test_schema_does_not_keep_flags_index() {
    let dir = create_test_dir();
    let store = TrafficDbStore::new(dir.clone(), 5_000, 0, None).unwrap();

    let conn = store.read_pool.acquire();
    let has_idx_flags: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = 'idx_flags')",
            [],
            |row| row.get(0),
        )
        .unwrap();

    assert!(!has_idx_flags);

    cleanup_test_dir(&dir);
}
