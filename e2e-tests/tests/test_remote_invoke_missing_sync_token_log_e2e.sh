#!/bin/bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

set -uo pipefail

unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY all_proxy ALL_PROXY no_proxy NO_PROXY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

source "${PROJECT_DIR}/e2e-tests/test_utils/assert.sh"
source "${PROJECT_DIR}/e2e-tests/test_utils/process.sh"

pick_free_port() {
    python3 - <<'PY'
import socket
with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
    s.bind(("127.0.0.1", 0))
    print(s.getsockname()[1])
PY
}

ADMIN_PORT="${ADMIN_PORT:-$(pick_free_port)}"
BIFROST_BIN="${BIFROST_BIN:-${PROJECT_DIR}/target/debug/bifrost}"
TEST_DATA_DIR=""
BIFROST_PID=""

cleanup() {
    if [[ -n "$BIFROST_PID" ]]; then
        kill_pid "$BIFROST_PID"
        wait_pid "$BIFROST_PID" || true
    fi
    kill_bifrost_on_port "$ADMIN_PORT"
    if [[ -n "$TEST_DATA_DIR" && -d "$TEST_DATA_DIR" ]]; then
        rm -rf "$TEST_DATA_DIR"
    fi
}
trap cleanup EXIT

build_bifrost() {
    if [[ "${SKIP_BUILD:-false}" == "true" && -x "$BIFROST_BIN" ]]; then
        return 0
    fi
    cargo build --bin bifrost
}

wait_for_admin_api() {
    local waited=0
    while [[ "$waited" -lt 40 ]]; do
        if curl -fsS "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/proxy/address" >/dev/null 2>&1; then
            return 0
        fi
        if ! kill -0 "$BIFROST_PID" 2>/dev/null; then
            echo "bifrost exited early" >&2
            cat "${TEST_DATA_DIR}/bifrost.log" >&2 || true
            return 1
        fi
        sleep 0.25
        waited=$((waited + 1))
    done
    echo "admin api did not become ready" >&2
    cat "${TEST_DATA_DIR}/bifrost.log" >&2 || true
    return 1
}

wait_for_log_observation_window() {
    local waited=0
    while [[ "$waited" -lt 40 ]]; do
        if grep -q "remote invoke relay registration waiting for sync session token" "${TEST_DATA_DIR}/bifrost.log" 2>/dev/null; then
            break
        fi
        if ! kill -0 "$BIFROST_PID" 2>/dev/null; then
            echo "bifrost exited before remote invoke log observation completed" >&2
            cat "${TEST_DATA_DIR}/bifrost.log" >&2 || true
            return 1
        fi
        sleep 0.25
        waited=$((waited + 1))
    done
    sleep 5
}

main() {
    build_bifrost || { echo "build failed"; exit 1; }

    TEST_DATA_DIR="$(mktemp -d)"
    export BIFROST_DATA_DIR="$TEST_DATA_DIR"

    RUST_LOG=info "$BIFROST_BIN" start -p "$ADMIN_PORT" \
        --yes --skip-cert-check --unsafe-ssl --no-system-proxy \
        >"${TEST_DATA_DIR}/bifrost.log" 2>&1 &
    BIFROST_PID=$!

    wait_for_admin_api || exit 1
    wait_for_log_observation_window || exit 1

    local log_content
    log_content="$(cat "${TEST_DATA_DIR}/bifrost.log")"
    assert_body_contains \
        "remote invoke relay registration waiting for sync session token" \
        "$log_content" \
        "missing sync token should be logged as one waiting message"
    assert_body_not_contains \
        "failed to register with relay" \
        "$log_content" \
        "missing sync token should not be emitted as relay registration ERROR"

    local waiting_count
    waiting_count="$(grep -c "remote invoke relay registration waiting for sync session token" "${TEST_DATA_DIR}/bifrost.log" || true)"
    assert_equals "1" "$waiting_count" "missing sync token waiting message should be logged once"

    print_test_summary || exit 1
}

main "$@"
