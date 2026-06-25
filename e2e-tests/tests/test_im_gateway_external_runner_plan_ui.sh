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
STREAM_LOG="$TEST_DIR/external-plan-stream.ndjson"
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
      echo "[im-gateway-external-runner-plan-ui] $label exited before becoming ready" >&2
      [[ -f "$BIFROST_LOG" ]] && tail -160 "$BIFROST_LOG" >&2 || true
      return 1
    fi
    if curl -fsS --noproxy '*' "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.25
  done
  echo "[im-gateway-external-runner-plan-ui] $label did not become ready" >&2
  [[ -f "$BIFROST_LOG" ]] && tail -160 "$BIFROST_LOG" >&2 || true
  return 1
}

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
  echo "[im-gateway-external-runner-plan-ui] skipping build, using $BIFROST_BIN"
else
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
  echo "[im-gateway-external-runner-plan-ui] building bifrost"
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

echo "[im-gateway-external-runner-plan-ui] starting bifrost on $BIFROST_PORT"
BIFROST_DATA_DIR="$TEST_DIR" "$BIFROST_BIN" start \
  --host 127.0.0.1 \
  -p "$BIFROST_PORT" \
  --unsafe-ssl \
  --skip-cert-check \
  --no-system-proxy \
  >"$BIFROST_LOG" 2>&1 &
BIFROST_PID=$!
wait_http "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address" "bifrost"

python3 - "$BIFROST_PORT" <<'PY'
import json
import sys
import urllib.request

port = sys.argv[1]
mock_script = (
    "cat >/dev/null; "
    "printf '%s\\n' "
    "'{\"type\":\"plan_updated\",\"title\":\"Runner plan\",\"items\":[{\"text\":\"inspect output\",\"status\":\"completed\"},{\"text\":\"map parser\",\"status\":\"in_progress\"},{\"text\":\"verify UI\",\"status\":\"pending\"}]}' "
    "'{\"type\":\"assistant_final\",\"content\":\"BIFROST_EXTERNAL_PLAN_UI_OK\"}'"
)
payload = {
    "version": 1,
    "defaultRunnerId": "mock-plan",
    "runners": {
        "mock-plan": {
            "enabled": True,
            "adapter": "custom",
            "adapterConfig": {
                "executable": "sh",
                "args": ["-c", mock_script],
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

agent_payload = {"enabled": True, "runner": "mock-plan"}
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
import urllib.request

port, stream_log = sys.argv[1:3]
payload = {
    "message": "exercise external runner plan UI",
    "sessionKey": "external-plan-ui-e2e",
    "runnerId": "mock-plan",
    "runtime": "external_cli",
}
req = urllib.request.Request(
    f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/chat/stream",
    data=json.dumps(payload).encode("utf-8"),
    headers={"content-type": "application/json"},
    method="POST",
)
events = []
with urllib.request.urlopen(req, timeout=120) as resp:
    for raw_line in resp:
        line = raw_line.decode("utf-8").strip()
        if not line:
            continue
        events.append(json.loads(line))

with open(stream_log, "w", encoding="utf-8") as handle:
    for event in events:
        handle.write(json.dumps(event, ensure_ascii=False) + "\n")

plan_events = [event for event in events if event.get("eventType") == "plan_updated"]
assert len(plan_events) == 1, events
steps = plan_events[0].get("steps") or []
assert [step.get("status") for step in steps] == ["completed", "in_progress", "pending"], plan_events[0]
finished = [event for event in events if event.get("eventType") == "run_finished"]
assert len(finished) == 1, events
assert finished[0].get("status") == "succeeded", finished[0]
assert "BIFROST_EXTERNAL_PLAN_UI_OK" in (finished[0].get("response") or ""), finished[0]
print(finished[0]["runId"])
PY
RUN_ID="$(tail -n 1 "$STREAM_LOG" | python3 -c 'import json,sys; print(json.load(sys.stdin)["runId"])')"

python3 - "$BIFROST_PORT" "$RUN_ID" "$TEST_DIR" <<'PY'
import glob
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

assert any(event.get("eventType") == "plan_updated" for event in detail.get("events") or []), detail
session_paths = glob.glob(
    os.path.join(test_dir, "agent", "sessions", "**", "session-external-plan-ui-e2e-*.jsonl"),
    recursive=True,
)
assert session_paths, "session timeline should be persisted"
timeline = "\n".join(open(path, encoding="utf-8").read() for path in session_paths)
assert '"event_type":"plan_updated"' in timeline or '"event_type": "plan_updated"' in timeline, timeline
assert "inspect output" in timeline and "map parser" in timeline and "verify UI" in timeline, timeline
assert "BIFROST_EXTERNAL_PLAN_UI_OK" in timeline, timeline
PY

echo "[im-gateway-external-runner-plan-ui] PASS"
