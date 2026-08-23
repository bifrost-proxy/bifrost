#!/bin/bash
set -euo pipefail

: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"
source "${PROJECT_DIR}/e2e-tests/test_utils/assert.sh"
source "${PROJECT_DIR}/e2e-tests/test_utils/process.sh"

PROXY_PORT="${PROXY_PORT:-$(allocate_free_port)}"
UPSTREAM_PORT="${UPSTREAM_PORT:-0}"
BIFROST_BIN="${BIFROST_BIN:-${PROJECT_DIR}/target/release/bifrost}"
if [[ ! -x "$BIFROST_BIN" && -x "${PROJECT_DIR}/target/debug/bifrost" ]]; then
    BIFROST_BIN="${PROJECT_DIR}/target/debug/bifrost"
fi

TEST_DATA_DIR="$(mktemp -d)"
PROXY_PID=""
UPSTREAM_PID=""

cleanup() {
    safe_cleanup_proxy "$PROXY_PID"
    if [[ -n "$UPSTREAM_PID" ]]; then
        kill_pid "$UPSTREAM_PID"
        wait_pid "$UPSTREAM_PID"
    fi
    rm -rf "$TEST_DATA_DIR"
}
trap cleanup EXIT

wait_for_admin() {
    local deadline=$((SECONDS + 45))
    while (( SECONDS < deadline )); do
        if env NO_PROXY="*" no_proxy="*" curl -fsS --max-time 2 \
            "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/auth/status" >/dev/null 2>&1; then
            return 0
        fi
        if [[ -n "$PROXY_PID" ]] && ! kill -0 "$PROXY_PID" 2>/dev/null; then
            cat "${TEST_DATA_DIR}/proxy.log" || true
            return 1
        fi
        sleep 0.2
    done
    cat "${TEST_DATA_DIR}/proxy.log" || true
    return 1
}

request_admin_status() {
    local method="$1"
    local path="$2"
    local body="${3:-}"
    local response_file="${TEST_DATA_DIR}/last-admin-response.json"
    local curl_args=(
        -sS -o "$response_file" -w '%{http_code}'
        -X "$method"
        "http://127.0.0.1:${PROXY_PORT}/_bifrost${path}"
    )
    if [[ -n "$body" ]]; then
        curl_args=(-sS -o "$response_file" -w '%{http_code}' -X "$method"
            -H 'Content-Type: application/json' --data "$body"
            "http://127.0.0.1:${PROXY_PORT}/_bifrost${path}")
    fi
    env NO_PROXY="*" no_proxy="*" curl "${curl_args[@]}"
}

assert_pressure_allowed() {
    local method="$1"
    local path="$2"
    local body="${3:-}"
    local status
    status="$(request_admin_status "$method" "$path" "$body")"
    if (( status >= 500 )); then
        echo "[FAIL] ${method} ${path} returned ${status} under pressure" >&2
        cat "${TEST_DATA_DIR}/last-admin-response.json" >&2 || true
        exit 1
    fi
    echo "[PASS] ${method} ${path} remains routed under pressure (${status})"
}

assert_not_pressure_rejected() {
    local method="$1"
    local path="$2"
    local body="${3:-}"
    local status
    status="$(request_admin_status "$method" "$path" "$body")"
    if [[ "$status" == "503" ]]; then
        echo "[FAIL] ${method} ${path} was rejected by the parent pressure governor" >&2
        cat "${TEST_DATA_DIR}/last-admin-response.json" >&2 || true
        exit 1
    fi
    echo "[PASS] ${method} ${path} is not pressure-gated (${status})"
}

PYTHON_BIN="$(python3_cmd)"
UPSTREAM_PORT_FILE="${TEST_DATA_DIR}/upstream.port"
"$PYTHON_BIN" - "$UPSTREAM_PORT" "$UPSTREAM_PORT_FILE" \
    >"${TEST_DATA_DIR}/upstream.log" 2>&1 <<'PY' &
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        body = b"basic-forwarding-survived-pressure"
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *_args):
        return

server = ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), Handler)
with open(sys.argv[2], "w", encoding="utf-8") as handle:
    handle.write(str(server.server_address[1]))
    handle.flush()
server.serve_forever()
PY
UPSTREAM_PID=$!
UPSTREAM_START_TIMEOUT_SECS="${UPSTREAM_START_TIMEOUT_SECS:-60}"
UPSTREAM_START_DEADLINE=$((SECONDS + UPSTREAM_START_TIMEOUT_SECS))
while (( SECONDS < UPSTREAM_START_DEADLINE )); do
    [[ -s "$UPSTREAM_PORT_FILE" ]] && break
    if ! kill -0 "$UPSTREAM_PID" 2>/dev/null; then
        echo "[FAIL] pressure-test upstream exited before publishing its port" >&2
        cat "${TEST_DATA_DIR}/upstream.log" >&2 || true
        exit 1
    fi
    sleep 0.1
done
if [[ ! -s "$UPSTREAM_PORT_FILE" ]]; then
    echo "[FAIL] pressure-test upstream did not publish its port" >&2
    cat "${TEST_DATA_DIR}/upstream.log" >&2 || true
    exit 1
fi
UPSTREAM_PORT="$(<"$UPSTREAM_PORT_FILE")"
if ! wait_for_http_ready "http://127.0.0.1:${UPSTREAM_PORT}/pressure" 30 0.1; then
    echo "[FAIL] pressure-test upstream did not become ready" >&2
    cat "${TEST_DATA_DIR}/upstream.log" >&2 || true
    exit 1
fi

export BIFROST_DATA_DIR="$TEST_DATA_DIR"
export BIFROST_RESOURCE_PRESSURE_OVERRIDE=critical
"$BIFROST_BIN" -H 127.0.0.1 -p "$PROXY_PORT" start \
    --yes --skip-cert-check --no-system-proxy \
    >"${TEST_DATA_DIR}/proxy.log" 2>&1 &
PROXY_PID=$!
wait_for_admin

HEALTH_PORT="$("$PYTHON_BIN" - "${TEST_DATA_DIR}/runtime.json" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as handle:
    print(json.load(handle)["health_port"])
PY
)"

HEALTH_JSON="$(env NO_PROXY="*" no_proxy="*" curl -fsS \
    "http://127.0.0.1:${HEALTH_PORT}/health")"
assert_json_field ".pressure" "critical" "$HEALTH_JSON" \
    "dedicated health lane reports forced critical pressure"

CANARY_STATUS="$(env NO_PROXY="" no_proxy="" curl -sS -o /dev/null -w '%{http_code}' \
    --proxy "http://127.0.0.1:${PROXY_PORT}" \
    "http://bifrost-runtime-canary.invalid/__bifrost_runtime_canary")"
assert_equals "204" "$CANARY_STATUS" "data-plane canary remains available"

HEAVY_STATUS="$(env NO_PROXY="*" no_proxy="*" curl -sS -o /dev/null -w '%{http_code}' \
    "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/traffic")"
assert_equals "503" "$HEAVY_STATUS" "large traffic query is rejected under pressure"

LIGHT_STATUS="$(env NO_PROXY="*" no_proxy="*" curl -sS -o /dev/null -w '%{http_code}' \
    "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/proxy/system/support")"
assert_equals "200" "$LIGHT_STATUS" "lightweight Admin health remains available"

IM_PROVIDERS_STATUS="$(env NO_PROXY="*" no_proxy="*" curl -sS -o /dev/null -w '%{http_code}' \
    "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/im-gateway/providers")"
assert_equals "200" "$IM_PROVIDERS_STATUS" \
    "IM provider control plane remains available under pressure"

AI_CONFIG_STATUS="$(env NO_PROXY="*" no_proxy="*" curl -sS -o /dev/null -w '%{http_code}' \
    "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/im-gateway/agent")"
assert_equals "200" "$AI_CONFIG_STATUS" \
    "AI configuration remains available under pressure"

AI_CHANNELS_STATUS="$(env NO_PROXY="*" no_proxy="*" curl -sS -o /dev/null -w '%{http_code}' \
    "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/im-gateway/chat/config")"
assert_equals "200" "$AI_CHANNELS_STATUS" \
    "AI channel configuration remains available under pressure"

ASR_CAPABILITIES_STATUS="$(env NO_PROXY="*" no_proxy="*" curl -sS -o /dev/null -w '%{http_code}' \
    "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/asr/capabilities")"
assert_equals "200" "$ASR_CAPABILITIES_STATUS" \
    "AI hub ASR capabilities remain available under pressure"

ASR_TASKS_STATUS="$(env NO_PROXY="*" no_proxy="*" curl -sS -o /dev/null -w '%{http_code}' \
    "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/asr/tasks")"
assert_equals "200" "$ASR_TASKS_STATUS" \
    "AI hub ASR task summary remains available under pressure"

ASR_STATUS_STATUS="$(env NO_PROXY="*" no_proxy="*" curl -sS -o /dev/null -w '%{http_code}' \
    "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/asr/status")"
assert_equals "200" "$ASR_STATUS_STATUS" \
    "ASR status remains available under pressure"

ASR_AUDIO_DIR="${TEST_DATA_DIR}/asr-pressure-audio"
mkdir -p "$ASR_AUDIO_DIR"
ASR_TASK_BODY="$(jq -nc --arg audio_dir "$ASR_AUDIO_DIR" \
    '{name:"Pressure route audit",audio_dir:$audio_dir,enabled:false,recursive:false}')"
ASR_TASK_CREATE_STATUS="$(request_admin_status POST "/api/asr/tasks" "$ASR_TASK_BODY")"
assert_equals "201" "$ASR_TASK_CREATE_STATUS" \
    "ASR task configuration can be created under pressure"
ASR_TASK_ID="$(jq -r '.id' "${TEST_DATA_DIR}/last-admin-response.json")"
[[ -n "$ASR_TASK_ID" && "$ASR_TASK_ID" != "null" ]]

for route in \
    "/api/asr/capabilities" \
    "/api/asr/status" \
    "/api/asr/moss/status" \
    "/api/asr/diarization/status" \
    "/api/asr/tasks" \
    "/api/asr/tasks/-/watch" \
    "/api/asr/tasks/${ASR_TASK_ID}" \
    "/api/asr/tasks/${ASR_TASK_ID}/watch" \
    "/api/asr/external-volumes" \
    "/api/asr/tasks/${ASR_TASK_ID}/external-import" \
    "/api/asr/tasks/${ASR_TASK_ID}/daily-agent" \
    "/api/asr/tasks/${ASR_TASK_ID}/daily-agent/agents" \
    "/api/asr/tasks/${ASR_TASK_ID}/daily-agent/runs" \
    "/api/asr/speaker-profiles" \
    "/api/asr/offline-jobs/missing" \
    "/api/speech/pipelines" \
    "/api/speech/pipelines/status" \
    "/api/speech/resources" \
    "/api/speech/decision?mode=realtime" \
    "/api/voice/sources" \
    "/api/voice/status" \
    "/api/voice/vocabulary" \
    "/api/voice/wake/status" \
    "/api/voice/wake/kws/status" \
    "/api/voice/wake/profiles" \
    "/api/voice/wake/bindings" \
    "/api/voice/wake/events"; do
    assert_pressure_allowed GET "$route"
done

assert_pressure_allowed PATCH "/api/asr/tasks/${ASR_TASK_ID}" '{"enabled":false}'
assert_pressure_allowed PUT "/api/asr/tasks/${ASR_TASK_ID}/daily-agent" '{}'
assert_pressure_allowed POST "/api/asr/service/stop" '{}'
assert_pressure_allowed POST "/api/voice/wake/listener/progress" '{}'
assert_pressure_allowed POST "/api/voice/wake/listener/stop" '{}'

for route in \
    "/api/replay/groups" \
    "/api/replay/requests" \
    "/api/replay/requests/count" \
    "/api/replay/history" \
    "/api/replay/history/count" \
    "/api/replay/stats" \
    "/api/im-gateway/providers" \
    "/api/im-gateway/targets" \
    "/api/im-gateway/routes" \
    "/api/im-gateway/schedules" \
    "/api/im-gateway/history/events" \
    "/api/im-gateway/history/runs" \
    "/api/im-gateway/agent" \
    "/api/im-gateway/agent/session-summaries" \
    "/api/im-gateway/chat/config" \
    "/api/worker-jobs"; do
    assert_pressure_allowed GET "$route"
done

assert_not_pressure_rejected GET "/api/asr/transcribe-ws"
assert_not_pressure_rejected POST "/api/asr/offline-jobs" '{}'
assert_not_pressure_rejected POST "/api/asr/transcribe-stream" '{}'
assert_not_pressure_rejected POST "/api/asr/speaker-profiles/identify" '{}'
assert_pressure_allowed POST "/api/voice/sessions" '{}'
assert_not_pressure_rejected GET "/api/voice/listen-ws"
assert_not_pressure_rejected POST "/api/voice/wake/trigger" '{}'

SPEECH_STATUS="$(env NO_PROXY="*" no_proxy="*" curl -sS -o /dev/null -w '%{http_code}' \
    "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/speech/pipelines/status")"
assert_equals "200" "$SPEECH_STATUS" \
    "speech pipeline status remains available under pressure"

REMOTE_INVOKE_STATUS="$(env NO_PROXY="*" no_proxy="*" curl -sS -o /dev/null -w '%{http_code}' \
    "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/remote-invoke/status")"
assert_equals "200" "$REMOTE_INVOKE_STATUS" \
    "Remote Invoke control plane remains available under pressure"

SCRIPT_CONTENT='export function onRequest(context, request) { return request; }'
SCRIPT_SAVE_STATUS="$(env NO_PROXY="*" no_proxy="*" curl -sS -o /dev/null -w '%{http_code}' \
    -X PUT -H 'Content-Type: application/json' \
    -d "$(jq -nc --arg content "$SCRIPT_CONTENT" '{content:$content}')" \
    "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/scripts/request/pressure-control-plane")"
assert_equals "200" "$SCRIPT_SAVE_STATUS" \
    "script create and save remains available under pressure"

SCRIPT_LIST_STATUS="$(env NO_PROXY="*" no_proxy="*" curl -sS -o /dev/null -w '%{http_code}' \
    "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/scripts")"
assert_equals "200" "$SCRIPT_LIST_STATUS" \
    "script list remains available under pressure"

for route in \
    "/api/remote-invoke/status" \
    "/api/remote-invoke/pairings/pending" \
    "/api/remote-invoke/grants" \
    "/api/remote-invoke/identity" \
    "/api/remote-invoke/calls?limit=100" \
    "/api/remote-invoke/shell-config" \
    "/api/remote-invoke/ssh-key" \
    "/api/remote-invoke/file-access-config"; do
    assert_pressure_allowed GET "$route"
done

SCRIPT_TEST_STATUS="$(env NO_PROXY="*" no_proxy="*" curl -sS -o /dev/null -w '%{http_code}' \
    -X POST -H 'Content-Type: application/json' \
    -d '{"type":"request","content":"export function onRequest(context, request) { return request; }"}' \
    "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/scripts/test")"
assert_equals "200" "$SCRIPT_TEST_STATUS" \
    "bounded script test remains available under pressure"

assert_not_pressure_rejected POST "/api/im-gateway/chat/stream" '{}'
assert_pressure_allowed GET "/api/worker-jobs?kind=asr&limit=100"

ASR_RUN_STATUS="$(request_admin_status POST "/api/asr/tasks/${ASR_TASK_ID}/run")"
assert_equals "200" "$ASR_RUN_STATUS" \
    "ASR worker-backed task can start under parent critical pressure"

ASR_WORKER_DEADLINE=$((SECONDS + 20))
while (( SECONDS < ASR_WORKER_DEADLINE )); do
    ASR_WORKER_JOBS="$(env NO_PROXY="*" no_proxy="*" curl -fsS \
        "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/worker-jobs?kind=asr&limit=100")"
    if jq -e 'length > 0' <<<"$ASR_WORKER_JOBS" >/dev/null; then
        break
    fi
    sleep 0.2
done
if ! jq -e 'length > 0' <<<"${ASR_WORKER_JOBS:-[]}" >/dev/null; then
    echo "[FAIL] ASR task did not create a worker job under parent pressure" >&2
    exit 1
fi
if jq -e 'any(.[]; ((.error // "") | contains("resource pressure governor")))' \
    <<<"$ASR_WORKER_JOBS" >/dev/null; then
    echo "[FAIL] ASR worker job was rejected by parent pressure" >&2
    jq . <<<"$ASR_WORKER_JOBS" >&2
    exit 1
fi
echo "[PASS] ASR worker job was created without pressure rejection"
assert_pressure_allowed POST "/api/asr/tasks/${ASR_TASK_ID}/pause?mode=long_term"

FORWARDED="$(env NO_PROXY="" no_proxy="" curl -fsS \
    --proxy "http://127.0.0.1:${PROXY_PORT}" \
    "http://127.0.0.1:${UPSTREAM_PORT}/pressure")"
assert_equals "basic-forwarding-survived-pressure" "$FORWARDED" \
    "basic forwarding remains available under pressure"

REPLAY_RESPONSE="$(env NO_PROXY="*" no_proxy="*" curl -fsS \
    -X POST \
    -H "Content-Type: application/json" \
    -d "$(jq -nc --arg url "http://127.0.0.1:${UPSTREAM_PORT}/replay-pressure" \
        '{url:$url,method:"GET",headers:[],rule_config:{mode:"none"},timeout_ms:5000}')" \
    "http://127.0.0.1:${PROXY_PORT}/_bifrost/api/replay/execute/unified")"
assert_json_field ".success" "true" "$REPLAY_RESPONSE" \
    "interactive Replay send remains available under pressure"
assert_json_field ".data.status" "200" "$REPLAY_RESPONSE" \
    "Replay receives the upstream response under pressure"
assert_json_field ".data.body" "basic-forwarding-survived-pressure" "$REPLAY_RESPONSE" \
    "Replay preserves the upstream response body under pressure"
REPLAY_TRAFFIC_ID="$(jq -r '.data.traffic_id' <<<"$REPLAY_RESPONSE")"
REPLAY_TRAFFIC_STATUS="$(request_admin_status GET "/api/traffic/${REPLAY_TRAFFIC_ID}")"
assert_equals "200" "$REPLAY_TRAFFIC_STATUS" \
    "Replay traffic detail remains readable under pressure"

if [[ "$(uname -s)" == "Darwin" && "$(uname -m)" == "arm64" ]]; then
    (
        cd "${PROJECT_DIR}/web"
        BIFROST_LIVE_BASE_URL="http://127.0.0.1:${PROXY_PORT}" \
        BIFROST_ASR_PRESSURE_TASK_ID="$ASR_TASK_ID" \
            node tests/ui/asr-pressure-live-e2e.mjs
    )
fi

ASR_TASK_DELETE_STATUS="$(request_admin_status DELETE \
    "/api/asr/tasks/${ASR_TASK_ID}?confirm_name=Pressure%20route%20audit")"
assert_equals "200" "$ASR_TASK_DELETE_STATUS" \
    "ASR task configuration can be deleted under pressure"

BODY_FILE_COUNT="$(find "${TEST_DATA_DIR}/body_cache" -type f 2>/dev/null | wc -l | tr -d ' ')"
assert_equals "0" "$BODY_FILE_COUNT" "Body payload persistence is paused"

DOCTOR_JSON="$(env BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" \
    system-proxy doctor --format json)"
assert_json_field ".health.pressure" "critical" "$DOCTOR_JSON" \
    "doctor includes runtime pressure snapshot"

[[ -s "${TEST_DATA_DIR}/system_proxy_owner_state.json" ]]
[[ -s "${TEST_DATA_DIR}/logs/system_proxy_events.jsonl" ]]

echo "[PASS] runtime pressure degradation and diagnostics E2E"
