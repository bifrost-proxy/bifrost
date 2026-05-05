#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

cd "$REPO_DIR"

BIFROST_PORT="${BIFROST_PORT:-18897}"
MOCK_PORT="${MOCK_PORT:-18898}"
TEST_DIR="$(mktemp -d)"
MOCK_LOG="$TEST_DIR/mock-requests.jsonl"
BIFROST_LOG="$TEST_DIR/bifrost.log"
CHAT_RESPONSE="$TEST_DIR/chat-response.json"
STATUS_RESPONSE="$TEST_DIR/status-response.json"
IDLE_STATUS_RESPONSE="$TEST_DIR/idle-status-response.json"
WORK_DIR="$TEST_DIR/workdir"
mkdir -p "$WORK_DIR"

cleanup() {
  if [[ -n "${CHAT_PID:-}" ]]; then
    wait "$CHAT_PID" >/dev/null 2>&1 || true
  fi
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
  echo "[agent-builtin-status-runtime] $label did not become ready" >&2
  return 1
}

python3 - "$MOCK_PORT" "$MOCK_LOG" <<'PY' &
import json
import sys
import time
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
        body = self.rfile.read(length)
        try:
            payload = json.loads(body or b"{}")
        except Exception:
            payload = {}

        with open(log_path, "a", encoding="utf-8") as fh:
            fh.write(json.dumps(payload, ensure_ascii=False) + "\n")

        time.sleep(3.0)
        response = {
            "choices": [
                {
                    "message": {
                        "role": "assistant",
                        "content": "mock turn finished",
                    },
                    "finish_reason": "stop",
                }
            ],
            "usage": {"prompt_tokens": 12, "completion_tokens": 5, "total_tokens": 17},
        }
        encoded = json.dumps(response, ensure_ascii=False).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)


ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
MOCK_PID=$!
wait_http "http://127.0.0.1:$MOCK_PORT/health" "mock model"

echo "[agent-builtin-status-runtime] building bifrost"
SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost

echo "[agent-builtin-status-runtime] starting bifrost on $BIFROST_PORT"
BIFROST_DATA_DIR="$TEST_DIR" ./target/debug/bifrost start \
  --host 127.0.0.1 \
  -p "$BIFROST_PORT" \
  --unsafe-ssl \
  --no-system-proxy \
  >"$BIFROST_LOG" 2>&1 &
BIFROST_PID=$!
wait_http "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address" "bifrost"

BASE="http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/agent"

echo "[agent-builtin-status-runtime] configuring agent mock provider"
curl -fsS --noproxy '*' -X PATCH "$BASE" \
  -H 'Content-Type: application/json' \
  -d "{
    \"enabled\": true,
    \"model_provider\": \"mock-status-runtime\",
    \"model\": \"mock-model\",
    \"base_url\": \"http://127.0.0.1:$MOCK_PORT/chat/completions\",
    \"api_key\": \"test-key\",
    \"request_timeout_secs\": 20,
    \"max_turn_iterations\": 8,
    \"memories\": {
      \"use_memories\": false,
      \"generate_memories\": false
    }
  }" >/dev/null

echo "[agent-builtin-status-runtime] starting a long model request"
curl -fsS --noproxy '*' -X POST "$BASE/chat" \
  -H 'Content-Type: application/json' \
  -d "{\"session_key\":\"agent-status-runtime\",\"work_dir\":\"$WORK_DIR\",\"message\":\"请等待 mock 模型完成。\"}" \
  >"$CHAT_RESPONSE" &
CHAT_PID=$!

sleep 0.8

echo "[agent-builtin-status-runtime] querying /status while turn is active"
curl -fsS --noproxy '*' -X POST "$BASE/chat" \
  -H 'Content-Type: application/json' \
  -d '{"session_key":"agent-status-runtime","message":"/status"}' \
  >"$STATUS_RESPONSE"

python3 - "$STATUS_RESPONSE" "$WORK_DIR" <<'PY'
import json
import sys
from pathlib import Path

response = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
assert response.get("success") is True, response
text = response.get("response", "")
assert "会话状态:" in text, text
assert "正在处理中" in text, text
assert "工作路径:" in text, text
assert "Loop: 第 1 次" in text, text
assert "实时 token:" in text, text
assert "Context 用量:" in text, text
assert "压缩次数:" in text, text
assert "请稍后再试" not in text, text

active = response.get("active_status")
assert isinstance(active, dict), response
assert active.get("session_key") == "agent-status-runtime", active
assert active.get("work_dir") == sys.argv[2], active
assert active.get("current_loop_iteration") == 1, active
assert active.get("max_loop_iterations") == 8, active
assert active.get("context_window_tokens") == 250000, active
assert active.get("compaction_count") == 0, active
assert active.get("estimated_context_tokens", 0) >= 0, active
assert active.get("context_usage_percent") is not None, active
PY

wait "$CHAT_PID"

python3 - "$CHAT_RESPONSE" <<'PY'
import json
import sys
from pathlib import Path

response = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
assert response.get("success") is True, response
assert "mock turn finished" in response.get("response", ""), response
PY

curl -fsS --noproxy '*' -X POST "$BASE/chat" \
  -H 'Content-Type: application/json' \
  -d '{"session_key":"agent-status-runtime","message":"/status"}' \
  >"$IDLE_STATUS_RESPONSE"

python3 - "$IDLE_STATUS_RESPONSE" "$WORK_DIR" <<'PY'
import json
import sys
from pathlib import Path

response = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
assert response.get("success") is True, response
text = response.get("response", "")
assert "会话状态:" in text, text
assert f"工作路径: {sys.argv[2]}" in text, text
assert "API 累计 token:" in text, text
assert "Context 用量:" in text, text
PY

echo "[agent-builtin-status-runtime] PASS"
