#!/bin/bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

# This test spawns daemons via `bifrost start --daemon` and relies on the parent
# process detaching (forking the child and returning control). If the harness
# that launches this script was itself started as a detached daemon child, it may
# export BIFROST_DETACHED_DAEMON_CHILD=1; that env var would be inherited here and
# force `start --daemon` to run in the FOREGROUND (it tells bifrost "you are the
# already-detached child"), hanging the test at the start step. Clear it so the
# daemon spawn always detaches, independent of how this script was invoked.
unset BIFROST_DETACHED_DAEMON_CHILD BIFROST_DESKTOP_CORE BIFROST_EXTERNAL_CLI_WORKER

#
# Bifrost Admin-API Upgrade E2E 测试
#
# 目标：真实地、端到端地验证 "Admin UI 立即更新" 这条新链路，而不是 mock 数据。
#
#   BIFROST_EXTERNAL_CLI_WORKER=1 bifrost upgrade
#     -> CLI POST /_bifrost/api/system/upgrade?channel=cli and exits promptly
#     -> 写入初始 Checking 进度
#     -> spawn `bifrost self-update --source admin` 子进程（继承 admin 进程环境）
#     -> 子进程真实执行：解包本地 release 归档 -> 原子替换二进制 -> verify_binary
#        -> 停止旧 daemon -> 用新二进制重启 daemon
#     -> 进度文件写入 Completed
#   GET /_bifrost/api/system/upgrade/progress  全程可读，最终 phase=completed
#
# 与 test_upgrade_local_restart_e2e.sh 的区别：
#   那条测试走 CLI `bifrost upgrade` 默认自动重启；本测试走 Admin HTTP 端点，覆盖
#   本次新增的 admin 路由 + self-update 子进程派发 + 跨重启的进度文件通道。
#
# 真实性保证：
#   - 不 mock 升级逻辑。仅用 BIFROST_UPGRADE_TEST_ARCHIVE 把"网络下载源"替换成
#     一个真实的本地 .tar.xz 归档；解包/原子替换/二进制校验/进程重启全部真实执行。
#   - 断言 worker CLI 只提交任务并及时退出；后台 updater 不依赖调用者存活。
#   - 断言旧 daemon PID 与新 daemon PID 不同，证明确实发生了停止+重启。
#   - 断言运行中的二进制路径指向被替换的安装路径，证明确实换了二进制。
#   - 断言进度文件最终 phase=completed 且 source=admin，证明新链路打通。

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

source "${PROJECT_DIR}/e2e-tests/test_utils/assert.sh"
source "${PROJECT_DIR}/e2e-tests/test_utils/process.sh"

BIFROST_BIN="${BIFROST_BIN:-${PROJECT_DIR}/target/release/bifrost}"
if [[ ! -x "$BIFROST_BIN" && -x "${PROJECT_DIR}/target/debug/bifrost" ]]; then
    BIFROST_BIN="${PROJECT_DIR}/target/debug/bifrost"
fi

# Derive the upgrade target version from the workspace version (patch + 1) so the
# self-upgrade actually sees a newer release.
_default_test_version() {
    local ws_version major minor patch
    ws_version="$(grep -m1 '^version' "${PROJECT_DIR}/Cargo.toml" 2>/dev/null \
        | sed -E 's/.*"([0-9]+\.[0-9]+\.[0-9]+)".*/\1/')"
    if [[ ! "$ws_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
        echo "0.0.101"
        return
    fi
    IFS='.' read -r major minor patch <<<"$ws_version"
    echo "${major}.${minor}.$((patch + 1))"
}
TEST_VERSION="${BIFROST_UPGRADE_E2E_VERSION:-$(_default_test_version)}"

TEST_ROOT=""
TEST_DATA_DIR=""
INSTALL_BIN=""
PROXY_PORT=""

cleanup() {
    if [[ -n "$TEST_DATA_DIR" && -x "$INSTALL_BIN" ]]; then
        BIFROST_DATA_DIR="$TEST_DATA_DIR" "$INSTALL_BIN" stop >/dev/null 2>&1 || true
    fi
    if [[ -n "$PROXY_PORT" ]]; then
        kill_bifrost_on_port "$PROXY_PORT" 2>/dev/null || true
    fi
    if [[ -n "$TEST_ROOT" && -d "$TEST_ROOT" ]]; then
        rm -rf "$TEST_ROOT"
    fi
}
trap cleanup EXIT

host_triple() {
    rustc -vV | awk '/^host:/ {print $2}'
}

allocate_free_port() {
    local py
    py="$(python3_cmd 2>/dev/null || true)"
    if [[ -n "$py" ]]; then
        "$py" - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
        return 0
    fi
    return 1
}

pid_is_running() {
    kill -0 "$1" 2>/dev/null
}

# Build a real release archive (.tar.xz) containing the freshly built binary,
# laid out exactly like a GitHub release artifact: bifrost-v<ver>-<triple>/bifrost
create_local_release_archive() {
    local archive_root="$1" version="$2" target="$3" binary="$4"
    local package_dir="${archive_root}/bifrost-v${version}-${target}"
    mkdir -p "$package_dir"
    cp "$binary" "${package_dir}/bifrost"
    chmod +x "${package_dir}/bifrost" 2>/dev/null || true

    # The checked-in workspace version cannot be bumped merely to manufacture
    # a newer E2E release. Patch the equal-length version bytes in this temporary
    # copy, then execute it before packaging. This keeps the real binary's
    # install/restart behavior while making post-install version verification
    # meaningful instead of relying on a test-only bypass.
    local current_version py
    current_version="$("$binary" --version 2>/dev/null | awk '{print $NF}' | sed 's/^v//')"
    if [[ "$current_version" != "$version" ]]; then
        if [[ ${#current_version} -ne ${#version} ]]; then
            echo "cannot patch E2E binary version with a different byte length" >&2
            return 1
        fi
        py="$(python3_cmd 2>/dev/null || true)"
        [[ -z "$py" ]] && return 1
        "$py" - "${package_dir}/bifrost" "$current_version" "$version" <<'PY'
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
old = sys.argv[2].encode()
new = sys.argv[3].encode()
data = path.read_bytes()
count = data.count(old)
if count == 0:
    raise SystemExit(f"compiled version bytes {old!r} not found in {path}")
path.write_bytes(data.replace(old, new))
PY
        if [[ "$(uname -s)" == "Darwin" ]] && command -v codesign >/dev/null 2>&1; then
            codesign --force --sign - "${package_dir}/bifrost" >/dev/null 2>&1
        fi
    fi
    local packaged_version
    packaged_version="$("${package_dir}/bifrost" --version 2>/dev/null | awk '{print $NF}' | sed 's/^v//')"
    if [[ "$packaged_version" != "$version" ]]; then
        echo "temporary E2E binary reports ${packaged_version:-unknown}, expected $version" >&2
        return 1
    fi

    local archive_path="${archive_root}/bifrost-v${version}-${target}.tar.xz"
    tar -C "$archive_root" -cJf "$archive_path" "bifrost-v${version}-${target}"
    echo "$archive_path"
}

# Seed version_cache.json so the admin /version-check reports has_update=true
# offline (no GitHub network), unblocking POST /api/system/upgrade's gate.
seed_version_cache() {
    local data_dir="$1" version="$2"
    local py
    py="$(python3_cmd 2>/dev/null || true)"
    [[ -z "$py" ]] && return 1
    mkdir -p "$data_dir"
    "$py" - "$data_dir/version_cache.json" "$version" <<'PY'
import json, sys, datetime
path, version = sys.argv[1], sys.argv[2]
now = datetime.datetime.now(datetime.timezone.utc).isoformat()
json.dump({
    "latest_version": version,
    "release_highlights": ["admin api upgrade e2e"],
    "checked_at": now,
}, open(path, "w"))
PY
}

admin_curl() {
    local method="$1" path="$2"
    env NO_PROXY='*' no_proxy='*' curl -sS -X "$method" \
        --connect-timeout 2 --max-time 15 \
        -H "Accept: application/json" \
        "http://127.0.0.1:${PROXY_PORT}/_bifrost${path}" 2>/dev/null
}

json_field() {
    local json="$1" field="$2"
    local py
    py="$(python3_cmd 2>/dev/null || true)"
    [[ -z "$py" ]] && return 1
    printf '%s' "$json" | "$py" -c "import json,sys
try:
    print(json.load(sys.stdin).get('$field', ''))
except Exception:
    print('')"
}

read_runtime_field() {
    local field="$1"
    local py
    py="$(python3_cmd 2>/dev/null || true)"
    [[ -z "$py" ]] && return 1
    "$py" - "${TEST_DATA_DIR}/runtime.json" "$field" <<'PY'
import json, sys
try:
    print(json.load(open(sys.argv[1])).get(sys.argv[2], ""))
except Exception:
    print("")
PY
}

wait_admin_ready() {
    local waited=0
    while [[ $waited -lt 100 ]]; do
        if admin_curl GET /api/system >/dev/null 2>&1 \
            && [[ -n "$(admin_curl GET /api/system)" ]]; then
            return 0
        fi
        sleep 0.2
        waited=$((waited + 1))
    done
    return 1
}

test_admin_api_upgrade_restarts_daemon_with_new_binary() {
    _log_info "case: admin POST /api/system/upgrade performs a real download->install->restart"

    if [[ ! -x "$BIFROST_BIN" ]]; then
        _log_fail "bifrost binary exists" "$BIFROST_BIN" "missing"
        return 1
    fi

    TEST_ROOT="$(mktemp -d)"
    TEST_DATA_DIR="${TEST_ROOT}/data"
    local install_dir="${TEST_ROOT}/install"
    local archive_root="${TEST_ROOT}/archive"
    mkdir -p "$TEST_DATA_DIR" "$install_dir" "$archive_root"

    INSTALL_BIN="${install_dir}/bifrost"
    cp "$BIFROST_BIN" "$INSTALL_BIN"
    chmod +x "$INSTALL_BIN" 2>/dev/null || true

    PROXY_PORT="$(allocate_free_port)"
    if [[ -z "$PROXY_PORT" || ! "$PROXY_PORT" =~ ^[0-9]+$ ]]; then
        _log_fail "free proxy port allocated" "numeric port" "${PROXY_PORT:-empty}"
        return 1
    fi

    local target archive_path
    target="$(host_triple)"
    archive_path="$(create_local_release_archive "$archive_root" "$TEST_VERSION" "$target" "$BIFROST_BIN")"
    if [[ ! -f "$archive_path" ]]; then
        _log_fail "local release archive built" "$archive_path" "missing"
        return 1
    fi
    _log_pass "built real local release archive: $(basename "$archive_path")"

    seed_version_cache "$TEST_DATA_DIR" "$TEST_VERSION" || {
        _log_fail "seed version cache" "python3 available" "unavailable"
        return 1
    }

    # `--no-tray` only exists on builds that ship the tray helper (macOS/Windows);
    # the Linux `start` command does not define the flag (it is gated behind
    # `#[cfg(not(target_os = "linux"))]`), so passing it there aborts the daemon
    # with "unexpected argument '--no-tray'". Add it conditionally.
    local -a tray_args=()
    if [[ "$(uname -s)" != "Linux" ]]; then
        tray_args=(--no-tray)
    fi

    # Start the daemon with the upgrade test-override env vars in its environment.
    # The admin server runs inside this process; the self-update subprocess it
    # spawns inherits these vars, so the upgrade pulls from our real local
    # archive instead of GitHub — everything else executes for real.
    BIFROST_DATA_DIR="$TEST_DATA_DIR" \
    BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
    BIFROST_DISABLE_TRAY=1 \
    BIFROST_UPGRADE_TEST_ALLOW_RELEASE_OVERRIDES=1 \
    BIFROST_UPGRADE_TEST_LATEST_VERSION="$TEST_VERSION" \
    BIFROST_UPGRADE_TEST_ARCHIVE="$archive_path" \
    BIFROST_APP_INSTALL_DIR="$TEST_ROOT/no-desktop-app" \
    "$INSTALL_BIN" start -p "$PROXY_PORT" --host 127.0.0.1 --daemon \
        --access-mode allow_all --skip-cert-check --no-system-proxy \
        --no-intercept "${tray_args[@]}" -y >/tmp/bifrost-admin-upgrade-start.log 2>&1

    local old_pid=""
    for _ in $(seq 1 50); do
        old_pid="$(cat "${TEST_DATA_DIR}/bifrost.pid" 2>/dev/null || true)"
        [[ -n "$old_pid" && "$old_pid" =~ ^[0-9]+$ ]] && break
        sleep 0.1
    done
    if [[ -z "$old_pid" || ! "$old_pid" =~ ^[0-9]+$ ]] || ! pid_is_running "$old_pid"; then
        _log_fail "daemon started" "running pid" "$(cat /tmp/bifrost-admin-upgrade-start.log 2>/dev/null)"
        return 1
    fi
    wait_admin_ready || {
        _log_fail "admin server ready" "reachable" "unreachable"
        return 1
    }
    _log_pass "daemon started on port $PROXY_PORT (PID: $old_pid), admin reachable"

    # Sanity: the admin must agree there is an update (gate for POST upgrade).
    local vc has_update
    vc="$(admin_curl GET /api/system/version-check)"
    has_update="$(json_field "$vc" has_update)"
    assert_equals "True" "$has_update" "admin version-check reports an available update" || {
        echo "version-check: $vc"
        return 1
    }

    # Reproduce the Feishu/Codex process role. The nested CLI must not stop the
    # daemon itself; it delegates to Admin, receives 202, and exits while the
    # detached updater continues independently of this caller.
    local worker_output="$TEST_ROOT/worker-upgrade.log"
    local competing_response="$TEST_ROOT/competing-upgrade.json"
    local competing_code_file="$TEST_ROOT/competing-upgrade.code"
    local worker_started_at worker_elapsed
    worker_started_at="$(date +%s)"
    BIFROST_DATA_DIR="$TEST_DATA_DIR" \
    BIFROST_EXTERNAL_CLI_WORKER=1 \
    NO_PROXY='*' no_proxy='*' \
        "$INSTALL_BIN" upgrade >"$worker_output" 2>&1 &
    local worker_pid=$!
    env NO_PROXY='*' no_proxy='*' curl -sS -X POST --connect-timeout 2 --max-time 15 \
        -o "$competing_response" -w '%{http_code}' \
        "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/system/upgrade" >"$competing_code_file" &
    local competing_pid=$!
    wait "$worker_pid" || {
        _log_fail "external worker upgrade delegates successfully" "exit 0" "$(cat "$worker_output")"
        return 1
    }
    wait "$competing_pid" || true
    worker_elapsed=$(( $(date +%s) - worker_started_at ))
    local competing_code
    competing_code="$(cat "$competing_code_file" 2>/dev/null)"
    if grep -q "Upgrade scheduled by the running Bifrost service" "$worker_output"; then
        assert_equals "409" "$competing_code" "competing POST observes delegated upgrade" || return 1
    elif grep -q "An upgrade is already in progress" "$worker_output"; then
        assert_equals "202" "$competing_code" "competing POST owns serialized upgrade" || return 1
    else
        _log_fail "external worker reports delegated upgrade" "scheduled/already-in-progress message" "$(cat "$worker_output")"
        return 1
    fi
    if [[ $worker_elapsed -ge 10 ]]; then
        _log_fail "external worker returns promptly" "< 10 seconds" "${worker_elapsed}s"
        return 1
    fi
    _log_pass "external worker delegated upgrade, competing POST serialized, and caller exited in ${worker_elapsed}s"

    local initial_progress phase source
    initial_progress="$(cat "${TEST_DATA_DIR}/upgrade-progress.json" 2>/dev/null || true)"
    phase="$(json_field "$initial_progress" phase)"
    source="$(json_field "$initial_progress" source)"
    if [[ "$phase" != "checking" && "$phase" != "downloading" && "$phase" != "installing" && "$phase" != "restarting" && "$phase" != "completed" ]]; then
        _log_fail "delegated upgrade writes active or completed progress" "active/completed phase" "$phase ($initial_progress)"
        return 1
    fi
    assert_equals "admin" "$source" "delegated upgrade records source=admin" || return 1

    # Poll progress across the restart window. The admin endpoint goes down while
    # the proxy restarts, then comes back and reads terminal state from the file.
    local final_phase="" saw_disconnect=0
    for _ in $(seq 1 200); do
        local resp p
        resp="$(admin_curl GET /api/system/upgrade/progress)"
        if [[ -z "$resp" ]]; then
            saw_disconnect=1
            sleep 0.3
            continue
        fi
        p="$(json_field "$resp" phase)"
        case "$p" in
            completed) final_phase="completed"; break ;;
            failed)
                _log_fail "upgrade progress completes" "completed" "failed: $(json_field "$resp" error)"
                return 1
                ;;
        esac
        sleep 0.3
    done

    if [[ "$final_phase" != "completed" ]]; then
        _log_fail "upgrade progress reaches completed" "completed" "${final_phase:-timeout}"
        echo "=== start log ==="; cat /tmp/bifrost-admin-upgrade-start.log 2>/dev/null
        echo "=== progress file ==="; cat "${TEST_DATA_DIR}/upgrade-progress.json" 2>/dev/null
        return 1
    fi
    _log_pass "upgrade progress reached phase=completed"

    # The terminal progress record must attribute the upgrade to admin.
    local prog_source
    prog_source="$(json_field "$(cat "${TEST_DATA_DIR}/upgrade-progress.json" 2>/dev/null)" source)"
    assert_equals "admin" "$prog_source" "terminal progress source=admin" || return 1

    local background_log="${TEST_DATA_DIR}/logs/upgrade-background.log"
    if [[ ! -s "$background_log" ]]; then
        _log_fail "background upgrade subprocess writes diagnostics log" "non-empty $background_log" "missing or empty"
        return 1
    fi
    _log_pass "background upgrade subprocess wrote diagnostics log"

    # Wait for the restarted admin server to come back up.
    wait_admin_ready || {
        _log_fail "admin server ready after restart" "reachable" "unreachable"
        return 1
    }

    # Prove a real restart happened: new PID, alive, and using the install path.
    local new_pid
    new_pid="$(read_runtime_field pid)"
    if [[ -z "$new_pid" || ! "$new_pid" =~ ^[0-9]+$ ]]; then
        new_pid="$(cat "${TEST_DATA_DIR}/bifrost.pid" 2>/dev/null || true)"
    fi
    if [[ "$new_pid" == "$old_pid" ]]; then
        _log_fail "upgrade replaced daemon pid" "new PID != $old_pid" "$new_pid"
        return 1
    fi
    if ! pid_is_running "$new_pid"; then
        _log_fail "new daemon alive" "PID $new_pid alive" "not running"
        return 1
    fi
    _log_pass "admin-driven upgrade restarted daemon with new PID: $new_pid (was $old_pid)"

    local new_cmd
    new_cmd="$(ps -p "$new_pid" -o command= 2>/dev/null || true)"
    if [[ "$new_cmd" != "$INSTALL_BIN"* ]]; then
        _log_fail "new daemon uses upgraded install path" "$INSTALL_BIN" "$new_cmd"
        return 1
    fi
    _log_pass "new daemon runs from upgraded install path"

    # Clean shutdown + port release.
    BIFROST_DATA_DIR="$TEST_DATA_DIR" "$INSTALL_BIN" stop >/tmp/bifrost-admin-upgrade-stop.log 2>&1 || {
        _log_fail "upgraded daemon stops cleanly" "stop exits 0" "$(cat /tmp/bifrost-admin-upgrade-stop.log)"
        return 1
    }
    sleep 1
    if admin_curl GET /api/system >/dev/null 2>&1 && [[ -n "$(admin_curl GET /api/system)" ]]; then
        _log_fail "port released after stopping upgraded daemon" "unreachable" "still reachable"
        return 1
    fi
    _log_pass "upgraded daemon stops and releases port"
}

test_background_self_update_restarts_when_disk_binary_already_latest() {
    _log_info "case: background self-update restarts daemon when the installed binary is already latest"

    cleanup

    if [[ ! -x "$BIFROST_BIN" ]]; then
        _log_fail "bifrost binary exists" "$BIFROST_BIN" "missing"
        return 1
    fi

    TEST_ROOT="$(mktemp -d)"
    TEST_DATA_DIR="${TEST_ROOT}/data"
    local install_dir="${TEST_ROOT}/install"
    mkdir -p "$TEST_DATA_DIR" "$install_dir"

    INSTALL_BIN="${install_dir}/bifrost"
    cp "$BIFROST_BIN" "$INSTALL_BIN"
    chmod +x "$INSTALL_BIN" 2>/dev/null || true

    PROXY_PORT="$(allocate_free_port)"
    if [[ -z "$PROXY_PORT" || ! "$PROXY_PORT" =~ ^[0-9]+$ ]]; then
        _log_fail "free proxy port allocated" "numeric port" "${PROXY_PORT:-empty}"
        return 1
    fi

    local -a tray_args=()
    if [[ "$(uname -s)" != "Linux" ]]; then
        tray_args=(--no-tray)
    fi

    BIFROST_DATA_DIR="$TEST_DATA_DIR" \
    BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
    BIFROST_DISABLE_TRAY=1 \
    BIFROST_APP_INSTALL_DIR="$TEST_ROOT/no-desktop-app" \
    "$INSTALL_BIN" start -p "$PROXY_PORT" --host 127.0.0.1 --daemon \
        --access-mode allow_all --skip-cert-check --no-system-proxy \
        --no-intercept "${tray_args[@]}" -y >/tmp/bifrost-admin-upgrade-already-latest-start.log 2>&1

    local old_pid=""
    for _ in $(seq 1 50); do
        old_pid="$(cat "${TEST_DATA_DIR}/bifrost.pid" 2>/dev/null || true)"
        [[ -n "$old_pid" && "$old_pid" =~ ^[0-9]+$ ]] && break
        sleep 0.1
    done
    if [[ -z "$old_pid" || ! "$old_pid" =~ ^[0-9]+$ ]] || ! pid_is_running "$old_pid"; then
        _log_fail "daemon started" "running pid" "$(cat /tmp/bifrost-admin-upgrade-already-latest-start.log 2>/dev/null)"
        return 1
    fi
    wait_admin_ready || {
        _log_fail "admin server ready" "reachable" "unreachable"
        return 1
    }

    local current_version
    current_version="$("$INSTALL_BIN" --version | awk '{print $2}')"
    if [[ -z "$current_version" ]]; then
        _log_fail "current binary version detected" "non-empty version" "empty"
        return 1
    fi

    BIFROST_DATA_DIR="$TEST_DATA_DIR" \
    BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
    BIFROST_DISABLE_TRAY=1 \
    BIFROST_UPGRADE_TEST_ALLOW_RELEASE_OVERRIDES=1 \
    BIFROST_UPGRADE_TEST_LATEST_VERSION="$current_version" \
    BIFROST_APP_INSTALL_DIR="$TEST_ROOT/no-desktop-app" \
    "$INSTALL_BIN" self-update --target "$current_version" --source admin \
        >/tmp/bifrost-admin-upgrade-already-latest-self-update.log 2>&1 || {
        _log_fail "background self-update exits 0" "success" "$(cat /tmp/bifrost-admin-upgrade-already-latest-self-update.log 2>/dev/null)"
        return 1
    }

    wait_admin_ready || {
        _log_fail "admin server ready after already-latest restart" "reachable" "unreachable"
        return 1
    }

    local new_pid
    new_pid="$(read_runtime_field pid)"
    if [[ -z "$new_pid" || ! "$new_pid" =~ ^[0-9]+$ ]]; then
        new_pid="$(cat "${TEST_DATA_DIR}/bifrost.pid" 2>/dev/null || true)"
    fi
    if [[ "$new_pid" == "$old_pid" ]]; then
        _log_fail "already-latest background upgrade restarted daemon" "new PID != $old_pid" "$new_pid"
        echo "=== self-update log ==="; cat /tmp/bifrost-admin-upgrade-already-latest-self-update.log 2>/dev/null
        return 1
    fi
    if ! pid_is_running "$new_pid"; then
        _log_fail "new daemon alive after already-latest restart" "PID $new_pid alive" "not running"
        return 1
    fi
    _log_pass "already-latest background upgrade restarted daemon with new PID: $new_pid (was $old_pid)"

    local progress_phase
    progress_phase="$(json_field "$(cat "${TEST_DATA_DIR}/upgrade-progress.json" 2>/dev/null)" phase)"
    assert_equals "completed" "$progress_phase" "already-latest background upgrade writes completed progress" || return 1

    BIFROST_DATA_DIR="$TEST_DATA_DIR" "$INSTALL_BIN" stop >/tmp/bifrost-admin-upgrade-already-latest-stop.log 2>&1 || {
        _log_fail "already-latest restarted daemon stops cleanly" "stop exits 0" "$(cat /tmp/bifrost-admin-upgrade-already-latest-stop.log)"
        return 1
    }
    _log_pass "already-latest restarted daemon stops cleanly"
}

main() {
    test_admin_api_upgrade_restarts_daemon_with_new_binary || true
    test_background_self_update_restarts_when_disk_binary_already_latest || true
    print_test_summary || exit 1
}

main "$@"
