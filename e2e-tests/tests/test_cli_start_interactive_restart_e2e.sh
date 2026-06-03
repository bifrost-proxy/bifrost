#!/bin/bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

source "${PROJECT_DIR}/e2e-tests/test_utils/assert.sh"
source "${PROJECT_DIR}/e2e-tests/test_utils/process.sh"

# 禁止使用 9900 端口（正式环境端口）
PROXY_PORT="${PROXY_PORT:-18890}"
BIFROST_BIN="${PROJECT_DIR}/target/release/bifrost"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi

TEST_DATA_DIR=""
PROXY_PID=""
RESTART_PID=""

cleanup() {
    if is_windows; then kill_bifrost_on_port "$PROXY_PORT"; fi
    safe_cleanup_proxy "$RESTART_PID"
    safe_cleanup_proxy "$PROXY_PID"

    if [[ -n "$TEST_DATA_DIR" ]] && [[ -d "$TEST_DATA_DIR" ]]; then
        rm -rf "$TEST_DATA_DIR"
    fi
}
trap cleanup EXIT

wait_proxy_ready() {
    local port="$1"
    local waited=0
    while [[ "$waited" -lt 60 ]]; do
        if curl -fsS "http://127.0.0.1:${port}/_bifrost/api/proxy/address" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.2
        waited=$((waited + 1))
    done
    return 1
}

wait_pid_exit() {
    local pid="$1"
    local waited=0
    while [[ "$waited" -lt 50 ]]; do
        if ! kill -0 "$pid" 2>/dev/null; then
            return 0
        fi
        sleep 0.1
        waited=$((waited + 1))
    done
    return 1
}

start_proxy_bg() {
    local log_file="$1"

    export BIFROST_DATA_DIR="${TEST_DATA_DIR}"

    "$BIFROST_BIN" -p "${PROXY_PORT}" start \
        --skip-cert-check --unsafe-ssl --no-system-proxy \
        >"${log_file}" 2>&1 &
    echo $!
}

stop_proxy() {
    export BIFROST_DATA_DIR="${TEST_DATA_DIR}"
    "$BIFROST_BIN" stop >/dev/null 2>&1 || true
    sleep 0.5
}

test_restart_yes() {
    _log_info "case: conflict -> input y -> restart"

    local old_pid
    old_pid="$(start_proxy_bg "${TEST_DATA_DIR}/proxy-old.log")"
    PROXY_PID="$old_pid"

    sleep 1
    if ! kill -0 "$old_pid" 2>/dev/null; then
        _log_fail "proxy started" "running process" "not running"
        cat "${TEST_DATA_DIR}/proxy-old.log" || true
        return 1
    fi

    if ! wait_proxy_ready "$PROXY_PORT"; then
        _log_fail "admin api ready" "reachable" "unreachable"
        cat "${TEST_DATA_DIR}/proxy-old.log" || true
        return 1
    fi

    export BIFROST_DATA_DIR="${TEST_DATA_DIR}"
    printf 'y\n' | "$BIFROST_BIN" -p "${PROXY_PORT}" start \
        --skip-cert-check --unsafe-ssl --no-system-proxy \
        >"${TEST_DATA_DIR}/proxy-restart.log" 2>&1 &
    local new_pid=$!
    RESTART_PID="$new_pid"

    assert_not_equals "$old_pid" "$new_pid" "restart should spawn a new process" || return 1

    if ! wait_proxy_ready "$PROXY_PORT"; then
        _log_fail "admin api ready after restart" "reachable" "unreachable"
        cat "${TEST_DATA_DIR}/proxy-restart.log" || true
        return 1
    fi

    if wait_pid_exit "$old_pid"; then
        _log_pass "old process exited"
    else
        _log_fail "old process exited" "not running" "still running"
        return 1
    fi

    if kill -0 "$new_pid" 2>/dev/null; then
        _log_pass "new process running"
    else
        _log_fail "new process running" "running" "not running"
        cat "${TEST_DATA_DIR}/proxy-restart.log" || true
        return 1
    fi

    if command -v jq >/dev/null 2>&1 && [[ -f "${TEST_DATA_DIR}/runtime.json" ]]; then
        local runtime_pid
        runtime_pid="$(jq -r '.pid' "${TEST_DATA_DIR}/runtime.json" 2>/dev/null || true)"
        assert_equals "$new_pid" "$runtime_pid" "runtime.json pid should match restarted process" || return 1
    fi

    stop_proxy
    safe_cleanup_proxy "$new_pid"
    RESTART_PID=""
    PROXY_PID=""
}

test_restart_auto_yes() {
    _log_info "case: conflict -> -y -> restart"

    local old_pid
    old_pid="$(start_proxy_bg "${TEST_DATA_DIR}/proxy-old-auto.log")"
    PROXY_PID="$old_pid"

    sleep 1
    if ! kill -0 "$old_pid" 2>/dev/null; then
        _log_fail "proxy started" "running process" "not running"
        cat "${TEST_DATA_DIR}/proxy-old-auto.log" || true
        return 1
    fi

    if ! wait_proxy_ready "$PROXY_PORT"; then
        _log_fail "admin api ready" "reachable" "unreachable"
        cat "${TEST_DATA_DIR}/proxy-old-auto.log" || true
        return 1
    fi

    export BIFROST_DATA_DIR="${TEST_DATA_DIR}"
    "$BIFROST_BIN" -p "${PROXY_PORT}" start -y \
        --skip-cert-check --unsafe-ssl --no-system-proxy \
        >"${TEST_DATA_DIR}/proxy-restart-auto.log" 2>&1 &
    local new_pid=$!
    RESTART_PID="$new_pid"

    assert_not_equals "$old_pid" "$new_pid" "auto yes restart should spawn a new process" || return 1

    if ! wait_proxy_ready "$PROXY_PORT"; then
        _log_fail "admin api ready after auto restart" "reachable" "unreachable"
        cat "${TEST_DATA_DIR}/proxy-restart-auto.log" || true
        return 1
    fi

    if wait_pid_exit "$old_pid"; then
        _log_pass "old process exited"
    else
        _log_fail "old process exited" "not running" "still running"
        return 1
    fi

    if kill -0 "$new_pid" 2>/dev/null; then
        _log_pass "new process running"
    else
        _log_fail "new process running" "running" "not running"
        cat "${TEST_DATA_DIR}/proxy-restart-auto.log" || true
        return 1
    fi

    local log_content
    log_content="$(cat "${TEST_DATA_DIR}/proxy-restart-auto.log" 2>/dev/null || true)"
    assert_body_not_contains "Restart? (y/n)" "$log_content" "-y should skip interactive prompt" || return 1

    stop_proxy
    safe_cleanup_proxy "$new_pid"
    RESTART_PID=""
    PROXY_PID=""
}

test_restart_no() {
    _log_info "case: conflict -> input n -> exit without killing"

    local old_pid
    old_pid="$(start_proxy_bg "${TEST_DATA_DIR}/proxy-old2.log")"
    PROXY_PID="$old_pid"

    sleep 1
    if ! kill -0 "$old_pid" 2>/dev/null; then
        _log_fail "proxy started" "running process" "not running"
        cat "${TEST_DATA_DIR}/proxy-old2.log" || true
        return 1
    fi

    if ! wait_proxy_ready "$PROXY_PORT"; then
        _log_fail "admin api ready" "reachable" "unreachable"
        cat "${TEST_DATA_DIR}/proxy-old2.log" || true
        return 1
    fi

    export BIFROST_DATA_DIR="${TEST_DATA_DIR}"
    local output
    output="$(printf 'n\n' | "$BIFROST_BIN" -p "${PROXY_PORT}" start --skip-cert-check --unsafe-ssl --no-system-proxy 2>&1)"
    local code=$?

    assert_equals "0" "$code" "start should exit 0 when cancelled" || {
        echo "$output" >&2
        return 1
    }

    if kill -0 "$old_pid" 2>/dev/null; then
        _log_pass "old process still running"
    else
        _log_fail "old process still running" "running" "not running"
        echo "$output" >&2
        return 1
    fi

    stop_proxy
    safe_cleanup_proxy "$old_pid"
    PROXY_PID=""
}

main() {
    TEST_DATA_DIR="$(mktemp -d)"

    test_restart_yes || { print_test_summary || exit 1; return 1; }
    test_restart_auto_yes || { print_test_summary || exit 1; return 1; }
    test_restart_no || { print_test_summary || exit 1; return 1; }

    print_test_summary || exit 1
}

main "$@"
