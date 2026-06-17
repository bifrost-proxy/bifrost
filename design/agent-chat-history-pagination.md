# Agent Chat History Pagination

## 功能模块说明

Agent Chat 历史页需要从“打开时全量读取 JSONL 并全量渲染”调整为“列表只加载摘要，详情按需加载”。会话列表负责展示 session 摘要，不携带对话详情；用户选中某条会话后，详情接口先加载最近一页事件，向上滚动或点击加载更早内容时再请求上一页。运行中的会话轮询只请求已加载游标之后的新事件。

## 实现逻辑

- 后端 `GET /api/im-gateway/agent/sessions/history/{path}` 保留无参数全量返回的兼容行为。
- 后端 `sessions/all`、`sessions/history` 与 history detail 文件读取放入 `spawn_blocking`，避免在 Tokio async worker 上同步扫描和解析 JSONL，降低对代理主链路的影响。
- 新增查询参数：
  - `tail=true&limit=N`：返回最新 N 条事件，响应带 `start_index`、`end_index`、`next_cursor`、`has_more`、`total_count`。
  - `cursor=K&limit=N`：把 `K` 作为 end-exclusive cursor，返回 `[K-N, K)` 的旧事件页。
  - `since=K`：返回 `[K, total_count)` 的新增事件，用于 running 轮询。
- 分页读取在 `tail`、`cursor`、`since` 路径只解析被选中的事件行；尾页/旧页仍会顺序统计行数，但不会把未展示的旧 JSONL 全量反序列化成事件。
- 前端 `historyPath` 详情首屏只请求 `tail=true&limit=300`。
- 前端保留已加载事件窗口，加载旧页时 prepend 到窗口并保持滚动位置；运行中轮询按 `end_index` 使用 `since` 增量追加；旧页加载使用 ref 级防重，避免滚动事件在 React state 生效前重复触发同一页请求。
- 会话列表 `/agent/sessions/all` 继续只返回摘要字段；详情内容只通过选中的 history detail API 加载。

## 依赖项

- `crates/agent/src/persistence.rs`：提供分页读取 JSONL event 的基础能力。
- `crates/bifrost-admin/src/handlers/im_gateway/agent_api.rs`：解析分页查询参数并返回分页元数据。
- `web/src/pages/AI/AgentChatSection.tsx`：接入尾页、旧页和 `since` 增量轮询（分页 fetch helper、`HISTORY_EVENT_PAGE_SIZE = 300` 与轮询调用均在此文件内）。
- `web/src/pages/AI/AgentChatSection.timelinePolling.ts`：当前仅提供 `isRunStateActive` / `isThreadActive` 辅助；尚未承载分页/轮询逻辑（planned, not yet shipped as of 2026-06-16）。

## 测试方案

- 单元测试：`test_load_conversation_events_page_supports_tail_cursor_and_since` 验证 tail、cursor、since 三种分页语义；`test_load_conversation_events_page_does_not_parse_unselected_lines` 验证尾页分页不反序列化未选中的旧行。
- E2E 测试：`e2e-tests/tests/test_agent_history_pagination_api.sh` 用独立 `BIFROST_DATA_DIR` 生成测试 JSONL，验证 summary/list 不返回 events、tail 只返回尾页、cursor 返回旧页、since 返回增量。
- WebUI 测试：`tests/ui/agent-chat.spec.ts` 的 `AI Agent Chat loads history detail progressively` 验证首屏只请求 tail，并且连续加载旧页后能看到完整线程（planned, not yet shipped as of 2026-06-16；当前仓库无 `tests/ui/` 目录）。
- 真实场景测试：`human_tests/agent-chat-history-pagination.md` 覆盖列表摘要、详情尾页、旧页加载、运行中增量轮询、分页不解析未选中旧行、WebUI 多页加载完整历史。

## Review/Fix/Test 闭环方案

- 第 1 轮：复核用户目标、后端分页边界、前端首屏和轮询请求，运行 targeted Rust 单测、TypeScript 检查、API E2E 与 human_tests。
- 第 2 轮：复查第 1 轮修复后的 diff，确认兼容无参数全量行为、已有 long task preview 改动未被覆盖，复跑受影响测试。

## 校验要求

- `pnpm --dir web exec tsc -b`
- `pnpm --dir web exec playwright test tests/ui/agent-chat.spec.ts -g "loads history detail progressively"`（planned, not yet shipped as of 2026-06-16）
- `cargo test -p bifrost-agent test_load_conversation_events_page_supports_tail_cursor_and_since`
- `cargo test -p bifrost-admin agent_history`
- `bash e2e-tests/tests/test_agent_history_pagination_api.sh`
- 收尾阶段按项目规则执行 `rust-project-validate`，并至少执行一次 `cargo test --workspace --all-features`。

## 文档更新要求

- 更新 `human_tests/readme.md` 索引。
- 如后续把分页参数公开给外部用户，需要同步补充 API 文档；当前参数仅服务 WebUI 内部渐进加载。
