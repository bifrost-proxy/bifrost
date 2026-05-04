#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

cd "$REPO_DIR"

BIFROST_PORT="${BIFROST_PORT:-18891}"
MOCK_PORT="${MOCK_PORT:-18892}"
TEST_DIR="$(mktemp -d)"
MOCK_LOG="$TEST_DIR/mock-requests.jsonl"
BIFROST_LOG="$TEST_DIR/bifrost.log"
GATE_PROMPT="You already have an active task plan. Before concluding, call update_plan to reflect the final task state. If the work is complete, mark all steps as completed."

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
  echo "[update-plan-human-api] $label did not become ready" >&2
  return 1
}

python3 - "$MOCK_PORT" "$MOCK_LOG" "$GATE_PROMPT" <<'PY' &
import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port = int(sys.argv[1])
log_path = sys.argv[2]
gate_prompt = sys.argv[3]
stage = {"value": 0}
lock = threading.Lock()


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
                            "call-plan-open",
                            "update_plan",
                            json.dumps(
                                {
                                    "explanation": "Start working",
                                    "plan": [
                                        {"step": "Inspect workspace", "status": "completed"},
                                        {"step": "Summarize findings", "status": "in_progress"},
                                    ],
                                },
                                ensure_ascii=False,
                            ),
                        ),
                        tool_call(
                            "call-list-dir",
                            "list_directory",
                            json.dumps({"path": "."}, ensure_ascii=False),
                        ),
                    ],
                }
                finish_reason = "tool_calls"
                stage["value"] = 1
            elif current_stage == 1:
                message = {
                    "role": "assistant",
                    "content": "工作已经完成，我现在直接给出最终总结。",
                }
                finish_reason = "stop"
                stage["value"] = 2
            elif current_stage == 2:
                if gate_prompt not in joined:
                    self.send_response(500)
                    self.end_headers()
                    self.wfile.write(b"missing runtime gate prompt")
                    return
                message = {
                    "role": "assistant",
                    "content": None,
                    "tool_calls": [
                        tool_call(
                            "call-plan-close",
                            "update_plan",
                            json.dumps(
                                {
                                    "explanation": "All work finished after runtime reminder",
                                    "plan": [
                                        {"step": "Inspect workspace", "status": "completed"},
                                        {"step": "Summarize findings", "status": "completed"},
                                    ],
                                },
                                ensure_ascii=False,
                            ),
                        )
                    ],
                }
                finish_reason = "tool_calls"
                stage["value"] = 3
            else:
                message = {
                    "role": "assistant",
                    "content": "最终总结：runtime 已经强制我先收口 update_plan，然后才允许结束。",
                }
                finish_reason = "stop"
                stage["value"] = 4

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

echo "[update-plan-human-api] building bifrost"
SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost

echo "[update-plan-human-api] starting bifrost on $BIFROST_PORT with temp data dir $TEST_DIR"
BIFROST_DATA_DIR="$TEST_DIR" ./target/debug/bifrost start \
  --host 127.0.0.1 \
  -p "$BIFROST_PORT" \
  --unsafe-ssl \
  --no-system-proxy \
  >"$BIFROST_LOG" 2>&1 &
BIFROST_PID=$!
wait_http "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address" "bifrost"

BASE="http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/agent"

echo "[update-plan-human-api] configuring agent mock provider"
curl -fsS --noproxy '*' -X PATCH "$BASE" \
  -H 'Content-Type: application/json' \
  -d "{
    \"enabled\": true,
    \"model_provider\": \"mock-update-plan\",
    \"model\": \"mock-model\",
    \"base_url\": \"http://127.0.0.1:$MOCK_PORT/chat/completions\",
    \"api_key\": \"test-key\",
    \"request_timeout_secs\": 20,
    \"max_turn_iterations\": 6,
    \"history\": {\"persistence\": \"save-all\"},
    \"memories\": {
      \"use_memories\": false,
      \"generate_memories\": false
    }
  }" >/dev/null

echo "[update-plan-human-api] calling real agent chat API"
RESPONSE_FILE="$TEST_DIR/agent-response.json"
curl -fsS --noproxy '*' -X POST "$BASE/chat" \
  -H 'Content-Type: application/json' \
  -d '{"session_key":"update-plan-human-api","message":"请检查当前工作区，然后给我一个简短总结。"}' \
  > "$RESPONSE_FILE"

python3 - "$RESPONSE_FILE" "$MOCK_LOG" "$GATE_PROMPT" <<'PY'
import json
import sys
from pathlib import Path

response_path = Path(sys.argv[1])
mock_log_path = Path(sys.argv[2])
gate_prompt = sys.argv[3]

response = json.loads(response_path.read_text(encoding="utf-8"))
assert response.get("success") is True, response
assert "runtime 已经强制我先收口 update_plan" in response.get("response", ""), response

plan_steps = response.get("plan_steps")
assert isinstance(plan_steps, list) and len(plan_steps) == 2, response
assert all(step.get("status") == "completed" for step in plan_steps), plan_steps
assert [step.get("step") for step in plan_steps] == ["Inspect workspace", "Summarize findings"], plan_steps

tool_calls = response.get("tool_calls")
assert isinstance(tool_calls, list), response
update_plan_calls = [call for call in tool_calls if call.get("tool_name") == "update_plan"]
assert len(update_plan_calls) >= 2, tool_calls

payloads = [json.loads(line) for line in mock_log_path.read_text(encoding="utf-8").splitlines() if line.strip()]
assert len(payloads) >= 4, len(payloads)
assert any(
    gate_prompt in (message.get("content") or "")
    for payload in payloads
    for message in payload.get("messages", [])
    if isinstance(message.get("content"), str)
), payloads
PY

echo "[update-plan-human-api] PASS"
