#!/bin/bash
set -euo pipefail

unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY all_proxy ALL_PROXY no_proxy NO_PROXY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
SYNC_SERVER_DIR="$REPO_DIR/packages/bifrost-sync-server"

source "$SCRIPT_DIR/../test_utils/assert.sh"
source "$SCRIPT_DIR/../test_utils/admin_client.sh"

pick_free_port() {
    python3 - <<'PY'
import socket
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

RELAY_PORT="$(pick_free_port)"
RELAY_URL="http://127.0.0.1:${RELAY_PORT}"
RELAY_DATA_DIR="$(mktemp -d)"
RELAY_LOG="$(mktemp)"
TMPDIR="$(mktemp -d)"
MOCK_PORT="$(pick_free_port)"
MOCK_DIR="$(mktemp -d)"
MOCK_LOG="$(mktemp)"

ADMIN_PORT="${ADMIN_PORT:-$(pick_free_port)}"
export ADMIN_PORT
export ADMIN_HOST="127.0.0.1"
export ADMIN_PATH_PREFIX="/_bifrost"
export ADMIN_BASE_URL="http://127.0.0.1:${ADMIN_PORT}${ADMIN_PATH_PREFIX}"
BIFROST_DATA_DIR="${BIFROST_DATA_DIR:-$SCRIPT_DIR/../../.bifrost-e2e-remote-invoke-ssh-$RANDOM}"
export BIFROST_DATA_DIR

cleanup() {
    admin_cleanup_bifrost || true
    if [[ -n "${RELAY_PID:-}" ]] && kill -0 "$RELAY_PID" 2>/dev/null; then
        kill "$RELAY_PID" 2>/dev/null || true
        wait "$RELAY_PID" 2>/dev/null || true
    fi
    if [[ -n "${MOCK_PID:-}" ]] && kill -0 "$MOCK_PID" 2>/dev/null; then
        kill "$MOCK_PID" 2>/dev/null || true
        wait "$MOCK_PID" 2>/dev/null || true
    fi
    rm -rf "$RELAY_DATA_DIR" "$TMPDIR" "$BIFROST_DATA_DIR" "${CALLER_DATA_DIR:-}" "$MOCK_DIR" >/dev/null 2>&1 || true
    rm -f "$RELAY_LOG" "$MOCK_LOG" >/dev/null 2>&1 || true
}
trap cleanup EXIT

log() { echo "[remote-invoke-ssh-e2e] $*"; }

start_local_relay() {
    log "Starting local bifrost-sync-server on port $RELAY_PORT..."
    (
        cd "$SYNC_SERVER_DIR" && \
            npx tsx src/cli.ts -p "$RELAY_PORT" -d "$RELAY_DATA_DIR" --enable-remote-invoke
    ) >"$RELAY_LOG" 2>&1 &
    RELAY_PID=$!

    for _ in $(seq 1 60); do
        local code
        code=$(curl -s -o /dev/null -w '%{http_code}' "${RELAY_URL}/v4/remote-invoke/client/register" || true)
        if echo "$code" | grep -Eq '4[0-9][0-9]|200'; then
            return 0
        fi
        sleep 0.5
    done

    log "Relay did not become ready"
    tail -n 120 "$RELAY_LOG" || true
    exit 1
}

json_get() {
    local file="$1"
    local expr="$2"
    python3 - "$file" "$expr" <<'PY'
import json
import sys

obj = json.load(open(sys.argv[1]))
expr = sys.argv[2]
parts = expr.split(".")
cur = obj
for part in parts:
    if part == "":
        continue
    if isinstance(cur, dict):
        cur = cur.get(part)
    else:
        cur = None
        break
print("" if cur is None else cur)
PY
}

assert_python() {
    local file="$1"
    local snippet="$2"
    python3 - "$file" "$snippet" <<'PY'
import json
import re
import sys

obj = json.load(open(sys.argv[1]))
snippet = sys.argv[2]
namespace = {"obj": obj, "re": re}
exec(snippet, namespace)
PY
}

wait_for_worker_connected() {
    local status_json="$1"
    for _ in $(seq 1 60); do
        curl -s "${ADMIN_BASE_URL}/api/remote-invoke/status" >"$status_json"
        if [[ "$(json_get "$status_json" "state")" == "Connected" ]]; then
            return 0
        fi
        sleep 1
    done

    return 1
}

open_call_and_collect() {
    local command="$1"
    local args_json="$2"
    local output_file="$3"
    python3 - "$RELAY_URL" "$CLIENT_INSTANCE_ID" "$FINGERPRINT" "$MATCH_GRANT" "$command" "$args_json" "$output_file" <<'PY'
import json
import sys
import urllib.request

relay_url, client_id, fingerprint, grant_id, command, args_json, output_file = sys.argv[1:8]
payload = {
    "grant_id": grant_id,
    "client_instance_id": client_id,
    "caller_fingerprint": fingerprint,
    "command": {
        "command": command,
        "args_json": args_json,
    },
}
req = urllib.request.Request(
    relay_url + "/v4/remote-invoke/calls/open",
    data=json.dumps(payload).encode(),
    headers={"Content-Type": "application/json"},
)
with urllib.request.urlopen(req, timeout=30) as resp:
    opened = json.loads(resp.read().decode())["data"]
call_id = opened["call_id"]
token = opened["relay_token"]

stream_req = urllib.request.Request(
    relay_url + f"/v4/remote-invoke/calls/{call_id}/events",
    headers={"Authorization": "Bearer " + token},
)
stdout = []
exit_payload = None
with urllib.request.urlopen(stream_req, timeout=120) as resp:
    event = None
    data_lines = []
    while True:
        line = resp.readline()
        if not line:
            break
        text = line.decode().rstrip("\n")
        if text.startswith("event: "):
            event = text[7:]
        elif text.startswith("data: "):
            data_lines.append(text[6:])
        elif text == "":
            if event:
                payload = json.loads("\n".join(data_lines))
                if event == "frame":
                    envelope = json.loads(payload["envelope_json"])
                    stdout.append(envelope.get("ciphertext", ""))
                elif event == "exit":
                    exit_payload = payload
                    break
            event = None
            data_lines = []

with open(output_file, "w") as fh:
    json.dump(
        {
            "call_id": call_id,
            "stdout": "".join(stdout),
            "exit": exit_payload,
        },
        fh,
    )
PY
}

start_local_relay

log "Build bifrost (release)..."
(cd "$REPO_DIR" && cargo build --release --bin bifrost >/dev/null 2>&1)

log "Start bifrost client on port $ADMIN_PORT..."
mkdir -p "$BIFROST_DATA_DIR"
cat >"$BIFROST_DATA_DIR/config.toml" <<EOF
[sync]
remote_base_url = "$RELAY_URL"
EOF
admin_start_bifrost

log "Register relay sync user and save session"
SYNC_USER_ID="remote_invoke_ssh_${RANDOM}"
REGISTER_JSON="$TMPDIR/register.json"
curl -s -X POST "${RELAY_URL}/v4/sso/register" \
    -H "Content-Type: application/json" \
    -d "{\"user_id\":\"${SYNC_USER_ID}\",\"password\":\"remote_invoke_ssh_123\",\"nickname\":\"Remote Invoke SSH\"}" \
    >"$REGISTER_JSON"
SYNC_TOKEN="$(json_get "$REGISTER_JSON" "data.token")"
assert_not_empty "$SYNC_TOKEN" "relay 注册 token 不应为空"
curl -s -X POST "${ADMIN_BASE_URL}/api/sync/session" \
    -H "Content-Type: application/json" \
    -d "{\"token\":\"${SYNC_TOKEN}\"}" >/dev/null

log "Wait for remote invoke worker to connect..."
STATUS_JSON="$TMPDIR/status.json"
wait_for_worker_connected "$STATUS_JSON"
assert_equals "Connected" "$(json_get "$STATUS_JSON" "state")" "worker 应连接到 relay"

log "Fetch client identity"
IDENTITY_JSON="$TMPDIR/identity.json"
curl -s "${ADMIN_BASE_URL}/api/remote-invoke/identity" >"$IDENTITY_JSON"
CLIENT_INSTANCE_ID="$(json_get "$IDENTITY_JSON" "instance_id")"
assert_not_empty "$CLIENT_INSTANCE_ID" "client instance id 不应为空"

log "Start mock target on port $MOCK_PORT..."
python3 -m http.server "$MOCK_PORT" --directory "$MOCK_DIR" >"$MOCK_LOG" 2>&1 &
MOCK_PID=$!

log "Create SSH key"
CREATE_JSON="$TMPDIR/create.json"
curl -s -X POST "${ADMIN_BASE_URL}/api/remote-invoke/ssh-key" \
    -H "Content-Type: application/json" \
    -d '{"label":"CI Agent","grant_mode":"30m"}' >"$CREATE_JSON"
assert_python "$CREATE_JSON" 'assert re.match(r"^BF-[0-9A-F]{16}$", obj["device_code"])'
assert_python "$CREATE_JSON" 'assert obj["grant_mode"] == "permanent"'
DEVICE_CODE="$(json_get "$CREATE_JSON" "device_code")"
FINGERPRINT="$(json_get "$CREATE_JSON" "ssh_key_fingerprint")"
assert_not_empty "$FINGERPRINT" "ssh key fingerprint 不应为空"

log "Export private key and ensure it matches the active key"
EXPORT_JSON="$TMPDIR/export.json"
curl -s "${ADMIN_BASE_URL}/api/remote-invoke/ssh-key/private-key" >"$EXPORT_JSON"
assert_equals "$DEVICE_CODE" "$(json_get "$EXPORT_JSON" "device_code")" "导出的 device_code 应与 active key 一致"
assert_equals "$FINGERPRINT" "$(json_get "$EXPORT_JSON" "ssh_key_fingerprint")" "导出的 fingerprint 应与 active key 一致"

log "Reset SSH key"
RESET_JSON="$TMPDIR/reset.json"
curl -s -X POST "${ADMIN_BASE_URL}/api/remote-invoke/ssh-key/reset" >"$RESET_JSON"
NEW_DEVICE_CODE="$(json_get "$RESET_JSON" "device_code")"
assert_not_empty "$NEW_DEVICE_CODE" "reset 后 device_code 不应为空"
if [[ "$NEW_DEVICE_CODE" == "$DEVICE_CODE" ]]; then
    echo "reset did not rotate device_code" >&2
    exit 1
fi
DEVICE_CODE="$NEW_DEVICE_CODE"
FINGERPRINT="$(json_get "$RESET_JSON" "ssh_key_fingerprint")"
KEY_FILE="$(json_get "$RESET_JSON" "bifrost_key_file")"

log "Wait for worker reconnect after reset"
wait_for_worker_connected "$STATUS_JSON"
assert_equals "Connected" "$(json_get "$STATUS_JSON" "state")" "reset 后 worker 应重新连接到 relay"

log "Wait for relay route sync after reset"
for _ in $(seq 1 30); do
    HTTP_CODE=$(curl -s -o "$TMPDIR/challenge.json" -w '%{http_code}' \
        -X POST "${RELAY_URL}/v4/remote-invoke/ssh/challenge" \
        -H "Content-Type: application/json" \
        -d "{\"device_code\":\"${DEVICE_CODE}\"}" || true)
    if [[ "$HTTP_CODE" == "200" ]]; then
        break
    fi
    sleep 1
done
assert_equals "200" "$HTTP_CODE" "新 device_code 应能拿到 challenge"

log "Use CLI remote connect --ssh-key"
CALLER_DATA_DIR="$(mktemp -d)"
CLI_CONNECT_OUTPUT="$TMPDIR/cli_connect.out"
printf '%s' "$KEY_FILE" >"$TMPDIR/cli-test.bifrost"
BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$REPO_DIR/target/release/bifrost" remote connect --ssh-key "$TMPDIR/cli-test.bifrost" --relay-url "$RELAY_URL" \
    >"$CLI_CONNECT_OUTPUT" 2>&1
grep -q "Connected with SSH key" "$CLI_CONNECT_OUTPUT"
CALLER_CONNECTIONS_JSON="$CALLER_DATA_DIR/remote-connections.json"
assert_python "$CALLER_CONNECTIONS_JSON" '
assert obj["connections"], "caller 应写入 remote-connections.json"
conn = obj["connections"][0]
assert conn["client_instance_id"] == "'"$CLIENT_INSTANCE_ID"'"
assert conn["grant_mode"] == "permanent"
assert conn["caller_fingerprint"] == "'"$FINGERPRINT"'"
assert conn["device_code"] == "'"$DEVICE_CODE"'"
assert conn["auth_method"] == "ssh_publickey"
'

log "Wait for ssh_publickey grant created by CLI"
MATCH_GRANT=""
for _ in $(seq 1 60); do
    GRANTS_JSON="$TMPDIR/grants.json"
    curl -s "${ADMIN_BASE_URL}/api/remote-invoke/grants" >"$GRANTS_JSON"
    MATCH_GRANT="$(python3 - "$GRANTS_JSON" "$FINGERPRINT" <<'PY'
import json
import sys
obj = json.load(open(sys.argv[1]))
for grant in obj.get("grants", []):
    if grant.get("auth_method") == "ssh_publickey" and grant.get("ssh_key_fingerprint") == sys.argv[2]:
        print(grant["grant_id"])
        break
PY
)"
    if [[ -n "$MATCH_GRANT" ]]; then
        break
    fi
    sleep 0.5
done
assert_not_empty "$MATCH_GRANT" "应创建 ssh_publickey grant"
assert_python "$GRANTS_JSON" '
for grant in obj.get("grants", []):
    if grant.get("grant_id") == "'"$MATCH_GRANT"'":
        assert grant.get("grant_mode") == "permanent"
        assert grant.get("expires_at") in (None, "")
        break
else:
    raise AssertionError("grant not found")
'

KEY_AFTER_USE_JSON="$TMPDIR/key_after_use.json"
curl -s "${ADMIN_BASE_URL}/api/remote-invoke/ssh-key" >"$KEY_AFTER_USE_JSON"
assert_python "$KEY_AFTER_USE_JSON" 'assert obj["last_used_at"] is not None'

log "Execute remote status via saved SSH connection"
CLI_STATUS_OUTPUT="$TMPDIR/cli_status.out"
BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$REPO_DIR/target/release/bifrost" remote status --relay-url "$RELAY_URL" \
    >"$CLI_STATUS_OUTPUT" 2>&1
grep -q "Using authorization" "$CLI_STATUS_OUTPUT"

log "Generate proxied traffic for search.get and traffic.get"
MARKER="remote-invoke-ssh-${RANDOM}-${RANDOM}"
python3 - "$MOCK_DIR" "$MARKER" <<'PY'
from pathlib import Path
import sys

mock_dir = Path(sys.argv[1])
marker = sys.argv[2]
(mock_dir / f"{marker}.txt").write_text((marker + "\n") * 5000)
PY
curl -sS --proxy "http://127.0.0.1:${ADMIN_PORT}" \
    "http://127.0.0.1:${MOCK_PORT}/${MARKER}.txt" >/dev/null

TRAFFIC_JSON="$TMPDIR/traffic.json"
TRAFFIC_ID=""
for _ in $(seq 1 20); do
    curl -s "${ADMIN_BASE_URL}/api/traffic?limit=50" >"$TRAFFIC_JSON"
    TRAFFIC_ID="$(python3 - "$TRAFFIC_JSON" "$MARKER" <<'PY'
import json
import sys
obj = json.load(open(sys.argv[1]))
for rec in obj.get("records", []):
    path = rec.get("path") or rec.get("p") or ""
    if sys.argv[2] in path:
        print(rec.get("id") or "")
        break
PY
)"
    if [[ -n "$TRAFFIC_ID" ]]; then
        break
    fi
    sleep 1
done
assert_not_empty "$TRAFFIC_ID" "应生成可供 remote traffic get 查询的流量"

log "Verify reusable SSH grant exists on relay"
REUSABLE_JSON="$TMPDIR/reusable.json"
curl -s "${RELAY_URL}/v4/remote-invoke/grants/reusable?client_instance_id=${CLIENT_INSTANCE_ID}&caller_fingerprint=${FINGERPRINT}" >"$REUSABLE_JSON"
assert_python "$REUSABLE_JSON" '
assert obj["data"]["grant_id"] == "'"$MATCH_GRANT"'"
assert obj["data"]["grant_mode"] == "permanent"
assert obj["data"]["expires_at"] in (None, "")
'

log "Execute search.get via SSH grant"
SEARCH_RESULT_JSON="$TMPDIR/search_result.json"
SEARCH_ARGS_JSON="$(python3 -c 'import json,sys; print(json.dumps({"query": sys.argv[1], "limit": 10}, separators=(",",":")))' "$MARKER")"
open_call_and_collect "search.get" "$SEARCH_ARGS_JSON" "$SEARCH_RESULT_JSON"
assert_python "$SEARCH_RESULT_JSON" 'assert obj["exit"]["exit_code"] == 0; assert "'"$MARKER"'" in obj["stdout"]'
SEARCH_CALL_ID="$(json_get "$SEARCH_RESULT_JSON" "call_id")"
SEARCH_CALL_INFO_JSON="$TMPDIR/search_call.json"
curl -s "${ADMIN_BASE_URL}/api/remote-invoke/calls/${SEARCH_CALL_ID}" >"$SEARCH_CALL_INFO_JSON"
assert_python "$SEARCH_CALL_INFO_JSON" 'assert str(obj["call"]["status"]).lower() == "completed"'

log "Execute traffic.get via SSH grant"
TRAFFIC_RESULT_JSON="$TMPDIR/traffic_result.json"
TRAFFIC_ARGS_JSON="$(python3 -c 'import json,sys; print(json.dumps({"id": sys.argv[1], "response_body": True}, separators=(",",":")))' "$TRAFFIC_ID")"
open_call_and_collect "traffic.get" "$TRAFFIC_ARGS_JSON" "$TRAFFIC_RESULT_JSON"
assert_python "$TRAFFIC_RESULT_JSON" 'assert obj["exit"]["exit_code"] == 0; assert "'"$MARKER"'" in obj["stdout"]; assert "'"$TRAFFIC_ID"'" in obj["stdout"]'
TRAFFIC_CALL_ID="$(json_get "$TRAFFIC_RESULT_JSON" "call_id")"
TRAFFIC_CALL_INFO_JSON="$TMPDIR/traffic_call.json"
curl -s "${ADMIN_BASE_URL}/api/remote-invoke/calls/${TRAFFIC_CALL_ID}" >"$TRAFFIC_CALL_INFO_JSON"
assert_python "$TRAFFIC_CALL_INFO_JSON" 'assert str(obj["call"]["status"]).lower() == "completed"'

log "Revoke SSH key"
REVOKE_JSON="$TMPDIR/revoke.json"
curl -s -X DELETE "${ADMIN_BASE_URL}/api/remote-invoke/ssh-key" >"$REVOKE_JSON"
assert_python "$REVOKE_JSON" 'assert obj["success"] is True'
NULL_BODY="$(curl -s "${ADMIN_BASE_URL}/api/remote-invoke/ssh-key")"
assert_equals "null" "$NULL_BODY" "撤销后当前 active key 应为空"

GRANTS_AFTER_REVOKE_JSON="$TMPDIR/grants_after_revoke.json"
curl -s "${ADMIN_BASE_URL}/api/remote-invoke/grants" >"$GRANTS_AFTER_REVOKE_JSON"
assert_python "$GRANTS_AFTER_REVOKE_JSON" 'assert not any(g.get("auth_method") == "ssh_publickey" for g in obj.get("grants", []))'

for _ in $(seq 1 20); do
    FAIL_CODE=$(curl -s -o "$TMPDIR/challenge_fail.json" -w '%{http_code}' \
        -X POST "${RELAY_URL}/v4/remote-invoke/ssh/challenge" \
        -H "Content-Type: application/json" \
        -d "{\"device_code\":\"${DEVICE_CODE}\"}" || true)
    if [[ "$FAIL_CODE" != "200" ]]; then
        break
    fi
    sleep 1
done
if [[ "$FAIL_CODE" == "200" ]]; then
    echo "old device_code still accepted after revoke" >&2
    exit 1
fi
assert_python "$TMPDIR/challenge_fail.json" 'assert "device_code" in obj["message"]'

log "All SSH remote invoke E2E checks passed"
