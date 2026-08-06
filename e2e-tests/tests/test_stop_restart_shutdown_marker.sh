#!/bin/bash
set -euo pipefail

: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
: "${BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY
export BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

source "${PROJECT_DIR}/e2e-tests/test_utils/assert.sh"

BIFROST_BIN="${BIFROST_BIN:-${PROJECT_DIR}/target/debug/bifrost}"
TEST_DATA_DIR=""
PROXY_PORT=""
FAKE_PROXY_BIN=""
FAKE_PROXY_STATE=""
FAKE_PROXY_COMMAND_LOG=""

ensure_pid_stopped() {
    local pid="${1:-}"
    [[ "${pid}" =~ ^[0-9]+$ ]] || return 0
    for _ in $(seq 1 50); do
        if ! kill -0 "${pid}" 2>/dev/null; then
            break
        fi
        sleep 0.1
    done
    if kill -0 "${pid}" 2>/dev/null; then
        kill -TERM "${pid}" 2>/dev/null || true
        for _ in $(seq 1 20); do
            if ! kill -0 "${pid}" 2>/dev/null; then
                break
            fi
            sleep 0.1
        done
        kill -KILL "${pid}" 2>/dev/null || true
    fi

}

if [[ "${BIFROST_COVERAGE_E2E:-0}" == "1" ]]; then
    echo "SKIP: daemon detachment cannot provide a bounded LLVM profile lifecycle"
    exit 0
fi

remove_test_data_dir() {
    local dir="$1"
    [[ -n "${dir}" && -d "${dir}" ]] || return 0

    for _ in $(seq 1 5); do
        rm -rf "${dir}" 2>/dev/null && return 0
        sleep 0.2
    done

    # macOS can briefly report "Directory not empty" while daemon log writers
    # are exiting. Cleanup must not turn a fully-passed regression suite red.
    rm -rf "${dir}" 2>/dev/null || true
}

cleanup() {
    set +e
    if [[ -n "${TEST_DATA_DIR}" && -d "${TEST_DATA_DIR}" ]]; then
        local runtime_pid
        runtime_pid="$(read_runtime_pid)"
        if [[ -n "${FAKE_PROXY_BIN:-}" && -d "${FAKE_PROXY_BIN:-}" ]]; then
            PATH="${FAKE_PROXY_BIN}:$PATH" \
            BIFROST_FAKE_SYSTEM_PROXY_STATE="${FAKE_PROXY_STATE:-}" \
            BIFROST_FAKE_SYSTEM_PROXY_COMMAND_LOG="${FAKE_PROXY_COMMAND_LOG:-}" \
            BIFROST_DATA_DIR="${TEST_DATA_DIR}" "${BIFROST_BIN}" stop >/dev/null 2>&1 || true
        else
            BIFROST_DATA_DIR="${TEST_DATA_DIR}" "${BIFROST_BIN}" stop >/dev/null 2>&1 || true
        fi
        ensure_pid_stopped "${runtime_pid}"
        remove_test_data_dir "${TEST_DATA_DIR}"
    fi
    return 0
}
trap cleanup EXIT

now_ms() {
    python3 - <<'PY'
import time
print(int(time.time() * 1000))
PY
}

free_port() {
    python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

build_bifrost() {
    if [[ "${SKIP_BUILD:-false}" == "true" && -x "${BIFROST_BIN}" ]]; then
        return
    fi
    cargo build --bin bifrost
}

wait_until_ready() {
    local url="http://127.0.0.1:${PROXY_PORT}/_bifrost/api/proxy/address"
    for _ in $(seq 1 50); do
        if curl -fsS "${url}" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.1
    done
    _log_fail "daemon admin API ready" "reachable" "timeout"
    cat "${TEST_DATA_DIR}/start.log" || true
    return 1
}

read_runtime_pid() {
    python3 - "${TEST_DATA_DIR}/runtime.json" <<'PY'
import json
import sys
try:
    with open(sys.argv[1], "r", encoding="utf-8") as f:
        print(json.load(f).get("pid", ""))
except Exception:
    print("")
PY
}

setup_fake_macos_system_proxy() {
    FAKE_PROXY_BIN="${TEST_DATA_DIR}/fake-bin"
    FAKE_PROXY_STATE="${TEST_DATA_DIR}/fake-system-proxy.env"
    FAKE_PROXY_COMMAND_LOG="${TEST_DATA_DIR}/fake-system-proxy-commands.log"
    mkdir -p "${FAKE_PROXY_BIN}"
    cat > "${FAKE_PROXY_STATE}" <<'EOF'
WEB_ENABLED=No
WEB_HOST=
WEB_PORT=0
SECURE_ENABLED=No
SECURE_HOST=
SECURE_PORT=0
BYPASS=
EOF

    cat > "${FAKE_PROXY_BIN}/networksetup" <<'SH'
#!/bin/bash
set -euo pipefail
state_file="${BIFROST_FAKE_SYSTEM_PROXY_STATE:?}"
command_log="${BIFROST_FAKE_SYSTEM_PROXY_COMMAND_LOG:-}"
if [[ -n "$command_log" ]]; then
    printf '%s pid=%s ppid=%s networksetup %s\n' "$(date +%s.%N)" "$$" "$PPID" "$*" >>"$command_log"
fi
read_state() {
    # shellcheck disable=SC1090
    source "$state_file"
}
write_state() {
    cat > "${state_file}.tmp" <<EOF
WEB_ENABLED=${WEB_ENABLED:-No}
WEB_HOST=${WEB_HOST:-}
WEB_PORT=${WEB_PORT:-0}
SECURE_ENABLED=${SECURE_ENABLED:-No}
SECURE_HOST=${SECURE_HOST:-}
SECURE_PORT=${SECURE_PORT:-0}
BYPASS=${BYPASS:-}
EOF
    mv "${state_file}.tmp" "$state_file"
}
state_value() {
    case "${1:-off}" in
        on|On|ON|yes|Yes|YES) printf 'Yes' ;;
        *) printf 'No' ;;
    esac
}
cmd="${1:-}"
case "$cmd" in
    -listallnetworkservices)
        printf '%s\n' "An asterisk (*) denotes that a network service is disabled."
        printf '%s\n' "Wi-Fi"
        ;;
    -getwebproxy)
        read_state
        printf 'Enabled: %s\nServer: %s\nPort: %s\nAuthenticated Proxy Enabled: 0\n' "${WEB_ENABLED:-No}" "${WEB_HOST:-}" "${WEB_PORT:-0}"
        ;;
    -getsecurewebproxy)
        read_state
        printf 'Enabled: %s\nServer: %s\nPort: %s\nAuthenticated Proxy Enabled: 0\n' "${SECURE_ENABLED:-No}" "${SECURE_HOST:-}" "${SECURE_PORT:-0}"
        ;;
    -setwebproxy)
        read_state
        WEB_HOST="${3:?}"
        WEB_PORT="${4:?}"
        write_state
        ;;
    -setwebproxystate)
        read_state
        WEB_ENABLED="$(state_value "${3:-off}")"
        write_state
        ;;
    -setsecurewebproxy)
        read_state
        SECURE_HOST="${3:?}"
        SECURE_PORT="${4:?}"
        write_state
        ;;
    -setsecurewebproxystate)
        read_state
        SECURE_ENABLED="$(state_value "${3:-off}")"
        write_state
        ;;
    -setproxybypassdomains)
        read_state
        shift 2
        BYPASS="$(IFS=,; printf '%s' "$*")"
        write_state
        ;;
    *)
        echo "unsupported fake networksetup args: $*" >&2
        exit 2
        ;;
esac
SH
    chmod +x "${FAKE_PROXY_BIN}/networksetup"

    cat > "${FAKE_PROXY_BIN}/scutil" <<'SH'
#!/bin/bash
set -euo pipefail
if [[ "${1:-}" != "--proxy" ]]; then
    echo "unsupported fake scutil args: $*" >&2
    exit 2
fi
state_file="${BIFROST_FAKE_SYSTEM_PROXY_STATE:?}"
command_log="${BIFROST_FAKE_SYSTEM_PROXY_COMMAND_LOG:-}"
if [[ -n "$command_log" ]]; then
    printf '%s pid=%s ppid=%s scutil %s\n' "$(date +%s.%N)" "$$" "$PPID" "$*" >>"$command_log"
fi
# shellcheck disable=SC1090
source "$state_file"
web_enable=0
secure_enable=0
[[ "${WEB_ENABLED:-No}" == "Yes" ]] && web_enable=1
[[ "${SECURE_ENABLED:-No}" == "Yes" ]] && secure_enable=1
cat <<EOF
<dictionary> {
  HTTPEnable : ${web_enable}
  HTTPProxy : ${WEB_HOST:-}
  HTTPPort : ${WEB_PORT:-0}
  HTTPSEnable : ${secure_enable}
  HTTPSProxy : ${SECURE_HOST:-}
  HTTPSPort : ${SECURE_PORT:-0}
  ExceptionsList : <array> {
EOF
IFS=',' read -r -a bypass_items <<< "${BYPASS:-}"
idx=0
for item in "${bypass_items[@]}"; do
    [[ -n "$item" ]] || continue
    printf '    %s : %s\n' "$idx" "$item"
    idx=$((idx + 1))
done
cat <<'EOF'
  }
}
EOF
SH
    chmod +x "${FAKE_PROXY_BIN}/scutil"
}

fake_proxy_points_to_bifrost() {
    # shellcheck disable=SC1090
    source "${FAKE_PROXY_STATE}"
    [[ "${WEB_ENABLED:-No}" == "Yes" ]] \
        && [[ "${WEB_HOST:-}" == "127.0.0.1" ]] \
        && [[ "${WEB_PORT:-0}" == "${PROXY_PORT}" ]] \
        && [[ "${SECURE_ENABLED:-No}" == "Yes" ]] \
        && [[ "${SECURE_HOST:-}" == "127.0.0.1" ]] \
        && [[ "${SECURE_PORT:-0}" == "${PROXY_PORT}" ]]
}

wait_fake_proxy_points_to_bifrost() {
    for _ in $(seq 1 100); do
        if fake_proxy_points_to_bifrost; then
            return 0
        fi
        sleep 0.1
    done
    return 1
}

test_restart_handoff_with_fake_system_proxy() {
    if [[ "$(uname -s)" != "Darwin" ]]; then
        _log_pass "non-macOS: restart handoff system proxy E2E skipped"
        return
    fi

    if [[ -n "${TEST_DATA_DIR}" && -d "${TEST_DATA_DIR}" ]]; then
        BIFROST_DATA_DIR="${TEST_DATA_DIR}" "${BIFROST_BIN}" stop >/dev/null 2>&1 || true
        remove_test_data_dir "${TEST_DATA_DIR}"
    fi
    TEST_DATA_DIR="$(mktemp -d)"
    PROXY_PORT="$(free_port)"
    export BIFROST_DATA_DIR="${TEST_DATA_DIR}"
    setup_fake_macos_system_proxy

    PATH="${FAKE_PROXY_BIN}:$PATH" \
    BIFROST_FAKE_SYSTEM_PROXY_STATE="${FAKE_PROXY_STATE}" \
    BIFROST_FAKE_SYSTEM_PROXY_COMMAND_LOG="${FAKE_PROXY_COMMAND_LOG}" \
    "${BIFROST_BIN}" start \
        -p "${PROXY_PORT}" \
        -H 127.0.0.1 \
        --daemon \
        --skip-cert-check \
        --system-proxy \
        --proxy-bypass "localhost,127.0.0.1,*.local" \
        --no-intercept \
        >"${TEST_DATA_DIR}/restart-start.log" 2>&1

    wait_until_ready
    local old_pid
    old_pid="$(read_runtime_pid)"
    if [[ -n "${old_pid}" ]] && wait_fake_proxy_points_to_bifrost; then
        _log_pass "restart handoff fixture ready with fake system proxy"
    else
        _log_fail "restart handoff fixture ready with fake system proxy" "runtime pid and fake proxy target" "pid=${old_pid}; state=$(cat "${FAKE_PROXY_STATE}")"
        return
    fi

    local restart_output
    restart_output=$(PATH="${FAKE_PROXY_BIN}:$PATH" \
        BIFROST_FAKE_SYSTEM_PROXY_STATE="${FAKE_PROXY_STATE}" \
        BIFROST_FAKE_SYSTEM_PROXY_COMMAND_LOG="${FAKE_PROXY_COMMAND_LOG}" \
        "${BIFROST_BIN}" restart 2>&1)
    local restart_code=$?
    if [[ "${restart_code}" -ne 0 ]]; then
        _log_fail "bifrost restart exits successfully" "exit 0" "code=${restart_code}; output=${restart_output}"
        return
    fi

    restart_debug() {
        printf 'marker=%s\n' "$(cat "${TEST_DATA_DIR}/.system_proxy_shutdown_mode" 2>/dev/null || echo '<missing>')"
        printf 'state=%s\n' "$(cat "${FAKE_PROXY_STATE}" 2>/dev/null || true)"
        printf 'fake_commands=%s\n' "$(cat "${FAKE_PROXY_COMMAND_LOG}" 2>/dev/null || true)"
        printf 'restart_log=%s\n' "$(tail -n 160 "${TEST_DATA_DIR}/logs/restart.log" 2>/dev/null || true)"
        printf 'daemon_log=%s\n' "$(tail -n 220 "${TEST_DATA_DIR}"/logs/bifrost*.log "${TEST_DATA_DIR}/bifrost.log" 2>/dev/null || true)"
    }

    local new_pid=""
    for _ in $(seq 1 450); do
        if ! fake_proxy_points_to_bifrost; then
            _log_fail "restart handoff keeps fake system proxy continuously enabled" "127.0.0.1:${PROXY_PORT}" "output=${restart_output}; $(restart_debug)"
            return
        fi
        new_pid="$(read_runtime_pid)"
        if [[ -n "${new_pid}" && "${new_pid}" != "${old_pid}" ]] \
            && kill -0 "${new_pid}" 2>/dev/null \
            && curl -fsS "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/proxy/address" >/dev/null 2>&1; then
            break
        fi
        sleep 0.2
    done

    if [[ -z "${new_pid}" || "${new_pid}" == "${old_pid}" ]]; then
        _log_fail "restart handoff starts a fresh daemon" "new runtime pid and ready Admin API" "old_pid=${old_pid}; new_pid=${new_pid}; output=${restart_output}; $(restart_debug)"
        return
    fi

    if grep -q -- "--system-proxy" "${TEST_DATA_DIR}/logs/restart.log" \
        && fake_proxy_points_to_bifrost; then
        _log_pass "bifrost restart preserves system proxy handoff without cleanup gap"
    else
        _log_fail "bifrost restart preserves system proxy handoff without cleanup gap" "restart argv contains --system-proxy and fake proxy still points to Bifrost" "$(restart_debug)"
    fi

    PATH="${FAKE_PROXY_BIN}:$PATH" \
    BIFROST_FAKE_SYSTEM_PROXY_STATE="${FAKE_PROXY_STATE}" \
    BIFROST_FAKE_SYSTEM_PROXY_COMMAND_LOG="${FAKE_PROXY_COMMAND_LOG}" \
    "${BIFROST_BIN}" stop >/dev/null 2>&1 || true
    ensure_pid_stopped "${new_pid}"
}

test_restart_without_system_proxy_cross_platform() {
    if [[ -n "${TEST_DATA_DIR}" && -d "${TEST_DATA_DIR}" ]]; then
        BIFROST_DATA_DIR="${TEST_DATA_DIR}" "${BIFROST_BIN}" stop >/dev/null 2>&1 || true
        remove_test_data_dir "${TEST_DATA_DIR}"
    fi
    TEST_DATA_DIR="$(mktemp -d)"
    PROXY_PORT="$(free_port)"
    export BIFROST_DATA_DIR="${TEST_DATA_DIR}"

    "${BIFROST_BIN}" start \
        -p "${PROXY_PORT}" \
        -H 127.0.0.1 \
        --daemon \
        --skip-cert-check \
        --no-system-proxy \
        --no-intercept \
        >"${TEST_DATA_DIR}/restart-no-proxy-start.log" 2>&1

    wait_until_ready
    local old_pid
    old_pid="$(read_runtime_pid)"
    if [[ -n "${old_pid}" ]]; then
        _log_pass "cross-platform restart fixture ready without system proxy"
    else
        _log_fail "cross-platform restart fixture ready without system proxy" "runtime pid" "pid=${old_pid}"
        return
    fi

    local restart_output
    restart_output="$("${BIFROST_BIN}" restart 2>&1)"
    local restart_code=$?
    if [[ "${restart_code}" -ne 0 ]]; then
        _log_fail "bifrost restart without system proxy exits successfully" "exit 0" "code=${restart_code}; output=${restart_output}"
        return
    fi

    local new_pid=""
    for _ in $(seq 1 250); do
        new_pid="$(read_runtime_pid)"
        if [[ -n "${new_pid}" && "${new_pid}" != "${old_pid}" ]] \
            && kill -0 "${new_pid}" 2>/dev/null \
            && curl -fsS "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/proxy/address" >/dev/null 2>&1; then
            break
        fi
        sleep 0.2
    done

    if [[ -n "${new_pid}" && "${new_pid}" != "${old_pid}" ]]; then
        _log_pass "bifrost restart without system proxy starts a fresh daemon"
    else
        _log_fail "bifrost restart without system proxy starts a fresh daemon" "new runtime pid and ready Admin API" "old_pid=${old_pid}; new_pid=${new_pid}; output=${restart_output}; restart_log=$(tail -n 160 "${TEST_DATA_DIR}/logs/restart.log" 2>/dev/null || true)"
        return
    fi

    if grep -q -- "--system-proxy" "${TEST_DATA_DIR}/logs/restart.log" 2>/dev/null; then
        _log_fail "bifrost restart without system proxy keeps start argv platform-neutral" "no --system-proxy restart arg" "$(tail -n 160 "${TEST_DATA_DIR}/logs/restart.log" 2>/dev/null || true)"
    else
        _log_pass "bifrost restart without system proxy keeps start argv platform-neutral"
    fi

    if [[ ! -e "${TEST_DATA_DIR}/.system_proxy_shutdown_mode" ]]; then
        _log_pass "bifrost restart without system proxy leaves no shutdown marker"
    else
        _log_fail "bifrost restart without system proxy leaves no shutdown marker" "marker absent" "$(cat "${TEST_DATA_DIR}/.system_proxy_shutdown_mode" 2>/dev/null || true)"
    fi

    "${BIFROST_BIN}" stop >/dev/null 2>&1 || true
    ensure_pid_stopped "${new_pid}"
}

main() {
    build_bifrost

    TEST_DATA_DIR="$(mktemp -d)"
    PROXY_PORT="$(free_port)"
    export BIFROST_DATA_DIR="${TEST_DATA_DIR}"
    printf 'preserve_for_restart' >"${TEST_DATA_DIR}/.system_proxy_shutdown_mode"

    "${BIFROST_BIN}" start \
        -p "${PROXY_PORT}" \
        -H 127.0.0.1 \
        --daemon \
        --skip-cert-check \
        --no-system-proxy \
        --no-intercept \
        >"${TEST_DATA_DIR}/start.log" 2>&1

    wait_until_ready
    _log_pass "daemon ready before stop"
    if [[ ! -e "${TEST_DATA_DIR}/.system_proxy_shutdown_mode" ]]; then
        _log_pass "startup should clear stale restart shutdown marker"
    else
        _log_fail "startup should clear stale restart shutdown marker" "marker removed" "marker still exists"
    fi

    local started stopped elapsed stop_output stopped_pid
    stopped_pid="$(read_runtime_pid)"
    started="$(now_ms)"
    stop_output="$("${BIFROST_BIN}" stop 2>&1)"
    ensure_pid_stopped "${stopped_pid}"
    stopped="$(now_ms)"
    elapsed=$((stopped - started))
    printf '%s\n' "${stop_output}"

    assert_body_contains "Bifrost proxy stopped" "${stop_output}" "stop should report stopped"
    assert_body_not_contains "Sending SIGKILL" "${stop_output}" "stop should not escalate to SIGKILL"
    assert_body_contains "Cleaning managed proxy settings before stopping Bifrost proxy..." "${stop_output}" "stop should clean system and CLI proxy settings before SIGTERM"
    assert_body_not_contains "System proxy cleanup continues in background if needed." "${stop_output}" "stop should not defer system proxy cleanup to background"

    if [[ "${elapsed}" -lt 5000 ]]; then
        _log_pass "stop should return quickly (${elapsed}ms)"
    else
        _log_fail "stop should return quickly" "< 5000ms" "${elapsed}ms"
    fi

    local status_output
    status_output="$("${BIFROST_BIN}" -p "${PROXY_PORT}" status 2>&1)"
    assert_body_contains "Status: Stopped" "${status_output}" "status should report stopped after stop"

    test_restart_without_system_proxy_cross_platform
    test_restart_handoff_with_fake_system_proxy

    print_test_summary || exit 1
}

main "$@"
