#!/usr/bin/env bash
set -euo pipefail
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_DIR"

BIFROST_PORT="${BIFROST_PORT:-${ADMIN_PORT:-18937}}"
if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/release/bifrost}"
else
  BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
fi

case "${BIFROST_BIN//\\//}" in
  target/release/bifrost|*/target/release/bifrost|target/release/bifrost.exe|*/target/release/bifrost.exe)
    echo "[weixin-provider] SKIP fake iLink: release build rejects Weixin loopback by design"
    exit 0
    ;;
esac

TEST_DIR="$(mktemp -d)"
BIFROST_LOG="$TEST_DIR/bifrost.log"
WEIXIN_REQUEST_LOG="$TEST_DIR/weixin-requests.ndjson"
WEIXIN_PORT_FILE="$TEST_DIR/weixin-port"

cleanup() {
  if [[ -n "${BIFROST_PID:-}" ]]; then
    kill "$BIFROST_PID" >/dev/null 2>&1 || true
    wait "$BIFROST_PID" >/dev/null 2>&1 || true
  fi
  if [[ -n "${WEIXIN_PID:-}" ]]; then
    kill "$WEIXIN_PID" >/dev/null 2>&1 || true
    wait "$WEIXIN_PID" >/dev/null 2>&1 || true
  fi
  # The native runner and threaded mock server may finish their final file
  # writes a few milliseconds after their parent processes have been reaped.
  # Retry removal so that this harmless shutdown race cannot turn a passing
  # provider flow into a failed E2E job.
  local cleanup_attempt
  for cleanup_attempt in $(seq 1 20); do
    if rm -rf "$TEST_DIR" 2>/dev/null; then
      return 0
    fi
    sleep 0.1
  done
  rm -rf "$TEST_DIR"
}

python3 - "$WEIXIN_PORT_FILE" "$WEIXIN_REQUEST_LOG" <<'PY' &
import base64
import json
import pathlib
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port_file = pathlib.Path(sys.argv[1])
request_log = pathlib.Path(sys.argv[2])
lock = threading.Lock()
state = {"uploads": 0}
inline_png = (
    b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01"
    b"\x08\x06\x00\x00\x00\x1f\x15\xc4\x89\x00\x00\x00\rIDAT\x08\xd7c\xf8\xcf\xc0\xf0\x1f\x00\x05\x00\x01\xff\x89\x99=\x1d\x00\x00\x00\x00IEND\xaeB`\x82"
)

class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_args):
        return

    def read_body(self):
        length = int(self.headers.get("content-length", "0"))
        return self.rfile.read(length) if length else b""

    def record(self, record):
        with lock:
            with request_log.open("a", encoding="utf-8") as handle:
                handle.write(json.dumps(record, ensure_ascii=False) + "\n")

    def send_json(self, payload, extra_headers=None):
        body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        for key, value in (extra_headers or {}).items():
            self.send_header(key, value)
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        raw = self.read_body()
        if self.path.startswith("/cdn-upload/"):
            upload_id = self.path.rsplit("/", 1)[-1]
            self.record({
                "path": self.path,
                "kind": "cdn_upload",
                "upload_id": upload_id,
                "body_base64": base64.b64encode(raw).decode("ascii"),
            })
            self.send_json({}, {"x-encrypted-param": f"download-{upload_id}"})
            return

        body = json.loads(raw.decode("utf-8") or "{}")
        if self.path.endswith("/ilink/bot/getupdates"):
            cursor = body.get("get_updates_buf", "")
            if cursor == "":
                payload = {
                    "ret": 0,
                    "longpolling_timeout_ms": 5000,
                    "get_updates_buf": "cursor-weixin-e2e-1",
                    "msgs": [{
                        "message_id": "weixin-e2e-inbound-1",
                        "from_user_id": "owner@im.wechat",
                        "to_user_id": "mock-bot@im.bot",
                        "message_type": 1,
                        "context_token": "weixin-e2e-context",
                        "item_list": [{"type": 1, "text_item": {"text": "RUN_WEIXIN_NATIVE_E2E"}}],
                    }],
                }
            elif cursor == "cursor-weixin-e2e-1":
                time.sleep(2.0)
                payload = {
                    "ret": 0,
                    "longpolling_timeout_ms": 5000,
                    "get_updates_buf": "cursor-weixin-e2e-2",
                    "msgs": [{
                        "message_id": "weixin-e2e-inbound-image-1",
                        "from_user_id": "owner@im.wechat",
                        "to_user_id": "mock-bot@im.bot",
                        "message_type": 2,
                        "context_token": "weixin-e2e-context",
                        "item_list": [{
                            "type": 2,
                            "msg_id": "weixin-e2e-image-1",
                            "image_item": {
                                "mime_type": "image/png",
                                "data_base64": base64.b64encode(inline_png).decode("ascii"),
                            },
                        }],
                    }],
                }
            else:
                time.sleep(1.0)
                payload = {
                    "ret": 0,
                    "longpolling_timeout_ms": 5000,
                    "get_updates_buf": "cursor-weixin-e2e-2",
                    "msgs": [],
                }
            try:
                self.send_json(payload)
            except (BrokenPipeError, ConnectionResetError):
                return
            # A failed long-poll response must not advance the mock cursor or
            # enter the request log. The real iLink cursor only advances after
            # the client receives the response and presents the new cursor.
            self.record({"path": self.path, "kind": "json", "body": body})
            return
        self.record({"path": self.path, "kind": "json", "body": body})
        if self.path.endswith("/ilink/bot/getconfig"):
            self.send_json({"ret": 0, "typing_ticket": "typing-weixin-e2e"})
            return
        if self.path.endswith("/ilink/bot/getuploadurl"):
            with lock:
                state["uploads"] += 1
                upload_id = str(state["uploads"])
            self.send_json({
                "ret": 0,
                "upload_param": f"upload-{upload_id}",
                "upload_full_url": f"http://127.0.0.1:{self.server.server_address[1]}/cdn-upload/{upload_id}",
            })
            return
        if self.path.endswith("/ilink/bot/sendtyping") or self.path.endswith("/ilink/bot/sendmessage"):
            self.send_json({"ret": 0})
            return
        self.send_response(404)
        self.send_header("content-length", "0")
        self.end_headers()

server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
port_file.write_text(str(server.server_address[1]), encoding="utf-8")
server.serve_forever()
PY
WEIXIN_PID=$!

for _ in $(seq 1 80); do
  [[ -s "$WEIXIN_PORT_FILE" ]] && break
  kill -0 "$WEIXIN_PID" 2>/dev/null || {
    echo "[weixin-provider] mock iLink server exited before reporting its port" >&2
    exit 1
  }
  sleep 0.1
done
[[ -s "$WEIXIN_PORT_FILE" ]]
WEIXIN_PORT="$(<"$WEIXIN_PORT_FILE")"
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
  [[ -f "$BIFROST_LOG" ]] && tail -100 "$BIFROST_LOG" >&2 || true
  return 1
}

if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
  echo "[weixin-provider] skipping build, using $BIFROST_BIN"
else
  echo "[weixin-provider] building bifrost"
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

if [[ ! -x "$BIFROST_BIN" ]]; then
  echo "[weixin-provider] bifrost binary is not executable: $BIFROST_BIN" >&2
  exit 1
fi

echo "[weixin-provider] starting bifrost on $BIFROST_PORT with data dir $TEST_DIR"
export HTTP_PROXY=http://127.0.0.1:9
export HTTPS_PROXY=http://127.0.0.1:9
export ALL_PROXY=http://127.0.0.1:9
export NO_PROXY=127.0.0.1,localhost
export http_proxy="$HTTP_PROXY"
export https_proxy="$HTTPS_PROXY"
export all_proxy="$ALL_PROXY"
export no_proxy="$NO_PROXY"
BIFROST_E2E_ALLOW_WEIXIN_LOOPBACK_BASE_URL=1 \
BIFROST_DATA_DIR="$TEST_DIR" "$BIFROST_BIN" start \
  --host 127.0.0.1 \
  -p "$BIFROST_PORT" \
  --unsafe-ssl \
  --skip-cert-check \
  --no-system-proxy \
  >"$BIFROST_LOG" 2>&1 &
BIFROST_PID=$!
wait_http "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address" "bifrost"

IM_BASE="http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway"

BASE_URL_ERR="$("$BIFROST_BIN" -p "$BIFROST_PORT" im provider add weixin-bad \
  --type weixin \
  --base-url http://127.0.0.1:9 \
  --runner traex 2>&1 || true)"
grep -q "base_url is managed by system and cannot be set via CLI" <<<"$BASE_URL_ERR"

RUNNER_ERR="$("$BIFROST_BIN" -p "$BIFROST_PORT" im provider add weixin-no-runner \
  --type weixin \
  --app-id mock-bot \
  --secret mock-token 2>&1 || true)"
grep -q -- "--runner is required when stdin is not interactive" <<<"$RUNNER_ERR"
grep -q "Default built-in runners include" <<<"$RUNNER_ERR"

"$BIFROST_BIN" -p "$BIFROST_PORT" im provider add weixin-cli \
  --type weixin \
  --app-id mock-bot@im.bot \
  --secret mock-token \
  --owner-open-id owner@im.wechat \
  --enable-long-connection false \
  --runner claude-code >/dev/null

PROVIDER_JSON="$(curl -fsS --noproxy '*' "$IM_BASE/providers/weixin-cli")"
python3 - "$PROVIDER_JSON" <<'PY'
import json
import sys

provider = json.loads(sys.argv[1])
assert provider["provider_type"] == "weixin", provider
assert provider["base_url"] == "https://ilinkai.weixin.qq.com", provider
assert provider["app_id"] == "mock-bot@im.bot", provider
assert provider["owner_open_id"] == "owner@im.wechat", provider
assert provider["secret_configured"] is True, provider
assert "secret_ref" not in provider, provider
assert provider["event_connection_enabled"] is False, provider
assert provider["agent_config"]["runner"] == "Claude-Code", provider
PY

curl -fsS --noproxy '*' -X POST "$IM_BASE/providers" \
  -H 'Content-Type: application/json' \
  -d '{
    "id": "weixin-admin",
    "provider_type": "weixin",
    "display_name": "Weixin Admin",
    "enabled": true,
    "app_id": "admin-bot@im.bot",
    "base_url": "https://evil.example",
    "app_secret": "admin-token",
    "owner_open_id": "owner@im.wechat",
    "event_connection_enabled": false,
    "agent_config": {"runner": "traex"}
  }' >/dev/null

curl -fsS --noproxy '*' -X PATCH "$IM_BASE/providers/weixin-admin" \
  -H 'Content-Type: application/json' \
  -d '{"base_url":"https://evil.example","event_connection_enabled":false}' >/dev/null

ADMIN_PROVIDER_JSON="$(curl -fsS --noproxy '*' "$IM_BASE/providers/weixin-admin")"
python3 - "$ADMIN_PROVIDER_JSON" <<'PY'
import json
import sys

provider = json.loads(sys.argv[1])
assert provider["base_url"] == "https://ilinkai.weixin.qq.com", provider
assert provider["secret_configured"] is True, provider
assert "secret_ref" not in provider, provider
assert provider["event_connection_enabled"] is False, provider
PY

SEND_READY_JSON="$(curl -fsS --noproxy '*' "$IM_BASE/providers/weixin-admin/status")"
python3 - "$SEND_READY_JSON" <<'PY'
import json
import sys

status = json.loads(sys.argv[1])
assert status.get("state") == "disconnected", status
assert status.get("send_ready") is False, status
assert "context token" in status.get("send_ready_reason", ""), status
assert "context_token" not in status, status
PY

for attempt in 1 2; do
  SEND_BODY="$(curl -sS --noproxy '*' -w '\n%{http_code}' -X POST "$IM_BASE/messages/send" \
    -H 'Content-Type: application/json' \
    -d '{
      "provider_id": "weixin-admin",
      "target_id": "__owner__",
      "msg_type": "text",
      "text": "idempotent readiness probe",
      "idempotency_key": "weixin-e2e-send-ready"
    }' || true)"
  SEND_CODE="$(tail -n 1 <<<"$SEND_BODY")"
  SEND_JSON="$(sed '$d' <<<"$SEND_BODY")"
  [[ "$SEND_CODE" == "409" ]]
  python3 - "$SEND_JSON" <<'PY'
import json
import sys

payload = json.loads(sys.argv[1])
message = payload.get("error") or payload.get("message") or ""
assert "not send-ready" in message, payload
PY
done

python3 - "$TEST_DIR/admin/im_gateway_outbox.json" <<'PY'
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
payload = json.loads(path.read_text())
record = payload["records"]["weixin-e2e-send-ready"]
assert record["status"] == "pending", record
assert record["attempt_count"] == 2, record
assert record["provider_id"] == "weixin-admin", record
assert record["target_id"] == "__owner__", record
assert "idempotent readiness probe" not in path.read_text(), payload
PY

curl -fsS --noproxy '*' -X POST "$IM_BASE/providers" \
  -H 'Content-Type: application/json' \
  -d '{
    "id": "weixin-unconfigured",
    "provider_type": "weixin",
    "display_name": "Weixin Unconfigured",
    "enabled": true,
    "base_url": "https://evil.example",
    "event_connection_enabled": true,
    "agent_config": {"runner": "traex"}
  }' >/dev/null

UNCONFIGURED_JSON="$(curl -fsS --noproxy '*' "$IM_BASE/providers/weixin-unconfigured")"
python3 - "$UNCONFIGURED_JSON" <<'PY'
import json
import sys

provider = json.loads(sys.argv[1])
assert provider["base_url"] == "https://ilinkai.weixin.qq.com", provider
assert provider["secret_configured"] is False, provider
assert "secret_ref" not in provider, provider
PY

CONNECT_BODY="$(curl -sS --noproxy '*' -w '\n%{http_code}' -X POST "$IM_BASE/providers/weixin-unconfigured/connect" || true)"
CONNECT_CODE="$(tail -n 1 <<<"$CONNECT_BODY")"
CONNECT_JSON="$(sed '$d' <<<"$CONNECT_BODY")"
if [[ "$CONNECT_CODE" == "200" ]]; then
  echo "[weixin-provider] connect unexpectedly succeeded without completed QR login" >&2
  exit 1
fi
python3 - "$CONNECT_JSON" <<'PY'
import json
import sys

payload = json.loads(sys.argv[1])
message = payload.get("error") or payload.get("message") or ""
assert "QR login" in message or "bot token" in message or "secret configured" in message, payload
PY

STATUS_JSON="$(curl -fsS --noproxy '*' "$IM_BASE/providers/weixin-unconfigured/status")"
python3 - "$STATUS_JSON" <<'PY'
import json
import sys

status = json.loads(sys.argv[1])
assert status.get("state") in {"failed", "disconnected"}, status
if status.get("state") == "failed":
    last_error = status.get("last_error") or ""
    assert "QR login" in last_error or "bot token" in last_error or "secret configured" in last_error, status
PY

python3 - "$BIFROST_PORT" "$REPO_DIR" "$WEIXIN_PORT" "$TEST_DIR" <<'PY'
import json
import pathlib
import sys
import urllib.request

port, repo_dir, weixin_port, test_dir = sys.argv[1:5]
base = f"http://127.0.0.1:{port}/_bifrost/api/im-gateway"

def request(path, payload=None, method="POST"):
    data = None if payload is None else json.dumps(payload, ensure_ascii=False).encode("utf-8")
    req = urllib.request.Request(
        base + path,
        data=data,
        headers={"content-type": "application/json"},
        method=method,
    )
    with urllib.request.urlopen(req, timeout=30) as response:
        body = response.read().decode("utf-8")
        assert response.status == 200, body
        return json.loads(body) if body else None

test_root = pathlib.Path(test_dir)
report_path = test_root / "weixin-e2e-report.md"
report_path.write_text("# Weixin E2E Report\n\nnative file delivery\n", encoding="utf-8")
image_path = test_root / "weixin-e2e-chart.png"
image_path.write_bytes(
    b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01"
    b"\x08\x06\x00\x00\x00\x1f\x15\xc4\x89\x00\x00\x00\rIDAT\x08\xd7c\xf8\xcf\xc0\xf0\x1f\x00\x05\x00\x01\xff\x89\x99=\x1d\x00\x00\x00\x00IEND\xaeB`\x82"
)
video_path = test_root / "weixin-e2e-video.mp4"
video_path.write_bytes(b"\x00\x00\x00\x18ftypisomweixin-e2e-video")
input_capture_path = test_root / "weixin-e2e-agent-input.txt"
runner_code = r'''
import json
import pathlib
import sys
import time
_prompt = sys.stdin.read()
with pathlib.Path(sys.argv[4]).open("a", encoding="utf-8") as capture:
    capture.write("===RUN===\n")
    capture.write(_prompt)
    capture.write("\n===END===\n")
print(json.dumps({"type": "run_started", "content": "started", "session_id": "weixin-native-e2e-session"}))
print(json.dumps({"type": "tool_started", "tool_name": "exec_command", "arguments": "verify weixin attachments"}))
time.sleep(5.2)
print(json.dumps({"type": "tool_finished", "tool_name": "exec_command", "arguments": "verify weixin attachments", "result": "ok", "success": True, "duration_ms": 5200}))
print(json.dumps({
    "type": "assistant_final",
    "content": "WEIXIN_NATIVE_E2E_FINAL\nWEIXIN_NATIVE_E2E_SECOND_LINE\n\nImage: %s\nReport: %s\nVideo: %s" % (sys.argv[1], sys.argv[2], sys.argv[3]),
}))
'''
request("/chat/config", {
    "version": 1,
    "defaultRunnerId": "weixin-native-e2e",
    "runners": {
        "weixin-native-e2e": {
            "enabled": True,
            "adapter": "custom",
            "adapterConfig": {
                "executable": sys.executable,
                "args": ["-c", runner_code, str(image_path), str(report_path), str(video_path), str(input_capture_path)],
                "timeoutSecs": 30,
            },
            "injectBifrostTools": False,
            "skillPaths": [],
            "deliveryMode": "progress_card",
        }
    },
    "channels": {},
}, "PATCH")
request("/agent", {"enabled": True, "runner": "weixin-native-e2e", "work_dir": repo_dir}, "PATCH")
request("/providers", {
    "id": "weixin-native-e2e",
    "provider_type": "weixin",
    "display_name": "Weixin Native E2E",
    "enabled": True,
    "base_url": f"http://127.0.0.1:{weixin_port}",
    "app_id": "mock-bot@im.bot",
    "app_secret": "mock-token",
    "owner_open_id": "owner@im.wechat",
    "event_connection_enabled": True,
    "agent_config": {"runner": "weixin-native-e2e", "work_dir": repo_dir},
})
provider = request("/providers/weixin-native-e2e", None, "GET")
assert provider["base_url"] == f"http://127.0.0.1:{weixin_port}", provider
request("/providers/weixin-native-e2e/connect")
PY

for _ in $(seq 1 400); do
  VIDEO_COUNT="$(python3 - "$WEIXIN_REQUEST_LOG" <<'PY'
import json
import pathlib
import sys
path = pathlib.Path(sys.argv[1])
if not path.exists():
    print(0)
else:
    records = [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]
    print(sum(
        1 for record in records
        if record["path"].endswith("/ilink/bot/sendmessage")
        and record["body"]["msg"].get("item_list", [{}])[0].get("type") == 5
    ))
PY
)"
  [[ "$VIDEO_COUNT" -ge 1 ]] && break
  sleep 0.1
done
[[ "$VIDEO_COUNT" -ge 1 ]] || {
  echo "[weixin-provider] native agent flow did not emit expected messages" >&2
  cat "$WEIXIN_REQUEST_LOG" >&2 || true
  tail -160 "$BIFROST_LOG" >&2 || true
  exit 1
}

python3 - "$WEIXIN_REQUEST_LOG" "$TEST_DIR" <<'PY'
import base64
import json
import pathlib
import subprocess
import sys

log_path = pathlib.Path(sys.argv[1])
test_root = pathlib.Path(sys.argv[2])
records = [json.loads(line) for line in log_path.read_text(encoding="utf-8").splitlines() if line.strip()]

agent_input = (test_root / "weixin-e2e-agent-input.txt").read_text(encoding="utf-8")
assert agent_input.count("===RUN===") == 1, agent_input
assert "RUN_WEIXIN_NATIVE_E2E" in agent_input, agent_input
assert "## Attached Images" in agent_input, agent_input
assert "image-1.png" in agent_input, agent_input

typing = [
    record for record in records
    if record["path"].endswith("/ilink/bot/sendtyping")
]
statuses = [record["body"]["status"] for record in typing]
assert statuses[0] == 1 and statuses[-1] == 2, statuses
assert statuses.count(1) >= 2, statuses

messages = [
    record["body"]["msg"] for record in records
    if record["path"].endswith("/ilink/bot/sendmessage")
    and record["body"]["msg"].get("run_id")
]
item_types = [message["item_list"][0]["type"] for message in messages]
for required in [11, 12, 1, 2, 4, 5]:
    assert required in item_types, (required, item_types, messages)
run_ids = {message.get("run_id") for message in messages}
assert None not in run_ids and len(run_ids) == 1, run_ids
client_ids = [message["client_id"] for message in messages]
assert len(client_ids) == len(set(client_ids)), client_ids
assert item_types.index(11) < item_types.index(12) < item_types.index(1), item_types
assert all(message["message_state"] == 2 for message in messages), messages

text_message = next(message for message in messages if message["item_list"][0]["type"] == 1)
rendered_text = text_message["item_list"][0]["text_item"]["text"]
assert "WEIXIN_NATIVE_E2E_FINAL\n\nWEIXIN_NATIVE_E2E_SECOND_LINE" in rendered_text, text_message
file_message = next(message for message in messages if message["item_list"][0]["type"] == 4)
assert file_message["item_list"][0]["file_item"]["file_name"] == "weixin-e2e-report.md", file_message
video_message = next(message for message in messages if message["item_list"][0]["type"] == 5)
assert video_message["item_list"][0]["video_item"]["video_size"] > 0, video_message

cancel_index = next(
    index for index, record in enumerate(records)
    if record["path"].endswith("/ilink/bot/sendtyping") and record["body"]["status"] == 2
)
terminal_index = next(
    index for index, record in enumerate(records)
    if record["path"].endswith("/ilink/bot/sendmessage")
    and record["body"]["msg"]["item_list"][0]["type"] == 1
    and record["body"]["msg"].get("run_id") in run_ids
)
assert cancel_index < terminal_index, (cancel_index, terminal_index)

polls = [record["body"] for record in records if record["path"].endswith("/ilink/bot/getupdates")]
assert len(polls) >= 2, polls
cursors = [poll.get("get_updates_buf", "") for poll in polls]
assert cursors[0] == "", cursors
# A connection replacement can replay the empty cursor after the mock has
# delivered its response but before the consumer durably commits cursor-1.
# That is valid at-least-once polling (the single agent run asserted above
# proves message-id deduplication). What must hold is eventual advancement and
# no regression to the empty cursor after the durable cursor is observed.
assert "cursor-weixin-e2e-1" in cursors[1:], cursors
advanced_index = cursors.index("cursor-weixin-e2e-1")
assert "" not in cursors[advanced_index:], cursors

upload_requests = [
    record["body"] for record in records
    if record["path"].endswith("/ilink/bot/getuploadurl")
]
cdn_uploads = {
    int(record["upload_id"]): base64.b64decode(record["body_base64"])
    for record in records if record.get("kind") == "cdn_upload"
}
assert {request["media_type"] for request in upload_requests} == {1, 2, 3}, upload_requests
expected_by_media_type = {
    1: (test_root / "weixin-e2e-chart.png").read_bytes(),
    2: (test_root / "weixin-e2e-video.mp4").read_bytes(),
    3: (test_root / "weixin-e2e-report.md").read_bytes(),
}
for upload_id, request in enumerate(upload_requests, 1):
    ciphertext = cdn_uploads[upload_id]
    completed = subprocess.run(
        ["openssl", "enc", "-aes-128-ecb", "-d", "-K", request["aeskey"], "-nosalt"],
        input=ciphertext,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=True,
    )
    expected = expected_by_media_type[request["media_type"]]
    assert completed.stdout == expected, (request, completed.stderr.decode("utf-8", "replace"))
    assert request["rawsize"] == len(expected), request
PY

echo "[weixin-provider] PASS"
