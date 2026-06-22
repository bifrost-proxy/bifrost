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

Dashboard header 建议尺寸为 2x bitmap：`680 x 328 px`，对应 macOS points `340 x 164`。

布局：

```text
CPU       23%          load 1.8 / 2.1 / 2.4
                       logical cores 12

Memory    18.2 / 32G   pressure 42%
                       comp 1.2G  cache 4.8G  swap 512M / 2G

Disk      59%          free 410G of 460G
                       read 12.3M/s   write 4.8M/s

Network                up 1.2M/s
                       down 512K/s
```

V1 主要字段：

| 区域 | 字段 | 来源 |
| --- | --- | --- |
| CPU | percent、load averages、logical cores | `SystemStatsSampler` |
| Memory | used/total、pressure、compressed、cached、swap | `SystemStatsSampler` + macOS swap helper |
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

- 优先用 pressure percent。
- 无 pressure 时用 used/total percent。
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

Disk read/write 通过 `ioreg -rc IOBlockStorageDriver -k Statistics -l` 读取 `Bytes (Read)` 和 `Bytes (Write)`，后台间隔采样后计算每秒速率。失败时显示 `--`，不阻塞菜单打开。

## 采样策略

- CPU/memory/network：沿用 `SYSTEM_STATS_POLL_INTERVAL = 1s`。
- Disk usage：沿用现有较低频刷新。
- Disk I/O：后台采样，不能在菜单点击路径同步执行；菜单关闭时不触发 `ioreg` read/write 采样，避免 idle 状态额外拉起查询进程。
- 菜单打开时：直接使用最近 snapshot 渲染；采样为空时显示 `Collecting...`。
- Dashboard header 首次 stats 到达时安装一次；安装后只有菜单处于打开状态才刷新 bitmap image。菜单关闭期间的 1s stats 更新只更新数据 snapshot，不做 680x328 bitmap 渲染和 `NSImage` 替换。
- Dashboard 复用菜单栏统计项已有 fontdue font 与 glyph cache，不再维护第二套字体和 glyph cache。

## 渲染策略

新增模块：

- `crates/bifrost-cli/src/commands/tray/dashboard.rs`

职责：

- 构造 `TrayDashboardSnapshot`
- 渲染 `TrayDashboardBitmap { width, height, rgba }`
- 提供格式化和阈值颜色纯函数

渲染元素：

- 四个 metric row
- 轻量分隔线
- 左侧统一展示指标名称和主数值。
- 右侧用上下两行展示细节。
- CPU 右侧展示 load averages 和 logical cores。
- Memory 右侧展示 pressure、compressed、cached、swap。
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

本功能的 idle 性能目标是：不因为 dashboard header 引入每秒 bitmap 重绘、`NSImage` 替换或磁盘 I/O 子进程采样。2026-06-22 使用 debug build、临时 `BIFROST_DATA_DIR=/tmp/bifrost-tray-dashboard.A4Uf6G`、端口 `18890` 验证：

- 优化前：tray helper idle 约 `3.3% CPU / 109456 KB RSS`。
- 优化后：30 秒 idle 采样 `samples=30 avg_cpu=0.3067 min_cpu=0.0 max_cpu=1.5 avg_rss_kb=95015 min_rss_kb=92544 max_rss_kb=95664 rss_delta_kb=3120`。
- 日志确认 `native tray dashboard header installed items=14 width=340 height=164` 只安装一次；未看到 stats 更新触发 `tray menu refreshed` 循环。

剩余可优化项：group list 502 仍会按菜单数据轮询周期写 warning，这不是 dashboard bitmap 的成本，但后续可以增加失败退避，减少 idle 请求和日志噪声。

## 测试方案

单元测试：

- 颜色阈值：CPU/memory/disk 三档状态。
- pressure fallback：无 memory pressure 时使用 used/total。
- 格式化：bytes、bytes/sec、swap unavailable、disk read/write fallback。
- I/O Registry parser：累加非空块设备 read/write counter。
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
  - 内存展示 pressure/compressed/cached/swap
  - 磁盘展示 used/free/read/write
  - 菜单打开期间 stats 刷新不导致菜单自动关闭

## 风险与降级

- AppKit custom view API 只在 macOS 启用，其他平台不受影响。
- 若 `NSImageView` 或 bitmap image 创建失败，菜单退回原状。
- 若 disk I/O helper 不可用，显示 `read -- / write --`。
- 若 swap 信息不可用，显示 `swap --`。
- 若 dashboard 造成菜单尺寸过大，可通过 `BIFROST_TRAY_DASHBOARD=0` 临时关闭。

## 后续扩展

- 加 SMC 温度、风扇、电池。
- 加 Bifrost QPS/active connections/history。
- 升级为 `NSPopover`/`NSPanel`，支持 tab、hover、点击跳转。
