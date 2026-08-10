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

TEST_DIR="$(mktemp -d "$REPO_DIR/.bifrost-e2e-terminal-notification.XXXXXX")"
BIFROST_LOG="$TEST_DIR/bifrost.log"
FEISHU_REQUEST_LOG="$TEST_DIR/feishu-requests.jsonl"
FEISHU_PORT_FILE="$TEST_DIR/feishu.port"
BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"

case "${BIFROST_BIN//\\//}" in
  target/release/bifrost|*/target/release/bifrost|target/release/bifrost.exe|*/target/release/bifrost.exe)
    echo "[feishu-progress-terminal] SKIP fake OpenAPI: release build rejects Feishu loopback by design"
    exit 0
    ;;
esac

choose_loopback_port() {
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
  rm -rf "$TEST_DIR"
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
        if path.endswith("/im/v1/files"):
            length = int(self.headers.get("content-length", "0"))
            raw = self.rfile.read(length) if length else b""
            self.record({"multipart_bytes": len(raw)})
            self.send_json({"code": 0, "data": {"file_key": "file_terminal_e2e"}})
            return
        body = self.read_json()
        if path.endswith("/auth/v3/tenant_access_token/internal"):
            self.send_json({"code": 0, "tenant_access_token": "terminal-e2e-token", "expire": 7200})
            return
        if path.endswith("/cardkit/v1/cards"):
            type(self).card_counter += 1
            self.record(body)
            self.send_json({"code": 0, "data": {"card_id": f"card_{type(self).card_counter}"}})
            return
        if path.endswith("/reply") or path.endswith("/im/v1/messages"):
            type(self).message_counter += 1
            self.record(body)
            self.send_json({"code": 0, "data": {"message_id": f"om_{type(self).message_counter}"}})
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
    echo "[feishu-progress-terminal] fake Feishu exited before reporting its port" >&2
    exit 1
  }
  sleep 0.1
done
[[ -s "$FEISHU_PORT_FILE" ]]
FEISHU_PORT="$(<"$FEISHU_PORT_FILE")"

if [[ "${SKIP_BUILD:-false}" != "true" ]]; then
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

BIFROST_PORT="${BIFROST_PORT:-$(choose_loopback_port)}"
BIFROST_DATA_DIR="$TEST_DIR" "$BIFROST_BIN" start \
  --host 127.0.0.1 \
  -p "$BIFROST_PORT" \
  --unsafe-ssl \
  --skip-cert-check \
  --no-system-proxy \
  >"$BIFROST_LOG" 2>&1 &
BIFROST_PID=$!

for _ in $(seq 1 180); do
  if curl -fsS --noproxy '*' "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address" >/dev/null 2>&1; then
    break
  fi
  kill -0 "$BIFROST_PID" 2>/dev/null || {
    tail -160 "$BIFROST_LOG" >&2 || true
    exit 1
  }
  sleep 0.25
done
curl -fsS --noproxy '*' "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address" >/dev/null

python3 - "$BIFROST_PORT" "$REPO_DIR" "$FEISHU_PORT" "$TEST_DIR" <<'PY'
import json
import pathlib
import sys
import urllib.request

port, repo_dir, feishu_port, test_dir = sys.argv[1:5]
base = f"http://127.0.0.1:{port}/_bifrost/api/im-gateway"

def request(path, payload, method="POST"):
    req = urllib.request.Request(
        base + path,
        data=json.dumps(payload, ensure_ascii=False).encode("utf-8"),
        headers={"content-type": "application/json"},
        method=method,
    )
    with urllib.request.urlopen(req, timeout=30) as response:
        body = response.read().decode("utf-8")
        assert response.status == 200, body

report_path = pathlib.Path(test_dir) / "terminal-e2e-report.txt"
report_path.write_text("terminal attachment contents", encoding="utf-8")
runner_code = r'''
import json
import sys
prompt = sys.stdin.read()
if "FAIL_TERMINAL_E2E" in prompt:
    print(json.dumps({"type": "run_failed", "content": "E2E_PERMISSION_DENIED"}))
    print("E2E_PERMISSION_DENIED", file=sys.stderr)
    raise SystemExit(17)
print(json.dumps({"type": "assistant_final", "content": "E2E_FINAL_SUMMARY_SUCCESS\n\n[E2E report](%s)" % sys.argv[1]}))
'''
request("/chat/config", {
    "version": 1,
    "defaultRunnerId": "terminal-e2e",
    "runners": {
        "terminal-e2e": {
            "enabled": True,
            "adapter": "custom",
            "adapterConfig": {
                "executable": sys.executable,
                "args": ["-c", runner_code, str(report_path)],
                "timeoutSecs": 30,
            },
            "injectBifrostTools": False,
            "skillPaths": [],
            "deliveryMode": "progress_card",
        }
    },
    "channels": {},
}, "PATCH")
request("/agent", {"enabled": True, "runner": "terminal-e2e", "work_dir": repo_dir}, "PATCH")
request("/providers", {
    "id": "feishu-terminal-e2e",
    "provider_type": "feishu",
    "display_name": "Feishu Terminal E2E",
    "enabled": True,
    "base_url": f"http://127.0.0.1:{feishu_port}/open-apis",
    "app_id": "cli_terminal_e2e",
    "app_secret": "terminal-e2e-secret",
    "owner_open_id": "ou_terminal_owner",
    "event_connection_enabled": False,
    "agent_config": {"runner": "terminal-e2e"},
})
PY

inject() {
  local message_id="$1"
  local text="$2"
  python3 - "$BIFROST_PORT" "$message_id" "$text" <<'PY'
import json
import sys
import urllib.request

port, message_id, text = sys.argv[1:4]
payload = {
    "providerId": "feishu-terminal-e2e",
    "chatId": "oc_terminal_e2e",
    "chatType": "group",
    "userId": "ou_terminal_owner",
    "userName": "Terminal E2E",
    "messageId": message_id,
    "eventId": "event-" + message_id,
    "text": text,
    "mentionBot": True,
}
req = urllib.request.Request(
    f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/debug/mock-inbound",
    data=json.dumps(payload).encode("utf-8"),
    headers={"content-type": "application/json"},
    method="POST",
)
with urllib.request.urlopen(req, timeout=30) as response:
    assert response.status == 200, response.read().decode("utf-8")
PY
}

wait_session_idle() {
  for _ in $(seq 1 240); do
    if curl -fsS --noproxy '*' \
      "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/agent/sessions/all?limit=80" \
      | python3 -c '
import json, sys
sessions = json.load(sys.stdin).get("sessions", [])
raise SystemExit(1 if any(item.get("running") is True for item in sessions) else 0)
'; then
      return 0
    fi
    sleep 0.1
  done
  tail -160 "$BIFROST_LOG" >&2 || true
  return 1
}

wait_message_count() {
  local expected="$1"
  for _ in $(seq 1 240); do
    local actual=0
    if [[ -f "$FEISHU_REQUEST_LOG" ]]; then
      actual="$(python3 - "$FEISHU_REQUEST_LOG" <<'PY'
import json
import sys
records = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8") if line.strip()]
print(sum(1 for record in records if "/im/v1/messages" in record["path"]))
PY
)"
    fi
    [[ "$actual" == "$expected" ]] && return 0
    sleep 0.1
  done
  cat "$FEISHU_REQUEST_LOG" >&2 || true
  return 1
}

inject terminal-success "run terminal success e2e"
wait_session_idle
wait_message_count 3
inject terminal-failure "FAIL_TERMINAL_E2E"
wait_session_idle
wait_message_count 5

python3 - "$FEISHU_REQUEST_LOG" <<'PY'
import json
import sys

records = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8") if line.strip()]
messages = [record for record in records if "/im/v1/messages" in record["path"]]
assert len(messages) == 5, messages

success_progress, success_terminal, success_file, failure_progress, failure_terminal = messages
assert success_progress["path"].endswith("/im/v1/messages/terminal-success/reply"), success_progress
assert success_terminal["path"].endswith("/im/v1/messages/om_1/reply"), success_terminal
assert success_file["path"].split("?", 1)[0].endswith("/im/v1/messages"), success_file
assert success_file["body"]["msg_type"] == "file", success_file
assert "file_terminal_e2e" in success_file["body"]["content"], success_file
assert failure_progress["path"].endswith("/im/v1/messages/terminal-failure/reply"), failure_progress
assert failure_terminal["path"].endswith("/im/v1/messages/om_4/reply"), failure_terminal

success_card = json.loads(success_terminal["body"]["content"])
failure_card = json.loads(failure_terminal["body"]["content"])
supported_locales = {
    "zh_cn", "en_us", "ja_jp", "zh_hk", "zh_tw", "id_id", "vi_vn", "th_th",
    "pt_br", "es_es", "ko_kr", "de_de", "fr_fr", "it_it", "ru_ru", "ms_my",
}
assert success_card["header"]["template"] == "green", success_card
assert success_card["header"]["title"]["content"] == "Task completed", success_card
assert set(success_card["header"]["title"]["i18n_content"]) == supported_locales, success_card
assert success_card["header"]["title"]["i18n_content"]["zh_cn"] == "任务执行结束", success_card
assert "E2E_FINAL_SUMMARY_SUCCESS" in json.dumps(success_card["body"], ensure_ascii=False), success_card

assert failure_card["header"]["template"] == "red", failure_card
assert failure_card["header"]["title"]["content"] == "Task failed", failure_card
assert set(failure_card["header"]["title"]["i18n_content"]) == supported_locales, failure_card
assert failure_card["header"]["title"]["i18n_content"]["zh_cn"] == "任务执行失败", failure_card
assert "E2E_PERMISSION_DENIED" in json.dumps(failure_card["body"], ensure_ascii=False), failure_card

updates = [
    record for record in records
    if record["method"] == "PUT"
    and "/cardkit/v1/cards/" in record["path"]
    and isinstance(record["body"].get("card"), dict)
    and "data" in record["body"]["card"]
]
rendered_updates = "\n".join(record["body"]["card"]["data"] for record in updates)
assert "E2E_FINAL_SUMMARY_SUCCESS" in rendered_updates, rendered_updates
assert "E2E_PERMISSION_DENIED" in rendered_updates, rendered_updates

def terminal_progress_card(marker):
    candidates = [
        json.loads(record["body"]["card"]["data"])
        for record in updates
        if marker in record["body"]["card"]["data"]
    ]
    assert candidates, (marker, rendered_updates)
    return candidates[-1]

for marker, title in [
    ("E2E_FINAL_SUMMARY_SUCCESS", "最终结论"),
    ("E2E_PERMISSION_DENIED", "失败结论"),
]:
    progress_card = terminal_progress_card(marker)
    elements = progress_card["body"]["elements"]
    status = next(element for element in elements if element.get("element_id") == "agent_status_panel")
    output = next(element for element in elements if element.get("element_id") == "agent_output")
    assert status["tag"] == "collapsible_panel" and status["expanded"] is False, status
    assert output["tag"] == "collapsible_panel" and output["expanded"] is False, output
    assert output["header"]["title"]["content"] == title, output
    assert marker in json.dumps(output, ensure_ascii=False), output

uploads = [record for record in records if record["path"].split("?", 1)[0].endswith("/im/v1/files")]
assert len(uploads) == 1 and uploads[0]["body"]["multipart_bytes"] > 0, uploads
PY

echo "[feishu-progress-terminal] PASS"
