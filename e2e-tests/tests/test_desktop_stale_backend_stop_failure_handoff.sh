#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "SKIP: desktop stale-backend stop regression only runs on macOS"
  exit 0
fi
if ! pgrep -x WindowServer >/dev/null 2>&1; then
  echo "SKIP: no active macOS WindowServer session"
  exit 0
fi

if [[ "${SKIP_BUILD:-}" != "true" ]]; then
  pnpm --dir web run build:desktop
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

TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-desktop-stale-stop.XXXXXX")"
CALLS_LOG="$TEST_DIR/sidecar-calls.log"
STUB_BIN="$TEST_DIR/bifrost-stop-failure-sidecar"
cat >"$STUB_BIN" <<'STUB'
#!/bin/sh
printf '%s\n' "$*" >>"$BIFROST_TEST_SIDECAR_CALLS"
if [ "${1:-}" = "stop" ]; then
  echo "simulated stale backend stop failure" >&2
  exit 17
fi
echo "unexpected second backend start" >&2
exit 42
STUB
chmod +x "$STUB_BIN"

cleanup() {
  if [[ -n "${APP_PID:-}" ]] && kill -0 "$APP_PID" >/dev/null 2>&1; then
    kill "$APP_PID" >/dev/null 2>&1 || true
    sleep 1
    kill -9 "$APP_PID" >/dev/null 2>&1 || true
  fi
  rm -rf "$TEST_DIR"
}
trap cleanup EXIT

export BIFROST_DESKTOP_BIN="$STUB_BIN"
export BIFROST_DESKTOP_TEST_ALLOW_MULTIPLE_INSTANCES=1
export BIFROST_TEST_SIDECAR_CALLS="$CALLS_LOG"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
export BIFROST_DISABLE_TRAY=1
export BIFROST_DESKTOP_NO_SYSTEM_PROXY=1
export BIFROST_DESKTOP_SKIP_CERT_PREFLIGHT=1
export BIFROST_DATA_DIR="$TEST_DIR/data"
mkdir -p "$BIFROST_DATA_DIR"
printf '%s\n' '{"pid":999999,"port":59999}' >"$BIFROST_DATA_DIR/runtime.json"
printf '%s\n' '{"proxy_port":59999}' >"$BIFROST_DATA_DIR/desktop-config.json"

"$APP_BIN" >"$TEST_DIR/bifrost-desktop.log" 2>&1 &
APP_PID=$!
BOOTSTRAP_LOG="$BIFROST_DATA_DIR/logs/desktop-bootstrap.log"

for _ in $(seq 1 40); do
  if [[ -f "$BOOTSTRAP_LOG" ]] && grep -Fq "embedded webview handoff completed" "$BOOTSTRAP_LOG"; then
    break
  fi
  if ! kill -0 "$APP_PID" >/dev/null 2>&1; then
    echo "FAIL: desktop app exited instead of exposing the stale-stop error"
    exit 1
  fi
  sleep 0.25
done

for expected in \
  "stale backend stop failed; refusing to start a second backend" \
  "desktop backend bootstrap failed" \
  "embedded webview handoff completed"
do
  if [[ ! -f "$BOOTSTRAP_LOG" ]] || ! grep -Fq "$expected" "$BOOTSTRAP_LOG"; then
    echo "FAIL: desktop bootstrap log did not contain: $expected"
    tail -180 "$BOOTSTRAP_LOG" 2>/dev/null || true
    exit 1
  fi
done

if [[ "$(wc -l <"$CALLS_LOG" | tr -d ' ')" != "1" ]] || ! grep -Fxq "stop" "$CALLS_LOG"; then
  echo "FAIL: desktop attempted to start another backend after stale stop failed"
  cat "$CALLS_LOG" 2>/dev/null || true
  exit 1
fi

if ! kill -0 "$APP_PID" >/dev/null 2>&1; then
  echo "FAIL: desktop app did not remain available after stale-stop failure handoff"
  exit 1
fi

echo "PASS: stale backend stop failure blocked a second backend and exposed recovery UI"
