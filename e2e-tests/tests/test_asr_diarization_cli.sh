#!/usr/bin/env bash

set -euo pipefail
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

fail() {
  echo "[asr-diarization-cli-e2e] error: $*" >&2
  exit 1
}

ADMIN_PORT="${BIFROST_ASR_DIARIZATION_E2E_PORT:-${ADMIN_PORT:-18993}}"
ADMIN_DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-asr-diarization.XXXXXX")"
AUDIO_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-asr-diarization-audio.XXXXXX")"
ADMIN_PID=""

cleanup() {
  if [[ -n "$ADMIN_PID" ]]; then
    kill "$ADMIN_PID" >/dev/null 2>&1 || true
    wait "$ADMIN_PID" >/dev/null 2>&1 || true
  fi
  rm -rf "$ADMIN_DATA_DIR" "$AUDIO_DIR"
}
trap cleanup EXIT

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
  BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/release/bifrost}"
else
  BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/debug/bifrost}"
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

[[ -x "$BIFROST_BIN" ]] || fail "Bifrost binary not executable: $BIFROST_BIN"

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

echo "[asr-diarization-cli-e2e] profile starts uninitialized"
BIFROST_DATA_DIR="$ADMIN_DATA_DIR" "$BIFROST_BIN" -p "$ADMIN_PORT" \
  ai asr diarization status --json >"$ADMIN_DATA_DIR/diarization-status-before.json"
python3 - "$ADMIN_DATA_DIR/diarization-status-before.json" <<'PY'
import json, sys
with open(sys.argv[1], "r", encoding="utf-8") as f:
    status = json.load(f)
assert status["profile"]["id"] == "sherpa-onnx-balanced", status
assert status["profile"]["ready"] is False, status
PY

echo "[asr-diarization-cli-e2e] init profile through CLI"
BIFROST_DATA_DIR="$ADMIN_DATA_DIR" "$BIFROST_BIN" -p "$ADMIN_PORT" \
  ai asr diarization init --json >"$ADMIN_DATA_DIR/diarization-init.json"
python3 - "$ADMIN_DATA_DIR/diarization-init.json" "$ADMIN_DATA_DIR" <<'PY'
import json, pathlib, sys
with open(sys.argv[1], "r", encoding="utf-8") as f:
    payload = json.load(f)
profile = payload["status"]["profile"]
assert profile["ready"] is True, payload
profile_dir = pathlib.Path(sys.argv[2]) / "asr" / "diarization" / "profiles" / "sherpa-onnx-balanced"
assert (profile_dir / "profile.json").is_file()
segmentation = profile_dir / "segmentation" / "model.int8.onnx"
embedding = profile_dir / "embedding" / "3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx"
assert segmentation.is_file() and segmentation.stat().st_size > 1_000_000, segmentation
assert embedding.is_file() and embedding.stat().st_size > 30_000_000, embedding
PY

TASK_JSON="$(python3 - "$AUDIO_DIR" <<'PY'
import json, sys
print(json.dumps({
    "name": "ASR diarization E2E task",
    "audio_dir": sys.argv[1],
    "recursive": True,
    "enabled": False,
    "schedule": {"kind": "daily", "hour": 2, "minute": 0},
    "language": "chinese",
    "model": "Qwen3-ASR-1.7B",
    "runtime_strategy": "reuse_per_file",
    "diarization": {
        "enabled": True,
        "profile": "sherpa-onnx-balanced",
        "known_speaker_count": 2,
        "voiceprint_matching": True
    }
}))
PY
)"

curl -fsS -X POST "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/tasks" \
  -H 'Content-Type: application/json' \
  --data "$TASK_JSON" >"$ADMIN_DATA_DIR/task-create.json"
TASK_ID="$(python3 - "$ADMIN_DATA_DIR/task-create.json" <<'PY'
import json, sys
with open(sys.argv[1], "r", encoding="utf-8") as f:
    print(json.load(f)["id"])
PY
)"

BIFROST_DATA_DIR="$ADMIN_DATA_DIR" "$BIFROST_BIN" -p "$ADMIN_PORT" \
  ai asr task show "$TASK_ID" >"$ADMIN_DATA_DIR/task-show.out"
grep -q "Diarization:" "$ADMIN_DATA_DIR/task-show.out"
grep -q "sherpa-onnx-balanced" "$ADMIN_DATA_DIR/task-show.out"

curl -fsS "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/asr/tasks/${TASK_ID}" \
  >"$ADMIN_DATA_DIR/task-detail.json"
python3 - "$ADMIN_DATA_DIR/task-detail.json" <<'PY'
import json, sys
with open(sys.argv[1], "r", encoding="utf-8") as f:
    detail = json.load(f)
assert detail["diarization"]["enabled"] is True, detail
assert detail["summary"]["diarization_enabled"] is True, detail["summary"]
assert detail["summary"]["diarization_ready"] is True, detail["summary"]
PY

echo "[asr-diarization-cli-e2e] ok"
