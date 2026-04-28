#!/bin/bash
set -uo pipefail

unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY all_proxy ALL_PROXY no_proxy NO_PROXY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

source "$ROOT_DIR/e2e-tests/test_utils/assert.sh"
source "$ROOT_DIR/e2e-tests/test_utils/admin_client.sh"

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "Missing required command: $1" >&2
        exit 1
    }
}

pick_free_port() {
    python3 - <<'PY'
import socket
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

require_cmd curl
require_cmd jq
require_cmd python3

TEST_ROOT="$(mktemp -d)"
MOCK_PORT="$(pick_free_port)"
MOCK_URL="http://127.0.0.1:${MOCK_PORT}"
export ADMIN_PORT="$(pick_free_port)"
export BIFROST_DATA_DIR="${TEST_ROOT}/bifrost-data"
MOCK_PID=""

cleanup() {
    admin_cleanup_bifrost
    if [[ -n "$MOCK_PID" ]] && kill -0 "$MOCK_PID" 2>/dev/null; then
        kill "$MOCK_PID" 2>/dev/null || true
        wait "$MOCK_PID" 2>/dev/null || true
    fi
    rm -rf "$TEST_ROOT" 2>/dev/null || true
}
trap cleanup EXIT

log() { echo "[sync-login-direct-e2e] $*"; }

start_mock_sync_server() {
    local server_py="${TEST_ROOT}/sync_server.py"
    cat > "$server_py" <<'PY'
import json
import os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(os.environ["MOCK_PORT"])

class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        return

    def _json(self, status, payload):
        body = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path == "/healthz":
            self._json(200, {"ok": True})
            return
        if self.path == "/v4/sso/check":
            self._json(200, {"code": 0, "message": "ok", "data": {}})
            return
        if self.path == "/v4/sso/info":
            token = self.headers.get("x-bifrost-token", "")
            if token != "ci-token":
                self._json(401, {"code": 401, "message": "unauthorized"})
                return
            self._json(200, {
                "code": 0,
                "message": "ok",
                "data": {
                    "user_id": "ci-user",
                    "nickname": "CI User",
                    "avatar": "",
                    "email": "ci@example.test"
                }
            })
            return
        self._json(404, {"code": 404, "message": "not found"})

ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
PY
    MOCK_PORT="$MOCK_PORT" python3 "$server_py" >"${TEST_ROOT}/sync-server.log" 2>&1 &
    MOCK_PID=$!
    for _ in $(seq 1 50); do
        if curl -sS "${MOCK_URL}/healthz" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.2
    done
    echo "mock sync server did not become ready" >&2
    cat "${TEST_ROOT}/sync-server.log" >&2 || true
    return 1
}

wait_for_authorized() {
    local status
    for _ in $(seq 1 30); do
        status="$(admin_get "/api/sync/status")"
        if echo "$status" | jq -e '.has_session == true and .authorized == true and .user.user_id == "ci-user"' >/dev/null; then
            echo "$status"
            return 0
        fi
        sleep 0.5
    done
    admin_get "/api/sync/status"
    return 1
}

log "Starting isolated mock sync server and Bifrost admin"
BIFROST_BIN="$ROOT_DIR/target/release/bifrost"
log "Building release bifrost binary"
(cd "$ROOT_DIR" && cargo build --release --bin bifrost) || exit 1
start_mock_sync_server || exit 1
admin_start_bifrost || exit 1

log "Login with token and url via CLI"
LOGIN_OUTPUT="$(CI=1 BIFROST_DATA_DIR="$BIFROST_DATA_DIR" "$BIFROST_BIN" -p "$ADMIN_PORT" sync login --token ci-token --url "$MOCK_URL" 2>&1)"
assert_body_contains "Login token saved" "$LOGIN_OUTPUT" "CLI direct login should save token" || exit 1
assert_body_contains "Remote URL: ${MOCK_URL}" "$LOGIN_OUTPUT" "CLI direct login should report remote url" || exit 1

log "Wait until background sync authorizes the token"
STATUS="$(wait_for_authorized)"
assert_body_contains '"has_session":true' "$STATUS" "status should have session" || exit 1
assert_body_contains '"authorized":true' "$STATUS" "status should be authorized" || exit 1
assert_body_contains '"remote_base_url":"'"$MOCK_URL"'"' "$STATUS" "status should persist remote url" || exit 1
assert_body_contains '"user_id":"ci-user"' "$STATUS" "status should include user from token" || exit 1

log "Partial direct-login payload should fail with 400"
PARTIAL_STATUS="$(curl -sS -o "${TEST_ROOT}/partial.json" -w "%{http_code}" \
    -X POST "$(admin_base_url)/api/sync/login" \
    -H "content-type: application/json" \
    -d '{"token":"ci-token"}')"
assert_equals "400" "$PARTIAL_STATUS" "missing remote_base_url should return 400" || exit 1
assert_body_contains "token and remote_base_url must be provided together" "$(cat "${TEST_ROOT}/partial.json")" "partial payload error should be explicit" || exit 1

log "PASS"
