#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "SKIP: tray menu click regression is macOS-only"
  exit 0
fi

if [[ "${SKIP_BUILD:-false}" == "true" || "${BIFROST_TRAY_MENU_SKIP_UNIT_GUARD:-}" == "1" ]]; then
  echo "Skipping tray menu click regression unit guard (SKIP_BUILD=${SKIP_BUILD:-false})"
else
  echo "Running tray menu click regression unit guard..."
  cargo test -p bifrost-cli pure_tray_icon_event_does_not_rebuild_native_menu -- --nocapture
fi

if [[ -n "${BIFROST_BIN:-}" ]]; then
  BIN="$BIFROST_BIN"
elif [[ "${SKIP_BUILD:-false}" == "true" ]]; then
  BIN="$ROOT_DIR/target/release/bifrost"
else
  echo "Building bifrost binary..."
  cargo build --bin bifrost
  BIN="$ROOT_DIR/target/debug/bifrost"
fi
if [[ ! -x "$BIN" ]]; then
  echo "FAIL: bifrost binary is not executable: $BIN" >&2
  exit 1
fi
DATA_DIR="$(mktemp -d "$ROOT_DIR/.bifrost-e2e-tray.XXXXXX")"
PORT="$(python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
)"
START_PID=""

cleanup() {
  if [[ -n "${START_PID:-}" ]] && kill -0 "$START_PID" 2>/dev/null; then
    BIFROST_DATA_DIR="$DATA_DIR" "$BIN" stop >/dev/null 2>&1 || true
    kill "$START_PID" 2>/dev/null || true
  fi
  if [[ -f "$DATA_DIR/tray.pid" ]]; then
    kill "$(cat "$DATA_DIR/tray.pid")" 2>/dev/null || true
  fi
  rm -rf "$DATA_DIR"
}
trap cleanup EXIT

export BIFROST_DATA_DIR="$DATA_DIR"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1

echo "Starting bifrost with tray helper on port $PORT..."
"$BIN" start -p "$PORT" --unsafe-ssl --no-system-proxy --skip-cert-check \
  >"$DATA_DIR/start.out" 2>"$DATA_DIR/start.err" &
START_PID="$!"

for _ in {1..120}; do
  if curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/proxy/address" >/dev/null 2>&1 \
    && [[ -s "$DATA_DIR/tray.pid" ]] \
    && compgen -G "$DATA_DIR/logs/tray.log*" >/dev/null; then
    break
  fi
  sleep 0.25
done

curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/proxy/address" >/dev/null
test -s "$DATA_DIR/tray.pid"
TRAY_PID="$(cat "$DATA_DIR/tray.pid")"
kill -0 "$TRAY_PID"

TRAY_LOG="$(ls "$DATA_DIR"/logs/tray.log* | head -n 1)"
grep -q "bifrost-tray starting" "$TRAY_LOG"

sleep 1
if grep -q "icon_interacted=true" "$TRAY_LOG"; then
  echo "FAIL: tray helper rebuilt menu from a pure icon interaction" >&2
  cat "$TRAY_LOG" >&2
  exit 1
fi

echo "PASS: tray helper launched and pure icon interaction rebuild guard is active"
