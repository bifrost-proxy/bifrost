#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

cd "$REPO_DIR"

BIFROST_PORT="${BIFROST_PORT:-18883}"
MOCK_PORT="${MOCK_PORT:-18884}"
TEST_DIR="$(mktemp -d)"
MOCK_LOG="$TEST_DIR/mock-requests.jsonl"
BIFROST_LOG="$TEST_DIR/bifrost.log"

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

assert_contains() {
  local value="$1"
  local expected="$2"
  local label="$3"
  if [[ "$value" != *"$expected"* ]]; then
    echo "[long-term-memory-human-api] ASSERT FAILED: $label should contain '$expected'" >&2
    echo "$value" >&2
    exit 1
  fi
}

wait_http() {
  local url="$1"
  local label="$2"
  for _ in $(seq 1 80); do
    if curl -fsS --noproxy '*' "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.25
  done
  echo "[long-term-memory-human-api] $label did not become ready" >&2
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
        joined = "\n".join(texts)
        if "You extract durable memories from a Bifrost Agent conversation" in joined:
            content = json.dumps({"memories": ["用户自称是独孤怼怼。"]}, ensure_ascii=False)
        elif "Bifrost memory consolidation agent" in joined:
            content = json.dumps({
                "memory_summary": "- 用户自称是独孤怼怼。",
                "memory": "# Memory\n\n- 用户自称是独孤怼怼。\n  source: phase2_consolidated",
                "skills": [],
            }, ensure_ascii=False)
        elif "## Memory" in joined and "独孤怼怼" in joined and "我是谁" in joined:
            content = "你是独孤怼怼。"
        else:
            content = "已记住，你是独孤怼怼。"

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

echo "[long-term-memory-human-api] building bifrost"
cargo build --bin bifrost

echo "[long-term-memory-human-api] starting bifrost on $BIFROST_PORT with temp data dir $TEST_DIR"
BIFROST_DATA_DIR="$TEST_DIR" ./target/debug/bifrost start \
  --host 127.0.0.1 \
  -p "$BIFROST_PORT" \
  --unsafe-ssl \
  --no-system-proxy \
  >"$BIFROST_LOG" 2>&1 &
BIFROST_PID=$!
wait_http "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address" "bifrost"

BASE="http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/agent"

echo "[long-term-memory-human-api] configuring agent mock provider and memory switches"
curl -fsS --noproxy '*' -X PATCH "$BASE" \
  -H 'Content-Type: application/json' \
  -d "{
    \"enabled\": true,
    \"model_provider\": \"mock-human-memory\",
    \"model\": \"mock-model\",
    \"base_url\": \"http://127.0.0.1:$MOCK_PORT/chat/completions\",
    \"api_key\": \"test-key\",
    \"request_timeout_secs\": 20,
    \"max_turn_iterations\": 2,
    \"history\": {\"persistence\": \"save-all\"},
    \"memories\": {
      \"use_memories\": true,
      \"generate_memories\": true
    }
  }" >/dev/null

echo "[long-term-memory-human-api] first independent session writes memory"
FIRST_RESPONSE="$(curl -fsS --noproxy '*' -X POST "$BASE/chat" \
  -H 'Content-Type: application/json' \
  -d '{"session_key":"human-memory-source","message":"请记住我是“独孤怼怼”。"}')"
assert_contains "$FIRST_RESPONSE" "独孤怼怼" "first response"

SUMMARY_FILE="$TEST_DIR/agent/memory/memory_summary.md"
MEMORY_FILE="$TEST_DIR/agent/memory/MEMORY.md"
RAW_FILE="$TEST_DIR/agent/memory/raw_memories.md"
PHASE2_STATE_FILE="$TEST_DIR/agent/memory/.phase2_state.json"
ROLLOUT_DIR="$TEST_DIR/agent/memory/rollout_summaries"
test -f "$SUMMARY_FILE"
test -f "$MEMORY_FILE"
test -f "$RAW_FILE"
test -f "$PHASE2_STATE_FILE"
test -d "$ROLLOUT_DIR"
assert_contains "$(cat "$SUMMARY_FILE")" "独孤怼怼" "memory_summary.md"
assert_contains "$(cat "$MEMORY_FILE")" "source: phase2_consolidated" "MEMORY.md source"
assert_contains "$(cat "$MEMORY_FILE")" "独孤怼怼" "MEMORY.md content"
assert_contains "$(cat "$RAW_FILE")" "source: auto_extract" "raw_memories.md source"
assert_contains "$(cat "$RAW_FILE")" "独孤怼怼" "raw_memories.md content"
assert_contains "$(cat "$MOCK_LOG")" "Bifrost memory consolidation agent" "mock request log"
python3 - "$PHASE2_STATE_FILE" <<'PY'
import json
import sys
with open(sys.argv[1], encoding="utf-8") as fh:
    state = json.load(fh)
assert state.get("last_input_hash"), state
assert state.get("processed_input_count") == 1, state
assert state.get("total_input_count") == 1, state
assert state.get("has_more_inputs") is False, state
assert state.get("updated_at_unix", 0) > 0, state
PY
if [[ "$(find "$ROLLOUT_DIR" -type f -name '*.md' | wc -l | tr -d ' ')" -lt 1 ]]; then
  echo "[long-term-memory-human-api] ASSERT FAILED: rollout_summaries should contain at least one markdown file" >&2
  exit 1
fi
if [[ -e "$TEST_DIR/agent/memory/memories.sqlite" ]]; then
  echo "[long-term-memory-human-api] ASSERT FAILED: memories.sqlite should not exist" >&2
  exit 1
fi

echo "[long-term-memory-human-api] second independent session consumes memory"
SECOND_RESPONSE="$(curl -fsS --noproxy '*' -X POST "$BASE/chat" \
  -H 'Content-Type: application/json' \
  -d '{"session_key":"human-memory-consumer-1","message":"这是新的独立 session。请只根据长期记忆回答：我是谁？"}')"
assert_contains "$SECOND_RESPONSE" "独孤怼怼" "second response"

echo "[long-term-memory-human-api] third independent session consumes memory"
THIRD_RESPONSE="$(curl -fsS --noproxy '*' -X POST "$BASE/chat" \
  -H 'Content-Type: application/json' \
  -d '{"session_key":"human-memory-consumer-2","message":"再开一个新的独立 session。请只根据长期记忆回答：我是谁？"}')"
assert_contains "$THIRD_RESPONSE" "独孤怼怼" "third response"

assert_contains "$(cat "$MOCK_LOG")" "## Memory" "mock request log"
assert_contains "$(cat "$MOCK_LOG")" "MEMORY.md (searchable registry; primary file to query)" "mock request log"
assert_contains "$(cat "$MOCK_LOG")" "独孤怼怼" "mock request log"

echo "[long-term-memory-human-api] PASS"
