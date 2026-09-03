#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
source "$SCRIPT_DIR/../test_utils/process.sh"

ADMIN_PORT="${ADMIN_PORT:-$(allocate_free_port)}"
BIFROST_BIN="${BIFROST_BIN:-${PROJECT_ROOT}/target/debug/bifrost}"
TEST_DATA_DIR="${TEST_DATA_DIR:-$(mktemp -d "${TMPDIR:-/tmp}/bifrost-traffic-db-reset.XXXXXX")}"
PROXY_PID=""

log_info() { echo "[INFO] $*"; }
log_pass() { echo -e "\033[0;32m[PASS]\033[0m $*"; }
log_fail() { echo -e "\033[0;31m[FAIL]\033[0m $*"; }

cleanup() {
    if [[ -n "$PROXY_PID" ]]; then
        safe_cleanup_proxy "$PROXY_PID" || true
        wait_pid "$PROXY_PID" || true
        PROXY_PID=""
    fi
    kill_bifrost_on_port "$ADMIN_PORT" || true
    rm -rf "$TEST_DATA_DIR"
}
trap cleanup EXIT

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
    if [[ ! -x "$BIFROST_BIN" ]]; then
        log_fail "SKIP_BUILD=true but BIFROST_BIN is not executable: $BIFROST_BIN"
        exit 1
    fi
    log_info "Using prebuilt Bifrost binary: $BIFROST_BIN"
else
    log_info "Building current bifrost binary..."
    cargo build --bin bifrost
fi

mkdir -p "$TEST_DATA_DIR/traffic"

python3 - "$TEST_DATA_DIR/traffic/traffic.db" <<'PY'
import sqlite3
import sys

db_path = sys.argv[1]
conn = sqlite3.connect(db_path)
conn.executescript("""
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
""")
conn.commit()
conn.close()
PY

log_info "Starting Bifrost with legacy traffic DB on port ${ADMIN_PORT}..."
BIFROST_DATA_DIR="$TEST_DATA_DIR" \
    BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
    BIFROST_DISABLE_TRAY=1 \
    "$BIFROST_BIN" start -p "$ADMIN_PORT" --host 127.0.0.1 --skip-cert-check --no-system-proxy \
    >"$TEST_DATA_DIR/start.log" 2>&1 &
PROXY_PID=$!

for _ in $(seq 1 60); do
    if ! kill -0 "$PROXY_PID" 2>/dev/null; then
        log_fail "Bifrost exited before readiness"
        cat "$TEST_DATA_DIR/start.log" || true
        exit 1
    fi
    if curl -fsS "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/proxy/address" >/dev/null 2>&1; then
        break
    fi
    sleep 0.5
done

if ! curl -fsS "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/proxy/address" >/dev/null 2>&1; then
    log_fail "Bifrost did not become ready"
    cat "$TEST_DATA_DIR/start.log" || true
    exit 1
fi

python3 - "$TEST_DATA_DIR/traffic/traffic.db" <<'PY'
import sqlite3
import sys

conn = sqlite3.connect(sys.argv[1])
columns = {row[1] for row in conn.execute("PRAGMA table_info(traffic_records)")}
indexes = {
    row[0]
    for row in conn.execute(
        "SELECT name FROM sqlite_master WHERE type = 'index'"
    )
}
record_count = conn.execute("SELECT COUNT(*) FROM traffic_records").fetchone()[0]
schema_version = conn.execute(
    "SELECT value FROM metadata WHERE key = 'schema_version'"
).fetchone()[0]
conn.close()

assert "devtools_client_req_id" in columns, columns
assert "idx_devtools_client_req_id" in indexes, indexes
assert record_count == 0, record_count
assert schema_version == "15", schema_version
PY

log_pass "Legacy traffic DB was reset and Bifrost started successfully"
