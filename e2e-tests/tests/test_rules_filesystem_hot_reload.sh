#!/bin/bash
set -eo pipefail

: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

source "$SCRIPT_DIR/../test_utils/http_client.sh"
source "$SCRIPT_DIR/../test_utils/assert.sh"
source "$SCRIPT_DIR/../test_utils/process.sh"

BIFROST_BIN="${BIFROST_BIN:-${PROJECT_ROOT}/target/debug/bifrost}"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi
if [[ ! -x "$BIFROST_BIN" ]]; then
    (cd "$PROJECT_ROOT" && cargo build --bin bifrost)
fi

PROXY_HOST="127.0.0.1"
PROXY_PORT="${PROXY_PORT:-$((18889 + ($$ % 1000)))}"
ECHO_PORT="${ECHO_PORT:-$((20889 + ($$ % 1000)))}"
DATA_DIR="${DATA_DIR:-${PROJECT_ROOT}/.bifrost-test-rules-fs-hot-reload-$$}"
ADMIN_BASE_URL="http://${PROXY_HOST}:${PROXY_PORT}/_bifrost"
TEST_RULE_NAME="fs-hot-reload-$$"
TEST_TARGET_HOST="127.0.0.1:${ECHO_PORT}"
TEST_URL="http://${TEST_TARGET_HOST}/status/200"
PROXY_PID=""
ECHO_PID=""

export BIFROST_DATA_DIR="$DATA_DIR"
export PROXY_HOST
export PROXY_PORT
TEST_ID="${TEST_ID:-}"

cleanup() {
    echo ""
    echo "Cleaning up..."
    if [[ -n "${PROXY_PID:-}" ]]; then
        safe_cleanup_proxy "$PROXY_PID" || true
    fi
    if [[ -n "${ECHO_PID:-}" ]]; then
        kill_pid "$ECHO_PID" || true
        wait_pid "$ECHO_PID" || true
    fi
    rm -rf "$DATA_DIR"
    echo "Cleanup done"
}

trap cleanup EXIT

log_info() { echo "[INFO] $*"; }
log_fail() { echo "[FAIL] $*"; }

wait_for_admin_ready() {
    local waited=0
    while [[ $waited -lt 60 ]]; do
        if curl -sf --connect-timeout 2 --max-time 5 "${ADMIN_BASE_URL}/api/auth/status" >/dev/null 2>&1; then
            return 0
        fi
        if [[ -n "$PROXY_PID" ]] && ! kill -0 "$PROXY_PID" 2>/dev/null; then
            log_fail "Proxy exited while waiting for admin readiness"
            tail -80 "$DATA_DIR/proxy.log" || true
            return 1
        fi
        sleep 1
        waited=$((waited + 1))
    done
    log_fail "Admin API did not become ready"
    tail -80 "$DATA_DIR/proxy.log" || true
    return 1
}

wait_for_http_status() {
    local expected="$1"
    local label="$2"
    local waited=0
    while [[ $waited -lt 20 ]]; do
        http_get "$TEST_URL" || true
        if [[ "${HTTP_STATUS:-}" == "$expected" ]]; then
            log_info "$label: observed HTTP $expected"
            return 0
        fi
        sleep 0.5
        waited=$((waited + 1))
    done
    log_fail "$label: expected HTTP $expected, got ${HTTP_STATUS:-empty}"
    tail -80 "$DATA_DIR/proxy.log" || true
    return 1
}

active_summary() {
    curl -sf --connect-timeout 2 --max-time 5 "${ADMIN_BASE_URL}/api/rules/active-summary"
}

wait_for_active_summary_contains() {
    local needle="$1"
    local label="$2"
    local waited=0
    while [[ $waited -lt 20 ]]; do
        local body
        body="$(active_summary || true)"
        if echo "$body" | jq -e --arg needle "$needle" '.merged_content | contains($needle)' >/dev/null 2>&1; then
            log_info "$label: active summary contains '$needle'"
            return 0
        fi
        sleep 0.5
        waited=$((waited + 1))
    done
    log_fail "$label: active summary did not contain '$needle'"
    active_summary || true
    tail -80 "$DATA_DIR/proxy.log" || true
    return 1
}

wait_for_active_summary_not_contains() {
    local needle="$1"
    local label="$2"
    local waited=0
    while [[ $waited -lt 20 ]]; do
        local body
        body="$(active_summary || true)"
        if echo "$body" | jq -e --arg needle "$needle" '(.merged_content | contains($needle)) | not' >/dev/null 2>&1; then
            log_info "$label: active summary no longer contains '$needle'"
            return 0
        fi
        sleep 0.5
        waited=$((waited + 1))
    done
    log_fail "$label: active summary still contained '$needle'"
    active_summary || true
    tail -80 "$DATA_DIR/proxy.log" || true
    return 1
}

start_services() {
    mkdir -p "$DATA_DIR"
    log_info "Starting HTTP echo server on port $ECHO_PORT"
    python3 "$SCRIPT_DIR/../mock_servers/http_echo_server.py" "$ECHO_PORT" > "$DATA_DIR/echo.log" 2>&1 &
    ECHO_PID=$!

    local waited=0
    while [[ $waited -lt 30 ]]; do
        if curl -sf --connect-timeout 2 --max-time 5 "http://127.0.0.1:${ECHO_PORT}/health" >/dev/null 2>&1; then
            break
        fi
        sleep 1
        waited=$((waited + 1))
    done
    if [[ $waited -ge 30 ]]; then
        log_fail "Echo server did not become ready"
        cat "$DATA_DIR/echo.log" || true
        return 1
    fi

    log_info "Starting Bifrost on port $PROXY_PORT with data dir $DATA_DIR"
    RUST_LOG=info "$BIFROST_BIN" \
        -p "$PROXY_PORT" \
        start --unsafe-ssl --skip-cert-check --no-system-proxy \
        > "$DATA_DIR/proxy.log" 2>&1 &
    PROXY_PID=$!

    wait_for_admin_ready
}

edit_rule_file_status_code() {
    local rule_path="$DATA_DIR/rules/${TEST_RULE_NAME}.bifrost"
    local from_status="$1"
    local to_status="$2"
    python3 - "$rule_path" "$from_status" "$to_status" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
from_status = sys.argv[2]
to_status = sys.argv[3]
content = path.read_text()
needle = f"statusCode://{from_status}"
if needle not in content:
    raise SystemExit(f"expected {needle} in rule file")
path.write_text(content.replace(needle, f"statusCode://{to_status}"))
PY
}

main() {
    echo "=========================================="
    echo "  Rules Filesystem Hot Reload E2E Tests"
    echo "=========================================="

    start_services
    wait_for_http_status "200" "initial request before local rule"

    log_info "Adding rule through local CLI storage path"
    "$BIFROST_BIN" rule add "$TEST_RULE_NAME" --content "127.0.0.1 statusCode://203"
    wait_for_http_status "203" "CLI rule add without Admin API save"
    wait_for_active_summary_contains "statusCode://203" "CLI rule add badge/active summary"

    log_info "Updating rule through local CLI storage path"
    "$BIFROST_BIN" rule update "$TEST_RULE_NAME" --content "127.0.0.1 statusCode://204"
    wait_for_http_status "204" "CLI rule update without Admin API save"
    wait_for_active_summary_contains "statusCode://204" "CLI rule update badge/active summary"

    log_info "Editing .bifrost rule file directly"
    edit_rule_file_status_code "204" "205"
    wait_for_http_status "205" "direct .bifrost edit without Admin API save"
    wait_for_active_summary_contains "statusCode://205" "direct edit badge/active summary"

    log_info "Deleting rule through local CLI storage path"
    "$BIFROST_BIN" rule delete "$TEST_RULE_NAME"
    wait_for_http_status "200" "CLI rule delete without Admin API save"
    wait_for_active_summary_not_contains "$TEST_RULE_NAME" "CLI rule delete badge/active summary"

    log_info "Re-adding rule for direct file deletion coverage"
    "$BIFROST_BIN" rule add "$TEST_RULE_NAME" --content "127.0.0.1 statusCode://206"
    wait_for_http_status "206" "CLI rule re-add before direct deletion"
    wait_for_active_summary_contains "statusCode://206" "CLI rule re-add badge/active summary"

    log_info "Deleting .bifrost rule file directly"
    rm -f "$DATA_DIR/rules/${TEST_RULE_NAME}.bifrost"
    wait_for_http_status "200" "direct .bifrost delete without Admin API save"
    wait_for_active_summary_not_contains "$TEST_RULE_NAME" "direct delete badge/active summary"

    echo ""
    echo "All filesystem hot reload checks passed."
}

main "$@"
