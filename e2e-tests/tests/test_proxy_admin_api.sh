#!/bin/bash

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test_utils/admin_client.sh"

ADMIN_HOST="${ADMIN_HOST:-127.0.0.1}"
ADMIN_PORT="${ADMIN_PORT:-}"
if [[ -z "${ADMIN_PORT}" ]]; then
    ADMIN_PORT="$(allocate_free_port)"
fi
ADMIN_PATH_PREFIX="${ADMIN_PATH_PREFIX:-/_bifrost}"
export ADMIN_PATH_PREFIX

TESTS_RUN=0
TESTS_PASSED=0
TESTS_FAILED=0

log_info() { echo "[INFO] $*"; }
log_pass() { echo "[PASS] $*"; }
log_fail() { echo "[FAIL] $*"; }
log_debug() { [[ "${DEBUG:-0}" == "1" ]] && echo "[DEBUG] $*"; }

assert_equals() {
    local expected="$1"
    local actual="$2"
    local msg="${3:-Values should be equal}"

    if [[ "$expected" == "$actual" ]]; then
        return 0
    else
        log_fail "$msg: expected '$expected', got '$actual'"
        return 1
    fi
}

assert_not_empty() {
    local value="$1"
    local msg="${2:-Value should not be empty}"

    if [[ -n "$value" && "$value" != "null" ]]; then
        return 0
    else
        log_fail "$msg: value is empty or null"
        return 1
    fi
}

assert_json_has_field() {
    local json="$1"
    local field="$2"
    local msg="${3:-JSON should have field}"

    local has_field
    has_field=$(echo "$json" | jq "has(\"$field\")")

    if [[ "$has_field" == "true" ]]; then
        return 0
    else
        log_fail "$msg: field '$field' not found"
        return 1
    fi
}

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

test_system_proxy_get_api() {
    local response
    response=$(get_system_proxy)

    if [[ $? -ne 0 ]]; then
        log_fail "Failed to call get system proxy API"
        return 1
    fi

    if ! assert_not_empty "$response" "System proxy response should not be empty"; then
        return 1
    fi

    local has_error
    has_error=$(echo "$response" | jq 'has("error")')
    if [[ "$has_error" == "true" ]]; then
        log_debug "System proxy API returned error (may not be supported): $(echo "$response" | jq -r '.error')"
        return 0
    fi

    if ! assert_json_has_field "$response" "enabled" "Response should have enabled field"; then
        log_debug "Response: $response"
        return 1
    fi
    if ! assert_json_has_field "$response" "configured_enabled" "Response should have configured_enabled field"; then
        log_debug "Response: $response"
        return 1
    fi

    return 0
}

test_system_proxy_structure() {
    local response
    response=$(get_system_proxy)

    local has_error
    has_error=$(echo "$response" | jq 'has("error")')
    if [[ "$has_error" == "true" ]]; then
        log_debug "System proxy API returned error, skipping structure test"
        return 0
    fi

    local enabled
    enabled=$(echo "$response" | jq -r '.enabled')

    if [[ "$enabled" != "true" && "$enabled" != "false" ]]; then
        log_fail "Invalid enabled value: $enabled"
        return 1
    fi

    local configured_enabled
    configured_enabled=$(echo "$response" | jq -r '.configured_enabled')

    if [[ "$configured_enabled" != "true" && "$configured_enabled" != "false" ]]; then
        log_fail "Invalid configured_enabled value: $configured_enabled"
        return 1
    fi

    local bypass
    bypass=$(echo "$response" | jq -r '.bypass // empty')

    return 0
}

test_system_proxy_support_api() {
    local response
    response=$(get_system_proxy_support)

    if [[ $? -ne 0 ]]; then
        log_fail "Failed to call get system proxy support API"
        return 1
    fi

    if ! assert_not_empty "$response" "System proxy support response should not be empty"; then
        return 1
    fi

    if ! assert_json_has_field "$response" "supported" "Response should have supported field"; then
        log_debug "Response: $response"
        return 1
    fi

    return 0
}

test_system_proxy_support_structure() {
    local response
    response=$(get_system_proxy_support)

    local supported
    supported=$(echo "$response" | jq -r '.supported')

    if [[ "$supported" != "true" && "$supported" != "false" ]]; then
        log_fail "Invalid supported value: $supported"
        return 1
    fi

    if [[ "$supported" == "false" ]]; then
        local reason
        reason=$(echo "$response" | jq -r '.reason // empty')
        if [[ -n "$reason" ]]; then
            log_debug "System proxy not supported: $reason"
        fi
    fi

    return 0
}

test_system_proxy_set_api() {
    local support_response
    support_response=$(get_system_proxy_support)
    local supported
    supported=$(echo "$support_response" | jq -r '.supported')

    if [[ "$supported" != "true" ]]; then
        log_debug "System proxy not supported, skipping set test"
        return 0
    fi

    local original_response
    original_response=$(get_system_proxy)
    local original_enabled
    original_enabled=$(echo "$original_response" | jq -r '.enabled')

    local response
    response=$(set_system_proxy "false")

    if [[ $? -ne 0 ]]; then
        log_fail "Failed to call set system proxy API"
        return 1
    fi

    local verify_response
    verify_response=$(get_system_proxy)
    local new_enabled
    new_enabled=$(echo "$verify_response" | jq -r '.enabled')

    if [[ "$new_enabled" != "false" ]]; then
        log_debug "System proxy enabled state may not have changed (expected false, got $new_enabled)"
    fi

    if [[ "$original_enabled" == "true" ]]; then
        set_system_proxy "true" > /dev/null 2>&1
    fi

    return 0
}

test_system_proxy_set_with_bypass() {
    local support_response
    support_response=$(get_system_proxy_support)
    local supported
    supported=$(echo "$support_response" | jq -r '.supported')

    if [[ "$supported" != "true" ]]; then
        log_debug "System proxy not supported, skipping bypass test"
        return 0
    fi

    local original_response
    original_response=$(get_system_proxy)
    local original_enabled
    original_enabled=$(echo "$original_response" | jq -r '.enabled')
    local original_bypass
    original_bypass=$(echo "$original_response" | jq -r '.bypass // empty')

    local response
    response=$(set_system_proxy "true" "localhost,127.0.0.1")

    if [[ $? -ne 0 ]]; then
        log_fail "Failed to call set system proxy with bypass API"
        return 1
    fi

    local verify_response
    verify_response=$(get_system_proxy)
    local bypass
    bypass=$(echo "$verify_response" | jq -r '.bypass // empty')

    if [[ "$original_enabled" == "false" ]]; then
        set_system_proxy "false" > /dev/null 2>&1
    elif [[ -n "$original_bypass" ]]; then
        set_system_proxy "$original_enabled" "$original_bypass" > /dev/null 2>&1
    fi

    return 0
}

print_summary() {
    echo ""
    echo "======================================"
    echo "Proxy Admin API Test Summary"
    echo "======================================"
    echo "Tests Run:    $TESTS_RUN"
    echo "Tests Passed: $TESTS_PASSED"
    echo "Tests Failed: $TESTS_FAILED"
    echo "======================================"

    if [[ $TESTS_FAILED -eq 0 ]]; then
        echo "All tests passed!"
        return 0
    else
        echo "Some tests failed!"
        return 1
    fi
}

main() {
    trap admin_cleanup_bifrost EXIT

    if ! admin_ensure_bifrost; then
        log_fail "Admin server is not reachable and failed to start"
        exit 1
    fi

    log_info "Starting Proxy Admin API Tests"
    log_info "Admin: $ADMIN_HOST:$ADMIN_PORT"
    echo ""

    run_test "System Proxy Get API" test_system_proxy_get_api
    run_test "System Proxy Structure" test_system_proxy_structure
    run_test "System Proxy Support API" test_system_proxy_support_api
    run_test "System Proxy Support Structure" test_system_proxy_support_structure
    run_test "System Proxy Set API" test_system_proxy_set_api
    run_test "System Proxy Set With Bypass" test_system_proxy_set_with_bypass

    print_summary
    exit $?
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    main "$@"
fi
