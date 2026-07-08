#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_DIR"

echo "[im-online-notification-runner-context] running focused Rust coverage"
cargo test -p bifrost-admin online_notification_ --lib
cargo test -p bifrost-admin im_help_ --lib

echo "[im-online-notification-runner-context] PASS"
