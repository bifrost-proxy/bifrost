#!/bin/bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
E2E_DIR="$(dirname "$SCRIPT_DIR")"
PROJECT_ROOT="$(dirname "$E2E_DIR")"

source "$E2E_DIR/test_utils/assert.sh"
source "$E2E_DIR/test_utils/process.sh"
source "$E2E_DIR/test_utils/rule_fixture.sh"

PROXY_HOST="${PROXY_HOST:-127.0.0.1}"
PROXY_PORT="${PROXY_PORT:-$((18780 + ($$ % 500)))}"
SOCKS5_PORT="${SOCKS5_PORT:-$((18781 + ($$ % 500)))}"
ECHO_HTTP_PORT="${ECHO_HTTP_PORT:-$((13700 + ($$ % 500)))}"
ECHO_HTTPS_PORT="${ECHO_HTTPS_PORT:-$((13743 + ($$ % 500)))}"
ECHO_PROXY_PORT="${ECHO_PROXY_PORT:-$((13900 + ($$ % 500)))}"

if [[ -n "${BIFROST_BIN:-}" ]]; then
    :
elif [[ -x "$PROJECT_ROOT/target/release/bifrost" ]]; then
    BIFROST_BIN="$PROJECT_ROOT/target/release/bifrost"
else
    BIFROST_BIN="$PROJECT_ROOT/target/debug/bifrost"
fi

TEST_DATA_DIR="$PROJECT_ROOT/.bifrost-e2e-socks5-tls-routing-${PROXY_PORT}-$$"
RULES_FILE="$TEST_DATA_DIR/routing_exceptions.txt"
RULES_TEMPLATE="$E2E_DIR/rules/socks5_tls/routing_exceptions.txt"
PROXY_LOG_FILE="$TEST_DATA_DIR/proxy.log"
PROXY_PID=""

cleanup() {
    if [[ -n "${PROXY_PID:-}" ]]; then
        safe_cleanup_proxy "$PROXY_PID"
        wait "$PROXY_PID" 2>/dev/null || true
    fi
    kill_bifrost_on_port "$PROXY_PORT"
    kill_bifrost_on_port "$SOCKS5_PORT"
    MOCK_SERVERS="http,https,proxy" \
    HTTP_PORT="$ECHO_HTTP_PORT" \
    HTTPS_PORT="$ECHO_HTTPS_PORT" \
    PROXY_PORT="$ECHO_PROXY_PORT" \
        "$E2E_DIR/mock_servers/start_servers.sh" stop >/dev/null 2>&1 || true
    rm -rf "$TEST_DATA_DIR"
}

trap cleanup EXIT

log_section() {
    echo ""
    echo "============================================================"
    echo "$1"
    echo "============================================================"
}

start_mock_servers() {
    log_section "Starting mock servers"
    MOCK_SERVERS="http,https,proxy" \
    HTTP_PORT="$ECHO_HTTP_PORT" \
    HTTPS_PORT="$ECHO_HTTPS_PORT" \
    PROXY_PORT="$ECHO_PROXY_PORT" \
        "$E2E_DIR/mock_servers/start_servers.sh" start-bg

    local waited=0
    while [[ $waited -lt 30 ]]; do
        if curl -sf "http://127.0.0.1:${ECHO_HTTP_PORT}/health" >/dev/null 2>&1 \
            && curl -skf "https://127.0.0.1:${ECHO_HTTPS_PORT}/health" >/dev/null 2>&1; then
            _log_pass "Mock HTTP/HTTPS servers are ready"
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done

    echo "Mock servers failed to start" >&2
    exit 1
}

write_rules() {
    mkdir -p "$TEST_DATA_DIR"
    render_rule_fixture_to_file "$RULES_TEMPLATE" "$RULES_FILE" \
        "ECHO_HTTP_PORT=${ECHO_HTTP_PORT}" \
        "ECHO_HTTPS_PORT=${ECHO_HTTPS_PORT}" \
        "ECHO_PROXY_PORT=${ECHO_PROXY_PORT}"
}

start_proxy() {
    log_section "Starting Bifrost"

    if [[ ! -x "$BIFROST_BIN" ]]; then
        echo "Bifrost binary not found at $BIFROST_BIN" >&2
        exit 1
    fi

    export BIFROST_DATA_DIR="$TEST_DATA_DIR"

    RUST_LOG=bifrost_proxy=debug "$BIFROST_BIN" -p "$PROXY_PORT" --socks5-port "$SOCKS5_PORT" start \
        --unsafe-ssl \
        --skip-cert-check \
        --no-intercept \
        --no-system-proxy \
        --rules-file "$RULES_FILE" \
        >"$PROXY_LOG_FILE" 2>&1 &
    PROXY_PID=$!

    local waited=0
    while [[ $waited -lt 30 ]]; do
        if curl -sf "http://${PROXY_HOST}:${PROXY_PORT}/_bifrost/api/proxy/address" >/dev/null 2>&1; then
            _log_pass "Bifrost proxy is ready on HTTP ${PROXY_PORT}, SOCKS5 ${SOCKS5_PORT}"
            return 0
        fi
        if ! kill -0 "$PROXY_PID" 2>/dev/null; then
            tail -n 200 "$PROXY_LOG_FILE" >&2 || true
            echo "Proxy exited unexpectedly" >&2
            exit 1
        fi
        sleep 1
        waited=$((waited + 1))
    done

    tail -n 200 "$PROXY_LOG_FILE" >&2 || true
    echo "Timed out waiting for proxy" >&2
    exit 1
}

curl_socks_https() {
    local url="$1"
    local header_file="$2"
    local body_file="$3"

    curl -skS \
        --socks5-hostname "${PROXY_HOST}:${SOCKS5_PORT}" \
        --noproxy "" \
        --connect-timeout 5 \
        --max-time 20 \
        -D "$header_file" \
        -o "$body_file" \
        -w "%{http_code}" \
        "$url"
}

https_record_count_for_host() {
    local host="$1"
    curl -sS "http://${PROXY_HOST}:${PROXY_PORT}/_bifrost/api/traffic?limit=100" \
        | jq -r --arg host "$host" '
            [ .records[]?
              | select((.h // .host // "") == $host)
              | select((.proto // .protocol // "") == "https")
            ] | length
        '
}

assert_no_tls_intercept_for_host() {
    local host="$1"
    local count
    count="$(https_record_count_for_host "$host")"
    if [[ "$count" == "0" ]]; then
        _log_pass "No HTTPS intercept traffic record for ${host}"
    else
        _log_fail "No HTTPS intercept traffic record for ${host}" "0" "$count"
        return 1
    fi

    if grep -F "TLS interception enabled" "$PROXY_LOG_FILE" | grep -F "original_host=${host}" >/dev/null 2>&1; then
        _log_fail "No SOCKS5 TLS interception log for ${host}" "absent" "present"
        return 1
    fi
    _log_pass "No SOCKS5 TLS interception log for ${host}"
}

test_host_only_rule_stays_passthrough() {
    log_section "TC-S5TRE-01 host-only rule stays TLS passthrough"
    : > "$PROXY_LOG_FILE"

    local headers body status
    headers="$(mktemp)"
    body="$(mktemp)"
    status="$(curl_socks_https "https://socks-host-only.local/health" "$headers" "$body")"

    assert_status "200" "$status" "host-only HTTPS request succeeds through SOCKS5"
    assert_no_tls_intercept_for_host "socks-host-only.local"

    rm -f "$headers" "$body"
}

test_proxy_only_rule_stays_passthrough() {
    log_section "TC-S5TRE-02 proxy-only rule stays TLS passthrough"
    : > "$PROXY_LOG_FILE"

    local headers body status target_url
    headers="$(mktemp)"
    body="$(mktemp)"
    target_url="https://127.0.0.1:${ECHO_HTTPS_PORT}/health"
    status="$(curl_socks_https "$target_url" "$headers" "$body")"

    assert_status "200" "$status" "proxy-only HTTPS request succeeds through SOCKS5"
    assert_no_tls_intercept_for_host "127.0.0.1"

    rm -f "$headers" "$body"
}

test_content_rule_still_auto_intercepts() {
    log_section "TC-S5TRE-03 content rule still auto-enables TLS intercept"
    : > "$PROXY_LOG_FILE"

    local headers body status count
    headers="$(mktemp)"
    body="$(mktemp)"
    status="$(curl_socks_https "https://socks-mutation.local/health" "$headers" "$body")"

    assert_status "200" "$status" "content-rule HTTPS request succeeds through SOCKS5"
    assert_header_value "X-Bifrost-Auto-TLS" "socks5-content-rule" "$(cat "$headers")" \
        "content rule response header is applied after TLS interception"

    count="$(https_record_count_for_host "socks-mutation.local")"
    if [[ "$count" -gt 0 ]]; then
        _log_pass "HTTPS intercept traffic record exists for socks-mutation.local"
    else
        _log_fail "HTTPS intercept traffic record exists for socks-mutation.local" ">0" "$count"
        return 1
    fi

    rm -f "$headers" "$body"
}

echo "=============================================="
echo "  SOCKS5 TLS Routing Exceptions E2E"
echo "=============================================="

start_mock_servers
write_rules
start_proxy

test_host_only_rule_stays_passthrough
test_proxy_only_rule_stays_passthrough
test_content_rule_still_auto_intercepts

echo ""
echo "All SOCKS5 TLS routing exception checks passed."
