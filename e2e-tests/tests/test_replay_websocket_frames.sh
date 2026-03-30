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
WSS_PORT="${WSS_PORT:-}"
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
if [[ -z "$WSS_PORT" ]]; then
    WSS_PORT=$((WS_PORT + 1))
fi
if [[ -z "$ADMIN_PORT" ]]; then
    ADMIN_PORT="$PROXY_PORT"
fi

ADMIN_HOST_HEADER="${ADMIN_HOST}:${ADMIN_PORT}"
WS_BASE_URL="ws://${WS_HOST}:${WS_PORT}"
WSS_BASE_URL="wss://${WS_HOST}:${WSS_PORT}"

source "$SCRIPT_DIR/../test_utils/assert.sh"
source "$SCRIPT_DIR/../test_utils/admin_client.sh"
source "$SCRIPT_DIR/../test_utils/process.sh"

TESTS_PASSED=0
TESTS_FAILED=0

BIFROST_DATA_DIR=""
BIFROST_PID=""
WS_SERVER_PID=""
WSS_SERVER_PID=""

cleanup() {
    if [[ -n "$BIFROST_PID" ]]; then
        safe_cleanup_proxy "$BIFROST_PID"
    fi

    if [[ -n "$WS_SERVER_PID" ]]; then
        kill_pid "$WS_SERVER_PID"
        wait_pid "$WS_SERVER_PID"
    fi

    if [[ -n "$WSS_SERVER_PID" ]]; then
        kill_pid "$WSS_SERVER_PID"
        wait_pid "$WSS_SERVER_PID"
    fi

    if [[ -n "$BIFROST_DATA_DIR" && -d "$BIFROST_DATA_DIR" ]]; then
        rm -rf "$BIFROST_DATA_DIR"
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

wait_for_port() {
    local port="$1"
    local max_wait="${2:-10}"
    local waited=0
    while [[ $waited -lt $max_wait ]]; do
        if (echo >/dev/tcp/127.0.0.1/"$port") 2>/dev/null || \
           command -v nc &>/dev/null && nc -z 127.0.0.1 "$port" 2>/dev/null; then
            return 0
        fi
        sleep 0.5
        waited=$((waited + 1))
    done
    return 1
}

start_ws_server() {
    python3 "$SCRIPT_DIR/../mock_servers/ws_echo_server.py" --port "$WS_PORT" > /dev/null 2>&1 &
    WS_SERVER_PID=$!
    if ! wait_for_port "$WS_PORT" 20; then
        kill -0 "$WS_SERVER_PID" 2>/dev/null
        return 1
    fi
}

start_wss_server() {
    python3 "$SCRIPT_DIR/../mock_servers/ws_echo_server.py" --port "$WSS_PORT" --ssl > /dev/null 2>&1 &
    WSS_SERVER_PID=$!
    if ! wait_for_port "$WSS_PORT" 20; then
        kill -0 "$WSS_SERVER_PID" 2>/dev/null
        return 1
    fi
}

start_bifrost() {
    BIFROST_DATA_DIR="$(mktemp -d)"
    export BIFROST_DATA_DIR

    local log_file="$BIFROST_DATA_DIR/proxy.log"
    BIFROST_DATA_DIR="$BIFROST_DATA_DIR" "$BIFROST_BIN" -p "$PROXY_PORT" start --skip-cert-check --unsafe-ssl > "$log_file" 2>&1 &
    BIFROST_PID=$!
    sleep 1
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

ws_replay_generate_echo_traffic() {
    local path_suffix="${1:-/ws}"
    local messages="${2:-3}"
    local url="${WS_BASE_URL}${path_suffix}"
    local encoded
    encoded="$(python3 -c "import urllib.parse; print(urllib.parse.quote('''$url''', safe=''))")"
    python3 "$SCRIPT_DIR/../test_utils/ws_stress_client.py" \
        --proxy-host "$ADMIN_HOST" \
        --proxy-port "$ADMIN_PORT" \
        --host-header "$ADMIN_HOST_HEADER" \
        --path "${ADMIN_PATH_PREFIX}/api/replay/execute/ws?url=${encoded}" \
        --messages "$messages" \
        --timeout 15.0
}

ws_replay_generate_echo_traffic_wss() {
    local path_suffix="${1:-/ws}"
    local messages="${2:-3}"
    local url="${WSS_BASE_URL}${path_suffix}"
    local encoded
    encoded="$(python3 -c "import urllib.parse; print(urllib.parse.quote('''$url''', safe=''))")"
    python3 "$SCRIPT_DIR/../test_utils/ws_stress_client.py" \
        --proxy-host "$ADMIN_HOST" \
        --proxy-port "$ADMIN_PORT" \
        --host-header "$ADMIN_HOST_HEADER" \
        --path "${ADMIN_PATH_PREFIX}/api/replay/execute/ws?url=${encoded}" \
        --messages "$messages" \
        --timeout 20.0
}

ws_replay_connect_receive_only() {
    local upstream_url="$1"
    local extensions="${2:-}"
    local protocol="${3:-}"
    local expect_protocol="${4:-}"
    local expect_extensions="${5:-}"
    local read_seconds="${6:-10}"
    local expect_text_contains="${7:-}"
    local expect_text_count="${8:-1}"

    local encoded
    encoded="$(python3 -c "import urllib.parse; print(urllib.parse.quote('''$upstream_url''', safe=''))")"
    python3 "$SCRIPT_DIR/../test_utils/ws_stress_client.py" \
        --proxy-host "$ADMIN_HOST" \
        --proxy-port "$ADMIN_PORT" \
        --host-header "$ADMIN_HOST_HEADER" \
        --path "${ADMIN_PATH_PREFIX}/api/replay/execute/ws?url=${encoded}" \
        --timeout 80.0 \
        --messages 0 \
        --no-send \
        --read-seconds "$read_seconds" \
        ${extensions:+--extensions "$extensions"} \
        ${protocol:+--protocol "$protocol"} \
        ${expect_protocol:+--expect-protocol "$expect_protocol"} \
        ${expect_extensions:+--expect-extensions "$expect_extensions"} \
        ${expect_text_contains:+--expect-text-contains "$expect_text_contains"} \
        --expect-text-count "$expect_text_count"
}

ws_replay_send_invalid_control_fin0_expect_close() {
    local upstream_url="$1"
    local encoded
    encoded="$(python3 -c "import urllib.parse; print(urllib.parse.quote('''$upstream_url''', safe=''))")"
    python3 "$SCRIPT_DIR/../test_utils/ws_stress_client.py" \
        --proxy-host "$ADMIN_HOST" \
        --proxy-port "$ADMIN_PORT" \
        --host-header "$ADMIN_HOST_HEADER" \
        --path "${ADMIN_PATH_PREFIX}/api/replay/execute/ws?url=${encoded}" \
        --timeout 10.0 \
        --no-send \
        --send-invalid-control-fin0 \
        --expect-close
}

ws_replay_send_oversize_len_expect_close() {
    local upstream_url="$1"
    local oversize_len="$2"
    local encoded
    encoded="$(python3 -c "import urllib.parse; print(urllib.parse.quote('''$upstream_url''', safe=''))")"
    python3 "$SCRIPT_DIR/../test_utils/ws_stress_client.py" \
        --proxy-host "$ADMIN_HOST" \
        --proxy-port "$ADMIN_PORT" \
        --host-header "$ADMIN_HOST_HEADER" \
        --path "${ADMIN_PATH_PREFIX}/api/replay/execute/ws?url=${encoded}" \
        --timeout 10.0 \
        --no-send \
        --send-oversize-len "$oversize_len" \
        --expect-close
}

test_ws_replay_echo_forwarding() {
    log_test "Replay WebSocket echo forwarding"
    if ws_replay_generate_echo_traffic "/ws" 2; then
        pass "Echo frames forwarded via replay"
        return 0
    fi
    fail "Failed to forward echo frames via replay"
    return 1
}

test_ws_replay_frames_capture() {
    log_test "Replay WebSocket frames capture"

    clear_traffic >/dev/null 2>&1 || true
    sleep 0.5

    ws_replay_generate_echo_traffic "/ws" 3 >/dev/null 2>&1 || true
    sleep 1

    local traffic_id
    traffic_id=$(find_traffic_id_by_url "$ADMIN_HOST" "$ADMIN_PORT" "/ws" 20)
    if [[ -z "$traffic_id" || "$traffic_id" == "null" ]]; then
        fail "No WebSocket replay traffic recorded"
        return 1
    fi

    local record
    record=$(get_traffic_detail "$traffic_id")
    local is_replay is_ws status protocol
    is_replay=$(echo "$record" | jq -r '.is_replay // false')
    is_ws=$(echo "$record" | jq -r '.is_websocket // false')
    status=$(echo "$record" | jq -r '.status // 0')
    protocol=$(echo "$record" | jq -r '.protocol // ""')

    if [[ "$is_replay" != "true" ]]; then
        fail "Traffic not marked as replay"
        return 1
    fi
    if [[ "$is_ws" != "true" ]]; then
        fail "Traffic not marked as WebSocket"
        return 1
    fi
    if [[ "${status:-0}" -ne 101 ]]; then
        fail "Replay WebSocket status should be 101, got ${status}"
        return 1
    fi
    if [[ "$protocol" != "ws" && "$protocol" != "wss" ]]; then
        fail "Replay WebSocket protocol should be ws/wss, got ${protocol}"
        return 1
    fi

    local frames_response
    frames_response=$(get_frames "$traffic_id")
    local frame_count
    frame_count=$(echo "$frames_response" | jq -r '.frames | length')
    if [[ "$frame_count" -lt 1 ]]; then
        fail "Expected frames, got ${frame_count}"
        return 1
    fi

    local send_count receive_count
    send_count=$(echo "$frames_response" | jq '[.frames[] | select(.direction == "send")] | length')
    receive_count=$(echo "$frames_response" | jq '[.frames[] | select(.direction == "receive")] | length')
    if [[ "$send_count" -lt 1 || "$receive_count" -lt 1 ]]; then
        fail "Expected send and receive frames, got send=${send_count}, receive=${receive_count}"
        return 1
    fi

    local response_size
    response_size=$(echo "$record" | jq -r '.response_size // 0')
    if [[ "${response_size:-0}" -le 0 ]]; then
        fail "Replay WebSocket response_size should be persisted"
        return 1
    fi
    local socket_bytes
    socket_bytes=$(echo "$record" | jq -r '(.socket_status.send_bytes // 0) + (.socket_status.receive_bytes // 0)')
    if [[ "${socket_bytes:-0}" -le 0 ]]; then
        fail "Replay WebSocket socket_status bytes should be persisted"
        return 1
    fi

    pass "Captured ${frame_count} frames via replay"
    return 0
}

test_ws_replay_ping_pong_capture() {
    log_test "Replay WebSocket ping/pong forwarding and capture"

    clear_traffic >/dev/null 2>&1 || true
    sleep 0.5

    ws_replay_generate_echo_traffic "/ws/ping" 3 >/dev/null 2>&1 || true
    sleep 1

    local traffic_id
    traffic_id=$(find_traffic_id_by_url "$ADMIN_HOST" "$ADMIN_PORT" "/ws/ping" 20)
    if [[ -z "$traffic_id" || "$traffic_id" == "null" ]]; then
        fail "No traffic ID found"
        return 1
    fi

    local types
    types=$(get_frame_types "$traffic_id")
    if ! echo "$types" | grep -q "ping"; then
        fail "Expected Ping frame type"
        return 1
    fi
    if ! echo "$types" | grep -q "pong"; then
        fail "Expected Pong frame type"
        return 1
    fi

    pass "Observed ping/pong frames via replay"
    return 0
}

test_ws_replay_long_connection_over_30s() {
    log_test "Replay WebSocket long connection (>15s)"

    clear_traffic >/dev/null 2>&1 || true
    sleep 0.5

    local upstream_url="${WS_BASE_URL}/ws/idle?delay=15&msg=late_message"
    if ! ws_replay_connect_receive_only "$upstream_url" "" "" "" "" 25 "late_message" 1; then
        fail "Long connection was disconnected before late message"
        return 1
    fi

    pass "Long connection stayed alive and received late message"
    return 0
}

test_ws_replay_permessage_deflate_fragmentation_decompress() {
    log_test "Replay permessage-deflate + fragmentation capture/decompress"

    clear_traffic >/dev/null 2>&1 || true
    sleep 0.5

    local upstream_url="${WS_BASE_URL}/ws/deflate_frag"
    if ! ws_replay_connect_receive_only \
        "$upstream_url" \
        "permessage-deflate" \
        "" \
        "" \
        "permessage-deflate" \
        10 \
        "" \
        1; then
        fail "Failed to connect/receive deflate-frag traffic"
        return 1
    fi

    sleep 1
    local traffic_id
    traffic_id=$(find_traffic_id_by_url "$ADMIN_HOST" "$ADMIN_PORT" "/ws/deflate_frag" 50)
    if [[ -z "$traffic_id" || "$traffic_id" == "null" ]]; then
        fail "No WebSocket replay traffic recorded for deflate_frag"
        return 1
    fi

    local frames
    frames=$(get_frames "$traffic_id")
    local candidate_frame_id
    candidate_frame_id=$(echo "$frames" | jq -r '.frames[] | select(.direction=="receive" and ((.payload_preview // "") | contains("hello-deflate-frag"))) | .frame_id' | tail -1)
    if [[ -z "$candidate_frame_id" ]]; then
        fail "No receive frames captured for deflate_frag"
        return 1
    fi

    local detail
    detail=$(get_frame_detail "$traffic_id" "$candidate_frame_id")
    local full_payload
    full_payload=$(echo "$detail" | jq -r '.full_payload // ""')
    if ! echo "$full_payload" | grep -q "hello-deflate-frag"; then
        fail "Decompressed payload not found in frame detail"
        return 1
    fi

    local raw_size payload_size
    raw_size=$(echo "$detail" | jq -r '.frame.raw_payload_size // 0')
    payload_size=$(echo "$detail" | jq -r '.frame.payload_size // 0')
    if [[ "$raw_size" -le 0 || "$payload_size" -le 0 ]]; then
        fail "Expected both raw and decompressed payload sizes"
        return 1
    fi

    pass "Captured compressed fragments and decompressed message for preview/detail"
    return 0
}

test_ws_replay_reject_invalid_control_frame() {
    log_test "Replay rejects invalid control frame (FIN=0)"

    local upstream_url="${WS_BASE_URL}/ws"
    if ws_replay_send_invalid_control_fin0_expect_close "$upstream_url"; then
        if curl -sf "http://${ADMIN_HOST}:${ADMIN_PORT}${ADMIN_PATH_PREFIX}/api/system" >/dev/null 2>&1; then
            pass "Invalid control frame caused expected close without crashing server"
            return 0
        fi
    fi

    fail "Invalid control frame test failed"
    return 1
}

test_ws_replay_reject_oversize_payload_len_header() {
    log_test "Replay rejects oversize payload length header"

    local upstream_url="${WS_BASE_URL}/ws"
    # 触发 replay_ws 的 MAX_FRAME_PAYLOAD_LEN(16MiB) 防御性校验
    local oversize=$((17 * 1024 * 1024))
    if ws_replay_send_oversize_len_expect_close "$upstream_url" "$oversize"; then
        pass "Oversize length header caused expected close"
        return 0
    fi

    fail "Oversize length header test failed"
    return 1
}

test_ws_replay_wss_upstream_forwarding() {
    log_test "Replay wss upstream forwarding"

    clear_traffic >/dev/null 2>&1 || true
    sleep 0.5

    if ws_replay_generate_echo_traffic_wss "/ws" 2; then
        sleep 1
        local traffic_id
        traffic_id=$(find_traffic_id_by_url "$ADMIN_HOST" "$ADMIN_PORT" "/ws" 50)
        if [[ -n "$traffic_id" && "$traffic_id" != "null" ]]; then
            local record protocol
            record=$(get_traffic_detail "$traffic_id")
            protocol=$(echo "$record" | jq -r '.protocol // ""')
            if [[ "$protocol" == "wss" ]]; then
                pass "wss upstream replay recorded as wss"
                return 0
            fi
        fi
    fi

    fail "wss upstream replay forwarding failed"
    return 1
}

test_ws_replay_subprotocol_negotiation() {
    log_test "Replay subprotocol negotiation"

    local upstream_url="${WS_BASE_URL}/ws"
    # 仅校验握手头（不依赖上游主动推送消息）
    if ws_replay_connect_receive_only "$upstream_url" "" "chat" "chat" "" 0 "" 1; then
        pass "Subprotocol negotiated and reflected in handshake"
        return 0
    fi

    fail "Subprotocol negotiation failed"
    return 1
}

main() {
    check_deps
    start_ws_server
    start_wss_server
    start_bifrost

    test_ws_replay_echo_forwarding || true
    test_ws_replay_frames_capture || true
    test_ws_replay_ping_pong_capture || true
    test_ws_replay_long_connection_over_30s || true
    test_ws_replay_permessage_deflate_fragmentation_decompress || true
    test_ws_replay_reject_invalid_control_frame || true
    test_ws_replay_reject_oversize_payload_len_header || true
    test_ws_replay_wss_upstream_forwarding || true
    test_ws_replay_subprotocol_negotiation || true

    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "Replay WebSocket E2E Results: PASSED=$TESTS_PASSED FAILED=$TESTS_FAILED"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    if [[ $TESTS_FAILED -gt 0 ]]; then
        exit 1
    fi
}

main "$@"
