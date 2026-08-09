# External Runner Plan UI 测试用例

## 功能模块说明

本模块验证 Codex Runner 与 Traex/Traex Runner 的 plan/todo list 输出被 Bifrost 解析为统一 `plan_updated`，并在飞书 progress card 与 Web UI Agent Chat 实时展示中可见；完整计划属于外部执行器过程数据，不写入 run artifact 或 session durable history。

## 前置条件

1. 当前目录位于仓库根目录。
2. 本地已安装 `codex`；Traex 真实采样需要本地已安装 `traex`。
3. 启动 Bifrost 服务时必须使用临时 `BIFROST_DATA_DIR`，并设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。
4. 真实 Codex/Traex CLI 采样可能依赖当前账号、网络和模型状态；稳定回归以 mock external runner E2E、Rust 单测和 Web 单测为准。

## 测试用例列表

### TC-ERP-01：真实 Codex JSONL 输出包含 todo_list plan 事件

**操作步骤**：
1. 执行真实 Codex 探针：
   ```bash
   tmpdir=$(mktemp -d)
   prompt='Do not edit files. First call the update_plan tool with exactly three steps: inspect output, map parser, verify UI. Then call update_plan again marking inspect output completed and map parser in_progress. Then answer exactly PLAN_STREAM_PROBE_DONE.'
   printf '%s\n' "$prompt" | codex exec --json --ephemeral --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox -C "$tmpdir" -
   ```

**预期结果**：
- stdout 包含 `thread.started` 和 `turn.started`。
- stdout 包含 `item.started` / `item.updated` / `item.completed`，其中 `item.type` 为 `todo_list`。
- `item.items[]` 至少包含 `text` 与 `completed` 字段。
- 当前 Codex 只输出 `completed=true/false`，不直接输出 `in_progress` 字段；Bifrost 应映射 `completed=true -> completed`，其余映射为 `pending`。

**实际结果（2026-06-25）**：
- 通过。真实输出包含 `{"type":"item.started","item":{"type":"todo_list","items":[...]}}`、`item.updated` 和 `item.completed`。
- `inspect output` 从 `completed:false` 更新为 `completed:true`；`map parser` 和 `verify UI` 仍为 `completed:false`。
- 最终输出 `PLAN_STREAM_PROBE_DONE`。

### TC-ERP-02：真实 Traex JSONL 输出协议采样

**操作步骤**：
1. 执行真实 Traex 探针：
   ```bash
   tmpdir=$(mktemp -d)
   prompt='Do not edit files. First call the update_plan tool with exactly three steps: inspect output, map parser, verify UI. Then call update_plan again marking inspect output completed and map parser in_progress. Then answer exactly TRAEX_PLAN_STREAM_PROBE_DONE.'
   printf '%s\n' "$prompt" | traex exec --json --ephemeral --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox -C "$tmpdir" -
   ```
2. 若 90 秒内没有新的 JSONL 事件，终止探针并记录当前输出。

**预期结果**：
- Traex 至少输出与 Codex 兼容的 `thread.started` / `turn.started` JSONL。
- 如果 Traex 输出 `todo_list`，Bifrost 按 TC-ERP-01 同一规则解析。
- 如果本次真实 Traex 未在限定时间输出 todo list，记录为 Runner/环境输出差异，不作为 Bifrost parser 失败。

**实际结果（2026-06-25）**：
- 通过。Traex 输出 `thread.started` 和 `turn.started`。
- 本次 90 秒内未继续输出 todo list 或最终消息，已终止探针；Bifrost parser 通过 Codex 真实 fixture 与通用 `plan_updated` fixture 覆盖 Traex 同协议兼容。

### TC-ERP-03：真实 Bifrost 服务实时推送 external runner plan，但不长期落盘

**操作步骤**：
1. 执行稳定 E2E：
   ```bash
   e2e-tests/tests/test_im_gateway_external_runner_plan_ui.sh
   ```

**预期结果**：
- 脚本输出 `PASS`。
- `/chat/stream` 返回 `plan_updated` 事件，`steps` 状态为 `completed`、`in_progress`、`pending`。
- run detail normalized events 不包含 `plan_updated` 或计划文本。
- session JSONL 不包含 `plan_updated`、`inspect output`、`map parser`、`verify UI`，但包含最终回复 `BIFROST_EXTERNAL_PLAN_UI_OK`。

**实际结果**：
- 通过。2026-06-25 的旧版本验证了实时 stream；2026-08-09 按“外部执行器详细过程不落档”的新契约更新脚本并重新执行，实时 plan 保留，run detail/session history 不再保存完整步骤。

### TC-ERP-04：Rust focused 测试覆盖解析、飞书卡片和 live-only 持久化边界

**操作步骤**：
1. 执行：
   ```bash
   cargo test -p bifrost-admin --lib external_runner_progress_is_live_while_durable_timeline_keeps_tool_summary_only
   cargo test -p bifrost-admin --lib external_runner_todo_list_plan_renders_in_feishu_progress_card
   cargo test -p bifrost-admin --lib codex_cli_parser_maps_real_todo_list_events_to_plan_updates
   cargo test -p bifrost-admin --lib generic_plan_updated_parser_accepts_status_fields
   cargo test -p bifrost-admin --lib external_progress_maps_to_agent_turn_progress_events
   ```

**预期结果**：
- 所有测试通过。
- 解析层产生 `ExternalCliProgressEventType::PlanUpdated`。
- 飞书 card payload 包含任务计划面板和计划条目。
- live stream 保留 `plan_updated`，durable history 不保存 plan、模型 delta 或完整工具参数/结果。

**实际结果**：
- 通过。2026-06-25 已执行：
  - `cargo test -p bifrost-admin --lib external_runner_plan_progress`：旧契约下 2 passed；2026-08-09 已由 live-only durable timeline 回归替代持久化断言。
  - `cargo test -p bifrost-admin --lib codex_cli_parser_maps_real_todo_list_events_to_plan_updates`：1 passed。
  - `cargo test -p bifrost-admin --lib generic_plan_updated_parser_accepts_status_fields`：1 passed。
  - `cargo test -p bifrost-admin --lib external_progress_maps_to_agent_turn_progress_events`：1 passed。
  - `cargo test -p bifrost-admin --lib external_runner_todo_list_plan_renders_in_feishu_progress_card`：1 passed。

### TC-ERP-05：Web UI 兼容旧 history 中的 external runner plan

**操作步骤**：
1. 执行：
   ```bash
   pnpm --dir web test AgentChatSection.timeline.test.ts -- --runInBand
   ```

**预期结果**：
- 测试通过。
- `historyEventsToTelemetry` 可从旧版 persisted `plan_updated` 恢复 plan。
- `historyEventsToMessages` 可把旧版 external runner plan 作为同轮 assistant 的过程步骤；新运行不再产生该 durable event。

**实际结果**：
- 通过。2026-06-25 执行 `pnpm --dir web run test:unit AgentChatSection.timeline.test.ts`，`1 passed`，共 `15` 个测试通过。

## 清理步骤

1. 删除测试临时目录。
2. 确认没有残留 Bifrost 测试进程。
3. 若手动执行真实 Codex/Traex 探针且进程长时间无输出，应终止探针。
