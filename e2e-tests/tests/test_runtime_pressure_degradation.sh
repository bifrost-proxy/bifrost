#!/bin/bash
set -euo pipefail

: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"
source "${PROJECT_DIR}/e2e-tests/test_utils/assert.sh"
source "${PROJECT_DIR}/e2e-tests/test_utils/process.sh"

PROXY_PORT="${PROXY_PORT:-$(allocate_free_port)}"
UPSTREAM_PORT="${UPSTREAM_PORT:-0}"
BIFROST_BIN="${BIFROST_BIN:-${PROJECT_DIR}/target/release/bifrost}"
if [[ ! -x "$BIFROST_BIN" && -x "${PROJECT_DIR}/target/debug/bifrost" ]]; then
    BIFROST_BIN="${PROJECT_DIR}/target/debug/bifrost"
fi

TEST_DATA_DIR="$(mktemp -d)"
PROXY_PID=""
UPSTREAM_PID=""

cleanup() {
    safe_cleanup_proxy "$PROXY_PID"
    if [[ -n "$UPSTREAM_PID" ]]; then
        kill_pid "$UPSTREAM_PID"
        wait_pid "$UPSTREAM_PID"
    fi
    rm -rf "$TEST_DATA_DIR"
}
trap cleanup EXIT

wait_for_admin() {
    local deadline=$((SECONDS + 45))
    while (( SECONDS < deadline )); do
        if env NO_PROXY="*" no_proxy="*" curl -fsS --max-time 2 \
            "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/auth/status" >/dev/null 2>&1; then
            return 0
        fi
        if [[ -n "$PROXY_PID" ]] && ! kill -0 "$PROXY_PID" 2>/dev/null; then
            cat "${TEST_DATA_DIR}/proxy.log" || true
            return 1
        fi
        sleep 0.2
    done
    cat "${TEST_DATA_DIR}/proxy.log" || true
    return 1
}

PYTHON_BIN="$(python3_cmd)"
UPSTREAM_PORT_FILE="${TEST_DATA_DIR}/upstream.port"
"$PYTHON_BIN" - "$UPSTREAM_PORT" "$UPSTREAM_PORT_FILE" \
    >"${TEST_DATA_DIR}/upstream.log" 2>&1 <<'PY' &
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        body = b"basic-forwarding-survived-pressure"
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *_args):
        return

server = ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), Handler)
with open(sys.argv[2], "w", encoding="utf-8") as handle:
    handle.write(str(server.server_address[1]))
    handle.flush()
server.serve_forever()
PY
UPSTREAM_PID=$!
UPSTREAM_START_TIMEOUT_SECS="${UPSTREAM_START_TIMEOUT_SECS:-60}"
UPSTREAM_START_DEADLINE=$((SECONDS + UPSTREAM_START_TIMEOUT_SECS))
while (( SECONDS < UPSTREAM_START_DEADLINE )); do
    [[ -s "$UPSTREAM_PORT_FILE" ]] && break
    if ! kill -0 "$UPSTREAM_PID" 2>/dev/null; then
        echo "[FAIL] pressure-test upstream exited before publishing its port" >&2
        cat "${TEST_DATA_DIR}/upstream.log" >&2 || true
        exit 1
    fi
    sleep 0.1
done
if [[ ! -s "$UPSTREAM_PORT_FILE" ]]; then
    echo "[FAIL] pressure-test upstream did not publish its port" >&2
    cat "${TEST_DATA_DIR}/upstream.log" >&2 || true
    exit 1
fi
UPSTREAM_PORT="$(<"$UPSTREAM_PORT_FILE")"
if ! wait_for_http_ready "http://127.0.0.1:${UPSTREAM_PORT}/pressure" 30 0.1; then
    echo "[FAIL] pressure-test upstream did not become ready" >&2
    cat "${TEST_DATA_DIR}/upstream.log" >&2 || true
    exit 1
fi

export BIFROST_DATA_DIR="$TEST_DATA_DIR"
export BIFROST_RESOURCE_PRESSURE_OVERRIDE=critical
"$BIFROST_BIN" -H 127.0.0.1 -p "$PROXY_PORT" start \
    --yes --skip-cert-check --no-system-proxy \
    >"${TEST_DATA_DIR}/proxy.log" 2>&1 &
PROXY_PID=$!
wait_for_admin

HEALTH_PORT="$("$PYTHON_BIN" - "${TEST_DATA_DIR}/runtime.json" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as handle:
    print(json.load(handle)["health_port"])
PY
)"

HEALTH_JSON="$(env NO_PROXY="*" no_proxy="*" curl -fsS \
    "http://127.0.0.1:${HEALTH_PORT}/health")"
assert_json_field ".pressure" "critical" "$HEALTH_JSON" \
    "dedicated health lane reports forced critical pressure"

CANARY_STATUS="$(env NO_PROXY="" no_proxy="" curl -sS -o /dev/null -w '%{http_code}' \
    --proxy "http://127.0.0.1:${PROXY_PORT}" \
    "http://bifrost-runtime-canary.invalid/__bifrost_runtime_canary")"
assert_equals "204" "$CANARY_STATUS" "data-plane canary remains available"

HEAVY_STATUS="$(env NO_PROXY="*" no_proxy="*" curl -sS -o /dev/null -w '%{http_code}' \
    "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/traffic")"
assert_equals "503" "$HEAVY_STATUS" "large traffic query is rejected under pressure"

LIGHT_STATUS="$(env NO_PROXY="*" no_proxy="*" curl -sS -o /dev/null -w '%{http_code}' \
    "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/proxy/system/support")"
assert_equals "200" "$LIGHT_STATUS" "lightweight Admin health remains available"

FORWARDED="$(env NO_PROXY="" no_proxy="" curl -fsS \
    --proxy "http://127.0.0.1:${PROXY_PORT}" \
    "http://127.0.0.1:${UPSTREAM_PORT}/pressure")"
assert_equals "basic-forwarding-survived-pressure" "$FORWARDED" \
    "basic forwarding remains available under pressure"

REPLAY_RESPONSE="$(env NO_PROXY="*" no_proxy="*" curl -fsS \
    -X POST \
    -H "Content-Type: application/json" \
    -d "$(jq -nc --arg url "http://127.0.0.1:${UPSTREAM_PORT}/replay-pressure" \
        '{url:$url,method:"GET",headers:[],rule_config:{mode:"none"},timeout_ms:5000}')" \
    "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/replay/execute/unified")"
assert_json_field ".success" "true" "$REPLAY_RESPONSE" \
    "interactive Replay send remains available under pressure"
assert_json_field ".data.status" "200" "$REPLAY_RESPONSE" \
    "Replay receives the upstream response under pressure"
assert_json_field ".data.body" "basic-forwarding-survived-pressure" "$REPLAY_RESPONSE" \
    "Replay preserves the upstream response body under pressure"

BODY_FILE_COUNT="$(find "${TEST_DATA_DIR}/body_cache" -type f 2>/dev/null | wc -l | tr -d ' ')"
assert_equals "0" "$BODY_FILE_COUNT" "Body payload persistence is paused"

DOCTOR_JSON="$(env BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" \
    system-proxy doctor --format json)"
assert_json_field ".health.pressure" "critical" "$DOCTOR_JSON" \
    "doctor includes runtime pressure snapshot"

[[ -s "${TEST_DATA_DIR}/system_proxy_owner_state.json" ]]
[[ -s "${TEST_DATA_DIR}/logs/system_proxy_events.jsonl" ]]

echo "[PASS] runtime pressure degradation and diagnostics E2E"
