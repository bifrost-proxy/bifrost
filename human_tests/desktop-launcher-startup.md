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

## 清理步骤

```bash
pkill -f bifrost-desktop || true
bifrost stop || true
rm -rf "$BIFROST_DATA_DIR"
```

## 执行记录

| 日期 | 用例 | 执行命令 / 证据 | 结果 |
| --- | --- | --- | --- |
| 2026-07-06 | TC-DLS-01 | `BIFROST_DESKTOP_LAUNCHER_ONLY=1 desktop/src-tauri/target/debug/bifrost-desktop`，临时目录 `/tmp/bifrost-launcher-direct-timed-20260706090421`；System Events 记录 `window=Bifrost pos=560,230 size=1440x920`；截图 `t000.png`。 | 通过：启动窗口首次可见即为 1440x920，全尺寸毛玻璃 overlay 覆盖窗口，中间仅 Bifrost 标识和水平进度条，未见主 Web UI。 |
| 2026-07-06 | TC-DLS-02 | `SKIP_FRONTEND_BUILD=1 cargo test --manifest-path desktop/src-tauri/Cargo.toml`，新增用例 `virtual_progress_matches_startup_milestones` 和 `handoff_progress_uses_only_final_one_percent`；截图 `/tmp/bifrost-launcher-direct-timed-20260706090421/t000.png`、`t100.png`、`t155.png`。 | 通过：单测精确断言 0s=21%、1s=80%、1.5s=99%、之后保持 99%，handoff 只补最后 1%；真实截图无百分比数字或跳闪。 |
| 2026-07-06 | TC-DLS-03 | 当前系统外观为亮色（`defaults read -g AppleInterfaceStyle` 无输出）；新版截图 `/tmp/bifrost-launcher-direct-timed-20260706090421/t000.png` 验证启动页背景和前景对比。 | 通过：亮色环境下启动页呈柔和浅灰毛玻璃，标题和进度条使用深灰中性色，可读且不过度抢眼，未出现固定深黑或纯白闪屏；暗色外观未通过全局系统切换实测，代码路径按 `window.theme()` 选择暗色 palette 并依赖系统 `UnderWindowBackground` 材质适配。 |
| 2026-07-06 | TC-DLS-04 | `pnpm exec tauri dev --config desktop/src-tauri/tauri.conf.json`，临时目录 `/tmp/bifrost-launcher-handoff-20260706085524`；截图 `initial.png`、`after.png`；窗口记录 `window-initial.txt` 与 `window-after.txt` 均为 `560,230 1440x920`。 | 通过：handoff 前后窗口位置和尺寸保持一致，启动页淡出后主界面在同一尺寸下显现，未见小窗放大或中间尺寸重排。 |
| 2026-07-09 | TC-DLS-05 | `source ~/.zshrc && e2e-tests/tests/test_desktop_launcher_startup_no_crash.sh`；脚本构建 `web/dist-desktop`、`bifrost-cli` sidecar 与 `desktop/src-tauri/target/debug/bifrost-desktop`，使用临时 `BIFROST_DATA_DIR`、禁用系统代理/托盘/证书预检，启动后等待 8 秒，并断言 `desktop-bootstrap.log` 包含 setup、page load、handoff start、handoff completed。补充复跑：`source ~/.zshrc && SKIP_BUILD=true e2e-tests/tests/test_desktop_launcher_startup_no_crash.sh`。CI shell shard 适配验证：`source ~/.zshrc && SKIP_BUILD=true BIFROST_DESKTOP_APP_BIN=/tmp/bifrost-missing-desktop-bin e2e-tests/tests/test_desktop_launcher_startup_no_crash.sh`。 | 通过：真实启动两次均输出 `PASS: bifrost-desktop stayed alive through launcher handoff startup window`，验证 handoff 后进程保持存活且日志到达完整 handoff，未出现启动期 `SIGABRT`/foreign exception 闪退；CI 缺少桌面 debug binary 且 `SKIP_BUILD=true` 时输出 `SKIP: missing desktop binary ...` 并退出 0，避免通用 shell shard 误报失败。 |
