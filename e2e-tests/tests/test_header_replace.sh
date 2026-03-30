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
ECHO_HTTP_PORT="${ECHO_HTTP_PORT:-3000}"
TEST_ID="${TEST_ID:-header_replace}"
BIFROST_E2E_HTTP_RETRIES="${BIFROST_E2E_HTTP_RETRIES:-2}"
export TEST_ID BIFROST_E2E_HTTP_RETRIES

TEST_DATA_DIR="$PROJECT_DIR/.bifrost-test-header-replace"
PROXY_LOG_FILE="$TEST_DATA_DIR/proxy.log"
MOCK_LOG_FILE="$TEST_DATA_DIR/mock.log"
PROXY_PID=""

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
max_body_buffer_size = 10485760
max_body_memory_size = 0
max_records = 2000
EOF

    local rules_file="$TEST_DATA_DIR/header_replace_rules.txt"
    cat > "$rules_file" <<EOF
test-req-header-replace.local http://127.0.0.1:${ECHO_HTTP_PORT}/echo headerReplace://req.x-trace:abc=xyz
test-res-header-replace.local http://127.0.0.1:${ECHO_HTTP_PORT}/echo headerReplace://res.x-echo-server:bifrost-test=bifrost-custom
EOF

    local bifrost_bin="$PROJECT_DIR/target/release/bifrost"
    if [[ ! -x "$bifrost_bin" ]]; then
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
    while ! nc -z "$PROXY_HOST" "$PROXY_PORT" 2>/dev/null; do
        count=$((count + 1))
        if [[ $count -ge 60 ]]; then
            cat "$PROXY_LOG_FILE"
            exit 1
        fi
        sleep 1
    done
}

test_req_header_replace() {
    local url="http://test-req-header-replace.local/echo"
    http_get "$url" "X-Trace: abc123"
    assert_status_2xx "$HTTP_STATUS" "headerReplace should update request headers"
    local echoed_header
    echoed_header=$(get_json_field ".request.headers.\"X-Trace\"")
    assert_body_equals "xyz123" "$echoed_header" "Request header should be replaced"
}

test_res_header_replace() {
    local url="http://test-res-header-replace.local/echo"
    http_get "$url"
    assert_status_2xx "$HTTP_STATUS" "headerReplace should update response headers"
    assert_header_value "X-Echo-Server" "bifrost-custom" "$HTTP_HEADERS" "Response header should be replaced"
}

main() {
    start_mock_servers
    start_proxy
    sleep 1
    test_req_header_replace
    test_res_header_replace

    echo "========================================"
    echo "Total:  $TOTAL_ASSERTIONS"
    echo "Passed: $PASSED_ASSERTIONS"
    echo "Failed: $FAILED_ASSERTIONS"
    echo "========================================"
    [ "$FAILED_ASSERTIONS" -eq 0 ]
}

main "$@"
