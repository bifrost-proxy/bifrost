#!/bin/bash
#
# Offline regression tests for Windows PATH configuration in install-binary.sh.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

PASSED=0

pass() {
    echo "  PASS $1"
    PASSED=$((PASSED + 1))
}

fail() {
    echo "  FAIL $1"
    echo "    $2"
    exit 1
}

assert_contains() {
    local haystack="$1"
    local needle="$2"
    local label="$3"

    if [[ "$haystack" != *"$needle"* ]]; then
        fail "$label" "missing: $needle"
    fi

    pass "$label"
}

assert_file_contains_once() {
    local file="$1"
    local needle="$2"
    local label="$3"
    local count

    count=$(grep -F "$needle" "$file" | wc -l | tr -d ' ')
    if [[ "$count" != "1" ]]; then
        fail "$label" "expected one match for '$needle', got $count"
    fi

    pass "$label"
}

echo "==> install-binary.sh Windows PATH configuration"

BIFROST_INSTALL_BINARY_SKIP_MAIN=1 source "$PROJECT_DIR/install-binary.sh"

run_windows_path_case() {
    local tmpdir install_dir output path_state

    tmpdir="$(mktemp -d)"
    install_dir="/c/Users/test-user/.local/bin"
    path_state="$tmpdir/windows-user-path"

    cygpath() {
        if [[ "${1:-}" == "-w" && "${2:-}" == "$install_dir" ]]; then
            printf '%s\n' 'C:\Users\test-user\.local\bin'
            return 0
        fi
        return 1
    }

    powershell.exe() {
        if [[ "${BIFROST_WINDOWS_PATH_DIR:-}" != 'C:\Users\test-user\.local\bin' ]]; then
            echo "unexpected-dir"
            return 1
        fi

        if [[ -f "$path_state" ]]; then
            echo "already"
        else
            printf '%s\n' "$BIFROST_WINDOWS_PATH_DIR" >"$path_state"
            echo "added"
        fi
    }

    HOME="$tmpdir/home" \
    SHELL="/usr/bin/bash" \
    PATH="/usr/bin:/bin" \
    NO_MODIFY_PATH=0 \
        output=$(configure_install_path windows "$install_dir" 2>&1)

    assert_contains "$output" "Added to Windows User PATH: C:\\Users\\test-user\\.local\\bin" \
        "Windows User PATH is updated automatically"
    assert_contains "$output" "export PATH=\"$install_dir:\$PATH\"" \
        "Git Bash immediate-use hint is still printed"
    assert_file_contains_once "$tmpdir/home/.bashrc" "export PATH=\"$install_dir:\$PATH\"" \
        "Git Bash .bashrc receives one PATH entry"

    HOME="$tmpdir/home" \
    SHELL="/usr/bin/bash" \
    PATH="$install_dir:/usr/bin:/bin" \
    NO_MODIFY_PATH=0 \
        output=$(configure_install_path windows "$install_dir" 2>&1)

    assert_contains "$output" "Windows User PATH already contains: C:\\Users\\test-user\\.local\\bin" \
        "Windows User PATH update is idempotent"
    assert_file_contains_once "$tmpdir/home/.bashrc" "export PATH=\"$install_dir:\$PATH\"" \
        "Git Bash .bashrc is not duplicated"

    rm -rf "$tmpdir"
}

run_no_modify_path_case() {
    local tmpdir install_dir output

    tmpdir="$(mktemp -d)"
    install_dir="/c/Users/test-user/.local/bin"

    cygpath() {
        fail "--no-modify-path skips cygpath" "cygpath should not be called"
    }

    powershell.exe() {
        fail "--no-modify-path skips PowerShell PATH update" "powershell.exe should not be called"
    }

    HOME="$tmpdir/home" \
    SHELL="/usr/bin/bash" \
    PATH="/usr/bin:/bin" \
    NO_MODIFY_PATH=1 \
        output=$(configure_install_path windows "$install_dir" 2>&1)

    assert_contains "$output" "auto-configuration skipped by --no-modify-path" \
        "--no-modify-path reports skipped PATH configuration"

    if [[ -e "$tmpdir/home/.bashrc" ]]; then
        fail "--no-modify-path does not write shell rc" "$tmpdir/home/.bashrc was created"
    fi
    pass "--no-modify-path does not write shell rc"

    rm -rf "$tmpdir"
}

run_current_process_path_prepend_case() {
    local old_path

    old_path="$PATH"
    PATH="/usr/bin:/c/Users/test-user/.local/bin:/bin"
    prepend_current_process_path "/c/Users/test-user/.local/bin"

    if [[ "$PATH" != "/c/Users/test-user/.local/bin:/usr/bin:/bin" ]]; then
        fail "current process PATH is prepended and deduplicated" "got PATH=$PATH"
    fi

    pass "current process PATH is prepended and deduplicated"
    PATH="$old_path"
}

run_windows_path_case
run_no_modify_path_case
run_current_process_path_prepend_case

echo ""
echo "Passed: $PASSED"
