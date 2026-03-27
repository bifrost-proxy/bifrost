#!/bin/bash

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test_utils/admin_client.sh"

ADMIN_HOST="${ADMIN_HOST:-127.0.0.1}"
ADMIN_PORT="${ADMIN_PORT:-9900}"
ADMIN_PATH_PREFIX="${ADMIN_PATH_PREFIX:-/_bifrost}"
export ADMIN_PATH_PREFIX

trap admin_cleanup_bifrost EXIT

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

assert_json_field() {
    local json="$1"
    local field="$2"
    local expected="$3"
    local msg="${4:-JSON field should match}"

    local actual
    actual=$(echo "$json" | jq -r "$field" 2>/dev/null)

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

    log_info "Running: $test_name"
    ((TESTS_RUN++))

    if $test_func; then
        log_pass "$test_name"
        ((TESTS_PASSED++))
    else
        log_fail "$test_name"
        ((TESTS_FAILED++))
    fi
}

test_get_default_tls_config() {
    local response
    response=$(admin_get "/api/config/tls")

    assert_json_field "$response" ".enable_tls_interception | type" "boolean" "enable_tls_interception should be a boolean" || return 1
    assert_json_field "$response" ".intercept_exclude | type" "array" "intercept_exclude should be an array" || return 1
    assert_json_field "$response" ".intercept_include | type" "array" "intercept_include should be an array" || return 1

    return 0
}

test_update_include_list() {
    local response
    response=$(admin_put "/api/config/tls" '{"intercept_include": ["*.api.example.com", "test.local"]}')

    local include_list
    include_list=$(echo "$response" | jq -r '.intercept_include | length')

    if [[ "$include_list" == "2" ]]; then
        return 0
    else
        log_fail "Include list should have 2 items, got $include_list"
        return 1
    fi
}

test_update_exclude_list() {
    local response
    response=$(admin_put "/api/config/tls" '{"intercept_exclude": ["*.apple.com", "*.microsoft.com", "localhost"]}')

    local exclude_list
    exclude_list=$(echo "$response" | jq -r '.intercept_exclude | length')

    if [[ "$exclude_list" == "3" ]]; then
        return 0
    else
        log_fail "Exclude list should have 3 items, got $exclude_list"
        return 1
    fi
}

test_batch_update_lists_and_enable() {
    local response
    response=$(admin_put "/api/config/tls" '{
        "intercept_include": ["*.secure.com"],
        "intercept_exclude": ["*.example.net"],
        "enable_tls_interception": true
    }')

    assert_json_field "$response" ".enable_tls_interception" "true" "TLS interception should be enabled" || return 1

    local include_list
    include_list=$(echo "$response" | jq -r '.intercept_include | length')

    if [[ "$include_list" == "1" ]]; then
        return 0
    else
        log_fail "Include list should have 1 item, got $include_list"
        return 1
    fi
}

test_disable_tls_interception() {
    local response
    response=$(admin_put "/api/config/tls" '{"enable_tls_interception": false}')

    assert_json_field "$response" ".enable_tls_interception" "false" "TLS interception should be disabled" || return 1

    return 0
}

test_reenable_tls_interception() {
    local response
    response=$(admin_put "/api/config/tls" '{"enable_tls_interception": true}')

    assert_json_field "$response" ".enable_tls_interception" "true" "TLS interception should be re-enabled" || return 1

    return 0
}

cleanup_tls_config() {
    admin_put "/api/config/tls" '{
        "enable_tls_interception": false,
        "intercept_exclude": [],
        "intercept_include": [],
        "app_intercept_exclude": [],
        "app_intercept_include": []
    }' > /dev/null
}

main() {
    if ! admin_ensure_bifrost; then
        log_fail "Failed to start Bifrost admin server"
        exit 1
    fi

    echo "=============================================="
    echo "TLS Intercept Mode API Tests"
    echo "=============================================="
    echo ""

    run_test "Get default TLS config" test_get_default_tls_config
    run_test "Update include list" test_update_include_list
    run_test "Update exclude list" test_update_exclude_list
    run_test "Batch update lists and enable" test_batch_update_lists_and_enable
    run_test "Disable TLS interception" test_disable_tls_interception
    run_test "Re-enable TLS interception" test_reenable_tls_interception

    cleanup_tls_config

    echo ""
    echo "=============================================="
    echo "Test Results: $TESTS_PASSED/$TESTS_RUN passed"
    if [[ $TESTS_FAILED -gt 0 ]]; then
        echo "FAILED: $TESTS_FAILED tests"
        exit 1
    fi
    echo "=============================================="

    exit 0
}

main "$@"
