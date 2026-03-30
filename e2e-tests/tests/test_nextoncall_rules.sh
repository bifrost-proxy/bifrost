#!/bin/bash
#
# www.qq.com 代理规则测试脚本
#
# 测试规则:
# 1. https://www.qq.com/api/* -> 直接转发到原始服务 (excludeFilter 排除)
# 2. https://www.qq.com/* -> http://localhost:8000/
# 3. wss://www.qq.com/ -> ws://localhost:8000/
#
# 使用方式:
#   ./test_nextoncall_rules.sh           # 运行完整测试
#   ./test_nextoncall_rules.sh --manual  # 只启动服务，手动测试
#

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
E2E_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
BIFROST_BIN="${ROOT_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}/target/release/bifrost"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi

PROXY_HOST="${PROXY_HOST:-127.0.0.1}"
PROXY_PORT="${PROXY_PORT:-9900}"
MOCK_PORT="${MOCK_PORT:-8000}"
BIFROST_DATA_DIR="${BIFROST_DATA_DIR:-./.bifrost-nextoncall-test}"

RULES_FILE="$E2E_DIR/rules/forwarding/nextoncall_rules.txt"
MOCK_SERVER="$E2E_DIR/mock_servers/http_ws_echo_server.py"

source "$SCRIPT_DIR/../test_utils/process.sh"

PROXY_PID=""
MOCK_PID=""
MANUAL_MODE=false

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[PASS]${NC} $1"
}

log_error() {
    echo -e "${RED}[FAIL]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

run_with_timeout() {
    local seconds="$1"
    shift

    if command -v timeout >/dev/null 2>&1; then
        timeout "$seconds" "$@"
    elif command -v gtimeout >/dev/null 2>&1; then
        gtimeout "$seconds" "$@"
    elif [[ -x "$E2E_DIR/bin/timeout" ]]; then
        "$E2E_DIR/bin/timeout" "$seconds" "$@"
    else
        "$@"
    fi
}

cleanup() {
    log_info "Cleaning up..."

    if [[ -n "$PROXY_PID" ]]; then
        log_info "Stopping proxy server (PID: $PROXY_PID)"
        safe_cleanup_proxy "$PROXY_PID"
    fi

    if [[ -n "$MOCK_PID" ]]; then
        log_info "Stopping mock server (PID: $MOCK_PID)"
        kill_pid "$MOCK_PID"
        wait_pid "$MOCK_PID"
    fi

    if [[ -d "$BIFROST_DATA_DIR" ]]; then
        rm -rf "$BIFROST_DATA_DIR"
    fi

    if is_windows; then kill_bifrost_on_port "$PROXY_PORT"; fi
    log_info "Cleanup complete"
}

trap cleanup EXIT

check_deps() {
    log_info "Checking dependencies..."

    if ! command -v curl &> /dev/null; then
        log_error "curl is required but not installed"
        exit 1
    fi

    if ! command -v python3 &> /dev/null; then
        log_error "python3 is required but not installed"
        exit 1
    fi

    if ! command -v jq &> /dev/null; then
        log_warn "jq is not installed, JSON parsing will be limited"
    fi

    if command -v websocat &> /dev/null; then
        log_info "websocat found, WebSocket tests will be enabled"
        HAS_WEBSOCAT=true
        if websocat --help 2>/dev/null | grep -q -- "--proxy"; then
            HAS_WEBSOCAT_HTTP_PROXY=true
        else
            HAS_WEBSOCAT_HTTP_PROXY=false
            log_warn "websocat is installed but does not support --proxy; WSS proxy test will be skipped"
        fi
    else
        log_warn "websocat not found, WebSocket tests will be skipped"
        log_warn "Install with: brew install websocat"
        HAS_WEBSOCAT=false
        HAS_WEBSOCAT_HTTP_PROXY=false
    fi

    log_success "Dependencies check passed"
}

build_proxy() {
    :
}

start_mock_server() {
    log_info "Starting mock HTTP+WS server on port $MOCK_PORT..."

    python3 "$MOCK_SERVER" "$MOCK_PORT" &
    MOCK_PID=$!

    sleep 1

    if ! kill -0 "$MOCK_PID" 2>/dev/null; then
        log_error "Failed to start mock server"
        exit 1
    fi

    for i in {1..10}; do
        if nc -z 127.0.0.1 "$MOCK_PORT" 2>/dev/null; then
            log_success "Mock server started (PID: $MOCK_PID)"
            return 0
        fi
        sleep 0.5
    done

    log_error "Mock server failed to start"
    exit 1
}

start_proxy() {
    log_info "Starting bifrost proxy on port $PROXY_PORT with debug logging..."

    BIFROST_DATA_DIR="$BIFROST_DATA_DIR" \
    RUST_LOG=debug \
    "$BIFROST_BIN" \
        -p "$PROXY_PORT" \
        -l debug \
        start \
        --unsafe-ssl \
        --rules-file "$RULES_FILE" \
        --skip-cert-check \
        2>&1 &
    PROXY_PID=$!

    sleep 2

    if ! kill -0 "$PROXY_PID" 2>/dev/null; then
        log_error "Failed to start proxy server"
        exit 1
    fi

    for i in {1..20}; do
        if nc -z "$PROXY_HOST" "$PROXY_PORT" 2>/dev/null; then
            log_success "Proxy server started (PID: $PROXY_PID)"
            return 0
        fi
        sleep 0.5
    done

    log_error "Proxy server failed to start"
    exit 1
}

print_test_header() {
    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "🧪 TEST: $1"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
}

test_http_root_forward() {
    print_test_header "HTTPS root path -> localhost:8000"

    log_info "Testing: curl -x http://$PROXY_HOST:$PROXY_PORT https://www.qq.com/"
    log_info "Expected: Request forwarded to mock server at 127.0.0.1:$MOCK_PORT"

    local response exit_code=0
    response=$(curl -s -x "http://$PROXY_HOST:$PROXY_PORT" \
        -k \
        --connect-timeout 10 \
        --max-time 30 \
        "https://www.qq.com/" 2>&1) || exit_code=$?

    if [[ $exit_code -ne 0 ]]; then
        log_error "curl failed with exit code: $exit_code"
        log_error "Response: $response"
        return 1
    fi

    echo "Response:"
    echo "$response" | head -50

    if echo "$response" | grep -q "http_ws_echo_server"; then
        log_success "Request was forwarded to mock server"
        return 0
    elif echo "$response" | grep -q "http_echo"; then
        log_success "Request was forwarded to mock server"
        return 0
    else
        log_warn "Response doesn't contain expected mock server signature"
        log_warn "This might indicate the request was forwarded elsewhere or blocked"
        return 1
    fi
}

test_http_api_path() {
    print_test_header "HTTPS /api/ path -> original service (excluded)"

    log_info "Testing: curl -x http://$PROXY_HOST:$PROXY_PORT https://www.qq.com/api/test"
    log_info "Expected: Request NOT forwarded to mock server (excludeFilter should exclude /api/)"

    local response exit_code=0
    response=$(curl -s -x "http://$PROXY_HOST:$PROXY_PORT" \
        -k \
        --connect-timeout 10 \
        --max-time 30 \
        "https://www.qq.com/api/test" 2>&1) || exit_code=$?

    echo "Response (first 500 chars):"
    echo "$response" | head -c 500
    echo ""

    if echo "$response" | grep -q "http_ws_echo_server"; then
        log_warn "Request was forwarded to mock server - excludeFilter may not be working"
        log_warn "Check proxy logs for rule matching details"
        return 1
    else
        log_success "Request was NOT forwarded to mock server (as expected)"
        log_info "The /api/ path was excluded by excludeFilter"
        return 0
    fi
}

test_websocket_forward() {
    print_test_header "WSS -> ws://localhost:8000"

    if [[ "$HAS_WEBSOCAT" != "true" ]]; then
        log_warn "Skipping WebSocket test (websocat not installed)"
        return 0
    fi

    if [[ "$HAS_WEBSOCAT_HTTP_PROXY" != "true" ]]; then
        log_warn "Skipping WebSocket test (installed websocat has no HTTP proxy support)"
        return 0
    fi

    log_info "Testing: websocat wss://www.qq.com/ via proxy"
    log_info "Expected: WebSocket connection forwarded to mock server"

    local response
    response=$(echo '{"test": "hello"}' | run_with_timeout 10 websocat -v \
        --ws-c-uri "wss://www.qq.com/" \
        --proxy "http://$PROXY_HOST:$PROXY_PORT" \
        -k \
        "wss://www.qq.com/" 2>&1 || true)

    echo "Response:"
    echo "$response" | head -30

    if echo "$response" | grep -q "connection_info\|echo\|http_ws_echo_server"; then
        log_success "WebSocket connection was forwarded to mock server"
        return 0
    else
        log_warn "WebSocket test result unclear"
        log_warn "Check proxy logs for WebSocket handling details"
        return 1
    fi
}

run_manual_mode() {
    echo ""
    echo "╔══════════════════════════════════════════════════════════════╗"
    echo "║              Manual Testing Mode                             ║"
    echo "╠══════════════════════════════════════════════════════════════╣"
    echo "║  Proxy:  http://$PROXY_HOST:$PROXY_PORT                               ║"
    echo "║  Mock:   http://127.0.0.1:$MOCK_PORT                               ║"
    echo "║  Log:    debug level (verbose)                               ║"
    echo "╠══════════════════════════════════════════════════════════════╣"
    echo "║  Test commands:                                              ║"
    echo "║                                                              ║"
    echo "║  # Test root path (should forward to mock server)            ║"
    echo "║  curl -x http://$PROXY_HOST:$PROXY_PORT -k \\                         ║"
    echo "║       https://www.qq.com/                      ║"
    echo "║                                                              ║"
    echo "║  # Test /api/ path (should NOT forward to mock server)       ║"
    echo "║  curl -x http://$PROXY_HOST:$PROXY_PORT -k \\                         ║"
    echo "║       https://www.qq.com/api/test              ║"
    echo "║                                                              ║"
    echo "║  # Test WebSocket (requires websocat)                        ║"
    echo "║  echo 'hello' | websocat -v \\                                ║"
    echo "║       --proxy http://$PROXY_HOST:$PROXY_PORT -k \\                    ║"
    echo "║       wss://www.qq.com/                        ║"
    echo "╠══════════════════════════════════════════════════════════════╣"
    echo "║  Press Ctrl+C to stop                                        ║"
    echo "╚══════════════════════════════════════════════════════════════╝"
    echo ""

    wait
}

run_tests() {
    local passed=0
    local failed=0
    local skipped=0

    echo ""
    echo "╔══════════════════════════════════════════════════════════════╗"
    echo "║         www.qq.com Rules Test Suite            ║"
    echo "╠══════════════════════════════════════════════════════════════╣"
    echo "║  Proxy:  http://$PROXY_HOST:$PROXY_PORT                               ║"
    echo "║  Mock:   http://127.0.0.1:$MOCK_PORT                               ║"
    echo "║  Rules:  $RULES_FILE  ║"
    echo "╚══════════════════════════════════════════════════════════════╝"
    echo ""

    if test_http_root_forward; then
        ((passed++)) || true
    else
        ((failed++)) || true
    fi

    if test_http_api_path; then
        ((passed++)) || true
    else
        ((failed++)) || true
    fi

    if test_websocket_forward; then
        ((passed++)) || true
    else
        ((failed++)) || true
    fi

    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "📊 TEST SUMMARY"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo -e "  ${GREEN}Passed:${NC}  $passed"
    echo -e "  ${RED}Failed:${NC}  $failed"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

    if [[ $failed -gt 0 ]]; then
        return 1
    fi
    return 0
}

main() {
    echo ""
    echo "╔══════════════════════════════════════════════════════════════╗"
    echo "║     www.qq.com Proxy Rules Test Script         ║"
    echo "╚══════════════════════════════════════════════════════════════╝"
    echo ""

    for arg in "$@"; do
        case $arg in
            --manual|-m)
                MANUAL_MODE=true
                ;;
            --help|-h)
                echo "Usage: $0 [OPTIONS]"
                echo ""
                echo "Options:"
                echo "  --manual, -m    Start services for manual testing"
                echo "  --help, -h      Show this help message"
                echo ""
                echo "Environment variables:"
                echo "  PROXY_PORT      Proxy port (default: 9900)"
                echo "  MOCK_PORT       Mock server port (default: 8000)"
                exit 0
                ;;
        esac
    done

    check_deps
    build_proxy
    start_mock_server
    start_proxy

    if [[ "$MANUAL_MODE" == "true" ]]; then
        run_manual_mode
    else
        run_tests
    fi
}

main "$@"
