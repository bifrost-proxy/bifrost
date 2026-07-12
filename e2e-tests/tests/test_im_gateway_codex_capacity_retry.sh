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
MOCK_LOG="$TEST_DIR/mock-capacity.ndjson"
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
  if [[ "${KEEP_TEST_DIR:-false}" == "true" ]]; then
    echo "[codex-capacity-retry] keeping test dir: $TEST_DIR" >&2
  else
    rm -rf "$TEST_DIR"
  fi
}
trap cleanup EXIT

MOCK_CODEX="$TEST_DIR/codex"
touch "$MOCK_LOG"
python3 - "$MOCK_CODEX" <<'PY'
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
path.write_text(r'''#!/usr/bin/env python3
import json
import os
import sys

mode = os.environ["MOCK_MODE"]
runner = os.environ["MOCK_RUNNER"]
log_path = os.environ["MOCK_LOG"]
thread_id = f"thread-{runner}"
attempt = 0

def send(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)

def record(value):
    with open(log_path, "a", encoding="utf-8") as handle:
        handle.write(json.dumps(value, separators=(",", ":")) + "\n")

if "--version" in sys.argv:
    print("codex-cli 0.0.0-mock")
    sys.exit(0)

for line in sys.stdin:
    frame = json.loads(line)
    method = frame.get("method")
    request_id = frame.get("id")
    if method == "initialize":
        send({"jsonrpc":"2.0","id":request_id,"result":{}})
    elif method in ("thread/start", "thread/resume"):
        send({"jsonrpc":"2.0","method":"thread/started","params":{"thread":{"id":thread_id}}})
        send({"jsonrpc":"2.0","id":request_id,"result":{"thread":{"id":thread_id}}})
    elif method == "turn/start":
        attempt += 1
        turn_id = f"turn-{runner}-{attempt}"
        record({"runner":runner,"attempt":attempt,"threadId":frame["params"]["threadId"],"clientUserMessageId":frame["params"]["clientUserMessageId"],"prompt":frame["params"]["input"][0]["text"]})
        send({"jsonrpc":"2.0","id":request_id,"result":{"turn":{"id":turn_id}}})
        if mode == "retry_success" and attempt > 1:
            send({"jsonrpc":"2.0","method":"item/completed","params":{"threadId":thread_id,"turnId":turn_id,"item":{"id":"message-success","type":"agentMessage","text":"recovered"}}})
            send({"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":thread_id,"turn":{"id":turn_id,"status":"completed"}}})
        elif mode == "ordinary_error":
            send({"jsonrpc":"2.0","method":"error","params":{"threadId":thread_id,"turnId":turn_id,"error":{"message":"invalid request","codexErrorInfo":"other"},"willRetry":False}})
        else:
            if mode == "after_output":
                send({"jsonrpc":"2.0","method":"item/agentMessage/delta","params":{"threadId":thread_id,"turnId":turn_id,"itemId":"message-partial","delta":"partial"}})
            send({"jsonrpc":"2.0","method":"error","params":{"threadId":thread_id,"turnId":turn_id,"error":{"message":"Selected model is at capacity. Please try a different model.","codexErrorInfo":"serverOverloaded"},"willRetry":False}})
''', encoding="utf-8")
path.chmod(0o755)
PY

if [[ "${SKIP_BUILD:-false}" != "true" ]]; then
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

BIFROST_DATA_DIR="$TEST_DIR" "$BIFROST_BIN" start \
  --host 127.0.0.1 \
  -p "$BIFROST_PORT" \
  --unsafe-ssl \
  --skip-cert-check \
  --no-system-proxy \
  >"$BIFROST_LOG" 2>&1 &
BIFROST_PID=$!

for _ in $(seq 1 180); do
  if curl -fsS --noproxy '*' "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address" >/dev/null 2>&1; then
    break
  fi
  if ! kill -0 "$BIFROST_PID" >/dev/null 2>&1; then
    tail -160 "$BIFROST_LOG" >&2 || true
    exit 1
  fi
  sleep 0.25
done

python3 - "$BIFROST_PORT" "$MOCK_CODEX" "$MOCK_LOG" "$REPO_DIR" <<'PY'
import json
import sys
import urllib.request

port, executable, mock_log, repo_dir = sys.argv[1:5]
runners = {}
for runner_id, mode in (
    ("capacity-success", "retry_success"),
    ("capacity-exhausted", "exhausted"),
    ("capacity-ordinary", "ordinary_error"),
    ("capacity-after-output", "after_output"),
):
    runners[runner_id] = {
        "enabled": True,
        "adapter": "codex",
        "adapterConfig": {
            "executable": executable,
            "transport": "app_server",
            "env": {"MOCK_MODE": mode, "MOCK_RUNNER": runner_id, "MOCK_LOG": mock_log},
            "timeoutSecs": 30,
        },
        "injectBifrostTools": False,
        "skillPaths": [],
        "workDir": repo_dir,
        "deliveryMode": "final_reply",
    }

request = urllib.request.Request(
    f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/chat/config",
    data=json.dumps({"version":1,"defaultRunnerId":"capacity-success","runners":runners,"channels":{}}).encode(),
    headers={"content-type":"application/json"},
    method="PATCH",
)
with urllib.request.urlopen(request, timeout=30) as response:
    assert response.status == 200, response.read().decode()
PY

run_case() {
  local runner="$1"
  local expected_status="$2"
  local output="$TEST_DIR/$runner.ndjson"
  set +e
  "$BIFROST_BIN" -H 127.0.0.1 -p "$BIFROST_PORT" agent run \
    --runner "$runner" --session "session-$runner" --json "capacity test" \
    >"$output" 2>&1
  local exit_code=$?
  set -e
  if [[ "$expected_status" == "succeeded" ]]; then
    [[ "$exit_code" -eq 0 ]]
    grep -q '"status":"succeeded"' "$output"
    grep -q '"response":"recovered"' "$output"
  else
    [[ "$exit_code" -ne 0 ]]
    grep -q '"eventType":"run_failed"' "$output"
  fi
}

run_case capacity-success succeeded
run_case capacity-exhausted failed
run_case capacity-ordinary failed
run_case capacity-after-output failed

python3 - "$TEST_DIR" "$MOCK_LOG" <<'PY'
import glob
import json
import os
import sys

test_dir, mock_log = sys.argv[1:3]
records = [json.loads(line) for line in open(mock_log, encoding="utf-8") if line.strip()]

def attempts(runner):
    return [record for record in records if record["runner"] == runner]

success = attempts("capacity-success")
assert len(success) == 2, success
assert {record["threadId"] for record in success} == {"thread-capacity-success"}, success
assert len({record["clientUserMessageId"] for record in success}) == 1, success
assert len(attempts("capacity-exhausted")) == 4, records
assert len(attempts("capacity-ordinary")) == 1, records
assert len(attempts("capacity-after-output")) == 1, records

results = []
for path in glob.glob(os.path.join(test_dir, "agent", "im_gateway", "chat_runs", "*", "result.json")):
    with open(path, encoding="utf-8") as handle:
        results.append(json.load(handle))

success_result = next(result for result in results if result.get("sessionKey") == "session-capacity-success")
assert success_result["status"] == "succeeded", success_result
assert success_result["response"] == "recovered", success_result
assert success_result["metadata"]["runner.capacityRetryCount"] == "1", success_result["metadata"]
assert not any(event["eventType"] == "run_failed" for event in success_result["events"]), success_result["events"]

exhausted = next(result for result in results if result.get("sessionKey") == "session-capacity-exhausted")
assert exhausted["status"] == "failed", exhausted
assert exhausted["metadata"]["runner.capacityRetryCount"] == "3", exhausted["metadata"]
assert any(event["eventType"] == "run_failed" for event in exhausted["events"]), exhausted["events"]
PY

echo "[codex-capacity-retry] PASS"
