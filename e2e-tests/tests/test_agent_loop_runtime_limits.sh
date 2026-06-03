#!/usr/bin/env bash
set -euo pipefail
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

source "$REPO_DIR/e2e-tests/test_utils/assert.sh"

cd "$REPO_DIR"

BIFROST_PORT="${BIFROST_PORT:-${ADMIN_PORT:-18895}}"
MOCK_PORT="${MOCK_PORT:-${MOCK_HTTP_PORT:-18896}}"
TEST_DIR="$(mktemp -d)"
MOCK_LOG="$TEST_DIR/mock-requests.jsonl"
BIFROST_LOG="$TEST_DIR/bifrost.log"
BIFROST_BIN="${BIFROST_BIN:-}"
RESPONSE_FILE="$TEST_DIR/agent-response.json"
FINAL_TEXT="已完成 35 次 list_directory 调用，默认迭代上限未在 30 次时提前中断。"

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
  for _ in $(seq 1 80); do
    if curl -fsS --noproxy '*' "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.25
  done
  echo "[agent-loop-runtime-limits] $label did not become ready" >&2
  return 1
}

python3 - "$MOCK_PORT" "$MOCK_LOG" "$FINAL_TEXT" <<'PY' &
import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port = int(sys.argv[1])
log_path = sys.argv[2]
final_text = sys.argv[3]
state = {"request_no": 0}
lock = threading.Lock()
TOOL_CALLS_BEFORE_STOP = 35


def tool_call(call_id: str, name: str, arguments: str):
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
        body = self.rfile.read(length)
        try:
            payload = json.loads(body or b"{}")
        except Exception:
            payload = {}

        with open(log_path, "a", encoding="utf-8") as fh:
            fh.write(json.dumps(payload, ensure_ascii=False) + "\n")

        with lock:
            state["request_no"] += 1
            request_no = state["request_no"]

        if request_no <= TOOL_CALLS_BEFORE_STOP:
            message = {
                "role": "assistant",
                "content": None,
                "tool_calls": [
                    tool_call(
                        f"call-list-directory-{request_no}",
                        "list_directory",
                        json.dumps({"path": "design"}, ensure_ascii=False),
                    )
                ],
            }
            finish_reason = "tool_calls"
        else:
            message = {
                "role": "assistant",
                "content": final_text,
            }
            finish_reason = "stop"

        response = {
            "choices": [{"message": message, "finish_reason": finish_reason}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15},
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

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/release/bifrost}"
  echo "[agent-loop-runtime-limits] skipping build, using $BIFROST_BIN"
else
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
  echo "[agent-loop-runtime-limits] building bifrost"
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

echo "[agent-loop-runtime-limits] starting bifrost on $BIFROST_PORT with temp data dir $TEST_DIR"
BIFROST_DATA_DIR="$TEST_DIR" "$BIFROST_BIN" start \
  --host 127.0.0.1 \
  -p "$BIFROST_PORT" \
  --unsafe-ssl \
  --skip-cert-check \
  --no-system-proxy \
  >"$BIFROST_LOG" 2>&1 &
BIFROST_PID=$!
wait_http "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address" "bifrost"

BASE="http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/agent"

echo "[agent-loop-runtime-limits] verifying default agent config"
DEFAULT_CONFIG="$(curl -fsS --noproxy '*' "$BASE")"
assert_json_field ".request_timeout_secs" "600" "$DEFAULT_CONFIG" "default request timeout should be 600 seconds"
assert_json_field ".max_turn_iterations" "1000" "$DEFAULT_CONFIG" "default max turn iterations should be 1000"
assert_json_field ".background_terminal_max_timeout" "300000" "$DEFAULT_CONFIG" "default background terminal timeout should be 300000 ms"

echo "[agent-loop-runtime-limits] configuring agent mock provider without overriding iteration defaults"
curl -fsS --noproxy '*' -X PATCH "$BASE" \
  -H 'Content-Type: application/json' \
  -d "{
    \"enabled\": true,
    \"model_provider\": \"mock-runtime-limits\",
    \"model\": \"mock-model\",
    \"base_url\": \"http://127.0.0.1:$MOCK_PORT/chat/completions\",
    \"api_key\": \"test-key\",
    \"memories\": {
      \"use_memories\": false,
      \"generate_memories\": false
    }
  }" >/dev/null

UPDATED_CONFIG="$(curl -fsS --noproxy '*' "$BASE")"
assert_json_field ".max_turn_iterations" "1000" "$UPDATED_CONFIG" "patched config should still retain default iteration ceiling"

echo "[agent-loop-runtime-limits] calling real agent chat API"
curl -fsS --noproxy '*' -X POST "$BASE/chat" \
  -H 'Content-Type: application/json' \
  -d '{"session_key":"agent-runtime-limits","message":"请连续检查 design 目录 35 次，然后告诉我是否在 30 次迭代前被中断。"}' \
  > "$RESPONSE_FILE"

python3 - "$RESPONSE_FILE" "$MOCK_LOG" "$FINAL_TEXT" <<'PY'
import json
import sys
from pathlib import Path

response_path = Path(sys.argv[1])
mock_log_path = Path(sys.argv[2])
final_text = sys.argv[3]

response = json.loads(response_path.read_text(encoding="utf-8"))
assert response.get("success") is True, response
assert final_text in response.get("response", ""), response
assert "exceeded maximum iterations (30)" not in response.get("response", ""), response

tool_calls = response.get("tool_calls")
assert isinstance(tool_calls, list), response
assert len(tool_calls) >= 35, len(tool_calls)
assert sum(1 for call in tool_calls if call.get("tool_name") == "list_directory") >= 35, tool_calls

payloads = [json.loads(line) for line in mock_log_path.read_text(encoding="utf-8").splitlines() if line.strip()]
assert len(payloads) >= 36, len(payloads)
PY

if grep -q "exceeded maximum iterations (30)" "$BIFROST_LOG"; then
  echo "[agent-loop-runtime-limits] ASSERT FAILED: bifrost log still contains exceeded maximum iterations (30)" >&2
  tail -n 100 "$BIFROST_LOG" >&2 || true
  exit 1
fi

echo ""
echo "Results: $PASSED_ASSERTIONS passed, $FAILED_ASSERTIONS failed"
if [ "$FAILED_ASSERTIONS" -gt 0 ]; then
  exit 1
fi

echo "[agent-loop-runtime-limits] PASS"
