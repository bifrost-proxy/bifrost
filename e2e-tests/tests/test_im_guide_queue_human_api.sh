#!/usr/bin/env bash
set -euo pipefail
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

cd "$REPO_DIR"

BIFROST_PORT="${BIFROST_PORT:-${ADMIN_PORT:-18897}}"
MOCK_PORT="${MOCK_PORT:-${MOCK_HTTP_PORT:-18898}}"
TEST_DIR="$(mktemp -d)"
MOCK_LOG="$TEST_DIR/mock-requests.jsonl"
BIFROST_LOG="$TEST_DIR/bifrost.log"
BIFROST_BIN="${BIFROST_BIN:-}"

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
  echo "[im-guide-queue-human-api] $label did not become ready" >&2
  return 1
}

python3 - "$MOCK_PORT" "$MOCK_LOG" <<'PY' &
import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port = int(sys.argv[1])
log_path = sys.argv[2]
# The release gates make the active-turn window deterministic: the test
# verifies /status while the mock model request is intentionally blocked.
default_guide_gate = threading.Event()
multi_guide_gate = threading.Event()

class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        return

    def do_GET(self):
        if self.path == "/health":
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b"ok")
            return
        if self.path == "/release/default-guide":
            default_guide_gate.set()
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b"released")
            return
        if self.path == "/release/multi-guide":
            multi_guide_gate.set()
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b"released")
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

        texts = []
        for message in payload.get("messages", []):
            content = message.get("content")
            if isinstance(content, str):
                texts.append(content)
        user_texts = [t for t in texts if t.strip()]

        if any("第一条引导" in t for t in user_texts) and any("第二条引导" in t for t in user_texts):
            content = "GUIDES_MERGED: 第一条引导 -> 第二条引导"
        elif any("MULTI_GUIDE_INITIAL" in t for t in user_texts):
            multi_guide_gate.wait(timeout=30)
            content = "INITIAL_DONE"
        elif any("DEFAULT_GUIDE_INITIAL" in t for t in user_texts):
            default_guide_gate.wait(timeout=30)
            content = "DEFAULT_GUIDE_INITIAL_DONE"
        elif any("默认引导消息" in t for t in user_texts):
            content = "DEFAULT_GUIDE_CONSUMED"
        elif any("初始消息" in t for t in user_texts):
            content = "ORDER: 初始消息 -> guide 插入 -> queue-1 -> queue-2"
        elif any("第一条" in t for t in user_texts):
            content = "ORDER: 第一条 -> 第二条 -> 第三条"
        elif any("先处理 initial" in t for t in user_texts):
            content = "GUIDE_DRAINED: 先处理 initial -> 这是 turn 结束前插入的 guide"
        elif any("hello" in t for t in user_texts):
            content = "ORDER: hello -> real queued"
        else:
            content = "OK"

        response = {
            "choices": [{
                "message": {"role": "assistant", "content": content},
                "finish_reason": "stop",
            }],
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
  echo "[im-guide-queue-human-api] skipping build, using $BIFROST_BIN"
else
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
  echo "[im-guide-queue-human-api] building bifrost"
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

echo "[im-guide-queue-human-api] starting bifrost on $BIFROST_PORT with temp data dir $TEST_DIR"
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

echo "[im-guide-queue-human-api] configuring agent mock provider"
curl -fsS --noproxy '*' -X PATCH "$BASE" \
  -H 'Content-Type: application/json' \
  -d "{
    \"enabled\": true,
    \"model_provider\": \"mock-guide-queue\",
    \"model\": \"mock-model\",
    \"base_url\": \"http://127.0.0.1:$MOCK_PORT/chat/completions\",
    \"api_key\": \"test-key\",
    \"request_timeout_secs\": 20,
    \"max_turn_iterations\": 8,
    \"history\": {\"persistence\": \"save-all\"},
    \"memories\": {
      \"use_memories\": false,
      \"generate_memories\": false
    }
  }" >/dev/null

GUIDE_PROVIDER_ID="mock-guide-default-provider"
GUIDE_OWNER_ID="owner-guide-default"
GUIDE_SESSION_KEY="$GUIDE_PROVIDER_ID:$GUIDE_OWNER_ID"
echo "[im-guide-queue-human-api] verifying IM inbound default guide mode"
curl -fsS --noproxy '*' -X POST "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/providers" \
  -H 'Content-Type: application/json' \
  -d "{
    \"id\": \"$GUIDE_PROVIDER_ID\",
    \"provider_type\": \"feishu\",
    \"display_name\": \"Mock Guide Default Provider\",
    \"enabled\": true,
    \"app_id\": \"cli_mock_guide_default\",
    \"owner_open_id\": \"$GUIDE_OWNER_ID\",
    \"event_connection_enabled\": false
  }" >/dev/null

curl -fsS --noproxy '*' -X POST "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/debug/mock-inbound" \
  -H 'Content-Type: application/json' \
  -d "{
    \"providerId\": \"$GUIDE_PROVIDER_ID\",
    \"userId\": \"$GUIDE_OWNER_ID\",
    \"chatId\": \"chat-guide-default\",
    \"text\": \"DEFAULT_GUIDE_INITIAL\"
  }" >/dev/null

# Barrier (not a fixed sleep): wait until the first inbound turn has actually
# reached the mock model. That request is what marks the session active and
# publishes its active-turn status; only then will the second inbound message
# be routed through guide mode instead of starting a fresh turn. A fixed sleep
# is racy on loaded CI runners and was the root cause of this test's flakiness.
DEFAULT_GUIDE_INITIAL_STARTED=""
for _ in $(seq 1 200); do
  if [[ -f "$MOCK_LOG" ]] && grep -q "DEFAULT_GUIDE_INITIAL" "$MOCK_LOG"; then
    DEFAULT_GUIDE_INITIAL_STARTED="true"
    break
  fi
  sleep 0.05
done
if [[ -z "$DEFAULT_GUIDE_INITIAL_STARTED" ]]; then
  echo "[im-guide-queue-human-api] default guide initial turn did not reach mock model" >&2
  exit 1
fi

curl -fsS --noproxy '*' -X POST "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/debug/mock-inbound" \
  -H 'Content-Type: application/json' \
  -d "{
    \"providerId\": \"$GUIDE_PROVIDER_ID\",
    \"userId\": \"$GUIDE_OWNER_ID\",
    \"chatId\": \"chat-guide-default\",
    \"text\": \"默认引导消息\"
  }" >/dev/null

DEFAULT_GUIDE_STATUS=""
DEFAULT_GUIDE_LAST_STATUS=""
for _ in $(seq 1 100); do
  candidate="$(curl -fsS --noproxy '*' -X POST "$BASE/chat" \
    -H 'Content-Type: application/json' \
    -d "{\"session_key\":\"$GUIDE_SESSION_KEY\",\"message\":\"/status\"}")"
  DEFAULT_GUIDE_LAST_STATUS="$candidate"
  if python3 - "$candidate" <<'PY'
import json
import sys

payload = json.loads(sys.argv[1])
guides = payload.get("pending_guide_messages") or []
text = payload.get("response", "")
ok = (
    payload.get("success") is True
    and guides == ["默认引导消息"]
    and "引导消息: 1 条尚未进入 loop" in text
)
raise SystemExit(0 if ok else 1)
PY
  then
    DEFAULT_GUIDE_STATUS="$candidate"
    break
  fi
  sleep 0.2
done

if [[ -z "$DEFAULT_GUIDE_STATUS" ]]; then
  echo "[im-guide-queue-human-api] default IM inbound message did not become pending guide" >&2
  echo "[im-guide-queue-human-api] last default status response: $DEFAULT_GUIDE_LAST_STATUS" >&2
  exit 1
fi

curl -fsS --noproxy '*' "http://127.0.0.1:$MOCK_PORT/release/default-guide" >/dev/null

DEFAULT_GUIDE_DRAINED=""
for _ in $(seq 1 80); do
  if [[ -f "$MOCK_LOG" ]] && grep -q "默认引导消息" "$MOCK_LOG"; then
    DEFAULT_GUIDE_DRAINED="true"
    break
  fi
  sleep 0.1
done

if [[ -z "$DEFAULT_GUIDE_DRAINED" ]]; then
  echo "[im-guide-queue-human-api] default IM inbound guide was not consumed by the active loop" >&2
  exit 1
fi

GUIDE_RESPONSE="$(curl -fsS --noproxy '*' -X POST "$BASE/chat" \
  -H 'Content-Type: application/json' \
  -d '{"session_key":"guide-end-test","message":"先处理 initial","guide_message":"这是 turn 结束前插入的 guide"}')"
FIFO_RESPONSE="$(curl -fsS --noproxy '*' -X POST "$BASE/chat" \
  -H 'Content-Type: application/json' \
  -d '{"session_key":"queue-test","message":"第一条","queue_messages":["第二条","第三条"]}')"
PRIORITY_RESPONSE="$(curl -fsS --noproxy '*' -X POST "$BASE/chat" \
  -H 'Content-Type: application/json' \
  -d '{"session_key":"guide-priority-test","message":"初始消息","guide_message":"guide 插入","queue_messages":["queue-1","queue-2"]}')"
BLANK_RESPONSE="$(curl -fsS --noproxy '*' -X POST "$BASE/chat" \
  -H 'Content-Type: application/json' \
  -d '{"session_key":"blank-ignore-test","message":"hello","guide_message":"   ","queue_messages":["","   ","real queued"]}')"

MULTI_RESPONSE_FILE="$TEST_DIR/multi-guide-response.json"
curl -fsS --noproxy '*' -X POST "$BASE/chat" \
  -H 'Content-Type: application/json' \
  -d '{"session_key":"multi-guide-status-test","message":"MULTI_GUIDE_INITIAL","guide_messages":["第一条引导","第二条引导"]}' \
  >"$MULTI_RESPONSE_FILE" &
MULTI_PID=$!

MULTI_MODEL_STARTED=""
for _ in $(seq 1 100); do
  if [[ -f "$MOCK_LOG" ]] && grep -q "MULTI_GUIDE_INITIAL" "$MOCK_LOG"; then
    MULTI_MODEL_STARTED="true"
    break
  fi
  sleep 0.05
done

if [[ -z "$MULTI_MODEL_STARTED" ]]; then
  echo "[im-guide-queue-human-api] multi-guide request did not reach mock model" >&2
  kill "$MULTI_PID" >/dev/null 2>&1 || true
  wait "$MULTI_PID" >/dev/null 2>&1 || true
  exit 1
fi

STATUS_RESPONSE=""
LAST_STATUS_RESPONSE=""
for _ in $(seq 1 100); do
  candidate="$(curl -fsS --noproxy '*' -X POST "$BASE/chat" \
    -H 'Content-Type: application/json' \
    -d '{"session_key":"multi-guide-status-test","message":"/status"}')"
  LAST_STATUS_RESPONSE="$candidate"
  if python3 - "$candidate" <<'PY'
import json
import sys

payload = json.loads(sys.argv[1])
guides = payload.get("pending_guide_messages") or []
active_guides = (payload.get("active_status") or {}).get("pending_guide_messages") or []
text = payload.get("response", "")
ok = (
    payload.get("success") is True
    and guides == ["第一条引导", "第二条引导"]
    and active_guides == guides
    and "引导消息: 2 条尚未进入 loop" in text
    and "第一条引导" in text
    and "第二条引导" in text
)
raise SystemExit(0 if ok else 1)
PY
  then
    STATUS_RESPONSE="$candidate"
    break
  fi
  sleep 0.2
done

if [[ -z "$STATUS_RESPONSE" ]]; then
  echo "[im-guide-queue-human-api] /status did not expose pending guide messages" >&2
  echo "[im-guide-queue-human-api] last multi-guide status response: $LAST_STATUS_RESPONSE" >&2
  kill "$MULTI_PID" >/dev/null 2>&1 || true
  wait "$MULTI_PID" >/dev/null 2>&1 || true
  exit 1
fi

curl -fsS --noproxy '*' "http://127.0.0.1:$MOCK_PORT/release/multi-guide" >/dev/null
wait "$MULTI_PID"
MULTI_RESPONSE="$(cat "$MULTI_RESPONSE_FILE")"

python3 - "$GUIDE_RESPONSE" "$FIFO_RESPONSE" "$PRIORITY_RESPONSE" "$BLANK_RESPONSE" "$MULTI_RESPONSE" "$MOCK_LOG" <<'PY'
import json
import sys
from pathlib import Path

guide = json.loads(sys.argv[1])
fifo = json.loads(sys.argv[2])
priority = json.loads(sys.argv[3])
blank = json.loads(sys.argv[4])
multi = json.loads(sys.argv[5])
mock_log = Path(sys.argv[6]).read_text(encoding="utf-8")

assert guide.get("success") is True, guide
assert fifo.get("success") is True, fifo
assert priority.get("success") is True, priority
assert blank.get("success") is True, blank
assert multi.get("success") is True, multi

assert "GUIDE_DRAINED" in guide.get("response", ""), guide
assert "ORDER: 第一条 -> 第二条 -> 第三条" in fifo.get("response", ""), fifo
assert "ORDER: 初始消息 -> guide 插入 -> queue-1 -> queue-2" in priority.get("response", ""), priority
assert "ORDER: hello -> real queued" in blank.get("response", ""), blank
assert "GUIDES_MERGED: 第一条引导 -> 第二条引导" in multi.get("response", ""), multi

assert "这是 turn 结束前插入的 guide" in mock_log, mock_log
assert "第二条" in mock_log and "第三条" in mock_log, mock_log
assert "guide 插入" in mock_log and "queue-1" in mock_log and "queue-2" in mock_log, mock_log
assert "real queued" in mock_log, mock_log
assert "引导消息 1:\\n第一条引导" in mock_log and "引导消息 2:\\n第二条引导" in mock_log, mock_log
assert "默认引导消息" in mock_log, mock_log
PY

echo "[im-guide-queue-human-api] PASS"
