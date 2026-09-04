#!/usr/bin/env bash
set -euo pipefail

export BIFROST_DISABLE_TRAY=1
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1

unset BIFROST_DETACHED_DAEMON_CHILD BIFROST_EXTERNAL_CLI_WORKER
export CARGO_NET_OFFLINE="${CARGO_NET_OFFLINE:-true}"

# Fail closed for network access. This scenario is self-contained and must only
# reach its loopback Feishu fixture and loopback Bifrost API. Any accidental
# public request is sent to a closed local port instead of reaching the network.
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

# CI's broad shell matrix intentionally reuses the release artifact. Release
# builds keep Feishu's API host allowlist closed even when the debug-only E2E
# loopback flag is present, so a fake local OpenAPI cannot exercise this flow.
# The real black-box path runs with the debug binary in the focused E2E/human
# test, while release CI still selects this script and records the security
# boundary instead of waiting for a request that must never reach loopback.
case "${BIFROST_BIN//\\//}" in
  target/release/bifrost|*/target/release/bifrost|target/release/bifrost.exe|*/target/release/bifrost.exe)
    echo "[feishu-new-group-command] SKIP fake OpenAPI: release build rejects Feishu loopback by design"
    exit 0
    ;;
esac

TEST_DIR="$(mktemp -d)"
BIFROST_LOG="$TEST_DIR/bifrost.log"
REQUEST_LOG="$TEST_DIR/feishu-requests.jsonl"
BIFROST_PORT="${BIFROST_PORT:-$(python3 - <<'PY'
import socket
with socket.socket() as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
)}"
FEISHU_PORT="$(python3 - <<'PY'
import socket
with socket.socket() as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
)"
START_EXTRA_ARGS=()
[[ "$(uname -s)" == "Linux" ]] || START_EXTRA_ARGS+=(--no-tray)

cleanup() {
  for pid in "${BIFROST_PID:-}" "${FEISHU_PID:-}"; do
    [[ -z "$pid" ]] || kill "$pid" >/dev/null 2>&1 || true
    [[ -z "$pid" ]] || wait "$pid" >/dev/null 2>&1 || true
  done
  [[ "${KEEP_TEST_DIR:-false}" == "true" ]] || rm -rf "$TEST_DIR"
}
trap cleanup EXIT

python3 - "$FEISHU_PORT" "$REQUEST_LOG" <<'PY' &
import http.server, json, sys, urllib.parse

port, log_path = int(sys.argv[1]), sys.argv[2]
class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_): pass
    def do_POST(self):
        length = int(self.headers.get("content-length", "0"))
        raw = self.rfile.read(length)
        body = json.loads(raw or b"{}")
        with open(log_path, "a", encoding="utf-8") as log:
            log.write(json.dumps({"path": self.path, "body": body}, ensure_ascii=False) + "\n")
        path = urllib.parse.urlsplit(self.path).path
        if path.endswith("/auth/v3/tenant_access_token/internal"):
            response = {"code": 0, "tenant_access_token": "token", "expire": 7200}
        elif path.endswith("/im/v1/chats"):
            response = {"code": 0, "data": {"chat_id": "oc_created", "name": body["name"]}}
        elif path.endswith("/im/v1/messages/om_new/reply") or path.endswith("/im/v1/messages/om_denied/reply") or path.endswith("/im/v1/messages/om_help/reply"):
            response = {"code": 0, "data": {"message_id": "om_reply"}}
        elif path.endswith("/im/v1/messages"):
            response = {"code": 0, "data": {"message_id": "om_welcome"}}
        else:
            response = {"code": 0}
        encoded = json.dumps(response).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)
http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
FEISHU_PID=$!

if [[ "${SKIP_BUILD:-false}" != "true" ]]; then
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi
BIFROST_E2E_ALLOW_FEISHU_LOOPBACK_BASE_URL=1 BIFROST_DATA_DIR="$TEST_DIR" "$BIFROST_BIN" start --host 127.0.0.1 -p "$BIFROST_PORT" \
  --unsafe-ssl --skip-cert-check --no-system-proxy "${START_EXTRA_ARGS[@]}" >"$BIFROST_LOG" 2>&1 &
BIFROST_PID=$!
for _ in $(seq 1 180); do
  curl -fsS --noproxy '*' "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address" >/dev/null 2>&1 && break
  sleep 0.25
done

python3 - "$BIFROST_PORT" "$FEISHU_PORT" <<'PY'
import json, sys, urllib.request
port, feishu_port = sys.argv[1:3]
base = f"http://127.0.0.1:{port}/_bifrost/api/im-gateway"
def post(path, payload):
    request = urllib.request.Request(base + path, data=json.dumps(payload, ensure_ascii=False).encode(), headers={"content-type":"application/json"}, method="POST")
    with urllib.request.urlopen(request, timeout=30) as response: assert response.status == 200
post("/providers", {"id":"feishu-new-e2e","provider_type":"feishu","display_name":"Feishu New E2E","enabled":True,"base_url":f"http://127.0.0.1:{feishu_port}/open-apis","app_id":"cli_new_e2e","app_secret":"secret","owner_open_id":"ou_owner","event_connection_enabled":False})
def inject(user, message_id, text, chat_type="p2p", mention=False):
    post("/debug/mock-inbound", {"providerId":"feishu-new-e2e","chatId":"oc_source","chatType":chat_type,"userId":user,"messageId":message_id,"eventId":"event-"+message_id,"text":text,"mentionBot":mention})
inject("ou_owner", "om_new", "/new 发布 项目群")
inject("ou_owner", "om_new", "/new 发布 项目群")
inject("ou_member", "om_denied", "/new 越权群", "group")
inject("ou_owner", "om_help", "/help")
PY

for _ in $(seq 1 120); do
  [[ -f "$REQUEST_LOG" ]] && [[ "$(grep -c '/im/v1/chats?' "$REQUEST_LOG" || true)" == "1" ]] && \
    grep -q '/new <群名>' "$TEST_DIR/admin/im_gateway_message_logs.json" 2>/dev/null && break
  sleep 0.25
done

python3 - "$REQUEST_LOG" "$TEST_DIR/admin/im_group_context.db" "$TEST_DIR/admin/im_gateway_message_logs.json" <<'PY'
import json, sqlite3, sys, urllib.parse
request_log, db_path, messages_path = sys.argv[1:4]
requests = [json.loads(line) for line in open(request_log, encoding="utf-8") if line.strip()]
creates = [item for item in requests if urllib.parse.urlsplit(item["path"]).path.endswith("/im/v1/chats")]
assert len(creates) == 1, creates
create = creates[0]
query = urllib.parse.parse_qs(urllib.parse.urlsplit(create["path"]).query)
assert query["user_id_type"] == ["open_id"]
assert query["set_bot_manager"] == ["true"]
assert len(query["uuid"][0]) > 20
assert create["body"] == {"name":"发布 项目群","owner_id":"ou_owner","chat_mode":"group","chat_type":"private"}, create
welcomes = [item for item in requests if urllib.parse.urlsplit(item["path"]).path.endswith("/im/v1/messages")]
assert len(welcomes) == 1 and welcomes[0]["body"]["receive_id"] == "oc_created", welcomes
connection = sqlite3.connect(db_path)
rows = connection.execute("SELECT source_message_id, group_name, chat_id, owner_open_id FROM im_feishu_new_groups").fetchall()
assert rows == [("om_new", "发布 项目群", "oc_created", "ou_owner")], rows
messages = json.load(open(messages_path, encoding="utf-8"))["messages"]
contents = [entry.get("content", "") for entry in messages if entry.get("direction") == "outbound"]
assert any("未重复创建" in content for content in contents), contents
assert any("只有当前飞书 Provider 的 owner" in content for content in contents), contents
help_text = next(content for content in contents if "/new <群名>" in content)
assert "仅 Provider owner" in help_text, help_text
PY

echo "[feishu-new-group-command] PASS"
