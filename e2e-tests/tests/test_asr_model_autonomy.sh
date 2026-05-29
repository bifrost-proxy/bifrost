#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

fail() {
  echo "[asr-model-autonomy-e2e] error: $*" >&2
  exit 1
}

ADMIN_PORT="${BIFROST_ASR_MODEL_AUTONOMY_E2E_PORT:-${ADMIN_PORT:-18991}}"
ADMIN_DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-asr-model-autonomy.XXXXXX")"
AUDIO_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-asr-model-autonomy-audio.XXXXXX")"
ADMIN_PID=""
BUSY_PID=""

cleanup() {
  if [[ -n "$ADMIN_PID" ]]; then
    kill "$ADMIN_PID" >/dev/null 2>&1 || true
    wait "$ADMIN_PID" >/dev/null 2>&1 || true
  fi
  if [[ -n "$BUSY_PID" ]]; then
    kill "$BUSY_PID" >/dev/null 2>&1 || true
    wait "$BUSY_PID" >/dev/null 2>&1 || true
  fi
  rm -rf "$ADMIN_DATA_DIR" "$AUDIO_DIR"
}
trap cleanup EXIT

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
  BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/release/bifrost}"
  if [[ ! -x "$BIFROST_BIN" && -x "$ROOT_DIR/target/debug/bifrost" ]]; then
    BIFROST_BIN="$ROOT_DIR/target/debug/bifrost"
  fi
  echo "[asr-model-autonomy-e2e] skipping build, using $BIFROST_BIN"
else
  BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/debug/bifrost}"
  echo "[asr-model-autonomy-e2e] build current bifrost binary"
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

if [[ ! -x "$BIFROST_BIN" ]]; then
  fail "Bifrost binary not executable: $BIFROST_BIN"
fi

echo "[asr-model-autonomy-e2e] CLI help exposes explicit model options"
"$BIFROST_BIN" ai asr stream-file --help | grep -q -- "--model"
"$BIFROST_BIN" ai voice listen --help | grep -q -- "--model"

mkdir -p "$ADMIN_DATA_DIR/asr"
cat >"$ADMIN_DATA_DIR/asr/service.json" <<'JSON'
{
  "host": "127.0.0.1",
  "port": 19081,
  "model": "Qwen3-ASR-0.6B",
  "language": "chinese",
  "home": "/tmp/bifrost-asr-legacy-home",
  "pid": null,
  "managed_by": "webui",
  "started_at_ms": 1
}
JSON
BIFROST_DATA_DIR="$ADMIN_DATA_DIR" "$BIFROST_BIN" ai asr status --json >/tmp/bifrost-asr-autonomy-status.json 2>/tmp/bifrost-asr-autonomy-status.err || true
python3 - "$ADMIN_DATA_DIR/asr/service.json" <<'PY'
import json
import sys
with open(sys.argv[1], "r", encoding="utf-8") as f:
    data = json.load(f)
assert "owner_module" not in data
PY
rm -f "$ADMIN_DATA_DIR/asr/service.json"
rm -f /tmp/bifrost-asr-autonomy-status.json /tmp/bifrost-asr-autonomy-status.err

echo "[asr-model-autonomy-e2e] start temporary Bifrost on ${ADMIN_PORT}"
BIFROST_DATA_DIR="$ADMIN_DATA_DIR" "$BIFROST_BIN" start \
  -p "$ADMIN_PORT" \
  --unsafe-ssl \
  --skip-cert-check \
  --no-system-proxy >"$ADMIN_DATA_DIR/bifrost.log" 2>&1 &
ADMIN_PID=$!

for _ in $(seq 1 120); do
  if curl -fsS "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/system/overview" >/dev/null 2>&1; then
    break
  fi
  sleep 1
done
curl -fsS "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/system/overview" >/dev/null || {
  tail -100 "$ADMIN_DATA_DIR/bifrost.log" >&2 || true
  fail "Bifrost admin did not start"
}

echo "[asr-model-autonomy-e2e] model-management status is owner scoped"
curl -fsS "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/status?model=Qwen3-ASR-0.6B&owner_module=model_management" \
  >"$ADMIN_DATA_DIR/status-model-management.json"
python3 - "$ADMIN_DATA_DIR/status-model-management.json" <<'PY'
import json
import sys
with open(sys.argv[1], "r", encoding="utf-8") as f:
    status = json.load(f)
assert status["model"] == "Qwen3-ASR-0.6B", status
assert status["owner_module"] == "model_management", status
assert "ready" in status and "installed" in status and "managed" in status, status
PY

echo "[asr-model-autonomy-e2e] directory task keeps per-task model"
TASK_JSON="$(python3 - "$AUDIO_DIR" <<'PY'
import json
import sys
print(json.dumps({
    "name": "ASR model autonomy task",
    "audio_dir": sys.argv[1],
    "recursive": True,
    "enabled": False,
    "schedule": {"kind": "daily", "hour": 2, "minute": 0},
    "language": "english",
    "model": "Qwen3-ASR-0.6B",
    "runtime_strategy": "reuse_per_file",
}))
PY
)"
curl -fsS -X POST "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/tasks" \
  -H 'Content-Type: application/json' \
  --data "$TASK_JSON" >"$ADMIN_DATA_DIR/task-create.json"
TASK_ID="$(python3 - "$ADMIN_DATA_DIR/task-create.json" <<'PY'
import json
import sys
with open(sys.argv[1], "r", encoding="utf-8") as f:
    print(json.load(f)["id"])
PY
)"
curl -fsS "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/tasks/${TASK_ID}" \
  >"$ADMIN_DATA_DIR/task-detail.json"
python3 - "$ADMIN_DATA_DIR/task-detail.json" <<'PY'
import json
import sys
with open(sys.argv[1], "r", encoding="utf-8") as f:
    task = json.load(f)
assert task["model"] == "Qwen3-ASR-0.6B", task
assert task["language"] == "english", task
assert task["runtime_strategy"] == "reuse_per_file", task
PY

echo "[asr-model-autonomy-e2e] seed speech_workbench service and verify same-model runtime sharing"
BUSY_PORT="$(python3 - <<'PY'
import socket

with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
    s.bind(("127.0.0.1", 0))
    print(s.getsockname()[1])
PY
)"
python3 - "$BUSY_PORT" >"$ADMIN_DATA_DIR/busy-asr-mock.log" 2>&1 <<'PY' &
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import sys

class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/health":
            body = json.dumps({"ok": True}).encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        self.send_response(404)
        self.end_headers()

    def log_message(self, fmt, *args):
        return

ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), Handler).serve_forever()
PY
BUSY_PID=$!
for _ in $(seq 1 30); do
  if curl -fsS "http://127.0.0.1:${BUSY_PORT}/health" >/dev/null 2>&1; then
    break
  fi
  sleep 1
done
curl -fsS "http://127.0.0.1:${BUSY_PORT}/health" >/dev/null || {
  cat "$ADMIN_DATA_DIR/busy-asr-mock.log" >&2 || true
  fail "busy ASR mock did not start"
}
cat >"$ADMIN_DATA_DIR/asr/service.json" <<JSON
{
  "host": "127.0.0.1",
  "port": ${BUSY_PORT},
  "model": "Qwen3-ASR-1.7B",
  "language": "chinese",
  "home": "${HOME}/.bifrost/asr",
  "pid": null,
  "managed_by": "speech_workbench",
  "owner_module": "speech_workbench",
  "owner_id": null,
  "started_at_ms": 1
}
JSON
curl -fsS "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/status?model=Qwen3-ASR-1.7B&owner_module=directory_task&owner_id=${TASK_ID}" \
  >"$ADMIN_DATA_DIR/status-directory-owner.json"
python3 - "$ADMIN_DATA_DIR/status-directory-owner.json" <<'PY'
import json
import sys
with open(sys.argv[1], "r", encoding="utf-8") as f:
    status = json.load(f)
assert status["owner_module"] == "directory_task", status
assert status.get("owner_id"), status
assert status["model"] == "Qwen3-ASR-1.7B", status
PY
set +e
SHARED_BODY="$(curl -sS -w '\n%{http_code}' -X POST "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/service/start?model=Qwen3-ASR-1.7B&owner_module=directory_task&owner_id=${TASK_ID}")"
SHARED_EXIT=$?
set -e
if [[ "$SHARED_EXIT" -ne 0 ]]; then
  fail "shared runtime request failed"
fi
SHARED_STATUS="${SHARED_BODY##*$'\n'}"
SHARED_JSON="${SHARED_BODY%$'\n'*}"
python3 - "$SHARED_STATUS" "$SHARED_JSON" "$BUSY_PORT" <<'PY'
import json
import platform
import sys
status = sys.argv[1]
body = json.loads(sys.argv[2])
busy_port = sys.argv[3]
supported = platform.system().lower() == "darwin" and platform.machine().lower() in {
    "arm64",
    "aarch64",
}
if not supported:
    assert status in {"200", "400"}, (status, body)
    assert body["ready"] is False, body
    assert body["managed"] is False, body
    message = body.get("message", "").lower()
    detail = body.get("detail", "").lower()
    assert "not supported" in message or "not supported" in detail, body
    sys.exit(0)
assert status == "200", (status, body)
assert body["ready"] is True, body
assert body["managed"] is True, body
assert body["server_url"] == f"http://127.0.0.1:{busy_port}", body
message = body.get("message", "")
assert "already running" in message.lower() or "reusing" in message.lower(), body
PY
kill "$BUSY_PID" >/dev/null 2>&1 || true
wait "$BUSY_PID" >/dev/null 2>&1 || true

echo "[asr-model-autonomy-e2e] PASS"
