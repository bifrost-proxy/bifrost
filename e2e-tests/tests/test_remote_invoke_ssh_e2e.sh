#!/bin/bash
set -euo pipefail

unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY all_proxy ALL_PROXY no_proxy NO_PROXY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
SYNC_SERVER_DIR="$REPO_DIR/packages/bifrost-sync-server"

source "$SCRIPT_DIR/../test_utils/assert.sh"
source "$SCRIPT_DIR/../test_utils/admin_client.sh"
source "$SCRIPT_DIR/../test_utils/sync_server.sh"

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
    rm -rf "$RELAY_DATA_DIR" "$TMPDIR" "$BIFROST_DATA_DIR" "${CALLER_DATA_DIR:-}" "${CALLER_DATA_DIR_2:-}" "$MOCK_DIR" >/dev/null 2>&1 || true
    rm -f "$RELAY_LOG" "$MOCK_LOG" >/dev/null 2>&1 || true
}
trap cleanup EXIT

log() { echo "[remote-invoke-ssh-e2e] $*"; }

start_local_relay() {
    log "Starting local bifrost-sync-server on port $RELAY_PORT..."
    local relay_exec
    relay_exec="$(sync_server_exec "$SYNC_SERVER_DIR")"
    (
        cd "$SYNC_SERVER_DIR" && \
            eval "$relay_exec" -p "$RELAY_PORT" -d "$RELAY_DATA_DIR" --enable-remote-invoke
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

dump_reconnect_diagnostics() {
    local status_json="$1"
    echo "[remote-invoke-ssh-e2e] Worker did not reach Connected in time" >&2
    echo "[remote-invoke-ssh-e2e] Last worker status:" >&2
    cat "$status_json" >&2 2>/dev/null || true
    echo >&2
    echo "[remote-invoke-ssh-e2e] Relay log tail:" >&2
    tail -n 120 "$RELAY_LOG" >&2 2>/dev/null || true
    if [[ -n "${ADMIN_CLIENT_BIFROST_LOG_FILE:-}" && -f "$ADMIN_CLIENT_BIFROST_LOG_FILE" ]]; then
        echo "[remote-invoke-ssh-e2e] Bifrost log tail:" >&2
        tail -n 160 "$ADMIN_CLIENT_BIFROST_LOG_FILE" >&2 2>/dev/null || true
    fi
}

wait_for_worker_connected() {
    local status_json="$1"
    local timeout_secs="${2:-60}"
    local interval_secs=1
    local attempts=$((timeout_secs / interval_secs))
    if [[ "$attempts" -lt 1 ]]; then
        attempts=1
    fi

    for _ in $(seq 1 "$attempts"); do
        curl -s "${ADMIN_BASE_URL}/api/remote-invoke/status" >"$status_json"
        if [[ "$(json_get "$status_json" "state")" == "Connected" ]]; then
            return 0
        fi
        sleep "$interval_secs"
    done

    dump_reconnect_diagnostics "$status_json"
    return 1
}

start_local_relay

if [[ "${SKIP_BUILD:-}" != "true" ]]; then
    NEED_BUILD=0
    if [[ ! -x "$REPO_DIR/target/release/bifrost" ]] \
        || [[ "$REPO_DIR/Cargo.toml" -nt "$REPO_DIR/target/release/bifrost" ]] \
        || [[ "$REPO_DIR/Cargo.lock" -nt "$REPO_DIR/target/release/bifrost" ]]; then
        NEED_BUILD=1
    elif find "$REPO_DIR/crates" -type f \( -name '*.rs' -o -name 'Cargo.toml' \) -newer "$REPO_DIR/target/release/bifrost" -print -quit | grep -q .; then
        NEED_BUILD=1
    fi

    if [[ "$NEED_BUILD" -eq 1 ]]; then
        log "Build bifrost (release)..."
        (cd "$REPO_DIR" && cargo build --release --bin bifrost >/dev/null 2>&1)
    fi
fi

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
wait_for_worker_connected "$STATUS_JSON" "${BIFROST_E2E_REMOTE_INVOKE_RESET_RECONNECT_TIMEOUT:-180}"
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

log "Use CLI remote conn up --ssh-key"
CALLER_DATA_DIR="$(mktemp -d)"
CLI_CONNECT_OUTPUT="$TMPDIR/cli_connect.out"
printf '%s' "$KEY_FILE" >"$TMPDIR/cli-test.bifrost"
BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$REPO_DIR/target/release/bifrost" remote conn up --ssh-key "$TMPDIR/cli-test.bifrost" --relay-url "$RELAY_URL" \
    >"$CLI_CONNECT_OUTPUT" 2>&1
grep -q "Connected with SSH key" "$CLI_CONNECT_OUTPUT"
CALLER_CONNECTIONS_JSON="$CALLER_DATA_DIR/remote-connections.json"
assert_python "$CALLER_CONNECTIONS_JSON" '
assert obj["connections"], "caller 应写入 remote-connections.json"
conn = obj["connections"][0]
assert conn["client_instance_id"] == "'"$CLIENT_INSTANCE_ID"'"
assert conn["grant_mode"] == "permanent"
assert conn["caller_fingerprint"].startswith("caller-")
assert conn["caller_fingerprint"] != "'"$FINGERPRINT"'"
assert conn["device_code"] == "'"$DEVICE_CODE"'"
assert conn["auth_method"] == "ssh_publickey"
'
CALLER_FINGERPRINT_1="$(jq -r '.connections[0].caller_fingerprint' "$CALLER_CONNECTIONS_JSON")"

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
        assert grant.get("caller_fingerprint") == "'"$CALLER_FINGERPRINT_1"'"
        assert grant.get("ssh_key_fingerprint") == "'"$FINGERPRINT"'"
        break
else:
    raise AssertionError("grant not found")
'

log "Use same SSH key from another caller sandbox and verify caller identity isolation"
CALLER_DATA_DIR_2="$(mktemp -d)"
CLI_CONNECT_OUTPUT_2="$TMPDIR/cli_connect_2.out"
BIFROST_DATA_DIR="$CALLER_DATA_DIR_2" "$REPO_DIR/target/release/bifrost" remote conn up --ssh-key "$TMPDIR/cli-test.bifrost" --relay-url "$RELAY_URL" \
    >"$CLI_CONNECT_OUTPUT_2" 2>&1
grep -q "Connected with SSH key" "$CLI_CONNECT_OUTPUT_2"
CALLER_CONNECTIONS_JSON_2="$CALLER_DATA_DIR_2/remote-connections.json"
CALLER_FINGERPRINT_2="$(jq -r '.connections[0].caller_fingerprint' "$CALLER_CONNECTIONS_JSON_2")"
assert_not_empty "$CALLER_FINGERPRINT_2" "第二个 caller fingerprint 不应为空"
if [[ "$CALLER_FINGERPRINT_1" == "$CALLER_FINGERPRINT_2" ]]; then
    echo "two caller sandboxes reused the same caller_fingerprint" >&2
    exit 1
fi

for _ in $(seq 1 60); do
    GRANTS_JSON="$TMPDIR/grants_two_callers.json"
    curl -s "${ADMIN_BASE_URL}/api/remote-invoke/grants" >"$GRANTS_JSON"
    if python3 - "$GRANTS_JSON" "$FINGERPRINT" "$CALLER_FINGERPRINT_1" "$CALLER_FINGERPRINT_2" <<'PY'
import json
import sys
obj = json.load(open(sys.argv[1]))
ssh_fp, caller_a, caller_b = sys.argv[2:5]
matches = [
    grant for grant in obj.get("grants", [])
    if grant.get("auth_method") == "ssh_publickey"
    and grant.get("ssh_key_fingerprint") == ssh_fp
    and grant.get("caller_fingerprint") in {caller_a, caller_b}
]
assert {g.get("caller_fingerprint") for g in matches} == {caller_a, caller_b}
PY
    then
        break
    fi
    sleep 0.5
done
python3 - "$GRANTS_JSON" "$FINGERPRINT" "$CALLER_FINGERPRINT_1" "$CALLER_FINGERPRINT_2" <<'PY'
import json
import sys
obj = json.load(open(sys.argv[1]))
ssh_fp, caller_a, caller_b = sys.argv[2:5]
matches = [
    grant for grant in obj.get("grants", [])
    if grant.get("auth_method") == "ssh_publickey"
    and grant.get("ssh_key_fingerprint") == ssh_fp
    and grant.get("caller_fingerprint") in {caller_a, caller_b}
]
assert {g.get("caller_fingerprint") for g in matches} == {caller_a, caller_b}
PY

KEY_AFTER_USE_JSON="$TMPDIR/key_after_use.json"
curl -s "${ADMIN_BASE_URL}/api/remote-invoke/ssh-key" >"$KEY_AFTER_USE_JSON"
assert_python "$KEY_AFTER_USE_JSON" 'assert obj["last_used_at"] is not None'

log "Execute remote conn status via saved SSH connection"
CLI_STATUS_OUTPUT="$TMPDIR/cli_status.out"
BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$REPO_DIR/target/release/bifrost" remote conn status --relay-url "$RELAY_URL" \
    >"$CLI_STATUS_OUTPUT" 2>&1
python3 - "$CLI_STATUS_OUTPUT" "$CLIENT_INSTANCE_ID" <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as fh:
    content = fh.read().strip()
obj = json.loads(content)
assert obj["version"]
assert obj["device_name"]
assert obj["os"]
assert obj["arch"]
assert isinstance(obj["cpu_logical_cores"], int)
assert obj["cpu_logical_cores"] > 0
assert isinstance(obj["memory_total_bytes"], int)
assert obj["memory_total_bytes"] > 0
assert isinstance(obj["memory_available_bytes"], int)
assert obj["memory_available_bytes"] <= obj["memory_total_bytes"]
assert isinstance(obj["storage_total_bytes"], int)
assert obj["storage_total_bytes"] > 0
assert isinstance(obj["storage_available_bytes"], int)
assert obj["storage_available_bytes"] <= obj["storage_total_bytes"]
assert isinstance(obj["storage_mount_point"], str)
assert obj["storage_mount_point"]
assert "rust_version" not in obj
assert isinstance(obj["uptime_secs"], int)
PY

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
curl -s "${RELAY_URL}/v4/remote-invoke/grants/reusable?client_instance_id=${CLIENT_INSTANCE_ID}&caller_fingerprint=${CALLER_FINGERPRINT_1}" >"$REUSABLE_JSON"
assert_python "$REUSABLE_JSON" '
assert obj["data"]["grant_id"] == "'"$MATCH_GRANT"'"
assert obj["data"]["grant_mode"] == "permanent"
assert obj["data"]["expires_at"] in (None, "")
assert obj["data"]["caller_fingerprint"] == "'"$CALLER_FINGERPRINT_1"'"
assert obj["data"]["ssh_key_fingerprint"] == "'"$FINGERPRINT"'"
'

log "Execute search.get via SSH grant"
SEARCH_CALLS_BEFORE_JSON="$TMPDIR/search_calls_before.json"
curl -s "${ADMIN_BASE_URL}/api/remote-invoke/calls" >"$SEARCH_CALLS_BEFORE_JSON"
SEARCH_PRE_STARTED_AT="$(python3 - "$SEARCH_CALLS_BEFORE_JSON" <<'PY'
import json
import sys

obj = json.load(open(sys.argv[1]))
candidates = [
    call for call in obj.get("calls", [])
    if ((call.get("command") or {}).get("command") or call.get("command")) in ("search.get", "search.stream")
]
candidates.sort(key=lambda call: call.get("started_at") or 0, reverse=True)
print(candidates[0].get("started_at") or 0 if candidates else 0)
PY
)"
SEARCH_OUTPUT="$TMPDIR/search.out"
SEARCH_STDERR="$TMPDIR/search.err"
BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$REPO_DIR/target/release/bifrost" remote traffic search "$MARKER" \
    --relay-url "$RELAY_URL" --client-id "${CLIENT_INSTANCE_ID:0:12}" --limit 10 \
    >"$SEARCH_OUTPUT" 2>"$SEARCH_STDERR"
grep -q "$MARKER" "$SEARCH_OUTPUT"
SEARCH_CALLS_AFTER_JSON="$TMPDIR/search_calls_after.json"
curl -s "${ADMIN_BASE_URL}/api/remote-invoke/calls" >"$SEARCH_CALLS_AFTER_JSON"
assert_python "$SEARCH_CALLS_AFTER_JSON" '
calls = [
    call for call in obj.get("calls", [])
    if ((call.get("command") or {}).get("command") or call.get("command")) in ("search.get", "search.stream")
    and (call.get("started_at") or 0) > int("'"$SEARCH_PRE_STARTED_AT"'")
]
assert calls, "应记录新的 search 调用"
calls.sort(key=lambda call: call.get("started_at") or 0, reverse=True)
latest = calls[0]
assert str(latest.get("status", "")).lower() == "completed"
'

log "Execute traffic.get via SSH grant"
TRAFFIC_CALLS_BEFORE_JSON="$TMPDIR/traffic_calls_before.json"
curl -s "${ADMIN_BASE_URL}/api/remote-invoke/calls" >"$TRAFFIC_CALLS_BEFORE_JSON"
TRAFFIC_PRE_STARTED_AT="$(python3 - "$TRAFFIC_CALLS_BEFORE_JSON" <<'PY'
import json
import sys

obj = json.load(open(sys.argv[1]))
candidates = [
    call for call in obj.get("calls", [])
    if ((call.get("command") or {}).get("command") or call.get("command")) == "traffic.get"
]
candidates.sort(key=lambda call: call.get("started_at") or 0, reverse=True)
print(candidates[0].get("started_at") or 0 if candidates else 0)
PY
)"
TRAFFIC_OUTPUT="$TMPDIR/traffic.out"
TRAFFIC_STDERR="$TMPDIR/traffic.err"
# Parse stdout as JSON; keep stderr (warn/info logs) in a sidecar file so the
# JSON parser never sees stray log lines that would otherwise corrupt it.
BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$REPO_DIR/target/release/bifrost" remote traffic get "$TRAFFIC_ID" \
    --relay-url "$RELAY_URL" --client-id "${CLIENT_INSTANCE_ID:0:12}" --response-body \
    >"$TRAFFIC_OUTPUT" 2>"$TRAFFIC_STDERR"
if [[ ! -s "$TRAFFIC_OUTPUT" ]]; then
    echo "remote traffic get produced no stdout; stderr was:" >&2
    cat "$TRAFFIC_STDERR" >&2 || true
    exit 1
fi
python3 - "$TRAFFIC_OUTPUT" "$TRAFFIC_ID" "$MARKER" <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as fh:
    obj = json.load(fh)

assert obj["id"] == sys.argv[2]
payload = json.dumps(obj, ensure_ascii=False)
assert sys.argv[3] in payload
PY
TRAFFIC_CALLS_AFTER_JSON="$TMPDIR/traffic_calls_after.json"
curl -s "${ADMIN_BASE_URL}/api/remote-invoke/calls" >"$TRAFFIC_CALLS_AFTER_JSON"
assert_python "$TRAFFIC_CALLS_AFTER_JSON" '
calls = [
    call for call in obj.get("calls", [])
    if ((call.get("command") or {}).get("command") or call.get("command")) == "traffic.get"
    and (call.get("started_at") or 0) > int("'"$TRAFFIC_PRE_STARTED_AT"'")
]
assert calls, "应记录新的 traffic.get 调用"
calls.sort(key=lambda call: call.get("started_at") or 0, reverse=True)
latest = calls[0]
assert str(latest.get("status", "")).lower() == "completed"
'

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
