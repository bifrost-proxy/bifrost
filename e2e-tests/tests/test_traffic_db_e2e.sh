#!/bin/bash

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test_utils/admin_client.sh"
source "$SCRIPT_DIR/../test_utils/http_client.sh"
source "$SCRIPT_DIR/../test_utils/process.sh"

ADMIN_HOST="${ADMIN_HOST:-127.0.0.1}"
ADMIN_PORT="${ADMIN_PORT:-9900}"
PROXY_PORT="${PROXY_PORT:-9900}"
ADMIN_PATH_PREFIX="${ADMIN_PATH_PREFIX:-/_bifrost}"
ADMIN_BASE_URL="http://${ADMIN_HOST}:${ADMIN_PORT}${ADMIN_PATH_PREFIX}"
MOCK_HTTP_PORT="${MOCK_HTTP_PORT:-3197}"
MOCK_PID=""

export ADMIN_HOST ADMIN_PORT ADMIN_PATH_PREFIX

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
        log_fail "$msg: '$needle' not found in response"
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
    local count="${1:-5}"
    local pids=()
    
    log_info "Generating $count traffic records via proxy ${PROXY_PORT}..."
    
    for i in $(seq 1 "$count"); do
        curl -sS --proxy "http://127.0.0.1:${PROXY_PORT}" \
            --connect-timeout 5 --max-time 10 \
            "http://127.0.0.1:${MOCK_HTTP_PORT}/get?test_id=traffic_db_test_$$_${i}" \
            -o /dev/null -w "" 2>/dev/null &
        pids+=($!)
    done
    for pid in "${pids[@]}"; do
        wait "$pid" 2>/dev/null || true
    done
    sleep 2
}

test_traffic_query_api() {
    log_info "Testing traffic query API..."
    
    generate_traffic 3
    
    local response
    response=$(curl -sS -X POST "${ADMIN_BASE_URL}/api/traffic/query" \
        -H "Content-Type: application/json" \
        -d '{"limit": 10}')
    
    log_debug "Query response: $response"
    
    assert_not_empty "$response" "Query response should not be empty" || return 1
    
    local has_records
    has_records=$(echo "$response" | jq -r '.records | length > 0')
    assert_equals "true" "$has_records" "Query should return records" || return 1
    
    local total
    total=$(echo "$response" | jq -r '.total // 0')
    assert_greater_than "$total" 0 "Total should be greater than 0" || return 1
    
    log_info "Query API test passed, total records: $total"
    return 0
}

test_traffic_updates_api() {
    log_info "Testing traffic updates API..."
    
    local initial_response
    initial_response=$(curl -sS "${ADMIN_BASE_URL}/api/traffic/updates?limit=50")
    
    assert_not_empty "$initial_response" "Initial updates response should not be empty" || return 1
    
    local initial_seq
    initial_seq=$(echo "$initial_response" | jq -r '.server_sequence // 0')
    log_info "Initial server sequence: $initial_seq"
    
    generate_traffic 3
    sleep 1
    
    local new_response
    new_response=$(curl -sS "${ADMIN_BASE_URL}/api/traffic/updates?after_seq=${initial_seq}&limit=50")
    
    local new_records_count
    new_records_count=$(echo "$new_response" | jq -r '.new_records | length')
    log_info "New records count: $new_records_count"
    
    assert_greater_than "$new_records_count" 0 "Should have new records after generating traffic" || return 1
    
    local new_seq
    new_seq=$(echo "$new_response" | jq -r '.server_sequence // 0')
    log_info "New server sequence: $new_seq"
    
    if [[ "$new_seq" -gt "$initial_seq" ]]; then
        log_info "Server sequence increased correctly"
    else
        log_warn "Server sequence did not increase as expected (initial: $initial_seq, new: $new_seq)"
    fi
    
    return 0
}

test_traffic_pending_updates() {
    log_info "Testing pending records update tracking..."
    
    local response
    response=$(curl -sS "${ADMIN_BASE_URL}/api/traffic/updates?limit=100")
    
    local records_json
    records_json=$(echo "$response" | jq -r '.new_records')
    
    local first_record_id
    first_record_id=$(echo "$records_json" | jq -r '.[0].id // empty')
    
    if [[ -z "$first_record_id" ]]; then
        generate_traffic 1
        sleep 1
        response=$(curl -sS "${ADMIN_BASE_URL}/api/traffic/updates?limit=100")
        first_record_id=$(echo "$response" | jq -r '.new_records[0].id // empty')
    fi
    
    if [[ -z "$first_record_id" ]]; then
        log_warn "No records available for pending test, skipping..."
        return 0
    fi
    
    log_info "Using record ID for pending test: $first_record_id"
    
    local pending_response
    pending_response=$(curl -sS "${ADMIN_BASE_URL}/api/traffic/updates?pending_ids=${first_record_id}&limit=10")
    
    assert_not_empty "$pending_response" "Pending updates response should not be empty" || return 1
    
    local updated_records_count
    updated_records_count=$(echo "$pending_response" | jq -r '.updated_records | length')
    log_info "Updated records count: $updated_records_count"
    
    return 0
}

test_traffic_detail_api() {
    log_info "Testing traffic detail API..."
    
    generate_traffic 1
    
    local list_response
    list_response=$(curl -sS "${ADMIN_BASE_URL}/api/traffic?limit=5")
    
    local first_id
    first_id=$(echo "$list_response" | jq -r '.records[0].id // empty')
    
    if [[ -z "$first_id" ]]; then
        log_warn "No records available for detail test, skipping..."
        return 0
    fi
    
    log_info "Fetching detail for record: $first_id"
    
    local detail_response
    detail_response=$(curl -sS "${ADMIN_BASE_URL}/api/traffic/${first_id}")
    
    assert_not_empty "$detail_response" "Detail response should not be empty" || return 1
    
    local detail_id
    detail_id=$(echo "$detail_response" | jq -r '.id // empty')
    assert_equals "$first_id" "$detail_id" "Detail ID should match requested ID" || return 1
    
    local has_method
    has_method=$(echo "$detail_response" | jq 'has("method")')
    assert_equals "true" "$has_method" "Detail should have method field" || return 1
    
    local has_host
    has_host=$(echo "$detail_response" | jq 'has("host")')
    assert_equals "true" "$has_host" "Detail should have host field" || return 1
    
    log_info "Traffic detail API test passed"
    return 0
}

test_compact_format() {
    log_info "Testing compact format conversion..."
    
    local response
    response=$(curl -sS "${ADMIN_BASE_URL}/api/traffic/updates?limit=10")
    
    local first_record
    first_record=$(echo "$response" | jq -r '.new_records[0] // empty')
    
    if [[ -z "$first_record" || "$first_record" == "null" ]]; then
        generate_traffic 1
        sleep 1
        response=$(curl -sS "${ADMIN_BASE_URL}/api/traffic/updates?limit=10")
        first_record=$(echo "$response" | jq '.new_records[0] // empty')
    fi
    
    if [[ -z "$first_record" || "$first_record" == "null" ]]; then
        log_warn "No records for compact format test, skipping..."
        return 0
    fi
    
    local has_seq
    has_seq=$(echo "$first_record" | jq 'has("seq")')
    local has_m
    has_m=$(echo "$first_record" | jq 'has("m")')
    local has_h
    has_h=$(echo "$first_record" | jq 'has("h")')
    local has_p
    has_p=$(echo "$first_record" | jq 'has("p")')
    local has_s
    has_s=$(echo "$first_record" | jq 'has("s")')
    
    assert_equals "true" "$has_seq" "Compact format should have 'seq' field" || return 1
    assert_equals "true" "$has_m" "Compact format should have 'm' (method) field" || return 1
    assert_equals "true" "$has_h" "Compact format should have 'h' (host) field" || return 1
    assert_equals "true" "$has_p" "Compact format should have 'p' (path) field" || return 1
    assert_equals "true" "$has_s" "Compact format should have 's' (status) field" || return 1
    
    log_info "Compact format conversion test passed"
    return 0
}

test_traffic_clear() {
    log_info "Testing traffic clear API..."
    
    admin_put "/api/config/performance" '{"max_body_memory_size": 1}'

    local payload_file
    payload_file=$(mktemp)
    python3 - <<'PY' > "$payload_file"
print("x" * 4096)
PY
    curl -sS --proxy "http://127.0.0.1:${PROXY_PORT}" \
        --connect-timeout 5 --max-time 10 \
        -X POST "http://127.0.0.1:${MOCK_HTTP_PORT}/post" \
        -H "Content-Type: text/plain" \
        --data-binary "@${payload_file}" \
        -o /dev/null 2>/dev/null
    rm -f "$payload_file"

    local before_config
    before_config=$(admin_get "/api/config/performance")
    local before_body_files
    before_body_files=$(echo "$before_config" | jq -r '.body_store_stats.file_count // 0')
    if [[ "$before_body_files" -le 0 ]]; then
        log_warn "Body cache file count is 0 before clear; skipping cache count assertion"
    fi

    local initial_response
    initial_response=$(curl -sS "${ADMIN_BASE_URL}/api/traffic?limit=5")
    
    local initial_count
    initial_count=$(echo "$initial_response" | jq -r '.total // 0')
    log_info "Initial record count: $initial_count"
    
    if [[ "$initial_count" -eq 0 ]]; then
        generate_traffic 3
        sleep 1
        initial_response=$(curl -sS "${ADMIN_BASE_URL}/api/traffic?limit=5")
        initial_count=$(echo "$initial_response" | jq -r '.total // 0')
        log_info "After generating traffic, count: $initial_count"
    fi
    
    log_info "Clearing traffic..."
    local clear_response
    clear_response=$(curl -sS -X DELETE "${ADMIN_BASE_URL}/api/traffic")
    log_debug "Clear response: $clear_response"
    
    sleep 1
    
    local after_clear
    after_clear=$(curl -sS "${ADMIN_BASE_URL}/api/traffic?limit=5")
    local after_count
    after_count=$(echo "$after_clear" | jq -r '.total // 0')
    log_info "After clear, record count: $after_count"
    
    if [[ "$after_count" -gt 1 ]]; then
        log_fail "After clear, total should be 0 or 1, got ${after_count}"
        return 1
    fi

    local after_config
    after_config=$(admin_get "/api/config/performance")
    local after_body_files
    after_body_files=$(echo "$after_config" | jq -r '.body_store_stats.file_count // 0')
    if [[ "$before_body_files" -gt 0 ]]; then
        assert_equals "0" "$after_body_files" "After clear, body cache files should be 0" || return 1
    fi
    
    log_info "Traffic clear API test passed"
    return 0
}

test_sequence_persistence() {
    log_info "Testing sequence persistence across queries..."
    
    local response1
    response1=$(curl -sS "${ADMIN_BASE_URL}/api/traffic/updates?limit=50")
    local seq1
    seq1=$(echo "$response1" | jq -r '.server_sequence // 0')
    log_info "Initial sequence: $seq1"
    
    generate_traffic 5
    sleep 1
    
    local response2
    response2=$(curl -sS "${ADMIN_BASE_URL}/api/traffic/updates?limit=50")
    local seq2
    seq2=$(echo "$response2" | jq -r '.server_sequence // 0')
    log_info "After traffic sequence: $seq2"
    
    assert_greater_than "$seq2" "$seq1" "Sequence should increase after new traffic" || return 1
    
    local incremental_response
    incremental_response=$(curl -sS "${ADMIN_BASE_URL}/api/traffic/updates?after_seq=${seq1}&limit=50")
    local new_count
    new_count=$(echo "$incremental_response" | jq -r '.new_records | length')
    
    log_info "Incremental query returned $new_count new records"
    assert_greater_than "$new_count" 0 "Should have new records in incremental query" || return 1
    
    return 0
}

test_pagination() {
    log_info "Testing pagination..."
    
    generate_traffic 10
    sleep 1
    
    local page1
    page1=$(curl -sS -X POST "${ADMIN_BASE_URL}/api/traffic/query" \
        -H "Content-Type: application/json" \
        -d '{"limit": 5, "direction": "backward"}')
    
    local page1_count
    page1_count=$(echo "$page1" | jq -r '.records | length')
    local page1_total
    page1_total=$(echo "$page1" | jq -r '.total // 0')
    
    log_info "Page 1: $page1_count records, total: $page1_total"
    
    if [[ "$page1_total" -gt 5 ]]; then
        local first_seq
        first_seq=$(echo "$page1" | jq -r '.records[0].seq // 0')
        log_info "Page 1 first seq: $first_seq"
        
        local page2
        page2=$(curl -sS -X POST "${ADMIN_BASE_URL}/api/traffic/query" \
            -H "Content-Type: application/json" \
            -d "{\"cursor\": $first_seq, \"limit\": 5, \"direction\": \"backward\"}")
        
        local page2_count
        page2_count=$(echo "$page2" | jq -r '.records | length')
        log_info "Page 2: $page2_count records"
        
        if [[ "$page2_count" -eq 0 ]]; then
            log_info "No more records in page 2 (may be expected if at beginning)"
            return 0
        fi
        
        local page2_last_seq
        page2_last_seq=$(echo "$page2" | jq -r '.records[-1].seq // 0')
        log_info "Page 2 last seq: $page2_last_seq"
        
        if [[ "$page2_last_seq" -ge "$first_seq" ]]; then
            log_fail "Page 2 last seq ($page2_last_seq) should be less than page 1 first seq ($first_seq)"
            return 1
        fi
    else
        log_info "Not enough records for pagination test, skipping page 2"
    fi
    
    return 0
}

test_filter_by_method() {
    log_info "Testing filter by method..."
    
    generate_traffic 5
    sleep 1
    
    local response
    response=$(curl -sS -X POST "${ADMIN_BASE_URL}/api/traffic/query" \
        -H "Content-Type: application/json" \
        -d '{"method": "GET", "limit": 50}')
    
    local records
    records=$(echo "$response" | jq -r '.records')
    local count
    count=$(echo "$records" | jq -r 'length')
    
    log_info "GET method filter returned $count records"
    
    if [[ "$count" -gt 0 ]]; then
        local non_get
        non_get=$(echo "$records" | jq -r '[.[] | select(.m != "GET")] | length')
        assert_equals "0" "$non_get" "All records should be GET method" || return 1
    fi
    
    return 0
}

test_body_retrieval() {
    log_info "Testing request/response body retrieval..."
    
    generate_traffic 1
    sleep 1
    
    local list_response
    list_response=$(curl -sS "${ADMIN_BASE_URL}/api/traffic?limit=5")
    
    local record_id
    record_id=$(echo "$list_response" | jq -r '.records[0].id // empty')
    
    if [[ -z "$record_id" ]]; then
        log_warn "No records for body retrieval test, skipping..."
        return 0
    fi
    
    log_info "Testing body retrieval for record: $record_id"
    
    local response_body
    response_body=$(curl -sS "${ADMIN_BASE_URL}/api/traffic/${record_id}/response-body" 2>/dev/null)
    
    if [[ -n "$response_body" && "$response_body" != "null" ]]; then
        log_info "Response body retrieved successfully (length: ${#response_body})"
    else
        log_info "No response body available (may be expected for some requests)"
    fi
    
    return 0
}

main() {
    echo "=========================================="
    echo "  Traffic DB E2E Tests"
    echo "=========================================="
    echo "Admin URL: ${ADMIN_BASE_URL}"
    echo "Proxy Port: ${PROXY_PORT}"
    echo "=========================================="

    trap 'stop_mock_server; admin_cleanup_bifrost; if is_windows; then kill_bifrost_on_port "$PROXY_PORT"; fi' EXIT

    admin_ensure_bifrost || { log_fail "Could not start Bifrost"; exit 1; }

    log_info "Connected to Bifrost admin API"
    start_mock_server || { log_fail "Could not start mock server"; exit 1; }

    run_test "Traffic Query API" test_traffic_query_api
    run_test "Traffic Updates API" test_traffic_updates_api
    run_test "Traffic Pending Updates" test_traffic_pending_updates
    run_test "Traffic Detail API" test_traffic_detail_api
    run_test "Compact Format" test_compact_format
    run_test "Sequence Persistence" test_sequence_persistence
    run_test "Pagination" test_pagination
    run_test "Filter by Method" test_filter_by_method
    run_test "Body Retrieval" test_body_retrieval
    run_test "Traffic Clear" test_traffic_clear

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
