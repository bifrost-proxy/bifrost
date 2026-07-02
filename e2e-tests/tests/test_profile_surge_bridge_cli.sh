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
INCLUDE_PROFILE="$TEST_DIR/included.conf"
RULE_SET="$TEST_DIR/rules.list"
DOMAIN_SET="$TEST_DIR/domains.list"
cat > "$INCLUDE_PROFILE" <<'PROFILE_EOF'
[Rule]
DOMAIN,included.example,DIRECT
PROFILE_EOF
cat > "$RULE_SET" <<'PROFILE_EOF'
DOMAIN-SUFFIX,ruleset.example
DOMAIN,exact.ruleset.example
PROFILE_EOF
cat > "$DOMAIN_SET" <<'PROFILE_EOF'
domainset.example
.sub.domainset.example
PROFILE_EOF
cat > "$PROFILE" <<'PROFILE_EOF'
#!include included.conf
[General]
dns-server = 8.8.8.8

[Proxy]
ProxyA = http, 127.0.0.1, 8080

[Proxy Group]
Proxy = select, ProxyA, DIRECT, MissingProxy

[MITM]
hostname = *.example.com

[Rule]
RULE-SET,rules.list,Proxy
DOMAIN-SET,domains.list,DIRECT
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
assert_body_contains "Resolved resources" "$IMPORT_OUTPUT" "dry-run import prints resolved resource summary"
assert_body_contains "rules.list" "$IMPORT_OUTPUT" "dry-run import resolves local RULE-SET"
assert_body_contains "cache sha256:" "$IMPORT_OUTPUT" "dry-run import prints local resource cache key"

echo "Running bifrost profile effective..."
EFFECTIVE_OUTPUT="$("$BIFROST_BIN" profile effective "$PROFILE")"
assert_body_contains "Surge effective profile dry-run" "$EFFECTIVE_OUTPUT" "effective prints dry-run header"
assert_body_contains "Policy graph" "$EFFECTIVE_OUTPUT" "effective prints policy graph"
assert_body_contains "missing members: MissingProxy" "$EFFECTIVE_OUTPUT" "effective reports missing policy members"
assert_body_contains "RULE-SET:rules.list" "$EFFECTIVE_OUTPUT" "effective expands local RULE-SET"
assert_body_contains "DOMAIN-SET:domains.list" "$EFFECTIVE_OUTPUT" "effective expands local DOMAIN-SET"

echo "Running bifrost profile explain..."
EXPLAIN_OUTPUT="$("$BIFROST_BIN" profile explain --profile "$PROFILE" "https://sub.example.com/path")"
assert_body_contains "Matched: line" "$EXPLAIN_OUTPUT" "explain prints matched rule line"
assert_body_contains "DOMAIN-SUFFIX" "$EXPLAIN_OUTPUT" "explain uses ordered DOMAIN-SUFFIX first match"
assert_body_contains "Selected policy Proxy" "$EXPLAIN_OUTPUT" "explain prints selected policy"

RESOURCE_EXPLAIN_OUTPUT="$("$BIFROST_BIN" profile explain --profile "$PROFILE" "https://api.ruleset.example/path")"
assert_body_contains "RULE-SET:rules.list" "$RESOURCE_EXPLAIN_OUTPUT" "explain evaluates expanded RULE-SET before FINAL"
assert_body_contains "Selected policy Proxy" "$RESOURCE_EXPLAIN_OUTPUT" "expanded RULE-SET keeps parent policy"

echo "Running bifrost profile convert..."
CONVERT_OUTPUT="$("$BIFROST_BIN" profile convert "$PROFILE" --to bifrost)"
assert_body_contains "Bifrost Native Profile Preview" "$CONVERT_OUTPUT" "convert prints native profile preview"
assert_body_contains "host suffix example.com -> Proxy" "$CONVERT_OUTPUT" "convert previews suffix rule"
assert_body_contains "host suffix ruleset.example -> Proxy" "$CONVERT_OUTPUT" "convert includes expanded RULE-SET rule"
assert_body_contains "Compatibility summary" "$CONVERT_OUTPUT" "convert prints compatibility summary"

print_test_summary
