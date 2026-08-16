#!/bin/bash
# Verifies that a CLI running from npm/pnpm package layouts upgrades through
# the owning package manager and then resolves the newly installed binary.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
BIFROST_BIN="${BIFROST_BIN:-$PROJECT_DIR/target/debug/bifrost}"

case "$(uname -s)" in
    Darwin|Linux) ;;
    *)
        echo "SKIP package-manager source E2E currently uses Unix executable fixtures"
        exit 0
        ;;
esac

if [[ ! -x "$BIFROST_BIN" ]]; then
    echo "FAIL missing Bifrost binary: $BIFROST_BIN" >&2
    exit 1
fi

TEST_ROOT="$(mktemp -d)"
cleanup() {
    rm -rf "$TEST_ROOT"
}
trap cleanup EXIT

CURRENT_VERSION="$($BIFROST_BIN --version | awk '{print $2}')"
IFS=. read -r major minor patch <<< "${CURRENT_VERSION%%-*}"
TARGET_VERSION="$major.$minor.$((patch + 1))"
FAKE_BIN="$TEST_ROOT/bin"
FAKE_GLOBAL_ROOT="$TEST_ROOT/global/node_modules"
FAKE_NEW_BINARY="$FAKE_GLOBAL_ROOT/@bifrost-proxy/bifrost-platform/bin/bifrost"
FAKE_PM_LOG="$TEST_ROOT/package-manager.log"
mkdir -p "$FAKE_BIN" "$(dirname "$FAKE_NEW_BINARY")" "$TEST_ROOT/no-app"

cat > "$FAKE_NEW_BINARY" <<EOF
#!/bin/sh
if [ "\${1:-}" = "--version" ]; then
    echo "bifrost $TARGET_VERSION"
fi
exit 0
EOF
chmod +x "$FAKE_NEW_BINARY"

cat > "$FAKE_BIN/node" <<'EOF'
#!/bin/sh
printf '%s' "$FAKE_NEW_BINARY"
EOF
chmod +x "$FAKE_BIN/node"

cat > "$FAKE_BIN/npm" <<'EOF'
#!/bin/sh
printf 'npm %s\n' "$*" >> "$FAKE_PM_LOG"
if [ "${1:-}" = "root" ]; then
    printf '%s\n' "$FAKE_GLOBAL_ROOT"
    exit 0
fi
if [ "${FAKE_PM_FAIL:-0}" = "1" ]; then
    echo "simulated npm failure" >&2
    exit 23
fi
exit 0
EOF
chmod +x "$FAKE_BIN/npm"

cat > "$FAKE_BIN/pnpm" <<'EOF'
#!/bin/sh
printf 'pnpm %s\n' "$*" >> "$FAKE_PM_LOG"
if [ "${1:-}" = "root" ]; then
    printf '%s\n' "$FAKE_GLOBAL_ROOT"
    exit 0
fi
if [ "${FAKE_PM_FAIL:-0}" = "1" ]; then
    echo "simulated pnpm failure" >&2
    exit 24
fi
exit 0
EOF
chmod +x "$FAKE_BIN/pnpm"

export FAKE_GLOBAL_ROOT FAKE_NEW_BINARY FAKE_PM_LOG

assert_update_alias_help() {
    local upgrade_help update_help top_level_help
    upgrade_help="$($BIFROST_BIN upgrade --help)"
    update_help="$($BIFROST_BIN update --help)"
    top_level_help="$($BIFROST_BIN --help)"

    if [[ "$update_help" != "$upgrade_help" || "$top_level_help" != *"alias: update"* ]]; then
        echo "FAIL update alias help does not match or is not discoverable" >&2
        diff -u <(printf '%s\n' "$upgrade_help") <(printf '%s\n' "$update_help") >&2 || true
        exit 1
    fi
    echo "PASS update alias exposes the same help and parameters as upgrade"
}

run_source_case() {
    local manager="$1"
    local command="$2"
    local install_binary="$TEST_ROOT/$manager/node_modules/@bifrost-proxy/bifrost-darwin-arm64/bin/bifrost"
    local data_dir="$TEST_ROOT/data-$manager"
    local output status expected_command
    mkdir -p "$(dirname "$install_binary")" "$data_dir"
    cp "$BIFROST_BIN" "$install_binary"
    chmod +x "$install_binary"
    printf '%s\n' package-manager-owned > "$FAKE_NEW_BINARY.backup"
    : > "$FAKE_PM_LOG"

    set +e
    output=$(PATH="$FAKE_BIN:/usr/bin:/bin" \
        BIFROST_DATA_DIR="$data_dir" \
        BIFROST_APP_INSTALL_DIR="$TEST_ROOT/no-app" \
        BIFROST_CLI_INSTALL_SOURCE="$manager" \
        BIFROST_DESKTOP_MANAGED_UPGRADE_SKIP_APP=1 \
        BIFROST_DESKTOP_MANAGED_UPGRADE_SKIP_RESTART=1 \
        BIFROST_UPGRADE_TEST_ALLOW_RELEASE_OVERRIDES=1 \
        BIFROST_UPGRADE_TEST_LATEST_VERSION="$TARGET_VERSION" \
        "$install_binary" "$command" 2>&1)
    status=$?
    set -e

    if [[ "$manager" = "npm" ]]; then
        expected_command="npm install --global --no-audit --progress=false @bifrost-proxy/bifrost@$TARGET_VERSION"
    else
        expected_command="pnpm add --global @bifrost-proxy/bifrost@$TARGET_VERSION"
    fi
    if [[ $status -ne 0 \
        || "$output" != *"Install method: $manager"* \
        || "$output" != *"Upgrading via $manager"* \
        || "$output" != *"package upgrade completed"* \
        || "$output" == *"Replacing binary at:"* \
        || ! -f "$FAKE_NEW_BINARY.backup" ]] \
        || ! grep -Fxq "$expected_command" "$FAKE_PM_LOG"; then
        echo "FAIL $manager source-aware upgrade: status=$status" >&2
        echo "$output" >&2
        echo "package-manager log:" >&2
        cat "$FAKE_PM_LOG" >&2
        exit 1
    fi
    echo "PASS $manager install source uses $command and pinned global package-manager upgrade without direct package-file cleanup"
}

assert_update_alias_help
run_source_case npm update
run_source_case pnpm upgrade

case "$(uname -s)-$(uname -m)" in
    Darwin-arm64|Darwin-aarch64) LOCAL_TARGET="aarch64-apple-darwin" ;;
    Darwin-x86_64) LOCAL_TARGET="x86_64-apple-darwin" ;;
    Linux-arm64|Linux-aarch64) LOCAL_TARGET="aarch64-unknown-linux-gnu" ;;
    Linux-x86_64) LOCAL_TARGET="x86_64-unknown-linux-gnu" ;;
    *) LOCAL_TARGET="" ;;
esac

if [[ -n "$LOCAL_TARGET" ]]; then
    LOCAL_ASSETS_DIR="$TEST_ROOT/local-assets"
    LOCAL_ARCHIVE_ROOT="$TEST_ROOT/local-archive/bifrost-v$TARGET_VERSION-$LOCAL_TARGET"
    LOCAL_INSTALL_BINARY="$TEST_ROOT/npm-local/node_modules/@bifrost-proxy/bifrost-$LOCAL_TARGET/bin/bifrost"
    mkdir -p "$LOCAL_ASSETS_DIR" "$LOCAL_ARCHIVE_ROOT" "$(dirname "$LOCAL_INSTALL_BINARY")" "$TEST_ROOT/data-local"
    cp "$BIFROST_BIN" "$LOCAL_INSTALL_BINARY"
    chmod +x "$LOCAL_INSTALL_BINARY"
    cp "$FAKE_NEW_BINARY" "$LOCAL_ARCHIVE_ROOT/bifrost"
    tar -C "$TEST_ROOT/local-archive" -czf \
        "$LOCAL_ASSETS_DIR/bifrost-v$TARGET_VERSION-$LOCAL_TARGET.tar.gz" \
        "bifrost-v$TARGET_VERSION-$LOCAL_TARGET"
    : > "$FAKE_PM_LOG"

    set +e
    LOCAL_OUTPUT=$(PATH="$FAKE_BIN:/usr/bin:/bin" \
        BIFROST_DATA_DIR="$TEST_ROOT/data-local" \
        BIFROST_APP_INSTALL_DIR="$TEST_ROOT/no-app" \
        BIFROST_CLI_INSTALL_SOURCE=npm \
        BIFROST_DESKTOP_MANAGED_UPGRADE_SKIP_APP=1 \
        BIFROST_DESKTOP_MANAGED_UPGRADE_SKIP_RESTART=1 \
        "$LOCAL_INSTALL_BINARY" upgrade --local-assets "$LOCAL_ASSETS_DIR" 2>&1)
    LOCAL_STATUS=$?
    set -e

    if [[ $LOCAL_STATUS -eq 0 \
        || "$LOCAL_OUTPUT" != *"--local-assets cannot update a npm-owned CLI"* \
        || -s "$FAKE_PM_LOG" ]]; then
        echo "FAIL local assets did not fail closed for an npm-owned installation" >&2
        echo "$LOCAL_OUTPUT" >&2
        echo "package-manager log:" >&2
        cat "$FAKE_PM_LOG" >&2
        exit 1
    fi
    echo "PASS local assets fail closed before contacting npm or overwriting its package tree"
fi

FAIL_INSTALL_BINARY="$TEST_ROOT/npm-failure/node_modules/@bifrost-proxy/bifrost-darwin-arm64/bin/bifrost"
mkdir -p "$(dirname "$FAIL_INSTALL_BINARY")" "$TEST_ROOT/data-failure"
cp "$BIFROST_BIN" "$FAIL_INSTALL_BINARY"
chmod +x "$FAIL_INSTALL_BINARY"
set +e
FAIL_OUTPUT=$(PATH="$FAKE_BIN:/usr/bin:/bin" \
    BIFROST_DATA_DIR="$TEST_ROOT/data-failure" \
    BIFROST_APP_INSTALL_DIR="$TEST_ROOT/no-app" \
    BIFROST_CLI_INSTALL_SOURCE=npm \
    BIFROST_DESKTOP_MANAGED_UPGRADE_SKIP_APP=1 \
    BIFROST_DESKTOP_MANAGED_UPGRADE_SKIP_RESTART=1 \
    BIFROST_UPGRADE_TEST_ALLOW_RELEASE_OVERRIDES=1 \
    BIFROST_UPGRADE_TEST_LATEST_VERSION="$TARGET_VERSION" \
    FAKE_PM_FAIL=1 \
    "$FAIL_INSTALL_BINARY" upgrade 2>&1)
FAIL_STATUS=$?
set -e

if [[ $FAIL_STATUS -eq 0 \
    || "$FAIL_OUTPUT" != *"npm upgrade failed"* \
    || "$FAIL_OUTPUT" != *"simulated npm failure"* \
    || "$FAIL_OUTPUT" == *"Replacing binary at:"* ]]; then
    echo "FAIL npm package-manager failure was not preserved" >&2
    echo "$FAIL_OUTPUT" >&2
    exit 1
fi
echo "PASS package-manager failure is actionable and never falls back to binary overwrite"
