# CLI 进程状态检测

## 背景

`bifrost stop`、`bifrost restart`、`bifrost status` 等命令都依赖同一个底层判断：给定一个 PID，daemon 是否仍在运行？当前实现集中在 `crates/bifrost-cli/src/process.rs::is_process_running(pid)`，Unix 上做的是 `nix::sys::signal::kill(pid, None)`。

`kill(pid, 0)` 的语义只能证明「PID 在系统进程表里、且当前用户有权限向它发信号」，不能证明这个 PID **仍在运行**：如果 daemon 已经 `exit()` 但父进程还没 `wait()` 回收，它会残留成 zombie（`Z` 状态），此时 `kill(pid, 0)` 依然返回 `Ok`。

Linux 上已经通过读 `/proc/<pid>/stat` 排除 `Z`（第三个字段）。macOS 等非 Linux Unix 没有 `/proc`；本地 `cargo test --workspace --all-features` 跑到 daemon shutdown 压力用例（`stop_triggers_graceful_shutdown_in_daemon_mode`）时会把已经收到 SIGTERM、`exit()` 但尚未被父 shell 回收的 daemon 误判为「仍在运行」，`stop` 一路等到 `SIGTERM` 宽限窗口超时，然后升级为 `SIGKILL`，测试断言的「优雅退出」被打破。

本方案在非 Linux Unix 增加一次 `ps` 状态查询，识别 zombie 并视为已退出，让 `stop/restart/status` 的判定与实际进程生命周期一致；同时保留保守回退，避免 `ps` 不可用时误删仍在运行的 PID。

## 用户目标验证清单

### 必须实现

- Unix 平台 `is_process_running(pid)` 在 PID 存在但已 zombie 的场景返回 `false`。
- Linux 保留基于 `/proc/<pid>/stat` 的现有 zombie 判定，不做行为回归。
- macOS 及其它非 Linux Unix 通过 `ps -o stat= -p <pid>` 读取进程状态字符串，首字符是 `Z` 时视为 zombie（已退出）。
- `bifrost stop` 在 daemon 已收到 SIGTERM 并 exit 后，能立刻感知退出，跳过后续等待和 `SIGKILL` 升级。
- `bifrost restart` 停旧起新时，判断旧 daemon 已退出的语义与 `stop` 一致。
- `bifrost status` 报告 daemon 状态时不再把 zombie 展示为「运行中」。
- Windows 路径（`GetExitCodeProcess` / `OpenProcess` 分支）不受本次改动影响。
- 如果 `ps` 命令不可用或返回非预期输出，保持既有保守行为：默认认定进程仍在运行（防止误删仍活着的 PID）。

### 必须不破坏

- 现有 `kill(pid, None)` 检查 PID 存在性的调用点不变，只是在 non-Linux Unix 上追加一次 `ps` 检查。
- `stop` 的 SIGTERM → 等待 → SIGKILL 升级路径本身不变，只是「等待」阶段的判定更准。
- Linux 上 `/proc/<pid>/stat` 判 zombie 的现有路径继续走，避免对 Linux CI 产生扰动。
- Windows 上 `is_process_running` 的实现完全不动。
- 权限异常场景（其它用户的 PID、跨 namespace 的 PID）保守判定 running，防止误删。
- `ps` 调用不引入额外网络、不依赖 root。

### 必须真实验证

- 真实跑 `cargo test -p bifrost-cli --test daemon_shutdown stop_triggers_graceful_shutdown_in_daemon_mode -- --nocapture`：不再超时升级到 SIGKILL。
- 真实跑 `cargo test --workspace --all-features`：stop/restart/status 相关用例全绿。
- macOS 真机上真实启动 daemon → `bifrost stop`，观察 stop 在 SIGTERM 后短时间内返回，不出现 SIGKILL 日志。
- macOS 真机上手动构造 zombie（父进程未 wait），`bifrost status` 报告 stopped 而不是 running。

## 产品语义

### 「运行中」= 真正在跑，不含 zombie

Bifrost 对 daemon 生命周期的官方语义只有两种终态：**running**（进程还能响应信号或至少还在被调度）和 **stopped**（进程已经不在了）。zombie 是内核语义上的过渡状态：进程已经 `exit()`，只是 PID 还没被 `wait()` 回收。对 CLI 用户来说，zombie 应当算作 stopped —— 因为它已经不能被 kill、不能响应任何东西、不能占端口。

### stop 不应该等它「真的死透」，只需要等它 exit

历史行为在 macOS 上会等 SIGTERM 宽限窗口超时后强升 SIGKILL，实际上被杀的是 zombie 的父进程，或者根本没有目标（父进程已回收），既噪声大也会破坏「优雅退出」的 UX。修正后 stop 只要观察到 zombie 就返回成功。

### 保守回退：ps 不可用 → 保留 running 判定

如果 `ps` 二进制被裁掉、被替换成不兼容变体、或者返回内容异常（例如空、非预期列），我们保守认为进程仍在运行。理由：误判 zombie 为 running 最坏结果是多等一次 SIGTERM 宽限（性能损失），误判 running 为 zombie 会导致上游代码认为进程已死而跳过 SIGTERM/SIGKILL，甚至进入 restart 分支去起第二个 daemon（正确性问题）。

## 技术细节

### 修改点

`crates/bifrost-cli/src/process.rs`：

```rust
#[cfg(all(unix, not(target_os = "linux")))]
pub fn is_process_running(pid: u32) -> bool {
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    let p = Pid::from_raw(pid as i32);
    if kill(p, None).is_err() {
        return false; // PID 不存在
    }

    // 非 Linux Unix 通过 ps 排除 zombie
    match ps_state(pid) {
        Some(state) if state.starts_with('Z') => false, // zombie == stopped
        Some(_) => true,                                 // 其它状态视为 running
        None => true,                                    // ps 不可用，保守 running
    }
}

fn ps_state(pid: u32) -> Option<String> {
    let output = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8(output.stdout).ok()?;
    let s = s.trim();
    if s.is_empty() { None } else { Some(s.to_string()) }
}
```

Linux 分支（`#[cfg(target_os = "linux")]`）：保留读 `/proc/<pid>/stat` 的现有逻辑。

Windows 分支：保留 `#[cfg(windows)]` 的既有实现。

### stop 等待循环（保持不变，但受益于新判定）

```rust
// crates/bifrost-cli/src/commands/stop.rs（示意）
kill(p, Signal::SIGTERM)?;
let deadline = Instant::now() + GRACEFUL_TIMEOUT;
while Instant::now() < deadline {
    if !is_process_running(pid) {
        return Ok(StopOutcome::GracefulExit);
    }
    sleep(POLL_INTERVAL);
}
let _ = kill(p, Signal::SIGKILL);
```

改动点在于 `is_process_running(pid)` 现在会正确把 zombie 判为 `false`，因此循环能在 SIGTERM 后短时间内退出。

### 相关文件

- `crates/bifrost-cli/src/process.rs`：`is_process_running` 平台分支、新 `ps_state` 辅助函数。
- `crates/bifrost-cli/src/commands/stop.rs`：调用方，行为不变但受益。
- `crates/bifrost-cli/src/commands/restart.rs`：调用方，行为不变但受益。
- `crates/bifrost-cli/src/commands/status.rs`：调用方，行为不变但受益。
- `crates/bifrost-cli/tests/daemon_shutdown.rs`：`stop_triggers_graceful_shutdown_in_daemon_mode` 场景。

## CLI + Web + Admin API

### CLI

无新增命令，无新增参数；`stop / restart / status` 的输出格式不变。

行为差异（用户感知）：

- macOS 上 `bifrost stop` 在 SIGTERM 后返回速度明显更快；不再出现 `SIGKILL fallback` 日志（在优雅退出场景下）。
- `bifrost status` 输出 `Status: stopped` 更贴近内核语义。

### Admin API

无 Admin API 表面变化。`GET /api/system/status` 里的 running 字段（若有）现在会与 CLI 判定一致（不误报 zombie 为 running）。

### Web UI

无 Web UI 改动。若 UI 展示 daemon 状态，逻辑源自 Admin API，因此自动跟随判定修正。

## Sync 边界

无 sync 影响。进程状态是本机运行时概念，不参与配置同步。

## 实现切分

### Phase 1：ps_state 辅助函数 + non-Linux Unix 分支

- 在 `process.rs` 增加 `ps_state`（unix-only 内部 helper）。
- 修改 `#[cfg(all(unix, not(target_os = "linux")))]` 的 `is_process_running` 增加 zombie 判断。
- 保留 Linux 与 Windows 分支不动。

### Phase 2：单元与集成测试

- 复跑 `daemon_shutdown::stop_triggers_graceful_shutdown_in_daemon_mode`。
- 在能构造 zombie 的测试环境下（fork 子进程 exit 且父不 wait）加针对 `is_process_running` 的单元测试；如无法在 CI 里稳定构造 zombie，则通过 mock/宿主 `ps` 输出实现分支覆盖，或仅通过 daemon_shutdown 集成测试兜底。

### Phase 3：workspace 全量回归

- `cargo test --workspace --all-features` 覆盖 stop/restart/status 共享进程状态判断。
- 确认 Linux CI 分支未被误动。

### Phase 4：human_tests + 文档

- 更新 `human_tests/cli-start-stop-status.md` 加入 daemon 优雅停止的回归用例；确认临时数据目录、非 9900 端口、`--no-system-proxy` 下不修改系统代理。
- `human_tests/readme.md` 索引同步。

## 测试方案

### 单元 / 集成测试

- `cargo test -p bifrost-cli --test daemon_shutdown stop_triggers_graceful_shutdown_in_daemon_mode -- --nocapture`：主要目标测试。
- 若可实现的 `is_process_running_returns_false_for_zombie_on_macos`：fork 一个 `exit(0)` 的子进程并故意不 wait，等待其成为 zombie 后调用 `is_process_running`。macOS/BSD 上通常可行；Linux 通过 `/proc` 分支覆盖不走本次新代码。
- `is_process_running_returns_true_when_ps_unavailable`：注入一个假 `PATH` 使 `ps` 找不到，验证保守 running 语义。

### 工作区回归

- `cargo test --workspace --all-features`：覆盖 stop / restart / status 共享判定。
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`

### 真实场景测试（human_tests）

更新 `human_tests/cli-start-stop-status.md`：

- TC-CSS-01：临时 `BIFROST_DATA_DIR`、非 9900 端口、`--no-system-proxy` 启动 daemon（`bifrost -p 18899 start -d`）。
- TC-CSS-02：`bifrost stop`：在 SIGTERM 宽限窗口内退出，不出现 SIGKILL 升级日志。
- TC-CSS-03：`bifrost status`：报告 stopped，无残留 daemon 进程。
- TC-CSS-04：（macOS）手动构造 zombie 父子，`bifrost status` 报告 stopped。
- TC-CSS-05：`bifrost restart`：旧 daemon 退出后新 daemon 启动，`is_process_running` 判定不阻塞。

`human_tests/readme.md` 同步索引；用例编号从既有基础上延续。

### 覆盖率与项目校验

- 本地跑 focused 单测 + daemon_shutdown 集成 + `cargo test --workspace --all-features`。
- rust-project-validate。
- no-local-coverage 约定下不跑 `make coverage`；说明豁免依据。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核 `is_process_running` 的非 Linux Unix 分支：zombie 判定、保守回退、错误处理。
- 复核 `stop` 等待逻辑：不变但语义修正后行为期望更新。
- 运行 daemon shutdown focused test 与 workspace test 抽样。
- 检查 `human_tests/cli-start-stop-status.md` 用例覆盖。

### 第 2 轮

- 复查 diff：`process.rs` 分支纯 non-Linux Unix 添加，未误改 Linux 路径。
- 复查 Windows 分支未动。
- 全量 `cargo test --workspace --all-features` 复跑。
- human_tests 索引和验证命令一致；rust-project-validate 通过或记录豁免。

## 风险与决策

- `ps` 输出差异：不同 Unix 变体 `ps -o stat=` 的输出格式略有差异（macOS 通常返回 `R+`、`S+`、`Z` 等首字符 + 后缀），本方案只判首字符 `Z`，兼容性最强。若发现某些平台首字符不代表 zombie，回退到保守 running 语义（不会误删）。
- 性能：每次 `is_process_running` 调用 fork+exec `ps`。stop 等待循环里高频调用可能带来轻量开销；测试后如果影响明显，可以将轮询间隔从 ms 级放大到 100ms 或引入短期缓存。当前 poll interval 已经不至于扰动 CI。
- Linux 上误引入：本方案严格用 `#[cfg(all(unix, not(target_os = "linux")))]` 隔离；Linux CI 上不会跑到新代码路径。
- `ps` 被裁剪的 Unix 环境（容器、极简镜像）：保守回退为 running，最坏结果是多等一次 SIGTERM 宽限，然后 SIGKILL 升级 —— 与本次改动之前的行为相同，不产生新回归。
- Windows 分支不改：Windows 有 `GetExitCodeProcess` / `OpenProcess` 等更准确的 API，未来若需要类似修正应走独立 issue。
