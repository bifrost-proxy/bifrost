#!/usr/bin/env bash
set -euo pipefail
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PORT="${BIFROST_ASR_TASK_RECOVERY_E2E_PORT:-18883}"
DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-asr-task-recovery.XXXXXX")"
AUDIO_DIR="$DATA_DIR/audio"
LOG_FILE="$DATA_DIR/bifrost.log"
PID=""

resolve_bifrost_bin() {
  local candidate="${BIFROST_BIN:-}"
  if [[ -z "$candidate" ]]; then
    if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
      candidate="$ROOT_DIR/target/release/bifrost"
    else
      candidate="$ROOT_DIR/target/debug/bifrost"
    fi
  fi
  if [[ ! -x "$candidate" ]]; then
    echo "bifrost binary is not executable: $candidate" >&2
    return 1
  fi
  printf '%s\n' "$candidate"
}

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

mkdir -p "$AUDIO_DIR" "$DATA_DIR/asr/tasks/stale-paused-task" "$DATA_DIR/asr/tasks/retryable-active-task"
printf 'not real audio\n' > "$AUDIO_DIR/stale.wav"
printf 'not real audio\n' > "$AUDIO_DIR/retryable.wav"

python3 - "$DATA_DIR" "$AUDIO_DIR" <<'PY'
import json
import pathlib
import sys

data_dir = pathlib.Path(sys.argv[1])
audio_dir = pathlib.Path(sys.argv[2])
paused_task_id = "stale-paused-task"
retryable_task_id = "retryable-active-task"
now = 1779345000000

def source_key(path: pathlib.Path) -> str:
    import hashlib

    stat = path.stat()
    hasher = hashlib.sha1()
    hasher.update(str(path.resolve()).encode())
    hasher.update(int(stat.st_size).to_bytes(8, "little", signed=False))
    hasher.update(int(stat.st_mtime * 1000).to_bytes(8, "little", signed=False))
    return hasher.hexdigest()

def task_payload(task_id: str, name: str, paused: bool, audio_name: str):
    return {
        "id": task_id,
        "name": name,
        "audio_dir": str(audio_dir),
        "recursive": True,
        "enabled": True,
        "paused": paused,
        "paused_at_ms": now if paused else None,
        "schedule": {"kind": "daily", "hour": 2, "minute": 0},
        "language": "chinese",
        "model": "Qwen3-ASR-1.7B",
        "runtime_strategy": "fork_per_chunk",
        "created_at_ms": now,
        "updated_at_ms": now,
        "last_run_at_ms": None,
        "next_run_at_ms": None if paused else now,
        "last_error": None,
        "daily_agent": {"enabled": False},
        "external_devices": [],
        "import_policy": {"enabled": False},
    }

(data_dir / "asr").mkdir(parents=True, exist_ok=True)
(data_dir / "asr" / "tasks.json").write_text(json.dumps({
    "version": 1,
    "tasks": [
        task_payload(paused_task_id, "Stale Paused Task", True, "stale.wav"),
        task_payload(retryable_task_id, "Retryable Active Task", False, "retryable.wav"),
    ],
}, indent=2), encoding="utf-8")

task_dir = data_dir / "asr" / "tasks" / paused_task_id
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
            "task_id": paused_task_id,
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

retryable_audio = audio_dir / "retryable.wav"
retryable_dir = data_dir / "asr" / "tasks" / retryable_task_id
retryable_dir.mkdir(parents=True, exist_ok=True)
(retryable_dir / "files.json").write_text(json.dumps({
    "version": 1,
    "files": {
        source_key(retryable_audio): {
            "task_id": retryable_task_id,
            "source_path": str(retryable_audio),
            "source_size": retryable_audio.stat().st_size,
            "source_modified_ms": int(retryable_audio.stat().st_mtime * 1000),
            "source_created_at_ms": None,
            "source_created_at_source": None,
            "media_duration_ms": None,
            "status": "failed",
            "output_text_path": None,
            "output_metadata_path": None,
            "output_timeline_path": None,
            "text_chars": 0,
            "error": "managed ASR server start failed: Qwen3-ASR service is busy.; detail=requested owner=directory_task:retryable-active-task model=Qwen3-ASR-1.7B; active owner=directory_task:retryable-active-task model=Qwen3-ASR-1.7B server=http://127.0.0.1:60241",
            "runtime_strategy": "fork_per_chunk",
            "chunk_metrics": [],
            "started_at_ms": 123,
            "finished_at_ms": 456,
            "progress_current": 1,
            "progress_total": 1,
            "failed_chunks": [],
            "memory_limit_hints": [],
        }
    }
}, indent=2), encoding="utf-8")
PY

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
  echo "[asr-task-startup-recovery] skipping build, using ${BIFROST_BIN:-$ROOT_DIR/target/release/bifrost}"
else
  echo "[asr-task-startup-recovery] build bifrost"
  cargo build --bin bifrost >/dev/null
fi
BIFROST_BIN="$(resolve_bifrost_bin)"

echo "[asr-task-startup-recovery] start bifrost on ${PORT}"
BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" start \
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

echo "[asr-task-startup-recovery] inspect retryable failed task recovery"
RETRYABLE_JSON="$DATA_DIR/retryable-detail.json"
for _ in {1..40}; do
  curl -fsS "http://127.0.0.1:${PORT}/_bifrost/api/asr/tasks/retryable-active-task" >"$RETRYABLE_JSON"
  if python3 - "$RETRYABLE_JSON" <<'PY'
import json
import sys

detail = json.load(open(sys.argv[1], encoding="utf-8"))
files = detail.get("files") or []
record = (files[0].get("record") or files[0]) if files else {}
error = record.get("error") or ""
if "Qwen3-ASR service is busy" in error:
    raise SystemExit(1)
if detail["summary"]["running"] or record.get("status") in {"pending", "processing", "failed"}:
    raise SystemExit(0)
raise SystemExit(1)
PY
  then
    break
  fi
  sleep 0.5
done

python3 - "$RETRYABLE_JSON" <<'PY'
import json
import sys

detail = json.load(open(sys.argv[1], encoding="utf-8"))
files = detail.get("files") or []
assert files, detail
record = files[0].get("record") or files[0]
assert "Qwen3-ASR service is busy" not in (record.get("error") or ""), record
assert record["status"] in {"pending", "processing", "failed"}, record
PY

echo "[asr-task-startup-recovery] PASS"
