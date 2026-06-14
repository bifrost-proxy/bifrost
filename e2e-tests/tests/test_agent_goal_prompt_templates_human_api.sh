#!/usr/bin/env bash
set -euo pipefail
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_DIR"

source "$REPO_DIR/e2e-tests/test_utils/process.sh"

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

cleanup() {
  if [[ -n "${BIFROST_PID:-}" ]]; then
    if [[ -n "${BIFROST_BIN:-}" ]]; then
      BIFROST_DATA_DIR="$BIFROST_DATA_DIR" "$BIFROST_BIN" -p "$BIFROST_PORT" stop >/dev/null 2>&1 || true
    fi
    safe_cleanup_proxy "$BIFROST_PID"
  fi
  if [[ -n "${MOCK_PID:-}" ]]; then
    kill_pid "$MOCK_PID"
    wait_pid "$MOCK_PID" >/dev/null 2>&1 || true
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
  echo "[agent-goal-prompts] $label did not become ready" >&2
  return 1
}

python3 - "$MOCK_PORT" "$MOCK_LOG" <<'PY' &
import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port = int(sys.argv[1])
log_path = sys.argv[2]
lock = threading.Lock()
stage = {"value": 0}


def tool_call(call_id, name, arguments):
    return {
        "id": call_id,
        "type": "function",
        "function": {
            "name": name,
            "arguments": arguments,
        },
    }


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

        with lock:
            current_stage = stage["value"]
            if current_stage == 0:
                message = {
                    "role": "assistant",
                    "content": None,
                    "tool_calls": [
                        tool_call(
                            "call-create-goal",
                            "create_goal",
                            json.dumps(
                                {
                                    "objective": "verify goal prompt markdown wiring",
                                    "token_budget": 2000,
                                },
                                ensure_ascii=False,
                            ),
                        )
                    ],
                }
                finish_reason = "tool_calls"
                stage["value"] = 1
            elif current_stage == 1:
                message = {
                    "role": "assistant",
                    "content": "Goal created; continuation should start automatically.",
                }
                finish_reason = "stop"
                stage["value"] = 2
            elif current_stage == 2:
                required = [
                    "Work from evidence",
                    "The audit must prove completion",
                    "strict blocked audit",
                    "verify goal prompt markdown wiring",
                    "Tokens remaining:",
                ]
                forbidden = ["Avoid repeating work that is already done"]
                missing = [text for text in required if text not in joined]
                leaked_old = [text for text in forbidden if text in joined]
                if missing or leaked_old:
                    self.send_response(500)
                    self.end_headers()
                    self.wfile.write(
                        json.dumps(
                            {"missing": missing, "leaked_old": leaked_old},
                            ensure_ascii=False,
                        ).encode("utf-8")
                    )
                    return
                message = {
                    "role": "assistant",
                    "content": None,
                    "tool_calls": [
                        tool_call(
                            "call-complete-goal",
                            "update_goal",
                            json.dumps({"status": "complete"}, ensure_ascii=False),
                        )
                    ],
                }
                finish_reason = "tool_calls"
                stage["value"] = 3
            else:
                message = {
                    "role": "assistant",
                    "content": "Goal prompt markdown wiring verified.",
                }
                finish_reason = "stop"
                stage["value"] = 4

        body = {
            "id": f"chatcmpl-goal-prompts-{current_stage}",
            "object": "chat.completion",
            "created": 1,
            "model": "mock-model",
            "choices": [{"index": 0, "message": message, "finish_reason": finish_reason}],
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
  echo "[agent-goal-prompts] skipping build, using $BIFROST_BIN"
else
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
fi

if [[ ! -x "$BIFROST_BIN" ]]; then
  echo "[agent-goal-prompts] bifrost binary not found or not executable: $BIFROST_BIN" >&2
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
    \"model_provider\": \"mock-goal-prompts\",
    \"model\": \"mock-model\",
    \"base_url\": \"http://127.0.0.1:$MOCK_PORT/chat/completions\",
    \"api_key\": \"test-key\",
    \"memories\": {\"use_memories\": false, \"generate_memories\": false}
  }" >/dev/null

curl -fsS --noproxy '*' -X POST "$BASE/chat" \
  -H 'Content-Type: application/json' \
  -d "{\"session_key\":\"goal-prompt-template-human-api\",\"message\":\"创建并完成一个目标\"}" \
  >"$RESPONSE_FILE"

python3 - "$RESPONSE_FILE" "$MOCK_LOG" <<'PY'
import json
import sys
from pathlib import Path

response = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
mock_log = Path(sys.argv[2]).read_text(encoding="utf-8")
assert response.get("success") is True, response
assert "Goal prompt markdown wiring verified" in response.get("response", ""), response
assert "Work from evidence" in mock_log, mock_log
assert "The audit must prove completion" in mock_log, mock_log
assert "strict blocked audit" in mock_log, mock_log
assert "Avoid repeating work that is already done" not in mock_log, mock_log
PY

echo "[agent-goal-prompts] PASS"
