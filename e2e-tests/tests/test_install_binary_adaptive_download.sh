#!/bin/bash
#
# Bifrost binary installer adaptive download source regression tests.
#
# These checks are offline: they source install-binary.sh without running main
# and replace network-facing helpers with deterministic shell stubs.

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

assert_eq() {
    local actual="$1"
    local expected="$2"
    local label="$3"

    if [[ "$actual" != "$expected" ]]; then
        fail "$label" "expected '$expected', got '$actual'"
    fi

    pass "$label"
}

run_case() {
    local label="$1"
    shift
    local test_func="$1"
    shift

    (
        set -euo pipefail
        set --
        BIFROST_INSTALL_BINARY_SKIP_MAIN=1 source "$PROJECT_DIR/install-binary.sh"
        "$test_func" "$@"
    )

    pass "$label"
}

echo "==> install-binary.sh adaptive download source selection"

test_preferred_mirror_is_first_without_duplicates() {
    local first count

    first=$(BIFROST_GITHUB_MIRROR="https://ghfast.top/https://github.com" build_mirror_url_list | sed -n '1p')
    count=$(BIFROST_GITHUB_MIRROR="https://ghfast.top/https://github.com" build_mirror_url_list | grep -c '^https://ghfast\.top/https://github\.com$')

    assert_eq "$first" "https://ghfast.top/https://github.com" \
        "BIFROST_GITHUB_MIRROR stays first"
    assert_eq "$count" "1" \
        "preferred mirror is not duplicated"
}

test_select_fastest_github_base_skips_unavailable_default() {
    probe_github_url() {
        case "$1" in
            https://ghfast.top/*) return 0 ;;
            *) return 1 ;;
        esac
    }

    local selected
    selected=$(select_fastest_github_base "${REPO}/releases/latest")
    assert_eq "$selected" "https://ghfast.top/https://github.com" \
        "fast mirror wins when github.com probe fails"
}

test_latest_version_uses_raced_redirect_result() {
    get_latest_version_via_redirect() {
        case "$1" in
            https://ghfast.top/*)
                echo "v9.8.7"
                return 0
                ;;
            *)
                return 1
                ;;
        esac
    }

    get_latest_version_via_api() {
        return 1
    }

    local version
    version=$(get_latest_version)
    assert_eq "$version" "v9.8.7" \
        "latest version detection accepts fastest mirror redirect"
}

test_download_uses_selected_source_first() {
    local tmpdir
    tmpdir=$(mktemp -d)
    trap 'rm -rf "$tmpdir"' RETURN

    probe_github_url() {
        case "$1" in
            https://ghfast.top/*) return 0 ;;
            *) return 1 ;;
        esac
    }

    download_file() {
        printf '%s\n' "$1" > "$tmpdir/selected-url"
        printf 'archive' > "$2"
        return 0
    }

    download_github_file "${REPO}/releases/download/v1.0.0/bifrost-v1.0.0-test.tar.gz" "$tmpdir/out" >/dev/null

    assert_eq "$(cat "$tmpdir/selected-url")" \
        "https://ghfast.top/https://github.com/${REPO}/releases/download/v1.0.0/bifrost-v1.0.0-test.tar.gz" \
        "full download starts with selected fastest mirror"
}

test_download_falls_back_to_full_race_after_selected_failure() {
    local tmpdir
    tmpdir=$(mktemp -d)
    trap 'rm -rf "$tmpdir"' RETURN

    probe_github_url() {
        case "$1" in
            https://ghfast.top/*) return 0 ;;
            *) return 1 ;;
        esac
    }

    download_file() {
        return 1
    }

    download_github_file_race() {
        printf '%s\n' "$1" > "$tmpdir/race-path"
        printf 'archive' > "$2"
        return 0
    }

    download_github_file "${REPO}/releases/download/v1.0.0/bifrost-v1.0.0-test.tar.gz" "$tmpdir/out" >/dev/null

    assert_eq "$(cat "$tmpdir/race-path")" \
        "${REPO}/releases/download/v1.0.0/bifrost-v1.0.0-test.tar.gz" \
        "full mirror race remains as fallback"
}

run_case "preferred mirror ordering" test_preferred_mirror_is_first_without_duplicates
run_case "fastest mirror probe selection" test_select_fastest_github_base_skips_unavailable_default
run_case "latest version redirect race" test_latest_version_uses_raced_redirect_result
run_case "selected source full download" test_download_uses_selected_source_first
run_case "fallback full mirror race" test_download_falls_back_to_full_race_after_selected_failure

help_output=$(bash "$PROJECT_DIR/install-binary.sh" --help)
if [[ "$help_output" != *"BIFROST_MIRROR_PROBE_TIMEOUT"* ]]; then
    fail "help documents mirror probe timeout" "missing BIFROST_MIRROR_PROBE_TIMEOUT"
fi
pass "help documents mirror probe timeout"

echo ""
echo "Passed: $PASSED"
