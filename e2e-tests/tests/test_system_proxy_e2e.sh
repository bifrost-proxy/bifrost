#!/bin/bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

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

BIFROST_BIN="${BIFROST_BIN:-${PROJECT_DIR}/target/release/bifrost}"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi
TEST_DATA_DIR=""
RULES_TEMPLATE="${PROJECT_DIR}/e2e-tests/rules/system_proxy/basic_forwarding.txt"
PROXY_PID=""
ECHO_PID=""
BLOCKER_PID=""
PLATFORM="$(uname -s)"

passed=0
failed=0

cleanup() {
    if [[ -n "$PROXY_PID" ]]; then
        cleanup_system_proxy_process "$PROXY_PID"
    fi
    if [[ -n "$ECHO_PID" ]]; then
        kill_pid "$ECHO_PID"
        wait_pid "$ECHO_PID"
    fi
    if [[ -n "$BLOCKER_PID" ]]; then
        kill_pid "$BLOCKER_PID"
        wait_pid "$BLOCKER_PID"
    fi
    if is_windows; then kill_bifrost_on_port "$PROXY_PORT"; fi
    if [[ -n "$TEST_DATA_DIR" ]] && [[ -d "$TEST_DATA_DIR" ]]; then
        rm -rf "$TEST_DATA_DIR"
    fi
}
trap cleanup EXIT

cleanup_system_proxy_process() {
    local pid="$1"
    if [[ -z "$pid" ]]; then
        return 0
    fi

    kill_pid "$pid"

    local timeout=60
    local elapsed=0
    while kill -0 "$pid" 2>/dev/null; do
        sleep 0.5
        elapsed=$((elapsed + 1))
        if [[ "$elapsed" -ge "$((timeout * 2))" ]]; then
            break
        fi
    done

    if kill -0 "$pid" 2>/dev/null; then
        kill_pid_force "$pid"
        sleep 0.5
    fi
}

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

start_port_blocker() {
    local port="$1"
    python3 - "$port" > "${TEST_DATA_DIR}/port-blocker.log" 2>&1 <<'PY' &
import socket
import sys
import time

port = int(sys.argv[1])
sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
sock.bind(("127.0.0.1", port))
sock.listen(1)
print("ready", flush=True)
try:
    while True:
        time.sleep(1)
finally:
    sock.close()
PY
    BLOCKER_PID=$!

    local waited=0
    while [[ $waited -lt 30 ]]; do
        if grep -q "ready" "${TEST_DATA_DIR}/port-blocker.log" 2>/dev/null; then
            return 0
        fi
        if ! kill -0 "$BLOCKER_PID" 2>/dev/null; then
            cat "${TEST_DATA_DIR}/port-blocker.log" 2>/dev/null || true
            return 1
        fi
        sleep 0.2
        waited=$((waited + 1))
    done
    return 1
}

stop_port_blocker() {
    if [[ -n "$BLOCKER_PID" ]]; then
        kill_pid "$BLOCKER_PID"
        wait_pid "$BLOCKER_PID"
        BLOCKER_PID=""
    fi
}

stop_proxy() {
    if [[ -n "$PROXY_PID" ]]; then
        cleanup_system_proxy_process "$PROXY_PID"
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
        --no-system-proxy \
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

macos_set_external_http_proxy() {
    local host="$1"
    local port="$2"
    local svc
    while IFS= read -r svc; do
        networksetup -setwebproxy "$svc" "$host" "$port" >/dev/null 2>&1 || return 1
        networksetup -setwebproxystate "$svc" on >/dev/null 2>&1 || return 1
    done < <(macos_find_services)
}

macos_clear_http_proxy() {
    local svc
    while IFS= read -r svc; do
        networksetup -setwebproxystate "$svc" off >/dev/null 2>&1 || return 1
    done < <(macos_find_services)
}

macos_clear_web_and_secure_proxy() {
    local svc
    while IFS= read -r svc; do
        networksetup -setwebproxystate "$svc" off >/dev/null 2>&1 || return 1
        networksetup -setsecurewebproxystate "$svc" off >/dev/null 2>&1 || return 1
    done < <(macos_find_services)
}

macos_proxy_snapshot() {
    local svc
    while IFS= read -r svc; do
        echo "service=${svc}"
        networksetup -getwebproxy "$svc" 2>/dev/null | sed 's/^/  web: /' || true
        networksetup -getsecurewebproxy "$svc" 2>/dev/null | sed 's/^/  secure: /' || true
    done < <(macos_find_services)
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

macos_check_proxy_not_pointing_to() {
    local expected_host="$1"
    local expected_port="$2"
    ! macos_check_proxy_enabled_for_any_service "$expected_host" "$expected_port"
}

wait_for_condition() {
    local timeout="$1"
    local interval="${2:-1}"
    shift 2
    local waited=0

    while [[ "$waited" -le "$timeout" ]]; do
        if "$@"; then
            return 0
        fi
        sleep "$interval"
        waited=$((waited + interval))
    done

    return 1
}

macos_wait_proxy_enabled() {
    local expected_host="$1"
    local expected_port="$2"
    local timeout="${3:-30}"
    wait_for_condition "$timeout" 1 macos_check_proxy_enabled_for_any_service "$expected_host" "$expected_port"
}

macos_wait_proxy_disabled() {
    local timeout="${1:-30}"
    wait_for_condition "$timeout" 1 macos_check_proxy_disabled_for_all_services
}

windows_wait_proxy_enabled() {
    local expected="$1"
    local timeout="${2:-30}"
    local waited=0
    local server
    while [[ "$waited" -le "$timeout" ]]; do
        server="$(windows_proxy_server)"
        if windows_proxy_enabled && windows_proxy_matches "$server" "$expected"; then
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done
    return 1
}

windows_wait_proxy_disabled() {
    local timeout="${1:-30}"
    local waited=0
    while [[ "$waited" -le "$timeout" ]]; do
        if ! windows_proxy_enabled; then
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done
    return 1
}

test_external_proxy_not_disabled_without_bifrost_ownership() {
    stop_proxy
    rm -f "${TEST_DATA_DIR}/proxy_backup.json" "${TEST_DATA_DIR}/proxy_state.json" 2>/dev/null || true
    local external_host="127.0.0.1"
    local external_port="$((PROXY_PORT + 127))"

    case "$PLATFORM" in
        Darwin)
            if ! macos_set_external_http_proxy "$external_host" "$external_port"; then
                _log_fail "macOS: 无法设置外部系统代理夹具" "${external_host}:${external_port}" "$(macos_proxy_snapshot)"
                failed=$((failed + 1))
                return
            fi
            ;;
        MINGW*|MSYS*|CYGWIN*)
            _log_pass "Windows: 外部代理归属回归用例暂不修改注册表，跳过真实系统写入"
            passed=$((passed + 1))
            return
            ;;
    esac

    start_proxy_without_system_proxy
    local status
    status="$(curl -s -X PUT "http://${ADMIN_HOST}:${ADMIN_PORT}${ADMIN_PATH_PREFIX}/api/proxy/system" \
        -H 'Content-Type: application/json' \
        -d '{"enabled":false}' || true)"

    if echo "$status" | grep -q '"managed_by_bifrost":false' \
        && macos_wait_proxy_enabled "$external_host" "$external_port" 10; then
        _log_pass "macOS: 外部系统代理未被 Bifrost disable 误关闭"
        passed=$((passed + 1))
    else
        _log_fail "macOS: 外部系统代理被误关闭或状态归属错误" "managed_by_bifrost=false 且 ${external_host}:${external_port} 保持启用" "status=${status}; snapshot=$(macos_proxy_snapshot)"
        failed=$((failed + 1))
    fi

    stop_proxy
    macos_clear_http_proxy || true
}

test_enable_on_startup() {
    start_proxy_with_system_proxy
    case "$PLATFORM" in
        Darwin)
            if macos_wait_proxy_enabled "127.0.0.1" "$PROXY_PORT" 45; then
                _log_pass "macOS: 系统代理设置正确"
                passed=$((passed + 1))
            else
                _log_fail "macOS: 未检测到正确的系统代理设置" "127.0.0.1:${PROXY_PORT}" "$(macos_proxy_snapshot)"
                failed=$((failed + 1))
            fi
            ;;
        MINGW*|MSYS*|CYGWIN*)
            local server
            server="$(windows_proxy_server)"
            if windows_wait_proxy_enabled "127.0.0.1:${PROXY_PORT}" 45; then
                _log_pass "Windows: 系统代理设置正确"
                passed=$((passed + 1))
            else
                _log_fail "Windows: 未检测到正确的系统代理设置" "127.0.0.1:${PROXY_PORT}" "${server:-disabled}"
                failed=$((failed + 1))
            fi
            ;;
    esac
}

test_disable_on_startup() {
    stop_proxy
    start_proxy_without_system_proxy
    case "$PLATFORM" in
        Darwin)
            if wait_for_condition 45 1 macos_check_proxy_not_pointing_to "127.0.0.1" "$PROXY_PORT"; then
                _log_pass "macOS: 未启用 Bifrost 系统代理（外部代理会保留）"
                passed=$((passed + 1))
            else
                _log_fail "macOS: 未启用 Bifrost 系统代理检查失败" "不指向 127.0.0.1:${PROXY_PORT}" "$(macos_proxy_snapshot)"
                failed=$((failed + 1))
            fi
            ;;
        MINGW*|MSYS*|CYGWIN*)
            if windows_wait_proxy_disabled 45; then
                _log_pass "Windows: 未启用系统代理（符合预期）"
                passed=$((passed + 1))
            else
                _log_fail "Windows: 未启用系统代理检查失败" "Disabled" "$(windows_proxy_server)"
                failed=$((failed + 1))
            fi
            ;;
    esac
}

test_restore_on_exit() {
    if [[ -n "$PROXY_PID" ]]; then
        cleanup_system_proxy_process "$PROXY_PID"
    fi
    PROXY_PID=""
    rm -f "${TEST_DATA_DIR}/bifrost.pid" "${TEST_DATA_DIR}/runtime.json" 2>/dev/null || true
    sleep 2
    case "$PLATFORM" in
        Darwin)
            if macos_wait_proxy_disabled 45; then
                _log_pass "macOS: 代理退出后系统代理已恢复"
                passed=$((passed + 1))
            else
                _log_fail "macOS: 代理退出后系统代理未恢复" "全部服务 Disabled" "$(macos_proxy_snapshot)"
                failed=$((failed + 1))
            fi
            ;;
        MINGW*|MSYS*|CYGWIN*)
            if windows_wait_proxy_disabled 45; then
                _log_pass "Windows: 代理退出后系统代理已恢复"
                passed=$((passed + 1))
            else
                _log_fail "Windows: 代理退出后系统代理未恢复" "Disabled" "$(windows_proxy_server)"
                failed=$((failed + 1))
            fi
            ;;
    esac
}

test_sleep_wake_style_reconcile() {
    stop_proxy
    start_proxy_with_system_proxy
    case "$PLATFORM" in
        Darwin)
            if ! macos_wait_proxy_enabled "127.0.0.1" "$PROXY_PORT" 45; then
                _log_fail "macOS: 睡眠恢复收敛回归准备失败" "系统代理先指向 127.0.0.1:${PROXY_PORT}" "$(macos_proxy_snapshot)"
                failed=$((failed + 1))
                return
            fi
            if ! macos_clear_web_and_secure_proxy; then
                _log_fail "macOS: 无法模拟睡眠恢复后的系统代理漂移" "关闭 Web/Secure Web 代理状态" "$(macos_proxy_snapshot)"
                failed=$((failed + 1))
                return
            fi
            if macos_wait_proxy_enabled "127.0.0.1" "$PROXY_PORT" 75; then
                _log_pass "macOS: 睡眠恢复式系统代理漂移后已重新收敛到 Bifrost"
                passed=$((passed + 1))
            else
                _log_fail "macOS: 睡眠恢复式系统代理漂移后未重新收敛" "127.0.0.1:${PROXY_PORT}" "$(macos_proxy_snapshot)"
                failed=$((failed + 1))
            fi
            ;;
        MINGW*|MSYS*|CYGWIN*)
            _log_pass "Windows: 睡眠恢复式系统代理漂移回归暂不修改注册表，跳过真实系统写入"
            passed=$((passed + 1))
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
            if macos_wait_proxy_enabled "127.0.0.1" "$PROXY_PORT" 45; then
                _log_pass "macOS: 崩溃后系统代理仍保持启用（符合预期）"
                passed=$((passed + 1))
            else
                _log_fail "macOS: 崩溃后系统代理未保持启用" "保持启用" "$(macos_proxy_snapshot)"
                failed=$((failed + 1))
            fi
            ;;
        MINGW*|MSYS*|CYGWIN*)
            if windows_wait_proxy_enabled "127.0.0.1:${PROXY_PORT}" 45; then
                _log_pass "Windows: 崩溃后系统代理仍保持启用（符合预期）"
                passed=$((passed + 1))
            else
                _log_fail "Windows: 崩溃后系统代理未保持启用" "127.0.0.1:${PROXY_PORT}" "$(windows_proxy_server)"
                failed=$((failed + 1))
            fi
            ;;
    esac
    start_proxy_without_system_proxy
    case "$PLATFORM" in
        Darwin)
            if wait_for_condition 45 1 macos_check_proxy_not_pointing_to "127.0.0.1" "$PROXY_PORT"; then
                _log_pass "macOS: 再次启动未启用 Bifrost 系统代理，崩溃恢复生效"
                passed=$((passed + 1))
            else
                _log_fail "macOS: 崩溃恢复未生效" "不指向 127.0.0.1:${PROXY_PORT}" "$(macos_proxy_snapshot)"
                failed=$((failed + 1))
            fi
            ;;
        MINGW*|MSYS*|CYGWIN*)
            if windows_wait_proxy_disabled 45; then
                _log_pass "Windows: 再次启动未启用系统代理，崩溃恢复生效"
                passed=$((passed + 1))
            else
                _log_fail "Windows: 崩溃恢复未生效" "Disabled" "$(windows_proxy_server)"
                failed=$((failed + 1))
            fi
            ;;
    esac
}

test_crash_recovery_runs_before_start_failure() {
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
            if ! macos_wait_proxy_enabled "127.0.0.1" "$PROXY_PORT" 45; then
                _log_fail "macOS: 启动失败前恢复回归准备失败" "崩溃后系统代理仍指向 127.0.0.1:${PROXY_PORT}" "$(macos_proxy_snapshot)"
                failed=$((failed + 1))
                return
            fi
            ;;
        MINGW*|MSYS*|CYGWIN*)
            if ! windows_wait_proxy_enabled "127.0.0.1:${PROXY_PORT}" 45; then
                _log_fail "Windows: 启动失败前恢复回归准备失败" "127.0.0.1:${PROXY_PORT}" "$(windows_proxy_server)"
                failed=$((failed + 1))
                return
            fi
            ;;
    esac

    if ! start_port_blocker "$PROXY_PORT"; then
        _log_fail "端口占用夹具启动失败" "127.0.0.1:${PROXY_PORT} 被占用" "$(cat "${TEST_DATA_DIR}/port-blocker.log" 2>/dev/null || true)"
        failed=$((failed + 1))
        return
    fi

    export BIFROST_DATA_DIR="${TEST_DATA_DIR}"
    local output
    output=$("$BIFROST_BIN" --port "${PROXY_PORT}" start \
        --skip-cert-check --unsafe-ssl \
        --rules-file "${TEST_DATA_DIR}/.bifrost/rules/test.txt" \
        --no-system-proxy 2>&1)
    local code=$?
    stop_port_blocker

    if [[ "$code" -eq 0 ]]; then
        _log_fail "启动失败前恢复回归未触发端口失败" "start 因端口占用非零退出" "$output"
        failed=$((failed + 1))
        return
    fi

    case "$PLATFORM" in
        Darwin)
            if wait_for_condition 45 1 macos_check_proxy_not_pointing_to "127.0.0.1" "$PROXY_PORT"; then
                _log_pass "macOS: 启动失败前已同步清理崩溃残留系统代理"
                passed=$((passed + 1))
            else
                _log_fail "macOS: 启动失败前未清理崩溃残留系统代理" "不指向 127.0.0.1:${PROXY_PORT}" "output=${output}; snapshot=$(macos_proxy_snapshot)"
                failed=$((failed + 1))
            fi
            ;;
        MINGW*|MSYS*|CYGWIN*)
            if windows_wait_proxy_disabled 45; then
                _log_pass "Windows: 启动失败前已同步清理崩溃残留系统代理"
                passed=$((passed + 1))
            else
                _log_fail "Windows: 启动失败前未清理崩溃残留系统代理" "Disabled" "output=${output}; proxy=$(windows_proxy_server)"
                failed=$((failed + 1))
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
    test_external_proxy_not_disabled_without_bifrost_ownership
    test_restore_on_exit
    test_sleep_wake_style_reconcile
    test_crash_recovery
    test_crash_recovery_runs_before_start_failure

    print_test_summary || exit 1
}

SKIP_BUILD="${SKIP_BUILD:-false}"
main "$@"
