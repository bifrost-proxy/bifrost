# Feishu Progress Card 真实场景测试用例

## 功能模块说明

本模块验证飞书通道只过滤 Codex app-server 同一条 `agentMessage` 的逐 token delta，保留完整 Progress Step、reasoning、AssistantFinal 与工具时间线；同时验证单条过程正文不会被渲染成几字一行，token usage 机器态事件不会污染执行过程。

## 前置条件

1. 当前目录位于仓库根目录。
2. 使用当前源码执行 Rust focused 测试和 `bifrost-e2e` renderer 用例。
3. 若需要真实飞书 API 验证，需配置 `FEISHU_APP_ID`、`FEISHU_APP_SECRET`、`FEISHU_OWNER_OPEN_ID`；无凭据时以本地 CardKit JSON payload 验证为准。

## 测试用例列表

### TC-FPC-01：只过滤 Codex agentMessage token delta

**操作步骤**：
1. 执行：
   ```bash
   cargo test -p bifrost-admin --lib codex_app_server_agent_message_delta_is_not_mapped_to_progress_timeline -- --nocapture
   ```

**预期结果**：
- 测试通过。
- `item/agentMessage/delta` 不映射为 progress timeline，逐 token 片段不会逐行展示。
- `item/completed agentMessage` 仍映射为完整 `AssistantFinal` Progress Step。
- 普通 Runner AssistantDelta、reasoning 和工具事件不受影响。

### TC-FPC-02：Prose normalizer 保留列表和代码块

**操作步骤**：
1. 执行：
   ```bash
   cargo test -p bifrost-admin --lib progress_prose_linebreak_normalizer -- --nocapture
   ```

**预期结果**：
- 测试通过。
- 中文短行被合并，英文软换行补空格。
- Markdown 列表和 fenced code block 原始换行保留。

### TC-FPC-03：E2E renderer 输出的 progress card payload 不含短行断裂

**操作步骤**：
1. 执行：
   ```bash
   cargo run -p bifrost-e2e -- --test im_gateway_agent_streaming_progress_card_renderer --jobs 1 --timeout 120
   ```

**预期结果**：
- E2E renderer 用例通过。
- progress card JSON 2.0 的 `agent_process_panel` 中包含合并后的思考内容。
- payload 不包含 `启动\n检查`、`工作\n区` 或 `test_rule_share\n_confirm`。

### TC-FPC-04：token usage 机器态不展示为过程状态行

**操作步骤**：
1. 执行：
   ```bash
   cargo test -p bifrost-admin --lib progress_card -- --nocapture
   ```

**预期结果**：
- 测试通过。
- `token_usage_updated`、`token usage updated`、`token-usage-update` 都被判定为机器态状态。
- Feishu progress card process panel 不包含 `状态：token_usage_updated` 或同类 token usage 状态行。

### TC-FPC-05：file change 工具展开后展示文件变更详情

**操作步骤**：
1. 执行：
   ```bash
   cargo test -p bifrost-admin --lib feishu_progress_card_file_change_tool_expands_with_detail -- --nocapture
   ```

**预期结果**：
- 测试通过。
- Feishu progress card 的 `file_change` 工具展开详情包含文件路径、变更摘要和 diff 内容。
- 展开详情不显示 `暂无工具详情`。

### TC-FPC-06：卡片尾部展示可读执行耗时且 token usage 刷新耗时

**操作步骤**：
1. 执行：
   ```bash
   cargo test -p bifrost-admin --lib progress_card -- --nocapture
   ```
2. 执行：
   ```bash
   cargo run -p bifrost-e2e -- --test im_gateway_agent_streaming_progress_card_renderer --jobs 1 --timeout 120
   ```

**预期结果**：
- 测试通过。
- footer 中展示 `耗时：1 分 05 秒` 或 `耗时：2 分 05 秒` 这类可读耗时，不展示毫秒精度。
- token usage progress event 能触发 status refresh，但 process panel 不展示 `token usage updated`。

## 清理步骤

1. 确认没有残留 `bifrost-e2e` 或测试启动的 Bifrost 进程。
2. 删除测试过程中生成的临时目录。
