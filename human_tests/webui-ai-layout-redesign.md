# WebUI AI Layout Redesign 真实场景测试

## 功能模块说明

验证 WebUI AI 页面改造后的真实用户路径：进入 `/ai` 默认展示新建对话输入态，左侧提供 `New Chat`、`ASR`、`IM`、线程列表和底部 Settings，右侧根据入口展示新对话、历史对话、ASR 工作台、IM 工作台或 Settings 二级内容页。已下线的 Videos Tool 不再出现在导航中，旧 `view=videos` / `aiSection=tools-videos` 深链安全回退到 New Chat。Runner 选择必须位于新对话输入面板底部工具栏的“高级/Runner”位置，默认使用 Codex Runner，并能切换到后端已启用的其它 runner。Settings 只能承载原 AI 左侧菜单中的配置项，顶部只合并为 `Agent`、`Runner`、`IM` 三个 tab，配置项在各自 tab 内以卡片方式向下平铺；对话状态、`Back`、`Session Detail`、`Messages` 等会话级信息必须留在具体对话页的头部操作或弹窗中。

## 前置条件

- 当前工作目录为 Bifrost 仓库根目录。
- 不使用正式 `9900` 端口。
- 使用最新源码启动 WebUI；推荐通过 Playwright UI 测试 global setup 或临时 Bifrost 后端启动。
- 测试数据目录必须是临时目录，不能复用用户真实 `~/.bifrost` 数据。
- 后端 `/api/im-gateway/chat/config` 至少提供以下 enabled 外部 runner 中的若干项：Codex、Claude Code、Trae X。若 Codex 不可用，测试必须记录实际 fallback runner。
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
4. 选择 `Claude Code` 或 `Trae X`，取决于当前后端 enabled runner。

预期结果：

- Codex Runner 可用时，默认值为 `Codex Runner`。
- Codex Runner 不可用时，默认值显示真实 fallback runner，不能显示 Codex 但实际使用其它 runner。
- 下拉列表只包含 enabled runner，或 disabled runner 置灰且不可提交。
- 可用 runner 至少按产品排序展示：Codex Runner、Claude Code、Trae X、ChatGPT Web、自定义 runner。
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
- IM 内容区使用比 Settings 更窄的工作台阅读轨道，桌面宽屏下宽度上限约 920px，并在 AI 右侧主内容区水平居中，不能铺满右侧区域。
- IM 内容顶部与 ASR、历史消息线程、Settings 保持一致留白，不能贴住 AI 右侧顶部。
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
7. 检查 IM 分组下 Targets、Routes、Schedules、History 等配置卡片，并确认 Connections 不在 Settings 中重复出现。
8. 从 `/_bifrost/ai?view=chat&session=history-thread-1` 进入历史对话后再次点击 Settings。
9. 点击左侧 `New Chat` 或其它主入口离开 Settings 二级内容页。

预期结果：

- Settings 作为右侧主内容打开，替换右侧主内容。
- Settings 顶部 tabs 只有 `Agent`、`Runner`、`IM` 三个入口，不能继续平铺 General、Model、Runtime、Runners、IM Connections、IM Routes 等细分 tab。
- `Agent` tab 下以卡片方式纵向平铺 General、Model、Runtime、History、Memories、Skills、Memory Records、MCP Servers、Sessions 等 Agent 配置。
- `Runner` tab 下以卡片方式展示 Runners 配置。
- `IM` tab 下以卡片方式纵向平铺 Targets、Routes、Schedules、History 等 IM Gateway 配置。
- `IM` tab 下不展示 Connections Provider 卡片网格；Provider 连接配置统一由左侧主入口 `IM` 工作台承载。
- Settings 内容轨道不应撑满整个右侧主内容区；桌面宽屏下宽度上限约 1120px，并在右侧主内容区水平居中。
- Settings 顶部留白与 ASR、IM、历史消息线程一致，不能贴住 AI 右侧顶部。
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
5. 打开已下线的 Videos 链接：`/_bifrost/ai?view=videos` 与 `/_bifrost/ai?aiSection=tools-videos`。

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
6. 点击左侧 `New Chat`。

预期结果：

- 从 Settings 点击 `ASR` 后，URL 切到 `view=asr`，删除 `settings` 参数，右侧显示 ASR 工作台。
- 从 Settings 点击 `IM` 后，URL 切到 `view=im`，删除 `settings` 参数，右侧显示 IM 工作台。
- 从 Settings 点击 `New Chat` 后，URL 切到 `view=chat&mode=new`，删除 `settings` 参数，右侧显示新建对话输入态。
- 任一主入口点击后都不能继续显示 `ai-settings-content`，也不能被旧的 `agentSection` / `imGatewaySection` 参数拉回 Settings。

### TC-AILR-11 AI 子页面顶部留白与内容宽度一致

操作步骤：

1. 打开 `/ai?view=chat&session=history-thread-1`。
2. 记录历史消息线程内容区顶部与 AI 右侧主内容区顶部的距离。
3. 点击左侧 `ASR`，记录 ASR 内容轨道顶部距离与宽度。
4. 点击左侧 `IM`，记录 IM 内容轨道顶部距离、宽度和 Provider 卡片网格。
5. 点击左侧 `Settings`，分别切换 `Agent`、`Runner`、`IM`，记录内容轨道顶部距离与宽度。

预期结果：

- 历史消息线程顶部不贴住 AI 右侧顶部，conversation header 从统一留白后开始。
- ASR、IM、Settings 内容轨道顶部留白一致，桌面下约 24px。
- ASR、IM 使用工作台阅读轨道，桌面宽屏下最大宽度约 920px；Settings 配置页保留较宽配置轨道，最大宽度约 1120px；这些轨道都必须在右侧主内容区水平居中。
- IM 工作入口的 Connections Provider 区域使用 CSS grid 布局；桌面宽度下多个 Provider 可以并排展示，窄屏下自动变为单列。Settings 的 IM 分组不再重复展示 Connections Provider 卡片。
- Videos 导航按钮和内容轨道均不存在。

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

### TC-AILR-13 历史线程打开后完整加载且实时追加不覆盖

操作步骤：

1. 准备一个带 `historyPath` 的 Agent Chat 历史线程，历史中至少包含三轮消息：最早一轮、中间一轮、最新一轮。
2. 打开深链：`/_bifrost/ai?view=chat&session=<session_key>&historyPath=<encoded_history_path>`。
3. 观察首次 history 请求参数。
4. 查看右侧消息列表。
5. 模拟或等待该线程收到一条 `timeline_changed` 实时事件，事件只包含新增过程或新增回答。
6. 再次查看右侧消息列表。

预期结果：

- 首次 history 请求不携带 `tail=true`、`limit`、`cursor` 或 `since` 参数。
- 最早一轮、中间一轮、最新一轮消息在首次打开后立即展示，不需要点击 `Load more`。
- 后端未返回真实分页时，`Load more` 按钮不出现。
- 收到实时事件后，新事件追加或合并到当前消息列表，旧的最早一轮和中间一轮消息仍然保留。
- 如果实时增量不连续，前端可以重新拉取完整 history 做恢复，但不能退回只显示最后一页。
- detail、timeline 和实时事件中相同 role + 内容的消息不会重复显示。

### TC-AILR-14 每轮执行过程按 Codex 风格展示

操作步骤：

1. 准备一个历史线程，其中一轮包含 user message、assistant delta 过程文本、tool call、tool result、final assistant message。
2. 打开该线程。
3. 查看该轮完成后的默认折叠状态。
4. 点击该轮的 `已处理` 折叠按钮。
5. 查看展开后的过程文本和命令组摘要。
6. 点击命令组摘要。
7. 查看命令详情。

预期结果：

- 默认折叠状态只展示 user message、处理耗时和最终 assistant answer；中间 assistant delta 和 tool 细节不直接铺满消息列表。
- 展开该轮后，过程文本按时间顺序显示，最终 assistant answer 仍然稳定显示在该轮最后。
- thinking/status 过程文本使用普通正文样式展示，长文本可展开更多。
- 相邻命令合并为一条轻量命令组摘要，例如 `已运行 1 条命令`，不会把每条命令默认散开成大块日志。
- 点击命令组后可以看到具体命令名、Input 和 Output。
- 再次点击单条命令可以折叠或展开该命令详情。

### TC-AILR-15 新建对话输入框图片粘贴回归

操作步骤：

1. 打开 WebUI AI 页面：`http://127.0.0.1:<web_port>/_bifrost/ai`。
2. 确认右侧显示新建对话输入面板。
3. 检查新建对话输入面板底部工具栏。
4. 在输入框输入 `Describe the pasted screenshot`。
5. 向该输入框粘贴一张 PNG 图片。
6. 切换到暗色主题并再次查看图片预览。
7. 点击发送按钮。
8. 查看发送后的普通对话页面、用户消息图片缩略图和请求 payload。

预期结果：

- 新建对话输入面板底部工具栏不再显示无功能的左侧加号按钮。
- 新建对话输入面板底部工具栏不再显示无功能的右侧语音输入按钮。
- 粘贴图片后，输入框下方展示与已有对话 composer 一致的图片预览条、缩略图、删除按钮和大小标签。
- 亮色和暗色主题下图片预览条、删除按钮和大小标签都清晰可见。
- 发送按钮在文本或图片至少一项存在时可点击；文本和图片都为空时保持禁用。
- 发送后用户消息在普通对话页面展示文本和图片缩略图。
- 已有对话 composer 的粘贴图片预览、最多 6 张上限、纯图片发送和外部 Runner 图片桥接能力保持不变。

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
- 2026-07-08：根据用户反馈收窄左侧 ASR、IM、Videos 三个主入口对应的右侧工作台内容区。三页从 Settings 的 1120px 配置轨道拆出为约 `920px` 工作台阅读轨道，继续保持顶部 `24px` 留白和水平居中，避免右侧内容在宽屏下被拉满；Settings 仍保留约 `1120px` 配置轨道。首次执行 `pnpm --dir web exec playwright test tests/ui/ai-layout-redesign.spec.ts --config=playwright.frontend.config.ts --reporter=line` 因动态端口 Vite 未保持监听导致 7 条用例均 `ERR_CONNECTION_REFUSED`，未进入产品断言；随后按既有固定端口方式复跑 `WEB_PORT=4177 pnpm --dir web exec playwright test tests/ui/ai-layout-redesign.spec.ts --config=playwright.frontend.config.ts --reporter=line`，结果 `7 passed (18.7s)`。
- 2026-07-08：根据用户反馈移除 Settings > IM 中重复的 Connections Provider 卡片和连接配置入口。左侧主入口 `IM` 继续承载 Connections、Targets、Routes、Schedules、History 全量 IM 配置能力；Settings > IM 只保留 Targets、Routes、Schedules、History，默认切入 `imGatewaySection=targets`，旧 `settings=im&imGatewaySection=connections` 会归一化到 Targets。第一轮 review 发现直接打开旧 Settings IM Connections 深链时还未触发归一化，已补充清理逻辑和 Playwright 断言；复测命令 `WEB_PORT=4177 pnpm --dir web exec playwright test tests/ui/ai-layout-redesign.spec.ts --config=playwright.frontend.config.ts --reporter=line`，结果 `7 passed (20.4s)`。
- 2026-07-08：根据用户反馈修复历史线程加载不稳定问题。打开带 `historyPath` 的线程时改为一次性请求完整 history，不再默认使用 `tail=true&limit=300` 或前端最近轮次切片；实时 `timeline_changed` 正常走 `since=end_index` 增量追加，lag/reconnect/不连续时回退完整 history 恢复并去重，避免旧消息被最后一页覆盖。同步修正每轮执行过程展示：完成轮次默认只显示最终回答和处理耗时，展开后文本过程直接展示，相邻命令折叠为命令组摘要，命令组展开后可查看命令 Input/Output。复测命令 `pnpm --dir web exec vitest run src/pages/AI/AgentChatSection.timeline.test.ts src/pages/AI/AgentChatSection.helpers.test.ts`，结果 `2 passed / 26 tests passed`；复测命令 `WEB_PORT=4177 pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts --config=playwright.frontend.config.ts --grep "loads full history|updates running history timeline|restores active timeline process steps|restores JSONL history|continues external runner history" --reporter=line`，结果 `5 passed (8.1s)`。
- 2026-07-08：根据用户反馈继续收敛每轮 process step 的真实展示样式。通过当前分支 3000 端口真实运行 UI 打开 `Runner: 继续` 历史线程，确认 Vite 进程来自 `/Users/eden_studio/work/github/bifrost-ai-tab-layout-plan/web`，真实 history 返回 `508` 条事件且 `has_more=false`；修复前运行中 process block 顶部额外显示全局摘要 `正在运行 146 条命令 · 1 条执行中 ...`，与目标截图不一致。修复后 3000 端口复查确认 process block 直接展示过程正文，相邻命令以轻量单行 `已运行 4 条命令`、`失败 6 条命令 · 5s` 插在正文之间，外层 `Expand execution process` 不存在，命令组默认 `aria-expanded=false` 且不展开 Input/Output；最终 DOM 复查确认 `hasRunningCommandSummary=false`、`visibleBadText=false`，截图记录 `/tmp/bifrost-ai-process-3000-lightweight.png`。
- 2026-07-08：根据用户反馈修复新建对话输入框图片粘贴回归。新建态输入面板移除无功能的左侧加号和右侧语音输入按钮；图片粘贴复用已有对话 composer 的预览条、删除按钮、大小标签和最多 6 张限制；修复外层 `startNewChat` controls 未转发 `images` 导致空态发送丢图的问题。复测命令 `pnpm --dir web exec vitest run src/pages/AI/AgentChatSection.images.test.tsx src/pages/AI/AgentChatSection.helpers.test.ts`，结果 `2 passed / 6 tests passed`；复测命令 `WEB_PORT=4177 pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts --config=playwright.frontend.config.ts --grep "pasted image|new chat landing sends pasted images" --reporter=line`，结果 `3 passed (9.5s)`，覆盖已有对话纯图片发送、空态输入框粘贴图片发送和外部 Runner 图片桥接。

执行明细：

- 2026-08-05：下线 Videos Tool 后执行 `pnpm --dir web exec playwright test tests/ui/ai-layout-redesign.spec.ts --grep "removes Videos|left rail switches|Settings does not trap" --workers=1`，结果 `3 passed`。亮色/暗色主题均无 Videos 导航和页面，`view=videos` / `aiSection=tools-videos` 回退到 New Chat；ASR、IM、Settings、New Chat 与历史线程切换继续通过。

| 用例 | 实际结果 |
| --- | --- |
| TC-AILR-01 | 通过。打开 `/ai` 后左侧 `New Chat` 为 `aria-current=true`，线程列表可见且没有选中线程，右侧 AI Shell 显示 `How can Bifrost help?` 与居中输入面板；Playwright 断言侧栏宽度在 `210-230px`、输入面板高度在 `110-140px`、圆角大于等于 `16px`，并验证 textarea 位于底部工具栏上方，Runner 与发送按钮都在同一工具栏内，未进入 Agent General/Model 配置页。 |
| TC-AILR-02 | 通过。mock 后端提供 `codex_runner`、`claude_runner`、`traex_runner`，默认显示 `Codex Runner`；打开下拉可见 `Claude Code`、`Trae X`，切换到 `Claude Code` 后输入内容保持可发送。 |
| TC-AILR-03 | 通过。在新建态输入 `Summarize current workspace status` 并发送，请求命中 `/api/im-gateway/chat/stream`，body 中 `runnerId=claude_runner`；NDJSON `run_finished` 返回后右侧显示 `Summary complete`，URL 更新为包含 `session=admin-chat-...` 且不再包含 `mode=new`。 |
| TC-AILR-04 | 通过。左侧点击 `Existing thread` 后 `New Chat` 取消选中，右侧显示历史消息 `Existing answer`；右上角保留 `Status`，不再显示内部 `agent-chat-new` 按钮；Playwright 断言线程行选中前后高度一致，composer 宽度充分使用右侧主内容区，不再保留旧内部 thread rail 空列；再次点击左侧 `New Chat` 回到新建对话输入态。 |
| TC-AILR-05 | 通过。ASR capability mock 为可用，左侧展示 `ASR`；点击后 URL 包含 `view=asr`，右侧渲染 ASR 工作台，不打开 Settings 二级内容页。 |
| TC-AILR-06 | 通过。点击 `IM` 后 URL 包含 `view=im`，右侧渲染 IM Gateway 工作台；旧 `imGatewaySection=routes` 深链可直接显示 Routes section；Connections Provider 使用 CSS grid 响应式卡片网格，mock 的 `feishu-main` 与 `weixin-main` Provider 卡片可见；工作台内容轨道最大宽度不超过约 920px、水平居中且顶部留白与 ASR 一致。 |
| TC-AILR-07 | 通过。点击底部 Settings 后 URL 包含 `view=settings`，右侧显示 `ai-settings-content`；顶部配置 tabs 只有 `Agent`、`Runner`、`IM` 三个入口。默认 Agent 分组可见并平铺 General、Model、Runtime、MCP Servers 等配置卡片；Settings 内容轨道宽度不超过约 1120px、在右侧主内容区居中且顶部留白一致；Runner 分组仅展示 Runners 配置卡片且不再显示 Agent General；IM 分组平铺 Targets、Routes、Schedules、History 等配置卡片，不再展示 Connections Provider 卡片网格，Provider 连接配置由左侧主入口 IM 工作台承载。Chat tab 不存在，`Back`、`Session Detail`、`Messages` 不存在。从历史对话点击 Settings 后 URL 清理 `session=history-thread-1`；切换 Runner 后 URL 包含 `settings=agent&agentSection=runners`，切换 IM 后 URL 包含 `settings=im&imGatewaySection=targets`。 |
| TC-AILR-08 | 通过。旧 `agent-chat`、`agent-model`、`tools-asr`、`im-gateway-routes` 链接均进入新 AI Shell 对应视图或 Settings 二级内容页；已下线的 `view=videos` 与 `aiSection=tools-videos` 均安全回退到 New Chat，无空白页和无限跳转；旧 Agent Model 链接进入 Agent 分组并展示 Model 配置卡片；`settings=agent&agentSection=chat&session=...` 被归一化为 Settings Agent General，不展示 Chat 或会话详情。 |
| TC-AILR-09 | 通过。分别在 `768x900` 与 `390x844` viewport 打开 `/ai`，New Chat、Runner 下拉和 Settings 二级内容页均可操作；`document.documentElement.scrollWidth <= window.innerWidth + 1`，未发现非预期横向滚动。 |
| TC-AILR-10 | 通过。从 `view=settings&settings=agent&agentSection=model` 进入 Settings 后，依次点击左侧 ASR、IM、New Chat 和历史线程，Playwright 断言 URL 分别切到 `view=asr`、`view=im`、`view=chat&mode=new`、`view=chat&session=history-thread-1`，均删除 `settings` 参数，不再显示 `ai-settings-content`，并展示对应主内容。 |
| TC-AILR-11 | 通过。Playwright 断言 ASR、IM 的工作台内容轨道顶部距离为约 `24px`、宽度不超过约 `920px` 且明显窄于右侧主内容区；Settings 配置轨道宽度不超过约 `1120px`；所有轨道都在 AI 右侧主内容区内水平居中。历史消息线程顶部不吸顶，Chat header 和消息区从统一留白后开始；IM 工作入口 Connections 使用 `settings-im-card-grid` CSS grid，Settings IM 不再重复展示 Connections；Videos 导航和内容轨道不存在。 |
| TC-AILR-12 | 通过。Playwright 在运行中对话里 mock 4 条队列消息，断言 `agent-chat-queue-list` 高度不超过两条消息空间、`scrollHeight > clientHeight` 且 `overflowY=auto`；第一条长消息右侧删除按钮位于同一队列行内并在文本之后，没有换行；删除第一条后按钮消失，其余队列消息仍保留在紧凑滚动区域。 |
| TC-AILR-13 | 通过。Playwright mock `/agent/sessions/history/<path>` 完整返回 old/middle/latest 事件，断言首次请求不包含 `tail`、`cursor`、`limit`，页面立即展示 `Oldest question`、`Middle answer`、`Newest answer`，且 `agent-chat-load-older` 不出现；运行中 timeline 回归同时断言增量追加后 `Previous question`、`Previous answer` 仍保留。 |
| TC-AILR-14 | 通过。Playwright mock 包含 assistant delta、tool_call、tool_result、final assistant message 的 timeline，断言完成轮次折叠时显示 `IM timeline answer` 且不显示中间过程；展开后过程文本按顺序出现，process block 不再显示额外的外层全局执行摘要或 `Expand execution process`，命令组摘要直接以 `已运行 1 条命令` 的轻量单行展示；展开命令组后可见 `exec_command`、`pnpm test` 和 `ok`。3000 端口真实线程复查同样确认运行中长过程按正文 + 命令摘要穿插展示，页面内不再出现 `条执行中`。 |
| TC-AILR-15 | 通过。Playwright 打开 `/ai` 新建对话输入态后，断言输入面板内 `Attach context` 和 `Voice input` 均不存在；粘贴 PNG 后同一输入面板显示 `agent-chat-image-preview`，切换暗色主题后预览仍存在；发送后普通对话页面显示用户文本、图片缩略图和 `Landing image received`，请求 payload 含 `message="Describe the pasted screenshot"` 以及 `images[0].mime_type=image/png`。同一命令还回归已有对话 composer 的最多 6 张图片、纯图片发送和外部 Runner `mimeType=image/png` 图片桥接。 |
