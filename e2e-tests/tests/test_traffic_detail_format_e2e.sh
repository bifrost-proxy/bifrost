#!/bin/bash

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

source "${PROJECT_DIR}/e2e-tests/test_utils/process.sh"

BIFROST_BIN="${PROJECT_DIR}/target/release/bifrost"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi

PROXY_PORT="${PROXY_PORT:-18823}"
TEST_DATA_DIR=""
PROXY_PID=""
BIFROST_LOG_FILE=""

PASSED=0
FAILED=0

RED='\033[0;31m'
GREEN='\033[0;32m'
CYAN='\033[0;36m'
BLUE='\033[0;34m'
NC='\033[0m'

info() { echo -e "${CYAN}[INFO]${NC} $1"; }
pass() { echo -e "  ${GREEN}✓${NC} $1"; ((PASSED++)); }
fail() { echo -e "  ${RED}✗${NC} $1"; ((FAILED++)); }

cleanup() {
    if [[ -n "$PROXY_PID" ]] && kill -0 "$PROXY_PID" 2>/dev/null; then
        safe_cleanup_proxy "$PROXY_PID"
    fi
    if [[ -n "$TEST_DATA_DIR" && -d "$TEST_DATA_DIR" ]]; then
        rm -rf "$TEST_DATA_DIR"
    fi
    if [[ -n "$BIFROST_LOG_FILE" && -f "$BIFROST_LOG_FILE" ]]; then
        rm -f "$BIFROST_LOG_FILE"
    fi
}

start_bifrost_with_rule() {
    TEST_DATA_DIR="$(mktemp -d)"
    BIFROST_LOG_FILE="$(mktemp)"

    local home_dir="${TEST_DATA_DIR}/home"
    local xdg_config="${TEST_DATA_DIR}/xdg-config"
    local xdg_data="${TEST_DATA_DIR}/xdg-data"
    mkdir -p "$home_dir" "$xdg_config" "$xdg_data"

    SKIP_FRONTEND_BUILD=1 \
        HOME="$home_dir" \
        XDG_CONFIG_HOME="$xdg_config" \
        XDG_DATA_HOME="$xdg_data" \
        BIFROST_DATA_DIR="$TEST_DATA_DIR" \
        "$BIFROST_BIN" -H 127.0.0.1 -p "$PROXY_PORT" start --skip-cert-check --unsafe-ssl \
        >"$BIFROST_LOG_FILE" 2>&1 &
    PROXY_PID=$!

    local waited=0
    while [[ $waited -lt 60 ]]; do
        if curl -s "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/system" >/dev/null 2>&1; then
            info "Bifrost started (PID: $PROXY_PID, port: $PROXY_PORT)"
            local payload
            payload=$(jq -cn --arg name "traffic-test" --arg content "httpbin.org host://httpbin.org" --argjson enabled true \
                '{name:$name, content:$content, enabled:$enabled}')
            curl -s -X POST -H "Content-Type: application/json" -d "$payload" \
                "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/rules" >/dev/null
            sleep 0.3
            return 0
        fi
        if ! kill -0 "$PROXY_PID" 2>/dev/null; then
            fail "Bifrost exited early"
            tail -50 "$BIFROST_LOG_FILE" >&2
            return 1
        fi
        sleep 1
        waited=$((waited + 1))
    done
    fail "Timeout waiting for Bifrost"
    return 1
}

generate_traffic() {
    info "Generating traffic via proxy..."

    curl -s --max-time 10 \
        --proxy "http://127.0.0.1:${PROXY_PORT}" \
        "http://httpbin.org/get?test=table_format" \
        -o /dev/null 2>&1 || true

    curl -s --max-time 10 \
        --proxy "http://127.0.0.1:${PROXY_PORT}" \
        -X POST \
        -H "Content-Type: application/json" \
        -d '{"test":"compact_format"}' \
        "http://httpbin.org/post" \
        -o /dev/null 2>&1 || true

    sleep 1

    local traffic_list
    traffic_list=$(curl -s "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/traffic?limit=10")
    local count
    count=$(echo "$traffic_list" | jq '.records | length' 2>/dev/null || echo "0")

    if [[ "$count" -ge 2 ]]; then
        info "Traffic generated: $count records"
        return 0
    else
        info "Traffic might be insufficient (got $count records), continuing..."
        return 0
    fi
}

get_first_traffic_seq() {
    local traffic_list
    traffic_list=$(curl -s "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/traffic?limit=10")
    echo "$traffic_list" | jq -r '.records[0].seq // empty' 2>/dev/null
}

test_traffic_list_default() {
    info "Test: bifrost traffic list - default output"

    local output
    output=$( \
        BIFROST_DATA_DIR="$TEST_DATA_DIR" \
        "$BIFROST_BIN" -p "$PROXY_PORT" traffic list 2>&1 \
    ) || true

    if [[ -z "$output" ]]; then
        fail "traffic list output is empty"
        return
    fi

    if echo "$output" | grep -qi "panic\|fatal"; then
        fail "traffic list contains panic/fatal: $output"
        return
    fi

    if echo "$output" | grep -q "httpbin.org"; then
        pass "traffic list shows httpbin.org traffic"
    else
        fail "traffic list missing httpbin.org: $output"
    fi
}

test_traffic_get_table_format() {
    info "Test: bifrost traffic get --format table"

    local seq
    seq=$(get_first_traffic_seq)

    if [[ -z "$seq" ]]; then
        fail "No traffic record found for table format test"
        return
    fi

    local output
    output=$( \
        BIFROST_DATA_DIR="$TEST_DATA_DIR" \
        "$BIFROST_BIN" -p "$PROXY_PORT" traffic get "$seq" --format table --request-body --response-body 2>&1 \
    ) || true

    if [[ -z "$output" ]]; then
        fail "traffic get table output is empty"
        return
    fi

    if echo "$output" | grep -qi "panic\|fatal"; then
        fail "traffic get table contains panic/fatal: $output"
        return
    fi

    pass "traffic get --format table does not panic (seq=$seq)"

    if echo "$output" | grep -qi "request\|response\|header\|status\|url\|method"; then
        pass "traffic get table output contains expected sections"
    else
        fail "traffic get table output missing expected sections: $output"
    fi
}

test_traffic_get_compact_format() {
    info "Test: bifrost traffic get --format compact"

    local seq
    seq=$(get_first_traffic_seq)

    if [[ -z "$seq" ]]; then
        fail "No traffic record found for compact format test"
        return
    fi

    local output
    output=$( \
        BIFROST_DATA_DIR="$TEST_DATA_DIR" \
        "$BIFROST_BIN" -p "$PROXY_PORT" traffic get "$seq" --format compact 2>&1 \
    ) || true

    if [[ -z "$output" ]]; then
        fail "traffic get compact output is empty"
        return
    fi

    if echo "$output" | grep -qi "panic\|fatal"; then
        fail "traffic get compact contains panic/fatal: $output"
        return
    fi

    pass "traffic get --format compact does not panic (seq=$seq)"
}

test_traffic_get_json_format() {
    info "Test: bifrost traffic get --format json"

    local seq
    seq=$(get_first_traffic_seq)

    if [[ -z "$seq" ]]; then
        fail "No traffic record found for json format test"
        return
    fi

    local output
    output=$( \
        BIFROST_DATA_DIR="$TEST_DATA_DIR" \
        "$BIFROST_BIN" -p "$PROXY_PORT" traffic get "$seq" --format json 2>&1 \
    ) || true

    if [[ -z "$output" ]]; then
        fail "traffic get json output is empty"
        return
    fi

    if echo "$output" | jq . >/dev/null 2>&1; then
        pass "traffic get --format json produces valid JSON"
    else
        fail "traffic get --format json output is not valid JSON: $output"
    fi

    local url
    url=$(echo "$output" | jq -r '.url // empty' 2>/dev/null)
    if [[ -n "$url" ]]; then
        pass "traffic get json contains 'url' field"
    else
        fail "traffic get json missing 'url' field"
    fi
}

test_traffic_get_with_body_flags() {
    info "Test: bifrost traffic get with --request-body and --response-body"

    local seq
    seq=$(get_first_traffic_seq)

    if [[ -z "$seq" ]]; then
        fail "No traffic record found for body flags test"
        return
    fi

    local output
    output=$( \
        BIFROST_DATA_DIR="$TEST_DATA_DIR" \
        "$BIFROST_BIN" -p "$PROXY_PORT" traffic get "$seq" --request-body --response-body --format table 2>&1 \
    ) || true

    if echo "$output" | grep -qi "panic\|fatal"; then
        fail "traffic get with body flags contains panic: $output"
        return
    fi

    pass "traffic get with --request-body --response-body does not panic"
}

test_traffic_get_nonexistent_id() {
    info "Test: bifrost traffic get with non-existent ID"

    local output
    output=$( \
        BIFROST_DATA_DIR="$TEST_DATA_DIR" \
        "$BIFROST_BIN" -p "$PROXY_PORT" traffic get "99999999" --format table 2>&1 \
    ) || true

    if echo "$output" | grep -qi "panic\|fatal"; then
        fail "traffic get with non-existent ID panicked: $output"
    else
        pass "traffic get with non-existent ID does not panic"
    fi
}

main() {
    trap cleanup EXIT

    echo -e "${BLUE}═══════════════════════════════════════════════════════════════${NC}"
    echo -e "${BLUE}  Bifrost Traffic Detail Format E2E Test${NC}"
    echo -e "${BLUE}═══════════════════════════════════════════════════════════════${NC}"

    if [[ ! -x "$BIFROST_BIN" ]]; then
        fail "Bifrost binary not found at $BIFROST_BIN"
        exit 1
    fi

    if ! start_bifrost_with_rule; then
        exit 1
    fi

    if ! generate_traffic; then
        fail "Failed to generate traffic"
        exit 1
    fi

    test_traffic_list_default
    test_traffic_get_table_format
    test_traffic_get_compact_format
    test_traffic_get_json_format
    test_traffic_get_with_body_flags
    test_traffic_get_nonexistent_id

    echo ""
    echo -e "${BLUE}═══════════════════════════════════════════════════════════════${NC}"
    echo -e "  Results: ${GREEN}${PASSED} passed${NC}, ${RED}${FAILED} failed${NC}"
    echo -e "${BLUE}═══════════════════════════════════════════════════════════════${NC}"

    if [[ $FAILED -gt 0 ]]; then
        exit 1
    fi
}

main "$@"
