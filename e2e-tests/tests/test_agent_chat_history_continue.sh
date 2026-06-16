#!/usr/bin/env bash
set -euo pipefail
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

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

BIFROST_PORT="${BIFROST_PORT:-${ADMIN_PORT:-${PROXY_PORT:-$(pick_port)}}}"
MOCK_PORT="${MOCK_PORT:-}"
TEST_DIR="$(mktemp -d)"
BIFROST_DATA_DIR="$TEST_DIR/data"
AGENT_HOME="$BIFROST_DATA_DIR/agent"
HISTORY_FILE="$AGENT_HOME/sessions/2026/05/23/session-history-continue-1.jsonl"
OUTSIDE_FILE="$TEST_DIR/outside.jsonl"
BIFROST_LOG="$TEST_DIR/bifrost.log"
MOCK_LOG="$TEST_DIR/mock.log"
MOCK_PORT_FILE="$TEST_DIR/mock-port"
RESPONSE_FILE="$TEST_DIR/response.json"
BIFROST_BIN="${BIFROST_BIN:-}"

cleanup() {
  local exit_code="$?"
  if [[ "$exit_code" -ne 0 ]]; then
    echo "[agent-chat-history-continue] DEBUG temp dir: $TEST_DIR" >&2
    [[ -f "$BIFROST_LOG" ]] && tail -n 120 "$BIFROST_LOG" >&2 || true
    [[ -f "$MOCK_LOG" ]] && cat "$MOCK_LOG" >&2 || true
    [[ -f "$RESPONSE_FILE" ]] && cat "$RESPONSE_FILE" >&2 || true
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
  echo "[agent-chat-history-continue] $label did not become ready" >&2
  return 1
}

mkdir -p "$(dirname "$HISTORY_FILE")"
python3 - "$HISTORY_FILE" <<'PY'
import json
import sys
from pathlib import Path

path = Path(sys.argv[1])
replacement_history = [
    {"role": "user", "content": "Latest request before compaction"},
    {"role": "user", "content": "Compacted summary with preserved progress"},
]
current_plan = [
    {"step": "Inspect previous logs", "status": "completed"},
    {"step": "Continue after history restore", "status": "in_progress"},
]
events = [
    {"timestamp": 1, "event_type": "user_message", "session_key": "history-continue", "content": {"message": "Old question before compaction"}},
    {"timestamp": 2, "event_type": "assistant_message", "session_key": "history-continue", "content": {"message": "Old answer before compaction", "tokens": 11}},
    {"timestamp": 3, "event_type": "plan_updated", "session_key": "history-continue", "content": {"explanation": "pre-compact plan", "plan": current_plan}},
    {"timestamp": 4, "event_type": "compaction", "session_key": "history-continue", "content": {"compaction_count": 1, "replacement_history": replacement_history, "current_plan": current_plan}},
]
path.write_text("\n".join(json.dumps(event, ensure_ascii=False) for event in events) + "\n" + '{"timestamp":5,"event_type":"assistant_message"\n', encoding="utf-8")
PY
printf '{}\n' >"$OUTSIDE_FILE"

python3 - "$MOCK_PORT" "$MOCK_LOG" "$MOCK_PORT_FILE" <<'PY' &
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

requested_port = int(sys.argv[1]) if sys.argv[1] else 0
log_path = sys.argv[2]
port_path = sys.argv[3]

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

    def do_GET(self):
        if self.path == "/health":
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b"ok")
        else:
            self.send_error(404)

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        payload = json.loads(self.rfile.read(length) or b"{}")
        with open(log_path, "a", encoding="utf-8") as fh:
            fh.write(json.dumps(payload, ensure_ascii=False) + "\n")
        joined = "\n".join(message_text(message) for message in payload.get("messages", []))
        missing = [text for text in ["Latest request before compaction", "Compacted summary with preserved progress", "New question"] if text not in joined]
        if missing:
            body = json.dumps({"missing": missing, "joined": joined}, ensure_ascii=False).encode("utf-8")
            self.send_response(500)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        if "Old question before compaction" in joined or "Old answer before compaction" in joined:
            body = json.dumps({"unexpected_old_history": True, "joined": joined}, ensure_ascii=False).encode("utf-8")
            self.send_response(500)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        body = json.dumps({
            "choices": [{"message": {"role": "assistant", "content": "CONTINUED_FROM_HISTORY_OK"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 2, "total_tokens": 12},
        }, ensure_ascii=False).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

server = ThreadingHTTPServer(("127.0.0.1", requested_port), Handler)
with open(port_path, "w", encoding="utf-8") as fh:
    fh.write(str(server.server_address[1]))
server.serve_forever()
PY
MOCK_PID=$!
for _ in $(seq 1 120); do
  if [[ -s "$MOCK_PORT_FILE" ]]; then
    MOCK_PORT="$(cat "$MOCK_PORT_FILE")"
    break
  fi
  if ! kill -0 "$MOCK_PID" >/dev/null 2>&1; then
    echo "[agent-chat-history-continue] mock model exited before reporting port" >&2
    [[ -f "$MOCK_LOG" ]] && cat "$MOCK_LOG" >&2 || true
    exit 1
  fi
  sleep 0.25
done
if [[ -z "$MOCK_PORT" ]]; then
  echo "[agent-chat-history-continue] mock model did not report a port" >&2
  exit 1
fi
wait_http "http://127.0.0.1:$MOCK_PORT/health" "mock model"

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/release/bifrost}"
  echo "[agent-chat-history-continue] skipping build, using $BIFROST_BIN"
else
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
fi

if [[ ! -x "$BIFROST_BIN" ]]; then
  echo "[agent-chat-history-continue] bifrost binary not found or not executable: $BIFROST_BIN" >&2
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

BASE="http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/agent"
curl -fsS --noproxy '*' -X PATCH "$BASE" \
  -H 'Content-Type: application/json' \
  -d "{
    \"enabled\": true,
    \"model_provider\": \"mock-history-continue\",
    \"model\": \"mock-model\",
    \"base_url\": \"http://127.0.0.1:$MOCK_PORT/chat/completions\",
    \"api_key\": \"test-key\",
    \"history\": {\"persistence\": \"save-all\"},
    \"memories\": {\"use_memories\": false, \"generate_memories\": false}
  }" >/dev/null

curl -fsS --noproxy '*' -X POST "$BASE/chat" \
  -H 'Content-Type: application/json' \
  -d "{
    \"session_key\": \"history-continue\",
    \"history_path\": \"$HISTORY_FILE\",
    \"message\": \"New question\"
  }" >"$RESPONSE_FILE"

python3 - "$RESPONSE_FILE" "$HISTORY_FILE" <<'PY'
import json
import sys
from pathlib import Path

response = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
assert response.get("success") is True, response
assert "CONTINUED_FROM_HISTORY_OK" in response.get("response", ""), response
plan = response.get("plan_steps")
assert plan and plan[0]["step"] == "Inspect previous logs", response
assert plan[0]["status"] == "completed", response
history = Path(sys.argv[2]).read_text(encoding="utf-8")
assert "New question" in history, history
assert "CONTINUED_FROM_HISTORY_OK" in history, history
PY

status="$(
  curl -sS --noproxy '*' -o "$TEST_DIR/outside-response.txt" -w '%{http_code}' \
    "$BASE/sessions/history/$(python3 -c 'import sys, urllib.parse; print(urllib.parse.quote(sys.argv[1], safe=""))' "$OUTSIDE_FILE")"
)"
if [[ "$status" != "400" ]]; then
  echo "[agent-chat-history-continue] expected outside history path to return 400, got $status" >&2
  cat "$TEST_DIR/outside-response.txt" >&2 || true
  exit 1
fi

echo "[agent-chat-history-continue] PASS"
