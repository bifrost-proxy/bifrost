# Skill Creator — Bifrost Agent Loop 技能创建子系统

> 实现状态：已发布 (implemented, refreshed against code as of 2026-07-03)。
> 核心 crate `crates/skills` (`authoring/executor/model/packager/registry/store/tool_bridge/validator`)、
> `crates/agent/src/skill_authoring.rs`、`crates/agent/src/slash.rs`、`crates/bifrost-admin/src/handlers/agent_skills.rs`
> 均已落地。前端 SkillsSection 与向导已上线。E2E 集成入口位于 `crates/bifrost-e2e/src/tests/skill_creator.rs`。

## 背景

Bifrost Agent Loop 早期对 skill 的支持仅限于**只读发现**：

- `SkillsManager`（`crates/agent/src/skills/mod.rs`）在进程启动时扫描 `~/.bifrost/skills/` 与工作区目录，
  只读加载 `SKILL.md` frontmatter。
- `GET /agent/skills` 只返回清单。
- 斜杠命令（`/clear /reset /undo /compact /status /resume /remember /memories /forget`）在 `session.rs` 里
  硬编码 `match`，skill 无法声明自己的 `/xxx`。
- Skill 如何执行、如何声明工具、如何访问长期记忆，没有任何原语；WebUI 上也没有 skill 管理页。

为让 Skill 成为 Agent Loop 的一等公民，我们把 skill 独立成 `crates/skills` crate，覆盖数据模型、存储、
校验、执行、注册表、打包 6 个子模块，并在 Agent 与 Admin 侧接入。用户可以直接在会话内或 WebUI
完成 skill 的 **create / edit / test / register / invoke / delete**，无需离开浏览器或手工摆放 SKILL.md。

## 用户目标验证清单

### 必须实现

- 用户/Agent 在会话内即可完成 skill 的创建、编辑、测试、注册、调用、删除全流程。
- Skill 是一等公民：独立 crate、数据模型、存储层、注册表、校验器、执行器、打包器、REST CRUD、
  WebUI 管理页。
- Skill 可以声明自己的 tools（`allowed_tools` 白名单）。
- Skill 可以通过 `memory.read` / `memory.write` 访问长期记忆。
- Skill 可以声明自己的斜杠命令（`slash_command`），被 `SlashCommandRouter` 动态注册。
- 同名同 scope 只保留一个 active 版本，老版本进 `.history/` 归档。
- IM Gateway 场景下 skill 执行不需要审批（本期决定）。
- Skill 支持 `.skill` zip 打包 / 解包，含 checksum 与 manifest 校验。

### 必须不破坏

- 原 SkillsManager 只读扫描的 skill 目录布局仍能被识别（迁移到 SkillStore 后布局兼容）。
- 内置斜杠命令 `/clear /reset /undo /compact /status /resume /remember /memories /forget` 优先级
  高于 skill 定义的同名命令；skill 声明冲突名时创建应失败并返回 409。
- Agent Loop 的 turn 循环行为、tools 调用协议保持稳定；skill 只是新增一路 tool 提供方。
- 未安装任何 skill 时 Agent 行为与新 crate 引入前一致。

### 必须真实验证

- `crates/skills` 各模块单测覆盖 CRUD、归档、slash 冲突、manifest 校验、执行超时、tool_bridge。
- Admin handler 单测覆盖 REST CRUD、多种错误 → HTTP status 映射、multipart 导入。
- E2E 集成 `crates/bifrost-e2e/src/tests/skill_creator.rs` 覆盖端到端创建 → 调用 → 归档路径。
- 前端交互测试覆盖 SkillsSection 列表、SkillCreatorWizard、SkillEditor 三个入口。

## 产品语义

### Skill 生命周期

```
[Draft] --commit--> [Active vN] --update--> [Active vN+1]
                        │                        │
                        │                        └── vN → .history/vN/
                        └── delete --> .history/vN/
```

- 同名同 scope 只保留一个 `Active`；`update` 会先把当前 active 归档到 `.history/vN/`，再写入新版本。
- 删除是软删除：把 active 归档到 `.history/vN/`，Registry 立即摘除；不会立刻回收磁盘。
- `enable` / `disable` 只切换 `enabled` 位，不动版本；被 disable 的 skill 从 prompt 与 slash router 摘除。

### Scope 三级

- `Global` → `~/.bifrost/skills/`
- `User` → `~/.bifrost/skills/users/<user>/`
- `Project` → `<workspace>/.bifrost/skills/`

Registry 加载顺序：Global → User → Project；同名同 scope 冲突走 409；跨 scope 同名不冲突，
但 prompt 展示会带 scope 前缀，slash 命令按 Project > User > Global 覆盖。

### Skill 声明自己的 tools

`SkillManifest::allowed_tools` 是白名单，支持两类绑定：

- 引用 Bifrost `ToolRegistry` 已注册的内置 tool（如 `memory.read`、`memory.write`、`http.fetch`）。
- 声明 skill 自己的 tool（本期支持子进程 + JSON stdio 协议）。

`SkillToolBridge` 负责把 skill 声明的 tools 转成 Agent Loop 可调用的 tool 描述；同时把
`memory.read` / `memory.write` 桥接到 `crates/agent/src/memory_runtime.rs`（progressive disclosure：
只暴露 `read/write` 两个原语，不暴露内部存储路径）。

### 斜杠命令动态注册

`crates/agent/src/slash.rs` 引入 `SlashCommandRouter`：

- 内置命令走 `register_builtin(name, handler)`；
- Skill 声明的 `slash_command` 由 `SkillRegistry` 在 load / enable / update 时调用 `register_skill(name, skill_id)`；
- 名字冲突返回 409（skill 侧创建 / 更新失败），不会覆盖内置。

## 技术细节

### crates/skills 分层

| 文件 | 行数 | 职责 |
| --- | --- | --- |
| `model.rs` (213) | | `SkillManifest / SkillScope / Entrypoint / TriggerRule / ToolBinding / SkillAuthor` |
| `store.rs` (856) | | 磁盘读写、atomic commit、checksum、`.history/vN/` 归档、`SkillDraft` |
| `registry.rs` (610) | | 内存索引、热更新、`default_roots()`、`system_skills_cache_dir()`、slash 注册 |
| `validator.rs` (729) | | YAML/JSON Schema、`allowed_tools` 校验、slash 命名规范、triggers 冲突 |
| `executor.rs` (481) | | 子进程执行、超时、内存限制、`ExecutionEvent` / `SkillInvocation` / `SkillTestReport` |
| `packager.rs` (229) | | `.skill` zip 打包/解包、checksum 校验 |
| `authoring.rs` (395) | | `SkillAuthoringSession` 状态机、`AuthoringState` |
| `tool_bridge.rs` (370) | | `SkillToolBridge`、`MemoryToolRequest/Response` |
| `lib.rs` (26) | | 模块与 re-exports |

### SkillManifest（简化）

```rust
pub struct SkillManifest {
    pub name: String,          // ^[a-z][a-z0-9-]{0,63}$
    pub version: String,       // semver
    pub description: String,   // <= 1024
    pub scope: SkillScope,
    pub entrypoint: Entrypoint,
    pub allowed_tools: Vec<ToolBinding>,
    pub slash_command: Option<String>,
    pub triggers: Vec<TriggerRule>,
    pub inputs_schema: Option<serde_json::Value>,
    pub outputs_schema: Option<serde_json::Value>,
    pub metadata: BTreeMap<String, String>,
    pub created_by: SkillAuthor,
    pub created_at_unix: i64,
    pub updated_at_unix: i64,
    pub checksum: String,      // 目录 sha256（不含 manifest.json 自身）
    pub schema_version: u32,   // 当前 1
}

pub enum Entrypoint {
    Inline { instructions_md: String },
    Shell { script: PathBuf, shell: ShellKind },
    Python { script: PathBuf, python: Option<String> },
    Node { script: PathBuf, node: Option<String> },
}
```

### Executor 与安全

- `SkillExecutor` 使用子进程 + JSON stdio；每次调用创建新的进程，不复用。
- 通过 `env_clear` + 白名单 env（`PATH`、`HOME`、`BIFROST_*`）避免宿主 env 泄漏。
- 超时可配置，默认 60s；超时后 SIGTERM → 3s → SIGKILL。
- 内存限制通过 `prlimit` (Linux) / `setrlimit` (macOS) 施加；Windows 上仅记录未强制。
- `FileAccessPolicy` 依赖 `crates/bifrost-core` 的 sandbox policy；本期不做 OS 级 sandbox。

### 归档结构

```
~/.bifrost/skills/<name>/
  ├─ SKILL.md
  ├─ manifest.json
  ├─ entrypoint/…
  └─ .history/
      ├─ v0.1.0/
      │   ├─ SKILL.md
      │   └─ manifest.json
      └─ v0.2.0/
```

`.history/` 只做审计与回滚基础，本期不暴露前端 UI。

### Codex 任务巡检 Skill

内置一个仓库级只读 skill：`codex-task-inspector`。用户在会话中提到“检查 Codex 任务 / 查异步任务进展 /
汇总 .codex-tasks”时自动触发，固化 `.codex-tasks/` 排查顺序：

1. 先读 `.codex-tasks/*.pid` + `ps` 判断本地任务是否仍在运行。
2. 再读 `.codex-tasks/*-last.md`（必要时补 `*.jsonl` / `*.meta` / `*-report-*.md`）提取结论。
3. 最后读 CI 轮询日志（`*.log`，尤其 `skill-creator-ci-poll.log`）尾部，输出 still running / completed /
   failure 三态。
4. 输出必须分四段：本地 Codex 进程 / 任务产物摘要 / CI 状态 / 建议下一步。
5. 不能把 `.codex-tasks` 文件存在视为任务仍在运行；必须以 PID + `ps` 结果为准。
6. 建议动作只限“继续查失败原因”或“整理状态表”，不假设要改代码或重跑任务。

对应真实场景在 `human_tests/codex-task-inspector.md`（另有独立设计
`design/codex-task-inspector.md`）。

## CLI + Web + Admin API

### Admin API（`crates/bifrost-admin/src/handlers/agent_skills.rs`）

| Method | Path | 处理函数 (line) | 说明 |
| --- | --- | --- | --- |
| GET | `/agent/skills` | `list_skills` (56) | 列表；含 name/version/scope/enabled/slash |
| GET | `/agent/skills/:name` | `get_skill` (65) | 详情，含 SKILL.md 原文与 manifest |
| POST | `/agent/skills` | `create_skill` (98) | 创建 draft → commit |
| PATCH | `/agent/skills/:name` | `patch_skill` (114) | 更新元信息 / enable / disable |
| DELETE | `/agent/skills/:name` | `delete_skill` (146) | 归档到 `.history/vN/`，从 Registry 摘除 |
| POST | `/agent/skills/:name/test` | `test_skill` (169) | 干跑（dry-run），返回 `SkillTestReport` |
| POST | `/agent/skills/:name/package` | `package_skill` (196) | 导出 `.skill` zip |
| POST | `/agent/skills/import` | `import_skill` (214) | 导入 `.skill` zip（multipart） |
| POST | `/agent/skills/validate` | `validate_skill` (249) | 仅校验，不落盘 |

错误映射见 `AgentSkillError::from_store_error` (line 322)：

- `AlreadyExists` → 409
- `NotFound` → 404
- `ValidationFailed` → 422
- `SlashConflict` → 409
- `PermissionDenied` → 403
- 其它 → 500

### Agent 会话内 Meta-Tool

`crates/agent/src/skill_authoring.rs` (429 行) 暴露 `SkillAuthoringHub`：

- `skill_authoring.start(name, scope)` → 打开 draft session
- `skill_authoring.interview(question, answer)` → 状态机推进
- `skill_authoring.draft(fields)` → 更新 manifest / entrypoint
- `skill_authoring.test(inputs)` → 调 `SkillExecutor` dry-run
- `skill_authoring.commit()` → 走 SkillStore commit
- `skill_authoring.list()` / `.get(name)` / `.delete(name)` / `.enable(name)` / `.disable(name)`

会话内 meta-tool 不需要单独 API；直接走 Agent Loop tool 调用。

### CLI

Skill 管理主入口在 WebUI / 会话内。CLI 保留最小面：

- `bifrost skill list` — 复用 `GET /agent/skills`。
- `bifrost skill install --file <pkg.skill>` — 复用 `POST /agent/skills/import`。
- `bifrost skill enable <name>` / `disable <name>` / `delete <name>`。
- `bifrost skill validate <dir>` — 只跑 `SkillValidator`，不落盘。

### WebUI

`Settings → Agent → Skills`：

- `SkillsSection`：列表 + 启用开关 + 快捷进入编辑器；空态引导创建。
- `SkillCreatorWizard`：向导 = 元信息 → entrypoint → tools 白名单 → 触发规则 → 测试用例 →
  commit；每一步走 `POST /agent/skills/validate` 做 in-place 校验。
- `SkillEditor`：Monaco 编辑 SKILL.md + manifest；保存前跑 validate；保存走 PATCH。

## Sync 边界

- Skill 内容 (`SKILL.md` / `manifest.json` / 执行脚本) **不进入 Bifrost Sync**。
  Sync 只处理 rules / groups / values；skill 属于本地资产。
- Skill 分发通过 `.skill` zip + `POST /agent/skills/import`。远程 Skill Market 是后续 crate
  (`bifrost-skill-market`)，不在本设计范围。
- `.history/` 归档只在本地，不上传远端。
- `codex-task-inspector` 是内置 skill，随二进制分发，不通过 Sync 推送。

## 实现切分

### Phase 1：crate 骨架

- 新建 `crates/skills`。
- 完成 `model.rs / store.rs / validator.rs / packager.rs`。
- 单元测试：manifest roundtrip、`.history/` 归档、checksum、YAML 校验。

### Phase 2：Registry + Executor + Tool Bridge

- 完成 `registry.rs / executor.rs / tool_bridge.rs`。
- `SlashCommandRouter` 落地，取代 `session.rs` 硬编码 match。
- `memory.read` / `memory.write` 桥接。
- 单元测试：slash 冲突、超时、tool_bridge memory 权限。

### Phase 3：Admin API + WebUI 列表

- 完成 `handlers/agent_skills.rs` 9 个 endpoint。
- `SkillsSection` 列表 + 启用开关 + 空态。
- `AgentSkillError` HTTP 映射；handler 单测覆盖 CRUD + slash 冲突 + multipart。

### Phase 4：Authoring + Wizard + E2E

- 完成 `authoring.rs`（`SkillAuthoringSession` 状态机）。
- `SkillAuthoringHub` 暴露 meta-tool。
- `SkillCreatorWizard` / `SkillEditor` 前端。
- E2E `crates/bifrost-e2e/src/tests/skill_creator.rs` 覆盖端到端 create → invoke → archive。

## 测试方案

### 单元测试

- `crates/skills/src/store.rs` (15+ tests, line 545 起)
  - `create_and_load_active_skill`
  - `commit_archives_previous_active_to_history`
  - `delete_moves_active_to_history`
  - `enable_disable_toggles_flag_without_version_bump`
  - `atomic_commit_rolls_back_on_partial_write`
- `crates/skills/src/validator.rs` (15+ tests, line 405 起)
  - `manifest_missing_name_returns_validation_error`
  - `slash_command_name_conflict_with_builtin_rejected`
  - `allowed_tools_reference_unknown_tool_rejected`
  - `triggers_must_be_unique_within_manifest`
- `crates/skills/src/executor.rs`
  - `execute_python_entrypoint_reports_stdout_and_exit`
  - `execute_timeout_sends_sigterm_then_sigkill`
  - `execute_denies_env_leak`
- `crates/skills/src/tool_bridge.rs`
  - `memory_tool_bridge_allows_read_when_whitelisted`
  - `memory_tool_bridge_rejects_write_when_missing_scope`
- `crates/skills/src/packager.rs`
  - `package_roundtrip_preserves_checksum`
  - `unpackage_detects_corrupt_zip`
- `crates/bifrost-admin/src/handlers/agent_skills.rs` (tests, line 478 起)
  - `agent_skills_crud_store_happy_path` (line 510)
  - `agent_skills_detects_slash_conflict` (line 542)
  - `multipart_import_extracts_package_field_bytes` (line 574)
  - `agent_skill_error_maps_conflict_to_409` (line 585)
- `crates/agent/src/slash.rs`
  - `router_prefers_builtin_over_skill_slash`
  - `router_removes_slash_on_skill_disable`

### E2E 测试

- `crates/bifrost-e2e/src/tests/skill_creator.rs`
  - `skill_creator_end_to_end_create_invoke_archive` — 通过 REST 创建 draft → commit →
    从 Agent 会话调用 `/skill-name` → 更新 → 断言旧版本落在 `.history/v0.1.0/`。
  - `skill_creator_import_and_delete_roundtrip` — 导入 `.skill` zip → list 可见 → delete →
    archive 存在。
- `crates/bifrost-e2e/src/tests/skill_loading.rs` — 兼容旧 SkillsManager 布局。

### 真实场景测试

- `human_tests/skill-loading-e2e.md` — 已存在，覆盖启动加载。
- `human_tests/codex-task-inspector.md` — 覆盖 Codex 巡检 skill 的 pid + CI 双通道排查。
- 建议新增 `human_tests/skill-creator-wizard.md`：真实浏览器走向导 → commit → 在会话内触发 slash。

## Review / Fix / Test 闭环

- 第 1 轮：跑 `crates/skills` 全部单测 + `handlers/agent_skills.rs` 单测；确认 slash 冲突 / 归档 /
  checksum 三条链路。
- 第 2 轮：跑 E2E `skill_creator` / `skill_loading`；前端 Playwright 覆盖向导与列表。
- 第 3 轮：真实场景走 `human_tests/skill-creator-wizard.md`；抽查 3 个真实 skill 的 packaging /
  import roundtrip。
- 校验命令：
  - `cargo test -p bifrost-skills`
  - `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-admin agent_skills -- --nocapture`
  - `cargo test -p bifrost-e2e --test skill_creator`
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets --all-features -- -D warnings`

## 风险与决策

- **风险：子进程执行的安全边界**。缓解：env 白名单 + timeout + memory limit + `FileAccessPolicy`；
  文档明确本期不做 OS sandbox。
- **风险：slash 命令冲突**。缓解：`SlashCommandRouter` 拒绝重名，内置命令永远优先；创建/更新
  返回 409 由前端明确提示。
- **风险：manifest 变更破坏已加载 skill**。缓解：`schema_version` 字段 + `SkillValidator`
  做 migration 阻断；同名同 scope 归档到 `.history/`，可回滚。
- **风险：`.skill` zip 恶意内容**。缓解：`SkillPackager` 严格校验 zip 目录结构、拒绝绝对路径与
  `..`、限制 entry 大小；导入前跑 `SkillValidator`。
- **决策：IM Gateway 场景免审批**。理由：IM Gateway 已在网关侧做粗粒度访问控制，二次审批
  影响自动化；本期落到设计里作为显式豁免，日志侧仍留痕。
- **决策：不做多版本并行 active**。理由：Registry 简化 + prompt 稳定 + slash 唯一；有需要用 scope
  差异化解决。
- **决策：Sync 不承接 skill 分发**。理由：skill 是可执行资产，风险面与规则不同，走独立 market
  crate 更合适。
- **决策：`.history/` 只归档不清理**。理由：审计与回滚基础；未来加 GC 策略再单独设计。

## 文档更新要求

- 更新 `docs/agent-skill.md` / `docs-en/agent-skill.md`：加入 CRUD API、向导流程、`.skill` zip 打包
  规范、`allowed_tools` 说明。
- `site/src/content/docs/reference/agent-skill.md`（含 en 版本）保持与 docs 同步。
- `human_tests/readme.md` 新增 `skill-creator-wizard.md` 索引。
- 若引入 remote skill market，需要另起 `design/bifrost-skill-market.md`。
