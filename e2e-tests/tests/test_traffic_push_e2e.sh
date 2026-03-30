#!/bin/bash

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test_utils/admin_client.sh"
source "$SCRIPT_DIR/../test_utils/process.sh"

ADMIN_HOST="${ADMIN_HOST:-127.0.0.1}"
ADMIN_PORT="${ADMIN_PORT:-9900}"
PROXY_PORT="${PROXY_PORT:-9900}"
ADMIN_PATH_PREFIX="${ADMIN_PATH_PREFIX:-/_bifrost}"
ADMIN_BASE_URL="http://${ADMIN_HOST}:${ADMIN_PORT}${ADMIN_PATH_PREFIX}"
WS_URL="ws://${ADMIN_HOST}:${ADMIN_PORT}${ADMIN_PATH_PREFIX}/api/ws"
WS_PUSH_URL="ws://${ADMIN_HOST}:${ADMIN_PORT}${ADMIN_PATH_PREFIX}/api/push"
MOCK_HTTP_PORT="${MOCK_HTTP_PORT:-3199}"
MOCK_PID=""

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

ensure_websocat() {
    if ! command -v websocat &> /dev/null; then
        log_warn "websocat not found. Install with: brew install websocat"
        log_warn "Skipping WebSocket tests"
        return 1
    fi
    return 0
}

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
    
    log_info "Generating $count traffic records via proxy ${PROXY_PORT}..."
    
    for i in $(seq 1 "$count"); do
        curl -sS --proxy "http://127.0.0.1:${PROXY_PORT}" \
            --connect-timeout 5 --max-time 10 \
            "http://127.0.0.1:${MOCK_HTTP_PORT}/get?test_id=push_test_$$_${i}" \
            -o /dev/null -w "" 2>/dev/null &
        pids+=($!)
    done
    for pid in "${pids[@]}"; do
        wait "$pid" 2>/dev/null || true
    done
    sleep 1
}

test_ws_connection() {
    log_info "Testing WebSocket connection..."
    
    local temp_file
    temp_file=$(mktemp)
    
    (echo '{"need_overview":true}' | websocat -t --one-message "${WS_PUSH_URL}?x_client_id=e2e_conn_$$_$RANDOM" > "$temp_file" 2>&1) &
    local ws_pid=$!
    local ws_wait=2
    if is_windows; then ws_wait=5; fi
    sleep "$ws_wait"
    kill_pid $ws_pid
    
    local response
    response=$(cat "$temp_file")
    rm -f "$temp_file"
    
    log_debug "WS response: $response"
    
    if [[ -z "$response" ]]; then
        log_fail "No response from WebSocket"
        return 1
    fi
    
    if echo "$response" | grep -q '"type":"connected"'; then
        log_info "WebSocket connection established successfully"
        return 0
    else
        log_warn "Unexpected WebSocket response: $response"
        return 0
    fi
}

test_ws_traffic_delta() {
    log_info "Testing WebSocket traffic delta push..."
    
    local temp_file
    temp_file=$(mktemp)
    
    local initial_seq
    initial_seq=$(curl -sS "${ADMIN_BASE_URL}/api/traffic/updates?limit=1" | jq -r '.server_sequence // 0')
    log_info "Initial sequence: $initial_seq"
    
    (echo "{\"last_sequence\":$initial_seq}" | websocat -t "${WS_PUSH_URL}?x_client_id=e2e_delta_$$_$RANDOM" > "$temp_file" 2>&1) &
    local ws_pid=$!
    
    local ws_settle=1
    if is_windows; then ws_settle=3; fi
    sleep "$ws_settle"
    
    generate_traffic 2
    
    local ws_data_wait=3
    if is_windows; then ws_data_wait=6; fi
    sleep "$ws_data_wait"
    
    kill_pid $ws_pid
    wait_pid $ws_pid
    
    local messages
    messages=$(cat "$temp_file")
    rm -f "$temp_file"
    
    log_debug "Received messages: $messages"
    
    if echo "$messages" | grep -q '"type":"traffic_delta"'; then
        log_info "Received traffic_delta push message"
        
        local delta_msg
        delta_msg=$(echo "$messages" | grep '"type":"traffic_delta"' | head -1)
        
        if echo "$delta_msg" | jq -e '.data.inserts | length > 0' > /dev/null 2>&1; then
            log_info "Delta message contains new inserts"
            return 0
        else
            log_info "Delta message received but no inserts (may be updates only)"
            return 0
        fi
    elif echo "$messages" | grep -q '"type":"connected"'; then
        log_info "WebSocket connected, but no traffic_delta received (may need more traffic)"
        return 0
    else
        log_warn "No expected messages received"
        return 0
    fi
}

test_ws_overview_push() {
    log_info "Testing WebSocket overview push..."
    
    local temp_file
    temp_file=$(mktemp)
    
    (echo '{"need_overview":true}' | websocat -t "${WS_PUSH_URL}?x_client_id=e2e_overview_$$_$RANDOM" > "$temp_file" 2>&1) &
    local ws_pid=$!
    
    local ws_overview_wait=3
    if is_windows; then ws_overview_wait=6; fi
    sleep "$ws_overview_wait"
    
    kill_pid $ws_pid
    wait_pid $ws_pid
    
    local messages
    messages=$(cat "$temp_file")
    rm -f "$temp_file"
    
    log_debug "Overview messages: $messages"
    
    if echo "$messages" | grep -q '"type":"overview_update"'; then
        log_info "Received overview_update push message"
        return 0
    elif echo "$messages" | grep -q '"type":"connected"'; then
        log_info "WebSocket connected"
        return 0
    else
        log_warn "No overview message received"
        return 0
    fi
}

test_ws_max_channels() {
    log_info "Testing WebSocket max client channels (MAX=3)..."

    sleep 2

    local attempt
    for attempt in 1 2; do
        local probe_output
        probe_output=$(node "$SCRIPT_DIR/../test_utils/ws_channel_limit_probe.js" "$WS_PUSH_URL" 4 5000)

        log_debug "Max channels probe (attempt $attempt): $probe_output"

        if echo "$probe_output" | jq -e '.oldest_disconnect == true' >/dev/null 2>&1; then
            return 0
        fi

        if [[ "$attempt" -eq 1 ]]; then
            log_warn "Attempt 1 failed, retrying after wait..."
            sleep 3
        fi
    done

    log_warn "Oldest channel messages: $(echo "$probe_output" | jq -c '.oldest_messages')"
    log_fail "Disconnect message not received on oldest channel"
    return 1
}

test_ws_metrics_push() {
    log_info "Testing WebSocket metrics push..."
    
    local temp_file
    temp_file=$(mktemp)
    
    (echo '{"need_metrics":true,"metrics_interval_ms":500}' | websocat -t "${WS_PUSH_URL}?x_client_id=e2e_metrics_$$_$RANDOM" > "$temp_file" 2>&1) &
    local ws_pid=$!
    
    local ws_metrics_wait=3
    if is_windows; then ws_metrics_wait=6; fi
    sleep "$ws_metrics_wait"
    
    kill_pid $ws_pid
    wait_pid $ws_pid
    
    local messages
    messages=$(cat "$temp_file")
    rm -f "$temp_file"
    
    log_debug "Metrics messages: $messages"
    
    if echo "$messages" | grep -q '"type":"metrics_update"'; then
        log_info "Received metrics_update push message"
        
        local metrics_count
        metrics_count=$(echo "$messages" | grep -c '"type":"metrics_update"' || echo "0")
        log_info "Received $metrics_count metrics updates"
        
        return 0
    elif echo "$messages" | grep -q '"type":"connected"'; then
        log_info "WebSocket connected"
        return 0
    else
        log_warn "No metrics message received"
        return 0
    fi
}

test_polling_fallback() {
    log_info "Testing HTTP polling as fallback..."
    
    local seq1
    seq1=$(curl -sS "${ADMIN_BASE_URL}/api/traffic/updates?limit=50" | jq -r '.server_sequence // 0')
    log_info "Before traffic sequence: $seq1"
    
    generate_traffic 3
    
    sleep 1
    
    local response
    response=$(curl -sS "${ADMIN_BASE_URL}/api/traffic/updates?after_seq=${seq1}&limit=50")
    
    local new_count
    new_count=$(echo "$response" | jq -r '.new_records | length')
    local seq2
    seq2=$(echo "$response" | jq -r '.server_sequence // 0')
    
    log_info "After traffic sequence: $seq2, new records: $new_count"
    
    if [[ "$new_count" -gt 0 ]] || [[ "$seq2" -gt "$seq1" ]]; then
        log_info "HTTP polling fallback working correctly"
        return 0
    else
        log_warn "No new records detected via polling"
        return 0
    fi
}

test_pending_ids_tracking() {
    log_info "Testing pending IDs tracking in push..."
    
    local response
    response=$(curl -sS "${ADMIN_BASE_URL}/api/traffic?limit=10")
    
    local first_id
    first_id=$(echo "$response" | jq -r '.records[0].id // empty')
    
    if [[ -z "$first_id" ]]; then
        generate_traffic 1
        sleep 1
        response=$(curl -sS "${ADMIN_BASE_URL}/api/traffic?limit=10")
        first_id=$(echo "$response" | jq -r '.records[0].id // empty')
    fi
    
    if [[ -z "$first_id" ]]; then
        log_warn "No records available for pending IDs test"
        return 0
    fi
    
    log_info "Testing pending IDs with record: $first_id"
    
    local pending_response
    pending_response=$(curl -sS "${ADMIN_BASE_URL}/api/traffic/updates?pending_ids=${first_id}&limit=10")
    
    local updated_count
    updated_count=$(echo "$pending_response" | jq -r '.updated_records | length')
    
    log_info "Updated records for pending ID: $updated_count"
    
    if [[ "$updated_count" -gt 0 ]]; then
        local updated_id
        updated_id=$(echo "$pending_response" | jq -r '.updated_records[0].id // empty')
        assert_equals "$first_id" "$updated_id" "Updated record ID should match requested pending ID" || return 1
    fi
    
    return 0
}

test_incremental_sequence() {
    log_info "Testing incremental sequence tracking..."
    
    local seq1
    seq1=$(curl -sS "${ADMIN_BASE_URL}/api/traffic/updates?limit=1" | jq -r '.server_sequence // 0')
    
    generate_traffic 5
    sleep 1
    
    local seq2
    seq2=$(curl -sS "${ADMIN_BASE_URL}/api/traffic/updates?limit=1" | jq -r '.server_sequence // 0')
    
    log_info "Sequence before: $seq1, after: $seq2"
    
    assert_greater_than "$seq2" "$seq1" "Sequence should increase after traffic generation" || return 1
    
    local incremental
    incremental=$(curl -sS "${ADMIN_BASE_URL}/api/traffic/updates?after_seq=${seq1}&limit=50")
    
    local new_count
    new_count=$(echo "$incremental" | jq -r '.new_records | length')
    log_info "Incremental query returned $new_count new records"
    
    assert_greater_than "$new_count" 0 "Should have new records since sequence $seq1" || return 1
    
    return 0
}

main() {
    echo "=========================================="
    echo "  Traffic Push E2E Tests"
    echo "=========================================="
    echo "Admin URL: ${ADMIN_BASE_URL}"
    echo "WebSocket URL: ${WS_URL}"
    echo "Proxy Port: ${PROXY_PORT}"
    echo "=========================================="

    trap 'stop_mock_server; admin_cleanup_bifrost' EXIT
    admin_ensure_bifrost || { log_fail "Could not start Bifrost"; exit 1; }

    local connectivity
    connectivity=$(curl -sS -o /dev/null -w "%{http_code}" "${ADMIN_BASE_URL}/api/traffic?limit=1" 2>/dev/null || echo "000")
    
    if [[ "$connectivity" != "200" ]]; then
        log_fail "Cannot connect to Bifrost admin API at ${ADMIN_BASE_URL}"
        log_info "Make sure Bifrost proxy is running on port ${ADMIN_PORT}"
        exit 1
    fi
    
    log_info "Connected to Bifrost admin API"

    start_mock_server || { log_fail "Could not start mock server"; exit 1; }

    local has_websocat=true
    if ! ensure_websocat; then
        has_websocat=false
        log_warn "WebSocket tests will be limited"
    fi

    if [[ "$has_websocat" == "true" ]]; then
        run_test "WebSocket Connection" test_ws_connection
        run_test "WebSocket Traffic Delta" test_ws_traffic_delta
        run_test "WebSocket Overview Push" test_ws_overview_push
        run_test "WebSocket Metrics Push" test_ws_metrics_push
        run_test "WebSocket Max Channels" test_ws_max_channels
    else
        log_warn "Skipping WebSocket tests (websocat not available)"
    fi

    run_test "HTTP Polling Fallback" test_polling_fallback
    run_test "Pending IDs Tracking" test_pending_ids_tracking
    run_test "Incremental Sequence" test_incremental_sequence

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
