#!/usr/bin/env bash
# Guard against reintroduction of legacy `bifrost remote` CLI subcommands.
#
# Background: commit bb4d933a restructured `bifrost remote` into four privilege
# tiers as a hard cut (no deprecation aliases). Subsequent fix-up commits kept
# missing stragglers in e2e scripts and helper scripts, causing CI to fail on
# all shell-e2e shards. This script scans the repo for any leftover legacy
# invocation patterns and fails the build if found.
#
# Run locally: bash scripts/ci/check-remote-cli-legacy.sh
# Wired into CI via the `lint` job in .github/workflows/ci.yml.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

# Legacy -> new mapping (for reviewer reference):
#   remote connect        -> remote conn up
#   remote disconnect     -> remote conn down
#   remote status         -> remote conn status
#   remote search         -> remote traffic search
#   remote command exec   -> remote exec
#   remote file search    -> remote file find
#   remote file mv        -> remote file move
#   remote file rm        -> remote file delete
#   remote file apply-patch -> remote file patch

SCAN_PATHS=(
  "e2e-tests"
  "scripts"
  "crates/bifrost-cli"
)

# Skip this guard file itself (it must reference the patterns it forbids)
# plus CHANGELOGs / git metadata.
EXCLUDE_REGEX='(scripts/ci/check-remote-cli-legacy\.sh|CHANGELOG|\.git/)'

# Forbidden patterns. Separator between regex and description is '@@'
# (avoid '|' because it also appears inside the extended regex alternations).
declare -a FORBIDDEN=(
  '(\$\{?BIFROST_BIN\}?|run_bifrost|bifrost)[[:space:]]+remote[[:space:]]+connect([[:space:]]|$)@@legacy: use `remote conn up`'
  '(\$\{?BIFROST_BIN\}?|run_bifrost|bifrost)[[:space:]]+remote[[:space:]]+disconnect([[:space:]]|$)@@legacy: use `remote conn down`'
  '(\$\{?BIFROST_BIN\}?|run_bifrost|bifrost)[[:space:]]+remote[[:space:]]+status([[:space:]]|$)@@legacy: use `remote conn status`'
  '(\$\{?BIFROST_BIN\}?|run_bifrost|bifrost)[[:space:]]+remote[[:space:]]+search([[:space:]]|$)@@legacy: use `remote traffic search`'
  'remote[[:space:]]+command[[:space:]]+exec([[:space:]]|$)@@legacy: use `remote exec`'
  'remote[[:space:]]+file[[:space:]]+search([[:space:]]|$)@@legacy: use `remote file find`'
  'remote[[:space:]]+file[[:space:]]+mv([[:space:]]|$)@@legacy: use `remote file move`'
  'remote[[:space:]]+file[[:space:]]+rm([[:space:]]|$)@@legacy: use `remote file delete`'
  'remote[[:space:]]+file[[:space:]]+apply-patch([[:space:]]|$)@@legacy: use `remote file patch`'
)

echo "==> Scanning for legacy \`bifrost remote\` CLI usages..."
fail_count=0
for entry in "${FORBIDDEN[@]}"; do
  pattern="${entry%%@@*}"
  desc="${entry##*@@}"
  set +e
  matches="$(grep -rnE "$pattern" "${SCAN_PATHS[@]}" 2>/dev/null \
              | grep -vE "$EXCLUDE_REGEX" || true)"
  set -e
  if [[ -n "$matches" ]]; then
    echo ""
    echo "❌ Found legacy pattern ($desc):"
    echo "$matches" | sed 's/^/    /'
    fail_count=$((fail_count + 1))
  fi
done

if [[ "$fail_count" -gt 0 ]]; then
  echo ""
  echo "❌ $fail_count legacy pattern(s) detected."
  echo "   Please update to the new \`bifrost remote\` CLI surface (see mapping in this script's header)."
  exit 1
fi

echo "✓ No legacy \`bifrost remote\` CLI patterns detected."
