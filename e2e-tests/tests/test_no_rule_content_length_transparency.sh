#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

source "$PROJECT_DIR/e2e-tests/test_utils/assert.sh"
source "$PROJECT_DIR/e2e-tests/test_utils/process.sh"

pick_free_port() {
    python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

PROXY_HOST="127.0.0.1"
PROXY_PORT="${PROXY_PORT:-$(pick_free_port)}"
ORIGIN_PORT="${ORIGIN_PORT:-$(pick_free_port)}"
TEST_ROOT="${TEST_ROOT:-/tmp/bifrost-e2e-no-rule-content-length}"
BIFROST_BIN="${BIFROST_BIN:-${PROJECT_DIR}/target/debug/bifrost}"
MARKER="transparent-content-length-$(date +%s)-$$"
EXPECTED_BODY="{\"marker\":\"${MARKER}\",\"status\":\"ok\"}"
EXPECTED_LEN="${#EXPECTED_BODY}"

PROXY_PID=""
ORIGIN_PID=""

cleanup() {
    if [[ -n "${PROXY_PID:-}" ]]; then
        kill "$PROXY_PID" 2>/dev/null || true
        wait "$PROXY_PID" 2>/dev/null || true
    fi
    if [[ -n "${ORIGIN_PID:-}" ]]; then
        kill "$ORIGIN_PID" 2>/dev/null || true
        wait "$ORIGIN_PID" 2>/dev/null || true
    fi
    kill_bifrost_on_port "$PROXY_PORT"
    rm -rf "$TEST_ROOT"
}
trap cleanup EXIT

wait_for_http() {
    local url="$1"
    local log_file="$2"
    local pid="$3"
    for _ in {1..120}; do
        if env NO_PROXY="*" no_proxy="*" curl -sS --max-time 2 "$url" >/dev/null 2>&1; then
            return 0
        fi
        if ! kill -0 "$pid" >/dev/null 2>&1; then
            echo "process exited early while waiting for ${url}" >&2
            tail -80 "$log_file" >&2 || true
            return 1
        fi
        sleep 0.2
    done
    echo "timeout waiting for ${url}" >&2
    tail -80 "$log_file" >&2 || true
    return 1
}

start_origin() {
    python3 - "$ORIGIN_PORT" "$EXPECTED_BODY" <<'PY' >"$TEST_ROOT/origin.log" 2>&1 &
import http.server
import sys

port = int(sys.argv[1])
body = sys.argv[2].encode("utf-8")

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/health":
            self.send_response(200)
            self.send_header("Content-Length", "2")
            self.end_headers()
            self.wfile.write(b"ok")
            return
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, fmt, *args):
        return

server = http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler)
server.serve_forever()
PY
    ORIGIN_PID=$!
    wait_for_http "http://127.0.0.1:${ORIGIN_PORT}/health" "$TEST_ROOT/origin.log" "$ORIGIN_PID"
}

start_proxy() {
    if [[ "${SKIP_BUILD:-false}" != "true" || ! -x "$BIFROST_BIN" ]]; then
        CARGO_INCREMENTAL=0 cargo build --bin bifrost
    fi

    BIFROST_DATA_DIR="$TEST_ROOT/data" \
        "$BIFROST_BIN" start \
        --host "$PROXY_HOST" \
        -p "$PROXY_PORT" \
        --skip-cert-check \
        --unsafe-ssl \
        --no-system-proxy \
        >"$TEST_ROOT/bifrost.log" 2>&1 &
    PROXY_PID=$!

    wait_for_http "http://${PROXY_HOST}:${PROXY_PORT}/_bifrost/api/proxy/address" \
        "$TEST_ROOT/bifrost.log" \
        "$PROXY_PID"
}

request_via_proxy() {
    local headers_file="$TEST_ROOT/response.headers"
    local body_file="$TEST_ROOT/response.body"
    local url="http://127.0.0.1:${ORIGIN_PORT}/${MARKER}"

    HTTP_STATUS="$(NO_PROXY="" no_proxy="" curl -sS --noproxy "" --max-time 10 \
        --proxy "http://${PROXY_HOST}:${PROXY_PORT}" \
        -D "$headers_file" \
        -o "$body_file" \
        -w '%{http_code}' \
        "$url")"
    HTTP_HEADERS="$(tr -d '\r' < "$headers_file")"
    HTTP_BODY="$(cat "$body_file")"
}

traffic_detail_json() {
    python3 - "$PROXY_HOST" "$PROXY_PORT" "$MARKER" <<'PY'
import json
import sys
import time
import urllib.request

host, port, marker = sys.argv[1], sys.argv[2], sys.argv[3]
base = f"http://{host}:{port}/_bifrost/api/traffic"

for _ in range(60):
    data = json.load(urllib.request.urlopen(f"{base}?limit=100", timeout=2))
    for record in data.get("records", []):
        record_text = json.dumps(record)
        if marker in record_text:
            detail = json.load(urllib.request.urlopen(f"{base}/{record['id']}", timeout=2))
            print(json.dumps(detail))
            raise SystemExit(0)
    time.sleep(0.2)

raise SystemExit(f"traffic record for {marker} not found")
PY
}

header_value_json() {
    python3 - "$1" "$2" "$3" <<'PY'
import json
import sys

detail = json.loads(sys.argv[1])
field = sys.argv[2]
header = sys.argv[3].lower()
for key, value in detail.get(field) or []:
    if key.lower() == header:
        print(value)
        raise SystemExit(0)
raise SystemExit(1)
PY
}

response_headers_state() {
    python3 - "$1" "$EXPECTED_LEN" <<'PY'
import json
import sys

detail = json.loads(sys.argv[1])
expected = sys.argv[2]
headers = detail.get("response_headers")
if headers is None:
    print("absent")
    raise SystemExit(0)
for key, value in headers:
    if key.lower() == "content-length" and value == expected:
        print("contains_expected_length")
        raise SystemExit(0)
print("missing_expected_length")
raise SystemExit(0)
PY
}

main() {
    rm -rf "$TEST_ROOT"
    mkdir -p "$TEST_ROOT"

    start_origin
    start_proxy
    request_via_proxy

    assert_status "200" "$HTTP_STATUS" "no-rule proxied request should succeed"
    assert_body_equals "$EXPECTED_BODY" "$HTTP_BODY" "no-rule proxied response body should remain unchanged"
    assert_header_value "Content-Length" "$EXPECTED_LEN" "$HTTP_HEADERS" "client response should preserve upstream Content-Length"

    local detail original_len final_state has_rule_hit
    detail="$(traffic_detail_json)"
    has_rule_hit="$(python3 -c 'import json,sys; print(str(json.loads(sys.stdin.read()).get("has_rule_hit")).lower())' <<<"$detail")"
    assert_body_equals "false" "$has_rule_hit" "traffic detail should remain marked as no rule hit"

    original_len="$(header_value_json "$detail" "original_response_headers" "content-length")"
    final_state="$(response_headers_state "$detail")"
    assert_body_equals "$EXPECTED_LEN" "$original_len" "traffic original response headers should record upstream Content-Length"
    if [[ "$final_state" == "absent" ]]; then
        _log_pass "traffic detail should not store modified response headers when final headers match upstream"
    else
        assert_body_equals "contains_expected_length" "$final_state" "traffic final response headers should preserve Content-Length when stored"
    fi

    print_test_summary
}

main "$@"
