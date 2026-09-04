# Feishu Progress Card 真实场景测试用例

## 功能模块说明

本模块验证飞书通道的 Agent progress card 会保留外部 Runner 的 `AssistantDelta` / 运行中 `AssistantFinal` 过程信息，同时把逐字符/累计快照归并成可读思考；Codex 依次输出公开 reasoning summary、commentary delta 和相同 commentary final 时，每段过程只展示一次。执行过程默认折叠：运行态“最新进展”只展示最新公开解释，当前工具和本轮成功/失败/执行中统计合并到“执行过程”折叠标题单行展示；最终结论出现后移除“最新进展”。运行中若连续 10 秒没有模型事件，session 保活循环必须局部刷新卡片；底部展示设备本地时间的“处理中... · 耗时：天/时/分/秒 · 最后更新：YYYY-MM-DD HH:mm:ss”，方便判断任务是否仍活跃。过程输出中的 Markdown 图片需先上传飞书并以 `image_key` 原位渲染。任务结束后，原任务卡的状态摘要、任务计划/实施方案、执行过程和最终结论统一默认折叠；原任务卡结论与另发的完整终态卡复用同一 `image_key`，不补发重复的独立图片消息。随后另发的成功卡显示多语言“任务执行结束 / Task completed”等标题并包含最后一次最终总结，失败卡显示多语言失败标题并包含异常原因；终态卡直接引用刚结束的任务卡，并继续自动上传和发送最终总结显式引用的本地文档与压缩包。卡片折叠面板使用飞书主题自适应默认背景/文本色，需在亮色和暗色主题下保持可读。工具、计划、可读状态、错误、token usage 刷新和可读执行耗时仍按原语义工作。Runner/Adapter 状态行还需在目标 Runner 创建 session 后立即展示其 Session ID；Codex 顶部状态展示 thread/session 累计 Token、7 天额度余额和任务耗时；长过程卡片把旧工具退化为名称、状态、耗时组成的步骤摘要，仅保留最近 5 次工具详情，超预算时按“思考/状态 + 对应工具”完整执行段裁剪。

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
- 执行过程标题显示“共 N 步 · 工具 M 次”，不把总体步骤和工具次数混为一个口径，也不把 `fileChange` 误称为命令；工作区外路径保持原样，不进行错误截断。
- 展开详情不显示 `暂无工具详情`。
- 卡片只使用 CardKit 标准 `default` 背景与 `text_color=default`，Markdown 文本不写死亮色或暗色值；飞书亮色、暗色主题由 CardKit 自适应。无飞书测试凭据时验证完整 payload 的主题安全契约，并明确记录未执行线上双主题截图测试。

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
- 旧工具不再完全消失，而是逐条显示为“`- 工具名 · 完成/失败/执行中 · 耗时`”Markdown 列表项，不展示参数和输出详情；最近 5 次工具仍使用可展开详情面板。
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
- 已创建目标 session 时，顶部折叠标题立即包含 `Session：thread-resource-e2e`，展开详情继续包含 `Runner：codex · Adapter：codex · Session ID：thread-resource-e2e` 与“外部会话”行。
- 目标 Runner 尚未返回 session ID 时，运行态顶部显示 `Session：获取中`；终态仍无 ID 时显示 `Session：未提供`，不显示空值或 `N/A`。
- 超长或包含 Markdown backtick 的异常 ID 会被限制长度并安全转义，不能破坏卡片其余布局。
- E2E CardKit payload 在既有 24KB 预算内，原 Token、额度、耗时和过程信息不丢失。

### TC-FPC-10：长任务结束后折叠过程卡并发送多语言终态卡、附件

**操作步骤**：
1. 执行成功、失败和通知发送失败的 HTTP 集成回归：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin --lib external_runner_success_finishes_progress_card_and_sends_terminal_summary -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin --lib external_runner_failure_finishes_progress_card_and_sends_terminal_reason -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin --lib external_runner_terminal_send_failure_does_not_rollback_finished_progress_card -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin --lib terminal_progress_card_collapses_status_plan_process_and_conclusion -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin --lib failed_progress_card_collapses_failure_conclusion_in_standard_and_compact_cards -- --nocapture
   ```
2. 使用当前 debug 二进制执行真实 Service + mock Runner + loopback Feishu OpenAPI 黑盒回归：
   ```bash
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_feishu_progress_terminal_notification.sh
   ```

**预期结果**：
- 成功和失败路径都先更新原任务进度卡，再各发送且只发送一张新终态卡。
- 成功终态的原任务卡中，状态摘要、任务计划/实施方案、执行过程和最终结论均为 `collapsible_panel` 且 `expanded=false`；失败终态使用同样的折叠策略，结论标题为“失败结论”。运行中计划仍展开，执行过程默认折叠，摘要保持可见。
- compact 降级卡的成功/失败结论也保持默认折叠，不能重新裸露与终态卡重复的正文。
- 成功终态卡为绿色 header，默认标题为 `Task completed`，通过 `i18n_content` 覆盖飞书支持的 16 个 locale，其中 `zh_cn=任务执行结束`、`ja_jp=タスク実行完了`；正文包含最后一个非空 `AssistantFinal` 的最终总结。
- 失败终态卡为红色 header，默认标题为 `Task failed`，包含相同的 16-locale i18n 集合；正文包含真实异常原因。
- 消息请求路径形成 `用户原消息 → progress card message_id → terminal card` 引用链；终态卡不得再次直接引用用户原消息。
- 原任务卡仍保持单卡正文去重语义；终态卡是独立主动通知，不由 progress snapshot 重试产生。
- 最终总结引用的本地文档、`tar.gz` 压缩包与 `next-harness.yaml` 配置都经 `/im/v1/files` 上传后，以独立 `msg_type=file` 消息发送到同一 IM 会话；同一总结中的 `.rs` 源码链接不上传。过程卡折叠不得影响附件解析、上传和发送。
- 若终态卡发送失败，原任务卡仍保持 Finished/Failed 终态，outbound message log 记录失败，session/queue 收尾不回滚。

### TC-FPC-11：折叠摘要、Session 三态、压缩包与暗色主题契约

**操作步骤**：
1. 执行当前轮摘要、Session 三态、主题颜色和归档扩展名单元回归：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin --lib feishu_progress_card_collapses_process_with_inline_current_round_status -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin --lib external_runner_status_title_exposes_session_id_lifecycle -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin --lib generated_feishu_cards_use_theme_adaptive_colors -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin --lib agent_reply_collects_local_and_remote_archive_attachments -- --nocapture
   ```
2. 执行完整 renderer 与隔离 Service 黑盒链路：
   ```bash
   target/debug/bifrost-e2e --test im_gateway_progress_card_budget_and_codex_resources --jobs 1 --timeout 180
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_feishu_progress_terminal_notification.sh
   ```
3. 若当前租户凭据可用，在飞书客户端分别切换亮色/暗色主题，查看运行态摘要、手动展开过程面板、成功终态和失败终态；再触发一次进度更新确认面板仍可操作。若凭据不可用，记录未执行线上截图，使用步骤 1/2 的完整 CardKit JSON 契约作为本地验证证据。

**预期结果**：
- 运行过程 `agent_process_panel` 初始 `expanded=false`，其前方 `agent_process_sum` 只展示最后一次公开解释；当前工具（最多三个类型）和本轮成功/失败/执行中次数合并进过程折叠标题，在一行展示；展开后原时间线与工具详情完整。
- 运行态精简卡中的最新解释也只出现一次；底部 `agent_output` 只展示“处理中”活动时间行，不重复最新解释。
- 最终结论或失败结论出现后，standard 与 compact 原任务卡均不再包含 `agent_process_sum` 或“最新进展”；过程面板和最终结论仍保留且默认折叠。
- 折叠标题展示“共 N 步 · 工具 M 次”及当前轮工具状态，不再把工具数误称为全部步骤数。
- 外部 Runner Session ID 在 live metadata 到达后立即刷新；无 ID 的运行/终态分别显示“获取中/未提供”。
- 本地和远程 `tar.gz/tgz/tar.xz/tar.zst/7z/rar` 等显式链接被识别为文件附件；黑盒 E2E 的 `.txt` 与 `.tar.gz` 均真实调用上传并各发送一条文件消息。
- 所有折叠面板均为 `background_color=default`，标题为 `text_color=default`；payload 不含固定 `grey`、黑白、十六进制或 RGB/RGBA 样式。亮色/暗色客户端均可读；无租户凭据时不伪造线上截图结论。

### TC-FPC-12：过程卡与双终态卡原位渲染同一张图片

**操作步骤**：
1. 执行共享图片解析器和进度 Registry 聚焦测试：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin --lib feishu_markdown::tests -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin --lib progress_registry_ -- --nocapture
   ```
2. 使用当前 debug 二进制执行隔离 Service + mock Runner + loopback Feishu OpenAPI 黑盒回归：
   ```bash
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_feishu_progress_terminal_notification.sh
   ```
3. 检查当前环境是否具备真实租户凭据：
   ```bash
   for key in FEISHU_APP_ID FEISHU_APP_SECRET FEISHU_OWNER_OPEN_ID; do
     [[ -n "${!key:-}" ]] && echo "$key=set" || echo "$key=missing"
   done
   ```
   若三个变量均存在，再向测试会话投递一条包含本地 PNG 的过程事件和终态，分别查看运行态、原任务卡结论和独立终态卡；否则记录真实租户 UI 验证阻塞，不输出变量值、不伪造客户端渲染结论。

**预期结果**：
- 本地相对路径按 Runner work dir 解析，远程 HTTP 图片先下载再上传；既有 `img_*` 不重复上传，fenced code block 中的图片示例不上传，缺图只在原位置降级成“未能上传”文案。
- 图片语法跨多个流式 delta 时，在闭合后按累计 Markdown 上传；远程响应超过 10 MiB 时在读取完整 body 前拒绝，不向飞书上传。
- mock `/im/v1/images` 收到内联 `terminal-e2e-chart.png` 与普通 SVG 链接同名的 `terminal-e2e-flow.png` 预览，并返回 `img_v3_terminal_e2e`。
- 运行中过程卡、原任务卡最终结论和独立 `Task completed` 终态卡的 Markdown 均包含 `![E2E chart](img_v3_terminal_e2e)`；原本地绝对路径不进入 CardKit payload。
- 显式 Markdown 图片继续只内联复用，不补发独立图片；普通 SVG 链接的同名 PNG 预览发送一条 `msg_type=image`，SVG 本体发送一条 `msg_type=file`。
- 普通 `.txt`、`.tar.gz`、`.yaml` 和 `.svg` 分别调用 `/im/v1/files` 并发送 `msg_type=file`，图片内联不破坏非图片附件；`.rs` 源码链接不调用上传接口。
- 无真实租户凭据时，本地完整 HTTP/CardKit payload 契约必须通过，并明确将飞书客户端肉眼渲染标记为未执行，而不是假设通过。

### TC-FPC-13：出站文件 100 MiB 产品上限与非阻塞失败提示

**操作步骤**：
1. 使用当前 debug 二进制执行隔离 Service + mock Runner + loopback Feishu OpenAPI：
   ```bash
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_feishu_progress_terminal_notification.sh
   ```
2. mock Runner 的成功结论同时显式链接一个 `100 MiB + 1 byte` 稀疏文件和一个小文本文件；mock `/im/v1/files` 对小文本文件返回飞书错误码 `234006`。
3. 检查请求日志：绿色 `Task completed` 终态卡先发布；超限文件不进入 `/im/v1/files`；小文件发生一次上传尝试但不产生 `msg_type=file` 消息；随后只补发一张汇总提示卡。
4. 检查提示卡和消息日志包含两个失败文件名、100 MiB 产品上限、上传错误码及“任务结论已正常发布”；确认 Bifrost 进程仍健康且 Session 已 idle。

**预期结果**：Bifrost 允许不超过 100 MiB 的飞书出站文件进入上传链路，`100 MiB + 1 byte` 在本地拒绝。飞书公开接口文档仍声明 30 MB 上限，因此平台返回 `234006` 时按单附件失败降级。超限、空文件、上传失败或文件消息发送失败均只影响对应附件，其余附件继续；终态卡和任务状态不回滚，提示卡发送失败也不会使事件循环或服务异常。入站引用消息仍独立执行 100 MiB 单文件、250 MiB 单 Turn 总预算。

### TC-FPC-14：执行过程历史步骤与多行输出不再粘连

**操作步骤**：
1. 执行历史步骤列表、段落边界和最近五次详情的聚焦单元回归：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin --lib process_timeline_keeps_latest_thirty_tool_calls_with_omission_notice -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin --lib old_tools_render_as_list_items_while_latest_five_keep_expandable_details -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin --lib process_tool_detail_caps_input_and_output_previews_at_three_hundred_chars -- --nocapture
   ```
2. 使用当前源码构建的 E2E runner 执行长过程 CardKit renderer：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo run -p bifrost-e2e -- --test im_gateway_progress_card_budget_and_codex_resources --jobs 1 --timeout 180
   ```
3. 检查 E2E 生成的 `agent_process_panel`：历史工具摘要内容包含 `THINKING_ROUND_N\n\n- \`tool_N\` · 完成 · 10ms`；最后一个工具详情包含 `LATEST_MARKER\nSECOND_OUTPUT_LINE`；完整 JSON 小于 24 KiB。

**预期结果**：
- 被降级为摘要的历史工具逐条使用 `-` Markdown 列表项，状态和耗时仍可见；不再出现多个“步骤：工具名”被普通单换行折叠后连成一整行。
- 公开解释、状态、子 Agent 与工具列表之间至少保留一个空行，飞书 CardKit 按块级段落渲染，长过程可从上到下扫读。
- 最近 5 次工具仍为可展开详情面板，输入/输出最多 300 个 Unicode 字符的既有裁剪不变；fenced code block 中的多行输出保留真实换行。
- 30 次工具窗口、完整执行段裁剪、默认折叠、主题自适应和 24 KiB / 180 组件预算不退化。

### TC-FPC-15：最终消息自动发送易读配置文件但排除源码

**操作步骤**：
1. 执行配置扩展名、远程 MIME、源码拒绝优先级聚焦单测：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin --lib agent_reply_collects_config_attachments_but_excludes_source_code -- --nocapture
   ```
2. 使用当前源码构建 debug 二进制并执行隔离 Service + mock Runner + loopback Feishu OpenAPI：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo build --bin bifrost
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_feishu_progress_terminal_notification.sh
   ```
3. 检查 mock `/im/v1/files` multipart 文件名：必须包含 `next-harness.yaml`，不得包含 `terminal-e2e-handler.rs`；成功任务必须多一条 `msg_type=file` 配置消息，既有 `.txt`、`.tar.gz`、图片与失败降级行为保持通过。

**预期结果**：
- 本地及远程显式链接中的 `yaml/yml`、`toml`、`ini`、`cfg`、`conf/cnf`、`config`、`properties`、`xml`、`jsonc/json5`、`hcl`、`tfvars`、`plist`、`xcconfig` 被识别为配置附件；远程 YAML/TOML/XML MIME 能生成对应后缀。
- `.rs/.py/.js/.ts/.go/.sql/.css` 等源码后缀不自动发送；即使 Markdown 标签包含 `file/附件/download`，或 URL 来自受信下载域名、通过 `filename=` 查询参数携带源码名，也不能绕过拒绝判断。
- `.env` 等未列入配置白名单且可能携带敏感凭据的文件不因后缀自动发送；系统不扫描工作目录，只处理最终回复显式链接。
- 配置附件继续受普通文件、去重、飞书 100 MiB / Weixin 30 MiB provider 上限与非阻塞失败规则约束，不改变任务成功状态。

### TC-FPC-16：Codex reasoning 前缀后的 commentary 终态快照不重复

**操作步骤**：
1. 执行聚焦单元回归：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin --lib assistant_final_does_not_repeat_commentary_after_reasoning_prefix -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin --lib assistant_stream_keeps_repeated_tokens_and_word_boundaries -- --nocapture
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin --lib assistant_stream_fragments_are_coalesced_and_terminal_duplicate_is_removed -- --nocapture
   ```
2. 使用当前源码构建 debug Bifrost，并执行隔离 Service + mock Runner + loopback Feishu OpenAPI 黑盒链路：
   ```bash
   BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_feishu_progress_terminal_notification.sh
   ```
3. 检查终态原任务卡：不再包含 `agent_process_sum` / “最新进展”；`agent_process_panel` 标题单行包含当前工具与本轮三态计数，其详情中的 `E2E_REASONING_PREFIX` 与 `E2E_LATEST_EXPLANATION` 各只出现一次，工具详情和最终结论仍正常。

**预期结果**：
- `reasoning summary → commentary delta → 相同 commentary final` 归并为一条 thinking item，final 被识别为已经存在的规范化后缀，不再追加第二份 commentary，也不丢失前面的 reasoning summary。
- 工具事件形成边界后，下一轮相同序列仍独立归并且不重复。
- 普通 `AssistantDelta` 的合法重复 token 不被全局去重，`"哈" + "哈" + " " + "done"` 仍为 `"哈哈 done"`。
- mock Feishu 收到的完整 CardKit JSON 中过程面板无重复 marker，终态没有“最新进展”，工具状态合并进过程标题；终态通知、图片上传复用、附件发送和失败降级既有行为不退化。

### TC-FPC-17：10 秒静默保活、精细耗时与设备最后更新时间

**操作步骤**：
1. 执行进度卡聚焦单元回归：
   ```bash
   SKIP_FRONTEND_BUILD=1 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1 cargo test -p bifrost-admin --lib progress_card -- --nocapture
   ```
2. 使用当前源码构建 debug Bifrost，并执行包含静默 Runner 的隔离黑盒链路：
   ```bash
   SKIP_FRONTEND_BUILD=1 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1 cargo build --bin bifrost
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_feishu_progress_terminal_notification.sh
   ```
3. mock Runner 先发送 `run_started`，随后静默 12 秒再发送 `E2E_HEARTBEAT_FINAL`；检查 `card_1` 在终态前是否自动收到 `agent_output` 与 `agent_status_panel` 元素更新。
4. 检查底部活动行和状态摘要中的时间；再检查终态后 session 的保活条件已关闭。

**预期结果**：
- 任意运行态可见刷新后连续 10 秒无新事件时自动刷新；新模型事件会重置 10 秒静默窗口，不在固定整点无条件刷卡。
- 保活只更新稳定元素，不新增 thinking/status/tool timeline，不创建新消息或新卡片；同一 session 被替换、完成、失败或没有卡片句柄时停止。
- 底部单行符合 `处理中... · 耗时：... · 最后更新：YYYY-MM-DD HH:mm:ss`，最后更新时间使用 Bifrost 所在设备本地时区并在每次刷新时推进。
- 耗时依次显示 `10 秒`、`1 分 05 秒`、`1 小时 03 分 01 秒`、`1 天 01 小时 01 分 01 秒`；不展示 `0 天` 等前导零高位单位，高位出现后保留两位低位字段。
- 静默 12 秒的真实 Service + mock Runner + mock Feishu 链路至少产生一次 10 秒保活更新，随后终态结论、Reason 去重、终态密度、图片与附件行为继续通过。

### TC-FPC-18：Codex 本地文件链接行号后缀附件回归

**操作步骤**：
1. 执行本地附件路径聚焦单元测试：
   ```bash
   SKIP_FRONTEND_BUILD=1 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1 cargo test -p bifrost-admin --lib agent_reply_resolves_codex_source_positions_for_local_attachments -- --nocapture
   ```
2. 使用当前源码构建 debug Bifrost，并执行隔离黑盒链路：
   ```bash
   SKIP_FRONTEND_BUILD=1 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1 cargo build --bin bifrost
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_feishu_progress_terminal_notification.sh
   ```
3. mock Runner 的最终总结中放入 `[完整方案](/临时目录/方案.md:1)`，检查 mock Feishu `/im/v1/files` 的 multipart 文件名和随后发送的 `msg_type=file` 消息。
4. 同时检查单元用例中的绝对路径、相对路径、`file://`、`:line:column`、真实文件名含 `:数字`、缺失文件与远程 URL 边界。

**预期结果**：
- 原始路径存在时原样使用，合法的 `literal.md:7` 不会错误改写成 `literal.md`。
- 原始路径不存在时，`:line` 或 `:line:column` 仅在剥离后的候选是现存文件时被识别为源码位置。
- `[完整方案](.../方案.md:1)` 实际上传文件名为 `方案.md`，不会尝试上传不存在的 `方案.md:1`，并发送一条对应的飞书文件消息。
- 相对路径按 Runner 工作目录解析，`file://` 路径同样兼容；缺失文件和远程 URL 不会被误当成本地附件。
- 测试只启动临时数据目录、动态端口的隔离 Bifrost 与 loopback mock Feishu，结束后由 trap 清理，不操作当前运行服务。

### TC-FPC-19：最终回复按真实路径发现全类型文件，不依赖 Markdown 格式

**操作步骤**：
1. 执行路径解析聚焦单元回归：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin agent_reply_ -- --nocapture
   ```
2. 使用当前 debug 二进制执行隔离 Service + mock Runner + loopback Feishu OpenAPI：
   ```bash
   SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_feishu_progress_terminal_notification.sh
   ```
3. 核对原始复现场景中的普通 Markdown 链接：技术方案 `.md`、两张流程图 `.svg` 及同名 `.png` 均真实存在；再核对裸绝对路径、工作目录相对路径、反引号内含空格路径、`:line[:column]`、重复路径、缺失文件、目录、代码块、源码与敏感配置。
4. 检查 mock 请求：普通 SVG 链接产生同名 PNG 图片预览和 SVG 文件消息；裸 `.txt` 路径、归档、YAML 和带位置后缀的 Markdown 文档分别按文件发送。

**预期结果**：
- 文件发现不要求 `![...](...)` 或 `[...] (...)` 特定格式；绝对路径、相对 Runner work dir 路径及常见包裹形式只要解析到真实普通文件且命中既有扩展白名单，就进入原生发送队列。
- PNG/JPEG/GIF/WEBP/BMP 走图片消息；文档、配置、Office、归档、patch/diff、音频、视频和 SVG 走既有文件/媒体模式。
- SVG 有同名 PNG 时发送 PNG 预览，同时保留 SVG 文件；飞书不把 `image/svg+xml` 错传给不支持 SVG 的图片 API。
- canonical path 去重；缺失文件、目录、fenced code block 中的示例、源码和 `.env*`/credentials/secrets/私钥不发送。
- 显式 Markdown 图片仍在 CardKit 原位复用 `image_key`；新增路径发现不破坏终态卡、失败通知和 provider-aware 非阻塞门禁。

## 清理步骤

1. 确认没有残留 `bifrost-e2e` 或测试启动的 Bifrost 进程。
2. 删除测试过程中生成的临时目录。

## 执行记录

- 2026-08-13：PASS（TC-FPC-19 路径驱动全类型 artifact）— 更新用例后立即执行 `agent_reply_` 聚焦测试；第一轮 review 补齐父目录、ASCII/中文标签与标点、canonical symlink 拒绝、缺失/未知 Markdown 图片保留，第二轮补齐裸 SVG 不隐式预览及 SVG 预览 symlink 拒绝，最终复跑 29/29 通过。原始 MD + 两个 SVG 普通链接解析为技术方案文件、两个 SVG 文件及两个同名 PNG 预览，裸绝对/相对/包裹路径、去重、缺失/目录/code fence、源码和敏感配置拒绝均通过。随后用当前 debug 二进制执行隔离 Service + mock Runner + loopback Feishu OpenAPI，黑盒 E2E PASS：显式 chart 继续 CardKit 内联，普通 SVG 链接额外上传同名 PNG 并发送一条 image message，SVG 本体与裸 report、归档、YAML、带行号 Markdown 分别发送 file message；`.rs` 未上传，30 MiB 和上传错误仍汇总为非阻塞提示。测试 trap 已清理进程和临时目录。

- 2026-08-12：PASS（TC-FPC-18 Codex 本地文件链接行号后缀附件回归）— 更新用例后立即执行聚焦单测与隔离黑盒链路。单测覆盖绝对路径 `:line`、带空格路径 `:line:column`、相对路径、`file://`、真实存在的 `literal.md:7` 优先、缺失文件和远程 URL；隔离 Service + mock Runner + loopback Feishu OpenAPI 验证 `[完整方案](.../方案.md:1)` 最终调用 `/im/v1/files` 上传真实 `方案.md`，未上传 `方案.md:1`，并发送对应 `msg_type=file` 消息。既有终态卡、图片、报告/归档/配置附件、源码排除、30 MiB 预检、上传失败非阻塞提示和 10 秒静默保活断言继续通过；测试 trap 已清理所属进程和临时目录，未操作当前运行服务。

- 2026-08-12：PASS（TC-FPC-15 配置附件与源码排除）— 在独立 `codex/feishu-config-attachments` worktree、最新 `origin/main@8820d5ef` 上执行。`agent_reply_collects_config_attachments_but_excludes_source_code` 聚焦单测通过，覆盖本地 `next-harness.yaml`、远程 `filename=service.TOML`、YAML/TOML/XML MIME、全部配置扩展名与大小写，以及显式 `source file` 标签、受信下载域名、URL 编码的 `worker%2Epy` 均不能绕过源码拒绝；`.env.production`、`credentials/secrets` 和私钥文件名即使带宽泛 `file` 标签也被拒绝。随后构建当前 debug 二进制并执行隔离 Service + mock Runner + loopback Feishu OpenAPI，黑盒 E2E PASS：成功任务新增 `next-harness.yaml` multipart 上传与 `msg_type=file` 消息，`terminal-e2e-handler.rs` 未进入 `/im/v1/files`，既有 `.txt`、`.tar.gz`、内联图片、30 MiB 预检和上传失败非阻塞提示全部保持通过。第 1 轮 review 补强敏感配置与 URL 编码边界后，聚焦单测和同一黑盒 E2E 均再次通过。首次独立 worktree 构建因磁盘只余 240 MiB 报 `errno=28`；仅清理该 worktree 5.9 GiB 可再生成 target 和共享 target 的 Cargo incremental 缓存后，以 `CARGO_INCREMENTAL=0` 增量构建复跑通过，未删除源码、用户数据、release 产物或现有 debug 二进制。

- 2026-08-12：PASS（TC-FPC-17 静默保活与时间可观测性）— 更新用例后立即执行 111 项 progress card 聚焦单测，结果全通过；覆盖 10 秒静默阈值、9 秒时剩余 1 秒、Finished 后停止、本地 `YYYY-MM-DD HH:mm:ss` 格式，以及秒/分秒/时分秒/天时分秒规则。随后用当前 debug Bifrost 执行隔离 Service + 静默 12 秒 mock Runner + loopback Feishu OpenAPI 黑盒 E2E，结果 PASS：`card_1` 在终态前局部更新 `agent_output` 和 `agent_status_panel`，底部出现 10+ 秒耗时与设备本地最后更新时间，未新增过程条目；12 秒后正常发布 `E2E_HEARTBEAT_FINAL`，既有 Reason 去重、终态隐藏“最新进展”、工具状态单行、图片/附件及失败降级断言全部通过，测试 trap 已清理进程和临时目录。

- 2026-08-12：PASS（TC-FPC-11/16 终态密度与 Reason 去重联合回归）— 更新用例后立即执行 6 项聚焦单测，验证运行态 `agent_process_sum` 只保留最新公开解释，当前工具与本轮成功/失败/执行中统计合并进 `agent_process_panel` 标题单行展示，成功/失败的 standard 与 compact 终态卡均移除 `agent_process_sum` / “最新进展”；Reason/commentary 后缀去重、工具边界和合法重复 token 同时通过。随后以 `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1` 构建当前源码的 debug Bifrost，并执行隔离 Service + mock Runner + loopback Feishu OpenAPI 黑盒 E2E，结果 PASS：终态过程标题包含“当前工具：暂无正在执行的工具 · 本轮工具：成功 1 · 失败 0 · 执行中 0”，`E2E_REASONING_PREFIX` 与 `E2E_LATEST_EXPLANATION` 在过程详情各出现一次，图片复用、终态通知、附件发送和非阻塞失败提示保持通过；测试 trap 已清理所属进程和临时目录。

- 2026-08-12：PASS（TC-FPC-16 Codex Reason/commentary 重复回归）— 截图、真实 CardKit payload 和 Codex rollout 共同确认旧卡片把公开 reasoning summary、commentary delta 合并后，又把相同 commentary final 追加成第二条。更新用例后立即逐条执行：`assistant_final_does_not_repeat_commentary_after_reasoning_prefix`、合法重复 token 和既有终态归并 3 个聚焦单测全部通过。隔离 Service + mock Runner + loopback Feishu OpenAPI 黑盒 E2E 首次因独立 worktree 的 Cargo debug 构建占满磁盘而失败（`No space left on device`）；仅清理该 worktree 2.9 GiB 可再生 incremental 缓存后，以 `CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1` 原命令重跑通过。最终原任务卡 `agent_process_panel` 中 `E2E_REASONING_PREFIX` 与 `E2E_LATEST_EXPLANATION` 各出现一次，折叠摘要、工具详情、图片复用、终态通知、附件发送及非阻塞失败提示均保持通过；测试 trap 已清理隔离服务和临时目录。

- 2026-08-11：PASS（TC-FPC-14 执行过程可读性回归）— 截图与群消息 `om_x100b688669cf78b8c2a0077a2205800` 先确认旧卡将普通单换行折叠为空格，造成 `select_page` / `evaluate_script` 等历史步骤和公开解释粘成一整行。更新用例后立即逐条执行：历史 30 工具窗口、旧工具列表与 300 字符详情裁剪 3 个聚焦单测全部通过；`im_gateway_progress_card_budget_and_codex_resources` renderer E2E 通过，直接断言 `agent_process_log` 中公开解释与 `- \`tool_N\` · 完成 · 10ms` 之间存在空段落，最后一个工具详情保留 `LATEST_MARKER\nSECOND_OUTPUT_LINE`，完整 CardKit JSON 仍小于 24 KiB，最近 5 次可展开详情、主题自适应和裁剪边界不变。首次 E2E 构建因磁盘仅余 396 MiB 在链接阶段报 `errno=28`，仅清理 `bifrost-e2e` 可再生成构建缓存后重跑；随后一次断言误在 JSON 序列化字符串中匹配真实换行，改为读取详情组件 `content` 后复跑通过。当前运行中的正式 Bifrost 仍是修复前二进制，未重启承载本 Agent 的服务，因此未伪造修复后飞书客户端截图结论；线上肉眼复核留待新版本部署后执行。

- 2026-09-05：PASS（TC-FPC-13 出站文件 100 MiB 产品上限与失败降级）— 隔离 Service + mock Runner + loopback Feishu OpenAPI 验证绿色 `Task completed` 终态卡先成功发布，`100 MiB + 1 byte` 稀疏文件在本地被拒绝且未调用上传接口，小文件上传返回飞书错误码 `234006` 后未发送 file 消息；两项失败汇总到一张“附件发送提示（不影响任务结论）”卡。另由出站发送 E2E 验证 `32 MiB + 1 byte` 文件可完整穿过 CLI、Admin、worker spool 与 mock Feishu 上传链路，证明旧 30/32 MiB 内部门槛已移除。Session 正常 idle、服务继续健康，测试 trap 已清理临时目录和所属进程。飞书公开文档仍声明上传不超过 30 MB，因此 30–100 MiB 文件若被平台拒绝，继续按逐附件失败降级。

- 2026-08-11：PASS（rebase 后终态过程说明回归复测）— TC-FPC-12 在最新 `origin/main` 上首次重跑时发现：同一图片在 `AssistantDelta` 中已转换为 `image_key`，但 `TurnFinished` 仍携带本地路径，导致最终结论未从过程时间线去重，`agent_process_sum` 错误覆盖最新过程说明。修复文本比较键对 Markdown 图片目标的归一化，并新增 `progress_registry_keeps_delta_explanation_when_terminal_reuses_uploaded_image`；聚焦 Registry、共享解析器和黑盒 E2E 复测均通过，终态同时保留 `E2E_LATEST_EXPLANATION` 与 `E2E_FINAL_SUMMARY_SUCCESS`，图片仍只上传一次并渲染为 `img_v3_terminal_e2e`。

- 2026-08-11：PASS（第 2 轮 Review/Fix/Test）— 复查发现逐 delta 解析无法处理跨分片 `![alt](path)`，改为先合并进度快照再在 session mutex 外解析累计 Markdown，并补 `progress_registry_uploads_markdown_image_split_across_deltas`；同时把远程图片改为 Content-Length 预检 + 分片累计 10 MiB 硬限额，补超大响应回归。共享解析器 6 项、progress Registry 4 项、标准回复 1 项测试通过；重新构建当前 `target/debug/bifrost` 后，黑盒 E2E 再次 PASS，确认没有复用旧二进制造成假通过。

- 2026-08-11：PASS（本地完整 HTTP/CardKit 链路）— 新增 TC-FPC-12 后立即逐条执行。共享图片解析器 5 项测试全部通过，覆盖既有 `img_*`、fenced code block、缺图降级、远程下载上传和缓存复用；progress Registry 3 项测试全部通过，确认本地相对路径按 work dir 上传后再更新卡片，同一文件复用上传结果。隔离 Bifrost + mock Runner + loopback Feishu OpenAPI 黑盒 E2E 通过：`terminal-e2e-chart.png` 只调用一次 `/im/v1/images`，运行中过程更新、原任务卡最终结论和独立成功终态卡都包含 `![E2E chart](img_v3_terminal_e2e)`；6 条成功/失败相关消息均无 `msg_type=image`，`.txt` 与 `.tar.gz` 仍各自上传并发送 `msg_type=file`。测试 trap 已清理所属临时目录和进程。当前环境的 `FEISHU_APP_ID`、`FEISHU_APP_SECRET` 已设置，但 `FEISHU_OWNER_OPEN_ID` 缺失，因此未向真实租户投递卡片，也未把 mock payload 验证表述为真实客户端肉眼渲染。

- 2026-08-11：PASS（本地真实链路）— 更新 TC-FPC-11 后立即执行。4 个 focused Rust 用例全部通过，分别验证运行过程默认折叠、最新公开解释与当前轮工具三态计数、总体步骤/工具次数、Session ID 的“获取中/实时值/未提供”三态、standard/compact 卡片主题安全字段，以及本地/远程复合归档扩展名。`im_gateway_progress_card_budget_and_codex_resources` renderer E2E 通过，完整 CardKit JSON 小于 24KB，`agent_process_sum` 位于默认折叠的 `agent_process_panel` 前方，面板使用 `background_color=default` 与 `text_color=default`。隔离 Bifrost + mock Runner + loopback Feishu OpenAPI 黑盒 E2E 通过：成功/失败终态卡按预期更新，`.txt` 与 `.tar.gz` 两份显式链接均真实调用 `/im/v1/files` multipart 上传并各发送一条 `msg_type=file` 消息，所有进度卡 payload 不含固定 `grey`、黑白或 RGB/RGBA 样式。测试临时目录和所属进程已由 trap 清理。当前环境缺少 `FEISHU_OWNER_OPEN_ID`，未向真实租户投递测试卡，也未伪造亮/暗主题客户端截图；暗色兼容结论以飞书 CardKit 官方主题语义字段和完整 HTTP payload 契约为证据。

- 2026-08-10：PASS — 更新 TC-FPC-10 后立即执行。5 个 focused Rust/HTTP 回归全部通过，覆盖成功、失败、终态通知发送失败、完整四板块终态折叠和 standard/compact 失败结论折叠。随后构建当前 debug `bifrost` 并执行隔离 Service + mock external runner + loopback Feishu OpenAPI 黑盒场景；第一次执行发现测试夹具漏导入 `pathlib`，第二次捕获到真实文件消息后发现路径断言未剥离 query，两处均修复测试而未削弱产品断言，第三次完整 PASS。最终成功/失败原任务卡的状态摘要与“最终结论/失败结论”均为 `collapsible_panel` 且 `expanded=false`；独立成功/失败终态卡继续保留 16-locale 标题与完整正文，引用链分别指向对应 progress message；成功总结引用的本地报告实际调用 `/im/v1/files` multipart 上传并以 `msg_type=file` 发送。测试临时目录和所属进程已由 trap 清理。

- 2026-08-09：PASS — 新增 TC-FPC-10 后立即逐条执行。成功、失败、终态通知发送失败 3 个 focused HTTP 集成测试全部通过：原 CardKit 分别收敛为 Finished/Failed，成功终态卡保留绿色 header、默认 `Task completed` 与 16-locale `i18n_content`（含 `zh_cn=任务执行结束`、`ja_jp=タスク実行完了`）并携带 `FINAL_SUMMARY_MARKER`，失败终态卡保留红色 header、多语言失败标题与真实错误原因；mock 请求路径确认终态卡通过 `/im/v1/messages/om_1/reply` 直接引用任务进度卡。随后使用当前 debug 二进制执行 `test_feishu_progress_terminal_notification.sh`，真实启动隔离 Bifrost、mock external runner 与 loopback Feishu OpenAPI；成功任务和退出码 17 异常任务共生成 4 条消息，引用链分别为 `terminal-success → om_1`、`terminal-failure → om_3`，成功正文包含最后一个 `AssistantFinal` 的 `E2E_FINAL_SUMMARY_SUCCESS`，失败正文包含 `E2E_PERMISSION_DENIED`，整卡 update 同时保留对应终态。通知发送被 mock 拒绝时原卡仍为 Finished，outbound log 为 Failed，未回滚任务状态。测试沙箱与所属进程已由 trap 清理。

- 2026-08-07：PASS — 新增 TC-FPC-09 后立即逐条执行。`codex_like_progress_metadata_captures_target_runner_session_id_immediately`、`external_runner_status_footer_uses_runner_metadata_instead_of_agent_metrics`、`external_runner_footer_hides_machine_status_line` 三项 focused 单测全部通过，覆盖 Codex `thread_id`、Trae `threadId`、Claude Code `session_id` 的运行中 metadata 合并，以及 Session ID 存在/缺失两种 Runner 行渲染。第一轮 Review/Fix/Test 发现异常 Session ID 可能过长或含 backtick，并补充 `external_runner_footer_bounds_and_escapes_session_id` 与 `external_cli_progress_session_id_flows_from_live_event_to_feishu_card`；首次链路测试因引用私有测试常量编译失败，改用公开协议值 `codex` 后两项均通过，完整验证 live event → metadata → runner summary → CardKit payload。`im_gateway_progress_card_budget_and_codex_resources` E2E 1 项通过，完整 CardKit JSON 在 24KB 预算内包含 `Runner：codex · Adapter：codex · Session ID：thread-resource-e2e`，并保留 Token、周额度、耗时和执行过程。当前未向真实飞书租户发送测试卡片；本轮真实场景证据来自当前源码生成的完整 CardKit payload。
- 2026-08-07：PASS — 第二轮 Review/Fix/Test 基于第一轮最新 diff 重新执行 `git status --short`、本需求 `git diff --check` 和 staged 检查；确认 9 个本需求文件与当前分支既有 breakpoint 改动边界清晰、暂存区为空。随后 `progress_card` 69 项、既有 `codex_progress_metadata` 2 项、live-event 链路 1 项和 CardKit E2E 1 项全部通过；未发现新的功能、布局、预算、文档或测试缺口，无需第三轮。

- 2026-07-23：PASS — 更新 TC-FPC-08 后立即逐条执行，并在 Review/Fix/Test 修复纯状态/思考退化边界及 CI 变更行覆盖率缺口后再次执行。`SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin --lib progress_card -- --nocapture` 共 64 项全部通过，覆盖旧工具步骤摘要的完成/失败/执行中三态、最近 5 次工具可展开详情、旧思考/状态与对应工具按完整执行段裁剪、无工具时先删状态并保留最近 5 轮思考、30 工具窗口、UTF-8 字节/组件预算和限制错误降级；`im_gateway_progress_card_budget_and_codex_resources` E2E 1 项通过，最终 CardKit JSON 小于 24KB，至少保留一条无参数/输出详情的旧工具步骤及其对应思考，首个保留段之前不残留孤立思考或工具，最近 5 次工具输入/输出详情完整。专项 `coverage-diff.py` 验证变更生产 Rust 行覆盖率为 100%（71/71），通过 95% 门禁。当前未使用用户另一台设备的运行数据，也未向真实飞书租户发送测试卡片；本轮真实场景证据来自当前源码生成的完整 CardKit payload。

- 2026-07-14：PASS — 更新 TC-FPC-08 后立即逐条执行。`cargo test -p bifrost-admin --lib progress_card` 共 59 项全部通过，覆盖工具输入/输出 300 字符截断、30 工具窗口不连带删除前置思考、16KB 压力下优先删除旧工具并保留最近 5 轮思考、`200860`/`300305` 同卡收缩和精简降级；独立 `im_gateway_progress_card_budget_and_codex_resources` E2E 通过，真实 CardKit JSON 小于 24KB，包含 `THINKING_ROUND_35..39` 与最新工具 marker，不包含最旧思考或最新工具 300 字符之后的尾部 marker。

- 2026-07-13：PASS — 按 TC-FPC-07/08 逐条执行。真实 `codex-cli 0.144.1` app-server schema 包含 `account/rateLimits/read`、`account/rateLimits/updated`、`thread/tokenUsage/updated`；当前登录态只读快照返回 `limitId=codex`、`usedPercent=64`、`windowDurationMins=10080`、`resetsAt=1784490086`，输出未包含账号或凭据。资源解析 2 项、顶部状态 renderer 1 项、progress card 58 项单测全部通过；长卡片 E2E 1 项通过，确认最终 JSON 小于 24KB，顶部显示 session Token、周余额和耗时，旧过程被省略且最新 marker 保留。mock CardKit 覆盖 `200860`、`300305`、同卡收缩重试、精简模式、精简限额后的最终 rollover，以及精简普通错误不创建重复消息。当前环境未向真实飞书租户故意发送超限 payload，平台错误恢复由本地 mock API 验证。
- 2026-07-13：PASS — 按 TC-FPC-05 依次执行 4 组命令。文件行数边界单测、真实 Codex app-server `fileChange` 解析单测、既有卡片回归单测和独立 renderer E2E 全部通过；payload 包含文件路径及修改/新增/删除行数，并确认不存在“暂无工具详情”。CardKit payload 沿用标准 `grey` 背景和 Markdown，无硬编码主题颜色；当前环境未注入飞书测试凭据，未执行线上亮暗主题截图验证。
- 2026-07-13：PASS — 优化后的 TC-FPC-05 四组命令全部通过：`file_change_` 9 项单测、app-server 映射 1 项单测、进度卡回归 1 项单测、独立 renderer E2E 1 项用例。真实 payload 已验证纯正文新增/删除行数（含正文自身以 `+` / `-` 开头的边界）、工作区相对路径、工作区外路径保留、逐行缩进、“文件变更”标题和“已执行 N 个步骤”文案，且不显示“暂无工具详情”或工作区绝对路径前缀。
