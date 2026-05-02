# Bifrost Long-term Memory 长期方案

## 目标

- 为 Bifrost Agent 提供跨 session、跨 chat、跨设备且按用户隔离的长期记忆。
- 支持自动沉淀、显式沉淀、召回注入、WebUI 管理、JSONL 导入导出、脱敏、GC 与后续 consolidation/embedding 扩展。
- 记忆系统必须是长期架构骨架：抽取、召回、存储、脱敏、管理 API 互相解耦，后续替换召回策略或加入 embedding 时不推倒重来。

## 非目标

- 本期不接入任何网络 embedding 服务，也不调用 OpenAI/Cohere/Voyage embedding。
- 本期不做真实 LLM consolidation 合并，只保留 trait 与配置消费点，并落日志。
- 本期不把 relay/sync-server 改成记忆传输层；跨设备迁移先通过 JSONL 导入导出。

## 参考与差异

实际探查命令：

```text
ls -la ~/.codex/ ~/.codex/memories/ ~/.claude/ ~/.claude/memories/ ~/.claude/projects/ 2>/dev/null
head -50 ~/.codex/memories/memory_summary.md
head -50 ~/.codex/memories/MEMORY.md
head -80 ~/.codex/config.toml
```

观察证据：

- `~/.codex/config.toml` 存在 `[features] memories = true` 与 `[memories] no_memories_if_mcp_or_web_search = true`。Bifrost 对应保留 `MemoriesConfig`，但要把字段全部接到抽取、召回、GC 与 consolidation 占位流程。
- `~/.codex/memories/` 是 git 化目录，包含 `MEMORY.md`、`memory_summary.md`、`raw_memories.md`、`rollout_summaries/`、`skills/`。Codex 的主路径偏 Markdown + rollout consolidation，适合人读和上下文压缩，不适合作为 Bifrost WebUI 高频增删改查主存储。
- `~/.codex/memories/MEMORY.md` 按 task group、scope、rollout_summary_files、keywords、user preferences 组织，说明“可召回事实”需要携带 scope、来源、关键词/tag 与可追溯来源。
- 本机 `~/.claude/` 只有 `skills/`，没有 `~/.claude/memories/` 或 `~/.claude/projects/`。因此 Claude Code 的 `CLAUDE.md` 分层记忆模型只作为概念参考：Global/User/Project/Session 分层优先级，不照搬本机不存在的目录形态。

Bifrost 本土化差异：

- 主存储采用 SQLite，文件在 `$agent_home/memory/memories.sqlite`，适合 Admin API、WebUI 管理、FTS5 搜索和事务去重。
- JSONL 只作为导入导出与 tombstone 审计，不作为在线主存储。
- scope 固化为 `Global / User(user_id) / Project(path_hash) / Session(session_key)`，召回时按照更具体 scope 优先。

## Crate 划分

决策：新增 `crates/memory`。

理由：

- `crates/agent` 负责 turn loop、prompt、tool 与模型调用；记忆存储和 WebUI API 不应被耦合进 session 主循环。
- `crates/bifrost-admin` 需要直接暴露管理 API 和 stats，如果记忆只放在 agent 子模块会形成 admin 反向依赖 agent 内部实现。
- `crates/memory` 只依赖 serde/rusqlite/regex/tracing/uuid/time，不依赖 admin 或 agent；agent 通过 trait 使用抽取与召回，admin 通过同一个 store 暴露 HTTP。

## 数据模型

```rust
pub struct MemoryRecord {
    pub id: MemoryId,
    pub scope: MemoryScope,
    pub kind: MemoryKind,
    pub content: String,
    pub source: MemorySource,
    pub tags: Vec<String>,
    pub pinned: bool,
    pub confidence: f32,
    pub created_at: u64,
    pub updated_at: u64,
    pub last_used_at: Option<u64>,
    pub use_count: u32,
    pub expires_at: Option<u64>,
    pub dedupe_hash: String,
}

pub enum MemoryScope { Global, User(String), Project(String), Session(String) }
pub enum MemoryKind { Fact, Preference, Rule, Skill, TaskContext, Other }
pub enum MemorySource {
    AutoExtract { session_key: String, turn: u64 },
    UserExplicit,
    Import,
    Seed,
}
```

## SQLite schema v1

```sql
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS schema_version (
  version INTEGER PRIMARY KEY,
  applied_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS memories (
  id TEXT PRIMARY KEY,
  scope_kind TEXT NOT NULL,
  scope_type TEXT NOT NULL,
  scope_value TEXT,
  kind TEXT NOT NULL,
  content TEXT NOT NULL,
  source_json TEXT NOT NULL,
  tags_json TEXT NOT NULL,
  pinned INTEGER NOT NULL DEFAULT 0,
  confidence REAL NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  last_used_at INTEGER,
  use_count INTEGER NOT NULL DEFAULT 0,
  expires_at INTEGER,
  dedupe_hash TEXT NOT NULL,
  deleted_at INTEGER
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_memories_scope_dedupe
  ON memories(scope_kind, dedupe_hash)
  WHERE deleted_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_memories_scope_last_used
  ON memories(scope_kind, last_used_at DESC, updated_at DESC)
  WHERE deleted_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_memories_kind
  ON memories(kind)
  WHERE deleted_at IS NULL;

CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
  id UNINDEXED,
  content,
  tags,
  tokenize = 'unicode61'
);
```

`scope_kind` 使用稳定字符串：`global:*`、`user:<id>`、`project:<path_hash>`、`session:<session_key>`，用于唯一索引和查询。

## 抽取流程

```text
assistant turn 完成
  |
  v
检查 generate_memories / disable_on_external_context / skip_threshold_bytes
  |
  v
MemoryExtractor.extract(turn transcript, extract_model)
  |
  v
严格 JSON 数组 candidates
  |
  v
redact -> normalize -> tags sanitize -> dedupe_hash
  |
  +-- 命中 (scope_kind, dedupe_hash) -> use_count++, updated_at, last_used_at
  |
  +-- 未命中 -> INSERT memories + memories_fts
```

prompt 文件固定放在 `crates/memory/prompts/extract.md`，agent 读取后传给 `AgentClient::chat_completion`。

## 召回流程

```text
build_messages() 前
  |
  v
检查 use_memories == Some(false)
  |
  v
构造 RecallContext(user_id, project_path, session_key, latest_user_message, max_items, max_chars)
  |
  v
FTS/关键词 + scope 过滤 + pinned/specificity/time/rank 排序
  |
  v
截断到 topK 与 max_chars
  |
  v
插入 system message:
  # Long-term memories (per user)
  - [kind scope tags] content
  |
  v
mark_used(ids): use_count++, last_used_at=now
```

## GC / consolidation 流程

```text
启动或 12h interval
  |
  v
读取 MemoriesConfig max_unused_days / max_raw_memories_for_consolidation / max_rollout_age_days
  |
  v
软删除: 非 pinned 且 unused >= max_unused_days
  |
  v
写 tombstones.jsonl
  |
  v
若 consolidation_model 配置存在 -> 调用 Consolidator trait 占位日志
```

本期 GC 至少实际执行软删除；consolidation 只记录配置已消费、候选数量和跳过原因。

## 导入导出流程

```text
导出: query active records -> JSONL -> $agent_home/memory/exports/<ts>.jsonl
导入: read JSONL -> validate -> redact -> normalize -> dedupe -> insert/update
```

导入不信任外部内容，仍然执行脱敏和 tag 规范化。

## 隐私脱敏流程

```text
candidate/explicit/import content
  |
  v
Redactor regex pass
  |
  v
normalize whitespace
  |
  v
dedupe_hash
```

规则覆盖：

- `sk-[A-Za-z0-9]{20,}`
- `ghp_[A-Za-z0-9_]{20,}`
- `AIza[A-Za-z0-9_-]{20,}`
- `Bearer\s+[A-Za-z0-9\._-]+`
- 长度 >= 32 的 base64
- `password=...`
- `token=...`
- `api[_-]?key\s*[:=]\s*...`
- `BF-[A-F0-9]{16}`

## Rust API

```rust
pub trait MemoryStore {
    fn insert(&self, record: NewMemoryRecord) -> Result<MemoryRecord>;
    fn update(&self, id: &MemoryId, patch: MemoryPatch) -> Result<MemoryRecord>;
    fn soft_delete(&self, id: &MemoryId) -> Result<bool>;
    fn get(&self, id: &MemoryId) -> Result<Option<MemoryRecord>>;
    fn search(&self, query: MemorySearchQuery) -> Result<Vec<MemoryRecord>>;
    fn mark_used(&self, ids: &[MemoryId], now: u64) -> Result<usize>;
    fn stats(&self, now: u64) -> Result<MemoryStats>;
    fn export_jsonl(&self, writer: impl Write) -> Result<usize>;
    fn import_jsonl(&self, reader: impl BufRead) -> Result<ImportReport>;
    fn gc(&self, policy: GcPolicy) -> Result<GcReport>;
}

pub trait MemoryExtractor {
    async fn extract(&self, request: ExtractRequest) -> Result<Vec<MemoryCandidate>>;
}

pub trait MemoryRecaller {
    fn recall(&self, context: RecallContext) -> Result<Vec<MemoryRecord>>;
}

pub trait MemoryConsolidator {
    async fn consolidate(&self, request: ConsolidationRequest) -> Result<ConsolidationReport>;
}
```

## HTTP API

- `GET /agent/memories`：分页列表，支持 scope/kind/tag/query。
- `POST /agent/memories`：显式创建，复用 Redactor。
- `PATCH /agent/memories/:id`：编辑 content/kind/tags/pinned/expires_at。
- `DELETE /agent/memories/:id`：软删除。
- `POST /agent/memories/search`：FTS 搜索。
- `POST /agent/memories/import`：JSONL 导入。
- `GET /agent/memories/export`：JSONL 导出。
- `GET /agent/memories/stats`：总数、按 scope/kind 分布、近 7 天写入/召回次数。

鉴权复用已有 Admin API auth，不新增认证体系。

## TS 类型

同步到 `web/src/pages/Settings/tabs/agent/types.ts`：

```ts
export type MemoryScopeType = 'global' | 'user' | 'project' | 'session';
export type MemoryKind = 'fact' | 'preference' | 'rule' | 'skill' | 'task_context' | 'other';
export interface MemoryRecord { id: string; scope: MemoryScope; kind: MemoryKind; content: string; tags: string[]; pinned: boolean; confidence: number; created_at: number; updated_at: number; last_used_at?: number; use_count: number; expires_at?: number; }
```

## WebUI 线框

位置：Settings -> Agent -> Memories 独立页面。

布局：

```text
Agent Settings
  Sessions | Skills | MCP | Instructions | Memories

Memories
  Toolbar: Search [      ] Scope [all] Kind [all] Tag [all] New Import Export
  Main list:
    Pin | Kind | Scope | Content preview | Tags | Updated | Last used | Actions(Edit/Delete)
  Side panel / modal:
    Content textarea
    Scope selector
    Kind selector
    Tags input
    Pinned toggle
  Recent recalled:
    Top N by last_used_at
```

颜色使用现有 CSS 变量，亮色/暗色主题都要验证。

## Hook 点

- `ConversationRecorder`：新增 `record_compaction()`，修复 `event_types::COMPACTION` 只定义不写入的问题。
- `compact_session()`：成功后由调用方或返回结果触发 recorder 写 compaction 事件。
- `run_turn_with_mcp()`：
  - 内置命令层增加 `/remember <text>`、`/memories`、`/forget <id|last>`。
  - pre-turn compaction 后、模型请求前构造 `RecallContext`，把召回 system message 插入主 system prompt 后。
  - assistant final response 记录后触发 auto-extract。
- `build_messages()`：保持纯函数形态，增加可选 extra system memories 参数，避免直接访问 DB。
- IM Gateway：已有 `process_agent_chat` 走 `run_turn_with_mcp()`，因此不单独实现记忆逻辑，只保证 session_key/user_id/source 传入 RecallContext。
- `/resume`：统一从 `agent_home_dir()` 查找 session JSONL，若旧 work_dir 路径存在则 fallback 并写日志。
- `HistoryConfig.max_bytes`：优先实装历史文件写入后的大小修剪，保留配置字段。

## MemoriesConfig 接线计划

- `use_memories`：`Some(false)` 时召回完全短路，不打开 SQLite、不注入 system message。
- `generate_memories`：`Some(false)` 时自动抽取完全短路，显式 `/remember` 和 WebUI 创建仍可用。
- `disable_on_external_context`：用户消息超过 `extractor.skip_threshold_bytes` 默认 8KB 时，本轮不自动抽取。
- `max_raw_memories_for_consolidation`：GC/consolidation 选择候选上限。
- `max_unused_days`：非 pinned 且 `last_used_at/updated_at` 超过阈值时软删除。
- `max_rollout_age_days`：consolidation 候选时间窗，本期记录并用于候选查询。
- `max_rollouts_per_startup`：启动 consolidation 扫描上限。
- `min_rollout_idle_hours`：只处理空闲超过该时长的 raw/session 候选。
- `extract_model`：传入 LLM 抽取调用；为空复用当前 agent model。
- `consolidation_model`：传入 `MemoryConsolidator` 占位日志。

## 测试计划

单元测试：

- Redactor 每条规则各一例，并验证无敏感内容时不误替换。
- normalize/dedupe：空白归一、大小写 tag、重复插入更新 use_count。
- SqliteMemoryStore：insert/update/delete/get/search/stats/export/import，每个方法正常 + 边界。
- Recall 排序：pinned 优先、Project/User/Global specificity、last_used_at/updated_at、max_chars 截断。
- Extractor 合约：mock completion 输出 JSON 数组、空数组、非法 JSON。

E2E：

- 新增 `e2e-tests/tests/test_long_term_memory_remember_recall.sh`，使用 mock Chat Completions 跑 `/remember -> 新 session -> 召回注入 prompt`。

human_tests：

- 新增 `human_tests/long-term-memory.md`，至少 10 条用例覆盖 `/remember`、`/memories`、`/forget`、自动抽取、召回注入、WebUI 增删改查、导入导出、GC、隐私脱敏、关闭召回。
- 更新 `human_tests/readme.md` 索引。
- 文档创建后立即逐条执行，记录实际结果。

项目校验：

- `cargo fmt --all`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo check --workspace`
- `cargo test -p memory -p bifrost-agent -p bifrost-admin`
- `cargo test --workspace --all-features`
