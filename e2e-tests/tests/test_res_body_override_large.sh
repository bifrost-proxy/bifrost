#!/bin/bash
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
E2E_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
PROJECT_DIR="$(cd "$E2E_DIR/.." && pwd)"

source "$E2E_DIR/test_utils/assert.sh"
source "$E2E_DIR/test_utils/http_client.sh"
source "$E2E_DIR/test_utils/process.sh"
source "$E2E_DIR/test_utils/rule_fixture.sh"

PROXY_HOST="${PROXY_HOST:-127.0.0.1}"
PROXY_PORT="${PROXY_PORT:-8080}"
ECHO_HTTP_PORT="${ECHO_HTTP_PORT:-3000}"
REQ_BODY_SIZE="${REQ_BODY_SIZE:-1024}"
RES_BODY_SIZE="${RES_BODY_SIZE:-2048}"
REQ_MARKER="REQ_MARKER"
RES_MARKER="RES_MARKER"

TEST_DATA_DIR="$PROJECT_DIR/.bifrost-test-res-body-large"
PROXY_LOG_FILE="$TEST_DATA_DIR/proxy.log"
MOCK_LOG_FILE="$TEST_DATA_DIR/mock.log"
PROXY_PID=""
TEST_ID=""

cleanup() {
    if [[ -n "$PROXY_PID" ]]; then
        safe_cleanup_proxy "$PROXY_PID"
    fi
    kill_bifrost_on_port "$PROXY_PORT"

    "$E2E_DIR/mock_servers/start_servers.sh" stop 2>/dev/null || true
}

trap cleanup EXIT

start_mock_servers() {
    mkdir -p "$TEST_DATA_DIR"

    "$E2E_DIR/mock_servers/start_servers.sh" stop >/dev/null 2>&1 || true
    "$E2E_DIR/mock_servers/start_servers.sh" start > "$MOCK_LOG_FILE" 2>&1 &

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

start_proxy() {
    mkdir -p "$TEST_DATA_DIR"
    cat > "$TEST_DATA_DIR/config.toml" <<EOF
[traffic]
max_body_buffer_size = 64
max_body_memory_size = 0
max_records = 2000
EOF

    local rules_template="$E2E_DIR/rules/advanced/body_size_strategy.txt"
    local rules_file="$TEST_DATA_DIR/body_size_strategy.txt"
    render_rule_fixture_to_file "$rules_template" "$rules_file" \
        "ECHO_HTTP_PORT=${ECHO_HTTP_PORT}"
    if [[ ! -f "$rules_file" ]]; then
        exit 1
    fi

    local bifrost_bin="$PROJECT_DIR/target/release/bifrost"
    if [[ ! -x "$bifrost_bin" && -f "${bifrost_bin}.exe" ]]; then
        bifrost_bin="${bifrost_bin}.exe"
    fi
    if [[ ! -f "$bifrost_bin" ]]; then
        exit 1
    fi

    BIFROST_DATA_DIR="$TEST_DATA_DIR" \
    "$bifrost_bin" \
        -p "$PROXY_PORT" \
        start \
        --unsafe-ssl \
        --rules-file "$rules_file" \
        > "$PROXY_LOG_FILE" 2>&1 &

    PROXY_PID=$!

    local count=0
    while true; do
        if curl -s --max-time 2 "http://${PROXY_HOST}:${PROXY_PORT}/_bifrost/api/system" >/dev/null 2>&1; then
            break
        fi
        count=$((count + 1))
        if [[ $count -ge 60 ]]; then
            cat "$PROXY_LOG_FILE"
            exit 1
        fi
        sleep 1
    done
}

assert_body_not_contains() {
    local expected=$1
    local actual=$2
    local message=$3

    if [[ "$actual" == *"$expected"* ]]; then
        _log_fail "$message" "Body without '$expected'" "Body contains '$expected'"
        return 1
    fi

    _log_pass "$message"
    return 0
}

build_large_json_body() {
    local pad_size=$1
    local pad
    pad=$(head -c "$pad_size" /dev/zero | tr '\0' 'X')
    echo "{\"marker\":\"REQ_JSON\",\"pad\":\"$pad\"}"
}

test_req_body_override_large() {
    local expected_body="REQ_BODY_OVERRIDE"
    local url="http://test-req-body-large.local/echo"

    http_post_large_body "$url" "$REQ_BODY_SIZE" "$REQ_MARKER" "Expect:"

    assert_status_2xx "$HTTP_STATUS" "reqBody should apply to large requests"

    local echoed_body
    echoed_body=$(get_json_field ".request.body")
    assert_body_equals "$expected_body" "$echoed_body" "Request body should be replaced"
}

test_req_prepend_skipped() {
    local url="http://test-req-prepend-large.local/echo"
    local original_body
    original_body=$(generate_large_body "$REQ_BODY_SIZE" "$REQ_MARKER")

    http_post_large_body "$url" "$REQ_BODY_SIZE" "$REQ_MARKER" "Expect:"

    assert_status_2xx "$HTTP_STATUS" "reqPrepend should not block large requests"
    local echoed_body
    echoed_body=$(get_json_field ".request.body")
    assert_body_equals "$original_body" "$echoed_body" "reqPrepend should be skipped for large body"
}

test_req_append_skipped() {
    local url="http://test-req-append-large.local/echo"
    local original_body
    original_body=$(generate_large_body "$REQ_BODY_SIZE" "$REQ_MARKER")

    http_post_large_body "$url" "$REQ_BODY_SIZE" "$REQ_MARKER" "Expect:"

    assert_status_2xx "$HTTP_STATUS" "reqAppend should not block large requests"
    local echoed_body
    echoed_body=$(get_json_field ".request.body")
    assert_body_equals "$original_body" "$echoed_body" "reqAppend should be skipped for large body"
}

test_req_replace_skipped() {
    local url="http://test-req-replace-large.local/echo"
    local original_body
    original_body=$(generate_large_body "$REQ_BODY_SIZE" "$REQ_MARKER")

    http_post_large_body "$url" "$REQ_BODY_SIZE" "$REQ_MARKER" "Expect:"

    assert_status_2xx "$HTTP_STATUS" "reqReplace should not block large requests"
    local echoed_body
    echoed_body=$(get_json_field ".request.body")
    assert_body_equals "$original_body" "$echoed_body" "reqReplace should be skipped for large body"
}

test_req_merge_skipped() {
    local url="http://test-req-merge-large.local/echo"
    local json_body
    json_body=$(build_large_json_body 512)

    http_post "$url" "$json_body" "Content-Type: application/json
Expect:"

    assert_status_2xx "$HTTP_STATUS" "reqMerge should not block large requests"
    local echoed_body
    echoed_body=$(get_json_field ".request.body")
    assert_body_equals "$json_body" "$echoed_body" "reqMerge should be skipped for large body"
}

test_res_body_override_large() {
    local expected_body="RES_BODY_OVERRIDE"
    local expected_len="${#expected_body}"
    local url="http://test-res-body-large.local/large-response?size=${RES_BODY_SIZE}&marker=${RES_MARKER}"

    http_get "$url"

    assert_status_2xx "$HTTP_STATUS" "resBody should apply to large responses"
    assert_body_equals "$expected_body" "$HTTP_BODY" "Response body should be replaced"
    assert_header_value "Content-Length" "$expected_len" "$HTTP_HEADERS" "Content-Length should match overridden body"
}

test_res_prepend_skipped() {
    local url="http://test-res-prepend-large.local/large-response?size=${RES_BODY_SIZE}&marker=${RES_MARKER}"

    http_get "$url"

    assert_status_2xx "$HTTP_STATUS" "resPrepend should not block large responses"
    assert_body_contains "$RES_MARKER" "$HTTP_BODY" "Original response marker should remain"
    assert_body_not_contains "RES_PREPEND_" "$HTTP_BODY" "resPrepend should be skipped for large body"
}

test_res_append_skipped() {
    local url="http://test-res-append-large.local/large-response?size=${RES_BODY_SIZE}&marker=${RES_MARKER}"

    http_get "$url"

    assert_status_2xx "$HTTP_STATUS" "resAppend should not block large responses"
    assert_body_contains "$RES_MARKER" "$HTTP_BODY" "Original response marker should remain"
    assert_body_not_contains "_RES_APPEND" "$HTTP_BODY" "resAppend should be skipped for large body"
}

test_res_replace_skipped() {
    local url="http://test-res-replace-large.local/large-response?size=${RES_BODY_SIZE}&marker=${RES_MARKER}"

    http_get "$url"

    assert_status_2xx "$HTTP_STATUS" "resReplace should not block large responses"
    assert_body_contains "$RES_MARKER" "$HTTP_BODY" "Original response marker should remain"
    assert_body_not_contains "RES_REPLACED" "$HTTP_BODY" "resReplace should be skipped for large body"
}

test_res_merge_skipped() {
    local url="http://test-res-merge-large.local/echo?pad=$(head -c 512 /dev/zero | tr '\0' 'A')"

    http_get "$url"

    assert_status_2xx "$HTTP_STATUS" "resMerge should not block large responses"
    assert_body_not_contains "res_merged" "$HTTP_BODY" "resMerge should be skipped for large body"
}

test_html_append_skipped() {
    local url="http://test-html-append-large.local/test.html"

    http_get "$url"

    assert_status_2xx "$HTTP_STATUS" "htmlAppend should not block large responses"
    assert_body_contains "Echo Response" "$HTTP_BODY" "HTML response should remain"
    assert_body_not_contains "HTML_APPEND" "$HTTP_BODY" "htmlAppend should be skipped for large body"
}

test_html_prepend_skipped() {
    local url="http://test-html-prepend-large.local/test.html"

    http_get "$url"

    assert_status_2xx "$HTTP_STATUS" "htmlPrepend should not block large responses"
    assert_body_contains "Echo Response" "$HTTP_BODY" "HTML response should remain"
    assert_body_not_contains "HTML_PREPEND" "$HTTP_BODY" "htmlPrepend should be skipped for large body"
}

test_html_body_skipped() {
    local url="http://test-html-body-large.local/test.html"

    http_get "$url"

    assert_status_2xx "$HTTP_STATUS" "htmlBody should not block large responses"
    assert_body_contains "Echo Response" "$HTTP_BODY" "HTML response should remain"
    assert_body_not_contains "HTML_BODY_OVERRIDE" "$HTTP_BODY" "htmlBody should be skipped for large body"
}

test_js_append_skipped() {
    local url="http://test-js-append-large.local/test.js"

    http_get "$url"

    assert_status_2xx "$HTTP_STATUS" "jsAppend should not block large responses"
    assert_body_contains "Echo Server Response" "$HTTP_BODY" "JS response should remain"
    assert_body_not_contains "JS_APPEND" "$HTTP_BODY" "jsAppend should be skipped for large body"
}

test_js_prepend_skipped() {
    local url="http://test-js-prepend-large.local/test.js"

    http_get "$url"

    assert_status_2xx "$HTTP_STATUS" "jsPrepend should not block large responses"
    assert_body_contains "Echo Server Response" "$HTTP_BODY" "JS response should remain"
    assert_body_not_contains "JS_PREPEND" "$HTTP_BODY" "jsPrepend should be skipped for large body"
}

test_js_body_skipped() {
    local url="http://test-js-body-large.local/test.js"

    http_get "$url"

    assert_status_2xx "$HTTP_STATUS" "jsBody should not block large responses"
    assert_body_contains "Echo Server Response" "$HTTP_BODY" "JS response should remain"
    assert_body_not_contains "JS_BODY_OVERRIDE" "$HTTP_BODY" "jsBody should be skipped for large body"
}

test_css_append_skipped() {
    local url="http://test-css-append-large.local/test.css"

    http_get "$url"

    assert_status_2xx "$HTTP_STATUS" "cssAppend should not block large responses"
    assert_body_contains "Echo Response" "$HTTP_BODY" "CSS response should remain"
    assert_body_not_contains "CSS_APPEND" "$HTTP_BODY" "cssAppend should be skipped for large body"
}

test_css_prepend_skipped() {
    local url="http://test-css-prepend-large.local/test.css"

    http_get "$url"

    assert_status_2xx "$HTTP_STATUS" "cssPrepend should not block large responses"
    assert_body_contains "Echo Response" "$HTTP_BODY" "CSS response should remain"
    assert_body_not_contains "CSS_PREPEND" "$HTTP_BODY" "cssPrepend should be skipped for large body"
}

test_css_body_skipped() {
    local url="http://test-css-body-large.local/test.css"

    http_get "$url"

    assert_status_2xx "$HTTP_STATUS" "cssBody should not block large responses"
    assert_body_contains "Echo Response" "$HTTP_BODY" "CSS response should remain"
    assert_body_not_contains "CSS_BODY_OVERRIDE" "$HTTP_BODY" "cssBody should be skipped for large body"
}

main() {
    start_mock_servers
    start_proxy
    test_req_body_override_large
    test_req_prepend_skipped
    test_req_append_skipped
    test_req_replace_skipped
    test_req_merge_skipped
    test_res_body_override_large
    test_res_prepend_skipped
    test_res_append_skipped
    test_res_replace_skipped
    test_res_merge_skipped
    test_html_append_skipped
    test_html_prepend_skipped
    test_html_body_skipped
    test_js_append_skipped
    test_js_prepend_skipped
    test_js_body_skipped
    test_css_append_skipped
    test_css_prepend_skipped
    test_css_body_skipped

    echo "========================================"
    echo "Total:  $TOTAL_ASSERTIONS"
    echo "Passed: $PASSED_ASSERTIONS"
    echo "Failed: $FAILED_ASSERTIONS"
    echo "========================================"
    [ "$FAILED_ASSERTIONS" -eq 0 ]
}

main "$@"
