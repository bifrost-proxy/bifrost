#!/bin/bash
set -uo pipefail

unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY all_proxy ALL_PROXY no_proxy NO_PROXY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

source "$ROOT_DIR/e2e-tests/test_utils/assert.sh"

pick_free_port() {
    python3 - <<'PY'
import socket
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "Missing required command: $1" >&2
        exit 1
    }
}

require_cmd python3
require_cmd cargo

TEST_ROOT=""
SERVER_LOG_DIR=""
declare -a SERVER_PIDS=()
RUNTIME_STATUS_PID=""

RUNTIME_PORT="$(pick_free_port)"
RUNTIME_URL="http://127.0.0.1:${RUNTIME_PORT}"

EXPLICIT_PORT="$(pick_free_port)"
EXPLICIT_URL="http://127.0.0.1:${EXPLICIT_PORT}"

RUNTIME_RELAY_PORT="$(pick_free_port)"
RUNTIME_RELAY_URL="http://127.0.0.1:${RUNTIME_RELAY_PORT}"

CONFIG_RELAY_PORT="$(pick_free_port)"
CONFIG_RELAY_URL="http://127.0.0.1:${CONFIG_RELAY_PORT}"

cleanup() {
    for pid in "${SERVER_PIDS[@]}"; do
        if kill -0 "$pid" 2>/dev/null; then
            kill "$pid" 2>/dev/null || true
            wait "$pid" 2>/dev/null || true
        fi
    done
    [[ -n "$TEST_ROOT" ]] && rm -rf "$TEST_ROOT" 2>/dev/null || true
}
trap cleanup EXIT

log() { echo "[remote-relay-fallback-e2e] $*"; }

wait_for_server() {
    local url="$1"
    local pid="$2"
    local log_file="$3"
    local waited=0
    while [[ $waited -lt 50 ]]; do
        if curl -sS "${url}/healthz" >/dev/null 2>&1; then
            return 0
        fi
        if ! kill -0 "$pid" 2>/dev/null; then
            echo "mock server exited early: $url" >&2
            [[ -f "$log_file" ]] && cat "$log_file" >&2
            exit 1
        fi
        sleep 0.2
        waited=$((waited + 1))
    done
    echo "mock server did not become ready: $url" >&2
    [[ -f "$log_file" ]] && cat "$log_file" >&2
    exit 1
}

start_mock_relay() {
    local port="$1"
    local label="$2"
    local log_file="$3"

    python3 -u - "$port" "$label" >"$log_file" 2>&1 <<'PY' &
import json
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port = int(sys.argv[1])
label = sys.argv[2]

pairings = {}

class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        return

    def _write_json(self, status, payload):
        body = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path == "/healthz":
            self._write_json(200, {"ok": True})
            return

        if self.path.startswith("/v4/remote-invoke/pairings/") and self.path.endswith("/watch"):
            pairing_id = self.path.split("/")[4]
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Cache-Control", "no-cache")
            self.end_headers()
            self.wfile.write(b"event: connected\n")
            self.wfile.write(f"data: {json.dumps({'pairing_id': pairing_id})}\n\n".encode())
            self.wfile.flush()
            time.sleep(0.05)
            payload = {
                "status": "approved",
                "grant_id": f"grant-{label}",
                "client_instance_id": f"client-{label}-123456",
                "device_name": f"{label}-device",
                "platform": "macos",
                "grant_mode": "permanent",
                "pair_code": pairings.get(pairing_id, ""),
            }
            self.wfile.write(b"event: approved\n")
            self.wfile.write(f"data: {json.dumps(payload)}\n\n".encode())
            self.wfile.flush()
            return

        self._write_json(404, {"code": 404, "message": "not_found", "data": None})

    def do_POST(self):
        if self.path != "/v4/remote-invoke/pairings/start":
            self._write_json(404, {"code": 404, "message": "not_found", "data": None})
            return

        content_length = int(self.headers.get("Content-Length", "0"))
        raw = self.rfile.read(content_length or 0)
        body = json.loads(raw.decode() or "{}")
        pair_code = body.get("pair_code", "")
        pairing_id = f"{label}-pairing-{pair_code}"
        pairings[pairing_id] = pair_code
        print(f"relay={label} pair_code={pair_code}", flush=True)
        self._write_json(
            200,
            {
                "code": 0,
                "message": "ok",
                "data": {
                    "pairing_id": pairing_id,
                    "approval_sse_url": f"/v4/remote-invoke/pairings/{pairing_id}/watch",
                },
            },
        )

server = ThreadingHTTPServer(("127.0.0.1", port), Handler)
server.serve_forever()
PY
    local pid=$!
    SERVER_PIDS+=("$pid")
    wait_for_server "http://127.0.0.1:${port}" "$pid" "$log_file"
}

start_runtime_status_server() {
    local port="$1"
    local relay_url="$2"
    local log_file="$3"

    python3 -u - "$port" "$relay_url" >"$log_file" 2>&1 <<'PY' &
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port = int(sys.argv[1])
relay_url = sys.argv[2]

status_payload = {
    "enabled": True,
    "auto_sync": True,
    "remote_base_url": relay_url,
    "has_session": False,
    "reachable": True,
    "authorized": False,
    "syncing": False,
    "reason": "mock_runtime",
    "last_sync_at": None,
    "last_sync_action": None,
    "last_error": None,
    "user": None,
}

class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        return

    def _write_json(self, status, payload):
        body = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path == "/healthz":
            self._write_json(200, {"ok": True})
            return
        if self.path == "/_bifrost/api/sync/status":
            self._write_json(200, status_payload)
            return
        self._write_json(404, {"error": "not_found"})

server = ThreadingHTTPServer(("127.0.0.1", port), Handler)
server.serve_forever()
PY
    local pid=$!
    RUNTIME_STATUS_PID="$pid"
    SERVER_PIDS+=("$pid")
    wait_for_server "http://127.0.0.1:${port}" "$pid" "$log_file"
}

assert_request_count() {
    local label="$1"
    local expected="$2"
    local log_file="$3"
    local actual
    actual="$(grep -c "relay=${label} " "$log_file" || true)"
    assert_body_equals "$expected" "$actual" "${label} relay request count should be ${expected}" || exit 1
}

assert_connections_file_has() {
    local data_dir="$1"
    local expected="$2"
    local connections_file="$data_dir/remote-connections.json"
    [[ -f "$connections_file" ]] || {
        echo "missing connections file: $connections_file" >&2
        exit 1
    }
    assert_body_contains "$expected" "$(cat "$connections_file")" "connections file should contain ${expected}" || exit 1
}

run_connect() {
    local data_dir="$1"
    shift
    BIFROST_DATA_DIR="$data_dir" "$BIFROST_BIN" "$@" 2>&1
}

log "Build bifrost (release)..."
(cd "$ROOT_DIR" && cargo build --release --bin bifrost >/dev/null 2>&1)

BIFROST_BIN="$ROOT_DIR/target/release/bifrost"
if [[ ! -x "$BIFROST_BIN" ]]; then
    echo "bifrost binary not found at $BIFROST_BIN" >&2
    exit 1
fi

TEST_ROOT="$(mktemp -d "${ROOT_DIR}/.bifrost-e2e-remote-relay-fallback.XXXXXX")"
SERVER_LOG_DIR="$TEST_ROOT/server-logs"
mkdir -p "$SERVER_LOG_DIR"

EXPLICIT_LOG="$SERVER_LOG_DIR/relay-explicit.log"
RUNTIME_RELAY_LOG="$SERVER_LOG_DIR/relay-runtime.log"
CONFIG_RELAY_LOG="$SERVER_LOG_DIR/relay-config.log"
RUNTIME_STATUS_LOG="$SERVER_LOG_DIR/runtime-status.log"

start_mock_relay "$EXPLICIT_PORT" "explicit" "$EXPLICIT_LOG"
start_mock_relay "$RUNTIME_RELAY_PORT" "runtime" "$RUNTIME_RELAY_LOG"
start_mock_relay "$CONFIG_RELAY_PORT" "config" "$CONFIG_RELAY_LOG"
start_runtime_status_server "$RUNTIME_PORT" "$RUNTIME_RELAY_URL" "$RUNTIME_STATUS_LOG"

CASE1_DIR="$TEST_ROOT/case1"
mkdir -p "$CASE1_DIR"

log "Case 1: explicit --relay-url should override running sync config"
CASE1_OUTPUT="$(run_connect "$CASE1_DIR" --port "$RUNTIME_PORT" remote connect 881001 --relay-url "$EXPLICIT_URL")"
CASE1_EXIT=$?
assert_status "0" "$CASE1_EXIT" "explicit relay-url should succeed" || exit 1
assert_body_contains "Connected! Authorization granted" "$CASE1_OUTPUT" "explicit relay-url should connect successfully" || exit 1
assert_connections_file_has "$CASE1_DIR" "$EXPLICIT_URL"
assert_connections_file_has "$CASE1_DIR" "client-explicit-123456"
assert_request_count "explicit" "1" "$EXPLICIT_LOG"
assert_request_count "runtime" "0" "$RUNTIME_RELAY_LOG"
assert_request_count "config" "0" "$CONFIG_RELAY_LOG"

CASE2_DIR="$TEST_ROOT/case2"
mkdir -p "$CASE2_DIR"

log "Case 2: running sync config should be used when --relay-url is omitted"
CASE2_OUTPUT="$(run_connect "$CASE2_DIR" --port "$RUNTIME_PORT" remote connect 881002)"
CASE2_EXIT=$?
assert_status "0" "$CASE2_EXIT" "runtime sync relay-url should succeed" || exit 1
assert_body_contains "Connected! Authorization granted" "$CASE2_OUTPUT" "runtime sync relay-url should connect successfully" || exit 1
assert_connections_file_has "$CASE2_DIR" "$RUNTIME_RELAY_URL"
assert_connections_file_has "$CASE2_DIR" "client-runtime-123456"
assert_request_count "explicit" "1" "$EXPLICIT_LOG"
assert_request_count "runtime" "1" "$RUNTIME_RELAY_LOG"
assert_request_count "config" "0" "$CONFIG_RELAY_LOG"

if [[ -n "$RUNTIME_STATUS_PID" ]] && kill -0 "$RUNTIME_STATUS_PID" 2>/dev/null; then
    kill "$RUNTIME_STATUS_PID" 2>/dev/null || true
    wait "$RUNTIME_STATUS_PID" 2>/dev/null || true
fi
RUNTIME_STATUS_PID=""

CASE3_DIR="$TEST_ROOT/case3"
mkdir -p "$CASE3_DIR"
cat >"$CASE3_DIR/config.toml" <<EOF
[sync]
remote_base_url = "${CONFIG_RELAY_URL}"
EOF

log "Case 3: local config should be used when runtime sync config is unavailable"
CASE3_OUTPUT="$(run_connect "$CASE3_DIR" --port "$RUNTIME_PORT" remote connect 881003)"
CASE3_EXIT=$?
assert_status "0" "$CASE3_EXIT" "configured relay-url should succeed" || exit 1
assert_body_contains "Connected! Authorization granted" "$CASE3_OUTPUT" "configured relay-url should connect successfully" || exit 1
assert_connections_file_has "$CASE3_DIR" "$CONFIG_RELAY_URL"
assert_connections_file_has "$CASE3_DIR" "client-config-123456"
assert_request_count "explicit" "1" "$EXPLICIT_LOG"
assert_request_count "runtime" "1" "$RUNTIME_RELAY_LOG"
assert_request_count "config" "1" "$CONFIG_RELAY_LOG"

if [[ "$FAILED_ASSERTIONS" -gt 0 ]]; then
    exit 1
fi

echo "All remote relay URL fallback assertions passed."
