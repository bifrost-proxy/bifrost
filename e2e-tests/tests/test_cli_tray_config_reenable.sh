#!/usr/bin/env bash
set -euo pipefail

: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

if [[ "${BIFROST_E2E_ALLOW_TRAY:-0}" != "1" ]]; then
  echo "SKIP: tray desktop helper tests are disabled by default; set BIFROST_E2E_ALLOW_TRAY=1 to run"
  exit 0
fi

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "SKIP: tray config re-enable regression is macOS-only"
  exit 0
fi

if [[ "${SKIP_BUILD:-false}" == "true" || "${BIFROST_TRAY_CONFIG_SKIP_UNIT_GUARD:-}" == "1" ]]; then
  echo "Skipping tray config unit guard (SKIP_BUILD=${SKIP_BUILD:-false})"
else
  echo "Running tray config unit guard..."
  cargo test -p bifrost-admin tray_launch -- --nocapture
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

DATA_DIR="$(mktemp -d "$ROOT_DIR/.bifrost-e2e-tray-reenable.XXXXXX")"
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
export BIFROST_DISABLE_TRAY=0

echo "Starting bifrost with tray helper on port $PORT..."
"$BIN" start -p "$PORT" --unsafe-ssl --no-system-proxy --skip-cert-check --yes \
  >"$DATA_DIR/start.out" 2>"$DATA_DIR/start.err" &
START_PID="$!"

for _ in {1..120}; do
  if curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/config/tray" >/dev/null 2>&1 \
    && [[ -s "$DATA_DIR/tray.pid" ]]; then
    break
  fi
  sleep 0.25
done

curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/config/tray" >/dev/null
test -s "$DATA_DIR/tray.pid"
TRAY_PID_BEFORE="$(cat "$DATA_DIR/tray.pid")"
kill -0 "$TRAY_PID_BEFORE"

curl -fsS -X PUT "http://127.0.0.1:$PORT/_bifrost/api/config/tray" \
  -H 'Content-Type: application/json' \
  -d '{"enabled":false}' >/dev/null

for _ in {1..120}; do
  if [[ ! -e "$DATA_DIR/tray.pid" ]]; then
    break
  fi
  sleep 0.25
done
if [[ -e "$DATA_DIR/tray.pid" ]]; then
  echo "FAIL: tray.pid was not removed after disabling tray config" >&2
  cat "$DATA_DIR"/logs/tray.log* >&2 || true
  exit 1
fi

curl -fsS -X PUT "http://127.0.0.1:$PORT/_bifrost/api/config/tray" \
  -H 'Content-Type: application/json' \
  -d '{"enabled":true}' >/dev/null

for _ in {1..120}; do
  if [[ -s "$DATA_DIR/tray.pid" ]]; then
    TRAY_PID_AFTER="$(cat "$DATA_DIR/tray.pid")"
    if [[ "$TRAY_PID_AFTER" != "$TRAY_PID_BEFORE" ]] \
      && kill -0 "$TRAY_PID_AFTER" 2>/dev/null \
      && ! ps -p "$TRAY_PID_AFTER" -o stat= | grep -q Z; then
      echo "PASS: tray helper relaunched after config enable (before=$TRAY_PID_BEFORE after=$TRAY_PID_AFTER)"
      exit 0
    fi
  fi
  sleep 0.25
done

echo "FAIL: tray helper did not relaunch after enabling tray config" >&2
echo "before=$TRAY_PID_BEFORE after=${TRAY_PID_AFTER:-missing}" >&2
sed -n '/\[tray\]/,+2p' "$DATA_DIR/config.toml" >&2 || true
cat "$DATA_DIR"/logs/tray.log* >&2 || true
cat "$DATA_DIR/start.out" >&2 || true
cat "$DATA_DIR/start.err" >&2 || true
exit 1
