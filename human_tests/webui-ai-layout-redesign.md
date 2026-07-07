# WebUI AI Layout Redesign 真实场景测试

## 功能模块说明

验证 WebUI AI 页面改造后的真实用户路径：进入 `/ai` 默认展示新建对话输入态，左侧提供 `New Chat`、`ASR`、`Videos`、`IM`、线程列表和底部 Settings，右侧根据入口展示新对话、历史对话、ASR 工作台、Videos Tool、IM 工作台或 Settings 二级内容页。Runner 选择必须位于新对话输入面板底部工具栏的“高级/Runner”位置，默认使用 Codex Runner，并能切换到后端已启用的其它 runner。Settings 只能承载原 AI 左侧菜单中的配置项，顶部只合并为 `Agent`、`Runner`、`IM` 三个 tab，配置项在各自 tab 内以卡片方式向下平铺；对话状态、`Back`、`Session Detail`、`Messages` 等会话级信息必须留在具体对话页的头部操作或弹窗中。

## 前置条件

- 当前工作目录为 Bifrost 仓库根目录。
- 不使用正式 `9900` 端口。
- 使用最新源码启动 WebUI；推荐通过 Playwright UI 测试 global setup 或临时 Bifrost 后端启动。
- 测试数据目录必须是临时目录，不能复用用户真实 `~/.bifrost` 数据。
- 后端 `/api/im-gateway/chat/config` 至少提供以下 enabled runner 中的若干项：Codex、Bifrost Agent、Claude Code、Trae X。若 Codex 不可用，测试必须记录实际 fallback runner。
- 若 ASR capability 在当前平台不可用，ASR 入口隐藏或展示能力不可用空态均可接受，但必须与 capability API 返回一致。

## 测试用例列表

### TC-AILR-01 默认进入 AI 展示新建对话输入态

操作步骤：

1. 打开 WebUI AI 页面：`http://127.0.0.1:<web_port>/_bifrost/ai`。
2. 查看左侧顶部工作入口。
3. 查看左侧中部线程列表。
4. 查看右侧主内容区域。

预期结果：

- 左侧 `New Chat` 入口处于选中态。
- 左侧栏为轻量灰底侧栏，桌面宽度约 216px；顶部入口无重边框卡片感，线程标题有足够横向展示空间。
- 左侧中部展示线程列表；如果没有历史线程，展示空列表文案。
- 线程列表行使用单行紧凑样式，选中态为浅灰背景，不能显示大号 runner 方块或双行详情卡片；点击选中前后行高保持一致，列表不能抖动。
- 没有历史 thread 被选中。
- 右侧不是 Agent General、Model 或 IM Gateway 配置页。
- 右侧展示新建对话面板，主区域无卡片外框，输入框位于主内容区域中部或视觉中心附近。
- 新建态输入面板接近截图参考的中间输入体验，问候文案位于输入面板上方，文本输入区和底部工具栏上下分层，内部控件对齐稳定。
- 输入面板底部工具栏左侧展示新增/附加入口与 Runner 下拉，Runner 位置对应截图中的“高级”区域；右侧展示语音/发送等即时操作。
- Runner 下拉不能独立漂在输入面板外，也不能挤压 textarea 或发送按钮。
- 页面无水平滚动条，主要按钮和输入框没有重叠。

### TC-AILR-02 Runner 默认 Codex 并支持切换

操作步骤：

1. 在 `/ai` 默认新建对话态查看 Runner 下拉当前值。
2. 打开 Runner 下拉。
3. 查看可选项。
4. 选择 `Bifrost Agent`。
5. 再次打开下拉并选择 `Claude Code` 或 `Trae X`，取决于当前后端 enabled runner。

预期结果：

- Codex Runner 可用时，默认值为 `Codex Runner`。
- Codex Runner 不可用时，默认值显示真实 fallback runner，不能显示 Codex 但实际使用其它 runner。
- 下拉列表只包含 enabled runner，或 disabled runner 置灰且不可提交。
- 可用 runner 至少按产品排序展示：Codex Runner、Bifrost Agent、Claude Code、Trae X、ChatGPT Web、自定义 runner。
- 切换 runner 后，当前新建对话输入态保持不变，输入内容不丢失。

### TC-AILR-03 首条消息创建新线程

操作步骤：

1. 打开 `/ai` 默认新建对话态。
2. 在 Runner 下拉选择 `Codex Runner`；若不可用，选择当前默认 runner。
3. 在输入框输入 `Summarize current workspace status`。
4. 点击 Send 按钮或使用提交快捷键。
5. 观察右侧对话区域、URL 和左侧线程列表。

预期结果：

- 发送前不会创建 session 或选中历史 thread。
- 发送后右侧切换为普通 Chat conversation。
- 用户输入作为第一条 user message 出现在对话中。
- URL 从 `/ai` 或 `mode=new` 更新为包含具体 `session` 或等价会话标识的 Chat URL。
- 左侧线程列表出现新线程或刷新出新线程。
- 新线程处于选中态。
- 请求使用用户发送时选中的 runner。

### TC-AILR-04 点击历史线程退出新建态

操作步骤：

1. 准备至少两条历史 Agent thread。
2. 打开 `/ai`。
3. 在左侧线程列表点击第一条历史 thread。
4. 查看右侧对话内容和 URL。
5. 点击左侧顶部 `New Chat`。

预期结果：

- 点击历史 thread 后，`New Chat` 不再选中。
- 被点击 thread 在左侧处于选中态。
- 右侧展示该 thread 的历史消息或加载状态。
- 对话右上角不能再显示内部 `New Chat` 按钮；新建对话只通过左侧顶部 `New Chat` 入口触发。
- 右侧对话区域填满 AI Shell 主内容区，不保留旧内部 thread rail 空列；composer 宽度应接近右侧主内容宽度。
- URL 包含对应 `session` 或 `historyPath`，且不再包含 `mode=new`。
- 再次点击 `New Chat` 后，回到新建对话输入态。
- 正在运行的历史 thread 不会因为点击 `New Chat` 被停止或删除。

### TC-AILR-05 ASR 工作入口

操作步骤：

1. 打开 `/ai`。
2. 点击左侧顶部 `ASR`。
3. 如 ASR 可用，切换 ASR 内部 tab，例如 `ASR Management` 或 scheduled tasks。
4. 刷新当前页面。

预期结果：

- ASR capability 可用时，左侧展示 `ASR` 入口。
- 点击 `ASR` 后，右侧渲染 ASR 工作台，不打开 Settings 二级内容页。
- URL 包含 `view=asr` 或等价状态。
- ASR 内部 `asrTab`、`asrTask`、`asrTaskTab` 等参数继续生效。
- 刷新后仍停留在 ASR 工作台对应位置。
- ASR capability 不可用时，`ASR` 入口隐藏或右侧展示能力不可用空态，且行为与 capability API 一致。

### TC-AILR-06 IM 工作入口

操作步骤：

1. 打开 `/ai`。
2. 点击左侧顶部 `IM`。
3. 在右侧 IM 工作台或 IM Gateway 页面切换到 Routes 或 Connections。
4. 刷新当前页面。

预期结果：

- 点击 `IM` 后，右侧展示 IM 工作入口或 IM Gateway 工作台，不打开 Settings 二级内容页。
- URL 包含 `view=im` 或等价状态。
- `imGatewaySection=connections|targets|routes|schedules|history` 深链继续生效。
- IM Connections 以响应式卡片网格展示 Provider，不再是过窄单列；桌面下至少可以多列排布，窄屏下收敛为单列。
- IM 内容区宽度不超过约 1120px，并在 AI 右侧主内容区水平居中。
- IM 内容顶部与 ASR、Videos、历史消息线程、Settings 保持一致留白，不能贴住 AI 右侧顶部。
- 刷新后仍停留在对应 IM section。
- 左侧线程列表仍保留，不被 IM 内容挤出或覆盖。

### TC-AILR-07 Settings 二级内容页承载配置项

操作步骤：

1. 打开 `/ai` 默认新建对话态。
2. 点击左侧底部 Settings 按钮。
3. 查看 Settings 顶部配置 tabs。
4. 在 Settings 内容页中查看 Agent 分组下的配置卡片。
5. 在 Settings 内容页中切换到 Runner 分组。
6. 在 Settings 内容页中切换到 IM 分组。
7. 检查 IM 分组下 Connections、Targets、Routes 等配置卡片。
8. 从 `/_bifrost/ai?view=chat&session=history-thread-1` 进入历史对话后再次点击 Settings。
9. 点击左侧 `New Chat` 或其它主入口离开 Settings 二级内容页。

预期结果：

- Settings 作为右侧主内容打开，替换右侧主内容。
- Settings 顶部 tabs 只有 `Agent`、`Runner`、`IM` 三个入口，不能继续平铺 General、Model、Runtime、Runners、IM Connections、IM Routes 等细分 tab。
- `Agent` tab 下以卡片方式纵向平铺 General、Model、Runtime、History、Memories、Skills、Memory Records、MCP Servers、Sessions 等 Agent 配置。
- `Runner` tab 下以卡片方式展示 Runners 配置。
- `IM` tab 下以卡片方式纵向平铺 Connections、Targets、Routes、Schedules、History 等 IM Gateway 配置。
- `IM` tab 下 Connections 使用与 IM 工作入口一致的响应式卡片网格展示 Provider。
- Settings 内容轨道不应撑满整个右侧主内容区；桌面宽屏下宽度上限约 1120px，并在右侧主内容区水平居中。
- Settings 顶部留白与 ASR、IM、Videos、历史消息线程一致，不能贴住 AI 右侧顶部。
- Settings 顶部不能显示 Chat tab。
- Settings 不能显示 `Back` 按钮、`Session Detail`、`Messages` 等会话详情内容。
- 从历史对话进入 Settings 后，URL 中的 `session`、`historyPath`、`mode` 会话状态被清理，不会污染 Settings 内容。
- Settings 内容区只渲染当前激活分组；切到 `Runner` 后页面上不能继续出现 Agent General；切到 `IM` 后页面上不能继续出现 Agent General 或 Runners。
- 切回其它主入口后，右侧展示对应主内容。
- URL 使用 `view=settings&settings=agent|im` 表达 Settings 二级内容页，Runner 分组用 `settings=agent&agentSection=runners` 兼容既有路由；切回其它主入口后删除 `settings` 参数。

### TC-AILR-08 旧 AI 深链兼容

操作步骤：

1. 打开旧 Chat 链接：`/_bifrost/ai?aiSection=agent-chat&agentSection=chat`。
2. 打开旧 Agent Model 链接：`/_bifrost/ai?aiSection=agent-model&agentSection=model`。
3. 打开旧 ASR 链接：`/_bifrost/ai?aiSection=tools-asr`。
4. 打开旧 IM Routes 链接：`/_bifrost/ai?aiSection=im-gateway-routes&imGatewaySection=routes`。

预期结果：

- 旧 Chat 链接进入新 AI Shell 的 Chat 区域。
- 旧 Agent Model 链接打开 Settings 二级内容页的 Agent 分组，并展示包含 Model 的 Agent 配置卡片列表。
- `settings=agent&agentSection=chat` 链接归一化到 Agent General，不能在 Settings 中打开 Chat 或 Session Detail。
- 旧 ASR 链接进入 ASR 工作台。
- 旧 IM Routes 链接进入 IM 工作台或打开 Settings 中的 IM Routes；具体行为必须与设计文档最终选择一致。
- 所有旧链接都不能显示空白页、无限跳转或丢失主布局。

### TC-AILR-09 窄屏布局与可访问性

操作步骤：

1. 用桌面宽度打开 `/ai`，记录左侧栏、输入框、线程列表和 Settings 按钮布局。
2. 切换到 tablet 宽度，例如 `768x900`。
3. 切换到 mobile 宽度，例如 `390x844`。
4. 分别尝试 New Chat、Runner 下拉、线程点击、ASR、IM、Settings。

预期结果：

- 桌面宽度下左侧栏固定，右侧内容填满剩余空间。
- tablet/mobile 下没有文本互相遮挡，没有按钮文字溢出容器。
- Runner 下拉可以打开并选择。
- Settings 二级内容页可滚动，可切回其它主入口，焦点不丢失。
- 线程列表不会把输入框挤到不可见区域。
- 页面没有非预期横向滚动。

### TC-AILR-10 Settings 不阻塞左侧主入口切换

操作步骤：

1. 打开 Settings 深链：`/_bifrost/ai?view=settings&settings=agent&agentSection=model`。
2. 点击左侧 `ASR`。
3. 再次点击左侧 Settings。
4. 点击左侧 `IM`。
5. 再次点击左侧 Settings。
6. 点击左侧 `Videos`。
7. 再次点击左侧 Settings。
8. 点击左侧 `New Chat`。

预期结果：

- 从 Settings 点击 `ASR` 后，URL 切到 `view=asr`，删除 `settings` 参数，右侧显示 ASR 工作台。
- 从 Settings 点击 `IM` 后，URL 切到 `view=im`，删除 `settings` 参数，右侧显示 IM 工作台。
- 从 Settings 点击 `Videos` 后，URL 切到 `view=videos`，删除 `settings` 参数，右侧显示 Videos Tool。
- 从 Settings 点击 `New Chat` 后，URL 切到 `view=chat&mode=new`，删除 `settings` 参数，右侧显示新建对话输入态。
- 任一主入口点击后都不能继续显示 `ai-settings-content`，也不能被旧的 `agentSection` / `imGatewaySection` 参数拉回 Settings。

### TC-AILR-11 AI 子页面顶部留白与内容宽度一致

操作步骤：

1. 打开 `/ai?view=chat&session=history-thread-1`。
2. 记录历史消息线程内容区顶部与 AI 右侧主内容区顶部的距离。
3. 点击左侧 `ASR`，记录 ASR 内容轨道顶部距离与宽度。
4. 点击左侧 `IM`，记录 IM 内容轨道顶部距离、宽度和 Provider 卡片网格。
5. 点击左侧 `Videos`，记录 Videos 内容轨道顶部距离与宽度。
6. 点击左侧 `Settings`，分别切换 `Agent`、`Runner`、`IM`，记录内容轨道顶部距离与宽度。

预期结果：

- 历史消息线程顶部不贴住 AI 右侧顶部，conversation header 从统一留白后开始。
- ASR、IM、Videos、Settings 内容轨道顶部留白一致，桌面下约 24px。
- ASR、IM、Videos、Settings 内容轨道最大宽度约 1120px，并在右侧主内容区水平居中。
- IM 工作入口和 Settings 的 IM Connections Provider 区域使用 CSS grid 布局；桌面宽度下多个 Provider 可以并排展示，窄屏下自动变为单列。
- Videos 工具不再额外叠加一圈导致顶部或左右间距明显大于其它页面。

### TC-AILR-12 运行中队列消息紧凑展示

操作步骤：

1. 打开 `/ai?aiSection=agent-chat&agentSection=chat`。
2. 启动一个未结束的 Agent Chat 任务。
3. 在任务运行中切换到 Queue 模式，连续提交 4 条 follow-up 消息，其中至少一条为长文本。
4. 查看输入框上方的 Queued 区域。
5. 点击第一条排队消息右侧删除按钮。

预期结果：

- Queued 区域最多占用两条排队消息的高度；超过两条时只在该区域内部纵向滚动，不继续向上挤压消息列表。
- 每条排队消息单行展示，长文本以省略号截断。
- 每条排队消息右侧始终预留操作区，删除按钮不能换行，也不能被长文本挤出可视区域。
- 删除第一条后，该条从队列中移除，其余排队消息继续在同一紧凑滚动区域展示。

## 清理步骤

- 关闭 Playwright 浏览器。
- 停止临时 WebUI / Bifrost 后端进程。
- 删除测试使用的临时数据目录。
- 若测试中创建了真实 Agent session、IM provider 或 ASR task，按对应 UI 或 API 删除。

## 执行记录

- 2026-07-07：创建本设计验证用例。
- 2026-07-07：实现后使用 Chromium + Playwright 执行真实浏览器验证，命令为 `WEB_PORT=4177 pnpm --dir web exec playwright test tests/ui/ai-layout-redesign.spec.ts --config=playwright.frontend.config.ts --reporter=line`，结果 `4 passed (11.4s)`。测试使用 Playwright frontend dev server 和 mock Admin API，不复用正式 `9900` 端口，不写入用户真实 `~/.bifrost` 数据。
- 2026-07-07：根据用户截图补充轻量灰底左栏、紧凑线程列表、居中问候和横向胶囊输入框验收点；根据用户纠偏将截图式输入框限制在 AI Shell 默认新建面板，保持原 Agent Chat 会话布局；Settings 改为右侧二级内容页，Videos 保留为左侧主入口。补充 session 深链回归，确保已有对话直接打开普通消息区与 composer，不展示新建落地页。复测命令 `WEB_PORT=4177 pnpm --dir web exec playwright test tests/ui/ai-layout-redesign.spec.ts --config=playwright.frontend.config.ts --reporter=line`，结果 `5 passed (11.4s)`。
- 2026-07-08：根据用户反馈修正新建 Chat 中间区域位置与输入面板内部布局，将 Runner 从面板外下方移动到输入面板底部工具栏的“高级/Runner”位置，文本输入区独立占上半部分，语音/发送按钮固定右侧并与 Runner 工具栏同基线。补充 Playwright 几何断言验证输入区在工具栏上方、Runner 与发送按钮位于同一工具栏内。
- 2026-07-08：根据用户反馈修正 Settings 信息架构，恢复原 AI 左侧配置项的顶部 tabs，包括 Agent General、Model、Runtime、History、Memories、Skills、Runners、Memory Records、MCP Servers、Sessions，以及 IM Gateway Connections、Targets、Routes、Schedules、History；排除 Chat 和会话详情。修复 Settings 重复挂载所有 Agent 配置面板导致的重复 DOM 与布局混乱，改为顶部 tabs 导航 + 单一当前配置面板，并补充直接打开 Settings 脏链接时清理 `session` / `agentSection=chat` 的回归。复测命令 `WEB_PORT=4177 pnpm --dir web exec playwright test tests/ui/ai-layout-redesign.spec.ts --config=playwright.frontend.config.ts --reporter=line`，结果 `6 passed (14.8s)`。
- 2026-07-08：根据用户反馈修正右侧对话区域未使用空白、左侧线程列表偏窄和线程选中后列表抖动。桌面 AI 左栏调整为约 216px；compact thread item 固定 36px 高，选中态不再改变字体权重导致抖动；embedded Chat 移除旧内部 thread rail 空列并放宽 message/composer track。复测命令 `WEB_PORT=4177 pnpm --dir web exec playwright test tests/ui/ai-layout-redesign.spec.ts --config=playwright.frontend.config.ts --reporter=line`，结果 `6 passed (15.4s)`。
- 2026-07-08：根据用户反馈把 Settings 顶部菜单再次收敛为 `Agent`、`Runner`、`IM` 三个 tab，按配置对象归类；Agent 分组平铺 General、Model、Runtime、History、Memories、Skills、Memory Records、MCP Servers、Sessions，Runner 分组展示 Runners，IM 分组平铺 Connections、Targets、Routes、Schedules、History。复测命令 `WEB_PORT=4177 pnpm --dir web exec playwright test tests/ui/ai-layout-redesign.spec.ts --config=playwright.frontend.config.ts --reporter=line`，结果 `6 passed (17.4s)`。
- 2026-07-08：根据用户反馈修复 Settings 页面点击左侧其它主入口无法切回的问题。主入口点击改为覆盖式路由，显式 `view=asr|im|videos|chat` 不再被残留 `settings` 参数抢回 Settings；补充 Settings -> ASR / IM / Videos / New Chat / thread 浏览器回归。复测命令 `WEB_PORT=4177 pnpm --dir web exec playwright test tests/ui/ai-layout-redesign.spec.ts --config=playwright.frontend.config.ts --reporter=line`，结果 `7 passed (19.6s)`。
- 2026-07-08：根据用户反馈删除已有对话右上角内部 `New Chat` 按钮，避免和左侧主入口重复；保留 `Status` 会话状态入口，并补充已有对话页面不显示 `agent-chat-new` 的回归断言。复测命令 `WEB_PORT=4177 pnpm --dir web exec playwright test tests/ui/ai-layout-redesign.spec.ts --config=playwright.frontend.config.ts --reporter=line`，结果 `7 passed (21.0s)`。
- 2026-07-08：根据用户反馈限制 Settings 内容宽度，新增居中的 `ai-settings-track`，宽度上限与嵌入式对话 message/composer track 对齐为约 1120px，避免配置卡片撑满右侧主内容区。复测命令 `WEB_PORT=4177 pnpm --dir web exec playwright test tests/ui/ai-layout-redesign.spec.ts --config=playwright.frontend.config.ts --reporter=line`，结果 `7 passed (19.5s)`。
- 2026-07-08：根据用户反馈统一 AI 子页面顶部留白和内容宽度，ASR、IM、Videos、Settings 都通过同一个右侧内容轨道展示，桌面顶部留白约 `24px`、最大宽度约 `1120px` 并居中；历史消息线程嵌入态也从统一顶部留白后开始。IM 工作入口和 Settings IM Connections 改为响应式 Provider 卡片网格，Videos 去掉嵌入 AI Shell 时自身额外 padding。首次复测发现 embedded Chat 左右 padding 压缩 composer 宽度，已修复为只保留顶部 padding；复测命令 `WEB_PORT=4177 pnpm --dir web exec playwright test tests/ui/ai-layout-redesign.spec.ts --config=playwright.frontend.config.ts --reporter=line`，结果 `7 passed (19.5s)`。
- 2026-07-08：根据用户反馈压缩运行中对话的 Queued 区域。输入框上方队列列表最多显示两条消息高度，更多消息在列表内部滚动；队列行改为文本列 + 固定操作列，长文本单行省略，删除按钮不换行。补充 Playwright 几何断言验证 4 条队列消息时 `agent-chat-queue-list` 内部滚动、删除按钮位于第一行右侧且不换行。

执行明细：

| 用例 | 实际结果 |
| --- | --- |
| TC-AILR-01 | 通过。打开 `/ai` 后左侧 `New Chat` 为 `aria-current=true`，线程列表可见且没有选中线程，右侧 AI Shell 显示 `How can Bifrost help?` 与居中输入面板；Playwright 断言侧栏宽度在 `210-230px`、输入面板高度在 `110-140px`、圆角大于等于 `16px`，并验证 textarea 位于底部工具栏上方，Runner 与发送按钮都在同一工具栏内，未进入 Agent General/Model 配置页。 |
| TC-AILR-02 | 通过。mock 后端提供 `codex_runner`、`claude_runner`、`traex_runner`，默认显示 `Codex Runner`；打开下拉可见 `Bifrost Agent`、`Claude Code`，切换到 `Claude Code` 后输入内容保持可发送。 |
| TC-AILR-03 | 通过。在新建态输入 `Summarize current workspace status` 并发送，请求命中 `/api/im-gateway/chat/stream`，body 中 `runnerId=claude_runner`；NDJSON `run_finished` 返回后右侧显示 `Summary complete`，URL 更新为包含 `session=admin-chat-...` 且不再包含 `mode=new`。 |
| TC-AILR-04 | 通过。左侧点击 `Existing thread` 后 `New Chat` 取消选中，右侧显示历史消息 `Existing answer`；右上角保留 `Status`，不再显示内部 `agent-chat-new` 按钮；Playwright 断言线程行选中前后高度一致，composer 宽度充分使用右侧主内容区，不再保留旧内部 thread rail 空列；再次点击左侧 `New Chat` 回到新建对话输入态。 |
| TC-AILR-05 | 通过。ASR capability mock 为可用，左侧展示 `ASR`；点击后 URL 包含 `view=asr`，右侧渲染 ASR 工作台，不打开 Settings 二级内容页。 |
| TC-AILR-06 | 通过。点击 `IM` 后 URL 包含 `view=im`，右侧渲染 IM Gateway 工作台；旧 `imGatewaySection=routes` 深链可直接显示 Routes section；Connections Provider 使用 CSS grid 响应式卡片网格，mock 的 `feishu-main` 与 `weixin-main` Provider 卡片可见；内容轨道最大宽度不超过约 1120px、水平居中且顶部留白与 ASR 一致。 |
| TC-AILR-07 | 通过。点击底部 Settings 后 URL 包含 `view=settings`，右侧显示 `ai-settings-content`；顶部配置 tabs 只有 `Agent`、`Runner`、`IM` 三个入口。默认 Agent 分组可见并平铺 General、Model、Runtime、MCP Servers 等配置卡片；Settings 内容轨道宽度不超过约 1120px、在右侧主内容区居中且顶部留白一致；Runner 分组仅展示 Runners 配置卡片且不再显示 Agent General；IM 分组平铺 Connections、Targets、Routes 等配置卡片，Connections Provider 使用响应式卡片网格。Chat tab 不存在，`Back`、`Session Detail`、`Messages` 不存在。从历史对话点击 Settings 后 URL 清理 `session=history-thread-1`；切换 Runner 后 URL 包含 `settings=agent&agentSection=runners`，切换 IM 后 URL 包含 `settings=im&imGatewaySection=connections`。 |
| TC-AILR-08 | 通过。旧 `agent-chat`、`agent-model`、`tools-asr`、`im-gateway-routes` 链接均进入新 AI Shell 对应视图或 Settings 二级内容页，无空白页和无限跳转；旧 Agent Model 链接进入 Agent 分组并展示 Model 配置卡片；`settings=agent&agentSection=chat&session=...` 被归一化为 Settings Agent General，不展示 Chat 或会话详情。 |
| TC-AILR-09 | 通过。分别在 `768x900` 与 `390x844` viewport 打开 `/ai`，New Chat、Runner 下拉和 Settings 二级内容页均可操作；`document.documentElement.scrollWidth <= window.innerWidth + 1`，未发现非预期横向滚动。 |
| TC-AILR-10 | 通过。从 `view=settings&settings=agent&agentSection=model` 进入 Settings 后，依次点击左侧 ASR、IM、Videos、New Chat 和历史线程，Playwright 断言 URL 分别切到 `view=asr`、`view=im`、`view=videos`、`view=chat&mode=new`、`view=chat&session=history-thread-1`，均删除 `settings` 参数，不再显示 `ai-settings-content`，并展示对应主内容。 |
| TC-AILR-11 | 通过。Playwright 断言 ASR、IM、Videos、Settings 的内容轨道顶部距离为约 `24px`，宽度不超过约 `1120px` 并在 AI 右侧主内容区内水平居中；历史消息线程顶部不吸顶，Chat header 和消息区从统一留白后开始；IM 工作入口和 Settings IM Connections 均使用 `settings-im-card-grid` CSS grid；Videos 嵌入 AI Shell 时不再叠加自身额外 padding。 |
| TC-AILR-12 | 通过。Playwright 在运行中对话里 mock 4 条队列消息，断言 `agent-chat-queue-list` 高度不超过两条消息空间、`scrollHeight > clientHeight` 且 `overflowY=auto`；第一条长消息右侧删除按钮位于同一队列行内并在文本之后，没有换行；删除第一条后按钮消失，其余队列消息仍保留在紧凑滚动区域。 |
