#!/usr/bin/env bash

# Parse every tracked Bash script. This is deliberately repository-wide: a new
# script must enter the syntax gate automatically without updating an allowlist.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

checked=0
failed=0
while IFS= read -r script; do
  checked=$((checked + 1))
  if ! bash -n "$script"; then
    echo "ERROR: bash syntax check failed: $script" >&2
    failed=$((failed + 1))
  fi
done < <(git ls-files '*.sh' | sort)

echo "Shell syntax: checked=$checked failed=$failed"
[[ "$failed" -eq 0 ]]
