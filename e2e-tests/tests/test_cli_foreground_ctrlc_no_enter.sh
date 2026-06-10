#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

BIN="${BIFROST_BIN:-$ROOT_DIR/target/release/bifrost}"
if [[ ! -x "$BIN" ]]; then
  echo "Building release bifrost binary..."
  SKIP_FRONTEND_BUILD=1 cargo build --release --bin bifrost
  BIN="$ROOT_DIR/target/release/bifrost"
fi

PORT="${BIFROST_CTRL_C_TEST_PORT:-${ADMIN_PORT:-0}}"
if [[ -z "$PORT" || "$PORT" == "0" ]]; then
  PORT="$(python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
)"
fi

export BIFROST_CTRL_C_TEST_BIN="$BIN"
export BIFROST_CTRL_C_TEST_PORT="$PORT"

python3 <<'PY'
import errno
import os
import pty
import select
import shutil
import signal
import sys
import tempfile
import time

root = os.getcwd()
data_dir = tempfile.mkdtemp(prefix="bifrost-ctrlc-e2e.")
env = os.environ.copy()
env["BIFROST_DATA_DIR"] = data_dir
env["BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT"] = "1"
bin_path = env["BIFROST_CTRL_C_TEST_BIN"]
port = env["BIFROST_CTRL_C_TEST_PORT"]

pid, master_fd = pty.fork()
if pid == 0:
    os.chdir(root)
    os.execvpe(
        bin_path,
        [
            bin_path,
            "start",
            "-p",
            port,
            "--no-system-proxy",
            "--no-tray",
            "--skip-cert-check",
        ],
        env,
    )


def poll_child():
    try:
        waited_pid, status = os.waitpid(pid, os.WNOHANG)
    except ChildProcessError:
        return 0
    if waited_pid == 0:
        return None
    return os.waitstatus_to_exitcode(status)


def read_master():
    try:
        return os.read(master_fd, 8192).decode("utf-8", errors="replace")
    except OSError as exc:
        if exc.errno == errno.EIO:
            return None
        raise


buffer = ""
deadline = time.time() + 45
try:
    while time.time() < deadline:
        code = poll_child()
        if code is not None:
            raise RuntimeError(f"bifrost exited before ready with code {code}\n{buffer[-4000:]}")
        ready, _, _ = select.select([master_fd], [], [], 0.2)
        if ready:
            chunk = read_master()
            if chunk is None:
                continue
            buffer += chunk
            if "MOBILE AVAILABILITY CHECK" in buffer:
                break
    else:
        raise RuntimeError(f"timed out waiting for foreground startup\n{buffer[-4000:]}")

    os.write(master_fd, b"\x03")
    exit_deadline = time.time() + 8
    while time.time() < exit_deadline:
        ready, _, _ = select.select([master_fd], [], [], 0.2)
        if ready:
            chunk = read_master()
            if chunk:
                buffer += chunk
        code = poll_child()
        if code is not None:
            if code != 0:
                raise RuntimeError(f"bifrost exited with code {code}\n{buffer[-4000:]}")
            if "Bifrost proxy stopped." not in buffer:
                raise RuntimeError(f"bifrost exited but stop message was missing\n{buffer[-4000:]}")
            print("PASS: foreground Ctrl-C exits without an extra Enter")
            sys.exit(0)
        if "Bifrost proxy stopped." in buffer:
            print("PASS: foreground Ctrl-C stops without an extra Enter")
            sys.exit(0)
    raise RuntimeError(f"bifrost did not stop after Ctrl-C without Enter\n{buffer[-4000:]}")
finally:
    if poll_child() is None:
        os.kill(pid, signal.SIGTERM)
        try:
            deadline = time.time() + 5
            while time.time() < deadline and poll_child() is None:
                time.sleep(0.1)
            if poll_child() is None:
                os.kill(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    os.close(master_fd)
    shutil.rmtree(data_dir, ignore_errors=True)
PY
