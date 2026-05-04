#!/bin/bash
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

source "${PROJECT_DIR}/e2e-tests/test_utils/assert.sh"
source "${PROJECT_DIR}/e2e-tests/test_utils/process.sh"

PROXY_PORT="${PROXY_PORT:-18930}"
BIFROST_BIN="${PROJECT_DIR}/target/release/bifrost"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi

TEST_DATA_DIR=""
UDP_HOLDER_PID=""
FOREGROUND_PID=""

cleanup() {
    if [[ -n "$FOREGROUND_PID" ]]; then
        safe_cleanup_proxy "$FOREGROUND_PID"
    fi
    if [[ -n "$TEST_DATA_DIR" ]]; then
        BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" -p "$PROXY_PORT" stop >/dev/null 2>&1 || true
    fi
    if [[ -n "$UDP_HOLDER_PID" ]]; then
        kill_pid "$UDP_HOLDER_PID"
        wait_pid "$UDP_HOLDER_PID"
    fi
    kill_bifrost_on_port "$PROXY_PORT"
    if [[ -n "$TEST_DATA_DIR" && -d "$TEST_DATA_DIR" ]]; then
        rm -rf "$TEST_DATA_DIR"
    fi
}
trap cleanup EXIT

build_if_needed() {
    if [[ "${SKIP_BUILD:-false}" == "true" && -x "$BIFROST_BIN" ]]; then
        return 0
    fi
    cargo build --release --bin bifrost
}

start_udp_holder() {
    python3 - "$PROXY_PORT" <<'PY' &
import socket
import sys
import time

port = int(sys.argv[1])
sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
sock.bind(("0.0.0.0", port))
print("udp-ready", flush=True)
while True:
    time.sleep(1)
PY
    UDP_HOLDER_PID=$!
    sleep 0.5
    if ! kill -0 "$UDP_HOLDER_PID" 2>/dev/null; then
        _log_fail "udp holder started" "running" "not running"
        return 1
    fi
    _log_pass "udp holder started on ${PROXY_PORT}"
}

wait_process_exit() {
    local pid="$1"
    local waited=0
    while [[ "$waited" -lt 80 ]]; do
        if ! kill -0 "$pid" 2>/dev/null; then
            return 0
        fi
        sleep 0.1
        waited=$((waited + 1))
    done
    return 1
}

admin_unreachable() {
    ! curl -fsS "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/proxy/address" >/dev/null 2>&1
}

test_foreground_listener_failure_exits() {
    _log_info "case: foreground listener task failure exits process"

    local log_file="${TEST_DATA_DIR}/foreground.log"
    BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" -p "$PROXY_PORT" start \
        --skip-cert-check --unsafe-ssl --no-system-proxy \
        >"$log_file" 2>&1 &
    FOREGROUND_PID=$!

    if wait_process_exit "$FOREGROUND_PID"; then
        _log_pass "foreground process exited after listener failure"
    else
        _log_fail "foreground process exited after listener failure" "exited" "still running"
        cat "$log_file" || true
        return 1
    fi
    FOREGROUND_PID=""

    if admin_unreachable; then
        _log_pass "admin api is not reachable after listener failure"
    else
        _log_fail "admin api is not reachable after listener failure" "unreachable" "reachable"
        return 1
    fi

    local log_content
    log_content="$(cat "$log_file" 2>/dev/null || true)"
    assert_body_contains "Address already in use" "$log_content" "foreground surfaces UDP listener bind error" || return 1
}

test_daemon_waits_for_listener_ready() {
    _log_info "case: daemon start waits for listener readiness"

    local output
    output="$(BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" -p "$PROXY_PORT" start \
        --daemon --skip-cert-check --unsafe-ssl --no-system-proxy 2>&1)"
    local code=$?

    if [[ "$code" -ne 0 ]]; then
        _log_pass "daemon start returns non-zero when listener cannot become ready"
    else
        _log_fail "daemon start returns non-zero when listener cannot become ready" "non-zero" "$code"
        echo "$output" >&2
        return 1
    fi

    assert_body_contains "before the proxy listener became ready" "$output" "daemon reports readiness failure" || {
        echo "$output" >&2
        return 1
    }
    assert_body_not_contains "Daemon started with PID" "$output" "daemon does not report started before listener bind" || return 1

    if admin_unreachable; then
        _log_pass "daemon admin api is not reachable after failed readiness"
    else
        _log_fail "daemon admin api is not reachable after failed readiness" "unreachable" "reachable"
        return 1
    fi
}

main() {
    if is_windows; then
        _log_info "daemon readiness regression is Unix-specific; skipping on Windows"
        print_test_summary || exit 1
        return 0
    fi

    TEST_DATA_DIR="$(mktemp -d)"
    export BIFROST_DATA_DIR="$TEST_DATA_DIR"

    build_if_needed || { print_test_summary || exit 1; return 1; }
    start_udp_holder || { print_test_summary || exit 1; return 1; }

    test_foreground_listener_failure_exits || { print_test_summary || exit 1; return 1; }
    test_daemon_waits_for_listener_ready || { print_test_summary || exit 1; return 1; }

    print_test_summary || exit 1
}

main "$@"
