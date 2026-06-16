#!/usr/bin/env bash
set -euo pipefail

: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_DIR"

if [[ "${RUN_REAL_TRAEX_E2E:-}" != "1" ]]; then
  echo "[im-gateway-traex-runner-streaming] SKIP: set RUN_REAL_TRAEX_E2E=1 to run the real Trae CLI E2E"
  exit 0
fi

TRAEX_BIN="${BIFROST_TRAEX_BIN:-$(command -v traex || true)}"
if [[ -z "$TRAEX_BIN" || ! -x "$TRAEX_BIN" ]]; then
  echo "[im-gateway-traex-runner-streaming] SKIP: traex executable not found"
  exit 0
fi

TEST_DIR="$(mktemp -d)"
BIFROST_LOG="$TEST_DIR/bifrost.log"
STREAM_LOG="$TEST_DIR/traex-stream.ndjson"
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
      echo "[im-gateway-traex-runner-streaming] $label exited before becoming ready" >&2
      [[ -f "$BIFROST_LOG" ]] && tail -160 "$BIFROST_LOG" >&2 || true
      return 1
    fi
    if curl -fsS --noproxy '*' "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.25
  done
  echo "[im-gateway-traex-runner-streaming] $label did not become ready" >&2
  [[ -f "$BIFROST_LOG" ]] && tail -160 "$BIFROST_LOG" >&2 || true
  return 1
}

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
  echo "[im-gateway-traex-runner-streaming] skipping build, using $BIFROST_BIN"
else
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
  echo "[im-gateway-traex-runner-streaming] building bifrost"
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

echo "[im-gateway-traex-runner-streaming] starting bifrost on $BIFROST_PORT"
BIFROST_DATA_DIR="$TEST_DIR" "$BIFROST_BIN" start \
  --host 127.0.0.1 \
  -p "$BIFROST_PORT" \
  --unsafe-ssl \
  --skip-cert-check \
  --no-system-proxy \
  >"$BIFROST_LOG" 2>&1 &
BIFROST_PID=$!
wait_http "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address" "bifrost"

python3 - "$BIFROST_PORT" "$TRAEX_BIN" "$REPO_DIR" <<'PY'
import json
import sys
import urllib.request

port, traex_bin, repo_dir = sys.argv[1:4]
payload = {
    "version": 1,
    "defaultRunnerId": "traex",
    "runners": {
        "traex": {
            "enabled": True,
            "adapter": "traex",
            "adapterConfig": {
                "executable": traex_bin,
                "sandbox": "read-only",
                "skipGitRepoCheck": True,
                "permissionMode": "default",
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
    "runner": "traex",
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
import urllib.request

port, stream_log = sys.argv[1:3]
payload = {
    "message": "Reply exactly: BIFROST_TRAEX_E2E_STREAM_OK. Do not run tools.",
    "sessionKey": "traex-e2e-streaming",
    "runnerId": "traex",
    "runtime": "external_cli",
}
req = urllib.request.Request(
    f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/chat/stream",
    data=json.dumps(payload).encode("utf-8"),
    headers={"content-type": "application/json"},
    method="POST",
)
events = []
with urllib.request.urlopen(req, timeout=240) as resp:
    for raw_line in resp:
        line = raw_line.decode("utf-8").strip()
        if not line:
            continue
        events.append(json.loads(line))

with open(stream_log, "w", encoding="utf-8") as handle:
    for event in events:
        handle.write(json.dumps(event, ensure_ascii=False) + "\n")

assert events, "stream should emit events"
finished = [event for event in events if event.get("eventType") == "run_finished"]
assert len(finished) == 1, events
assert finished[0].get("status") == "succeeded", finished[0]
assert "BIFROST_TRAEX_E2E_STREAM_OK" in (finished[0].get("response") or ""), finished[0]
assert any(event.get("eventType") not in {"run_finished"} for event in events), events
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

snapshot = detail.get("snapshot") or {}
assert snapshot.get("adapter") == "traex", detail
assert snapshot.get("timeoutSecs") in (None, 0), snapshot
args = snapshot.get("args") or []
joined = " ".join(args)
assert "--permission-mode default" not in joined, args
assert not (
    "--permission-mode" in args
    and "--dangerously-bypass-approvals-and-sandbox" in args
), args
assert "--cd" in args and "exec" in args and "--json" in args and "--output-last-message" in args, args
assert detail.get("events"), detail
metadata = detail.get("metadata") or {}
assert metadata.get("modelSource") == "trae default", metadata
assert metadata.get("modelLabel") == "Trae default model (not explicitly configured)", metadata
for key in ("usageInputTokens", "usageOutputTokens", "usageTotalTokens"):
    value = metadata.get(key)
    assert value and int(value) > 0, metadata

session_paths = glob.glob(
    os.path.join(test_dir, "agent", "sessions", "**", "session-traex-e2e-streaming-*.jsonl"),
    recursive=True,
)
assert session_paths, "session timeline should be persisted"
timeline = "\n".join(open(path, encoding="utf-8").read() for path in session_paths)
assert '"adapter":"traex"' in timeline or '"adapter": "traex"' in timeline, timeline
assert '"runner_id":"traex"' in timeline or '"runner_id": "traex"' in timeline, timeline
assert '"tool_name":"traex"' not in timeline and '"tool_name": "traex"' not in timeline, timeline
assert "BIFROST_TRAEX_E2E_STREAM_OK" in timeline, timeline
PY

echo "[im-gateway-traex-runner-streaming] PASS"
