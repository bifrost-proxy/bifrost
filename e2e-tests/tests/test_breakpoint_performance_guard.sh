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
TMP_DIR="$(mktemp -d)"
MOCK_PID=""
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
            "len": len(body),
            "sha256": hashlib.sha256(body).hexdigest(),
            "prefix": body[:16].decode("utf-8", "replace"),
            "breakpoint_header": self.headers.get("X-Breakpoint-Request", ""),
        }
        self._send(200, json.dumps(payload))

    def do_GET(self):
        self._send(200, b"hello-breakpoint-timeout", "text/plain")

    def log_message(self, *_args):
        return

port = int(sys.argv[1])
ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
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

write_ws_probe() {
    cat >"$TMP_DIR/wait_breakpoint_push.js" <<'JS'
const fs = require("fs");
const path = require("path");
const { WebSocket } = require(path.join(process.cwd(), "web", "node_modules", "ws"));

const [url, readyFile, eventFile, expectedCountArg] = process.argv.slice(2);
const expectedCount = Math.max(1, Number(expectedCountArg || "1"));
const events = [];
const ws = new WebSocket(url);
const timer = setTimeout(() => {
  console.error("timeout waiting for breakpoint_paused");
  try { ws.close(); } catch {}
  process.exit(2);
}, 10000);

ws.on("open", () => {
  fs.writeFileSync(readyFile, "ready");
});

ws.on("message", (raw) => {
  let msg;
  try {
    msg = JSON.parse(raw.toString());
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

ws.on("error", (err) => {
  clearTimeout(timer);
  console.error(err.message || String(err));
  process.exit(1);
});
JS
}

assert_json_value() {
    local json="$1"
    local expr="$2"
    local expected="$3"
    local actual
    actual="$(echo "$json" | jq -r "$expr")"
    [[ "$actual" == "$expected" ]] || fail "expected $expr to be '$expected', got '$actual' in $json"
}

log "building bifrost debug binary"
TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_DIR/target}"
cargo build --bin bifrost
export BIFROST_BIN="$TARGET_DIR/debug/bifrost"

start_mock_server
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
node "$TMP_DIR/wait_breakpoint_push.js" "ws://127.0.0.1:$ADMIN_PORT${ADMIN_PATH_PREFIX}/api/push" "$NO_RULE_READY_FILE" "$NO_RULE_EVENT_FILE" &
WS_PID=$!
wait_for_file "$NO_RULE_READY_FILE" 5 || fail "no-rule push websocket did not become ready"
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

log "editable request breakpoint can modify small body and headers"
create_rule "breakpoint-request-e2e" "127.0.0.1:${MOCK_PORT}/request-edit breakpoint://request"
curl -fsS -X POST "$(settings_url)" \
    -H 'Content-Type: application/json' \
    --data '{"enabled":true,"max_body_bytes":1048576}' >/dev/null

REQ_EDIT_READY_FILE="$TMP_DIR/ws-request-edit.ready"
REQ_EDIT_EVENT_FILE="$TMP_DIR/breakpoint-request-edit-event.json"
node "$TMP_DIR/wait_breakpoint_push.js" "ws://127.0.0.1:$ADMIN_PORT${ADMIN_PATH_PREFIX}/api/push" "$REQ_EDIT_READY_FILE" "$REQ_EDIT_EVENT_FILE" &
WS_PID=$!
wait_for_file "$REQ_EDIT_READY_FILE" 5 || fail "request edit push websocket did not become ready"

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

request_edit_resume_payload="$(jq -c '.headers += [["X-Breakpoint-Request","edited"]] | {request_id: .request_id, phase: .phase, headers: .headers, body: "edited-request-body"}' "$REQ_EDIT_EVENT_FILE")"
curl -fsS -X POST "$(resume_url)" -H 'Content-Type: application/json' --data "$request_edit_resume_payload" >/dev/null
wait "$CURL_PID"
CURL_PID=""
wait "$WS_PID"
WS_PID=""
request_edit_response="$(cat "$REQ_EDIT_OUT")"
assert_json_value "$request_edit_response" ".len" "19"
assert_json_value "$request_edit_response" ".prefix" "edited-request-b"
assert_json_value "$request_edit_response" ".breakpoint_header" "edited"

log "oversized request body is header-only paused and cannot be overwritten"
create_rule "breakpoint-request-oversized-e2e" "127.0.0.1:${MOCK_PORT}/oversized breakpoint://request"
curl -fsS -X POST "$(settings_url)" \
    -H 'Content-Type: application/json' \
    --data '{"enabled":true,"max_body_bytes":1024}' >/dev/null

READY_FILE="$TMP_DIR/ws.ready"
EVENT_FILE="$TMP_DIR/breakpoint-event.json"
node "$TMP_DIR/wait_breakpoint_push.js" "ws://127.0.0.1:$ADMIN_PORT${ADMIN_PATH_PREFIX}/api/push" "$READY_FILE" "$EVENT_FILE" &
WS_PID=$!
wait_for_file "$READY_FILE" 5 || fail "push websocket did not become ready"

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
create_rule "breakpoint-response-e2e" "127.0.0.1:${MOCK_PORT}/response-edit breakpoint://response"
curl -fsS -X POST "$(settings_url)" \
    -H 'Content-Type: application/json' \
    --data '{"enabled":true,"max_body_bytes":1048576}' >/dev/null

RES_EDIT_READY_FILE="$TMP_DIR/ws-response-edit.ready"
RES_EDIT_EVENT_FILE="$TMP_DIR/breakpoint-response-edit-event.json"
node "$TMP_DIR/wait_breakpoint_push.js" "ws://127.0.0.1:$ADMIN_PORT${ADMIN_PATH_PREFIX}/api/push" "$RES_EDIT_READY_FILE" "$RES_EDIT_EVENT_FILE" &
WS_PID=$!
wait_for_file "$RES_EDIT_READY_FILE" 5 || fail "response edit push websocket did not become ready"

RES_EDIT_OUT="$TMP_DIR/response-edit-body.txt"
RES_EDIT_HEADERS="$TMP_DIR/response-edit-headers.txt"
curl --noproxy "" -fsS -D "$RES_EDIT_HEADERS" -x "http://127.0.0.1:$ADMIN_PORT" \
    "http://127.0.0.1:$MOCK_PORT/response-edit" >"$RES_EDIT_OUT" &
CURL_PID=$!

wait_for_file "$RES_EDIT_EVENT_FILE" 5 || fail "did not receive editable response breakpoint event"
response_edit_event="$(cat "$RES_EDIT_EVENT_FILE")"
assert_json_value "$response_edit_event" ".phase" "response"
assert_json_value "$response_edit_event" ".body_omitted" "false"
assert_json_value "$response_edit_event" ".body" "hello-breakpoint-timeout"

response_edit_resume_payload="$(jq -c '.headers += [["X-Breakpoint-Response","edited"]] | {request_id: .request_id, phase: .phase, headers: .headers, body: "edited-response-body"}' "$RES_EDIT_EVENT_FILE")"
curl -fsS -X POST "$(resume_url)" -H 'Content-Type: application/json' --data "$response_edit_resume_payload" >/dev/null
wait "$CURL_PID"
CURL_PID=""
wait "$WS_PID"
WS_PID=""
[[ "$(cat "$RES_EDIT_OUT")" == "edited-response-body" ]] || fail "response body was not edited: $(cat "$RES_EDIT_OUT")"
grep -qi '^x-breakpoint-response: edited' "$RES_EDIT_HEADERS" || fail "response header was not edited: $(cat "$RES_EDIT_HEADERS")"

log "combined breakpoint rule pauses request and response phases in order"
create_rule "breakpoint-both-e2e" "127.0.0.1:${MOCK_PORT}/both-phase breakpoint://request,response"
curl -fsS -X POST "$(settings_url)" \
    -H 'Content-Type: application/json' \
    --data '{"enabled":true,"max_body_bytes":1048576}' >/dev/null

BOTH_READY_FILE="$TMP_DIR/ws-both.ready"
BOTH_EVENT_FILE="$TMP_DIR/breakpoint-both-events.json"
node "$TMP_DIR/wait_breakpoint_push.js" "ws://127.0.0.1:$ADMIN_PORT${ADMIN_PATH_PREFIX}/api/push" "$BOTH_READY_FILE" "$BOTH_EVENT_FILE" 2 &
WS_PID=$!
wait_for_file "$BOTH_READY_FILE" 5 || fail "both-phase push websocket did not become ready"

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
create_rule "breakpoint-response-timeout-e2e" "127.0.0.1:${MOCK_PORT}/timeout breakpoint://response"
curl -fsS -X POST "$(settings_url)" \
    -H 'Content-Type: application/json' \
    --data '{"enabled":true,"max_body_bytes":1048576}' >/dev/null
timeout_config="$(curl -fsS -X PUT "$(performance_url)" \
    -H 'Content-Type: application/json' \
    --data '{"breakpoint_timeout_ms":5000}')"
assert_json_value "$timeout_config" ".breakpoint.timeout_ms" "5000"

TIMEOUT_READY_FILE="$TMP_DIR/ws-timeout.ready"
TIMEOUT_EVENT_FILE="$TMP_DIR/breakpoint-timeout-event.json"
node "$TMP_DIR/wait_breakpoint_push.js" "ws://127.0.0.1:$ADMIN_PORT${ADMIN_PATH_PREFIX}/api/push" "$TIMEOUT_READY_FILE" "$TIMEOUT_EVENT_FILE" &
WS_PID=$!
wait_for_file "$TIMEOUT_READY_FILE" 5 || fail "timeout push websocket did not become ready"

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

log "all breakpoint performance guard tests passed"
