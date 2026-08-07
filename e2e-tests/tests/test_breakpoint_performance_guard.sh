#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
source "$SCRIPT_DIR/../test_utils/admin_client.sh"

ADMIN_HOST="${ADMIN_HOST:-127.0.0.1}"
ADMIN_PORT="${ADMIN_PORT:-$(allocate_free_port)}"
ADMIN_PATH_PREFIX="${ADMIN_PATH_PREFIX:-/_bifrost}"
export ADMIN_HOST ADMIN_PORT ADMIN_PATH_PREFIX

MOCK_PORT="${MOCK_PORT:-$(allocate_free_port)}"
TLS_PORT="${TLS_PORT:-8443}"
WS_READY_TIMEOUT="${BIFROST_E2E_WS_READY_TIMEOUT:-20}"
TMP_DIR="$(mktemp -d)"
MOCK_PID=""
TLS_PID=""
WS_PID=""
CURL_PID=""

log() {
    echo "[breakpoint-perf-e2e] $*"
}

fail() {
    echo "[breakpoint-perf-e2e][FAIL] $*" >&2
    if [[ -n "${ADMIN_CLIENT_BIFROST_LOG_FILE:-}" && -f "$ADMIN_CLIENT_BIFROST_LOG_FILE" ]]; then
        echo "[breakpoint-perf-e2e][FAIL] bifrost log tail:" >&2
        tail -120 "$ADMIN_CLIENT_BIFROST_LOG_FILE" >&2 || true
    fi
    exit 1
}

cleanup() {
    if [[ -n "${CURL_PID:-}" ]] && kill -0 "$CURL_PID" 2>/dev/null; then
        kill "$CURL_PID" 2>/dev/null || true
    fi
    if [[ -n "${WS_PID:-}" ]] && kill -0 "$WS_PID" 2>/dev/null; then
        kill "$WS_PID" 2>/dev/null || true
    fi
    if [[ -n "${MOCK_PID:-}" ]] && kill -0 "$MOCK_PID" 2>/dev/null; then
        kill "$MOCK_PID" 2>/dev/null || true
    fi
    if [[ -n "${TLS_PID:-}" ]] && kill -0 "$TLS_PID" 2>/dev/null; then
        kill "$TLS_PID" 2>/dev/null || true
    fi
    admin_cleanup_bifrost
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT

wait_for_file() {
    local file="$1"
    local timeout_secs="$2"
    local start
    start="$(date +%s)"
    while [[ ! -s "$file" ]]; do
        if (( $(date +%s) - start >= timeout_secs )); then
            return 1
        fi
        sleep 0.1
    done
}

settings_url() {
    echo "$(admin_base_url)/api/breakpoint/settings"
}

performance_url() {
    echo "$(admin_base_url)/api/config/performance"
}

resume_url() {
    echo "$(admin_base_url)/api/breakpoint/resume"
}

pending_url() {
    echo "$(admin_base_url)/api/breakpoint/pending"
}

rules_url() {
    echo "$(admin_base_url)/api/rules"
}

create_rule() {
    local name="$1"
    local content="$2"
    local payload
    payload="$(jq -cn --arg name "$name" --arg content "$content" '{name:$name,content:$content,enabled:true}')"
    curl -fsS -X POST "$(rules_url)" \
        -H 'Content-Type: application/json' \
        --data "$payload" >/dev/null
}

start_mock_server() {
    local script="$TMP_DIR/mock_server.py"
    cat >"$script" <<'PY'
import hashlib
import json
import ssl
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def _send(self, status, body, content_type="application/json"):
        data = body if isinstance(body, bytes) else body.encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length)
        payload = {
            "method": self.command,
            "path": self.path,
            "len": len(body),
            "sha256": hashlib.sha256(body).hexdigest(),
            "prefix": body[:16].decode("utf-8", "replace"),
            "breakpoint_header": self.headers.get("X-Breakpoint-Request", ""),
        }
        self._send(200, json.dumps(payload))

    def do_PUT(self):
        self.do_POST()

    def do_GET(self):
        self._send(200, b"hello-breakpoint-timeout", "text/plain")

    def log_message(self, *_args):
        return

port = int(sys.argv[1])
server = ThreadingHTTPServer(("127.0.0.1", port), Handler)
if len(sys.argv) == 4:
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(sys.argv[2], sys.argv[3])
    server.socket = context.wrap_socket(server.socket, server_side=True)
server.serve_forever()
PY
    python3 "$script" "$MOCK_PORT" >"$TMP_DIR/mock.log" 2>&1 &
    MOCK_PID=$!
    for _ in $(seq 1 50); do
        if curl -fsS "http://127.0.0.1:$MOCK_PORT/ready" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.1
    done
    fail "mock server did not start; log=$(cat "$TMP_DIR/mock.log" 2>/dev/null || true)"
}

start_tls_mock_server() {
    # Git Bash rewrites values beginning with `/` as Windows paths. Excluding
    # the certificate subject from MSYS argument conversion keeps the command
    # portable while remaining a no-op on Unix shells.
    if ! MSYS2_ARG_CONV_EXCL='/CN=127.0.0.1' openssl req \
        -x509 -newkey rsa:2048 -nodes -days 1 \
        -subj '/CN=127.0.0.1' \
        -keyout "$TMP_DIR/tls.key" -out "$TMP_DIR/tls.crt" \
        >"$TMP_DIR/openssl.log" 2>&1; then
        fail "TLS certificate generation failed; log=$(cat "$TMP_DIR/openssl.log" 2>/dev/null || true)"
    fi
    python3 "$TMP_DIR/mock_server.py" "$TLS_PORT" "$TMP_DIR/tls.crt" "$TMP_DIR/tls.key" \
        >"$TMP_DIR/tls-mock.log" 2>&1 &
    TLS_PID=$!
    for _ in $(seq 1 50); do
        if curl -kfsS "https://127.0.0.1:$TLS_PORT/ready" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.1
    done
    fail "TLS mock server did not start; log=$(cat "$TMP_DIR/tls-mock.log" 2>/dev/null || true)"
}

write_ws_probe() {
    cat >"$TMP_DIR/wait_breakpoint_push.js" <<'JS'
const fs = require("fs");

const [url, readyFile, eventFile, expectedCountArg] = process.argv.slice(2);
const expectedCount = Math.max(1, Number(expectedCountArg || "1"));
const events = [];
if (typeof globalThis.WebSocket !== "function") {
  console.error(`built-in WebSocket is unavailable in ${process.version}`);
  process.exit(1);
}
const ws = new WebSocket(url);
const timer = setTimeout(() => {
  console.error("timeout waiting for breakpoint_paused");
  try { ws.close(); } catch {}
  process.exit(2);
}, 10000);

ws.addEventListener("open", () => {
  fs.writeFileSync(readyFile, "ready");
});

ws.addEventListener("message", (event) => {
  let msg;
  try {
    msg = JSON.parse(String(event.data));
  } catch {
    return;
  }
  if (msg.type === "breakpoint_paused") {
    events.push(msg.data);
    fs.writeFileSync(eventFile, expectedCount === 1 ? JSON.stringify(msg.data) : JSON.stringify(events));
    if (events.length >= expectedCount) {
      clearTimeout(timer);
      ws.close();
      process.exit(0);
    }
  }
});

ws.addEventListener("error", (event) => {
  clearTimeout(timer);
  console.error(event.error?.message || event.message || "WebSocket connection failed");
  process.exit(1);
});
JS
}

start_breakpoint_probe() {
    local ready_file="$1"
    local event_file="$2"
    local label="$3"
    local expected_count="${4:-1}"
    local probe_log="$TMP_DIR/ws-${label}.log"

    node "$TMP_DIR/wait_breakpoint_push.js" \
        "ws://127.0.0.1:$ADMIN_PORT${ADMIN_PATH_PREFIX}/api/push" \
        "$ready_file" "$event_file" "$expected_count" \
        >"$probe_log" 2>&1 &
    WS_PID=$!

    if wait_for_file "$ready_file" "$WS_READY_TIMEOUT"; then
        return 0
    fi

    local probe_state="running"
    if ! kill -0 "$WS_PID" 2>/dev/null; then
        set +e
        wait "$WS_PID"
        local probe_status=$?
        set -e
        WS_PID=""
        probe_state="exited with status ${probe_status}"
    fi
    fail "${label} push websocket did not become ready within ${WS_READY_TIMEOUT}s; node=${probe_state}; log=$(cat "$probe_log" 2>/dev/null || true)"
}

assert_json_value() {
    local json="$1"
    local expr="$2"
    local expected="$3"
    local actual
    actual="$(echo "$json" | jq -r "$expr")"
    [[ "$actual" == "$expected" ]] || fail "expected $expr to be '$expected', got '$actual' in $json"
}

assert_cors_origin() {
    local headers_file="$1"
    local expected_origin="$2"
    local actual
    actual="$(tr -d '\r' <"$headers_file" | awk -F': ' 'tolower($1) == "access-control-allow-origin" { print $2 }' | tail -1)"
    [[ "$actual" == "$expected_origin" ]] || fail "expected CORS origin '$expected_origin', got '$actual': $(cat "$headers_file")"
}

resolve_bifrost_bin() {
    local candidate="${BIFROST_BIN:-}"
    if [[ -n "$candidate" && -x "$candidate" ]]; then
        echo "$candidate"
        return 0
    fi

    for candidate in \
        "$REPO_DIR/target/release/bifrost" \
        "$REPO_DIR/target/release/bifrost.exe" \
        "$REPO_DIR/target/debug/bifrost" \
        "$REPO_DIR/target/debug/bifrost.exe"; do
        if [[ -x "$candidate" ]]; then
            echo "$candidate"
            return 0
        fi
    done

    return 1
}

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
    BIFROST_BIN="$(resolve_bifrost_bin || true)"
    [[ -n "$BIFROST_BIN" ]] || fail "SKIP_BUILD=true but no executable bifrost binary was found"
    log "skipping build, using $BIFROST_BIN"
else
    log "building bifrost debug binary"
    TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_DIR/target}"
    SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
    BIFROST_BIN="$TARGET_DIR/debug/bifrost"
    if [[ ! -x "$BIFROST_BIN" && -x "${BIFROST_BIN}.exe" ]]; then
        BIFROST_BIN="${BIFROST_BIN}.exe"
    fi
fi
export BIFROST_BIN

start_mock_server
start_tls_mock_server
admin_ensure_bifrost
write_ws_probe

log "verify default breakpoint settings are safe"
settings="$(curl -fsS "$(settings_url)")"
assert_json_value "$settings" ".enabled" "false"
assert_json_value "$settings" ".max_body_bytes" "1048576"
performance="$(curl -fsS "$(performance_url)")"
assert_json_value "$performance" ".breakpoint.timeout_ms" "30000"
assert_json_value "$performance" ".breakpoint.timeout_min_ms" "5000"
assert_json_value "$performance" ".breakpoint.timeout_max_ms" "300000"

log "Breakpoint APIs support browser and desktop CORS"
for origin in "http://localhost:3000" "tauri://localhost"; do
    cors_headers="$TMP_DIR/cors-$(echo "$origin" | tr -c 'A-Za-z0-9' '_').headers"
    curl -sS -o /dev/null -D "$cors_headers" -X OPTIONS "$(pending_url)" \
        -H "Origin: $origin" \
        -H 'Access-Control-Request-Method: GET' \
        -H 'Access-Control-Request-Headers: X-Client-Id' \
        -w '%{http_code}' | grep -q '^204$' || fail "pending CORS preflight failed for $origin"
    assert_cors_origin "$cors_headers" "$origin"
    curl -fsS -o "$TMP_DIR/cors-pending.json" -D "$cors_headers" "$(pending_url)" \
        -H "Origin: $origin"
    assert_cors_origin "$cors_headers" "$origin"

    curl -sS -o /dev/null -D "$cors_headers" -X OPTIONS "$(resume_url)" \
        -H "Origin: $origin" \
        -H 'Access-Control-Request-Method: POST' \
        -H 'Access-Control-Request-Headers: Content-Type, X-Client-Id, X-Bifrost-CSRF' \
        -w '%{http_code}' | grep -q '^204$' || fail "resume CORS preflight failed for $origin"
    assert_cors_origin "$cors_headers" "$origin"
    tr -d '\r' <"$cors_headers" | grep -qi '^Access-Control-Allow-Headers:.*X-Bifrost-CSRF' \
        || fail "resume CORS preflight omitted X-Bifrost-CSRF for $origin"
done

log "default-off large body request completes without breakpoint pause"
BODY_2M="$TMP_DIR/body-2m.bin"
python3 - "$BODY_2M" <<'PY'
import pathlib, sys
pathlib.Path(sys.argv[1]).write_bytes(b"a" * (2 * 1024 * 1024))
PY
large_response="$(curl --noproxy "" -fsS -x "http://127.0.0.1:$ADMIN_PORT" --data-binary @"$BODY_2M" "http://127.0.0.1:$MOCK_PORT/echo")"
assert_json_value "$large_response" ".len" "2097152"

log "global request breakpoint switch without breakpoint rule does not pause traffic"
curl -fsS -X POST "$(settings_url)" \
    -H 'Content-Type: application/json' \
    --data '{"enabled":true,"max_body_bytes":1048576}' >/dev/null

NO_RULE_READY_FILE="$TMP_DIR/ws-no-rule.ready"
NO_RULE_EVENT_FILE="$TMP_DIR/breakpoint-no-rule-event.json"
start_breakpoint_probe "$NO_RULE_READY_FILE" "$NO_RULE_EVENT_FILE" "no-rule"
no_rule_response="$(curl --noproxy "" -fsS -x "http://127.0.0.1:$ADMIN_PORT" \
    -H 'Content-Type: text/plain' \
    --data-binary 'no-rule-request-body' \
    "http://127.0.0.1:$MOCK_PORT/no-breakpoint-rule")"
assert_json_value "$no_rule_response" ".len" "20"
sleep 0.5
[[ ! -s "$NO_RULE_EVENT_FILE" ]] || fail "breakpoint paused without matching breakpoint://request rule: $(cat "$NO_RULE_EVENT_FILE")"
kill "$WS_PID" 2>/dev/null || true
wait "$WS_PID" 2>/dev/null || true
WS_PID=""

log "turning breakpoint off releases pending request"
BREAKPOINT_RULE_FILE="$REPO_DIR/e2e-tests/rules/breakpoint/production-ready.txt"
breakpoint_rules="$(sed \
    -e "s/__MOCK_HTTP_PORT__/$MOCK_PORT/g" \
    -e "s/__MOCK_HTTPS_PORT__/$TLS_PORT/g" \
    "$BREAKPOINT_RULE_FILE")"
create_rule "breakpoint-production-ready-e2e" "$breakpoint_rules"
curl -fsS -X POST "$(settings_url)" \
    -H 'Content-Type: application/json' \
    --data '{"enabled":true,"max_body_bytes":1048576}' >/dev/null

DISABLE_READY_FILE="$TMP_DIR/ws-disable-release.ready"
DISABLE_EVENT_FILE="$TMP_DIR/breakpoint-disable-release-event.json"
start_breakpoint_probe "$DISABLE_READY_FILE" "$DISABLE_EVENT_FILE" "disable-release"

DISABLE_OUT="$TMP_DIR/disable-release-response.json"
curl --noproxy "" -fsS -x "http://127.0.0.1:$ADMIN_PORT" \
    -H 'Content-Type: text/plain' \
    --data-binary 'disable-release-body' \
    "http://127.0.0.1:$MOCK_PORT/disable-release" >"$DISABLE_OUT" &
CURL_PID=$!

wait_for_file "$DISABLE_EVENT_FILE" 5 || fail "did not receive disable-release breakpoint event"
disable_event="$(cat "$DISABLE_EVENT_FILE")"
assert_json_value "$disable_event" ".phase" "request"
pending="$(curl -fsS "$(pending_url)")"
assert_json_value "$pending" ".[0].request_id" "$(echo "$disable_event" | jq -r '.request_id')"
assert_json_value "$pending" ".[0].phase" "request"
assert_json_value "$pending" ".[0].body" "disable-release-body"
curl -fsS -X POST "$(settings_url)" \
    -H 'Content-Type: application/json' \
    --data '{"enabled":false,"max_body_bytes":1048576}' >/dev/null
wait "$CURL_PID"
CURL_PID=""
wait "$WS_PID"
WS_PID=""
disable_response="$(cat "$DISABLE_OUT")"
assert_json_value "$disable_response" ".len" "20"
assert_json_value "$disable_response" ".prefix" "disable-release-"

log "editable request breakpoint can modify small body and headers"
curl -fsS -X POST "$(settings_url)" \
    -H 'Content-Type: application/json' \
    --data '{"enabled":true,"max_body_bytes":1048576}' >/dev/null

REQ_EDIT_READY_FILE="$TMP_DIR/ws-request-edit.ready"
REQ_EDIT_EVENT_FILE="$TMP_DIR/breakpoint-request-edit-event.json"
start_breakpoint_probe "$REQ_EDIT_READY_FILE" "$REQ_EDIT_EVENT_FILE" "request-edit"

REQ_EDIT_OUT="$TMP_DIR/request-edit-response.json"
curl --noproxy "" -fsS -x "http://127.0.0.1:$ADMIN_PORT" \
    -H 'Content-Type: text/plain' \
    --data-binary 'original-request-body' \
    "http://127.0.0.1:$MOCK_PORT/request-edit" >"$REQ_EDIT_OUT" &
CURL_PID=$!

wait_for_file "$REQ_EDIT_EVENT_FILE" 5 || fail "did not receive editable request breakpoint event"
request_edit_event="$(cat "$REQ_EDIT_EVENT_FILE")"
assert_json_value "$request_edit_event" ".phase" "request"
assert_json_value "$request_edit_event" ".body_omitted" "false"
assert_json_value "$request_edit_event" ".body" "original-request-body"

phase_mismatch_payload="$(jq -c '{request_id: .request_id, phase: "response"}' "$REQ_EDIT_EVENT_FILE")"
csrf_token="$(curl -fsS "$(admin_base_url)/api/security/csrf" | jq -r '.csrf_token')"
phase_mismatch_status="$(curl -sS -o "$TMP_DIR/phase-mismatch.json" -D "$TMP_DIR/phase-mismatch.headers" -w '%{http_code}' -X POST "$(resume_url)" \
    -H 'Origin: tauri://localhost' \
    -H 'Sec-Fetch-Site: cross-site' \
    -H "X-Bifrost-CSRF: $csrf_token" \
    -H 'Content-Type: application/json' \
    --data "$phase_mismatch_payload")"
[[ "$phase_mismatch_status" == "409" ]] || fail "wrong-phase resume should return 409, got $phase_mismatch_status"
assert_cors_origin "$TMP_DIR/phase-mismatch.headers" "tauri://localhost"
assert_json_value "$(curl -fsS "$(pending_url)")" ".[0].phase" "request"

request_edit_resume_payload="$(jq -c --arg url "http://127.0.0.1:${MOCK_PORT}/request-edited?mode=e2e" '.headers += [["X-Breakpoint-Request","edited"],["X-Duplicate","one"],["X-Duplicate","two"]] | {request_id: .request_id, phase: .phase, method: "PUT", url: $url, headers: .headers, body: "edited-request-body"}' "$REQ_EDIT_EVENT_FILE")"
curl -fsS -X POST "$(resume_url)" -H 'Content-Type: application/json' --data "$request_edit_resume_payload" >/dev/null
wait "$CURL_PID"
CURL_PID=""
wait "$WS_PID"
WS_PID=""
request_edit_response="$(cat "$REQ_EDIT_OUT")"
assert_json_value "$request_edit_response" ".len" "19"
assert_json_value "$request_edit_response" ".prefix" "edited-request-b"
assert_json_value "$request_edit_response" ".breakpoint_header" "edited"
assert_json_value "$request_edit_response" ".method" "PUT"
assert_json_value "$request_edit_response" ".path" "/request-edited?mode=e2e"

log "oversized request body is header-only paused and cannot be overwritten"
curl -fsS -X POST "$(settings_url)" \
    -H 'Content-Type: application/json' \
    --data '{"enabled":true,"max_body_bytes":1024}' >/dev/null

READY_FILE="$TMP_DIR/ws.ready"
EVENT_FILE="$TMP_DIR/breakpoint-event.json"
start_breakpoint_probe "$READY_FILE" "$EVENT_FILE" "oversized"

BODY_4K="$TMP_DIR/body-4k.bin"
python3 - "$BODY_4K" <<'PY'
import pathlib, sys
pathlib.Path(sys.argv[1]).write_bytes(b"b" * 4096)
PY
CURL_OUT="$TMP_DIR/oversized-response.json"
curl --noproxy "" -fsS -x "http://127.0.0.1:$ADMIN_PORT" --data-binary @"$BODY_4K" "http://127.0.0.1:$MOCK_PORT/oversized" >"$CURL_OUT" &
CURL_PID=$!

wait_for_file "$EVENT_FILE" 5 || fail "did not receive breakpoint_paused event"
event="$(cat "$EVENT_FILE")"
assert_json_value "$event" ".phase" "request"
assert_json_value "$event" ".body_omitted" "true"
assert_json_value "$event" ".body_size" "4096"
assert_json_value "$event" ".max_body_bytes" "1024"

resume_payload="$(jq -c '{request_id: .request_id, phase: .phase, headers: .headers, body: "mutated"}' "$EVENT_FILE")"
curl -fsS -X POST "$(resume_url)" -H 'Content-Type: application/json' --data "$resume_payload" >/dev/null
wait "$CURL_PID"
CURL_PID=""
wait "$WS_PID"
WS_PID=""
oversized_response="$(cat "$CURL_OUT")"
assert_json_value "$oversized_response" ".len" "4096"
assert_json_value "$oversized_response" ".prefix" "bbbbbbbbbbbbbbbb"

log "editable response breakpoint can modify small body and headers"
curl -fsS -X POST "$(settings_url)" \
    -H 'Content-Type: application/json' \
    --data '{"enabled":true,"max_body_bytes":1048576}' >/dev/null

RES_EDIT_READY_FILE="$TMP_DIR/ws-response-edit.ready"
RES_EDIT_EVENT_FILE="$TMP_DIR/breakpoint-response-edit-event.json"
start_breakpoint_probe "$RES_EDIT_READY_FILE" "$RES_EDIT_EVENT_FILE" "response-edit"

RES_EDIT_OUT="$TMP_DIR/response-edit-body.txt"
RES_EDIT_HEADERS="$TMP_DIR/response-edit-headers.txt"
curl --noproxy "" -sS -D "$RES_EDIT_HEADERS" -x "http://127.0.0.1:$ADMIN_PORT" \
    "http://127.0.0.1:$MOCK_PORT/response-edit" >"$RES_EDIT_OUT" &
CURL_PID=$!

wait_for_file "$RES_EDIT_EVENT_FILE" 5 || fail "did not receive editable response breakpoint event"
response_edit_event="$(cat "$RES_EDIT_EVENT_FILE")"
assert_json_value "$response_edit_event" ".phase" "response"
assert_json_value "$response_edit_event" ".body_omitted" "false"
assert_json_value "$response_edit_event" ".body" "hello-breakpoint-timeout"

response_edit_resume_payload="$(jq -c '.headers += [["X-Breakpoint-Response","edited"],["Set-Cookie","first=1"],["Set-Cookie","second=2"]] | {request_id: .request_id, phase: .phase, status: 418, headers: .headers, body: "edited-response-body"}' "$RES_EDIT_EVENT_FILE")"
curl -fsS -X POST "$(resume_url)" -H 'Content-Type: application/json' --data "$response_edit_resume_payload" >/dev/null
wait "$CURL_PID"
CURL_PID=""
wait "$WS_PID"
WS_PID=""
[[ "$(cat "$RES_EDIT_OUT")" == "edited-response-body" ]] || fail "response body was not edited: $(cat "$RES_EDIT_OUT")"
grep -qi '^x-breakpoint-response: edited' "$RES_EDIT_HEADERS" || fail "response header was not edited: $(cat "$RES_EDIT_HEADERS")"
grep -q '^HTTP/1.1 418' "$RES_EDIT_HEADERS" || fail "response status was not edited: $(cat "$RES_EDIT_HEADERS")"
[[ "$(grep -ci '^set-cookie:' "$RES_EDIT_HEADERS")" == "2" ]] || fail "duplicate response headers were not preserved: $(cat "$RES_EDIT_HEADERS")"

log "HTTPS breakpoint rule automatically enables scoped TLS interception"
TLS_READY_FILE="$TMP_DIR/ws-tls.ready"
TLS_EVENT_FILE="$TMP_DIR/breakpoint-tls-event.json"
start_breakpoint_probe "$TLS_READY_FILE" "$TLS_EVENT_FILE" "tls"
TLS_OUT="$TMP_DIR/tls-response-body.txt"
TLS_HEADERS="$TMP_DIR/tls-response-headers.txt"
curl --noproxy "" -kvfsS -D "$TLS_HEADERS" -x "http://127.0.0.1:$ADMIN_PORT" \
    "https://127.0.0.1:$TLS_PORT/https-breakpoint" \
    >"$TLS_OUT" 2>"$TMP_DIR/tls-curl.log" &
CURL_PID=$!
if ! wait_for_file "$TLS_EVENT_FILE" 10; then
    curl_state="running"
    if ! kill -0 "$CURL_PID" 2>/dev/null; then
        set +e
        wait "$CURL_PID"
        curl_status=$?
        set -e
        CURL_PID=""
        curl_state="exited with status ${curl_status}"
    fi
    fail "HTTPS breakpoint did not pause; curl=${curl_state}; curl_log=$(cat "$TMP_DIR/tls-curl.log" 2>/dev/null || true); tls_mock_log=$(cat "$TMP_DIR/tls-mock.log" 2>/dev/null || true)"
fi
tls_event="$(cat "$TLS_EVENT_FILE")"
assert_json_value "$tls_event" ".phase" "response"
assert_json_value "$tls_event" ".body" "hello-breakpoint-timeout"
tls_resume="$(jq -c '{request_id: .request_id, phase: .phase, status: 202, headers: .headers, body: "edited-https-response"}' "$TLS_EVENT_FILE")"
curl -fsS -X POST "$(resume_url)" -H 'Content-Type: application/json' --data "$tls_resume" >/dev/null
wait "$CURL_PID"
CURL_PID=""
wait "$WS_PID"
WS_PID=""
[[ "$(cat "$TLS_OUT")" == "edited-https-response" ]] || fail "HTTPS response body was not edited: $(cat "$TLS_OUT")"
grep -Eq '^HTTP/(1\.[01]|2) 202' "$TLS_HEADERS" || fail "HTTPS response status was not edited: $(cat "$TLS_HEADERS")"

log "combined breakpoint rule pauses request and response phases in order"
curl -fsS -X POST "$(settings_url)" \
    -H 'Content-Type: application/json' \
    --data '{"enabled":true,"max_body_bytes":1048576}' >/dev/null

BOTH_READY_FILE="$TMP_DIR/ws-both.ready"
BOTH_EVENT_FILE="$TMP_DIR/breakpoint-both-events.json"
start_breakpoint_probe "$BOTH_READY_FILE" "$BOTH_EVENT_FILE" "both-phase" 2

BOTH_OUT="$TMP_DIR/both-phase-body.txt"
BOTH_HEADERS="$TMP_DIR/both-phase-headers.txt"
curl --noproxy "" -fsS -D "$BOTH_HEADERS" -x "http://127.0.0.1:$ADMIN_PORT" \
    -H 'Content-Type: text/plain' \
    --data-binary 'both-request-body' \
    "http://127.0.0.1:$MOCK_PORT/both-phase" >"$BOTH_OUT" &
CURL_PID=$!

wait_for_file "$BOTH_EVENT_FILE" 5 || fail "did not receive first combined breakpoint event"
both_first_event="$(jq -c '.[0]' "$BOTH_EVENT_FILE")"
assert_json_value "$both_first_event" ".phase" "request"
assert_json_value "$both_first_event" ".body" "both-request-body"
both_request_resume="$(jq -c '.[0] | .headers += [["X-Breakpoint-Request","both"]] | {request_id: .request_id, phase: .phase, headers: .headers, body: "both-request-edited"}' "$BOTH_EVENT_FILE")"
curl -fsS -X POST "$(resume_url)" -H 'Content-Type: application/json' --data "$both_request_resume" >/dev/null

wait "$WS_PID"
WS_PID=""
both_second_event="$(jq -c '.[1]' "$BOTH_EVENT_FILE")"
assert_json_value "$both_second_event" ".phase" "response"
both_response_resume="$(jq -c '.[1] | .headers += [["X-Breakpoint-Response","both"]] | {request_id: .request_id, phase: .phase, headers: .headers, body: "both-response-edited"}' "$BOTH_EVENT_FILE")"
curl -fsS -X POST "$(resume_url)" -H 'Content-Type: application/json' --data "$both_response_resume" >/dev/null
wait "$CURL_PID"
CURL_PID=""
[[ "$(cat "$BOTH_OUT")" == "both-response-edited" ]] || fail "combined response body was not edited: $(cat "$BOTH_OUT")"
grep -qi '^x-breakpoint-response: both' "$BOTH_HEADERS" || fail "combined response header was not edited: $(cat "$BOTH_HEADERS")"

log "response breakpoint timeout releases traffic"
curl -fsS -X POST "$(settings_url)" \
    -H 'Content-Type: application/json' \
    --data '{"enabled":true,"max_body_bytes":1048576}' >/dev/null
timeout_config="$(curl -fsS -X PUT "$(performance_url)" \
    -H 'Content-Type: application/json' \
    --data '{"breakpoint_timeout_ms":5000}')"
assert_json_value "$timeout_config" ".breakpoint.timeout_ms" "5000"

TIMEOUT_READY_FILE="$TMP_DIR/ws-timeout.ready"
TIMEOUT_EVENT_FILE="$TMP_DIR/breakpoint-timeout-event.json"
start_breakpoint_probe "$TIMEOUT_READY_FILE" "$TIMEOUT_EVENT_FILE" "timeout"

start_ms="$(python3 - <<'PY'
import time
print(int(time.time() * 1000))
PY
)"
timeout_response="$(curl --noproxy "" -fsS -x "http://127.0.0.1:$ADMIN_PORT" "http://127.0.0.1:$MOCK_PORT/timeout")"
end_ms="$(python3 - <<'PY'
import time
print(int(time.time() * 1000))
PY
)"
elapsed_ms=$((end_ms - start_ms))
[[ "$timeout_response" == "hello-breakpoint-timeout" ]] || fail "unexpected timeout response: $timeout_response"
wait_for_file "$TIMEOUT_EVENT_FILE" 1 || fail "did not receive response breakpoint_paused event"
timeout_event="$(cat "$TIMEOUT_EVENT_FILE")"
assert_json_value "$timeout_event" ".phase" "response"
assert_json_value "$timeout_event" ".body_omitted" "false"
wait "$WS_PID"
WS_PID=""
if (( elapsed_ms < 4500 || elapsed_ms > 12000 )); then
    fail "response timeout elapsed ${elapsed_ms}ms outside expected range"
fi
assert_json_value "$(curl -fsS "$(pending_url)")" ". | length" "0"

log "all breakpoint performance guard tests passed"
