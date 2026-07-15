#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

source e2e-tests/test_utils/process.sh

if [[ "$(uname -s)" != "Darwin" || "$(uname -m)" != "arm64" ]]; then
  echo "[asr-media-tool-resolution] skipping: local ASR media-tool fallback requires macOS arm64"
  exit 0
fi

MEDIA_BIN=""
for candidate in /opt/homebrew/bin/ffmpeg /usr/local/bin/ffmpeg /opt/local/bin/ffmpeg; do
  if [[ -x "$candidate" ]]; then
    MEDIA_BIN="$candidate"
    break
  fi
done
if [[ -z "$MEDIA_BIN" ]]; then
  echo "[asr-media-tool-resolution] skipping: no ffmpeg outside the system PATH"
  exit 0
fi

PORT="$(pick_available_base_port "${BIFROST_ASR_MEDIA_TOOL_E2E_PORT:-0}" 1)"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/.bifrost-e2e-asr-media-tool.XXXXXX")"
mark_e2e_data_root "$TEST_ROOT"
DATA_DIR="$TEST_ROOT/data"
LOG_FILE="$TEST_ROOT/bifrost.log"
PID=""

cleanup() {
  if [[ -n "$PID" ]] && kill -0 "$PID" 2>/dev/null; then
    kill "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
  fi
  rm -rf "$TEST_ROOT"
}
trap cleanup EXIT

fail() {
  echo "[asr-media-tool-resolution] error: $*" >&2
  tail -n 160 "$LOG_FILE" >&2 2>/dev/null || true
  exit 1
}

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
  BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/debug/bifrost}"
  echo "[asr-media-tool-resolution] skipping build, using $BIFROST_BIN"
else
  BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/debug/bifrost}"
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost >/dev/null
fi
[[ -x "$BIFROST_BIN" ]] || fail "bifrost binary is not executable: $BIFROST_BIN"

mkdir -p "$DATA_DIR"
SANITIZED_PATH="/usr/bin:/bin:/usr/sbin:/sbin"
env \
  PATH="$SANITIZED_PATH" \
  BIFROST_DATA_DIR="$DATA_DIR" \
  BIFROST_DISABLE_TRAY=1 \
  BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
  "$BIFROST_BIN" start \
    -p "$PORT" \
    --skip-cert-check \
    --no-system-proxy \
    --no-tray \
    >"$LOG_FILE" 2>&1 &
PID="$!"

wait_for_http_ready "http://127.0.0.1:${PORT}/_bifrost/api/system/overview" 60 0.25 \
  || fail "temporary Bifrost did not become ready"

STATUS_FILE="$TEST_ROOT/asr-status.json"
curl -fsS "http://127.0.0.1:${PORT}/_bifrost/api/asr/status" >"$STATUS_FILE"
python3 - "$STATUS_FILE" "$MEDIA_BIN" "$SANITIZED_PATH" <<'PY'
import json
import pathlib
import sys

status_path = pathlib.Path(sys.argv[1])
media_bin = pathlib.Path(sys.argv[2])
sanitized_path = sys.argv[3].split(":")
status = json.loads(status_path.read_text(encoding="utf-8"))

assert str(media_bin.parent) not in sanitized_path, (media_bin, sanitized_path)
assert status["platform_supported"] is True, status
assert status["ffmpeg_available"] is True, status
PY

echo "[asr-media-tool-resolution] PASS: sanitized desktop PATH resolved $MEDIA_BIN"
