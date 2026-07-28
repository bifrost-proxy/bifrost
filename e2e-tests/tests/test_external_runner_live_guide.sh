#!/usr/bin/env bash
set -euo pipefail

unset BIFROST_DETACHED_DAEMON_CHILD

: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_DIR"

TEST_DIR="$(mktemp -d "$REPO_DIR/.bifrost-e2e-external-guide.XXXXXX")"
export BIFROST_DATA_DIR="$TEST_DIR"
source "$REPO_DIR/e2e-tests/test_utils/process.sh"
mark_e2e_data_root "$TEST_DIR"

BIFROST_LOG="$TEST_DIR/bifrost.log"
MOCK_LOG="$TEST_DIR/mock-app-server.ndjson"
BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"

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
    echo "[external-runner-live-guide] keeping test dir: $TEST_DIR" >&2
  else
    rm -rf "$TEST_DIR"
  fi
}
trap cleanup EXIT

cat >"$TEST_DIR/mock-runner" <<'PY'
#!/usr/bin/env python3
import json
import os
import sys
import time

runner = os.environ["MOCK_RUNNER"]
mode = os.environ.get("MOCK_MODE", "accept")
log_path = os.environ["MOCK_LOG"]
thread_id = f"thread-{runner}"
turn_id = f"turn-{runner}"

def send(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)

def record(value):
    with open(log_path, "a", encoding="utf-8") as handle:
        handle.write(json.dumps(value, separators=(",", ":")) + "\n")

if "--version" in sys.argv:
    print(f"{runner} 0.0.0-mock")
    sys.exit(0)

if sys.argv[1:3] == ["debug", "models"]:
    print(json.dumps({
        "models": [{
            "slug": f"{runner}-model",
            "display_name": f"{runner} model",
            "default_reasoning_level": "medium",
            "supported_reasoning_levels": [
                {"effort": "low"},
                {"effort": "medium"},
                {"effort": "high"},
            ],
            "visibility": "list",
        }]
    }))
    sys.exit(0)

if "--input-format" in sys.argv and sys.argv[sys.argv.index("--input-format") + 1] == "stream-json":
    first = json.loads(sys.stdin.readline())
    record({"event":"turn_started","runner":runner,"pid":os.getpid(),"frame":first})
    send({"type":"system","subtype":"init","session_id":thread_id})
    send(first)
    # Publish readiness only after the mock emits the init frame and replays
    # the initial prompt. The adapter then registers its steerable session;
    # app-server uses turn_ready below.
    record({"event":"stream_ready","runner":runner,"pid":os.getpid()})
    prompt = first["message"]["content"][0]["text"]
    if "queue-after-reject" in prompt:
        send({"type":"assistant","message":{"content":[{"type":"text","text":f"QUEUED_{runner}"}]},"session_id":thread_id})
        send({"type":"result","subtype":"success","is_error":False,"result":f"QUEUED_{runner}","session_id":thread_id})
        sys.exit(0)
    interrupt = json.loads(sys.stdin.readline())
    assert interrupt["type"] == "control_request", interrupt
    assert interrupt["request"]["subtype"] == "interrupt", interrupt
    record({"event":"stream_interrupt_received","runner":runner,"pid":os.getpid(),"frame":interrupt})
    if mode == "reject":
        send({"type":"control_response","response":{"subtype":"error","request_id":interrupt["request_id"],"error":"no active Claude response"}})
        time.sleep(0.5)
        send({"type":"assistant","message":{"content":[{"type":"text","text":f"FIRST_{runner}"}]},"session_id":thread_id})
        send({"type":"result","subtype":"success","is_error":False,"result":f"FIRST_{runner}","session_id":thread_id})
        sys.exit(0)
    send({"type":"control_response","response":{"subtype":"success","request_id":interrupt["request_id"],"response":{}}})
    guide = json.loads(sys.stdin.readline())
    record({"event":"stream_guide_received","runner":runner,"pid":os.getpid(),"frame":guide})
    send(guide)
    send({"type":"result","subtype":"error_during_execution","is_error":True,"session_id":thread_id})
    send({"type":"system","subtype":"init","session_id":thread_id})
    send({"type":"assistant","message":{"content":[{"type":"tool_use","id":"tool-claude","name":"Bash","input":{"command":"pwd"}}]},"session_id":thread_id})
    send({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tool-claude","content":"/tmp"}]},"session_id":thread_id})
    send({"type":"assistant","message":{"content":[{"type":"text","text":f"GUIDED_{runner}"}]},"session_id":thread_id})
    send({"type":"result","subtype":"success","is_error":False,"result":f"GUIDED_{runner}","session_id":thread_id,"usage":{"input_tokens":11,"output_tokens":7}})
    sys.exit(0)

if "app-server" not in sys.argv:
    prompt = sys.stdin.read()
    record({"event":"exec_started","runner":runner,"prompt":prompt,"argv":sys.argv[1:]})
    # The coverage-instrumented CLI has materially higher startup overhead.
    # Keep the exec transport active long enough for the guide command to be
    # issued after the explicit exec_started readiness marker.
    time.sleep(10 if runner.endswith("-exec") else 1)
    if runner.startswith("claude"):
        send({"type":"system","subtype":"init","session_id":thread_id})
        send({"type":"assistant","message":{"content":[{"type":"text","text":f"EXEC_{runner}"}]},"session_id":thread_id})
        send({"type":"result","subtype":"success","is_error":False,"result":f"EXEC_{runner}","session_id":thread_id})
    else:
        send({"type":"thread.started","thread_id":thread_id})
        send({"type":"turn.started"})
        send({"type":"item.completed","item":{"id":"message-1","type":"agent_message","text":f"EXEC_{runner}"}})
        send({"type":"turn.completed","usage":{"input_tokens":5,"cached_input_tokens":0,"output_tokens":3,"reasoning_output_tokens":0,"total_tokens":8}})
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
        record({"event":"turn_started","runner":runner,"params":frame["params"]})
        send({"jsonrpc":"2.0","id":request_id,"result":{"turn":{"id":turn_id}}})
        # The product registers the steerable session only after consuming the
        # turn/start response. Publish a separate readiness marker after that
        # protocol boundary instead of racing on turn_started above.
        time.sleep(0.5)
        record({"event":"turn_ready","runner":runner})
        prompt = frame["params"]["input"][0]["text"]
        if "queue-explicit" in prompt:
            send({"jsonrpc":"2.0","method":"item/completed","params":{"threadId":thread_id,"turnId":turn_id,"item":{"id":"message-queued-explicit","type":"agentMessage","text":f"QUEUED_EXPLICIT_{runner}"}}})
            send({"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":thread_id,"turn":{"id":turn_id,"status":"completed"}}})
        if "QUOTE_CURRENT_QUESTION" in prompt:
            send({"jsonrpc":"2.0","method":"item/completed","params":{"threadId":thread_id,"turnId":turn_id,"item":{"id":"message-quote-current","type":"agentMessage","text":"QUOTE_CONTEXT_COMPLETE"}}})
            send({"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":thread_id,"turn":{"id":turn_id,"status":"completed"}}})
        if mode == "reject" and "queue-after-reject" in prompt:
            send({"jsonrpc":"2.0","method":"item/completed","params":{"threadId":thread_id,"turnId":turn_id,"item":{"id":"message-queued","type":"agentMessage","text":f"QUEUED_{runner}"}}})
            send({"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":thread_id,"turn":{"id":turn_id,"status":"completed"}}})
    elif method == "turn/steer":
        record({"event":"turn_steered","runner":runner,"params":frame["params"]})
        if mode == "reject":
            send({"jsonrpc":"2.0","id":request_id,"error":{"code":-32600,"message":"no active turn to steer"}})
            time.sleep(0.5)
            send({"jsonrpc":"2.0","method":"item/completed","params":{"threadId":thread_id,"turnId":turn_id,"item":{"id":"message-first","type":"agentMessage","text":f"FIRST_{runner}"}}})
            send({"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":thread_id,"turn":{"id":turn_id,"status":"completed"}}})
            continue
        send({"jsonrpc":"2.0","id":request_id,"result":{"turnId":turn_id}})
        send({"jsonrpc":"2.0","method":"item/started","params":{"threadId":thread_id,"turnId":turn_id,"item":{"id":"command-1","type":"commandExecution","command":"pwd","aggregatedOutput":"","exitCode":None}}})
        send({"jsonrpc":"2.0","method":"item/completed","params":{"threadId":thread_id,"turnId":turn_id,"item":{"id":"command-1","type":"commandExecution","command":"pwd","aggregatedOutput":"/tmp\n","exitCode":0,"durationMs":4}}})
        send({"jsonrpc":"2.0","method":"item/completed","params":{"threadId":thread_id,"turnId":turn_id,"item":{"id":"message-1","type":"agentMessage","text":f"GUIDED_{runner}"}}})
        send({"jsonrpc":"2.0","method":"thread/tokenUsage/updated","params":{"threadId":thread_id,"turnId":turn_id,"tokenUsage":{"last":{"inputTokens":11,"cachedInputTokens":2,"outputTokens":7,"reasoningOutputTokens":3,"totalTokens":18},"total":{"inputTokens":11,"cachedInputTokens":2,"outputTokens":7,"reasoningOutputTokens":3,"totalTokens":18},"modelContextWindow":1000}}})
        send({"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":thread_id,"turn":{"id":turn_id,"status":"completed"}}})
PY
chmod +x "$TEST_DIR/mock-runner"

if [[ "${SKIP_BUILD:-false}" != "true" ]]; then
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

START_ARGS=(
  --daemon
  --host 127.0.0.1
  -p "$BIFROST_PORT"
  --unsafe-ssl
  --skip-cert-check
  --no-system-proxy
)
if "$BIFROST_BIN" start --help | grep -q -- '--no-tray'; then
  START_ARGS+=(--no-tray)
fi

if ! BIFROST_EXTERNAL_CLI_WORKER=1 BIFROST_DATA_DIR="$TEST_DIR" "$BIFROST_BIN" start \
  "${START_ARGS[@]}" \
  >"$BIFROST_LOG" 2>&1; then
  echo "[external-runner-live-guide] detached daemon failed to start" >&2
  tail -160 "$BIFROST_LOG" >&2 || true
  exit 1
fi

for _ in $(seq 1 180); do
  if curl -fsS --noproxy '*' "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address" >/dev/null 2>&1; then
    break
  fi
  sleep 0.25
done

if ! curl -fsS --noproxy '*' "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address" >/dev/null; then
  tail -160 "$BIFROST_LOG" >&2 || true
  exit 1
fi

BIFROST_PID="$(pid_from_runtime_file "$TEST_DIR/runtime.json")"
if [[ ! "$BIFROST_PID" =~ ^[0-9]+$ ]] || ! kill -0 "$BIFROST_PID" >/dev/null 2>&1; then
  echo "[external-runner-live-guide] detached daemon PID is not running: ${BIFROST_PID:-missing}" >&2
  tail -160 "$BIFROST_LOG" >&2 || true
  exit 1
fi

python3 - "$BIFROST_PORT" "$TEST_DIR/mock-runner" "$MOCK_LOG" "$REPO_DIR" <<'PY'
import json
import sys
import urllib.request

port, executable, mock_log, repo_dir = sys.argv[1:5]
def runner(adapter, mode="accept", transport="app_server"):
    return {
        "enabled": True,
        "adapter": adapter,
        "adapterConfig": {
            "executable": executable,
            "transport": transport,
            "env": {"MOCK_RUNNER": runner_name, "MOCK_MODE": mode, "MOCK_LOG": mock_log},
            "sandbox": "read-only",
            "approvalPolicy": "never",
            "timeoutSecs": 30,
        },
        "injectBifrostTools": False,
        "skillPaths": [],
        "workDir": repo_dir,
        "deliveryMode": "progress_card",
    }

configured = {}
for runner_name, adapter, mode, transport in (
    ("codex", "codex", "accept", "app_server"),
    ("traex", "traex", "accept", "app_server"),
    ("codex-web", "codex", "accept", "app_server"),
    ("traex-web", "traex", "accept", "app_server"),
    ("codex-im", "codex", "accept", "app_server"),
    ("codex-cross-channel", "codex", "accept", "app_server"),
    ("codex-im-queue", "codex", "accept", "app_server"),
    ("codex-im-effort", "codex", "accept", "app_server"),
    ("traex-im-effort", "traex", "accept", "app_server"),
    ("codex-im-quote", "codex", "accept", "app_server"),
    ("codex-reject", "codex", "reject", "app_server"),
    ("claude-reject", "claude_code", "reject", None),
    ("codex-exec", "codex", "accept", "exec"),
    ("claude-exec", "claude_code", "accept", "exec"),
    ("claude", "claude_code", "accept", None),
):
    configured[runner_name] = runner(adapter, mode, transport)

payload = {
    "version": 1,
    "defaultRunnerId": "codex",
    "runners": configured,
    "channels": {},
}
request = urllib.request.Request(
    f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/chat/config",
    data=json.dumps(payload).encode(),
    headers={"content-type":"application/json"},
    method="PATCH",
)
with urllib.request.urlopen(request, timeout=30) as response:
    assert response.status == 200, response.read().decode()
PY

run_case() {
  local runner="$1"
  local session="live-guide-$runner"
  local stream_log="$TEST_DIR/$runner-stream.ndjson"
  local guide_log="$TEST_DIR/$runner-guide.json"
  local wait_event="turn_ready"
  [[ "$runner" == claude* ]] && wait_event="stream_ready"

  "$BIFROST_BIN" -H 127.0.0.1 -p "$BIFROST_PORT" agent run \
    --runner "$runner" --session "$session" --json \
    "wait for live guidance" >"$stream_log" 2>&1 &
  local run_pid=$!

  for _ in $(seq 1 200); do
    if grep -q "\"event\":\"$wait_event\",\"runner\":\"$runner\"" "$MOCK_LOG" 2>/dev/null; then
      break
    fi
    if ! kill -0 "$run_pid" >/dev/null 2>&1; then
      cat "$stream_log" >&2 || true
      return 1
    fi
    sleep 0.05
  done

  "$BIFROST_BIN" -H 127.0.0.1 -p "$BIFROST_PORT" agent guide \
    --session "$session" --json "focus-$runner" >"$guide_log"
  wait "$run_pid"

  python3 - "$runner" "$guide_log" "$stream_log" "$MOCK_LOG" "$BIFROST_PORT" <<'PY'
import json
import sys
import urllib.request

runner, guide_path, stream_path, mock_path, port = sys.argv[1:6]
guide = json.load(open(guide_path, encoding="utf-8"))
assert guide["delivery"] == "steered", guide
assert guide["threadId"] == f"thread-{runner}", guide
if runner == "claude":
    assert guide.get("turnId") is None, guide
else:
    assert guide["turnId"] == f"turn-{runner}", guide

events = [json.loads(line) for line in open(stream_path, encoding="utf-8") if line.strip().startswith("{")]
finished = [event for event in events if event.get("eventType") == "run_finished"]
assert len(finished) == 1, events
assert finished[0]["status"] == "succeeded", finished[0]
assert finished[0]["response"] == f"GUIDED_{runner}", finished[0]
tool_started = [index for index, event in enumerate(events) if event.get("eventType") == "tool_started"]
tool_finished = [index for index, event in enumerate(events) if event.get("eventType") == "tool_finished"]
assert len(tool_started) == len(tool_finished) == 1, events
assert tool_started[0] < tool_finished[0] < events.index(finished[0]), events

records = [json.loads(line) for line in open(mock_path, encoding="utf-8")]
if runner == "claude":
    interrupts = [record for record in records if record.get("event") == "stream_interrupt_received" and record.get("runner") == runner]
    assert len(interrupts) == 1, records
    assert interrupts[0]["frame"]["request"]["subtype"] == "interrupt", interrupts
    steered = [record for record in records if record.get("event") == "stream_guide_received" and record.get("runner") == runner]
    assert len(steered) == 1, records
    frame = steered[0]["frame"]
    assert frame["type"] == "user", frame
    assert frame["message"]["content"][0]["text"] == f"focus-{runner}", frame
    starts = [record for record in records if record.get("event") == "turn_started" and record.get("runner") == runner]
    assert len(starts) == 1 and starts[0]["pid"] == steered[0]["pid"], records
    ready = [record for record in records if record.get("event") == "stream_ready" and record.get("runner") == runner]
    assert len(ready) == 1 and ready[0]["pid"] == steered[0]["pid"], records
else:
    steered = [record for record in records if record.get("event") == "turn_steered" and record.get("runner") == runner]
    assert len(steered) == 1, records
    params = steered[0]["params"]
    assert params["expectedTurnId"] == f"turn-{runner}", params
    assert params["input"][0]["text"] == f"focus-{runner}", params
    assert params["clientUserMessageId"].startswith("guide-"), params

run_id = finished[0]["runId"]
with urllib.request.urlopen(
    f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/chat/runs/{run_id}", timeout=30
) as response:
    detail = json.loads(response.read().decode())
args = detail["snapshot"]["args"]
if runner == "claude":
    assert "--input-format" in args, args
    assert args[args.index("--input-format") + 1] == "stream-json", args
    assert "--replay-user-messages" in args, args
elif runner == "traex":
    assert args[:3] == ["app-server", "--listen", "stdio://"], args
else:
    assert args[:2] == ["app-server", "--stdio"], args
metadata = detail["metadata"]
assert metadata["threadId"] == f"thread-{runner}", metadata
assert metadata["usageInputTokens"] == "11", metadata
assert metadata["usageOutputTokens"] == "7", metadata
if runner != "claude":
    assert metadata["usageTotalTokens"] == "18", metadata
PY
}

run_case codex
run_case traex
run_case claude

run_web_guide_case() {
  local runner="$1"
  local session="web-guide-$runner"
  local stream_log="$TEST_DIR/$runner-web-stream.ndjson"
  local guide_log="$TEST_DIR/$runner-web-guide.ndjson"

  curl -sS -N --noproxy '*' \
    -H 'content-type: application/json' \
    -d "{\"message\":\"wait for web guidance\",\"runnerId\":\"$runner\",\"sessionKey\":\"$session\",\"runtime\":\"external_cli\"}" \
    "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/chat/stream" \
    >"$stream_log" &
  local stream_pid=$!

  for _ in $(seq 1 200); do
    if grep -q "\"event\":\"turn_ready\",\"runner\":\"$runner\"" "$MOCK_LOG" 2>/dev/null; then
      break
    fi
    kill -0 "$stream_pid" >/dev/null 2>&1 || return 1
    sleep 0.05
  done

  curl -fsS --noproxy '*' \
    -H 'content-type: application/json' \
    -d "{\"message\":\"/g focus-$runner\",\"runnerId\":\"$runner\",\"sessionKey\":\"$session\",\"runtime\":\"external_cli\"}" \
    "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/chat/stream" \
    >"$guide_log"
  wait "$stream_pid"

  python3 - "$runner" "$guide_log" "$stream_log" <<'PY'
import json
import sys

runner, guide_path, stream_path = sys.argv[1:4]
guide = [json.loads(line) for line in open(guide_path, encoding="utf-8") if line.strip()]
assert len(guide) == 1, guide
assert guide[0]["delivery"] == "steered", guide
assert guide[0]["guide"] is True, guide
events = [json.loads(line) for line in open(stream_path, encoding="utf-8") if line.strip()]
finished = [event for event in events if event.get("eventType") == "run_finished"]
assert len(finished) == 1, events
assert finished[0]["response"] == f"GUIDED_{runner}", finished
PY
}

run_web_guide_case codex-web
run_web_guide_case traex-web

create_im_provider() {
  local provider_id="$1"
  local owner_id="$2"
  local runner="$3"
  curl -fsS --noproxy '*' -X POST \
    "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/providers" \
    -H 'content-type: application/json' \
    -d "{\"id\":\"$provider_id\",\"provider_type\":\"feishu\",\"display_name\":\"Guide E2E\",\"enabled\":true,\"app_id\":\"cli_guide_e2e\",\"owner_open_id\":\"$owner_id\",\"event_connection_enabled\":false,\"agent_config\":{\"runner\":\"$runner\"}}" \
    >/dev/null
}

send_im_inbound() {
  local provider_id="$1"
  local owner_id="$2"
  local text="$3"
  curl -fsS --noproxy '*' -X POST \
    "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/debug/mock-inbound" \
    -H 'content-type: application/json' \
    -d "{\"providerId\":\"$provider_id\",\"userId\":\"$owner_id\",\"chatId\":\"chat-$provider_id\",\"text\":\"$text\"}" \
    >/dev/null
}

send_im_inbound_with_reference() {
  local provider_id="$1"
  local owner_id="$2"
  local text="$3"
  local message_id="$4"
  local reply_message_id="${5:-}"
  python3 - "$BIFROST_PORT" "$provider_id" "$owner_id" "$text" "$message_id" "$reply_message_id" <<'PY'
import json
import sys
import urllib.request

port, provider_id, owner_id, text, message_id, reply_message_id = sys.argv[1:7]
payload = {
    "providerId": provider_id,
    "userId": owner_id,
    "chatId": f"chat-{provider_id}",
    "text": text,
    "messageId": message_id,
}
if reply_message_id:
    payload["replyTo"] = {"messageId": reply_message_id}
request = urllib.request.Request(
    f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/debug/mock-inbound",
    data=json.dumps(payload).encode(),
    headers={"content-type": "application/json"},
    method="POST",
)
with urllib.request.urlopen(request, timeout=30) as response:
    assert response.status == 200, response.read().decode()
PY
}

wait_for_mock_record() {
  local pattern="$1"
  for _ in $(seq 1 240); do
    if grep -q "$pattern" "$MOCK_LOG" 2>/dev/null; then
      return 0
    fi
    sleep 0.05
  done
  echo "[external-runner-live-guide] missing mock record: $pattern" >&2
  tail -120 "$BIFROST_LOG" >&2 || true
  tail -80 "$MOCK_LOG" >&2 || true
  return 1
}

create_im_provider "im-guide-provider" "im-guide-owner" "codex-im"
send_im_inbound "im-guide-provider" "im-guide-owner" "wait for default IM guide"
wait_for_mock_record '"event":"turn_ready","runner":"codex-im"'
send_im_inbound "im-guide-provider" "im-guide-owner" "default-im-guide"
wait_for_mock_record 'default-im-guide'

create_im_provider "cross-channel-provider" "cross-channel-owner" "codex-cross-channel"
curl -fsS --noproxy '*' -X PATCH \
  "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/chat/config/channels/cross-channel-provider" \
  -H 'content-type: application/json' \
  -d '{"runnerId":"codex-cross-channel","deliveryMode":"no_im"}' \
  >/dev/null
cross_channel_stream="$TEST_DIR/cross-channel-web-stream.ndjson"
curl -sS -N --noproxy '*' \
  -H 'content-type: application/json' \
  -d '{"message":"wait for IM guidance on the Web-started turn","runnerId":"codex-cross-channel","sessionKey":"cross-channel-provider:cross-channel-owner","runtime":"external_cli"}' \
  "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/chat/stream" \
  >"$cross_channel_stream" &
cross_channel_pid=$!
wait_for_mock_record '"event":"turn_ready","runner":"codex-cross-channel"'
send_im_inbound "cross-channel-provider" "cross-channel-owner" "guide-from-im-to-web"
wait "$cross_channel_pid"
python3 - "$cross_channel_stream" "$MOCK_LOG" <<'PY'
import json
import sys

events = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8") if line.strip()]
finished = [event for event in events if event.get("eventType") == "run_finished"]
assert len(finished) == 1, events
assert finished[0]["response"] == "GUIDED_codex-cross-channel", finished
records = [json.loads(line) for line in open(sys.argv[2], encoding="utf-8") if line.strip()]
steered = [
    record for record in records
    if record.get("event") == "turn_steered"
    and record.get("runner") == "codex-cross-channel"
]
assert len(steered) == 1, steered
assert steered[0]["params"]["input"][0]["text"] == "guide-from-im-to-web", steered
PY

create_im_provider "im-queue-provider" "im-queue-owner" "codex-im-queue"
send_im_inbound "im-queue-provider" "im-queue-owner" "wait for explicit IM queue"
wait_for_mock_record '"event":"turn_ready","runner":"codex-im-queue"'
send_im_inbound "im-queue-provider" "im-queue-owner" "/q queue-explicit"
send_im_inbound "im-queue-provider" "im-queue-owner" "/g release-queue"
wait_for_mock_record 'queue-explicit'

run_im_effort_case() {
  local adapter="$1"
  local runner="$adapter-im-effort"
  local provider="im-$adapter-effort-provider"
  local owner="im-$adapter-effort-owner"

  create_im_provider "$provider" "$owner" "$runner"
  send_im_inbound "$provider" "$owner" "wait for $adapter effort commands"
  wait_for_mock_record "\"event\":\"turn_ready\",\"runner\":\"$runner\""
  send_im_inbound "$provider" "$owner" "/efforts"
  send_im_inbound "$provider" "$owner" "/effort high"

  python3 - "$TEST_DIR/agent/im_gateway/session_state.json" "$runner" <<'PY'
import json
import pathlib
import sys
import time

state_path = pathlib.Path(sys.argv[1])
runner = sys.argv[2]
for _ in range(200):
    if state_path.exists():
        state = json.loads(state_path.read_text(encoding="utf-8"))
        sessions = [
            value for value in state.get("sessions", {}).values()
            if value.get("runnerId") == runner
        ]
        if sessions and sessions[0].get("reasoningEffortOverride") == "high":
            break
    time.sleep(0.05)
else:
    raise AssertionError(f"missing high effort override for {runner}")
PY

  send_im_inbound "$provider" "$owner" "/g release-$adapter-after-effort"
  wait_for_mock_record "release-$adapter-after-effort"
}

run_im_effort_case codex
run_im_effort_case traex

create_im_provider "im-quote-provider" "im-quote-owner" "codex-im-quote"
send_im_inbound_with_reference \
  "im-quote-provider" \
  "im-quote-owner" \
  "QUOTE_SOURCE_REQUEST https://example.com/quoted-article" \
  "quote-source-message"
wait_for_mock_record 'QUOTE_SOURCE_REQUEST'
send_im_inbound_with_reference \
  "im-quote-provider" \
  "im-quote-owner" \
  "QUOTE_CURRENT_QUESTION 这条引用里的链接是什么？" \
  "quote-current-message" \
  "quote-source-message"
wait_for_mock_record 'QUOTE_CURRENT_QUESTION'

python3 - "$MOCK_LOG" "$TEST_DIR/agent/im_gateway/session_state.json" <<'PY'
import json
import sys

records = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8")]
default_steers = [
    record for record in records
    if record.get("event") == "turn_steered" and record.get("runner") == "codex-im"
]
assert len(default_steers) == 1, default_steers
assert default_steers[0]["params"]["input"][0]["text"] == "default-im-guide", default_steers

queue_steers = [
    record for record in records
    if record.get("event") == "turn_steered" and record.get("runner") == "codex-im-queue"
]
assert len(queue_steers) == 1, queue_steers
assert queue_steers[0]["params"]["input"][0]["text"] == "release-queue", queue_steers
assert all(
    record["params"]["input"][0]["text"] != "queue-explicit"
    for record in queue_steers
), queue_steers
queued_turns = [
    record for record in records
    if record.get("event") == "turn_started"
    and record.get("runner") == "codex-im-queue"
    and "queue-explicit" in record.get("params", {}).get("input", [{}])[0].get("text", "")
]
assert len(queued_turns) == 1, queued_turns

for adapter in ("codex", "traex"):
    runner = f"{adapter}-im-effort"
    effort_steers = [
        record for record in records
        if record.get("event") == "turn_steered" and record.get("runner") == runner
    ]
    assert len(effort_steers) == 1, effort_steers
    assert effort_steers[0]["params"]["input"][0]["text"] == f"release-{adapter}-after-effort", effort_steers
    assert all(
        record["params"]["input"][0]["text"] not in ("/efforts", "/effort high")
        for record in effort_steers
    ), effort_steers

state = json.load(open(sys.argv[2], encoding="utf-8"))
for adapter in ("codex", "traex"):
    runner = f"{adapter}-im-effort"
    session = next(
        value for value in state["sessions"].values()
        if value.get("runnerId") == runner
    )
    assert session["reasoningEffortOverride"] == "high", session
    assert session["reasoningEffortOverrideSource"] == "session slash command", session

quote_steers = [
    record for record in records
    if record.get("event") == "turn_steered" and record.get("runner") == "codex-im-quote"
]
assert len(quote_steers) == 1, quote_steers
quote_prompt = quote_steers[0]["params"]["input"][0]["text"]
assert "【引用消息（仅作为上下文）】" in quote_prompt, quote_prompt
assert "QUOTE_SOURCE_REQUEST https://example.com/quoted-article" in quote_prompt, quote_prompt
assert "【当前消息】" in quote_prompt, quote_prompt
assert "QUOTE_CURRENT_QUESTION 这条引用里的链接是什么？" in quote_prompt, quote_prompt
PY

run_queue_fallback_case() {
  local runner="$1"
  local expected_delivery="$2"
  local session="live-guide-$runner"
  local stream_log="$TEST_DIR/$runner-stream.ndjson"
  local guide_log="$TEST_DIR/$runner-guide.json"
  local wait_event="turn_ready"
  [[ "$runner" == claude* ]] && wait_event="stream_ready"
  [[ "$runner" == *"-exec" ]] && wait_event="exec_started"

  curl -sS -N --noproxy '*' \
    -H 'content-type: application/json' \
    -d "{\"message\":\"wait for fallback\",\"runnerId\":\"$runner\",\"sessionKey\":\"$session\",\"runtime\":\"external_cli\"}" \
    "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/chat/stream" \
    >"$stream_log" &
  local stream_pid=$!

  for _ in $(seq 1 200); do
    if grep -q "\"event\":\"$wait_event\",\"runner\":\"$runner\"" "$MOCK_LOG" 2>/dev/null; then
      break
    fi
    kill -0 "$stream_pid" >/dev/null 2>&1 || return 1
    sleep 0.05
  done

  "$BIFROST_BIN" -H 127.0.0.1 -p "$BIFROST_PORT" agent guide \
    --session "$session" --json "queue-after-reject" >"$guide_log"
  wait "$stream_pid"

  python3 - "$runner" "$expected_delivery" "$guide_log" "$stream_log" <<'PY'
import json
import sys

runner, expected_delivery, guide_path, stream_path = sys.argv[1:5]
guide = json.load(open(guide_path, encoding="utf-8"))
assert guide["delivery"] == expected_delivery, guide
events = [json.loads(line) for line in open(stream_path, encoding="utf-8") if line.strip()]
finished = [event for event in events if event.get("eventType") == "run_finished"]
assert len(finished) == 2, events
if runner in ("codex-reject", "claude-reject"):
    assert finished[0]["response"] == f"FIRST_{runner}", finished
    assert finished[1]["response"] == f"QUEUED_{runner}", finished
    expected_reason = "no active turn to steer" if runner == "codex-reject" else "no active Claude response"
    assert expected_reason in guide["reason"], guide
else:
    assert all(event["response"] == f"EXEC_{runner}" for event in finished), finished
    assert "exec transport" in guide["reason"], guide
PY
}

run_queue_fallback_case codex-reject queued
run_queue_fallback_case claude-reject queued
run_queue_fallback_case codex-exec queued
run_queue_fallback_case claude-exec queued

python3 - "$MOCK_LOG" <<'PY'
import json
import sys

records = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8") if line.strip()]
claude_exec = [
    record for record in records
    if record.get("event") == "exec_started" and record.get("runner") == "claude-exec"
]
assert len(claude_exec) == 2, claude_exec
for record in claude_exec:
    argv = record["argv"]
    input_format = argv[argv.index("--input-format") + 1]
    assert input_format == "text", record
    assert "--replay-user-messages" not in argv, record
PY

python3 - "$BIFROST_PORT" <<'PY'
import json
import sys
import urllib.error
import urllib.request

port = sys.argv[1]
request = urllib.request.Request(
    f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/chat/sessions/not-active/guide",
    data=json.dumps({"message":"too late"}).encode(),
    headers={"content-type":"application/json"},
    method="POST",
)
try:
    urllib.request.urlopen(request, timeout=30)
    raise AssertionError("inactive session guide should return conflict")
except urllib.error.HTTPError as error:
    assert error.code == 409, error
    body = json.loads(error.read().decode())
    assert body["delivery"] == "rejected", body
PY

echo "[external-runner-live-guide] PASS"
