#!/bin/bash
set -euo pipefail

unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY all_proxy ALL_PROXY no_proxy NO_PROXY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
SYNC_SERVER_DIR="$REPO_DIR/packages/bifrost-sync-server"
BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/release/bifrost}"

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
TARGET_HOME_FILE="${HOME}/.bifrost-remote-invoke-ssh-e2e-${RANDOM}-${RANDOM}.txt"

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
    pkill -f "bifrost __tray --data-dir ${BIFROST_DATA_DIR}" >/dev/null 2>&1 || true
    rm -rf "$RELAY_DATA_DIR" "$TMPDIR" "$BIFROST_DATA_DIR" "${CALLER_DATA_DIR:-}" "${CALLER_DATA_DIR_2:-}" "$MOCK_DIR" >/dev/null 2>&1 || true
    rm -f "$TARGET_HOME_FILE" "$RELAY_LOG" "$MOCK_LOG" >/dev/null 2>&1 || true
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

run_remote_exec_target_bifrost_cli_checks() {
    local status_output local_search_output local_batch_get_output auth_status_output export_output replay_output replay_help_output
    local capture_output capture_stderr capture_exit

    log "Execute target-local bifrost status through remote exec"
    status_output="$TMPDIR/target_status_via_remote_exec.json"
    BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote exec --relay-url "$RELAY_URL" --client-id "${CLIENT_INSTANCE_ID:0:12}" \
        -- "$BIFROST_BIN" --port "$ADMIN_PORT" status --format json >"$status_output"
    assert_python "$status_output" 'assert obj["version"]; assert obj["running"] is True; assert obj["listener"]["port"] == int("'"$ADMIN_PORT"'")'

    log "Execute target-local search --include through remote exec"
    local_search_output="$TMPDIR/target_search_include_via_remote_exec.ndjson"
    BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote exec --relay-url "$RELAY_URL" --client-id "${CLIENT_INSTANCE_ID:0:12}" \
        -- "$BIFROST_BIN" --port "$ADMIN_PORT" search "$MARKER" --include bodies,headers --max-body 32768 --format ndjson \
        >"$local_search_output"
    grep -q "$MARKER" "$local_search_output"
    python3 - "$local_search_output" "$MARKER" <<'PY'
import json
import sys

marker = sys.argv[2]
seen = False
for line in open(sys.argv[1], encoding="utf-8"):
    line = line.strip()
    if not line or not line.startswith("{"):
        continue
    obj = json.loads(line)
    if marker in json.dumps(obj, ensure_ascii=False):
        seen = True
        break
assert seen, "target-local search --include should return the generated marker"
PY

    log "Execute target-local traffic get --ids through remote exec"
    local_batch_get_output="$TMPDIR/target_traffic_get_ids_via_remote_exec.ndjson"
    BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote exec --relay-url "$RELAY_URL" --client-id "${CLIENT_INSTANCE_ID:0:12}" \
        -- "$BIFROST_BIN" --port "$ADMIN_PORT" traffic get --ids "$TRAFFIC_ID" --max-body 32768 --format ndjson \
        >"$local_batch_get_output"
    grep -q "$MARKER" "$local_batch_get_output"
    python3 - "$local_batch_get_output" "$TRAFFIC_ID" "$MARKER" <<'PY'
import json
import sys

traffic_id, marker = sys.argv[2:4]
seen = False
for line in open(sys.argv[1], encoding="utf-8"):
    line = line.strip()
    if not line or not line.startswith("{"):
        continue
    obj = json.loads(line)
    payload = json.dumps(obj, ensure_ascii=False)
    if traffic_id in payload and marker in payload:
        seen = True
        break
assert seen, "traffic get --ids should return the exact target record and response marker"
PY

    log "Execute target-local traffic auth-status through remote exec"
    auth_status_output="$TMPDIR/target_auth_status_via_remote_exec.json"
    BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote exec --relay-url "$RELAY_URL" --client-id "${CLIENT_INSTANCE_ID:0:12}" \
        -- "$BIFROST_BIN" --port "$ADMIN_PORT" traffic auth-status "$TRAFFIC_ID" --format json >"$auth_status_output"
    assert_python "$auth_status_output" 'assert "has_jwt" in obj; assert "has_cookie" in obj'

    log "Execute target-local traffic export --as curl through remote exec"
    export_output="$TMPDIR/target_export_curl_via_remote_exec.txt"
    BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote exec --relay-url "$RELAY_URL" --client-id "${CLIENT_INSTANCE_ID:0:12}" \
        -- "$BIFROST_BIN" --port "$ADMIN_PORT" traffic export "$TRAFFIC_ID" --as curl >"$export_output"
    grep -q "curl" "$export_output"
    grep -q "$MARKER" "$export_output"

    log "Execute target-local traffic replay --refresh-auth through remote exec"
    replay_output="$TMPDIR/target_replay_via_remote_exec.json"
    BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote exec --relay-url "$RELAY_URL" --client-id "${CLIENT_INSTANCE_ID:0:12}" \
        -- "$BIFROST_BIN" --port "$ADMIN_PORT" traffic replay "$TRAFFIC_ID" --refresh-auth --format json >"$replay_output"
    assert_python "$replay_output" 'assert obj["success"] is True; assert obj["data"]["response"]["status"] == 200'

    log "Verify target-local traffic replay help exposes --patch through remote exec"
    replay_help_output="$TMPDIR/target_replay_help_via_remote_exec.txt"
    BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote exec --relay-url "$RELAY_URL" --client-id "${CLIENT_INSTANCE_ID:0:12}" \
        -- "$BIFROST_BIN" traffic replay --help >"$replay_help_output"
    grep -q -- "--patch <PATCH>" "$replay_help_output"

    log "Execute target-local capture wait timeout through remote exec"
    capture_output="$TMPDIR/target_capture_wait_via_remote_exec.json"
    capture_stderr="$TMPDIR/target_capture_wait_via_remote_exec.err"
    set +e
    BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote exec --relay-url "$RELAY_URL" --client-id "${CLIENT_INSTANCE_ID:0:12}" \
        --timeout-ms 6000 \
        -- "$BIFROST_BIN" --port "$ADMIN_PORT" capture wait --host skill-remote-never.invalid --timeout 1s --format json \
        >"$capture_output" 2>"$capture_stderr"
    capture_exit=$?
    set -e
    if [[ "$capture_exit" -eq 124 ]]; then
        _log_pass "target-local capture wait should preserve timeout exit code 124 through remote exec"
    else
        _log_fail "target-local capture wait should preserve timeout exit code 124 through remote exec" \
            "124" "exit=${capture_exit} stdout=$(cat "$capture_output") stderr=$(cat "$capture_stderr")"
        return 1
    fi
}

dump_grant_diagnostics() {
    echo "[remote-invoke-ssh-e2e] Grant diagnostics:" >&2
    if [[ -n "${CALLER_CONNECTIONS_JSON:-}" && -f "$CALLER_CONNECTIONS_JSON" ]]; then
        echo "[remote-invoke-ssh-e2e] caller remote-connections.json:" >&2
        cat "$CALLER_CONNECTIONS_JSON" >&2
    fi
    if [[ -n "${CALLER_CONNECTIONS_JSON_2:-}" && -f "$CALLER_CONNECTIONS_JSON_2" ]]; then
        echo "[remote-invoke-ssh-e2e] caller-2 remote-connections.json:" >&2
        cat "$CALLER_CONNECTIONS_JSON_2" >&2
    fi
    if [[ -n "${ADMIN_BASE_URL:-}" ]]; then
        echo "[remote-invoke-ssh-e2e] target grants:" >&2
        curl -s "${ADMIN_BASE_URL}/api/remote-invoke/grants" >&2 || true
        echo >&2
        echo "[remote-invoke-ssh-e2e] target calls:" >&2
        curl -s "${ADMIN_BASE_URL}/api/remote-invoke/calls" >&2 || true
        echo >&2
    fi
    if [[ -n "${BIFROST_DATA_DIR:-}" ]]; then
        for store in \
            "$BIFROST_DATA_DIR/admin/remote_invoke_grant_info.json" \
            "$BIFROST_DATA_DIR/admin/remote_invoke_grant_crypto.json"; do
            if [[ -f "$store" ]]; then
                echo "[remote-invoke-ssh-e2e] target store $(basename "$store"):" >&2
                cat "$store" >&2 || true
                echo >&2
            fi
        done
    fi
    if [[ -n "${ADMIN_CLIENT_BIFROST_LOG_FILE:-}" && -f "$ADMIN_CLIENT_BIFROST_LOG_FILE" ]]; then
        echo "[remote-invoke-ssh-e2e] Bifrost log tail:" >&2
        tail -n 180 "$ADMIN_CLIENT_BIFROST_LOG_FILE" >&2 || true
    fi
}

expect_remote_exec_success() {
    local marker="$1"
    local output_file="$TMPDIR/remote_exec_${marker}.out"
    local status=1
    local attempt
    local max_attempts="${BIFROST_E2E_GRANT_PROPAGATION_ATTEMPTS:-20}"
    log "  expect remote exec success: ${marker}"

    for attempt in $(seq 1 "$max_attempts"); do
        set +e
        BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote exec --relay-url "$RELAY_URL" --shell-text "printf ${marker}" \
            >"$output_file" 2>&1
        status=$?
        set -e

        if [[ "$status" -eq 0 ]] && grep -q "$marker" "$output_file"; then
            return 0
        fi

        if grep -Eiq "grant scope .*does not allow command kind ShellExec|grant_scope_mismatch" "$output_file"; then
            if [[ "$attempt" -lt "$max_attempts" ]]; then
                log "  grant scope update not visible to remote exec yet (${attempt}/${max_attempts}); retrying ${marker}"
                sleep 0.5
                continue
            fi
        fi

        break
    done

    if [[ "$status" -ne 0 ]] || ! grep -q "$marker" "$output_file"; then
        echo "remote exec success assertion failed for ${marker} (status=${status})" >&2
        cat "$output_file" >&2
        dump_grant_diagnostics
        exit 1
    fi
}

expect_remote_exec_denied() {
    local label="$1"
    local output_file="$TMPDIR/remote_exec_denied_${label}.out"
    set +e
    BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote exec --relay-url "$RELAY_URL" --shell-text "printf ${label}" \
        >"$output_file" 2>&1
    local status=$?
    set -e
    if [[ "$status" -eq 0 ]]; then
        echo "remote exec unexpectedly succeeded for ${label}" >&2
        cat "$output_file" >&2
        exit 1
    fi
    grep -Eiq "grant_scope_mismatch|forbidden|denied|not.*allow|scope" "$output_file"
}

expect_remote_file_rw_success() {
    local marker="$1"
    local write_output="$TMPDIR/file_write_${marker}.out"
    local read_output="$TMPDIR/file_read_${marker}.out"
    local marker_b64
    marker_b64="$(printf '%s' "$marker" | base64)"
    log "  expect remote file read/write success: ${marker}"
    set +e
    BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" file write "$TARGET_HOME_FILE" \
        --content "$marker" --allow-overwrite true --create-parents --output json \
        >"$write_output" 2>&1
    local write_status=$?
    set -e
    if [[ "$write_status" -ne 0 ]]; then
        echo "remote file write success assertion failed for ${marker} (status=${write_status})" >&2
        cat "$write_output" >&2
        dump_grant_diagnostics
        exit 1
    fi
    set +e
    BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" file read "$TARGET_HOME_FILE" --output json \
        >"$read_output" 2>&1
    local read_status=$?
    set -e
    if [[ "$read_status" -ne 0 ]] || ! grep -q "\"content_b64\":\"${marker_b64}\"" "$read_output"; then
        echo "remote file read success assertion failed for ${marker} (status=${read_status})" >&2
        cat "$read_output" >&2
        dump_grant_diagnostics
        exit 1
    fi
}

expect_remote_file_write_denied() {
    local marker="$1"
    local output_file="$TMPDIR/file_write_denied_${marker}.out"
    set +e
    BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" file write "$TARGET_HOME_FILE" \
        --content "$marker" --allow-overwrite true --create-parents --output json \
        >"$output_file" 2>&1
    local status=$?
    set -e
    if [[ "$status" -eq 0 ]]; then
        echo "remote file write unexpectedly succeeded for ${marker}" >&2
        cat "$output_file" >&2
        exit 1
    fi
    grep -Eiq "file_access|forbidden|denied|not.*allow|scope" "$output_file"
}

update_grant_level_and_assert() {
    local level="$1"
    local expected_scope="$2"
    local expected_file_access="$3"
    local output_file="$TMPDIR/cli_grant_${level}.out"
    "$BIFROST_BIN" --port "$ADMIN_PORT" setting grant update --device "$CALLER_FINGERPRINT_1" --level "$level" \
        >"$output_file" 2>&1
    python3 - "$output_file" "$MATCH_GRANT" "$expected_scope" "$expected_file_access" "$level" <<'PY'
import json
import sys
obj = json.load(open(sys.argv[1]))
grant_id, expected_scope, expected_file_access, level = sys.argv[2:6]
data = obj.get("data", obj)
assert data.get("grant_id") == grant_id
assert data.get("grant_scope") == expected_scope
assert data.get("file_access") == expected_file_access
if level in ("full", "full-trust"):
    assert data.get("interactive_allowed") is True
    assert data.get("stdin_allowed") is True
    binding = data.get("policy_binding") or {}
    assert binding.get("mode") == "selected"
    assert "ssh-key-full-access" in binding.get("policy_ids", [])
elif level in ("shell", "commands-files", "commands-and-files"):
    assert data.get("interactive_allowed") is False
    assert data.get("stdin_allowed") is False
    binding = data.get("policy_binding") or {}
    assert binding.get("mode") == "selected"
    assert "ssh-key-full-access" in binding.get("policy_ids", [])
else:
    assert data.get("interactive_allowed") is None
    assert data.get("stdin_allowed") is None
    assert data.get("policy_binding") in (None, {})
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

if [[ "$BIFROST_BIN" == "$REPO_DIR/target/release/bifrost" && "${SKIP_BUILD:-}" != "true" ]]; then
    NEED_BUILD=0
    if [[ ! -x "$BIFROST_BIN" ]] \
        || [[ "$REPO_DIR/Cargo.toml" -nt "$BIFROST_BIN" ]] \
        || [[ "$REPO_DIR/Cargo.lock" -nt "$BIFROST_BIN" ]]; then
        NEED_BUILD=1
    elif find "$REPO_DIR/crates" -type f \( -name '*.rs' -o -name 'Cargo.toml' \) -newer "$BIFROST_BIN" -print -quit | grep -q .; then
        NEED_BUILD=1
    fi

    if [[ "$NEED_BUILD" -eq 1 ]]; then
        log "Build bifrost (release)..."
        (cd "$REPO_DIR" && cargo build --release --bin bifrost >/dev/null 2>&1)
    fi
fi
if [[ ! -x "$BIFROST_BIN" ]]; then
    echo "bifrost binary not found at $BIFROST_BIN" >&2
    exit 1
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
python3 - "$MOCK_DIR" "$MOCK_PORT" <<'PY' >"$MOCK_LOG" 2>&1 &
import json
import sys
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer

directory = sys.argv[1]
port = int(sys.argv[2])

class Handler(SimpleHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("Content-Length") or "0")
        raw = self.rfile.read(length)
        try:
            body = json.loads(raw.decode("utf-8"))
        except Exception:
            body = raw.decode("utf-8", "replace")
        payload = json.dumps({"path": self.path, "received": body}, ensure_ascii=False).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, fmt, *args):
        return

handler = partial(Handler, directory=directory)
ThreadingHTTPServer(("127.0.0.1", port), handler).serve_forever()
PY
MOCK_PID=$!

log "Create SSH key through CLI against the running Bifrost service"
CLI_KEY_PATH="$TMPDIR/cli-created.bifrost"
CLI_CREATE_KEY_OUTPUT="$TMPDIR/cli_create_key.out"
"$BIFROST_BIN" --port "$ADMIN_PORT" setting ssh-key create \
    --label "CI Agent" --grant-mode permanent --output "$CLI_KEY_PATH" \
    >"$CLI_CREATE_KEY_OUTPUT" 2>&1
grep -q "Remote-invoke SSH key created" "$CLI_CREATE_KEY_OUTPUT"
grep -q "Use on caller: bifrost remote conn up --ssh-key $CLI_KEY_PATH" "$CLI_CREATE_KEY_OUTPUT"
test -f "$CLI_KEY_PATH"
grep -q "BEGIN BIFROST KEY" "$CLI_KEY_PATH"
grep -q "Device-Code: BF-" "$CLI_KEY_PATH"
KEY_FILE="$(cat "$CLI_KEY_PATH")"

log "Verify CLI-created SSH key metadata through Admin API"
CREATE_JSON="$TMPDIR/create.json"
curl -s "${ADMIN_BASE_URL}/api/remote-invoke/ssh-key" >"$CREATE_JSON"
assert_python "$CREATE_JSON" 'assert re.match(r"^BF-[0-9A-F]{16}$", obj["device_code"])'
assert_python "$CREATE_JSON" 'assert obj["grant_mode"] == "permanent"'
DEVICE_CODE="$(json_get "$CREATE_JSON" "device_code")"
FINGERPRINT="$(json_get "$CREATE_JSON" "ssh_key_fingerprint")"
assert_not_empty "$FINGERPRINT" "ssh key fingerprint 不应为空"

log "Wait for relay route sync after CLI ssh-key create"
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
BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote conn up --ssh-key "$TMPDIR/cli-test.bifrost" --relay-url "$RELAY_URL" \
    >"$CLI_CONNECT_OUTPUT" 2>&1
grep -q "Connected with SSH key" "$CLI_CONNECT_OUTPUT"
CALLER_CONNECTIONS_JSON="$CALLER_DATA_DIR/remote-connections.json"
assert_python "$CALLER_CONNECTIONS_JSON" '
import re
assert obj["connections"], "caller 应写入 remote-connections.json"
conn = obj["connections"][0]
assert conn["client_instance_id"] == "'"$CLIENT_INSTANCE_ID"'"
assert conn["grant_mode"] == "permanent"
assert re.fullmatch(r"[0-9a-f]{64}", conn["caller_fingerprint"])
assert conn["caller_fingerprint"] != "'"$FINGERPRINT"'"
assert conn["device_code"] == "'"$DEVICE_CODE"'"
assert conn["auth_method"] == "ssh_publickey"
assert conn.get("grant_session_token")
assert conn.get("shared_secret_encrypted")
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
        assert grant.get("grant_scope") == "remote_shell_interactive"
        assert grant.get("file_access") == "read_write"
        assert grant.get("interactive_allowed") is True
        assert grant.get("stdin_allowed") is True
        break
else:
    raise AssertionError("grant not found")
'

log "Verify SSH grant is visible on target Admin grants"
GRANTS_AFTER_CONNECT_JSON="$TMPDIR/grants_after_connect.json"
curl -s "${ADMIN_BASE_URL}/api/remote-invoke/grants" >"$GRANTS_AFTER_CONNECT_JSON"
python3 - "$GRANTS_AFTER_CONNECT_JSON" "$MATCH_GRANT" <<'PY'
import json
import sys
obj = json.load(open(sys.argv[1]))
data = next((g for g in obj.get("grants", []) if g.get("grant_id") == sys.argv[2]), {})
assert data.get("grant_id") == sys.argv[2]
assert data.get("grant_scope") == "remote_shell_interactive"
assert data.get("file_access") == "read_write"
PY

log "Verify default SSH-key Full Trust can run commands and read/write files"
expect_remote_file_rw_success "ssh-full-trust-file-ok"
expect_remote_exec_success "ssh-full-trust-ok"

log "Switch SSH grant to Run commands & read/write files and verify capabilities"
update_grant_level_and_assert "shell" "remote_shell_exec" "read_write"
expect_remote_exec_success "ssh-shell-ok"
expect_remote_file_rw_success "ssh-shell-file-ok"

log "Switch SSH grant to Files only and verify command denial plus file access"
update_grant_level_and_assert "files" "remote_query" "read_write"
expect_remote_exec_denied "ssh-files-deny-exec"
expect_remote_file_rw_success "ssh-files-file-ok"

log "Switch SSH grant to Read-only watch and verify command/file denial plus status access"
update_grant_level_and_assert "query" "remote_query" "none"
CLI_QUERY_STATUS_OUTPUT="$TMPDIR/cli_query_status.out"
BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote conn status --relay-url "$RELAY_URL" \
    >"$CLI_QUERY_STATUS_OUTPUT" 2>&1
python3 - "$CLI_QUERY_STATUS_OUTPUT" <<'PY'
import json
import sys
obj = json.load(open(sys.argv[1]))
assert obj["version"]
assert obj["device_name"]
assert obj["os"]
PY
expect_remote_exec_denied "ssh-query-deny-exec"
expect_remote_file_write_denied "ssh-query-deny-file"

log "Switch SSH grant back to Full Trust and verify command access is restored"
update_grant_level_and_assert "full" "remote_shell_interactive" "read_write"
expect_remote_exec_success "ssh-full-restored-ok"

log "Use same SSH key from another caller sandbox and verify caller identity isolation"
CALLER_DATA_DIR_2="$(mktemp -d)"
CLI_CONNECT_OUTPUT_2="$TMPDIR/cli_connect_2.out"
BIFROST_REMOTE_SSH_KEY="$KEY_FILE" \
BIFROST_DATA_DIR="$CALLER_DATA_DIR_2" "$BIFROST_BIN" remote conn up --ssh-key --relay-url "$RELAY_URL" \
    >"$CLI_CONNECT_OUTPUT_2" 2>&1
grep -q "Connected with SSH key" "$CLI_CONNECT_OUTPUT_2"
CALLER_CONNECTIONS_JSON_2="$CALLER_DATA_DIR_2/remote-connections.json"
assert_python "$CALLER_CONNECTIONS_JSON_2" '
conn = obj["connections"][0]
assert conn["ssh_key_source"] == "env:BIFROST_REMOTE_SSH_KEY"
'
CALLER_FINGERPRINT_2="$(jq -r '.connections[0].caller_fingerprint' "$CALLER_CONNECTIONS_JSON_2")"
assert_not_empty "$CALLER_FINGERPRINT_2" "第二个 caller fingerprint 不应为空"
if [[ "$CALLER_FINGERPRINT_1" == "$CALLER_FINGERPRINT_2" ]]; then
    echo "two caller sandboxes reused the same caller_fingerprint" >&2
    exit 1
fi

for _ in $(seq 1 60); do
    GRANTS_JSON="$TMPDIR/grants_two_callers.json"
    curl -s "${ADMIN_BASE_URL}/api/remote-invoke/grants" >"$GRANTS_JSON"
    if python3 - "$GRANTS_JSON" "$FINGERPRINT" "$CALLER_FINGERPRINT_1" "$CALLER_FINGERPRINT_2" 2>/dev/null <<'PY'
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
if ! python3 - "$GRANTS_JSON" "$FINGERPRINT" "$CALLER_FINGERPRINT_1" "$CALLER_FINGERPRINT_2" <<'PY'
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
    echo "expected two ssh_publickey grants for the same SSH key and distinct caller fingerprints" >&2
    echo "caller_a=${CALLER_FINGERPRINT_1}" >&2
    echo "caller_b=${CALLER_FINGERPRINT_2}" >&2
    cat "$GRANTS_JSON" >&2
    dump_grant_diagnostics
    exit 1
fi

KEY_AFTER_USE_JSON="$TMPDIR/key_after_use.json"
curl -s "${ADMIN_BASE_URL}/api/remote-invoke/ssh-key" >"$KEY_AFTER_USE_JSON"
assert_python "$KEY_AFTER_USE_JSON" 'assert obj["last_used_at"] is not None'

log "Execute remote conn status via saved SSH connection"
CLI_STATUS_OUTPUT="$TMPDIR/cli_status.out"
BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote conn status --relay-url "$RELAY_URL" \
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

log "Verify SSH grant remains visible on target before traffic.get"
GRANTS_BEFORE_TRAFFIC_GET_JSON="$TMPDIR/grants_before_traffic_get.json"
curl -s "${ADMIN_BASE_URL}/api/remote-invoke/grants" >"$GRANTS_BEFORE_TRAFFIC_GET_JSON"
assert_python "$GRANTS_BEFORE_TRAFFIC_GET_JSON" '
grant = next((g for g in obj.get("grants", []) if g.get("grant_id") == "'"$MATCH_GRANT"'"), None)
assert grant is not None
assert grant["grant_mode"] == "permanent"
assert grant.get("expires_at") in (None, "")
assert grant["caller_fingerprint"] == "'"$CALLER_FINGERPRINT_1"'"
assert grant["ssh_key_fingerprint"] == "'"$FINGERPRINT"'"
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
BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote traffic search "$MARKER" \
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
BIFROST_DATA_DIR="$CALLER_DATA_DIR" "$BIFROST_BIN" remote traffic get "$TRAFFIC_ID" \
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

run_remote_exec_target_bifrost_cli_checks

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
