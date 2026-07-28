#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

source "${PROJECT_DIR}/e2e-tests/test_utils/process.sh"

allocate_port() {
    python3 - <<'PY'
import socket
with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
}

PROXY_PORT="${PROXY_PORT:-$(allocate_port)}"
UPSTREAM_PORT="${UPSTREAM_PORT:-$(allocate_port)}"
UNUSED_PORT="${UNUSED_PORT:-$(allocate_port)}"
REQUEST_COUNT="${REQUEST_COUNT:-80}"
CONNECT_REQUEST_COUNT="${CONNECT_REQUEST_COUNT:-40}"
CONCURRENCY="${CONCURRENCY:-64}"
GLOBAL_INFLIGHT="${GLOBAL_INFLIGHT:-8}"
CONNECT_CONCURRENCY="${CONNECT_CONCURRENCY:-4}"
BIFROST_BIN="${BIFROST_BIN:-${PROJECT_DIR}/target/debug/bifrost}"

TEST_DATA_DIR=""
RESPONSES_DIR=""
PROXY_PID=""
UPSTREAM_PID=""

cleanup() {
    if [[ -n "$TEST_DATA_DIR" ]]; then
        BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" -p "$PROXY_PORT" stop >/dev/null 2>&1 || true
    fi
    safe_cleanup_proxy "$PROXY_PID"
    kill_pid "$UPSTREAM_PID"
    if [[ -n "$UPSTREAM_PID" ]]; then
        wait "$UPSTREAM_PID" 2>/dev/null || true
    fi
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

wait_for_url() {
    local url="$1"
    local owner_pid="$2"
    local log_file="$3"
    local attempts=0
    while [[ "$attempts" -lt 120 ]]; do
        if env NO_PROXY="*" no_proxy="*" curl -fsS --connect-timeout 1 --max-time 2 \
            "$url" >/dev/null 2>&1; then
            return 0
        fi
        if [[ -n "$owner_pid" ]] && ! kill -0 "$owner_pid" 2>/dev/null; then
            break
        fi
        sleep 0.25
        attempts=$((attempts + 1))
    done
    echo "Timed out waiting for ${url}. Log:" >&2
    sed -n '1,240p' "$log_file" >&2 2>/dev/null || true
    return 1
}

start_fixture() {
    TEST_DATA_DIR="$(mktemp -d)"
    RESPONSES_DIR="${TEST_DATA_DIR}/responses"
    mkdir -p "$RESPONSES_DIR"
    mark_e2e_data_root "$TEST_DATA_DIR"

    python3 "${PROJECT_DIR}/e2e-tests/mock_servers/upstream_connection_stability_server.py" \
        "$UPSTREAM_PORT" >"${TEST_DATA_DIR}/upstream.log" 2>&1 &
    UPSTREAM_PID=$!
    wait_for_url \
        "http://127.0.0.1:${UPSTREAM_PORT}/stats" \
        "$UPSTREAM_PID" \
        "${TEST_DATA_DIR}/upstream.log"

    BIFROST_DATA_DIR="$TEST_DATA_DIR" \
    BIFROST_DISABLE_TRAY=1 \
    BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
    BIFROST_UPSTREAM_MAX_INFLIGHT_GLOBAL="$GLOBAL_INFLIGHT" \
    BIFROST_UPSTREAM_CONNECT_CONCURRENCY="$CONNECT_CONCURRENCY" \
        "$BIFROST_BIN" -H 127.0.0.1 -p "$PROXY_PORT" start \
        -y \
        --access-mode allow_all \
        --skip-cert-check \
        --no-system-proxy \
        >"${TEST_DATA_DIR}/proxy.log" 2>&1 &
    PROXY_PID=$!
    wait_for_url \
        "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/proxy/address" \
        "$PROXY_PID" \
        "${TEST_DATA_DIR}/proxy.log"
}

assert_responses() {
    local expected_count="$1"
    local prefix="$2"
    local count
    count="$(find "$RESPONSES_DIR" -type f -name "${prefix}-*.json" | wc -l | tr -d ' ')"
    if [[ "$count" != "$expected_count" ]]; then
        echo "Expected ${expected_count} ${prefix} responses, got ${count}" >&2
        return 1
    fi

    local response
    while IFS= read -r response; do
        jq -e '.ok == true and .method == "GET" and .path == "/work"' "$response" >/dev/null
    done < <(find "$RESPONSES_DIR" -type f -name "${prefix}-*.json" | sort)
}

run_http_burst() {
    env NO_PROXY="*" no_proxy="*" \
        curl -fsS "http://127.0.0.1:${UPSTREAM_PORT}/reset" >/dev/null

    seq "$REQUEST_COUNT" | xargs -P "$CONCURRENCY" -I{} \
        curl --noproxy "" -fsS --max-time 15 \
        -x "http://127.0.0.1:${PROXY_PORT}" \
        "http://127.0.0.1:${UPSTREAM_PORT}/work?id={}&delay_ms=120" \
        -o "${RESPONSES_DIR}/http-{}.json"

    assert_responses "$REQUEST_COUNT" "http"

    local stats active peak total
    stats="$(env NO_PROXY="*" no_proxy="*" curl -fsS \
        "http://127.0.0.1:${UPSTREAM_PORT}/stats")"
    active="$(jq -r '.active' <<<"$stats")"
    peak="$(jq -r '.peak' <<<"$stats")"
    total="$(jq -r '.total' <<<"$stats")"
    if [[ "$total" != "$REQUEST_COUNT" ]]; then
        echo "Upstream handled ${total} requests, expected ${REQUEST_COUNT}" >&2
        return 1
    fi
    if [[ "$peak" -gt "$GLOBAL_INFLIGHT" || "$peak" -lt 2 ]]; then
        echo "Unexpected upstream peak concurrency ${peak}; expected 2..${GLOBAL_INFLIGHT}" >&2
        return 1
    fi
    if [[ "$active" != "0" ]]; then
        echo "Expected upstream activity to drain after the burst, got ${active}" >&2
        return 1
    fi
}

run_connect_burst() {
    seq "$CONNECT_REQUEST_COUNT" | xargs -P "$CONCURRENCY" -I{} \
        curl --noproxy "" -fsS --max-time 15 \
        --proxytunnel \
        -x "http://127.0.0.1:${PROXY_PORT}" \
        "http://127.0.0.1:${UPSTREAM_PORT}/work?id=connect-{}&delay_ms=20" \
        -o "${RESPONSES_DIR}/connect-{}.json"

    assert_responses "$CONNECT_REQUEST_COUNT" "connect"
}

run_non_resource_failure_recovery() {
    local status
    status="$(curl --noproxy "" -sS --max-time 5 -o /dev/null -w '%{http_code}' \
        -x "http://127.0.0.1:${PROXY_PORT}" \
        "http://127.0.0.1:${UNUSED_PORT}/expected-refusal")"
    if [[ "$status" != "502" ]]; then
        echo "Expected a 502 for the unused upstream port, got ${status}" >&2
        return 1
    fi

    curl --noproxy "" -fsS --max-time 3 \
        -x "http://127.0.0.1:${PROXY_PORT}" \
        "http://127.0.0.1:${UPSTREAM_PORT}/work?id=recovered&delay_ms=0" \
        | jq -e '.ok == true and .id == "recovered"' >/dev/null
}

assert_no_resource_pressure_errors() {
    if rg -n \
        "Can't assign requested address|cannot assign requested address|No buffer space available|too many open files" \
        "${TEST_DATA_DIR}/proxy.log"; then
        echo "Proxy log contains resource-pressure connection failures" >&2
        return 1
    fi
}

main() {
    build_bifrost
    start_fixture
    run_http_burst
    run_connect_burst
    run_non_resource_failure_recovery
    assert_no_resource_pressure_errors
    echo "Upstream connection stability E2E passed"
}

main "$@"
