#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
source "$ROOT_DIR/e2e-tests/test_utils/process.sh"

fail() {
  echo "[cli-resource-live-sync] FAIL: $*" >&2
  if [[ -n "${DATA_DIR:-}" && -f "$DATA_DIR/bifrost.log" ]]; then
    tail -100 "$DATA_DIR/bifrost.log" >&2 || true
  fi
  exit 1
}

wait_for_file() {
  local path="$1"
  for _ in $(seq 1 200); do
    [[ -f "$path" ]] && return 0
    sleep 0.05
  done
  fail "timed out waiting for $path"
}

wait_for_pid() {
  local pid="$1"
  if ! wait "$pid"; then
    [[ -f "$DATA_DIR/probe-output.json" ]] && cat "$DATA_DIR/probe-output.json" >&2
    fail "push probe $pid failed"
  fi
}

allocate_port() {
  python3 - <<'PY'
import socket
sock = socket.socket()
sock.bind(("127.0.0.1", 0))
print(sock.getsockname()[1])
sock.close()
PY
}

DATA_DIR="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-cli-resource-sync.XXXXXX")"
mark_e2e_data_root "$DATA_DIR"
PROXY_PID=""
PROBE_PID=""
PROXY_PORT="${PROXY_PORT:-$(allocate_port)}"
ADMIN_BASE="http://127.0.0.1:${PROXY_PORT}/_bifrost/api"
PUSH_URL="ws://127.0.0.1:${PROXY_PORT}/_bifrost/api/push?x_client_id=cli-resource-e2e-$$"
BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/release/bifrost}"
if [[ ! -x "$BIFROST_BIN" && -x "${BIFROST_BIN}.exe" ]]; then
  BIFROST_BIN="${BIFROST_BIN}.exe"
fi

cleanup() {
  if [[ -n "${PROBE_PID:-}" ]]; then
    kill_pid "$PROBE_PID"
    wait_pid "$PROBE_PID"
  fi
  if [[ -n "${PROXY_PID:-}" ]]; then
    safe_cleanup_proxy "$PROXY_PID" || true
  fi
  if [[ -d "${DATA_DIR:-}" ]] && is_e2e_data_root "$DATA_DIR"; then
    rm -rf "$DATA_DIR"
  fi
}
trap cleanup EXIT

cd "$ROOT_DIR"

if [[ "${SKIP_BUILD:-false}" != "true" && "${SKIP_BUILD:-0}" != "1" ]]; then
  cargo build --release --bin bifrost
fi
[[ -x "$BIFROST_BIN" ]] || fail "missing Bifrost binary: $BIFROST_BIN"
command -v node >/dev/null 2>&1 || fail "node is required"
command -v jq >/dev/null 2>&1 || fail "jq is required"

echo "[cli-resource-live-sync] starting isolated daemon on ${PROXY_PORT}"
BIFROST_DATA_DIR="$DATA_DIR" \
  BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
  BIFROST_DISABLE_TRAY=1 \
  "$BIFROST_BIN" -p "$PROXY_PORT" start \
    --unsafe-ssl --skip-cert-check --no-system-proxy \
    >"$DATA_DIR/bifrost.log" 2>&1 &
PROXY_PID=$!

for _ in $(seq 1 120); do
  if curl -fsS "$ADMIN_BASE/system/overview" >/dev/null 2>&1; then
    break
  fi
  kill -0 "$PROXY_PID" 2>/dev/null || fail "daemon exited during startup"
  sleep 0.25
done
curl -fsS "$ADMIN_BASE/system/overview" >/dev/null || fail "Admin API did not become ready"
kill -0 "$PROXY_PID" 2>/dev/null || fail "daemon process is not alive"
if command -v lsof >/dev/null 2>&1; then
  lsof -nP -iTCP:"$PROXY_PORT" -sTCP:LISTEN | grep -q "$PROXY_PID" \
    || fail "daemon PID is not the listener on ${PROXY_PORT}"
fi
[[ -s "$DATA_DIR/runtime.json" ]] || fail "runtime.json was not created"

RSS_BEFORE="$(ps -o rss= -p "$PROXY_PID" | tr -d ' ')"

echo "[cli-resource-live-sync] Values active push follows CLI API mutation"
READY="$DATA_DIR/values-active.ready"
CONTINUE="$DATA_DIR/unused-values-active.continue"
OUTPUT="$DATA_DIR/probe-output.json"
node "$ROOT_DIR/e2e-tests/test_utils/cli_resource_push_probe.js" \
  "$PUSH_URL" values CLI_ACTIVE_VALUE active "$READY" "$CONTINUE" "$OUTPUT" &
PROBE_PID=$!
wait_for_file "$READY"
BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" value add CLI_ACTIVE_VALUE active-v1 \
  | grep -q "added successfully"
wait_for_pid "$PROBE_PID"
PROBE_PID=""
jq -e '.ok == true and .mode == "active"' "$OUTPUT" >/dev/null
[[ "$(curl -fsS "$ADMIN_BASE/values/CLI_ACTIVE_VALUE" | jq -r '.value')" == "active-v1" ]] \
  || fail "CLI-added value was not visible through Admin API"

echo "[cli-resource-live-sync] Values resubscribe receives latest snapshot"
READY="$DATA_DIR/values-resub.ready"
CONTINUE="$DATA_DIR/values-resub.continue"
OUTPUT="$DATA_DIR/probe-output.json"
node "$ROOT_DIR/e2e-tests/test_utils/cli_resource_push_probe.js" \
  "$PUSH_URL" values CLI_RESUB_VALUE resubscribe "$READY" "$CONTINUE" "$OUTPUT" &
PROBE_PID=$!
wait_for_file "$READY"
BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" value add CLI_RESUB_VALUE resub-v1 >/dev/null
: >"$CONTINUE"
wait_for_pid "$PROBE_PID"
PROBE_PID=""
jq -e '.ok == true and .mode == "resubscribe"' "$OUTPUT" >/dev/null

echo "[cli-resource-live-sync] Scripts active push follows CLI API mutation"
READY="$DATA_DIR/scripts-active.ready"
CONTINUE="$DATA_DIR/unused-scripts-active.continue"
OUTPUT="$DATA_DIR/probe-output.json"
node "$ROOT_DIR/e2e-tests/test_utils/cli_resource_push_probe.js" \
  "$PUSH_URL" scripts cli-active-script active "$READY" "$CONTINUE" "$OUTPUT" &
PROBE_PID=$!
wait_for_file "$READY"
BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" script add request cli-active-script \
  --content 'function onRequest(request) { request.headers["x-cli-live"] = "yes"; }' \
  | grep -q "saved successfully"
wait_for_pid "$PROBE_PID"
PROBE_PID=""
jq -e '.ok == true and .mode == "active"' "$OUTPUT" >/dev/null
curl -fsS "$ADMIN_BASE/scripts/request/cli-active-script" \
  | jq -e '.content | contains("x-cli-live")' >/dev/null

echo "[cli-resource-live-sync] Scripts resubscribe receives latest snapshot"
READY="$DATA_DIR/scripts-resub.ready"
CONTINUE="$DATA_DIR/scripts-resub.continue"
OUTPUT="$DATA_DIR/probe-output.json"
node "$ROOT_DIR/e2e-tests/test_utils/cli_resource_push_probe.js" \
  "$PUSH_URL" scripts cli-resub-script resubscribe "$READY" "$CONTINUE" "$OUTPUT" &
PROBE_PID=$!
wait_for_file "$READY"
BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" script add response cli-resub-script \
  --content 'function onResponse(response) { response.headers["x-cli-resub"] = "yes"; }' >/dev/null
: >"$CONTINUE"
wait_for_pid "$PROBE_PID"
PROBE_PID=""
jq -e '.ok == true and .mode == "resubscribe"' "$OUTPUT" >/dev/null

echo "[cli-resource-live-sync] update, delete, rename and batch import stay canonical"
cat >"$DATA_DIR/import-values.json" <<'JSON'
{"CLI_IMPORT_A":"one","CLI_IMPORT_B":"two"}
JSON
BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" value import "$DATA_DIR/import-values.json" \
  | grep -q "Imported 2 value(s)"
[[ "$(curl -fsS "$ADMIN_BASE/values" | jq -r '.values | map(select(.name | startswith("CLI_IMPORT_"))) | length')" == "2" ]] \
  || fail "batch value import was not visible through Admin API"
INVALID_BATCH_STATUS="$(curl -sS -o "$DATA_DIR/invalid-batch.json" -w '%{http_code}' \
  -X PUT "$ADMIN_BASE/values" \
  -H 'Content-Type: application/json' \
  -d '{"values":{"":"invalid","CLI_SHOULD_NOT_EXIST":"blocked"}}')"
[[ "$INVALID_BATCH_STATUS" == "400" ]] || fail "empty-name batch should return 400"
if curl -fsS "$ADMIN_BASE/values/CLI_SHOULD_NOT_EXIST" >/dev/null 2>&1; then
  fail "invalid batch wrote a value"
fi
BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" value update CLI_ACTIVE_VALUE active-v2 >/dev/null
[[ "$(curl -fsS "$ADMIN_BASE/values/CLI_ACTIVE_VALUE" | jq -r '.value')" == "active-v2" ]] \
  || fail "value update was not visible"
BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" value delete CLI_ACTIVE_VALUE >/dev/null
if curl -fsS "$ADMIN_BASE/values/CLI_ACTIVE_VALUE" >/dev/null 2>&1; then
  fail "value delete was not visible"
fi
BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" script update request cli-active-script \
  --content 'function onRequest(request) { request.headers["x-cli-live"] = "updated"; }' >/dev/null
curl -fsS "$ADMIN_BASE/scripts/request/cli-active-script" \
  | jq -e '.content | contains("updated")' >/dev/null
BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" script rename request cli-active-script cli-renamed-script >/dev/null
curl -fsS "$ADMIN_BASE/scripts/request/cli-renamed-script" >/dev/null
BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" script delete request cli-renamed-script >/dev/null
if curl -fsS "$ADMIN_BASE/scripts/request/cli-renamed-script" >/dev/null 2>&1; then
  fail "script delete was not visible"
fi

echo "[cli-resource-live-sync] live runtime API failure is fail-closed"
cp "$DATA_DIR/runtime.json" "$DATA_DIR/runtime.json.saved"
python3 - "$DATA_DIR/runtime.json" <<'PY'
import json
import sys
path = sys.argv[1]
with open(path, encoding="utf-8") as handle:
    runtime = json.load(handle)
runtime["port"] = 1
with open(path, "w", encoding="utf-8") as handle:
    json.dump(runtime, handle)
PY
if BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" value add SHOULD_NOT_WRITE rejected \
  >"$DATA_DIR/fail-closed.out" 2>&1; then
  fail "CLI unexpectedly wrote directly when live runtime API verification failed"
fi
grep -q "refusing direct file writes" "$DATA_DIR/fail-closed.out" \
  || fail "fail-closed diagnostic was missing"
[[ ! -f "$DATA_DIR/values/SHOULD_NOT_WRITE.txt" ]] \
  || fail "fail-closed path created a value file"
mv "$DATA_DIR/runtime.json.saved" "$DATA_DIR/runtime.json"

sleep 1
RSS_AFTER="$(ps -o rss= -p "$PROXY_PID" | tr -d ' ')"
RSS_DELTA_KB=$((RSS_AFTER - RSS_BEFORE))
if (( RSS_DELTA_KB > 32768 )); then
  fail "daemon RSS grew by ${RSS_DELTA_KB} KiB (before=${RSS_BEFORE}, after=${RSS_AFTER})"
fi
echo "[cli-resource-live-sync] RSS before=${RSS_BEFORE} KiB after=${RSS_AFTER} KiB delta=${RSS_DELTA_KB} KiB"

echo "[cli-resource-live-sync] offline file operations remain available"
safe_cleanup_proxy "$PROXY_PID"
PROXY_PID=""
BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" value add CLI_OFFLINE_VALUE offline >/dev/null
BIFROST_DATA_DIR="$DATA_DIR" "$BIFROST_BIN" script add request cli-offline-script \
  --content 'function onRequest() {}' >/dev/null
[[ "$(cat "$DATA_DIR/values/CLI_OFFLINE_VALUE.txt")" == "offline" ]] \
  || fail "offline value write failed"
find "$DATA_DIR/scripts" -type f -name '*cli-offline-script*' -print -quit | grep -q . \
  || fail "offline script write failed"

echo "[cli-resource-live-sync] PASS"
