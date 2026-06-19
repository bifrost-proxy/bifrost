#!/usr/bin/env bash

set -euo pipefail
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

fail() {
  echo "[asr-task-concurrency-e2e] error: $*" >&2
  exit 1
}

ADMIN_PORT="${BIFROST_ASR_TASK_CONCURRENCY_E2E_PORT:-${ADMIN_PORT:-18994}}"
ADMIN_DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-asr-task-concurrency.XXXXXX")"
AUDIO_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-asr-task-concurrency-audio.XXXXXX")"
ADMIN_PID=""

cleanup() {
  if [[ -n "$ADMIN_PID" ]]; then
    kill "$ADMIN_PID" >/dev/null 2>&1 || true
    wait "$ADMIN_PID" >/dev/null 2>&1 || true
  fi
  rm -rf "$ADMIN_DATA_DIR" "$AUDIO_DIR"
}
trap cleanup EXIT

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
  BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/debug/bifrost}"
  echo "[asr-task-concurrency-e2e] skipping build, using $BIFROST_BIN"
else
  BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/debug/bifrost}"
  echo "[asr-task-concurrency-e2e] build current bifrost binary"
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

if [[ ! -x "$BIFROST_BIN" ]]; then
  fail "Bifrost binary not executable: $BIFROST_BIN"
fi

echo "[asr-task-concurrency-e2e] start temporary Bifrost on ${ADMIN_PORT}"
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

TASK_JSON="$(python3 - "$AUDIO_DIR" <<'PY'
import json
import sys
print(json.dumps({
    "name": "ASR concurrency E2E",
    "audio_dir": sys.argv[1],
    "recursive": False,
    "enabled": False,
    "schedule": {"kind": "daily", "hour": 2, "minute": 0},
    "language": "chinese",
    "model": "Qwen3-ASR-0.6B",
    "runtime_strategy": "fork_per_chunk",
    "max_concurrent_files": 3,
    "diarization": {"enabled": False, "profile": "sherpa-onnx-balanced"},
}))
PY
)"

echo "[asr-task-concurrency-e2e] create fork_per_chunk task with concurrency=3"
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
  >"$ADMIN_DATA_DIR/task-detail-3.json"
python3 - "$ADMIN_DATA_DIR/task-detail-3.json" <<'PY'
import json
import sys
data = json.load(open(sys.argv[1], encoding="utf-8"))
assert data["max_concurrent_files"] == 3, data
assert data["summary"]["max_concurrent_files"] == 3, data["summary"]
assert data["summary"]["effective_max_concurrent_files"] == 3, data["summary"]
PY

echo "[asr-task-concurrency-e2e] patch concurrency while task config is otherwise unchanged"
curl -fsS -X PATCH "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/tasks/${TASK_ID}" \
  -H 'Content-Type: application/json' \
  --data '{"max_concurrent_files":2}' >"$ADMIN_DATA_DIR/task-patch-2.json"
python3 - "$ADMIN_DATA_DIR/task-patch-2.json" <<'PY'
import json
import sys
data = json.load(open(sys.argv[1], encoding="utf-8"))
assert data["max_concurrent_files"] == 2, data
assert data["summary"]["effective_max_concurrent_files"] == 2, data["summary"]
PY

echo "[asr-task-concurrency-e2e] shared runtime keeps desired value but effective concurrency is 1"
curl -fsS -X PATCH "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/tasks/${TASK_ID}" \
  -H 'Content-Type: application/json' \
  --data '{"runtime_strategy":"reuse_per_file","max_concurrent_files":4}' \
  >"$ADMIN_DATA_DIR/task-patch-reuse.json"
python3 - "$ADMIN_DATA_DIR/task-patch-reuse.json" <<'PY'
import json
import sys
data = json.load(open(sys.argv[1], encoding="utf-8"))
assert data["runtime_strategy"] == "reuse_per_file", data
assert data["max_concurrent_files"] == 4, data
assert data["summary"]["effective_max_concurrent_files"] == 1, data["summary"]
PY

echo "[asr-task-concurrency-e2e] diarization keeps fork_per_chunk file concurrency effective"
curl -fsS -X PATCH "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/tasks/${TASK_ID}" \
  -H 'Content-Type: application/json' \
  --data '{"runtime_strategy":"fork_per_chunk","max_concurrent_files":3,"diarization":{"enabled":true,"profile":"sherpa-onnx-balanced"}}' \
  >"$ADMIN_DATA_DIR/task-patch-diarization.json"
python3 - "$ADMIN_DATA_DIR/task-patch-diarization.json" <<'PY'
import json
import sys
data = json.load(open(sys.argv[1], encoding="utf-8"))
assert data["runtime_strategy"] == "fork_per_chunk", data
assert data["diarization"]["enabled"] is True, data["diarization"]
assert data["max_concurrent_files"] == 3, data
assert data["summary"]["effective_max_concurrent_files"] == 3, data["summary"]
PY

echo "[asr-task-concurrency-e2e] PASS"
