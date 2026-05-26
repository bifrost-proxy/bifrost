#!/bin/bash
#
# Bifrost binary installer post-install regression tests.
#
# These checks are intentionally offline: they source install-binary.sh without
# running main, then exercise the one-click setup command planner in dry-run mode.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

PASSED=0

pass() {
    echo "  ✓ $1"
    PASSED=$((PASSED + 1))
}

fail() {
    echo "  ✗ $1"
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

assert_not_contains() {
    local haystack="$1"
    local needle="$2"
    local label="$3"

    if [[ "$haystack" == *"$needle"* ]]; then
        fail "$label" "unexpected: $needle"
    fi

    pass "$label"
}

assert_order() {
    local haystack="$1"
    local first="$2"
    local second="$3"
    local label="$4"
    local first_line second_line

    first_line=$(printf '%s\n' "$haystack" | grep -nF "$first" | head -1 | cut -d: -f1)
    second_line=$(printf '%s\n' "$haystack" | grep -nF "$second" | head -1 | cut -d: -f1)

    if [[ -z "$first_line" || -z "$second_line" || "$first_line" -ge "$second_line" ]]; then
        fail "$label" "order check failed: '$first' should appear before '$second'"
    fi

    pass "$label"
}

echo "==> install-binary.sh post-install one-click setup"

BIFROST_INSTALL_BINARY_SKIP_MAIN=1 source "$PROJECT_DIR/install-binary.sh"

output=$(
    BIFROST_INSTALL_POST_INSTALL_DRY_RUN=1 \
        run_post_install "/tmp/bifrost-test-bin" 2>&1
)

assert_contains "$output" "[dry-run] /tmp/bifrost-test-bin ca install" \
    "default post-install installs and trusts CA certificate"
assert_contains "$output" "[dry-run] /tmp/bifrost-test-bin install-skill --tool all -y" \
    "default post-install installs all Bifrost skills"
assert_contains "$output" "[dry-run] /tmp/bifrost-test-bin start --daemon --yes" \
    "default post-install starts Bifrost service with auto-confirm"
assert_order "$output" "ca install" "install-skill --tool all -y" \
    "CA install runs before skill install"
assert_order "$output" "install-skill --tool all -y" "start --daemon --yes" \
    "skill install runs before service start"

skip_output=$(
    BIFROST_INSTALL_POST_INSTALL_DRY_RUN=1 \
    BIFROST_INSTALL_POST_INSTALL=0 \
        run_post_install "/tmp/bifrost-test-bin" 2>&1
)

assert_contains "$skip_output" "Post-install setup skipped" \
    "BIFROST_INSTALL_POST_INSTALL=0 disables all post-install setup"
assert_not_contains "$skip_output" "[dry-run] /tmp/bifrost-test-bin ca install" \
    "post-install opt-out skips CA command"
assert_not_contains "$skip_output" "[dry-run] /tmp/bifrost-test-bin install-skill --tool all -y" \
    "post-install opt-out skips skill command"
assert_not_contains "$skip_output" "[dry-run] /tmp/bifrost-test-bin start --daemon --yes" \
    "post-install opt-out skips start command"

partial_output=$(
    BIFROST_INSTALL_POST_INSTALL_DRY_RUN=1 \
    BIFROST_INSTALL_AUTO_CERT=0 \
    BIFROST_INSTALL_AUTO_SKILLS=0 \
    BIFROST_INSTALL_AUTO_START=0 \
        run_post_install "/tmp/bifrost-test-bin" 2>&1
)

assert_contains "$partial_output" "CA certificate installation skipped" \
    "BIFROST_INSTALL_AUTO_CERT=0 skips CA only"
assert_contains "$partial_output" "Bifrost skill installation skipped" \
    "BIFROST_INSTALL_AUTO_SKILLS=0 skips skills only"
assert_contains "$partial_output" "Bifrost service startup skipped" \
    "BIFROST_INSTALL_AUTO_START=0 skips service start only"
assert_not_contains "$partial_output" "[dry-run] /tmp/bifrost-test-bin ca install" \
    "per-step opt-out skips CA command"
assert_not_contains "$partial_output" "[dry-run] /tmp/bifrost-test-bin install-skill --tool all -y" \
    "per-step opt-out skips skill command"
assert_not_contains "$partial_output" "[dry-run] /tmp/bifrost-test-bin start --daemon --yes" \
    "per-step opt-out skips start command"

help_output=$(bash "$PROJECT_DIR/install-binary.sh" --help)
assert_contains "$help_output" "--no-post-install" \
    "help documents --no-post-install"
assert_contains "$help_output" "BIFROST_INSTALL_AUTO_START" \
    "help documents post-install environment variables"

echo ""
echo "Passed: $PASSED"
