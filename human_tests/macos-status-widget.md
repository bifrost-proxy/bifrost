# macOS 原生状态小组件

## 功能模块说明

验证 Bifrost 的原生 macOS WidgetKit 小组件能在桌面小组件图库中被发现，并以系统 Liquid Glass 背景展示 CPU、内存、磁盘最近采样、全局代理状态和更新时间。小组件只提供中号尺寸，超过 30 分钟的数据必须降级为陈旧状态。

## 前置条件

1. 使用 macOS 26 或更新版本。
2. 在仓库根目录执行：
   `APPLE_SIGNING_IDENTITY=- pnpm exec tauri build --config desktop/src-tauri/tauri.conf.json --bundles app`
3. 执行：
   `APPLE_SIGNING_IDENTITY=- bash scripts/resign-macos-app.sh desktop/src-tauri/target/release/bundle/macos/Bifrost.app`
4. 执行：
   `pluginkit -a desktop/src-tauri/target/release/bundle/macos/Bifrost.app/Contents/PlugIns/BifrostStatusWidget.appex`
5. 在 `~/Library/Group Containers/group.com.bifrost.desktop/status.json` 写入 schemaVersion 1 的当前时间测试快照，CPU、内存、磁盘分别设为 42、68、53，proxyStatus 设为 `on`。

## 测试用例

### TC-MSW-01：发布包内嵌原生 WidgetKit 扩展

操作步骤：

1. 执行 `codesign --verify --deep --strict desktop/src-tauri/target/release/bundle/macos/Bifrost.app`。
2. 执行 `pluginkit -m -A -D -i com.bifrost.desktop.status-widget`。
3. 检查 App、sidecar 和 `.appex` 的签名 entitlements。

预期结果：

- App 深度签名验证通过。
- 系统列出 `com.bifrost.desktop.status-widget`。
- App 与 sidecar 包含 `group.com.bifrost.desktop`，Widget 同时包含 App Sandbox 和同一 App Group。

### TC-MSW-02：小组件图库与尺寸

操作步骤：

1. 在 macOS 桌面右键，选择“编辑小组件”。
2. 在小组件图库搜索 `Bifrost`。
3. 选中 Bifrost Status。

预期结果：

- 图库能发现 Bifrost Status。
- 只出现中号尺寸，不出现小号或大号版本。
- 图库预览包含三个环形占比和底部状态行。

### TC-MSW-03：占比、代理与更新时间

操作步骤：

1. 将 Bifrost Status 添加到桌面。
2. 等待 WidgetKit 完成首次渲染。
3. 对照测试快照检查三个指标、代理状态和更新时间。

预期结果：

- CPU、Memory、Disk 分别显示 42%、68%、53%。
- 每项使用独立圆环、SF Symbol、百分比和标签，视觉层级与参考图一致。
- 底部显示 Global proxy on 和相对更新时间。

### TC-MSW-04：Liquid Glass 与可读性

操作步骤：

1. 在浅色桌面外观下观察小组件背景、圆环和文字。
2. 切换到深色桌面外观，再次观察。
3. 将小组件放在包含明暗细节的墙纸区域。
4. 在 macOS 的小组件外观设置中分别选择 Clear/Tinted 与 Full Color（若系统提供），
   返回桌面观察背景与内容。

预期结果：

- Clear/Tinted 外观下，系统移除 Widget 声明的可移除背景并提供 Liquid Glass；第三方
  Widget 不自行调用 `.glassEffect`。
- Full Color 外观下允许系统提供普通不透明容器背景；Bifrost 不承诺或强制玻璃效果。
- 浅色和深色外观下文字、图标、进度轨道均清晰。
- 三个圆环使用 Bifrost 绿色或阈值警告色，系统着色模式可接管强调内容。

### TC-MSW-05：陈旧数据降级

操作步骤：

1. 将测试快照 sampledAtMs 改为当前时间 30 分钟以前。
2. 让 WidgetKit 重载时间线后观察。

预期结果：

- 指标保留最近数值但使用次要色显示。
- 代理胶囊显示 Proxy status stale 和时钟警告图标。
- 更新时间仍显示最近一次采样时间，不伪装成实时数据。

### TC-MSW-06：点击打开 Bifrost

操作步骤：

1. 点击桌面上的 Bifrost Status 小组件。

预期结果：

- 系统尝试打开 `bifrost://settings`。
- 已安装 Bifrost 时进入 Settings；未安装协议处理器时只出现系统无法打开提示，不影响小组件自身渲染。

### TC-MSW-07：ad-hoc 安装包共享数据回归

操作步骤：

1. 在没有 Apple Developer 签名身份的机器上，以 `APPLE_SIGNING_IDENTITY=-` 构建并安装 Bifrost。
2. 启动 `/Applications/Bifrost.app`，等待至少 17 秒。
3. 检查 App Group 与
   `~/Library/Containers/com.bifrost.desktop.status-widget/Data/Library/Application Support/Bifrost/status.json`
   的修改时间和 JSON 内容。
4. 让 WidgetKit 重载时间线，检查生成的 `.chrono-timeline` 文本内容。

预期结果：

- Widget 扩展仍启用 App Sandbox。
- 两处快照都按 5 秒节流周期更新，schema 和指标内容一致。
- App Group 因 ad-hoc Team 身份不可读时，扩展从自身容器副本成功读取。
- 时间线包含 CPU、Memory、Disk 数值、真实代理状态和更新时间，不显示 `--` 或
  `Open Bifrost to collect data`。

### TC-MSW-08：宿主主动刷新与代理变化刷新

操作步骤：

1. 确认 `/Applications/Bifrost.app` 正在运行，记录 Widget 当前显示的相对更新时间。
2. 检查 `~/.bifrost/logs/desktop-bootstrap.log`，确认宿主启动 5 秒后记录主动刷新请求。
3. 等待不超过 75 秒，检查宿主日志出现下一条主动刷新请求，并检查扩展沙箱
   `Bifrost/timeline.log` 出现新的 `getTimeline`。
4. 切换一次 Bifrost 全局代理状态，并记录切换时间。
5. 在下一次系统接受的 WidgetKit 刷新后检查代理胶囊状态。
6. 检查 Bifrost 主进程、9900 监听和 helper 进程，确认 helper 已退出且没有常驻或僵尸进程。

预期结果：

- Widget timeline 使用 `.after(now + 5 秒)` 持续提出下一次刷新请求；macOS 可按系统预算
  合并实际执行。
- `bifrost-desktop` 本体在启动后 5 秒及每 60 秒直接记录并调用一次
  `WidgetCenter.reloadTimelines`；不再依赖 helper → URL Scheme 往返。
- WidgetKit 仍可按系统预算合并请求；扩展日志证明系统实际执行 timeline，不把相对时间
  文字每秒重绘误认为 timeline 每秒重载。
- 全局代理变化写入下一份 5 秒快照，并在下一次系统接受的刷新中反映。
- `bifrost-widget-reloader` 每次调用后退出，不常驻、不累积僵尸进程。
- helper 缺失或失败不影响 Bifrost 主进程及 9900 代理监听，Widget 自身 timeline 仍作为
  主刷新链路。

### TC-MSW-09：退出托盘不停止小组件采样

操作步骤：

1. 启动 `/Applications/Bifrost.app`，确认核心、托盘进程和 9900 监听均存在。
2. 记录 `~/Library/Group Containers/group.com.bifrost.desktop/status.json` 的修改时间。
3. 退出 Bifrost 托盘进程，但保持 Bifrost Desktop 和代理核心运行。
4. 在 20 秒内再次读取快照修改时间，同时检查核心 PID 和 9900 监听。

预期结果：

- 托盘进程退出。
- 核心进程和 9900 监听保持运行。
- 快照修改时间在约 5 秒内继续前进；小组件数据生产不再依赖托盘生命周期。

### TC-MSW-10：首次 CPU 快照不是未采样的零值

操作步骤：

1. 重启 Bifrost 核心并删除或记录旧快照。
2. 等待首个新快照写入。
3. 读取 JSON 的 `cpuPercent`，并在活动监视器或 `ps` 中确认系统存在 CPU 活动。

预期结果：

- 发布器先积累至少 1 秒 CPU tick 采样窗口，再写首个快照。
- 首帧不是因缺少 tick 差值而生成的合成 `0%`；真实系统恰好空闲时允许测得 0%。

## 实际执行记录

执行日期：2026-07-25

| 用例 | 状态 | 实际结果 |
| --- | --- | --- |
| TC-MSW-01 | 通过 | `codesign --verify --deep --strict` 通过；`pluginkit` 列出 0.0.165 扩展；App、sidecar、Widget entitlements 与预期一致。 |
| TC-MSW-02 | 通过 | macOS 26.4.1 WidgetKit Simulator 的图库发现 Bifrost；源码和编译契约确认仅支持 `.systemMedium`，预览包含三项环形指标与底部状态行。 |
| TC-MSW-03 | 通过 | 已安装扩展生成的真实 `.chrono-timeline` 包含 CPU 33%、Memory 63%、Disk 39%、Global proxy on 与 `Updated 2026/7/25, 18:57`，没有等待数据文案。 |
| TC-MSW-04 | 部分通过 | 用户实机确认系统 Widget 可呈 Liquid Glass，但 Bifrost 在当前系统选择的 full-color 渲染中呈白底；手动 `.glassEffect` 被 WidgetKit 快照成白底并破坏内容，已依据 Apple WidgetKit 文档回退为可移除透明容器。Clear/Tinted 模式下由系统决定并生成玻璃，应用不能强制。 |
| TC-MSW-05 | 通过 | 陈旧预览保留三个数值并降为次要色，代理胶囊显示 Proxy status stale；三个指标的辅助功能提示均为 out of date。 |
| TC-MSW-06 | 通过 | 执行与 Widget `.widgetURL` 相同的 `bifrost://settings`，已安装 Bifrost 被唤起且 WebView 路由为 `tauri://localhost#/settings`。 |
| TC-MSW-07 | 通过 | ad-hoc Widget 保持 App Sandbox；两处快照在 17 秒观察窗内同时从 18:57:38 更新到 18:57:52；扩展日志只有 App Group 首选路径权限失败、回退路径无读取/解码错误，真实时间线归档包含三项指标、代理开启和更新时间。 |
| TC-MSW-08 | 通过 | 扩展日志从 23:28:12 到次日 00:08:45 连续记录 `getTimeline`；绝大多数间隔 64 秒，偶有 65 秒。宿主日志同期每 60 秒记录主动请求，确认不是仅重启刷新。 |
| TC-MSW-09 | 通过 | 托盘已退出时复测：核心 PID 45029 与 9900 监听保持运行，App Group 快照 mtime 在观察窗内前进 6 秒，确认约 5 秒发布链路不依赖托盘。 |
| TC-MSW-10 | 通过 | 加入 1 秒 CPU tick 预热后，重装首轮快照读取到 CPU 15.07%，后续持续为真实采样；未再缓存启动占位 0%。 |

## 清理步骤

1. 从桌面移除测试小组件。
2. 执行 `pluginkit -r desktop/src-tauri/target/release/bundle/macos/Bifrost.app/Contents/PlugIns/BifrostStatusWidget.appex` 取消测试构建注册。
3. 删除本次测试写入的 `~/Library/Group Containers/group.com.bifrost.desktop/status.json`；保留正式 Bifrost 已创建的其他数据。
