#!/usr/bin/env bash
set -euo pipefail
# Reuse the audited, CI-guarded native fixture under the ordinary runner UID.
# Its sudo command is restricted to final journal-based fixture cleanup only.
export BIFROST_NATIVE_PROXY_MODE=nonroot-recovery
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec /bin/bash "$SCRIPT_DIR/test_system_proxy_reconcile_stability.sh"
