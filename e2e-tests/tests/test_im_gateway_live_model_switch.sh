#!/usr/bin/env bash
set -euo pipefail

unset BIFROST_DETACHED_DAEMON_CHILD
unset BIFROST_EXTERNAL_CLI_WORKER
unset BIFROST_IM_GATEWAY_WORKER
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
export BIFROST_DISABLE_TRAY=1
export BIFROST_E2E_ALLOW_FEISHU_LOOPBACK_BASE_URL=1
export CARGO_NET_OFFLINE=true

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

case "${BIFROST_BIN//\\//}" in
  target/release/bifrost|*/target/release/bifrost|target/release/bifrost.exe|*/target/release/bifrost.exe)
    echo "[im-live-model] SKIP fake OpenAPI: release build rejects Feishu loopback by design"
    exit 0
    ;;
esac

TEST_DIR="$(mktemp -d "$REPO_DIR/.bifrost-e2e-live-model.XXXXXX")"
BIFROST_LOG="$TEST_DIR/bifrost.log"
PROTOCOL_LOG="$TEST_DIR/app-server-protocol.jsonl"
TURN_READY="$TEST_DIR/turn.ready"
FEISHU_REQUEST_LOG="$TEST_DIR/feishu-requests.jsonl"
FEISHU_PORT_FILE="$TEST_DIR/feishu.port"
BIFROST_PORT="${BIFROST_PORT:-$(python3 - <<'PY'
import socket
with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
)}"

cleanup() {
  if [[ -n "${FEISHU_PID:-}" ]]; then
    kill "$FEISHU_PID" >/dev/null 2>&1 || true
    wait "$FEISHU_PID" >/dev/null 2>&1 || true
  fi
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

python3 - "$FEISHU_PORT_FILE" "$FEISHU_REQUEST_LOG" <<'PY' &
import json
import pathlib
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port_file = pathlib.Path(sys.argv[1])
request_log = pathlib.Path(sys.argv[2])
lock = threading.Lock()

class Handler(BaseHTTPRequestHandler):
    card_counter = 0
    message_counter = 0

    def log_message(self, *_args):
        pass

    def send_json(self, payload):
        body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def read_json(self):
        length = int(self.headers.get("content-length", "0"))
        raw = self.rfile.read(length) if length else b"{}"
        return json.loads(raw.decode("utf-8"))

    def record(self, body):
        with lock:
            with request_log.open("a", encoding="utf-8") as handle:
                handle.write(json.dumps({"method": self.command, "path": self.path, "body": body}, ensure_ascii=False) + "\n")

    def do_POST(self):
        path = self.path.split("?", 1)[0]
        body = self.read_json()
        if path.endswith("/auth/v3/tenant_access_token/internal"):
            self.send_json({"code": 0, "tenant_access_token": "live-model-token", "expire": 7200})
            return
        self.record(body)
        if path.endswith("/cardkit/v1/cards"):
            type(self).card_counter += 1
            self.send_json({"code": 0, "data": {"card_id": f"card_live_model_{type(self).card_counter}"}})
            return
        if path.endswith("/reply") or path.endswith("/im/v1/messages"):
            type(self).message_counter += 1
            self.send_json({"code": 0, "data": {"message_id": f"om_live_model_{type(self).message_counter}"}})
            return
        self.send_json({"code": 0})

    def do_PUT(self):
        body = self.read_json()
        self.record(body)
        self.send_json({"code": 0})

    def do_PATCH(self):
        body = self.read_json()
        self.record(body)
        self.send_json({"code": 0})

server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
port_file.write_text(str(server.server_address[1]), encoding="utf-8")
server.serve_forever()
PY
FEISHU_PID=$!

for _ in $(seq 1 80); do
  [[ -s "$FEISHU_PORT_FILE" ]] && break
  kill -0 "$FEISHU_PID" 2>/dev/null || {
    echo "[im-live-model] fake Feishu exited before reporting its port" >&2
    exit 1
  }
  sleep 0.1
done
[[ -s "$FEISHU_PORT_FILE" ]]
FEISHU_PORT="$(<"$FEISHU_PORT_FILE")"

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

wait_for_progress_card_text() {
  local expected="$1"
  for _ in $(seq 1 300); do
    if python3 - "$FEISHU_REQUEST_LOG" "$expected" <<'PY'
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
expected = sys.argv[2]
if not path.exists():
    raise SystemExit(1)
for line in path.read_text(encoding="utf-8").splitlines():
    record = json.loads(line)
    if "/cardkit/v1/cards/" not in record.get("path", ""):
        continue
    if expected in json.dumps(record.get("body"), ensure_ascii=False):
        raise SystemExit(0)
raise SystemExit(1)
PY
    then
      return 0
    fi
    sleep 0.05
  done
  echo "[im-live-model] missing progress card text '$expected'" >&2
  [[ -f "$FEISHU_REQUEST_LOG" ]] && cat "$FEISHU_REQUEST_LOG" >&2 || true
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
  env -u HTTP_PROXY -u HTTPS_PROXY -u ALL_PROXY \
    -u http_proxy -u https_proxy -u all_proxy \
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

python3 - "$BIFROST_PORT" "$MOCK_CODEX" "$PROTOCOL_LOG" "$TURN_READY" "$FEISHU_PORT" <<'PY'
import json
import sys
import urllib.request

port, executable, protocol_log, turn_ready, feishu_port = sys.argv[1:6]
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
                "model": "gpt-runner-default",
                "env": {
                    "BIFROST_PROTOCOL_LOG": protocol_log,
                    "BIFROST_TURN_READY": turn_ready,
                },
                "timeoutSecs": 30,
            },
            "injectBifrostTools": False,
            "skillPaths": [],
            "deliveryMode": "progress_card",
        }
    },
    "channels": {},
}, "PATCH")
request("/providers", {
    "id": "live-model-provider",
    "provider_type": "feishu",
    "display_name": "Live Model E2E",
    "enabled": True,
    "base_url": f"http://127.0.0.1:{feishu_port}/open-apis",
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
wait_for_progress_card_text "gpt-live-unit"
inject "live-model-clear" "/model clear"
wait_for_pattern "$PROTOCOL_LOG" '"method":"thread/settings/update","params":{"model":null,"threadId":"live-model-thread"}'
wait_for_pattern "$TEST_DIR/admin/im_gateway_message_logs.json" "已清除 Codex Runner"
wait_for_progress_card_text "Codex 默认模型"
wait_for_pattern "$TEST_DIR/admin/im_gateway_message_logs.json" "LIVE_MODEL_SWITCH_OK"

python3 - "$TEST_DIR" "$PROTOCOL_LOG" "$FEISHU_REQUEST_LOG" <<'PY'
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

feishu_records = [
    json.loads(line)
    for line in pathlib.Path(sys.argv[3]).read_text(encoding="utf-8").splitlines()
]
card_updates = [
    record
    for record in feishu_records
    if "/cardkit/v1/cards/" in record.get("path", "")
]
assert card_updates, feishu_records
serialized_updates = [json.dumps(record.get("body"), ensure_ascii=False) for record in card_updates]
assert any("gpt-live-unit" in body for body in serialized_updates), serialized_updates
model_updates = [
    body
    for body in serialized_updates
    if any(label in body for label in ("gpt-runner-default", "gpt-live-unit", "Codex 默认模型"))
]
assert model_updates, serialized_updates
assert "Codex 默认模型" in model_updates[-1], model_updates[-1]
assert "gpt-runner-default" not in model_updates[-1], model_updates[-1]
assert "gpt-live-unit" not in model_updates[-1], model_updates[-1]
PY

echo "[im-live-model] PASS"
