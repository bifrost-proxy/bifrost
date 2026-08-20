#!/usr/bin/env bash

set -euo pipefail
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

fail() {
  echo "[asr-assisted-voiceprint-api-e2e] error: $*" >&2
  exit 1
}

expect_status() {
  local expected="$1"
  local output="$2"
  shift 2
  local actual
  actual="$(curl -sS -o "$output" -w '%{http_code}' "$@")"
  [[ "$actual" == "$expected" ]] || fail "expected HTTP $expected, got $actual: $(cat "$output")"
}

ADMIN_PORT="${BIFROST_ASR_ASSISTED_VOICEPRINT_E2E_PORT:-${ADMIN_PORT:-18996}}"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-asr-assisted-voiceprint.XXXXXX")"
ADMIN_PID=""
DAEMON_PID=""
BASE_URL="http://127.0.0.1:${ADMIN_PORT}/_bifrost/api"

if [[ "${BIFROST_ASR_ASSISTED_VOICEPRINT_FORCE_FAKE_FFMPEG:-false}" == "true" ]] || \
  ! command -v ffmpeg >/dev/null 2>&1; then
  mkdir -p "$TEST_ROOT/bin"
  ln -s "$ROOT_DIR/e2e-tests/test_utils/fake_ffmpeg_voiceprint.py" "$TEST_ROOT/bin/ffmpeg"
  export PATH="$TEST_ROOT/bin:$PATH"
fi

wait_for_pid_exit() {
  local pid="${1:-}"
  local attempts="${2:-50}"
  [[ "$pid" =~ ^[0-9]+$ ]] || return 0
  for _ in $(seq 1 "$attempts"); do
    kill -0 "$pid" >/dev/null 2>&1 || return 0
    sleep 0.1
  done
  return 1
}

cleanup() {
  set +e
  local helper_pid=""
  if [[ -f "${DATA_DIR:-}/system_proxy_owner_state.json" ]]; then
    helper_pid="$(python3 - "${DATA_DIR}/system_proxy_owner_state.json" <<'PY'
import json
import sys
try:
    with open(sys.argv[1], encoding="utf-8") as source:
        print(json.load(source).get("helper_pid") or "")
except Exception:
    print("")
PY
)"
  fi
  if [[ -n "${BIFROST_BIN:-}" && -x "${BIFROST_BIN:-}" && -n "${DATA_DIR:-}" ]]; then
    BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" stop >/dev/null 2>&1 || true
  fi
  wait_for_pid_exit "${DAEMON_PID:-}" 50 || kill "${DAEMON_PID}" >/dev/null 2>&1 || true
  if [[ -n "$ADMIN_PID" ]]; then
    kill "$ADMIN_PID" >/dev/null 2>&1 || true
    wait "$ADMIN_PID" >/dev/null 2>&1 || true
  fi
  if ! wait_for_pid_exit "$helper_pid" 100; then
    kill "$helper_pid" >/dev/null 2>&1 || true
    wait_for_pid_exit "$helper_pid" 20 || kill -KILL "$helper_pid" >/dev/null 2>&1 || true
  fi
  for _ in $(seq 1 20); do
    rm -rf "$TEST_ROOT" 2>/dev/null || true
    [[ ! -e "$TEST_ROOT" ]] && return 0
    sleep 0.1
  done
  echo "[asr-assisted-voiceprint-api-e2e] warning: temporary test root remained after cleanup retries: $TEST_ROOT" >&2
  return 0
}
trap cleanup EXIT

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
  BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/debug/bifrost}"
else
  BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/debug/bifrost}"
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi
[[ -x "$BIFROST_BIN" ]] || fail "Bifrost binary not executable: $BIFROST_BIN"

AUDIO_DIR="$TEST_ROOT/audio"
DATA_DIR="$TEST_ROOT/data"
mkdir -p "$AUDIO_DIR" "$DATA_DIR"
SOURCE_WAV="$AUDIO_DIR/meeting.wav"
python3 - "$SOURCE_WAV" <<'PY'
import math
import struct
import sys
import wave

sample_rate = 16000
with wave.open(sys.argv[1], "wb") as output:
    output.setnchannels(1)
    output.setsampwidth(2)
    output.setframerate(sample_rate)
    for index in range(sample_rate * 40):
        sample = int(12000 * math.sin(2 * math.pi * 440 * index / sample_rate))
        output.writeframesraw(struct.pack("<h", sample))
PY

PROFILE_DIR="$DATA_DIR/asr/diarization/profiles/sherpa-onnx-balanced"
mkdir -p "$PROFILE_DIR/segmentation" "$PROFILE_DIR/embedding" \
  "$DATA_DIR/asr/diarization/speaker-profiles"
truncate -s 1000001 "$PROFILE_DIR/segmentation/model.int8.onnx"
truncate -s 30000001 "$PROFILE_DIR/embedding/3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx"
printf '{"profile":"sherpa-onnx-balanced","engine":"sherpa-onnx","test_seed":true}' \
  >"$PROFILE_DIR/profile.json"

BIFROST_ASR_VOICEPRINT_TEST_EMBEDDING=1 BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" start \
  -p "$ADMIN_PORT" --unsafe-ssl --skip-cert-check --access-mode allow_all \
  --no-system-proxy >"$TEST_ROOT/bifrost.log" 2>&1 &
ADMIN_PID=$!

for _ in $(seq 1 120); do
  if curl -fsS "$BASE_URL/system/overview" >/dev/null 2>&1; then
    break
  fi
  sleep 1
done
curl -fsS "$BASE_URL/system/overview" >/dev/null || {
  tail -100 "$TEST_ROOT/bifrost.log" >&2 || true
  fail "Bifrost admin did not start"
}
if [[ -f "$DATA_DIR/bifrost.pid" ]]; then
  DAEMON_PID="$(tr -cd '0-9' < "$DATA_DIR/bifrost.pid")"
fi

curl -fsS -X POST "$BASE_URL/asr/tasks" -H 'Content-Type: application/json' \
  --data "{\"name\":\"Assisted E2E\",\"audio_dir\":\"$AUDIO_DIR\",\"enabled\":false}" \
  >"$TEST_ROOT/task.json"
TASK_ID="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["id"])' "$TEST_ROOT/task.json")"
TASK_DIR="$DATA_DIR/asr/tasks/$TASK_ID"
TIMELINE="$TASK_DIR/meeting.timeline.json"
mkdir -p "$TASK_DIR"

python3 - "$TASK_ID" "$SOURCE_WAV" "$TIMELINE" "$TASK_DIR/files.json" <<'PY'
import json, os, sys, time
task_id, source, timeline_path, files_path = sys.argv[1:]
segments = []
for index in range(8):
    start = index * 4000
    segments.append({
        "index": index,
        "audio_start_ms": start,
        "audio_end_ms": start + 4000,
        "absolute_start_ms": None,
        "absolute_end_ms": None,
        "speaker": "speaker_00" if index < 6 else "speaker_01",
        "speaker_display_name": "User A" if index < 6 else "User B",
        "overlap": False,
        "text": f"meeting segment {index}",
    })
timeline = {
    "task_id": task_id,
    "task_name": "Assisted E2E",
    "source_path": source,
    "source_size": os.path.getsize(source),
    "source_modified_ms": int(os.path.getmtime(source) * 1000),
    "source_created_at_ms": None,
    "source_created_at_source": None,
    "media_duration_ms": 40000,
    "model": "test",
    "language": "chinese",
    "diarization_profile": "sherpa-onnx-balanced",
    "speakers": [
        {"id": "speaker_00", "display_name": "User A"},
        {"id": "speaker_01", "display_name": "User B"},
    ],
    "processed_at_ms": int(time.time() * 1000),
    "segments": segments,
}
with open(timeline_path, "w", encoding="utf-8") as f:
    json.dump(timeline, f)
record = {
    "task_id": task_id,
    "source_path": source,
    "source_size": os.path.getsize(source),
    "source_modified_ms": int(os.path.getmtime(source) * 1000),
    "source_created_at_ms": None,
    "source_created_at_source": None,
    "media_duration_ms": 40000,
    "status": "success",
    "output_text_path": None,
    "output_metadata_path": None,
    "output_timeline_path": timeline_path,
    "text_chars": 128,
    "error": None,
    "runtime_strategy": "fork_per_chunk",
    "started_at_ms": 1,
    "finished_at_ms": 2,
}
with open(files_path, "w", encoding="utf-8") as f:
    json.dump({"version": 1, "files": {"meeting": record}}, f)
PY

cp "$TASK_DIR/files.json" "$TEST_ROOT/files.valid.json"
cp "$TIMELINE" "$TEST_ROOT/timeline.valid.json"

echo "[asr-assisted-voiceprint-api-e2e] validate assisted enrollment error boundaries"
expect_status 400 "$TEST_ROOT/empty-name.json" -X POST \
  "$BASE_URL/asr/speaker-profiles/assisted-sessions" -H 'Content-Type: application/json' \
  --data "{\"name\":\"   \",\"task_id\":\"$TASK_ID\",\"file_key\":\"meeting\"}"
expect_status 400 "$TEST_ROOT/invalid-profile.json" -X POST \
  "$BASE_URL/asr/speaker-profiles/assisted-sessions" -H 'Content-Type: application/json' \
  --data "{\"name\":\"Eden\",\"profile_id\":\"../bad\",\"task_id\":\"$TASK_ID\",\"file_key\":\"meeting\"}"
expect_status 404 "$TEST_ROOT/missing-profile.json" -X POST \
  "$BASE_URL/asr/speaker-profiles/assisted-sessions" -H 'Content-Type: application/json' \
  --data "{\"name\":\"Eden\",\"profile_id\":\"spk-missing\",\"task_id\":\"$TASK_ID\",\"file_key\":\"meeting\"}"
expect_status 404 "$TEST_ROOT/missing-task.json" -X POST \
  "$BASE_URL/asr/speaker-profiles/assisted-sessions" -H 'Content-Type: application/json' \
  --data '{"name":"Eden","task_id":"task-missing","file_key":"meeting"}'
expect_status 404 "$TEST_ROOT/missing-file.json" -X POST \
  "$BASE_URL/asr/speaker-profiles/assisted-sessions" -H 'Content-Type: application/json' \
  --data "{\"name\":\"Eden\",\"task_id\":\"$TASK_ID\",\"file_key\":\"missing\"}"

python3 - "$TASK_DIR/files.json" <<'PY'
import json, sys
path = sys.argv[1]
payload = json.load(open(path, encoding="utf-8"))
payload["files"]["meeting"]["status"] = "pending"
json.dump(payload, open(path, "w", encoding="utf-8"))
PY
expect_status 409 "$TEST_ROOT/incomplete-file.json" -X POST \
  "$BASE_URL/asr/speaker-profiles/assisted-sessions" -H 'Content-Type: application/json' \
  --data "{\"name\":\"Eden\",\"task_id\":\"$TASK_ID\",\"file_key\":\"meeting\"}"
cp "$TEST_ROOT/files.valid.json" "$TASK_DIR/files.json"

mv "$SOURCE_WAV" "$SOURCE_WAV.missing"
expect_status 409 "$TEST_ROOT/missing-source.json" -X POST \
  "$BASE_URL/asr/speaker-profiles/assisted-sessions" -H 'Content-Type: application/json' \
  --data "{\"name\":\"Eden\",\"task_id\":\"$TASK_ID\",\"file_key\":\"meeting\"}"
mv "$SOURCE_WAV.missing" "$SOURCE_WAV"

python3 - "$TASK_DIR/files.json" <<'PY'
import json, sys
path = sys.argv[1]
payload = json.load(open(path, encoding="utf-8"))
payload["files"]["meeting"]["output_timeline_path"] = None
json.dump(payload, open(path, "w", encoding="utf-8"))
PY
expect_status 409 "$TEST_ROOT/no-timeline.json" -X POST \
  "$BASE_URL/asr/speaker-profiles/assisted-sessions" -H 'Content-Type: application/json' \
  --data "{\"name\":\"Eden\",\"task_id\":\"$TASK_ID\",\"file_key\":\"meeting\"}"
cp "$TEST_ROOT/files.valid.json" "$TASK_DIR/files.json"

printf '{invalid' >"$TIMELINE"
expect_status 500 "$TEST_ROOT/invalid-timeline.json" -X POST \
  "$BASE_URL/asr/speaker-profiles/assisted-sessions" -H 'Content-Type: application/json' \
  --data "{\"name\":\"Eden\",\"task_id\":\"$TASK_ID\",\"file_key\":\"meeting\"}"
cp "$TEST_ROOT/timeline.valid.json" "$TIMELINE"

python3 - "$TIMELINE" <<'PY'
import json, sys
path = sys.argv[1]
payload = json.load(open(path, encoding="utf-8"))
payload["diarization_profile"] = None
json.dump(payload, open(path, "w", encoding="utf-8"))
PY
expect_status 409 "$TEST_ROOT/no-diarization.json" -X POST \
  "$BASE_URL/asr/speaker-profiles/assisted-sessions" -H 'Content-Type: application/json' \
  --data "{\"name\":\"Eden\",\"task_id\":\"$TASK_ID\",\"file_key\":\"meeting\"}"

python3 - "$TIMELINE" <<'PY'
import json, sys
path = sys.argv[1]
payload = json.load(open(path, encoding="utf-8"))
payload["diarization_profile"] = "sherpa-onnx-balanced"
payload["segments"] = []
json.dump(payload, open(path, "w", encoding="utf-8"))
PY
expect_status 409 "$TEST_ROOT/no-candidates.json" -X POST \
  "$BASE_URL/asr/speaker-profiles/assisted-sessions" -H 'Content-Type: application/json' \
  --data "{\"name\":\"Eden\",\"task_id\":\"$TASK_ID\",\"file_key\":\"meeting\"}"
cp "$TEST_ROOT/timeline.valid.json" "$TIMELINE"

echo "[asr-assisted-voiceprint-api-e2e] reject arbitrary source path"
HTTP_CODE="$(curl -sS -o "$TEST_ROOT/rejected.json" -w '%{http_code}' -X POST \
  "$BASE_URL/asr/speaker-profiles/assisted-sessions" -H 'Content-Type: application/json' \
  --data "{\"name\":\"Eden\",\"task_id\":\"$TASK_ID\",\"file_key\":\"meeting\",\"source_path\":\"/tmp/forbidden.wav\"}")"
[[ "$HTTP_CODE" == "400" ]] || fail "arbitrary source_path was not rejected: HTTP $HTTP_CODE"

create_and_finish() {
  local profile_id="${1:-}"
  local request_file="$TEST_ROOT/session-request.json"
  python3 - "$TASK_ID" "$profile_id" "$request_file" <<'PY'
import json, sys
task_id, profile_id, path = sys.argv[1:]
payload = {"name": "Eden", "task_id": task_id, "file_key": "meeting"}
if profile_id:
    payload["profile_id"] = profile_id
with open(path, "w", encoding="utf-8") as f:
    json.dump(payload, f)
PY
  curl -fsS -X POST "$BASE_URL/asr/speaker-profiles/assisted-sessions" \
    -H 'Content-Type: application/json' --data-binary "@$request_file" >"$TEST_ROOT/session.json"
  python3 - "$TEST_ROOT/session.json" "$TEST_ROOT/labels.json" <<'PY'
import json, sys
payload = json.load(open(sys.argv[1], encoding="utf-8"))
candidates = payload["session"]["candidates"]
assert candidates and all(not item["overlap"] for item in candidates), candidates
assert all(item["duration_ms"] >= 3000 for item in candidates), candidates
labels = [{"candidate_id": item["id"], "label": "mine"} for item in candidates[:3]]
with open(sys.argv[2], "w", encoding="utf-8") as f:
    json.dump({"labels": labels}, f)
PY
  local session_id
  session_id="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["session"]["id"])' "$TEST_ROOT/session.json")"
  curl -fsS "$BASE_URL/asr/speaker-profiles/assisted-sessions/$session_id" >"$TEST_ROOT/session-get.json"
  expect_status 400 "$TEST_ROOT/invalid-session-get.json" \
    "$BASE_URL/asr/speaker-profiles/assisted-sessions/%2E%2Ebad"
  expect_status 404 "$TEST_ROOT/missing-session-get.json" \
    "$BASE_URL/asr/speaker-profiles/assisted-sessions/assisted-missing"
  expect_status 400 "$TEST_ROOT/invalid-label-json.json" -X POST \
    "$BASE_URL/asr/speaker-profiles/assisted-sessions/$session_id/labels" \
    -H 'Content-Type: application/json' --data '{invalid'
  expect_status 400 "$TEST_ROOT/unknown-candidate.json" -X POST \
    "$BASE_URL/asr/speaker-profiles/assisted-sessions/$session_id/labels" \
    -H 'Content-Type: application/json' --data '{"labels":[{"candidate_id":"missing","label":"mine"}]}'
  expect_status 400 "$TEST_ROOT/below-gate.json" -X POST \
    "$BASE_URL/asr/speaker-profiles/assisted-sessions/$session_id/finish" \
    -H 'Content-Type: application/json' --data '{}'
  python3 - "$DATA_DIR/asr/diarization/speaker-profiles/assisted-sessions/$session_id/session.json" <<'PY'
import json, sys
path = sys.argv[1]
payload = json.load(open(path, encoding="utf-8"))
payload["state"] = "finishing"
json.dump(payload, open(path, "w", encoding="utf-8"))
PY
  expect_status 409 "$TEST_ROOT/finishing-label.json" -X POST \
    "$BASE_URL/asr/speaker-profiles/assisted-sessions/$session_id/labels" \
    -H 'Content-Type: application/json' --data-binary "@$TEST_ROOT/labels.json"
  python3 - "$DATA_DIR/asr/diarization/speaker-profiles/assisted-sessions/$session_id/session.json" <<'PY'
import json, sys
path = sys.argv[1]
payload = json.load(open(path, encoding="utf-8"))
payload["state"] = "open"
json.dump(payload, open(path, "w", encoding="utf-8"))
PY
  curl -fsS -X POST "$BASE_URL/asr/speaker-profiles/assisted-sessions/$session_id/labels" \
    -H 'Content-Type: application/json' --data-binary "@$TEST_ROOT/labels.json" >"$TEST_ROOT/labeled.json"
  python3 - "$TEST_ROOT/labeled.json" <<'PY'
import json, sys
payload = json.load(open(sys.argv[1], encoding="utf-8"))
assert payload["selected_count"] == 3, payload
assert payload["selected_duration_ms"] >= 12000, payload
assert payload["ready_to_finish"] is True, payload
PY
  mv "$SOURCE_WAV" "$SOURCE_WAV.missing"
  expect_status 409 "$TEST_ROOT/finish-missing-source.json" -X POST \
    "$BASE_URL/asr/speaker-profiles/assisted-sessions/$session_id/finish" \
    -H 'Content-Type: application/json' --data '{}'
  mv "$SOURCE_WAV.missing" "$SOURCE_WAV"
  cp "$SOURCE_WAV" "$TEST_ROOT/source.valid.wav"
  printf 'not audio' >"$SOURCE_WAV"
  expect_status 409 "$TEST_ROOT/finish-invalid-audio.json" -X POST \
    "$BASE_URL/asr/speaker-profiles/assisted-sessions/$session_id/finish" \
    -H 'Content-Type: application/json' --data '{}'
  cp "$TEST_ROOT/source.valid.wav" "$SOURCE_WAV"
  curl -fsS -X POST "$BASE_URL/asr/speaker-profiles/assisted-sessions/$session_id/finish" \
    -H 'Content-Type: application/json' --data '{}' >"$TEST_ROOT/finished.json"
  [[ ! -d "$DATA_DIR/asr/diarization/speaker-profiles/assisted-sessions/$session_id" ]] || \
    fail "finished assisted session was not cleaned: $session_id"
  python3 - "$TEST_ROOT/finished.json" <<'PY'
import json, sys
profile = json.load(open(sys.argv[1], encoding="utf-8"))["profile"]
assert profile["schema_version"] == 2, profile
assert profile["source"] == "assisted_recording", profile
assert len(profile["templates"]) >= 3, profile
assert profile["prototypes"], profile
assert all(item["source_kind"] == "task_segment" for item in profile["templates"]), profile
PY
}

echo "[asr-assisted-voiceprint-api-e2e] create and finish"
create_and_finish
PROFILE_ID="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["profile"]["id"])' "$TEST_ROOT/finished.json")"
curl -fsS "$BASE_URL/asr/speaker-profiles" >"$TEST_ROOT/profiles.json"
curl -fsS "$BASE_URL/asr/speaker-profiles/$PROFILE_ID" >"$TEST_ROOT/profile.json"
expect_status 400 "$TEST_ROOT/invalid-profile-get.json" "$BASE_URL/asr/speaker-profiles/%2E%2Ebad"
expect_status 404 "$TEST_ROOT/missing-profile-get.json" "$BASE_URL/asr/speaker-profiles/spk-missing"
expect_status 404 "$TEST_ROOT/malformed-sample-route.json" -X DELETE \
  "$BASE_URL/asr/speaker-profiles/$PROFILE_ID/samples/sample/extra"
expect_status 409 "$TEST_ROOT/profile-name-mismatch.json" -X POST \
  "$BASE_URL/asr/speaker-profiles/assisted-sessions" -H 'Content-Type: application/json' \
  --data "{\"name\":\"Not Eden\",\"profile_id\":\"$PROFILE_ID\",\"task_id\":\"$TASK_ID\",\"file_key\":\"meeting\"}"

echo "[asr-assisted-voiceprint-api-e2e] append samples"
create_and_finish "$PROFILE_ID"
python3 - "$TEST_ROOT/finished.json" <<'PY'
import json, sys
profile = json.load(open(sys.argv[1], encoding="utf-8"))["profile"]
assert len(profile["templates"]) == 6, profile
PY

SAMPLE_ID="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["profile"]["templates"][0]["id"])' "$TEST_ROOT/finished.json")"
expect_status 400 "$TEST_ROOT/invalid-sample-profile.json" -X DELETE \
  "$BASE_URL/asr/speaker-profiles/%2E%2Ebad/samples/$SAMPLE_ID"
expect_status 404 "$TEST_ROOT/missing-sample-profile.json" -X DELETE \
  "$BASE_URL/asr/speaker-profiles/spk-missing/samples/$SAMPLE_ID"
expect_status 404 "$TEST_ROOT/missing-sample.json" -X DELETE \
  "$BASE_URL/asr/speaker-profiles/$PROFILE_ID/samples/sample-missing"
curl -fsS -X DELETE "$BASE_URL/asr/speaker-profiles/$PROFILE_ID/samples/$SAMPLE_ID" \
  >"$TEST_ROOT/deleted-sample.json"
python3 - "$TEST_ROOT/deleted-sample.json" <<'PY'
import json, sys
payload = json.load(open(sys.argv[1], encoding="utf-8"))
assert payload["deleted"] is True, payload
assert len(payload["profile"]["templates"]) == 5, payload
assert payload["profile"]["prototypes"], payload
PY

curl -fsS -X POST "$BASE_URL/asr/speaker-profiles/assisted-sessions" \
  -H 'Content-Type: application/json' \
  --data "{\"name\":\"Eden\",\"profile_id\":\"$PROFILE_ID\",\"task_id\":\"$TASK_ID\",\"file_key\":\"meeting\"}" \
  >"$TEST_ROOT/cancel-session.json"
CANCEL_SESSION_ID="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["session"]["id"])' "$TEST_ROOT/cancel-session.json")"
curl -fsS -X DELETE "$BASE_URL/asr/speaker-profiles/assisted-sessions/$CANCEL_SESSION_ID" \
  >"$TEST_ROOT/cancelled.json"
python3 - "$TEST_ROOT/cancelled.json" "$CANCEL_SESSION_ID" <<'PY'
import json, sys
payload = json.load(open(sys.argv[1], encoding="utf-8"))
assert payload == {"deleted": True, "session_id": sys.argv[2]}, payload
PY
[[ ! -d "$DATA_DIR/asr/diarization/speaker-profiles/assisted-sessions/$CANCEL_SESSION_ID" ]] || \
  fail "cancelled assisted session was not cleaned: $CANCEL_SESSION_ID"
expect_status 400 "$TEST_ROOT/invalid-session-delete.json" -X DELETE \
  "$BASE_URL/asr/speaker-profiles/assisted-sessions/%2E%2Ebad"
expect_status 404 "$TEST_ROOT/missing-session-delete.json" -X DELETE \
  "$BASE_URL/asr/speaker-profiles/assisted-sessions/assisted-missing"

rm -rf "$DATA_DIR/asr/diarization/speaker-profiles/assisted-sessions"
printf 'blocks session directory creation' \
  >"$DATA_DIR/asr/diarization/speaker-profiles/assisted-sessions"
expect_status 500 "$TEST_ROOT/session-directory-blocked.json" -X POST \
  "$BASE_URL/asr/speaker-profiles/assisted-sessions" -H 'Content-Type: application/json' \
  --data "{\"name\":\"Eden\",\"profile_id\":\"$PROFILE_ID\",\"task_id\":\"$TASK_ID\",\"file_key\":\"meeting\"}"
rm -f "$DATA_DIR/asr/diarization/speaker-profiles/assisted-sessions"

curl -fsS -X DELETE "$BASE_URL/asr/speaker-profiles/$PROFILE_ID" >"$TEST_ROOT/deleted-profile.json"
python3 - "$TEST_ROOT/deleted-profile.json" "$PROFILE_ID" <<'PY'
import json, sys
payload = json.load(open(sys.argv[1], encoding="utf-8"))
assert payload == {"deleted": True, "profile_id": sys.argv[2]}, payload
PY
expect_status 404 "$TEST_ROOT/missing-profile-delete.json" -X DELETE \
  "$BASE_URL/asr/speaker-profiles/$PROFILE_ID"

echo "[asr-assisted-voiceprint-api-e2e] ok"
