#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
TEST_ROOT="$(mktemp -d)"
export BIFROST_E2E_SANDBOX_DIR="$TEST_ROOT"
# shellcheck source=e2e-tests/test_utils/process.sh
source "$REPO_DIR/e2e-tests/test_utils/process.sh"
mark_e2e_data_root "$TEST_ROOT"

BIFROST_BIN="${BIFROST_BIN:-$REPO_DIR/target/debug/bifrost}"
BIFROST_LOG="$TEST_ROOT/bifrost.log"
BIFROST_PID=""
RUN_PID=""

cleanup() {
  if [[ -n "$RUN_PID" ]]; then
    wait "$RUN_PID" >/dev/null 2>&1 || true
  fi
  if [[ -n "$BIFROST_PID" ]]; then
    safe_cleanup_proxy "$BIFROST_PID" || true
  fi
  kill_bifrost_in_data_root "$BIFROST_E2E_SANDBOX_DIR" || true
  rm -rf "$TEST_ROOT"
}
trap cleanup EXIT INT TERM

fail() {
  echo "[im-gateway-config-hot-reload] ERROR: $*" >&2
  for artifact in \
    provider.json target.json schedule.json schedule-updated.json \
    worker-before.json worker-active.json worker-after.json run-result.json \
    runner-config.json runner-schedule.json runner-schedule-updated.json \
    runner-worker-before.json runner-worker-active.json runner-worker-after.json \
    runner-run-result.json; do
    if [[ -f "$TEST_ROOT/$artifact" ]]; then
      echo "[im-gateway-config-hot-reload] artifact: $artifact" >&2
      sed -n '1,240p' "$TEST_ROOT/$artifact" >&2 || true
    fi
  done
  if [[ -f "$BIFROST_DATA_DIR/runtime/im-gateway-worker/im-gateway-worker.log" ]]; then
    echo "[im-gateway-config-hot-reload] IM worker log" >&2
    tail -200 "$BIFROST_DATA_DIR/runtime/im-gateway-worker/im-gateway-worker.log" >&2 || true
  fi
  [[ -f "$BIFROST_LOG" ]] && tail -200 "$BIFROST_LOG" >&2 || true
  exit 1
}

if [[ "${SKIP_BUILD:-false}" != "true" ]]; then
  echo "[im-gateway-config-hot-reload] building bifrost"
  (cd "$REPO_DIR" && SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost)
fi
[[ -x "$BIFROST_BIN" ]] || fail "bifrost binary not found: $BIFROST_BIN"

ADMIN_PORT="${ADMIN_PORT:-$(allocate_free_port)}"
export BIFROST_DATA_DIR="$TEST_ROOT/data"
mark_e2e_data_root "$BIFROST_DATA_DIR"

BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 \
BIFROST_DISABLE_TRAY=1 \
BIFROST_E2E=1 \
BIFROST_FEISHU_DRY_RUN_FILE="$TEST_ROOT/feishu-dry-run.ndjson" \
BIFROST_FEISHU_DRY_RUN_PROVIDER_ID="hot-reload-provider" \
"$BIFROST_BIN" start \
  --host 127.0.0.1 \
  --port "$ADMIN_PORT" \
  --unsafe-ssl \
  --skip-cert-check \
  --no-system-proxy \
  >"$BIFROST_LOG" 2>&1 &
BIFROST_PID=$!

BASE_URL="http://127.0.0.1:$ADMIN_PORT/_bifrost/api"
wait_for_http_ready "$BASE_URL/proxy/address" 45 0.2 \
  || fail "bifrost did not become ready"

curl -fsS --noproxy '*' -X POST -H 'content-type: application/json' \
  --data '{"id":"hot-reload-provider","provider_type":"feishu","display_name":"Hot Reload Provider","enabled":false,"app_id":"hot-reload-app","app_secret":"hot-reload-secret"}' \
  "$BASE_URL/im-gateway/providers" >"$TEST_ROOT/provider.json"
curl -fsS --noproxy '*' -X POST -H 'content-type: application/json' \
  --data '{"id":"hot-reload-target","provider_id":"hot-reload-provider","display_name":"Hot Reload Target","receive_id_type":"chat_id","receive_id":"hot-reload-chat","enabled":true}' \
  "$BASE_URL/im-gateway/targets" >"$TEST_ROOT/target.json"
curl -fsS --noproxy '*' -X POST -H 'content-type: application/json' \
  --data '{"id":"hot-reload-schedule","name":"Hot Reload Schedule","enabled":true,"message_channel":{"provider_id":"hot-reload-provider","target_id":"hot-reload-target","target_mode":"configured_target"},"trigger":{"type":"interval","every_ms":3600000},"task_type":"script","script":{"script_text":"sleep 12; printf hot-reload-task-ok"},"timeout_ms":30000,"max_output_bytes":4096}' \
  "$BASE_URL/im-gateway/schedules" >"$TEST_ROOT/schedule.json"

for _ in $(seq 1 150); do
  curl -fsS --noproxy '*' "$BASE_URL/workers/im_gateway" \
    >"$TEST_ROOT/worker-before.json" || true
  if python3 - "$TEST_ROOT/worker-before.json" <<'PY'
import json, sys
try:
    workers = json.load(open(sys.argv[1], encoding="utf-8"))
except Exception:
    raise SystemExit(1)
raise SystemExit(0 if len(workers) == 1 and workers[0].get("pid") else 1)
PY
  then
    break
  fi
  sleep 0.1
done

python3 - "$TEST_ROOT/worker-before.json" <<'PY'
import json, sys
workers = json.load(open(sys.argv[1], encoding="utf-8"))
assert len(workers) == 1 and workers[0].get("pid"), workers
PY

# Provider/target/schedule writes notify the controller independently. Wait for
# those initial notifications to settle so the mutation below is the only
# configuration change racing the active task.
for _ in $(seq 1 20); do
  sleep 0.25
  curl -fsS --noproxy '*' "$BASE_URL/workers/im_gateway" \
    >"$TEST_ROOT/worker-stable.json" || true
  if python3 - "$TEST_ROOT/worker-before.json" "$TEST_ROOT/worker-stable.json" <<'PY'
import json, sys
try:
    before = json.load(open(sys.argv[1], encoding="utf-8"))[0]
    current = json.load(open(sys.argv[2], encoding="utf-8"))[0]
except Exception:
    raise SystemExit(1)
raise SystemExit(0 if current.get("pid") == before.get("pid") else 1)
PY
  then
    cp "$TEST_ROOT/worker-stable.json" "$TEST_ROOT/worker-before.json"
    break
  fi
  cp "$TEST_ROOT/worker-stable.json" "$TEST_ROOT/worker-before.json"
done

curl -fsS --noproxy '*' -X POST \
  "$BASE_URL/im-gateway/schedules/hot-reload-schedule/run" \
  >"$TEST_ROOT/run-result.json" &
RUN_PID=$!

for _ in $(seq 1 100); do
  curl -fsS --noproxy '*' "$BASE_URL/workers/im_gateway" \
    >"$TEST_ROOT/worker-active.json" || true
  if python3 - "$TEST_ROOT/worker-active.json" <<'PY'
import json, sys
try:
    workers = json.load(open(sys.argv[1], encoding="utf-8"))
except Exception:
    raise SystemExit(1)
raise SystemExit(0 if workers and workers[0].get("activeJobs", 0) >= 1 else 1)
PY
  then
    break
  fi
  kill -0 "$RUN_PID" 2>/dev/null \
    || fail "manual schedule run ended before config mutation"
  sleep 0.1
done

python3 - "$TEST_ROOT/worker-active.json" <<'PY'
import json, sys
workers = json.load(open(sys.argv[1], encoding="utf-8"))
assert workers and workers[0].get("activeJobs", 0) >= 1, workers
PY

curl -fsS --noproxy '*' -X PATCH -H 'content-type: application/json' \
  --data '{"name":"Hot Reload Schedule Updated"}' \
  "$BASE_URL/im-gateway/schedules/hot-reload-schedule" \
  >"$TEST_ROOT/schedule-updated.json"

if ! wait "$RUN_PID"; then
  RUN_PID=""
  fail "active schedule run was interrupted by config reload"
fi
RUN_PID=""

python3 - "$TEST_ROOT/run-result.json" <<'PY'
import json, sys
result = json.load(open(sys.argv[1], encoding="utf-8"))
assert result.get("status") == "success", result
assert result.get("stdout_preview") == "hot-reload-task-ok", result
PY

curl -fsS --noproxy '*' "$BASE_URL/workers/im_gateway" \
  >"$TEST_ROOT/worker-after.json"
python3 - "$TEST_ROOT/worker-before.json" "$TEST_ROOT/worker-after.json" <<'PY'
import json, sys
before = json.load(open(sys.argv[1], encoding="utf-8"))[0]
after = json.load(open(sys.argv[2], encoding="utf-8"))[0]
assert after["pid"] == before["pid"], (before, after)
assert after["restartCount"] == before["restartCount"], (before, after)
PY

# Repeat the same regression with a real external Runner execution path. The
# mock adapter is a child process driven by the IM worker and deliberately
# remains active while an unrelated Schedule edit is hot-loaded.
python3 - "$TEST_ROOT/runner-config-request.json" <<'PY'
import json, sys
script = (
    "cat >/dev/null; "
    "printf '%s\\n' "
    "'{\"type\":\"thread.started\",\"thread_id\":\"hot-reload-thread\"}' "
    "'{\"type\":\"turn.started\"}'; "
    "sleep 12; "
    "printf '%s\\n' "
    "'{\"type\":\"item.completed\",\"item\":{\"id\":\"final\",\"type\":\"agent_message\",\"text\":\"hot-reload-runner-ok\"}}' "
    "'{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}'"
)
payload = {
    "version": 1,
    "defaultRunnerId": "hot-reload-runner",
    "runners": {
        "hot-reload-runner": {
            "enabled": True,
            "adapter": "mock",
            "adapterConfig": {"executable": "/bin/sh", "args": ["-c", script]},
            "injectBifrostTools": False,
            "skillPaths": [],
            "deliveryMode": "progress_card",
        }
    },
    "channels": {},
}
with open(sys.argv[1], "w", encoding="utf-8") as handle:
    json.dump(payload, handle)
PY
curl -fsS --noproxy '*' -X PATCH -H 'content-type: application/json' \
  --data-binary "@$TEST_ROOT/runner-config-request.json" \
  "$BASE_URL/im-gateway/chat/config" >"$TEST_ROOT/runner-config.json"

python3 - "$TEST_ROOT/runner-schedule-request.json" "$REPO_DIR" <<'PY'
import json, sys
payload = {
    "name": "Hot Reload External Runner",
    "task_type": "agent",
    "agent": {
        "prompt": "verify external runner survives config hot reload",
        "runner_id": "hot-reload-runner",
        "work_dir": sys.argv[2],
    },
    "timeout_ms": 30000,
}
with open(sys.argv[1], "w", encoding="utf-8") as handle:
    json.dump(payload, handle)
PY
curl -fsS --noproxy '*' -X PATCH -H 'content-type: application/json' \
  --data-binary "@$TEST_ROOT/runner-schedule-request.json" \
  "$BASE_URL/im-gateway/schedules/hot-reload-schedule" \
  >"$TEST_ROOT/runner-schedule.json"

sleep 1
curl -fsS --noproxy '*' "$BASE_URL/workers/im_gateway" \
  >"$TEST_ROOT/runner-worker-before.json"
curl -fsS --noproxy '*' -X POST \
  "$BASE_URL/im-gateway/schedules/hot-reload-schedule/run" \
  >"$TEST_ROOT/runner-run-result.json" &
RUN_PID=$!

for _ in $(seq 1 150); do
  curl -fsS --noproxy '*' "$BASE_URL/workers/im_gateway" \
    >"$TEST_ROOT/runner-worker-active.json" || true
  if python3 - "$TEST_ROOT/runner-worker-active.json" <<'PY'
import json, sys
try:
    workers = json.load(open(sys.argv[1], encoding="utf-8"))
except Exception:
    raise SystemExit(1)
raise SystemExit(0 if workers and workers[0].get("activeJobs", 0) >= 1 else 1)
PY
  then
    break
  fi
  kill -0 "$RUN_PID" 2>/dev/null \
    || fail "external Runner ended before config mutation"
  sleep 0.1
done

python3 - "$TEST_ROOT/runner-worker-active.json" <<'PY'
import json, sys
workers = json.load(open(sys.argv[1], encoding="utf-8"))
assert workers and workers[0].get("activeJobs", 0) >= 1, workers
PY

curl -fsS --noproxy '*' -X PATCH -H 'content-type: application/json' \
  --data '{"name":"Hot Reload External Runner Updated"}' \
  "$BASE_URL/im-gateway/schedules/hot-reload-schedule" \
  >"$TEST_ROOT/runner-schedule-updated.json"

if ! wait "$RUN_PID"; then
  RUN_PID=""
  fail "active external Runner was interrupted by config reload"
fi
RUN_PID=""

python3 - "$TEST_ROOT/runner-run-result.json" <<'PY'
import json, sys
result = json.load(open(sys.argv[1], encoding="utf-8"))
assert result.get("status") == "success", result
assert result.get("agent_final_response") == "hot-reload-runner-ok", result
PY

curl -fsS --noproxy '*' "$BASE_URL/workers/im_gateway" \
  >"$TEST_ROOT/runner-worker-after.json"
python3 - "$TEST_ROOT/runner-worker-before.json" "$TEST_ROOT/runner-worker-after.json" <<'PY'
import json, sys
before = json.load(open(sys.argv[1], encoding="utf-8"))[0]
after = json.load(open(sys.argv[2], encoding="utf-8"))[0]
assert after["pid"] == before["pid"], (before, after)
assert after["restartCount"] == before["restartCount"], (before, after)
PY

kill -0 "$BIFROST_PID" 2>/dev/null || fail "main process exited"
echo "[im-gateway-config-hot-reload] PASS"
