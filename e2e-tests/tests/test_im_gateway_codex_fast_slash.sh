#!/usr/bin/env bash
set -euo pipefail

unset BIFROST_DETACHED_DAEMON_CHILD
unset BIFROST_EXTERNAL_CLI_WORKER
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
export BIFROST_DISABLE_TRAY=1

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
TEST_DIR="$(mktemp -d "$REPO_DIR/.bifrost-e2e-codex-fast.XXXXXX")"
BIFROST_LOG="$TEST_DIR/bifrost.log"
CODEX_ARGV_LOG="$TEST_DIR/codex-argv.log"
CODEX_STDIN_LOG="$TEST_DIR/codex-stdin.log"
TRAEX_ARGV_LOG="$TEST_DIR/traex-argv.log"
BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
BIFROST_PORT="${BIFROST_PORT:-$(python3 - <<'PY'
import socket

with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
)}"

cleanup() {
  if [[ -n "${BIFROST_PID:-}" ]]; then
    kill "$BIFROST_PID" >/dev/null 2>&1 || true
    wait "$BIFROST_PID" >/dev/null 2>&1 || true
  fi
  if [[ "${KEEP_TEST_DIR:-false}" == "true" ]]; then
    echo "[im-codex-fast] kept test directory: $TEST_DIR" >&2
  else
    rm -rf "$TEST_DIR"
  fi
}
trap cleanup EXIT

wait_http() {
  for _ in $(seq 1 180); do
    if curl -fsS --noproxy '*' \
      "http://127.0.0.1:$BIFROST_PORT/_bifrost/api/proxy/address" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.25
  done
  tail -160 "$BIFROST_LOG" >&2 || true
  return 1
}

inject() {
  local provider_id="$1"
  local owner_id="$2"
  local message_id="$3"
  local text="$4"
  python3 - "$BIFROST_PORT" "$provider_id" "$owner_id" "$message_id" "$text" <<'PY'
import json
import sys
import urllib.request

port, provider_id, owner_id, message_id, text = sys.argv[1:6]
payload = {
    "providerId": provider_id,
    "chatId": f"chat-{provider_id}",
    "chatType": "p2p",
    "userId": owner_id,
    "userName": "Fast Mode E2E",
    "messageId": message_id,
    "eventId": "event-" + message_id,
    "text": text,
    "mentionBot": False,
}
request = urllib.request.Request(
    f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/debug/mock-inbound",
    data=json.dumps(payload, ensure_ascii=False).encode(),
    headers={"content-type": "application/json"},
    method="POST",
)
with urllib.request.urlopen(request, timeout=30) as response:
    assert response.status == 200, response.read().decode()
PY
}

wait_for_file_pattern() {
  local path="$1"
  local pattern="$2"
  for _ in $(seq 1 240); do
    if grep -Fq -- "$pattern" "$path" 2>/dev/null; then
      return 0
    fi
    sleep 0.05
  done
  echo "[im-codex-fast] missing pattern '$pattern' in $path" >&2
  [[ -f "$path" ]] && sed -n '1,120p' "$path" >&2 || true
  tail -160 "$BIFROST_LOG" >&2 || true
  return 1
}

wait_for_outbound_pattern_count() {
  local path="$1"
  local pattern="$2"
  local expected="$3"
  for _ in $(seq 1 240); do
    if python3 - "$path" "$pattern" "$expected" <<'PY'
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
pattern = sys.argv[2]
expected = int(sys.argv[3])
if not path.exists():
    raise SystemExit(1)
messages = json.loads(path.read_text(encoding="utf-8")).get("messages", [])
count = sum(
    1 for item in messages
    if item.get("direction") == "outbound"
    and pattern in (item.get("content") or "")
)
raise SystemExit(0 if count >= expected else 1)
PY
    then
      return 0
    fi
    sleep 0.05
  done
  echo "[im-codex-fast] missing $expected outbound messages containing '$pattern'" >&2
  [[ -f "$path" ]] && sed -n '1,160p' "$path" >&2 || true
  return 1
}

MOCK_CODEX="$TEST_DIR/mock-codex"
MOCK_TRAEX="$TEST_DIR/mock-traex"
cat >"$MOCK_CODEX" <<'SH'
#!/usr/bin/env sh
printf '%s\n' "$*" >>"$BIFROST_CODEX_ARGV_LOG"
prompt="$(cat)"
printf '%s\n' "$prompt" >>"$BIFROST_CODEX_STDIN_LOG"
case "$prompt" in
  *"hold for busy fast switch"*) sleep 2 ;;
esac
printf '%s\n' '{"type":"thread.started","thread_id":"thread-codex-fast"}'
printf '%s\n' '{"type":"assistant_final","content":"CODEX_FAST_E2E_OK"}'
printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}'
SH
chmod +x "$MOCK_CODEX"
cat >"$MOCK_TRAEX" <<'SH'
#!/usr/bin/env sh
printf '%s\n' "$*" >>"$BIFROST_TRAEX_ARGV_LOG"
cat >/dev/null
printf '%s\n' '{"type":"thread.started","thread_id":"thread-traex-fast"}'
printf '%s\n' '{"type":"assistant_final","content":"TRAEX_FAST_E2E_OK"}'
printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}'
SH
chmod +x "$MOCK_TRAEX"

if [[ "${SKIP_BUILD:-false}" != "true" ]]; then
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

BIFROST_DATA_DIR="$TEST_DIR" "$BIFROST_BIN" start \
  --host 127.0.0.1 \
  -p "$BIFROST_PORT" \
  --unsafe-ssl \
  --skip-cert-check \
  --no-system-proxy \
  >"$BIFROST_LOG" 2>&1 &
BIFROST_PID=$!
wait_http

python3 - "$BIFROST_PORT" "$MOCK_CODEX" "$CODEX_ARGV_LOG" "$CODEX_STDIN_LOG" "$MOCK_TRAEX" "$TRAEX_ARGV_LOG" <<'PY'
import json
import sys
import urllib.request

port, mock_codex, codex_log, codex_stdin_log, mock_traex, traex_log = sys.argv[1:7]
api = f"http://127.0.0.1:{port}/_bifrost/api/im-gateway"

def request(path, payload, method):
    req = urllib.request.Request(
        api + path,
        data=json.dumps(payload).encode(),
        headers={"content-type": "application/json"},
        method=method,
    )
    with urllib.request.urlopen(req, timeout=30) as response:
        assert response.status == 200, response.read().decode()

request("/chat/config", {
    "version": 1,
    "defaultRunnerId": "codex-fast-e2e",
    "runners": {
        "codex-fast-e2e": {
            "enabled": True,
            "adapter": "codex",
            "adapterConfig": {
                "executable": mock_codex,
                "env": {
                    "BIFROST_CODEX_ARGV_LOG": codex_log,
                    "BIFROST_CODEX_STDIN_LOG": codex_stdin_log,
                },
                "timeoutSecs": 30,
            },
            "injectBifrostTools": False,
            "skillPaths": [],
            "deliveryMode": "final_reply",
        },
        "traex-fast-e2e": {
            "enabled": True,
            "adapter": "traex",
            "adapterConfig": {
                "executable": mock_traex,
                "env": {"BIFROST_TRAEX_ARGV_LOG": traex_log},
                "timeoutSecs": 30,
            },
            "injectBifrostTools": False,
            "skillPaths": [],
            "deliveryMode": "final_reply",
        },
    },
    "channels": {},
}, "PATCH")

for provider_id, owner_id, runner in (
    ("codex-fast-provider", "codex-fast-owner", "codex-fast-e2e"),
    ("traex-fast-provider", "traex-fast-owner", "traex-fast-e2e"),
):
    request("/providers", {
        "id": provider_id,
        "provider_type": "feishu",
        "display_name": "Fast Slash E2E",
        "enabled": True,
        "base_url": "http://127.0.0.1:9/open-apis",
        "app_id": "cli_fast_e2e",
        "app_secret": "fast-e2e-secret",
        "owner_open_id": owner_id,
        "event_connection_enabled": False,
        "agent_config": {"runner": runner},
    }, "POST")
PY

inject "codex-fast-provider" "codex-fast-owner" "codex-fast-off" "/fast off"
wait_for_file_pattern "$TEST_DIR/agent/im_gateway/session_state.json" '"serviceTierOverride": "default"'
wait_for_file_pattern "$TEST_DIR/admin/im_gateway_message_logs.json" '切换到标准模式'
[[ ! -e "$CODEX_ARGV_LOG" ]] || [[ ! -s "$CODEX_ARGV_LOG" ]]

inject "codex-fast-provider" "codex-fast-owner" "codex-normal-turn" "run in standard mode"
wait_for_file_pattern "$CODEX_ARGV_LOG" 'service_tier="default"'

inject "codex-fast-provider" "codex-fast-owner" "codex-fast-toggle" "/fast"
wait_for_file_pattern "$TEST_DIR/agent/im_gateway/session_state.json" '"serviceTierOverride": "fast"'
inject "codex-fast-provider" "codex-fast-owner" "codex-fast-status" "/fast status"
wait_for_file_pattern "$TEST_DIR/admin/im_gateway_message_logs.json" '使用快速模式'

inject "codex-fast-provider" "codex-fast-owner" "codex-fast-turn" "run in fast mode"
wait_for_file_pattern "$CODEX_ARGV_LOG" 'service_tier="fast"'

inject "codex-fast-provider" "codex-fast-owner" "codex-busy-turn" "hold for busy fast switch"
wait_for_file_pattern "$CODEX_STDIN_LOG" 'hold for busy fast switch'
inject "codex-fast-provider" "codex-fast-owner" "codex-busy-off" "/fast off"
wait_for_file_pattern "$TEST_DIR/agent/im_gateway/session_state.json" '"serviceTierOverride": "default"'
inject "codex-fast-provider" "codex-fast-owner" "codex-busy-queue" "/q queued after busy switch"
wait_for_file_pattern "$CODEX_STDIN_LOG" 'queued after busy switch'

inject "traex-fast-provider" "traex-fast-owner" "traex-fast-reject" "/fast off"
wait_for_file_pattern "$TEST_DIR/admin/im_gateway_message_logs.json" '当前 Runner 不支持 `/fast` 命令'
inject "traex-fast-provider" "traex-fast-owner" "traex-fast-invalid-reject" "/fast invalid"
wait_for_outbound_pattern_count \
  "$TEST_DIR/admin/im_gateway_message_logs.json" \
  '当前 Runner 不支持 `/fast` 命令' \
  2
[[ ! -e "$TRAEX_ARGV_LOG" ]] || [[ ! -s "$TRAEX_ARGV_LOG" ]]

python3 - "$TEST_DIR" "$CODEX_ARGV_LOG" "$CODEX_STDIN_LOG" <<'PY'
import json
import pathlib
import sys

test_dir = pathlib.Path(sys.argv[1])
all_codex_argv = pathlib.Path(sys.argv[2]).read_text(encoding="utf-8").splitlines()
codex_stdin = pathlib.Path(sys.argv[3]).read_text(encoding="utf-8")
codex_argv = [line for line in all_codex_argv if line.startswith("exec ")]
assert len(codex_argv) == 4, all_codex_argv
assert 'service_tier="default"' in codex_argv[0], codex_argv
assert 'service_tier="fast"' in codex_argv[1], codex_argv
assert 'service_tier="fast"' in codex_argv[2], codex_argv
assert 'service_tier="default"' in codex_argv[3], codex_argv
assert "/fast" not in codex_stdin, codex_stdin

state = json.loads(
    (test_dir / "agent" / "im_gateway" / "session_state.json").read_text(encoding="utf-8")
)
codex_sessions = [
    value for value in state["sessions"].values()
    if value.get("runnerId") == "codex-fast-e2e"
]
assert len(codex_sessions) == 1, codex_sessions
assert codex_sessions[0]["serviceTierOverride"] == "default", codex_sessions[0]
assert codex_sessions[0]["serviceTierOverrideSource"] == "session slash command", codex_sessions[0]

logs = json.loads(
    (test_dir / "admin" / "im_gateway_message_logs.json").read_text(encoding="utf-8")
)["messages"]
outbound = [
    item.get("content") or ""
    for item in logs
    if item.get("direction") == "outbound"
]
assert any("切换到标准模式" in content for content in outbound), outbound
assert any("切换到快速模式" in content for content in outbound), outbound
assert any("使用快速模式" in content for content in outbound), outbound
unsupported = [
    content for content in outbound
    if "当前 Runner 不支持 `/fast` 命令" in content
]
assert len(unsupported) == 2, unsupported
PY

echo "[im-codex-fast] PASS"
