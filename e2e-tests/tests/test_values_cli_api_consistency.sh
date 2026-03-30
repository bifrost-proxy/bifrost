#!/bin/bash

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

PROXY_PORT="${PROXY_PORT:-$((18700 + ($$ % 500)))}"
ADMIN_HOST="${ADMIN_HOST:-127.0.0.1}"
ADMIN_PORT="${ADMIN_PORT:-$PROXY_PORT}"

export ADMIN_HOST ADMIN_PORT ADMIN_PATH_PREFIX="/_bifrost"

source "$SCRIPT_DIR/../test_utils/assert.sh"
source "$SCRIPT_DIR/../test_utils/admin_client.sh"
source "$SCRIPT_DIR/../test_utils/process.sh"
ADMIN_BASE_URL="http://${ADMIN_HOST}:${ADMIN_PORT}${ADMIN_PATH_PREFIX}"
export ADMIN_BASE_URL

BIFROST_BIN="$ROOT_DIR/target/release/bifrost"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi
TEST_DATA_DIR="$ROOT_DIR/.bifrost-e2e-values-consistency-${PROXY_PORT}-$$"

PROXY_PID=""
passed=0
failed=0

assert_equals() {
    local expected="$1"
    local actual="$2"
    local message="${3:-Values should be equal}"

    if [[ "$expected" == "$actual" ]]; then
        _log_pass "$message"
        return 0
    else
        _log_fail "$message" "$expected" "$actual"
        return 1
    fi
}

cleanup() {
    if is_windows; then kill_bifrost_on_port "$PROXY_PORT"; fi
    safe_cleanup_proxy "$PROXY_PID"
    rm -rf "$TEST_DATA_DIR"
}

trap cleanup EXIT

record_result() {
    if [[ $1 -eq 0 ]]; then
        ((passed++))
    else
        ((failed++))
    fi
}

start_bifrost() {
    mkdir -p "$TEST_DATA_DIR"

    echo "[INFO] Starting bifrost on port $PROXY_PORT with data dir $TEST_DATA_DIR"
    BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" \
        -p "$PROXY_PORT" start --skip-cert-check --unsafe-ssl \
        >"$TEST_DATA_DIR/bifrost.log" 2>&1 &
    PROXY_PID=$!

    local waited=0
    while [[ $waited -lt 60 ]]; do
        if curl -s "${ADMIN_BASE_URL}/api/system/status" >/dev/null 2>&1; then
            echo "[INFO] Bifrost started (PID: $PROXY_PID)"
            return 0
        fi
        if ! kill -0 "$PROXY_PID" 2>/dev/null; then
            echo "[FAIL] Bifrost process exited early"
            tail -50 "$TEST_DATA_DIR/bifrost.log" 2>/dev/null
            return 1
        fi
        sleep 1
        waited=$((waited + 1))
    done

    echo "[FAIL] Timeout waiting for bifrost"
    return 1
}

cli_value_set() {
    local name="$1"
    local value="$2"
    BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" value set "$name" "$value" 2>&1
}

cli_value_get() {
    local name="$1"
    BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" value get "$name" 2>&1
}

cli_value_get_value() {
    local name="$1"
    cli_value_get "$name" | awk 'NF { last=$0 } END { print last }'
}

cli_value_list() {
    BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" value list 2>&1
}

cli_value_delete() {
    local name="$1"
    BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" value delete "$name" 2>&1
}

test_api_set_cli_get() {
    echo "[TEST] API set -> CLI get"
    local name="API_TO_CLI_$$"
    local value="hello-from-api"

    create_value "$name" "$value" >/dev/null

    local cli_output
    cli_output=$(cli_value_get_value "$name")

    if assert_equals "$value" "$cli_output" "CLI should see value set by API"; then
        return 0
    else
        return 1
    fi
}

test_api_set_cli_list() {
    echo "[TEST] API set -> CLI list contains"
    local name="API_LIST_$$"
    local value="api-list-test"

    create_value "$name" "$value" >/dev/null

    local value_file="$TEST_DATA_DIR/values/${name}.txt"
    if [[ -f "$value_file" ]]; then
        local file_value
        file_value=$(cat "$value_file")
        assert_equals "$value" "$file_value" "Value file should match API value" || return 1
    else
        _log_fail "Value file should exist" "$value_file" "missing"
        return 1
    fi

    local cli_output
    cli_output=$(cli_value_list)

    if echo "$cli_output" | grep -q "$name = $value"; then
        _log_pass "API set -> CLI list: found"
        return 0
    else
        _log_fail "API set -> CLI list: not found"
        echo "[DEBUG] CLI list output: $cli_output"
        return 1
    fi
}

test_api_update_cli_verify() {
    echo "[TEST] API update -> CLI verify"
    local name="API_UPDATE_$$"

    create_value "$name" "original" >/dev/null
    update_value "$name" "updated-by-api" >/dev/null

    local cli_output
    cli_output=$(cli_value_get_value "$name")

    if assert_equals "updated-by-api" "$cli_output" "CLI should see updated value from API"; then
        return 0
    else
        return 1
    fi
}

test_api_delete_cli_verify() {
    echo "[TEST] API delete -> CLI verify"
    local name="API_DEL_$$"

    create_value "$name" "to-be-deleted" >/dev/null
    delete_value "$name" >/dev/null

    local cli_output
    cli_output=$(cli_value_get "$name" 2>&1)
    local rc=$?

    if [[ $rc -ne 0 ]] || echo "$cli_output" | grep -qi "not found"; then
        _log_pass "API delete -> CLI verify: value removed"
        return 0
    else
        _log_fail "API delete -> CLI verify: value still exists"
        echo "[DEBUG] CLI output: $cli_output"
        return 1
    fi
}

main() {
    echo "=========================================="
    echo "  Values CLI-API Consistency E2E Tests"
    echo "=========================================="
    echo ""

    if ! start_bifrost; then
        echo "[FAIL] Cannot start bifrost, aborting"
        exit 1
    fi

    echo ""

    test_api_set_cli_get
    record_result $?

    test_api_set_cli_list
    record_result $?

    test_api_update_cli_verify
    record_result $?

    test_api_delete_cli_verify
    record_result $?

    echo ""
    echo "======================================"
    echo "  Values CLI-API Consistency Summary"
    echo "======================================"
    echo "  Passed: $passed"
    echo "  Failed: $failed"
    echo "======================================"

    if [[ $failed -eq 0 ]]; then
        echo "All tests passed!"
        exit 0
    else
        echo "Some tests failed!"
        exit 1
    fi
}

main "$@"
