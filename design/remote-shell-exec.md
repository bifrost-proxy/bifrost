# Remote Shell Exec 设计方案

> 状态：已实现（持续加固） | 更新时间：2026-07-03

## 背景

Bifrost Remote Invoke 除了远程只读诊断（status / traffic / search），还需要在 target 上执行 shell 命令用于运维、修数据、跑 codex 等场景。本设计把 `bifrost remote exec` 作为 caller 侧统一入口，把安全边界、策略选择、策略绑定、审计和 stdin/PTY 交互能力全部沉淀到 target 本地：

- caller 只表达“要执行什么命令”，不能指定 policy。
- relay 只做最小 `grant_scope` 路由与端到端加密转发，不存储策略细节。
- target 本地 `remote_shell.json` 独立保存 policy set 与快照版本，并把策略选择、拒绝、命中日志放在 target。

本轮设计沉淀了 2026-04 至 2026-06 五次迭代（stdin 早到帧修复、stdin stream 并发预算、Windows PTY 兼容、`--login` UX、`--detach` 长任务）中形成的稳定契约。

## 用户目标验证清单

### 必须实现

- caller 通过 `bifrost remote exec [--] <argv...>` 或 `--shell-text` 发起远端命令。
- caller CLI 不再暴露 `--policy` / `policy_id`，避免用户绕过 target 策略选择。
- target 本地 `remote_shell.json` 独立存储 profile / policy set，`bifrost setting shell` 直接编辑。
- grant scope 严格区分 `remote_query`（只读）与 `shell.exec`（可执行），只读 grant 拒绝执行 shell。
- target 执行前校验 executable allowlist、shell_text regex allowlist、cwd allowlist、env key allowlist、stdin / pty 开关、timeout 上限、输出截断。
- grant 与 `shell_policy_set_version_snapshot` 绑定；策略版本变更后旧 grant 自动失效。
- caller 侧 Recent Calls 记录 target 最终命中的 `policy_id` / `exec_mode`；命令内容通过 E2E 加密传输。
- `--interactive` / `--pty` 支持 stdin 流式输入、Ctrl-C 转发、window resize、真实分配 Unix PTY 或 Windows ConPTY。
- `--detach` 支持长任务：open_call 后立即返回 `call_id` / `relay_token`，后续通过 `bifrost remote job status|logs|watch` 复用 stream resume 通道恢复。
- shell 默认走非 login 路径（`-c`），`--login`（`-lc`）为 opt-in；始终注入 `BIFROST_REMOTE=1`、`TERM=dumb`、`ITERM_ENABLE_SHELL_INTEGRATION_WITH_TMUX=NO` 降噪，`--login` 时在 shell 内前置 `cd -- <cwd>`。
- 覆盖 macOS / Linux / Windows 三平台。

### 必须不破坏

- 只读 `remote status/traffic/search` 链路不受 shell 改动影响。
- 只读 grant 仍能创建、复用、导入导出，不被 shell 策略字段污染。
- relay 仅保留 `client_instance_id + grant_scope` 用于路由，不能持有策略细节。
- Recent Calls / call meta 只保留路由级字段，业务字段（exit_code、digest、bytes）全部由 target 本地审计承载。

### 必须真实验证

- 真实 remote 链路执行 `remote exec -- /bin/pwd` 命中 target 本地 `pwd-argv` 策略。
- 只读 grant 场景真机验证 `shell.exec` 被拒并给出升级建议。
- 真实 stdin / PTY 场景验证 first stdin frame 不丢失、Ctrl-C 生效、退出后恢复本地 raw mode。
- 长任务 `--detach` + `remote job watch` 真机验证跨 caller 重连仍能拿到最终 exit code。

## 产品语义

### caller 与 target 分工

| 角色 | 职责 |
| --- | --- |
| caller | 只表达“执行什么命令”；显示 target 返回的策略命中或拒绝原因；本地做 UX（Ctrl-C、raw mode、`--detach` 状态持久化）。 |
| relay | 校验 `grant_scope=shell.exec`；转发密文帧；维护最小 `grant_scope` 覆盖策略。 |
| target | 选择 policy、执行 allowlist / regex 校验、执行命令、封装 stdout/stderr、写本地审计、返回 exit。 |

### `bifrost remote exec` 参数模型

- `argv_exec` 模式：`bifrost remote exec -- <program> <args...>`。必须显式 `--` 进入，避免裸参数被误解成 CLI 子命令。
- `shell_text` 模式：`bifrost remote exec --shell-text "printf x; /bin/pwd"`。target 会用 shell 解释器执行。
- 通用参数：`--cwd`、`--env KEY=VAL`（多次）、`--timeout-ms`、`--stdin file|-`、`--interactive`、`--pty`、`--login`、`--detach`、`--no-verify-digest`。
- 不允许 `--policy` / `policy_id`；target 侧根据 `mode`(single/all) 与命令内容自动选。

### grant 与 policy set 版本绑定

grant 创建时快照 target 当前 `shell_policy_set_version`。target 每次执行前比对；不一致则返回：

```
shell policy set version changed on target (was v3, now v4); reconnect is required
```

CLI 在 caller 侧展示为 `run 'bifrost remote connect ...' to refresh grant`。

### 输出与审计

- target 逐帧封装 stdout/stderr 为 `EncryptedEnvelope v2`（ChaCha20-Poly1305），caller 解密后直接写终端。
- target 本地写审计：`policy_id`、`exec_mode`、命令摘要、start/end、exit_code、bytes_in/out、stdout/stderr digest。
- relay call 记录仅保留 `status/started_at/ended_at`，不存业务字段。

## 技术细节

### 数据结构

`crates/bifrost-storage/src/remote_shell.rs`（418 行）：

```rust
pub struct RemoteShellStore {
    pub profiles: Vec<RemoteShellProfile>,
    pub policy_sets: Vec<RemoteShellPolicySet>,
    pub active_policy_set_id: String,
    pub version: u64,
}

pub struct RemoteShellPolicy {
    pub id: String,
    pub mode: PolicyMode,          // Single | All
    pub executable_allowlist: Vec<String>,
    pub shell_text_regex_allowlist: Vec<String>,
    pub cwd_allowlist: Vec<String>,
    pub env_key_allowlist: Vec<String>,
    pub stdin_allowed: bool,
    pub interactive_allowed: bool,
    pub max_timeout_ms: u64,
    pub max_output_bytes: u64,
    pub exec_mode: ExecMode,       // Argv | ShellText | Both
}
```

grant 记录：`shell_policy_set_version_snapshot: Option<u64>`、`grant_scope: RemoteGrantScope { readonly, shell_exec, file_read, ... }`。

### CLI 命令表

| 命令 | 说明 |
| --- | --- |
| `bifrost remote exec [--] argv...` | argv_exec 模式 |
| `bifrost remote exec --shell-text "..."` | shell_text 模式 |
| `bifrost remote exec --detach ...` | 长任务，立即返回 `call_id` |
| `bifrost remote job list` | 列出本机 caller 记录的长任务 |
| `bifrost remote job status <id> [--wait-ms N]` | 短 SSE 订阅 running/exited |
| `bifrost remote job logs <id>` | resume stream 拉历史输出 |
| `bifrost remote job watch <id>` | resume stream 并持续输出，返回真实 exit code |
| `bifrost setting shell profile add/update/remove/list/show` | 编辑 target 本地 profile |
| `bifrost setting shell policy add/update/remove/list/show` | 编辑 target 本地 policy set |
| `bifrost setting grant grant/revoke/edit/list` | 编辑本地 grant overlay，含 `--allow-shell-exec` |

CLI 定义位于 `crates/bifrost-cli/src/cli/remote.rs`：
`RemoteCommands::Exec(Box<RemoteCommandExecArgs>)`（line 31）、`RemoteShellCommands`（line 602）、`RemoteShellProfileCommands`（line 676）、`RemoteShellPolicyCommands`（line 711）、`RemoteJobCommands`（line 972）。执行入口 `crates/bifrost-cli/src/commands/remote_shell.rs::handle_remote_shell_command`（line 13）、`crates/bifrost-cli/src/commands/remote.rs::handle_remote_job_command`（line 3846）。

### Web UI

Settings → Remote Invoke → Shell Access：`web/src/pages/Settings/tabs/RemoteInvokeTab.tsx`。功能：

- 列出本机 profile / policy set / active policy set。
- 新增 / 编辑 / 删除 policy（包含 allowlist、cwd、env、stdin、pty、timeout、max output）。
- Recent Calls tab 展示最近命令、`policy_id`、`exec_mode`、exit_code、耗时。
- Grants tab `Edit Access` 只操作本地 grant overlay，relay 仅同步 `grant_scope`。

### Admin API

`crates/bifrost-admin/src/remote_invoke/`：

- `POST /api/remote/exec/open` – target 收到 open_call，做 grant scope + policy 选择 + allowlist 校验。
- `POST /api/remote/exec/frame` – stdin / resize / signal 帧入口。
- `GET /api/remote/exec/logs?id=&cursor=` – resume stream，供 `remote job logs/watch`。
- `GET /api/remote/exec/status?id=&wait_ms=` – 短 SSE 订阅。
- `PUT /api/remote/shell/policies/:id` / `POST /api/remote/shell/policies` – 本地编辑 policy set，改动会推进 `shell_policy_set_version`。
- `PATCH /v4/remote-invoke/client/grants/:id`（relay 侧）：只接受 `client_instance_id` + `grant_scope`。

### Sync 边界

- `remote_shell.json` 与 grant overlay **不参与 sync**：每台设备的策略是本地事实。
- relay 只在授权时校验 `grant_scope`，不保存 policy 内容；重新 connect / approve 会覆盖旧 grant，`disconnect --all` 会清空该 caller 在 target 的全部 reusable grants。
- `bifrost remote job` 记录写入本机 `~/.bifrost/remote_jobs.json`，仅供本 caller 使用，不同步。

## Phase 1-4 拆分

### Phase 1：策略与 grant 隔离（已完成）

- 独立 `remote_shell.json`。
- grant scope 拆分 `remote_query` / `shell.exec`。
- `bifrost setting shell` / `bifrost setting grant` 编辑本地策略与 grant overlay。
- relay 拆分只读与执行 scope，最小 scope 校验。

### Phase 2：安全边界与 E2E 加密（已完成）

- allowlist / regex / cwd / env / stdin / pty / timeout / max output 全量校验。
- `EncryptedEnvelope v2` 强制 ChaCha20-Poly1305 + X25519 ECDH + HKDF per-call 会话密钥。
- Recent Calls / call meta 只保留路由级字段。
- 策略版本变化让旧 grant 失效。

### Phase 3：stdin / PTY 交互能力（已完成）

- Unix PTY / Windows ConPTY 真实分配。
- stdin 早到帧修复：`active_calls.insert()` 前预建 mpsc stdin channel（2026-05-10）。
- `--interactive` / `--pty` / resize / Ctrl-C 转发。
- 独立 wall-clock timeout 单测与 stdin stream 单测分离（2026-06-04）。

### Phase 4：长任务与 UX 加固（已完成）

- `--detach` + `remote job status|logs|watch` 通道恢复。
- wall-clock timeout 错误显式携带 `requested/policy_cap/capped_by_policy`。
- 默认非 login shell，`--login` opt-in；`BIFROST_REMOTE=1` 等降噪 env 无条件注入。
- streaming digest mismatch 给出 `--detach` + `--no-verify-digest` 恢复建议。

## 测试方案

### 单元测试

- `cargo test -p bifrost-admin remote_invoke::executor::tests::test_execute_shell_exec_forwards_stdin_stream`
- `cargo test -p bifrost-admin remote_invoke::executor::tests::test_execute_shell_exec_wall_clock_timeout_still_enforced`
- `cargo test -p bifrost-admin remote_invoke::executor::tests::active_call_accepts_stdin_before_executor_start`
- `cargo test -p bifrost-admin remote_invoke::executor::tests::command_accepts_stdin_for_stdin_mode_or_pty`
- `cargo test -p bifrost-admin remote_invoke::executor::tests::timeout_policy_note_reports_policy_cap`
- `cargo test -p bifrost-admin remote_invoke::executor::tests::shell_text_cwd_prefix_quotes_and_forces_cd_inside_shell`
- `cargo test -p bifrost-cli remote::tests::test_build_remote_command_shell_exec_login_flag_reaches_payload`
- `cargo test -p bifrost-cli remote::tests::remote_shell_policy_update_parses_all_flags`
- `cargo test -p bifrost-cli remote::tests::remote_shell_policy_update_minimal_args`
- `cargo test -p bifrost-cli remote::tests::update_remote_job_status_persists_exit_code`

### E2E 测试

- `crates/bifrost-e2e/src/tests/remote_shell_exec.rs`（469 行）：策略绑定、拒绝、版本变化、多 caller 隔离。
- `e2e-tests/tests/test_remote_shell_exec_streaming_e2e.sh`（656 行）：真实流式 stdout、Ctrl-C、Windows shell_text UTF-8 fallback。
- `e2e-tests/tests/test_remote_invoke_ssh_e2e.sh`：saved SSH connection 执行 shell 回归。
- `e2e-tests/tests/test_remote_job_real_e2e.sh`：`--detach` + `remote job watch` 长任务闭环。
- `e2e-tests/tests/test_remote_job_cache_cli.sh`：`remote job list/status/logs` 本地缓存与恢复。

### 真实场景测试

`human_tests/remote-shell-exec.md`（445 行）已覆盖 TC-RSE-01 ~ TC-RSE-22：

| 用例 | 场景 |
| --- | --- |
| TC-RSE-01 | caller CLI 不再暴露 `--policy` |
| TC-RSE-02 | read-only grant 拒绝 `shell.exec` |
| TC-RSE-03 | selected policy grant 命中唯一 argv 策略 |
| TC-RSE-04 | 未命中 allowlist 命令被拒绝 |
| TC-RSE-05 | caller 伪造 `policy_id` 被拒 |
| TC-RSE-06 | `mode=all` 命中多条策略歧义拒绝 |
| TC-RSE-07 | Full Access shell_text 可执行、Default Sandbox 拒绝 |
| TC-RSE-08 | 策略版本变化后旧 grant 失效 |
| TC-RSE-09 | 删除 caller A grant 不影响 caller B |
| TC-RSE-10 | 编辑 grant 策略只落 target 本地 |
| TC-RSE-11 | 重新 connect 覆盖同 caller/device 旧 grant，disconnect 清残留 |
| TC-RSE-12 | 旧 Full Access 配置兼容 argv |
| TC-RSE-13 | argv_exec 必须显式 `--` |
| TC-RSE-14 | 长任务 stdout 流式返回 |
| TC-RSE-15 | 流式回归沉淀到 shell E2E |
| TC-RSE-16 | Windows 流式 shell E2E |
| TC-RSE-17 | Windows shell_text Unix 路径 fallback / UTF-8 |
| TC-RSE-18 | `policy update` 不破坏 grant |
| TC-RSE-19 | Remote Invoke stdin frame 转发到 executor active session |
| TC-RSE-20 | 真链路 `--interactive` stdin 转发 |
| TC-RSE-21 | `--pty` 真实 PTY 且退出恢复 raw mode |
| TC-RSE-22 | 首个 stdin frame 不丢失（早到帧回归） |

新增待跟踪回归：

- TC-RSE-23：`--detach` 长任务在 caller 断开后 `remote job watch` 仍能收敛到真实 exit code。
- TC-RSE-24：`--login` opt-in 与默认非 login 路径 rc 注入差异对比。

## Review/Fix/Test 闭环

### 第 1 轮

- 目标复核：确认 caller / relay / target 分工与 grant scope 隔离。
- 代码 review：`remote_invoke/executor.rs` 是否所有拒绝路径都返回可读错误；relay 是否只接受最小 grant 字段。
- 复测：单元 + 上述 shell E2E + human_tests TC-RSE-01/03/07/08/22。

### 第 2 轮

- 复核 stdin / PTY / `--detach` 修复的边界条件。
- 检查 `git status --short`、`git diff` 是否有 relay 侧误加策略字段。
- 复测：Windows shell E2E、`remote job watch` 长任务恢复。

## 风险与决策

- **策略集中在 target**：多 caller 共同使用 target 时，某个管理员改动 policy 会立即让所有 caller 的旧 grant 失效——保留此约束换取“target 是策略事实源”。
- **E2E 加密强制 v2**：一刀切升级，旧客户端会收到协议版本不匹配错误；不再兼容 v1 明文伪装。
- **`--detach` 记录只在本 caller 本地**：跨设备恢复长任务需要携带原 `call_id + relay_token`，未来若要跨设备恢复，需要单独设计跨设备 job manifest 同步。
- **relay 最小 scope**：为了防止只读 grant 打开 `shell.exec`，relay 保留最小 `grant_scope`；未来 scope 扩展（如 `file.write`）需要同时更新 relay 与 target。
