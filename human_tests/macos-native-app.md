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
- Native 使用 SwiftUI `NavigationSplitView` + source-list 行的左侧主导航，不再使用顶部三段式主 tab。
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
   rg -n 'Remote Invoke|生成 SSH Key|复制 SSH Key|证书与移动端|安装本机 CA|刷新设备|重新生成 QR|可用性检查|QRPreview|MobileDeviceRow|TrustProbeDeviceRow|RemoteInvokeGrantRow' \
     apps/macos/Sources/Bifrost/Features/Dashboard/DashboardView.swift
   ```
5. 打开 Native `.app` 并进入 `概览`：
   ```bash
   scripts/build-macos-native.sh --skip-sidecar --test
   open -n apps/macos/.build/Bifrost.app
   ```

**预期结果：**
- `--check-settings-data` 输出 `cert_status=`、`mobile_android=`、`mobile_ios=`、`remote_state=`、`grants=`、`calls=`、`ssh_key=`、`trust_probe_host=` 和 `trust_probe_qr=true`。
- Overview 的 Remote Invoke 面板不再只是发现模式开关；必须展示 SSH Key 状态、生成/重新生成按钮、复制 SSH Key 按钮、已授权客户端数、活动调用数、最近调用数、最近活跃时间，以及最多 3 个授权客户端/最近调用摘要。
- Overview 的证书与移动端面板必须展示本机 CA 状态、代理地址、移动设备数、证书指纹、`安装本机 CA`、`刷新设备`、`重新生成 QR`。
- 可用性检查区域必须展示二维码图片容器、检查链接、打开/复制链接操作；扫码后 `trustProbeSession.devices` 中的正在连接设备必须在 `TrustProbeDeviceRow` 中显示网络/TLS/代理状态。
- USB/ADB/cfgutil 发现的移动设备必须在同一证书面板内显示设备名称、平台图标、证书信任状态或设备状态；没有设备时显示明确空态和扫码引导。
- 这些能力都在 `概览` 一级页面内完成，不要求用户再进入完整 Settings 页或 WebUI 才能看到基础状态。

**执行记录：**
- 2026-07-03：执行 `swift build --package-path apps/macos` 通过。
- 2026-07-03：执行 `scripts/build-macos-native.sh --skip-sidecar --test && apps/macos/.build/Bifrost.app/Contents/MacOS/Bifrost --check-settings-data` 通过，输出包含 `cert_status=installed_and_trusted`、`mobile_android=0`、`mobile_ios=0`、`remote_state=Connected`、`grants=2`、`calls=5`、`ssh_key=true`、`trust_probe_host=10.71.185.109`、`trust_probe_qr=true`。
- 2026-07-03：执行源码检查命令，确认 Overview 具备 `RemoteInvokeGrantRow`、`MobileDeviceRow`、`AvailabilityProbePanel`、`QRPreview`、`TrustProbeDeviceRow`，并通过 `BifrostClient` 拉取 mobile/proxy/trust-probe/Remote Invoke SSH key/grants/calls。

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

## 清理步骤

```bash
rm -rf apps/macos/.build/sidecar
rm -f /tmp/bifrost-shell-shard-list.txt
pkill -x Bifrost || true
pkill -f 'Bifrost.app/Contents/MacOS/Bifrost' || true
```
