# Bifrost Memory 系统现状分析

> 生成时间：2026-05-02 16:40:37 CST（上次全量分析）
> 刷新时间：2026-06-16（针对当前实现重新对齐）
> 分支 / commit：codex/design-doc-refresh
> 分析范围：整仓库

## 0. TL;DR

- 自上次分析以来，仓库新增了**文件式长期记忆子系统** `crates/agent/src/memory/`（mod / layout / read_path / extract / consolidation / write / search / state_db / mcp_tools / pollution / retention / sub_agent / rollout_tracker / telemetry / usage_tracking / citation_consumer 等）。存储介质是 `$agent_home/memory/` 下的 `MEMORY.md`、`memory_summary.md`、`raw_memories.md`、`rollout_summaries/`、`skills/`、`extensions/`，由 `ensure_memory_layout()` 维护，证据见 `crates/agent/src/memory/layout.rs:L9-L31`、`crates/agent/src/memory/mod.rs:L1-L51`。
- `MemoriesConfig` 现已被实际消费：`use_memories` / `generate_memories` 由 `crates/agent/src/memory/read_path.rs:L17-L24` 解析，`extract_model` / `consolidation_model` / `max_*` 在 `crates/agent/src/memory/extract.rs:L173-L295`、`crates/agent/src/memory/consolidation.rs:L33-L381`、`crates/agent/src/memory/retention.rs:L25-L200` 被使用，配置定义位于 `crates/agent/src/config.rs:L403-L436`，Admin PATCH 透传见 `crates/bifrost-admin/src/handlers/im_gateway.rs` 的 `memories` 字段路径。
- 自动写入除原有会话事件流外，新增 `compaction` 事件持久化（`ConversationRecorder::record_compaction()`，证据 `crates/agent/src/persistence.rs:L298-L307`、`crates/agent/src/session/turn_loop.rs:L3284-L3310`）以及 phase-1 抽取 + phase-2 整合两阶段后台流程（`memory::auto_extract_after_turn_with_pollution_check_blocking()`、`memory::consolidation`），由 `crates/agent/src/session/turn_loop.rs:L2394` 在每轮 turn 结束后触发。
- 召回采用**读路径开发者消息注入**：`memory::recall_system_message()` 在 `crates/agent/src/session/turn_loop.rs:L1725` 被调用，把 `memory_summary.md` 截断后通过 `READ_PATH_TEMPLATE` 拼成 developer message，并通过 MCP 工具 `memory/list` / `memory/read` / `memory/search`（`crates/agent/src/memory/mcp_tools.rs`）让模型按需读取条目；仍未引入 embedding / 向量库 / topK rank 召回，关键词检索见 `crates/agent/src/memory/search.rs`。
- 用户显式入口已扩展：`/remember <text>` 与 `/forget <id|last>` 注册在 `crates/agent/src/slash.rs:L78-L194`；MCP 工具 `memory/*` 暴露 list / read / search 给模型；`HistoryConfig.max_bytes` 现已通过 `ConversationRecorder::enforce_max_bytes()` 强制裁剪（`crates/agent/src/persistence.rs:L426-L450`）。原有 session/history CRUD（active session 清空、JSONL list/read/delete、90 天清理）仍在 `crates/bifrost-admin/src/handlers/im_gateway.rs` 的 `/agent/sessions*` 路由。

## 1. 架构总览

### 1.1 代码分布

| 归属 | 关键文件 | 角色 | 证据 |
| --- | --- | --- | --- |
| Agent crate | `crates/agent/src/lib.rs` | 声明 Agent 运行时能力：tool calling、多轮对话、memory compaction、persistence | `crates/agent/src/lib.rs:L1-L13` |
| Agent 会话 | `crates/agent/src/session.rs` | 保存单 session 的 `history`、token、压缩状态、recorder，并执行 turn loop | `crates/agent/src/session.rs:L32-L69`、`crates/agent/src/session.rs:L425-L433` |
| 压缩 | `crates/agent/src/compact.rs` | 对长上下文做模型总结，替换 session history | `crates/agent/src/compact.rs:L80-L99`、`crates/agent/src/compact.rs:L111-L125` |
| 持久化 | `crates/agent/src/persistence.rs` | 将会话事件写入 `$agent_home/sessions/YYYY/MM/DD/session-*.jsonl`，并支持加载/扫描/清理 | `crates/agent/src/persistence.rs:L1-L4`、`crates/agent/src/persistence.rs:L44-L84` |
| 配置 | `crates/agent/src/config.rs` | 定义 `MemoriesConfig`、history 配置和 agent home 目录 | `crates/agent/src/config.rs:L403-L436`、`crates/agent/src/config.rs:L595`（`get_memories_config`）、`crates/agent/src/config.rs:L1013-L1027` |
| 长期记忆 | `crates/agent/src/memory/` | 文件式长期记忆：layout / read-path / extract / consolidation / write / search / mcp_tools | `crates/agent/src/memory/mod.rs:L1-L51`、`crates/agent/src/memory/layout.rs:L9-L31`、`crates/agent/src/memory/read_path.rs:L1-L75`、`crates/agent/src/memory/extract.rs:L30-L399`、`crates/agent/src/memory/consolidation.rs:L33-L381`、`crates/agent/src/memory/mcp_tools.rs` |
| Admin API | `crates/bifrost-admin/src/handlers/im_gateway.rs` | Feishu / API 入口、session history API、agent config PATCH（含 `memories`） | `crates/bifrost-admin/src/handlers/im_gateway.rs`（事件入口、`/agent/sessions*` 与 `memories` PATCH 段，行号随仓库演进） |
| WebUI | `web/src/pages/Settings/tabs/AgentTab.tsx`、`web/src/pages/Settings/tabs/agent/MemoriesSection.tsx` | Agent 设置页 Memories 子区域（`activeSection === "memories"`），消费 `config.memories` 字段 | `web/src/pages/Settings/tabs/AgentTab.tsx:L55`、`web/src/pages/Settings/tabs/AgentTab.tsx:L1178-L1240` |
| 测试 | `crates/agent/src/persistence.rs`、`crates/agent/src/compact.rs` / `compact_tests.rs`、`crates/agent/src/history.rs`、`crates/agent/src/memory/tests.rs`、`crates/agent/src/session/tests.rs`、`crates/bifrost-e2e/src/tests/im_gateway_agent.rs` | 单元 + E2E 覆盖 history / persistence / compaction / memory recall / `/remember` `/forget` / compaction 事件回归 | `crates/agent/src/persistence.rs::record_compaction_event_round_trip`、`crates/agent/src/session/tests.rs::test_record_compaction_event_includes_emergency_and_total_tokens`、`crates/agent/src/memory/tests.rs` |

结论：Memory 相关能力**已经**集中到 `crates/agent/src/memory/` 子模块（文件式存储 + read-path 注入 + MCP 工具 + phase-1/phase-2 抽取整合），不在独立 crate 中；由 `crates/agent/src/session/turn_loop.rs` 消费，IM Gateway 仅负责入口与 PATCH 透传。`bifrost-cli config memory` 仍是进程内存诊断（`crates/bifrost-cli/src/cli.rs:L2161-L2162`），不是长期记忆 CLI。

### 1.2 当前数据流

```text
Feishu WebSocket / POST /agent/chat
        |
        v
crates/bifrost-admin/src/handlers/im_gateway.rs
  - process_agent_chat / /agent/chat
  - 创建或复用 AgentSession
  - 可选创建 ConversationRecorder（含 max_bytes）
        |
        v
crates/agent/src/session/turn_loop.rs::run_turn_with_mcp
  - 内置命令：/clear /undo /compact /status /resume /remember /forget
  - pre-turn / mid-turn compact_session（含 record_compaction_event）
  - recall_system_message() 注入 memory_summary developer message
  - build_messages(system prompt + memory developer message + session.history)
  - model chat completion + tool loop（含 memory/list, memory/read, memory/search MCP 工具）
  - recorder 写 user/tool/assistant/compaction 事件
  - turn 结尾 auto_extract_after_turn_with_pollution_check_blocking() 异步 phase-1 抽取
        |
        +--> crates/agent/src/memory/
        |      extract.rs   -> raw_memories.md（phase-1）
        |      consolidation.rs -> MEMORY.md / memory_summary.md（phase-2）
        |      write.rs       -> remember_explicit / replace_memory / forget_memory
        |      retention.rs   -> max_unused_days / max_rollout_age_days 清理
        v
crates/agent/src/persistence.rs
  - JSONL: agent_home/sessions/YYYY/MM/DD/session-{session_key}-{timestamp}.jsonl
  - load_conversation / scan_session_summary / cleanup_expired_sessions / enforce_max_bytes
        |
        v
Admin/WebUI:
  - /agent/sessions, /agent/sessions/all
  - /agent/sessions/history, /agent/sessions/history/{path}
```

证据：IM 事件入口和默认 Agent chat 路径仍在 `crates/bifrost-admin/src/handlers/im_gateway.rs`；turn loop 步骤注释见 `crates/agent/src/session.rs:L1-L12`，`recall_system_message()` 调用点见 `crates/agent/src/session/turn_loop.rs:L1725`，phase-1 抽取触发见 `crates/agent/src/session/turn_loop.rs:L2394`，compaction event 持久化见 `crates/agent/src/session/turn_loop.rs:L3284-L3310`。

### 1.3 存储介质与 schema 摘要

当前持久存储分两层：

1. **会话事件 JSONL**（短期 / 重放）——路径模板写在代码注释和构造函数中：
2. **长期记忆文件树**（`$agent_home/memory/`，由 `memory::layout::ensure_memory_layout()` 维护）：`MEMORY.md`（已巩固的长期记忆）、`memory_summary.md`（注入到 developer message 的精简摘要）、`raw_memories.md`（phase-1 抽取产出的待巩固原料）、`rollout_summaries/`（按 rollout 切分的对话摘要）、`skills/`（含 `_memory` 子目录）、`extensions/`。仍**没有** SQLite/Postgres/向量库（`state_db.rs` 仅是租约/心跳的进程间协调，不是 memory 内容存储）。

```rust
// crates/agent/src/persistence.rs:L1-L4
//! Conversation persistence: recording and replaying conversation events.
//!
//! Events are stored in JSONL files organized by date and session key:
//! `{data_dir}/sessions/YYYY/MM/DD/session-{session_key}-{timestamp}.jsonl`
```

核心事件 schema：

```rust
// crates/agent/src/persistence.rs:L18-L38
pub struct ConversationEvent {
    pub timestamp: u64,
    pub event_type: String,
    pub session_key: String,
    pub content: serde_json::Value,
}

pub mod event_types {
    pub const USER_MESSAGE: &str = "user_message";
    pub const ASSISTANT_MESSAGE: &str = "assistant_message";
    pub const TOOL_CALL: &str = "tool_call";
    pub const TOOL_RESULT: &str = "tool_result";
    pub const COMPACTION: &str = "compaction";
    pub const SESSION_START: &str = "session_start";
    pub const SESSION_END: &str = "session_end";
    pub const MCP_TOOLS_LOADED: &str = "mcp_tools_loaded";
    pub const SKILLS_LOADED: &str = "skills_loaded";
}
```

事件类型清单较旧版本扩展：除原有 user/assistant/tool_call/tool_result/session_start/session_end/mcp_tools_loaded/skills_loaded/compaction 外，新增 `ASSISTANT_DELTA`、`TITLE_UPDATED`、`GOAL_UPDATED` / `GOAL_CLEARED`、`PLAN_UPDATED` / `PLAN_CLEARED`、`PROPOSED_PLAN`、`RUN_STATE_CHANGED`，证据见 `crates/agent/src/persistence.rs:L29-L48`。

注意（**已修复**）：`compact_session()` 之外，turn loop 现在通过 `record_compaction_event()` 把每次压缩（pre-turn / mid-turn / 手动 `/compact`）都写入 JSONL，证据见 `crates/agent/src/session/turn_loop.rs:L3284-L3310`、`crates/agent/src/session/tests.rs::test_record_compaction_event_includes_emergency_and_total_tokens`、`crates/agent/src/persistence.rs:L298-L307`。`scan_session_summary()` 识别 compaction 事件的逻辑也保留下来。

## 2. 数据模型

### 2.1 会话内存单元：`AgentSession`

| 字段 | 类型 | 含义 | 证据 |
| --- | --- | --- | --- |
| `history` | `Vec<ChatMessage>` | 单 session 对话历史；system prompt 不存入 history，而是在请求时 prepend | `crates/agent/src/session.rs:L32-L35` |
| `session_key` | `String` | 会话隔离键，例如用户 ID / chat ID | `crates/agent/src/session.rs:L37-L38` |
| `created_at` | `u64` | 创建时间，秒级 UNIX timestamp | `crates/agent/src/session.rs:L40-L41` |
| `last_active_at` | `u64` | 最后活跃时间，用于 TTL 过期 | `crates/agent/src/session.rs:L43-L44` |
| `compaction_count` | `u32` | 压缩次数 | `crates/agent/src/session.rs:L46-L47` |
| `total_tokens_used` | `Option<u64>` | API 返回的累计 token 用量 | `crates/agent/src/session.rs:L49-L51` |
| `last_response_tokens` | `Option<u64>` | 最近一次响应 token，用于 mid-turn 预算检查 | `crates/agent/src/session.rs:L53-L54` |
| `history_version` | `u64` | history 被 compaction / rollback / clear 改写时递增 | `crates/agent/src/session.rs:L56-L58` |
| `work_dir` | `Option<String>` | session 工作目录 | `crates/agent/src/session.rs:L60-L61` |
| `source` | `String` | session 来源，例如 `feishu` / `api` / `unknown` | `crates/agent/src/session.rs:L63-L64` |
| `recorder` | `Option<ConversationRecorder>` | 可选 JSONL recorder，跨 turn 复用 | `crates/agent/src/session.rs:L66-L69` |

`ChatMessage` 是 OpenAI-compatible message，不是长期记忆记录：

```rust
// crates/agent/src/types.rs:L5-L21
pub struct ChatMessage {
    pub role: String,
    pub content: Option<String>,
    pub tool_calls: Option<Vec<ToolCallMessage>>,
    pub tool_call_id: Option<String>,
    pub name: Option<String>,
}
```

### 2.2 持久化事件单元：`ConversationEvent`

| 字段 | 类型 | 含义 | 证据 |
| --- | --- | --- | --- |
| `timestamp` | `u64` | 事件发生时间，秒级 UNIX timestamp | `crates/agent/src/persistence.rs:L20-L21` |
| `event_type` | `String` | 事件类型，取值见 `event_types` | `crates/agent/src/persistence.rs:L22-L38` |
| `session_key` | `String` | 原始 session key，文件名会使用 sanitized key | `crates/agent/src/persistence.rs:L23-L24`、`crates/agent/src/persistence.rs:L566-L577` |
| `content` | `serde_json::Value` | 事件载荷；不同事件类型有不同结构 | `crates/agent/src/persistence.rs:L24-L25` |

事件载荷举例：

- user message：`{"message": content}`，证据 `crates/agent/src/persistence.rs:L87-L95`。
- assistant message：`{"message": content}`，证据 `crates/agent/src/persistence.rs:L97-L109`。
- tool call：`{"call_id": call_id, "tool_name": tool_name, "arguments": arguments}`，证据 `crates/agent/src/persistence.rs:L121-L139`。
- tool result：`{"call_id": call_id, "tool_name": tool_name, "result": result, "success": success}`，证据 `crates/agent/src/persistence.rs:L152-L172`。
- session start/end：metadata 透传，证据 `crates/agent/src/persistence.rs:L174-L200`。

### 2.3 配置模型：`MemoriesConfig`

`MemoriesConfig` 是 memory 配置；它**已经被实际链路消费**，不再只是占位（参见 §0、§3、§4 的引用点）：

```rust
// crates/agent/src/config.rs:L403-L436
pub struct MemoriesConfig {
    pub disable_on_external_context: Option<bool>,
    pub generate_memories: Option<bool>,
    pub use_memories: Option<bool>,
    pub max_raw_memories_for_consolidation: Option<usize>,
    pub max_unused_days: Option<i64>,
    pub max_rollout_age_days: Option<i64>,
    pub max_rollouts_per_startup: Option<usize>,
    pub min_rollout_idle_hours: Option<i64>,
    pub extract_model: Option<String>,
    pub consolidation_model: Option<String>,
}
```

同名 TS 类型见 `web/src/pages/Settings/tabs/agent/types.ts`。这些字段对应的运行时实体已在 `crates/agent/src/memory/types.rs` 落地：`MemoryFileEntry`、`MemoryFileStats`、`Phase1Status`、`RolloutRecord`、`SearchQuery` / `SearchResult` / `SearchResponse`、`SearchMatchMode`、`MemoryTool` / `MemoryToolResult`、`ThreadMemoryMode`（`crates/agent/src/memory/pollution.rs`）等。不再使用关系/向量数据库式 `MemoryRecord`：记忆条目以 Markdown 文本块形式存放在 `MEMORY.md` / `raw_memories.md` / `rollout_summaries/*.md` 中，由 `memory::parse` 与 `memory::write` 模块按段处理。

### 2.4 分层结构

现已落地的分层：

- **active session in-memory history**：`DashMap<String, AgentSession>`（`AgentSessionManager`，`crates/agent/src/session/session_store.rs:L17-L27`）。
- **persisted session JSONL archive**：`list_conversations()` 扫描 `data_dir/sessions`，证据 `crates/agent/src/persistence.rs`。
- **compaction summary**：作为一条用户消息形式的 summary 写回 `session.history`（`crates/agent/src/compact.rs`），同时落 `compaction` JSONL 事件。
- **长期记忆（文件式）**：`$agent_home/memory/`，分为 raw（`raw_memories.md`）→ consolidated（`MEMORY.md`）→ summary（`memory_summary.md`），以及按 rollout 切分的对话摘要 `rollout_summaries/*.md`。phase-1 抽取产出 raw，phase-2 整合写入 MEMORY/summary，并由 `retention.rs` 按 `max_unused_days` / `max_rollout_age_days` 等阈值修剪。
- **污染（pollution）控制**：`ThreadMemoryMode` + `PollutionDetector`（`crates/agent/src/memory/pollution.rs`）决定本轮是否允许写入 long-term memory（`disable_on_external_context` 的实际开关点）。

仍未引入 episodic vs semantic / working vs archive 的多层向量记忆模型；当前是 raw → consolidated → summary 的**三档文件层级**。

## 3. 自动化写入链路

### 3.1 触发点清单

| 事件 | 触发点 | 处理函数 | 写入目标 | 证据 |
| --- | --- | --- | --- | --- |
| Feishu inbound message | WebSocket event loop 收到 owner 消息 | `run_event_loop()` -> `process_agent_chat()` | active session + optional JSONL | `crates/bifrost-admin/src/handlers/im_gateway.rs:L619-L731`、`crates/bifrost-admin/src/handlers/im_gateway.rs:L833-L900` |
| `/agent/chat` API | `POST /agent/chat` | `run_turn_with_mcp()` | active session + optional JSONL | `crates/bifrost-admin/src/handlers/im_gateway.rs:L2062-L2135` |
| session start | 创建 recorder 时 | `record_session_start()` | JSONL | `crates/bifrost-admin/src/handlers/im_gateway.rs:L859-L883`、`crates/bifrost-admin/src/handlers/im_gateway.rs:L2099-L2121` |
| user message | 普通 turn 或内置命令 | `record_user_message()` | JSONL | `crates/agent/src/session.rs:L474-L518`、`crates/agent/src/session.rs:L657-L665` |
| assistant message | 模型返回最终文本 | `record_assistant_message()` | JSONL | `crates/agent/src/session.rs:L869-L884` |
| tool call | 模型请求工具 | `record_tool_call_with_id()` | JSONL | `crates/agent/src/session.rs:L908-L929` |
| tool result | 工具执行结束 | `record_tool_result_with_call_id()` | JSONL | `crates/agent/src/session.rs:L981-L995` |
| 自动压缩 | pre-turn / mid-turn token 超阈值 | `compact::compact_session()` | session.history，非 JSONL | `crates/agent/src/session.rs:L620-L650`、`crates/agent/src/session.rs:L1023-L1054` |
| 手动压缩 | 用户发送 `/compact` | `compact::compact_session()` | session.history，返回中文结果 | `crates/agent/src/session.rs:L512-L561` |

### 3.2 压缩链路与长期记忆抽取

`compact_session()` 的 prompt 明确是“CONTEXT CHECKPOINT COMPACTION”，用于 handoff summary；它调用同一个 `AgentClient::chat_completion()`，然后重建 session history：

```rust
// crates/agent/src/compact.rs:L80-L93
const COMPACTION_PROMPT: &str = r#"You are performing a CONTEXT CHECKPOINT COMPACTION. Create a concise handoff summary for another LLM that will resume the task.

Include:
- Current progress and key decisions made
- Important context, constraints, or user preferences discovered
- What was accomplished (including tool calls and their results — summarize file paths, command outputs, and key findings)
- What remains to be done (clear next steps if any)
- Any critical data, examples, references, or error patterns needed to continue
- File paths that were read, written, or modified
- Environment details that affect execution (working directory, shell, OS)

Be concise and structured. Use bullet points. Focus on preserving information needed to continue the conversation seamlessly without re-discovering context."#;
```

模型调用与 history 替换证据：

```rust
// crates/agent/src/compact.rs:L163-L180
let summary_messages = vec![
    ChatMessage::system(COMPACTION_PROMPT),
    ChatMessage::user(&truncated_history),
];

let response = client
    .chat_completion(config, &summary_messages, &[])
    .await?;

let summary = response
    .content
    .or(response.reasoning_content)
    .unwrap_or_else(|| "(compaction produced no summary)".to_string());

let recent_user_messages = collect_recent_user_messages(&session.history, 3);
```

```rust
// crates/agent/src/compact.rs:L196-L221
new_history.push(ChatMessage::user(&format!(
    "{SUMMARY_PREFIX}{summary}{compaction_note}"
)));

for msg in &recent_user_messages {
    new_history.push(msg.clone());
}

let (new_history, sanitize_report) = history::sanitize_chat_history(&new_history);
...
session.history = new_history;
session.compaction_count += 1;
session.history_version = session.history_version.saturating_add(1);
```

结论：`compact_session()` 本身仍只做上下文压缩。但**自动化记忆写库已经独立实现**：每轮 turn 结束后 `crates/agent/src/session/turn_loop.rs:L2394` 调用 `memory::auto_extract_after_turn_with_pollution_check_blocking()`，由 `extract.rs` 用 `extract_model` 抽取候选事实写入 `raw_memories.md`；当 raw 条目数达到 `max_raw_memories_for_consolidation` 或满足 phase-2 条件时由 `consolidation.rs` 用 `consolidation_model` 整合到 `MEMORY.md` / `memory_summary.md`，超时分别由 `MEMORY_EXTRACT_TIMEOUT_SECS` / `MEMORY_CONSOLIDATION_TIMEOUT_SECS` 控制（`crates/agent/src/memory/constants.rs`）。`PollutionDetector` 决定当前 thread 是否允许写入。

### 3.3 去重 / 合并 / 覆盖 / 衰减 / TTL / pin

已实现：

- active session TTL：`AgentSessionManager::cleanup_expired()` 按 `session_ttl_secs` 从内存移除 active session，证据 `crates/agent/src/session.rs:L310-L314`。
- JSONL 文件 90 天清理：IM Gateway startup 时在 `HistoryPersistence::Last90Days` 下调用 `cleanup_expired_sessions()`，证据 `crates/bifrost-admin/src/handlers/im_gateway.rs:L543-L556`；清理实现见 `crates/agent/src/persistence.rs:L495-L512`。
- session 文件名 sanitize，避免路径非法字符，证据 `crates/agent/src/persistence.rs:L566-L577`。
- session listing dedup active/history：`/agent/sessions/all` 用 active keys 跳过重复，证据 `crates/bifrost-admin/src/handlers/im_gateway.rs:L1844-L1905`。

长期 memory 级策略**部分落地**：`memory::consolidation` 在 phase-2 用模型做合并 / 改写（替代单条 dedupe），`retention.rs::prune_memory_artifacts` 按 `max_unused_days`、`max_rollout_age_days`、`max_rollouts_per_startup`、`min_rollout_idle_hours` 做衰减式清理；`prune_stage1_outputs_for_retention` 控制 phase-1 阶段产物保留。仍未实现显式 pin（保护条目不被覆盖）与 per-record TTL；当前去重靠模型 prompt 与 phase-2 整合提示词约束，证据 `crates/agent/src/memory/consolidation.rs:L33-L381`、`crates/agent/src/memory/retention.rs:L25-L200`。

### 3.4 敏感信息过滤 / 脱敏

当前对 provider 配置的 API response 有脱敏：`sanitize_provider()` “never expose secret_ref in plaintext”，证据 `crates/bifrost-admin/src/handlers/im_gateway.rs:L2506-L2515`。但 conversation recorder 写 user/tool/assistant 原文，没有通用 redact/mask：

- `record_user_message()` 直接写 `{"message": content}`，证据 `crates/agent/src/persistence.rs:L87-L95`。
- `record_tool_call_with_id()` 直接写 `arguments`，证据 `crates/agent/src/persistence.rs:L121-L139`。
- `record_tool_result_with_call_id()` 直接写 `result`，证据 `crates/agent/src/persistence.rs:L152-L172`。

`history::sanitize_chat_history()` 只修复 Chat Completions tool-call 序列，不是隐私脱敏，证据 `crates/agent/src/history.rs:L19-L25`。

### 3.5 用户显式“记住这件事”入口

现已落地以下入口：

- chat slash 命令 `/remember <text>` → `BuiltinCommand::Remember` → `memory::remember_explicit()`，把一条用户口述事实写入长期记忆；
- chat slash 命令 `/forget <id|last>` → `BuiltinCommand::Forget` → `memory::forget_memory()`，按 id 或 `last` 移除最近一条；
- 模型自助通过 MCP 工具 `memory/list`、`memory/read`、`memory/search`（`crates/agent/src/memory/mcp_tools.rs`）按需查询。

证据：`crates/agent/src/slash.rs:L78-L194`、`crates/agent/src/memory/write.rs:L133-L381`。仍**没有**面向最终用户的 `bifrost memory` 顶层 CLI 或独立 HTTP `/memory` 路由（planned, not yet shipped as of 2026-06-16）；既有的 `bifrost-cli config memory` 是进程内存诊断，与长期记忆无关。

## 4. 检索与注入

### 4.1 检索方式

已实现的检索分两层：

**会话级（已有）**

- active session detail：从 `AgentSessionManager` `DashMap` 取单个 session。
- JSONL 文件列表 / replay / summary quick scan，见 `crates/agent/src/persistence.rs`。

**长期记忆级（新增）**

- 文件式罗列：`memory::list_visible_memories()`（`crates/agent/src/memory/write.rs:L336`）。
- 关键词搜索：`memory::search_memory_files()` + `crates/agent/src/memory/search.rs`，支持 `SearchMatchMode` / `SearchQuery`（`crates/agent/src/memory/types.rs:L178-L236`）。
- MCP 工具 `memory/list`、`memory/read`、`memory/search` 让模型按需调用（`crates/agent/src/memory/mcp_tools.rs`）。

仍**未实现** embedding / 向量库 / 语义相似度 topK 召回；planned, not yet shipped as of 2026-06-16。

### 4.2 召回策略

当前 prompt 注入策略：每次模型请求调用 `build_messages(system_prompt, memory_message)`：先 prepend system prompt，再注入 `memory::recall_system_message()` 返回的 developer message（含 `memory_summary.md` 摘要和读路径说明），最后接 `session.history`。历史过长时通过 token/context budget 触发 compaction；provider context-window overflow fallback 才会删除最老消息重试。

证据：

```rust
// crates/agent/src/session.rs
fn build_messages(
    &self,
    prompt_prefix: &[ChatMessage],
    memory_message: Option<&ChatMessage>,
) -> Vec<ChatMessage> {
    let full_history = self.history.clone();
    let mut messages = Vec::new();
    messages.extend(prompt_prefix.iter().cloned());
    if let Some(memory_message) = memory_message {
        messages.push(memory_message.clone());
    }
    messages.extend(full_history);
    let (sanitized, report) = history::sanitize_chat_history(&messages);
    ...
    sanitized
}
```

长期记忆 scope 当前以 `$agent_home/memory/` 全局共享为主，`AgentSession` 新增 `user_id`（`crates/agent/src/session.rs`）字段以承载 per-user 长期记忆 scope，但 read-path / write-path 当前并未基于 `user_id` 切分文件目录——多用户隔离仍属 planned, not yet shipped as of 2026-06-16。已有隔离键仍是 `session_key`、`work_dir`、`source`。

### 4.3 注入形式

注入形式是 Chat Completions messages：

- system prompt：`prompt::build_system_prompt(...)`，证据 `crates/agent/src/session.rs:L653-L655`。
- history：`session.history` 进入 `build_messages()`，证据 `crates/agent/src/session.rs:L694-L705`。
- compaction summary：作为 `ChatMessage::user(...)` 放到 history 第一条，证据 `crates/agent/src/compact.rs:L196-L204`。

`MemoriesConfig.use_memories` 的 UI 文案是“Inject memory usage instructions into developer prompts”，对应 `web/src/pages/Settings/tabs/AgentTab.tsx:L1178-L1240`。`get_memories_config()` 现在在多个位置被消费：`crates/agent/src/memory/read_path.rs:L17-L24`（开关）、`crates/agent/src/memory/extract.rs:L194`、`crates/agent/src/memory/consolidation.rs:L351-L381`、`crates/agent/src/memory/retention.rs:L27/L191`。

## 5. 生命周期与管理

### 5.1 增删改查 / 导入导出

| 能力 | 当前状态 | 证据 |
| --- | --- | --- |
| 创建 active session | 首次消息时 `take_session()` 没有命中则新建 | `crates/agent/src/session.rs:L284-L303` |
| 清空 active session | `DELETE /agent/sessions` 或 `DELETE /agent/sessions/:key` | `crates/bifrost-admin/src/handlers/im_gateway.rs:L1824-L1836`、`crates/bifrost-admin/src/handlers/im_gateway.rs:L2039-L2059` |
| 查看 active session | `GET /agent/sessions`、`GET /agent/sessions/:key` | `crates/bifrost-admin/src/handlers/im_gateway.rs:L1824-L1829`、`crates/bifrost-admin/src/handlers/im_gateway.rs:L2044-L2053` |
| 查看 active + history | `GET /agent/sessions/all` | `crates/bifrost-admin/src/handlers/im_gateway.rs:L1839-L1950` |
| 查看 JSONL history 列表 | `GET /agent/sessions/history` | `crates/bifrost-admin/src/handlers/im_gateway.rs:L1953-L1990` |
| 查看 JSONL history 内容 | `GET /agent/sessions/history/{encoded_path}` | `crates/bifrost-admin/src/handlers/im_gateway.rs:L1992-L2024` |
| 删除 JSONL history 文件 | `DELETE /agent/sessions/history/{encoded_path}` | `crates/bifrost-admin/src/handlers/im_gateway.rs:L2025-L2035` |
| 恢复最近 JSONL 到当前 session | chat 命令 `/resume` | `crates/agent/src/session.rs:L587-L618` |
| 长期记忆显式 CRUD | chat slash `/remember`、`/forget`；模型侧 MCP `memory/list`、`memory/read`、`memory/search` | `crates/agent/src/slash.rs:L78-L194`、`crates/agent/src/memory/write.rs:L133-L381`、`crates/agent/src/memory/mcp_tools.rs` |
| 长期记忆导入/导出 / 顶层 CLI | 未实现（planned, not yet shipped as of 2026-06-16） | 仓库内仅 `bifrost-cli config memory` 进程内存诊断（`crates/bifrost-cli/src/cli.rs:L2161-L2162`），与长期记忆无关 |

### 5.2 清理策略

- active session：`session_ttl_secs` 默认 3600 秒，证据 `crates/agent/src/config.rs:L323-L328`、`crates/agent/src/session.rs:L310-L314`。
- persisted history：配置可选 `HistoryPersistence::Last90Days`，startup 删除 90 天前 JSONL，证据 `crates/bifrost-admin/src/handlers/im_gateway.rs:L543-L556`、`crates/agent/src/persistence.rs:L495-L512`。
- `HistoryConfig.max_bytes` 现已落地：`ConversationRecorder::enforce_max_bytes()` 在每次写入时检查文件大小，超过即按最早事件切片重写，证据 `crates/agent/src/persistence.rs:L426-L450`、`crates/agent/src/persistence.rs:L120`（写入入口处调用）、`crates/agent/src/persistence.rs:L87-L102`（构造函数 `new_with_max_bytes` / `from_existing_file` 接收 `max_bytes`）。
- 长期记忆侧由 `memory::retention::prune_memory_artifacts()` 在启动 / 周期触发时按 `max_unused_days` / `max_rollout_age_days` 修剪 raw / rollout 产物。

### 5.3 多租户 / 多用户 / 多设备隔离

当前隔离边界：

- Feishu owner check：只处理 owner_open_id 发来的消息，证据 `crates/bifrost-admin/src/handlers/im_gateway.rs:L619-L650`。
- session key：使用 `event.source.user_id` 作为 per-user session key，证据 `crates/bifrost-admin/src/handlers/im_gateway.rs:L712-L727`、`crates/bifrost-admin/src/handlers/im_gateway.rs:L782-L799`。
- active session map：`DashMap<String, AgentSession>`，证据 `crates/agent/src/session.rs:L270-L281`。
- persisted history filename：session key 经过 `sanitize_key()` 写入路径，但 JSONL 内保留原始 session key，证据 `crates/agent/src/persistence.rs:L63-L64`、`crates/agent/src/persistence.rs:L410-L413`。

与 Remote Invoke / FileAccessPolicy / Shell Access 没有直接 memory 耦合；远端通道里出现的 `long_term_pubkey`、`remember_bridge_seq` 等是连接层概念，与 Agent memory 无关。`AgentSession.user_id` 字段为长期记忆 per-user scope 预留，但 `$agent_home/memory/` 当前仍是设备级共享（planned, not yet shipped as of 2026-06-16）。

## 6. 测试覆盖

### 6.1 单元测试

| 文件 | 覆盖点 | 证据 |
| --- | --- | --- |
| `crates/agent/src/persistence.rs` | recorder 基本写入、tool 事件恢复、orphan tool 跳过、多 pending tool call 匹配、JSONL events 加载、session lifecycle | `crates/agent/src/persistence.rs:L584-L915` |
| `crates/agent/src/history.rs` | tool-call message invariant sanitize：孤立 tool、不完整 assistant tool_calls、未知 tool id | `crates/agent/src/history.rs:L98-L200` |
| `crates/agent/src/compact.rs` | history format、长消息截断、多字节安全、recent user message、tool call invariant | `crates/agent/src/compact.rs:L385-L492` |
| `crates/agent/src/session.rs` | token usage、build_messages sanitize、trim 等（搜索命中） | `crates/agent/src/session.rs:L1232-L1240`、`crates/agent/src/session.rs:L1346-L1369` |

### 6.2 E2E / human_tests

- E2E `im_gateway_agent_tool_history_resume_regression` 使用 mock Chat Completions，验证首次工具调用、JSONL 恢复、恢复后再次工具调用，证据 `crates/bifrost-e2e/src/tests/im_gateway_agent.rs:L371-L420`。
- `human_tests/im-gateway-agent.md` 包含 `/compact` 手动记忆压缩用例，证据 `human_tests/im-gateway-agent.md:L152-L163`。
- `human_tests/im-gateway-agent.md` TC-IMA-67 记录了 Agent Loop tool message 序列回归，含执行命令和 PASS 记录，证据 `human_tests/im-gateway-agent.md:L863-L895`。
- `human_tests/agent-session-persistence.md` 说明 JSONL 持久化用例覆盖 session_start/user/assistant/tool/compaction/session_end 等预期，证据 `human_tests/agent-session-persistence.md:L5-L7`、`human_tests/agent-session-persistence.md:L24-L73`。

### 6.3 空白 / 风险点

- `MemoriesConfig` 字段现在有 `crates/agent/src/memory/tests.rs`（含 `recall_system_message` 等用例）和 phase-1/phase-2 单元测试覆盖。
- compaction 事件 gap **已修复**：turn loop 调用 `record_compaction_event()` 写 JSONL，回归测试见 `crates/agent/src/session/tests.rs::test_record_compaction_event_includes_emergency_and_total_tokens`、`crates/agent/src/persistence.rs::record_compaction_event_round_trip`。
- 仍无通用隐私脱敏测试覆盖 conversation recorder；user/tool/result 原文照旧 JSONL 落盘。`PollutionDetector` 主要解决 cross-thread 上下文污染，并非密钥脱敏。
- 仍无 memory embedding / topK 测试，因 embedding/向量库未实现；关键词搜索由 `crates/agent/src/memory/search.rs` 的单元用例覆盖。

## 7. 与其它子系统的耦合

### 7.1 Agent Loop / Tool Router / MCP

Agent loop 构建 tool definitions（本地 tools + MCP tools），执行后把 tool call/result 进入 history 和 recorder，证据 `crates/agent/src/session.rs:L667-L705`、`crates/agent/src/session.rs:L908-L995`。这使会话持久化强耦合于 Chat Completions tool-call invariant，设计文档也明确该点：

- `design/im-gateway-agent.md:L36-L48`：合法片段必须 `assistant(tool_calls)` 紧邻 `tool` 结果，恢复时要重建配对。
- `design/im-gateway-agent.md:L52-L60`：`sanitize_chat_history()` 在发送模型前统一检查。

### 7.2 IM Gateway

IM Gateway 是当前 Agent 的主要上游入口：

- `run_event_loop()` 接收 Feishu 事件、owner 检查、route match，默认进入 `process_agent_chat()`，证据 `crates/bifrost-admin/src/handlers/im_gateway.rs:L517-L533`、`crates/bifrost-admin/src/handlers/im_gateway.rs:L703-L731`。
- `process_agent_chat()` 创建 recorder 并调用 `run_turn_with_mcp()`，证据 `crates/bifrost-admin/src/handlers/im_gateway.rs:L833-L900`。

### 7.3 WebUI / Admin API

Settings -> Agent 页面消费 `/im-gateway/agent` 配置和 `/agent/sessions*` API。`Memories` UI 会 PATCH `memories` 整体字段，证据 `web/src/pages/Settings/tabs/AgentTab.tsx:L829-L1059`、`crates/bifrost-admin/src/handlers/im_gateway.rs:L2253-L2305`。

### 7.4 Config / Credential / TLS 基础设施

Agent 复用 Bifrost data dir 约定：

- agent home 解析顺序：`$BIFROST_DATA_DIR/agent/`、`~/.bifrost/agent/`，不再支持历史 `BIFROST_AGENT_HOME` 或 `~/.bifrost-agent/`。
- config 加载顺序：user-level `$BIFROST_DATA_DIR/agent/config.toml`、project-level `.bifrost/agent/config.toml`、env override。
- provider API key 从环境变量解析，不直接落 session history；证据 `crates/agent/src/config.rs:L507-L558`。

未发现 TLS、Traffic Recorder 或 Remote Invoke 将 session rollout 直接转存为 memory。搜索证据：`rg -n -i "rollout|memory|memories" crates web design docs human_tests README.md` 只命中 `MemoriesConfig` 中 rollout 配置字段、agent skill 文档和本地任务派发设计；无自动导入 rollout 的实现。

## 8. 现状 vs 设计文档的 Gap

### 8.1 已落地

- Agent 集成到 IM Gateway，通过 Feishu/内部 API 触发模型对话，证据 `design/im-gateway-agent.md:L3-L18` 与 `crates/bifrost-admin/src/handlers/im_gateway.rs:L703-L731` 一致。
- Session manager、TTL、内置命令、上下文压缩、history sanitize 已落地，证据 `crates/agent/src/session.rs:L270-L346`、`crates/agent/src/session.rs:L474-L618`、`crates/agent/src/compact.rs:L111-L125`、`crates/agent/src/history.rs:L19-L92`。
- JSONL persistence + restore 修复已落地，设计文档列出的 tool-call 恢复原则与 `load_conversation()` 当前实现一致，证据 `design/im-gateway-agent.md:L42-L48`、`crates/agent/src/persistence.rs:L279-L345`。

### 8.2 设计了但未完全实现

- 长期记忆 embedding / 向量检索 / 语义相似度 topK 仍未引入；当前长期记忆检索为关键词 + 模型读路径（planned, not yet shipped as of 2026-06-16）。
- 多用户隔离：`AgentSession.user_id` 已加，但 `$agent_home/memory/` 默认全局共享，未按 `user_id` 切分（planned, not yet shipped as of 2026-06-16）。
- 顶层 `bifrost memory ...` CLI / `/memory` HTTP 路由：尚未提供（planned）；当前仅 chat slash + MCP 工具。
- pin / per-record TTL：phase-2 consolidation 与 retention 已覆盖批量衰减，但没有“此条永不被覆盖”的显式 pin 标记（planned）。
- 通用隐私脱敏（user/tool/result 原文落盘前 redact）仍未实现。
- 此前文档中提到的 `event_types::COMPACTION` 写入 gap 与 `HistoryConfig.max_bytes` 落地 gap **均已修复**，见 §3.1 / §5.2。
- `design/im-gateway-agent.md` 早期段落的旧结构描述（`ImAgentSessionManager` / `Session { messages: Vec<ChatMessage> }`）仍可能与当前 `AgentSession` / `ConversationRecorder` 实现不一致，建议同步刷新。

### 8.3 实现了但未充分文档化

- `ConversationRecorder` 的 JSONL 路径、事件 schema、恢复规则主要在代码注释和 human_tests 中，README 仅有 Agent Skill 链接和资源内存诊断，证据 `README.md:L31-L31`、`README.md:L143-L143`、`crates/agent/src/persistence.rs:L1-L4`。
- `/agent/sessions/history/{path}` 允许按 URL encoded 文件路径读取/删除 history 文件；在 WebUI 中使用，但缺少面向用户的 API 文档，证据 `crates/bifrost-admin/src/handlers/im_gateway.rs:L1992-L2036`。
- `history::sanitize_chat_history()` 是关键稳定性层，但设计文档只在 Agent Loop 章节描述，未形成独立 API/安全文档，证据 `design/im-gateway-agent.md:L50-L60`、`crates/agent/src/history.rs:L19-L92`。

## 9. 风险与建议

1. **P0 → 已交付**：长期记忆系统已落地为文件式 `$agent_home/memory/`，含 raw → consolidated → summary 三层，UI `Memories` 文案与运行时一致。
2. **P0 → 已交付**：`MemoriesConfig` 字段已被 read-path / extract / consolidation / retention 全链路消费（参见 §0、§3、§4）。
3. **P1（保留）**：为 conversation recorder 增加敏感信息过滤；当前 user/tool/result 原文直接 JSONL 落盘。
4. **P1 → 已交付**：compaction event 已通过 `record_compaction_event()` 写入 JSONL（§3.1 末尾说明）。
5. **P1（保留 / 重新核实）**：复核 `/resume` 在 `data_dir` 选择上的一致性——`recall_system_message` 与 phase-1 抽取都基于 `agent_home_dir()`，但 `/resume` 仍使用 `config.resolve_work_dir()`，建议对照当前 `crates/agent/src/session/slash_commands.rs` 重新核实。
6. **P1 → 已交付**：`HistoryConfig.max_bytes` 已经在 `enforce_max_bytes()` 中实现（§5.2）。
7. **P2（保留）**：长期记忆检索仍以关键词 + 文件读取为主；如要引入 embedding / 向量索引应先评估隐私与可解释性。
8. **P2（保留）**：把 `/agent/sessions/history/{path}` 路径安全约束与 `bifrost-cli` 侧的 memory 子命令缺口写入面向用户的文档。
9. **P2（保留）**：补齐 `bifrost memory ...` 顶层 CLI 与/或 `/memory` HTTP 路由，覆盖 list / read / search / forget / export 等管理动作。
10. **P2（保留）**：同步刷新 `design/im-gateway-agent.md` 中过期的 session manager 结构描述，与当前 `crates/agent/src/session.rs::AgentSession` 对齐。

## 附录 A：搜索命令清单

本次用于复核的主要命令如下（`web/dist-desktop` 的构建产物搜索会产生大量无关 Monaco 命中，结论以源码目录为准）：

```bash
pwd
git rev-parse --abbrev-ref HEAD && git rev-parse --short HEAD && git status --porcelain
rg -n -i "\b(memory|memories|recall|remember|memo|knowledge|embedding|vector|episodic|semantic|long[-_ ]?term|short[-_ ]?term|记忆)\b" .
rg --files | rg -i "memory|memories|recall|remember|memo|knowledge|embedding|vector|episodic|semantic|long[-_]?term|short[-_]?term|记忆"
rg -n -i "memories|memory compaction|memory usage instructions|generate_memories|use_memories|disable_on_external_context|max_raw_memories|max_unused_days|max_rollout_age_days|extract_model|consolidation_model" crates web design docs human_tests README.md AGENTS.md Cargo.toml
rg -n -i "recall|remember|knowledge|embedding|vector|episodic|semantic|long[-_ ]?term|short[-_ ]?term|记忆" crates web design docs human_tests README.md AGENTS.md Cargo.toml
rg -n -i "memory" crates/agent web/src/pages/Settings/tabs/AgentTab.tsx web/src/pages/Settings/tabs/agent/types.ts crates/bifrost-admin/src/handlers/im_gateway.rs human_tests/im-gateway-agent.md design/im-gateway-agent.md
find crates -maxdepth 3 -type f | sort | rg 'crates/agent|memory|memories'
rg -n "record_(session_start|session_end|user_message|assistant_message|tool_call|tool_result)|ConversationRecorder::new|cleanup_expired_sessions|list_conversations|load_conversation" crates web e2e-tests human_tests design docs README.md
rg -n "agent/sessions|sessions/history|sessions/|compact|resume|memory|memories" crates/bifrost-admin/src/handlers/im_gateway.rs web/src/pages/Settings/tabs/agent web/src/pages/Settings/tabs/AgentTab.tsx
rg -n -i "agent|memory|memories|compact|session|history" README.md docs design --glob '!web/dist-desktop/**'
rg -n -i "memory_mode|raw_memor|rollout|consolidat|extract_model|use_memories|generate_memories|disable_on_external_context|embedding|vector|semantic|episodic|recall|remember" crates/agent crates/bifrost-admin/src/handlers/im_gateway.rs web/src/pages/Settings/tabs/AgentTab.tsx web/src/pages/Settings/tabs/agent/types.ts design docs human_tests README.md
rg -n -i "bifrost memory|memory (add|set|delete|remove|list|search|recall|remember)|remember this|记住|召回|长期记忆|短期记忆|向量|嵌入|embedding|vector|semantic|episodic" crates web design docs human_tests README.md SKILL.md
rg -n -i "sqlite|postgres|jsonl|schema|migration|cache|sessions/.+jsonl|conversation" crates/agent crates/bifrost-admin/src/handlers/im_gateway.rs design/im-gateway-agent.md human_tests/im-gateway-agent.md human_tests/agent-session-persistence.md
rg -n "struct .*Memory|enum .*Memory|trait .*Memory|fn .*memory|memory_mode|MemoryStore|MemoryRecord|MemoryEntry|MemoryManager|MemoryRepository" crates/agent crates/bifrost-admin crates/bifrost-cli web/src design docs human_tests README.md
rg -n -i "recall|retrieve|retrieval|top_k|topK|threshold|similarity|embedding|vector|semantic|episodic|knowledge|long[-_ ]term|short[-_ ]term|working memory|archive" crates/agent crates/bifrost-admin crates/bifrost-cli web/src design docs human_tests README.md
rg -n -i "redact|mask|sanitize|sensitive|secret|credential|api[_-]?key|token|password" crates/agent crates/bifrost-admin/src/handlers/im_gateway.rs design/im-gateway-agent.md human_tests/agent-session-persistence.md human_tests/im-gateway-agent.md
rg -n -i "delete.*memory|memory.*delete|export.*memory|import.*memory|memory.*import|memory.*export|memory.*list|memory.*search|remember|记住|召回|pin|ttl|dedup|dedupe|merge|expire|cleanup|retention" crates/agent crates/bifrost-admin crates/bifrost-cli web/src design docs human_tests README.md
test -e design/memory-system-analysis.md; echo $?
date '+%Y-%m-%d %H:%M:%S %Z'
```

## 附录 B：关键文件索引

- `crates/agent/src/lib.rs:L1-L13` -> Agent crate 能力总览，说明 memory compaction / persistence。
- `crates/agent/src/config.rs:L119-L131` -> `AgentConfig` 包含 history、ephemeral、memories。
- `crates/agent/src/config.rs:L262-L295` -> `MemoriesConfig` 字段定义。
- `crates/agent/src/config.rs:L826-L845` -> agent home 目录解析。
- `crates/agent/src/session.rs:L32-L69` -> `AgentSession` 核心字段。
- `crates/agent/src/session.rs:L192-L235` -> prompt/history 注入与裁剪策略。
- `crates/agent/src/session.rs:L474-L618` -> 内置命令 `/clear`、`/undo`、`/compact`、`/status`、`/resume`。
- `crates/agent/src/session.rs:L620-L650` -> pre-turn auto compaction。
- `crates/agent/src/session.rs:L879-L995` -> assistant/tool event 自动记录。
- `crates/agent/src/session.rs:L1023-L1054` -> mid-turn auto compaction。
- `crates/agent/src/compact.rs:L80-L99` -> compaction prompt 和 summary prefix。
- `crates/agent/src/compact.rs:L111-L125` -> `compact_session()` API。
- `crates/agent/src/compact.rs:L196-L221` -> compaction summary 注入回 session history。
- `crates/agent/src/persistence.rs:L1-L4` -> JSONL 存储路径说明。
- `crates/agent/src/persistence.rs:L18-L38` -> `ConversationEvent` 和事件类型。
- `crates/agent/src/persistence.rs:L44-L84` -> `ConversationRecorder` 写入实现。
- `crates/agent/src/persistence.rs:L246-L345` -> JSONL 恢复为 `ChatMessage`。
- `crates/agent/src/persistence.rs:L371-L468` -> persisted session summary scan。
- `crates/agent/src/persistence.rs:L470-L512` -> list / cleanup JSONL。
- `crates/agent/src/history.rs:L19-L92` -> Chat history invariant sanitize。
- `crates/bifrost-admin/src/handlers/im_gateway.rs:L517-L533` -> IM event loop 参数包含 agent runtime。
- `crates/bifrost-admin/src/handlers/im_gateway.rs:L543-L556` -> startup 清理 90 天前 session files。
- `crates/bifrost-admin/src/handlers/im_gateway.rs:L833-L900` -> Feishu 消息进入 Agent turn，并创建 recorder。
- `crates/bifrost-admin/src/handlers/im_gateway.rs:L1824-L2036` -> session/history Admin API。
- `crates/bifrost-admin/src/handlers/im_gateway.rs:L2062-L2135` -> `POST /agent/chat` 内部测试入口。
- `crates/bifrost-admin/src/handlers/im_gateway.rs:L2253-L2305` -> PATCH `memories` 配置字段。
- `web/src/pages/Settings/tabs/AgentTab.tsx:L829-L1059` -> WebUI Memories 配置卡片。
- `web/src/pages/Settings/tabs/agent/types.ts:L54-L65` -> TS `MemoriesConfig` 类型。
- `design/im-gateway-agent.md:L20-L60` -> tool message 序列稳定性设计与当前恢复逻辑。
- `human_tests/im-gateway-agent.md:L152-L163` -> `/compact` 真实场景用例。
- `human_tests/im-gateway-agent.md:L863-L895` -> Agent Loop tool history resume 回归记录。
