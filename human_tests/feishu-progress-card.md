# Feishu Progress Card 真实场景测试用例

## 功能模块说明

本模块验证飞书通道的 Agent progress card 会保留外部 Runner 的 `AssistantDelta` / 运行中 `AssistantFinal` 过程信息，同时把逐字符/累计快照归并成可读思考；最终正文只在卡片底部出现一次。工具、计划、可读状态、错误、token usage 刷新和可读执行耗时仍按原语义工作。Codex 顶部状态还需展示 thread/session 累计 Token、7 天额度余额和任务耗时，长过程卡片则需在飞书限制内保新去旧并自动降级。

## 前置条件

1. 当前目录位于仓库根目录。
2. 使用当前源码执行 Rust focused 测试和 `bifrost-e2e` renderer 用例。
3. 若需要真实飞书 API 验证，需配置 `FEISHU_APP_ID`、`FEISHU_APP_SECRET`、`FEISHU_OWNER_OPEN_ID`；无凭据时以本地 CardKit JSON payload 验证为准。

## 测试用例列表

### TC-FPC-01：逐字符 assistant stream 归并为可读过程且最终不重复

**操作步骤**：
1. 执行：
   ```bash
   cargo test -p bifrost-admin --lib assistant_stream_fragments_are_coalesced_and_terminal_duplicate_is_removed -- --nocapture
   ```

**预期结果**：
- 测试通过。
- Running 卡片包含归并后的完整思考正文与 `agent_process_panel`，不包含逐字符硬换行。
- snapshot 只有一条 thinking item，不会为每个字符生成一条 timeline。
- `TurnFinished` 后等价的末尾 thinking 被移除，卡片 body 只出现一次完整最终正文。

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

### TC-FPC-03：E2E renderer 归并 assistant stream 并保留完整过程事件

**操作步骤**：
1. 执行：
   ```bash
   cargo run -p bifrost-e2e -- --test im_gateway_agent_streaming_progress_card_renderer --jobs 1 --timeout 120
   ```

**预期结果**：
- E2E renderer 用例通过。
- progress card JSON 2.0 的 `agent_process_panel` 同时包含归并后的 `启动检查飞书过程卡片` 思考、`list_directory` 工具事件、耗时和状态。
- payload 不包含 `启动\\n检查` 等逐字符硬换行。
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
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin --lib file_change_ -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin --lib app_server_file_change_notification_includes_paths_and_line_stats -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin --lib feishu_progress_card_file_change_tool_expands_with_detail -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo run -p bifrost-e2e -- --test im_gateway_file_change_progress_card_renderer --jobs 1 --timeout 120
   ```

**预期结果**：
- 4 组测试全部通过。
- Codex app-server 的真实 `fileChange` 完成事件从 `params.item.changes[]` 提取详情，不依赖为空的 `result` 字段。
- Feishu progress card 的 `fileChange` / `file_change` 工具标题显示为“文件变更”，展开详情包含工作区相对路径、变更摘要和 diff 内容。
- unified diff 按文件汇总为“修改 N 行”“新增 N 行”“删除 N 行”；`+++` / `---` 文件头不计入统计。
- app-server 只返回新增/删除文件正文时，按正文逻辑行数显示“新增 N 行”或“删除 N 行”；多行正文的每一行保持一致缩进。
- 执行过程标题显示“已执行 N 个步骤”，不把 `fileChange` 误称为命令；工作区外路径保持原样，不进行错误截断。
- 展开详情不显示 `暂无工具详情`。
- 卡片只使用 CardKit 标准 `grey` 背景与 Markdown 文本，不写死亮色或暗色值；飞书亮色、暗色主题由 CardKit 自适应。无飞书测试凭据时仅验证 payload，不宣称完成线上双主题截图测试。

### TC-FPC-06：卡片尾部展示可读执行耗时且 token usage 刷新耗时

**操作步骤**：
1. 执行：
   ```bash
   cargo test -p bifrost-admin --lib progress_footer_formats_elapsed_duration_without_milliseconds -- --nocapture
   ```
**预期结果**：
- 测试通过。
- duration formatter 输出 `1 分 05 秒`、`1 小时 03 分` 等可读耗时，不展示毫秒精度。

### TC-FPC-07：Codex 顶部状态展示 session Token、周余额和耗时

**操作步骤**：
1. 确认本机 Codex 支持 app-server 协议并生成 schema：
   ```bash
   codex --version
   rm -rf .tmp-codex-app-server-schema
   codex app-server generate-json-schema --out .tmp-codex-app-server-schema
   rg -n 'account/rateLimits/read|account/rateLimits/updated|thread/tokenUsage/updated|windowDurationMins|usedPercent' .tmp-codex-app-server-schema/codex_app_server_protocol.v2.schemas.json
   ```
2. 使用当前登录态向 Codex app-server 发出只读 `account/rateLimits/read`，确认返回窗口中存在 `windowDurationMins=10080`；记录 `usedPercent` 和 `resetsAt`，不得输出账号邮箱或凭据。
3. 执行资源字段解析和最终 CardKit renderer：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin --lib codex_progress_metadata -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin --lib external_runner_status_footer_uses_runner_metadata_instead_of_agent_metrics -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo run -p bifrost-e2e -- --test im_gateway_progress_card_budget_and_codex_resources --jobs 1 --timeout 120
   ```

**预期结果**：
- app-server schema 同时包含额度初始快照、滚动更新和 thread 累计 Token 通知。
- 只有 10080 分钟窗口被识别为 Codex 周额度；短时窗口不会误展示为周额度。
- 卡片顶部折叠标题展示“本次 N Token”“周余额 N%”“耗时 N”；展开状态展示输入、输出、缓存输入、推理输出、周额度已用/剩余和本地时区重置时间。
- 无额度数据时省略额度字段，不伪造 0% 或剩余 Token 数；额度读取失败不阻断 Codex 任务。

### TC-FPC-08：长卡片预算裁剪与飞书限额错误自动降级

**操作步骤**：
1. 执行完整 progress card 单元与 mock CardKit 回归：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin --lib progress_card -- --nocapture
   ```
2. 再执行长卡片 renderer E2E：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo run -p bifrost-e2e -- --test im_gateway_progress_card_budget_and_codex_resources --jobs 1 --timeout 120
   ```

**预期结果**：
- 单条工具输入/输出最多展示 1000/3000 个 Unicode 字符，整体 payload 仍按 UTF-8 JSON 字节数和组件数预算。
- 长卡片从最旧过程开始裁剪，保留最新工具、顶部状态和最终结论，并展示省略提示；原始 snapshot 不丢历史。
- mock CardKit 返回 `200860` 或 `300305` 后，先在同一 card entity 使用收缩预算重试；连续两次限制错误后改用精简卡片，精简卡也被限额拒绝才 rollover；精简阶段的普通错误不创建重复消息。
- 非限制错误不进入裁剪重试；E2E 最终 CardKit JSON 不超过 24KB。

## 清理步骤

1. 确认没有残留 `bifrost-e2e` 或测试启动的 Bifrost 进程。
2. 删除测试过程中生成的临时目录。

## 执行记录

- 2026-07-13：PASS — 按 TC-FPC-07/08 逐条执行。真实 `codex-cli 0.144.1` app-server schema 包含 `account/rateLimits/read`、`account/rateLimits/updated`、`thread/tokenUsage/updated`；当前登录态只读快照返回 `limitId=codex`、`usedPercent=64`、`windowDurationMins=10080`、`resetsAt=1784490086`，输出未包含账号或凭据。资源解析 2 项、顶部状态 renderer 1 项、progress card 50 项单测全部通过；长卡片 E2E 1 项通过，确认最终 JSON 小于 24KB，顶部显示 session Token、周余额和耗时，旧过程被省略且最新 marker 保留。mock CardKit 覆盖 `200860`、`300305`、同卡收缩重试、精简模式、精简限额后的最终 rollover，以及精简普通错误不创建重复消息。当前环境未向真实飞书租户故意发送超限 payload，平台错误恢复由本地 mock API 验证。
- 2026-07-13：PASS — 按 TC-FPC-05 依次执行 4 组命令。文件行数边界单测、真实 Codex app-server `fileChange` 解析单测、既有卡片回归单测和独立 renderer E2E 全部通过；payload 包含文件路径及修改/新增/删除行数，并确认不存在“暂无工具详情”。CardKit payload 沿用标准 `grey` 背景和 Markdown，无硬编码主题颜色；当前环境未注入飞书测试凭据，未执行线上亮暗主题截图验证。
- 2026-07-13：PASS — 优化后的 TC-FPC-05 四组命令全部通过：`file_change_` 9 项单测、app-server 映射 1 项单测、进度卡回归 1 项单测、独立 renderer E2E 1 项用例。真实 payload 已验证纯正文新增/删除行数（含正文自身以 `+` / `-` 开头的边界）、工作区相对路径、工作区外路径保留、逐行缩进、“文件变更”标题和“已执行 N 个步骤”文案，且不显示“暂无工具详情”或工作区绝对路径前缀。
