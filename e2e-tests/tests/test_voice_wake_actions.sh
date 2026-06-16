#!/usr/bin/env bash
set -euo pipefail
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

TEST_ROOT="$(mktemp -d "./.bifrost-e2e-voice-wake.XXXXXX")"
DATA_DIR="$TEST_ROOT/data"
PORT="${BIFROST_VOICE_WAKE_E2E_PORT:-18891}"
TARGET_DIR="${CARGO_TARGET_DIR:-./.bifrost-voice-wake-target}"
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
  echo "[voice-wake] skipping build, using $BIN"
else
  BIN="${BIFROST_BIN:-$TARGET_DIR/debug/bifrost}"
  echo "[voice-wake] building bifrost"
  SKIP_FRONTEND_BUILD=1 CARGO_TARGET_DIR="$TARGET_DIR" cargo build --bin bifrost
fi

if [[ ! -x "$BIN" ]]; then
  echo "[voice-wake] bifrost binary not executable: $BIN" >&2
  exit 1
fi

echo "[voice-wake] starting bifrost on port $PORT"
mkdir -p "$DATA_DIR"
mkdir -p "$DATA_DIR/asr/diarization/speaker-profiles"
python3 - "$DATA_DIR/asr/diarization/speaker-profiles/spk-e2e.json" <<'PY'
import json
import sys

profile = {
    "id": "spk-e2e",
    "display_name": "E2E User",
    "source": "e2e",
    "diarization_profile": "sherpa-onnx-balanced",
    "embedding_model": "e2e-fixture",
    "embedding_dim": 16,
    "embedding": [1.0] + [0.0] * 15,
    "sample_rate": 16000,
    "total_duration_ms": 2000,
    "samples": [],
    "created_at_ms": 1,
    "updated_at_ms": 1,
}
with open(sys.argv[1], "w", encoding="utf-8") as f:
    json.dump(profile, f)
PY
printf 'voice wake fixture' >"$TEST_ROOT/wake.wav"
BIFROST_DATA_DIR="$DATA_DIR" "$BIN" start -p "$PORT" --unsafe-ssl --skip-cert-check --no-system-proxy >"$LOG_FILE" 2>&1 &
SERVER_PID="$!"

for _ in $(seq 1 60); do
  if curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/voice/wake/status" >/dev/null 2>&1; then
    break
  fi
  if ! kill -0 "$SERVER_PID" >/dev/null 2>&1; then
    echo "[voice-wake] bifrost exited early"
    cat "$LOG_FILE"
    exit 1
  fi
  sleep 0.5
done

curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/voice/wake/status" >/dev/null

echo "[voice-wake] rejecting listener start before voiceprint binding exists"
if BIFROST_DATA_DIR="$DATA_DIR" "$BIN" -p "$PORT" ai voice wake listener start --source mock \
  --engine lightweight_kws_listener \
  --mock-transcripts "现在 打开 录音" \
  --dry-run \
  --mock-speaker-profile-id spk-e2e \
  --mock-speaker-confidence 0.91 \
  --json >"$TEST_ROOT/listener-empty-start.json" 2>"$TEST_ROOT/listener-empty-start.err"; then
  echo "[voice-wake] listener unexpectedly started without a saved voice command"
  cat "$TEST_ROOT/listener-empty-start.json"
  exit 1
fi
grep -Eq "speaker voiceprint|saved voice command" "$TEST_ROOT/listener-empty-start.err"

echo "[voice-wake] binding audio and shortcut via CLI"
BIFROST_DATA_DIR="$DATA_DIR" "$BIN" -p "$PORT" ai voice wake bind-audio "$TEST_ROOT/wake.wav" \
  --voiceprint-profile-id spk-e2e \
  --profile-id wake_profile_e2e \
  --binding-id wake_binding_e2e \
  --name "E2E User" \
  --phrase "打开录音" \
  --key space \
  --modifiers cmd \
  --cooldown-ms 1 \
  --json >"$TEST_ROOT/bind-audio.json"
python3 - "$TEST_ROOT/bind-audio.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
assert data["phrase"] == "打开录音", data
assert data["profile"]["id"] == "wake_profile_e2e", data
assert data["profile"]["display_name"] == "E2E User", data
assert data["profile"]["voiceprint_profile_id"] == "spk-e2e", data
assert data["binding"]["id"] == "wake_binding_e2e", data
assert data["binding"]["action"]["type"] == "key_press", data
assert data["binding"]["action"]["key"] == "space", data
PY

echo "[voice-wake] triggering binding dry-run via CLI"
BIFROST_DATA_DIR="$DATA_DIR" "$BIN" -p "$PORT" ai voice wake trigger \
  --profile wake_profile_e2e \
  --phrase "打开录音" \
  --json >"$TEST_ROOT/trigger.json"
python3 - "$TEST_ROOT/trigger.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
assert data["matched"] is True, data
assert data["action_result"]["dry_run"] is True, data
assert data["action_result"]["executed"] is False, data
PY

echo "[voice-wake] checking API events"
curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/voice/wake/events" >"$TEST_ROOT/events.json"
python3 - "$TEST_ROOT/events.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
events = data["events"]
assert len(events) == 1, data
assert events[0]["binding_id"] == "wake_binding_e2e", data
assert events[0]["dry_run"] is True, data
PY

echo "[voice-wake] rejecting legacy backend ASR listener engine"
REJECT_CODE="$(curl -sS -o "$TEST_ROOT/backend-asr-reject.json" -w "%{http_code}" \
  -H 'content-type: application/json' \
  --data '{"source":"mock","engine":"backend_asr_phrase_match","mock_transcripts":["hello bifrost"],"execute":false,"chunk_ms":1000}' \
  "http://127.0.0.1:$PORT/_bifrost/api/voice/wake/listener/start")"
if [[ "$REJECT_CODE" != "400" ]]; then
  echo "[voice-wake] expected backend_asr_phrase_match listener start to return 400, got $REJECT_CODE" >&2
  cat "$TEST_ROOT/backend-asr-reject.json" >&2
  exit 1
fi
grep -q "lightweight_kws_listener" "$TEST_ROOT/backend-asr-reject.json"

echo "[voice-wake] starting lightweight KWS listener with mock wake candidate"
BIFROST_DATA_DIR="$DATA_DIR" "$BIN" -p "$PORT" ai voice wake listener start \
  --source mock \
  --engine lightweight_kws_listener \
  --mock-transcripts "现在 打开 录音" \
  --dry-run \
  --mock-interval-ms 1 \
  --mock-speaker-profile-id spk-e2e \
  --mock-speaker-confidence 0.91 \
  --json >"$TEST_ROOT/listener-start.json"
python3 - "$TEST_ROOT/listener-start.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
assert data["listener"]["running"] is True, data
assert data["listener"]["source"] == "mock", data
PY

for _ in $(seq 1 20); do
  curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/voice/wake/status" >"$TEST_ROOT/listener-status.json"
  if python3 - "$TEST_ROOT/listener-status.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
listener = data["listener"]
raise SystemExit(0 if listener["trigger_count"] >= 1 and listener["last_speaker_profile_id"] == "spk-e2e" else 1)
PY
  then
    break
  fi
  sleep 0.2
done

curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/voice/wake/events" >"$TEST_ROOT/events-after-listener.json"
python3 - "$TEST_ROOT/events-after-listener.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
events = data["events"]
assert len(events) == 2, data
assert events[-1]["binding_id"] == "wake_binding_e2e", data
assert events[-1]["dry_run"] is True, data
assert events[-1]["action_result"]["executed"] is False, data
assert events[-1]["speaker_confidence"] >= 0.9, data
PY

echo "[voice-wake] rejecting mock KWS candidate from a different speaker"
BIFROST_DATA_DIR="$DATA_DIR" "$BIN" -p "$PORT" ai voice wake listener stop --json >/dev/null
sleep 0.05
BIFROST_DATA_DIR="$DATA_DIR" "$BIN" -p "$PORT" ai voice wake listener start \
  --source mock \
  --engine lightweight_kws_listener \
  --mock-transcripts "现在 打开 录音" \
  --dry-run \
  --mock-interval-ms 1 \
  --mock-speaker-profile-id spk-other \
  --mock-speaker-confidence 0.95 \
  --json >"$TEST_ROOT/listener-reject-start.json"

for _ in $(seq 1 20); do
  curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/voice/wake/status" >"$TEST_ROOT/listener-reject-status.json"
  if python3 - "$TEST_ROOT/listener-reject-status.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
error = data["listener"].get("last_error") or ""
raise SystemExit(0 if "speaker verification failed" in error else 1)
PY
  then
    break
  fi
  sleep 0.2
done

curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/voice/wake/events" >"$TEST_ROOT/events-after-reject.json"
python3 - "$TEST_ROOT/events-after-reject.json" "$TEST_ROOT/listener-reject-status.json" <<'PY'
import json, sys
events = json.load(open(sys.argv[1]))["events"]
status = json.load(open(sys.argv[2]))["listener"]
assert len(events) == 2, events
assert status["last_speaker_profile_id"] == "spk-other", status
assert "speaker verification failed" in status["last_error"], status
PY

test -f "$DATA_DIR/voice/wake/actions.json"
echo "[voice-wake] passed"
