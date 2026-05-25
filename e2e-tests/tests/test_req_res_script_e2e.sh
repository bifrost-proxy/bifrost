#!/bin/bash
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
E2E_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
PROJECT_DIR="$(cd "$E2E_DIR/.." && pwd)"

source "$E2E_DIR/test_utils/assert.sh"
source "$E2E_DIR/test_utils/http_client.sh"
source "$E2E_DIR/test_utils/process.sh"

PROXY_HOST="${PROXY_HOST:-127.0.0.1}"
PROXY_PORT="${PROXY_PORT:-8080}"

export ADMIN_HOST="${ADMIN_HOST:-$PROXY_HOST}"
export ADMIN_PORT="${ADMIN_PORT:-$PROXY_PORT}"
export ADMIN_PATH_PREFIX="${ADMIN_PATH_PREFIX:-/_bifrost}"
source "$E2E_DIR/test_utils/admin_client.sh"
ECHO_HTTP_PORT="${ECHO_HTTP_PORT:-3000}"
TEST_ID="${TEST_ID:-req_res_script}"
export TEST_ID

TEST_DATA_DIR="$PROJECT_DIR/.bifrost-test-req-res-script"
PROXY_LOG_FILE="$TEST_DATA_DIR/proxy.log"
MOCK_LOG_FILE="$TEST_DATA_DIR/mock.log"
RULE_FIXTURE="$E2E_DIR/rules/request_modify/req_res_script.txt"
PROXY_PID=""

cleanup() {
    if [[ -n "$PROXY_PID" ]]; then
        safe_cleanup_proxy "$PROXY_PID"
    fi

    MOCK_SERVERS=http HTTP_PORT="$ECHO_HTTP_PORT" \
    "$E2E_DIR/mock_servers/start_servers.sh" stop 2>/dev/null || true

    kill_bifrost_on_port "$PROXY_PORT"
}

trap cleanup EXIT

assert_body_contains_ci() {
    local expected_substring=$1
    local body=$2
    local message=${3:-"Response body should contain '$expected_substring'"}
    local expected_lower
    local body_lower
    expected_lower=$(echo "$expected_substring" | tr '[:upper:]' '[:lower:]')
    body_lower=$(echo "$body" | tr '[:upper:]' '[:lower:]')

    if [[ "$body_lower" == *"$expected_lower"* ]]; then
        _log_pass "$message"
        return 0
    else
        _log_fail "$message" "Contains '$expected_substring'" "${body:0:200}..."
        return 1
    fi
}

skip_pass() {
    local message="$1"
    local reason="${2:-skipped}"
    echo "[SKIP-PASS] ${message}: ${reason}"
}

start_mock_servers() {
    mkdir -p "$TEST_DATA_DIR"

    HTTP_PORT="$ECHO_HTTP_PORT" "$E2E_DIR/mock_servers/start_servers.sh" start-http > "$MOCK_LOG_FILE" 2>&1 &

    local count=0
    while ! curl -s "http://127.0.0.1:${ECHO_HTTP_PORT}/health" >/dev/null 2>&1; do
        count=$((count + 1))
        if [[ $count -ge 30 ]]; then
            cat "$MOCK_LOG_FILE"
            exit 1
        fi
        sleep 1
    done
}

write_scripts() {
    mkdir -p "$TEST_DATA_DIR/scripts/request"
    mkdir -p "$TEST_DATA_DIR/scripts/response"
    mkdir -p "$TEST_DATA_DIR/scripts/decode"

    cat > "$TEST_DATA_DIR/scripts/request/req_script.js" <<'EOF'
request.headers["X-ReqScript"] = "enabled";
request.headers["X-ReqScript-Protocol"] = request.protocol;

// 默认用固定 body，保证基础用例稳定；skip-decode 用例使用 /echo-skip 保留原始大 body。
if (request.method === "POST" && request.path.indexOf("/echo-skip") < 0) {
  request.body = "body-from-reqscript";
}
EOF

    cat > "$TEST_DATA_DIR/scripts/response/res_script.js" <<'EOF'
response.headers["X-ResScript"] = "enabled";
if (response.request.path.indexOf("/anything/http-repro") >= 0 || response.request.path.indexOf("/anything/https-repro") >= 0) {
  response.status = 218;
  response.statusText = "This is fine";
  response.headers["x-repro-script"] = "ran";
  response.headers["content-type"] = "application/json;charset=utf-8";
  delete response.headers["content-length"];
  delete response.headers["Content-Length"];
  response.body = JSON.stringify({
    via: "resScript",
    url: response.request.url || "",
    path: response.request.path || "",
    statusWas: 218
  });
  log.info("repro resScript applied");
}
if (response.request.path.indexOf("/res-body") >= 0 && response.body) {
  response.body = response.body + "::res-script";
}
EOF

    cat > "$TEST_DATA_DIR/scripts/decode/decode_script.js" <<'EOF'
log.info("decode phase", ctx.phase);

if (ctx.phase === "request") {
  ctx.output = {
    code: "0",
    data: "decoded-req::" + (request.body || ""),
    msg: "",
  };
} else {
  ctx.output = {
    code: "0",
    data:
      "decoded-res::" + (response.body || "") + "::req-path::" + response.request.path,
    msg: "",
  };
}
EOF
}

start_proxy() {
    mkdir -p "$TEST_DATA_DIR"

    local rules_file="$TEST_DATA_DIR/req_res_script.txt"
    sed "s/__ECHO_HTTP_PORT__/${ECHO_HTTP_PORT}/g" "$RULE_FIXTURE" > "$rules_file"

    local bifrost_bin="$PROJECT_DIR/target/release/bifrost"
    if [[ ! -x "$bifrost_bin" ]]; then
        exit 1
    fi

    BIFROST_DATA_DIR="$TEST_DATA_DIR" \
    "$bifrost_bin" \
        -p "$PROXY_PORT" \
        start \
        --unsafe-ssl \
        --skip-cert-check \
        --no-system-proxy \
        --intercept-include "httpbin.org" \
        --rules-file "$rules_file" \
        > "$PROXY_LOG_FILE" 2>&1 &

    PROXY_PID=$!

    local count=0
    while ! nc -z "$PROXY_HOST" "$PROXY_PORT" 2>/dev/null; do
        count=$((count + 1))
        if [[ $count -ge 90 ]]; then
            echo "ERROR: Proxy port $PROXY_PORT not available after 90s" >&2
            cat "$PROXY_LOG_FILE"
            exit 1
        fi
        sleep 1
    done

    if ! wait_for_admin 60; then
        echo "ERROR: Admin API not ready after 60s" >&2
        cat "$PROXY_LOG_FILE"
        exit 1
    fi
}

wait_for_traffic_id() {
    local url_pattern="$1"
    local timeout_seconds="${2:-30}"

    local waited=0
    while [[ $waited -lt $timeout_seconds ]]; do
        local id
        id=$(admin_get "/api/traffic?limit=50" | jq -r ".records[] | select((.url // .p // \"\") | contains(\"$url_pattern\")) | .id" | head -1)
        if [[ -n "$id" && "$id" != "null" ]]; then
            echo "$id"
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done

    echo "" 
    return 1
}

get_request_body_text() {
    local id="$1"
    admin_get "/api/traffic/${id}/request-body" | jq -r '.data // ""'
}

get_response_body_text() {
    local id="$1"
    admin_get "/api/traffic/${id}/response-body" | jq -r '.data // ""'
}

get_response_body_text_raw() {
    local id="$1"
    admin_get "/api/traffic/${id}/response-body?raw=1" | jq -r '.data // ""'
}

httpbin_https_request() {
    local url="$1"
    local headers_file
    local body_file
    headers_file="$(mktemp)"
    body_file="$(mktemp)"

    HTTP_STATUS=$(NO_PROXY="" no_proxy="" HTTP_PROXY="" http_proxy="" HTTPS_PROXY="" https_proxy="" \
        curl -s -k \
        --connect-timeout 10 \
        --max-time 20 \
        --proxy "http://${PROXY_HOST}:${PROXY_PORT}" \
        --noproxy "" \
        -D "$headers_file" \
        -o "$body_file" \
        -w '%{http_code}' \
        "$url" 2>/dev/null) || HTTP_STATUS="000"
    HTTP_HEADERS="$(tr -d '\r' < "$headers_file")"
    HTTP_BODY="$(cat "$body_file")"

    rm -f "$headers_file" "$body_file"
}

httpbin_reachable_via_proxy() {
    local status
    status=$(NO_PROXY="" no_proxy="" HTTP_PROXY="" http_proxy="" HTTPS_PROXY="" https_proxy="" \
        curl -s -k \
        --connect-timeout 10 \
        --max-time 20 \
        --proxy "http://${PROXY_HOST}:${PROXY_PORT}" \
        --noproxy "" \
        -o /dev/null \
        -w '%{http_code}' \
        "https://httpbin.org/get" 2>/dev/null || true)
    [[ "$status" =~ ^2 ]]
}

update_sandbox_limits() {
    local max_decode_input_bytes="$1"
    local max_decompress_output_bytes="$2"
    local payload
    payload=$(cat <<EOF
{"limits":{"max_decode_input_bytes":${max_decode_input_bytes},"max_decompress_output_bytes":${max_decompress_output_bytes}}}
EOF
)
    local resp
    resp=$(admin_put "/api/config/sandbox" "$payload")

    local got_decode
    local got_decompress
    got_decode=$(echo "$resp" | jq -r '.limits.max_decode_input_bytes // 0')
    got_decompress=$(echo "$resp" | jq -r '.limits.max_decompress_output_bytes // 0')
    assert_equals "${max_decode_input_bytes}" "$got_decode" "sandbox.max_decode_input_bytes 应更新成功"
    assert_equals "${max_decompress_output_bytes}" "$got_decompress" "sandbox.max_decompress_output_bytes 应更新成功"
}

test_decode_script_bodies() {
    sleep 2
    local id
    id=$(wait_for_traffic_id "/echo" 30)
    if [[ -z "$id" ]]; then
        _log_fail "decode should record traffic" "traffic id" "not found"
        return 1
    fi

    local req_body
    req_body=$(get_request_body_text "$id")
    assert_body_contains "decoded-req::" "$req_body" "decode should store decoded request body"
    assert_body_contains "decoded-req::body-from-reqscript" "$req_body" "decode should see final (post-reqScript) request body"

    local res_id
    res_id=$(wait_for_traffic_id "/res-body" 30)
    if [[ -z "$res_id" ]]; then
        _log_fail "decode should record response traffic" "traffic id" "not found"
        return 1
    fi

    local res_body
    res_body=$(get_response_body_text "$res_id")
    assert_body_contains "decoded-res::" "$res_body" "decode should store decoded response body"
    assert_body_contains "::req-path::/res-body" "$res_body" "decode response phase should carry request snapshot"
}

test_req_script() {
    local url="http://script-test.local/echo"
    http_post "$url" "origin-body"
    assert_status_2xx "$HTTP_STATUS" "reqScript should allow proxy request"

    assert_body_contains_ci "\\\"x-reqscript\\\": \\\"enabled\\\"" "$HTTP_BODY" "reqScript should inject request header"
    assert_body_contains_ci "\\\"x-reqscript-protocol\\\": \\\"http\\\"" "$HTTP_BODY" "reqScript should expose protocol"
    assert_body_contains "\\\"body\\\": \\\"body-from-reqscript\\\"" "$HTTP_BODY" "reqScript should update request body"

    assert_header_value "X-ResScript" "enabled" "$HTTP_HEADERS" "resScript should add response header"
}

test_max_decode_input_bytes_skip() {
    # 把 decode 输入上限调小，确保会触发跳过
    update_sandbox_limits 32 $((10 * 1024 * 1024))

    local marker="SKIP_DECODE_MARK"
    local url="http://script-test.local/echo-skip?case=skip_decode"
    http_post_large_body "$url" 256 "$marker"
    assert_status_2xx "$HTTP_STATUS" "skip-decode 请求应成功"

    sleep 2
    local id
    id=$(wait_for_traffic_id "/echo-skip" 30)
    assert_not_empty "$id" "应记录 skip-decode 的 traffic"

    local req_body
    req_body=$(get_request_body_text "$id")
    assert_body_contains "$marker" "$req_body" "decode 跳过时应仍保存原始请求体"
    assert_body_not_contains "decoded-req::" "$req_body" "decode 输入过大时应跳过 decode"

    local detail
    detail=$(admin_get "/api/traffic/${id}")
    local script_name
    script_name=$(echo "$detail" | jq -r '.decode_req_script_results[0].script_name // ""')
    assert_equals "__bifrost_skip__" "$script_name" "应记录 decode 跳过原因"
}

test_max_decompress_output_bytes_fallback() {
    # 把解压输出上限调小（1KiB），同时把 decode 输入上限恢复为默认值，避免误跳过 decode
    update_sandbox_limits $((2 * 1024 * 1024)) 1024

    local marker="DECOMP_LIMIT_MARK"
    local url="http://script-test.local/large-response?case=decompress_limit&size=4096&marker=${marker}&encoding=gzip"
    http_get "$url"
    assert_status_2xx "$HTTP_STATUS" "decompress-limit 请求应成功"

    sleep 2
    local id
    id=$(wait_for_traffic_id "/large-response" 30)
    assert_not_empty "$id" "应记录 decompress-limit 的 traffic"

    local res_body
    res_body=$(get_response_body_text "$id")
    assert_body_contains "decoded-res::" "$res_body" "decode 应仍然执行（即使解压回退）"
    assert_body_contains "::req-path::/large-response" "$res_body" "decode response phase 应携带请求快照"
    assert_body_not_contains "$marker" "$res_body" "解压输出超限时应回退到压缩数据（decode 看不到明文 marker）"

    local raw_res_body
    raw_res_body=$(get_response_body_text_raw "$id")
    assert_body_not_contains "$marker" "$raw_res_body" "raw=1 下也应是压缩数据（无明文 marker）"
}

test_res_script_body() {
    local url="http://script-test.local/res-body"
    http_get "$url"
    assert_status_2xx "$HTTP_STATUS" "resScript should allow proxy request"
    assert_body_contains "::res-script" "$HTTP_BODY" "resScript should append response body"
}

test_https_res_script_httpbin() {
    if ! httpbin_reachable_via_proxy; then
        skip_pass "HTTPS intercepted resScript httpbin regression" "httpbin.org unreachable through proxy"
        return 0
    fi

    local url="https://httpbin.org/anything/https-repro"
    httpbin_https_request "$url"
    assert_equals "218" "$HTTP_STATUS" "HTTPS resScript should override upstream status"
    assert_header_value "x-repro-script" "ran" "$HTTP_HEADERS" "HTTPS resScript should add response header"
    assert_body_contains 'resScript' "$HTTP_BODY" "HTTPS resScript should rewrite response body"
    assert_body_contains '/anything/https-repro' "$HTTP_BODY" "HTTPS resScript response body should include request path"

    sleep 2
    local id
    id=$(wait_for_traffic_id "/anything/https-repro" 30)
    assert_not_empty "$id" "HTTPS resScript traffic should be recorded"

    local detail
    detail=$(admin_get "/api/traffic/${id}")
    local recorded_status
    recorded_status=$(echo "$detail" | jq -r '.status // 0')
    assert_equals "218" "$recorded_status" "Traffic detail should record resScript-updated status"

    local script_name
    script_name=$(echo "$detail" | jq -r '.res_script_results[0].script_name // ""')
    assert_equals "res_script" "$script_name" "Traffic detail should include resScript execution result"

    local script_success
    script_success=$(echo "$detail" | jq -r '.res_script_results[0].success // false')
    assert_equals "true" "$script_success" "Traffic detail should mark HTTPS resScript as successful"
}

main() {
    start_mock_servers
    write_scripts
    start_proxy
    clear_traffic >/dev/null 2>&1 || true
    sleep 1
    test_req_script
    test_res_script_body
    test_https_res_script_httpbin
    test_decode_script_bodies
    test_max_decode_input_bytes_skip
    test_max_decompress_output_bytes_fallback

    echo "========================================"
    echo "Total:  $TOTAL_ASSERTIONS"
    echo "Passed: $PASSED_ASSERTIONS"
    echo "Failed: $FAILED_ASSERTIONS"
    echo "========================================"
    [ "$FAILED_ASSERTIONS" -eq 0 ]
}

main "$@"
