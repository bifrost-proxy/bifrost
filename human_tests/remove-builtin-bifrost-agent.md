# Remove built-in Bifrost Agent

## Module

Verify that the built-in Bifrost Agent can no longer be selected or executed while configured external runners remain available.

## Preconditions

- Run from the repository root.
- Use the current task worktree; do not start or stop the user's default Bifrost service.
- Frontend dependencies and Rust toolchain are installed.

## TC-RBA-01: Built-in runtime entry points are absent

1. Run `test ! -e crates/bifrost-admin/src/im_gateway/agent_worker.rs`.
2. Run `! rg -n 'handle_agent_chat|AgentCommands::Worker|/api/agent/chat' crates/bifrost-admin/src crates/bifrost-cli/src`.
3. Run `! rg -n 'Bifrost Agent|bifrost_agent|adapter: "builtin"' web/src`.

Expected: all commands exit 0; no built-in HTTP route, worker command, worker source file, or WebUI choice remains.

## TC-RBA-02: External runner flow remains buildable

1. Run `SKIP_FRONTEND_BUILD=1 cargo check -p bifrost-admin -p bifrost-cli`.
2. Run `pnpm --dir web exec tsc -b --pretty false`.
3. Run `rg -n '/api/im-gateway/chat/stream' web/src/pages/AI/AgentChatSection.helpers.tsx`.

Expected: Rust and TypeScript checks pass, and Agent Chat uses the external runner stream endpoint.

## TC-RBA-03: Dedicated built-in tests and docs are removed

1. Run `test ! -e e2e-tests/tests/test_agent_builtin_status_runtime.sh`.
2. Run `test ! -e design/bifrost-agent-codex-parity.md`.
3. Run `test ! -e human_tests/agent-builtin-commands.md`.
4. Run `test ! -e crates/agent/tests/p1_tools_e2e.rs`.

Expected: every removed built-in-only asset is absent.

## TC-RBA-04: External runner selectors remain

1. Run `rg -n 'Codex Runner|Claude Code|Trae X|ChatGPT Web' web/src/pages/AI web/src/pages/Settings/tabs`.
2. Run `rg -n 'default_runner_id|DEFAULT_CODEX_RUNNER_ID' crates/bifrost-admin/src/im_gateway/external_cli/mod.rs`.

Expected: external runner UI and default registry code remain present.

## Cleanup

No service or temporary data directory is created by these cases.

## Execution record

- 2026-07-13: TC-RBA-01 through TC-RBA-04 all passed in the isolated task worktree. Rust and TypeScript checks succeeded, the external chat stream/default registry remained present, and every sampled built-in runtime, documentation, and test asset was absent.
