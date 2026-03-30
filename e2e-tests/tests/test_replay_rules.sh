#!/bin/bash
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
BIFROST_BIN="${BIFROST_BIN:-${ROOT_DIR}/target/release/bifrost}"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi

PROXY_PORT="${PROXY_PORT:-18888}"
MOCK_HTTP_PORT="${MOCK_HTTP_PORT:-13000}"
MOCK_SSE_PORT="${MOCK_SSE_PORT:-13001}"
MOCK_WS_PORT="${MOCK_WS_PORT:-13002}"
ADMIN_PORT="$PROXY_PORT"
ADMIN_BASE_URL="http://127.0.0.1:${ADMIN_PORT}/_bifrost"

source "$SCRIPT_DIR/../test_utils/assert.sh"
source "$SCRIPT_DIR/../test_utils/http_client.sh"
source "$SCRIPT_DIR/../test_utils/admin_client.sh"
source "$SCRIPT_DIR/../test_utils/ws_client.sh"
source "$SCRIPT_DIR/../test_utils/rule_fixture.sh"
source "$SCRIPT_DIR/../test_utils/process.sh"

BIFROST_PID=""
MOCK_HTTP_PID=""
MOCK_SSE_PID=""
MOCK_WS_PID=""
passed=0
failed=0
RULES_DIR="$SCRIPT_DIR/../rules/replay"

cleanup() {
    echo ""
    echo "Cleaning up..."
    
    if [ -n "$BIFROST_PID" ]; then
        echo "  Stopping Bifrost proxy (PID: $BIFROST_PID)..."
        safe_cleanup_proxy "$BIFROST_PID"
    fi
    
    if [ -n "$MOCK_HTTP_PID" ]; then
        echo "  Stopping Mock HTTP server (PID: $MOCK_HTTP_PID)..."
        kill_pid "$MOCK_HTTP_PID"
        wait_pid "$MOCK_HTTP_PID"
    fi

    if [ -n "$MOCK_SSE_PID" ]; then
        echo "  Stopping Mock SSE server (PID: $MOCK_SSE_PID)..."
        kill_pid "$MOCK_SSE_PID"
        wait_pid "$MOCK_SSE_PID"
    fi

    if [ -n "$MOCK_WS_PID" ]; then
        echo "  Stopping Mock WS server (PID: $MOCK_WS_PID)..."
        kill_pid "$MOCK_WS_PID"
        wait_pid "$MOCK_WS_PID"
    fi

    if is_windows; then kill_bifrost_on_port "$PROXY_PORT"; fi
    echo "Cleanup complete."
}

urlencode() {
    python3 - "$1" <<'PY'
import sys, urllib.parse
print(urllib.parse.quote(sys.argv[1], safe=''))
PY
}

replay_rule_config_from_fixture() {
    local fixture_name="$1"
    shift || true
    custom_rule_config_from_fixture "$RULES_DIR/$fixture_name" "$@"
}

trap cleanup EXIT

start_mock_server() {
    echo "Starting Mock HTTP Echo Server on port $MOCK_HTTP_PORT..."
    python3 "$SCRIPT_DIR/../mock_servers/http_echo_server.py" "$MOCK_HTTP_PORT" > /dev/null 2>&1 &
    MOCK_HTTP_PID=$!
    
    local timeout=10
    local waited=0
    while [ $waited -lt $timeout ]; do
        if curl -s "http://127.0.0.1:${MOCK_HTTP_PORT}/health" >/dev/null 2>&1; then
            echo "  Mock HTTP server started (PID: $MOCK_HTTP_PID)"
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done
    
    if ! kill -0 "$MOCK_HTTP_PID" 2>/dev/null; then
        echo "Failed to start Mock HTTP server"
        exit 1
    fi
    echo "  Mock HTTP server started (PID: $MOCK_HTTP_PID)"
}

start_sse_server() {
    echo "Starting Mock SSE Echo Server on port $MOCK_SSE_PORT..."
    python3 "$SCRIPT_DIR/../mock_servers/sse_echo_server.py" --port "$MOCK_SSE_PORT" > /dev/null 2>&1 &
    MOCK_SSE_PID=$!

    local timeout=10
    local waited=0
    while [ $waited -lt $timeout ]; do
        if curl -s "http://127.0.0.1:${MOCK_SSE_PORT}/health" >/dev/null 2>&1; then
            echo "  Mock SSE server started (PID: $MOCK_SSE_PID)"
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done

    if ! kill -0 "$MOCK_SSE_PID" 2>/dev/null; then
        echo "Failed to start Mock SSE server"
        exit 1
    fi
    echo "  Mock SSE server started (PID: $MOCK_SSE_PID)"
}

start_ws_server() {
    echo "Starting Mock HTTP+WS Echo Server on port $MOCK_WS_PORT..."
    python3 "$SCRIPT_DIR/../mock_servers/http_ws_echo_server.py" "$MOCK_WS_PORT" > /dev/null 2>&1 &
    MOCK_WS_PID=$!

    local timeout=10
    local waited=0
    while [ $waited -lt $timeout ]; do
        if curl -s "http://127.0.0.1:${MOCK_WS_PORT}/" >/dev/null 2>&1; then
            echo "  Mock WS server started (PID: $MOCK_WS_PID)"
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done

    if ! kill -0 "$MOCK_WS_PID" 2>/dev/null; then
        echo "Failed to start Mock WS server"
        exit 1
    fi
    echo "  Mock WS server started (PID: $MOCK_WS_PID)"
}

start_bifrost() {
    echo "Starting Bifrost proxy on port $PROXY_PORT..."
    cd "$ROOT_DIR"

    BIFROST_DATA_DIR="${BIFROST_DATA_DIR:-./.bifrost-e2e-test}" \
        "$BIFROST_BIN" start -p "$PROXY_PORT" --unsafe-ssl --skip-cert-check > /tmp/bifrost_e2e.log 2>&1 &
    BIFROST_PID=$!
    
    local timeout=120
    local waited=0
    while [ $waited -lt $timeout ]; do
        if ! kill -0 "$BIFROST_PID" 2>/dev/null; then
            echo "Bifrost process exited unexpectedly (PID: $BIFROST_PID)"
            echo "Last log:"
            tail -30 /tmp/bifrost_e2e.log
            exit 1
        fi
        if curl -s "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/system" >/dev/null 2>&1; then
            echo "  Bifrost proxy started (PID: $BIFROST_PID)"
            return 0
        fi
        sleep 2
        waited=$((waited + 2))
    done
    
    echo "Failed to start Bifrost proxy within ${timeout}s"
    echo "Last log:"
    tail -30 /tmp/bifrost_e2e.log
    exit 1
}

replay_request() {
    local url="$1"
    local method="${2:-GET}"
    local headers_json="${3:-[]}"
    local body="$4"
    local rule_config="$5"
    local timeout="${6:-15000}"
    
    if [ -z "$rule_config" ]; then
        rule_config='{"mode":"none"}'
    fi
    
    local body_field=""
    if [ -n "$body" ]; then
        body_field=",\"body\":\"$body\""
    fi

    # replay HTTP 端点已统一为 unified payload（与 /api/replay/execute/unified 相同）
    local request_json
    request_json="{\"url\":\"$url\",\"method\":\"$method\",\"headers\":$headers_json$body_field,\"rule_config\":$rule_config,\"timeout_ms\":$timeout}"
    
    if [ "${DEBUG:-}" = "1" ]; then
        echo "[DEBUG] Request JSON: $request_json" >&2
    fi
    
    local response
    response=$(curl -s -X POST "${ADMIN_BASE_URL}/api/replay/execute" \
        -H "Content-Type: application/json" \
        -d "$request_json")
    
    if [ "${DEBUG:-}" = "1" ]; then
        echo "[DEBUG] Response: $response" >&2
    fi
    
    printf '%s' "$response"
}

test_reqHeaders_rule() {
    echo ""
    echo "=== Test: reqHeaders Rule ==="
    
    local url="http://127.0.0.1:${MOCK_HTTP_PORT}/test-headers"
    local rule_config
    rule_config=$(replay_rule_config_from_fixture req_headers.txt)
    
    local response
    response=$(replay_request "$url" "GET" '[["Accept", "application/json"]]' "" "$rule_config")
    
    local status
    status=$(printf '%s' "$response" | jq -r '.data.status // empty')
    
    local received_header
    received_header=$(printf '%s' "$response" | jq -r '.data.body | fromjson | .request.headers["x-custom-header"] // empty')
    
    if [ "$status" = "200" ] && [ "$received_header" = "custom-value-123" ]; then
        _log_pass "reqHeaders rule applied: X-Custom-Header=custom-value-123"
        ((passed++))
    else
        _log_fail "reqHeaders rule not applied" "custom-value-123" "$received_header"
        ((failed++))
    fi
    
    local applied_rules
    applied_rules=$(printf '%s' "$response" | jq -r '.data.applied_rules | length // 0')
    if [ "$applied_rules" -gt 0 ]; then
        _log_pass "applied_rules returned: count=$applied_rules"
        ((passed++))
    else
        _log_fail "applied_rules not returned" ">0" "$applied_rules"
        ((failed++))
    fi
}

test_host_rule() {
    echo ""
    echo "=== Test: Host Rule ==="
    
    local url="http://fake-host.local:${MOCK_HTTP_PORT}/test-host"
    local rule_config
    rule_config=$(replay_rule_config_from_fixture host_redirect.txt "MOCK_HTTP_PORT=${MOCK_HTTP_PORT}")
    
    local response
    response=$(replay_request "$url" "GET" '[["Accept", "application/json"]]' "" "$rule_config")
    
    local status
    status=$(printf '%s' "$response" | jq -r '.data.status // empty')
    
    local server_port
    server_port=$(printf '%s' "$response" | jq -r '.data.body | fromjson | .server.port // empty')
    
    if [ "$status" = "200" ] && [ "$server_port" = "$MOCK_HTTP_PORT" ]; then
        _log_pass "Host rule applied: redirected to 127.0.0.1:${MOCK_HTTP_PORT}"
        ((passed++))
    else
        _log_fail "Host rule not applied" "port=$MOCK_HTTP_PORT" "status=$status, port=$server_port"
        ((failed++))
    fi
}

test_method_rule() {
    echo ""
    echo "=== Test: Method Rule ==="
    
    local url="http://127.0.0.1:${MOCK_HTTP_PORT}/test-method"
    local rule_config
    rule_config=$(replay_rule_config_from_fixture method_post.txt)
    
    local response
    response=$(replay_request "$url" "GET" '[["Accept", "application/json"]]' "" "$rule_config")
    
    local status
    status=$(printf '%s' "$response" | jq -r '.data.status // empty')
    
    local received_method
    received_method=$(printf '%s' "$response" | jq -r '.data.body | fromjson | .request.method // empty')
    
    if [ "$status" = "200" ] && [ "$received_method" = "POST" ]; then
        _log_pass "Method rule applied: GET -> POST"
        ((passed++))
    else
        _log_fail "Method rule not applied" "POST" "$received_method"
        ((failed++))
    fi
}

test_ua_rule() {
    echo ""
    echo "=== Test: User-Agent Rule ==="
    
    local url="http://127.0.0.1:${MOCK_HTTP_PORT}/test-ua"
    local rule_config
    rule_config=$(replay_rule_config_from_fixture user_agent.txt)
    
    local response
    response=$(replay_request "$url" "GET" '[["Accept", "application/json"]]' "" "$rule_config")
    
    local status
    status=$(printf '%s' "$response" | jq -r '.data.status // empty')
    
    local received_ua
    received_ua=$(printf '%s' "$response" | jq -r '.data.body | fromjson | .request.headers["user-agent"] // empty')
    
    if [ "$status" = "200" ] && [ "$received_ua" = "CustomUA/1.0-test" ]; then
        _log_pass "UA rule applied: User-Agent=CustomUA/1.0-test"
        ((passed++))
    else
        _log_fail "UA rule not applied" "CustomUA/1.0-test" "$received_ua"
        ((failed++))
    fi
}

test_referer_rule() {
    echo ""
    echo "=== Test: Referer Rule ==="
    
    local url="http://127.0.0.1:${MOCK_HTTP_PORT}/test-referer"
    local rule_config
    rule_config=$(replay_rule_config_from_fixture referer.txt)
    
    local response
    response=$(replay_request "$url" "GET" '[["Accept", "application/json"]]' "" "$rule_config")
    
    local status
    status=$(printf '%s' "$response" | jq -r '.data.status // empty')
    
    local received_referer
    received_referer=$(printf '%s' "$response" | jq -r '.data.body | fromjson | .request.headers["referer"] // empty')
    
    if [ "$status" = "200" ] && [ "$received_referer" = "https://example.com/page" ]; then
        _log_pass "Referer rule applied: Referer=https://example.com/page"
        ((passed++))
    else
        _log_fail "Referer rule not applied" "https://example.com/page" "$received_referer"
        ((failed++))
    fi
}

test_urlParams_rule() {
    echo ""
    echo "=== Test: urlParams Rule ==="
    
    local url="http://127.0.0.1:${MOCK_HTTP_PORT}/test-params?existing=value"
    local rule_config
    rule_config=$(replay_rule_config_from_fixture url_params.txt)
    
    local response
    response=$(replay_request "$url" "GET" '[["Accept", "application/json"]]' "" "$rule_config")
    
    local status
    status=$(printf '%s' "$response" | jq -r '.data.status // empty')
    
    local existing_param
    existing_param=$(printf '%s' "$response" | jq -r '.data.body | fromjson | .request.query_params.existing[0] // empty')
    
    local added_param
    added_param=$(printf '%s' "$response" | jq -r '.data.body | fromjson | .request.query_params.added_param[0] // empty')
    
    if [ "$status" = "200" ] && [ "$existing_param" = "value" ] && [ "$added_param" = "new_value" ]; then
        _log_pass "urlParams rule applied: added_param=new_value (existing param preserved)"
        ((passed++))
    else
        _log_fail "urlParams rule not applied" "existing=value, added_param=new_value" "existing=$existing_param, added_param=$added_param"
        ((failed++))
    fi
}

test_reqCookies_rule() {
    echo ""
    echo "=== Test: reqCookies Rule ==="
    
    local url="http://127.0.0.1:${MOCK_HTTP_PORT}/test-cookies"
    local rule_config
    rule_config=$(replay_rule_config_from_fixture req_cookies.txt)
    
    local response
    response=$(replay_request "$url" "GET" '[["Accept", "application/json"]]' "" "$rule_config")
    
    local status
    status=$(printf '%s' "$response" | jq -r '.data.status // empty')
    
    local received_cookie
    received_cookie=$(printf '%s' "$response" | jq -r '.data.body | fromjson | .request.cookies.session_id // empty')
    
    if [ "$status" = "200" ] && [ "$received_cookie" = "abc123" ]; then
        _log_pass "reqCookies rule applied: session_id=abc123"
        ((passed++))
    else
        _log_fail "reqCookies rule not applied" "abc123" "$received_cookie"
        ((failed++))
    fi
}

test_reqBody_rule() {
    echo ""
    echo "=== Test: reqBody Rule ==="
    
    local url="http://127.0.0.1:${MOCK_HTTP_PORT}/test-body"
    local rule_config
    rule_config=$(replay_rule_config_from_fixture req_body.txt)
    
    local response
    response=$(replay_request "$url" "POST" '[["Content-Type", "text/plain"]]' "original_body" "$rule_config")
    
    local status
    status=$(printf '%s' "$response" | jq -r '.data.status // empty')
    
    local received_body
    received_body=$(printf '%s' "$response" | jq -r '.data.body | fromjson | .request.body // empty')
    
    if [ "$status" = "200" ] && [ "$received_body" = "replaced_body_content" ]; then
        _log_pass "reqBody rule applied: body replaced"
        ((passed++))
    else
        _log_fail "reqBody rule not applied" "replaced_body_content" "$received_body"
        ((failed++))
    fi
}

test_delete_header_rule() {
    echo ""
    echo "=== Test: Delete Header Rule ==="
    
    local url="http://127.0.0.1:${MOCK_HTTP_PORT}/test-delete-header"
    local rule_config
    rule_config=$(replay_rule_config_from_fixture delete_header.txt)
    
    local response
    response=$(replay_request "$url" "GET" '[["Accept", "application/json"], ["X-Remove-Me", "should-be-removed"]]' "" "$rule_config")
    
    local status
    status=$(printf '%s' "$response" | jq -r '.data.status // empty')
    
    local removed_header
    removed_header=$(printf '%s' "$response" | jq -r '.data.body | fromjson | .request.headers["x-remove-me"] // "null"')
    
    if [ "$status" = "200" ] && [ "$removed_header" = "null" ]; then
        _log_pass "Delete header rule applied: X-Remove-Me removed"
        ((passed++))
    else
        _log_fail "Delete header rule not applied" "null (removed)" "$removed_header"
        ((failed++))
    fi
}

test_multiple_rules() {
    echo ""
    echo "=== Test: Multiple Rules Combined ==="
    
    local url="http://127.0.0.1:${MOCK_HTTP_PORT}/test-multi"
    local rule_config
    rule_config=$(replay_rule_config_from_fixture multiple_rules.txt)
    
    local response
    response=$(replay_request "$url" "GET" '[["Accept", "application/json"]]' "" "$rule_config")
    
    local status
    status=$(printf '%s' "$response" | jq -r '.data.status // empty')
    
    local header1
    header1=$(printf '%s' "$response" | jq -r '.data.body | fromjson | .request.headers["x-multi-1"] // empty')
    
    local header2
    header2=$(printf '%s' "$response" | jq -r '.data.body | fromjson | .request.headers["x-multi-2"] // empty')
    
    local ua
    ua=$(printf '%s' "$response" | jq -r '.data.body | fromjson | .request.headers["user-agent"] // empty')
    
    local applied_count
    applied_count=$(printf '%s' "$response" | jq -r '.data.applied_rules | length // 0')
    
    if [ "$status" = "200" ] && [ "$header1" = "value1" ] && [ "$header2" = "value2" ] && [ "$ua" = "MultiTestUA/2.0" ]; then
        _log_pass "Multiple rules applied: 3 rules, all effective"
        ((passed++))
    else
        _log_fail "Multiple rules not fully applied" "X-Multi-1=value1, X-Multi-2=value2, UA=MultiTestUA/2.0" "h1=$header1, h2=$header2, ua=$ua"
        ((failed++))
    fi
    
    if [ "$applied_count" = "3" ]; then
        _log_pass "Applied rules count correct: $applied_count"
        ((passed++))
    else
        _log_fail "Applied rules count incorrect" "3" "$applied_count"
        ((failed++))
    fi
}

test_no_rules_mode() {
    echo ""
    echo "=== Test: No Rules Mode ==="
    
    local url="http://127.0.0.1:${MOCK_HTTP_PORT}/test-no-rules"
    local rule_config='{"mode":"none"}'
    
    local response
    response=$(replay_request "$url" "GET" '[["Accept", "application/json"]]' "" "$rule_config")
    
    local status
    status=$(printf '%s' "$response" | jq -r '.data.status // empty')
    
    local applied_count
    applied_count=$(printf '%s' "$response" | jq -r '.data.applied_rules | length // 0')
    
    if [ "$status" = "200" ] && [ "$applied_count" = "0" ]; then
        _log_pass "No rules mode: request sent without rules"
        ((passed++))
    else
        _log_fail "No rules mode failed" "applied_rules=0" "applied_rules=$applied_count"
        ((failed++))
    fi
}

test_sse_replay_with_rules() {
    echo ""
    echo "=== Test: SSE Replay with Rules ==="

    # 让 SSE 流持续 > 10s，用于验证 replay 的 timeout_ms 不会错误断开长连接
    local upstream_url="http://127.0.0.1:${MOCK_SSE_PORT}/sse/custom?count=30&interval=1"
    local payload
    payload=$(cat <<EOF
{"url":"${upstream_url}","method":"GET","headers":[["Accept","text/event-stream"]],"rule_config":$(replay_rule_config_from_fixture sse_req_headers.txt),"timeout_ms":10000}
EOF
)

    local out_file="/tmp/bifrost_replay_sse_${PROXY_PORT}_$$.log"
    local err_file="/tmp/bifrost_replay_sse_${PROXY_PORT}_$$.err"
    rm -f "$out_file" "$err_file" || true

    curl -sN -X POST "${ADMIN_BASE_URL}/api/replay/execute/unified" \
        -H "Content-Type: application/json" \
        -d "$payload" > "$out_file" 2>"$err_file" &
    local curl_pid=$!

    sleep 2
    if ! kill -0 "$curl_pid" 2>/dev/null; then
        _log_fail "SSE Replay: stream exited too early" ">=2s alive" "exited"
        ((failed++))
        return
    fi

    # 等待超过 10s，如果此时连接被错误断开（历史问题），curl 会提前退出
    sleep 11
    if ! kill -0 "$curl_pid" 2>/dev/null; then
        _log_fail "SSE Replay: stream was disconnected (timeout?)" ">=13s alive" "exited"
        echo "--- curl stderr ---" >&2
        tail -20 "$err_file" >&2 || true
        echo "--- curl stdout ---" >&2
        tail -20 "$out_file" >&2 || true
        ((failed++))
        return
    fi

    if grep -q '"type_":"connection"' "$out_file" && grep -q '"applied_rules":' "$out_file"; then
        _log_pass "SSE Replay: connection event received and stream kept alive >10s"
        ((passed++))
    else
        _log_fail "SSE Replay: missing connection/applied_rules" "connection + applied_rules" "not found"
        echo "--- curl stdout ---" >&2
        tail -40 "$out_file" >&2 || true
        ((failed++))
    fi

    kill "$curl_pid" 2>/dev/null || true
    wait "$curl_pid" 2>/dev/null || true
}

test_response_modification_rules() {
    echo ""
    echo "=== Test: Response Modification Rules ==="

    local url="http://127.0.0.1:${MOCK_HTTP_PORT}/test-res-mod?x=1"
    local rule_config
    rule_config=$(replay_rule_config_from_fixture response_modification.txt)

    local response
    response=$(replay_request "$url" "GET" '[ ["Accept", "application/json"] ]' "" "$rule_config")

    local status
    status=$(printf '%s' "$response" | jq -r '.data.status // empty')

    local header_val
    header_val=$(printf '%s' "$response" | jq -r '.data.headers[]? | select(.[0] | ascii_downcase == "x-replay-res") | .[1]' | head -1)

    local body
    body=$(printf '%s' "$response" | jq -r '.data.body // empty')

    if [ "$status" = "201" ] && [ "$header_val" = "ok" ] && [ "$body" = "replaced" ]; then
        _log_pass "Response rules applied: replaceStatus/resHeaders/resBody"
        ((passed++))
    else
        _log_fail "Response rules not applied" "status=201 & X-Replay-Res=ok & body=replaced" "status=$status header=$header_val body=$body"
        ((failed++))
    fi
}

test_websocket_replay_with_rules() {
    echo ""
    echo "=== Test: WebSocket Replay with Rules ==="

    local upstream_url="ws://127.0.0.1:${MOCK_WS_PORT}/ws"
    local rule_config
    rule_config=$(replay_rule_config_from_fixture websocket_req_headers.txt)

    local encoded_url encoded_rule
    encoded_url=$(urlencode "$upstream_url")
    encoded_rule=$(urlencode "$rule_config")
    local ws_path="/_bifrost/api/replay/execute/ws?url=${encoded_url}&rule_config=${encoded_rule}"

    # 1) 仅握手：验证规则注入的 Header 能在上游回传的 connection_info 中出现
    if python3 "$SCRIPT_DIR/../test_utils/ws_stress_client.py" \
        --proxy-host "127.0.0.1" \
        --proxy-port "$PROXY_PORT" \
        --host-header "127.0.0.1:${PROXY_PORT}" \
        --path "$ws_path" \
        --timeout 15.0 \
        --messages 0 \
        --no-send \
        --expect-text-contains "X-WS-Rule" \
        --expect-text-count 1 \
        >/dev/null 2>&1; then
        _log_pass "WebSocket replay: upstream handshake headers include rule header"
        ((passed++))
    else
        _log_fail "WebSocket replay: missing handshake header from rules" "X-WS-Rule in connection_info" "not found"
        ((failed++))
    fi

    # 2) 发送消息：验证消息能通过 replay 转发并回显
    if python3 "$SCRIPT_DIR/../test_utils/ws_stress_client.py" \
        --proxy-host "127.0.0.1" \
        --proxy-port "$PROXY_PORT" \
        --host-header "127.0.0.1:${PROXY_PORT}" \
        --path "$ws_path" \
        --timeout 15.0 \
        --message "hello-websocket" \
        --messages 1 \
        --expect-text-contains "hello-websocket" \
        --expect-text-count 1 \
        >/dev/null 2>&1; then
        _log_pass "WebSocket replay: message proxied and echoed"
        ((passed++))
    else
        _log_fail "WebSocket replay: echo missing" "echo hello-websocket" "not found"
        ((failed++))
    fi
}

main() {
    echo "=========================================="
    echo "  Replay Rules E2E Tests"
    echo "=========================================="
    echo ""
    echo "Configuration:"
    echo "  PROXY_PORT: $PROXY_PORT"
    echo "  MOCK_HTTP_PORT: $MOCK_HTTP_PORT"
    echo "  MOCK_SSE_PORT: $MOCK_SSE_PORT"
    echo "  MOCK_WS_PORT: $MOCK_WS_PORT"
    echo ""
    
    start_mock_server
    start_sse_server
    start_ws_server
    start_bifrost
    
    sleep 2
    
    test_reqHeaders_rule
    test_host_rule
    test_method_rule
    test_ua_rule
    test_referer_rule
    test_urlParams_rule
    test_reqCookies_rule
    test_reqBody_rule
    test_delete_header_rule
    test_multiple_rules
    test_no_rules_mode
    test_sse_replay_with_rules
    test_response_modification_rules
    test_websocket_replay_with_rules
    
    echo ""
    echo "=========================================="
    echo "  Test Results"
    echo "=========================================="
    echo "  Passed: $passed"
    echo "  Failed: $failed"
    echo "=========================================="
    
    if [ "$failed" -gt 0 ]; then
        exit 1
    fi
    exit 0
}

main "$@"
