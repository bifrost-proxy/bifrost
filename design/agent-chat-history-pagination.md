# Agent Chat History Pagination

## 背景

Agent Chat 的会话历史以 JSONL append-only 事件流形式落盘（`ConversationRecorder`）。早期 WebUI Agent Chat History 打开一个 session 时，一次性 `GET` 全量事件并整体渲染，随着单个 session 越写越长，出现以下问题：

- 首屏时间线性增长，长会话首屏加载几百 KB 甚至几 MB JSON。
- 前端一次性反序列化并 mount 上千条 timeline items，首屏帧率抖动。
- 后端读取 JSONL 时把整个文件解析成事件数组，占用 Tokio async worker、拖慢主代理链路。
- 运行中的会话轮询也每次请求全量事件，浪费网络与解析成本。
- 会话列表接口 `/agent/sessions/all` 顺带返回详情字段时，浏览器打开设置页要拉几十 MB。

本模块把 Agent Chat 历史读取拆成 `sessions/all`（列表摘要）与 `sessions/history/{path}`（详情分页），使详情按需分页、运行中轮询按增量拉取，同时保持无参数全量读取的历史兼容行为。

## 用户目标验证清单

### 必须实现

- `GET /api/im-gateway/agent/sessions/all` 只返回摘要字段（session key、work_dir、title、last_ts、total_count、running 状态等），不携带 events。
- `GET /api/im-gateway/agent/sessions/history/{path}` 支持三种分页语义：
  - `tail=true&limit=N`：返回最新 N 条事件。
  - `cursor=K&limit=N`：把 `K` 作为 end-exclusive cursor，返回 `[K-N, K)` 的旧事件页。
  - `since=K`：返回 `[K, total_count)` 的增量事件，供 running 会话轮询。
- 分页响应统一带 `start_index`、`end_index`、`next_cursor`、`has_more`、`total_count`。
- WebUI 首屏只请求 `tail=true&limit=HISTORY_EVENT_PAGE_SIZE`，向上滚动或点击「加载更多」触发 `cursor` 请求上一页。
- 运行中 timeline 轮询按 `since=<last_end_index>` 请求增量事件，不重复拉尾页。
- 后端 JSONL 读取放入 `spawn_blocking`，不阻塞代理主线程。
- 分页读取只对被选中窗口的 JSONL 行做完整反序列化，未选中的旧行只做行计数。
- 保留无参数 `GET .../history/{path}` 的兼容行为：仍全量返回，供旧客户端与脚本使用。

### 必须不破坏

- 现有 `/agent/sessions/all` 消费方（Agent 设置页、Session Detail 页、IM 卡片 session 概览）继续可用。
- 已存在的 JSONL 文件格式不变，`ConversationRecorder` 追加语义不变。
- Long task preview、会话删除、Session Detail 等相邻改动不因分页字段扩展而回归。
- CLI `bifrost agent session` 视图（如果通过同一 admin API 读取）在无参数路径继续拿到完整历史。

### 必须真实验证

- Rust 单元测试覆盖 tail / cursor / since 三种分页语义，包括边界（`limit=0`、`cursor` 越界、`since` 超过 `total_count`）。
- Rust 单元测试证明未选中行不会被完整反序列化。
- Shell E2E 用真实 JSONL 文件走完整 HTTP 路径，覆盖 summary / list / tail / cursor / since 四种响应。
- WebUI Playwright 用例验证首屏只发一个 tail 请求，向上滚动多次后能拼出完整线程。
- human_tests 覆盖“真人操作 Agent Chat 打开长会话并连续加载”场景。

## 产品语义

### 列表与详情双入口

- `sessions/all`：session 摘要列表，用于 Agent Chat 会话选择器、Settings > Agent > Sessions 列表以及 IM 卡片。摘要不含 events。
- `sessions/history/{path}`：详情接口。`path` 是 URL-encoded session key（通常是 JSONL 相对路径）。默认返回完整历史；追加分页参数后返回窗口切片。

### 分页语义

- `tail=true&limit=N`：首屏。返回 `[max(0, total_count - N), total_count)`。
- `cursor=K&limit=N`：向上翻页。返回 `[max(0, K - N), K)`，`next_cursor` 为窗口起点。`K=0` 意味着已到达文件头，`has_more=false`。
- `since=K`：增量轮询。返回 `[K, total_count)`。用于运行中的 session 追加事件。
- 未提供任何参数：兼容路径，返回全量事件。

### 前端窗口维护

- 首屏 fetch tail，把窗口写入 `visibleEvents` 状态。
- 向上滚动到阈值：以当前 `visibleEvents[0].event_index` 为 `cursor` 请求上一页，prepend 到窗口。使用 ref 级 in-flight 标记防止 React state 尚未更新时重复触发。
- 恢复滚动位置：prepend 后按 prepend 前后 `scrollHeight` 差补偿 `scrollTop`。
- 运行中轮询：每 tick 以 `visibleEvents.at(-1).event_index + 1` 作为 `since` 请求增量。若 `total_count > end_index`，追加新事件。
- 切换 session：清空窗口并重新 tail。

## 技术细节

### 后端

- 关键文件：
  - `crates/agent/src/persistence.rs`：JSONL 追加与分页读取基础能力。新增（或已存在的）`load_conversation_events_page(path, spec)` 支持 `tail` / `cursor` / `since` spec，并只对选中行调用 `serde_json::from_str`。
  - `crates/bifrost-admin/src/handlers/im_gateway/agent_api.rs`：解析 query params，调用 persistence，把 `spawn_blocking` 结果封装成响应。
- 响应结构（示例）：

```json
{
  "events": [ ... ],
  "start_index": 1200,
  "end_index": 1500,
  "next_cursor": 1200,
  "has_more": true,
  "total_count": 1500
}
```

- 参数校验：`limit` 上限固定为一个安全常量（例如 500），超出裁剪并在响应中反映真实窗口。
- Query 冲突处理：`tail=true` 与 `cursor` 同时出现时优先 `tail`；`since` 与 `tail`/`cursor` 同时出现时返回 400。
- 所有 JSONL 读取通过 `tokio::task::spawn_blocking` 执行，避免占用 async worker。
- `sessions/all` 摘要字段：`session_key`、`work_dir`、`title`、`updated_at`、`created_at`、`total_events`、`running`、`runner_id` 等；显式排除 events。

### 前端

- 关键文件：
  - `web/src/pages/AI/AgentChatSection.tsx`：分页 fetch helper、`HISTORY_EVENT_PAGE_SIZE = 300`、tail/cursor/since 三条调用路径、in-flight ref 防重。
  - `web/src/pages/AI/AgentChatSection.timelinePolling.ts`：`isRunStateActive` / `isThreadActive` 辅助，用于判断是否继续轮询。
- 状态：`visibleEvents`、`windowStartIndex`、`windowEndIndex`、`totalCount`、`hasMoreOlder`、`isLoadingOlder`、`nextPollSince`。
- 滚动位置补偿逻辑集中在 `prependEvents(newEvents)`，保证用户视觉锚点不跳动。
- Long task preview 与 timeline polling 复用同一份 `visibleEvents`，polling 只追加新事件。

### Admin API 表格

| Method | Path | 语义 |
| --- | --- | --- |
| GET | `/api/im-gateway/agent/sessions/all` | 列表摘要 |
| GET | `/api/im-gateway/agent/sessions/history/{path}` | 详情，兼容全量或分页 |
| GET | `/api/im-gateway/agent/sessions/history/{path}?tail=true&limit=N` | 尾页 |
| GET | `/api/im-gateway/agent/sessions/history/{path}?cursor=K&limit=N` | 旧页 |
| GET | `/api/im-gateway/agent/sessions/history/{path}?since=K` | 增量 |

## CLI 交互

分页参数当前仅服务 WebUI。CLI 如果通过同一 admin API 读取，仍用无参数路径拿到完整历史。若后续新增 CLI 分页命令，应复用相同 query 语义（tail/cursor/since）。

## Web UI 交互

- Agent Chat 打开一个 session：右侧详情区先展示 skeleton，收到 tail 响应后渲染最近 N 条并把视图滚到最下方。
- 向上滚动到阈值：显示「加载更早内容...」loading indicator；请求成功后 prepend。
- 到达文件头：indicator 变为「已到最早」。
- Running session：底部持续追加，滚动条自动跟随最新事件除非用户手动向上滚动。
- 顶部展示 `total_events` 与当前窗口范围（可选调试信息）。

## Sync / 导入导出 / 分享边界

- 历史分页只作用于本地 JSONL 读取，不参与 rule/group sync。
- 导出 session：使用 `sessions/history/{path}` 无参数路径拿全量，保持导出完整。
- 分享/协作：不在本轮范围。

## 实现切分

### Phase 1：后端分页与摘要拆分

- `persistence` 新增分页读取 API，支持 tail/cursor/since。
- `agent_api` 解析 query params 并封装响应，含 `start_index`、`end_index`、`next_cursor`、`has_more`、`total_count`。
- `sessions/all` 剥离 events。
- 单元测试覆盖三种语义与懒解析。

### Phase 2：`spawn_blocking` 化

- 所有 JSONL 读取（`sessions/all`、`sessions/history`、分页 detail）迁移到 `spawn_blocking`。
- 增加对文件不存在、权限错误、损坏行的容错。

### Phase 3：WebUI 首屏与旧页加载

- `AgentChatSection.tsx` 首屏改为 tail 请求。
- 加载旧页交互与滚动补偿。
- in-flight ref 防重。

### Phase 4：轮询增量与联调

- Running session 轮询改为 `since` 增量。
- Long task preview / IM 卡片链路回归。
- 更新 human_tests 与 readme。

## 测试方案

### 单元测试

- `test_load_conversation_events_page_supports_tail_cursor_and_since`：验证 tail、cursor、since 三种分页语义和响应元数据。
- `test_load_conversation_events_page_does_not_parse_unselected_lines`：写入包含无效 JSON 的旧行，尾页请求成功，说明未选中行不会被 `serde_json::from_str`。
- `test_load_conversation_events_page_rejects_conflicting_params`：`since` 与 `tail`/`cursor` 同时出现返回 400。
- `test_sessions_all_omits_events`：`sessions/all` 响应中不含 `events` 字段。

### E2E 测试

- `e2e-tests/tests/test_agent_history_pagination_api.sh`：用独立 `BIFROST_DATA_DIR` 写入固定 JSONL，启动 Bifrost（非 9900 端口、`--no-system-proxy`、`BIFROST_DISABLE_TRAY=1`），依次验证：
  1. `sessions/all` 只返回摘要。
  2. `history/{path}` 无参数返回全量。
  3. `tail=true&limit=50` 只返回最后 50 条，元数据正确。
  4. `cursor=<start_index>&limit=50` 返回上一页，`has_more` 边界正确。
  5. `since=<end_index>` 追加事件后能拿到增量。

### WebUI 测试

- `web/tests/ui/agent-chat.spec.ts` 的 `AI Agent Chat loads history detail progressively`：验证首屏只发 tail 请求，向上滚动多次后能看到完整线程，滚动位置保持在锚点附近。
- 结合 `AgentChatSection.timelinePolling.ts` 辅助，验证 running session polling 使用 `since`。

### 真实场景测试 human_tests

- `human_tests/agent-chat-history-pagination.md`：
  - TC-CHP-01：列表摘要不携带 events。
  - TC-CHP-02：详情首屏只加载尾页。
  - TC-CHP-03：连续向上加载旧页，可回溯到文件头。
  - TC-CHP-04：running session 轮询按 `since` 追加。
  - TC-CHP-05：分页读取不解析未选中旧行（观察进程 CPU / 日志）。
  - TC-CHP-06：WebUI 多页加载后能看到完整线程。

所有 human_tests 使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1` 与 `--no-system-proxy`。

### 覆盖率与项目校验

- `pnpm --dir web exec tsc -b`
- `pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts -g "loads history detail progressively"`
- `cargo test -p bifrost-agent test_load_conversation_events_page_supports_tail_cursor_and_since`
- `cargo test -p bifrost-agent test_load_conversation_events_page_does_not_parse_unselected_lines`
- `cargo test -p bifrost-admin agent_history`
- `bash e2e-tests/tests/test_agent_history_pagination_api.sh`
- 收尾按项目规则执行 `rust-project-validate`，并至少执行一次 `cargo test --workspace --all-features`。
- 本机存在 no-local-coverage 约定时，不运行 `make coverage` / `make coverage-unit`；交付时说明豁免并依赖远端 CI。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：摘要/详情拆分、tail/cursor/since 三种分页、`spawn_blocking`、首屏与轮询降载。
- 复核 diff：persistence、admin handler、AgentChatSection、Playwright、human_tests、readme。
- 重点 review：query 参数校验、越界边界、in-flight ref 是否覆盖 React 双重渲染、无参数兼容路径是否仍全量、long task preview 未回归。
- 复测：targeted Rust 单测、agent_history admin 测试、E2E shell、Playwright 用例、human_tests。

### 第 2 轮

- 复核第 1 轮修复后的 diff、readme 索引、design 文档字段同步。
- 重点 review：分页元数据在真实浏览器上是否与后端一致；滚动补偿在快速连续加载时是否稳定；running polling 不重复解析已加载事件。
- 复测：失败路径重跑，必要时补充真实浏览器操作截图或日志。

## 风险与决策点

- 无参数兼容路径长期保留还是逐步收敛：本轮保留，避免破坏脚本消费。若后续 CLI/脚本迁移完成再考虑弃用。
- 分页 API 是否公开：当前仅服务 WebUI 内部渐进加载，参数不进入公开 API 文档。如果后续开放，需要补齐鉴权、限流与稳定字段说明。
- JSONL 损坏行的容错策略：分页读取遇到无法反序列化的行时应跳过并计入 warning，而不是整体失败。
- 未来若允许在 Agent Chat 内做全文搜索，需要新增倒排索引或数据库层，本轮不实现。
