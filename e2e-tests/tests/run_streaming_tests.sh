#!/bin/bash

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

source "$SCRIPT_DIR/../test_utils/process.sh"

PROXY_HOST="${PROXY_HOST:-127.0.0.1}"
PROXY_PORT="${PROXY_PORT:-9900}"
ADMIN_HOST="${ADMIN_HOST:-127.0.0.1}"
ADMIN_PORT="${ADMIN_PORT:-9900}"
WS_HOST="${WS_HOST:-127.0.0.1}"
WS_PORT="${WS_PORT:-8766}"
SSE_HOST="${SSE_HOST:-127.0.0.1}"
SSE_PORT="${SSE_PORT:-8767}"
ADMIN_PATH_PREFIX="${ADMIN_PATH_PREFIX:-/_bifrost}"

export PROXY_HOST PROXY_PORT ADMIN_HOST ADMIN_PORT WS_HOST WS_PORT SSE_HOST SSE_PORT ADMIN_PATH_PREFIX

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

MOCK_SERVERS_PID=""
PROXY_PID=""

log_info() { echo -e "${BLUE}[INFO]${NC} $*"; }
log_success() { echo -e "${GREEN}[SUCCESS]${NC} $*"; }
log_warning() { echo -e "${YELLOW}[WARNING]${NC} $*"; }
log_error() { echo -e "${RED}[ERROR]${NC} $*"; }

cleanup() {
    log_info "Cleaning up..."

    if [[ -n "$MOCK_SERVERS_PID" ]]; then
        log_info "Stopping mock servers..."
        for pid in $MOCK_SERVERS_PID; do
            kill_pid "$pid"
        done
        for pid in $MOCK_SERVERS_PID; do
            wait_pid "$pid"
        done
    fi

    log_info "Cleanup complete"
}

trap cleanup EXIT

check_dependencies() {
    log_info "Checking dependencies..."

    local missing=()

    command -v curl >/dev/null 2>&1 || missing+=("curl")
    command -v jq >/dev/null 2>&1 || missing+=("jq")
    command -v python3 >/dev/null 2>&1 || missing+=("python3")
    command -v websocat >/dev/null 2>&1 || missing+=("websocat")

    if [[ ${#missing[@]} -gt 0 ]]; then
        log_error "Missing dependencies: ${missing[*]}"
        log_info "Please install them before running the tests"
        exit 1
    fi

    log_success "All dependencies available"
}

wait_for_service() {
    local host="$1"
    local port="$2"
    local name="$3"
    local timeout="${4:-30}"

    log_info "Waiting for $name at $host:$port..."

    local count=0
    while ! nc -z "$host" "$port" 2>/dev/null; do
        count=$((count + 1))
        if [[ $count -ge $timeout ]]; then
            log_error "$name failed to start within ${timeout}s"
            return 1
        fi
        sleep 1
    done

    log_success "$name is ready"
    return 0
}

start_mock_servers() {
    log_info "Starting mock servers..."

    local mock_dir="$SCRIPT_DIR/../mock_servers"

    if [[ ! -f "$mock_dir/ws_echo_server.py" ]]; then
        log_error "WebSocket mock server not found at $mock_dir/ws_echo_server.py"
        return 1
    fi

    if [[ ! -f "$mock_dir/sse_echo_server.py" ]]; then
        log_error "SSE mock server not found at $mock_dir/sse_echo_server.py"
        return 1
    fi

    python3 "$mock_dir/ws_echo_server.py" --port "$WS_PORT" &
    local ws_pid=$!

    python3 "$mock_dir/sse_echo_server.py" --port "$SSE_PORT" &
    local sse_pid=$!

    MOCK_SERVERS_PID="$ws_pid $sse_pid"

    wait_for_service "$WS_HOST" "$WS_PORT" "WebSocket Server" 10 || return 1
    wait_for_service "$SSE_HOST" "$SSE_PORT" "SSE Server" 10 || return 1

    log_success "Mock servers started"
    return 0
}

check_proxy() {
    log_info "Checking proxy server..."

    if ! nc -z "$PROXY_HOST" "$PROXY_PORT" 2>/dev/null; then
        log_error "Proxy server not running at $PROXY_HOST:$PROXY_PORT"
        log_info "Please start the proxy server before running tests"
        return 1
    fi

    log_success "Proxy server is running"
    return 0
}

check_admin() {
    log_info "Checking admin API..."

    local response
    response=$(curl -s "http://$ADMIN_HOST:$ADMIN_PORT${ADMIN_PATH_PREFIX}/api/traffic?limit=1" 2>/dev/null)

    if [[ -z "$response" ]] || ! echo "$response" | jq -e '.records' >/dev/null 2>&1; then
        log_error "Admin API not responding at http://$ADMIN_HOST:$ADMIN_PORT${ADMIN_PATH_PREFIX}"
        log_info "Please start the proxy server before running tests"
        return 1
    fi

    log_success "Admin API is responding"
    return 0
}

run_test_suite() {
    local suite_name="$1"
    local script_path="$2"

    echo ""
    echo "=============================================="
    echo " Running: $suite_name"
    echo "=============================================="

    if [[ ! -f "$script_path" ]]; then
        log_error "Test script not found: $script_path"
        return 1
    fi

    if bash "$script_path"; then
        log_success "$suite_name completed"
        return 0
    else
        log_error "$suite_name failed"
        return 1
    fi
}

print_usage() {
    echo "Usage: $0 [OPTIONS] [TEST_SUITES...]"
    echo ""
    echo "Options:"
    echo "  -h, --help          Show this help message"
    echo "  -s, --skip-servers  Skip starting mock servers (assume they're already running)"
    echo "  -v, --verbose       Enable verbose output"
    echo "  --ws-only          Run only WebSocket tests"
    echo "  --sse-only         Run only SSE tests"
    echo "  --api-only         Run only Admin API tests"
    echo ""
    echo "Test Suites:"
    echo "  websocket          WebSocket frame tests"
    echo "  sse                SSE frame tests"
    echo "  admin              Admin API tests"
    echo "  all                All tests (default)"
    echo ""
    echo "Environment Variables:"
    echo "  PROXY_HOST         Proxy server host (default: 127.0.0.1)"
    echo "  PROXY_PORT         Proxy server port (default: 9900)"
    echo "  ADMIN_HOST         Admin server host (default: 127.0.0.1)"
    echo "  ADMIN_PORT         Admin server port (default: 8900)"
    echo "  WS_HOST            WebSocket server host (default: 127.0.0.1)"
    echo "  WS_PORT            WebSocket server port (default: 8766)"
    echo "  SSE_HOST           SSE server host (default: 127.0.0.1)"
    echo "  SSE_PORT           SSE server port (default: 8767)"
    echo ""
    echo "Examples:"
    echo "  $0                    # Run all tests"
    echo "  $0 websocket sse      # Run WebSocket and SSE tests"
    echo "  $0 --ws-only          # Run only WebSocket tests"
    echo "  $0 -s all             # Run all tests without starting mock servers"
}

main() {
    local skip_servers=false
    local verbose=false
    local suites=()

    while [[ $# -gt 0 ]]; do
        case "$1" in
            -h|--help)
                print_usage
                exit 0
                ;;
            -s|--skip-servers)
                skip_servers=true
                shift
                ;;
            -v|--verbose)
                verbose=true
                export DEBUG=1
                shift
                ;;
            --ws-only)
                suites=("websocket")
                shift
                ;;
            --sse-only)
                suites=("sse")
                shift
                ;;
            --api-only)
                suites=("admin")
                shift
                ;;
            websocket|sse|admin|all)
                if [[ "$1" == "all" ]]; then
                    suites=("websocket" "sse" "admin")
                else
                    suites+=("$1")
                fi
                shift
                ;;
            *)
                log_error "Unknown option: $1"
                print_usage
                exit 1
                ;;
        esac
    done

    if [[ ${#suites[@]} -eq 0 ]]; then
        suites=("websocket" "sse" "admin")
    fi

    echo ""
    echo "=============================================="
    echo " Streaming Protocol E2E Tests"
    echo "=============================================="
    echo ""

    check_dependencies

    check_proxy || exit 1
    check_admin || exit 1

    if [[ "$skip_servers" != "true" ]]; then
        start_mock_servers || exit 1
    else
        log_info "Skipping mock server startup (--skip-servers)"
        wait_for_service "$WS_HOST" "$WS_PORT" "WebSocket Server" 5 || exit 1
        wait_for_service "$SSE_HOST" "$SSE_PORT" "SSE Server" 5 || exit 1
    fi

    local total_suites=0
    local passed_suites=0
    local failed_suites=()

    for suite in "${suites[@]}"; do
        total_suites=$((total_suites + 1))

        case "$suite" in
            websocket)
                if run_test_suite "WebSocket Frame Tests" "$SCRIPT_DIR/test_websocket_frames.sh"; then
                    passed_suites=$((passed_suites + 1))
                else
                    failed_suites+=("websocket")
                fi
                ;;
            sse)
                if run_test_suite "SSE Frame Tests" "$SCRIPT_DIR/test_sse_frames.sh"; then
                    passed_suites=$((passed_suites + 1))
                else
                    failed_suites+=("sse")
                fi
                ;;
            admin)
                if run_test_suite "Admin API Tests" "$SCRIPT_DIR/test_frames_admin_api.sh"; then
                    passed_suites=$((passed_suites + 1))
                else
                    failed_suites+=("admin")
                fi
                ;;
        esac
    done

    echo ""
    echo "=============================================="
    echo " Final Summary"
    echo "=============================================="
    echo "Test Suites Run:    $total_suites"
    echo "Test Suites Passed: $passed_suites"
    echo "Test Suites Failed: ${#failed_suites[@]}"

    if [[ ${#failed_suites[@]} -gt 0 ]]; then
        echo ""
        echo "Failed suites: ${failed_suites[*]}"
    fi

    echo "=============================================="

    if [[ ${#failed_suites[@]} -eq 0 ]]; then
        log_success "All test suites passed!"
        exit 0
    else
        log_error "Some test suites failed!"
        exit 1
    fi
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    main "$@"
fi
