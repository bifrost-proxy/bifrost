# WebUI AI Layout Redesign

## 背景

当前 WebUI AI 页面把 Agent、ASR、Videos 和 IM Gateway 的多个配置 section 平铺在一级左侧导航中。用户进入 `/ai` 后默认落在 Agent General 配置，而不是开始一次 AI 任务；真正的 Agent Chat 又在内部维护一套独立的对话区、右侧线程 rail、`New Chat` 弹窗和 `Status` 弹窗。

这个结构对配置管理友好，但对“使用 AI 完成任务”的主路径不够直接：

- 新用户进入 AI 页面后首先看到配置项，不能直接输入任务。
- 线程列表只出现在 Chat 页面内部，而且位于右侧，不是 AI 页面稳定的上下文导航。
- ASR 和 IM Gateway 既是工作入口，又混在配置 section 中，用户很难区分“开始使用”和“系统设置”。
- Agent Runner 选择藏在新建对话弹窗里，默认 runner 语义不够清晰。

本方案把 AI 页面调整为类似 Codex 的工作台：左侧是固定工作导航，右侧是当前内容。进入 AI 页面默认就是新建对话输入态，用户可以直接输入任务并生成新的对话线程。

## 用户目标验证清单

### 必须实现

- `/ai` 默认展示新建对话面板，左侧 `New Chat` 处于选中态，不默认打开配置页。
- 新建对话面板在右侧主内容区垂直居中展示输入框，用户输入消息并发送后才创建新线程。
- 新建对话输入面板底部工具栏展示 Runner 下拉选择，位置对应截图中“高级”操作区，默认使用 Codex Runner。
- Runner 下拉必须包含后端已启用的 runner，至少能展示 Codex、Bifrost Agent、Claude Code、Trae X 这几类可用 runner。
- 左侧顶部区域包含 `New Chat`、`ASR`、`Videos`、`IM` 四个工作入口。
- 左侧中间区域展示所有 Agent threads，支持选中、运行状态、加载更多、右键删除或等效删除入口。
- 左侧底部只有一个 Settings 入口，点击后在 Settings 内容页中操作原 AI 左侧菜单里的配置项。Settings 顶部只保留 `Agent`、`Runner`、`IM` 三个分组 tab；每个 tab 内把归属该分组的配置项以卡片方式向下平铺。
- Settings 不承载某个对话的状态信息；`Back`、`Session Detail`、`Messages`、workspace、runner、context、diagnostics 等会话级信息只能出现在具体对话的头部操作或弹窗中。
- 右侧主内容根据左侧入口切换：
  - `New Chat` / thread：Agent Chat 对话区。
  - `ASR`：现有 ASR 工作台。
  - `IM`：IM 通道入口或 IM Gateway 工作台。
  - `Settings`：配置二级内容页，替换右侧主内容。

### 必须不破坏

- 现有 Agent Chat SSE 流、队列输入、停止、图片粘贴、slash command、Plan Mode、历史加载和 token HUD 行为保持可用。
- 打开已有 Agent Chat 线程时必须一次性加载该线程完整历史，不再用前端“最近几轮”窗口或 `tail=true&limit=...` 作为默认展示数据源；实时推送只能追加合并和去重，不能用最后一页覆盖当前完整消息列表。
- 对话中每一轮执行过程需要接近 Codex 风格：文本推理/状态作为普通过程文本展示，相邻命令折叠成一条命令组摘要，展开后可查看每条命令及 Input/Output，默认完成轮次只展示最终回答和简洁处理耗时。
- 已有线程深链仍可打开对应对话，例如 `session` / `historyPath` / `view` 参数不丢失。
- ASR 内部深链参数继续工作，例如 `asrTab`、`asrTask`、`asrTaskTab`、`asrDay`。
- IM Gateway 内部 section 深链继续工作，例如 `imGatewaySection=connections|targets|routes|schedules|history`。
- 现有 Videos Tool 能力不能被删除。它作为左侧主入口之一保留，并继续支持旧深链。
- 旧 URL 兼容：
  - `aiSection=agent-chat&agentSection=chat` 进入 Chat。
  - `aiSection=tools-asr` 进入 ASR。
  - `aiSection=tools-videos` 进入 Videos Tool。
  - `aiSection=im-gateway-*` 进入 IM 或 Settings 中对应 IM section。
  - `agentSection=*` 在 Settings 二级内容页中仍定位到对应 Agent 配置 section。
- 小屏幕布局不能出现文字溢出、按钮重叠、线程列表挤压输入框或 Settings 二级内容页不可滚动/不可切回主入口。
- 桌面左侧栏必须给线程标题留出足够宽度，目标宽度约 216px；线程选中态不能改变行高、字体基线或虚拟列表估算高度，避免点击时列表抖动。
- 外置线程列表后，右侧 Chat conversation 不能继续保留旧内部 thread rail 的空列；嵌入 AI Shell 时消息区和 composer 应使用右侧主内容宽度，仅保留合理的阅读宽度上限和边距。

### 必须真实验证

- WebUI 自动化测试覆盖默认进入 `/ai` 后的新建对话居中输入态。
- WebUI 自动化测试覆盖默认 Codex Runner 选择、Runner 下拉可切换到 Bifrost Agent / Claude Code / Trae X。
- WebUI 自动化测试覆盖发送首条消息后创建 session，URL 和左侧线程选中态同步。
- WebUI 自动化测试覆盖点击历史 thread 后退出新建态并打开旧对话。
- WebUI 自动化测试覆盖打开历史 thread 时全量展示旧消息、中间消息和最新消息，不出现无必要的 `Load more`；运行中 timeline 推送追加后历史消息仍保留。
- WebUI 自动化测试覆盖执行过程样式：完成轮次折叠后只显示最终回答，展开后过程文本、命令组和命令详情按预期展示。
- WebUI 自动化测试覆盖 ASR / IM 工作入口切换和 Settings 二级内容页配置入口。
- human_tests 覆盖真实浏览器中的默认态、Runner 选择、线程切换、ASR/IM/Settings、深链兼容、窄屏布局。

## 信息架构

AI 页面只保留一个模块级 shell：

```text
AI Shell
├── Left Rail
│   ├── Primary Actions
│   │   ├── New Chat
│   │   ├── ASR
│   │   ├── Videos
│   │   └── IM
│   ├── Threads
│   │   ├── active / idle / failed status
│   │   ├── runner mark
│   │   ├── title
│   │   └── relative time / duration
│   └── Settings
└── Main Content
    ├── New Chat Compose
    ├── Chat Conversation
    ├── ASR Workspace
    ├── Videos Tool
    ├── IM Workspace
    └── Settings Content
```

一级左栏不再展示 Agent General、Model、Runtime、History、Memories、Skills、Runners、Memory Records、MCP Servers 等配置型 section。Settings 顶部只展示 `Agent`、`Runner`、`IM` 三个分组 tab，内容区只挂载当前分组，并把该分组内的配置 section 作为卡片纵向平铺；IM Provider Connections 由左侧主入口 `IM` 工作台承载，不在 Settings > IM 中重复展示；不能把所有分组同时挂在隐藏 tabpane 中，避免重复表单和路由状态串扰。

## 路由与状态

新增 AI 页面内部视图参数：

| URL | 含义 |
| --- | --- |
| `/ai` | 默认等同于 `/ai?view=chat&mode=new` |
| `/ai?view=chat&mode=new` | 新建对话输入态 |
| `/ai?view=chat&session=<session_key>` | 打开指定 live session |
| `/ai?view=chat&historyPath=<path>` | 打开历史 JSONL 会话 |
| `/ai?view=asr&asrTab=scheduled` | 打开 ASR 工作台 |
| `/ai?view=im&imGatewaySection=connections` | 打开 IM 工作台 |
| `/ai?view=videos` | 打开 Videos Tool 兼容入口 |
| `/ai?settings=agent&agentSection=model` | 打开 Settings 二级内容页的 Agent 分组，Agent 配置卡片平铺展示并包含 Model |
| `/ai?settings=agent&agentSection=runners` | 打开 Settings 二级内容页的 Runner 分组 |
| `/ai?settings=im&imGatewaySection=targets` | 打开 Settings 二级内容页的 IM 分组，IM 配置卡片平铺展示并从 Targets 开始 |

兼容旧参数时只做映射，不把旧左侧 section nav 继续作为 UI 展示：

- `aiSection=agent-chat` -> `view=chat`。
- `aiSection=agent-general|agent-model|...` -> `view=settings&settings=agent&agentSection=<section>`。
- `aiSection=tools-asr` -> `view=asr`。
- `aiSection=tools-videos` -> `view=videos`。
- `aiSection=im-gateway-routes` -> `view=im&imGatewaySection=routes`；IM Provider Connections 只通过主入口 `view=im` 展示，Settings 的 IM 分组从 Targets 开始。

## 默认新建对话交互

进入 `/ai` 时：

1. 左侧 `New Chat` 选中。
2. 左侧 threads 加载并展示，但没有历史 thread 被选中。
3. 右侧展示新建对话面板，输入区垂直居中。
4. 输入面板底部工具栏展示 Runner 下拉，位置与附加/高级操作同一排，不单独漂在面板外。
5. 默认 Runner 为 Codex Runner。
6. 如果后端没有启用 Codex Runner，前端必须选择第一个可用 runner，并在下拉中显示真实选中值；不能显示 Codex 但实际使用其它 runner。

发送首条消息：

1. 用户在居中输入框输入任务。
2. 点击 Send 或按提交快捷键。
3. 前端用当前输入、当前 Runner、当前 workspace 创建新的 Agent session。
4. SSE 流开始后右侧切换为普通 Chat conversation。
5. URL 更新为具体 session，例如 `/ai?view=chat&session=admin-chat-...`。
6. 左侧 threads 立即插入或刷新新线程，并把新线程标为选中。

点击 `New Chat`：

- 总是回到 `/ai?view=chat&mode=new`。
- 清空右侧新建输入框和 pending images。
- 不删除当前线程，不停止正在运行的线程。
- 如果当前线程仍在运行，左侧 thread 继续显示 running 状态。

## Runner 选择

Runner 下拉只影响新建对话，不影响已有历史线程。新建态中 Runner 控件必须位于输入面板底部工具栏，和附加、语音、发送操作形成一条稳定基线；输入文本区域独立占据面板上半部分，避免 textarea、Runner 和发送按钮互相挤压。

Runner 展示名建议：

| adapter / runner | 展示名 |
| --- | --- |
| `codex` | Codex Runner |
| `bifrost_agent` / `builtin` | Bifrost Agent |
| `claude_code` | Claude Code |
| `traex` | Trae X |
| `chatgpt_web` | ChatGPT Web |
| custom id | 使用后端返回 label，缺失时展示 runner id |

排序规则：

1. Codex Runner。
2. Bifrost Agent。
3. Claude Code。
4. Trae X。
5. ChatGPT Web。
6. 其它自定义 runner 按 label 或 id 排序。

Runner 下拉必须展示不可用状态：

- 后端返回 disabled runner 时，不出现在默认可选列表。
- 如果未来需要展示 disabled runner，只能置灰并给出原因，不能允许提交。
- 发送时再次校验当前 runner 仍可用；不可用则阻止创建并提示刷新。

## 左侧线程列表

线程列表使用现有 `/api/im-gateway/agent/sessions/all?limit=80` 数据源。第一版应复用现有 `AgentThreadListCard` 的虚拟列表、状态计算、runner mark、load more 与 delete 行为，但视觉上改成左栏原生列表，而不是右侧 Card。桌面左栏宽度约 216px，compact thread item 使用固定高度；选中态只改变背景和文字颜色，不通过加粗或额外 padding 改变行高。

线程选中规则：

- `mode=new` 时没有 thread 选中。
- URL 有 `session` 或 `historyPath` 时按现有 `isSelectedThread()` 规则选中。
- 用户点击 thread 后：
  - URL 切到 `view=chat`。
  - 删除 `mode=new`。
  - 保留 `session` / `historyPath` / `view` 等必要参数。

线程列表刷新：

- 进入 AI 页面立即加载。
- Chat SSE run started / finished / failed 后刷新。
- 每个 active running thread 保持轮询或 SSE 事件触发刷新。

## Chat 历史加载与实时合并

选中历史 thread 或通过 `historyPath` 深链打开会话时，前端必须优先请求完整 timeline history：

- 初始请求调用 `/api/im-gateway/agent/sessions/history/<path>` 时不带 `tail`、`limit`、`cursor` 或 `since`。
- 后端未分页返回时，`has_more=false`，右侧不展示 `Load more`。
- 如果 session detail 和 timeline 同时存在，前端将两者合并去重；当两边消息都有时间戳时按时间稳定排序，缺失时间戳时保持原合并顺序，避免 mock、旧数据或部分数据源乱序。
- 不再对已选中的 active/detail 消息调用最近 N 轮切片；用户打开线程即看到完整上下文。
- 只有后端明确返回 `has_more=true` 时才展示 `Load more`，该按钮只用于真实分页历史，不用于前端 recent-window 扩展。

实时推送策略：

- 普通 `timeline_changed` 且本地已有 `end_index` 时，使用 `since=<currentEndIndex>` 拉增量事件。
- 如果收到 lagged/reconnect、缺失 `end_index`，或增量返回的 `start_index` 与本地 `end_index` 不连续，则改拉完整未分页 history 做恢复。
- 增量事件追加到本地事件窗口后重新生成消息；消息合并按 role + normalized content 去重，避免 detail、timeline 和 SSE 三个来源重复显示同一轮。
- 恢复路径禁止使用 tail page 覆盖当前消息列表，避免用户看到“只剩最后一条人类输入和 Agent 回复”的不稳定状态。

## Chat 执行过程展示

每一轮对话的执行过程遵循“最终回答优先、过程可展开”的模型：

- 完成轮次默认折叠，只展示 user message、处理耗时和最终 assistant answer；中间 delta、tool call、status 不直接占据主消息流。
- 展开完成轮次后，按时间顺序展示过程文本、命令组和最终回答；最终回答必须稳定保留在该轮最后。
- thinking/status 文本作为普通可读过程文本展示，长文本允许“展开更多”，不使用大块灰底日志卡片。
- 相邻 tool steps 合并为一条轻量命令组摘要，例如 `已运行 2 条命令`；失败命令组可追加短耗时，例如 `失败 1 条命令 · 2s`。
- 点击命令组后展示具体命令行；命令详情默认随命令组展开可见，用户可以继续点击单条命令折叠或展开 Input/Output。
- 运行中轮次默认展开过程摘要和 thinking tail；完成后自动回到折叠状态，减少纵向占用。

## ASR 入口

左侧 `ASR` 是工作入口，不是 Settings。

点击 `ASR`：

- URL 切到 `view=asr`。
- 右侧渲染现有 `<ASR />`。
- ASR 内部 tab 与 task detail 继续由现有 URL 参数控制。
- 如果平台 capability 隐藏 ASR，则左侧不展示 ASR 入口；旧 `view=asr` 深链显示能力不可用空态或回退到新建对话。

## IM 入口

左侧 `IM` 表示 IM 通道工作入口。第一版可以复用现有 `<ImGatewayTab hideSectionNav />` 作为右侧内容，但长期目标是把高频 IM 工作流和低频配置区分开：

- 高频：连接状态、通道健康、最近消息、手动发送测试、目标 channel 选择。
- 低频：Provider、Targets、Routes、Schedules、History 详细配置。

第一版主入口 `IM` 直接复用 `ImGatewayTab`，并承载 Connections、Targets、Routes、Schedules、History 全量 IM 配置能力。Settings 二级内容页中的 `IM` 分组只保留 Targets、Routes、Schedules、History，避免 Provider Connections 在两个入口重复出现。

## Videos Tool

Videos Tool 作为左侧主入口之一保留，避免用户认为能力消失。旧 `aiSection=tools-videos` 映射到 `view=videos`，右侧渲染 `<VideosTool />`。必须保留 YouTube 下载入口、默认目录、自定义目录、下载进度和非 YouTube URL 拒绝行为。

## Settings 二级内容页

左侧底部 Settings 入口切换到右侧 Settings 内容页。Settings 内容页以顶部 tabs 或 segmented nav 承载三个配置分组，顶层只能出现 `Agent`、`Runner`、`IM`：

- Agent：General、Model、Runtime、History、Memories、Skills、Memory Records、MCP Servers、Sessions。
- Runner：Runners。
- IM：Targets、Routes、Schedules、History。Connections Provider 配置由左侧主入口 `IM` 工作台承载。
- Speech / ASR Resources：仅当现有 Settings Speech 初始化入口需要与 AI 入口打通时纳入；否则 ASR 资源仍保留现有 Settings Speech。

每个分组内部直接展示该组的配置卡片，按稳定顺序从上到下排列，不再把 General、Model、Runtime、Runners、IM Routes 等作为 Settings 顶层 tab。Settings 内容不应撑满整个右侧主内容区；应使用和嵌入式 Chat message/composer track 一致的阅读宽度上限（约 1120px）并整体居中。这样用户在 Settings 中先按对象类型选择 `Agent` / `Runner` / `IM`，再在当前页纵向扫配置卡片。

Chat 或历史会话不属于 Settings 配置项。打开 Settings 时必须清理 `mode`、`session`、`historyPath` 等会话路由状态；如果旧链接传入 `settings=agent&agentSection=chat`，必须归一化到 Agent General。会话详情、消息列表、返回按钮和诊断状态应由具体对话页右上角操作触发弹窗查看，不应出现在 Settings 页面。

Settings URL 语义：

- 打开 Settings 时设置 `view=settings&settings=agent|im`；Runner 分组使用 `settings=agent&agentSection=runners` 兼容既有 Agent runners 路由。
- 离开 Settings 时切换到对应主入口，保留可兼容的 section 参数。
- Settings 顶部切到 `Agent` 时更新为 `settings=agent`，并把非法或会话型 `agentSection` 归一化到 `general`。
- Settings 顶部切到 `Runner` 时更新为 `settings=agent&agentSection=runners`。
- Settings 顶部切到 `IM` 时更新为 `settings=im&imGatewaySection=targets`，并在 IM 分组内平铺 Targets、Routes、Schedules、History 配置卡片；旧 `settings=im&imGatewaySection=connections` 归一化到 Targets。
- Settings 内部不能保留 `session`、`historyPath`、`mode=new` 或 `agentSection=chat`。

## 组件拆分建议

第一版尽量不重写成熟业务组件，只调整容器层：

- `web/src/pages/AI/index.tsx`
  - 改为 AI Shell。
  - 管理 `view`、`mode`、settings open state 和旧 URL 映射。
- `web/src/pages/AI/AgentChatSection.tsx`
  - 增加 `mode="new" | "thread"` 或拆出 `AgentChatConversation`。
  - 支持隐藏内部 thread rail。
  - 暴露 thread state 或把 thread state 提升到 AI Shell。
- `web/src/pages/AI/AgentChatSection.panels.tsx`
  - 抽出 `AgentThreadList`，支持 `variant="rail" | "sidebar"`。
  - 保留现有测试 id，降低测试迁移成本。
- `web/src/pages/AI/AISettingsContent.tsx`
  - 新增 Settings 内容页，复用 `AgentTab hideSectionNav` 和 `ImGatewayTab hideSectionNav`。
  - 顶层只暴露 `Agent`、`Runner`、`IM` 三个 tab，并通过 `visibleSections` 控制每个分组内部的卡片集合。
- `web/src/pages/AI/NewChatCompose.tsx`
  - 新增默认新建对话面板，包含居中输入框、pending images、Runner 下拉、workspace 可选入口和 Send。

## 实现阶段

### Phase 1：AI Shell 与默认新建态

- `/ai` 默认映射到 `view=chat&mode=new`。
- 新增左栏骨架。
- 新增居中新建对话输入面板。
- Runner 下拉默认 Codex Runner，使用现有 chat config 数据源。

### Phase 2：线程列表外置

- 抽出 thread list。
- Chat 右侧不再展示内部 thread rail。
- 点击 thread 打开 conversation。
- 新建首条消息后创建并选中新 thread。

### Phase 3：ASR / IM 工作入口

- 左侧 ASR 按 capability 展示。
- 右侧渲染 ASR，并保持 ASR 深链参数。
- 左侧 IM 渲染 IM 工作台，并保持 IM Gateway 参数。

### Phase 4：Settings 二级内容页

- 迁移 Agent 配置 section 到 `Agent` 分组卡片列表。
- 迁移 Runner 配置 section 到 `Runner` 分组卡片列表。
- 迁移 IM Gateway 配置 section 到 `IM` 分组卡片列表。
- 旧 `agentSection` / `imGatewaySection` 深链打开 Settings 中对应分组。

### Phase 5：测试与清理

- 更新 Playwright 用例。
- 新增 human_tests。
- 删除旧 AI section nav 依赖。
- 保留兼容层并补回归测试。

## 测试方案

### 单元测试

- `resolveAiRouteState_defaults_to_new_chat`：空 query 映射到 `{ view: "chat", mode: "new" }`。
- `resolveAiRouteState_maps_legacy_agent_chat`：`aiSection=agent-chat&agentSection=chat` 映射到 Chat conversation。
- `resolveAiRouteState_maps_legacy_agent_config_to_settings`：`aiSection=agent-model` 打开 Settings Agent Model。
- `resolveAiRouteState_keeps_chat_out_of_settings_routes`：`settings=agent&agentSection=chat&session=<id>` 归一化到 Settings Agent General，且不把会话状态当作 Settings 内容。
- `resolveAiRouteState_maps_legacy_asr`：`aiSection=tools-asr` 映射到 ASR。
- `buildRunnerOptions_prefers_codex_then_builtin_then_claude_then_traex`：Runner 排序符合产品语义。
- `selectDefaultRunner_falls_back_when_codex_unavailable`：Codex 不可用时选择第一个可用 runner，并返回 fallback reason。

### Web UI E2E

- `ai-layout-default-new-chat.spec.ts`
  - 打开 `/ai`。
  - 断言左侧 `New Chat` 选中。
  - 断言右侧居中输入面板可见。
  - 断言线程列表可见但没有选中 thread。
  - 断言 Runner 下拉默认 `Codex Runner`。
- `ai-layout-create-thread.spec.ts`
  - mock chat config 包含 Codex、Bifrost Agent、Claude Code、Trae X。
  - 切换 Runner 到 Claude Code。
  - 输入首条消息并发送。
  - 断言请求使用 Claude Code runner。
  - 断言 URL 更新到 session。
  - 断言左侧新 thread 选中。
- `ai-layout-thread-navigation.spec.ts`
  - mock 两条历史 thread。
  - 点击历史 thread。
  - 断言退出 new mode，右侧展示历史消息。
  - 断言左侧 thread item 选中前后高度一致，Chat conversation 的 composer 使用右侧主内容宽度，不再保留旧 thread rail 空列。
  - 再点击 `New Chat`，断言回到居中输入态。
- `agent-chat-full-history.spec.ts`
  - mock history endpoint 返回完整 old/middle/latest 消息。
  - 打开 historyPath 深链。
  - 断言第一次 history 请求不带 `tail`、`limit`、`cursor`。
  - 断言 old/middle/latest 消息立即展示，`Load more` 不出现。
  - mock `timeline_changed` 增量事件后，断言新增过程消息追加展示，旧消息仍保留。
- `agent-chat-process-log.spec.ts`
  - mock 一轮包含 assistant delta、tool_call、tool_result 和 final assistant message 的 timeline。
  - 断言完成轮次折叠时只展示最终回答和处理耗时。
  - 展开轮次后断言过程文本按顺序显示，命令组摘要折叠展示，展开命令组后可见命令名、Input 和 Output。
- `ai-layout-tools-settings.spec.ts`
  - 点击 ASR，断言 ASR 工作台渲染并保留 `asrTab`。
  - 点击 IM，断言 IM 工作台渲染并保留 `imGatewaySection`。
  - 点击 Settings，断言右侧 Settings 内容页打开；顶部只显示 `Agent`、`Runner`、`IM`；Agent 分组平铺 General、Model、Runtime、MCP Servers 等卡片，Runner 分组只展示 runners 卡片，IM 分组平铺 Targets、Routes、Schedules、History 等卡片且不展示 Connections；切回其它主入口后对应主内容恢复。
  - 断言 Settings 内容轨道宽度不超过约 1120px，并在右侧主内容区内水平居中，不把配置卡片撑满全宽。
  - 打开带 `session` 和 `agentSection=chat` 的 Settings 脏链接，断言 URL 清理会话参数，Settings 顶部只显示 `Agent`、`Runner`、`IM`，不显示 Chat、Back、Session Detail 或 Messages。
- `ai-layout-videos-compat.spec.ts`
  - 打开旧 `aiSection=tools-videos` 链接。
  - 断言 Videos Tool 仍可访问。
  - 断言 YouTube URL、下载目录和进度展示入口仍存在。
- `ai-layout-legacy-links.spec.ts`
  - 打开旧 Agent Chat 链接。
  - 打开旧 Agent Model 链接。
  - 打开旧 ASR 链接。
  - 打开旧 Videos Tool 链接。
  - 打开旧 IM Gateway Routes 链接。
  - 断言全部被映射到新 shell。
- `ai-layout-responsive.spec.ts`
  - desktop、tablet、mobile viewport 下检查无重叠、无横向溢出、主要按钮可点击。

### Shell E2E

如果本次实现只改 Web UI 容器层，不需要新增 shell E2E。若后续为 Runner default 新增 Admin API 字段或改变 `/api/im-gateway/chat/config` 结构，需要补 shell E2E 验证：

- chat config 返回 runner label、adapter、enabled 状态。
- disabled runner 不会作为默认 runner。
- Codex Runner 缺失时返回稳定 fallback。

### human_tests

新增 `human_tests/webui-ai-layout-redesign.md`，覆盖：

- 默认新建对话居中输入。
- Runner 默认 Codex 与切换。
- 首条消息创建线程。
- 历史线程切换。
- ASR / IM 入口切换。
- Settings 二级内容页的 `Agent` / `Runner` / `IM` 分组和配置卡片平铺。
- 旧深链兼容。
- 窄屏布局。

## 验收标准

- 用户进入 `/ai` 可以不进入任何配置页，直接输入任务并启动新对话。
- 默认 Runner 是 Codex Runner；不可用时 UI 明确展示实际 fallback runner。
- 左侧导航只表达工作路径：New Chat、ASR、Videos、IM、Threads、Settings。
- 左侧线程列表宽度和选中态稳定，不因点击选中产生列表抖动；右侧对话区域没有未使用的内部 thread rail 空白。
- 运行中对话的排队消息区域应保持紧凑：输入框上方最多展示两行队列消息高度，更多消息在该区域内部滚动；每条消息右侧必须预留操作按钮空间，删除按钮不能被长文本挤到下一行。
- 打开历史线程时完整历史必须立即展示；默认请求不带 `tail` / `limit`，实时推送使用增量追加和全量恢复去重，不能再用最后一页覆盖当前消息。
- 每一轮执行过程采用可读过程文本 + 命令组摘要样式；完成轮次默认只展示最终回答，展开后可查看中间过程和命令 Input/Output。
- 配置项不再占据 AI 页面一级导航。
- Settings 只展示配置项，且顶层只合并为 `Agent`、`Runner`、`IM` 三个 tab，配置项在对应 tab 内以卡片向下平铺；Chat 和会话状态信息不进入 Settings；Connections Provider 配置不在 Settings > IM 中重复展示。
- IM 工作入口使用响应式卡片网格展示连接通道，桌面下自动多列，窄屏下收敛为单列；Settings > IM 保留 Targets、Routes、Schedules、History 等表格型配置，整体仍在同一内容轨道内。
- ASR、IM、Videos、历史消息线程和 Settings 各分组共享 AI 右侧内容区的居中规则；ASR/IM/Videos 工作台页桌面最大宽度约 920px，Settings 配置页最大宽度约 1120px，顶部留白统一为约 24px，避免内容吸顶。
- 旧深链不失效。
- Playwright、human_tests 和必要单元测试全部通过。

## 残余风险

- `AgentChatSection` 当前状态集中，线程外置会触动 session、history、SSE 和 URL 同步逻辑；实现时应优先抽 route/runner/thread helper，再移动 UI。
- IM 工作入口和 IM 配置页边界还需要产品确认。第一版可复用 `ImGatewayTab`，但长期应拆出更轻的 IM Channel 工作面板。
- Codex Runner 默认依赖后端 runner 配置。若现有环境没有 Codex runner，必须避免假默认。
- Settings 内容页承载大量配置项，移动端需要专门验证滚动、切回主入口、焦点和表单宽度。
