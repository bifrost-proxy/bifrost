#!/usr/bin/env bash
#
# E2E launch guard lint.
#
# Background: launching the full `bifrost` binary spawns the macOS tray helper
# and (when sync is unconfigured) the auto-login browser window. During testing
# these MUST be disabled, otherwise running a test matrix concurrently opens
# dozens of tray processes and login pages and overwhelms the machine.
#
# The repository already provides two guards:
#   - BIFROST_DISABLE_TRAY=1                       (suppresses the tray helper)
#   - BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1     (suppresses the login browser)
# They are exported by the umbrella runner scripts/run_all_e2e.sh and defaulted
# in the shared helper e2e-tests/test_utils/process.sh.
#
# This lint enforces that EVERY shell script that launches `bifrost start` or
# `bifrost run` carries those guards itself (either sets the env vars directly,
# or sources the shared process.sh helper). That way an ad-hoc script run
# OUTSIDE the umbrella runner can never spawn trays / login pages.
#
# Exit non-zero (with an actionable report) if any launching script is missing
# the guard. Designed to be cheap and run locally before commit (NOT a CI job).

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

# Directories to scan. Test/automation shell scripts live here.
SCAN_DIRS=(e2e-tests scripts tests)

# A line is considered a "full bifrost launch" when it invokes the bifrost
# binary (literal `bifrost`, $BIFROST_BIN, or a path ending in /bifrost) with
# the `start` or `run` subcommand. We deliberately ignore `remote`, `status`,
# `stop`, etc. since those do not spawn desktop UI.
LAUNCH_RE='(\bbifrost|\$\{?BIFROST_BIN\}?|/bifrost)"? +(start|run)([ "]|$)'

# Guard tokens that make a script safe.
GUARD_RE='BIFROST_DISABLE_TRAY|BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT|test_utils/process\.sh|run_all_e2e\.sh'

# Allowlist: scripts that legitimately exercise tray/login behaviour and
# therefore manage the env themselves on a per-launch basis. They must STILL
# set the guard somewhere; this list only documents intentional exceptions if
# ever needed. Keep empty by default.
ALLOWLIST=()

is_allowlisted() {
  local f="$1"
  local a
  for a in "${ALLOWLIST[@]:-}"; do
    [[ "$f" == "$a" ]] && return 0
  done
  return 1
}

mapfile -t candidates < <(
  grep -rlnE "$LAUNCH_RE" --include='*.sh' "${SCAN_DIRS[@]}" 2>/dev/null | sort -u
)

violations=()
checked=0
for f in "${candidates[@]}"; do
  [[ -z "$f" ]] && continue
  is_allowlisted "$f" && continue
  checked=$((checked + 1))
  if ! grep -qE "$GUARD_RE" "$f"; then
    violations+=("$f")
  fi
done

echo "e2e launch guard lint: scanned ${checked} script(s) that launch 'bifrost start|run'"

if [[ ${#violations[@]} -gt 0 ]]; then
  echo
  echo "ERROR: the following scripts launch the full bifrost binary but do NOT"
  echo "carry the tray/login guard. Running them outside scripts/run_all_e2e.sh"
  echo "would spawn the macOS tray helper and the auto-login browser window."
  echo
  for v in "${violations[@]}"; do
    echo "  - $v"
  done
  echo
  echo "Fix: add ONE of the following to the script before launching bifrost:"
  echo "  1. source the shared helper:"
  echo "       source \"\$(dirname \"\${BASH_SOURCE[0]}\")/../test_utils/process.sh\""
  echo "  2. or export the guards explicitly:"
  echo "       export BIFROST_DISABLE_TRAY=1"
  echo "       export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1"
  echo
  echo "See AGENTS.md > '测试启动红线' for the full rule."
  exit 1
fi

echo "OK: all launching scripts carry the tray/login guard."
exit 0
