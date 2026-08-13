#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
TEST_ROOT="$(mktemp -d)"
export BIFROST_E2E_SANDBOX_DIR="$TEST_ROOT"
# shellcheck source=e2e-tests/test_utils/process.sh
source "$REPO_DIR/e2e-tests/test_utils/process.sh"

BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
BIFROST_LOG="$TEST_ROOT/bifrost.log"
ECHO_LOG="$TEST_ROOT/echo.log"
WORKER_LOG_DIR="$TEST_ROOT/worker-logs"
mkdir -p "$WORKER_LOG_DIR"

BIFROST_PID=""
ECHO_PID=""

cleanup() {
  if [[ -n "$BIFROST_PID" ]]; then
    safe_cleanup_proxy "$BIFROST_PID" || true
  fi
  if [[ -n "$ECHO_PID" ]]; then
    terminate_process_tree "$ECHO_PID" 1 || true
  fi
  kill_bifrost_in_data_root "$BIFROST_E2E_SANDBOX_DIR" || true
  rm -rf "$TEST_ROOT"
}
trap cleanup EXIT INT TERM

fail() {
  echo "[auxiliary-worker-isolation] ERROR: $*" >&2
  [[ -f "$BIFROST_LOG" ]] && tail -160 "$BIFROST_LOG" >&2 || true
  exit 1
}

if [[ "${SKIP_BUILD:-false}" != "true" ]]; then
  echo "[auxiliary-worker-isolation] building bifrost"
  (cd "$REPO_DIR" && SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost)
fi
[[ -x "$BIFROST_BIN" ]] || fail "bifrost binary not found: $BIFROST_BIN"

ADMIN_PORT="$(allocate_free_port)"
ECHO_PORT="$(allocate_free_port)"
PYTHON_BIN="$(python3_cmd)"

"$PYTHON_BIN" "$REPO_DIR/e2e-tests/mock_servers/http_echo_server.py" "$ECHO_PORT" \
  >"$ECHO_LOG" 2>&1 &
ECHO_PID=$!
for _ in $(seq 1 100); do
  if grep -q '^READY$' "$ECHO_LOG" 2>/dev/null; then
    break
  fi
  kill -0 "$ECHO_PID" 2>/dev/null || fail "echo server exited before ready"
  sleep 0.1
done
grep -q '^READY$' "$ECHO_LOG" || fail "echo server did not become ready"

export BIFROST_DATA_DIR="$TEST_ROOT/data"
mark_e2e_data_root "$BIFROST_DATA_DIR"
BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
BIFROST_DISABLE_TRAY=1 \
BIFROST_DATA_DIR="$BIFROST_DATA_DIR" \
"$BIFROST_BIN" start \
  --host 127.0.0.1 \
  --port "$ADMIN_PORT" \
  --unsafe-ssl \
  --skip-cert-check \
  --no-system-proxy \
  >"$BIFROST_LOG" 2>&1 &
BIFROST_PID=$!

READY_URL="http://127.0.0.1:$ADMIN_PORT/_bifrost/api/proxy/address"
wait_for_http_ready "$READY_URL" 45 0.2 || fail "bifrost did not become ready"

# No optional worker or browser may be eagerly launched by `bifrost start`.
curl -fsS --noproxy '*' \
  "http://127.0.0.1:$ADMIN_PORT/_bifrost/api/workers" \
  >"$TEST_ROOT/workers.json"
"$PYTHON_BIN" - "$TEST_ROOT/workers.json" <<'PY'
import json, sys
workers = json.load(open(sys.argv[1], encoding="utf-8"))
assert workers == [], workers
PY

curl -fsS --noproxy '*' \
  "http://127.0.0.1:$ADMIN_PORT/_bifrost/api/workers/modes" \
  >"$TEST_ROOT/modes.json"
"$PYTHON_BIN" - "$TEST_ROOT/modes.json" <<'PY'
import json, sys
modes = json.load(open(sys.argv[1], encoding="utf-8"))
assert len(modes) == 6, modes
assert {item["workerKind"] for item in modes} == {
    "external_cli", "browser", "asr", "im_gateway", "remote_invoke", "remote_execution"
}, modes
assert all(item["executionMode"] == "worker" for item in modes), modes
assert all(item["environmentVariable"].startswith("BIFROST_") for item in modes), modes
PY

# The proxy data path must work before and after auxiliary worker failures.
proxy_smoke() {
  curl -fsS --noproxy '' \
    --proxy "http://127.0.0.1:$ADMIN_PORT" \
    "http://127.0.0.1:$ECHO_PORT/auxiliary-worker-isolation?phase=$1" \
    >"$TEST_ROOT/proxy-$1.json"
  grep -q 'auxiliary-worker-isolation' "$TEST_ROOT/proxy-$1.json" \
    || fail "proxy smoke response did not contain request path ($1)"
}
proxy_smoke before

# Exercise the real hidden worker entrypoint and its bounded NDJSON protocol.
"$PYTHON_BIN" - "$BIFROST_BIN" "$BIFROST_DATA_DIR" "$WORKER_LOG_DIR" <<'PY'
import json
import os
import select
import subprocess
import sys
import time

binary, data_dir, log_dir = sys.argv[1:4]
TOKEN = "e2e-startup-token"

def read_frame(proc, timeout=10.0):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        ready, _, _ = select.select([proc.stdout], [], [], 0.2)
        if not ready:
            if proc.poll() is not None:
                raise AssertionError(f"worker exited early: {proc.returncode}")
            continue
        line = proc.stdout.readline()
        if not line:
            raise AssertionError(f"worker stdout closed: {proc.returncode}")
        return json.loads(line)
    raise AssertionError("timed out waiting for worker frame")

def start_worker(kind, stderr_name):
    env = os.environ.copy()
    env["BIFROST_WORKER_STARTUP_TOKEN"] = TOKEN
    stderr_path = os.path.join(log_dir, stderr_name)
    stderr = open(stderr_path, "w+", encoding="utf-8")
    proc = subprocess.Popen(
        [binary, "auxiliary-worker", "--kind", kind, "--data-dir", data_dir,
         "--admin-host", "127.0.0.1", "--admin-port", "0"],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=stderr,
        text=True, bufsize=1, env=env,
    )
    hello = read_frame(proc)
    assert hello["type"] == "hello", hello
    assert hello["hello"]["startupToken"] == TOKEN, hello
    assert hello["hello"]["workerKind"] == kind, hello
    ready = read_frame(proc)
    assert ready["type"] == "ready", ready
    return proc, stderr, stderr_path

def send(proc, frame):
    proc.stdin.write(json.dumps(frame, separators=(",", ":")) + "\n")
    proc.stdin.flush()

def wait_response(proc, request_id, timeout=10.0):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        frame = read_frame(proc, max(0.2, deadline - time.monotonic()))
        if frame.get("type") == "response" and frame["response"]["requestId"] == request_id:
            return frame["response"]
    raise AssertionError(f"response {request_id!r} not observed")

def clean_roundtrip(kind):
    proc, stderr, _ = start_worker(kind, f"{kind}.stderr.log")
    try:
        send(proc, {"type": "ping", "request_id": f"ping-{kind}"})
        response = wait_response(proc, f"ping-{kind}")
        assert response["ok"] is True and response["payload"]["pong"] is True, response
        send(proc, {"type": "shutdown", "request_id": f"shutdown-{kind}"})
        response = wait_response(proc, f"shutdown-{kind}")
        assert response["ok"] is True, response
        proc.stdin.close()
        assert proc.wait(timeout=10) == 0
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait(timeout=5)
        stderr.close()

clean_roundtrip("asr")
clean_roundtrip("remote_execution")

# A frame larger than 1 MiB must terminate only that worker protocol session.
proc, stderr, stderr_path = start_worker("asr", "oversized.stderr.log")
try:
    oversized = '{"type":"ping","request_id":"' + ('x' * (1024 * 1024 + 128)) + '"}\n'
    proc.stdin.write(oversized)
    proc.stdin.flush()
    proc.stdin.close()
    proc.wait(timeout=10)
finally:
    if proc.poll() is None:
        proc.kill()
        proc.wait(timeout=5)
    stderr.flush()
    stderr.seek(0)
    error_text = stderr.read()
    stderr.close()
assert "hard limit" in error_text or "input rejected" in error_text, error_text
PY

kill -0 "$BIFROST_PID" 2>/dev/null || fail "main process exited after worker protocol failure"
proxy_smoke after

# Job and lifecycle control APIs remain available even when no worker is active.
curl -fsS --noproxy '*' \
  "http://127.0.0.1:$ADMIN_PORT/_bifrost/api/worker-jobs" \
  >"$TEST_ROOT/jobs.json"
"$PYTHON_BIN" - "$TEST_ROOT/jobs.json" <<'PY'
import json, sys
jobs = json.load(open(sys.argv[1], encoding="utf-8"))
assert isinstance(jobs, list), jobs
PY

HTTP_CODE="$(curl -sS --noproxy '*' -o "$TEST_ROOT/start-asr.json" -w '%{http_code}' \
  -X POST "http://127.0.0.1:$ADMIN_PORT/_bifrost/api/workers/asr/start")"
[[ "$HTTP_CODE" == "409" ]] || fail "manual start without registered spawn spec should return 409, got $HTTP_CODE"

curl -fsS --noproxy '*' -X POST \
  "http://127.0.0.1:$ADMIN_PORT/_bifrost/api/workers/asr/reset-circuit" \
  >"$TEST_ROOT/reset-asr.json"

echo "[auxiliary-worker-isolation] PASS"
