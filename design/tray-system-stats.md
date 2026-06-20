# Tray 系统状态展示

## 功能模块详细描述

Tray 新增 macOS 全系统状态展示能力，默认开启。macOS 上优先在菜单栏 status item 中以模板图像常驻展示紧凑状态，避免用户每次点开菜单才能看到：

- Bifrost 原图标保持与未启用系统状态时一致的视觉大小。
- 图标右侧使用等宽字体展示 `Cxx% | Mxx% | Dxx% | ↑nnnU/s | ↓nnnU/s`，其中 `C/M/D` 分别表示 CPU、Memory、Disk。
- 百分比不足两位时补 `0`，网速数字始终三位；等宽数字避免 `1/8/0` 字宽不同造成菜单栏左右抖动。

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
- 采样线程每 3 秒刷新菜单栏系统状态；CPU、内存和磁盘每 3 秒刷新一次并复用最近值。
- 使用同一个 `sysinfo::System`、`sysinfo::Networks` 与 `sysinfo::Disks` 实例循环刷新，避免 CPU 与网络差值类指标在重建实例时失真。
- 磁盘百分比按当前 `data_dir` 所在挂载点计算，避免随便取第一个磁盘导致显示与用户实际数据目录无关。
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
- 3 秒采样窗口比 1 秒更稳，仍能反映菜单栏趋势；配合 60/40 EMA 平滑可减少瞬时 TCP burst 抖动。若未来需要更接近 Activity Monitor 的实时曲线，可把窗口降到 1 秒，但 CPU 与重绘开销会增加，且菜单栏文本更抖。

### 性能与实时性权衡

刷新频率不做成用户可见配置，默认系统状态线程 3 秒、CPU/内存 3 秒、磁盘 30 秒、网卡列表 60 秒。理由：

- 3 秒刷新能明显降低 tray helper 空闲 CPU，同时仍足够观察菜单栏趋势；CPU/内存按 3 秒刷新，网速每 3 秒按累计字节差分刷新。磁盘容量变化和默认路由/网卡列表变化相对慢，分别按 30 秒和 60 秒刷新，避免频繁 I/O 和系统路由查询。
- 采样只在 tray helper 本地执行，不访问 Admin API，不阻塞主代理进程。
- 当 `show_system_stats=false` 或所有 `system_stats_items` 子项均关闭时，线程只读取配置并清空菜单状态，不执行 `sysinfo` CPU/内存/网络采样；当 Upload/Download 均关闭但 CPU/Memory/Disk 仍开启时，只刷新非网络指标并重置网络累计基线，避免把关闭期间的字节变化折算成当前速率。
- 子项关闭时跳过对应重采样：CPU/Memory 均关闭时不刷新 `sysinfo::System`，Disk 关闭或未到 30 秒窗口时不刷新磁盘列表，Upload/Download 均关闭时不刷新网络计数。
- Mac 本地 release microbench 使用 `sysinfo 0.31.4` 连续执行 500 次采样，单次平均 1.1796ms、最大 1.8934ms；按采样耗时估算，1s/2s/3s 刷新分别约为 0.1180% / 0.0590% / 0.0393% 单核 CPU。
- Mac debug tray helper 真实进程 warm-up 5 秒后采样 20 秒：网络 1 秒、CPU/内存 3 秒刷新开启系统状态时平均 CPU 0.4700%、RSS 67,989KB；关闭系统状态时平均 CPU 0.0600%、RSS 67,076KB。增量约 0.4100% CPU 与 913KB RSS，处在可接受范围。

若后续真实用户机器上看到明显 CPU 增高，优先进一步减少未变化文本的图像重绘，或把 CPU/内存/磁盘刷新间隔调高到 5 秒；只有存在明确用户需求时再增加 Settings 中的刷新频率选项。

### 菜单更新

macOS 系统状态进入菜单栏 status item 常驻图像，展开菜单不重复展示 `_system_stats` 与 `_network_stats` 两个 disabled item。Windows 不展示系统信息菜单行。

现有 `NativeMenuState::refresh_in_place` 会对同结构菜单调用原生 item `set_text`，不会频繁替换整个 native menu，避免 macOS/Windows 上展开菜单时被后台刷新关闭。

### macOS 菜单栏常驻标题

`tray-icon 0.19` 在 macOS 上会把 icon 按系统菜单栏高度缩放，因此系统状态开启且服务 Running 时，helper 生成一张透明模板图像作为 status item icon。模板图左侧绘制占满图像高度的 Bifrost template icon，确保开启系统状态后图标视觉大小不小于原版；右侧用 SF Mono/Menlo 等宽字体绘制固定宽度状态文本 `Cxx% | Mxx% | Dxx% | ↑nnnU/s | ↓nnnU/s`。关闭 `show_system_stats`、关闭全部子项或服务不在 Running 状态时恢复普通 Bifrost 图标。

macOS 展开菜单不再重复展示 `System:` / `Network:` 两排资源信息，避免同一信息同时出现在菜单栏和下拉菜单。Windows notification area 的 `set_title` 在当前 `tray-icon` 实现中是 no-op，系统托盘也没有 macOS 这种可横向常驻文本区域，因此 Windows 不支持 Tray 系统信息，而不是用菜单详情降级。

## 依赖项

- `bifrost-cli` 在 macOS tray 构建下使用 `sysinfo = "0.31"` 采样系统状态。
- 不新增 Tauri tray 依赖；仍复用现有 `tao` + `tray-icon` helper。

## Windows / macOS 兼容调研

- `tray-icon` 的 `MenuItem`/`CheckMenuItem` 支持 `set_text`，满足高频文本原地更新需求。
- `sysinfo 0.31.4` 支持 macOS 的 CPU、内存和网络接口统计；CPU 使用率依赖同一实例连续刷新，网络速率使用 `total_received()` / `total_transmitted()` 平台累计计数自行计算。
- macOS 侧 `sysinfo` 读取 `if_msghdr2.ifm_data.ifi_ibytes/ifi_obytes`，属于系统原生累计计数来源。
- Windows 虽然可以通过 `GetIfTable2` / `GetIfEntry2` 读取 `InOctets` / `OutOctets`，但产品层面不支持 Tray 系统信息，避免 notification area 菜单展示和后台采样带来的体验/性能折扣。
- 如果后续 macOS 发现内存或 CPU 数值与系统 Activity Monitor 口径偏差不可接受，fallback 方案是 macOS `host_statistics64` / `sysctl` 平台封装。

## 测试方案

### 单元测试

- `bifrost-storage`：`TrayConfig::default()` 默认 `enabled=true`、`show_system_stats=true`，序列化 round-trip 保留字段。
- `bifrost-storage`：部分 `[tray.system_stats_items]` TOML 仍让未声明子项默认开启。
- `bifrost-admin`：`GET /api/config/tray` 与 `GET /api/config` 返回 `system_stats_supported`、`show_system_stats` 和 `system_stats_items`；macOS `PUT /api/config/tray` 支持仅更新系统状态总开关或单个子项，非 macOS 携带系统状态字段时返回 400。
- `bifrost-cli`：
  - 系统状态格式化输出稳定单位。
  - macOS 菜单栏 status item icon 使用原尺寸 Bifrost 图标 + 等宽固定宽度 C/M/D/Up/Down 文案，且按子项开关过滤展示。
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

- macOS：真实启动 tray helper，验证默认开启、Settings Tray tab 开关、Admin API 持久化和性能基线；通过截图或辅助功能确认菜单栏常驻状态、原图标视觉大小、等宽数字和下行完整展示。
- Windows VM：验证 `GET /api/config/tray` 返回 `system_stats_supported=false`、系统状态全部 mask off；Settings Tray tab 只展示 `Tray Icon`；携带 `show_system_stats` 或 `system_stats_items` 的 `PUT` 返回 400；tray helper 日志中不应出现系统状态采样线程。

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
