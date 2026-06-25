#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
  ask_runs_use_shared_chatgpt_web_browser_profile_not_run_local_profile \
  --lib -- --nocapture
