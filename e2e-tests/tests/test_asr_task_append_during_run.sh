#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PORT="${BIFROST_ASR_TASK_APPEND_E2E_PORT:-18884}"
DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-asr-task-append.XXXXXX")"
HOME_DIR="$DATA_DIR/home"
AUDIO_DIR="$DATA_DIR/audio"
FAKE_BIN_DIR="$DATA_DIR/bin"
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
  echo "[asr-task-append] error: $*" >&2
  if [[ -f "$LOG_FILE" ]]; then
    tail -n 160 "$LOG_FILE" >&2 || true
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

make_wav() {
  local path="$1"
  python3 - "$path" <<'PY'
import math
import pathlib
import struct
import sys
import wave

path = pathlib.Path(sys.argv[1])
path.parent.mkdir(parents=True, exist_ok=True)
sample_rate = 16000
frames = int(sample_rate * 0.2)
with wave.open(str(path), "wb") as wav:
    wav.setnchannels(1)
    wav.setsampwidth(2)
    wav.setframerate(sample_rate)
    for i in range(frames):
        sample = int(1200 * math.sin(2 * math.pi * 440 * i / sample_rate))
        wav.writeframesraw(struct.pack("<h", sample))
PY
}

mkdir -p "$AUDIO_DIR" "$FAKE_BIN_DIR" "$HOME_DIR/.bifrost/asr/qwen3_asr_rs/Qwen3-ASR-1.7B"
cat >"$FAKE_BIN_DIR/ffprobe" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf 'pcm_s16le,16000,1\n'
SH
cat >"$FAKE_BIN_DIR/ffmpeg" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
input=""
previous=""
for arg in "$@"; do
  if [[ "$previous" == "-i" ]]; then
    input="$arg"
  fi
  previous="$arg"
done
output="${@: -1}"
if [[ -z "$input" || "$input" == "$output" ]]; then
  echo "fake ffmpeg expected distinct -i input and output" >&2
  exit 1
fi
cp "$input" "$output"
SH
chmod +x "$FAKE_BIN_DIR/ffprobe" "$FAKE_BIN_DIR/ffmpeg"
cat >"$HOME_DIR/.bifrost/asr/qwen3_asr_rs/asr" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
sleep "${BIFROST_FAKE_ASR_SLEEP_SECS:-0}"
audio="$2"
printf '<asr_text>fake transcript for %s</asr_text>\n' "$(basename "$audio")"
SH
chmod +x "$HOME_DIR/.bifrost/asr/qwen3_asr_rs/asr"
printf '{}\n' >"$HOME_DIR/.bifrost/asr/qwen3_asr_rs/Qwen3-ASR-1.7B/tokenizer.json"

FIRST_AUDIO="$AUDIO_DIR/TX02_MIC001_20260525_090000_orig.wav"
SECOND_AUDIO="$AUDIO_DIR/TX02_MIC002_20260525_100000_orig.wav"
make_wav "$FIRST_AUDIO"

echo "[asr-task-append] build bifrost"
cargo build --bin bifrost >/dev/null

echo "[asr-task-append] start bifrost on ${PORT}"
HOME="$HOME_DIR" BIFROST_DATA_DIR="$DATA_DIR" BIFROST_FAKE_ASR_SLEEP_SECS=1.5 PATH="$FAKE_BIN_DIR:$PATH" \
  "$ROOT_DIR/target/debug/bifrost" start \
  -p "$PORT" \
  --unsafe-ssl \
  --skip-cert-check \
  --no-system-proxy \
  >"$LOG_FILE" 2>&1 &
PID="$!"
wait_http "http://127.0.0.1:${PORT}/_bifrost/api/proxy/address"

CREATE_JSON="$DATA_DIR/create.json"
curl -fsS -X POST "http://127.0.0.1:${PORT}/_bifrost/api/asr/tasks" \
  -H 'Content-Type: application/json' \
  --data "$(
    python3 - "$AUDIO_DIR" <<'PY'
import json
import sys
print(json.dumps({
    "name": "Append During Run E2E",
    "audio_dir": sys.argv[1],
    "recursive": True,
    "enabled": False,
    "schedule": {"kind": "daily", "hour": 2, "minute": 0},
    "language": "chinese",
    "model": "Qwen3-ASR-1.7B",
    "runtime_strategy": "fork_per_chunk",
    "daily_agent": {"enabled": False},
}))
PY
  )" >"$CREATE_JSON"
TASK_ID="$(python3 - "$CREATE_JSON" <<'PY'
import json
import sys
print(json.load(open(sys.argv[1]))["id"])
PY
)"

echo "[asr-task-append] start manual run with one file"
curl -fsS -X POST "http://127.0.0.1:${PORT}/_bifrost/api/asr/tasks/${TASK_ID}/run" >"$DATA_DIR/run.json"

WATCH_JSON="$DATA_DIR/watch.json"
for _ in {1..80}; do
  curl -fsS "http://127.0.0.1:${PORT}/_bifrost/api/asr/tasks/${TASK_ID}/watch" >"$WATCH_JSON"
  if python3 - "$WATCH_JSON" <<'PY'
import json
import sys
data = json.load(open(sys.argv[1]))
progress = data.get("progress") or {}
if progress.get("current_file_total") == 1 and progress.get("current_source_path"):
    raise SystemExit(0)
raise SystemExit(1)
PY
  then
    break
  fi
  sleep 0.2
done

python3 - "$WATCH_JSON" <<'PY'
import json
import sys
data = json.load(open(sys.argv[1]))
progress = data.get("progress") or {}
assert progress.get("current_file_total") == 1, data
assert progress.get("current_source_path"), data
PY

echo "[asr-task-append] append second file while first batch is running"
make_wav "$SECOND_AUDIO"

DETAIL_JSON="$DATA_DIR/detail.json"
for _ in {1..120}; do
  curl -fsS "http://127.0.0.1:${PORT}/_bifrost/api/asr/tasks/${TASK_ID}" >"$DETAIL_JSON"
  if python3 - "$DETAIL_JSON" <<'PY'
import json
import sys
data = json.load(open(sys.argv[1]))
files = data.get("files") or []
statuses = sorted((item.get("record") or item).get("status") for item in files)
if data["summary"]["running"]:
    raise SystemExit(1)
if statuses == ["success", "success"]:
    raise SystemExit(0)
raise SystemExit(1)
PY
  then
    break
  fi
  sleep 0.5
done

python3 - "$DETAIL_JSON" <<'PY'
import json
import pathlib
import sys

data = json.load(open(sys.argv[1]))
files = [item.get("record") or item for item in data.get("files") or []]
assert len(files) == 2, data
assert [f["status"] for f in files] == ["success", "success"], files
paths = [pathlib.Path(f["source_path"]).name for f in files]
assert paths == [
    "TX02_MIC001_20260525_090000_orig.wav",
    "TX02_MIC002_20260525_100000_orig.wav",
], paths
dates = [doc["date"] for doc in data.get("daily_documents") or []]
assert "2026-05-25" in dates, dates
PY

echo "[asr-task-append] PASS"
