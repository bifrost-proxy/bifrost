#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "SKIP: desktop launcher failure handoff regression only runs on macOS"
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

TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-desktop-startup-failure.XXXXXX")"
APP_LOG="$TEST_DIR/bifrost-desktop.log"
STUB_BIN="$TEST_DIR/bifrost-failing-sidecar"
printf '%s\n' '#!/bin/sh' 'echo "simulated sidecar startup failure" >&2' 'exit 42' >"$STUB_BIN"
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
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
export BIFROST_DISABLE_TRAY=1
export BIFROST_DESKTOP_NO_SYSTEM_PROXY=1
export BIFROST_DESKTOP_SKIP_CERT_PREFLIGHT=1
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

started_at="$(date +%s)"
"$APP_BIN" >"$APP_LOG" 2>&1 &
APP_PID=$!
BOOTSTRAP_LOG="$BIFROST_DATA_DIR/logs/desktop-bootstrap.log"

for _ in $(seq 1 40); do
  if [[ -f "$BOOTSTRAP_LOG" ]] && grep -Fq "embedded webview handoff completed" "$BOOTSTRAP_LOG"; then
    break
  fi
  if ! kill -0 "$APP_PID" >/dev/null 2>&1; then
    echo "FAIL: desktop app exited instead of exposing the startup failure UI"
    sed -n '1,220p' "$APP_LOG" || true
    exit 1
  fi
  sleep 0.25
done

elapsed="$(( $(date +%s) - started_at ))"
if [[ ! -f "$BOOTSTRAP_LOG" ]]; then
  echo "FAIL: missing desktop bootstrap log"
  exit 1
fi

for expected in \
  "exited before becoming ready" \
  "desktop backend bootstrap failed" \
  "starting embedded webview handoff" \
  "embedded webview handoff completed"
do
  if ! grep -Fq "$expected" "$BOOTSTRAP_LOG"; then
    echo "FAIL: desktop bootstrap log did not contain: $expected"
    tail -160 "$BOOTSTRAP_LOG" || true
    exit 1
  fi
done

if (( elapsed >= 10 )); then
  echo "FAIL: startup failure handoff took ${elapsed}s; expected a fast failure"
  tail -160 "$BOOTSTRAP_LOG" || true
  exit 1
fi

if ! grep -Fq "simulated sidecar startup failure" "$BIFROST_DATA_DIR/logs/desktop-sidecar.err.log"; then
  echo "FAIL: sidecar stderr did not retain the startup failure"
  exit 1
fi

echo "PASS: sidecar failure became visible through launcher handoff in ${elapsed}s"
