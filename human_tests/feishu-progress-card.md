# Feishu Progress Card 真实场景测试用例

## 功能模块说明

本模块验证飞书通道的 Agent progress card 不会把外部 Runner 的 `AssistantDelta` / 运行中 `AssistantFinal` 重复展示到执行过程；最终正文只在卡片底部出现一次。工具、可读状态、错误、token usage 刷新和可读执行耗时仍按原语义工作。

## 前置条件

1. 当前目录位于仓库根目录。
2. 使用当前源码执行 Rust focused 测试和 `bifrost-e2e` renderer 用例。
3. 若需要真实飞书 API 验证，需配置 `FEISHU_APP_ID`、`FEISHU_APP_SECRET`、`FEISHU_OWNER_OPEN_ID`；无凭据时以本地 CardKit JSON payload 验证为准。

## 测试用例列表

### TC-FPC-01：逐字符 assistant stream 不进入执行过程

**操作步骤**：
1. 执行：
   ```bash
   cargo test -p bifrost-admin --lib assistant_stream_content_is_hidden_until_final_output -- --nocapture
   ```

**预期结果**：
- 测试通过。
- Running 卡片不包含逐字符 `AssistantDelta`，也不包含运行中的 `AssistantFinal`。
- `TurnFinished` 后卡片 body 只出现一次完整最终正文，且不存在仅由 assistant stream 产生的 `agent_process_panel`。

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

### TC-FPC-03：E2E renderer 隐藏 assistant stream 并保留真实过程事件

**操作步骤**：
1. 执行：
   ```bash
   cargo run -p bifrost-e2e -- --test im_gateway_agent_streaming_progress_card_renderer --jobs 1 --timeout 120
   ```

**预期结果**：
- E2E renderer 用例通过。
- progress card JSON 2.0 的 `agent_process_panel` 包含 `list_directory` 工具事件、耗时和状态。
- payload 不包含 `checking progress card sections`、逐字符中文或路径片段。
- 卡片 body 中 `streaming card done` 最终正文只出现一次。

### TC-FPC-04：token usage 机器态不展示为过程状态行

**操作步骤**：
1. 执行：
   ```bash
   cargo test -p bifrost-admin --lib codex_usage_progress_event_refreshes_status_without_timeline_noise -- --nocapture
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
   cargo test -p bifrost-admin --lib progress_footer_formats_elapsed_duration_without_milliseconds -- --nocapture
   ```
**预期结果**：
- 测试通过。
- duration formatter 输出 `1 分 05 秒`、`1 小时 03 分` 等可读耗时，不展示毫秒精度。

## 清理步骤

1. 确认没有残留 `bifrost-e2e` 或测试启动的 Bifrost 进程。
2. 删除测试过程中生成的临时目录。
