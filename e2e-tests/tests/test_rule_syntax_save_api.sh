#!/bin/bash
set -euo pipefail

: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../test_utils/admin_client.sh"
source "$SCRIPT_DIR/../test_utils/process.sh"

ADMIN_HOST="${ADMIN_HOST:-127.0.0.1}"
ADMIN_PORT="${ADMIN_PORT:-$(allocate_free_port)}"
ADMIN_PATH_PREFIX="${ADMIN_PATH_PREFIX:-/_bifrost}"
export ADMIN_HOST ADMIN_PORT ADMIN_PATH_PREFIX

TEST_PREFIX="syntax-save-$$"

cleanup() {
    admin_cleanup_bifrost
}
trap cleanup EXIT

log_info() { echo "[INFO] $*"; }
log_fail() { echo "[FAIL] $*"; }

assert_json_eq() {
    local json="$1"
    local query="$2"
    local expected="$3"
    local label="$4"
    local actual
    actual="$(jq -r "$query" <<<"$json")"
    if [[ "$actual" != "$expected" ]]; then
        log_fail "$label: expected '$expected', got '$actual'"
        echo "$json" | jq .
        exit 1
    fi
    log_info "$label"
}

assert_rule_missing() {
    local name="$1"
    local code
    code="$(env NO_PROXY="*" no_proxy="*" curl -sS -o /dev/null -w '%{http_code}' "$(admin_base_url)/api/rules/${name}")"
    if [[ "$code" != "404" ]]; then
        log_fail "Expected rule '$name' to be absent, got HTTP $code"
        exit 1
    fi
    log_info "Rule '$name' is absent as expected"
}

request_with_status() {
    local method="$1"
    local path="$2"
    local payload="$3"
    local body_file
    body_file="$(mktemp)"
    local code
    code="$(env NO_PROXY="*" no_proxy="*" curl -sS -X "$method" \
        -H "Content-Type: application/json" \
        --data-binary "$payload" \
        -o "$body_file" \
        -w '%{http_code}' \
        "$(admin_base_url)${path}")"
    printf '%s\n' "$code"
    cat "$body_file"
    rm -f "$body_file"
}

main() {
    admin_start_bifrost
    admin_wait_for_admin_ready 90

    local valid_name="${TEST_PREFIX}-valid"
    local invalid_name="${TEST_PREFIX}-invalid"
    local preserve_name="${TEST_PREFIX}-preserve"
    local allowed_name="${TEST_PREFIX}-allowed"

    log_info "Valid create returns syntax success"
    local valid_payload valid_response
    valid_payload="$(jq -cn --arg name "$valid_name" \
        --arg content "example.com host://127.0.0.1:3000" \
        '{name:$name, content:$content, enabled:true}')"
    valid_response="$(admin_post "/api/rules" "$valid_payload")"
    assert_json_eq "$valid_response" ".success" "true" "create success is true"
    assert_json_eq "$valid_response" ".saved" "true" "valid create is saved"
    assert_json_eq "$valid_response" ".syntax.valid" "true" "valid create syntax is valid"

    log_info "Invalid create is rejected and not persisted"
    local invalid_payload invalid_result invalid_code invalid_body
    invalid_payload="$(jq -cn --arg name "$invalid_name" \
        --arg content "@missing-shared-rule" \
        '{name:$name, content:$content, enabled:true}')"
    invalid_result="$(request_with_status POST "/api/rules" "$invalid_payload")"
    invalid_code="$(head -n 1 <<<"$invalid_result")"
    invalid_body="$(tail -n +2 <<<"$invalid_result")"
    if [[ "$invalid_code" != "422" ]]; then
        log_fail "Expected invalid create HTTP 422, got $invalid_code"
        echo "$invalid_body" | jq .
        exit 1
    fi
    assert_json_eq "$invalid_body" ".success" "false" "invalid create success is false"
    assert_json_eq "$invalid_body" ".saved" "false" "invalid create is not saved"
    assert_json_eq "$invalid_body" ".syntax.valid" "false" "invalid create syntax is invalid"
    assert_json_eq "$invalid_body" ".syntax.errors[0].code" "E020" "invalid create returns E020"
    assert_rule_missing "$invalid_name"

    log_info "Invalid update is rejected and original content is preserved"
    local preserve_payload update_payload update_result update_code update_body preserved_content
    preserve_payload="$(jq -cn --arg name "$preserve_name" \
        --arg content "preserve.example.com host://127.0.0.1:3001" \
        '{name:$name, content:$content, enabled:true}')"
    admin_post "/api/rules" "$preserve_payload" >/dev/null
    update_payload="$(jq -cn --arg content "@missing-update-reference" '{content:$content, enabled:true}')"
    update_result="$(request_with_status PUT "/api/rules/${preserve_name}" "$update_payload")"
    update_code="$(head -n 1 <<<"$update_result")"
    update_body="$(tail -n +2 <<<"$update_result")"
    if [[ "$update_code" != "422" ]]; then
        log_fail "Expected invalid update HTTP 422, got $update_code"
        echo "$update_body" | jq .
        exit 1
    fi
    assert_json_eq "$update_body" ".saved" "false" "invalid update is not saved"
    assert_json_eq "$update_body" ".syntax.errors[0].code" "E020" "invalid update returns E020"
    preserved_content="$(get_rule "$preserve_name" | jq -r '.content')"
    if [[ "$preserved_content" != "preserve.example.com host://127.0.0.1:3001" ]]; then
        log_fail "Invalid update changed stored content"
        exit 1
    fi
    log_info "Original content remained unchanged after rejected update"

    log_info "allow_invalid explicitly persists invalid rules with syntax report"
    local allowed_payload allowed_response
    allowed_payload="$(jq -cn --arg name "$allowed_name" \
        --arg content "@missing-allowed-reference" \
        '{name:$name, content:$content, enabled:true, allow_invalid:true}')"
    allowed_response="$(admin_post "/api/rules" "$allowed_payload")"
    assert_json_eq "$allowed_response" ".success" "true" "allow_invalid create success is true"
    assert_json_eq "$allowed_response" ".saved" "true" "allow_invalid create is saved"
    assert_json_eq "$allowed_response" ".syntax.valid" "false" "allow_invalid returns invalid syntax"
    assert_json_eq "$allowed_response" ".syntax.errors[0].code" "E020" "allow_invalid returns E020"
    assert_json_eq "$(get_rule "$allowed_name")" ".name" "$allowed_name" "allow_invalid rule can be loaded"

    log_info "Rule syntax save API checks passed."
}

main "$@"
