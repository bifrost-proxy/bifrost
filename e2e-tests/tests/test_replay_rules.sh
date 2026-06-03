#!/bin/bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

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
MOCK_LOG_DIR="${BIFROST_DATA_DIR:-/tmp}/mock-logs"
mkdir -p "$MOCK_LOG_DIR"

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

create_replay_script() {
    local script_type="$1"
    local script_name="$2"
    local script_content="$3"

    local payload
    payload=$(jq -n --arg content "$script_content" '{content:$content}')
    local response
    response=$(curl -sS -X PUT "${ADMIN_BASE_URL}/api/scripts/${script_type}/${script_name}" \
        -H "Content-Type: application/json" \
        -d "$payload")

    local saved_name
    saved_name=$(printf '%s' "$response" | jq -r '.name // empty')
    if [ "$saved_name" != "$script_name" ]; then
        echo "Failed to save ${script_type} script ${script_name}: ${response}" >&2
        return 1
    fi
}

setup_replay_scripts() {
    create_replay_script "request" "replay_req_script" '
request.headers["X-Replay-ReqScript"] = "applied";
request.headers["X-Replay-ReqScript-Path"] = request.path;
request.headers["X-Replay-ReqScript-Value"] = ctx.values.replayScriptMarker || "";
if (request.body) {
  request.body = request.body + "::req-script";
}
' || exit 1

    create_replay_script "response" "replay_res_script" '
response.headers["X-Replay-ResScript"] = "applied";
response.status = 209;
if (response.body) {
  response.body = response.body + "::res-script";
}
' || exit 1

    create_replay_script "parser" "replay_bp_parser" '
ctx.output = {
  code: "0",
  data: JSON.stringify({
    replayBp: true,
    phase: ctx.phase,
    path: (response && response.request && response.request.path) || (request && request.path) || "",
    body: (response && response.body) || (request && request.body) || ""
  }),
  msg: ""
};
' || exit 1
}

trap cleanup EXIT

start_mock_server() {
    echo "Starting Mock HTTP Echo Server on port $MOCK_HTTP_PORT..."
    python3 "$SCRIPT_DIR/../mock_servers/http_echo_server.py" "$MOCK_HTTP_PORT" > "$MOCK_LOG_DIR/http_echo.log" 2>&1 &
    MOCK_HTTP_PID=$!
    
    local timeout=20
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
        echo "--- Mock HTTP server log ---"
        cat "$MOCK_LOG_DIR/http_echo.log" 2>/dev/null || echo "(empty)"
        exit 1
    fi
    echo "  Mock HTTP server started (PID: $MOCK_HTTP_PID)"
}

start_sse_server() {
    echo "Starting Mock SSE Echo Server on port $MOCK_SSE_PORT..."
    python3 "$SCRIPT_DIR/../mock_servers/sse_echo_server.py" --port "$MOCK_SSE_PORT" > "$MOCK_LOG_DIR/sse_echo.log" 2>&1 &
    MOCK_SSE_PID=$!

    local timeout=20
    local waited=0
    while [ $waited -lt $timeout ]; do
        if ! kill -0 "$MOCK_SSE_PID" 2>/dev/null; then
            echo "Failed to start Mock SSE server (process exited early)"
            echo "--- Mock SSE server log ---"
            cat "$MOCK_LOG_DIR/sse_echo.log" 2>/dev/null || echo "(empty)"
            exit 1
        fi
        if curl -s "http://127.0.0.1:${MOCK_SSE_PORT}/health" >/dev/null 2>&1; then
            echo "  Mock SSE server started (PID: $MOCK_SSE_PID)"
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done

    echo "Failed to start Mock SSE server (health check timeout after ${timeout}s)"
    echo "--- Mock SSE server log ---"
    cat "$MOCK_LOG_DIR/sse_echo.log" 2>/dev/null || echo "(empty)"
    kill_pid "$MOCK_SSE_PID" 2>/dev/null || true
    exit 1
}

start_ws_server() {
    echo "Starting Mock HTTP+WS Echo Server on port $MOCK_WS_PORT..."
    python3 "$SCRIPT_DIR/../mock_servers/http_ws_echo_server.py" "$MOCK_WS_PORT" > "$MOCK_LOG_DIR/ws_echo.log" 2>&1 &
    MOCK_WS_PID=$!

    local timeout=20
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
        echo "--- Mock WS server log ---"
        cat "$MOCK_LOG_DIR/ws_echo.log" 2>/dev/null || echo "(empty)"
        exit 1
    fi
    echo "  Mock WS server started (PID: $MOCK_WS_PID)"
}

start_bifrost() {
    echo "Starting Bifrost proxy on port $PROXY_PORT..."
    cd "$ROOT_DIR"

    BIFROST_DATA_DIR="${BIFROST_DATA_DIR:-./.bifrost-e2e-test}" \
        "$BIFROST_BIN" start -p "$PROXY_PORT" --unsafe-ssl --skip-cert-check --no-system-proxy > /tmp/bifrost_e2e.log 2>&1 &
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
    
    local request_json
    if [ -n "$body" ]; then
        request_json=$(jq -n \
            --arg url "$url" \
            --arg method "$method" \
            --argjson headers "$headers_json" \
            --arg body "$body" \
            --argjson rule_config "$rule_config" \
            --argjson timeout "$timeout" \
            '{url:$url,method:$method,headers:$headers,body:$body,rule_config:$rule_config,timeout_ms:$timeout}')
    else
        request_json=$(jq -n \
            --arg url "$url" \
            --arg method "$method" \
            --argjson headers "$headers_json" \
            --argjson rule_config "$rule_config" \
            --argjson timeout "$timeout" \
            '{url:$url,method:$method,headers:$headers,rule_config:$rule_config,timeout_ms:$timeout}')
    fi
    
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
        passed=$((passed + 1))
    else
        _log_fail "reqHeaders rule not applied" "custom-value-123" "$received_header"
        failed=$((failed + 1))
    fi
    
    local applied_rules
    applied_rules=$(printf '%s' "$response" | jq -r '.data.applied_rules | length // 0')
    if [ "$applied_rules" -gt 0 ]; then
        _log_pass "applied_rules returned: count=$applied_rules"
        passed=$((passed + 1))
    else
        _log_fail "applied_rules not returned" ">0" "$applied_rules"
        failed=$((failed + 1))
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
        passed=$((passed + 1))
    else
        _log_fail "Host rule not applied" "port=$MOCK_HTTP_PORT" "status=$status, port=$server_port"
        failed=$((failed + 1))
    fi
}

test_path_prefix_forward_rule() {
    echo ""
    echo "=== Test: Path Prefix Forward Rule ==="

    local asset_path="/labor_cost/static/07c1d7e1fb3e13436b958af5f90ec9c8.svg"
    local url="https://internal.example.test${asset_path}?v=1"
    local rule_config
    rule_config=$(replay_rule_config_from_fixture path_prefix_forward.txt "MOCK_HTTP_PORT=${MOCK_HTTP_PORT}")

    local response
    response=$(replay_request "$url" "GET" '[["Accept", "application/json"]]' "" "$rule_config")

    local status
    status=$(printf '%s' "$response" | jq -r '.data.status // empty')

    local received_path
    received_path=$(printf '%s' "$response" | jq -r '.data.body | fromjson | .request.parsed_path // empty')

    local received_query
    received_query=$(printf '%s' "$response" | jq -r '.data.body | fromjson | .request.query_string // empty')

    if [ "$status" = "200" ] && [ "$received_path" = "$asset_path" ] && [ "$received_query" = "v=1" ]; then
        _log_pass "Path prefix forward rule applied without duplicated path"
        passed=$((passed + 1))
    else
        _log_fail "Path prefix forward rule duplicated or lost path/query" "path=$asset_path query=v=1" "status=$status, path=$received_path, query=$received_query"
        failed=$((failed + 1))
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
        passed=$((passed + 1))
    else
        _log_fail "Method rule not applied" "POST" "$received_method"
        failed=$((failed + 1))
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
        passed=$((passed + 1))
    else
        _log_fail "UA rule not applied" "CustomUA/1.0-test" "$received_ua"
        failed=$((failed + 1))
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
        passed=$((passed + 1))
    else
        _log_fail "Referer rule not applied" "https://example.com/page" "$received_referer"
        failed=$((failed + 1))
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
        passed=$((passed + 1))
    else
        _log_fail "urlParams rule not applied" "existing=value, added_param=new_value" "existing=$existing_param, added_param=$added_param"
        failed=$((failed + 1))
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
        passed=$((passed + 1))
    else
        _log_fail "reqCookies rule not applied" "abc123" "$received_cookie"
        failed=$((failed + 1))
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
        passed=$((passed + 1))
    else
        _log_fail "reqBody rule not applied" "replaced_body_content" "$received_body"
        failed=$((failed + 1))
    fi
}

test_request_body_mutation_rules() {
    echo ""
    echo "=== Test: Request Body Mutation Rules ==="

    local url="http://127.0.0.1:${MOCK_HTTP_PORT}/test-body-mutations"
    local rule_config
    rule_config=$(replay_rule_config_from_fixture request_body_mutations.txt)

    local response
    response=$(replay_request "$url" "POST" '[["Content-Type", "text/plain"]]' "hello" "$rule_config")

    local status
    status=$(printf '%s' "$response" | jq -r '.data.status // empty')

    local received_body
    received_body=$(printf '%s' "$response" | jq -r '.data.body | fromjson | .request.body // empty')

    if [ "$status" = "200" ] && [ "$received_body" = "pre-world-post" ]; then
        _log_pass "Request body mutation rules applied: reqPrepend/reqAppend/reqReplace"
        passed=$((passed + 1))
    else
        _log_fail "Request body mutation rules not applied" "pre-world-post" "status=$status body=$received_body"
        failed=$((failed + 1))
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
        passed=$((passed + 1))
    else
        _log_fail "Delete header rule not applied" "null (removed)" "$removed_header"
        failed=$((failed + 1))
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
        passed=$((passed + 1))
    else
        _log_fail "Multiple rules not fully applied" "X-Multi-1=value1, X-Multi-2=value2, UA=MultiTestUA/2.0" "h1=$header1, h2=$header2, ua=$ua"
        failed=$((failed + 1))
    fi
    
    if [ "$applied_count" = "3" ]; then
        _log_pass "Applied rules count correct: $applied_count"
        passed=$((passed + 1))
    else
        _log_fail "Applied rules count incorrect" "3" "$applied_count"
        failed=$((failed + 1))
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
        passed=$((passed + 1))
    else
        _log_fail "No rules mode failed" "applied_rules=0" "applied_rules=$applied_count"
        failed=$((failed + 1))
    fi
}

test_sse_replay_with_rules() {
    echo ""
    echo "=== Test: SSE Replay with Rules ==="

    # 让 SSE 流持续超过 timeout_ms，用于验证 replay 的 timeout_ms 不会错误断开长连接。
    # CI runner 上曾观察到 10s 边界存在外部连接关闭噪声，因此用 5s 超时边界做更快、更稳定的回归。
    local upstream_url="http://127.0.0.1:${MOCK_SSE_PORT}/sse/custom?count=20&interval=0.5"
    local payload
    payload=$(cat <<EOF
{"url":"${upstream_url}","method":"GET","headers":[["Accept","text/event-stream"]],"rule_config":$(replay_rule_config_from_fixture sse_req_headers.txt),"timeout_ms":5000}
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
        failed=$((failed + 1))
        return
    fi

    # 等待超过 timeout_ms，如果此时连接被错误断开（历史问题），curl 会提前退出
    sleep 6
    if ! kill -0 "$curl_pid" 2>/dev/null; then
        if grep -Eq '"id":"(1[2-9]|[2-9][0-9]+)"' "$out_file"; then
            _log_pass "SSE Replay: received post-timeout event before client disconnect"
            passed=$((passed + 1))
            return
        fi

        _log_fail "SSE Replay: stream was disconnected (timeout?)" ">=8s alive" "exited"
        echo "--- curl stderr ---" >&2
        tail -20 "$err_file" >&2 || true
        echo "--- curl stdout ---" >&2
        tail -20 "$out_file" >&2 || true
        # 把 bifrost proxy 日志拷贝到 CI artifact 目录,方便诊断
        if [ -n "${BIFROST_E2E_REPORT_DIR:-}" ] && [ -f /tmp/bifrost_e2e.log ]; then
            cp /tmp/bifrost_e2e.log "${BIFROST_E2E_REPORT_DIR}/bifrost_replay_sse_fail.log" || true
        fi
        echo "--- bifrost log (last 80 lines mentioning UNIFIED_REPLAY or SSE) ---" >&2
        grep -E 'UNIFIED_REPLAY|\[SSE\]|process_sse_response' /tmp/bifrost_e2e.log 2>/dev/null | tail -80 >&2 || true
        failed=$((failed + 1))
        return
    fi

    if grep -q '"type_":"connection"' "$out_file" && grep -q '"applied_rules":' "$out_file" && grep -Eq '"id":"(1[2-9]|[2-9][0-9]+)"' "$out_file"; then
        _log_pass "SSE Replay: connection event received and stream kept alive beyond timeout_ms"
        passed=$((passed + 1))
    else
        _log_fail "SSE Replay: missing connection/applied_rules/post-timeout event" "connection + applied_rules + id>=12" "not found"
        echo "--- curl stdout ---" >&2
        tail -40 "$out_file" >&2 || true
        failed=$((failed + 1))
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
        passed=$((passed + 1))
    else
        _log_fail "Response rules not applied" "status=201 & X-Replay-Res=ok & body=replaced" "status=$status header=$header_val body=$body"
        failed=$((failed + 1))
    fi
}

test_replay_req_res_script_rules() {
    echo ""
    echo "=== Test: Replay Request/Response Script Rules ==="

    local url="http://127.0.0.1:${MOCK_HTTP_PORT}/replay-script"
    local rule_config
    rule_config=$(replay_rule_config_from_fixture req_res_script.txt)

    local response
    response=$(replay_request "$url" "POST" '[["Content-Type", "text/plain"]]' "origin-body" "$rule_config")

    local status
    status=$(printf '%s' "$response" | jq -r '.data.status // empty')

    local body
    body=$(printf '%s' "$response" | jq -r '.data.body // empty')

    local res_header
    res_header=$(printf '%s' "$response" | jq -r '.data.headers[]? | select(.[0] | ascii_downcase == "x-replay-resscript") | .[1]' | head -1)

    if [ "$status" = "209" ] && [[ "$body" == *"x-replay-reqscript"* ]] && [[ "$body" == *"applied"* ]] && [[ "$body" == *"replay-inline-value"* ]] && [[ "$body" == *"origin-body::req-script"* ]] && [ "$res_header" = "applied" ] && [[ "$body" == *"::res-script" ]]; then
        _log_pass "Replay reqScript/resScript rules modified upstream request and replay response"
        passed=$((passed + 1))
    else
        _log_fail "Replay reqScript/resScript rules not fully applied" "status=209 req header/body/ctx.values and res header/body modified" "status=$status res_header=$res_header body=${body:0:300}"
        failed=$((failed + 1))
    fi

    local traffic_id
    traffic_id=$(printf '%s' "$response" | jq -r '.data.traffic_id // empty')
    local detail
    detail=$(curl -sS "${ADMIN_BASE_URL}/api/traffic/${traffic_id}")

    local req_script_name res_script_name
    req_script_name=$(printf '%s' "$detail" | jq -r '.req_script_results[0].script_name // empty')
    res_script_name=$(printf '%s' "$detail" | jq -r '.res_script_results[0].script_name // empty')

    if [ "$req_script_name" = "replay_req_script" ] && [ "$res_script_name" = "replay_res_script" ]; then
        _log_pass "Replay traffic detail records reqScript/resScript execution results"
        passed=$((passed + 1))
    else
        _log_fail "Replay traffic detail missing script execution results" "req replay_req_script + res replay_res_script" "req=$req_script_name res=$res_script_name detail=$detail"
        failed=$((failed + 1))
    fi
}

test_replay_bp_decode_rules() {
    echo ""
    echo "=== Test: Replay BP Decode Rules ==="

    local url="http://127.0.0.1:${MOCK_HTTP_PORT}/replay-bp-decode"
    local rule_config
    rule_config=$(replay_rule_config_from_fixture bp_decode.txt)

    local response
    response=$(replay_request "$url" "POST" '[["Content-Type", "text/plain"]]' "bp-origin" "$rule_config")

    local status
    status=$(printf '%s' "$response" | jq -r '.data.status // empty')
    local traffic_id
    traffic_id=$(printf '%s' "$response" | jq -r '.data.traffic_id // empty')

    local detail
    detail=$(curl -sS "${ADMIN_BASE_URL}/api/traffic/${traffic_id}")

    local req_parser_success res_parser_success
    req_parser_success=$(printf '%s' "$detail" | jq -r '.decode_req_script_results[]? | select(.script_type == "parser" and .script_name == "replay_bp_parser") | .success' | head -1)
    res_parser_success=$(printf '%s' "$detail" | jq -r '.decode_res_script_results[]? | select(.script_type == "parser" and .script_name == "replay_bp_parser") | .success' | head -1)

    local req_body_response res_body_response req_body res_body
    req_body_response=$(curl -sS "${ADMIN_BASE_URL}/api/traffic/${traffic_id}/request-body")
    res_body_response=$(curl -sS "${ADMIN_BASE_URL}/api/traffic/${traffic_id}/response-body")
    req_body=$(printf '%s' "$req_body_response" | jq -r '.data // empty')
    res_body=$(printf '%s' "$res_body_response" | jq -r '.data // empty')

    if [ "$status" = "200" ] && [ "$req_parser_success" = "true" ] && [ "$res_parser_success" = "true" ] && [[ "$req_body" == *'"replayBp":true'* ]] && [[ "$req_body" == *'"phase":"request"'* ]] && [[ "$res_body" == *'"phase":"response"'* ]]; then
        _log_pass "Replay decode://bp applies parser results to replay traffic bodies"
        passed=$((passed + 1))
    else
        _log_fail "Replay decode://bp did not record decoded parser output" "parser success and decoded request/response bodies" "status=$status req_success=$req_parser_success res_success=$res_parser_success req_body=$req_body res_body=${res_body:0:200} detail=$detail"
        failed=$((failed + 1))
    fi
}

test_replay_full_modify_matrix_rules() {
    echo ""
    echo "=== Test: Replay Full Modify Matrix Rules ==="

    local url="http://127.0.0.1:${MOCK_HTTP_PORT}/replay-full?drop=1&old=1"
    local rule_config
    rule_config=$(replay_rule_config_from_fixture full_modify_matrix.txt)

    local response
    response=$(replay_request "$url" "POST" '[["Content-Type", "text/plain"], ["X-Remove", "1"], ["X-Trace", "old"]]' '{"a":1,"b":0}' "$rule_config")

    local status
    status=$(printf '%s' "$response" | jq -r '.data.status // empty')

    local body_json
    body_json=$(printf '%s' "$response" | jq -r '.data.body')

    local response_rule
    response_rule=$(printf '%s' "$body_json" | jq -r '.response_rule // empty')

    local path
    path=$(printf '%s' "$body_json" | jq -r '.request.path // empty')

    local keep_param
    keep_param=$(printf '%s' "$body_json" | jq -r '.request.query_params.keep[0] // empty')

    local drop_param
    drop_param=$(printf '%s' "$body_json" | jq -r '.request.query_params.drop[0] // "null"')

    local request_body
    request_body=$(printf '%s' "$body_json" | jq -c '.request.body | fromjson')

    replay_matrix_request_header() {
        local name="$1"
        printf '%s' "$body_json" | jq -r --arg name "$name" '.request.headers | to_entries[]? | select(.key | ascii_downcase == $name) | .value' | head -1
    }

    replay_matrix_response_header() {
        local name="$1"
        printf '%s' "$response" | jq -r --arg name "$name" '.data.headers[]? | select(.[0] | ascii_downcase == $name) | .[1]' | head -1
    }

    replay_matrix_protocol_present() {
        local name="$1"
        printf '%s' "$response" | jq -e --arg name "$name" '.data.applied_rules | any(.protocol == $name)' >/dev/null
    }

    if [ "$status" = "201" ] && [ "$response_rule" = "ok" ]; then
        _log_pass "Response status and resMerge applied in replay matrix"
        passed=$((passed + 1))
    else
        _log_fail "Response status/resMerge missing in replay matrix" "status=201 response_rule=ok" "status=$status response_rule=$response_rule"
        failed=$((failed + 1))
    fi

    if [ "$keep_param" = "2" ] && [ "$drop_param" = "null" ] && [[ "$path" == *"old=1"* ]]; then
        _log_pass "URL params applied: keep added, drop deleted, old preserved"
        passed=$((passed + 1))
    else
        _log_fail "URL params not applied" "keep=2 drop removed old=1" "path=$path keep=$keep_param drop=$drop_param"
        failed=$((failed + 1))
    fi

    if [ "$request_body" = '{"a":1,"b":2,"c":3}' ]; then
        _log_pass "reqMerge applied to JSON request body"
        passed=$((passed + 1))
    else
        _log_fail "reqMerge not applied to JSON request body" '{"a":1,"b":2,"c":3}' "$request_body"
        failed=$((failed + 1))
    fi

    local x_trace content_type origin cors_method cors_headers forwarded_for cookie
    x_trace=$(replay_matrix_request_header "x-trace")
    content_type=$(replay_matrix_request_header "content-type")
    origin=$(replay_matrix_request_header "origin")
    cors_method=$(replay_matrix_request_header "access-control-request-method")
    cors_headers=$(replay_matrix_request_header "access-control-request-headers")
    forwarded_for=$(replay_matrix_request_header "x-forwarded-for")
    cookie=$(replay_matrix_request_header "cookie")

    if [ "$x_trace" = "new" ] \
        && [ "$content_type" = "application/json; charset=utf-8" ] \
        && [ "$origin" = "https://app.example.test" ] \
        && [ "$cors_method" = "POST" ] \
        && [ "$cors_headers" = "X-Test" ] \
        && [ "$forwarded_for" = "1.2.3.4" ] \
        && [[ "$cookie" == *"sid=abc"* ]] \
        && [[ "$cookie" == *"theme=dark"* ]]; then
        _log_pass "Request header/cookie/CORS rules applied in replay matrix"
        passed=$((passed + 1))
    else
        _log_fail "Request header/cookie/CORS rules missing" "trace/content-type/cors/forwarded/cookie applied" "trace=$x_trace content_type=$content_type origin=$origin method=$cors_method headers=$cors_headers forwarded=$forwarded_for cookie=$cookie"
        failed=$((failed + 1))
    fi

    local x_response set_cookie res_content_type cache_control disposition response_for allow_origin trailer
    x_response=$(replay_matrix_response_header "x-response")
    set_cookie=$(replay_matrix_response_header "set-cookie")
    res_content_type=$(replay_matrix_response_header "content-type")
    cache_control=$(replay_matrix_response_header "cache-control")
    disposition=$(replay_matrix_response_header "content-disposition")
    response_for=$(replay_matrix_response_header "x-bifrost-response-for")
    allow_origin=$(replay_matrix_response_header "access-control-allow-origin")
    trailer=$(replay_matrix_response_header "trailer")

    if [ "$x_response" = "new" ] \
        && [ "$set_cookie" = "rid=xyz" ] \
        && [ "$res_content_type" = "application/json; charset=utf-8" ] \
        && [ "$cache_control" = "max-age=60" ] \
        && [ "$disposition" = 'attachment; filename="report.json"' ] \
        && [ "$response_for" = "mobile" ] \
        && [ "$allow_origin" = "https://app.example.test" ] \
        && [ "$trailer" = "X-Trailer" ]; then
        _log_pass "Response header/cookie/CORS metadata rules applied in replay matrix"
        passed=$((passed + 1))
    else
        _log_fail "Response metadata rules missing" "response metadata applied" "x_response=$x_response set_cookie=$set_cookie content_type=$res_content_type cache=$cache_control disposition=$disposition response_for=$response_for allow_origin=$allow_origin trailer=$trailer"
        failed=$((failed + 1))
    fi

    local protocol
    for protocol in reqHeaders reqCookies delete urlParams reqType reqCharset forwardedFor headerReplace reqCors params statusCode resHeaders resCookies resType resCharset cache attachment responseFor resCors trailers htmlAppend resReplace resMerge; do
        if replay_matrix_protocol_present "$protocol"; then
            continue
        fi
        _log_fail "Applied rules missing protocol in replay matrix" "$protocol" "not present"
        failed=$((failed + 1))
        return
    done

    _log_pass "Applied rules include full replay modify matrix"
    passed=$((passed + 1))
}

test_forward_localhost_api_rule() {
    echo ""
    echo "=== Test: Replay HTTPS API Forwarded to Local HTTP ==="

    local url="https://bifrost.local/api/nextagent/v1/sessions?source=replay"
    local rule_config
    rule_config=$(replay_rule_config_from_fixture forward_localhost_api.txt "MOCK_HTTP_PORT=${MOCK_HTTP_PORT}")

    local response
    response=$(replay_request "$url" "POST" '[["Host", "bifrost.local"], ["Connection", "keep-alive"], ["Content-Type", "application/json"]]' "{}" "$rule_config")

    local status
    status=$(printf '%s' "$response" | jq -r '.data.status // empty')

    local parsed_path
    parsed_path=$(printf '%s' "$response" | jq -r '.data.body | fromjson | .request.parsed_path // empty')

    local host_header
    host_header=$(printf '%s' "$response" | jq -r '.data.body | fromjson | .request.headers | to_entries[]? | select(.key | ascii_downcase == "host") | .value' | head -1)

    local connection_header
    connection_header=$(printf '%s' "$response" | jq -r '.data.body | fromjson | .request.headers | to_entries[]? | select(.key | ascii_downcase == "connection") | .value' | head -1)

    if [ "$status" = "200" ] && [ "$parsed_path" = "/api/nextagent/v1/sessions" ]; then
        _log_pass "Replay forwarding: nextoncall API path reached local HTTP upstream"
        passed=$((passed + 1))
    else
        _log_fail "Replay forwarding failed" "status=200 parsed_path=/api/nextagent/v1/sessions" "status=$status parsed_path=$parsed_path response=$response"
        failed=$((failed + 1))
    fi

    if [ "$host_header" != "bifrost.local" ] && [ -n "$host_header" ]; then
        _log_pass "Replay forwarding regenerated Host header for local upstream: $host_header"
        passed=$((passed + 1))
    else
        _log_fail "Replay forwarding leaked stale Host header" "local upstream host" "$host_header"
        failed=$((failed + 1))
    fi

    if [ -z "$connection_header" ]; then
        _log_pass "Replay forwarding dropped hop-by-hop Connection header"
        passed=$((passed + 1))
    else
        _log_fail "Replay forwarding leaked Connection header" "empty" "$connection_header"
        failed=$((failed + 1))
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
        passed=$((passed + 1))
    else
        _log_fail "WebSocket replay: missing handshake header from rules" "X-WS-Rule in connection_info" "not found"
        failed=$((failed + 1))
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
        passed=$((passed + 1))
    else
        _log_fail "WebSocket replay: echo missing" "echo hello-websocket" "not found"
        failed=$((failed + 1))
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
    setup_replay_scripts
    
    sleep 2
    
    test_reqHeaders_rule
    test_host_rule
    test_path_prefix_forward_rule
    test_method_rule
    test_ua_rule
    test_referer_rule
    test_urlParams_rule
    test_reqCookies_rule
    test_reqBody_rule
    test_request_body_mutation_rules
    test_delete_header_rule
    test_multiple_rules
    test_no_rules_mode
    test_sse_replay_with_rules
    test_response_modification_rules
    test_replay_req_res_script_rules
    test_replay_bp_decode_rules
    test_replay_full_modify_matrix_rules
    test_forward_localhost_api_rule
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
