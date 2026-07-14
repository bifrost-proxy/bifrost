#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

grep -q "send_with_temporary_headed_fallback" crates/bifrost-admin/src/im_gateway/chatgpt_web/send.rs
grep -q "open_login_and_capture" crates/bifrost-admin/src/im_gateway/chatgpt_web/send.rs
grep -q "human_verification_required" crates/bifrost-admin/src/im_gateway/chatgpt_web/send.rs
grep -q "restored headless mode for current retry and future runs" crates/bifrost-admin/src/im_gateway/chatgpt_web/send.rs
grep -q "required_dom_terminal_idle_for" crates/bifrost-admin/src/im_gateway/chatgpt_web/interaction.rs
grep -q "dom_terminal_content_settle_for" crates/bifrost-admin/src/im_gateway/chatgpt_web/interaction.rs
grep -q "stop_button_visible" crates/bifrost-admin/src/im_gateway/chatgpt_web/interaction.rs
grep -q "assistant_message_not_committed" crates/bifrost-admin/src/im_gateway/chatgpt_web/interaction.rs
grep -q "provisionalAssistantShell" crates/bifrost-admin/src/im_gateway/chatgpt_web/interaction.rs
grep -q "hasGeneratedImageAfterLastUser" crates/bifrost-admin/src/im_gateway/chatgpt_web/interaction.rs
grep -q "conversation_busy" crates/bifrost-admin/src/im_gateway/chatgpt_web/send.rs
grep -q "conversation_busy_if_stop_button_visible" crates/bifrost-admin/src/im_gateway/chatgpt_web/send.rs
grep -q "diagnostic_has_visible_stop_button" crates/bifrost-admin/src/im_gateway/chatgpt_web/send.rs
grep -q "recover_conversation_tab_from_browser" crates/bifrost-admin/src/im_gateway/chatgpt_web/browser.rs
grep -q "wait_chatgpt_web_daily_agent_conversation" crates/bifrost-admin/src/handlers/asr_jobs/daily_agent.rs
grep -q "daily_agent_chatgpt_web_same_conversation_wait_timeout_ms" crates/bifrost-admin/src/handlers/asr_jobs/daily_agent.rs
grep -q "tomorrow_todo_target_date" crates/bifrost-admin/src/handlers/asr_jobs/daily_agent.rs
grep -q "Tomorrow ToDo 日期规则" crates/bifrost-admin/src/handlers/asr_jobs/daily_agent_prompt.rs

SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
  ask_runs_use_shared_chatgpt_web_browser_profile_not_run_local_profile \
  --lib -- --nocapture

SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
  dom_output_in_progress_reason_uses_stop_button_before_text_state \
  --lib -- --nocapture

SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
  dom_terminal_content_settle_waits_after_controls_idle_for_markdown_render \
  --lib -- --nocapture

SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
  dom_output_in_progress_reason_waits_for_committed_assistant_message \
  --lib -- --nocapture

SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
  dom_ready_signature_detects_equal_length_content_replacement \
  --lib -- --nocapture

SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
  is_retryable_send_error_matches_known_prefixes \
  --lib -- --nocapture

SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
  diagnostic_has_visible_stop_button_is_the_busy_gate \
  --lib -- --nocapture

SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
  daily_agent_chatgpt_web_same_conversation_wait_uses_daily_timeout_with_headroom \
  --lib -- --nocapture

SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
  daily_agent_chatgpt_web_tomorrow_todo \
  --lib -- --nocapture
