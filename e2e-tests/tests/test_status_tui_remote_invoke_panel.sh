#!/bin/bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

set -euo pipefail

unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY all_proxy ALL_PROXY no_proxy NO_PROXY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

source "${PROJECT_DIR}/e2e-tests/test_utils/process.sh"

BIFROST_BIN="${BIFROST_BIN:-${PROJECT_DIR}/target/debug/bifrost}"
TEST_DATA_DIR=""
ADMIN_PORT="${ADMIN_PORT:-}"
BIFROST_PID=""

pick_free_port() {
    python3 - <<'PY'
import socket
with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
    s.bind(("127.0.0.1", 0))
    print(s.getsockname()[1])
PY
}

cleanup() {
    if [[ -n "$BIFROST_PID" ]]; then
        kill "$BIFROST_PID" >/dev/null 2>&1 || true
        wait "$BIFROST_PID" >/dev/null 2>&1 || true
    fi
    if [[ -n "$TEST_DATA_DIR" ]]; then
        BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" stop >/dev/null 2>&1 || true
    fi
    if [[ -n "$ADMIN_PORT" ]]; then
        kill_bifrost_on_port "$ADMIN_PORT"
    fi
    if [[ -n "$TEST_DATA_DIR" && -d "$TEST_DATA_DIR" ]]; then
        rm -rf "$TEST_DATA_DIR"
    fi
}
trap cleanup EXIT

build_bifrost() {
    if [[ "${SKIP_BUILD:-false}" == "true" && -x "$BIFROST_BIN" ]]; then
        return 0
    fi
    SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
}

wait_for_admin_api() {
    local waited=0
    while [[ "$waited" -lt 80 ]]; do
        if env NO_PROXY="*" no_proxy="*" curl -fsS \
            "http://127.0.0.1:${ADMIN_PORT}/_bifrost/api/proxy/address" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.25
        waited=$((waited + 1))
    done
    echo "admin api did not become ready on port ${ADMIN_PORT}" >&2
    return 1
}

wait_for_remote_invoke_endpoint() {
    local path="$1"
    local waited=0
    while [[ "$waited" -lt 80 ]]; do
        if env NO_PROXY="*" no_proxy="*" curl -fsS \
            "http://127.0.0.1:${ADMIN_PORT}${path}" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.25
        waited=$((waited + 1))
    done
    echo "remote-invoke endpoint did not become ready: ${path}" >&2
    return 1
}

assert_remote_invoke_api_ready() {
    wait_for_remote_invoke_endpoint "/_bifrost/api/remote-invoke/status"
    wait_for_remote_invoke_endpoint "/_bifrost/api/remote-invoke/grants"
    wait_for_remote_invoke_endpoint "/_bifrost/api/remote-invoke/calls?limit=20"
}

assert_tui_remote_invoke_panel() {
    BIFROST_DATA_DIR="$TEST_DATA_DIR" python3 - <<'PY'
import fcntl
import os
import pty
import select
import struct
import subprocess
import termios
import time

cmd = [os.environ["BIFROST_BIN"], "status", "--tui"]
env = os.environ.copy()
env["TERM"] = "xterm-256color"

master, slave = pty.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 140, 0, 0))
proc = subprocess.Popen(cmd, stdin=slave, stdout=slave, stderr=slave, env=env, close_fds=True)
os.close(slave)

chunks = []
deadline = time.time() + 2
while time.time() < deadline:
    ready, _, _ = select.select([master], [], [], 0.2)
    if ready:
        chunks.append(os.read(master, 65536))

os.write(master, b"\x1b[C\x1b[C\x1b[C")
deadline = time.time() + 2
while time.time() < deadline:
    ready, _, _ = select.select([master], [], [], 0.2)
    if ready:
        chunks.append(os.read(master, 65536))

os.write(master, b"q")
deadline = time.time() + 5
while time.time() < deadline:
    ready, _, _ = select.select([master], [], [], 0.2)
    if ready:
        try:
            chunks.append(os.read(master, 65536))
        except OSError:
            break
    if proc.poll() is not None:
        break

proc.wait(timeout=5)
output = b"".join(chunks).decode("utf-8", "ignore")
os.close(master)

for needle in [
    "Remote Invoke",
    "Remote Invoke Status",
    "Connected Clients",
    "Recent",
    "Commands",
    "State:",
]:
    if needle not in output:
        raise SystemExit(f"missing {needle!r} in TUI output\n--- output ---\n{output[-4000:]}")

print("Remote Invoke TUI panel verified")
PY
}

main() {
    build_bifrost

    TEST_DATA_DIR="$(mktemp -d)"
    export BIFROST_DATA_DIR="$TEST_DATA_DIR"
    export BIFROST_BIN
    ADMIN_PORT="${ADMIN_PORT:-$(pick_free_port)}"

    local -a start_args=(
        start -p "$ADMIN_PORT" -H 127.0.0.1
        --skip-cert-check --unsafe-ssl --no-system-proxy --no-intercept
    )
    if [[ "${BIFROST_COVERAGE_E2E:-0}" == "1" ]]; then
        "$BIFROST_BIN" "${start_args[@]}" >"$TEST_DATA_DIR/bifrost.log" 2>&1 &
        BIFROST_PID=$!
    else
        "$BIFROST_BIN" "${start_args[@]}" --daemon
    fi

    wait_for_admin_api
    assert_remote_invoke_api_ready
    assert_tui_remote_invoke_panel

    echo "status TUI Remote Invoke panel E2E passed on port ${ADMIN_PORT}"
}

main "$@"
