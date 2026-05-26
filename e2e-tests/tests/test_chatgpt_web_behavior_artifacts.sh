#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_DIR"

echo "[chatgpt-web-artifacts] verifying behavior-btn image/ZIP artifact extraction"
SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin chatgpt_web::tests::dom_ --lib -- --nocapture

echo "[chatgpt-web-artifacts] verifying behavior artifact download response handling"
SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin chatgpt_web_behavior --lib -- --nocapture
SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin downloaded_artifact --lib -- --nocapture
