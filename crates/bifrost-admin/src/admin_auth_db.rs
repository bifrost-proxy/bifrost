use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use bifrost_core::{BifrostError, Result};
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use tracing::{info, warn};

const AUTH_SCHEMA_VERSION: u32 = 1;

pub type SharedAuthDb = Arc<AuthDb>;

pub struct AuthDb {
    conn: Mutex<Connection>,
}

pub fn auth_db_path() -> Result<PathBuf> {
    let dir = bifrost_storage::data_dir().join("admin");
    fs::create_dir_all(&dir)?;
    Ok(dir.join("auth.db"))
}

pub fn auth_db_path_in(data_dir: &std::path::Path) -> Result<PathBuf> {
    let dir = data_dir.join("admin");
    fs::create_dir_all(&dir)?;
    Ok(dir.join("auth.db"))
}

impl AuthDb {
    pub fn open_default() -> Result<Self> {
        let db_path = auth_db_path()?;
        Self::open(&db_path)
    }

    pub fn open(db_path: &std::path::Path) -> Result<Self> {
        let conn = open_conn(db_path)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn get(&self, key: &str) -> Option<String> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT value FROM auth_kv WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .ok()
    }

    pub fn set(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR REPLACE INTO auth_kv (key, value) VALUES (?1, ?2)",
            params![key, value],
        )
        .map_err(|e| BifrostError::Storage(format!("auth_db set({key}): {e}")))?;
        Ok(())
    }

    pub fn delete(&self, key: &str) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM auth_kv WHERE key = ?1", params![key])
            .map_err(|e| BifrostError::Storage(format!("auth_db delete({key}): {e}")))?;
        Ok(())
    }

    pub fn get_username(&self) -> Option<String> {
        self.get("username")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    pub fn set_username(&self, username: &str) -> Result<()> {
        self.set("username", username)
    }

    pub fn get_password_hash(&self) -> Option<String> {
        self.get("password_hash").filter(|s| !s.trim().is_empty())
    }

    pub fn set_password_hash(&self, hash: &str) -> Result<()> {
        self.set("password_hash", hash)
    }

    pub fn clear_password_hash(&self) -> Result<()> {
        self.delete("password_hash")
    }

    pub fn has_password(&self) -> bool {
        self.get_password_hash().is_some()
    }

    pub fn get_jwt_secret(&self) -> Option<String> {
        self.get("jwt_secret").filter(|s| !s.trim().is_empty())
    }

    pub fn set_jwt_secret(&self, secret: &str) -> Result<()> {
        self.set("jwt_secret", secret)
    }

    pub fn get_revoke_before(&self) -> Option<i64> {
        self.get("revoke_before")
            .and_then(|s| s.trim().parse::<i64>().ok())
    }

    pub fn set_revoke_before(&self, ts: i64) -> Result<()> {
        self.set("revoke_before", &ts.to_string())
    }

    pub fn get_failed_count(&self) -> u32 {
        self.get("login_failed_count")
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(0)
    }

    pub fn set_failed_count(&self, count: u32) -> Result<()> {
        self.set("login_failed_count", &count.to_string())
    }

    pub fn increment_failed_count(&self) -> Result<u32> {
        let current = self.get_failed_count();
        let new_count = current.saturating_add(1);
        self.set_failed_count(new_count)?;
        Ok(new_count)
    }

    pub fn reset_failed_count(&self) -> Result<()> {
        self.set_failed_count(0)
    }

    pub fn is_remote_access_enabled(&self) -> bool {
        self.get("remote_access_enabled")
            .map(|v| {
                matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .unwrap_or(false)
    }

    pub fn set_remote_access_enabled(&self, enabled: bool) -> Result<()> {
        self.set(
            "remote_access_enabled",
            if enabled { "true" } else { "false" },
        )
    }
}

enum SchemaError {
    Sqlite(rusqlite::Error),
    VersionMismatch { current: u32, expected: u32 },
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
        "PRAGMA journal_mode = WAL;\
         PRAGMA synchronous = NORMAL;\
         CREATE TABLE IF NOT EXISTS auth_metadata (\
           key TEXT PRIMARY KEY NOT NULL,\
           value TEXT NOT NULL\
         );\
         CREATE TABLE IF NOT EXISTS auth_kv (\
           key TEXT PRIMARY KEY NOT NULL,\
           value TEXT NOT NULL\
         );",
    )?;

    let current = get_schema_version(conn);
    if current != 0 && current != AUTH_SCHEMA_VERSION {
        return Err(SchemaError::VersionMismatch {
            current,
            expected: AUTH_SCHEMA_VERSION,
        });
    }

    conn.execute(
        "INSERT OR REPLACE INTO auth_metadata (key, value) VALUES ('schema_version', ?1)",
        params![AUTH_SCHEMA_VERSION.to_string()],
    )?;

    Ok(())
}

fn get_schema_version(conn: &Connection) -> u32 {
    conn.query_row(
        "SELECT value FROM auth_metadata WHERE key = 'schema_version'",
        [],
        |row| {
            let v: String = row.get(0)?;
            Ok(v.parse::<u32>().unwrap_or(0))
        },
    )
    .unwrap_or(0)
}

/// Move a schema-mismatched database file aside to a timestamped backup so it
/// is preserved (for recovery/forensics) instead of silently deleted. Also
/// moves the SQLite `-wal` / `-shm` sidecar files if present.
fn backup_mismatched_db(db_path: &std::path::Path) -> std::io::Result<()> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let backup = db_path.with_extension(format!("bak.{ts}"));
    fs::rename(db_path, &backup)?;
    warn!(backup = %backup.display(), "[DB] mismatched database moved to backup");
    for sidecar in ["wal", "shm"] {
        let mut side = db_path.as_os_str().to_os_string();
        side.push(format!("-{sidecar}"));
        let side = PathBuf::from(side);
        if side.exists() {
            let mut side_bak = backup.as_os_str().to_os_string();
            side_bak.push(format!("-{sidecar}"));
            let _ = fs::rename(&side, PathBuf::from(side_bak));
        }
    }
    Ok(())
}

fn open_conn(db_path: &std::path::Path) -> Result<Connection> {
    let conn = Connection::open(db_path)
        .map_err(|e| BifrostError::Storage(format!("Failed to open auth db: {e}")))?;

    match init_db(&conn) {
        Ok(()) => Ok(conn),
        Err(SchemaError::VersionMismatch { current, expected }) => {
            warn!(
                current_version = current,
                expected_version = expected,
                "[AUTH_DB] Schema version mismatch, backing up and recreating database"
            );
            drop(conn);
            // Preserve the old database rather than deleting it: a silent
            // delete is irreversible data loss (sessions, credentials) and
            // destroys forensic evidence after an unexpected downgrade. Move it
            // aside to a timestamped backup so it can be inspected/recovered.
            if let Err(e) = backup_mismatched_db(db_path) {
                tracing::error!("[AUTH_DB] Failed to back up old database: {e}");
                return Err(BifrostError::Storage(format!(
                    "auth db schema mismatch and backup failed: {e}"
                )));
            }
            let new_conn = Connection::open(db_path)
                .map_err(|e| BifrostError::Storage(format!("Failed to open auth db: {e}")))?;
            init_db(&new_conn)
                .map_err(|e| BifrostError::Storage(format!("Failed to init auth db: {e}")))?;
            info!("[AUTH_DB] Database recreated after backing up the previous file");
            Ok(new_conn)
        }
        Err(e) => Err(BifrostError::Storage(format!(
            "Failed to init auth db: {e}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> (AuthDb, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db_path = tmp.path().join("auth.db");
        let db = AuthDb::open(&db_path).expect("open auth db");
        (db, tmp)
    }

    #[test]
    fn test_get_set_delete() {
        let (db, _tmp) = temp_db();
        assert!(db.get("foo").is_none());
        db.set("foo", "bar").unwrap();
        assert_eq!(db.get("foo"), Some("bar".to_string()));
        db.delete("foo").unwrap();
        assert!(db.get("foo").is_none());
    }

    #[test]
    fn test_username() {
        let (db, _tmp) = temp_db();
        assert!(db.get_username().is_none());
        db.set_username("admin").unwrap();
        assert_eq!(db.get_username(), Some("admin".to_string()));
    }

    #[test]
    fn test_password_hash() {
        let (db, _tmp) = temp_db();
        assert!(!db.has_password());
        db.set_password_hash("$2b$12$hash").unwrap();
        assert!(db.has_password());
        assert_eq!(db.get_password_hash(), Some("$2b$12$hash".to_string()));
        db.clear_password_hash().unwrap();
        assert!(!db.has_password());
    }

    #[test]
    fn test_jwt_secret() {
        let (db, _tmp) = temp_db();
        assert!(db.get_jwt_secret().is_none());
        db.set_jwt_secret("mysecret").unwrap();
        assert_eq!(db.get_jwt_secret(), Some("mysecret".to_string()));
    }

    #[test]
    fn test_revoke_before() {
        let (db, _tmp) = temp_db();
        assert!(db.get_revoke_before().is_none());
        db.set_revoke_before(1234567890).unwrap();
        assert_eq!(db.get_revoke_before(), Some(1234567890));
    }

    #[test]
    fn test_failed_count() {
        let (db, _tmp) = temp_db();
        assert_eq!(db.get_failed_count(), 0);
        let c = db.increment_failed_count().unwrap();
        assert_eq!(c, 1);
        assert_eq!(db.get_failed_count(), 1);
        let c = db.increment_failed_count().unwrap();
        assert_eq!(c, 2);
        db.reset_failed_count().unwrap();
        assert_eq!(db.get_failed_count(), 0);
    }
}
