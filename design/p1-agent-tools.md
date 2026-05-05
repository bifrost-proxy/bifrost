# P1 Agent Tools 对齐设计

## 功能概述

本次实现对齐 Codex 的 3 个 P1 能力，并补上一条显式的用户入口：

1. Goal 系统：保留 `get_goal` / `create_goal` / `update_goal` 三工具，并新增 `/goal` 专用命令，供普通会话和 IM Gateway 直接激活。
2. Apply Patch Diff：用结构化 `apply_patch` 取代旧的 search-and-replace 方案，只保留一套补丁工具。
3. Shell PTY：提供持久会话、`write_stdin`、交互前台模式和退出码回传。

## 实现逻辑

### Goal

- 工具层位于 `crates/agent/src/tools/goal.rs`，统一由 `GoalManager` 持有状态。
- Goal 工具对外返回 Codex `ThreadGoal` 兼容快照：使用 `threadId` 作为线程目标标识，不暴露内部 `goalId`；`GoalStatus` 使用 camelCase 序列化，例如 `budgetLimited`。
- 会话入口位于 `crates/agent/src/slash.rs` 与 `crates/agent/src/session.rs`：
  - `/goal`
  - `/goal show`
  - `/goal set <objective>`
  - `/goal set --budget <N> <objective>`
  - `/goal complete`
- IM Gateway 继续复用 `run_turn()` 路径，因此 `/goal` 在 Feishu/IM 侧不需要额外协议，只需要把忙碌态命令识别补进 `crates/bifrost-admin/src/handlers/im_gateway.rs`。

### Apply Patch

- 结构化补丁实现位于 `crates/agent/src/tools/apply_patch_diff/`。
- 对齐点：
  - 工具名统一为 `apply_patch`
  - 支持 JSON `input` 字段
  - 兼容裸 freeform patch 文本
  - 支持 `*** Move To:` 与 `*** Move to:`
  - 支持空 `@@` hunk
  - 拒绝绝对路径，只允许相对路径
- 旧 `patch.rs` 已移除，避免同一目标保留两套工具。

### Shell PTY

- 实现位于 `crates/agent/src/tools/pty_shell.rs`。
- 保留持久 session + `write_stdin` 交互。
- 新增 `wait_for_completion=false` 前台交互模式，允许先返回 `session_id`，再由 `write_stdin` 驱动前台程序。
- 为了避免 macOS 交互式 shell 启动脚本、prompt 与控制序列污染 sentinel 检测，session 默认改为启动“干净 shell”（zsh 使用 `-f`，bash 使用 `--noprofile --norc`）。
- stdout/stderr 改为按原始字节块持续读取，而不是按行读取，确保无换行输出、prompt 前缀和前台交互场景都能稳定保留到缓冲区。
- 输出补齐：
  - `session_id`
  - `exit_indicator`
  - `exit_code`
- `exit_code` 解析不能假设 sentinel 位于行首；交互式 zsh/bash 可能把 prompt 与 sentinel 放在同一行，解析时需在行内查找 sentinel 前缀。

## 与 Codex 的剩余差异

- Goal：当前只实现目标状态与预算字段，还没有像 Codex 那样把 token/time usage 挂到 session runtime 并在完成时回报预算消耗。
- Apply Patch：运行时已兼容 freeform raw patch，但工具注册层仍是 JSON schema，不是 Codex 那套独立 freeform grammar tool。
- Shell PTY：当前是“PTY-like”持久 shell，会话与交互能力已覆盖核心场景，但还没有 Codex unified exec 那种完整的 `tty`/`max_output_tokens`/统一输出 schema。

## 测试方案

### 单元测试

- `apply_patch_diff`：
  - `test_parse_update_with_lowercase_move_to`
  - `test_parse_update_chunk_with_bare_context_marker`
  - `test_apply_rejects_absolute_path`
  - `test_apply_diff_tool_accepts_raw_patch_body`
- `pty_shell`：
  - `test_pty_shell_simple_command`
  - `test_pty_shell_reports_non_zero_exit_code`
  - `test_pty_shell_interactive_mode_returns_running`
  - `test_write_stdin_to_session`
- `goal`：create/get/update/duplicate/thread-safety 全覆盖。

### E2E 测试

- 新增 `crates/agent/tests/p1_tools_e2e.rs`：
  - goal 生命周期黑盒验证，断言响应包含 `threadId` 且不暴露内部 `goalId`
  - `apply_patch` 多文件 patch 黑盒验证
  - PTY 会话复用 + 交互输入黑盒验证
- 扩展 `crates/agent/tests/session_skills_integration.rs`：
  - `/goal set --budget ...` 通过 session slash router 黑盒验证，保持 ThreadGoal 输出契约

### 真实场景测试

- 新增 `human_tests/agent-p1-tools.md`
- 覆盖 `/goal`、IM/会话入口、`apply_patch`、PTY 交互链路
- 覆盖交互式 shell prompt 前缀不影响 `exit_code` 解析的回归
- 按文档逐条执行并记录结果

## 校验要求

- `cargo test -p bifrost-agent`
- `cargo test -p bifrost-agent --test p1_tools_e2e --test session_skills_integration`
- `cargo test --workspace --all-features`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `rust-project-validate`
