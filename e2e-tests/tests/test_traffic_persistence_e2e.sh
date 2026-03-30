#!/bin/bash

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test_utils/admin_client.sh"
source "$SCRIPT_DIR/../test_utils/process.sh"

ADMIN_HOST="${ADMIN_HOST:-127.0.0.1}"
ADMIN_PORT="${ADMIN_PORT:-19900}"
PROXY_PORT="${PROXY_PORT:-19900}"
ADMIN_PATH_PREFIX="${ADMIN_PATH_PREFIX:-/_bifrost}"
ADMIN_BASE_URL="http://${ADMIN_HOST}:${ADMIN_PORT}${ADMIN_PATH_PREFIX}"

TEST_DATA_DIR="${TEST_DATA_DIR:-./.bifrost-persistence-test}"
MOCK_HTTP_PORT="${MOCK_HTTP_PORT:-3198}"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
BIFROST_BIN="${PROJECT_ROOT}/target/release/bifrost"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi

TESTS_RUN=0
TESTS_PASSED=0
TESTS_FAILED=0

log_info() { echo "[INFO] $*"; }
log_pass() { echo -e "\033[0;32m[PASS]\033[0m $*"; }
log_fail() { echo -e "\033[0;31m[FAIL]\033[0m $*"; }
log_debug() { [[ "${DEBUG:-0}" == "1" ]] && echo "[DEBUG] $*"; }
log_warn() { echo -e "\033[1;33m[WARN]\033[0m $*"; }

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

assert_greater_than() {
    local actual="$1"
    local threshold="$2"
    local msg="${3:-Value should be greater than threshold}"

    if [[ "$actual" -gt "$threshold" ]]; then
        return 0
    else
        log_fail "$msg: expected > $threshold, got $actual"
        return 1
    fi
}

assert_greater_equal() {
    local actual="$1"
    local threshold="$2"
    local msg="${3:-Value should be greater than or equal to threshold}"

    if [[ "$actual" -ge "$threshold" ]]; then
        return 0
    else
        log_fail "$msg: expected >= $threshold, got $actual"
        return 1
    fi
}

run_test() {
    local test_name="$1"
    local test_func="$2"

    TESTS_RUN=$((TESTS_RUN + 1))
    echo ""
    log_info "===================="
    log_info "Running test: $test_name"
    log_info "===================="

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

PROXY_PID=""
MOCK_PID=""

start_proxy() {
    log_info "Starting Bifrost proxy on port ${PROXY_PORT}..."
    
    rm -rf "$TEST_DATA_DIR"
    mkdir -p "$TEST_DATA_DIR"
    
    BIFROST_DATA_DIR="$TEST_DATA_DIR" $BIFROST_BIN start -p "$PROXY_PORT" --skip-cert-check > "$TEST_DATA_DIR/proxy.log" 2>&1 &
    PROXY_PID=$!
    
    local max_wait=30
    if is_windows; then max_wait=60; fi
    local waited=0
    while [[ $waited -lt $max_wait ]]; do
        if ! kill -0 "$PROXY_PID" 2>/dev/null; then
            log_fail "Proxy process exited unexpectedly (PID: $PROXY_PID)"
            cat "$TEST_DATA_DIR/proxy.log" || true
            return 1
        fi
        if curl -sS -o /dev/null -w "" "${ADMIN_BASE_URL}/api/traffic?limit=1" 2>/dev/null; then
            log_info "Proxy started successfully (PID: $PROXY_PID)"
            return 0
        fi
        sleep 1
        ((waited++))
    done
    
    log_fail "Failed to start proxy within ${max_wait} seconds"
    cat "$TEST_DATA_DIR/proxy.log" || true
    return 1
}

stop_proxy() {
    if [[ -n "$PROXY_PID" ]]; then
        log_info "Stopping proxy (PID: $PROXY_PID)..."
        safe_cleanup_proxy "$PROXY_PID"
        PROXY_PID=""
    fi
    if is_windows; then
        kill_bifrost_on_port "$PROXY_PORT"
        win_wait_port_free "$PROXY_PORT" 30 || true
    fi
    sleep 2
}

restart_proxy() {
    stop_proxy
    
    log_info "Restarting Bifrost proxy..."
    
    BIFROST_DATA_DIR="$TEST_DATA_DIR" $BIFROST_BIN start -p "$PROXY_PORT" --skip-cert-check >> "$TEST_DATA_DIR/proxy.log" 2>&1 &
    PROXY_PID=$!
    
    local max_wait=30
    if is_windows; then max_wait=60; fi
    local waited=0
    while [[ $waited -lt $max_wait ]]; do
        if ! kill -0 "$PROXY_PID" 2>/dev/null; then
            log_fail "Proxy process exited unexpectedly during restart (PID: $PROXY_PID)"
            cat "$TEST_DATA_DIR/proxy.log" || true
            return 1
        fi
        if curl -sS -o /dev/null -w "" "${ADMIN_BASE_URL}/api/traffic?limit=1" 2>/dev/null; then
            log_info "Proxy restarted successfully (PID: $PROXY_PID)"
            return 0
        fi
        sleep 1
        ((waited++))
    done
    
    log_fail "Failed to restart proxy within ${max_wait} seconds"
    cat "$TEST_DATA_DIR/proxy.log" || true
    return 1
}

cleanup() {
    stop_mock_server
    stop_proxy
    if is_windows; then kill_bifrost_on_port "$PROXY_PORT"; fi
    log_info "Cleaning up test data directory..."
    rm -rf "$TEST_DATA_DIR"
}

trap cleanup EXIT

start_mock_server() {
    python3 "$SCRIPT_DIR/../mock_servers/http_echo_server.py" "$MOCK_HTTP_PORT" >/dev/null 2>&1 &
    MOCK_PID=$!
    local waited=0
    while [[ $waited -lt 10 ]]; do
        if curl -sS -o /dev/null -w "" "http://127.0.0.1:${MOCK_HTTP_PORT}/get" 2>/dev/null; then
            return 0
        fi
        sleep 1
        ((waited++))
    done
    return 1
}

stop_mock_server() {
    if [[ -n "$MOCK_PID" ]]; then
        kill_pid "$MOCK_PID"
        wait_pid "$MOCK_PID"
        MOCK_PID=""
    fi
}

generate_traffic() {
    local count="${1:-3}"
    local pids=()
    
    log_info "Generating $count traffic records..."
    
    for i in $(seq 1 "$count"); do
        curl -sS --proxy "http://127.0.0.1:${PROXY_PORT}" \
            --connect-timeout 5 --max-time 10 \
            "http://127.0.0.1:${MOCK_HTTP_PORT}/get?test_id=persistence_test_$$_${i}" \
            -o /dev/null 2>/dev/null &
        pids+=($!)
    done
    for pid in "${pids[@]}"; do
        wait "$pid" 2>/dev/null || true
    done
    sleep 2
}

test_data_persistence_after_restart() {
    log_info "Testing data persistence after proxy restart..."
    
    generate_traffic 5
    
    local before_restart
    before_restart=$(curl -sS "${ADMIN_BASE_URL}/api/traffic?limit=100")
    
    local before_count
    before_count=$(echo "$before_restart" | jq -r '.total // 0')
    local before_seq
    before_seq=$(curl -sS "${ADMIN_BASE_URL}/api/traffic/updates?limit=1" | jq -r '.server_sequence // 0')
    
    log_info "Before restart - Total records: $before_count, Sequence: $before_seq"
    
    if [[ "$before_count" -eq 0 ]]; then
        log_fail "No records before restart"
        return 1
    fi
    
    local sample_id
    sample_id=$(echo "$before_restart" | jq -r '.records[0].id // empty')
    log_info "Sample record ID: $sample_id"
    
    restart_proxy || return 1
    
    local after_restart
    after_restart=$(curl -sS "${ADMIN_BASE_URL}/api/traffic?limit=100")
    
    local after_count
    after_count=$(echo "$after_restart" | jq -r '.total // 0')
    local after_seq
    after_seq=$(curl -sS "${ADMIN_BASE_URL}/api/traffic/updates?limit=1" | jq -r '.server_sequence // 0')
    
    log_info "After restart - Total records: $after_count, Sequence: $after_seq"
    
    assert_equals "$before_count" "$after_count" "Record count should be preserved after restart" || return 1
    
    assert_greater_equal "$after_seq" "$before_seq" "Sequence should be preserved or continue after restart" || return 1
    
    if [[ -n "$sample_id" ]]; then
        local sample_exists
        sample_exists=$(echo "$after_restart" | jq -r ".records[] | select(.id == \"$sample_id\") | .id")
        
        if [[ "$sample_exists" == "$sample_id" ]]; then
            log_info "Sample record $sample_id found after restart"
        else
            log_fail "Sample record $sample_id not found after restart"
            return 1
        fi
    fi
    
    return 0
}

test_sequence_continuity() {
    log_info "Testing sequence continuity after restart..."
    
    local seq_before
    seq_before=$(curl -sS "${ADMIN_BASE_URL}/api/traffic/updates?limit=1" | jq -r '.server_sequence // 0')
    
    generate_traffic 3
    
    local seq_after_traffic
    seq_after_traffic=$(curl -sS "${ADMIN_BASE_URL}/api/traffic/updates?limit=1" | jq -r '.server_sequence // 0')
    
    log_info "Sequence before traffic: $seq_before, after traffic: $seq_after_traffic"
    
    restart_proxy || return 1
    
    local seq_after_restart
    seq_after_restart=$(curl -sS "${ADMIN_BASE_URL}/api/traffic/updates?limit=1" | jq -r '.server_sequence // 0')
    
    log_info "Sequence after restart: $seq_after_restart"
    
    assert_greater_equal "$seq_after_restart" "$seq_after_traffic" "Sequence should continue from where it left off" || return 1
    
    generate_traffic 2
    
    local seq_final
    seq_final=$(curl -sS "${ADMIN_BASE_URL}/api/traffic/updates?limit=1" | jq -r '.server_sequence // 0')
    
    log_info "Final sequence after new traffic: $seq_final"
    
    assert_greater_than "$seq_final" "$seq_after_restart" "Sequence should continue increasing after restart" || return 1
    
    return 0
}

test_incremental_updates_after_restart() {
    log_info "Testing incremental updates work correctly after restart..."
    
    local seq_before
    seq_before=$(curl -sS "${ADMIN_BASE_URL}/api/traffic/updates?limit=1" | jq -r '.server_sequence // 0')
    
    restart_proxy || return 1
    
    generate_traffic 5
    
    local response
    response=$(curl -sS "${ADMIN_BASE_URL}/api/traffic/updates?after_seq=${seq_before}&limit=100")
    
    local new_count
    new_count=$(echo "$response" | jq -r '.new_records | length')
    local has_more
    has_more=$(echo "$response" | jq -r '.has_more')
    
    log_info "Incremental query after restart: $new_count new records, has_more: $has_more"
    
    assert_greater_than "$new_count" 0 "Should have new records in incremental query after restart" || return 1
    
    return 0
}

test_detail_retrieval_after_restart() {
    log_info "Testing detail retrieval after restart..."
    
    generate_traffic 1
    
    local list_response
    list_response=$(curl -sS "${ADMIN_BASE_URL}/api/traffic?limit=5")
    
    local record_id
    record_id=$(echo "$list_response" | jq -r '.records[0].id // empty')
    
    if [[ -z "$record_id" ]]; then
        log_warn "No records available for detail test"
        return 0
    fi
    
    log_info "Testing detail retrieval for record: $record_id"
    
    local detail_before
    detail_before=$(curl -sS "${ADMIN_BASE_URL}/api/traffic/${record_id}")
    
    local host_before
    host_before=$(echo "$detail_before" | jq -r '.host // empty')
    local method_before
    method_before=$(echo "$detail_before" | jq -r '.method // empty')
    
    log_info "Before restart - Host: $host_before, Method: $method_before"
    
    restart_proxy || return 1
    
    local detail_after
    detail_after=$(curl -sS "${ADMIN_BASE_URL}/api/traffic/${record_id}")
    
    local host_after
    host_after=$(echo "$detail_after" | jq -r '.host // empty')
    local method_after
    method_after=$(echo "$detail_after" | jq -r '.method // empty')
    
    log_info "After restart - Host: $host_after, Method: $method_after"
    
    assert_equals "$host_before" "$host_after" "Host should be preserved" || return 1
    assert_equals "$method_before" "$method_after" "Method should be preserved" || return 1
    
    return 0
}

test_clear_and_restart() {
    log_info "Testing clear and restart behavior..."
    
    generate_traffic 5
    
    local before_clear
    before_clear=$(curl -sS "${ADMIN_BASE_URL}/api/traffic?limit=5" | jq -r '.total // 0')
    log_info "Before clear: $before_clear records"
    
    log_info "Clearing traffic..."
    curl -sS -X DELETE "${ADMIN_BASE_URL}/api/traffic" > /dev/null
    
    sleep 1
    
    local after_clear
    after_clear=$(curl -sS "${ADMIN_BASE_URL}/api/traffic?limit=5" | jq -r '.total // 0')
    log_info "After clear: $after_clear records"
    
    assert_equals "0" "$after_clear" "Should have no records after clear" || return 1
    
    restart_proxy || return 1
    
    local after_restart
    after_restart=$(curl -sS "${ADMIN_BASE_URL}/api/traffic?limit=5" | jq -r '.total // 0')
    log_info "After restart: $after_restart records"
    
    assert_equals "0" "$after_restart" "Should still have no records after restart" || return 1
    
    generate_traffic 3
    
    local after_new_traffic
    after_new_traffic=$(curl -sS "${ADMIN_BASE_URL}/api/traffic?limit=5" | jq -r '.total // 0')
    log_info "After new traffic: $after_new_traffic records"
    
    assert_greater_than "$after_new_traffic" 0 "Should have new records after generating traffic" || return 1
    
    return 0
}

main() {
    echo "=========================================="
    echo "  Traffic Persistence E2E Tests"
    echo "=========================================="
    echo "Admin URL: ${ADMIN_BASE_URL}"
    echo "Proxy Port: ${PROXY_PORT}"
    echo "Data Dir: ${TEST_DATA_DIR}"
    echo "=========================================="

    start_proxy || exit 1
    start_mock_server || { log_fail "Could not start mock server"; exit 1; }

    run_test "Data Persistence After Restart" test_data_persistence_after_restart
    run_test "Sequence Continuity" test_sequence_continuity
    run_test "Incremental Updates After Restart" test_incremental_updates_after_restart
    run_test "Detail Retrieval After Restart" test_detail_retrieval_after_restart
    run_test "Clear and Restart" test_clear_and_restart

    echo ""
    echo "=========================================="
    echo "  Test Results"
    echo "=========================================="
    echo "Total:  $TESTS_RUN"
    echo -e "\033[0;32mPassed: $TESTS_PASSED\033[0m"
    if [[ $TESTS_FAILED -gt 0 ]]; then
        echo -e "\033[0;31mFailed: $TESTS_FAILED\033[0m"
    else
        echo "Failed: $TESTS_FAILED"
    fi
    echo "=========================================="

    if [[ $TESTS_FAILED -gt 0 ]]; then
        exit 1
    fi
    exit 0
}

main "$@"
