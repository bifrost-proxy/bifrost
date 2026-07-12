#!/bin/bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

source "$ROOT_DIR/e2e-tests/test_utils/assert.sh"
source "$ROOT_DIR/e2e-tests/test_utils/process.sh"
source "$ROOT_DIR/e2e-tests/test_utils/rule_fixture.sh"

PROXY_HOST="${PROXY_HOST:-127.0.0.1}"
if [[ -z "${ENTRY_PORT:-}" || -z "${UPSTREAM_PORT:-}" || -z "${ECHO_HTTP_PORT:-}" || -z "${PROXY_ECHO_PORT:-}" ]]; then
    PORT_BASE="$(pick_available_base_port "${BIFROST_E2E_PROXY_CHAIN_BASE_PORT:-0}" 4)"
    if [[ -z "$PORT_BASE" || "$PORT_BASE" == "0" ]]; then
        echo "failed to allocate proxy chain e2e ports" >&2
        exit 1
    fi
    ENTRY_PORT="${ENTRY_PORT:-$PORT_BASE}"
    UPSTREAM_PORT="${UPSTREAM_PORT:-$((PORT_BASE + 1))}"
    ECHO_HTTP_PORT="${ECHO_HTTP_PORT:-$((PORT_BASE + 2))}"
    PROXY_ECHO_PORT="${PROXY_ECHO_PORT:-$((PORT_BASE + 3))}"
fi

BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/release/bifrost}"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi

PYTHON_BIN="$(python_cmd)"
ENTRY_RULES_TEMPLATE="$ROOT_DIR/e2e-tests/rules/forwarding/proxy_chain_entry_auth.txt"
UPSTREAM_RULES_TEMPLATE="$ROOT_DIR/e2e-tests/rules/forwarding/proxy_chain_upstream_host.txt"
HTTP_ECHO_SERVER="$ROOT_DIR/e2e-tests/mock_servers/http_echo_server.py"
PROXY_ECHO_SERVER="$ROOT_DIR/e2e-tests/mock_servers/proxy_echo_server.py"

TEST_ROOT_DIR=""
ENTRY_DATA_DIR=""
UPSTREAM_DATA_DIR=""
ENTRY_RULES_FILE=""
UPSTREAM_RULES_FILE=""

ENTRY_PID=""
UPSTREAM_PID=""
HTTP_ECHO_PID=""
PROXY_ECHO_PID=""

HTTP_STATUS=""
HTTP_HEADERS=""
HTTP_BODY=""

cleanup() {
    kill_bifrost_on_port "$ENTRY_PORT"
    kill_bifrost_on_port "$UPSTREAM_PORT"
    safe_cleanup_proxy "$ENTRY_PID"
    safe_cleanup_proxy "$UPSTREAM_PID"
    kill_pid "$HTTP_ECHO_PID"
    kill_pid "$PROXY_ECHO_PID"
    wait_pid "$ENTRY_PID"
    wait_pid "$UPSTREAM_PID"
    wait_pid "$HTTP_ECHO_PID"
    wait_pid "$PROXY_ECHO_PID"
    if [[ -n "$TEST_ROOT_DIR" && -d "$TEST_ROOT_DIR" ]]; then
        rm -rf "$TEST_ROOT_DIR"
    fi
}

trap cleanup EXIT

log_section() {
    echo ""
    echo "============================================================"
    echo "$1"
    echo "============================================================"
}

ensure_dependencies() {
    if [[ ! -x "$BIFROST_BIN" ]]; then
        echo "missing bifrost binary: $BIFROST_BIN" >&2
        exit 1
    fi
    if [[ ! -f "$HTTP_ECHO_SERVER" || ! -f "$PROXY_ECHO_SERVER" ]]; then
        echo "missing mock server scripts" >&2
        exit 1
    fi
    if ! command -v curl >/dev/null 2>&1; then
        echo "missing curl" >&2
        exit 1
    fi
}

wait_for_http_service() {
    local url="$1"
    local name="$2"
    local pid="$3"
    local log_file="$4"
    local waited=0
    while [[ $waited -lt 30 ]]; do
        if curl -fsS "$url" >/dev/null 2>&1; then
            _log_pass "$name is ready"
            return 0
        fi
        if [[ -n "$pid" ]] && ! kill -0 "$pid" 2>/dev/null; then
            _log_fail "$name is running" "running process" "exited early"
            [[ -f "$log_file" ]] && tail -n 200 "$log_file" >&2
            return 1
        fi
        sleep 1
        waited=$((waited + 1))
    done
    _log_fail "$name is ready" "$url" "timeout"
    [[ -f "$log_file" ]] && tail -n 200 "$log_file" >&2
    return 1
}

wait_for_bifrost_ready() {
    local port="$1"
    local pid="$2"
    local log_file="$3"
    local waited=0
    while [[ $waited -lt 40 ]]; do
        if command -v lsof >/dev/null 2>&1 && ! lsof -nP -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1; then
            :
        elif curl -fsS "http://${PROXY_HOST}:${port}/_bifrost/api/proxy/address" >/dev/null 2>&1; then
            _log_pass "bifrost on ${port} is ready"
            return 0
        fi
        if [[ -n "$pid" ]] && ! kill -0 "$pid" 2>/dev/null; then
            _log_fail "bifrost on ${port} is running" "running process" "exited early"
            [[ -f "$log_file" ]] && tail -n 200 "$log_file" >&2
            return 1
        fi
        sleep 1
        waited=$((waited + 1))
    done
    _log_fail "bifrost on ${port} is ready" "admin api ready" "timeout"
    [[ -f "$log_file" ]] && tail -n 200 "$log_file" >&2
    return 1
}

perform_request() {
    local url="$1"
    shift || true

    local headers_file
    local body_file
    headers_file=$(mktemp)
    body_file=$(mktemp)

    HTTP_STATUS=$(curl -sS -o "$body_file" -D "$headers_file" \
        --proxy "http://${PROXY_HOST}:${ENTRY_PORT}" \
        --connect-timeout 5 \
        --max-time 20 \
        "$@" \
        "$url" \
        -w '%{http_code}')
    HTTP_HEADERS=$(cat "$headers_file")
    HTTP_BODY=$(cat "$body_file")
    rm -f "$headers_file" "$body_file"
}

dump_diagnostics() {
    echo "" >&2
    echo "---- proxy chain diagnostics ----" >&2
    echo "entry_port=${ENTRY_PORT} upstream_port=${UPSTREAM_PORT} echo_http_port=${ECHO_HTTP_PORT} proxy_echo_port=${PROXY_ECHO_PORT}" >&2
    echo "HTTP status: ${HTTP_STATUS:-<empty>}" >&2
    if [[ -n "${HTTP_HEADERS:-}" ]]; then
        echo "HTTP headers:" >&2
        printf '%s\n' "$HTTP_HEADERS" >&2
    fi
    if [[ -n "${HTTP_BODY:-}" ]]; then
        echo "HTTP body:" >&2
        printf '%s\n' "$HTTP_BODY" >&2
    fi
    for log_file in \
        "${TEST_ROOT_DIR}/entry-bifrost.log" \
        "${TEST_ROOT_DIR}/upstream-bifrost.log" \
        "${TEST_ROOT_DIR}/http_echo.log" \
        "${TEST_ROOT_DIR}/proxy_echo.log"; do
        if [[ -f "$log_file" ]]; then
            echo "" >&2
            echo "---- tail ${log_file} ----" >&2
            tail -n 120 "$log_file" >&2 || true
        fi
    done
}

prepare_workspace() {
    TEST_ROOT_DIR="$(mktemp -d "${ROOT_DIR}/.bifrost-e2e-proxy-chain.XXXXXX")"
    ENTRY_DATA_DIR="${TEST_ROOT_DIR}/entry-data"
    UPSTREAM_DATA_DIR="${TEST_ROOT_DIR}/upstream-data"
    ENTRY_RULES_FILE="${TEST_ROOT_DIR}/entry.rules.txt"
    UPSTREAM_RULES_FILE="${TEST_ROOT_DIR}/upstream.rules.txt"
    mkdir -p "$ENTRY_DATA_DIR" "$UPSTREAM_DATA_DIR"
}

write_rules() {
    render_rule_fixture_to_file "$ENTRY_RULES_TEMPLATE" "$ENTRY_RULES_FILE" \
        "UPSTREAM_BIFROST_PORT=${UPSTREAM_PORT}" \
        "PROXY_ECHO_PORT=${PROXY_ECHO_PORT}"
    render_rule_fixture_to_file "$UPSTREAM_RULES_TEMPLATE" "$UPSTREAM_RULES_FILE" \
        "ECHO_HTTP_PORT=${ECHO_HTTP_PORT}"
}

start_http_echo() {
    local log_file="${TEST_ROOT_DIR}/http_echo.log"
    "$PYTHON_BIN" "$HTTP_ECHO_SERVER" "$ECHO_HTTP_PORT" >"$log_file" 2>&1 &
    HTTP_ECHO_PID=$!
    wait_for_http_service "http://127.0.0.1:${ECHO_HTTP_PORT}/health" "http echo server" "$HTTP_ECHO_PID" "$log_file"
}

start_proxy_echo() {
    local log_file="${TEST_ROOT_DIR}/proxy_echo.log"
    "$PYTHON_BIN" "$PROXY_ECHO_SERVER" "$PROXY_ECHO_PORT" >"$log_file" 2>&1 &
    PROXY_ECHO_PID=$!
    wait_for_http_service "http://127.0.0.1:${PROXY_ECHO_PORT}/health" "proxy echo server" "$PROXY_ECHO_PID" "$log_file"
}

start_upstream_bifrost() {
    local log_file="${TEST_ROOT_DIR}/upstream-bifrost.log"
    BIFROST_DATA_DIR="$UPSTREAM_DATA_DIR" \
        "$BIFROST_BIN" --port "$UPSTREAM_PORT" start \
        --skip-cert-check \
        --unsafe-ssl --no-system-proxy \
        --rules-file "$UPSTREAM_RULES_FILE" \
        >"$log_file" 2>&1 &
    UPSTREAM_PID=$!
    wait_for_bifrost_ready "$UPSTREAM_PORT" "$UPSTREAM_PID" "$log_file"
}

start_entry_bifrost() {
    local log_file="${TEST_ROOT_DIR}/entry-bifrost.log"
    BIFROST_DATA_DIR="$ENTRY_DATA_DIR" \
        "$BIFROST_BIN" --port "$ENTRY_PORT" start \
        --skip-cert-check \
        --unsafe-ssl --no-system-proxy \
        --rules-file "$ENTRY_RULES_FILE" \
        >"$log_file" 2>&1 &
    ENTRY_PID=$!
    wait_for_bifrost_ready "$ENTRY_PORT" "$ENTRY_PID" "$log_file"
}

test_bifrost_proxy_chain() {
    log_section "双 Bifrost 代理链路"
    perform_request "http://chain.test/chain?via=entry"
    assert_status_2xx "$HTTP_STATUS" "双代理链路请求成功" || {
        dump_diagnostics
        return 1
    }
    assert_body_contains '"parsed_path": "/chain"' "$HTTP_BODY" "上游 Bifrost 继续转发到最终 echo 服务" || return 1
    assert_body_contains '"query_string": "via=entry"' "$HTTP_BODY" "双代理链路保留查询参数" || return 1
}

test_downstream_proxy_auth() {
    log_section "下游代理鉴权"
    perform_request "http://auth-proxy.test/auth?hello=1"
    assert_status_2xx "$HTTP_STATUS" "下游代理鉴权请求成功" || return 1
    assert_body_contains '"raw_path": "http://auth-proxy.test/auth?hello=1"' "$HTTP_BODY" "下游代理收到 absolute-form URL" || return 1
    assert_body_contains '"proxy-authorization": "Basic dXNlcjpwYXNz"' "$HTTP_BODY" "下游代理收到 Proxy-Authorization" || return 1
    assert_body_contains '"host": "auth-proxy.test"' "$HTTP_BODY" "下游代理识别原始目标主机" || return 1
}

main() {
    ensure_dependencies
    prepare_workspace
    write_rules

    start_http_echo || exit 1
    start_proxy_echo || exit 1
    start_upstream_bifrost || exit 1
    start_entry_bifrost || exit 1

    test_bifrost_proxy_chain || { print_test_summary || true; exit 1; }
    test_downstream_proxy_auth || { print_test_summary || true; exit 1; }

    print_test_summary || exit 1
}

main "$@"
