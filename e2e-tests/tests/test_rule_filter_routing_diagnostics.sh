#!/bin/bash

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

source "$SCRIPT_DIR/../test_utils/assert.sh"
source "$SCRIPT_DIR/../test_utils/process.sh"
source "$SCRIPT_DIR/../test_utils/rule_fixture.sh"

PROXY_HOST="${PROXY_HOST:-127.0.0.1}"
PROXY_PORT="${PROXY_PORT:-$((19180 + ($$ % 500)))}"
ECHO_HTTP_PORT="${ECHO_HTTP_PORT:-$((14100 + ($$ % 500)))}"
ECHO_HTTPS_PORT="${ECHO_HTTPS_PORT:-$((14543 + ($$ % 500)))}"
ECHO_WS_PORT="${ECHO_WS_PORT:-$((14200 + ($$ % 500)))}"
ECHO_WSS_PORT="${ECHO_WSS_PORT:-$((14250 + ($$ % 500)))}"
ECHO_SSE_PORT="${ECHO_SSE_PORT:-$((14300 + ($$ % 500)))}"
ECHO_PROXY_PORT="${ECHO_PROXY_PORT:-$((14900 + ($$ % 200)))}"

if [[ -n "${BIFROST_BIN:-}" ]]; then
    :
elif [[ -x "$ROOT_DIR/target/release/bifrost" ]]; then
    BIFROST_BIN="$ROOT_DIR/target/release/bifrost"
else
    BIFROST_BIN="$ROOT_DIR/target/debug/bifrost"
fi

TEST_DATA_DIR="$ROOT_DIR/.bifrost-e2e-rule-filter-routing-${PROXY_PORT}-$$"
RULES_FILE="$TEST_DATA_DIR/rules.txt"
RULES_TEMPLATE="$ROOT_DIR/e2e-tests/rules/regression/rule_filter_routing_diagnostics.txt"
PROXY_PID=""

cleanup() {
    safe_cleanup_proxy "$PROXY_PID"
    if [[ -n "$PROXY_PID" ]]; then
        wait "$PROXY_PID" 2>/dev/null || true
    fi
    kill_bifrost_on_port "$PROXY_PORT"
    HTTP_PORT="$ECHO_HTTP_PORT" \
    HTTPS_PORT="$ECHO_HTTPS_PORT" \
    WS_PORT="$ECHO_WS_PORT" \
    WSS_PORT="$ECHO_WSS_PORT" \
    SSE_PORT="$ECHO_SSE_PORT" \
    PROXY_PORT="$ECHO_PROXY_PORT" \
    "$ROOT_DIR/e2e-tests/mock_servers/start_servers.sh" stop >/dev/null 2>&1 || true
    rm -rf "$TEST_DATA_DIR"
}

trap cleanup EXIT

log_section() {
    echo ""
    echo "============================================================"
    echo "$1"
    echo "============================================================"
}

assert_json_expr() {
    local json="$1"
    shift
    local message="${@: -1}"
    set -- "${@:1:$(($# - 1))}"
    local expr="${@: -1}"
    local jq_args=()
    if [[ $# -gt 1 ]]; then
        jq_args=("${@:1:$(($# - 1))}")
    fi

    if [[ ${#jq_args[@]} -gt 0 ]]; then
        echo "$json" | jq -e "${jq_args[@]}" "$expr" >/dev/null
    else
        echo "$json" | jq -e "$expr" >/dev/null
    fi
    if [[ $? -eq 0 ]]; then
        _log_pass "$message"
        return 0
    fi

    _log_fail "$message" "$expr" "$json"
    return 1
}

start_mock_servers() {
    log_section "Starting mock servers"
    HTTP_PORT="$ECHO_HTTP_PORT" \
    HTTPS_PORT="$ECHO_HTTPS_PORT" \
    WS_PORT="$ECHO_WS_PORT" \
    WSS_PORT="$ECHO_WSS_PORT" \
    SSE_PORT="$ECHO_SSE_PORT" \
    PROXY_PORT="$ECHO_PROXY_PORT" \
        "$ROOT_DIR/e2e-tests/mock_servers/start_servers.sh" start-bg

    local waited=0
    while [[ $waited -lt 20 ]]; do
        if curl -sf "http://127.0.0.1:${ECHO_HTTP_PORT}/health" >/dev/null 2>&1 \
            && curl -ksf "https://127.0.0.1:${ECHO_HTTPS_PORT}/health" >/dev/null 2>&1; then
            _log_pass "HTTP echo server is ready on ${ECHO_HTTP_PORT}"
            _log_pass "HTTPS echo server is ready on ${ECHO_HTTPS_PORT}"
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done

    echo "Mock servers failed to start" >&2
    exit 1
}

write_rules() {
    render_rule_fixture_to_file "$RULES_TEMPLATE" "$RULES_FILE" \
        "ECHO_HTTP_PORT=${ECHO_HTTP_PORT}" \
        "ECHO_HTTPS_PORT=${ECHO_HTTPS_PORT}"
}

start_proxy() {
    log_section "Starting proxy"

    if [[ ! -x "$BIFROST_BIN" ]]; then
        echo "Bifrost binary not found at $BIFROST_BIN" >&2
        exit 1
    fi

    export BIFROST_DATA_DIR="$TEST_DATA_DIR"

    "$BIFROST_BIN" --port "$PROXY_PORT" start \
        --skip-cert-check \
        --no-system-proxy \
        --rules-file "$RULES_FILE" \
        >"$TEST_DATA_DIR/proxy.log" 2>&1 &
    PROXY_PID=$!

    local waited=0
    while [[ $waited -lt 30 ]]; do
        if curl -sf --proxy "http://${PROXY_HOST}:${PROXY_PORT}" "http://example.com/" >/dev/null 2>&1; then
            _log_pass "Proxy is ready on ${PROXY_PORT}"
            return 0
        fi
        if ! kill -0 "$PROXY_PID" 2>/dev/null; then
            tail -n 200 "$TEST_DATA_DIR/proxy.log" >&2 || true
            echo "Proxy exited unexpectedly" >&2
            exit 1
        fi
        sleep 1
        waited=$((waited + 1))
    done

    tail -n 200 "$TEST_DATA_DIR/proxy.log" >&2 || true
    echo "Timed out waiting for proxy" >&2
    exit 1
}

request_account_center() {
    curl -sS \
        --proxy "http://${PROXY_HOST}:${PROXY_PORT}" \
        --connect-timeout 5 \
        --max-time 15 \
        "http://filter-regression.local/account-center/account-assistant?from=e2e"
}

request_qianchuan_path() {
    local path="$1"
    curl -sS \
        --proxy "http://${PROXY_HOST}:${PROXY_PORT}" \
        --connect-timeout 5 \
        --max-time 15 \
        "http://qianchuan.jinritemai.com${path}"
}

request_unsafe_upstream() {
    curl -sS \
        --proxy "http://${PROXY_HOST}:${PROXY_PORT}" \
        --connect-timeout 5 \
        --max-time 15 \
        "http://unsafe-upstream.local/unsafe-cert?from=e2e"
}

request_strict_upstream_headers() {
    curl -sS -i \
        --proxy "http://${PROXY_HOST}:${PROXY_PORT}" \
        --connect-timeout 5 \
        --max-time 15 \
        "http://strict-upstream.local/unsafe-cert?from=e2e"
}

latest_record_id_for_url() {
    local url="$1"
    local path="${url#*filter-regression.local}"
    path="${path%%\?*}"
    local waited=0
    local record_id=""

    while [[ $waited -lt 20 ]]; do
        record_id=$(curl -sS "http://${PROXY_HOST}:${PROXY_PORT}/_bifrost/api/traffic?limit=50" \
            | jq -r --arg host "filter-regression.local" --arg path "$path" \
                '.records[]? | select(.h == $host and .p == $path) | .id' \
            | head -1)
        if [[ -n "$record_id" && "$record_id" != "null" ]]; then
            printf '%s' "$record_id"
            return 0
        fi
        sleep 0.5
        waited=$((waited + 1))
    done

    return 1
}

test_path_filter_prefix_match() {
    log_section "Path filter prefix match"

    local body
    body="$(request_account_center)" || return 1

    assert_json_expr "$body" '.request.parsed_path == "/account-center/account-assistant"' \
        "includeFilter:///account should match /account-center" || return 1
}

test_long_exclude_filter_chain_prefix_match() {
    log_section "Long excludeFilter chain prefix match"

    local paths=(
        "/account-center/account-assistant?from=e2e"
        "/garrmodlistv3-extra/list?from=e2e"
        "/apiary/resource?from=e2e"
        "/cg_project/backend/open-extra?from=e2e"
        "/star-pages/deep/link?from=e2e"
    )

    local path body
    for path in "${paths[@]}"; do
        body="$(request_qianchuan_path "$path")" || return 1
        assert_json_expr "$body" --arg expected_path "${path%%\?*}" '.request.parsed_path == $expected_path' \
            "excludeFilter chain should fall through for ${path}" || return 1
    done
}

test_per_rule_upstream_unsafe_ssl() {
    log_section "Per-rule upstream unsafe SSL"

    local body strict_response
    body="$(request_unsafe_upstream)" || return 1
    assert_json_expr "$body" '.request.parsed_path == "/unsafe-cert"' \
        "upstreamUnsafeSsl://true should allow self-signed HTTPS upstream" || return 1

    strict_response="$(request_strict_upstream_headers)" || return 1
    assert_body_contains " 502 " "$strict_response" \
        "same HTTPS upstream without upstreamUnsafeSsl should fail certificate verification" || return 1
    assert_body_contains "X-Bifrost-Error" "$strict_response" \
        "strict HTTPS upstream failure should be reported as a proxy error" || return 1
    assert_body_contains "upstreamUnsafeSsl://true" "$strict_response" \
        "strict HTTPS upstream failure should suggest the per-rule unsafe SSL operation" || return 1
}

test_traffic_detail_keeps_routing_diagnostics() {
    log_section "Traffic detail keeps routing diagnostics"

    local url="http://filter-regression.local/account-center/account-assistant?from=e2e"
    local record_id detail
    record_id="$(latest_record_id_for_url "$url")" || {
        _log_fail "traffic record should be written" "$url" "missing"
        return 1
    }

    detail="$(curl -sS "http://${PROXY_HOST}:${PROXY_PORT}/_bifrost/api/traffic/${record_id}")" || return 1
    assert_json_expr "$detail" '.has_rule_hit == true' \
        "traffic detail records the rule hit" || return 1
    assert_json_expr "$detail" --arg host "127.0.0.1" '.actual_host == $host' \
        "traffic detail records the upstream host" || return 1
    assert_json_expr "$detail" --arg port ":${ECHO_HTTP_PORT}" '.actual_url | contains($port)' \
        "traffic detail records the upstream port in actual_url" || return 1
}

test_network_export_keeps_routing_diagnostics() {
    log_section "Network export keeps routing diagnostics"

    local url="http://filter-regression.local/account-center/account-assistant?from=e2e"
    local record_id export_response records_json
    record_id="$(latest_record_id_for_url "$url")" || {
        _log_fail "traffic record should be available for export" "$url" "missing"
        return 1
    }

    export_response="$(curl -sS -X POST \
        -H "Content-Type: application/json" \
        -d "{\"record_ids\":[\"${record_id}\"],\"include_body\":true,\"description\":\"routing diagnostics regression\"}" \
        "http://${PROXY_HOST}:${PROXY_PORT}/_bifrost/api/bifrost-file/export/network")" || return 1
    records_json="$(printf '%s\n' "$export_response" | awk 'found { print } /^---[[:space:]]*$/ { found = 1 }')"

    assert_json_expr "$records_json" '.[0].has_rule_hit == true' \
        "network export records has_rule_hit" || return 1
    assert_json_expr "$records_json" --arg host "127.0.0.1" '.[0].actual_host == $host' \
        "network export records actual_host" || return 1
    assert_json_expr "$records_json" --arg port ":${ECHO_HTTP_PORT}" '.[0].actual_url | contains($port)' \
        "network export records actual_url with upstream port" || return 1
    assert_json_expr "$records_json" --argjson port "$PROXY_PORT" '.[0].listener_port == $port' \
        "network export records listener_port" || return 1
}

main() {
    start_mock_servers
    write_rules
    start_proxy

    test_path_filter_prefix_match
    test_long_exclude_filter_chain_prefix_match
    test_per_rule_upstream_unsafe_ssl
    test_traffic_detail_keeps_routing_diagnostics
    test_network_export_keeps_routing_diagnostics

    echo ""
    echo "Assertions: ${PASSED_ASSERTIONS}/${TOTAL_ASSERTIONS} passed"
    if [[ "${FAILED_ASSERTIONS}" -gt 0 ]]; then
        echo "Rule filter routing diagnostics regressions detected: ${FAILED_ASSERTIONS} assertion(s) failed."
        exit 1
    fi

    echo "All rule filter routing diagnostics regression tests passed."
}

main "$@"
