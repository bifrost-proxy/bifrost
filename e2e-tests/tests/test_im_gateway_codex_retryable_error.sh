#!/usr/bin/env bash
set -euo pipefail

: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_DIR"

TEST_DIR="$(mktemp -d)"
BIFROST_LOG="$TEST_DIR/bifrost.log"
MOCK_CODEX="$TEST_DIR/mock-codex"
BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
BIFROST_PORT="${BIFROST_PORT:-$(python3 - <<'PY'
import socket

with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
)}"

cleanup() {
  if [[ -n "${BIFROST_PID:-}" ]]; then
    kill "$BIFROST_PID" >/dev/null 2>&1 || true
    wait "$BIFROST_PID" >/dev/null 2>&1 || true
  fi
  rm -rf "$TEST_DIR"
}
trap cleanup EXIT

cat >"$MOCK_CODEX" <<'PY'
#!/usr/bin/env python3
import json
import sys

if "--version" in sys.argv:
    print("codex-cli 0.144.1")
    raise SystemExit(0)

def send(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)

for line in sys.stdin:
    frame = json.loads(line)
    method = frame.get("method")
    request_id = frame.get("id")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": request_id, "result": {"userAgent": "mock-codex"}})
    elif method == "thread/start":
        send({"jsonrpc": "2.0", "method": "thread/started", "params": {"thread": {"id": "thread-retry"}}})
        send({"jsonrpc": "2.0", "id": request_id, "result": {"thread": {"id": "thread-retry"}}})
    elif method == "turn/start":
        prompt = frame["params"]["input"][0]["text"]
        send({"jsonrpc": "2.0", "id": request_id, "result": {"turn": {"id": "turn-retry"}}})
        if "TERMINAL_FAILURE" in prompt:
            send({
                "jsonrpc": "2.0",
                "method": "error",
                "params": {
                    "error": {"message": "permanent request failure"},
                    "willRetry": False,
                    "threadId": "thread-retry",
                    "turnId": "turn-retry",
                },
            })
        else:
            send({
                "jsonrpc": "2.0",
                "method": "error",
                "params": {
                    "error": {"message": "Reconnecting... 2/5"},
                    "willRetry": True,
                    "threadId": "thread-retry",
                    "turnId": "turn-retry",
                },
            })
            send({
                "jsonrpc": "2.0",
                "method": "item/completed",
                "params": {
                    "threadId": "thread-retry",
                    "turnId": "turn-retry",
                    "item": {"id": "message-1", "type": "agentMessage", "text": "BIFROST_RETRY_RECOVERED"},
                },
            })
            send({
                "jsonrpc": "2.0",
                "method": "turn/completed",
                "params": {
                    "threadId": "thread-retry",
                    "turn": {"id": "turn-retry", "status": "completed"},
                },
            })
PY
chmod +x "$MOCK_CODEX"

if [[ "${SKIP_BUILD:-false}" != "true" ]]; then
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

BIFROST_DATA_DIR="$TEST_DIR/data" "$BIFROST_BIN" start \
  --host 127.0.0.1 \
  -p "$BIFROST_PORT" \
  --unsafe-ssl \
  --skip-cert-check \
  --no-system-proxy \
  >"$BIFROST_LOG" 2>&1 &
BIFROST_PID=$!

READY=false
for _ in $(seq 1 160); do
  if ! kill -0 "$BIFROST_PID" >/dev/null 2>&1; then
    tail -120 "$BIFROST_LOG" >&2 || true
    exit 1
  fi
  if curl -fsS --noproxy '*' "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address" >/dev/null 2>&1; then
    READY=true
    break
  fi
  sleep 0.25
done
if [[ "$READY" != "true" ]]; then
  tail -120 "$BIFROST_LOG" >&2 || true
  exit 1
fi

python3 - "$BIFROST_PORT" "$MOCK_CODEX" "$REPO_DIR" <<'PY'
import json
import sys
import urllib.request

port, executable, repo_dir = sys.argv[1:4]
endpoint = f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/chat"

def run(message, session_key):
    payload = {
        "message": message,
        "sessionKey": session_key,
        "runtime": "external_cli",
        "adapter": "codex",
        "workDir": repo_dir,
        "allowWorkDirs": [repo_dir],
        "injectBifrostTools": False,
        "adapterConfig": {
            "executable": executable,
            "transport": "app_server",
            "sandbox": "read-only",
            "approvalPolicy": "never",
            "timeoutSecs": 30,
        },
    }
    request = urllib.request.Request(
        endpoint,
        data=json.dumps(payload).encode("utf-8"),
        headers={"content-type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=60) as response:
        return json.loads(response.read().decode("utf-8"))

def detail(run_id):
    with urllib.request.urlopen(
        f"{endpoint}/runs/{run_id}", timeout=30
    ) as response:
        return json.loads(response.read().decode("utf-8"))

recovered = run("RETRY_THEN_SUCCEED", "codex-retryable-error-success")
assert recovered["status"] == "succeeded", recovered
assert recovered["response"] == "BIFROST_RETRY_RECOVERED", recovered
assert not recovered["response"].startswith('{"id":1'), recovered
retry_events = [event for event in recovered["events"] if event.get("content") == "Reconnecting... 2/5"]
assert len(retry_events) == 1, recovered
assert retry_events[0]["eventType"] == "status", retry_events[0]
assert not any(event.get("eventType") == "run_failed" for event in recovered["events"]), recovered
recovered_detail = detail(recovered["runId"])
assert '"userAgent":"mock-codex"' in recovered_detail["stdout"], recovered_detail
assert recovered_detail["response"] == "BIFROST_RETRY_RECOVERED", recovered_detail

failed = run("TERMINAL_FAILURE", "codex-retryable-error-failure")
assert failed["status"] == "failed", failed
assert failed["response"] == "permanent request failure", failed
assert not failed["response"].startswith('{"id":1'), failed
assert any(event.get("eventType") == "run_failed" for event in failed["events"]), failed
failed_detail = detail(failed["runId"])
assert '"userAgent":"mock-codex"' in failed_detail["stdout"], failed_detail
assert failed_detail["response"] == "permanent request failure", failed_detail
PY

echo "[im-gateway-codex-retryable-error] PASS"
