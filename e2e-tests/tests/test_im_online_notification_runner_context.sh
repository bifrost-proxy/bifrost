#!/usr/bin/env bash
set -euo pipefail
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-im-online-context.XXXXXX")"
DATA_DIR="$TEST_DIR/data"
SERVER_LOG="$TEST_DIR/bifrost.log"
MOCK_LOG="$TEST_DIR/feishu_requests.ndjson"
MOCK_PORT_FILE="$TEST_DIR/mock_port"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT_DIR/.bifrost-e2e-target}"
export CARGO_TARGET_DIR

cleanup() {
  local status=$?
  if [[ "$status" -ne 0 && -f "$SERVER_LOG" ]]; then
    echo "---- bifrost test server log ----" >&2
    cat "$SERVER_LOG" >&2 || true
    echo "---- end bifrost test server log ----" >&2
  fi
  if [[ -n "${BIFROST_PID:-}" ]] && kill -0 "$BIFROST_PID" 2>/dev/null; then
    kill "$BIFROST_PID" 2>/dev/null || true
    wait "$BIFROST_PID" 2>/dev/null || true
  fi
  if [[ -n "${MOCK_PID:-}" ]] && kill -0 "$MOCK_PID" 2>/dev/null; then
    kill "$MOCK_PID" 2>/dev/null || true
    wait "$MOCK_PID" 2>/dev/null || true
  fi
  rm -rf "$TEST_DIR"
}
trap cleanup EXIT

mkdir -p "$DATA_DIR/admin" "$DATA_DIR/agent/sessions/2026/06/04"

python3 - "$MOCK_LOG" "$MOCK_PORT_FILE" <<'PY' &
import http.server
import json
import socketserver
import sys

log_path = sys.argv[1]
port_file = sys.argv[2]

class Handler(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("content-length", "0"))
        raw = self.rfile.read(length) if length else b""
        try:
            body = json.loads(raw.decode("utf-8")) if raw else {}
        except Exception:
            body = {"raw": raw.decode("utf-8", errors="replace")}
        with open(log_path, "a", encoding="utf-8") as f:
            f.write(json.dumps({"path": self.path, "body": body}, ensure_ascii=False) + "\n")
        if self.path.endswith("/auth/v3/tenant_access_token/internal"):
            payload = {"code": 0, "tenant_access_token": "tenant-token", "expire": 7200}
        elif self.path.startswith("/callback/ws/endpoint"):
            payload = {
                "code": 0,
                "data": {
                    "URL": "ws://127.0.0.1:9/mock?service_id=42",
                    "ClientConfig": {"PingInterval": 60},
                },
            }
        elif self.path.startswith("/im/v1/messages") or self.path.startswith("/open-apis/im/v1/messages"):
            payload = {"code": 0, "data": {"message_id": "om_online_context"}}
        else:
            payload = {"code": 404, "msg": "not found"}
        data = json.dumps(payload).encode("utf-8")
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *_args):
        pass

with socketserver.TCPServer(("127.0.0.1", 0), Handler) as httpd:
    with open(port_file, "w", encoding="utf-8") as f:
        f.write(str(httpd.server_address[1]))
    httpd.serve_forever()
PY
MOCK_PID=$!

for _ in $(seq 1 50); do
  [[ -s "$MOCK_PORT_FILE" ]] && break
  sleep 0.1
done
MOCK_PORT="$(cat "$MOCK_PORT_FILE")"

cat >"$DATA_DIR/admin/im_gateway_external_cli_agent.json" <<'JSON'
{
  "version": 1,
  "defaultRunnerId": "codex-default",
  "runners": {
    "codex-default": {"enabled": true, "adapter": "codex"},
    "web-main": {"enabled": true, "adapter": "chatgpt_web", "adapterConfig": {"auth": {"statePath": "startup-auth-e2e.json"}}}
  },
  "channels": {}
}
JSON

cat >"$DATA_DIR/agent/sessions/2026/06/04/session-feishu-main_ou_owner-1780017999.jsonl" <<'JSONL'
{"timestamp":1780017900,"event_type":"session_start","session_key":"feishu-main:ou_owner","content":{"source":"im","work_dir":"/tmp/im-provider-workdir"}}
{"timestamp":1780017901,"event_type":"user_message","session_key":"feishu-main:ou_owner","content":{"message":"first user"}}
{"timestamp":1780017902,"event_type":"assistant_message","session_key":"feishu-main:ou_owner","content":{"message":"first answer"}}
{"timestamp":1780017903,"event_type":"user_message","session_key":"feishu-main:ou_owner","content":{"message":"second user"}}
JSONL

if [[ "${SKIP_BUILD:-false}" != "true" ]]; then
  cargo build --bin bifrost >/dev/null
  BIFROST_BIN="$CARGO_TARGET_DIR/debug/bifrost"
else
  BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/release/bifrost}"
fi
if [[ ! -x "$BIFROST_BIN" ]]; then
  echo "bifrost binary is not executable: $BIFROST_BIN" >&2
  exit 1
fi

PORT="$(python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
)"

BIFROST_DATA_DIR="$DATA_DIR" BIFROST_DEVICE_NAME="im-online-e2e" BIFROST_CHATGPT_WEB_STARTUP_AUTH_DRY_RUN=1 "$BIFROST_BIN" start \
  -p "$PORT" \
  --unsafe-ssl \
  --no-system-proxy \
  --skip-cert-check \
  --yes >"$SERVER_LOG" 2>&1 &
BIFROST_PID=$!

for _ in $(seq 1 80); do
  if curl --noproxy '*' -fsS "http://127.0.0.1:$PORT/_bifrost/api/system/overview" >/dev/null 2>&1; then
    break
  fi
  sleep 0.25
done

curl --noproxy '*' -fsS -X POST "http://127.0.0.1:$PORT/_bifrost/api/im-gateway/providers" \
  -H 'Content-Type: application/json' \
  -d "{
    \"id\": \"feishu-main\",
    \"provider_type\": \"feishu\",
    \"display_name\": \"Feishu Main\",
    \"enabled\": true,
    \"base_url\": \"http://127.0.0.1:$MOCK_PORT/open-apis\",
    \"app_id\": \"cli_online_context\",
    \"app_secret\": \"cli_secret\",
    \"owner_open_id\": \"ou_owner\",
    \"event_connection_enabled\": false,
    \"agent_config\": {
      \"runner\": \"web-main\",
      \"work_dir\": \"/tmp/im-provider-workdir\"
    }
  }" >/dev/null

curl --noproxy '*' -fsS -X POST "http://127.0.0.1:$PORT/_bifrost/api/im-gateway/providers/feishu-main/connect" >/dev/null

MESSAGES_JSON="$TEST_DIR/messages.json"
for _ in $(seq 1 80); do
  curl --noproxy '*' -fsS "http://127.0.0.1:$PORT/_bifrost/api/im-gateway/providers/feishu-main/messages?direction=outbound&limit=20" >"$MESSAGES_JSON"
  if python3 - "$MESSAGES_JSON" <<'PY'
import json, sys
messages = json.load(open(sys.argv[1], encoding="utf-8"))
if any((m.get("trigger") == "online" and m.get("status") == "success") for m in messages):
    raise SystemExit(0)
raise SystemExit(1)
PY
  then
    break
  fi
  sleep 0.25
done

python3 - "$MESSAGES_JSON" <<'PY'
import json
import sys

messages = json.load(open(sys.argv[1], encoding="utf-8"))
online = next((m for m in messages if m.get("trigger") == "online"), None)
assert online is not None, messages
assert online.get("status") == "success", online
assert online.get("msg_type") == "interactive", online
content = online.get("content_preview") or ""
assert "**Bifrost is online**" in content, content
assert "- **Provider**: Feishu Main (`feishu-main`)" in content, content
assert "- **Device**: im-online-e2e" in content, content
assert "- **Workspace**: `/tmp/im-provider-workdir`" in content, content
assert "- **Runner Type**: `chatgpt_web`" in content, content
assert "- **Runner ID**: `web-main`" in content, content
assert "- **Model**: `N/A`" in content, content
assert "- **Reasoning Effort**: `N/A`" in content, content
assert "- **Reasoning Summary**: `N/A`" in content, content
assert "- **Bound Session**: `feishu-main:ou_owner`" in content, content
assert "- **Completed User Turns**: 2" in content, content
assert "- **Status**: Ready" in content, content
PY

python3 - "$MOCK_LOG" <<'PY'
import json
import sys
records = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8")]
send = next((r for r in records if r["path"].startswith("/open-apis/im/v1/messages")), None)
assert send is not None, records
assert send["body"]["receive_id"] == "ou_owner", send
assert send["body"]["msg_type"] == "interactive", send
card = json.loads(send["body"]["content"])
markdown = card["body"]["elements"][0]["content"]
assert "Runner Type" in markdown and "chatgpt_web" in markdown, markdown
assert "Model" in markdown and "N/A" in markdown, markdown
assert "Reasoning Effort" in markdown and "N/A" in markdown, markdown
assert "Reasoning Summary" in markdown and "N/A" in markdown, markdown
assert "Completed User Turns" in markdown and "2" in markdown, markdown
PY

echo "[im-online-notification-runner-context] PASS"
