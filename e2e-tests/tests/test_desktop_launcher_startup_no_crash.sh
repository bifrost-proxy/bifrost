#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "SKIP: desktop launcher startup crash regression only runs on macOS"
  exit 0
fi

if ! pgrep -x WindowServer >/dev/null 2>&1; then
  echo "SKIP: no active macOS WindowServer session"
  exit 0
fi

if [[ "${SKIP_BUILD:-}" != "true" ]]; then
  pnpm --dir web run build:desktop
  cargo build -p bifrost-cli
  node scripts/prepare-tauri-sidecar.mjs debug
  cargo build --manifest-path desktop/src-tauri/Cargo.toml
fi

APP_BIN="${BIFROST_DESKTOP_APP_BIN:-desktop/src-tauri/target/debug/bifrost-desktop}"
if [[ ! -x "$APP_BIN" ]]; then
  if [[ "${SKIP_BUILD:-}" == "true" ]]; then
    echo "SKIP: missing desktop binary at $APP_BIN and SKIP_BUILD=true"
    exit 0
  fi
  echo "FAIL: missing desktop binary at $APP_BIN"
  exit 1
fi

TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-desktop-launcher-crash.XXXXXX")"
APP_LOG="$TEST_DIR/bifrost-desktop.log"

cleanup() {
  if [[ -n "${APP_PID:-}" ]] && kill -0 "$APP_PID" >/dev/null 2>&1; then
    kill "$APP_PID" >/dev/null 2>&1 || true
    sleep 1
    kill -9 "$APP_PID" >/dev/null 2>&1 || true
  fi
  rm -rf "$TEST_DIR"
}
trap cleanup EXIT

export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
export BIFROST_DISABLE_TRAY=1
export BIFROST_DESKTOP_NO_SYSTEM_PROXY=1
export BIFROST_DESKTOP_SKIP_CERT_PREFLIGHT=1
export BIFROST_DATA_DIR="$TEST_DIR/data"
export RUST_BACKTRACE=full

"$APP_BIN" >"$APP_LOG" 2>&1 &
APP_PID=$!

sleep "${DESKTOP_STARTUP_HOLD_SECONDS:-8}"

if ! kill -0 "$APP_PID" >/dev/null 2>&1; then
  wait "$APP_PID" || status=$?
  status="${status:-unknown}"
  echo "FAIL: bifrost-desktop exited during startup hold window, status=$status"
  sed -n '1,220p' "$APP_LOG" || true
  if [[ -f "$BIFROST_DATA_DIR/logs/desktop-bootstrap.log" ]]; then
    echo "--- desktop-bootstrap.log ---"
    tail -120 "$BIFROST_DATA_DIR/logs/desktop-bootstrap.log" || true
  fi
  exit 1
fi

BOOTSTRAP_LOG="$BIFROST_DATA_DIR/logs/desktop-bootstrap.log"
if [[ ! -f "$BOOTSTRAP_LOG" ]]; then
  echo "FAIL: missing desktop bootstrap log at $BOOTSTRAP_LOG"
  sed -n '1,220p' "$APP_LOG" || true
  exit 1
fi

for expected in \
  "desktop setup started" \
  "embedded webview page load event" \
  "starting embedded webview handoff" \
  "embedded webview handoff completed"
do
  if ! grep -Fq "$expected" "$BOOTSTRAP_LOG"; then
    echo "FAIL: desktop bootstrap log did not contain: $expected"
    tail -160 "$BOOTSTRAP_LOG" || true
    exit 1
  fi
done

echo "PASS: bifrost-desktop stayed alive through launcher handoff startup window"
