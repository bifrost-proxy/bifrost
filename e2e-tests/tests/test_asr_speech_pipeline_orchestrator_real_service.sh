#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

fail() {
  echo "[asr-speech-pipeline-real] error: $*" >&2
  exit 1
}

PORT="${BIFROST_ASR_PIPELINE_E2E_PORT:-18997}"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-asr-pipeline-real.XXXXXX")"
DATA_DIR="$TEST_ROOT/data"
AUDIO_DIR="$TEST_ROOT/audio"
OUT_DIR="$TEST_ROOT/out"
LOG_FILE="$TEST_ROOT/bifrost.log"
SERVER_PID=""

cleanup() {
  if [[ -n "$SERVER_PID" ]] && kill -0 "$SERVER_PID" >/dev/null 2>&1; then
    kill "$SERVER_PID" >/dev/null 2>&1 || true
    wait "$SERVER_PID" >/dev/null 2>&1 || true
  fi
  rm -rf "$TEST_ROOT"
}
trap cleanup EXIT

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
  BIN="${BIFROST_BIN:-$ROOT_DIR/target/release/bifrost}"
  if [[ ! -x "$BIN" && -x "$ROOT_DIR/target/debug/bifrost" ]]; then
    BIN="$ROOT_DIR/target/debug/bifrost"
  fi
else
  BIN="${BIFROST_BIN:-$ROOT_DIR/target/debug/bifrost}"
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi
[[ -x "$BIN" ]] || fail "Bifrost binary not executable: $BIN"

command -v curl >/dev/null 2>&1 || fail "curl is required"
command -v python3 >/dev/null 2>&1 || fail "python3 is required"

mkdir -p "$DATA_DIR" "$AUDIO_DIR" "$OUT_DIR"

if command -v say >/dev/null 2>&1 && [[ "$(uname -s)" == "Darwin" ]]; then
  say --data-format=LEI16@16000 -o "$AUDIO_DIR/meeting.wav" \
    "hello bifrost speech pipeline. this recording verifies offline subtitle artifacts."
else
  python3 - "$AUDIO_DIR/meeting.wav" <<'PY'
import math
import struct
import sys
import wave

path = sys.argv[1]
sample_rate = 16000
duration_sec = 1.5
frames = int(sample_rate * duration_sec)
with wave.open(path, "wb") as wav:
    wav.setnchannels(1)
    wav.setsampwidth(2)
    wav.setframerate(sample_rate)
    for i in range(frames):
        value = int(9000 * math.sin(2 * math.pi * 440 * i / sample_rate))
        wav.writeframesraw(struct.pack("<h", value))
PY
fi

BIFROST_DATA_DIR="$DATA_DIR" "$BIN" start \
  -p "$PORT" \
  --unsafe-ssl \
  --skip-cert-check \
  --no-system-proxy >"$LOG_FILE" 2>&1 &
SERVER_PID="$!"

for _ in $(seq 1 120); do
  if curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/system/overview" >/dev/null 2>&1; then
    break
  fi
  if ! kill -0 "$SERVER_PID" >/dev/null 2>&1; then
    cat "$LOG_FILE" >&2 || true
    fail "Bifrost exited before admin API was ready"
  fi
  sleep 1
done
curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/system/overview" >/dev/null || {
  tail -120 "$LOG_FILE" >&2 || true
  fail "Bifrost admin did not start"
}

echo "[asr-speech-pipeline-real] speech pipeline status and decisions"
curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/speech/pipelines/status" >"$TEST_ROOT/speech-status.json"
python3 - "$TEST_ROOT/speech-status.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
ids = {profile["id"] for profile in data["profiles"]}
assert "realtime-dictation-local" in ids, data
assert "offline-speaker-subtitle-local" in ids, data
assert "scheduled-speaker-subtitle-local" in ids, data
assert "resources" in data and "runtime" in data, data
PY
curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/speech/decision?mode=realtime_dictation" >"$TEST_ROOT/realtime-decision.json"
python3 - "$TEST_ROOT/realtime-decision.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
assert data["asr"]["provider"] == "qwen3_stateful_streaming", data
assert data["asr"]["model"] == "Qwen3-ASR-0.6B", data
PY
curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/speech/decision?mode=offline_file&speaker_aware=true" >"$TEST_ROOT/offline-decision.json"
python3 - "$TEST_ROOT/offline-decision.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
assert data["pipeline_profile"] == "offline-speaker-subtitle-local", data
assert data["diarization"] is not None, data
assert data["asr"]["model"] == "Qwen3-ASR-1.7B", data
PY

echo "[asr-speech-pipeline-real] legacy ASR websocket is gone"
STATUS_CODE="$(curl -sS -o "$TEST_ROOT/legacy-ws.txt" -w "%{http_code}" "http://127.0.0.1:$PORT/_bifrost/api/asr/transcribe-ws")"
[[ "$STATUS_CODE" == "410" ]] || fail "expected legacy ASR WS HTTP 410, got $STATUS_CODE"
grep -q "/api/voice/listen-ws" "$TEST_ROOT/legacy-ws.txt"

echo "[asr-speech-pipeline-real] wake phrase-only dry-run and lightweight listener"
curl -fsS -X POST "http://127.0.0.1:$PORT/_bifrost/api/voice/wake/profiles" \
  -H 'Content-Type: application/json' \
  --data '{"id":"wake_phrase_only","display_name":"Phrase only"}' >"$TEST_ROOT/wake-profile.json"
curl -fsS -X POST "http://127.0.0.1:$PORT/_bifrost/api/voice/wake/bindings" \
  -H 'Content-Type: application/json' \
  --data '{"id":"wake_binding_phrase_only","phrase":"hello bifrost","profile_id":"wake_phrase_only","cooldown_ms":1,"action":{"type":"key_press","key":"space","keycode":null,"modifiers":["cmd"],"press_count":1}}' >"$TEST_ROOT/wake-binding.json"
curl -fsS -X POST "http://127.0.0.1:$PORT/_bifrost/api/voice/wake/listener/start" \
  -H 'Content-Type: application/json' \
  --data '{"source":"mic","engine":"lightweight_kws_listener","execute":false,"chunk_ms":1000}' >"$TEST_ROOT/wake-listener.json"
python3 - "$TEST_ROOT/wake-listener.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
assert data["listener"]["running"] is True, data
assert data["listener"]["engine"] == "lightweight_kws_listener", data
assert data["listener"]["worker_pid"] is None, data
PY
curl -fsS -X POST "http://127.0.0.1:$PORT/_bifrost/api/voice/wake/trigger" \
  -H 'Content-Type: application/json' \
  --data '{"phrase":"hello bifrost","profile_id":"wake_phrase_only","dry_run":true}' >"$TEST_ROOT/wake-trigger.json"
python3 - "$TEST_ROOT/wake-trigger.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
assert data["matched"] is True, data
assert data["action_result"]["dry_run"] is True, data
assert data["action_result"]["executed"] is False, data
PY
curl -fsS -X POST "http://127.0.0.1:$PORT/_bifrost/api/voice/wake/listener/stop" >/dev/null

ONLINE_ASR="${BIFROST_ASR_PIPELINE_E2E_ONLINE:-}"
if [[ -z "$ONLINE_ASR" ]]; then
  if [[ "$(uname -s)" == "Darwin" && "$(uname -m)" == "arm64" && -z "${CI:-}" ]]; then
    ONLINE_ASR="1"
  else
    ONLINE_ASR="0"
  fi
fi

if [[ "$ONLINE_ASR" != "1" ]]; then
  echo "[asr-speech-pipeline-real] online ASR artifact path skipped; set BIFROST_ASR_PIPELINE_E2E_ONLINE=1 on Apple Silicon with Qwen3-ASR assets to run it"
  echo "[asr-speech-pipeline-real] passed"
  exit 0
fi

echo "[asr-speech-pipeline-real] offline-jobs API creates subtitle artifacts"
curl -fsS -X POST \
  "http://127.0.0.1:$PORT/_bifrost/api/asr/offline-jobs?model=Qwen3-ASR-1.7B&language=english&pipeline_profile=offline-speaker-subtitle-local&speaker_aware=0" \
  -F "file=@$AUDIO_DIR/meeting.wav;type=audio/wav" >"$TEST_ROOT/offline-create.json"
JOB_ID="$(python3 - "$TEST_ROOT/offline-create.json" <<'PY'
import json, sys
print(json.load(open(sys.argv[1]))["job_id"])
PY
)"
for _ in $(seq 1 360); do
  curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/asr/offline-jobs/$JOB_ID" >"$TEST_ROOT/offline-job.json"
  STATUS="$(python3 - "$TEST_ROOT/offline-job.json" <<'PY'
import json, sys
print(json.load(open(sys.argv[1]))["status"])
PY
)"
  [[ "$STATUS" == "succeeded" || "$STATUS" == "failed" ]] && break
  sleep 1
done
python3 - "$TEST_ROOT/offline-job.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
assert data["status"] == "succeeded", data
assert data["artifacts"], data
PY
for format in txt srt vtt timeline_json metadata; do
  curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/asr/offline-jobs/$JOB_ID/artifacts/$format" >"$OUT_DIR/offline.$format"
  [[ -s "$OUT_DIR/offline.$format" ]] || fail "offline artifact $format is empty"
done

echo "[asr-speech-pipeline-real] CLI subtitle downloads offline artifacts"
BIFROST_DATA_DIR="$DATA_DIR" "$BIN" -p "$PORT" ai asr subtitle "$AUDIO_DIR/meeting.wav" \
  --language english \
  --format srt,vtt,txt,timeline_json,metadata \
  --out "$OUT_DIR/cli" \
  --json >"$TEST_ROOT/cli-subtitle.json"
python3 - "$TEST_ROOT/cli-subtitle.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
assert data["status"] == "succeeded", data
assert len(data["outputs"]) == 5, data
PY
test -s "$OUT_DIR/cli/meeting.srt"
test -s "$OUT_DIR/cli/meeting.vtt"
test -s "$OUT_DIR/cli/meeting.txt"
test -s "$OUT_DIR/cli/meeting.timeline.json"
test -s "$OUT_DIR/cli/meeting.metadata.json"

echo "[asr-speech-pipeline-real] directory task keeps artifacts and post-processing surfaces"
TASK_JSON="$(python3 - "$AUDIO_DIR" <<'PY'
import json, sys
print(json.dumps({
  "name": "ASR Speech Pipeline Real",
  "audio_dir": sys.argv[1],
  "recursive": False,
  "enabled": False,
  "schedule": {"kind": "daily", "hour": 3, "minute": 0},
  "language": "english",
  "model": "Qwen3-ASR-1.7B",
  "runtime_strategy": "reuse_per_file",
  "pipeline_profile": "scheduled-speaker-subtitle-local",
  "diarization": {"enabled": False, "profile": "sherpa-onnx-balanced"},
  "subtitle": {"formats": ["srt", "vtt", "txt", "timeline_json"]},
}))
PY
)"
curl -fsS -X POST "http://127.0.0.1:$PORT/_bifrost/api/asr/tasks" \
  -H 'Content-Type: application/json' \
  --data "$TASK_JSON" >"$TEST_ROOT/task-create.json"
TASK_ID="$(python3 - "$TEST_ROOT/task-create.json" <<'PY'
import json, sys
print(json.load(open(sys.argv[1]))["id"])
PY
)"
curl -fsS -X POST "http://127.0.0.1:$PORT/_bifrost/api/asr/tasks/$TASK_ID/run" >"$TEST_ROOT/task-run.json"
for _ in $(seq 1 360); do
  curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/asr/tasks/$TASK_ID" >"$TEST_ROOT/task-detail.json"
  TASK_FILE_STATUS="$(python3 - "$TEST_ROOT/task-detail.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
files = data.get("files") or []
for item in files:
  if item.get("status") in {"success", "completed", "failed"}:
    print(item.get("status"))
    break
else:
  print("")
PY
)"
  if [[ "$TASK_FILE_STATUS" == "success" || "$TASK_FILE_STATUS" == "completed" || "$TASK_FILE_STATUS" == "failed" ]]; then
    break
  fi
  sleep 1
done
python3 - "$TEST_ROOT/task-detail.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
files = data.get("files") or []
assert any(item.get("status") in {"success", "completed"} for item in files), data
PY
FILE_KEY="$(python3 - "$TEST_ROOT/task-detail.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
for item in data.get("files") or []:
  if item.get("status") in {"success", "completed"}:
    print(item.get("file_key") or item["key"])
    break
else:
  raise SystemExit("no completed file")
PY
)"
for format in txt srt vtt timeline_json metadata; do
  curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/asr/tasks/$TASK_ID/files/$FILE_KEY/artifacts/$format" >"$OUT_DIR/task.$format"
  [[ -s "$OUT_DIR/task.$format" ]] || fail "task artifact $format is empty"
done
curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/asr/tasks/$TASK_ID/daily-agent" >"$TEST_ROOT/daily-agent.json"
python3 - "$TEST_ROOT/daily-agent.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
assert "config" in data and "enabled" in data["config"], data
assert "last_run" in data, data
PY

echo "[asr-speech-pipeline-real] passed"
