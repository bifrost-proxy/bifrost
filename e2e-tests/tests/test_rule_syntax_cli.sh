#!/bin/bash
set -euo pipefail

: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

BIFROST_BIN="${BIFROST_BIN:-${PROJECT_ROOT}/target/debug/bifrost}"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi
if [[ ! -x "$BIFROST_BIN" ]]; then
    (cd "$PROJECT_ROOT" && SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost)
fi

TEST_DATA_DIR=""

cleanup() {
    if [[ -n "$TEST_DATA_DIR" && -d "$TEST_DATA_DIR" ]]; then
        rm -rf "$TEST_DATA_DIR"
    fi
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

main() {
    TEST_DATA_DIR="$(mktemp -d)"
    export BIFROST_DATA_DIR="$TEST_DATA_DIR"

    log_info "Valid rule add returns success syntax report"
    local valid_output
    valid_output="$(BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" rule add syntax-valid \
        --json \
        --content "example.com host://127.0.0.1:3000")"
    assert_json_eq "$valid_output" ".success" "true" "valid add success is true"
    assert_json_eq "$valid_output" ".saved" "true" "valid add is saved"
    assert_json_eq "$valid_output" ".syntax.valid" "true" "valid add syntax is valid"

    log_info "Invalid rule add is rejected with exit code 2 and not persisted"
    local invalid_output invalid_code
    set +e
    invalid_output="$(BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" rule add syntax-invalid \
        --json \
        --content "@missing-shared-rule" 2>&1)"
    invalid_code=$?
    set -e
    if [[ "$invalid_code" != "2" ]]; then
        log_fail "Expected invalid add exit code 2, got $invalid_code"
        echo "$invalid_output"
        exit 1
    fi
    assert_json_eq "$invalid_output" ".success" "false" "invalid add success is false"
    assert_json_eq "$invalid_output" ".saved" "false" "invalid add is not saved"
    assert_json_eq "$invalid_output" ".syntax.valid" "false" "invalid add syntax is invalid"
    assert_json_eq "$invalid_output" ".syntax.errors[0].code" "E020" "invalid add returns E020"
    if BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" rule show syntax-invalid >/dev/null 2>&1; then
        log_fail "Rejected invalid rule was unexpectedly persisted"
        exit 1
    fi
    log_info "Rejected invalid rule is absent"

    log_info "Invalid update is rejected and keeps old content"
    BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" rule add syntax-preserve \
        --content "preserve.example.com host://127.0.0.1:3001" >/dev/null
    local update_output update_code
    set +e
    update_output="$(BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" rule update syntax-preserve \
        --json \
        --content "@missing-update-reference" 2>&1)"
    update_code=$?
    set -e
    if [[ "$update_code" != "2" ]]; then
        log_fail "Expected invalid update exit code 2, got $update_code"
        echo "$update_output"
        exit 1
    fi
    assert_json_eq "$update_output" ".saved" "false" "invalid update is not saved"
    assert_json_eq "$update_output" ".syntax.errors[0].code" "E020" "invalid update returns E020"
    local preserved
    preserved="$(BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" rule show syntax-preserve)"
    if [[ "$preserved" != *"preserve.example.com host://127.0.0.1:3001"* ]]; then
        log_fail "Invalid update changed stored content"
        echo "$preserved"
        exit 1
    fi
    log_info "Invalid update preserved old content"

    log_info "allow-invalid persists invalid rule while returning syntax errors"
    local allowed_output
    allowed_output="$(BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" rule add syntax-allowed \
        --json \
        --allow-invalid \
        --content "@missing-allowed-reference")"
    assert_json_eq "$allowed_output" ".success" "true" "allow-invalid success is true"
    assert_json_eq "$allowed_output" ".saved" "true" "allow-invalid is saved"
    assert_json_eq "$allowed_output" ".syntax.valid" "false" "allow-invalid syntax remains invalid"
    assert_json_eq "$allowed_output" ".syntax.errors[0].code" "E020" "allow-invalid returns E020"
    BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" rule show syntax-allowed | grep -F "@missing-allowed-reference" >/dev/null

    log_info "Rule syntax CLI checks passed."
}

main "$@"
