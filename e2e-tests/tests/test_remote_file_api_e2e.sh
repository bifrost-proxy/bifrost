#!/bin/bash
#
# Remote File API (Phase 1) CLI contract test.
#
# This test verifies that the `bifrost remote file` command surface was
# wired correctly — six read-only subcommands are present and their
# --help text mentions the documented flags. It is intentionally
# hermetic (no relay, no daemon, no network): it only invokes the local
# binary's help output.
#
# The matching Rust integration test
# (crates/bifrost-e2e/src/tests/remote_file_api.rs) exercises the
# FileAccessPolicy logic in depth; real end-to-end transport is
# covered by the manual checklist in human_tests/remote-invoke-file.md.
#

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

BIFROST_BIN="${PROJECT_DIR}/target/release/bifrost"
if [[ ! -x "$BIFROST_BIN" && -f "${BIFROST_BIN}.exe" ]]; then
    BIFROST_BIN="${BIFROST_BIN}.exe"
fi

PASSED=0
FAILED=0

header() {
    echo ""
    echo -e "${BLUE}===============================================================${NC}"
    echo -e "${BLUE}  $1${NC}"
    echo -e "${BLUE}===============================================================${NC}"
}

info() { echo -e "${CYAN}[INFO]${NC} $1"; }
pass() { echo -e "  ${GREEN}OK${NC} $1"; PASSED=$((PASSED + 1)); }
fail() { echo -e "  ${RED}FAIL${NC} $1"; FAILED=$((FAILED + 1)); }

run_bifrost() {
    "$BIFROST_BIN" "$@" 2>&1 || true
}

ensure_binary() {
    header "check bifrost binary"
    if [[ ! -x "$BIFROST_BIN" ]]; then
        echo -e "${RED}FATAL${NC}: bifrost binary not found: $BIFROST_BIN"
        echo "Build first: cargo build --release -p bifrost-cli"
        exit 1
    fi
    info "binary: $BIFROST_BIN"
}

test_remote_file_root_help() {
    header "bifrost remote file --help lists six subcommands"
    local out
    out=$(run_bifrost remote file --help)
    local missing=""
    for sub in read list stat glob search hash; do
        if ! echo "$out" | grep -qiE "(^|[[:space:]])$sub([[:space:]]|$)"; then
            missing+=" $sub"
        fi
    done
    if [[ -z "$missing" ]]; then
        pass "all six subcommands (read/list/stat/glob/search/hash) present"
    else
        fail "missing subcommands:$missing"
        echo "$out" | head -40
    fi
}

test_read_help() {
    header "remote file read --help has --max-bytes / --allow-binary / --cwd"
    local out
    out=$(run_bifrost remote file read --help)
    if echo "$out" | grep -q -- "--max-bytes" \
       && echo "$out" | grep -qi "allow-binary\|binary" \
       && echo "$out" | grep -qi "cwd"; then
        pass "read --help surface ok"
    else
        fail "read --help missing required flag"
        echo "$out" | head -30
    fi
}

test_list_help() {
    header "remote file list --help has --depth"
    local out
    out=$(run_bifrost remote file list --help)
    if echo "$out" | grep -q -- "--depth"; then
        pass "list --help has --depth"
    else
        fail "list --help missing --depth"
        echo "$out" | head -30
    fi
}

test_stat_help() {
    header "remote file stat --help"
    local out
    out=$(run_bifrost remote file stat --help)
    if echo "$out" | grep -qi "stat\|metadata\|sha256"; then
        pass "stat --help surface ok"
    else
        fail "stat --help abnormal"
        echo "$out" | head -20
    fi
}

test_glob_help() {
    header "remote file glob --help has --max-matches"
    local out
    out=$(run_bifrost remote file glob --help)
    if echo "$out" | grep -qi "pattern\|glob" && echo "$out" | grep -q -- "--max-matches"; then
        pass "glob --help has pattern / --max-matches"
    else
        fail "glob --help missing"
        echo "$out" | head -20
    fi
}

test_search_help() {
    header "remote file search --help has --path / --max-scan"
    local out
    out=$(run_bifrost remote file search --help)
    if echo "$out" | grep -q -- "--path" && echo "$out" | grep -q -- "--max-scan"; then
        pass "search --help has --path / --max-scan"
    else
        fail "search --help missing"
        echo "$out" | head -20
    fi
}

test_hash_help() {
    header "remote file hash --help has --algo"
    local out
    out=$(run_bifrost remote file hash --help)
    if echo "$out" | grep -q -- "--algo"; then
        pass "hash --help has --algo"
    else
        fail "hash --help missing --algo"
        echo "$out" | head -20
    fi
}

test_output_json_supported() {
    header "all subcommands accept --output (human | json)"
    local any_fail=0
    for sub in read list stat glob search hash; do
        local out
        out=$(run_bifrost remote file "$sub" --help)
        if ! echo "$out" | grep -qi -- "--output\|human\|json"; then
            fail "subcommand $sub --help missing output/human/json"
            any_fail=1
        fi
    done
    if [[ $any_fail -eq 0 ]]; then
        pass "six subcommands --help all mention --output"
    fi
}

test_missing_required_path_fails() {
    header "missing required path: read should fail"
    local out rc
    out=$("$BIFROST_BIN" remote file read 2>&1)
    rc=$?
    if [[ $rc -ne 0 ]] || echo "$out" | grep -qi "required\|missing\|usage\|error"; then
        pass "read without path correctly rejected (exit=$rc)"
    else
        fail "read without path not rejected: $out"
    fi
}

main() {
    ensure_binary
    test_remote_file_root_help
    test_read_help
    test_list_help
    test_stat_help
    test_glob_help
    test_search_help
    test_hash_help
    test_output_json_supported
    test_missing_required_path_fails

    echo ""
    echo -e "${BLUE}===============================================================${NC}"
    echo -e "  passed: ${GREEN}${PASSED}${NC}   failed: ${RED}${FAILED}${NC}"
    echo -e "${BLUE}===============================================================${NC}"

    if [[ $FAILED -gt 0 ]]; then
        exit 1
    fi
}

main "$@"
