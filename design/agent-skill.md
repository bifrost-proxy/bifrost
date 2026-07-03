# bifrost install-skill 与 Skills Runtime 完整方案

## 背景

Bifrost 的 skill 体系覆盖两个交叉面：

1. **对外分发**：`bifrost-cli` 的 `install-skill` 子命令负责把 `SKILL.md` 与 `skill_remote.md` 安装到各 AI 编程工具（Claude Code、Codex、Trae、Cursor、GitHub Copilot、通用 `.agents/skills`）的全局或项目级路径，让不同 agent 都能通过其标准 skill 发现机制加载 Bifrost 技能。目标是覆盖式安装、脱机可用（内嵌兜底）、支持自定义目录与项目级安装、脚本友好。
2. **对内 runtime**：`crates/skills` crate 承载 skill executor、registry watcher、packager、authoring 状态机；`crates/agent` 里 `SkillsManager::build_skills_instructions()` 把 enabled skill 的名称/描述/路径注入 system prompt（不 eager 注入 body），`AgentSession::new_with_work_dir()` 在真实 session 中装配 `SkillRegistry`。Admin API `POST /agent/skills/import` 接收原始包 bytes，前端 Skill Creator Wizard/Editor 共享 form schema，IM CLI 的 secret 解析走强类型错误。

feat/agent review fixpass 已经把多个 review 结论沉到代码：executor env 白名单、watcher 精准 reload、checksum 缺失处理、packager scope 保留、authoring 状态机、runtime 装配 SkillRegistry、prompt digest 有界、长期记忆 append 加 advisory lock、admin import 拒收 client PathBuf、CLI secret 强类型、Web 表单 fence 转义、IM Gateway loading/error 清理、E2E env 隔离与 storage size guard。本设计把上述方案统一固化，并按标准模板给出验证与切分。

## 用户目标验证清单

### 必须实现

- **install-skill CLI**：
  - `bifrost install-skill [OPTIONS]` 覆盖式安装最新 `SKILL.md` + `skill_remote.md` 到各工具目录，遵循 Standard Agent Skills Format。
  - 支持 `--tool/-t`（`claude-code`/`codex`/`trae`/`cursor`/`github-copilot`/`universal`/`all`，默认 `all`）。
  - 支持 `--dir/-d` 自定义目录（与 `--cwd` 互斥）；支持 `--cwd` 项目级安装。
  - 支持 `-y`/`--yes` 跳过确认。
  - 网络失败或 >45s 超时回退到 `include_str!` 嵌入副本；`BIFROST_INSTALL_SKILL_SOURCE=embedded` 强制走嵌入。
  - Trae 全局同时安装到 `.trae` 与 `.trae-cn`；Codex 同时安装到 `.codex/skills` 与 `.agents/skills`。
  - GitHub Copilot 支持专用目录与项目级 `.github/skills`。
  - `all` 覆盖专用 agent + 通用 `.agents/skills`。
- **Skills runtime**：
  - Executor `env_clear()` 后保留白名单：`PATH`、`HOME`、`USER`、`LOGNAME`、`LANG`、`LC_ALL`、`LC_CTYPE`、`TMPDIR`、`TEMP`、`TMP`、`TERM`、`SHELL`、`SSL_CERT_FILE`、`SSL_CERT_DIR`、`CARGO_HOME`、`RUSTUP_HOME`，再叠加 `SkillManifest.env`。
  - `SkillRegistry::reload_one(slug)` 仅重建对应 slug；目录不存在则删除索引；其他 skill 不受影响；watcher 从事件路径反推 root 下第一级 slug 触发差异重载。
  - `verify_checksum()` 对缺失 `manifest.json` 返回 `false` 并 warning。
  - `SkillPackager::import()` 保留包内合法 `manifest.scope`；缺失/非法时用调用方默认 scope。
  - `SkillAuthoringSession::test()` 在非 `Drafted`/`Validated`/`Tested` 状态返回 `AuthoringError::InvalidState`。
  - `AgentSession::new_with_work_dir()` 装配 `SkillRegistry`（`with_skills(Arc<SkillRegistry>)` 链式 API 保持兼容），slash router 与 prompt digest 复用同一 registry。
  - `SkillsManager::build_skills_instructions()` 输出 `## Available Skills` digest：每个 enabled skill 一行 `- <name>: <description_one_line>`，总长度 ≤ 4KB，稳定按名称排序。
  - 渐进式披露：base prompt 只含 name/description/`SKILL.md` 路径，不 eager 注入 body；模型按需读取。
  - 长期记忆 append 使用 `fs2::FileExt::lock_exclusive()`，多 session 并发写不交错。
- **Admin API import**：
  - `POST /agent/skills/import` 只接受 `application/octet-stream` 原始 bytes 或 `multipart/form-data` 的 `package` 字段，禁收 client `PathBuf`。
  - bytes 落到 `<agent_data_dir>/skills/.import-tmp/` 后再交 `SkillPackager::import()`。
  - scope 从 `x-bifrost-skill-scope` 请求头读取，默认 `Repo`。
  - `AgentSkillError` 分层：参数错误 400、冲突 409、语义校验失败 422、未知 I/O 500。
- **IM CLI secret**：
  - `resolve_secret` 返回 `Result<String, ResolveSecretError>`：`Missing` env / `Io` file 读取失败分别报错，不再 warning 后写空 secret。
- **Web 表单与 IM Gateway**：
  - Skill Creator Wizard 与 SkillEditor 复用 `utils/skillFormSchema.ts` 与 Manifest/Script Editor/Test Panel 组件。
  - `buildSkillMd()` 不再把 shell/python/node script body 塞 SKILL.md 正文；inline 用 fenced block，遇三反引号或独立 `---` 行时切换/转义 fence，保 frontmatter 边界。
  - IM Gateway Tab `fetchData` 失败通过 `message.error()` 暴露，`finally` 清理 loading；切 tab 先 reset loading 再加载新 tab。
- **E2E env 与 storage size guard**：
  - `im_gateway_agent` E2E 中涉及 `BIFROST_DATA_DIR` 的用例使用当前布局并通过 `temp-env` 作用域隔离；guard 放 `spawn_blocking` + 单线程 tokio runtime，避免 non-Send guard 跨 await。
  - `bifrost-core/src/limits.rs::MAX_RULE_FILE_BYTES = 256 * 1024 * 1024`、`ensure_file_size_within_limit(path, limit)` 统一 metadata 检查；`bifrost-storage/src/rules.rs` load/load_summary 复用。

### 必须不破坏

- 已有 skill 目录、SKILL.md 内容、slash 命令语义不变。
- Agent runtime 装配 SkillRegistry 不改变 slash router 单测已有接口。
- `install-skill` 命令行参数向后兼容旧调用；未指定 `--tool` 仍然 `all`。
- `bifrost upgrade` 自动 `install-skill --tool all -y` 的 post-install 步骤保持一致。
- 长期记忆 lock 只在 append 路径加，不影响读或列出。
- Admin API 老的 raw PathBuf 上传路径直接下线，前端已切到 multipart，不影响用户已完成的 skill 包。

### 必须真实验证

- CLI E2E：`--dir`、`--cwd`、`--tool` 各值、覆盖安装、frontmatter 校验、未知工具、`--dir` vs. `--cwd` 互斥。
- Skills runtime 单测：executor env、watcher reload、checksum 缺失、packager scope、authoring 状态机、session skills 集成、prompt digest 有界、长期记忆并发 append。
- Admin API 单测：multipart import、`AgentSkillError` 映射、rejects raw PathBuf。
- IM CLI 单测：`resolve_secret` 两条错误路径。
- Web 单测：`SkillCreatorWizard.test.ts` fence 转义两回归；构建：`pnpm --dir web build`。
- E2E env：`cargo test -p bifrost-e2e --no-run`、`cargo check -p bifrost-storage --quiet`。
- human_tests：`skill-creator.md`（新增 TC-SC-07..14）、`agent-runtime-review-fixes.md`（TC-ARF-01..03）、`agent-skills-admin-cli.md`（TC-ASAC-01..03）、`storage-e2e-safety.md`（TC-SES-01..03）、`cli-import-export.md` 覆盖 install-skill 各 tool。

## 产品语义

### install-skill

- 每次安装都是**覆盖式**：确保用户随命令拿到 main 分支最新技能内容。
- 全局路径优先；`--cwd` 用于给项目仓库注入本地 skill。
- `.agents/skills` 是通用兜底，兼容任何遵循 Standard Agent Skills Format 的 runtime。
- 网络不可用时不应阻塞用户，回退嵌入副本；`upgrade` 后台安装失败也只 warning，不阻止升级完成。

### Skills runtime

- **渐进式披露**：SKILL.md body 大时不进 base prompt；只有需要真正调用 skill 时才按路径读入。
- **SkillRegistry watcher** 是差异 reload：新增/修改/删除单个 slug 只重建该 slug，其他 skill 索引与句柄不动。
- **长期记忆并发写**：`MEMORY.md` / `raw_memories.md` 是行级 append 语义，必须 advisory lock。
- **Admin import** 只在服务端信任服务端路径：客户端 payload 是原始 bytes，服务端才决定落到 `.import-tmp/`。

## 技术细节

### 1. install-skill 下载源与嵌入副本

`crates/bifrost-cli/src/commands/install_skill.rs`：

```rust
const SKILL_RAW_URL: &str = "https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/SKILL.md";
const REMOTE_SKILL_RAW_URL: &str =
    "https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/skill_remote.md";
const EMBEDDED_SKILL_MD: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../SKILL.md"));
const EMBEDDED_REMOTE_SKILL_MD: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../skill_remote.md"));

const SKILL_SOURCES: &[SkillSource] = &[
    SkillSource::Bifrost { url: SKILL_RAW_URL, embedded: EMBEDDED_SKILL_MD, sub_dir: "bifrost" },
    SkillSource::BifrostRemote { url: REMOTE_SKILL_RAW_URL, embedded: EMBEDDED_REMOTE_SKILL_MD, sub_dir: "bifrost-remote" },
];
```

- `download_skill_source(source)`：`ureq` GET，45s 超时；失败或 non-2xx 回退 `embedded`。
- `download_skill_bundle()` 顺序下载两份 skill；`BIFROST_INSTALL_SKILL_SOURCE=embedded` 强制嵌入。
- frontmatter warning：下载内容缺 `---` 时 stderr warning（不 fail），提示各 agent 需要 name/description frontmatter。

### 2. 工具映射与目录

统一 `skills/bifrost/SKILL.md` + `skills/bifrost-remote/SKILL.md` 双子目录：

| 工具 | 标识 | 全局路径 | `--cwd` 项目路径 |
| --- | --- | --- | --- |
| Claude Code | `claude-code`, `claude` | `~/.claude/skills/bifrost/SKILL.md` | `./.claude/skills/bifrost/SKILL.md` |
| Codex / 通用 | `codex`, `openai-codex`, `universal` | `~/.codex/skills/bifrost/SKILL.md` + `~/.agents/skills/bifrost/SKILL.md` | `./.codex/skills/bifrost/SKILL.md` + `./.agents/skills/bifrost/SKILL.md` |
| Trae | `trae` | `~/.trae/skills/bifrost/SKILL.md` + `~/.trae-cn/skills/bifrost/SKILL.md` | `./.trae/skills/bifrost/SKILL.md` |
| Cursor | `cursor` | `~/.cursor/skills/bifrost/SKILL.md` | `./.cursor/skills/bifrost/SKILL.md` |
| GitHub Copilot | `github-copilot`, `copilot` | `~/.copilot/skills/bifrost/SKILL.md` | `./.github/skills/bifrost/SKILL.md` |

- `all` 展开：Claude + Codex + Trae + Cursor + Copilot + `.agents/skills`。
- `resolve_target_dirs(tool, custom_dir, cwd)`：`--dir` 只替换父目录；`--cwd` 走项目路径；`--dir` 与 `--cwd` 互斥。
- `install_to_dir()`：创建父目录 → 写入文件（覆盖）。
- `install_to_tool()`：一个 tool 可映射多个目录，逐个写。

### 3. CLI 参数与错误处理

- 参数：`--tool/-t`、`--dir/-d`、`--cwd`、`-y/--yes`。
- 未知 tool：`parse_tool()` 返回错误，列出可选值（含 `github-copilot`/`universal`）。
- 网络错误：DNS/连接超时/非 2xx 分别友好提示；错误发生时若 embedded 可用则回退。
- 权限错误：无写入权限提示使用 `--dir` 或 sudo。
- I/O 错误：磁盘空间/一般 I/O 展示具体 message。
- 终端输出：`colored` 库；成功绿色、失败红色，末尾展示成功/失败数量汇总。

### 4. Skills crate hardening

`crates/skills/src/executor.rs`：

```rust
const HOST_ENV_WHITELIST: &[&str] = &[
    "PATH","HOME","USER","LOGNAME","LANG","LC_ALL","LC_CTYPE",
    "TMPDIR","TEMP","TMP","TERM","SHELL",
    "SSL_CERT_FILE","SSL_CERT_DIR","CARGO_HOME","RUSTUP_HOME",
];
cmd.env_clear();
for key in HOST_ENV_WHITELIST { if let Ok(v) = env::var(key) { cmd.env(key, v); } }
for (k, v) in &manifest.env { cmd.env(k, v); }
```

`crates/skills/src/registry.rs`：

- `reload_one(slug)`：目标目录不存在 → `index.remove(slug)`；存在 → 重新读取 `manifest.json` + `SKILL.md` 并替换单条。
- Watcher（`notify` crate）事件路径 `<root>/<slug>/...` → 提取第一级 `slug` → `reload_one(slug)`。

`crates/skills/src/validator.rs::verify_checksum()`：

- 缺 `manifest.json` → `warn!(...) ; return false`。

`crates/skills/src/packager.rs::import()`：

- 包内 `manifest.scope` 合法（`User`/`Repo`/其它枚举）→ 保留；否则用调用方 `default_scope`。

`crates/skills/src/authoring.rs::test()`：

- 状态不属于 `Drafted|Validated|Tested` → `AuthoringError::InvalidState`；禁止用空 `PathBuf` 继续执行。

### 5. Agent runtime 装配

`crates/agent/src/session.rs`：

- `AgentSession::new_with_work_dir(work_dir, ...)` 构造 `SkillRegistry::open(work_dir)` 并 `with_skills(Arc::new(registry))`。
- session 持同一 registry 供 slash router 与 prompt digest 复用。

`crates/agent/src/skills/mod.rs::SkillsManager::build_skills_instructions()`：

- 输出 `## Available Skills` digest：
  ```
  ## Available Skills
  - foo: One-line description of foo.
  - bar: One-line description of bar.
  ```
- 总长度 ≤ 4KB；按 name 稳定排序；不 eager 注入 `prompt_content`（body 仍在 SKILL.md 文件）。

`crates/agent/src/prompt/mod.rs`：

- base prompt 附加上述 digest；模型按需读取路径。

### 6. 长期记忆 append 锁

`crates/agent/src/skills/memory.rs`（或对应文件）：

```rust
use fs2::FileExt;
let file = OpenOptions::new().create(true).append(true).open(&path)?;
file.lock_exclusive()?;
file.write_all(line.as_bytes())?;
file.unlock()?;
```

- 覆盖 `MEMORY.md`、`raw_memories.md`。
- 单元测试 `append_line_locks_concurrent_writers`：8 线程 × 1000 行验证零交错。

### 7. Admin import + IM CLI secret

`crates/bifrost-admin/src/handlers/agent_skills.rs`：

- `POST /agent/skills/import`：
  - `application/octet-stream` → 直接 body bytes。
  - `multipart/form-data` → 提取 `package` 字段 bytes。
  - Scope：`x-bifrost-skill-scope`，默认 `Repo`。
  - 写 `<agent_data_dir>/skills/.import-tmp/<uuid>.zip` → `SkillPackager::import(&path, scope)`。
- `AgentSkillError` → HTTP：
  - `InvalidArgument` → 400
  - `Conflict` → 409
  - `SemanticValidation` → 422
  - `Io`/其它 → 500

IM CLI `resolve_secret`：

```rust
pub fn resolve_secret(spec: &SecretSpec) -> Result<String, ResolveSecretError> {
    match spec {
        SecretSpec::Env(name) => env::var(name).map_err(|_| ResolveSecretError::Missing(name.clone())),
        SecretSpec::File(path) => fs::read_to_string(path).map_err(|e| ResolveSecretError::Io(e)),
    }
}
```

调用方转成 CLI 配置错误终止执行，不再 warning 后写空 secret。

### 8. Web 表单与 IM Gateway

- `web/src/pages/Settings/tabs/agent/skills/utils/skillFormSchema.ts`：集中 name/description/slash command/required 校验。
- Wizard 与 Editor 复用 Manifest、Script Editor、Test Panel 组件。
- `buildSkillMd()`：非 inline script 引用 `./scripts/run.*`；inline fenced block；遇 ``` 或独立 `---` 行时切换到 ~~~ 或转义。
- IM Gateway Tab `fetchData` 失败 `message.error(err.message)`；`finally` 清理 loading；切 tab 先 reset。

### 9. E2E env 与 storage size guard

- `im_gateway_agent` 涉及 `BIFROST_DATA_DIR` 的用例：
  ```rust
  tokio::task::spawn_blocking(|| {
      let _guard = temp_env::async_with_vars(vars, /* async block via single-thread rt */);
      // build local tokio runtime and block_on(test_body())
  }).await??;
  ```
- `bifrost-core/src/limits.rs`：
  ```rust
  pub const MAX_RULE_FILE_BYTES: u64 = 256 * 1024 * 1024;
  pub fn ensure_file_size_within_limit(path: &Path, limit: u64) -> Result<(), BifrostError> { ... }
  ```
- `bifrost-storage/src/rules.rs::load/load_summary` 复用 helper。

## CLI / Admin API / Web

### CLI

```bash
bifrost install-skill                       # 覆盖式安装所有工具
bifrost install-skill --tool claude-code    # 只装 Claude Code
bifrost install-skill --tool github-copilot # 覆盖 Copilot
bifrost install-skill --tool universal      # 只装 .agents/skills
bifrost install-skill --cwd -t codex        # 项目级 codex + .agents
bifrost install-skill --dir /tmp/skills-out -y  # 自定义目录，跳过确认
```

未知 tool：

```text
Error: unknown tool 'foo'. Supported: claude-code, codex, trae, cursor, github-copilot, universal, all.
```

### Admin API

- `POST /agent/skills/import`（octet-stream / multipart）；scope header `x-bifrost-skill-scope`。
- `AgentSkillError` HTTP mapping 保持稳定。

### Web

- Skill Creator Wizard + SkillEditor 共享 form；`buildSkillMd` 保 frontmatter。
- IM Gateway Tab 错误/loading 语义清晰。

## Sync 边界

- `install-skill` 写本机文件系统，不参与 sync。
- Runtime SkillRegistry 与长期记忆是本机数据。
- Admin import 只落到本机 agent 数据目录。

## 实现切分

### Phase 1：install-skill CLI

- 下载源、嵌入副本、tool 映射表、参数解析。
- `--dir/--cwd` 互斥、`all` 展开、`upgrade` 集成 post-install。
- 单元测试 + E2E `install_skill.rs`。

### Phase 2：Skills crate hardening

- Executor env 白名单；registry `reload_one`；watcher path→slug；checksum；packager scope；authoring 状态机。
- 单元测试全部就绪。

### Phase 3：Agent runtime 装配

- `AgentSession::new_with_work_dir` 装配 registry。
- `SkillsManager::build_skills_instructions` 有界 digest。
- 长期记忆 append 加锁。
- 单元/集成测试。

### Phase 4：Admin import + IM CLI secret

- 改造 `POST /agent/skills/import` 只收 bytes。
- `AgentSkillError` 分层。
- IM CLI `resolve_secret` 强类型。

### Phase 5：Web 表单与 IM Gateway

- `skillFormSchema.ts` 与共用组件。
- `buildSkillMd` fence 转义。
- IM Gateway Tab error/loading。

### Phase 6：E2E env 与 storage size guard

- `temp-env` 隔离 + spawn_blocking + single-thread rt。
- `MAX_RULE_FILE_BYTES` 与 `ensure_file_size_within_limit`。
- storage/e2e 复用。

### Phase 7：human_tests 与文档

- 更新 `human_tests/skill-creator.md` 新增 TC-SC-07..14。
- 新增 `human_tests/agent-runtime-review-fixes.md`（TC-ARF-01..03）。
- 新增 `human_tests/agent-skills-admin-cli.md`（TC-ASAC-01..03）。
- 新增 `human_tests/storage-e2e-safety.md`（TC-SES-01..03）。
- 更新 `human_tests/cli-import-export.md` install-skill 全 tool 回归。
- 更新 `human_tests/readme.md` 索引与计数。
- 同步 `docs/agent-skill.md` / `docs-en/agent-skill.md` 支持的 agent 与路径。

## 测试方案

### 单元测试

- Executor：`process_executor_keeps_common_host_env` 验证白名单。
- Watcher：`watcher_reloads_one_slug_and_removes_deleted_slug`。
- Checksum：`verify_checksum_missing_manifest_returns_false`。
- Packager：`import_preserves_manifest_scope_when_valid`。
- Authoring：`test_rejects_unvalidated_state`。
- Agent runtime：`session_skills_integration`、`system_prompt_includes_bounded_skill_registry_digest`、`test_build_skills_instructions`。
- 长期记忆：`append_line_locks_concurrent_writers`（8×1000 并发）。
- Admin：`multipart_import_extracts_package_field_bytes`、`agent_skill_error_maps_conflict_to_409`。
- IM CLI：`resolve_secret_missing_env_returns_error`、`resolve_secret_missing_file_returns_io_error`。
- Storage：`ensure_file_size_within_limit_rejects_oversized_file`。

### E2E 测试

`bifrost-e2e` 覆盖：

1. `install_skill` 到临时目录：`--dir` + frontmatter 校验。
2. 覆盖安装旧文件被替换。
3. 未知工具错误分支。
4. `all` 模式覆盖 `.claude`/`.codex`/`.agents`/`.trae`/`.github`。
5. `--cwd` 项目级安装；覆盖 `.agents`/`.github`。
6. `--dir` 与 `--cwd` 互斥错误。
7. GitHub Copilot、Universal 单独安装。
8. `test_skill_creator_flow.sh` 覆盖 create → test → invoke → delete → import 主流程。
9. `skill_loading_enabled_skill_appears_in_prompt` 验证 base prompt 含 metadata 不含 body。

约束：`--cwd` 相关用例临时改进程 `current_dir`，属共享全局状态，必须 serial-only 或加锁；否则 Windows CI 会出现首跑误判、单条重试通过的竞态。

### 真实场景测试（human_tests）

- 更新 `human_tests/skill-creator.md`：TC-SC-07..11（Skills crate hardening），TC-SC-12..14（Web 表单）。
- 新增 `human_tests/agent-runtime-review-fixes.md`：TC-ARF-01（session skills 集成）、TC-ARF-02（prompt digest 有界）、TC-ARF-03（长期记忆并发 append）。
- 新增 `human_tests/agent-skills-admin-cli.md`：TC-ASAC-01（multipart import）、TC-ASAC-02（`AgentSkillError` 映射）、TC-ASAC-03（secret 错误终止）。
- 新增 `human_tests/storage-e2e-safety.md`：TC-SES-01（E2E env 隔离编译）、TC-SES-02（storage size guard）、TC-SES-03（rule load 拒绝超限文件）。
- 更新 `human_tests/cli-import-export.md`：install-skill 各 tool 覆盖。

启动 Bifrost 必须使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo build -p bifrost-cli`
- `cargo test -p bifrost-cli install_skill`
- `cargo test -p bifrost-skills` / `crates/skills` 全量
- `cargo test -p bifrost-agent session_skills skills prompt memory`
- `cargo test -p bifrost-admin agent_skills import`
- `cargo test -p bifrost-e2e --no-run`
- `cargo check -p bifrost-storage --quiet`
- `pnpm --dir web build`
- `pnpm --dir web test -- SkillCreatorWizard`
- `cargo test --workspace --all-features`
- `rust-project-validate`
- 本机 no-local-coverage 生效时不跑 `make coverage`；交付时说明。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：install-skill CLI 各 tool、runtime 装配、admin import、CLI secret、Web 表单、E2E env、storage size guard。
- 复核 diff：CLI/skills/agent/admin/web/e2e/storage/human_tests。
- 重点：`--cwd` 全局状态隔离；registry watcher 差异 reload；prompt digest 4KB 上限；长期记忆 lock；import 拒收 client PathBuf。
- 运行受影响单测 + E2E（含 install_skill、skill_creator、skill_loading）。

### 第 2 轮

- 复核第 1 轮问题修复与新增 human_tests 索引。
- 再跑 CLI 全 tool 安装、Playwright fence 转义、真实 skill import。
- 复查 `docs/agent-skill.md` 与 CLI help 是否同步。
- 若第 2 轮仍发现口径问题，追加轮次。

## 风险与决策点

- **覆盖式安装**：始终覆盖用户自行修改的 SKILL.md；符合“始终最新”意图，但需在 CLI help 中提示。
- **网络回退嵌入**：嵌入内容随二进制发布；离线仍可安装，但可能落后 main 分支；`upgrade` 场景可接受，交付说明。
- **`--cwd` 全局状态**：进程级 `current_dir` 切换必须 serial-only；未来考虑绝对路径解析代替。
- **`.agents/skills` 通用目录**：作为标准兜底，若上游标准演进（例如 name/description 长度调整），需要同步更新校验。
- **长期记忆 lock 与 Windows**：`fs2` advisory lock 在部分 Windows/Docker 场景可能返回 unsupported；需要在测试中确认降级或早失败策略。
- **Admin API 兼容**：拒收 client PathBuf 是安全硬约束；前端已切 multipart，如果外部有历史脚本仍传路径，会直接 400，需在 changelog 提醒。
- **`AgentSkillError` 映射**：422 用于语义校验（如 slug 冲突之外的字段非法），后续如果扩展错误码，需要保持稳定 mapping 表。
- **storage size guard**：256MB 是当前上限；超大规则应该走分片或 Group 引用，不放大限制。
