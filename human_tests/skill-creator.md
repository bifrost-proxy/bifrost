# Skill Creator 真实场景测试用例

## 功能模块说明

Skill Creator 子系统让 Bifrost Agent 在 Settings -> Agent 中管理、创建、测试、编辑、打包、删除和导入 skill，并让 Agent Loop 能通过 `/skill` 与 skill 自定义 slash command 触发 skill。

## 前置条件

```bash
cd /Users/eden/work/github/bifrost
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

### TC-SC-06: WebUI 亮色/暗色主题可读性回归

- **操作步骤**:
  1. 启动带临时数据目录的 Bifrost 测试实例，端口使用 `8800`，必须带 `--no-system-proxy`。
  2. 浏览器打开 `http://127.0.0.1:8800/_bifrost/`。
  3. 进入 Settings -> Agent，定位 Memory Records 后的 Skills 面板。
  4. 在亮色主题下打开 New Skill，依次检查 Metadata、Entrypoint、Tools、Test 四步表单文字、按钮、输入框可读。
  5. 切换暗色主题，重复第 4 步。
- **预期结果**: Skills 列表、New Skill 向导、Edit 弹窗在亮色和暗色主题下文字不重叠，按钮可识别，表单错误提示可读。

## 清理步骤

```bash
rm -rf "$BIFROST_DATA_DIR"
```
