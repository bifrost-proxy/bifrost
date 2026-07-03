# macOS Tray Dashboard Menu Header

## 背景

Bifrost 的 macOS tray 已经能在菜单栏标题里显示 CPU / 内存 / 磁盘 / 网络的摘要，但点击图标后仍是普通的 `NSMenu` 项列表。用户希望点击 tray 后，菜单顶部出现一个更大的、颜色/分隔线区分的系统状态区域：CPU 百分比 + P/E cores、Memory 百分比 + pressure + swap、Disk 使用率 + read/write、Network 上下行。

本方案走短期可落地路线：在原生 `NSMenu` 顶部插入一个 macOS-only 的 custom `NSMenuItem` view，view 中展示一张 Rust 侧渲染的 dashboard bitmap，下方保留现有菜单项。非 macOS 平台走原状；macOS 渲染失败自动降级为普通菜单。

## 用户目标验证清单

### 必须实现

- 点击 Bifrost 菜单栏图标时，菜单顶部出现固定高度 dashboard header。
- Header 只展示 CPU / Memory / Disk / Network 四类系统状态；Bifrost 运行状态与版本号移动到菜单底部。
- 使用颜色 (`status_color`) 与分隔线增强可读性；不使用曲线/柱状/压力条。
- 菜单打开期间只更新 header view 的 image，不替换整个 `NSMenu`，避免菜单被刷新自动关闭。
- 非 macOS 平台完全不受影响。
- macOS 渲染失败或 `BIFROST_TRAY_DASHBOARD=0` 时自动回退到普通菜单。

### 必须不破坏

- 菜单栏短标题渲染（`render_menu_bar_stats_bitmap`）继续按 1 秒轮询更新。
- Group list API 502 不影响 header 渲染；后台采用 60 秒退避，避免频繁 warn。
- `NativeStatsStatusItem` / `NSStatusItem` / `NSMenu` / `NSMenuDelegate` 结构不变。
- Bifrost 运行状态、版本号、系统代理开关等菜单动作项保持在菜单下半段。
- `action_map` 不包含 header view，dashboard 不吞下方菜单项 action。

### 必须真实验证

- macOS 上真实点开菜单：header 出现四行系统状态；菜单打开期间 stats 刷新不会关闭菜单。
- Disk I/O 首次采样期间显示 `collecting`；第二个样本后显示 read/write 瞬时速率。
- Memory 百分比与顶部 `MEM` 一致；pressure 只影响右侧健康标签的颜色与文字。
- 关闭菜单后不再进入 dashboard bitmap 渲染路径；rss/cpu 曲线保持稳定。
- 设置 `BIFROST_TRAY_DASHBOARD=0` 后菜单退回原状。

## 非目标

- V1 不引入 WebView / Tauri / 前端 bundle。
- V1 不做可点击控件、tab、hover tooltip 等复杂交互。
- V1 不做风扇控制、SMC 温度、按 App 统计流量。
- V1 不做复杂自定义看板，只把一个原生菜单 item 放大展示实时指标。

## 产品语义

### Dashboard 是 tray 的顶部视图，不是独立窗口

Header 是 `NSMenu` 内的第一个 custom `NSMenuItem`，通过 `setView(NSImageView)` 显示 Rust 渲染的 bitmap。它随菜单一起打开/关闭，不是常驻窗口，也不是 `NSPopover`（后者作为后续扩展）。

Bifrost 运行状态和版本号从头部下移到菜单尾部，dashboard 顶部只留系统状态；用户扫一眼就能判断"机器现在忙不忙"。

### 颜色规则

状态色：

- Running: green
- Stopped: neutral gray
- Disconnected: red

CPU：

- `< 60%`: green
- `60%..85%`: amber
- `>= 85%`: red

Memory（主数值 = used/total percent，与菜单栏 `MEM` 一致；pressure 用于健康状态文字与颜色 fallback）：

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
- 不因吞吐大小告警。

底色 / 主文本 / 副文本 / 分隔线使用固定透明色，兼容 dark 和 light menu，V1 bitmap 不查询系统 appearance。

### 采样与刷新

- CPU / Memory / Network：沿用 `SYSTEM_STATS_POLL_INTERVAL = 1s`。
- Disk usage：沿用现有低频刷新。
- Disk I/O：只在原生菜单处于打开状态时读取 IOKit counter；首次打开建立 baseline，~1 秒后第二个样本产出速率。菜单关闭后清空 baseline 与速率。
- Dashboard header 首次 stats 到达时安装一次；菜单关闭期间的 1s stats 更新只刷新菜单栏短标题，不做 680×352 bitmap 渲染或 `NSImage` 替换。
- Dashboard 复用菜单栏统计项已有的 fontdue font 与 glyph cache，不维护第二套。
- Remote group 接口失败使用 60 秒退避。

## 技术细节

### 布局与尺寸

Dashboard header 建议 2× bitmap：`680 × 352 px`，对应 macOS points `340 × 176`。

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

V1 字段：

| 区域 | 字段 | 来源 |
| --- | --- | --- |
| CPU | percent、logical cores、P/E cores | `SystemStatsSampler` + macOS `hw.perflevel0/1.logicalcpu` |
| Memory | used%、used/total、compressed、cached、swap、pressure | `SystemStatsSampler` + macOS swap helper |
| Disk | used%、free、read/write instant I/O | `sysinfo::Disks` + macOS IOKit `IOBlockStorageDriver.Statistics` |
| Network | up/down rate | `SystemStatsSampler` |

CPU P/E cores 通过 sysctl；不可用时降级为 `logical cores N`。CPU 区域不展示 load average；也不采集温度（无稳定免权限系统 API，V1 不进热路径）。

### 数据模型 (crates/bifrost-cli/src/commands/tray/dashboard.rs)

```rust
pub struct TrayDashboardSnapshot {
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

pub struct TrayDashboardBitmap { pub width: u32, pub height: u32, pub rgba: Vec<u8> }

pub fn render_dashboard_with_theme(
    snapshot: &TrayDashboardSnapshot,
    font: &fontdue::Font,
) -> Option<TrayDashboardBitmap>;

pub fn dashboard_enabled_from_env(value: Option<&str>) -> bool;
```

绘制入口按 CPU → Memory → Disk → Network 顺序 draw metric row 与分隔线（约 dashboard.rs:126-133）。

### Disk I/O 采样

生产路径直接通过 macOS IOKit 枚举 `IOBlockStorageDriver`，读取 `Statistics` 属性的 `Bytes (Read)` / `Bytes (Write)` counter，按相邻样本差值与 elapsed time 计算瞬时速率。`ioreg` / `iostat` 仅作为人工验证参考。

- 采样未完成时显示 `collecting`；完成且无 I/O 时显示 `0B/s`。
- 菜单关闭后清空 baseline 与速率，避免旧值伪装成实时。
- 2026-06-22 用连续 6 秒 `ioreg` 抽样 + `iostat -d -w 1 -c 5` 交叉验证，counter 持续增长，`disk0` 有 MB/s 级吞吐，UI 不能长期停留在空值。

### AppKit 集成 (crates/bifrost-cli/src/commands/tray/tray.rs)

```rust
struct NativeMenuState {
    menu: NSMenu,
    dashboard_header: Option<NativeDashboardHeader>,
    ...
}

impl NativeMenuState {
    fn new(items, action_map, dashboard: Option<&TrayDashboardSnapshot>) -> Self { ... }
    fn refresh_dashboard(&mut self, dashboard: Option<&TrayDashboardSnapshot>) { ... }
    fn install_action_targets(&mut self, items) { ... }
}

fn install_dashboard_header(menu: &NSMenu, dashboard: Option<&TrayDashboardSnapshot>)
    -> Option<NativeDashboardHeader>;

fn native_dashboard_image_from_bitmap(bitmap: &TrayDashboardBitmap)
    -> Option<NativeStatsImage>;

fn copy_dashboard_bitmap_to_image_rep(bitmap: &TrayDashboardBitmap, image_rep: &NSBitmapImageRep)
    -> bool;

fn should_refresh_dashboard(prev_snapshot, next_snapshot, dashboard_installed, ...)
    -> bool;
```

流程：

1. 普通菜单项 append 前先 `install_dashboard_header`；创建 `NSMenuItem` + `setView(NSImageView)`。
2. 保存 `NSImageView` 与最后一次 bitmap 到 `NativeMenuState.dashboard_header`。
3. 后台 stats 更新时若菜单打开，`refresh_dashboard(...)` 只替换 image；否则跳过 bitmap 渲染。
4. Header 创建失败记录 debug 日志并继续普通菜单。

关键约束：

- `NSMenu` 结构不能在菜单打开时由后台线程替换。
- 普通菜单轮询刷新必须保留 `dashboard` snapshot，避免 header 被先置空再重装导致闪烁。
- Header view 不可点击；`NSImageView` isEnabled=false，`action_map` 不含 header。
- Dashboard 关闭 (`TRAY_DASHBOARD_ENV = "BIFROST_TRAY_DASHBOARD"` = `0`) 时不安装 header，也不清理老 header（若已装则保留最后一帧）。

## Sync 边界

- Dashboard 完全在本机采样，不产生任何跨设备 sync。
- 远端 `bifrost remote` 不代理 tray；远端机器如需 dashboard 需自己在目标机运行 `bifrost` GUI。

## Phase 1-4

### Phase 1: 数据模型与渲染

- `TrayDashboardSnapshot` / `TrayDashboardBitmap`。
- `render_dashboard_with_theme` + `dashboard_width` + `draw_metric_row` 系列纯函数。
- 单元测试覆盖颜色阈值、格式化、bitmap 非空。

### Phase 2: Disk I/O 采样

- macOS IOKit `IOBlockStorageDriver.Statistics` 枚举。
- 菜单打开建立 baseline，第二样本产出速率。
- 菜单关闭清空 baseline 与速率。

### Phase 3: AppKit 集成

- `install_dashboard_header` 首次安装。
- `refresh_dashboard` 只替换 image。
- `should_refresh_dashboard` 判断是否需要重绘。
- Header 失败降级；`BIFROST_TRAY_DASHBOARD=0` 逃生。

### Phase 4: 性能验证 & 文档

- 2026-06-22 性能样本记录在设计文档，并写入 human_tests。
- README / docs 更新。

## 性能验证

Idle 目标：不因 dashboard 引入每秒 bitmap 重绘或 `NSImage` 替换；Disk I/O 只在菜单打开期间做 1s IOKit counter 差分。

2026-06-22 debug build、临时端口 `18890`：

- 优化前：tray helper idle 约 `3.3% CPU / 109456 KB RSS`。
- IOKit + 菜单打开采样：`samples=30 avg_cpu=1.00 min=0.0 max=4.9 avg_rss_kb=103095 rss_delta_kb=288`。
- 关闭菜单保留 snapshot + remote group 60s 退避：`samples=30 avg_cpu=0.57 max=7.3 avg_rss_kb=87370 rss_delta_kb=3648`。
- Release `target/release/bifrost __tray` RSS 约 `64208 KB`；debug build 有额外符号，不做发布峰值参考。
- 日志确认 `native tray dashboard header installed ...` 只安装一次；新增底部分隔线后 header 高度 `176` points。Group list 502 warning 从每 6s 一次降为每 60s 一次。

## 测试方案

### 单元测试 (crates/bifrost-cli/src/commands/tray)

- 颜色阈值：CPU / Memory / Disk 三档 status_color。
- Pressure fallback：无 memory pressure 时使用 used/total percent。
- 格式化：CPU P/E cores fallback、bytes、bytes/sec、swap unavailable、disk read/write collecting fallback。
- Disk I/O counter delta：正常增长按 elapsed time 换算 bytes/sec；counter reset 时不产出负速率。
- Disk I/O 采样生命周期：菜单关闭不采集；菜单打开首个样本 baseline，第二个样本出速率。
- Bitmap：`render_dashboard_with_theme(sample_snapshot, font)` 返回固定尺寸、非空 alpha、含不同颜色像素。
- `should_refresh_dashboard` 逻辑：仅当 dashboard 未安装或 stats 变化时返回 true。
- `test_menu_bar_stats_bitmap_is_compact_and_non_empty`：菜单栏短标题保持 <=1400px 宽、36px 高。
- `sample_dashboard_snapshot` fixture 覆盖 `dashboard: None` 时 `menu_bar_stats_title` 仍工作。

### 集成 / 构建

- `cargo test -p bifrost-cli tray::`
- `cargo build --bin bifrost`
- 因是 macOS 原生 tray UI，不用规则 E2E 夹具直接覆盖。

### human_tests

- `human_tests/cli-tray-dashboard.md`
  - TC-TRAY-DASH-01 首次打开菜单 header 出现四行。
  - TC-TRAY-DASH-02 CPU 显示 P/E cores，不显示 load。
  - TC-TRAY-DASH-03 Memory 主值与顶部 MEM 一致；pressure 只影响健康文字。
  - TC-TRAY-DASH-04 Disk 首次显示 collecting；第二样本显示 read/write。
  - TC-TRAY-DASH-05 Network 下方分隔线。
  - TC-TRAY-DASH-06 菜单打开期间 stats 刷新不关闭菜单。
  - TC-TRAY-DASH-07 `BIFROST_TRAY_DASHBOARD=0` 关闭 dashboard。
  - TC-TRAY-DASH-08 Bifrost 运行状态与版本号出现在菜单底部。
- `human_tests/cli-tray-helper.md` 保留原 tray helper 用例，不重复。

启动 bifrost 使用临时 `BIFROST_DATA_DIR`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`；本次测 tray 所以不设 `BIFROST_DISABLE_TRAY=1`；主服务加 `--no-system-proxy`。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核 dashboard 是否在菜单打开期间做过多 bitmap 渲染。
- 复核 Disk I/O baseline 生命周期：菜单关闭后必须清空。
- 复测单元测试 + `cli-tray-dashboard.md` 前 4 个用例。

### 第 2 轮

- 检查 header 安装失败路径日志级别（debug 不 warn）。
- 检查 remote group 502 退避是否 60s；避免菜单打开期间狂 warn。
- 复测 TC-TRAY-DASH-05..08，特别是 dashboard 关闭逃生。

## 风险与降级

- **AppKit custom view** 仅在 macOS 编译；其它平台自动忽略。
- **NSImageView / bitmap image 失败**：菜单退回原状；不阻塞菜单打开。
- **Disk I/O IOKit 不可用**：显示 `collecting`，永不阻塞。
- **Swap 信息不可用**：显示 `swap --`。
- **菜单过大**：`BIFROST_TRAY_DASHBOARD=0` 立即关闭。
- **温度/风扇/GPU**：V1 不做；后续如需，走 SMC / powermetrics / private API，需额外权限与采样成本评估。
- **Popover 演进**：升级到 `NSPopover` / `NSPanel` 可支持 tab / hover / 点击跳转，属于 V2 范畴，本方案不阻塞该演进（数据模型与渲染入口已解耦）。
