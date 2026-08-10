# External Runner Plan UI

## 背景

Bifrost 的 IM Gateway 会作为聚合层承接多个外部 Runner（Codex CLI、Traex CLI、Cursor CLI 等），把它们输出的 JSONL 事件流归一化后：

1. 转成 `AgentTurnProgressEvent`，用于飞书 progress card 与其他 IM 通道的实时进度展示；
2. 通过 `/chat/stream` SSE 推送到 Web UI 与调用方，作为增量渲染源；
3. 计划步骤属于外部执行器的高频过程数据，只在当前运行内存中保留，不写入 durable history。

Codex/Traex Runner 的执行过程输出（thinking、file edits、shell exec、tool call、summary）此前已经归一化。缺口在 **plan / todo list** 类事件：Runner 真实输出计划更新时，飞书卡片没有"任务计划"面板，Web UI Agent Chat 的 timeline 也不会显示计划步骤。这会让用户在 IM 或 Web 上看不到"接下来 Runner 打算做什么"，从而对长任务失去可见性。

本文档定义 external runner plan 事件的归一化、渲染与持久化契约，同时说明真实 Codex/Traex 探针结果、飞书 card 与 Web UI 的更新点、Web UI 任务计划胶囊的 hover 行为，并给出测试与 Review/Fix/Test 闭环方案。

## 用户目标验证清单

### 必须实现

- 新增 `ExternalCliProgressEventType::PlanUpdated`，覆盖以下真实 Codex/Traex 事件形态：
  - Codex/Traex 协议：`type in ["item.started", "item.updated", "item.completed"]` 且 `item.type = "todo_list"`；条目位于 `item.items[]`。
  - 通用协议：`type = "plan_updated"` 或 `type = "todo_list"`；条目位于 `items[]`。
- 条目文本字段优先级：`text` → `step` → `content` → `title`。
- 条目状态字段优先级：`completed=true` → `completed`；否则解析 `status` 中 `pending` / `in_progress` / `completed`；缺省 `pending`。
- 飞书 progress card 收到 `AgentTurnProgressEvent::PlanUpdated` 时渲染任务计划面板；`ImAgentProgressSnapshot.plan_steps` 已有的展示逻辑复用。
- Web UI Agent Chat 实时 timeline 消费 `/chat/stream` 的 `plan_updated`；刷新后的 durable history 不恢复完整计划步骤，避免把外部执行器过程数据变成滚动日志。
- Web UI 任务计划胶囊采用 hover 展开详情浮层；胶囊、桥接区、浮层共享同一展开状态，浮层内选文本不会被误关闭，离开整组区域后延迟关闭。

### 必须不破坏

- 现有 Codex/Traex `thread.started` / `turn.started` / `item.completed` 归一化路径保持不变。
- 现有飞书 progress card 的 thinking、tool call、file edits 展示不变。
- Web UI Agent Chat 现有 durable history 恢复不变；旧版本已经保存的 `plan_updated` 仍可兼容读取。
- 不为 Codex/Traex 分叉 UI；不新增前端专用协议。
- 现有 IM Gateway session JSONL 保持向后兼容，但新运行不再追加 `plan_updated`。
- Traex 短时间未输出 todo list 不视为 parser 失败，只作为 Runner 行为差异记录。

### 必须真实验证

- 真实运行 Codex CLI 探针，采集 `todo_list` 事件并验证 parser 输出 `PlanUpdated`。
- 真实运行 Traex CLI 探针，记录当前 Runner 是否在合理窗口内输出 todo list；若未输出，需要作为 human_tests 环境差异记录。
- 飞书真实 progress card 或 mock card payload 中出现任务计划面板。
- Web UI Agent Chat 的 `/chat/stream` 实时流能看到任务计划胶囊，hover 可展开详情；完成后刷新不要求保留完整计划。
- E2E 场景 `test_im_gateway_external_runner_plan_ui.sh` 端到端通过。

## 产品语义

### 计划是"Runner 意图的可见摘要"

计划事件与 tool call/file edits 不同：它反映 Runner 内部的"接下来我打算做的步骤"，用户看到计划就能判断长任务的进度、是否走偏。计划面板应当在真实进度更新时刷新，不做假信息补齐。

### Codex / Traex 走同一入口，避免 UI 分叉

Codex 使用 `item.type = "todo_list"` 载荷；Traex 兼容同一 JSONL 协议入口，一旦输出该事件即进入同一展示链路。Bifrost parser 不为 Codex/Traex 分别写解析器分支。

### 计划状态映射保守

Codex 当前 `todo_list` 只输出 `completed=true/false`，没有直接的 `in_progress`。Bifrost 采用保守映射：`completed=true → completed`，其余映射为 `pending`。仅当通用协议明确携带 `status = in_progress` 时才映射为 `in_progress`。

### 标题只在通用协议携带时更新

Codex/Traex 的 `todo_list` 是协议字段，不作为卡片标题；只有通用 `plan_updated` 携带 `title` / `name` 字段时才更新飞书卡片标题。

### 胶囊 hover 是"整组状态"

Web UI 上任务计划胶囊、胶囊与浮层之间的透明桥接区域、浮层本身共享同一展开状态；三者中任意一个 hover 都保持展开，全部离开后延迟关闭。这样用户可以从胶囊移动到浮层选文本而不误关。

## 真实输出验证

### Codex CLI 探针（2026-06-25）

- 命令：`codex exec --json --ephemeral --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox -C <tmp> -`。
- 输出：Codex 在 `thread.started` / `turn.started` 后输出 `item.started` / `item.updated` / `item.completed`。
- 计划载荷：`item.type = "todo_list"`，条目位于 `item.items[]`，字段 `text` 与 `completed`。
- 状态语义：只出现 `completed = true/false`，无 `in_progress`。

### Traex CLI 探针（2026-06-25）

- 命令：`traex exec --json --ephemeral --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox -C <tmp> -`。
- 输出：`thread.started` / `turn.started` 后 90 秒内未继续输出 todo list 事件；探针终止避免长期占用。
- 兼容判断：Traex 使用与 Codex 相同 JSONL 协议入口，现有 Traex fixture 已覆盖 `thread.started` / `turn.started` / `item.completed`；本设计按同一 `todo_list` 事件形态兼容 Traex，一旦 Traex 输出即进入同一展示链路。

## 技术细节

### Parser 分支

`crates/bifrost-admin/src/im_gateway/external_cli/mod.rs`：

- 新增 `ExternalCliProgressEventType::PlanUpdated { steps: Vec<PlanStep>, title: Option<String> }`。
- Codex/Traex 分支：`type ∈ {"item.started", "item.updated", "item.completed"}` 且 `item.type == "todo_list"` → 提取 `item.items[]` 转为 `PlanStep { text, status }`。
- 通用分支：`type ∈ {"plan_updated", "todo_list"}` → 提取 `items[]`。
- 文本字段：`text || step || content || title`。
- 状态字段：`completed=true → Completed`；否则 `status` 解析 `pending` / `in_progress` / `completed`；缺省 `Pending`。
- 标题：仅通用协议携带 `title` / `name` 时提取。

### 飞书 progress card

- `external_progress_to_agent_turn_event()`：把 `PlanUpdated` 映射为 `AgentTurnProgressEvent::PlanUpdated { steps, title }`。
- `ImAgentProgressSnapshot`：`plan_steps` 字段已支持渲染，无需新增卡片模板；只需在 event 层接通。
- 卡片标题：Codex/Traex 不更新标题；只有通用协议携带 `title` 才覆盖。

### Web UI timeline

- `record_external_cli_progress_event_to_timeline()`：`PlanUpdated` 为 live-only，不写入 `ConversationRecorder`。
- Agent Chat 实时增量沿用 `plan_updated` telemetry 解析。
- `historyEventsToTelemetry` 继续兼容旧版本已经持久化的 plan updates，但新运行不会产生这类 durable event。

### Web UI 任务计划胶囊

- 胶囊 + 桥接区 + 浮层共享 `open` state（React `useState` + `useRef` 计时器）。
- 三者的 `onMouseEnter` 都置为 `open`；`onMouseLeave` 只启动延迟关闭定时器（建议 200–300 ms），另外两者 `onMouseEnter` 会取消该定时器。
- 浮层内文本选择：selection change 期间不关闭；`document.selection` / `window.getSelection()` 非空时延迟关闭再延长一次。
- 胶囊触发区域必须至少 24 px 高度，桥接区无边框但可捕获鼠标，避免用户在缝隙里"掉出去"。

### 持久化

- IM Gateway session JSONL：不写入 `plan_updated`、步骤文本和标题；只保存最终助手回复、紧凑工具摘要与子 Agent 终态摘要。
- external runner `normalized_events.jsonl` / `result.json`：不保存 plan 或模型增量；完整 plan 只在 live stream 生命周期内存在。
- Web UI history：继续兼容旧记录中的 `plan_updated`，不要求新记录跨刷新恢复计划。
- 不新增新的数据库表；沿用现有 session run 存储。

## CLI / API / IM 集成

- CLI：不新增用户可见命令；`bifrost agent status` 等命令若展示计划，可复用同一事件流（后续扩展）。
- API：`/chat/stream` SSE 增加 `plan_updated` payload；`/agent/history` 只会在读取旧版本记录时返回历史 `plan_updated`。
- IM：飞书 card 自动渲染 `plan_steps`；其他 IM 通道（Lark 未来的 mini-card、Slack 等）沿用 `AgentTurnProgressEvent::PlanUpdated`。

## Sync 边界

- Plan events 属于 live-only 运行时事件，不进入 session run history 或 Sync；不新增 sync 契约字段。
- IM Gateway 与 Sync 服务器交互不变。

## 实现切分

### Phase 1：Parser & event 类型

- `ExternalCliProgressEventType::PlanUpdated` 新增。
- Codex/Traex 与通用 JSONL 分支解析。
- 单元测试覆盖真实 Codex `todo_list` fixture 与通用 `plan_updated` fixture。

### Phase 2：飞书 progress card

- `external_progress_to_agent_turn_event` 映射 `PlanUpdated`。
- `ImAgentProgressSnapshot.plan_steps` 接通；card payload snapshot 测试。

### Phase 3：Web UI timeline

- `record_external_cli_progress_event_to_timeline` 明确把 plan 保持为 live-only。
- `historyEventsToTelemetry` 仅承担旧 persisted history 的向后兼容。
- 任务计划胶囊 hover 组件（胶囊 + 桥接 + 浮层共享状态）。

### Phase 4：E2E 与真实场景

- 新增 `e2e-tests/tests/test_im_gateway_external_runner_plan_ui.sh`。
- Web UI Playwright case 覆盖 hover 展开、跨缝隙、浮层内文本选择。
- 更新 `human_tests/external-runner-plan-ui.md`。

## 测试方案

### 单元测试

- `codex_cli_parser_maps_real_todo_list_events_to_plan_updates`：真实 Codex `todo_list` started/updated/completed JSONL 覆盖。
- `generic_plan_updated_parser_accepts_status_fields`：通用 `plan_updated` 与 `in_progress` 状态。
- `external_progress_maps_to_agent_turn_progress_events`：external plan event → `AgentTurnProgressEvent::PlanUpdated`。
- `external_runner_progress_is_live_while_durable_timeline_keeps_tool_summary_only`：plan 不进入 durable history。
- `external_runner_todo_list_plan_renders_in_feishu_progress_card`：飞书 card payload 展示。
- `historyEventsToTelemetry restores external runner plan updates from persisted history`：只验证旧记录向后兼容。

### E2E 测试

- `e2e-tests/tests/test_im_gateway_external_runner_plan_ui.sh`：使用真实 Bifrost 服务和 mock external runner 输出稳定 `plan_updated`，断言 `/chat/stream` 中出现完整 `plan_updated.steps`；run detail 与 session JSONL 不保存计划文本，session 仍保存最终回复。
- 真实 Codex/Traex 输出作为 `human_tests` 采样证据；Traex 若在限定时间未输出 todo list，记录为环境/Runner 输出差异，不判 parser 失败。
- Web UI 单测复用旧 Agent Chat history fixture，断言既有 `plan_updated` 记录仍能恢复任务计划。
- Web UI Playwright：任务计划胶囊 hover 展开、慢速跨过胶囊与浮层缝隙、浮层内文本选择、离开后延迟关闭。

### 真实场景测试 human_tests

- 更新 `human_tests/external-runner-plan-ui.md`：
  - TC-ERP-01：Codex 真实 CLI 输出采集与 parser 输出。
  - TC-ERP-02：Traex 输出差异采集。
  - TC-ERP-03：飞书 card payload 渲染任务计划面板。
  - TC-ERP-04：Web UI Agent Chat 历史页恢复任务计划。
  - TC-ERP-05：Web UI 实时流展示任务计划胶囊 hover。
- 启动 Bifrost 时使用临时 `BIFROST_DATA_DIR`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

### 覆盖率与项目校验

- E2E 与 human_tests 需优先于 `rust-project-validate`。
- 收尾执行 `cargo test --workspace --all-features`、`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`。
- 若完整 E2E 覆盖环境不可用，退化为 `make coverage-unit` 并说明原因；本机 no-local-coverage 约定下可豁免 `make coverage`。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：Codex/Traex parser、飞书 card、Web 实时流、Web UI hover 组件是否全部落地，且完整 plan 不落档。
- 复核 diff：`ExternalCliProgressEventType` 是否 exhaustive match；状态映射是否保守；title 是否只在通用协议时更新。
- 复核真实 Codex/Traex 输出、parser 状态映射、飞书 card 事件路径与 durable history 排除 plan 的边界。
- 复测：focused Rust 单测、Web timeline 单测、飞书 card payload snapshot 单测。

### 第 2 轮

- 复核第 1 轮修复的 diff。
- 复核 human_tests、E2E 脚本、coverage 缺口。
- 复核 Web UI 胶囊 hover 组件的实际交互（延迟关闭、文本选择、桥接区）。
- 复测：focused 单测、human_tests 命令、workspace 校验。

## 风险与决策点

- **Runner 输出漂移**：Codex 后续可能新增 `in_progress` 状态；本方案保守映射，届时只需扩展状态映射表。
- **Traex 迟迟不输出 plan**：视为 Runner 行为差异；不作为 Bifrost 失败。若 Traex 后续支持，Parser 无需改动。
- **Web UI hover 误关**：桥接区高度/延迟阈值需要真实用户测试；建议提供 e2e 交互测试并预留延迟时间常量。
- **飞书 card 空计划**：Runner 只输出 `title` 无 `steps` 时，plan 面板应显示"无步骤"或不渲染；建议不渲染避免误导。
- **持久化字段膨胀**：完整 plan snapshot 已设为 live-only；历史只保留最终回复与紧凑摘要，不通过落盘重建执行器内部过程。
