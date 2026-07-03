# macOS Native App 方案

## 目标

新增 `apps/macos` 原生 macOS 客户端骨架，作为现有 Tauri 桌面端之外的 Native Preview。第一阶段只建立可验证的工程边界：SwiftUI/AppKit 做控制台体验，Rust `bifrost` CLI 继续作为 sidecar 负责代理、TLS、规则、脚本、存储和 Admin API。

## 非目标

- 不改造或删除 `desktop/` Tauri 桌面端。
- 不重写代理数据面。
- 不引入 Rust FFI、NetworkExtension 或透明代理。
- 不在本切片实现完整 Traffic/Rules 业务交互。

## 目录结构

```text
apps/macos/
  Package.swift
  Project.yml
  Sources/
    Bifrost/              # internal Swift target; executable product is Bifrost
    BifrostNativeCore/
  Tests/
    BifrostNativeCoreTests/
```

SwiftPM 是第一阶段的强制可复现构建路径；`Project.yml` 仅作为后续 XcodeGen 工程入口。

## 实现逻辑

### 安装与自动更新

2026-07-03 新增 Native app 安装与更新链路：

- release workflow 产出独立 Native app DMG，命名固定为 `bifrost-native-v<version>-<target>.dmg`，其中 target 为 `aarch64-apple-darwin` 或 `x86_64-apple-darwin`。
- CLI 新增 `bifrost native-app status/install`：
  - `status` 读取 `/Applications/Bifrost.app/Contents/Info.plist` 的 `CFBundleShortVersionString`，并与 latest version 比较。
  - `install` 支持本地 `Bifrost.app`、本地 `.dmg`、显式 `--url`、`BIFROST_NATIVE_APP_SOURCE`、`BIFROST_NATIVE_APP_URL` 和默认 GitHub release URL。
  - 默认安装目录为 `/Applications`，测试可通过 `--install-dir` 或 `BIFROST_NATIVE_APP_INSTALL_DIR` 改到临时目录。
  - 安装完成后可用 `--open` 自动启动 Native app。
- `bifrost start` 在 macOS 交互式终端、非 detached daemon child、未安装 Native app 时提示安装；非交互、CI、daemon child 或设置 `BIFROST_NATIVE_APP_DISABLE_INSTALL_PROMPT=1` 时不提示，避免自动化被 stdin 阻塞。
- Admin API 新增：
  - `GET /_bifrost/api/system/native-app` 返回安装状态。
  - `POST /_bifrost/api/system/native-app/install` 后台 spawn `bifrost native-app install -y --open`。
- Web UI 启动后读取 Native app 状态，若需要安装则弹出安装提示；用户可选择 Install 或 Later。
- Tray 菜单在 macOS 下显示 `Install Native App` 或 `Open Native App`，安装动作复用 CLI 命令。
- Native Swift app 定时调用 Admin `version-check`；发现新版本后弹原生确认，确认后调用 Admin Native app install API，安装完成后提示用户重启 app。运行中的 Swift 进程不直接覆盖自身，由 CLI helper 替换 `/Applications/Bifrost.app`。

安全边界：

- 安装 helper 支持 dry-run 和测试安装目录，E2E 不写 `/Applications`。
- 安装流程不修改系统代理、不启动 Tray、不打开 Sync 登录页。
- 真实 app 替换使用临时目录 + 备份目录 + rename；失败时尽量恢复旧 app。

### BifrostNativeCore

- `AdminAPIRequestFactory` 统一构造 `/_bifrost/api/` URL、`X-Client-Id`、`Authorization` 和 unsafe method 的 CSRF header。
- `BifrostClient` 作为 actor 包装 URLSession，第一阶段暴露 System、Traffic、Rules、Cert、Proxy 的 Data 级方法，避免在 schema 稳定前过早绑定大量 DTO。
- `SidecarConfiguration` 保持与现有 Tauri 端一致的默认值：默认端口 `9900`、sidecar 绑定 `0.0.0.0`、Admin 探测 `127.0.0.1`、数据目录默认 `~/.bifrost`。
- `SidecarManager` 负责 `ensureRunning`：先通过同一个 `bifrost status --format json` 探测默认数据目录里的已运行 daemon；若 CLI 或其他桌面端已经启动服务，Native app 只消费这个服务，不再启动第二个 daemon。
- 只有没有已运行服务时，`SidecarManager` 才使用 bundled sidecar CLI 以 `bifrost start --daemon` 启动共享默认数据目录的服务。开发默认保留 `--no-system-proxy`、`BIFROST_DISABLE_TRAY=1`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`，避免 Native app 意外改系统代理或拉起托盘。

### Bifrost

2026-07-03 范围收敛：Native UI 不再追求覆盖 WebUI 全量工作台，首屏能力按 Surge 风格收敛为轻量控制台。

- `活动` 展示核心实时指标：活动连接、上传/下载速率、请求数、规则状态、服务状态和应用流量分布。
- `概览` 承载设置类核心操作：系统代理开关、TLS 解密开关、TLS 解包应用/域名/IP 白名单与黑名单数量及弹窗编辑、Remote Invoke 状态/发现开关、SSH Key 生成与复制、已授权客户端/活动调用/最近活跃摘要、同步登录/自动同步/立即同步、证书状态与本机 CA 安装。
- `概览` 的证书与移动端面板必须展示本机 CA 状态、代理地址、证书指纹、安装按钮、已连接移动设备、移动端可用性检查 QR、检查链接复制/打开操作，以及扫码后正在连接的设备状态；不能只保留 Settings 中的完整证书页入口。
- `规则` 保留原生列表、启停和内容编辑保存，并使用与 Activity/Overview 一致的冷白 surface + 白色卡片布局；规则列表和编辑器不能回退到旧的全宽灰色 toolbar、硬分割表格风格。
- `规则` 编辑器使用原生 `NSTextView` bridge 演进出的 `BifrostRuleEditorView`，不引入 Monaco/WebView 或第三方编辑器组件；语言能力由 `BifrostNativeCore/RuleLanguage` 的纯 Swift service 提供，AppKit 层只负责行号、高亮、补全浮窗、Cmd+S、Cmd+Click/F12 导航和文本同步。
- `网络` 不再在 Native 内实现复杂捕获/解密/重写工作台，只提供 Web UI 打开入口和少量状态摘要。
- `Settings` 不作为主导航入口；完整设置页仍保留在 macOS app 的 Settings scene，避免丢失现有实现，但主窗口只暴露核心控制。
- 视觉以 Apple/Surge 风格的清爽冷白 surface 为主：sidebar 可以保留轻量层级感，主内容避免大面积 `NSVisualEffectView` material 灰化；卡片保持白色、轻描边、弱阴影，并带轻微边缘高光/hover 悬浮反馈；窗口按钮必须可见，页面内容不得被标题栏安全区裁切。
- Activity、Overview、Rules、Network 的页面骨架和面板必须复用 `NativeSurface` 共享组件；禁止为某个页面复制一套相似但独立的 card/panel 样式，也禁止恢复旧的 `.bar` 顶部 toolbar 作为主内容操作区。
- 主导航切换必须保持轻量：Activity、Overview、Rules、Network 只刷新当前页面需要的数据，不允许每次切 tab 都全量拉取 overview、rules、system controls 和 traffic；Activity 所需的应用/IP 统计由 `AppModel` 在 traffic 增量合并时缓存，SwiftUI render 路径不能反复遍历 traffic 列表；Native 主窗口最多保留一段轻量 traffic 窗口用于控制台指标，复杂 Network 历史列表继续交给 Web UI。

- 主界面使用 `NavigationSplitView`。
- Activity/Overview/Rules/Network 组成主窗口 release scope。
- Traffic 表格从第一版使用 AppKit `NSTableView` bridge，给后续高吞吐流量列表留出性能边界。
- Rules 编辑器从第一版使用 AppKit `NSTextView` bridge，后续再补规则 DSL 高亮和诊断。
- 对外可执行产品名固定为 `Bifrost`，避免 Dock、App Switcher、Console 或 `open` 启动路径暴露内部工程名。
- 启动时优先从 `.app` bundle 的 `Contents/Resources/bifrost.icns` 加载图标，SwiftPM 直接运行时回退到 `Bundle.module` 里的 `bifrost.png`。`swift run --package-path apps/macos Bifrost --check-icon` 作为无窗口 smoke check，验证资源可加载且尺寸有效。
- 开发启动必须打开 `apps/macos/.build/Bifrost.app`，不能打开裸 `apps/macos/.build/.../debug/Bifrost` Mach-O；后者会被 macOS 当成终端可执行文件启动，不是桌面 app 体验。

## 核心交互 1:1 还原审计

本切片是 PR1 工程骨架，不满足"核心功能 1:1 交互还原"。以下审计结果是合并前的明确边界，避免把构建通过误判为产品体验完成：

| 交互域 | 目标交互 | 当前实现 | 状态 |
| --- | --- | --- | --- |
| Sidecar lifecycle | Start / Stop / Restart、端口冲突回退、复用已有 daemon、health check、watchdog 和日志入口 | `SidecarManager` 已能先复用默认数据目录下已有 daemon，无服务时再 `--daemon` 启动；仍缺 Stop / Restart / watchdog / 日志入口 | 部分还原 |
| Overview | 后端状态、版本、端口、系统代理、TLS/CA、代理地址、Open Web UI 均来自真实 Admin API | Dashboard 大部分是静态 tile，`Open Web UI` 可打开固定 9900 URL | 未还原 |
| Traffic Studio | 真实 Traffic 列表、搜索/过滤、选中详情、headers/body 懒加载、AppKit 表格高性能滚动 | `TrafficView` 使用 `TrafficRecord.sampleRows`，没有 Admin API 加载、选择态、搜索、详情 tab 或 body 懒加载 | 未还原 |
| Rules | 规则列表、编辑、校验、保存、启停、模板和错误反馈 | `RulesView` 只有 `NSTextView` 草稿，Validate/Save disabled，无规则 API 调用 | 未还原 |
| System Proxy / Certificates | 可查看并切换系统代理，查看/安装/信任 CA，展示失败原因 | 只有 Settings/Overview 文案，占位 toggles 使用 `.constant(true)` | 未还原 |
| Devices / Scripts / Replay / Metrics | 对应 sidebar 入口进入真实工作流 | 只有 sidebar 和 placeholder 页面 | 未还原 |
| 基础设施 | Admin API URL/header 构造、sidecar 参数安全开关、AppKit Table/Text bridge、SwiftPM 可构建 | 已实现并通过 `BifrostNativeCoreChecks` 与 smoke 验证 | 已具备骨架 |

因此本 PR 只能作为 Native App scaffold 合并；若验收标准是"Mac Native MVP 核心交互 1:1 可用"，必须继续实现 SidecarManager 可操作化、typed `BifrostClient`、Overview 真实状态、Traffic 真实列表/详情/搜索、Rules 真实 CRUD/启停，以及证书/系统代理真实操作后再交付。

后续从 scaffold 进入 WebUI 全量能力对齐时，以 `design/macos-native-webui-parity.md` 为范围、实现顺序和验收矩阵。任何页面不能只以"入口出现"或"截图相似"标记完成，必须完成真实 Admin API 数据、交互、状态同步和 human_tests 验证。

## 依赖项

- macOS 13+
- SwiftPM / Swift 5.9+
- Rust workspace 中现有 `bifrost-cli`
- 现有 Admin API：`/_bifrost/api/system/overview`、`/traffic`、`/rules`、`/cert`、`/proxy/address`、`/proxy/system`

## 测试方案

### 单元测试

- `AdminAPIRequestFactoryTests`
  - 验证 URL 前缀为 `/_bifrost/api/`。
  - 验证 query 参数构造。
  - 验证 unsafe request 附带 Client/Auth/CSRF/Content-Type headers。
  - 验证相对 path 被拒绝。
- `SidecarConfigurationTests`
  - 验证默认数据目录保持 `~/.bifrost`。
  - 验证 start command 包含 `--skip-cert-check`、`--no-system-proxy` 和安全环境变量。
  - 验证端口候选策略与 Tauri 的 preferred port + 递增窗口一致。
- `macos_native_app` Rust 单元测试：
  - 解析 `Info.plist` 版本。
  - 生成 `bifrost-native-v<version>-<target>.dmg` 资产名。
  - 未安装状态在受支持 macOS target 下标记为需要安装。
- `native_app` CLI 单元测试：
  - 本地 `Bifrost.app` 复制到安装目录。
  - `--dry-run` 不创建目标 app。
- Tray 单元测试覆盖 Native app 菜单项显示。

执行命令：

```bash
swift run --package-path apps/macos BifrostNativeCoreChecks
swift run --package-path apps/macos Bifrost --check-icon
```

### E2E 测试

本切片不改变 Rust daemon 运行行为，不新增代理规则语义。E2E 采用工程 smoke：

```bash
scripts/build-macos-native.sh --test
```

该命令构建 Rust sidecar、复制到 Native app 本地 sidecar 目录，并运行 SwiftPM build 与 `BifrostNativeCoreChecks`。真实代理启动留到 SidecarManager 接入 UI 控制后补充。

Native app 安装链路新增 shell E2E：

```bash
bash e2e-tests/tests/test_macos_native_app_install.sh
```

该用例构造本地假 `Bifrost.app`，使用临时安装目录验证 `bifrost native-app install --dry-run`、真实复制、`native-app status --format json`，不启动代理、不触碰系统代理。

### 真实场景测试

新增 `human_tests/macos-native-app.md`，覆盖：

- SwiftPM build/test。
- sidecar 准备脚本输出可执行文件。
- build 脚本 `--skip-sidecar --test` 可只验证 Swift 工程。
- sidecar 命令计划包含开发安全开关。
- `desktop/` Tauri 文件未被修改。
- Linux shell E2E CI 使用现有 shard 机制拆成 4 个 job，避免全量 shell 套件在单个 60 分钟 job 中超时，同时通过覆盖守卫确认用例不丢失。
- Native app 对外进程/窗口名为 `Bifrost`，且启动时设置 Bifrost app icon。
- Native app 启动时复用默认数据目录里的已有 CLI daemon；无服务时按需启动共享默认数据目录的 daemon。
- 主窗口各页面复用 `NativeSurface`，Rules 不再维护单独的 `RuleSurfaceCard`，旧顶部 toolbar 代码不保留。
- 主窗口四个核心入口切换时按页刷新，Activity 使用缓存流量统计，避免切 tab 卡顿或延迟渲染。
- Overview 的 Remote Invoke 面板展示 SSH Key 状态、生成/复制操作、已授权客户端数、活动调用数、最近调用和最近活跃时间；证书面板展示本机 CA、移动设备、trust-probe QR 与扫码设备状态。
- Overview 的 TLS 解密面板展示应用、域名、IP 三类 include/exclude 名单数量；点击任一计数块弹出编辑框，每行一个规则并保存回 `/config/tls`。
- Rules 编辑器覆盖 Bifrost DSL 注释、`@rule`、`reqScript://`、`resScript://`、`bp://`、`${value}`/`{value}`、`key=value`、`` ```blockVariable ``、正则和 code fence/line block 的 tokenizer；补全数据来自 Rules/Values/Scripts Admin API；本地变量同时来自 `key=value` 与 fenced block 变量定义；本阶段 Values/Scripts 目标在主导航未暴露时 fallback 到 Web UI。
- CLI 启动时在 macOS 交互式终端提示安装 Native app，非交互或禁用环境变量时不提示。
- Web UI 弹出 Native app 安装提示，点击 Install 后 Admin API 启动后台安装。
- Tray 菜单显示 Native app 安装/打开入口。
- Native app 定时检查版本，发现新版本后确认安装并提示重启。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：新增 native client，不改 Tauri，不做 FFI/NetworkExtension。
- 执行 `git status --short`、`git diff`。
- Review `apps/macos`、`scripts/build-macos-native.sh`、`scripts/prepare-macos-native-sidecar.sh`、`docs/macos-native.md`、`human_tests/macos-native-app.md`。
- 复跑 `swift run --package-path apps/macos BifrostNativeCoreChecks`、`swift run --package-path apps/macos Bifrost --check-icon` 和脚本 smoke。

### 第 2 轮

- 复查第 1 轮修复后的 diff 与 human_tests 索引。
- 确认脚本不启动系统代理，不拉起托盘，不触碰 Tauri 配置。
- 复跑受影响命令，并确认是否需要第 3 轮。

## 校验要求

- `swift run --package-path apps/macos BifrostNativeCoreChecks`
- `swift run --package-path apps/macos Bifrost --check-icon`
- `scripts/build-macos-native.sh --skip-sidecar --test`
- `bash e2e-tests/tests/test_macos_native_app_install.sh`
- `scripts/prepare-macos-native-sidecar.sh --skip-cargo-build` 在已有 binary 时可复用
- `cargo fmt --all -- --check`
- `cargo fmt --manifest-path desktop/src-tauri/Cargo.toml --all -- --check`
- `cargo test --workspace --all-features`
- `cargo build --all-targets --all-features`
- `make coverage` 或记录覆盖率门禁对 Swift scaffold 的不适用/阻塞原因
- `bash scripts/ci/check-e2e-shell-ci-coverage.sh`
- YAML 检查确认 `.github/workflows/ci.yml` 的 Linux `e2e-shell` job 配置 `matrix.shard: [1, 2, 3, 4]`

## 文档更新要求

- 新增 `docs/macos-native.md`。
- 更新 `human_tests/readme.md` 索引。
- 当前不修改 README 顶层安装入口，避免把 Native Preview 误写成正式替代路径。
