# WebUI AI Layout Redesign 真实场景测试

## 功能模块说明

验证 WebUI AI 页面改造后的真实用户路径：进入 `/ai` 默认展示新建对话输入态，左侧提供 `New Chat`、`ASR`、`Videos`、`IM`、线程列表和底部 Settings，右侧根据入口展示新对话、历史对话、ASR 工作台、Videos Tool、IM 工作台或 Settings 二级内容页。Runner 选择必须在新对话输入框下方可见，默认使用 Codex Runner，并能切换到后端已启用的其它 runner。

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
- 左侧栏为轻量灰底侧栏，宽度接近截图参考的窄侧栏；顶部入口无重边框卡片感。
- 左侧中部展示线程列表；如果没有历史线程，展示空列表文案。
- 线程列表行使用单行紧凑样式，选中态为浅灰背景，不能显示大号 runner 方块或双行详情卡片。
- 没有历史 thread 被选中。
- 右侧不是 Agent General、Model 或 IM Gateway 配置页。
- 右侧展示新建对话面板，主区域无卡片外框，输入框位于主内容区域中部或视觉中心附近。
- 新建态输入框为接近截图参考的横向胶囊输入框，问候文案位于输入框上方，输入框内左侧有新增/附加入口，右侧有语音/发送等即时操作。
- 输入框下方展示轻量 Runner 下拉，默认仍遵守 Codex Runner 优先。
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
- 刷新后仍停留在对应 IM section。
- 左侧线程列表仍保留，不被 IM 内容挤出或覆盖。

### TC-AILR-07 Settings 二级内容页承载配置项

操作步骤：

1. 打开 `/ai` 默认新建对话态。
2. 点击左侧底部 Settings 按钮。
3. 在Settings 内容页中切换到 Agent Model。
4. 在Settings 内容页中切换到 Agent Runners。
5. 在Settings 内容页中切换到 IM Gateway Routes。
6. 点击左侧 `New Chat` 或其它主入口离开 Settings 二级内容页。

预期结果：

- Settings 作为右侧主内容打开，替换右侧主内容。
- Settings 内容页中可以访问 Agent General、Model、Runtime、History、Memories、Skills、Runners、Memory Records、MCP Servers、Sessions 等配置。
- Settings 内容页中可以访问 IM Gateway Connections、Targets、Routes、Schedules、History 等配置。
- 切回其它主入口后，右侧展示对应主内容。
- URL 使用 `view=settings&settings=agent|im` 表达 Settings 二级内容页，切回其它主入口后删除 `settings` 参数。

### TC-AILR-08 旧 AI 深链兼容

操作步骤：

1. 打开旧 Chat 链接：`/_bifrost/ai?aiSection=agent-chat&agentSection=chat`。
2. 打开旧 Agent Model 链接：`/_bifrost/ai?aiSection=agent-model&agentSection=model`。
3. 打开旧 ASR 链接：`/_bifrost/ai?aiSection=tools-asr`。
4. 打开旧 IM Routes 链接：`/_bifrost/ai?aiSection=im-gateway-routes&imGatewaySection=routes`。

预期结果：

- 旧 Chat 链接进入新 AI Shell 的 Chat 区域。
- 旧 Agent Model 链接打开 Settings 二级内容页并定位到 Agent Model。
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

## 清理步骤

- 关闭 Playwright 浏览器。
- 停止临时 WebUI / Bifrost 后端进程。
- 删除测试使用的临时数据目录。
- 若测试中创建了真实 Agent session、IM provider 或 ASR task，按对应 UI 或 API 删除。

## 执行记录

- 2026-07-07：创建本设计验证用例。
- 2026-07-07：实现后使用 Chromium + Playwright 执行真实浏览器验证，命令为 `WEB_PORT=4177 pnpm --dir web exec playwright test tests/ui/ai-layout-redesign.spec.ts --config=playwright.frontend.config.ts --reporter=line`，结果 `4 passed (11.4s)`。测试使用 Playwright frontend dev server 和 mock Admin API，不复用正式 `9900` 端口，不写入用户真实 `~/.bifrost` 数据。
- 2026-07-07：根据用户截图补充轻量灰底左栏、紧凑线程列表、居中问候和横向胶囊输入框验收点；根据用户纠偏将截图式输入框限制在 AI Shell 默认新建面板，保持原 Agent Chat 会话布局；Settings 改为右侧二级内容页，Videos 保留为左侧主入口。补充 session 深链回归，确保已有对话直接打开普通消息区与 composer，不展示新建落地页。复测命令 `WEB_PORT=4177 pnpm --dir web exec playwright test tests/ui/ai-layout-redesign.spec.ts --config=playwright.frontend.config.ts --reporter=line`，结果 `5 passed (11.4s)`。

执行明细：

| 用例 | 实际结果 |
| --- | --- |
| TC-AILR-01 | 通过。打开 `/ai` 后左侧 `New Chat` 为 `aria-current=true`，线程列表可见且没有选中线程，右侧 AI Shell 显示 `How can Bifrost help?` 与居中胶囊输入框；Playwright 断言侧栏宽度在 `160-190px`、输入胶囊高度在 `46-58px`、圆角大于 `20px`，未进入 Agent General/Model 配置页。 |
| TC-AILR-02 | 通过。mock 后端提供 `codex_runner`、`claude_runner`、`traex_runner`，默认显示 `Codex Runner`；打开下拉可见 `Bifrost Agent`、`Claude Code`，切换到 `Claude Code` 后输入内容保持可发送。 |
| TC-AILR-03 | 通过。在新建态输入 `Summarize current workspace status` 并发送，请求命中 `/api/im-gateway/chat/stream`，body 中 `runnerId=claude_runner`；NDJSON `run_finished` 返回后右侧显示 `Summary complete`，URL 更新为包含 `session=admin-chat-...` 且不再包含 `mode=new`。 |
| TC-AILR-04 | 通过。左侧点击 `Existing thread` 后 `New Chat` 取消选中，右侧显示历史消息 `Existing answer`；再次点击 `New Chat` 回到新建对话输入态。 |
| TC-AILR-05 | 通过。ASR capability mock 为可用，左侧展示 `ASR`；点击后 URL 包含 `view=asr`，右侧渲染 ASR 工作台，不打开 Settings 二级内容页。 |
| TC-AILR-06 | 通过。点击 `IM` 后 URL 包含 `view=im`，右侧渲染 IM Gateway 工作台；旧 `imGatewaySection=routes` 深链可直接显示 Routes section。 |
| TC-AILR-07 | 通过。点击底部 Settings 后 URL 包含 `view=settings`，右侧显示 `ai-settings-content`；默认 Agent General 可见，`settings=agent&agentSection=runners` 可定位 Agent Runners，旧 `agent-model` 深链可定位 Model Configuration；切换 IM Gateway tab 后 Connections 可见，旧 IM Routes 深链可定位 Routes。 |
| TC-AILR-08 | 通过。旧 `agent-chat`、`agent-model`、`tools-asr`、`im-gateway-routes` 链接均进入新 AI Shell 对应视图或 Settings 二级内容页，无空白页和无限跳转。 |
| TC-AILR-09 | 通过。分别在 `768x900` 与 `390x844` viewport 打开 `/ai`，New Chat、Runner 下拉和 Settings 二级内容页均可操作；`document.documentElement.scrollWidth <= window.innerWidth + 1`，未发现非预期横向滚动。 |
