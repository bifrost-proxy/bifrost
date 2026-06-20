#!/bin/bash
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../test_utils/admin_client.sh
source "$SCRIPT_DIR/../test_utils/admin_client.sh"
# shellcheck source=../test_utils/assert.sh
source "$SCRIPT_DIR/../test_utils/assert.sh"

ADMIN_HOST="${ADMIN_HOST:-127.0.0.1}"
ADMIN_PORT="${ADMIN_PORT:-$(allocate_free_port)}"
ADMIN_PATH_PREFIX="${ADMIN_PATH_PREFIX:-/_bifrost}"
export ADMIN_HOST ADMIN_PORT ADMIN_PATH_PREFIX

cleanup() {
    admin_cleanup_bifrost
}
trap cleanup EXIT

if ! admin_ensure_bifrost; then
    echo "Failed to start or reach Bifrost admin API" >&2
    exit 1
fi

tray_url="$(admin_base_url)/api/config/tray"

get_tray_config() {
    env NO_PROXY="*" no_proxy="*" curl -fsS "$tray_url"
}

put_tray_config() {
    local payload="$1"
    env NO_PROXY="*" no_proxy="*" curl -fsS -X PUT "$tray_url" \
        -H 'Content-Type: application/json' \
        -d "$payload"
}

default_config="$(get_tray_config)"
assert_json_field ".enabled" "true" "$default_config" "tray icon should default to enabled"
assert_json_field ".show_system_stats" "true" "$default_config" "tray system stats should default to enabled"

updated_stats_off="$(put_tray_config '{"show_system_stats":false}')"
assert_json_field ".enabled" "true" "$updated_stats_off" "system stats update should preserve tray enabled"
assert_json_field ".show_system_stats" "false" "$updated_stats_off" "system stats can be disabled independently"

persisted_stats_off="$(get_tray_config)"
assert_json_field ".show_system_stats" "false" "$persisted_stats_off" "disabled system stats should persist through GET"

if [[ -n "${BIFROST_DATA_DIR:-}" ]]; then
    if grep -q 'show_system_stats = false' "$BIFROST_DATA_DIR/config.toml"; then
        _log_pass "config.toml persisted show_system_stats = false"
    else
        _log_fail "config.toml persisted show_system_stats = false" \
            "show_system_stats = false" \
            "$(sed -n '/\[tray\]/,+4p' "$BIFROST_DATA_DIR/config.toml" 2>/dev/null || true)"
    fi
fi

updated_tray_off="$(put_tray_config '{"enabled":false}')"
assert_json_field ".enabled" "false" "$updated_tray_off" "tray icon can still be disabled with legacy payload"
assert_json_field ".show_system_stats" "false" "$updated_tray_off" "tray enabled update should preserve system stats setting"

updated_both_on="$(put_tray_config '{"enabled":true,"show_system_stats":true}')"
assert_json_field ".enabled" "true" "$updated_both_on" "tray icon can be re-enabled"
assert_json_field ".show_system_stats" "true" "$updated_both_on" "system stats can be re-enabled"

invalid_response="$(env NO_PROXY="*" no_proxy="*" curl -sS -X PUT "$tray_url" \
    -H 'Content-Type: application/json' \
    -d '{}')"
assert_json_field ".error" "enabled or show_system_stats is required" "$invalid_response" \
    "empty tray config update should be rejected"

echo ""
echo "Results: $PASSED_ASSERTIONS passed, $FAILED_ASSERTIONS failed"
if [ "$FAILED_ASSERTIONS" -gt 0 ]; then
    exit 1
fi
