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
STREAM_LOG="$TEST_DIR/delayed-final-stream.ndjson"
MID_SESSIONS_JSON="$TEST_DIR/mid-sessions.json"
FINAL_SESSIONS_JSON="$TEST_DIR/final-sessions.json"
BIFROST_BIN="${BIFROST_BIN:-}"
SESSION_KEY="traex-delayed-final-e2e-$(date +%s)"
RUNNER_ID="traex-delayed-final"
FINAL_MARKER="BIFROST_DELAYED_FINAL_OK"

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
  if [[ -n "${STREAM_PID:-}" ]]; then
    kill "$STREAM_PID" >/dev/null 2>&1 || true
    wait "$STREAM_PID" >/dev/null 2>&1 || true
  fi
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
      echo "[im-gateway-delayed-final-state] $label exited before becoming ready" >&2
      [[ -f "$BIFROST_LOG" ]] && tail -160 "$BIFROST_LOG" >&2 || true
      return 1
    fi
    if curl -fsS --noproxy '*' "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.25
  done
  echo "[im-gateway-delayed-final-state] $label did not become ready" >&2
  [[ -f "$BIFROST_LOG" ]] && tail -160 "$BIFROST_LOG" >&2 || true
  return 1
}

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
  echo "[im-gateway-delayed-final-state] skipping build, using $BIFROST_BIN"
else
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
  echo "[im-gateway-delayed-final-state] building bifrost"
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

echo "[im-gateway-delayed-final-state] starting bifrost on $BIFROST_PORT"
BIFROST_DATA_DIR="$TEST_DIR" "$BIFROST_BIN" start \
  --host 127.0.0.1 \
  -p "$BIFROST_PORT" \
  --unsafe-ssl \
  --skip-cert-check \
  --no-system-proxy \
  >"$BIFROST_LOG" 2>&1 &
BIFROST_PID=$!
wait_http "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address" "bifrost"

python3 - "$BIFROST_PORT" "$RUNNER_ID" "$FINAL_MARKER" "$REPO_DIR" <<'PY'
import json
import sys
import urllib.request

port, runner_id, final_marker, repo_dir = sys.argv[1:5]
script = (
    "cat >/dev/null; "
    "printf '%s\n' "
    "'{\"type\":\"thread.started\",\"thread_id\":\"thread-delayed-final\"}' "
    "'{\"type\":\"turn.started\"}' "
    "'{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}'; "
    "sleep 5; "
    "printf '%s\n' "
    f"'{{\"type\":\"item.completed\",\"item\":{{\"id\":\"item_final\",\"type\":\"agent_message\",\"text\":\"{final_marker}\"}}}}'"
)
payload = {
    "version": 1,
    "defaultRunnerId": runner_id,
    "runners": {
        runner_id: {
            "enabled": True,
            "adapter": "mock",
            "adapterConfig": {
                "executable": "/bin/sh",
                "args": ["-c", script],
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
    "runner": runner_id,
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

python3 - "$BIFROST_PORT" "$SESSION_KEY" "$RUNNER_ID" "$STREAM_LOG" <<'PY' &
import json
import sys
import urllib.request

port, session_key, runner_id, stream_log = sys.argv[1:5]
payload = {
    "message": "delayed final verification",
    "sessionKey": session_key,
    "runnerId": runner_id,
    "runtime": "external_cli",
}
req = urllib.request.Request(
    f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/chat/stream",
    data=json.dumps(payload).encode("utf-8"),
    headers={"content-type": "application/json"},
    method="POST",
)
with urllib.request.urlopen(req, timeout=120) as resp, open(stream_log, "w", encoding="utf-8") as handle:
    for raw_line in resp:
        line = raw_line.decode("utf-8").strip()
        if not line:
            continue
        json.loads(line)
        handle.write(line + "\n")
        handle.flush()
PY
STREAM_PID=$!

sleep 2
curl -fsS --noproxy '*' \
  "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/agent/sessions/all?limit=80" \
  >"$MID_SESSIONS_JSON"

python3 - "$MID_SESSIONS_JSON" "$SESSION_KEY" <<'PY'
import json
import sys

path, session_key = sys.argv[1:3]
data = json.load(open(path, encoding="utf-8"))
matches = [item for item in data.get("sessions", []) if item.get("session_key") == session_key]
assert len(matches) == 1, data
item = matches[0]
assert item.get("running") is True, item
assert item.get("status") == "active", item
assert item.get("state") == "running", item
assert item.get("run_state") == "running", item
assert item.get("runner_id") == "traex-delayed-final", item
assert item.get("timeline_event_count", 0) >= 3, item
PY

wait "$STREAM_PID"
STREAM_PID=""

python3 - "$STREAM_LOG" "$FINAL_MARKER" <<'PY'
import json
import sys

stream_log, final_marker = sys.argv[1:3]
events = [json.loads(line) for line in open(stream_log, encoding="utf-8") if line.strip()]
assert events, "stream should emit events"
finished = [event for event in events if event.get("eventType") == "run_finished"]
assert len(finished) == 1, events
assert finished[0].get("status") == "succeeded", finished[0]
assert final_marker in (finished[0].get("response") or ""), finished[0]
assert any(event.get("eventType") == "assistant_final" and final_marker in (event.get("content") or "") for event in events), events
PY

curl -fsS --noproxy '*' \
  "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/agent/sessions/all?limit=80" \
  >"$FINAL_SESSIONS_JSON"

python3 - "$FINAL_SESSIONS_JSON" "$SESSION_KEY" "$FINAL_MARKER" "$TEST_DIR" <<'PY'
import hashlib
import json
import os
import sys

sessions_path, session_key, final_marker, test_dir = sys.argv[1:5]
data = json.load(open(sessions_path, encoding="utf-8"))
matches = [item for item in data.get("sessions", []) if item.get("session_key") == session_key]
assert len(matches) == 1, data
item = matches[0]
assert item.get("running") is False, item
assert item.get("status") == "ended", item
assert item.get("state") == "ended", item
assert item.get("run_state") == "completed", item

digest = hashlib.sha256(session_key.encode("utf-8")).hexdigest()
session_paths = [
    os.path.join(test_dir, "agent", "sessions", "by-key", f"session-{digest}.jsonl")
]
assert os.path.isfile(session_paths[0]), "canonical session timeline should be persisted"
events = []
for path in session_paths:
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            if line.strip():
                events.append(json.loads(line))

completed_indexes = [
    index
    for index, event in enumerate(events)
    if event.get("event_type") == "run_state_changed"
    and ((event.get("content") or {}).get("run_state") or (event.get("content") or {}).get("state"))
    == "completed"
]
assistant_indexes = [
    index
    for index, event in enumerate(events)
    if event.get("event_type") == "assistant_message"
    and final_marker in ((event.get("content") or {}).get("message") or "")
]
assert completed_indexes, events
assert assistant_indexes, events
assert assistant_indexes[-1] < completed_indexes[-1], events
assert not any(index < assistant_indexes[-1] for index in completed_indexes), events
PY

echo "[im-gateway-delayed-final-state] PASS"
