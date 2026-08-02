#!/bin/bash
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test_utils/admin_client.sh"
source "$SCRIPT_DIR/../test_utils/assert.sh"
source "$SCRIPT_DIR/../test_utils/http_client.sh"

ADMIN_HOST="${ADMIN_HOST:-127.0.0.1}"
ADMIN_PORT="${ADMIN_PORT:-}"
if [[ -z "${ADMIN_PORT}" ]]; then
    ADMIN_PORT="$(allocate_free_port)"
fi
PROXY_PORT="${PROXY_PORT:-$ADMIN_PORT}"
HTTP_PORT="${HTTP_PORT:-}"
if [[ -z "${HTTP_PORT}" ]]; then
    HTTP_PORT="$(allocate_free_port)"
fi
ADMIN_PATH_PREFIX="${ADMIN_PATH_PREFIX:-/_bifrost}"
export ADMIN_PORT ADMIN_PATH_PREFIX PROXY_PORT
TEST_ID=""
server_pid=""
MOCK_LOG=""

trap 'admin_cleanup_bifrost; kill "$server_pid" 2>/dev/null || true; rm -f "$MOCK_LOG" 2>/dev/null || true' EXIT

admin_ensure_bifrost || { echo "ERROR: Could not start Bifrost" >&2; exit 1; }

MOCK_LOG="$(mktemp)"
python3 -u "$SCRIPT_DIR/../mock_servers/http_echo_server.py" --port "$HTTP_PORT" --retries 5 >"$MOCK_LOG" 2>&1 &
server_pid=$!

MOCK_READY_TIMEOUT_SECONDS="${MOCK_READY_TIMEOUT_SECONDS:-90}"
mock_ready_deadline=$((SECONDS + MOCK_READY_TIMEOUT_SECONDS))
mock_ready=0
while [ "$SECONDS" -lt "$mock_ready_deadline" ]; do
  if grep -q "READY" "$MOCK_LOG" 2>/dev/null; then
    ready_port="$HTTP_PORT"
    bound_line=$(grep -o "bound to [0-9]*" "$MOCK_LOG" 2>/dev/null | head -1 || true)
    if [[ -n "$bound_line" ]]; then
      ready_port="${bound_line##*bound to }"
    fi
    if env NO_PROXY="*" no_proxy="*" curl -fsS --connect-timeout 1 --max-time 3 \
      "http://127.0.0.1:${ready_port}/health" >/dev/null 2>&1; then
      mock_ready=1
      break
    fi
  fi
  if ! kill -0 "$server_pid" 2>/dev/null; then
    echo "ERROR: Mock server process exited unexpectedly" >&2
    cat "$MOCK_LOG" >&2
    exit 1
  fi
  sleep 0.25
done
if [ "$mock_ready" -eq 0 ]; then
  echo "ERROR: Mock server on port $HTTP_PORT not ready after ${MOCK_READY_TIMEOUT_SECONDS}s" >&2
  cat "$MOCK_LOG" >&2
  exit 1
fi

actual_mock_port="$HTTP_PORT"
bound_line=$(grep -o "bound to [0-9]*" "$MOCK_LOG" 2>/dev/null | head -1 || true)
if [[ -n "$bound_line" ]]; then
  actual_mock_port="${bound_line##*bound to }"
  echo "NOTE: Mock server bound to port $actual_mock_port (requested $HTTP_PORT)" >&2
  HTTP_PORT="$actual_mock_port"
fi
rm -f "$MOCK_LOG"

warmup_ok=0
for warmup_try in 1 2 3; do
  http_post "http://127.0.0.1:${HTTP_PORT}/echo" "warmup" >/dev/null 2>&1 && { warmup_ok=1; break; }
  sleep 2
done
if [ "$warmup_ok" -eq 0 ]; then
  echo "WARN: warmup requests failed, proxy may not be forwarding" >&2
fi
sleep 1

payload=$(python3 - <<'PY'
print("a" * 32768)
PY
)

config_response=$(env NO_PROXY="*" no_proxy="*" curl -s -X PUT -H "Content-Type: application/json" \
  -d '{"max_db_size_bytes":262144,"max_body_memory_size":1024}' \
  "http://127.0.0.1:${ADMIN_PORT}${ADMIN_PATH_PREFIX}/api/config/performance")
if echo "$config_response" | jq -e '.error' >/dev/null 2>&1; then
  echo "WARN: config update may have failed: $config_response" >&2
fi

admin_delete "/api/traffic" >/dev/null 2>&1 || true
sleep 1

send_ok=0
send_fail=0
for i in $(seq 1 150); do
  if http_post "http://127.0.0.1:${HTTP_PORT}/echo" "$payload" >/dev/null 2>&1; then
    send_ok=$((send_ok + 1))
  else
    send_fail=$((send_fail + 1))
    if [ "$send_fail" -ge 10 ] && [ "$send_ok" -eq 0 ]; then
      echo "WARN: first $send_fail requests all failed, aborting loop early" >&2
      break
    fi
  fi
done

if [ "$send_ok" -eq 0 ]; then
  echo "WARN: all http_post requests failed (send_fail=$send_fail)" >&2
fi

sleep 3

waited=0
record_count=0
traffic_response=""
found_records=0
while [ $waited -lt 60 ]; do
  traffic_response=$(admin_get "/api/traffic?limit=200")
  record_count=$(echo "$traffic_response" | jq -r '(.records // []) | length')
  record_count="${record_count:-0}"
  if ! [[ "$record_count" =~ ^[0-9]+$ ]]; then
    record_count=0
  fi
  if [ "$record_count" -gt 0 ]; then
    found_records=1
  fi
  if [ "$found_records" -eq 1 ] && [ "$record_count" -lt 150 ]; then
    break
  fi
  sleep 1
  waited=$((waited + 1))
done

if [ "$record_count" -eq 0 ]; then
  _log_fail "total size cleanup should have recorded some traffic" "> 0" "$record_count"
elif [ "$record_count" -lt 150 ]; then
  _log_pass "total size cleanup removed oldest records (count $record_count)"
else
  _log_fail "total size cleanup removed oldest records" "< 150" "$record_count"
fi

echo ""
echo "Results: $PASSED_ASSERTIONS passed, $FAILED_ASSERTIONS failed"
if [ "$FAILED_ASSERTIONS" -gt 0 ]; then
  exit 1
fi
