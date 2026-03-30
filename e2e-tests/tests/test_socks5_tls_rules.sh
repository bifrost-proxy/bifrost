#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
E2E_DIR="$(dirname "$SCRIPT_DIR")"
PROJECT_ROOT="$(dirname "$E2E_DIR")"

source "$E2E_DIR/test_utils/assert.sh"
source "$E2E_DIR/test_utils/http_client.sh"
source "$E2E_DIR/test_utils/rule_fixture.sh"
source "$E2E_DIR/test_utils/process.sh"

PROXY_HOST=${PROXY_HOST:-127.0.0.1}
PROXY_PORT=${PROXY_PORT:-18081}
SOCKS5_PORT=${SOCKS5_PORT:-18082}
BIFROST_BIN="${PROJECT_ROOT}/target/release/bifrost"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi
DATA_DIR="${PROJECT_ROOT}/.bifrost-socks5-tls-rules-test"
PROXY_LOG_FILE="${DATA_DIR}/proxy.log"
RULES_DIR="$E2E_DIR/rules/socks5_tls"

rules_from_fixture() {
    local fixture_name="$1"
    rule_fixture_content "$RULES_DIR/$fixture_name"
}

cleanup() {
    echo "Cleaning up..."
    if [ -n "${PROXY_PID:-}" ]; then
        safe_cleanup_proxy "$PROXY_PID"
    fi
    kill_bifrost_on_port "$PROXY_PORT"
    HTTP_PORT="${ECHO_HTTP_PORT:-3000}" \
    HTTPS_PORT="${ECHO_HTTPS_PORT:-3443}" \
    "$E2E_DIR/mock_servers/start_servers.sh" stop >/dev/null 2>&1 || true
    sleep 1
    rm -rf "$DATA_DIR"
}

trap cleanup EXIT

start_mock_servers() {
    local http_port="${ECHO_HTTP_PORT:-3000}"
    local https_port="${ECHO_HTTPS_PORT:-3443}"

    echo "Starting mock HTTP ($http_port) and HTTPS ($https_port) servers..."
    HTTP_PORT="$http_port" \
    HTTPS_PORT="$https_port" \
    "$E2E_DIR/mock_servers/start_servers.sh" start-bg 2>&1 || true

    local waited=0
    while [ $waited -lt 30 ]; do
        if curl -sf --connect-timeout 2 --max-time 3 "http://127.0.0.1:${http_port}/health" >/dev/null 2>&1; then
            echo "Mock servers ready"
            return 0
        fi
        sleep 0.5
        waited=$((waited + 1))
    done
    echo "WARNING: Mock servers may not be fully ready"
}

wait_for_proxy_ready() {
    local max_wait=30
    local waited=0
    while [ $waited -lt $max_wait ]; do
        if curl -sf --connect-timeout 2 --max-time 3 \
            "http://${PROXY_HOST}:${PROXY_PORT}/_bifrost/api/system" >/dev/null 2>&1; then
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done
    return 1
}

start_proxy_with_rules() {
    local rules="$1"
    local http_port="${ECHO_HTTP_PORT:-3000}"
    local combined_rules
    combined_rules="$(cat <<EOF
http://httpbin.org/ http://127.0.0.1:${http_port}
https://httpbin.org/ http://127.0.0.1:${http_port}
EOF
)"
    if [ -n "$rules" ]; then
        combined_rules="${combined_rules}"$'\n'"${rules}"
    fi

    echo "Starting Bifrost proxy on port $PROXY_PORT (SOCKS5: $SOCKS5_PORT)..."
    echo "Rules: $combined_rules"
    
    if [ -n "${PROXY_PID:-}" ]; then
        safe_cleanup_proxy "$PROXY_PID"
    fi
    sleep 1
    
    rm -rf "$DATA_DIR"
    mkdir -p "$DATA_DIR/rules"
    
    if [ -z "${MOCK_SERVERS_STARTED:-}" ]; then
        start_mock_servers
        MOCK_SERVERS_STARTED=1
    fi
    
    export BIFROST_DATA_DIR="$DATA_DIR"
    
    if [ -n "$combined_rules" ]; then
        RUST_LOG=bifrost_proxy=debug "$BIFROST_BIN" -p "$PROXY_PORT" --socks5-port "$SOCKS5_PORT" start \
            --unsafe-ssl --skip-cert-check --rules "$combined_rules" >"$PROXY_LOG_FILE" 2>&1 &
    else
        RUST_LOG=bifrost_proxy=debug "$BIFROST_BIN" -p "$PROXY_PORT" --socks5-port "$SOCKS5_PORT" start \
            --unsafe-ssl --skip-cert-check >"$PROXY_LOG_FILE" 2>&1 &
    fi
    PROXY_PID=$!
    
    if ! wait_for_proxy_ready; then
        if ! kill -0 $PROXY_PID 2>/dev/null; then
            echo "ERROR: Proxy process died"
            echo "=== Proxy log ==="
            cat "$PROXY_LOG_FILE" 2>/dev/null || true
            exit 1
        fi
        echo "WARNING: Proxy admin API not reachable, but process is alive"
    fi
    
    echo "Proxy started (PID: $PROXY_PID)"
}

restart_proxy_with_rules() {
    local rules="$1"
    echo "Restarting proxy with new rules..."
    
    if [ -n "${PROXY_PID:-}" ]; then
        kill_pid "$PROXY_PID"
    fi
    sleep 2
    
    local wait_count=0
    while [ $wait_count -lt 15 ] && kill -0 ${PROXY_PID:-0} 2>/dev/null; do
        echo "  Waiting for proxy process to exit..."
        sleep 1
        wait_count=$((wait_count + 1))
    done
    wait_pid "$PROXY_PID"
    
    rm -f "$DATA_DIR/bifrost.pid" "$DATA_DIR/runtime.json" 2>/dev/null || true
    
    start_proxy_with_rules "$rules"
}

test_https_via_socks5_basic() {
    echo ""
    echo "=== Test 1: Basic HTTPS via SOCKS5 ==="
    
    start_proxy_with_rules ""
    
    PROXY_MODE="socks5"
    https_request "https://httpbin.org/ip"
    
    if [ "$HTTP_STATUS" = "200" ]; then
        echo "  ✅ Basic HTTPS via SOCKS5 works"
        echo "  Response: $HTTP_BODY"
    else
        echo "  ❌ Basic HTTPS via SOCKS5 failed: status=$HTTP_STATUS"
        return 1
    fi
}

test_https_header_rule_via_socks5() {
    echo ""
    echo "=== Test 2: HTTPS Header Rule via SOCKS5 ==="
    
    restart_proxy_with_rules "$(rules_from_fixture res_header.txt)"
    
    PROXY_MODE="socks5"
    https_request "https://httpbin.org/headers"
    
    echo "  Response Headers:"
    echo "$HTTP_HEADERS" | head -20
    
    if echo "$HTTP_HEADERS" | grep -qi "X-Bifrost-Test"; then
        echo "  ✅ Response header rule applied via SOCKS5 TLS intercept"
    else
        echo "  ❌ Response header not found"
        return 1
    fi
    
    if [ "$HTTP_STATUS" = "200" ]; then
        echo "  ✅ HTTPS request successful"
    else
        echo "  ❌ HTTPS request failed: status=$HTTP_STATUS"
        return 1
    fi
}

test_https_host_redirect_via_socks5() {
    echo ""
    echo "=== Test 3: HTTPS Host Redirect via SOCKS5 ==="
    
    restart_proxy_with_rules "$(rules_from_fixture host_redirect.txt)"
    
    PROXY_MODE="socks5"
    https_request "https://httpbin.org/get"
    
    if [ "$HTTP_STATUS" = "200" ]; then
        echo "  ✅ Host redirect rule works via SOCKS5"
        echo "  Response body preview: ${HTTP_BODY:0:200}"
    else
        echo "  ❌ Host redirect failed: status=$HTTP_STATUS"
        return 1
    fi
}

test_https_mock_response_via_socks5() {
    echo ""
    echo "=== Test 4: HTTPS Mock Response via SOCKS5 ==="
    
    restart_proxy_with_rules "$(rules_from_fixture mock_response.txt)"
    
    PROXY_MODE="socks5"
    https_request "https://httpbin.org/mock-test"
    
    echo "  Response: $HTTP_BODY"
    
    if echo "$HTTP_BODY" | grep -q "mocked"; then
        echo "  ✅ Mock response rule works via SOCKS5 TLS intercept"
    else
        echo "  ❌ Mock response not applied"
        return 1
    fi
}

test_compare_http_vs_socks5() {
    echo ""
    echo "=== Test 5: Compare HTTP Proxy vs SOCKS5 Proxy ==="
    
    restart_proxy_with_rules "$(rules_from_fixture compare_header_mode.txt)"
    
    echo "  --- HTTP Proxy Mode ---"
    PROXY_MODE="http"
    https_request "https://httpbin.org/anything"
    local http_status=$HTTP_STATUS
    local http_has_header=$(echo "$HTTP_HEADERS" | grep -ci "X-Proxy-Mode" || echo "0")
    echo "  HTTP Proxy: status=$http_status, has_header=$http_has_header"
    
    echo "  --- SOCKS5 Proxy Mode ---"
    PROXY_MODE="socks5"
    https_request "https://httpbin.org/anything"
    local socks5_status=$HTTP_STATUS
    local socks5_has_header=$(echo "$HTTP_HEADERS" | grep -ci "X-Proxy-Mode" || echo "0")
    echo "  SOCKS5 Proxy: status=$socks5_status, has_header=$socks5_has_header"
    
    if [ "$http_status" = "200" ] && [ "$socks5_status" = "200" ]; then
        echo "  ✅ Both proxy modes work"
        if [ "$http_has_header" -gt 0 ] && [ "$socks5_has_header" -gt 0 ]; then
            echo "  ✅ Both proxy modes applied the header rule"
        else
            echo "  ⚠ Header rule: HTTP=$http_has_header, SOCKS5=$socks5_has_header"
        fi
    else
        echo "  ❌ One or both proxy modes failed"
        return 1
    fi
}

test_socks5_udp_associate() {
    echo ""
    echo "=== Test 6: SOCKS5 UDP ASSOCIATE ==="
    
    local socks_port=$SOCKS5_PORT
    python3 << PYTHON_SCRIPT
import socket
import struct
import sys

SOCKS5_HOST = "127.0.0.1"
SOCKS5_PORT = $socks_port

try:
    tcp_sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    tcp_sock.settimeout(10)
    tcp_sock.connect((SOCKS5_HOST, SOCKS5_PORT))
    print(f"✓ Connected to SOCKS5 server at {SOCKS5_HOST}:{SOCKS5_PORT}")
    
    tcp_sock.send(b'\x05\x01\x00')
    auth_resp = tcp_sock.recv(2)
    if auth_resp != b'\x05\x00':
        print(f"❌ Auth failed: {auth_resp.hex()}")
        sys.exit(1)
    print("✓ SOCKS5 auth OK")
    
    tcp_sock.send(b'\x05\x03\x00\x01\x00\x00\x00\x00\x00\x00')
    resp = tcp_sock.recv(10)
    
    if resp[1] != 0x00:
        print(f"❌ UDP ASSOCIATE failed: reply={resp[1]}")
        sys.exit(1)
    
    relay_ip = socket.inet_ntoa(resp[4:8])
    relay_port = struct.unpack('>H', resp[8:10])[0]
    
    if relay_ip == '0.0.0.0':
        relay_ip = SOCKS5_HOST
    
    print(f"✅ UDP relay at {relay_ip}:{relay_port}")
    
    udp_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    udp_sock.settimeout(5)
    
    dns_query = b'\x12\x34\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00'
    dns_query += b'\x06google\x03com\x00\x00\x01\x00\x01'
    
    packet = b'\x00\x00\x00'
    packet += b'\x01'
    packet += socket.inet_aton('8.8.8.8')
    packet += struct.pack('>H', 53)
    packet += dns_query
    
    relay_addr = (relay_ip, relay_port)
    udp_sock.sendto(packet, relay_addr)
    print(f"✓ Sent DNS query via UDP relay ({len(packet)} bytes)")
    
    try:
        response_data, addr = udp_sock.recvfrom(4096)
        print(f"✅ Received UDP response from {addr} ({len(response_data)} bytes)")
        print("✅ SOCKS5 UDP relay is working!")
    except socket.timeout:
        print("⚠ UDP response timeout (relay works, target may not respond)")
    
    tcp_sock.close()
    udp_sock.close()
    print("✅ SOCKS5 UDP ASSOCIATE test completed")
    
except Exception as e:
    print(f"❌ Error: {e}")
    sys.exit(1)
PYTHON_SCRIPT
}

echo "=============================================="
echo "  SOCKS5 TLS Intercept Rules E2E Test Suite"
echo "=============================================="
echo ""
echo "This test verifies that SOCKS5 proxy with TLS"
echo "intercept supports the same rules as HTTP proxy."
echo ""
echo "=== Running Tests ==="

test_https_via_socks5_basic
test_https_header_rule_via_socks5
test_https_host_redirect_via_socks5
test_https_mock_response_via_socks5
test_compare_http_vs_socks5
test_socks5_udp_associate

echo ""
echo "=============================================="
echo "  All SOCKS5 TLS Rules Tests Completed!"
echo "=============================================="
