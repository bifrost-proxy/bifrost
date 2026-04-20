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

TEST_DATA_DIR=""
SERVER_LOG=""
SERVER_PID=""
RELAY_PORT="$(pick_free_port)"
RELAY_URL="http://127.0.0.1:${RELAY_PORT}"

cleanup() {
    if [[ -n "$SERVER_PID" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
    [[ -n "$SERVER_LOG" ]] && rm -f "$SERVER_LOG" 2>/dev/null || true
    [[ -n "$TEST_DATA_DIR" ]] && rm -rf "$TEST_DATA_DIR" 2>/dev/null || true
}
trap cleanup EXIT

log() { echo "[remote-connect-overload-e2e] $*"; }

wait_for_server() {
    local waited=0
    while [[ $waited -lt 30 ]]; do
        if curl -sS "${RELAY_URL}/healthz" >/dev/null 2>&1; then
            return 0
        fi
        if ! kill -0 "$SERVER_PID" 2>/dev/null; then
            echo "mock relay exited early" >&2
            [[ -f "$SERVER_LOG" ]] && cat "$SERVER_LOG" >&2
            exit 1
        fi
        sleep 0.2
        waited=$((waited + 1))
    done
    echo "mock relay did not become ready" >&2
    [[ -f "$SERVER_LOG" ]] && cat "$SERVER_LOG" >&2
    exit 1
}

start_mock_relay() {
    SERVER_LOG="$(mktemp)"
    python3 -u - "$RELAY_PORT" >"$SERVER_LOG" 2>&1 <<'PY' &
import json
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port = int(sys.argv[1])
attempts = {}
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
            meta = pairings.get(pairing_id, {})
            payload = {
                "status": "approved",
                "grant_id": "grant-retry-ok",
                "client_instance_id": "client-retry-ok-123456",
                "device_name": "Retry Device",
                "platform": "macos",
                "grant_mode": "permanent",
                "pair_code": meta.get("pair_code", ""),
            }
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Cache-Control", "no-cache")
            self.end_headers()
            self.wfile.write(b"event: connected\n")
            self.wfile.write(f"data: {json.dumps({'pairing_id': pairing_id})}\n\n".encode())
            self.wfile.flush()
            time.sleep(0.05)
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
        attempts[pair_code] = attempts.get(pair_code, 0) + 1
        current = attempts[pair_code]
        print(f"start_pairing attempt pair_code={pair_code} attempt={current}", flush=True)

        should_overload = pair_code == "994431" or current <= 2
        if should_overload:
            payload = b"overload-protect triggered"
            self.send_response(503)
            self.send_header("Content-Type", "text/plain; charset=utf-8")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
            return

        pairing_id = f"pairing-{pair_code}"
        pairings[pairing_id] = {"pair_code": pair_code}
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
    SERVER_PID=$!
    wait_for_server
}

log "Build bifrost (release)..."
(cd "$ROOT_DIR" && cargo build --release --bin bifrost >/dev/null 2>&1)

BIFROST_BIN="$ROOT_DIR/target/release/bifrost"
if [[ ! -x "$BIFROST_BIN" ]]; then
    echo "bifrost binary not found at $BIFROST_BIN" >&2
    exit 1
fi

TEST_DATA_DIR="$(mktemp -d "${ROOT_DIR}/.bifrost-e2e-remote-connect-overload.XXXXXX")"
start_mock_relay

log "Case 1: transient overload should retry and eventually connect"
SUCCESS_OUTPUT="$(BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" remote connect 994430 --relay-url "$RELAY_URL" 2>&1)"
SUCCESS_EXIT=$?
assert_status "0" "$SUCCESS_EXIT" "transient overload 后 remote connect 应成功" || exit 1
assert_body_contains "Relay is temporarily busy, retrying pairing" "$SUCCESS_OUTPUT" "CLI 应提示正在重试" || exit 1
assert_body_contains "✓ Connected! Authorization granted" "$SUCCESS_OUTPUT" "重试后应连接成功" || exit 1

ATTEMPTS_994430="$(grep -c 'pair_code=994430' "$SERVER_LOG" || true)"
assert_body_equals "3" "$ATTEMPTS_994430" "transient overload 场景应总共请求 3 次 start_pairing" || exit 1

CONNECTIONS_FILE="$TEST_DATA_DIR/remote-connections.json"
assert_body_contains "client-retry-ok-123456" "$(cat "$CONNECTIONS_FILE")" "成功后应写入本地连接记录" || exit 1

log "Case 2: persistent overload should stop after limited retries with actionable error"
set +e
FAIL_OUTPUT="$(BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" remote connect 994431 --relay-url "$RELAY_URL" 2>&1)"
FAIL_EXIT=$?
set -e
if [[ "$FAIL_EXIT" -eq 0 ]]; then
    _log_fail "persistent overload 后 remote connect 应失败" "non-zero exit" "0"
    exit 1
fi
_log_pass "persistent overload 后 remote connect 返回非零退出码"
assert_body_contains "pairing service is temporarily busy" "$FAIL_OUTPUT" "CLI 应输出可行动的 overload 提示" || exit 1
assert_body_contains "bifrost remote connect <pair-code>" "$FAIL_OUTPUT" "CLI 应提示稍后重试 connect" || exit 1

ATTEMPTS_994431="$(grep -c 'pair_code=994431' "$SERVER_LOG" || true)"
assert_body_equals "4" "$ATTEMPTS_994431" "persistent overload 场景应限制为 4 次 start_pairing 尝试" || exit 1

if [[ "$FAILED_ASSERTIONS" -gt 0 ]]; then
    exit 1
fi

echo "All remote connect overload retry assertions passed."
