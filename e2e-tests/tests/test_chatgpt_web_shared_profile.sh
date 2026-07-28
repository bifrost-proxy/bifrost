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
grep -q "const chatLabels = new Set(\\['Chat', '聊天'\\])" crates/bifrost-admin/src/im_gateway/chatgpt_web/send.rs
grep -q "const workLabels = new Set(\\['Work', '工作'\\])" crates/bifrost-admin/src/im_gateway/chatgpt_web/send.rs
grep -q "\\['on', 'checked', 'active'\\].includes(dataState)" crates/bifrost-admin/src/im_gateway/chatgpt_web/send.rs
grep -q "recover_conversation_tab_from_browser" crates/bifrost-admin/src/im_gateway/chatgpt_web/browser.rs
grep -q "wait_chatgpt_web_daily_agent_conversation" crates/bifrost-admin/src/handlers/asr_jobs/daily_agent.rs
grep -q "daily_agent_chatgpt_web_same_conversation_wait_timeout_ms" crates/bifrost-admin/src/handlers/asr_jobs/daily_agent.rs
grep -q "ASR daily agent entry failed; continuing with remaining entries" crates/bifrost-admin/src/handlers/asr_jobs/daily_agent.rs
grep -q "partial_success" crates/bifrost-admin/src/handlers/asr_jobs/daily_agent.rs
grep -q "tomorrow_todo_target_date" crates/bifrost-admin/src/handlers/asr_jobs/daily_agent.rs
grep -q "Tomorrow ToDo 日期规则" crates/bifrost-admin/src/handlers/asr_jobs/daily_agent_prompt.rs
if grep -q "bifrost_new_chat" crates/bifrost-admin/src/im_gateway/chatgpt_web/send.rs; then
  echo "ChatGPT Web new-conversation URL must not expose bifrost_new_chat" >&2
  exit 1
fi
if ! grep -Fq 'format!("{}/c/{}", config.chatgpt.base_url, cid)' \
  crates/bifrost-admin/src/im_gateway/chatgpt_web/send.rs; then
  echo "ChatGPT Web existing-conversation URL must keep the /c/{conversation_id} route" >&2
  exit 1
fi
if rg -n "close_chatgpt_pages_for_fresh_run" \
  crates/bifrost-admin/src/im_gateway/chatgpt_web/send.rs \
  crates/bifrost-admin/src/im_gateway/chatgpt_web/browser.rs; then
  echo "Fresh ChatGPT Web runs must not close existing browser tabs" >&2
  exit 1
fi
grep -q "take_or_attach_reusable_chatgpt_tab" crates/bifrost-admin/src/im_gateway/chatgpt_web/send.rs
grep -q "take_reusable_conversation_tab" crates/bifrost-admin/src/im_gateway/chatgpt_web/browser.rs
grep -q "find_chatgpt_page" crates/bifrost-admin/src/im_gateway/chatgpt_web/browser.rs

SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
  ask_runs_use_shared_chatgpt_web_browser_profile_not_run_local_profile \
  --lib -- --nocapture

SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
  fresh_runs_reuse_existing_chatgpt_tab_without_closing_or_reopening \
  --lib -- --nocapture

SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
  fresh_conversation_takes_most_recent_tab_without_closing_it \
  --lib -- --nocapture

SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
  fresh_conversation_reuse_helper_ \
  --lib -- --nocapture

SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
  fresh_conversation_reuse_skips_other_profiles_and_closed_tabs \
  --lib -- --nocapture

SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
  reuse_fresh_tab_installs_pooled_target_and_skips_existing_selection \
  --lib -- --nocapture

SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
  new_conversation_url_ \
  --lib -- --nocapture

SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
  conversation_page_match_treats_homepage_as_expected_for_fresh_run \
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
  wait_final_resets_settle_for_equal_length_content_replacement \
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
  daily_agent_report_gate_excludes_known_failed_entries_from_missing_reports \
  --lib -- --nocapture

SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
  daily_agent_entry_failure_summary_lists_failed_dates_and_targets \
  --lib -- --nocapture

SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin \
  daily_agent_chatgpt_web_tomorrow_todo \
  --lib -- --nocapture
