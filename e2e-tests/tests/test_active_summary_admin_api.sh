#!/bin/bash

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test_utils/admin_client.sh"

ADMIN_HOST="${ADMIN_HOST:-127.0.0.1}"
ADMIN_PORT="${ADMIN_PORT:-18821}"
ADMIN_PATH_PREFIX="${ADMIN_PATH_PREFIX:-/_bifrost}"
export ADMIN_HOST ADMIN_PORT ADMIN_PATH_PREFIX

TESTS_RUN=0
TESTS_PASSED=0
TESTS_FAILED=0

log_info() { echo "[INFO] $*"; }
log_pass() { echo -e "\033[0;32m[PASS]\033[0m $*"; }
log_fail() { echo -e "\033[0;31m[FAIL]\033[0m $*"; }
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

assert_contains() {
    local haystack="$1"
    local needle="$2"
    local msg="${3:-String should contain substring}"
    if [[ "$haystack" == *"$needle"* ]]; then
        return 0
    else
        log_fail "$msg: '$needle' not found"
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

run_test() {
    local test_name="$1"
    local test_func="$2"
    TESTS_RUN=$((TESTS_RUN + 1))
    echo ""
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

TEST_RULE_PREFIX="e2e-active-summary-$$"

cleanup_test_rules() {
    delete_rule "${TEST_RULE_PREFIX}-a" > /dev/null 2>&1
    delete_rule "${TEST_RULE_PREFIX}-b" > /dev/null 2>&1
    delete_rule "${TEST_RULE_PREFIX}-x" > /dev/null 2>&1
    delete_rule "${TEST_RULE_PREFIX}-y" > /dev/null 2>&1
}

test_active_summary_empty() {
    cleanup_test_rules

    local response
    response=$(admin_get "/api/rules/active-summary")

    if ! assert_not_empty "$response" "active-summary response should not be empty"; then
        return 1
    fi

    local total
    total=$(echo "$response" | jq -r '.total')
    if ! assert_equals "0" "$total" "Total should be 0 when no rules exist"; then
        log_debug "Response: $response"
        return 1
    fi

    local rules_count
    rules_count=$(echo "$response" | jq '.rules | length')
    if ! assert_equals "0" "$rules_count" "Rules array should be empty"; then
        return 1
    fi

    local merged
    merged=$(echo "$response" | jq -r '.merged_content')
    if [[ "$merged" != "" && "$merged" != "null" ]]; then
        log_fail "merged_content should be empty, got: '$merged'"
        return 1
    fi

    local conflicts_count
    conflicts_count=$(echo "$response" | jq '.variable_conflicts | length')
    if ! assert_equals "0" "$conflicts_count" "variable_conflicts should be empty"; then
        return 1
    fi

    return 0
}

test_active_summary_with_rules() {
    cleanup_test_rules

    create_rule "${TEST_RULE_PREFIX}-a" "example.com host://127.0.0.1:3000" "true" > /dev/null
    create_rule "${TEST_RULE_PREFIX}-b" "api.test.com host://127.0.0.1:4000" "true" > /dev/null

    sleep 0.5

    local response
    response=$(admin_get "/api/rules/active-summary")

    local total
    total=$(echo "$response" | jq -r '.total')
    if [[ "$total" -lt 2 ]]; then
        log_fail "Total should be >= 2, got $total"
        log_debug "Response: $response"
        cleanup_test_rules
        return 1
    fi

    local merged
    merged=$(echo "$response" | jq -r '.merged_content')
    if ! assert_contains "$merged" "example.com" "merged_content should contain example.com"; then
        cleanup_test_rules
        return 1
    fi
    if ! assert_contains "$merged" "api.test.com" "merged_content should contain api.test.com"; then
        cleanup_test_rules
        return 1
    fi

    local rules_count
    rules_count=$(echo "$response" | jq '.rules | length')
    if [[ "$rules_count" -lt 2 ]]; then
        log_fail "Rules array should have >= 2 elements, got $rules_count"
        cleanup_test_rules
        return 1
    fi

    cleanup_test_rules
    return 0
}

test_active_summary_variable_conflicts() {
    cleanup_test_rules

    local rule_x_content
    rule_x_content=$(printf 'example.com reqHeaders://{data}\n\n``` data\nx-env: prod\n```')
    local rule_y_content
    rule_y_content=$(printf 'example.com reqHeaders://{data}\n\n``` data\nx-env: staging\n```')

    create_rule "${TEST_RULE_PREFIX}-x" "$rule_x_content" "true" > /dev/null
    create_rule "${TEST_RULE_PREFIX}-y" "$rule_y_content" "true" > /dev/null

    sleep 0.5

    local response
    response=$(admin_get "/api/rules/active-summary")

    local conflicts_count
    conflicts_count=$(echo "$response" | jq '.variable_conflicts | length')

    if [[ "$conflicts_count" -lt 1 ]]; then
        log_fail "variable_conflicts should have at least 1 entry, got $conflicts_count"
        log_debug "Response: $response"
        cleanup_test_rules
        return 1
    fi

    local conflict_name
    conflict_name=$(echo "$response" | jq -r '.variable_conflicts[0].variable_name')
    if ! assert_equals "data" "$conflict_name" "Conflict variable_name should be 'data'"; then
        cleanup_test_rules
        return 1
    fi

    local defs_count
    defs_count=$(echo "$response" | jq '.variable_conflicts[0].definitions | length')
    if [[ "$defs_count" -lt 2 ]]; then
        log_fail "Conflict should have >= 2 definitions, got $defs_count"
        cleanup_test_rules
        return 1
    fi

    cleanup_test_rules
    return 0
}

test_active_summary_disabled_rule_not_counted() {
    cleanup_test_rules

    create_rule "${TEST_RULE_PREFIX}-a" "enabled.com host://127.0.0.1:3000" "true" > /dev/null
    create_rule "${TEST_RULE_PREFIX}-b" "disabled.com host://127.0.0.1:4000" "false" > /dev/null

    sleep 0.5

    local response
    response=$(admin_get "/api/rules/active-summary")

    local merged
    merged=$(echo "$response" | jq -r '.merged_content')
    if ! assert_contains "$merged" "enabled.com" "merged_content should contain enabled rule"; then
        cleanup_test_rules
        return 1
    fi

    if [[ "$merged" == *"disabled.com"* ]]; then
        log_fail "merged_content should NOT contain disabled rule, but found 'disabled.com'"
        cleanup_test_rules
        return 1
    fi

    cleanup_test_rules
    return 0
}

test_active_summary_merged_content_has_rule_count() {
    cleanup_test_rules

    create_rule "${TEST_RULE_PREFIX}-a" "$(printf 'a.com host://1.2.3.4\nb.com host://5.6.7.8')" "true" > /dev/null

    sleep 0.5

    local response
    response=$(admin_get "/api/rules/active-summary")

    local rule_count
    rule_count=$(echo "$response" | jq -r '.rules[0].rule_count // 0')

    if [[ "$rule_count" -lt 2 ]]; then
        log_fail "Rule file should report rule_count >= 2, got $rule_count"
        log_debug "Response: $response"
        cleanup_test_rules
        return 1
    fi

    cleanup_test_rules
    return 0
}

print_summary() {
    echo ""
    echo "======================================"
    echo "Active Summary Admin API Test Results"
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

    log_info "Starting Active Summary Admin API Tests"
    log_info "Admin: $ADMIN_HOST:$ADMIN_PORT"

    cleanup_test_rules

    run_test "Active Summary - Empty (no rules)" test_active_summary_empty
    run_test "Active Summary - With Rules and Merged Content" test_active_summary_with_rules
    run_test "Active Summary - Variable Conflicts Detection" test_active_summary_variable_conflicts
    run_test "Active Summary - Disabled Rule Not Counted" test_active_summary_disabled_rule_not_counted
    run_test "Active Summary - Merged Content Has Rule Count" test_active_summary_merged_content_has_rule_count

    cleanup_test_rules

    print_summary
    exit $?
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    main "$@"
fi
