#!/usr/bin/env bash
#
# E2E shell CI coverage guard.
#
# Contract:
#   - Every shell E2E script named e2e-tests/tests/test_*.sh is included in the
#     GitHub Actions shell E2E job via scripts/run_all_e2e.sh --ci --full-shell.
#   - A script may be excluded only by adding it to SKIP_IN_CI_TESTS in
#     scripts/run_all_e2e.sh with an adjacent rationale.
#
# This prevents newly added shell E2E coverage from becoming local-only by
# accident. It is intentionally cheap and does not execute the E2E scripts.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

RUNNER="scripts/run_all_e2e.sh"
TEST_DIR="e2e-tests/tests"

if [[ ! -f "$RUNNER" ]]; then
  echo "ERROR: missing $RUNNER" >&2
  exit 1
fi

read_array() {
  local array_name="$1"
  local line
  eval "$array_name=()"
  while IFS= read -r line; do
    eval "$array_name+=(\"\$line\")"
  done
}

read_array all_tests < <(
  find "$TEST_DIR" -maxdepth 1 -type f -name 'test_*.sh' -print \
    | xargs -n1 basename \
    | sort
)

read_array selected_tests < <(
  bash "$RUNNER" --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build --list-shell-tests \
    | sed '/^[[:space:]]*$/d' \
    | sort
)

read_array skipped_tests < <(
  awk '
    $0 ~ /^SKIP_IN_CI_TESTS=\(/ { in_list=1; next }
    in_list && $0 ~ /^\)/ { in_list=0; next }
    in_list {
      line=$0
      sub(/#.*/, "", line)
      while (match(line, /"[^"]+"/)) {
        item=substr(line, RSTART + 1, RLENGTH - 2)
        print item
        line=substr(line, RSTART + RLENGTH)
      }
    }
  ' "$RUNNER" | sort
)

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

printf '%s\n' "${all_tests[@]}" >"$tmp_dir/all.txt"
printf '%s\n' "${selected_tests[@]}" >"$tmp_dir/selected.txt"
printf '%s\n' "${skipped_tests[@]}" >"$tmp_dir/skipped.txt"
cat "$tmp_dir/selected.txt" "$tmp_dir/skipped.txt" | sort -u >"$tmp_dir/covered.txt"

missing=()
while IFS= read -r name; do
  [[ -n "$name" ]] || continue
  missing+=("$name")
done < <(comm -23 "$tmp_dir/all.txt" "$tmp_dir/covered.txt")

stale_selected=()
while IFS= read -r name; do
  [[ -n "$name" ]] || continue
  if [[ ! -f "$TEST_DIR/$name" ]]; then
    stale_selected+=("$name")
  fi
done <"$tmp_dir/selected.txt"

stale_skips=()
while IFS= read -r name; do
  [[ -n "$name" ]] || continue
  if [[ ! -f "$TEST_DIR/$name" ]]; then
    stale_skips+=("$name")
  fi
done <"$tmp_dir/skipped.txt"

echo "e2e shell CI coverage guard:"
echo "  discovered test_*.sh : ${#all_tests[@]}"
echo "  selected by CI       : ${#selected_tests[@]}"
echo "  explicitly skipped   : ${#skipped_tests[@]}"

if [[ ${#missing[@]} -gt 0 || ${#stale_selected[@]} -gt 0 || ${#stale_skips[@]} -gt 0 ]]; then
  echo
  echo "ERROR: shell E2E CI coverage contract failed."

  if [[ ${#missing[@]} -gt 0 ]]; then
    echo
    echo "Scripts under $TEST_DIR matching test_*.sh are neither selected by"
    echo "$RUNNER --ci --full-shell nor listed in SKIP_IN_CI_TESTS:"
    for name in "${missing[@]}"; do
      echo "  - $name"
    done
  fi

  if [[ ${#stale_selected[@]} -gt 0 ]]; then
    echo
    echo "The CI shell test selector returned non-existent scripts:"
    for name in "${stale_selected[@]}"; do
      echo "  - $name"
    done
  fi

  if [[ ${#stale_skips[@]} -gt 0 ]]; then
    echo
    echo "SKIP_IN_CI_TESTS contains non-existent scripts:"
    for name in "${stale_skips[@]}"; do
      echo "  - $name"
    done
  fi

  echo
  echo "Fix:"
  echo "  - Put new shell E2E tests in $TEST_DIR and name them test_*.sh."
  echo "  - Keep them in CI by default; scripts/run_all_e2e.sh --ci --full-shell"
  echo "    will collect them automatically."
  echo "  - Only add a script to SKIP_IN_CI_TESTS when CI cannot run it, and include"
  echo "    a nearby comment explaining the external dependency or platform reason."
  exit 1
fi

echo "OK: every test_*.sh shell E2E script is selected by CI or explicitly skipped."
