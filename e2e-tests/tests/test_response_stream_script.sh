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
    --skip-cert-check --unsafe-ssl --intercept --no-system-proxy --rules-file "$RULES_FILE" \
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
import subprocess
import sys
import time

proxy_port = int(sys.argv[1])
limit = 16 * 1024 * 1024


def raw_response(host, path, timeout=10):
    connection = http.client.HTTPConnection("127.0.0.1", proxy_port, timeout=timeout)
    connection.request("GET", f"http://{host}{path}", headers={"Host": host})
    result = connection.getresponse()
    return connection, result


def secure_request(host, path, timeout=10):
    marker = b"\n__BIFROST_STATUS__:"
    completed = subprocess.run(
        [
            "curl",
            "-ksS",
            "--http1.1",
            "--max-time",
            str(timeout),
            "--proxy",
            f"http://127.0.0.1:{proxy_port}",
            "--write-out",
            "\n__BIFROST_STATUS__:%{http_code}\n",
            f"https://{host}{path}",
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    assert marker in completed.stdout, (host, completed.returncode, completed.stderr)
    body, status_text = completed.stdout.rsplit(marker, 1)
    status = int(status_text.strip())
    assert completed.returncode == 0, (host, status, completed.stderr, body[:1024])
    return status, body


def response(host, path, timeout=10):
    connection, result = raw_response(host, path, timeout=timeout)
    assert result.status == 200, (result.status, result.read(1024))
    assert result.getheader("Content-Length") is None
    assert result.getheader("Content-Type").startswith("text/event-stream")
    return connection, result


def expect_bad_gateway(host, path, expected, secure=False):
    if secure:
        status, body = secure_request(host, path)
        assert status == 502, (host, status, body)
        assert expected in body, (host, expected, body)
        return

    connection, result = raw_response(host, path)
    body = result.read(16 * 1024)
    assert result.status == 502, (host, result.status, body)
    assert expected in body, (host, expected, body)
    connection.close()


def expect_sse_error(host, path, expected):
    connection, result = response(host, path)
    body = result.read(64 * 1024)
    assert b"event: error" in body, (host, body)
    assert expected in body, (host, expected, body)
    connection.close()


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


# HTTPS intercepted traffic follows the tunnel implementation and preserves
# transformed events plus the onEnd output.
tunnel_status, tunnel_body = secure_request("sse-tunnel.local", "/stream")
assert tunnel_status == 200, (tunnel_status, tunnel_body)
assert b"data: first\n\n" in tunnel_body, tunnel_body
assert b"data: second\n\n" in tunnel_body, tunnel_body
assert b"event: done\ndata: end\n\n" in tunnel_body, tunnel_body


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


# An upstream close without an SSE delimiter still flushes the final event and
# then emits the stream onEnd result.
conn, stream = response("sse-tail.local", "/tail")
assert stream.readline() == b"data: tail\n"
assert stream.readline() == b"\n"
assert stream.readline() == b"event: done\n"
assert stream.readline() == b"data: end\n"
assert stream.readline() == b"\n"
conn.close()


# Callback failures after response commit are serialized as SSE errors.
expect_sse_error("sse-event-error.local", "/stream", b"bifrost_stream_script_error")
conn, stream = response("sse-tail-end-error.local", "/tail")
assert stream.readline() == b"data: tail\n"
assert stream.readline() == b"\n"
tail_error = stream.read(64 * 1024)
assert b"event: error" in tail_error, tail_error
assert b"bifrost_stream_script_error" in tail_error, tail_error
conn.close()
expect_sse_error("sse-direct-next-error.local", "/no-upstream", b"bifrost_stream_script_error")


# HTTP response validation and initialization errors are explicit 502s.
expect_bad_gateway(
    "sse-conflict.local",
    "/stream",
    b"resScript and resStreamScript cannot be combined",
)
expect_bad_gateway(
    "sse-json.local",
    "/json",
    b"resStreamScript requires a text/event-stream response",
)
expect_bad_gateway(
    "sse-encoded.local",
    "/encoded",
    b"resStreamScript does not support encoded upstream SSE responses",
)
expect_bad_gateway(
    "sse-init-error.local",
    "/stream",
    b"stream script initialization failed",
)
expect_bad_gateway(
    "sse-multiple.local",
    "/stream",
    b"currently requires exactly one script",
)
expect_bad_gateway(
    "sse-direct-conflict.local",
    "/no-upstream",
    b"resScript and resStreamScript cannot be combined",
)
expect_bad_gateway(
    "sse-direct-transform.local",
    "/no-upstream",
    b"direct status resStreamScript requires stream.mode",
)
expect_bad_gateway(
    "sse-direct-init-error.local",
    "/no-upstream",
    b"stream script initialization failed",
)


# The same validation and initialization failures are enforced on intercepted
# HTTPS traffic by the tunnel implementation.
expect_bad_gateway(
    "sse-tunnel-conflict.local",
    "/stream",
    b"resScript and resStreamScript cannot be combined",
    secure=True,
)
expect_bad_gateway(
    "sse-tunnel-json.local",
    "/json",
    b"resStreamScript requires a text/event-stream response",
    secure=True,
)
expect_bad_gateway(
    "sse-tunnel-encoded.local",
    "/encoded",
    b"resStreamScript does not support encoded upstream SSE responses",
    secure=True,
)
expect_bad_gateway(
    "sse-tunnel-init-error.local",
    "/stream",
    b"stream script initialization failed",
    secure=True,
)


# Inline request and response scripts execute with the live config and apply
# every mutation class surfaced by ScriptManager.
conn, inline = raw_response("inline-script.local", "/json")
inline_body = inline.read()
assert inline.status == 202, (inline.status, inline_body)
assert inline.reason == "Accepted", inline.reason
assert inline.getheader("x-inline-response") == "applied", inline.getheaders()
assert inline_body == b"inline-response-body", inline_body
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
