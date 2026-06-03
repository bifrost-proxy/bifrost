#!/usr/bin/env bash

set -euo pipefail
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

fail() {
  echo "[asr-voiceprint-enroll-cli-e2e] error: $*" >&2
  exit 1
}

ADMIN_PORT="${BIFROST_ASR_VOICEPRINT_E2E_PORT:-${ADMIN_PORT:-18994}}"
ADMIN_DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-asr-voiceprint.XXXXXX")"
ADMIN_PID=""

cleanup() {
  if [[ -n "$ADMIN_PID" ]]; then
    kill "$ADMIN_PID" >/dev/null 2>&1 || true
    wait "$ADMIN_PID" >/dev/null 2>&1 || true
  fi
  rm -rf "$ADMIN_DATA_DIR"
}
trap cleanup EXIT

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
  BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/release/bifrost}"
else
  BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/debug/bifrost}"
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

[[ -x "$BIFROST_BIN" ]] || fail "Bifrost binary not executable: $BIFROST_BIN"

PROFILE_DIR="$ADMIN_DATA_DIR/asr/diarization/profiles/sherpa-onnx-balanced"
mkdir -p "$PROFILE_DIR/segmentation" "$PROFILE_DIR/embedding" \
  "$ADMIN_DATA_DIR/asr/diarization/speaker-profiles"
truncate -s 1000001 "$PROFILE_DIR/segmentation/model.int8.onnx"
truncate -s 30000001 "$PROFILE_DIR/embedding/3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx"
printf '{"profile":"sherpa-onnx-balanced","engine":"sherpa-onnx","test_seed":true}' \
  >"$PROFILE_DIR/profile.json"

PCM="$ADMIN_DATA_DIR/one-second.pcm16le"
dd if=/dev/zero of="$PCM" bs=32000 count=1 >/dev/null 2>&1

BIFROST_ASR_VOICEPRINT_TEST_EMBEDDING=1 BIFROST_DATA_DIR="$ADMIN_DATA_DIR" "$BIFROST_BIN" start \
  -p "$ADMIN_PORT" \
  --unsafe-ssl \
  --skip-cert-check \
  --access-mode allow_all \
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

echo "[asr-voiceprint-enroll-cli-e2e] enroll speaker through live prompt CLI"
"$BIFROST_BIN" -p "$ADMIN_PORT" \
  ai asr diarization speakers enroll-live \
  --name Eden \
  --test-pcm16 "$PCM" \
  --json >"$ADMIN_DATA_DIR/enroll.json"

python3 - "$ADMIN_DATA_DIR/enroll.json" <<'PY'
import json, sys
with open(sys.argv[1], "r", encoding="utf-8") as f:
    payload = json.load(f)
profile = payload["profile"]
assert profile["display_name"] == "Eden", profile
assert profile["source"] == "live_enrollment", profile
assert profile["total_duration_ms"] >= 2000, profile
assert len(profile["samples"]) == 3, profile
assert all(sample["text"] for sample in profile["samples"]), profile["samples"]
PY

"$BIFROST_BIN" -p "$ADMIN_PORT" \
  ai asr diarization speakers list --json >"$ADMIN_DATA_DIR/list.json"
python3 - "$ADMIN_DATA_DIR/list.json" <<'PY'
import json, sys
with open(sys.argv[1], "r", encoding="utf-8") as f:
    payload = json.load(f)
profiles = payload["profiles"]
assert len(profiles) == 1, profiles
assert profiles[0]["display_name"] == "Eden", profiles
assert profiles[0]["embedding_dim"] == 16, profiles
PY

echo "[asr-voiceprint-enroll-cli-e2e] ok"
