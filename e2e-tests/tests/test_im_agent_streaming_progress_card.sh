#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

cd "${REPO_ROOT}"

echo "[im-agent-streaming-progress-card] running progress card unit coverage"
cargo test -p bifrost-admin progress_card

echo "[im-agent-streaming-progress-card] running turn-end inbound drain regression"
cargo test -p bifrost-admin drain_ready_events_after_turn_preserves_late_message_as_guide

echo "[im-agent-streaming-progress-card] running bifrost-e2e renderer coverage"
cargo run -p bifrost-e2e -- \
  --test im_gateway_agent_streaming_progress_card_renderer \
  --jobs 1 \
  --timeout 120

if [[ -n "${FEISHU_APP_ID:-}" && -n "${FEISHU_APP_SECRET:-}" && -n "${FEISHU_OWNER_OPEN_ID:-}" ]]; then
  echo "[im-agent-streaming-progress-card] Feishu credentials detected; real-card run must be driven by human_tests/TC-IMA-88..90 with the connected app."
else
  echo "[im-agent-streaming-progress-card] Feishu credentials not present; skipped external Feishu API call."
  echo "[im-agent-streaming-progress-card] Local shell E2E still verified JSON 2.0 streaming card structure, fixed CardKit element ids, optional plan/tool/thinking modules, visible latest thinking content, tool duration, guide/queue footer state, visible guide acknowledgement title, mock Feishu repost/recall behavior, finished-card retention, and turn-end inbound drain behavior."
fi

echo "[im-agent-streaming-progress-card] PASS"
