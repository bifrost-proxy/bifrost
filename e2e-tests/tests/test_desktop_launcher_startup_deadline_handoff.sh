#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "SKIP: desktop launcher deadline regression only runs on macOS"
  exit 0
fi
if ! pgrep -x WindowServer >/dev/null 2>&1; then
  echo "SKIP: no active macOS WindowServer session"
  exit 0
fi

APP_BIN="${BIFROST_DESKTOP_APP_BIN:-desktop/src-tauri/target/debug/bifrost-desktop}"
if [[ ! -x "$APP_BIN" ]]; then
  echo "SKIP: missing desktop binary at $APP_BIN"
  exit 0
fi

TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-desktop-startup-deadline.XXXXXX")"
STUB_BIN="$TEST_DIR/bifrost-hanging-sidecar"
printf '%s\n' \
  '#!/bin/sh' \
  'echo "simulated hanging sidecar; session_id=${BIFROST_DESKTOP_STARTUP_SESSION_ID:-missing}" >&2' \
  'sleep 30' >"$STUB_BIN"
chmod +x "$STUB_BIN"

cleanup() {
  pkill -f "$STUB_BIN" >/dev/null 2>&1 || true
  if [[ -n "${APP_PID:-}" ]] && kill -0 "$APP_PID" >/dev/null 2>&1; then
    kill "$APP_PID" >/dev/null 2>&1 || true
    sleep 1
    kill -9 "$APP_PID" >/dev/null 2>&1 || true
  fi
  rm -rf "$TEST_DIR"
}
trap cleanup EXIT

export BIFROST_DESKTOP_BIN="$STUB_BIN"
export BIFROST_DESKTOP_STARTUP_DEADLINE_MS=1500
export BIFROST_DESKTOP_NO_SYSTEM_PROXY=1
export BIFROST_DESKTOP_SKIP_CERT_PREFLIGHT=1
export BIFROST_DISABLE_TRAY=1
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
export BIFROST_DESKTOP_TEST_ALLOW_MULTIPLE_INSTANCES=1
export BIFROST_DATA_DIR="$TEST_DIR/data"
mkdir -p "$BIFROST_DATA_DIR"
TEST_PORT="$(/usr/bin/python3 <<'PY'
import socket

for base in range(49152, 65471, 65):
    sockets = []
    try:
        for port in range(base, base + 65):
            sock = socket.socket()
            sock.bind(("0.0.0.0", port))
            sockets.append(sock)
        print(base)
        break
    except OSError:
        pass
    finally:
        for sock in sockets:
            sock.close()
else:
    raise SystemExit("no contiguous 65-port block is available")
PY
)"
printf '{"proxy_port":%s}\n' "$TEST_PORT" >"$BIFROST_DATA_DIR/desktop-config.json"

"$APP_BIN" >"$TEST_DIR/bifrost-desktop.log" 2>&1 &
APP_PID=$!
BOOTSTRAP_LOG="$BIFROST_DATA_DIR/logs/desktop-bootstrap.log"

for _ in $(seq 1 28); do
  if [[ -f "$BOOTSTRAP_LOG" ]] && grep -Fq "embedded webview handoff completed" "$BOOTSTRAP_LOG"; then
    break
  fi
  if ! kill -0 "$APP_PID" >/dev/null 2>&1; then
    echo "FAIL: desktop app exited while the startup deadline was recovering the launcher"
    exit 1
  fi
  sleep 0.25
done

SESSION_ID="$(sed -n 's/.*desktop startup session started; session_id=\([^ ]*\).*/\1/p' "$BOOTSTRAP_LOG" | tail -1)"
if [[ -z "$SESSION_ID" ]]; then
  echo "FAIL: desktop bootstrap log did not expose a startup session id"
  tail -180 "$BOOTSTRAP_LOG" 2>/dev/null || true
  exit 1
fi

for expected in \
  "desktop setup started; session_id=$SESSION_ID" \
  "starting sidecar; session_id=$SESSION_ID" \
  "sidecar spawned; session_id=$SESSION_ID" \
  "desktop startup deadline exceeded after 1500ms" \
  "desktop startup deadline recorded a recoverable startup error" \
  "starting embedded webview handoff; reason=desktop startup deadline" \
  "embedded webview handoff completed"
do
  if [[ ! -f "$BOOTSTRAP_LOG" ]] || ! grep -Fq "$expected" "$BOOTSTRAP_LOG"; then
    echo "FAIL: desktop bootstrap log did not contain: $expected"
    tail -180 "$BOOTSTRAP_LOG" 2>/dev/null || true
    exit 1
  fi
done

if ! grep -Fq "simulated hanging sidecar; session_id=$SESSION_ID" "$BIFROST_DATA_DIR/logs/desktop-sidecar.err.log"; then
  echo "FAIL: hanging sidecar stderr did not retain the startup session id"
  exit 1
fi

if ! kill -0 "$APP_PID" >/dev/null 2>&1; then
  echo "FAIL: desktop app did not remain available after launcher deadline handoff"
  exit 1
fi

echo "PASS: launcher deadline exposed a recoverable startup error instead of hanging indefinitely"
