#!/bin/bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
# E2E Test: Group Management and Group Rules via sync-server + bifrost proxy + CLI
# Tests full lifecycle: sync-server direct API → bifrost admin proxy → CLI commands

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

source "$SCRIPT_DIR/../test_utils/assert.sh"
source "$SCRIPT_DIR/../test_utils/admin_client.sh"
source "$SCRIPT_DIR/../test_utils/http_client.sh"
source "$SCRIPT_DIR/../test_utils/sync_server.sh"

# 确保本脚本中的所有直接 curl 调用（针对 sync-server 和 mock-server）都不受环境代理干扰
# 对于需要走代理的请求，http_client.sh 已被修改为显式清除这些变量
export NO_PROXY="*"
export no_proxy="*"
export HTTP_PROXY=""
export http_proxy=""
export HTTPS_PROXY=""
export https_proxy=""

if [[ -n "${ADMIN_PORT:-}" ]]; then
    BIFROST_PORT="$ADMIN_PORT"
    SYNC_PORT=${SYNC_PORT:-$((BIFROST_PORT + 8))}
    ECHO_PORT=${ECHO_PORT:-$((BIFROST_PORT + 9))}
else
    PORT_BASE=$((18600 + ($$ % 400)))
    SYNC_PORT=${SYNC_PORT:-$PORT_BASE}
    BIFROST_PORT=${BIFROST_PORT:-$((PORT_BASE + 1))}
    ECHO_PORT=${ECHO_PORT:-$((PORT_BASE + 2))}
fi
SYNC_SERVER_PID=""
ECHO_PID=""
SYNC_DATA_DIR=""
BIFROST_DATA_DIR_E2E=""

CURL_OPTS="--connect-timeout 5 --max-time 10"

api() {
    curl -s $CURL_OPTS "$@"
}

api_status() {
    curl -s $CURL_OPTS -o /dev/null -w "%{http_code}" "$@"
}

wait_for_rule_effect() {
    local url="$1"
    local expected_status="$2"
    local max_wait="${3:-10}"
    for _w in $(seq 1 "$max_wait"); do
        http_get "$url"
        if [[ "$HTTP_STATUS" == "$expected_status" ]]; then
            return 0
        fi
        sleep 1
    done
    return 1
}

cleanup() {
    echo ""
    echo "=== Cleaning up ==="
    if [[ -n "$ECHO_PID" ]] && kill -0 "$ECHO_PID" 2>/dev/null; then
        kill "$ECHO_PID" 2>/dev/null || true
        wait "$ECHO_PID" 2>/dev/null || true
    fi
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

for port in $SYNC_PORT $BIFROST_PORT $ECHO_PORT; do
    existing_pid=$(lsof -nP -iTCP:${port} -sTCP:LISTEN -t 2>/dev/null || true)
    if [[ -n "$existing_pid" ]]; then
        echo "Killing existing process on port ${port} (PID: ${existing_pid})"
        kill -9 "$existing_pid" 2>/dev/null || true
    fi
done
sleep 1

for port in $SYNC_PORT $BIFROST_PORT $ECHO_PORT; do
    if lsof -nP -iTCP:${port} -sTCP:LISTEN -t >/dev/null 2>&1; then
        echo "FATAL: Port ${port} still in use after cleanup"
        exit 1
    fi
done

echo "Using ports: sync=$SYNC_PORT bifrost=$BIFROST_PORT echo=$ECHO_PORT"

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
# Step 1: Register test users and obtain tokens
# =============================================
echo ""
echo "=== Step 1: Register test users ==="

REGISTER_RESP=$(api -X POST -H "Content-Type: application/json" \
    -d '{"user_id":"test_bifrost","password":"testpass123","nickname":"Test Bifrost","email":"test@bifrost.dev"}' \
    "${SYNC_URL}/v4/sso/register")
assert_body_contains '"code":0' "$REGISTER_RESP" "Register test_bifrost user"
TEST_TOKEN=$(echo "$REGISTER_RESP" | jq -r '.data.token')
assert_not_empty "$TEST_TOKEN" "test_bifrost token should not be empty"
echo "TEST_TOKEN=${TEST_TOKEN}"

CHECK_RESP=$(api -H "x-bifrost-token: ${TEST_TOKEN}" "${SYNC_URL}/v4/sso/check")
assert_body_contains '"user_id":"test_bifrost"' "$CHECK_RESP" "Token check returns test_bifrost"

REGISTER_RESP2=$(api -X POST -H "Content-Type: application/json" \
    -d '{"user_id":"e2e_member","password":"testpass123","nickname":"Member User"}' \
    "${SYNC_URL}/v4/sso/register")
MEMBER_TOKEN=$(echo "$REGISTER_RESP2" | jq -r '.data.token')
assert_not_empty "$MEMBER_TOKEN" "Member token should not be empty"

REGISTER_RESP3=$(api -X POST -H "Content-Type: application/json" \
    -d '{"user_id":"e2e_outsider","password":"testpass123","nickname":"Outsider"}' \
    "${SYNC_URL}/v4/sso/register")
OUTSIDER_TOKEN=$(echo "$REGISTER_RESP3" | jq -r '.data.token')
assert_not_empty "$OUTSIDER_TOKEN" "Outsider token should not be empty"

# =============================================
# Step 2: Group CRUD via sync-server directly
# =============================================
echo ""
echo "=== Step 2: Group CRUD ==="

echo "--- Create Group ---"
CREATE_GROUP=$(api -X POST -H "Content-Type: application/json" \
    -H "x-bifrost-token: ${TEST_TOKEN}" \
    -d '{"name":"TestGroup","description":"E2E test group","visibility":"private"}' \
    "${SYNC_URL}/v4/group")
assert_body_contains '"code":0' "$CREATE_GROUP" "Create group"
GROUP_ID=$(echo "$CREATE_GROUP" | jq -r '.data.id')
GROUP_NAME=$(echo "$CREATE_GROUP" | jq -r '.data.name')
assert_not_empty "$GROUP_ID" "Group ID not empty"
assert_equals "TestGroup" "$GROUP_NAME" "Group name=TestGroup"

echo "--- Read Group ---"
READ_GROUP=$(api -H "x-bifrost-token: ${TEST_TOKEN}" "${SYNC_URL}/v4/group/${GROUP_ID}")
assert_body_contains '"code":0' "$READ_GROUP" "Read group"
assert_json_field '.data.name' 'TestGroup' "$READ_GROUP" "Group name from read"

echo "--- Update Group ---"
UPDATE_GROUP=$(api -X PATCH -H "Content-Type: application/json" \
    -H "x-bifrost-token: ${TEST_TOKEN}" \
    -d '{"description":"Updated description"}' \
    "${SYNC_URL}/v4/group/${GROUP_ID}")
assert_body_contains '"code":0' "$UPDATE_GROUP" "Update group description"

echo "--- Search Groups (my groups) ---"
SEARCH_GROUPS=$(api -H "x-bifrost-token: ${TEST_TOKEN}" \
    "${SYNC_URL}/v4/group?my=true")
assert_body_contains '"code":0' "$SEARCH_GROUPS" "Search my groups"
MY_GROUPS_COUNT=$(echo "$SEARCH_GROUPS" | jq '.data.list | length')
assert_not_equals "0" "$MY_GROUPS_COUNT" "Has at least 1 group"

# =============================================
# Step 3: Group Members Management
# =============================================
echo ""
echo "=== Step 3: Members Management ==="

echo "--- Invite member (level 1 = master) ---"
INVITE_MASTER=$(api -X POST -H "Content-Type: application/json" \
    -H "x-bifrost-token: ${TEST_TOKEN}" \
    -d "{\"group_id\":\"${GROUP_ID}\",\"user_ids\":[\"e2e_member\"],\"level\":1}" \
    "${SYNC_URL}/v4/group/invite")
assert_body_contains '"code":0' "$INVITE_MASTER" "Invite master"

echo "--- List members ---"
LIST_MEMBERS=$(api -H "x-bifrost-token: ${TEST_TOKEN}" \
    "${SYNC_URL}/v4/group/${GROUP_ID}/members")
assert_body_contains '"code":0' "$LIST_MEMBERS" "List members"
MEMBER_COUNT=$(echo "$LIST_MEMBERS" | jq '.data.list | length')
assert_equals "2" "$MEMBER_COUNT" "2 members (owner+master)"

echo "--- Demote to regular member ---"
DEMOTE=$(api -X PATCH -H "Content-Type: application/json" \
    -H "x-bifrost-token: ${TEST_TOKEN}" \
    -d '{"level":0}' \
    "${SYNC_URL}/v4/group/${GROUP_ID}/member/e2e_member")
assert_body_contains '"code":0' "$DEMOTE" "Demote member"

echo "--- Promote back to master ---"
PROMOTE=$(api -X PATCH -H "Content-Type: application/json" \
    -H "x-bifrost-token: ${TEST_TOKEN}" \
    -d '{"level":1}' \
    "${SYNC_URL}/v4/group/${GROUP_ID}/member/e2e_member")
assert_body_contains '"code":0' "$PROMOTE" "Promote to master"

# =============================================
# Step 4: Group Settings
# =============================================
echo ""
echo "=== Step 4: Group Settings ==="

echo "--- Get group setting ---"
GET_SETTING=$(api -H "x-bifrost-token: ${TEST_TOKEN}" \
    "${SYNC_URL}/v4/group/${GROUP_ID}/setting")
assert_body_contains '"code":0' "$GET_SETTING" "Get setting"
assert_json_field '.data.level' '0' "$GET_SETTING" "Default visibility=private"

echo "--- Update setting to public ---"
UPDATE_SETTING=$(api -X PATCH -H "Content-Type: application/json" \
    -H "x-bifrost-token: ${TEST_TOKEN}" \
    -d '{"visibility":"public","rules_enabled":true}' \
    "${SYNC_URL}/v4/group/${GROUP_ID}/setting")
assert_body_contains '"code":0' "$UPDATE_SETTING" "Update setting"

GET_SETTING2=$(api -H "x-bifrost-token: ${TEST_TOKEN}" \
    "${SYNC_URL}/v4/group/${GROUP_ID}/setting")
assert_json_field '.data.level' '1' "$GET_SETTING2" "visibility=public"

echo "--- Revert to private ---"
api -X PATCH -H "Content-Type: application/json" \
    -H "x-bifrost-token: ${TEST_TOKEN}" \
    -d '{"visibility":"private"}' \
    "${SYNC_URL}/v4/group/${GROUP_ID}/setting" > /dev/null

# =============================================
# Step 5: Group Environment (Rule) Permission
# =============================================
echo ""
echo "=== Step 5: Group Env Permissions ==="

echo "--- Owner creates env for group ---"
CREATE_ENV=$(api -X POST -H "Content-Type: application/json" \
    -H "x-bifrost-token: ${TEST_TOKEN}" \
    -d "{\"user_id\":\"${GROUP_NAME}\",\"name\":\"production\",\"rule\":\"example.com 127.0.0.1\"}" \
    "${SYNC_URL}/v4/env")
assert_body_contains '"code":0' "$CREATE_ENV" "Owner creates group env"
ENV_ID=$(echo "$CREATE_ENV" | jq -r '.data.id')
assert_not_empty "$ENV_ID" "Env ID not empty"

echo "--- Master creates env ---"
CREATE_ENV2=$(api -X POST -H "Content-Type: application/json" \
    -H "x-bifrost-token: ${MEMBER_TOKEN}" \
    -d "{\"user_id\":\"${GROUP_NAME}\",\"name\":\"staging\",\"rule\":\"staging.com 10.0.0.1\"}" \
    "${SYNC_URL}/v4/env")
assert_body_contains '"code":0' "$CREATE_ENV2" "Master creates group env"
ENV_ID2=$(echo "$CREATE_ENV2" | jq -r '.data.id')

echo "--- Outsider CANNOT create ---"
CREATE_ENV_FAIL=$(api_status -X POST -H "Content-Type: application/json" \
    -H "x-bifrost-token: ${OUTSIDER_TOKEN}" \
    -d "{\"user_id\":\"${GROUP_NAME}\",\"name\":\"hack\",\"rule\":\"evil.com 1.2.3.4\"}" \
    "${SYNC_URL}/v4/env")
assert_status "403" "$CREATE_ENV_FAIL" "Outsider denied on create"

echo "--- Search group envs ---"
SEARCH_ENV=$(api -H "x-bifrost-token: ${TEST_TOKEN}" \
    "${SYNC_URL}/v4/env?user_id=${GROUP_NAME}")
assert_body_contains '"code":0' "$SEARCH_ENV" "Search group envs"
ENV_COUNT=$(echo "$SEARCH_ENV" | jq '.data.list | length')
assert_equals "2" "$ENV_COUNT" "2 group envs"

echo "--- Member reads env ---"
READ_ENV=$(api -H "x-bifrost-token: ${MEMBER_TOKEN}" "${SYNC_URL}/v4/env/${ENV_ID}")
assert_body_contains '"code":0' "$READ_ENV" "Member reads group env"

echo "--- Outsider CANNOT read ---"
READ_ENV_FAIL=$(api_status \
    -H "x-bifrost-token: ${OUTSIDER_TOKEN}" \
    "${SYNC_URL}/v4/env/${ENV_ID}")
assert_status "403" "$READ_ENV_FAIL" "Outsider denied on read"

echo "--- Master updates env ---"
UPDATE_ENV=$(api -X PATCH -H "Content-Type: application/json" \
    -H "x-bifrost-token: ${MEMBER_TOKEN}" \
    -d '{"rule":"example.com 10.0.0.100"}' \
    "${SYNC_URL}/v4/env/${ENV_ID}")
assert_body_contains '"code":0' "$UPDATE_ENV" "Master updates group env"

echo "--- Outsider CANNOT update ---"
UPDATE_ENV_FAIL=$(api_status -X PATCH -H "Content-Type: application/json" \
    -H "x-bifrost-token: ${OUTSIDER_TOKEN}" \
    -d '{"rule":"hacked"}' \
    "${SYNC_URL}/v4/env/${ENV_ID}")
assert_status "403" "$UPDATE_ENV_FAIL" "Outsider denied on update"

echo "--- Owner deletes staging env ---"
DELETE_ENV=$(api -X DELETE -H "x-bifrost-token: ${TEST_TOKEN}" \
    "${SYNC_URL}/v4/env/${ENV_ID2}")
assert_body_contains '"code":0' "$DELETE_ENV" "Owner deletes staging env"

# =============================================
# Step 6: User Peer Endpoint
# =============================================
echo ""
echo "=== Step 6: User Peer ==="

PEER_RESP=$(api -H "x-bifrost-token: ${TEST_TOKEN}" \
    "${SYNC_URL}/v4/user/peer?offset=0&limit=100")
assert_body_contains '"code":0' "$PEER_RESP" "Peer endpoint"
PEER_COUNT=$(echo "$PEER_RESP" | jq '.data.list | length')
assert_not_equals "0" "$PEER_COUNT" "Has peers"

SELF_PEER=$(echo "$PEER_RESP" | jq -r '.data.list[] | select(.channel == 1) | .user_id')
assert_equals "test_bifrost" "$SELF_PEER" "Self peer (channel=1)"

GROUP_PEER=$(echo "$PEER_RESP" | jq -r '.data.list[] | select(.channel == 3) | .user_id')
assert_equals "$GROUP_NAME" "$GROUP_PEER" "Group virtual peer (channel=3)"

GROUP_PEER_EDITABLE=$(echo "$PEER_RESP" | jq -r '.data.list[] | select(.channel == 3) | .editable')
assert_equals "true" "$GROUP_PEER_EDITABLE" "Owner editable=true"

GROUP_PEER_GID=$(echo "$PEER_RESP" | jq -r '.data.list[] | select(.channel == 3) | .group_id')
assert_equals "$GROUP_ID" "$GROUP_PEER_GID" "Group peer has group_id"

# =============================================
# Step 7: Public Group Access
# =============================================
echo ""
echo "=== Step 7: Public Group Access ==="

echo "--- Create public group ---"
CREATE_PUB_GROUP=$(api -X POST -H "Content-Type: application/json" \
    -H "x-bifrost-token: ${TEST_TOKEN}" \
    -d '{"name":"PublicGroup","visibility":"public"}' \
    "${SYNC_URL}/v4/group")
PUB_GROUP_ID=$(echo "$CREATE_PUB_GROUP" | jq -r '.data.id')

api -X PATCH -H "Content-Type: application/json" \
    -H "x-bifrost-token: ${TEST_TOKEN}" \
    -d '{"visibility":"public"}' \
    "${SYNC_URL}/v4/group/${PUB_GROUP_ID}/setting" > /dev/null

CREATE_PUB_ENV=$(api -X POST -H "Content-Type: application/json" \
    -H "x-bifrost-token: ${TEST_TOKEN}" \
    -d '{"user_id":"PublicGroup","name":"pub-rule","rule":"public.com 10.0.0.1"}' \
    "${SYNC_URL}/v4/env")
PUB_ENV_ID=$(echo "$CREATE_PUB_ENV" | jq -r '.data.id')

echo "--- Outsider reads public group env ---"
READ_PUB=$(api -H "x-bifrost-token: ${OUTSIDER_TOKEN}" "${SYNC_URL}/v4/env/${PUB_ENV_ID}")
assert_body_contains '"code":0' "$READ_PUB" "Outsider reads public env"

echo "--- Outsider searches public group ---"
SEARCH_PUB=$(api -H "x-bifrost-token: ${OUTSIDER_TOKEN}" \
    "${SYNC_URL}/v4/env?user_id=PublicGroup")
assert_body_contains '"code":0' "$SEARCH_PUB" "Outsider searches public group envs"

echo "--- Outsider CANNOT create for public group ---"
PUB_CREATE_FAIL=$(api_status -X POST -H "Content-Type: application/json" \
    -H "x-bifrost-token: ${OUTSIDER_TOKEN}" \
    -d '{"user_id":"PublicGroup","name":"hack","rule":"evil 1.2.3.4"}' \
    "${SYNC_URL}/v4/env")
assert_status "403" "$PUB_CREATE_FAIL" "Outsider denied create on public group"

# =============================================
# Step 8: Cascade Delete
# =============================================
echo ""
echo "=== Step 8: Cascade Delete ==="

CASCADE_GROUP=$(api -X POST -H "Content-Type: application/json" \
    -H "x-bifrost-token: ${TEST_TOKEN}" \
    -d '{"name":"CascadeGroup"}' \
    "${SYNC_URL}/v4/group")
CASCADE_ID=$(echo "$CASCADE_GROUP" | jq -r '.data.id')

CASCADE_ENV1=$(api -X POST -H "Content-Type: application/json" \
    -H "x-bifrost-token: ${TEST_TOKEN}" \
    -d '{"user_id":"CascadeGroup","name":"env1","rule":"a.com 1.1.1.1"}' \
    "${SYNC_URL}/v4/env")
CASCADE_ENV1_ID=$(echo "$CASCADE_ENV1" | jq -r '.data.id')

CASCADE_ENV2=$(api -X POST -H "Content-Type: application/json" \
    -H "x-bifrost-token: ${TEST_TOKEN}" \
    -d '{"user_id":"CascadeGroup","name":"env2","rule":"b.com 2.2.2.2"}' \
    "${SYNC_URL}/v4/env")
CASCADE_ENV2_ID=$(echo "$CASCADE_ENV2" | jq -r '.data.id')

echo "--- Delete group ---"
api -X DELETE -H "x-bifrost-token: ${TEST_TOKEN}" \
    "${SYNC_URL}/v4/group/${CASCADE_ID}" > /dev/null

echo "--- Verify cascade deletion ---"
ENV1_CHECK=$(api_status \
    -H "x-bifrost-token: ${TEST_TOKEN}" \
    "${SYNC_URL}/v4/env/${CASCADE_ENV1_ID}")
assert_status "404" "$ENV1_CHECK" "Cascade: env1 deleted"

ENV2_CHECK=$(api_status \
    -H "x-bifrost-token: ${TEST_TOKEN}" \
    "${SYNC_URL}/v4/env/${CASCADE_ENV2_ID}")
assert_status "404" "$ENV2_CHECK" "Cascade: env2 deleted"

# =============================================
# Step 9: Start Bifrost proxy with sync config
# =============================================
echo ""
echo "=== Step 9: Start Bifrost proxy ==="

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
    if [[ -n "$ADMIN_CLIENT_BIFROST_LOG_FILE" ]]; then
        echo "=== Bifrost log ===" >&2
        cat "$ADMIN_CLIENT_BIFROST_LOG_FILE" 2>/dev/null >&2 || true
    fi
    exit 1
fi

echo "--- Saving sync token via admin API (triggers immediate sync) ---"
admin_post "/api/sync/session" "{\"token\":\"${TEST_TOKEN}\"}" > /dev/null 2>&1 || true

echo "--- Waiting for sync to become authorized ---"
for i in $(seq 1 20); do
    SYNC_STATUS=$(admin_get "/api/sync/status")
    if echo "$SYNC_STATUS" | jq -e '.has_session == true and .authorized == true' >/dev/null 2>&1; then
        echo "Sync is ready (waited ${i}s)"
        break
    fi
    sleep 1
done

SYNC_STATUS=$(admin_get "/api/sync/status")
echo "Sync status: $SYNC_STATUS"
assert_body_contains '"has_session":true' "$SYNC_STATUS" "Sync has session"
assert_body_contains '"enabled":true' "$SYNC_STATUS" "Sync enabled"

# =============================================
# Step 10: Group operations via Bifrost Admin API
# =============================================
echo ""
echo "=== Step 10: Group via Admin API ==="

echo "--- List groups ---"
PROXY_GROUP_LIST=$(admin_get "/api/group?my=true&offset=0&limit=50")
echo "Group list: $PROXY_GROUP_LIST"
assert_body_contains '"code":0' "$PROXY_GROUP_LIST" "Proxy list groups"
PROXY_GROUP_COUNT=$(echo "$PROXY_GROUP_LIST" | jq '.data.list | length')
assert_not_equals "0" "$PROXY_GROUP_COUNT" "Has groups via proxy"

echo "--- Show group ---"
PROXY_GROUP_SHOW=$(admin_get "/api/group/${GROUP_ID}")
assert_body_contains '"code":0' "$PROXY_GROUP_SHOW" "Proxy show group"
assert_json_field '.data.name' 'TestGroup' "$PROXY_GROUP_SHOW" "Group name via proxy"

echo "--- Search groups ---"
PROXY_GROUP_SEARCH=$(admin_get "/api/group?keyword=Test&offset=0&limit=50")
assert_body_contains '"code":0' "$PROXY_GROUP_SEARCH" "Proxy search groups"

# =============================================
# Step 11: Group Rules via Admin API
# =============================================
echo ""
echo "=== Step 11: Group Rules via Admin API ==="

echo "--- List group rules ---"
RULES_LIST=$(admin_get "/api/group-rules/${GROUP_ID}")
echo "Group rules: $RULES_LIST"
assert_body_contains '"group_id"' "$RULES_LIST" "Has group_id"
assert_body_contains '"group_name":"TestGroup"' "$RULES_LIST" "group_name=TestGroup"
assert_body_contains '"writable":true' "$RULES_LIST" "writable=true"

RULES_COUNT=$(echo "$RULES_LIST" | jq '.rules | length')
assert_equals "1" "$RULES_COUNT" "1 rule (production)"

FIRST_RULE=$(echo "$RULES_LIST" | jq -r '.rules[0].name')
assert_equals "production" "$FIRST_RULE" "First rule=production"

echo "--- Create group rule ---"
CREATE_RULE=$(admin_post "/api/group-rules/${GROUP_ID}" '{"name":"e2e-rule","content":"test.local 127.0.0.1\napi.local 127.0.0.2"}')
echo "Create rule: $CREATE_RULE"
assert_body_contains '"name":"e2e-rule"' "$CREATE_RULE" "Created rule name"

echo "--- List rules (should be 2) ---"
RULES_LIST2=$(admin_get "/api/group-rules/${GROUP_ID}")
RULES_COUNT2=$(echo "$RULES_LIST2" | jq '.rules | length')
assert_equals "2" "$RULES_COUNT2" "2 rules"

echo "--- Show single rule ---"
RULE_DETAIL=$(admin_get "/api/group-rules/${GROUP_ID}/e2e-rule")
echo "Rule detail: $RULE_DETAIL"
assert_body_contains '"name":"e2e-rule"' "$RULE_DETAIL" "Rule name in detail"
assert_body_contains '"status":"synced"' "$RULE_DETAIL" "Rule synced status"

echo "--- Update group rule ---"
UPDATE_RULE_RESP=$(admin_put "/api/group-rules/${GROUP_ID}/e2e-rule" '{"content":"updated.local 10.0.0.1"}')
echo "Update rule: $UPDATE_RULE_RESP"
assert_body_contains '"name":"e2e-rule"' "$UPDATE_RULE_RESP" "Updated rule name"
assert_body_contains 'updated.local' "$UPDATE_RULE_RESP" "Updated content"

echo "--- Enable rule ---"
ENABLE_RESP=$(admin_put "/api/group-rules/${GROUP_ID}/e2e-rule/enable" "{}")
echo "Enable resp: $ENABLE_RESP"
assert_body_contains 'enabled' "$ENABLE_RESP" "Rule enabled"

echo "--- Disable rule ---"
DISABLE_RESP=$(admin_put "/api/group-rules/${GROUP_ID}/e2e-rule/disable" "{}")
echo "Disable resp: $DISABLE_RESP"
assert_body_contains 'disabled' "$DISABLE_RESP" "Rule disabled"

echo "--- Delete rule ---"
DELETE_RULE_RESP=$(admin_delete "/api/group-rules/${GROUP_ID}/e2e-rule")
echo "Delete resp: $DELETE_RULE_RESP"
assert_body_contains 'deleted' "$DELETE_RULE_RESP" "Rule deleted"

echo "--- Verify deletion ---"
RULES_AFTER=$(admin_get "/api/group-rules/${GROUP_ID}")
RULES_AFTER_COUNT=$(echo "$RULES_AFTER" | jq '.rules | length')
assert_equals "1" "$RULES_AFTER_COUNT" "1 rule after deletion"

# =============================================
# Step 12: Rule Effectiveness Verification
# =============================================
echo ""
echo "=== Step 12: Rule Effectiveness Verification ==="

export PROXY_HOST="127.0.0.1"
export PROXY_PORT="$BIFROST_PORT"

echo "--- Start echo server on port $ECHO_PORT ---"
python3 "$SCRIPT_DIR/../mock_servers/http_echo_server.py" "$ECHO_PORT" > "${BIFROST_DATA_DIR_E2E}/echo.log" 2>&1 &
ECHO_PID=$!

ECHO_READY=false
for i in $(seq 1 15); do
    if curl -sf --connect-timeout 2 --max-time 5 "http://127.0.0.1:${ECHO_PORT}/health" >/dev/null 2>&1; then
        echo "Echo server is ready (waited ${i}s)"
        ECHO_READY=true
        break
    fi
    if ! kill -0 "$ECHO_PID" 2>/dev/null; then
        echo "FATAL: Echo server exited early"
        cat "${BIFROST_DATA_DIR_E2E}/echo.log" 2>/dev/null || true
        ECHO_PID=""
        break
    fi
    sleep 1
done
if [[ "$ECHO_READY" != "true" ]]; then
    echo "WARNING: Echo server not ready, skipping rule effectiveness tests"
fi

ECHO_TARGET="echo-group-test.local"
ECHO_RULE_CONTENT="${ECHO_TARGET} 127.0.0.1:${ECHO_PORT}"

echo "--- Create forwarding group rule ---"
EFFECT_RULE=$(admin_post "/api/group-rules/${GROUP_ID}" "{\"name\":\"effect-test\",\"content\":\"${ECHO_RULE_CONTENT}\"}")
echo "Effect rule: $EFFECT_RULE"
assert_body_contains '"name":"effect-test"' "$EFFECT_RULE" "Forwarding rule created"

echo "--- Enable forwarding rule ---"
EFFECT_ENABLE=$(admin_put "/api/group-rules/${GROUP_ID}/effect-test/enable" "{}")
assert_body_contains 'enabled' "$EFFECT_ENABLE" "Forwarding rule enabled"

echo "--- Request through proxy (rule should forward to echo server) ---"
wait_for_rule_effect "http://${ECHO_TARGET}/health" "200" 10
echo "  HTTP status: $HTTP_STATUS"
assert_status "200" "$HTTP_STATUS" "Rule effect: forwarded to echo server (200)"
assert_body_contains "ok" "$HTTP_BODY" "Rule effect: echo server responded"

echo "--- Request through proxy (echo server status endpoint) ---"
http_get "http://${ECHO_TARGET}/status/200"
echo "  HTTP status: $HTTP_STATUS"
assert_status "200" "$HTTP_STATUS" "Rule effect: echo status 200"

echo "--- Disable forwarding rule ---"
admin_put "/api/group-rules/${GROUP_ID}/effect-test/disable" "{}" > /dev/null

echo "--- Request after disable (rule should NOT forward) ---"
for _dw in $(seq 1 10); do
    http_get "http://${ECHO_TARGET}/health"
    if [[ "$HTTP_STATUS" != "200" ]]; then
        break
    fi
    sleep 1
done
echo "  HTTP status after disable: $HTTP_STATUS"
assert_not_equals "200" "$HTTP_STATUS" "Rule disable: not forwarded anymore"

echo "--- Re-enable and create statusCode mock rule ---"
MOCK_RULE=$(admin_post "/api/group-rules/${GROUP_ID}" "{\"name\":\"mock-status\",\"content\":\"${ECHO_TARGET} statusCode://201\"}")
assert_body_contains '"name":"mock-status"' "$MOCK_RULE" "Mock status rule created"

MOCK_ENABLE=$(admin_put "/api/group-rules/${GROUP_ID}/mock-status/enable" "{}")
assert_body_contains 'enabled' "$MOCK_ENABLE" "Mock status rule enabled"

echo "--- Request through proxy (should get mocked 201) ---"
wait_for_rule_effect "http://${ECHO_TARGET}/anything" "201" 10
echo "  HTTP status: $HTTP_STATUS"
assert_status "201" "$HTTP_STATUS" "Mock rule effect: status code 201"

echo "--- Disable mock rule ---"
admin_put "/api/group-rules/${GROUP_ID}/mock-status/disable" "{}" > /dev/null

echo "--- Request after mock disable ---"
for _dw in $(seq 1 10); do
    http_get "http://${ECHO_TARGET}/anything"
    if [[ "$HTTP_STATUS" != "201" ]]; then
        break
    fi
    sleep 1
done
echo "  HTTP status after mock disable: $HTTP_STATUS"
assert_not_equals "201" "$HTTP_STATUS" "Mock disable: status not 201"

echo "--- Cleanup: delete test rules ---"
admin_delete "/api/group-rules/${GROUP_ID}/effect-test" > /dev/null
admin_delete "/api/group-rules/${GROUP_ID}/mock-status" > /dev/null

if [[ -n "$ECHO_PID" ]] && kill -0 "$ECHO_PID" 2>/dev/null; then
    kill "$ECHO_PID" 2>/dev/null || true
    wait "$ECHO_PID" 2>/dev/null || true
    ECHO_PID=""
fi

# =============================================
# Step 13: CLI Group Operations
# =============================================
echo ""
echo "=== Step 13: CLI Group Operations ==="

BIFROST_BIN="$REPO_DIR/target/release/bifrost"
if [[ ! -x "$BIFROST_BIN" ]]; then
    echo "WARNING: No release binary, skipping CLI tests"
    BIFROST_BIN=""
fi

if [[ -n "$BIFROST_BIN" ]]; then
    echo "--- CLI: group list ---"
    CLI_LIST=$(BIFROST_DATA_DIR="$BIFROST_DATA_DIR_E2E" "$BIFROST_BIN" -p "$BIFROST_PORT" group list 2>&1) || true
    echo "$CLI_LIST"
    assert_body_contains "TestGroup" "$CLI_LIST" "CLI: group list shows TestGroup"

    echo "--- CLI: group show ---"
    CLI_SHOW=$(BIFROST_DATA_DIR="$BIFROST_DATA_DIR_E2E" "$BIFROST_BIN" -p "$BIFROST_PORT" group show "$GROUP_ID" 2>&1) || true
    echo "$CLI_SHOW"
    assert_body_contains "TestGroup" "$CLI_SHOW" "CLI: group show"

    echo "--- CLI: group rule list ---"
    CLI_RULE_LIST=$(BIFROST_DATA_DIR="$BIFROST_DATA_DIR_E2E" "$BIFROST_BIN" -p "$BIFROST_PORT" group rule list "$GROUP_ID" 2>&1) || true
    echo "$CLI_RULE_LIST"
    assert_body_contains "production" "$CLI_RULE_LIST" "CLI: rule list shows production"

    echo "--- CLI: group rule add ---"
    CLI_RULE_ADD=$(BIFROST_DATA_DIR="$BIFROST_DATA_DIR_E2E" "$BIFROST_BIN" -p "$BIFROST_PORT" group rule add "$GROUP_ID" cli-rule --content "cli.test 127.0.0.1" 2>&1) || true
    echo "$CLI_RULE_ADD"
    assert_body_contains "added" "$CLI_RULE_ADD" "CLI: rule added"

    echo "--- CLI: group rule show ---"
    CLI_RULE_SHOW=$(BIFROST_DATA_DIR="$BIFROST_DATA_DIR_E2E" "$BIFROST_BIN" -p "$BIFROST_PORT" group rule show "$GROUP_ID" cli-rule 2>&1) || true
    echo "$CLI_RULE_SHOW"
    assert_body_contains "cli-rule" "$CLI_RULE_SHOW" "CLI: rule show name"
    assert_body_contains "cli.test" "$CLI_RULE_SHOW" "CLI: rule show content"

    echo "--- CLI: group rule update ---"
    CLI_RULE_UPD=$(BIFROST_DATA_DIR="$BIFROST_DATA_DIR_E2E" "$BIFROST_BIN" -p "$BIFROST_PORT" group rule update "$GROUP_ID" cli-rule --content "updated.cli 10.0.0.1" 2>&1) || true
    echo "$CLI_RULE_UPD"
    assert_body_contains "updated" "$CLI_RULE_UPD" "CLI: rule updated"

    echo "--- CLI: group rule enable ---"
    CLI_ENABLE=$(BIFROST_DATA_DIR="$BIFROST_DATA_DIR_E2E" "$BIFROST_BIN" -p "$BIFROST_PORT" group rule enable "$GROUP_ID" cli-rule 2>&1) || true
    echo "$CLI_ENABLE"
    assert_body_contains "enabled" "$CLI_ENABLE" "CLI: rule enabled"

    echo "--- CLI: group rule disable ---"
    CLI_DISABLE=$(BIFROST_DATA_DIR="$BIFROST_DATA_DIR_E2E" "$BIFROST_BIN" -p "$BIFROST_PORT" group rule disable "$GROUP_ID" cli-rule 2>&1) || true
    echo "$CLI_DISABLE"
    assert_body_contains "disabled" "$CLI_DISABLE" "CLI: rule disabled"

    echo "--- CLI: group rule delete ---"
    CLI_DEL=$(BIFROST_DATA_DIR="$BIFROST_DATA_DIR_E2E" "$BIFROST_BIN" -p "$BIFROST_PORT" group rule delete "$GROUP_ID" cli-rule 2>&1) || true
    echo "$CLI_DEL"
    assert_body_contains "deleted" "$CLI_DEL" "CLI: rule deleted"

    echo "--- CLI: verify deletion ---"
    CLI_FINAL=$(BIFROST_DATA_DIR="$BIFROST_DATA_DIR_E2E" "$BIFROST_BIN" -p "$BIFROST_PORT" group rule list "$GROUP_ID" 2>&1) || true
    echo "$CLI_FINAL"
    assert_body_not_contains "cli-rule" "$CLI_FINAL" "CLI: deleted rule gone"
fi

# =============================================
# Step 14: Member Leave & Permission Checks
# =============================================
echo ""
echo "=== Step 14: Member Leave ==="

echo "--- Member leaves group ---"
LEAVE_RESP=$(api -X POST -H "Content-Type: application/json" \
    -H "x-bifrost-token: ${MEMBER_TOKEN}" \
    "${SYNC_URL}/v4/group/${GROUP_ID}/leave")
assert_body_contains '"code":0' "$LEAVE_RESP" "Member leaves"

echo "--- Former member CANNOT create env ---"
FORMER_CREATE=$(api_status -X POST -H "Content-Type: application/json" \
    -H "x-bifrost-token: ${MEMBER_TOKEN}" \
    -d "{\"user_id\":\"${GROUP_NAME}\",\"name\":\"hack\",\"rule\":\"evil 1\"}" \
    "${SYNC_URL}/v4/env")
assert_status "403" "$FORMER_CREATE" "Former member denied create"

echo "--- Former member CANNOT read ---"
FORMER_READ=$(api_status \
    -H "x-bifrost-token: ${MEMBER_TOKEN}" \
    "${SYNC_URL}/v4/env/${ENV_ID}")
assert_status "403" "$FORMER_READ" "Former member denied read"

# =============================================
# Step 15: Sync permissions
# =============================================
echo ""
echo "=== Step 15: Sync Permissions ==="

echo "--- Outsider sync update denied ---"
SYNC_FAIL=$(api -X POST -H "Content-Type: application/json" \
    -H "x-bifrost-token: ${OUTSIDER_TOKEN}" \
    -d "{\"user_ids\":[],\"check_list\":[],\"update_list\":[{\"user_id\":\"${GROUP_NAME}\",\"id\":\"${ENV_ID}\",\"name\":\"production\",\"rule\":\"hacked\",\"update_time\":\"2026-01-01T00:00:00Z\"}],\"delete_list\":[]}" \
    "${SYNC_URL}/v4/env/sync")
assert_body_contains '"status":1' "$SYNC_FAIL" "Outsider sync denied"
assert_body_contains 'denied' "$SYNC_FAIL" "Denied message"

echo "--- Owner sync update succeeds ---"
SYNC_OK=$(api -X POST -H "Content-Type: application/json" \
    -H "x-bifrost-token: ${TEST_TOKEN}" \
    -d "{\"user_ids\":[],\"check_list\":[],\"update_list\":[{\"user_id\":\"${GROUP_NAME}\",\"id\":\"${ENV_ID}\",\"name\":\"production\",\"rule\":\"example.com 10.0.0.200\",\"update_time\":\"2026-01-01T00:00:00Z\"}],\"delete_list\":[]}" \
    "${SYNC_URL}/v4/env/sync")
assert_body_contains '"status":0' "$SYNC_OK" "Owner sync succeeds"

echo ""
print_test_summary
exit $?
