#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_DIR"

echo "[agent-send-msg-feishu-card] running focused Rust coverage"
cargo test -p bifrost-admin feishu_image_key_send_msg_builds_card_image_element --lib
cargo test -p bifrost-admin feishu_text_with_image_key_keeps_text_in_generated_card --lib
cargo test -p bifrost-admin rich_card_builder_uses_image_key_and_markdown --lib

echo "[agent-send-msg-feishu-card] PASS"
