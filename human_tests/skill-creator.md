# Skill Creator 真实场景测试用例

## 功能模块说明

Skill Creator 子系统让 Bifrost Agent 在 Settings -> Agent 中管理、创建、测试、编辑、打包、删除和导入 skill，并让 Agent Loop 能通过 `/skill` 与 skill 自定义 slash command 触发 skill。

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
- **本次执行结果**: 2026-05-03 复跑通过。回归覆盖 `ModelProviderConfig.api_key` 新字段补齐后，`test_skill_creator_flow.sh` 能重新编译 `bifrost-agent`/`bifrost-e2e` 并完成 `skill_creator_create_test_invoke_delete_import`，结果 `1 passed, 0 failed`。

### TC-SC-05: WebUI Skills 面板构建与 lint

- **操作步骤**:
  ```bash
  cd web
  pnpm run lint
  pnpm run build
  ```
- **预期结果**: ESLint 零 error；TypeScript build 通过；Settings -> Agent 中 Skills 面板挂在 Memory Records 后、MCP Servers 前。

### TC-SC-06: WebUI 亮色/暗色主题可读性回归

- **操作步骤**:
  1. 启动带临时数据目录的 Bifrost 测试实例，端口使用 `8800`，必须带 `--no-system-proxy`。
  2. 浏览器打开 `http://127.0.0.1:8800/_bifrost/`。
  3. 进入 Settings -> Agent，定位 Memory Records 后的 Skills 面板。
  4. 在亮色主题下打开 New Skill，依次检查 Metadata、Entrypoint、Tools、Test 四步表单文字、按钮、输入框可读。
  5. 切换暗色主题，重复第 4 步。
- **预期结果**: Skills 列表、New Skill 向导、Edit 弹窗在亮色和暗色主题下文字不重叠，按钮可识别，表单错误提示可读。

### TC-SC-07: Executor 环境白名单回归

- **操作步骤**:
  ```bash
  CARGO_TARGET_DIR=./.codex-target/fixpass cargo test -p skills process_executor_keeps_common_host_env --quiet
  ```
- **预期结果**: 测试通过；子进程在 `env_clear()` 后仍能读取非空 `HOME` 与 `PATH`。
- **本次执行结果**: 2026-05-03 通过，结果 `1 passed, 0 failed`。

### TC-SC-08: Registry watcher 单 slug 热重载回归

- **操作步骤**:
  ```bash
  CARGO_TARGET_DIR=./.codex-target/fixpass cargo test -p skills watcher_reloads_one_slug_and_removes_deleted_slug --quiet
  ```
- **预期结果**: 测试通过；修改 `weather/SKILL.md` 后 registry description 更新，删除 `weather` 后索引移除，`notes` skill 保持存在。
- **本次执行结果**: 2026-05-03 初次执行暴露 watcher 事件路径与 root 路径 canonicalization 不一致，修复后复跑通过，结果 `1 passed, 0 failed`。

### TC-SC-09: checksum 缺失 manifest 回归

- **操作步骤**:
  ```bash
  CARGO_TARGET_DIR=./.codex-target/fixpass cargo test -p skills verify_checksum_missing_manifest_returns_false --quiet
  ```
- **预期结果**: 测试通过；缺失 `manifest.json` 的 skill 目录返回 `false`，不再误报校验成功。
- **本次执行结果**: 2026-05-03 通过，结果 `1 passed, 0 failed`。

### TC-SC-10: packager import scope 保留回归

- **操作步骤**:
  ```bash
  CARGO_TARGET_DIR=./.codex-target/fixpass cargo test -p skills import_preserves_manifest_scope_when_valid --quiet
  ```
- **预期结果**: 测试通过；将 scope=`repo` 的包按 User 默认导入后，导入记录仍保持 `Repo` scope。
- **本次执行结果**: 2026-05-03 通过，结果 `1 passed, 0 failed`。

### TC-SC-11: authoring.test 非法状态回归

- **操作步骤**:
  ```bash
  CARGO_TARGET_DIR=./.codex-target/fixpass cargo test -p skills test_rejects_unvalidated_state --quiet
  ```
- **预期结果**: 测试通过；`SkillAuthoringSession::start()` 后未 validate 直接 `test()` 返回 `AuthoringError::InvalidState`。
- **本次执行结果**: 2026-05-03 通过，结果 `1 passed, 0 failed`。

## 清理步骤

```bash
rm -rf "$BIFROST_DATA_DIR"
```
