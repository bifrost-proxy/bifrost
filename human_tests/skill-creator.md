# Skill Creator 真实场景测试用例

## 功能模块说明

Skill Creator 子系统让 Bifrost Agent 在 Settings -> Agent 中管理 skill，支持查看详情、删除、启用/禁用、导入 zip 包。新建 skill 必须通过 Agent 对话或导入 zip 包，WebUI 不提供直接创建和编辑功能。

## 前置条件

```bash
cd <REPO_ROOT>
export BIFROST_DATA_DIR="$(mktemp -d /tmp/bifrost-skill-creator-human.XXXXXX)"
```

如需启动服务验证 WebUI，必须使用临时数据目录且带 `--no-system-proxy`：

```bash
BIFROST_DATA_DIR="$BIFROST_DATA_DIR" cargo run --bin bifrost -- start -p 8800 --unsafe-ssl --no-system-proxy
```

## 测试用例列表

### TC-SC-01: skills crate 核心模型、校验、存储和打包回归

- **操作步骤**: `cargo test -p skills`
- **预期结果**: 通过所有 `skills` 单元测试，覆盖 manifest serde、validator、checksum、commit 归档、registry slash、executor inline、packager import/export 和 authoring 状态机。

### TC-SC-02: Agent slash router 与 skill_creator meta-tool 回归

- **操作步骤**: `cargo test -p bifrost-agent slash:: tools::skill_creator:: -- --nocapture`
- **预期结果**: `/remember` 等内置命令由 router 分发，skill slash command 可解析到 skill，`skill_creator.start` 返回 `session_id`。

### TC-SC-03: Admin Skill CRUD 回归

- **操作步骤**: `cargo test -p bifrost-admin agent_skills -- --nocapture`
- **预期结果**: Skill CRUD store happy path 通过；同 scope slash command 冲突被拒绝。

### TC-SC-04: Skill Creator E2E 主流程

- **操作步骤**: `bash e2e-tests/tests/test_skill_creator_flow.sh`
- **预期结果**: 自动跑通 create -> test -> invoke -> delete -> import，删除后 `/weather` 不再解析，导入后 skill 恢复。

### TC-SC-05: WebUI Skills 面板构建与 lint

- **操作步骤**:
  ```bash
  cd web
  pnpm run lint
  pnpm run build
  ```
- **预期结果**: ESLint 零 error；TypeScript build 通过；Settings -> Agent 中 Skills 面板挂在 Memory Records 后、MCP Servers 前。

### TC-SC-06: WebUI Skills 面板无 New Skill 按钮、无 Edit 按钮

- **操作步骤**:
  1. 启动带临时数据目录的 Bifrost 测试实例，端口使用 `8800`，必须带 `--no-system-proxy`。
  2. 浏览器打开 `http://127.0.0.1:8800/_bifrost/`。
  3. 进入 Settings -> Agent，定位 Skills 面板。
- **预期结果**:
  - 面板右上角无 `+ New Skill` 按钮
  - 每行 Actions 列无编辑（铅笔）图标，只有查看详情（眼睛）图标和删除图标
  - 右上角有 Import 按钮和 scope 选择器

### TC-SC-07: WebUI Skills 查看详情弹窗

- **操作步骤**:
  1. 在 Skills 列表中找到任意一个 skill，点击眼睛图标
- **预期结果**:
  - 弹出只读详情弹窗，标题为 `Skill: <skill-name>`
  - 弹窗中展示 Name、Version、Scope、Enabled、Description、Entrypoint、Triggers、Allowed Tools、Path、Checksum 等信息
  - 如果有 SKILL.md 内容，展示预格式化文本
  - 弹窗无 Save/Edit 按钮，只有关闭
  - 弹窗 footer 为空（无操作按钮）

### TC-SC-08: WebUI Skills Import ZIP 功能

- **操作步骤**:
  1. 在 Skills 面板右上角选择 scope（默认 Repo）
  2. 点击 Import 按钮
  3. 在文件选择器中选择一个 .zip 格式的 skill 包
- **预期结果**:
  - 文件选择器只接受 `.zip` 文件
  - 导入成功后显示 toast 提示 `Skill "xxx" imported`
  - Skills 列表中出现新导入的 skill

### TC-SC-09: WebUI Skills 列表分页固定 10 条

- **操作步骤**:
  1. 确保 Skills 列表中有超过 10 个 skill
  2. 观察分页器
- **预期结果**:
  - 每页固定显示 10 条
  - 无 page size 切换器
  - 使用简洁分页样式

### TC-SC-10: WebUI Skills 删除功能

- **操作步骤**:
  1. 找到一个非 system scope 的 skill，点击删除图标
  2. 在确认弹窗中点击确认
- **预期结果**:
  - 弹出确认弹窗 "Delete skill?"
  - 确认后 skill 从列表中移除
  - System scope 的 skill 删除按钮为禁用状态

### TC-SC-11: Executor 环境白名单回归

- **操作步骤**:
  ```bash
  CARGO_TARGET_DIR=./.codex-target/fixpass cargo test -p skills process_executor_keeps_common_host_env --quiet
  ```
- **预期结果**: 测试通过；子进程在 `env_clear()` 后仍能读取非空 `HOME` 与 `PATH`。

### TC-SC-12: Registry watcher 单 slug 热重载回归

- **操作步骤**:
  ```bash
  CARGO_TARGET_DIR=./.codex-target/fixpass cargo test -p skills watcher_reloads_one_slug_and_removes_deleted_slug --quiet
  ```
- **预期结果**: 测试通过；修改 `weather/SKILL.md` 后 registry description 更新，删除 `weather` 后索引移除，`notes` skill 保持存在。

### TC-SC-13: checksum 缺失 manifest 回归

- **操作步骤**:
  ```bash
  CARGO_TARGET_DIR=./.codex-target/fixpass cargo test -p skills verify_checksum_missing_manifest_returns_false --quiet
  ```
- **预期结果**: 测试通过；缺失 `manifest.json` 的 skill 目录返回 `false`，不再误报校验成功。

### TC-SC-14: packager import scope 保留回归

- **操作步骤**:
  ```bash
  CARGO_TARGET_DIR=./.codex-target/fixpass cargo test -p skills import_preserves_manifest_scope_when_valid --quiet
  ```
- **预期结果**: 测试通过；将 scope=`repo` 的包按 User 默认导入后，导入记录仍保持 `Repo` scope。

### TC-SC-15: authoring.test 非法状态回归

- **操作步骤**:
  ```bash
  CARGO_TARGET_DIR=./.codex-target/fixpass cargo test -p skills test_rejects_unvalidated_state --quiet
  ```
- **预期结果**: 测试通过；`SkillAuthoringSession::start()` 后未 validate 直接 `test()` 返回 `AuthoringError::InvalidState`。

## 清理步骤

```bash
rm -rf "$BIFROST_DATA_DIR"
```
