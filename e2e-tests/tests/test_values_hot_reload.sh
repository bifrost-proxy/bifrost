#!/bin/bash

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
E2E_DIR="$SCRIPT_DIR/.."
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
BIFROST_BIN="${PROJECT_ROOT}/target/release/bifrost"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi

PROXY_PORT="${PROXY_PORT:-18890}"
PROXY_HOST="127.0.0.1"
DATA_DIR="./.bifrost-test-values-hot-reload-$$"
MOCK_HTTP_PORT="${MOCK_HTTP_PORT:-$((PROXY_PORT + 1))}"

export ADMIN_HOST="$PROXY_HOST"
export ADMIN_PORT="$PROXY_PORT"
export ADMIN_PATH_PREFIX="/_bifrost"
export ADMIN_BASE_URL="http://${ADMIN_HOST}:${ADMIN_PORT}${ADMIN_PATH_PREFIX}"

source "$SCRIPT_DIR/../test_utils/admin_client.sh"
source "$SCRIPT_DIR/../test_utils/http_client.sh"
source "$SCRIPT_DIR/../test_utils/assert.sh"
source "$SCRIPT_DIR/../test_utils/rule_fixture.sh"
source "$SCRIPT_DIR/../test_utils/process.sh"

TESTS_RUN=0
TESTS_PASSED=0
TESTS_FAILED=0
PROXY_PID=""
MOCK_PID=""
RULE_FIXTURE="$SCRIPT_DIR/../rules/values/status_code_value.txt"

cleanup() {
    echo ""
    echo "Cleaning up..."
    if [[ -n "$PROXY_PID" ]]; then
        safe_cleanup_proxy "$PROXY_PID"
    fi
    if [[ -n "$MOCK_PID" ]]; then
        kill_pid "$MOCK_PID"
        wait_pid "$MOCK_PID"
    fi
    if [[ -d "$DATA_DIR" ]]; then
        rm -rf "$DATA_DIR"
    fi
    if is_windows; then kill_bifrost_on_port "$PROXY_PORT"; fi
    echo "Cleanup done"
}

trap cleanup EXIT

log_info() { echo "[INFO] $*"; }
log_pass() { echo "[PASS] $*"; }
log_fail() { echo "[FAIL] $*"; }
log_debug() { [[ "${DEBUG:-0}" == "1" ]] && echo "[DEBUG] $*"; }

run_test() {
    local test_name="$1"
    local test_func="$2"

    TESTS_RUN=$((TESTS_RUN + 1))
    log_info "Running test: $test_name"

    if $test_func; then
        TESTS_PASSED=$((TESTS_PASSED + 1))
        log_pass "$test_name"
        return 0
    else
        TESTS_FAILED=$((TESTS_FAILED + 1))
        log_fail "$test_name"
        return 1
    fi
}

start_proxy() {
    log_info "Starting proxy server on port $PROXY_PORT..."

    mkdir -p "$DATA_DIR"
    export BIFROST_DATA_DIR="$DATA_DIR"

    RUST_LOG=info "$BIFROST_BIN" \
        -p "$PROXY_PORT" \
        start --unsafe-ssl \
        > "$DATA_DIR/proxy.log" 2>&1 &
    PROXY_PID=$!

    log_info "Proxy PID: $PROXY_PID"

    sleep 2

    if ! kill -0 "$PROXY_PID" 2>/dev/null; then
        log_fail "Proxy server failed to start"
        cat "$DATA_DIR/proxy.log"
        return 1
    fi

    local max_wait=30
    local waited=0
    while [[ $waited -lt $max_wait ]]; do
        if ! kill -0 "$PROXY_PID" 2>/dev/null; then
            log_fail "Proxy process exited during startup (PID: $PROXY_PID)"
            cat "$DATA_DIR/proxy.log"
            return 1
        fi
        if curl -s "http://${PROXY_HOST}:${PROXY_PORT}${ADMIN_PATH_PREFIX}/api/system/status" >/dev/null 2>&1; then
            log_info "Proxy server is ready"
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done

    log_fail "Proxy server not responding after ${max_wait}s"
    cat "$DATA_DIR/proxy.log"
    return 1
}

setup_rule_with_value() {
    log_info "Creating rule that uses a value variable..."
    
    local rule_content
    rule_content=$(rule_fixture_content "$RULE_FIXTURE" "TARGET_HOST=${TEST_TARGET_HOST}")
    local response
    response=$(create_rule "value-test-rule" "$rule_content" "true")
    
    if [[ $? -ne 0 ]]; then
        log_fail "Failed to create rule via API"
        return 1
    fi
    
    local success
    success=$(echo "$response" | jq -r '.success // empty')
    
    if [[ "$success" != "true" ]]; then
        local error
        error=$(echo "$response" | jq -r '.error // empty')
        log_fail "Create rule failed: $error"
        return 1
    fi
    
    sleep 1
    log_info "Rule created successfully"
    return 0
}

TEST_VALUE_NAME="status_code"
TEST_TARGET_HOST="127.0.0.1:${MOCK_HTTP_PORT}"
TEST_URL="http://${TEST_TARGET_HOST}/status/200"

test_initial_without_value() {
    http_get "$TEST_URL"

    if [[ "$HTTP_STATUS" != "200" ]]; then
        log_fail "Initial request should return 200 (value not set), got $HTTP_STATUS"
        return 1
    fi

    log_info "Initial request succeeded (value not set, rule uses literal)"
    return 0
}

test_create_value_via_api() {
    local response
    response=$(create_value "$TEST_VALUE_NAME" "201")

    if [[ $? -ne 0 ]]; then
        log_fail "Failed to create value via API"
        log_debug "Response: $response"
        return 1
    fi

    local success
    success=$(echo "$response" | jq -r '.success // empty')

    if [[ "$success" != "true" ]]; then
        local error
        error=$(echo "$response" | jq -r '.error // empty')
        log_fail "Create value failed: $error"
        return 1
    fi

    sleep 1

    local verify_response
    verify_response=$(get_value "$TEST_VALUE_NAME")
    local value_name
    value_name=$(echo "$verify_response" | jq -r '.name // empty')

    if [[ "$value_name" != "$TEST_VALUE_NAME" ]]; then
        log_fail "Value was not created correctly"
        log_debug "Verify response: $verify_response"
        return 1
    fi

    log_info "Value created successfully"
    return 0
}

test_value_takes_effect() {
    sleep 1

    http_get "$TEST_URL"

    if [[ "$HTTP_STATUS" != "201" ]]; then
        log_fail "Request should return 201 (value applied), got $HTTP_STATUS"
        return 1
    fi

    log_info "New value took effect (hot reload working)"
    return 0
}

test_update_value_via_api() {
    local response
    response=$(update_value "$TEST_VALUE_NAME" "202")

    if [[ $? -ne 0 ]]; then
        log_fail "Failed to update value via API"
        return 1
    fi

    sleep 1

    http_get "$TEST_URL"

    if [[ "$HTTP_STATUS" != "202" ]]; then
        log_fail "Request should return 202 (updated value), got $HTTP_STATUS"
        return 1
    fi

    log_info "Updated value took effect (hot reload working)"
    return 0
}

test_delete_value_via_api() {
    local response
    response=$(delete_value "$TEST_VALUE_NAME")

    if [[ $? -ne 0 ]]; then
        log_fail "Failed to delete value via API"
        return 1
    fi

    sleep 1

    http_get "$TEST_URL"

    if [[ "$HTTP_STATUS" != "200" ]]; then
        log_fail "Request should return 200 (value deleted, rule uses literal), got $HTTP_STATUS"
        return 1
    fi

    log_info "Value deletion took effect (hot reload working)"
    return 0
}

main() {
    echo "=========================================="
    echo "  Values Hot Reload E2E Tests"
    echo "=========================================="
    echo ""

    if ! start_proxy; then
        echo "Failed to start proxy server"
        exit 1
    fi

    python3 "$SCRIPT_DIR/../mock_servers/http_echo_server.py" "$MOCK_HTTP_PORT" >/dev/null 2>&1 &
    MOCK_PID=$!
    local mock_waited=0
    while [[ $mock_waited -lt 15 ]]; do
        if curl -s "http://127.0.0.1:${MOCK_HTTP_PORT}/health" >/dev/null 2>&1; then
            break
        fi
        sleep 1
        mock_waited=$((mock_waited + 1))
    done

    if ! setup_rule_with_value; then
        echo "Failed to setup rule with value variable"
        exit 1
    fi

    echo ""
    run_test "Initial request without value" test_initial_without_value
    run_test "Create value via Admin API" test_create_value_via_api
    run_test "New value takes effect (hot reload)" test_value_takes_effect
    run_test "Update value via Admin API" test_update_value_via_api
    run_test "Delete value via Admin API" test_delete_value_via_api

    echo ""
    echo "=========================================="
    echo "  Test Summary"
    echo "=========================================="
    echo "  Total:  $TESTS_RUN"
    echo "  Passed: $TESTS_PASSED"
    echo "  Failed: $TESTS_FAILED"
    echo "=========================================="

    if [[ $TESTS_FAILED -gt 0 ]]; then
        echo ""
        echo "Proxy log:"
        tail -50 "$DATA_DIR/proxy.log"
        exit 1
    fi

    exit 0
}

main "$@"
