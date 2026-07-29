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
FOREIGN_CORE_PID=""

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

free_consecutive_ports() {
  /usr/bin/python3 <<'PY'
import socket

for _ in range(200):
    first = socket.socket()
    first.bind(("127.0.0.1", 0))
    port = first.getsockname()[1]
    if port >= 65535:
        first.close()
        continue
    second = socket.socket()
    try:
        second.bind(("127.0.0.1", port + 1))
    except OSError:
        first.close()
        second.close()
        continue
    first.close()
    second.close()
    print(port)
    raise SystemExit(0)
raise SystemExit("could not allocate consecutive ports")
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
    kill -CONT "$CORE_PID" >/dev/null 2>&1 || true
    kill "$CORE_PID" >/dev/null 2>&1 || true
    wait_for_process_exit "$CORE_PID" || kill -9 "$CORE_PID" >/dev/null 2>&1 || true
  fi
  if [[ -n "$FOREIGN_CORE_PID" ]] && process_is_running "$FOREIGN_CORE_PID"; then
    kill "$FOREIGN_CORE_PID" >/dev/null 2>&1 || true
    wait_for_process_exit "$FOREIGN_CORE_PID" ||
      kill -9 "$FOREIGN_CORE_PID" >/dev/null 2>&1 || true
  fi
  rm -rf "$TEST_ROOT"
}
trap cleanup EXIT

runtime_pid() {
  /usr/bin/python3 -c \
    'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' \
    "$1/runtime.json"
}

wait_for_runtime_pid_change() {
  local data_dir="$1"
  local port="$2"
  local old_pid="$3"
  for _ in $(seq 1 200); do
    if [[ -f "$data_dir/runtime.json" ]]; then
      local new_pid
      new_pid="$(runtime_pid "$data_dir" 2>/dev/null || true)"
      if [[ -n "$new_pid" ]] && [[ "$new_pid" != "$old_pid" ]] &&
        process_is_running "$new_pid" &&
        curl -fsS --max-time 1 \
          "http://127.0.0.1:$port/_bifrost/api/proxy/system/support" >/dev/null 2>&1
      then
        printf '%s\n' "$new_pid"
        return 0
      fi
    fi
    if [[ -n "$APP_PID" ]] && ! process_is_running "$APP_PID"; then
      return 1
    fi
    sleep 0.1
  done
  return 1
}

wait_for_log_line() {
  local log_file="$1"
  local pattern="$2"
  for _ in $(seq 1 300); do
    if [[ -f "$log_file" ]] && grep -Fq "$pattern" "$log_file"; then
      return 0
    fi
    sleep 0.1
  done
  return 1
}

assert_owned_shutdown_log_order() {
  local log_file="$1"
  /usr/bin/python3 - "$log_file" <<'PY'
import pathlib
import sys

lines = pathlib.Path(sys.argv[1]).read_text().splitlines()
stop_index = next(
    i
    for i, line in enumerate(lines)
    if "backend stop helper completed successfully; owned backend and tray are stopped" in line
)
exit_index = next(
    i
    for i, line in enumerate(lines)
    if "desktop lifecycle group shutdown complete; requesting final app exit" in line
)
if stop_index >= exit_index:
    raise SystemExit("Desktop final exit was logged before owned backend/tray shutdown")
PY
}

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

foreign_data_dir="$TEST_ROOT/foreign-data-dir"
isolated_desktop_data_dir="$TEST_ROOT/isolated-desktop"
isolated_desktop_port="$(free_consecutive_ports)"
foreign_port="$((isolated_desktop_port + 1))"
mkdir -p "$foreign_data_dir"
env -u BIFROST_DETACHED_DAEMON_CHILD \
  BIFROST_DATA_DIR="$foreign_data_dir" \
  BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
  BIFROST_DISABLE_TRAY=1 \
  "$CLI_BIN" start \
    --host 127.0.0.1 \
    --port "$foreign_port" \
    --daemon \
    --skip-cert-check \
    --no-system-proxy \
    --no-intercept >"$TEST_ROOT/foreign-start.log" 2>&1
if ! wait_for_runtime "$foreign_data_dir" "$foreign_port"; then
  echo "FAIL: foreign data-dir Service did not become ready"
  cat "$TEST_ROOT/foreign-start.log" || true
  exit 1
fi
FOREIGN_CORE_PID="$(/usr/bin/python3 -c \
  'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' \
  "$foreign_data_dir/runtime.json")"

launch_desktop \
  "$isolated_desktop_data_dir" \
  "$isolated_desktop_port" \
  "$TEST_ROOT/isolated-desktop-app.log" \
  BIFROST_DETACHED_DAEMON_CHILD=1
if ! wait_for_desktop_startup "$isolated_desktop_data_dir"; then
  echo "FAIL: Desktop app did not start beside the foreign data-dir Service"
  cat "$TEST_ROOT/isolated-desktop-app.log" || true
  exit 1
fi
if ! wait_for_runtime "$isolated_desktop_data_dir" "$isolated_desktop_port"; then
  echo "FAIL: Desktop did not start its own Service instead of reusing the foreign data-dir"
  cat "$TEST_ROOT/isolated-desktop-app.log" || true
  cat "$isolated_desktop_data_dir/logs/desktop-bootstrap.log" || true
  exit 1
fi
CORE_PID="$(/usr/bin/python3 -c \
  'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' \
  "$isolated_desktop_data_dir/runtime.json")"
if [[ "$CORE_PID" == "$FOREIGN_CORE_PID" ]]; then
  echo "FAIL: Desktop reused the foreign data-dir Service"
  exit 1
fi
if ! request_desktop_shutdown "$isolated_desktop_data_dir" "$APP_PID"; then
  echo "FAIL: isolated Desktop app did not exit"
  exit 1
fi
APP_PID=""
if ! wait_for_process_exit "$CORE_PID"; then
  echo "FAIL: isolated Desktop-owned Service remained running after Desktop quit"
  exit 1
fi
CORE_PID=""
if ! process_is_running "$FOREIGN_CORE_PID"; then
  echo "FAIL: isolated Desktop shutdown stopped the foreign data-dir Service"
  exit 1
fi
BIFROST_DATA_DIR="$foreign_data_dir" "$CLI_BIN" stop >/dev/null
wait_for_process_exit "$FOREIGN_CORE_PID"
FOREIGN_CORE_PID=""

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

desktop_bootstrap_log="$desktop_data_dir/logs/desktop-bootstrap.log"
if ! wait_for_log_line \
  "$desktop_bootstrap_log" \
  "desktop backend start succeeded"; then
  echo "FAIL: Desktop-owned Service startup did not leave the recovery gate"
  cat "$desktop_bootstrap_log" || true
  exit 1
fi
kill -STOP "$CORE_PID"
sleep 12
if ! process_is_running "$CORE_PID"; then
  echo "FAIL: short backend stall terminated the Desktop-owned Service"
  exit 1
fi
if ! grep -Fq "desktop backend health degraded" "$desktop_bootstrap_log"; then
  kill -CONT "$CORE_PID" >/dev/null 2>&1 || true
  echo "FAIL: short backend stall did not exercise the degraded watchdog path"
  cat "$desktop_bootstrap_log" || true
  exit 1
fi
kill -CONT "$CORE_PID"
if ! wait_for_runtime "$desktop_data_dir" "$desktop_port"; then
  echo "FAIL: Desktop-owned Service did not recover after a short stall"
  cat "$desktop_bootstrap_log" || true
  exit 1
fi
runtime_pid_after_stall="$(runtime_pid "$desktop_data_dir")"
if [[ "$runtime_pid_after_stall" != "$CORE_PID" ]]; then
  echo "FAIL: Desktop watchdog restarted Service after a short stall"
  cat "$desktop_bootstrap_log" || true
  exit 1
fi
if ! wait_for_log_line \
  "$desktop_bootstrap_log" \
  "desktop backend health recovered without restart"; then
  echo "FAIL: Desktop watchdog did not record non-restarting health recovery"
  cat "$desktop_bootstrap_log" || true
  exit 1
fi

exited_core_pid="$CORE_PID"
kill -9 "$exited_core_pid"
if ! wait_for_process_exit "$exited_core_pid"; then
  echo "FAIL: could not terminate Desktop-owned Service for watchdog recovery test"
  exit 1
fi
if ! CORE_PID="$(wait_for_runtime_pid_change \
  "$desktop_data_dir" "$desktop_port" "$exited_core_pid")"; then
  echo "FAIL: Desktop watchdog did not replace an exited owned Service"
  cat "$desktop_bootstrap_log" || true
  exit 1
fi
if ! grep -Fq \
  "managed backend child pid=$exited_core_pid exited" \
  "$desktop_bootstrap_log"; then
  echo "FAIL: Desktop watchdog did not record the real child exit"
  cat "$desktop_bootstrap_log" || true
  exit 1
fi

set +e
desktop_stop_output="$(
  BIFROST_DATA_DIR="$desktop_data_dir" "$CLI_BIN" stop 2>&1
)"
desktop_stop_status=$?
desktop_restart_output="$(
  BIFROST_DATA_DIR="$desktop_data_dir" "$CLI_BIN" restart 2>&1
)"
desktop_restart_status=$?
set -e
if [[ "$desktop_stop_status" -eq 0 ]] ||
  [[ "$desktop_stop_output" != *"owned by the Desktop app"* ]]; then
  echo "FAIL: CLI stop did not reject the Desktop-owned Service"
  printf '%s\n' "$desktop_stop_output"
  exit 1
fi
if [[ "$desktop_restart_status" -eq 0 ]] ||
  [[ "$desktop_restart_output" != *"owned by the Desktop app"* ]]; then
  echo "FAIL: CLI restart did not reject the Desktop-owned Service"
  printf '%s\n' "$desktop_restart_output"
  exit 1
fi
sleep 1
runtime_pid_after_cli_commands="$(/usr/bin/python3 -c \
  'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' \
  "$desktop_data_dir/runtime.json")"
runtime_mode_after_cli_commands="$(/usr/bin/python3 -c \
  'import json,sys; print(json.load(open(sys.argv[1]))["runtime_start_mode"])' \
  "$desktop_data_dir/runtime.json")"
if [[ "$runtime_pid_after_cli_commands" != "$CORE_PID" ]] ||
  [[ "$runtime_mode_after_cli_commands" != "desktop" ]] ||
  ! process_is_running "$CORE_PID"; then
  echo "FAIL: CLI stop/restart changed the Desktop-owned Service"
  cat "$desktop_data_dir/runtime.json"
  exit 1
fi

if ! request_desktop_shutdown "$desktop_data_dir" "$APP_PID"; then
  echo "FAIL: Desktop app did not exit after graceful shutdown request"
  exit 1
fi
APP_PID=""
if ! assert_owned_shutdown_log_order "$desktop_bootstrap_log"; then
  echo "FAIL: Desktop did not stop its owned lifecycle group before final App exit"
  cat "$desktop_bootstrap_log" || true
  exit 1
fi
if ! wait_for_process_exit "$CORE_PID"; then
  echo "FAIL: Desktop-owned Service remained running after Desktop quit"
  exit 1
fi
sleep 3
if process_is_running "$CORE_PID" ||
  curl -fsS --max-time 2 \
    "http://127.0.0.1:$desktop_port/_bifrost/api/proxy/system/support" >/dev/null 2>&1
then
  echo "FAIL: Desktop watchdog restarted the owned Service after Desktop quit"
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

echo "PASS: Desktop ownership stays scoped by data-dir, transient stalls preserve PID, real exits recover, and CLI lifecycle commands preserve App ownership"
