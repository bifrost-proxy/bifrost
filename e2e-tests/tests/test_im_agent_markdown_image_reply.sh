#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

export SKIP_FRONTEND_BUILD=1

cargo test -p bifrost-admin agent_reply_ --lib
cargo test -p bifrost-admin agent_chat_final_reply_sends_local_markdown_images_as_im_images --lib
