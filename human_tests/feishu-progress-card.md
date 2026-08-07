# Feishu Progress Card 真实场景测试用例

## 功能模块说明

本模块验证飞书通道的 Agent progress card 会保留外部 Runner 的 `AssistantDelta` / 运行中 `AssistantFinal` 过程信息，同时把逐字符/累计快照归并成可读思考；最终正文只在卡片底部出现一次。工具、计划、可读状态、错误、token usage 刷新和可读执行耗时仍按原语义工作。Runner/Adapter 状态行还需在目标 Runner 创建 session 后立即展示其 Session ID；Codex 顶部状态展示 thread/session 累计 Token、7 天额度余额和任务耗时；长过程卡片把旧工具退化为名称、状态、耗时组成的步骤摘要，仅保留最近 5 次工具详情，超预算时按“思考/状态 + 对应工具”完整执行段裁剪。

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

### TC-FPC-08：长卡片保留工具执行脉络并按完整执行段裁剪

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
- 单条工具输入和输出分别最多展示 300 个 Unicode 字符，超过上限的尾部 marker 不出现在 CardKit payload；整体 payload 仍按 UTF-8 JSON 字节数和组件数预算。
- 旧工具不再完全消失，而是显示“步骤：工具名 · 完成/失败/执行中 · 耗时”的可读摘要，不展示参数和输出详情；最近 5 次工具仍使用可展开详情面板。
- “最多 30 次工具”窗口以及字节/组件预算裁剪都以完整执行段为边界：删除旧工具时，同时删除该轮位于工具之前的思考和状态，不能留下失去工具边界、最终粘连成大段的孤立思考。
- 长卡片仍能看出执行过哪些步骤、哪些步骤成功或失败；最近 5 次工具的输入/输出、顶部状态和最终结论保持可见，并展示省略提示；原始 snapshot 不丢历史。
- mock CardKit 返回 `200860` 或 `300305` 后，先在同一 card entity 使用收缩预算重试；连续两次限制错误后改用精简卡片，精简卡也被限额拒绝才 rollover；精简阶段的普通错误不创建重复消息。
- 非限制错误不进入裁剪重试；E2E 最终 CardKit JSON 不超过 24KB。

### TC-FPC-09：Runner 行实时展示目标 Runner Session ID

**操作步骤**：
1. 执行增量 metadata 和卡片 renderer 单元回归：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin --lib codex_like_progress_metadata_captures_target_runner_session_id_immediately -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin --lib external_runner_status_footer_uses_runner_metadata_instead_of_agent_metrics -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin --lib external_runner_footer_hides_machine_status_line -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin --lib external_runner_footer_bounds_and_escapes_session_id -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin --lib external_cli_progress_session_id_flows_from_live_event_to_feishu_card -- --nocapture
   ```
2. 执行最终 CardKit renderer E2E：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo run -p bifrost-e2e -- --test im_gateway_progress_card_budget_and_codex_resources --jobs 1 --timeout 120
   ```

**预期结果**：
- Codex `thread_id`、Trae `threadId` 和 Claude Code `session_id` 启动事件到达时立即写入运行中 metadata，无需等待 run 完成。
- 已创建目标 session 时，卡片包含 `Runner：codex · Adapter：codex · Session ID：thread-resource-e2e`，并继续保留“外部会话”详情行。
- 目标 Runner 尚未返回 session ID 时，Runner/Adapter 行不显示空值或 `N/A` Session ID。
- 超长或包含 Markdown backtick 的异常 ID 会被限制长度并安全转义，不能破坏卡片其余布局。
- E2E CardKit payload 在既有 24KB 预算内，原 Token、额度、耗时和过程信息不丢失。

## 清理步骤

1. 确认没有残留 `bifrost-e2e` 或测试启动的 Bifrost 进程。
2. 删除测试过程中生成的临时目录。

## 执行记录

- 2026-08-07：PASS — 新增 TC-FPC-09 后立即逐条执行。`codex_like_progress_metadata_captures_target_runner_session_id_immediately`、`external_runner_status_footer_uses_runner_metadata_instead_of_agent_metrics`、`external_runner_footer_hides_machine_status_line` 三项 focused 单测全部通过，覆盖 Codex `thread_id`、Trae `threadId`、Claude Code `session_id` 的运行中 metadata 合并，以及 Session ID 存在/缺失两种 Runner 行渲染。第一轮 Review/Fix/Test 发现异常 Session ID 可能过长或含 backtick，并补充 `external_runner_footer_bounds_and_escapes_session_id` 与 `external_cli_progress_session_id_flows_from_live_event_to_feishu_card`；首次链路测试因引用私有测试常量编译失败，改用公开协议值 `codex` 后两项均通过，完整验证 live event → metadata → runner summary → CardKit payload。`im_gateway_progress_card_budget_and_codex_resources` E2E 1 项通过，完整 CardKit JSON 在 24KB 预算内包含 `Runner：codex · Adapter：codex · Session ID：thread-resource-e2e`，并保留 Token、周额度、耗时和执行过程。当前未向真实飞书租户发送测试卡片；本轮真实场景证据来自当前源码生成的完整 CardKit payload。
- 2026-08-07：PASS — 第二轮 Review/Fix/Test 基于第一轮最新 diff 重新执行 `git status --short`、本需求 `git diff --check` 和 staged 检查；确认 9 个本需求文件与当前分支既有 breakpoint 改动边界清晰、暂存区为空。随后 `progress_card` 69 项、既有 `codex_progress_metadata` 2 项、live-event 链路 1 项和 CardKit E2E 1 项全部通过；未发现新的功能、布局、预算、文档或测试缺口，无需第三轮。

- 2026-07-23：PASS — 更新 TC-FPC-08 后立即逐条执行，并在 Review/Fix/Test 修复纯状态/思考退化边界及 CI 变更行覆盖率缺口后再次执行。`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin --lib progress_card -- --nocapture` 共 64 项全部通过，覆盖旧工具步骤摘要的完成/失败/执行中三态、最近 5 次工具可展开详情、旧思考/状态与对应工具按完整执行段裁剪、无工具时先删状态并保留最近 5 轮思考、30 工具窗口、UTF-8 字节/组件预算和限制错误降级；`im_gateway_progress_card_budget_and_codex_resources` E2E 1 项通过，最终 CardKit JSON 小于 24KB，至少保留一条无参数/输出详情的旧工具步骤及其对应思考，首个保留段之前不残留孤立思考或工具，最近 5 次工具输入/输出详情完整。专项 `coverage-diff.py` 验证变更生产 Rust 行覆盖率为 100%（71/71），通过 95% 门禁。当前未使用用户另一台设备的运行数据，也未向真实飞书租户发送测试卡片；本轮真实场景证据来自当前源码生成的完整 CardKit payload。

- 2026-07-14：PASS — 更新 TC-FPC-08 后立即逐条执行。`cargo test -p bifrost-admin --lib progress_card` 共 59 项全部通过，覆盖工具输入/输出 300 字符截断、30 工具窗口不连带删除前置思考、16KB 压力下优先删除旧工具并保留最近 5 轮思考、`200860`/`300305` 同卡收缩和精简降级；独立 `im_gateway_progress_card_budget_and_codex_resources` E2E 通过，真实 CardKit JSON 小于 24KB，包含 `THINKING_ROUND_35..39` 与最新工具 marker，不包含最旧思考或最新工具 300 字符之后的尾部 marker。

- 2026-07-13：PASS — 按 TC-FPC-07/08 逐条执行。真实 `codex-cli 0.144.1` app-server schema 包含 `account/rateLimits/read`、`account/rateLimits/updated`、`thread/tokenUsage/updated`；当前登录态只读快照返回 `limitId=codex`、`usedPercent=64`、`windowDurationMins=10080`、`resetsAt=1784490086`，输出未包含账号或凭据。资源解析 2 项、顶部状态 renderer 1 项、progress card 58 项单测全部通过；长卡片 E2E 1 项通过，确认最终 JSON 小于 24KB，顶部显示 session Token、周余额和耗时，旧过程被省略且最新 marker 保留。mock CardKit 覆盖 `200860`、`300305`、同卡收缩重试、精简模式、精简限额后的最终 rollover，以及精简普通错误不创建重复消息。当前环境未向真实飞书租户故意发送超限 payload，平台错误恢复由本地 mock API 验证。
- 2026-07-13：PASS — 按 TC-FPC-05 依次执行 4 组命令。文件行数边界单测、真实 Codex app-server `fileChange` 解析单测、既有卡片回归单测和独立 renderer E2E 全部通过；payload 包含文件路径及修改/新增/删除行数，并确认不存在“暂无工具详情”。CardKit payload 沿用标准 `grey` 背景和 Markdown，无硬编码主题颜色；当前环境未注入飞书测试凭据，未执行线上亮暗主题截图验证。
- 2026-07-13：PASS — 优化后的 TC-FPC-05 四组命令全部通过：`file_change_` 9 项单测、app-server 映射 1 项单测、进度卡回归 1 项单测、独立 renderer E2E 1 项用例。真实 payload 已验证纯正文新增/删除行数（含正文自身以 `+` / `-` 开头的边界）、工作区相对路径、工作区外路径保留、逐行缩进、“文件变更”标题和“已执行 N 个步骤”文案，且不显示“暂无工具详情”或工作区绝对路径前缀。
