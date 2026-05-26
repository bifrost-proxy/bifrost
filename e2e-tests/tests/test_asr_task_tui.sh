#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

PORT="${BIFROST_TEST_PORT:-18883}"
TARGET_DIR="${CARGO_TARGET_DIR:-target}"
BIN="$TARGET_DIR/debug/bifrost"
DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-asr-tui.XXXXXX")"
LOG_FILE="$DATA_DIR/bifrost.log"
OPEN_LOG="$DATA_DIR/open.log"
SERVER_PID=""

cleanup() {
  if [[ -n "${SERVER_PID:-}" ]]; then
    kill "$SERVER_PID" >/dev/null 2>&1 || true
    wait "$SERVER_PID" >/dev/null 2>&1 || true
  fi
  rm -rf "$DATA_DIR"
}
trap cleanup EXIT

cargo build --bin bifrost

mkdir -p "$DATA_DIR/audio-one" "$DATA_DIR/audio-two"
BIFROST_DATA_DIR="$DATA_DIR" "$BIN" start -p "$PORT" --unsafe-ssl --no-system-proxy --skip-cert-check >"$LOG_FILE" 2>&1 &
SERVER_PID=$!

for _ in $(seq 1 80); do
  if curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/asr/tasks/-/watch" >/dev/null 2>&1; then
    break
  fi
  if ! kill -0 "$SERVER_PID" >/dev/null 2>&1; then
    echo "bifrost server exited before ready"
    tail -n 200 "$LOG_FILE" || true
    exit 1
  fi
  sleep 0.25
done
if ! curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/asr/tasks/-/watch" >/dev/null; then
  echo "bifrost server did not become ready"
  tail -n 200 "$LOG_FILE" || true
  exit 1
fi

if BIFROST_DATA_DIR="$DATA_DIR" "$BIN" --port "$PORT" ai asr task tui --no-interactive-select --json-snapshot >"$DATA_DIR/no-task.out" 2>&1; then
  echo "expected no-task snapshot to fail"
  exit 1
fi
grep -q "No ASR directory tasks" "$DATA_DIR/no-task.out"

create_task() {
  local name="$1"
  local dir="$2"
  curl -fsS \
    -X POST \
    -H 'content-type: application/json' \
    --data "{\"name\":\"$name\",\"audio_dir\":\"$dir\",\"recursive\":true,\"enabled\":true,\"schedule\":{\"kind\":\"hourly\",\"minute\":0}}" \
    "http://127.0.0.1:$PORT/_bifrost/api/asr/tasks"
}

TASK_ONE_JSON="$(create_task "CLI TUI One" "$DATA_DIR/audio-one")"
TASK_ONE_ID="$(printf '%s' "$TASK_ONE_JSON" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"
TASK_ONE_DAILY_DIR="$DATA_DIR/asr/data/text/$TASK_ONE_ID/.daily"
mkdir -p "$TASK_ONE_DAILY_DIR/report" "$DATA_DIR/asr/tasks/$TASK_ONE_ID"
cat >"$TASK_ONE_DAILY_DIR/2026-05-20.md" <<'EOF'
# 2026-05-20

Daily merged ASR content from the first task.
EOF
cat >"$TASK_ONE_DAILY_DIR/report/2026-05-20-report.md" <<'EOF'
# Jennie Agent Report

Processed report for 2026-05-20.
EOF
python3 - "$TASK_ONE_ID" "$DATA_DIR" "$TASK_ONE_DAILY_DIR/2026-05-20.md" "$TASK_ONE_DAILY_DIR/report/2026-05-20-report.md" <<'PY'
import hashlib
import json
import pathlib
import sys

task_id, data_dir, source_path, report_path = sys.argv[1:5]
source = pathlib.Path(source_path)
report = pathlib.Path(report_path)
content = source.read_bytes()
state = {
    "version": 1,
    "documents": {
        "2026-05-20": {
            "date": "2026-05-20",
            "source_sha256": hashlib.sha256(content).hexdigest(),
            "source_len_bytes": len(content),
            "processed_at_ms": 1779292800000,
            "runner": "jennie-agent",
            "report_path": str(report),
            "last_run_id": "e2e-run-2026-05-20",
        }
    },
}
path = pathlib.Path(data_dir) / "asr" / "tasks" / task_id / "daily_agent_processed.json"
path.write_text(json.dumps(state), encoding="utf-8")
PY

BIFROST_DATA_DIR="$DATA_DIR" "$BIN" --port "$PORT" ai asr task watch --json-snapshot >"$DATA_DIR/one.json"
grep -q '"name": "CLI TUI One"' "$DATA_DIR/one.json"
grep -q '"consumption"' "$DATA_DIR/one.json"
grep -q '"daily_agent"' "$DATA_DIR/one.json"
grep -q '"processed_documents": 1' "$DATA_DIR/one.json"

BIFROST_DATA_DIR="$DATA_DIR" "$BIN" --port "$PORT" ai asr task daily list >"$DATA_DIR/daily-list-one.out"
grep -q '2026-05-20' "$DATA_DIR/daily-list-one.out"
BIFROST_DATA_DIR="$DATA_DIR" "$BIN" --port "$PORT" ai asr task daily show 2026-05-20 >"$DATA_DIR/daily-show-one.out"
grep -q 'Daily merged ASR content from the first task' "$DATA_DIR/daily-show-one.out"

python3 - "$BIN" "$PORT" "$DATA_DIR" <<'PY'
import os
import pty
import subprocess
import sys
import time
import fcntl
import struct
import termios

bin_path, port, data_dir = sys.argv[1:4]
master, slave = pty.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 120, 0, 0))
fcntl.fcntl(master, fcntl.F_SETFL, os.O_NONBLOCK)
env = os.environ.copy()
env["BIFROST_DATA_DIR"] = data_dir
env["BIFROST_ASR_TUI_OPEN_LOG"] = os.path.join(data_dir, "open.log")
proc = subprocess.Popen(
    [bin_path, "--port", port, "ai", "asr", "task", "tui", "--read-only"],
    stdin=slave,
    stdout=slave,
    stderr=slave,
    env=env,
)
os.close(slave)
output = b""
deadline = time.time() + 5
while time.time() < deadline and proc.poll() is None:
    try:
        chunk = os.read(master, 4096)
        if chunk:
            output += chunk
            if b"Daily" in output and b"Jennie" in output:
                break
    except BlockingIOError:
        pass
    except OSError:
        break
    time.sleep(0.05)
for _ in range(3):
    os.write(master, b"q")
    time.sleep(0.1)
deadline = time.time() + 5
while time.time() < deadline and proc.poll() is None:
    try:
        output += os.read(master, 4096)
    except BlockingIOError:
        pass
    except OSError:
        break
    time.sleep(0.05)
if proc.poll() is None:
    proc.terminate()
    try:
        proc.wait(timeout=2)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait(timeout=2)
    raise SystemExit(f"tui did not exit after q: {output[-500:]!r}")
if proc.returncode != 0:
    raise SystemExit(f"tui exited with {proc.returncode}: {output[-500:]!r}")
if b"Daily" not in output or b"Jennie" not in output:
    raise SystemExit(f"tui did not render Daily/Jennie Agent panel: {output[-800:]!r}")
PY

python3 - "$BIN" "$PORT" "$DATA_DIR" "$TASK_ONE_ID" <<'PY'
import os
import pty
import subprocess
import sys
import time
import fcntl
import struct
import termios

bin_path, port, data_dir, task_id = sys.argv[1:5]
master, slave = pty.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 140, 0, 0))
fcntl.fcntl(master, fcntl.F_SETFL, os.O_NONBLOCK)
env = os.environ.copy()
env["BIFROST_DATA_DIR"] = data_dir
env["BIFROST_ASR_TUI_OPEN_LOG"] = os.path.join(data_dir, "open.log")
proc = subprocess.Popen(
    [bin_path, "--port", port, "ai", "asr", "task", "tui", task_id],
    stdin=slave,
    stdout=slave,
    stderr=slave,
    env=env,
)
os.close(slave)
output = b""
deadline = time.time() + 5
while time.time() < deadline and proc.poll() is None:
    try:
        chunk = os.read(master, 4096)
        if chunk:
            output += chunk
            if b"R run now" in output:
                break
    except BlockingIOError:
        pass
    except OSError:
        break
    time.sleep(0.05)
for key in (b"r", b"\r", b"P"):
    os.write(master, key)
    time.sleep(0.4)
    deadline = time.time() + 3
    while time.time() < deadline and proc.poll() is None:
        try:
            chunk = os.read(master, 4096)
            if chunk:
                output += chunk
                if key == b"\r" and b"Opened" in output:
                    break
                if key == b"P" and (b"force-pause" in output or b"paused" in output):
                    break
        except BlockingIOError:
            pass
        except OSError:
            break
        time.sleep(0.05)
for _ in range(3):
    os.write(master, b"q")
    time.sleep(0.1)
deadline = time.time() + 5
while time.time() < deadline and proc.poll() is None:
    try:
        output += os.read(master, 4096)
    except BlockingIOError:
        pass
    except OSError:
        break
    time.sleep(0.05)
if proc.poll() is None:
    proc.terminate()
    try:
        proc.wait(timeout=2)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait(timeout=2)
    raise SystemExit(f"interactive control tui did not exit: {output[-800:]!r}")
if proc.returncode != 0:
    raise SystemExit(f"interactive control tui exited with {proc.returncode}: {output[-800:]!r}")
if b"force-pause" not in output and b"paused" not in output:
    raise SystemExit(f"force pause shortcut did not report a pause result: {output[-1000:]!r}")
if b"Opened" not in output:
    raise SystemExit(f"open shortcut did not report an opened file: {output[-1200:]!r}")
PY
if ! grep -q "2026-05-20-report.md" "$OPEN_LOG"; then
  echo "expected TUI open log to include Daily Agent report path"
  cat "$OPEN_LOG" || true
  exit 1
fi

curl -fsS -X POST "http://127.0.0.1:$PORT/_bifrost/api/asr/tasks/$TASK_ONE_ID/resume" >"$DATA_DIR/resume.json"
grep -q '"paused":false' "$DATA_DIR/resume.json"
curl -fsS -X POST "http://127.0.0.1:$PORT/_bifrost/api/asr/tasks/$TASK_ONE_ID/pause?mode=temporary" >"$DATA_DIR/pause.json"
grep -q '"pause_mode":"temporary"' "$DATA_DIR/pause.json"
RUN_PAUSED_STATUS="$(curl -sS -o "$DATA_DIR/run-paused.json" -w '%{http_code}' -X POST "http://127.0.0.1:$PORT/_bifrost/api/asr/tasks/$TASK_ONE_ID/run")"
if [[ "$RUN_PAUSED_STATUS" != "409" ]]; then
  echo "expected paused task run to fail with 409, got $RUN_PAUSED_STATUS"
  exit 1
fi
grep -q 'ASR task is paused; resume it before starting a run' "$DATA_DIR/run-paused.json"
curl -fsS -X POST "http://127.0.0.1:$PORT/_bifrost/api/asr/tasks/$TASK_ONE_ID/resume" >"$DATA_DIR/resume-again.json"
grep -q '"running":true' "$DATA_DIR/resume-again.json"

create_task "CLI TUI Two" "$DATA_DIR/audio-two" >/dev/null

python3 - "$BIN" "$PORT" "$DATA_DIR" <<'PY'
import os
import pty
import subprocess
import sys
import time
import fcntl
import struct
import termios

bin_path, port, data_dir = sys.argv[1:4]
master, slave = pty.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 120, 0, 0))
fcntl.fcntl(master, fcntl.F_SETFL, os.O_NONBLOCK)
env = os.environ.copy()
env["BIFROST_DATA_DIR"] = data_dir
proc = subprocess.Popen(
    [bin_path, "--port", port, "ai", "asr", "task", "tui", "--read-only"],
    stdin=slave,
    stdout=slave,
    stderr=slave,
    env=env,
)
os.close(slave)
output = b""
deadline = time.time() + 5
while time.time() < deadline and proc.poll() is None:
    try:
        chunk = os.read(master, 4096)
        if chunk:
            output += chunk
            if (
                b"Select ASR task to watch" in output
                and b"CLI TUI One" in output
                and b"CLI TUI Two" in output
            ):
                break
    except BlockingIOError:
        pass
    except OSError:
        break
    time.sleep(0.05)
if (
    b"Select ASR task to watch" not in output
    or b"CLI TUI One" not in output
    or b"CLI TUI Two" not in output
):
    raise SystemExit(f"multi-task selector did not render both task options: {output[-1200:]!r}")
os.write(master, b"\x1b[B")
time.sleep(0.1)
os.write(master, b"\r")
deadline = time.time() + 10
while time.time() < deadline and proc.poll() is None:
    try:
        chunk = os.read(master, 4096)
        if chunk:
            output += chunk
            if b"CLI TUI Two" in output:
                break
    except BlockingIOError:
        pass
    except OSError:
        break
    time.sleep(0.05)
for _ in range(3):
    os.write(master, b"q")
    time.sleep(0.1)
deadline = time.time() + 5
while time.time() < deadline and proc.poll() is None:
    try:
        output += os.read(master, 4096)
    except BlockingIOError:
        pass
    except OSError:
        break
    time.sleep(0.05)
if proc.poll() is None:
    proc.terminate()
    try:
        proc.wait(timeout=2)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait(timeout=2)
    raise SystemExit(f"multi-task selector tui did not exit: {output[-800:]!r}")
if proc.returncode != 0:
    raise SystemExit(f"multi-task selector tui exited with {proc.returncode}: {output[-800:]!r}")
if b"Select ASR task to watch" not in output or b"CLI TUI Two" not in output:
    raise SystemExit(f"multi-task selector did not choose second task: {output[-1200:]!r}")
PY

if BIFROST_DATA_DIR="$DATA_DIR" "$BIN" --port "$PORT" ai asr task watch --no-interactive-select --json-snapshot >"$DATA_DIR/many.out" 2>&1; then
  echo "expected multi-task command without task to fail"
  exit 1
fi
grep -q "Multiple ASR directory tasks" "$DATA_DIR/many.out"

if BIFROST_DATA_DIR="$DATA_DIR" "$BIN" --port "$PORT" ai asr task daily list >"$DATA_DIR/daily-many.out" 2>&1; then
  echo "expected multi-task daily list without task to fail"
  exit 1
fi
grep -q "Multiple ASR directory tasks" "$DATA_DIR/daily-many.out"

BIFROST_DATA_DIR="$DATA_DIR" "$BIN" --port "$PORT" ai asr task daily list "$TASK_ONE_ID" >"$DATA_DIR/daily-list-by-id.out"
grep -q '2026-05-20' "$DATA_DIR/daily-list-by-id.out"
BIFROST_DATA_DIR="$DATA_DIR" "$BIN" --port "$PORT" ai asr task daily show 2026-05-20 --task "$TASK_ONE_ID" >"$DATA_DIR/daily-show-by-task.out"
grep -q 'Daily merged ASR content from the first task' "$DATA_DIR/daily-show-by-task.out"

BIFROST_DATA_DIR="$DATA_DIR" "$BIN" --port "$PORT" ai asr task watch "$TASK_ONE_ID" --json-snapshot >"$DATA_DIR/one-by-id.json"
grep -q "\"id\": \"$TASK_ONE_ID\"" "$DATA_DIR/one-by-id.json"

BIFROST_DATA_DIR="$DATA_DIR" "$BIN" --port "$PORT" ai asr task watch --all --json-snapshot >"$DATA_DIR/all.json"
grep -q '"tasks"' "$DATA_DIR/all.json"
grep -q 'CLI TUI Two' "$DATA_DIR/all.json"

echo "ASR task TUI E2E passed"
