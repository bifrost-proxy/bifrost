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
TRAEX_RUNNER_ID="mock-image-traex"
FILE_RUNNER_ID="mock-file"
CHAT_SESSION_KEY="web-image-session-e2e"
TRAEX_SESSION_KEY="web-image-session-traex-e2e"
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

python3 - "$BIFROST_PORT" "$TEST_DIR" "$RUNNER_ID" "$TRAEX_RUNNER_ID" "$FILE_RUNNER_ID" "$FINAL_MARKER" "$CHAT_SESSION_KEY" "$TRAEX_SESSION_KEY" "$CALLER_SESSION_KEY" <<'PY'
import base64
import json
import os
import pathlib
import sys
import urllib.parse
import urllib.request

port, test_dir, runner_id, traex_runner_id, file_runner_id, final_marker, chat_session_key, traex_session_key, caller_session_key = sys.argv[1:10]
base_url = f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/chat"
agent_base_url = f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/agent"
test_path = pathlib.Path(test_dir)
prompt_capture = test_path / "captured-prompts.txt"
evil_attachment_dir = test_path / "evil-attachments"
script = (
    'input="$(cat)"; '
    'printf "%s\\n---PROMPT-END---\\n" "$input" >> "$BIFROST_CAPTURE_PROMPTS"; '
    "printf '%s\\n' '{\"type\":\"thread.started\",\"thread_id\":\"thread-mock-image\"}'; "
    "printf '%s\\n' '{\"type\":\"item.started\",\"item\":{\"id\":\"tool_1\",\"type\":\"command_execution\",\"command\":\"pwd\"}}'; "
    "printf '%s\\n' '{\"type\":\"item.completed\",\"item\":{\"id\":\"tool_1\",\"type\":\"command_execution\",\"command\":\"pwd\",\"aggregated_output\":\"/tmp/mock\\\\n\",\"exit_code\":0,\"status\":\"completed\"}}'; "
    "printf '%s\\n' '{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":100,\"cached_input_tokens\":10,\"output_tokens\":5,\"reasoning_output_tokens\":2}}'; "
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


def get_json(url, timeout=60):
    with urllib.request.urlopen(url, timeout=timeout) as resp:
        assert resp.status == 200, resp.status
        return json.loads(resp.read().decode("utf-8"))


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
            },
            traex_runner_id: {
                "enabled": True,
                "adapter": "traex",
                "adapterConfig": {
                    "executable": "/bin/sh",
                    "args": ["-c", script],
                    "env": {"BIFROST_CAPTURE_PROMPTS": str(prompt_capture)},
                    "timeoutSecs": 30,
                },
                "injectBifrostTools": False,
                "skillPaths": [],
                "deliveryMode": "final_reply",
            },
            file_runner_id: {
                "enabled": True,
                "adapter": "mock_file",
                "adapterConfig": {
                    "executable": "/bin/sh",
                    "args": ["-c", script],
                    "env": {"BIFROST_CAPTURE_PROMPTS": str(prompt_capture)},
                    "timeoutSecs": 30,
                },
                "injectBifrostTools": False,
                "skillPaths": [],
                "deliveryMode": "final_reply",
            },
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


def assert_run_images(run_id, expected_images):
    run_dir = test_path / "agent" / "im_gateway" / "chat_runs" / run_id
    prompt = json.loads((run_dir / "prompt.md").read_text(encoding="utf-8"))
    assert prompt["_bifrost_compacted"] is True, prompt
    assert prompt["image_count"] == len(expected_images), prompt

    result = json.loads((run_dir / "result.json").read_text(encoding="utf-8"))
    attachments = json.loads(result["metadata"]["attachments.images"])
    assert len(attachments) == len(expected_images), attachments
    paths = []
    for image, expected in zip(attachments, expected_images):
        expected_name, expected_mime, expected_bytes = expected
        assert image["name"] == expected_name, image
        assert image["mimeType"] == expected_mime, image
        assert image["sizeBytes"] == len(expected_bytes), image
        image_path = pathlib.Path(image["path"])
        assert image_path.is_absolute(), image
        assert image_path.exists(), image_path
        assert b"/attachments/session-" in str(image_path).encode("utf-8"), image_path
        assert image_path.read_bytes() == expected_bytes, image_path
        paths.append(image_path)
    return paths


def assert_run_files(run_id, expected_files):
    run_dir = test_path / "agent" / "im_gateway" / "chat_runs" / run_id
    prompt = json.loads((run_dir / "prompt.md").read_text(encoding="utf-8"))
    assert prompt["_bifrost_compacted"] is True, prompt
    assert prompt["file_count"] == len(expected_files), prompt

    result = json.loads((run_dir / "result.json").read_text(encoding="utf-8"))
    attachments = json.loads(result["metadata"]["attachments.files"])
    assert len(attachments) == len(expected_files), attachments
    paths = []
    for attachment, expected in zip(attachments, expected_files):
        expected_name, expected_mime, expected_bytes = expected
        assert attachment["name"] == expected_name, attachment
        assert attachment["mimeType"] == expected_mime, attachment
        assert attachment["sizeBytes"] == len(expected_bytes), attachment
        attachment_path = pathlib.Path(attachment["path"])
        assert attachment_path.is_absolute(), attachment
        assert attachment_path.exists(), attachment_path
        assert attachment_path.parent.name == "files", attachment_path
        assert attachment_path.read_bytes() == expected_bytes, attachment_path
        paths.append(attachment_path)
    return paths


def assert_run_image(run_id, expected_name, expected_bytes):
    return assert_run_images(run_id, [(expected_name, "image/png", expected_bytes)])[0]


def assert_runner_metadata(run_id, expected_adapter, expected_attachment_count=1):
    run_dir = test_path / "agent" / "im_gateway" / "chat_runs" / run_id
    result = json.loads((run_dir / "result.json").read_text(encoding="utf-8"))
    metadata = result["metadata"]
    required_keys = [
        "cli.executable",
        "cli.argCount",
        "cli.argFlags",
        "cli.version",
        "runner.adapter",
        "prompt.estimatedTokens",
        "attachments.count",
        "attachments.totalBytes",
        "io.stdoutBytes",
        "io.stderrBytes",
        "timing.totalDurationMs",
        "tools.count",
        "tools.totalDurationMs",
        "resume.requested",
        "usageInputTokens",
        "usageOutputTokens",
        "usageTotalTokens",
    ]
    for key in required_keys:
        assert key in metadata, (key, metadata)
    assert metadata["runner.adapter"] == expected_adapter, metadata
    assert metadata["attachments.count"] == str(expected_attachment_count), metadata
    assert metadata["tools.count"] == "1", metadata
    assert int(metadata["prompt.estimatedTokens"]) > 0, metadata
    assert int(metadata["io.stdoutBytes"]) > 0, metadata
    assert metadata["resume.requested"] in {"true", "false"}, metadata
    if metadata["resume.requested"] == "true":
        assert metadata.get("resume.requestedThreadId"), metadata

    normalized = [
        json.loads(line)
        for line in (run_dir / "normalized_events.jsonl").read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    tool_finished = [
        event for event in normalized
        if event.get("eventType") == "tool_finished" or event.get("event_type") == "tool_finished"
    ]
    assert tool_finished, normalized
    assert any("durationMs" in event.get("raw", {}) for event in tool_finished), tool_finished
    return metadata


def create_im_provider(provider_id, owner_id, runner):
    payload = {
        "id": provider_id,
        "provider_type": "feishu",
        "display_name": "Attachment E2E",
        "enabled": True,
        "app_id": "cli_attachment_e2e",
        "owner_open_id": owner_id,
        "event_connection_enabled": False,
        "agent_config": {"runner": runner},
    }
    req = urllib.request.Request(
        f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/providers",
        data=json.dumps(payload).encode("utf-8"),
        headers={"content-type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=30) as resp:
        body = resp.read().decode("utf-8")
        assert resp.status == 200, body


def send_mock_inbound_file(provider_id, owner_id, chat_id, file_name, mime_type, content):
    payload = {
        "providerId": provider_id,
        "userId": owner_id,
        "chatId": chat_id,
        "text": "",
        "files": [
            {
                "fileKey": "mock-file-report",
                "name": file_name,
                "mimeType": mime_type,
                "data": base64.b64encode(content).decode("ascii"),
            }
        ],
    }
    req = urllib.request.Request(
        f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/debug/mock-inbound",
        data=json.dumps(payload).encode("utf-8"),
        headers={"content-type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=30) as resp:
        body = resp.read().decode("utf-8")
        assert resp.status == 200, body


def wait_for_latest_run_after(before):
    runs_dir = test_path / "agent" / "im_gateway" / "chat_runs"
    for _ in range(240):
        if runs_dir.exists():
            candidates = [
                path
                for path in runs_dir.iterdir()
                if path.is_dir() and path.name not in before and (path / "result.json").exists()
            ]
            if candidates:
                return max(candidates, key=lambda path: path.stat().st_mtime).name
        import time

        time.sleep(0.1)
    raise AssertionError("timed out waiting for IM inbound external runner run")


def assert_session_detail_metadata(session_key, expected_metadata):
    encoded = urllib.parse.quote(session_key, safe="")
    detail = get_json(f"{agent_base_url}/sessions/{encoded}")
    metadata = detail.get("metadata") or {}
    for key in ["cli.executable", "runner.adapter", "prompt.estimatedTokens", "attachments.count", "tools.count"]:
        assert metadata.get(key) == expected_metadata.get(key), (key, detail, expected_metadata)
    return detail


patch_config()

chat_events = stream(
    "/stream",
    {
        "message": "please inspect attached image",
        "providerId": "web-e2e",
        "runnerId": runner_id,
        "sessionKey": chat_session_key,
        "params": {"attachmentBaseDir": str(evil_attachment_dir)},
        "images": [
            {
                "mimeType": "image/png",
                "data": base64.b64encode(b"hello-image").decode("ascii"),
                "name": "hello.png",
            },
            {
                "mimeType": "image/jpeg",
                "data": base64.b64encode(b"hello-image-two").decode("ascii"),
                "name": "hello-two.jpg",
            }
        ],
    },
)
chat_finished = final_event(chat_events, "run_finished")
chat_paths = assert_run_images(
    chat_finished["runId"],
    [
        ("hello.png", "image/png", b"hello-image"),
        ("hello-two.jpg", "image/jpeg", b"hello-image-two"),
    ],
)
chat_path = chat_paths[0]
second_initial_chat_path = chat_paths[1]
assert not evil_attachment_dir.exists(), evil_attachment_dir
assert_runner_metadata(chat_finished["runId"], "codex", expected_attachment_count=2)

second_chat_events = stream(
    "/stream",
    {
        "message": "please inspect a second attached image",
        "providerId": "web-e2e",
        "runnerId": runner_id,
        "sessionKey": chat_session_key,
        "images": [
            {
                "mimeType": "image/png",
                "data": base64.b64encode(b"second-hello-image").decode("ascii"),
                "name": "second-hello.png",
            }
        ],
    },
)
second_chat_finished = final_event(second_chat_events, "run_finished")
second_chat_path = assert_run_image(
    second_chat_finished["runId"],
    "second-hello.png",
    b"second-hello-image",
)
second_metadata = assert_runner_metadata(second_chat_finished["runId"], "codex")
assert_session_detail_metadata(chat_session_key, second_metadata)
assert chat_path != second_chat_path, (chat_path, second_chat_path)
assert second_initial_chat_path != second_chat_path, (second_initial_chat_path, second_chat_path)
assert chat_path.read_bytes() == b"hello-image", chat_path
assert second_initial_chat_path.read_bytes() == b"hello-image-two", second_initial_chat_path

traex_events = stream(
    "/stream",
    {
        "message": "please inspect attached image with trae",
        "providerId": "web-e2e",
        "runnerId": traex_runner_id,
        "sessionKey": traex_session_key,
        "images": [
            {
                "mimeType": "image/png",
                "data": base64.b64encode(b"traex-image").decode("ascii"),
                "name": "traex.png",
            }
        ],
    },
)
traex_finished = final_event(traex_events, "run_finished")
traex_path = assert_run_image(traex_finished["runId"], "traex.png", b"traex-image")
traex_metadata = assert_runner_metadata(traex_finished["runId"], "traex")
assert_session_detail_metadata(traex_session_key, traex_metadata)

runner_events = stream(
    "/runner-calls/stream",
    {
        "callerSessionKey": caller_session_key,
        "callerRunnerId": "Codex",
        "callerRunnerAdapter": "Codex",
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
runner_metadata = assert_runner_metadata(runner_finished["runId"], "codex")
assert_session_detail_metadata(caller_session_key, runner_metadata)

provider_id = "file-inbound-provider"
owner_id = "file-inbound-owner"
runs_dir = test_path / "agent" / "im_gateway" / "chat_runs"
existing_runs = {path.name for path in runs_dir.iterdir()} if runs_dir.exists() else set()
create_im_provider(provider_id, owner_id, file_runner_id)
send_mock_inbound_file(
    provider_id,
    owner_id,
    f"chat-{provider_id}",
    "../report final.md",
    "text/markdown",
    b"# Report\n\nhello from file\n",
)
file_run_id = wait_for_latest_run_after(existing_runs)
file_paths = assert_run_files(
    file_run_id,
    [("../report final.md", "text/markdown", b"# Report\n\nhello from file\n")],
)
file_metadata = json.loads(
    (test_path / "agent" / "im_gateway" / "chat_runs" / file_run_id / "result.json")
    .read_text(encoding="utf-8")
)["metadata"]
assert file_metadata["runner.adapter"] == "mock_file", file_metadata
assert file_metadata["attachments.fileCount"] == "1", file_metadata
assert file_metadata["attachments.imageCount"] == "0", file_metadata
assert file_metadata["attachments.count"] == "1", file_metadata
file_path = file_paths[0]

assert chat_path != runner_path, (chat_path, runner_path)
assert chat_path != traex_path, (chat_path, traex_path)
capture = prompt_capture.read_text(encoding="utf-8")
assert capture.count("## Attached Images") >= 4, capture
assert capture.count("## Attached Files") >= 1, capture
assert str(chat_path) in capture, capture
assert str(second_initial_chat_path) in capture, capture
assert str(second_chat_path) in capture, capture
assert str(traex_path) in capture, capture
assert str(runner_path) in capture, capture
assert str(file_path) in capture, capture

print("[im-gateway-external-runner-image-input] PASS")
print(f"chat_run={chat_finished['runId']} chat_images={chat_path},{second_initial_chat_path}")
print(f"second_chat_run={second_chat_finished['runId']} second_chat_image={second_chat_path}")
print(f"traex_run={traex_finished['runId']} traex_image={traex_path}")
print(f"runner_call_run={runner_finished['runId']} runner_image={runner_path}")
print(f"file_inbound_run={file_run_id} file_attachment={file_path}")
PY
