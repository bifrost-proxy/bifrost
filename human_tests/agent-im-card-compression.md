# Agent IM 卡片进度信息压缩与过滤

## 功能模块说明

验证内置 Bifrost Agent 飞书 IM progress card 的状态区和执行过程区不会展示过期或无意义的进度噪声。该用例覆盖 payload 渲染层：`Status`、`ContextUpdated`、`CompactionFinished` 事件进入 `ImAgentProgressSnapshot` 后，卡片 JSON 中的状态标题、footer、context/token 指标和过程列表必须与最新事件一致，且不会被 `tool_calls`、`waiting_on_session`、`model_request`、`model_response` 等机器态状态刷屏。

## 前置条件

- 在仓库根目录执行。
- Rust toolchain 可用。
- 本用例不启动 Bifrost 服务、不连接真实飞书，不会修改系统代理。
- 若后续需要验证真实飞书发送链路，必须使用临时 `BIFROST_DATA_DIR` 并启动服务时带 `--no-system-proxy`，除非测试目标明确是系统代理。

## 测试用例列表

### TC-AICC-01：回归 - CompactionFinished 刷新已有飞书卡片 status 压缩次数

**背景**：飞书 progress card 过去只消费 `Status` 事件，忽略 `CompactionFinished`。如果压缩发生后没有立即收到新的 status，卡片可能继续显示旧的 `压缩：0 次`。

**操作步骤**：

1. 运行以下命令：
   ```bash
   cargo test -p bifrost-admin im_gateway::progress_card::tests::progress_snapshot_updates_status_from_compaction_context --lib -- --nocapture
   ```
2. 检查测试输出是否为 `1 passed`。

**预期结果**：

- 测试构造旧 status `compaction_count=0`，再注入 `CompactionFinished` 的 context `compaction_count=2`。
- `ActiveTurnStatus` 被回写为 `compaction_count=2`、`history_version=8`、最新 token/context 指标。
- 生成的飞书卡片 JSON 包含 `压缩：2 次`，不包含 `压缩：0 次`。

**本次执行结果**：通过。2026-05-24 执行 `cargo test -p bifrost-admin im_gateway::progress_card::tests::progress_snapshot_updates_status_from_compaction_context --lib -- --nocapture`，输出 `1 passed`；卡片 JSON 断言包含 `压缩：2 次` 和最新 token 指标，且不包含旧的 `压缩：0 次`。

### TC-AICC-02：回归 - 无 status 时 ContextUpdated 也能渲染压缩次数

**背景**：在卡片刚创建或 status 尚未抵达时，context progress event 仍应让卡片状态区展示压缩次数和 context/token 指标。

**操作步骤**：

1. 运行以下命令：
   ```bash
   cargo test -p bifrost-admin im_gateway::progress_card::tests::progress_snapshot_uses_context_when_status_is_not_available --lib -- --nocapture
   ```
2. 检查测试输出是否为 `1 passed`。

**预期结果**：

- 测试只注入 `ContextUpdated`，不注入 `Status`。
- 状态面板标题显示 `Token：累计 1.1K · 最近 77`。
- footer 显示 `Context：~1.2K / 10K (12.0%)` 和 `压缩：3 次`。

**本次执行结果**：通过。2026-05-24 首轮执行 `cargo test -p bifrost-admin im_gateway::progress_card::tests::progress_snapshot_uses_context_when_status_is_not_available --lib -- --nocapture` 失败，归因为 footer 在 `status=None` 且 `context=Some(...)` 时未渲染 context fallback 指标；修复后复跑同一命令输出 `1 passed`，footer 断言包含 `Context：~1.2K / 10K (12.0%)` 和 `压缩：3 次`。

### TC-AICC-03：机器态 Status 不进入飞书卡片执行过程

**背景**：Agent 运行过程中会持续产生 `tool_calls`、`waiting_on_session`、`model_request`、`model_response` 等机器态 `Status` 事件。过去这些事件会被渲染成多行 `状态：xxx`，导致飞书 progress card 的执行过程区域被无意义状态刷屏，挤占真正的思考、工具和结果内容。

**操作步骤**：

1. 运行以下命令：
   ```bash
   cargo test -p bifrost-admin im_gateway::progress_card::tests::machine_status_events_do_not_flood_process_card --lib -- --nocapture
   ```
2. 检查测试输出是否为 `1 passed`。
3. 检查断言覆盖卡片 JSON：不包含 assistant 流式正文，仍包含正在运行的工具调用，但不包含 `状态：tool_calls`、`状态：waiting_on_session`、`状态：model_request`、`状态：model_response` 或自定义蛇形机器态。
4. 运行以下命令确认外部 Runner footer 不会从 `当前状态：xxx` 漏出机器态：
   ```bash
   cargo test -p bifrost-admin im_gateway::progress_card::tests::external_runner_footer_hides_machine_status_line --lib -- --nocapture
   ```

**预期结果**：

- `Status` 事件仍更新运行状态上下文，但不会追加到执行过程 timeline。
- 飞书卡片执行过程只展示工具调用和可读状态，不展示会在最终区重复出现的 assistant 正文。
- 卡片 JSON 不包含 `状态：tool_calls`、`状态：waiting_on_session`、`状态：model_request`、`状态：model_response` 或 `custom_machine_state`。
- 外部 Runner footer 不展示 `当前状态：model_request`。

**本次执行结果**：通过。2026-07-11 执行 `cargo test -p bifrost-admin im_gateway::progress_card::tests::machine_status_events_do_not_flood_process_card --lib -- --nocapture` 和 `cargo test -p bifrost-admin im_gateway::progress_card::tests::external_runner_footer_hides_machine_status_line --lib -- --nocapture`，断言确认卡片 JSON 过滤 assistant 正文并保留 `正在运行：exec_command`，同时过滤过程区和 footer 中的机器态状态行。

## 清理步骤

- 本用例不创建持久临时目录，无需额外清理。
- 若命令失败，保留 cargo 输出用于归因；不得削弱断言或删除用例。
