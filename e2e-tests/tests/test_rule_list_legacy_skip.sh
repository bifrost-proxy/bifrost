#!/bin/bash
: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

BIFROST_BIN="${PROJECT_DIR}/target/release/bifrost"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi

TEST_DATA_DIR=""

cleanup() {
    if [[ -n "${TEST_DATA_DIR}" ]] && [[ -d "${TEST_DATA_DIR}" ]]; then
        rm -rf "${TEST_DATA_DIR}"
    fi
}
trap cleanup EXIT

build_bifrost() {
    if [[ -x "${BIFROST_BIN}" && "${SKIP_BUILD:-false}" == "true" ]]; then
        return 0
    fi

    if [[ ! -x "${BIFROST_BIN}" ]]; then
        cargo build --release --bin bifrost >/dev/null
    fi
}

assert_contains() {
    local haystack="$1"
    local needle="$2"
    local message="$3"

    if [[ "${haystack}" == *"${needle}"* ]]; then
        echo "PASS: ${message}"
    else
        echo "FAIL: ${message}"
        echo "Expected to find: ${needle}"
        echo "Actual output:"
        echo "${haystack}"
        exit 1
    fi
}

assert_not_contains() {
    local haystack="$1"
    local needle="$2"
    local message="$3"

    if [[ "${haystack}" == *"${needle}"* ]]; then
        echo "FAIL: ${message}"
        echo "Did not expect to find: ${needle}"
        echo "Actual output:"
        echo "${haystack}"
        exit 1
    else
        echo "PASS: ${message}"
    fi
}

main() {
    build_bifrost

    TEST_DATA_DIR="$(mktemp -d)"
    export BIFROST_DATA_DIR="${TEST_DATA_DIR}"

    "${BIFROST_BIN}" rule add valid --content "example.com host://127.0.0.1:3000" >/dev/null

    mkdir -p "${TEST_DATA_DIR}/rules"
    cat > "${TEST_DATA_DIR}/rules/.group_cache" <<'EOF'
{"group_id":"debug2","rules":["cached-only"]}
EOF

    mkdir -p "${TEST_DATA_DIR}/rules/demo-group"
    cat > "${TEST_DATA_DIR}/rules/demo-group/group-rule.bifrost" <<'EOF'
01 rules
[meta]
name = "group-rule"
enabled = true
sort_order = 0
version = "1.0.0"
created_at = "2026-04-23T00:00:00Z"
updated_at = "2026-04-23T00:00:00Z"
[meta.sync]
rule_id = "rl_demo_group_rule"
status = "local_only"
[options]
rule_count = 1
---
group.example.com host://127.0.0.1:4100
EOF

    local output
    output="$("${BIFROST_BIN}" rule list 2>&1)"

    assert_contains "${output}" "Rules (1):" "rule list counts only valid local rules"
    assert_contains "${output}" "valid [enabled]" "rule list keeps valid local rule"
    assert_not_contains "${output}" ".group_cache" "rule list ignores non-bifrost cache file"
    assert_not_contains "${output}" "Failed to load rule file" "rule list avoids parse warnings for ignored files"
    assert_not_contains "${output}" "missing field \`name\`" "rule list no longer parses cache files as legacy rules"
    assert_not_contains "${output}" "demo-group" "rule list keeps group subdirectories out of local file scan"

    echo "All rule list bifrost-filter checks passed."
}

main "$@"
