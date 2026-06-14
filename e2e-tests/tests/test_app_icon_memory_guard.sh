#!/bin/bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

source "${PROJECT_DIR}/e2e-tests/test_utils/process.sh"

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "Skipping app icon memory guard: macOS-specific NSWorkspace regression test"
    exit 0
fi

allocate_port() {
    python3 - <<'PY'
import socket
with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
    s.bind(("127.0.0.1", 0))
    print(s.getsockname()[1])
PY
}

PROXY_PORT="${PROXY_PORT:-$(allocate_port)}"
BIFROST_BIN="${BIFROST_BIN:-${PROJECT_DIR}/target/debug/bifrost}"
TEST_DATA_DIR=""
PROXY_PID=""

cleanup() {
    if [[ -n "$TEST_DATA_DIR" ]]; then
        BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" -p "$PROXY_PORT" stop >/dev/null 2>&1 || true
    fi
    safe_cleanup_proxy "$PROXY_PID"
    kill_bifrost_on_port "$PROXY_PORT"
    if [[ -n "$TEST_DATA_DIR" && -d "$TEST_DATA_DIR" ]]; then
        rm -rf "$TEST_DATA_DIR"
    fi
}
trap cleanup EXIT

build_bifrost() {
    if [[ -x "$BIFROST_BIN" && "${SKIP_BUILD:-false}" == "true" ]]; then
        return 0
    fi
    (cd "$PROJECT_DIR" && SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost)
}

wait_admin_ready() {
    local waited=0
    while [[ "$waited" -lt 120 ]]; do
        if curl -fsS "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/proxy/address" >/dev/null 2>&1; then
            return 0
        fi
        if [[ -n "$PROXY_PID" ]] && ! kill -0 "$PROXY_PID" 2>/dev/null; then
            break
        fi
        sleep 0.5
        waited=$((waited + 1))
    done

    echo "Bifrost did not become ready. Log:" >&2
    cat "${TEST_DATA_DIR}/proxy.log" >&2 2>/dev/null || true
    return 1
}

start_proxy() {
    TEST_DATA_DIR="$(mktemp -d)"
    kill_bifrost_on_port "$PROXY_PORT"
    BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" -p "$PROXY_PORT" start \
        --skip-cert-check \
        --unsafe-ssl \
        --no-system-proxy \
        -y \
        >"${TEST_DATA_DIR}/proxy.log" 2>&1 &
    PROXY_PID=$!
    wait_admin_ready
}

rss_kb() {
    ps -o rss= -p "$PROXY_PID" | tr -d '[:space:]'
}

request_icon() {
    local app_name="$1"
    local encoded
    encoded="$(python3 - "$app_name" <<'PY'
import sys, urllib.parse
print(urllib.parse.quote(sys.argv[1]))
PY
)"
    curl -sS -o "${TEST_DATA_DIR}/${app_name}.png" -w "%{http_code}" \
        "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/app-icon/${encoded}"
}

main() {
    build_bifrost
    start_proxy

    local before
    before="$(rss_kb)"
    if [[ -z "$before" || "$before" -le 0 ]]; then
        echo "Failed to read Bifrost RSS before icon requests" >&2
        exit 1
    fi

    local apps=("Calculator" "Calendar" "Contacts" "FaceTime" "Freeform" "Maps" "Photos" "Preview" "Reminders" "Shortcuts" "Stickies" "TextEdit")
    local successes=0
    local status
    for app in "${apps[@]}"; do
        status="$(request_icon "$app")"
        if [[ "$status" == "200" ]]; then
            successes=$((successes + 1))
        elif [[ "$status" != "404" ]]; then
            echo "Unexpected app icon status for ${app}: ${status}" >&2
            exit 1
        fi
    done

    if [[ "$successes" -lt 3 ]]; then
        echo "Expected at least 3 real app icons, got ${successes}" >&2
        cat "${TEST_DATA_DIR}/proxy.log" >&2 2>/dev/null || true
        exit 1
    fi

    sleep 2
    local after
    after="$(rss_kb)"
    local delta=$((after - before))
    if [[ "$delta" -gt 150000 ]]; then
        echo "Bifrost RSS grew too much after app icon requests: before=${before}KB after=${after}KB delta=${delta}KB" >&2
        exit 1
    fi

    echo "App icon memory guard passed: successes=${successes} before=${before}KB after=${after}KB delta=${delta}KB"
}

main "$@"
