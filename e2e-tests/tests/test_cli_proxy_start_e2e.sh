#!/bin/bash
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

source "${PROJECT_DIR}/e2e-tests/test_utils/assert.sh"
source "${PROJECT_DIR}/e2e-tests/test_utils/process.sh"

PROXY_PORT="${PROXY_PORT:-18889}"
BIFROST_BIN="${PROJECT_DIR}/target/release/bifrost"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi
TEST_DATA_DIR=""
TEST_HOME=""
PROXY_PID=""
PLATFORM="$(uname -s)"
ADMIN_API_BASE="http://127.0.0.1:${PROXY_PORT}/_bifrost/api/proxy/cli"

cleanup() {
    if is_windows; then kill_bifrost_on_port "$PROXY_PORT"; fi
    safe_cleanup_proxy "$PROXY_PID"
    if [[ -n "$TEST_DATA_DIR" ]] && [[ -d "$TEST_DATA_DIR" ]]; then
        rm -rf "$TEST_DATA_DIR"
    fi
    if [[ -n "$TEST_HOME" ]] && [[ -d "$TEST_HOME" ]]; then
        rm -rf "$TEST_HOME"
    fi
}
trap cleanup EXIT

build_bifrost() {
    if [[ -f "$BIFROST_BIN" ]] && [[ "${SKIP_BUILD:-false}" == "true" ]]; then
        return 0
    fi
    return 0
}

start_proxy() {
    export BIFROST_DATA_DIR="${TEST_DATA_DIR}"
    "$BIFROST_BIN" -p "${PROXY_PORT}" start \
        --skip-cert-check --unsafe-ssl \
        --cli-proxy \
        --cli-proxy-no-proxy "localhost,127.0.0.1,::1,*.local" \
        > "${TEST_DATA_DIR}/proxy.log" 2>&1 &
    PROXY_PID=$!
    local wait_secs=2
    if is_windows; then wait_secs=8; fi
    sleep "$wait_secs"
    if ! kill -0 "$PROXY_PID" 2>/dev/null; then
        _log_fail "proxy started" "running process" "not running"
        cat "${TEST_DATA_DIR}/proxy.log" || true
        return 1
    fi
}

stop_proxy() {
    export BIFROST_DATA_DIR="${TEST_DATA_DIR}"
    "$BIFROST_BIN" stop >/dev/null 2>&1 || true
    sleep 1
}

assert_marker_present() {
    local file="$1"
    local waited=0
    while [[ ! -f "$file" && "$waited" -lt 10 ]]; do
        sleep 0.2
        waited=$((waited + 1))
    done
    if [[ ! -f "$file" ]]; then
        _log_fail "marker file exists" "$file" "(not found)"
        cat "${TEST_DATA_DIR}/proxy.log" || true
        return 1
    fi
    local content
    content="$(cat "$file")"
    assert_body_contains "# >>> Bifrost proxy start >>>" "$content" "marker begin exists"
    assert_body_contains "# <<< Bifrost proxy end <<<" "$content" "marker end exists"
}

assert_marker_removed() {
    local file="$1"
    if [[ ! -f "$file" ]]; then
        _log_pass "marker removed (file deleted)"
        return 0
    fi
    local content
    content="$(cat "$file")"
    if [[ "$content" == *"# >>> Bifrost proxy start >>>"* ]] || [[ "$content" == *"# <<< Bifrost proxy end <<<"* ]]; then
        _log_fail "marker removed" "markers absent" "markers still present"
    else
        _log_pass "marker removed"
    fi
}

assert_windows_cli_proxy_status() {
    local response
    response="$(curl -fsS "$ADMIN_API_BASE")" || {
        _log_fail "cli proxy status endpoint" "reachable" "unreachable"
        return 1
    }

    local shell config_count enabled proxy_url
    shell="$(printf '%s' "$response" | jq -r '.shell')"
    config_count="$(printf '%s' "$response" | jq -r '.config_files | length')"
    enabled="$(printf '%s' "$response" | jq -r '.enabled')"
    proxy_url="$(printf '%s' "$response" | jq -r '.proxy_url')"

    if [[ "$shell" != "cmd" && "$shell" != "powershell" ]]; then
        _log_fail "windows shell detection" "cmd or powershell" "$shell"
        return 1
    fi

    assert_equals "0" "$config_count" "windows cli proxy should not write shell config files" || return 1
    assert_equals "false" "$enabled" "windows cli proxy persistent config should remain disabled" || return 1
    assert_equals "http://127.0.0.1:${PROXY_PORT}" "$proxy_url" "windows cli proxy status should expose proxy url" || return 1
}

main() {
    build_bifrost || { echo "编译失败"; exit 1; }

    TEST_DATA_DIR="$(mktemp -d)"
    TEST_HOME="$(mktemp -d)"
    export HOME="$TEST_HOME"
    if [[ "$PLATFORM" == MINGW* || "$PLATFORM" == MSYS* || "$PLATFORM" == CYGWIN* ]]; then
        unset SHELL
    else
        export SHELL="/bin/zsh"
    fi

    start_proxy
    if [[ "${PROXY_PID}" == "" ]]; then
        print_test_summary || exit 1
        return 1
    fi

    if [[ "$PLATFORM" == MINGW* || "$PLATFORM" == MSYS* || "$PLATFORM" == CYGWIN* ]]; then
        assert_windows_cli_proxy_status || { print_test_summary || exit 1; return 1; }
    else
        assert_marker_present "${TEST_HOME}/.zshrc"
        assert_marker_present "${TEST_HOME}/.zprofile"
    fi

    stop_proxy

    if [[ "$PLATFORM" != MINGW* && "$PLATFORM" != MSYS* && "$PLATFORM" != CYGWIN* ]]; then
        assert_marker_removed "${TEST_HOME}/.zshrc"
        assert_marker_removed "${TEST_HOME}/.zprofile"
    fi

    print_test_summary || exit 1
}

SKIP_BUILD="${SKIP_BUILD:-false}"
main "$@"
