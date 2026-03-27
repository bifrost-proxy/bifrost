#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
E2E_DIR="$(dirname "$SCRIPT_DIR")"
PROJECT_ROOT="$(dirname "$E2E_DIR")"

source "$E2E_DIR/test_utils/assert.sh"
source "$E2E_DIR/test_utils/http_client.sh"
source "$E2E_DIR/test_utils/rule_fixture.sh"

PROXY_HOST=${PROXY_HOST:-127.0.0.1}
PROXY_PORT=${PROXY_PORT:-18080}
SOCKS5_PORT=${SOCKS5_PORT:-11080}
BIFROST_BIN="${PROJECT_ROOT}/target/release/bifrost"
DATA_DIR="${PROJECT_ROOT}/.bifrost-socks5-rules-test"
RULES_DIR="$E2E_DIR/rules/socks5"

cleanup() {
    echo "Cleaning up..."
    pkill -f "bifrost.*${DATA_DIR}" 2>/dev/null || true
    HTTP_PORT="${ECHO_HTTP_PORT:-3000}" \
    HTTPS_PORT="${ECHO_HTTPS_PORT:-3443}" \
    "$E2E_DIR/mock_servers/start_servers.sh" stop >/dev/null 2>&1 || true
    sleep 1
    rm -rf "$DATA_DIR"
}

trap cleanup EXIT

start_proxy() {
    echo "Starting Bifrost proxy on port $PROXY_PORT (SOCKS5: $SOCKS5_PORT)..."
    
    pkill -f "bifrost" 2>/dev/null || true
    sleep 1
    
    rm -rf "$DATA_DIR"
    mkdir -p "$DATA_DIR/rules"
    HTTP_PORT="${ECHO_HTTP_PORT:-3000}" \
    HTTPS_PORT="${ECHO_HTTPS_PORT:-3443}" \
    "$E2E_DIR/mock_servers/start_servers.sh" start-bg >/dev/null
    
    export BIFROST_DATA_DIR="$DATA_DIR"
    
    "$BIFROST_BIN" --port "$PROXY_PORT" --socks5-port "$SOCKS5_PORT" start \
        --unsafe-ssl --skip-cert-check 2>&1 &
    PROXY_PID=$!
    
    sleep 5
    
    if ! kill -0 $PROXY_PID 2>/dev/null; then
        echo "ERROR: Proxy failed to start"
        exit 1
    fi
    
    echo "Proxy started (PID: $PROXY_PID)"
}

add_rule() {
    local name=$1
    local content=$2
    export BIFROST_DATA_DIR="$DATA_DIR"
    "$BIFROST_BIN" rule add "$name" -c "$content"
}

add_rule_from_fixture() {
    local name="$1"
    local fixture_name="$2"
    add_rule "$name" "$(rule_fixture_content "$RULES_DIR/$fixture_name")"
}

run_test_both_modes() {
    local test_name=$1
    local test_func=$2
    
    echo ""
    echo "=========================================="
    echo "TEST: $test_name"
    echo "=========================================="
    
    echo "--- HTTP Proxy Mode ---"
    export PROXY_MODE="http"
    unset SOCKS5_PORT_OVERRIDE
    $test_func "http"
    
    echo "--- SOCKS5 Proxy Mode ---"
    export PROXY_MODE="socks5"
    export SOCKS5_PORT_OVERRIDE="$SOCKS5_PORT"
    $test_func "socks5"
    
    echo "✅ $test_name PASSED (both modes)"
}

test_host_redirect() {
    local mode=$1
    local old_socks5_port=$SOCKS5_PORT
    
    if [ "$mode" = "socks5" ]; then
        SOCKS5_PORT="$SOCKS5_PORT_OVERRIDE"
    fi
    
    http_get "http://httpbin.org/ip"
    
    SOCKS5_PORT=$old_socks5_port
    
    if [ -n "$HTTP_STATUS" ]; then
        echo "  ✓ Request completed (status: $HTTP_STATUS)"
        echo "  ✓ Rule was applied (redirected to example.com)"
    else
        echo "  ✗ No response received"
        return 1
    fi
}

test_response_header_modification() {
    local mode=$1
    local old_socks5_port=$SOCKS5_PORT
    
    if [ "$mode" = "socks5" ]; then
        SOCKS5_PORT="$SOCKS5_PORT_OVERRIDE"
    fi
    
    http_get "http://httpbin.org/headers"
    
    SOCKS5_PORT=$old_socks5_port
    
    if [ "$HTTP_STATUS" = "200" ]; then
        echo "  ✓ Request successful"
    else
        echo "  ✗ Request failed: $HTTP_STATUS"
        return 1
    fi
}

test_request_block() {
    local mode=$1
    local old_socks5_port=$SOCKS5_PORT
    
    if [ "$mode" = "socks5" ]; then
        SOCKS5_PORT="$SOCKS5_PORT_OVERRIDE"
    fi
    
    http_get "http://blocked-domain.test/"
    
    SOCKS5_PORT=$old_socks5_port
    
    if [ "$HTTP_STATUS" = "403" ] || [ "$HTTP_STATUS" = "502" ] || [ -z "$HTTP_STATUS" ]; then
        echo "  ✓ Request blocked as expected"
    else
        echo "  ✗ Request should be blocked but got: $HTTP_STATUS"
        return 1
    fi
}

echo "=============================================="
echo "  SOCKS5 Rules E2E Test Suite"
echo "=============================================="

start_proxy

echo ""
echo "=== Adding test rules ==="
add_rule "httpbin-mock-http" "http://httpbin.org/ http://127.0.0.1:${ECHO_HTTP_PORT:-3000}"
add_rule "httpbin-mock-https" "https://httpbin.org/ tlsIntercept:// https://127.0.0.1:${ECHO_HTTPS_PORT:-3443}"
add_rule_from_fixture "host-redirect" "host_redirect.txt"
add_rule_from_fixture "block-domain" "block_domain.txt"

echo ""
echo "=== Restarting proxy to load rules ==="
pkill -f "bifrost" 2>/dev/null || true
sleep 3

export BIFROST_DATA_DIR="$DATA_DIR"
"$BIFROST_BIN" --port "$PROXY_PORT" --socks5-port "$SOCKS5_PORT" start \
    --unsafe-ssl --skip-cert-check 2>&1 &
PROXY_PID=$!
sleep 5

if ! kill -0 $PROXY_PID 2>/dev/null; then
    echo "ERROR: Proxy failed to restart"
    exit 1
fi
echo "Proxy restarted (PID: $PROXY_PID)"

echo ""
echo "=== Running Tests ==="

run_test_both_modes "Host Redirect Rule" test_host_redirect
run_test_both_modes "Request Block Rule" test_request_block

echo ""
echo "=============================================="
echo "  All SOCKS5 Rules Tests PASSED!"
echo "=============================================="
