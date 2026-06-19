#!/usr/bin/env bash
set -euo pipefail

: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

PORT="${BIFROST_ASR_TASK_STATE_RECOVERY_E2E_PORT:-18996}"
DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-asr-state-recovery.XXXXXX")"
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
  echo "[asr-task-state-recovery] error: $*" >&2
  if [[ -f "$LOG_FILE" ]]; then
    tail -n 160 "$LOG_FILE" >&2 || true
  fi
  exit 1
}

wait_http() {
  local url="$1"
  for _ in {1..120}; do
    if curl -fsS "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.5
  done
  fail "timed out waiting for $url"
}

mkdir -p "$AUDIO_DIR" "$DATA_DIR/asr/tasks/state-recovery-task" "$DATA_DIR/asr/data/text/state-recovery-task"
printf 'not real audio\n' > "$AUDIO_DIR/meeting.wav"
printf 'Recovered transcript\n' > "$DATA_DIR/asr/data/text/state-recovery-task/meeting.txt"
printf '{"segments":[]}\n' > "$DATA_DIR/asr/data/text/state-recovery-task/meeting.timeline.json"

python3 - "$DATA_DIR" "$AUDIO_DIR" <<'PY'
import hashlib
import json
import pathlib
import sys

data_dir = pathlib.Path(sys.argv[1])
audio_dir = pathlib.Path(sys.argv[2])
task_id = "state-recovery-task"
audio = audio_dir / "meeting.wav"
text_path = data_dir / "asr/data/text/state-recovery-task/meeting.txt"
timeline_path = data_dir / "asr/data/text/state-recovery-task/meeting.timeline.json"
now = 1781856000000

def source_key(path: pathlib.Path) -> str:
    stat = path.stat()
    hasher = hashlib.sha1()
    hasher.update(str(path.resolve()).encode())
    hasher.update(int(stat.st_size).to_bytes(8, "little", signed=False))
    hasher.update(int(stat.st_mtime * 1000).to_bytes(8, "little", signed=False))
    return hasher.hexdigest()

base_task = {
    "id": task_id,
    "name": "State Recovery Task",
    "audio_dir": str(audio_dir),
    "recursive": True,
    "enabled": False,
    "paused": False,
    "paused_at_ms": None,
    "schedule": {"kind": "daily", "hour": 2, "minute": 0},
    "language": "chinese",
    "model": "Qwen3-ASR-0.6B",
    "runtime_strategy": "fork_per_chunk",
    "max_concurrent_files": 4,
    "diarization": {"enabled": True, "profile": "sherpa-onnx-balanced"},
    "created_at_ms": now,
    "updated_at_ms": now,
    "last_run_at_ms": now,
    "next_run_at_ms": None,
    "last_error": None,
    "daily_agent": {"enabled": False},
    "external_devices": [],
    "import_policy": {"enabled": False},
}
(data_dir / "asr").mkdir(parents=True, exist_ok=True)
(data_dir / "asr/tasks.json").write_text(
    json.dumps({"version": 1, "tasks": [base_task]}, indent=2),
    encoding="utf-8",
)

def record(status, *, progress=None, metrics=None, text=False):
    return {
        "task_id": task_id,
        "source_path": str(audio),
        "source_size": audio.stat().st_size,
        "source_modified_ms": int(audio.stat().st_mtime * 1000),
        "source_created_at_ms": None,
        "source_created_at_source": None,
        "media_duration_ms": 6000,
        "status": status,
        "output_text_path": str(text_path) if text else None,
        "output_metadata_path": None,
        "output_timeline_path": str(timeline_path) if text else None,
        "text_chars": 0,
        "error": None,
        "runtime_strategy": "fork_per_chunk",
        "chunk_metrics": metrics or [],
        "started_at_ms": now - 1000 if status == "processing" else None,
        "finished_at_ms": None,
        "progress_current": progress[0] if progress else None,
        "progress_total": progress[1] if progress else None,
        "failed_chunks": [],
        "memory_limit_hints": [],
    }

metrics = [
    {
        "chunk_index": 0,
        "offset_secs": 0,
        "duration_secs": 3,
        "runner": "fork_per_chunk",
        "status": "ok",
        "elapsed_ms": 100,
        "rtf": 0.1,
        "text_chars": 8,
        "text_sha1": "a",
        "recorded_at_ms": now - 500,
    },
    {
        "chunk_index": 1,
        "offset_secs": 3,
        "duration_secs": 3,
        "runner": "fork_per_chunk",
        "status": "ok",
        "elapsed_ms": 120,
        "rtf": 0.1,
        "text_chars": 12,
        "text_sha1": "b",
        "recorded_at_ms": now - 250,
    },
]
files = {
    source_key(audio): record("processing", progress=(2, 2), metrics=metrics, text=True),
    "old-pending-key": record("pending"),
    "old-processing-key": record("processing", progress=(9, 9)),
}
(data_dir / f"asr/tasks/{task_id}/files.json").write_text(
    json.dumps({"version": 1, "files": files}, indent=2),
    encoding="utf-8",
)
PY

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
  BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/debug/bifrost}"
  echo "[asr-task-state-recovery] skipping build, using $BIFROST_BIN"
else
  BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/debug/bifrost}"
  echo "[asr-task-state-recovery] build current bifrost binary"
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost >/dev/null
fi

[[ -x "$BIFROST_BIN" ]] || fail "bifrost binary is not executable: $BIFROST_BIN"

echo "[asr-task-state-recovery] start bifrost on ${PORT}"
BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" start \
  -p "$PORT" \
  --unsafe-ssl \
  --skip-cert-check \
  --no-system-proxy \
  >"$LOG_FILE" 2>&1 &
PID="$!"
wait_http "http://127.0.0.1:${PORT}/_bifrost/api/system/overview"

DETAIL_JSON="$DATA_DIR/detail.json"
curl -fsS "http://127.0.0.1:${PORT}/_bifrost/api/asr/tasks/state-recovery-task" >"$DETAIL_JSON"

python3 - "$DETAIL_JSON" <<'PY'
import json
import sys

detail = json.load(open(sys.argv[1], encoding="utf-8"))
summary = detail["summary"]
assert summary["processed"] == 1, summary
assert summary["pending"] == 0, summary
assert summary["failed"] == 0, summary
assert summary["diarization_running"] is False, summary
files = detail.get("files") or []
assert len(files) == 1, files
record = files[0].get("record") or files[0]
assert record["status"] == "success", record
assert record["progress_current"] == 2, record
assert record["progress_total"] == 2, record
assert record["finished_at_ms"] is not None, record
assert record["text_chars"] == len("Recovered transcript\n"), record
PY

echo "[asr-task-state-recovery] PASS"
