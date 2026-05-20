#!/bin/bash
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

TEST_DIRS=()
NEW_DATA_DIR=""

cleanup() {
    if [[ ${#TEST_DIRS[@]} -gt 0 ]]; then
        for dir in "${TEST_DIRS[@]}"; do
            if [[ -n "$dir" && -d "$dir" ]]; then
                BIFROST_DATA_DIR="$dir" "$BIFROST_BIN" -p "$PROXY_PORT" stop >/dev/null 2>&1 || true
                rm -rf "$dir"
            fi
        done
    fi
    kill_bifrost_on_port "$PROXY_PORT"
}
trap cleanup EXIT

build_if_needed() {
    if [[ "${SKIP_BUILD:-false}" == "true" && -x "$BIFROST_BIN" ]]; then
        return 0
    fi
    cargo build --release --bin bifrost
}

new_data_dir() {
    NEW_DATA_DIR="$(mktemp -d)"
    TEST_DIRS+=("$NEW_DATA_DIR")
}

admin_unreachable() {
    ! curl --connect-timeout 1 --max-time 2 -fsS \
        "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/proxy/address" >/dev/null 2>&1
}

test_daemon_blocks_missing_ca_without_tty_or_yes() {
    _log_info "case: daemon start blocks missing CA in non-interactive mode without --yes"

    local test_data_dir
    new_data_dir
    test_data_dir="$NEW_DATA_DIR"

    local output
    output="$(BIFROST_DATA_DIR="$test_data_dir" "$BIFROST_BIN" -p "$PROXY_PORT" start \
        --daemon --unsafe-ssl --no-system-proxy </dev/null 2>&1)"
    local code=$?

    if [[ "$code" -ne 0 ]]; then
        _log_pass "daemon start exits non-zero when CA needs confirmation"
    else
        _log_fail "daemon start exits non-zero when CA needs confirmation" "non-zero" "$code"
        echo "$output" >&2
        return 1
    fi

    assert_body_contains "no interactive terminal is available" "$output" \
        "daemon start explains non-interactive CA gate" || return 1
    assert_body_contains "--yes" "$output" \
        "daemon start suggests --yes for automated CA install" || return 1
    assert_body_not_contains "Daemon started with PID" "$output" \
        "daemon start does not report success before CA gate passes" || return 1

    if [[ -f "$test_data_dir/certs/ca.crt" ]]; then
        _log_pass "daemon CA gate still generates local CA material for follow-up install"
    else
        _log_fail "daemon CA gate still generates local CA material for follow-up install" \
            "certs/ca.crt exists" "missing"
        return 1
    fi

    if admin_unreachable; then
        _log_pass "admin API remains unreachable after blocked daemon start"
    else
        _log_fail "admin API remains unreachable after blocked daemon start" "unreachable" "reachable"
        return 1
    fi
}

test_daemon_skip_cert_check_still_starts() {
    _log_info "case: daemon start --skip-cert-check remains explicit escape hatch"

    local test_data_dir
    new_data_dir
    test_data_dir="$NEW_DATA_DIR"

    local output
    output="$(BIFROST_DATA_DIR="$test_data_dir" "$BIFROST_BIN" -p "$PROXY_PORT" start \
        --daemon --skip-cert-check --unsafe-ssl --no-system-proxy 2>&1)"
    local code=$?

    if [[ "$code" -eq 0 ]]; then
        _log_pass "daemon start succeeds with --skip-cert-check"
    else
        _log_fail "daemon start succeeds with --skip-cert-check" "0" "$code"
        echo "$output" >&2
        return 1
    fi

    assert_body_contains "Daemon started with PID" "$output" \
        "daemon start reports success after explicit cert-check skip" || return 1

    local ready_url="http://127.0.0.1:${PROXY_PORT}/_bifrost/api/proxy/address"
    if wait_for_http_ready "$ready_url" 30 0.5; then
        _log_pass "daemon admin API ready after --skip-cert-check"
    else
        _log_fail "daemon admin API ready after --skip-cert-check" "reachable" "timeout"
        echo "$output" >&2
        return 1
    fi

    BIFROST_DATA_DIR="$test_data_dir" "$BIFROST_BIN" -p "$PROXY_PORT" stop >/dev/null 2>&1 || true
}

main() {
    build_if_needed || { print_test_summary || exit 1; return 1; }

    test_daemon_blocks_missing_ca_without_tty_or_yes || { print_test_summary || exit 1; return 1; }
    test_daemon_skip_cert_check_still_starts || { print_test_summary || exit 1; return 1; }

    print_test_summary || exit 1
}

main "$@"
