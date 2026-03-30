#!/bin/bash

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
E2E_DIR="$SCRIPT_DIR/.."
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
BIFROST_BIN="${PROJECT_ROOT}/target/release/bifrost"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi

PROXY_PORT="${PROXY_PORT:-18889}"
PROXY_HOST="127.0.0.1"
DATA_DIR="./.bifrost-test-hot-reload-$$"
ECHO_PORT="${ECHO_PORT:-$((20000 + ($$ % 1000)))}"
ECHO_PID=""

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

cleanup() {
    echo ""
    echo "Cleaning up..."
    if [[ -n "$PROXY_PID" ]]; then
        safe_cleanup_proxy "$PROXY_PID"
    fi
    if [[ -d "$DATA_DIR" ]]; then
        rm -rf "$DATA_DIR"
    fi
    if [[ -n "$ECHO_PID" ]]; then
        kill_pid "$ECHO_PID"
        wait_pid "$ECHO_PID"
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

    log_info "Starting HTTP echo server on port $ECHO_PORT..."
    python3 "$SCRIPT_DIR/../mock_servers/http_echo_server.py" "$ECHO_PORT" > "$DATA_DIR/echo.log" 2>&1 &
    ECHO_PID=$!

    local echo_wait=0
    local echo_max_wait=30
    while [[ $echo_wait -lt $echo_max_wait ]]; do
        if ! kill -0 "$ECHO_PID" 2>/dev/null; then
            log_fail "Echo server process exited during startup"
            cat "$DATA_DIR/echo.log"
            return 1
        fi
        if curl -sf --connect-timeout 2 --max-time 5 "http://127.0.0.1:${ECHO_PORT}/health" >/dev/null 2>&1; then
            log_info "Echo server is ready on port $ECHO_PORT"
            break
        fi
        sleep 1
        echo_wait=$((echo_wait + 1))
    done
    if [[ $echo_wait -ge $echo_max_wait ]]; then
        log_fail "Echo server not responding after ${echo_max_wait}s"
        cat "$DATA_DIR/echo.log"
        return 1
    fi

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

    local max_wait=60
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

TEST_RULE_NAME="hotreload-test-rule-$$"
RULE_FIXTURES_DIR="$SCRIPT_DIR/../rules/hot_reload"
TEST_TARGET_HOST="127.0.0.1:${ECHO_PORT}"
TEST_URL="http://${TEST_TARGET_HOST}/status/200"

rule_content_from_fixture() {
    local name="$1"
    rule_fixture_content "$RULE_FIXTURES_DIR/$name"
}

test_initial_no_rule() {
    http_get "$TEST_URL"

    if [[ "$HTTP_STATUS" != "200" ]]; then
        log_fail "Initial request should return 200, got $HTTP_STATUS"
        return 1
    fi

    log_info "Initial request succeeded without rules"
    return 0
}

test_create_rule_via_api() {
    local rule_content
    rule_content=$(rule_content_from_fixture status_201.txt)

    local response
    response=$(create_rule "$TEST_RULE_NAME" "$rule_content" "true")

    if [[ $? -ne 0 ]]; then
        log_fail "Failed to create rule via API"
        log_debug "Response: $response"
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

    local verify_response
    verify_response=$(get_rule "$TEST_RULE_NAME")
    local rule_name
    rule_name=$(echo "$verify_response" | jq -r '.name // empty')

    if [[ "$rule_name" != "$TEST_RULE_NAME" ]]; then
        log_fail "Rule was not created correctly"
        log_debug "Verify response: $verify_response"
        return 1
    fi

    log_info "Rule created successfully"
    return 0
}

test_rule_takes_effect() {
    sleep 1

    http_get "$TEST_URL"

    if [[ "$HTTP_STATUS" != "201" ]]; then
        log_fail "Request should return 201 (rule applied), got $HTTP_STATUS"
        return 1
    fi

    log_info "New rule took effect (hot reload working)"
    return 0
}

test_disable_rule_via_api() {
    local response
    response=$(disable_rule "$TEST_RULE_NAME")

    if [[ $? -ne 0 ]]; then
        log_fail "Failed to disable rule via API"
        return 1
    fi

    sleep 1

    local verify_response
    verify_response=$(get_rule "$TEST_RULE_NAME")
    local enabled
    enabled=$(echo "$verify_response" | jq -r '.enabled')

    if [[ "$enabled" != "false" ]]; then
        log_fail "Rule should be disabled"
        return 1
    fi

    log_info "Rule disabled successfully"
    return 0
}

test_disabled_rule_not_applied() {
    sleep 1

    http_get "$TEST_URL"

    if [[ "$HTTP_STATUS" != "200" ]]; then
        log_fail "Request should return 200 (original) after disabling rule, got $HTTP_STATUS"
        return 1
    fi

    log_info "Disabled rule is no longer applied (hot reload working)"
    return 0
}

test_enable_rule_via_api() {
    local response
    response=$(enable_rule "$TEST_RULE_NAME")

    if [[ $? -ne 0 ]]; then
        log_fail "Failed to enable rule via API"
        return 1
    fi

    sleep 1

    local verify_response
    verify_response=$(get_rule "$TEST_RULE_NAME")
    local enabled
    enabled=$(echo "$verify_response" | jq -r '.enabled')

    if [[ "$enabled" != "true" ]]; then
        log_fail "Rule should be enabled"
        return 1
    fi

    log_info "Rule enabled successfully"
    return 0
}

test_enabled_rule_applied_again() {
    sleep 1

    http_get "$TEST_URL"

    if [[ "$HTTP_STATUS" != "201" ]]; then
        log_fail "Request should return 201 (rule re-applied), got $HTTP_STATUS"
        return 1
    fi

    log_info "Re-enabled rule is applied again (hot reload working)"
    return 0
}

test_update_rule_content() {
    local new_content
    new_content=$(rule_content_from_fixture status_202.txt)

    local response
    response=$(update_rule "$TEST_RULE_NAME" "$new_content" "true")

    if [[ $? -ne 0 ]]; then
        log_fail "Failed to update rule via API"
        return 1
    fi

    sleep 1

    http_get "$TEST_URL"

    if [[ "$HTTP_STATUS" != "202" ]]; then
        log_fail "Request should return 202 (updated rule), got $HTTP_STATUS"
        return 1
    fi

    log_info "Updated rule content took effect (hot reload working)"
    return 0
}

test_delete_rule_via_api() {
    local response
    response=$(delete_rule "$TEST_RULE_NAME")

    if [[ $? -ne 0 ]]; then
        log_fail "Failed to delete rule via API"
        return 1
    fi

    sleep 1

    http_get "$TEST_URL"

    if [[ "$HTTP_STATUS" != "200" ]]; then
        log_fail "Request should return 200 (original) after deleting rule, got $HTTP_STATUS"
        return 1
    fi

    log_info "Deleted rule no longer applied (hot reload working)"
    return 0
}

main() {
    echo "=========================================="
    echo "  Rules Hot Reload E2E Tests"
    echo "=========================================="
    echo ""

    if ! start_proxy; then
        echo "Failed to start proxy server"
        exit 1
    fi

    echo ""
    run_test "Initial request without rules" test_initial_no_rule
    run_test "Create rule via Admin API" test_create_rule_via_api
    run_test "New rule takes effect (hot reload)" test_rule_takes_effect
    run_test "Disable rule via Admin API" test_disable_rule_via_api
    run_test "Disabled rule not applied (hot reload)" test_disabled_rule_not_applied
    run_test "Enable rule via Admin API" test_enable_rule_via_api
    run_test "Re-enabled rule applied (hot reload)" test_enabled_rule_applied_again
    run_test "Update rule content via Admin API" test_update_rule_content
    run_test "Delete rule via Admin API" test_delete_rule_via_api

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
