#!/bin/bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT


set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

source "$SCRIPT_DIR/../test_utils/assert.sh"
source "$SCRIPT_DIR/../test_utils/rule_fixture.sh"
source "$SCRIPT_DIR/../test_utils/process.sh"

PROXY_HOST="${PROXY_HOST:-127.0.0.1}"
PROXY_PORT="${PROXY_PORT:-$((18880 + ($$ % 500)))}"
ECHO_HTTP_PORT="${ECHO_HTTP_PORT:-$((13000 + ($$ % 500)))}"

BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/release/bifrost}"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi

TEST_DATA_DIR="$ROOT_DIR/.bifrost-e2e-line-block-filter-${PROXY_PORT}-$$"
RULES_FILE="$TEST_DATA_DIR/rules.txt"
RULES_TEMPLATE="$ROOT_DIR/e2e-tests/rules/regression/line_block_filter_effect.txt"
PROXY_PID=""

HTTP_STATUS=""
HTTP_HEADERS=""
HTTP_BODY=""

cleanup() {
    kill_bifrost_on_port "$PROXY_PORT"
    safe_cleanup_proxy "$PROXY_PID"
    wait "$PROXY_PID" 2>/dev/null || true
    MOCK_SERVERS=http HTTP_PORT="$ECHO_HTTP_PORT" \
        "$ROOT_DIR/e2e-tests/mock_servers/start_servers.sh" stop >/dev/null 2>&1 || true
    rm -rf "$TEST_DATA_DIR"
}

trap cleanup EXIT

log_section() {
    echo ""
    echo "============================================================"
    echo "$1"
    echo "============================================================"
}

assert_json_equals() {
    local jq_expr="$1"
    local expected="$2"
    local json="$3"
    local message="$4"
    local actual

    actual=$(echo "$json" | jq -r "$jq_expr")
    if [[ "$actual" == "$expected" ]]; then
        _log_pass "$message"
    else
        _log_fail "$message" "$expected" "$actual"
        return 1
    fi
}

assert_json_empty() {
    local jq_expr="$1"
    local json="$2"
    local message="$3"
    local actual

    actual=$(echo "$json" | jq -r "$jq_expr")
    if [[ -z "$actual" || "$actual" == "null" ]]; then
        _log_pass "$message"
    else
        _log_fail "$message" "empty" "$actual"
        return 1
    fi
}

perform_request() {
    local method="$1"
    local url="$2"
    shift 2 || true

    local headers_file
    local body_file
    headers_file=$(mktemp)
    body_file=$(mktemp)

    HTTP_STATUS=$(curl -sS -o "$body_file" -D "$headers_file" \
        --proxy "http://${PROXY_HOST}:${PROXY_PORT}" \
        --connect-timeout 5 \
        --max-time 15 \
        -X "$method" \
        "$@" \
        "$url" \
        -w '%{http_code}')
    HTTP_HEADERS=$(cat "$headers_file")
    HTTP_BODY=$(cat "$body_file")
    rm -f "$headers_file" "$body_file"
}

start_mock_server() {
    log_section "Starting mock server"
    MOCK_SERVERS=http HTTP_PORT="$ECHO_HTTP_PORT" \
        "$ROOT_DIR/e2e-tests/mock_servers/start_servers.sh" stop >/dev/null 2>&1 || true
    MOCK_SERVERS=http HTTP_PORT="$ECHO_HTTP_PORT" \
        "$ROOT_DIR/e2e-tests/mock_servers/start_servers.sh" start-bg >/dev/null

    local waited=0
    while [[ $waited -lt 20 ]]; do
        if curl -sf "http://127.0.0.1:${ECHO_HTTP_PORT}/health" >/dev/null 2>&1; then
            _log_pass "HTTP echo server is ready on ${ECHO_HTTP_PORT}"
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done

    echo "Mock HTTP echo server failed to start" >&2
    exit 1
}

build_bifrost() {
    log_section "Checking bifrost binary"
    if [[ -x "$BIFROST_BIN" ]]; then
        _log_pass "Using existing bifrost binary at $BIFROST_BIN"
        return 0
    fi

    cargo build --release --bin bifrost >/dev/null
    if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
        BIFROST_BIN="${BIFROST_BIN}.exe"
    fi
    [[ -x "$BIFROST_BIN" ]]
}

write_rules() {
    render_rule_fixture_to_file "$RULES_TEMPLATE" "$RULES_FILE" \
        "ECHO_HTTP_PORT=${ECHO_HTTP_PORT}"
}

start_proxy() {
    log_section "Starting proxy"
    export BIFROST_DATA_DIR="$TEST_DATA_DIR"

    "$BIFROST_BIN" --port "$PROXY_PORT" start \
        --skip-cert-check \
        --unsafe-ssl --no-system-proxy \
        --rules-file "$RULES_FILE" \
        >"$TEST_DATA_DIR/proxy.log" 2>&1 &
    PROXY_PID=$!

    local waited=0
    while [[ $waited -lt 30 ]]; do
        if curl -sf --proxy "http://${PROXY_HOST}:${PROXY_PORT}" "http://line-block-filter.local/healthz" >/dev/null 2>&1; then
            _log_pass "Proxy is ready on ${PROXY_PORT}"
            return 0
        fi
        if ! kill -0 "$PROXY_PID" 2>/dev/null; then
            tail -n 200 "$TEST_DATA_DIR/proxy.log" >&2 || true
            echo "Proxy exited unexpectedly" >&2
            exit 1
        fi
        sleep 1
        waited=$((waited + 1))
    done

    tail -n 200 "$TEST_DATA_DIR/proxy.log" >&2 || true
    echo "Timed out waiting for proxy" >&2
    exit 1
}

test_include_filters_apply_on_matching_request() {
    log_section "GET /api/users matches include filters"
    perform_request "GET" "http://line-block-filter.local/api/users?from=e2e"

    assert_status_2xx "$HTTP_STATUS" "matching request should succeed" || return 1
    assert_json_equals '.server.type' 'http_echo_server' "$HTTP_BODY" "request should reach mock echo server" || return 1
    assert_json_equals '.request.parsed_path' '/api/users' "$HTTP_BODY" "matching request should keep original path" || return 1
    assert_json_equals '.request.headers["x-line-block-request"]' 'matched' "$HTTP_BODY" "reqHeaders should apply when include filters match" || return 1
    assert_header_value "X-Line-Block-Response" "matched" "$HTTP_HEADERS" "resHeaders should apply when include filters match" || return 1
}

test_exclude_filter_blocks_modification() {
    log_section "GET /api/internal/users matches exclude filter"
    perform_request "GET" "http://line-block-filter.local/api/internal/users"

    assert_status_2xx "$HTTP_STATUS" "excluded request should still succeed via base forwarding rule" || return 1
    assert_json_equals '.server.type' 'http_echo_server' "$HTTP_BODY" "excluded request should still reach mock echo server" || return 1
    assert_json_empty '.request.headers["x-line-block-request"] // empty' "$HTTP_BODY" "excludeFilter should suppress reqHeaders modification" || return 1
    assert_header_not_exists "X-Line-Block-Response" "$HTTP_HEADERS" "excludeFilter should suppress resHeaders modification" || return 1
}

test_method_include_filter_blocks_post() {
    log_section "POST /api/users misses method include filter"
    perform_request "POST" "http://line-block-filter.local/api/users" \
        -H "Content-Type: application/json" \
        --data '{"message":"hello"}'

    assert_status_2xx "$HTTP_STATUS" "POST request should still succeed via base forwarding rule" || return 1
    assert_json_equals '.server.type' 'http_echo_server' "$HTTP_BODY" "POST request should still reach mock echo server" || return 1
    assert_json_equals '.request.method' 'POST' "$HTTP_BODY" "mock server should observe POST method" || return 1
    assert_json_empty '.request.headers["x-line-block-request"] // empty' "$HTTP_BODY" "method includeFilter should prevent reqHeaders modification" || return 1
    assert_header_not_exists "X-Line-Block-Response" "$HTTP_HEADERS" "method includeFilter should prevent resHeaders modification" || return 1
}

test_path_include_filter_blocks_non_api() {
    log_section "GET /home misses path include filter"
    perform_request "GET" "http://line-block-filter.local/home"

    assert_status_2xx "$HTTP_STATUS" "non-api request should still succeed via base forwarding rule" || return 1
    assert_json_equals '.server.type' 'http_echo_server' "$HTTP_BODY" "non-api request should still reach mock echo server" || return 1
    assert_json_equals '.request.parsed_path' '/home' "$HTTP_BODY" "non-api request should keep original path" || return 1
    assert_json_empty '.request.headers["x-line-block-request"] // empty' "$HTTP_BODY" "path includeFilter should prevent reqHeaders modification" || return 1
    assert_header_not_exists "X-Line-Block-Response" "$HTTP_HEADERS" "path includeFilter should prevent resHeaders modification" || return 1
}

main() {
    start_mock_server
    build_bifrost
    write_rules
    start_proxy

    test_include_filters_apply_on_matching_request
    test_exclude_filter_blocks_modification
    test_method_include_filter_blocks_post
    test_path_include_filter_blocks_non_api

    echo ""
    echo "Assertions: ${PASSED_ASSERTIONS}/${TOTAL_ASSERTIONS} passed"
    if [[ "${FAILED_ASSERTIONS}" -gt 0 ]]; then
        echo "Multiline rule filter E2E assertions failed: ${FAILED_ASSERTIONS}"
        exit 1
    fi

    echo "Multiline rule filter E2E checks passed."
}

main "$@"
