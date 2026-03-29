#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

BIFROST_BIN="${PROJECT_DIR}/target/release/bifrost"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi
TEST_DATA_DIR=""
RULE_FILE=""

cleanup() {
    if [[ -n "${TEST_DATA_DIR}" ]] && [[ -d "${TEST_DATA_DIR}" ]]; then
        rm -rf "${TEST_DATA_DIR}"
    fi
}
trap cleanup EXIT

build_bifrost() {
    if [[ -f "${BIFROST_BIN}" ]] && [[ "${SKIP_BUILD:-false}" == "true" ]]; then
        return 0
    fi

    return 0
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

main() {
    build_bifrost

    TEST_DATA_DIR="$(mktemp -d)"
    export BIFROST_DATA_DIR="${TEST_DATA_DIR}"

    "${BIFROST_BIN}" rule add demo --content "example.com host://127.0.0.1:3000"

    local updated
    updated="$("${BIFROST_BIN}" rule update demo --content "example.com statusCode://201" 2>&1)"
    assert_contains "${updated}" "updated successfully" "rule update accepts inline content"

    local shown
    shown="$("${BIFROST_BIN}" rule get demo 2>&1)"
    assert_contains "${shown}" "example.com statusCode://201" "rule content is replaced after inline update"

    RULE_FILE="${TEST_DATA_DIR}/updated-rule.txt"
    cat > "${RULE_FILE}" <<'EOF'
example.com
resHeaders://X-Updated=1
EOF

    updated="$("${BIFROST_BIN}" rule update demo --file "${RULE_FILE}" 2>&1)"
    assert_contains "${updated}" "updated successfully" "rule update accepts file input"

    shown="$("${BIFROST_BIN}" rule get demo 2>&1)"
    assert_contains "${shown}" "resHeaders://X-Updated=1" "rule content is replaced after file update"

    local missing_output
    if missing_output="$("${BIFROST_BIN}" rule update missing --content "example.com statusCode://500" 2>&1)"; then
        echo "FAIL: updating missing rule should fail"
        exit 1
    fi
    assert_contains "${missing_output}" "not found" "rule update returns not found for missing rule"

    echo "All rule update CLI checks passed."
}

main "$@"
