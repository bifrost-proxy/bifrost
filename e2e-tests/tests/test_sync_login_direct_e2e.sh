#!/bin/bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

set -uo pipefail

unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY all_proxy ALL_PROXY no_proxy NO_PROXY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

source "$ROOT_DIR/e2e-tests/test_utils/assert.sh"
source "$ROOT_DIR/e2e-tests/test_utils/admin_client.sh"
source "$ROOT_DIR/e2e-tests/test_utils/default_remote.sh"

DEFAULT_REMOTE_BASE_URL="$(bifrost_default_remote_base_url)" || exit 1
DEFAULT_TOKEN_LOGIN_URL="${DEFAULT_REMOTE_BASE_URL}/v4/sso/token-login"

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
: "${BIFROST_BIN:=$ROOT_DIR/target/release/bifrost}"
if [[ "${SKIP_BUILD:-false}" == "true" && -x "$BIFROST_BIN" ]]; then
    log "Reusing pre-built bifrost binary at $BIFROST_BIN"
else
    log "Building release bifrost binary from current checkout"
    (cd "$ROOT_DIR" && cargo build --release --bin bifrost) || exit 1
    BIFROST_BIN="$ROOT_DIR/target/release/bifrost"
fi
start_mock_sync_server || exit 1
admin_start_bifrost || exit 1

log "Help should explain where to get a headless login token"
HELP_OUTPUT="$("$BIFROST_BIN" sync login --help 2>&1)"
assert_body_contains "$DEFAULT_TOKEN_LOGIN_URL" "$HELP_OUTPUT" "sync login help should include token login URL" || exit 1

log "Top-level login help should match sync login semantics"
TOP_LEVEL_HELP_OUTPUT="$("$BIFROST_BIN" login --help 2>&1)"
assert_body_contains "Equivalent to \`bifrost sync login\`" "$TOP_LEVEL_HELP_OUTPUT" "top-level login help should explain sync login equivalence" || exit 1
assert_body_contains "$DEFAULT_TOKEN_LOGIN_URL" "$TOP_LEVEL_HELP_OUTPUT" "top-level login help should include token login URL" || exit 1

log "Missing --token value should explain the default token login URL"
MISSING_DEFAULT_OUTPUT="$(CI=1 BIFROST_DATA_DIR="$BIFROST_DATA_DIR" "$BIFROST_BIN" -p "$ADMIN_PORT" sync login --token 2>&1)"
MISSING_DEFAULT_STATUS=$?
assert_equals "1" "$MISSING_DEFAULT_STATUS" "missing token value should fail before login" || exit 1
assert_body_contains "--token must not be empty" "$MISSING_DEFAULT_OUTPUT" "missing token error should stay explicit" || exit 1
assert_body_contains "Sync session token for non-interactive login; get one at ${DEFAULT_TOKEN_LOGIN_URL}" "$MISSING_DEFAULT_OUTPUT" "missing token error should include default token login URL" || exit 1

log "Top-level login missing --token value should explain the default token login URL"
TOP_LEVEL_MISSING_DEFAULT_OUTPUT="$(CI=1 BIFROST_DATA_DIR="$BIFROST_DATA_DIR" "$BIFROST_BIN" -p "$ADMIN_PORT" login --token 2>&1)"
TOP_LEVEL_MISSING_DEFAULT_STATUS=$?
assert_equals "1" "$TOP_LEVEL_MISSING_DEFAULT_STATUS" "top-level missing token value should fail before login" || exit 1
assert_body_contains "--token must not be empty" "$TOP_LEVEL_MISSING_DEFAULT_OUTPUT" "top-level missing token error should stay explicit" || exit 1
assert_body_contains "Sync session token for non-interactive login; get one at ${DEFAULT_TOKEN_LOGIN_URL}" "$TOP_LEVEL_MISSING_DEFAULT_OUTPUT" "top-level missing token error should include default token login URL" || exit 1

log "Missing --token value with explicit url should explain the custom token login URL"
MISSING_CUSTOM_OUTPUT="$(CI=1 BIFROST_DATA_DIR="$BIFROST_DATA_DIR" "$BIFROST_BIN" -p "$ADMIN_PORT" sync login --token --url "$MOCK_URL" 2>&1)"
MISSING_CUSTOM_STATUS=$?
assert_equals "1" "$MISSING_CUSTOM_STATUS" "missing token value with custom url should fail before login" || exit 1
assert_body_contains "--token must not be empty" "$MISSING_CUSTOM_OUTPUT" "custom missing token error should stay explicit" || exit 1
assert_body_contains "Sync session token for non-interactive login; get one at ${MOCK_URL}/v4/sso/token-login" "$MISSING_CUSTOM_OUTPUT" "missing token error should include custom token login URL" || exit 1

log "Top-level login missing --token value with explicit url should explain the custom token login URL"
TOP_LEVEL_MISSING_CUSTOM_OUTPUT="$(CI=1 BIFROST_DATA_DIR="$BIFROST_DATA_DIR" "$BIFROST_BIN" -p "$ADMIN_PORT" login --token --url "$MOCK_URL" 2>&1)"
TOP_LEVEL_MISSING_CUSTOM_STATUS=$?
assert_equals "1" "$TOP_LEVEL_MISSING_CUSTOM_STATUS" "top-level missing token value with custom url should fail before login" || exit 1
assert_body_contains "--token must not be empty" "$TOP_LEVEL_MISSING_CUSTOM_OUTPUT" "top-level custom missing token error should stay explicit" || exit 1
assert_body_contains "Sync session token for non-interactive login; get one at ${MOCK_URL}/v4/sso/token-login" "$TOP_LEVEL_MISSING_CUSTOM_OUTPUT" "top-level missing token error should include custom token login URL" || exit 1

log "Point sync config at mock server for CI-safe authorization"
CONFIG_OUTPUT="$(CI=1 BIFROST_DATA_DIR="$BIFROST_DATA_DIR" "$BIFROST_BIN" -p "$ADMIN_PORT" sync config --remote-url "$MOCK_URL" 2>&1)"
assert_body_contains "Sync configuration updated" "$CONFIG_OUTPUT" "CLI sync config should update remote URL" || exit 1
assert_body_contains "Remote URL: ${MOCK_URL}" "$CONFIG_OUTPUT" "CLI sync config should report mock URL" || exit 1

log "Login with token only via configured HTTP URL should fail closed"
set +e
LOGIN_OUTPUT="$(CI=1 BIFROST_DATA_DIR="$BIFROST_DATA_DIR" "$BIFROST_BIN" -p "$ADMIN_PORT" sync login --token ci-token 2>&1)"
LOGIN_STATUS=$?
set -e
assert_equals "1" "$LOGIN_STATUS" "CLI direct login with configured HTTP URL should fail" || exit 1
assert_body_contains "Failed to connect to Bifrost admin API" "$LOGIN_OUTPUT" "CLI token-only login should fail through admin API" || exit 1

log "Login with explicit HTTP token URL via CLI should fail closed"
set +e
LOGIN_EXPLICIT_OUTPUT="$(CI=1 BIFROST_DATA_DIR="$BIFROST_DATA_DIR" "$BIFROST_BIN" -p "$ADMIN_PORT" sync login --token ci-token --url "$MOCK_URL" 2>&1)"
LOGIN_EXPLICIT_STATUS=$?
set -e
assert_equals "1" "$LOGIN_EXPLICIT_STATUS" "explicit direct login with HTTP URL should fail" || exit 1
assert_body_contains "Failed to connect to Bifrost admin API" "$LOGIN_EXPLICIT_OUTPUT" "explicit direct login should fail through admin API" || exit 1

log "Top-level login with explicit HTTP token URL should fail closed"
set +e
TOP_LEVEL_LOGIN_OUTPUT="$(CI=1 BIFROST_DATA_DIR="$BIFROST_DATA_DIR" "$BIFROST_BIN" -p "$ADMIN_PORT" login --token ci-token --url "$MOCK_URL" 2>&1)"
TOP_LEVEL_LOGIN_STATUS=$?
set -e
assert_equals "1" "$TOP_LEVEL_LOGIN_STATUS" "top-level direct login with HTTP URL should fail" || exit 1
assert_body_contains "Failed to connect to Bifrost admin API" "$TOP_LEVEL_LOGIN_OUTPUT" "top-level direct login should fail through admin API" || exit 1

log "Token-only direct-login API payload should use configured default URL and reject HTTP"
TOKEN_ONLY_STATUS="$(curl -sS -o "${TEST_ROOT}/token-only.json" -w "%{http_code}" \
    -X POST "$(admin_base_url)/api/sync/login" \
    -H "content-type: application/json" \
    -d '{"token":"ci-token"}')"
assert_equals "400" "$TOKEN_ONLY_STATUS" "missing remote_base_url should use configured HTTP default and fail" || exit 1
assert_body_contains "remote_base_url must start with https://" "$(cat "${TEST_ROOT}/token-only.json")" "token-only API should reject configured HTTP URL" || exit 1

log "URL-only direct-login payload should still fail with 400"
URL_ONLY_STATUS="$(curl -sS -o "${TEST_ROOT}/url-only.json" -w "%{http_code}" \
    -X POST "$(admin_base_url)/api/sync/login" \
    -H "content-type: application/json" \
    -d '{"remote_base_url":"'"$MOCK_URL"'"}')"
assert_equals "400" "$URL_ONLY_STATUS" "missing token should return 400" || exit 1
assert_body_contains "token is required" "$(cat "${TEST_ROOT}/url-only.json")" "url-only payload error should be explicit" || exit 1

log "Login with token only should use built-in default provider URL on fresh default config"
admin_cleanup_bifrost
export ADMIN_PORT="$(pick_free_port)"
export BIFROST_DATA_DIR="${TEST_ROOT}/bifrost-default-data"
admin_start_bifrost || exit 1
LOGIN_DEFAULT_OUTPUT="$(CI=1 BIFROST_DATA_DIR="$BIFROST_DATA_DIR" "$BIFROST_BIN" -p "$ADMIN_PORT" sync login --token ci-token-default 2>&1)"
assert_body_contains "Login successful" "$LOGIN_DEFAULT_OUTPUT" "CLI token-only login should save token" || exit 1
DEFAULT_STATUS="$(admin_get "/api/sync/status")"
assert_body_contains "\"remote_base_url\":\"${DEFAULT_REMOTE_BASE_URL}\"" "$DEFAULT_STATUS" "CLI token-only login should keep built-in default URL" || exit 1

log "PASS"
