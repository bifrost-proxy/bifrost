# External Runner Plan UI

## 背景

Bifrost 已接入 Codex Runner 与 TraeX Runner，并把二者的执行过程输出归一化为 Web UI timeline 与飞书 progress card。但外置 Runner 的 plan/todo list 事件此前没有归一化，导致 Runner 真实输出了计划更新时，飞书卡片和 Web UI 都看不到任务计划。

## 真实输出验证

2026-06-25 在临时目录执行真实 Codex CLI 探针：

- 命令形态：`codex exec --json --ephemeral --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox -C <tmp> -`
- 结果：Codex 输出 `thread.started`、`turn.started` 后，计划事件为 `item.started` / `item.updated` / `item.completed`。
- 计划载荷：`item.type = "todo_list"`，条目位于 `item.items[]`，字段为 `text` 与 `completed`。
- 状态语义：当前 Codex todo list 只给 `completed=true/false`，没有直接输出 `in_progress`；因此 Bifrost 只能保守映射 `completed=true -> completed`，其余映射为 `pending`。

同日执行真实 TraeX CLI 探针：

- 命令形态：`traex exec --json --ephemeral --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox -C <tmp> -`
- 结果：TraeX 输出 `thread.started` 和 `turn.started` 后，本次 90 秒内没有继续输出 todo list 事件，探针被终止以避免长期占用资源。
- 兼容判断：TraeX 当前使用与 Codex 相同的 JSONL 协议入口，现有 TraeX fixture 已覆盖 `thread.started` / `turn.started` / `item.completed`；本次实现按同一 `todo_list` 事件形态兼容 TraeX，一旦 TraeX 输出该事件即可进入同一展示链路。

## 实现逻辑

新增 `ExternalCliProgressEventType::PlanUpdated`，在 external CLI JSONL parser 中识别：

- Codex/TraeX 协议：`type in ["item.started", "item.updated", "item.completed"]` 且 `item.type = "todo_list"`。
- 通用协议：`type = "plan_updated"` 或 `type = "todo_list"`，条目可位于 `items[]`。
- 条目文本字段优先级：`text`、`step`、`content`、`title`。
- 条目状态字段优先级：`completed=true` 映射 `completed`；否则解析 `status` 中的 `pending` / `in_progress` / `completed`；缺省为 `pending`。

事件输出后复用既有 UI 管道：

- 飞书卡片：`external_progress_to_agent_turn_event()` 把 `PlanUpdated` 转为 `AgentTurnProgressEvent::PlanUpdated`，`ImAgentProgressSnapshot` 已支持 `plan_steps` 并渲染任务计划面板。
- 标题处理：Codex/TraeX `todo_list` 是协议字段，不作为卡片标题；只有通用 `plan_updated` 明确携带 `title` / `name` 时才更新标题。
- Web UI：`record_external_cli_progress_event_to_timeline()` 把 `PlanUpdated` 写入 `ConversationRecorder::record_plan_updated()`；Agent Chat 历史与实时增量沿用现有 `plan_updated` telemetry 解析。
- 不新增前端专用协议，不为 Codex/TraeX 分叉 UI。

## 测试方案

### 单元测试

- `codex_cli_parser_maps_real_todo_list_events_to_plan_updates`：覆盖真实 Codex `todo_list` started/updated/completed JSONL。
- `generic_plan_updated_parser_accepts_status_fields`：覆盖通用 `plan_updated` 和 `in_progress` 状态。
- `external_progress_maps_to_agent_turn_progress_events`：覆盖 external plan event 到 `AgentTurnProgressEvent::PlanUpdated`。
- `external_runner_plan_progress_is_recorded_as_plan_updated_event`：覆盖 Web history 持久化。
- `external_runner_todo_list_plan_renders_in_feishu_progress_card`：覆盖飞书 card payload 展示。
- `historyEventsToTelemetry restores external runner plan updates from persisted history`：覆盖 Web UI telemetry 与过程步骤。

### E2E 测试

- 新增 `e2e-tests/tests/test_im_gateway_external_runner_plan_ui.sh`：使用真实 Bifrost 服务和 mock external runner 输出稳定 `plan_updated`，断言 `/chat/stream` 中出现 `plan_updated.steps`，run detail normalized events 保留 plan，session JSONL 持久化 `plan_updated`。
- 真实 Codex/TraeX 输出作为 `human_tests` 采样证据；TraeX 若未在限定时间输出 todo list，应记录为环境/Runner 输出差异，不把 Bifrost parser 判失败。
- Web UI 单测复用 Agent Chat history fixture，断言 external runner 历史页恢复任务计划。

### 真实场景测试

- 新增 `human_tests/external-runner-plan-ui.md`。
- 覆盖 Codex 真实 CLI 输出采集、TraeX 输出差异采集、飞书 card payload 渲染、Web UI history telemetry。
- 启动 Bifrost 服务时必须使用临时 `BIFROST_DATA_DIR`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核真实 Codex/TraeX 输出、parser 状态映射、飞书 card 事件路径、Web history 持久化路径。
- 运行 focused Rust 单测与 Web timeline 单测。
- 修复任何遗漏的 exhaustive match、状态映射或序列化问题。

### 第 2 轮

- 基于第 1 轮最新 diff 复查设计文档、human_tests、E2E 脚本和 coverage 缺口。
- 复跑 focused 单测、human_tests 命令、coverage 门禁和 workspace 校验。

## 校验要求

- E2E 与 human_tests 先于 `rust-project-validate`。
- 收尾必须运行 `make coverage`；若完整 E2E 覆盖环境不可用，退化为 `make coverage-unit` 并说明原因。
- 提交前至少执行一次 `cargo test --workspace --all-features`。
