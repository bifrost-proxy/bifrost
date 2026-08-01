#!/bin/bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT BIFROST_DISABLE_TRAY

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
mkdir -p "$ROOT_DIR/.bifrost-e2e-runs"
TEST_ROOT="$(mktemp -d "$ROOT_DIR/.bifrost-e2e-runs/ws-rejection-XXXXXX")"
export BIFROST_DATA_DIR="$TEST_ROOT/data"
source "$SCRIPT_DIR/../test_utils/process.sh"
source "$SCRIPT_DIR/../test_utils/rule_fixture.sh"
mark_e2e_data_root "$TEST_ROOT"

BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/debug/bifrost}"
PROXY_PORT="${PROXY_PORT:-$(python3 - <<'PY'
import socket
with socket.socket() as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
)}"
UPSTREAM_PORT="${UPSTREAM_PORT:-$(python3 - <<'PY'
import socket
with socket.socket() as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
)}"

mkdir -p "$BIFROST_DATA_DIR"
RULE_FILE="$TEST_ROOT/ws-rejection.txt"
PROXY_LOG="$TEST_ROOT/proxy.log"
UPSTREAM_LOG="$TEST_ROOT/upstream.log"
PROXY_PID=""
UPSTREAM_PID=""

cleanup() {
    if [[ -n "$PROXY_PID" ]]; then
        safe_cleanup_proxy "$PROXY_PID"
    fi
    if [[ -n "$UPSTREAM_PID" ]]; then
        kill_pid "$UPSTREAM_PID"
        wait_pid "$UPSTREAM_PID"
    fi
    if [[ "${KEEP_E2E_ARTIFACTS:-0}" == "1" ]]; then
        echo "preserved E2E artifacts at $TEST_ROOT"
    else
        rm -rf "$TEST_ROOT"
    fi
}
trap cleanup EXIT

if [[ ! -x "$BIFROST_BIN" ]]; then
    cargo build --manifest-path "$ROOT_DIR/Cargo.toml" --bin bifrost
fi

render_rule_fixture_to_file \
    "$ROOT_DIR/e2e-tests/test_data/websocket_upstream_rejection.txt" \
    "$RULE_FILE" \
    "UPSTREAM_PORT=$UPSTREAM_PORT"

python3 "$ROOT_DIR/e2e-tests/mock_servers/ws_reject_server.py" \
    --port "$UPSTREAM_PORT" >"$UPSTREAM_LOG" 2>&1 &
UPSTREAM_PID=$!

for _ in $(seq 1 40); do
    if python3 - "$UPSTREAM_PORT" <<'PY' >/dev/null 2>&1
import socket
import sys
with socket.create_connection(("127.0.0.1", int(sys.argv[1])), timeout=1):
    pass
PY
    then
        break
    fi
    sleep 0.1
done

"$BIFROST_BIN" -p "$PROXY_PORT" start \
    --skip-cert-check --unsafe-ssl --no-system-proxy \
    --rules-file "$RULE_FILE" >"$PROXY_LOG" 2>&1 &
PROXY_PID=$!

for _ in $(seq 1 60); do
    if curl -sf "http://127.0.0.1:$PROXY_PORT/_bifrost/api/system" >/dev/null 2>&1; then
        break
    fi
    if ! kill -0 "$PROXY_PID" 2>/dev/null; then
        tail -n 120 "$PROXY_LOG"
        exit 1
    fi
    sleep 0.5
done
curl -sf "http://127.0.0.1:$PROXY_PORT/_bifrost/api/system" >/dev/null

python3 - "$PROXY_PORT" <<'PY'
import socket
import sys

proxy_port = int(sys.argv[1])
request = (
    "GET /ws HTTP/1.1\r\n"
    "Host: ws-reject.test\r\n"
    "Connection: Upgrade\r\n"
    "Upgrade: websocket\r\n"
    "Sec-WebSocket-Version: 13\r\n"
    "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
).encode()

for _ in range(6):
    with socket.create_connection(("127.0.0.1", proxy_port), timeout=5) as sock:
        sock.sendall(request)
        response = b""
        while b"Bad Gateway" not in response:
            try:
                chunk = sock.recv(4096)
            except TimeoutError as error:
                raise SystemExit(f"timed out waiting for proxy response: {response!r}") from error
            if not chunk:
                break
            response += chunk
            if len(response) > 64 * 1024:
                raise SystemExit("proxy response exceeded 64KiB")
    status_line = response.split(b"\r\n", 1)[0]
    if b" 502 " not in status_line:
        raise SystemExit(f"expected 502, got {status_line!r}")
    if b"Bad Gateway" not in response:
        raise SystemExit(f"expected compatibility body, got {response!r}")
PY

sleep 1
warning_count="$(
    { grep -h "upstream rejected WebSocket handshake" \
        "$PROXY_LOG" "$BIFROST_DATA_DIR"/logs/*.log 2>/dev/null || true; } \
        | wc -l | tr -d ' '
)"
if [[ "$warning_count" -ne 1 ]]; then
    echo "expected exactly one rate-limited warning, got $warning_count"
    tail -n 160 "$PROXY_LOG" "$BIFROST_DATA_DIR"/logs/*.log 2>/dev/null || true
    exit 1
fi
if ! grep -h -q "error_category=.*upstream_handshake_rejected" \
    "$PROXY_LOG" "$BIFROST_DATA_DIR"/logs/*.log 2>/dev/null; then
    echo "missing structured upstream_handshake_rejected category"
    tail -n 160 "$PROXY_LOG" "$BIFROST_DATA_DIR"/logs/*.log 2>/dev/null || true
    exit 1
fi
if grep -h -q "HTTP proxy error:.*WebSocket handshake failed" \
    "$PROXY_LOG" "$BIFROST_DATA_DIR"/logs/*.log 2>/dev/null; then
    echo "upstream handshake rejection leaked into the generic HTTP network error log"
    tail -n 160 "$PROXY_LOG" "$BIFROST_DATA_DIR"/logs/*.log 2>/dev/null || true
    exit 1
fi

echo "PASS: six upstream WebSocket rejections preserved 502 behavior and emitted one structured warning"
