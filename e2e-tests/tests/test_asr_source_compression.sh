#!/usr/bin/env bash

set -euo pipefail
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

fail() {
  echo "[asr-source-compression-e2e] error: $*" >&2
  exit 1
}

ADMIN_PORT="${BIFROST_ASR_SOURCE_COMPRESSION_E2E_PORT:-$(python3 - <<'PY'
import socket
with socket.socket() as listener:
    listener.bind(("127.0.0.1", 0))
    print(listener.getsockname()[1])
PY
)}"
TEST_DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-asr-compression-data.XXXXXX")"
TEST_AUDIO_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-asr-compression-audio.XXXXXX")"
ADMIN_PID=""

cleanup() {
  if [[ -n "$ADMIN_PID" ]]; then
    kill "$ADMIN_PID" >/dev/null 2>&1 || true
    wait "$ADMIN_PID" >/dev/null 2>&1 || true
  fi
  rm -rf "$TEST_DATA_DIR" "$TEST_AUDIO_DIR"
}
trap cleanup EXIT

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
  BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/debug/bifrost}"
else
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
  BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/debug/bifrost}"
fi
[[ -x "$BIFROST_BIN" ]] || fail "Bifrost binary not executable: $BIFROST_BIN"
command -v ffmpeg >/dev/null || fail "ffmpeg is required"

BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" start \
  -p "$ADMIN_PORT" --unsafe-ssl --skip-cert-check --no-system-proxy \
  >"$TEST_DATA_DIR/bifrost.log" 2>&1 &
ADMIN_PID=$!

for _ in $(seq 1 120); do
  curl -fsS "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/system/overview" >/dev/null 2>&1 && break
  sleep 0.25
done
curl -fsS "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/system/overview" >/dev/null || {
  tail -100 "$TEST_DATA_DIR/bifrost.log" >&2 || true
  fail "Bifrost admin did not start"
}

python3 - "$TEST_AUDIO_DIR" >"$TEST_DATA_DIR/task-request.json" <<'PY'
import json
import sys
print(json.dumps({
    "name": "ASR source compression E2E",
    "audio_dir": sys.argv[1],
    "recursive": True,
    "enabled": False,
    "schedule": {"kind": "daily", "hour": 2, "minute": 0},
    "language": "chinese",
    "model": "Qwen3-ASR-1.7B",
}))
PY
curl -fsS -X POST "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/tasks" \
  -H 'Content-Type: application/json' --data-binary @"$TEST_DATA_DIR/task-request.json" \
  >"$TEST_DATA_DIR/task-create.json"
TASK_ID="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["id"])' "$TEST_DATA_DIR/task-create.json")"

python3 - "$TEST_DATA_DIR" "$TEST_AUDIO_DIR" "$TASK_ID" <<'PY'
import hashlib
import json
import os
import pathlib
import struct
import sys
import wave

data_dir = pathlib.Path(sys.argv[1])
audio_dir = pathlib.Path(sys.argv[2])
task_id = sys.argv[3]

def write_wav(path, seconds):
    with wave.open(str(path), "wb") as output:
        output.setnchannels(1)
        output.setsampwidth(2)
        output.setframerate(16000)
        samples = (1200 if index % 4000 < 2000 else -1200 for index in range(16000 * seconds))
        output.writeframes(b"".join(struct.pack("<h", sample) for sample in samples))

good = audio_dir / "completed.wav"
broken = audio_dir / "broken.wav"
partial = audio_dir / "partial.wav"
write_wav(good, 4)
broken.write_bytes(b"invalid wav data")
write_wav(partial, 1)

output_dir = data_dir / "asr" / "data" / "text" / task_id
output_dir.mkdir(parents=True, exist_ok=True)

def source_key(path):
    stat = path.stat()
    digest = hashlib.sha1()
    digest.update(os.path.realpath(path).encode())
    digest.update(stat.st_size.to_bytes(8, "little"))
    digest.update((stat.st_mtime_ns // 1_000_000).to_bytes(8, "little"))
    return digest.hexdigest()

def record(path, status):
    text = output_dir / f"{path.stem}.txt"
    timeline = output_dir / f"{path.stem}.timeline.json"
    text.write_text(f"transcript for {path.name}", encoding="utf-8")
    timeline.write_text("{}", encoding="utf-8")
    stat = path.stat()
    return {
        "task_id": task_id,
        "source_path": str(path),
        "source_size": stat.st_size,
        "source_modified_ms": stat.st_mtime_ns // 1_000_000,
        "source_created_at_ms": None,
        "source_created_at_source": None,
        "media_duration_ms": 4000,
        "status": status,
        "output_text_path": str(text),
        "output_metadata_path": None,
        "output_timeline_path": str(timeline),
        "text_chars": text.stat().st_size,
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
    source_key(good): record(good, "success"),
    source_key(broken): record(broken, "success"),
    source_key(partial): record(partial, "partial_success"),
}
store_path = data_dir / "asr" / "tasks" / task_id / "files.json"
store_path.parent.mkdir(parents=True, exist_ok=True)
store_path.write_text(json.dumps({"version": 1, "files": files}, indent=2), encoding="utf-8")
PY

ffmpeg -hide_banner -loglevel error -i "$TEST_AUDIO_DIR/completed.wav" \
  -map 0:a:0 -c:a pcm_s32le -f hash -hash sha256 - >"$TEST_DATA_DIR/original-pcm.sha256"

curl -fsS "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/tasks/${TASK_ID}" \
  >"$TEST_DATA_DIR/detail-before.json"
python3 - "$TEST_DATA_DIR/detail-before.json" <<'PY'
import json, sys
summary = json.load(open(sys.argv[1]))["summary"]
assert summary["compressible_source_file_count"] == 2, summary
assert summary["compressed_source_file_count"] == 0, summary
PY

curl -fsS -X POST \
  "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/tasks/${TASK_ID}/compress-source-audio" \
  >"$TEST_DATA_DIR/compression-start.json"

for _ in $(seq 1 200); do
  curl -fsS \
    "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/tasks/${TASK_ID}/compress-source-audio" \
    >"$TEST_DATA_DIR/compression-state.json"
  STATUS="$(python3 -c 'import json,sys; print((json.load(open(sys.argv[1]))["compression"] or {}).get("status", ""))' "$TEST_DATA_DIR/compression-state.json")"
  case "$STATUS" in
    completed|completed_with_errors|cancelled|failed|interrupted) break ;;
  esac
  sleep 0.1
done
[[ "$STATUS" == "completed_with_errors" ]] || fail "unexpected terminal status: $STATUS"

python3 - "$TEST_DATA_DIR/compression-state.json" "$TEST_AUDIO_DIR" <<'PY'
import json
import pathlib
import sys
state = json.load(open(sys.argv[1]))["compression"]
audio = pathlib.Path(sys.argv[2])
assert state["queued_files"] == 2, state
assert state["processed_files"] == 2, state
assert state["compressed_files"] == 1, state
assert state["failed_files"] == 1, state
assert state["saved_bytes"] > 0, state
assert not (audio / "completed.wav").exists()
assert (audio / "completed.flac").is_file()
assert (audio / "broken.wav").is_file()
assert (audio / "partial.wav").is_file()
assert not list(audio.glob(".*bifrost-compress.part")), list(audio.iterdir())
assert not list(audio.glob(".*bifrost-compress-backup")), list(audio.iterdir())
PY

ffmpeg -hide_banner -loglevel error -i "$TEST_AUDIO_DIR/completed.flac" \
  -map 0:a:0 -c:a pcm_s32le -f hash -hash sha256 - >"$TEST_DATA_DIR/compressed-pcm.sha256"
cmp "$TEST_DATA_DIR/original-pcm.sha256" "$TEST_DATA_DIR/compressed-pcm.sha256"

curl -fsS "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/tasks/${TASK_ID}" \
  >"$TEST_DATA_DIR/detail-after.json"
python3 - "$TEST_DATA_DIR/detail-after.json" >"$TEST_DATA_DIR/compressed-key" <<'PY'
import json
import pathlib
import sys
detail = json.load(open(sys.argv[1]))
summary = detail["summary"]
assert summary["pending"] == 0, summary
assert summary["compressed_source_file_count"] == 1, summary
assert summary["compression_saved_bytes"] > 0, summary
completed = next(record for record in detail["files"] if pathlib.Path(record["source_path"]).name == "completed.flac")
assert completed["status"] == "success", completed
assert completed["source_compression"]["codec"] == "flac", completed
assert completed["source_compression"]["pcm_sha256"], completed
print(completed["key"])
PY

COMPRESSED_KEY="$(cat "$TEST_DATA_DIR/compressed-key")"
curl -sS -D "$TEST_DATA_DIR/source-range.headers" -o "$TEST_DATA_DIR/source-range.bin" \
  -H 'Range: bytes=0-31' \
  "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/tasks/${TASK_ID}/files/${COMPRESSED_KEY}/source"
grep -qE '^HTTP/[^ ]+ 206' "$TEST_DATA_DIR/source-range.headers" || \
  fail "compressed source did not return HTTP 206"
grep -qiE '^content-type: audio/flac' "$TEST_DATA_DIR/source-range.headers" || \
  fail "compressed source did not return audio/flac"
[[ "$(wc -c <"$TEST_DATA_DIR/source-range.bin" | tr -d ' ')" == "32" ]] || \
  fail "compressed source range did not contain 32 bytes"

# Simulate the historical stale-snapshot bug: a later writer kept the FLAC
# path but erased its compression metadata. The completed ledger must restore
# API statistics, and the next compression start must persist that repair
# before replacing the ledger.
python3 - "$TEST_DATA_DIR/asr/tasks/$TASK_ID/files.json" "$COMPRESSED_KEY" <<'PY'
import json
import sys
path, key = sys.argv[1:]
store = json.load(open(path))
record = store["files"][key]
record.pop("source_compression", None)
record["source_size"] = None
open(path, "w").write(json.dumps(store, indent=2))
PY
curl -fsS "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/tasks/${TASK_ID}" \
  >"$TEST_DATA_DIR/detail-repaired.json"
python3 - "$TEST_DATA_DIR/detail-repaired.json" "$COMPRESSED_KEY" <<'PY'
import json
import sys
detail = json.load(open(sys.argv[1]))
record = next(item for item in detail["files"] if item["key"] == sys.argv[2])
assert detail["summary"]["compressed_source_file_count"] == 1, detail["summary"]
assert detail["summary"]["compression_saved_bytes"] > 0, detail["summary"]
assert record["source_compression"]["codec"] == "flac", record
assert "pcm_sha256" not in record["source_compression"], record
PY

# A repeated run may retry the still-invalid WAV, but must never enqueue the
# already-compressed success record or the partial-success record.
curl -fsS -X POST \
  "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/tasks/${TASK_ID}/compress-source-audio" \
  >"$TEST_DATA_DIR/compression-retry-start.json"
for _ in $(seq 1 200); do
  curl -fsS \
    "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/tasks/${TASK_ID}/compress-source-audio" \
    >"$TEST_DATA_DIR/compression-retry-state.json"
  RETRY_STATUS="$(python3 -c 'import json,sys; print((json.load(open(sys.argv[1]))["compression"] or {}).get("status", ""))' "$TEST_DATA_DIR/compression-retry-state.json")"
  case "$RETRY_STATUS" in
    completed|completed_with_errors|cancelled|failed|interrupted) break ;;
  esac
  sleep 0.1
done
python3 - "$TEST_DATA_DIR/compression-retry-state.json" "$TEST_AUDIO_DIR" <<'PY'
import json
import pathlib
import sys
state = json.load(open(sys.argv[1]))["compression"]
audio = pathlib.Path(sys.argv[2])
assert state["status"] == "completed_with_errors", state
assert state["queued_files"] == 1, state
assert state["compressed_files"] == 0, state
assert state["failed_files"] == 1, state
assert len(list(audio.glob("completed.flac"))) == 1, list(audio.iterdir())
assert not list(audio.glob(".*bifrost-compress.part")), list(audio.iterdir())
assert not list(audio.glob(".*bifrost-compress-backup")), list(audio.iterdir())
PY
python3 - "$TEST_DATA_DIR/asr/tasks/$TASK_ID/files.json" "$COMPRESSED_KEY" <<'PY'
import json
import sys
record = json.load(open(sys.argv[1]))["files"][sys.argv[2]]
assert record["source_compression"]["codec"] == "flac", record
PY

# Whole-file retry endpoints distinguish transcription failures from failed
# chunks and safely skip failed records whose source audio is unavailable.
python3 - "$TEST_DATA_DIR/asr/tasks/$TASK_ID/files.json" "$TEST_AUDIO_DIR" <<'PY'
import copy
import json
import pathlib
import sys
path, audio_dir = sys.argv[1:]
store = json.load(open(path))
template = next(iter(store["files"].values()))
missing = copy.deepcopy(template)
missing["source_path"] = str(pathlib.Path(audio_dir) / "missing-failed.wav")
missing["source_size"] = 123
missing["status"] = "failed"
missing["error"] = "decoder failed"
missing["output_text_path"] = None
missing["output_metadata_path"] = None
missing["output_timeline_path"] = None
missing["source_compression"] = None
store["files"]["missing-failed-key"] = missing
open(path, "w").write(json.dumps(store, indent=2))
PY
SINGLE_RETRY_STATUS="$(curl -sS -o "$TEST_DATA_DIR/retry-missing.json" -w '%{http_code}' -X POST \
  "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/tasks/${TASK_ID}/files/missing-failed-key/retry")"
[[ "$SINGLE_RETRY_STATUS" == "409" ]] || fail "missing-source retry returned $SINGLE_RETRY_STATUS"
curl -fsS -X POST \
  "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/tasks/${TASK_ID}/retry-failed-files" \
  >"$TEST_DATA_DIR/retry-failed-files.json"
python3 - "$TEST_DATA_DIR/retry-failed-files.json" <<'PY'
import json
import sys
result = json.load(open(sys.argv[1]))
assert result["queued"] == 0, result
assert result["skipped"] == 1, result
PY

echo "[asr-source-compression-e2e] PASS"
