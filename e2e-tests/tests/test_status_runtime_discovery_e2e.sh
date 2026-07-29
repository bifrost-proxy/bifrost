#!/bin/bash
set -euo pipefail

: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

source "${PROJECT_DIR}/e2e-tests/test_utils/assert.sh"
source "${PROJECT_DIR}/e2e-tests/test_utils/process.sh"

BIFROST_BIN="${BIFROST_BIN:-${PROJECT_DIR}/target/release/bifrost}"
if [[ ! -x "$BIFROST_BIN" && -x "${PROJECT_DIR}/target/debug/bifrost" ]]; then
    BIFROST_BIN="${PROJECT_DIR}/target/debug/bifrost"
fi
if [[ ! -x "$BIFROST_BIN" && -x "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi

TEST_DATA_DIR=""
PROXY_PORT=""
PROXY_PID=""
HTTP_PORT=""
HTTP_PID=""
PYTHON_BIN=""

cleanup() {
    if [[ -n "$HTTP_PID" ]]; then
        safe_cleanup_proxy "$HTTP_PID"
    fi

    if [[ -n "$TEST_DATA_DIR" && -d "$TEST_DATA_DIR" ]]; then
        if [[ ! -f "${TEST_DATA_DIR}/runtime.json" \
            && -f "${TEST_DATA_DIR}/runtime.json.saved" ]]; then
            mv "${TEST_DATA_DIR}/runtime.json.saved" "${TEST_DATA_DIR}/runtime.json"
        fi
        if [[ ! -f "${TEST_DATA_DIR}/bifrost.pid" \
            && -f "${TEST_DATA_DIR}/bifrost.pid.saved" ]]; then
            mv "${TEST_DATA_DIR}/bifrost.pid.saved" "${TEST_DATA_DIR}/bifrost.pid"
        fi
        BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" stop >/dev/null 2>&1 || true
    fi

    if [[ -n "$PROXY_PID" ]]; then
        safe_cleanup_proxy "$PROXY_PID"
    fi
    if [[ -n "$TEST_DATA_DIR" && -d "$TEST_DATA_DIR" ]]; then
        rm -rf "$TEST_DATA_DIR"
    fi
}
trap cleanup EXIT

wait_admin_ready() {
    local port="$1"
    for _ in $(seq 1 100); do
        if curl -fsS \
            "http://127.0.0.1:${port}/_bifrost/api/system/overview" \
            >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.1
    done
    return 1
}

json_field() {
    local json="$1"
    local field="$2"
    if command -v jq >/dev/null 2>&1; then
        jq -r "$field" <<<"$json"
        return
    fi

    case "$field" in
        .running)
            sed -n 's/.*"running":\(true\|false\).*/\1/p' <<<"$json"
            ;;
        .pid)
            sed -n 's/.*"pid":\([0-9][0-9]*\).*/\1/p' <<<"$json"
            ;;
        .listener.port)
            sed -n 's/.*"listener":{[^}]*"port":\([0-9][0-9]*\).*/\1/p' <<<"$json"
            ;;
        .runtime_source)
            sed -n 's/.*"runtime_source":"\([^"]*\)".*/\1/p' <<<"$json"
            ;;
    esac
}

main() {
    if [[ ! -x "$BIFROST_BIN" ]]; then
        echo "Bifrost binary not found: $BIFROST_BIN" >&2
        exit 1
    fi

    PYTHON_BIN="$(python3_cmd)"
    TEST_DATA_DIR="$(mktemp -d "${PROJECT_DIR}/.bifrost-e2e-status-runtime-XXXXXX")"
    mark_e2e_data_root "$TEST_DATA_DIR"
    export BIFROST_DATA_DIR="$TEST_DATA_DIR"
    PROXY_PORT="$(allocate_free_port)"

    "$BIFROST_BIN" -p "$PROXY_PORT" start --daemon \
        --skip-cert-check --unsafe-ssl --no-system-proxy \
        >"${TEST_DATA_DIR}/start.log" 2>&1
    wait_admin_ready "$PROXY_PORT"

    PROXY_PID="$(pid_from_runtime_file "${TEST_DATA_DIR}/runtime.json")"
    [[ -n "$PROXY_PID" ]]
    kill -0 "$PROXY_PID" 2>/dev/null

    mv "${TEST_DATA_DIR}/runtime.json" "${TEST_DATA_DIR}/runtime.json.saved"
    mv "${TEST_DATA_DIR}/bifrost.pid" "${TEST_DATA_DIR}/bifrost.pid.saved"

    local status_json
    status_json="$("$BIFROST_BIN" -p "$PROXY_PORT" status --format json)"
    assert_equals "true" "$(json_field "$status_json" ".running")" \
        "status should discover the live service without runtime markers"
    assert_equals "$PROXY_PID" "$(json_field "$status_json" ".pid")" \
        "status should report the Admin API PID"
    assert_equals "$PROXY_PORT" "$(json_field "$status_json" ".listener.port")" \
        "status should report the requested listener port"
    assert_equals "admin_api" "$(json_field "$status_json" ".runtime_source")" \
        "status should expose the fallback source"

    "$PYTHON_BIN" - "${TEST_DATA_DIR}/runtime.json.saved" \
        "${TEST_DATA_DIR}/runtime.json" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as source:
    runtime = json.load(source)
runtime["pid"] = 2147483647
with open(sys.argv[2], "w", encoding="utf-8") as target:
    json.dump(runtime, target)
PY

    local stale_status_json
    stale_status_json="$("$BIFROST_BIN" -p "$PROXY_PORT" status --format json)"
    assert_equals "true" "$(json_field "$stale_status_json" ".running")" \
        "status should recover from a stale runtime PID"
    assert_equals "$PROXY_PID" "$(json_field "$stale_status_json" ".pid")" \
        "stale runtime fallback should report the live Admin API PID"
    assert_equals "admin_api" "$(json_field "$stale_status_json" ".runtime_source")" \
        "stale runtime fallback should expose the Admin API source"

    local reuse_output
    reuse_output="$("$BIFROST_BIN" -p "$PROXY_PORT" start --daemon --yes \
        --skip-cert-check --unsafe-ssl --no-system-proxy 2>&1)"
    assert_body_contains "Reusing the live service" "$reuse_output" \
        "start --yes should reuse the discovered Bifrost"
    kill -0 "$PROXY_PID" 2>/dev/null
    assert_equals "$PROXY_PID" \
        "$(curl -fsS "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/system/overview" \
            | sed -n 's/.*"pid":\([0-9][0-9]*\).*/\1/p')" \
        "start --yes must not replace the live process"

    HTTP_PORT="$(allocate_free_port)"
    "$PYTHON_BIN" -m http.server "$HTTP_PORT" \
        --bind 127.0.0.1 >"${TEST_DATA_DIR}/http.log" 2>&1 &
    HTTP_PID=$!
    for _ in $(seq 1 30); do
        if curl -fsS "http://127.0.0.1:${HTTP_PORT}/" >/dev/null 2>&1; then
            break
        fi
        sleep 0.1
    done
    curl -fsS "http://127.0.0.1:${HTTP_PORT}/" >/dev/null

    local non_bifrost_status
    non_bifrost_status="$("$BIFROST_BIN" -p "$HTTP_PORT" status --format json)"
    assert_equals "false" "$(json_field "$non_bifrost_status" ".running")" \
        "an ordinary HTTP listener must not be identified as Bifrost"

    print_test_summary
}

main "$@"
