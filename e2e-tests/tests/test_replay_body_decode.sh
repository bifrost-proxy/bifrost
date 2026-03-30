#!/bin/bash
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
BIFROST_BIN="${BIFROST_BIN:-${ROOT_DIR}/target/release/bifrost}"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi

PROXY_PORT="${PROXY_PORT:-18888}"
ADMIN_PORT="$PROXY_PORT"
ADMIN_BASE_URL="http://127.0.0.1:${ADMIN_PORT}/_bifrost"

source "$SCRIPT_DIR/../test_utils/assert.sh"
source "$SCRIPT_DIR/../test_utils/process.sh"

BIFROST_PID=""
passed=0
failed=0

cleanup() {
    echo ""
    echo "Cleaning up..."

    if [ -n "$BIFROST_PID" ]; then
        echo "  Stopping Bifrost proxy (PID: $BIFROST_PID)..."
        safe_cleanup_proxy "$BIFROST_PID"
    fi

    if is_windows; then kill_bifrost_on_port "$PROXY_PORT"; fi
    echo "Cleanup complete."
}

trap cleanup EXIT

start_bifrost() {
    echo "Starting Bifrost proxy on port $PROXY_PORT..."
    cd "$ROOT_DIR"

    BIFROST_DATA_DIR="${BIFROST_DATA_DIR:-./.bifrost-e2e-test}" "$BIFROST_BIN" start -p "$PROXY_PORT" --unsafe-ssl --skip-cert-check > /tmp/bifrost_e2e.log 2>&1 &
    BIFROST_PID=$!

    local timeout=120
    local waited=0
    while [ $waited -lt $timeout ]; do
        if ! kill -0 "$BIFROST_PID" 2>/dev/null; then
            echo "Bifrost process exited unexpectedly (PID: $BIFROST_PID)"
            echo "Last log:"
            tail -30 /tmp/bifrost_e2e.log
            exit 1
        fi
        if curl -s "${ADMIN_BASE_URL}/api/system" >/dev/null 2>&1; then
            echo "  Bifrost proxy started (PID: $BIFROST_PID)"
            return 0
        fi
        sleep 2
        waited=$((waited + 2))
    done

    echo "Failed to start Bifrost proxy within ${timeout}s"
    echo "Last log:"
    tail -30 /tmp/bifrost_e2e.log
    exit 1
}

test_replay_gzip_decoding() {
    echo ""
    echo "=== Test: Replay gzip body decoding ==="

    local payload
    payload='{"method":"GET","url":"https://httpbin.org/gzip","headers":[["accept","application/json"],["accept-encoding","gzip"]],"rule_config":{"mode":"none"},"timeout_ms":15000}'

    local response
    response=$(curl -sS -X POST "${ADMIN_BASE_URL}/api/replay/execute" \
        -H "Content-Type: application/json" \
        -d "$payload")

    local status
    status=$(printf '%s' "$response" | jq -r '.data.status // empty')

    local body
    body=$(printf '%s' "$response" | jq -r '.data.body // ""')
    local gzipped
    if printf '%s' "$body" | grep -Eq '"gzipped"[[:space:]]*:[[:space:]]*true'; then
        gzipped="true"
    else
        gzipped=""
    fi

    if [ "$status" = "200" ] && [ "$gzipped" = "true" ]; then
        _log_pass "Replay decoded gzip response body as JSON"
        ((passed++))
    else
        _log_fail "Replay gzip body decoding failed" "status=200 & gzipped=true" "status=$status gzipped=$gzipped"
        ((failed++))
    fi
}

main() {
    echo "=========================================="
    echo "  Replay Body Decode E2E Tests"
    echo "=========================================="

    start_bifrost
    test_replay_gzip_decoding

    echo ""
    echo "Results: $passed passed, $failed failed"
    [ $failed -eq 0 ]
}

main "$@"
