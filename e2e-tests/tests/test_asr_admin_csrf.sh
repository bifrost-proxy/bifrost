#!/usr/bin/env bash
set -euo pipefail

: "${BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT:=1}"
: "${BIFROST_DISABLE_TRAY:=1}"
export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT
export BIFROST_DISABLE_TRAY

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

echo "[asr-admin-csrf-e2e] verify ASR frontend API client injects admin CSRF"
pnpm --dir web test:unit src/api/asr.test.ts

if [[ "${SKIP_ADMIN_SECURITY_E2E:-false}" == "true" ]]; then
  echo "[asr-admin-csrf-e2e] skipped admin security E2E by SKIP_ADMIN_SECURITY_E2E=true"
  exit 0
fi

echo "[asr-admin-csrf-e2e] build current bifrost binary for admin CSRF gate regression"
CARGO_BIN="${CARGO_BIN:-$HOME/.cargo/bin/cargo}"
if [[ ! -x "$CARGO_BIN" ]]; then
  CARGO_BIN="cargo"
fi
PATH="$HOME/.cargo/bin:$PATH" SKIP_FRONTEND_BUILD=1 "$CARGO_BIN" build --bin bifrost

echo "[asr-admin-csrf-e2e] verify backend CSRF gate still rejects missing or cross-site tokens"
BIFROST_BIN="${BIFROST_BIN:-$ROOT_DIR/target/debug/bifrost}" \
  bash e2e-tests/tests/test_admin_cross_site_security.sh

echo "[asr-admin-csrf-e2e] passed"
