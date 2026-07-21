#!/usr/bin/env bash
set -euo pipefail

: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_DIR"

if [[ "${RUN_REAL_CODEX_E2E:-}" != "1" ]]; then
  echo "[im-gateway-codex-runner-streaming] SKIP: set RUN_REAL_CODEX_E2E=1 to run the real Codex CLI E2E"
  exit 0
fi

CODEX_BIN="${BIFROST_CODEX_BIN:-$(command -v codex || true)}"
if [[ -z "$CODEX_BIN" || ! -x "$CODEX_BIN" ]]; then
  echo "[im-gateway-codex-runner-streaming] SKIP: codex executable not found"
  exit 0
fi

TEST_DIR="$(mktemp -d)"
BIFROST_LOG="$TEST_DIR/bifrost.log"
STREAM_LOG="$TEST_DIR/codex-stream.ndjson"
BIFROST_BIN="${BIFROST_BIN:-}"

if [[ -z "${BIFROST_PORT:-}" ]]; then
  BIFROST_PORT="$(python3 - <<'PY'
import socket

with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
)"
fi

cleanup() {
  if [[ -n "${BIFROST_PID:-}" ]]; then
    kill "$BIFROST_PID" >/dev/null 2>&1 || true
    wait "$BIFROST_PID" >/dev/null 2>&1 || true
  fi
  rm -rf "$TEST_DIR"
}
trap cleanup EXIT

wait_http() {
  local url="$1"
  local label="$2"
  for _ in $(seq 1 180); do
    if ! kill -0 "$BIFROST_PID" >/dev/null 2>&1; then
      echo "[im-gateway-codex-runner-streaming] $label exited before becoming ready" >&2
      [[ -f "$BIFROST_LOG" ]] && tail -160 "$BIFROST_LOG" >&2 || true
      return 1
    fi
    if curl -fsS --noproxy '*' "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.25
  done
  echo "[im-gateway-codex-runner-streaming] $label did not become ready" >&2
  [[ -f "$BIFROST_LOG" ]] && tail -160 "$BIFROST_LOG" >&2 || true
  return 1
}

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
  echo "[im-gateway-codex-runner-streaming] skipping build, using $BIFROST_BIN"
else
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
  echo "[im-gateway-codex-runner-streaming] building bifrost"
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

echo "[im-gateway-codex-runner-streaming] starting bifrost on $BIFROST_PORT"
BIFROST_DATA_DIR="$TEST_DIR" "$BIFROST_BIN" start \
  --host 127.0.0.1 \
  -p "$BIFROST_PORT" \
  --unsafe-ssl \
  --skip-cert-check \
  --no-system-proxy \
  >"$BIFROST_LOG" 2>&1 &
BIFROST_PID=$!
wait_http "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address" "bifrost"

python3 - "$BIFROST_PORT" "$CODEX_BIN" "$REPO_DIR" <<'PY'
import json
import sys
import urllib.request

port, codex_bin, repo_dir = sys.argv[1:4]
payload = {
    "version": 1,
    "defaultRunnerId": "codex",
    "runners": {
        "codex": {
            "enabled": True,
            "adapter": "codex",
            "adapterConfig": {
                "executable": codex_bin,
                "sandbox": "read-only",
                "approvalPolicy": "never",
                "skipGitRepoCheck": True,
                "timeoutSecs": 240,
            },
            "injectBifrostTools": False,
            "skillPaths": [],
            "deliveryMode": "progress_card",
        }
    },
    "channels": {},
}
req = urllib.request.Request(
    f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/chat/config",
    data=json.dumps(payload).encode("utf-8"),
    headers={"content-type": "application/json"},
    method="PATCH",
)
with urllib.request.urlopen(req, timeout=30) as resp:
    body = resp.read().decode("utf-8")
    assert resp.status == 200, body

agent_payload = {
    "enabled": True,
    "runner": "codex",
    "work_dir": repo_dir,
}
req = urllib.request.Request(
    f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/agent",
    data=json.dumps(agent_payload).encode("utf-8"),
    headers={"content-type": "application/json"},
    method="PATCH",
)
with urllib.request.urlopen(req, timeout=30) as resp:
    body = resp.read().decode("utf-8")
    assert resp.status == 200, body
PY

python3 - "$BIFROST_PORT" "$STREAM_LOG" <<'PY'
import json
import sys
import time
import urllib.request

port, stream_log = sys.argv[1:3]
payload = {
    "message": "You must call the exec_command tool exactly once now with command pwd. Do not claim you ran it without a tool result. After the tool result, reply exactly: BIFROST_CODEX_E2E_STREAM_OK",
    "sessionKey": "codex-e2e-streaming",
    "runnerId": "codex",
    "runtime": "external_cli",
}
req = urllib.request.Request(
    f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/chat/stream",
    data=json.dumps(payload).encode("utf-8"),
    headers={"content-type": "application/json"},
    method="POST",
)
events = []
first_tool_at = None
finished_at = None
with urllib.request.urlopen(req, timeout=300) as resp:
    start = time.monotonic()
    for raw_line in resp:
        line = raw_line.decode("utf-8").strip()
        if not line:
            continue
        event = json.loads(line)
        event["_receivedAtMs"] = int((time.monotonic() - start) * 1000)
        events.append(event)
        if event.get("eventType") == "tool_started" and first_tool_at is None:
            first_tool_at = len(events) - 1
        if event.get("eventType") == "run_finished" and finished_at is None:
            finished_at = len(events) - 1

with open(stream_log, "w", encoding="utf-8") as handle:
    for event in events:
        handle.write(json.dumps(event, ensure_ascii=False) + "\n")

assert events, "stream should emit events"
finished = [event for event in events if event.get("eventType") == "run_finished"]
assert len(finished) == 1, events
assert finished[0].get("status") == "succeeded", finished[0]
assert "BIFROST_CODEX_E2E_STREAM_OK" in (finished[0].get("response") or ""), finished[0]
assert finished_at is not None, events
if first_tool_at is not None:
    assert first_tool_at < finished_at, events
    assert any(event.get("eventType") == "tool_finished" for event in events), events
else:
    # Real models may decline or skip a requested tool call. The deterministic
    # mock app-server E2E owns the unconditional tool ordering assertion.
    assert any(event.get("eventType") == "assistant_delta" for event in events), events
print(finished[0]["runId"])
PY
RUN_ID="$(tail -n 1 "$STREAM_LOG" | python3 -c 'import json,sys; print(json.load(sys.stdin)["runId"])')"

python3 - "$BIFROST_PORT" "$RUN_ID" "$TEST_DIR" <<'PY'
import hashlib
import json
import os
import sys
import urllib.request

port, run_id, test_dir = sys.argv[1:4]
with urllib.request.urlopen(
    f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/chat/runs/{run_id}",
    timeout=30,
) as resp:
    detail = json.loads(resp.read().decode("utf-8"))

snapshot = detail.get("snapshot") or {}
assert snapshot.get("adapter") == "codex", detail
args = snapshot.get("args") or []
assert args[:2] == ["app-server", "--stdio"], args
tool_started = any(event.get("eventType") == "tool_started" for event in detail.get("events") or [])
tool_finished = any(event.get("eventType") == "tool_finished" for event in detail.get("events") or [])
assert tool_started == tool_finished, detail
metadata = detail.get("metadata") or {}
assert metadata.get("modelSource") in ("codex default", "codex config"), metadata
assert metadata.get("modelLabel"), metadata
assert metadata.get("threadId"), metadata
for key in ("usageInputTokens", "usageOutputTokens", "usageTotalTokens"):
    value = metadata.get(key)
    assert value and int(value) > 0, metadata

digest = hashlib.sha256(b"codex-e2e-streaming").hexdigest()
session_paths = [
    os.path.join(test_dir, "agent", "sessions", "by-key", f"session-{digest}.jsonl")
]
assert os.path.isfile(session_paths[0]), "canonical session timeline should be persisted"
timeline = "\n".join(open(path, encoding="utf-8").read() for path in session_paths)
assert '"adapter":"codex"' in timeline or '"adapter": "codex"' in timeline, timeline
assert '"runner_id":"codex"' in timeline or '"runner_id": "codex"' in timeline, timeline
if tool_started:
    assert '"tool_name":"exec_command"' in timeline or '"tool_name": "exec_command"' in timeline, timeline
assert "BIFROST_CODEX_E2E_STREAM_OK" in timeline, timeline
PY

echo "[im-gateway-codex-runner-streaming] PASS"
