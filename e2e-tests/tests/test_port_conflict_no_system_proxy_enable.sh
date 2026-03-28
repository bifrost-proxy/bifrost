#!/bin/bash

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"
BIFROST_BIN="${BIFROST_BIN:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)/target/release/bifrost}"

source "${SCRIPT_DIR}/../test_utils/assert.sh"

TMP_DIR=""
DUMMY_PID=""

cleanup() {
    if [[ -n "${DUMMY_PID}" ]] && kill -0 "${DUMMY_PID}" 2>/dev/null; then
        kill "${DUMMY_PID}" 2>/dev/null || true
        wait "${DUMMY_PID}" 2>/dev/null || true
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

main() {
    local port
    port="$(pick_free_port)"

    python3 -m http.server "${port}" > /dev/null 2>&1 &
    DUMMY_PID=$!
    sleep 0.5

    TMP_DIR="$(mktemp -d)"
    local output
    set +e
    output=$(
        BIFROST_DATA_DIR="${TMP_DIR}" \
        RUST_LOG=info \
        "$BIFROST_BIN" start -p "${port}" --system-proxy --skip-cert-check --unsafe-ssl 2>&1
    )
    local exit_code=$?
    set -e

    if [[ "${exit_code}" -eq 0 ]]; then
        _log_fail "port 冲突时应启动失败" "non-zero exit" "exit_code=0"
        print_test_summary
        exit 1
    fi

    assert_body_contains "Failed to bind to" "${output}" "应报告端口绑定失败" || true
    assert_body_not_contains "System proxy enabled:" "${output}" "端口冲突时不应启用系统代理" || true

    print_test_summary || exit 1
}

main "$@"
