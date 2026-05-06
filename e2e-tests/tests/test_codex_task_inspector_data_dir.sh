#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"
SKILL_DIR="${PROJECT_DIR}/.agents/skills/codex-task-inspector"
SCRIPT_PATH="${SKILL_DIR}/scripts/detect_codex_data_dir.py"

PASS_COUNT=0

pass() {
    echo "[PASS] $1"
    PASS_COUNT=$((PASS_COUNT + 1))
}

fail() {
    echo "[FAIL] $1" >&2
    exit 1
}

assert_json_field() {
    local json="$1"
    local expr="$2"
    local expected="$3"
    local actual
    actual="$(printf '%s' "$json" | python3 -c "import json,sys; data=json.load(sys.stdin); print(${expr})")"
    if [[ "$actual" != "$expected" ]]; then
        fail "${expr} expected '${expected}' but got '${actual}'"
    fi
}

test_default_dir_detection() {
    local output
    output="$(env -u CODEX_HOME python3 "$SCRIPT_PATH")"

    assert_json_field "$output" 'data["selected_source"]' 'default:$HOME/.codex'
    assert_json_field "$output" 'data["selected_path"]' "$HOME/.codex"
    pass "default data dir detection"
}

test_codex_home_override_even_when_missing() {
    local temp_root override output
    temp_root="$(mktemp -d)"
    override="$temp_root/custom-codex-home"

    output="$(CODEX_HOME="$override" python3 "$SCRIPT_PATH")"

    assert_json_field "$output" 'data["selected_source"]' 'env:CODEX_HOME'
    assert_json_field "$output" 'data["selected_path"]' "$override"
    assert_json_field "$output" 'str(data["selected_exists"]).lower()' 'false'

    rm -rf "$temp_root"
    pass "CODEX_HOME override takes precedence when missing"
}

main() {
    [[ -f "$SCRIPT_PATH" ]] || fail "script not found: $SCRIPT_PATH"

    test_default_dir_detection
    test_codex_home_override_even_when_missing

    echo "All ${PASS_COUNT} codex-task-inspector E2E checks passed."
}

main "$@"
