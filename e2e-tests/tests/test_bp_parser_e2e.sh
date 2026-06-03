#!/bin/bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

source "${ROOT_DIR}/e2e-tests/test_utils/assert.sh"
source "${ROOT_DIR}/e2e-tests/test_utils/process.sh"

PROXY_PORT="${PROXY_PORT:-$(allocate_free_port)}"
HTTP_PORT="${HTTP_PORT:-$(allocate_free_port)}"
REMOTE_SCRIPT_PORT="${REMOTE_SCRIPT_PORT:-$(allocate_free_port)}"
BAM_MOCK_PORT="${BAM_MOCK_PORT:-$(allocate_free_port)}"
ADMIN_BASE_URL="http://127.0.0.1:${PROXY_PORT}/_bifrost/api"
MOCK_SERVER_SCRIPT="${ROOT_DIR}/e2e-tests/mock_servers/start_servers.sh"
BIFROST_BIN="${ROOT_DIR}/target/release/bifrost"

TEST_DATA_DIR=""
MOCK_LOG_DIR=""
BIFROST_LOG_FILE=""
BIFROST_PID=""
REMOTE_SCRIPT_PID=""
BAM_MOCK_PID=""

LOCAL_MARKER="bp-local-decoded-${RANDOM}"
REMOTE_MARKER="bp-remote-decoded-${RANDOM}"
BUILD_IN_BP_MARKER="bp-build-in-sync-token-${RANDOM}"
THRIFT_STATUS_MARKER="ok"
HTTP_RPC_PERMISSION_MARKER="has_permission"
BAM_SYNC_TOKEN="sync-token-for-bp-e2e"
BAM_COOKIE="ak=bifrost_v4;c_token=mock"
NEXT_AGENT_HEALTHZ_CALL_B64="gAEAAQAAAAdIZWFsdGh6AAAABwwAAQAA"
NEXT_AGENT_HEALTHZ_REPLY_B64="gAEAAgAAAAdIZWFsdGh6AAAABwwAAAsAAQAAAAJvawwA/wAAAA=="

cleanup() {
    if [[ -n "$REMOTE_SCRIPT_PID" ]]; then
        kill "$REMOTE_SCRIPT_PID" >/dev/null 2>&1 || true
    fi
    if [[ -n "$BAM_MOCK_PID" ]]; then
        kill "$BAM_MOCK_PID" >/dev/null 2>&1 || true
    fi
    if [[ -x "$MOCK_SERVER_SCRIPT" ]]; then
        HTTP_PORT="$HTTP_PORT" MOCK_SERVERS="http" SERVER_LOG_DIR="$MOCK_LOG_DIR" \
            "$MOCK_SERVER_SCRIPT" stop >/dev/null 2>&1 || true
    fi
    safe_cleanup_proxy "$BIFROST_PID"
    [[ -n "$TEST_DATA_DIR" && -d "$TEST_DATA_DIR" ]] && rm -rf "$TEST_DATA_DIR"
    [[ -n "$MOCK_LOG_DIR" && -d "$MOCK_LOG_DIR" ]] && rm -rf "$MOCK_LOG_DIR"
    [[ -n "$BIFROST_LOG_FILE" && -f "$BIFROST_LOG_FILE" ]] && rm -f "$BIFROST_LOG_FILE"
}

trap cleanup EXIT

log() {
    echo "[bp-parser-e2e] $*"
}

build_bifrost() {
    if [[ "${SKIP_BUILD:-false}" == "true" ]]; then
        [[ -x "$BIFROST_BIN" ]] || {
            echo "SKIP_BUILD=true but missing $BIFROST_BIN" >&2
            exit 1
        }
        return
    fi
    (cd "$ROOT_DIR" && cargo build --release --bin bifrost)
}

setup_dirs() {
    TEST_DATA_DIR="$(mktemp -d "${ROOT_DIR}/.bifrost-e2e-bp-parser.XXXXXX")"
    MOCK_LOG_DIR="$(mktemp -d "${ROOT_DIR}/.bifrost-e2e-bp-mock.XXXXXX")"
    BIFROST_LOG_FILE="$(mktemp)"
    mkdir -p "$TEST_DATA_DIR/scripts/parser" "$TEST_DATA_DIR/remote"
    export BIFROST_DATA_DIR="$TEST_DATA_DIR"
}

write_parser_scripts() {
    cat > "$TEST_DATA_DIR/scripts/parser/local_echo.js" <<EOF
ctx.output = {
  code: "0",
  data: JSON.stringify({
    parser: "local",
    phase: ctx.phase,
    marker: "${LOCAL_MARKER}",
    bodyBase64: response.bodyBase64 || request.bodyBase64
  }),
  msg: ""
};
EOF

    cat > "$TEST_DATA_DIR/remote/remote_echo.js" <<EOF
ctx.output = {
  code: "0",
  data: JSON.stringify({
    parser: "remote",
    phase: ctx.phase,
    marker: "${REMOTE_MARKER}",
    bodyBase64: response.bodyBase64 || request.bodyBase64
  }),
  msg: ""
};
EOF
}

sha256_file() {
    python3 - "$1" <<'PY'
import hashlib
import pathlib
import sys
print(hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest())
PY
}

start_remote_script_server() {
    (cd "$TEST_DATA_DIR/remote" && python3 -m http.server "$REMOTE_SCRIPT_PORT" --bind 127.0.0.1 >/dev/null 2>&1) &
    REMOTE_SCRIPT_PID=$!
    for _ in $(seq 1 60); do
        if curl -sf "http://127.0.0.1:${REMOTE_SCRIPT_PORT}/remote_echo.js" >/dev/null; then
            return 0
        fi
        sleep 0.2
    done
    echo "remote script server failed to start" >&2
    exit 1
}

start_bam_mock_server() {
    cat > "$TEST_DATA_DIR/bam_mock.py" <<PY
from http.server import BaseHTTPRequestHandler, HTTPServer
import json
import urllib.parse

MARKER = "${BUILD_IN_BP_MARKER}"
SYNC_TOKEN = "${BAM_SYNC_TOKEN}"
BAM_COOKIE = "${BAM_COOKIE}"

class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def _send(self, status, payload):
        body = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        parsed = urllib.parse.urlparse(self.path)
        path = parsed.path
        if path == "/v4/sso/info" and self.headers.get("x-bifrost-token") == SYNC_TOKEN:
            self._send(200, {"code": 0, "message": "success", "data": {"bam_token": BAM_COOKIE}})
            return
        if self.headers.get("cookie") == BAM_COOKIE and path == "/api/endpoint/list":
            self._send(200, {"status_code": 0, "msg": "OK", "data": [{
                "endpoint_id": 3964350,
                "rpc_method": "Healthz",
                "path": "/api/v1/healthz",
                "serializer": "json",
                "resp_serializer": "json"
            }]})
            return
        if self.headers.get("cookie") == BAM_COOKIE and path == "/api/endpoint/info":
            self._send(200, {"status_code": 0, "msg": "OK", "data": {
                "endpoint_id": 3964350,
                "rpc_method": "Healthz",
                "name": "Healthz",
                "path": "/api/v1/healthz",
                "serializer": "json",
                "resp_serializer": "json",
                "req_schema": {
                    "req_type": "idl/flow.devops.next_agent.HealthzRequest"
                },
                "resp_schema": {
                    "200": {
                        "resp_type": "idl/flow.devops.next_agent.HealthResponse"
                    }
                }
            }})
            return
        if self.headers.get("cookie") == BAM_COOKIE and path == "/api/service/refschema":
            self._send(200, {"status_code": 0, "msg": "OK", "data": {"structs": {
                "idl/flow.devops.next_agent.HealthzRequest": {
                    "type": "object",
                    "children": {
                        "Base": {"type": "object", "field_id": 255, "raw_name": "Base", "ref_name": "idl/base.Base"}
                    }
                },
                "idl/flow.devops.next_agent.HealthResponse": {
                    "type": "object",
                    "children": {
                        "status": {"type": "string", "field_id": 1, "raw_name": "status", "raw_type": "string"},
                        "BaseResp": {"type": "object", "field_id": 255, "raw_name": "BaseResp", "ref_name": "idl/base.BaseResp"}
                    }
                },
                "idl/base.Base": {"type": "object", "children": {}},
                "idl/base.BaseResp": {"type": "object", "children": {}}
            }}})
            return
        self._send(401, {"code": 401, "message": "unauthorized"})

    def do_POST(self):
        path = urllib.parse.urlparse(self.path).path
        body = self.rfile.read(int(self.headers.get("content-length") or "0"))
        if path == "/http-service/binary_tools/parse" and self.headers.get("cookie") == BAM_COOKIE:
            parse_result = {
                "parser": "build_in_bp",
                "marker": MARKER,
                "body_hex": body.hex(),
            }
            self._send(200, {"status_code": 0, "msg": "OK", "data": {"parse_result": json.dumps(parse_result)}})
            return
        if path == "/api/v1/rpc_request":
            self._send(200, {
                "has_permission": False,
                "error_code": 0,
                "error_message": "mock explorer policy gate",
                "data": None,
                "address": "",
                "log_id": "mock-log-id",
                "req_latency": "1 ms",
            })
            return
        self._send(401, {"status_code": 401, "msg": "需要登陆", "data": None})

HTTPServer(("127.0.0.1", ${BAM_MOCK_PORT}), Handler).serve_forever()
PY
    python3 "$TEST_DATA_DIR/bam_mock.py" >/dev/null 2>&1 &
    BAM_MOCK_PID=$!
    for _ in $(seq 1 60); do
        if env NO_PROXY="*" no_proxy="*" curl -sf \
            -H "x-bifrost-token: ${BAM_SYNC_TOKEN}" \
            "http://127.0.0.1:${BAM_MOCK_PORT}/v4/sso/info" >/dev/null; then
            return 0
        fi
        sleep 0.2
    done
    echo "BAM mock server failed to start" >&2
    exit 1
}

start_mock_server() {
    HTTP_PORT="$HTTP_PORT" MOCK_SERVERS="http" SERVER_LOG_DIR="$MOCK_LOG_DIR" \
        "$MOCK_SERVER_SCRIPT" start-bg >/dev/null
    env NO_PROXY="*" no_proxy="*" curl -sf "http://127.0.0.1:${HTTP_PORT}/health" >/dev/null
}

start_bifrost() {
    local remote_sha
    remote_sha="$(sha256_file "$TEST_DATA_DIR/remote/remote_echo.js")"

    local rules_file="$TEST_DATA_DIR/bp-rules.txt"
    cat > "$rules_file" <<RULES
bp-local.test host://127.0.0.1:${HTTP_PORT}
bp-local.test bp://local_echo
bp-local.test decode://bp

bp-invalid.test host://127.0.0.1:${HTTP_PORT}
bp-invalid.test bp://../local_echo
bp-invalid.test decode://bp

bp-remote.test host://127.0.0.1:${HTTP_PORT}
bp-remote.test bp://http://127.0.0.1:${REMOTE_SCRIPT_PORT}/remote_echo.js?sha256=${remote_sha}
bp-remote.test decode://bp

bp-buildin.test host://127.0.0.1:${HTTP_PORT}
bp-buildin.test bp://build_in_bp?psm=mock.psm&pattern=%2Fget&syncToken=${BAM_SYNC_TOKEN}&bamAuthUrl=http%3A%2F%2F127.0.0.1%3A${BAM_MOCK_PORT}%2Fv4%2Fsso%2Finfo&bamParseUrl=http%3A%2F%2F127.0.0.1%3A${BAM_MOCK_PORT}%2Fhttp-service%2Fbinary_tools%2Fparse
bp-buildin.test decode://bp

bp-thrift.test host://127.0.0.1:${HTTP_PORT}
bp-thrift.test bp://build_in_bp?protocol=thrift&psm=flow.devops.next_agent&version=1.0.77&method=Healthz&syncToken=${BAM_SYNC_TOKEN}&bamAuthUrl=http%3A%2F%2F127.0.0.1%3A${BAM_MOCK_PORT}%2Fv4%2Fsso%2Finfo&bamBaseUrl=http%3A%2F%2F127.0.0.1%3A${BAM_MOCK_PORT}
bp-thrift.test decode://bp

bp-http-rpc.test host://127.0.0.1:${BAM_MOCK_PORT}
bp-http-rpc.test bp://build_in_bp?protocol=http-rpc&psm=flow.devops.next_agent&version=1.0.77&method=Healthz&syncToken=${BAM_SYNC_TOKEN}&bamAuthUrl=http%3A%2F%2F127.0.0.1%3A${BAM_MOCK_PORT}%2Fv4%2Fsso%2Finfo&bamBaseUrl=http%3A%2F%2F127.0.0.1%3A${BAM_MOCK_PORT}
bp-http-rpc.test decode://bp
RULES

    SKIP_FRONTEND_BUILD=1 BIFROST_DATA_DIR="$TEST_DATA_DIR" \
        "$BIFROST_BIN" -p "$PROXY_PORT" start -y --access-mode allow_all --skip-cert-check --unsafe-ssl --no-system-proxy \
        --rules-file "$rules_file" >"$BIFROST_LOG_FILE" 2>&1 &
    BIFROST_PID=$!

    if ! wait_for_http_ready "${ADMIN_BASE_URL}/system" 60 0.5; then
        echo "bifrost failed to start" >&2
        tail -n 200 "$BIFROST_LOG_FILE" >&2 || true
        exit 1
    fi
}

admin_get() {
    local path="$1"
    env NO_PROXY="*" no_proxy="*" curl -sf "${ADMIN_BASE_URL}${path}"
}

admin_post_json() {
    local path="$1"
    local json="$2"
    env NO_PROXY="*" no_proxy="*" curl -sf \
        -H "content-type: application/json" \
        -X POST "${ADMIN_BASE_URL}${path}" \
        --data-binary "$json"
}

run_cli() {
    CI=1 BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" -p "$PROXY_PORT" "$@" 2>&1
}

proxy_get() {
    local url="$1"
    env NO_PROXY="" no_proxy="" HTTP_PROXY="" HTTPS_PROXY="" \
        curl -sf --proxy "http://127.0.0.1:${PROXY_PORT}" --noproxy "" "$url"
}

proxy_post_b64() {
    local url="$1"
    local body_b64="$2"
    python3 - "$body_b64" <<'PY' | env NO_PROXY="" no_proxy="" HTTP_PROXY="" HTTPS_PROXY="" \
        curl -sf --proxy "http://127.0.0.1:${PROXY_PORT}" --noproxy "" \
        -X POST --data-binary @- -o /dev/null -w "ok" "$url"
import base64
import sys
sys.stdout.buffer.write(base64.b64decode(sys.argv[1]))
PY
}

proxy_post_b64_capture_response() {
    local url="$1"
    local body_b64="$2"
    local response_file
    response_file="$(mktemp)"
    python3 - "$body_b64" <<'PY' | env NO_PROXY="" no_proxy="" HTTP_PROXY="" HTTPS_PROXY="" \
        curl -sf --proxy "http://127.0.0.1:${PROXY_PORT}" --noproxy "" \
        -H "content-type: application/x-thrift" \
        -X POST --data-binary @- -o "$response_file" "$url"
import base64
import sys
sys.stdout.buffer.write(base64.b64decode(sys.argv[1]))
PY
    python3 - "$response_file" <<'PY'
import base64
import pathlib
import sys
print(base64.b64encode(pathlib.Path(sys.argv[1]).read_bytes()).decode("ascii"))
PY
    rm -f "$response_file"
}

proxy_post_json() {
    local url="$1"
    local body="$2"
    printf '%s' "$body" | env NO_PROXY="" no_proxy="" HTTP_PROXY="" HTTPS_PROXY="" \
        curl -sf --proxy "http://127.0.0.1:${PROXY_PORT}" --noproxy "" \
        -H "content-type: application/json" \
        -X POST --data-binary @- -o /dev/null -w "ok" "$url"
}

search_stream_json() {
    local keyword="$1"
    local scope_key="$2"
    local payload
    payload="$(jq -nc --arg keyword "$keyword" --arg scope_key "$scope_key" '
      {
        keyword: $keyword,
        scope: {
          request_body: ($scope_key == "request_body"),
          response_body: ($scope_key == "response_body"),
          request_headers: false,
          response_headers: false,
          url: false,
          websocket_messages: false,
          sse_events: false,
          all: false
        },
        limit: 100,
        max_results: 100,
        max_scan: 500
      }
    ')"
    printf '%s' "$payload" | env NO_PROXY="*" no_proxy="*" curl -sfN --max-time 20 \
        -H "content-type: application/json" \
        -X POST --data-binary @- "${ADMIN_BASE_URL}/search/stream"
}

wait_for_traffic_id() {
    local needle="$1"
    for _ in $(seq 1 80); do
        local id
        id="$(admin_get "/traffic?limit=50" | jq -r --arg needle "$needle" '
            (.records // [])
            | map(select(((.url // .u // "") + " " + (.host // .h // "") + " " + (.path // .p // "")) | contains($needle)))
            | sort_by(.seq // .sequence // 0)
            | last
            | .id // empty
        ')"
        if [[ -n "$id" ]]; then
            echo "$id"
            return 0
        fi
        sleep 0.25
    done
    return 1
}

body_text() {
    local id="$1"
    local suffix="${2:-}"
    admin_get "/traffic/${id}/response-body${suffix}" | jq -r '.data // ""'
}

request_body_text() {
    local id="$1"
    local suffix="${2:-}"
    admin_get "/traffic/${id}/request-body${suffix}" | jq -r '.data // ""'
}

assert_json_total_positive() {
    local json="$1"
    local message="$2"
    if printf '%s' "$json" | jq -e '.total_matched > 0' >/dev/null; then
        _log_pass "$message"
    else
        _log_fail "$message" "total_matched > 0" "$json"
        return 1
    fi
}

assert_sse_has_result_and_done() {
    local sse="$1"
    local needle="$2"
    local message="$3"
    if [[ "$sse" == *"event: result"* && "$sse" == *"$needle"* && "$sse" == *"event: done"* ]]; then
        _log_pass "$message"
    else
        _log_fail "$message" "result containing ${needle} and done event" "$sse"
        return 1
    fi
}

test_local_parser_detail_and_search() {
    local client_body
    client_body="$(proxy_get "http://bp-local.test/get?case=local")"
    assert_body_contains "\"method\": \"GET\"" "$client_body" "client should still receive upstream body for local parser"
    assert_body_not_contains "$LOCAL_MARKER" "$client_body" "decode://bp must not rewrite client response"

    local id
    id="$(wait_for_traffic_id "bp-local.test" || true)"
    if [[ -z "$id" ]]; then
        admin_get "/traffic?limit=10" >&2 || true
    fi
    assert_not_empty "$id" "local parser traffic should be recorded"

    local decoded raw detail search_json
    decoded="$(body_text "$id")"
    raw="$(body_text "$id" "?raw=1")"
    detail="$(admin_get "/traffic/${id}")"
    search_json="$(run_cli search "$LOCAL_MARKER" --res-body --format json)"
    if [[ "$decoded" != *"$LOCAL_MARKER"* ]]; then
        echo "$detail" | jq '{rules: (.rules // .matched_rules // null), decode_req_script_results, decode_res_script_results, response_body_ref, raw_response_body_ref}' >&2 || true
        tail -n 120 "$BIFROST_LOG_FILE" >&2 || true
    fi

    assert_body_contains "$LOCAL_MARKER" "$decoded" "response-body should expose local decoded parser output"
    assert_body_not_contains "$LOCAL_MARKER" "$raw" "raw response body should remain upstream body"
    echo "$detail" | jq -e '.decode_res_script_results[]? | select(.script_type == "parser" and .script_name == "local_echo" and .success == true)' >/dev/null
    assert_json_total_positive "$search_json" "bifrost search should find local decoded bp response body"

    local search_sse
    search_sse="$(search_stream_json "$LOCAL_MARKER" "response_body")"
    assert_sse_has_result_and_done "$search_sse" "$LOCAL_MARKER" "Search SSE should stream decoded bp response result and done event"
}

test_invalid_local_parser_name_is_rejected() {
    local client_body
    client_body="$(proxy_get "http://bp-invalid.test/get?case=invalid")"
    assert_body_contains "\"method\": \"GET\"" "$client_body" "client should still receive upstream body when parser name is invalid"

    local id
    id="$(wait_for_traffic_id "bp-invalid.test" || true)"
    if [[ -z "$id" ]]; then
        admin_get "/traffic?limit=10" >&2 || true
    fi
    assert_not_empty "$id" "invalid parser traffic should be recorded"

    local detail decoded
    detail="$(admin_get "/traffic/${id}")"
    decoded="$(body_text "$id")"
    assert_body_not_contains "$LOCAL_MARKER" "$decoded" "invalid parser name must not load sibling parser output"
    echo "$detail" | jq -e '.decode_res_script_results[]? | select(.script_type == "parser" and .script_name == "../local_echo" and .success == false and ((.error // "") | contains("cannot contain"))) ' >/dev/null
    _log_pass "invalid local parser name should be rejected in traffic decode result"
}

test_remote_parser_detail_cache_and_search() {
    local client_body
    client_body="$(proxy_get "http://bp-remote.test/get?case=remote")"
    assert_body_contains "\"method\": \"GET\"" "$client_body" "client should still receive upstream body for remote parser"
    assert_body_not_contains "$REMOTE_MARKER" "$client_body" "remote decode must not rewrite client response"

    local id
    id="$(wait_for_traffic_id "bp-remote.test" || true)"
    if [[ -z "$id" ]]; then
        admin_get "/traffic?limit=10" >&2 || true
    fi
    assert_not_empty "$id" "remote parser traffic should be recorded"

    local decoded detail search_json
    decoded="$(body_text "$id")"
    detail="$(admin_get "/traffic/${id}")"
    search_json="$(run_cli search "$REMOTE_MARKER" --res-body --format json)"
    if [[ "$decoded" != *"$REMOTE_MARKER"* ]]; then
        echo "$detail" | jq '{rules: (.rules // .matched_rules // null), decode_req_script_results, decode_res_script_results, response_body_ref, raw_response_body_ref}' >&2 || true
        tail -n 120 "$BIFROST_LOG_FILE" >&2 || true
    fi

    assert_body_contains "$REMOTE_MARKER" "$decoded" "response-body should expose remote decoded parser output"
    echo "$detail" | jq -e '.decode_res_script_results[]? | select(.script_type == "parser" and (.script_name | startswith("http://127.0.0.1")) and .success == true)' >/dev/null
    assert_json_total_positive "$search_json" "bifrost search should find remote decoded bp response body"

    local cache_count
    cache_count="$(find "$TEST_DATA_DIR/scripts/_remote-cache/parser" -name '*.js' -type f | wc -l | tr -d ' ')"
    assert_equals "1" "$cache_count" "remote parser should be cached after first execution"
}

test_builtin_parser_script_released_and_syntax_hinted() {
    local released_script="$TEST_DATA_DIR/scripts/parser/build_in_bp.js"
    local released_content=""
    if [[ -f "$released_script" ]]; then
        released_content="$(cat "$released_script")"
    fi
    assert_not_empty "$released_content" "build_in_bp parser script should be auto released to data dir"
    assert_body_contains "Bifrost BP parser adapter" "$released_content" "released build_in_bp script should contain adapter source"

    local syntax_json
    syntax_json="$(admin_get "/syntax")"
    if echo "$syntax_json" | jq -e '.scripts.parser_scripts[]? | select(.name == "build_in_bp")' >/dev/null; then
        _log_pass "syntax endpoint should expose auto released build_in_bp parser"
    else
        _log_fail "syntax endpoint should expose auto released build_in_bp parser" "build_in_bp parser script" "$syntax_json"
        return 1
    fi
    if echo "$syntax_json" | jq -e '.scripts.decode_scripts[]? | select(.name == "bp")' >/dev/null; then
        _log_pass "syntax endpoint should expose decode://bp smart hint"
    else
        _log_fail "syntax endpoint should expose decode://bp smart hint" "decode script bp" "$syntax_json"
        return 1
    fi
}

test_rules_validate_accepts_bp_builtin_and_non_name_refs() {
    local payload response warnings
    payload="$(python3 - <<'PY'
import json
content = "\n".join([
    "bp-validate-local.test bp://build_in_bp?protocol=thrift decode://bp",
    "bp-validate-remote.test bp://https://example.com/parser/build_in_bp.js?sha256=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef decode://bp",
    "bp-validate-file-url.test bp://file:///tmp/parser/build_in_bp.js decode://bp",
    "bp-validate-abs.test bp:///tmp/parser/build_in_bp.js decode://bp",
])
print(json.dumps({"content": content}))
PY
)"
    response="$(admin_post_json "/rules/validate" "$payload")"
    warnings="$(echo "$response" | jq -r '[.warnings[]?.message] | join("\n")')"
    if [[ "$warnings" == *"Script 'bp' not found"* ]]; then
        _log_fail "rules validate should treat decode://bp as built-in" "no missing bp warning" "$response"
        return 1
    fi
    if [[ "$warnings" == *"parser/build_in_bp.js"* || "$warnings" == *"example.com/parser"* ]]; then
        _log_fail "rules validate should not treat remote or absolute bp refs as missing scripts" "no missing parser warning" "$response"
        return 1
    fi
    echo "$response" | jq -e '.valid == true' >/dev/null
    _log_pass "rules validate accepts decode://bp and remote/absolute bp refs without script-not-found warnings"
}

test_build_in_bp_sync_token_exchange_and_search() {
    local client_body
    client_body="$(proxy_get "http://bp-buildin.test/get?case=build-in")"
    assert_body_contains "\"method\": \"GET\"" "$client_body" "client should still receive upstream body for build_in_bp parser"
    assert_body_not_contains "$BUILD_IN_BP_MARKER" "$client_body" "build_in_bp decode must not rewrite client response"

    local id
    id="$(wait_for_traffic_id "bp-buildin.test" || true)"
    if [[ -z "$id" ]]; then
        admin_get "/traffic?limit=10" >&2 || true
    fi
    assert_not_empty "$id" "build_in_bp parser traffic should be recorded"

    local decoded detail search_json
    decoded="$(body_text "$id")"
    detail="$(admin_get "/traffic/${id}")"
    search_json="$(run_cli search "$BUILD_IN_BP_MARKER" --res-body --format json)"
    if [[ "$decoded" != *"$BUILD_IN_BP_MARKER"* ]]; then
        echo "$detail" | jq '{rules: (.rules // .matched_rules // null), decode_req_script_results, decode_res_script_results, response_body_ref, raw_response_body_ref}' >&2 || true
        tail -n 120 "$BIFROST_LOG_FILE" >&2 || true
    fi

    assert_body_contains "$BUILD_IN_BP_MARKER" "$decoded" "response-body should expose build_in_bp decoded parser output"
    echo "$detail" | jq -e '.decode_res_script_results[]? | select(.script_type == "parser" and (.script_name | startswith("build_in_bp?")) and .success == true)' >/dev/null
    assert_json_total_positive "$search_json" "bifrost search should find build_in_bp decoded response body"
}

test_build_in_bp_thrift_real_rpc_bytes_and_search() {
    local client_body
    client_body="$(proxy_post_b64_capture_response "http://bp-thrift.test/thrift/healthz" "$NEXT_AGENT_HEALTHZ_CALL_B64")"
    assert_equals "$NEXT_AGENT_HEALTHZ_REPLY_B64" "$client_body" "client should receive real thrift binary response bytes"

    local id
    id="$(wait_for_traffic_id "bp-thrift.test" || true)"
    if [[ -z "$id" ]]; then
        admin_get "/traffic?limit=10" >&2 || true
    fi
    assert_not_empty "$id" "thrift build_in_bp parser traffic should be recorded"

    local decoded_req decoded_res detail search_req_json search_res_json
    decoded_req="$(request_body_text "$id")"
    decoded_res="$(body_text "$id")"
    detail="$(admin_get "/traffic/${id}")"
    search_req_json="$(run_cli search "Healthz" --req-body --format json)"
    search_res_json="$(run_cli search "$THRIFT_STATUS_MARKER" --res-body --format json)"
    if [[ "$decoded_res" != *'"status":"ok"'* ]]; then
        echo "$detail" | jq '{rules: (.rules // .matched_rules // null), decode_req_script_results, decode_res_script_results, request_body_ref, response_body_ref, raw_request_body_ref, raw_response_body_ref}' >&2 || true
        tail -n 120 "$BIFROST_LOG_FILE" >&2 || true
    fi

    assert_body_contains '"protocol":"thrift-binary"' "$decoded_req" "request-body should identify thrift binary decoding"
    assert_body_contains '"schema_type":"request"' "$decoded_req" "request-body should use request schema"
    assert_body_contains '"method":"Healthz"' "$decoded_req" "request-body should expose decoded RPC method"
    assert_body_contains '"protocol":"thrift-binary"' "$decoded_res" "response-body should identify thrift binary decoding"
    assert_body_contains '"schema_type":"response"' "$decoded_res" "response-body should use response schema"
    assert_body_contains '"method":"Healthz"' "$decoded_res" "response-body should expose decoded RPC method"
    assert_body_contains '"status":"ok"' "$decoded_res" "response-body should expose decoded Healthz status"
    echo "$detail" | jq -e '.decode_req_script_results[]? | select(.script_type == "parser" and (.script_name | contains("protocol=thrift")) and .success == true)' >/dev/null
    echo "$detail" | jq -e '.decode_res_script_results[]? | select(.script_type == "parser" and (.script_name | contains("protocol=thrift")) and .success == true)' >/dev/null
    assert_json_total_positive "$search_req_json" "bifrost search should find thrift decoded request body"
    assert_json_total_positive "$search_res_json" "bifrost search should find thrift decoded response body"
}

test_build_in_bp_http_rpc_request_and_search() {
    local rpc_body
    rpc_body='{"func_name":"Healthz","request":"{\"Base\":{\"LogID\":\"\",\"Caller\":\"\",\"Addr\":\"\",\"Client\":\"\",\"TrafficEnv\":{\"Open\":false,\"Env\":\"\"},\"Extra\":{}}}","psm":"flow.devops.next_agent","zone":"CN","idc":"lf","cluster":"default","env":"ppe_next_agent_new","request_timeout":10000,"connect_timeout":3000,"enable_fallback":true,"use_go_tag_alias":true,"idl_source":2,"idl_version":"1.0.77"}'

    local client_body
    client_body="$(proxy_post_json "http://bp-http-rpc.test/api/v1/rpc_request" "$rpc_body")"
    assert_equals "ok" "$client_body" "client should receive mocked BAM RPC response"

    local id
    id="$(wait_for_traffic_id "bp-http-rpc.test" || true)"
    if [[ -z "$id" ]]; then
        admin_get "/traffic?limit=10" >&2 || true
    fi
    assert_not_empty "$id" "http-rpc build_in_bp parser traffic should be recorded"

    local decoded_req decoded_res detail search_json
    decoded_req="$(request_body_text "$id")"
    decoded_res="$(body_text "$id")"
    detail="$(admin_get "/traffic/${id}")"
    search_json="$(run_cli search "$HTTP_RPC_PERMISSION_MARKER" --res-body --format json)"
    if [[ "$decoded_res" != *'"has_permission":false'* ]]; then
        echo "$detail" | jq '{rules: (.rules // .matched_rules // null), decode_req_script_results, decode_res_script_results, response_body_ref, raw_response_body_ref}' >&2 || true
        tail -n 120 "$BIFROST_LOG_FILE" >&2 || true
    fi

    assert_body_contains '"protocol":"http-rpc"' "$decoded_req" "request-body should identify http-rpc decoding"
    assert_body_contains '"method":"Healthz"' "$decoded_req" "request-body should expose http-rpc RPC method"
    assert_body_contains '"has_permission":false' "$decoded_res" "response-body should expose decoded BAM RPC permission result"
    echo "$detail" | jq -e '.decode_req_script_results[]? | select(.script_type == "parser" and (.script_name | contains("protocol=http-rpc")) and .success == true)' >/dev/null
    echo "$detail" | jq -e '.decode_res_script_results[]? | select(.script_type == "parser" and (.script_name | contains("protocol=http-rpc")) and .success == true)' >/dev/null
    assert_json_total_positive "$search_json" "bifrost search should find http-rpc decoded response body"
}

main() {
    build_bifrost
    setup_dirs
    write_parser_scripts
    start_remote_script_server
    start_bam_mock_server
    start_mock_server
    start_bifrost

    test_local_parser_detail_and_search
    test_invalid_local_parser_name_is_rejected
    test_remote_parser_detail_cache_and_search
    test_builtin_parser_script_released_and_syntax_hinted
    test_rules_validate_accepts_bp_builtin_and_non_name_refs
    test_build_in_bp_sync_token_exchange_and_search
    test_build_in_bp_thrift_real_rpc_bytes_and_search
    test_build_in_bp_http_rpc_request_and_search

    echo "========================================"
    echo "Total:  $TOTAL_ASSERTIONS"
    echo "Passed: $PASSED_ASSERTIONS"
    echo "Failed: $FAILED_ASSERTIONS"
    echo "========================================"
    [ "$FAILED_ASSERTIONS" -eq 0 ]
}

main "$@"
