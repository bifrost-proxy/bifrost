# External Runner 子 Agent 状态可观测性真实场景测试

## 功能模块说明

本模块验证 Codex、TraeX 与 Claude Code 的子 Agent 协作事件会归一为统一状态，并在飞书进度卡片与 Web UI Agent Chat 中明确展示派发任务、当前阶段、Agent 身份、执行状态、进度详情和耗时。同一子 Agent 从派发到完成、失败或中断只保留一个持续更新的条目，不混入普通命令统计。

## 前置条件

1. 当前目录位于仓库根目录。
2. 使用当前源码执行 Rust focused 测试、单条 `bifrost-e2e` renderer 用例和单条 Playwright 用例。
3. 不运行本地全量 E2E；浏览器场景使用 mock external runner NDJSON 流，避免依赖真实模型和账号。

## 测试用例列表

### TC-ERSO-01：三类 Runner 协议归一、持久化与生命周期合并

**操作步骤**：

1. 执行：
   ```bash
   cargo test -p bifrost-agent --lib subagent -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin --lib subagent -- --nocapture
   ```

**预期结果**：

- Codex/TraeX 的 `collabAgentToolCall`、snake_case CLI 事件与 `subAgentActivity` 均产生 `sub_agent_updated`。
- Claude Code 的 `Task`/`Agent` tool use/result 产生相同的 provider-neutral 事件。
- 事件保留 task、phase、agent id、状态、详情、开始/更新时间和终态耗时，并写入可回放 session timeline。
- 同一 Agent 的后续 `wait`/完成事件不会丢失首次派发的任务。

**实际结果（2026-08-09）**：

- 通过。`bifrost-agent` focused 测试 `1 passed`；`bifrost-admin` focused 测试 `5 passed`。
- 覆盖 Codex/TraeX collab、Codex activity、Claude Code Task、飞书 snapshot 合并和 Web history 持久化。

### TC-ERSO-02：飞书进度卡片展示子 Agent 任务、状态和耗时

**操作步骤**：

1. 执行：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo run -p bifrost-e2e -- --test im_gateway_subagent_progress_card_renderer --jobs 1 --timeout 120
   ```

**预期结果**：

- 只执行 `im_gateway_subagent_progress_card_renderer` 一条 E2E 并通过。
- CardKit JSON 2.0 的执行过程区域只保留一个子 Agent 条目。
- 条目展示 `Review the authentication flow`、`waiting`、`agent-7`、`已完成`、`Review complete` 和 `4 秒`。
- 协调动作 `spawnAgent`/`wait` 不冒充 Agent 名称，也不渲染为普通工具步骤。

**实际结果（2026-08-09）**：

- 通过。`bifrost-e2e` 只运行该用例，结果 `1 passed`。
- 首轮验证发现终态 `wait` 曾覆盖 Agent 标签；修复后复跑通过，卡片使用中性“子 Agent”标题并保留 `waiting` 阶段。

### TC-ERSO-03：Web UI 实时与历史展示、主题和命令分组边界

**操作步骤**：

1. 执行 Web focused 单元测试：
   ```bash
   pnpm --dir web exec vitest run src/pages/AI/AgentChatSection.helpers.test.ts src/pages/AI/AgentChatSection.timeline.test.ts
   ```
2. 只执行新增的 Playwright 场景：
   ```bash
   pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts --grep "shows sub-agent task phase identity status and duration"
   ```

**预期结果**：

- 实时 `sub_agent_updated` 和历史 `subagent_updated` 都恢复为同一子 Agent 过程条目。
- 运行态与终态按 agent id 合并，保留首次任务并冻结 `4s` 终态耗时。
- 折叠摘要可见标签、完整任务、`Completed · waiting · 4s`；展开后可见 Agent ID 和进度详情。
- 子 Agent 不计入 command group；浅色和深色主题下均可读。

**实际结果（2026-08-09）**：

- 通过。Vitest `2 files / 35 tests passed`。
- Playwright 只运行新增场景，结果 `1 passed`；确认 lifecycle 合并、详情展开、命令分组隔离和亮暗主题。

## 清理步骤

1. Playwright/Vite 测试服务由测试 runner 自动停止。
2. 确认未启动真实 Codex、TraeX 或 Claude Code 子 Agent 进程。
3. 确认未残留本地全量 E2E 进程。
