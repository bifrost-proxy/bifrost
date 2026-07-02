#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
source "$ROOT_DIR/e2e-tests/test_utils/assert.sh"

TEST_DIR="$(mktemp -d)"
cleanup() {
  rm -rf "$TEST_DIR"
}
trap cleanup EXIT

export BIFROST_DATA_DIR="$TEST_DIR/data"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
export BIFROST_DISABLE_TRAY=1

PROFILE="$TEST_DIR/surge.conf"
cat > "$PROFILE" <<'PROFILE_EOF'
[General]
dns-server = 8.8.8.8

[Proxy]
ProxyA = http, 127.0.0.1, 8080

[Proxy Group]
Proxy = select, ProxyA, DIRECT

[MITM]
hostname = *.example.com

[Rule]
DOMAIN,exact.example.com,DIRECT
DOMAIN-SUFFIX,example.com,Proxy
DOMAIN-KEYWORD,google,DIRECT
GEOIP,US,DIRECT
FINAL,ProxyA
PROFILE_EOF

echo "Building bifrost CLI for Surge Bridge E2E..."
cargo build --bin bifrost
BIFROST_BIN="$ROOT_DIR/target/debug/bifrost"

echo "Running bifrost profile import --dry-run..."
IMPORT_OUTPUT="$("$BIFROST_BIN" profile import "$PROFILE" --dry-run)"
assert_body_contains "Surge profile dry-run import" "$IMPORT_OUTPUT" "dry-run import prints report header"
assert_body_contains "DOMAIN-SUFFIX" "$IMPORT_OUTPUT" "dry-run import reports DOMAIN-SUFFIX capability"
assert_body_contains "Not supported yet" "$IMPORT_OUTPUT" "dry-run import reports unsupported items"

echo "Running bifrost profile explain..."
EXPLAIN_OUTPUT="$("$BIFROST_BIN" profile explain --profile "$PROFILE" "https://sub.example.com/path")"
assert_body_contains "Matched: line" "$EXPLAIN_OUTPUT" "explain prints matched rule line"
assert_body_contains "DOMAIN-SUFFIX" "$EXPLAIN_OUTPUT" "explain uses ordered DOMAIN-SUFFIX first match"
assert_body_contains "Selected policy Proxy" "$EXPLAIN_OUTPUT" "explain prints selected policy"

echo "Running bifrost profile convert..."
CONVERT_OUTPUT="$("$BIFROST_BIN" profile convert "$PROFILE" --to bifrost)"
assert_body_contains "Bifrost Native Profile Preview" "$CONVERT_OUTPUT" "convert prints native profile preview"
assert_body_contains "host suffix example.com -> Proxy" "$CONVERT_OUTPUT" "convert previews suffix rule"
assert_body_contains "Compatibility summary" "$CONVERT_OUTPUT" "convert prints compatibility summary"

print_test_summary
