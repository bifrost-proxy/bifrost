#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

grep -q "send_with_temporary_headed_fallback" crates/bifrost-admin/src/im_gateway/chatgpt_web/send.rs
grep -q "open_login_and_capture" crates/bifrost-admin/src/im_gateway/chatgpt_web/send.rs
grep -q "human_verification_required" crates/bifrost-admin/src/im_gateway/chatgpt_web/send.rs
grep -q "restored headless mode for current retry and future runs" crates/bifrost-admin/src/im_gateway/chatgpt_web/send.rs

SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
  ask_runs_use_shared_chatgpt_web_browser_profile_not_run_local_profile \
  --lib -- --nocapture
