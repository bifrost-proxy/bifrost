#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

cd "$REPO_DIR"

BIFROST_PORT="${BIFROST_PORT:-${ADMIN_PORT:-18901}}"
MOCK_PORT="${MOCK_PORT:-${MOCK_HTTP_PORT:-18902}}"
TEST_DIR="$(mktemp -d)"
MOCK_LOG="$TEST_DIR/mock-requests.jsonl"
BIFROST_LOG="$TEST_DIR/bifrost.log"
MCP_SERVER="$TEST_DIR/mcp_resource_server.py"
BIFROST_BIN="${BIFROST_BIN:-}"
RESPONSE_FILE="$TEST_DIR/agent-response.json"
FINAL_TEXT="CHAT_CODEX_ALIGNMENT_OK: parallel tools, ordered state tools, tool_search, and MCP resources completed."

resolve_bifrost_bin() {
  local preferred="$1"

  if [[ -n "$preferred" && -x "$preferred" ]]; then
    printf '%s\n' "$preferred"
    return 0
  fi
  if [[ -n "$preferred" && ! -x "$preferred" && -x "${preferred}.exe" ]]; then
    printf '%s\n' "${preferred}.exe"
    return 0
  fi

  for candidate in \
    "$REPO_DIR/target/release/bifrost" \
    "$REPO_DIR/target/release/bifrost.exe" \
    "$REPO_DIR/target/debug/bifrost" \
    "$REPO_DIR/target/debug/bifrost.exe"; do
    if [[ -x "$candidate" ]]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done

  if [[ -n "$preferred" ]]; then
    printf '%s\n' "$preferred"
  else
    printf '%s\n' "$REPO_DIR/target/release/bifrost"
  fi
}

dump_debug() {
  local exit_code="$1"
  if [[ "$exit_code" -ne 0 ]]; then
    echo "[agent-codex-alignment-chat-api] DEBUG: temp dir $TEST_DIR" >&2
    if [[ -f "$BIFROST_LOG" ]]; then
      echo "[agent-codex-alignment-chat-api] DEBUG: bifrost log tail" >&2
      tail -n 160 "$BIFROST_LOG" >&2 || true
    fi
    if [[ -f "$MOCK_LOG" ]]; then
      echo "[agent-codex-alignment-chat-api] DEBUG: mock log tail" >&2
      tail -n 20 "$MOCK_LOG" >&2 || true
    fi
    if [[ -f "$RESPONSE_FILE" ]]; then
      echo "[agent-codex-alignment-chat-api] DEBUG: response body" >&2
      cat "$RESPONSE_FILE" >&2 || true
    fi
  fi
  return "$exit_code"
}

cleanup() {
  local exit_code="$?"
  dump_debug "$exit_code" || true
  if [[ -n "${BIFROST_PID:-}" ]]; then
    kill "$BIFROST_PID" >/dev/null 2>&1 || true
    wait "$BIFROST_PID" >/dev/null 2>&1 || true
  fi
  if [[ -n "${MOCK_PID:-}" ]]; then
    kill "$MOCK_PID" >/dev/null 2>&1 || true
    wait "$MOCK_PID" >/dev/null 2>&1 || true
  fi
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
  echo "[agent-codex-alignment-chat-api] $label did not become ready" >&2
  return 1
}

cat >"$MCP_SERVER" <<'PY'
#!/usr/bin/env python3
import json
import sys


def emit(message):
    sys.stdout.write(json.dumps(message, ensure_ascii=False) + "\n")
    sys.stdout.flush()


for line in sys.stdin:
    try:
        request = json.loads(line)
    except Exception:
        continue

    method = request.get("method")
    request_id = request.get("id")

    if request_id is None:
        continue

    if method == "initialize":
        emit({
            "jsonrpc": "2.0",
            "id": request_id,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {"resources": {}, "tools": {}},
                "serverInfo": {"name": "mcpfixture", "version": "1.0.0"},
            },
        })
    elif method == "tools/list":
        tools = []
        for idx in range(100):
            tools.append({
                "name": f"sample_tool_{idx:03d}",
                "description": f"Deferred visibility fixture sample tool {idx:03d}.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "additionalProperties": False,
                },
            })
        emit({
            "jsonrpc": "2.0",
            "id": request_id,
            "result": {"tools": tools},
        })
    elif method == "resources/list":
        emit({
            "jsonrpc": "2.0",
            "id": request_id,
            "result": {
                "resources": [
                    {
                        "uri": "bifrost://codex-alignment",
                        "name": "Codex alignment fixture",
                        "mimeType": "text/plain",
                    }
                ]
            },
        })
    elif method == "resources/templates/list":
        emit({
            "jsonrpc": "2.0",
            "id": request_id,
            "result": {"resourceTemplates": []},
        })
    elif method == "resources/read":
        emit({
            "jsonrpc": "2.0",
            "id": request_id,
            "result": {
                "contents": [
                    {
                        "uri": "bifrost://codex-alignment",
                        "mimeType": "text/plain",
                        "text": "MCP_RESOURCE_OK",
                    }
                ]
            },
        })
    elif method == "shutdown":
        emit({"jsonrpc": "2.0", "id": request_id, "result": {}})
    else:
        emit({
            "jsonrpc": "2.0",
            "id": request_id,
            "error": {"code": -32601, "message": f"unknown method: {method}"},
        })
PY
chmod +x "$MCP_SERVER"

python3 - "$MOCK_PORT" "$MOCK_LOG" "$FINAL_TEXT" <<'PY' &
import json
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port = int(sys.argv[1])
log_path = sys.argv[2]
final_text = sys.argv[3]
state = {"request_no": 0}
lock = threading.Lock()


def tool_call(call_id, name, arguments):
    return {
        "id": call_id,
        "type": "function",
        "function": {"name": name, "arguments": arguments},
    }


def message_text(message):
    content = message.get("content")
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n".join(part.get("text", "") for part in content if isinstance(part, dict))
    return ""


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
        try:
            payload = json.loads(body or b"{}")
        except Exception:
            payload = {}

        with open(log_path, "a", encoding="utf-8") as fh:
            fh.write(json.dumps(payload, ensure_ascii=False) + "\n")

        with lock:
            state["request_no"] += 1
            request_no = state["request_no"]

        if request_no == 1:
            names = tool_names(payload)
            joined = "\n".join(message_text(message) for message in payload.get("messages", []))
            required_tools = {
                "exec_command",
                "write_stdin",
                "update_plan",
                "set_title",
                "tool_search",
                "list_mcp_resources",
                "list_mcp_resource_templates",
                "read_mcp_resource",
            }
            missing = sorted(required_tools - set(names))
            forbidden = sorted({"shell", "shell_pty", "shell_command", "local_shell"} & set(names))
            leaked_deferred = sorted(name for name in names if name.startswith("mcp_mcpfixture__sample_tool_"))
            leaked_internal_notes = [
                phrase
                for phrase in (
                    "mirrors Codex",
                    "permission escalation",
                    "internal implementation metadata",
                )
                if phrase in joined
            ]
            if missing or forbidden or leaked_deferred or leaked_internal_notes:
                self.send_response(500)
                self.end_headers()
                self.wfile.write(
                    json.dumps(
                        {
                            "missing": missing,
                            "forbidden": forbidden,
                            "leaked_deferred": leaked_deferred[:5],
                            "leaked_internal_notes": leaked_internal_notes,
                            "tools": names,
                        },
                        ensure_ascii=False,
                    ).encode("utf-8")
                )
                return

            message = {
                "role": "assistant",
                "content": None,
                "tool_calls": [
                    tool_call(
                        "call-parallel-a",
                        "exec_command",
                        json.dumps(
                            {
                                "cmd": "python3 -c 'import time; time.sleep(1.5); print(\"parallel-a\")'",
                                "yield_time_ms": 10000
                            },
                            ensure_ascii=False,
                        ),
                    ),
                    tool_call(
                        "call-parallel-b",
                        "exec_command",
                        json.dumps(
                            {
                                "cmd": "python3 -c 'import time; time.sleep(1.5); print(\"parallel-b\")'",
                                "yield_time_ms": 10000
                            },
                            ensure_ascii=False,
                        ),
                    ),
                    tool_call(
                        "call-plan",
                        "update_plan",
                        json.dumps(
                            {
                                "explanation": "Real chat API Codex alignment coverage.",
                                "plan": [
                                    {"step": "Run parallel tools", "status": "completed"},
                                    {"step": "Read MCP resource", "status": "completed"},
                                ],
                            },
                            ensure_ascii=False,
                        ),
                    ),
                    tool_call(
                        "call-title",
                        "set_title",
                        json.dumps({"title": "Codex Alignment Real Chat"}, ensure_ascii=False),
                    ),
                    tool_call(
                        "call-tool-search",
                        "tool_search",
                        json.dumps({"query": "sample_tool_042", "limit": 5}, ensure_ascii=False),
                    ),
                    tool_call(
                        "call-list-resources",
                        "list_mcp_resources",
                        json.dumps({"server": "mcpfixture"}, ensure_ascii=False),
                    ),
                    tool_call(
                        "call-read-resource",
                        "read_mcp_resource",
                        json.dumps(
                            {
                                "server": "mcpfixture",
                                "uri": "bifrost://codex-alignment",
                            },
                            ensure_ascii=False,
                        ),
                    ),
                ],
            }
            finish_reason = "tool_calls"
        elif request_no == 2:
            messages = payload.get("messages", [])
            tool_outputs = [message_text(message) for message in messages if message.get("role") == "tool"]
            expected_fragments = [
                "parallel-a",
                "parallel-b",
                "Run parallel tools",
                "SET_TITLE:Codex Alignment Real Chat",
                "sample_tool_042",
                "bifrost://codex-alignment",
                "MCP_RESOURCE_OK",
            ]
            mismatches = [
                {"index": idx, "expected": fragment, "actual": tool_outputs[idx] if idx < len(tool_outputs) else None}
                for idx, fragment in enumerate(expected_fragments)
                if idx >= len(tool_outputs) or fragment not in tool_outputs[idx]
            ]
            if mismatches:
                self.send_response(500)
                self.end_headers()
                self.wfile.write(
                    json.dumps({"mismatches": mismatches, "tool_outputs": tool_outputs}, ensure_ascii=False).encode("utf-8")
                )
                return

            message = {
                "role": "assistant",
                "content": None,
                "tool_calls": [
                    tool_call(
                        "call-background-watch",
                        "exec_command",
                        json.dumps(
                            {
                                "cmd": "python3 -u -c 'import time; print(\"WATCH_BEGIN\", flush=True); time.sleep(0.8); print(\"WATCH_DONE\", flush=True)'",
                                "yield_time_ms": 50,
                            },
                            ensure_ascii=False,
                        ),
                    ),
                ],
            }
            finish_reason = "tool_calls"
        elif request_no == 3:
            messages = payload.get("messages", [])
            tool_outputs = [message_text(message) for message in messages if message.get("role") == "tool"]
            if not tool_outputs or "\"session_id\":" not in tool_outputs[-1] or "WATCH_BEGIN" not in tool_outputs[-1]:
                self.send_response(500)
                self.end_headers()
                self.wfile.write(
                    json.dumps({"missing_background_session": tool_outputs[-1:]}, ensure_ascii=False).encode("utf-8")
                )
                return
            try:
                session_id = json.loads(tool_outputs[-1])["session_id"]
            except Exception as exc:
                self.send_response(500)
                self.end_headers()
                self.wfile.write(json.dumps({"bad_session_json": tool_outputs[-1], "error": str(exc)}, ensure_ascii=False).encode("utf-8"))
                return
            time.sleep(1.0)
            message = {
                "role": "assistant",
                "content": None,
                "tool_calls": [
                    tool_call(
                        "call-background-poll",
                        "write_stdin",
                        json.dumps(
                            {
                                "session_id": session_id,
                                "chars": "",
                                "yield_time_ms": 1,
                            },
                            ensure_ascii=False,
                        ),
                    ),
                ],
            }
            finish_reason = "tool_calls"
        else:
            messages = payload.get("messages", [])
            tool_outputs = [message_text(message) for message in messages if message.get("role") == "tool"]
            if not tool_outputs or "WATCH_DONE" not in tool_outputs[-1] or "\"exit_code\":0" not in tool_outputs[-1].replace(" ", ""):
                self.send_response(500)
                self.end_headers()
                self.wfile.write(
                    json.dumps({"missing_background_completion": tool_outputs[-2:]}, ensure_ascii=False).encode("utf-8")
                )
                return

            message = {"role": "assistant", "content": final_text}
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
  BIFROST_BIN="$(resolve_bifrost_bin "$BIFROST_BIN")"
  echo "[agent-codex-alignment-chat-api] skipping build, using $BIFROST_BIN"
else
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
  echo "[agent-codex-alignment-chat-api] building bifrost"
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
  BIFROST_BIN="$(resolve_bifrost_bin "$BIFROST_BIN")"
fi

if [[ ! -x "$BIFROST_BIN" ]]; then
  echo "[agent-codex-alignment-chat-api] bifrost binary not found or not executable: $BIFROST_BIN" >&2
  exit 1
fi

echo "[agent-codex-alignment-chat-api] starting bifrost on $BIFROST_PORT with temp data dir $TEST_DIR"
BIFROST_DATA_DIR="$TEST_DIR" "$BIFROST_BIN" start \
  --host 127.0.0.1 \
  -p "$BIFROST_PORT" \
  --unsafe-ssl \
  --no-system-proxy \
  >"$BIFROST_LOG" 2>&1 &
BIFROST_PID=$!
wait_http "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address" "bifrost"

BASE="http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/agent"

echo "[agent-codex-alignment-chat-api] configuring agent mock provider and MCP fixture"
curl -fsS --noproxy '*' -X PATCH "$BASE" \
  -H 'Content-Type: application/json' \
  -d "{
    \"enabled\": true,
    \"model_provider\": \"mock-codex-alignment\",
    \"model\": \"mock-model\",
    \"base_url\": \"http://127.0.0.1:$MOCK_PORT/chat/completions\",
    \"api_key\": \"test-key\",
    \"request_timeout_secs\": 30,
    \"max_turn_iterations\": 6,
    \"history\": {\"persistence\": \"save-all\"},
    \"memories\": {\"use_memories\": false, \"generate_memories\": false},
    \"mcp_servers\": {
      \"mcpfixture\": {
        \"command\": \"python3\",
        \"args\": [\"$MCP_SERVER\"],
        \"enabled\": true,
        \"startup_timeout_sec\": 10,
        \"tool_timeout_sec\": 10
      }
    }
  }" >/dev/null

START_MS="$(python3 - <<'PY'
import time
print(int(time.monotonic() * 1000))
PY
)"

echo "[agent-codex-alignment-chat-api] calling real agent chat API"
curl -fsS --noproxy '*' -X POST "$BASE/chat" \
  -H 'Content-Type: application/json' \
  -d '{"session_key":"agent-codex-alignment-chat-api","message":"请通过真实工具链完成 Codex alignment 覆盖测试。"}' \
  > "$RESPONSE_FILE"

END_MS="$(python3 - <<'PY'
import time
print(int(time.monotonic() * 1000))
PY
)"
ELAPSED_MS=$((END_MS - START_MS))

python3 - "$RESPONSE_FILE" "$MOCK_LOG" "$FINAL_TEXT" "$ELAPSED_MS" <<'PY'
import json
import sys
from pathlib import Path

response_path = Path(sys.argv[1])
mock_log_path = Path(sys.argv[2])
final_text = sys.argv[3]
elapsed_ms = int(sys.argv[4])

response = json.loads(response_path.read_text(encoding="utf-8"))
assert response.get("success") is True, response
assert final_text in response.get("response", ""), response

tool_calls = response.get("tool_calls")
assert isinstance(tool_calls, list), response
names = [call.get("tool_name") for call in tool_calls]
expected_names = [
    "exec_command",
    "exec_command",
    "update_plan",
    "set_title",
    "tool_search",
    "list_mcp_resources",
    "read_mcp_resource",
    "exec_command",
    "write_stdin",
]
assert names[: len(expected_names)] == expected_names, names
assert all(call.get("success") is True for call in tool_calls[: len(expected_names)]), tool_calls

plan_steps = response.get("plan_steps")
assert isinstance(plan_steps, list), response
assert [step.get("status") for step in plan_steps] == ["completed", "completed"], plan_steps

payloads = [json.loads(line) for line in mock_log_path.read_text(encoding="utf-8").splitlines() if line.strip()]
assert len(payloads) >= 2, len(payloads)
first_tool_names = [
    (tool.get("function") or {}).get("name")
    for tool in payloads[0].get("tools", [])
    if isinstance(tool, dict)
]
assert "list_mcp_resources" in first_tool_names, first_tool_names
assert "read_mcp_resource" in first_tool_names, first_tool_names
assert "tool_search" in first_tool_names, first_tool_names
assert "write_stdin" in first_tool_names, first_tool_names
assert "shell_command" not in first_tool_names, first_tool_names
assert "local_shell" not in first_tool_names, first_tool_names
assert not any(name.startswith("mcp_mcpfixture__sample_tool_") for name in first_tool_names if name), first_tool_names

# Two exec_command calls are in one model tool-call batch; the rest of this
# scenario adds an intentional background-completion wait. Sequential first-batch
# execution is comfortably above this budget on the same local fixture.
assert elapsed_ms < 7000, f"parallel/background exec_command scenario took too long: {elapsed_ms}ms"
PY

if grep -q "agent chat api failed" "$BIFROST_LOG"; then
  echo "[agent-codex-alignment-chat-api] ASSERT FAILED: service log contains chat failure" >&2
  tail -n 120 "$BIFROST_LOG" >&2 || true
  exit 1
fi

echo "[agent-codex-alignment-chat-api] PASS elapsed_ms=$ELAPSED_MS"
