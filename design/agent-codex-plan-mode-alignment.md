# Agent Codex Plan Mode Alignment

## 背景

Codex 的 Agent 有两种协作模式（collaboration mode）：Default 与 Plan。Default 直接执行任务；Plan 只做只读探索、规划，最终把实施方案作为独立 `<proposed_plan>` 输出，不污染 assistant 普通回复、记忆抽取或 `update_plan` checklist。

Bifrost Agent 早期没有区分协作模式，`update_plan` 既被当成 TODO checklist，也被误用为最终 plan proposal，导致：

- 用户看到的「实施方案」和运行时 plan 混在一起，历史回放丢失方案文本。
- Web UI slash 只把 `/plan ` 塞进输入框，模型侧没有独立 developer prompt。
- IM `/plan` 与前端 slash、后端 worker mode 各走各的，`/plan <message>` 常常没有进入 Plan Mode。
- Goal continuation prompt 与 `request_user_input` 策略与 Codex 最新版本偏移。

本模块对齐 Codex 最新 collaboration mode、Plan Mode、Web UI/IM `/plan` 入口、`<proposed_plan>`、`request_user_input` 使用策略与 goal continuation prompt。目标是让 Bifrost Agent 在 Default 模式继续执行任务，在 Plan 模式先探索和规划，最终方案作为独立 proposal 输出，而不是污染普通 assistant 回复、记忆抽取或 `update_plan` checklist。

## 用户目标验证清单

### 必须实现

- 新增 `CollaborationMode::{Default, Plan}` 作为每轮请求参数（不写入全局 `AgentConfig`）。
- `/api/agent/chat/stream` 与 `/api/im-gateway/agent/chat` 接受 `collaborationMode` / `collaboration_mode`；worker JSON 协议传递 `collaborationMode`。
- Web UI slash 面板提供 `/plan` 入口：仅在当前线程/runner 是内置 Bifrost Agent 时展示；选择后进入 Plan Mode 状态并显示可关闭的模式标记；发送时前端显式传 `collaboration_mode=plan`；用户手动输入 `/plan <message>` 时后端剥离 slash 文本。
- IM/API 后端统一解析 `/plan <message>`：即使调用方没有显式传 `collaboration_mode`，也会剥离 slash 文本并以 Plan Mode 启动 worker；`/planner` 等普通文本不得误判。
- Prompt builder 每轮追加独立 developer fragment：
  - Default：声明当前处于 Default，用户文本不切换模式，`request_user_input` 只在工具可用时使用。
  - Plan：声明只允许非 mutating exploration，禁止执行实现，最终官方方案必须包在 `<proposed_plan>` 块中。
- Plan Mode 拦截 `update_plan` 调用，返回错误，避免把 proposal 错写为进度快照。
- `<proposed_plan>` 解析器只接受独占一行的 open/close tag；标签内外文本严格拆分。
- 标签外文本作为普通 `response`、assistant history、recorder、memory extraction 与 citation 输入。
- 标签内文本作为 `TurnResult.proposed_plan`、`AgentTurnProgressEvent::ProposedPlan`、SSE `proposed_plan`、worker `proposedPlan`、API `proposed_plan`。
- Web UI 消息区渲染 `proposed_plan`；即使标签外 `response` 为空也必须显示方案正文。
- JSONL history 记录 `proposed_plan` 事件；history replay 还原为 assistant 规划结果。
- IM progress card 收到 `ProposedPlan` 复用计划面板展示「实施方案」；final output 不含 proposal 标签。
- `request_user_input` 策略更新为 Codex 对齐：Plan Mode 有可用交互通道时优先使用，否则直接问。
- Goal continuation prompt 更新为 evidence-first、progress visibility、fidelity、completion audit、blocked audit 规则，`{{ objective }}` 变量替换有效。
- Goal prompt markdown 通过 `include_str!` 读取 `prompts/goals/continuation.md` 与 `prompts/goals/budget_limit.md`；单测与真实 API E2E 验证运行时请求包含 markdown 独有策略文本，且不再包含旧内联 continuation prompt。

### 必须不破坏

- Default 模式下现有 `update_plan` snapshot 语义不变，计划卡片正常显示。
- Plan Mode 与 `update_plan` 是不同层：一个是协作模式，一个是 TODO/checklist runtime 工具，不互相切换。
- 外部 APP、Codex、ChatGPT Web 或其他 external runner 线程不展示 `/plan` / `/compact`；用户手动输入按普通消息交给外部 runner，不能由前端误剥离或静默改路由。
- Goal continuation 现有 `update_goal` 完成语义仍生效。

### 必须真实验证

- Rust 单测覆盖 proposal 解析、prompt 独立注入、goal prompt markdown 加载、worker IPC round-trip、slash 边界。
- Rust E2E 通过真实 `/api/im-gateway/agent/chat` 覆盖 Plan Mode developer prompt、`<proposed_plan>` 独立字段、`/plan <message>` slash 剥离、goal continuation markdown 真实接线。
- Web UI Playwright 覆盖 slash 面板行为、Plan Mode 标记、external runner 回归、history replay。
- human_tests 覆盖真实 API 与 UI 操作。

## 产品语义

### Collaboration Mode 是每轮参数

- 不写入 `AgentConfig`，避免破坏现有配置初始化与 IM Provider 默认行为。
- 请求级参数：`collaborationMode` / `collaboration_mode`。
- Worker JSON IPC 保留 `collaborationMode` 字段。
- Web UI 状态：composer 层记录当前 Plan Mode 标记，用户可关闭。

### Plan Mode 与 update_plan 的边界

- `update_plan` 是运行时 checklist 工具；Plan Mode 是协作模式。两者独立。
- Plan Mode 下 turn loop 拦截 `update_plan` 调用，返回错误提示。
- Default 模式下 `update_plan` 行为完全不变。

### `<proposed_plan>` 是独立通道

- 独占一行的 open/close tag：解析器不接受同行 open/close 混排。
- 标签外文本 → 普通 `response`。
- 标签内文本 → `proposed_plan` 通道，跨 SSE、worker、API、JSONL 全链路传递。
- Web UI 消息区把 `proposed_plan` progress event 与 `run_finished.proposedPlan` 渲染为「Plan Mode result」；如果模型只输出 `<proposed_plan>` 块、`response` 为空，消息区仍必须展示方案正文。
- JSONL history replay：`proposed_plan` 事件恢复为 assistant 规划结果，避免 live UI 刷新或历史打开后看起来「没有规划结果」。

### `/plan` slash 只对内置 Agent

- Web UI slash 面板：仅在当前线程/runner 是内置 Bifrost Agent 时展示 `/plan` / `/compact`。
- 切到外部 runner：`/` 面板不展示 `/plan` / `/compact`，但 runner 选择器仍可用。
- 用户手动输入 `/plan xxx` 到外部 runner：按普通消息交给外部 runner，不能由前端误剥离或静默改路由。

## 技术细节

### 关键类型与字段

```rust
#[derive(Copy, Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationMode {
    #[default]
    Default,
    Plan,
}

pub struct TurnResult {
    pub response: String,
    pub proposed_plan: Option<String>,
    ...
}

pub enum AgentTurnProgressEvent {
    ...
    ProposedPlan { text: String },
    ...
}
```

### 关键文件

- `crates/agent/src/session.rs` / `crates/agent/src/session/turn_loop.rs`：Plan Mode 拦截 `update_plan`；`TurnResult.proposed_plan` 字段。
- `crates/agent/src/prompt/mod.rs`：Default/Plan mode developer prompt fragment。
- `crates/agent/src/prompts/goals/continuation.md` / `budget_limit.md`：`include_str!` 加载。
- `crates/agent/src/tools/goal.rs`：使用 markdown 模板渲染 continuation prompt。
- `crates/agent/src/proposed_plan.rs`：`<proposed_plan>` 解析器。
- `crates/bifrost-admin/src/im_gateway/agent_slash.rs`：`/plan <message>` 解析、外部 runner 边界。
- `crates/bifrost-admin/src/im_gateway/agent_worker.rs`：worker IPC 承载 `collaborationMode` 与 `proposedPlan`。
- `crates/bifrost-admin/src/im_gateway/progress_card.rs`：`ProposedPlan` 事件渲染。
- `crates/bifrost-admin/src/handlers/im_gateway/agent_api.rs`：API 层字段。
- `crates/agent/src/persistence.rs`：JSONL `proposed_plan` 事件与 replay。
- `web/src/pages/AI/AgentChatSection.tsx` / `AgentChatSection.timeline.ts`：slash 面板、Plan Mode 标记、消息区渲染、history replay。
- `human_tests/agent-codex-plan-mode.md`
- `human_tests/readme.md`
- `e2e-tests/tests/test_agent_plan_mode_human_api.sh`
- `e2e-tests/tests/test_agent_goal_prompt_templates_human_api.sh`

### `<proposed_plan>` 解析器

- 只接受独占一行的 `<proposed_plan>` / `</proposed_plan>`。
- 允许 leading/trailing whitespace。
- 同行含 open/close 或未闭合 block → 忽略，作为普通 response 内容。
- 多个 block 时取最后一个 well-formed block；其余保留在 response 中作为提示。

### Prompt fragment

- Default developer prompt：
  - 声明当前处于 Default。
  - 用户文本不会切换模式。
  - `request_user_input` 只在工具可用时使用。
- Plan developer prompt：
  - 只允许非 mutating exploration；禁止执行实现。
  - 最终官方方案必须包在 `<proposed_plan>` 块中。
  - `request_user_input` 有可用交互通道时优先使用。

Fragment 每轮独立注入，不写入 base instructions，避免污染 Default 模式默认 prompt。

### Goal continuation

- markdown 通过 `include_str!` 读取 `prompts/goals/continuation.md` 与 `prompts/goals/budget_limit.md`。
- 变量替换：`{{ objective }}` 在运行时替换为当前 goal objective。
- 新策略文本：
  - `Work from evidence`
  - `The audit must prove completion`
  - `strict blocked audit`
- 旧内联 continuation prompt 文本（如 `Avoid repeating work that is already done`）必须完全移除。

## CLI 交互

本模块不新增 CLI 命令。IM `/plan <message>` 由 `agent_slash` 统一解析。

## Web UI 交互

- Composer slash 面板：
  - 内置 Bifrost Agent runner：`/` 面板显示 `/plan` 与 `/compact`。
  - external runner：`/` 面板不显示 `/plan` / `/compact`。
- `/plan` 选中：composer 进入 Plan Mode 状态，显示可关闭的模式标记；发送时 payload 附带 `collaboration_mode: "plan"`。
- 用户手动键入 `/plan <message>`：前端不做特殊剥离，交给后端 slash 解析。
- 消息区：`proposed_plan` 单独渲染为「Plan Mode result」；`response` 空但 `proposed_plan` 非空时仍要显示方案正文。
- History timeline：JSONL `proposed_plan` 事件 replay 为 assistant 规划结果。

## Admin API

- `POST /api/agent/chat/stream`：body 支持 `collaborationMode` / `collaboration_mode`；SSE 事件流新增 `proposed_plan`；`run_finished` payload 含 `proposedPlan`。
- `POST /api/im-gateway/agent/chat`：同上；worker JSON IPC 保留 `collaborationMode` / `proposedPlan`。
- 后端在读取 body 时先做 slash 剥离与模式推断：显式 `collaboration_mode` 优先；否则解析 `/plan <message>` 剥离 slash 并推断 Plan Mode。

## Sync / 导入导出 / 分享边界

- Collaboration mode 是请求级参数，不参与 rule sync。
- JSONL `proposed_plan` 事件是本地历史；导出 session 时随 JSONL 一起。
- 分享/协作不在本轮范围。

## 实现切分

### Phase 1：模式参数与 prompt fragment

- 引入 `CollaborationMode`。
- Prompt builder 每轮附加 Default/Plan developer fragment。
- Worker JSON IPC 与 API 字段扩展。
- 单元测试覆盖 prompt 独立注入与默认值。

### Phase 2：`<proposed_plan>` 通道

- 解析器。
- `TurnResult.proposed_plan`、`AgentTurnProgressEvent::ProposedPlan`。
- SSE / worker / API 字段。
- JSONL 事件与 replay。
- Web UI 渲染与 history replay。

### Phase 3：Slash 入口与外部 runner 边界

- IM/API 后端统一解析 `/plan <message>`。
- Web UI slash 面板仅在内置 Agent 时展示 `/plan` / `/compact`。
- 外部 runner 手动输入不误剥离。

### Phase 4：Goal continuation 与 human_tests

- markdown `include_str!` 化。
- Goal continuation 单测覆盖 markdown 策略文本。
- 新增 human_tests 与 readme 索引。

## 测试方案

### 单元测试

- `proposed_plan::tests::*`：验证 proposal 提取、可见文本剥离、unterminated block 和非独占 tag 忽略。
- `prompt::tests::test_plan_mode_prompt_is_separate_developer_message`：Plan Mode developer prompt 独立注入且包含 `<proposed_plan>` 策略。
- `prompt::tests::test_default_mode_prompt_declares_default_mode`：Default mode prompt 默认注入。
- `tools::goal::tests::continuation_prompt_contains_remaining_tokens`：continuation prompt 变量替换生效。
- `tools::goal::tests::goal_prompt_rendering_uses_markdown_templates`：continuation/budget 渲染包含 markdown 文件独有策略文本且排除旧内联 continuation prompt 文本。
- `im_gateway::agent_worker::tests::turn_result_roundtrip_preserves_stop_fields`：worker IPC 保留 `proposedPlan`。
- `im_gateway::agent_slash::tests::*`：`/plan` slash 解析、非命令边界、multiline 消息与显式 mode 优先级。
- `persistence::tests::proposed_plan_event_roundtrip_restores_history_render`：JSONL replay 恢复 assistant 规划结果。

### E2E 测试

- `e2e-tests/tests/test_agent_plan_mode_human_api.sh`：启动真实 Bifrost + mock model，调用 `/api/im-gateway/agent/chat` 的 `collaboration_mode=plan`，验证：
  - 模型请求包含 Plan Mode developer prompt 与 `<proposed_plan>` 指令。
  - API `response` 不包含 `<proposed_plan>` 标签。
  - API `proposed_plan` 独立包含方案正文。
  - IM/API `/plan <message>` 入口在不显式传 `collaboration_mode` 时仍进入 Plan Mode，用户正文已剥离 `/plan`。
- `web/tests/ui/agent-chat.spec.ts` 的 `slash plan mode`：验证 Web UI slash 面板可选 `/plan`；输入框显示 Plan Mode 标记；发送 payload `message` 已剥离 slash 且含 `collaboration_mode: "plan"`；消息区展示 `proposed_plan` 结果。
- `web/tests/ui/agent-chat.spec.ts` 的 external runner 回归：切到 Codex/external runner 后 `/` 不展示 `/plan` / `/compact`，但 runner 选择仍可用。
- `web/src/pages/AI/AgentChatSection.timeline.test.ts`：persisted `proposed_plan` history event 渲染为 assistant 规划结果。
- `e2e-tests/tests/test_agent_goal_prompt_templates_human_api.sh`：启动真实 Bifrost + mock model，先创建 active goal，触发 worker 自动 continuation，验证：
  - continuation 模型请求包含 `Work from evidence`、`The audit must prove completion` 与 `strict blocked audit`。
  - continuation 模型请求不包含旧内联 prompt `Avoid repeating work that is already done`。
  - mock model 通过 `update_goal` 完成目标，避免 continuation 循环误判。

### 真实场景测试 human_tests

- `human_tests/agent-codex-plan-mode.md`：
  - TC-PM-01：静态文档同步。
  - TC-PM-02：真实 API Plan Mode proposal 独立字段。
  - TC-PM-03：Plan Mode 拦截 `update_plan`。
  - TC-PM-04：goal prompt markdown 真实接线。
  - TC-PM-05：Web UI slash 面板行为回归。
  - TC-PM-06：IM `/plan <message>` 入口回归。
  - TC-PM-07：external runner 不展示 `/plan` / `/compact`。
  - TC-PM-08：JSONL history replay 恢复 `proposed_plan` 渲染。
- 更新 `human_tests/readme.md` 索引与总数。

所有 human_tests 使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1` 与 `--no-system-proxy`。

### 覆盖率与项目校验

- E2E 测试必须先于 `rust-project-validate`。
- 收尾必须运行：
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  - `cargo test --workspace --all-features`
- 推送后使用 GitHub Actions PAT skill 盯 CI，直到远端 CI 全绿。
- 本机 no-local-coverage 约定生效时不执行 `make coverage`。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 目标复核：对照 Codex 差异清单，确认 Default/Plan prompt、proposal 输出、API/worker/SSE/IM、goal continuation 均已覆盖。
- 代码 review：Plan Mode 不误更新 `current_plan`；proposal 不污染 assistant history / memory；Default `update_plan` 保持兼容；`/plan` 只对内置 Agent；history replay 稳定。
- 复测：focused `cargo test`、新增 E2E、human_tests 用例。

### 第 2 轮

- 复查第 1 轮修复后的 `git diff`、新增文档索引、协议字段。
- 复跑受影响测试与 workspace 校验，确认无 exhaustive match、serde 字段或 prompt 变量遗漏。

## 风险与决策点

- Plan Mode 与 `update_plan` 边界必须严格：任何回归都会把 proposal 写成进度快照。
- 前端 slash 面板 gating：如果 runner 切换在 payload 到达后端前发生，需保证 `collaboration_mode` 与 runner 一致，否则外部 runner 会看到未剥离 slash。
- `<proposed_plan>` 解析对格式敏感：模型偶发输出未闭合 block 时必须回退为普通 response，不能吞掉最后消息。
- Goal continuation markdown 加载路径：`include_str!` 在 build 时定死，路径改动需要同步测试。
- 本次新增 Web UI slash 入口和 IM `/plan` 文本入口；README 暂不列出 Agent chat 内部命令，真实使用说明由 Web UI slash 菜单和 `human_tests/` 覆盖。
