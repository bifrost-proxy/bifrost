#!/bin/bash
#
# Remote CLI tooling regression tests.
# Covers local-only command surface checks for remote run, client selection, and file write path compatibility.
#

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

BIFROST_BIN="${BIFROST_BIN:-${PROJECT_DIR}/target/debug/bifrost}"
if [[ ! -x "$BIFROST_BIN" && -x "${PROJECT_DIR}/target/release/bifrost" ]]; then
  BIFROST_BIN="${PROJECT_DIR}/target/release/bifrost"
fi
if [[ ! -x "$BIFROST_BIN" && -x "${BIFROST_BIN}.exe" ]]; then
  BIFROST_BIN="${BIFROST_BIN}.exe"
fi

if [[ ! -x "$BIFROST_BIN" ]]; then
  cargo build --bin bifrost
fi

TEST_DATA_DIR="$(mktemp -d)"
cleanup() {
  rm -rf "$TEST_DATA_DIR"
}
trap cleanup EXIT

run_cli() {
  BIFROST_DATA_DIR="$TEST_DATA_DIR" "$BIFROST_BIN" "$@" 2>&1
}

assert_contains() {
  local haystack="$1"
  local needle="$2"
  local label="$3"
  if ! grep -Fq -- "$needle" <<<"$haystack"; then
    echo "FAIL: $label" >&2
    echo "Expected to find: $needle" >&2
    echo "Actual output:" >&2
    echo "$haystack" >&2
    exit 1
  fi
  echo "PASS: $label"
}

remote_help="$(run_cli remote --help)"
assert_contains "$remote_help" "BIFROST_REMOTE_CLIENT_ID" "remote help documents client-id env fallback"
assert_contains "$remote_help" "run" "remote help lists run subcommand"

run_help="$(run_cli remote run --help)"
assert_contains "$run_help" "--script-file" "remote run help exposes script-file"
assert_contains "$run_help" "--interpreter" "remote run help exposes interpreter"
assert_contains "$run_help" "--detach" "remote run help exposes detach"

write_help="$(run_cli remote file write --help)"
assert_contains "$write_help" "[PATH]" "file write still documents positional path"

missing_connection_output="$(run_cli remote --client-id devbox run --script-file ./q.py --interpreter python3 -- --limit 5 || true)"
assert_contains "$missing_connection_output" "no saved connection matching 'devbox'" "remote run accepts parent client-id before connection resolution"

path_alias_output="$(printf 'hello' | run_cli remote file write --path flag-path.txt --content-file - || true)"
assert_contains "$path_alias_output" "no saved connection" "file write --path compatibility reaches connection resolution"

echo "Remote CLI tooling regression tests passed."
