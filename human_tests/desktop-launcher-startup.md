# macOS 桌面端启动页真实场景测试

## 功能模块说明

验证 macOS Tauri 桌面端启动阶段使用单个最终尺寸 host window，并展示全尺寸毛玻璃 launcher overlay、居中虚拟水平进度条和稳定的 fade-only handoff。启动页应匹配 macOS 玻璃质感，桌面背景在启动页后方若隐若现。启动过程中主 Web UI 必须保持不可见，避免响应式布局在窗口放大或中间尺寸下露出。启动页淡出后，桌面进程必须继续存活，不能因为 native overlay 动画 tick 与移除时序竞争触发启动闪退。

非 macOS 平台不展示 native launcher overlay，仍直接进入完整 host window。

## 前置条件

- 当前工作目录为 Bifrost 仓库根目录。
- 构建桌面端前端与 sidecar：
  ```bash
  pnpm --dir web run build:desktop
  cargo build -p bifrost-cli
  node scripts/prepare-tauri-sidecar.mjs debug
  cargo build --manifest-path desktop/src-tauri/Cargo.toml
  ```
- 启动真实 macOS 桌面窗口时使用临时数据目录，避免污染系统代理：
  ```bash
  export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
  export BIFROST_DISABLE_TRAY=1
  export BIFROST_DESKTOP_NO_SYSTEM_PROXY=1
  export BIFROST_DESKTOP_SKIP_CERT_PREFLIGHT=1
  export BIFROST_DATA_DIR="$(mktemp -d)"
  ```

## 测试用例列表

### TC-DLS-01 macOS 冷启动显示全尺寸启动页

操作步骤：

1. 执行前置构建命令。
2. 使用临时环境启动 `target/debug/bifrost-desktop`。
3. 启动后立即截图并观察窗口尺寸、Dock / Cmd+Tab 表现和启动页内容。

预期结果：

- Dock / Cmd+Tab / Mission Control 只出现一个 Bifrost 窗口。
- 启动窗口一开始就是最终主窗口尺寸，不再显示 360x260 小窗。
- 启动页全尺寸覆盖 host window，中间只有 Bifrost 标识和水平进度条。
- 启动页背景呈现 macOS 毛玻璃效果，能看到模糊的桌面背景透出，而不是实色遮罩。
- Bifrost 标识和进度条对比度足够，不能因为玻璃背景变得模糊或难以辨认。
- 主 Web UI 内容在启动页淡出前不可见。

### TC-DLS-02 虚拟进度条节奏

操作步骤：

1. 以 `BIFROST_DESKTOP_LAUNCHER_ONLY=1` 启动桌面端，避免 handoff 过快影响观察。
2. 启动后立即截图，约 1 秒后截图，约 1.5 秒后截图。
3. 对比三张截图中的进度条填充长度。

预期结果：

- 首次可见时进度条已经约为 21%，不是从 0 开始。
- 约 1 秒时进度条约为 80%。
- 约 1.5 秒后进度条停在约 99%，不会自行完成到 100%。
- 进度条为虚拟进度，不显示百分比数字，不出现倒退或跳闪。

### TC-DLS-03 启动页背景与暗色/亮色风格过渡

操作步骤：

1. 使用当前系统/应用暗色偏好启动 `BIFROST_DESKTOP_LAUNCHER_ONLY=1` 桌面端并截图。
2. 切换到亮色偏好或使用已有亮色主题状态重新启动并截图。
3. 对比启动页背景、标题、进度条和主窗口背景的过渡观感。

预期结果：

- 启动页背景使用毛玻璃/系统动态色效果，暗色和亮色下都接近主窗口背景风格，并保留桌面背景模糊透出的玻璃感。
- 不出现固定深黑或固定纯白底色导致的强烈闪烁。
- 标题和进度条在暗色/亮色下均清晰可读，进度填充与轨道有明确对比，且不过度抢眼。

### TC-DLS-04 Handoff 只做 launcher 淡出和正式页面淡入

操作步骤：

1. 不设置 `BIFROST_DESKTOP_LAUNCHER_ONLY`，使用临时数据目录启动真实桌面端。
2. 观察 backend/WebView ready 后的 handoff 过程并截图。
3. Handoff 完成后拖拽或调整窗口，观察主界面尺寸和布局。

预期结果：

- Handoff 过程中窗口不再从小变大，位置和尺寸保持稳定。
- 主 Web UI 在启动页淡出前保持不可见；启动页淡出后正式页面从同一尺寸下显现。
- Handoff 完成后窗口仍为最终主窗口尺寸，主 UI 布局稳定，无中间尺寸重排痕迹。

### TC-DLS-05 启动 handoff 后不闪退回归

操作步骤：

1. 执行前置构建命令。
2. 使用临时环境启动 `desktop/src-tauri/target/debug/bifrost-desktop`，环境变量必须包含 `BIFROST_DESKTOP_NO_SYSTEM_PROXY=1`、`BIFROST_DESKTOP_SKIP_CERT_PREFLIGHT=1`、`BIFROST_DISABLE_TRAY=1` 和独立 `BIFROST_DATA_DIR`。
3. 启动后等待至少 8 秒，覆盖 native launcher overlay 动画、WebView load、backend ready、handoff、overlay fade/remove 以及晚到动画 tick 的时间窗。
4. 检查桌面进程仍然存活；如已退出，读取 stdout/stderr 与 `desktop-bootstrap.log` 判断是否出现 Rust panic、foreign exception、`SIGABRT` 或启动期崩溃。

预期结果：

- `bifrost-desktop` 在 8 秒启动观察窗口内保持运行。
- `desktop-bootstrap.log` 至少包含 `desktop setup started`、`embedded webview page load event`、`starting embedded webview handoff` 和 `embedded webview handoff completed`。
- 不产生 macOS crash report；进程不因 native overlay 移除、动画线程 tick 或 Objective-C 对象生命周期触发 `EXC_CRASH (SIGABRT)`。
- 测试使用临时 `BIFROST_DATA_DIR`，不修改系统代理，不弹出证书信任或 LaunchDaemon 授权窗口。

### TC-DLS-06 Sidecar 提前退出时快速进入可恢复错误界面

操作步骤：

1. 完成前置构建，确认 `desktop/src-tauri/target/debug/bifrost-desktop` 存在。
2. 执行 `SKIP_BUILD=true e2e-tests/tests/test_desktop_launcher_startup_failure_handoff.sh`。脚本会创建独立 `BIFROST_DATA_DIR`、动态空闲端口和一个输出错误后以 42 退出的 sidecar stub。
3. 检查 `desktop-bootstrap.log`、`desktop-sidecar.err.log` 和桌面进程状态。

预期结果：

- readiness 等待检测到 sidecar child 退出后立即失败，不等待完整 20 秒，也不顺延到其余候选端口重复尝试。
- `desktop-bootstrap.log` 包含唯一 `session_id`、desktop PID、版本/OS/架构、sidecar spawn PID、`exited before becoming ready`、`desktop backend bootstrap failed`、`starting embedded webview handoff` 和 `embedded webview handoff completed`。
- `desktop-sidecar.err.log` 保留 stub 的原始 stderr，并且其中的 `session_id` 与 bootstrap 日志一致。
- 桌面进程继续存活，native launcher 在 10 秒内移除，主 WebView 可读取 `startupError` 并展示 “Start Bifrost Service” 重试入口。
- 测试不复用本机已有 9900 backend，不修改系统代理或用户数据。

### TC-DLS-07 macOS 发布包主程序与 Sidecar 架构一致

操作步骤：

1. 执行 `e2e-tests/tests/test_macos_desktop_architecture_gate.sh`。
2. 检查 thin 匹配场景、包含目标架构的 universal binary 场景，以及缺少目标架构的不匹配场景。
3. 检查 `.github/workflows/ci.yml` 与 `.github/workflows/release.yml` 都在 macOS app 重新签名后调用 `scripts/validate-macos-desktop-architectures.sh`。

预期结果：

- target 为 `aarch64-apple-darwin` 时，桌面主程序和 `Contents/Resources/resources/bin/bifrost` 都必须包含 `arm64`；target 为 `x86_64-apple-darwin` 时两者都必须包含 `x86_64`。thin 与 universal binary 均允许。
- 任一组件架构不匹配时脚本非零退出，并打印包含 expected、actual、target 和文件路径的诊断。
- 普通 CI bundle 与 release bundle 都执行同一门禁，混合架构 DMG 无法发布。

### TC-DLS-08 启动链路阻塞时按期限退出原生 Loading 页

操作步骤：

1. 完成前置构建，确认 `desktop/src-tauri/target/debug/bifrost-desktop` 存在。
2. 执行 `SKIP_BUILD=true e2e-tests/tests/test_desktop_launcher_startup_deadline_handoff.sh`。脚本使用不会监听端口也不会退出的 sidecar stub，并把 `BIFROST_DESKTOP_STARTUP_DEADLINE_MS` 设为 1500。
3. 检查 `desktop-bootstrap.log`、桌面进程状态与 handoff 时间。

预期结果：

- 后端子进程即使永久阻塞，原生 launcher 也在 deadline 后移除，不会无限停在 99%。
- `desktop-bootstrap.log` 包含 `desktop startup deadline exceeded`、可恢复的 `startup_error` 以及 handoff start/completed。
- 桌面进程继续存活，主 WebView 可展示 “Start Bifrost Service” 重试入口。
- 默认产品 deadline 为 30 秒；测试专用环境变量只缩短测试等待，不改变正式默认值。

### TC-DLS-09 Stale backend 停止失败时禁止启动第二实例

操作步骤：

1. 完成前置构建，确认 `desktop/src-tauri/target/debug/bifrost-desktop` 存在。
2. 执行 `SKIP_BUILD=true e2e-tests/tests/test_desktop_stale_backend_stop_failure_handoff.sh`。脚本创建 runtime marker，并使用 `stop` 返回 17、其他命令记录为意外启动的 sidecar stub。
3. 检查 `desktop-bootstrap.log`、stub 调用记录与桌面进程状态。

预期结果：

- bootstrap log 明确记录 stale backend stop 失败，并说明拒绝为同一数据目录启动第二个 backend。
- sidecar stub 只收到一次 `stop`，不得收到 `start` 或其他第二实例启动参数。
- failure handoff 完成，桌面进程继续存活并向用户暴露可恢复错误。
- debug 测试使用临时 `BIFROST_DATA_DIR` 和仅 debug 生效的多实例开关，不修改本机服务、系统代理、正式 App 或用户数据。

### TC-DLS-10 正式 App 运行时仍可隔离执行启动诊断回归

操作步骤：

1. 记录当前 `/Applications/Bifrost.app/Contents/MacOS/bifrost-desktop` PID；若本机未安装或未运行正式 App，则记录为不适用但继续后续步骤。
2. 确认 `desktop/src-tauri/target/debug/bifrost-desktop` 是当前分支最新构建。
3. 依次执行 `SKIP_BUILD=true e2e-tests/tests/test_desktop_launcher_startup_failure_handoff.sh` 与 `SKIP_BUILD=true e2e-tests/tests/test_desktop_launcher_startup_deadline_handoff.sh`。
4. 再次检查正式 App PID，并检查两个测试各自临时 `BIFROST_DATA_DIR` 的 bootstrap/sidecar 日志断言结果。

预期结果：

- debug 测试通过 `BIFROST_DESKTOP_TEST_ALLOW_MULTIPLE_INSTANCES=1` 仅关闭测试进程的 single-instance plugin，不要求退出或杀死正式 App。
- release 构建不响应此测试开关，正式产品仍保持 single-instance。
- 两条 E2E 都能启动隔离 App、完成 failure handoff，并证明 bootstrap 与 sidecar stderr 使用同一 `session_id`。
- 若测试前存在正式 App，其 PID 在测试后仍存活；测试不改变正式 App 数据目录、9900 服务或系统代理。

### TC-DLS-11 低日志级别下保留启动阶段且 Bootstrap 行不穿插

操作步骤：

1. 执行 `RUST_LOG=warn SKIP_BUILD=true e2e-tests/tests/test_desktop_launcher_startup_no_crash.sh`。
2. 检查独立数据目录中的 `desktop-bootstrap.log` 与日期化 `bifrost*.log`。
3. 确认脚本实际使用动态空闲端口拉起当前分支 sidecar，而不是复用正式 9900 backend。

预期结果：

- sidecar 日志保留用户的 `warn` filter，同时仍包含当前 `session_id`、`startup phase started` 与 `startup phase completed`。
- bootstrap 日志包含 desktop/sidecar PID、版本、OS、架构和相同 `session_id`。
- bootstrap/watchdog/handoff 并发写入后，每行仍完整匹配单个 `[SystemTime ...] message`，没有两条日志拼接或半行。
- 正常启动完成 handoff，测试 App 保持存活到观察窗口结束，正式 App 不受影响。

### TC-DLS-12 恢复与端口切换不得在旧 Core 未确认退出时启动替代实例

操作步骤：

1. 执行 `SKIP_FRONTEND_BUILD=1 cargo test --manifest-path desktop/src-tauri/Cargo.toml restart_stop_failure_blocks_a_replacement_backend`。
2. 执行 `SKIP_FRONTEND_BUILD=1 cargo test --manifest-path desktop/src-tauri/Cargo.toml restart_requires_the_old_backend_to_be_observed_down`。
3. 执行 `SKIP_FRONTEND_BUILD=1 cargo test --manifest-path desktop/src-tauri/Cargo.toml poisoned_managed_child_state_blocks_a_replacement_backend`。
4. 执行 `SKIP_BUILD=true e2e-tests/tests/test_desktop_stale_backend_stop_failure_handoff.sh`，复核真实桌面启动 handoff 链路仍保持 fail-closed。

预期结果：

- stop helper 非零退出时，端口切换回退返回明确错误，不进入 replacement core 启动阶段。
- stop helper 成功但旧端口在期限后仍健康时，端口切换回退仍拒绝 replacement core。
- managed child 状态不可读取或 child 无法终止时，watchdog、手动重试和端口切换均停止恢复，不吞错后继续启动。
- 真实桌面 E2E 中 sidecar stub 只收到一次 `stop`，桌面完成 failure handoff 并保持可恢复，而不是启动第二实例。

### TC-DLS-13 marker 缺失时复用首选端口上的健康 Bifrost（回归）

操作步骤：

1. 使用临时 `BIFROST_DATA_DIR` 和动态首选端口启动隔离 Bifrost，确认 Admin identity 与 health API 均正常。
2. 删除临时目录中的 `runtime.json` 与 `bifrost.pid`，模拟历史 CLI 与 Desktop 生命周期漂移。
3. 启动使用同一临时数据目录和首选端口的 debug Desktop，检查 `desktop-bootstrap.log` 和实际监听端口。
4. 另设一个健康 Bifrost 在 `preferred_port + 1`，确认 marker 缺失时不会把它当成首选实例复用。

预期结果：

- Desktop 复用首选端口上的健康 Bifrost，bootstrap log 包含 `missing lifecycle markers; reusing it without claiming ownership`。
- Desktop 不启动第二个 9901 类替代实例，也不会停止或认领 markerless 外部进程。
- 非首选端口上的 markerless Bifrost 不会被自动复用。
- 全流程使用临时目录、动态端口并禁用系统代理，正式 App 与正式 9900 服务不受影响。

### TC-DLS-14 端口抢占与陌生 Core 不得伪造 Desktop 恢复（回归）

操作步骤：

1. 执行 `node scripts/prepare-tauri-sidecar.mjs debug`，准备当前 checkout 的 Desktop sidecar 资源。
2. 执行 `SKIP_FRONTEND_BUILD=1 cargo test --manifest-path desktop/src-tauri/Cargo.toml health_only_external_backend_cannot_clear_manual_start_gate`。
3. 执行 `SKIP_FRONTEND_BUILD=1 cargo test --manifest-path desktop/src-tauri/Cargo.toml matching_markerless_backend_clears_manual_start_gate`。
4. 执行 `SKIP_FRONTEND_BUILD=1 cargo test --manifest-path desktop/src-tauri/Cargo.toml bind_conflict_detection_reads_only_new_sidecar_stderr`。
5. 执行 `SKIP_FRONTEND_BUILD=1 cargo test --manifest-path desktop/src-tauri/Cargo.toml port_retry_only_handles_confirmed_bind_races`。

预期结果：

- 只返回 HTTP 200、但不提供匹配 Core identity 的监听者不能清除启动错误，Desktop 不会把登录请求发给它。
- identity、data-dir fingerprint 与首选端口均匹配的 markerless Bifrost 仍可恢复，保持 TC-DLS-13 的兼容语义。
- 历史 sidecar stderr 中旧的端口冲突不触发回退；只有本次启动新增且端口一致的 bind error 才触发下一候选端口。
- 全部测试使用临时目录和动态端口，不停止正式 Core、不修改系统代理、不读取用户登录凭证。

## 清理步骤

```bash
# E2E 脚本通过 trap 只终止自身记录的 APP_PID/sidecar PID，并删除自身 mktemp 目录。
# 手动测试时也只能终止本次记录的 PID；禁止 pkill/killall，以免关闭正式 Bifrost。
kill "$TEST_APP_PID" 2>/dev/null || true
rm -rf "$TEST_DATA_DIR"
```

## 执行记录

| 日期 | 用例 | 执行命令 / 证据 | 结果 |
| --- | --- | --- | --- |
| 2026-07-06 | TC-DLS-01 | `BIFROST_DESKTOP_LAUNCHER_ONLY=1 desktop/src-tauri/target/debug/bifrost-desktop`，临时目录 `/tmp/bifrost-launcher-direct-timed-20260706090421`；System Events 记录 `window=Bifrost pos=560,230 size=1440x920`；截图 `t000.png`。 | 通过：启动窗口首次可见即为 1440x920，全尺寸毛玻璃 overlay 覆盖窗口，中间仅 Bifrost 标识和水平进度条，未见主 Web UI。 |
| 2026-07-06 | TC-DLS-02 | `SKIP_FRONTEND_BUILD=1 cargo test --manifest-path desktop/src-tauri/Cargo.toml`，新增用例 `virtual_progress_matches_startup_milestones` 和 `handoff_progress_uses_only_final_one_percent`；截图 `/tmp/bifrost-launcher-direct-timed-20260706090421/t000.png`、`t100.png`、`t155.png`。 | 通过：单测精确断言 0s=21%、1s=80%、1.5s=99%、之后保持 99%，handoff 只补最后 1%；真实截图无百分比数字或跳闪。 |
| 2026-07-06 | TC-DLS-03 | 当前系统外观为亮色（`defaults read -g AppleInterfaceStyle` 无输出）；新版截图 `/tmp/bifrost-launcher-direct-timed-20260706090421/t000.png` 验证启动页背景和前景对比。 | 通过：亮色环境下启动页呈柔和浅灰毛玻璃，标题和进度条使用深灰中性色，可读且不过度抢眼，未出现固定深黑或纯白闪屏；暗色外观未通过全局系统切换实测，代码路径按 `window.theme()` 选择暗色 palette 并依赖系统 `UnderWindowBackground` 材质适配。 |
| 2026-07-06 | TC-DLS-04 | `pnpm exec tauri dev --config desktop/src-tauri/tauri.conf.json`，临时目录 `/tmp/bifrost-launcher-handoff-20260706085524`；截图 `initial.png`、`after.png`；窗口记录 `window-initial.txt` 与 `window-after.txt` 均为 `560,230 1440x920`。 | 通过：handoff 前后窗口位置和尺寸保持一致，启动页淡出后主界面在同一尺寸下显现，未见小窗放大或中间尺寸重排。 |
| 2026-07-09 | TC-DLS-05 | `source ~/.zshrc && e2e-tests/tests/test_desktop_launcher_startup_no_crash.sh`；脚本构建 `web/dist-desktop`、`bifrost-cli` sidecar 与 `desktop/src-tauri/target/debug/bifrost-desktop`，使用临时 `BIFROST_DATA_DIR`、禁用系统代理/托盘/证书预检，启动后等待 8 秒，并断言 `desktop-bootstrap.log` 包含 setup、page load、handoff start、handoff completed。补充复跑：`source ~/.zshrc && SKIP_BUILD=true e2e-tests/tests/test_desktop_launcher_startup_no_crash.sh`。CI shell shard 适配验证：`source ~/.zshrc && SKIP_BUILD=true BIFROST_DESKTOP_APP_BIN=/tmp/bifrost-missing-desktop-bin e2e-tests/tests/test_desktop_launcher_startup_no_crash.sh`。 | 通过：真实启动两次均输出 `PASS: bifrost-desktop stayed alive through launcher handoff startup window`，验证 handoff 后进程保持存活且日志到达完整 handoff，未出现启动期 `SIGABRT`/foreign exception 闪退；CI 缺少桌面 debug binary 且 `SKIP_BUILD=true` 时输出 `SKIP: missing desktop binary ...` 并退出 0，避免通用 shell shard 误报失败。 |
| 2026-07-15 | TC-DLS-06 | `source ~/.zshrc && SKIP_BUILD=true e2e-tests/tests/test_desktop_launcher_startup_failure_handoff.sh`；使用动态空闲端口与 exit 42 sidecar stub，检查 bootstrap/sidecar stderr、进程存活和 failure handoff。 | 通过：输出 `PASS: sidecar failure became visible through launcher handoff in 2s`；日志包含 child 提前退出、startup error、handoff start/completed，未复用本机 9900 backend。 |
| 2026-07-15 | TC-DLS-07 | `e2e-tests/tests/test_macos_desktop_architecture_gate.sh`；验证 arm64 thin、arm64+x86_64 universal fixture，再验证缺少 arm64 的 sidecar 被拒绝。 | 通过：thin/universal 包逐个输出 `Validated macOS architecture`，缺失目标架构的包非零退出并包含 `Architecture mismatch`。 |
| 2026-07-15 | TC-DLS-08 | `source ~/.zshrc && SKIP_BUILD=true e2e-tests/tests/test_desktop_launcher_startup_deadline_handoff.sh`；使用 hanging sidecar stub 和 1500ms deadline。 | 通过：输出 `PASS: launcher deadline exposed a recoverable startup error instead of hanging indefinitely`；日志记录 deadline exceeded、startup error 与 handoff completed，进程保持存活。 |
| 2026-07-15 | TC-DLS-09 | `SKIP_BUILD=true e2e-tests/tests/test_desktop_stale_backend_stop_failure_handoff.sh`；使用 stop exit 17 stub、runtime marker 与独立数据目录。 | 通过：stub 只收到一次 `stop`；bootstrap log 记录拒绝第二实例、startup error 与 handoff completed；桌面进程保持存活。 |
| 2026-07-15 | TC-DLS-10 | 先记录正式 App PID `15260`，再依次执行 `SKIP_BUILD=true e2e-tests/tests/test_desktop_launcher_startup_failure_handoff.sh` 与 `SKIP_BUILD=true e2e-tests/tests/test_desktop_launcher_startup_deadline_handoff.sh`，最后复查正式 App PID。 | 通过：两条测试分别输出 failure handoff / deadline handoff PASS；debug App 在正式 App 同时运行时成功启动，bootstrap 与 sidecar stderr 的 session ID 断言通过；正式 App PID 前后均为 `15260`，未被测试终止。 |
| 2026-07-15 | TC-DLS-11 | 正式 App PID `15260` 运行时执行 `RUST_LOG=warn SKIP_BUILD=true e2e-tests/tests/test_desktop_launcher_startup_no_crash.sh`，断言独立端口、session/phase 日志和 bootstrap 行格式。 | 通过：输出 `PASS: bifrost-desktop stayed alive through launcher handoff startup window`；`warn` 环境下仍保留 startup info，所有 bootstrap 行满足单行格式，正式 App PID 前后保持 `15260`。 |
| 2026-07-16 | TC-DLS-12 | `cargo test` 定点执行 restart stop 失败、旧端口仍健康、managed child mutex poison 三个 fail-closed 用例；随后执行 `SKIP_BUILD=true e2e-tests/tests/test_desktop_stale_backend_stop_failure_handoff.sh`。 | 通过：三个定点单测各 1 passed；真实桌面 E2E 输出 `PASS: stale backend stop failure blocked a second backend and exposed recovery UI`，sidecar 未进入第二实例启动。 |
| 2026-08-23 | TC-DLS-13 | `SKIP_BUILD=true bash e2e-tests/tests/test_desktop_service_ownership_lifecycle.sh`，临时数据目录与动态连续端口；另执行 Desktop 聚焦单测 `normal_startup_reuses_markerless_bifrost_only_on_the_preferred_port`。 | 通过：Desktop bootstrap 记录 `missing lifecycle markers; reusing it without claiming ownership`；未监听 fallback 端口；退出 Desktop 后 markerless core 仍存活，随后由带端口的 CLI stop 回收。完整 Desktop ownership E2E 同时覆盖 sustained stall 与真实 child exit 恢复。 |
| 2026-09-02 | TC-DLS-14 | 先构建当前 debug CLI 并执行 `node scripts/prepare-tauri-sidecar.mjs debug`，随后依次执行 4 个 Desktop 聚焦测试；同时复核 `desktop-sidecar.err.log` 的本次启动 offset 隔离。 | 通过：health-only 外部监听未清除 manual-start gate；匹配 data-dir fingerprint 的 markerless 首选端口 Core 正常恢复；旧 stderr 未触发冲突，新追加且端口匹配的 bind error 正确触发；4 个定点测试各 1 passed。 |
