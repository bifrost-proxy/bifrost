#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_DIR"

BIFROST_PORT="${BIFROST_PORT:-${ADMIN_PORT:-18937}}"
TEST_DIR="$(mktemp -d)"
BIFROST_LOG="$TEST_DIR/bifrost.log"
BIFROST_BIN="${BIFROST_BIN:-}"

cleanup() {
  if [[ -n "${BIFROST_PID:-}" ]]; then
    kill "$BIFROST_PID" >/dev/null 2>&1 || true
    wait "$BIFROST_PID" >/dev/null 2>&1 || true
  fi
  if [[ -n "${MOCK_PID:-}" ]]; then
    kill "$MOCK_PID" >/dev/null 2>&1 || true
    wait "$MOCK_PID" >/dev/null 2>&1 || true
  fi
  rm -rf "$TEST_DIR"
}
trap cleanup EXIT

wait_http() {
  local url="$1"
  local label="$2"
  for _ in $(seq 1 120); do
    if curl -fsS --noproxy '*' "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.25
  done
  echo "[weixin-provider] $label did not become ready" >&2
  if [[ -f "$BIFROST_LOG" ]]; then
    tail -100 "$BIFROST_LOG" >&2 || true
  fi
  return 1
}

start_bifrost() {
  echo "[weixin-provider] starting bifrost on $BIFROST_PORT with data dir $TEST_DIR"
  BIFROST_DATA_DIR="$TEST_DIR" "$BIFROST_BIN" start \
    --host 127.0.0.1 \
    -p "$BIFROST_PORT" \
    --unsafe-ssl \
    --skip-cert-check \
    --no-system-proxy \
    >"$BIFROST_LOG" 2>&1 &
  BIFROST_PID=$!
  wait_http "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address" "bifrost"
}

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/release/bifrost}"
  echo "[weixin-provider] skipping build, using $BIFROST_BIN"
else
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
  echo "[weixin-provider] building bifrost"
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

MOCK_SERVER="$TEST_DIR/weixin_mock.py"
MOCK_PORT_FILE="$TEST_DIR/weixin_mock_port"
MOCK_SEND_LOG="$TEST_DIR/weixin_mock_send.log"
cat >"$MOCK_SERVER" <<'PY'
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port_file = sys.argv[1]
send_log = sys.argv[2]

class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        return

    def _json(self, status, payload):
        body = json.dumps(payload).encode("utf-8")
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        host = self.headers.get("host")
        if self.path.startswith("/ilink/bot/get_bot_qrcode"):
            self._json(200, {
                "ret": 0,
                "qrcode": "mock-qr-key",
                "qrcode_img_content": f"http://{host}/scan/mock-qr-key?bot_type=3",
            })
            return
        if self.path.startswith("/ilink/bot/get_qrcode_status"):
            self._json(200, {
                "ret": 0,
                "status": "confirmed",
                "bot_token": "mock-token",
                "ilink_bot_id": "mock-bot@im.bot",
                "ilink_user_id": "mock-user@im.wechat",
                "baseurl": f"http://{host}",
            })
            return
        if self.path.startswith("/cdn/image.png"):
            body = b"\x89PNG\r\n\x1a\nmock-image"
            self.send_response(200)
            self.send_header("content-type", "image/png")
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        self._json(404, {"error": "not found"})

    def do_POST(self):
        length = int(self.headers.get("content-length") or 0)
        body = self.rfile.read(length) if length else b"{}"
        if self.path.startswith("/ilink/bot/getupdates"):
            if self.headers.get("AuthorizationType") != "ilink_bot_token":
                self._json(401, {"error": "missing AuthorizationType"})
                return
            if self.headers.get("Authorization") != "Bearer mock-token":
                self._json(401, {"error": "missing Authorization"})
                return
            try:
                request_payload = json.loads(body.decode("utf-8") or "{}")
            except Exception:
                request_payload = {}
            if not request_payload.get("get_updates_buf"):
                self._json(200, {
                    "sync_buf": "next-buf",
                    "msgs": [{
                        "message_id": 7459890488013826184,
                        "from_user_id": "mock-user@im.wechat",
                        "message_type": 1,
                        "item_list": [{
                            "type": 1,
                            "text_item": {"text": "hello from weixin provider"},
                        }],
                        "context_token": "ctx-1",
                    }, {
                        "message_id": 7459890488013826185,
                        "from_user_id": "mock-user@im.wechat",
                        "message_type": 1,
                        "item_list": [{
                            "type": 2,
                            "msg_id": "mock-image-1",
                            "image_item": {
                                "media": {
                                    "full_url": f"http://{self.headers.get('host')}/cdn/image.png"
                                },
                                "thumb_width": 16,
                                "thumb_height": 16
                            },
                        }],
                    }],
                })
            else:
                self._json(200, {"sync_buf": "next-buf", "msgs": []})
            return
        if self.path.startswith("/ilink/bot/sendmessage"):
            if self.headers.get("AuthorizationType") != "ilink_bot_token":
                self._json(401, {"error": "missing AuthorizationType"})
                return
            if self.headers.get("Authorization") != "Bearer mock-token":
                self._json(401, {"error": "missing Authorization"})
                return
            with open(send_log, "ab") as f:
                f.write(body + b"\n")
            self._json(200, {"message_id": "sent-mock-1", "request_id": "req-mock-1"})
            return
        self._json(404, {"error": "not found"})

server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
with open(port_file, "w", encoding="utf-8") as f:
    f.write(str(server.server_address[1]))
server.serve_forever()
PY
python3 "$MOCK_SERVER" "$MOCK_PORT_FILE" "$MOCK_SEND_LOG" &
MOCK_PID=$!
for _ in $(seq 1 80); do
  [[ -s "$MOCK_PORT_FILE" ]] && break
  sleep 0.1
done
MOCK_PORT="$(cat "$MOCK_PORT_FILE")"
MOCK_BASE_URL="http://127.0.0.1:$MOCK_PORT"

start_bifrost

IM_BASE="http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway"
curl -fsS --noproxy '*' -X POST "$IM_BASE/providers" \
  -H 'Content-Type: application/json' \
  -d "{\"id\":\"weixin-mock\",\"provider_type\":\"weixin\",\"display_name\":\"Weixin Mock\",\"enabled\":true,\"base_url\":\"$MOCK_BASE_URL\",\"event_connection_enabled\":true,\"event_types\":[\"message.receive\"]}" >/dev/null

LOGIN_START="$(curl -fsS --noproxy '*' -X POST "$IM_BASE/providers/weixin-mock/weixin-login/start" -H 'Content-Type: application/json' -d '{}')"
python3 - "$LOGIN_START" <<'PY'
import json
import sys

payload = json.loads(sys.argv[1])
if not payload.get("success"):
    raise SystemExit("weixin login start failed")
if not payload.get("scan_url", "").startswith("http://127.0.0.1:"):
    raise SystemExit(f"expected scan url, got {payload}")
if "mock-qr-key" in payload:
    raise SystemExit("poll key must not be exposed by provider API")
PY

LOGIN_STATUS="$(curl -fsS --noproxy '*' "$IM_BASE/providers/weixin-mock/weixin-login/status")"
python3 - "$LOGIN_STATUS" <<'PY'
import json
import sys

payload = json.loads(sys.argv[1])
if payload.get("status") not in {"confirmed", "authorized"}:
    raise SystemExit(f"login status did not auto-check QR state: {payload}")
if "provider" in payload and "secret_ref" in payload["provider"]:
    raise SystemExit("status response leaked secret_ref")
PY

python3 - "$LOGIN_STATUS" <<'PY'
import json
import sys

payload = json.loads(sys.argv[1])
provider = payload.get("provider", {})
if provider.get("provider_type") != "weixin":
    raise SystemExit(f"unexpected provider type: {provider}")
if provider.get("app_id") != "mock-bot@im.bot":
    raise SystemExit(f"unexpected app_id: {provider}")
if provider.get("owner_open_id") != "mock-user@im.wechat":
    raise SystemExit(f"unexpected owner: {provider}")
if not provider.get("secret_configured"):
    raise SystemExit("provider token was not configured")
if "secret_ref" in provider:
    raise SystemExit("provider response leaked secret_ref")
PY

curl -fsS --noproxy '*' -X POST "$IM_BASE/providers/weixin-mock/connect" >/dev/null
EVENTS=""
for _ in $(seq 1 40); do
  EVENTS="$(curl -fsS --noproxy '*' "$IM_BASE/history/events")"
  if python3 - "$EVENTS" <<'PY'
import json
import sys
events = json.loads(sys.argv[1])
raise SystemExit(0 if any(e.get("provider_id") == "weixin-mock" and e.get("event_id") == "7459890488013826185" for e in events) else 1)
PY
  then
    break
  fi
  sleep 0.25
done

python3 - "$EVENTS" <<'PY'
import json
import sys
events = json.loads(sys.argv[1])
text_matches = [e for e in events if e.get("provider_id") == "weixin-mock" and e.get("event_id") == "7459890488013826184"]
if len(text_matches) != 1:
    raise SystemExit(f"expected one weixin provider text event, got {len(text_matches)}")
event = text_matches[0]
if event.get("provider_type") != "weixin":
    raise SystemExit(f"unexpected provider type: {event}")
if event.get("message", {}).get("text") != "hello from weixin provider":
    raise SystemExit(f"unexpected event text: {event}")
image_matches = [e for e in events if e.get("provider_id") == "weixin-mock" and e.get("event_id") == "7459890488013826185"]
if len(image_matches) != 1:
    raise SystemExit(f"expected one weixin image event, got {len(image_matches)}")
image_event = image_matches[0]
images = image_event.get("message", {}).get("images") or []
if len(images) != 1:
    raise SystemExit(f"expected one image attachment, got {image_event}")
if images[0].get("file_key") != "mock-image-1":
    raise SystemExit(f"unexpected image file key: {images}")
if not images[0].get("download_url", "").endswith("/cdn/image.png"):
    raise SystemExit(f"unexpected image download url: {images}")
PY

curl -fsS --noproxy '*' -X POST "$IM_BASE/messages/send" \
  -H 'Content-Type: application/json' \
  -d '{"provider_id":"weixin-mock","target_id":"__owner__","msg_type":"text","text":"provider send ok"}' >/dev/null
python3 - "$MOCK_SEND_LOG" <<'PY'
import json
import pathlib
import sys

lines = [line for line in pathlib.Path(sys.argv[1]).read_text().splitlines() if line.strip()]
if not lines:
    raise SystemExit("mock sendmessage was not called")
payloads = [json.loads(line) for line in lines]
msgs = [payload.get("msg", {}) for payload in payloads]
if not any(msg.get("to_user_id") == "mock-user@im.wechat" for msg in msgs):
    raise SystemExit(f"sendmessage did not target mock owner: {payloads}")
if not any(
    (msg.get("item_list") or [{}])[0].get("text_item", {}).get("text") == "provider send ok"
    for msg in msgs
):
    raise SystemExit(f"manual provider send text missing: {payloads}")
if not any(msg.get("context_token") == "ctx-1" for msg in msgs):
    raise SystemExit(f"sendmessage did not include stored context_token: {payloads}")
if not all(msg.get("message_type") == 2 and msg.get("message_state") == 2 for msg in msgs):
    raise SystemExit(f"sendmessage did not use BOT/FINISH message shape: {payloads}")
if not all(payload.get("base_info", {}).get("channel_version") == "1.0.3" for payload in payloads):
    raise SystemExit(f"sendmessage missing base_info channel_version: {payloads}")
PY

echo "[weixin-provider] passed"
