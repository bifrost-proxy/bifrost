use rusqlite::Connection;

pub const SCHEMA_VERSION: u32 = 12;

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

pub fn init_database(conn: &mut Connection) -> Result<(), InitError> {
    conn.execute_batch(
        "
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
        PRAGMA cache_size = 2000;
        PRAGMA temp_store = MEMORY;
        PRAGMA mmap_size = 268435456;
        PRAGMA foreign_keys = ON;
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
    listener_port INTEGER NOT NULL DEFAULT 0,
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
    rule_protocols TEXT NOT NULL DEFAULT '[]',
    devtools_client_req_id TEXT
);

CREATE INDEX IF NOT EXISTS idx_id ON traffic_records(id);
CREATE INDEX IF NOT EXISTS idx_timestamp ON traffic_records(timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_host ON traffic_records(host);
CREATE INDEX IF NOT EXISTS idx_status ON traffic_records(status) WHERE status > 0;
CREATE INDEX IF NOT EXISTS idx_method ON traffic_records(method);
CREATE INDEX IF NOT EXISTS idx_client_app ON traffic_records(client_app) WHERE client_app IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_seq_desc ON traffic_records(sequence DESC);
CREATE INDEX IF NOT EXISTS idx_host_seq ON traffic_records(host, sequence DESC);
CREATE INDEX IF NOT EXISTS idx_status_seq ON traffic_records(status, sequence DESC);
CREATE INDEX IF NOT EXISTS idx_devtools_client_req_id ON traffic_records(devtools_client_req_id, sequence) WHERE devtools_client_req_id IS NOT NULL;
DROP INDEX IF EXISTS idx_flags;

CREATE TABLE IF NOT EXISTS traffic_record_details (
    id TEXT PRIMARY KEY NOT NULL REFERENCES traffic_records(id) ON DELETE CASCADE,
    timing_blob BLOB,
    request_headers_blob BLOB,
    original_response_headers_blob BLOB,
    matched_rules_blob BLOB,
    request_body_ref_blob BLOB,
    response_body_ref_blob BLOB,
    derived_response_body_ref_blob BLOB,
    raw_request_body_ref_blob BLOB,
    raw_response_body_ref_blob BLOB,
    actual_url TEXT,
    actual_host TEXT,
    original_request_headers_blob BLOB,
    response_headers_blob BLOB,
    socket_status_blob BLOB,
    req_script_results_blob BLOB,
    res_script_results_blob BLOB,
    decode_req_script_results_blob BLOB,
    decode_res_script_results_blob BLOB,
    error_message TEXT
);

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
        listener_port, client_ip, client_app, client_pid, client_path,
        flags, frame_count, last_frame_id,
        socket_is_open, socket_send_count, socket_receive_count,
        socket_send_bytes, socket_receive_bytes, socket_frame_count,
        rule_count, rule_protocols, devtools_client_req_id
    ) VALUES (
        ?1, ?2, ?3, ?4, ?5, ?6, ?7,
        ?8, ?9, ?10, ?11,
        ?12, ?13, ?14,
        ?15, ?16, ?17, ?18, ?19,
        ?20, ?21, ?22,
        ?23, ?24, ?25, ?26, ?27, ?28,
        ?29, ?30, ?31
    )
    "#
}

pub fn get_insert_detail_sql() -> &'static str {
    r#"
    INSERT INTO traffic_record_details (
        id, timing_blob, request_headers_blob, original_response_headers_blob,
        matched_rules_blob, request_body_ref_blob, response_body_ref_blob,
        derived_response_body_ref_blob, raw_request_body_ref_blob, raw_response_body_ref_blob,
        actual_url, actual_host, original_request_headers_blob,
        response_headers_blob, socket_status_blob, req_script_results_blob,
        res_script_results_blob, decode_req_script_results_blob,
        decode_res_script_results_blob, error_message
    ) VALUES (
        ?1, ?2, ?3, ?4,
        ?5, ?6, ?7,
        ?8, ?9, ?10,
        ?11, ?12, ?13,
        ?14, ?15, ?16,
        ?17, ?18,
        ?19, ?20
    )
    "#
}

pub fn get_update_sql() -> &'static str {
    r#"
    UPDATE traffic_records SET
        status = ?1,
        content_type = ?2,
        request_content_type = ?3,
        request_size = ?4,
        response_size = ?5,
        duration_ms = ?6,
        listener_port = ?7,
        client_app = ?8,
        client_pid = ?9,
        client_path = ?10,
        flags = ?11,
        frame_count = ?12,
        last_frame_id = ?13,
        socket_is_open = ?14,
        socket_send_count = ?15,
        socket_receive_count = ?16,
        socket_send_bytes = ?17,
        socket_receive_bytes = ?18,
        socket_frame_count = ?19,
        rule_count = ?20,
        rule_protocols = ?21,
        devtools_client_req_id = ?22
    WHERE id = ?23
    "#
}

pub fn get_update_detail_sql() -> &'static str {
    r#"
    INSERT INTO traffic_record_details (
        id, timing_blob, request_headers_blob, original_response_headers_blob,
        matched_rules_blob, request_body_ref_blob, response_body_ref_blob,
        derived_response_body_ref_blob, raw_request_body_ref_blob, raw_response_body_ref_blob,
        actual_url, actual_host, original_request_headers_blob,
        response_headers_blob, socket_status_blob, req_script_results_blob,
        res_script_results_blob, decode_req_script_results_blob,
        decode_res_script_results_blob, error_message
    ) VALUES (
        ?1, ?2, ?3, ?4,
        ?5, ?6, ?7,
        ?8, ?9, ?10,
        ?11, ?12, ?13,
        ?14, ?15, ?16,
        ?17, ?18,
        ?19, ?20
    )
    ON CONFLICT(id) DO UPDATE SET
        timing_blob = excluded.timing_blob,
        request_headers_blob = excluded.request_headers_blob,
        original_response_headers_blob = excluded.original_response_headers_blob,
        matched_rules_blob = excluded.matched_rules_blob,
        request_body_ref_blob = excluded.request_body_ref_blob,
        response_body_ref_blob = excluded.response_body_ref_blob,
        derived_response_body_ref_blob = excluded.derived_response_body_ref_blob,
        raw_request_body_ref_blob = excluded.raw_request_body_ref_blob,
        raw_response_body_ref_blob = excluded.raw_response_body_ref_blob,
        actual_url = excluded.actual_url,
        actual_host = excluded.actual_host,
        original_request_headers_blob = excluded.original_request_headers_blob,
        response_headers_blob = excluded.response_headers_blob,
        socket_status_blob = excluded.socket_status_blob,
        req_script_results_blob = excluded.req_script_results_blob,
        res_script_results_blob = excluded.res_script_results_blob,
        decode_req_script_results_blob = excluded.decode_req_script_results_blob,
        decode_res_script_results_blob = excluded.decode_res_script_results_blob,
        error_message = excluded.error_message
    "#
}
