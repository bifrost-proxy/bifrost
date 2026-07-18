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
  rm -rf "$DATA_DIR"
}
trap cleanup EXIT

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

curl -fsS -X PATCH "$api/$TASK_ID" -H 'Content-Type: application/json' \
  --data '{"transcription_prompt":""}' >"$DATA_DIR/cleared.json"
python3 - "$DATA_DIR/cleared.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1], encoding="utf-8"))
assert data["transcription_mode"] == "moss_joint", data
assert data["transcription_prompt"] == "", data
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
