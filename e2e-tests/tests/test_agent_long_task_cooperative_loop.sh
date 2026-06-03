#!/usr/bin/env bash
set -euo pipefail
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_DIR"

pick_free_port() {
  python3 - <<'PY'
import socket
with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
}

BIFROST_PORT="${BIFROST_PORT:-${ADMIN_PORT:-${PROXY_PORT:-$(pick_free_port)}}}"
MOCK_PORT="${MOCK_PORT:-}"
TEST_DIR="$(mktemp -d)"
MOCK_LOG="$TEST_DIR/mock-requests.jsonl"
MOCK_PORT_FILE="$TEST_DIR/mock-port"
BIFROST_LOG="$TEST_DIR/bifrost.log"
RESPONSE_FILE="$TEST_DIR/agent-response.json"
INTERACTIVE_RESPONSE_FILE="$TEST_DIR/agent-interactive-response.json"
FINAL_TEXT="COOPERATIVE_LONG_TASK_LOOP_OK"
INTERACTIVE_FINAL_TEXT="COOPERATIVE_INTERACTIVE_LOOP_OK"

resolve_bifrost_bin() {
  local requested="${1:-}"
  local candidate
  for candidate in \
    "$requested" \
    "${requested}.exe" \
    "$REPO_DIR/target/debug/bifrost" \
    "$REPO_DIR/target/debug/bifrost.exe" \
    "$REPO_DIR/target/release/bifrost" \
    "$REPO_DIR/target/release/bifrost.exe"; do
    if [[ -n "$candidate" && -x "$candidate" ]]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done
  return 1
}

dump_debug() {
  local exit_code="$1"
  if [[ "$exit_code" -ne 0 ]]; then
    echo "[agent-long-task-cooperative-loop] DEBUG: temp dir $TEST_DIR" >&2
    [[ -f "$BIFROST_LOG" ]] && tail -n 160 "$BIFROST_LOG" >&2 || true
    [[ -f "$MOCK_LOG" ]] && tail -n 20 "$MOCK_LOG" >&2 || true
    [[ -f "$RESPONSE_FILE" ]] && cat "$RESPONSE_FILE" >&2 || true
    [[ -f "$INTERACTIVE_RESPONSE_FILE" ]] && cat "$INTERACTIVE_RESPONSE_FILE" >&2 || true
  fi
  return "$exit_code"
}

cleanup() {
  local exit_code="$?"
  dump_debug "$exit_code" || true
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
  echo "[agent-long-task-cooperative-loop] $label did not become ready" >&2
  return 1
}

python3 - "$MOCK_PORT" "$MOCK_LOG" "$MOCK_PORT_FILE" "$FINAL_TEXT" "$INTERACTIVE_FINAL_TEXT" <<'PY' &
import json
import re
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

requested_port = int(sys.argv[1]) if sys.argv[1] else 0
log_path = sys.argv[2]
port_path = sys.argv[3]
final_text = sys.argv[4]
interactive_final_text = sys.argv[5]
state = {"request_no": 0, "interactive_session_id": None}
lock = threading.Lock()


def message_text(message):
    content = message.get("content")
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n".join(part.get("text", "") for part in content if isinstance(part, dict))
    return ""


def tool_call(call_id, name, arguments):
    return {
        "id": call_id,
        "type": "function",
        "function": {"name": name, "arguments": arguments},
    }


def tool_names(payload):
    names = []
    for tool in payload.get("tools") or []:
        function = tool.get("function") or {}
        name = function.get("name")
        if name:
            names.append(name)
    return names


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
        payload = json.loads(body or b"{}")
        with open(log_path, "a", encoding="utf-8") as fh:
            fh.write(json.dumps(payload, ensure_ascii=False) + "\n")

        tool_outputs = [
            message_text(message)
            for message in payload.get("messages", [])
            if message.get("role") == "tool"
        ]
        joined_messages = "\n".join(message_text(message) for message in payload.get("messages", []))
        joined_tools = "\n".join(tool_outputs)
        compact_tools = joined_tools.replace(" ", "")
        latest_tool = tool_outputs[-1] if tool_outputs else ""
        compact_latest_tool = latest_tool.replace(" ", "")

        with lock:
            state["request_no"] += 1

        if "LT_BEGIN" in latest_tool or "LT_DONE" in latest_tool:
            required = [
                "process_exited",
                "LT_BEGIN",
                "LT_DONE",
                '"exit_code":0',
                '"model_request_count_while_waiting":0',
            ]
            missing = [item for item in required if item not in compact_latest_tool and item not in latest_tool]
            if missing:
                self.send_response(500)
                self.end_headers()
                self.wfile.write(json.dumps({"missing_final_summary": missing, "tool_outputs": tool_outputs}, ensure_ascii=False).encode())
                return
            if '"running":true' in compact_latest_tool and '"exit_code":null' in compact_latest_tool:
                self.send_response(500)
                self.end_headers()
                self.wfile.write(json.dumps({"model_was_asked_to_poll_running_session": tool_outputs}, ensure_ascii=False).encode())
                return
            message = {"role": "assistant", "content": final_text}
            finish_reason = "stop"
        elif "E2E_ANSWER=yes" in joined_tools and '"exit_code":0' in compact_latest_tool:
            message = {"role": "assistant", "content": interactive_final_text}
            finish_reason = "stop"
        elif "E2E_CONFIRM?" in latest_tool:
            required = [
                "stalled",
                "interactive",
                "E2E_CONFIRM?",
            ]
            missing = [item for item in required if item not in compact_latest_tool and item not in latest_tool]
            if missing:
                self.send_response(500)
                self.end_headers()
                self.wfile.write(json.dumps({"missing_interactive_stall": missing, "tool_outputs": tool_outputs}, ensure_ascii=False).encode())
                return
            match = re.search(r'"session_id"\s*:\s*"?(\d+)"?', latest_tool)
            if not match:
                self.send_response(500)
                self.end_headers()
                self.wfile.write(json.dumps({"missing_interactive_session_id": tool_outputs}, ensure_ascii=False).encode())
                return
            state["interactive_session_id"] = int(match.group(1))
            message = {
                "role": "assistant",
                "content": None,
                "tool_calls": [
                    tool_call(
                        "call-interactive-stdin",
                        "write_stdin",
                        json.dumps(
                            {
                                "session_id": state["interactive_session_id"],
                                "chars": "yes\n",
                                "yield_time_ms": 1000,
                            },
                            ensure_ascii=False,
                        ),
                    )
                ],
            }
            finish_reason = "tool_calls"
        elif "yes" in latest_tool and state["interactive_session_id"] is not None:
            message = {
                "role": "assistant",
                "content": None,
                "tool_calls": [
                    tool_call(
                        "call-interactive-final-poll",
                        "write_stdin",
                        json.dumps(
                            {
                                "session_id": state["interactive_session_id"],
                                "yield_time_ms": 1000,
                            },
                            ensure_ascii=False,
                        ),
                    )
                ],
            }
            finish_reason = "tool_calls"
        elif "启动一个长任务" in joined_messages:
            names = set(tool_names(payload))
            if "exec_command" not in names or "write_stdin" not in names:
                self.send_response(500)
                self.end_headers()
                self.wfile.write(json.dumps({"missing_tools": sorted(names)}, ensure_ascii=False).encode())
                return
            message = {
                "role": "assistant",
                "content": None,
                "tool_calls": [
                    tool_call(
                        "call-long-task",
                        "exec_command",
                        json.dumps(
                            {
                                "cmd": "python3 -u -c 'import time; print(\"LT_BEGIN\", flush=True); time.sleep(1.2); print(\"LT_DONE\", flush=True)'",
                                "yield_time_ms": 50,
                            },
                            ensure_ascii=False,
                        ),
                    )
                ],
            }
            finish_reason = "tool_calls"
        elif "启动一个交互式终端命令" in joined_messages:
            message = {
                "role": "assistant",
                "content": None,
                "tool_calls": [
                    tool_call(
                        "call-interactive-task",
                        "exec_command",
                        json.dumps(
                            {
                                "cmd": "python3 -u -c 'import sys; print(\"E2E_CONFIRM?\", flush=True); line=sys.stdin.readline(); print(\"E2E_ANSWER=\" + line.strip(), flush=True)'",
                                "tty": True,
                                "yield_time_ms": 50,
                            },
                            ensure_ascii=False,
                        ),
                    )
                ],
            }
            finish_reason = "tool_calls"
        else:
            message = {"role": "assistant", "content": "IGNORED_HELPER_REQUEST"}
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
    echo "[agent-long-task-cooperative-loop] mock model exited before reporting port" >&2
    [[ -f "$MOCK_LOG" ]] && cat "$MOCK_LOG" >&2 || true
    exit 1
  fi
  sleep 0.25
done
if [[ -z "$MOCK_PORT" ]]; then
  echo "[agent-long-task-cooperative-loop] mock model did not report a port" >&2
  exit 1
fi
wait_http "http://127.0.0.1:$MOCK_PORT/health" "mock model"

if [[ "${SKIP_BUILD:-false}" != "true" ]]; then
  echo "[agent-long-task-cooperative-loop] building bifrost"
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi
BIFROST_BIN="$(resolve_bifrost_bin "${BIFROST_BIN:-}")" || {
  echo "[agent-long-task-cooperative-loop] bifrost binary not found" >&2
  exit 1
}

echo "[agent-long-task-cooperative-loop] starting bifrost"
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
curl -fsS --noproxy '*' -X PATCH "$BASE" \
  -H 'Content-Type: application/json' \
  -d "{
    \"enabled\": true,
    \"model_provider\": \"mock-long-task\",
    \"model\": \"mock-model\",
    \"base_url\": \"http://127.0.0.1:$MOCK_PORT/chat/completions\",
    \"api_key\": \"test-key\",
    \"request_timeout_secs\": 30,
    \"max_turn_iterations\": 4,
    \"history\": {\"persistence\": \"save-all\"},
    \"memories\": {\"use_memories\": false, \"generate_memories\": false}
  }" >/dev/null

curl -fsS --noproxy '*' -X POST "$BASE/chat" \
  -H 'Content-Type: application/json' \
  -d '{"session_key":"agent-long-task-cooperative-loop","message":"启动一个长任务并等待它完成。"}' \
  > "$RESPONSE_FILE"

curl -fsS --noproxy '*' -X POST "$BASE/chat" \
  -H 'Content-Type: application/json' \
  -d '{"session_key":"agent-long-task-interactive-loop","message":"启动一个交互式终端命令，看到确认提示后输入 yes。"}' \
  > "$INTERACTIVE_RESPONSE_FILE"

python3 - "$RESPONSE_FILE" "$INTERACTIVE_RESPONSE_FILE" "$MOCK_LOG" "$FINAL_TEXT" "$INTERACTIVE_FINAL_TEXT" <<'PY'
import json
import sys
from pathlib import Path

response = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
interactive_response = json.loads(Path(sys.argv[2]).read_text(encoding="utf-8"))
payloads = [json.loads(line) for line in Path(sys.argv[3]).read_text(encoding="utf-8").splitlines() if line.strip()]
final_text = sys.argv[4]
interactive_final_text = sys.argv[5]

assert response.get("success") is True, response
assert final_text in response.get("response", ""), response
assert len(payloads) >= 5, len(payloads)
tool_calls = response.get("tool_calls") or []
assert [call.get("tool_name") for call in tool_calls] == ["exec_command"], tool_calls
tool_output = tool_calls[0].get("result", "")
assert "process_exited" in tool_output, tool_output
assert "LT_BEGIN" in tool_output and "LT_DONE" in tool_output, tool_output
assert "model_request_count_while_waiting" in tool_output, tool_output

assert interactive_response.get("success") is True, interactive_response
assert interactive_final_text in interactive_response.get("response", ""), interactive_response
interactive_calls = interactive_response.get("tool_calls") or []
interactive_names = [call.get("tool_name") for call in interactive_calls]
assert interactive_names[:2] == ["exec_command", "write_stdin"], interactive_names
assert interactive_names in (
    ["exec_command", "write_stdin"],
    ["exec_command", "write_stdin", "write_stdin"],
), interactive_names
interactive_output = "\n".join(call.get("result", "") for call in interactive_calls)
assert "stalled" in interactive_output, interactive_output
assert "interactive" in interactive_output, interactive_output
assert "E2E_CONFIRM?" in interactive_output, interactive_output
assert "E2E_ANSWER=yes" in interactive_output, interactive_output
assert '"exit_code":0' in interactive_output.replace(" ", ""), interactive_output
PY

echo "[agent-long-task-cooperative-loop] PASS"
