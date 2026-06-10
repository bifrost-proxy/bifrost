#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
E2E_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
source "$E2E_DIR/test_utils/admin_client.sh"
source "$E2E_DIR/test_utils/rule_fixture.sh"

ADMIN_HOST="${ADMIN_HOST:-127.0.0.1}"
ADMIN_PORT="${ADMIN_PORT:-$(allocate_free_port)}"
export ADMIN_HOST ADMIN_PORT ADMIN_PATH_PREFIX="${ADMIN_PATH_PREFIX:-/_bifrost}"

if [[ -z "${BIFROST_BIN:-}" ]]; then
  (cd "$E2E_DIR/.." && cargo build --bin bifrost >/dev/null)
  export BIFROST_BIN="$E2E_DIR/../target/debug/bifrost"
fi

MOCK_PORT="$(allocate_free_port)"
MOCK_PID=""
SHARED_RULE_NAME="at-ref-shared-$$"
COMMENTED_RULE_NAME="commented-at-ref-shared-$$"
COMMENT_ONLY_RULE_NAME="at-ref-comment-only-$$"
ENTRY_RULE_NAME="at-ref-entry-$$"
MISSING_ENTRY_RULE_NAME="at-ref-missing-entry-$$"
GROUP_NAME="at-ref-team-$$"
GROUP_ID="gid-at-ref-team-$$"
GROUP_SHARED_RULE_NAME="at-ref-group-shared-$$"
GROUP_RULE_REF="${GROUP_NAME}/${GROUP_SHARED_RULE_NAME}"

if [[ -z "${BIFROST_DATA_DIR:-}" ]]; then
  export BIFROST_DATA_DIR="$(mktemp -d)"
fi

cleanup() {
  delete_rule "$MISSING_ENTRY_RULE_NAME" >/dev/null 2>&1 || true
  delete_rule "$ENTRY_RULE_NAME" >/dev/null 2>&1 || true
  delete_rule "$COMMENTED_RULE_NAME" >/dev/null 2>&1 || true
  delete_rule "$SHARED_RULE_NAME" >/dev/null 2>&1 || true
  if [[ -n "$MOCK_PID" ]] && kill -0 "$MOCK_PID" 2>/dev/null; then
    kill "$MOCK_PID" 2>/dev/null || true
    wait "$MOCK_PID" 2>/dev/null || true
  fi
  admin_cleanup_bifrost
}
trap cleanup EXIT

log() {
  echo "[rule-references] $*"
}

start_mock_server() {
  python3 - "$MOCK_PORT" <<'PY' &
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port = int(sys.argv[1])

class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_GET(self):
        body = json.dumps({
            "path": self.path,
            "x_at_rule": self.headers.get("X-At-Rule", ""),
            "x_commented_at_rule": self.headers.get("X-Commented-At-Rule", ""),
            "x_group_at_rule": self.headers.get("X-Group-At-Rule", ""),
        }).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
  MOCK_PID=$!

  for _ in {1..50}; do
    if curl -fsS --max-time 2 "http://127.0.0.1:${MOCK_PORT}/health" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.1
  done
  echo "mock server failed to start" >&2
  return 1
}

assert_json_field() {
  local json="$1"
  local filter="$2"
  local expected="$3"
  local actual
  actual="$(jq -r "$filter" <<<"$json")"
  if [[ "$actual" != "$expected" ]]; then
    echo "expected $filter to be '$expected', got '$actual'. body=$json" >&2
    return 1
  fi
}

write_group_rule_file() {
  local group_name="$1"
  local rule_name="$2"
  local content="$3"
  local group_dir="${BIFROST_DATA_DIR}/rules/${group_name}"
  local rule_file="${group_dir}/${rule_name}.bifrost"
  local now
  local rule_id
  now="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
  rule_id="$(uuidgen 2>/dev/null || echo "00000000-0000-4000-8000-${RANDOM}${RANDOM}")"
  mkdir -p "$group_dir"
  cat >"$rule_file" <<EOF_RULE
01 rules

[meta]
name = "${rule_name}"
enabled = false
sort_order = 0
version = "1.0.0"
created_at = "${now}"
updated_at = "${now}"
group = "${group_name}"

[meta.sync]
rule_id = "${rule_id}"
status = "local_only"

[options]
rule_count = 1

---
${content}
EOF_RULE
  printf '{"%s":"%s"}\n' "$GROUP_ID" "$group_name" >"${BIFROST_DATA_DIR}/rules/.group_cache.json"
}

shared_content="$(rule_fixture_content "$E2E_DIR/rules/references/at_rule_shared.txt")"
commented_content="$(rule_fixture_content "$E2E_DIR/rules/references/at_rule_commented_shared.txt")"
group_shared_content="$(rule_fixture_content "$E2E_DIR/rules/references/at_rule_group_shared.txt")"
entry_content="$(rule_fixture_content \
  "$E2E_DIR/rules/references/at_rule_merge.txt" \
  "SHARED_RULE=${SHARED_RULE_NAME}" \
  "COMMENTED_RULE=${COMMENTED_RULE_NAME}" \
  "COMMENT_ONLY_RULE=${COMMENT_ONLY_RULE_NAME}" \
  "GROUP_RULE_REF=${GROUP_RULE_REF}" \
  "MOCK_PORT=${MOCK_PORT}")"

write_group_rule_file "$GROUP_NAME" "$GROUP_SHARED_RULE_NAME" "$group_shared_content"

admin_ensure_bifrost
start_mock_server

log "creating disabled shared rule ${SHARED_RULE_NAME}"
create_rule "$SHARED_RULE_NAME" "$shared_content" "false" >/dev/null

log "creating disabled comment-prefixed shared rule ${COMMENTED_RULE_NAME}"
create_rule "$COMMENTED_RULE_NAME" "$commented_content" "false" >/dev/null

log "creating enabled entry rule ${ENTRY_RULE_NAME}"
create_rule "$ENTRY_RULE_NAME" "$entry_content" "true" >/dev/null

validate_payload="$(jq -cn \
  --arg name "$ENTRY_RULE_NAME" \
  --arg content "$entry_content" \
  '{current_rule_name:$name, content:$content}')"
validate_response="$(admin_post "/api/rules/validate" "$validate_payload")"
assert_json_field "$validate_response" ".valid" "true"
assert_json_field "$validate_response" ".rule_count" "4"

missing_payload="$(jq -cn \
  --arg name "$ENTRY_RULE_NAME" \
  '{current_rule_name:$name, content:"@missing-rule"}')"
missing_response="$(admin_post "/api/rules/validate" "$missing_payload")"
assert_json_field "$missing_response" ".valid" "false"
assert_json_field "$missing_response" ".errors[0].code" "E020"

log "requesting through proxy and waiting for referenced request header"
reference_matched=0
for _ in {1..40}; do
  response="$(env NO_PROXY="" no_proxy="" curl -sS --noproxy "" \
    -x "http://127.0.0.1:${ADMIN_PORT}" \
    "http://at-rule-e2e.test/check" 2>/dev/null || true)"
  if [[ "$(jq -r '.x_at_rule // empty' <<<"$response" 2>/dev/null || true)" == "ok" \
    && "$(jq -r '.x_commented_at_rule // empty' <<<"$response" 2>/dev/null || true)" == "ok" \
    && "$(jq -r '.x_group_at_rule // empty' <<<"$response" 2>/dev/null || true)" == "ok" ]]; then
    reference_matched=1
    break
  fi
  sleep 0.25
done

if [[ "$reference_matched" != "1" ]]; then
  echo "referenced rule header was not applied. last response=${response:-}" >&2
  exit 1
fi

missing_runtime_content="$(cat <<EOF_RUNTIME
@missing-runtime-reference
at-rule-missing-e2e.test host://127.0.0.1:${MOCK_PORT}
EOF_RUNTIME
)"

log "creating enabled rule with missing reference ${MISSING_ENTRY_RULE_NAME}"
create_rule "$MISSING_ENTRY_RULE_NAME" "$missing_runtime_content" "true" >/dev/null

log "requesting through proxy and waiting for runtime missing reference to be ignored"
missing_runtime_matched=0
for _ in {1..40}; do
  missing_response="$(env NO_PROXY="" no_proxy="" curl -sS --noproxy "" \
    -x "http://127.0.0.1:${ADMIN_PORT}" \
    "http://at-rule-missing-e2e.test/check" 2>/dev/null || true)"
  if [[ "$(jq -r '.path // empty' <<<"$missing_response" 2>/dev/null || true)" == "/check" ]]; then
    missing_runtime_matched=1
    break
  fi
  sleep 0.25
done

if [[ "$missing_runtime_matched" != "1" ]]; then
  echo "runtime missing reference was not ignored. last response=${missing_response:-}" >&2
  exit 1
fi

log "pass"
