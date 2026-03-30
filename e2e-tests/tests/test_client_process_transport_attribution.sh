#!/bin/bash

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

PROXY_HOST="${PROXY_HOST:-127.0.0.1}"
PROXY_PORT="${PROXY_PORT:-$((19180 + ($$ % 500)))}"
SOCKS5_PORT="${SOCKS5_PORT:-$((11180 + ($$ % 500)))}"
ADMIN_HOST="${ADMIN_HOST:-127.0.0.1}"
ADMIN_PORT="${ADMIN_PORT:-$PROXY_PORT}"
ECHO_HTTP_PORT="${ECHO_HTTP_PORT:-$((14000 + ($$ % 500)))}"
ECHO_HTTPS_PORT="${ECHO_HTTPS_PORT:-$((14443 + ($$ % 500)))}"
ECHO_WS_PORT="${ECHO_WS_PORT:-$((14200 + ($$ % 500)))}"
ECHO_WSS_PORT="${ECHO_WSS_PORT:-$((14250 + ($$ % 500)))}"
ECHO_SSE_PORT="${ECHO_SSE_PORT:-$((14300 + ($$ % 500)))}"
ECHO_PROXY_PORT="${ECHO_PROXY_PORT:-$((14999 + ($$ % 200)))}"

export ADMIN_HOST ADMIN_PORT ADMIN_PATH_PREFIX="/_bifrost"

source "$SCRIPT_DIR/../test_utils/assert.sh"
source "$SCRIPT_DIR/../test_utils/admin_client.sh"
source "$SCRIPT_DIR/../test_utils/rule_fixture.sh"
source "$SCRIPT_DIR/../test_utils/process.sh"
ADMIN_BASE_URL="http://${ADMIN_HOST}:${ADMIN_PORT}${ADMIN_PATH_PREFIX}"
export ADMIN_BASE_URL

BIFROST_BIN="$ROOT_DIR/target/release/bifrost"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi
CARGO_BIN="${CARGO_BIN:-$HOME/.cargo/bin/cargo}"
TEST_DATA_DIR="$ROOT_DIR/.bifrost-e2e-client-attribution-${PROXY_PORT}-$$"
RULES_FILE="$TEST_DATA_DIR/rules.txt"
RULES_TEMPLATE="$ROOT_DIR/e2e-tests/rules/runtime/client_process_transport_attribution.txt"
PROXY_PID=""

cleanup() {
    if [[ -n "$PROXY_PID" ]]; then
        safe_cleanup_proxy "$PROXY_PID"
    fi
    kill_bifrost_on_port "$PROXY_PORT"
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

assert_not_unknown_client() {
    local traffic_json="$1"
    local message="$2"
    local client_app

    client_app=$(echo "$traffic_json" | jq -r '.client_app // ""')
    if [[ -n "$client_app" && "$client_app" != "Unknown" && "$client_app" != "null" ]]; then
        _log_pass "$message (${client_app})"
    else
        _log_fail "$message" "non-empty client_app" "${client_app:-empty}"
        return 1
    fi
}

assert_positive_pid() {
    local traffic_json="$1"
    local message="$2"
    local client_pid

    client_pid=$(echo "$traffic_json" | jq -r '.client_pid // 0')
    if [[ "$client_pid" =~ ^[0-9]+$ ]] && [[ "$client_pid" -gt 0 ]]; then
        _log_pass "$message (${client_pid})"
    else
        _log_fail "$message" "positive pid" "$client_pid"
        return 1
    fi
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
    if [[ "${SKIP_BUILD:-false}" == "true" ]] && [[ -f "$BIFROST_BIN" ]]; then
        echo "[INFO] Skipping build (SKIP_BUILD=true), using existing binary: $BIFROST_BIN"
        return 0
    fi
    log_section "Building bifrost"
    (cd "$ROOT_DIR" && "$CARGO_BIN" build --release --bin bifrost) || {
        echo "Failed to build bifrost with $CARGO_BIN" >&2
        exit 1
    }
}

write_rules() {
    render_rule_fixture_to_file "$RULES_TEMPLATE" "$RULES_FILE" \
        "ECHO_HTTP_PORT=${ECHO_HTTP_PORT}" \
        "ECHO_HTTPS_PORT=${ECHO_HTTPS_PORT}" \
        "ECHO_WS_PORT=${ECHO_WS_PORT}"
}

start_proxy() {
    log_section "Starting proxy"
    export BIFROST_DATA_DIR="$TEST_DATA_DIR"

    "$BIFROST_BIN" --port "$PROXY_PORT" --socks5-port "$SOCKS5_PORT" start \
        --skip-cert-check \
        --unsafe-ssl \
        --intercept \
        --rules-file "$RULES_FILE" \
        >"$TEST_DATA_DIR/proxy.log" 2>&1 &
    PROXY_PID=$!

    local waited=0
    while [[ $waited -lt 30 ]]; do
        if curl -sf "http://${ADMIN_HOST}:${ADMIN_PORT}/_bifrost/api/system" >/dev/null 2>&1; then
            _log_pass "Proxy admin is ready on ${ADMIN_PORT}"
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

wait_for_traffic() {
    local pattern="$1"
    local timeout="${2:-20}"
    local waited=0

    while [[ $waited -lt $timeout ]]; do
        local traffic_id
        traffic_id=$(find_traffic_id_by_url "$ADMIN_HOST" "$ADMIN_PORT" "$pattern" 50)
        if [[ -n "$traffic_id" && "$traffic_id" != "null" ]]; then
            echo "$traffic_id"
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done

    return 1
}

run_curl_capture_with_retries() {
    local body_file="$1"
    local headers_file="$2"
    shift 2

    local attempts=0
    local max_attempts=3
    local status=""
    local curl_exit=0

    while [[ $attempts -lt $max_attempts ]]; do
        : > "$body_file"
        : > "$headers_file"
        curl_exit=0
        status=$(curl -sS -o "$body_file" -D "$headers_file" "$@" -w '%{http_code}') || curl_exit=$?
        if [[ $curl_exit -eq 0 && "$status" != "000" ]]; then
            printf '%s\n' "$status"
            return 0
        fi
        attempts=$((attempts + 1))
        if [[ $attempts -lt $max_attempts ]]; then
            sleep 1
        fi
    done

    printf '%s\n' "${status:-000}"
    return "$curl_exit"
}

test_http_proxy_attribution() {
    log_section "HTTP proxy client attribution"
    clear_traffic >/dev/null 2>&1 || true

    local body_file
    body_file=$(mktemp)
    local status
    status=$(curl -sS -o "$body_file" \
        --proxy "http://${PROXY_HOST}:${PROXY_PORT}" \
        --connect-timeout 5 \
        --max-time 15 \
        "http://http-attr.local/test" \
        -w '%{http_code}')
    local body
    body=$(cat "$body_file")
    rm -f "$body_file"

    assert_status_2xx "$status" "http proxy request should succeed" || return 1
    assert_json_equals '.server.protocol' 'http' "$body" "http request should reach HTTP echo server" || return 1

    local traffic_id
    traffic_id=$(wait_for_traffic "http-attr.local" 20) || return 1
    local traffic
    traffic=$(get_traffic_detail "$traffic_id")
    assert_not_unknown_client "$traffic" "http traffic should record client_app" || return 1
    assert_positive_pid "$traffic" "http traffic should record client_pid" || return 1
}

test_websocket_attribution() {
    log_section "WebSocket client attribution"
    clear_traffic >/dev/null 2>&1 || true

    python3 "$ROOT_DIR/e2e-tests/test_utils/ws_stress_client.py" \
        --proxy-host "$PROXY_HOST" \
        --proxy-port "$PROXY_PORT" \
        --host-header "ws-attr.local" \
        --path "/ws" \
        --messages 2 \
        --timeout 15.0 \
        >/dev/null

    local traffic_id
    traffic_id=$(wait_for_traffic "ws-attr.local" 20) || return 1
    local traffic
    traffic=$(get_traffic_detail "$traffic_id")

    assert_json_equals '.is_websocket' 'true' "$traffic" "websocket traffic should be marked as websocket" || return 1
    assert_not_unknown_client "$traffic" "websocket traffic should record client_app" || return 1
    assert_positive_pid "$traffic" "websocket traffic should record client_pid" || return 1
}

test_socks5_tls_attribution() {
    log_section "SOCKS5 + TLS intercept client attribution"
    clear_traffic >/dev/null 2>&1 || true

    local headers_file
    local body_file
    headers_file=$(mktemp)
    body_file=$(mktemp)

    local status
    status=$(run_curl_capture_with_retries "$body_file" "$headers_file" \
        --socks5-hostname "${PROXY_HOST}:${SOCKS5_PORT}" \
        --connect-timeout 5 \
        --max-time 20 \
        -k \
        "https://httpbin.org/headers")
    local body
    body=$(cat "$body_file")
    local headers
    headers=$(cat "$headers_file")
    rm -f "$headers_file" "$body_file"

    assert_status_2xx "$status" "socks5 TLS request should succeed" || return 1
    assert_header_value "X-Socks5-Rule" "applied" "$headers" "tls-intercept response rule should still apply over SOCKS5" || return 1

    local traffic_id
    traffic_id=$(wait_for_traffic "httpbin.org/headers" 20) || return 1
    local traffic
    traffic=$(get_traffic_detail "$traffic_id")

    assert_not_unknown_client "$traffic" "socks5 TLS traffic should record client_app" || return 1
    assert_positive_pid "$traffic" "socks5 TLS traffic should record client_pid" || return 1
    assert_json_equals '.is_websocket' 'false' "$traffic" "socks5 TLS traffic should not be marked as websocket" || return 1
}

test_https_tunnel_attribution() {
    log_section "HTTPS CONNECT tunnel client attribution"
    clear_traffic >/dev/null 2>&1 || true

    local body_file
    body_file=$(mktemp)
    local status
    status=$(curl -sS -o "$body_file" \
        --proxy "http://${PROXY_HOST}:${PROXY_PORT}" \
        --connect-timeout 5 \
        --max-time 20 \
        -k \
        "https://tunnel-attr.local/health" \
        -w '%{http_code}')
    rm -f "$body_file"

    assert_status_2xx "$status" "https CONNECT tunnel request should succeed" || return 1

    local traffic_id
    traffic_id=$(wait_for_traffic "tunnel-attr.local" 20) || return 1
    local traffic
    traffic=$(get_traffic_detail "$traffic_id")

    assert_json_equals '.is_tunnel' 'true' "$traffic" "https passthrough traffic should be marked as tunnel" || return 1
    assert_not_unknown_client "$traffic" "https tunnel traffic should record client_app" || return 1
    assert_positive_pid "$traffic" "https tunnel traffic should record client_pid" || return 1
}

main() {
    start_mock_servers
    build_bifrost
    write_rules
    start_proxy

    test_http_proxy_attribution
    test_websocket_attribution
    test_https_tunnel_attribution
    test_socks5_tls_attribution

    echo ""
    echo "Assertions: ${PASSED_ASSERTIONS}/${TOTAL_ASSERTIONS} passed"
    if [[ "${FAILED_ASSERTIONS}" -gt 0 ]]; then
        echo "Client process attribution regressions detected: ${FAILED_ASSERTIONS} assertion(s) failed."
        exit 1
    fi

    echo "All client process attribution tests passed."
}

main "$@"
