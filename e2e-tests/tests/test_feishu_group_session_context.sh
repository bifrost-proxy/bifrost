#!/usr/bin/env bash
set -euo pipefail

unset BIFROST_DETACHED_DAEMON_CHILD
unset BIFROST_EXTERNAL_CLI_WORKER
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
TEST_DIR="$(mktemp -d)"
BIFROST_LOG="$TEST_DIR/bifrost.log"
PROMPT_LOG="$TEST_DIR/group-prompts.jsonl"
RUN_LOG="$TEST_DIR/runner-events.jsonl"
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
    echo "[feishu-group-session] kept test directory: $TEST_DIR" >&2
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

if [[ "${SKIP_BUILD:-false}" != "true" ]]; then
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

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

python3 - "$BIFROST_PORT" "$REPO_DIR" "$PROMPT_LOG" "$RUN_LOG" <<'PY'
import json
import sys
import urllib.request

port, repo_dir, prompt_log, run_log = sys.argv[1:5]
base = f"http://127.0.0.1:{port}/_bifrost/api/im-gateway"

def request(path, payload, method="POST"):
    req = urllib.request.Request(
        base + path,
        data=json.dumps(payload).encode(),
        headers={"content-type": "application/json"},
        method=method,
    )
    with urllib.request.urlopen(req, timeout=30) as response:
        assert response.status == 200, response.read().decode()

runner_code = (
    "import json,os,sys,time; "
    "prompt=sys.stdin.read(); "
    f"open({prompt_log!r},'a',encoding='utf-8').write(json.dumps(prompt,ensure_ascii=False)+'\\n'); "
    f"open({run_log!r},'a',encoding='utf-8').write(json.dumps({{'phase':'start','pid':os.getpid(),'at':time.time()}})+'\\n'); "
    "time.sleep(1); "
    f"open({run_log!r},'a',encoding='utf-8').write(json.dumps({{'phase':'finish','pid':os.getpid(),'at':time.time()}})+'\\n'); "
    "print(json.dumps({'type':'assistant_final','content':'GROUP_OK'}))"
)
request("/chat/config", {
    "version": 1,
    "defaultRunnerId": "group-mock",
    "runners": {
        "group-mock": {
            "enabled": True,
            "adapter": "mock",
            "adapterConfig": {
                "executable": sys.executable,
                "args": ["-c", runner_code],
            },
            "injectBifrostTools": False,
            "skillPaths": [],
            "deliveryMode": "final_reply",
        }
    },
    "channels": {},
}, "PATCH")
request("/agent", {
    "enabled": True,
    "runner": "group-mock",
    "work_dir": repo_dir,
}, "PATCH")
request("/providers", {
    "id": "feishu-group-e2e",
    "provider_type": "feishu",
    "display_name": "Feishu Group E2E",
    "enabled": True,
    "base_url": "http://127.0.0.1:9/open-apis",
    "app_id": "cli_group_e2e",
    "app_secret": "group-e2e-secret",
    "owner_open_id": "owner-only-in-p2p",
    "event_connection_enabled": False,
})
PY

inject() {
  local chat_id="$1"
  local user_id="$2"
  local user_name="$3"
  local message_id="$4"
  local text="$5"
  local mention_bot="${6:-false}"
  local chat_type="${7:-group}"
  python3 - "$BIFROST_PORT" "$chat_id" "$user_id" "$user_name" "$message_id" "$text" "$mention_bot" "$chat_type" <<'PY'
import json
import sys
import urllib.request

port, chat_id, user_id, user_name, message_id, text, mention_bot, chat_type = sys.argv[1:9]
payload = {
    "providerId": "feishu-group-e2e",
    "chatId": chat_id,
    "chatType": chat_type,
    "chatName": {"chat-alpha": "Alpha 发布群", "chat-beta": "Beta 讨论群"}.get(chat_id),
    "userId": user_id,
    "userName": user_name,
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

inject chat-alpha user-alice Alice a1 "先讨论发布窗口"
inject chat-alpha user-bob Bob a2 "我建议周三晚上"
sleep 1
[[ ! -f "$PROMPT_LOG" ]] || [[ ! -s "$PROMPT_LOG" ]]

inject chat-alpha user-alice Alice a3 "@_user_1 请总结刚才的讨论" true
wait_prompt_count 1
sleep 1.25

inject chat-alpha user-bob Bob a4 "/cwd $REPO_DIR"
inject chat-alpha user-bob Bob a5 "/runner group-mock"
inject chat-alpha user-bob Bob a6 "/status"
sleep 1
wait_prompt_count 1
inject chat-alpha user-bob Bob a7 "补充：需要回滚预案"
inject chat-alpha user-bob Bob a8 "/g 继续给出行动项"
wait_prompt_count 2
sleep 1.25

# Exact command boundaries match direct messages: /help is a command, while
# `/help extra` falls through to the model and therefore receives group context.
inject chat-alpha user-bob Bob a9 "/help extra"
wait_prompt_count 3
sleep 1.25

inject chat-beta user-carol Carol b1 "另一个群的背景"
inject chat-beta user-carol Carol b2 "@_user_1 给出一句结论" true
wait_prompt_count 4

# While the first Beta turn is still running, an ambient message must survive
# a rejected /clear and be included in the subsequently accepted queued turn.
inject chat-beta user-dave Dave b3 "忙时补充：保留这条背景"
inject chat-beta user-carol Carol b4 "/clear"
inject chat-beta user-carol Carol b5 "/q 排队后的结论"
wait_prompt_count 5
sleep 1.25

# A long-running group task must not own the provider receiver. Two groups and
# the provider owner's direct-message session start separate worker processes
# before any of the three workers finishes.
inject chat-alpha user-alice Alice a10 "@_user_1 并发检查 Alpha" true
inject chat-beta user-carol Carol b6 "@_user_1 并发检查 Beta" true
inject direct-owner owner-only-in-p2p Owner p1 "并发检查私聊" false p2p
wait_prompt_count 8
for _ in $(seq 1 160); do
  if [[ -f "$RUN_LOG" ]] && [[ "$(wc -l <"$RUN_LOG" | tr -d ' ')" == "16" ]]; then
    break
  fi
  sleep 0.25
done
[[ -f "$RUN_LOG" ]]
[[ "$(wc -l <"$RUN_LOG" | tr -d ' ')" == "16" ]]

python3 - "$PROMPT_LOG" "$RUN_LOG" "$TEST_DIR/admin/im_group_context.db" "$REPO_DIR" <<'PY'
import json
import pathlib
import sqlite3
import sys

prompt_path, run_path, db_path, repo_dir = sys.argv[1:5]
prompts = [json.loads(line) for line in open(prompt_path, encoding="utf-8") if line.strip()]
assert len(prompts) == 8, prompts
first, second, slash_fallback, third, queued = prompts[:5]
concurrent_prompts = prompts[5:]

assert "群名称：Alpha 发布群" in first, first
assert "群 ID：chat-alpha" in first, first
assert "先讨论发布窗口" in first and "我建议周三晚上" in first, first
assert "<at id=user-alice>Alice</at>：先讨论发布窗口" in first, first
assert "<at id=user-bob>Bob</at>：我建议周三晚上" in first, first
assert "<at id=user-alice>Alice</at>：请总结刚才的讨论" in first, first
assert first.count("请总结刚才的讨论") == 1, first
for internal_field in (
    "provider_id", "session_key", "message_id", "sender_open_id",
    "sender_type", "tenant_key", "attachment_count", "context_range",
):
    assert internal_field not in first, (internal_field, first)

assert "先讨论发布窗口" not in second, second
assert "/cwd" not in second and "/runner group-mock" not in second, second
assert "/status" not in second and "补充：需要回滚预案" in second, second
assert "<at id=user-bob>Bob</at>：继续给出行动项" in second, second
assert "群名称：" not in second and "群 ID：" not in second, second

assert "<at id=user-bob>Bob</at>：/help extra" in slash_fallback, slash_fallback
assert "先讨论发布窗口" not in slash_fallback, slash_fallback
assert "群名称：" not in slash_fallback and "群 ID：" not in slash_fallback, slash_fallback

assert "群名称：Beta 讨论群" in third, third
assert "群 ID：chat-beta" in third, third
assert "另一个群的背景" in third, third
assert "chat-alpha" not in third, third

assert "忙时补充：保留这条背景" in queued, queued
assert "<at id=user-carol>Carol</at>：排队后的结论" in queued, queued
assert "/clear" not in queued, queued
assert "群名称：" not in queued and "群 ID：" not in queued, queued

for expected in ("并发检查 Alpha", "并发检查 Beta", "并发检查私聊"):
    assert sum(expected in prompt for prompt in concurrent_prompts) == 1, concurrent_prompts

runner_events = [json.loads(line) for line in open(run_path, encoding="utf-8") if line.strip()]
assert len(runner_events) == 16, runner_events
concurrent_events = runner_events[-6:]
assert [event["phase"] for event in concurrent_events[:3]] == ["start"] * 3, concurrent_events
assert len({event["pid"] for event in concurrent_events[:3]}) == 3, concurrent_events
assert all(event["phase"] == "finish" for event in concurrent_events[3:]), concurrent_events

connection = sqlite3.connect(db_path)
bindings = connection.execute(
    "SELECT chat_id, session_key, last_assigned_seq, chat_name, work_dir, runner_id "
    "FROM im_group_bindings ORDER BY chat_id"
).fetchall()
assert len(bindings) == 2, bindings
assert bindings[0][0] == "chat-alpha" and bindings[1][0] == "chat-beta", bindings
assert bindings[0][1] != bindings[1][1], bindings
assert bindings[0][3] == "Alpha 发布群" and bindings[1][3] == "Beta 讨论群", bindings
assert bindings[0][4] == str(pathlib.Path(repo_dir).resolve()), bindings
assert bindings[0][5] == "group-mock", bindings
messages = connection.execute(
    "SELECT chat_id, COUNT(*) FROM im_group_messages GROUP BY chat_id ORDER BY chat_id"
).fetchall()
assert messages == [("chat-alpha", 10), ("chat-beta", 6)], messages
beta_turns = connection.execute(
    "SELECT status FROM im_group_turns WHERE chat_id = 'chat-beta' ORDER BY created_at"
).fetchall()
assert beta_turns == [("completed",), ("completed",), ("completed",)], beta_turns
PY

echo "[feishu-group-session] PASS"
