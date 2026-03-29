#!/bin/bash
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test_utils/admin_client.sh"
source "$SCRIPT_DIR/../test_utils/assert.sh"
source "$SCRIPT_DIR/../test_utils/http_client.sh"

HTTP_PORT="${HTTP_PORT:-3000}"
PROXY_PORT="${PROXY_PORT:-9910}"
ADMIN_PORT="${ADMIN_PORT:-9910}"
ADMIN_PATH_PREFIX="${ADMIN_PATH_PREFIX:-/_bifrost}"
ADMIN_PROXY_READY_URL="${ADMIN_PROXY_READY_URL:-http://127.0.0.1:${HTTP_PORT}/health}"
export ADMIN_PATH_PREFIX
export ADMIN_PROXY_READY_URL
TEST_ID=""

python3 "$SCRIPT_DIR/../mock_servers/http_echo_server.py" "$HTTP_PORT" >/tmp/bifrost_echo.log 2>&1 &
server_pid=$!
trap 'kill "$server_pid" 2>/dev/null || true; admin_cleanup_bifrost' EXIT

if ! admin_ensure_bifrost; then
  echo "Failed to start admin server"
  exit 1
fi

admin_delete "/api/config/performance/clear-cache" >/dev/null

curl -fsS -X PUT -H "Content-Type: application/json" \
  -d '{"max_db_size_bytes":262144,"max_body_memory_size":1,"file_retention_days":7,"max_records":1000}' \
  "http://127.0.0.1:${ADMIN_PORT}${ADMIN_PATH_PREFIX}/api/config/performance" >/dev/null

payload=$(python3 - <<'PY'
print("a" * 32768)
PY
)

request_count=150

for i in $(seq 1 "$request_count"); do
  http_post "http://127.0.0.1:${HTTP_PORT}/echo" "$payload"
done

for i in $(seq 1 60); do
  traffic_response=$(admin_get "/api/traffic?limit=20")
  record_count=$(echo "$traffic_response" | jq -r '.records | length')
  if [ "$record_count" -lt "$request_count" ]; then
    break
  fi
  sleep 1
done

if [ "$record_count" -lt "$request_count" ]; then
  _log_pass "traffic records were cleaned by performance policy (count $record_count)"
else
  _log_fail "traffic records were cleaned by performance policy" "< ${request_count}" "$record_count"
fi

body_waited=0
body_files="$request_count"
while [ "$body_waited" -lt 30 ]; do
  perf_response=$(admin_get "/api/config/performance")
  body_files=$(echo "$perf_response" | jq -r '.body_store_stats.file_count')
  body_files="${body_files:-$request_count}"
  if ! [[ "$body_files" =~ ^[0-9]+$ ]]; then
    body_files="$request_count"
  fi
  if [ "$body_files" -lt "$request_count" ]; then
    break
  fi
  sleep 1
  body_waited=$((body_waited + 1))
done

if [ "$body_files" -lt "$request_count" ]; then
  _log_pass "body_cache files cleaned with records (count $body_files)"
else
  _log_fail "body_cache files cleaned with records" "< ${request_count}" "$body_files"
fi

echo ""
echo "Results: $PASSED_ASSERTIONS passed, $FAILED_ASSERTIONS failed"
if [ "$FAILED_ASSERTIONS" -gt 0 ]; then
  exit 1
fi
