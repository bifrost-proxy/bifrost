#!/usr/bin/env bash
set -euo pipefail

: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_DIR"

TEST_DIR="$(mktemp -d)"
TEST_HOME="$TEST_DIR/home"
MOCK_BIN_DIR="$TEST_HOME/.local/bin"
MOCK_LOG="$TEST_DIR/mock-traex.log"
BIFROST_LOG="$TEST_DIR/bifrost.log"
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
  rm -rf "$TEST_DIR"
}
trap cleanup EXIT

mkdir -p "$MOCK_BIN_DIR"
python3 - "$MOCK_BIN_DIR/traex" <<'PY'
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
path.write_text(r'''#!/usr/bin/env python3
import json
import os
import sys

log_path = os.environ["MOCK_TRAEX_LOG"]
thread_id = "thread-desktop-path"

def send(value):
    print(json.dumps(value, separators=(",", ":")), flush=True)

with open(log_path, "a", encoding="utf-8") as handle:
    handle.write(json.dumps({"argv": sys.argv[1:], "path": os.environ.get("PATH", "")}) + "\n")

if "--version" in sys.argv:
    print("traex 0.0.0-desktop-path-mock")
    sys.exit(0)

if sys.argv[1:4] != ["app-server", "--listen", "stdio://"]:
    print(f"unexpected args: {sys.argv[1:]}", file=sys.stderr)
    sys.exit(2)

for line in sys.stdin:
    frame = json.loads(line)
    method = frame.get("method")
    request_id = frame.get("id")
    if method == "initialize":
        send({"jsonrpc":"2.0","id":request_id,"result":{}})
    elif method in ("thread/start", "thread/resume"):
        send({"jsonrpc":"2.0","method":"thread/started","params":{"thread":{"id":thread_id}}})
        send({"jsonrpc":"2.0","id":request_id,"result":{"thread":{"id":thread_id}}})
    elif method == "turn/start":
        turn_id = "turn-desktop-path"
        send({"jsonrpc":"2.0","id":request_id,"result":{"turn":{"id":turn_id}}})
        send({"jsonrpc":"2.0","method":"item/completed","params":{"threadId":thread_id,"turnId":turn_id,"item":{"id":"message-desktop-path","type":"agentMessage","text":"BIFROST_DESKTOP_PATH_TRAEX_OK"}}})
        send({"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":thread_id,"turn":{"id":turn_id,"status":"completed"}}})
''', encoding="utf-8")
path.chmod(0o755)
PY

if [[ "${SKIP_BUILD:-false}" != "true" ]]; then
  SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
fi

PATH="/usr/bin:/bin" \
HOME="$TEST_HOME" \
MOCK_TRAEX_LOG="$MOCK_LOG" \
BIFROST_DATA_DIR="$TEST_DIR" \
"$BIFROST_BIN" start \
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
  if ! kill -0 "$BIFROST_PID" >/dev/null 2>&1; then
    tail -160 "$BIFROST_LOG" >&2 || true
    exit 1
  fi
  sleep 0.25
done

python3 - "$BIFROST_PORT" "$REPO_DIR" <<'PY'
import json
import sys
import urllib.request

port, repo_dir = sys.argv[1:3]
config = {
    "version": 1,
    "defaultRunnerId": "traex",
    "runners": {
        "traex": {
            "enabled": True,
            "adapter": "traex",
            "adapterConfig": {"timeoutSecs": 30},
            "workDir": repo_dir,
            "injectBifrostTools": False,
            "skillPaths": [],
            "deliveryMode": "final_reply",
        }
    },
    "channels": {},
}
request = urllib.request.Request(
    f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/chat/config",
    data=json.dumps(config).encode(),
    headers={"content-type": "application/json"},
    method="PATCH",
)
with urllib.request.urlopen(request, timeout=30) as response:
    assert response.status == 200, response.read().decode()

payload = {
    "message": "desktop path regression",
    "sessionKey": "desktop-path-traex",
    "runnerId": "traex",
    "runtime": "external_cli",
}
request = urllib.request.Request(
    f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/chat/stream",
    data=json.dumps(payload).encode(),
    headers={"content-type": "application/json"},
    method="POST",
)
events = []
with urllib.request.urlopen(request, timeout=60) as response:
    for raw_line in response:
        line = raw_line.decode().strip()
        if line:
            events.append(json.loads(line))

finished = [event for event in events if event.get("eventType") == "run_finished"]
assert len(finished) == 1, events
assert finished[0].get("status") == "succeeded", finished[0]
assert finished[0].get("response") == "BIFROST_DESKTOP_PATH_TRAEX_OK", finished[0]
run_id = finished[0]["runId"]

with urllib.request.urlopen(
    f"http://127.0.0.1:{port}/_bifrost/api/im-gateway/chat/runs/{run_id}",
    timeout=30,
) as response:
    detail = json.loads(response.read().decode())

snapshot = detail["snapshot"]
assert snapshot["executable"] == "traex", snapshot
assert "--listen" in snapshot["argFlags"], snapshot
assert "PATH" not in snapshot["envKeys"], snapshot
assert (
    detail["metadata"].get("cli.version") == "traex 0.0.0-desktop-path-mock"
), detail["metadata"]
PY

python3 - "$MOCK_LOG" "$MOCK_BIN_DIR" <<'PY'
import json
import os
import sys

log_path, mock_bin_dir = sys.argv[1:3]
records = [json.loads(line) for line in open(log_path, encoding="utf-8") if line.strip()]
assert records, "mock traex was not executed"
assert any(record["argv"][:3] == ["app-server", "--listen", "stdio://"] for record in records), records
assert any(
    os.path.normpath(mock_bin_dir) in {
        os.path.normpath(entry) for entry in record["path"].split(os.pathsep)
    }
    for record in records
), records
PY

echo "[im-gateway-desktop-path-traex] PASS"
