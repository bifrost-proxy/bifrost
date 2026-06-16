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

BIFROST_PORT="${BIFROST_PORT:-$(pick_port)}"
MOCK_PORT="${MOCK_PORT:-$(pick_port)}"
TEST_DIR="$(mktemp -d)"
BIFROST_DATA_DIR="$TEST_DIR/data"
MOCK_LOG="$TEST_DIR/mock-requests.jsonl"
BIFROST_LOG="$TEST_DIR/bifrost.log"
RESPONSE_FILE="$TEST_DIR/response.json"
SLASH_RESPONSE_FILE="$TEST_DIR/slash-response.json"

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
  echo "[agent-plan-mode] $label did not become ready" >&2
  return 1
}

python3 - "$MOCK_PORT" "$MOCK_LOG" <<'PY' &
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port = int(sys.argv[1])
log_path = sys.argv[2]


class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        return

    def do_GET(self):
        if self.path == "/health":
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b"ok")
            return
        self.send_error(404)

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        payload = json.loads(self.rfile.read(length) or b"{}")
        with open(log_path, "a", encoding="utf-8") as fh:
            fh.write(json.dumps(payload, ensure_ascii=False) + "\n")
        joined = "\n".join(
            content
            for message in payload.get("messages", [])
            for content in [message.get("content")]
            if isinstance(content, str)
        )
        if "# Plan Mode" not in joined or "<proposed_plan>" not in joined:
            self.send_response(500)
            self.end_headers()
            self.wfile.write(b"missing plan mode prompt")
            return
        message = {
            "role": "assistant",
            "content": (
                "我已经确认需求，正式方案如下。\n"
                "<proposed_plan>\n"
                "# Plan Mode 回归方案\n"
                "\n"
                "## Summary\n"
                "- 使用 proposal 独立输出。\n"
                "\n"
                "## Test Plan\n"
                "- 校验 response 不包含 proposal 标签。\n"
                "</proposed_plan>\n"
            ),
        }
        body = {
            "id": "chatcmpl-plan-mode",
            "object": "chat.completion",
            "created": 1,
            "model": "mock-model",
            "choices": [{"index": 0, "message": message, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15},
        }
        data = json.dumps(body).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
MOCK_PID=$!
wait_http "http://127.0.0.1:$MOCK_PORT/health" "mock model"

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/release/bifrost}"
  echo "[agent-plan-mode] skipping build, using $BIFROST_BIN"
else
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
fi

if [[ ! -x "$BIFROST_BIN" ]]; then
  echo "[agent-plan-mode] bifrost binary not found or not executable: $BIFROST_BIN" >&2
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
    \"model_provider\": \"mock-plan-mode\",
    \"model\": \"mock-model\",
    \"base_url\": \"http://127.0.0.1:$MOCK_PORT/chat/completions\",
    \"api_key\": \"test-key\",
    \"memories\": {\"use_memories\": false, \"generate_memories\": false}
  }" >/dev/null

curl -fsS --noproxy '*' -X POST "$BASE/chat" \
  -H 'Content-Type: application/json' \
  -d "{\"session_key\":\"plan-mode-human-api\",\"message\":\"请规划修复方案\",\"collaboration_mode\":\"plan\"}" \
  >"$RESPONSE_FILE"

curl -fsS --noproxy '*' -X POST "$BASE/chat" \
  -H 'Content-Type: application/json' \
  -d "{\"session_key\":\"plan-mode-slash-human-api\",\"message\":\"/plan 请规划斜杠入口方案\"}" \
  >"$SLASH_RESPONSE_FILE"

python3 - "$RESPONSE_FILE" "$SLASH_RESPONSE_FILE" "$MOCK_LOG" <<'PY'
import json
import sys
from pathlib import Path

response = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
slash_response = json.loads(Path(sys.argv[2]).read_text(encoding="utf-8"))
mock_log_path = Path(sys.argv[3])
mock_log = mock_log_path.read_text(encoding="utf-8")
for item in [response, slash_response]:
    assert item.get("success") is True, item
    text = item.get("response", "")
    assert "<proposed_plan>" not in text, item
    assert "</proposed_plan>" not in text, item
    assert "正式方案" in text, item
    plan = item.get("proposed_plan")
    assert isinstance(plan, str) and "Plan Mode 回归方案" in plan, item
    assert "Test Plan" in plan, item
assert "# Plan Mode" in mock_log and "<proposed_plan>" in mock_log, mock_log
requests = [
    json.loads(line)
    for line in mock_log_path.read_text(encoding="utf-8").splitlines()
    if line.strip()
]
joined_messages = "\n".join(
    content
    for payload in requests
    for message in payload.get("messages", [])
    for content in [message.get("content")]
    if isinstance(content, str)
)
assert "请规划斜杠入口方案" in joined_messages, joined_messages
assert "/plan 请规划斜杠入口方案" not in joined_messages, joined_messages
PY

echo "[agent-plan-mode] PASS"
