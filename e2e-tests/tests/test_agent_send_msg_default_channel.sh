#!/usr/bin/env bash
set -euo pipefail
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_DIR"

BIFROST_PORT="${BIFROST_PORT:-${ADMIN_PORT:-18941}}"
MOCK_PORT="${MOCK_PORT:-18942}"
TEST_DIR="$(mktemp -d)"
MOCK_LOG="$TEST_DIR/mock-model.jsonl"
SEND_LOG="$TEST_DIR/mock-send.jsonl"
BIFROST_LOG="$TEST_DIR/bifrost.log"
BIFROST_BIN="${BIFROST_BIN:-}"

cleanup() {
  if [[ -n "${BIFROST_PID:-}" ]]; then
    kill "$BIFROST_PID" >/dev/null 2>&1 || true
    wait "$BIFROST_PID" >/dev/null 2>&1 || true
  fi
  if [[ -n "${MOCK_PID:-}" ]]; then
    kill "$MOCK_PID" >/dev/null 2>&1 || true
    wait "$MOCK_PID" >/dev/null 2>&1 || true
  fi
  rm -rf "$TEST_DIR"
}
trap cleanup EXIT

wait_http() {
  local url="$1"
  local label="$2"
  for _ in $(seq 1 120); do
    if curl -fsS --noproxy '*' "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.25
  done
  echo "[agent-send-msg-default-channel] $label did not become ready" >&2
  [[ -f "$BIFROST_LOG" ]] && tail -120 "$BIFROST_LOG" >&2 || true
  return 1
}

python3 - "$MOCK_PORT" "$MOCK_LOG" "$SEND_LOG" <<'PY' &
import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port = int(sys.argv[1])
model_log = sys.argv[2]
send_log = sys.argv[3]
stage = {"value": 0}
lock = threading.Lock()

def tool_call(call_id, name, arguments):
    return {
        "id": call_id,
        "type": "function",
        "function": {"name": name, "arguments": json.dumps(arguments, ensure_ascii=False)},
    }

class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        return

    def _json(self, status, payload):
        body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path == "/health":
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b"ok")
            return
        self._json(404, {"error": "not found"})

    def do_POST(self):
        length = int(self.headers.get("content-length") or 0)
        body = self.rfile.read(length) if length else b"{}"
        if self.path.startswith("/chat/completions"):
            payload = json.loads(body.decode("utf-8") or "{}")
            with open(model_log, "a", encoding="utf-8") as fh:
                fh.write(json.dumps(payload, ensure_ascii=False) + "\n")
            tool_names = {
                tool.get("function", {}).get("name")
                for tool in payload.get("tools", [])
                if tool.get("type") == "function"
            }
            if "send_msg" not in tool_names or "schedule_create" not in tool_names:
                self._json(500, {"error": f"missing injected tools: {sorted(tool_names)}"})
                return
            with lock:
                current = stage["value"]
                stage["value"] += 1
            if current == 0:
                message = {
                    "role": "assistant",
                    "content": None,
                    "tool_calls": [
                        tool_call("call-send-msg", "send_msg", {"text": "hello via send_msg"}),
                        tool_call("call-schedule-create", "schedule_create", {
                            "id": "default-bound-schedule",
                            "name": "Default Bound Schedule",
                            "enabled": False,
                            "task_type": "agent",
                            "trigger": {"type": "interval", "every_ms": 60000},
                            "agent": {"prompt": "scheduled prompt"},
                        }),
                    ],
                }
                finish_reason = "tool_calls"
            else:
                message = {"role": "assistant", "content": "send_msg and schedule_create done"}
                finish_reason = "stop"
            self._json(200, {
                "choices": [{"message": message, "finish_reason": finish_reason}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15},
            })
            return

        if self.path.startswith("/ilink/bot/sendmessage"):
            if self.headers.get("AuthorizationType") != "ilink_bot_token":
                self._json(401, {"error": "missing AuthorizationType"})
                return
            if self.headers.get("Authorization") != "Bearer mock-token":
                self._json(401, {"error": "missing Authorization"})
                return
            with open(send_log, "ab") as fh:
                fh.write(body + b"\n")
            self._json(200, {"message_id": "sent-agent-msg-1", "request_id": "req-agent-msg-1"})
            return

        self._json(404, {"error": "not found"})

ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
MOCK_PID=$!
wait_http "http://127.0.0.1:$MOCK_PORT/health" "mock server"

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/release/bifrost}"
  echo "[agent-send-msg-default-channel] skipping build, using $BIFROST_BIN"
else
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
  echo "[agent-send-msg-default-channel] building bifrost"
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

echo "[agent-send-msg-default-channel] starting bifrost on $BIFROST_PORT"
BIFROST_DATA_DIR="$TEST_DIR" "$BIFROST_BIN" start \
  --host 127.0.0.1 \
  -p "$BIFROST_PORT" \
  --unsafe-ssl \
  --skip-cert-check \
  --no-system-proxy \
  >"$BIFROST_LOG" 2>&1 &
BIFROST_PID=$!
wait_http "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address" "bifrost"

IM_BASE="http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway"
curl -fsS --noproxy '*' -X POST "$IM_BASE/providers" \
  -H 'Content-Type: application/json' \
  -d "{
    \"id\":\"weixin-mock\",
    \"provider_type\":\"weixin\",
    \"display_name\":\"Weixin Mock\",
    \"enabled\":true,
    \"base_url\":\"http://127.0.0.1:$MOCK_PORT\",
    \"app_id\":\"mock-bot@im.bot\",
    \"secret_ref\":\"mock-token\",
    \"owner_open_id\":\"mock-user@im.wechat\"
  }" >/dev/null

AGENT_BASE="$IM_BASE/agent"
curl -fsS --noproxy '*' -X PATCH "$AGENT_BASE" \
  -H 'Content-Type: application/json' \
  -d "{
    \"enabled\": true,
    \"model_provider\": \"mock-send-msg\",
    \"model\": \"mock-model\",
    \"base_url\": \"http://127.0.0.1:$MOCK_PORT/chat/completions\",
    \"api_key\": \"test-key\",
    \"request_timeout_secs\": 20,
    \"max_turn_iterations\": 4,
    \"history\": {\"persistence\": \"none\"},
    \"memories\": {\"use_memories\": false, \"generate_memories\": false},
    \"default_message_channel\": {
      \"provider_id\": \"weixin-mock\",
      \"target_id\": \"mock-user@im.wechat\",
      \"target_mode\": \"owner\"
    }
  }" >/dev/null

RESPONSE_FILE="$TEST_DIR/agent-response.json"
curl -fsS --noproxy '*' -X POST "$AGENT_BASE/chat" \
  -H 'Content-Type: application/json' \
  -d '{"session_key":"send-msg-default-channel","message":"send one message and create one schedule"}' \
  >"$RESPONSE_FILE"

python3 - "$RESPONSE_FILE" "$SEND_LOG" "$IM_BASE" <<'PY'
import json
import pathlib
import sys
import urllib.request

response = json.loads(pathlib.Path(sys.argv[1]).read_text())
assert response.get("success") is True, response
names = [call.get("tool_name") for call in response.get("tool_calls", [])]
assert "send_msg" in names, names
assert "schedule_create" in names, names

send_lines = [line for line in pathlib.Path(sys.argv[2]).read_text().splitlines() if line.strip()]
assert send_lines, "send_msg did not call weixin sendmessage"
send_payloads = [json.loads(line) for line in send_lines]
msgs = [payload.get("msg", {}) for payload in send_payloads]
assert any(msg.get("to_user_id") == "mock-user@im.wechat" for msg in msgs), send_payloads
assert any(
    (msg.get("item_list") or [{}])[0].get("text_item", {}).get("text") == "hello via send_msg"
    for msg in msgs
), send_payloads

with urllib.request.urlopen(sys.argv[3] + "/schedules") as resp:
    schedules = json.loads(resp.read().decode("utf-8"))
schedule = next((item for item in schedules if item.get("id") == "default-bound-schedule"), None)
assert schedule is not None, schedules
channel = schedule.get("message_channel") or {}
assert channel.get("provider_id") == "weixin-mock", schedule
assert channel.get("target_mode") == "owner", schedule
assert channel.get("target_id") == "mock-user@im.wechat", schedule
PY

echo "[agent-send-msg-default-channel] PASS"
