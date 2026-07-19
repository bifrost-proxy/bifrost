#!/bin/bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_DISABLE_TRAY

#
# Bifrost Upgrade Restart E2E 测试
# 测试 upgrade 完成后对运行中进程的检测与重启行为
#
# 测试策略：
# 由于真实 upgrade 需要网络和版本差异，我们通过以下方式验证：
# 1. 无 daemon 运行时 upgrade 不报错（不触发重启逻辑）
# 2. 有 daemon 运行时 upgrade（版本已最新不触发重启）不报错
# 3. 默认自动重启语义与 daemon 模式启动的组合
# 4. 验证 runtime.json 中信息正确性，确保重启时能正确读取
# 5. 源码门禁验证 upgrade 的真实重启路径在 stop 后等待端口释放，避免 EADDRINUSE 崩溃

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

source "${PROJECT_DIR}/e2e-tests/test_utils/assert.sh"
source "${PROJECT_DIR}/e2e-tests/test_utils/process.sh"

PROXY_PORT="${PROXY_PORT:-18891}"
BIFROST_BIN="${BIFROST_BIN:-${PROJECT_DIR}/target/release/bifrost}"
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
    local exit_code=0
    if [[ "${BIFROST_COVERAGE_E2E:-0}" == "1" ]]; then
        # LLVM-instrumented binaries must stay alive to flush their profile and
        # therefore cannot use the normal detached-daemon launcher contract.
        BIFROST_DATA_DIR="${TEST_DATA_DIR}" "$BIFROST_BIN" start \
            -p "${PROXY_PORT}" \
            --skip-cert-check --unsafe-ssl --no-system-proxy -y \
            >"${log_file}" 2>&1 &
        PROXY_PID=$!
    else
        BIFROST_DATA_DIR="${TEST_DATA_DIR}" "$BIFROST_BIN" start -d \
            -p "${PROXY_PORT}" \
            --skip-cert-check --unsafe-ssl --no-system-proxy -y \
            >"${log_file}" 2>&1
        exit_code=$?
    fi

    if [[ $exit_code -ne 0 ]]; then
        echo "  [DEBUG] start -d exited with code $exit_code" >&2
        cat "${log_file}" >&2
        return 1
    fi

    sleep 1

    local pid="${PROXY_PID}"
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

assert_no_tray_helper_for_test_data_dir() {
    if is_windows; then
        return 0
    fi

    local matches
    matches="$(ps -axo command 2>/dev/null | grep -F 'bifrost __tray' | grep -F -- "$TEST_DATA_DIR" || true)"
    if [[ -n "$matches" ]]; then
        _log_fail "daemon stop cleaned tray helper" "no __tray helper for $TEST_DATA_DIR" "$matches"
        return 1
    fi

    _log_pass "daemon stop leaves no tray helper for test data dir"
}

stop_daemon() {
    BIFROST_DATA_DIR="${TEST_DATA_DIR}" "$BIFROST_BIN" stop >/dev/null 2>&1 || true
    sleep 1
    safe_cleanup_proxy "$PROXY_PID"
    PROXY_PID=""
    sleep 1
    assert_no_tray_helper_for_test_data_dir
}


test_upgrade_no_daemon_no_error() {
    _log_info "case: upgrade without running daemon -> no error"

    local result
    result=$(run_bifrost upgrade)
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
    result=$(run_bifrost upgrade)

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

test_upgrade_default_with_daemon_no_update() {
    _log_info "case: upgrade with daemon but no version update -> daemon stays"

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
    result=$(run_bifrost upgrade)

    if echo "$result" | grep -qi "already on the latest\|could not check"; then
        _log_pass "upgrade reports version status correctly"
    else
        _log_fail "upgrade output" "version status" "$result"
    fi

    if kill -0 "$old_pid" 2>/dev/null; then
        _log_pass "daemon not restarted when no version changed"
    else
        _log_fail "daemon still running" "running" "not running (incorrectly restarted)"
        PROXY_PID=""
        return 1
    fi

    stop_daemon
}

test_upgrade_restart_flag_removed_from_help() {
    _log_info "case: --restart flag removed from help"

    local result
    result=$("$BIFROST_BIN" upgrade --help 2>&1 || true)

    if ! echo "$result" | grep -q "\-\-restart"; then
        _log_pass "--restart flag absent from upgrade help"
    else
        _log_fail "--restart in help" "flag removed" "still listed"
        return 1
    fi

    if echo "$result" | grep -qi "restart.*running\|running.*proxy\|automatically restart"; then
        _log_pass "upgrade help description mentions default proxy restart"
    else
        _log_fail "upgrade restart description" "mentions default restart" "description unclear"
    fi
}

test_upgrade_restart_port_release_guard_in_source() {
    _log_info "case: upgrade restart path waits for port release before start"

    local source_file="${PROJECT_DIR}/crates/bifrost-cli/src/commands/upgrade.rs"
    local restart_file="${PROJECT_DIR}/crates/bifrost-cli/src/commands/upgrade/restart.rs"

    if grep -q "wait_for_restart_ports_release(&restart_ports)" "$restart_file" \
        && grep -q "fn restart_ports_from_runtime" "$restart_file" \
        && grep -q "info.socks5_port" "$restart_file" \
        && grep -q "restart_executable_for_install_method(&install_method)" "$source_file" \
        && grep -q "maybe_restart_running_proxy(&restart_executable)" "$source_file" \
        && grep -q "Command::new(restart_executable)" "$restart_file" \
        && grep -q "Proxy port .*still occupied after" "$restart_file" \
        && grep -q "find_process_on_port(port)" "$restart_file" \
        && grep -q "recover_from_crash(&data_dir)" "$restart_file"; then
        _log_pass "upgrade restart has multi-port release guard, fixed restart executable, listener diagnostics, and system proxy recovery"
    else
        _log_fail "upgrade restart port guard" \
            "wait_for_restart_ports_release plus occupied-port diagnostics, socks5 coverage, fixed restart executable, and system proxy recovery" \
            "guard missing from upgrade.rs"
        return 1
    fi
}

test_upgrade_restart_port_guard_covers_windows() {
    _log_info "case: upgrade restart port-release guard is not unix-only"

    local upgrade_src="${PROJECT_DIR}/crates/bifrost-cli/src/commands/upgrade/restart.rs"
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

test_macos_daemon_start_uses_exec_child_guard() {
    _log_info "case: macOS and Windows daemon start use exec child guards"

    local start_src="${PROJECT_DIR}/crates/bifrost-cli/src/commands/start.rs"
    local main_src="${PROJECT_DIR}/crates/bifrost-cli/src/main.rs"
    local daemon_exec_cfg_ok=0
    if awk '
        prev == "#[cfg(any(unix, windows))]" && $0 ~ /^fn run_daemon_via_exec/ { found = 1 }
        { prev = $0 }
        END { exit found ? 0 : 1 }
    ' "$start_src"; then
        daemon_exec_cfg_ok=1
    fi

    if grep -Fq 'run_daemon_via_exec' "$start_src" \
        && grep -Fq 'BIFROST_DETACHED_DAEMON_CHILD' "$start_src" \
        && grep -Fq 'foreground_runtime_start_mode()' "$start_src" \
        && [ "$daemon_exec_cfg_ok" = "1" ] \
        && grep -Fq 'detached_daemon_readiness_host' "$start_src" \
        && grep -Fq 'current_dir(&bifrost_dir)' "$start_src" \
        && grep -Fq 'libc::setsid()' "$start_src" \
        && grep -Fq 'std::os::windows::process::CommandExt' "$start_src" \
        && grep -Fq 'DETACHED_PROCESS' "$start_src" \
        && grep -Fq '#[cfg(windows)]' "$start_src" \
        && grep -Fq 'run_daemon_via_exec(&proxy_config, &config_manager, &log_dir, log_retention_days)' "$start_src" \
        && grep -Fq 'is_detached_daemon_child_process' "$main_src" \
        && grep -Fq 'daemon && !is_detached_daemon_child' "$main_src"; then
        _log_pass "macOS and Windows daemon start exec a fresh detached child before runtime init"
    else
        _log_fail "daemon exec child guard" \
            "run_daemon_via_exec + unix/windows cfg + detached child env + main daemon bypass + setsid/current_dir + Windows detached process" \
            "cross-platform detached daemon guard missing"
        return 1
    fi
}

test_upgrade_installs_binary_atomically_in_source() {
    _log_info "case: upgrade installs binary through atomic replacement helper"

    local source_file="${PROJECT_DIR}/crates/bifrost-cli/src/commands/upgrade.rs"

    if grep -q "fn install_binary_atomically" "$source_file" \
        && grep -q "install_binary_atomically(&new_binary, target_path, version)" "$source_file" \
        && grep -q "fs::rename(&temp_target, target_path)" "$source_file" \
        && ! grep -q "fs::copy(&new_binary, target_path)" "$source_file"; then
        _log_pass "upgrade uses temp file plus rename instead of copying directly to final binary path"
    else
        _log_fail "upgrade atomic binary replacement" \
            "install_binary_atomically with temp rename and no fs::copy(&new_binary, target_path)" \
            "upgrade can still expose a partially copied executable"
        return 1
    fi
}

test_windows_upgrade_defers_self_replacement_in_source() {
    _log_info "case: Windows upgrade defers self replacement until current exe exits"

    local source_file="${PROJECT_DIR}/crates/bifrost-cli/src/commands/upgrade.rs"
    local restart_file="${PROJECT_DIR}/crates/bifrost-cli/src/commands/upgrade/restart.rs"

    if grep -q "DeferredWindows" "$source_file" \
        && grep -q "unique_pending_binary_path" "$source_file" \
        && grep -q "schedule_windows_deferred_install" "$restart_file" \
        && grep -q "Wait-Process -Timeout 120" "$restart_file" \
        && grep -Fq 'Move-Item -LiteralPath $PendingPath -Destination $TargetPath -Force' "$restart_file" \
        && grep -Fq 'Start-Process -FilePath $TargetPath -ArgumentList $restartArgs' "$restart_file" \
        && grep -q "Proxy restart scheduled with the new version" "$restart_file"; then
        _log_pass "Windows upgrade stages self replacement and restarts after the upgrade process exits"
    else
        _log_fail "Windows deferred self replacement" \
            "DeferredWindows + pending exe + PowerShell wait/replace/start helper" \
            "Windows upgrade can still try to overwrite the running exe directly"
        return 1
    fi
}

test_upgrade_review_feedback_contracts() {
    _log_info "case: upgrade review feedback contracts remain enforced"

    local app_src="${PROJECT_DIR}/crates/bifrost-cli/src/commands/app.rs"
    local installer_src="${PROJECT_DIR}/crates/bifrost-cli/src/commands/app/installer.rs"
    local app_tests="${PROJECT_DIR}/crates/bifrost-cli/src/commands/app/tests.rs"
    local upgrade_src="${PROJECT_DIR}/crates/bifrost-cli/src/commands/upgrade.rs"
    local upgrade_desktop_src="${PROJECT_DIR}/crates/bifrost-cli/src/commands/upgrade/desktop_companion.rs"
    local upgrade_download_src="${PROJECT_DIR}/crates/bifrost-cli/src/commands/upgrade/download.rs"
    local upgrade_restart_src="${PROJECT_DIR}/crates/bifrost-cli/src/commands/upgrade/restart.rs"
    local upgrade_tests="${PROJECT_DIR}/crates/bifrost-cli/src/commands/upgrade/tests.rs"
    local upgrade_review_tests="${PROJECT_DIR}/crates/bifrost-cli/src/commands/upgrade/tests/review_comments.rs"
    local background_src="${PROJECT_DIR}/crates/bifrost-cli/src/commands/upgrade_background.rs"
    local admin_src="${PROJECT_DIR}/crates/bifrost-admin/src/handlers/system.rs"
    local admin_version_src="${PROJECT_DIR}/crates/bifrost-admin/src/handlers/system/version_companion.rs"
    local desktop_src="${PROJECT_DIR}/desktop/src-tauri/src/main.rs"
    local desktop_upgrade_src="${PROJECT_DIR}/desktop/src-tauri/src/upgrade_handoff.rs"
    local desktop_backend_src="${PROJECT_DIR}/desktop/src-tauri/src/backend_runtime.rs"
    local desktop_tests_src="${PROJECT_DIR}/desktop/src-tauri/src/tests.rs"
    local web_store_src="${PROJECT_DIR}/web/src/stores/useVersionStore.ts"

    if [ "$(wc -l <"$app_src")" -le 1500 ] \
        && [ "$(wc -l <"$installer_src")" -le 1500 ] \
        && [ "$(wc -l <"$app_tests")" -le 1500 ] \
        && [ "$(wc -l <"$upgrade_src")" -le 1500 ] \
        && [ "$(wc -l <"$upgrade_desktop_src")" -le 1500 ] \
        && [ "$(wc -l <"$upgrade_download_src")" -le 1500 ] \
        && [ "$(wc -l <"$upgrade_restart_src")" -le 1500 ] \
        && [ "$(wc -l <"$upgrade_tests")" -le 1500 ] \
        && [ "$(wc -l <"$upgrade_review_tests")" -le 1500 ] \
        && [ "$(wc -l <"$admin_src")" -le 1500 ] \
        && [ "$(wc -l <"$admin_version_src")" -le 1500 ] \
        && [ "$(wc -l <"$desktop_src")" -le 1500 ] \
        && [ "$(wc -l <"$desktop_upgrade_src")" -le 1500 ] \
        && [ "$(wc -l <"$desktop_backend_src")" -le 1500 ] \
        && [ "$(wc -l <"$desktop_tests_src")" -le 1500 ] \
        && grep -Fq 'parent.join(format!(".{name}.backup"))' "$app_src" \
        && grep -Fq 'verify_installed_cli_target_version_or_restore' "$upgrade_src" \
        && grep -Fq 'or_else(|| Some("127.0.0.1".to_string()))' "$upgrade_restart_src" \
        && grep -Fq 'info.start_mode != RuntimeStartMode::Desktop' "$upgrade_restart_src" \
        && grep -Fq 'preserving progress owned by another updater' "$background_src" \
        && ! grep -Fq 'desktop_app_install_dir_for_upgrade' "$admin_src" \
        && grep -Fq 'macos_app_dir_from_exe_path' "$app_src" \
        && grep -Fq 'defer_desktop_install_to_handoff' "$installer_src" \
        && grep -Fq 'acquire_top_level_app_upgrade_lock' "$app_src" \
        && grep -Fq 'try_acquire_upgrade_lock_attempt' "$installer_src" \
        && grep -Fq 'handle_app_managed_upgrade(target_version.to_string())' "$app_src" \
        && grep -Fq 'app_managed_upgrade_behavior()' "$upgrade_src" \
        && grep -Fq 'installed_desktop_app_is_running' "$upgrade_desktop_src" \
        && grep -Fq 'DesktopCompanionMode::DesktopHandoff' "$upgrade_desktop_src" \
        && grep -Fq 'desktop_companion_environment' "$upgrade_desktop_src" \
        && grep -Fq 'PARENT_UPGRADE_LOCK_HELD_ENV' "$upgrade_desktop_src" \
        && grep -Fq 'WEBVIEW_UPGRADE_ORIGIN_ENV' "$upgrade_desktop_src" \
        && grep -Fq 'should_request_desktop_shutdown_before_update' "$upgrade_desktop_src" \
        && grep -Fq 'DESKTOP_UPGRADE_SHUTDOWN_ARG' "$upgrade_desktop_src" \
        && grep -Fq 'request_legacy_desktop_shutdown' "$upgrade_desktop_src" \
        && grep -Fq 'PARENT_UPGRADE_LOCK_HELD_ENV' "$app_src" \
        && grep -Fq 'WEBVIEW_UPGRADE_ORIGIN_ENV' "$admin_src" \
        && grep -Fq 'desktop_upgrade_shutdown_requested' "$desktop_src" \
        && grep -Fq 'macos_app_bundle_from_executable' "$admin_version_src" \
        && grep -Fq 'spawn_windows_desktop_upgrade_handoff' "$desktop_upgrade_src" \
        && grep -Fq 'deferred_desktop_install_version_error' "$desktop_upgrade_src" \
        && grep -Fq 'package_owned_by_updater' "$installer_src" \
        && grep -Fq 'progress.source === "desktop"' "$web_store_src" \
        && grep -Fq 'persist_desktop_upgrade_handoff_failure' "$desktop_upgrade_src" \
        && grep -Fq 'read_installed_cli_version_with_timeout' "$app_src"; then
        _log_pass "review feedback contracts cover recovery, ownership, rollback, shared locking, pinned targets, verified deferred desktop install, active app path, and module size"
    else
        _log_fail "upgrade review feedback contracts" \
            "stable backup + exact rollback + safe marker + shared App lock + pinned target + active app path + foreground restart + verified deferred desktop install + source-gated handoff + caller package preservation + persisted handoff + bounded modules" \
            "one or more contracts are missing"
        return 1
    fi
}

main() {
    TEST_DATA_DIR="$(mktemp -d)"

    test_upgrade_restart_flag_removed_from_help || true
    test_upgrade_restart_port_release_guard_in_source || true
    test_upgrade_restart_port_guard_covers_windows || true
    test_macos_daemon_start_uses_exec_child_guard || true
    test_upgrade_installs_binary_atomically_in_source || true
    test_windows_upgrade_defers_self_replacement_in_source || true
    test_upgrade_review_feedback_contracts || true
    test_upgrade_no_daemon_no_error || true
    test_upgrade_with_daemon_version_current || true
    test_runtime_json_contains_correct_info || true
    test_upgrade_default_with_daemon_no_update || true

    print_test_summary || exit 1
}

main "$@"
