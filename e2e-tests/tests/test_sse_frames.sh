#!/bin/bash

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIFROST_BIN="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)/target/release/bifrost"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi

PROXY_HOST="${PROXY_HOST:-127.0.0.1}"
pick_free_port() {
    python3 - << 'PY'
import socket
for port in range(18990, 19050):
    s = socket.socket()
    try:
        s.bind(("127.0.0.1", port))
        print(port)
        break
    except OSError:
        pass
    finally:
        s.close()
else:
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    print(s.getsockname()[1])
    s.close()
PY
}
if [[ -z "${PROXY_PORT:-}" ]]; then
    PROXY_PORT="$(pick_free_port)"
fi
ADMIN_HOST="$PROXY_HOST"
ADMIN_PORT="$PROXY_PORT"
SSE_HOST="${SSE_HOST:-127.0.0.1}"
SSE_PORT="${SSE_PORT:-8767}"
ADMIN_PATH_PREFIX="${ADMIN_PATH_PREFIX:-/_bifrost}"
export ADMIN_PATH_PREFIX

SSE_PROXY="http://${PROXY_HOST}:${PROXY_PORT}"
SSE_TARGET="http://${SSE_HOST}:${SSE_PORT}"
export SSE_PROXY

source "$SCRIPT_DIR/../test_utils/sse_client.sh"
source "$SCRIPT_DIR/../test_utils/admin_client.sh"
source "$SCRIPT_DIR/../test_utils/process.sh"

TESTS_RUN=0
TESTS_PASSED=0
TESTS_FAILED=0

log_info() { echo "[INFO] $*"; }
log_pass() { echo "[PASS] $*"; }
log_fail() { echo "[FAIL] $*"; }
log_debug() { [[ "${DEBUG:-0}" == "1" ]] && echo "[DEBUG] $*"; }

BIFROST_DATA_DIR=""
BIFROST_PID=""
SSE_SERVER_PID=""

cleanup() {
    log_info "Cleaning up..."

    if is_windows; then kill_bifrost_on_port "$PROXY_PORT"; fi

    safe_cleanup_proxy "$BIFROST_PID"

    if [[ -n "$SSE_SERVER_PID" ]] && kill -0 "$SSE_SERVER_PID" 2>/dev/null; then
        kill_pid "$SSE_SERVER_PID"
        wait_pid "$SSE_SERVER_PID"
    fi

    if [[ -n "$BIFROST_DATA_DIR" && -d "$BIFROST_DATA_DIR" ]]; then
        rm -rf "$BIFROST_DATA_DIR"
    fi

    sse_cleanup_all 2>/dev/null || true
}

trap cleanup EXIT

start_sse_server() {
    log_info "Starting SSE echo server on port $SSE_PORT..."
    python3 "$SCRIPT_DIR/../mock_servers/sse_echo_server.py" --port "$SSE_PORT" > /dev/null 2>&1 &
    SSE_SERVER_PID=$!

    if ! kill -0 "$SSE_SERVER_PID" 2>/dev/null; then
        log_fail "Failed to start SSE server"
        return 1
    fi

    local max_wait=50
    if is_windows; then max_wait=100; fi
    local waited=0
    while [[ $waited -lt $max_wait ]]; do
        if curl -s "${SSE_TARGET}/health" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.1
        waited=$((waited + 1))
    done

    log_fail "SSE server health check failed"
    return 1
}

start_bifrost() {
    log_info "Starting Bifrost proxy on port $PROXY_PORT..."
    BIFROST_DATA_DIR="$(mktemp -d)"
    export BIFROST_DATA_DIR

    BIFROST_DATA_DIR="$BIFROST_DATA_DIR" "$BIFROST_BIN" start -p "$PROXY_PORT" --skip-cert-check --unsafe-ssl > /dev/null 2>&1 &
    BIFROST_PID=$!

    local max_wait=60
    local waited=0
    while [[ $waited -lt $max_wait ]]; do
        if ! kill -0 "$BIFROST_PID" 2>/dev/null; then
            log_fail "Bifrost proxy exited unexpectedly"
            return 1
        fi
        if curl -sf "http://${ADMIN_HOST}:${ADMIN_PORT}${ADMIN_PATH_PREFIX}/api/system" >/dev/null 2>&1; then
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done

    log_fail "Failed to start Bifrost proxy"
    return 1
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

find_latest_sse_traffic_id() {
    local url_pattern="$1"
    local limit="${2:-50}"
    get_traffic_list "$limit" | jq -r "[.records[] | select((.is_sse // false) == true) | select((.url // .p // .path // \"\") | contains(\"$url_pattern\"))] | sort_by(.sequence) | last | .id"
}

wait_for_sse_traffic_id() {
    local url_pattern="$1"
    local timeout="${2:-10}"
    local waited=0
    while [[ $waited -lt $((timeout * 10)) ]]; do
        local traffic_id
        traffic_id=$(find_latest_sse_traffic_id "$url_pattern")
        if [[ -n "$traffic_id" && "$traffic_id" != "null" ]]; then
            echo "$traffic_id"
            return 0
        fi
        sleep 0.1
        waited=$((waited + 1))
    done
    return 1
}

wait_for_traffic_id_by_url() {
    local url_pattern="$1"
    local timeout="${2:-10}"
    local waited=0
    while [[ $waited -lt $((timeout * 10)) ]]; do
        local traffic_id
        traffic_id=$(find_traffic_id_by_url "$url_pattern" 50)
        if [[ -n "$traffic_id" && "$traffic_id" != "null" ]]; then
            echo "$traffic_id"
            return 0
        fi
        sleep 0.1
        waited=$((waited + 1))
    done
    return 1
}

assert_contains() {
    local haystack="$1"
    local needle="$2"
    local msg="${3:-Should contain substring}"

    if [[ "$haystack" == *"$needle"* ]]; then
        return 0
    else
        log_fail "$msg: '$needle' not found in '$haystack'"
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

test_sse_basic_events() {
    local events
    local event_count

    events=$(sse_fetch_all "$SSE_TARGET" "/sse?count=3" 5)
    if [[ $? -ne 0 ]]; then
        log_fail "Failed to fetch SSE events"
        return 1
    fi

    event_count=$(echo "$events" | grep -c "^data:" 2>/dev/null | tr -d '[:space:]' || echo "0")

    if ! assert_greater_than "$event_count" 0 "Should receive at least one SSE event"; then
        log_debug "Events received: $events"
        return 1
    fi

    if ! assert_contains "$events" "data:" "Events should contain data fields"; then
        return 1
    fi

    return 0
}

test_sse_custom_events() {
    local events

    events=$(sse_fetch_all "$SSE_TARGET" "/sse/custom?count=2&interval=0.1" 5)
    if [[ $? -ne 0 ]]; then
        log_fail "Failed to fetch custom SSE events"
        return 1
    fi

    local event_count
    event_count=$(echo "$events" | grep -c "^event:" 2>/dev/null | tr -d '[:space:]' || echo "0")

    if ! assert_greater_than "$event_count" 0 "Should receive custom events with event type"; then
        log_debug "Events received: $events"
        return 1
    fi

    return 0
}

test_sse_multiline_data() {
    local events

    events=$(sse_fetch_all "$SSE_TARGET" "/sse/multiline" 5)
    if [[ $? -ne 0 ]]; then
        log_fail "Failed to fetch multiline SSE events"
        return 1
    fi

    local data_lines
    data_lines=$(echo "$events" | grep -c "^data:" 2>/dev/null | tr -d '[:space:]' || echo "0")

    if ! assert_greater_than "$data_lines" 1 "Multiline events should have multiple data lines"; then
        log_debug "Events received: $events"
        return 1
    fi

    return 0
}

test_sse_json_events() {
    local events

    events=$(sse_fetch_all "$SSE_TARGET" "/sse/json" 5)
    if [[ $? -ne 0 ]]; then
        log_fail "Failed to fetch JSON SSE events"
        return 1
    fi

    if ! assert_contains "$events" "{" "JSON events should contain JSON data"; then
        log_debug "Events received: $events"
        return 1
    fi

    if ! assert_contains "$events" "}" "JSON events should contain complete JSON"; then
        return 1
    fi

    return 0
}

test_sse_frames_capture() {
    sse_fetch_all "$SSE_TARGET" "/sse?count=3" 5 > /dev/null 2>&1

    sleep 1

    local traffic_list
    traffic_list=$(get_traffic_list "$ADMIN_HOST" "$ADMIN_PORT" 20)
    if [[ $? -ne 0 ]]; then
        log_fail "Failed to get traffic list"
        return 1
    fi

    local traffic_id
    traffic_id=$(echo "$traffic_list" | jq -r '.records[] | select((.url // .p // "") | contains("/sse")) | .id' | head -1)

    if [[ -z "$traffic_id" || "$traffic_id" == "null" ]]; then
        log_fail "No SSE traffic found in traffic list"
        log_debug "Traffic list: $traffic_list"
        return 1
    fi

    local record
    record=$(get_traffic_detail "$traffic_id")
    if [[ $? -ne 0 || -z "$record" ]]; then
        log_fail "Failed to get traffic detail for $traffic_id"
        return 1
    fi

    local is_sse
    is_sse=$(echo "$record" | jq -r '.is_sse // false')

    if ! assert_equals "true" "$is_sse" "Traffic should be marked as SSE"; then
        log_debug "Record: $record"
        return 1
    fi

    return 0
}

test_sse_stream_closed_behavior() {
    sse_fetch_all "$SSE_TARGET" "/sse/custom?count=5&interval=0.05" 5 > /dev/null 2>&1

    sleep 1

    local traffic_id
    traffic_id=$(wait_for_traffic_id_by_url "/sse/custom" 10)

    if [[ -z "$traffic_id" ]]; then
        log_fail "No SSE traffic found"
        return 1
    fi

    local resp
    resp=$(curl -s -w "\n%{http_code}" \
        "http://${ADMIN_HOST}:${ADMIN_PORT}/_bifrost/api/traffic/${traffic_id}/sse/stream?from=begin")
    local code
    code=$(echo "$resp" | tail -n 1)
    if ! assert_equals "409" "$code" "Closed SSE should reject /sse/stream"; then
        log_debug "Response: $(echo "$resp" | head -n 5)"
        return 1
    fi

    local body
    body=$(get_response_body "$traffic_id" | jq -r -j '.data // ""' | head -c 50)
    if [[ -z "$body" ]]; then
        log_fail "Closed SSE should have response-body"
        return 1
    fi

    return 0
}

test_sse_stream_open_full_and_live() {
    clear_traffic >/dev/null 2>&1 || true
    sleep 0.5

    sse_fetch_all "$SSE_TARGET" "/sse/custom?count=80&interval=0.05" 10 > /dev/null 2>&1 &
    local sse_pid=$!

    local traffic_id
    traffic_id=$(wait_for_traffic_id_by_url "/sse/custom" 10)
    if [[ -z "$traffic_id" ]]; then
        log_fail "No SSE traffic found"
        kill_pid "$sse_pid"
        return 1
    fi

    local stream_output
    stream_output=$(curl -s --no-buffer --max-time 5 \
        "http://${ADMIN_HOST}:${ADMIN_PORT}/_bifrost/api/traffic/${traffic_id}/sse/stream?from=begin" | \
        awk '/^data: /{print substr($0,7); c++; if(c>=3) exit 0}')

    if [[ -z "$stream_output" ]]; then
        log_fail "Open SSE stream should output data events"
        kill_pid "$sse_pid"
        return 1
    fi

    local seq0
    seq0=$(echo "$stream_output" | head -n 1 | jq -r '.seq // 0')
    if [[ "${seq0:-0}" -le 0 ]]; then
        log_fail "SSE stream payload should contain seq"
        kill_pid "$sse_pid"
        return 1
    fi

    local data0
    data0=$(echo "$stream_output" | head -n 1 | jq -r '.data // ""')
    if [[ -z "$data0" ]]; then
        log_fail "SSE stream payload should contain non-empty data"
        kill_pid "$sse_pid"
        return 1
    fi

    wait_pid "$sse_pid"
    return 0
}

test_sse_frame_content() {
    clear_traffic >/dev/null 2>&1 || true
    sleep 0.5

    sse_fetch_all "$SSE_TARGET" "/sse/custom?count=20&interval=0.05" 5 > /dev/null 2>&1
    sleep 1

    local traffic_id
    traffic_id=$(wait_for_traffic_id_by_url "/sse/custom" 10)
    if [[ -z "$traffic_id" || "$traffic_id" == "null" ]]; then
        log_fail "No SSE traffic found"
        return 1
    fi

    local record
    record=$(get_traffic_detail "$traffic_id")
    local is_sse
    is_sse=$(echo "$record" | jq -r '.is_sse // false')
    if ! assert_equals "true" "$is_sse" "Traffic should be marked as SSE"; then
        log_debug "Record: $record"
        return 1
    fi
    local response_body_ref
    response_body_ref=$(echo "$record" | jq -r '.response_body_ref // empty')
    if [[ -z "$response_body_ref" ]]; then
        log_debug "Record: $record"
        log_fail "SSE response_body_ref should be persisted"
        return 1
    fi
    local response_size
    response_size=$(echo "$record" | jq -r '.response_size // 0')
    if [[ "${response_size:-0}" -le 0 ]]; then
        log_debug "Record: $record"
        log_fail "SSE response_size should be persisted"
        return 1
    fi
    local socket_bytes=0
    local waited=0
    while [[ $waited -lt 50 ]]; do
        record=$(get_traffic_detail "$traffic_id")
        socket_bytes=$(echo "$record" | jq -r '(.socket_status.send_bytes // 0) + (.socket_status.receive_bytes // 0)')
        if [[ "${socket_bytes:-0}" -gt 0 ]]; then
            break
        fi
        sleep 0.1
        waited=$((waited + 1))
    done
    if [[ "${socket_bytes:-0}" -le 0 ]]; then
        log_fail "SSE socket_status bytes should be persisted"
        return 1
    fi

    return 0
}

test_sse_response_body_integrity() {
    clear_traffic >/dev/null 2>&1 || true
    sleep 0.5

    local raw_file
    raw_file="$(mktemp)"
    local stored_file
    stored_file="$(mktemp)"

    sse_fetch_all "$SSE_TARGET" "/sse/custom?count=6&interval=0.02" 8 > "$raw_file" 2>/dev/null
    sleep 1

    local traffic_id
    traffic_id=$(wait_for_traffic_id_by_url "/sse/custom" 10)
    if [[ -z "$traffic_id" || "$traffic_id" == "null" ]]; then
        log_fail "No SSE traffic found"
        rm -f "$raw_file" "$stored_file"
        return 1
    fi

    local waited=0
    while [[ $waited -lt 50 ]]; do
        get_response_body "$traffic_id" | jq -r -j '.data // ""' > "$stored_file"
        if [[ -s "$stored_file" ]]; then
            break
        fi
        sleep 0.1
        waited=$((waited + 1))
    done
    if ! cmp -s "$raw_file" "$stored_file"; then
        log_fail "SSE response body should match captured raw stream"
        log_debug "Traffic ID: $traffic_id"
        rm -f "$raw_file" "$stored_file"
        return 1
    fi

    rm -f "$raw_file" "$stored_file"
    return 0
}

test_sse_frames_api_disabled() {
    sse_fetch_all "$SSE_TARGET" "/sse?count=2" 5 > /dev/null 2>&1

    sleep 1

    local traffic_id
    traffic_id=$(wait_for_traffic_id_by_url "/sse" 10)

    if [[ -z "$traffic_id" ]]; then
        log_fail "No SSE traffic found"
        return 1
    fi

    local code
    code=$(curl -s -o /dev/null -w "%{http_code}" \
        "http://${ADMIN_HOST}:${ADMIN_PORT}/_bifrost/api/traffic/${traffic_id}/frames")
    if ! assert_equals "200" "$code" "Frames API should remain available"; then
        return 1
    fi

    return 0
}

test_sse_updates_live_size() {
    clear_traffic >/dev/null 2>&1 || true
    sleep 0.5

    sse_fetch_all "$SSE_TARGET" "/sse/custom?count=400&interval=0.05" 15 > /dev/null 2>&1 &
    local sse_pid=$!

    local traffic_id
    traffic_id=$(wait_for_traffic_id_by_url "/sse/custom" 10)
    if [[ -z "$traffic_id" ]]; then
        log_fail "No SSE traffic found"
        kill_pid "$sse_pid"
        return 1
    fi

    local size
    local waited=0
    size=0
    while [[ $waited -lt 50 ]]; do
        local resp
        resp=$(curl -s "http://${ADMIN_HOST}:${ADMIN_PORT}/_bifrost/api/traffic/updates?after_seq=0&pending_ids=${traffic_id}&limit=1")
        size=$(echo "$resp" | jq -r --arg id "$traffic_id" '(.updated_records[]? , .new_records[]?) | select(.id==$id) | (.res_sz // 0)' | head -n 1)
        size="${size:-0}"
        if [[ "${size:-0}" -gt 0 ]]; then
            break
        fi
        sleep 0.1
        waited=$((waited + 1))
    done
    if [[ "${size:-0}" -le 0 ]]; then
        log_fail "SSE updates should include non-zero res_sz while open"
        log_debug "Updates response: $(curl -s "http://${ADMIN_HOST}:${ADMIN_PORT}/_bifrost/api/traffic/updates?after_seq=0&pending_ids=${traffic_id}&limit=1" | head -c 300)"
        kill_pid "$sse_pid"
        return 1
    fi

    wait_pid "$sse_pid"
    return 0
}

test_sse_traffic_identification() {
    sse_fetch_all "$SSE_TARGET" "/sse/custom?count=1&interval=0.01" 5 > /dev/null 2>&1

    sleep 1

    local traffic_id
    traffic_id=$(wait_for_traffic_id_by_url "/sse/custom" 10)

    if [[ -z "$traffic_id" ]]; then
        log_fail "No SSE traffic found"
        return 1
    fi

    local is_sse
    is_sse=$(is_sse_traffic "$traffic_id" | tr -d '[:space:]')
    if ! assert_equals "true" "$is_sse" "Traffic should be identified as SSE"; then
        return 1
    fi

    return 0
}

test_sse_vs_websocket_distinction() {
    sse_fetch_all "$SSE_TARGET" "/sse?count=1" 5 > /dev/null 2>&1

    sleep 1

    local traffic_list
    traffic_list=$(get_traffic_list "$ADMIN_HOST" "$ADMIN_PORT" 50)

    local sse_records
    local ws_records

    sse_records=$(echo "$traffic_list" | jq '[.records[] | select(.is_sse == true)] | length')
    ws_records=$(echo "$traffic_list" | jq '[.records[] | select(.is_websocket == true)] | length')

    local overlap
    overlap=$(echo "$traffic_list" | jq '[.records[] | select(.is_sse == true and .is_websocket == true)] | length')

    if ! assert_equals "0" "$overlap" "No traffic should be both SSE and WebSocket"; then
        log_debug "Traffic list: $traffic_list"
        return 1
    fi

    return 0
}

print_summary() {
    echo ""
    echo "======================================"
    echo "SSE Frames Test Summary"
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
    log_info "Starting SSE Frames Tests"
    log_info "Proxy: $PROXY_HOST:$PROXY_PORT"
    log_info "Admin: $ADMIN_HOST:$ADMIN_PORT"
    log_info "SSE Server: $SSE_HOST:$SSE_PORT"
    echo ""

    start_sse_server
    start_bifrost

    clear_traffic >/dev/null 2>&1 || true
    sleep 0.5

    run_test "SSE Basic Events" test_sse_basic_events
    run_test "SSE Custom Events" test_sse_custom_events
    run_test "SSE Multiline Data" test_sse_multiline_data
    run_test "SSE JSON Events" test_sse_json_events
    run_test "SSE Frames Capture" test_sse_frames_capture
    run_test "SSE Stream (Closed)" test_sse_stream_closed_behavior
    run_test "SSE Stream (Open)" test_sse_stream_open_full_and_live
    run_test "SSE Frame Content" test_sse_frame_content
    run_test "SSE Response Body Integrity" test_sse_response_body_integrity
    run_test "SSE Frames API Disabled" test_sse_frames_api_disabled
    run_test "SSE Updates Live Size" test_sse_updates_live_size
    run_test "SSE Traffic Identification" test_sse_traffic_identification
    run_test "SSE vs WebSocket Distinction" test_sse_vs_websocket_distinction

    print_summary
    exit $?
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    main "$@"
fi
