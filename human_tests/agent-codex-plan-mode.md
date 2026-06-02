# Agent Codex Plan Mode 对齐测试用例

## 功能模块说明

本模块验证 Bifrost Agent 对齐 Codex 最新 collaboration mode、Plan Mode、`/plan` 入口、`<proposed_plan>` 独立输出、`request_user_input` 策略和 goal continuation prompt。

## 前置条件

1. 当前目录位于仓库根目录。
2. 启动真实 Bifrost 服务时必须使用临时 `BIFROST_DATA_DIR`，并携带 `--no-system-proxy`。
3. API 验证使用 `e2e-tests/tests/test_agent_plan_mode_human_api.sh` 和 `e2e-tests/tests/test_agent_goal_prompt_templates_human_api.sh`，脚本会启动 mock model 和真实 Bifrost 服务。

## 测试用例列表

### TC-ACPM-01：设计与 prompt 策略文档同步

**操作步骤**：
1. 执行：
   ```bash
   test -f design/agent-codex-plan-mode-alignment.md
   rg -n "Collaboration Mode|Plan Mode|proposed_plan|request_user_input|Goal Continuation" design/agent-codex-plan-mode-alignment.md
   rg -n "Collaboration Mode: Default|request_user_input" crates/agent/src/prompts/collaboration/default.md
   rg -n "Plan Mode|<proposed_plan>|update_plan" crates/agent/src/prompts/collaboration/plan.md
   rg -n "Progress visibility|Completion audit|Blocked audit" crates/agent/src/prompts/goals/continuation.md
   ```

**预期结果**：
- 设计文档存在并覆盖 collaboration mode、Plan Mode、proposal 输出、`request_user_input` 和 goal continuation。
- Default prompt 声明默认模式和 `request_user_input` 可用性。
- Plan prompt 声明 `<proposed_plan>` finalization 和禁止 `update_plan`。
- Goal continuation prompt 包含 progress visibility、completion audit、blocked audit。

### TC-ACPM-02：真实 API Plan Mode proposal 独立输出

**操作步骤**：
1. 执行：
   ```bash
   SKIP_BUILD=true BIFROST_BIN="${BIFROST_BIN:-$(pwd)/target/debug/bifrost}" e2e-tests/tests/test_agent_plan_mode_human_api.sh
   ```

**预期结果**：
- 脚本输出 `[agent-plan-mode] PASS`。
- mock model 收到的请求包含 Plan Mode prompt 和 `<proposed_plan>` 指令。
- API JSON 的 `response` 不包含 `<proposed_plan>` / `</proposed_plan>` 标签。
- API JSON 的 `proposed_plan` 独立包含方案正文。

### TC-ACPM-03：Plan Mode 下 update_plan 不会更新 checklist

**操作步骤**：
1. 执行：
   ```bash
   rg -n 'collaboration_mode\\.is_plan\\(\\).*update_plan|not allowed in Plan Mode|Use a <proposed_plan> block' crates/agent/src/session/turn_loop.rs
   rg -n 'AgentTurnProgressEvent::ProposedPlan|proposed_plan' crates/agent/src/session_status.rs crates/bifrost-admin/src/handlers/agent_chat.rs crates/bifrost-admin/src/im_gateway/progress_card.rs
   cargo test -p bifrost-agent proposed_plan -- --nocapture
   cargo test -p bifrost-agent mode_prompt -- --nocapture
   ```

**预期结果**：
- turn loop 存在 Plan Mode `update_plan` 拦截逻辑，错误提示要求使用 `<proposed_plan>`。
- progress/API/IM 链路存在 `ProposedPlan` / `proposed_plan` 映射。
- focused 单元测试全部通过。

### TC-ACPM-04：Goal continuation prompt markdown 真实进入运行请求

**操作步骤**：
1. 执行：
   ```bash
   rg -n 'include_str!\\("../prompts/goals/(continuation|budget_limit)\\.md"\\)' crates/agent/src/tools/goal.rs
   cargo test -p bifrost-agent goal_prompt_rendering_uses_markdown_templates -- --nocapture
   SKIP_BUILD=true BIFROST_BIN="${BIFROST_BIN:-$(pwd)/target/debug/bifrost}" e2e-tests/tests/test_agent_goal_prompt_templates_human_api.sh
   ```

**预期结果**：
- `goal.rs` 通过 `include_str!` 读取 goal prompt markdown，而不是继续使用旧内联 continuation prompt。
- 单元测试证明 `continuation_prompt()` / `budget_limit_prompt()` 渲染输出包含 markdown 文件独有策略内容。
- 真实 API E2E 输出 `[agent-goal-prompts] PASS`。
- mock model 收到的自动 continuation 请求包含 `Work from evidence`、`The audit must prove completion` 和 `strict blocked audit`。
- mock model 请求中不包含旧内联 prompt 的 `Avoid repeating work that is already done`。

### TC-ACPM-05：Web UI 与 IM API 均支持 `/plan` 入口

**操作步骤**：
1. 执行：
   ```bash
   rg -n 'command: "/plan"|agent-chat-plan-mode-pill|agent-chat-active-plan-mode|collaboration_mode: params\\.collaborationMode|parseAgentPlanSlash' web/src/pages/AI/AgentChatSection.tsx web/src/pages/AI/AgentChatSection.helpers.tsx web/src/pages/AI/AgentChatSection.runnerCall.tsx
   rg -n 'parse_agent_slash_mode|worker_request\\.collaboration_mode' crates/bifrost-admin/src/im_gateway/agent_slash.rs crates/bifrost-admin/src/handlers/agent_chat.rs crates/bifrost-admin/src/handlers/im_gateway/agent_api.rs crates/bifrost-admin/src/handlers/im_gateway/agent_chat.rs
   cargo test -p bifrost-admin agent_slash -- --nocapture
   pnpm --dir web exec playwright test web/tests/ui/agent-chat.spec.ts -g "slash plan mode"
   SKIP_BUILD=true BIFROST_BIN="${BIFROST_BIN:-$(pwd)/target/debug/bifrost}" e2e-tests/tests/test_agent_plan_mode_human_api.sh
   ```

**预期结果**：
- Web UI slash 面板包含 `/plan`，选择后输入框显示 `Plan Mode` 状态标记和 planning hint，而不是像 `/compact` 一样立即静默发送。
- Web UI 发送 `/plan Create a migration plan` 时，请求体包含 `collaboration_mode: "plan"`，且 `message` 被剥离为 `Create a migration plan`。
- Web UI 收到 `proposed_plan` 或 `run_finished.proposedPlan` 后，消息区展示 `Plan Mode result` 与方案正文；即使普通 `response` 为空也不能空白。
- IM/API 入口发送 `/plan 请规划斜杠入口方案` 时，即使请求体没有显式 `collaboration_mode`，模型请求仍包含 Plan Mode prompt 和 `<proposed_plan>` 指令。
- IM/API 模型请求中的用户正文包含 `请规划斜杠入口方案`，不包含原始 `/plan 请规划斜杠入口方案`。

### TC-ACPM-06：外部 runner/Codex 隐藏内置 Agent 控制命令，history 保留 proposed plan

**操作步骤**：
1. 执行：
   ```bash
   rg -n 'supportsBuiltInAgentCommands|enableCommands|proposedPlanMessageContent|eventType === "proposed_plan"|event\\.proposedPlan' web/src/pages/AI/AgentChatSection.tsx web/src/pages/AI/AgentChatSection.runnerCall.tsx
   rg -n 'PROPOSED_PLAN|record_proposed_plan|proposed_plan' crates/agent/src/persistence.rs crates/agent/src/session/turn_loop.rs web/src/pages/AI/AgentChatSection.timeline.ts
   pnpm --dir web exec vitest run src/pages/AI/AgentChatSection.timeline.test.ts
   pnpm --dir web exec playwright test web/tests/ui/agent-chat.spec.ts -g "external runner"
   ```

**预期结果**：
- 当前线程/runner 为 Codex、ChatGPT Web 或 external runner 时，输入 `/` 不展示 `/plan`、`/compact` 命令；runner 选择项仍可正常展示和选择。
- 外部 runner 手动输入 `/plan ...` 时不会被前端剥离为 Plan Mode，也不会走内置 `/api/agent/chat/stream`。
- Plan Mode live UI 收到 proposed plan 后会显示规划结果。
- JSONL history 中的 `proposed_plan` 事件可被前端 replay 成 assistant 规划结果，刷新或打开历史不会丢失方案正文。

## 清理步骤

- `test_agent_plan_mode_human_api.sh` 和 `test_agent_goal_prompt_templates_human_api.sh` 会自动清理临时目录、mock model 和 Bifrost 进程。
- 如果测试被外部中断，执行：
  ```bash
  pkill -f "test_agent_plan_mode_human_api" || true
  pkill -f "test_agent_goal_prompt_templates_human_api" || true
  pkill -f "mock-plan-mode" || true
  ```
