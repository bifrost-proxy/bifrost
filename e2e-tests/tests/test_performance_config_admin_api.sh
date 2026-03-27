#!/bin/bash
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test_utils/admin_client.sh"
source "$SCRIPT_DIR/../test_utils/assert.sh"

ADMIN_HOST="${ADMIN_HOST:-127.0.0.1}"
ADMIN_PORT="${ADMIN_PORT:-9900}"
ADMIN_PATH_PREFIX="${ADMIN_PATH_PREFIX:-/_bifrost}"
export ADMIN_PATH_PREFIX

trap admin_cleanup_bifrost EXIT

if ! admin_ensure_bifrost; then
    echo "[FAIL] Failed to start Bifrost admin server"
    exit 1
fi

response=$(admin_get "/api/config/performance")

assert_json_field_exists ".traffic.max_db_size_bytes" "$response" "max_db_size_bytes should exist"
assert_json_field_exists ".body_store_stats.total_size" "$response" "body_store_stats.total_size should exist"
assert_json_field_exists ".frame_store_stats.total_size" "$response" "frame_store_stats.total_size should exist"
assert_json_field_exists ".ws_payload_store_stats.total_size" "$response" "ws_payload_store_stats.total_size should exist"

echo ""
echo "Results: $PASSED_ASSERTIONS passed, $FAILED_ASSERTIONS failed"
if [ "$FAILED_ASSERTIONS" -gt 0 ]; then
    exit 1
fi
