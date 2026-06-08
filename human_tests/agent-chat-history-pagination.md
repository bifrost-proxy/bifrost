# Agent Chat History Pagination 真实场景测试

## 功能模块说明

验证 Agent Chat 页面不再打开即全量加载历史详情。会话列表只加载摘要；选中某个历史会话后，详情首屏只加载最新事件页；继续查看旧内容时再按需加载上一页；运行中的历史轮询只拉取新增事件。历史文件扫描和详情读取不能占用主 async worker；分页详情不能反序列化未展示的旧 JSONL 事件；WebUI 必须能连续加载旧页直到看到完整线程。

## 前置条件

1. 在仓库根目录执行命令前先运行 `source ~/.zshrc`。
2. 使用独立临时数据目录，避免污染本机 Bifrost 数据。
3. 启动服务时必须带 `--no-system-proxy`，除非测试目标是系统代理。
4. 测试用 history 文件至少包含 6 条 event，用于验证 tail、cursor、since。

## 测试用例列表

### TC-ACH-01 列表接口只返回摘要

操作步骤：

1. 启动测试 Bifrost 服务。
2. 请求 `GET /_bifrost/api/im-gateway/agent/sessions/all`。
3. 检查响应中的 session 条目。

预期结果：

- 响应包含 `session_key`、`title`、`history_path`、`timeline_event_count` 等摘要字段。
- 响应不包含 `events` 数组。
- 响应不包含完整对话正文详情。

### TC-ACH-02 选中详情首屏只加载尾页

操作步骤：

1. 使用 TC-ACH-01 得到的 `history_path`。
2. 请求 `GET /_bifrost/api/im-gateway/agent/sessions/history/{history_path}?tail=true&limit=2`。
3. 检查分页元数据与事件数量。

预期结果：

- `count` 为 `2`。
- `total_count` 大于 `count`。
- `start_index` 指向尾页起始下标。
- `has_more` 为 `true`。
- 响应只包含尾页事件，不包含整份 JSONL 的全部事件。

### TC-ACH-03 向上查看时加载旧页

操作步骤：

1. 读取 TC-ACH-02 响应中的 `next_cursor`。
2. 请求 `GET /_bifrost/api/im-gateway/agent/sessions/history/{history_path}?cursor={next_cursor}&limit=2`。
3. 检查返回事件与尾页不同。

预期结果：

- `count` 为 `2`。
- 返回的是尾页之前的旧事件。
- `end_index` 等于上一次尾页的 `start_index`。
- 如果更早还有内容，`has_more` 继续为 `true`。

### TC-ACH-04 运行中轮询只加载新增事件

操作步骤：

1. 请求 `GET /_bifrost/api/im-gateway/agent/sessions/history/{history_path}?since=5`。
2. 检查响应事件数量和下标。

预期结果：

- 响应只包含下标 `5` 及之后的事件。
- `start_index` 为 `5`。
- `end_index` 等于当前 `total_count`。
- 不返回 `since` 之前的旧事件。

### TC-ACH-05 分页详情不反序列化未选中旧事件

操作步骤：

1. 构造一个 JSONL 文件，其中旧行包含无法解析成事件的坏数据，后两行为合法事件。
2. 调用分页读取逻辑请求 `tail=true&limit=2`。
3. 再调用无分页全量读取逻辑读取同一文件。

预期结果：

- `tail=true&limit=2` 成功返回最后两条合法事件。
- 分页响应的 `total_count`、`start_index`、`end_index`、`has_more` 正确。
- 无分页全量读取因旧坏行失败，证明分页读取没有反序列化未选中的旧行。

### TC-ACH-06 WebUI 可连续加载旧页直到完整线程可见

操作步骤：

1. 打开 Agent Chat history 深链，URL 带 `session`、`view=history` 和 `historyPath`。
2. 拦截 history detail API，首个响应只返回最新尾页并设置 `has_more=true`。
3. 点击消息区顶部的 `Load older` 按钮，返回中间旧页并继续设置 `has_more=true`。
4. 再次点击 `Load older`，返回最旧页并设置 `has_more=false`。
5. 检查消息区内容。

预期结果：

- 首屏请求包含 `tail=true` 和 `limit`，不包含 `cursor`。
- 首屏只显示最新消息，不显示最旧消息。
- 第一次加载后能看到中间页消息。
- 第二次加载后同时能看到最旧消息和最新消息。
- 旧页请求都使用 `cursor`，不会退化成无参数全量 history detail 请求。

### TC-ACH-07 Chat 线程列表限量扫描最新摘要

操作步骤：

1. 使用默认 `~/.bifrost` 数据目录启动 9900 服务。
2. 确认数据目录中存在多个同一 session key 的旧大 JSONL 历史文件。
3. 请求 `GET /_bifrost/api/im-gateway/agent/sessions/all?limit=80` 并记录耗时。
4. 请求 `GET /_bifrost/api/im-gateway/agent/sessions/all?limit=20`，检查旧的外部 runner `status=running` 但没有 `latest_run_id` / `history_path` 的会话投影。

预期结果：

- `limit=80` 列表接口只扫描每个 session key 最新 history 摘要，耗时应保持在百毫秒量级。
- 响应不包含完整 `events` 数组。
- 没有 `latest_run_id` 和 `history_path` 的旧外部 runner running 状态应投影为 `status=ended`、`running=false`、`run_state=completed`。

### TC-ACH-08 Chat 页面不触发 Traffic 历史回填

操作步骤：

1. 使用默认 `~/.bifrost` 数据目录启动 9900 服务。
2. 用浏览器打开 `/_bifrost/ai?aiSection=agent-chat&agentSection=chat`。
3. 记录页面发出的 `/_bifrost/api/*` 请求。
4. 采样 Bifrost 主进程 CPU 至少 5 秒。

预期结果：

- Chat 页面只请求 Agent Chat 必需接口和全局轻量状态接口。
- Chat 页面不应触发 `/traffic?cursor=...` 历史回填请求。
- 主进程 CPU 仅有短暂初始化波动，稳定后回落到低占用；峰值不应接近长期 100%。

## 清理步骤

1. 停止测试 Bifrost 进程。
2. 删除临时 `BIFROST_DATA_DIR`。
3. 删除测试期间生成的临时响应文件。

## 执行记录

- 通过。2026-05-29 执行 `bash e2e-tests/tests/test_agent_history_pagination_api.sh`，脚本使用独立 `BIFROST_DATA_DIR` 创建 7 条事件的 JSONL，启动测试 Bifrost 服务并逐条验证 TC-ACH-01 至 TC-ACH-04：`sessions/all` 只返回摘要且无 `events`；`tail=true&limit=2` 只返回尾页并带 `has_more=true`；`cursor=5&limit=2` 返回旧页；`since=6` 只返回新增事件。最终输出 `agent history pagination API checks passed`。
- 通过。2026-06-07 执行 `SKIP_BUILD=true BIFROST_BIN="$PWD/target/debug/bifrost" bash e2e-tests/tests/test_agent_history_pagination_api.sh`，复用最新 debug 二进制和独立临时数据目录验证 TC-ACH-01 至 TC-ACH-04，最终输出 `agent history pagination API checks passed`。
- 通过。2026-06-07 执行 `cargo test -p bifrost-agent test_load_conversation_events_page -- --nocapture`，验证 TC-ACH-02 至 TC-ACH-05：分页 tail/cursor/since 语义正确，且 `tail=true&limit=2` 在旧行包含坏 JSONL 时仍成功返回最后两条合法事件，无分页全量读取同一文件失败，证明分页详情没有反序列化未选中的旧事件。
- 通过。2026-06-07 执行 `pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts -g "loads history detail progressively"`，验证 TC-ACH-06：WebUI history 深链首屏只请求 `tail=true`，连续两次点击 `Load older` 分别用 `cursor=3` 和 `cursor=2` 加载旧页，最终同屏可见最旧消息和最新消息，未发生无参数全量 history detail 请求。
- 通过。2026-06-08 使用默认 `~/.bifrost` 数据目录启动 9900 服务，执行 `GET /_bifrost/api/im-gateway/agent/sessions/all?limit=80` 验证 TC-ACH-07：接口耗时约 `0.06s`，响应为 4 条摘要列表；旧 `admin-chat-1779726991205` ChatGPT Web running 遗留状态返回 `status=ended`、`running=false`、`run_state=completed`。
- 通过。2026-06-08 使用 Playwright 打开默认 9900 的 `/_bifrost/ai?aiSection=agent-chat&agentSection=chat` 验证 TC-ACH-08：请求列表中 `trafficBackfillCount=0`，`sessions/all?limit=80` 请求只带 `?limit=80`，页面不显示无效 `Run state:` 状态行，Bifrost 主进程 CPU 采样峰值 `28.4%`、均值 `3.61%`，未出现持续高 CPU。
- 通过。2026-06-08 执行 `pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts tests/ui/agent-chat-threads.spec.ts --reporter=line`：43 个 Agent Chat UI 用例全部通过，覆盖线程列表、折叠状态、running/terminal SSE、外部 runner 排队/停止/错误展示、跨线程隔离和历史恢复。
