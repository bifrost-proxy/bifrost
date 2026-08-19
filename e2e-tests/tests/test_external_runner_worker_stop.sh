#!/usr/bin/env bash
set -euo pipefail

unset BIFROST_DETACHED_DAEMON_CHILD
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
# Keep the IM request dispatcher in the service process so this scenario
# exercises the dedicated external-runner worker rather than the IM worker's
# in-process broker fallback.
: "${BIFROST_IM_GATEWAY_EXECUTION_MODE:=legacy}"
: "${BIFROST_EXTERNAL_CLI_EXECUTION_MODE:=worker}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT BIFROST_DISABLE_TRAY
export BIFROST_IM_GATEWAY_EXECUTION_MODE BIFROST_EXTERNAL_CLI_EXECUTION_MODE

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_DIR"

TEST_DIR="$(mktemp -d "$REPO_DIR/.bifrost-e2e-worker-stop.XXXXXX")"
export BIFROST_DATA_DIR="$TEST_DIR"
source "$REPO_DIR/e2e-tests/test_utils/process.sh"
mark_e2e_data_root "$TEST_DIR"

BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
BIFROST_LOG="$TEST_DIR/bifrost.log"
MOCK_LOG="$TEST_DIR/mock-protocol.jsonl"
BIFROST_PORT="${BIFROST_PORT:-$(python3 - <<'PY'
import socket
with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
)}"

cleanup() {
  if [[ -x "${BIFROST_BIN:-}" ]]; then
    BIFROST_DATA_DIR="$TEST_DIR" "$BIFROST_BIN" stop >/dev/null 2>&1 || true
  fi
  kill_bifrost_in_data_root "$TEST_DIR" >/dev/null 2>&1 || true
  if [[ "${KEEP_TEST_DIR:-false}" == "true" ]]; then
    echo "[external-runner-worker-stop] keeping test dir: $TEST_DIR" >&2
  else
    rm -rf "$TEST_DIR"
  fi
}
trap cleanup EXIT

python3 - "$TEST_DIR/mock-runner" <<'PY'
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
path.write_text(r'''#!/usr/bin/env python3
import json
import os
import sys

runner = os.environ["MOCK_RUNNER"]
log_path = os.environ["MOCK_LOG"]
thread_id = f"thread-{runner}"
turn_id = f"turn-{runner}"

def send(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)

def record(event, **values):
    with open(log_path, "a", encoding="utf-8") as handle:
        handle.write(json.dumps({"event": event, "runner": runner, **values}, separators=(",", ":")) + "\n")

if "--version" in sys.argv:
    print(f"{runner} 0.0.0-mock")
    raise SystemExit(0)

if "--input-format" in sys.argv and sys.argv[sys.argv.index("--input-format") + 1] == "stream-json":
    first = json.loads(sys.stdin.readline())
    prompt = first["message"]["content"][0]["text"]
    record("turn_started", prompt=prompt)
    send({"type":"system","subtype":"init","session_id":thread_id})
    send(first)
    record("turn_ready")
    if "complete-now" in prompt or "queued-after-stop" in prompt:
        send({"type":"assistant","message":{"content":[{"type":"text","text":f"DONE_{runner}"}]},"session_id":thread_id})
        send({"type":"result","subtype":"success","is_error":False,"result":f"DONE_{runner}","session_id":thread_id})
        raise SystemExit(0)
    interrupt = json.loads(sys.stdin.readline())
    record("interrupt_received", frame=interrupt)
    assert interrupt["type"] == "control_request", interrupt
    assert interrupt["request"]["subtype"] == "interrupt", interrupt
    send({"type":"control_response","response":{"subtype":"success","request_id":interrupt["request_id"],"response":{}}})
    send({"type":"result","subtype":"error_during_execution","is_error":True,"result":"interrupted","session_id":thread_id})
    raise SystemExit(0)

for line in sys.stdin:
    frame = json.loads(line)
    method = frame.get("method")
    request_id = frame.get("id")
    if method == "initialize":
        send({"jsonrpc":"2.0","id":request_id,"result":{}})
    elif method in ("thread/start", "thread/resume"):
        send({"jsonrpc":"2.0","id":request_id,"result":{"thread":{"id":thread_id}}})
    elif method == "turn/start":
        prompt = frame["params"]["input"][0]["text"]
        record("turn_started", prompt=prompt)
        send({"jsonrpc":"2.0","id":request_id,"result":{"turn":{"id":turn_id}}})
        record("turn_ready")
        if "complete-now" in prompt or "queued-after-stop" in prompt:
            send({"jsonrpc":"2.0","method":"item/completed","params":{"threadId":thread_id,"turnId":turn_id,"item":{"id":"message-done","type":"agentMessage","text":f"DONE_{runner}"}}})
            send({"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":thread_id,"turn":{"id":turn_id,"status":"completed"}}})
    elif method == "account/rateLimits/read":
        send({"jsonrpc":"2.0","id":request_id,"error":{"code":-32601,"message":"unsupported"}})
    elif method == "turn/interrupt":
        record("interrupt_received", frame=frame)
        send({"jsonrpc":"2.0","id":request_id,"result":{}})
        send({"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":thread_id,"turn":{"id":turn_id,"status":"interrupted"}}})
''', encoding="utf-8")
path.chmod(0o755)
PY

if [[ "${SKIP_BUILD:-false}" != "true" ]]; then
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

START_ARGS=(--daemon --host 127.0.0.1 -p "$BIFROST_PORT" --unsafe-ssl --skip-cert-check --no-system-proxy)
if "$BIFROST_BIN" start --help | grep -q -- '--no-tray'; then
  START_ARGS+=(--no-tray)
fi
BIFROST_DATA_DIR="$TEST_DIR" "$BIFROST_BIN" start "${START_ARGS[@]}" >"$BIFROST_LOG" 2>&1

for _ in $(seq 1 180); do
  if curl -fsS --noproxy '*' "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address" >/dev/null 2>&1; then
    break
  fi
  sleep 0.25
done
curl -fsS --noproxy '*' "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address" >/dev/null

python3 - "$BIFROST_PORT" "$TEST_DIR/mock-runner" "$MOCK_LOG" "$REPO_DIR" <<'PY'
import json
import sys
import urllib.request

port, executable, mock_log, repo_dir = sys.argv[1:5]
runners = {}
for runner_id, adapter, transport in (
    ("codex-stop", "codex", "app_server"),
    ("traex-stop", "traex", "app_server"),
    ("claude-stop", "claude_code", "stream_json"),
):
    runners[runner_id] = {
        "enabled": True,
        "adapter": adapter,
        "adapterConfig": {
            "executable": executable,
            "transport": transport,
            "env": {"MOCK_RUNNER": runner_id, "MOCK_LOG": mock_log},
            "timeoutSecs": 30,
        },
        "workDir": repo_dir,
        "injectBifrostTools": False,
        "skillPaths": [],
        "deliveryMode": "final_reply",
    }
request = urllib.request.Request(
    f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/chat/config",
    data=json.dumps({"version":2,"defaultRunnerId":"codex-stop","runners":runners,"channels":{}}).encode(),
    headers={"content-type":"application/json"},
    method="PATCH",
)
with urllib.request.urlopen(request, timeout=30) as response:
    assert response.status == 200, response.read().decode()
PY

wait_for_record() {
  local runner="$1"
  local event="$2"
  for _ in $(seq 1 160); do
    if [[ -f "$MOCK_LOG" ]] && grep -q "\"event\":\"$event\",\"runner\":\"$runner\"" "$MOCK_LOG"; then
      return 0
    fi
    sleep 0.05
  done
  echo "missing mock record runner=$runner event=$event" >&2
  tail -100 "$MOCK_LOG" >&2 || true
  return 1
}

run_stop_case() {
  local runner="$1"
  local session="worker-stop-$runner"
  local run_log="$TEST_DIR/$runner-run.ndjson"
  local stop_log="$TEST_DIR/$runner-stop.ndjson"

  "$BIFROST_BIN" -H 127.0.0.1 -p "$BIFROST_PORT" agent run \
    --runner "$runner" --session "$session" --json "hold-$runner" >"$run_log" 2>&1 &
  local run_pid=$!
  wait_for_record "$runner" turn_ready
  "$BIFROST_BIN" -H 127.0.0.1 -p "$BIFROST_PORT" agent run \
    --runner "$runner" --session "$session" --json "/stop" >"$stop_log" 2>&1
  wait "$run_pid"

  wait_for_record "$runner" interrupt_received
  grep -q '"stopped":true' "$stop_log"
  grep -q '"status":"stopped"' "$run_log"
  if grep -q '"runId":"stopped-' "$run_log"; then
    echo "stop returned a synthetic run id for $runner" >&2
    cat "$run_log" >&2
    return 1
  fi
}

run_stop_case codex-stop
run_stop_case traex-stop
run_stop_case claude-stop

# Queue state remains owned by the main service while the isolated worker stops.
# The stopped turn and the queued continuation must each execute exactly once.
QUEUE_SESSION="worker-stop-queue"
QUEUE_RUN_LOG="$TEST_DIR/queue-run.ndjson"
"$BIFROST_BIN" -H 127.0.0.1 -p "$BIFROST_PORT" agent run \
  --runner codex-stop --session "$QUEUE_SESSION" --json "hold-queue" >"$QUEUE_RUN_LOG" 2>&1 &
QUEUE_PID=$!
for _ in $(seq 1 160); do
  [[ "$(grep -c '"event":"turn_ready","runner":"codex-stop"' "$MOCK_LOG" 2>/dev/null || true)" -ge 2 ]] && break
  sleep 0.05
done
"$BIFROST_BIN" -H 127.0.0.1 -p "$BIFROST_PORT" agent run \
  --runner codex-stop --session "$QUEUE_SESSION" --json "/q queued-after-stop" >"$TEST_DIR/queue-command.ndjson" 2>&1
"$BIFROST_BIN" -H 127.0.0.1 -p "$BIFROST_PORT" agent run \
  --runner codex-stop --session "$QUEUE_SESSION" --json "/stop" >"$TEST_DIR/queue-stop.ndjson" 2>&1
wait "$QUEUE_PID"
grep -q 'DONE_codex-stop' "$QUEUE_RUN_LOG"
[[ "$(grep -c 'queued-after-stop' "$MOCK_LOG")" -eq 1 ]]

# The non-stream API bypasses the UI queue. A replacement run for the same
# session must stop the previous worker before it starts the new one.
REPLACE_SESSION="worker-stop-replace"
curl -fsS --noproxy '*' -X POST \
  "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/chat" \
  -H 'content-type: application/json' \
  -d "{\"runnerId\":\"codex-stop\",\"sessionKey\":\"$REPLACE_SESSION\",\"runtime\":\"external_cli\",\"message\":\"hold-replacement\"}" \
  >"$TEST_DIR/replaced-run.json" &
REPLACED_PID=$!
for _ in $(seq 1 160); do
  grep -q 'hold-replacement' "$MOCK_LOG" 2>/dev/null && break
  sleep 0.05
done
curl -fsS --noproxy '*' -X POST \
  "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/chat" \
  -H 'content-type: application/json' \
  -d "{\"runnerId\":\"codex-stop\",\"sessionKey\":\"$REPLACE_SESSION\",\"runtime\":\"external_cli\",\"message\":\"complete-now replacement\"}" \
  >"$TEST_DIR/replacement-run.json"
wait "$REPLACED_PID"
grep -q '"status":"stopped"' "$TEST_DIR/replaced-run.json"
grep -q 'DONE_codex-stop' "$TEST_DIR/replacement-run.json"

# Chat Gateway /clear must wait for the isolated worker stop before deleting
# the service-owned queue/session state. The queued continuation must not run.
CLEAR_SESSION="worker-stop-clear"
"$BIFROST_BIN" -H 127.0.0.1 -p "$BIFROST_PORT" agent run \
  --runner codex-stop --session "$CLEAR_SESSION" --json "hold-clear" \
  >"$TEST_DIR/clear-active-run.ndjson" 2>&1 &
CLEAR_RUN_PID=$!
for _ in $(seq 1 160); do
  grep -q 'hold-clear' "$MOCK_LOG" 2>/dev/null && break
  sleep 0.05
done
"$BIFROST_BIN" -H 127.0.0.1 -p "$BIFROST_PORT" agent run \
  --runner codex-stop --session "$CLEAR_SESSION" --json "/q queued-must-be-cleared" \
  >"$TEST_DIR/clear-queued.ndjson" 2>&1
grep -q '"queued":true' "$TEST_DIR/clear-queued.ndjson"
"$BIFROST_BIN" -H 127.0.0.1 -p "$BIFROST_PORT" agent run \
  --runner codex-stop --session "$CLEAR_SESSION" --json "/clear" \
  >"$TEST_DIR/clear-command.ndjson" 2>&1
wait "$CLEAR_RUN_PID"
grep -q '"cleared":true' "$TEST_DIR/clear-command.ndjson"
grep -q '"status":"stopped"' "$TEST_DIR/clear-active-run.ndjson"
sleep 0.2
if grep -q 'queued-must-be-cleared' "$MOCK_LOG"; then
  echo "queued continuation ran after /clear" >&2
  tail -100 "$MOCK_LOG" >&2
  exit 1
fi

# Service shutdown must stop an active isolated worker through the same native
# interrupt path before the parent runtime and worker control channels vanish.
SHUTDOWN_INTERRUPTS_BEFORE="$(grep -c '"event":"interrupt_received"' "$MOCK_LOG" 2>/dev/null || true)"
curl -fsS --noproxy '*' -X POST \
  "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/chat" \
  -H 'content-type: application/json' \
  -d '{"runnerId":"codex-stop","runtime":"external_cli","message":"hold-service-shutdown"}' \
  >"$TEST_DIR/shutdown-run.json" 2>&1 &
SHUTDOWN_RUN_PID=$!
for _ in $(seq 1 160); do
  grep -q 'hold-service-shutdown' "$MOCK_LOG" 2>/dev/null && break
  sleep 0.05
done
grep -q 'hold-service-shutdown' "$MOCK_LOG"
BIFROST_DATA_DIR="$TEST_DIR" "$BIFROST_BIN" stop >"$TEST_DIR/service-stop.log" 2>&1
wait "$SHUTDOWN_RUN_PID" || true
for _ in $(seq 1 160); do
  [[ "$(grep -c '"event":"interrupt_received"' "$MOCK_LOG" 2>/dev/null || true)" -gt "$SHUTDOWN_INTERRUPTS_BEFORE" ]] && break
  sleep 0.05
done

python3 - "$MOCK_LOG" <<'PY'
import json
import sys

records = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8")]
for runner in ("codex-stop", "traex-stop", "claude-stop"):
    interrupts = [r for r in records if r["runner"] == runner and r["event"] == "interrupt_received"]
    assert interrupts, (runner, records)
    frame = interrupts[0]["frame"]
    if runner == "claude-stop":
        assert frame["request"]["subtype"] == "interrupt", frame
    else:
        assert frame["method"] == "turn/interrupt", frame
        assert frame["params"]["threadId"] == f"thread-{runner}", frame
        assert frame["params"]["turnId"] == f"turn-{runner}", frame

replacement = [r for r in records if r.get("prompt", "").strip() == "complete-now replacement"]
replaced = [r for r in records if r.get("prompt", "").strip() == "hold-replacement"]
assert len(replacement) == 1 and len(replaced) == 1, records
replacement_index = records.index(replacement[0])
replaced_interrupt_index = next(
    index for index, record in enumerate(records)
    if record["runner"] == "codex-stop"
    and record["event"] == "interrupt_received"
    and index > records.index(replaced[0])
)
assert replaced_interrupt_index < replacement_index, (replaced_interrupt_index, replacement_index, records)

shutdown_start = next(
    index for index, record in enumerate(records)
    if record.get("prompt", "").strip() == "hold-service-shutdown"
)
assert any(
    record["runner"] == "codex-stop" and record["event"] == "interrupt_received"
    for record in records[shutdown_start + 1:]
), records
PY

echo "[external-runner-worker-stop] PASS"
