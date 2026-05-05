# Agent P1 Tools 真实场景测试用例

## 功能模块说明

覆盖 Agent 侧本轮新增/对齐的 4 个用户可感知能力：

1. `/goal` 显式目标命令入口
2. Goal 工具生命周期（create/get/update）
3. Codex 风格结构化 `apply_patch`
4. PTY 会话复用、交互模式与 `write_stdin`

## 前置条件

```bash
cd <REPO_ROOT>
export CARGO_TARGET_DIR=./.bifrost-test/agent-p1-target
mkdir -p ./.bifrost-test
```

## 测试用例列表

### TC-APT-01: `/goal` 命令通过 session slash router 激活

- **操作步骤**:
  ```bash
  CARGO_TARGET_DIR=./.bifrost-test/agent-p1-target cargo test -p bifrost-agent --test session_skills_integration goal_slash_command_runs_through_session_router -- --nocapture
  ```
- **预期结果**: 测试通过；`/goal set --budget 128 finish the p1 work` 返回 active goal，响应包含 objective 与 `"active"`。
- **本次执行结果**: 待执行。

### TC-APT-02: `apply_patch` 支持 Codex 风格结构化 patch

- **操作步骤**:
  ```bash
  CARGO_TARGET_DIR=./.bifrost-test/agent-p1-target cargo test -p bifrost-agent apply_patch_tool_works_end_to_end -- --nocapture
  ```
- **预期结果**: 测试通过；`apply_patch` 能处理 `*** Begin Patch` patch，支持 `*** Move to:`，完成 update + move + add + delete，且最终只保留 patch 后文件状态。
- **本次执行结果**: 待执行。

### TC-APT-03: `apply_patch` 兼容 raw patch body 与关键语法边界

- **操作步骤**:
  ```bash
  CARGO_TARGET_DIR=./.bifrost-test/agent-p1-target cargo test -p bifrost-agent tools::apply_patch_diff -- --nocapture
  ```
- **预期结果**: 测试通过；覆盖 raw patch body、空 `@@` hunk、`*** Move to:`、绝对路径拒绝。
- **本次执行结果**: 待执行。

### TC-APT-04: PTY 会话复用与交互输入

- **操作步骤**:
  ```bash
  CARGO_TARGET_DIR=./.bifrost-test/agent-p1-target cargo test -p bifrost-agent pty_tools_work_end_to_end -- --nocapture
  ```
- **预期结果**: 测试通过；同一 session 可复用环境变量，前台交互模式返回 `exit_indicator: running`，`write_stdin` 可以把输入送入前台进程并读回输出。
- **本次执行结果**: 待执行。

### TC-APT-05: 目标工具全量回归

- **操作步骤**:
  ```bash
  CARGO_TARGET_DIR=./.bifrost-test/agent-p1-target cargo test -p bifrost-agent -- --nocapture
  ```
- **预期结果**: 测试通过；本轮 Goal、`apply_patch`、PTY 与 `/goal` 接入不会破坏 `bifrost-agent` 既有能力。
- **本次执行结果**: 待执行。

### TC-APT-06: PTY exit_code 解析兼容交互式 shell prompt 前缀

- **操作步骤**:
  ```bash
  cargo test -p bifrost-agent tools::pty_shell::tests::test_pty_shell -- --nocapture
  ```
- **预期结果**: 测试通过；当交互式 shell 将 prompt 与 sentinel 输出放在同一行时，`shell_pty` 仍能解析 `exit_code: 0` 与 `exit_code: 1`。
- **本次执行结果**: 通过。2026-05-05 执行后 PTY shell 指定单测均 passed。

## 清理步骤

```bash
rm -rf ./.bifrost-test/agent-p1-target
```
