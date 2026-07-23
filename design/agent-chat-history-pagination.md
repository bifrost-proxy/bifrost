# Agent Chat 单会话日志与全量历史渲染

## 背景

Agent Chat 的会话历史以 append-only JSONL 保存。后端曾提供 `tail`、`cursor`、`since` 分页语义，WebUI 也曾显示 `Load older`。产品交互现已明确收敛：用户打开会话时必须默认看到全部对话记录，不出现 loader、`Load older` 或 modal 式历史补载；运行中的会话仍可用 `since` 仅拉新增事件。

持久化契约进一步收敛为：同一个 `session_key` 在任意时刻只能对应一个 JSONL，规范路径由 `sha256(session_key)` 唯一确定，不能把每次提问、每次 Runner 进程或每次内存 Session 重建拆成新文件。不兼容旧的日期/时间戳分片；服务启动时直接删除所有不符合规范路径或内容约束的 JSONL，之后所有写入器都必须通过“打开规范文件，否则新建”的统一入口取得 recorder。

真实长会话还暴露了第二类问题：外部 Runner 会逐 token 输出 `assistant_delta`，并周期性发送 `token_usage` / `rate_limits` 刷新。若每条 delta 都映射成独立过程步骤，中文会逐字换行、DOM 节点数量与 token 数量线性增长，内部 usage 元事件也会泄露到用户界面。

## 用户目标验证清单

### 必须实现

- WebUI 打开历史会话时使用无分页参数的 history detail 请求，一次加载全部事件。
- 删除 WebUI 的 `Load older` 按钮、顶部滚动自动加载、分页 loading 状态和旧页 cursor 状态。
- 运行中会话保留 `since=<end_index>` 增量同步，索引不连续时回退为一次全量刷新。
- 相邻 `assistant_delta` 合并为同一 thinking 段落；工具、计划或其他过程步骤形成明确边界，不能跨边界合并。
- 聚合后的 delta 若与随后落盘的最终回答等价，只保留最终回答，避免整段重复。
- `token_usage`、`rate_limits` 及等价的 `usage updated` 内部刷新不得渲染为 thinking 文本。
- 历史恢复与实时 SSE 使用同一套过滤和相邻 thinking 合并语义。
- 同一个 `session_key` 的所有轮次追加到唯一 JSONL；进程重启、Session TTL 过期、Runner 重建均不得产生第二个文件。
- 启动时删除旧版日期/时间戳分片、空文件、损坏 JSONL、混有多个 key 的文件和路径/key 不匹配的文件，不导入旧历史。
- 重复或乱序到达的 `timeline_changed` 增量响应只能忽略已见区间或追加新事件，不能用尾段覆盖完整历史。

### 必须不破坏

- 后端无参数 history API 继续返回完整事件；已有 `tail` / `cursor` 参数保留给兼容客户端和诊断脚本。
- 会话列表仍只返回摘要，不把完整 `events` 塞进列表接口。
- 最终回答、工具调用、计划胶囊、运行状态、时间戳与滚动到底部行为保持不变。
- 明暗主题继续使用 Ant Design token，不新增硬编码颜色。
- 用户明确执行清空后允许删除该 key 的唯一文件；下一轮对话再创建一个新文件。
- 不同原始 `session_key` 通过完整 SHA-256 路径隔离，不能因字符清洗碰撞共用文件。

### 必须真实验证

- 使用隔离数据目录中的规范 SHA-256 JSONL 长会话验证全部历史一次加载、无 `Load older`、usage 元事件不可见、逐 token 文本合并成段；旧格式用户数据不作为兼容输入。
- Vitest 覆盖 usage 过滤、delta 合并和工具边界。
- Playwright 覆盖无分页请求、即使响应携带旧 `has_more=true` 也不恢复旧按钮、长会话过程文本成段展示。
- human_tests 同时验证亮色和暗色主题。

## 产品语义

### 历史默认完整

Agent Chat 是会话审阅界面，不是无限信息流。历史完整性优先于渐进加载交互。后端读取 JSONL 已在 blocking worker 中执行；前端收到完整事件后应先归一化，再渲染为数量受过程语义约束的消息和步骤节点。

### Delta 是文本片段，不是步骤

`assistant_delta` 表示同一段过程文本的增量片段。只有工具调用、计划更新、压缩、显式状态边界或新的用户轮次才会切断段落。字符、词或短句 delta 都不能各自占一个步骤。

### Usage 是遥测，不是对话

`token_usage` 与 `rate_limits` 更新只服务 token HUD、限额或状态统计，不属于模型对用户可见的思考过程。前端在实时事件映射与历史恢复两条路径都必须过滤这些机器状态。

## 技术方案

### 单会话单文件持久化

- `canonical_conversation_path(data_dir, session_key)` 固定返回 `sessions/by-key/session-{sha256}.jsonl`；文件名不包含轮次或时间戳。
- `ConversationRecorder::open_or_create` 是生产路径唯一建档入口：规范文件存在且每一条事件都属于同 key 时继续追加；文件为空、损坏或混入其他 key 时先删除，再创建干净文件。
- `clean_noncanonical_conversations` 在网关服务初始化、尚无活跃写入器时扫描 `agent/sessions`。只有内容可严格解析、所有事件 key 一致且路径等于该 key 规范路径的文件保留，其余 JSONL 直接删除。
- `sessions/**/attachments/` 是附件存储边界，不参与会话 JSONL 扫描；即使附件扩展名为 `.jsonl` 也不得删除或展示为会话。
- `session_state.history_path` 若仍指向已删除的旧格式路径，恢复逻辑失败后按 `session_key` 回退到规范路径；没有规范文件即视为没有历史。

### WebUI 历史加载

- `fetchHistoryPage(historyPath)`：首屏和强制刷新均不带 `tail`、`cursor`、`limit`。
- `fetchHistoryPage(historyPath, { since })`：仅用于运行中的增量同步。
- 删除 `historyHasOlder`、`historyLoadingOlder`、`historyOlderCursorRef`、`loadOlderHistoryPage` 及 `agent-chat-load-older`。
- 增量合并后保留最初 `start_index`，更新 `end_index`；出现索引断层时重新全量加载。
- 增量窗口按当前实时 `end_index` 计算重叠：完全重复的响应丢弃，部分重叠只追加未见后缀，真正断层才全量刷新。并发请求返回顺序不再影响已渲染历史。

### 过程事件归一化

- `isReadableProgressStatus()` 过滤内部 usage refresh 的稳定变体。
- `appendProcessStepToTimeline()` 负责过程步骤追加：相邻 thinking 合并 `summary`，保留第一片段的 `startedAt`；非 thinking 直接形成边界。
- 写入 `assistant_message` 前比较聚合 thinking 与最终回答的规范化文本，删除等价的重复过程段。
- 历史 `historyEventsToMessages()` 与实时 `AgentChatSection` 都调用同一 helper，避免实时正确但刷新后回归，或反之。

## 测试方案

### 单元测试

- 相邻中文 delta 合并为 `你说得对。`。
- 工具步骤前后的 thinking 不跨边界合并。
- `token_usage: token usage updated`、`rate_limits: usage updated` 在历史和实时映射中均返回空/不可见。
- Rust 覆盖首次建档复用、旧分片删除、损坏/混 key 文件删除、哈希路径隔离和后续追加。
- Vitest 覆盖重复窗口与部分重叠窗口不会缩短历史。

### E2E / UI

- `web/tests/ui/agent-chat.spec.ts` 使用完整历史响应，断言请求无 `tail` / `cursor` / `limit`。
- 响应故意携带旧 `has_more=true` 与 `next_cursor`，断言页面仍不存在 `agent-chat-load-older`。
- 断言完整旧消息、最新消息与合并后的过程段落同时可见，usage 文本不可见。
- 连续发出相同 `since` 的 SSE 更新并让响应乱序返回，断言首轮消息始终存在且只追加最新事件。

### 真实场景

- 更新 `human_tests/agent-chat-history-pagination.md` 为全量历史与过程渲染回归，创建/更新后立即执行。
- 真实 9900 页面分别验证亮色和暗色主题；不修改用户会话数据。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核全量加载、增量 since、usage 过滤、delta 合并与工具边界。
- 执行 `git status --short`、`git diff`、聚焦 Vitest 与 Playwright。

### 第 2 轮

- 基于最新 diff 复核长历史性能、明暗主题、最终回答/计划/工具未回归、文档和 human_tests 对齐。
- 复跑聚焦测试与真实指定会话。

## 风险与边界

- 长会话网络响应会增大，但这是明确的产品完整性选择；通过 delta 合并和内部事件过滤控制 DOM 规模。
- 升级到新格式会主动丢弃全部旧日期/时间戳 JSONL；这是明确的数据边界，不提供旧历史导入或拼接兼容。
- 删除失败时服务继续启动并输出具体路径告警；该文件不会被 `open_or_create` 当成规范数据源。
- 后端分页 API 暂不删除，以免破坏 CLI 或外部脚本；WebUI 不再消费旧页分页语义。
- 本机不运行高成本 coverage；90% 覆盖率由远端 `coverage-all.sh --json --gate` 兜底。
