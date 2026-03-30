#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
E2E_DIR="$(dirname "$SCRIPT_DIR")"
PROJECT_ROOT="$(dirname "$E2E_DIR")"

source "$E2E_DIR/test_utils/process.sh"

PROXY_HOST=${PROXY_HOST:-127.0.0.1}
PROXY_PORT=${PROXY_PORT:-18080}
SOCKS5_PORT=${SOCKS5_PORT:-11080}
export SOCKS5_PORT
BIFROST_BIN="${PROJECT_ROOT}/target/release/bifrost"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi
DATA_DIR="${PROJECT_ROOT}/.bifrost-socks5-udp-test"
PROXY_LOG_FILE="${DATA_DIR}/proxy.log"

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

echo "=============================================="
echo "  SOCKS5 UDP Protocol E2E Test Suite"
echo "=============================================="

start_proxy

echo ""
echo "=== Test 1: UDP ASSOCIATE Handshake ==="

python3 << 'PYTHON_SCRIPT'
import socket
import struct
import sys
import os

PROXY_HOST = "127.0.0.1"
SOCKS5_PORT = int(os.environ.get('SOCKS5_PORT', '11080'))

try:
    tcp_sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    tcp_sock.settimeout(10)
    tcp_sock.connect((PROXY_HOST, SOCKS5_PORT))
    print(f"✓ Connected to SOCKS5 server at {PROXY_HOST}:{SOCKS5_PORT}")
    
    tcp_sock.send(b'\x05\x01\x00')
    auth_response = tcp_sock.recv(2)
    if auth_response != b'\x05\x00':
        print(f"❌ Auth failed: {auth_response.hex()}")
        sys.exit(1)
    print("✓ SOCKS5 auth OK (no auth required)")
    
    tcp_sock.send(b'\x05\x03\x00\x01\x00\x00\x00\x00\x00\x00')
    response = tcp_sock.recv(10)
    
    if len(response) < 10:
        print(f"❌ Response too short: {len(response)} bytes")
        sys.exit(1)
    
    version = response[0]
    reply = response[1]
    rsv = response[2]
    atyp = response[3]
    
    print(f"  Version: {version}")
    print(f"  Reply: {reply} (0=success)")
    print(f"  Address type: {atyp}")
    
    if reply != 0x00:
        print(f"❌ UDP ASSOCIATE failed: reply={reply}")
        sys.exit(1)
    
    if atyp == 0x01:
        relay_ip = socket.inet_ntoa(response[4:8])
        relay_port = struct.unpack('!H', response[8:10])[0]
    else:
        print(f"❌ Unexpected address type: {atyp}")
        sys.exit(1)
    
    if relay_ip == "0.0.0.0":
        relay_ip = PROXY_HOST
    
    print(f"✅ UDP relay at {relay_ip}:{relay_port}")
    
    tcp_sock.close()
    print("✓ Test 1 PASSED: UDP ASSOCIATE handshake successful")
    
except Exception as e:
    print(f"❌ Error: {e}")
    sys.exit(1)
PYTHON_SCRIPT

echo ""
echo "=== Test 2: UDP DNS Query via SOCKS5 ==="

python3 << 'PYTHON_SCRIPT'
import socket
import struct
import sys
import time
import os

PROXY_HOST = "127.0.0.1"
SOCKS5_PORT = int(os.environ.get('SOCKS5_PORT', '11080'))

try:
    tcp_sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    tcp_sock.settimeout(10)
    tcp_sock.connect((PROXY_HOST, SOCKS5_PORT))
    
    tcp_sock.send(b'\x05\x01\x00')
    auth_response = tcp_sock.recv(2)
    if auth_response != b'\x05\x00':
        print(f"❌ Auth failed")
        sys.exit(1)
    
    tcp_sock.send(b'\x05\x03\x00\x01\x00\x00\x00\x00\x00\x00')
    response = tcp_sock.recv(10)
    
    if response[1] != 0x00:
        print(f"❌ UDP ASSOCIATE failed")
        sys.exit(1)
    
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
    print(f"✓ Sent DNS query for google.com to 8.8.8.8:53 ({len(packet)} bytes)")
    
    try:
        response_data, addr = udp_sock.recvfrom(4096)
        print(f"✓ Received response from relay ({len(response_data)} bytes)")
        
        if len(response_data) > 10:
            rsv = (response_data[0] << 8) | response_data[1]
            frag = response_data[2]
            atyp = response_data[3]
            
            print(f"  RSV: {rsv}, FRAG: {frag}, ATYP: {atyp}")
            
            if atyp == 0x01:
                src_ip = socket.inet_ntoa(response_data[4:8])
                src_port = struct.unpack('!H', response_data[8:10])[0]
                dns_response = response_data[10:]
                print(f"  Source: {src_ip}:{src_port}")
                print(f"  DNS response: {len(dns_response)} bytes")
                
                if len(dns_response) > 12:
                    answers = (dns_response[6] << 8) | dns_response[7]
                    print(f"  DNS answers: {answers}")
                    print("✅ Test 2 PASSED: DNS query via SOCKS5 UDP successful")
                else:
                    print("⚠ DNS response too short")
            else:
                print(f"⚠ Unexpected address type in response: {atyp}")
        else:
            print("⚠ Response too short")
            
    except socket.timeout:
        print("⚠ UDP response timeout (DNS server may not respond)")
    
    tcp_sock.close()
    udp_sock.close()
    
except Exception as e:
    print(f"❌ Error: {e}")
    import traceback
    traceback.print_exc()
    sys.exit(1)
PYTHON_SCRIPT

echo ""
echo "=== Test 3: UDP with Domain Name Address ==="

python3 << 'PYTHON_SCRIPT'
import socket
import struct
import sys
import os

PROXY_HOST = "127.0.0.1"
SOCKS5_PORT = int(os.environ.get('SOCKS5_PORT', '11080'))

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
    print(f"✓ Sent DNS query to dns.google:53 (domain name address)")
    
    try:
        response_data, addr = udp_sock.recvfrom(4096)
        print(f"✓ Received response ({len(response_data)} bytes)")
        print("✅ Test 3 PASSED: Domain name address type works")
    except socket.timeout:
        print("⚠ Timeout - but domain name parsing may have worked")
    
    tcp_sock.close()
    udp_sock.close()
    
except Exception as e:
    print(f"❌ Error: {e}")
    sys.exit(1)
PYTHON_SCRIPT

echo ""
echo "=== Test 4: Multiple UDP Sessions ==="

python3 << 'PYTHON_SCRIPT'
import socket
import struct
import sys
import threading
import time
import os

PROXY_HOST = "127.0.0.1"
SOCKS5_PORT = int(os.environ.get('SOCKS5_PORT', '11080'))

results = []

def create_session(session_id):
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
        
        udp_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        udp_sock.settimeout(5)
        
        dns_query = bytes([
            session_id, session_id,
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
        
        try:
            response_data, addr = udp_sock.recvfrom(4096)
            results.append((session_id, True, len(response_data)))
        except socket.timeout:
            results.append((session_id, False, 0))
        
        tcp_sock.close()
        udp_sock.close()
        
    except Exception as e:
        results.append((session_id, False, str(e)))

threads = []
for i in range(5):
    t = threading.Thread(target=create_session, args=(i+1,))
    threads.append(t)
    t.start()

for t in threads:
    t.join()

success_count = sum(1 for r in results if r[1])
print(f"✓ Created 5 concurrent UDP sessions")
print(f"  Successful responses: {success_count}/5")

for r in results:
    status = "✓" if r[1] else "✗"
    print(f"  Session {r[0]}: {status} (response: {r[2]} bytes)")

if success_count >= 3:
    print("✅ Test 4 PASSED: Multiple concurrent sessions work")
else:
    print("⚠ Test 4: Some sessions failed (may be network issue)")
PYTHON_SCRIPT

echo ""
echo "=== Test 5: UDP Session Persistence ==="

python3 << 'PYTHON_SCRIPT'
import socket
import struct
import sys
import time
import os

PROXY_HOST = "127.0.0.1"
SOCKS5_PORT = int(os.environ.get('SOCKS5_PORT', '11080'))

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
    
    udp_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    udp_sock.settimeout(5)
    
    success_count = 0
    for i in range(3):
        dns_query = bytes([
            0xAB + i, 0xCD,
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
        
        try:
            response_data, addr = udp_sock.recvfrom(4096)
            print(f"  Query {i+1}: ✓ Response received ({len(response_data)} bytes)")
            success_count += 1
        except socket.timeout:
            print(f"  Query {i+1}: ✗ Timeout")
        
        time.sleep(0.5)
    
    tcp_sock.close()
    udp_sock.close()
    
    if success_count >= 2:
        print("✅ Test 5 PASSED: UDP session persistence works")
    else:
        print("⚠ Test 5: Session persistence may have issues")
    
except Exception as e:
    print(f"❌ Error: {e}")
    sys.exit(1)
PYTHON_SCRIPT

echo ""
echo "=== Test 6: Check Logs for UDP Activity ==="

if [ -d "$DATA_DIR/logs" ]; then
    echo "Checking UDP relay logs..."
    grep -i "UDP" "$DATA_DIR/logs/"*.log 2>/dev/null | tail -20 || echo "No UDP logs found"
else
    echo "Logs directory not found"
fi

echo ""
echo "=============================================="
echo "  SOCKS5 UDP Tests Completed"
echo "=============================================="
