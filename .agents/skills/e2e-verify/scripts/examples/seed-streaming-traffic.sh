#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../../../../" && pwd)"
PROXY_HOST="${PROXY_HOST:-127.0.0.1}"
PROXY_PORT="${PROXY_PORT:-9900}"
SSE_PORT="${SSE_PORT:-8767}"
WS_PORT="${WS_PORT:-3020}"
SSE_SERVER_PID=""
WS_SERVER_PID=""

cleanup() {
  if [[ -n "$SSE_SERVER_PID" ]] && kill -0 "$SSE_SERVER_PID" 2>/dev/null; then
    kill "$SSE_SERVER_PID" 2>/dev/null || true
    wait "$SSE_SERVER_PID" 2>/dev/null || true
  fi
  if [[ -n "$WS_SERVER_PID" ]] && kill -0 "$WS_SERVER_PID" 2>/dev/null; then
    kill "$WS_SERVER_PID" 2>/dev/null || true
    wait "$WS_SERVER_PID" 2>/dev/null || true
  fi
}

trap cleanup EXIT

if ! curl -s "http://${PROXY_HOST}:${PROXY_PORT}/_bifrost/api/system" >/dev/null 2>&1; then
  echo "proxy is not running on ${PROXY_HOST}:${PROXY_PORT}" >&2
  exit 1
fi

python3 "${ROOT_DIR}/e2e-tests/mock_servers/sse_echo_server.py" --port "${SSE_PORT}" >/dev/null 2>&1 &
SSE_SERVER_PID=$!

python3 "${ROOT_DIR}/e2e-tests/mock_servers/ws_echo_server.py" "${WS_PORT}" >/dev/null 2>&1 &
WS_SERVER_PID=$!

for _ in $(seq 1 50); do
  if curl -s "http://127.0.0.1:${SSE_PORT}/health" >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done

SSE_PROXY="http://${PROXY_HOST}:${PROXY_PORT}"
export SSE_PROXY
SSE_TEMP_DIR="/tmp/bifrost_sse_ui"
export SSE_TEMP_DIR

source "${ROOT_DIR}/e2e-tests/test_utils/sse_client.sh"
conn_id="$(sse_connect "http://127.0.0.1:${SSE_PORT}/sse?count=6&interval=0.2")"
sse_wait_events "$conn_id" 3 10 >/dev/null
sse_disconnect "$conn_id"

if command -v websocat >/dev/null 2>&1; then
  websocat -t --one-message --proxy "http://${PROXY_HOST}:${PROXY_PORT}" "ws://127.0.0.1:${WS_PORT}/ws/broadcast" >/dev/null 2>&1 || true
else
  echo "websocat not found" >&2
fi
