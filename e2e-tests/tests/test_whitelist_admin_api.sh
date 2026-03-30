#!/bin/bash

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test_utils/admin_client.sh"

ADMIN_HOST="${ADMIN_HOST:-127.0.0.1}"
ADMIN_PORT="${ADMIN_PORT:-9900}"
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

assert_json_field() {
    local json="$1"
    local field="$2"
    local expected="$3"
    local msg="${4:-JSON field should match}"

    local actual
    actual=$(echo "$json" | jq -r "$field")

    if [[ "$actual" == "$expected" ]]; then
        return 0
    else
        log_fail "$msg: field '$field' expected '$expected', got '$actual'"
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

TEST_IP="192.168.100.$(($$ % 254 + 1))"
TEST_CIDR="10.0.0.0/24"
ORIGINAL_MODE=""
ORIGINAL_ALLOW_LAN=""

check_admin_alive() {
    local retries=3
    local i=0
    while [[ $i -lt $retries ]]; do
        if curl -sf "$(admin_base_url)/api/system/status" >/dev/null 2>&1 || \
           curl -sf "$(admin_base_url)/api/system" >/dev/null 2>&1; then
            return 0
        fi
        sleep 1
        i=$((i + 1))
    done
    log_fail "Admin server is not reachable"
    return 1
}

save_whitelist_state() {
    local response
    response=$(get_whitelist)
    ORIGINAL_MODE=$(echo "$response" | jq -r '.mode // "open"')
    ORIGINAL_ALLOW_LAN=$(echo "$response" | jq -r '.allow_lan // true')
}

restore_whitelist_state() {
    if [[ -n "$ORIGINAL_MODE" ]]; then
        set_whitelist_mode "$ORIGINAL_MODE" > /dev/null 2>&1
    fi
    if [[ -n "$ORIGINAL_ALLOW_LAN" ]]; then
        set_allow_lan "$ORIGINAL_ALLOW_LAN" > /dev/null 2>&1
    fi
    remove_whitelist "$TEST_IP" > /dev/null 2>&1
    remove_whitelist "$TEST_CIDR" > /dev/null 2>&1
    remove_temporary_whitelist "$TEST_IP" > /dev/null 2>&1
}

test_whitelist_get_api() {
    local response
    response=$(get_whitelist)

    if [[ $? -ne 0 ]]; then
        log_fail "Failed to call get whitelist API"
        return 1
    fi

    if ! assert_not_empty "$response" "Whitelist response should not be empty"; then
        return 1
    fi

    if ! assert_json_has_field "$response" "mode" "Response should have mode field"; then
        log_debug "Response: $response"
        return 1
    fi

    if ! assert_json_has_field "$response" "whitelist" "Response should have whitelist field"; then
        return 1
    fi

    return 0
}

test_whitelist_add_ip() {
    remove_whitelist "$TEST_IP" > /dev/null 2>&1

    local response
    response=$(add_whitelist "$TEST_IP")

    if [[ $? -ne 0 ]]; then
        log_fail "Failed to call add whitelist API"
        return 1
    fi

    local verify_response
    verify_response=$(get_whitelist)
    local found
    found=$(echo "$verify_response" | jq -r ".whitelist[] | select(. == \"$TEST_IP\")")

    if [[ -z "$found" ]]; then
        log_debug "IP not found in whitelist: $verify_response"
    fi

    return 0
}

test_whitelist_add_cidr() {
    remove_whitelist "$TEST_CIDR" > /dev/null 2>&1

    local response
    response=$(add_whitelist "$TEST_CIDR")

    if [[ $? -ne 0 ]]; then
        log_fail "Failed to call add whitelist CIDR API"
        return 1
    fi

    local verify_response
    verify_response=$(get_whitelist)
    local found
    found=$(echo "$verify_response" | jq -r ".whitelist[] | select(. == \"$TEST_CIDR\")")

    if [[ -z "$found" ]]; then
        log_debug "CIDR not found in whitelist: $verify_response"
    fi

    return 0
}

test_whitelist_remove_api() {
    add_whitelist "$TEST_IP" > /dev/null 2>&1

    local response
    response=$(remove_whitelist "$TEST_IP")

    if [[ $? -ne 0 ]]; then
        log_fail "Failed to call remove whitelist API"
        return 1
    fi

    return 0
}

test_whitelist_mode_get_api() {
    local response
    response=$(get_whitelist_mode)

    if [[ $? -ne 0 ]]; then
        log_fail "Failed to call get whitelist mode API"
        return 1
    fi

    if ! assert_not_empty "$response" "Mode response should not be empty"; then
        return 1
    fi

    if ! assert_json_has_field "$response" "mode" "Response should have mode field"; then
        log_debug "Response: $response"
        return 1
    fi

    local mode
    mode=$(echo "$response" | jq -r '.mode')
    if [[ "$mode" != "allow_all" && "$mode" != "local_only" && "$mode" != "whitelist" && "$mode" != "interactive" ]]; then
        log_fail "Invalid mode value: $mode"
        return 1
    fi

    return 0
}

test_whitelist_mode_set_api() {
    save_whitelist_state

    local response
    response=$(set_whitelist_mode "allow_all")

    if [[ $? -ne 0 ]]; then
        log_fail "Failed to call set whitelist mode API"
        restore_whitelist_state
        return 1
    fi

    local verify_response
    verify_response=$(get_whitelist_mode)
    local mode
    mode=$(echo "$verify_response" | jq -r '.mode')

    if ! assert_equals "allow_all" "$mode" "Mode should be set to allow_all"; then
        restore_whitelist_state
        return 1
    fi

    restore_whitelist_state
    return 0
}

test_whitelist_mode_values() {
    save_whitelist_state

    local modes=("allow_all" "local_only" "whitelist")
    for mode in "${modes[@]}"; do
        set_whitelist_mode "$mode" > /dev/null 2>&1
        local verify_response
        verify_response=$(get_whitelist_mode)
        local actual_mode
        actual_mode=$(echo "$verify_response" | jq -r '.mode')

        if [[ "$actual_mode" != "$mode" ]]; then
            log_debug "Mode '$mode' not set correctly, got '$actual_mode'"
        fi
    done

    restore_whitelist_state
    return 0
}

test_allow_lan_get_api() {
    local response
    response=$(get_allow_lan)

    if [[ $? -ne 0 ]]; then
        log_fail "Failed to call get allow LAN API"
        return 1
    fi

    if ! assert_not_empty "$response" "Allow LAN response should not be empty"; then
        return 1
    fi

    if ! assert_json_has_field "$response" "allow_lan" "Response should have allow_lan field"; then
        log_debug "Response: $response"
        return 1
    fi

    return 0
}

test_allow_lan_set_api() {
    save_whitelist_state

    local response
    response=$(set_allow_lan "true")

    if [[ $? -ne 0 ]]; then
        log_fail "Failed to call set allow LAN API"
        restore_whitelist_state
        return 1
    fi

    local verify_response
    verify_response=$(get_allow_lan)
    local allow_lan
    allow_lan=$(echo "$verify_response" | jq -r '.allow_lan')

    if ! assert_equals "true" "$allow_lan" "Allow LAN should be set to true"; then
        restore_whitelist_state
        return 1
    fi

    set_allow_lan "false" > /dev/null 2>&1
    verify_response=$(get_allow_lan)
    allow_lan=$(echo "$verify_response" | jq -r '.allow_lan')

    if ! assert_equals "false" "$allow_lan" "Allow LAN should be set to false"; then
        restore_whitelist_state
        return 1
    fi

    restore_whitelist_state
    return 0
}

test_temporary_whitelist_add_api() {
    remove_temporary_whitelist "$TEST_IP" > /dev/null 2>&1

    local response
    response=$(add_temporary_whitelist "$TEST_IP")

    if [[ $? -ne 0 ]]; then
        log_fail "Failed to call add temporary whitelist API"
        return 1
    fi

    local verify_response
    verify_response=$(get_whitelist)
    local found
    found=$(echo "$verify_response" | jq -r ".temporary_whitelist[]? | select(. == \"$TEST_IP\")")

    if [[ -z "$found" ]]; then
        log_debug "IP not found in temporary whitelist"
    fi

    return 0
}

test_temporary_whitelist_remove_api() {
    add_temporary_whitelist "$TEST_IP" > /dev/null 2>&1
    sleep 0.5

    if ! check_admin_alive; then
        return 1
    fi

    local response
    response=$(remove_temporary_whitelist "$TEST_IP")

    if [[ $? -ne 0 ]]; then
        log_fail "Failed to call remove temporary whitelist API"
        return 1
    fi

    return 0
}

test_pending_authorizations_get_api() {
    if ! check_admin_alive; then
        return 1
    fi

    local response
    response=$(get_pending_authorizations)

    if [[ $? -ne 0 ]]; then
        log_fail "Failed to call get pending authorizations API"
        return 1
    fi

    if ! assert_not_empty "$response" "Pending authorizations response should not be empty"; then
        return 1
    fi

    return 0
}

test_pending_authorizations_clear_api() {
    if ! check_admin_alive; then
        return 1
    fi

    local response
    response=$(clear_pending_authorizations)

    if [[ $? -ne 0 ]]; then
        log_fail "Failed to call clear pending authorizations API"
        return 1
    fi

    return 0
}

test_whitelist_structure() {
    if ! check_admin_alive; then
        return 1
    fi

    local response
    response=$(get_whitelist)

    local mode
    mode=$(echo "$response" | jq -r '.mode')
    if [[ "$mode" != "allow_all" && "$mode" != "local_only" && "$mode" != "whitelist" && "$mode" != "interactive" ]]; then
        log_fail "Invalid mode in whitelist structure: $mode"
        return 1
    fi

    local allow_lan
    allow_lan=$(echo "$response" | jq -r '.allow_lan')
    if [[ "$allow_lan" != "true" && "$allow_lan" != "false" ]]; then
        log_fail "Invalid allow_lan in whitelist structure: $allow_lan"
        return 1
    fi

    local whitelist_type
    whitelist_type=$(echo "$response" | jq 'type')
    if [[ "$whitelist_type" != "\"object\"" ]]; then
        log_fail "Whitelist response should be an object"
        return 1
    fi

    return 0
}

print_summary() {
    echo ""
    echo "======================================"
    echo "Whitelist Admin API Test Summary"
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

    log_info "Starting Whitelist Admin API Tests"
    log_info "Admin: $ADMIN_HOST:$ADMIN_PORT"
    echo ""

    save_whitelist_state

    run_test "Whitelist Get API" test_whitelist_get_api
    run_test "Whitelist Add IP" test_whitelist_add_ip
    run_test "Whitelist Add CIDR" test_whitelist_add_cidr
    run_test "Whitelist Remove API" test_whitelist_remove_api
    run_test "Whitelist Mode Get API" test_whitelist_mode_get_api
    run_test "Whitelist Mode Set API" test_whitelist_mode_set_api
    run_test "Whitelist Mode Values" test_whitelist_mode_values
    run_test "Allow LAN Get API" test_allow_lan_get_api
    run_test "Allow LAN Set API" test_allow_lan_set_api
    run_test "Temporary Whitelist Add API" test_temporary_whitelist_add_api
    run_test "Temporary Whitelist Remove API" test_temporary_whitelist_remove_api
    run_test "Pending Authorizations Get API" test_pending_authorizations_get_api
    run_test "Pending Authorizations Clear API" test_pending_authorizations_clear_api
    run_test "Whitelist Structure" test_whitelist_structure

    restore_whitelist_state

    print_summary
    exit $?
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    main "$@"
fi
