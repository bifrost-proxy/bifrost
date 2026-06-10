#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

echo "Building bifrost binary..."
cargo build --bin bifrost

python3 <<'PY'
import os
import pty
import select
import shutil
import signal
import subprocess
import sys
import tempfile
import time

root = os.getcwd()
data_dir = tempfile.mkdtemp(prefix="bifrost-ctrlc-e2e.")
env = os.environ.copy()
env["BIFROST_DATA_DIR"] = data_dir
env["BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT"] = "1"

master_fd, slave_fd = pty.openpty()
proc = subprocess.Popen(
    [
        os.path.join(root, "target", "debug", "bifrost"),
        "start",
        "-p",
        "59124",
        "--no-system-proxy",
        "--no-tray",
        "--skip-cert-check",
    ],
    cwd=root,
    env=env,
    stdin=slave_fd,
    stdout=slave_fd,
    stderr=slave_fd,
    close_fds=True,
)
os.close(slave_fd)

buffer = ""
deadline = time.time() + 45
try:
    while time.time() < deadline:
        ready, _, _ = select.select([master_fd], [], [], 0.2)
        if ready:
            chunk = os.read(master_fd, 8192).decode("utf-8", errors="replace")
            buffer += chunk
            if "MOBILE AVAILABILITY CHECK" in buffer:
                break
        if proc.poll() is not None:
            raise RuntimeError(f"bifrost exited before ready with code {proc.returncode}\n{buffer[-4000:]}")
    else:
        raise RuntimeError(f"timed out waiting for foreground startup\n{buffer[-4000:]}")

    proc.send_signal(signal.SIGINT)
    exit_deadline = time.time() + 8
    while time.time() < exit_deadline:
        ready, _, _ = select.select([master_fd], [], [], 0.2)
        if ready:
            try:
                buffer += os.read(master_fd, 8192).decode("utf-8", errors="replace")
            except OSError:
                pass
        code = proc.poll()
        if code is not None:
            if code != 0:
                raise RuntimeError(f"bifrost exited with code {code}\n{buffer[-4000:]}")
            if "Bifrost proxy stopped." not in buffer:
                raise RuntimeError(f"bifrost exited but stop message was missing\n{buffer[-4000:]}")
            print("PASS: foreground Ctrl-C exits without an extra Enter")
            sys.exit(0)
    raise RuntimeError(f"bifrost did not exit after Ctrl-C without Enter\n{buffer[-4000:]}")
finally:
    if proc.poll() is None:
        proc.send_signal(signal.SIGTERM)
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
    os.close(master_fd)
    shutil.rmtree(data_dir, ignore_errors=True)
PY
