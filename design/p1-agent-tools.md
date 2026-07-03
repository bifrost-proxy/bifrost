# P1 Agent Tools 对齐设计

## 背景

Bifrost Agent 与 Codex Protocol 对齐的第三阶段，聚焦补齐 3 组 P1 能力并提供一个显式用户入口：

1. **Goal 系统**：让会话拥有可持久的目标、预算限制、生命周期状态；同时提供 `/goal` slash command 让普通会话与 IM Gateway 直接进入 Goal 语义。
2. **Apply Patch Diff**：用结构化 `apply_patch` 取代旧的 search-and-replace `patch` 工具，保持与 Codex `apply_patch` 一致的语法与错误处理。
3. **Unified Exec Terminal**：一套 `exec_command` + `write_stdin` 协议同时覆盖普通子进程、PTY、后台长任务、Ctrl-C，取消历史上并存的 `shell_pty` 与 sentinel-based shell fallback。

这三项能力 + 显式 `/goal` 入口共同构成本次对齐范围。文档遵循 Bifrost 标准设计模板。

## 用户目标验证清单

### 必须实现

- `create_goal` / `get_goal` / `update_goal` 三工具在 `crates/agent/src/tools/goal.rs`，通过 `execute_goal_tool` 分派；运行时状态挂在 `AgentSession.current_goal: Option<GoalState>`。
- 目标快照 `ThreadGoal` 使用 `threadId` 作为线程标识，不暴露内部 `goalId`；`GoalStatus` 使用 camelCase 序列化（`active`、`paused`、`complete`、`budgetLimited`）。
- `/goal` slash command 在 `crates/agent/src/slash.rs` + `crates/agent/src/session/turn_loop.rs` 支持 `show` / `set <objective>` / `set --budget <N> <objective>` / `pause` / `resume` / `complete`。
- IM Gateway 无需新增协议，`crates/bifrost-admin/src/handlers/im_gateway/agent_chat.rs` 只需把 `/goal` 补进忙碌态命令识别，`handlers/im_gateway.rs` 模块入口 re-export `busy_message_mode`。
- `apply_patch` 结构化补丁工具位于 `crates/agent/src/tools/apply_patch_diff/`，接受 JSON `input` 字段与裸 freeform patch 文本；旧 `patch.rs` 已移除。
- `apply_patch` 支持 `*** Move To:` / `*** Move to:`、空 `@@` hunk、`*** End of File` 尾标记；拒绝绝对路径；只允许相对路径。
- `exec_command` + `write_stdin` 位于 `crates/agent/src/tools/exec_command.rs`，输出 schema 统一 `session_id` / `exit_code` / `output`。
- 长任务在初始 `yield_time_ms` 后返回 `session_id`；`ExecSessionManager` 后台 watcher 监听真实进程退出；后续 `write_stdin` 可拿到最终 `exit_code` 与尾部输出。
- `write_stdin` 是会话型有序工具，必须按模型输出顺序执行，不进入 local parallel tool batch。
- `enter_worktree` / `exit_worktree` 成功后立即更新当前 turn loop 的 `work_dir`；失败不污染 session work_dir；不重复消费同一信号。

### 必须不破坏

- 其他 tool（`read_file`、`write_file`、`shell_run`、Skill 调用、Chat/UI slash）继续工作。
- `/goal` 在无 IM Gateway 部署下也能通过纯 CLI 会话触发；不依赖任何远端服务。
- `run_turn()` 主循环、slash router、tool 分派入口保持稳定；不引入独立 GoalManager 或 exec fallback 模块。

### 必须真实验证

- `crates/agent/tests/p1_tools_e2e.rs` 覆盖 goal 生命周期、`apply_patch` 多文件补丁、PTY 会话复用 + 交互输入。
- `crates/agent/tests/session_skills_integration.rs` 覆盖 `/goal set --budget ...` 走 session slash router 的黑盒。
- `human_tests/agent-p1-tools.md` 逐条真实执行，覆盖 IM/会话 `/goal`、`apply_patch`、PTY 交互链路、shell prompt 前缀不影响 `exit_code` 解析、worktree 切换即时生效。

## 产品语义

### Goal 系统

- `ThreadGoal { threadId, objective, status, budgetLimitTokens?, createdAt, updatedAt }`——严格镜像 Codex protocol `ThreadGoal`。
- `GoalStatus`（camelCase）：`active`、`paused`、`complete`、`budgetLimited`。
- `GoalRuntimeEvent`（镜像 Codex）：`ThreadResumed`、`MaybeContinueIfIdle`、`TurnStarted`、`ToolCompleted`、`ToolCompletedGoal`、`ExternalSet`。
- 预算触发：当前累计 token 使用超过 `budgetLimitTokens` 时，`GoalState` 从 `Active` 迁移到 `BudgetLimited`。
- `/goal` 会话入口：
  - `/goal` 或 `/goal show`：显示当前 goal 快照。
  - `/goal set <objective>`：创建/替换当前 goal，默认 status=Active。
  - `/goal set --budget <N> <objective>`：设置预算 token 上限。
  - `/goal pause` / `/goal resume` / `/goal complete`：切换状态。

### Apply Patch

- 工具名固定为 `apply_patch`；同时暴露 JSON schema（`{ input: string }`）与裸 patch 文本兼容。
- 支持完整 Codex freeform patch 语法：`*** Begin Patch` / `*** End Patch` / `*** Add File`、`*** Delete File`、`*** Update File`、`*** Move To/to`、`@@ context`、`+` / `-` / 空格前缀。
- 拒绝绝对路径（如 `/etc/passwd`、`C:\` 前缀），只允许相对当前 work_dir 的路径。
- 空 `@@` hunk（无 context 行）合法；行内空 context 行合法；chunk 之间 blank separator 合法。

### Unified Exec

- `exec_command`：真实子进程；支持 `tty=true` 走 PTY；`yield_time_ms` 默认 10000 ms；未完成时返回 `session_id`，`output` 是初始截断输出。
- `write_stdin`：写入或轮询指定 `session_id`；非空 input 默认 250 ms yield；空 input 默认 poll 至少 5000 ms（受 `background_terminal_max_timeout` 上限约束）；输出 `output` + `exit_code`（未退出返回 `null`）。
- 输出限流：单次响应 output 有最大字节数（`MAX_YIELD_TIME_MS` clamp、截断标记）；`clamp_yield_time` 上下限固化在 `crates/agent/src/tools/exec_command.rs`。
- Ctrl-C 通过 `write_stdin` 发送特殊 sentinel（`\x03`）或专用参数中止 session。

### Worktree

- `enter_worktree`：切换到指定 worktree；工具成功后立刻更新 turn loop `work_dir`。
- `exit_worktree`：恢复原目录；同样立刻生效；重复调用不会把 worktree 本身当 original dir。
- 工具失败：不修改 session `work_dir`；后续 tool 仍在原目录执行。

## 技术细节

### 关键源码位置

- Goal：`crates/agent/src/tools/goal.rs`（`ThreadGoal`, `GoalStatus`, `GoalState`, `GoalRuntimeEvent`, `goal_runtime_apply`, `CREATE_GOAL_TOOL_NAME` / `UPDATE_GOAL_TOOL_NAME`）。
- Slash：`crates/agent/src/slash.rs`（`/goal` 分派），`crates/agent/src/session/turn_loop.rs`（`run_turn` 集成）。
- IM Gateway：`crates/bifrost-admin/src/handlers/im_gateway/agent_chat.rs`（`busy_message_mode`），`crates/bifrost-admin/src/handlers/im_gateway.rs`（模块入口 re-export），`crates/bifrost-admin/src/im_gateway/agent_worker.rs`。
- Apply Patch：`crates/agent/src/tools/apply_patch_diff/parser.rs`（`parse_patch`, `MOVE_TO_MARKER`, `MOVE_TO_MARKER_ALT`），`crates/agent/src/tools/apply_patch_diff/apply.rs`。
- Exec Command：`crates/agent/src/tools/exec_command.rs`（`ExecSessionManager`, `clamp_yield_time`, `write_stdin_poll_mode`, `max_empty_write_stdin_yield_time_ms`）。
- 工具注册：`crates/agent/src/tools/mod.rs`。

### Goal 序列化契约

```json
{
  "threadId": "th_abc",
  "objective": "帮我完成 checkout flow",
  "status": "active",
  "budgetLimitTokens": 200000,
  "createdAt": 1730000000,
  "updatedAt": 1730000060
}
```

- `status` 允许值：`active` / `paused` / `complete` / `budgetLimited`。
- 不包含 `goalId`。
- 预算触发后 status 迁移到 `budgetLimited`；后续如果显式 resume 恢复 `active`。

### CLI + Web + Admin API

- CLI：`bifrost chat` / `bifrost agent` 会话中直接使用 `/goal` slash。
- Web：Chat UI 会把 `/goal` 输入透传给 turn loop；无需专门 UI 组件。
- Admin API：`POST /api/agent/chat` 等入口不额外新增 endpoint；`/goal` 走 slash router。IM Gateway `POST /api/im_gateway/*` 复用 busy_message_mode 逻辑。

### Sync 边界

- Goal 状态存活于单个 session；不通过 rule sync 或 group sync 传输。
- Apply Patch 只修改本地工作区文件，不触发 sync。
- Exec Command 是本地子进程；如果 CLI 起在远端 Mac（通过 `bifrost remote`），执行发生在远端。

## Phase 1-4

### Phase 1：Goal 三件套 + `/goal`

- 定义 `ThreadGoal` / `GoalStatus` / `GoalState` / `GoalRuntimeEvent`。
- 实现 `goal_runtime_apply` 与预算触发。
- 集成 slash router；IM Gateway busy_message_mode 补 `/goal`。

### Phase 2：Apply Patch 收敛

- 引入 `apply_patch_diff/{parser,apply}.rs`；解析 freeform patch。
- 支持 lowercase `Move to`、空 hunk、bare context marker。
- 移除旧 `patch.rs`，工具注册表统一为 `apply_patch`。

### Phase 3：Unified Exec

- 合并 `exec_command` + `write_stdin`；引入 `ExecSessionManager` 与后台 watcher。
- 匹配 Codex 默认 yield：`exec_command` 10000 ms、非空 `write_stdin` 250 ms、空 input ≥ 5000 ms。
- Worktree 信号同步 turn loop `work_dir`。

### Phase 4：测试与 human_tests

- `p1_tools_e2e.rs` / `session_skills_integration.rs` 补 goal / apply_patch / PTY / worktree 用例。
- `human_tests/agent-p1-tools.md` 覆盖真实链路。

## 测试方案

### 单元测试

- `apply_patch_diff`（`crates/agent/src/tools/apply_patch_diff/parser.rs`）：
  - `test_parse_update_with_lowercase_move_to`（line 517）
  - `test_parse_update_chunk_with_bare_context_marker`（line 536）
  - `test_parse_update_allows_first_chunk_without_context_marker`（line 557）
  - `test_parse_update_allows_empty_context_line_inside_hunk`（line 577）
  - `test_parse_update_skips_blank_separator_after_header`（line 600）
  - `test_parse_update_skips_blank_separator_between_chunks`（line 622）
  - `test_parse_multiple_hunks`（line 646）
  - `test_parse_end_of_file_marker`（line 665）
  - `test_parse_missing_begin_marker`（line 684）
  - `test_parse_missing_end_marker`（line 692）
  - `test_parse_empty_patch`（line 700）
  - `test_apply_rejects_absolute_path`
  - `test_apply_diff_tool_accepts_raw_patch_body`
- `exec_command`（`crates/agent/src/tools/exec_command.rs`）：
  - `exec_command_returns_completed_output`（line 1363）
  - `exec_command_yields_session_and_write_stdin_polls_to_exit`（line 1394）
  - `exec_command_background_watcher_observes_exit_before_next_poll`（line 1555）
  - `exec_command_write_stdin_drives_pipe_process`（line 1605）
  - `exec_command_yield_defaults_and_clamps_match_codex_unified_exec`
  - `test_exec_command_tty_reports_isatty_true`
- `worktree`：
  - `enter_worktree_signal_updates_session_work_dir_immediately`
  - `exit_worktree_signal_restores_original_work_dir`
  - `failed_worktree_signal_keeps_current_work_dir`
- `goal`（`crates/agent/src/tools/goal.rs`）：create/get/update/duplicate/thread-safety 全覆盖，加上 `snapshot`、`budgetLimited` 迁移、`goal_runtime_apply` 各 `GoalRuntimeEvent` 分支。

### E2E 测试

- `crates/agent/tests/p1_tools_e2e.rs`：
  - Goal 生命周期黑盒；断言响应包含 `threadId` 且不暴露内部 `goalId`。
  - `apply_patch` 多文件 patch 黑盒。
  - PTY 会话复用 + 交互输入黑盒。
- `crates/agent/tests/session_skills_integration.rs`：
  - `/goal set --budget ...` 走 session slash router 黑盒；保持 ThreadGoal 输出契约。

### 真实场景测试

- `human_tests/agent-p1-tools.md`：
  - TC-P1-01：`/goal set` / `/goal show` / `/goal pause` / `/goal resume` / `/goal complete` 全流程。
  - TC-P1-02：`/goal set --budget 10000 <obj>`，触发预算迁移到 `budgetLimited`。
  - TC-P1-03：IM Gateway 侧 `/goal` 与忙碌态识别。
  - TC-P1-04：`apply_patch` 多文件 add/delete/update/move；含 lowercase move。
  - TC-P1-05：`apply_patch` 拒绝绝对路径的错误提示。
  - TC-P1-06：`exec_command` PTY 会话复用 + `write_stdin` 交互；shell prompt 前缀不影响 `exit_code` 解析。
  - TC-P1-07：`enter_worktree` 成功后紧跟 `read_file` 落在新目录；`exit_worktree` 恢复原目录；失败结果不污染 session work_dir。

### 校验要求

- `cargo test -p bifrost-agent`
- `cargo test -p bifrost-agent --test p1_tools_e2e --test session_skills_integration`
- `cargo test --workspace --all-features`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo fmt --all -- --check`
- `rust-project-validate`
- 本机无 `make coverage` 时按仓库 no-local-coverage 约定说明豁免。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核用户目标：Goal 三工具 + `/goal` 入口 + IM Gateway 忙碌态识别 + apply_patch 语法 + unified exec + worktree 即时切换。
- 重点 review：是否残留旧 `patch.rs` / `shell_pty` / sentinel fallback；`goalId` 是否泄漏；`write_stdin` 是否被误纳入 parallel batch。
- 复测：goal / apply_patch / exec / worktree 单元 + `p1_tools_e2e`。

### 第 2 轮

- 复核第 1 轮修复。
- 重点 review：`yield_time_ms` 与 Codex 数值一致；`ExecSessionManager` 后台 watcher 无 leak；`enter_worktree` / `exit_worktree` 不重复消费信号。
- 复跑 human_tests；失败路径重跑。

## 风险与决策

- **决策**：Goal 状态挂在 `AgentSession.current_goal`，不引入独立 GoalManager；跨 session 泄漏由 session 生命周期保证。
- **决策**：`apply_patch` 是唯一 patch 入口；保留 freeform 兼容以便模型直接输出 raw patch body。
- **决策**：删除 `shell_pty`，只保留 `exec_command` + `write_stdin` 一套；后续 Ctrl-C、后台监听、超时都在这套内扩展。
- **决策**：`write_stdin` 属于有序状态工具，不进入 local parallel tool batch；避免同 session 顺序错乱。
- **风险**：`enter_worktree` 后立即切换 `work_dir` 依赖 turn loop 现场更新；如果 tool 分派引入延迟必须由测试兜住。
- **风险**：Goal 预算迁移依赖 token 计数；若 token 计数上游改动会影响触发时机，需要 e2e 定期回归。
- **未来增强**：Goal token/time usage 挂载到 session runtime；apply_patch 独立 freeform grammar tool；unified exec 内置更多 signal 支持——都不改变本方案 API。
