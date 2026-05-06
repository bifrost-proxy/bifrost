#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

cd "$REPO_DIR"

log() { echo "[utf8-safe-preview-e2e] $*"; }

run_test() {
    local label="$1"
    shift
    log "RUN ${label}"
    "$@"
    log "PASS ${label}"
}

run_test "core UTF-8 truncation helper tests" \
    cargo test -p bifrost-core text::tests:: -- --nocapture

run_test "agent compaction multibyte tool arguments" \
    cargo test -p bifrost-agent compact::tests::test_format_history_handles_multibyte_tool_arguments -- --nocapture

run_test "agent compaction multibyte user messages" \
    cargo test -p bifrost-agent compact::tests::test_format_history_truncates_multibyte_user_messages -- --nocapture

run_test "IM Gateway task stdout/stderr preview multibyte boundary" \
    cargo test -p bifrost-admin im_gateway::task_executor::tests::test_truncate_preview_multibyte_boundary -- --nocapture

run_test "proxy body preview multibyte boundary" \
    cargo test -p bifrost-proxy utils::logging::tests::test_truncate_body_multibyte_boundary -- --nocapture

run_test "E2E assertion failure preview multibyte boundary" \
    cargo test -p bifrost-e2e assertions::tests::test_assert_body_contains_multibyte_preview -- --nocapture

log "UTF-8 safe preview E2E passed"
