#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

cd "$REPO_DIR"

pick_port() {
  python3 - <<'PY'
import socket
with socket.socket() as s:
    s.bind(("127.0.0.1", 0))
    print(s.getsockname()[1])
PY
}

BIFROST_PORT="${BIFROST_PORT:-$(pick_port)}"
MOCK_PORT="${MOCK_PORT:-$(pick_port)}"
TEST_DIR="$(mktemp -d)"
BIFROST_DATA_DIR="$TEST_DIR/data"
BIFROST_LOG="$TEST_DIR/bifrost.log"
MOCK_LOG="$TEST_DIR/mock.jsonl"
IM_RESPONSE="$TEST_DIR/im-response.json"
SESSIONS_RESPONSE="$TEST_DIR/sessions.json"
HISTORY_RESPONSE="$TEST_DIR/history.json"
SSE_RESPONSE="$TEST_DIR/web-stream.sse"
FINAL_HISTORY_RESPONSE="$TEST_DIR/final-history.json"
BIFROST_BIN="${BIFROST_BIN:-}"
SESSION_KEY="timeline-channel-unified"

cleanup() {
  local exit_code="$?"
  if [[ "$exit_code" -ne 0 ]]; then
    echo "[agent-run-timeline-channel-unification] DEBUG temp dir: $TEST_DIR" >&2
    [[ -f "$BIFROST_LOG" ]] && tail -n 160 "$BIFROST_LOG" >&2 || true
    [[ -f "$MOCK_LOG" ]] && cat "$MOCK_LOG" >&2 || true
    [[ -f "$IM_RESPONSE" ]] && cat "$IM_RESPONSE" >&2 || true
    [[ -f "$SESSIONS_RESPONSE" ]] && cat "$SESSIONS_RESPONSE" >&2 || true
    [[ -f "$HISTORY_RESPONSE" ]] && cat "$HISTORY_RESPONSE" >&2 || true
    [[ -f "$SSE_RESPONSE" ]] && cat "$SSE_RESPONSE" >&2 || true
  fi
  [[ -n "${BIFROST_PID:-}" ]] && kill "$BIFROST_PID" >/dev/null 2>&1 || true
  [[ -n "${MOCK_PID:-}" ]] && kill "$MOCK_PID" >/dev/null 2>&1 || true
  [[ -n "${BIFROST_PID:-}" ]] && wait "$BIFROST_PID" >/dev/null 2>&1 || true
  [[ -n "${MOCK_PID:-}" ]] && wait "$MOCK_PID" >/dev/null 2>&1 || true
  rm -rf "$TEST_DIR"
  return "$exit_code"
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
  echo "[agent-run-timeline-channel-unification] $label did not become ready" >&2
  return 1
}

python3 - "$MOCK_PORT" "$MOCK_LOG" <<'PY' &
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port = int(sys.argv[1])
log_path = sys.argv[2]

def tool_call(call_id, name, arguments):
    return {
        "id": call_id,
        "type": "function",
        "function": {"name": name, "arguments": json.dumps(arguments, ensure_ascii=False)},
    }

def message_text(message):
    content = message.get("content")
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n".join(part.get("text", "") for part in content if isinstance(part, dict))
    return ""

class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        return

    def _json(self, status, payload):
        body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
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
        length = int(self.headers.get("Content-Length", "0"))
        payload = json.loads(self.rfile.read(length) or b"{}")
        with open(log_path, "a", encoding="utf-8") as fh:
            fh.write(json.dumps(payload, ensure_ascii=False) + "\n")
        joined = "\n".join(message_text(message) for message in payload.get("messages", []))
        if "Web continues same canonical timeline" in joined:
            if "IM asks for canonical timeline" not in joined:
                self._json(500, {"error": "web turn did not receive IM timeline history", "joined": joined})
                return
            self._json(200, {
                "choices": [{"message": {"role": "assistant", "content": "WEB_TURN_OK"}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 30, "completion_tokens": 2, "total_tokens": 32},
            })
            return
        if "Plan updated" in joined:
            self._json(200, {
                "choices": [{"message": {"role": "assistant", "content": "IM_TURN_OK"}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 20, "completion_tokens": 2, "total_tokens": 22},
            })
            return
        if "IM asks for canonical timeline" in joined:
            self._json(200, {
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": None,
                        "tool_calls": [tool_call("call-plan", "update_plan", {
                            "explanation": "IM turn creates canonical timeline progress",
                            "plan": [
                                {"step": "Record IM turn", "status": "completed"},
                                {"step": "Expose timeline to Web", "status": "in_progress"},
                            ],
                        })],
                    },
                    "finish_reason": "tool_calls",
                }],
                "usage": {"prompt_tokens": 10, "completion_tokens": 3, "total_tokens": 13},
            })
            return
        self._json(500, {"error": "unexpected model request", "joined": joined})

ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
MOCK_PID=$!
wait_http "http://127.0.0.1:$MOCK_PORT/health" "mock model"

resolve_bifrost_bin() {
  local preferred="${1:-}"
  local candidates=()
  [[ -n "$preferred" ]] && candidates+=("$preferred")
  candidates+=("$REPO_DIR/target/release/bifrost" "$REPO_DIR/target/debug/bifrost")
  for candidate in "${candidates[@]}"; do
    if [[ -x "$candidate" ]]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done
  return 1
}

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
  BIFROST_BIN="$(resolve_bifrost_bin "$BIFROST_BIN" || true)"
  echo "[agent-run-timeline-channel-unification] skipping build, using ${BIFROST_BIN:-<missing>}"
else
  echo "[agent-run-timeline-channel-unification] building bifrost"
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
  BIFROST_BIN="$(resolve_bifrost_bin "$BIFROST_BIN" || true)"
fi

if [[ ! -x "$BIFROST_BIN" ]]; then
  echo "[agent-run-timeline-channel-unification] bifrost binary not found: $BIFROST_BIN" >&2
  exit 1
fi

BIFROST_DATA_DIR="$BIFROST_DATA_DIR" "$BIFROST_BIN" start \
  --host 127.0.0.1 \
  -p "$BIFROST_PORT" \
  --unsafe-ssl \
  --skip-cert-check \
  --no-system-proxy \
  >"$BIFROST_LOG" 2>&1 &
BIFROST_PID=$!
wait_http "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address" "bifrost"

AGENT_BASE="http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/agent"
curl -fsS --noproxy '*' -X PATCH "$AGENT_BASE" \
  -H 'Content-Type: application/json' \
  -d "{
    \"enabled\": true,
    \"model_provider\": \"mock-timeline-unified\",
    \"model\": \"mock-model\",
    \"base_url\": \"http://127.0.0.1:$MOCK_PORT/chat/completions\",
    \"api_key\": \"test-key\",
    \"request_timeout_secs\": 20,
    \"max_turn_iterations\": 4,
    \"history\": {\"persistence\": \"save-all\"},
    \"memories\": {\"use_memories\": false, \"generate_memories\": false}
  }" >/dev/null

curl -fsS --noproxy '*' -X POST "$AGENT_BASE/chat" \
  -H 'Content-Type: application/json' \
  -d "{
    \"session_key\": \"$SESSION_KEY\",
    \"message\": \"IM asks for canonical timeline\"
  }" >"$IM_RESPONSE"

curl -fsS --noproxy '*' "$AGENT_BASE/sessions/all" >"$SESSIONS_RESPONSE"

HISTORY_PATH="$(
  python3 - "$IM_RESPONSE" "$SESSIONS_RESPONSE" "$SESSION_KEY" <<'PY'
import json
import sys
from pathlib import Path

im_response = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
sessions = json.loads(Path(sys.argv[2]).read_text(encoding="utf-8"))
session_key = sys.argv[3]
assert im_response.get("success") is True, im_response
assert "IM_TURN_OK" in im_response.get("response", ""), im_response
assert any(call.get("tool_name") == "update_plan" for call in im_response.get("tool_calls", [])), im_response
row = next(item for item in sessions.get("sessions", []) if item.get("session_key") == session_key)
assert row.get("has_timeline") is True, row
assert row.get("timeline_event_count", 0) >= 6, row
assert row.get("run_state") == "completed", row
history_path = row.get("history_path")
assert history_path, row
print(history_path)
PY
)"

ENCODED_HISTORY_PATH="$(python3 - "$HISTORY_PATH" <<'PY'
import sys
import urllib.parse
print(urllib.parse.quote(sys.argv[1], safe=""))
PY
)"
curl -fsS --noproxy '*' "$AGENT_BASE/sessions/history/$ENCODED_HISTORY_PATH" >"$HISTORY_RESPONSE"

python3 - "$HISTORY_RESPONSE" "$SESSION_KEY" <<'PY'
import json
import sys
from pathlib import Path

payload = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
events = payload.get("events", [])
types = [event.get("event_type") for event in events]
assert "user_message" in types, types
assert "tool_call" in types, types
assert "tool_result" in types, types
assert "assistant_message" in types, types
states = [event.get("content", {}).get("state") for event in events if event.get("event_type") == "run_state_changed"]
assert states[:1] == ["running"], states
assert states[-1:] == ["completed"], states
messages = [event.get("content", {}).get("message", "") for event in events]
assert "IM asks for canonical timeline" in messages, messages
assert "IM_TURN_OK" in messages, messages
PY

curl -fsS --noproxy '*' -N -X POST "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/agent/chat/stream" \
  -H 'Content-Type: application/json' \
  -d "{
    \"session_key\": \"$SESSION_KEY\",
    \"message\": \"Web continues same canonical timeline\"
  }" >"$SSE_RESPONSE"

python3 - "$SSE_RESPONSE" <<'PY'
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text(encoding="utf-8")
assert "run_finished" in text, text
assert "WEB_TURN_OK" in text, text
PY

curl -fsS --noproxy '*' "$AGENT_BASE/sessions/history/$ENCODED_HISTORY_PATH" >"$FINAL_HISTORY_RESPONSE"
python3 - "$FINAL_HISTORY_RESPONSE" <<'PY'
import json
import sys
from pathlib import Path

payload = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
events = payload.get("events", [])
messages = [event.get("content", {}).get("message", "") for event in events]
assert "IM asks for canonical timeline" in messages, messages
assert "IM_TURN_OK" in messages, messages
assert "Web continues same canonical timeline" in messages, messages
assert "WEB_TURN_OK" in messages, messages
run_states = [
    (event.get("content", {}).get("source_channel"), event.get("content", {}).get("state"))
    for event in events
    if event.get("event_type") == "run_state_changed"
]
assert ("api", "running") in run_states, run_states
assert ("api", "completed") in run_states, run_states
assert ("web", "running") in run_states, run_states
assert ("web", "completed") in run_states, run_states
PY

echo "[agent-run-timeline-channel-unification] PASS"
