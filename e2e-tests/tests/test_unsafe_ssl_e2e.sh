#!/bin/bash
#
# 端到端测试: unsafe_ssl 配置动态切换
#
# 该测试验证：
# 1. 当 unsafe_ssl=false 时，访问自签名证书的 HTTPS 服务应该失败（证书验证失败）
# 2. 通过 API 切换 unsafe_ssl=true 后，同样的请求应该成功
# 3. 再次切换 unsafe_ssl=false，请求又应该失败
#

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test_utils/admin_client.sh"
source "$SCRIPT_DIR/../test_utils/assert.sh"

ADMIN_HOST="${ADMIN_HOST:-127.0.0.1}"
ADMIN_PORT="${ADMIN_PORT:-9900}"
PROXY_PORT="${PROXY_PORT:-9900}"
HTTPS_MOCK_PORT="${HTTPS_MOCK_PORT:-3443}"
ADMIN_PATH_PREFIX="${ADMIN_PATH_PREFIX:-/_bifrost}"
export ADMIN_PATH_PREFIX

TESTS_RUN=0
TESTS_PASSED=0
TESTS_FAILED=0

log_info() { echo "[INFO] $*"; }
log_pass() { echo "[PASS] $*"; }
log_fail() { echo "[FAIL] $*"; }
log_debug() { [[ "${DEBUG:-0}" == "1" ]] && echo "[DEBUG] $*"; }

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

check_proxy_available() {
    if ! curl -s --connect-timeout 3 "http://${ADMIN_HOST}:${ADMIN_PORT}${ADMIN_PATH_PREFIX}/api/system/status" > /dev/null 2>&1; then
        log_fail "Proxy server not available at ${ADMIN_HOST}:${ADMIN_PORT}"
        return 1
    fi
    return 0
}

check_mock_server_available() {
    if ! nc -z 127.0.0.1 "$HTTPS_MOCK_PORT" 2>/dev/null; then
        log_fail "HTTPS mock server not available at port $HTTPS_MOCK_PORT"
        return 1
    fi
    return 0
}

request_via_proxy_https() {
    local url="$1"
    local timeout="${2:-10}"
    
    curl -s -k --connect-timeout "$timeout" \
         --max-time "$timeout" \
         --proxy "http://${ADMIN_HOST}:${PROXY_PORT}" \
         --write-out "\n%{http_code}" \
         "$url" 2>&1
}

test_initial_state() {
    local current_unsafe_ssl
    current_unsafe_ssl=$(get_unsafe_ssl)
    
    log_debug "Current unsafe_ssl setting: $current_unsafe_ssl"
    
    if [[ "$current_unsafe_ssl" == "true" || "$current_unsafe_ssl" == "false" ]]; then
        return 0
    else
        log_fail "Failed to get unsafe_ssl setting, got: $current_unsafe_ssl"
        return 1
    fi
}

test_unsafe_ssl_false_should_fail() {
    log_debug "Setting unsafe_ssl to false..."
    local response
    response=$(set_unsafe_ssl "false")
    log_debug "Set unsafe_ssl response: $response"
    
    sleep 0.5
    
    local current_unsafe_ssl
    current_unsafe_ssl=$(get_unsafe_ssl)
    log_debug "Verified unsafe_ssl is now: $current_unsafe_ssl"
    
    if [[ "$current_unsafe_ssl" != "false" ]]; then
        log_fail "Failed to set unsafe_ssl to false, current value: $current_unsafe_ssl"
        return 1
    fi
    
    log_debug "Requesting HTTPS mock server via proxy (should fail due to cert validation)..."
    local result
    result=$(request_via_proxy_https "https://127.0.0.1:${HTTPS_MOCK_PORT}/echo" 10)
    
    log_debug "Request result: $result"
    
    local http_code
    http_code=$(echo "$result" | tail -1)
    
    if [[ "$http_code" == "200" ]]; then
        log_fail "Request succeeded but should have failed (unsafe_ssl=false)"
        log_debug "Response body: $(echo "$result" | head -n -1)"
        return 1
    fi
    
    if echo "$result" | grep -qi "certificate\|ssl\|tls\|502\|CONNECT\|error"; then
        log_debug "Got expected SSL/certificate error or 502 Bad Gateway"
        return 0
    fi
    
    if [[ "$http_code" == "502" ]]; then
        log_debug "Got 502 Bad Gateway (expected when SSL validation fails)"
        return 0
    fi
    
    log_debug "HTTP code: $http_code (non-200 is expected)"
    return 0
}

test_unsafe_ssl_true_should_succeed() {
    log_debug "Setting unsafe_ssl to true..."
    local response
    response=$(set_unsafe_ssl "true")
    log_debug "Set unsafe_ssl response: $response"
    
    sleep 0.5
    
    local current_unsafe_ssl
    current_unsafe_ssl=$(get_unsafe_ssl)
    log_debug "Verified unsafe_ssl is now: $current_unsafe_ssl"
    
    if [[ "$current_unsafe_ssl" != "true" ]]; then
        log_fail "Failed to set unsafe_ssl to true, current value: $current_unsafe_ssl"
        return 1
    fi
    
    log_debug "Requesting HTTPS mock server via proxy (should succeed with unsafe_ssl=true)..."
    local result
    result=$(request_via_proxy_https "https://127.0.0.1:${HTTPS_MOCK_PORT}/echo" 10)
    
    log_debug "Request result: $result"
    
    local http_code
    http_code=$(echo "$result" | tail -1)
    
    if [[ "$http_code" == "200" ]]; then
        log_debug "Request succeeded as expected"
        local body
        body=$(echo "$result" | head -n -1)
        if echo "$body" | grep -q "method\|path\|headers"; then
            log_debug "Got valid echo response from mock server"
            return 0
        fi
        return 0
    else
        log_fail "Request failed with HTTP code: $http_code (expected 200)"
        log_debug "Response: $(echo "$result" | head -n -1)"
        return 1
    fi
}

test_switch_back_to_false() {
    log_debug "Switching unsafe_ssl back to false..."
    local response
    response=$(set_unsafe_ssl "false")
    log_debug "Set unsafe_ssl response: $response"
    
    sleep 0.5
    
    local current_unsafe_ssl
    current_unsafe_ssl=$(get_unsafe_ssl)
    
    if [[ "$current_unsafe_ssl" != "false" ]]; then
        log_fail "Failed to switch unsafe_ssl back to false"
        return 1
    fi
    
    log_debug "Requesting HTTPS mock server via proxy (should fail again)..."
    local result
    result=$(request_via_proxy_https "https://127.0.0.1:${HTTPS_MOCK_PORT}/echo" 10)
    
    local http_code
    http_code=$(echo "$result" | tail -1)
    
    if [[ "$http_code" == "200" ]]; then
        log_fail "Request succeeded but should have failed after switching back to unsafe_ssl=false"
        return 1
    fi
    
    log_debug "Request failed as expected (HTTP code: $http_code)"
    return 0
}

test_config_persistence_in_session() {
    log_debug "Testing that config changes persist within session..."
    
    set_unsafe_ssl "true" > /dev/null
    sleep 0.3
    
    local check1
    check1=$(get_unsafe_ssl)
    
    set_unsafe_ssl "false" > /dev/null
    sleep 0.3
    
    local check2
    check2=$(get_unsafe_ssl)
    
    set_unsafe_ssl "true" > /dev/null
    sleep 0.3
    
    local check3
    check3=$(get_unsafe_ssl)
    
    if [[ "$check1" == "true" && "$check2" == "false" && "$check3" == "true" ]]; then
        log_debug "Config changes persist correctly: true->false->true"
        return 0
    else
        log_fail "Config changes not persisting: expected true/false/true, got $check1/$check2/$check3"
        return 1
    fi
}

cleanup() {
    log_info "Cleaning up: restoring unsafe_ssl to false..."
    set_unsafe_ssl "false" > /dev/null 2>&1
}

main() {
    echo "=========================================="
    echo "  Unsafe SSL Dynamic Switch E2E Tests"
    echo "=========================================="
    echo ""
    echo "Test Configuration:"
    echo "  Proxy: http://${ADMIN_HOST}:${PROXY_PORT}"
    echo "  Admin: http://${ADMIN_HOST}:${ADMIN_PORT}${ADMIN_PATH_PREFIX}"
    echo "  HTTPS Mock: https://127.0.0.1:${HTTPS_MOCK_PORT}"
    echo ""
    
    trap 'cleanup; admin_cleanup_bifrost' EXIT
    
    admin_ensure_bifrost || { echo "ERROR: Could not start Bifrost"; exit 1; }

    if ! check_proxy_available; then
        echo ""
        echo "ERROR: Proxy server is not available. Please start it first."
        exit 1
    fi
    
    if ! check_mock_server_available; then
        echo ""
        echo "WARNING: HTTPS mock server is not available at port $HTTPS_MOCK_PORT"
        echo "Some tests will be skipped."
        echo ""
    fi
    
    run_test "Initial state check" test_initial_state
    run_test "Config persistence in session" test_config_persistence_in_session
    
    if check_mock_server_available; then
        run_test "unsafe_ssl=false should reject self-signed cert" test_unsafe_ssl_false_should_fail
        run_test "unsafe_ssl=true should accept self-signed cert" test_unsafe_ssl_true_should_succeed
        run_test "Switch back to false should reject again" test_switch_back_to_false
    else
        log_info "Skipping mock server tests (mock server not available)"
    fi
    
    echo ""
    echo "=========================================="
    echo "  Results: $TESTS_PASSED/$TESTS_RUN passed"
    echo "=========================================="
    
    if [[ $TESTS_FAILED -gt 0 ]]; then
        echo "  Failed: $TESTS_FAILED"
        exit 1
    fi
    
    exit 0
}

main "$@"
