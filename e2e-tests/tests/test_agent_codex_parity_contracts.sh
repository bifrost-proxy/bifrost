#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

cd "$REPO_DIR"

echo "[agent-codex-parity] checking bifrost-agent test target"
cargo check -p bifrost-agent --tests

echo "[agent-codex-parity] validating open authorization policy"
cargo test -p bifrost-agent authorization::tests::default_policy_is_open_without_sandbox_or_approval -- --exact
cargo test -p bifrost-agent authorization::tests::orchestrator_allows_local_and_mcp_tools -- --exact

echo "[agent-codex-parity] validating Responses streaming protocol"
cargo test -p bifrost-agent responses::tests::responses_url_converts_chat_completions_endpoint -- --exact
cargo test -p bifrost-agent responses::tests::parses_responses_stream_text_and_function_call -- --exact
cargo test -p bifrost-agent responses::tests::streaming_client_sends_responses_shape -- --exact

echo "[agent-codex-parity] validating OpenAI provider switches to Responses"
cargo test -p bifrost-agent config::tests::test_resolve_effective_config_openai -- --exact

echo "[agent-codex-parity] OK"
