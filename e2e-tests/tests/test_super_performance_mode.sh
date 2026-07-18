#!/bin/bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

source "${PROJECT_DIR}/e2e-tests/test_utils/assert.sh"
source "${PROJECT_DIR}/e2e-tests/test_utils/process.sh"

PROXY_PORT="${PROXY_PORT:-$(allocate_free_port)}"
UPSTREAM_PORT="${UPSTREAM_PORT:-$(allocate_free_port)}"
TEST_DATA_DIR=""
TEST_HOME=""
PROXY_PID=""
UPSTREAM_PID=""

BIFROST_BIN="${BIFROST_BIN:-${PROJECT_DIR}/target/release/bifrost}"
if [[ ! -x "$BIFROST_BIN" && -x "${PROJECT_DIR}/target/debug/bifrost" ]]; then
    BIFROST_BIN="${PROJECT_DIR}/target/debug/bifrost"
fi
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi

cleanup() {
    safe_cleanup_proxy "$PROXY_PID"
    if [[ -n "$UPSTREAM_PID" ]]; then
        kill "$UPSTREAM_PID" 2>/dev/null || true
        wait "$UPSTREAM_PID" 2>/dev/null || true
    fi
    if [[ -n "$TEST_DATA_DIR" && -d "$TEST_DATA_DIR" ]]; then
        rm -rf "$TEST_DATA_DIR"
    fi
    if [[ -n "$TEST_HOME" && -d "$TEST_HOME" ]]; then
        rm -rf "$TEST_HOME"
    fi
}
trap cleanup EXIT

wait_for_http() {
    local url="$1"
    local timeout_secs="${2:-30}"
    local start_ts
    start_ts="$(date +%s)"
    while true; do
        if env NO_PROXY="*" no_proxy="*" curl -fsS --connect-timeout 1 --max-time 3 "$url" >/dev/null 2>&1; then
            return 0
        fi
        if (( $(date +%s) - start_ts >= timeout_secs )); then
            return 1
        fi
        sleep 0.2
    done
}

wait_for_http_process() {
    local url="$1"
    local timeout_secs="$2"
    local pid="$3"
    local log_file="$4"
    local start_ts
    start_ts="$(date +%s)"
    while true; do
        if env NO_PROXY="*" no_proxy="*" curl -fsS --connect-timeout 1 --max-time 3 "$url" >/dev/null 2>&1; then
            return 0
        fi
        if [[ -n "$pid" ]] && ! kill -0 "$pid" 2>/dev/null; then
            echo "[FAIL] fixture process ${pid} exited before becoming ready"
            cat "$log_file" || true
            return 1
        fi
        if (( $(date +%s) - start_ts >= timeout_secs )); then
            return 1
        fi
        sleep 0.2
    done
}

start_upstream() {
    local py
    py="$(python3_cmd)"
    "$py" - "$UPSTREAM_PORT" <<'PY' >"${TEST_DATA_DIR}/upstream.log" 2>&1 &
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port = int(sys.argv[1])

class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        body = json.dumps({"ok": True, "path": self.path}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_args):
        return

ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
    UPSTREAM_PID=$!
    wait_for_http_process "http://127.0.0.1:${UPSTREAM_PORT}/ready" 45 \
        "$UPSTREAM_PID" "${TEST_DATA_DIR}/upstream.log" || {
        echo "[FAIL] upstream did not become ready"
        cat "${TEST_DATA_DIR}/upstream.log" || true
        return 1
    }
}

start_proxy() {
    if [[ ! -x "$BIFROST_BIN" ]]; then
        echo "[FAIL] bifrost binary not found at ${BIFROST_BIN}"
        return 1
    fi

    export BIFROST_DATA_DIR="$TEST_DATA_DIR"
    export BIFROST_DISABLE_TRAY=1
    export HOME="$TEST_HOME"
    export XDG_CONFIG_HOME="${TEST_DATA_DIR}/xdg-config"
    export XDG_DATA_HOME="${TEST_DATA_DIR}/xdg-data"
    mkdir -p "$XDG_CONFIG_HOME" "$XDG_DATA_HOME"

    "$BIFROST_BIN" -H 127.0.0.1 -p "$PROXY_PORT" start \
        -y \
        --access-mode allow_all \
        --skip-cert-check \
        --unsafe-ssl \
        --no-system-proxy \
        --super-performance-mode \
        --rules "127.0.0.1 resHeaders://X-Bifrost-Super-Mode=on" \
        >"${TEST_DATA_DIR}/proxy.log" 2>&1 &
    PROXY_PID=$!

    wait_for_http "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/auth/status" 45 || {
        echo "[FAIL] proxy did not become ready"
        cat "${TEST_DATA_DIR}/proxy.log" || true
        return 1
    }
}

admin_get() {
    env NO_PROXY="*" no_proxy="*" curl -fsS "http://127.0.0.1:${PROXY_PORT}/_bifrost$1"
}

admin_post() {
    local path="$1"
    local body="$2"
    env NO_PROXY="*" no_proxy="*" curl -fsS \
        -H "Content-Type: application/json" \
        -X POST \
        --data "$body" \
        "http://127.0.0.1:${PROXY_PORT}/_bifrost${path}"
}

assert_no_body_cache_files() {
    local count
    count="$(find "${TEST_DATA_DIR}/body_cache" -type f 2>/dev/null | wc -l | tr -d ' ')"
    assert_equals "0" "$count" "super performance mode should not write body cache files"
}

main() {
    TEST_DATA_DIR="$(mktemp -d)"
    TEST_HOME="$(mktemp -d)"

    start_upstream || exit 1
    start_proxy || exit 1

    local config
    config="$(admin_get "/api/config/performance")"
    assert_json_field ".traffic.super_performance_mode" "true" "$config" "performance config reports super mode"

    local headers_file
    headers_file="${TEST_DATA_DIR}/response.headers"
    local body_file
    body_file="${TEST_DATA_DIR}/response.body"
    env NO_PROXY="" no_proxy="" curl -fsS \
        --noproxy "" \
        -x "http://127.0.0.1:${PROXY_PORT}" \
        -D "$headers_file" \
        -o "$body_file" \
        "http://127.0.0.1:${UPSTREAM_PORT}/super-performance-e2e"
    assert_header_value "X-Bifrost-Super-Mode" "on" "$(cat "$headers_file")" "rules still process response headers"
    assert_body_contains "super-performance-e2e" "$(cat "$body_file")" "proxy request reaches upstream"

    sleep 1
    local traffic
    traffic="$(admin_get "/api/traffic?limit=100")"
    assert_json_field ".records | length" "0" "$traffic" "traffic list stays empty in super mode"
    assert_json_field ".total" "0" "$traffic" "traffic total stays zero in super mode"

    local query
    query="$(admin_post "/api/traffic/query" '{"limit":100,"url_contains":"super-performance-e2e"}')"
    assert_json_field ".records | length" "0" "$query" "traffic query stays empty in super mode"
    assert_json_field ".total" "0" "$query" "traffic query total stays zero in super mode"

    assert_no_body_cache_files
    print_test_summary || exit 1
}

main "$@"
