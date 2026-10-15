use super::*;
use crate::body_store::BodyRef;
use std::env;
use std::sync::atomic::{AtomicU64, Ordering};

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

fn create_test_dir() -> PathBuf {
    let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = env::temp_dir().join(format!(
        "bifrost_traffic_db_test_{}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        counter
    ));
    let _ = fs::create_dir_all(&dir);
    dir
}

fn cleanup_test_dir(dir: &PathBuf) {
    let _ = fs::remove_dir_all(dir);
}

fn epoch_test_record(id: &str) -> TrafficRecord {
    TrafficRecord::new(
        id.to_string(),
        "GET".to_string(),
        "https://example.test/epoch".to_string(),
    )
}

fn create_legacy_v10_traffic_db(db_path: &PathBuf) {
    let conn = Connection::open(db_path).unwrap();
    conn.execute_batch(
        r#"
        CREATE TABLE traffic_records (
            sequence INTEGER PRIMARY KEY,
            id TEXT NOT NULL UNIQUE,
            timestamp INTEGER NOT NULL,
            host TEXT NOT NULL,
            method TEXT NOT NULL,
            status INTEGER NOT NULL DEFAULT 0,
            protocol TEXT NOT NULL,
            url TEXT NOT NULL,
            path TEXT NOT NULL,
            content_type TEXT,
            request_content_type TEXT,
            request_size INTEGER NOT NULL DEFAULT 0,
            response_size INTEGER NOT NULL DEFAULT 0,
            duration_ms INTEGER NOT NULL DEFAULT 0,
            client_ip TEXT NOT NULL DEFAULT '',
            client_app TEXT,
            client_pid INTEGER,
            client_path TEXT,
            flags INTEGER NOT NULL DEFAULT 0,
            frame_count INTEGER NOT NULL DEFAULT 0,
            last_frame_id INTEGER NOT NULL DEFAULT 0,
            socket_is_open INTEGER NOT NULL DEFAULT 0,
            socket_send_count INTEGER NOT NULL DEFAULT 0,
            socket_receive_count INTEGER NOT NULL DEFAULT 0,
            socket_send_bytes INTEGER NOT NULL DEFAULT 0,
            socket_receive_bytes INTEGER NOT NULL DEFAULT 0,
            socket_frame_count INTEGER NOT NULL DEFAULT 0,
            rule_count INTEGER NOT NULL DEFAULT 0,
            rule_protocols TEXT NOT NULL DEFAULT '[]'
        );
        CREATE TABLE metadata (
            key TEXT PRIMARY KEY NOT NULL,
            value TEXT NOT NULL
        );
        INSERT INTO metadata (key, value) VALUES ('schema_version', '10');
        INSERT INTO traffic_records (
            sequence, id, timestamp, host, method, status, protocol, url, path, client_ip
        ) VALUES (
            1, 'legacy-record', 123, 'example.test', 'GET', 200, 'http',
            'http://example.test/path', '/path', '127.0.0.1'
        );
        "#,
    )
    .unwrap();
}

fn create_current_version_broken_traffic_db(db_path: &PathBuf) {
    let conn = Connection::open(db_path).unwrap();
    conn.execute_batch(
        r#"
        CREATE TABLE traffic_records (
            sequence INTEGER PRIMARY KEY,
            id TEXT NOT NULL UNIQUE,
            timestamp INTEGER NOT NULL,
            host TEXT NOT NULL,
            method TEXT NOT NULL,
            status INTEGER NOT NULL DEFAULT 0,
            protocol TEXT NOT NULL,
            url TEXT NOT NULL,
            path TEXT NOT NULL
        );
        CREATE TABLE metadata (
            key TEXT PRIMARY KEY NOT NULL,
            value TEXT NOT NULL
        );
        INSERT INTO metadata (key, value) VALUES ('schema_version', '14');
        INSERT INTO traffic_records (
            sequence, id, timestamp, host, method, status, protocol, url, path
        ) VALUES (
            1, 'broken-record', 123, 'example.test', 'GET', 200, 'http',
            'http://example.test/path', '/path'
        );
        "#,
    )
    .unwrap();
}

fn sqlite_object_exists(db_path: &PathBuf, object_type: &str, name: &str) -> bool {
    let conn = Connection::open(db_path).unwrap();
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = ?1 AND name = ?2)",
        [object_type, name],
        |row| row.get::<_, i64>(0),
    )
    .unwrap()
        != 0
}

fn sqlite_column_exists(db_path: &PathBuf, table: &str, column: &str) -> bool {
    let conn = Connection::open(db_path).unwrap();
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({})", table))
        .unwrap();
    let rows = stmt.query_map([], |row| row.get::<_, String>(1)).unwrap();
    for row in rows {
        if row.unwrap() == column {
            return true;
        }
    }
    false
}

mod database;
mod read_pool;
mod reads;
mod retention;
mod schema;
mod statistics;
