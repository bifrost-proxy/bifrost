# Remove the built-in Bifrost Agent

## Goal

Remove the built-in `Bifrost Agent` runtime and every product entry point that can select or execute it. Agent Chat, IM providers, scheduled Agent tasks, runner calls, and ASR Daily Agent continue through configured external runners such as Codex, Trae X, Claude Code, and ChatGPT Web.

## Boundaries

- Delete the `/api/agent/chat` route, isolated built-in worker command, worker implementation, shutdown hooks, runner-call branch, scheduled-task branch, and ASR Daily Agent branch.
- Delete the built-in model client, sampling/turn loop, prompt renderer, compaction, MCP, memory, authorization, and local tool implementations. Remove their model-provider/MCP/memory/tool runtime configuration and tests from the shared config API.
- Remove `Bifrost Agent` from WebUI runner selectors and defaults. The external runner registry owns the default runner; Codex is the fallback only when configuration is unavailable.
- Remove the dead WebUI `/plan` and `/compact` controls that were permanently disabled after external-runner-only routing.
- Reject persisted `bifrost_agent`, `builtin`, `bifrost`, and empty runner values instead of silently reviving the removed runtime.
- Remove dedicated built-in runtime design documents, E2E scripts, crate integration tests, and human-test documents.
- Keep shared session history, progress events, external-runner metadata/configuration, AGENTS.md/skills discovery, and external-runner infrastructure even where the historical crate name remains `bifrost-agent`; these are still used by external runners and are not an executable built-in runtime.

## Compatibility

Existing external runner IDs and histories remain readable. A stale configuration that explicitly selects the removed built-in runner is invalid and must be changed to an enabled external runner. No hidden fallback to the removed runtime is allowed.

## Validation

- Rust config tests reject removed aliases and default to Codex.
- Web tests verify runner options contain only enabled external runners and all chat requests use `/api/im-gateway/chat/stream`.
- Static checks verify the built-in HTTP route, worker module, CLI worker variant, model/tool runtime modules, dead WebUI command controls, selector labels, and dedicated test assets are absent.
- Existing external runner E2E and Agent Chat UI tests remain the regression path.
- Remote CI provides the mandatory `bash scripts/ci/coverage-all.sh --json --gate` coverage gate.
