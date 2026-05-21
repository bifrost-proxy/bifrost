#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_DIR"

echo "[im-long-reply] verifying full ChatGPT Web extraction and Weixin split retry delivery"
cargo test -p bifrost-admin im_long_reply_delivery --lib -- --nocapture
