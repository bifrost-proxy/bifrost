#!/bin/bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
source "$ROOT_DIR/e2e-tests/test_utils/assert.sh"

TEST_DIR="$(mktemp -d)"
cleanup() {
  if [[ -n "${REMOTE_PID:-}" ]]; then
    kill "$REMOTE_PID" >/dev/null 2>&1 || true
    wait "$REMOTE_PID" >/dev/null 2>&1 || true
  fi
  rm -rf "$TEST_DIR"
}
trap cleanup EXIT

export BIFROST_DATA_DIR="$TEST_DIR/data"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
export BIFROST_DISABLE_TRAY=1

PROFILE="$TEST_DIR/surge.conf"
NATIVE_PROFILE="$TEST_DIR/native.bifrost-profile.toml"
INCLUDE_PROFILE="$TEST_DIR/included.conf"
RULE_SET="$TEST_DIR/rules.list"
DOMAIN_SET="$TEST_DIR/domains.list"
REMOTE_DIR="$TEST_DIR/remote"
REMOTE_PORT_FILE="$TEST_DIR/remote-port"
mkdir -p "$REMOTE_DIR"
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
cat > "$REMOTE_DIR/remote-include.conf" <<'PROFILE_EOF'
[Rule]
DOMAIN,remote-include.example,DIRECT
PROFILE_EOF
cat > "$REMOTE_DIR/remote-rules.list" <<'PROFILE_EOF'
DOMAIN-SUFFIX,remote-ruleset.example
PROFILE_EOF
cat > "$REMOTE_DIR/remote-domains.list" <<'PROFILE_EOF'
remote-domainset.example
PROFILE_EOF
cat > "$REMOTE_DIR/managed.conf" <<'PROFILE_EOF'
[Rule]
DOMAIN,managed.example,DIRECT
FINAL,DIRECT
PROFILE_EOF

python3 - "$REMOTE_DIR" "$REMOTE_PORT_FILE" <<'PY' &
import sys
from http.server import ThreadingHTTPServer, SimpleHTTPRequestHandler

root = sys.argv[1]
port_file = sys.argv[2]

class Handler(SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=root, **kwargs)

    def log_message(self, fmt, *args):
        return

    def end_headers(self):
        self.send_header("ETag", '"surge-remote-v1"')
        self.send_header("Last-Modified", "Wed, 02 Jul 2026 00:00:00 GMT")
        super().end_headers()

    def do_GET(self):
        if self.path == "/health":
            body = b"ok"
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        if self.headers.get("If-None-Match") == '"surge-remote-v1"':
            self.send_response(304)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        return super().do_GET()

server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
with open(port_file, "w", encoding="utf-8") as fh:
    fh.write(str(server.server_address[1]))
server.serve_forever()
PY
REMOTE_PID=$!
for _ in {1..50}; do
  if [[ -s "$REMOTE_PORT_FILE" ]]; then
    break
  fi
  sleep 0.1
done
REMOTE_PORT="$(cat "$REMOTE_PORT_FILE")"
REMOTE_BASE="http://127.0.0.1:${REMOTE_PORT}"
curl -sf "${REMOTE_BASE}/health" >/dev/null

cat > "$PROFILE" <<'PROFILE_EOF'
#!include included.conf
[General]
dns-server = 8.8.8.8

[Host]
api.hosted.example = 203.0.113.10

[Proxy]
ProxyA = http, 127.0.0.1, 8080

[Proxy Group]
Proxy = select, ProxyA, DIRECT, MissingProxy
Auto = url-test, ProxyA, DIRECT, url=http://example.com/generate_204

[MITM]
hostname = %APPEND% *.example.com, -private.example.com

[URL Rewrite]
^https://rewrite\.example/path https://target.example/path 302

[Map Local]
^https://assets\.example/app\.js data/app.js

[Header Rewrite]
^https://headers\.example header-replace User-Agent Bifrost

[Script]
http-response ^https://script\.example script-path=scripts/response.js

[Rule]
RULE-SET,rules.list,Proxy
DOMAIN-SET,domains.list,DIRECT
DOMAIN,api.hosted.example,DIRECT
DOMAIN,auto.example,Auto
DOMAIN,rewrite.example,DIRECT
DOMAIN,assets.example,DIRECT
DOMAIN,headers.example,DIRECT
DOMAIN,script.example,DIRECT
DOMAIN,private.example.com,DIRECT
DOMAIN,exact.example.com,DIRECT
DOMAIN-SUFFIX,example.com,Proxy
DOMAIN-KEYWORD,google,DIRECT
GEOIP,US,DIRECT
FINAL,ProxyA
PROFILE_EOF
{
  echo "#!include ${REMOTE_BASE}/remote-include.conf"
  echo "[Rule]"
  echo "RULE-SET,${REMOTE_BASE}/remote-rules.list,Proxy"
  echo "DOMAIN-SET,${REMOTE_BASE}/remote-domains.list,DIRECT"
} >> "$PROFILE"

cat > "$NATIVE_PROFILE" <<'PROFILE_EOF'
[profile]
name = "native-e2e"
version = 1

[[policies]]
name = "ProxyA"
type = "proxy"
url = "http://127.0.0.1:8080"

[[policy_groups]]
name = "Auto"
type = "url-test"
policies = ["ProxyA", "DIRECT", "MissingProxy"]

[[rules]]
match = { domain = "api.example.com" }
policy = "DIRECT"

[[rules]]
match = { domain_suffix = "example.com" }
policy = "Auto"

[[rules]]
match = { final = true }
policy = "REJECT"

[dns]
servers = ["https://dns.example/dns-query"]

[dns.hosts]
"api.example.com" = "203.0.113.10"

[mitm]
include = ["*.example.com"]
exclude = ["private.example.com"]

[[http_pipeline]]
match = "^https://rewrite\\.example/path"
action = "redirect"
value = "https://target.example/path"
PROFILE_EOF

echo "Building bifrost CLI for Surge Bridge E2E..."
cargo build --bin bifrost
BIFROST_BIN="$ROOT_DIR/target/debug/bifrost"

echo "Running bifrost profile native validate..."
NATIVE_VALIDATE_OUTPUT="$("$BIFROST_BIN" profile native validate "$NATIVE_PROFILE")"
assert_body_contains "Bifrost Native Profile validate" "$NATIVE_VALIDATE_OUTPUT" "native validate prints header"
assert_body_contains "Plan: sha256:" "$NATIVE_VALIDATE_OUTPUT" "native validate prints stable plan id"
assert_body_contains "Mode: bifrost-native-dry-run" "$NATIVE_VALIDATE_OUTPUT" "native validate prints runtime mode"
assert_body_contains "Runtime plan: 1 proxies, 1 policy groups, 3 rules, 2 dns entries, 2 mitm entries, 1 pipeline entries" "$NATIVE_VALIDATE_OUTPUT" "native validate prints runtime counts"
assert_body_contains "native.policy_group.missing_member" "$NATIVE_VALIDATE_OUTPUT" "native validate reports missing policy group members"
assert_body_contains "dry-run-only" "$NATIVE_VALIDATE_OUTPUT" "native validate stays non-activating"

echo "Running bifrost profile native effective..."
NATIVE_EFFECTIVE_OUTPUT="$("$BIFROST_BIN" profile native effective "$NATIVE_PROFILE")"
assert_body_contains "Bifrost Native Profile effective" "$NATIVE_EFFECTIVE_OUTPUT" "native effective prints header"
assert_body_contains "Policies" "$NATIVE_EFFECTIVE_OUTPUT" "native effective prints policies"
assert_body_contains "Policy graph" "$NATIVE_EFFECTIVE_OUTPUT" "native effective prints policy graph"
assert_body_contains "missing members: MissingProxy" "$NATIVE_EFFECTIVE_OUTPUT" "native effective prints missing group members"
assert_body_contains "Ordered rules" "$NATIVE_EFFECTIVE_OUTPUT" "native effective prints ordered rules"
assert_body_contains "DNS entries" "$NATIVE_EFFECTIVE_OUTPUT" "native effective prints DNS entries"
assert_body_contains "MITM entries" "$NATIVE_EFFECTIVE_OUTPUT" "native effective prints MITM entries"
assert_body_contains "HTTP Pipeline entries" "$NATIVE_EFFECTIVE_OUTPUT" "native effective prints HTTP Pipeline entries"

echo "Running bifrost profile import --dry-run..."
IMPORT_OUTPUT="$("$BIFROST_BIN" profile import "$PROFILE" --dry-run)"
assert_body_contains "Surge profile dry-run import" "$IMPORT_OUTPUT" "dry-run import prints report header"
assert_body_contains "DOMAIN-SUFFIX" "$IMPORT_OUTPUT" "dry-run import reports DOMAIN-SUFFIX capability"
assert_body_contains "Not supported yet" "$IMPORT_OUTPUT" "dry-run import reports unsupported items"
assert_body_contains "Resolved resources" "$IMPORT_OUTPUT" "dry-run import prints resolved resource summary"
assert_body_contains "rules.list" "$IMPORT_OUTPUT" "dry-run import resolves local RULE-SET"
assert_body_contains "cache sha256:" "$IMPORT_OUTPUT" "dry-run import prints local resource cache key"
assert_body_contains "remote-rules.list" "$IMPORT_OUTPUT" "dry-run import fetches remote RULE-SET"
assert_body_contains "etag \"surge-remote-v1\"" "$IMPORT_OUTPUT" "dry-run import prints remote ETag"

echo "Running bifrost profile effective..."
EFFECTIVE_OUTPUT="$("$BIFROST_BIN" profile effective "$PROFILE")"
assert_body_contains "Surge effective profile dry-run" "$EFFECTIVE_OUTPUT" "effective prints dry-run header"
assert_body_contains "Policy graph" "$EFFECTIVE_OUTPUT" "effective prints policy graph"
assert_body_contains "missing members: MissingProxy" "$EFFECTIVE_OUTPUT" "effective reports missing policy members"
assert_body_contains "RULE-SET:rules.list" "$EFFECTIVE_OUTPUT" "effective expands local RULE-SET"
assert_body_contains "DOMAIN-SET:domains.list" "$EFFECTIVE_OUTPUT" "effective expands local DOMAIN-SET"
assert_body_contains "RULE-SET:${REMOTE_BASE}/remote-rules.list" "$EFFECTIVE_OUTPUT" "effective expands remote RULE-SET"
assert_body_contains "DOMAIN-SET:${REMOTE_BASE}/remote-domains.list" "$EFFECTIVE_OUTPUT" "effective expands remote DOMAIN-SET"
assert_body_contains "cache-hit" "$EFFECTIVE_OUTPUT" "effective reuses cached remote resources with conditional request"

echo "Running bifrost profile explain..."
EXPLAIN_OUTPUT="$("$BIFROST_BIN" profile explain --profile "$PROFILE" "https://sub.example.com/path")"
assert_body_contains "Matched: line" "$EXPLAIN_OUTPUT" "explain prints matched rule line"
assert_body_contains "DOMAIN-SUFFIX" "$EXPLAIN_OUTPUT" "explain uses ordered DOMAIN-SUFFIX first match"
assert_body_contains "Selected policy Proxy" "$EXPLAIN_OUTPUT" "explain prints selected policy"
assert_body_contains "Policy decision: Proxy -> ProxyA (terminal ProxyA; proxy endpoint)" "$EXPLAIN_OUTPUT" "explain resolves select group to terminal proxy"
assert_body_contains "MITM decision: host sub.example.com is included in MITM dry-run scope" "$EXPLAIN_OUTPUT" "explain reports MITM include scope"

RESOURCE_EXPLAIN_OUTPUT="$("$BIFROST_BIN" profile explain --profile "$PROFILE" "https://api.ruleset.example/path")"
assert_body_contains "RULE-SET:rules.list" "$RESOURCE_EXPLAIN_OUTPUT" "explain evaluates expanded RULE-SET before FINAL"
assert_body_contains "Selected policy Proxy" "$RESOURCE_EXPLAIN_OUTPUT" "expanded RULE-SET keeps parent policy"

AUTO_EXPLAIN_OUTPUT="$("$BIFROST_BIN" profile explain --profile "$PROFILE" "https://auto.example/path")"
assert_body_contains "Policy decision: Auto -> ProxyA (terminal ProxyA; proxy endpoint)" "$AUTO_EXPLAIN_OUTPUT" "explain resolves url-test group in dry-run"
assert_body_contains "active latency probing is not running" "$AUTO_EXPLAIN_OUTPUT" "explain reports url-test dry-run health boundary"

DNS_EXPLAIN_OUTPUT="$("$BIFROST_BIN" profile explain --profile "$PROFILE" "https://api.hosted.example/path")"
assert_body_contains "DNS decision: Host mapping api.hosted.example -> 203.0.113.10" "$DNS_EXPLAIN_OUTPUT" "explain reports Host mapping decision"

MITM_EXCLUDE_OUTPUT="$("$BIFROST_BIN" profile explain --profile "$PROFILE" "https://private.example.com/path")"
assert_body_contains "MITM decision: host private.example.com is excluded from MITM" "$MITM_EXCLUDE_OUTPUT" "explain reports MITM exclusion"

PIPELINE_EXPLAIN_OUTPUT="$("$BIFROST_BIN" profile explain --profile "$PROFILE" "https://rewrite.example/path")"
assert_body_contains "HTTP pipeline: 1 matched" "$PIPELINE_EXPLAIN_OUTPUT" "explain reports matched HTTP pipeline count"
assert_body_contains "matched [URL Rewrite]" "$PIPELINE_EXPLAIN_OUTPUT" "explain reports URL Rewrite pipeline match"

SCRIPT_EXPLAIN_OUTPUT="$("$BIFROST_BIN" profile explain --profile "$PROFILE" "https://script.example/path")"
assert_body_contains "matched [Script]" "$SCRIPT_EXPLAIN_OUTPUT" "explain reports Script pipeline match"

echo "Running bifrost profile convert..."
CONVERT_OUTPUT="$("$BIFROST_BIN" profile convert "$PROFILE" --to bifrost)"
assert_body_contains "Bifrost Native Profile Preview" "$CONVERT_OUTPUT" "convert prints native profile preview"
assert_body_contains "host suffix example.com -> Proxy" "$CONVERT_OUTPUT" "convert previews suffix rule"
assert_body_contains "host suffix ruleset.example -> Proxy" "$CONVERT_OUTPUT" "convert includes expanded RULE-SET rule"
assert_body_contains "host suffix remote-ruleset.example -> Proxy" "$CONVERT_OUTPUT" "convert includes expanded remote RULE-SET rule"
assert_body_contains "Compatibility summary" "$CONVERT_OUTPUT" "convert prints compatibility summary"

echo "Running bifrost profile import save for disabled rule review..."
SAVE_OUTPUT="$("$BIFROST_BIN" profile import "$PROFILE" --name profile/surge-e2e)"
assert_body_contains "Saved Bifrost rule 'profile/surge-e2e' [disabled for review]" "$SAVE_OUTPUT" "non-dry-run import saves disabled rule"
RULE_SHOW_OUTPUT="$("$BIFROST_BIN" rule show profile/surge-e2e)"
assert_body_contains "Status: disabled" "$RULE_SHOW_OUTPUT" "saved Surge import rule is disabled by default"
assert_body_contains "api.hosted.example passthrough://" "$RULE_SHOW_OUTPUT" "saved rule contains DIRECT passthrough conversion"
assert_body_contains "*.example.com proxy://http://127.0.0.1:8080" "$RULE_SHOW_OUTPUT" "saved rule contains proxy endpoint conversion"
assert_body_contains "/.*/ proxy://http://127.0.0.1:8080" "$RULE_SHOW_OUTPUT" "saved rule contains FINAL proxy conversion"
assert_body_contains "/^https:\\/\\/rewrite\\.example\\/path/ redirect://302:https://target.example/path" "$RULE_SHOW_OUTPUT" "saved rule contains URL Rewrite redirect conversion"
assert_body_contains "/^https:\\/\\/assets\\.example\\/app\\.js/ file://data/app.js" "$RULE_SHOW_OUTPUT" "saved rule contains Map Local file conversion"
assert_body_contains "Header Rewrite requires request/response/header-scope review before activation" "$RULE_SHOW_OUTPUT" "saved rule keeps Header Rewrite as manual review comment"
assert_body_contains "Script entries reference external JavaScript that must be imported into Bifrost scripts before activation" "$RULE_SHOW_OUTPUT" "saved rule keeps Script as manual review comment"

echo "Running bifrost profile effective for managed profile URL..."
MANAGED_OUTPUT="$("$BIFROST_BIN" profile effective "${REMOTE_BASE}/managed.conf")"
assert_body_contains "Source: ${REMOTE_BASE}/managed.conf" "$MANAGED_OUTPUT" "managed profile URL is accepted as profile source"
assert_body_contains "ManagedProfile" "$MANAGED_OUTPUT" "managed profile URL is tracked as a resource"
assert_body_contains "managed.example" "$MANAGED_OUTPUT" "managed profile URL rules are loaded into runtime plan"

print_test_summary
