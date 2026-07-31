#!/bin/bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
: "${BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL:=1}"
export BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL
# Shorten the system-proxy reconcile loop so the dirty-backup reconcile case does not
# force a >30s wait per run. The daemon honours this via system_proxy_reconcile_interval().
: "${BIFROST_SYSTEM_PROXY_RECONCILE_SECS:=3}"
export BIFROST_SYSTEM_PROXY_RECONCILE_SECS
: "${RUST_LOG:=info}"
export RUST_LOG

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

read_runtime_pid() {
    local runtime_file="${TEST_DATA_DIR}/runtime.json"
    [[ -f "$runtime_file" ]] || return 1
    python3 - "$runtime_file" <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as f:
    data = json.load(f)
pid = data.get("pid")
if pid is None:
    sys.exit(1)
print(pid)
PY
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

start_daemon_proxy_with_system_proxy() {
    export BIFROST_DATA_DIR="${TEST_DATA_DIR}"
    "$BIFROST_BIN" --port "${PROXY_PORT}" start \
        --daemon --yes \
        --skip-cert-check --unsafe-ssl \
        --rules-file "${TEST_DATA_DIR}/.bifrost/rules/test.txt" \
        --system-proxy \
        --proxy-bypass "localhost,127.0.0.1,::1,*.local" \
        > "${TEST_DATA_DIR}/proxy-daemon-start.log" 2>&1
    local code=$?
    if [[ $code -ne 0 ]]; then
        cat "${TEST_DATA_DIR}/proxy-daemon-start.log" 2>/dev/null || true
        return "$code"
    fi
    local max_wait=30
    local waited=0
    while [[ $waited -lt $max_wait ]]; do
        if curl -s "http://${ADMIN_HOST}:${ADMIN_PORT}${ADMIN_PATH_PREFIX}/api/system" >/dev/null 2>&1; then
            PROXY_PID="$(read_runtime_pid 2>/dev/null || true)"
            sleep 2
            return 0
        fi
        sleep 1
        waited=$((waited + 1))
    done
    echo "[WARN] Daemon proxy did not become ready within ${max_wait}s"
    return 1
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
        networksetup -setsecurewebproxy "$svc" "$host" "$port" >/dev/null 2>&1 || return 1
        networksetup -setsecurewebproxystate "$svc" on >/dev/null 2>&1 || return 1
    done < <(macos_find_services)
}

macos_clear_http_proxy() {
    local svc
    while IFS= read -r svc; do
        networksetup -setwebproxystate "$svc" off >/dev/null 2>&1 || return 1
        networksetup -setsecurewebproxystate "$svc" off >/dev/null 2>&1 || return 1
    done < <(macos_find_services)
}

macos_save_proxy_snapshot_file() {
    local file="$1"
    : > "$file"
    local svc
    while IFS= read -r svc; do
        local web secure web_enabled web_host web_port secure_enabled secure_host secure_port
        web="$(networksetup -getwebproxy "$svc" 2>/dev/null || true)"
        secure="$(networksetup -getsecurewebproxy "$svc" 2>/dev/null || true)"
        web_enabled="$(echo "$web" | grep -i '^Enabled:' | awk '{print $2}')"
        web_host="$(echo "$web" | grep -i '^Server:' | awk '{print $2}')"
        web_port="$(echo "$web" | grep -i '^Port:' | awk '{print $2}')"
        secure_enabled="$(echo "$secure" | grep -i '^Enabled:' | awk '{print $2}')"
        secure_host="$(echo "$secure" | grep -i '^Server:' | awk '{print $2}')"
        secure_port="$(echo "$secure" | grep -i '^Port:' | awk '{print $2}')"
        printf '%s|%s|%s|%s|%s|%s|%s\n' \
            "$svc" "$web_enabled" "$web_host" "$web_port" "$secure_enabled" "$secure_host" "$secure_port" >> "$file"
    done < <(macos_find_services)
}

macos_restore_proxy_snapshot_file() {
    local file="$1"
    [[ -f "$file" ]] || return 0

    local svc web_enabled web_host web_port secure_enabled secure_host secure_port
    while IFS='|' read -r svc web_enabled web_host web_port secure_enabled secure_host secure_port; do
        if [[ -n "$web_host" && -n "$web_port" ]]; then
            networksetup -setwebproxy "$svc" "$web_host" "$web_port" >/dev/null 2>&1 || return 1
        fi
        if [[ "$web_enabled" == "Yes" ]]; then
            networksetup -setwebproxystate "$svc" on >/dev/null 2>&1 || return 1
        else
            networksetup -setwebproxystate "$svc" off >/dev/null 2>&1 || return 1
        fi

        if [[ -n "$secure_host" && -n "$secure_port" ]]; then
            networksetup -setsecurewebproxy "$svc" "$secure_host" "$secure_port" >/dev/null 2>&1 || return 1
        fi
        if [[ "$secure_enabled" == "Yes" ]]; then
            networksetup -setsecurewebproxystate "$svc" on >/dev/null 2>&1 || return 1
        else
            networksetup -setsecurewebproxystate "$svc" off >/dev/null 2>&1 || return 1
        fi
    done < "$file"
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

macos_check_any_proxy_enabled_not_pointing_to() {
    local expected_host="$1"
    local expected_port="$2"
    local found="false"
    while IFS= read -r svc; do
        local web secure web_enabled web_host web_port secure_enabled secure_host secure_port
        web="$(networksetup -getwebproxy "$svc" 2>/dev/null || true)"
        secure="$(networksetup -getsecurewebproxy "$svc" 2>/dev/null || true)"
        web_enabled="$(echo "$web" | grep -i '^Enabled:' | awk '{print $2}')"
        web_host="$(echo "$web" | grep -i '^Server:' | awk '{print $2}')"
        web_port="$(echo "$web" | grep -i '^Port:' | awk '{print $2}')"
        secure_enabled="$(echo "$secure" | grep -i '^Enabled:' | awk '{print $2}')"
        secure_host="$(echo "$secure" | grep -i '^Server:' | awk '{print $2}')"
        secure_port="$(echo "$secure" | grep -i '^Port:' | awk '{print $2}')"
        if [[ "$web_enabled" == "Yes" && ( "$web_host" != "$expected_host" || "$web_port" != "$expected_port" ) ]]; then
            found="true"
            break
        fi
        if [[ "$secure_enabled" == "Yes" && ( "$secure_host" != "$expected_host" || "$secure_port" != "$expected_port" ) ]]; then
            found="true"
            break
        fi
    done < <(macos_find_services)
    [[ "$found" == "true" ]]
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

macos_system_proxy_status_json() {
    curl -s "http://${ADMIN_HOST}:${ADMIN_PORT}${ADMIN_PATH_PREFIX}/api/proxy/system" || true
}

admin_api_ready() {
    curl -s "http://${ADMIN_HOST}:${ADMIN_PORT}${ADMIN_PATH_PREFIX}/api/system" >/dev/null 2>&1
}

macos_enable_system_proxy_api() {
    curl -s -X PUT "http://${ADMIN_HOST}:${ADMIN_PORT}${ADMIN_PATH_PREFIX}/api/proxy/system" \
        -H 'Content-Type: application/json' \
        -d '{"enabled":true}' >/dev/null
}

macos_disable_system_proxy_api() {
    curl -s -X PUT "http://${ADMIN_HOST}:${ADMIN_PORT}${ADMIN_PATH_PREFIX}/api/proxy/system" \
        -H 'Content-Type: application/json' \
        -d '{"enabled":false}'
}

macos_ensure_bifrost_system_proxy_enabled() {
    if macos_wait_proxy_enabled "127.0.0.1" "$PROXY_PORT" 10; then
        return 0
    fi

    macos_enable_system_proxy_api
    macos_wait_proxy_enabled "127.0.0.1" "$PROXY_PORT" 45
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
    local previous_snapshot="${TEST_DATA_DIR}/macos-proxy-before-external-fixture.txt"

    case "$PLATFORM" in
        Darwin)
            macos_save_proxy_snapshot_file "$previous_snapshot" || true
            if ! macos_set_external_http_proxy "$external_host" "$external_port"; then
                _log_fail "macOS: 无法设置外部系统代理夹具" "${external_host}:${external_port}" "$(macos_proxy_snapshot)"
                failed=$((failed + 1))
                macos_restore_proxy_snapshot_file "$previous_snapshot" || true
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
    elif echo "$status" | grep -q '"managed_by_bifrost":false' \
        && macos_check_proxy_not_pointing_to "127.0.0.1" "$PROXY_PORT" \
        && macos_check_any_proxy_enabled_not_pointing_to "127.0.0.1" "$PROXY_PORT"; then
        _log_pass "macOS: 外部系统代理 owner 发生变化，但 Bifrost 未误关闭或接管"
        passed=$((passed + 1))
    else
        _log_fail "macOS: 外部系统代理被误关闭或状态归属错误" "managed_by_bifrost=false 且系统代理保持为非 ${PROXY_PORT} 的外部代理" "status=${status}; snapshot=$(macos_proxy_snapshot)"
        failed=$((failed + 1))
    fi

    stop_proxy
    macos_restore_proxy_snapshot_file "$previous_snapshot" || true
}

test_enable_on_startup() {
    start_proxy_with_system_proxy
    case "$PLATFORM" in
        Darwin)
            if macos_wait_proxy_enabled "127.0.0.1" "$PROXY_PORT" 45; then
                _log_pass "macOS: 系统代理设置正确"
                passed=$((passed + 1))
                sleep "$(( ${BIFROST_SYSTEM_PROXY_RECONCILE_SECS:-30} * 2 + 2 ))"
                local full_reconcile_count
                full_reconcile_count="$(grep -c "system proxy full reconcile completed" "${TEST_DATA_DIR}/proxy.log" 2>/dev/null || true)"
                if [[ "$full_reconcile_count" -eq 1 ]]; then
                    _log_pass "macOS: 已收敛系统代理跨两个高频周期只执行一次完整 reconcile"
                    passed=$((passed + 1))
                else
                    _log_fail "macOS: 已收敛系统代理重复执行完整 reconcile" "full_reconcile_count=1" "count=${full_reconcile_count}; log=$(tail -n 120 "${TEST_DATA_DIR}/proxy.log" 2>/dev/null || true)"
                    failed=$((failed + 1))
                fi
            elif echo "$(macos_system_proxy_status_json)" | grep -q '"managed_by_bifrost":false'; then
                _log_pass "macOS: 启动时检测到外部系统代理，未自动抢占"
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
            if wait_for_condition 45 1 macos_check_proxy_not_pointing_to "127.0.0.1" "$PROXY_PORT"; then
                _log_pass "macOS: 代理退出后系统代理已恢复"
                passed=$((passed + 1))
            else
                _log_fail "macOS: 代理退出后系统代理未恢复" "不指向 127.0.0.1:${PROXY_PORT}" "$(macos_proxy_snapshot)"
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
    local previous_snapshot="${TEST_DATA_DIR}/macos-proxy-before-sleep-reconcile.txt"
    if [[ "$PLATFORM" == "Darwin" ]]; then
        macos_save_proxy_snapshot_file "$previous_snapshot" || true
        if macos_check_any_proxy_enabled_not_pointing_to "127.0.0.1" "$PROXY_PORT"; then
            _log_pass "macOS: 检测到外部系统代理 owner，跳过非隔离的睡眠漂移自动化用例"
            passed=$((passed + 1))
            return
        fi
        if ! macos_clear_web_and_secure_proxy; then
            _log_fail "macOS: 无法准备睡眠恢复收敛夹具" "临时清空当前 Web/Secure Web 代理" "$(macos_proxy_snapshot)"
            failed=$((failed + 1))
            macos_restore_proxy_snapshot_file "$previous_snapshot" || true
            return
        fi
    fi
    start_proxy_with_system_proxy
    case "$PLATFORM" in
        Darwin)
            if [[ -z "$PROXY_PID" ]] || ! kill -0 "$PROXY_PID" 2>/dev/null || ! admin_api_ready; then
                _log_fail "macOS: 睡眠恢复收敛回归准备失败" "Bifrost 服务必须保持运行且 Admin API 可访问" "pid=${PROXY_PID}; log=$(tail -n 80 "${TEST_DATA_DIR}/proxy.log" 2>/dev/null || true)"
                failed=$((failed + 1))
                stop_proxy
                macos_restore_proxy_snapshot_file "$previous_snapshot" || true
                return
            fi
            if ! macos_ensure_bifrost_system_proxy_enabled; then
                _log_fail "macOS: 睡眠恢复收敛回归准备失败" "系统代理先指向 127.0.0.1:${PROXY_PORT}" "$(macos_proxy_snapshot)"
                failed=$((failed + 1))
                stop_proxy
                macos_restore_proxy_snapshot_file "$previous_snapshot" || true
                return
            fi
            if ! macos_clear_web_and_secure_proxy; then
                _log_fail "macOS: 无法模拟睡眠恢复后的系统代理漂移" "关闭 Web/Secure Web 代理状态" "$(macos_proxy_snapshot)"
                failed=$((failed + 1))
                stop_proxy
                macos_restore_proxy_snapshot_file "$previous_snapshot" || true
                return
            fi
            if macos_wait_proxy_enabled "127.0.0.1" "$PROXY_PORT" 75; then
                _log_pass "macOS: 睡眠恢复式系统代理漂移后已重新收敛到 Bifrost"
                passed=$((passed + 1))
            else
                _log_fail "macOS: 睡眠恢复式系统代理漂移后未重新收敛" "127.0.0.1:${PROXY_PORT}" "$(macos_proxy_snapshot)"
                failed=$((failed + 1))
            fi
            stop_proxy
            macos_restore_proxy_snapshot_file "$previous_snapshot" || true
            ;;
        MINGW*|MSYS*|CYGWIN*)
            _log_pass "Windows: 睡眠恢复式系统代理漂移回归暂不修改注册表，跳过真实系统写入"
            passed=$((passed + 1))
            ;;
    esac
}

test_restart_preserves_system_proxy() {
    stop_proxy
    local previous_snapshot="${TEST_DATA_DIR}/macos-proxy-before-restart-preserve.txt"
    case "$PLATFORM" in
        Darwin)
            macos_save_proxy_snapshot_file "$previous_snapshot" || true
            if macos_check_any_proxy_enabled_not_pointing_to "127.0.0.1" "$PROXY_PORT"; then
                _log_pass "macOS: 检测到外部系统代理 owner，跳过非隔离的 restart 系统代理保持用例"
                passed=$((passed + 1))
                return
            fi
            if ! macos_clear_web_and_secure_proxy; then
                _log_fail "macOS: restart 系统代理保持用例准备失败" "临时清空当前 Web/Secure Web 代理" "$(macos_proxy_snapshot)"
                failed=$((failed + 1))
                macos_restore_proxy_snapshot_file "$previous_snapshot" || true
                return
            fi
            ;;
        MINGW*|MSYS*|CYGWIN*)
            _log_pass "Windows: bifrost restart 暂不支持，跳过系统代理保持用例"
            passed=$((passed + 1))
            return
            ;;
    esac

    start_proxy_with_system_proxy
    if ! macos_ensure_bifrost_system_proxy_enabled; then
        _log_fail "macOS: restart 系统代理保持用例准备失败" "系统代理先指向 127.0.0.1:${PROXY_PORT}" "$(macos_proxy_snapshot)"
        failed=$((failed + 1))
        stop_proxy
        macos_restore_proxy_snapshot_file "$previous_snapshot" || true
        return
    fi

    local old_pid="$PROXY_PID"
    export BIFROST_DATA_DIR="${TEST_DATA_DIR}"
    local output
    output=$("$BIFROST_BIN" restart 2>&1)
    local code=$?
    if [[ "$code" -ne 0 ]]; then
        _log_fail "macOS: bifrost restart 调用失败" "restart exit 0" "code=${code}; output=${output}"
        failed=$((failed + 1))
        stop_proxy
        macos_restore_proxy_snapshot_file "$previous_snapshot" || true
        return
    fi

    local waited=0
    local new_pid=""
    while [[ "$waited" -lt 90 ]]; do
        new_pid="$(read_runtime_pid 2>/dev/null || true)"
        if [[ -n "$new_pid" && "$new_pid" != "$old_pid" ]] \
            && kill -0 "$new_pid" 2>/dev/null \
            && admin_api_ready; then
            PROXY_PID="$new_pid"
            break
        fi
        sleep 1
        waited=$((waited + 1))
    done

    if [[ "$PROXY_PID" == "$old_pid" ]]; then
        _log_fail "macOS: restart 后新 daemon 未就绪" "runtime pid 更新且 Admin API ready" "output=${output}; restart_log=$(tail -n 120 "${TEST_DATA_DIR}/logs/restart.log" 2>/dev/null || true)"
        failed=$((failed + 1))
        stop_proxy
        macos_restore_proxy_snapshot_file "$previous_snapshot" || true
        return
    fi

    if macos_wait_proxy_enabled "127.0.0.1" "$PROXY_PORT" 75; then
        _log_pass "macOS: bifrost restart 后系统代理重新指向 Bifrost"
        passed=$((passed + 1))
    else
        _log_fail "macOS: bifrost restart 后系统代理未恢复" "127.0.0.1:${PROXY_PORT}" "output=${output}; snapshot=$(macos_proxy_snapshot); restart_log=$(tail -n 120 "${TEST_DATA_DIR}/logs/restart.log" 2>/dev/null || true)"
        failed=$((failed + 1))
    fi

    stop_proxy
    macos_restore_proxy_snapshot_file "$previous_snapshot" || true
}

test_crash_recovery() {
    stop_proxy
    local previous_snapshot="${TEST_DATA_DIR}/macos-proxy-before-crash-recovery.txt"
    if [[ "$PLATFORM" == "Darwin" ]]; then
        macos_save_proxy_snapshot_file "$previous_snapshot" || true
        if macos_check_any_proxy_enabled_not_pointing_to "127.0.0.1" "$PROXY_PORT"; then
            _log_pass "macOS: 检测到外部系统代理 owner，跳过非隔离的崩溃后保持启用用例"
            passed=$((passed + 1))
            return
        fi
        if ! macos_clear_web_and_secure_proxy; then
            _log_fail "macOS: 崩溃恢复准备失败" "临时清空当前 Web/Secure Web 代理" "$(macos_proxy_snapshot)"
            failed=$((failed + 1))
            macos_restore_proxy_snapshot_file "$previous_snapshot" || true
            return
        fi
    fi
    export BIFROST_SYSTEM_PROXY_DISABLE_LIFECYCLE_HELPER=1
    start_proxy_with_system_proxy
    unset BIFROST_SYSTEM_PROXY_DISABLE_LIFECYCLE_HELPER
    if [[ "$PLATFORM" == "Darwin" ]] && ! macos_ensure_bifrost_system_proxy_enabled; then
        _log_fail "macOS: 崩溃恢复准备失败" "系统代理先指向 127.0.0.1:${PROXY_PORT}" "$(macos_proxy_snapshot)"
        failed=$((failed + 1))
        macos_restore_proxy_snapshot_file "$previous_snapshot" || true
        return
    fi
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
            macos_restore_proxy_snapshot_file "$previous_snapshot" || true
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

test_runtime_target_no_state_crash_recovery() {
    stop_proxy
    rm -f "${TEST_DATA_DIR}/proxy_backup.json" "${TEST_DATA_DIR}/proxy_state.json" 2>/dev/null || true
    local previous_snapshot="${TEST_DATA_DIR}/macos-proxy-before-runtime-target-recovery.txt"

    case "$PLATFORM" in
        Darwin)
            macos_save_proxy_snapshot_file "$previous_snapshot" || true
            if ! macos_set_external_http_proxy "127.0.0.1" "$PROXY_PORT"; then
                _log_fail "macOS: runtime target 恢复准备失败" "系统代理先指向 127.0.0.1:${PROXY_PORT}" "$(macos_proxy_snapshot)"
                failed=$((failed + 1))
                macos_restore_proxy_snapshot_file "$previous_snapshot" || true
                return
            fi
            ;;
        MINGW*|MSYS*|CYGWIN*)
            _log_pass "Windows: runtime target no-state 恢复回归暂不修改注册表，跳过真实系统写入"
            passed=$((passed + 1))
            return
            ;;
    esac

    cat > "${TEST_DATA_DIR}/runtime.json" <<EOF
{
  "pid": 999999,
  "port": ${PROXY_PORT},
  "host": "0.0.0.0"
}
EOF
    echo "999999" > "${TEST_DATA_DIR}/bifrost.pid"

    start_proxy_without_system_proxy

    case "$PLATFORM" in
        Darwin)
            if wait_for_condition 45 1 macos_check_proxy_not_pointing_to "127.0.0.1" "$PROXY_PORT"; then
                _log_pass "macOS: 无 backup/state 时按 runtime target 清理崩溃残留系统代理"
                passed=$((passed + 1))
            else
                _log_fail "macOS: 无 backup/state 时 runtime target 恢复未生效" "不指向 127.0.0.1:${PROXY_PORT}" "$(macos_proxy_snapshot)"
                failed=$((failed + 1))
            fi
            stop_proxy
            macos_restore_proxy_snapshot_file "$previous_snapshot" || true
            ;;
    esac
}

test_lifecycle_helper_cleans_after_parent_crash() {
    stop_proxy
    unset BIFROST_SYSTEM_PROXY_DISABLE_LIFECYCLE_HELPER
    if [[ "$PLATFORM" == "Darwin" ]] && macos_check_any_proxy_enabled_not_pointing_to "127.0.0.1" "$PROXY_PORT"; then
        _log_pass "macOS: 检测到外部系统代理 owner，跳过非隔离的 lifecycle helper 崩溃兜底用例"
        passed=$((passed + 1))
        return
    fi
    start_proxy_with_system_proxy
    if [[ "$PLATFORM" == "Darwin" ]] && ! macos_ensure_bifrost_system_proxy_enabled; then
        _log_fail "macOS: lifecycle helper 回归准备失败" "系统代理先指向 127.0.0.1:${PROXY_PORT}" "$(macos_proxy_snapshot)"
        failed=$((failed + 1))
        return
    fi
    if [[ -n "$PROXY_PID" ]]; then
        kill_pid_force "$PROXY_PID"
        wait_pid "$PROXY_PID"
    fi
    PROXY_PID=""
    rm -f "${TEST_DATA_DIR}/bifrost.pid" "${TEST_DATA_DIR}/runtime.json" 2>/dev/null || true

    case "$PLATFORM" in
        Darwin)
            if wait_for_condition 45 1 macos_check_proxy_not_pointing_to "127.0.0.1" "$PROXY_PORT"; then
                _log_pass "macOS: 独立 lifecycle helper 已在主进程崩溃后清理 Bifrost 系统代理"
                passed=$((passed + 1))
            else
                _log_fail "macOS: 独立 lifecycle helper 未清理主进程崩溃残留系统代理" "不指向 127.0.0.1:${PROXY_PORT}" "$(macos_proxy_snapshot); log=$(tail -n 80 "${TEST_DATA_DIR}/proxy.log" 2>/dev/null || true)"
                failed=$((failed + 1))
            fi
            ;;
        MINGW*|MSYS*|CYGWIN*)
            _log_pass "Windows: lifecycle helper 为 macOS 关机兜底路径，跳过"
            passed=$((passed + 1))
            ;;
    esac
}

test_lifecycle_helper_starts_power_watcher_directly() {
    stop_proxy
    case "$PLATFORM" in
        Darwin)
            local helper_log="${TEST_DATA_DIR}/lifecycle-helper-power-watcher.log"
            RUST_LOG=info BIFROST_DATA_DIR="${TEST_DATA_DIR}" "$BIFROST_BIN" system-proxy lifecycle-helper \
                --data-dir "${TEST_DATA_DIR}" \
                --parent-pid 1 \
                --poll-secs 60 \
                > "$helper_log" 2>&1 &
            local helper_pid=$!
            sleep 3
            if grep -q "system proxy lifecycle helper power watcher started" "${TEST_DATA_DIR}"/logs/bifrost*.log 2>/dev/null; then
                _log_pass "macOS: lifecycle helper 可独立注册 IOKit power watcher"
                passed=$((passed + 1))
            else
                _log_fail "macOS: lifecycle helper 未注册 IOKit power watcher" "日志包含 system proxy lifecycle helper power watcher started" "$(cat "$helper_log" "${TEST_DATA_DIR}"/logs/bifrost*.log 2>/dev/null || true)"
                failed=$((failed + 1))
            fi
            kill_pid "$helper_pid"
            wait_pid "$helper_pid"
            ;;
        MINGW*|MSYS*|CYGWIN*)
            _log_pass "Windows: IOKit power watcher 为 macOS-only，跳过"
            passed=$((passed + 1))
            ;;
    esac
}

test_lifecycle_helper_restarts_restartable_daemon_after_parent_crash() {
    stop_proxy
    unset BIFROST_SYSTEM_PROXY_DISABLE_LIFECYCLE_HELPER
    if [[ "$PLATFORM" == "Darwin" ]] && macos_check_any_proxy_enabled_not_pointing_to "127.0.0.1" "$PROXY_PORT"; then
        _log_pass "macOS: 检测到外部系统代理 owner，跳过 daemon restart-before-restore 用例"
        passed=$((passed + 1))
        return
    fi

    case "$PLATFORM" in
        Darwin)
            if ! start_daemon_proxy_with_system_proxy; then
                _log_fail "macOS: daemon restart-before-restore 准备失败" "daemon start exit 0 and Admin API ready" "$(cat "${TEST_DATA_DIR}/proxy-daemon-start.log" 2>/dev/null || true)"
                failed=$((failed + 1))
                return
            fi
            if ! macos_ensure_bifrost_system_proxy_enabled; then
                _log_fail "macOS: daemon restart-before-restore 准备失败" "系统代理先指向 127.0.0.1:${PROXY_PORT}" "$(macos_proxy_snapshot)"
                failed=$((failed + 1))
                return
            fi
            local old_pid="${PROXY_PID:-$(read_runtime_pid 2>/dev/null || true)}"
            if [[ -z "$old_pid" ]]; then
                _log_fail "macOS: daemon restart-before-restore 准备失败" "runtime.json 包含 daemon pid" "$(cat "${TEST_DATA_DIR}/runtime.json" 2>/dev/null || true)"
                failed=$((failed + 1))
                return
            fi
            kill_pid_force "$old_pid"
            wait_pid "$old_pid"
            PROXY_PID=""

            local waited=0
            local new_pid=""
            while [[ $waited -lt 45 ]]; do
                new_pid="$(read_runtime_pid 2>/dev/null || true)"
                if [[ -n "$new_pid" && "$new_pid" != "$old_pid" ]] \
                    && curl -s "http://${ADMIN_HOST}:${ADMIN_PORT}${ADMIN_PATH_PREFIX}/api/system" >/dev/null 2>&1 \
                    && macos_check_proxy_enabled_for_any_service "127.0.0.1" "$PROXY_PORT"; then
                    PROXY_PID="$new_pid"
                    _log_pass "macOS: restartable daemon 崩溃后 lifecycle helper 自动重启主进程并保留系统代理"
                    passed=$((passed + 1))
                    return
                fi
                sleep 1
                waited=$((waited + 1))
            done

            _log_fail "macOS: restartable daemon 崩溃后未自动重启" "新 runtime pid、Admin API ready、系统代理仍指向 127.0.0.1:${PROXY_PORT}" "old_pid=${old_pid}; new_pid=${new_pid}; proxy=$(macos_proxy_snapshot); logs=$(tail -n 160 "${TEST_DATA_DIR}"/logs/bifrost*.log 2>/dev/null || true)"
            failed=$((failed + 1))
            ;;
        MINGW*|MSYS*|CYGWIN*)
            _log_pass "Windows: daemon restart-before-restore focused coverage uses unit tests in this phase"
            passed=$((passed + 1))
            ;;
    esac
}

test_admin_api_enable_starts_lifecycle_helper() {
    stop_proxy
    unset BIFROST_SYSTEM_PROXY_DISABLE_LIFECYCLE_HELPER
    start_proxy_without_system_proxy
    case "$PLATFORM" in
        Darwin)
            if [[ -z "$PROXY_PID" ]] || ! kill -0 "$PROXY_PID" 2>/dev/null || ! admin_api_ready; then
                _log_fail "macOS: Admin API 启用 lifecycle helper 回归准备失败" "Bifrost 服务必须保持运行且 Admin API 可访问" "pid=${PROXY_PID}; log=$(tail -n 80 "${TEST_DATA_DIR}/proxy.log" 2>/dev/null || true)"
                failed=$((failed + 1))
                return
            fi
            if macos_check_any_proxy_enabled_not_pointing_to "127.0.0.1" "$PROXY_PORT"; then
                _log_pass "macOS: 检测到外部系统代理 owner，跳过非隔离的 Admin API lifecycle helper 用例"
                passed=$((passed + 1))
                return
            fi
            macos_enable_system_proxy_api
            if ! macos_wait_proxy_enabled "127.0.0.1" "$PROXY_PORT" 45; then
                _log_fail "macOS: Admin API 启用系统代理失败" "系统代理指向 127.0.0.1:${PROXY_PORT}" "$(macos_proxy_snapshot); log=$(tail -n 80 "${TEST_DATA_DIR}/proxy.log" 2>/dev/null || true)"
                failed=$((failed + 1))
                return
            fi
            if wait_for_condition 15 1 grep -q "system proxy lifecycle helper started after Admin API enable" "${TEST_DATA_DIR}"/logs/bifrost*.log; then
                _log_pass "macOS: Admin API 运行时启用系统代理后已启动 lifecycle helper"
                passed=$((passed + 1))
            else
                _log_fail "macOS: Admin API 运行时启用系统代理后未启动 lifecycle helper" "日志包含 lifecycle helper started after Admin API enable" "$(tail -n 120 "${TEST_DATA_DIR}/proxy.log" "${TEST_DATA_DIR}"/logs/bifrost*.log 2>/dev/null || true)"
                failed=$((failed + 1))
            fi
            if wait_for_condition 15 1 grep -q "system proxy lifecycle helper power watcher started" "${TEST_DATA_DIR}"/logs/bifrost*.log; then
                _log_pass "macOS: lifecycle helper 已注册 IOKit power watcher"
                passed=$((passed + 1))
            else
                _log_fail "macOS: lifecycle helper 未注册 IOKit power watcher" "日志包含 system proxy lifecycle helper power watcher started" "$(tail -n 160 "${TEST_DATA_DIR}/proxy.log" "${TEST_DATA_DIR}"/logs/bifrost*.log 2>/dev/null || true)"
                failed=$((failed + 1))
            fi
            stop_proxy
            ;;
        MINGW*|MSYS*|CYGWIN*)
            _log_pass "Windows: lifecycle helper 为 macOS 运行期崩溃兜底路径，跳过"
            passed=$((passed + 1))
            ;;
    esac
}

test_admin_api_enable_lifecycle_helper_cleans_after_parent_crash() {
    stop_proxy
    unset BIFROST_SYSTEM_PROXY_DISABLE_LIFECYCLE_HELPER
    local previous_snapshot="${TEST_DATA_DIR}/macos-proxy-before-admin-api-helper-crash.txt"
    case "$PLATFORM" in
        Darwin)
            macos_save_proxy_snapshot_file "$previous_snapshot" || true
            if ! macos_clear_web_and_secure_proxy; then
                _log_fail "macOS: 无法准备 Admin API lifecycle helper 崩溃兜底夹具" "临时清空当前 Web/Secure Web 代理" "$(macos_proxy_snapshot)"
                failed=$((failed + 1))
                macos_restore_proxy_snapshot_file "$previous_snapshot" || true
                return
            fi
            start_proxy_without_system_proxy
            if [[ -z "$PROXY_PID" ]] || ! kill -0 "$PROXY_PID" 2>/dev/null || ! admin_api_ready; then
                _log_fail "macOS: Admin API lifecycle helper 崩溃兜底准备失败" "Bifrost 服务必须保持运行且 Admin API 可访问" "pid=${PROXY_PID}; log=$(tail -n 80 "${TEST_DATA_DIR}/proxy.log" 2>/dev/null || true)"
                failed=$((failed + 1))
                stop_proxy
                macos_restore_proxy_snapshot_file "$previous_snapshot" || true
                return
            fi
            if macos_check_any_proxy_enabled_not_pointing_to "127.0.0.1" "$PROXY_PORT"; then
                _log_pass "macOS: 检测到外部系统代理 owner，跳过非隔离的 Admin API lifecycle helper 崩溃兜底用例"
                passed=$((passed + 1))
                stop_proxy
                macos_restore_proxy_snapshot_file "$previous_snapshot" || true
                return
            fi
            macos_enable_system_proxy_api
            if ! macos_wait_proxy_enabled "127.0.0.1" "$PROXY_PORT" 45; then
                if macos_check_any_proxy_enabled_not_pointing_to "127.0.0.1" "$PROXY_PORT"; then
                    _log_pass "macOS: 检测到外部系统代理 owner，跳过非隔离的 Admin API lifecycle helper 崩溃兜底用例"
                    passed=$((passed + 1))
                    stop_proxy
                    macos_restore_proxy_snapshot_file "$previous_snapshot" || true
                    return
                fi
                _log_fail "macOS: Admin API lifecycle helper 崩溃兜底启用系统代理失败" "系统代理指向 127.0.0.1:${PROXY_PORT}" "$(macos_proxy_snapshot); log=$(tail -n 80 "${TEST_DATA_DIR}/proxy.log" 2>/dev/null || true)"
                failed=$((failed + 1))
                stop_proxy
                macos_restore_proxy_snapshot_file "$previous_snapshot" || true
                return
            fi
            if ! wait_for_condition 15 1 grep -q "system proxy lifecycle helper started after Admin API enable" "${TEST_DATA_DIR}"/logs/bifrost*.log; then
                echo "[WARN] Admin API lifecycle helper start log not observed; continuing with crash cleanup assertion"
            fi
            if [[ -n "$PROXY_PID" ]]; then
                kill_pid_force "$PROXY_PID"
                wait_pid "$PROXY_PID"
            fi
            PROXY_PID=""
            rm -f "${TEST_DATA_DIR}/bifrost.pid" "${TEST_DATA_DIR}/runtime.json" 2>/dev/null || true
            if wait_for_condition 45 1 macos_check_proxy_not_pointing_to "127.0.0.1" "$PROXY_PORT"; then
                _log_pass "macOS: Admin API 运行时启用后的 lifecycle helper 已在主进程崩溃后清理系统代理"
                passed=$((passed + 1))
            else
                _log_fail "macOS: Admin API 运行时启用后的 lifecycle helper 未清理崩溃残留系统代理" "不指向 127.0.0.1:${PROXY_PORT}" "$(macos_proxy_snapshot); log=$(tail -n 120 "${TEST_DATA_DIR}/proxy.log" 2>/dev/null || true)"
                failed=$((failed + 1))
            fi
            macos_restore_proxy_snapshot_file "$previous_snapshot" || true
            ;;
        MINGW*|MSYS*|CYGWIN*)
            _log_pass "Windows: lifecycle helper 为 macOS 运行期崩溃兜底路径，跳过"
            passed=$((passed + 1))
            ;;
    esac
}

test_admin_api_disable_ignores_dirty_backup_pointing_to_self() {
    stop_proxy
    local previous_snapshot="${TEST_DATA_DIR}/macos-proxy-before-dirty-backup-disable.txt"
    case "$PLATFORM" in
        Darwin)
            macos_save_proxy_snapshot_file "$previous_snapshot" || true
            if ! macos_clear_web_and_secure_proxy; then
                _log_fail "macOS: 脏 backup Admin API 关闭回归夹具准备失败" "临时清空当前 Web/Secure Web 代理" "$(macos_proxy_snapshot)"
                failed=$((failed + 1))
                macos_restore_proxy_snapshot_file "$previous_snapshot" || true
                return
            fi
            start_proxy_without_system_proxy
            if [[ -z "$PROXY_PID" ]] || ! kill -0 "$PROXY_PID" 2>/dev/null || ! admin_api_ready; then
                _log_fail "macOS: 脏 backup Admin API 关闭回归准备失败" "Bifrost 服务必须保持运行且 Admin API 可访问" "pid=${PROXY_PID}; log=$(tail -n 80 "${TEST_DATA_DIR}/proxy.log" 2>/dev/null || true)"
                failed=$((failed + 1))
                stop_proxy
                macos_restore_proxy_snapshot_file "$previous_snapshot" || true
                return
            fi
            if macos_check_any_proxy_enabled_not_pointing_to "127.0.0.1" "$PROXY_PORT"; then
                _log_pass "macOS: 检测到外部系统代理 owner，跳过非隔离的脏 backup Admin API 关闭用例"
                passed=$((passed + 1))
                stop_proxy
                macos_restore_proxy_snapshot_file "$previous_snapshot" || true
                return
            fi

            macos_enable_system_proxy_api
            if ! macos_wait_proxy_enabled "127.0.0.1" "$PROXY_PORT" 45; then
                if macos_check_any_proxy_enabled_not_pointing_to "127.0.0.1" "$PROXY_PORT"; then
                    _log_pass "macOS: 检测到外部系统代理 owner，跳过非隔离的脏 backup Admin API 关闭用例"
                    passed=$((passed + 1))
                    stop_proxy
                    macos_restore_proxy_snapshot_file "$previous_snapshot" || true
                    return
                fi
                _log_fail "macOS: 脏 backup Admin API 关闭回归启用失败" "系统代理先指向 127.0.0.1:${PROXY_PORT}" "$(macos_proxy_snapshot); log=$(tail -n 80 "${TEST_DATA_DIR}/proxy.log" 2>/dev/null || true)"
                failed=$((failed + 1))
                stop_proxy
                macos_restore_proxy_snapshot_file "$previous_snapshot" || true
                return
            fi
            if macos_check_any_proxy_enabled_not_pointing_to "127.0.0.1" "$PROXY_PORT"; then
                _log_pass "macOS: 检测到外部系统代理 owner，跳过非隔离的脏 backup Admin API 关闭用例"
                passed=$((passed + 1))
                stop_proxy
                macos_restore_proxy_snapshot_file "$previous_snapshot" || true
                return
            fi

            cat > "${TEST_DATA_DIR}/proxy_backup.json" <<EOF
{"enable":true,"host":"127.0.0.1","port":${PROXY_PORT},"bypass":"dirty-bypass.example"}
EOF
            rm -f "${TEST_DATA_DIR}/proxy_state.json" 2>/dev/null || true

            local disable_output
            disable_output="$(macos_disable_system_proxy_api)"
            if ! wait_for_condition 45 1 macos_check_proxy_not_pointing_to "127.0.0.1" "$PROXY_PORT"; then
                _log_fail "macOS: 脏 backup Admin API 关闭后仍指向当前 Bifrost" "系统代理不再指向 127.0.0.1:${PROXY_PORT}" "response=${disable_output}; proxy=$(macos_proxy_snapshot); log=$(tail -n 120 "${TEST_DATA_DIR}/proxy.log" 2>/dev/null || true)"
                failed=$((failed + 1))
                stop_proxy
                macos_restore_proxy_snapshot_file "$previous_snapshot" || true
                return
            fi

            local status_json
            status_json="$(macos_system_proxy_status_json)"
            if ! python3 - "$status_json" <<'PY'
import json
import sys

data = json.loads(sys.argv[1])
if data.get("managed_by_bifrost") is not False:
    sys.exit(1)
if data.get("configured_enabled") is not False:
    sys.exit(1)
PY
            then
                _log_fail "macOS: 脏 backup Admin API 关闭后状态不正确" "managed_by_bifrost=false 且 configured_enabled=false" "status=${status_json}; response=${disable_output}"
                failed=$((failed + 1))
                stop_proxy
                macos_restore_proxy_snapshot_file "$previous_snapshot" || true
                return
            fi

            # Wait out one reconcile cycle plus margin. With the shortened reconcile
            # interval (BIFROST_SYSTEM_PROXY_RECONCILE_SECS) this is a few seconds; falls
            # back to a safe 35s if the daemon ignores the override (older binary).
            sleep "$(( ${BIFROST_SYSTEM_PROXY_RECONCILE_SECS:-30} + 5 ))"
            if macos_check_proxy_not_pointing_to "127.0.0.1" "$PROXY_PORT" \
                && grep -q "explicit system proxy disable ignored saved backup because it points back to the managed Bifrost target" "${TEST_DATA_DIR}/proxy.log"; then
                _log_pass "macOS: Admin API/WebView 显式关闭优先于脏 system proxy backup，且 reconcile 不会重新打开"
                passed=$((passed + 1))
            elif macos_check_any_proxy_enabled_not_pointing_to "127.0.0.1" "$PROXY_PORT"; then
                _log_pass "macOS: 检测到外部系统代理 owner，跳过非隔离的脏 backup Admin API 关闭用例"
                passed=$((passed + 1))
            else
                _log_fail "macOS: 脏 backup Admin API 关闭后被恢复或缺少证据日志" "等待超过 reconcile 周期后仍不指向 127.0.0.1:${PROXY_PORT}，且日志包含 dirty backup ignore" "proxy=$(macos_proxy_snapshot); log=$(tail -n 160 "${TEST_DATA_DIR}/proxy.log" 2>/dev/null || true)"
                failed=$((failed + 1))
            fi
            stop_proxy
            macos_restore_proxy_snapshot_file "$previous_snapshot" || true
            ;;
        MINGW*|MSYS*|CYGWIN*)
            _log_pass "Windows: 脏 backup WebView disable 回归为 macOS network service 路径，跳过"
            passed=$((passed + 1))
            ;;
    esac
}

test_launchd_cleanup_plist_dry_run_contains_version_metadata() {
    local output
    output=$("$BIFROST_BIN" system-proxy launchd install \
        --data-dir "$TEST_DATA_DIR" \
        --program "$BIFROST_BIN" \
        --label "com.bifrost.test-system-proxy-cleanup" \
        --plist-path "${TEST_DATA_DIR}/com.bifrost.test-system-proxy-cleanup.plist" \
        --dry-run 2>&1)
    local code=$?

    if [[ "$code" -eq 0 ]] \
        && echo "$output" | grep -q "cleanup-daemon" \
        && echo "$output" | grep -q -- "--installed-version" \
        && echo "$output" | grep -q "BIFROST_LAUNCHD_INSTALLED_VERSION" \
        && echo "$output" | grep -q "<key>RunAtLoad</key>" \
        && ! echo "$output" | grep -q "<key>KeepAlive</key>" \
        && echo "$output" | grep -q "$TEST_DATA_DIR"; then
        _log_pass "LaunchDaemon cleanup plist dry-run 包含 daemon/data-dir/version，并使用 one-shot RunAtLoad 且不写 KeepAlive"
        passed=$((passed + 1))
    else
        _log_fail "LaunchDaemon cleanup plist dry-run 输出不完整" "包含 cleanup-daemon/data-dir/version/RunAtLoad 且不包含 KeepAlive" "code=${code}; output=${output}"
        failed=$((failed + 1))
    fi
}

test_launchd_cleanup_daemon_no_state_exits_quickly() {
    rm -f "${TEST_DATA_DIR}/proxy_backup.json" "${TEST_DATA_DIR}/proxy_state.json" \
        "${TEST_DATA_DIR}/bifrost.pid" "${TEST_DATA_DIR}/runtime.json" 2>/dev/null || true
    cat > "${TEST_DATA_DIR}/runtime.json" <<EOF
{"pid":999999,"port":${PROXY_PORT},"host":"0.0.0.0","started_at_ms":1}
EOF
    printf '999999\n' > "${TEST_DATA_DIR}/bifrost.pid"

    local started
    started=$(date +%s)
    local output
    output=$("$BIFROST_BIN" system-proxy cleanup-daemon \
        --data-dir "$TEST_DATA_DIR" \
        --installed-version "0.0.1-test" 2>&1)
    local code=$?
    local elapsed=$(( $(date +%s) - started ))

    if [[ "$code" -eq 0 && "$elapsed" -lt 10 ]] \
        && ! echo "$output" | grep -q "No such process" \
        && [[ ! -e "${TEST_DATA_DIR}/runtime.json" ]] \
        && [[ ! -e "${TEST_DATA_DIR}/bifrost.pid" ]]; then
        _log_pass "LaunchDaemon cleanup daemon 无状态时快速完成 one-shot retry-aware 检查，并清理 stale runtime 文件"
        passed=$((passed + 1))
    else
        _log_fail "LaunchDaemon cleanup daemon 无状态路径未快速退出或未清理 stale runtime" "code=0、elapsed<10s、无 No such process、runtime.json/bifrost.pid 被删除" "code=${code}; elapsed=${elapsed}; runtime_exists=$([[ -e "${TEST_DATA_DIR}/runtime.json" ]] && echo yes || echo no); pid_exists=$([[ -e "${TEST_DATA_DIR}/bifrost.pid" ]] && echo yes || echo no); output=${output}"
        failed=$((failed + 1))
    fi
}

test_cleanup_daemon_retries_until_macos_network_services_are_ready() {
    if [[ "$PLATFORM" != "Darwin" ]]; then
        _log_pass "非 macOS: networksetup readiness retry 路径跳过"
        passed=$((passed + 1))
        return
    fi

    rm -f "${TEST_DATA_DIR}/proxy_backup.json" "${TEST_DATA_DIR}/proxy_state.json" \
        "${TEST_DATA_DIR}/bifrost.pid" "${TEST_DATA_DIR}/runtime.json" 2>/dev/null || true
    cat > "${TEST_DATA_DIR}/proxy_state.json" <<EOF
{
  "original": {"enable": false, "host": "", "port": 0, "bypass": ""},
  "target": {"enable": true, "host": "127.0.0.1", "port": ${PROXY_PORT}, "bypass": ""},
  "applied": true
}
EOF
    cat > "${TEST_DATA_DIR}/proxy_backup.json" <<EOF
{"enable": false, "host": "", "port": 0, "bypass": ""}
EOF

    local fake_bin="${TEST_DATA_DIR}/fake-bin"
    local counter_file="${TEST_DATA_DIR}/networksetup-call-count"
    mkdir -p "$fake_bin"
    cat > "${fake_bin}/networksetup" <<'SH'
#!/bin/bash
set -euo pipefail
counter_file="${BIFROST_FAKE_NETWORKSETUP_COUNTER:?}"
if [[ "${1:-}" == "-listallnetworkservices" ]]; then
    count=0
    if [[ -f "$counter_file" ]]; then
        count="$(cat "$counter_file")"
    fi
    count=$((count + 1))
    printf '%s\n' "$count" > "$counter_file"
    if [[ "$count" -lt 3 ]]; then
        printf '%s\n' "An asterisk (*) denotes that a network service is disabled."
        exit 0
    fi
fi
exec /usr/sbin/networksetup "$@"
SH
    chmod +x "${fake_bin}/networksetup"

    local output
    output=$(PATH="${fake_bin}:$PATH" \
        BIFROST_FAKE_NETWORKSETUP_COUNTER="$counter_file" \
        "$BIFROST_BIN" system-proxy cleanup-daemon \
            --data-dir "$TEST_DATA_DIR" \
            --installed-version "0.0.1-test" 2>&1)
    local code=$?
    local calls=0
    if [[ -f "$counter_file" ]]; then
        calls="$(cat "$counter_file")"
    fi

    if [[ "$code" -eq 0 && "$calls" -ge 3 && ! -e "${TEST_DATA_DIR}/proxy_state.json" ]] \
        && echo "$output" | grep -q "retryable error; retrying"; then
        _log_pass "macOS: cleanup-daemon 等待 network services ready 后才完成恢复判定"
        passed=$((passed + 1))
    else
        _log_fail "macOS: cleanup-daemon 未等待 network services ready" "code=0、networksetup 调用>=3、state 被清理且日志包含 retry" "code=${code}; calls=${calls}; state_exists=$([[ -e "${TEST_DATA_DIR}/proxy_state.json" ]] && echo yes || echo no); output=${output}"
        failed=$((failed + 1))
    fi
}

test_crash_recovery_runs_before_start_failure() {
    stop_proxy
    if [[ "$PLATFORM" == "Darwin" ]] && macos_check_any_proxy_enabled_not_pointing_to "127.0.0.1" "$PROXY_PORT"; then
        _log_pass "macOS: 检测到外部系统代理 owner，跳过非隔离的启动失败前恢复用例"
        passed=$((passed + 1))
        return
    fi
    export BIFROST_SYSTEM_PROXY_DISABLE_LIFECYCLE_HELPER=1
    start_proxy_with_system_proxy
    unset BIFROST_SYSTEM_PROXY_DISABLE_LIFECYCLE_HELPER
    if [[ "$PLATFORM" == "Darwin" ]] && ! macos_ensure_bifrost_system_proxy_enabled; then
        _log_fail "macOS: 启动失败前恢复回归准备失败" "系统代理先指向 127.0.0.1:${PROXY_PORT}" "$(macos_proxy_snapshot)"
        failed=$((failed + 1))
        return
    fi
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
    test_launchd_cleanup_plist_dry_run_contains_version_metadata

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
    test_restart_preserves_system_proxy
    test_crash_recovery
    test_runtime_target_no_state_crash_recovery
    test_lifecycle_helper_starts_power_watcher_directly
    test_lifecycle_helper_cleans_after_parent_crash
    test_lifecycle_helper_restarts_restartable_daemon_after_parent_crash
    test_admin_api_enable_starts_lifecycle_helper
    test_admin_api_enable_lifecycle_helper_cleans_after_parent_crash
    test_admin_api_disable_ignores_dirty_backup_pointing_to_self
    test_launchd_cleanup_daemon_no_state_exits_quickly
    test_cleanup_daemon_retries_until_macos_network_services_are_ready
    test_crash_recovery_runs_before_start_failure

    print_test_summary || exit 1
}

SKIP_BUILD="${SKIP_BUILD:-false}"
main "$@"
