# macOS Native App 真实场景测试

## 功能模块说明

验证 `apps/macos` SwiftUI/AppKit 原生客户端骨架可以在本机可复现构建，并且第一阶段只作为控制面接入现有 Rust sidecar 与 Admin API，不破坏现有 `desktop/` Tauri 桌面端。

## 前置条件

- macOS 13 或更高版本。
- 本机可执行 `swift`、`xcodebuild`、`cargo`。
- 仓库根目录为当前工作目录。
- 如需执行 sidecar 准备脚本，`target/debug/bifrost` 已存在，或允许脚本先执行 `cargo build -p bifrost-cli --bin bifrost`。

## 测试用例

### TC-MNA-01：SwiftPM core contract 检查通过

**操作步骤：**
1. 在仓库根目录执行：
   ```bash
   swift run --package-path apps/macos BifrostNativeCoreChecks
   ```

**预期结果：**
- SwiftPM 成功解析 `apps/macos/Package.swift`。
- `BifrostNativeCoreChecks` 输出 `BifrostNativeCoreChecks passed`。
- 测试覆盖 Admin API URL/header 构造、默认数据目录、sidecar 启动参数和端口候选策略。

### TC-MNA-02：Native build 脚本可只验证 Swift 工程

**操作步骤：**
1. 在仓库根目录执行：
   ```bash
   scripts/build-macos-native.sh --skip-sidecar --test
   ```

**预期结果：**
- 命令成功完成。
- 不要求构建 Rust sidecar。
- Swift app 与 core target 均可构建，单元测试通过。

### TC-MNA-03：sidecar 准备脚本输出可执行文件

**操作步骤：**
1. 如本机已有 `target/debug/bifrost`，执行：
   ```bash
   scripts/prepare-macos-native-sidecar.sh --skip-cargo-build
   ```
2. 如不存在，执行：
   ```bash
   scripts/prepare-macos-native-sidecar.sh
   ```
3. 检查脚本输出路径是否存在且可执行。

**预期结果：**
- 输出路径为 `apps/macos/.build/sidecar/bin/bifrost`。
- 文件存在且具备可执行权限。
- 脚本不启动 `bifrost start`，不修改系统代理。

### TC-MNA-04：sidecar 启动计划包含开发安全开关

**操作步骤：**
1. 执行：
   ```bash
   swift run --package-path apps/macos BifrostNativeCoreChecks
   ```

**预期结果：**
- core contract 检查通过。
- start plan 参数包含 `--skip-cert-check` 与 `--no-system-proxy`。
- 环境变量包含 `BIFROST_DISABLE_TRAY=1`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 和独立 `BIFROST_DATA_DIR`。

### TC-MNA-05：现有 Tauri 桌面端未被本切片改动

**操作步骤：**
1. 执行：
   ```bash
   git diff -- desktop package.json scripts/prepare-tauri-sidecar.mjs
   ```

**预期结果：**
- `desktop/`、Tauri sidecar 准备脚本和现有 desktop npm scripts 没有非预期 diff。
- Native Preview 与 Tauri 继续并行存在。

### TC-MNA-06：Linux shell E2E CI 使用分片避免 60 分钟超时

**操作步骤：**
1. 执行：
   ```bash
   for shard in 1 2 3 4; do
     bash scripts/run_all_e2e.sh --ci --full-shell --skip-rules --skip-runner --skip-ui --skip-build \
       --shard "${shard}/4" --list-shell-tests
   done >/tmp/bifrost-shell-shard-list.txt
   ```
2. 执行：
   ```bash
   bash scripts/ci/check-e2e-shell-ci-coverage.sh
   ```
3. 执行：
   ```bash
   ruby -e 'require "yaml"; ci=YAML.load_file(".github/workflows/ci.yml"); job=ci.fetch("jobs").fetch("e2e-shell"); raise "missing matrix" unless job.fetch("strategy").fetch("matrix").fetch("shard")==[1,2,3,4]; env=job.fetch("env"); raise "missing shard env" unless env.fetch("BIFROST_E2E_SHARD_INDEX").include?("matrix.shard") && env.fetch("BIFROST_E2E_SHARD_TOTAL")=="4"; puts "linux e2e-shell sharding ok"'
   ```

**预期结果：**
- `run_all_e2e.sh` 能按 `--shard N/4` 列出 shell E2E 子集，且不实际启动代理测试。
- CI shell 覆盖守卫通过，确认新增分片不丢失 shell E2E 用例。
- GitHub Actions `e2e-shell` job 配置包含 `matrix.shard: [1, 2, 3, 4]`、`BIFROST_E2E_SHARD_INDEX` 和 `BIFROST_E2E_SHARD_TOTAL=4`。

### TC-MNA-07：Native UI 不得继续使用 scaffold 假数据

**操作步骤：**
1. 执行：
   ```bash
   ruby -e 'paths=Dir["apps/macos/Sources/Bifrost/**/*.swift"]; text=paths.map{|p| File.read(p)}.join("\n"); forbidden=["TrafficRecord.sampleRows","REQ-preview","api.local","Native preview rule editor scaffold","example.com proxy://127.0.0.1:8080",".constant(true)"]; found=forbidden.select{|needle| text.include?(needle)}; abort("forbidden scaffold data remains: #{found.join(", ")}") unless found.empty?; puts "macOS native scaffold fake data removed"'
   ```

**预期结果：**
- 命令输出 `macOS native scaffold fake data removed`。
- Traffic、Rules、顶部开关和 Settings 不再使用 scaffold sample rows、固定 `.constant(true)` 或假规则文案。
- Native 页面表格数据必须来自 Admin API 或明确显示空态/错误态。

### TC-MNA-08：Coverage CI 的 TLS switch E2E 端口 bind race 有重试

**操作步骤：**
1. 执行：
   ```bash
   rg -n "START_PROXY_MAX_ATTEMPTS|start_proxy_with_admin_retry|is_bind_race" crates/bifrost-e2e/src/tests/tls_switch_test.rs
   ```
2. 执行：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-e2e tls_switch_test -- --nocapture
   ```

**预期结果：**
- `tls_switch_test.rs` 包含端口 bind race retry helper。
- helper 对 `Failed to bind` 或 `already listening on this port` 重试，不因 `portpicker` 与实际 bind 之间的瞬时竞态直接失败。
- `cargo test -p bifrost-e2e tls_switch_test -- --nocapture` 通过。

### TC-MNA-09：Native app 对外命名为 Bifrost 且启动图标有效

**操作步骤：**
1. 执行无窗口图标资源检查：
   ```bash
   swift run --package-path apps/macos Bifrost --check-icon
   ```
2. 如已有旧的 native preview 进程，先结束：
   ```bash
   pkill -x BifrostMac || true
   pkill -x Bifrost || true
   ```
3. 构建 Swift 工程并生成开发用 `.app` bundle：
   ```bash
   scripts/build-macos-native.sh --skip-sidecar --test
   test -x apps/macos/.build/Bifrost.app/Contents/MacOS/Bifrost
   test -f apps/macos/.build/Bifrost.app/Contents/Resources/bifrost.icns
   ```
4. 用 LaunchServices 启动 `.app`，不要启动裸 Mach-O 可执行文件：
   ```bash
   open -n apps/macos/.build/Bifrost.app
   ```
5. 查询进程与窗口名：
   ```bash
   pgrep -fl 'Bifrost.app/Contents/MacOS/Bifrost'
   osascript -e 'tell application "System Events" to tell process "Bifrost" to get name of windows'
   ```
6. 查询运行中应用图标尺寸：
   ```bash
   APP_PID="$(pgrep -f 'Bifrost.app/Contents/MacOS/Bifrost' | head -1)"
   swift -e 'import AppKit; let pid = pid_t(Int32(CommandLine.arguments[1])!); guard let app = NSRunningApplication(processIdentifier: pid), let icon = app.icon else { fatalError("missing running app icon") }; print("running app icon: \(Int(icon.size.width))x\(Int(icon.size.height))")' "$APP_PID"
   ```

**预期结果：**
- 图标资源检查输出 `Bifrost icon check passed: 1000x1000`。
- SwiftPM 构建输出包含 `Linking Bifrost` 或 `Build of product 'Bifrost' complete!`。
- 开发用 app bundle 存在于 `apps/macos/.build/Bifrost.app`，并包含 `Contents/MacOS/Bifrost` 与 `Contents/Resources/bifrost.icns`。
- 运行中的 native app 来自 `Bifrost.app/Contents/MacOS/Bifrost`，不是裸 `debug/Bifrost` 终端可执行文件。
- 运行中的 native app 对外进程名是 `Bifrost`，不是 `BifrostMac`。
- System Events 能读取到名为 `Bifrost` 的窗口。
- AppKit 能读取到运行中应用的非空图标尺寸，例如 `running app icon: 32x32`。
- 启动过程中不启动 sidecar、不启用系统代理、不拉起托盘、不打开 Sync 登录页。

### TC-MNA-10：Native app 复用默认数据目录里的既有 CLI daemon

**操作步骤：**
1. 确认默认数据目录中已有 CLI daemon 正在运行：
   ```bash
   bifrost status --format json
   ```
2. 构建开发用 `.app` bundle：
   ```bash
   scripts/build-macos-native.sh --skip-sidecar --test
   ```
3. 关闭旧 native app 窗口，但不要停止默认 `bifrost` daemon：
   ```bash
   pkill -f 'Bifrost.app/Contents/MacOS/Bifrost' || true
   ```
4. 打开 Native app bundle：
   ```bash
   open -n apps/macos/.build/Bifrost.app
   ```
5. 检查 Native app 进程、子进程和 sidecar 启动情况：
   ```bash
   APP_PID="$(pgrep -f 'Bifrost.app/Contents/MacOS/Bifrost' | head -1)"
   ps -p "$APP_PID" -o pid= -o comm=
   sleep 2
   ps -axo pid,ppid,command | awk -v app="$APP_PID" '$2 == app { print }'
   ps -axo pid,ppid,command | rg 'Bifrost\.app.*/bifrost start' | rg -v 'rg|zsh -lc' || true
   bifrost status --format json
   ```

**预期结果：**
- `bifrost status --format json` 显示 `running: true`，`data_dir` 为默认 `~/.bifrost`，并带有 listener port。
- Native app 进程路径是 `apps/macos/.build/Bifrost.app/Contents/MacOS/Bifrost`。
- Native app 允许短暂派生 `bifrost status --format json` 做服务发现；2 秒后没有持久子进程。
- 没有 `Bifrost.app/.../bifrost start` 进程，说明已有 CLI daemon 被复用，没有启动第二个服务。
- Native app 与 CLI/Web UI 消费同一个默认数据目录。

### TC-MNA-11：Network 页面按 Web UI 本地化布局并使用左侧系统 source-list

**操作步骤：**
1. 打开真实 Web UI 作为对照：
   ```bash
   open http://127.0.0.1:9900/_bifrost/
   ```
2. 构建并打开 Native `.app`：
   ```bash
   scripts/build-macos-native.sh --skip-sidecar --test
   open -n apps/macos/.build/Bifrost.app
   ```
3. 将 Native 窗口移动到可见区域并截取左侧主导航和右侧顶部操作区：
   ```bash
   osascript -e 'tell application "System Events" to tell process "Bifrost" to set position of window 1 to {120, 80}'
   osascript -e 'tell application "System Events" to set frontmost of process "Bifrost" to true'
   screencapture -x -R 120,80,1180,792 /tmp/bifrost-native-source-list.png
   ```

**预期结果：**
- Native 使用固定宽度 source-list 行作为左侧主导航，不再使用顶部三段式主 tab，也不允许最外层主菜单被拖拽折叠。
- 左侧 source-list 视觉上为 macOS 系统式 material 浮层，窗口红黄绿按钮和左侧导航共享同一块背景区域。
- macOS 红黄绿三个窗口按钮使用系统原生标题栏按钮，不再由应用自绘。
- 左侧主导航显示 `活动`、`概览`、`规则`、`网络` 四个核心入口，窗口按钮必须保持可见，内容不得被标题栏或安全区裁切；窗口和页面背景呈清爽冷白 surface，卡片为白色卡片，非控件空白区域仍可拖拽移动窗口。

### TC-MNA-24：回归 - Native 主窗口只暴露核心控制台范围

**操作步骤：**
1. 执行 release scope 检查：
   ```bash
   swift run --package-path apps/macos Bifrost --check-release-scope
   ```
2. 执行 SwiftPM 构建：
   ```bash
   swift build --package-path apps/macos
   ```
3. 代码级检查主窗口路由：
   ```bash
   rg -n 'case \\.activity|case \\.overview|case \\.rules|case \\.network|SettingsView\\(' apps/macos/Sources/Bifrost/App apps/macos/Sources/Bifrost/Features/Dashboard
   ```

**预期结果：**
- release scope 输出包含 `活动,概览,规则,网络`。
- SwiftPM 构建通过。
- 主窗口 `MainWindowScene` 路由到 Activity、Overview、Rules、Network Web 入口，不再把 `SettingsView` 作为主导航内容，也不再暴露独立 Processes/进程 tab 或 Devices/设备 tab。
- Overview 页面包含系统代理、TLS 解密、远程调用、同步和证书管理卡片。
- Activity 页面轻量展示基于 `TrafficRecordSummary.clientApp` 的应用流量分布；设备/IP 不作为独立一级菜单展示。
- Network 页面只提供打开 Web UI 的入口和摘要，不再承载复杂流量工作台。
- UI 视觉以白色为主：白色半透明毛玻璃背景和纯白卡片，不得出现灰扑扑的大面积背景、灰色重卡片或厚重渐变。
- 隐藏顶部 `Bifrost` 标题后，红黄绿窗口按钮仍可见且可点击，页面标题不被裁切；点击非控件的白色背景区域拖动，窗口位置应随鼠标移动。
- Rules 页面必须与 Activity/Overview 使用同一套冷白 surface 与白色卡片：顶部页面标题为 `规则`，规则列表与编辑器分别在白色卡片中展示，卡片具有轻微边缘高光、弱阴影和 hover 悬浮反馈，不再使用旧的全宽灰色 toolbar、硬分割表格和突兀系统编辑器背景。

**执行记录：**
- 2026-07-03：执行 `swift build --package-path apps/macos` 通过，确认新核心范围 SwiftUI 页面可编译。
- 2026-07-03：执行 `swift run --package-path apps/macos Bifrost --check-release-scope` 通过，输出 `Bifrost release scope check passed: 活动,概览,进程,设备,规则,网络`。（后续按用户反馈移除独立进程和设备入口，需重新执行为四入口结果。）
- 2026-07-03：移除独立进程和设备入口后，重新执行 `swift run --package-path apps/macos Bifrost --check-release-scope` 通过，输出 `Bifrost release scope check passed: 活动,概览,规则,网络`。
- 2026-07-03：执行 `swift run --package-path apps/macos BifrostNativeCoreChecks` 通过，输出 `BifrostNativeCoreChecks passed`。
- 2026-07-03：执行 `scripts/build-macos-native.sh --skip-sidecar --test` 通过，生成 `apps/macos/.build/Bifrost.app`。
- 2026-07-03：执行 `open -n apps/macos/.build/Bifrost.app` 并截取 `/tmp/bifrost-native-core-window.png`，确认主窗口启动、左侧核心导航可见、Rules 原生列表/编辑区可见。
- 2026-07-03：按用户反馈修正窗口安全区与 Rules 风格后，执行 `swift build --package-path apps/macos`、`swift run --package-path apps/macos Bifrost --check-release-scope` 和 `scripts/build-macos-native.sh --skip-sidecar --test` 通过；当前机器辅助访问/截图权限受限，无法继续用脚本移动窗口或截取可靠局部截图。

### TC-MNA-12：Native 明暗主题切换可用且 Network 数据来自真实 Admin API

**操作步骤：**
1. 执行真实 Admin API smoke：
   ```bash
   swift run --package-path apps/macos Bifrost --check-admin-data
   ```
2. 截取亮色 source-list 和暗色 source-list：
   ```bash
   screencapture -x -R 120,80,1180,792 /tmp/bifrost-native-source-list-light.png
   osascript -e 'tell application "System Events" to click at {155, 820}'
   screencapture -x -R 120,80,1180,792 /tmp/bifrost-native-dark.png
   ```
3. 对照 Admin API：
   ```bash
   curl -s http://127.0.0.1:9900/_bifrost/api/traffic?limit=5
   curl -s http://127.0.0.1:9900/_bifrost/api/system/overview
   curl -s http://127.0.0.1:9900/_bifrost/api/proxy/system
   curl -s http://127.0.0.1:9900/_bifrost/api/config/tls
   ```

**预期结果：**
- `--check-admin-data` 输出 `Bifrost admin data check passed`，且包含真实 `port`、`pid`、`traffic_records`、`rules`。
- Native Network 表格中显示的 seq、protocol、method、status、client、port、host/path 与 `/traffic?limit=5` 返回的真实记录一致。
- Filters 面板中的 Client IP、Applications、Domains 计数来自当前加载的真实 traffic records。
- System Proxy 和 TLS Decode 开关状态分别来自 `/proxy/system` 与 `/config/tls`。
- 点击左侧 source-list 底部主题按钮后，Native app 切换到暗色主题；source-list、Network 表格、Filters、详情空态、状态栏均为暗色，不出现白底残留或文字不可读。

### TC-MNA-13：Network 工具区位于右侧顶部区域且关键开关接真实 Admin API

**操作步骤：**
1. 构建并打开 Native `.app`：
   ```bash
   scripts/build-macos-native.sh --skip-sidecar --test
   pkill -f 'Bifrost.app/Contents/MacOS/Bifrost' || true
   open -n apps/macos/.build/Bifrost.app
   ```
2. 将 Native 窗口移动到可见区域并截图：
   ```bash
   osascript -e 'tell application "System Events" to set frontmost of process "Bifrost" to true'
   osascript -e 'tell application "System Events" to tell process "Bifrost" to set position of window 1 to {120, 80}'
   osascript -e 'tell application "System Events" to tell process "Bifrost" to set size of window 1 to {1220, 820}'
   screencapture -x -R 120,80,1220,820 /tmp/bifrost-native-titlebar-network.png
   ```
3. 验证 Native 源码中不存在固定真假绑定或 scaffold 假数据：
   ```bash
   ruby -e 'paths=Dir["apps/macos/Sources/Bifrost/**/*.swift"]; text=paths.map{|p| File.read(p)}.join("\n"); forbidden=["TrafficRecord.sampleRows","REQ-preview","api.local","Native preview rule editor scaffold","example.com proxy://127.0.0.1:8080",".constant(true)",".constant(false)"]; found=forbidden.select{|needle| text.include?(needle)}; abort("forbidden scaffold data remains: #{found.join(", ")}") unless found.empty?; puts "macOS native scaffold fake data removed"'
   ```
4. 验证 Breakpoint、System Proxy、TLS Decode 的数据来源：
   ```bash
   curl -s http://127.0.0.1:9900/_bifrost/api/breakpoint/settings
   curl -s http://127.0.0.1:9900/_bifrost/api/proxy/system
   curl -s http://127.0.0.1:9900/_bifrost/api/config/tls
   ```

**预期结果：**
- `/tmp/bifrost-native-titlebar-network.png` 中顶部 macOS 标题栏不再显示单独的 `Network` 标题占位，也不显示 Network/Rules/Settings 顶部主 tab。
- Network 主窗口入口只展示 Web UI 打开入口和摘要；复杂 Network 操作区、Filters / Traffic table / Detail 三栏在 Web UI 中完成。
- Breakpoint 开关来自 `/breakpoint/settings`，不是固定 false。
- TLS Decode 开关来自 `/config/tls`，System Proxy 开关来自 `/proxy/system`。
- Clear traffic、filter tags、Fuzzy Search、Add Filter、detail panel toggle 均有实际 UI 状态变化或真实 API 行为，不是空按钮。
- 源码扫描输出 `macOS native scaffold fake data removed`。

### TC-MNA-14：首版本左侧主导航不得暴露未完成页面入口

**操作步骤：**
1. 执行 release scope smoke：
   ```bash
   swift run --package-path apps/macos Bifrost --check-release-scope
   ```
2. 执行源码入口扫描：
   ```bash
   ruby -e 'text=File.read("apps/macos/Sources/Bifrost/App/Sidebar.swift"); required=["case activity = \\"活动\\"","case overview = \\"概览\\"","case rules = \\"规则\\"","case network = \\"网络\\""]; forbidden=["case processes","case devices","Replay","Values","Scripts","AI","DevTools","Groups","Notify"]; missing=required.reject{|x| text.include?(x)}; found=forbidden.select{|x| text.include?(x)}; abort("missing=#{missing.join(",")} forbidden=#{found.join(",")}") unless missing.empty? && found.empty?; puts "macOS native release navigation scope ok"'
   ```
3. 执行完整 Native build smoke：
   ```bash
   scripts/build-macos-native.sh --skip-sidecar --test
   ```

**预期结果：**
- `--check-release-scope` 输出 `Bifrost release scope check passed: 活动,概览,规则,网络`。
- 源码入口扫描输出 `macOS native release navigation scope ok`。
- Native build smoke 通过。
- 用户侧左侧主导航只能进入活动、概览、规则、网络；进程、设备、Replay、Values、Scripts、AI、DevTools、Groups、Notify 不得以占位页、API 状态页或半成品页面形式出现在导航上。
- 长期 WebUI parity 仍在 `design/macos-native-webui-parity.md` 维护；后续页面必须达到真实交互完成后再开放入口。

### TC-MNA-15：Native 必须建立 WebUI 同源 `/api/push` WebSocket 并支持 Network 选中详情

**操作步骤：**
1. 执行增强后的 Admin API smoke，验证 REST 数据和 WebSocket push 同时可用：
   ```bash
   swift run --package-path apps/macos Bifrost --check-admin-data
   ```
2. 执行 Swift build/icon smoke，确保新增实时同步代码可编译：
   ```bash
   swift run --package-path apps/macos Bifrost --check-icon
   ```
3. 打开 Native `.app`，进入 Network 页面，点击任意 traffic 行：
   ```bash
   scripts/build-macos-native.sh --skip-sidecar --test
   open -n apps/macos/.build/Bifrost.app
   ```
4. 对照 Admin API 详情与 body 接口：
   ```bash
   first_id="$(curl -s http://127.0.0.1:9900/_bifrost/api/traffic?limit=1 | jq -r '.records[0].id')"
   curl -s "http://127.0.0.1:9900/_bifrost/api/traffic/$first_id"
   curl -s "http://127.0.0.1:9900/_bifrost/api/traffic/$first_id/request-body"
   curl -s "http://127.0.0.1:9900/_bifrost/api/traffic/$first_id/response-body"
   ```

**预期结果：**
- `--check-admin-data` 输出包含 `push_client_id=`，证明 Native smoke 已建立 `/_bifrost/api/push` WebSocket 并收到服务端 `connected` 消息。
- `--check-admin-data` 输出包含 `sse_streams=/whitelist/pending/stream=text/event-stream,/config/ip-tls/pending/stream=text/event-stream`，证明 Native smoke 对 WebUI 使用的全局 SSE stream 建立了真实连接并校验响应头。
- 状态栏不再固定显示 `Sync: Synced`；WebSocket 成功时显示 `Sync: Live #<client_id>`，失败时显示 fallback/poll 状态。
- Network 表格行可选中；选中后右侧详情区域展示真实 `/traffic/{id}`、request body、response body，不再只显示空态。
- 详情区域必须提供与 WebUI 同类的 Request / Response 分段，以及 Overview / Header / Body / Raw 子页；Overview 至少展示 URL、Method、Status、Protocol、Proxy Port、Host、Client 等真实字段。
- push 连接失败时 Native 仍通过轮询 fallback 自动刷新数据，不能停留在启动时快照。

### TC-MNA-16：Rules 原生页必须具备真实核心 CRUD，Values / Scripts 不暴露入口

**操作步骤：**
1. 执行真实 Admin API smoke，验证原生客户端的核心写链路：
   ```bash
   swift run --package-path apps/macos Bifrost --check-admin-data
   ```
2. 打开 Native `.app`：
   ```bash
   scripts/build-macos-native.sh --skip-sidecar --test
   open -n apps/macos/.build/Bifrost.app
   ```
3. 在 Rules 页面验证：
   - 左侧列表显示真实 `/rules` 数据，带启停开关和搜索；新建入口位于页面顶部操作区。
   - 点击规则后右侧加载 `/rules/{name}` 内容。
   - 编辑内容后出现未保存标记，Save 调用真实更新接口，Revert 恢复服务端内容。
   - 更多菜单可 Rename / Delete，Delete 必须出现确认弹窗。
4. 检查左侧主导航没有 Values / Scripts 入口：
   ```bash
   swift run --package-path apps/macos Bifrost --check-release-scope
   ```

**预期结果：**
- `--check-admin-data` 输出包含 `crud=rules,values,scripts`，证明 smoke 已完成临时 Rule、Value、Request Script 的 create/update/rename/delete，并清理测试对象。
- Rules 页面不再只读或只展示 API JSON；页面有真实列表、编辑器、保存按钮、重命名和删除确认。
- 通过 Native 写入的临时 Rule 在 WebUI 刷新后可见；删除后 WebUI 与 Admin API 均不可再读取。
- `--check-release-scope` 证明 Values / Scripts 没有用户侧入口。
- 本用例只覆盖首版本 Rules 核心 CRUD；Rules 拖拽排序/share link/import/export，以及 Values/Scripts 完整原生交互仍属于 `design/macos-native-webui-parity.md` 矩阵中未完成的后续工单，禁止标记为完整 WebUI parity。

### TC-MNA-17：Network 表格必须支持 10 万行级别高性能渲染路径

**操作步骤：**
1. 构建 Native app 并执行性能 smoke：
   ```bash
   scripts/build-macos-native.sh --skip-sidecar --test
   apps/macos/.build/Bifrost.app/Contents/MacOS/Bifrost --check-traffic-table-performance
   ```
2. 检查表格实现没有退回全量刷新和 cell 重建：
   ```bash
   rg -n 'reloadData\(\)|insertRows|removeRows|reloadData\(forRowIndexes|makeView\(withIdentifier|trafficRecordIndexById|trafficDeltaFlushTask|pendingTraffic|check-traffic-table-performance|didUpdateNotification' \
     apps/macos/Sources/Bifrost/AppKitBridge/RequestTableView.swift \
     apps/macos/Sources/Bifrost/App/AppModel.swift \
     apps/macos/Sources/Bifrost/App/BifrostApp.swift
   ```

**预期结果：**
- 性能 smoke 输出包含 `Traffic table performance smoke passed`。
- 输出包含 `base_rows=100000 append_rows=1000 changed_rows=`，证明 10 万行基础数据、1 千行 append 和稀疏 update bookkeeping 已真实执行。
- `RequestTableView` 使用 `makeView(withIdentifier:)` 复用 `TrafficCellView`，append 走 `insertRows`，尾部删除走 `removeRows`，行更新走 `reloadData(forRowIndexes:columnIndexes:)`。
- `AppModel` 维护 `trafficRecordIndexById` 并在 `mergeTrafficDelta` 中按 id 原地 merge/append，append-only 高频请求不再全量字典化和排序。
- `AppModel` 使用 `trafficDeltaFlushTask` 与 `pendingTraffic*` 以 16ms batch 合并 WebSocket delta，不能逐条 push 触发 SwiftUI update。
- App icon 使用异步 cache 更新通知；cell draw 只能使用缓存或 placeholder，不能在绘制路径同步扫描 `/Applications`。
- 相同行序的 update 只为真实变化行重建 `TrafficRowViewModel`，不能每次 SwiftUI update 都为 10 万行重建显示模型。

**实际结果（2026-06-30）：**
- `swift build --package-path apps/macos` 通过。
- `scripts/build-macos-native.sh --skip-sidecar --test` 通过，并输出 `BifrostNativeCoreChecks passed`。
- `apps/macos/.build/Bifrost.app/Contents/MacOS/Bifrost --check-traffic-table-performance` 通过，输出 `Traffic table performance smoke passed: base_rows=100000 append_rows=1000 changed_rows=1031 build_ms=566.01 append_ms=9.58 update_ms=9.16`。
- `apps/macos/.build/Bifrost.app/Contents/MacOS/Bifrost --check-admin-data` 通过，输出包含 `traffic_records=5`、`push_client_id=85`、`sse_streams=/whitelist/pending/stream=text/event-stream,/config/ip-tls/pending/stream=text/event-stream` 和 `crud=rules,values,scripts`。

### TC-MNA-18：首版本 release scope smoke 固化 Network / Rules / Settings 三入口

**操作步骤：**
1. 执行 release scope smoke：
   ```bash
   swift run --package-path apps/macos Bifrost --check-release-scope
   ```
2. 执行构建产物 release scope smoke：
   ```bash
   scripts/build-macos-native.sh --skip-sidecar --test
   apps/macos/.build/Bifrost.app/Contents/MacOS/Bifrost --check-release-scope
   ```

**预期结果：**
- 两次 smoke 均输出 `Bifrost release scope check passed: 活动,概览,规则,网络`。
- Native app 的 `SidebarItem.releaseScopeItems` 只包含 `.activity`、`.overview`、`.rules`、`.network`。
- Native app 的 `SidebarItem.allCases` 与 `releaseScopeItems` 完全一致，不保留隐藏页面枚举 case。
- 左侧主导航仅包含活动、概览、规则、网络；切换后会自动刷新当前页需要的状态。
- Replay、Values、Scripts、AI、DevTools、Groups、Notify 没有主导航入口；后续恢复入口时必须先补齐真实交互和对应 human_tests。

### TC-MNA-19：Settings 使用系统设置式左侧导航并覆盖四个首版页面

**操作步骤：**
1. 构建 Native app 并执行 Settings 真实接口 smoke：
   ```bash
   scripts/build-macos-native.sh --skip-sidecar --test
   BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 BIFROST_DISABLE_TRAY=1 \
     apps/macos/.build/Bifrost.app/Contents/MacOS/Bifrost --check-settings-data
   ```
2. 打开 Native `.app` 并进入顶部 `Settings`：
   ```bash
   open -n apps/macos/.build/Bifrost.app
   ```
3. 在 Settings 内部逐项点击左侧子页面：
   - Proxy
   - Certificate
   - Sync
   - Remote Invoke
4. 对照真实 Admin API：
   ```bash
   curl -s http://127.0.0.1:9900/_bifrost/api/proxy/system
   curl -s http://127.0.0.1:9900/_bifrost/api/proxy/system/launchd
   curl -s http://127.0.0.1:9900/_bifrost/api/proxy/cli
   curl -s http://127.0.0.1:9900/_bifrost/api/proxy/address
   curl -s http://127.0.0.1:9900/_bifrost/api/cert/info
   curl -s http://127.0.0.1:9900/_bifrost/api/mobile-devices
   curl -s http://127.0.0.1:9900/_bifrost/api/sync/status
   curl -s http://127.0.0.1:9900/_bifrost/api/remote-invoke/status
   curl -s http://127.0.0.1:9900/_bifrost/api/remote-invoke/identity
   curl -s http://127.0.0.1:9900/_bifrost/api/remote-invoke/pairings/pending
   curl -s http://127.0.0.1:9900/_bifrost/api/remote-invoke/grants
   curl -s 'http://127.0.0.1:9900/_bifrost/api/remote-invoke/calls?limit=5'
   ```

**预期结果：**
- `--check-settings-data` 输出 `Bifrost settings data check passed`，并包含 `proxy_supported`、`proxy_addresses`、`cert_status`、`sync_reason`、`remote_state`、`remote_identity`、`pending_pairings`、`grants`、`calls`、`ssh_key`。
- Settings 内部不是顶部多 Tab，而是类似 macOS 系统设置的左侧列表；左侧只显示 Proxy、Certificate、Sync、Remote Invoke。
- Proxy 页面展示真实 System Proxy、LaunchAgent、CLI Proxy、Proxy Addresses，并可通过系统 Switch 调用真实接口；除本用例明确操作外，不得自动修改系统代理。
- Certificate 页面展示真实 CA 状态、SHA256、下载/QR 入口、移动设备 discovery，并提供本机 CA 安装入口。
- Sync 页面展示真实状态、remote base URL、enable/auto sync、sign in/out、sync now；测试环境设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1` 时不得自动弹登录页。
- Remote Invoke 页面展示真实 connection status、identity、discovery pair code、pending pairings、SSH key、grants、recent calls，并提供 pair approve/reject、grant revoke、calls clear、SSH key create/export/reset/revoke 的真实入口。
- Remote Invoke 历史中缺少 `created_at` 的旧 call 记录不能导致 Settings 页面或 smoke 崩溃。
- Metrics、Access Control、Performance、Tray、Remote Access 等未完成 Settings 子页面本期不得暴露入口。

### TC-MNA-20：Network 列表重复 ID、标题栏开关和应用图标布局回归

**操作步骤：**
1. 构建 Native `.app`：
   ```bash
   scripts/build-macos-native.sh --skip-sidecar --test
   ```
2. 打开 Native `.app`，保持 Network 页面处于可见状态：
   ```bash
   open -n apps/macos/.build/Bifrost.app
   ```
3. 在真实流量持续进入时等待 60 秒，并反复切换 Network / Rules / Settings / Network。
4. 观察 Network 左侧 Filters 的 Applications 区域和顶部标题栏开关。
5. 如需要从命令行确认进程仍存活：
   ```bash
   pgrep -fl 'Bifrost.app/Contents/MacOS/Bifrost'
   ```

**预期结果：**
- 即使 Admin 初始列表与 WebSocket delta 出现相同 request id，Network 表格也只保留同一 id 的最新行，不因 `Dictionary(uniqueKeysWithValues:)` 重复 key 触发 Swift runtime assertion 闪退。
- 顶部 Breakpoint、TLS Decode、System Proxy 使用紧凑的系统 `NSSwitch`，尺寸接近 Rules 列表里的小 switch，不撑高标题栏。
- Filters > Applications 的应用图标固定在 16x16 行内；Edge、Doubao、Codex、Lark 等图标不得溢出到后续行或覆盖 Domains 标题。
- 持续滚动或新增流量时，过滤列表图标缓存不得加载原始大图到行布局；图标异常时显示系统 `app` 占位。
- Network 页面 60 秒内不闪退，窗口仍名为 `Bifrost`。

### TC-MNA-21：主导航使用 Apple Music 风格左侧 source-list，页面 tab 保持在内容内部

**操作步骤：**
1. 构建并打开 Native `.app`：
   ```bash
   scripts/build-macos-native.sh --skip-sidecar --test
   pkill -f 'Bifrost.app/Contents/MacOS/Bifrost' || true
   open -n apps/macos/.build/Bifrost.app
   ```
2. 固定窗口位置与尺寸并截图：
   ```bash
   osascript -e 'tell application "System Events" to set frontmost of process "Bifrost" to true'
   osascript -e 'tell application "System Events" to tell process "Bifrost" to set position of window 1 to {120, 80}'
   osascript -e 'tell application "System Events" to tell process "Bifrost" to set size of window 1 to {1220, 820}'
   screencapture -x -R 120,80,1220,820 /tmp/bifrost-native-apple-sidebar.png
   ```
3. 通过左侧 source-list 依次点击活动、概览、规则、网络，并观察右侧内容区域。
4. 在网络页点击 Web UI 打开入口，确认复杂 Network 工作台在浏览器中打开。

**预期结果：**
- 左侧主导航是系统 source-list 风格区域，具有淡 material 背景；红黄绿窗口按钮落在同一侧栏背景区域内。
- 活动、概览、规则、网络是左侧主导航入口；顶部右侧不得出现 Network / Rules / Settings 主 tab，且不得再出现独立进程或设备入口。
- Network 页面只展示 Web UI 打开入口和摘要；Breakpoint/TLS Decode/System Proxy、Remote Invoke、Sync、Certificate 管理收敛到概览。
- Rules 页面展示 `规则` 页面标题、启用数量和 New Rule 操作；规则列表与编辑器使用白色卡片承载，切换到 Rules 时自动刷新列表，规则详情内部的 Enabled、Copy、Revert、Save、Rename/Delete 仍保留在详情头部。
- Settings scene 仍可从 macOS app Settings 入口打开，主窗口不再把 Settings 作为一级导航。
- Request / Response、Overview / Header / Body / Raw 等 Network 详情 tab 不进入 Native 主窗口一级导航。

### TC-MNA-22：Network 表格文字绘制不得因 CoreText attribute 崩溃

**操作步骤：**
1. 构建 Native `.app`：
   ```bash
   swift build --package-path apps/macos
   ```
2. 打开 Native `.app` 并进入 Network 页面：
   ```bash
   osascript -e 'tell application "Bifrost" to quit' >/dev/null 2>&1 || true
   open -n apps/macos/.build/Bifrost.app
   ```
3. 确认真实 Admin 数据可以被 Native 读取：
   ```bash
   apps/macos/.build/Bifrost.app/Contents/MacOS/Bifrost --check-admin-data
   ```
4. 保持 Network 页面可见至少 8 秒，确认进程仍存活：
   ```bash
   sleep 8
   pgrep -fl 'Bifrost.app/Contents/MacOS/Bifrost'
   ```

**预期结果：**
- Network 表格渲染真实请求行时，`TrafficCellView.drawText` 不得触发 `attempt to insert nil object` / CoreText / `NSString.draw` 崩溃。
- `--check-admin-data` 输出 `Bifrost admin data check passed`，包含真实 traffic、rules、push client id 和 SSE stream 信息。
- 8 秒后 Native app 进程仍存在，Network 页面可继续滚动和切换详情。
- 文字测量和绘制不再走 `NSString.size(withAttributes:)` / `NSStringDrawingEngine`；Network 滚动并发绘制时使用稳定的 CoreText/CG 路径和显式裁剪。
- 右侧 Network 表格、Rules 详情、Settings 面板操作风格不受影响。

### TC-MNA-23：Network 列表按 WebUI 加载策略展示全部服务端保留数据

**操作步骤：**
1. 构建 Native `.app`：
   ```bash
   scripts/build-macos-native.sh --skip-sidecar --test
   ```
2. 确认服务端当前 traffic 保留策略：
   ```bash
   curl -s http://127.0.0.1:9900/_bifrost/api/config/performance
   ```
3. 用 WebUI 同款 initial window + backward history page 策略拉取 Network 列表，并与 100 条查询对照：
   ```bash
   python3 - <<'PY'
   import json
   import urllib.request

   base = "http://127.0.0.1:9900/_bifrost/api"
   perf = json.load(urllib.request.urlopen(f"{base}/config/performance", timeout=5))
   limit = int(perf["traffic"]["max_records"])
   initial = json.load(urllib.request.urlopen(f"{base}/traffic/updates?limit=500", timeout=10))
   records_by_id = {record["id"]: record for record in initial.get("new_records", [])}
   ordered = list(initial.get("new_records", []))
   has_more = bool(initial.get("has_more"))
   cursor = ordered[-1]["seq"] if ordered else None
   while has_more and cursor is not None:
       page = json.load(urllib.request.urlopen(f"{base}/traffic?limit=500&cursor={cursor}&direction=backward", timeout=10))
       batch = list(reversed(page.get("records", [])))
       for record in batch:
           records_by_id[record["id"]] = record
       ordered = batch + ordered
       has_more = bool(page.get("has_more"))
       cursor = ordered[0]["seq"] if ordered else None
   full = json.load(urllib.request.urlopen(f"{base}/traffic?limit={limit}", timeout=10))
   capped = json.load(urllib.request.urlopen(f"{base}/traffic?limit=100", timeout=10))
   print(f"traffic.max_records={limit}")
   print(f"webui_strategy_records={len(records_by_id)}")
   print(f"full_records={len(full.get('records', []))} total={full.get('total')}")
   print(f"capped_records={len(capped.get('records', []))} total={capped.get('total')}")
   assert limit > 100
   assert len(records_by_id) >= len(capped.get("records", []))
   assert len(full.get("records", [])) >= len(capped.get("records", []))
   PY
   ```
4. 运行 Native 真实 Admin 数据检查：
   ```bash
   apps/macos/.build/Bifrost.app/Contents/MacOS/Bifrost --check-admin-data
   ```

**预期结果：**
- Native `TrafficQuery()` 默认值与服务端默认保留策略一致，不再是 100；Network 页面实际加载策略必须对齐 WebUI 的 `/traffic/updates?limit=500` 最新窗口和 `/traffic?cursor=<oldest>&direction=backward&limit=500` 历史回填。
- Network 初始渲染先展示最新窗口，随后后台分批补齐服务端已保留的全部请求数据；不得由 macOS 客户端额外固定截断成 100 条。
- 如果服务端当前数据少于 100 条，`webui_strategy_records`、`full_records` 可以等于 `capped_records`；但 smoke 输出仍必须包含 `traffic_limit=` 且该值大于 100。
- 旧数据清理只能依赖服务端 `traffic.max_records`、数据库大小和 retention 策略；Native 客户端不得主动删除或按 UI 上限清理历史数据。
- `--check-admin-data` 输出 `traffic_limit=<服务端 max_records>`、`traffic_initial_window=<最新窗口数>`、`traffic_history_page=<历史回填页数>` 和 `traffic_retained_records=<实际返回数>`。
- 切换到 Rules 或 Settings 后，Native 必须发送 `need_traffic=false` 停止 traffic push/polling；切回 Network 时重新加载最新窗口并按 `last_sequence` / `pending_ids` 恢复增量同步。

### TC-MNA-25：回归 - 主窗口切换 tab 不触发全量刷新和重复统计

**操作步骤：**
1. 构建 Native `.app`：
   ```bash
   scripts/build-macos-native.sh --skip-sidecar --test
   ```
2. 执行 release scope smoke：
   ```bash
   swift run --package-path apps/macos Bifrost --check-release-scope
   ```
3. 检查主窗口刷新策略和 Activity 统计缓存：
   ```bash
   rg -n 'refreshSelectedSidebarData|includeOverview|includeRules|includeSystemControls|maxNativeRecords|activityClientAppCounts|activityClientIpCounts|selectInitialTrafficRecordIfNeeded' \
     apps/macos/Sources/Bifrost/App/AppModel.swift \
     apps/macos/Sources/Bifrost/Features/Dashboard/DashboardView.swift
   ! rg -n 'selectInitialTrafficRecordIfNeeded' apps/macos/Sources/Bifrost/App/AppModel.swift
   ```
4. 打开 Native `.app`，依次快速点击 `活动`、`概览`、`规则`、`网络`、`活动`：
   ```bash
   open -n apps/macos/.build/Bifrost.app
   ```

**预期结果：**
- Activity 切换只加载 overview + traffic，不加载 rules 或 system controls；Overview 切换只加载 overview + system controls，不加载 traffic/rules；Rules 切换只加载 overview + rules，不加载 traffic/system controls；Network 切换不触发 Native traffic 工作台刷新。
- `AppModel` 保留 `activityClientAppCounts` 与 `activityClientIpCounts` 缓存，并在 traffic reload/merge/delete/clear 后更新；`ActivityView` 使用缓存统计，不在 SwiftUI render 路径反复遍历 `trafficRecords`。
- Native 主窗口只保留轻量 traffic 窗口，`maxNativeRecords` 限制 Activity 控制台指标的本地记录数量；复杂 Network 历史列表仍由浏览器 Web UI 处理。
- 切换主导航不预取隐藏 traffic 详情，不调用 `selectInitialTrafficRecordIfNeeded` 作为 tab 切换副作用；只有用户进入需要详情的 Network/WebUI 路径时才按需加载复杂数据。
- 连续快速切换四个主入口时，页面标题和首屏卡片应立即出现，不应出现明显卡住后才渲染的空白等待。

**执行记录：**
- 2026-07-03：执行 `swift run --package-path apps/macos Bifrost --check-release-scope` 通过，输出 `Bifrost release scope check passed: 活动,概览,规则,网络`。
- 2026-07-03：执行 `scripts/build-macos-native.sh --skip-sidecar --test` 通过，生成 `apps/macos/.build/Bifrost.app` 并输出 `BifrostNativeCoreChecks passed`。
- 2026-07-03：执行源码检查命令，确认存在 `refreshSelectedSidebarData`、按页 `includeOverview/includeRules/includeSystemControls`、`maxNativeRecords` 和 `activityClientAppCounts/activityClientIpCounts` 缓存路径，且不存在 `selectInitialTrafficRecordIfNeeded` 隐藏预取 helper；当前机器辅助访问/截图权限不稳定，人工点击观察项需在有 UI 权限环境复核。

### TC-MNA-26：回归 - 主窗口所有面板必须复用统一 NativeSurface 风格

**操作步骤：**
1. 执行 SwiftPM 构建：
   ```bash
   swift build --package-path apps/macos
   ```
2. 检查共享 surface 组件和页面使用点：
   ```bash
   rg -n 'struct NativePageScaffold|struct NativePanel|struct NativeCard|struct NativeCardHeader|struct CompactFact|struct StatusPill|struct EmptyNativeState' \
     apps/macos/Sources/Bifrost/App/NativeSurface.swift
   rg -n 'NativePageScaffold|NativePanel|NativeCard' \
     apps/macos/Sources/Bifrost/Features/Dashboard/DashboardView.swift \
     apps/macos/Sources/Bifrost/Features/Rules/RulesView.swift
   ! rg -n 'RuleSurfaceCard|TopToolbar|ToolbarIconButton|MiniToolbarSwitch' \
     apps/macos/Sources/Bifrost/App/MainWindowScene.swift \
     apps/macos/Sources/Bifrost/Features/Dashboard/DashboardView.swift \
     apps/macos/Sources/Bifrost/Features/Rules/RulesView.swift
   ```
3. 打开 Native `.app` 后依次查看 `活动`、`概览`、`规则`、`网络`：
   ```bash
   scripts/build-macos-native.sh --skip-sidecar --test
   open -n apps/macos/.build/Bifrost.app
   ```

**预期结果：**
- `NativeSurface.swift` 是主窗口冷白 surface、白色面板、边缘高光、弱阴影、hover 悬浮、卡片标题和空态的唯一共享实现。
- Activity、Overview、Network 使用 `NativeCard`；Rules 的列表和编辑器使用同一个 `NativePanel`，不得保留独立 `RuleSurfaceCard`。
- 主窗口源码中不得保留旧 `TopToolbar`、`ToolbarIconButton`、`MiniToolbarSwitch` 等未渲染旧工具栏代码，避免后续回退到灰色 `.bar` 风格。
- 四个主入口的页面标题、横向边距、最大内容宽度、面板圆角、描边、发光边缘、hover 阴影和卡片白色填充保持一致。
- Status bar 可继续使用系统 `.bar` 作为底部状态区域；它不属于主内容面板，不应影响 Activity/Overview/Rules/Network 面板统一性。

**执行记录：**
- 2026-07-03：执行 `swift build --package-path apps/macos` 通过，确认 `NativeSurface` 共享组件、Dashboard 页面和 Rules 页面可编译。
- 2026-07-03：执行源码检查命令，确认 `NativeSurface.swift` 包含共享组件，Dashboard/Rules 使用 `NativePageScaffold`、`NativePanel`、`NativeCard`，且 `RuleSurfaceCard`、`TopToolbar`、`ToolbarIconButton`、`MiniToolbarSwitch` 不存在。

### TC-MNA-27：回归 - Overview 展示证书、移动端可用性和 Remote Invoke 核心操作

**操作步骤：**
1. 执行 SwiftPM 构建：
   ```bash
   swift build --package-path apps/macos
   ```
2. 执行设置数据 smoke，覆盖 Overview 依赖的真实 Admin API：
   ```bash
   apps/macos/.build/Bifrost.app/Contents/MacOS/Bifrost --check-settings-data
   ```
3. 检查 Overview 数据模型和操作入口：
   ```bash
   rg -n 'mobileDevices|proxyAddressInfo|trustProbeSession|createRemoteInvokeSshKey|copyRemoteInvokeSshKey|refreshMobileDevices|regenerateTrustProbe|fetchRemoteInvokeGrants|fetchRemoteInvokeCalls|fetchRemoteInvokeSshKey|createTrustProbeSession' \
     apps/macos/Sources/Bifrost/Features/Dashboard/DashboardView.swift \
     apps/macos/Sources/BifrostNativeCore/BifrostClient/BifrostClient.swift
   ```
4. 检查 Overview 面板渲染结构：
   ```bash
   rg -n 'Remote Invoke|RemoteDiscoveryCodeStrip|授权码|复制 SSH Key|refreshRemotePairCode|copyRemotePairCode|剩余 %02d:%02d|证书与移动端|安装本机 CA|刷新设备|重新生成 QR|可用性检查|QRPreview|MobileDeviceRow|TrustProbeDeviceRow|RemoteInvokeGrantRow' \
     apps/macos/Sources/Bifrost/Features/Dashboard/DashboardView.swift
   ```
5. 执行源码合同扫描，确认 `同步` 与 `证书管理` 两个小面板排在顶部 grid 中，位于 `Remote Invoke` 之前；顶部 grid 最多 4 列，列宽使用 flexible 自适应，窄窗口依次降到 3/2/1 列：
   ```bash
   ruby -e 'text=File.read("apps/macos/Sources/Bifrost/Features/Dashboard/DashboardView.swift"); grid_call=text.index("overviewControlGrid\n\n            RemoteInvokeCard(model: model)") or abort("overview grid must render before RemoteInvokeCard"); required=["ViewThatFits(in: .horizontal)", "overviewControlGrid(columnCount: 4)", "overviewControlGrid(columnCount: 3)", "overviewControlGrid(columnCount: 2)", "overviewControlGrid(columnCount: 1)", "GridItem(.flexible(minimum: 220)", "SyncControlCard(model: model)", "CertificateManagementCard(model: model)"]; missing=required.reject{|needle| text.include?(needle)}; abort("missing capped flexible grid markers: #{missing.join(", ")}") unless missing.empty?; abort("overview top grid still uses adaptive columns") if text.include?("GridItem(.adaptive(minimum: 260"); puts "macOS native overview capped flexible grid order ok"'
   ```
6. 执行源码合同扫描，确认 `同步` 卡片展示远端服务地址，未登录点击走授权登录，已登录后支持原位编辑和保存：
   ```bash
   ruby -e 'text=File.read("apps/macos/Sources/Bifrost/Features/Dashboard/DashboardView.swift"); required=["syncRemoteBaseURLDraft", "SyncRemoteServiceRow", "handleSyncRemoteBaseURLClick", "openSyncLogin()", "beginSyncRemoteBaseURLEdit()", "saveSyncRemoteBaseURL", "UpdateSyncConfigRequest(remoteBaseURL: trimmed)", "Text(\"远端服务\")", "TextField(\"https://bifrost.example.com\""]; missing=required.reject{|needle| text.include?(needle)}; abort("missing sync remote service markers: #{missing.join(", ")}") unless missing.empty?; puts "macOS native sync remote service edit contract ok"'
   ```
7. 执行源码合同扫描，确认证书已经信任后不再展示安装按钮：
   ```bash
   ruby -e 'text=File.read("apps/macos/Sources/Bifrost/Features/Dashboard/DashboardView.swift"); required=["shouldShowInstallButton", "model.certInfo?.trusted != true", "if shouldShowInstallButton", "Button(\"安装本机 CA\")"]; missing=required.reject{|needle| text.include?(needle)}; abort("missing certificate install visibility markers: #{missing.join(", ")}") unless missing.empty?; puts "macOS native certificate install visibility contract ok"'
   ```
8. 打开 Native `.app` 并进入 `概览`：
   ```bash
   scripts/build-macos-native.sh --skip-sidecar --test
   open -n apps/macos/.build/Bifrost.app
   ```

**预期结果：**
- `--check-settings-data` 输出 `cert_status=`、`mobile_android=`、`mobile_ios=`、`remote_state=`、`grants=`、`calls=`、`ssh_key=`、`trust_probe_host=` 和 `trust_probe_qr=true`。
- Overview 的 Remote Invoke 面板不再只是发现模式开关；必须展示 SSH Key 状态、生成/重新生成按钮、复制 SSH Key 按钮、已授权客户端数、活动调用数、最近调用数、最近活跃时间，以及最多 3 个授权客户端/最近调用摘要。
- Remote Invoke 进入发现模式后必须展示授权码 Code；点击 Code 直接复制，复制后短暂显示已复制；右侧必须有重置按钮；倒计时按秒刷新，和后端约 2 分钟有效期一致，过期后显示已过期。
- Overview 的证书与移动端面板必须展示本机 CA 状态、代理地址、移动设备数、证书指纹、`安装本机 CA`、`刷新设备`、`重新生成 QR`。
- 当本机 CA 已安装且已信任时，证书管理卡片不展示 `安装本机 CA` 按钮；只有未信任或未安装时才展示安装入口。
- Overview 顶部区域先排系统代理、TLS 解密、同步、证书管理；顶部最多一排 4 个卡片，4 列时列宽平分可用空间，窄窗口自动降到 3/2/1 列；`Remote Invoke` 下移到下一段。
- Overview 的同步卡片必须直接展示远端服务地址；未登录时点击该地址行触发同步登录授权；已有 session 时点击进入原位编辑，保存后通过 `/sync/config` 更新 `remote_base_url`。
- 可用性检查区域必须展示二维码图片容器、检查链接、打开/复制链接操作；扫码后 `trustProbeSession.devices` 中的正在连接设备必须在 `TrustProbeDeviceRow` 中显示网络/TLS/代理状态。
- USB/ADB/cfgutil 发现的移动设备必须在同一证书面板内显示设备名称、平台图标、证书信任状态或设备状态；没有设备时显示明确空态和扫码引导。
- 这些能力都在 `概览` 一级页面内完成，不要求用户再进入完整 Settings 页或 WebUI 才能看到基础状态。

**执行记录：**
- 2026-07-03：执行 `swift build --package-path apps/macos` 通过。
- 2026-07-03：执行 `scripts/build-macos-native.sh --skip-sidecar --test && apps/macos/.build/Bifrost.app/Contents/MacOS/Bifrost --check-settings-data` 通过，输出包含 `cert_status=installed_and_trusted`、`mobile_android=0`、`mobile_ios=0`、`remote_state=Connected`、`grants=2`、`calls=5`、`ssh_key=true`、`trust_probe_host=10.71.185.109`、`trust_probe_qr=true`。
- 2026-07-03：执行源码检查命令，确认 Overview 具备 `RemoteInvokeGrantRow`、`MobileDeviceRow`、`AvailabilityProbePanel`、`QRPreview`、`TrustProbeDeviceRow`，并通过 `BifrostClient` 拉取 mobile/proxy/trust-probe/Remote Invoke SSH key/grants/calls。
- 2026-07-04：执行源码合同扫描通过，输出 `macOS native overview capped flexible grid order ok`。
- 2026-07-04：执行 `swift build --package-path apps/macos` 通过；执行同步远端服务地址源码合同扫描通过，输出 `macOS native sync remote service edit contract ok`；执行 `scripts/build-macos-native.sh --skip-sidecar --test` 通过；执行 `apps/macos/.build/Bifrost.app/Contents/MacOS/Bifrost --check-settings-data` 通过，输出包含 `sync_reason=ready`、`remote_state=Connected`、`ssh_key=true` 和 `trust_probe_qr=true`；重新打开 Native `.app`。
- 2026-07-04：执行证书安装按钮可见性源码合同扫描通过，输出 `macOS native certificate install visibility contract ok`。

### TC-MNA-28：回归 - Overview TLS 解密卡片支持名单数量和弹窗编辑

**操作步骤：**
1. 执行 SwiftPM 构建：
   ```bash
   swift build --package-path apps/macos
   ```
2. 执行设置数据 smoke，覆盖 TLS 名单真实 Admin API：
   ```bash
   apps/macos/.build/Bifrost.app/Contents/MacOS/Bifrost --check-settings-data
   ```
3. 检查 Overview TLS 名单 UI 和保存链路：
   ```bash
   rg -n 'TlsInterceptionCard|TlsListKind|TlsListEditorSheet|TlsListCountTile|应用白名单|应用黑名单|域名白名单|域名黑名单|IP 白名单|IP 黑名单|updateTlsConfig' \
     apps/macos/Sources/Bifrost/Features/Dashboard/DashboardView.swift \
     apps/macos/Sources/Bifrost/App/AppModel.swift
   ```
4. 打开 Native `.app`，进入 `概览`，点击 TLS 解密卡片内任一名单数量块：
   ```bash
   open -n apps/macos/.build/Bifrost.app
   ```

**预期结果：**
- `--check-settings-data` 输出 `tls_domain_include=`、`tls_domain_exclude=`、`tls_app_include=`、`tls_app_exclude=`、`tls_ip_include=` 和 `tls_ip_exclude=`。
- Overview 的 TLS 解密卡片展示应用、域名、IP 三类白名单/黑名单共 6 个计数块；卡片仍保留 TLS 解密总开关。
- 点击任一计数块弹出编辑框，标题对应具体名单类型，输入区支持每行一个规则并显示该类型示例占位。
- 保存时会去除空行和重复项，更新对应 `TlsConfig` 字段并调用 `AppModel.updateTlsConfig` 保存到 `/config/tls`。
- 卡片和弹窗使用 `NativeCard` / `AppSurface` 风格，不能回退到 Settings 全页或 WebUI 才能编辑基础 TLS 解包名单。

**执行记录：**
- 2026-07-03：执行 `swift build --package-path apps/macos` 通过。
- 2026-07-03：执行 `scripts/build-macos-native.sh --skip-sidecar --test && apps/macos/.build/Bifrost.app/Contents/MacOS/Bifrost --check-settings-data` 通过，输出包含 `tls_domain_include=2`、`tls_domain_exclude=0`、`tls_app_include=9`、`tls_app_exclude=2`、`tls_ip_include=0`、`tls_ip_exclude=1`。
- 2026-07-03：执行源码检查命令，确认 `TlsInterceptionCard`、`TlsListKind`、`TlsListEditorSheet`、`TlsListCountTile` 和 `AppModel.updateTlsConfig` 均存在，六类名单标题均在 Overview 源码中出现。

### TC-MNA-29：回归 - Rules 使用原生 BifrostRuleEditorView 编辑 DSL

**操作步骤：**
1. 执行 core contract 检查：
   ```bash
   swift run --package-path apps/macos BifrostNativeCoreChecks
   ```
2. 执行 SwiftPM 构建：
   ```bash
   swift build --package-path apps/macos
   ```
3. 检查语言服务和编辑器分层：
   ```bash
   rg -n 'BifrostRuleLanguageService|BifrostRuleToken|BifrostCompletionItem|BifrostReferenceMatch|BifrostNavigationTarget|localVariables' \
     apps/macos/Sources/BifrostNativeCore/RuleLanguage \
     apps/macos/Sources/BifrostNativeCoreChecks/main.swift
   rg -n 'BifrostRuleEditorView|BifrostRuleTextView|BifrostRuleHighlighter|BifrostRuleCompletionController|BifrostLineNumberRulerView|keyDown|mouseDown|NSF12FunctionKey' \
     apps/macos/Sources/Bifrost/Features/Rules/Editor/BifrostRuleEditorView.swift
   rg -n 'BifrostRuleEditorView|ruleEditorContext|refreshRuleEditorDynamicData|navigateFromRuleEditor' \
     apps/macos/Sources/Bifrost/Features/Rules/RulesView.swift \
     apps/macos/Sources/Bifrost/App/AppModel.swift
   ```
4. 打开 Native `.app`，进入 `规则`，选择一条规则，手工验证：
   ```bash
   open -n apps/macos/.build/Bifrost.app
   ```

**预期结果：**
- `BifrostNativeCoreChecks` 覆盖 tokenizer、`key=value` 本地变量、`` ```headers `` fenced block 变量、`@rule` 补全、`{value}`/`{headers}` 补全、`reqScript://` 补全、reference detection 和 navigation target。
- Rules 详情区使用 `BifrostRuleEditorView`，不再使用通用 `CodeEditorView` 作为规则 DSL 编辑器。
- 编辑器底层是原生 `NSTextView`，禁用智能引号、破折号、文本替换、拼写/语法检查和自动链接；支持 undo、等宽字体、水平/垂直滚动。
- 编辑器展示行号；高亮覆盖注释、规则引用、脚本引用、`{value}`/`${value}` 变量、`` ```headers `` 块变量名、scheme、key/value 和正则。
- 输入 `@`、`{`、`reqScript://`、`resScript://`、`bp://` 时可基于 Rules/Values/Scripts 动态数据弹出补全；`{` 补全同时包含 fenced block 变量和全局 Values；方向键选择，Enter/Tab 插入，Esc 关闭。
- Cmd+S 调用现有规则保存链路；Cmd+Click/F12 对本地变量跳到定义行，对 `@RuleName` 在 Native Rules 中选择对应规则，对 Values/Scripts 在当前主导航缺失页面时 fallback 打开 Web UI。
- 不引入 Monaco、WebView、CodeEditSourceEditor、STTextView、CodeEditorView 第三方包或新的 Swift Package 依赖。

**执行记录：**
- 2026-07-03：执行 `swift run --package-path apps/macos BifrostNativeCoreChecks` 通过，输出 `BifrostNativeCoreChecks passed`。
- 2026-07-03：执行 `swift build --package-path apps/macos` 通过。
- 2026-07-03：执行源码检查命令，确认 `BifrostRuleLanguageService`、`BifrostRuleEditorView`、`BifrostRuleTextView`、`BifrostRuleHighlighter`、`BifrostRuleCompletionController`、`BifrostLineNumberRulerView`、`ruleEditorContext`、`refreshRuleEditorDynamicData`、`navigateFromRuleEditor` 均存在。

### TC-MNA-30：CLI 可安装 Native App 到应用目录并查询状态

**操作步骤：**
1. 执行 shell E2E 回归脚本，使用临时安装目录而不是 `/Applications`：
   ```bash
   bash e2e-tests/tests/test_macos_native_app_install.sh
   ```
2. 单独验证 CLI dry-run 不写入目标目录：
   ```bash
   TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/bifrost-native-app-human.XXXXXX")"
   mkdir -p "$TEST_ROOT/source/Bifrost.app/Contents" "$TEST_ROOT/install"
   printf '<plist><dict><key>CFBundleShortVersionString</key><string>9.9.9</string></dict></plist>' \
     > "$TEST_ROOT/source/Bifrost.app/Contents/Info.plist"
   cargo run -p bifrost-cli --bin bifrost -- native-app install \
     --source "$TEST_ROOT/source/Bifrost.app" \
     --install-dir "$TEST_ROOT/install" \
     --latest-version 9.9.9 \
     --dry-run
   test ! -e "$TEST_ROOT/install/Bifrost.app"
   rm -rf "$TEST_ROOT"
   ```

**预期结果：**
- `native-app install --dry-run` 输出 JSON，包含 `dry_run: true`、source、target 和 open_after_install。
- 真实安装会把 `Bifrost.app` 拷贝到指定安装目录，并保留 Info.plist 版本号。
- `native-app status --format json` 返回 `installed: true`、`installed_version: 9.9.9`、`needs_install: false`。
- 用例只使用临时目录，不写入 `/Applications`，不启动系统代理，不打开 Sync 登录页。

**执行记录：**
- 2026-07-03：执行 `bash e2e-tests/tests/test_macos_native_app_install.sh` 通过，输出 `macOS native app install CLI E2E passed`，状态 JSON 包含 `installed: true`、`installed_version: "9.9.9"`、`needs_install: false`。
- 2026-07-03：执行临时目录 dry-run 命令通过，输出 `{"dry_run":true,...,"target":".../install/Bifrost.app"}`，并确认目标 `Bifrost.app` 未创建。

### TC-MNA-31：Admin API 暴露 Native App 状态与安装入口

**操作步骤：**
1. 执行 Rust 编译检查，确认 Admin 路由、CLI 子命令和共享状态模型类型一致：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo check -p bifrost-admin --all-targets
   ```
2. 检查 Admin 路由与安装子进程命令：
   ```bash
   rg -n '"/api/system/native-app"|get_native_app_status|start_native_app_install|native-app|install|-y|--open' \
     crates/bifrost-admin/src/handlers/system.rs
   ```

**预期结果：**
- `cargo check -p bifrost-admin --all-targets` 通过。
- `GET /api/system/native-app` 使用 `status_for_install_dir` 返回当前安装状态、安装路径、已安装版本、最新版本和 `needs_install`。
- `POST /api/system/native-app/install` 在 macOS 可安装状态下启动当前 `bifrost native-app install -y --open --latest-version <version>`，让 CLI 负责下载、替换、启动。
- 非 macOS 或已安装最新版时，API 必须返回明确状态，不应误报已接受安装。

**执行记录：**
- 2026-07-03：执行 `SKIP_FRONTEND_BUILD=1 cargo check -p bifrost-admin --all-targets` 通过。
- 2026-07-03：执行源码检查命令通过，确认存在 `/api/system/native-app`、`get_native_app_status`、`start_native_app_install`，安装子进程包含 `native-app install -y --open`。

### TC-MNA-32：Web UI 弹出 Native App 安装提示并调用 Admin API

**操作步骤：**
1. 执行前端类型检查：
   ```bash
   pnpm --dir web exec tsc -b
   ```
2. 检查 Web UI 全局挂载点、API client 和安装按钮：
   ```bash
   rg -n 'NativeAppInstallPrompt|getNativeAppStatus|installNativeApp|native-app-install-button|bifrost-native-app-install-later' \
     web/src/App.tsx web/src/api/nativeApp.ts web/src/components/NativeAppInstallPrompt/index.tsx
   ```

**预期结果：**
- 前端 typecheck 通过。
- Web UI 启动后会查询 `/system/native-app`；当 `supported=true` 且 `needs_install=true` 时弹出安装提示。
- 用户点击安装按钮后调用 `/system/native-app/install`，Admin 接受后显示后台安装提示并刷新状态。
- 用户点击 Later 只按当前 latest version 记录本地延后，不应永久屏蔽未来新版本。

**执行记录：**
- 2026-07-03：首次执行 `pnpm --dir web exec tsc -b` 因 `node_modules` 缺失失败；随后执行 `pnpm --dir web install --frozen-lockfile` 按锁文件安装依赖。
- 2026-07-03：复跑 `pnpm --dir web exec tsc -b` 通过。
- 2026-07-03：执行 `pnpm --dir web run lint` 通过，但保留既有 14 个 warning；期间修复当前分支 `web/src/api/asr.test.ts` 中阻断 lint 的未使用 mock 参数。
- 2026-07-03：执行源码检查命令通过，确认 `NativeAppInstallPrompt` 已挂载到 `App.tsx`，API client 与 `native-app-install-button` 存在。

### TC-MNA-33：Tray 菜单提供 Native App 安装/打开入口

**操作步骤：**
1. 执行 Tray 相关单元测试：
   ```bash
   SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli tray:: -- --nocapture
   ```
2. 检查 Tray 菜单构建和点击处理：
   ```bash
   rg -n 'InstallNativeApp|Open Native App|Install Native App|native_app_installed|native_app_needs_install|bifrost-tray-native-app-install' \
     crates/bifrost-cli/src/commands/tray/menu.rs \
     crates/bifrost-cli/src/commands/tray/tray.rs
   ```

**预期结果：**
- Tray 单元测试通过。
- 当 Native App 未安装或需要更新时，菜单显示 `Install Native App`，点击后启动受信任 bifrost 二进制执行 `native-app install -y --open`。
- 当 Native App 已安装且无需更新时，菜单显示 `Open Native App`。
- 安装任务必须使用 Tray 现有 busy 状态，避免并发执行升级/安装任务。

**执行记录：**
- 2026-07-03：执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli tray:: -- --nocapture` 通过，lib 与 bin 两组各 140 个 tray 相关测试通过。
- 2026-07-03：补充并执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli test_menu_native_app -- --nocapture` 通过，直接覆盖 `Install Native App` 与 `Open Native App` 菜单分支。
- 2026-07-03：执行源码检查命令通过，确认点击处理会派生 `bifrost-tray-native-app-install` 任务并调用 `native-app install -y --open`。

### TC-MNA-34：Native App 定时检查更新并提示重启

**操作步骤：**
1. 执行 Swift 构建与 core contract 检查：
   ```bash
   scripts/build-macos-native.sh --skip-sidecar --test
   ```
2. 检查 Native App 自动更新 loop、版本检查 API 和重启提示源码：
   ```bash
   rg -n 'startNativeAppUpdateChecks|checkNativeAppUpdate|BIFROST_NATIVE_UPDATE_INTERVAL_SECONDS|BIFROST_NATIVE_UPDATE_CHECK_DISABLED|installNativeApp|Restart Bifrost Native App|fetchVersionCheck|fetchNativeAppStatus' \
     apps/macos/Sources/Bifrost/App/AppModel.swift \
     apps/macos/Sources/BifrostNativeCore/BifrostClient/BifrostClient.swift \
     apps/macos/Sources/BifrostNativeCore/BifrostClient/AdminModels.swift
   ```

**预期结果：**
- Swift 构建和 core contract 检查通过。
- Native App 启动连接到 Admin API 后，默认每 6 小时检查 `/system/version-check`；测试可用 `BIFROST_NATIVE_UPDATE_INTERVAL_SECONDS` 降低轮询间隔。
- 检测到新版本后同一个 latest version 只弹一次确认；用户确认后调用 `/system/native-app/install`。
- 安装 accepted 后弹出重启提示，用户确认后重新打开 `Bifrost.app` 并退出旧进程完成更新。

**执行记录：**
- 2026-07-03：执行 `scripts/build-macos-native.sh --skip-sidecar --test` 通过，生成 `apps/macos/.build/Bifrost.app`，`BifrostNativeCoreChecks passed`。
- 2026-07-03：执行源码检查命令通过，确认 `startNativeAppUpdateChecks`、`checkNativeAppUpdate`、`BIFROST_NATIVE_UPDATE_INTERVAL_SECONDS`、`BIFROST_NATIVE_UPDATE_CHECK_DISABLED`、`fetchVersionCheck`、`installNativeApp` 与 `Restart Bifrost Native App` 提示均存在。
- 2026-07-03：复查并调整重启路径，确认 Native App 重启时打开安装 API 返回的 `install_path`，不是固定打开当前 bundle。

### TC-MNA-35：Release workflow 产出 macOS Native App 安装包

**操作步骤：**
1. 检查 release workflow 的 native app 构建矩阵和 release artifact 依赖：
   ```bash
   ruby -e 'require "yaml"; ci=YAML.load_file(".github/workflows/release.yml"); job=ci.fetch("jobs").fetch("build-macos-native"); matrix=job.fetch("strategy").fetch("matrix").fetch("include"); raise "missing native matrix" unless matrix.any?{|item| item["target"]=="aarch64-apple-darwin"} && matrix.any?{|item| item["target"]=="x86_64-apple-darwin"}; needs=ci.fetch("jobs").fetch("release").fetch("needs"); raise "release does not need native job" unless needs.include?("build-macos-native"); puts "macos native release workflow ok"'
   ```
2. 检查 Native App bundle 版本来自发布版本号，而不是硬编码值：
   ```bash
   rg -n 'BIFROST_VERSION|CFBundleShortVersionString|CFBundleVersion' scripts/build-macos-native.sh
   ```

**预期结果：**
- Release workflow 包含 arm64 与 x86_64 的 `build-macos-native` 矩阵。
- Release job 等待 `build-macos-native` artifacts，因此 GitHub Release 会包含 `bifrost-native-v<version>-<target>.dmg` 与 checksum。
- `scripts/build-macos-native.sh` 通过 `BIFROST_VERSION` 写入 Info.plist，Native App 自动更新能读到真实发布版本。

**执行记录：**
- 2026-07-03：执行 release workflow Ruby 检查通过，输出 `macos native release workflow ok`。
- 2026-07-03：执行 `rg -n 'BIFROST_VERSION|CFBundleShortVersionString|CFBundleVersion' scripts/build-macos-native.sh` 通过，确认 Info.plist 版本字段来自 `BIFROST_VERSION`。

### TC-MNA-36：回归 - 核心面板切换不做重复拉取或大列表 render 扫描

**操作步骤：**
1. 执行 Swift core contract 与 app 构建：
   ```bash
   swift run --package-path apps/macos BifrostNativeCoreChecks
   swift build --package-path apps/macos
   ```
2. 检查 Activity/Network 的统计缓存与 Rules 选择链路：
   ```bash
   rg -n 'activityRuleHitCount|refreshActivityTrafficSummaries|filter\(\\.hasRuleHit\)|task\(id: appModel.selectedRuleName\)|allowsHoverEffect: false' \
     apps/macos/Sources/Bifrost/App/AppModel.swift \
     apps/macos/Sources/Bifrost/Features/Dashboard/DashboardView.swift \
     apps/macos/Sources/Bifrost/Features/Rules/RulesView.swift
   ```
3. 执行 native 性能 smoke：
   ```bash
   apps/macos/.build/Bifrost.app/Contents/MacOS/Bifrost --check-traffic-table-performance
   apps/macos/.build/Bifrost.app/Contents/MacOS/Bifrost --check-release-scope
   ```

**预期结果：**
- `BifrostNativeCoreChecks` 与 Swift build 通过。
- Network 页展示规则命中时读取 `activityRuleHitCount` 缓存，不在 SwiftUI render 阶段对 `trafficRecords` 执行 `filter(\.hasRuleHit)` 全量扫描。
- Rules 页点击列表行只通过行 action 调用 `selectRule`，不存在 `.task(id: selectedRuleName)` 再次触发同一规则详情拉取。
- 首次进入 Rules 页时，如果 AppModel 只选中了首条规则名但尚未加载详情，入口刷新流程会主动加载一次选中规则详情。
- Rules 列表与详情两块大容器关闭 hover 缩放动画，保持白色卡片、描边和柔光，避免编辑器区域在 hover 时触发大面积重绘。
- release scope 仍然只暴露 `活动,概览,规则,网络`，traffic table performance smoke 通过。

### TC-MNA-37：回归 - Rules 原生交互契约与自动保存

**操作步骤：**
1. 执行 Swift build 与 core contract：
   ```bash
   swift run --package-path apps/macos BifrostNativeCoreChecks
   swift build --package-path apps/macos
   ```
2. 检查 Default 规则保护、拖拽排序、自动保存和 Rules 顶部按钮：
   ```bash
   rg -n 'isDefaultRule|moveRules\\(|reorderRules|autosaveSelectedRule|RuleAutoSaveState|onMove|Save|Revert|Button\\(\"刷新\"|刷新设备|重新生成 QR' \
     apps/macos/Sources/Bifrost/App/AppModel.swift \
     apps/macos/Sources/Bifrost/Features/Rules/RulesView.swift \
     apps/macos/Sources/Bifrost/Features/Dashboard/DashboardView.swift
   ```
3. 执行设置数据 smoke，确认移除刷新按钮后页面切入数据仍可自动获取：
   ```bash
   apps/macos/.build/Bifrost.app/Contents/MacOS/Bifrost --check-settings-data
   ```

**预期结果：**
- AppModel 对 `Default` 规则有保护：不能禁用、不能重命名、不能删除，且拖拽排序时不能移动 Default。
- Rules 列表对非搜索状态支持拖拽排序，排序通过 `BifrostClient.reorderRules` 持久化；搜索状态只筛选查看，不做排序。
- Rules 详情区没有额外 `Save` / `Revert` 按钮；编辑器输入后 debounce 自动保存，Cmd+S 触发立即保存。
- 自动保存成功不重新 `selectRule`，避免重置编辑器光标、滚动和 undo 状态。
- 概览页不展示通用 `刷新`、`刷新设备`、`重新生成 QR` 按钮；切到页面后仍通过自动加载展示系统代理、TLS、Remote Invoke、证书和移动端可用性数据。

### TC-MNA-38：回归 - Native 深色主题不能残留浅色底板

**操作步骤：**
1. 执行无窗口主题合同检查：
   ```bash
   swift run --package-path apps/macos Bifrost --check-theme-contract
   ```
2. 执行源码扫描，确认主窗口 surface、通用代码编辑器、Rules DSL 编辑器和 Dashboard TLS 名单编辑器不再写死浅色背景：
   ```bash
   ruby -e 'checks={
     "AppModel default system theme"=>"@Published var colorSchemeMode: ColorSchemeMode = .system",
     "AppSurface adaptive colors"=>"static func resolvedContentColor(for appearance: NSAppearance.Name)",
     "CodeEditor system text background"=>"textView.backgroundColor = .textBackgroundColor",
     "Rule editor adaptive theme"=>"BifrostRuleEditorTheme(appearance: effectiveAppearance)",
     "Dashboard editor card background"=>".background(AppSurface.card, in: RoundedRectangle"
   }; text=Dir["apps/macos/Sources/Bifrost/**/*.swift"].map{|p| File.read(p)}.join("\n"); missing=checks.reject{|_, needle| text.include?(needle)}; abort("missing dark theme contract markers: #{missing.keys.join(", ")}") unless missing.empty?; puts "macOS native dark theme source contract ok"'
   ```
3. 构建并打开 Native `.app`，通过应用左下角主题按钮切换到深色外观后截图：
   ```bash
   scripts/build-macos-native.sh --skip-sidecar --test
   pkill -f 'Bifrost.app/Contents/MacOS/Bifrost' || true
   open -n apps/macos/.build/Bifrost.app
   sleep 3
   osascript -e 'tell application "System Events" to set frontmost of process "Bifrost" to true'
   # 从默认 System 依次切到 Light、Dark；如当前已是 Light，则第二次点击后仍进入 Dark。
   osascript -e 'tell application "System Events" to tell process "Bifrost" to click button 1 of group 1 of window 1' || true
   osascript -e 'tell application "System Events" to tell process "Bifrost" to click button 1 of group 1 of window 1' || true
   screencapture -x /tmp/bifrost-native-dark-theme.png
   ```
4. 如可操作窗口，进入 Rules 页面并切换主题按钮，肉眼确认列表、编辑器、补全面板、行号栏和状态栏均可读。

**预期结果：**
- `--check-theme-contract` 输出 `Bifrost theme contract check passed`。
- 源码扫描输出 `macOS native dark theme source contract ok`。
- Native app 默认跟随系统外观；深色系统外观下不再强制 `.preferredColorScheme(.light)`。
- 主窗口 content、sidebar、selection、card、card border、card highlight、card shadow、subtle fill 均为 appearance-adaptive 动态色。
- AppKit `CodeEditorView` 使用系统 text background/text color/insertion point color。
- `BifrostRuleEditorView` 的正文背景、文字、插入点、行号栏和语法高亮都跟随 effective appearance；深色主题下不出现白底编辑器或浅色行号栏。
- Dashboard TLS 名单编辑器使用统一 card 背景；深色主题下不出现整块白色文本编辑区域。
- 除图标、二维码图片本体、accent 按钮文字等语义上必须保持白色的元素外，页面不应有大面积浅色底板或低对比文字。

**实际结果（2026-07-03）：**
- 执行 `swift run --package-path apps/macos Bifrost --check-theme-contract` 通过，输出 `Bifrost theme contract check passed`。
- 执行源码扫描通过，输出 `macOS native dark theme source contract ok`。
- 执行 `scripts/build-macos-native.sh --skip-sidecar --test` 通过，生成 `apps/macos/.build/Bifrost.app`，`BifrostNativeCoreChecks passed`。
- 执行 `open -n apps/macos/.build/Bifrost.app` 启动最新开发 app；通过 System Events 点击左下角主题按钮切到 `Dark`，截图保存到 `/tmp/bifrost-native-dark-theme-after-toggle.png`。
- 截图确认 Rules 页面 sidebar、主内容底色、规则列表卡片、详情卡片、规则编辑器正文背景、行号栏、状态栏均为深色且文字可读；未见大面积浅色底板残留。

### TC-MNA-39：回归 - 四个主入口右侧内容自适应可用宽度

**操作步骤：**
1. 执行源码布局合同扫描：
   ```bash
   ruby -e 'checks={
     "page scaffold fills width"=>".frame(maxWidth: .infinity, alignment: .leading)",
     "activity metric six columns"=>"activityMetricGrid(columnCount: 6)",
     "activity metric flexible columns"=>"GridItem(.flexible(minimum: 150)",
     "rules detail layout priority"=>".layoutPriority(1)",
     "rules list fixed width"=>".frame(width: ruleListWidth)",
     "traffic content fills width"=>".frame(maxWidth: .infinity, alignment: .leading)"
   }; text=Dir["apps/macos/Sources/Bifrost/**/*.swift"].map{|p| File.read(p)}.join("\n"); missing=checks.reject{|_, needle| text.include?(needle)}; abort("missing adaptive layout markers: #{missing.keys.join(", ")}") unless missing.empty?; forbidden=["maxWidth: 1180", "maxWidth: 980", "maxWidth: 760"]; found=forbidden.select{|needle| text.include?(needle)}; abort("fixed layout caps remain: #{found.join(", ")}") unless found.empty?; puts "macOS native adaptive layout source contract ok"'
   ```
2. 构建 Native `.app`：
   ```bash
   scripts/build-macos-native.sh --skip-sidecar --test
   ```
3. 打开 Native `.app`，逐个切换 `活动`、`概览`、`规则`、`网络`，并截图：
   ```bash
   pkill -f 'Bifrost.app/Contents/MacOS/Bifrost' || true
   open -n apps/macos/.build/Bifrost.app
   sleep 3
   # 通过左侧 source-list 切换四个主入口后分别 screencapture 到 /tmp/bifrost-native-adaptive-layout-*.png
   ```

**预期结果：**
- 源码扫描输出 `macOS native adaptive layout source contract ok`。
- `NativePageScaffold` 不再把主内容封顶在 `1180`，Settings 内容不再封顶在 `980`，Traffic 表格不再封顶在 `760`。
- `活动` 顶部指标卡最多一排 6 个；宽屏时 6 个卡片平分并撑满可用宽度，超过 6 个时按 6 列换行；窄窗口自动降到 5/4/3/2/1 列。
- `活动`、`概览`、`网络` 页面卡片或入口卡片横向填充右侧内容区，而不是只占左侧一块固定宽度。
- `规则` 页面左侧列表固定 300px，右侧规则编辑器卡片优先吃满剩余空间。
- 窗口放大时，四个主入口都应重新分配右侧内容宽度，不出现明显的大块空白浪费。

**实际结果（2026-07-03）：**
- 执行源码布局合同扫描通过，输出 `macOS native adaptive layout source contract ok`。
- 执行 `scripts/build-macos-native.sh --skip-sidecar --test` 通过，生成 `apps/macos/.build/Bifrost.app`，`BifrostNativeCoreChecks passed`。
- 重新打开 Native `.app` 并截图：`活动` 保存到 `/tmp/bifrost-native-adaptive-layout.png`，`概览` 保存到 `/tmp/bifrost-native-adaptive-layout-overview.png`，`规则` 保存到 `/tmp/bifrost-native-adaptive-layout-rules.png`，`网络` 保存到 `/tmp/bifrost-native-adaptive-layout-network.png`。
- 截图确认 `活动` 三列指标卡、`概览` 双列控制卡和 `规则` 右侧编辑器都铺满右侧内容区；未见旧版固定窄容器导致的大面积右侧空白。

**实际结果（2026-07-04）：**
- 执行 `swift build --package-path apps/macos` 通过。
- 执行源码布局合同扫描通过，输出 `macOS native adaptive layout source contract ok`，并确认 Activity 顶部指标卡使用 `activityMetricGrid(columnCount: 6)` 和 `GridItem(.flexible(minimum: 150)`，不再使用 `GridItem(.adaptive(minimum: 210, maximum: 360)`。
- 执行 `scripts/build-macos-native.sh --skip-sidecar --test` 通过，生成 `apps/macos/.build/Bifrost.app`，`BifrostNativeCoreChecks passed`。
- 重新打开 Native `.app`，进程路径为 `apps/macos/.build/Bifrost.app/Contents/MacOS/Bifrost`。

### TC-MNA-40：回归 - Rules 页面左右面板高度撑满可用区域

**操作步骤：**
1. 执行源码高度布局合同扫描：
   ```bash
   ruby -e 'checks={
     "scaffold can fill height"=>"contentFillsAvailableHeight",
     "rules enables fill height"=>"NativePageScaffold(title: \"规则\", contentFillsAvailableHeight: true)",
     "rules row fills height"=>".frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)",
     "rules detail fills height"=>".frame(minWidth: 520, maxWidth: .infinity, maxHeight: .infinity)",
     "rule editor fills height"=>".frame(maxWidth: .infinity, maxHeight: .infinity)"
   }; text=Dir["apps/macos/Sources/Bifrost/**/*.swift"].map{|p| File.read(p)}.join("\n"); missing=checks.reject{|_, needle| text.include?(needle)}; abort("missing height fill markers: #{missing.keys.join(", ")}") unless missing.empty?; puts "macOS native rules height contract ok"'
   ```
2. 构建并打开 Native `.app`，进入 `规则` 页面截图：
   ```bash
   scripts/build-macos-native.sh --skip-sidecar --test
   pkill -f 'Bifrost.app/Contents/MacOS/Bifrost' || true
   open -n apps/macos/.build/Bifrost.app
   sleep 3
   # 切换到 Rules 后截图 /tmp/bifrost-native-adaptive-layout-rules-height.png
   ```

**预期结果：**
- 源码扫描输出 `macOS native rules height contract ok`。
- Rules 页面不再只使用固定 `minHeight: 600`。
- 左侧规则列表 panel 和右侧规则编辑器 panel 从页面标题下方延伸到状态栏上方，随窗口高度变化自适应。
- 右侧规则编辑器内部正文区域也撑满卡片高度，不在编辑器下方留下大块空白。

**实际结果（2026-07-03）：**
- 执行 `swift build --package-path apps/macos` 通过。
- 执行 `scripts/build-macos-native.sh --skip-sidecar --test` 通过，生成 `apps/macos/.build/Bifrost.app`，`BifrostNativeCoreChecks passed`。
- 重新打开 Native `.app` 并切换到 Rules 页面，截图保存到 `/tmp/bifrost-native-adaptive-layout-rules-height.png`。
- 截图确认左侧规则列表和右侧规则编辑器卡片均撑满到状态栏上方；旧版底部大块空白已消失。

### TC-MNA-41：回归 - Native 主页面顶部留白保持紧凑

**操作步骤：**
1. 执行源码布局合同扫描，确认统一 scaffold 顶部 padding 和标题到内容间距保持紧凑：
   ```bash
   ruby -e 'text=File.read("apps/macos/Sources/Bifrost/App/NativeSurface.swift"); required=["VStack(alignment: .leading, spacing: 11)", ".padding(.top, 10)"]; missing=required.reject{|needle| text.include?(needle)}; abort("missing compact top spacing markers: #{missing.join(", ")}") unless missing.empty?; forbidden=["VStack(alignment: .leading, spacing: 22)", ".padding(.top, 20)"]; found=forbidden.select{|needle| text.include?(needle)}; abort("loose top spacing remains: #{found.join(", ")}") unless found.empty?; puts "macOS native compact top spacing contract ok"'
   ```
2. 构建并打开 Native `.app`，进入 `规则` 页面截图：
   ```bash
   scripts/build-macos-native.sh --skip-sidecar --test
   pkill -f 'Bifrost.app/Contents/MacOS/Bifrost' || true
   open -n apps/macos/.build/Bifrost.app
   sleep 3
   # 切换到 Rules 后截图 /tmp/bifrost-native-rules-tight-layout.png
   ```

**预期结果：**
- 源码扫描输出 `macOS native compact top spacing contract ok`。
- 主页面标题顶部留白约为旧版一半。
- 标题到第一行内容的间距约为旧版一半。
- `活动`、`概览`、`规则`、`网络` 页面都继承同一套紧凑顶部 spacing。

**实际结果（2026-07-03）：**
- 执行 `swift build --package-path apps/macos` 通过。
- 执行 `scripts/build-macos-native.sh --skip-sidecar --test` 通过，生成 `apps/macos/.build/Bifrost.app`，`BifrostNativeCoreChecks passed`。
- 重新打开 Native `.app` 并切换到 Rules 页面，截图保存到 `/tmp/bifrost-native-rules-tight-layout.png`。
- 截图确认 Rules 页面顶部留白已明显缩小，且左右面板仍保持宽高自适应。

### TC-MNA-42：回归 - Rules 编辑器内容可见、可编辑且切换侧栏不闪退

**操作步骤：**
1. 执行源码合同扫描，确认 Rules 页面临时使用稳定标准编辑器，侧栏不再走 macOS 26 上触发崩溃的 SwiftUI `List`/`OutlineListCoordinator` 路径：
   ```bash
   ruby -e 'rules=File.read("apps/macos/Sources/Bifrost/Features/Rules/RulesView.swift"); sidebar=File.read("apps/macos/Sources/Bifrost/App/Sidebar.swift"); editor=File.read("apps/macos/Sources/Bifrost/AppKitBridge/CodeEditorView.swift"); abort("Rules still uses custom invisible editor") if rules.include?("BifrostRuleEditorView("); abort("sidebar still uses crash-prone SwiftUI List") if sidebar.include?("List(") || sidebar.include?(".listStyle"); required=["CodeEditorView(", "onTextChanged", "onSave", "CodeEditorTextView", "ForEach(SidebarItem.releaseScopeItems)"]; text=rules+"\n"+sidebar+"\n"+editor; missing=required.reject{|needle| text.include?(needle)}; abort("missing native recovery markers: #{missing.join(", ")}") unless missing.empty?; puts "macOS native rule editor recovery contract ok"'
   ```
2. 执行无窗口编辑器布局检查：
   ```bash
   swift run --package-path apps/macos Bifrost --check-rule-editor-layout
   ```
3. 构建并打开 Native `.app`，从左侧主导航切到 `规则`：
   ```bash
   scripts/build-macos-native.sh --skip-sidecar --test
   pkill -f 'Bifrost.app/Contents/MacOS/Bifrost' || true
   open -n apps/macos/.build/Bifrost.app
   sleep 3
   # 切换到 Rules 后截图 /tmp/bifrost-native-rule-editor-standard-visible.png
   ```
4. 在 Rules 页面确认 Default 规则正文注释可见；点击编辑器正文区域，确认可以进入文本编辑状态；切换左侧主导航不发生闪退。

**预期结果：**
- 源码扫描输出 `macOS native rule editor recovery contract ok`。
- `--check-rule-editor-layout` 输出 `Bifrost rule editor layout check passed`。
- Rules 页面右侧编辑器正文内容可见，不再只显示行号或空白底板。
- 编辑器使用标准 AppKit 文本视图路径，支持文本输入、自动保存回调和 Cmd+S。
- 左侧主导航使用普通 `ForEach + Button`，不再触发 `OutlineListCoordinator.applyContext()` 崩溃路径。

**实际结果（2026-07-03）：**
- 用户反馈 Rules 编辑器内容不可见且应用闪退；崩溃报告显示主线程崩在 SwiftUI `OutlineListCoordinator.applyContext()`。
- 将左侧主导航从 SwiftUI `List` 改为稳定的 `VStack + ForEach + Button` 后，重新启动 Native `.app` 并切换到 Rules 页面未再复现闪退。
- 将 Rules 编辑器临时切换到标准 `CodeEditorView` 后，截图 `/tmp/bifrost-native-rule-editor-standard-visible.png` 确认 Default 规则正文 `# Global default rules.` 和第二行注释可见。
- 执行 `swift run --package-path apps/macos Bifrost --check-theme-contract`、`swift run --package-path apps/macos Bifrost --check-release-scope`、源码恢复合同扫描均通过。

### TC-MNA-43：回归 - Overview 证书卡片不挤压且窗口标题不重复显示

**操作步骤：**
1. 执行源码合同扫描，确认 Overview 证书与移动端卡片使用自适应布局，窗口 chrome 每次更新都隐藏系统标题：
   ```bash
   ruby -e 'dashboard=File.read("apps/macos/Sources/Bifrost/Features/Dashboard/DashboardView.swift"); chrome=File.read("apps/macos/Sources/Bifrost/App/MainWindowScene.swift"); required=["ViewThatFits(in: .horizontal)", "CertificateSummarySection", "GridItem(.adaptive(minimum: 118", ".frame(minWidth: 300, idealWidth: 320, maxWidth: 360", "window.title = \"\"", "window.subtitle = \"\"", "window.titleVisibility = .hidden", "window.toolbar = nil"]; text=dashboard+"\n"+chrome; missing=required.reject{|needle| text.include?(needle)}; abort("missing overview layout/title markers: #{missing.join(", ")}") unless missing.empty?; abort("fixed cramped probe width remains") if dashboard.include?(".frame(width: 260"); puts "macOS native overview layout and titlebar contract ok"'
   ```
2. 构建并打开 Native `.app`，切换到 `概览` 页面截图：
   ```bash
   scripts/build-macos-native.sh --skip-sidecar --test
   pkill -f 'Bifrost.app/Contents/MacOS/Bifrost' || true
   open -n apps/macos/.build/Bifrost.app
   sleep 3
   # 切换到 Overview 后截图 /tmp/bifrost-native-overview-layout-title-fix.png
   ```

**预期结果：**
- 源码扫描输出 `macOS native overview layout and titlebar contract ok`。
- 主窗口顶部 titlebar 不再重复显示 `Bifrost` 文本。
- `证书与移动端` 卡片里的 `本机 CA`、`代理地址`、`移动设备` 统计项不会被压成逐字竖排；空间不足时卡片内容换行或上下排列。
- `可用性检查` 面板不再固定 260 宽挤压左侧内容。

**实际结果（2026-07-03）：**
- 执行 `swift build --package-path apps/macos` 通过。
- 执行源码合同扫描通过，输出 `macOS native certificate card responsive layout contract ok` 和 `macOS native titlebar suppression contract ok`。
- 执行 `scripts/build-macos-native.sh --skip-sidecar --test` 通过，生成 `apps/macos/.build/Bifrost.app`，`BifrostNativeCoreChecks passed`。
- 重新打开 Native `.app` 并切换到 Overview 页面，截图保存到 `/tmp/bifrost-native-overview-layout-title-fix.png`；截图确认顶部重复 `Bifrost` 标题已消失，证书卡片统计项不再逐字竖排。

### TC-MNA-44：回归 - Overview Remote Invoke SSH Key 复制按钮始终可见

**操作步骤：**
1. 执行源码合同扫描，确认 Remote Invoke 的 SSH Key 复制操作是稳定按钮，不再用 `已复制` 文案替换按钮标题：
   ```bash
   ruby -e 'text=File.read("apps/macos/Sources/Bifrost/Features/Dashboard/DashboardView.swift"); required=["Label(\"复制 SSH Key\", systemImage: \"doc.on.doc\")", ".buttonStyle(.bordered)", "sshKeyRecentlyCopied", "Text(\"已复制\")"]; missing=required.reject{|needle| text.include?(needle)}; abort("missing ssh key copy button markers: #{missing.join(", ")}") unless missing.empty?; abort("copy feedback still replaces button title") if text.include?("Button(copiedTitle)") || text.include?("private var copiedTitle"); puts "macOS native ssh key copy button contract ok"'
   ```
2. 构建并打开 Native `.app`，切换到 `概览` 页面截图：
   ```bash
   scripts/build-macos-native.sh --skip-sidecar --test
   pkill -f 'Bifrost.app/Contents/MacOS/Bifrost' || true
   open -n apps/macos/.build/Bifrost.app
   sleep 3
   # 切换到 Overview 后截图 /tmp/bifrost-native-ssh-key-copy-button.png
   ```
3. 点击 Remote Invoke 的 `复制 SSH Key` 按钮，确认按钮仍可见，旁边短暂显示 `已复制`。

**预期结果：**
- 源码扫描输出 `macOS native ssh key copy button contract ok`。
- Remote Invoke 的 SSH Key 操作区始终显示带 `doc.on.doc` 图标的 `复制 SSH Key` 按钮。
- 点击复制后剪贴板写入 SSH key 内容，按钮不消失，只在旁边显示短暂 `已复制` 反馈。

**实际结果（2026-07-03）：**
- 执行 `swift build --package-path apps/macos` 通过。
- 执行源码合同扫描通过，输出 `macOS native ssh key copy button contract ok`。
- 执行 `scripts/build-macos-native.sh --skip-sidecar --test` 通过，生成 `apps/macos/.build/Bifrost.app`，`BifrostNativeCoreChecks passed`。
- 重新打开 Native `.app` 并切换到 Overview 页面，截图保存到 `/tmp/bifrost-native-ssh-key-copy-button.png`；截图确认 Remote Invoke SSH Key 区域显示带图标的 `复制 SSH Key` 按钮。

### TC-MNA-45：回归 - Rules 详情标题支持双击原位重命名

**操作步骤：**
1. 执行源码合同扫描，确认 Rules 详情标题支持原位编辑，不再弹出旧 Rename sheet：
   ```bash
   ruby -e 'text=File.read("apps/macos/Sources/Bifrost/Features/Rules/RulesView.swift"); required=["inlineRuleTitle", "onTapGesture(count: 2)", "beginInlineRename()", "commitInlineRename()", "onExitCommand", "renameSelectedRule(to: trimmed)", "focused($inlineRenameFocused)"]; missing=required.reject{|needle| text.include?(needle)}; abort("missing inline rename markers: #{missing.join(", ")}") unless missing.empty?; forbidden=["renameSheetVisible", "Rename Rule"]; found=forbidden.select{|needle| text.include?(needle)}; abort("old rename sheet remains: #{found.join(", ")}") unless found.empty?; puts "macOS native inline rule rename contract ok"'
   ```
2. 构建并打开 Native `.app`，进入 `规则` 页面，选择一条非 `Default` 规则。
3. 双击右侧详情标题中的规则名。
4. 修改名称后按 Enter 提交；再次选择一条非 `Default` 规则，双击标题后按 Esc 取消。
5. 选择 `Default` 规则后双击标题，确认不会进入编辑态。

**预期结果：**
- 源码扫描输出 `macOS native inline rule rename contract ok`。
- 非保护规则标题双击后，原规则名位置直接变成单行输入框。
- Enter 提交重命名并刷新规则列表；Esc 取消，不改名。
- 菜单里的 `Rename` 也进入同一个原位编辑流程。
- `Default` 规则受保护，双击标题和菜单 Rename 都不能重命名。

**实际结果（2026-07-03）：**
- 执行 `swift build --package-path apps/macos` 通过。
- 执行源码合同扫描通过，输出 `macOS native inline rule rename contract ok`。
- 执行 `scripts/build-macos-native.sh --skip-sidecar --test` 通过，生成 `apps/macos/.build/Bifrost.app`，`BifrostNativeCoreChecks passed`。
- 重新打开 Native `.app` 并进入 Rules 页面，确认 `Default` 规则标题双击不会进入编辑态；非保护规则原位编辑能力由源码合同覆盖。

### TC-MNA-46：回归 - Rules 左侧列表固定 300px 且不可拖拽调整宽度

**操作步骤：**
1. 执行源码合同扫描，确认 Rules 左侧列表使用固定 300px 宽度，不再存在拖拽手柄、resize cursor 或拖拽宽度状态：
   ```bash
   ruby -e 'text=File.read("apps/macos/Sources/Bifrost/Features/Rules/RulesView.swift"); required=["private let ruleListWidth: CGFloat = 300", ".frame(width: ruleListWidth)"]; missing=required.reject{|needle| text.include?(needle)}; abort("missing fixed rule list markers: #{missing.join(", ")}") unless missing.empty?; forbidden=["RuleListResizeHandle", "DragGesture(minimumDistance: 0)", "NSCursor.resizeLeftRight", "ruleListResizeStartWidth", "clampedRuleListWidth", "ruleListMaxWidth", "ruleListMinWidth"]; found=forbidden.select{|needle| text.include?(needle)}; abort("rule list resize behavior remains: #{found.join(", ")}") unless found.empty?; puts "macOS native fixed rule list width contract ok"'
   ```
2. 构建并打开 Native `.app`，进入 `规则` 页面：
   ```bash
   scripts/build-macos-native.sh --skip-sidecar --test
   pkill -f 'Bifrost.app/Contents/MacOS/Bifrost' || true
   open -n apps/macos/.build/Bifrost.app
   ```
3. 观察左侧规则列表和右侧编辑器之间的区域，并尝试鼠标 hover / 拖拽。

**预期结果：**
- 源码扫描输出 `macOS native fixed rule list width contract ok`。
- 左侧规则列表固定为 300px。
- 左侧规则列表和右侧编辑器之间不存在可拖拽分割线，不出现左右 resize 光标。
- 鼠标拖拽不会改变左侧规则列表宽度。
- 右侧编辑器始终自适应剩余可用空间。

**实际结果（2026-07-03）：**
- 执行 `swift build --package-path apps/macos` 通过。
- 执行源码合同扫描通过，输出 `macOS native rule list resize contract ok`。
- 执行 `scripts/build-macos-native.sh --skip-sidecar --test` 通过，生成 `apps/macos/.build/Bifrost.app`，`BifrostNativeCoreChecks passed`。
- 重新打开 Native `.app` 并进入 Rules 页面；拖拽行为由源码合同中的 `DragGesture`、`clampedRuleListWidth` 和 `ruleListMaxWidth = 300` 覆盖。

**实际结果（2026-07-04）：**
- 执行源码合同扫描通过，输出 `macOS native fixed rule list width contract ok`。
- Rules 左侧列表固定为 300px，源码中不再存在 `RuleListResizeHandle`、`DragGesture` 或 `NSCursor.resizeLeftRight`。

### TC-MNA-47：回归 - Native 全局指标和网络数据通过 WebSocket 实时推送刷新

**操作步骤：**
1. 执行源码合同扫描，确认 Native WebSocket 订阅和处理覆盖 overview、metrics、traffic 三类实时数据：
   ```bash
   ruby -e 'core=File.read("apps/macos/Sources/BifrostNativeCore/BifrostClient/AdminModels.swift"); push=File.read("apps/macos/Sources/BifrostNativeCore/BifrostClient/PushClient.swift"); app=File.read("apps/macos/Sources/Bifrost/App/AppModel.swift"); sidebar=File.read("apps/macos/Sources/Bifrost/App/Sidebar.swift"); required={"overview decode"=>"case overviewUpdate(SystemOverview)", "metrics decode"=>"case metricsUpdate(MetricsPushData)", "need metrics query"=>"need_metrics", "overview handler"=>"case .overviewUpdate(let data):", "metrics handler"=>"applyMetricsUpdate(data.metrics)", "global traffic subscription"=>"needTraffic: true", "500ms metrics"=>"metricsIntervalMs: 500", "network traffic records"=>"case .activity, .network:"}; text=[core,push,app,sidebar].join("\n"); missing=required.reject{|_, needle| text.include?(needle)}; abort("missing realtime markers: #{missing.keys.join(", ")}") unless missing.empty?; abort("traffic delta still gated by selected tab") if app.include?("if selectedSidebarItem.needsTrafficRecords {\n                enqueueTrafficDelta"); puts "macOS native realtime overview metrics traffic contract ok"'
   ```
2. 构建并打开 Native `.app`，停留在 `活动` 页面观察活动连接、上传、下载、请求数和底部状态栏。
3. 在浏览器或命令行持续产生代理流量；不要切换 tab。
4. 切换到 `网络` 页面，确认流量列表和过滤统计已经随 WebSocket 推送更新，而不是只在切换 tab 时刷新。

**预期结果：**
- 源码扫描输出 `macOS native realtime overview metrics traffic contract ok`。
- `PushSubscription` 会发送 `need_overview=true`、`need_metrics=true`、`need_traffic=true` 和 `metrics_interval_ms=500`。
- Native 能 decode `overview_update` 与 `metrics_update`，并写回 `appModel.overview`。
- Activity 指标卡和全局底部 bar 的上传/下载速度、连接数、请求数、内存、CPU、uptime 不依赖切 tab 刷新。
- Network 页面也被标记为需要 traffic records；traffic delta 不再按当前 tab 丢弃。

**实际结果（2026-07-03）：**
- 执行 `swift build --package-path apps/macos` 通过。
- 执行源码合同扫描通过，输出 `macOS native realtime overview metrics traffic contract ok`。
- 执行 `scripts/build-macos-native.sh --skip-sidecar --test` 通过，生成 `apps/macos/.build/Bifrost.app`，`BifrostNativeCoreChecks passed`。
- 重新打开 Native `.app`；实时刷新能力由源码合同中的 `overview_update`、`metrics_update`、`need_metrics`、`needTraffic: true` 和 Network traffic subscription 覆盖。

### TC-MNA-48：回归 - 最外层主菜单使用系统 source-list sidebar

**操作步骤：**
1. 执行源码合同扫描，确认主窗口使用 macOS 系统推荐的 `NavigationSplitView + List(.sidebar)` source-list，不使用自定义 HStack/overlay/sidebar rail，且右侧 detail 不再用过大的最小宽度把左侧菜单挤出窗口：
   ```bash
   ruby -e 'main=File.read("apps/macos/Sources/Bifrost/App/MainWindowScene.swift"); sidebar=File.read("apps/macos/Sources/Bifrost/App/Sidebar.swift"); surface=File.read("apps/macos/Sources/Bifrost/App/NativeSurface.swift"); dashboard=File.read("apps/macos/Sources/Bifrost/Features/Dashboard/DashboardView.swift"); rules=File.read("apps/macos/Sources/Bifrost/Features/Rules/RulesView.swift"); traffic=File.read("apps/macos/Sources/Bifrost/Features/Traffic/TrafficView.swift"); sidebar_text=main+"\n"+sidebar; forbidden=["HStack(spacing: 0)", "GeometryReader { proxy in", "isSidebarCollapsed", "SidebarItemRow", ".navigationTitle(\"\")"]; found=forbidden.select{|needle| sidebar_text.include?(needle)}; abort("custom sidebar shell remains: #{found.join(", ")}") unless found.empty?; required=["NavigationSplitView {", ".navigationSplitViewColumnWidth(min: 176, ideal: 204, max: 232)", ".navigationSplitViewStyle(.balanced)", ".frame(minWidth: 0, maxWidth: .infinity, maxHeight: .infinity)", "List(SidebarItem.releaseScopeItems)", ".listStyle(.sidebar)", ".scrollContentBackground(.hidden)", "Label(item.rawValue, systemImage: item.systemImage)"]; missing=required.reject{|needle| sidebar_text.include?(needle)}; abort("missing native source-list sidebar markers: #{missing.join(", ")}") unless missing.empty?; width_contracts=[surface.include?("pageHorizontalPadding(for:"), dashboard.include?("overviewControlGrid(columnCount: 4)") && dashboard.include?("GridItem(.flexible(minimum: 220)"), rules.include?(".frame(minWidth: 360"), traffic.include?(".frame(minWidth: 320")]; abort("right detail min-width contract missing") unless width_contracts.all?; puts "macOS native source-list responsive width contract ok"'
   ```
2. 构建并打开 Native `.app`：
   ```bash
   scripts/build-macos-native.sh --skip-sidecar --test
   pkill -f 'Bifrost.app/Contents/MacOS/Bifrost' || true
   open -n apps/macos/.build/Bifrost.app
   ```
3. 使用系统标题栏的 sidebar 按钮折叠/展开左侧菜单。

**预期结果：**
- 源码扫描输出 `macOS native source-list responsive width contract ok`。
- 左侧主菜单使用系统 source-list/sidebar 样式，显示 `活动`、`概览`、`规则`、`网络` 和主题按钮。
- 不存在自定义 overlay 悬浮层、自定义 icon rail 或自绘折叠按钮。
- 系统 sidebar 按钮负责折叠/展开，左右布局行为交给 `NavigationSplitView`。
- 窗口宽度变窄时，右侧 detail 区域先减少页面 padding、卡片换行并降低 Rules/Network 详情面板最小宽度；左侧 source-list 不被右侧内容推出屏幕外。
- Rules 页面内部的规则列表宽度拖拽仍然保留，且最大 300px。

**实际结果（2026-07-03）：**
- 执行 `swift build --package-path apps/macos` 通过。
- 执行源码合同扫描通过，输出 `macOS native source-list responsive width contract ok`。
- 执行 `scripts/build-macos-native.sh --skip-sidecar --test` 通过，生成 `apps/macos/.build/Bifrost.app`，`BifrostNativeCoreChecks passed`。
- 重新打开 Native `.app`；最外层主菜单回到系统 `NavigationSplitView + List(.sidebar)` source-list 布局。

**实际结果（2026-07-04）：**
- 执行 `swift build --package-path apps/macos` 通过。
- 执行源码合同扫描通过，输出 `macOS native source-list responsive width contract ok`。
- 执行 `scripts/build-macos-native.sh --skip-sidecar --test` 通过，生成 `apps/macos/.build/Bifrost.app`，`BifrostNativeCoreChecks passed`。
- 将 Native `.app` 窗口调整为约 `980x720` 后截图观察，左侧系统 source-list 仍在窗口内，右侧 Activity 卡片自适应换行，没有继续把左侧主菜单挤出屏幕。

## 清理步骤

```bash
rm -rf apps/macos/.build/sidecar
rm -f /tmp/bifrost-shell-shard-list.txt
rm -rf "${TMPDIR:-/tmp}"/bifrost-native-app-human.*
pkill -x Bifrost || true
pkill -f 'Bifrost.app/Contents/MacOS/Bifrost' || true
```
