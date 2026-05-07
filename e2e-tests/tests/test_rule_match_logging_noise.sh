#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"
RULES_FILE="${PROJECT_DIR}/e2e-tests/rules/logging/rule_match_noise.txt"
TEST_ROOT="${TEST_ROOT:-/tmp/bifrost-e2e-rule-match-logging}"
BIFROST_BIN="${BIFROST_BIN:-${PROJECT_DIR}/target/debug/bifrost}"

INFO_PID=""
DEBUG_PID=""

cleanup() {
    if [[ -n "${INFO_PID:-}" ]]; then
        kill "$INFO_PID" 2>/dev/null || true
        wait "$INFO_PID" 2>/dev/null || true
    fi
    if [[ -n "${DEBUG_PID:-}" ]]; then
        kill "$DEBUG_PID" 2>/dev/null || true
        wait "$DEBUG_PID" 2>/dev/null || true
    fi
    rm -rf "$TEST_ROOT"
}

trap cleanup EXIT

wait_for_proxy() {
    local pid="$1"
    local port="$2"
    local log_file="$3"

    for _ in {1..60}; do
        if grep -q "Unified proxy server listening on 127.0.0.1:${port}" "$log_file"; then
            return 0
        fi
        if ! kill -0 "$pid" 2>/dev/null; then
            echo "proxy exited early on port ${port}" >&2
            tail -80 "$log_file" >&2 || true
            return 1
        fi
        sleep 1
    done

    echo "proxy did not become ready on port ${port}" >&2
    tail -80 "$log_file" >&2 || true
    return 1
}

request_via_proxy() {
    local port="$1"
    http_proxy="http://127.0.0.1:${port}" \
        https_proxy="http://127.0.0.1:${port}" \
        no_proxy= \
        curl -sS --max-time 5 http://example.test/
}

if [[ ! -x "$BIFROST_BIN" ]]; then
    CARGO_INCREMENTAL=0 cargo build --bin bifrost
fi

rm -rf "$TEST_ROOT"
mkdir -p "$TEST_ROOT"

BIFROST_DATA_DIR="${TEST_ROOT}/data-info" \
    RUST_LOG=info \
    "$BIFROST_BIN" start \
    --host 127.0.0.1 \
    -p 18887 \
    --unsafe-ssl \
    --no-system-proxy \
    --rules-file "$RULES_FILE" \
    >"${TEST_ROOT}/info.log" 2>&1 &
INFO_PID=$!

wait_for_proxy "$INFO_PID" 18887 "${TEST_ROOT}/info.log"
INFO_BODY="$(request_via_proxy 18887)"
test "$INFO_BODY" = "ok"
sleep 1
if grep -E "rule MATCHED|rules matched for request|matched rule detail" "${TEST_ROOT}/info.log"; then
    echo "FAIL: rule match logs visible at info" >&2
    exit 1
fi

kill "$INFO_PID" 2>/dev/null || true
wait "$INFO_PID" 2>/dev/null || true
INFO_PID=""

BIFROST_DATA_DIR="${TEST_ROOT}/data-debug" \
    RUST_LOG="bifrost_core::rules=debug,bifrost_proxy::rules=trace,info" \
    "$BIFROST_BIN" start \
    --host 127.0.0.1 \
    -p 18888 \
    --unsafe-ssl \
    --no-system-proxy \
    --rules-file "$RULES_FILE" \
    >"${TEST_ROOT}/debug.log" 2>&1 &
DEBUG_PID=$!

wait_for_proxy "$DEBUG_PID" 18888 "${TEST_ROOT}/debug.log"
DEBUG_BODY="$(request_via_proxy 18888)"
test "$DEBUG_BODY" = "ok"
sleep 1
grep -q "rule MATCHED" "${TEST_ROOT}/debug.log"
grep -q "rules matched for request" "${TEST_ROOT}/debug.log"
grep -q "matched rule detail" "${TEST_ROOT}/debug.log"

echo "PASS: rule match logging noise is hidden at info and available when explicitly enabled"
