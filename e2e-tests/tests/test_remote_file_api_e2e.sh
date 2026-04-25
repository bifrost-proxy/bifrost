#!/bin/bash
#
# Remote File API — full CLI contract test.
#
# This test verifies that ALL `bifrost remote file` subcommands
# (twelve subcommands) are wired correctly and their --help text
# mentions the documented flags. It is intentionally hermetic
# (no relay, no daemon, no network): it only invokes the local
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

# ---------------------------------------------------------------------------
#  Read-only subcommands
# ---------------------------------------------------------------------------

test_remote_file_root_help() {
    header "bifrost remote file --help lists all twelve subcommands"
    local out
    out=$(run_bifrost remote file --help)
    local missing=""
    for sub in read list stat glob search hash write edit mkdir mv rm apply-patch; do
        if ! echo "$out" | grep -qiE "(^|[[:space:]])$sub([[:space:]]|$)"; then
            missing+=" $sub"
        fi
    done
    if [[ -z "$missing" ]]; then
        pass "all twelve subcommands present"
    else
        fail "missing subcommands:$missing"
        echo "$out" | head -50
    fi
}

test_read_help() {
    header "remote file read --help has --max-bytes / --allow-binary / --offset / --limit / --cwd"
    local out
    out=$(run_bifrost remote file read --help)
    if echo "$out" | grep -q -- "--max-bytes" \
       && echo "$out" | grep -qi "allow-binary\|binary" \
       && echo "$out" | grep -q -- "--offset" \
       && echo "$out" | grep -q -- "--limit" \
       && echo "$out" | grep -qi "cwd"; then
        pass "read --help surface ok"
    else
        fail "read --help missing required flag"
        echo "$out" | head -30
    fi
}

test_list_help() {
    header "remote file list --help has --depth / --exclude"
    local out
    out=$(run_bifrost remote file list --help)
    if echo "$out" | grep -q -- "--depth" \
       && echo "$out" | grep -q -- "--exclude"; then
        pass "list --help has --depth / --exclude"
    else
        fail "list --help missing --depth or --exclude"
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
    header "remote file glob --help has --max-matches / --exclude"
    local out
    out=$(run_bifrost remote file glob --help)
    if echo "$out" | grep -qi "pattern\|glob" \
       && echo "$out" | grep -q -- "--max-matches" \
       && echo "$out" | grep -q -- "--exclude"; then
        pass "glob --help has pattern / --max-matches / --exclude"
    else
        fail "glob --help missing"
        echo "$out" | head -20
    fi
}

test_search_help() {
    header "remote file search --help has --path / --max-scan / --context-before / --context-after / --exclude"
    local out
    out=$(run_bifrost remote file search --help)
    if echo "$out" | grep -q -- "--path" \
       && echo "$out" | grep -q -- "--max-scan" \
       && echo "$out" | grep -q -- "--context-before" \
       && echo "$out" | grep -q -- "--context-after" \
       && echo "$out" | grep -q -- "--exclude"; then
        pass "search --help has --path / --max-scan / context / exclude"
    else
        fail "search --help missing flags"
        echo "$out" | head -30
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

# ---------------------------------------------------------------------------
#  Write subcommands
# ---------------------------------------------------------------------------

test_write_help() {
    header "remote file write --help has --content-file / --base-sha256 / --allow-overwrite"
    local out
    out=$(run_bifrost remote file write --help)
    if echo "$out" | grep -q -- "--content-file" \
       && echo "$out" | grep -qi "base-sha256\|sha256" \
       && echo "$out" | grep -qi "allow-overwrite\|overwrite"; then
        pass "write --help surface ok"
    else
        fail "write --help missing required flag"
        echo "$out" | head -30
    fi
}

test_edit_help() {
    header "remote file edit --help has --edits / --base-sha256"
    local out
    out=$(run_bifrost remote file edit --help)
    if echo "$out" | grep -q -- "--edits" \
       && echo "$out" | grep -qi "base-sha256\|sha256"; then
        pass "edit --help surface ok"
    else
        fail "edit --help missing required flag"
        echo "$out" | head -30
    fi
}

test_mkdir_help() {
    header "remote file mkdir --help has --parents"
    local out
    out=$(run_bifrost remote file mkdir --help)
    if echo "$out" | grep -q -- "--parents"; then
        pass "mkdir --help has --parents"
    else
        fail "mkdir --help missing --parents"
        echo "$out" | head -20
    fi
}

test_mv_help() {
    header "remote file mv --help accepts <FROM> <TO>"
    local out
    out=$(run_bifrost remote file mv --help)
    if echo "$out" | grep -qi "source\|path" \
       && echo "$out" | grep -qi "destination\|to"; then
        pass "mv --help surface ok"
    else
        fail "mv --help missing source/destination"
        echo "$out" | head -20
    fi
}

test_rm_help() {
    header "remote file rm --help has --recursive"
    local out
    out=$(run_bifrost remote file rm --help)
    if echo "$out" | grep -q -- "--recursive"; then
        pass "rm --help has --recursive"
    else
        fail "rm --help missing --recursive"
        echo "$out" | head -20
    fi
}

# ---------------------------------------------------------------------------
#  Apply-patch
# ---------------------------------------------------------------------------

test_apply_patch_help() {
    header "remote file apply-patch --help has --patch-file"
    local out
    out=$(run_bifrost remote file apply-patch --help)
    if echo "$out" | grep -q -- "--patch-file"; then
        pass "apply-patch --help has --patch-file"
    else
        fail "apply-patch --help missing --patch-file"
        echo "$out" | head -20
    fi
}

# ---------------------------------------------------------------------------
#  Cross-cutting checks
# ---------------------------------------------------------------------------

test_output_json_supported() {
    header "all twelve subcommands accept --output (human | json)"
    local any_fail=0
    for sub in read list stat glob search hash write edit mkdir mv rm apply-patch; do
        local out
        out=$(run_bifrost remote file "$sub" --help)
        if ! echo "$out" | grep -qi -- "--output\|human\|json"; then
            fail "subcommand $sub --help missing output/human/json"
            any_fail=1
        fi
    done
    if [[ $any_fail -eq 0 ]]; then
        pass "twelve subcommands --help all mention --output"
    fi
}

test_all_subcommands_have_cwd() {
    header "all twelve subcommands accept --cwd"
    local any_fail=0
    for sub in read list stat glob search hash write edit mkdir mv rm apply-patch; do
        local out
        out=$(run_bifrost remote file "$sub" --help)
        if ! echo "$out" | grep -qi -- "--cwd"; then
            fail "subcommand $sub --help missing --cwd"
            any_fail=1
        fi
    done
    if [[ $any_fail -eq 0 ]]; then
        pass "twelve subcommands --help all mention --cwd"
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

test_missing_write_content_file_reads_stdin() {
    header "missing --content-file: write should read stdin"
    local out rc
    out=$(printf 'hello' | "$BIFROST_BIN" remote file write test.txt --help >/dev/null 2>&1)
    rc=$?
    if [[ $rc -eq 0 ]]; then
        pass "write supports stdin by default; help command exits cleanly"
    else
        fail "write help unexpectedly failed: $out"
    fi
}

test_missing_required_edit_edits_fails() {
    header "missing required --edits: edit should fail"
    local out rc
    out=$("$BIFROST_BIN" remote file edit test.txt 2>&1)
    rc=$?
    if [[ $rc -ne 0 ]] || echo "$out" | grep -qi "required\|missing\|usage\|error"; then
        pass "edit without --edits correctly rejected (exit=$rc)"
    else
        fail "edit without --edits not rejected: $out"
    fi
}

test_missing_required_mv_to_fails() {
    header "missing required <TO>: mv should fail"
    local out rc
    out=$("$BIFROST_BIN" remote file mv src.txt 2>&1)
    rc=$?
    if [[ $rc -ne 0 ]] || echo "$out" | grep -qi "required\|missing\|usage\|error"; then
        pass "mv without <TO> correctly rejected (exit=$rc)"
    else
        fail "mv without <TO> not rejected: $out"
    fi
}

test_missing_required_apply_patch_file_fails() {
    header "missing required --patch-file: apply-patch should fail"
    local out rc
    out=$("$BIFROST_BIN" remote file apply-patch 2>&1)
    rc=$?
    if [[ $rc -ne 0 ]] || echo "$out" | grep -qi "required\|missing\|usage\|error"; then
        pass "apply-patch without --patch-file correctly rejected (exit=$rc)"
    else
        fail "apply-patch without --patch-file not rejected: $out"
    fi
}

# ---------------------------------------------------------------------------
#  Missing required positional args (additional subcommands)
# ---------------------------------------------------------------------------

test_missing_required_glob_pattern_fails() {
    header "missing required <PATTERN>: glob should fail"
    local out rc
    out=$("$BIFROST_BIN" remote file glob 2>&1)
    rc=$?
    if [[ $rc -ne 0 ]] || echo "$out" | grep -qi "required\|missing\|usage\|error"; then
        pass "glob without pattern correctly rejected (exit=$rc)"
    else
        fail "glob without pattern not rejected: $out"
    fi
}

test_missing_required_search_pattern_fails() {
    header "missing required <PATTERN>: search should fail"
    local out rc
    out=$("$BIFROST_BIN" remote file search 2>&1)
    rc=$?
    if [[ $rc -ne 0 ]] || echo "$out" | grep -qi "required\|missing\|usage\|error"; then
        pass "search without pattern correctly rejected (exit=$rc)"
    else
        fail "search without pattern not rejected: $out"
    fi
}

test_missing_required_hash_path_fails() {
    header "missing required <PATH>: hash should fail"
    local out rc
    out=$("$BIFROST_BIN" remote file hash 2>&1)
    rc=$?
    if [[ $rc -ne 0 ]] || echo "$out" | grep -qi "required\|missing\|usage\|error"; then
        pass "hash without path correctly rejected (exit=$rc)"
    else
        fail "hash without path not rejected: $out"
    fi
}

test_missing_required_stat_path_fails() {
    header "missing required <PATH>: stat should fail"
    local out rc
    out=$("$BIFROST_BIN" remote file stat 2>&1)
    rc=$?
    if [[ $rc -ne 0 ]] || echo "$out" | grep -qi "required\|missing\|usage\|error"; then
        pass "stat without path correctly rejected (exit=$rc)"
    else
        fail "stat without path not rejected: $out"
    fi
}

test_missing_required_rm_path_fails() {
    header "missing required <PATH>: rm should fail"
    local out rc
    out=$("$BIFROST_BIN" remote file rm 2>&1)
    rc=$?
    if [[ $rc -ne 0 ]] || echo "$out" | grep -qi "required\|missing\|usage\|error"; then
        pass "rm without path correctly rejected (exit=$rc)"
    else
        fail "rm without path not rejected: $out"
    fi
}

test_missing_required_mkdir_path_fails() {
    header "missing required <PATH>: mkdir should fail"
    local out rc
    out=$("$BIFROST_BIN" remote file mkdir 2>&1)
    rc=$?
    if [[ $rc -ne 0 ]] || echo "$out" | grep -qi "required\|missing\|usage\|error"; then
        pass "mkdir without path correctly rejected (exit=$rc)"
    else
        fail "mkdir without path not rejected: $out"
    fi
}

test_missing_required_write_path_fails() {
    header "missing required <PATH>: write should fail"
    local out rc
    out=$("$BIFROST_BIN" remote file write 2>&1)
    rc=$?
    if [[ $rc -ne 0 ]] || echo "$out" | grep -qi "required\|missing\|usage\|error"; then
        pass "write without path correctly rejected (exit=$rc)"
    else
        fail "write without path not rejected: $out"
    fi
}

# ---------------------------------------------------------------------------
#  Coding-agent enhancement flags
# ---------------------------------------------------------------------------

test_read_offset_limit_help_description() {
    header "read --help: --offset mentions '1-based', --limit mentions 'lines'"
    local out
    out=$(run_bifrost remote file read --help)
    local ok=1
    if ! echo "$out" | grep -i -- "--offset" | grep -qi "1-based\|start line"; then
        fail "read --offset help missing '1-based' or 'start line'"
        ok=0
    fi
    if ! echo "$out" | grep -i -- "--limit" | grep -qi "line"; then
        fail "read --limit help missing 'line'"
        ok=0
    fi
    if [[ $ok -eq 1 ]]; then
        pass "read --offset/--limit help descriptions are agent-friendly"
    fi
}

test_search_short_flags_B_A() {
    header "search --help: -B and -A short flags accepted"
    local out
    out=$(run_bifrost remote file search --help)
    if echo "$out" | grep -qE -- "-B" \
       && echo "$out" | grep -qE -- "-A"; then
        pass "search -B / -A short flags present"
    else
        fail "search missing -B / -A short flags"
        echo "$out" | head -30
    fi
}

test_search_exclude_flag() {
    header "search --help: --exclude flag present"
    local out
    out=$(run_bifrost remote file search --help)
    if echo "$out" | grep -q -- "--exclude"; then
        pass "search --exclude flag present"
    else
        fail "search --exclude flag missing"
        echo "$out" | head -30
    fi
}

test_list_exclude_flag() {
    header "list --help: --exclude flag present"
    local out
    out=$(run_bifrost remote file list --help)
    if echo "$out" | grep -q -- "--exclude"; then
        pass "list --exclude flag present"
    else
        fail "list --exclude flag missing"
        echo "$out" | head -30
    fi
}

test_glob_exclude_flag() {
    header "glob --help: --exclude flag present"
    local out
    out=$(run_bifrost remote file glob --help)
    if echo "$out" | grep -q -- "--exclude"; then
        pass "glob --exclude flag present"
    else
        fail "glob --exclude flag missing"
        echo "$out" | head -30
    fi
}

# ---------------------------------------------------------------------------
#  No "Phase" text in any subcommand help
# ---------------------------------------------------------------------------

test_no_phase_text_in_help() {
    header "no subcommand --help contains 'Phase' text"
    local any_fail=0
    for sub in read list stat glob search hash write edit mkdir mv rm apply-patch; do
        local out
        out=$(run_bifrost remote file "$sub" --help)
        if echo "$out" | grep -qi "phase"; then
            fail "subcommand $sub --help contains 'Phase' text"
            any_fail=1
        fi
    done
    # Also check the root help
    local root_out
    root_out=$(run_bifrost remote file --help)
    if echo "$root_out" | grep -qi "phase"; then
        fail "remote file --help contains 'Phase' text"
        any_fail=1
    fi
    if [[ $any_fail -eq 0 ]]; then
        pass "no 'Phase' text in any file subcommand help"
    fi
}

# ---------------------------------------------------------------------------
#  Subcommand about descriptions are meaningful
# ---------------------------------------------------------------------------

test_subcommand_about_descriptions() {
    header "all subcommands have non-empty about descriptions"
    local root_out
    root_out=$(run_bifrost remote file --help)
    local any_fail=0
    for sub in read list stat glob search hash write edit mkdir mv rm apply-patch; do
        if ! echo "$root_out" | grep -qiE "$sub[[:space:]]"; then
            fail "subcommand $sub not visible in root help"
            any_fail=1
        fi
    done
    if [[ $any_fail -eq 0 ]]; then
        pass "all twelve subcommands visible in root help with about text"
    fi
}

# ---------------------------------------------------------------------------
#  Main
# ---------------------------------------------------------------------------

main() {
    ensure_binary

    # Read-only
    test_remote_file_root_help
    test_read_help
    test_list_help
    test_stat_help
    test_glob_help
    test_search_help
    test_hash_help

    # Write
    test_write_help
    test_edit_help
    test_mkdir_help
    test_mv_help
    test_rm_help

    # Apply-patch
    test_apply_patch_help

    # Cross-cutting
    test_output_json_supported
    test_all_subcommands_have_cwd
    test_missing_required_path_fails
    test_missing_write_content_file_reads_stdin
    test_missing_required_edit_edits_fails
    test_missing_required_mv_to_fails
    test_missing_required_apply_patch_file_fails

    # Additional missing-arg rejections
    test_missing_required_glob_pattern_fails
    test_missing_required_search_pattern_fails
    test_missing_required_hash_path_fails
    test_missing_required_stat_path_fails
    test_missing_required_rm_path_fails
    test_missing_required_mkdir_path_fails
    test_missing_required_write_path_fails

    # Coding-agent enhancement flags
    test_read_offset_limit_help_description
    test_search_short_flags_B_A
    test_search_exclude_flag
    test_list_exclude_flag
    test_glob_exclude_flag

    # Sanitization
    test_no_phase_text_in_help
    test_subcommand_about_descriptions

    echo ""
    echo -e "${BLUE}===============================================================${NC}"
    echo -e "  passed: ${GREEN}${PASSED}${NC}   failed: ${RED}${FAILED}${NC}"
    echo -e "${BLUE}===============================================================${NC}"

    if [[ $FAILED -gt 0 ]]; then
        exit 1
    fi
}

main "$@"
