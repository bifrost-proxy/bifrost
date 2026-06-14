#!/bin/bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

source "${PROJECT_DIR}/e2e-tests/test_utils/assert.sh"
source "${PROJECT_DIR}/e2e-tests/test_utils/process.sh"

BIFROST_BIN="${BIFROST_BIN:-${PROJECT_DIR}/target/debug/bifrost}"
OLD_BIFROST_BIN="${OLD_BIFROST_BIN:-}"
BIFROST_UPGRADE_E2E_START_WITH_INSTALL_BIN="${BIFROST_UPGRADE_E2E_START_WITH_INSTALL_BIN:-0}"
TEST_VERSION="${BIFROST_UPGRADE_E2E_VERSION:-0.0.101}"
TEST_ROOT=""
TEST_DATA_DIR=""
INSTALL_BIN=""
PROXY_PORT=""

cleanup() {
    if [[ -n "$TEST_DATA_DIR" && -x "$INSTALL_BIN" ]]; then
        BIFROST_DATA_DIR="$TEST_DATA_DIR" "$INSTALL_BIN" stop >/dev/null 2>&1 || true
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
    if [[ -n "$TEST_ROOT" && -d "$TEST_ROOT" ]]; then
        rm -rf "$TEST_ROOT"
    fi
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

find_old_bifrost_bin() {
    if [[ -n "$OLD_BIFROST_BIN" && -x "$OLD_BIFROST_BIN" ]]; then
        echo "$OLD_BIFROST_BIN"
        return 0
    fi

    for candidate in /Users/eden/.cargo/bin/bifrost "$(command -v bifrost 2>/dev/null || true)"; do
        if [[ -n "$candidate" && -x "$candidate" ]] \
            && "$candidate" --version 2>/dev/null | grep -q '0\.0\.99'; then
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
    local install_dir="${TEST_ROOT}/install"
    local archive_root="${TEST_ROOT}/archive"
    mkdir -p "$TEST_DATA_DIR" "$install_dir" "$archive_root"
    INSTALL_BIN="${install_dir}/$(binary_name)"
    cp "$BIFROST_BIN" "$INSTALL_BIN"
    chmod +x "$INSTALL_BIN" 2>/dev/null || true

    local start_bin
    if [[ "$BIFROST_UPGRADE_E2E_START_WITH_INSTALL_BIN" == "1" ]]; then
        start_bin="$INSTALL_BIN"
        _log_pass "start binary selected: installed test binary $start_bin"
    else
        if ! start_bin="$(find_old_bifrost_bin)"; then
            _log_fail "old bifrost binary available" "0.0.99 binary via OLD_BIFROST_BIN or ~/.cargo/bin/bifrost" "not found"
            return 1
        fi
        _log_pass "old binary selected: $start_bin"
    fi

    local py
    py="$(python3_cmd)"
    PROXY_PORT="$("$py" - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
)"

    local target archive_path
    target="$(host_triple)"
    archive_path="$(create_local_release_archive "$archive_root" "$TEST_VERSION" "$target" "$BIFROST_BIN")"

    BIFROST_DATA_DIR="$TEST_DATA_DIR" \
    BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
    "$start_bin" start -p "$PROXY_PORT" --host 127.0.0.1 --daemon \
        --skip-cert-check --no-system-proxy --no-intercept --no-tray -y >/tmp/bifrost-upgrade-old-start.log 2>&1
    local old_pid
    old_pid="$(cat "${TEST_DATA_DIR}/bifrost.pid" 2>/dev/null || true)"
    if [[ -z "$old_pid" || ! "$old_pid" =~ ^[0-9]+$ ]] || ! kill -0 "$old_pid" 2>/dev/null; then
        _log_fail "old daemon started" "running pid" "$(cat /tmp/bifrost-upgrade-old-start.log 2>/dev/null)"
        return 1
    fi
    wait_proxy_ready "$PROXY_PORT" || {
        _log_fail "old daemon admin ready" "reachable" "unreachable"
        return 1
    }
    _log_pass "old daemon started on port $PROXY_PORT (PID: $old_pid)"

    local upgrade_log="${TEST_ROOT}/upgrade.log"
    BIFROST_DATA_DIR="$TEST_DATA_DIR" \
    BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
    BIFROST_UPGRADE_TEST_LATEST_VERSION="$TEST_VERSION" \
    BIFROST_UPGRADE_TEST_ARCHIVE="$archive_path" \
    "$INSTALL_BIN" upgrade -y --restart >"$upgrade_log" 2>&1
    local upgrade_status=$?
    if [[ $upgrade_status -ne 0 ]]; then
        _log_fail "upgrade command exits successfully" "0" "$(cat "$upgrade_log")"
        return 1
    fi

    grep -q 'Detected running Bifrost proxy' "$upgrade_log" \
        && grep -q 'Stopping current proxy' "$upgrade_log" \
        && grep -q "Waiting for proxy port ${PROXY_PORT} to be released" "$upgrade_log" \
        && grep -Eq 'Proxy restarted successfully with the new version|Proxy restart scheduled with the new version' "$upgrade_log"
    assert_equals "0" "$?" "upgrade output contains stop/wait/restart milestones" || return 1

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

    local new_pid
    new_pid="$(python3 - "$TEST_DATA_DIR/runtime.json" <<'PY'
import json, sys
print(json.load(open(sys.argv[1]))["pid"])
PY
)"
    if [[ "$new_pid" == "$old_pid" ]]; then
        _log_fail "upgrade replaced daemon pid" "new PID different from $old_pid" "$new_pid"
        return 1
    fi
    if ! kill -0 "$new_pid" 2>/dev/null; then
        _log_fail "new daemon process alive" "PID $new_pid alive" "not running"
        return 1
    fi
    _log_pass "upgrade restarted daemon with new PID: $new_pid"

    local runtime_system_proxy_enabled
    runtime_system_proxy_enabled="$(python3 - "$TEST_DATA_DIR/runtime.json" <<'PY'
import json, sys
print(json.load(open(sys.argv[1])).get("system_proxy_enabled"))
PY
)"
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

    BIFROST_DATA_DIR="$TEST_DATA_DIR" "$INSTALL_BIN" stop >/tmp/bifrost-upgrade-new-stop.log 2>&1 || {
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
