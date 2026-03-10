use rusqlite::Connection;

pub const SCHEMA_VERSION: u32 = 5;

#[derive(Debug)]
pub enum InitError {
    Sqlite(rusqlite::Error),
    VersionMismatch { current: u32, expected: u32 },
}

impl From<rusqlite::Error> for InitError {
    fn from(e: rusqlite::Error) -> Self {
        InitError::Sqlite(e)
    }
}

impl std::fmt::Display for InitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InitError::Sqlite(e) => write!(f, "SQLite error: {}", e),
            InitError::VersionMismatch { current, expected } => {
                write!(
                    f,
                    "Schema version mismatch: current={}, expected={}",
                    current, expected
                )
            }
        }
    }
}

impl std::error::Error for InitError {}

pub fn check_schema_version(conn: &Connection) -> Result<(), InitError> {
    let current_version = get_schema_version(conn);
    if current_version != 0 && current_version != SCHEMA_VERSION {
        return Err(InitError::VersionMismatch {
            current: current_version,
            expected: SCHEMA_VERSION,
        });
    }
    Ok(())
}

pub fn init_database(conn: &Connection) -> Result<(), InitError> {
    conn.execute_batch(
        "
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
        PRAGMA cache_size = 10000;
        PRAGMA temp_store = MEMORY;
        PRAGMA mmap_size = 268435456;
        ",
    )?;

    check_schema_version(conn)?;

    conn.execute_batch(SCHEMA_SQL)?;

    conn.execute(
        "INSERT OR REPLACE INTO metadata (key, value) VALUES ('schema_version', ?)",
        [SCHEMA_VERSION.to_string()],
    )?;

    Ok(())
}

fn get_schema_version(conn: &Connection) -> u32 {
    conn.query_row(
        "SELECT value FROM metadata WHERE key = 'schema_version'",
        [],
        |row| {
            let version_str: String = row.get(0)?;
            Ok(version_str.parse::<u32>().unwrap_or(0))
        },
    )
    .unwrap_or(0)
}

const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS traffic_records (
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
    timing_blob BLOB,
    request_headers_blob BLOB,
    response_headers_blob BLOB,
    matched_rules_blob BLOB,
    socket_status_blob BLOB,
    request_body_ref_blob BLOB,
    response_body_ref_blob BLOB,
    actual_url TEXT,
    actual_host TEXT,
    original_request_headers_blob BLOB,
    actual_response_headers_blob BLOB,
    req_script_results_blob BLOB,
    res_script_results_blob BLOB,
    error_message TEXT
);

CREATE INDEX IF NOT EXISTS idx_id ON traffic_records(id);
CREATE INDEX IF NOT EXISTS idx_timestamp ON traffic_records(timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_host ON traffic_records(host);
CREATE INDEX IF NOT EXISTS idx_status ON traffic_records(status) WHERE status > 0;
CREATE INDEX IF NOT EXISTS idx_method ON traffic_records(method);
CREATE INDEX IF NOT EXISTS idx_client_app ON traffic_records(client_app) WHERE client_app IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_flags ON traffic_records(flags);
CREATE INDEX IF NOT EXISTS idx_seq_desc ON traffic_records(sequence DESC);
CREATE INDEX IF NOT EXISTS idx_host_seq ON traffic_records(host, sequence DESC);
CREATE INDEX IF NOT EXISTS idx_status_seq ON traffic_records(status, sequence DESC);

-- Body index for fast negative filtering in search (v1)
-- Note: project rule says no backwards compatibility; bump schema_version and recreate DB.
CREATE TABLE IF NOT EXISTS traffic_body_index_v1 (
    id TEXT NOT NULL,
    kind INTEGER NOT NULL, -- 0=req_body, 1=res_body
    algo_version INTEGER NOT NULL,
    block_size INTEGER NOT NULL,
    bitset_bits INTEGER NOT NULL,
    body_path TEXT NOT NULL,
    range_offset INTEGER NOT NULL DEFAULT 0,
    body_size INTEGER NOT NULL,
    block_count INTEGER NOT NULL,
    bitsets BLOB NOT NULL,
    PRIMARY KEY (id, kind)
);

CREATE INDEX IF NOT EXISTS idx_body_index_id ON traffic_body_index_v1(id);

CREATE TABLE IF NOT EXISTS metadata (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);
"#;

pub fn get_insert_sql() -> &'static str {
    r#"
    INSERT INTO traffic_records (
        sequence, id, timestamp, host, method, status, protocol,
        url, path, content_type, request_content_type,
        request_size, response_size, duration_ms,
        client_ip, client_app, client_pid, client_path,
        flags, frame_count, last_frame_id,
        timing_blob, request_headers_blob, response_headers_blob,
        matched_rules_blob, socket_status_blob,
        request_body_ref_blob, response_body_ref_blob,
        actual_url, actual_host, original_request_headers_blob,
        actual_response_headers_blob, req_script_results_blob,
        res_script_results_blob, error_message
    ) VALUES (
        ?1, ?2, ?3, ?4, ?5, ?6, ?7,
        ?8, ?9, ?10, ?11,
        ?12, ?13, ?14,
        ?15, ?16, ?17, ?18,
        ?19, ?20, ?21,
        ?22, ?23, ?24,
        ?25, ?26,
        ?27, ?28,
        ?29, ?30, ?31,
        ?32, ?33, ?34,
        ?35
    )
    "#
}

pub fn get_update_sql() -> &'static str {
    r#"
    UPDATE traffic_records SET
        status = ?1,
        content_type = ?2,
        request_size = ?3,
        response_size = ?4,
        duration_ms = ?5,
        client_app = ?6,
        client_pid = ?7,
        client_path = ?8,
        flags = ?9,
        frame_count = ?10,
        last_frame_id = ?11,
        timing_blob = ?12,
        request_headers_blob = ?13,
        response_headers_blob = ?14,
        matched_rules_blob = ?15,
        socket_status_blob = ?16,
        request_body_ref_blob = ?17,
        response_body_ref_blob = ?18,
        actual_url = ?19,
        actual_host = ?20,
        original_request_headers_blob = ?21,
        actual_response_headers_blob = ?22,
        req_script_results_blob = ?23,
        res_script_results_blob = ?24,
        error_message = ?25
    WHERE id = ?26
    "#
}
