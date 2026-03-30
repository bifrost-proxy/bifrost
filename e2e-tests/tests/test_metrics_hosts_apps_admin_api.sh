#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TEST_ID="${TEST_ID:-}"
export TEST_ID
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
BIFROST_BIN="${ROOT_DIR}/target/release/bifrost"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi
source "$SCRIPT_DIR/../test_utils/http_client.sh"
source "$SCRIPT_DIR/../test_utils/process.sh"

PROXY_HOST="${PROXY_HOST:-127.0.0.1}"
PROXY_PORT="${PROXY_PORT:-18990}"
ADMIN_HOST="${ADMIN_HOST:-127.0.0.1}"
ADMIN_PORT="${ADMIN_PORT:-$PROXY_PORT}"
ADMIN_PATH_PREFIX="${ADMIN_PATH_PREFIX:-/_bifrost}"
ECHO_HTTP_PORT="${ECHO_HTTP_PORT:-3000}"

BIFROST_PID=""
MOCK_HTTP_PID=""
BIFROST_DATA_DIR=""
BIFROST_LOG_FILE=""

log_info() { echo "[INFO] $*"; }
log_pass() { echo "[PASS] $*"; }
log_fail() { echo "[FAIL] $*"; }

cleanup() {
    kill_bifrost_on_port "$PROXY_PORT"

    safe_cleanup_proxy "$BIFROST_PID"

    if [[ -n "$MOCK_HTTP_PID" ]] && kill -0 "$MOCK_HTTP_PID" 2>/dev/null; then
        kill_pid "$MOCK_HTTP_PID"
        wait_pid "$MOCK_HTTP_PID"
    fi

    if [[ -n "$BIFROST_DATA_DIR" && -d "$BIFROST_DATA_DIR" ]]; then
        rm -rf "$BIFROST_DATA_DIR" || true
    fi

    if [[ -n "$BIFROST_LOG_FILE" && -f "$BIFROST_LOG_FILE" ]]; then
        rm -f "$BIFROST_LOG_FILE" || true
    fi
}

trap cleanup EXIT

start_mock_http() {
    local port="${1:-3000}"
    python3 "$SCRIPT_DIR/../mock_servers/http_echo_server.py" "$port" >/dev/null 2>&1 &
    MOCK_HTTP_PID=$!
    sleep 0.5
    if ! kill -0 "$MOCK_HTTP_PID" 2>/dev/null; then
        log_fail "Failed to start mock HTTP server"
        return 1
    fi
    return 0
}

start_bifrost() {
    BIFROST_DATA_DIR=$(mktemp -d)
    export BIFROST_DATA_DIR
    BIFROST_LOG_FILE=$(mktemp)

    local repo_dir
    repo_dir="$(cd "$SCRIPT_DIR/../.." && pwd)"
    cd "$repo_dir" || return 1

    SKIP_FRONTEND_BUILD=1 BIFROST_DATA_DIR="$BIFROST_DATA_DIR" \
        "$BIFROST_BIN" -p "$PROXY_PORT" start --skip-cert-check >"$BIFROST_LOG_FILE" 2>&1 &
    BIFROST_PID=$!

    local max_wait=180
    local waited=0
    while [[ $waited -lt $max_wait ]]; do
        if [[ -n "$BIFROST_PID" ]] && ! kill -0 "$BIFROST_PID" 2>/dev/null; then
            tail -n 200 "$BIFROST_LOG_FILE" 2>/dev/null || true
            return 1
        fi
        if curl -s "http://${ADMIN_HOST}:${ADMIN_PORT}${ADMIN_PATH_PREFIX}/api/system" >/dev/null 2>&1; then
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done
    tail -n 200 "$BIFROST_LOG_FILE" 2>/dev/null || true
    return 1
}

generate_http_traffic() {
    local url="http://${PROXY_HOST}:${ECHO_HTTP_PORT}/test"
    for _ in $(seq 1 3); do
        http_get "$url"
    done
    sleep 0.5
}

assert_hosts_metrics() {
    local resp
    resp=$(curl -s "http://${ADMIN_HOST}:${ADMIN_PORT}${ADMIN_PATH_PREFIX}/api/metrics/hosts")
    local cnt
    cnt=$(echo "$resp" | jq '[.[] | select(.host == "127.0.0.1") | select(.requests > 0) | select(.http_requests > 0)] | length')
    if [[ "$cnt" -ge 1 ]]; then
        return 0
    fi
    log_fail "Hosts metrics missing or empty: $resp"
    return 1
}

assert_apps_metrics() {
    local resp
    resp=$(curl -s "http://${ADMIN_HOST}:${ADMIN_PORT}${ADMIN_PATH_PREFIX}/api/metrics/apps")
    local cnt
    cnt=$(echo "$resp" | jq '[.[] | select(.requests > 0) | select(.http_requests > 0)] | length')
    if [[ "$cnt" -ge 1 ]]; then
        return 0
    fi
    log_fail "Apps metrics missing or empty: $resp"
    return 1
}

main() {
    if ! command -v jq >/dev/null 2>&1; then
        log_fail "jq is required"
        exit 1
    fi

    log_info "Starting mock HTTP server..."
    start_mock_http "$ECHO_HTTP_PORT"

    log_info "Starting Bifrost..."
    if ! start_bifrost; then
        log_fail "Failed to start Bifrost"
        exit 1
    fi

    log_info "Generating traffic..."
    generate_http_traffic

    log_info "Checking hosts metrics..."
    assert_hosts_metrics
    log_pass "Hosts metrics OK"

    log_info "Checking apps metrics..."
    assert_apps_metrics
    log_pass "Apps metrics OK"
}

main "$@"
