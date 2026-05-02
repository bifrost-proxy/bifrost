# Skill Creator — Bifrost Agent Loop 技能创建子系统设计

Status: Proposal (approved, pending implementation)
Owners: bifrost-agent, bifrost-admin, webui
Related: `design/long-term-memory.md`, `crates/agent/src/session.rs`, `crates/bifrost-admin/src/handlers/im_gateway.rs` (`SkillsManager`)

---

## 1. 背景与目标

### 1.1 现状

Bifrost Agent Loop 目前对 "skill" 的支持是**被动只读发现**：

- `SkillsManager`（`crates/bifrost-admin/src/handlers/im_gateway.rs`）在进程启动时扫描 `~/.bifrost/skills/` 和工作区目录，把 `SKILL.md` frontmatter 读进内存。
- `GET /agent/skills` 返回只读清单。
- `crates/agent/src/prompt.rs` 把 enabled skill 的 `name + description` 渲染进 system prompt 的 "Available skills" 块。
- Agent Loop 的斜杠命令（`/clear /reset /undo /compact /status /resume /remember /memories /forget`）在 `session.rs` 里是**硬编码 match**，skill 无法声明自己的 `/xxx`。
- Skill 如何实际执行、如何访问工具、如何访问长期记忆，**没有任何原语**。
- WebUI 侧 `AgentTab` 没有 skill 管理页；用户必须手动到磁盘摆 SKILL.md 才能"教" Agent 新能力。

### 1.2 目标

1. 在 Agent Loop 内新增 **Skill Creator** 能力：用户/Agent 在会话中即可完成 Skill 的 **create / edit / test / register / invoke / delete**，无需离开 WebUI 或聊天。
2. Skill 成为**一等公民**：有独立 crate (`crates/skills`)、数据模型、存储层、注册表、执行器、校验器、REST CRUD、WebUI 管理页。
3. Skill 可以**声明自己的 tools**（本期新增能力，不再受限于现有 ToolRegistry）。
4. Skill 可以**访问长期记忆**（通过 `allowed_tools` 白名单里的 `memory.read` / `memory.write`）。
5. Skill 可以声明**自己的斜杠命令**（slash_command），被 `SlashCommandRouter` 动态注册。
6. **同名同 scope 只保留一个 active 版本**，老版本进 `.history/` 归档，简化 Registry。
7. IM Gateway 场景下 Skill 执行**不需审批**（本期决定）。
8. **一次到位，不分期**：本设计涵盖从数据模型到 WebUI 的全链路。

### 1.3 非目标

- 不做远程 Skill 市场 / 分发（留给后续 `bifrost-skill-market`）。
- 不做 OS 级 sandbox（本期依赖子进程 + 环境隔离 + FileAccessPolicy）。
- 不做多版本并行 active（只保留一个 active + 归档历史）。
- 不做 Skill 级 RBAC（单 installation 范围内所有调用同权）。

---

## 2. 架构总览

### 2.1 分层

```
┌──────────────────────────────────────────────────────────────┐
│ WebUI: Settings → Agent → Skills                             │
│  - SkillsSection (list + 启用开关)                           │
│  - SkillCreatorWizard (向导：元信息→entrypoint→tools→test)  │
│  - SkillEditor (Monaco)                                      │
└──────────────────────────┬───────────────────────────────────┘
                           │ REST
┌──────────────────────────▼───────────────────────────────────┐
│ bifrost-admin: handlers/agent_skills.rs (NEW)                │
│   GET    /agent/skills                 列表                  │
│   GET    /agent/skills/:name           详情 (含 SKILL.md)    │
│   POST   /agent/skills                 创建（manifest+资产） │
│   PATCH  /agent/skills/:name           更新 / enable-disable │
│   DELETE /agent/skills/:name           删除（归档到 history） │
│   POST   /agent/skills/:name/test      干跑（dry run）       │
│   POST   /agent/skills/:name/package   导出 .skill zip       │
│   POST   /agent/skills/import          导入 .skill zip       │
│   POST   /agent/skills/validate        仅校验，不落盘         │
└──────────────────────────┬───────────────────────────────────┘
                           │
┌──────────────────────────▼───────────────────────────────────┐
│ crates/skills (NEW crate)                                    │
│  ├─ model.rs         SkillManifest / Entrypoint / TriggerRule│
│  ├─ store.rs         磁盘读写 + atomic commit + checksum     │
│  ├─ registry.rs      内存索引 + 热更新 + slash_command 注册  │
│  ├─ validator.rs     YAML + JSONSchema + allowed_tools 校验 │
│  ├─ executor.rs      子进程起跑 + 超时/mem limit + tool 桥接│
│  ├─ packager.rs      .skill zip 打包/解包/checksum           │
│  ├─ authoring.rs     SkillAuthoringSession 状态机            │
│  └─ tool_bridge.rs   Skill 声明的 tools 与 ToolRegistry 桥接│
└──────────────────────────┬───────────────────────────────────┘
                           │
┌──────────────────────────▼───────────────────────────────────┐
│ crates/agent                                                 │
│  session.rs                                                  │
│    - SlashCommandRouter 取代 L491 硬编码 match              │
│    - 内置命令 → router.register_builtin()                   │
│    - Skill 的 slash_command → router.register_skill()       │
│  prompt.rs                                                   │
│    - "Available skills" 块读 SkillRegistry::enabled()       │
│    - 只渲染 name + description (progressive disclosure)     │
│  memory_runtime.rs                                           │
│    - 暴露 memory.read / memory.write 作为 Skill-scoped tool │
│  tools/skill_creator.rs (NEW)                               │
│    - meta-tool: skill_creator.{start, interview, draft,     │
│      test, commit, cancel, list_templates}                  │
└──────────────────────────────────────────────────────────────┘
```

### 2.2 模块依赖图

```
bifrost-admin ──▶ crates/skills ──▶ crates/memory
      │                │                  ▲
      │                ├──▶ crates/agent ─┘
      │                │        ▲
      ▼                │        │
  handlers/        (prompt/    session.rs
  agent_skills      session     uses
                    uses         SkillRegistry)
                    registry)
```

`crates/skills` 不依赖 `crates/agent`（避免循环）；反向 `agent` 依赖 `skills`。`skills` 依赖 `memory` 以便通过 `tool_bridge` 暴露 memory 读写能力。

---

## 3. 数据模型

### 3.1 SkillManifest（Rust）

位于 `crates/skills/src/model.rs`：

```rust
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SkillManifest {
    /// kebab-case, ^[a-z][a-z0-9-]{0,63}$
    pub name: String,
    /// semver
    pub version: String,
    /// <= 1024 chars; 描述匹配触发
    pub description: String,
    pub scope: SkillScope,
    pub entrypoint: Entrypoint,
    /// Skill 被允许使用的工具白名单（本期允许声明 Skill 自己的 tools，也可引用 ToolRegistry 内置）
    pub allowed_tools: Vec<ToolBinding>,
    /// 可选 "/xxx" 斜杠命令; 不可与内置冲突
    pub slash_command: Option<String>,
    /// 描述匹配 / 关键词 / regex 触发规则
    pub triggers: Vec<TriggerRule>,
    pub inputs_schema: Option<serde_json::Value>,  // JSON Schema
    pub outputs_schema: Option<serde_json::Value>, // JSON Schema
    pub metadata: BTreeMap<String, String>,
    pub created_by: SkillAuthor,
    pub created_at_unix: i64,
    pub updated_at_unix: i64,
    /// 整个 skill 目录的 sha256 (排除 manifest.json 本身)
    pub checksum: String,
    /// manifest 版本号, 当前固定 1
    pub schema_version: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillScope {
    Global,   // ~/.bifrost/skills/
    User,     // ~/.bifrost/skills/users/<user>/
    Project,  // <workspace>/.bifrost/skills/
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Entrypoint {
    /// 声明式: skill 本体只是一段提示词 + 规则, 无需执行外部进程
    Inline { instructions_md: String },
    /// shell 脚本
    Shell { script: PathBuf, shell: ShellKind },
    /// python 脚本 (同目录 requirements.txt 可选)
    Python { script: PathBuf, python: Option<String> },
    /// node 脚本 (同目录 package.json 可选)
    Node { script: PathBuf },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellKind { Bash, Sh, Zsh, PowerShell }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolBinding {
    /// 引用 ToolRegistry 中已有工具
    Registry { name: String },
    /// 引用 MCP server 暴露的 tool
    Mcp { server: String, tool: String },
    /// 访问长期记忆
    Memory { op: MemoryOp },
    /// Skill 自己声明的 tool (本期允许)
    /// JSON Schema 描述 input; 执行时由 Skill entrypoint 通过 stdin/stdout 协议响应
    Owned { name: String, description: String, input_schema: serde_json::Value },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryOp { Read, Write, Both }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TriggerRule {
    DescriptionMatch,            // 默认: LLM 依据 description 自行决定
    Keyword { any_of: Vec<String> },
    Regex { pattern: String },
    SlashCommand,                // 等价于 slash_command.is_some()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillAuthor {
    User { id: String },
    Agent { session_id: String },
    Imported { origin: String },
}
```

### 3.2 磁盘布局

`<scope_root>/<name>/` 目录:

```
<scope_root>/<name>/
  SKILL.md               # 人写的 source of truth; YAML frontmatter + Markdown 正文
  manifest.json          # 由 validator 生成; 机器可读; 带 checksum
  scripts/               # entrypoint 指向的脚本及其辅助文件
  references/            # 可选: 供 LLM 按需加载的参考文档
  assets/                # 可选: 模板、样例、图片
  .history/              # 被替换/删除的旧版本归档 (manifest.json + tarball)
```

**`SKILL.md` frontmatter 与 `manifest.json` 关系**：
- `SKILL.md` 是 source of truth，用户/Agent 直接编辑。
- 每次 write 触发 `Validator::regenerate_manifest()` 重写 `manifest.json`，包含 checksum。
- Registry 热加载时以 `manifest.json` 为索引，`SKILL.md` 正文只在 skill 被"拉进对话"时按需 load。

`SKILL.md` frontmatter 字段与 `SkillManifest` 一一对应，但用 YAML 写：

```yaml
---
name: weather-lookup
version: 0.1.0
description: Fetch current weather for a city via wttr.in.
scope: global
entrypoint:
  kind: shell
  script: scripts/run.sh
  shell: bash
allowed_tools:
  - kind: registry
    name: http.fetch
  - kind: memory
    op: read
slash_command: /weather
triggers:
  - kind: description_match
  - kind: keyword
    any_of: [weather, 天气]
inputs_schema:
  type: object
  properties:
    city: { type: string }
  required: [city]
---

# Weather Lookup Skill

This skill fetches current weather ...
```

### 3.3 scope 叠加规则

Registry 加载顺序：`Global` → `User` → `Project`。

- 同名冲突时**后者覆盖前者**（Project > User > Global）。
- 对用户完全透明：`GET /agent/skills` 返回时附带 `effective_scope` 字段，WebUI 展示冲突徽标。
- Delete 时只删当前 scope 的版本；如果更高优先级 scope 还有同名，Registry 自动暴露那个。

---

## 4. 核心子系统详解

### 4.1 `SkillStore`（`crates/skills/src/store.rs`）

职责：**唯一的磁盘读写接入点**。

核心 API：

```rust
pub struct SkillStore { roots: Vec<ScopeRoot> }

impl SkillStore {
    pub fn read_all(&self) -> Result<Vec<SkillRecord>>;
    pub fn read_one(&self, scope: SkillScope, name: &str) -> Result<SkillRecord>;
    /// 原子写入: 写 draft/ 目录 -> validator -> 生成 manifest.json
    ///          -> 计算 checksum -> atomic rename 到正式目录
    /// 若同名已存在, 把旧目录 tar 后移入 .history/<name>-<ts>.tar.zst
    pub fn commit(&self, draft: SkillDraft) -> Result<SkillRecord>;
    /// 删除: 归档到 .history/ 再移除正式目录
    pub fn delete(&self, scope: SkillScope, name: &str) -> Result<()>;
    pub fn enable(&self, scope: SkillScope, name: &str, enabled: bool) -> Result<()>;
    pub fn verify_checksum(&self, scope: SkillScope, name: &str) -> Result<bool>;
}
```

原子性实现：
1. 所有写入先落到 `<scope_root>/.drafts/<authoring-session-id>/<name>/`。
2. `commit()` 在 draft 中校验通过后，先归档旧目录（如果存在），再 `rename(draft, final)`。
3. 失败路径：保留 draft，返回错误；`.drafts/` 有 TTL（7 天），过期清理。

### 4.2 `SkillValidator`（`crates/skills/src/validator.rs`）

校验规则（每条都是硬约束）：

1. **name**: `^[a-z][a-z0-9-]{0,63}$`；不可与内置保留词冲突（`system, agent, memory, tools, admin`）。
2. **version**: 合法 semver。
3. **description**: 1 ≤ len ≤ 1024。
4. **slash_command**:
   - 必须 `/` 开头，`^/[a-z][a-z0-9-]{0,31}$`。
   - 不可与 `SlashCommandRouter` 已注册的内置命令冲突。
   - 不可在同一 scope 下重复。
5. **allowed_tools**:
   - `Registry { name }`：`ToolRegistry::has(&name)` 必须为 true。
   - `Mcp { server, tool }`：MCP server 必须在当前配置里，且暴露该 tool。
   - `Owned`：`input_schema` 必须是合法 JSON Schema（draft-07）。
   - `Memory`：本期无额外约束（IM Gateway 场景下也不需审批）。
6. **entrypoint**:
   - 文件必须存在于 skill 目录下、不可引用 `..` / 绝对路径跳出。
   - `Shell`: shell 必须在目标 OS 可用；`PowerShell` 仅 Windows。
   - `Python`: 若声明 `python`，必须 `command -v` 验证；否则用系统默认。
   - `Node`: 必须存在 `node` 可执行。
7. **inputs_schema** / **outputs_schema**: 若给定，必须是合法 JSON Schema。
8. **triggers**: 至少 1 条；若含 `SlashCommand`，必须 `slash_command.is_some()`。
9. **checksum**: 对 skill 目录（排除 `manifest.json` 自身和 `.history/`）做稳定 sha256（文件路径升序 + `<path>\0<content>` 串接后 hash）。

### 4.3 `SkillRegistry`（`crates/skills/src/registry.rs`）

内存索引 + 热更新 + slash_command 桥接。

```rust
pub struct SkillRegistry {
    by_name: HashMap<String, SkillRecord>,
    by_slash: HashMap<String, String>,      // "/weather" -> "weather-lookup"
    enabled: HashSet<String>,
    store: Arc<SkillStore>,
    watcher: notify::RecommendedWatcher,    // inotify / FSEvents / ReadDirectoryChangesW
}

impl SkillRegistry {
    pub fn init(store: Arc<SkillStore>) -> Result<Self>;
    pub fn list(&self) -> Vec<&SkillRecord>;
    pub fn enabled(&self) -> Vec<&SkillRecord>;
    pub fn resolve_slash(&self, cmd: &str) -> Option<&SkillRecord>;
    pub fn reload_one(&self, name: &str) -> Result<()>;
    pub fn reload_all(&self) -> Result<()>;
}
```

热更新：
- FS watcher 监听三个 scope_root；debounce 500ms；文件变动触发 `reload_one`。
- `commit()` 成功后主动调 `reload_one` 避免等 watcher。
- 并发安全：`RwLock<Inner>`。

### 4.4 `SlashCommandRouter`（`crates/agent/src/slash.rs`，新增；替换 `session.rs` L491 硬编码 match）

```rust
pub struct SlashCommandRouter {
    builtin: HashMap<String, BuiltinHandler>,
    skills:  Arc<SkillRegistry>,
}

impl SlashCommandRouter {
    pub fn register_builtin(&mut self, name: &str, handler: BuiltinHandler);
    pub fn dispatch(&self, cmd: &str, ctx: &mut TurnContext) -> Dispatch;
}

pub enum Dispatch {
    Handled(SessionResponse),
    RunSkill(SkillRecord, SkillInvocation),
    NotACommand,
    Unknown(String),
}
```

内置命令在 `AgentSession::new` 时注册：`/clear /reset /undo /compact /status /resume /remember /memories /forget /skill` （新增 `/skill` 作为 skill-creator 快捷入口，等价于调 `skill_creator.start`）。

### 4.5 `SkillExecutor`（`crates/skills/src/executor.rs`）

子进程模式 + tool 桥接协议。

**生命周期**：
1. `execute(record, input)` 启动子进程，cwd = skill 目录。
2. stdin 写入 JSON：`{ "input": ..., "tool_ack_id": null }`，行分隔。
3. 子进程在 stdout 按行写 JSON 事件：
   - `{ "type": "log", "level": "info", "message": "..." }`
   - `{ "type": "tool_call", "id": "...", "name": "http.fetch", "arguments": {...} }`
   - `{ "type": "output", "data": ... }`
   - `{ "type": "done" }`
4. Executor 收到 `tool_call` → 在 `allowed_tools` 里校验 → 转发给 `ToolRegistry` / `MemoryRuntime` / `MCPRouter` → 结果回写给子进程 stdin：`{ "tool_ack_id": "...", "result": ... }`。
5. `Inline` entrypoint 不起子进程，直接把 `instructions_md` 作为动态系统提示追加到当前 turn。

**隔离与限制**：
- cwd 固定为 skill 目录；env 白名单 + 目标机 OS 必要项。
- 超时：默认 30s，SkillManifest 可 override（上限 10min）。
- mem limit：rlimit_as（Linux/macOS）/ Job Object（Windows），默认 512MB。
- stdout/stderr 总量上限 4MB；溢出截断并 log。
- **禁止网络出站**由 entrypoint 声明决定（通过 allowed_tools 间接控制）；本期不做 netns/network sandbox。

**事件审计**：
每次 execute 的开始/结束 + 所有 tool_call 会落一条 `SkillExecutionEvent` 到 `persistence.rs` JSONL，跟 memory 走同一条事件总线。

### 4.6 `SkillAuthoringSession`（`crates/skills/src/authoring.rs`）

状态机，由 meta-tool `skill_creator` 驱动：

```rust
pub enum AuthoringState {
    Started { session_id: String },
    CapturedIntent { brief: String },
    Interviewed { partial: SkillDraftInProgress },
    Drafted { draft_dir: PathBuf, skill_md: String },
    Validated { draft_dir: PathBuf, manifest: SkillManifest },
    Tested { draft_dir: PathBuf, test_report: TestReport },
    Committed { record: SkillRecord },
    Cancelled,
}
```

状态转移动作：
- `start` → `Started`
- `capture_intent(brief)` → `CapturedIntent`
- `interview(answers)` → `Interviewed`（可多次迭代）
- `draft()` → `Drafted`（写 SKILL.md 到 `.drafts/`）
- `validate()` → `Validated`（自动在 draft 上触发）
- `test(inputs)` → `Tested`
- `commit()` → `Committed`（调 `SkillStore::commit`，归档旧版本）
- `cancel()` → `Cancelled`（清理 draft 目录）

### 4.7 `skill_creator` Meta-Tool（`crates/agent/src/tools/skill_creator.rs`）

在 ToolRegistry 注册以下 sub-tools（对 LLM 暴露）：

| Tool | 输入 | 输出 |
|---|---|---|
| `skill_creator.start` | `{ "brief": string }` | `{ "session_id": string, "next": "interview" }` |
| `skill_creator.interview` | `{ "session_id", "answers": {...} }` | `{ "next": "draft"\|"more_questions", "questions"?: [...] }` |
| `skill_creator.draft` | `{ "session_id", "overrides"?: {...} }` | `{ "skill_md": string, "manifest": SkillManifest }` |
| `skill_creator.test` | `{ "session_id", "inputs": {...} }` | `{ "stdout", "stderr", "tool_calls", "duration_ms", "exit_code" }` |
| `skill_creator.commit` | `{ "session_id" }` | `{ "record": SkillRecord }` |
| `skill_creator.cancel` | `{ "session_id" }` | `{ "ok": true }` |
| `skill_creator.list_templates` | `{}` | `[ { "name", "description", "entrypoint_kind" } ]` |
| `skill_creator.import` | `{ "path": string }` | `{ "record": SkillRecord }`（从 `.skill` zip 导入） |

LLM 使用模式：用户说"帮我建一个天气 skill" → LLM 调 `start` → `interview`（自动 / 询问用户 / 查 knowledge base）→ `draft` → 把 SKILL.md 展示给用户 → `test`（dry run）→ 用户确认后 `commit`。

### 4.8 Tool Bridge — Skill 声明自己的 tools

Skill 在 `allowed_tools` 里可以声明 `Owned` 类型的 tool。该 tool **只在当前 skill 执行范围内可见**，生命周期与 SkillExecutor 一致。

桥接协议：
- Skill entrypoint 在 stdout 上写 `{"type": "owned_tool_call", "name": "...", "arguments": ...}`（LLM 的回传由 Skill 自己在 entrypoint 内部分流处理——Owned tool 本质上是 skill 自己提供的 function）。
- 对 Agent Loop 而言：Skill 被调用时，Loop 把 `allowed_tools` 里的所有 tool（包括 Owned）一起加入**本次 turn 的 tool_schema**，允许 LLM 直接调用。
- 与 Registry tool 的区别：Owned tool 的 execution 由 SkillExecutor 路由回自身的 entrypoint 进程，而不是 `ToolRegistry`。

对 LLM 完全透明：两者都是标准 OpenAI tool schema。

### 4.9 Memory 访问

Skill 在 `allowed_tools` 里声明 `Memory { op: Read|Write|Both }` 即可。Executor 注入下列 tool 到 skill 的可用 tool 列表：

- `memory.read(query, kinds?, scopes?, limit?)` → 返回 `MemoryRecaller::recall(query).take(limit)`.
- `memory.write(content, kind, scope, tags?)` → 写入 `MemoryStore`，并记 `SkillExecutionEvent::MemoryWrite`。

**本期不做审批**：IM Gateway 场景下 Skill 的 memory 读写直接生效（用户决策）。

---

## 5. REST API 详细定义

路径前缀 `/agent/skills`，全部要求 admin 认证（沿用现有 token / session 机制）。

### 5.1 列表 / 详情

```
GET /agent/skills
  query: enabled=true|false, scope=global|user|project, slash_command=/weather
  200: { "skills": [SkillRecord] }

GET /agent/skills/:name
  200: { "record": SkillRecord, "skill_md": string, "manifest": SkillManifest,
         "effective_scope": "project", "shadow_scopes": ["global"] }
```

### 5.2 创建

```
POST /agent/skills
  Content-Type: multipart/form-data
  fields:
    manifest (application/json): SkillManifest
    skill_md (text/markdown): SKILL.md 正文
    assets/* (任意 file parts, 保留相对路径): 同步落到 skill 目录
  201: { "record": SkillRecord }
  422: { "errors": [ValidationError] }
  409: { "error": "slash_command_conflict" | "name_conflict_in_scope" }
```

### 5.3 更新

```
PATCH /agent/skills/:name
  body: { "enabled"?, "skill_md"?, "manifest_overrides"?, "assets_diff"? }
  200: { "record": SkillRecord }
  409: { "error": "checksum_mismatch", "expected_checksum": "..." }
```

更新语义：若 `skill_md` 或任何 asset 变动，Registry 视为新版本——旧版本目录移入 `.history/<name>-<ts>.tar.zst`，新版本落到正式目录，保持"同名单版本 active"。

### 5.4 删除

```
DELETE /agent/skills/:name
  query: scope=project (删除特定 scope; 不传则删除 effective_scope)
  204
```

删除后如果更高优先级 scope 还有同名，Registry 自动 fallback。

### 5.5 Test / Package / Import / Validate

```
POST /agent/skills/:name/test
  body: { "inputs": {...}, "timeout_ms"?: number }
  200: { "stdout", "stderr", "tool_calls", "duration_ms", "exit_code" }

POST /agent/skills/:name/package
  200: { "download_url": "/agent/skills/:name/download?token=...", "expires_at_unix": ... }

POST /agent/skills/import
  Content-Type: multipart/form-data (字段: archive=<.skill zip>)
  201: { "record": SkillRecord }
  422: { "errors": [...] }

POST /agent/skills/validate
  body: { "manifest": SkillManifest, "skill_md": string }
  200: { "ok": true, "warnings": [] } | 422: { "errors": [...] }
```

---

## 6. WebUI 设计

### 6.1 入口

`web/src/pages/Settings/tabs/AgentTab.tsx` 追加 `SkillsSection` 面板（位于 `MemoriesSection` 之后）。

### 6.2 `SkillsSection.tsx`

```
┌ Skills ─────────────────────────────────────────── [+ New Skill] ┐
│                                                                   │
│  [ All | Enabled | Project | User | Global ]  [🔍 search]        │
│                                                                   │
│  ┌─────────────────────────────────────────────────────────────┐ │
│  │ ☑ weather-lookup            v0.1.0   project    /weather    │ │
│  │   Fetch current weather for a city via wttr.in.             │ │
│  │                          [Test] [Edit] [Package] [Delete]   │ │
│  └─────────────────────────────────────────────────────────────┘ │
│  ┌─────────────────────────────────────────────────────────────┐ │
│  │ ☐ long-term-memory-export   v0.2.0   global     –           │ │
│  │   Export memory records to JSONL.                            │ │
│  └─────────────────────────────────────────────────────────────┘ │
└───────────────────────────────────────────────────────────────────┘
```

每行包含：启用开关、name、version、effective_scope 徽标、slash_command、一行 description，以及 Test/Edit/Package/Delete 操作。

### 6.3 `SkillCreatorWizard`

分 4 步：

1. **Intent & Metadata** — name, description, scope, version, slash_command, triggers。
2. **Entrypoint** — 选 Inline / Shell / Python / Node；Monaco 编辑器写脚本内容。
3. **Tools & Permissions** — 多选 Registry tools；声明 Owned tools（name + description + JSON Schema）；勾选 memory.read / memory.write；选 MCP tools。
4. **Test Run** — 填 inputs（按 inputs_schema 表单化），点 "Run Test"，展示 stdout/stderr/tool_calls 列表、duration、exit_code；通过后 "Save" 按钮触发 `POST /agent/skills`。

向导内部维护 `SkillDraftInProgress` 状态，最后一步 Save 失败时可 back 修复。

### 6.4 `SkillEditor`

复用 Wizard 的 4-tab 布局，但所有字段预填为当前 skill，顶部展示 checksum、created_by、updated_at。保存时走 `PATCH /agent/skills/:name`。

### 6.5 Import / Package

`SkillsSection` 顶部按钮组额外提供 `[⇧ Import .skill]` 上传 zip 走 `POST /agent/skills/import`；每行的 `Package` 按钮走 `POST /agent/skills/:name/package` 然后触发浏览器下载。

---

## 7. 安全与约束

1. **路径逃逸防护**：所有 manifest 里引用的路径必须 `canonicalize() starts_with(skill_dir)`。
2. **名字唯一性**：`(scope, name)` 复合主键；跨 scope 的同名不冲突但 WebUI 提示 shadow。
3. **Slash 冲突**：注册时冲突 → 拒绝；多 scope 冲突以 Project > User > Global 的 effective scope 为准。
4. **Checksum 校验**：Registry 热加载与 Test 前均校验；不一致 → 拒绝加载并在 admin API 上返回 `checksum_mismatch`。
5. **资源限制**：见 §4.5。
6. **审计**：所有 CRUD + execute 走 `persistence.rs` JSONL，字段包括 `actor`、`scope`、`name`、`version`、`checksum`、`result`。
7. **删除保留期**：`.history/` 保留 30 天后自动清理（由现有 persistence GC 承接）。
8. **Draft TTL**：`.drafts/` 中未提交目录 7 天后清理。
9. **Memory ACL**：Skill 对 memory 的写入强制携带 `source_skill = <name>@<version>` 元数据，便于追溯。

---

## 8. 与现有代码的最小入侵改动

| 文件 | 改动类型 | 说明 |
|---|---|---|
| `crates/skills/**` | **新增 crate** | 模块见 §2.1 |
| `Cargo.toml`（workspace）| 追加 | 加入 `crates/skills` |
| `crates/agent/Cargo.toml` | 追加 | 依赖 `skills`, 移除对 im_gateway::SkillsManager 的间接依赖 |
| `crates/agent/src/session.rs` | 替换 L491 硬编码 match | 改为 `SlashCommandRouter::dispatch` |
| `crates/agent/src/slash.rs` | **新增** | `SlashCommandRouter` 实现 |
| `crates/agent/src/prompt.rs` | 修改 "Available skills" 块 | 读 `SkillRegistry::enabled()` |
| `crates/agent/src/tools/mod.rs` | 追加模块 | 注册 `skill_creator` meta-tool |
| `crates/agent/src/tools/skill_creator.rs` | **新增** | 见 §4.7 |
| `crates/agent/src/memory_runtime.rs` | 暴露 memory.read/write | 供 Skill tool_bridge 调用 |
| `crates/bifrost-admin/Cargo.toml` | 依赖 `skills` | |
| `crates/bifrost-admin/src/handlers/mod.rs` | 挂 `agent_skills` 模块 | |
| `crates/bifrost-admin/src/handlers/agent_skills.rs` | **新增** | 见 §5 |
| `crates/bifrost-admin/src/handlers/im_gateway.rs` | 重构 `SkillsManager` | 改为 `Arc<SkillRegistry>` 的薄封装；`GET /agent/skills` 语义保持 |
| `crates/bifrost-admin/src/router.rs` | 追加路由 | `/agent/skills/*` |
| `web/src/pages/Settings/tabs/AgentTab.tsx` | 追加 `<SkillsSection />` | |
| `web/src/pages/Settings/tabs/agent/SkillsSection.tsx` | **新增** | |
| `web/src/pages/Settings/tabs/agent/SkillCreatorWizard.tsx` | **新增** | |
| `web/src/pages/Settings/tabs/agent/SkillEditor.tsx` | **新增** | |
| `web/src/pages/Settings/tabs/agent/types.ts` | 追加 SkillRecord/Manifest 类型 | |
| `web/src/api/agent-skills.ts` | **新增** | 对应 §5 REST |
| `e2e-tests/tests/test_skill_creator_flow.sh` | **新增** | 跑通 create→test→invoke→delete |
| `crates/bifrost-e2e/src/tests/skill_creator.rs` | **新增** | runner 侧 |
| `human_tests/skill-creator.md` | **新增** | 手测脚本 |
| `human_tests/readme.md` | 更新索引 | 74+N/1455+M |

---

## 9. 测试策略

### 9.1 单元测试（`cargo test -p skills`）

- `model`: serde 序列化/反序列化 round-trip
- `validator`: 名字/版本/slash/allowed_tools/entrypoint 每条规则的正/反例
- `store`: 原子 commit、归档、checksum 计算稳定性、draft TTL
- `registry`: scope 覆盖、slash 冲突检测、reload_one、FS watcher debounce
- `executor`: stdin/stdout 协议、超时、mem limit、tool 桥接、owned tool 路由
- `packager`: zip 打包/解包、checksum 一致
- `authoring`: 状态机转移

### 9.2 集成测试（`cargo test -p bifrost-agent`）

- `SlashCommandRouter` 内置命令等价（`/clear /compact /status /resume /remember ...`）
- skill 声明 `/weather` 后 router 正确分发
- skill 读取 memory / 写入 memory 的端到端

### 9.3 Admin 测试（`cargo test -p bifrost-admin agent_skills`）

- CRUD happy path
- 冲突路径（checksum mismatch、slash 冲突、name 冲突）
- import/export 对称

### 9.4 E2E (`e2e-tests/tests/test_skill_creator_flow.sh`)

1. 启 bifrost + mock LLM
2. 通过 chat 触发 `skill_creator.start → interview → draft → test → commit`
3. 再次 chat，通过 `/weather <city>` 触发 skill，验证输出
4. DELETE skill，再次触发 `/weather` 返回 unknown command
5. Import 预打包的 `.skill` zip，Re-invoke

### 9.5 WebUI 冒烟

在 `SkillsSection` 完成 New Skill 向导 → Test → Save → 列表中可见 → Edit → Delete 全流程。

---

## 10. 回滚与兼容

- 新 `crates/skills` 引入失败：保留现有 `SkillsManager` 只读路径；通过 feature flag `skills-crud` 控制。
- `SlashCommandRouter` 重构与硬编码 match **语义等价**；失败时可切换 feature flag `slash-router` 回退。
- 现有 `GET /agent/skills` 响应字段**只增不减**（新增 `effective_scope`, `shadow_scopes`, `checksum` 等），保证前端旧版本不崩。
- 已存在的 `~/.bifrost/skills/<name>/` 目录若缺 `manifest.json`，首次加载时由 validator 根据 `SKILL.md` 自动生成并持久化。

---

## 11. 开放问题（已决策）

| 问题 | 决策 |
|---|---|
| SKILL.md 与 manifest.json 的 source of truth | SKILL.md 为人写 source of truth；manifest.json 由 validator 生成带 checksum |
| Skill 能否声明自己的 tools | **允许**，见 §4.8 Owned tool |
| Skill 能否访问 memory | **允许**，通过 `allowed_tools: Memory { op }` 白名单 |
| 多版本共存 | **不支持**，同名单版本 active，旧版本归档到 `.history/` |
| IM Gateway 场景审批 | **不做**，Skill 执行直接生效 |
| Meta-tool 命名 | `skill_creator`（带 `.start .interview .draft .test .commit .cancel .list_templates .import`） |
| 分期实现 | **不分期**，一次到位（数据模型 → crate → admin → agent → webui → e2e 同一个 PR） |

---

## 12. 实施 Checklist

- [ ] 新建 `crates/skills` crate，模型/store/validator/registry/executor/packager/authoring/tool_bridge 就绪
- [ ] `crates/agent` 接入：SlashCommandRouter、prompt 重写、meta-tool、memory 桥接
- [ ] `crates/bifrost-admin` 接入：`handlers/agent_skills.rs`、router、替换 `SkillsManager`
- [ ] WebUI：`SkillsSection` + `SkillCreatorWizard` + `SkillEditor` + API client
- [ ] 单元 + 集成 + admin + e2e 测试全绿
- [ ] `human_tests/skill-creator.md` 写全手测步骤，索引更新
- [ ] `design/skill-creator.md`（本文档）入库
- [ ] `cargo fmt --all -- --check` / `cargo clippy --workspace --all-targets --all-features -- -D warnings` / `cargo check --workspace` / `cargo test --workspace` / `cd web && pnpm run lint && pnpm run build` 全绿
- [ ] commit 拆分：`docs(design)` → `feat(skills)` crate → `feat(agent)` router+meta-tool → `feat(admin)` CRUD → `feat(webui)` → `test(e2e)` → `docs(human-tests)`
- [ ] 推送到 `origin/feat/agent`，CI 绿

---

*end of design document*
