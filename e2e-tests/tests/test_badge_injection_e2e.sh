#!/bin/bash

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

source "$PROJECT_DIR/e2e-tests/test_utils/assert.sh"
source "$PROJECT_DIR/e2e-tests/test_utils/http_client.sh"

pick_free_port() {
  python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()'
}

PROXY_HOST="127.0.0.1"
PROXY_PORT="${PROXY_PORT:-}"
HTML_PORT="${HTML_PORT:-}"

if [[ -z "$PROXY_PORT" ]]; then
  PROXY_PORT="$(pick_free_port)"
fi

if [[ -z "$HTML_PORT" ]]; then
  while true; do
    HTML_PORT="$(pick_free_port)"
    if [[ "$HTML_PORT" != "$PROXY_PORT" ]]; then
      break
    fi
  done
fi

export PROXY_HOST PROXY_PORT HTML_PORT
export TEST_ID="${TEST_ID:-}"


BIFROST_DATA_DIR_BASE="${PROJECT_DIR}/.bifrost-e2e-badge"
HTML_DIR="${PROJECT_DIR}/e2e-tests/test_data/badge_injection"

BIFROST_BIN="${PROJECT_DIR}/target/release/bifrost"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi

PROXY_PID=""
HTML_PID=""

cleanup() {
  if [[ -n "${PROXY_PID}" ]]; then
    kill "${PROXY_PID}" >/dev/null 2>&1 || true
    wait "${PROXY_PID}" >/dev/null 2>&1 || true
  fi
  if [[ -n "${HTML_PID}" ]]; then
    kill "${HTML_PID}" >/dev/null 2>&1 || true
    wait "${HTML_PID}" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

build_bifrost() {
  if [[ -f "$BIFROST_BIN" ]] && [[ "${SKIP_BUILD:-false}" == "true" ]]; then
    echo "[INFO] Skipping build (SKIP_BUILD=true), using existing binary: $BIFROST_BIN" >&2
    echo "$BIFROST_BIN"
    return 0
  fi

  echo "[INFO] Building bifrost binary..." >&2
  cargo build --release --bin bifrost >/dev/null

  if [[ ! -x "$BIFROST_BIN" ]]; then
    echo "[FAIL] bifrost binary not found at $BIFROST_BIN" >&2
    exit 1
  fi

  echo "$BIFROST_BIN"
}

start_html_server() {
  echo "[INFO] Starting html server on :${HTML_PORT}"
  python3 -m http.server "$HTML_PORT" --bind 127.0.0.1 --directory "$HTML_DIR" >/dev/null 2>&1 &
  HTML_PID=$!

  for _ in $(seq 1 30); do
    if env NO_PROXY="*" no_proxy="*" curl -sS "http://127.0.0.1:${HTML_PORT}/index.html" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.2
  done

  echo "[FAIL] html server not ready" >&2
  exit 1
}

start_proxy() {
  local data_dir="$1"
  shift

  rm -rf "$data_dir" || true
  mkdir -p "$data_dir"

  local log_file="$data_dir/bifrost.log"

  echo "[INFO] Starting bifrost proxy on ${PROXY_HOST}:${PROXY_PORT} (data_dir=${data_dir})"
  BIFROST_DATA_DIR="$data_dir" "$BIFROST_BIN" start -p "$PROXY_PORT" --skip-cert-check --unsafe-ssl --no-system-proxy "$@" >"$log_file" 2>&1 &
  PROXY_PID=$!

  for _ in $(seq 1 120); do
    if env NO_PROXY="*" no_proxy="*" curl -sS "http://${PROXY_HOST}:${PROXY_PORT}/_bifrost/api/proxy/address" >/dev/null 2>&1; then
      return 0
    fi
    if ! kill -0 "$PROXY_PID" >/dev/null 2>&1; then
      echo "[FAIL] bifrost process exited early" >&2
      tail -n 80 "$log_file" >&2 || true
      exit 1
    fi
    sleep 0.2
  done

  echo "[FAIL] bifrost proxy not ready" >&2
  tail -n 80 "$log_file" >&2 || true
  exit 1
}

stop_proxy() {
  if [[ -n "${PROXY_PID}" ]]; then
    kill "${PROXY_PID}" >/dev/null 2>&1 || true
    wait "${PROXY_PID}" >/dev/null 2>&1 || true
    PROXY_PID=""
  fi
}

fetch_via_proxy() {
  local url="$1"
  local headers_file
  headers_file="$(mktemp)"
  local body_file
  body_file="$(mktemp)"

  HTTP_STATUS="$(NO_PROXY="" no_proxy="" curl -sS --max-time "$(http_timeout)" --proxy "http://${PROXY_HOST}:${PROXY_PORT}" -D "$headers_file" -o "$body_file" -w '%{http_code}' "$url" 2>/dev/null || echo 000)"
  HTTP_HEADERS="$(cat "$headers_file" | tr -d '\r')"
  HTTP_BODY="$(cat "$body_file")"

  rm -f "$headers_file" "$body_file"
}

assert_badge_injection() {
  local expected="$1"
  local url="http://127.0.0.1:${HTML_PORT}/index.html"

  fetch_via_proxy "$url"
  assert_status "200" "$HTTP_STATUS" "HTML request should succeed"

  if [[ "$expected" == "present" ]]; then
    assert_body_contains "__bifrost_badge__" "$HTTP_BODY" "Badge marker should be injected"
    assert_body_contains "__bb_copy" "$HTTP_BODY" "Merged rules copy button should be injected"
    assert_body_contains "Copy merged rules" "$HTTP_BODY" "Copy button should expose accessible label"
    assert_body_contains "navigator.clipboard" "$HTTP_BODY" "Copy button should use Clipboard API when available"
    assert_body_contains "document.execCommand('copy')" "$HTTP_BODY" "Copy button should include execCommand fallback"
    assert_body_contains "clipboardData.setData('text/plain',text)" "$HTTP_BODY" "Fallback copy should write text via copy event clipboardData"
    assert_body_contains "ok&&copied?resolve()" "$HTTP_BODY" "Fallback copy should only report success after copy event writes data"
    assert_body_contains "z-index:2147483647!important" "$HTTP_BODY" "Badge panel should use top z-index"
  else
    assert_body_not_contains "__bifrost_badge__" "$HTTP_BODY" "Badge marker should not be injected"
    assert_body_not_contains "__bb_copy" "$HTTP_BODY" "Merged rules copy button should not be injected"
  fi
}

BIFROST_BIN="$(build_bifrost)"
start_html_server

echo "[INFO] Case 1: --disable-badge-injection"
start_proxy "${BIFROST_DATA_DIR_BASE}-disabled" --disable-badge-injection
assert_badge_injection "absent" || { print_test_summary || true; exit 1; }
stop_proxy

echo "[INFO] Case 2: --enable-badge-injection"
start_proxy "${BIFROST_DATA_DIR_BASE}-enabled" --enable-badge-injection
assert_badge_injection "present" || { print_test_summary || true; exit 1; }
stop_proxy

print_test_summary || exit 1
echo "[PASS] Badge injection E2E finished"
