#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "SKIP: desktop service ownership lifecycle only runs on macOS"
  exit 0
fi

if ! pgrep -x WindowServer >/dev/null 2>&1; then
  echo "SKIP: no active macOS WindowServer session"
  exit 0
fi

if pgrep -f '[b]ifrost-desktop([[:space:]]|$)' >/dev/null 2>&1; then
  echo "SKIP: an existing Bifrost Desktop process is running"
  exit 0
fi

if [[ "${SKIP_BUILD:-}" != "true" ]]; then
  pnpm --dir web run build:desktop
  SKIP_FRONTEND_BUILD=1 cargo build -p bifrost-cli --bin bifrost
  node scripts/prepare-tauri-sidecar.mjs debug
  SKIP_FRONTEND_BUILD=1 cargo build --manifest-path desktop/src-tauri/Cargo.toml
fi

APP_BIN="${BIFROST_DESKTOP_APP_BIN:-desktop/src-tauri/target/debug/bifrost-desktop}"
CLI_BIN="${BIFROST_CLI_BIN:-target/debug/bifrost}"
if [[ ! -x "$APP_BIN" || ! -x "$CLI_BIN" ]]; then
  echo "FAIL: missing debug desktop or CLI binary"
  exit 1
fi

TEST_ROOT="$(mktemp -d "$ROOT_DIR/.bifrost-e2e-desktop-service-ownership.XXXXXX")"
APP_PID=""
CORE_PID=""
REQUESTER_PID=""

process_is_running() {
  kill -0 "$1" >/dev/null 2>&1
}

wait_for_process_exit() {
  local pid="$1"
  for _ in $(seq 1 150); do
    if ! process_is_running "$pid"; then
      return 0
    fi
    sleep 0.1
  done
  return 1
}

free_port() {
  /usr/bin/python3 <<'PY'
import socket

sock = socket.socket()
sock.bind(("127.0.0.1", 0))
print(sock.getsockname()[1])
sock.close()
PY
}

wait_for_runtime() {
  local data_dir="$1"
  local port="$2"
  for _ in $(seq 1 200); do
    if [[ -f "$data_dir/runtime.json" ]] &&
      curl -fsS --max-time 1 \
        "http://127.0.0.1:$port/_bifrost/api/proxy/system/support" >/dev/null 2>&1
    then
      return 0
    fi
    if [[ -n "$APP_PID" ]] && ! process_is_running "$APP_PID"; then
      return 1
    fi
    sleep 0.1
  done
  return 1
}

wait_for_desktop_startup() {
  local data_dir="$1"
  for _ in $(seq 1 150); do
    if [[ -f "$data_dir/logs/desktop-bootstrap.log" ]] &&
      grep -Fq "desktop startup session started" \
        "$data_dir/logs/desktop-bootstrap.log"
    then
      return 0
    fi
    if [[ -n "$APP_PID" ]] && ! process_is_running "$APP_PID"; then
      return 1
    fi
    sleep 0.1
  done
  return 1
}

request_desktop_shutdown() {
  local data_dir="$1"
  local pid="$2"
  BIFROST_DATA_DIR="$data_dir" \
    BIFROST_DESKTOP_NO_SYSTEM_PROXY=1 \
    BIFROST_DESKTOP_SKIP_CERT_PREFLIGHT=1 \
    BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
    BIFROST_DISABLE_TRAY=1 \
    "$APP_BIN" --bifrost-upgrade-shutdown \
      >"$data_dir/shutdown-request.log" 2>&1 &
  REQUESTER_PID=$!
  if ! wait_for_process_exit "$REQUESTER_PID"; then
    return 1
  fi
  REQUESTER_PID=""
  wait_for_process_exit "$pid"
}

cleanup() {
  if [[ -n "$REQUESTER_PID" ]] && process_is_running "$REQUESTER_PID"; then
    kill "$REQUESTER_PID" >/dev/null 2>&1 || true
    wait_for_process_exit "$REQUESTER_PID" ||
      kill -9 "$REQUESTER_PID" >/dev/null 2>&1 || true
  fi
  if [[ -n "$APP_PID" ]] && process_is_running "$APP_PID"; then
    kill "$APP_PID" >/dev/null 2>&1 || true
    wait_for_process_exit "$APP_PID" || kill -9 "$APP_PID" >/dev/null 2>&1 || true
  fi
  if [[ -n "$CORE_PID" ]] && process_is_running "$CORE_PID"; then
    kill "$CORE_PID" >/dev/null 2>&1 || true
    wait_for_process_exit "$CORE_PID" || kill -9 "$CORE_PID" >/dev/null 2>&1 || true
  fi
  rm -rf "$TEST_ROOT"
}
trap cleanup EXIT

launch_desktop() {
  local data_dir="$1"
  local port="$2"
  local app_log="$3"
  shift 3

  mkdir -p "$data_dir"
  printf '{"proxy_port":%s}\n' "$port" >"$data_dir/desktop-config.json"
  env \
    BIFROST_DATA_DIR="$data_dir" \
    BIFROST_DESKTOP_NO_SYSTEM_PROXY=1 \
    BIFROST_DESKTOP_SKIP_CERT_PREFLIGHT=1 \
    BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
    BIFROST_DISABLE_TRAY=1 \
    "$@" \
    "$APP_BIN" >"$app_log" 2>&1 &
  APP_PID=$!
}

desktop_data_dir="$TEST_ROOT/desktop-owned"
desktop_port="$(free_port)"
launch_desktop \
  "$desktop_data_dir" \
  "$desktop_port" \
  "$TEST_ROOT/desktop-owned-app.log" \
  BIFROST_DETACHED_DAEMON_CHILD=1

if ! wait_for_desktop_startup "$desktop_data_dir"; then
  echo "FAIL: Desktop app did not start in the Desktop-owned scenario"
  cat "$TEST_ROOT/desktop-owned-app.log" || true
  exit 1
fi
if ! wait_for_runtime "$desktop_data_dir" "$desktop_port"; then
  echo "FAIL: desktop-owned Service did not become ready"
  cat "$TEST_ROOT/desktop-owned-app.log" || true
  exit 1
fi

desktop_mode="$(/usr/bin/python3 -c \
  'import json,sys; print(json.load(open(sys.argv[1]))["runtime_start_mode"])' \
  "$desktop_data_dir/runtime.json")"
CORE_PID="$(/usr/bin/python3 -c \
  'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' \
  "$desktop_data_dir/runtime.json")"
if [[ "$desktop_mode" != "desktop" ]]; then
  echo "FAIL: inherited daemon marker changed Desktop Service owner to $desktop_mode"
  cat "$desktop_data_dir/runtime.json"
  exit 1
fi

if ! request_desktop_shutdown "$desktop_data_dir" "$APP_PID"; then
  echo "FAIL: Desktop app did not exit after graceful shutdown request"
  exit 1
fi
APP_PID=""
if ! wait_for_process_exit "$CORE_PID"; then
  echo "FAIL: Desktop-owned Service remained running after Desktop quit"
  exit 1
fi
if ! grep -Fq \
  "desktop shutdown owns the active backend; requesting backend stop" \
  "$desktop_data_dir/logs/desktop-bootstrap.log"; then
  echo "FAIL: Desktop did not record the owned-backend stop decision"
  cat "$desktop_data_dir/logs/desktop-bootstrap.log" || true
  exit 1
fi
CORE_PID=""

cli_data_dir="$TEST_ROOT/cli-owned"
cli_port="$(free_port)"
mkdir -p "$cli_data_dir"
env -u BIFROST_DETACHED_DAEMON_CHILD \
  BIFROST_DATA_DIR="$cli_data_dir" \
  BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
  BIFROST_DISABLE_TRAY=1 \
  "$CLI_BIN" start \
    --host 127.0.0.1 \
    --port "$cli_port" \
    --daemon \
    --skip-cert-check \
    --no-system-proxy \
    --no-intercept >"$TEST_ROOT/cli-start.log" 2>&1

if ! wait_for_runtime "$cli_data_dir" "$cli_port"; then
  echo "FAIL: CLI-owned Service did not become ready"
  cat "$TEST_ROOT/cli-start.log" || true
  exit 1
fi
cli_mode="$(/usr/bin/python3 -c \
  'import json,sys; print(json.load(open(sys.argv[1]))["runtime_start_mode"])' \
  "$cli_data_dir/runtime.json")"
CORE_PID="$(/usr/bin/python3 -c \
  'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' \
  "$cli_data_dir/runtime.json")"
if [[ "$cli_mode" != "daemon" ]]; then
  echo "FAIL: CLI Service owner is $cli_mode instead of daemon"
  exit 1
fi

launch_desktop \
  "$cli_data_dir" \
  "$cli_port" \
  "$TEST_ROOT/cli-owned-app.log" \
  BIFROST_DETACHED_DAEMON_CHILD=1
if ! wait_for_desktop_startup "$cli_data_dir"; then
  echo "FAIL: Desktop app did not start in the CLI-owned scenario"
  cat "$TEST_ROOT/cli-owned-app.log" || true
  exit 1
fi
if ! wait_for_runtime "$cli_data_dir" "$cli_port"; then
  echo "FAIL: Desktop did not reuse the CLI-owned Service"
  cat "$TEST_ROOT/cli-owned-app.log" || true
  exit 1
fi

if ! request_desktop_shutdown "$cli_data_dir" "$APP_PID"; then
  echo "FAIL: Desktop app reusing CLI Service did not exit after graceful shutdown request"
  exit 1
fi
APP_PID=""
if ! process_is_running "$CORE_PID" ||
  ! curl -fsS --max-time 2 \
    "http://127.0.0.1:$cli_port/_bifrost/api/proxy/system/support" >/dev/null
then
  echo "FAIL: Desktop quit stopped the CLI-owned Service"
  exit 1
fi
if ! grep -Fq \
  "desktop shutdown is preserving the external CLI-owned backend" \
  "$cli_data_dir/logs/desktop-bootstrap.log"; then
  echo "FAIL: Desktop did not record the external-backend preserve decision"
  cat "$cli_data_dir/logs/desktop-bootstrap.log" || true
  exit 1
fi

BIFROST_DATA_DIR="$cli_data_dir" "$CLI_BIN" stop >/dev/null
wait_for_process_exit "$CORE_PID"
CORE_PID=""

echo "PASS: Desktop-owned Service exits with Desktop while CLI-owned Service is preserved"
