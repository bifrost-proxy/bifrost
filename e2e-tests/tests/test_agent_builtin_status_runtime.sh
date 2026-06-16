#!/usr/bin/env bash
set -euo pipefail
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

cd "$REPO_DIR"

BIFROST_PORT="${BIFROST_PORT:-${ADMIN_PORT:-18897}}"
MOCK_PORT="${MOCK_PORT:-${MOCK_HTTP_PORT:-18898}}"
TEST_DIR="$(mktemp -d)"
MOCK_LOG="$TEST_DIR/mock-requests.jsonl"
BIFROST_LOG="$TEST_DIR/bifrost.log"
BIFROST_BIN="${BIFROST_BIN:-}"
CHAT_RESPONSE="$TEST_DIR/chat-response.json"
STATUS_RESPONSE="$TEST_DIR/status-response.json"
IDLE_STATUS_RESPONSE="$TEST_DIR/idle-status-response.json"
TOOL_CHAT_RESPONSE="$TEST_DIR/tool-chat-response.json"
TOOL_STATUS_RESPONSE="$TEST_DIR/tool-status-response.json"
TOOL_ACTIVE_STATUS_RESPONSE="$TEST_DIR/tool-active-status-response.json"
TOOL_SESSIONS_RESPONSE="$TEST_DIR/tool-sessions-response.json"
TOOL_DETAIL_RESPONSE="$TEST_DIR/tool-detail-response.json"
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

wait_for_mock_log() {
  local needle="$1"
  local label="$2"
  for _ in $(seq 1 160); do
    if [[ -f "$MOCK_LOG" ]] && grep -Fq "$needle" "$MOCK_LOG"; then
      return 0
    fi
    sleep 0.25
  done
  echo "[agent-builtin-status-runtime] mock model did not observe $label" >&2
  if [[ -f "$MOCK_LOG" ]]; then
    tail -n 20 "$MOCK_LOG" >&2 || true
  fi
  if [[ -s "$TOOL_CHAT_RESPONSE" ]]; then
    cat "$TOOL_CHAT_RESPONSE" >&2 || true
  fi
  return 1
}

python3 - "$MOCK_PORT" "$MOCK_LOG" <<'PY' &
import json
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port = int(sys.argv[1])
log_path = sys.argv[2]


def message_text(message):
    content = message.get("content")
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        parts = []
        for item in content:
            if isinstance(item, dict):
                text = item.get("text")
                if isinstance(text, str):
                    parts.append(text)
        return "\n".join(parts)
    return ""


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
        body = self.rfile.read(length)
        try:
            payload = json.loads(body or b"{}")
        except Exception:
            payload = {}

        with open(log_path, "a", encoding="utf-8") as fh:
            fh.write(json.dumps(payload, ensure_ascii=False) + "\n")

        messages = payload.get("messages", [])
        joined = "\n".join(
            message_text(message)
            for message in messages
            if isinstance(message, dict)
        )
        is_compaction_request = any(
            isinstance(message, dict)
            and "CONTEXT CHECKPOINT COMPACTION" in message_text(message)
            for message in messages
        )
        has_compacted_summary = any(
            isinstance(message, dict)
            and (
                "Another language model started to solve this problem" in message_text(message)
                or "Another language model started to work on this task" in message_text(message)
            )
            for message in messages
        )
        has_tool_result = any(
            isinstance(message, dict) and message.get("role") == "tool"
            for message in messages
        )

        if is_compaction_request:
            response = {
                "choices": [
                    {
                        "message": {
                            "role": "assistant",
                            "content": "large tool output summarized for continuation",
                        },
                        "finish_reason": "stop",
                    }
                ],
                "usage": {"prompt_tokens": 70, "completion_tokens": 7, "total_tokens": 77},
            }
        elif "触发大输出工具" in joined and has_compacted_summary:
            # Keep the second-loop status observable under slower CI runners.
            time.sleep(15.0)
            response = {
                "choices": [
                    {
                        "message": {
                            "role": "assistant",
                            "content": "large tool turn finished after compacted context",
                        },
                        "finish_reason": "stop",
                    }
                ],
                "usage": {"prompt_tokens": 12, "completion_tokens": 5, "total_tokens": 17},
            }
        elif "触发大输出工具" in joined and not has_tool_result:
            response = {
                "choices": [
                    {
                        "message": {
                            "role": "assistant",
                            "content": None,
                            "tool_calls": [
                                tool_call(
                                    "call-large-output",
                                    "exec_command",
                                    json.dumps(
                                        {
                                            "cmd": "source ~/.zshrc; python3 -c 'print(\"CTX\" * 20000)'",
                                            "yield_time_ms": 10000,
                                            "max_output_tokens": 30000,
                                        },
                                        ensure_ascii=False,
                                    ),
                                )
                            ],
                        },
                        "finish_reason": "tool_calls",
                    }
                ],
                "usage": {"prompt_tokens": 12, "completion_tokens": 5, "total_tokens": 17},
            }
        elif "触发大输出工具" in joined and has_tool_result:
            time.sleep(3.0)
            response = {
                "choices": [
                    {
                        "message": {
                            "role": "assistant",
                            "content": "large tool turn finished",
                        },
                        "finish_reason": "stop",
                    }
                ],
                "usage": {"prompt_tokens": 12, "completion_tokens": 5, "total_tokens": 17},
            }
        elif "请等待 mock 模型完成" in joined:
            # Keep the first-loop status observable under slower CI runners.
            time.sleep(15.0)
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
        elif "历史保留探针" in joined:
            response = {
                "choices": [
                    {
                        "message": {
                            "role": "assistant",
                            "content": "history probe acknowledged",
                        },
                        "finish_reason": "stop",
                    }
                ],
                "usage": {"prompt_tokens": 12, "completion_tokens": 5, "total_tokens": 17},
            }
        else:
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

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/release/bifrost}"
  echo "[agent-builtin-status-runtime] skipping build, using $BIFROST_BIN"
else
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
  echo "[agent-builtin-status-runtime] building bifrost"
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

echo "[agent-builtin-status-runtime] starting bifrost on $BIFROST_PORT"
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
    \"model_auto_compact_token_limit\": 2000,
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

wait_for_mock_log "请等待 mock 模型完成" "the first-loop model request"

echo "[agent-builtin-status-runtime] querying /status while turn is active"
for _ in $(seq 1 160); do
  curl -fsS --noproxy '*' -X POST "$BASE/chat" \
    -H 'Content-Type: application/json' \
    -d "{\"session_key\":\"agent-status-runtime\",\"work_dir\":\"$WORK_DIR\",\"message\":\"/status\"}" \
    >"$STATUS_RESPONSE"
  if python3 - "$STATUS_RESPONSE" "$WORK_DIR" <<'PY'
import json
import sys
from pathlib import Path

response = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
active = response.get("active_status")
if (
    response.get("success") is True
    and isinstance(active, dict)
    and active.get("session_key") == "agent-status-runtime"
    and active.get("work_dir") == sys.argv[2]
    and active.get("current_loop_iteration") == 1
):
    raise SystemExit(0)
raise SystemExit(1)
PY
  then
    break
  fi
  sleep 0.25
done

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
assert "Agent 类型: Bifrost Agent" in text, text
assert "Runner 类型: bifrost_agent" in text, text
assert "历史对话轮次:" in text, text
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
assert active.get("last_response_tokens") in (None, 0), active
assert active.get("total_tokens_used") in (None, 0), active
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
assert "Agent 类型: Bifrost Agent" in text, text
assert "Runner 类型: bifrost_agent" in text, text
assert "历史对话轮次:" in text, text
assert "API 累计 token:" in text, text
assert "Context 用量:" in text, text
assert "API 累计 token: 17" in text, text
assert "Context 用量: ~12 / 250K" in text, text
assert "显式压缩次数: 0" in text, text
assert "上下文管理: 按 token/context budget 与 compaction 管理" in text, text
assert "常规请求使用完整 history：2 条" in text, text
assert "\n- 压缩次数:" not in text, text
PY

echo "[agent-builtin-status-runtime] verifying long history is not count-trimmed"
for i in $(seq -w 0 54); do
  curl -fsS --noproxy '*' -X POST "$BASE/chat" \
    -H 'Content-Type: application/json' \
    -d "{\"session_key\":\"agent-status-runtime-long-history\",\"work_dir\":\"$WORK_DIR\",\"message\":\"历史保留探针 $i\"}" \
    >"$CHAT_RESPONSE"
done

curl -fsS --noproxy '*' -X POST "$BASE/chat" \
  -H 'Content-Type: application/json' \
  -d "{\"session_key\":\"agent-status-runtime-long-history\",\"work_dir\":\"$WORK_DIR\",\"message\":\"请报告历史保留探针。\"}" \
  >"$CHAT_RESPONSE"

python3 - "$CHAT_RESPONSE" "$MOCK_LOG" <<'PY'
import json
import sys
from pathlib import Path

response = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
assert response.get("success") is True, response

entries = [
    json.loads(line)
    for line in Path(sys.argv[2]).read_text(encoding="utf-8").splitlines()
    if line.strip()
]
probe_requests = []
for payload in entries:
    messages = payload.get("messages", [])
    joined = "\n".join(
        message.get("content", "")
        for message in messages
        if isinstance(message, dict) and isinstance(message.get("content"), str)
    )
    if "请报告历史保留探针" in joined:
        probe_requests.append((payload, joined))
assert probe_requests, "missing final long-history probe request"
payload, joined = probe_requests[-1]
messages = payload.get("messages", [])
assert len(messages) > 50, len(messages)
assert "历史保留探针 00" in joined, joined
assert "历史保留探针 54" in joined, joined
PY

echo "[agent-builtin-status-runtime] starting a tool-output growth request"
curl -fsS --noproxy '*' -X POST "$BASE/chat" \
  -H 'Content-Type: application/json' \
  -d "{\"session_key\":\"agent-status-runtime-tool-growth\",\"work_dir\":\"$WORK_DIR\",\"message\":\"触发大输出工具，然后等待 mock 模型完成。\"}" \
  >"$TOOL_CHAT_RESPONSE" &
CHAT_PID=$!

wait_for_mock_log "Another language model started" "the compacted second-loop model request"

for _ in $(seq 1 160); do
  curl -fsS --noproxy '*' -X POST "$BASE/chat" \
    -H 'Content-Type: application/json' \
    -d "{\"session_key\":\"agent-status-runtime-tool-growth\",\"work_dir\":\"$WORK_DIR\",\"message\":\"/status\"}" \
    >"$TOOL_STATUS_RESPONSE"
  if python3 - "$TOOL_STATUS_RESPONSE" "$TOOL_ACTIVE_STATUS_RESPONSE" <<'PY'
import json
import sys
from pathlib import Path

response = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
active_path = Path(sys.argv[2])
active = response.get("active_status")
if isinstance(active, dict):
    active_path.write_text(json.dumps(response, ensure_ascii=False), encoding="utf-8")
    if active.get("current_loop_iteration") == 2 and active.get("compaction_count") == 1:
        raise SystemExit(0)
raise SystemExit(1)
PY
  then
    break
  fi
  sleep 0.25
done

if [[ -s "$TOOL_ACTIVE_STATUS_RESPONSE" ]]; then
  cp "$TOOL_ACTIVE_STATUS_RESPONSE" "$TOOL_STATUS_RESPONSE"
fi

python3 - "$TOOL_STATUS_RESPONSE" <<'PY'
import json
import re
import sys
from pathlib import Path

response = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
assert response.get("success") is True, response
text = response.get("response", "")
active = response.get("active_status")
assert isinstance(active, dict), response
assert active.get("session_key") == "agent-status-runtime-tool-growth", active
assert active.get("current_loop_iteration") == 2, active
estimated = active.get("estimated_context_tokens", 0)
def fmt_metric(value):
    units = [(1_000, "K"), (1_000_000, "M"), (1_000_000_000, "B")]
    if value < units[0][0]:
        return str(value)
    idx = len(units) - 1
    for i, (unit, _) in enumerate(units):
        if value < unit:
            idx = max(i - 1, 0)
            break
    def rounded_tenths(unit):
        return (value * 10 + unit // 2) // unit
    while idx + 1 < len(units) and rounded_tenths(units[idx][0]) >= 10000:
        idx += 1
    unit, suffix = units[idx]
    scaled = rounded_tenths(unit)
    whole, decimal = divmod(scaled, 10)
    return f"{whole}{suffix}" if decimal == 0 else f"{whole}.{decimal}{suffix}"
assert active.get("compaction_count") == 1, active
assert estimated < 10000, active
assert active.get("message_count", 0) < 10, active
assert active.get("last_response_tokens", 0) == estimated, active
assert active.get("total_tokens_used") == 94, active
estimated_text = fmt_metric(estimated)
assert f"Context 用量: ~{estimated_text} / 250K" in text, text
assert "实时 token: 累计" in text and f"最近响应 {estimated_text}" in text, text
match = re.search(r"Context 用量: ~([0-9.]+[KMB]?) / 250K", text)
assert match, text
assert match.group(1) == estimated_text, (text, active)
assert "压缩次数: 1" in text, text
PY

curl -fsS --noproxy '*' "$BASE/sessions/all" >"$TOOL_SESSIONS_RESPONSE"
curl -fsS --noproxy '*' "$BASE/sessions/agent-status-runtime-tool-growth" >"$TOOL_DETAIL_RESPONSE"

python3 - "$TOOL_STATUS_RESPONSE" "$TOOL_SESSIONS_RESPONSE" "$TOOL_DETAIL_RESPONSE" <<'PY'
import json
import sys
from pathlib import Path

status_response = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
sessions_response = json.loads(Path(sys.argv[2]).read_text(encoding="utf-8"))
detail = json.loads(Path(sys.argv[3]).read_text(encoding="utf-8"))
active = status_response.get("active_status")
assert isinstance(active, dict), status_response

sessions = sessions_response.get("sessions") or []
summary = next(
    (
        item
        for item in sessions
        if item.get("session_key") == "agent-status-runtime-tool-growth"
    ),
    None,
)
assert isinstance(summary, dict), sessions_response
assert summary.get("running") is True, summary
assert summary.get("tokens") == active.get("total_tokens_used"), (summary, active)
assert summary.get("estimated_tokens") == active.get("estimated_context_tokens"), (summary, active)
assert summary.get("context_window_tokens") == active.get("context_window_tokens"), (summary, active)
assert summary.get("context_usage_percent") == active.get("context_usage_percent"), (summary, active)
assert summary.get("last_response_tokens") == active.get("last_response_tokens"), (summary, active)

assert detail.get("run_state") == "running", detail
assert detail.get("active_status", {}).get("session_key") == active.get("session_key"), detail
assert detail.get("total_tokens_used") == active.get("total_tokens_used"), (detail, active)
assert detail.get("estimated_tokens") == active.get("estimated_context_tokens"), (detail, active)
assert detail.get("context_window_tokens") == active.get("context_window_tokens"), (detail, active)
assert detail.get("context_usage_percent") == active.get("context_usage_percent"), (detail, active)
assert detail.get("last_response_tokens") == active.get("last_response_tokens"), (detail, active)
PY

wait "$CHAT_PID"

python3 - "$TOOL_CHAT_RESPONSE" <<'PY'
import json
import sys
from pathlib import Path

response = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
assert response.get("success") is True, response
assert "large tool turn finished after compacted context" in response.get("response", ""), response
tool_calls = response.get("tool_calls")
assert isinstance(tool_calls, list), response
assert any(call.get("tool_name") == "exec_command" for call in tool_calls), tool_calls
PY

curl -fsS --noproxy '*' -X POST "$BASE/chat" \
  -H 'Content-Type: application/json' \
  -d '{"session_key":"agent-status-runtime-tool-growth","message":"/status"}' \
  >"$TOOL_STATUS_RESPONSE"

python3 - "$TOOL_STATUS_RESPONSE" <<'PY'
import json
import sys
from pathlib import Path

response = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
assert response.get("success") is True, response
text = response.get("response", "")
assert "显式压缩次数: 1" in text, text
assert "上下文管理: 按 token/context budget 与 compaction 管理" in text, text
assert "省略" not in text, text
assert "\n- 压缩次数:" not in text, text
PY

echo "[agent-builtin-status-runtime] PASS"
