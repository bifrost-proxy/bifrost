# WebUI AI 能力中心

## 适用范围

验证 AI 板块从会话工作台改为模块化能力中心后的用户可见行为：四卡首页、独立详情页、摘要型运行记录、无详情下钻、响应式布局，以及亮色/暗色主题一致性。

## 环境与数据边界

- 使用 Playwright 启动隔离的 Vite 前端，Admin API 由测试路由提供固定摘要数据。
- 不写入用户真实 `~/.bifrost`，不创建外部 Agent 会话。
- 浏览器网络请求必须只使用 `/im-gateway/agent/session-summaries` 获取运行记录；不得请求 sessions/all、history、单线程详情或消息正文。

## 用例

### TC-AIH-01 四卡首页与基础摘要

1. 打开 `/_bifrost/ai`。
2. 确认首页固定展示 ASR、IM 通道、Agent 配置、Agent 运行记录四张卡片。
3. 确认桌面宽度为两列布局，卡片展示对应任务数、连接数、Runner 数与运行计数。
4. 确认页面不存在 New Chat、Threads 或旧会话详情入口。
5. 确认模块标题、说明、指标与操作默认使用英文，不出现本模块写死的中文 UI 文案。

预期：四卡结构稳定，局部摘要失败不阻断卡片进入，旧聊天工作台不再出现。

### TC-AIH-02 详情路由与返回导航

1. 分别点击四张模块卡片。
2. 确认进入 `/ai/asr`、`/ai/channels`、`/ai/agents`、`/ai/runs`。
3. 确认详情页页头和内容区与首页卡片内容区同为 1120px 最大宽度，并水平居中对齐。
4. 在 IM Channels 准备至少三个 Provider，确认桌面宽度下一排仅展示两个 Provider 卡片，第三张换到下一行；正文首个工具栏与页头分隔线之间保留 24px 顶部留白。
5. 在 Agent Configuration 确认 Runners 位于顶部，其后仅保留 General；页面没有 `Enable Agent` 开关，也没有 Skills 管理区。
6. 每页点击左上角“AI Home”。

预期：每个模块都有独立 URL，返回操作均回到四卡首页且保留系统侧栏；IM Channels 宽屏最多两列，详情正文不紧贴页头。

### TC-AIH-03 运行记录摘要与数据最小化

1. 打开 `/ai/runs`。
2. 确认列表按开始时间倒序，展示状态、标题、Runner、时长、用户消息数、来源、开始时间。
3. 对正在运行的线程记录其 `start_time`，刷新页面或返回首页后重新进入，确认时长仍为“当前时间减执行开始时间”且继续增长，不从 0 开始。
4. 检查飞书群聊显示 `Feishu` 且空标题回退为群名称；飞书单聊空标题回退为机器人名称；微信会话显示 `Weixin`。
5. 操作状态筛选并观察 URL。
6. 检查行内不存在链接或点击下钻；检查网络请求。

预期：运行中时长基于真实执行开始时间跨页面访问连续累计；IM 来源按 Provider 归类，标题按群名称/机器人名称回退；筛选写入 URL；列表仅展示允许的摘要字段；不请求工作目录、消息、思考、工具调用、历史路径和诊断详情。

### TC-AIH-04 窄屏布局

1. 将视口设置为 `390 × 844`。
2. 打开 AI 首页，再打开包含至少三个 Provider 的 IM Channels，最后打开运行记录。

预期：首页卡片和 IM Provider 卡片均变为单列；详情正文顶部保留 16px 留白；运行记录变为不可点击的移动端摘要卡片，无横向表格溢出。

### TC-AIH-05 亮色与暗色主题

1. 分别设置系统主题为 light 和 dark 后打开 AI 首页与 IM Channels。
2. 检查页面、模块卡片、Provider 卡片、文字、边框、状态 Tag 与交互焦点。

预期：两种主题均复用 Ant Design 语义 token，背景确实切换，文字和状态保持可读。

### TC-AIH-06 旧链接收口

1. 打开带 `session` 和 `historyPath` 的旧 `/ai` 链接。
2. 确认 Agent Configuration 只展示 Runners 和 General，不出现 Skills、Chat、History、Sessions 或消息详情组件。

预期：重定向到 `/ai/runs`，只用 session key 作为摘要搜索词；不再渲染消息详情，旧聊天和会话详情前端代码不再构建。

## 执行记录

- 2026-08-14：使用隔离 Vite 前端与 Playwright Chromium 执行 `node_modules/.bin/playwright test tests/ui/ai-layout-redesign.spec.ts --config=playwright.frontend.config.ts --reporter=line`，TC-AIH-01 至 TC-AIH-06 全部通过（`6 passed`）。首次执行为 `5 passed / 1 failed`，失败原因是测试点击 Ant Select 占位文字时被 combobox 输入层拦截；改用可交互 Select 容器和可见下拉层后，专项复测 `1 passed`，完整复测 `6 passed (25.0s)`。网络断言确认页面未请求 `/sessions/all`、`/sessions/history` 或单线程详情。
- 2026-08-14：根据体验反馈补充默认英文 UI 与详情宽度对齐；自动用例新增英文文案、1120px 同宽和水平居中断言，最终完整复测 `6 passed (53.2s)`。
- 2026-08-14：删除旧聊天工作台、线程/消息渲染、会话列表与会话详情前端代码后，复查源码无残余引用；Agent Configuration 自动断言仅保留 General、Skills、Runners。再次执行完整 AI Hub Playwright，结果 `6 passed (20.9s)`。
- 2026-08-14：根据配置优先级反馈，将 Agent Configuration 调整为 Runners、General、Skills 顺序并删除 `Enable Agent` 总开关；新增 DOM 顺序与开关不存在断言，完整复测 `6 passed (57.7s)`。
- 2026-08-14：按最终改造执行隔离 Vite + Playwright Chromium 全量 AI Hub 回归；真实刷新 `/ai/runs` 后运行中时长仍从执行开始时间累计，不从 0 重置。同时验证 Feishu/Weixin 来源、Provider 卡片桌面两列/窄屏单列、详情顶部留白、双主题、Skills 移除与 Runners/General 顺序，结果 `6 passed (2.0m)`。
