#!/usr/bin/env bash
set -euo pipefail

unset BIFROST_DETACHED_DAEMON_CHILD
unset BIFROST_EXTERNAL_CLI_WORKER
unset BIFROST_IM_GATEWAY_WORKER
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
export BIFROST_DISABLE_TRAY=1

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
TEST_DIR="$(mktemp -d "$REPO_DIR/.bifrost-e2e-live-model.XXXXXX")"
BIFROST_LOG="$TEST_DIR/bifrost.log"
PROTOCOL_LOG="$TEST_DIR/app-server-protocol.jsonl"
TURN_READY="$TEST_DIR/turn.ready"
BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
BIFROST_PORT="${BIFROST_PORT:-$(python3 - <<'PY'
import socket
with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
)}"

cleanup() {
  if [[ -n "${BIFROST_PID:-}" ]]; then
    kill "$BIFROST_PID" >/dev/null 2>&1 || true
    wait "$BIFROST_PID" >/dev/null 2>&1 || true
  fi
  if [[ "${KEEP_TEST_DIR:-false}" == "true" ]]; then
    echo "[im-live-model] kept test directory: $TEST_DIR" >&2
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

wait_for_pattern() {
  local path="$1"
  local pattern="$2"
  for _ in $(seq 1 300); do
    if grep -Fq -- "$pattern" "$path" 2>/dev/null; then
      return 0
    fi
    sleep 0.05
  done
  echo "[im-live-model] missing '$pattern' in $path" >&2
  [[ -f "$path" ]] && sed -n '1,160p' "$path" >&2 || true
  tail -160 "$BIFROST_LOG" >&2 || true
  return 1
}

inject() {
  local message_id="$1"
  local text="$2"
  python3 - "$BIFROST_PORT" "$message_id" "$text" <<'PY'
import json
import sys
import urllib.request

port, message_id, text = sys.argv[1:4]
payload = {
    "providerId": "live-model-provider",
    "chatId": "chat-live-model",
    "chatType": "p2p",
    "userId": "live-model-owner",
    "userName": "Live Model E2E",
    "messageId": message_id,
    "eventId": "event-" + message_id,
    "text": text,
    "mentionBot": False,
}
request = urllib.request.Request(
    f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/debug/mock-inbound",
    data=json.dumps(payload).encode(),
    headers={"content-type": "application/json"},
    method="POST",
)
with urllib.request.urlopen(request, timeout=30) as response:
    assert response.status == 200, response.read().decode()
PY
}

MOCK_CODEX="$TEST_DIR/mock-codex.py"
cat >"$MOCK_CODEX" <<'PY'
#!/usr/bin/env python3
import json
import os
import pathlib
import sys

if len(sys.argv) >= 3 and sys.argv[1:3] == ["debug", "models"]:
    print(json.dumps({"models": [{
        "slug": "gpt-live-unit",
        "description": "Live model unit",
        "visibility": "list",
        "supported_in_api": True,
        "supported_reasoning_levels": [{"effort": "medium"}],
    }]}))
    raise SystemExit(0)
if "--version" in sys.argv:
    print("codex 0.0.0-live-model")
    raise SystemExit(0)

log = pathlib.Path(os.environ["BIFROST_PROTOCOL_LOG"])
ready = pathlib.Path(os.environ["BIFROST_TURN_READY"])
updates = 0

def send(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)

for line in sys.stdin:
    frame = json.loads(line)
    with log.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(frame, separators=(",", ":")) + "\n")
    method, request_id = frame.get("method"), frame.get("id")
    if method == "initialize":
        send({"jsonrpc":"2.0","id":request_id,"result":{}})
    elif method == "thread/start":
        send({"jsonrpc":"2.0","id":request_id,"result":{"thread":{"id":"live-model-thread"}}})
    elif method == "turn/start":
        send({"jsonrpc":"2.0","id":request_id,"result":{"turn":{"id":"live-model-turn"}}})
        ready.write_text("ready", encoding="utf-8")
    elif method == "account/rateLimits/read":
        send({"jsonrpc":"2.0","id":request_id,"error":{"code":-32601,"message":"unsupported"}})
    elif method == "thread/settings/update":
        assert frame["params"]["threadId"] == "live-model-thread"
        if updates == 0:
            assert frame["params"]["model"] == "gpt-live-unit"
        else:
            assert frame["params"]["model"] is None
        updates += 1
        send({"jsonrpc":"2.0","id":request_id,"result":{}})
        if updates == 2:
            send({"jsonrpc":"2.0","method":"item/completed","params":{"threadId":"live-model-thread","turnId":"live-model-turn","item":{"id":"final","type":"agentMessage","text":"LIVE_MODEL_SWITCH_OK"}}})
            send({"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":"live-model-thread","turn":{"id":"live-model-turn","status":"completed"}}})
PY
chmod +x "$MOCK_CODEX"

if [[ "${SKIP_BUILD:-false}" != "true" ]]; then
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

BIFROST_DATA_DIR="$TEST_DIR" "$BIFROST_BIN" start \
  --host 127.0.0.1 \
  -p "$BIFROST_PORT" \
  --unsafe-ssl \
  --skip-cert-check \
  --no-system-proxy \
  >"$BIFROST_LOG" 2>&1 &
BIFROST_PID=$!
wait_http

python3 - "$BIFROST_PORT" "$MOCK_CODEX" "$PROTOCOL_LOG" "$TURN_READY" <<'PY'
import json
import sys
import urllib.request

port, executable, protocol_log, turn_ready = sys.argv[1:5]
api = f"http://127.0.0.1:{port}/_bifrost/api/im-gateway"

def request(path, payload, method):
    req = urllib.request.Request(
        api + path,
        data=json.dumps(payload).encode(),
        headers={"content-type": "application/json"},
        method=method,
    )
    with urllib.request.urlopen(req, timeout=30) as response:
        assert response.status == 200, response.read().decode()

request("/chat/config", {
    "version": 1,
    "defaultRunnerId": "live-model-codex",
    "runners": {
        "live-model-codex": {
            "enabled": True,
            "adapter": "codex",
            "adapterConfig": {
                "executable": executable,
                "transport": "app_server",
                "env": {
                    "BIFROST_PROTOCOL_LOG": protocol_log,
                    "BIFROST_TURN_READY": turn_ready,
                },
                "timeoutSecs": 30,
            },
            "injectBifrostTools": False,
            "skillPaths": [],
            "deliveryMode": "final_reply",
        }
    },
    "channels": {},
}, "PATCH")
request("/providers", {
    "id": "live-model-provider",
    "provider_type": "feishu",
    "display_name": "Live Model E2E",
    "enabled": True,
    "base_url": "http://127.0.0.1:9/open-apis",
    "app_id": "cli_live_model_e2e",
    "app_secret": "live-model-secret",
    "owner_open_id": "live-model-owner",
    "event_connection_enabled": False,
    "agent_config": {"runner": "live-model-codex"},
}, "POST")
PY

inject "live-model-turn" "keep this turn running"
wait_for_pattern "$TURN_READY" "ready"
inject "live-model-set" "/model gpt-live-unit"
wait_for_pattern "$PROTOCOL_LOG" '"method":"thread/settings/update","params":{"model":"gpt-live-unit","threadId":"live-model-thread"}'
wait_for_pattern "$TEST_DIR/admin/im_gateway_message_logs.json" "后续响应/轮次生效"
inject "live-model-clear" "/model clear"
wait_for_pattern "$PROTOCOL_LOG" '"method":"thread/settings/update","params":{"model":null,"threadId":"live-model-thread"}'
wait_for_pattern "$TEST_DIR/admin/im_gateway_message_logs.json" "已清除 Codex Runner"
wait_for_pattern "$TEST_DIR/admin/im_gateway_message_logs.json" "LIVE_MODEL_SWITCH_OK"

python3 - "$TEST_DIR" "$PROTOCOL_LOG" <<'PY'
import json
import pathlib
import sys

test_dir = pathlib.Path(sys.argv[1])
frames = [json.loads(line) for line in pathlib.Path(sys.argv[2]).read_text().splitlines()]
updates = [frame for frame in frames if frame.get("method") == "thread/settings/update"]
assert len(updates) == 2, updates
assert [frame["params"]["model"] for frame in updates] == ["gpt-live-unit", None], updates
assert all(frame["params"]["threadId"] == "live-model-thread" for frame in updates), updates

state = json.loads(
    (test_dir / "agent" / "im_gateway" / "session_state.json").read_text(encoding="utf-8")
)
sessions = [value for value in state["sessions"].values() if value.get("runnerId") == "live-model-codex"]
assert len(sessions) == 1, sessions
assert sessions[0].get("modelOverride") is None, sessions[0]

messages = json.loads(
    (test_dir / "admin" / "im_gateway_message_logs.json").read_text(encoding="utf-8")
)["messages"]
outbound = [item.get("content") or "" for item in messages if item.get("direction") == "outbound"]
assert any("运行中的 session" in item and "后续响应/轮次生效" in item for item in outbound), outbound
assert not any("等待任务结束后再切换 Runner 模型" in item for item in outbound), outbound
PY

echo "[im-live-model] PASS"
