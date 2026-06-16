#!/usr/bin/env bash
set -euo pipefail
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

PORT="${BIFROST_AGENT_WORKER_E2E_PORT:-${ADMIN_PORT:-18881}}"
MOCK_PORT="${BIFROST_AGENT_WORKER_E2E_MOCK_PORT:-${MOCK_HTTP_PORT:-18882}}"
TEST_DIR="$(mktemp -d)"
LOG_FILE="$TEST_DIR/bifrost.log"
SSE_FILE="$TEST_DIR/agent.sse"
DISCONNECT_SSE_FILE="$TEST_DIR/agent-disconnect.sse"
EXTERNAL_SSE_FILE="$TEST_DIR/external-runner.sse"
MOCK_LOG_FILE="$TEST_DIR/mock-model.log"
BIFROST_PID=""
MOCK_PID=""
CURL_PID=""
DISCONNECT_CURL_PID=""
EXTERNAL_CURL_PID=""

worker_pids() {
  local pattern="$1"
  if [[ -z "$BIFROST_PID" ]]; then
    return 0
  fi
  pgrep -P "$BIFROST_PID" -f "$pattern" 2>/dev/null || true
}

worker_running() {
  local pattern="$1"
  [[ -n "$(worker_pids "$pattern")" ]]
}

kill_workers() {
  local pattern="$1"
  local pids
  pids="$(worker_pids "$pattern")"
  if [[ -n "$pids" ]]; then
    kill $pids >/dev/null 2>&1 || true
  fi
}

cleanup() {
  set +e
  if [[ -n "$CURL_PID" ]]; then kill "$CURL_PID" >/dev/null 2>&1 || true; fi
  if [[ -n "$DISCONNECT_CURL_PID" ]]; then kill "$DISCONNECT_CURL_PID" >/dev/null 2>&1 || true; fi
  if [[ -n "$EXTERNAL_CURL_PID" ]]; then kill "$EXTERNAL_CURL_PID" >/dev/null 2>&1 || true; fi
  kill_workers "bifrost agent worker"
  kill_workers "bifrost agent external-runner-worker"
  if [[ -n "$BIFROST_PID" ]]; then kill "$BIFROST_PID" >/dev/null 2>&1 || true; fi
  if [[ -n "$MOCK_PID" ]]; then kill "$MOCK_PID" >/dev/null 2>&1 || true; fi
  BIFROST_DATA_DIR="$TEST_DIR" "$ROOT_DIR/target/debug/bifrost" -p "$PORT" stop >/dev/null 2>&1 || true
  rm -rf "$TEST_DIR"
}
trap cleanup EXIT

fail() {
  echo "[agent-worker-process-isolation-e2e] FAIL: $*" >&2
  echo "--- bifrost log ---" >&2
  cat "$LOG_FILE" >&2 2>/dev/null || true
  echo "--- sse ---" >&2
  cat "$SSE_FILE" >&2 2>/dev/null || true
  echo "--- disconnect sse ---" >&2
  cat "$DISCONNECT_SSE_FILE" >&2 2>/dev/null || true
  echo "--- external sse ---" >&2
  cat "$EXTERNAL_SSE_FILE" >&2 2>/dev/null || true
  exit 1
}

if [[ "${SKIP_BUILD:-}" != "true" ]]; then
  cargo build --bin bifrost
fi
BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/debug/bifrost}"

mkdir -p "$TEST_DIR/agent"
cat > "$TEST_DIR/agent/config.toml" <<'TOML'
enabled = true
model = "mock-model"
model_provider = "openai"

[model_providers.openai]
base_url = "http://127.0.0.1:__MOCK_PORT__/v1"
api_key = "test"
TOML
perl -0pi -e "s/__MOCK_PORT__/$MOCK_PORT/g" "$TEST_DIR/agent/config.toml"

mkdir -p "$TEST_DIR/admin"
cat > "$TEST_DIR/admin/im_gateway_external_cli_agent.json" <<'JSON'
{
  "version": 1,
  "defaultRunnerId": "slow-mock",
  "runners": {
    "slow-mock": {
      "enabled": true,
      "adapter": "mock",
      "injectBifrostTools": false,
      "adapterConfig": {
        "executable": "sh",
        "args": [
          "-c",
          "cat >/dev/null; sleep 30; printf '%s\n' '{\"type\":\"assistant_final\",\"content\":\"external ok\"}'"
        ],
        "timeoutSecs": 60
      }
    }
  },
  "channels": {}
}
JSON

cat > "$TEST_DIR/mock_model.py" <<'PY'
import json
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"ok")

    def do_POST(self):
        length = int(self.headers.get("content-length", "0") or "0")
        if length:
            self.rfile.read(length)
        time.sleep(30)
        payload = {
            "id": "chatcmpl-agent-worker-e2e",
            "object": "chat.completion",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
        }
        data = json.dumps(payload).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, fmt, *args):
        return

ThreadingHTTPServer(("127.0.0.1", int(__import__("sys").argv[1])), Handler).serve_forever()
PY
python3 "$TEST_DIR/mock_model.py" "$MOCK_PORT" >"$MOCK_LOG_FILE" 2>&1 &
MOCK_PID=$!
for _ in {1..40}; do
  if curl -sS --noproxy '*' "http://127.0.0.1:$MOCK_PORT/health" >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done

BIFROST_DATA_DIR="$TEST_DIR" "$BIFROST_BIN" start -p "$PORT" --unsafe-ssl --skip-cert-check --no-system-proxy >"$LOG_FILE" 2>&1 &
BIFROST_PID=$!

for _ in {1..80}; do
  if curl -sS "http://127.0.0.1:${PORT}/_bifrost/api/proxy/address" >/dev/null 2>&1; then
    break
  fi
  sleep 0.25
done
curl -sS "http://127.0.0.1:${PORT}/_bifrost/api/proxy/address" >/dev/null || fail "admin api not ready"

curl -sS -N -X POST "http://127.0.0.1:${PORT}/_bifrost/api/agent/chat/stream" \
  -H 'Content-Type: application/json' \
  -d '{"message":"hello","session_key":"e2e-agent-worker"}' >"$SSE_FILE" &
CURL_PID=$!

worker_seen=false
for _ in {1..40}; do
  if worker_running "bifrost agent worker"; then
    worker_seen=true
    break
  fi
  sleep 0.25
done
[[ "$worker_seen" == "true" ]] || fail "worker process was not spawned"

for _ in {1..40}; do
  if grep -q "run_started" "$SSE_FILE"; then
    break
  fi
  sleep 0.25
done
grep -q "run_started" "$SSE_FILE" || fail "stream did not emit run_started"
curl -sS "http://127.0.0.1:${PORT}/_bifrost/api/proxy/address" >/dev/null || fail "admin api stopped responding while worker active"

STOP_RESPONSE="$(curl -sS -X POST "http://127.0.0.1:${PORT}/_bifrost/api/agent/chat/stream" \
  -H 'Content-Type: application/json' \
  -d '{"message":"/stop","session_key":"e2e-agent-worker"}')"
echo "$STOP_RESPONSE" | grep -Eq 'stopped|已请求停止当前 Agent loop|Agent worker 子进程已停止' || fail "stop response did not acknowledge stop: $STOP_RESPONSE"

for _ in {1..40}; do
  if ! worker_running "bifrost agent worker"; then
    break
  fi
  sleep 0.25
done
if worker_running "bifrost agent worker"; then
  fail "worker process still running after stop"
fi
curl -sS "http://127.0.0.1:${PORT}/_bifrost/api/proxy/address" >/dev/null || fail "admin api not responding after stop"

curl -sS -N -X POST "http://127.0.0.1:${PORT}/_bifrost/api/agent/chat/stream" \
  -H 'Content-Type: application/json' \
  -d '{"message":"hello again","session_key":"e2e-agent-worker-disconnect"}' >"$DISCONNECT_SSE_FILE" &
DISCONNECT_CURL_PID=$!

for _ in {1..40}; do
  if worker_running "bifrost agent worker"; then
    break
  fi
  sleep 0.25
done
for _ in {1..40}; do
  if grep -q "run_started" "$DISCONNECT_SSE_FILE"; then
    break
  fi
  sleep 0.25
done
grep -q "run_started" "$DISCONNECT_SSE_FILE" || fail "disconnect stream did not emit run_started"
kill "$DISCONNECT_CURL_PID" >/dev/null 2>&1 || true
DISCONNECT_CURL_PID=""
for _ in {1..40}; do
  if ! worker_running "bifrost agent worker"; then
    break
  fi
  sleep 0.25
done
if worker_running "bifrost agent worker"; then
  fail "worker process still running after SSE disconnect"
fi
curl -sS "http://127.0.0.1:${PORT}/_bifrost/api/proxy/address" >/dev/null || fail "admin api not responding after disconnect cleanup"

curl -sS -N -X POST "http://127.0.0.1:${PORT}/_bifrost/api/im-gateway/chat/stream" \
  -H 'Content-Type: application/json' \
  -d '{"message":"hello external","sessionKey":"e2e-external-worker","runnerId":"slow-mock"}' >"$EXTERNAL_SSE_FILE" &
EXTERNAL_CURL_PID=$!

external_worker_seen=false
for _ in {1..40}; do
  if worker_running "bifrost agent external-runner-worker"; then
    external_worker_seen=true
    break
  fi
  sleep 0.25
done
[[ "$external_worker_seen" == "true" ]] || fail "external runner worker process was not spawned"
curl -sS "http://127.0.0.1:${PORT}/_bifrost/api/proxy/address" >/dev/null || fail "admin api stopped responding while external runner active"

EXTERNAL_STOP_RESPONSE="$(curl -sS -N -X POST "http://127.0.0.1:${PORT}/_bifrost/api/im-gateway/chat/stream" \
  -H 'Content-Type: application/json' \
  -d '{"message":"/stop","sessionKey":"e2e-external-worker","runnerId":"slow-mock"}')"
echo "$EXTERNAL_STOP_RESPONSE" | grep -Eq 'stopped|已请求停止当前 Runner' || fail "external stop response did not acknowledge stop: $EXTERNAL_STOP_RESPONSE"

for _ in {1..40}; do
  if ! worker_running "bifrost agent external-runner-worker"; then
    break
  fi
  sleep 0.25
done
if worker_running "bifrost agent external-runner-worker"; then
  fail "external runner worker process still running after stop"
fi
curl -sS "http://127.0.0.1:${PORT}/_bifrost/api/proxy/address" >/dev/null || fail "admin api not responding after external runner stop"

echo "[agent-worker-process-isolation-e2e] PASS"
