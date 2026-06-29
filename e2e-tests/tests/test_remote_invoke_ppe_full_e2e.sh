#!/usr/bin/env bash
set -euo pipefail

# Full Remote Invoke regression against the deployed ByteDance relay.
#
# This intentionally uses two local Bifrost processes built from the current
# checkout: one target daemon and two isolated caller data dirs. The relay is
# always remote (default: https://bifrost.bytedance.net), never a local relay.
# Set BIFROST_REMOTE_RELAY_HEADERS only when a pre-release PPE route must be
# exercised, for example: x-tt-env=ppe_ticket_system,x-use-ppe=1.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
REPO="$(cd "$SCRIPT_DIR/../.." && pwd -P)"
RELAY_URL="${BIFROST_REMOTE_RELAY_URL:-https://bifrost.bytedance.net}"
REMOTE_RELAY_HEADERS="${BIFROST_REMOTE_RELAY_HEADERS-}"
SYNC_STATE_FILE="${BIFROST_SYNC_STATE_FILE:-$HOME/.bifrost/sync-state.json}"
BIFROST_BIN="${BIFROST_BIN:-$REPO/target/debug/bifrost}"
KEEP_TMP="${KEEP_TMP:-0}"
BIFROST_SERVER_V4_DIR="${BIFROST_SERVER_V4_DIR:-$REPO/bifrost-server-v4}"
RUN_LOCAL_CASES="${RUN_LOCAL_CASES:-1}"
RUN_SERVER_V4_CASES="${RUN_SERVER_V4_CASES:-1}"
RUN_REMOTE_RELAY_CASES="${RUN_REMOTE_RELAY_CASES:-1}"
FAILED=0

if [[ -n "$REMOTE_RELAY_HEADERS" ]]; then
  export BIFROST_REMOTE_RELAY_HEADERS="$REMOTE_RELAY_HEADERS"
else
  unset BIFROST_REMOTE_RELAY_HEADERS
fi
export BIFROST_DISABLE_TRAY=1
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY all_proxy ALL_PROXY no_proxy NO_PROXY

if [[ "${CI:-}" == "true" || "${GITHUB_ACTIONS:-}" == "true" ]]; then
  echo "SKIP: remote relay full regression requires local/internal network access and is not supported in CI."
  exit 0
fi

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "missing required command: $1" >&2
    exit 1
  }
}

require_cmd curl
require_cmd jq
require_cmd python3
require_cmd shasum

pass() {
  echo "PASS $*"
  if [[ -n "${RESULTS:-}" ]]; then
    echo "PASS $*" >>"$RESULTS"
  fi
}

fail() {
  FAILED=1
  echo "FAIL $*" >&2
  if [[ -n "${RESULTS:-}" ]]; then
    echo "FAIL $*" >>"$RESULTS"
    echo "DIAG preserved failed temp dir: ${TMP_ROOT:-<not-created>}" | tee -a "$RESULTS" >&2
  else
    echo "DIAG preserved failed temp dir: ${TMP_ROOT:-<not-created>}" >&2
  fi
  exit 1
}

run_cmd() {
  echo "+ $*" >&2
  "$@"
}

run_local_case_suites() {
  if [[ "$RUN_LOCAL_CASES" != "1" ]]; then
    pass "local remote invoke case suites skipped RUN_LOCAL_CASES=$RUN_LOCAL_CASES"
    return 0
  fi

  require_cmd cargo
  require_cmd pnpm

  local sync_server_dir="$REPO/packages/bifrost-sync-server"
  local vitest_cases=(
    src/__tests__/p0-hardening.test.ts
    src/__tests__/remote-invoke-security.test.ts
    src/__tests__/remote-invoke-relay-v2-phase1.test.ts
    src/__tests__/remote-invoke-pairing-timeout.test.ts
    src/__tests__/sse-multi-watcher.test.ts
    src/__tests__/remote-invoke-sse.test.ts
    src/__tests__/remote-invoke-stream-frame.test.ts
    src/__tests__/grants-claim.test.ts
    src/__tests__/grants-lookup.test.ts
    src/__tests__/grants-revoke.test.ts
    src/__tests__/pop.test.ts
  )

  run_cmd pnpm --dir "$sync_server_dir" exec vitest run "${vitest_cases[@]}"
  pass "sync-server remote invoke vitest suites"

  run_cmd cargo test -p bifrost-cli remote -- --nocapture
  pass "bifrost-cli remote invoke unit suites"
}

run_server_v4_case_suites() {
  if [[ "$RUN_SERVER_V4_CASES" != "1" ]]; then
    pass "server-v4 remote invoke case suites skipped RUN_SERVER_V4_CASES=$RUN_SERVER_V4_CASES"
    return 0
  fi

  if [[ ! -f "$BIFROST_SERVER_V4_DIR/package.json" ]]; then
    fail "missing bifrost-server-v4 checkout at $BIFROST_SERVER_V4_DIR; set RUN_SERVER_V4_CASES=0 to skip or BIFROST_SERVER_V4_DIR to override"
  fi

  require_cmd pnpm
  run_cmd pnpm --dir "$BIFROST_SERVER_V4_DIR" run build
  run_cmd pnpm --dir "$BIFROST_SERVER_V4_DIR" run test:remote-invoke-hardening
  pass "bifrost-server-v4 remote invoke hardening suites"
}

relay_header_args() {
  if [[ -z "$REMOTE_RELAY_HEADERS" ]]; then
    return 0
  fi
  python3 - "$REMOTE_RELAY_HEADERS" <<'PY'
import sys
for item in sys.argv[1].split(","):
    item = item.strip()
    if not item:
        continue
    if ":" in item:
        name, value = item.split(":", 1)
    elif "=" in item:
        name, value = item.split("=", 1)
    else:
        raise SystemExit(f"invalid relay header: {item}")
    name = name.strip()
    value = value.strip()
    if not name:
        raise SystemExit(f"invalid relay header: {item}")
    print("-H")
    print(f"{name}: {value}")
PY
}

if [[ "$RUN_LOCAL_CASES" == "1" || "$RUN_SERVER_V4_CASES" == "1" ]]; then
  pass "one-click regression phases local=$RUN_LOCAL_CASES server_v4=$RUN_SERVER_V4_CASES remote_relay=$RUN_REMOTE_RELAY_CASES"
fi
run_local_case_suites
run_server_v4_case_suites

if [[ "$RUN_REMOTE_RELAY_CASES" != "1" ]]; then
  pass "remote relay full regression skipped RUN_REMOTE_RELAY_CASES=$RUN_REMOTE_RELAY_CASES"
  exit 0
fi

if [[ "${SKIP_BUILD:-false}" != "true" ]]; then
  cargo build --manifest-path "$REPO/Cargo.toml" --bin bifrost
fi
if [[ ! -x "$BIFROST_BIN" ]]; then
  echo "bifrost binary not executable: $BIFROST_BIN" >&2
  exit 1
fi

SYNC_TOKEN="${BIFROST_SYNC_TOKEN:-}"
if [[ -z "$SYNC_TOKEN" ]]; then
  if [[ ! -f "$SYNC_STATE_FILE" ]]; then
    echo "missing sync state file: $SYNC_STATE_FILE" >&2
    exit 1
  fi
  SYNC_TOKEN="$(jq -r '.token // empty' "$SYNC_STATE_FILE")"
fi
if [[ -z "$SYNC_TOKEN" ]]; then
  echo "missing sync token in $SYNC_STATE_FILE" >&2
  exit 1
fi

free_port() {
  python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

TMP_ROOT="$(mktemp -d /tmp/bifrost-relay-full.XXXXXX)"
TMP_ROOT_REAL="$(cd "$TMP_ROOT" && pwd -P)"
FILE_ROOT="$TMP_ROOT_REAL/remote-files"
TARGET_DATA="$TMP_ROOT/target"
CALLER_CODE_DATA="$TMP_ROOT/caller-code"
CALLER_POWER_DATA="$TMP_ROOT/caller-power"
CALLER_SSH_DATA="$TMP_ROOT/caller-ssh"
TARGET_PORT="$(free_port)"
MOCK_PORT="$(free_port)"
PYTHON_BIN="$(command -v python3)"
ADMIN="http://127.0.0.1:${TARGET_PORT}/_bifrost"
TARGET_PID=""
MOCK_PID=""
TARGET_LOG="$TMP_ROOT/target.log"
MOCK_LOG="$TMP_ROOT/mock.log"
RESULTS="$TMP_ROOT/results.log"

mkdir -p "$TARGET_DATA" "$CALLER_CODE_DATA" "$CALLER_POWER_DATA" "$CALLER_SSH_DATA" "$FILE_ROOT"

json() { jq -c . 2>/dev/null || cat; }
http_post() {
  local path="$1" data="$2"
  curl -sS --max-time 20 -H 'Content-Type: application/json' \
    -X POST "$ADMIN$path" --data-binary "$data"
}
http_get() {
  local path="$1"
  curl -sS --max-time 20 "$ADMIN$path"
}
cleanup() {
  set +e
  [[ -n "$TARGET_PID" ]] && kill "$TARGET_PID" 2>/dev/null || true
  [[ -n "$MOCK_PID" ]] && kill "$MOCK_PID" 2>/dev/null || true
  pkill -f "bifrost __tray --data-dir $TARGET_DATA" 2>/dev/null || true
  if [[ "$KEEP_TMP" != "1" && "$FAILED" != "1" ]]; then
    rm -rf "$TMP_ROOT"
  else
    echo "preserved $TMP_ROOT"
  fi
}
trap cleanup EXIT

if [[ -n "$REMOTE_RELAY_HEADERS" ]]; then
  pass "relay mode=PPE relay_url=$RELAY_URL headers=$REMOTE_RELAY_HEADERS"
else
  pass "relay mode=default relay_url=$RELAY_URL headers=<none>; set BIFROST_REMOTE_RELAY_HEADERS='x-tt-env=ppe_ticket_system,x-use-ppe=1' for PPE"
fi

cat >"$TARGET_DATA/config.toml" <<CFG
[sync]
enabled = true
auto_sync = true
remote_base_url = "$RELAY_URL"
probe_interval_secs = 5
connect_timeout_ms = 5000
CFG

cat >"$TARGET_DATA/file-access.toml" <<CFG
[default]
roots = ["$FILE_ROOT"]
ops = ["read", "read_many", "list", "stat", "glob", "search", "hash", "outline", "write", "edit", "mkdir", "move", "delete", "apply_patch"]
allow_overwrite = true
allow_recursive_delete = true
CFG

python3 - "$MOCK_PORT" >"$MOCK_LOG" 2>&1 <<'PY' &
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

port = int(sys.argv[1])

class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        body = json.dumps({
            "ok": True,
            "method": "GET",
            "path": self.path,
            "marker": "PPE_REMOTE_TRAFFIC_MARKER",
        }).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("x-ppe-echo", "remote")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        data = self.rfile.read(int(self.headers.get("content-length", "0") or 0)).decode()
        body = json.dumps({
            "ok": True,
            "method": "POST",
            "path": self.path,
            "body": data,
            "marker": "PPE_REMOTE_BODY_MARKER",
        }).encode()
        self.send_response(201)
        self.send_header("content-type", "application/json")
        self.send_header("x-ppe-echo", "remote-post")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_args):
        pass

HTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
MOCK_PID=$!

BIFROST_DATA_DIR="$TARGET_DATA" "$BIFROST_BIN" -H 127.0.0.1 -p "$TARGET_PORT" \
  --log-output file start --daemon --no-system-proxy --no-tray \
  --skip-cert-check --unsafe-ssl >"$TARGET_LOG" 2>&1

for _ in {1..120}; do
  if curl -fsS --max-time 2 "$ADMIN/api/auth/status" >/dev/null 2>&1; then
    break
  fi
  sleep 0.5
done
curl -fsS --max-time 2 "$ADMIN/api/auth/status" >/dev/null \
  || fail "target admin did not start; log=$(tail -80 "$TARGET_LOG")"
TARGET_PID="$(jq -r '.pid // empty' "$TARGET_DATA/runtime.json" 2>/dev/null || true)"
pass "target started port=$TARGET_PORT pid=$TARGET_PID data=$TARGET_DATA"

http_post "/api/sync/session" "{\"token\":\"$SYNC_TOKEN\"}" >"$TMP_ROOT/sync-save.json"
for _ in {1..90}; do
  status="$(http_get /api/remote-invoke/status || true)"
  state="$(jq -r '.state // empty' <<<"$status" 2>/dev/null || true)"
  [[ "$state" == "Connected" ]] && break
  sleep 1
done
[[ "$(http_get /api/remote-invoke/status | jq -r '.state // empty')" == "Connected" ]] \
  || fail "target worker not connected to relay: $(http_get /api/remote-invoke/status | json)"
CLIENT_ID="$(http_get /api/remote-invoke/identity | jq -r '.instance_id // empty')"
[[ -n "$CLIENT_ID" ]] || fail "missing target client id"
pass "target registered with relay client_id=$CLIENT_ID"

BIFROST_DATA_DIR="$TARGET_DATA" "$BIFROST_BIN" setting shell policy add \
  --id ppe-shell \
  --name PPE \
  --mode shell_text \
  --pattern '^(?s:.*)$' \
  --shell /bin/zsh \
  --cwd "$FILE_ROOT" \
  --env PPE_REMOTE_ENV \
  --stdin \
  --timeout-ms 60000 >/dev/null

BIFROST_DATA_DIR="$TARGET_DATA" "$BIFROST_BIN" setting shell policy add \
  --id ppe-run \
  --name "PPE Run" \
  --mode argv_exec \
  --program "$PYTHON_BIN" \
  --cwd "$FILE_ROOT" \
  --env PPE_REMOTE_ENV \
  --timeout-ms 60000 >/dev/null
pass "target shell policy configured"

curl -sS --max-time 10 -x "http://127.0.0.1:$TARGET_PORT" \
  "http://127.0.0.1:$MOCK_PORT/ppe-get?marker=PPE_REMOTE_TRAFFIC_MARKER" >/dev/null
curl -sS --max-time 10 -x "http://127.0.0.1:$TARGET_PORT" \
  -H 'content-type: application/json' \
  -d '{"marker":"PPE_REMOTE_BODY_MARKER"}' \
  "http://127.0.0.1:$MOCK_PORT/ppe-post" >/dev/null
sleep 1
pass "target traffic generated via proxy"

connect_code() {
  local out="$TMP_ROOT/code-connect.out"
  local pair_json pair_code pairing cp
  pair_json="$(http_post /api/remote-invoke/discovery/enter '{}')"
  pair_code="$(jq -r '.session.pair_code // empty' <<<"$pair_json")"
  [[ -n "$pair_code" ]] || fail "pair code empty: $pair_json"

  BIFROST_DATA_DIR="$CALLER_CODE_DATA" "$BIFROST_BIN" remote conn up "$pair_code" \
    --relay-url "$RELAY_URL" >"$out" 2>&1 &
  cp=$!

  pairing=""
  for _ in {1..60}; do
    pairing="$(http_get /api/remote-invoke/pairings/pending \
      | jq -r '.pairings[0].pairing_id // empty' 2>/dev/null || true)"
    [[ -n "$pairing" ]] && break
    sleep 1
  done
  [[ -n "$pairing" ]] || fail "no pending pairing for code; caller=$(cat "$out")"

  for _ in {1..30}; do
    grep -q 'Waiting for approval' "$out" && break
    sleep 1
  done
  grep -q 'Waiting for approval' "$out" || fail "caller did not enter pairing watch: $(cat "$out")"
  sleep 2

  http_post "/api/remote-invoke/pairings/$pairing/approve" \
    '{"grant_mode":"permanent","grant_scope":"remote_shell_exec","file_access":"read_write","policy_binding":{"mode":"selected","policy_ids":["ppe-shell","ppe-run"]},"stdin_allowed":true}' \
    >"$TMP_ROOT/code-approve.json"
  wait "$cp" || fail "code remote conn up failed: $(cat "$out")"
  grep -q 'Connected' "$out" || fail "code connect output missing Connected: $(cat "$out")"

  local grant_id
  grant_id="$(http_get /api/remote-invoke/grants | jq -r '.grants[0].grant_id // .grants[0].id // empty')"
  [[ -n "$grant_id" ]] || fail "code grant missing"
  http_post /api/remote-invoke/discovery/exit '{}' >/dev/null 2>&1 || true
  pass "code authorization connected grant=$grant_id"
}

connect_code_power() {
  local out="$TMP_ROOT/code-power-connect.out"
  local pair_json pair_code pairing cp grant_id
  pair_json="$(http_post /api/remote-invoke/discovery/enter '{}')"
  pair_code="$(jq -r '.session.pair_code // empty' <<<"$pair_json")"
  [[ -n "$pair_code" ]] || fail "power pair code empty: $pair_json"

  BIFROST_DATA_DIR="$CALLER_POWER_DATA" "$BIFROST_BIN" remote conn up "$pair_code" \
    --relay-url "$RELAY_URL" >"$out" 2>&1 &
  cp=$!

  pairing=""
  for _ in {1..60}; do
    pairing="$(http_get /api/remote-invoke/pairings/pending \
      | jq -r '.pairings[0].pairing_id // empty' 2>/dev/null || true)"
    [[ -n "$pairing" ]] && break
    sleep 1
  done
  [[ -n "$pairing" ]] || fail "no pending pairing for power grant; caller=$(cat "$out")"

  for _ in {1..30}; do
    grep -q 'Waiting for approval' "$out" && break
    sleep 1
  done
  grep -q 'Waiting for approval' "$out" || fail "power caller did not enter pairing watch: $(cat "$out")"
  sleep 2

  http_post "/api/remote-invoke/pairings/$pairing/approve" \
    '{"grant_mode":"permanent","grant_scope":"remote_power_mgmt","file_access":"none","policy_binding":{"mode":"selected","policy_ids":["ppe-shell"]},"stdin_allowed":false}' \
    >"$TMP_ROOT/code-power-approve.json"
  wait "$cp" || fail "code power remote conn up failed: $(cat "$out")"
  grep -q 'Connected' "$out" || fail "code power connect output missing Connected: $(cat "$out")"

  grant_id="$(http_get /api/remote-invoke/grants | jq -r '.grants[0].grant_id // .grants[0].id // empty')"
  [[ -n "$grant_id" ]] || fail "code power grant missing"
  http_post /api/remote-invoke/discovery/exit '{}' >/dev/null 2>&1 || true
  pass "code power authorization connected grant=$grant_id"
}

run_keep_awake_matrix() {
  local caller_data="$1" label="$2"
  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" \
    keep-awake status >"$TMP_ROOT/$label-keep-awake-status.json"
  jq -e 'type == "object"' "$TMP_ROOT/$label-keep-awake-status.json" >/dev/null

  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" \
    keep-awake on >"$TMP_ROOT/$label-keep-awake-on.json"
  jq -e 'type == "object"' "$TMP_ROOT/$label-keep-awake-on.json" >/dev/null

  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" \
    keep-awake mode get >"$TMP_ROOT/$label-keep-awake-mode-get.json"
  jq -e 'type == "object"' "$TMP_ROOT/$label-keep-awake-mode-get.json" >/dev/null

  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" \
    keep-awake mode set off >"$TMP_ROOT/$label-keep-awake-mode-set-off.json"
  jq -e 'type == "object"' "$TMP_ROOT/$label-keep-awake-mode-set-off.json" >/dev/null

  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" \
    keep-awake off >"$TMP_ROOT/$label-keep-awake-off.json"
  jq -e 'type == "object"' "$TMP_ROOT/$label-keep-awake-off.json" >/dev/null
  pass "$label remote keep-awake all subcommands"
}

run_remote_matrix() {
  local caller_data="$1" label="$2"
  local scratch="$FILE_ROOT/${label}-scratch"
  mkdir -p "$scratch"
  echo 'hello ppe remote' >"$scratch/hello.txt"
  printf 'alpha\nbeta\ngamma\n' >"$scratch/multi.txt"
  printf 'fn main() {}\nstruct Demo;\n' >"$scratch/main.rs"

  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" \
    conn status --client-id "${CLIENT_ID:0:12}" >"$TMP_ROOT/$label-status.json"
  jq -e '.client_instance_id == "'"$CLIENT_ID"'" or .version' "$TMP_ROOT/$label-status.json" >/dev/null
  pass "$label remote conn status"

  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" \
    traffic list --limit 20 --format json >"$TMP_ROOT/$label-traffic-list.json"
  jq -e '.records | length >= 1' "$TMP_ROOT/$label-traffic-list.json" >/dev/null
  local traffic_id
  traffic_id="$(jq -r '.records[0].id // empty' "$TMP_ROOT/$label-traffic-list.json")"
  [[ -n "$traffic_id" ]] || fail "$label traffic id missing"
  pass "$label remote traffic list"

  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" \
    traffic get "$traffic_id" --format json >"$TMP_ROOT/$label-traffic-get.json"
  jq -e '.' "$TMP_ROOT/$label-traffic-get.json" >/dev/null
  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" \
    traffic search PPE_REMOTE --format json --max-results 10 >"$TMP_ROOT/$label-search.json"
  jq -e '(.total // .total_matched // 0) >= 1 or ((.records // .results // []) | length >= 1)' \
    "$TMP_ROOT/$label-search.json" >/dev/null
  pass "$label remote traffic get/search"

  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" \
    file read "$scratch/hello.txt" --cwd "$FILE_ROOT" --output json >"$TMP_ROOT/$label-file-read.json"
  jq -e '.content_b64' "$TMP_ROOT/$label-file-read.json" >/dev/null
  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" \
    file read-many --path "$scratch/hello.txt" --path "$scratch/multi.txt" --cwd "$FILE_ROOT" --output json >/dev/null
  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" \
    file list "$scratch" --depth 2 --cwd "$FILE_ROOT" --output json >/dev/null
  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" \
    file stat "$scratch/hello.txt" --cwd "$FILE_ROOT" --output json >/dev/null
  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" \
    file glob "$scratch/*.txt" --cwd "$FILE_ROOT" --output json >/dev/null
  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" \
    file find hello --path "$scratch" --cwd "$FILE_ROOT" --output json >/dev/null
  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" \
    file hash "$scratch/hello.txt" --cwd "$FILE_ROOT" --output json >/dev/null
  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" \
    file outline "$scratch/main.rs" --cwd "$FILE_ROOT" --output json >/dev/null
  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" \
    file scratch-dir --cwd "$scratch" --name ppe --output json >/dev/null
  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" \
    file write "$scratch/write.txt" --content "write-$label" --allow-overwrite true --cwd "$FILE_ROOT" --output json >/dev/null
  printf 'local-write-%s\n' "$label" >"$TMP_ROOT/$label-local-write.txt"
  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" \
    file write "$scratch/write-local.txt" --from-local "$TMP_ROOT/$label-local-write.txt" \
    --allow-overwrite true --cwd "$FILE_ROOT" --output json >/dev/null
  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" \
    file edit "$scratch/write.txt" --edits '[{"start_line":1,"end_line":1,"replacement":"edit-'"$label"'\n"}]' --cwd "$FILE_ROOT" --output json >/dev/null
  printf '[{"start_line":1,"end_line":1,"replacement":"edit-local-%s\\n"}]\n' "$label" >"$TMP_ROOT/$label-local-edits.json"
  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" \
    file edit "$scratch/write-local.txt" --from-local "$TMP_ROOT/$label-local-edits.json" \
    --cwd "$FILE_ROOT" --output json >/dev/null

  cat >"$TMP_ROOT/$label.patch" <<PATCH
*** Begin Patch
*** Update File: $scratch/write.txt
@@
-edit-$label
+patch-$label
*** End Patch
PATCH
  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" \
    file patch --from-local "$TMP_ROOT/$label.patch" --cwd "$FILE_ROOT" --output json >/dev/null
  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" \
    file mkdir "$scratch/mkdir/a" --parents --cwd "$FILE_ROOT" --output json >/dev/null
  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" \
    file move "$scratch/write.txt" "$scratch/moved.txt" --cwd "$FILE_ROOT" --output json >/dev/null
  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" \
    file delete "$scratch/moved.txt" --cwd "$FILE_ROOT" --output json >/dev/null
  pass "$label remote file all subcommands"

  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" \
    exec --shell-text "printf ${label}-exec-ok" >"$TMP_ROOT/$label-exec.out" 2>&1
  grep -q "${label}-exec-ok" "$TMP_ROOT/$label-exec.out"

  cat >"$TMP_ROOT/$label-run.py" <<PY
import os
import sys
print("${label}-run-ok:{}:{}".format(sys.argv[1], os.environ.get("PPE_REMOTE_ENV", "")))
PY
  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" \
    run --script-file "$TMP_ROOT/$label-run.py" --interpreter "$PYTHON_BIN" --cwd "$FILE_ROOT" \
    --env PPE_REMOTE_ENV=ok -- arg1 >"$TMP_ROOT/$label-run.out" 2>&1
  grep -q "${label}-run-ok:arg1:ok" "$TMP_ROOT/$label-run.out"

  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" \
    exec --detach --shell-text "printf ${label}-job-begin; sleep 1; printf ${label}-job-end" \
    >"$TMP_ROOT/$label-detach.out" 2>&1
  local call_id
  call_id="$(grep -Eo '[A-Za-z0-9_-]{16,}' "$TMP_ROOT/$label-detach.out" | tail -1)"
  [[ -n "$call_id" ]] || fail "$label detach call id missing: $(cat "$TMP_ROOT/$label-detach.out")"
  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote job watch "$call_id" \
    --no-verify-digest --output-file "$TMP_ROOT/$label-job.log" >"$TMP_ROOT/$label-job-watch.out" 2>&1
  grep -q "${label}-job-end" "$TMP_ROOT/$label-job.log"
  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote job list >/dev/null
  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote job status "$call_id" --wait-ms 1000 >/dev/null
  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote job logs "$call_id" \
    --no-verify-digest --output-file "$TMP_ROOT/$label-job-logs.log" >"$TMP_ROOT/$label-job-logs.out" 2>&1 &
  local logs_pid=$!
  sleep 0.5
  if kill -0 "$logs_pid" 2>/dev/null; then
    kill "$logs_pid" 2>/dev/null || true
    wait "$logs_pid" 2>/dev/null || true
  else
    wait "$logs_pid" || fail "$label remote job logs exited early: $(cat "$TMP_ROOT/$label-job-logs.out")"
  fi

  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" \
    exec --detach --shell-text "printf ${label}-long-begin; sleep 1; printf ${label}-long-middle; sleep 1; printf ${label}-long-end" \
    >"$TMP_ROOT/$label-long-detach.out" 2>&1
  local long_call_id
  long_call_id="$(grep -Eo '[A-Za-z0-9_-]{16,}' "$TMP_ROOT/$label-long-detach.out" | tail -1)"
  [[ -n "$long_call_id" ]] || fail "$label long detach call id missing: $(cat "$TMP_ROOT/$label-long-detach.out")"
  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote job logs "$long_call_id" \
    --no-verify-digest --output-file "$TMP_ROOT/$label-long-job-logs.log" >"$TMP_ROOT/$label-long-job-logs.out" 2>&1
  grep -q "${label}-long-begin" "$TMP_ROOT/$label-long-job-logs.log"
  grep -q "${label}-long-middle" "$TMP_ROOT/$label-long-job-logs.log"
  grep -q "${label}-long-end" "$TMP_ROOT/$label-long-job-logs.log"
  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote job status "$long_call_id" --wait-ms 1000 \
    >"$TMP_ROOT/$label-long-job-status.out" 2>&1
  grep -Eq 'status=(exited|completed)' "$TMP_ROOT/$label-long-job-status.out"

  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" \
    run --detach --script-file "$TMP_ROOT/$label-run.py" --interpreter "$PYTHON_BIN" --cwd "$FILE_ROOT" \
    --env PPE_REMOTE_ENV=detached -- arg2 >"$TMP_ROOT/$label-run-detach.out" 2>&1
  local run_call_id
  run_call_id="$(grep -Eo '[A-Za-z0-9_-]{16,}' "$TMP_ROOT/$label-run-detach.out" | tail -1)"
  [[ -n "$run_call_id" ]] || fail "$label run detach call id missing: $(cat "$TMP_ROOT/$label-run-detach.out")"
  BIFROST_DATA_DIR="$caller_data" "$BIFROST_BIN" remote job watch "$run_call_id" \
    --no-verify-digest --output-file "$TMP_ROOT/$label-run-detach.log" >"$TMP_ROOT/$label-run-detach-watch.out" 2>&1
  grep -q "${label}-run-ok:arg2:detached" "$TMP_ROOT/$label-run-detach.log"
  pass "$label remote exec/run/job"
}

connect_code
run_remote_matrix "$CALLER_CODE_DATA" code
connect_code_power
run_keep_awake_matrix "$CALLER_POWER_DATA" code-power

KEY_PATH="$TMP_ROOT/ppe-ssh.bifrost"
BIFROST_DATA_DIR="$TARGET_DATA" "$BIFROST_BIN" setting ssh-key create \
  --label PPE --grant-mode permanent --output "$KEY_PATH" >"$TMP_ROOT/ssh-create.out" 2>&1
DEVICE_CODE="$(grep -Eo 'BF-[0-9A-F]{16}' "$TMP_ROOT/ssh-create.out" | head -1)"
[[ -n "$DEVICE_CODE" ]] || DEVICE_CODE="$(grep -Eo 'Device-Code: BF-[0-9A-F]{16}' "$KEY_PATH" | awk '{print $2}')"
[[ -n "$DEVICE_CODE" ]] || fail "missing SSH device code"

code=""
relay_headers=()
while IFS= read -r header_arg; do
  relay_headers+=("$header_arg")
done < <(relay_header_args)
for _ in {1..60}; do
  code="$(curl -sS -o "$TMP_ROOT/ssh-challenge.json" -w '%{http_code}' --max-time 10 \
    -H 'Content-Type: application/json' \
    "${relay_headers[@]}" \
    -X POST "$RELAY_URL/v4/remote-invoke/ssh/challenge" \
    -d "{\"device_code\":\"$DEVICE_CODE\"}" || true)"
  [[ "$code" == "200" ]] && break
  sleep 1
done
[[ "$code" == "200" ]] || fail "ssh challenge not visible on relay for $DEVICE_CODE: $(cat "$TMP_ROOT/ssh-challenge.json" 2>/dev/null || true)"

BIFROST_DATA_DIR="$CALLER_SSH_DATA" "$BIFROST_BIN" remote conn up \
  --ssh-key "$KEY_PATH" --relay-url "$RELAY_URL" >"$TMP_ROOT/ssh-connect.out" 2>&1 \
  || fail "ssh connect failed: $(cat "$TMP_ROOT/ssh-connect.out")"
grep -q 'Connected with SSH key' "$TMP_ROOT/ssh-connect.out" \
  || fail "ssh connect missing success: $(cat "$TMP_ROOT/ssh-connect.out")"
pass "ssh-key authorization connected via relay"
run_remote_matrix "$CALLER_SSH_DATA" ssh
run_keep_awake_matrix "$CALLER_SSH_DATA" ssh

BIFROST_DATA_DIR="$CALLER_CODE_DATA" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" conn down --all >/dev/null 2>&1 || true
BIFROST_DATA_DIR="$CALLER_POWER_DATA" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" conn down --all >/dev/null 2>&1 || true
BIFROST_DATA_DIR="$CALLER_SSH_DATA" "$BIFROST_BIN" remote --relay-url "$RELAY_URL" conn down --all >/dev/null 2>&1 || true
pass "remote conn down cleanup"

printf '\nSUMMARY\n'
cat "$RESULTS"
GIT_STATUS_SHORT="$(git -C "$REPO" status --short)"
if [[ -n "$GIT_STATUS_SHORT" ]]; then
  GIT_DIRTY=true
else
  GIT_DIRTY=false
fi
printf 'TMP_ROOT=%s\nTARGET_PORT=%s\nCLIENT_ID=%s\nBIN=%s\nSHA256=%s\nHEAD=%s\nGIT_DIRTY=%s\n' \
  "$TMP_ROOT" \
  "$TARGET_PORT" \
  "$CLIENT_ID" \
  "$BIFROST_BIN" \
  "$(shasum -a 256 "$BIFROST_BIN" | awk '{print $1}')" \
  "$(git -C "$REPO" rev-parse HEAD)" \
  "$GIT_DIRTY"
