#!/usr/bin/env bash
set -euo pipefail
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

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
FIRST_WORK_DIR="$TEST_DIR/work-a"
SECOND_WORK_DIR="$TEST_DIR/work-b"
BIFROST_LOG="$TEST_DIR/bifrost.log"
MOCK_LOG="$TEST_DIR/mock.log"
FIRST_RESPONSE="$TEST_DIR/first-response.json"
SWITCH_RESPONSE="$TEST_DIR/switch-response.json"
STATUS_RESPONSE="$TEST_DIR/status-response.json"
BIFROST_BIN="${BIFROST_BIN:-}"

cleanup() {
  local exit_code="$?"
  if [[ "$exit_code" -ne 0 ]]; then
    echo "[agent-direct-path-switch] DEBUG temp dir: $TEST_DIR" >&2
    [[ -f "$BIFROST_LOG" ]] && tail -n 120 "$BIFROST_LOG" >&2 || true
    [[ -f "$MOCK_LOG" ]] && cat "$MOCK_LOG" >&2 || true
    [[ -f "$FIRST_RESPONSE" ]] && cat "$FIRST_RESPONSE" >&2 || true
    [[ -f "$SWITCH_RESPONSE" ]] && cat "$SWITCH_RESPONSE" >&2 || true
    [[ -f "$STATUS_RESPONSE" ]] && cat "$STATUS_RESPONSE" >&2 || true
  fi
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
  echo "[agent-direct-path-switch] $label did not become ready" >&2
  return 1
}

mkdir -p "$FIRST_WORK_DIR" "$SECOND_WORK_DIR"

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
        else:
            self.send_error(404)

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        payload = json.loads(self.rfile.read(length) or b"{}")
        with open(log_path, "a", encoding="utf-8") as fh:
            fh.write(json.dumps(payload, ensure_ascii=False) + "\n")
        body = json.dumps({
            "choices": [{"message": {"role": "assistant", "content": "MODEL_OK"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 4, "completion_tokens": 2, "total_tokens": 6},
        }, ensure_ascii=False).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
MOCK_PID=$!
wait_http "http://127.0.0.1:$MOCK_PORT/health" "mock model"

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/release/bifrost}"
  echo "[agent-direct-path-switch] skipping build, using $BIFROST_BIN"
else
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
fi

if [[ ! -x "$BIFROST_BIN" ]]; then
  echo "[agent-direct-path-switch] bifrost binary not found or not executable: $BIFROST_BIN" >&2
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
    \"model_provider\": \"mock-direct-path-switch\",
    \"model\": \"mock-model\",
    \"base_url\": \"http://127.0.0.1:$MOCK_PORT/chat/completions\",
    \"api_key\": \"test-key\",
    \"memories\": {\"use_memories\": false, \"generate_memories\": false}
  }" >/dev/null

curl -fsS --noproxy '*' -X POST "$BASE/chat" \
  -H 'Content-Type: application/json' \
  -d "{\"session_key\":\"direct-path-switch\",\"work_dir\":\"$FIRST_WORK_DIR\",\"message\":\"hello\"}" \
  >"$FIRST_RESPONSE"

python3 - "$FIRST_RESPONSE" "$FIRST_WORK_DIR" <<'PY'
import json
import sys
from pathlib import Path

response = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
assert response.get("success") is True, response
assert "MODEL_OK" in response.get("response", ""), response
PY

curl -fsS --noproxy '*' -X POST "$BASE/chat" \
  -H 'Content-Type: application/json' \
  -d "{\"session_key\":\"direct-path-switch\",\"message\":\"$SECOND_WORK_DIR\"}" \
  >"$SWITCH_RESPONSE"

python3 - "$SWITCH_RESPONSE" "$SECOND_WORK_DIR" "$MOCK_LOG" <<'PY'
import json
import sys
from pathlib import Path

response = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
new_dir = sys.argv[2]
mock_log = Path(sys.argv[3]).read_text(encoding="utf-8")
assert response.get("success") is True, response
text = response.get("response", "")
assert "已切换工作目录到:" in text, text
assert new_dir in text, text
assert "未知命令" not in text, text
assert len([line for line in mock_log.splitlines() if line.strip()]) == 1, mock_log
PY

curl -fsS --noproxy '*' -X POST "$BASE/chat" \
  -H 'Content-Type: application/json' \
  -d "{\"session_key\":\"direct-path-switch\",\"message\":\"/status\"}" \
  >"$STATUS_RESPONSE"

python3 - "$STATUS_RESPONSE" "$SECOND_WORK_DIR" <<'PY'
import json
import sys
from pathlib import Path

response = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
assert response.get("success") is True, response
text = response.get("response", "")
assert f"工作路径: {sys.argv[2]}" in text, text
assert "历史对话轮次:" in text, text
PY

echo "[agent-direct-path-switch] PASS"
