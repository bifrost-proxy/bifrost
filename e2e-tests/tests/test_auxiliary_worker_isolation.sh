#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
TEST_ROOT="$(mktemp -d)"
export BIFROST_E2E_SANDBOX_DIR="$TEST_ROOT"
# shellcheck source=e2e-tests/test_utils/process.sh
source "$REPO_DIR/e2e-tests/test_utils/process.sh"
mark_e2e_data_root "$TEST_ROOT"

BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
BIFROST_LOG="$TEST_ROOT/bifrost.log"
ECHO_LOG="$TEST_ROOT/echo.log"
WORKER_LOG_DIR="$TEST_ROOT/worker-logs"
MOCK_CODEX="$TEST_ROOT/mock-codex"
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
  [[ -f "$ECHO_LOG" ]] && tail -80 "$ECHO_LOG" >&2 || true
  [[ -f "$BIFROST_LOG" ]] && tail -160 "$BIFROST_LOG" >&2 || true
  exit 1
}

if [[ "${SKIP_BUILD:-false}" != "true" ]]; then
  echo "[auxiliary-worker-isolation] building bifrost"
  (cd "$REPO_DIR" && SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost)
fi
[[ -x "$BIFROST_BIN" ]] || fail "bifrost binary not found: $BIFROST_BIN"

cat >"$MOCK_CODEX" <<'PY'
#!/usr/bin/env python3
import json
import sys
import time

if "--version" in sys.argv:
    print("codex-cli 0.144.1")
    raise SystemExit(0)

def send(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)

for line in sys.stdin:
    frame = json.loads(line)
    method = frame.get("method")
    request_id = frame.get("id")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": request_id, "result": {"userAgent": "awi-mock-codex"}})
    elif method == "thread/start":
        send({"jsonrpc": "2.0", "method": "thread/started", "params": {"thread": {"id": "awi-thread"}}})
        send({"jsonrpc": "2.0", "id": request_id, "result": {"thread": {"id": "awi-thread"}}})
    elif method == "turn/start":
        prompt = frame["params"]["input"][0]["text"]
        send({"jsonrpc": "2.0", "id": request_id, "result": {"turn": {"id": "awi-turn"}}})
        if "WAIT_FOR_CANCEL" in prompt:
            while True:
                time.sleep(1)
        response = "BIFROST_AUX_JOB_OK:" + ("x" * (320 * 1024))
        send({
            "jsonrpc": "2.0",
            "method": "item/completed",
            "params": {
                "threadId": "awi-thread",
                "turnId": "awi-turn",
                "item": {"id": "awi-message", "type": "agentMessage", "text": response},
            },
        })
        send({
            "jsonrpc": "2.0",
            "method": "turn/completed",
            "params": {
                "threadId": "awi-thread",
                "turn": {"id": "awi-turn", "status": "completed"},
            },
        })
PY
chmod +x "$MOCK_CODEX"

ADMIN_PORT="${ADMIN_PORT:-$(allocate_free_port)}"
ECHO_PORT="${ECHO_HTTP_PORT:-$(allocate_free_port)}"
PYTHON_BIN="$(python3_cmd)"

"$PYTHON_BIN" "$REPO_DIR/e2e-tests/mock_servers/http_echo_server.py" "$ECHO_PORT" \
  >"$ECHO_LOG" 2>&1 &
ECHO_PID=$!
for _ in $(seq 1 450); do
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
BIFROST_E2E=1 \
BIFROST_CHATGPT_WEB_E2E_MOCK=1 \
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

# Remote Invoke keeps one tokenless standby worker so its loopback Admin API is
# available before sync login. It must remain idle and isolated from the proxy;
# every other optional worker stays lazy.
WORKERS_URL="http://127.0.0.1:$ADMIN_PORT/_bifrost/api/workers"
# The proxy readiness endpoint can become available while the parent is still
# finishing its one-time Remote Invoke endpoint discovery. That control request
# is intentionally visible as an active worker job, so wait for the worker to
# settle instead of racing the next heartbeat snapshot.
for _ in $(seq 1 100); do
  curl -fsS --noproxy '*' "$WORKERS_URL" >"$TEST_ROOT/workers.json"
  if "$PYTHON_BIN" - "$TEST_ROOT/workers.json" "$BIFROST_PID" <<'PY'
import json, sys
workers = json.load(open(sys.argv[1], encoding="utf-8"))
settled = (
    len(workers) == 1
    and workers[0]["workerKind"] == "remote_invoke"
    and workers[0]["state"] == "ready"
    and workers[0]["pid"] != int(sys.argv[2])
    and workers[0]["activeJobs"] == 0
    and workers[0]["queuedJobs"] == 0
)
raise SystemExit(0 if settled else 1)
PY
  then
    break
  fi
  sleep 0.1
done
"$PYTHON_BIN" - "$TEST_ROOT/workers.json" "$BIFROST_PID" <<'PY'
import json, sys
workers = json.load(open(sys.argv[1], encoding="utf-8"))
assert len(workers) == 1, workers
worker = workers[0]
assert worker["workerKind"] == "remote_invoke", workers
assert worker["state"] == "ready", workers
assert worker["pid"] != int(sys.argv[2]), workers
assert worker["activeJobs"] == 0, workers
assert worker["queuedJobs"] == 0, workers
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

# The public Admin send endpoint must cross into the IM worker even for a
# validation failure. This proves that outbound provider work does not silently
# fall back to the proxy process.
HTTP_CODE="$(curl -sS --noproxy '*' -o "$TEST_ROOT/im-send-missing.json" -w '%{http_code}' \
  -X POST -H 'content-type: application/json' \
  --data '{"provider_id":"missing-provider","msg_type":"text","text":"isolate me"}' \
  "http://127.0.0.1:$ADMIN_PORT/_bifrost/api/im-gateway/messages/send")"
[[ "$HTTP_CODE" == "404" ]] || fail "isolated IM send validation should return 404, got $HTTP_CODE"
grep -q 'missing-provider' "$TEST_ROOT/im-send-missing.json" \
  || fail "isolated IM send response did not preserve worker validation detail"
curl -fsS --noproxy '*' \
  "http://127.0.0.1:$ADMIN_PORT/_bifrost/api/workers/im_gateway" \
  >"$TEST_ROOT/im-worker-after-send.json"
"$PYTHON_BIN" - "$TEST_ROOT/im-worker-after-send.json" "$BIFROST_PID" <<'PY'
import json, sys
workers = json.load(open(sys.argv[1], encoding="utf-8"))
assert len(workers) == 1, workers
assert workers[0]["workerKind"] == "im_gateway", workers
assert workers[0]["pid"] != int(sys.argv[2]), workers
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
"$PYTHON_BIN" - "$BIFROST_BIN" "$BIFROST_DATA_DIR" "$WORKER_LOG_DIR" "$ADMIN_PORT" "$ECHO_PORT" <<'PY'
import base64
import http.client
import json
import os
import select
import subprocess
import sys
import time

binary, data_dir, log_dir, admin_port, echo_port = sys.argv[1:6]
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

def start_worker(kind, stderr_name, extra_env=None):
    env = os.environ.copy()
    env["BIFROST_WORKER_STARTUP_TOKEN"] = TOKEN
    env.update(extra_env or {})
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
    assert hello["hello"]["protocolVersion"] == 1, hello
    assert hello["hello"]["pid"] == proc.pid, hello
    assert hello["hello"]["workerInstanceId"], hello
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

def request(proc, request_id, operation, payload, job_id=None):
    send(proc, {
        "type": "request",
        "request": {
            "requestId": request_id,
            "jobId": job_id,
            "operation": operation,
            "payload": payload,
        },
    })
    return wait_response(proc, request_id)

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

# Exercise one real operation for each reusable worker family. These requests
# stay inside the worker process and make protocol/dispatch regressions visible
# without requiring a real model, browser account, IM account, or relay.
proc, stderr, _ = start_worker(
    "browser",
    "browser-operation.stderr.log",
    {"BIFROST_E2E": "1", "BIFROST_CHATGPT_WEB_E2E_MOCK": "1"},
)
response = request(
    proc,
    "browser-clear",
    "browser.clear_session_conversation",
    {"sessionKey": "awi-browser-session"},
)
assert response["ok"] is True and response["payload"]["cleared"] is True, response
send(proc, {"type": "shutdown", "request_id": "browser-shutdown"})
assert wait_response(proc, "browser-shutdown")["ok"] is True
proc.stdin.close()
assert proc.wait(timeout=10) == 0
stderr.close()

proc, stderr, _ = start_worker("asr", "asr-operation.stderr.log")
response = request(
    proc,
    "asr-missing-task",
    "asr.run_directory_task",
    {"taskId": "awi-missing-task", "recordingDate": None},
    "task:awi-missing-task",
)
assert response["ok"] is False and "not found" in response["error"].lower(), response
send(proc, {"type": "shutdown", "request_id": "asr-shutdown"})
assert wait_response(proc, "asr-shutdown")["ok"] is True
proc.stdin.close()
assert proc.wait(timeout=10) == 0
stderr.close()

proc, stderr, _ = start_worker("im_gateway", "im-operation.stderr.log")
response = request(proc, "im-status", "im.runtime_status", {})
assert response["ok"] is True and isinstance(response["payload"]["providers"], list), response
im_request_dir = os.path.join(data_dir, "runtime", "im-gateway-worker", "requests")
os.makedirs(im_request_dir, exist_ok=True)
im_send_path = os.path.join(im_request_dir, "send-e2e.json")
with open(im_send_path, "w", encoding="utf-8") as handle:
    json.dump({
        "provider_id": "missing-provider",
        "msg_type": "text",
        "text": "isolated IM send",
    }, handle)
response = request(
    proc,
    "im-send",
    "im.send_message",
    {"requestPath": im_send_path},
)
assert response["ok"] is True and response["payload"]["status"] == 404, response
send_body = base64.b64decode(response["payload"]["bodyBase64"]).decode("utf-8")
assert "missing-provider" in send_body and not os.path.exists(im_send_path), response
im_upload_path = os.path.join(im_request_dir, "upload-e2e.bin")
with open(im_upload_path, "wb") as handle:
    handle.write(b"isolated IM upload")
response = request(
    proc,
    "im-upload",
    "im.upload_message",
    {
        "bodyPath": im_upload_path,
        "providerId": "missing-provider",
        "kind": "file",
        "fileName": "isolated.txt",
        "mimeType": "text/plain",
        "imageType": "message",
    },
)
assert response["ok"] is True and response["payload"]["status"] == 404, response
assert not os.path.exists(im_upload_path), response
send(proc, {"type": "shutdown", "request_id": "im-shutdown"})
assert wait_response(proc, "im-shutdown")["ok"] is True
proc.stdin.close()
assert proc.wait(timeout=10) == 0
stderr.close()

remote_env = {
    "BIFROST_REMOTE_RELAY_URL": "http://127.0.0.1:9",
    "BIFROST_REMOTE_SESSION_TOKEN": "awi-session-token",
    "BIFROST_REMOTE_WORKER_HTTP_TOKEN": "awi-http-token",
}
proc, stderr, _ = start_worker("remote_invoke", "remote-invoke-operation.stderr.log", remote_env)
response = request(proc, "remote-status", "remote.runtime_status", {})
assert response["ok"] is True and response["payload"]["relayUrl"] == "http://127.0.0.1:9", response
send(proc, {"type": "shutdown", "request_id": "remote-invoke-shutdown"})
assert wait_response(proc, "remote-invoke-shutdown")["ok"] is True
proc.stdin.close()
assert proc.wait(timeout=10) == 0
stderr.close()

proc, stderr, _ = start_worker("remote_execution", "remote-execution-operation.stderr.log")
response = request(
    proc,
    "remote-prepare",
    "remote_execution.prepare",
    {"executionId": "awi-execution"},
)
assert response["ok"] is True and response["payload"]["prepared"] is True, response
response = request(
    proc,
    "remote-stdin",
    "remote_execution.stdin",
    {
        "executionId": "awi-execution",
        "dataBase64": base64.b64encode(b"bounded-stdin").decode("ascii"),
    },
)
assert response["ok"] is True and response["payload"]["accepted"] is True, response
response = request(
    proc,
    "remote-stdin-close",
    "remote_execution.stdin_close",
    {"executionId": "awi-execution"},
)
assert response["ok"] is True and response["payload"]["closed"] is True, response
send(proc, {"type": "shutdown", "request_id": "remote-execution-shutdown"})
assert wait_response(proc, "remote-execution-shutdown")["ok"] is True
proc.stdin.close()
assert proc.wait(timeout=10) == 0
stderr.close()

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

# Keep proxy traffic flowing while every reusable auxiliary worker is killed.
# Each worker is a direct child owned by this test, so cleanup is exact.
def proxy_probe(phase):
    connection = http.client.HTTPConnection("127.0.0.1", int(admin_port), timeout=5)
    target = (
        f"http://127.0.0.1:{echo_port}/auxiliary-worker-isolation"
        f"?phase={phase}"
    )
    connection.request("GET", target)
    response = connection.getresponse()
    body = response.read()
    connection.close()
    assert response.status == 200 and b"auxiliary-worker-isolation" in body, (
        phase,
        response.status,
        body[:500],
    )

chaos_workers = []
for kind, env in [
    ("browser", {"BIFROST_E2E": "1", "BIFROST_CHATGPT_WEB_E2E_MOCK": "1"}),
    ("asr", {}),
    ("im_gateway", {}),
    ("remote_invoke", remote_env),
    ("remote_execution", {}),
]:
    proc, stderr, _ = start_worker(kind, f"chaos-{kind}.stderr.log", env)
    chaos_workers.append((kind, proc, stderr))

proxy_probe("chaos-before")
for kind, proc, stderr in chaos_workers:
    proc.kill()
    assert proc.wait(timeout=10) != 0, (kind, proc.returncode)
    stderr.close()
    proxy_probe(f"chaos-after-{kind}")
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

# Execute a real per-job External CLI worker through the Admin API. Verify the
# job registry, bounded artifact reads, containment, and cancellation path.
"$PYTHON_BIN" - "$ADMIN_PORT" "$MOCK_CODEX" "$REPO_DIR" <<'PY'
import json
import threading
import time
import urllib.error
import urllib.request
import sys

port, executable, repo_dir = sys.argv[1:4]
base = f"http://127.0.0.1:{port}/_bifrost/api"

def request_bytes(path, method="GET", payload=None, expected=(200,)):
    data = None if payload is None else json.dumps(payload).encode("utf-8")
    request = urllib.request.Request(
        base + path,
        data=data,
        headers={"content-type": "application/json"} if data is not None else {},
        method=method,
    )
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            status = response.status
            body = response.read()
    except urllib.error.HTTPError as error:
        status = error.code
        body = error.read()
    assert status in expected, (path, status, body[:500])
    return status, body

def request_json(path, method="GET", payload=None, expected=(200,)):
    status, body = request_bytes(path, method=method, payload=payload, expected=expected)
    return status, json.loads(body) if body else None

def run_payload(message, session_key):
    return {
        "message": message,
        "sessionKey": session_key,
        "runtime": "external_cli",
        "adapter": "codex",
        "workDir": repo_dir,
        "allowWorkDirs": [repo_dir],
        "injectBifrostTools": False,
        "adapterConfig": {
            "executable": executable,
            "transport": "app_server",
            "sandbox": "read-only",
            "approvalPolicy": "never",
            "timeoutSecs": 45,
        },
    }

# Exercise the reusable Browser worker through the same Admin API used by IM
# messages. Both the parent and worker inherit the explicit E2E mock, so this
# validates process/spool/result isolation without opening a real browser.
_, browser_result = request_json(
    "/im-gateway/chat",
    method="POST",
    payload={
        "message": "BROWSER_WORKER_E2E",
        "sessionKey": "awi-browser-success",
        "runtime": "external_cli",
        "adapter": "chatgpt_web",
        "workDir": repo_dir,
        "allowWorkDirs": [repo_dir],
        "injectBifrostTools": False,
        "adapterConfig": {"timeoutSecs": 30},
    },
)
assert browser_result["status"] == "succeeded", browser_result
assert browser_result["response"], browser_result

_, result = request_json(
    "/im-gateway/chat",
    method="POST",
    payload=run_payload("WRITE_LARGE_RESULT", "awi-success"),
)
assert result["status"] == "succeeded", result
assert result["response"].startswith("BIFROST_AUX_JOB_OK:"), result["response"][:200]

_, jobs = request_json("/worker-jobs?kind=external_cli&limit=20")
job = next(item for item in jobs if item.get("logicalJobId") == "awi-success")
assert job["status"] == "succeeded", job
assert job["finishedAtMs"] >= job["startedAtMs"], job
_, events = request_json(f"/worker-jobs/{job['id']}/events")
assert isinstance(events, list) and events, events
_, artifacts = request_json(f"/worker-jobs/{job['id']}/artifacts")
by_name = {artifact["name"]: artifact for artifact in artifacts}
assert {"result", "stdout", "stderr", "normalized_events"} <= set(by_name), by_name

result_artifact = by_name["result"]
artifact_url = f"/worker-jobs/{job['id']}/artifacts/{result_artifact['artifactId']}"
status, _ = request_bytes(artifact_url + "?offset=0&limit=64", expected=(200,))
assert status == 200
status, _ = request_bytes(artifact_url + "?tail=64", expected=(200,))
assert status == 200
status, _ = request_bytes(
    artifact_url + f"?offset={result_artifact['sizeBytes'] + 1}&limit=1",
    expected=(416,),
)
assert status == 416
status, _ = request_bytes(artifact_url + "?limit=1048577", expected=(400,))
assert status == 400
status, _ = request_bytes(
    f"/worker-jobs/{job['id']}/artifacts/not-registered",
    expected=(404,),
)
assert status == 404

cancel_result = {}
def run_cancelled():
    try:
        cancel_result["response"] = request_json(
            "/im-gateway/chat",
            method="POST",
            payload=run_payload("WAIT_FOR_CANCEL", "awi-cancel"),
            expected=(200, 500),
        )
    except BaseException as error:
        cancel_result["error"] = repr(error)

thread = threading.Thread(target=run_cancelled, daemon=True)
thread.start()
cancel_job = None
deadline = time.monotonic() + 20
while time.monotonic() < deadline:
    _, jobs = request_json("/worker-jobs?kind=external_cli&limit=20")
    cancel_job = next(
        (
            item for item in jobs
            if item.get("logicalJobId") == "awi-cancel"
            and item["status"] in {"queued", "running"}
        ),
        None,
    )
    if cancel_job is not None:
        break
    time.sleep(0.1)
assert cancel_job is not None, jobs
status, accepted = request_json(
    f"/worker-jobs/{cancel_job['id']}/cancel",
    method="POST",
    expected=(202,),
)
assert status == 202 and accepted["accepted"] is True, accepted
thread.join(timeout=20)
assert not thread.is_alive(), cancel_result

deadline = time.monotonic() + 10
while time.monotonic() < deadline:
    _, terminal = request_json(f"/worker-jobs/{cancel_job['id']}")
    if terminal["status"] in {"cancelled", "failed"}:
        break
    time.sleep(0.1)
assert terminal["status"] == "cancelled", terminal
PY

kill -0 "$BIFROST_PID" 2>/dev/null || fail "main process exited after External CLI cancellation"
proxy_smoke after-external-cli

echo "[auxiliary-worker-isolation] CORE PASS (full human-test PASS also requires the documented ASR, Weixin, and Remote Invoke companion scripts)"
