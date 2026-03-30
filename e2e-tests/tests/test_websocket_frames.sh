#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
BIFROST_BIN="${ROOT_DIR}/target/release/bifrost"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi

PROXY_HOST="${PROXY_HOST:-127.0.0.1}"
PROXY_PORT="${PROXY_PORT:-}"
WS_HOST="${WS_HOST:-127.0.0.1}"
WS_PORT="${WS_PORT:-}"
ADMIN_HOST="${ADMIN_HOST:-$PROXY_HOST}"
ADMIN_PORT="${ADMIN_PORT:-}"
ADMIN_PATH_PREFIX="${ADMIN_PATH_PREFIX:-/_bifrost}"
export ADMIN_PATH_PREFIX

if [[ -z "$PROXY_PORT" ]]; then
    PROXY_PORT=$((19000 + ($$ % 1000)))
fi
if [[ -z "$WS_PORT" ]]; then
    WS_PORT=$((20000 + ($$ % 1000)))
fi
if [[ -z "$ADMIN_PORT" ]]; then
    ADMIN_PORT="$PROXY_PORT"
fi

WS_HOST_HEADER="${WS_HOST}:${WS_PORT}"

source "$SCRIPT_DIR/../test_utils/assert.sh"
source "$SCRIPT_DIR/../test_utils/admin_client.sh"
source "$SCRIPT_DIR/../test_utils/rule_fixture.sh"
source "$SCRIPT_DIR/../test_utils/process.sh"

TESTS_PASSED=0
TESTS_FAILED=0

BIFROST_DATA_DIR="${BIFROST_DATA_DIR:-}"
BIFROST_PID=""
WS_SERVER_PID=""
RULE_FIXTURE="$SCRIPT_DIR/../rules/websocket/decode_utf8_searchable.txt"

cleanup() {
    if [[ -n "$BIFROST_PID" ]]; then
        safe_cleanup_proxy "$BIFROST_PID"
    fi

    if [[ -n "$WS_SERVER_PID" ]]; then
        kill_pid "$WS_SERVER_PID"
        wait_pid "$WS_SERVER_PID"
    fi

    if [[ -n "$BIFROST_DATA_DIR" && -d "$BIFROST_DATA_DIR" ]]; then
        if [[ "$BIFROST_DATA_DIR" == "$ROOT_DIR/.bifrost-e2e"* ]]; then
            rm -rf "$BIFROST_DATA_DIR"
        fi
    fi

    if is_windows; then kill_bifrost_on_port "$PROXY_PORT"; fi
}

trap cleanup EXIT

log_test() {
    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "🧪 TEST: $1"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
}

pass() {
    echo "✅ PASS: $1"
    TESTS_PASSED=$((TESTS_PASSED + 1))
}

fail() {
    echo "❌ FAIL: $1"
    TESTS_FAILED=$((TESTS_FAILED + 1))
}

check_deps() {
    if ! command -v python3 &> /dev/null; then
        echo "Error: python3 is required"
        exit 1
    fi
    if ! command -v jq &> /dev/null; then
        echo "Error: jq is required. Install with: brew install jq"
        exit 1
    fi
}

start_ws_server() {
    python3 "$SCRIPT_DIR/../mock_servers/ws_echo_server.py" --port "$WS_PORT" > /dev/null 2>&1 &
    WS_SERVER_PID=$!
    local max_wait=10
    if is_windows; then max_wait=20; fi
    local waited=0
    while [[ $waited -lt $max_wait ]]; do
        if kill -0 "$WS_SERVER_PID" 2>/dev/null; then
            if python3 -c "import socket; s=socket.create_connection(('${WS_HOST}', ${WS_PORT}), 2); s.close()" 2>/dev/null; then
                return 0
            fi
        fi
        sleep 0.5
        waited=$((waited + 1))
    done
    kill -0 "$WS_SERVER_PID" 2>/dev/null
}

start_bifrost() {
    if [[ -z "$BIFROST_DATA_DIR" ]]; then
        BIFROST_DATA_DIR="$ROOT_DIR/.bifrost-e2e-test/ws_frames_${PROXY_PORT}_$$"
    fi
    mkdir -p "$BIFROST_DATA_DIR"
    export BIFROST_DATA_DIR

    local log_file="$BIFROST_DATA_DIR/proxy.log"
    BIFROST_DATA_DIR="$BIFROST_DATA_DIR" "$BIFROST_BIN" -p "$PROXY_PORT" start --skip-cert-check --unsafe-ssl > "$log_file" 2>&1 &
    BIFROST_PID=$!
    local init_wait=1
    if is_windows; then init_wait=3; fi
    sleep "$init_wait"
    if ! kill -0 "$BIFROST_PID" 2>/dev/null; then
        tail -n 120 "$log_file" || true
        return 1
    fi

    local max_wait=60
    local waited=0
    while [[ $waited -lt $max_wait ]]; do
        if ! kill -0 "$BIFROST_PID" 2>/dev/null; then
            echo "Bifrost process exited during startup (PID: $BIFROST_PID)"
            tail -n 120 "$log_file" || true
            return 1
        fi
        if curl -sf "http://${ADMIN_HOST}:${ADMIN_PORT}${ADMIN_PATH_PREFIX}/api/system" >/dev/null 2>&1; then
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done
    tail -n 120 "$log_file" || true
    return 1
}

ws_generate_echo_traffic() {
    local messages="${1:-3}"
    local timeout=15.0
    if is_windows; then timeout=30.0; fi
    python3 "$SCRIPT_DIR/../test_utils/ws_stress_client.py" \
        --proxy-host "$PROXY_HOST" \
        --proxy-port "$PROXY_PORT" \
        --host-header "$WS_HOST_HEADER" \
        --path "/ws" \
        --messages "$messages" \
        --timeout "$timeout"
}

is_ws_record() {
    local traffic_id="$1"
    local record
    record=$(get_traffic_detail "$traffic_id")
    local is_ws
    is_ws=$(echo "$record" | jq -r '.is_websocket // false')
    if [[ "$is_ws" == "true" ]]; then
        return 0
    fi
    local flags
    flags=$(echo "$record" | jq -r '.flags // 0')
    [[ $(( (flags / 2) % 2 )) -eq 1 ]]
}

test_ws_text_frame_forwarding() {
    log_test "WebSocket text frame forwarding (via proxy)"
    if ws_generate_echo_traffic 2; then
        pass "Echo frames forwarded"
        return 0
    fi
    fail "Failed to forward echo frames"
    return 1
}

test_ws_frames_capture() {
    log_test "WebSocket frames capture"

    clear_traffic >/dev/null 2>&1 || true
    sleep 0.5

    ws_generate_echo_traffic 3 >/dev/null 2>&1 || true
    sleep 2

    local traffic_id=""
    local find_attempt=0
    while [[ $find_attempt -lt 5 ]]; do
        traffic_id=$(find_traffic_id_by_url "$ADMIN_HOST" "$ADMIN_PORT" "/ws" 20)
        if [[ -n "$traffic_id" && "$traffic_id" != "null" ]]; then
            break
        fi
        sleep 2
        find_attempt=$((find_attempt + 1))
    done
    if [[ -z "$traffic_id" || "$traffic_id" == "null" ]]; then
        fail "No WebSocket traffic recorded"
        return 1
    fi

    if ! is_ws_record "$traffic_id"; then
        fail "Traffic not marked as WebSocket"
        return 1
    fi

    local frames_response
    frames_response=$(get_frames "$traffic_id")
    local frame_count
    frame_count=$(echo "$frames_response" | jq -r '.frames | length')
    if [[ "$frame_count" -ge 1 ]]; then
        local first_frame_id
        first_frame_id=$(echo "$frames_response" | jq -r '.frames[0].frame_id // 0')
        if [[ "${first_frame_id:-0}" -le 0 ]]; then
            fail "Frame id should be available"
            return 1
        fi
        local frame_detail
        frame_detail=$(get_frame_detail "$traffic_id" "$first_frame_id")
        local full_payload
        full_payload=$(echo "$frame_detail" | jq -r '.full_payload // ""')
        if [[ -z "$full_payload" ]]; then
            fail "Captured frames but full_payload is empty"
            return 1
        fi
        local record
        record=$(get_traffic_detail "$traffic_id")
        local response_size
        response_size=$(echo "$record" | jq -r '.response_size // 0')
        if [[ "${response_size:-0}" -le 0 ]]; then
            fail "WebSocket response_size should be persisted"
            return 1
        fi
        local socket_bytes
        socket_bytes=$(echo "$record" | jq -r '(.socket_status.send_bytes // 0) + (.socket_status.receive_bytes // 0)')
        if [[ "${socket_bytes:-0}" -le 0 ]]; then
            fail "WebSocket socket_status bytes should be persisted"
            return 1
        fi
        pass "Captured $frame_count frames with full_payload"
        return 0
    fi

    fail "Expected frames, got $frame_count"
    return 1
}

test_ws_frame_directions() {
    log_test "WebSocket frame directions"

    clear_traffic >/dev/null 2>&1 || true
    sleep 0.5

    ws_generate_echo_traffic 1 >/dev/null 2>&1 || true
    sleep 2

    local traffic_id=""
    local find_attempt=0
    while [[ $find_attempt -lt 5 ]]; do
        traffic_id=$(find_traffic_id_by_url "$ADMIN_HOST" "$ADMIN_PORT" "/ws" 20)
        if [[ -n "$traffic_id" && "$traffic_id" != "null" ]]; then
            break
        fi
        sleep 2
        find_attempt=$((find_attempt + 1))
    done
    if [[ -z "$traffic_id" || "$traffic_id" == "null" ]]; then
        fail "No traffic ID found"
        return 1
    fi

    local frames_response
    frames_response=$(get_frames "$traffic_id")

    local send_count receive_count
    send_count=$(echo "$frames_response" | jq '[.frames[] | select(.direction == "send")] | length')
    receive_count=$(echo "$frames_response" | jq '[.frames[] | select(.direction == "receive")] | length')

    if [[ "$send_count" -ge 1 && "$receive_count" -ge 1 ]]; then
        pass "Found send ($send_count) and receive ($receive_count) frames"
        return 0
    fi

    fail "Expected both send and receive frames (send=$send_count, receive=$receive_count)"
    return 1
}

test_ws_connection_list() {
    log_test "WebSocket connection list API"

    local connections
    connections=$(list_websocket_connections)
    if echo "$connections" | jq -e 'type == "array" or type == "object"' >/dev/null 2>&1; then
        pass "Connection list API returned valid JSON"
        return 0
    fi

    fail "Invalid connection list response: $connections"
    return 1
}

test_ws_binary_payload_decode_and_search() {
    log_test "WebSocket binary payload decode://utf8 + searchable"

    delete_rule "ws_decode_test" >/dev/null 2>&1 || true
    create_rule "ws_decode_test" "$(rule_fixture_content "$RULE_FIXTURE")" true >/dev/null 2>&1 || true
    enable_rule "ws_decode_test" >/dev/null 2>&1 || true

    clear_traffic >/dev/null 2>&1 || true
    sleep 0.5

    local msg="hello_decode_test"
    python3 "$SCRIPT_DIR/../test_utils/ws_stress_client.py" \
        --proxy-host "$PROXY_HOST" \
        --proxy-port "$PROXY_PORT" \
        --host-header "$WS_HOST_HEADER" \
        --path "/ws/binary" \
        --message "$msg" \
        --messages 2 \
        --timeout 15.0 \
        --expect-binary >/dev/null 2>&1 || true
    sleep 2

    local traffic_id=""
    local find_attempt=0
    while [[ $find_attempt -lt 5 ]]; do
        traffic_id=$(find_traffic_id_by_url "$ADMIN_HOST" "$ADMIN_PORT" "/ws/binary" 20)
        if [[ -n "$traffic_id" && "$traffic_id" != "null" ]]; then
            break
        fi
        sleep 2
        find_attempt=$((find_attempt + 1))
    done
    if [[ -z "$traffic_id" || "$traffic_id" == "null" ]]; then
        fail "No WebSocket binary traffic recorded"
        return 1
    fi

    local frames_response
    frames_response=$(get_frames "$traffic_id")
    local binary_recv_frame_id
    binary_recv_frame_id=$(echo "$frames_response" | jq -r '.frames[] | select(.direction=="receive" and .frame_type=="binary") | .frame_id' | head -n 1)
    if [[ -z "$binary_recv_frame_id" || "$binary_recv_frame_id" == "null" || "${binary_recv_frame_id:-0}" -le 0 ]]; then
        fail "No receive binary frame found"
        return 1
    fi

    local frame_detail
    frame_detail=$(get_frame_detail "$traffic_id" "$binary_recv_frame_id")
    local full_payload
    full_payload=$(echo "$frame_detail" | jq -r '.full_payload // ""')
    if [[ "$full_payload" != *"$msg"* ]]; then
        fail "Decoded full_payload should contain message (got: $full_payload)"
        return 1
    fi

    local expected_raw_b64
    expected_raw_b64=$(python3 -c "import base64; print(base64.b64encode(b'$msg').decode())")
    local raw_full_payload
    raw_full_payload=$(echo "$frame_detail" | jq -r '.raw_full_payload // ""')
    if [[ -n "$raw_full_payload" && "$raw_full_payload" != "$expected_raw_b64" ]]; then
        fail "raw_full_payload should be base64 of message (got: $raw_full_payload)"
        return 1
    fi

    local search_response
    search_response=$(admin_post "/api/search" "{\"keyword\":\"$msg\"}")
    if ! echo "$search_response" | jq -e --arg id "$traffic_id" '.results[].record.id | select(. == $id)' >/dev/null 2>&1; then
        fail "Search should find traffic by decoded ws payload"
        return 1
    fi

    pass "Decoded binary payload stored & searchable"
    return 0
}

run_all_tests() {
    check_deps
    start_ws_server
    start_bifrost

    test_ws_text_frame_forwarding || true
    test_ws_frames_capture || true
    test_ws_frame_directions || true
    test_ws_binary_payload_decode_and_search || true
    test_ws_connection_list || true

    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "📊 TEST SUMMARY"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "  Passed:  $TESTS_PASSED"
    echo "  Failed:  $TESTS_FAILED"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    if [[ $TESTS_FAILED -gt 0 ]]; then
        exit 1
    fi
}

run_all_tests
