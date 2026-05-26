#!/bin/bash
set -euo pipefail

unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY all_proxy ALL_PROXY no_proxy NO_PROXY

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

SKIP_BUILD="${SKIP_BUILD:-false}"
if [[ -z "${BIFROST_BIN:-}" ]]; then
    if [[ "$SKIP_BUILD" == "true" && -x "$REPO_DIR/target/release/bifrost" ]]; then
        BIFROST_BIN="$REPO_DIR/target/release/bifrost"
    else
        BIFROST_BIN="$REPO_DIR/target/debug/bifrost"
    fi
fi
TEST_DATA_DIR="$(mktemp -d)"
CALLER_DATA_DIR="$(mktemp -d)"
KEY_FILE="$TEST_DATA_DIR/remote-device.key"
EXPORTED_KEY_FILE="$TEST_DATA_DIR/exported-device.key"
STDOUT_KEY_FILE="$TEST_DATA_DIR/stdout-device.key"
OVERWRITE_OUTPUT="$TEST_DATA_DIR/overwrite.out"

cleanup() {
    rm -rf "$TEST_DATA_DIR" "$CALLER_DATA_DIR" >/dev/null 2>&1 || true
}
trap cleanup EXIT

log() { echo "[setting-ssh-key-e2e] $*"; }

if [[ "$SKIP_BUILD" != "true" || ! -x "$BIFROST_BIN" ]]; then
    log "Building bifrost debug binary..."
    (cd "$REPO_DIR" && cargo build --bin bifrost)
fi

run_target() {
    BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" "$@"
}

assert_contains() {
    local file="$1"
    local pattern="$2"
    if ! grep -q "$pattern" "$file"; then
        echo "Expected $file to contain $pattern" >&2
        exit 1
    fi
}

log "Create local remote-invoke SSH key to an output file"
create_output="$(run_target setting ssh-key create --label "ci-device" --output "$KEY_FILE")"
test -f "$KEY_FILE"
assert_contains "$KEY_FILE" "BEGIN BIFROST KEY"
assert_contains "$KEY_FILE" "Device-Code: BF-"
grep -q "Use on caller: bifrost remote conn up --ssh-key $KEY_FILE" <<<"$create_output"

if [[ "$(uname -s)" != "Windows_NT" ]]; then
    mode="$(stat -f '%Lp' "$KEY_FILE" 2>/dev/null || stat -c '%a' "$KEY_FILE")"
    if [[ "$mode" != "600" ]]; then
        echo "Expected key file mode 600, got $mode" >&2
        exit 1
    fi
fi

log "Status shows active key metadata"
status_output="$(run_target setting ssh-key status)"
grep -q "Remote-invoke SSH key" <<<"$status_output"
grep -q "Label: ci-device" <<<"$status_output"
grep -q "Device code: BF-" <<<"$status_output"

log "Export active key and refuse accidental overwrite"
run_target setting ssh-key export --output "$EXPORTED_KEY_FILE" >/dev/null
cmp "$KEY_FILE" "$EXPORTED_KEY_FILE"
if run_target setting ssh-key export --output "$EXPORTED_KEY_FILE" >"$OVERWRITE_OUTPUT" 2>&1; then
    echo "Expected export without --force to refuse existing output file" >&2
    exit 1
fi
grep -q -- "--force" "$OVERWRITE_OUTPUT"
run_target setting ssh-key export --output "$EXPORTED_KEY_FILE" --force >/dev/null

log "Create can write key material directly to stdout"
run_target setting ssh-key create --label "stdout-device" --grant-mode 1h >"$STDOUT_KEY_FILE"
assert_contains "$STDOUT_KEY_FILE" "BEGIN BIFROST KEY"
assert_contains "$STDOUT_KEY_FILE" "Device-Code: BF-"

device_code="$(awk -F': ' '/^Device-Code:/ { print $2; exit }' "$STDOUT_KEY_FILE")"
if [[ -z "$device_code" ]]; then
    echo "Failed to parse Device-Code from generated key" >&2
    exit 1
fi

log "Generated key is accepted by caller-side --ssh-key parser"
set +e
parse_output="$(
    BIFROST_DATA_DIR="$CALLER_DATA_DIR" timeout 8 "$BIFROST_BIN" \
        remote --relay-url "http://127.0.0.1:9" \
        conn up --ssh-key "$STDOUT_KEY_FILE" --device-code "$device_code" 2>&1
)"
parse_status=$?
set -e
if [[ "$parse_status" -eq 0 ]]; then
    echo "Expected remote conn up to fail against the intentionally closed relay port" >&2
    exit 1
fi
if grep -Eqi "parse ssh key|invalid.*key|device code.*mismatch" <<<"$parse_output"; then
    echo "Generated key was rejected by caller parser:" >&2
    echo "$parse_output" >&2
    exit 1
fi

log "Revoke active key"
run_target setting ssh-key revoke >/dev/null
post_revoke_status="$(run_target setting ssh-key status)"
grep -q "No active remote-invoke SSH key" <<<"$post_revoke_status"

log "PASS"
