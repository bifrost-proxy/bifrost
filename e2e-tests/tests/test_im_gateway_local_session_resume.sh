#!/usr/bin/env bash
set -euo pipefail

unset BIFROST_DETACHED_DAEMON_CHILD
unset BIFROST_EXTERNAL_CLI_WORKER
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
export BIFROST_DISABLE_TRAY=1

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
TEST_DIR="$(mktemp -d "$REPO_DIR/.bifrost-e2e-local-resume.XXXXXX")"
BIFROST_LOG="$TEST_DIR/bifrost.log"
ARGV_LOG="$TEST_DIR/runner-argv.jsonl"
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
    echo "[im-local-session-resume] kept test directory: $TEST_DIR" >&2
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

MOCK_RUNNER="$TEST_DIR/mock-runner.py"
python3 - "$TEST_DIR" "$MOCK_RUNNER" <<'PY'
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
mock = pathlib.Path(sys.argv[2])

codex_id = "11111111-1111-1111-1111-111111111111"
traex_id = "22222222-2222-2222-2222-222222222222"
claude_id = "33333333-3333-3333-3333-333333333333"

def write_jsonl(path, rows):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("".join(json.dumps(row) + "\n" for row in rows), encoding="utf-8")

write_jsonl(root / "codex/sessions/2099/08/07/codex.jsonl", [{
    # Keep fixture timestamps ahead of the temporary file mtime so the
    # provider timestamps remain the deterministic winners.
    "timestamp": "2099-08-07T03:01:00Z",
    "type": "session_meta",
    "payload": {"id": codex_id, "timestamp": "2099-08-07T03:01:00Z"},
}])
write_jsonl(root / "codex/session_index.jsonl", [{
    "id": codex_id,
    "thread_name": "Codex local title",
    "updated_at": "2099-08-07T03:02:00Z",
}])
write_jsonl(root / "trae/cli/sessions/2099/08/07/traex.jsonl", [{
    "timestamp": "2099-08-07T04:01:00Z",
    "type": "session_meta",
    "payload": {"id": traex_id, "timestamp": "2099-08-07T04:01:00Z"},
}])
write_jsonl(root / "trae/cli/history.jsonl", [{
    "session_id": traex_id,
    "ts": 4089758580,
    "text": "Traex local title",
}])
write_jsonl(root / "claude/projects/project/claude.jsonl", [
    {
        "type": "user",
        "sessionId": claude_id,
        "timestamp": "2099-08-07T05:01:00Z",
        "message": {"content": "Claude fallback title"},
    },
    {"type": "ai-title", "sessionId": claude_id, "aiTitle": "Claude local title"},
])
write_jsonl(root / "claude/history.jsonl", [{
    "sessionId": claude_id,
    "timestamp": 4089762120000,
    "display": "Claude history title",
}])

mock.write_text(r'''#!/usr/bin/env python3
import json
import os
import sys

args = sys.argv[1:]
adapter = os.environ["BIFROST_MOCK_ADAPTER"]
with open(os.environ["BIFROST_MOCK_ARGV_LOG"], "a", encoding="utf-8") as file:
    file.write(json.dumps({"adapter": adapter, "args": args}) + "\n")

if "--version" in args or args[:2] == ["debug", "models"]:
    print(f"{adapter} mock 1.0")
    raise SystemExit(0)

if adapter == "claude_code":
    sys.stdin.readline()
    selected = args[args.index("--resume") + 1] if "--resume" in args else "new-claude"
    print(json.dumps({"type": "assistant", "message": {"content": [{"type": "text", "text": "CLAUDE_RESUME_OK"}]}}))
    print(json.dumps({"type": "result", "subtype": "success", "is_error": False, "result": "CLAUDE_RESUME_OK", "session_id": selected}))
else:
    sys.stdin.read()
    selected = "new-thread"
    if "resume" in args:
        position = args.index("resume")
        candidates = [arg for arg in args[position + 1:] if not arg.startswith("-")]
        if candidates:
            selected = candidates[-1]
    print(json.dumps({"type": "thread.started", "thread_id": selected}))
    print(json.dumps({"type": "assistant_final", "content": adapter.upper() + "_RESUME_OK"}))
    print(json.dumps({"type": "turn.completed", "usage": {"input_tokens": 1, "output_tokens": 1}}))
''', encoding="utf-8")
mock.chmod(0o755)
PY

if [[ "${SKIP_BUILD:-false}" != "true" ]]; then
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

CODEX_HOME="$TEST_DIR/codex" \
TRAE_HOME="$TEST_DIR/trae" \
CLAUDE_CONFIG_DIR="$TEST_DIR/claude" \
BIFROST_DATA_DIR="$TEST_DIR/data" \
  "$BIFROST_BIN" start \
  --host 127.0.0.1 \
  -p "$BIFROST_PORT" \
  --unsafe-ssl \
  --skip-cert-check \
  --no-system-proxy \
  >"$BIFROST_LOG" 2>&1 &
BIFROST_PID=$!
wait_http

python3 - "$BIFROST_PORT" "$MOCK_RUNNER" "$ARGV_LOG" <<'PY'
import json
import pathlib
import sys
import urllib.error
import urllib.request

port, mock_runner, argv_log = sys.argv[1:4]
api = f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/chat"

providers = [
    ("codex-resume", "codex", "11111111-1111-1111-1111-111111111111", "Codex local title", "CODEX_RESUME_OK"),
    ("traex-resume", "traex", "22222222-2222-2222-2222-222222222222", "Traex local title", "TRAEX_RESUME_OK"),
    ("claude-resume", "claude_code", "33333333-3333-3333-3333-333333333333", "Claude local title", "CLAUDE_RESUME_OK"),
]

config = {
    "version": 1,
    "defaultRunnerId": providers[0][0],
    "runners": {},
    "channels": {},
}
for runner_id, adapter, *_ in providers:
    config["runners"][runner_id] = {
        "enabled": True,
        "adapter": adapter,
        "adapterConfig": {
            "executable": mock_runner,
            "env": {
                "BIFROST_MOCK_ADAPTER": adapter,
                "BIFROST_MOCK_ARGV_LOG": argv_log,
            },
            "timeoutSecs": 30,
            "transport": "exec",
        },
        "injectBifrostTools": False,
        "skillPaths": [],
        "deliveryMode": "final_reply",
    }

request = urllib.request.Request(
    api + "/config",
    data=json.dumps(config).encode(),
    headers={"content-type": "application/json"},
    method="PATCH",
)
with urllib.request.urlopen(request, timeout=30) as response:
    assert response.status == 200, response.read().decode()

def stream(runner_id, session_key, message, expected_status=200):
    request = urllib.request.Request(
        api + "/stream",
        data=json.dumps({
            "message": message,
            "providerId": "web-e2e",
            "runnerId": runner_id,
            "sessionKey": session_key,
        }).encode(),
        headers={"content-type": "application/json"},
        method="POST",
    )
    try:
        response = urllib.request.urlopen(request, timeout=60)
    except urllib.error.HTTPError as error:
        body = error.read().decode()
        assert error.code == expected_status, body
        return [json.loads(line) for line in body.splitlines() if line]
    with response:
        assert response.status == expected_status, response.status
        return [json.loads(line) for line in response.read().decode().splitlines() if line]

def final(events):
    finished = [event for event in events if event.get("eventType") == "run_finished"]
    assert len(finished) == 1, events
    return finished[0].get("response", "")

def business_invocations():
    path = pathlib.Path(argv_log)
    if not path.exists():
        return []
    return [
        row for row in map(json.loads, path.read_text().splitlines())
        if "--version" not in row["args"] and row["args"][:2] != ["debug", "models"]
    ]

for runner_id, adapter, session_id, title, expected in providers:
    key = "e2e:" + adapter
    before = len(business_invocations())
    listing = final(stream(runner_id, key, "/resume"))
    assert session_id in listing, listing
    assert title in listing, listing
    assert "2099-08-07T" in listing, listing
    assert len(business_invocations()) == before, business_invocations()

    selected = final(stream(runner_id, key, f"/resume {session_id[:12]}"))
    assert session_id in selected and "下一条普通消息" in selected, selected
    assert len(business_invocations()) == before, business_invocations()

    response = final(stream(runner_id, key, "continue local session"))
    assert expected in response, response
    invocation = business_invocations()[-1]
    assert invocation["adapter"] == adapter, invocation
    args = invocation["args"]
    if adapter == "claude_code":
        assert "--resume" in args and session_id in args, args
    else:
        assert "resume" in args and session_id in args, args

bad = stream("traex-resume", "e2e:traex-bad", "/resume 11111111-1111", expected_status=400)
assert bad[0]["eventType"] == "run_failed", bad
assert "没有找到" in bad[0]["error"], bad

print("[im-local-session-resume] PASS")
PY
