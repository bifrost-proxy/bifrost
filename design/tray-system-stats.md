# Tray 系统状态展示

## 功能模块详细描述

Tray 新增 macOS 全系统状态展示能力，默认开启。macOS 上优先在菜单栏 status item 中以模板图像常驻展示紧凑状态，避免用户每次点开菜单才能看到：

- Bifrost 原图标保持与未启用系统状态时一致的视觉大小。
- 图标右侧使用接近系统监控菜单的常规系统字体绘制单行状态，例如 `C20% | M55% | D55% | ↑1.5 M/s ↓512 K/s`。CPU/Memory/Disk 分别缩写为 `C/M/D` 并放在数值前；上传/下载网速作为一个整体字段展示，中间仅用小空格分隔，不再用竖线拆开。
- 单行布局在 36px 透明模板图内使用 28px 字体，尽量吃满 macOS status item 可用高度，避免两行文字被系统缩放后过小；列宽由渲染器预留稳定空间，不依赖左侧补零。

这些指标表示整台机器的状态，不表示 Bifrost 进程自身资源消耗，也不表示 Bifrost 代理流量聚合。Windows 明确不支持 Tray 系统信息：不采样 CPU、内存、磁盘或网速，不展示菜单详情，不暴露 Settings 系统信息配置项。原因是 Windows notification area 对图标信息承载有限，把系统信息放进点击菜单会造成体验和性能折扣。

Settings 新增独立 `Tray` tab。macOS 展示 `Tray Icon` 与系统状态总开关/子项开关；Windows 只展示 `Tray Icon` 一个配置项。

## 实现逻辑

### 配置模型

在 `bifrost-storage::TrayConfig` 中增加：

- `enabled: bool`：是否启用 tray helper，沿用既有语义。
- `show_system_stats: bool`：是否在 tray 菜单展示系统状态，默认 `true`。
- `system_stats_items`：系统状态子项开关，包含 `cpu`、`memory`、`disk`、`upload`、`download`，每项默认 `true`；缺省字段也按 `true` 处理，确保老配置或部分手写配置不会误关其它指标。

Admin API `/api/config/tray` 额外返回 `system_stats_supported`：

- macOS：`system_stats_supported=true`，`show_system_stats` 与 `system_stats_items` 按配置返回；`PUT` 支持只更新 `enabled`、只更新 `show_system_stats`，或只更新某一个 `system_stats_items` 子字段。
- Windows：`system_stats_supported=false`，响应中 `show_system_stats=false` 且所有 `system_stats_items` 为 `false`；`PUT` 只允许更新 `enabled`，携带 `show_system_stats` 或 `system_stats_items` 时返回 400。
- Linux：tray helper 本身不支持，`supported=false` 且 `system_stats_supported=false`。

### Tray helper 本地采样

系统状态仅在 macOS 由 `bifrost __tray` helper 本地采样，不经过 Admin API：

- Tray helper 在独立进程运行，即使 Bifrost 主服务繁忙、停止或 Admin API 无响应，也可以继续更新机器状态。
- 采样线程每 1 秒刷新系统状态快照；CPU、内存最多每 2 秒刷新一次并复用最近值，磁盘按 30 秒窗口刷新。
- macOS CPU 使用 Mach `host_statistics(HOST_CPU_LOAD_INFO)` 的 tick delta 计算；菜单栏 `MEM` 使用 `host_statistics64(HOST_VM_INFO64)` + `sysctl HW_MEMSIZE` 得到 `Memory Used` 风格内存负载：计入 anonymous/internal、wired 和 compressed memory，排除 file-backed/speculative/purgeable cache；菜单下拉补充 `kern.memorystatus_level` 换算的 pressure 诊断值，避免为菜单栏状态走较重的通用系统刷新路径。
- macOS 网络接口计数使用 `getifaddrs` 读取 `AF_LINK` 的 `if_data.ifi_ibytes/ifi_obytes`，避免 `sysinfo::Networks::refresh()` 的额外列表刷新成本。
- 磁盘百分比按当前 `data_dir` 所在挂载点计算，避免随便取第一个磁盘导致显示与用户实际数据目录无关。
- 磁盘使用 `statvfs(mount_point)` 读取当前挂载点容量，不在周期路径刷新完整磁盘列表。
- 网络速率优先使用系统默认出站接口。macOS 通过 `route -n get default` 获取 IPv4 默认路由 interface，并在缺失时回退到 `route -n get -inet6 default`；随后按该接口的内核累计字节计数计算吞吐。获取不到默认接口时，才回退到最活跃的非虚拟物理接口。
- 每个接口保存平台累计字节计数，下一次采样用同一接口的累计差值除以 `Instant` 单调时钟间隔，转换为 bytes/sec 后格式化。
- 首次采样、网卡新增/移除、计数回退或采样间隔小于 900ms 时，该接口当前拍不参与速率计算，避免显示虚高尖峰。
- 网络接口过滤 loopback、AWDL、tunnel、bridge、Parallels/VMware/VirtualBox、Docker、Tailscale、ZeroTier 等虚拟/隧道接口，降低 macOS/VM/VPN 场景双算或虚高风险。
- 接口选择带 hysteresis：只有候选接口吞吐超过当前接口 2 倍才切换，避免 Wi-Fi/有线/虚拟接口在相近低流量下频繁抖动；展示值使用当前样本 60%、上一展示值 40% 的指数平滑，降低单样本尖峰。
- 默认路由接口变化时立即清空网络累计基线和平滑值，下一帧重新建立基线，避免从旧 Wi-Fi/有线/VPN 接口残留一段不符合直觉的速度。
- Windows 构建不启动系统状态采样线程，避免无展示价值的后台 CPU、内存、磁盘和网络轮询。

### 网速计算评估结论

可靠网速展示采用“接口累计字节计数差分 / 单调时间”的系统工具口径，而不是按 Bifrost 请求、连接或所有接口简单累加：

- Apple Activity Monitor 的 `Data received/sec` / `Data sent/sec` 也是单位时间内传输数据量；macOS `nettop` 支持 1 秒 delta mode；`netstat -ibn -I <iface>` 暴露接口累计 `Ibytes` / `Obytes`。这些工具都围绕系统维护的累计计数做差分。
- 本机实测默认路由为 `en1`，同时存在 `utun5`、`bridge100/101`、`vmenet*`、`awdl0`、`llw0` 等接口。全接口累加会把 VPN、虚拟机桥接、AWDL/本地链路流量混入用户直觉中的“当前上网速度”，容易双算或出现离谱尖峰。
- 只看 Bifrost 代理流量或 per-process 统计不符合用户目标，因为 Tray 系统状态表示整机当前网络负载，不是 Bifrost 进程自身吞吐。
- 1 秒采样窗口用于优先满足网速实时性；配合默认路由接口选择、短窗口保护和 60/40 EMA 平滑减少瞬时 TCP burst 抖动。

### 性能与实时性权衡

刷新频率不做成用户可见配置，默认系统状态线程 1 秒、CPU/内存 2 秒、磁盘 30 秒、网卡列表 60 秒。理由：

- 1 秒菜单状态刷新让上下行网速更接近系统监控工具；CPU/内存内部缓存按 2 秒刷新，磁盘容量变化和默认路由/网卡列表变化相对慢，分别按 30 秒和 60 秒刷新，避免频繁 I/O 和系统路由查询。
- macOS 菜单栏 `MEM` 使用 Activity Monitor / iStat Menus 风格的 `Memory Used` 近似值：`internal - purgeable + wired + compressor`，即把 compressed memory 计入已用内存，同时排除 file-backed cache / speculative / purgeable 可回收缓存。下拉系统详情保留 `Pressure`、`Compressed`、`Cached` 明细；`Pressure` 只作为健康度诊断，不作为菜单栏主百分比。
- 采样只在 tray helper 本地执行，不访问 Admin API，不阻塞主代理进程。
- 当 `show_system_stats=false` 或所有 `system_stats_items` 子项均关闭时，线程只读取配置并清空菜单状态，不执行 CPU/内存/网络采样；当 Upload/Download 均关闭但 CPU/Memory/Disk 仍开启时，只刷新非网络指标并重置网络累计基线，避免把关闭期间的字节变化折算成当前速率。
- 子项关闭时跳过对应重采样：CPU 关闭时不读取 CPU tick，Memory 关闭时不读取 VM 统计，Disk 关闭或未到 30 秒窗口时不执行 `statvfs`，Upload/Download 均关闭时不读取网络计数。
- 性能风险主要来自 macOS `set_icon` 位图重设，而不是 Mach/getifaddrs/statvfs 采样本身。实现只在菜单栏文本发生变化时更新 status item 图像；CPU/内存/磁盘缓存、接口过滤、固定列宽和平滑共同降低无意义重绘。
- 最终 release 真实采样在所有系统信息展示均启用时执行：`show_system_stats=true` 且 `cpu/memory/disk/upload/download=true`，warm-up 15 秒后采集 120 个 1 秒样本，tray helper 平均 CPU `0.0467%`，最大 CPU `0.7000%`，`over_1=0`、`over_1_5=0`，满足平均低于 1% 且不超过 1.5% 的目标。

若后续真实用户机器上看到明显 CPU 增高，优先排查是否又引入了周期性 AppKit `set_icon` / 原生菜单对象重建；其次再考虑把 CPU/内存/磁盘刷新间隔调高到 5 秒。只有存在明确用户需求时再增加 Settings 中的刷新频率选项。

### 菜单更新

macOS 系统状态进入菜单栏 status item 常驻图像，展开菜单不重复展示 `_system_stats` 与 `_network_stats` 两个 disabled item。Windows 不展示系统信息菜单行。

现有 `NativeMenuState::refresh_in_place` 会对同结构菜单调用原生 item `set_text`，不会频繁替换整个 native menu，避免 macOS/Windows 上展开菜单时被后台刷新关闭。

### macOS 菜单栏常驻标题

`tray-icon 0.19` 在 macOS 上会把 icon 按系统菜单栏高度缩放，因此系统状态开启且服务 Running 时，helper 生成一张透明模板图像作为 status item icon。模板图左侧绘制占满图像高度的 Bifrost template icon，确保开启系统状态后图标视觉大小不小于原版；右侧优先用 Arial/SF/Helvetica 常规系统字体绘制 28px 单行状态，例如 `C20% | M55% | D55% | ↑1.5 M/s ↓512 K/s`，并仅做轻量横向叠画以保持清晰度，避免过粗。渲染器按 `100%` 和 `↑999.9 M/s ↓999.9 M/s` 预留稳定列宽，文本不再通过 `05%` 或 `001M/s` 这类补零方式稳定宽度。关闭 `show_system_stats`、关闭全部子项或服务不在 Running 状态时恢复普通 Bifrost 图标。

macOS 展开菜单不再重复展示 `System:` / `Network:` 两排资源信息，避免同一信息同时出现在菜单栏和下拉菜单。Windows notification area 的 `set_title` 在当前 `tray-icon` 实现中是 no-op，系统托盘也没有 macOS 这种可横向常驻文本区域，因此 Windows 不支持 Tray 系统信息，而不是用菜单详情降级。

## 依赖项

- `bifrost-cli` 仅在 macOS tray 构建下使用 libc Mach、`getifaddrs`、`statvfs` 与 `sysinfo::Disks` 初始化挂载点；Windows 不引入系统状态采样模块。
- 不新增 Tauri tray 依赖；仍复用现有 `tao` + `tray-icon` helper。

## Windows / macOS 兼容调研

- `tray-icon` 的 `MenuItem`/`CheckMenuItem` 支持 `set_text`，满足普通菜单项原地更新需求；菜单栏 status item 位图不应随系统状态文本高频重设。
- macOS CPU/内存用 Mach host statistics；网络用 `getifaddrs` 暴露的 `if_data.ifi_ibytes/ifi_obytes` 原生累计计数；磁盘用 `statvfs`。
- Windows 虽然可以通过 `GetIfTable2` / `GetIfEntry2` 读取 `InOctets` / `OutOctets`，但产品层面不支持 Tray 系统信息，避免 notification area 菜单展示和后台采样带来的体验/性能折扣。

## 测试方案

### 单元测试

- `bifrost-storage`：`TrayConfig::default()` 默认 `enabled=true`、`show_system_stats=true`，序列化 round-trip 保留字段。
- `bifrost-storage`：部分 `[tray.system_stats_items]` TOML 仍让未声明子项默认开启。
- `bifrost-admin`：`GET /api/config/tray` 与 `GET /api/config` 返回 `system_stats_supported`、`show_system_stats` 和 `system_stats_items`；macOS `PUT /api/config/tray` 支持仅更新系统状态总开关或单个子项，非 macOS 携带系统状态字段时返回 400。
- `bifrost-cli`：
  - 系统状态格式化输出稳定单位。
  - macOS 菜单栏 status item icon 使用原尺寸 Bifrost 图标 + 常规字重单行 `C/M/D/↑/↓` 文案，且按子项开关过滤展示。
  - macOS Tray 菜单不重复展示系统状态两排。
  - Windows 不启动系统状态采样线程。
  - 同结构状态文本更新不改变 native menu shape。
- `web`：macOS Settings Tray tab 展示系统状态开关并通过 API 保存/回读；Windows 只展示 Tray Icon 开关。

### E2E 测试

- 新增 shell E2E 覆盖 `/api/config/tray` 默认值、macOS 关闭/开启 `show_system_stats`、单独关闭/开启 `system_stats_items.download`、关闭/开启 `enabled` 后 config 持久化；非 macOS 覆盖 `system_stats_supported=false`、响应 mask off 和系统状态更新 400。
- 新增/更新 Playwright 覆盖 `Settings -> Tray` tab：macOS 能看到系统状态总开关和 CPU/Memory/Disk/Upload/Download 子开关；Windows/unsupported fixture 只显示 Tray Icon。
- Tray helper smoke 继续使用 `test_cli_tray_startup_ci.sh`；新增系统状态日志或菜单结构断言时必须避免要求无头 Windows runner 长驻原生 tray。

### 真实场景测试

更新 `human_tests/cli-tray-helper.md`：

- macOS：真实启动 tray helper，验证默认开启、Settings Tray tab 开关、Admin API 持久化和性能基线；通过截图或辅助功能确认菜单栏常驻状态、原图标视觉大小、单行常规字重文本、列宽稳定和上下行网速作为一组完整展示。
- Windows：仅验证 `GET /api/config/tray` 返回 `system_stats_supported=false`、系统状态全部 mask off；Settings Tray tab 只展示 `Tray Icon`；携带 `show_system_stats` 或 `system_stats_items` 的 `PUT` 返回 400；tray helper 日志中不应出现系统状态采样线程。不做 Windows 资源状态展示、菜单详情或截图验收。

## Review/Fix/Test 闭环方案

第 1 轮：

- 复核用户目标：macOS 默认开启、常驻展示、全系统指标、Settings 独立 Tray tab；Windows 不支持系统信息且只保留 Tray Icon 配置。
- Review 修改文件：配置/API、tray helper、Settings UI、测试与文档。
- 运行最小测试：相关 Rust 单元测试、web UI 单测/Playwright、shell E2E。

第 2 轮：

- 复查第 1 轮修复后的 diff，确认没有把 Bifrost 进程指标误用为系统指标。
- 复跑受影响测试，执行 human_tests，补齐 coverage 门禁。

## 校验要求

- 必须按仓库规则执行 human_tests。
- 必须执行 `make coverage`；若 E2E coverage 环境不可用，执行 `make coverage-unit` 并说明原因。
- 收尾前按 `rust-project-validate` 执行格式、clippy、build、`cargo test --workspace --all-features`。
- 默认提交、推送、创建/更新 PR，并使用 GitHub Actions PAT fail-fast 看护 CI。
