#!/bin/bash

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

source "${PROJECT_DIR}/e2e-tests/test_utils/process.sh"

BIFROST_BIN="${PROJECT_DIR}/target/release/bifrost"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi

PROXY_PORT="${PROXY_PORT:-18822}"
ADMIN_BASE="http://127.0.0.1:${PROXY_PORT}/_bifrost"
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

start_bifrost() {
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
        if curl -s "${ADMIN_BASE}/api/system" >/dev/null 2>&1; then
            info "Bifrost started (PID: $PROXY_PID, port: $PROXY_PORT)"
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

create_api_rule() {
    local name="$1"
    local content="$2"
    local enabled="${3:-true}"
    local payload
    payload=$(jq -cn --arg name "$name" --arg content "$content" --argjson enabled "$enabled" \
        '{name:$name, content:$content, enabled:$enabled}')
    curl -s -X POST -H "Content-Type: application/json" -d "$payload" "${ADMIN_BASE}/api/rules" >/dev/null
}

delete_api_rule() {
    local name="$1"
    curl -s -X DELETE "${ADMIN_BASE}/api/rules/${name}" >/dev/null 2>&1
}

setup_rules() {
    info "Creating test rules via admin API..."
    create_api_rule "test-rule-a" "example.com host://127.0.0.1:3000" "true"
    create_api_rule "test-rule-b" "$(printf 'api.test.com host://127.0.0.1:4000\nstatic.test.com host://127.0.0.1:5000')" "true"
    sleep 0.5
}

cleanup_rules() {
    delete_api_rule "test-rule-a"
    delete_api_rule "test-rule-b"
}

run_rule_active() {
    BIFROST_DATA_DIR="$TEST_DATA_DIR" \
        "$BIFROST_BIN" -p "$PROXY_PORT" rule active 2>&1
}

test_rule_active_basic_output() {
    info "Test: bifrost rule active - basic output"

    local output
    output=$(run_rule_active) || true

    if [[ -z "$output" ]]; then
        fail "rule active output is empty"
        return
    fi

    if echo "$output" | grep -qi "panic\|fatal"; then
        fail "rule active output contains panic/fatal: $output"
        return
    fi

    if echo "$output" | grep -q "test-rule-a"; then
        pass "rule active output contains rule name 'test-rule-a'"
    else
        fail "rule active output missing rule name 'test-rule-a': $output"
    fi

    if echo "$output" | grep -q "test-rule-b"; then
        pass "rule active output contains rule name 'test-rule-b'"
    else
        fail "rule active output missing rule name 'test-rule-b': $output"
    fi
}

test_rule_active_shows_merged_content() {
    info "Test: bifrost rule active - shows merged content with domain names"

    local output
    output=$(run_rule_active) || true

    if echo "$output" | grep -q "example.com"; then
        pass "rule active output includes domain 'example.com'"
    else
        fail "rule active output missing domain 'example.com': $output"
    fi

    if echo "$output" | grep -q "api.test.com"; then
        pass "rule active output includes domain 'api.test.com'"
    else
        fail "rule active output missing domain 'api.test.com': $output"
    fi
}

test_rule_active_shows_rule_count() {
    info "Test: bifrost rule active - shows rule count"

    local output
    output=$(run_rule_active) || true

    if echo "$output" | grep -qE "2|total|rules"; then
        pass "rule active output references rule count or total"
    else
        fail "rule active output missing count information: $output"
    fi
}

test_rule_active_no_panic_on_empty() {
    info "Test: bifrost rule active - no panic on server with no rules"

    cleanup_rules
    sleep 0.3

    local output
    output=$(run_rule_active) || true

    if echo "$output" | grep -qi "panic"; then
        fail "rule active panicked on empty server: $output"
    else
        pass "rule active does not panic on empty server"
    fi

    if echo "$output" | grep -qi "no active rules\|0"; then
        pass "rule active correctly reports no rules"
    else
        fail "rule active should indicate no rules: $output"
    fi

    setup_rules
}

test_rule_active_disabled_rule_excluded() {
    info "Test: bifrost rule active - disabled rule excluded"

    create_api_rule "disabled-rule" "disabled.com host://127.0.0.1:6000" "false"
    sleep 0.3

    local output
    output=$(run_rule_active) || true

    if echo "$output" | grep -q "disabled.com"; then
        fail "rule active should NOT show disabled rule content: $output"
    else
        pass "rule active correctly excludes disabled rule content"
    fi

    delete_api_rule "disabled-rule"
}

main() {
    trap cleanup EXIT

    echo -e "${BLUE}═══════════════════════════════════════════════════════════════${NC}"
    echo -e "${BLUE}  Bifrost Rule Active CLI E2E Test${NC}"
    echo -e "${BLUE}═══════════════════════════════════════════════════════════════${NC}"

    if [[ ! -x "$BIFROST_BIN" ]]; then
        fail "Bifrost binary not found at $BIFROST_BIN"
        exit 1
    fi

    if ! start_bifrost; then
        exit 1
    fi

    setup_rules

    test_rule_active_basic_output
    test_rule_active_shows_merged_content
    test_rule_active_shows_rule_count
    test_rule_active_no_panic_on_empty
    test_rule_active_disabled_rule_excluded

    cleanup_rules

    echo ""
    echo -e "${BLUE}═══════════════════════════════════════════════════════════════${NC}"
    echo -e "  Results: ${GREEN}${PASSED} passed${NC}, ${RED}${FAILED} failed${NC}"
    echo -e "${BLUE}═══════════════════════════════════════════════════════════════${NC}"

    if [[ $FAILED -gt 0 ]]; then
        exit 1
    fi
}

main "$@"
