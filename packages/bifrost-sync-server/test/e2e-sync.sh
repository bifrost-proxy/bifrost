#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
SYNC_SERVER_DIR="$SCRIPT_DIR/.."
TMP_DIR="${BIFROST_SYNC_E2E_TMP_DIR:-}"
TMP_AUTO=false

python3_cmd() {
  if command -v python3 >/dev/null 2>&1; then
    echo "python3"
    return 0
  fi
  if command -v python >/dev/null 2>&1; then
    if python -c 'import sys; raise SystemExit(0 if sys.version_info[0] >= 3 else 1)' >/dev/null 2>&1; then
      echo "python"
      return 0
    fi
  fi
  return 1
}

PYTHON_BIN="$(python3_cmd 2>/dev/null || true)"
if [[ -z "${PYTHON_BIN:-}" ]]; then
  echo "python3 (or python>=3) is required for sync-server E2E" >&2
  exit 1
fi

alloc_free_port() {
  "$PYTHON_BIN" - <<'PY'
import socket
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

if [[ -z "${TMP_DIR:-}" ]]; then
  TMP_DIR="$(mktemp -d)"
  TMP_AUTO=true
fi

SYNC_DATA_DIR="${SYNC_DATA_DIR:-$TMP_DIR/sync-data}"
PROXY_DATA_DIR="${PROXY_DATA_DIR:-$TMP_DIR/proxy-data}"

SYNC_PORT="${SYNC_PORT:-0}"
PROXY_PORT="${PROXY_PORT:-0}"
if [[ "$SYNC_PORT" == "0" ]]; then
  SYNC_PORT="$(alloc_free_port)"
fi
if [[ "$PROXY_PORT" == "0" ]]; then
  PROXY_PORT="$(alloc_free_port)"
fi

RULE_TARGET_PORT_1="${RULE_TARGET_PORT_1:-0}"
RULE_TARGET_PORT_2="${RULE_TARGET_PORT_2:-0}"
if [[ "$RULE_TARGET_PORT_1" == "0" ]]; then
  RULE_TARGET_PORT_1="$(alloc_free_port)"
fi
if [[ "$RULE_TARGET_PORT_2" == "0" ]]; then
  RULE_TARGET_PORT_2="$(alloc_free_port)"
fi
ADMIN_BASE="http://127.0.0.1:$PROXY_PORT/_bifrost"
SYNC_BASE="http://127.0.0.1:$SYNC_PORT"

SYNC_PID=""
PROXY_PID=""

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

cleanup() {
  echo -e "\n${YELLOW}[cleanup] stopping services...${NC}"
  [ -n "$SYNC_PID" ] && kill "$SYNC_PID" 2>/dev/null && wait "$SYNC_PID" 2>/dev/null || true
  [ -n "$PROXY_PID" ] && kill "$PROXY_PID" 2>/dev/null && wait "$PROXY_PID" 2>/dev/null || true
  kill $(jobs -p) 2>/dev/null || true
  rm -rf "$TMP_DIR" 2>/dev/null || true
  echo -e "${YELLOW}[cleanup] done${NC}"
}
trap cleanup EXIT

assert_eq() {
  local label="$1" expected="$2" actual="$3"
  if [ "$expected" = "$actual" ]; then
    echo -e "  ${GREEN}✓ $label${NC}"
  else
    echo -e "  ${RED}✗ $label: expected '$expected', got '$actual'${NC}"
    exit 1
  fi
}

assert_contains() {
  local label="$1" haystack="$2" needle="$3"
  if echo "$haystack" | grep -q "$needle"; then
    echo -e "  ${GREEN}✓ $label${NC}"
  else
    echo -e "  ${RED}✗ $label: expected to contain '$needle'${NC}"
    echo -e "  ${RED}  actual: '$haystack'${NC}"
    exit 1
  fi
}

assert_not_empty() {
  local label="$1" value="$2"
  if [ -n "$value" ] && [ "$value" != "null" ]; then
    echo -e "  ${GREEN}✓ $label (value: $value)${NC}"
  else
    echo -e "  ${RED}✗ $label: expected non-empty value, got '$value'${NC}"
    exit 1
  fi
}

assert_http_ok() {
  local label="$1" status_code="$2"
  if [ "$status_code" = "200" ] || [ "$status_code" = "201" ] || [ "$status_code" = "204" ]; then
    echo -e "  ${GREEN}✓ $label (HTTP $status_code)${NC}"
  else
    echo -e "  ${RED}✗ $label: expected HTTP 2xx, got $status_code${NC}"
    exit 1
  fi
}

wait_for_service() {
  local url="$1" name="$2" max_wait="${3:-30}"
  local waited=0
  echo -e "${CYAN}[wait] waiting for $name ($url)...${NC}"
  while true; do
    local http_code
    http_code=$(curl -s -o /dev/null -w "%{http_code}" "$url" 2>/dev/null) || true
    if [ "$http_code" != "000" ] && [ -n "$http_code" ]; then
      break
    fi
    sleep 0.2
    waited=$((waited + 1))
    if [ "$waited" -ge "$max_wait" ]; then
      echo -e "${RED}[error] $name did not start within ${max_wait}s${NC}"
      exit 1
    fi
  done
  echo -e "${GREEN}[ready] $name is running (HTTP $http_code)${NC}"
}

wait_for_sync_status() {
  local url="$1" name="$2" max_wait="${3:-60}"
  local waited=0
  echo -e "${CYAN}[wait] waiting for $name to become reachable+authorized...${NC}"

  while [ "$waited" -lt "$max_wait" ]; do
    local body
    body=$(curl -s "$url" 2>/dev/null) || body=""
    if [ -n "$body" ]; then
      local ok
      ok=$(printf '%s' "$body" | "$PYTHON_BIN" -c '
import json
import sys

try:
  data = json.load(sys.stdin)
except Exception:
  print("0")
  sys.exit(0)

reachable = bool(data.get("reachable", False))
authorized = bool(data.get("authorized", False))
print("1" if (reachable and authorized) else "0")
')
      if [ "$ok" = "1" ]; then
        echo -e "${GREEN}[ready] $name is reachable+authorized${NC}"
        return 0
      fi
    fi
    sleep 0.5
    waited=$((waited + 1))
  done

  echo -e "${RED}[error] $name did not become reachable+authorized within ${max_wait}s${NC}"
  return 1
}

wait_for_remote_env_count() {
  local url="$1" token="$2" expected="$3" max_wait="${4:-60}"
  local waited=0
  while [ "$waited" -lt "$max_wait" ]; do
    local body
    body=$(curl -s "$url" -H "x-bifrost-token: $token" 2>/dev/null) || body=""
    if [ -n "$body" ]; then
      local count
      count=$(printf '%s' "$body" | "$PYTHON_BIN" -c '
import json
import sys

try:
  data = json.load(sys.stdin)
  lst = data["data"]["list"]
  print(len(lst))
except Exception:
  print(-1)
')
      if [ "$count" = "$expected" ]; then
        return 0
      fi
    fi
    sleep 0.5
    waited=$((waited + 1))
  done
  return 1
}

echo -e "${CYAN}╔════════════════════════════════════════════════════════════╗${NC}"
echo -e "${CYAN}║   Bifrost Sync Server E2E Test                            ║${NC}"
echo -e "${CYAN}║   Tests: register → login → sync → verify                ║${NC}"
echo -e "${CYAN}╚════════════════════════════════════════════════════════════╝${NC}"

rm -rf "$SYNC_DATA_DIR" "$PROXY_DATA_DIR"

# ═══════════════════════════════════════════════════════════
# Step 1: Start sync server
# ═══════════════════════════════════════════════════════════
echo ""
echo -e "${CYAN}[step 1/9] Starting Node.js sync server on port $SYNC_PORT...${NC}"
cd "$SYNC_SERVER_DIR"
npx tsx src/cli.ts -p "$SYNC_PORT" -d "$SYNC_DATA_DIR" &
SYNC_PID=$!
wait_for_service "$SYNC_BASE/v4/sso/check" "sync-server"

# ═══════════════════════════════════════════════════════════
# Step 2: Register two users with username/password
# ═══════════════════════════════════════════════════════════
echo ""
echo -e "${CYAN}[step 2/9] Registering users with username/password...${NC}"

REG1_RESP=$(curl -s -X POST "$SYNC_BASE/v4/sso/register" \
  -H "Content-Type: application/json" \
  -d '{"user_id": "alice", "password": "alice123", "nickname": "Alice", "email": "alice@test.local"}')
REG1_CODE=$(echo "$REG1_RESP" | "$PYTHON_BIN" -c "import sys,json; print(json.load(sys.stdin)['code'])")
assert_eq "user alice registered" "0" "$REG1_CODE"

REG2_RESP=$(curl -s -X POST "$SYNC_BASE/v4/sso/register" \
  -H "Content-Type: application/json" \
  -d '{"user_id": "bob", "password": "bob456", "nickname": "Bob", "email": "bob@test.local"}')
REG2_CODE=$(echo "$REG2_RESP" | "$PYTHON_BIN" -c "import sys,json; print(json.load(sys.stdin)['code'])")
assert_eq "user bob registered" "0" "$REG2_CODE"

DUP_RESP=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$SYNC_BASE/v4/sso/register" \
  -H "Content-Type: application/json" \
  -d '{"user_id": "alice", "password": "alice123"}')
assert_eq "duplicate user rejected (409)" "409" "$DUP_RESP"

# ═══════════════════════════════════════════════════════════
# Step 3: Login user alice with password
# ═══════════════════════════════════════════════════════════
echo ""
echo -e "${CYAN}[step 3/9] Logging in with username/password...${NC}"

LOGIN_RESP=$(curl -s -X POST "$SYNC_BASE/v4/sso/login" \
  -H "Content-Type: application/json" \
  -d '{"user_id": "alice", "password": "alice123"}')
LOGIN_CODE=$(echo "$LOGIN_RESP" | "$PYTHON_BIN" -c "import sys,json; print(json.load(sys.stdin)['code'])")
ALICE_TOKEN=$(echo "$LOGIN_RESP" | "$PYTHON_BIN" -c "import sys,json; print(json.load(sys.stdin)['data']['token'])")
assert_eq "login success" "0" "$LOGIN_CODE"
assert_not_empty "alice got token" "$ALICE_TOKEN"

BAD_LOGIN=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$SYNC_BASE/v4/sso/login" \
  -H "Content-Type: application/json" \
  -d '{"user_id": "alice", "password": "wrongpassword"}')
assert_eq "wrong password rejected (401)" "401" "$BAD_LOGIN"

CHECK_RESP=$(curl -s "$SYNC_BASE/v4/sso/check" -H "x-bifrost-token: $ALICE_TOKEN")
CHECK_UID=$(echo "$CHECK_RESP" | "$PYTHON_BIN" -c "import sys,json; print(json.load(sys.stdin)['data']['user_id'])")
assert_eq "check returns alice" "alice" "$CHECK_UID"

INFO_RESP=$(curl -s "$SYNC_BASE/v4/sso/info" -H "x-bifrost-token: $ALICE_TOKEN")
INFO_NICK=$(echo "$INFO_RESP" | "$PYTHON_BIN" -c "import sys,json; print(json.load(sys.stdin)['data']['nickname'])")
assert_eq "info returns Alice nickname" "Alice" "$INFO_NICK"

# ═══════════════════════════════════════════════════════════
# Step 4: Start Bifrost proxy
# ═══════════════════════════════════════════════════════════
echo ""
echo -e "${CYAN}[step 4/9] Starting Bifrost proxy on port $PROXY_PORT...${NC}"
cd "$PROJECT_ROOT"
BIFROST_DATA_DIR="$PROXY_DATA_DIR" cargo run --bin bifrost -- start -p "$PROXY_PORT" --unsafe-ssl --no-system-proxy &
PROXY_PID=$!
wait_for_service "$ADMIN_BASE/api/rules" "bifrost-proxy" 120

# ═══════════════════════════════════════════════════════════
# Step 5: Configure proxy sync
# ═══════════════════════════════════════════════════════════
echo ""
echo -e "${CYAN}[step 5/9] Configuring sync on the proxy...${NC}"

SYNC_CFG_STATUS=$(curl -s -o /dev/null -w "%{http_code}" -X PUT "$ADMIN_BASE/api/sync/config" \
  -H "Content-Type: application/json" \
  -d "{\"enabled\": true, \"auto_sync\": false, \"remote_base_url\": \"$SYNC_BASE\"}")
assert_http_ok "sync config updated" "$SYNC_CFG_STATUS"

SYNC_CFG_RESP=$(curl -s -X PUT "$ADMIN_BASE/api/sync/config" \
  -H "Content-Type: application/json" \
  -d "{\"enabled\": true, \"auto_sync\": false, \"remote_base_url\": \"$SYNC_BASE\"}")
echo "  sync config: $SYNC_CFG_RESP"

SESSION_STATUS=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$ADMIN_BASE/api/sync/session" \
  -H "Content-Type: application/json" \
  -d "{\"token\": \"$ALICE_TOKEN\"}")
assert_http_ok "session token saved" "$SESSION_STATUS"

wait_for_sync_status "$ADMIN_BASE/api/sync/status" "proxy sync status" 90

STATUS_RESP=$(curl -s "$ADMIN_BASE/api/sync/status")
echo "  sync status: $STATUS_RESP"
REACHABLE=$(echo "$STATUS_RESP" | "$PYTHON_BIN" -c "import sys,json; print(json.load(sys.stdin).get('reachable', False))")
AUTHORIZED=$(echo "$STATUS_RESP" | "$PYTHON_BIN" -c "import sys,json; print(json.load(sys.stdin).get('authorized', False))")
assert_eq "remote reachable" "True" "$REACHABLE"
assert_eq "remote authorized" "True" "$AUTHORIZED"

# ═══════════════════════════════════════════════════════════
# Step 6: Create local rules
# ═══════════════════════════════════════════════════════════
echo ""
echo -e "${CYAN}[step 6/9] Creating local rules on the proxy...${NC}"

CR1=$(curl -s -X POST "$ADMIN_BASE/api/rules" \
  -H "Content-Type: application/json" \
  -d "{\"name\": \"e2e-test-rule-1\", \"content\": \"*.example.com host://127.0.0.1:${RULE_TARGET_PORT_1}\", \"enabled\": true}")
echo "  create rule 1: $CR1"
assert_contains "rule 1 created" "$CR1" "created successfully"

CR2=$(curl -s -X POST "$ADMIN_BASE/api/rules" \
  -H "Content-Type: application/json" \
  -d '{"name": "e2e-test-rule-2", "content": "api.dev.local proxy://localhost:8080\napi.staging.local proxy://localhost:9090", "enabled": true}')
echo "  create rule 2: $CR2"
assert_contains "rule 2 created" "$CR2" "created successfully"

# ═══════════════════════════════════════════════════════════
# Step 7: Trigger sync local → remote
# ═══════════════════════════════════════════════════════════
echo ""
echo -e "${CYAN}[step 7/9] Triggering sync (local → remote)...${NC}"

SYNC_RUN_STATUS=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$ADMIN_BASE/api/sync/run")
assert_http_ok "sync run triggered" "$SYNC_RUN_STATUS"

echo "  waiting for sync to complete (remote env count -> 2)..."
REMOTE_ENVS_URL="$SYNC_BASE/v4/env?user_id=alice&offset=0&limit=500"
wait_for_remote_env_count "$REMOTE_ENVS_URL" "$ALICE_TOKEN" "2" 90 || true

SYNC_STATUS2=$(curl -s "$ADMIN_BASE/api/sync/status")
echo "  post-sync status: $SYNC_STATUS2"

# ═══════════════════════════════════════════════════════════
# Step 8: Verify data on remote sync server
# ═══════════════════════════════════════════════════════════
echo ""
echo -e "${CYAN}[step 8/9] Verifying synced data on the remote sync server...${NC}"

REMOTE_ENVS=$(curl -s "$SYNC_BASE/v4/env?user_id=alice&offset=0&limit=500" \
  -H "x-bifrost-token: $ALICE_TOKEN")
REMOTE_COUNT=$(echo "$REMOTE_ENVS" | "$PYTHON_BIN" -c "import sys,json; print(len(json.load(sys.stdin)['data']['list']))")
echo "  remote env count: $REMOTE_COUNT"
assert_eq "remote has 2 envs" "2" "$REMOTE_COUNT"

REMOTE_NAMES=$(echo "$REMOTE_ENVS" | "$PYTHON_BIN" -c "
import sys,json
envs = json.load(sys.stdin)['data']['list']
names = sorted([e['name'] for e in envs])
print(' '.join(names))
")
assert_contains "remote has rule-1" "$REMOTE_NAMES" "e2e-test-rule-1"
assert_contains "remote has rule-2" "$REMOTE_NAMES" "e2e-test-rule-2"

R1_RULE=$(echo "$REMOTE_ENVS" | "$PYTHON_BIN" -c "
import sys,json
envs = json.load(sys.stdin)['data']['list']
for e in envs:
    if e['name'] == 'e2e-test-rule-1':
        print(e['rule'])
        break
")
assert_eq "rule 1 content on remote" "*.example.com host://127.0.0.1:${RULE_TARGET_PORT_1}" "$R1_RULE"

R2_RULE=$(echo "$REMOTE_ENVS" | "$PYTHON_BIN" -c "
import sys,json
envs = json.load(sys.stdin)['data']['list']
for e in envs:
    if e['name'] == 'e2e-test-rule-2':
        print(repr(e['rule']))
        break
")
assert_contains "rule 2 has api.dev.local" "$R2_RULE" "api.dev.local"
assert_contains "rule 2 has api.staging.local" "$R2_RULE" "api.staging.local"

# ═══════════════════════════════════════════════════════════
# Step 9: Verify bidirectional sync (remote → local)
# ═══════════════════════════════════════════════════════════
echo ""
echo -e "${CYAN}[step 9/9] Testing bidirectional sync (remote → local)...${NC}"

R3_CREATE=$(curl -s -X POST "$SYNC_BASE/v4/env" \
  -H "Content-Type: application/json" \
  -H "x-bifrost-token: $ALICE_TOKEN" \
  -d "{\"user_id\": \"alice\", \"name\": \"e2e-remote-rule\", \"rule\": \"remote.example.com host://127.0.0.1:${RULE_TARGET_PORT_2}\"}")
R3_ID=$(echo "$R3_CREATE" | "$PYTHON_BIN" -c "import sys,json; print(json.load(sys.stdin)['data']['id'])")
assert_not_empty "remote rule 3 created" "$R3_ID"

curl -s -X POST "$ADMIN_BASE/api/sync/run" > /dev/null
echo "  waiting for reverse sync (rule appears locally)..."

reverse_waited=0
while [ "$reverse_waited" -lt 120 ]; do
  LOCAL_R3=$(curl -s "$ADMIN_BASE/api/rules/e2e-remote-rule" 2>/dev/null) || LOCAL_R3=""
  if echo "$LOCAL_R3" | "$PYTHON_BIN" -c "import sys,json; json.load(sys.stdin); print('ok')" >/dev/null 2>&1; then
    if echo "$LOCAL_R3" | "$PYTHON_BIN" -c "import sys,json; data=json.load(sys.stdin); print('1' if 'content' in data else '0')" | grep -q '^1$'; then
      break
    fi
  fi
  sleep 0.5
  reverse_waited=$((reverse_waited + 1))
done

LOCAL_R3=$(curl -s "$ADMIN_BASE/api/rules/e2e-remote-rule")
echo "  local rule 3: $LOCAL_R3"

R3_FOUND=$(echo "$LOCAL_R3" | "$PYTHON_BIN" -c "
import sys,json
data = json.load(sys.stdin)
print('found' if 'content' in data else 'missing')
")
assert_eq "remote rule pulled to local" "found" "$R3_FOUND"

R3_CONTENT=$(echo "$LOCAL_R3" | "$PYTHON_BIN" -c "import sys,json; print(json.load(sys.stdin)['content'])")
assert_eq "pulled rule content matches" "remote.example.com host://127.0.0.1:${RULE_TARGET_PORT_2}" "$R3_CONTENT"

# ═══════════════════════════════════════════════════════════
# Step 10: Logout verification
# ═══════════════════════════════════════════════════════════
echo ""
echo -e "${CYAN}[bonus] Testing logout...${NC}"
LOGOUT_RESP=$(curl -s "$SYNC_BASE/v4/sso/logout" -H "x-bifrost-token: $ALICE_TOKEN")
assert_contains "logout success" "$LOGOUT_RESP" '"code":0'

POST_LOGOUT_CHECK=$(curl -s -o /dev/null -w "%{http_code}" "$SYNC_BASE/v4/sso/check" -H "x-bifrost-token: $ALICE_TOKEN")
assert_eq "token invalid after logout (401)" "401" "$POST_LOGOUT_CHECK"

echo ""
echo -e "${GREEN}╔════════════════════════════════════════════════════════════╗${NC}"
echo -e "${GREEN}║   ALL E2E TESTS PASSED ✓                                 ║${NC}"
echo -e "${GREEN}╠════════════════════════════════════════════════════════════╣${NC}"
echo -e "${GREEN}║   ✓ Multi-user registration with password                ║${NC}"
echo -e "${GREEN}║   ✓ Username/password login + token auth                 ║${NC}"
echo -e "${GREEN}║   ✓ Duplicate user rejection                             ║${NC}"
echo -e "${GREEN}║   ✓ Wrong password rejection                             ║${NC}"
echo -e "${GREEN}║   ✓ Proxy sync config + session                          ║${NC}"
echo -e "${GREEN}║   ✓ Local rules → remote sync                            ║${NC}"
echo -e "${GREEN}║   ✓ Remote rules → local sync                            ║${NC}"
echo -e "${GREEN}║   ✓ Logout + token invalidation                          ║${NC}"
echo -e "${GREEN}╚════════════════════════════════════════════════════════════╝${NC}"
