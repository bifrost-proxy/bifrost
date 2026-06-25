use std::fs;
use std::path::PathBuf;

use bifrost_core::{BifrostError, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

const MAX_NOTIFICATION_RECORDS: i64 = 200;
const MAX_NOTIFICATION_AGE_DAYS: i64 = 90;
const NOTIFICATION_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationRecord {
    pub id: i64,
    pub notification_type: String,
    pub title: String,
    pub message: String,
    pub metadata: Option<String>,
    pub status: String,
    pub action_taken: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateNotification {
    pub notification_type: String,
    pub title: String,
    pub message: String,
    pub metadata: Option<String>,
}

pub fn notification_db_path() -> Result<PathBuf> {
    let dir = bifrost_storage::data_dir().join("admin");
    fs::create_dir_all(&dir)?;
    Ok(dir.join("notifications.db"))
}

fn open_notification_db() -> Result<Connection> {
    let db_path = notification_db_path()?;
    let conn = Connection::open(&db_path)
        .map_err(|e| BifrostError::Storage(format!("Failed to open notification db: {e}")))?;

    match init_db(&conn) {
        Ok(()) => Ok(conn),
        Err(SchemaError::VersionMismatch { current, expected }) => {
            tracing::warn!(
                current_version = current,
                expected_version = expected,
                "[NOTIFICATION_DB] Schema version mismatch, resetting database"
            );
            drop(conn);
            if let Err(e) = fs::remove_file(&db_path) {
                tracing::error!("[NOTIFICATION_DB] Failed to remove old database: {e}");
            }
            let new_conn = Connection::open(&db_path).map_err(|e| {
                BifrostError::Storage(format!("Failed to open notification db: {e}"))
            })?;
            init_db(&new_conn).map_err(|e| {
                BifrostError::Storage(format!("Failed to init notification db: {e}"))
            })?;
            tracing::info!("[NOTIFICATION_DB] Database reset successfully");
            Ok(new_conn)
        }
        Err(e) => Err(BifrostError::Storage(format!(
            "Failed to init notification db: {e}"
        ))),
    }
}

enum SchemaError {
    Sqlite(rusqlite::Error),
    VersionMismatch { current: u32, expected: u32 },
}

impl std::fmt::Debug for SchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

impl std::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SchemaError::Sqlite(e) => write!(f, "SQLite error: {e}"),
            SchemaError::VersionMismatch { current, expected } => {
                write!(
                    f,
                    "Schema version mismatch: current={current}, expected={expected}"
                )
            }
        }
    }
}

impl From<rusqlite::Error> for SchemaError {
    fn from(e: rusqlite::Error) -> Self {
        SchemaError::Sqlite(e)
    }
}

fn init_db(conn: &Connection) -> std::result::Result<(), SchemaError> {
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;\
         PRAGMA journal_mode = WAL;\
         PRAGMA synchronous = NORMAL;\
         CREATE TABLE IF NOT EXISTS notification_metadata (\
           key TEXT PRIMARY KEY NOT NULL,\
           value TEXT NOT NULL\
         );\
         CREATE TABLE IF NOT EXISTS notifications (\
           id INTEGER PRIMARY KEY AUTOINCREMENT,\
           notification_type TEXT NOT NULL,\
           title TEXT NOT NULL,\
           message TEXT NOT NULL,\
           metadata TEXT,\
           status TEXT NOT NULL DEFAULT 'unread',\
           action_taken TEXT,\
           created_at INTEGER NOT NULL,\
           updated_at INTEGER NOT NULL\
         );\
         CREATE INDEX IF NOT EXISTS idx_notifications_type ON notifications(notification_type);\
         CREATE INDEX IF NOT EXISTS idx_notifications_status ON notifications(status);\
         CREATE INDEX IF NOT EXISTS idx_notifications_created ON notifications(created_at);",
    )?;

    let current = get_schema_version(conn);
    if current != 0 && current != NOTIFICATION_SCHEMA_VERSION {
        return Err(SchemaError::VersionMismatch {
            current,
            expected: NOTIFICATION_SCHEMA_VERSION,
        });
    }

    conn.execute(
        "INSERT OR REPLACE INTO notification_metadata (key, value) VALUES ('schema_version', ?1)",
        params![NOTIFICATION_SCHEMA_VERSION.to_string()],
    )?;

    Ok(())
}

fn get_schema_version(conn: &Connection) -> u32 {
    conn.query_row(
        "SELECT value FROM notification_metadata WHERE key = 'schema_version'",
        [],
        |row| {
            let v: String = row.get(0)?;
            Ok(v.parse::<u32>().unwrap_or(0))
        },
    )
    .unwrap_or(0)
}

pub fn create_notification(input: &CreateNotification) -> Result<i64> {
    let conn = open_notification_db()?;

    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO notifications(notification_type, title, message, metadata, status, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, 'unread', ?5, ?5)",
        params![
            input.notification_type,
            input.title,
            input.message,
            input.metadata,
            now,
        ],
    )
    .map_err(|e| BifrostError::Storage(format!("Failed to insert notification: {e}")))?;

    let id = conn.last_insert_rowid();

    cleanup_old_records(&conn)
        .map_err(|e| BifrostError::Storage(format!("Failed to cleanup notifications: {e}")))?;

    Ok(id)
}

pub fn list_notifications(
    notification_type: Option<&str>,
    status: Option<&str>,
    limit: usize,
    offset: usize,
) -> Result<Vec<NotificationRecord>> {
    let db_path = notification_db_path()?;
    if !db_path.exists() {
        return Ok(Vec::new());
    }
    let conn = open_notification_db()?;

    let mut conditions = Vec::new();
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(t) = notification_type {
        conditions.push(format!("notification_type = ?{}", param_values.len() + 1));
        param_values.push(Box::new(t.to_string()));
    }
    if let Some(s) = status {
        conditions.push(format!("status = ?{}", param_values.len() + 1));
        param_values.push(Box::new(s.to_string()));
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    let sql = format!(
        "SELECT id, notification_type, title, message, metadata, status, action_taken, created_at, updated_at \
         FROM notifications {where_clause} \
         ORDER BY id DESC \
         LIMIT ?{} OFFSET ?{}",
        param_values.len() + 1,
        param_values.len() + 2,
    );

    param_values.push(Box::new(limit as i64));
    param_values.push(Box::new(offset as i64));

    let params_ref: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|p| p.as_ref()).collect();

    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| BifrostError::Storage(format!("Failed to prepare query: {e}")))?;

    let rows = stmt
        .query_map(params_ref.as_slice(), |row| {
            Ok(NotificationRecord {
                id: row.get(0)?,
                notification_type: row.get(1)?,
                title: row.get(2)?,
                message: row.get(3)?,
                metadata: row.get(4)?,
                status: row.get(5)?,
                action_taken: row.get(6)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
            })
        })
        .map_err(|e| BifrostError::Storage(format!("Failed to query notifications: {e}")))?;

    let mut out = Vec::new();
    for r in rows {
        out.push(
            r.map_err(|e| BifrostError::Storage(format!("Failed to read notification: {e}")))?,
        );
    }
    Ok(out)
}

pub fn count_notifications(notification_type: Option<&str>, status: Option<&str>) -> Result<i64> {
    let db_path = notification_db_path()?;
    if !db_path.exists() {
        return Ok(0);
    }
    let conn = open_notification_db()?;

    let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) =
        match (notification_type, status) {
            (Some(t), Some(s)) => (
                "SELECT COUNT(1) FROM notifications WHERE notification_type = ?1 AND status = ?2"
                    .to_string(),
                vec![Box::new(t.to_string()), Box::new(s.to_string())],
            ),
            (Some(t), None) => (
                "SELECT COUNT(1) FROM notifications WHERE notification_type = ?1".to_string(),
                vec![Box::new(t.to_string())],
            ),
            (None, Some(s)) => (
                "SELECT COUNT(1) FROM notifications WHERE status = ?1".to_string(),
                vec![Box::new(s.to_string())],
            ),
            (None, None) => ("SELECT COUNT(1) FROM notifications".to_string(), vec![]),
        };

    let params_ref: Vec<&dyn rusqlite::types::ToSql> =
        params_vec.iter().map(|p| p.as_ref()).collect();

    let count: i64 = conn
        .query_row(&sql, params_ref.as_slice(), |row| row.get(0))
        .map_err(|e| BifrostError::Storage(format!("Failed to count notifications: {e}")))?;
    Ok(count)
}

pub fn update_notification_status(
    id: i64,
    status: &str,
    action_taken: Option<&str>,
) -> Result<bool> {
    let conn = open_notification_db()?;
    let now = chrono::Utc::now().timestamp();

    let affected = conn
        .execute(
            "UPDATE notifications SET status = ?1, action_taken = ?2, updated_at = ?3 WHERE id = ?4",
            params![status, action_taken, now, id],
        )
        .map_err(|e| BifrostError::Storage(format!("Failed to update notification: {e}")))?;

    Ok(affected > 0)
}

pub fn mark_all_as_read(notification_type: Option<&str>) -> Result<usize> {
    let conn = open_notification_db()?;
    let now = chrono::Utc::now().timestamp();

    let affected = if let Some(t) = notification_type {
        conn.execute(
            "UPDATE notifications SET status = 'read', updated_at = ?1 WHERE status = 'unread' AND notification_type = ?2",
            params![now, t],
        )
    } else {
        conn.execute(
            "UPDATE notifications SET status = 'read', updated_at = ?1 WHERE status = 'unread'",
            params![now],
        )
    }
    .map_err(|e| BifrostError::Storage(format!("Failed to mark notifications as read: {e}")))?;

    Ok(affected)
}

pub fn count_unread() -> Result<i64> {
    count_notifications(None, Some("unread"))
}

fn cleanup_old_records(conn: &Connection) -> std::result::Result<(), rusqlite::Error> {
    do_cleanup(conn)
}

fn do_cleanup(conn: &Connection) -> std::result::Result<(), rusqlite::Error> {
    let cutoff_ts = chrono::Utc::now().timestamp() - MAX_NOTIFICATION_AGE_DAYS * 86400;
    conn.execute(
        "DELETE FROM notifications WHERE created_at < ?1",
        params![cutoff_ts],
    )?;

    let current_count: i64 =
        conn.query_row("SELECT COUNT(1) FROM notifications", [], |row| row.get(0))?;

    if current_count > MAX_NOTIFICATION_RECORDS {
        tracing::info!(
            current_count,
            max_records = MAX_NOTIFICATION_RECORDS,
            "[NOTIFICATION_DB] Cleanup triggered: trimming to latest records"
        );
        conn.execute(
            "DELETE FROM notifications WHERE id NOT IN \
             (SELECT id FROM notifications ORDER BY id DESC LIMIT ?1)",
            params![MAX_NOTIFICATION_RECORDS],
        )?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_test_db() -> (tempfile::TempDir, Connection) {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("notifications.db");
        let conn = Connection::open(&db_path).unwrap();
        init_db(&conn).unwrap();
        (tmp, conn)
    }

    #[test]
    fn test_create_and_list_notifications() {
        let (_tmp, conn) = setup_test_db();
        let now = chrono::Utc::now().timestamp();

        conn.execute(
            "INSERT INTO notifications(notification_type, title, message, metadata, status, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, 'unread', ?5, ?5)",
            params!["tls_trust_change", "TLS Trust Change", "Client 192.168.1.1 not trusted", "{}", now],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO notifications(notification_type, title, message, metadata, status, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, 'unread', ?5, ?5)",
            params!["pending_authorization", "New Client", "Client 10.0.0.1 pending", "{}", now],
        )
        .unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(1) FROM notifications", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_update_notification_status() {
        let (_tmp, conn) = setup_test_db();
        let now = chrono::Utc::now().timestamp();

        conn.execute(
            "INSERT INTO notifications(notification_type, title, message, status, created_at, updated_at) \
             VALUES ('tls_trust_change', 'Test', 'Test message', 'unread', ?1, ?1)",
            params![now],
        )
        .unwrap();

        let id = conn.last_insert_rowid();
        let affected = conn
            .execute(
                "UPDATE notifications SET status = 'read', action_taken = 'acknowledged', updated_at = ?1 WHERE id = ?2",
                params![now, id],
            )
            .unwrap();
        assert_eq!(affected, 1);

        let status: String = conn
            .query_row(
                "SELECT status FROM notifications WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "read");
    }

    #[test]
    fn test_cleanup_old_notifications() {
        let (_tmp, conn) = setup_test_db();
        let now = chrono::Utc::now().timestamp();
        let expired_ts = now - 91 * 86400;

        for i in 0..5 {
            conn.execute(
                "INSERT INTO notifications(notification_type, title, message, status, created_at, updated_at) \
                 VALUES ('test', ?1, 'old', 'read', ?2, ?2)",
                params![format!("old-{i}"), expired_ts],
            )
            .unwrap();
        }
        for i in 0..3 {
            conn.execute(
                "INSERT INTO notifications(notification_type, title, message, status, created_at, updated_at) \
                 VALUES ('test', ?1, 'new', 'unread', ?2, ?2)",
                params![format!("new-{i}"), now],
            )
            .unwrap();
        }

        do_cleanup(&conn).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(1) FROM notifications", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 3);
    }

    #[test]
    fn test_cleanup_keeps_records_when_below_limit() {
        let (_tmp, conn) = setup_test_db();
        let now = chrono::Utc::now().timestamp();

        for i in 0..100 {
            conn.execute(
                "INSERT INTO notifications(notification_type, title, message, status, created_at, updated_at) \
                 VALUES ('test', ?1, 'msg', 'unread', ?2, ?2)",
                params![format!("n-{i}"), now],
            )
            .unwrap();
        }

        do_cleanup(&conn).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(1) FROM notifications", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 100);
    }

    #[test]
    fn test_cleanup_trims_to_latest_200_records() {
        let (_tmp, conn) = setup_test_db();
        let now = chrono::Utc::now().timestamp();

        let insert_count = MAX_NOTIFICATION_RECORDS + 5;
        for i in 0..insert_count {
            conn.execute(
                "INSERT INTO notifications(notification_type, title, message, status, created_at, updated_at) \
                 VALUES ('test', ?1, 'msg', 'unread', ?2, ?2)",
                params![format!("n-{i}"), now],
            )
            .unwrap();
        }

        do_cleanup(&conn).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(1) FROM notifications", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, MAX_NOTIFICATION_RECORDS);

        let min_id: i64 = conn
            .query_row("SELECT MIN(id) FROM notifications", [], |row| row.get(0))
            .unwrap();
        let max_id: i64 = conn
            .query_row("SELECT MAX(id) FROM notifications", [], |row| row.get(0))
            .unwrap();
        assert_eq!(min_id, 6);
        assert_eq!(max_id, insert_count);
    }

    #[test]
    fn test_cleanup_runs_after_every_notification_write() {
        let tmp = tempfile::tempdir().unwrap();
        let _data_dir_guard = crate::test_env::BifrostDataDirGuard::set(tmp.path());

        for i in 0..(MAX_NOTIFICATION_RECORDS + 5) {
            create_notification(&CreateNotification {
                notification_type: "test".to_string(),
                title: format!("n-{i}"),
                message: "msg".to_string(),
                metadata: None,
            })
            .unwrap();
        }

        let records = list_notifications(None, None, 500, 0).unwrap();
        assert_eq!(records.len(), MAX_NOTIFICATION_RECORDS as usize);
        assert_eq!(records.first().unwrap().title, "n-204");
        assert_eq!(records.last().unwrap().title, "n-5");
    }
}
