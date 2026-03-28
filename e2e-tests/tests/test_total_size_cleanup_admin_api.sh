#!/bin/bash
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test_utils/admin_client.sh"
source "$SCRIPT_DIR/../test_utils/assert.sh"
source "$SCRIPT_DIR/../test_utils/http_client.sh"

HTTP_PORT="${HTTP_PORT:-3000}"
PROXY_PORT="${PROXY_PORT:-9900}"
ADMIN_PORT="${ADMIN_PORT:-9900}"
ADMIN_PATH_PREFIX="${ADMIN_PATH_PREFIX:-/_bifrost}"
export ADMIN_PATH_PREFIX
TEST_ID=""

admin_ensure_bifrost
trap 'admin_cleanup_bifrost; kill "$server_pid" 2>/dev/null || true' EXIT

python3 "$SCRIPT_DIR/../mock_servers/http_echo_server.py" "$HTTP_PORT" &
server_pid=$!

waited=0
while [ $waited -lt 15 ]; do
  if curl -sf --connect-timeout 2 --max-time 3 "http://127.0.0.1:${HTTP_PORT}/health" >/dev/null 2>&1; then
    break
  fi
  sleep 1
  waited=$((waited + 1))
done
if [ $waited -ge 15 ]; then
  echo "ERROR: Mock server on port $HTTP_PORT not ready after 15s" >&2
  exit 1
fi

payload=$(python3 - <<'PY'
print("a" * 32768)
PY
)

curl -s -X PUT -H "Content-Type: application/json" \
  -d '{"max_db_size_bytes":262144,"max_body_memory_size":1024}' \
  "http://127.0.0.1:${ADMIN_PORT}${ADMIN_PATH_PREFIX}/api/config/performance" >/dev/null

for i in $(seq 1 150); do
  http_post "http://127.0.0.1:${HTTP_PORT}/echo" "$payload"
done

traffic_response=$(admin_get "/api/traffic?limit=200")
record_count=$(echo "$traffic_response" | jq -r '.records | length // 0')
record_count="${record_count:-0}"

if [ "$record_count" -lt 150 ] 2>/dev/null; then
  _log_pass "total size cleanup removed oldest records (count $record_count)"
else
  _log_fail "total size cleanup removed oldest records" "< 150" "$record_count"
fi

echo ""
echo "Results: $PASSED_ASSERTIONS passed, $FAILED_ASSERTIONS failed"
if [ "$FAILED_ASSERTIONS" -gt 0 ]; then
  exit 1
fi
