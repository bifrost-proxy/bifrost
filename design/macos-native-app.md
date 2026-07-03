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

### BifrostNativeCore

- `AdminAPIRequestFactory` 统一构造 `/_bifrost/api/` URL、`X-Client-Id`、`Authorization` 和 unsafe method 的 CSRF header。
- `BifrostClient` 作为 actor 包装 URLSession，第一阶段暴露 System、Traffic、Rules、Cert、Proxy 的 Data 级方法，避免在 schema 稳定前过早绑定大量 DTO。
- `SidecarConfiguration` 保持与现有 Tauri 端一致的默认值：默认端口 `9900`、sidecar 绑定 `0.0.0.0`、Admin 探测 `127.0.0.1`、数据目录默认 `~/.bifrost`。
- `SidecarManager` 负责 `ensureRunning`：先通过同一个 `bifrost status --format json` 探测默认数据目录里的已运行 daemon；若 CLI 或其他桌面端已经启动服务，Native app 只消费这个服务，不再启动第二个 daemon。
- 只有没有已运行服务时，`SidecarManager` 才使用 bundled sidecar CLI 以 `bifrost start --daemon` 启动共享默认数据目录的服务。开发默认保留 `--no-system-proxy`、`BIFROST_DISABLE_TRAY=1`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`，避免 Native app 意外改系统代理或拉起托盘。

### Bifrost

2026-07-03 范围收敛：Native UI 不再追求覆盖 WebUI 全量工作台，首屏能力按 Surge 风格收敛为轻量控制台。

- `活动` 展示核心实时指标：活动连接、上传/下载速率、请求数、规则状态、服务状态和应用流量分布。
- `概览` 承载设置类核心操作：系统代理开关、TLS 解密开关、远程调用发现状态/开关、同步登录/自动同步/立即同步、证书状态与本机 CA 安装。
- `规则` 保留原生列表、启停和内容编辑保存，并使用与 Activity/Overview 一致的冷白 surface + 白色卡片布局；规则列表和编辑器不能回退到旧的全宽灰色 toolbar、硬分割表格风格。
- `网络` 不再在 Native 内实现复杂捕获/解密/重写工作台，只提供 Web UI 打开入口和少量状态摘要。
- `Settings` 不作为主导航入口；完整设置页仍保留在 macOS app 的 Settings scene，避免丢失现有实现，但主窗口只暴露核心控制。
- 视觉以 Apple/Surge 风格的清爽冷白 surface 为主：sidebar 可以保留轻量层级感，主内容避免大面积 `NSVisualEffectView` material 灰化；卡片保持白色、轻描边、弱阴影，并带轻微边缘高光/hover 悬浮反馈；窗口按钮必须可见，页面内容不得被标题栏安全区裁切。
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
- 主窗口四个核心入口切换时按页刷新，Activity 使用缓存流量统计，避免切 tab 卡顿或延迟渲染。

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
