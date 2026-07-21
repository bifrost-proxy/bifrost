# Agent Chat 单会话日志与全量历史渲染真实场景测试

## 功能模块说明

验证一个 `session_key` 只使用 `sessions/by-key/session-{sha256}.jsonl` 这一份规范日志，旧日期/时间戳分片和损坏数据直接删除；Agent Chat 默认一次加载规范文件内的完整对话历史，并确保并发增量响应不会用尾段覆盖首轮消息。

## 前置条件

1. 在仓库根目录执行命令。
2. WebUI 自动化使用 Playwright 的独立测试服务；不得使用或修改 9900 下旧格式的用户历史。
3. 将待验证的规范 JSONL 绝对路径设为 `HISTORY_PATH`，并确认路径位于 `sessions/by-key/` 且文件名为完整 SHA-256。
4. 涉及独立 Bifrost 进程时必须使用临时 `BIFROST_DATA_DIR`、非 9900 端口和 `--no-system-proxy`。

## 测试用例列表

### TC-ACH-01 无参数 history API 返回完整事件

操作步骤：

1. URL encode `HISTORY_PATH`。
2. 请求 `GET /_bifrost/api/im-gateway/agent/sessions/history/{encoded_history_path}`，不携带 `tail`、`cursor` 或 `limit`。
3. 对比 JSONL 行数与响应 `events.length` / `total_count`。

预期结果：

- 请求在可接受时间内成功。
- `events.length == total_count == JSONL` 当前有效事件数。
- `start_index=0`、`end_index=total_count`、`has_more=false`、`next_cursor=null`。

### TC-ACH-02 WebUI 首屏完整加载且永不显示旧页 loader

操作步骤：

1. 打开带 `session` 与 `historyPath` 的 Agent Chat 深链。
2. 检查 history detail 请求 query。
3. 检查页面 DOM 中 `agent-chat-load-older`。
4. 运行 Playwright 用例；fixture 故意返回旧的 `has_more=true` 与 `next_cursor`。

预期结果：

- 首次请求不带 `tail`、`cursor`、`limit`。
- 最旧与最新消息均在同一次响应后可见。
- 即使响应含旧分页元数据，页面也不存在 `Load older`、分页 loading 或 modal。

### TC-ACH-03 逐 token delta 合并且 usage 元事件不可见

操作步骤：

1. 使用包含逐字中文 `assistant_delta`、`token_usage: token usage updated`、`rate_limits: usage updated`、工具调用和最终回答的事件序列恢复历史。
2. 检查 `agent-chat-process-text` 节点与消息区文本。

预期结果：

- 相邻字符显示为完整句子，不是一字一个过程节点。
- 工具步骤仍形成边界，工具前后过程内容顺序正确。
- 消息区不包含 `token_usage`、`token usage updated` 或 `rate_limits`。
- 聚合 delta 若与最终回答全文相同，只显示一次最终回答；不同的真实思考过程仍保留。
- 最终回答仍完整显示。

### TC-ACH-04 完整 timeline 不与有噪音的 detail fallback 混合

操作步骤：

1. 让 session detail 返回包含 usage 文本的旧消息投影。
2. 让 history detail 返回完整、可归一化的 timeline。
3. 打开同一会话并检查消息区。

预期结果：

- history timeline 请求成功时只使用 timeline 渲染结果。
- detail 中的 usage 噪音和逐 token 消息不会被重新合并回页面。
- timeline 请求失败时 detail 仍可作为基础聊天内容兜底。

### TC-ACH-05 真实长会话 DOM 收敛

操作步骤：

1. 使用包含多轮消息的规范路径长 JSONL，在隔离测试服务打开本地开发页面。
2. 统计 `agent-chat-process-text` 节点数、单字符节点数和最长段落长度。
3. 检查报告中的中文过程句是否作为单个合并节点出现。

预期结果：

- 页面加载全部历史且无 `Load older`。
- 单字符过程节点数为 0。
- 报告中的连续中文过程句在一个 process text 节点中出现。
- usage 元事件不可见。

### TC-ACH-06 亮色与暗色主题一致

操作步骤：

1. 在亮色主题执行 TC-ACH-05 的 DOM 断言。
2. 点击 `theme-toggle` 切换暗色主题。
3. 再次执行相同 DOM 断言并检查页面背景切换。

预期结果：

- 两种主题均无 loader、usage 噪音和逐字断行。
- 暗色主题背景生效，文本仍清晰可见，布局与节点数量不发生语义变化。

### TC-ACH-07 新 usage refresh 不再写入 timeline

操作步骤：

1. 构造 `ExternalCliProgressEventType::Status`，内容分别为 `token usage updated` 与 `usage updated`，标题分别为 `token_usage` 与 `rate_limits`。
2. 调用 timeline recorder 后读取 JSONL。
3. 同时写入一条普通可读状态和 assistant 内容作为对照。

预期结果：

- usage refresh 不生成 `assistant_delta`。
- 普通可读状态、assistant 内容和工具记录仍正常持久化。

### TC-ACH-08 单 key 单文件与非规范数据清理回归

操作步骤：

1. 在临时 `BIFROST_DATA_DIR/agent/sessions/YYYY/MM/DD/` 写入同一 key 的旧时间戳 JSONL。
2. 在 `agent/sessions/by-key/` 写入该 key 的规范 SHA-256 JSONL，并另建空文件、损坏 JSONL、混合两个 key 的文件；同时在 `sessions/by-key/attachments/.../input.jsonl` 放置一个普通附件。
3. 启动隔离 Bifrost，再统计 `sessions` 下剩余 JSONL 并请求 session/history API。
4. 连续两次通过 `ConversationRecorder::open_or_create` 写入同一 key，比较两个 recorder 路径和最终事件顺序。

预期结果：

- 旧时间戳、空、损坏、混 key、路径/key 不匹配的 JSONL 全部删除，不做合并或导入。
- 合法规范文件保留，history API 只返回该文件已有事件，不包含旧分片内容。
- `attachments/` 下的 `.jsonl` 附件保持原样，既不被删除，也不进入 session/history API。
- 两次 `open_or_create` 路径完全相同，第二次 `created=false`，最终目录只有一个 JSONL 且两轮按写入顺序存在。

## 清理步骤

1. 停止本次启动的 Vite / 临时 Bifrost 进程。
2. 删除临时 `BIFROST_DATA_DIR`、Playwright 产物和临时响应文件。
3. 不删除或修改用户原始 JSONL 会话。

## 本次执行记录

- 通过。2026-07-21 执行 TC-ACH-01、TC-ACH-08：`SKIP_BUILD=true BIFROST_BIN=target/debug/bifrost bash e2e-tests/tests/test_agent_history_pagination_api.sh` 通过；隔离服务启动后旧日期分片已删除，会话扫描只保留 1 个规范 SHA-256 JSONL，7 个规范事件完整返回，`attachments/.../input.jsonl` 附件仍存在且未进入 history API。
- 通过。2026-07-21 执行 TC-ACH-08 单元边界：`cargo test -p bifrost-agent persistence::tests:: -- --nocapture` 为 41/41 通过；覆盖跨轮复用、旧分片丢弃、损坏/混 key 清理、`.jsonl` 附件保护及哈希碰撞隔离。
- 通过。2026-07-21 执行 TC-ACH-02 至 TC-ACH-06：聚焦 Playwright 的完整历史用例和重复并发 SSE 用例均通过；两份相同 `since=3` 响应乱序返回后，`Previous question`、`Previous answer` 与最新过程仍同时可见。
- 通过。2026-07-21 执行 TC-ACH-03 与 TC-ACH-04 单元边界：Web Vitest 38 文件、184 用例全通过，包含重复窗口忽略、部分重叠只追加未见后缀、真实断层识别。
- 通过。2026-07-21 执行 TC-ACH-07：`cargo test -p bifrost-admin external_runner_progress_events_are_recorded_as_visible_timeline_steps -- --nocapture` 通过。

- 通过。2026-07-15 执行 TC-ACH-01：用户指定的运行中 JSONL 当时为 12,155 条 / 6,456,544 bytes；无参数 API 在 0.045s 内返回全部 12,155 条，`start_index=0`、`end_index=12155`、`has_more=false`、`next_cursor=null`。
- 通过。2026-07-15 执行 TC-ACH-02 至 TC-ACH-04：`pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts -g "loads full history without restoring obsolete pagination controls"` 为 1/1 通过；所有 history 请求均无 `tail` / `cursor` / `limit`，fixture 即使返回 `has_more=true` 也无 `Load older`，展开完成回合后可见合并句且 usage 文本不可见。
- 通过。2026-07-15 执行 TC-ACH-03：第 2 轮复查后聚焦 Vitest 为 29/29 通过，覆盖相邻 delta 合并、累计快照去重、工具边界、历史/实时 usage 过滤，以及聚合 delta 与最终回答全文等价时的去重。
- 通过。2026-07-15 执行 TC-ACH-05：真实长会话完整载入后仅 19 个可见 process text 节点，单字符节点 0、重复长段落 0；目标中文过程句在单个节点中出现，`Load older` 为 0，usage 元事件不可见。
- 通过。2026-07-15 执行 TC-ACH-06：暗色背景 `rgb(20, 20, 20)` 与亮色主题均保持 `Load older=0`、usage 不可见、单字符节点 0、合并过程句可见。
- 通过。2026-07-15 执行 TC-ACH-07：`cargo test -p bifrost-admin external_runner_progress_events_are_recorded_as_visible_timeline_steps -- --nocapture` 为 1/1 通过；usage refresh 未写入 timeline，普通状态、assistant 内容和工具记录保持可见。
