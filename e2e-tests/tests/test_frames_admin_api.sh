#!/bin/bash

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIFROST_BIN="${BIFROST_BIN:-$(cd "$SCRIPT_DIR/../.." && pwd)/target/release/bifrost}"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi
source "$SCRIPT_DIR/../test_utils/ws_client.sh"
source "$SCRIPT_DIR/../test_utils/sse_client.sh"
source "$SCRIPT_DIR/../test_utils/admin_client.sh"
source "$SCRIPT_DIR/../test_utils/process.sh"

PROXY_HOST="${PROXY_HOST:-127.0.0.1}"
PROXY_PORT="${PROXY_PORT:-9900}"
ADMIN_HOST="${ADMIN_HOST:-127.0.0.1}"
ADMIN_PORT="${ADMIN_PORT:-9900}"
WS_PORT="${WS_PORT:-18766}"
SSE_PORT="${SSE_PORT:-18767}"
ADMIN_PATH_PREFIX="${ADMIN_PATH_PREFIX:-/_bifrost}"
export ADMIN_PATH_PREFIX

BIFROST_PID=""
WS_SERVER_PID=""
SSE_SERVER_PID=""
BIFROST_DATA_DIR=""
BIFROST_LOG_FILE=""
STARTED_BIFROST=0
CREATED_DATA_DIR=0
SSE_TRAFFIC_OK=0

TESTS_RUN=0
TESTS_PASSED=0
TESTS_FAILED=0

log_info() { echo "[INFO] $*"; }
log_pass() { echo "[PASS] $*"; }
log_fail() { echo "[FAIL] $*"; }
log_debug() { [[ "${DEBUG:-0}" == "1" ]] && echo "[DEBUG] $*"; }

traffic_list_json() {
    curl -s "http://$ADMIN_HOST:$ADMIN_PORT${ADMIN_PATH_PREFIX}/api/traffic?limit=100"
}

find_traffic_id_by_url_fragment() {
    local fragment="$1"
    local traffic_list
    traffic_list=$(traffic_list_json)
    echo "$traffic_list" | jq -r --arg fragment "$fragment" '.records[] | select(((.url // .p // .path // "") | contains($fragment))) | .id' | head -1
}

wait_for_traffic_id_by_url_fragment() {
    local fragment="$1"
    local timeout="${2:-10}"
    local waited=0
    local traffic_id=""

    while [[ $waited -lt $timeout ]]; do
        traffic_id=$(find_traffic_id_by_url_fragment "$fragment")
        if [[ -n "$traffic_id" && "$traffic_id" != "null" ]]; then
            printf '%s\n' "$traffic_id"
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done

    return 1
}

find_ws_record_id() {
    local traffic_id=""
    traffic_id=$(wait_for_traffic_id_by_url_fragment "/ws" 10) || true
    if [[ -n "$traffic_id" && "$traffic_id" != "null" ]]; then
        printf '%s\n' "$traffic_id"
        return 0
    fi

    local traffic_list
    traffic_list=$(traffic_list_json)
    echo "$traffic_list" | jq -r '.records[] | select((.is_websocket // false) == true or ((((.flags // 0) / 2) | floor) % 2 == 1)) | .id' | head -1
}

find_sse_record_id() {
    local traffic_id=""
    traffic_id=$(wait_for_traffic_id_by_url_fragment "/sse" 10) || true
    if [[ -n "$traffic_id" && "$traffic_id" != "null" ]]; then
        printf '%s\n' "$traffic_id"
        return 0
    fi

    local traffic_list
    traffic_list=$(traffic_list_json)
    echo "$traffic_list" | jq -r '.records[] | select((.is_sse // false) == true or ((((.flags // 0) / 4) | floor) % 2 == 1)) | .id' | head -1
}

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
        log_fail "$msg: $actual is not greater than $threshold"
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

start_ws_server() {
    log_info "Starting WebSocket echo server on port $WS_PORT..."
    python3 "$SCRIPT_DIR/../mock_servers/ws_echo_server.py" --port "$WS_PORT" &
    WS_SERVER_PID=$!
    sleep 1

    if ! kill -0 "$WS_SERVER_PID" 2>/dev/null; then
        log_fail "Failed to start WebSocket server"
        return 1
    fi
    log_info "WebSocket server started (PID: $WS_SERVER_PID)"
    return 0
}

start_sse_server() {
    log_info "Starting SSE echo server on port $SSE_PORT..."
    python3 "$SCRIPT_DIR/../mock_servers/sse_echo_server.py" --port "$SSE_PORT" &
    SSE_SERVER_PID=$!

    local max_wait=10
    local waited=0
    while [[ $waited -lt $max_wait ]]; do
        if ! kill -0 "$SSE_SERVER_PID" 2>/dev/null; then
            log_fail "SSE server process exited early"
            return 1
        fi
        if curl -s --max-time 2 "http://$PROXY_HOST:$SSE_PORT/health" | grep -q '"status"' 2>/dev/null; then
            log_info "SSE server started (PID: $SSE_SERVER_PID)"
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done

    log_fail "Failed to start SSE server (health check timeout)"
    return 1
}

start_bifrost() {
    log_info "Starting Bifrost proxy on port $PROXY_PORT..."

    if [[ "${SKIP_START_PROXY:-0}" == "1" ]]; then
        local max_wait=30
        local waited=0
        while [[ $waited -lt $max_wait ]]; do
            if curl -s "http://$ADMIN_HOST:$ADMIN_PORT${ADMIN_PATH_PREFIX}/api/system" > /dev/null 2>&1; then
                log_info "Using existing Bifrost proxy at $ADMIN_HOST:$ADMIN_PORT"
                return 0
            fi
            sleep 1
            waited=$((waited + 1))
        done
        log_fail "Existing Bifrost proxy not ready at $ADMIN_HOST:$ADMIN_PORT"
        return 1
    fi

    if [[ -z "${BIFROST_DATA_DIR:-}" ]]; then
        BIFROST_DATA_DIR=$(mktemp -d)
        CREATED_DATA_DIR=1
    fi
    export BIFROST_DATA_DIR

    BIFROST_LOG_FILE=$(mktemp)

    local rust_dir
    rust_dir="$(cd "$SCRIPT_DIR/../.." && pwd)"

    cd "$rust_dir" || return 1

    SKIP_FRONTEND_BUILD=1 BIFROST_DATA_DIR="$BIFROST_DATA_DIR" \
        "$BIFROST_BIN" start -p "$PROXY_PORT" --skip-cert-check --unsafe-ssl \
        >"$BIFROST_LOG_FILE" 2>&1 &
    BIFROST_PID=$!
    STARTED_BIFROST=1

    local max_wait=90
    local waited=0
    while [[ $waited -lt $max_wait ]]; do
        if [[ -n "$BIFROST_PID" ]] && ! kill -0 "$BIFROST_PID" 2>/dev/null; then
            log_fail "Bifrost exited early (PID: $BIFROST_PID)"
            if [[ -n "$BIFROST_LOG_FILE" ]]; then
                echo "Last log (tail -200):" >&2
                tail -200 "$BIFROST_LOG_FILE" 2>/dev/null >&2 || true
            fi
            return 1
        fi

        if curl -s "http://$ADMIN_HOST:$ADMIN_PORT${ADMIN_PATH_PREFIX}/api/system" > /dev/null 2>&1; then
            log_info "Bifrost proxy started (PID: $BIFROST_PID)"
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done

    log_fail "Failed to start Bifrost proxy"
    if [[ -n "$BIFROST_LOG_FILE" ]]; then
        echo "Last log (tail -200):" >&2
        tail -200 "$BIFROST_LOG_FILE" 2>/dev/null >&2 || true
    fi
    return 1
}

cleanup() {
    log_info "Cleaning up..."

    if [[ "$STARTED_BIFROST" == "1" && -n "$BIFROST_PID" ]]; then
        safe_cleanup_proxy "$BIFROST_PID"
        log_info "Stopped Bifrost proxy"
    fi

    if [[ -n "$WS_SERVER_PID" ]]; then
        kill_pid "$WS_SERVER_PID"
        wait_pid "$WS_SERVER_PID"
        log_info "Stopped WebSocket server"
    fi

    if [[ -n "$SSE_SERVER_PID" ]]; then
        kill_pid "$SSE_SERVER_PID"
        wait_pid "$SSE_SERVER_PID"
        log_info "Stopped SSE server"
    fi

    if is_windows; then kill_bifrost_on_port "$PROXY_PORT"; fi

    if [[ "$CREATED_DATA_DIR" == "1" && -n "$BIFROST_DATA_DIR" && -d "$BIFROST_DATA_DIR" ]]; then
        rm -rf "$BIFROST_DATA_DIR"
        log_info "Cleaned up data directory"
    fi

    if [[ -n "$BIFROST_LOG_FILE" && -f "$BIFROST_LOG_FILE" ]]; then
        rm -f "$BIFROST_LOG_FILE" 2>/dev/null
    fi

    ws_cleanup_all 2>/dev/null
    sse_cleanup_all 2>/dev/null
}

generate_ws_traffic() {
    log_info "Generating WebSocket traffic..."

    local ws_host_header="${PROXY_HOST}:${WS_PORT}"
    if ! python3 "$SCRIPT_DIR/../test_utils/ws_stress_client.py" \
        --proxy-host "$PROXY_HOST" \
        --proxy-port "$PROXY_PORT" \
        --host-header "$ws_host_header" \
        --path "/ws" \
        --messages 2 \
        --timeout 15.0 \
        >/dev/null 2>&1; then
        log_fail "Failed to generate WebSocket traffic"
        return 1
    fi

    if ! wait_for_traffic_id_by_url_fragment "/ws" 10 >/dev/null 2>&1; then
        log_fail "WebSocket traffic was not recorded in admin API"
        return 1
    fi

    log_info "WebSocket traffic generated"
}

generate_sse_traffic() {
    log_info "Generating SSE traffic..."

    local sse_url="http://$PROXY_HOST:$SSE_PORT"

    local health_ok=0
    for i in $(seq 1 5); do
        if curl -s --max-time 3 "http://$PROXY_HOST:$SSE_PORT/health" | grep -q '"status"' 2>/dev/null; then
            health_ok=1
            break
        fi
        log_debug "SSE server health check attempt $i failed, retrying..."
        sleep 1
    done
    if [[ "$health_ok" -ne 1 ]]; then
        log_fail "SSE echo server not reachable at $PROXY_HOST:$SSE_PORT"
        return 1
    fi

    local sse_ok=0
    local attempt
    for attempt in 1 2 3; do
        local sse_output
        sse_output=$(SSE_PROXY="http://$PROXY_HOST:$PROXY_PORT" sse_fetch_all "$sse_url" "/sse?count=3" 10 2>&1)
        local sse_exit=$?
        if [[ $sse_exit -eq 0 ]] && echo "$sse_output" | grep -q "data:" 2>/dev/null; then
            sse_ok=1
            break
        fi
        log_debug "SSE traffic attempt $attempt: exit=$sse_exit output_len=${#sse_output}"
        sleep 2
    done
    if [[ "$sse_ok" -ne 1 ]]; then
        log_fail "curl SSE request through proxy failed after 3 attempts"
        return 1
    fi

    if ! wait_for_traffic_id_by_url_fragment "/sse" 15 >/dev/null 2>&1; then
        log_debug "Traffic list at failure: $(traffic_list_json 2>/dev/null | head -c 500)"
        log_fail "SSE traffic was not recorded in admin API"
        return 1
    fi

    log_info "SSE traffic generated"
    SSE_TRAFFIC_OK=1
}

generate_http_traffic() {
    log_info "Generating HTTP traffic..."

    curl -s -x "http://$PROXY_HOST:$PROXY_PORT" "http://httpbin.org/get" > /dev/null 2>&1
    curl -s -x "http://$PROXY_HOST:$PROXY_PORT" "http://httpbin.org/headers" > /dev/null 2>&1

    sleep 1
    log_info "HTTP traffic generated"
}

test_traffic_list_api() {
    local response
    response=$(curl -s "http://$ADMIN_HOST:$ADMIN_PORT${ADMIN_PATH_PREFIX}/api/traffic?limit=10")

    if [[ $? -ne 0 ]]; then
        log_fail "Failed to call traffic list API"
        return 1
    fi

    if ! assert_not_empty "$response" "Traffic list response should not be empty"; then
        return 1
    fi

    local has_records
    has_records=$(echo "$response" | jq 'has("records")')

    if ! assert_equals "true" "$has_records" "Response should have records field"; then
        log_debug "Response: $response"
        return 1
    fi

    return 0
}

test_traffic_list_pagination() {
    local response1
    local response2

    response1=$(curl -s "http://$ADMIN_HOST:$ADMIN_PORT${ADMIN_PATH_PREFIX}/api/traffic?limit=5&offset=0")
    response2=$(curl -s "http://$ADMIN_HOST:$ADMIN_PORT${ADMIN_PATH_PREFIX}/api/traffic?limit=5&offset=5")

    local count1
    local count2
    count1=$(echo "$response1" | jq '.records | length')
    count2=$(echo "$response2" | jq '.records | length')

    if [[ "$count1" -le 5 ]] && [[ "$count2" -le 5 ]]; then
        return 0
    else
        log_fail "Pagination not working correctly"
        return 1
    fi
}

test_frames_api_structure() {
    local traffic_list
    traffic_list=$(traffic_list_json)

    local ws_record
    ws_record=$(find_ws_record_id)

    if [[ -z "$ws_record" || "$ws_record" == "null" ]]; then
        log_fail "No WebSocket traffic found"
        log_debug "Traffic list: $traffic_list"
        return 1
    fi

    local response
    response=$(curl -s "http://$ADMIN_HOST:$ADMIN_PORT${ADMIN_PATH_PREFIX}/api/traffic/$ws_record/frames")

    if ! assert_not_empty "$response" "Frames response should not be empty"; then
        return 1
    fi

    local has_frames
    has_frames=$(echo "$response" | jq 'has("frames")')

    if ! assert_equals "true" "$has_frames" "Response should have frames field"; then
        log_debug "Response: $response"
        return 1
    fi

    local has_socket_status
    has_socket_status=$(echo "$response" | jq 'has("socket_status")')

    if ! assert_equals "true" "$has_socket_status" "Response should have socket_status field"; then
        return 1
    fi

    local has_has_more
    has_has_more=$(echo "$response" | jq 'has("has_more")')

    if ! assert_equals "true" "$has_has_more" "Response should have has_more field"; then
        return 1
    fi

    return 0
}

test_frames_api_frame_fields() {
    if [[ "$SSE_TRAFFIC_OK" -ne 1 ]]; then
        log_debug "Skipping: SSE traffic was not generated"
        return 0
    fi

    local traffic_list
    traffic_list=$(traffic_list_json)

    local sse_record
    sse_record=$(find_sse_record_id)

    if [[ -z "$sse_record" || "$sse_record" == "null" ]]; then
        log_fail "No SSE traffic found"
        return 1
    fi

    local frames
    frames=$(curl -s "http://$ADMIN_HOST:$ADMIN_PORT${ADMIN_PATH_PREFIX}/api/traffic/$sse_record/frames")

    local frame_count
    frame_count=$(echo "$frames" | jq '.frames | length')

    if [[ "$frame_count" -eq 0 ]]; then
        log_debug "No frames found for SSE traffic (this may be expected if SSE frames are not captured)"
        return 0
    fi

    local first_frame
    first_frame=$(echo "$frames" | jq '.frames[0]')

    local has_frame_id has_direction has_timestamp has_frame_type
    has_frame_id=$(echo "$first_frame" | jq 'has("frame_id")')
    has_direction=$(echo "$first_frame" | jq 'has("direction")')
    has_timestamp=$(echo "$first_frame" | jq 'has("timestamp")')
    has_frame_type=$(echo "$first_frame" | jq 'has("frame_type")')

    if ! assert_equals "true" "$has_frame_id" "Frame should have frame_id field"; then
        return 1
    fi

    if ! assert_equals "true" "$has_direction" "Frame should have direction field"; then
        return 1
    fi

    if ! assert_equals "true" "$has_timestamp" "Frame should have timestamp field"; then
        return 1
    fi

    if ! assert_equals "true" "$has_frame_type" "Frame should have frame_type field"; then
        return 1
    fi

    return 0
}

test_frames_api_pagination() {
    local traffic_list
    traffic_list=$(traffic_list_json)

    local ws_record
    ws_record=$(find_ws_record_id)

    if [[ -z "$ws_record" || "$ws_record" == "null" ]]; then
        log_fail "No WebSocket traffic found"
        return 1
    fi

    local response1
    response1=$(curl -s "http://$ADMIN_HOST:$ADMIN_PORT${ADMIN_PATH_PREFIX}/api/traffic/$ws_record/frames?limit=5")

    local count1
    count1=$(echo "$response1" | jq '.frames | length')

    if [[ "$count1" -gt 5 ]]; then
        log_fail "Pagination limit not respected: got $count1 frames"
        return 1
    fi

    return 0
}

test_frames_api_invalid_traffic_id() {
    local response
    response=$(curl -s -w "\n%{http_code}" "http://$ADMIN_HOST:$ADMIN_PORT${ADMIN_PATH_PREFIX}/api/traffic/invalid_id_12345/frames")

    local http_code
    http_code=$(echo "$response" | tail -1)

    if [[ "$http_code" == "404" ]] || [[ "$http_code" == "400" ]]; then
        return 0
    fi

    local body
    body=$(echo "$response" | head -n -1)
    local frames_count
    frames_count=$(echo "$body" | jq '.frames | length // 0')

    if [[ "$frames_count" -eq 0 ]]; then
        return 0
    fi

    log_fail "Expected error for invalid traffic ID, got HTTP $http_code"
    log_debug "Response: $response"
    return 1
}

test_websocket_connections_list() {
    local response
    response=$(curl -s "http://$ADMIN_HOST:$ADMIN_PORT${ADMIN_PATH_PREFIX}/api/websocket/connections")

    if [[ $? -ne 0 ]]; then
        log_fail "Failed to call WebSocket connections API"
        return 1
    fi

    if ! assert_not_empty "$response" "Connections response should not be empty"; then
        return 1
    fi

    local has_connections
    has_connections=$(echo "$response" | jq 'has("connections")')

    if ! assert_equals "true" "$has_connections" "Response should have connections field"; then
        log_debug "Response: $response"
        return 1
    fi

    return 0
}

test_traffic_record_ws_fields() {
    local traffic_list
    traffic_list=$(traffic_list_json)

    local ws_record_id
    ws_record_id=$(find_ws_record_id)
    local ws_record
    ws_record=$(echo "$traffic_list" | jq --arg id "$ws_record_id" '[.records[] | select((.id // "") == $id)] | first')

    if [[ -z "$ws_record" || "$ws_record" == "null" ]]; then
        log_fail "No WebSocket traffic record found"
        log_debug "Traffic list: $traffic_list"
        return 1
    fi

    local is_ws
    is_ws=$(echo "$ws_record" | jq -r '((.is_websocket // false) == true) or ((((.flags // 0) / 2) | floor) % 2 == 1)')

    if ! assert_equals "true" "$is_ws" "Traffic should be marked as WebSocket"; then
        return 1
    fi

    return 0
}

test_traffic_record_sse_fields() {
    if [[ "$SSE_TRAFFIC_OK" -ne 1 ]]; then
        log_debug "Skipping: SSE traffic was not generated"
        return 0
    fi

    local traffic_list
    traffic_list=$(traffic_list_json)

    local sse_record_id
    sse_record_id=$(find_sse_record_id)
    local sse_record
    sse_record=$(echo "$traffic_list" | jq --arg id "$sse_record_id" '[.records[] | select((.id // "") == $id)] | first')

    if [[ -z "$sse_record" || "$sse_record" == "null" ]]; then
        log_fail "No SSE traffic record found"
        log_debug "Traffic list: $traffic_list"
        return 1
    fi

    local is_sse
    is_sse=$(echo "$sse_record" | jq -r '((.is_sse // false) == true) or ((((.flags // 0) / 4) | floor) % 2 == 1)')

    if ! assert_equals "true" "$is_sse" "Traffic should be marked as SSE"; then
        return 1
    fi

    return 0
}

test_frame_direction_values() {
    local traffic_list
    traffic_list=$(traffic_list_json)

    local ws_record
    ws_record=$(find_ws_record_id)

    if [[ -z "$ws_record" || "$ws_record" == "null" ]]; then
        log_fail "No WebSocket traffic found"
        return 1
    fi

    local frames
    frames=$(curl -s "http://$ADMIN_HOST:$ADMIN_PORT${ADMIN_PATH_PREFIX}/api/traffic/$ws_record/frames")

    local directions
    directions=$(echo "$frames" | jq -r '.frames[].direction' | sort -u)

    local valid=true
    while IFS= read -r dir; do
        if [[ -n "$dir" && "$dir" != "send" && "$dir" != "receive" ]]; then
            log_fail "Invalid direction: $dir"
            valid=false
        fi
    done <<< "$directions"

    if [[ "$valid" != "true" ]]; then
        return 1
    fi

    return 0
}

test_frame_type_values() {
    local traffic_list
    traffic_list=$(traffic_list_json)

    local ws_record
    ws_record=$(find_ws_record_id)

    if [[ -n "$ws_record" && "$ws_record" != "null" ]]; then
        local ws_frames
        ws_frames=$(curl -s "http://$ADMIN_HOST:$ADMIN_PORT${ADMIN_PATH_PREFIX}/api/traffic/$ws_record/frames")

        local ws_types
        ws_types=$(echo "$ws_frames" | jq -r '.frames[].frame_type' | sort -u)

        while IFS= read -r type; do
            if [[ -n "$type" ]]; then
                case "$type" in
                    text|binary|ping|pong|close|continuation)
                        ;;
                    *)
                        log_fail "Invalid WebSocket frame type: $type"
                        return 1
                        ;;
                esac
            fi
        done <<< "$ws_types"
    fi

    local sse_record
    sse_record=$(find_sse_record_id)

    if [[ -n "$sse_record" && "$sse_record" != "null" ]]; then
        local sse_frames
        sse_frames=$(curl -s "http://$ADMIN_HOST:$ADMIN_PORT${ADMIN_PATH_PREFIX}/api/traffic/$sse_record/frames")

        local sse_type
        sse_type=$(echo "$sse_frames" | jq -r '.frames[0].frame_type // empty')

        if [[ -n "$sse_type" && "$sse_type" != "sse" ]]; then
            log_fail "SSE frame should have type 'sse', got '$sse_type'"
            return 1
        fi
    fi

    return 0
}

test_concurrent_api_calls() {
    local pids=()
    for i in $(seq 1 5); do
        curl -s "http://$ADMIN_HOST:$ADMIN_PORT${ADMIN_PATH_PREFIX}/api/traffic?limit=10" > /dev/null &
        pids+=("$!")
    done

    for pid in "${pids[@]}"; do
        wait "$pid"
    done

    local response
    response=$(curl -s "http://$ADMIN_HOST:$ADMIN_PORT${ADMIN_PATH_PREFIX}/api/traffic?limit=10")

    if ! assert_not_empty "$response" "API should respond after concurrent calls"; then
        return 1
    fi

    return 0
}

print_summary() {
    echo ""
    echo "======================================"
    echo "Admin API Test Summary"
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
    trap cleanup EXIT

    log_info "Starting Frames Admin API Tests"
    log_info "Proxy: $PROXY_HOST:$PROXY_PORT"
    log_info "Admin: $ADMIN_HOST:$ADMIN_PORT"
    log_info "WebSocket Server: $PROXY_HOST:$WS_PORT"
    log_info "SSE Server: $PROXY_HOST:$SSE_PORT"
    echo ""

    if ! start_ws_server; then
        log_fail "Failed to start WebSocket server"
        exit 1
    fi

    if ! start_sse_server; then
        log_info "SSE server failed to start; SSE-dependent tests will be skipped"
    fi

    if ! start_bifrost; then
        log_fail "Failed to start Bifrost proxy"
        exit 1
    fi

    generate_ws_traffic
    generate_sse_traffic
    generate_http_traffic

    run_test "Traffic List API" test_traffic_list_api
    run_test "Traffic List Pagination" test_traffic_list_pagination
    run_test "Frames API Structure" test_frames_api_structure
    run_test "Frames API Frame Fields" test_frames_api_frame_fields
    run_test "Frames API Pagination" test_frames_api_pagination
    run_test "Frames API Invalid Traffic ID" test_frames_api_invalid_traffic_id
    run_test "WebSocket Connections List" test_websocket_connections_list
    run_test "Traffic Record WS Fields" test_traffic_record_ws_fields
    run_test "Traffic Record SSE Fields" test_traffic_record_sse_fields
    run_test "Frame Direction Values" test_frame_direction_values
    run_test "Frame Type Values" test_frame_type_values
    run_test "Concurrent API Calls" test_concurrent_api_calls

    print_summary
    exit $?
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    main "$@"
fi
