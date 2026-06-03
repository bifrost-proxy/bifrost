#!/usr/bin/env bash
set -eo pipefail
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

PORT="${BIFROST_PORT:-18891}"
DATA_DIR="${BIFROST_DATA_DIR:-$ROOT_DIR/.bifrost-test-im-cli-provider}"
BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/release/bifrost}"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
  BIFROST_BIN="${BIFROST_BIN}.exe"
fi
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
        raw_bytes = self.rfile.read(length) if length else b""
        content_type = self.headers.get("content-type", "")
        if content_type.startswith("multipart/form-data"):
            body = {
                "multipart": True,
                "content_type": content_type,
                "length": len(raw_bytes),
            }
        else:
            raw = raw_bytes.decode("utf-8", errors="replace") if raw_bytes else ""
            try:
                body = json.loads(raw) if raw else {}
            except json.JSONDecodeError:
                body = {"raw": raw}
        with open(log_path, "a", encoding="utf-8") as f:
            f.write(json.dumps({"path": self.path, "body": body}, ensure_ascii=False) + "\n")
        if self.path.endswith("/auth/v3/tenant_access_token/internal"):
            payload = {"code": 0, "tenant_access_token": "tenant-token", "expire": 7200}
        elif self.path.startswith("/im/v1/images"):
            payload = {"code": 0, "data": {"image_key": "img_uploaded_cli"}}
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

if [[ ! -x "$BIFROST_BIN" ]]; then
  cargo build --bin bifrost
  BIFROST_BIN="$ROOT_DIR/target/debug/bifrost"
fi

BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" start \
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

"$BIFROST_BIN" -p "$PORT" im provider add feishu-main \
  --type feishu \
  --app-id cli_app \
  --secret cli_secret \
  --base-url "http://127.0.0.1:$MOCK_PORT" \
  --display-name "Feishu Main" \
  --owner-open-id owner-open-id \
  --enabled true

SEND_OUTPUT="$("$BIFROST_BIN" -p "$PORT" im send --text 'hello owner from cli')"
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
assert send["body"]["msg_type"] == "interactive", send
card = json.loads(send["body"]["content"])
assert card["schema"] == "2.0", card
assert card["header"]["title"]["content"] == "Bifrost", card
assert card["body"]["elements"][0]["tag"] == "markdown", card
assert card["body"]["elements"][0]["content"] == "hello owner from cli", card
PY

python3 - "$MOCK_DIR/pixel.png" <<'PY'
import base64
import sys

data = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/p9sAAAAASUVORK5CYII="
open(sys.argv[1], "wb").write(base64.b64decode(data))
PY

IMAGE_OUTPUT="$("$BIFROST_BIN" -p "$PORT" im send --image-file "$MOCK_DIR/pixel.png")"
echo "$IMAGE_OUTPUT"
grep -q "Message sent" <<<"$IMAGE_OUTPUT"

CARD_OUTPUT="$("$BIFROST_BIN" -p "$PORT" im send --card-title 'Deploy report' --card-text '**Done** with chart' --card-image-key img_v3_chart)"
echo "$CARD_OUTPUT"
grep -q "Message sent" <<<"$CARD_OUTPUT"

CARD_FILE_OUTPUT="$("$BIFROST_BIN" -p "$PORT" im send --card-title 'Uploaded chart' --card-text 'Chart uploaded' --card-image-file "$MOCK_DIR/pixel.png")"
echo "$CARD_FILE_OUTPUT"
grep -q "Message sent" <<<"$CARD_FILE_OUTPUT"

RAW_CARD_OUTPUT="$("$BIFROST_BIN" -p "$PORT" im send --card-json '{"config":{},"elements":[],"header":{"title":{"tag":"plain_text","content":"Raw card"}}}')"
echo "$RAW_CARD_OUTPUT"
grep -q "Message sent" <<<"$RAW_CARD_OUTPUT"

python3 - "$MOCK_LOG" <<'PY'
import json
import sys

records = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8")]
uploads = [r for r in records if r["path"].startswith("/im/v1/images")]
assert uploads, "mock did not receive image upload request"
sends = [r for r in records if r["path"].startswith("/im/v1/messages")]
image_send = next((r for r in sends if r["body"]["msg_type"] == "image"), None)
assert image_send is not None, sends
assert json.loads(image_send["body"]["content"])["image_key"] == "img_uploaded_cli", image_send
card_send = next((
    r for r in sends
    if r["body"]["msg_type"] == "interactive"
    and json.loads(r["body"]["content"]).get("header", {}).get("title", {}).get("content") == "Deploy report"
), None)
assert card_send is not None, sends
card = json.loads(card_send["body"]["content"])
assert card["header"]["title"]["content"] == "Deploy report", card
assert card["elements"][0]["tag"] == "img", card
assert card["elements"][0]["img_key"] == "img_v3_chart", card
assert card["elements"][1]["tag"] == "markdown", card
assert card["elements"][1]["content"] == "**Done** with chart", card
uploaded_card_send = next((
    r for r in sends
    if r["body"]["msg_type"] == "interactive"
    and json.loads(r["body"]["content"]).get("header", {}).get("title", {}).get("content") == "Uploaded chart"
), None)
assert uploaded_card_send is not None, sends
uploaded_card = json.loads(uploaded_card_send["body"]["content"])
assert uploaded_card["elements"][0]["tag"] == "img", uploaded_card
assert uploaded_card["elements"][0]["img_key"] == "img_uploaded_cli", uploaded_card
assert uploaded_card["elements"][1]["content"] == "Chart uploaded", uploaded_card
raw_card_send = next((
    r for r in sends
    if r["body"]["msg_type"] == "interactive"
    and json.loads(r["body"]["content"]).get("header", {}).get("title", {}).get("content") == "Raw card"
), None)
assert raw_card_send is not None, sends
assert raw_card_send["body"]["msg_type"] == "interactive", raw_card_send
raw_card = json.loads(raw_card_send["body"]["content"])
assert raw_card["header"]["title"]["content"] == "Raw card", raw_card
assert raw_card["elements"] == [], raw_card
PY

LOGS="$("$BIFROST_BIN" -p "$PORT" im messages list)"
echo "$LOGS"
grep -q "Owner" <<<"$LOGS"
grep -q "hello owner" <<<"$LOGS"

echo "[im-cli-provider-selection] passed"
