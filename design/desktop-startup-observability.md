# 桌面端启动期日志与故障可观测性

## 背景

桌面端启动是一条相对复杂的链路：解析 binary 路径 / 数据目录 / 端口配置 → 判断是否已有 backend → 拉起 sidecar → 等 admin ready → 触发 handoff → 前端接管。任何一步失败都可能让用户看到“空白窗口 + 转圈” 或 “直接退出”，而没有可诊断线索。

历史实现的痛点：

- 桌面壳层没有独立日志，所有输出依赖 sidecar 内部 tracing；一旦 tracing 还没初始化就 panic / 打印，用户看不到；
- sidecar 的 stdout / stderr 都被 `Stdio::null()` 吞掉；
- 桌面壳层与 CLI 用了不同的默认数据目录（`bundleId/AppSupport/...` vs `~/.bifrost`），用户找日志找错地方；
- backend bootstrap 失败没有回传到前端，UI 表现为“窗口就是不显示”，没有任何 error toast。

当前实现聚焦三件事：

1. 桌面壳层与 CLI 共用 `~/.bifrost`（或 `BIFROST_DATA_DIR`）作为默认数据目录；
2. 桌面壳层自身写 `desktop-bootstrap.log`；sidecar 的 stdout/stderr 落盘为 `desktop-sidecar.out.log` / `desktop-sidecar.err.log`；
3. Backend bootstrap 失败原因回写到 `state.startup_error`，前端可读，UI 弹错误提示。

本文覆盖启动期日志文件、数据目录统一、失败信号传播、日志保留、sidecar 快速失败和 launcher 故障 handoff。相关：`desktop-launcher-startup.md`、`desktop-macos-close-behavior.md`、`desktop-runtime-port-switch.md`、`desktop-core-watchdog-resource-guard.md`、`desktop-core-cert-bootstrap.md`。

## 用户目标验证清单

### 必须实现

- 桌面壳层与 CLI 使用完全相同的数据目录：
  - `BIFROST_DATA_DIR` 生效时用该目录；
  - 否则默认 `~/.bifrost`；
  - `desktop-config.json` 位于同一目录，而不是 Tauri 私有目录；
  - 数据目录解析复用 `bifrost_storage::data_dir()`，桌面端不额外维护路径。
- 桌面壳层写入 `logs/desktop-bootstrap.log`，记录：
  - 使用的 bifrost 二进制路径；
  - 目标数据目录；
  - 目标端口与实际端口尝试过程；
  - sidecar 启动/停止/重启的原因；
  - 等待 backend ready 的超时；
  - Handoff / launcher overlay 事件；
  - Close / shutdown / recovery 事件。
- 桌面壳层将 sidecar 的 `stdout` / `stderr` 分别追加写入 `logs/desktop-sidecar.out.log` / `logs/desktop-sidecar.err.log`：
  - 覆盖 sidecar tracing 初始化前的 `println!` / `eprintln!` / panic；
  - 覆盖配置解析错误等冷启动异常。
- Backend bootstrap 失败：
  - 通过 `record_startup_error` 写入 `desktop-bootstrap.log`；
  - 同时更新 `BackendState.startup_error: Mutex<Option<String>>`；
  - 首次启动等待同时轮询 child 状态；sidecar 提前退出时立即返回退出状态，不再对后续端口重复 20 秒等待；
  - 主 WebView 已加载后必须执行 failure handoff，移除 native launcher；
  - 前端通过 `get_desktop_runtime` invoke 读到 `startupError` 并展示重试界面。
- 任一未知启动阶段阻塞超过 30 秒时，launcher deadline 必须写入当时的 backend/WebView 状态并强制 handoff；用户可以看到恢复界面，不能永远停在原生 loading 页。
- 停止 stale backend 的同步子进程最多等待 5 秒；超时后杀掉 helper 并把失败原因写入日志，不能让首次启动初始化无限卡住。
- macOS 发布包必须校验桌面主程序与内置 `resources/bin/bifrost` 的 Mach-O 架构都包含发布 target；允许 universal binary，但禁止 Apple Silicon 包夹带 Intel-only sidecar、反之亦然。
- 桌面日志按 `DESKTOP_LOG_RETENTION_DAYS = DEFAULT_LOG_RETENTION_DAYS` 自动清理（复用 `bifrost_core::cleanup_bifrost_log_dir`）。
- 每个 `data_dir` 每进程只做一次清理，避免每次写日志都扫目录。

### 必须不破坏

- Handoff / launcher / cert bootstrap / port switch / watchdog / close behavior 语义。
- CLI 日志目录与命名规范（`~/.bifrost/logs/*`）。
- Admin API 安全模型（loopback-only）。
- `desktop-config.json` 结构 `{ proxy_port: u16 }`。
- sidecar 内部 tracing / 结构化日志继续按原路径写入。

### 必须真实验证

- 正常冷启动后 `~/.bifrost/logs/` 下同时存在 `desktop-bootstrap.log`、`desktop-sidecar.out.log`、`desktop-sidecar.err.log`。
- `desktop-bootstrap.log` 可以按时间顺序读出：启动 → 端口尝试 → sidecar ready → handoff → shutdown 全链路。
- 使用 `BIFROST_DATA_DIR=/tmp/bifrost-foo` 启动，`desktop-config.json` 与 `logs/*` 都进入 `/tmp/bifrost-foo`。
- 构造失败场景（例如占用 9900 与后续 64 个端口）：前端弹窗展示 `startup_error`；`desktop-bootstrap.log` 定位到端口顺延失败或 backend ready 超时。
- 桌面壳层与 CLI 同时运行时不会互相污染日志（因为共用同一 daemon 状态而不是多副本）。

## 产品语义

### 本次首次启动卡死的证据分层

已由代码和截图确认：

- 桌面 sidecar 以 `--host 0.0.0.0` 启动，因此首次创建入站 listener 会进入 macOS Application Firewall 决策链路；这不是 System Proxy 授权窗。
- 当前 release workflow 明确使用 `APPLE_SIGNING_IDENTITY=-`。Application Firewall 会基于代码签名的 designated requirement 做首次与后续识别；在项目暂时没有 Developer ID / 公证凭据的约束下，不能依赖系统自动信任来消除授权窗。
- 历史 handoff 只接受 `startup_ready=true`。即使 sidecar 已退出并产生 `startup_error`，原生 launcher 仍覆盖在可重试页面上，于是用户只看到永远停住的 loading。

高度可能但仍需用户机器日志最终确认：

- 授权窗出现后立即消失，说明触发监听的进程或 socket 很可能随即消失、被替换或重启。正常情况下系统授权窗应等待用户选择；截图本身不能区分 sidecar crash、stale-backend stop 卡住、端口/配置错误或系统终止。
- “首次初始化没完成”是合理候选：sidecar 在 listener ready 前会打开配置、流量数据库、认证/回放存储、规则和脚本等持久化状态。目录权限、损坏/不兼容 SQLite、半写配置、磁盘不足都可能导致提前退出或长时间阻塞。现在 stdout/stderr、child exit status 与 30 秒 deadline 会把这些原因暴露出来，但不能在没有用户日志时把其中某一项宣称为根因。

目前没有证据支持：

- Rosetta 是 M3 用户的默认修复。Rosetta 只在包内存在 Intel-only 可执行文件时才相关；架构门禁会在发布前阻止 arm64 app 混入 x86_64 sidecar。
- 证书预检、CLI 安装或 System Proxy 是最初 loading 的直接阻塞点：这些步骤分别发生在 backend ready 之后、主界面阶段或异步路径。

### 与 CLI 对齐的数据目录

- `bifrost_storage::data_dir()` 统一返回：
  - `env::var("BIFROST_DATA_DIR").ok().map(PathBuf::from).filter(|p| !p.as_os_str().is_empty())`
  - 否则 `~/.bifrost`（跨平台取 `home_dir` + `.bifrost`）。
- 桌面端 `resolve_desktop_data_dir()` 直接 `Ok(shared_bifrost_data_dir())`。
- `resolve_desktop_config_path(data_dir)` 返回 `data_dir.join("desktop-config.json")`。
- 单元测试 `desktop_data_dir_matches_shared_cli_dir` 保证桌面端与 CLI 值一致。

### 日志目录结构

```
$BIFROST_DATA_DIR/
├── desktop-config.json
├── bifrost.pid          # daemon runtime marker（CLI）
├── runtime.json         # daemon runtime marker（CLI）
├── logs/
│   ├── desktop-bootstrap.log     # 桌面壳层写入
│   ├── desktop-sidecar.out.log   # sidecar stdout 追加
│   ├── desktop-sidecar.err.log   # sidecar stderr 追加
│   └── {core tracing logs...}    # sidecar 内部 tracing 输出
```

- 所有桌面日志文件用 `OpenOptions::new().create(true).append(true)` 打开，多次启动追加而不是覆盖。
- 每条 bootstrap 记录格式：`[{SystemTime:?}] {message}`，时间戳按 `SystemTime::now()`。

### 失败信号回传

- `BackendState.startup_ready: AtomicBool`
  - 初始 `false`；bootstrap / watchdog / restart 成功时置 `true`；重试前置回 `false`。
- `BackendState.startup_error: Mutex<Option<String>>`
  - 每次成功清 `None`；`record_startup_error(&state, msg)` 写入日志同时更新为 `Some(msg)`。
- Tauri invoke `get_desktop_runtime` 返回：
  - `expectedProxyPort`
  - `proxyPort`
  - `platform`
  - `startupReady`
  - `startupError`
- 前端在 App 层订阅 `desktop://handoff-complete` 与 runtime store，当 `startupError` 出现时弹提示，用户可以点击“查看日志”跳到 `~/.bifrost/logs/`。

### 日志保留策略

- `DESKTOP_LOG_RETENTION_DAYS = bifrost_core::DEFAULT_LOG_RETENTION_DAYS`。
- `cleanup_desktop_logs_once(data_dir)` 使用 `OnceLock<Mutex<Vec<PathBuf>>>` 记录已清理过的目录，避免每次写日志都扫描。
- 首次写入 `desktop-bootstrap.log` 或 sidecar 日志时执行 `cleanup_bifrost_log_dir(&log_dir, DESKTOP_LOG_RETENTION_DAYS)`。
- 清理错误 (`Err`) 被忽略，保证日志写入永不失败。

## 关键代码入口

- `desktop/src-tauri/src/main.rs`
  - `resolve_desktop_data_dir()` / `resolve_desktop_config_path(data_dir)`
  - `log_dir(data_dir) -> PathBuf`
  - `append_desktop_bootstrap_log(data_dir, message)`
  - `open_sidecar_log_file(data_dir, file_name)`
  - `cleanup_desktop_logs_once(data_dir)`
  - `record_startup_error(&state, msg)`
  - `bootstrap_desktop_backend(app)` / `monitor_desktop_backend(app)` / `attempt_backend_recovery(app, reason)` / `restart_backend_on_port(state, current, expected)`：所有失败路径都统一走 `record_startup_error` + `append_desktop_bootstrap_log`。
  - `start_backend(binary_path, data_dir, port)`：sidecar stdout/stderr 打开为文件 handle 后传入 `Command::stdout/stderr`。
  - `get_desktop_runtime(state)` invoke handler：读取 `startup_ready` 与 `startup_error`。
- `crates/bifrost-storage/src/lib.rs`
  - `pub fn data_dir() -> PathBuf`：`BIFROST_DATA_DIR` 或 `~/.bifrost`。
- `crates/bifrost-core/src/lib.rs`
  - `pub const DEFAULT_LOG_RETENTION_DAYS: u32`
  - `pub fn cleanup_bifrost_log_dir(dir: &Path, days: u32)`
- 前端：`web/src/desktop/tauri.ts` 提供 `getDesktopRuntime()` 与运行时 store。

## 日志内容清单

`desktop-bootstrap.log` 至少覆盖以下事件（按发生顺序）：

- `desktop setup started; binary_path=... data_dir=... config_dir=...`
- `native launcher unsupported on this platform; entering webview directly`（非 macOS）
- `launcher-only mode enabled; skipping embedded webview and backend bootstrap`（BIFROST_DESKTOP_LAUNCHER_ONLY=1）
- `desktop backend bootstrap started asynchronously`
- `ensuring backend is running; preferred_port=... data_dir=...`
- `reusing existing backend instance already serving on port ...`
- `detected healthy backend candidate on port ... before spawning`
- `found existing backend runtime markers under ...; stopping stale backend`
- `starting sidecar; binary_path=... data_dir=... port=... stdout_log=... stderr_log=...`
- `backend became ready at http://127.0.0.1:{port}`
- `backend failed to become ready on port ...: {error}`
- `desktop backend bootstrap finished; active_port=...`
- `desktop certificate preflight ...`（cert bootstrap 通知）
- `desktop backend watchdog started`
- `managed backend child pid=... exited with status ...`
- `desktop backend watchdog triggering recovery; reason=...`
- `desktop backend watchdog recovery succeeded; active_port=...`
- `desktop backend watchdog recovery failed; will retry after ...`
- `starting embedded webview handoff; reason=...`
- `embedded webview handoff completed; native launcher overlay removed`
- `host window close requested on macOS; hiding window and keeping app alive`
- `desktop reopen requested on macOS; restoring host window`
- `desktop shutdown requested; hiding window and stopping backend asynchronously`
- `spawned backend stop helper pid=...`
- `detached backend child pid=... so desktop UI can exit immediately`
- `desktop shutdown handoff complete; requesting final app exit`

## 未落地部分（保持不写成事实）

- **Web 侧 `#startup-splash` 骨架**：桌面启动视觉仍是 native launcher overlay + host window handoff；没有单独的 web 层 splash。前端只需订阅 `desktop://handoff-complete` 事件把自身 loading 状态清掉。
- **结构化 log**：`desktop-bootstrap.log` 仍是自由文本，不是 JSON，而且多次启动共用追加文件、没有独立 session ID。诊断复杂问题时需要按时间人工关联。

以上都是明确的“非目标”，需要新增能力时再单独文档化。

## 依赖项

- `desktop/src-tauri/src/main.rs`
- `crates/bifrost-storage/src/lib.rs`（`data_dir()`）
- `crates/bifrost-core/src/lib.rs`（`cleanup_bifrost_log_dir`、`DEFAULT_LOG_RETENTION_DAYS`）
- Tauri 2 runtime 的 `AppHandle`、`State`。
- Tokio 无关：本文所有日志写入走 `std::fs`。

## CLI / 环境变量 / Web / Admin API 表面

- 无新 CLI。
- 环境变量：
  - `BIFROST_DATA_DIR`：数据目录与日志根。
- `BIFROST_DESKTOP_LAUNCHER_ONLY=1`：仅 launcher 模式仍写入 `desktop-bootstrap.log` 的 launcher-only 提示。
  - `BIFROST_DESKTOP_STARTUP_DEADLINE_MS`：仅测试覆盖 launcher deadline；默认 30 秒。
- Tauri invoke：`get_desktop_runtime()` 输出 `startupError` / `startupReady`。
- Admin API：无扩展。

## Sync 边界

- 日志与 startup 状态是本机数据，不同步。
- `desktop-config.json` 与 CLI 共用数据目录，但 sync 仅关心 rules / groups，不同步 desktop config。

## 实现切分

### Phase 1：数据目录统一（已完成）

- 引入 `shared_bifrost_data_dir()`。
- `resolve_desktop_data_dir()` / `resolve_desktop_config_path()`。
- 单元测试 `desktop_config_uses_shared_data_dir` / `desktop_data_dir_matches_shared_cli_dir`。

### Phase 2：日志文件落盘（已完成）

- `log_dir()` / `append_desktop_bootstrap_log()` / `open_sidecar_log_file()`。
- `start_backend` 将 sidecar stdout / stderr 指向对应文件。
- `cleanup_desktop_logs_once` 保证按天清理只做一次。

### Phase 3：失败信号回传（已完成）

- `BackendState.startup_ready` / `startup_error`。
- `record_startup_error` 统一入口。
- `get_desktop_runtime` invoke 返回 `startupError`。
- 前端 store + 弹窗展示。

### Phase 4：sidecar 快速失败与可见故障恢复

- `wait_for_backend` 同时探测 health 与 `Child::try_wait()`，记录 pid/exit status。
- 可用端口上的新 sidecar 一旦退出或超时，立即结束本次启动；端口顺延只用于跳过启动前已经被占用的端口。
- `try_start_native_handoff` 在 WebView 已 loaded 且 backend ready **或** `startup_error` 已记录时允许 handoff。
- `schedule_desktop_startup_deadline` 在 30 秒后记录状态并强制 handoff，覆盖 child 未退出但也永不 ready、WebView load event 丢失等未知阻塞。
- stale backend stop helper 使用 5 秒有界等待；超时后 kill + wait，避免 `.status()` 永久卡住 bootstrap。
- kill 后只再等待 2 秒；stop 失败会记录可恢复错误并阻断新 core，避免同一数据目录双实例。
- 端口顺延只覆盖启动前占用和确认的 bind 竞争；确定性的 child exit、检查失败与 readiness timeout 直接暴露原始错误。
- deadline 仅在 WebView loaded 时 handoff；WebView 未加载时原生 launcher 进入错误态，避免用空白 parked WebView 替换 loading。
- CI/release 在 DMG 打包前校验 app executable 与 bundled sidecar 架构。

### Phase 5：文档与人工测试维护

- 保持本文与 launcher / close / port-switch / watchdog / cert-bootstrap 边界清晰。
- Human_tests 覆盖真实失败场景。

## 测试方案

### 单元测试

- `desktop/src-tauri/src/main.rs::tests`
  - `desktop_config_uses_shared_data_dir`
  - `desktop_data_dir_matches_shared_cli_dir`
  - `parses_snake_case_port_update_response`
  - `parses_camel_case_port_update_response`
  - `detects_legacy_server_config_response`
  - `macos_close_request_hides_window`
  - `non_macos_close_request_shuts_down_app`
  - `backend_recovery_guard_prevents_parallel_recovery`
  - `poll_managed_backend_exit_reports_exited_child`
  - `wait_for_backend_reports_child_exit_without_waiting_for_timeout`
  - `desktop_startup_deadline_defaults_and_accepts_test_override`
  - `wait_for_child_exit_kills_process_after_timeout`
  - `stale_backend_stop_failure_blocks_a_second_start`
  - `port_retry_only_handles_confirmed_bind_races`
  - `startup_deadline_does_not_overwrite_a_ready_backend`
  - `startup_deadline_uses_native_error_until_webview_is_loaded`
- `crates/bifrost-storage`：`data_dir` 相关测试（默认 & 环境变量覆盖）。
- `crates/bifrost-core`：`cleanup_bifrost_log_dir` 相关测试（按天清理、错误吞噬）。

### E2E / 真实场景（`human_tests/desktop-startup-observability.md`）

- TC-DSO-01：默认启动 → 关闭 → 重启，确认 `desktop-bootstrap.log` 追加而不是覆盖。
- TC-DSO-02：`BIFROST_DATA_DIR=/tmp/bifrost-foo` 启动，验证 `desktop-config.json` 与 `logs/*` 在该目录下。
- TC-DSO-03：手动占用 9900..9964 全部端口 → 启动失败，前端弹窗展示 `startup_error`，log 记录端口顺延失败。
- TC-DSO-04：sidecar 冷启动时 panic（模拟：把 `bifrost` binary 替换为 exit 1 的 stub）→ `desktop-sidecar.err.log` 保留 stderr；`desktop-bootstrap.log` 记录 “backend failed to become ready”。
- TC-DSO-05：`BIFROST_DESKTOP_LAUNCHER_ONLY=1` → 日志仅记录 launcher-only 提示，没有 sidecar 相关行。
- TC-DSO-06：Handoff / close / reopen 事件都出现在 bootstrap 日志中。
- TC-DSO-07：日志超过 `DEFAULT_LOG_RETENTION_DAYS` 天数的旧文件会被自动清理（可 mock mtime 验证）。
- TC-DLS-08：hanging sidecar + 缩短 deadline，确认 launcher 有界退出并保留 recoverable error。

启动使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=1`、`--no-system-proxy`。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `cargo test -p bifrost-desktop --tests`
- `cargo test -p bifrost-storage data_dir`
- `cargo test -p bifrost-core cleanup_bifrost_log_dir`
- `rust-project-validate`
- 本机 no-local-coverage。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核目标：数据目录统一、三类日志文件、失败信号回传、日志保留。
- 复核 diff：`main.rs` 每条失败/成功路径是否都调 `append_desktop_bootstrap_log`；`start_backend` 是否把 stdout/stderr 都接管；`record_startup_error` 是否覆盖所有失败入口（bootstrap / watchdog / restart）。
- 重点 review：`cleanup_desktop_logs_once` 是否会在多目录场景重复清理；`get_desktop_runtime` 是否会在锁竞争时返回过期数据。
- 复测：单元测试；正常启动、`BIFROST_DATA_DIR` 场景、失败场景三次真实操作。

### 第 2 轮

- 复核第 1 轮问题修复。
- 检查 `git status --short`、`git diff`、human_tests 索引。
- 重点 review：`startupError` 在 recovery 成功后是否被清 `None`；日志文件权限（`create + append`）在不同平台是否一致。
- 复测：失败重现 + 前端弹窗验证。

## 风险与决策点

- 桌面壳层直接写文本日志而非 tracing：优先保证 sidecar tracing 未就绪时也有轨迹；未来可以引入更结构化的 pipeline，但需保持“无 tracing 依赖”的兜底能力。
- sidecar stdout / stderr 采用 `create + append`：多次启动会持续追加，长期可能变大。日志保留策略只按天数删除，未按 size 截断；可接受，若产生问题可加 size cap。
- `record_startup_error` 触发 `request_desktop_shutdown`（bootstrap 完全失败时）：让用户看到错误弹窗后进程退出，避免长期黑屏。若产品希望允许失败后继续 launcher-only 展示，可在未来解耦。
- `wait_for_backend` 已在 sidecar 提前退出时立即短路，因此立即 panic 不再消耗完整的 20 秒 readiness timeout。sidecar 保持存活但死锁时仍只能由 30 秒 launcher deadline 兜底；日志可通过最后一个已完成的 sidecar 启动 phase 缩小范围，但精确到线程的根因仍可能需要 macOS sample/spindump。
- 应用日志不记录 macOS Application Firewall 的用户决策、LaunchServices 终止原因或第三方安全软件动作。涉及这些系统层的事故，除 `~/.bifrost/logs/` 外还必须收集同时间段的 macOS Unified Log，以及存在时的 `~/Library/Logs/DiagnosticReports/` 产物。
- `desktop-config.json` 与 daemon runtime marker 共用 `~/.bifrost`：CLI 用户可能看到额外的 `desktop-config.json`，但结构简单不会破坏 CLI；反过来桌面端也能读到 CLI 生成的 `bifrost.pid` / `runtime.json` 用于复用现有 backend。
- 未做结构化 JSON 日志：诊断复杂问题需人肉阅读。若接入分布式排障工具再评估。
