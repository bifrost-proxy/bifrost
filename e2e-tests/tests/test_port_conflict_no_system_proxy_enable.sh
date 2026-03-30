#!/bin/bash

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"
BIFROST_BIN="${BIFROST_BIN:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)/target/release/bifrost}"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi

source "${SCRIPT_DIR}/../test_utils/assert.sh"
source "${SCRIPT_DIR}/../test_utils/process.sh"

TMP_DIR=""
DUMMY_PID=""
START_TIMEOUT="${START_TIMEOUT:-5}"

cleanup() {
    if [[ -n "${DUMMY_PID}" ]] && kill -0 "${DUMMY_PID}" 2>/dev/null; then
        kill_pid "${DUMMY_PID}"
        wait_pid "${DUMMY_PID}"
    fi
    if [[ -n "${TMP_DIR}" ]] && [[ -d "${TMP_DIR}" ]]; then
        rm -rf "${TMP_DIR}"
    fi
}
trap cleanup EXIT

pick_free_port() {
    python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

start_dummy_listener() {
    local port="$1"

    python3 - "$port" <<'PY' > /dev/null 2>&1 &
import socket
import sys
import time

port = int(sys.argv[1])
sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
sock.bind(("127.0.0.1", port))
sock.listen(1)

try:
    while True:
        time.sleep(1)
except KeyboardInterrupt:
    pass
finally:
    sock.close()
PY
    DUMMY_PID=$!
}

main() {
    local port
    port="$(pick_free_port)"

    start_dummy_listener "${port}"
    sleep 0.5

    TMP_DIR="$(mktemp -d)"
    local output
    local start_pid
    set +e
    local output_file="${TMP_DIR}/start.log"
    (
        BIFROST_DATA_DIR="${TMP_DIR}" \
        RUST_LOG=info \
        "$BIFROST_BIN" start -H 127.0.0.1 -p "${port}" --system-proxy --skip-cert-check --unsafe-ssl
    ) > "${output_file}" 2>&1 &
    start_pid=$!

    local waited=0
    while kill -0 "${start_pid}" 2>/dev/null && [[ "${waited}" -lt "${START_TIMEOUT}" ]]; do
        sleep 1
        waited=$((waited + 1))
    done

    local exit_code=0
    if kill -0 "${start_pid}" 2>/dev/null; then
        safe_cleanup_proxy "${start_pid}"
        exit_code=124
    else
        wait "${start_pid}"
        exit_code=$?
    fi
    output="$(cat "${output_file}")"
    set -e

    if [[ "${exit_code}" -eq 0 || "${exit_code}" -eq 124 ]]; then
        local actual="exit_code=${exit_code}"
        if [[ "${exit_code}" -eq 124 ]]; then
            actual="process kept running (timeout=${START_TIMEOUT}s)"
        fi
        _log_fail "port 冲突时应启动失败" "non-zero exit before serving" "${actual}"
        print_test_summary
        exit 1
    fi

    assert_body_contains "Failed to bind to" "${output}" "应报告端口绑定失败" || true
    assert_body_not_contains "System proxy enabled:" "${output}" "端口冲突时不应启用系统代理" || true

    print_test_summary || exit 1
}

main "$@"
