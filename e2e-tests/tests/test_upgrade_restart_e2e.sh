#!/bin/bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

#
# Bifrost Upgrade Restart E2E 测试
# 测试 upgrade 完成后对运行中进程的检测与重启行为
#
# 测试策略：
# 由于真实 upgrade 需要网络和版本差异，我们通过以下方式验证：
# 1. 无 daemon 运行时 upgrade 不报错（不触发重启逻辑）
# 2. 有 daemon 运行时 upgrade（版本已最新不触发重启）不报错
# 3. --restart 参数与 daemon 模式启动的组合
# 4. 验证 runtime.json 中信息正确性，确保重启时能正确读取
# 5. 源码门禁验证 upgrade 的真实重启路径在 stop 后等待端口释放，避免 EADDRINUSE 崩溃

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

source "${PROJECT_DIR}/e2e-tests/test_utils/assert.sh"
source "${PROJECT_DIR}/e2e-tests/test_utils/process.sh"

PROXY_PORT="${PROXY_PORT:-18891}"
BIFROST_BIN="${PROJECT_DIR}/target/release/bifrost"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi

TEST_DATA_DIR=""
PROXY_PID=""

cleanup() {
    if is_windows; then kill_bifrost_on_port "$PROXY_PORT"; fi
    safe_cleanup_proxy "$PROXY_PID"

    if [[ -n "$TEST_DATA_DIR" ]] && [[ -d "$TEST_DATA_DIR" ]]; then
        rm -rf "$TEST_DATA_DIR"
    fi
}
trap cleanup EXIT

wait_proxy_ready() {
    local port="$1"
    local waited=0
    while [[ "$waited" -lt 60 ]]; do
        if curl -fsS "http://127.0.0.1:${port}/_bifrost/api/proxy/address" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.2
        waited=$((waited + 1))
    done
    return 1
}

run_bifrost() {
    BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" "$@" 2>&1 || true
}

start_daemon() {
    local log_file="${TEST_DATA_DIR}/proxy-${RANDOM}.log"
    BIFROST_DATA_DIR="${TEST_DATA_DIR}" "$BIFROST_BIN" start -d \
        -p "${PROXY_PORT}" \
        --skip-cert-check --unsafe-ssl --no-system-proxy -y \
        >"${log_file}" 2>&1
    local exit_code=$?

    if [[ $exit_code -ne 0 ]]; then
        echo "  [DEBUG] start -d exited with code $exit_code" >&2
        cat "${log_file}" >&2
        return 1
    fi

    sleep 1

    local pid
    pid="$(cat "${TEST_DATA_DIR}/bifrost.pid" 2>/dev/null || true)"
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
        PROXY_PID="$pid"
        return 0
    fi

    local waited=0
    while [[ "$waited" -lt 20 ]]; do
        pid="$(cat "${TEST_DATA_DIR}/bifrost.pid" 2>/dev/null || true)"
        if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
            PROXY_PID="$pid"
            return 0
        fi
        sleep 0.5
        waited=$((waited + 1))
    done

    echo "  [DEBUG] PID file content: $(cat "${TEST_DATA_DIR}/bifrost.pid" 2>/dev/null || echo 'empty')" >&2
    echo "  [DEBUG] log:" >&2
    cat "${log_file}" >&2
    return 1
}

stop_daemon() {
    BIFROST_DATA_DIR="${TEST_DATA_DIR}" "$BIFROST_BIN" stop >/dev/null 2>&1 || true
    sleep 1
    safe_cleanup_proxy "$PROXY_PID"
    PROXY_PID=""
    sleep 1
}

test_upgrade_no_daemon_no_error() {
    _log_info "case: upgrade without running daemon -> no error"

    local result
    result=$(run_bifrost upgrade -y --restart)
    local exit_ok=$?

    if echo "$result" | grep -qi "checking for updates\|latest version\|already on the latest\|could not check"; then
        _log_pass "upgrade without daemon runs normally"
    else
        _log_fail "upgrade without daemon" "normal output" "$result"
        return 1
    fi

    if echo "$result" | grep -qi "Detected running Bifrost proxy"; then
        _log_fail "no restart prompt without daemon" "no restart prompt" "restart prompt shown"
        return 1
    else
        _log_pass "no restart prompt when no daemon running"
    fi
}

test_upgrade_with_daemon_version_current() {
    _log_info "case: upgrade with daemon running but version already current"

    if ! start_daemon; then
        _log_fail "daemon started" "running" "failed to start"
        return 1
    fi

    if ! wait_proxy_ready "$PROXY_PORT"; then
        _log_fail "admin api ready" "reachable" "unreachable"
        return 1
    fi

    _log_pass "daemon started on port $PROXY_PORT (PID: $PROXY_PID)"

    local result
    result=$(run_bifrost upgrade -y)

    if echo "$result" | grep -qi "already on the latest\|could not check"; then
        _log_pass "upgrade correctly reports version status with daemon running"
    else
        _log_fail "upgrade output" "already latest or network error" "$result"
    fi

    if kill -0 "$PROXY_PID" 2>/dev/null; then
        _log_pass "daemon still running after upgrade (no version change)"
    else
        _log_fail "daemon still running" "running" "not running"
        return 1
    fi

    stop_daemon
}

test_runtime_json_contains_correct_info() {
    _log_info "case: runtime.json stores correct info for restart args"

    if ! start_daemon; then
        _log_fail "daemon started" "running" "failed to start"
        return 1
    fi

    if ! wait_proxy_ready "$PROXY_PORT"; then
        _log_fail "admin api ready" "reachable" "unreachable"
        return 1
    fi

    local runtime_file="${TEST_DATA_DIR}/runtime.json"
    if [[ ! -f "$runtime_file" ]]; then
        _log_fail "runtime.json exists" "file exists" "file not found"
        stop_daemon
        return 1
    fi

    _log_pass "runtime.json exists"

    if command -v jq >/dev/null 2>&1; then
        local rt_pid rt_port
        rt_pid="$(jq -r '.pid' "$runtime_file" 2>/dev/null || echo "")"
        rt_port="$(jq -r '.port' "$runtime_file" 2>/dev/null || echo "")"

        assert_equals "$PROXY_PID" "$rt_pid" "runtime.json pid matches daemon PID" || true
        assert_equals "$PROXY_PORT" "$rt_port" "runtime.json port matches configured port" || true
    else
        if grep -q "\"port\"" "$runtime_file" && grep -q "\"pid\"" "$runtime_file"; then
            _log_pass "runtime.json contains required fields (no jq to verify values)"
        else
            _log_fail "runtime.json fields" "port and pid fields" "missing fields"
        fi
    fi

    stop_daemon
}

test_upgrade_restart_flag_with_daemon_no_update() {
    _log_info "case: upgrade --restart with daemon but no version update -> daemon stays"

    if ! start_daemon; then
        _log_fail "daemon started" "running" "failed to start"
        return 1
    fi

    if ! wait_proxy_ready "$PROXY_PORT"; then
        _log_fail "admin api ready" "reachable" "unreachable"
        return 1
    fi

    local old_pid="$PROXY_PID"

    local result
    result=$(run_bifrost upgrade -y --restart)

    if echo "$result" | grep -qi "already on the latest\|could not check"; then
        _log_pass "upgrade --restart reports version status correctly"
    else
        _log_fail "upgrade --restart output" "version status" "$result"
    fi

    if kill -0 "$old_pid" 2>/dev/null; then
        _log_pass "daemon not restarted (no version change, even with --restart)"
    else
        _log_fail "daemon still running" "running" "not running (incorrectly restarted)"
        PROXY_PID=""
        return 1
    fi

    stop_daemon
}

test_upgrade_restart_flag_in_help() {
    _log_info "case: --restart flag documented in help"

    local result
    result=$("$BIFROST_BIN" upgrade --help 2>&1 || true)

    if echo "$result" | grep -q "\-\-restart"; then
        _log_pass "--restart flag present in upgrade help"
    else
        _log_fail "--restart in help" "flag listed" "not found"
        return 1
    fi

    if echo "$result" | grep -qi "restart.*running\|running.*proxy\|automatically restart"; then
        _log_pass "--restart help description mentions proxy restart"
    else
        _log_fail "--restart description" "mentions restart" "description unclear"
    fi
}

test_upgrade_restart_port_release_guard_in_source() {
    _log_info "case: upgrade restart path waits for port release before start"

    local source_file="${PROJECT_DIR}/crates/bifrost-cli/src/commands/upgrade.rs"

    if grep -q "wait_for_restart_port_release(restart_port)" "$source_file" \
        && grep -q "Proxy port .*still occupied after" "$source_file" \
        && grep -q "find_process_on_port(port)" "$source_file" \
        && grep -q "recover_from_crash(&data_dir)" "$source_file"; then
        _log_pass "upgrade restart has port-release guard, listener diagnostics, and system proxy recovery"
    else
        _log_fail "upgrade restart port guard" \
            "wait_for_restart_port_release plus occupied-port diagnostics and system proxy recovery" \
            "guard missing from upgrade.rs"
        return 1
    fi
}

test_upgrade_restart_port_guard_covers_windows() {
    _log_info "case: upgrade restart port-release guard is not unix-only"

    local upgrade_src="${PROJECT_DIR}/crates/bifrost-cli/src/commands/upgrade.rs"
    local process_src="${PROJECT_DIR}/crates/bifrost-cli/src/process.rs"

    # The active guard must compile on both Unix and Windows, and the only
    # no-op fallback may target neither-unix-nor-windows platforms. The shared
    # wait helper must likewise be available on Windows, otherwise Windows
    # upgrades silently fall back to the racy stop-then-start path.
    # Note: use portable grep only (macOS ships BSD grep without -P/-z).
    local wait_helper_cfg
    wait_helper_cfg="$(grep -B1 'pub fn wait_for_port_released' "$process_src" | head -1)"

    if grep -q '#\[cfg(any(unix, windows))\]' "$upgrade_src" \
        && grep -q '#\[cfg(not(any(unix, windows)))\]' "$upgrade_src" \
        && ! grep -q '#\[cfg(not(unix))\]' "$upgrade_src" \
        && printf '%s' "$wait_helper_cfg" | grep -q 'cfg(any(unix, windows))'; then
        _log_pass "upgrade restart port-release guard and wait helper cover Windows"
    else
        _log_fail "upgrade restart windows coverage" \
            "wait_for_restart_port_release + wait_for_port_released gated for any(unix, windows)" \
            "guard or wait helper is still unix-only"
        return 1
    fi
}

main() {
    TEST_DATA_DIR="$(mktemp -d)"

    test_upgrade_restart_flag_in_help || true
    test_upgrade_restart_port_release_guard_in_source || true
    test_upgrade_restart_port_guard_covers_windows || true
    test_upgrade_no_daemon_no_error || true
    test_upgrade_with_daemon_version_current || true
    test_runtime_json_contains_correct_info || true
    test_upgrade_restart_flag_with_daemon_no_update || true

    print_test_summary || exit 1
}

main "$@"
