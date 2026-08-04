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
    selected=$(BIFROST_GITHUB_MIRROR="https://ghfast.top/https://github.com" select_fastest_github_base "${REPO}/releases/latest")
    assert_eq "$selected" "https://ghfast.top/https://github.com" \
        "explicit fast mirror wins when github.com probe fails"
}

test_default_mirrors_exclude_third_party_hosts() {
    local mirrors
    mirrors=$(build_mirror_url_list)

    if echo "$mirrors" | grep -Eq 'ghfast\.top|github\.moeyy\.xyz'; then
        fail "third-party mirrors are opt-in only" "default mirrors: $mirrors"
    fi

    assert_eq "$(echo "$mirrors" | sed -n '1p')" "https://github.com" \
        "github.com is the only default mirror"
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
    version=$(BIFROST_GITHUB_MIRROR="https://ghfast.top/https://github.com" get_latest_version)
    assert_eq "$version" "v9.8.7" \
        "latest version detection accepts explicit fastest mirror redirect"
}

test_download_uses_selected_source_first() {
    local tmpdir
    tmpdir=$(mktemp -d)
    trap 'rm -rf "$tmpdir"' RETURN

    build_mirror_url_list() {
        echo "https://ghfast.top/https://github.com"
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

    build_mirror_url_list() {
        echo "https://ghfast.top/https://github.com"
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

test_download_file_enables_visible_progress_by_default() {
    local tmpdir
    tmpdir=$(mktemp -d)
    trap 'rm -rf "$tmpdir"' RETURN

    build_downloader_list() {
        echo curl
    }

    curl() {
        printf '%s\n' "$*" > "$tmpdir/curl-args"
        local output=""
        while [[ $# -gt 0 ]]; do
            if [[ "$1" == "-o" ]]; then
                output="$2"
                break
            fi
            shift
        done
        printf 'archive' > "$output"
        return 0
    }

    download_file "https://github.com/${REPO}/releases/download/v1.0.0/test.txt" "$tmpdir/out" >/dev/null

    if ! grep -q -- '--progress-bar' "$tmpdir/curl-args"; then
        fail "curl progress is visible by default" "expected --progress-bar in: $(cat "$tmpdir/curl-args")"
    fi
    pass "curl progress is visible by default"
}

test_race_download_suppresses_candidate_progress() {
    local tmpdir
    tmpdir=$(mktemp -d)
    trap 'rm -rf "$tmpdir"' RETURN

    build_mirror_url_list() {
        echo "https://github.com"
    }

    build_downloader_list() {
        echo curl
    }

    download_with_tool() {
        printf '%s\n' "${BIFROST_DOWNLOAD_PROGRESS:-unset}" > "$tmpdir/progress-env"
        printf 'archive' > "$3"
        return 0
    }

    validate_downloaded_file() {
        return 0
    }

    download_github_file_race "${REPO}/releases/download/v1.0.0/test.txt" "$tmpdir/out" >/dev/null

    assert_eq "$(cat "$tmpdir/progress-env")" "0" \
        "race candidate downloads keep progress quiet"
}

test_optional_archive_skips_full_race_when_probe_fails() {
    local tmpdir
    tmpdir=$(mktemp -d)
    trap 'rm -rf "$tmpdir"' RETURN

    select_fastest_github_base() {
        return 1
    }

    download_github_file_race() {
        touch "$tmpdir/race-called"
        return 0
    }

    if download_github_optional_file "${REPO}/releases/download/v1.0.0/missing.tar.xz" "$tmpdir/out" >"$tmpdir/stdout" 2>"$tmpdir/stderr"; then
        fail "optional archive fast-fails when probe fails" "optional archive unexpectedly downloaded"
    fi

    if [[ -f "$tmpdir/race-called" ]]; then
        fail "optional archive skips full mirror race" "full mirror race should not run for an unavailable optional archive"
    fi

    if ! grep -q "Optional archive not available" "$tmpdir/stderr"; then
        fail "optional archive reports fast fallback" "stderr: $(cat "$tmpdir/stderr")"
    fi

    pass "optional archive skips full mirror race"
}

test_checksum_mismatch_aborts_install() {
    local tmpdir
    tmpdir=$(mktemp -d)
    trap 'rm -rf "$tmpdir"' RETURN

    local script='
set -euo pipefail
set --
BIFROST_INSTALL_BINARY_SKIP_MAIN=1
export BIFROST_INSTALL_BINARY_SKIP_MAIN
source "$SCRIPT_PATH"
get_archive_ext_candidates() {
    echo "tar.gz"
}
download_github_file() {
    case "$1" in
        *checksums.txt)
            printf "0000000000000000000000000000000000000000000000000000000000000000  bifrost-v1.0.0-test-target.tar.gz\n" > "$2"
            ;;
        *)
            printf "archive" > "$2"
            ;;
    esac
}
install_binary_for_target "test-target" "v1.0.0" "linux" "$INSTALL_TMP" "$DOWNLOAD_TMP"
'

    if SCRIPT_PATH="$PROJECT_DIR/install-binary.sh" INSTALL_TMP="$tmpdir/install" DOWNLOAD_TMP="$tmpdir" bash -c "$script" >"$tmpdir/stdout" 2>"$tmpdir/stderr"; then
        fail "checksum mismatch aborts install" "install unexpectedly continued"
    fi

    if ! grep -q "Checksum mismatch" "$tmpdir/stderr"; then
        fail "checksum mismatch reports fatal error" "stderr: $(cat "$tmpdir/stderr")"
    fi

    pass "checksum mismatch aborts install"
}

test_atomic_install_records_script_source() {
    local tmpdir
    tmpdir=$(mktemp -d)
    trap 'rm -rf "$tmpdir"' RETURN
    printf '#!/bin/sh\nexit 0\n' > "$tmpdir/source"

    install_binary_atomically "$tmpdir/source" "$tmpdir/bifrost"

    assert_eq "$(cat "$tmpdir/bifrost.install-source")" "script" \
        "atomic installer records script provenance"
}

test_source_installer_and_uninstaller_manage_provenance() {
    local tmpdir
    tmpdir=$(mktemp -d)
    trap 'rm -rf "$tmpdir"' RETURN

    if ! grep -Fq 'bifrost.install-source.tmp.$$' "$PROJECT_DIR/install.sh"; then
        fail "source installer records provenance" "install.sh is missing the atomic marker"
    fi
    printf '#!/bin/sh\nexit 0\n' > "$tmpdir/bifrost"
    printf '%s\n' script > "$tmpdir/bifrost.install-source"
    bash "$PROJECT_DIR/uninstall.sh" --dir "$tmpdir" --cli-only --yes >/dev/null
    if [[ -e "$tmpdir/bifrost" || -e "$tmpdir/bifrost.install-source" ]]; then
        fail "uninstaller removes provenance" "binary or marker remains in $tmpdir"
    fi
    pass "source installer records and uninstaller removes provenance"
}

run_case "preferred mirror ordering" test_preferred_mirror_is_first_without_duplicates
run_case "third-party mirrors opt-in" test_default_mirrors_exclude_third_party_hosts
run_case "fastest mirror probe selection" test_select_fastest_github_base_skips_unavailable_default
run_case "latest version redirect race" test_latest_version_uses_raced_redirect_result
run_case "selected source full download" test_download_uses_selected_source_first
run_case "fallback full mirror race" test_download_falls_back_to_full_race_after_selected_failure
run_case "visible curl progress" test_download_file_enables_visible_progress_by_default
run_case "quiet race candidate progress" test_race_download_suppresses_candidate_progress
run_case "optional archive fast fallback" test_optional_archive_skips_full_race_when_probe_fails
run_case "checksum mismatch fail-close" test_checksum_mismatch_aborts_install
run_case "script install provenance marker" test_atomic_install_records_script_source
run_case "source install/uninstall provenance lifecycle" test_source_installer_and_uninstaller_manage_provenance

help_output=$(bash "$PROJECT_DIR/install-binary.sh" --help)
if [[ "$help_output" != *"BIFROST_MIRROR_PROBE_TIMEOUT"* ]]; then
    fail "help documents mirror probe timeout" "missing BIFROST_MIRROR_PROBE_TIMEOUT"
fi
pass "help documents mirror probe timeout"

echo ""
echo "Passed: $PASSED"
