#!/usr/bin/env bash
set -euo pipefail

unset BIFROST_DETACHED_DAEMON_CHILD
unset BIFROST_EXTERNAL_CLI_WORKER
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
TEST_DIR="$(mktemp -d)"
BIFROST_LOG="$TEST_DIR/bifrost.log"
PROMPT_LOG="$TEST_DIR/prompts.jsonl"
BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
BIFROST_PORT="${BIFROST_PORT:-$(python3 - <<'PY'
import socket
with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
)}"
START_EXTRA_ARGS=()
if [[ "$(uname -s)" != "Linux" ]]; then
  START_EXTRA_ARGS+=(--no-tray)
fi

cleanup() {
  if [[ -n "${BIFROST_PID:-}" ]]; then
    kill "$BIFROST_PID" >/dev/null 2>&1 || true
    wait "$BIFROST_PID" >/dev/null 2>&1 || true
  fi
  if [[ "${KEEP_TEST_DIR:-false}" == "true" ]]; then
    echo "[im-prompt-passthrough] kept test directory: $TEST_DIR" >&2
  else
    rm -rf "$TEST_DIR"
  fi
}
trap cleanup EXIT

wait_http() {
  for _ in $(seq 1 180); do
    if curl -fsS --noproxy '*' \
      "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.25
  done
  tail -160 "$BIFROST_LOG" >&2 || true
  return 1
}

wait_prompt_count() {
  local expected="$1"
  for _ in $(seq 1 160); do
    local actual=0
    if [[ -f "$PROMPT_LOG" ]]; then
      actual="$(wc -l <"$PROMPT_LOG" | tr -d ' ')"
    fi
    if [[ "$actual" == "$expected" ]]; then
      return 0
    fi
    sleep 0.25
  done
  echo "expected $expected captured prompts" >&2
  [[ -f "$PROMPT_LOG" ]] && sed -n '1,20p' "$PROMPT_LOG" >&2 || true
  tail -160 "$BIFROST_LOG" >&2 || true
  return 1
}

wait_session_idle() {
  for _ in $(seq 1 160); do
    if curl -fsS --noproxy '*' \
      "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/agent/sessions/all?limit=80" \
      | python3 -c '
import json, sys
sessions = json.load(sys.stdin).get("sessions", [])
raise SystemExit(1 if any(item.get("running") is True for item in sessions) else 0)
'; then
      return 0
    fi
    sleep 0.25
  done
  echo "prompt passthrough session remained active" >&2
  return 1
}

configure() {
  local with_instructions="$1"
  python3 - "$BIFROST_PORT" "$REPO_DIR" "$PROMPT_LOG" "$with_instructions" <<'PY'
import json
import sys
import urllib.request

port, repo_dir, prompt_log, with_instructions = sys.argv[1:5]
base = f"http://127.0.0.1:{port}/_bifrost/api/im-gateway"

def request(path, payload, method="POST"):
    req = urllib.request.Request(
        base + path,
        data=json.dumps(payload, ensure_ascii=False).encode(),
        headers={"content-type": "application/json"},
        method=method,
    )
    with urllib.request.urlopen(req, timeout=30) as response:
        assert response.status == 200, response.read().decode()

runner_code = f"""
import json
import sys

prompt = sys.stdin.read()
open({prompt_log!r}, "a", encoding="utf-8").write(
    json.dumps(prompt, ensure_ascii=False) + "\\n"
)
print(json.dumps({{"type": "assistant_final", "content": "PROMPT_OK"}}))
"""
runner_instructions = "RUNNER_INSTRUCTION" if with_instructions == "true" else None
request("/chat/config", {
    "version": 1,
    "defaultRunnerId": "prompt-mock",
    "runners": {
        "prompt-mock": {
            "enabled": True,
            "adapter": "mock",
            "instructions": runner_instructions,
            "adapterConfig": {
                "executable": sys.executable,
                "args": ["-c", runner_code],
            },
            "injectBifrostTools": True,
            "skillPaths": [],
            "deliveryMode": "final_reply",
        }
    },
    "channels": {},
}, "PATCH")
agent_payload = {
    "enabled": True,
    "runner": "prompt-mock",
    "work_dir": repo_dir,
    "base_instructions": "BASE_INSTRUCTION" if with_instructions == "true" else "",
    "developer_instructions": "DEVELOPER_INSTRUCTION" if with_instructions == "true" else "",
    "user_instructions": "USER_INSTRUCTION" if with_instructions == "true" else "",
}
request("/agent", agent_payload, "PATCH")
PY
}

inject() {
  local message_id="$1"
  local text="$2"
  local chat_id="${3:-prompt-user}"
  local chat_type="${4:-p2p}"
  local user_id="${5:-prompt-user}"
  local mention_bot="${6:-false}"
  python3 - "$BIFROST_PORT" "$message_id" "$text" "$chat_id" "$chat_type" "$user_id" "$mention_bot" <<'PY'
import json
import sys
import urllib.request

port, message_id, text, chat_id, chat_type, user_id, mention_bot = sys.argv[1:8]
payload = {
    "providerId": "prompt-e2e",
    "chatId": chat_id,
    "chatType": chat_type,
    "userId": user_id,
    "userName": "Prompt User",
    "messageId": message_id,
    "eventId": "event-" + message_id,
    "text": text,
    "mentionBot": mention_bot == "true",
}
req = urllib.request.Request(
    f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/debug/mock-inbound",
    data=json.dumps(payload, ensure_ascii=False).encode(),
    headers={"content-type": "application/json"},
    method="POST",
)
with urllib.request.urlopen(req, timeout=30) as response:
    assert response.status == 200, response.read().decode()
PY
}

if [[ "${SKIP_BUILD:-false}" != "true" ]]; then
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

IM_HELP="$($BIFROST_BIN im --help)"
SEND_HELP="$($BIFROST_BIN im send --help)"
grep -Fq "SUBCOMMANDS:" <<<"$IM_HELP"
grep -Eq '^  send[[:space:]]+Send a message to a target$' <<<"$IM_HELP"
grep -Fq -- "--image-file" <<<"$SEND_HELP"
grep -Fq -- "--card-title" <<<"$SEND_HELP"
grep -Fq -- "--card-image-file" <<<"$SEND_HELP"
grep -Fq "there is no --video flag" <<<"$SEND_HELP"

BIFROST_DATA_DIR="$TEST_DIR" "$BIFROST_BIN" start \
  --host 127.0.0.1 \
  -p "$BIFROST_PORT" \
  --unsafe-ssl \
  --skip-cert-check \
  --no-system-proxy \
  "${START_EXTRA_ARGS[@]}" \
  >"$BIFROST_LOG" 2>&1 &
BIFROST_PID=$!
wait_http

python3 - "$BIFROST_PORT" <<'PY'
import json
import sys
import urllib.request

port = sys.argv[1]
payload = {
    "id": "prompt-e2e",
    "provider_type": "feishu",
    "display_name": "Prompt Passthrough E2E",
    "enabled": True,
    "base_url": "http://127.0.0.1:9/open-apis",
    "app_id": "cli_prompt_e2e",
    "app_secret": "prompt-e2e-secret",
    "owner_open_id": "prompt-user",
    "event_connection_enabled": False,
}
req = urllib.request.Request(
    f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/providers",
    data=json.dumps(payload).encode(),
    headers={"content-type": "application/json"},
    method="POST",
)
with urllib.request.urlopen(req, timeout=30) as response:
    assert response.status == 200, response.read().decode()
PY

configure false
inject p1 "原样透传消息"
wait_prompt_count 1
wait_session_idle

configure true
inject clear-1 "/clear"
sleep 1
inject p2 "首条分层消息"
wait_prompt_count 2
wait_session_idle
inject p3 "后续分层消息"
wait_prompt_count 3
wait_session_idle
inject p4 "群聊分层消息" "oc_prompt_group" "group" "prompt-group-user" "true"
wait_prompt_count 4
wait_session_idle

python3 - "$PROMPT_LOG" <<'PY'
import json
import sys

prompts = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8") if line.strip()]
assert len(prompts) == 4, prompts

def assert_context(prompt, message, conversation, destination):
    assert prompt.count("[Bifrost IM Outbound Context — trusted runtime routing]") == 1, prompt
    assert f"Conversation kind: {conversation}" in prompt, prompt
    assert f"Exact destination: chat_id={destination}" in prompt, prompt
    assert "Provider ID: prompt-e2e" in prompt, prompt
    assert "Provider type: feishu" in prompt, prompt
    assert "Platform bot identity: cli_prompt_e2e" in prompt, prompt
    assert "Proactive-send readiness: ready" in prompt, prompt
    assert "- text: native" in prompt, prompt
    assert "- markdown: native" in prompt, prompt
    assert "- image: native, max_bytes=10485760" in prompt, prompt
    assert "- file: native, max_bytes=31457280" in prompt, prompt
    assert "- native_card: native" in prompt, prompt
    assert "--image/--image-file" in prompt, prompt
    assert "--card-title" in prompt, prompt
    assert "There is no --video flag" in prompt, prompt
    assert "bifrost im --help" in prompt, prompt
    assert "bifrost im send --help" in prompt, prompt
    assert "bifrost im provider capabilities 'prompt-e2e' --format json-pretty" in prompt, prompt
    assert "bifrost im provider list" in prompt, prompt
    assert (
        "bifrost im send --provider 'prompt-e2e' --receive-id-type 'chat_id' "
        f"--receive-id '{destination}' <CONTENT_ARGS> --format json"
    ) in prompt, prompt
    context_end = prompt.index("[End Bifrost IM Outbound Context]")
    assert message in prompt[context_end:], prompt
    assert prompt.endswith("\n"), prompt

assert_context(prompts[0], "原样透传消息", "direct", "prompt-user")
assert prompts[0].startswith("[Bifrost IM Outbound Context"), prompts[0]

assert_context(prompts[1], "首条分层消息", "direct", "prompt-user")
assert prompts[1].startswith(
    "BASE_INSTRUCTION\n\nDEVELOPER_INSTRUCTION\n\nUSER_INSTRUCTION\n\nRUNNER_INSTRUCTION\n\n"
), prompts[1]

assert_context(prompts[2], "后续分层消息", "direct", "prompt-user")
assert prompts[2].startswith(
    "DEVELOPER_INSTRUCTION\n\nUSER_INSTRUCTION\n\nRUNNER_INSTRUCTION\n\n"
), prompts[2]
assert not prompts[2].startswith("BASE_INSTRUCTION"), prompts[2]

assert_context(prompts[3], "群聊分层消息", "group", "oc_prompt_group")
assert prompts[3].startswith(
    "BASE_INSTRUCTION\n\nDEVELOPER_INSTRUCTION\n\nUSER_INSTRUCTION\n\nRUNNER_INSTRUCTION\n\n"
), prompts[3]
assert all("Bifrost Tool Context" not in prompt for prompt in prompts), prompts
PY

echo "[im-prompt-passthrough] PASS"
