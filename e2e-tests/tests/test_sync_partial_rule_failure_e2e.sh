#!/bin/bash
export BIFROST_DISABLE_TRAY=1
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
export BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL=1
export BIFROST_SYSTEM_PROXY_DISABLE_LIFECYCLE_HELPER=1

set -euo pipefail

unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY all_proxy ALL_PROXY no_proxy NO_PROXY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

source "$ROOT_DIR/e2e-tests/test_utils/assert.sh"
source "$ROOT_DIR/e2e-tests/test_utils/admin_client.sh"

pick_free_port() {
    python3 - <<'PY'
import socket
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

TEST_ROOT="$(mktemp -d)"
MOCK_PORT="$(pick_free_port)"
MOCK_URL="http://127.0.0.1:${MOCK_PORT}"
export ADMIN_PORT="$(pick_free_port)"
export BIFROST_DATA_DIR="${TEST_ROOT}/bifrost-data"
export BIFROST_BIN="${BIFROST_BIN:-${ROOT_DIR}/target/debug/bifrost}"
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

mkdir -p "$BIFROST_DATA_DIR"
cat >"$BIFROST_DATA_DIR/config.toml" <<EOF
[sync]
enabled = true
auto_sync = false
remote_base_url = "${MOCK_URL}"
probe_interval_secs = 5
connect_timeout_ms = 3000
EOF
cat >"$BIFROST_DATA_DIR/sync-state.json" <<'JSON'
{"token":"test-token","deleted_rules":{},"basic_configs":{}}
JSON

cat >"$TEST_ROOT/sync_server.py" <<'PY'
import json
import os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(os.environ["MOCK_PORT"])

class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        return

    def send_json(self, status, payload):
        body = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path == "/healthz":
            self.send_json(200, {"ok": True})
        elif self.path == "/v4/sso/check":
            self.send_json(200, {"code": 0, "message": "ok", "data": {}})
        elif self.path == "/v4/sso/info":
            self.send_json(200, {
                "code": 0,
                "message": "ok",
                "data": {
                    "user_id": "user-1",
                    "nickname": "Test User",
                    "avatar": "",
                    "email": "test@example.test"
                }
            })
        elif self.path.startswith("/v4/env"):
            self.send_json(200, {
                "code": 0,
                "message": "ok",
                "data": {
                    "list": [{
                        "id": "env-remote-only",
                        "user_id": "user-1",
                        "name": "remote-only",
                        "rule": "remote.example.test host://127.0.0.1:3100",
                        "create_time": "2026-01-01T00:00:00Z",
                        "update_time": "2026-01-02T00:00:00Z"
                    }]
                }
            })
        else:
            self.send_json(404, {"code": 404, "message": "not found"})

    def do_POST(self):
        if self.path == "/v4/env":
            self.send_json(404, {
                "code": -10002,
                "message": "Validation error: Validation not on name failed",
                "data": None
            })
        else:
            self.send_json(404, {"code": 404, "message": "not found"})

ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
PY
MOCK_PORT="$MOCK_PORT" python3 "$TEST_ROOT/sync_server.py" >"$TEST_ROOT/sync-server.log" 2>&1 &
MOCK_PID=$!
for _ in $(seq 1 50); do
    curl -fsS "$MOCK_URL/healthz" >/dev/null 2>&1 && break
    sleep 0.2
done
curl -fsS "$MOCK_URL/healthz" >/dev/null

if [[ "${SKIP_BUILD:-false}" != "true" ]]; then
    (cd "$ROOT_DIR" && cargo build --bin bifrost)
fi
admin_start_bifrost

INVALID_NAME="share/task - owner (production)"
CREATE_PAYLOAD="$(jq -cn --arg name "$INVALID_NAME" '{name:$name,content:"bad.example.test host://127.0.0.1:3000",enabled:true}')"
admin_post "/api/rules" "$CREATE_PAYLOAD" >/dev/null
admin_put "/api/sync/config" '{"auto_sync":true}' >/dev/null

STATUS=""
REMOTE_RULE=""
for _ in $(seq 1 50); do
    STATUS="$(admin_get "/api/sync/status")"
    REMOTE_RULE="$(admin_get "/api/rules/remote-only" 2>/dev/null || true)"
    if echo "$STATUS" | jq -e --arg name "$INVALID_NAME" '
        .providers[]
        | select(.id == "bifrost_cloud")
        | .authorized == true
          and .reason == "error"
          and .last_changed_sync_action == "remote_pulled"
          and (.last_error | contains($name))
          and (.last_error | contains("Validation not on name failed"))
    ' >/dev/null 2>&1 && echo "$REMOTE_RULE" | jq -e '
        .name == "remote-only"
        and .sync.status == "synced"
        and .sync.remote_id == "env-remote-only"
    ' >/dev/null 2>&1; then
        break
    fi
    sleep 0.2
done

if ! echo "$STATUS" | jq -e --arg name "$INVALID_NAME" '
    .providers[]
    | select(.id == "bifrost_cloud")
    | .authorized == true
      and .reason == "error"
      and .last_changed_sync_action == "remote_pulled"
      and (.last_error | contains($name))
      and (.last_error | contains("Validation not on name failed"))
' >/dev/null; then
    echo "Unexpected sync status:" >&2
    echo "$STATUS" | jq . >&2
    exit 1
fi
if ! echo "$REMOTE_RULE" | jq -e '
    .name == "remote-only"
    and .sync.status == "synced"
    and .sync.remote_id == "env-remote-only"
' >/dev/null; then
    echo "Unexpected pulled rule:" >&2
    echo "$REMOTE_RULE" | jq . >&2 || echo "$REMOTE_RULE" >&2
    exit 1
fi
INVALID_RULE="$(admin_get "/api/rules/$(python3 -c 'import sys, urllib.parse; print(urllib.parse.quote(sys.argv[1], safe=""))' "$INVALID_NAME")")"
if ! echo "$INVALID_RULE" | jq -e '.sync.status == "local_only" and .sync.remote_id == null' >/dev/null; then
    echo "Unexpected failed local rule:" >&2
    echo "$INVALID_RULE" | jq . >&2 || echo "$INVALID_RULE" >&2
    exit 1
fi
SYNC_CLI_STATUS="$(BIFROST_DATA_DIR="$BIFROST_DATA_DIR" "$BIFROST_BIN" -p "$ADMIN_PORT" sync status)"
assert_body_contains "Reason: error" "$SYNC_CLI_STATUS" "CLI should expose provider error state" || exit 1
assert_body_contains "Last error:" "$SYNC_CLI_STATUS" "CLI should expose provider error details" || exit 1
assert_body_contains "$INVALID_NAME" "$SYNC_CLI_STATUS" "CLI provider error should identify the failed rule" || exit 1

admin_post "/api/sync/run" '{}' >/dev/null
sleep 0.5
RULES="$(admin_get "/api/rules")"
if ! echo "$RULES" | jq -e '[.[] | select(.name == "remote-only")] | length == 1' >/dev/null; then
    echo "Unexpected rule list after retry:" >&2
    echo "$RULES" | jq . >&2
    exit 1
fi

echo "PASS: failed local upload did not block or duplicate remote rule pull"
