#!/bin/bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT


set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

source "$ROOT_DIR/e2e-tests/test_utils/assert.sh"
source "$ROOT_DIR/e2e-tests/test_utils/process.sh"

PROXY_HOST="${PROXY_HOST:-127.0.0.1}"
PROXY_PORT="${PROXY_PORT:-18881}"
ECHO_HTTP_PORT="${ECHO_HTTP_PORT:-13081}"
ECHO_HTTPS_PORT="${ECHO_HTTPS_PORT:-13481}"
ECHO_WS_PORT="${ECHO_WS_PORT:-13281}"
ECHO_WSS_PORT="${ECHO_WSS_PORT:-13291}"
ECHO_SSE_PORT="${ECHO_SSE_PORT:-13381}"
ECHO_PROXY_PORT="${ECHO_PROXY_PORT:-13981}"

BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/release/bifrost}"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi

TEST_DATA_DIR="$ROOT_DIR/.bifrost-e2e-host-path-${PROXY_PORT}-$$"
RULES_FILE="$TEST_DATA_DIR/rules.txt"
PROXY_PID=""

TARGET_HOST="internal.example.test"
TARGET_PREFIX="/labor_cost/static/"
TARGET_FILE="07c1d7e1fb3e13436b958af5f90ec9c8.svg"
TARGET_QUERY="v=1"
HMR_SOURCE_PATH="/labor_cost/static/__webpack_hmr"
HMR_TARGET_PATH="/__webpack_hmr"

HTTP_STATUS=""
HTTP_HEADERS=""
HTTP_BODY=""

cleanup() {
    kill_bifrost_on_port "$PROXY_PORT"
    safe_cleanup_proxy "$PROXY_PID"
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

perform_request() {
    local url="$1"
    local headers_file
    local body_file
    headers_file=$(mktemp)
    body_file=$(mktemp)

    HTTP_STATUS=$(curl -sS -o "$body_file" -D "$headers_file" \
        --proxy "http://${PROXY_HOST}:${PROXY_PORT}" \
        --noproxy "" \
        --connect-timeout 5 \
        --max-time 15 \
        -k \
        "$url" \
        -w '%{http_code}')
    HTTP_HEADERS=$(cat "$headers_file")
    HTTP_BODY=$(cat "$body_file")
    rm -f "$headers_file" "$body_file"
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
        if curl -sf "http://127.0.0.1:${ECHO_HTTP_PORT}/health" >/dev/null 2>&1; then
            _log_pass "HTTP echo server is ready on ${ECHO_HTTP_PORT}"
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done

    echo "Mock servers failed to start" >&2
    exit 1
}

build_bifrost_if_needed() {
    log_section "Building bifrost binary"
    if [[ -x "$BIFROST_BIN" || -f "${BIFROST_BIN}.exe" ]]; then
        _log_pass "Reusing pre-built release binary at $BIFROST_BIN"
        return 0
    fi
    cargo build --release --bin bifrost
    if [[ ! -x "$BIFROST_BIN" && ! -f "${BIFROST_BIN}.exe" ]]; then
        echo "Failed to build bifrost release binary" >&2
        exit 1
    fi
    _log_pass "Built release binary from current workspace"
}

write_rules() {
    mkdir -p "$TEST_DATA_DIR"
    cat > "$RULES_FILE" <<EOF
https://${TARGET_HOST}${HMR_SOURCE_PATH} http://127.0.0.1:${ECHO_HTTP_PORT}${HMR_TARGET_PATH}
https://${TARGET_HOST}${TARGET_PREFIX} http://127.0.0.1:${ECHO_HTTP_PORT}${TARGET_PREFIX}
EOF
}

start_proxy() {
    log_section "Starting proxy"
    export BIFROST_DATA_DIR="$TEST_DATA_DIR"

    "$BIFROST_BIN" --port "$PROXY_PORT" start \
        --skip-cert-check \
        --unsafe-ssl \
        --no-system-proxy \
        --intercept \
        --rules-file "$RULES_FILE" \
        >"$TEST_DATA_DIR/proxy.log" 2>&1 &
    PROXY_PID=$!

    local waited=0
    while [[ $waited -lt 30 ]]; do
        if curl -sk --proxy "http://${PROXY_HOST}:${PROXY_PORT}" --noproxy "" \
            "https://__bifrost_tls_probe.invalid/health" >/dev/null 2>&1; then
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

test_https_host_rule_path_rewrite() {
    log_section "HTTPS host rule path rewrite"
    local url="https://${TARGET_HOST}${TARGET_PREFIX}${TARGET_FILE}?${TARGET_QUERY}"
    perform_request "$url"

    assert_status_2xx "$HTTP_STATUS" "request through rewritten HTTPS host rule should succeed" || return 1

    local actual_path
    actual_path=$(echo "$HTTP_BODY" | jq -r '.request.parsed_path')
    local expected_path="${TARGET_PREFIX}${TARGET_FILE}"
    if [[ "$actual_path" == "$expected_path" ]]; then
        _log_pass "upstream parsed path should preserve single prefix"
    else
        _log_fail "upstream parsed path should preserve single prefix" "$expected_path" "$actual_path"
        return 1
    fi

    local actual_query
    actual_query=$(echo "$HTTP_BODY" | jq -r '.request.query_string')
    if [[ "$actual_query" == "$TARGET_QUERY" ]]; then
        _log_pass "query string should be preserved"
    else
        _log_fail "query string should be preserved" "$TARGET_QUERY" "$actual_query"
        return 1
    fi

    if [[ "$actual_path" == *"${TARGET_PREFIX}${TARGET_PREFIX}"* ]]; then
        _log_fail "path should not duplicate target prefix" "no duplicated prefix" "$actual_path"
        return 1
    fi
    _log_pass "path should not duplicate target prefix"
}

test_https_host_rule_exact_path_without_trailing_slash() {
    log_section "HTTPS exact host rule path rewrite without trailing slash"
    local url="https://${TARGET_HOST}${HMR_SOURCE_PATH}"
    perform_request "$url"

    assert_status_2xx "$HTTP_STATUS" "exact HMR path request through rewritten HTTPS host rule should succeed" || return 1

    local actual_path
    actual_path=$(echo "$HTTP_BODY" | jq -r '.request.parsed_path')
    if [[ "$actual_path" == "$HMR_TARGET_PATH" ]]; then
        _log_pass "upstream parsed path should preserve exact target path without trailing slash"
    else
        _log_fail "upstream parsed path should preserve exact target path without trailing slash" "$HMR_TARGET_PATH" "$actual_path"
        return 1
    fi

    if [[ "$actual_path" == "${HMR_TARGET_PATH}/" ]]; then
        _log_fail "exact target path should not gain a trailing slash" "$HMR_TARGET_PATH" "$actual_path"
        return 1
    fi
    _log_pass "exact target path should not gain a trailing slash"
}

main() {
    start_mock_servers
    build_bifrost_if_needed
    write_rules
    start_proxy
    test_https_host_rule_path_rewrite
    test_https_host_rule_exact_path_without_trailing_slash
}

main "$@"
