#!/bin/bash

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

source "$SCRIPT_DIR/../test_utils/assert.sh"
source "$SCRIPT_DIR/../test_utils/rule_fixture.sh"
source "$SCRIPT_DIR/../test_utils/process.sh"

PROXY_HOST="${PROXY_HOST:-127.0.0.1}"
PROXY_PORT="${PROXY_PORT:-$((18880 + ($$ % 500)))}"
ECHO_HTTP_PORT="${ECHO_HTTP_PORT:-$((13000 + ($$ % 500)))}"
ECHO_HTTPS_PORT="${ECHO_HTTPS_PORT:-$((13443 + ($$ % 500)))}"
ECHO_WS_PORT="${ECHO_WS_PORT:-$((13200 + ($$ % 500)))}"
ECHO_WSS_PORT="${ECHO_WSS_PORT:-$((13250 + ($$ % 500)))}"
ECHO_SSE_PORT="${ECHO_SSE_PORT:-$((13300 + ($$ % 500)))}"
ECHO_PROXY_PORT="${ECHO_PROXY_PORT:-$((13999 + ($$ % 200)))}"

BIFROST_BIN="$ROOT_DIR/target/release/bifrost"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi
TEST_DATA_DIR="$ROOT_DIR/.bifrost-e2e-rule-semantics-${PROXY_PORT}-$$"
RULES_FILE="$TEST_DATA_DIR/rules.txt"
RULES_TEMPLATE="$ROOT_DIR/e2e-tests/rules/regression/rule_semantics_split_parsing.txt"
PROXY_PID=""

HTTP_STATUS=""
HTTP_HEADERS=""
HTTP_BODY=""

cleanup() {
    kill_bifrost_on_port "$PROXY_PORT"
    safe_cleanup_proxy "$PROXY_PID"
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
    local url="$1"
    shift || true

    local headers_file
    local body_file
    headers_file=$(mktemp)
    body_file=$(mktemp)

    HTTP_STATUS=$(curl -sS -o "$body_file" -D "$headers_file" \
        --proxy "http://${PROXY_HOST}:${PROXY_PORT}" \
        --connect-timeout 5 \
        --max-time 15 \
        "$@" \
        "$url" \
        -w '%{http_code}')
    HTTP_HEADERS=$(cat "$headers_file")
    HTTP_BODY=$(cat "$body_file")
    rm -f "$headers_file" "$body_file"
}

start_mock_servers() {
    log_section "Starting mock servers"
    HTTP_PORT="$ECHO_HTTP_PORT" \
    HTTPS_PORT="$ECHO_HTTPS_PORT" \
    WS_PORT="$ECHO_WS_PORT" \
    WSS_PORT="$ECHO_WSS_PORT" \
    SSE_PORT="$ECHO_SSE_PORT" \
    PROXY_PORT="$ECHO_PROXY_PORT" \
        "$ROOT_DIR/e2e-tests/mock_servers/start_servers.sh" start-bg

    local waited=0
    while [[ $waited -lt 20 ]]; do
        if curl -sf "http://127.0.0.1:${ECHO_HTTP_PORT}/health" >/dev/null 2>&1; then
            _log_pass "HTTP echo server is ready on ${ECHO_HTTP_PORT}"
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done

    echo "Mock servers failed to start" >&2
    exit 1
}

build_bifrost() {
    log_section "Building bifrost"
    return 0
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
        --unsafe-ssl \
        --rules-file "$RULES_FILE" \
        >"$TEST_DATA_DIR/proxy.log" 2>&1 &
    PROXY_PID=$!

    local waited=0
    while [[ $waited -lt 30 ]]; do
        if curl -sf --proxy "http://${PROXY_HOST}:${PROXY_PORT}" "http://example.com/" >/dev/null 2>&1; then
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

test_url_rewrite_chain() {
    log_section "URL rewrite chain"
    perform_request "http://rewrite-chain.local/legacy/v1/users?keep=original&remove_me=yes"

    assert_status_2xx "$HTTP_STATUS" "rewrite chain request should succeed" || return 1
    assert_json_equals '.request.parsed_path' '/api/v99/users' "$HTTP_BODY" "pathReplace regex should rewrite path" || return 1
    assert_json_equals '.request.query_params.keep[0]' 'rewritten' "$HTTP_BODY" "urlParams should override keep" || return 1
    assert_json_empty '.request.query_params.remove_me[0] // empty' "$HTTP_BODY" "urlParams should delete remove_me" || return 1
}

test_full_url_pattern_with_spaced_header() {
    log_section "Full URL pattern + spaced header"
    perform_request "http://full-url-space.local/api/v1/users"

    assert_status_2xx "$HTTP_STATUS" "full URL spaced-header request should succeed" || return 1
    assert_json_equals '.request.headers["x-split-trace"]' 'note.example.com:8443 stays text' "$HTTP_BODY" "spaced header value should not be merged into host target" || return 1
}

test_full_url_pattern_with_value_ref() {
    log_section "Full URL pattern + value ref"
    perform_request "http://full-url-value.local/api/users"

    assert_status_2xx "$HTTP_STATUS" "full URL value-ref request should succeed" || return 1
    assert_json_equals '.request.headers["x-upstream"]' 'api.example.com:9443' "$HTTP_BODY" "value-ref header should still apply after host target" || return 1
}

test_full_url_pattern_with_regex_path_replace() {
    log_section "Full URL pattern + regex pathReplace"
    perform_request "http://full-url-regex-op.local/api/users"

    assert_status_2xx "$HTTP_STATUS" "full URL regex pathReplace request should succeed" || return 1
    assert_json_equals '.request.parsed_path' '/edge/users' "$HTTP_BODY" "regex pathReplace should remain active after full URL pattern" || return 1
}

test_protocol_first_regex_pattern() {
    log_section "Protocol-first regex pattern"
    perform_request "http://proto-regex.local/api/v2/users"

    assert_status_2xx "$HTTP_STATUS" "protocol-first regex request should succeed" || return 1
    assert_header_value "X-Regex-Split" "matched" "$HTTP_HEADERS" "regex pattern should stay split from protocol-first host target" || return 1
}

main() {
    start_mock_servers
    build_bifrost
    write_rules
    start_proxy

    test_url_rewrite_chain
    test_full_url_pattern_with_spaced_header
    test_full_url_pattern_with_value_ref
    test_full_url_pattern_with_regex_path_replace
    test_protocol_first_regex_pattern

    echo ""
    echo "Assertions: ${PASSED_ASSERTIONS}/${TOTAL_ASSERTIONS} passed"
    if [[ "${FAILED_ASSERTIONS}" -gt 0 ]]; then
        echo "Rule semantics regressions detected: ${FAILED_ASSERTIONS} assertion(s) failed."
        exit 1
    fi

    echo "All rule semantics regression tests passed."
}

main "$@"
