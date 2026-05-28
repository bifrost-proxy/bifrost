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
MOCK_HTTP_PORT="${MOCK_HTTP_PORT:-${ECHO_HTTP_PORT:-3000}}"
MOCK_BASE_URL="http://127.0.0.1:${MOCK_HTTP_PORT}"
MOCK_LOG_DIR="${SERVER_LOG_DIR:-${BIFROST_DATA_DIR:-./.bifrost-e2e-test}/mock-logs}"

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
    MOCK_SERVERS="http" \
    HTTP_PORT="$MOCK_HTTP_PORT" \
    "$ROOT_DIR/e2e-tests/mock_servers/start_servers.sh" stop >/dev/null 2>&1 || true
    echo "Cleanup complete."
}

trap cleanup EXIT

start_bifrost() {
    echo "Starting Bifrost proxy on port $PROXY_PORT..."
    cd "$ROOT_DIR"

    BIFROST_DATA_DIR="${BIFROST_DATA_DIR:-./.bifrost-e2e-test}" "$BIFROST_BIN" start -p "$PROXY_PORT" --unsafe-ssl --skip-cert-check --no-system-proxy > /tmp/bifrost_e2e.log 2>&1 &
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

start_mock_httpbin() {
    echo "Starting local httpbin mock on port $MOCK_HTTP_PORT..."
    MOCK_SERVERS="http" \
    HTTP_PORT="$MOCK_HTTP_PORT" \
    SERVER_LOG_DIR="$MOCK_LOG_DIR" \
    "$ROOT_DIR/e2e-tests/mock_servers/start_servers.sh" start-bg
}

test_replay_gzip_decoding() {
    echo ""
    echo "=== Test: Replay gzip body decoding ==="

    local payload
    payload=$(jq -cn --arg url "${MOCK_BASE_URL}/gzip" '{method:"GET",url:$url,headers:[["accept","application/json"],["accept-encoding","gzip"]],rule_config:{mode:"none"},timeout_ms:15000}')

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
        passed=$((passed + 1))
    else
        _log_fail "Replay gzip body decoding failed" "status=200 & gzipped=true" "status=$status gzipped=$gzipped"
        failed=$((failed + 1))
    fi
}

main() {
    echo "=========================================="
    echo "  Replay Body Decode E2E Tests"
    echo "=========================================="

    start_mock_httpbin
    start_bifrost
    test_replay_gzip_decoding

    echo ""
    echo "Results: $passed passed, $failed failed"
    [ $failed -eq 0 ]
}

main "$@"
