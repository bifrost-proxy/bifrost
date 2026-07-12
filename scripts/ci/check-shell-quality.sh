#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

BASE_REF="${1:-${BIFROST_SHELL_FORMAT_BASE_REF:-}}"

bash scripts/ci/check-shell-syntax.sh

require_tool() {
  local tool="$1"
  if command -v "$tool" >/dev/null 2>&1; then
    return 0
  fi
  if [[ "${CI:-}" == "true" || "${BIFROST_REQUIRE_SHELL_TOOLS:-0}" == "1" ]]; then
    echo "ERROR: required Shell quality tool is missing: $tool" >&2
    return 1
  fi
  echo "WARN: $tool is not installed; skipping its local-only check" >&2
  return 2
}

mapfile_compat() {
  local destination="$1"
  shift
  eval "$destination=()"
  local line
  while IFS= read -r line; do
    [[ -n "$line" ]] && eval "$destination+=(\"\$line\")"
  done < <("$@")
}

mapfile_compat shell_files git ls-files '*.sh'

if require_tool shellcheck; then
  shellcheck --severity=error "${shell_files[@]}"
  echo "ShellCheck: PASS (${#shell_files[@]} files, severity=error)"
else
  status=$?
  [[ "$status" -eq 2 ]] || exit "$status"
fi

list_changed_shell_files() {
  if [[ -n "$BASE_REF" ]]; then
    git rev-parse --verify "$BASE_REF^{commit}" >/dev/null
    git diff --name-only --diff-filter=AM "$BASE_REF...HEAD" -- 'scripts/ci/*.sh'
  fi
  git ls-files --others --exclude-standard -- 'scripts/ci/*.sh'
}

mapfile_compat changed_shell_files list_changed_shell_files

if ((${#changed_shell_files[@]} > 0)); then
  if require_tool shfmt; then
    shfmt -d -i 2 -ci "${changed_shell_files[@]}"
    echo "shfmt: PASS (${#changed_shell_files[@]} changed scripts/ci files)"
  else
    status=$?
    [[ "$status" -eq 2 ]] || exit "$status"
  fi
else
  echo "shfmt: no changed Shell files"
fi
