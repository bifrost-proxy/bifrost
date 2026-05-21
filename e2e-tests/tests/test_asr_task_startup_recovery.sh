#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PORT="${BIFROST_ASR_TASK_RECOVERY_E2E_PORT:-18883}"
DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-asr-task-recovery.XXXXXX")"
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
  echo "[asr-task-startup-recovery] error: $*" >&2
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

mkdir -p "$AUDIO_DIR" "$DATA_DIR/asr/tasks/stale-paused-task"
printf 'not real audio\n' > "$AUDIO_DIR/stale.wav"

python3 - "$DATA_DIR" "$AUDIO_DIR" <<'PY'
import json
import pathlib
import sys

data_dir = pathlib.Path(sys.argv[1])
audio_dir = pathlib.Path(sys.argv[2])
task_id = "stale-paused-task"
now = 1779345000000

(data_dir / "asr").mkdir(parents=True, exist_ok=True)
(data_dir / "asr" / "tasks.json").write_text(json.dumps({
    "version": 1,
    "tasks": [{
        "id": task_id,
        "name": "Stale Paused Task",
        "audio_dir": str(audio_dir),
        "recursive": True,
        "enabled": True,
        "paused": True,
        "paused_at_ms": now,
        "schedule": {"kind": "daily", "hour": 2, "minute": 0},
        "language": "chinese",
        "model": "Qwen3-ASR-1.7B",
        "runtime_strategy": "fork_per_chunk",
        "created_at_ms": now,
        "updated_at_ms": now,
        "last_run_at_ms": None,
        "next_run_at_ms": None,
        "last_error": None,
        "daily_agent": {"enabled": False},
        "external_devices": [],
        "import_policy": {"enabled": False},
    }],
}, indent=2), encoding="utf-8")

task_dir = data_dir / "asr" / "tasks" / task_id
task_dir.mkdir(parents=True, exist_ok=True)
(task_dir / "run.lock").write_text(json.dumps({
    "pid": 4294967295,
    "process_start_time": 1,
    "acquired_at_ms": 1,
}, indent=2), encoding="utf-8")
(task_dir / "files.json").write_text(json.dumps({
    "version": 1,
    "files": {
        "stale-file": {
            "task_id": task_id,
            "source_path": str(audio_dir / "stale.wav"),
            "source_size": None,
            "source_modified_ms": None,
            "source_created_at_ms": None,
            "source_created_at_source": None,
            "media_duration_ms": None,
            "status": "processing",
            "output_text_path": None,
            "output_metadata_path": None,
            "output_timeline_path": None,
            "text_chars": 0,
            "error": "old transient error",
            "runtime_strategy": "fork_per_chunk",
            "chunk_metrics": [],
            "started_at_ms": 123,
            "finished_at_ms": None,
            "progress_current": 3,
            "progress_total": 9,
            "failed_chunks": [],
            "memory_limit_hints": [],
        }
    }
}, indent=2), encoding="utf-8")
PY

echo "[asr-task-startup-recovery] build bifrost"
cargo build --bin bifrost >/dev/null

echo "[asr-task-startup-recovery] start bifrost on ${PORT}"
BIFROST_DATA_DIR="$DATA_DIR" "$ROOT_DIR/target/debug/bifrost" start \
  -p "$PORT" \
  --unsafe-ssl \
  --skip-cert-check \
  --no-system-proxy \
  >"$LOG_FILE" 2>&1 &
PID="$!"
wait_http "http://127.0.0.1:${PORT}/_bifrost/api/proxy/address"

echo "[asr-task-startup-recovery] trigger ASR scheduler startup and inspect task"
DETAIL_JSON="$DATA_DIR/detail.json"
curl -fsS "http://127.0.0.1:${PORT}/_bifrost/api/asr/tasks/stale-paused-task" >"$DETAIL_JSON"

python3 - "$DETAIL_JSON" "$DATA_DIR/asr/tasks/stale-paused-task/run.lock" <<'PY'
import json
import pathlib
import sys

detail = json.load(open(sys.argv[1], encoding="utf-8"))
lock_path = pathlib.Path(sys.argv[2])
assert detail["paused"] is True, detail
assert detail["summary"]["running"] is False, detail["summary"]
files = detail.get("files") or []
assert files, detail
record = files[0].get("record") or files[0]
assert record["status"] == "pending", record
assert record.get("started_at_ms") is None, record
assert record.get("progress_current") is None, record
assert record.get("progress_total") is None, record
assert record.get("error") is None, record
assert not lock_path.exists(), lock_path
PY

echo "[asr-task-startup-recovery] PASS"
