#!/usr/bin/env bash
set -euo pipefail

# Full-shell CI discovers shell entrypoints. Keep the real listener/intent
# regression Linux-only: other platforms can perform native system proxy writes.
if [[ "$(uname -s)" != "Linux" ]]; then
  echo "SKIP: proxy recovery handoff requires isolated Linux; no host proxy changes"
  exit 0
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
export BIFROST_BIN="${BIFROST_BIN:-$PROJECT_DIR/target/release/bifrost}"

# Preserve the CI-selected binary and LLVM_PROFILE_FILE for every real child.
# exec also preserves the Python regression's failure status without a fallback.
exec python3 "$SCRIPT_DIR/test_proxy_recovery_handoff.py"
