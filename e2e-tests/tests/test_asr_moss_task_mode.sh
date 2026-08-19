#!/usr/bin/env bash

set -euo pipefail
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT BIFROST_DISABLE_TRAY

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

fail() {
  echo "[asr-moss-task-mode-e2e] error: $*" >&2
  exit 1
}

ADMIN_PORT="${BIFROST_ASR_MOSS_TASK_E2E_PORT:-18995}"
DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-asr-moss-task.XXXXXX")"
AUDIO_DIR="$DATA_DIR/audio"
HOME_DIR="$DATA_DIR/home"
ADMIN_PID=""
mkdir -p "$AUDIO_DIR" "$HOME_DIR"

stop_admin() {
  if [[ -n "$ADMIN_PID" ]]; then
    kill "$ADMIN_PID" >/dev/null 2>&1 || true
    wait "$ADMIN_PID" >/dev/null 2>&1 || true
    ADMIN_PID=""
  fi
}

cleanup() {
  stop_admin
  remove_test_dir "$DATA_DIR"
}
trap cleanup EXIT

remove_test_dir() {
  local dir="$1"
  [[ -d "$dir" ]] || return 0

  # macOS may briefly recreate task state while the stopped admin's workers exit.
  # Cleanup must not turn otherwise-passing API assertions into a test failure.
  for _ in $(seq 1 5); do
    rm -rf "$dir" 2>/dev/null && return 0
    [[ -e "$dir" ]] || return 0
    sleep 0.2
  done

  echo "WARN: failed to remove temporary test directory: $dir" >&2
  return 0
}

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
  BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/debug/bifrost}"
else
  BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/debug/bifrost}"
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi
[[ -x "$BIFROST_BIN" ]] || fail "Bifrost binary not executable: $BIFROST_BIN"

start_admin() {
  HOME="$HOME_DIR" BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" start \
    -p "$ADMIN_PORT" --unsafe-ssl --skip-cert-check --no-system-proxy \
    >"$DATA_DIR/bifrost.log" 2>&1 &
  ADMIN_PID=$!
  for _ in $(seq 1 120); do
    if curl -fsS "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/system/overview" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  tail -100 "$DATA_DIR/bifrost.log" >&2 || true
  fail "temporary Bifrost admin did not start"
}

api="http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/tasks"
start_admin

CREATE_JSON="$(python3 - "$AUDIO_DIR" <<'PY'
import json, sys
print(json.dumps({
    "name": "MOSS task mode E2E",
    "audio_dir": sys.argv[1],
    "recursive": False,
    "enabled": False,
    "schedule": {"kind": "daily", "hour": 2, "minute": 0},
    "language": "chinese",
    "model": "Qwen3-ASR-0.6B",
    "transcription_mode": "moss_joint",
    "transcription_prompt": "  Bifrost\r\nNextOnCall 专有词  ",
    "runtime_strategy": "fork_per_chunk",
    "max_concurrent_files": 4,
    "diarization": {"enabled": True, "profile": "sherpa-onnx-balanced"},
}, ensure_ascii=False))
PY
)"

if [[ "$(uname -s)" != "Darwin" || "$(uname -m)" != "arm64" ]]; then
  CREATE_STATUS="$(curl -sS -o "$DATA_DIR/unsupported-create.json" -w '%{http_code}' \
    -X POST "$api" -H 'Content-Type: application/json' --data "$CREATE_JSON")"
  [[ "$CREATE_STATUS" == "400" ]] || fail "expected unsupported MOSS create status 400, got $CREATE_STATUS"
  grep -q 'only on Apple Silicon macOS' "$DATA_DIR/unsupported-create.json"

  curl -fsS -X POST "$api" -H 'Content-Type: application/json' --data "$(python3 - "$AUDIO_DIR" <<'PY'
import json, sys
print(json.dumps({"name":"standard platform gate","audio_dir":sys.argv[1],"enabled":False}))
PY
)" >"$DATA_DIR/standard.json"
  STANDARD_TASK_ID="$(python3 - "$DATA_DIR/standard.json" <<'PY'
import json, sys
print(json.load(open(sys.argv[1], encoding="utf-8"))["id"])
PY
)"
  UPDATE_STATUS="$(curl -sS -o "$DATA_DIR/unsupported-update.json" -w '%{http_code}' \
    -X PATCH "$api/$STANDARD_TASK_ID" -H 'Content-Type: application/json' \
    --data '{"transcription_mode":"moss_joint"}')"
  [[ "$UPDATE_STATUS" == "400" ]] || fail "expected unsupported MOSS update status 400, got $UPDATE_STATUS"
  grep -q 'only on Apple Silicon macOS' "$DATA_DIR/unsupported-update.json"
  echo "PASS: unsupported hosts reject MOSS task creation and updates"
  exit 0
fi

curl -fsS -X POST "$api" -H 'Content-Type: application/json' --data "$CREATE_JSON" \
  >"$DATA_DIR/create.json"
TASK_ID="$(python3 - "$DATA_DIR/create.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1], encoding="utf-8"))
assert data["transcription_mode"] == "moss_joint", data
assert data["transcription_prompt"] == "Bifrost\nNextOnCall 专有词", data
assert data["summary"]["effective_max_concurrent_files"] == 1, data["summary"]
print(data["id"])
PY
)"

FILES_JSON="$DATA_DIR/asr/tasks/$TASK_ID/files.json"
mkdir -p "$(dirname "$FILES_JSON")"
printf 'fixture audio' > "$AUDIO_DIR/completed.wav"
printf 'missing artifact audio' > "$AUDIO_DIR/missing.wav"
python3 - "$FILES_JSON" "$TASK_ID" "$AUDIO_DIR/completed.wav" "$DATA_DIR" <<'PY'
import json, sys
from pathlib import Path

files_json = Path(sys.argv[1])
task_id = sys.argv[2]
source = sys.argv[3]
data_dir = Path(sys.argv[4])
record = {
    "task_id": task_id,
    "source_path": source,
    "source_size": 13,
    "source_modified_ms": 1,
    "source_created_at_ms": None,
    "source_created_at_source": None,
    "media_duration_ms": 10_000,
    "status": "success",
    "output_text_path": str(data_dir / "old.txt"),
    "output_metadata_path": str(data_dir / "old.json"),
    "output_timeline_path": str(data_dir / "old.timeline.json"),
    "text_chars": 8,
    "error": None,
    "chunk_metrics": [{
        "chunk_index": 0,
        "offset_secs": 0,
        "duration_secs": 10,
        "runner": "moss_joint",
        "status": "ok",
        "elapsed_ms": 100,
        "rtf": 0.01,
        "text_chars": 8,
        "text_sha1": "fixture",
        "recorded_at_ms": 1,
    }],
    "started_at_ms": 1,
    "finished_at_ms": 2,
}
(data_dir / "old.txt").write_text("old text", encoding="utf-8")
(data_dir / "old.json").write_text("{}", encoding="utf-8")
(data_dir / "old.timeline.json").write_text("{}", encoding="utf-8")
missing = dict(record)
missing.update({
    "source_path": str(Path(source).with_name("missing.wav")),
    "source_size": 22,
    "output_text_path": str(data_dir / "gone.txt"),
    "output_metadata_path": str(data_dir / "gone.json"),
    "output_timeline_path": str(data_dir / "gone.timeline.json"),
})
orphaned = dict(record)
orphaned.update({
    "source_path": str(Path(source).with_name("removed.wav")),
    "status": "success",
    "output_text_path": None,
    "output_metadata_path": None,
    "output_timeline_path": None,
    "text_chars": 0,
    "chunk_metrics": [],
    "started_at_ms": None,
    "finished_at_ms": None,
})
files_json.write_text(
    json.dumps({
        "version": 1,
        "files": {"completed": record, "missing": missing, "orphaned": orphaned},
    }),
    encoding="utf-8",
)
PY

curl -fsS -X PATCH "$api/$TASK_ID" -H 'Content-Type: application/json' \
  --data '{"transcription_prompt":""}' >"$DATA_DIR/cleared.json"
python3 - "$DATA_DIR/cleared.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1], encoding="utf-8"))
assert data["transcription_mode"] == "moss_joint", data
assert data["transcription_prompt"] == "", data
PY
python3 - "$FILES_JSON" <<'PY'
import json, sys
records = json.load(open(sys.argv[1], encoding="utf-8"))["files"]
assert "orphaned" not in records, records["orphaned"]
completed = records["completed"]
assert completed["status"] == "success", completed
assert completed["output_text_path"].endswith("old.txt"), completed
assert completed["output_metadata_path"].endswith("old.json"), completed
assert completed["output_timeline_path"].endswith("old.timeline.json"), completed
assert len(completed.get("chunk_metrics", [])) == 1, completed
assert completed["text_chars"] == 8, completed

missing = records["missing"]
assert missing["status"] == "pending", missing
assert missing["output_text_path"] is None, missing
assert missing["output_metadata_path"] is None, missing
assert missing["output_timeline_path"] is None, missing
assert missing.get("chunk_metrics", []) == [], missing
assert missing["text_chars"] == 0, missing
PY

python3 - "$FILES_JSON" "$DATA_DIR" <<'PY'
import json, sys
from pathlib import Path

files_json = Path(sys.argv[1])
data_dir = Path(sys.argv[2])
data = json.loads(files_json.read_text(encoding="utf-8"))
record = data["files"]["completed"]
record.update({
    "status": "success",
    "output_text_path": str(data_dir / "preserved.txt"),
    "output_metadata_path": str(data_dir / "preserved.json"),
    "output_timeline_path": str(data_dir / "preserved.timeline.json"),
    "text_chars": 9,
    "started_at_ms": 3,
    "finished_at_ms": 4,
})
files_json.write_text(json.dumps(data), encoding="utf-8")
PY

curl -fsS -X PATCH "$api/$TASK_ID" -H 'Content-Type: application/json' \
  --data '{"transcription_mode":"standard","requeue_existing_files":false}' \
  >"$DATA_DIR/preserved-standard.json"
curl -fsS -X PATCH "$api/$TASK_ID" -H 'Content-Type: application/json' \
  --data '{"transcription_mode":"moss_joint","requeue_existing_files":false}' \
  >"$DATA_DIR/preserved-moss.json"
python3 - "$DATA_DIR/preserved-moss.json" "$FILES_JSON" <<'PY'
import json, sys
task = json.load(open(sys.argv[1], encoding="utf-8"))
record = json.load(open(sys.argv[2], encoding="utf-8"))["files"]["completed"]
assert task["transcription_mode"] == "moss_joint", task
assert "requeue_existing_files" not in task, task
assert record["status"] == "success", record
assert record["output_text_path"].endswith("preserved.txt"), record
assert record["text_chars"] == 9, record
PY

TOO_LONG="$(python3 - <<'PY'
import json
print(json.dumps({"transcription_prompt": "x" * 4001}))
PY
)"
STATUS="$(curl -sS -o "$DATA_DIR/too-long.json" -w '%{http_code}' -X PATCH "$api/$TASK_ID" \
  -H 'Content-Type: application/json' --data "$TOO_LONG")"
[[ "$STATUS" == "400" ]] || fail "expected overlong prompt status 400, got $STATUS"
grep -q 'must not exceed 4000 characters' "$DATA_DIR/too-long.json"

curl -fsS -X POST "$api" -H 'Content-Type: application/json' --data "$(python3 - "$AUDIO_DIR" <<'PY'
import json, sys
print(json.dumps({"name":"legacy defaults","audio_dir":sys.argv[1],"enabled":False}))
PY
)" >"$DATA_DIR/legacy.json"
python3 - "$DATA_DIR/legacy.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1], encoding="utf-8"))
assert data["transcription_mode"] == "standard", data
assert data["transcription_prompt"] == "", data
PY

stop_admin
start_admin
curl -fsS "$api/$TASK_ID" >"$DATA_DIR/reloaded.json"
python3 - "$DATA_DIR/reloaded.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1], encoding="utf-8"))
assert data["transcription_mode"] == "moss_joint", data
assert data["transcription_prompt"] == "", data
assert data["summary"]["effective_max_concurrent_files"] == 1, data["summary"]
PY

echo "PASS: ASR MOSS task mode and prompt persist across API updates and restart"
