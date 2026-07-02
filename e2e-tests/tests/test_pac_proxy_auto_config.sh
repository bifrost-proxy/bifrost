#!/bin/bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

source "$ROOT_DIR/e2e-tests/test_utils/assert.sh"
source "$ROOT_DIR/e2e-tests/test_utils/process.sh"

PROXY_HOST="${PROXY_HOST:-127.0.0.1}"
if [[ -z "${ENTRY_PORT:-}" || -z "${UPSTREAM_PORT:-}" || -z "${ECHO_HTTP_PORT:-}" ]]; then
    PORT_BASE="$(pick_available_base_port "${BIFROST_E2E_PAC_BASE_PORT:-0}" 3)"
    if [[ -z "$PORT_BASE" || "$PORT_BASE" == "0" ]]; then
        echo "failed to allocate PAC e2e ports" >&2
        exit 1
    fi
    ENTRY_PORT="${ENTRY_PORT:-$PORT_BASE}"
    UPSTREAM_PORT="${UPSTREAM_PORT:-$((PORT_BASE + 1))}"
    ECHO_HTTP_PORT="${ECHO_HTTP_PORT:-$((PORT_BASE + 2))}"
fi

BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/release/bifrost}"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi

PYTHON_BIN="$(python_cmd)"
HTTP_ECHO_SERVER="$ROOT_DIR/e2e-tests/mock_servers/http_echo_server.py"

TEST_ROOT_DIR=""
ENTRY_DATA_DIR=""
UPSTREAM_DATA_DIR=""
ENTRY_RULES_FILE=""
UPSTREAM_RULES_FILE=""

ENTRY_PID=""
UPSTREAM_PID=""
HTTP_ECHO_PID=""

HTTP_STATUS=""
HTTP_HEADERS=""
HTTP_BODY=""

cleanup() {
    kill_bifrost_on_port "$ENTRY_PORT"
    kill_bifrost_on_port "$UPSTREAM_PORT"
    safe_cleanup_proxy "$ENTRY_PID"
    safe_cleanup_proxy "$UPSTREAM_PID"
    kill_pid "$HTTP_ECHO_PID"
    wait_pid "$ENTRY_PID"
    wait_pid "$UPSTREAM_PID"
    wait_pid "$HTTP_ECHO_PID"
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
    if [[ ! -f "$HTTP_ECHO_SERVER" ]]; then
        echo "missing mock server script: $HTTP_ECHO_SERVER" >&2
        exit 1
    fi
    if ! command -v curl >/dev/null 2>&1; then
        echo "missing curl" >&2
        exit 1
    fi

    if [[ "${SKIP_BUILD:-false}" == "true" && -x "$BIFROST_BIN" ]]; then
        _log_pass "Using existing Bifrost binary: $BIFROST_BIN"
        return 0
    fi

    log_section "Build current Bifrost binary"
    (cd "$ROOT_DIR" && SKIP_FRONTEND_BUILD=1 cargo build --release --bin bifrost) || exit 1
    if [[ ! -x "$BIFROST_BIN" ]]; then
        echo "missing bifrost binary after build: $BIFROST_BIN" >&2
        exit 1
    fi
    _log_pass "Built release binary from current workspace"
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
        if curl -fsS "http://${PROXY_HOST}:${port}/_bifrost/api/proxy/address" >/dev/null 2>&1; then
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

    local headers_file
    local body_file
    headers_file=$(mktemp)
    body_file=$(mktemp)

    HTTP_STATUS=$(NO_PROXY="" no_proxy="" HTTP_PROXY="" http_proxy="" HTTPS_PROXY="" https_proxy="" \
        curl -sS -o "$body_file" -D "$headers_file" \
        --proxy "http://${PROXY_HOST}:${ENTRY_PORT}" \
        --noproxy "" \
        --connect-timeout 5 \
        --max-time 20 \
        "$url" \
        -w '%{http_code}')
    HTTP_HEADERS=$(cat "$headers_file")
    HTTP_BODY=$(cat "$body_file")
    rm -f "$headers_file" "$body_file"
}

dump_diagnostics() {
    echo "" >&2
    echo "---- PAC e2e diagnostics ----" >&2
    echo "entry_port=${ENTRY_PORT} upstream_port=${UPSTREAM_PORT} echo_http_port=${ECHO_HTTP_PORT}" >&2
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
        "${TEST_ROOT_DIR}/http_echo.log"; do
        if [[ -f "$log_file" ]]; then
            echo "" >&2
            echo "---- tail ${log_file} ----" >&2
            tail -n 120 "$log_file" >&2 || true
        fi
    done
}

prepare_workspace() {
    TEST_ROOT_DIR="$(mktemp -d "${ROOT_DIR}/.bifrost-e2e-pac.XXXXXX")"
    ENTRY_DATA_DIR="${TEST_ROOT_DIR}/entry-data"
    UPSTREAM_DATA_DIR="${TEST_ROOT_DIR}/upstream-data"
    ENTRY_RULES_FILE="${TEST_ROOT_DIR}/entry.rules.txt"
    UPSTREAM_RULES_FILE="${TEST_ROOT_DIR}/upstream.rules.txt"
    mkdir -p "$ENTRY_DATA_DIR" "$UPSTREAM_DATA_DIR"
}

write_rules() {
    cat > "$ENTRY_RULES_FILE" <<EOF
\`\`\`pac_values
function FindProxyForURL(url, host) {
  if (dnsDomainIs(host, "pac-chain.local") && shExpMatch(url, "*via=values*")) {
    return "PROXY 127.0.0.1:${UPSTREAM_PORT}";
  }
  return "DIRECT";
}
\`\`\`
\`\`\`pac_final_url_direct
function FindProxyForURL(url, host) {
  if (dnsDomainIs(host, "pac-forward.local") && shExpMatch(url, "*/forward*")) {
    return "DIRECT";
  }
  return "PROXY 127.0.0.1:1";
}
\`\`\`
pac-chain.local pac://{pac_values}
pac-remote.local pac://http://127.0.0.1:${ECHO_HTTP_PORT}/normal.pac?proxy=127.0.0.1:${UPSTREAM_PORT}
pac-forward.local proxy://127.0.0.1:1 host://127.0.0.1:${ECHO_HTTP_PORT} pac://{pac_final_url_direct}
EOF

    cat > "$UPSTREAM_RULES_FILE" <<EOF
pac-chain.local host://127.0.0.1:${ECHO_HTTP_PORT}
pac-remote.local host://127.0.0.1:${ECHO_HTTP_PORT}
EOF
}

start_http_echo() {
    local log_file="${TEST_ROOT_DIR}/http_echo.log"
    "$PYTHON_BIN" "$HTTP_ECHO_SERVER" "$ECHO_HTTP_PORT" >"$log_file" 2>&1 &
    HTTP_ECHO_PID=$!
    wait_for_http_service "http://127.0.0.1:${ECHO_HTTP_PORT}/health" "http echo server" "$HTTP_ECHO_PID" "$log_file"
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

test_values_pac_to_upstream_bifrost_proxy() {
    log_section "Values PAC returns an upstream Bifrost proxy"
    perform_request "http://pac-chain.local/chain?via=values"
    assert_status_2xx "$HTTP_STATUS" "Values PAC 代理链路请求成功" || {
        dump_diagnostics
        return 1
    }
    assert_body_contains '"parsed_path": "/chain"' "$HTTP_BODY" "上游 Bifrost 将 PAC 代理请求转发到 Mock Server" || return 1
    assert_body_contains '"query_string": "via=values"' "$HTTP_BODY" "PAC 代理链路保留查询参数" || return 1
}

test_remote_pac_to_upstream_bifrost_proxy() {
    log_section "Remote PAC returns an upstream Bifrost proxy"
    perform_request "http://pac-remote.local/remote?via=remote"
    assert_status_2xx "$HTTP_STATUS" "远程 PAC 代理链路请求成功" || {
        dump_diagnostics
        return 1
    }
    assert_body_contains '"parsed_path": "/remote"' "$HTTP_BODY" "远程 PAC 返回的上游 Bifrost 生效" || return 1
    assert_body_contains '"query_string": "via=remote"' "$HTTP_BODY" "远程 PAC 代理链路保留查询参数" || return 1
}

test_pac_with_forwarding_rules_uses_final_url() {
    log_section "PAC with forwarding rules clears existing proxy"
    perform_request "http://pac-forward.local/forward?case=forward"
    assert_status_2xx "$HTTP_STATUS" "PAC + 转发规则请求成功" || {
        dump_diagnostics
        return 1
    }
    assert_body_contains '"parsed_path": "/forward"' "$HTTP_BODY" "PAC 清除既有 proxy 后通过 host 转发到 Mock Server" || return 1
    assert_body_contains '"query_string": "case=forward"' "$HTTP_BODY" "PAC + 转发规则保留查询参数" || return 1
}

main() {
    ensure_dependencies
    prepare_workspace
    write_rules

    start_http_echo || exit 1
    start_upstream_bifrost || exit 1
    start_entry_bifrost || exit 1

    test_values_pac_to_upstream_bifrost_proxy || { print_test_summary || true; exit 1; }
    test_remote_pac_to_upstream_bifrost_proxy || { print_test_summary || true; exit 1; }
    test_pac_with_forwarding_rules_uses_final_url || { print_test_summary || true; exit 1; }

    print_test_summary || exit 1
}

main "$@"
