#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=../test_utils/process.sh
source "$ROOT_DIR/e2e-tests/test_utils/process.sh"

cd "$ROOT_DIR"

case "$(uname -s)" in
  Darwin|MINGW*|MSYS*|CYGWIN*)
    ;;
  *)
    echo "SKIP: tray startup smoke is only required on macOS and Windows"
    exit 0
    ;;
esac

if is_windows; then
  DEFAULT_BIN="$ROOT_DIR/target/release/bifrost.exe"
else
  DEFAULT_BIN="$ROOT_DIR/target/release/bifrost"
fi

BIN="${BIFROST_BIN:-$DEFAULT_BIN}"
DATA_DIR=""
DATA_DIR_CREATED=0
START_PID=""
PORT=""

dump_diagnostics() {
  echo "=== bifrost start stdout ===" >&2
  [[ -f "$DATA_DIR/start.out" ]] && cat "$DATA_DIR/start.out" >&2 || true
  echo "=== bifrost start stderr ===" >&2
  [[ -f "$DATA_DIR/start.err" ]] && cat "$DATA_DIR/start.err" >&2 || true
  echo "=== tray logs ===" >&2
  if compgen -G "$DATA_DIR/logs/tray.log*" >/dev/null; then
    for log in "$DATA_DIR"/logs/tray.log*; do
      echo "--- $log ---" >&2
      cat "$log" >&2 || true
    done
  fi
}

pid_is_alive() {
  local pid="$1"
  if [[ -z "$pid" ]]; then
    return 1
  fi
  if kill -0 "$pid" 2>/dev/null; then
    return 0
  fi
  if is_windows; then
    tasklist.exe /FI "PID eq $pid" /NH 2>/dev/null \
      | tr -d '\r' \
      | awk -v pid="$pid" '$2 == pid { found=1 } END { exit(found ? 0 : 1) }'
  else
    return 1
  fi
}

first_tray_log() {
  compgen -G "$DATA_DIR/logs/tray.log*" >/dev/null || return 1
  ls "$DATA_DIR"/logs/tray.log* | head -n 1
}

tray_log_has_startup_marker() {
  local log
  log="$(first_tray_log 2>/dev/null || true)"
  [[ -n "$log" ]] && grep -q "bifrost-tray starting" "$log"
}

cleanup() {
  if [[ -n "$DATA_DIR" && -x "$BIN" ]]; then
    BIFROST_DATA_DIR="$DATA_DIR" "$BIN" stop >/dev/null 2>&1 || true
  fi
  if [[ -n "$START_PID" ]]; then
    kill_process_tree "$START_PID"
  fi
  if [[ -n "$DATA_DIR" && -f "$DATA_DIR/tray.pid" ]]; then
    kill_pid_force "$(cat "$DATA_DIR/tray.pid" 2>/dev/null || true)"
  fi
  if [[ -n "$PORT" ]]; then
    kill_bifrost_on_port "$PORT"
  fi
  if [[ -n "$DATA_DIR" && "$DATA_DIR_CREATED" == "1" && "${BIFROST_TRAY_STARTUP_KEEP_DATA_DIR:-0}" != "1" ]]; then
    rm -rf "$DATA_DIR"
  fi
}
trap cleanup EXIT

if [[ "${BIFROST_TRAY_STARTUP_SKIP_BUILD:-0}" != "1" || ! -x "$BIN" ]]; then
  echo "Building bifrost binary for tray startup smoke..."
  SKIP_FRONTEND_BUILD=1 cargo build --release --bin bifrost
fi

if [[ ! -x "$BIN" ]]; then
  echo "ERROR: bifrost binary is not executable: $BIN" >&2
  exit 1
fi

if [[ -n "${BIFROST_TRAY_STARTUP_DATA_DIR:-}" ]]; then
  DATA_DIR="$BIFROST_TRAY_STARTUP_DATA_DIR"
  mkdir -p "$DATA_DIR"
else
  DATA_DIR="$(mktemp -d "$ROOT_DIR/.bifrost-e2e-tray-startup.XXXXXX")"
  DATA_DIR_CREATED=1
fi
PORT="$(pick_available_base_port "${BIFROST_TRAY_STARTUP_PORT:-0}" 1)"
if [[ -z "$PORT" || "$PORT" == "0" ]]; then
  echo "ERROR: failed to allocate a free port for tray startup smoke" >&2
  exit 1
fi

export BIFROST_DATA_DIR="$DATA_DIR"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
unset BIFROST_DISABLE_TRAY

echo "Starting bifrost with tray helper on 127.0.0.1:$PORT..."
"$BIN" start -p "$PORT" --unsafe-ssl --no-system-proxy --skip-cert-check \
  >"$DATA_DIR/start.out" 2>"$DATA_DIR/start.err" &
START_PID="$!"

if ! wait_for_http_ready "http://127.0.0.1:$PORT/_bifrost/api/proxy/address" 45 0.25; then
  echo "ERROR: bifrost admin API did not become ready" >&2
  dump_diagnostics
  exit 1
fi

TRAY_PID=""
for _ in $(seq 1 160); do
  if [[ -s "$DATA_DIR/tray.pid" ]]; then
    TRAY_PID="$(cat "$DATA_DIR/tray.pid" | tr -d '[:space:]')"
    if pid_is_alive "$TRAY_PID" && compgen -G "$DATA_DIR/logs/tray.log*" >/dev/null; then
      break
    fi
  fi
  if is_windows && tray_log_has_startup_marker; then
    break
  fi
  if ! pid_is_alive "$START_PID"; then
    echo "ERROR: bifrost start process exited before tray helper became ready" >&2
    dump_diagnostics
    exit 1
  fi
  sleep 0.25
done

TRAY_LOG="$(first_tray_log 2>/dev/null || true)"
if [[ -z "$TRAY_LOG" ]]; then
  echo "ERROR: tray log was not created" >&2
  dump_diagnostics
  exit 1
fi

if [[ -z "$TRAY_PID" || ! -s "$DATA_DIR/tray.pid" ]]; then
  if is_windows && grep -q "bifrost-tray starting" "$TRAY_LOG"; then
    echo "INFO: tray.pid was not created; verified Windows tray helper startup from $TRAY_LOG"
    TRAY_PID="log-only"
  else
    echo "ERROR: tray.pid was not created" >&2
    dump_diagnostics
    exit 1
  fi
fi

if [[ -n "$TRAY_PID" && "$TRAY_PID" != "log-only" ]] && ! pid_is_alive "$TRAY_PID"; then
  echo "ERROR: tray helper process is not alive: $TRAY_PID" >&2
  dump_diagnostics
  exit 1
fi

if ! grep -q "bifrost-tray starting" "$TRAY_LOG"; then
  echo "ERROR: tray log does not contain startup marker" >&2
  dump_diagnostics
  exit 1
fi

curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/proxy/address" >"$DATA_DIR/proxy-address.json"
if ! grep -q "$PORT" "$DATA_DIR/proxy-address.json"; then
  echo "ERROR: proxy address API response does not mention the expected port $PORT" >&2
  cat "$DATA_DIR/proxy-address.json" >&2
  exit 1
fi

if [[ ! -s "$DATA_DIR/runtime.json" ]]; then
  echo "ERROR: runtime.json was not created" >&2
  dump_diagnostics
  exit 1
fi

"$(python3_cmd)" - "$PORT" "$DATA_DIR/runtime.json" <<'PY'
import json
import sys

expected_port = int(sys.argv[1])
runtime_path = sys.argv[2]
with open(runtime_path, "r", encoding="utf-8") as fh:
    runtime = json.load(fh)

actual_port = int(runtime.get("port", 0))
pid = int(runtime.get("pid", 0))
if actual_port != expected_port or pid <= 0:
    raise SystemExit(
        f"runtime.json mismatch: expected port={expected_port}, got port={actual_port}, pid={pid}"
    )
PY

echo "PASS: tray helper started on $(uname -s); port=$PORT tray_pid=$TRAY_PID log=$TRAY_LOG"
