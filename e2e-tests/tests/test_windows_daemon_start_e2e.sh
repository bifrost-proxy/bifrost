#!/bin/bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

source "${PROJECT_DIR}/e2e-tests/test_utils/assert.sh"
source "${PROJECT_DIR}/e2e-tests/test_utils/process.sh"

if ! is_windows; then
    echo "SKIP: Windows daemon start regression only runs on Windows"
    exit 0
fi

PROXY_PORT="${PROXY_PORT:-18894}"
BIFROST_BIN="${BIFROST_BIN:-${PROJECT_DIR}/target/release/bifrost.exe}"
if [[ ! -x "$BIFROST_BIN" && -x "${PROJECT_DIR}/target/release/bifrost" ]]; then
    BIFROST_BIN="${PROJECT_DIR}/target/release/bifrost"
fi

TEST_DATA_DIR=""

cleanup() {
    if [[ -n "$TEST_DATA_DIR" && -d "$TEST_DATA_DIR" && -x "$BIFROST_BIN" ]]; then
        BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" -p "$PROXY_PORT" stop >/dev/null 2>&1 || true
        rm -rf "$TEST_DATA_DIR"
    fi
    kill_bifrost_on_port "$PROXY_PORT"
}
trap cleanup EXIT

build_if_needed() {
    if [[ "${SKIP_BUILD:-false}" == "true" && -x "$BIFROST_BIN" ]]; then
        return 0
    fi
    cargo build --release --bin bifrost
    if [[ ! -x "$BIFROST_BIN" && -x "${PROJECT_DIR}/target/release/bifrost.exe" ]]; then
        BIFROST_BIN="${PROJECT_DIR}/target/release/bifrost.exe"
    fi
}

assert_runtime_daemon_mode() {
    "$(python3_cmd)" - "$TEST_DATA_DIR/runtime.json" "$PROXY_PORT" <<'PY'
import json
import sys

runtime_path = sys.argv[1]
expected_port = int(sys.argv[2])
with open(runtime_path, "r", encoding="utf-8") as fh:
    runtime = json.load(fh)

mode = runtime.get("runtime_start_mode") or runtime.get("start_mode")
port = int(runtime.get("port", 0))
pid = int(runtime.get("pid", 0))
if mode != "daemon" or port != expected_port or pid <= 0:
    raise SystemExit(
        f"runtime mismatch: mode={mode!r}, port={port}, pid={pid}, expected_port={expected_port}"
    )
PY
}

test_windows_daemon_start() {
    _log_info "case: Windows start --daemon launches detached child and writes daemon runtime"

    TEST_DATA_DIR="$(mktemp -d)"

    local output
    output="$(BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" -p "$PROXY_PORT" start \
        --daemon --skip-cert-check --unsafe-ssl --no-system-proxy --no-tray 2>&1)"
    local code=$?

    if [[ "$code" -eq 0 ]]; then
        _log_pass "Windows daemon start command exits successfully"
    else
        _log_fail "Windows daemon start command exits successfully" "0" "$code"
        echo "$output" >&2
        return 1
    fi

    assert_body_contains "Daemon started with PID" "$output" \
        "Windows daemon parent reports child PID" || return 1

    local ready_url="http://127.0.0.1:${PROXY_PORT}/_bifrost/api/proxy/address"
    if wait_for_http_ready "$ready_url" 30 0.5; then
        _log_pass "Windows daemon admin API becomes ready"
    else
        _log_fail "Windows daemon admin API becomes ready" "reachable" "timeout"
        echo "$output" >&2
        return 1
    fi

    if [[ -s "$TEST_DATA_DIR/runtime.json" ]]; then
        _log_pass "Windows daemon writes runtime.json"
    else
        _log_fail "Windows daemon writes runtime.json" "runtime.json exists" "missing"
        echo "$output" >&2
        return 1
    fi

    assert_runtime_daemon_mode || {
        _log_fail "Windows daemon runtime records daemon mode and expected port" \
            "runtime_start_mode=daemon port=${PROXY_PORT}" "mismatch"
        echo "$output" >&2
        return 1
    }
    _log_pass "Windows daemon runtime records daemon mode and expected port"

    BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" -p "$PROXY_PORT" stop >/dev/null 2>&1 || true
    if win_wait_port_free "$PROXY_PORT" 20; then
        _log_pass "Windows daemon stop releases proxy port"
    else
        _log_fail "Windows daemon stop releases proxy port" "free" "still listening"
        return 1
    fi
}

main() {
    build_if_needed || { print_test_summary || exit 1; return 1; }
    test_windows_daemon_start || { print_test_summary || exit 1; return 1; }
    print_test_summary || exit 1
}

main "$@"
