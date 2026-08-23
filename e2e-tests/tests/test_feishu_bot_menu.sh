#!/usr/bin/env bash
set -euo pipefail

unset BIFROST_DETACHED_DAEMON_CHILD
unset BIFROST_EXTERNAL_CLI_WORKER
unset BIFROST_IM_GATEWAY_WORKER
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
export BIFROST_DISABLE_TRAY=1
export BIFROST_SYSTEM_PROXY_DISABLE_LIFECYCLE_HELPER=1
export BIFROST_IM_GATEWAY_EXECUTION_MODE=legacy
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
cd "$REPO_DIR"

TEST_DIR="$(mktemp -d "$REPO_DIR/.bifrost-e2e-feishu-menu.XXXXXX")"
BIFROST_LOG="$TEST_DIR/bifrost.log"
FEISHU_LOG="$TEST_DIR/feishu.jsonl"
FEISHU_PORT_FILE="$TEST_DIR/feishu.port"
FEISHU_DRY_RUN="$TEST_DIR/feishu-cards.jsonl"
BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"

case "${BIFROST_BIN//\\//}" in
  target/release/bifrost|*/target/release/bifrost|target/release/bifrost.exe|*/target/release/bifrost.exe)
    echo "[feishu-bot-menu] SKIP fake OpenAPI: release build rejects Feishu loopback by design"
    exit 0
    ;;
esac

choose_port() {
  python3 - <<'PY'
import socket
with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
}

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
    echo "[feishu-bot-menu] kept test directory: $TEST_DIR" >&2
  else
    rm -rf "$TEST_DIR"
  fi
}
trap cleanup EXIT

python3 - "$FEISHU_PORT_FILE" "$FEISHU_LOG" <<'PY' &
import json
import pathlib
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port_file = pathlib.Path(sys.argv[1])
request_log = pathlib.Path(sys.argv[2])
lock = threading.Lock()

class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def read_json(self):
        length = int(self.headers.get("content-length", "0"))
        raw = self.rfile.read(length) if length else b"{}"
        return json.loads(raw.decode("utf-8"))

    def send_json(self, payload, request_id):
        body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.send_header("x-tt-logid", request_id)
        self.end_headers()
        self.wfile.write(body)

    def record(self, body):
        with lock:
            with request_log.open("a", encoding="utf-8") as handle:
                handle.write(json.dumps({
                    "method": self.command,
                    "path": self.path,
                    "authorization": self.headers.get("authorization"),
                    "body": body,
                }, ensure_ascii=False) + "\n")

    def do_POST(self):
        path = self.path.split("?", 1)[0]
        body = self.read_json()
        self.record(body)
        if path.endswith("/auth/v3/tenant_access_token/internal"):
            self.send_json({
                "code": 0,
                "tenant_access_token": "menu-e2e-token",
                "expire": 7200,
            }, "token-request")
        elif path.endswith("/publish"):
            self.send_json({
                "code": 0,
                "data": {"version_id": "menu-version-id", "version": "1.0.1"},
            }, "publish-request")
        else:
            self.send_json({"code": 404, "msg": "not found"}, "not-found")

    def do_PATCH(self):
        path = self.path.split("?", 1)[0]
        body = self.read_json()
        self.record(body)
        if path.endswith("/ability"):
            self.send_json({"code": 0, "data": {}}, "ability-request")
        elif path.endswith("/config"):
            self.send_json({"code": 0, "data": {}}, "config-request")
        else:
            self.send_json({"code": 404, "msg": "not found"}, "not-found")

server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
port_file.write_text(str(server.server_address[1]), encoding="utf-8")
server.serve_forever()
PY
FEISHU_PID=$!

for _ in $(seq 1 80); do
  [[ -s "$FEISHU_PORT_FILE" ]] && break
  kill -0 "$FEISHU_PID" 2>/dev/null || {
    echo "[feishu-bot-menu] fake Feishu exited before reporting its port" >&2
    exit 1
  }
  sleep 0.1
done
[[ -s "$FEISHU_PORT_FILE" ]]
FEISHU_PORT="$(<"$FEISHU_PORT_FILE")"
BIFROST_PORT="${BIFROST_PORT:-$(choose_port)}"

MOCK_CODEX="$TEST_DIR/mock-codex"
cat >"$MOCK_CODEX" <<'SH'
#!/usr/bin/env sh
cat >/dev/null
printf '%s\n' '{"type":"thread.started","thread_id":"thread-menu"}'
printf '%s\n' '{"type":"assistant_final","content":"MENU_OK"}'
printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}'
SH
chmod +x "$MOCK_CODEX"

if [[ "${SKIP_BUILD:-false}" != "true" ]]; then
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

start_bifrost() {
  BIFROST_DATA_DIR="$TEST_DIR/data" \
  BIFROST_FEISHU_DRY_RUN_FILE="$FEISHU_DRY_RUN" \
  BIFROST_FEISHU_DRY_RUN_PROVIDER_ID=feishu-menu-e2e \
    "$BIFROST_BIN" start \
    --host 127.0.0.1 \
    -p "$BIFROST_PORT" \
    --unsafe-ssl \
    --skip-cert-check \
    --no-system-proxy \
    >>"$BIFROST_LOG" 2>&1 &
  BIFROST_PID=$!

  for _ in $(seq 1 180); do
    if curl -fsS --noproxy '*' \
      "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/providers" >/dev/null 2>&1; then
      return
    fi
    kill -0 "$BIFROST_PID" 2>/dev/null || {
      tail -160 "$BIFROST_LOG" >&2 || true
      exit 1
    }
    sleep 0.25
  done
  echo "[feishu-bot-menu] Bifrost did not become ready" >&2
  tail -160 "$BIFROST_LOG" >&2 || true
  exit 1
}

restart_bifrost() {
  kill "$BIFROST_PID" >/dev/null 2>&1 || true
  wait "$BIFROST_PID" >/dev/null 2>&1 || true
  BIFROST_PID=""
  start_bifrost
}

start_bifrost

python3 - "$BIFROST_PORT" "$FEISHU_PORT" "$MOCK_CODEX" <<'PY'
import json
import sys
import urllib.request

port, feishu_port, mock_codex = sys.argv[1:4]
base = f"http://127.0.0.1:{port}/_bifrost/api/im-gateway"

def request(path, payload, method="POST"):
    req = urllib.request.Request(
        base + path,
        data=json.dumps(payload).encode("utf-8"),
        headers={"content-type": "application/json"},
        method=method,
    )
    with urllib.request.urlopen(req, timeout=30) as response:
        assert response.status == 200, response.read().decode("utf-8")

request("/chat/config", {
    "version": 1,
    "defaultRunnerId": "menu-codex",
    "runners": {
        "menu-codex": {
            "enabled": True,
            "adapter": "codex",
            "adapterConfig": {"executable": mock_codex, "timeoutSecs": 30},
            "injectBifrostTools": False,
            "skillPaths": [],
            "deliveryMode": "final_reply",
        },
        "menu-traex": {
            "enabled": True,
            "adapter": "traex",
            "adapterConfig": {"executable": mock_codex, "timeoutSecs": 30},
            "injectBifrostTools": False,
            "skillPaths": [],
            "deliveryMode": "final_reply",
        },
    },
    "channels": {},
}, "PATCH")
request("/providers", {
    "id": "feishu-menu-e2e",
    "provider_type": "feishu",
    "display_name": "Feishu Menu E2E",
    "enabled": True,
    "base_url": f"http://127.0.0.1:{feishu_port}/open-apis",
    "app_id": "cli_menu_e2e",
    "app_secret": "menu-secret",
    "owner_open_id": "ou_owner",
    "event_connection_enabled": False,
    "agent_config": {"runner": "menu-codex"},
})
request("/providers", {
    "id": "feishu-menu-history",
    "provider_type": "feishu",
    "display_name": "Historical Feishu Menu E2E",
    "enabled": True,
    "base_url": f"http://127.0.0.1:{feishu_port}/open-apis",
    "app_id": "cli_menu_history",
    "app_secret": "menu-secret",
    "owner_open_id": "ou_owner",
    "event_connection_enabled": True,
    "agent_config": {"runner": "menu-codex"},
})
PY

# The provider above was persisted while the service was already running. A
# process restart must discover it, reconcile the draft once, and then start
# its transport without requiring an explicit provider reconnect.
restart_bifrost

python3 - "$BIFROST_PORT" "$FEISHU_LOG" <<'PY'
import json
import pathlib
import sys
import time
import urllib.request

port, log_path = sys.argv[1:3]
base = f"http://127.0.0.1:{port}/_bifrost/api/im-gateway"
for _ in range(240):
    with urllib.request.urlopen(
        base + "/providers/feishu-menu-history/feishu/menu/status", timeout=30
    ) as response:
        status = json.loads(response.read().decode("utf-8"))
    requests = [
        json.loads(line)
        for line in pathlib.Path(log_path).read_text(encoding="utf-8").splitlines()
    ]
    application_requests = [
        row for row in requests if "/applications/cli_menu_history/" in row["path"]
    ]
    if status["state"]["status"] == "draft_applied" and len(application_requests) == 2:
        break
    time.sleep(0.05)
else:
    raise AssertionError((status, application_requests))
assert [row["method"] for row in application_requests] == ["PATCH", "PATCH"]
assert application_requests[0]["path"].endswith("/ability")
assert application_requests[1]["path"].endswith("/config")
assert not any(row["path"].endswith("/publish") for row in application_requests)
PY

restart_bifrost

python3 - "$BIFROST_PORT" "$FEISHU_LOG" <<'PY'
import json
import pathlib
import sys
import time
import urllib.request

port, log_path = sys.argv[1:3]
base = f"http://127.0.0.1:{port}/_bifrost/api/im-gateway"
for _ in range(240):
    with urllib.request.urlopen(
        base + "/providers/feishu-menu-history/status", timeout=30
    ) as response:
        connection = json.loads(response.read().decode("utf-8"))
    if connection.get("state") in {"connecting", "connected", "reconnecting"}:
        break
    time.sleep(0.05)
else:
    raise AssertionError(connection)
time.sleep(0.25)
requests = [
    json.loads(line)
    for line in pathlib.Path(log_path).read_text(encoding="utf-8").splitlines()
]
application_requests = [
    row for row in requests if "/applications/cli_menu_history/" in row["path"]
]
assert len(application_requests) == 2, application_requests
PY

PREVIEW="$("$BIFROST_BIN" -p "$BIFROST_PORT" im provider menu feishu-menu-e2e preview)"
DRAFT="$("$BIFROST_BIN" -p "$BIFROST_PORT" im provider menu feishu-menu-e2e sync)"
PUBLISHED="$("$BIFROST_BIN" -p "$BIFROST_PORT" im provider menu feishu-menu-e2e sync --publish)"
SKIPPED="$("$BIFROST_BIN" -p "$BIFROST_PORT" im provider menu feishu-menu-e2e sync --publish)"
STATUS="$("$BIFROST_BIN" -p "$BIFROST_PORT" im provider menu feishu-menu-e2e status)"

python3 - "$PREVIEW" "$DRAFT" "$PUBLISHED" "$SKIPPED" "$STATUS" "$FEISHU_LOG" <<'PY'
import json
import pathlib
import sys

preview, draft, published, skipped, status = map(json.loads, sys.argv[1:6])
requests = [json.loads(line) for line in pathlib.Path(sys.argv[6]).read_text(encoding="utf-8").splitlines()]
requests = [
    row
    for row in requests
    if row["path"].endswith("/auth/v3/tenant_access_token/internal")
    or "/applications/cli_menu_e2e/" in row["path"]
]
assert preview["preset"] == "bifrost-default-v1", preview
nodes = preview["ability"]["bot"]["bot_menus"]
roots = [node for node in nodes if "parent_menu_id" not in node]
assert [node["default_name"] for node in roots] == ["会话", "Agent", "工具"], roots
assert draft["ability_updated"] is True and draft["published"] is False, draft
assert published["ability_updated"] is False and published["published"] is True, published
assert skipped["skipped"] is True and skipped["published"] is True, skipped
assert status["state"]["status"] == "published", status
application_requests = [row for row in requests if "/applications/cli_menu_e2e/" in row["path"]]
assert len(application_requests) == 3, requests
ability, config, publish = application_requests
assert ability["method"] == "PATCH" and ability["path"].endswith("/ability"), ability
assert ability["authorization"] == "Bearer menu-e2e-token", ability
assert ability["body"]["bot"]["bot_menu_enable"] is True, ability
assert "enable" not in ability["body"]["bot"], ability
assert config["method"] == "PATCH" and config["path"].endswith("/config"), config
assert config["body"] == {"event": {"add_events": ["application.bot.menu_v6"]}}, config
assert publish["method"] == "POST" and publish["path"].endswith("/publish"), publish
assert publish["body"]["mobile_default_ability"] == "bot", publish
assert publish["body"]["pc_default_ability"] == "bot", publish
PY

python3 - "$BIFROST_PORT" <<'PY'
import json
import sys
import urllib.error
import urllib.request

port = sys.argv[1]
base = f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/debug/mock-feishu-menu"

def inject(event_key, event_id, operator="ou_owner", expected=200):
    request = urllib.request.Request(
        base,
        data=json.dumps({
            "providerId": "feishu-menu-e2e",
            "eventKey": event_key,
            "operatorOpenId": operator,
            "eventId": event_id,
            "timestamp": 1787000000000,
        }).encode("utf-8"),
        headers={"content-type": "application/json"},
        method="POST",
    )
    try:
        response = urllib.request.urlopen(request, timeout=30)
    except urllib.error.HTTPError as error:
        assert error.code == expected, error.read().decode("utf-8")
        return
    with response:
        body = json.loads(response.read().decode("utf-8"))
        assert response.status == expected, body
        return body

assert inject("bifrost.status", "menu-status")["command"] == "/status"
assert inject("bifrost.runner.select", "menu-runner")["command"] == "/runner"
assert inject("bifrost.fast.manage", "menu-fast")["command"] == "/fast status"
inject("unknown.external.command", "menu-unknown", expected=400)
assert inject("bifrost.help", "menu-intruder", operator="ou_intruder")["command"] == "/help"
PY

cards_ready=false
for _ in $(seq 1 240); do
  if [[ -f "$FEISHU_DRY_RUN" ]] && python3 - "$FEISHU_DRY_RUN" 2>/dev/null <<'PY'
import json
import pathlib
import sys
rows = [json.loads(line) for line in pathlib.Path(sys.argv[1]).read_text(encoding="utf-8").splitlines()]
dump = json.dumps(rows, ensure_ascii=False)
assert len(rows) >= 3
assert all(row["receiveIdType"] == "open_id" for row in rows)
assert all(row["receiveId"] == "ou_owner" for row in rows)
assert all(row["sourceMessageId"] is None for row in rows)
assert "当前 Runner" in dump and "/runner menu-traex" in dump
assert "Fast 模式" in dump and "/fast on" in dump and "/fast off" in dump
assert "Bifrost status" in dump
PY
  then
    cards_ready=true
    break
  fi
  sleep 0.05
done
if [[ "$cards_ready" != "true" ]]; then
  echo "[feishu-bot-menu] menu commands did not produce the expected P2P cards" >&2
  [[ -f "$FEISHU_DRY_RUN" ]] && cat "$FEISHU_DRY_RUN" >&2 || true
  tail -160 "$BIFROST_LOG" >&2 || true
  exit 1
fi

python3 - "$BIFROST_PORT" <<'PY'
import json
import sys
import time
import urllib.request

url = f"http://127.0.0.1:{sys.argv[1]}/_bifrost/api/im-gateway/providers/feishu-menu-e2e/messages"
for _ in range(240):
    with urllib.request.urlopen(url, timeout=30) as response:
        rows = json.loads(response.read().decode("utf-8"))
    rejected = [row for row in rows if row.get("event_id") == "menu-intruder"]
    if rejected:
        assert rejected[0]["status"] == "rejected", rejected[0]
        assert rejected[0]["content"] == "/help", rejected[0]
        assert rejected[0]["sender_open_id"] == "ou_intruder", rejected[0]
        break
    time.sleep(0.05)
else:
    raise AssertionError(f"missing rejected menu event: {rows}")
PY

echo "[feishu-bot-menu] PASS"
