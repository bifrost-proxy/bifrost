# bifrost install-skill 技能安装方案

## 功能模块描述

为 `bifrost-cli` 新增 `install-skill` 子命令，用于从 GitHub 远端主干（main 分支）下载最新的 `SKILL.md` 文件，并安装到各 AI 编程工具的全局配置目录中。

目标语义：

- 每次安装都是覆盖式安装（overwrite），确保用户始终获取最新版本的技能文件
- 支持专用目录与通用 Agent Skills 目录的混合安装：
  - 专用目录：Claude Code、Trae、Trae CN、Cursor、GitHub Copilot
  - 通用目录：`.agents/skills`，用于兼容 Codex 以及更多遵循 Agent Skills 标准的运行时
- 默认安装到全部工具，支持通过 `--tool`（`-t`）参数选择单个工具
- 支持通过 `--dir`（`-d`）参数自定义安装目录，覆盖默认路径
- 支持 `-y` 跳过安装确认提示，适用于脚本/自动化场景

## 实现逻辑

### 一、下载源

从 GitHub 远端主干获取最新 SKILL.md：

```
https://raw.githubusercontent.com/bifrost-proxy/bifrost/main/SKILL.md
```

使用 `ureq` 发起 HTTP GET 请求，读取响应体为字符串作为技能文件原始内容。

### 二、工具与路径映射

所有工具统一使用 `skills/bifrost/SKILL.md` 目录结构，遵循 Standard Agent Skills Format 规范：

| 工具名 | 标识 | 全局安装路径 | 项目级安装路径（--cwd） |
| ---------- | ---------- | -------------------------------- | ------------------------------- |
| Claude Code | `claude-code`, `claude` | `~/.claude/skills/bifrost/SKILL.md` | `./.claude/skills/bifrost/SKILL.md` |
| Codex / 通用 Agent Skills | `codex`, `openai-codex`, `universal` | `~/.codex/skills/bifrost/SKILL.md` + `~/.agents/skills/bifrost/SKILL.md` | `./.codex/skills/bifrost/SKILL.md` + `./.agents/skills/bifrost/SKILL.md` |
| Trae | `trae` | `~/.trae/skills/bifrost/SKILL.md` + `~/.trae-cn/skills/bifrost/SKILL.md` | `./.trae/skills/bifrost/SKILL.md` |
| Cursor | `cursor` | `~/.cursor/skills/bifrost/SKILL.md` | `./.cursor/skills/bifrost/SKILL.md` |
| GitHub Copilot | `github-copilot`, `copilot` | `~/.copilot/skills/bifrost/SKILL.md` | `./.github/skills/bifrost/SKILL.md` |

设计约束：

- Trae 在全局模式下同时安装到 `.trae` 和 `.trae-cn` 两个目录（适配国内外版本），项目级安装仅安装到 `.trae`
- Codex 保留历史兼容路径 `.codex/skills`，同时补充标准通用目录 `.agents/skills`
- GitHub Copilot 增加专用目录支持，项目级目录使用 `.github/skills`
- `all` 模式默认包含以上全部目标，以便在一条命令里覆盖专用 agent 和更多标准兼容 agent

SKILL.md 源文件自带标准 YAML frontmatter（`name` + `description`），下载后直接写入，不做任何额外处理。

各工具的 skill 自动发现机制基于 frontmatter 中的 `name` 和 `description` 字段：
- `name`：skill 标识符，≤ 64 字符
- `description`：触发匹配描述，≤ 1024 字符，AI 通过此字段判断何时加载该 skill

### 三、CLI 参数设计

```bash
bifrost install-skill [OPTIONS]
```

参数说明：

- `--tool`（`-t`）：指定安装目标工具，可选值为 `claude-code`、`codex`、`trae`、`cursor`、`github-copilot`、`universal`、`all`，默认为 `all`
- `--dir`（`-d`）：自定义安装目录，覆盖工具的默认安装路径。指定后文件名保持不变，仅替换父目录
- `-y`：跳过确认提示，直接执行安装

### 四、安装流程

1. 解析命令行参数，确定目标工具列表；其中 `all` 会展开为全部专用目标 + 通用目录目标
2. 从远端下载 SKILL.md 内容
3. 若未指定 `-y`，展示将要安装的工具与目标路径，等待用户确认
4. 遍历目标工具列表，逐个执行安装：
   - 创建目标路径的父目录（若不存在）
   - 一个工具可映射到多个目录（例如 Trae、Codex）
   - 写入目标文件（覆盖已有文件）
5. 输出安装结果，包含成功/失败的工具及路径

### 五、错误处理

网络错误：

- DNS 解析失败：提示网络不可用或检查代理配置
- 连接超时：提示重试或检查网络连接
- HTTP 非 2xx 状态码：提示远端文件不可用，展示状态码

权限错误：

- 目标路径无写入权限：提示使用 `sudo` 或通过 `--dir` 指定有权限的目录

写入失败：

- 磁盘空间不足或其他 I/O 错误：提示具体错误信息

未知工具名称：

- `--tool` 传入不支持的工具名时，提示可选值列表，并包含 `github-copilot` 与 `universal`

### 六、终端输出

使用 `colored` 库实现彩色输出：

- 成功安装：绿色标记，展示工具名与安装路径
- 安装失败：红色标记，展示工具名与错误原因
- 安装总结：展示成功/失败数量

## 依赖项

- `ureq`：已有依赖，用于 HTTP 下载 SKILL.md 文件
- `dirs`：已有依赖，用于获取用户 home 目录以拼接默认安装路径
- `colored`：已有依赖，用于终端彩色输出

无需引入新依赖。

## 测试方案

### Skills crate hardening 回归（feat/agent review fixpass）

本模块的运行时 skill 执行、注册表热重载、checksum、导入和 authoring 状态机需要满足以下约束：

1. Executor 在 `env_clear()` 后保留必要宿主环境白名单，包括 `PATH`、`HOME`、`USER`、`LOGNAME`、`LANG`、`LC_ALL`、`LC_CTYPE`、`TMPDIR`、`TEMP`、`TMP`、`TERM`、`SHELL`、`SSL_CERT_FILE`、`SSL_CERT_DIR`、`CARGO_HOME`、`RUSTUP_HOME`，再叠加 `SkillManifest.env`。
2. `SkillRegistry::reload_one(slug)` 只能重建对应 slug。目录不存在时删除索引；其他 skill 保持不变；watcher 必须从文件事件路径反推出 root 下的第一级 slug 并触发差异重载。
3. `verify_checksum()` 遇到缺失 `manifest.json` 必须返回 `false` 并记录 warning，交由上层处理。
4. `SkillPackager::import()` 保留包内合法 `manifest.scope`，仅当 scope 缺失或非法时使用调用方默认 scope。
5. `SkillAuthoringSession::test()` 在非 `Drafted`/`Validated`/`Tested` 状态下返回 `AuthoringError::InvalidState`，禁止用空 `PathBuf` 继续执行。

验证计划：

- 单元测试：`process_executor_keeps_common_host_env` 验证白名单环境变量；`watcher_reloads_one_slug_and_removes_deleted_slug` 验证写入、删除和其他 skill 不受影响；`verify_checksum_missing_manifest_returns_false` 验证缺失 manifest 返回 false；`import_preserves_manifest_scope_when_valid` 验证合法 scope 保留；`test_rejects_unvalidated_state` 验证非法状态报错。
- E2E 测试：复用 `test_skill_creator_flow.sh` 覆盖 create -> test -> invoke -> delete -> import 主流程。
- 真实场景测试：更新 `human_tests/skill-creator.md`，新增 TC-SC-07 到 TC-SC-11，逐条执行对应 cargo/脚本命令并记录实际结果。

### Agent runtime skill 接入回归（feat/agent review fixpass）

Agent runtime 必须在 `AgentSession::new_with_work_dir` 构造出的真实 session 中装配 `SkillRegistry`，而不是只在 slash router 单元测试中手工注入。`with_skills(Arc<SkillRegistry>)` 保持链式 API 兼容，同时 session 持有同一个 registry 供 slash router 和 prompt digest 复用。

System prompt 末尾追加一个有界 `## Available Skills` digest：每个 enabled skill 一行 `- <name>: <description_one_line>`，总长度不超过 4KB；当前缺少使用次数和最近使用时间统计时按名称稳定排序，后续有统计字段后可提升排序策略。

长期记忆 append 使用 `fs2::FileExt::lock_exclusive()` 对目标文件加 advisory exclusive lock，保护多个本地 session 同时追加 `MEMORY.md` 或 `raw_memories.md` 时不会出现行交错。

验证计划：

- 单元/集成测试：`session_skills_integration` 验证真实 `AgentSession::new_with_work_dir` 下 `/skill list` 能看到 work dir skill；`append_line_locks_concurrent_writers` 验证 8x1000 并发写入行完整；`system_prompt_includes_bounded_skill_registry_digest` 验证 prompt digest 注入 3 个 skill。
- 真实场景测试：新增 `human_tests/agent-runtime-review-fixes.md`，包含 TC-ARF-01 到 TC-ARF-03，逐条执行并记录实际结果。

### Admin import 与 CLI secret 回归（feat/agent review fixpass）

`/agent/skills/import` 禁止继续接受客户端传入的本机 `PathBuf`。接口改为读取 `application/octet-stream` 原始包 bytes，或 multipart/form-data 中的 `package` 字段；服务端将 bytes 暂存到 agent 数据目录下 `skills/.import-tmp/` 后再调用 `SkillPackager::import()`。scope 可通过 `x-bifrost-skill-scope` 请求头传入，默认 `Repo`。

管理端 skill handler 使用 `AgentSkillError` 分层映射：参数错误为 400，冲突为 409，语义校验失败为 422，未知 I/O 为 500。后续如果 handler 迁移到 typed JSON response，可以保留同一映射表。

IM CLI 的 `resolve_secret` 返回 `Result<String, ResolveSecretError>`。缺失 env 映射为 `Missing`，文件读取失败映射为 `Io`，调用方转成 CLI 配置错误并停止，不再 warning 后写入空 secret。

验证计划：

- 单元测试：`multipart_import_extracts_package_field_bytes`、`agent_skill_error_maps_conflict_to_409`、`resolve_secret_missing_env_returns_error`、`resolve_secret_missing_file_returns_io_error`。
- 真实场景测试：新增 `human_tests/agent-skills-admin-cli.md`，包含 TC-ASAC-01 到 TC-ASAC-03，逐条执行并记录实际结果。

### E2E 测试

新增 `bifrost-e2e` 覆盖以下场景：

1. 安装到临时目录验证文件正确写入：使用 `--dir` 指定临时目录，验证新增工具仍能写入正确文件
2. 覆盖安装验证旧文件被替换：先写入旧内容，再执行安装，验证文件内容更新为最新版本
3. frontmatter 验证：安装后检查文件是否包含标准 YAML frontmatter（`name` 和 `description` 字段），确保兼容所有工具的 skill 自动发现机制
4. 未知工具名称的错误处理：传入无效的 `--tool` 参数，验证 CLI 返回正确错误信息
5. 全部工具安装验证：不指定 `--tool`，验证 `all` 模式会覆盖 `.claude`、`.codex`、`.agents`、`.trae`、`.github` 等目录
6. `--cwd` 项目级安装验证：验证文件写入到当前目录下的 `.<tool>/skills/bifrost/SKILL.md`，并覆盖 `.agents` 与 `.github`
7. `--dir` 和 `--cwd` 互斥验证：同时传入两个参数时返回互斥错误
8. GitHub Copilot 验证：`-t github-copilot` 时安装到 Copilot 专用目录
9. Universal 验证：`-t universal` 时仅安装到 `.agents/skills/bifrost/SKILL.md`

测试约束补充：

- `--cwd` 相关 E2E 会临时切换进程级 `current_dir`，这属于共享全局状态；在并发 runner 中必须加串行保护，避免不同用例互相污染工作目录
- Windows CI 上如果缺少这层保护，会出现首跑误判“目标文件不存在”、单条重试立刻通过的竞态现象，因此该类用例必须以“首跑稳定通过”为目标

## 校验要求

- `cargo build -p bifrost-cli` 编译通过
- `cargo test --workspace --all-features` 通过
- `rust-project-validate` 通过

## 文档更新要求

- `docs/agent-skill.md` 同步更新支持的 agent 与路径说明
- `human_tests/cli-import-export.md` 补充 install-skill 更多 agent 兼容回归用例
- `human_tests/readme.md` 索引同步更新测试用例数量
