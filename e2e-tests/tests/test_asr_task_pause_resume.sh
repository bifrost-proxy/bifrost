#!/usr/bin/env bash
set -euo pipefail
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PORT="${BIFROST_ASR_TASK_PAUSE_E2E_PORT:-18882}"
DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-asr-task-pause.XXXXXX")"
AUDIO_DIR="$DATA_DIR/audio"
LOG_FILE="$DATA_DIR/bifrost.log"
PID=""

cleanup() {
  if [[ -n "$PID" ]] && kill -0 "$PID" 2>/dev/null; then
    kill "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
  fi
  rm -rf "$DATA_DIR"
}
trap cleanup EXIT

fail() {
  echo "[asr-task-pause-resume] error: $*" >&2
  if [[ -f "$LOG_FILE" ]]; then
    tail -n 120 "$LOG_FILE" >&2 || true
  fi
  exit 1
}

wait_http() {
  local url="$1"
  for _ in {1..80}; do
    if curl -fsS "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.5
  done
  fail "timed out waiting for $url"
}

mkdir -p "$AUDIO_DIR"

echo "[asr-task-pause-resume] build bifrost"
cargo build --bin bifrost >/dev/null

echo "[asr-task-pause-resume] start bifrost on ${PORT}"
BIFROST_DATA_DIR="$DATA_DIR" "$ROOT_DIR/target/debug/bifrost" start \
  -p "$PORT" \
  --unsafe-ssl \
  --skip-cert-check \
  --no-system-proxy \
  >"$LOG_FILE" 2>&1 &
PID="$!"
wait_http "http://127.0.0.1:${PORT}/_bifrost/api/proxy/address"

echo "[asr-task-pause-resume] create empty ASR directory task"
CREATE_JSON="$DATA_DIR/create.json"
curl -fsS -X POST "http://127.0.0.1:${PORT}/_bifrost/api/asr/tasks" \
  -H 'Content-Type: application/json' \
  --data "$(
    python3 - "$AUDIO_DIR" <<'PY'
import json, sys
print(json.dumps({
  "name": "Pause Resume E2E",
  "audio_dir": sys.argv[1],
  "recursive": True,
  "enabled": True,
  "schedule": {"kind": "daily", "hour": 2, "minute": 0},
  "language": "chinese",
  "model": "Qwen3-ASR-1.7B",
}))
PY
  )" >"$CREATE_JSON"
TASK_ID="$(python3 - "$CREATE_JSON" <<'PY'
import json, sys
print(json.load(open(sys.argv[1]))["id"])
PY
)"

echo "[asr-task-pause-resume] pause task"
PAUSE_JSON="$DATA_DIR/pause.json"
curl -fsS -X POST "http://127.0.0.1:${PORT}/_bifrost/api/asr/tasks/${TASK_ID}/pause?mode=long_term" >"$PAUSE_JSON"
python3 - "$PAUSE_JSON" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
assert data["paused"] is True, data
assert data["running"] is False, data
assert data["pause_mode"] == "long_term", data
assert data["task"]["paused"] is True, data
assert data["task"]["next_run_at_ms"] is None, data
PY

echo "[asr-task-pause-resume] paused task rejects manual run"
RUN_STATUS="$(
  curl -sS -o "$DATA_DIR/run-paused.json" -w '%{http_code}' \
    -X POST "http://127.0.0.1:${PORT}/_bifrost/api/asr/tasks/${TASK_ID}/run"
)"
[[ "$RUN_STATUS" == "409" ]] || fail "expected paused run HTTP 409, got ${RUN_STATUS}"
grep -q '"paused":true' "$DATA_DIR/run-paused.json"

echo "[asr-task-pause-resume] temporary pause auto-resumes at next schedule"
TEMP_AUDIO_DIR="$DATA_DIR/temp-audio"
mkdir -p "$TEMP_AUDIO_DIR"
TEMP_CREATE_JSON="$DATA_DIR/temp-create.json"
curl -fsS -X POST "http://127.0.0.1:${PORT}/_bifrost/api/asr/tasks" \
  -H 'Content-Type: application/json' \
  --data "$(
    python3 - "$TEMP_AUDIO_DIR" <<'PY'
import json, sys
print(json.dumps({
  "name": "Temporary Pause E2E",
  "audio_dir": sys.argv[1],
  "recursive": True,
  "enabled": True,
  "schedule": {"kind": "daily", "hour": 2, "minute": 0},
  "language": "chinese",
  "model": "Qwen3-ASR-1.7B",
}))
PY
  )" >"$TEMP_CREATE_JSON"
TEMP_TASK_ID="$(python3 - "$TEMP_CREATE_JSON" <<'PY'
import json, sys
print(json.load(open(sys.argv[1]))["id"])
PY
)"
TEMP_PAUSE_JSON="$DATA_DIR/temp-pause.json"
curl -fsS -X POST "http://127.0.0.1:${PORT}/_bifrost/api/asr/tasks/${TEMP_TASK_ID}/pause?mode=temporary" >"$TEMP_PAUSE_JSON"
python3 - "$TEMP_PAUSE_JSON" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
assert data["paused"] is True, data
assert data["pause_mode"] == "temporary", data
assert data["task"]["paused"] is True, data
assert data["task"]["next_run_at_ms"] is not None, data
PY
python3 - "$DATA_DIR/asr/tasks.json" "$TEMP_TASK_ID" <<'PY'
import json, sys
path, task_id = sys.argv[1], sys.argv[2]
data = json.load(open(path))
for task in data["tasks"]:
    if task["id"] == task_id:
        task["next_run_at_ms"] = 1
        break
else:
    raise SystemExit("task not found")
json.dump(data, open(path, "w"), indent=2)
PY
TEMP_LIST_JSON="$DATA_DIR/temp-list.json"
for _ in {1..80}; do
  curl -fsS "http://127.0.0.1:${PORT}/_bifrost/api/asr/tasks" >"$TEMP_LIST_JSON"
  if python3 - "$TEMP_LIST_JSON" "$TEMP_TASK_ID" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
task_id = sys.argv[2]
task = next(t for t in data["tasks"] if t["id"] == task_id)
raise SystemExit(0 if not task["paused"] and not task["summary"]["running"] else 1)
PY
  then
    break
  fi
  sleep 0.25
done
python3 - "$TEMP_LIST_JSON" "$TEMP_TASK_ID" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
task_id = sys.argv[2]
task = next(t for t in data["tasks"] if t["id"] == task_id)
assert task["paused"] is False, task
assert task["summary"]["running"] is False, task
assert task["summary"]["pending"] == 0, task
PY

echo "[asr-task-pause-resume] resume task without pending files"
RESUME_JSON="$DATA_DIR/resume.json"
curl -fsS -X POST "http://127.0.0.1:${PORT}/_bifrost/api/asr/tasks/${TASK_ID}/resume" >"$RESUME_JSON"
python3 - "$RESUME_JSON" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
assert data["paused"] is False, data
assert data["running"] is True, data
assert data["task"]["paused"] is False, data
assert "queued for background processing" in data["message"], data
PY

echo "[asr-task-pause-resume] empty background run quickly releases running state"
LIST_JSON="$DATA_DIR/list.json"
for _ in {1..40}; do
  curl -fsS "http://127.0.0.1:${PORT}/_bifrost/api/asr/tasks" >"$LIST_JSON"
  if python3 - "$LIST_JSON" "$TASK_ID" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
task_id = sys.argv[2]
task = next(t for t in data["tasks"] if t["id"] == task_id)
raise SystemExit(0 if not task["summary"]["running"] else 1)
PY
  then
    break
  fi
  sleep 0.25
done
python3 - "$LIST_JSON" "$TASK_ID" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
task_id = sys.argv[2]
task = next(t for t in data["tasks"] if t["id"] == task_id)
assert task["paused"] is False, task
assert task["summary"]["running"] is False, task
assert task["summary"]["pending"] == 0, task
PY

echo "[asr-task-pause-resume] PASS"
