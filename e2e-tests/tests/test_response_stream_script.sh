#!/bin/bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
TEST_DATA_DIR="$(mktemp -d "$ROOT_DIR/.bifrost-e2e-response-stream.XXXXXX")"
export BIFROST_DATA_DIR="$TEST_DATA_DIR"

source "$SCRIPT_DIR/../test_utils/process.sh"

PROXY_PORT="${PROXY_PORT:-$((19400 + ($$ % 200)))}"
UPSTREAM_PORT="${UPSTREAM_PORT:-$((19600 + ($$ % 200)))}"
BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/release/bifrost}"
RULES_TEMPLATE="$ROOT_DIR/e2e-tests/rules/response_stream_script/true_sse.txt"
RULES_FILE="$TEST_DATA_DIR/rules.txt"
PROXY_PID=""
UPSTREAM_PID=""

cleanup() {
    safe_cleanup_proxy "$PROXY_PID"
    kill_bifrost_on_port "$PROXY_PORT"
    if [[ -n "$UPSTREAM_PID" ]]; then
        kill "$UPSTREAM_PID" 2>/dev/null || true
        wait "$UPSTREAM_PID" 2>/dev/null || true
    fi
    if command -v lsof >/dev/null 2>&1; then
        for _ in $(seq 1 40); do
            if ! lsof "$TEST_DATA_DIR/.system_proxy.lock" >/dev/null 2>&1; then
                break
            fi
            sleep 0.2
        done
    fi
    if [[ "${KEEP_TEST_DATA:-0}" == "1" ]]; then
        echo "Preserved test data: $TEST_DATA_DIR" >&2
    else
        rm -rf "$TEST_DATA_DIR"
    fi
}
trap cleanup EXIT

if [[ ! -x "$BIFROST_BIN" ]]; then
    cargo build --release --bin bifrost
fi

sed "s/__SSE_UPSTREAM_PORT__/${UPSTREAM_PORT}/g" "$RULES_TEMPLATE" >"$RULES_FILE"

python3 "$ROOT_DIR/e2e-tests/mock_servers/sse_stream_server.py" \
    --port "$UPSTREAM_PORT" >"$TEST_DATA_DIR/upstream.log" 2>&1 &
UPSTREAM_PID=$!

for _ in $(seq 1 50); do
    if curl -fsS --max-time 1 "http://127.0.0.1:${UPSTREAM_PORT}/ready" >/dev/null 2>&1; then
        break
    fi
    sleep 0.1
done
kill -0 "$UPSTREAM_PID"

BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" --port "$PROXY_PORT" start \
    --skip-cert-check --unsafe-ssl --no-system-proxy --rules-file "$RULES_FILE" \
    >"$TEST_DATA_DIR/proxy.log" 2>&1 &
PROXY_PID=$!

for _ in $(seq 1 100); do
    if curl -fsS "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/proxy/address" >/dev/null 2>&1; then
        break
    fi
    if ! kill -0 "$PROXY_PID" 2>/dev/null; then
        tail -n 200 "$TEST_DATA_DIR/proxy.log" >&2 || true
        exit 1
    fi
    sleep 0.1
done

python3 - "$PROXY_PORT" <<'PY'
import hashlib
import http.client
import sys
import time

proxy_port = int(sys.argv[1])
limit = 16 * 1024 * 1024


def response(host, path, timeout=10):
    connection = http.client.HTTPConnection("127.0.0.1", proxy_port, timeout=timeout)
    connection.request("GET", f"http://{host}{path}", headers={"Host": host})
    result = connection.getresponse()
    assert result.status == 200, (result.status, result.read(1024))
    assert result.getheader("Content-Length") is None
    assert result.getheader("Content-Type").startswith("text/event-stream")
    return connection, result


# Both transformed events must arrive well before the upstream's 3-second EOF hold.
conn, stream = response("sse-transform.local", "/stream")
started = time.monotonic()
first = stream.readline()
stream.readline()
first_at = time.monotonic() - started
second = stream.readline()
stream.readline()
second_at = time.monotonic() - started
assert first == b"data: first\n", first
assert second == b"data: second\n", second
assert first_at < 1.0, first_at
assert second_at < 1.5, second_at
conn.close()


# Mock events are produced one callback at a time, not buffered and replayed at EOF.
conn, stream = response("sse-mock.local", "/stream")
times = []
values = []
started = time.monotonic()
for _ in range(3):
    event_line = stream.readline()
    data_line = stream.readline()
    stream.readline()
    times.append(time.monotonic() - started)
    values.append((event_line, data_line))
assert values == [
    (b"event: mock\n", b"data: 1\n"),
    (b"event: mock\n", b"data: 2\n"),
    (b"event: mock\n", b"data: 3\n"),
], values
assert times[0] < 0.5, times
assert times[1] - times[0] >= 0.18, times
assert times[2] - times[1] >= 0.18, times
conn.close()


# Direct-status mock streaming must not require or contact an upstream server.
conn, stream = response("sse-direct-mock.local", "/no-upstream")
direct_values = []
for _ in range(3):
    event_line = stream.readline()
    data_line = stream.readline()
    stream.readline()
    direct_values.append((event_line, data_line))
assert direct_values == [
    (b"event: mock\n", b"data: 1\n"),
    (b"event: mock\n", b"data: 2\n"),
    (b"event: mock\n", b"data: 3\n"),
], direct_values
conn.close()


# A near-limit event is preserved byte-for-byte despite arbitrary upstream frames.
conn, stream = response("sse-transform.local", "/large", timeout=60)
large_line = stream.readline(limit + 64)
assert large_line.startswith(b"data: ") and large_line.endswith(b"\n")
payload = large_line[6:-1]
assert len(payload) == limit - 4096, len(payload)
expected = hashlib.sha256(b"x" * (limit - 4096)).digest()
assert hashlib.sha256(payload).digest() == expected
assert stream.readline() == b"\n"
conn.close()


# Oversize input never leaks a partial data event; it terminates with an SSE error.
conn, stream = response("sse-transform.local", "/oversize", timeout=60)
error_body = stream.read(64 * 1024)
assert b"event: error" in error_body, error_body[:500]
assert b"exceed" in error_body.lower(), error_body[:500]
assert b"data: zzzz" not in error_body, error_body[:500]
conn.close()

print("response stream script E2E passed")
PY
