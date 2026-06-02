# Agent Codex Plan Mode Alignment

## 功能模块说明

本模块对齐 Codex 最新 collaboration mode、Plan Mode、Web UI/IM `/plan` 入口、`<proposed_plan>`、`request_user_input` 使用策略和 goal continuation prompt。目标是让 Bifrost Agent 在 Default 模式继续执行任务，在 Plan 模式先探索和规划，并把最终方案作为独立 proposal 输出，而不是污染普通 assistant 回复、记忆抽取或 `update_plan` checklist。

## 实现逻辑

### Collaboration Mode

- 新增 `CollaborationMode::{Default, Plan}` 作为每轮请求参数，不写入全局 `AgentConfig`，避免破坏现有配置初始化和 IM Provider 默认行为。
- `/api/agent/chat/stream` 与 `/api/im-gateway/agent/chat` 接受 `collaborationMode` / `collaboration_mode`，worker JSON 协议传递 `collaborationMode`。
- Web UI slash 面板提供 `/plan` 入口：仅在当前线程/runner 是内置 Bifrost Agent 时展示；选择后进入 composer `Plan Mode` 状态并显示可关闭的模式标记，不再只把 `/plan ` 文本塞进输入框。发送时前端显式传 `collaboration_mode=plan`；用户手动输入 `/plan <message>` 时仍剥离 slash 文本。
- `/plan` 与 `/compact` 属于内置 Agent 控制命令。当前线程是外部 APP、Codex、ChatGPT Web 或其他 external runner 时，Web UI 不展示这些命令；用户手动输入时也按普通消息交给外部 runner，不能由前端误剥离或静默改路由。
- IM/API 后端统一解析 `/plan <message>`：即使调用方没有显式传 `collaboration_mode`，也会剥离 slash 文本并以 Plan Mode 启动 worker；`/planner` 等普通文本不得误判。
- Prompt builder 在每轮 prefix 中追加独立 developer fragment：
  - Default：声明当前处于 Default，用户文本不会切换模式，`request_user_input` 只在工具可用时使用。
  - Plan：声明只允许非 mutating exploration，禁止执行实现，最终官方方案必须包在 `<proposed_plan>` 块中。

### Plan Mode 与 update_plan 边界

- Plan Mode 是协作模式；`update_plan` 是 TODO/checklist runtime 工具，两者不互相切换。
- Turn loop 在 Plan Mode 下拦截 `update_plan` 调用并返回错误，避免模型把 proposal 错写为进度快照。
- 现有 `update_plan` snapshot 语义不变，Default 模式仍可正常显示计划卡片。

### proposed_plan 独立输出

- 新增 `<proposed_plan>` 解析器，只接受独占一行的 open/close tag。
- Plan Mode 最终回复中：
  - 标签外文本作为普通 `response`、assistant history、recorder、memory extraction 和 citation 输入。
  - 标签内文本作为 `TurnResult.proposed_plan`、`AgentTurnProgressEvent::ProposedPlan`、SSE `proposed_plan`、worker `proposedPlan`、API `proposed_plan`。
- Web UI 必须把 `proposed_plan` progress event 和 `run_finished.proposedPlan` 渲染为用户可见的 “Plan Mode result”。如果模型只输出 `<proposed_plan>` 块、标签外 `response` 为空，消息区仍必须展示方案正文。
- JSONL history 必须记录 `proposed_plan` 事件，history replay 必须把该事件还原为 assistant 规划结果。否则 live UI 刷新或打开历史会看起来“没有规划结果”。
- IM progress card 在收到 `ProposedPlan` 后复用计划面板展示“实施方案”，但 final output 不包含 proposal 标签。

### request_user_input 和 Goal Continuation

- `request_user_input` 仍处于“可校验但无真实等待通道”的边界；prompt 策略更新为 Codex 对齐：Plan Mode 有可用交互通道时优先使用，否则直接问。
- Goal continuation prompt 更新为 Codex 最新的 evidence-first、progress visibility、fidelity、completion audit 和 blocked audit 规则，并修正为 runtime 实际替换的 `{{ objective }}` 变量格式。
- Goal prompt markdown 不能只是文档资产；`tools::goal` 必须通过 `include_str!` 读取 `prompts/goals/continuation.md` 和 `prompts/goals/budget_limit.md`。单元测试与真实 API E2E 必须证明运行时模型请求包含 markdown 文件中的独有策略文本，且不再包含旧内联 continuation prompt。

## 依赖项

- 不引入新 crate。
- 复用现有 prompt fragment、worker JSON IPC、SSE progress、IM progress card、mock model E2E 框架。

## 测试方案

### 单元测试

- `proposed_plan::tests::*`：验证 proposal 提取、可见文本剥离、unterminated block 和非独占 tag 忽略。
- `prompt::tests::test_plan_mode_prompt_is_separate_developer_message`：验证 Plan Mode developer prompt 独立注入且包含 `<proposed_plan>` 策略。
- `prompt::tests::test_default_mode_prompt_declares_default_mode`：验证 Default mode prompt 默认注入。
- `tools::goal::tests::continuation_prompt_contains_remaining_tokens`：验证 continuation prompt 变量替换仍生效。
- `tools::goal::tests::goal_prompt_rendering_uses_markdown_templates`：验证 continuation/budget 渲染输出包含 markdown 文件独有策略文本，并排除旧内联 continuation prompt 文本。
- `im_gateway::agent_worker::tests::turn_result_roundtrip_preserves_stop_fields`：验证 worker IPC 保留 `proposedPlan`。
- `im_gateway::agent_slash::tests::*`：验证 `/plan` slash 解析、非命令边界、multiline 消息和显式 mode 优先级。

### E2E 测试

- `e2e-tests/tests/test_agent_plan_mode_human_api.sh`：启动真实 Bifrost + mock model，调用 `/api/im-gateway/agent/chat` 的 `collaboration_mode=plan`，验证：
  - 模型请求中包含 Plan Mode developer prompt 和 `<proposed_plan>` 指令。
  - API `response` 不包含 `<proposed_plan>` 标签。
  - API `proposed_plan` 独立包含方案正文。
  - IM/API `/plan <message>` 入口在不显式传 `collaboration_mode` 时仍进入 Plan Mode，且模型请求中的用户正文已剥离 `/plan`。
- `web/tests/ui/agent-chat.spec.ts` 的 `slash plan mode`：验证 Web UI slash 面板可选择 `/plan`，输入框显示 Plan Mode 标记和规划提示，发送 payload 的 `message` 已剥离 slash 且包含 `collaboration_mode: "plan"`，消息区展示 `proposed_plan` 规划结果。
- `web/tests/ui/agent-chat.spec.ts` 的 external runner 回归：验证切到 Codex/external runner 后输入 `/` 不展示 `/plan` / `/compact` 命令，但 runner 选择仍可用。
- `web/src/pages/AI/AgentChatSection.timeline.test.ts`：验证 persisted `proposed_plan` history event 会渲染为 assistant 规划结果。
- `e2e-tests/tests/test_agent_goal_prompt_templates_human_api.sh`：启动真实 Bifrost + mock model，先创建 active goal，再触发 worker 自动 continuation，验证：
  - continuation 模型请求包含 `Work from evidence`、`The audit must prove completion` 和 `strict blocked audit`。
  - continuation 模型请求不包含旧内联 prompt 的 `Avoid repeating work that is already done`。
  - mock model 通过 `update_goal` 完成目标，避免 continuation 循环误判。

### 真实场景测试

- 新增 `human_tests/agent-codex-plan-mode.md`，覆盖静态文档同步、真实 API Plan Mode proposal、Plan Mode update_plan 禁用、goal prompt markdown 真实接线、Web UI 与 IM `/plan` 入口和 prompt strategy 回归。
- 文档创建后必须立即按用例执行，记录命令和结果。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 目标复核：对照 Codex 差异清单，确认 Default/Plan prompt、proposal 输出、API/worker/SSE/IM、goal continuation 均已覆盖。
- 代码 review：重点检查 Plan Mode 是否会误更新 `current_plan`、proposal 是否污染 assistant history/memory、现有 Default `update_plan` 是否保持兼容。
- 复测命令：focused `cargo test`、新增 E2E、human_tests 用例。

### 第 2 轮

- 复查第 1 轮修复后的 `git diff`、新增文档索引和协议字段。
- 复跑受影响测试与 workspace 校验，确认没有 exhaustive match、serde 字段或 prompt 变量遗漏。

## 校验要求

- E2E 测试必须先于 `rust-project-validate`。
- 收尾必须运行 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`。
- 推送后使用 GitHub Actions PAT skill 盯 CI，直到远端 CI 全绿。

## 文档更新要求

- 更新 `human_tests/agent-codex-plan-mode.md`。
- 更新 `human_tests/readme.md` 索引和总数。
- 本次新增 Web UI slash 入口和 IM `/plan` 文本入口；README 暂不列出 Agent chat 内部命令，真实使用说明由 Web UI slash 菜单和 `human_tests/` 覆盖。
