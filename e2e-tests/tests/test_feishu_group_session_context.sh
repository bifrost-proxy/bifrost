#!/usr/bin/env bash
set -euo pipefail

unset BIFROST_DETACHED_DAEMON_CHILD
unset BIFROST_EXTERNAL_CLI_WORKER
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
export CARGO_NET_OFFLINE="${CARGO_NET_OFFLINE:-true}"

# Hard fail closed for network access: this scenario must only reach the two
# loopback Feishu fixtures and the loopback Bifrost API. Any accidental public
# request is routed to a closed local port instead of becoming a flaky CI call.
export HTTP_PROXY=http://127.0.0.1:9
export HTTPS_PROXY=http://127.0.0.1:9
export ALL_PROXY=http://127.0.0.1:9
export NO_PROXY=127.0.0.1,localhost
export http_proxy="$HTTP_PROXY"
export https_proxy="$HTTPS_PROXY"
export all_proxy="$ALL_PROXY"
export no_proxy="$NO_PROXY"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"

# CI's broad shell matrix reuses a production release artifact. Production
# builds deliberately normalize every Feishu base URL back to the official
# allowlist, even when the debug-only loopback flag is present. Running this
# fake-OpenAPI scenario with that binary would contact the official endpoint
# with test credentials; the first quoted-message read then fails with Feishu
# code 10003 instead of exercising the local fixture. Keep that security
# boundary closed. The focused debug E2E below covers the complete black-box
# flow, while release-safe unit tests cover URL normalization and the Feishu
# HTTP response/error matrix.
case "${BIFROST_BIN//\\//}" in
  target/release/bifrost|*/target/release/bifrost|target/release/bifrost.exe|*/target/release/bifrost.exe)
    echo "[feishu-group-session] SKIP fake OpenAPI: release build rejects Feishu loopback by design"
    exit 0
    ;;
esac

TEST_DIR="$(mktemp -d)"
BIFROST_LOG="$TEST_DIR/bifrost.log"
PROMPT_LOG="$TEST_DIR/group-prompts.jsonl"
RUN_LOG="$TEST_DIR/runner-events.jsonl"
CONCURRENT_RELEASE="$TEST_DIR/concurrent-release"
FEISHU_MOCK_PORT_FILE="$TEST_DIR/feishu-mock.port"
FEISHU_MOCK_B_PORT_FILE="$TEST_DIR/feishu-mock-b.port"
REQUESTED_BIFROST_PORT="${BIFROST_PORT:-}"

choose_loopback_port() {
  python3 - <<'PY'
import socket
with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
}
START_EXTRA_ARGS=()
if [[ "$(uname -s)" != "Linux" ]]; then
  START_EXTRA_ARGS+=(--no-tray)
fi

cleanup() {
  if [[ -n "${FEISHU_MOCK_PID:-}" ]]; then
    kill "$FEISHU_MOCK_PID" >/dev/null 2>&1 || true
    wait "$FEISHU_MOCK_PID" >/dev/null 2>&1 || true
  fi
  if [[ -n "${FEISHU_MOCK_B_PID:-}" ]]; then
    kill "$FEISHU_MOCK_B_PID" >/dev/null 2>&1 || true
    wait "$FEISHU_MOCK_B_PID" >/dev/null 2>&1 || true
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

python3 - "$FEISHU_MOCK_PORT_FILE" <<'PY' &
import json
import pathlib
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port_file = pathlib.Path(sys.argv[1])

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
        elif path.endswith("/im/v1/messages/card-parent"):
            content = json.dumps({
                "schema": "2.0",
                "header": {"title": {"tag": "plain_text", "content": "另一机器人结论"}},
                "body": {"elements": [
                    {"tag": "markdown", "content": "卡片正文：选择方案 A"},
                    {"tag": "button", "text": {"tag": "plain_text", "content": "不应读取"}, "url": "https://example.invalid/action"},
                ]},
            }, ensure_ascii=False)
            self.send_json({"code": 0, "data": {"items": [{
                "message_id": "card-parent", "chat_id": "chat-alpha", "msg_type": "interactive",
                "sender": {"id": "ou_other_bot", "sender_type": "app"},
                "body": {"content": content}, "create_time": "2"
            }]}})
        elif "/im/v1/chats/" in path:
            self.send_json({"code": 0, "data": {"name": "Alpha 发布群"}})
        else:
            self.send_json({"code": 404, "msg": "not found"})

server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
port_file.write_text(str(server.server_address[1]), encoding="utf-8")
server.serve_forever()
PY
FEISHU_MOCK_PID=$!
python3 - "$FEISHU_MOCK_B_PORT_FILE" <<'PY' &
import json
import pathlib
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

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
            self.send_json({"code": 0, "tenant_access_token": "e2e-token-b", "expire": 7200})
        else:
            self.send_json({"code": 0})
    def do_GET(self):
        path = self.path.split("?", 1)[0]
        if path.endswith("/bot/v3/info"):
            self.send_json({"code": 0, "bot": {"open_id": "ou_bot_b", "app_name": "Bifrost B"}})
        elif "/im/v1/chats/" in path:
            self.send_json({"code": 0, "data": {"name": "共享多机器人群"}})
        else:
            self.send_json({"code": 404, "msg": "not found"})

server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
pathlib.Path(sys.argv[1]).write_text(str(server.server_address[1]), encoding="utf-8")
server.serve_forever()
PY
FEISHU_MOCK_B_PID=$!
for _ in $(seq 1 80); do
  if [[ -s "$FEISHU_MOCK_PORT_FILE" && -s "$FEISHU_MOCK_B_PORT_FILE" ]]; then
    break
  fi
  kill -0 "$FEISHU_MOCK_PID" "$FEISHU_MOCK_B_PID" 2>/dev/null || {
    echo "Feishu loopback fixture exited before reporting its port" >&2
    exit 1
  }
  sleep 0.1
done
[[ -s "$FEISHU_MOCK_PORT_FILE" && -s "$FEISHU_MOCK_B_PORT_FILE" ]]
FEISHU_MOCK_PORT="$(<"$FEISHU_MOCK_PORT_FILE")"
FEISHU_MOCK_B_PORT="$(<"$FEISHU_MOCK_B_PORT_FILE")"
[[ "$FEISHU_MOCK_PORT" != "$FEISHU_MOCK_B_PORT" ]]
export BIFROST_E2E_ALLOW_FEISHU_LOOPBACK_BASE_URL=1


wait_http() {
  for _ in $(seq 1 180); do
    if curl -fsS --noproxy '*' \
      "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address" >/dev/null 2>&1; then
      return 0
    fi
    kill -0 "$BIFROST_PID" 2>/dev/null || return 1
    sleep 0.25
  done
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

BIFROST_READY=false
for attempt in $(seq 1 5); do
  if [[ -n "$REQUESTED_BIFROST_PORT" ]]; then
    BIFROST_PORT="$REQUESTED_BIFROST_PORT"
  else
    BIFROST_PORT="$(choose_loopback_port)"
    if [[ "$BIFROST_PORT" == "$FEISHU_MOCK_PORT" || "$BIFROST_PORT" == "$FEISHU_MOCK_B_PORT" ]]; then
      continue
    fi
  fi
  : >"$BIFROST_LOG"
  BIFROST_DATA_DIR="$TEST_DIR" "$BIFROST_BIN" start \
    --host 127.0.0.1 \
    -p "$BIFROST_PORT" \
    --unsafe-ssl \
    --skip-cert-check \
    --no-system-proxy \
    "${START_EXTRA_ARGS[@]}" \
    >"$BIFROST_LOG" 2>&1 &
  BIFROST_PID=$!
  if wait_http; then
    BIFROST_READY=true
    break
  fi
  kill "$BIFROST_PID" >/dev/null 2>&1 || true
  wait "$BIFROST_PID" >/dev/null 2>&1 || true
  unset BIFROST_PID
  [[ -z "$REQUESTED_BIFROST_PORT" ]] || break
  echo "[feishu-group-session] Bifrost bind/start attempt $attempt failed; retrying" >&2
done
if [[ "$BIFROST_READY" != "true" ]]; then
  tail -160 "$BIFROST_LOG" >&2 || true
  exit 1
fi

python3 - "$BIFROST_PORT" "$REPO_DIR" "$PROMPT_LOG" "$RUN_LOG" "$CONCURRENT_RELEASE" "$FEISHU_MOCK_PORT" "$FEISHU_MOCK_B_PORT" <<'PY'
import json
import sys
import urllib.request

port, repo_dir, prompt_log, run_log, concurrent_release, feishu_mock_port, feishu_mock_b_port = sys.argv[1:8]
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
request("/providers", {
    "id": "feishu-group-e2e-b",
    "provider_type": "feishu",
    "display_name": "Feishu Group E2E B",
    "enabled": True,
    "base_url": f"http://127.0.0.1:{feishu_mock_b_port}/open-apis",
    "app_id": "cli_group_e2e_b",
    "app_secret": "group-e2e-secret-b",
    "owner_open_id": "owner-b",
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
  local provider_id="${9:-feishu-group-e2e}"
  local mentioned_bot_open_id="${10:-}"
  python3 - "$BIFROST_PORT" "$chat_id" "$user_id" "$user_name" "$message_id" "$text" "$mention_bot" "$chat_type" "$parent_id" "$provider_id" "$mentioned_bot_open_id" <<'PY'
import json
import sys
import urllib.request

port, chat_id, user_id, user_name, message_id, text, mention_bot, chat_type, parent_id, provider_id, mentioned_bot_open_id = sys.argv[1:12]
payload = {
    "providerId": provider_id,
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
if mentioned_bot_open_id:
    payload["mentionedBotOpenId"] = mentioned_bot_open_id
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
inject chat-beta user-carol Carol b5two "/q 第二条排队任务"
inject chat-beta user-carol Carol b5query "/q"
wait_prompt_count 6
wait_run_count 12
wait_session_idle "im:feishu-group-e2e:group:chat-beta"

# A long-running group task must not own the provider receiver. Two groups and
# the provider owner's direct-message session start separate worker processes
# before any of the three workers finishes.
inject chat-alpha user-alice Alice a10 "@_user_1 并发检查 Alpha" true
inject chat-beta user-carol Carol b6 "@_user_1 并发检查 Beta" true
inject direct-owner owner-only-in-p2p Owner p1 "并发检查私聊" false p2p
wait_prompt_count 9
if [[ -n "${CONCURRENT_INJECT_DELAY_SECONDS:-}" ]]; then
  sleep "$CONCURRENT_INJECT_DELAY_SECONDS"
fi
# A trigger redelivered while Alpha's runner is active must be acknowledged and
# audited once, without being executed twice. The ambient message is retained
# as group context but must not receive trigger acknowledgement side effects.
inject chat-alpha user-alice Alice a11 "/status"
inject chat-alpha user-alice Alice a11 "/status"
inject chat-alpha user-bob Bob a12 "并发期间的普通背景消息"
inject chat-alpha user-bob Bob a12human "@_user_1 请人工复核" true group a1 feishu-group-e2e ou_human
wait_group_message_recorded a12
wait_group_message_recorded a12human
touch "$CONCURRENT_RELEASE"
for _ in $(seq 1 160); do
  if [[ -f "$RUN_LOG" ]] && [[ "$(wc -l <"$RUN_LOG" | tr -d ' ')" == "18" ]]; then
    break
  fi
  sleep 0.25
done
[[ -f "$RUN_LOG" ]]
[[ "$(wc -l <"$RUN_LOG" | tr -d ' ')" == "18" ]]
wait_group_turn_completed a10
wait_group_turn_completed b6

# Replying to an older message with only an @ mention is still a valid Agent
# turn. Inject as soon as the durable turn is complete, before waiting for the
# session/progress/history cleanup, to cover the mailbox completion boundary.
inject chat-alpha user-alice Alice a13 "@_user_1" true group a1
wait_prompt_count 10
wait_run_count 20
wait_session_idle "im:feishu-group-e2e:group:chat-alpha"
wait_session_idle "im:feishu-group-e2e:group:chat-beta"
wait_session_idle "feishu-group-e2e:owner-only-in-p2p"

# A card sent by another bot is read through the authoritative loopback Feishu
# message API. Only visible text reaches the prompt; actions and URLs do not.
inject chat-alpha user-alice Alice a13card "@_user_1" true group card-parent
wait_prompt_count 11
wait_run_count 22
wait_session_idle "im:feishu-group-e2e:group:chat-alpha"

# If Feishu denies reading the parent, return actionable permission guidance
# without starting a Runner or pretending that the quoted content was read.
inject chat-alpha user-alice Alice a14 "@_user_1" true group invisible-parent
wait_prompt_count 11
wait_run_count 22

wait_permission_reply() {
  for _ in $(seq 1 160); do
    if curl -fsS --noproxy '*' \
      "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/providers/feishu-group-e2e/messages" \
      | python3 -c '
import json, sys
messages = json.load(sys.stdin)
visible = "\n".join(
    (entry.get("content") or "") + "\n" + (entry.get("content_preview") or "")
    for entry in messages if entry.get("direction") == "outbound"
)
raise SystemExit(0 if "im:message:readonly" in visible and "im:message.group_msg" in visible else 1)
'
    then
      return 0
    fi
    sleep 0.25
  done
  echo "permission guidance was not written to the message log" >&2
  return 1
}
wait_permission_reply

# Two independent Provider loops receive the same group traffic. Unmentioned
# slash commands are consumed by both; an explicit Bot B command is retained
# only by Provider B and must not enter Provider A's SQLite/event/audit stores.
inject shared-multi user-alice Alice mb-a-broadcast "/status"
inject shared-multi user-alice Alice mb-b-broadcast "/status" false group "" feishu-group-e2e-b
# A human mention inside slash arguments is not a routing target. Both Provider
# loops must consume it just like an unmentioned broadcast slash.
inject shared-multi user-alice Alice mb-a-human-arg "/q ask @_user_1 to review" true group "" feishu-group-e2e ou_human
inject shared-multi user-alice Alice mb-b-human-arg "/q ask @_user_1 to review" true group "" feishu-group-e2e-b ou_human
wait_prompt_count 13
wait_run_count 26
wait_session_idle "im:feishu-group-e2e:group:shared-multi"
wait_session_idle "im:feishu-group-e2e-b:group:shared-multi"
inject shared-multi user-alice Alice mb-a-directed "@_user_1 /status" true group "" feishu-group-e2e ou_bot_b
inject shared-multi user-alice Alice mb-b-directed "@_user_1 /status" true group "" feishu-group-e2e-b ou_bot_b
for _ in $(seq 1 160); do
  if python3 - "$TEST_DIR/admin/im_group_context.db" <<'PY'
import pathlib, sqlite3, sys
path = pathlib.Path(sys.argv[1])
if not path.exists(): raise SystemExit(1)
c = sqlite3.connect(path)
ids = {row[0] for row in c.execute("SELECT message_id FROM im_group_messages WHERE chat_id='shared-multi'")}
raise SystemExit(0 if {"mb-a-broadcast", "mb-b-broadcast", "mb-a-human-arg", "mb-b-human-arg", "mb-b-directed"} <= ids else 1)
PY
  then break; fi
  sleep 0.25
done

python3 - "$PROMPT_LOG" "$RUN_LOG" "$TEST_DIR/admin/im_group_context.db" "$REPO_DIR" "$TEST_DIR/admin/im_gateway_message_logs.json" <<'PY'
import json
import pathlib
import sqlite3
import sys

prompt_path, run_path, db_path, repo_dir, message_log_path = sys.argv[1:6]
prompts = [json.loads(line) for line in open(prompt_path, encoding="utf-8") if line.strip()]
assert len(prompts) == 13, prompts
first, second, slash_fallback, third, queued = prompts[:5]
queued_second = prompts[5]
concurrent_prompts = prompts[6:9]
quoted_prompt = prompts[9]
quoted_card_prompt = prompts[10]
human_argument_prompts = prompts[11:13]

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
assert "第二条排队任务" in queued_second, queued_second

for expected in ("并发检查 Alpha", "并发检查 Beta", "并发检查私聊"):
    assert sum(expected in prompt for prompt in concurrent_prompts) == 1, concurrent_prompts

assert "本轮主要处理对象（来自被引用消息）" in quoted_prompt, quoted_prompt
assert "<at id=user-alice>Alice</at>：先讨论发布窗口" in quoted_prompt, quoted_prompt
assert quoted_prompt.count("先讨论发布窗口") == 1, quoted_prompt
assert "当前用户未附加文字；请直接理解并回应上述被引用消息" in quoted_prompt, quoted_prompt
assert "/help" not in quoted_prompt, quoted_prompt
assert "另一机器人结论" in quoted_card_prompt, quoted_card_prompt
assert "卡片正文：选择方案 A" in quoted_card_prompt, quoted_card_prompt
assert "example.invalid" not in quoted_card_prompt and "不应读取" not in quoted_card_prompt, quoted_card_prompt
assert len(human_argument_prompts) == 2, human_argument_prompts
assert all("ask <at id=ou_human>" in prompt and "to review" in prompt for prompt in human_argument_prompts), human_argument_prompts

runner_events = [json.loads(line) for line in open(run_path, encoding="utf-8") if line.strip()]
assert len(runner_events) == 26, runner_events
concurrent_events = runner_events[12:18]
assert [event["phase"] for event in concurrent_events[:3]] == ["start"] * 3, concurrent_events
assert len({event["pid"] for event in concurrent_events[:3]}) == 3, concurrent_events
assert all(event["phase"] == "finish" for event in concurrent_events[3:]), concurrent_events

connection = sqlite3.connect(db_path)
bindings = connection.execute(
    "SELECT chat_id, session_key, last_assigned_seq, chat_name, work_dir, runner_id "
    "FROM im_group_bindings WHERE provider_id = 'feishu-group-e2e' "
    "AND chat_id IN ('chat-alpha', 'chat-beta') ORDER BY chat_id"
).fetchall()
assert len(bindings) == 2, bindings
assert bindings[0][0] == "chat-alpha" and bindings[1][0] == "chat-beta", bindings
assert bindings[0][1] != bindings[1][1], bindings
assert bindings[0][3] == "Alpha 发布群" and bindings[1][3] == "Beta 讨论群", bindings
assert bindings[0][4] == str(pathlib.Path(repo_dir).resolve()), bindings
assert bindings[0][5] == "group-mock", bindings
messages = connection.execute(
    "SELECT chat_id, COUNT(*) FROM im_group_messages WHERE provider_id = 'feishu-group-e2e' "
    "AND chat_id IN ('chat-alpha', 'chat-beta') GROUP BY chat_id ORDER BY chat_id"
).fetchall()
assert messages == [("chat-alpha", 19), ("chat-beta", 8)], messages
permission_turns = connection.execute(
    "SELECT status FROM im_group_turns WHERE trigger_message_id = 'a14'"
).fetchall()
assert permission_turns == [], permission_turns
beta_turns = connection.execute(
    "SELECT status FROM im_group_turns WHERE chat_id = 'chat-beta' ORDER BY created_at"
).fetchall()
assert beta_turns == [("completed",)] * 4, beta_turns

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
shared_rows = connection.execute(
    "SELECT provider_id, message_id FROM im_group_messages WHERE chat_id = 'shared-multi' ORDER BY provider_id, message_id"
).fetchall()
assert ("feishu-group-e2e", "mb-a-broadcast") in shared_rows, shared_rows
assert ("feishu-group-e2e-b", "mb-b-broadcast") in shared_rows, shared_rows
assert ("feishu-group-e2e", "mb-a-human-arg") in shared_rows, shared_rows
assert ("feishu-group-e2e-b", "mb-b-human-arg") in shared_rows, shared_rows
assert ("feishu-group-e2e-b", "mb-b-directed") in shared_rows, shared_rows
assert ("feishu-group-e2e", "mb-a-directed") not in shared_rows, shared_rows
PY

echo "[feishu-group-session] PASS"
