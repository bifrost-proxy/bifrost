#!/bin/bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_DISABLE_TRAY
: "${BIFROST_DAEMON_READY_TIMEOUT_SECS:=90}"
export BIFROST_DAEMON_READY_TIMEOUT_SECS

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

source "${PROJECT_DIR}/e2e-tests/test_utils/assert.sh"
source "${PROJECT_DIR}/e2e-tests/test_utils/process.sh"

BIFROST_BIN="${BIFROST_BIN:-${PROJECT_DIR}/target/debug/bifrost}"
OLD_BIFROST_BIN="${OLD_BIFROST_BIN:-}"
BIFROST_UPGRADE_E2E_START_WITH_INSTALL_BIN="${BIFROST_UPGRADE_E2E_START_WITH_INSTALL_BIN:-0}"
# Derive the upgrade target version from the workspace version (patch + 1) so the
# self-upgrade actually sees a newer release. A hardcoded value silently breaks
# whenever the workspace version is bumped to match it (see the 0.0.101 collision).
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

kill_windows_bifrost_for_install_bin() {
    if ! is_windows || [[ -z "$INSTALL_BIN" ]]; then
        return 0
    fi

    local install_bin_win
    install_bin_win="$(windows_path "$INSTALL_BIN")"
    BIFROST_E2E_INSTALL_BIN_WIN="$install_bin_win" \
    BIFROST_E2E_INSTALL_BIN_MSYS="$INSTALL_BIN" \
    powershell.exe -NoProfile -ExecutionPolicy Bypass -Command '
        $patterns = @($env:BIFROST_E2E_INSTALL_BIN_WIN, $env:BIFROST_E2E_INSTALL_BIN_MSYS) |
            Where-Object { $_ -and $_.Trim().Length -gt 0 }
        Get-CimInstance Win32_Process |
            Where-Object {
                $process = $_
                $cmd = $process.CommandLine
                $exe = $process.ExecutablePath
                $process.Name -ieq "bifrost.exe" -and (
                    ($exe -and $patterns -contains $exe) -or
                    ($cmd -and (($patterns | Where-Object { $_ -and $cmd.Contains($_) }).Count -gt 0))
                )
            } |
            ForEach-Object {
                try { Stop-Process -Id $_.ProcessId -Force -ErrorAction Stop } catch {}
            }
    ' >/dev/null 2>&1 || true
}

remove_test_root() {
    if [[ -z "$TEST_ROOT" || ! -d "$TEST_ROOT" ]]; then
        return 0
    fi

    local attempt
    for attempt in $(seq 1 8); do
        rm -rf "$TEST_ROOT" >/dev/null 2>&1 && return 0
        kill_windows_bifrost_for_install_bin
        sleep 0.5
    done
    rm -rf "$TEST_ROOT" >/dev/null 2>&1 || true
}

cleanup() {
    if [[ -n "$TEST_DATA_DIR" && -x "$INSTALL_BIN" ]]; then
        BIFROST_DATA_DIR="$(bifrost_process_path "$TEST_DATA_DIR")" "$INSTALL_BIN" stop >/dev/null 2>&1 || true
    fi
    if [[ -n "$PROXY_PORT" ]]; then
        kill_bifrost_on_port "$PROXY_PORT"
    fi
    if [[ -n "$TEST_DATA_DIR" ]]; then
        ps -axo pid,command 2>/dev/null \
            | grep -F 'bifrost __tray' \
            | grep -F -- "$TEST_DATA_DIR" \
            | awk '{print $1}' \
            | while read -r pid; do
                [[ -n "$pid" ]] && kill "$pid" >/dev/null 2>&1 || true
            done
    fi
    kill_windows_bifrost_for_install_bin
    remove_test_root
}
trap cleanup EXIT

host_triple() {
    rustc -vV | awk '/^host:/ {print $2}'
}

binary_name() {
    if is_windows; then
        echo "bifrost.exe"
    else
        echo "bifrost"
    fi
}

windows_path() {
    if command -v cygpath >/dev/null 2>&1; then
        cygpath -w "$1"
    else
        echo "$1"
    fi
}

bifrost_process_path() {
    if is_windows; then
        windows_path "$1"
    else
        echo "$1"
    fi
}

pid_is_running() {
    local pid="$1"
    if is_windows; then
        powershell.exe -NoProfile -ExecutionPolicy Bypass -Command \
            "try { Get-Process -Id ${pid} -ErrorAction Stop | Out-Null; exit 0 } catch { exit 1 }" >/dev/null 2>&1
    else
        kill -0 "$pid" 2>/dev/null
    fi
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

    if is_windows; then
        powershell.exe -NoProfile -ExecutionPolicy Bypass -Command \
            '$listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Parse("127.0.0.1"), 0); $listener.Start(); $port = $listener.LocalEndpoint.Port; $listener.Stop(); Write-Output $port'
        return 0
    fi

    return 1
}

assert_upgrade_log_contains() {
    local upgrade_log="$1"
    local pattern="$2"
    local description="$3"

    if grep -Eq "$pattern" "$upgrade_log"; then
        _log_pass "$description"
        return 0
    fi

    _log_fail "$description" "$pattern" "$(cat "$upgrade_log")"
    return 1
}

find_old_bifrost_bin() {
    if [[ -n "$OLD_BIFROST_BIN" && -x "$OLD_BIFROST_BIN" ]]; then
        echo "$OLD_BIFROST_BIN"
        return 0
    fi

    local workspace_version
    workspace_version="$(grep -m1 '^version' "${PROJECT_DIR}/Cargo.toml" 2>/dev/null \
        | sed -E 's/.*"([0-9]+\.[0-9]+\.[0-9]+)".*/\1/')"
    for candidate in /Users/eden/.cargo/bin/bifrost "$(command -v bifrost 2>/dev/null || true)"; do
        if [[ -z "$candidate" || ! -x "$candidate" ]]; then
            continue
        fi
        local candidate_version
        candidate_version="$("$candidate" --version 2>/dev/null \
            | sed -nE 's/^bifrost ([0-9]+\.[0-9]+\.[0-9]+).*/\1/p' \
            | head -n1)"
        if [[ -z "$candidate_version" || -z "$workspace_version" ]]; then
            continue
        fi
        if [[ "$candidate_version" != "$workspace_version" ]] \
            && [[ "$(printf '%s\n%s\n' "$candidate_version" "$workspace_version" | sort -V | head -n1)" == "$candidate_version" ]]; then
            echo "$candidate"
            return 0
        fi
    done

    return 1
}

wait_proxy_ready() {
    local port="$1"
    local waited=0
    while [[ "$waited" -lt 100 ]]; do
        if curl -fsS "http://127.0.0.1:${port}/_bifrost/api/proxy/address" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.2
        waited=$((waited + 1))
    done
    return 1
}

assert_no_tray_helper_for_test_data_dir() {
    if is_windows; then
        return 0
    fi

    local matches
    matches="$(ps -axo command 2>/dev/null | grep -F 'bifrost __tray' | grep -F -- "$TEST_DATA_DIR" || true)"
    if [[ -n "$matches" ]]; then
        _log_fail "upgrade restart cleaned tray helper" "no __tray helper for $TEST_DATA_DIR" "$matches"
        return 1
    fi
    _log_pass "upgrade restart leaves no tray helper for test data dir"
}

assert_post_upgrade_skills_installed() {
    local test_home="$1"
    local primary_skill="${test_home}/.codex/skills/bifrost/SKILL.md"
    local remote_skill="${test_home}/.codex/skills/bifrost-remote/SKILL.md"

    if [[ ! -s "$primary_skill" ]]; then
        _log_fail "upgrade auto-installs primary Bifrost skill" "$primary_skill exists" "missing"
        return 1
    fi
    if [[ ! -s "$remote_skill" ]]; then
        _log_fail "upgrade auto-installs remote Bifrost skill" "$remote_skill exists" "missing"
        return 1
    fi
    if ! grep -Eq '^name: "?bifrost"?$' "$primary_skill"; then
        _log_fail "primary Bifrost skill has frontmatter" 'name: bifrost or name: "bifrost"' "$(head -n 5 "$primary_skill")"
        return 1
    fi
    if ! grep -Eq '^name: "?bifrost-remote"?$' "$remote_skill"; then
        _log_fail "remote Bifrost skill has frontmatter" 'name: bifrost-remote or name: "bifrost-remote"' "$(head -n 5 "$remote_skill")"
        return 1
    fi

    _log_pass "upgrade auto-installs Bifrost skills into isolated HOME"
}

create_local_release_archive() {
    local archive_root="$1"
    local version="$2"
    local target="$3"
    local binary="$4"
    local package_dir="${archive_root}/bifrost-v${version}-${target}"
    local bin_name
    bin_name="$(binary_name)"

    mkdir -p "$package_dir"
    cp "$binary" "${package_dir}/${bin_name}"
    chmod +x "${package_dir}/${bin_name}" 2>/dev/null || true

    if is_windows; then
        local archive_path="${archive_root}/bifrost-v${version}-${target}.zip"
        local package_dir_win archive_path_win
        package_dir_win="$(windows_path "$package_dir")"
        archive_path_win="$(windows_path "$archive_path")"
        powershell.exe -NoProfile -ExecutionPolicy Bypass -Command \
            "Compress-Archive -Path '${package_dir_win}' -DestinationPath '${archive_path_win}' -Force" >/dev/null
        echo "$archive_path"
        return 0
    fi

    local archive_path="${archive_root}/bifrost-v${version}-${target}.tar.xz"
    tar -C "$archive_root" -cJf "$archive_path" "bifrost-v${version}-${target}"
    echo "$archive_path"
}

test_local_upgrade_restarts_old_daemon_with_new_binary() {
    _log_info "case: local archive upgrade restarts a running daemon"

    if [[ ! -x "$BIFROST_BIN" ]]; then
        _log_fail "new bifrost binary exists" "$BIFROST_BIN" "missing"
        return 1
    fi

    TEST_ROOT="$(mktemp -d)"
    TEST_DATA_DIR="${TEST_ROOT}/data"
    local test_home="${TEST_ROOT}/home"
    local test_home_for_process
    local install_dir="${TEST_ROOT}/install"
    local archive_root="${TEST_ROOT}/archive"
    mkdir -p "$TEST_DATA_DIR" "$test_home" "$install_dir" "$archive_root"
    test_home_for_process="$(bifrost_process_path "$test_home")"
    INSTALL_BIN="${install_dir}/$(binary_name)"
    cp "$BIFROST_BIN" "$INSTALL_BIN"
    chmod +x "$INSTALL_BIN" 2>/dev/null || true

    local start_bin
    if [[ "$BIFROST_UPGRADE_E2E_START_WITH_INSTALL_BIN" == "1" ]]; then
        start_bin="$INSTALL_BIN"
        _log_pass "start binary selected: installed test binary $start_bin"
    else
        if ! start_bin="$(find_old_bifrost_bin)"; then
        _log_fail "old bifrost binary available" "older bifrost binary via OLD_BIFROST_BIN or ~/.cargo/bin/bifrost" "not found"
        return 1
    fi
        _log_pass "old binary selected: $start_bin"
    fi

    local py
    py="$(python3_cmd 2>/dev/null || true)"
    PROXY_PORT="$(allocate_free_port)"
    if [[ -z "$PROXY_PORT" || ! "$PROXY_PORT" =~ ^[0-9]+$ ]]; then
        _log_fail "free proxy port allocated" "numeric port" "$PROXY_PORT"
        return 1
    fi

    local target archive_path
    target="$(host_triple)"
    archive_path="$(create_local_release_archive "$archive_root" "$TEST_VERSION" "$target" "$BIFROST_BIN")"

    local bifrost_data_dir
    bifrost_data_dir="$(bifrost_process_path "$TEST_DATA_DIR")"
    BIFROST_DATA_DIR="$bifrost_data_dir" \
    BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
    "$start_bin" start -p "$PROXY_PORT" --host 127.0.0.1 --daemon \
        --skip-cert-check --no-system-proxy --no-intercept --no-tray -y >/tmp/bifrost-upgrade-old-start.log 2>&1
    local old_pid
    old_pid=""
    for _ in $(seq 1 50); do
        old_pid="$(cat "${TEST_DATA_DIR}/bifrost.pid" 2>/dev/null || true)"
        if [[ -n "$old_pid" && "$old_pid" =~ ^[0-9]+$ ]]; then
            break
        fi
        sleep 0.1
    done
    if [[ -z "$old_pid" || ! "$old_pid" =~ ^[0-9]+$ ]]; then
        old_pid="$(sed -n 's/.*Daemon started with PID: \([0-9][0-9]*\).*/\1/p' /tmp/bifrost-upgrade-old-start.log | tail -n 1)"
    fi
    if [[ -z "$old_pid" || ! "$old_pid" =~ ^[0-9]+$ ]] || ! pid_is_running "$old_pid"; then
        _log_fail "old daemon started" "running pid" "$(cat /tmp/bifrost-upgrade-old-start.log 2>/dev/null)"
        return 1
    fi
    wait_proxy_ready "$PROXY_PORT" || {
        _log_fail "old daemon admin ready" "reachable" "unreachable"
        return 1
    }
    _log_pass "old daemon started on port $PROXY_PORT (PID: $old_pid)"

    local runtime_port=""
    for _ in $(seq 1 50); do
        if [[ -z "$py" ]]; then
            runtime_port="$(powershell.exe -NoProfile -ExecutionPolicy Bypass -Command \
                "try { (Get-Content -Raw '$(windows_path "$TEST_DATA_DIR/runtime.json")' | ConvertFrom-Json).port } catch { '' }")"
        else
            runtime_port="$("$py" - "$TEST_DATA_DIR/runtime.json" <<'PY'
import json
import sys
try:
    with open(sys.argv[1], encoding="utf-8") as fh:
        print(json.load(fh).get("port", ""))
except Exception:
    print("")
PY
)"
        fi
        if [[ "$runtime_port" == "$PROXY_PORT" ]]; then
            break
        fi
        sleep 0.1
    done
    if [[ "$runtime_port" != "$PROXY_PORT" ]]; then
        _log_fail "old daemon runtime records proxy port" "$PROXY_PORT" "${runtime_port:-missing}"
        return 1
    fi
    _log_pass "old daemon runtime recorded port $PROXY_PORT"

    local upgrade_log="${TEST_ROOT}/upgrade.log"
    BIFROST_DATA_DIR="$bifrost_data_dir" \
    HOME="$test_home_for_process" \
    USERPROFILE="$test_home_for_process" \
    BIFROST_INSTALL_SKILL_SOURCE=embedded \
    BIFROST_INSTALL_SKILL_DIR="$(bifrost_process_path "${test_home}/.codex/skills/bifrost")" \
    BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
    BIFROST_UPGRADE_TEST_LATEST_VERSION="$TEST_VERSION" \
    BIFROST_UPGRADE_TEST_ARCHIVE="$(bifrost_process_path "$archive_path")" \
    "$INSTALL_BIN" upgrade >"$upgrade_log" 2>&1
    local upgrade_status=$?
    if [[ $upgrade_status -ne 0 ]]; then
        _log_fail "upgrade command exits successfully" "0" "$(cat "$upgrade_log")"
        return 1
    fi

    if ! is_windows; then
        assert_upgrade_log_contains \
            "$upgrade_log" \
            'Installing latest Bifrost skills' \
            "upgrade output installs Bifrost skills" || return 1
    fi
    assert_upgrade_log_contains \
        "$upgrade_log" \
        'Detected running Bifrost proxy' \
        "upgrade output detects running proxy" || return 1
    if ! is_windows; then
        assert_post_upgrade_skills_installed "$test_home" || return 1
    fi
    assert_upgrade_log_contains \
        "$upgrade_log" \
        'Stopping current proxy' \
        "upgrade output stops current proxy" || return 1
    assert_upgrade_log_contains \
        "$upgrade_log" \
        'Waiting for proxy port [0-9]+ to be released before restart' \
        "upgrade output waits for proxy port release" || return 1
    if is_windows; then
        assert_upgrade_log_contains \
            "$upgrade_log" \
            'Scheduling proxy restart with:|Proxy restart scheduled with the new version' \
            "upgrade output schedules Windows restart" || return 1
    else
        assert_upgrade_log_contains \
            "$upgrade_log" \
            'Starting proxy with:|Proxy restarted successfully with the new version' \
            "upgrade output starts replacement proxy" || return 1
    fi

    grep -q -- '--no-system-proxy' "$upgrade_log" \
        && ! grep -q 'System proxy: enabled' "$upgrade_log"
    assert_equals "0" "$?" "upgrade restart preserves no-system-proxy mode" || {
        cat "$upgrade_log"
        return 1
    }

    wait_proxy_ready "$PROXY_PORT" || {
        _log_fail "new daemon admin ready after upgrade" "reachable" "unreachable"
        return 1
    }
    if is_windows; then
        # Windows self-replacement runs in a deferred helper after the upgrade
        # process exits. The helper installs skills before starting the new
        # daemon, so Admin readiness is the synchronization point.
        assert_post_upgrade_skills_installed "$test_home" || return 1
    fi

    local runtime_json_path
    runtime_json_path="${TEST_DATA_DIR}/runtime.json"

    local new_pid
    if [[ -z "$py" ]]; then
        new_pid="$(powershell.exe -NoProfile -ExecutionPolicy Bypass -Command \
            "try { (Get-Content -Raw '$(windows_path "$runtime_json_path")' | ConvertFrom-Json).pid } catch { '' }")"
    else
        new_pid="$("$py" - "$runtime_json_path" <<'PY'
import json, sys
print(json.load(open(sys.argv[1]))["pid"])
PY
)"
    fi
    if [[ "$new_pid" == "$old_pid" && ! is_windows ]]; then
        _log_fail "upgrade replaced daemon pid" "new PID different from $old_pid" "$new_pid"
        return 1
    fi
    if ! pid_is_running "$new_pid"; then
        _log_fail "new daemon process alive" "PID $new_pid alive" "not running"
        return 1
    fi
    if [[ "$new_pid" == "$old_pid" ]]; then
        _log_info "Windows restart reported the same PID after replacement; continuing with behavior checks"
    else
        _log_pass "upgrade restarted daemon with new PID: $new_pid"
    fi

    local runtime_system_proxy_enabled
    if [[ -z "$py" ]]; then
        runtime_system_proxy_enabled="$(powershell.exe -NoProfile -ExecutionPolicy Bypass -Command \
            "try { (Get-Content -Raw '$(windows_path "$runtime_json_path")' | ConvertFrom-Json).system_proxy_enabled } catch { '' }")"
    else
        runtime_system_proxy_enabled="$("$py" - "$runtime_json_path" <<'PY'
import json, sys
print(json.load(open(sys.argv[1])).get("system_proxy_enabled"))
PY
)"
    fi
    assert_equals "False" "$runtime_system_proxy_enabled" "new daemon runtime records no-system-proxy mode" || return 1

    local new_cmd
    if ! is_windows; then
        new_cmd="$(ps -p "$new_pid" -o command= 2>/dev/null || true)"
        if [[ "$new_cmd" != "$INSTALL_BIN"* ]]; then
            _log_fail "new daemon uses upgraded install path" "$INSTALL_BIN" "$new_cmd"
            return 1
        fi
    fi
    _log_pass "new daemon command uses upgraded install path"

    local err_log="${TEST_DATA_DIR}/logs/bifrost.err"
    if [[ -f "$err_log" ]] && grep -E 'objc_initializeAfterForkError|\\+\\[NSNumber initialize\\]' "$err_log"; then
        _log_fail "new daemon has no ObjC fork crash" "no objc fork-safety crash" "$(cat "$err_log")"
        return 1
    fi
    _log_pass "new daemon error log has no ObjC fork crash"

    BIFROST_DATA_DIR="$bifrost_data_dir" "$INSTALL_BIN" stop >/tmp/bifrost-upgrade-new-stop.log 2>&1 || {
        _log_fail "new daemon stops cleanly" "stop exits 0" "$(cat /tmp/bifrost-upgrade-new-stop.log)"
        return 1
    }
    sleep 1
    assert_no_tray_helper_for_test_data_dir || return 1
    if curl -fsS "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/proxy/address" >/dev/null 2>&1; then
        _log_fail "port released after stopping upgraded daemon" "unreachable" "still reachable"
        return 1
    fi
    _log_pass "upgraded daemon stops and releases port"
}

main() {
    test_local_upgrade_restarts_old_daemon_with_new_binary || true
    print_test_summary || exit 1
}

main "$@"
