#!/bin/bash
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

source "${SCRIPT_DIR}/../test_utils/assert.sh"
source "${SCRIPT_DIR}/../test_utils/rule_fixture.sh"
source "${SCRIPT_DIR}/../test_utils/process.sh"

PROXY_PORT="${PROXY_PORT:-18889}"
ECHO_HTTP_PORT="${ECHO_HTTP_PORT:-19081}"
ADMIN_HOST="${ADMIN_HOST:-127.0.0.1}"
ADMIN_PORT="${ADMIN_PORT:-${PROXY_PORT}}"
ADMIN_PATH_PREFIX="${ADMIN_PATH_PREFIX:-/_bifrost}"

BIFROST_BIN="${PROJECT_DIR}/target/release/bifrost"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi
TEST_DATA_DIR=""
RULES_TEMPLATE="${PROJECT_DIR}/e2e-tests/rules/system_proxy/basic_forwarding.txt"
PROXY_PID=""
ECHO_PID=""
PLATFORM="$(uname -s)"

passed=0
failed=0

cleanup() {
    if [[ -n "$PROXY_PID" ]]; then
        safe_cleanup_proxy "$PROXY_PID"
    fi
    if [[ -n "$ECHO_PID" ]]; then
        kill_pid "$ECHO_PID"
        wait_pid "$ECHO_PID"
    fi
    if is_windows; then kill_bifrost_on_port "$PROXY_PORT"; fi
    if [[ -n "$TEST_DATA_DIR" ]] && [[ -d "$TEST_DATA_DIR" ]]; then
        rm -rf "$TEST_DATA_DIR"
    fi
}
trap cleanup EXIT

build_bifrost() {
    if [[ -f "$BIFROST_BIN" ]] && [[ "${SKIP_BUILD:-false}" == "true" ]]; then
        return 0
    fi
    return 0
}

setup_env() {
    TEST_DATA_DIR=$(mktemp -d)
    mkdir -p "${TEST_DATA_DIR}/.bifrost/rules"
    render_rule_fixture_to_file "$RULES_TEMPLATE" "${TEST_DATA_DIR}/.bifrost/rules/test.txt" \
        "ECHO_HTTP_PORT=${ECHO_HTTP_PORT}"
}

start_echo() {
    python3 "${PROJECT_DIR}/e2e-tests/mock_servers/http_echo_server.py" "${ECHO_HTTP_PORT}" &
    ECHO_PID=$!
    sleep 1
}

stop_proxy() {
    if [[ -n "$PROXY_PID" ]]; then
        safe_cleanup_proxy "$PROXY_PID"
    fi
    PROXY_PID=""
    rm -f "${TEST_DATA_DIR}/bifrost.pid" "${TEST_DATA_DIR}/runtime.json" 2>/dev/null || true
    if is_windows; then
        kill_bifrost_on_port "$PROXY_PORT"
        win_wait_port_free "$PROXY_PORT" 30 || true
    fi
    sleep 2
}

start_proxy_with_system_proxy() {
    export BIFROST_DATA_DIR="${TEST_DATA_DIR}"
    "$BIFROST_BIN" --port "${PROXY_PORT}" start \
        --skip-cert-check --unsafe-ssl \
        --rules-file "${TEST_DATA_DIR}/.bifrost/rules/test.txt" \
        --system-proxy \
        --proxy-bypass "localhost,127.0.0.1,::1,*.local" \
        > "${TEST_DATA_DIR}/proxy.log" 2>&1 &
    PROXY_PID=$!
    local max_wait=30
    local waited=0
    while [[ $waited -lt $max_wait ]]; do
        if curl -s "http://${ADMIN_HOST}:${ADMIN_PORT}${ADMIN_PATH_PREFIX}/api/system" >/dev/null 2>&1; then
            sleep 2
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done
    echo "[WARN] Proxy did not become ready within ${max_wait}s"
}

start_proxy_without_system_proxy() {
    export BIFROST_DATA_DIR="${TEST_DATA_DIR}"
    "$BIFROST_BIN" --port "${PROXY_PORT}" start \
        --skip-cert-check --unsafe-ssl \
        --rules-file "${TEST_DATA_DIR}/.bifrost/rules/test.txt" \
        > "${TEST_DATA_DIR}/proxy.log" 2>&1 &
    PROXY_PID=$!
    local max_wait=30
    local waited=0
    while [[ $waited -lt $max_wait ]]; do
        if curl -s "http://${ADMIN_HOST}:${ADMIN_PORT}${ADMIN_PATH_PREFIX}/api/system" >/dev/null 2>&1; then
            sleep 2
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done
    echo "[WARN] Proxy did not become ready within ${max_wait}s"
}

macos_find_services() {
    networksetup -listallnetworkservices 2>/dev/null | sed '1d' | sed '/^\*/d'
}

windows_proxy_field() {
    local field="$1"
    local key='HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings'

    if command -v reg >/dev/null 2>&1; then
        reg query "$key" /v "$field" 2>/dev/null && return 0
    fi

    cmd.exe //c "reg query \"$key\" /v $field" 2>/dev/null | tr -d '\r'
}

windows_proxy_enabled() {
    windows_proxy_field "ProxyEnable" | tr -d '\r' | grep -q "0x1"
}

windows_proxy_server() {
    windows_proxy_field "ProxyServer" | tr -d '\r' | awk '/ProxyServer/ {print $NF}'
}

windows_proxy_matches() {
    local raw="$1"
    local expected="$2"
    local http_proxy=""
    local https_proxy=""
    local part=""

    if [[ "$raw" == "$expected" ]]; then
        return 0
    fi

    IFS=';' read -r -a parts <<< "$raw"
    for part in "${parts[@]}"; do
        case "$part" in
            http=*)
                http_proxy="${part#http=}"
                ;;
            https=*)
                https_proxy="${part#https=}"
                ;;
        esac
    done

    if [[ -n "$http_proxy" && "$http_proxy" != "$expected" ]]; then
        return 1
    fi

    if [[ -n "$https_proxy" && "$https_proxy" != "$expected" ]]; then
        return 1
    fi

    [[ -n "$http_proxy" || -n "$https_proxy" ]]
}

macos_check_proxy_enabled_for_any_service() {
    local expected_host="$1"
    local expected_port="$2"
    local found="false"
    while IFS= read -r svc; do
        local web_enabled
        web_enabled=$(networksetup -getwebproxy "$svc" 2>/dev/null | grep -i '^Enabled:' | awk '{print $2}')
        local web_host
        web_host=$(networksetup -getwebproxy "$svc" 2>/dev/null | grep -i '^Server:' | awk '{print $2}')
        local web_port
        web_port=$(networksetup -getwebproxy "$svc" 2>/dev/null | grep -i '^Port:' | awk '{print $2}')
        if [[ "$web_enabled" == "Yes" && "$web_host" == "$expected_host" && "$web_port" == "$expected_port" ]]; then
            found="true"
            break
        fi
    done < <(macos_find_services)
    [[ "$found" == "true" ]]
}

macos_check_proxy_disabled_for_all_services() {
    local all_disabled="true"
    while IFS= read -r svc; do
        local web_enabled
        web_enabled=$(networksetup -getwebproxy "$svc" 2>/dev/null | grep -i '^Enabled:' | awk '{print $2}')
        if [[ "$web_enabled" == "Yes" ]]; then
            all_disabled="false"
            break
        fi
    done < <(macos_find_services)
    [[ "$all_disabled" == "true" ]]
}

test_enable_on_startup() {
    start_proxy_with_system_proxy
    case "$PLATFORM" in
        Darwin)
            if macos_check_proxy_enabled_for_any_service "127.0.0.1" "$PROXY_PORT"; then
                _log_pass "macOS: 系统代理设置正确"
                ((passed++))
            else
                _log_fail "macOS: 未检测到正确的系统代理设置" "127.0.0.1:${PROXY_PORT}" "networksetup 状态不匹配"
                ((failed++))
            fi
            ;;
        MINGW*|MSYS*|CYGWIN*)
            local server
            server="$(windows_proxy_server)"
            if windows_proxy_enabled && windows_proxy_matches "$server" "127.0.0.1:${PROXY_PORT}"; then
                _log_pass "Windows: 系统代理设置正确"
                ((passed++))
            else
                _log_fail "Windows: 未检测到正确的系统代理设置" "127.0.0.1:${PROXY_PORT}" "${server:-disabled}"
                ((failed++))
            fi
            ;;
    esac
}

test_disable_on_startup() {
    stop_proxy
    start_proxy_without_system_proxy
    case "$PLATFORM" in
        Darwin)
            if macos_check_proxy_disabled_for_all_services; then
                _log_pass "macOS: 未启用系统代理（符合预期）"
                ((passed++))
            else
                _log_fail "macOS: 未启用系统代理检查失败" "Disabled" "存在 Enabled=Yes"
                ((failed++))
            fi
            ;;
        MINGW*|MSYS*|CYGWIN*)
            if ! windows_proxy_enabled; then
                _log_pass "Windows: 未启用系统代理（符合预期）"
                ((passed++))
            else
                _log_fail "Windows: 未启用系统代理检查失败" "Disabled" "$(windows_proxy_server)"
                ((failed++))
            fi
            ;;
    esac
}

test_restore_on_exit() {
    if [[ -n "$PROXY_PID" ]]; then
        safe_cleanup_proxy "$PROXY_PID"
    fi
    PROXY_PID=""
    rm -f "${TEST_DATA_DIR}/bifrost.pid" "${TEST_DATA_DIR}/runtime.json" 2>/dev/null || true
    sleep 2
    case "$PLATFORM" in
        Darwin)
            if macos_check_proxy_disabled_for_all_services; then
                _log_pass "macOS: 代理退出后系统代理已恢复"
                ((passed++))
            else
                _log_fail "macOS: 代理退出后系统代理未恢复" "全部服务 Disabled" "存在 Enabled=Yes"
                ((failed++))
            fi
            ;;
        MINGW*|MSYS*|CYGWIN*)
            if ! windows_proxy_enabled; then
                _log_pass "Windows: 代理退出后系统代理已恢复"
                ((passed++))
            else
                _log_fail "Windows: 代理退出后系统代理未恢复" "Disabled" "$(windows_proxy_server)"
                ((failed++))
            fi
            ;;
    esac
}

test_crash_recovery() {
    stop_proxy
    start_proxy_with_system_proxy
    if [[ -n "$PROXY_PID" ]]; then
        kill_pid_force "$PROXY_PID"
        wait_pid "$PROXY_PID"
    fi
    PROXY_PID=""
    rm -f "${TEST_DATA_DIR}/bifrost.pid" "${TEST_DATA_DIR}/runtime.json" 2>/dev/null || true
    sleep 2
    case "$PLATFORM" in
        Darwin)
            if macos_check_proxy_enabled_for_any_service "127.0.0.1" "$PROXY_PORT"; then
                _log_pass "macOS: 崩溃后系统代理仍保持启用（符合预期）"
                ((passed++))
            else
                _log_fail "macOS: 崩溃后系统代理未保持启用" "保持启用" "未启用或端口不匹配"
                ((failed++))
            fi
            ;;
        MINGW*|MSYS*|CYGWIN*)
            if windows_proxy_enabled && windows_proxy_matches "$(windows_proxy_server)" "127.0.0.1:${PROXY_PORT}"; then
                _log_pass "Windows: 崩溃后系统代理仍保持启用（符合预期）"
                ((passed++))
            else
                _log_fail "Windows: 崩溃后系统代理未保持启用" "127.0.0.1:${PROXY_PORT}" "$(windows_proxy_server)"
                ((failed++))
            fi
            ;;
    esac
    start_proxy_without_system_proxy
    case "$PLATFORM" in
        Darwin)
            if macos_check_proxy_disabled_for_all_services; then
                _log_pass "macOS: 再次启动未启用系统代理，崩溃恢复生效"
                ((passed++))
            else
                _log_fail "macOS: 崩溃恢复未生效" "Disabled" "存在 Enabled=Yes"
                ((failed++))
            fi
            ;;
        MINGW*|MSYS*|CYGWIN*)
            if ! windows_proxy_enabled; then
                _log_pass "Windows: 再次启动未启用系统代理，崩溃恢复生效"
                ((passed++))
            else
                _log_fail "Windows: 崩溃恢复未生效" "Disabled" "$(windows_proxy_server)"
                ((failed++))
            fi
            ;;
    esac
}

main() {
    build_bifrost || { echo "编译失败"; exit 1; }
    setup_env
    start_echo

    case "$PLATFORM" in
        Darwin|MINGW*|MSYS*|CYGWIN*)
            ;;
        *)
            echo "Skipping system proxy E2E on unsupported platform: $PLATFORM"
            exit 0
            ;;
    esac

    test_enable_on_startup
    test_disable_on_startup
    test_restore_on_exit
    test_crash_recovery

    print_test_summary || exit 1
}

SKIP_BUILD="${SKIP_BUILD:-false}"
main "$@"
