# CLI start：进程冲突时交互式重启

## 背景

`bifrost start` 是 CLI 用户启动代理服务的主要入口。历史行为是：如果本机已经存在一个正在运行的 Bifrost 进程（由 `runtime.json`/`bifrost.pid` 记录且进程存活），`start` 会直接以错误退出，提示用户先手动 `bifrost stop`。这种“硬失败”模型在两种常见场景下会拖慢用户节奏：

- 用户明明想要“换配置重启一次”，却要多敲一条 `stop` 命令；
- 用户拿到新构建后 `cargo run -- start` 快速验证时，旧进程还挂在后台，报错信息不容易一眼看清是不是端口占用还是自己启动的老进程。

本方案把这个入口改为“交互式重启”：`start` 检测到已有 Bifrost 进程时，主动提示是否停掉旧进程再启动，同时保留 CI/自动化路径下的 `--yes` 旁路。

除了 Bifrost 自身进程冲突，`start` 还需要面对“监听端口被别的进程占用”这种非 Bifrost 占端口的情况。这两条链路都在这个文档里覆盖，因为它们共享相同的“检测 → 提示 → 用户 y/n → 退出或继续”的骨架，也共享 `--yes` 旁路。

## 用户目标验证清单

### 必须实现

- `bifrost start` 检测到本机已有一个存活的 Bifrost 进程时，输出 `Detected an existing Bifrost proxy process (PID: <pid>). Restart? (y/n)`，等待 stdin。
- 用户输入 `y` / `yes`（大小写不敏感）时，先停旧进程再执行本次 start。
- 用户输入 `n` / `no` 或直接回车时，取消本次启动，打印 `Start cancelled.`，以 exit code 0 退出。
- 用户 stdin 直接 EOF（如 `bifrost start < /dev/null`）时视为取消。
- 连续 3 次输入无法解析的字符串时视为取消，避免死循环。
- `bifrost start --yes` 时跳过交互，直接执行重启，用于 CI/自动化。
- PID 文件存在但进程已经不在时，如果不需要保留系统代理 handoff，直接清理陈旧 PID；否则保留 runtime 文件供 restart 使用。
- 重启路径中断时，旧进程状态、system proxy、CLI proxy、pid/runtime 文件都能被 stop 收敛干净，不留半成品状态。
- 监听端口被非 Bifrost 进程占用时，交互终端下提示 `Kill it and continue? (y/n)`；非交互 stdin 且未传 `--yes` 时直接返回明确错误。
- 端口冲突检查在动到系统代理之前完成，禁止先打印 `System proxy: enabled` 再因端口占用退出。

### 必须不破坏

- 无冲突场景下 `bifrost start` 的行为、退出码、输出摘要不变。
- `--daemon`、`--foreground`、`--port`、`--host`、`--socks5-port`、`--unsafe-ssl`、`--skip-cert-check`、`--system-proxy` 等旧参数语义不变。
- `bifrost stop`、`bifrost restart`、`bifrost status` 对 runtime.json / pid 文件的读写协议不变。
- 端口被同一个 Bifrost 进程占用（真正的“已运行”场景）走进程冲突分支，不会被端口占用分支误判成外部占用。
- 非 Windows 平台的进程检测语义不变，Windows 上补齐 `is_process_running` 后能识别新老 PID。
- 前台启动失败时的日志级别、退出码保持既有约定，交互提示走 stderr，不污染 `--json` 类结构化输出。

### 必须真实验证

- CLI 手动跑：启动一份 `bifrost start`，再在另一个终端跑一次 `bifrost start`，看到交互提示，`y` 能重启、`n` 能取消、EOF 能取消。
- CI 场景真实跑：`bifrost start --yes` 与占用端口的 dummy listener 组合，验证端口冲突下不进入系统代理启用阶段、退出码非零、错误信息包含 `already in use`。
- macOS + Linux + Windows 三平台都跑一遍进程冲突分支，确认 PID 检测语义一致。
- `bifrost start < /dev/null`（无 tty stdin）时，进程冲突和端口冲突都能给出明确错误或取消提示，不会 hang。

## 产品语义

### 交互提示的三层信号

`start` 检测到 Bifrost 自身进程冲突时进入交互分支：

```text
Detected an existing Bifrost proxy process (PID: 41321). Restart? (y/n)
> y
Stopping existing Bifrost proxy (PID: 41321)...
...
Starting Bifrost proxy at http://127.0.0.1:8888
```

- `y` / `yes`：走 `commands::stop::run_stop()` 完整收尾流程，再进入本次 start 的常规路径。
- `n` / `no` / 空回车：以 exit code 0 退出，打印 `Start cancelled.`；用户没有启动新进程，也没有停旧进程，是最保守的默认。
- 无法识别的输入连续 3 次：视为取消，避免脚本环境下的死循环。
- stdin EOF：视为取消，避免非交互场景下阻塞。

`--yes` 直通所有分支：只要检测到冲突，就默认答 `y`。这是 CI / 脚本 / systemd unit 里最常用的形态。

### 端口冲突与进程冲突的分工

Bifrost 自身进程 vs 外部进程占端口，是两条不同分支：

- Bifrost 进程冲突：靠 `runtime.json` + `is_process_running(pid)` 判定，进入“Restart?”提示；确认后 `stop` 收尾，之后继续常规 start。
- 外部占端口：靠 `check_and_resolve_port_conflict(&host, port, yes)`。它先用绑定探测（把地址归一到 `127.0.0.1` 试着 bind），只有当返回错误不是 `AddrInUse` 时才降级为一次 200ms TCP connect 探测。这一层的目的是：避免 TCP connect 探测被 backlog、未 accept 的测试监听器、防火墙 SYN 反射等异常状态误判为“端口空闲”。
- 端口占用且交互 tty：提示 `Kill it and continue? (y/n)`，用户确认后调用 `find_process_on_port` 找到占用方并 kill，等一小段时间复检端口。
- 端口占用且非交互 stdin：立即返回错误，消息形如 `Port <host>:<port> is already in use and no interactive terminal is available. Use --yes to auto-resolve.`。这是让 CI 显式失败、而不是静默尝试 kill 的关键。

两个分支的先后顺序是刚性的：**先解决 Bifrost 进程冲突，再解决端口冲突，最后才 `check_and_install_certificate` 和 system proxy 收敛**。这保证任何一条“不 OK”都发生在动到系统代理之前，用户环境不会被半途改动。

### `--yes` 的语义边界

`--yes` 只自动回答“重启已有 Bifrost 进程”和“kill 外部占端口进程”这两个问题；它不改变：

- 证书安装是否需要 sudo；
- 系统代理是否被启用；
- 是否覆盖 `runtime.json`（无冲突时本来就会写）。

也就是说 `--yes` 是“承接交互提示”，不是“绕过一切检查”。

## 技术细节

### 关键函数

- `crates/bifrost-cli/src/commands/start.rs::run_start`：入口。最前置阶段读 `read_pid()` 并调用 `is_process_running(pid)`。
- `crates/bifrost-cli/src/commands/start.rs::prompt_restart_if_running(pid)`（`start.rs:211`）：负责打印提示、读 stdin、判定 `y`/`n`/EOF/3 次失败。返回 `Ok(true)` 表示确认重启，`Ok(false)` 表示取消。
- `crates/bifrost-cli/src/commands/start.rs::check_and_resolve_port_conflict(&host, port, yes)`（`start.rs:237`）：端口占用检测 + 可选 kill。
- `crates/bifrost-cli/src/commands/stop.rs::run_stop()`：被重启分支复用，负责 SIGTERM/TerminateProcess、等退出、必要时强杀、恢复/关闭 system proxy、清理 CLI proxy、删 pid/runtime。
- `crates/bifrost-core/src/process.rs::is_process_running`：跨平台进程存活检测；Windows 侧走 `OpenProcess` + `GetExitCodeProcess`。

### 时序

```
bifrost start
  ├─ read_pid() / runtime.json
  ├─ if pid.is_some() && is_process_running(pid):
  │    ├─ if --yes -> confirm=true
  │    ├─ else -> confirm = prompt_restart_if_running(pid)?
  │    ├─ if !confirm -> println!("Start cancelled."); return Ok(())
  │    └─ run_stop()  // 复用 stop 完整收尾
  ├─ else if pid file stale:
  │    ├─ if system_proxy_shutdown_mode == PreserveForRestart -> keep runtime.json
  │    └─ else -> remove_pid()
  ├─ check_and_install_certificate() // 按需
  ├─ check_and_resolve_port_conflict(&host, port, yes)?
  ├─ init_config / init_data_dir / init_runtime
  ├─ print startup summary
  └─ start proxy listener + system proxy reconcile (async)
```

### 端口探测的两步策略

`is_port_in_use(host, port)` 的策略：

1. 归一化：`0.0.0.0` 和 `::` 都归一到 `127.0.0.1` 试 bind（这是端口是否真的被别人占用最权威的信号）。
2. 主路径：`TcpListener::bind` 到目标地址；`AddrInUse` 直接判定为占用。
3. 回退路径：仅当 bind 返回 `AddrInUse` 之外的错误（如权限受限、DNS 无法解析）时，降级为 200ms 超时的 TCP connect 探测。
4. 复检：kill 后等待 200ms 再复检一次；仍占用则错误退出。

这样做的目的是避免只用 TCP connect 时被 backlog、未 accept 的测试监听器或防火墙误判。

### `--yes` 传递路径

`--yes` 在 CLI 层解析进 `StartArgs`，一路透传给 `prompt_restart_if_running` 和 `check_and_resolve_port_conflict`；不通过环境变量透传，避免 daemon 派生子进程时误继承。

## CLI / Web / Admin API 呈现

### CLI

- 新增行为收敛在既有 `bifrost start` 命令，无新增子命令，无新增 flag（`--yes` 是 start 已有 flag）。
- 交互提示输出到 stderr，方便 `bifrost start > log.txt` 场景下仍能看到提示。

### Web / Admin API

- 本方案不改 Admin API 和 Web UI。管理端“Restart proxy”按钮走 `POST /_bifrost/api/system/restart`，是另一条链路，不复用该 CLI 交互。

## Sync 边界

- 该方案影响单机 CLI 启动流程，不涉及跨设备同步。
- `runtime.json` 是本机运行态元数据，不参与 sync。
- 端口冲突/进程冲突判定基于本机 `bifrost.pid` 和 `is_process_running`，不查询 relay。

## 实现切分

### Phase 1：基础交互

- 抽出 `prompt_restart_if_running`，把原“检测到已运行进程 → 报错退出”改成“调用提示”。
- 补 `--yes` 旁路。
- 单元测试覆盖 y/n/EOF/无效输入 3 次的四条分支。

### Phase 2：端口冲突分支

- 抽出 `check_and_resolve_port_conflict`，实现两步探测（bind 优先 + TCP connect 回退）。
- 交互 tty 提示 kill；非交互 stdin 直接错误。
- 保证在系统代理动作之前调用。

### Phase 3：跨平台补齐

- Windows：补齐 `is_process_running` 的 `OpenProcess` + `GetExitCodeProcess` 实现。
- macOS/Linux：验证 `kill(pid, 0)` 分支的错误码归一。

### Phase 4：E2E 与真实场景回归

- 新增 `e2e-tests/tests/test_cli_start_interactive_restart_e2e.sh`：覆盖 y/n 两条分支。
- 落地 `e2e-tests/tests/test_port_conflict_no_system_proxy_enable.sh`：验证端口冲突时非零退出、包含 `already in use`、不包含 `System proxy: enabled`。
- 更新 `human_tests/cli-start-stop-status.md` 手工用例。

## 测试方案

### 单元测试

- `crates/bifrost-cli/src/commands/start.rs`：
  - `check_and_resolve_port_conflict_returns_ok_when_port_free`（`start.rs:5140` 已落地）。
  - 新增：`prompt_restart_if_running` 在 `y`/`Y`/`yes`/`YES` 下返回 `Ok(true)`；`n`/`no`/空行/EOF 下返回 `Ok(false)`。
  - 新增：非法输入连续 3 次后返回 `Ok(false)`。
  - 新增：`--yes` 直通不调用 `stdin` 读取。

### E2E 测试

- `e2e-tests/tests/test_cli_start_interactive_restart_e2e.sh`（planned）：
  - 场景 1：起一份 daemon Bifrost；再跑 `printf 'y\n' | bifrost start`，断言第二个进程接管，第一个 PID 消失。
  - 场景 2：`printf 'n\n' | bifrost start`，断言 exit code 0、输出 `Start cancelled.`、旧 PID 仍然存活。
  - 场景 3：`bifrost start < /dev/null`，断言 exit code 0 且 `Start cancelled.`。
- `e2e-tests/tests/test_port_conflict_no_system_proxy_enable.sh`（已落地）：
  - 用一个 dummy listener 占用测试端口。
  - `bifrost start --system-proxy` 必须非零退出。
  - stdout/stderr 包含 `already in use`；不包含 `System proxy: enabled` 或 `System proxy enabled:`。

### 真实场景测试

- `human_tests/cli-start-stop-status.md`：
  - TC-CSS-26：进程冲突时 `y` 重启成功。
  - TC-CSS-27：进程冲突时 `n` 取消 + 旧进程仍存活。
  - 新增用例：端口被非 Bifrost 占用 + 非交互 stdin，观察是否明确失败且未启用系统代理。

## Review / Fix / Test 闭环

- 提交前跑：`cargo fmt --all -- --check`、`cargo clippy --all-targets --all-features -- -D warnings`、`cargo test --workspace --all-features`。
- E2E：`bash e2e-tests/tests/test_cli_start_interactive_restart_e2e.sh`、`bash e2e-tests/tests/test_port_conflict_no_system_proxy_enable.sh`。
- 手动 human_tests：TC-CSS-26 / TC-CSS-27。

## 风险与决策记录

- **决策 1**：非交互 stdin 默认取消，不默认重启。防止 CI 里静默把用户 PID 干掉。反例是要求用户显式 `--yes`。
- **决策 2**：端口占用即使有 `--yes` 也复检一次 200ms。防止 kill 后系统还没释放端口就直接启动导致 bind 失败。
- **决策 3**：不为该交互引入新的环境变量（例如 `BIFROST_START_NON_INTERACTIVE`），全部走 `--yes`。理由是环境变量在 daemon 子进程会被继承，容易出现“父终端不 --yes、daemon 子进程 --yes”的差异。
- **风险**：Windows 平台若 `is_process_running` 返回过时状态（进程刚退但句柄未收敛），可能出现“看到 PID 但 stop 无对象”。缓解：`run_stop` 内部对 “process gone” 走 no-op 分支，只清 pid 文件。
- **风险**：`printf 'y\n' | bifrost start` 与 daemon 模式组合时，daemon 会失去 tty；这被视为正常，交互提示只面向前台模式。daemon 模式下检测到冲突且未传 `--yes` 时，直接错误退出。

## 文档更新

- `docs/cli.md` 的 `start` 章节补一段：“检测到已有 Bifrost 进程时会提示是否重启，`--yes` 可跳过。”
- `human_tests/readme.md` 索引里加 TC-CSS-26 / TC-CSS-27 的入口。
