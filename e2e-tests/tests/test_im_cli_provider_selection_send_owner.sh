#!/usr/bin/env zsh
set -eo pipefail

source ~/.zshrc

ROOT_DIR="$(cd "${0:A:h}/../.." && pwd)"
cd "$ROOT_DIR"

PORT="${BIFROST_PORT:-18891}"
DATA_DIR="${BIFROST_DATA_DIR:-$ROOT_DIR/.bifrost-test-im-cli-provider}"
MOCK_DIR="$(mktemp -d)"
MOCK_LOG="$MOCK_DIR/requests.ndjson"
SERVER_LOG="$MOCK_DIR/bifrost.log"
MOCK_PORT_FILE="$MOCK_DIR/mock_port"

cleanup() {
  if [[ -n "${BIFROST_PID:-}" ]]; then
    kill "$BIFROST_PID" >/dev/null 2>&1 || true
    wait "$BIFROST_PID" >/dev/null 2>&1 || true
  fi
  if [[ -n "${MOCK_PID:-}" ]]; then
    kill "$MOCK_PID" >/dev/null 2>&1 || true
    wait "$MOCK_PID" >/dev/null 2>&1 || true
  fi
  rm -rf "$DATA_DIR" "$MOCK_DIR"
}
trap cleanup EXIT

rm -rf "$DATA_DIR"
mkdir -p "$DATA_DIR"

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
        raw = self.rfile.read(length).decode("utf-8") if length else ""
        try:
            body = json.loads(raw) if raw else {}
        except json.JSONDecodeError:
            body = {"raw": raw}
        with open(log_path, "a", encoding="utf-8") as f:
            f.write(json.dumps({"path": self.path, "body": body}, ensure_ascii=False) + "\n")
        if self.path.endswith("/auth/v3/tenant_access_token/internal"):
            payload = {"code": 0, "tenant_access_token": "tenant-token", "expire": 7200}
        elif self.path.startswith("/im/v1/messages"):
            payload = {"code": 0, "data": {"message_id": "om_owner_cli"}}
        else:
            payload = {"code": 404, "msg": "not found"}
        data = json.dumps(payload).encode("utf-8")
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("x-tt-logid", "mock-logid")
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

for _ in {1..50}; do
  [[ -s "$MOCK_PORT_FILE" ]] && break
  sleep 0.1
done
MOCK_PORT="$(cat "$MOCK_PORT_FILE")"

BIFROST_DATA_DIR="$DATA_DIR" cargo run --bin bifrost -- start \
  -p "$PORT" \
  --unsafe-ssl \
  --no-system-proxy \
  --skip-cert-check \
  >"$SERVER_LOG" 2>&1 &
BIFROST_PID=$!

for _ in {1..80}; do
  if curl -fsS "http://127.0.0.1:$PORT/_bifrost/api/im-gateway/providers" >/dev/null 2>&1; then
    break
  fi
  sleep 0.25
done

cargo run --bin bifrost -- -p "$PORT" im provider add feishu-main \
  --type feishu \
  --app-id cli_app \
  --secret cli_secret \
  --base-url "http://127.0.0.1:$MOCK_PORT" \
  --display-name "Feishu Main" \
  --owner-open-id owner-open-id \
  --enabled true

SEND_OUTPUT="$(cargo run --bin bifrost -- -p "$PORT" im send --text 'hello owner from cli')"
echo "$SEND_OUTPUT"
grep -q "Message sent" <<<"$SEND_OUTPUT"
grep -q "om_owner_cli" <<<"$SEND_OUTPUT"

python3 - "$MOCK_LOG" <<'PY'
import json
import sys

records = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8")]
send = next((r for r in records if r["path"].startswith("/im/v1/messages")), None)
assert send is not None, "mock did not receive send request"
assert send["path"].endswith("receive_id_type=open_id"), send
assert send["body"]["receive_id"] == "owner-open-id", send
assert send["body"]["msg_type"] == "text", send
assert json.loads(send["body"]["content"])["text"] == "hello owner from cli", send
PY

LOGS="$(cargo run --bin bifrost -- -p "$PORT" im messages list)"
echo "$LOGS"
grep -q "Owner" <<<"$LOGS"
grep -q "hello owner" <<<"$LOGS"

echo "[im-cli-provider-selection] passed"
