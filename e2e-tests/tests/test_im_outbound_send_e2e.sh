#!/usr/bin/env bash
set -euo pipefail

unset BIFROST_DETACHED_DAEMON_CHILD
unset BIFROST_EXTERNAL_CLI_WORKER
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
export BIFROST_DISABLE_TRAY=1
export BIFROST_E2E_ALLOW_FEISHU_LOOPBACK_BASE_URL=1
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

TEST_DIR="$(mktemp -d "$REPO_DIR/.bifrost-e2e-im-send.XXXXXX")"
BIFROST_LOG="$TEST_DIR/bifrost.log"
FEISHU_LOG="$TEST_DIR/feishu.jsonl"
FEISHU_PORT_FILE="$TEST_DIR/feishu.port"
BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"

case "${BIFROST_BIN//\\//}" in
  target/release/bifrost|*/target/release/bifrost|target/release/bifrost.exe|*/target/release/bifrost.exe)
    echo "[im-outbound-send] SKIP fake OpenAPI: release build rejects Feishu loopback by design"
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
  # A late service destructor can briefly recreate `.system_proxy.lock` after
  # the foreground process exits. Retry the exact test directory so the E2E
  # leaves no repository-local state behind.
  # The system-proxy lifecycle helper polls its parent every two seconds. Give
  # it more than two poll intervals to observe shutdown before the final
  # removal, otherwise it can recreate `.system_proxy.lock` after this trap.
  for _ in $(seq 1 24); do
    rm -rf "$TEST_DIR"
    sleep 0.25
  done
  rm -rf "$TEST_DIR"
}
trap cleanup EXIT

python3 - "$FEISHU_PORT_FILE" "$FEISHU_LOG" <<'PY' &
import json
import pathlib
import re
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port_file = pathlib.Path(sys.argv[1])
request_log = pathlib.Path(sys.argv[2])
lock = threading.Lock()

class Handler(BaseHTTPRequestHandler):
    counter = 0

    def log_message(self, *_args):
        pass

    def send_json(self, payload):
        body = json.dumps(payload, ensure_ascii=False).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def record(self, kind, body, byte_count):
        with lock:
            with request_log.open("a", encoding="utf-8") as handle:
                handle.write(json.dumps({
                    "kind": kind,
                    "path": self.path,
                    "body": body,
                    "byte_count": byte_count,
                }, ensure_ascii=False) + "\n")

    def do_POST(self):
        path = self.path.split("?", 1)[0]
        length = int(self.headers.get("content-length", "0"))
        raw = self.rfile.read(length) if length else b""
        if path.endswith("/auth/v3/tenant_access_token/internal"):
            self.send_json({"code": 0, "tenant_access_token": "e2e-token", "expire": 7200})
            return
        if path.endswith("/im/v1/images"):
            names = re.findall(r'filename="([^"]+)"', raw.decode(errors="replace"))
            self.record("image_upload", {"filenames": names}, len(raw))
            self.send_json({"code": 0, "data": {"image_key": "img_e2e_chart"}})
            return
        if path.endswith("/im/v1/files"):
            names = re.findall(r'filename="([^"]+)"', raw.decode(errors="replace"))
            self.record("file_upload", {"filenames": names}, len(raw))
            self.send_json({"code": 0, "data": {"file_key": "file_e2e_report"}})
            return
        if path.endswith("/im/v1/messages"):
            body = json.loads(raw.decode())
            type(self).counter += 1
            self.record("message", body, len(raw))
            self.send_json({"code": 0, "data": {"message_id": f"om_e2e_{type(self).counter}"}})
            return
        self.send_json({"code": 404, "msg": "not found"})

server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
port_file.write_text(str(server.server_address[1]), encoding="utf-8")
server.serve_forever()
PY
FEISHU_PID=$!

for _ in $(seq 1 80); do
  [[ -s "$FEISHU_PORT_FILE" ]] && break
  kill -0 "$FEISHU_PID" 2>/dev/null || exit 1
  sleep 0.1
done
[[ -s "$FEISHU_PORT_FILE" ]]
FEISHU_PORT="$(<"$FEISHU_PORT_FILE")"

if [[ ! -x "$BIFROST_BIN" ]]; then
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

SKILL_DIR="$TEST_DIR/installed-skill"
BIFROST_INSTALL_SKILL_SOURCE=embedded "$BIFROST_BIN" install-skill \
  --tool codex --dir "$SKILL_DIR" -y >/dev/null
grep -q "给指定群发 Markdown/图片/文件/卡片" "$SKILL_DIR/SKILL.md"
grep -q "bifrost im provider capabilities feishu-main" "$SKILL_DIR/SKILL.md"
grep -q "bifrost im send feishu-main --chat-id oc_xxx" "$SKILL_DIR/SKILL.md"
grep -q 'bifrost im send --bot-id cli_xxx --chat-id oc_xxx' "$SKILL_DIR/SKILL.md"
grep -q 'bifrost im send --bot-name "Release Bot" --chat-id oc_xxx' "$SKILL_DIR/SKILL.md"
grep -q "只有用户已明确要求发送" "$SKILL_DIR/SKILL.md"
grep -q "partial_success" "$SKILL_DIR/SKILL.md"

BIFROST_PORT="${BIFROST_PORT:-$(choose_port)}"
BIFROST_DATA_DIR="$TEST_DIR/data" "$BIFROST_BIN" start \
  --host 127.0.0.1 -p "$BIFROST_PORT" --unsafe-ssl --skip-cert-check --no-system-proxy \
  >"$BIFROST_LOG" 2>&1 &
BIFROST_PID=$!

for _ in $(seq 1 160); do
  if curl -fsS --noproxy '*' "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/providers" >/dev/null 2>&1; then
    break
  fi
  kill -0 "$BIFROST_PID" 2>/dev/null || {
    tail -120 "$BIFROST_LOG" >&2 || true
    exit 1
  }
  sleep 0.25
done

python3 - "$BIFROST_PORT" "$FEISHU_PORT" "$TEST_DIR" <<'PY'
import json
import pathlib
import sys
import urllib.request

port, feishu_port, test_dir = sys.argv[1:4]
base = f"http://127.0.0.1:{port}/_bifrost/api/im-gateway"

def post(path, payload):
    req = urllib.request.Request(
        base + path,
        data=json.dumps(payload).encode(),
        headers={"content-type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=20) as response:
        assert response.status == 200, response.read().decode()

post("/providers", {
    "id": "feishu-main",
    "provider_type": "feishu",
    "display_name": "Feishu Main",
    "enabled": True,
    "base_url": f"http://127.0.0.1:{feishu_port}/open-apis",
    "app_id": "cli_e2e",
    "app_secret": "e2e-secret",
    "owner_open_id": "ou_e2e_owner",
    "event_connection_enabled": False,
})
post("/providers", {
    "id": "weixin-main",
    "provider_type": "weixin",
    "display_name": "Weixin Main",
    "enabled": True,
    "event_connection_enabled": False,
})

root = pathlib.Path(test_dir)
(root / "report.md").write_text("# Release\n\n**ready**", encoding="utf-8")
(root / "card.json").write_text(json.dumps({
    "header": {"title": {"tag": "plain_text", "content": "Target card"}},
    "elements": [{"tag": "markdown", "content": "**done**"}],
}), encoding="utf-8")
(root / "chart.png").write_bytes(b"PNG-E2E-CONTENT")
(root / "report.pdf").write_bytes(b"PDF-E2E-CONTENT")
PY

FEISHU_CAPS="$($BIFROST_BIN -p "$BIFROST_PORT" im provider capabilities feishu-main --format json)"
WEIXIN_CAPS="$($BIFROST_BIN -p "$BIFROST_PORT" im provider capabilities weixin-main --format json)"
python3 - "$FEISHU_CAPS" "$WEIXIN_CAPS" <<'PY'
import json
import sys
feishu, weixin = map(json.loads, sys.argv[1:3])
assert feishu["parts"]["file"]["support"] == "native", feishu
assert feishu["parts"]["native_card"]["support"] == "native", feishu
assert weixin["parts"]["markdown"]["support"] == "degraded", weixin
assert weixin["parts"]["file"]["support"] == "unsupported", weixin
assert weixin["requires_context"] is True, weixin
PY

UNSAFE_UPLOAD_STATUS="$(curl -sS --noproxy '*' -o "$TEST_DIR/unsafe-upload.json" -w '%{http_code}' \
  -X POST --data-binary 'x' \
  "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/messages/upload?provider_id=feishu-main&kind=file&file_name=..%5Csecret.txt")"
[[ "$UNSAFE_UPLOAD_STATUS" == "400" ]]
grep -q 'plain file name' "$TEST_DIR/unsafe-upload.json"

$BIFROST_BIN -p "$BIFROST_PORT" im target add oncall \
  --provider feishu-main --receive-id-type chat_id --receive-id oc_oncall

OWNER_RESULT="$($BIFROST_BIN -p "$BIFROST_PORT" im send feishu-main \
  --text 'owner hello' --idempotency-key owner-e2e --format json)"
TARGET_RESULT="$($BIFROST_BIN -p "$BIFROST_PORT" im send --bot-name 'Feishu Main' \
  --target oncall --card-file "$TEST_DIR/card.json" --idempotency-key target-e2e --format json)"
DIRECT_RESULT="$($BIFROST_BIN -p "$BIFROST_PORT" im send --bot-id cli_e2e \
  --chat-id oc_direct --markdown-file "$TEST_DIR/report.md" \
  --image "$TEST_DIR/chart.png" --file "$TEST_DIR/report.pdf" \
  --idempotency-key direct-e2e --format json)"

python3 - "$OWNER_RESULT" "$TARGET_RESULT" "$DIRECT_RESULT" "$FEISHU_LOG" <<'PY'
import json
import pathlib
import sys

owner, target, direct = map(json.loads, sys.argv[1:4])
assert owner["status"] == "success" and owner["destination"] == "owner", owner
assert target["status"] == "success" and target["destination"] == "target:oncall", target
assert direct["status"] == "success" and len(direct["receipts"]) == 3, direct
assert [item["requested_kind"] for item in direct["receipts"]] == ["markdown", "image", "file"], direct

events = [json.loads(line) for line in pathlib.Path(sys.argv[4]).read_text().splitlines()]
image = next(item for item in events if item["kind"] == "image_upload")
file = next(item for item in events if item["kind"] == "file_upload")
assert image["body"]["filenames"] == ["chart.png"] and image["byte_count"] > 15, image
assert file["body"]["filenames"] == ["report.pdf"] and file["byte_count"] > 15, file

messages = [item for item in events if item["kind"] == "message"]
assert len(messages) == 5, messages
assert messages[0]["body"]["receive_id"] == "ou_e2e_owner", messages[0]
assert "receive_id_type=open_id" in messages[0]["path"], messages[0]
assert messages[1]["body"]["receive_id"] == "oc_oncall", messages[1]
assert "receive_id_type=chat_id" in messages[1]["path"], messages[1]
target_card = json.loads(messages[1]["body"]["content"])
assert target_card["header"]["title"]["content"] == "Target card", target_card
direct_messages = messages[2:]
assert [item["body"]["msg_type"] for item in direct_messages] == ["interactive", "image", "file"], direct_messages
assert all(item["body"]["receive_id"] == "oc_direct" for item in direct_messages), direct_messages
assert all(item["body"].get("uuid") for item in messages), messages
assert len({item["body"]["uuid"] for item in messages}) == 5, messages
PY

UNKNOWN_ERR="$($BIFROST_BIN -p "$BIFROST_PORT" im send feishu-main --text hello --typo 2>&1 || true)"
grep -q "unknown im send option '--typo'" <<<"$UNKNOWN_ERR"
HELP_OUTPUT="$($BIFROST_BIN im send --help)"
grep -q "bifrost im send \[PROVIDER\]" <<<"$HELP_OUTPUT"

echo "[im-outbound-send] passed"
