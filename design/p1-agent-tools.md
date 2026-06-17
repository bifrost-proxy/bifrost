# P1 Agent Tools 对齐设计

## 功能概述

本次实现 3 个 P1 能力，并补上一条显式的用户入口：

1. Goal 系统：保留 `get_goal` / `create_goal` / `update_goal` 三工具，并新增 `/goal` 专用命令，供普通会话和 IM Gateway 直接激活。
2. Apply Patch Diff：用结构化 `apply_patch` 取代旧的 search-and-replace 方案，只保留一套补丁工具。
3. Shell PTY：提供持久会话、`write_stdin`、交互前台模式和退出码回传。

## 实现逻辑

### Goal

- 工具层位于 `crates/agent/src/tools/goal.rs`，三件套工具（`get_goal` / `create_goal` / `update_goal`）通过 `execute_goal_tool` 调度；运行时状态作为 `current_goal: Option<GoalState>` 直接挂在 `AgentSession` 上，由 `goal_runtime_apply` + `GoalRuntimeEvent` 事件流推动，避免 cross-session goal 泄漏（GoalManager 这一中间层在现有实现里并不存在）。
- Goal 工具对外返回 `ThreadGoal` 快照：使用 `threadId` 作为线程目标标识，不暴露内部 `goalId`；`GoalStatus` 使用 camelCase 序列化，例如 `budgetLimited`。
- 会话入口位于 `crates/agent/src/slash.rs` 与 `crates/agent/src/session/turn_loop.rs`：
  - `/goal`
  - `/goal show`
  - `/goal set <objective>`
  - `/goal set --budget <N> <objective>`
  - `/goal pause`
  - `/goal resume`
  - `/goal complete`
- IM Gateway 继续复用 `run_turn()` 路径，因此 `/goal` 在 Feishu/IM 侧不需要额外协议，只需要把忙碌态命令识别补进 `crates/bifrost-admin/src/handlers/im_gateway/agent_chat.rs`（顶层 `handlers/im_gateway.rs` 作为模块入口 re-export `busy_message_mode` 等子模块）。

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

### Unified Exec Terminal

- 实现位于 `crates/agent/src/tools/exec_command.rs`。
- 只保留 `exec_command` + `write_stdin` 一套终端协议；旧 `shell_pty` 持久 shell session 已移除。
- `exec_command` 负责真实子进程和可选 PTY，长任务在初始 `yield_time_ms` 后返回数字 `session_id`。
- `write_stdin` 只写入或轮询 `ExecSessionManager` 中的真实进程 session，不再存在 sentinel-based shell fallback。
- `write_stdin` 属于会话型状态工具，必须按模型给出的顺序执行，不进入 local parallel tool batch，避免同一 `session_id` 的 stdin 写入和输出 drain 乱序。
- 长任务在初始 yield 前保存到 `ExecSessionManager`，后台 watcher 持续观察真实进程退出；即使模型没有立即继续 poll，后续 `write_stdin` 也能拿到真实 `exit_code` 和尾部输出。
- `yield_time_ms` 与 Codex unified exec 对齐：`exec_command` 默认 10000ms，非空 `write_stdin` 默认 250ms，空输入 poll 至少 5000ms 且受 `background_terminal_max_timeout` 上限控制。
- 输出 schema 统一包含：
  - `session_id`
  - `exit_code`
  - `output`

## 后续增强方向

- Goal：当前只实现目标状态与预算字段，后续可扩展 token/time usage 挂载到 session runtime 并在完成时回报预算消耗。
- Apply Patch：运行时已兼容 freeform raw patch，后续可考虑独立 freeform grammar tool 注册方式。
- Unified Exec：当前已覆盖 pipe/PTY、stdin 写入、有序轮询、后台结束监听、Ctrl-C 取消与输出截断；后续增强只在 `exec_command`/`write_stdin` 这一套协议内扩展。
- Worktree：`enter_worktree` / `exit_worktree` 属于有序状态工具。工具成功后必须立即更新当前 turn loop 的 `work_dir`，不能等到整批 tool calls 结束后再切换；否则同一 assistant tool-call 响应中跟在 `enter_worktree` 后面的 `read_file`、`apply_patch` 或 `exec_command` 仍会落在旧目录，违背“保留上下文但切换工作区”的用户预期。退出 worktree 时同样需要立即恢复原目录，且不能重复消费同一信号，避免把 worktree 自身误记为 original dir。

## 测试方案

### 单元测试

- `apply_patch_diff`：
  - `test_parse_update_with_lowercase_move_to`
  - `test_parse_update_chunk_with_bare_context_marker`
  - `test_apply_rejects_absolute_path`
  - `test_apply_diff_tool_accepts_raw_patch_body`
- `exec_command`：
  - `exec_command_returns_completed_output`
  - `exec_command_yields_session_and_write_stdin_polls_to_exit`
  - `exec_command_background_watcher_observes_exit_before_next_poll`
  - `exec_command_write_stdin_drives_pipe_process`
  - `exec_command_yield_defaults_and_clamps_match_codex_unified_exec`
  - `test_exec_command_tty_reports_isatty_true`
- `worktree`：
  - `enter_worktree_signal_updates_session_work_dir_immediately`
  - `exit_worktree_signal_restores_original_work_dir`
  - `failed_worktree_signal_keeps_current_work_dir`
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
- 覆盖 `enter_worktree` 成功后后续工具立即进入新 worktree、`exit_worktree` 恢复原目录、失败结果不污染 session work_dir 的回归
- 按文档逐条执行并记录结果

## 校验要求

- `cargo test -p bifrost-agent`
- `cargo test -p bifrost-agent --test p1_tools_e2e --test session_skills_integration`
- `cargo test --workspace --all-features`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `rust-project-validate`
