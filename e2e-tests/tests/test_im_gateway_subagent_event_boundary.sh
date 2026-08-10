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
MOCK_APP_SERVER="$TEST_DIR/mock-app-server"
MOCK_CLAUDE="$TEST_DIR/mock-claude"
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
    echo "[subagent-event-boundary] keeping test dir: $TEST_DIR" >&2
  else
    rm -rf "$TEST_DIR"
  fi
}
trap cleanup EXIT

python3 - "$MOCK_APP_SERVER" <<'PY'
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
path.write_text(r'''#!/usr/bin/env python3
import json
import sys

def send(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)

if "--version" in sys.argv:
    print("collaboration-app-server 0.0.0-mock")
    raise SystemExit(0)

for line in sys.stdin:
    frame = json.loads(line)
    method = frame.get("method")
    request_id = frame.get("id")
    if method == "initialize":
        send({"jsonrpc":"2.0","id":request_id,"result":{}})
    elif method in ("thread/start", "thread/resume"):
        send({"jsonrpc":"2.0","method":"thread/started","params":{"thread":{"id":"root-thread"}}})
        send({"jsonrpc":"2.0","id":request_id,"result":{"thread":{"id":"root-thread"}}})
    elif method == "turn/start":
        send({"jsonrpc":"2.0","method":"item/completed","params":{"threadId":"child-thread","turnId":"child-turn","item":{"id":"early-child-message","type":"agentMessage","text":"EARLY_CHILD_FINAL_MUST_NOT_ESCAPE"}}})
        send({"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":"child-thread","turn":{"id":"child-turn","status":"completed"}}})
        send({"jsonrpc":"2.0","id":request_id,"result":{"turn":{"id":"root-turn"}}})
        send({"jsonrpc":"2.0","method":"item/started","params":{"threadId":"root-thread","turnId":"root-turn","item":{"id":"collab-1","type":"collabAgentToolCall","tool":"spawnAgent","status":"inProgress","prompt":"inspect the boundary","receiverThreadIds":[],"agentsStates":{}}}})
        send({"jsonrpc":"2.0","method":"item/completed","params":{"threadId":"child-thread","turnId":"child-turn","item":{"id":"child-message","type":"agentMessage","text":"CHILD_FINAL_MUST_NOT_ESCAPE"}}})
        send({"jsonrpc":"2.0","method":"item/completed","params":{"threadId":"child-thread","turnId":"child-turn","item":{"id":"child-tool","type":"commandExecution","command":"false","aggregatedOutput":"CHILD_TOOL_MUST_NOT_ESCAPE","exitCode":1}}})
        send({"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":"child-thread","turn":{"id":"child-turn","status":"completed"}}})
        send({
            "jsonrpc": "2.0",
            "method": "item/completed",
            "params": {
                "threadId": "root-thread",
                "turnId": "root-turn",
                "item": {
                    "id": "collab-1",
                    "type": "collabAgentToolCall",
                    "tool": "spawnAgent",
                    "status": "completed",
                    "prompt": "inspect the boundary",
                    "result": "child result received",
                    "receiverThreadIds": ["child-thread"],
                    "agentsStates": {
                        "child-thread": {
                            "status": "completed",
                            "message": "CHILD_INTERNAL_STATE_MUST_NOT_ESCAPE"
                        }
                    }
                }
            }
        })
        send({"jsonrpc":"2.0","method":"item/completed","params":{"threadId":"root-thread","turnId":"root-turn","item":{"id":"root-message","type":"agentMessage","text":"ROOT_FINAL_OK"}}})
        send({"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":"root-thread","turn":{"id":"root-turn","status":"completed"}}})
''', encoding="utf-8")
path.chmod(0o755)
PY

python3 - "$MOCK_CLAUDE" <<'PY'
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
path.write_text(r'''#!/usr/bin/env python3
import json
import sys

if "--version" in sys.argv:
    print("claude-code 0.0.0-mock")
    raise SystemExit(0)

events = [
    {"type":"assistant","message":{"content":[{"type":"tool_use","id":"task-1","name":"Task","input":{"description":"Inspect boundary","prompt":"verify child completion","subagent_type":"reviewer"}}]}},
    {"type":"user","message":{"content":[{"tool_use_id":"task-1","type":"tool_result","content":"child result received","is_error":False}]},"tool_use_result":{"agentId":"claude-child","totalDurationMs":12,"interrupted":False}},
    {"type":"assistant","message":{"content":[{"type":"text","text":"ROOT_CLAUDE_FINAL_OK"}]}},
    {"type":"result","is_error":False,"result":"ROOT_CLAUDE_FINAL_OK"},
]
for event in events:
    print(json.dumps(event, separators=(",", ":")), flush=True)
''', encoding="utf-8")
path.chmod(0o755)
PY

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
for _ in $(seq 1 180); do
  if ! kill -0 "$BIFROST_PID" >/dev/null 2>&1; then
    tail -160 "$BIFROST_LOG" >&2 || true
    exit 1
  fi
  if curl -fsS --noproxy '*' "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address" >/dev/null 2>&1; then
    READY=true
    break
  fi
  sleep 0.25
done
if [[ "$READY" != "true" ]]; then
  tail -160 "$BIFROST_LOG" >&2 || true
  exit 1
fi

python3 - "$BIFROST_PORT" "$MOCK_APP_SERVER" "$MOCK_CLAUDE" "$REPO_DIR" <<'PY'
import json
import sys
import urllib.request

port, app_server, claude, repo_dir = sys.argv[1:5]
endpoint = f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/chat"

def run(adapter, executable, transport):
    payload = {
        "message": "delegate once, wait for the child, then return the root result",
        "sessionKey": f"subagent-event-boundary-{adapter}",
        "runtime": "external_cli",
        "adapter": adapter,
        "workDir": repo_dir,
        "allowWorkDirs": [repo_dir],
        "injectBifrostTools": False,
        "adapterConfig": {
            "executable": executable,
            "transport": transport,
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

for adapter in ("codex", "traex"):
    result = run(adapter, app_server, "app_server")
    assert result["status"] == "succeeded", result
    assert result["response"] == "ROOT_FINAL_OK", result
    serialized = json.dumps(result["events"], ensure_ascii=False)
    for forbidden in (
        "CHILD_FINAL_MUST_NOT_ESCAPE",
        "EARLY_CHILD_FINAL_MUST_NOT_ESCAPE",
        "CHILD_TOOL_MUST_NOT_ESCAPE",
        "CHILD_INTERNAL_STATE_MUST_NOT_ESCAPE",
        "subagent_updated",
    ):
        assert forbidden not in serialized, (adapter, forbidden, result)
    assert sum(event.get("eventType") == "run_finished" for event in result["events"]) == 1, result
    collaboration = [
        event for event in result["events"]
        if event.get("eventType") in ("tool_started", "tool_finished")
        and event.get("title") == "spawnAgent"
    ]
    assert [event["eventType"] for event in collaboration] == ["tool_started", "tool_finished"], result
    assert collaboration[0]["raw"]["arguments"]["prompt"] == "inspect the boundary", collaboration
    assert collaboration[1]["content"] == "child result received", collaboration

claude_result = run("claude_code", claude, "exec")
assert claude_result["status"] == "succeeded", claude_result
assert claude_result["response"] == "ROOT_CLAUDE_FINAL_OK", claude_result
claude_events = claude_result["events"]
assert not any(event.get("eventType") == "subagent_updated" for event in claude_events), claude_result
task_events = [event for event in claude_events if event.get("title") == "Task"]
assert [event["eventType"] for event in task_events] == ["tool_started", "tool_finished"], claude_result
assert task_events[0]["raw"]["arguments"]["prompt"] == "verify child completion", task_events
assert task_events[1]["content"] == "child result received", task_events
assert task_events[1]["raw"]["success"] is True, task_events
PY

RUN_REAL_TRAEX="${RUN_REAL_TRAEX_SUBAGENT_E2E:-${RUN_REAL_SUBAGENT_E2E:-false}}"
RUN_REAL_CLAUDE="${RUN_REAL_CLAUDE_SUBAGENT_E2E:-${RUN_REAL_SUBAGENT_E2E:-false}}"
if [[ "$RUN_REAL_TRAEX" == "true" || "$RUN_REAL_CLAUDE" == "true" ]]; then
  if [[ "$RUN_REAL_TRAEX" == "true" ]]; then
    : "${BIFROST_TRAEX_BIN:?set BIFROST_TRAEX_BIN for real Trae X verification}"
  fi
  if [[ "$RUN_REAL_CLAUDE" == "true" ]]; then
    : "${BIFROST_CLAUDE_BIN:?set BIFROST_CLAUDE_BIN for real Claude Code verification}"
  fi
  python3 - "$BIFROST_PORT" "${BIFROST_TRAEX_BIN:-}" "${BIFROST_CLAUDE_BIN:-}" "$REPO_DIR" "$RUN_REAL_TRAEX" "$RUN_REAL_CLAUDE" <<'PY'
import json
import sys
import urllib.request

port, traex, claude, repo_dir, run_traex, run_claude = sys.argv[1:7]
endpoint = f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/chat"

def run(adapter, executable, transport, prompt):
    payload = {
        "message": prompt,
        "sessionKey": f"real-subagent-event-boundary-{adapter}",
        "runtime": "external_cli",
        "adapter": adapter,
        "workDir": repo_dir,
        "allowWorkDirs": [repo_dir],
        "injectBifrostTools": False,
        "adapterConfig": {
            "executable": executable,
            "transport": transport,
            "sandbox": "read-only",
            "approvalPolicy": "never",
            "skipGitRepoCheck": True,
            "timeoutSecs": 600,
        },
    }
    request = urllib.request.Request(
        endpoint,
        data=json.dumps(payload).encode("utf-8"),
        headers={"content-type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=660) as response:
        return json.loads(response.read().decode("utf-8"))

summary = {}
if run_traex == "true":
    traex_result = run(
        "traex",
        traex,
        "app_server",
        "You must delegate exactly one read-only task to a sub-agent. Ask it to read design/external-runner-subagent-event-boundary.md and report its first heading. Wait for that child to finish. Only after receiving the child result, reply with ROOT_TRAEX_SUBAGENT_OK.",
    )
    assert traex_result["status"] == "succeeded", traex_result
    assert "ROOT_TRAEX_SUBAGENT_OK" in traex_result["response"], traex_result
    assert sum(event.get("eventType") == "run_finished" for event in traex_result["events"]) == 1, traex_result
    assert not any(event.get("eventType") == "subagent_updated" for event in traex_result["events"]), traex_result
    traex_collaboration = [
        event for event in traex_result["events"]
        if event.get("raw", {}).get("item", {}).get("type")
        in ("collabAgentToolCall", "collab_agent_tool_call", "collaboration_tool_call")
    ]
    assert any(event.get("eventType") == "tool_started" for event in traex_collaboration), traex_result
    assert any(event.get("eventType") == "tool_finished" for event in traex_collaboration), traex_result
    summary["traexRunId"] = traex_result["runId"]
    summary["traexResponse"] = traex_result["response"]

if run_claude == "true":
    claude_result = run(
        "claude_code",
        claude,
        "exec",
        "You must use the Agent or Task tool exactly once. Ask the child to read design/external-runner-subagent-event-boundary.md and report its first heading. Wait for the tool result. Only after it returns, reply with ROOT_CLAUDE_SUBAGENT_OK.",
    )
    assert claude_result["status"] == "succeeded", claude_result
    assert "ROOT_CLAUDE_SUBAGENT_OK" in claude_result["response"], claude_result
    assert sum(event.get("eventType") == "run_finished" for event in claude_result["events"]) == 1, claude_result
    assert not any(event.get("eventType") == "subagent_updated" for event in claude_result["events"]), claude_result
    claude_collaboration = [
        event for event in claude_result["events"]
        if event.get("title", "").lower() in ("task", "agent")
    ]
    assert any(event.get("eventType") == "tool_started" for event in claude_collaboration), claude_result
    assert any(event.get("eventType") == "tool_finished" for event in claude_collaboration), claude_result
    summary["claudeRunId"] = claude_result["runId"]
    summary["claudeResponse"] = claude_result["response"]

with urllib.request.urlopen(
    f"http://127.0.0.1:{port}/_bifrost/api/proxy/address", timeout=30
) as response:
    assert response.status == 200, response.read().decode("utf-8")

print(json.dumps(summary, ensure_ascii=False))
PY
fi

echo "[im-gateway-subagent-event-boundary] PASS"
