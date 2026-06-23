# macOS Tray Dashboard Menu Header

## 背景

Bifrost 的 macOS tray 当前已经能在菜单栏标题中展示 CPU、内存、磁盘和网络摘要，但点击菜单后仍主要是原生菜单项列表。用户希望点击菜单时，顶部展示一个更大、更清晰的系统状态区域：颜色区分、分隔线、内存 pressure/swap、磁盘 read/write、网络上下行等。

本方案采用短期可落地路线：在现有原生 `NSMenu` 顶部插入一个 macOS-only 的 custom `NSMenuItem` view。该 view 内展示一张由 Rust 渲染的 dashboard bitmap，下方仍保留现有菜单项。

## 目标

- 点击 Bifrost 菜单栏项时，菜单顶部出现固定高度 dashboard header。
- Header 只展示 CPU、内存、磁盘、网络四类系统状态；Bifrost 运行状态和版本号移动到菜单底部。
- 使用文字、颜色和分隔线增强可读性，不使用曲线图、柱状图或压力条。
- 菜单打开期间只更新 header view 的 image，不替换整个 `NSMenu`，避免菜单被刷新打断。
- 非 macOS 平台保持现状；macOS 渲染失败时自动降级为普通菜单。

## 非目标

- 不在 V1 引入 WebView、Tauri 或前端 bundle。
- 不在 V1 做可点击控件、tab、hover tooltip 等复杂交互。
- 不在 V1 做风扇控制、SMC 温度传感器、按 App 全局网络流量统计。
- 不在 V1 做复杂自定义看板；只把一个原生菜单 item 放大，展示当前实时指标。

## 现有基础

- `crates/bifrost-cli/src/commands/tray/system_stats.rs` 已采集：
  - CPU percent
  - memory used/total/pressure/compressed/cached
  - disk used percent
  - network upload/download rate
- `crates/bifrost-cli/src/commands/tray/tray.rs` 已有：
  - `NativeStatsStatusItem`
  - `NSStatusItem` / `NSMenu` / `NSMenuDelegate`
  - 菜单打开时不替换 `NSMenu` 的保护
  - bitmap 渲染菜单栏状态图标的基础函数

## 信息架构

Dashboard header 建议尺寸为 2x bitmap：`680 x 352 px`，对应 macOS points `340 x 176`。

布局：

```text
CPU       23%          cores 16 (P12 / E4)

Memory    42% (Healthy)  used 18.2 / 32G
                       comp 1.2G  cache 4.8G  swap 512M / 2G

Disk      59%          free 410G of 460G
                       read collecting   write collecting

Network                up 1.2M/s
                       down 512K/s
-------------------------------------------------
```

V1 主要字段：

| 区域 | 字段 | 来源 |
| --- | --- | --- |
| CPU | percent、logical cores、P/E cores（可用时） | `SystemStatsSampler` + macOS sysctl |
| Memory | used percent、used/total、compressed、cached、swap、pressure health | `SystemStatsSampler` + macOS swap helper |
| Disk | used percent、free bytes、read/write instant I/O | `sysinfo::Disks` + macOS I/O Registry statistics |
| Network | up/down rate | `SystemStatsSampler` |

## 颜色规则

状态色：

- Running: green
- Stopped: neutral gray
- Disconnected: red

CPU：

- `< 60%`: green
- `60%..85%`: amber
- `>= 85%`: red

Memory：

- 主数值使用 used/total percent，与顶部 status item 的 MEM 口径一致。
- pressure 只用于健康状态与颜色参考；无 pressure 时用 used/total percent 作为健康状态 fallback。
- `< 60%`: green
- `60%..80%`: amber
- `>= 80%`: red

Disk：

- `< 75%`: blue/green
- `75%..90%`: amber
- `>= 90%`: red

Network：

- Download: cyan/blue
- Upload: purple/magenta
- 不以吞吐大小触发告警色，只表达方向。

底色、主文本、副文本和分隔线使用固定透明色，优先保证 dark menu 和 light menu 中都可读。V1 bitmap 不依赖系统 appearance 查询，使用深浅兼容的半透明中性色。

## 数据模型

新增 macOS-only 数据结构：

```rust
struct TrayDashboardSnapshot {
    service_state: ServiceState,
    runtime_label: String,
    system_proxy_label: String,
    cpu_percent: f32,
    cpu_logical_cores: Option<usize>,
    cpu_performance_cores: Option<usize>,
    cpu_efficiency_cores: Option<usize>,
    load_one: f32,
    load_five: f32,
    load_fifteen: f32,
    memory_used_bytes: u64,
    memory_total_bytes: u64,
    memory_pressure_percent: Option<f32>,
    memory_compressed_bytes: u64,
    memory_cached_bytes: u64,
    swap_used_bytes: Option<u64>,
    swap_total_bytes: Option<u64>,
    disk_used_percent: Option<f32>,
    disk_free_bytes: Option<u64>,
    disk_total_bytes: Option<u64>,
    disk_read_bytes_per_sec: Option<u64>,
    disk_write_bytes_per_sec: Option<u64>,
    disk_total_bytes_per_sec: Option<u64>,
    network_up_bytes_per_sec: Option<u64>,
    network_down_bytes_per_sec: Option<u64>,
}
```

CPU P/E core 分布通过 `hw.perflevel0.logicalcpu` 与 `hw.perflevel1.logicalcpu` 读取；不可用时降级为 `logical cores N`。CPU 区域不展示 load average，避免把 Unix 队列指标暴露给普通 Tray 视觉。CPU 温度没有稳定免权限系统 API，V1 不在默认刷新路径里采集，避免引入 sudo、SMC 私有接口或高成本 `powermetrics`。

Disk read/write 生产路径直接通过 macOS IOKit 枚举 `IOBlockStorageDriver`，读取每个服务的 `Statistics` 属性中的累计 `Bytes (Read)` 和 `Bytes (Write)` counter，再用相邻 counter 的差值和真实 elapsed time 计算瞬时速率。`ioreg` 和 `iostat` 只作为人工验证参考，不进入 Tray 热路径。2026-06-22 在本机用连续 6 秒 `ioreg` 抽样和 `iostat -d -w 1 -c 5` 交叉验证：底层 counter 持续增长，`iostat` 同时能看到 `disk0` 等设备存在 MB/s 级吞吐，因此 UI 不能长期停留在空值。采样未完成时显示 `collecting`，采样完成且无 I/O 时显示 `0B/s`，不阻塞菜单打开。

## 采样策略

- CPU/memory/network：沿用 `SYSTEM_STATS_POLL_INTERVAL = 1s`。
- Disk usage：沿用现有较低频刷新。
- Disk I/O：不在后台持续高频采样，也不在 sampler 初始化时建立 baseline。只有原生菜单处于打开状态时才读取 IOKit counter；首次打开先建立 baseline，约 1 秒后的第二个样本产出 read/write 速率。菜单关闭后清空 Disk I/O baseline 和速率，避免旧值伪装成实时状态。
- 菜单打开时：Disk I/O 首个样本前显示 `collecting`；第二个样本后显示 read/write 瞬时速率。
- Dashboard header 首次 stats 到达时安装一次；安装后只有菜单处于打开状态才刷新 bitmap image。菜单关闭期间的 1s stats 更新只刷新菜单栏可见短标题；dashboard snapshot 保持不变，不做 680x352 bitmap 渲染、`NSImage` 替换或隐藏详细行 generation 更新。
- Dashboard 复用菜单栏统计项已有 fontdue font 与 glyph cache，不再维护第二套字体和 glyph cache。
- Remote group 接口失败后使用 60 秒退避，避免 group 服务暂时 502 时每个菜单数据轮询周期反复请求和刷 warning。

## 渲染策略

新增模块：

- `crates/bifrost-cli/src/commands/tray/dashboard.rs`

职责：

- 构造 `TrayDashboardSnapshot`
- 渲染 `TrayDashboardBitmap { width, height, rgba }`
- 提供格式化和阈值颜色纯函数

渲染元素：

- 四个 metric row
- 轻量分隔线；Network 区域底部也保留一条分隔线，用于和普通菜单项区分。
- 左侧统一展示指标名称和主数值。
- 右侧用上下两行展示细节。
- CPU 右侧只展示 logical/P-E cores；不展示 load average 或 Bifrost 自身进程负载。
- Memory 左侧主值展示与菜单栏 `MEM` 一致的 memory used 百分比，后面用小字号括号展示基于 pressure 的健康状态（如 `70% (Healthy)`、`70% (Pressure)`、`86% (Critical)`），无 pressure 时健康状态用 used percent 兜底；右侧第一行展示 used/total，第二行展示 compressed、cached、swap。
- Disk 右侧展示 free/total、read/write。
- Network 左侧只展示 `Network`，右侧第一行 upload、第二行 download。

文字仍使用现有 `fontdue` 字体缓存思路。Dashboard 需要支持 colored text，因此新增颜色版 glyph 绘制函数，而不是复用只能画 alpha 的菜单栏绘制函数。

## AppKit 集成

macOS 下扩展 `NativeMenuState`：

1. 普通菜单项 append 前，尝试创建 dashboard header。
2. 创建 `NSMenuItem`，调用 `setView(Some(&NSImageView as NSView))`。
3. 把 `NSImageView` 和最后一次 bitmap image 保存在 `NativeMenuState` 中。
4. 后台 stats 更新时，如果菜单已打开，只调用 `dashboard_header.set_snapshot(...)` 更新 image，不替换整个 menu。
5. 如果 header 创建失败，记录 debug 日志并继续普通菜单。

关键约束：

- `NSMenu` 结构不能在菜单打开时由后台刷新替换。
- 普通菜单轮询刷新必须保留 `dashboard` snapshot，避免 header 被先置空再重装导致闪烁。
- Header view 必须不可点击，避免吞掉下方菜单行为。
- Header 不进入 `action_map`，不参与普通菜单 action target 安装。

## 性能验证

本功能的 idle 性能目标是：不因为 dashboard header 引入每秒 bitmap 重绘或 `NSImage` 替换；Disk read/write 作为实时指标只在菜单打开期间做 1 秒级 IOKit counter 差分，菜单关闭后不持续高频采样。2026-06-22 使用 debug build、临时端口 `18890` 验证：

- 优化前：tray helper idle 约 `3.3% CPU / 109456 KB RSS`。
- IOKit + 菜单打开期间采样后：30 秒 idle 采样 `samples=30 avg_cpu=1.0033 min_cpu=0.0 max_cpu=4.9 avg_rss_kb=103095 min_rss_kb=103008 max_rss_kb=103296 rss_delta_kb=288`。
- 关闭菜单保留 dashboard snapshot + remote group 60 秒失败退避后：30 秒 idle 采样 `samples=30 avg_cpu=0.5733 min_cpu=0.0 max_cpu=7.3 avg_rss_kb=87370 min_rss_kb=85744 max_rss_kb=89392 rss_delta_kb=3648`。
- 当前线上 release helper 对照：`target/release/bifrost __tray` RSS 约 `64208 KB`；debug build 有额外符号和调试开销，不直接作为发布包峰值。
- 日志确认 `native tray dashboard header installed ...` 只安装一次；新增底部分隔线后 header 高度为 `176` points。group list 502 warning 从约每 6 秒一次降为约每 60 秒一次。

## 测试方案

单元测试：

- 颜色阈值：CPU/memory/disk 三档状态。
- pressure fallback：无 memory pressure 时使用 used/total。
- 格式化：CPU P/E core fallback、bytes、bytes/sec、swap unavailable、disk read/write collecting fallback。
- Disk I/O counter delta：相邻累计 counter 按真实 elapsed time 计算 read/write bytes/sec，counter reset 时不产出负速率。
- Disk I/O 采样生命周期：菜单关闭时不采集 read/write，菜单打开后按 1 秒 IOKit counter delta 产出速率。
- bitmap：样例 snapshot 渲染后尺寸固定、非空 alpha、包含不同颜色像素。

E2E：

- 本功能是 macOS 原生 tray UI，不适合用规则 E2E 夹具直接覆盖。
- 可执行构建级验证：`cargo test -p bifrost-cli ...`、`cargo build --bin bifrost`。
- 若需要真实 UI 验证，走 human_tests。

human_tests：

- 新增或更新 `human_tests/cli-tray-dashboard.md`。
- 按用例启动真实 Bifrost：
  - 使用临时 `BIFROST_DATA_DIR`
  - 设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`
  - 本次明确测试 tray，所以不设置 `BIFROST_DISABLE_TRAY=1`
  - 启动主服务必须加 `--no-system-proxy`
- 验证：
  - 菜单能打开
  - 顶部 header 出现 CPU/Memory/Disk/Network 区域
  - Bifrost 运行状态和版本号出现在菜单底部
  - CPU 不展示 Bifrost 自身进程负载或 load，只展示逻辑/P-E 核心信息
  - 内存左侧主值展示 used percent 并与顶部 MEM 一致，pressure 仅作为健康状态；右侧展示 used/total、compressed/cached/swap
  - 磁盘展示 used/free/read/write，未完成采样时显示 collecting
  - Network 下方有分隔线，把 header 和普通菜单项隔开
  - 菜单打开期间 stats 刷新不导致菜单自动关闭

## 风险与降级

- AppKit custom view API 只在 macOS 启用，其他平台不受影响。
- 若 `NSImageView` 或 bitmap image 创建失败，菜单退回原状。
- 若 disk I/O IOKit counter 尚未形成两个样本，显示 `read collecting / write collecting`；若 IOKit 不可用，会保持 collecting 而不阻塞菜单。
- 若 swap 信息不可用，显示 `swap --`。
- 若 dashboard 造成菜单尺寸过大，可通过 `BIFROST_TRAY_DASHBOARD=0` 临时关闭。

## 后续扩展

- 加 SMC 温度、GPU cores、风扇、电池；这些需要额外权限、私有接口或较高采样成本，后续单独评估。
- 加 Bifrost QPS/active connections/history。
- 升级为 `NSPopover`/`NSPanel`，支持 tab、hover、点击跳转。
