# Tray 系统状态展示

## 功能模块详细描述

Tray 新增全系统状态展示能力，默认开启。macOS 上优先在菜单栏 status item 中以模板图像常驻展示紧凑状态，避免用户每次点开菜单才能看到：

- Bifrost 原图标保持与未启用系统状态时一致的视觉大小。
- 图标右侧使用等宽字体展示 `Cxx% | Mxx% | Dxx% | ↑nnnU/s | ↓nnnU/s`，其中 `C/M/D` 分别表示 CPU、Memory、Disk。
- 百分比不足两位时补 `0`，网速数字始终三位；等宽数字避免 `1/8/0` 字宽不同造成菜单栏左右抖动。

Windows 原生托盘菜单中仍保留两排只读详情，作为点击菜单后的可读补充：

- `System: CPU <percent> | Memory <used> / <total> | Disk <percent>`
- `Network: Up <rate> | Down <rate>`

这些指标表示整台机器的状态，不表示 Bifrost 进程自身资源消耗，也不表示 Bifrost 代理流量聚合。Settings 新增独立 `Tray` tab，用于管理原生 Tray helper 与系统状态展示开关。

## 实现逻辑

### 配置模型

在 `bifrost-storage::TrayConfig` 中增加：

- `enabled: bool`：是否启用 tray helper，沿用既有语义。
- `show_system_stats: bool`：是否在 tray 菜单展示系统状态，默认 `true`。
- `system_stats_items`：系统状态子项开关，包含 `cpu`、`memory`、`disk`、`upload`、`download`，每项默认 `true`；缺省字段也按 `true` 处理，确保老配置或部分手写配置不会误关其它指标。

Admin API `/api/config/tray` 返回并更新 `show_system_stats` 与 `system_stats_items`；`PUT` 支持只更新 `enabled`、只更新 `show_system_stats`，或只更新某一个 `system_stats_items` 子字段，避免 Settings 中多个开关互相覆盖。

### Tray helper 本地采样

系统状态由 `bifrost __tray` helper 本地采样，不经过 Admin API：

- Tray helper 在独立进程运行，即使 Bifrost 主服务繁忙、停止或 Admin API 无响应，也可以继续更新机器状态。
- 采样线程每 3 秒刷新菜单栏系统状态；CPU、内存和磁盘每 3 秒刷新一次并复用最近值。
- 使用同一个 `sysinfo::System`、`sysinfo::Networks` 与 `sysinfo::Disks` 实例循环刷新，避免 CPU 与网络差值类指标在重建实例时失真。
- 磁盘百分比按当前 `data_dir` 所在挂载点计算，避免随便取第一个磁盘导致显示与用户实际数据目录无关。
- 网络速率优先使用系统默认出站接口。macOS 通过 `route -n get default` 获取默认路由 interface，再按该接口的内核累计字节计数计算吞吐；获取不到默认接口时，回退到最活跃的非虚拟物理接口。
- 每个接口保存平台累计字节计数，下一次采样用同一接口的累计差值除以 `Instant` 单调时钟间隔，转换为 bytes/sec 后格式化。
- 首次采样、网卡新增/移除、计数回退或采样间隔小于 900ms 时，该接口当前拍不参与速率计算，避免显示虚高尖峰。
- 网络接口过滤 loopback、AWDL、tunnel、bridge、Parallels/VMware/VirtualBox、Docker、Tailscale、ZeroTier 等虚拟/隧道接口，降低 macOS/VM/VPN 场景双算或虚高风险。
- 接口选择带 hysteresis：只有候选接口吞吐超过当前接口 2 倍才切换，避免 Wi-Fi/有线/虚拟接口在相近低流量下频繁抖动；展示值使用当前样本 60%、上一展示值 40% 的指数平滑，降低单样本尖峰。

### 性能与实时性权衡

刷新频率不做成用户可见配置，默认系统状态线程 3 秒、CPU/内存/磁盘 3 秒、网卡列表 30 秒。理由：

- 3 秒刷新能明显降低 tray helper 空闲 CPU，同时仍足够观察菜单栏趋势；CPU/内存/磁盘变化相对慢，3 秒刷新足够判断当前系统是否繁忙。
- 采样只在 tray helper 本地执行，不访问 Admin API，不阻塞主代理进程。
- 当 `show_system_stats=false` 或所有 `system_stats_items` 子项均关闭时，线程只读取配置并清空菜单状态，不执行 `sysinfo` CPU/内存/网络采样；当 Upload/Download 均关闭但 CPU/Memory/Disk 仍开启时，只刷新非网络指标并重置网络累计基线，避免把关闭期间的字节变化折算成当前速率。
- Mac 本地 release microbench 使用 `sysinfo 0.31.4` 连续执行 500 次采样，单次平均 1.1796ms、最大 1.8934ms；按采样耗时估算，1s/2s/3s 刷新分别约为 0.1180% / 0.0590% / 0.0393% 单核 CPU。
- Mac debug tray helper 真实进程 warm-up 5 秒后采样 20 秒：网络 1 秒、CPU/内存 3 秒刷新开启系统状态时平均 CPU 0.4700%、RSS 67,989KB；关闭系统状态时平均 CPU 0.0600%、RSS 67,076KB。增量约 0.4100% CPU 与 913KB RSS，处在可接受范围。

若后续真实用户机器上看到明显 CPU 增高，优先进一步减少未变化文本的图像重绘，或把 CPU/内存/磁盘刷新间隔调高到 5 秒；只有存在明确用户需求时再增加 Settings 中的刷新频率选项。

### 菜单更新

系统状态开启时菜单结构固定包含 `_system_stats` 与 `_network_stats` 两个 disabled item；初始显示 collecting 文案，后续只更新 item 文本。

现有 `NativeMenuState::refresh_in_place` 会对同结构菜单调用原生 item `set_text`，不会每 1 秒替换整个 native menu，避免 macOS/Windows 上展开菜单时被后台刷新关闭。

### macOS 菜单栏常驻标题

`tray-icon 0.19` 在 macOS 上会把 icon 按系统菜单栏高度缩放，因此系统状态开启且服务 Running 时，helper 生成一张透明模板图像作为 status item icon。模板图左侧绘制占满图像高度的 Bifrost template icon，确保开启系统状态后图标视觉大小不小于原版；右侧用 SF Mono/Menlo 等宽字体绘制固定宽度状态文本 `Cxx% | Mxx% | Dxx% | ↑nnnU/s | ↓nnnU/s`。关闭 `show_system_stats`、关闭全部子项或服务不在 Running 状态时恢复普通 Bifrost 图标。

macOS 展开菜单不再重复展示 `System:` / `Network:` 两排资源信息，避免同一信息同时出现在菜单栏和下拉菜单。Windows notification area 的 `set_title` 在当前 `tray-icon` 实现中是 no-op，系统托盘也没有 macOS 这种可横向常驻文本区域。因此 Windows 继续通过图标、菜单两排详情和 hover/菜单交互降级展示，不能承诺实现红框式常驻文字。

## 依赖项

- `bifrost-cli` 在非 Linux tray 构建下新增 `sysinfo = "0.31"`，与 `bifrost-admin` 已使用版本一致。
- 不新增 Tauri tray 依赖；仍复用现有 `tao` + `tray-icon` helper。

## Windows / macOS 兼容调研

- `tray-icon` 的 `MenuItem`/`CheckMenuItem` 支持 `set_text`，满足高频文本原地更新需求。
- `sysinfo 0.31.4` 支持 Windows 与 macOS 的 CPU、内存和网络接口统计；CPU 使用率依赖同一实例连续刷新，网络速率使用 `total_received()` / `total_transmitted()` 平台累计计数自行计算。
- macOS 侧 `sysinfo` 读取 `if_msghdr2.ifm_data.ifi_ibytes/ifi_obytes`，Windows 侧读取 `GetIfTable2` / `GetIfEntry2` 的 `InOctets` / `OutOctets`，属于系统原生累计计数来源。
- 如果后续 Windows VM 发现 `sysinfo::Networks` 对某些虚拟网卡漏报，fallback 方案是直接封装 Windows `GetIfTable2Ex` 汇总 `MIB_IF_ROW2` 字节计数。
- 如果后续 macOS 发现内存或 CPU 数值与系统 Activity Monitor 口径偏差不可接受，fallback 方案是 macOS `host_statistics64` / `sysctl` 平台封装。

## 测试方案

### 单元测试

- `bifrost-storage`：`TrayConfig::default()` 默认 `enabled=true`、`show_system_stats=true`，序列化 round-trip 保留字段。
- `bifrost-storage`：部分 `[tray.system_stats_items]` TOML 仍让未声明子项默认开启。
- `bifrost-admin`：`GET /api/config/tray` 与 `GET /api/config` 返回 `show_system_stats` 和 `system_stats_items`；`PUT /api/config/tray` 支持仅更新系统状态总开关或单个子项。
- `bifrost-cli`：
  - 系统状态格式化输出稳定单位。
  - macOS 菜单栏 status item icon 使用原尺寸 Bifrost 图标 + 等宽固定宽度 C/M/D/Up/Down 文案，且按子项开关过滤展示。
  - Tray 菜单开启系统状态时展示两排 disabled item。
  - 系统状态关闭时隐藏两排 item。
  - 同结构状态文本更新不改变 native menu shape。
- `web`：Settings Tray tab 展示两个开关，并通过 API 保存/回读。

### E2E 测试

- 新增 shell E2E 覆盖 `/api/config/tray` 默认值、关闭/开启 `show_system_stats`、单独关闭/开启 `system_stats_items.download`、关闭/开启 `enabled` 后 config 持久化。
- 新增/更新 Playwright 覆盖 `Settings -> Tray` tab、总开关和 CPU/Memory/Disk/Upload/Download 子开关可见与保存，并在桌面与移动端 viewport 截图确认图标、文案、开关没有遮挡、错位或横向溢出。
- Tray helper smoke 继续使用 `test_cli_tray_startup_ci.sh`；新增系统状态日志或菜单结构断言时必须避免要求无头 Windows runner 长驻原生 tray。

### 真实场景测试

更新 `human_tests/cli-tray-helper.md`：

- macOS：真实启动 tray helper，验证默认开启、Settings Tray tab 开关、Admin API 持久化和性能基线；通过原生菜单辅助功能读取确认两排状态与 1 秒级网络刷新；在获得 Screen Recording 权限后补齐连续 PNG 截图验收。
- Windows VM：在隔离 worktree 中构建并真实启动 tray helper，验证默认开启、Admin API 持久化和 tray helper 日志；在防火墙授权弹窗处理后补齐 notification area 原生菜单连续 PNG 截图验收。

## Review/Fix/Test 闭环方案

第 1 轮：

- 复核用户目标：默认开启、两排展示、全系统指标、Settings 独立 Tray tab、macOS/Windows 兼容。
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
