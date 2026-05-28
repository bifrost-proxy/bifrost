# Bifrost Memory 系统现状分析

> 生成时间：2026-05-02 16:40:37 CST
> 分支 / commit：feat/agent / d234548d
> 分析范围：整仓库

## 0. TL;DR

- Bifrost 当前没有完整的“长期记忆 / 自动化记忆 / 语义召回”系统；已落地的是 `crates/agent` 中的会话历史、JSONL 持久化和上下文压缩，证据见 `crates/agent/src/lib.rs:L1-L13`、`crates/agent/src/persistence.rs:L1-L4`、`crates/agent/src/compact.rs:L1-L13`。
- `memories` 配置结构、Admin PATCH 透传和 WebUI 表单已经存在，但没有被 `run_turn`、prompt 构造、持久化扫描或检索链路消费，证据见 `crates/agent/src/config.rs:L262-L295`、`crates/bifrost-admin/src/handlers/im_gateway.rs:L2253-L2305`、`web/src/pages/Settings/tabs/AgentTab.tsx:L829-L1059`。
- 自动写入当前只覆盖会话事件流：`session_start`、`user_message`、`assistant_message`、`tool_call`、`tool_result` 等写入 JSONL；没有“LLM 自动总结 -> 写入长期记忆”的独立存储链路，证据见 `crates/agent/src/persistence.rs:L18-L38`、`crates/agent/src/session.rs:L657-L665`、`crates/agent/src/session.rs:L879-L992`。
- 检索/召回当前是会话级历史重放和列表扫描，不存在关键词索引、全文索引、向量库、embedding 或 topK 召回策略；未实现判断基于 `rg -n -i "recall|retrieve|retrieval|top_k|embedding|vector|semantic|episodic|knowledge"` 等搜索只命中压缩阈值、远端密钥和无关文档。
- 生命周期管理已有 active session 清空、JSONL history 列表/读取/删除、90 天历史清理；但这些都是 session/history 级别，不是可编辑的长期记忆 CRUD，证据见 `crates/bifrost-admin/src/handlers/im_gateway.rs:L1824-L2036`、`crates/agent/src/persistence.rs:L470-L512`。

## 1. 架构总览

### 1.1 代码分布

| 归属 | 关键文件 | 角色 | 证据 |
| --- | --- | --- | --- |
| Agent crate | `crates/agent/src/lib.rs` | 声明 Agent 运行时能力：tool calling、多轮对话、memory compaction、persistence | `crates/agent/src/lib.rs:L1-L13` |
| Agent 会话 | `crates/agent/src/session.rs` | 保存单 session 的 `history`、token、压缩状态、recorder，并执行 turn loop | `crates/agent/src/session.rs:L32-L69`、`crates/agent/src/session.rs:L425-L433` |
| 压缩 | `crates/agent/src/compact.rs` | 对长上下文做模型总结，替换 session history | `crates/agent/src/compact.rs:L80-L99`、`crates/agent/src/compact.rs:L111-L125` |
| 持久化 | `crates/agent/src/persistence.rs` | 将会话事件写入 `$agent_home/sessions/YYYY/MM/DD/session-*.jsonl`，并支持加载/扫描/清理 | `crates/agent/src/persistence.rs:L1-L4`、`crates/agent/src/persistence.rs:L44-L84` |
| 配置 | `crates/agent/src/config.rs` | 定义 `MemoriesConfig`、history 配置和 agent home 目录 | `crates/agent/src/config.rs:L119-L131`、`crates/agent/src/config.rs:L262-L295`、`crates/agent/src/config.rs:L826-L845` |
| Admin API | `crates/bifrost-admin/src/handlers/im_gateway.rs` | Feishu / API 入口、session history API、agent config PATCH | `crates/bifrost-admin/src/handlers/im_gateway.rs:L517-L533`、`crates/bifrost-admin/src/handlers/im_gateway.rs:L1824-L2036`、`crates/bifrost-admin/src/handlers/im_gateway.rs:L2253-L2305` |
| WebUI | `web/src/pages/Settings/tabs/AgentTab.tsx`、`web/src/pages/Settings/tabs/agent/types.ts` | Agent 设置页展示 Memory 配置、session/history 视图类型 | `web/src/pages/Settings/tabs/AgentTab.tsx:L829-L1059`、`web/src/pages/Settings/tabs/agent/types.ts:L54-L65` |
| 测试 | `crates/agent/src/persistence.rs`、`crates/agent/src/compact.rs`、`crates/agent/src/history.rs`、`crates/bifrost-e2e/src/tests/im_gateway_agent.rs` | 单元测试和 E2E 覆盖 history / persistence / compaction 合法性 | `crates/agent/src/persistence.rs:L584-L915`、`crates/agent/src/compact.rs:L385-L492`、`crates/bifrost-e2e/src/tests/im_gateway_agent.rs:L371-L420` |

结论：Memory 相关能力不在独立 `memory` crate 中；实际代码集中在 `crates/agent`，由 `bifrost-admin` 的 IM Gateway 入口消费。`bifrost-cli config memory` 是进程内存诊断，不是记忆系统，证据见 `crates/bifrost-cli/src/cli.rs:L1346-L1353`、`crates/bifrost-cli/src/commands/config/mod.rs:L175-L205`。

### 1.2 当前数据流

```text
Feishu WebSocket / POST /agent/chat
        |
        v
crates/bifrost-admin/src/handlers/im_gateway.rs
  - process_agent_chat / /agent/chat
  - 创建或复用 AgentSession
  - 可选创建 ConversationRecorder
        |
        v
crates/agent/src/session.rs::run_turn_with_mcp
  - 内置命令：/clear /undo /compact /status /resume
  - pre-turn / mid-turn compact_session
  - build_messages(system prompt + session.history)
  - model chat completion + tool loop
  - recorder 写 user/tool/assistant 事件
        |
        v
crates/agent/src/persistence.rs
  - JSONL: agent_home/sessions/YYYY/MM/DD/session-{session_key}-{timestamp}.jsonl
  - load_conversation / scan_session_summary / cleanup_expired_sessions
        |
        v
Admin/WebUI:
  - /agent/sessions
  - /agent/sessions/all
  - /agent/sessions/history
  - /agent/sessions/history/{path}
```

证据：IM 事件入口和默认 Agent chat 路径见 `crates/bifrost-admin/src/handlers/im_gateway.rs:L703-L731`；内部测试 API 路径见 `crates/bifrost-admin/src/handlers/im_gateway.rs:L2062-L2135`；turn loop 步骤注释见 `crates/agent/src/session.rs:L425-L433`。

### 1.3 存储介质与 schema 摘要

当前“记忆相邻”的唯一持久存储是 JSONL 会话事件文件，不是 SQLite/Postgres/向量库。路径模板写在代码注释和构造函数中：

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

注意：`event_types::COMPACTION` 已声明，`scan_session_summary()` 也会识别 `compaction` 事件（`crates/agent/src/persistence.rs:L445-L451`），但 `compact_session()` 本身没有写 recorder，`run_turn_with_mcp()` 调 compact 后也没有记录 compaction 事件，证据见 `crates/agent/src/compact.rs:L118-L125`、`crates/agent/src/compact.rs:L218-L243`、`crates/agent/src/session.rs:L526-L546`、`crates/agent/src/session.rs:L1023-L1054`。

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

`MemoriesConfig` 是 memory 配置，但当前只存配置，不形成数据模型实体：

```rust
// crates/agent/src/config.rs:L262-L295
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

同名 TS 类型见 `web/src/pages/Settings/tabs/agent/types.ts:L54-L65`。这些字段暗示计划支持 raw memories、rollout 候选、extract/consolidation model、外部上下文污染标记等，但没有发现 `MemoryRecord`、`MemoryStore`、`memory_mode` 运行时结构。对应搜索证据：`rg -n "struct .*Memory|enum .*Memory|trait .*Memory|fn .*memory|memory_mode|MemoryStore|MemoryRecord|MemoryEntry|MemoryManager|MemoryRepository" crates/agent crates/bifrost-admin crates/bifrost-cli web/src design docs human_tests README.md` 只命中资源内存诊断和 `MemoriesConfig` 注释。

### 2.4 分层结构

已实现的分层只有：

- active session in-memory history：`DashMap<String, AgentSession>`，证据 `crates/agent/src/session.rs:L270-L281`。
- persisted session JSONL archive：`list_conversations()` 扫描 `data_dir/sessions`，证据 `crates/agent/src/persistence.rs:L470-L493`。
- compaction summary：在 history 中插入一条用户消息形式的 summary，不是独立 memory 表，证据 `crates/agent/src/compact.rs:L196-L204`。

未实现短期/长期、episodic/semantic、working memory/archive 等专门层。搜索证据：`rg -n -i "episodic|semantic|long[-_ ]?term|short[-_ ]?term|working memory|archive|knowledge"` 仅命中无关 remote long-term key、semantic highlight 和文档文本，没有命中 `crates/agent` 中的专用模型。

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

### 3.2 压缩链路：有 LLM 总结，但不是长期记忆

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

结论：这是上下文压缩，不是“自动化记忆”写库。未发现把 summary 写入 `memories`、向量库或独立 facts 表的代码。搜索证据：`rg -n -i "raw_memor|rollout|consolidat|extract_model|use_memories|generate_memories|memory_mode|embedding|vector|semantic|episodic|recall"` 只命中配置/UI 字段与无关内容；没有 `compact_session()` 调用 `ConversationRecorder::record` 或 memory store。

### 3.3 去重 / 合并 / 覆盖 / 衰减 / TTL / pin

已实现：

- active session TTL：`AgentSessionManager::cleanup_expired()` 按 `session_ttl_secs` 从内存移除 active session，证据 `crates/agent/src/session.rs:L310-L314`。
- JSONL 文件 90 天清理：IM Gateway startup 时在 `HistoryPersistence::Last90Days` 下调用 `cleanup_expired_sessions()`，证据 `crates/bifrost-admin/src/handlers/im_gateway.rs:L543-L556`；清理实现见 `crates/agent/src/persistence.rs:L495-L512`。
- session 文件名 sanitize，避免路径非法字符，证据 `crates/agent/src/persistence.rs:L566-L577`。
- session listing dedup active/history：`/agent/sessions/all` 用 active keys 跳过重复，证据 `crates/bifrost-admin/src/handlers/im_gateway.rs:L1844-L1905`。

未实现长期 memory 级去重、合并、覆盖、衰减、pin。搜索证据：`rg -n -i "dedup|dedupe|merge|expire|cleanup|retention|pin|ttl"` 在 agent 相关文件只命中 skills 去重、config merge、session TTL、history cleanup；没有 memory record 级策略。

### 3.4 敏感信息过滤 / 脱敏

当前对 provider 配置的 API response 有脱敏：`sanitize_provider()` “never expose secret_ref in plaintext”，证据 `crates/bifrost-admin/src/handlers/im_gateway.rs:L2506-L2515`。但 conversation recorder 写 user/tool/assistant 原文，没有通用 redact/mask：

- `record_user_message()` 直接写 `{"message": content}`，证据 `crates/agent/src/persistence.rs:L87-L95`。
- `record_tool_call_with_id()` 直接写 `arguments`，证据 `crates/agent/src/persistence.rs:L121-L139`。
- `record_tool_result_with_call_id()` 直接写 `result`，证据 `crates/agent/src/persistence.rs:L152-L172`。

`history::sanitize_chat_history()` 只修复 Chat Completions tool-call 序列，不是隐私脱敏，证据 `crates/agent/src/history.rs:L19-L25`。

### 3.5 用户显式“记住这件事”入口

未发现 `bifrost memory ...`、HTTP `/memory`、chat `/remember` 或 slash command。已存在的用户显式入口是 `/compact`（压缩）、`/resume`（恢复 JSONL history）、`/clear`/`/reset`、`/undo`、`/status`，证据 `crates/agent/src/session.rs:L474-L618`。搜索证据：`rg -n -i "bifrost memory|memory (add|set|delete|remove|list|search|recall|remember)|remember this|记住|召回"` 未命中真实 memory CRUD；命中的 `SKILL.md:796` 是“读取 + 记住 sha”的人工操作说明，不是产品入口。

## 4. 检索与注入

### 4.1 检索方式

已实现的“检索”只有：

- active session detail：从 `DashMap` 中取单个 session，证据 `crates/agent/src/session.rs:L348-L378`。
- JSONL 文件列表：递归扫 `data_dir/sessions/**/*.jsonl`，证据 `crates/agent/src/persistence.rs:L470-L493`、`crates/agent/src/persistence.rs:L514-L529`。
- JSONL replay：逐行解析事件，恢复成 `ChatMessage`，证据 `crates/agent/src/persistence.rs:L246-L345`。
- summary quick scan：逐行扫描事件提取 token、turn、source、work_dir，证据 `crates/agent/src/persistence.rs:L371-L468`。

没有关键词索引、正则搜索、全文索引、embedding、向量库、混合检索或 topK 召回。搜索证据：

```text
命令：rg -n -i "recall|retrieve|retrieval|top_k|topK|threshold|similarity|embedding|vector|semantic|episodic|knowledge|long[-_ ]term|short[-_ ]term|working memory|archive" crates/agent crates/bifrost-admin crates/bifrost-cli web/src design docs human_tests README.md
关键命中：crates/agent/src/config.rs:L65 auto compact threshold；crates/agent/src/compact.rs:L266-L273 compact threshold；web/src/components/BifrostEditor/index.ts semanticHighlighting；remote_invoke long_term_pubkey。未命中 memory retrieval/embedding/vector store。
```

### 4.2 召回策略

当前 prompt 注入策略是：每次模型请求调用 `build_messages(system_prompt)`，把 system prompt 和完整 sanitized history 拼成 Chat Completions messages；历史过长时通过 token/context budget 触发 compaction，只有 provider context-window overflow fallback 才会删除最老消息重试。

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

没有按用户/仓库/设备 scope 的 memory recall 过滤。已有隔离键是 `session_key` 和 `work_dir/source` 元数据，证据 `crates/agent/src/session.rs:L37-L64`、`crates/agent/src/persistence.rs:L371-L385`。

### 4.3 注入形式

注入形式是 Chat Completions messages：

- system prompt：`prompt::build_system_prompt(...)`，证据 `crates/agent/src/session.rs:L653-L655`。
- history：`session.history` 进入 `build_messages()`，证据 `crates/agent/src/session.rs:L694-L705`。
- compaction summary：作为 `ChatMessage::user(...)` 放到 history 第一条，证据 `crates/agent/src/compact.rs:L196-L204`。

`MemoriesConfig.use_memories` 的 UI 文案是“Inject memory usage instructions into developer prompts”，证据 `web/src/pages/Settings/tabs/AgentTab.tsx:L867-L884`；但源码中没有 `get_memories_config()` 的调用点。搜索证据：`rg -n "get_memories_config|use_memories|generate_memories|disable_on_external_context" crates/agent crates/bifrost-admin web/src` 仅命中配置定义、Admin PATCH 和 WebUI 表单。

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
| 长期记忆 CRUD | 未实现 | 搜索 `memory add/list/search/delete/export/import` 无真实入口 |

未发现导入/导出 memory 的专用 CLI/API。CLI `config memory` 是系统内存诊断，不是记忆管理，证据 `docs/cli.md:L301`、`crates/bifrost-cli/src/cli.rs:L1352-L1353`。

### 5.2 清理策略

- active session：`session_ttl_secs` 默认 3600 秒，证据 `crates/agent/src/config.rs:L323-L328`、`crates/agent/src/session.rs:L310-L314`。
- persisted history：配置可选 `HistoryPersistence::Last90Days`，startup 删除 90 天前 JSONL，证据 `crates/bifrost-admin/src/handlers/im_gateway.rs:L543-L556`、`crates/agent/src/persistence.rs:L495-L512`。
- `HistoryConfig.max_bytes` 字段存在，但在 `persistence.rs` 未见按文件大小裁剪实现。字段证据 `crates/agent/src/config.rs:L250-L255`；搜索 `rg -n "max_bytes" crates/agent crates/bifrost-admin` 仅命中配置/PATCH，没有命中 cleanup 写法。

### 5.3 多租户 / 多用户 / 多设备隔离

当前隔离边界：

- Feishu owner check：只处理 owner_open_id 发来的消息，证据 `crates/bifrost-admin/src/handlers/im_gateway.rs:L619-L650`。
- session key：使用 `event.source.user_id` 作为 per-user session key，证据 `crates/bifrost-admin/src/handlers/im_gateway.rs:L712-L727`、`crates/bifrost-admin/src/handlers/im_gateway.rs:L782-L799`。
- active session map：`DashMap<String, AgentSession>`，证据 `crates/agent/src/session.rs:L270-L281`。
- persisted history filename：session key 经过 `sanitize_key()` 写入路径，但 JSONL 内保留原始 session key，证据 `crates/agent/src/persistence.rs:L63-L64`、`crates/agent/src/persistence.rs:L410-L413`。

与 Remote Invoke / FileAccessPolicy / Shell Access 没有直接 memory 耦合；搜索 `rg -n -i "memory|memories|recall|remember" crates/bifrost-admin/src/remote_invoke crates/bifrost-core/src/file_access crates/agent` 中 remote_invoke 命中主要是 `long_term_pubkey`、`remember_bridge_seq` 或注释，不是 Agent memory。

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

- `MemoriesConfig` 没有对应单元测试证明字段对运行时生效；搜索 `rg -n "generate_memories|use_memories|max_raw_memories_for_consolidation|extract_model|consolidation_model" crates/agent/src/*` 只命中 config 定义。
- `event_types::COMPACTION` 声明和 history scan 已支持，但 turn loop 不写 compaction event；这会让 human_tests 预期“JSONL 包含 compaction”与代码存在 gap，证据 `crates/agent/src/persistence.rs:L33-L38`、`crates/agent/src/persistence.rs:L445-L451`、`crates/agent/src/session.rs:L512-L561`。
- 没有隐私脱敏测试覆盖 conversation recorder；写入方法直接持久化原文，证据 `crates/agent/src/persistence.rs:L87-L172`。
- 没有 memory recall/topK/embedding 测试，因为对应实现不存在。搜索证据见附录 A。

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

### 8.2 设计了但未实现 / 未完全实现

- `MemoriesConfig` 字段显示计划支持 memories，但没有实际 memory extraction/consolidation/recall 实现，证据 `crates/agent/src/config.rs:L262-L295` 与搜索命令 `rg -n -i "raw_memor|rollout|consolidat|extract_model|use_memories|generate_memories|memory_mode|embedding|vector|semantic|episodic|recall"` 的结果。
- `event_types::COMPACTION` 和 `scan_session_summary()` 识别 compaction，但 `compact_session()` 不写 JSONL event；证据 `crates/agent/src/persistence.rs:L33-L38`、`crates/agent/src/persistence.rs:L445-L451`、`crates/agent/src/compact.rs:L218-L243`。
- `HistoryConfig.max_bytes` 字段存在，但未见文件大小裁剪实现；证据 `crates/agent/src/config.rs:L250-L255`，搜索 `rg -n "max_bytes" crates/agent crates/bifrost-admin` 未发现清理逻辑。
- `design/im-gateway-agent.md` 较早段落仍有旧结构描述 `ImAgentSessionManager` / `Session { messages: Vec<ChatMessage> }`，与当前 `crates/agent/src/session.rs` 的 `AgentSession` / `ConversationRecorder` 不完全一致，证据 `design/im-gateway-agent.md:L123-L145` 与 `crates/agent/src/session.rs:L32-L69`。

### 8.3 实现了但未充分文档化

- `ConversationRecorder` 的 JSONL 路径、事件 schema、恢复规则主要在代码注释和 human_tests 中，README 仅有 Agent Skill 链接和资源内存诊断，证据 `README.md:L31-L31`、`README.md:L143-L143`、`crates/agent/src/persistence.rs:L1-L4`。
- `/agent/sessions/history/{path}` 允许按 URL encoded 文件路径读取/删除 history 文件；在 WebUI 中使用，但缺少面向用户的 API 文档，证据 `crates/bifrost-admin/src/handlers/im_gateway.rs:L1992-L2036`。
- `history::sanitize_chat_history()` 是关键稳定性层，但设计文档只在 Agent Loop 章节描述，未形成独立 API/安全文档，证据 `design/im-gateway-agent.md:L50-L60`、`crates/agent/src/history.rs:L19-L92`。

## 9. 风险与建议

1. **P0：明确产品语义，避免把 compaction 误称为长期记忆。** 当前 UI 名称是 `Memories`，但运行时只有 session compaction/history；建议短期改文案或补实现。证据：UI `Memories` 配置见 `web/src/pages/Settings/tabs/AgentTab.tsx:L829-L1059`，运行时压缩见 `crates/agent/src/compact.rs:L111-L125`。
2. **P0：补齐或移除未接入的 `MemoriesConfig`。** `generate_memories/use_memories/extract_model/consolidation_model` 当前会被保存但不影响任何链路，容易形成“开关已打开但无效”的错觉。证据：配置定义 `crates/agent/src/config.rs:L262-L295`，PATCH `crates/bifrost-admin/src/handlers/im_gateway.rs:L2253-L2305`。
3. **P0：如果要实现自动化长期记忆，先落数据模型和存储边界。** 至少需要 `MemoryRecord{id, content, source_session, scope, created_at, updated_at, last_used_at, confidence, tags, pinned}`、存储介质、迁移、删除策略和敏感信息策略。当前没有 `MemoryRecord/MemoryStore` 命中，搜索证据见附录 A。
4. **P1：为 conversation recorder 增加敏感信息过滤。** 现在 user/tool/result 原文直接 JSONL 落盘，工具参数或输出可能包含 token、路径、秘钥。证据 `crates/agent/src/persistence.rs:L87-L172`。
5. **P1：压缩事件应写入 JSONL 或删除 `COMPACTION` 事件类型预期。** 当前 event type 与 summary scan 支持 compaction，但 turn loop 不写，导致观测数据不完整。证据 `crates/agent/src/persistence.rs:L33-L38`、`crates/agent/src/persistence.rs:L445-L451`。
6. **P1：修复 `/resume` 的 data_dir 选择。** `run_turn_with_mcp()` 中 `/resume` 用 `config.resolve_work_dir()` 查找 conversations，而正常 recorder 写入 `agent_home_dir()`；这可能导致恢复找不到默认 JSONL。证据 `crates/agent/src/session.rs:L587-L590` 对比 `crates/bifrost-admin/src/handlers/im_gateway.rs:L871-L872`、`crates/bifrost-admin/src/handlers/im_gateway.rs:L2110-L2111`。
7. **P1：实现 `HistoryConfig.max_bytes` 或从配置中移除。** 字段存在但未见执行逻辑，会误导用户。证据 `crates/agent/src/config.rs:L250-L255`。
8. **P2：长期记忆检索若落地，应优先从关键词/全文 + scope 过滤开始，再评估 embedding。** 当前没有检索基础设施；直接上向量库会引入隐私、迁移、成本和可解释性问题。搜索证据见附录 A。
9. **P2：把 session history API 文档化并加路径安全约束说明。** `GET/DELETE /agent/sessions/history/{path}` 接收 encoded path 后直接 `Path::new` + 读/删，建议明确限制在 agent home 或增加 canonicalize 校验。证据 `crates/bifrost-admin/src/handlers/im_gateway.rs:L1992-L2036`。
10. **P2：同步更新 `design/im-gateway-agent.md` 的旧结构段落。** 文档 L123-L145 仍描述旧的 session manager 类型，当前实现已迁移到 `crates/agent`。证据 `design/im-gateway-agent.md:L123-L145`、`crates/agent/src/session.rs:L32-L69`。

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
