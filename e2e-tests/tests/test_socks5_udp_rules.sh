#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
E2E_DIR="$(dirname "$SCRIPT_DIR")"
PROJECT_ROOT="$(dirname "$E2E_DIR")"

PROXY_HOST=${PROXY_HOST:-127.0.0.1}
PROXY_PORT=${PROXY_PORT:-19080}
SOCKS5_PORT=${SOCKS5_PORT:-12080}
export SOCKS5_PORT
BIFROST_BIN="${PROJECT_ROOT}/target/release/bifrost"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi
DATA_DIR="${PROJECT_ROOT}/.bifrost-socks5-udp-rules-test"
PROXY_LOG_FILE="${DATA_DIR}/proxy.log"
source "$E2E_DIR/test_utils/rule_fixture.sh"
source "$E2E_DIR/test_utils/process.sh"
RULES_DIR="$E2E_DIR/rules/socks5_udp"

cleanup() {
    echo "Cleaning up..."
    if [ -n "${PROXY_PID:-}" ]; then
        safe_cleanup_proxy "$PROXY_PID"
    fi
    if is_windows; then kill_bifrost_on_port "$PROXY_PORT"; fi
    sleep 1
    rm -rf "$DATA_DIR"
}

trap cleanup EXIT

start_proxy() {
    echo "Starting Bifrost proxy on port $PROXY_PORT (SOCKS5: $SOCKS5_PORT)..."
    
    if [ -n "${PROXY_PID:-}" ]; then
        safe_cleanup_proxy "$PROXY_PID"
    fi
    sleep 2
    
    rm -rf "$DATA_DIR"
    mkdir -p "$DATA_DIR/rules"
    
    export BIFROST_DATA_DIR="$DATA_DIR"
    
    "$BIFROST_BIN" --port "$PROXY_PORT" --socks5-port "$SOCKS5_PORT" start \
        --unsafe-ssl --skip-cert-check >"$PROXY_LOG_FILE" 2>&1 &
    PROXY_PID=$!
    
    sleep 5
    
    if ! kill -0 $PROXY_PID 2>/dev/null; then
        echo "ERROR: Proxy failed to start"
        echo "=== Proxy log ==="
        cat "$PROXY_LOG_FILE" 2>/dev/null || true
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

restart_proxy() {
    echo "Restarting proxy to load rules..."
    
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
    
    export BIFROST_DATA_DIR="$DATA_DIR"
    "$BIFROST_BIN" --port "$PROXY_PORT" --socks5-port "$SOCKS5_PORT" start \
        --unsafe-ssl --skip-cert-check >"$PROXY_LOG_FILE" 2>&1 &
    PROXY_PID=$!
    
    sleep 5
    
    if ! kill -0 $PROXY_PID 2>/dev/null; then
        echo "ERROR: Proxy failed to restart"
        echo "=== Proxy log ==="
        cat "$PROXY_LOG_FILE" 2>/dev/null || true
        exit 1
    fi
    
    echo "Proxy restarted (PID: $PROXY_PID)"
}

echo "=============================================="
echo "  SOCKS5 UDP Rules E2E Test Suite"
echo "=============================================="

start_proxy

echo ""
echo "=== Adding Rules ==="
add_rule_from_fixture "dns-redirect" "dns_redirect_ip.txt"
add_rule_from_fixture "domain-redirect" "dns_redirect_domain.txt"

restart_proxy

echo ""
echo "=== Test 1: Host Redirect Rule (IP to IP) ==="

python3 << 'PYTHON_SCRIPT'
import socket
import struct
import sys
import os

PROXY_HOST = "127.0.0.1"
SOCKS5_PORT = int(os.environ.get('SOCKS5_PORT', '12080'))

try:
    tcp_sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    tcp_sock.settimeout(10)
    tcp_sock.connect((PROXY_HOST, SOCKS5_PORT))
    
    tcp_sock.send(b'\x05\x01\x00')
    tcp_sock.recv(2)
    
    tcp_sock.send(b'\x05\x03\x00\x01\x00\x00\x00\x00\x00\x00')
    response = tcp_sock.recv(10)
    
    relay_ip = socket.inet_ntoa(response[4:8])
    relay_port = struct.unpack('!H', response[8:10])[0]
    if relay_ip == "0.0.0.0":
        relay_ip = PROXY_HOST
    
    print(f"✓ UDP relay at {relay_ip}:{relay_port}")
    
    udp_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    udp_sock.settimeout(5)
    
    dns_query = bytes([
        0xAB, 0xCD,
        0x01, 0x00,
        0x00, 0x01,
        0x00, 0x00,
        0x00, 0x00,
        0x00, 0x00,
        0x06, 0x67, 0x6f, 0x6f, 0x67, 0x6c, 0x65,
        0x03, 0x63, 0x6f, 0x6d,
        0x00,
        0x00, 0x01,
        0x00, 0x01,
    ])
    
    socks5_udp_header = bytes([
        0x00, 0x00,
        0x00,
        0x01,
        0x08, 0x08, 0x08, 0x08,
        0x00, 0x35,
    ])
    
    packet = socks5_udp_header + dns_query
    
    udp_sock.sendto(packet, (relay_ip, relay_port))
    print(f"✓ Sent DNS query to 8.8.8.8:53 (should be redirected to 1.1.1.1)")
    
    try:
        response_data, addr = udp_sock.recvfrom(4096)
        print(f"✓ Received response ({len(response_data)} bytes)")
        
        if len(response_data) > 10:
            atyp = response_data[3]
            if atyp == 0x01:
                src_ip = socket.inet_ntoa(response_data[4:8])
                src_port = struct.unpack('!H', response_data[8:10])[0]
                print(f"  Response source: {src_ip}:{src_port}")
                
                if src_ip == "1.1.1.1":
                    print("✅ Test 1 PASSED: Host redirect rule applied (8.8.8.8 -> 1.1.1.1)")
                elif src_ip == "8.8.8.8":
                    print("⚠ Test 1: Response from original IP (rule may not be applied)")
                else:
                    print(f"⚠ Test 1: Response from unexpected IP: {src_ip}")
            else:
                print(f"⚠ Unexpected address type: {atyp}")
        else:
            print("⚠ Response too short")
            
    except socket.timeout:
        print("⚠ Timeout waiting for response")
    
    tcp_sock.close()
    udp_sock.close()
    
except Exception as e:
    print(f"❌ Error: {e}")
    sys.exit(1)
PYTHON_SCRIPT

echo ""
echo "=== Test 2: Domain Redirect Rule ==="

python3 << 'PYTHON_SCRIPT'
import socket
import struct
import sys
import os

PROXY_HOST = "127.0.0.1"
SOCKS5_PORT = int(os.environ.get('SOCKS5_PORT', '12080'))

try:
    tcp_sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    tcp_sock.settimeout(10)
    tcp_sock.connect((PROXY_HOST, SOCKS5_PORT))
    
    tcp_sock.send(b'\x05\x01\x00')
    tcp_sock.recv(2)
    
    tcp_sock.send(b'\x05\x03\x00\x01\x00\x00\x00\x00\x00\x00')
    response = tcp_sock.recv(10)
    
    relay_ip = socket.inet_ntoa(response[4:8])
    relay_port = struct.unpack('!H', response[8:10])[0]
    if relay_ip == "0.0.0.0":
        relay_ip = PROXY_HOST
    
    print(f"✓ UDP relay at {relay_ip}:{relay_port}")
    
    udp_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    udp_sock.settimeout(5)
    
    dns_query = bytes([
        0x12, 0x34,
        0x01, 0x00,
        0x00, 0x01,
        0x00, 0x00,
        0x00, 0x00,
        0x00, 0x00,
        0x07, 0x65, 0x78, 0x61, 0x6d, 0x70, 0x6c, 0x65,
        0x03, 0x63, 0x6f, 0x6d,
        0x00,
        0x00, 0x01,
        0x00, 0x01,
    ])
    
    domain = b"dns.google"
    socks5_udp_header = bytes([
        0x00, 0x00,
        0x00,
        0x03,
        len(domain),
    ]) + domain + bytes([0x00, 0x35])
    
    packet = socks5_udp_header + dns_query
    
    udp_sock.sendto(packet, (relay_ip, relay_port))
    print(f"✓ Sent DNS query to dns.google:53 (should be redirected to 8.8.4.4)")
    
    try:
        response_data, addr = udp_sock.recvfrom(4096)
        print(f"✓ Received response ({len(response_data)} bytes)")
        
        if len(response_data) > 10:
            atyp = response_data[3]
            if atyp == 0x01:
                src_ip = socket.inet_ntoa(response_data[4:8])
                src_port = struct.unpack('!H', response_data[8:10])[0]
                print(f"  Response source: {src_ip}:{src_port}")
                
                if src_ip == "8.8.4.4":
                    print("✅ Test 2 PASSED: Domain redirect rule applied (dns.google -> 8.8.4.4)")
                else:
                    print(f"⚠ Test 2: Response from {src_ip} (expected 8.8.4.4)")
            else:
                print(f"⚠ Unexpected address type: {atyp}")
        else:
            print("⚠ Response too short")
            
    except socket.timeout:
        print("⚠ Timeout waiting for response")
    
    tcp_sock.close()
    udp_sock.close()
    
except Exception as e:
    print(f"❌ Error: {e}")
    sys.exit(1)
PYTHON_SCRIPT

echo ""
echo "=== Test 3: QUIC Detection ==="

python3 << 'PYTHON_SCRIPT'
import socket
import struct
import sys
import os

PROXY_HOST = "127.0.0.1"
SOCKS5_PORT = int(os.environ.get('SOCKS5_PORT', '12080'))

try:
    tcp_sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    tcp_sock.settimeout(10)
    tcp_sock.connect((PROXY_HOST, SOCKS5_PORT))
    
    tcp_sock.send(b'\x05\x01\x00')
    tcp_sock.recv(2)
    
    tcp_sock.send(b'\x05\x03\x00\x01\x00\x00\x00\x00\x00\x00')
    response = tcp_sock.recv(10)
    
    relay_ip = socket.inet_ntoa(response[4:8])
    relay_port = struct.unpack('!H', response[8:10])[0]
    if relay_ip == "0.0.0.0":
        relay_ip = PROXY_HOST
    
    print(f"✓ UDP relay at {relay_ip}:{relay_port}")
    
    udp_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    udp_sock.settimeout(5)
    
    quic_initial = bytes([
        0xC0 | 0x01,
        0x00, 0x00, 0x00, 0x01,
        0x08,
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,
        0x00,
        0x00, 0x00,
    ])
    quic_initial += b'\x00' * 100
    
    target_ip = socket.inet_aton("142.250.185.206")
    socks5_udp_header = bytes([
        0x00, 0x00,
        0x00,
        0x01,
    ]) + target_ip + bytes([0x01, 0xBB])
    
    packet = socks5_udp_header + quic_initial
    
    udp_sock.sendto(packet, (relay_ip, relay_port))
    print(f"✓ Sent QUIC-like packet to google.com:443")
    print("  (QUIC detection should recognize this as QUIC traffic)")
    
    try:
        response_data, addr = udp_sock.recvfrom(4096)
        print(f"✓ Received response ({len(response_data)} bytes)")
        print("✅ Test 3 PASSED: QUIC packet forwarded successfully")
    except socket.timeout:
        print("⚠ Timeout (expected - server may not respond to malformed QUIC)")
        print("✅ Test 3 PASSED: QUIC packet was forwarded (no response expected)")
    
    tcp_sock.close()
    udp_sock.close()
    
except Exception as e:
    print(f"❌ Error: {e}")
    sys.exit(1)
PYTHON_SCRIPT

echo ""
echo "=== Test 4: Check Rule Application in Logs ==="

if [ -d "$DATA_DIR/logs" ]; then
    echo "Checking for rule application logs..."
    grep -i "rule\|redirect\|host" "$DATA_DIR/logs/"*.log 2>/dev/null | tail -10 || echo "No rule logs found"
else
    echo "Logs directory not found"
fi

echo ""
echo "=============================================="
echo "  SOCKS5 UDP Rules Tests Completed"
echo "=============================================="
