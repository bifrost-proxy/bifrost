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
DOWNSTREAM_PROXY_PORT="${DOWNSTREAM_PROXY_PORT:-${ECHO_PROXY_PORT:-${MOCK_ECHO_PROXY_PORT:-$((PROXY_PORT + 7))}}}"
ECHO_HTTP_PORT="${ECHO_HTTP_PORT:-$((13700 + ($$ % 500)))}"
ECHO_HTTPS_PORT="${ECHO_HTTPS_PORT:-$((13743 + ($$ % 500)))}"
SECONDARY_HTTPS_PORT="${SECONDARY_HTTPS_PORT:-$((14743 + ($$ % 500)))}"

if [[ -n "${BIFROST_BIN:-}" ]]; then
    :
elif [[ -x "$PROJECT_ROOT/target/release/bifrost" ]]; then
    BIFROST_BIN="$PROJECT_ROOT/target/release/bifrost"
else
    BIFROST_BIN="$PROJECT_ROOT/target/debug/bifrost"
fi

TEST_DATA_DIR="$PROJECT_ROOT/.bifrost-e2e-socks5-tls-routing-${PROXY_PORT}-$$"
UPSTREAM_DATA_DIR="$TEST_DATA_DIR/upstream"
DOWNSTREAM_DATA_DIR="$TEST_DATA_DIR/downstream"
RULES_FILE="$TEST_DATA_DIR/routing_exceptions.txt"
RULES_TEMPLATE="$E2E_DIR/rules/socks5_tls/routing_exceptions.txt"
PROXY_LOG_FILE="$TEST_DATA_DIR/proxy.log"
DOWNSTREAM_PROXY_LOG_FILE="$TEST_DATA_DIR/downstream-proxy.log"
SECONDARY_HTTPS_LOG_FILE="$TEST_DATA_DIR/secondary-https.log"
PROXY_PID=""
DOWNSTREAM_PROXY_PID=""
SECONDARY_HTTPS_PID=""

cleanup() {
    if [[ -n "${SECONDARY_HTTPS_PID:-}" ]]; then
        kill_pid "$SECONDARY_HTTPS_PID"
        wait "$SECONDARY_HTTPS_PID" 2>/dev/null || true
    fi
    if [[ -n "${PROXY_PID:-}" ]]; then
        safe_cleanup_proxy "$PROXY_PID"
        wait "$PROXY_PID" 2>/dev/null || true
    fi
    if [[ -n "${DOWNSTREAM_PROXY_PID:-}" ]]; then
        safe_cleanup_proxy "$DOWNSTREAM_PROXY_PID"
        wait "$DOWNSTREAM_PROXY_PID" 2>/dev/null || true
    fi
    kill_bifrost_on_port "$PROXY_PORT"
    kill_bifrost_on_port "$SOCKS5_PORT"
    kill_bifrost_on_port "$DOWNSTREAM_PROXY_PORT"
    kill_bifrost_on_port "$SECONDARY_HTTPS_PORT"
    MOCK_SERVERS="http,https" \
    HTTP_PORT="$ECHO_HTTP_PORT" \
    HTTPS_PORT="$ECHO_HTTPS_PORT" \
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
    mkdir -p "$TEST_DATA_DIR"
    MOCK_SERVERS="http,https" \
    HTTP_PORT="$ECHO_HTTP_PORT" \
    HTTPS_PORT="$ECHO_HTTPS_PORT" \
        "$E2E_DIR/mock_servers/start_servers.sh" start-bg
    python3 "$E2E_DIR/mock_servers/https_echo_server.py" "$SECONDARY_HTTPS_PORT" \
        >"$SECONDARY_HTTPS_LOG_FILE" 2>&1 &
    SECONDARY_HTTPS_PID=$!

    local waited=0
    while [[ $waited -lt 30 ]]; do
        if curl -sf "http://127.0.0.1:${ECHO_HTTP_PORT}/health" >/dev/null 2>&1 \
            && curl -skf "https://127.0.0.1:${ECHO_HTTPS_PORT}/health" >/dev/null 2>&1 \
            && curl -skf "https://127.0.0.1:${SECONDARY_HTTPS_PORT}/health" >/dev/null 2>&1; then
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
        "SECONDARY_HTTPS_PORT=${SECONDARY_HTTPS_PORT}" \
        "DOWNSTREAM_PROXY_PORT=${DOWNSTREAM_PROXY_PORT}"
}

wait_for_bifrost_proxy() {
    local port="$1"
    local label="$2"
    local pid="$3"
    local log_file="$4"

    local waited=0
    while [[ $waited -lt 30 ]]; do
        if curl -sf "http://${PROXY_HOST}:${port}/_bifrost/api/proxy/address" >/dev/null 2>&1; then
            _log_pass "${label} is ready on HTTP ${port}"
            return 0
        fi
        if ! kill -0 "$pid" 2>/dev/null; then
            tail -n 200 "$log_file" >&2 || true
            echo "${label} exited unexpectedly" >&2
            exit 1
        fi
        sleep 1
        waited=$((waited + 1))
    done

    tail -n 200 "$log_file" >&2 || true
    echo "Timed out waiting for ${label}" >&2
    exit 1
}

start_downstream_proxy() {
    log_section "Starting downstream Bifrost HTTP proxy"

    if [[ ! -x "$BIFROST_BIN" ]]; then
        echo "Bifrost binary not found at $BIFROST_BIN" >&2
        exit 1
    fi

    mkdir -p "$DOWNSTREAM_DATA_DIR"
    BIFROST_DATA_DIR="$DOWNSTREAM_DATA_DIR" \
    RUST_LOG=bifrost_proxy=debug "$BIFROST_BIN" -p "$DOWNSTREAM_PROXY_PORT" --log-output console,file start \
        --unsafe-ssl \
        --skip-cert-check \
        --no-intercept \
        --no-system-proxy \
        >"$DOWNSTREAM_PROXY_LOG_FILE" 2>&1 &
    DOWNSTREAM_PROXY_PID=$!

    wait_for_bifrost_proxy "$DOWNSTREAM_PROXY_PORT" "Downstream Bifrost proxy" "$DOWNSTREAM_PROXY_PID" "$DOWNSTREAM_PROXY_LOG_FILE"
}

start_proxy() {
    log_section "Starting upstream Bifrost SOCKS5 proxy"

    if [[ ! -x "$BIFROST_BIN" ]]; then
        echo "Bifrost binary not found at $BIFROST_BIN" >&2
        exit 1
    fi

    mkdir -p "$UPSTREAM_DATA_DIR"
    export BIFROST_DATA_DIR="$UPSTREAM_DATA_DIR"
    RUST_LOG=bifrost_proxy=debug "$BIFROST_BIN" -p "$PROXY_PORT" --socks5-port "$SOCKS5_PORT" --log-output console,file start \
        --unsafe-ssl \
        --skip-cert-check \
        --no-intercept \
        --no-system-proxy \
        --rules-file "$RULES_FILE" \
        >"$PROXY_LOG_FILE" 2>&1 &
    PROXY_PID=$!

    wait_for_bifrost_proxy "$PROXY_PORT" "Upstream Bifrost proxy" "$PROXY_PID" "$PROXY_LOG_FILE"
    _log_pass "Upstream SOCKS5 listener is configured on ${SOCKS5_PORT}"
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

curl_http_proxy_https() {
    local url="$1"
    local header_file="$2"
    local body_file="$3"

    curl -skS \
        --proxy "http://${PROXY_HOST}:${PROXY_PORT}" \
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

downstream_tunnel_record_count_for_host() {
    local host="$1"
    curl -sS "http://${PROXY_HOST}:${DOWNSTREAM_PROXY_PORT}/_bifrost/api/traffic?limit=100" \
        | jq -r --arg host "$host" '
            [ .records[]?
              | select((.h // .host // "") == $host)
              | select((.m // .method // "") == "CONNECT")
              | select((.proto // .protocol // "") == "tunnel")
            ] | length
        '
}

assert_downstream_proxy_saw_target() {
    local host="$1"
    local port="$2"
    local before="$3"
    local after

    after="$(downstream_tunnel_record_count_for_host "$host")"
    if [[ "$after" -le "$before" ]]; then
        _log_fail "Downstream proxy records CONNECT tunnel for ${host}" ">${before}" "$after"
        echo "Downstream proxy traffic snapshot:" >&2
        curl -sS "http://${PROXY_HOST}:${DOWNSTREAM_PROXY_PORT}/_bifrost/api/traffic?limit=100" >&2 || true
        echo "" >&2
        return 1
    fi

    if ! grep -F "CONNECT tunnel to ${host}:${port}" "$DOWNSTREAM_PROXY_LOG_FILE" >/dev/null 2>&1 \
        && ! grep -F "CONNECT tunnel established to ${host}:${port}" "$DOWNSTREAM_PROXY_LOG_FILE" >/dev/null 2>&1 \
        && ! grep -F "Received request: CONNECT ${host}:${port}" "$DOWNSTREAM_PROXY_LOG_FILE" >/dev/null 2>&1; then
        _log_fail "Downstream proxy log contains target ${host}:${port}" "present" "absent"
        tail -n 200 "$DOWNSTREAM_PROXY_LOG_FILE" >&2 || true
        return 1
    fi

    _log_pass "Downstream proxy records and logs CONNECT target ${host}:${port}"
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

    if grep -F "TLS interception enabled" "$PROXY_LOG_FILE" \
        | grep -E "(original_host=${host}|for ${host}([[:space:]]|\\())" >/dev/null 2>&1; then
        _log_fail "No TLS interception log for ${host}" "absent" "present"
        return 1
    fi
    _log_pass "No TLS interception log for ${host}"
}

test_proxy_only_rule_stays_passthrough() {
    log_section "TC-S5TRE-01 proxy-only rule stays TLS passthrough"
    : > "$PROXY_LOG_FILE"
    : > "$DOWNSTREAM_PROXY_LOG_FILE"

    local headers body status target_url downstream_before
    headers="$(mktemp)"
    body="$(mktemp)"
    target_url="https://127.0.0.1:${ECHO_HTTPS_PORT}/health"
    downstream_before="$(downstream_tunnel_record_count_for_host "127.0.0.1")"
    status="$(curl_socks_https "$target_url" "$headers" "$body")"

    assert_status "200" "$status" "proxy-only HTTPS request succeeds through SOCKS5"
    assert_no_tls_intercept_for_host "127.0.0.1"
    assert_downstream_proxy_saw_target "127.0.0.1" "$ECHO_HTTPS_PORT" "$downstream_before"

    rm -f "$headers" "$body"
}

test_http_connect_proxy_only_rule_uses_downstream_without_intercept() {
    log_section "TC-S5TRE-02 HTTP CONNECT proxy-only rule uses downstream without TLS intercept"
    : > "$PROXY_LOG_FILE"

    local headers body status target_url downstream_before
    headers="$(mktemp)"
    body="$(mktemp)"
    target_url="https://127.0.0.1:${ECHO_HTTPS_PORT}/health"
    downstream_before="$(downstream_tunnel_record_count_for_host "127.0.0.1")"
    status="$(curl_http_proxy_https "$target_url" "$headers" "$body")"

    assert_status "200" "$status" "proxy-only HTTPS request succeeds through HTTP CONNECT"
    assert_no_tls_intercept_for_host "127.0.0.1"
    assert_downstream_proxy_saw_target "127.0.0.1" "$ECHO_HTTPS_PORT" "$downstream_before"

    rm -f "$headers" "$body"
}

test_pure_wildcard_response_rule_does_not_auto_intercept() {
    log_section "TC-S5TRE-03 pure wildcard response rule does not auto-enable TLS intercept"
    : > "$PROXY_LOG_FILE"

    local headers body status
    headers="$(mktemp)"
    body="$(mktemp)"
    status="$(curl_socks_https "https://localhost:${ECHO_HTTPS_PORT}/health" "$headers" "$body")"

    assert_status "200" "$status" "pure wildcard response-rule HTTPS request succeeds through SOCKS5"
    assert_header_not_exists "X-Bifrost-Wildcard-Auto-TLS" "$(cat "$headers")" \
        "pure wildcard response rule is not applied to TLS passthrough traffic"
    assert_no_tls_intercept_for_host "localhost"

    rm -f "$headers" "$body"
}

test_regex_response_rule_does_not_auto_intercept() {
    log_section "TC-S5TRE-04 regex response rule does not auto-enable TLS intercept"
    : > "$PROXY_LOG_FILE"

    local headers body status
    headers="$(mktemp)"
    body="$(mktemp)"
    status="$(curl_socks_https "https://localhost:${ECHO_HTTPS_PORT}/health" "$headers" "$body")"

    assert_status "200" "$status" "regex response-rule HTTPS request succeeds through SOCKS5"
    assert_header_not_exists "X-Bifrost-Regex-Auto-TLS" "$(cat "$headers")" \
        "regex response rule is not applied to TLS passthrough traffic"
    assert_no_tls_intercept_for_host "localhost"

    rm -f "$headers" "$body"
}

test_content_rule_still_auto_intercepts() {
    log_section "TC-S5TRE-05 content rule still auto-enables TLS intercept"
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

test_plain_domain_path_route_auto_intercepts_and_uses_specific_match() {
    log_section "TC-S5TRE-06 plain-domain path route auto-enables TLS intercept"
    : > "$PROXY_LOG_FILE"

    local headers body status count server_port request_path
    headers="$(mktemp)"
    body="$(mktemp)"
    status="$(curl_http_proxy_https "https://domain-path-auto.local/app/account-center" "$headers" "$body")"

    assert_status "200" "$status" "plain-domain path HTTPS route succeeds through HTTP CONNECT"

    server_port="$(jq -r '.server.port // empty' "$body")"
    if [[ "$server_port" == "$SECONDARY_HTTPS_PORT" ]]; then
        _log_pass "most specific plain-domain path route selects secondary HTTPS upstream"
    else
        _log_fail "most specific plain-domain path route selects secondary HTTPS upstream" "$SECONDARY_HTTPS_PORT" "$server_port"
        cat "$body" >&2 || true
        return 1
    fi

    request_path="$(jq -r '.request.path // empty' "$body")"
    if [[ "$request_path" == "/app/account-center" ]]; then
        _log_pass "TLS-intercepted request preserves inner HTTPS path"
    else
        _log_fail "TLS-intercepted request preserves inner HTTPS path" "/app/account-center" "$request_path"
        return 1
    fi

    count="$(https_record_count_for_host "domain-path-auto.local")"
    if [[ "$count" -gt 0 ]]; then
        _log_pass "HTTPS intercept traffic record exists for plain-domain path route"
    else
        _log_fail "HTTPS intercept traffic record exists for plain-domain path route" ">0" "$count"
        return 1
    fi

    rm -f "$headers" "$body"
}

echo "=============================================="
echo "  TLS Routing Exceptions E2E"
echo "=============================================="

start_mock_servers
write_rules
start_downstream_proxy
start_proxy

test_proxy_only_rule_stays_passthrough
test_http_connect_proxy_only_rule_uses_downstream_without_intercept
test_pure_wildcard_response_rule_does_not_auto_intercept
test_regex_response_rule_does_not_auto_intercept
test_content_rule_still_auto_intercepts
test_plain_domain_path_route_auto_intercepts_and_uses_specific_match

echo ""
echo "All TLS routing exception checks passed."
