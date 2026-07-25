# macOS 系统状态小组件

## 背景

Bifrost 当前依赖菜单栏托盘承载运行状态和系统统计。MacBook 刘海、第三方状态项和较窄屏幕会压缩菜单栏空间，导致 Bifrost 托盘项被系统隐藏。原生 macOS 桌面小组件不占用菜单栏空间，适合提供可扫读的最近一次系统状态快照。

本方案使用 WidgetKit + SwiftUI 实现原生 macOS 中号小组件。小组件由系统负责生命周期和背景材质，Bifrost 核心进程负责把最新快照写入共享 App Group。WidgetKit 不持续运行，因此这里的 CPU、内存和磁盘数据均明确是最近一次采样，不宣称为实时监控。发布器不能依赖菜单栏托盘生命周期：用户退出托盘后，只要代理核心仍在运行，快照仍需继续更新。

## 用户目标验证清单

### 必须实现

- 提供原生 macOS WidgetKit 小组件，并随 Bifrost Desktop App 一起打包。
- 使用三个等宽圆环展示 CPU、内存和磁盘占用百分比；圆环内使用对应 SF Symbol，圆环下显示整数百分比。
- 展示系统全局代理状态，至少区分已开启、已关闭、检查中/未知和不支持。
- 展示快照更新时间；数据超过 30 分钟时明确显示为陈旧状态。
- 使用系统提供的 Liquid Glass 小组件背景。在支持 Liquid Glass 的系统上由 WidgetKit 替换为主题玻璃或着色效果；旧版 macOS 使用系统材质回退。
- 点击小组件通过 `bifrost://settings` 打开 Bifrost 设置页。

### 必须不破坏

- 不改变现有菜单栏托盘、原生统计面板、系统代理开关和 Bifrost Desktop 主窗口行为。
- 不新增第二套 CPU、内存和磁盘采样算法；复用 `SystemStatsSampler` 的 macOS 数据。
- 小组件不可用、App Group 不可写或扩展加载失败时，不得影响代理和托盘主流程。
- 不为 Liquid Glass 叠加自绘透明背景；避免降低对比度和破坏系统的降低透明度设置。
- macOS 以外平台的 CLI、Desktop 构建和发布行为保持不变。

### 必须真实验证

- 构建产物包含 `Contents/PlugIns/BifrostStatusWidget.appex`，主 App 与扩展分别携带正确的 App Group entitlement。
- 核心进程使用隔离的测试目录写出合法 JSON 快照，字段包含 schema、采样时间、三个百分比和代理状态。
- 退出菜单栏托盘但保持代理核心运行后，快照修改时间仍按约 5 秒周期前进。
- 在 macOS 桌面添加 Bifrost 小组件后，三个圆环、代理状态和更新时间可见。
- 开启与关闭系统代理后，下一次 WidgetKit 刷新读取到一致状态。
- 浅色、深色、Clear/Tinted 小组件外观下信息可读；开启“降低透明度”和“提高对比度”后仍可辨识。
- 快照缺失、损坏或超过 30 分钟时显示占位符/陈旧提示，而不是伪造 0%。

### 必须交付

- WidgetKit Swift 源码、Info.plist、App/扩展 entitlements、构建与签名脚本。
- Rust 快照发布实现及单元测试。
- macOS bundle E2E 契约测试与 `human_tests/macos-status-widget.md`。
- 两轮 Review/Fix/Test、本地适用验证、提交、PR 和远端 CI 看护。

## Apple 平台约束

Apple 的 WidgetKit 文档明确说明：小组件扩展不是持续活动进程，而是通过时间线提供快照；常见刷新预算约为每天 40–70 次，时间线条目建议至少相隔约 5 分钟，系统也可能合并或推迟刷新。因此：

- 核心进程每 5 秒最多写一次共享快照，让 WidgetKit 每次真正刷新时都能读取近期数据。
- Widget timeline 使用 `.after(now + 5 秒)` 请求下一次新条目；macOS 26.4.1 实测会把请求
  稳定合并为约 64–65 秒一次。UI 必须展示真实采样时间，不能把 timeline 渲染时间冒充
  采样时间。
- CPU 环只表示最近采样值。内存和磁盘变化较慢，更适合该展示模型。
- 本期不在小组件中显示瞬时网速，也不把小组件包装成活动监视器。

参考：

- [Keeping a widget up to date](https://developer.apple.com/documentation/widgetkit/keeping-a-widget-up-to-date)
- [Optimizing your widget for accented rendering mode and Liquid Glass](https://developer.apple.com/documentation/widgetkit/optimizing-your-widget-for-accented-rendering-mode-and-liquid-glass)
- [Applying Liquid Glass to custom views](https://developer.apple.com/documentation/swiftui/applying-liquid-glass-to-custom-views)

## 数据架构

```text
SystemStatsSampler + admin proxy snapshot
                  |
                  v
core-owned WidgetSnapshotPublisher (5s / proxy state change)
         |                         |
         v                         v
App Group/status.json       desktop host timer (5s initial / 60s fallback)
(unique temp + rename)             |
         |                         v
         +--------------> WidgetCenter.reloadTimelines
                                    |
                                    v
                         SwiftUI medium widget
```

共享容器标识为 `group.com.bifrost.desktop`，快照文件为 `status.json`。

```json
{
  "schemaVersion": 1,
  "sampledAtMs": 1780000000000,
  "cpuPercent": 24.0,
  "memoryPercent": 67.0,
  "diskPercent": 53.0,
  "proxyStatus": "on"
}
```

约束：

- 百分比统一裁剪到 `0...100`；无法取得的数据编码为 `null`，SwiftUI 显示 `--`。
- `proxyStatus ∈ { "on", "off", "checking", "unsupported" }`。
- 写入使用同目录唯一临时文件并原子 rename，避免扩展读到半份 JSON。
- 测试可通过 `BIFROST_WIDGET_GROUP_CONTAINER` 覆盖共享目录，禁止 human/E2E 测试污染真实 App Group。
- App Group 写入失败只记录诊断，不终止代理核心。

## 视觉与交互

仅支持 `.systemMedium`，对应用户提供的横向圆环参考：

1. 顶部为三个等宽指标列：CPU、内存、磁盘。
2. 每列包含圆形进度轨道、SF Symbol、粗体整数百分比和短标签。
3. 底部左侧是代理状态胶囊，使用 `network`/`network.slash` 图标和明确文字；右侧是相对更新时间。
4. 底栏最右侧显示 16pt Bifrost Logo，作为低干扰品牌标记，并随系统 Widget 渲染模式适配。
5. 正常状态使用 Bifrost teal 作为强调；警告和危险状态只改变强调色，同时保留数值和辅助功能标签，避免仅靠颜色表达。
6. 小组件使用系统字体、紧凑间距和稳定尺寸，不使用营销式大标题或嵌套卡片。

### Liquid Glass

- 使用 `containerBackground(for: .widget)` 声明可由系统管理和移除的容器背景。
- 在支持 Liquid Glass 的 macOS 上，WidgetKit 根据 Clear/Tinted 外观自动移除该背景并替换为主题玻璃效果。
- 不在整个小组件上额外调用 `glassEffect`，避免在系统小组件玻璃上继续叠加玻璃。
- 使用 `widgetAccentable(_:)` 把圆环和代理开启状态放入强调组，并让 full-color/accented 渲染均保持可读。
- 小组件不叠加 `.thinMaterial` 或 `.glassEffect`，也不把容器背景标记为不可移除。根据
  Apple 的 WidgetKit 约束，Liquid Glass 是由系统在用户选择 Clear/Tinted 小组件外观时
  生成的呈现效果；第三方扩展只能适配该模式，不能自行强制开启。在 full-color 模式下，
  系统仍可使用普通不透明容器背景。
- 正式签名使用 App Sandbox 与 App Group entitlement。本地没有 Developer Team 身份的 ad-hoc
  签名可能无法获得 App Group 文件授权，因此 sidecar 同时镜像快照到 Widget 扩展自己的
  Application Support 容器；扩展始终保持沙箱，并仅在 App Group 读取失败时使用该副本。

## 生命周期与过期语义

- 代理核心存在：持续更新共享快照，是否显示托盘不影响发布器。
- 代理停止：保留最后快照；超过 30 分钟后 Widget 显示陈旧状态和原始更新时间。
- 托盘退出但代理核心仍存在：共享快照继续按约 5 秒周期更新。
- 文件缺失或 JSON 损坏：显示 `--`、代理状态“等待 Bifrost”，并提示打开 Bifrost。
- WidgetKit timeline 每 5 秒提出下一次刷新请求，实际执行频率由系统合并；用户点击小组件
  可立即进入设置页查看真实状态。

## 打包与签名

- `desktop/macos-widget/` 保存 WidgetKit 源码和扩展元数据。
- Tauri 的 macOS `beforeBundleCommand` 调用 `scripts/build-macos-widget.sh`，按当前 target architecture 编译 `.appex`。
- Tauri `bundle.macOS.files` 把扩展放入 `Contents/PlugIns/BifrostStatusWidget.appex`。
- 主 App entitlement 和扩展 entitlement 都包含 `group.com.bifrost.desktop`；扩展额外启用 App Sandbox。
- `scripts/resign-macos-app.sh` 必须先签资源二进制，再用扩展 entitlement 签 `.appex`，最后用主 App entitlement 签外层 App，并执行 deep/strict verify。
- 本机只有 Command Line Tools 而没有完整 macOS SDK 时，可执行 Rust、shell 契约和静态验证；真实 `.appex` 编译由带完整 Xcode 的 macOS CI 和 human test 环境完成。

## 测试方案

### 单元测试

- 快照百分比计算：正常值、总量为 0、超过边界、非有限值。
- 代理状态映射：on/off/checking/unsupported。
- 发布节流：首次写、5 秒到期、代理状态变化立即写。
- CPU 首帧：发布器启动后先积累 1 秒 tick 差值，禁止把尚未形成采样窗口的启动值
  `0%` 写给 WidgetKit 缓存。
- 生命周期隔离：托盘采样循环不再创建 Widget 发布器；核心 runtime-ready 回调只启动一次
  Widget 发布线程。
- 主刷新链路：Widget 自己通过 `.after(now + 5 秒)` 请求新 timeline；参考 MacMonitor 的
  macOS Widget 实现。`bifrost-desktop` 在启动 5 秒后及每 60 秒以宿主 App 身份调用
  `WidgetCenter.reloadTimelines(ofKind:)` 作为兜底，不依赖 URL Scheme 往返。
- 诊断日志：扩展每次实际进入 `getTimeline` 都追加到沙箱 Application Support 下的
  `Bifrost/timeline.log`。实测连续执行间隔通常为 64 秒，偶尔 65 秒。
- 原子写入：隔离目录生成可反序列化 JSON，不残留临时文件。
- Swift 快照解码：合法、缺失、损坏和过期数据。

### E2E

新增 `e2e-tests/tests/test_macos_status_widget_contract.sh`：

- 校验 WidgetKit 源码使用 `.systemMedium`、三个指标、更新时间、`widgetAccentable` 和系统容器背景。
- 校验主 App/扩展 App Group entitlement 一致。
- 使用隔离目录运行 Rust 快照写入测试，校验 JSON schema。
- 在具备完整 Xcode SDK 时编译 `.appex` 并校验 Info.plist；无完整 SDK 的非 macOS环境只执行跨平台静态契约。
- 校验 Tauri macOS bundle 映射到 `Contents/PlugIns`，重签脚本分别应用两个 entitlement。

### human_tests

创建 `human_tests/macos-status-widget.md`，创建后立即执行：

- 添加原生小组件。
- 正常/高占用圆环显示。
- 代理 on/off 状态。
- 更新时间和 30 分钟过期状态。
- 退出托盘后快照仍持续更新。
- Liquid Glass Clear/Tinted、浅色/深色、降低透明度/提高对比度。
- 点击深链。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核用户目标、Apple 刷新限制、App Group 权限和 Liquid Glass 规则。
- 执行 `git status --short`、`git diff` 和必要的 `git diff --cached`。
- Review Rust 写入错误隔离、Swift stale 语义、bundle/signing 顺序和 macOS 以外平台边界。
- 修复问题后运行 Rust 单元测试、Swift/Bundle 契约 E2E 和 human tests。

### 第 2 轮

- 对照第 1 轮问题和最新 diff 再次复核三个圆环、代理状态、更新时间与 Liquid Glass。
- 再次执行 `git status --short`、`git diff` 和必要的 `git diff --cached`。
- 复查新增文件、CI/release 两条 macOS 打包链、entitlement 保留和辅助功能。
- 复跑受影响测试；如仍有问题继续追加轮次。

## 覆盖率与项目校验

- Rust 业务代码必须由单元测试和 macOS E2E 契约覆盖。
- 默认不在本地执行完整 coverage；远端 CI 运行 `bash scripts/ci/coverage-all.sh --json --gate`，按 `scripts/ci/coverage-thresholds.toml` 的棘轮门禁兜底。
- E2E 优先于 `rust-project-validate` 执行。
- 收尾必须运行 `rust-project-validate` 和至少一次 `cargo test --workspace --all-features`；local-ci 根据最终改动范围和成本决定，并记录证据。
