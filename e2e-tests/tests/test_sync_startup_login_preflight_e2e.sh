#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/debug/bifrost}"
TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-sync-startup-preflight.XXXXXX")"
MOCK_PORT_FILE="$TEST_DIR/mock-port.txt"
MOCK_LOG="$TEST_DIR/mock.log"
MOCK_PID=""
BIFROST_PID=""

cleanup() {
  if [[ -n "$BIFROST_PID" ]] && kill -0 "$BIFROST_PID" 2>/dev/null; then
    kill "$BIFROST_PID" 2>/dev/null || true
    wait "$BIFROST_PID" 2>/dev/null || true
  fi
  if [[ -n "$MOCK_PID" ]] && kill -0 "$MOCK_PID" 2>/dev/null; then
    kill "$MOCK_PID" 2>/dev/null || true
    wait "$MOCK_PID" 2>/dev/null || true
  fi
  rm -rf "$TEST_DIR"
}
trap cleanup EXIT

fail() {
  echo "[sync-startup-preflight] error: $*" >&2
  find "$TEST_DIR" -maxdepth 3 -name 'bifrost.log' -print -exec tail -n 120 {} \; >&2 || true
  [[ -f "$MOCK_LOG" ]] && tail -n 120 "$MOCK_LOG" >&2 || true
  exit 1
}

wait_file_contains() {
  local file="$1"
  local pattern="$2"
  for _ in {1..80}; do
    if [[ -f "$file" ]] && grep -q "$pattern" "$file"; then
      return 0
    fi
    sleep 0.25
  done
  fail "timed out waiting for $pattern in $file"
}

wait_http() {
  local url="$1"
  for _ in {1..80}; do
    if curl -fsS --noproxy '*' "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.25
  done
  fail "timed out waiting for $url"
}

open_count() {
  local file="$1"
  if [[ ! -f "$file" ]]; then
    echo 0
    return
  fi
  grep -c '/v4/sso/logout?next=' "$file" || true
}

unused_port() {
  python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

start_mock_sync_server() {
  python3 - "$MOCK_PORT_FILE" >"$MOCK_LOG" 2>&1 <<'PY' &
import http.server
import json
import socketserver
import sys

port_file = sys.argv[1]

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path.startswith("/v4/sso/check"):
            self.send_response(200)
            self.end_headers()
            return
        self.send_response(404)
        self.end_headers()

    def log_message(self, fmt, *args):
        sys.stderr.write(fmt % args + "\n")

with socketserver.TCPServer(("127.0.0.1", 0), Handler) as httpd:
    with open(port_file, "w", encoding="utf-8") as fh:
        fh.write(str(httpd.server_address[1]))
    httpd.serve_forever()
PY
  MOCK_PID="$!"
  wait_file_contains "$MOCK_PORT_FILE" '^[0-9]'
}

start_bifrost() {
  local data_dir="$1"
  local port="$2"
  local open_file="$3"
  local disable_auto_login_prompt="${4:-false}"
  local log_file="$data_dir/bifrost.log"
  local -a env_args=(
    "BIFROST_DATA_DIR=$data_dir"
    "BIFROST_SYNC_LOGIN_BROWSER_DRY_RUN_FILE=$open_file"
    "BIFROST_SYNC_STARTUP_LOGIN_PREFLIGHT_RETRY_DELAY_MS=100"
  )
  if [[ "$disable_auto_login_prompt" == "true" ]]; then
    env_args+=("BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1")
  fi
  env "${env_args[@]}" \
    "$BIFROST_BIN" start -p "$port" --unsafe-ssl --skip-cert-check --no-system-proxy \
    >"$log_file" 2>&1 &
  BIFROST_PID="$!"
  wait_http "http://127.0.0.1:${port}/_bifrost/api/proxy/address"
}

stop_bifrost() {
  if [[ -n "$BIFROST_PID" ]] && kill -0 "$BIFROST_PID" 2>/dev/null; then
    kill "$BIFROST_PID" 2>/dev/null || true
    wait "$BIFROST_PID" 2>/dev/null || true
  fi
  BIFROST_PID=""
}

if [[ "${SKIP_BUILD:-false}" != "true" || ! -x "$BIFROST_BIN" ]]; then
  cargo build --bin bifrost >/dev/null
  BIFROST_BIN="$ROOT_DIR/target/debug/bifrost"
fi

start_mock_sync_server
MOCK_PORT="$(cat "$MOCK_PORT_FILE")"
MOCK_URL="http://127.0.0.1:${MOCK_PORT}"

echo "[sync-startup-preflight] case: reachable remote opens once and persists prompt"
DATA_DIR="$TEST_DIR/reachable-data"
mkdir -p "$DATA_DIR"
cat >"$DATA_DIR/config.toml" <<EOF
[sync]
enabled = true
auto_sync = true
remote_base_url = "${MOCK_URL}"
probe_interval_secs = 2
connect_timeout_ms = 500
EOF
OPEN_FILE="$TEST_DIR/opened-login-urls.txt"
PORT="$(unused_port)"
start_bifrost "$DATA_DIR" "$PORT" "$OPEN_FILE"
for _ in {1..40}; do
  [[ "$(open_count "$OPEN_FILE")" == "1" ]] && break
  sleep 0.25
done
[[ "$(open_count "$OPEN_FILE")" == "1" ]] || fail "expected exactly one startup login open"
grep -q '"startup_login_prompt"' "$DATA_DIR/sync-state.json" || fail "startup prompt state was not persisted"
stop_bifrost

echo "[sync-startup-preflight] case: restart does not open again"
PORT="$(unused_port)"
start_bifrost "$DATA_DIR" "$PORT" "$OPEN_FILE"
sleep 1
[[ "$(open_count "$OPEN_FILE")" == "1" ]] || fail "expected restart to preserve one startup login open"
stop_bifrost

echo "[sync-startup-preflight] case: environment disables startup login prompt"
DISABLED_DATA_DIR="$TEST_DIR/disabled-prompt-data"
mkdir -p "$DISABLED_DATA_DIR"
cat >"$DISABLED_DATA_DIR/config.toml" <<EOF
[sync]
enabled = true
auto_sync = true
remote_base_url = "${MOCK_URL}"
probe_interval_secs = 2
connect_timeout_ms = 500
EOF
DISABLED_OPEN_FILE="$TEST_DIR/disabled-opened-login-urls.txt"
PORT="$(unused_port)"
start_bifrost "$DISABLED_DATA_DIR" "$PORT" "$DISABLED_OPEN_FILE" true
sleep 1
[[ "$(open_count "$DISABLED_OPEN_FILE")" == "0" ]] || fail "disabled auto login prompt should not open login"
if [[ -f "$DISABLED_DATA_DIR/sync-state.json" ]]; then
  ! grep -q '"startup_login_prompt"' "$DISABLED_DATA_DIR/sync-state.json" \
    || fail "disabled auto login prompt should not persist startup prompt state"
fi
stop_bifrost

echo "[sync-startup-preflight] case: unreachable remote does not open after three probes"
UNREACHABLE_DATA_DIR="$TEST_DIR/unreachable-data"
mkdir -p "$UNREACHABLE_DATA_DIR"
UNREACHABLE_PORT="$(unused_port)"
cat >"$UNREACHABLE_DATA_DIR/config.toml" <<EOF
[sync]
enabled = true
auto_sync = true
remote_base_url = "http://127.0.0.1:${UNREACHABLE_PORT}"
probe_interval_secs = 2
connect_timeout_ms = 500
EOF
UNREACHABLE_OPEN_FILE="$TEST_DIR/unreachable-opened-login-urls.txt"
PORT="$(unused_port)"
start_bifrost "$UNREACHABLE_DATA_DIR" "$PORT" "$UNREACHABLE_OPEN_FILE"
sleep 1
[[ "$(open_count "$UNREACHABLE_OPEN_FILE")" == "0" ]] || fail "unreachable remote should not open login"
stop_bifrost

echo "[sync-startup-preflight] PASS"
