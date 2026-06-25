#!/usr/bin/env bash
set -euo pipefail

: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_DIR"

TEST_DIR="$(mktemp -d "$REPO_DIR/.bifrost-e2e-runner-image.XXXXXX")"
BIFROST_LOG="$TEST_DIR/bifrost.log"
BIFROST_BIN="${BIFROST_BIN:-}"
RUNNER_ID="mock-image"
CHAT_SESSION_KEY="web-image-session-e2e"
CALLER_SESSION_KEY="runner-call-image-parent-e2e"
FINAL_MARKER="BIFROST_IMAGE_PATH_OK"

if [[ -z "${BIFROST_PORT:-}" ]]; then
  BIFROST_PORT="$(python3 - <<'PY'
import socket

with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
)"
fi

cleanup() {
  if [[ -n "${BIFROST_PID:-}" ]]; then
    kill "$BIFROST_PID" >/dev/null 2>&1 || true
    wait "$BIFROST_PID" >/dev/null 2>&1 || true
  fi
  rm -rf "$TEST_DIR"
}
trap cleanup EXIT

wait_http() {
  local url="$1"
  local label="$2"
  for _ in $(seq 1 180); do
    if ! kill -0 "$BIFROST_PID" >/dev/null 2>&1; then
      echo "[im-gateway-external-runner-image-input] $label exited before becoming ready" >&2
      [[ -f "$BIFROST_LOG" ]] && tail -160 "$BIFROST_LOG" >&2 || true
      return 1
    fi
    if curl -fsS --noproxy '*' "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.25
  done
  echo "[im-gateway-external-runner-image-input] $label did not become ready" >&2
  [[ -f "$BIFROST_LOG" ]] && tail -160 "$BIFROST_LOG" >&2 || true
  return 1
}

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
  echo "[im-gateway-external-runner-image-input] skipping build, using $BIFROST_BIN"
else
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
  echo "[im-gateway-external-runner-image-input] building bifrost"
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

echo "[im-gateway-external-runner-image-input] starting bifrost on $BIFROST_PORT"
BIFROST_DATA_DIR="$TEST_DIR" "$BIFROST_BIN" start \
  --host 127.0.0.1 \
  -p "$BIFROST_PORT" \
  --unsafe-ssl \
  --skip-cert-check \
  --no-system-proxy \
  >"$BIFROST_LOG" 2>&1 &
BIFROST_PID=$!
wait_http "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address" "bifrost"

python3 - "$BIFROST_PORT" "$TEST_DIR" "$RUNNER_ID" "$FINAL_MARKER" "$CHAT_SESSION_KEY" "$CALLER_SESSION_KEY" <<'PY'
import base64
import json
import os
import pathlib
import sys
import urllib.request

port, test_dir, runner_id, final_marker, chat_session_key, caller_session_key = sys.argv[1:7]
base_url = f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/chat"
test_path = pathlib.Path(test_dir)
prompt_capture = test_path / "captured-prompts.txt"
script = (
    'input="$(cat)"; '
    'printf "%s\\n---PROMPT-END---\\n" "$input" >> "$BIFROST_CAPTURE_PROMPTS"; '
    f"printf '%s\\n' '{{\"type\":\"assistant_final\",\"content\":\"{final_marker}\"}}'"
)


def request_json(path, payload, timeout=60):
    req = urllib.request.Request(
        f"{base_url}{path}",
        data=json.dumps(payload).encode("utf-8"),
        headers={"content-type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        assert resp.status == 200, resp.status
        body = resp.read().decode("utf-8")
    return body


def patch_config():
    payload = {
        "version": 1,
        "defaultRunnerId": runner_id,
        "runners": {
            runner_id: {
                "enabled": True,
                "adapter": "codex",
                "adapterConfig": {
                    "executable": "/bin/sh",
                    "args": ["-c", script],
                    "env": {"BIFROST_CAPTURE_PROMPTS": str(prompt_capture)},
                    "timeoutSecs": 30,
                },
                "injectBifrostTools": False,
                "skillPaths": [],
                "deliveryMode": "final_reply",
            }
        },
        "channels": {},
    }
    req = urllib.request.Request(
        f"{base_url}/config",
        data=json.dumps(payload).encode("utf-8"),
        headers={"content-type": "application/json"},
        method="PATCH",
    )
    with urllib.request.urlopen(req, timeout=30) as resp:
        body = resp.read().decode("utf-8")
        assert resp.status == 200, body


def stream(path, payload):
    req = urllib.request.Request(
        f"{base_url}{path}",
        data=json.dumps(payload).encode("utf-8"),
        headers={"content-type": "application/json"},
        method="POST",
    )
    events = []
    with urllib.request.urlopen(req, timeout=120) as resp:
        assert resp.status == 200, resp.status
        for raw_line in resp:
            line = raw_line.decode("utf-8").strip()
            if line:
                events.append(json.loads(line))
    return events


def final_event(events, event_type):
    matches = [event for event in events if event.get("eventType") == event_type]
    assert len(matches) == 1, events
    event = matches[0]
    assert event.get("status") == "succeeded", event
    assert final_marker in (event.get("response") or ""), event
    return event


def assert_run_image(run_id, expected_name, expected_bytes):
    run_dir = test_path / "agent" / "im_gateway" / "chat_runs" / run_id
    prompt = (run_dir / "prompt.md").read_text(encoding="utf-8")
    assert "## Attached Images" in prompt, prompt
    assert "/attachments/session-" in prompt, prompt

    result = json.loads((run_dir / "result.json").read_text(encoding="utf-8"))
    attachments = json.loads(result["metadata"]["attachments.images"])
    assert len(attachments) == 1, attachments
    image = attachments[0]
    assert image["name"] == expected_name, image
    assert image["mimeType"] == "image/png", image
    assert image["sizeBytes"] == len(expected_bytes), image
    image_path = pathlib.Path(image["path"])
    assert image_path.is_absolute(), image
    assert image_path.exists(), image_path
    assert b"/attachments/session-" in str(image_path).encode("utf-8"), image_path
    assert image_path.read_bytes() == expected_bytes, image_path
    assert str(image_path) in prompt, prompt
    return image_path


patch_config()

chat_events = stream(
    "/stream",
    {
        "message": "please inspect attached image",
        "providerId": "web-e2e",
        "runnerId": runner_id,
        "sessionKey": chat_session_key,
        "images": [
            {
                "mimeType": "image/png",
                "data": base64.b64encode(b"hello-image").decode("ascii"),
                "name": "hello.png",
            }
        ],
    },
)
chat_finished = final_event(chat_events, "run_finished")
chat_path = assert_run_image(chat_finished["runId"], "hello.png", b"hello-image")

runner_events = stream(
    "/runner-calls/stream",
    {
        "callerSessionKey": caller_session_key,
        "callerRunnerId": "Bifrost V4",
        "callerRunnerAdapter": "builtin_agent",
        "targetRunnerId": runner_id,
        "message": "",
        "images": [
            {
                "mimeType": "image/png",
                "data": base64.b64encode(b"runner-call-image").decode("ascii"),
                "name": "runner.png",
            }
        ],
        "callerMessages": [],
    },
)
runner_finished = final_event(runner_events, "runner_call_finished")
runner_path = assert_run_image(runner_finished["runId"], "runner.png", b"runner-call-image")

assert chat_path != runner_path, (chat_path, runner_path)
capture = prompt_capture.read_text(encoding="utf-8")
assert capture.count("## Attached Images") >= 2, capture
assert str(chat_path) in capture, capture
assert str(runner_path) in capture, capture

print("[im-gateway-external-runner-image-input] PASS")
print(f"chat_run={chat_finished['runId']} chat_image={chat_path}")
print(f"runner_call_run={runner_finished['runId']} runner_image={runner_path}")
PY
