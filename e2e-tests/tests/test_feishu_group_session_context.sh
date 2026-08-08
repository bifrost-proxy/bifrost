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
CONCURRENT_RELEASE="$TEST_DIR/concurrent-release"
FEISHU_MOCK_PORT="$(python3 - <<'PY'
import socket
with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
)"
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
  if [[ -n "${FEISHU_MOCK_PID:-}" ]]; then
    kill "$FEISHU_MOCK_PID" >/dev/null 2>&1 || true
    wait "$FEISHU_MOCK_PID" >/dev/null 2>&1 || true
  fi
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

python3 - "$FEISHU_MOCK_PORT" <<'PY' &
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port = int(sys.argv[1])

class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def send_json(self, payload):
        body = json.dumps(payload, ensure_ascii=False).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        if self.path.endswith("/auth/v3/tenant_access_token/internal"):
            self.send_json({"code": 0, "tenant_access_token": "e2e-token", "expire": 7200})
        else:
            self.send_json({"code": 0})

    def do_GET(self):
        path = self.path.split("?", 1)[0]
        if path.endswith("/bot/v3/info"):
            self.send_json({"code": 0, "bot": {"open_id": "ou_bot", "app_name": "Bifrost"}})
        elif path.endswith("/im/v1/messages/a1"):
            content = json.dumps({"text": "先讨论发布窗口"}, ensure_ascii=False)
            self.send_json({"code": 0, "data": {"items": [{
                "message_id": "a1", "chat_id": "chat-alpha", "msg_type": "text",
                "sender": {"id": "user-alice", "sender_type": "user"},
                "body": {"content": content}, "create_time": "1"
            }]}})
        elif path.endswith("/im/v1/messages/invisible-parent"):
            self.send_json({"code": 230027, "msg": "Lack of necessary permissions"})
        elif "/im/v1/chats/" in path:
            self.send_json({"code": 0, "data": {"name": "Alpha 发布群"}})
        else:
            self.send_json({"code": 404, "msg": "not found"})

ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
FEISHU_MOCK_PID=$!
export BIFROST_E2E_ALLOW_FEISHU_LOOPBACK_BASE_URL=1


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

wait_run_count() {
  local expected="$1"
  for _ in $(seq 1 160); do
    local actual=0
    if [[ -f "$RUN_LOG" ]]; then
      actual="$(wc -l <"$RUN_LOG" | tr -d ' ')"
    fi
    if [[ "$actual" == "$expected" ]]; then
      return 0
    fi
    sleep 0.25
  done
  echo "expected $expected runner lifecycle events" >&2
  [[ -f "$RUN_LOG" ]] && sed -n '1,40p' "$RUN_LOG" >&2 || true
  tail -160 "$BIFROST_LOG" >&2 || true
  return 1
}

wait_session_idle() {
  local session_key="$1"
  for _ in $(seq 1 160); do
    if curl -fsS --noproxy '*' \
      "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/agent/sessions/all?limit=80" \
      | python3 -c '
import json, sys
session_key = sys.argv[1]
sessions = json.load(sys.stdin).get("sessions", [])
raise SystemExit(1 if any(item.get("session_key") == session_key and item.get("running") is True for item in sessions) else 0)
' "$session_key"; then
      return 0
    fi
    sleep 0.25
  done
  echo "session remained active: $session_key" >&2
  return 1
}

wait_group_message_recorded() {
  local message_id="$1"
  for _ in $(seq 1 160); do
    if python3 - "$TEST_DIR/admin/im_group_context.db" "$message_id" <<'PY'
import pathlib
import sqlite3
import sys

db_path, message_id = sys.argv[1:3]
if not pathlib.Path(db_path).exists():
    raise SystemExit(1)
connection = sqlite3.connect(db_path)
recorded = connection.execute(
    "SELECT 1 FROM im_group_messages WHERE message_id = ? LIMIT 1",
    (message_id,),
).fetchone()
raise SystemExit(0 if recorded else 1)
PY
    then
      return 0
    fi
    sleep 0.25
  done
  echo "group message was not recorded: $message_id" >&2
  return 1
}

wait_group_turn_completed() {
  local trigger_message_id="$1"
  for _ in $(seq 1 160); do
    if python3 - "$TEST_DIR/admin/im_group_context.db" "$trigger_message_id" <<'PY'
import pathlib
import sqlite3
import sys

db_path, trigger_message_id = sys.argv[1:3]
if not pathlib.Path(db_path).exists():
    raise SystemExit(1)
connection = sqlite3.connect(db_path)
status = connection.execute(
    "SELECT status FROM im_group_turns WHERE trigger_message_id = ? LIMIT 1",
    (trigger_message_id,),
).fetchone()
raise SystemExit(0 if status == ("completed",) else 1)
PY
    then
      return 0
    fi
    sleep 0.25
  done
  echo "group turn did not complete: $trigger_message_id" >&2
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

python3 - "$BIFROST_PORT" "$REPO_DIR" "$PROMPT_LOG" "$RUN_LOG" "$CONCURRENT_RELEASE" "$FEISHU_MOCK_PORT" <<'PY'
import json
import sys
import urllib.request

port, repo_dir, prompt_log, run_log, concurrent_release, feishu_mock_port = sys.argv[1:7]
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

runner_code = f"""
import json
import os
import sys
import time

prompt = sys.stdin.read()
open({prompt_log!r}, "a", encoding="utf-8").write(
    json.dumps(prompt, ensure_ascii=False) + "\\n"
)
open({run_log!r}, "a", encoding="utf-8").write(
    json.dumps({{"phase": "start", "pid": os.getpid(), "at": time.time()}}) + "\\n"
)
if "并发检查" in prompt:
    deadline = time.time() + 30
    while not os.path.exists({concurrent_release!r}):
        if time.time() >= deadline:
            raise RuntimeError("timed out waiting for concurrent test release")
        time.sleep(0.05)
else:
    time.sleep(1)
open({run_log!r}, "a", encoding="utf-8").write(
    json.dumps({{"phase": "finish", "pid": os.getpid(), "at": time.time()}}) + "\\n"
)
print(json.dumps({{"type": "assistant_final", "content": "GROUP_OK"}}))
"""
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
    "base_url": f"http://127.0.0.1:{feishu_mock_port}/open-apis",
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
  local parent_id="${8:-}"
  python3 - "$BIFROST_PORT" "$chat_id" "$user_id" "$user_name" "$message_id" "$text" "$mention_bot" "$chat_type" "$parent_id" <<'PY'
import json
import sys
import urllib.request

port, chat_id, user_id, user_name, message_id, text, mention_bot, chat_type, parent_id = sys.argv[1:10]
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
if parent_id:
    payload["parentId"] = parent_id
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
wait_run_count 2
wait_session_idle "im:feishu-group-e2e:group:chat-alpha"

inject chat-alpha user-bob Bob a4 "/cwd $REPO_DIR"
inject chat-alpha user-bob Bob a5 "/runner group-mock"
inject chat-alpha user-bob Bob a6 "/status"
inject chat-alpha user-bob Bob a6q "/q"
inject chat-alpha user-bob Bob a6pwd "/pwd"
inject chat-alpha user-bob Bob a6runner "/runner"
sleep 1
wait_prompt_count 1
inject chat-alpha user-bob Bob a7 "补充：需要回滚预案"
inject chat-alpha user-bob Bob a8 "/g 继续给出行动项"
wait_prompt_count 2
wait_run_count 4
wait_session_idle "im:feishu-group-e2e:group:chat-alpha"

# Exact command boundaries match direct messages: /help is a command, while
# `/help extra` falls through to the model and therefore receives group context.
inject chat-alpha user-bob Bob a9 "/help extra"
wait_prompt_count 3
wait_run_count 6
wait_session_idle "im:feishu-group-e2e:group:chat-alpha"

inject chat-beta user-carol Carol b1 "另一个群的背景"
inject chat-beta user-carol Carol b2 "@_user_1 给出一句结论" true
wait_prompt_count 4

# While the first Beta turn is still running, an ambient message must survive
# a rejected /clear and be included in the subsequently accepted queued turn.
inject chat-beta user-dave Dave b3 "忙时补充：保留这条背景"
inject chat-beta user-carol Carol b4 "/clear"
inject chat-beta user-carol Carol b5 "/q 排队后的结论"
wait_prompt_count 5
wait_run_count 10
wait_session_idle "im:feishu-group-e2e:group:chat-beta"

# A long-running group task must not own the provider receiver. Two groups and
# the provider owner's direct-message session start separate worker processes
# before any of the three workers finishes.
inject chat-alpha user-alice Alice a10 "@_user_1 并发检查 Alpha" true
inject chat-beta user-carol Carol b6 "@_user_1 并发检查 Beta" true
inject direct-owner owner-only-in-p2p Owner p1 "并发检查私聊" false p2p
wait_prompt_count 8
if [[ -n "${CONCURRENT_INJECT_DELAY_SECONDS:-}" ]]; then
  sleep "$CONCURRENT_INJECT_DELAY_SECONDS"
fi
# A trigger redelivered while Alpha's runner is active must be acknowledged and
# audited once, without being executed twice. The ambient message is retained
# as group context but must not receive trigger acknowledgement side effects.
inject chat-alpha user-alice Alice a11 "/status"
inject chat-alpha user-alice Alice a11 "/status"
inject chat-alpha user-bob Bob a12 "并发期间的普通背景消息"
wait_group_message_recorded a12
touch "$CONCURRENT_RELEASE"
for _ in $(seq 1 160); do
  if [[ -f "$RUN_LOG" ]] && [[ "$(wc -l <"$RUN_LOG" | tr -d ' ')" == "16" ]]; then
    break
  fi
  sleep 0.25
done
[[ -f "$RUN_LOG" ]]
[[ "$(wc -l <"$RUN_LOG" | tr -d ' ')" == "16" ]]
wait_group_turn_completed a10
wait_group_turn_completed b6
wait_session_idle "im:feishu-group-e2e:group:chat-alpha"
wait_session_idle "im:feishu-group-e2e:group:chat-beta"
wait_session_idle "feishu-group-e2e:owner-only-in-p2p"

# Replying to an older message with only an @ mention is still a valid Agent
# turn. The quoted message becomes the main input even though it is before the
# current group cursor, and the trigger must not fall back to /help.
inject chat-alpha user-alice Alice a13 "@_user_1" true group a1
wait_prompt_count 9
wait_run_count 18
wait_session_idle "im:feishu-group-e2e:group:chat-alpha"

# If Feishu denies reading the parent, return actionable permission guidance
# without starting a Runner or pretending that the quoted content was read.
inject chat-alpha user-alice Alice a14 "@_user_1" true group invisible-parent
wait_prompt_count 9
wait_run_count 18

python3 - "$PROMPT_LOG" "$RUN_LOG" "$TEST_DIR/admin/im_group_context.db" "$REPO_DIR" "$TEST_DIR/admin/im_gateway_message_logs.json" <<'PY'
import json
import pathlib
import sqlite3
import sys

prompt_path, run_path, db_path, repo_dir, message_log_path = sys.argv[1:6]
prompts = [json.loads(line) for line in open(prompt_path, encoding="utf-8") if line.strip()]
assert len(prompts) == 9, prompts
first, second, slash_fallback, third, queued = prompts[:5]
concurrent_prompts = prompts[5:8]
quoted_prompt = prompts[8]

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

assert "本轮主要处理对象（来自被引用消息）" in quoted_prompt, quoted_prompt
assert "<at id=user-alice>Alice</at>：先讨论发布窗口" in quoted_prompt, quoted_prompt
assert quoted_prompt.count("先讨论发布窗口") == 1, quoted_prompt
assert "当前用户未附加文字；请直接理解并回应上述被引用消息" in quoted_prompt, quoted_prompt
assert "/help" not in quoted_prompt, quoted_prompt

runner_events = [json.loads(line) for line in open(run_path, encoding="utf-8") if line.strip()]
assert len(runner_events) == 18, runner_events
concurrent_events = runner_events[10:16]
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
assert messages == [("chat-alpha", 16), ("chat-beta", 6)], messages
permission_turns = connection.execute(
    "SELECT status FROM im_group_turns WHERE trigger_message_id = 'a14'"
).fetchall()
assert permission_turns == [], permission_turns
beta_turns = connection.execute(
    "SELECT status FROM im_group_turns WHERE chat_id = 'chat-beta' ORDER BY created_at"
).fetchall()
assert beta_turns == [("completed",), ("completed",), ("completed",)], beta_turns

message_logs = json.load(open(message_log_path, encoding="utf-8"))["messages"]
status_logs = [
    entry for entry in message_logs
    if entry.get("direction") == "inbound" and entry.get("message_id") == "a11"
]
assert len(status_logs) == 1, status_logs
assert status_logs[0].get("reaction_added") is True, status_logs
ambient_logs = [
    entry for entry in message_logs
    if entry.get("direction") == "inbound" and entry.get("message_id") == "a12"
]
assert ambient_logs == [], ambient_logs
permission_replies = [
    entry for entry in message_logs
    if entry.get("direction") == "outbound"
    and "im:message:readonly" in ((entry.get("content") or "") + (entry.get("content_preview") or ""))
    and "im:message.group_msg" in ((entry.get("content") or "") + (entry.get("content_preview") or ""))
]
assert len(permission_replies) == 1, permission_replies
outbound_text = "\n".join(
    (entry.get("content") or "") + "\n" + (entry.get("content_preview") or "")
    for entry in message_logs
    if entry.get("direction") == "outbound"
)
assert "当前线程排队消息" in outbound_text and "排队已清空" in outbound_text, outbound_text
assert "当前线程工作目录" in outbound_text and str(pathlib.Path(repo_dir).resolve()) in outbound_text, outbound_text
assert "当前 Runner：`group-mock`" in outbound_text, outbound_text
PY

echo "[feishu-group-session] PASS"
