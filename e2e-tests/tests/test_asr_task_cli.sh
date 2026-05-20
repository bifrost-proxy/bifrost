#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

fail() {
  echo "[asr-task-cli-e2e] error: $*" >&2
  exit 1
}

ADMIN_PORT="${BIFROST_ASR_TASK_CLI_E2E_PORT:-18990}"
ADMIN_DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-asr-task-cli.XXXXXX")"
AUDIO_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-asr-task-audio.XXXXXX")"
OUTPUT_DOC="$(mktemp "${TMPDIR:-/tmp}/bifrost-asr-task-day.XXXXXX.md")"
ADMIN_PID=""

cleanup() {
  if [[ -n "$ADMIN_PID" ]]; then
    kill "$ADMIN_PID" >/dev/null 2>&1 || true
    wait "$ADMIN_PID" >/dev/null 2>&1 || true
  fi
  rm -rf "$ADMIN_DATA_DIR" "$AUDIO_DIR" "$OUTPUT_DOC"
}
trap cleanup EXIT

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
  BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/release/bifrost}"
  echo "[asr-task-cli-e2e] skipping build, using $BIFROST_BIN"
else
  BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/debug/bifrost}"
  echo "[asr-task-cli-e2e] build current bifrost binary"
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

if [[ ! -x "$BIFROST_BIN" ]]; then
  fail "Bifrost binary not executable: $BIFROST_BIN"
fi

echo "[asr-task-cli-e2e] start temporary Bifrost on ${ADMIN_PORT}"
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
    "name": "ASR CLI E2E task",
    "audio_dir": sys.argv[1],
    "recursive": True,
    "enabled": False,
    "schedule": {"kind": "daily", "hour": 2, "minute": 0},
    "language": "chinese",
    "model": "Qwen3-ASR-1.7B",
}))
PY
)"

echo "[asr-task-cli-e2e] create ASR task through admin API"
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

mkdir -p "$ADMIN_DATA_DIR/asr/data/text/${TASK_ID}/daily"
cat > "$ADMIN_DATA_DIR/asr/data/text/${TASK_ID}/daily/2026-05-17.md" <<'EOF'
# ASR CLI E2E task — 2026-05-17

完整内容整理的文档展示。
EOF

echo "[asr-task-cli-e2e] task list uses runtime port when -p is omitted"
BIFROST_DATA_DIR="$ADMIN_DATA_DIR" "$BIFROST_BIN" ai asr task list >"$ADMIN_DATA_DIR/task-list.out"
grep -q "$TASK_ID" "$ADMIN_DATA_DIR/task-list.out"

echo "[asr-task-cli-e2e] task show exposes daily document count"
BIFROST_DATA_DIR="$ADMIN_DATA_DIR" "$BIFROST_BIN" ai asr task show "$TASK_ID" >"$ADMIN_DATA_DIR/task-show.out"
grep -q "Daily documents: 1" "$ADMIN_DATA_DIR/task-show.out"

echo "[asr-task-cli-e2e] task files handles empty task"
BIFROST_DATA_DIR="$ADMIN_DATA_DIR" "$BIFROST_BIN" ai asr task files "$TASK_ID" >"$ADMIN_DATA_DIR/task-files.out"
grep -q "No ASR task files matched" "$ADMIN_DATA_DIR/task-files.out"

echo "[asr-task-cli-e2e] source audio usage and cleanup keep ASR outputs"
python3 - "$ADMIN_DATA_DIR" "$AUDIO_DIR" "$TASK_ID" <<'PY'
import hashlib
import json
import os
import pathlib
import sys

data_dir = pathlib.Path(sys.argv[1])
audio_dir = pathlib.Path(sys.argv[2])
task_id = sys.argv[3]

done_audio = audio_dir / "done.wav"
partial_audio = audio_dir / "partial.wav"
done_audio.write_bytes(b"done-audio")
partial_audio.write_bytes(b"partial-audio")

text_dir = data_dir / "asr" / "data" / "text" / task_id
text_dir.mkdir(parents=True, exist_ok=True)
done_text = text_dir / "done.txt"
done_timeline = text_dir / "done.timeline.json"
partial_text = text_dir / "partial.txt"
partial_timeline = text_dir / "partial.timeline.json"
done_text.write_text("done transcript", encoding="utf-8")
done_timeline.write_text("{}", encoding="utf-8")
partial_text.write_text("partial transcript", encoding="utf-8")
partial_timeline.write_text("{}", encoding="utf-8")

def source_key(path: pathlib.Path) -> str:
    resolved = os.path.realpath(path)
    stat = path.stat()
    digest = hashlib.sha1()
    digest.update(resolved.encode())
    digest.update(stat.st_size.to_bytes(8, "little"))
    digest.update((stat.st_mtime_ns // 1_000_000).to_bytes(8, "little"))
    return digest.hexdigest()

def record(path: pathlib.Path, status: str, text: pathlib.Path, timeline: pathlib.Path) -> dict:
    stat = path.stat()
    return {
        "task_id": task_id,
        "source_path": str(path),
        "source_size": stat.st_size,
        "source_modified_ms": stat.st_mtime_ns // 1_000_000,
        "source_created_at_ms": None,
        "source_created_at_source": None,
        "media_duration_ms": 1000,
        "status": status,
        "output_text_path": str(text),
        "output_metadata_path": None,
        "output_timeline_path": str(timeline),
        "text_chars": len(text.read_text(encoding="utf-8")),
        "error": None,
        "runtime_strategy": "reuse_per_file",
        "chunk_metrics": [],
        "fallback_reason": None,
        "started_at_ms": 1,
        "finished_at_ms": 2,
        "progress_current": None,
        "progress_total": None,
        "failed_chunks": [],
        "memory_limit_hints": [],
    }

files = {
    source_key(done_audio): record(done_audio, "success", done_text, done_timeline),
    source_key(partial_audio): record(partial_audio, "partial_success", partial_text, partial_timeline),
}
store_path = data_dir / "asr" / "tasks" / task_id / "files.json"
store_path.parent.mkdir(parents=True, exist_ok=True)
store_path.write_text(json.dumps({"version": 1, "files": files}, indent=2), encoding="utf-8")
PY

curl -fsS "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/tasks/${TASK_ID}" \
  >"$ADMIN_DATA_DIR/task-detail-before-cleanup.json"
python3 - "$ADMIN_DATA_DIR/task-detail-before-cleanup.json" <<'PY'
import json
import sys
with open(sys.argv[1], "r", encoding="utf-8") as f:
    detail = json.load(f)
summary = detail["summary"]
assert summary["audio_source_file_count"] == 2, summary
assert summary["audio_source_bytes"] == len(b"done-audio") + len(b"partial-audio"), summary
assert summary["cleanable_source_file_count"] == 1, summary
assert summary["cleanable_source_bytes"] == len(b"done-audio"), summary
PY

curl -fsS -X POST "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/tasks/${TASK_ID}/cleanup-source-audio" \
  >"$ADMIN_DATA_DIR/task-cleanup.json"
python3 - "$ADMIN_DATA_DIR/task-cleanup.json" "$AUDIO_DIR" "$ADMIN_DATA_DIR" "$TASK_ID" <<'PY'
import json
import pathlib
import sys
with open(sys.argv[1], "r", encoding="utf-8") as f:
    result = json.load(f)
audio_dir = pathlib.Path(sys.argv[2])
data_dir = pathlib.Path(sys.argv[3])
task_id = sys.argv[4]
assert result["ok"] is True, result
assert result["deleted_files"] == 1, result
assert result["deleted_bytes"] == len(b"done-audio"), result
assert not (audio_dir / "done.wav").exists()
assert (audio_dir / "partial.wav").exists()
assert (data_dir / "asr" / "data" / "text" / task_id / "done.txt").is_file()
assert (data_dir / "asr" / "data" / "text" / task_id / "done.timeline.json").is_file()
assert result["summary"]["audio_source_file_count"] == 1, result["summary"]
assert result["summary"]["cleanable_source_file_count"] == 0, result["summary"]
PY

curl -fsS -X POST "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/tasks/${TASK_ID}/cleanup-source-audio" \
  >"$ADMIN_DATA_DIR/task-cleanup-second.json"
python3 - "$ADMIN_DATA_DIR/task-cleanup-second.json" <<'PY'
import json
import sys
with open(sys.argv[1], "r", encoding="utf-8") as f:
    result = json.load(f)
assert result["ok"] is True, result
assert result["deleted_files"] == 0, result
PY

echo "[asr-task-cli-e2e] daily list and show expose generated markdown"
BIFROST_DATA_DIR="$ADMIN_DATA_DIR" "$BIFROST_BIN" ai asr task daily list "$TASK_ID" >"$ADMIN_DATA_DIR/daily-list.out"
grep -q "2026-05-17" "$ADMIN_DATA_DIR/daily-list.out"
BIFROST_DATA_DIR="$ADMIN_DATA_DIR" "$BIFROST_BIN" ai asr task daily show "$TASK_ID" 2026-05-17 >"$ADMIN_DATA_DIR/daily-show.out"
grep -q "完整内容整理" "$ADMIN_DATA_DIR/daily-show.out"
BIFROST_DATA_DIR="$ADMIN_DATA_DIR" "$BIFROST_BIN" ai asr task daily show "$TASK_ID" 2026-05-17 --output "$OUTPUT_DOC" >/dev/null
grep -q "ASR CLI E2E task" "$OUTPUT_DOC"

echo "[asr-task-cli-e2e] run --wait refreshes daily documents without requiring ASR model when no files are pending"
BIFROST_DATA_DIR="$ADMIN_DATA_DIR" "$BIFROST_BIN" ai asr task run "$TASK_ID" --wait >"$ADMIN_DATA_DIR/task-run.out"
grep -q "ASR task completed" "$ADMIN_DATA_DIR/task-run.out"
BIFROST_DATA_DIR="$ADMIN_DATA_DIR" "$BIFROST_BIN" ai asr task daily list "$TASK_ID" >"$ADMIN_DATA_DIR/daily-list-after-run.out"
grep -q "2026-05-17" "$ADMIN_DATA_DIR/daily-list-after-run.out"

echo "[asr-task-cli-e2e] PASS"
