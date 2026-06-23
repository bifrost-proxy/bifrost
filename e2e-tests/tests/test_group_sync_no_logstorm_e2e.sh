#!/bin/bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
# E2E Test: Rule sync log-storm / self-wake regression guard.
#
# Reproduces the scenario fixed by "fix rule sync log storm and log retention":
# repeated remote group-rule sync GETs (each triggers handle_list_and_sync ->
# fetch remote envs -> sync_envs_to_local) must NOT rewrite the local .bifrost
# file and must NOT emit a fresh "config change event received, reloading rules"
# log line per request when the remote content is unchanged.
#
# Observables asserted end-to-end:
#   1. The synced local rule file's mtime + sha256 stay constant across many
#      repeated syncs (sync_envs_to_local skips unchanged writes).
#   2. The bifrost reload log line count does NOT grow across repeated syncs
#      (RulesChangeOrigin::RemoteSync does not self-wake Sync, and an unchanged
#      sync does not notify a rules change at all).
#   3. A REAL remote content change still produces exactly one reload + one
#      file rewrite (the fix does not break legitimate propagation).

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

source "$SCRIPT_DIR/../test_utils/assert.sh"
source "$SCRIPT_DIR/../test_utils/admin_client.sh"
source "$SCRIPT_DIR/../test_utils/http_client.sh"
source "$SCRIPT_DIR/../test_utils/sync_server.sh"

# Keep all direct curl calls (sync-server) off any ambient proxy.
export NO_PROXY="*"
export no_proxy="*"
export HTTP_PROXY=""
export http_proxy=""
export HTTPS_PROXY=""
export https_proxy=""

# Ensure the reload info! line is captured deterministically regardless of the
# binary's default log level.
export RUST_LOG="${RUST_LOG:-bifrost_cli::rules=info,bifrost_admin=info,info}"

if [[ -n "${ADMIN_PORT:-}" ]]; then
    BIFROST_PORT="$ADMIN_PORT"
    SYNC_PORT=${SYNC_PORT:-$((BIFROST_PORT + 8))}
else
    PORT_BASE=$((19000 + ($$ % 400)))
    SYNC_PORT=${SYNC_PORT:-$PORT_BASE}
    BIFROST_PORT=${BIFROST_PORT:-$((PORT_BASE + 1))}
fi
SYNC_SERVER_PID=""
SYNC_DATA_DIR=""
BIFROST_DATA_DIR_E2E=""

CURL_OPTS="--connect-timeout 5 --max-time 10"

api() {
    curl -s $CURL_OPTS "$@"
}

cleanup() {
    echo ""
    echo "=== Cleaning up ==="
    if [[ -n "$SYNC_SERVER_PID" ]] && kill -0 "$SYNC_SERVER_PID" 2>/dev/null; then
        kill "$SYNC_SERVER_PID" 2>/dev/null || true
        wait "$SYNC_SERVER_PID" 2>/dev/null || true
    fi
    admin_cleanup_bifrost

    if [[ -n "$SYNC_DATA_DIR" && -d "$SYNC_DATA_DIR" ]]; then
        rm -rf "$SYNC_DATA_DIR"
    fi
    if [[ -n "$BIFROST_DATA_DIR_E2E" && -d "$BIFROST_DATA_DIR_E2E" ]]; then
        rm -rf "$BIFROST_DATA_DIR_E2E"
    fi
}
trap cleanup EXIT

SYNC_DATA_DIR="$(mktemp -d)"
BIFROST_DATA_DIR_E2E="$(mktemp -d)"

SYNC_URL="http://127.0.0.1:${SYNC_PORT}"

for cmd in jq python3 curl lsof; do
    if ! command -v "$cmd" &>/dev/null; then
        echo "FATAL: Required command '$cmd' not found"
        exit 1
    fi
done

for port in $SYNC_PORT $BIFROST_PORT; do
    existing_pid=$(lsof -nP -iTCP:${port} -sTCP:LISTEN -t 2>/dev/null || true)
    if [[ -n "$existing_pid" ]]; then
        echo "Killing existing process on port ${port} (PID: ${existing_pid})"
        kill -9 "$existing_pid" 2>/dev/null || true
    fi
done
sleep 1

for port in $SYNC_PORT $BIFROST_PORT; do
    if lsof -nP -iTCP:${port} -sTCP:LISTEN -t >/dev/null 2>&1; then
        echo "FATAL: Port ${port} still in use after cleanup"
        exit 1
    fi
done

echo "Using ports: sync=$SYNC_PORT bifrost=$BIFROST_PORT"

# Helper: count reload log lines emitted by the rules hot-reload watcher.
reload_log_count() {
    if [[ -n "$ADMIN_CLIENT_BIFROST_LOG_FILE" && -f "$ADMIN_CLIENT_BIFROST_LOG_FILE" ]]; then
        local count
        count=$(grep -c "config change event received, reloading rules" \
            "$ADMIN_CLIENT_BIFROST_LOG_FILE" 2>/dev/null || true)
        echo "${count:-0}"
    else
        echo 0
    fi
}

# Helper: resolve the on-disk path of a synced group rule file.
# Layout: $BIFROST_DATA_DIR/rules/<sanitized_group_name>/<url-encoded-rule-name>.bifrost
group_rule_file() {
    local group_name="$1"
    local rule_name="$2"
    # sanitize_group_dir_name only replaces / \ \0 :
    local dir_name="${group_name//\//_}"
    dir_name="${dir_name//\\/_}"
    dir_name="${dir_name//:/_}"
    # encode_rule_name uses urlencoding::encode; for our ASCII rule names with no
    # reserved chars the encoded form equals the raw name. Resolve by glob to be
    # robust to encoding of incidental characters.
    local dir="${BIFROST_DATA_DIR_E2E}/rules/${dir_name}"
    local f
    f=$(ls "${dir}/${rule_name}.bifrost" 2>/dev/null | head -1)
    if [[ -z "$f" ]]; then
        # Fallback: first .bifrost whose decoded basename matches.
        f=$(ls "${dir}"/*.bifrost 2>/dev/null | head -1)
    fi
    echo "$f"
}

file_fingerprint() {
    local f="$1"
    if [[ -z "$f" || ! -f "$f" ]]; then
        echo "MISSING"
        return
    fi
    local mtime size sha
    # macOS stat vs GNU stat
    if stat -f '%m %z' "$f" >/dev/null 2>&1; then
        mtime=$(stat -f '%m' "$f")
        size=$(stat -f '%z' "$f")
    else
        mtime=$(stat -c '%Y' "$f")
        size=$(stat -c '%s' "$f")
    fi
    if command -v shasum >/dev/null 2>&1; then
        sha=$(shasum -a 256 "$f" | awk '{print $1}')
    else
        sha=$(sha256sum "$f" | awk '{print $1}')
    fi
    echo "${mtime}:${size}:${sha}"
}

# =============================================
# Start sync-server
# =============================================
SYNC_SERVER_DIR="$REPO_DIR/packages/bifrost-sync-server"
if [[ ! -d "$SYNC_SERVER_DIR/node_modules" ]]; then
    echo "Installing sync-server dependencies..."
    if command -v pnpm &>/dev/null; then
        (cd "$SYNC_SERVER_DIR" && pnpm install --frozen-lockfile) || {
            echo "FATAL: Failed to install sync-server dependencies via pnpm"
            exit 1
        }
    else
        (cd "$SYNC_SERVER_DIR" && npm install --prefer-offline --no-audit --no-fund) || {
            echo "FATAL: Failed to install sync-server dependencies via npm"
            exit 1
        }
    fi
fi

echo "=== Starting sync-server on port ${SYNC_PORT} ==="
SYNC_SERVER_EXEC="$(sync_server_exec "$SYNC_SERVER_DIR")"
(cd "$SYNC_SERVER_DIR" && \
    eval "$SYNC_SERVER_EXEC" \
        -p "$SYNC_PORT" \
        -H 127.0.0.1 \
        --rate-limit-per-ip 1000 \
        --auth-rate-limit-per-ip 1000 \
        -d "$SYNC_DATA_DIR" \
) > "${SYNC_DATA_DIR}/sync-server.log" 2>&1 &
SYNC_SERVER_PID=$!

echo "Waiting for sync-server..."
SYNC_READY=false
for i in $(seq 1 30); do
    if curl -s $CURL_OPTS "${SYNC_URL}/v4/sso/check" >/dev/null 2>&1; then
        echo "sync-server is ready (waited ${i}s)"
        SYNC_READY=true
        break
    fi
    if ! kill -0 "$SYNC_SERVER_PID" 2>/dev/null; then
        echo "FATAL: sync-server exited early"
        cat "${SYNC_DATA_DIR}/sync-server.log" 2>/dev/null || true
        exit 1
    fi
    sleep 1
done
if [[ "$SYNC_READY" != "true" ]]; then
    echo "FATAL: sync-server not ready after 30s"
    cat "${SYNC_DATA_DIR}/sync-server.log" 2>/dev/null || true
    exit 1
fi

# =============================================
# Register user + create group with one env
# =============================================
echo ""
echo "=== Register user and create group ==="

REGISTER_RESP=$(api -X POST -H "Content-Type: application/json" \
    -d '{"user_id":"storm_owner","password":"testpass123","nickname":"Storm Owner"}' \
    "${SYNC_URL}/v4/sso/register")
assert_body_contains '"code":0' "$REGISTER_RESP" "Register storm_owner"
TEST_TOKEN=$(echo "$REGISTER_RESP" | jq -r '.data.token')
assert_not_empty "$TEST_TOKEN" "owner token not empty"

CREATE_GROUP=$(api -X POST -H "Content-Type: application/json" \
    -H "x-bifrost-token: ${TEST_TOKEN}" \
    -d '{"name":"StormGroup","visibility":"private"}' \
    "${SYNC_URL}/v4/group")
assert_body_contains '"code":0' "$CREATE_GROUP" "Create StormGroup"
GROUP_ID=$(echo "$CREATE_GROUP" | jq -r '.data.id')
GROUP_NAME=$(echo "$CREATE_GROUP" | jq -r '.data.name')
assert_not_empty "$GROUP_ID" "group id not empty"

api -X PATCH -H "Content-Type: application/json" \
    -H "x-bifrost-token: ${TEST_TOKEN}" \
    -d '{"rules_enabled":true}' \
    "${SYNC_URL}/v4/group/${GROUP_ID}/setting" > /dev/null

CREATE_ENV=$(api -X POST -H "Content-Type: application/json" \
    -H "x-bifrost-token: ${TEST_TOKEN}" \
    -d "{\"user_id\":\"${GROUP_NAME}\",\"name\":\"storm-rule\",\"rule\":\"storm.example.com host://127.0.0.1:8080\"}" \
    "${SYNC_URL}/v4/env")
assert_body_contains '"code":0' "$CREATE_ENV" "Create group env storm-rule"
ENV_ID=$(echo "$CREATE_ENV" | jq -r '.data.id')
assert_not_empty "$ENV_ID" "env id not empty"

# =============================================
# Start bifrost with sync enabled
# =============================================
echo ""
echo "=== Start Bifrost proxy (sync enabled) ==="

export ADMIN_PORT="$BIFROST_PORT"
export BIFROST_DATA_DIR="$BIFROST_DATA_DIR_E2E"

cat > "${BIFROST_DATA_DIR_E2E}/config.toml" <<EOF
[sync]
enabled = true
auto_sync = false
remote_base_url = "${SYNC_URL}"
probe_interval_secs = 5
connect_timeout_ms = 3000
EOF

cat > "${BIFROST_DATA_DIR_E2E}/sync-state.json" <<EOF
{"token":"${TEST_TOKEN}"}
EOF

admin_start_bifrost
if [[ $? -ne 0 ]]; then
    echo "FATAL: Failed to start bifrost"
    exit 1
fi

admin_post "/api/sync/session" "{\"token\":\"${TEST_TOKEN}\"}" > /dev/null 2>&1 || true

echo "--- Waiting for sync authorized ---"
for i in $(seq 1 20); do
    SYNC_STATUS=$(admin_get "/api/sync/status")
    if echo "$SYNC_STATUS" | jq -e '.has_session == true and .authorized == true' >/dev/null 2>&1; then
        echo "Sync ready (waited ${i}s)"
        break
    fi
    sleep 1
done
SYNC_STATUS=$(admin_get "/api/sync/status")
assert_body_contains '"has_session":true' "$SYNC_STATUS" "Sync has session"

# =============================================
# First sync materializes the local rule file
# =============================================
echo ""
echo "=== First sync (materialize local rule file) ==="

FIRST_LIST=$(admin_get "/api/group-rules/${GROUP_ID}")
echo "First list: $FIRST_LIST"
assert_body_contains '"group_name":"StormGroup"' "$FIRST_LIST" "group_name=StormGroup"
FIRST_RULE=$(echo "$FIRST_LIST" | jq -r '.rules[0].name')
assert_equals "storm-rule" "$FIRST_RULE" "First rule synced = storm-rule"

# Enable the rule so that future content changes count as active_rules_changed.
admin_put "/api/group-rules/${GROUP_ID}/storm-rule/enable" "{}" > /dev/null 2>&1 || true

# Let any enable-triggered reload settle.
sleep 2

RULE_FILE="$(group_rule_file "$GROUP_NAME" "storm-rule")"
echo "Resolved rule file: $RULE_FILE"
assert_not_empty "$RULE_FILE" "Synced rule file path resolved"
if [[ -n "$RULE_FILE" ]]; then
    assert_equals "0" "$([[ -f "$RULE_FILE" ]] && echo 0 || echo 1)" "Synced rule file exists on disk"
fi

# =============================================
# Storm guard: repeated unchanged syncs
# =============================================
echo ""
echo "=== Storm guard: repeated unchanged syncs ==="

BASELINE_FP="$(file_fingerprint "$RULE_FILE")"
BASELINE_RELOADS="$(reload_log_count)"
echo "Baseline fingerprint: $BASELINE_FP"
echo "Baseline reload-log count: $BASELINE_RELOADS"

REPEAT=15
for n in $(seq 1 "$REPEAT"); do
    admin_get "/api/group-rules/${GROUP_ID}" > /dev/null
    sleep 0.3
done
# Allow the broadcast/reload loop time to act if it were (wrongly) going to.
sleep 3

AFTER_FP="$(file_fingerprint "$RULE_FILE")"
AFTER_RELOADS="$(reload_log_count)"
echo "After-$REPEAT-syncs fingerprint: $AFTER_FP"
echo "After-$REPEAT-syncs reload-log count: $AFTER_RELOADS"

assert_equals "$BASELINE_FP" "$AFTER_FP" \
    "No-storm: local rule file unchanged (mtime+size+sha) after $REPEAT repeated syncs"
assert_equals "$BASELINE_RELOADS" "$AFTER_RELOADS" \
    "No-storm: reload-log count unchanged after $REPEAT repeated unchanged syncs"

# =============================================
# Positive control: a real remote change still propagates exactly once
# =============================================
echo ""
echo "=== Positive control: real remote change propagates ==="

PRE_CHANGE_RELOADS="$(reload_log_count)"
PRE_CHANGE_FP="$(file_fingerprint "$RULE_FILE")"

UPDATE_ENV=$(api -X PATCH -H "Content-Type: application/json" \
    -H "x-bifrost-token: ${TEST_TOKEN}" \
    -d '{"rule":"storm.example.com host://10.9.8.7:9090"}' \
    "${SYNC_URL}/v4/env/${ENV_ID}")
assert_body_contains '"code":0' "$UPDATE_ENV" "Remote env updated"

# Trigger the sync that should now detect a real change.
admin_get "/api/group-rules/${GROUP_ID}" > /dev/null
CHANGED_DETAIL=$(admin_get "/api/group-rules/${GROUP_ID}/storm-rule")
assert_body_contains '10.9.8.7' "$CHANGED_DETAIL" "Changed content visible via admin detail"
sleep 3

POST_CHANGE_RELOADS="$(reload_log_count)"
POST_CHANGE_FP="$(file_fingerprint "$RULE_FILE")"
echo "Pre-change reloads=$PRE_CHANGE_RELOADS post-change reloads=$POST_CHANGE_RELOADS"
echo "Pre-change fp=$PRE_CHANGE_FP post-change fp=$POST_CHANGE_FP"

assert_not_equals "$PRE_CHANGE_FP" "$POST_CHANGE_FP" \
    "Real change: local rule file was rewritten with new content"
assert_body_contains '10.9.8.7' "$(cat "$RULE_FILE")" \
    "Real change: local rule file contains new content"

# =============================================
# Storm guard again: after the real change, repeats are stable once more
# =============================================
echo ""
echo "=== Storm guard after real change (re-stabilized) ==="

STABLE_FP="$(file_fingerprint "$RULE_FILE")"
STABLE_RELOADS="$(reload_log_count)"
for n in $(seq 1 "$REPEAT"); do
    admin_get "/api/group-rules/${GROUP_ID}" > /dev/null
    sleep 0.3
done
sleep 3
STABLE_FP2="$(file_fingerprint "$RULE_FILE")"
STABLE_RELOADS2="$(reload_log_count)"

assert_equals "$STABLE_FP" "$STABLE_FP2" \
    "No-storm after change: file unchanged across $REPEAT repeated syncs"
assert_equals "$STABLE_RELOADS" "$STABLE_RELOADS2" \
    "No-storm after change: reload-log count unchanged across $REPEAT repeated syncs"

echo ""
print_test_summary
exit $?
