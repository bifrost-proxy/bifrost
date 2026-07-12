#!/bin/bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

source "${PROJECT_DIR}/e2e-tests/test_utils/assert.sh"
source "${PROJECT_DIR}/e2e-tests/test_utils/process.sh"

PROXY_PORT="${PROXY_PORT:-18891}"
BIFROST_BIN="${BIFROST_BIN:-${PROJECT_DIR}/target/release/bifrost}"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi

if [[ "${BIFROST_COVERAGE_E2E:-0}" == "1" ]]; then
    echo "SKIP: daemon detachment cannot provide a bounded LLVM profile lifecycle"
    exit 0
fi

TEST_DATA_DIR=""
LOG_DIR=""

cleanup() {
    if [[ -n "${TEST_DATA_DIR}" ]] && [[ -d "${TEST_DATA_DIR}" ]]; then
        BIFROST_DATA_DIR="${TEST_DATA_DIR}" "${BIFROST_BIN}" stop >/dev/null 2>&1 || true
        sleep 1
        kill_bifrost_on_port "${PROXY_PORT}" || true
        rm -rf "${TEST_DATA_DIR}"
    fi
}
trap cleanup EXIT

build_bifrost() {
    if [[ -x "${BIFROST_BIN}" ]] && [[ "${SKIP_BUILD:-false}" == "true" ]]; then
        return 0
    fi

    if [[ ! -x "${BIFROST_BIN}" ]]; then
        (cd "${PROJECT_DIR}" && cargo build --release --bin bifrost) || return 1
    fi
}

start_daemon() {
    export BIFROST_DATA_DIR="${TEST_DATA_DIR}"
    RUST_LOG=debug "${BIFROST_BIN}" -l debug --log-dir "${LOG_DIR}" start \
        -p "${PROXY_PORT}" --skip-cert-check --unsafe-ssl --no-system-proxy --daemon \
        > "${TEST_DATA_DIR}/start.log" 2>&1
}

assert_proxy_ready() {
    local ready_url="http://127.0.0.1:${PROXY_PORT}/_bifrost/api/proxy/address"
    if wait_for_http_ready "${ready_url}" 30 0.5; then
        _log_pass "daemon admin API ready"
    else
        _log_fail "daemon admin API ready" "reachable" "timeout"
        [[ -f "${TEST_DATA_DIR}/start.log" ]] && cat "${TEST_DATA_DIR}/start.log"
        return 1
    fi
}

assert_proxy_request_debug_logs() {
    local http_status
    http_status="$(curl -sS -o "${TEST_DATA_DIR}/response.html" -w "%{http_code}" \
        -x "http://127.0.0.1:${PROXY_PORT}" http://example.com)" || {
        _log_fail "proxy request via daemon" "curl success" "curl failed"
        return 1
    }

    assert_status_2xx "${http_status}" "proxy request via daemon should succeed" || return 1

    # Hit an admin route so the daemon definitely handles at least one admin
    # request during the assertion window.
    curl -sS "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/app-icon/does-not-exist" >/dev/null 2>&1 || true
    sleep 1

    if grep -R -nE '^[0-9T:\.-]+Z (TRACE|DEBUG|INFO|WARN|ERROR) .+\.rs:[0-9]+:' "${LOG_DIR}"/bifrost*.log > "${TEST_DATA_DIR}/debug-lines.log" 2>/dev/null; then
        _log_pass "daemon log file contains structured tracing lines"
    else
        _log_fail "daemon log file contains structured tracing lines" "at least one tracing entry with file/line metadata" "none found"
        find "${LOG_DIR}" -maxdepth 1 -type f -print || true
        return 1
    fi
}

main() {
    build_bifrost || { echo "编译 bifrost 失败"; exit 1; }

    TEST_DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-daemon-log-level.XXXXXX")"
    LOG_DIR="${TEST_DATA_DIR}/logs"
    mkdir -p "${LOG_DIR}"

    start_daemon || { echo "daemon 启动失败"; cat "${TEST_DATA_DIR}/start.log"; exit 1; }
    assert_proxy_ready || { print_test_summary || exit 1; return 1; }
    assert_proxy_request_debug_logs || { print_test_summary || exit 1; return 1; }

    BIFROST_DATA_DIR="${TEST_DATA_DIR}" "${BIFROST_BIN}" stop >/dev/null 2>&1 || true

    print_test_summary || exit 1
}

SKIP_BUILD="${SKIP_BUILD:-false}"
main "$@"
