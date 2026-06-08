#!/bin/bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

source "${PROJECT_DIR}/e2e-tests/test_utils/assert.sh"
source "${PROJECT_DIR}/e2e-tests/test_utils/process.sh"

PROXY_PORT="${PROXY_PORT:-18892}"
BIFROST_BIN="${PROJECT_DIR}/target/release/bifrost"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi

TEST_DATA_DIR=""
PROXY_PID=""

cleanup() {
    if [[ -n "${TEST_DATA_DIR}" && -d "${TEST_DATA_DIR}" ]]; then
        BIFROST_DATA_DIR="${TEST_DATA_DIR}" "${BIFROST_BIN}" stop >/dev/null 2>&1 || true
        kill_bifrost_on_port "${PROXY_PORT}" || true
        rm -rf "${TEST_DATA_DIR}"
    fi
}
trap cleanup EXIT

build_bifrost() {
    if [[ -x "${BIFROST_BIN}" && "${SKIP_BUILD:-false}" == "true" ]]; then
        return 0
    fi

    (cd "${PROJECT_DIR}" && cargo build --release --bin bifrost) || return 1
}

start_proxy() {
    local mode="$1"
    shift

    export BIFROST_DATA_DIR="${TEST_DATA_DIR}"
    RUST_LOG=info "${BIFROST_BIN}" "$@" start \
        -p "${PROXY_PORT}" --skip-cert-check --unsafe-ssl --no-system-proxy \
        > "${TEST_DATA_DIR}/${mode}.stdout" 2> "${TEST_DATA_DIR}/${mode}.stderr" &
    PROXY_PID=$!

    local ready_url="http://127.0.0.1:${PROXY_PORT}/_bifrost/api/proxy/address"
    if wait_for_http_ready "${ready_url}" 30 0.5; then
        _log_pass "${mode} proxy admin API ready"
        return 0
    fi

    _log_fail "${mode} proxy admin API ready" "reachable" "timeout"
    cat "${TEST_DATA_DIR}/${mode}.stdout" || true
    cat "${TEST_DATA_DIR}/${mode}.stderr" || true
    return 1
}

stop_proxy() {
    export BIFROST_DATA_DIR="${TEST_DATA_DIR}"
    "${BIFROST_BIN}" stop >/dev/null 2>&1 || true
    if [[ -n "${PROXY_PID}" ]]; then
        wait_pid "${PROXY_PID}" || true
    fi
    kill_bifrost_on_port "${PROXY_PORT}" || true
    PROXY_PID=""
}

send_admin_request() {
    local mode="$1"

    curl -fsS "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/proxy/address" >/dev/null || {
        _log_fail "${mode} admin request" "success" "curl failed"
        return 1
    }
    _log_pass "${mode} admin request"
}

assert_file_log_created() {
    local mode="$1"

    sleep 1
    if find "${TEST_DATA_DIR}/logs" -maxdepth 1 -name 'bifrost*.log' -type f -size +0c | grep -q .; then
        _log_pass "${mode} writes non-empty file log"
    else
        _log_fail "${mode} writes non-empty file log" "bifrost*.log with content" "none"
        find "${TEST_DATA_DIR}" -maxdepth 3 -type f -print || true
        return 1
    fi
}

assert_no_console_tracing_logs() {
    local mode="$1"
    local combined="${TEST_DATA_DIR}/${mode}.combined"
    cat "${TEST_DATA_DIR}/${mode}.stdout" "${TEST_DATA_DIR}/${mode}.stderr" > "${combined}"

    if grep -E '^[0-9T:\.-]+Z[[:space:]]+(TRACE|DEBUG|INFO|WARN|ERROR) ' "${combined}" >/dev/null; then
        _log_fail "${mode} console tracing logs disabled by default" "no tracing console lines" "$(head -20 "${combined}")"
        return 1
    fi

    _log_pass "${mode} console tracing logs disabled by default"
}

assert_console_tracing_logs() {
    local mode="$1"
    local combined="${TEST_DATA_DIR}/${mode}.combined"
    cat "${TEST_DATA_DIR}/${mode}.stdout" "${TEST_DATA_DIR}/${mode}.stderr" > "${combined}"

    if grep -E '^[0-9T:\.-]+Z[[:space:]]+(TRACE|DEBUG|INFO|WARN|ERROR) ' "${combined}" >/dev/null; then
        _log_pass "${mode} console tracing logs enabled explicitly"
    else
        _log_fail "${mode} console tracing logs enabled explicitly" "at least one tracing console line" "$(head -40 "${combined}")"
        return 1
    fi
}

run_default_file_only_case() {
    TEST_DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-log-default-file.XXXXXX")"
    mkdir -p "${TEST_DATA_DIR}/logs"

    start_proxy "default-file" --log-dir "${TEST_DATA_DIR}/logs" || return 1
    send_admin_request "default-file" || { stop_proxy; return 1; }
    stop_proxy
    assert_file_log_created "default-file" || return 1
    assert_no_console_tracing_logs "default-file" || return 1
}

run_explicit_console_case() {
    TEST_DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-log-console.XXXXXX")"
    mkdir -p "${TEST_DATA_DIR}/logs"

    start_proxy "console-file" --log-dir "${TEST_DATA_DIR}/logs" --log-output console || return 1
    send_admin_request "console-file" || { stop_proxy; return 1; }
    stop_proxy
    assert_file_log_created "console-file" || return 1
    assert_console_tracing_logs "console-file" || return 1
}

run_macos_launchd_cleanup_case() {
    if [[ "$(uname -s)" != "Darwin" ]]; then
        _log_pass "launchd cleanup console log case skipped on non-macOS"
        return 0
    fi

    TEST_DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-log-launchd.XXXXXX")"
    mkdir -p "${TEST_DATA_DIR}/data" "${TEST_DATA_DIR}/logs"

    RUST_LOG=info BIFROST_DATA_DIR="${TEST_DATA_DIR}/data" "${BIFROST_BIN}" \
        --log-dir "${TEST_DATA_DIR}/logs" \
        system-proxy cleanup-daemon \
        --data-dir "${TEST_DATA_DIR}/data" \
        > "${TEST_DATA_DIR}/launchd-cleanup.stdout" \
        2> "${TEST_DATA_DIR}/launchd-cleanup.stderr" || return 1

    assert_file_log_created "launchd-cleanup" || return 1
    assert_console_tracing_logs "launchd-cleanup" || return 1
}

main() {
    build_bifrost || { echo "build bifrost failed"; exit 1; }

    run_default_file_only_case || { print_test_summary || exit 1; return 1; }
    run_explicit_console_case || { print_test_summary || exit 1; return 1; }
    run_macos_launchd_cleanup_case || { print_test_summary || exit 1; return 1; }

    print_test_summary || exit 1
}

SKIP_BUILD="${SKIP_BUILD:-false}"
main "$@"
