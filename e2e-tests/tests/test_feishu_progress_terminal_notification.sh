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
import re
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port_file = pathlib.Path(sys.argv[1])
request_log = pathlib.Path(sys.argv[2])
lock = threading.Lock()

class Handler(BaseHTTPRequestHandler):
    card_counter = 0
    file_counter = 0
    image_counter = 0
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
        if path.endswith("/im/v1/images"):
            length = int(self.headers.get("content-length", "0"))
            raw = self.rfile.read(length) if length else b""
            filenames = re.findall(r'filename="([^"]+)"', raw.decode("utf-8", errors="replace"))
            self.record({"multipart_bytes": len(raw), "filenames": filenames})
            type(self).image_counter += 1
            self.send_json({"code": 0, "data": {"image_key": "img_v3_terminal_e2e"}})
            return
        if path.endswith("/im/v1/files"):
            length = int(self.headers.get("content-length", "0"))
            raw = self.rfile.read(length) if length else b""
            filenames = re.findall(r'filename="([^"]+)"', raw.decode("utf-8", errors="replace"))
            self.record({"multipart_bytes": len(raw), "filenames": filenames})
            if "terminal-e2e-upload-failure.txt" in filenames:
                self.send_json({"code": 234006, "msg": "The file size exceed the max value."})
                return
            type(self).file_counter += 1
            self.send_json({"code": 0, "data": {"file_key": f"file_terminal_e2e_{type(self).file_counter}"}})
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
  env -u HTTP_PROXY -u HTTPS_PROXY -u ALL_PROXY \
    -u http_proxy -u https_proxy -u all_proxy \
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
codex_line_report_path = pathlib.Path(test_dir) / "方案.md"
codex_line_report_path.write_text("codex source-position attachment", encoding="utf-8")
archive_path = pathlib.Path(test_dir) / "terminal-e2e-bundle.tar.gz"
archive_path.write_bytes(b"terminal archive contents")
config_path = pathlib.Path(test_dir) / "next-harness.yaml"
config_path.write_text("runner: terminal-e2e\n", encoding="utf-8")
source_path = pathlib.Path(test_dir) / "terminal-e2e-handler.rs"
source_path.write_text("fn main() {}\n", encoding="utf-8")
image_path = pathlib.Path(test_dir) / "terminal-e2e-chart.png"
image_path.write_bytes(
    b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01"
    b"\x08\x06\x00\x00\x00\x1f\x15\xc4\x89\x00\x00\x00\rIDAT\x08\xd7c\xf8\xcf\xc0\xf0\x1f\x00\x05\x00\x01\xff\x89\x99\x3d\x1d\x00\x00\x00\x00IEND\xaeB`\x82"
)
flow_svg_path = pathlib.Path(test_dir) / "terminal-e2e-flow.svg"
flow_svg_path.write_text('<svg xmlns="http://www.w3.org/2000/svg"><rect width="1" height="1"/></svg>', encoding="utf-8")
flow_png_path = pathlib.Path(test_dir) / "terminal-e2e-flow.png"
flow_png_path.write_bytes(image_path.read_bytes())
oversized_path = pathlib.Path(test_dir) / "terminal-e2e-oversized.bin"
with oversized_path.open("wb") as handle:
    handle.truncate(30 * 1024 * 1024 + 1)
upload_failure_path = pathlib.Path(test_dir) / "terminal-e2e-upload-failure.txt"
upload_failure_path.write_text("upload should fail without failing the task", encoding="utf-8")
runner_code = r'''
import json
import sys
import time
prompt = sys.stdin.read()
if "HEARTBEAT_E2E" in prompt:
    print(json.dumps({"type": "run_started", "content": "started", "session_id": "heartbeat-session-e2e"}), flush=True)
    time.sleep(12)
    print(json.dumps({"type": "assistant_final", "content": "E2E_HEARTBEAT_FINAL"}), flush=True)
    raise SystemExit(0)
if "FAIL_TERMINAL_E2E" in prompt:
    print(json.dumps({"type": "run_failed", "content": "E2E_PERMISSION_DENIED"}))
    print("E2E_PERMISSION_DENIED", file=sys.stderr)
    raise SystemExit(17)
if "ATTACHMENT_FAILURE_E2E" in prompt:
    print(json.dumps({"type": "run_started", "content": "started", "session_id": "terminal-session-attachment-e2e"}))
    print(json.dumps({"type": "assistant_final", "content": "E2E_FINAL_SUMMARY_WITH_ATTACHMENT_FAILURE\n\n[E2E oversized file](%s)\n[E2E upload failure](%s)" % (sys.argv[4], sys.argv[5])}))
    raise SystemExit(0)
print(json.dumps({"type": "run_started", "content": "started", "session_id": "terminal-session-e2e"}))
print(json.dumps({"type": "assistant_delta", "content": "**E2E_REASONING_PREFIX**"}))
print(json.dumps({"type": "assistant_delta", "content": "E2E_LATEST_EXPLANATION\n\n![E2E chart](%s)" % sys.argv[3]}))
print(json.dumps({"type": "assistant_final", "content": "E2E_LATEST_EXPLANATION\n\n![E2E chart](%s)" % sys.argv[3]}))
print(json.dumps({"type": "tool_started", "tool_name": "exec_command", "content": "verify archive"}))
print(json.dumps({"type": "tool_finished", "tool_name": "exec_command", "arguments": "verify archive", "result": "ok", "success": True, "duration_ms": 5}))
print(json.dumps({"type": "assistant_final", "content": "E2E_FINAL_SUMMARY_SUCCESS\n\n![E2E chart](%s)\n\nE2E bare report: %s\n[E2E archive](%s)\n[E2E config](%s)\n[E2E source file](%s)\n[完整方案](%s:1)\n[E2E flow](%s)" % (sys.argv[3], sys.argv[1], sys.argv[2], sys.argv[6], sys.argv[7], sys.argv[8], sys.argv[9])}))
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
                "args": ["-c", runner_code, str(report_path), str(archive_path), str(image_path), str(oversized_path), str(upload_failure_path), str(config_path), str(source_path), str(codex_line_report_path), str(flow_svg_path)],
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

wait_session_running() {
  for _ in $(seq 1 240); do
    if curl -fsS --noproxy '*' \
      "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/im-gateway/agent/sessions/all?limit=80" \
      | python3 -c '
import json, sys
sessions = json.load(sys.stdin).get("sessions", [])
raise SystemExit(0 if any(item.get("running") is True for item in sessions) else 1)
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

inject terminal-heartbeat "HEARTBEAT_E2E"
wait_session_running
wait_session_idle
wait_message_count 2
inject terminal-success "run terminal success e2e"
wait_session_idle
wait_message_count 10
inject terminal-failure "FAIL_TERMINAL_E2E"
wait_session_idle
wait_message_count 12
inject terminal-attachment-failure "ATTACHMENT_FAILURE_E2E"
wait_session_idle
wait_message_count 15

python3 - "$FEISHU_REQUEST_LOG" <<'PY'
import json
import re
import sys

records = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8") if line.strip()]
messages = [record for record in records if "/im/v1/messages" in record["path"]]
assert len(messages) == 15, messages

(
    heartbeat_progress,
    heartbeat_terminal,
    success_progress,
    success_terminal,
    success_flow_preview,
    success_report,
    success_archive,
    success_config,
    success_codex_line_report,
    success_flow_svg,
    failure_progress,
    failure_terminal,
    attachment_progress,
    attachment_terminal,
    attachment_notice,
) = messages
assert heartbeat_progress["path"].endswith("/im/v1/messages/terminal-heartbeat/reply"), heartbeat_progress
assert heartbeat_terminal["path"].endswith("/im/v1/messages/om_1/reply"), heartbeat_terminal
assert success_progress["path"].endswith("/im/v1/messages/terminal-success/reply"), success_progress
assert success_terminal["path"].endswith("/im/v1/messages/om_3/reply"), success_terminal
assert success_flow_preview["body"]["msg_type"] == "image", success_flow_preview
assert "img_v3_terminal_e2e" in success_flow_preview["body"]["content"], success_flow_preview
for index, file_message in enumerate(
    [success_report, success_archive, success_config, success_codex_line_report, success_flow_svg], 1
):
    assert file_message["path"].split("?", 1)[0].endswith("/im/v1/messages"), file_message
    assert file_message["body"]["msg_type"] == "file", file_message
    assert f"file_terminal_e2e_{index}" in file_message["body"]["content"], file_message
assert failure_progress["path"].endswith("/im/v1/messages/terminal-failure/reply"), failure_progress
assert failure_terminal["path"].endswith("/im/v1/messages/om_11/reply"), failure_terminal
assert attachment_progress["path"].endswith("/im/v1/messages/terminal-attachment-failure/reply"), attachment_progress
assert attachment_terminal["path"].endswith("/im/v1/messages/om_13/reply"), attachment_terminal
assert attachment_notice["path"].endswith("/im/v1/messages/terminal-attachment-failure/reply"), attachment_notice

heartbeat_card = json.loads(heartbeat_terminal["body"]["content"])
success_card = json.loads(success_terminal["body"]["content"])
failure_card = json.loads(failure_terminal["body"]["content"])
attachment_terminal_card = json.loads(attachment_terminal["body"]["content"])
attachment_notice_card = json.loads(attachment_notice["body"]["content"])
supported_locales = {
    "zh_cn", "en_us", "ja_jp", "zh_hk", "zh_tw", "id_id", "vi_vn", "th_th",
    "pt_br", "es_es", "ko_kr", "de_de", "fr_fr", "it_it", "ru_ru", "ms_my",
}
assert "E2E_HEARTBEAT_FINAL" in json.dumps(heartbeat_card["body"], ensure_ascii=False), heartbeat_card
assert success_card["header"]["template"] == "green", success_card
assert success_card["header"]["title"]["content"] == "Task completed", success_card
assert set(success_card["header"]["title"]["i18n_content"]) == supported_locales, success_card
assert success_card["header"]["title"]["i18n_content"]["zh_cn"] == "任务执行结束", success_card
assert "E2E_FINAL_SUMMARY_SUCCESS" in json.dumps(success_card["body"], ensure_ascii=False), success_card
assert "![E2E chart](img_v3_terminal_e2e)" in json.dumps(success_card["body"], ensure_ascii=False), success_card

assert failure_card["header"]["template"] == "red", failure_card
assert failure_card["header"]["title"]["content"] == "Task failed", failure_card
assert set(failure_card["header"]["title"]["i18n_content"]) == supported_locales, failure_card
assert failure_card["header"]["title"]["i18n_content"]["zh_cn"] == "任务执行失败", failure_card
assert "E2E_PERMISSION_DENIED" in json.dumps(failure_card["body"], ensure_ascii=False), failure_card

assert attachment_terminal_card["header"]["template"] == "green", attachment_terminal_card
assert "E2E_FINAL_SUMMARY_WITH_ATTACHMENT_FAILURE" in json.dumps(attachment_terminal_card["body"], ensure_ascii=False), attachment_terminal_card
attachment_notice_text = json.dumps(attachment_notice_card["body"], ensure_ascii=False)
assert "附件发送提示（不影响任务结论）" in attachment_notice_text, attachment_notice_card
assert "terminal-e2e-oversized.bin" in attachment_notice_text, attachment_notice_card
assert "IM 通道上传文件 30 MiB 上限" in attachment_notice_text, attachment_notice_card
assert "terminal-e2e-upload-failure.txt" in attachment_notice_text, attachment_notice_card
assert "234006" in attachment_notice_text, attachment_notice_card
assert "任务结论已正常发布" in attachment_notice_text, attachment_notice_card

updates = [
    record for record in records
    if record["method"] == "PUT"
    and "/cardkit/v1/cards/" in record["path"]
    and isinstance(record["body"].get("card"), dict)
    and "data" in record["body"]["card"]
]
rendered_updates = "\n".join(record["body"]["card"]["data"] for record in updates)
assert "E2E_HEARTBEAT_FINAL" in rendered_updates, rendered_updates
assert "E2E_FINAL_SUMMARY_SUCCESS" in rendered_updates, rendered_updates
assert "E2E_PERMISSION_DENIED" in rendered_updates, rendered_updates
assert "E2E_LATEST_EXPLANATION" in rendered_updates, rendered_updates
assert "![E2E chart](img_v3_terminal_e2e)" in rendered_updates, rendered_updates
assert "Session：获取中" in rendered_updates, rendered_updates
assert "Session：未提供" in rendered_updates, rendered_updates

heartbeat_output_updates = [
    record for record in records
    if record["method"] == "PUT"
    and record["path"].split("?", 1)[0].endswith("/cardkit/v1/cards/card_1/elements/agent_output/content")
]
assert heartbeat_output_updates, records
heartbeat_activity = heartbeat_output_updates[-1]["body"]["content"]
assert re.search(r"处理中\.\.\. · 耗时：1[0-9] 秒 · 最后更新：\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}", heartbeat_activity), heartbeat_activity
heartbeat_status_updates = [
    record for record in records
    if record["method"] == "PUT"
    and record["path"].split("?", 1)[0].endswith("/cardkit/v1/cards/card_1/elements/agent_status_panel")
]
assert heartbeat_status_updates, records
assert "最后更新：" in heartbeat_status_updates[-1]["body"]["element"], heartbeat_status_updates[-1]

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
    summary = next((element for element in elements if element.get("element_id") == "agent_process_sum"), None)
    process = next((element for element in elements if element.get("element_id") == "agent_process_panel"), None)
    output = next(element for element in elements if element.get("element_id") == "agent_output")
    assert status["tag"] == "collapsible_panel" and status["expanded"] is False, status
    assert summary is None, summary
    assert "**最新进展**" not in json.dumps(progress_card, ensure_ascii=False), progress_card
    if marker == "E2E_FINAL_SUMMARY_SUCCESS":
        assert process is not None and process["expanded"] is False, process
        assert "当前工具：暂无正在执行的工具" in process["header"]["title"]["content"], process
        assert "本轮工具：成功 1 · 失败 0 · 执行中 0" in process["header"]["title"]["content"], process
        process_text = json.dumps(process, ensure_ascii=False)
        assert process_text.count("E2E_REASONING_PREFIX") == 1, process
        assert process_text.count("E2E_LATEST_EXPLANATION") == 1, process
    assert output["tag"] == "collapsible_panel" and output["expanded"] is False, output
    assert output["header"]["title"]["content"] == title, output
    assert marker in json.dumps(output, ensure_ascii=False), output

uploads = [record for record in records if record["path"].split("?", 1)[0].endswith("/im/v1/files")]
assert len(uploads) == 6 and all(upload["body"]["multipart_bytes"] > 0 for upload in uploads), uploads
filenames = [name for upload in uploads for name in upload["body"]["filenames"]]
assert "terminal-e2e-report.txt" in filenames, filenames
assert "terminal-e2e-bundle.tar.gz" in filenames, filenames
assert "next-harness.yaml" in filenames, filenames
assert "方案.md" in filenames, filenames
assert "terminal-e2e-flow.svg" in filenames, filenames
assert "方案.md:1" not in filenames, filenames
assert "terminal-e2e-upload-failure.txt" in filenames, filenames
assert "terminal-e2e-oversized.bin" not in filenames, filenames
assert "terminal-e2e-handler.rs" not in filenames, filenames

image_uploads = [record for record in records if record["path"].split("?", 1)[0].endswith("/im/v1/images")]
assert len(image_uploads) == 2, image_uploads
assert all(upload["body"]["multipart_bytes"] > 0 for upload in image_uploads), image_uploads
image_filenames = [name for upload in image_uploads for name in upload["body"]["filenames"]]
assert "terminal-e2e-chart.png" in image_filenames, image_filenames
assert "terminal-e2e-flow.png" in image_filenames, image_filenames
assert sum(message["body"].get("msg_type") == "image" for message in messages) == 1, messages

progress_cards = []
for record in records:
    path = record["path"].split("?", 1)[0]
    if path.endswith("/cardkit/v1/cards") and isinstance(record["body"].get("data"), str):
        progress_cards.append(json.loads(record["body"]["data"]))
    elif record["method"] == "PUT" and isinstance(record["body"].get("card", {}).get("data"), str):
        progress_cards.append(json.loads(record["body"]["card"]["data"]))
assert progress_cards, records
for card in progress_cards:
    serialized = json.dumps(card, ensure_ascii=False).lower()
    for forbidden in ["rgba(", "rgb(", "<font color='black'", "<font color='white'", '"background_color": "grey"']:
        assert forbidden not in serialized, (forbidden, card)
    for element in card.get("body", {}).get("elements", []):
        if element.get("tag") == "collapsible_panel":
            assert element.get("background_color") == "default", element
            assert element.get("header", {}).get("title", {}).get("text_color") == "default", element
PY

echo "[feishu-progress-terminal] PASS"
