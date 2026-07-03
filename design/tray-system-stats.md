# Tray 系统状态展示

## 背景

Bifrost 桌面用户长期只能通过 Activity Monitor / iStat Menus 等第三方工具观察机器负载,而 tray 只是一个静态图标 + 服务开关。要在 code review、代理调试、抓包场景下判断“机器是不是也在忙”,用户必须频繁切窗口。

Tray 系统状态展示把整机的 CPU、内存、磁盘、上/下行网速直接绘制到 macOS 菜单栏 status item 的模板图像里,并在下拉 dashboard 中给出更完整的诊断字段。macOS 默认开启;Windows/Linux 因 tray API 无法承载横向文本而明确不支持,只保留 Tray Icon 开关。

代码入口:

- `crates/bifrost-cli/src/commands/tray/system_stats.rs`(采样、格式化、bitmap 渲染)
- `crates/bifrost-cli/src/commands/tray/menu.rs`、`.../tray/tray.rs`(菜单构建与状态发布)
- `crates/bifrost-cli/src/commands/tray/dashboard.rs`(下拉 dashboard)
- `crates/bifrost-storage/src/unified_config.rs::TrayConfig`(配置模型)
- `web/src/pages/Settings/tabs/TrayTab.tsx` + `web/src/api/config.ts`(Settings UI 与前端 API)
- `crates/bifrost-cli/src/commands/tray/tray_tests.rs`(菜单栏 bitmap / 系统状态回归测试)

## 用户目标验证清单

### 必须实现

- macOS tray helper 默认开启系统状态展示,菜单栏 status item 常驻单行 `C/M/D/↑/↓` 文本。
- Bifrost 原图标视觉大小不因系统状态开启而缩水;右侧文本使用 28px 常规系统字体,列宽稳定。
- 采样目标是整机而不是 Bifrost 进程自身;上/下行网速作为一个整体字段展示,不用竖线拆开。
- Settings 提供独立 `Tray` tab: macOS 显示 Tray Icon + 系统状态总开关 + CPU/Memory/Disk/Upload/Download 5 个子开关;Windows 只显示 Tray Icon。
- 系统状态子项开关可以单独关闭某一个指标,而不影响其它指标继续采样和展示。
- 采样在 `bifrost __tray` helper 进程中执行,不经过 Admin API,主代理挂起时 tray 仍能刷新。

### 必须不破坏

- Tray helper 启停、服务状态展示、Login Item 逻辑不受系统状态开关影响。
- 已有 `TrayConfig` TOML 反序列化保持兼容:老配置缺失 `show_system_stats` 或部分 `system_stats_items` 时默认按 `true` 处理。
- Windows / Linux tray helper 不启动系统状态采样线程,不引入无展示价值的 CPU/IO 负担。
- 下拉 dashboard 与菜单栏顶部使用同一份 `SystemStatsSnapshot`,展开菜单不产生第二套口径。
- `bifrost start` / `bifrost stop` / `bifrost upgrade` 等 CLI 命令行为不变。

### 必须真实验证

- macOS 真实 tray helper 展开 dashboard,菜单栏顶部 `C/M/D/↑/↓` 与 dashboard 左侧 `MEM` 主值口径一致。
- macOS 关闭 `system_stats_items.download` 后,菜单栏右侧只剩 `↑` 一段,dashboard `Download` 隐藏或置零,采样线程停止读网络计数。
- Windows fixture 下 `GET /api/config/tray` 返回 `system_stats_supported=false`,Settings Tray tab 只显示 `Tray Icon`。
- 真实 release 采样(默认开启 + 全部子项开)tray helper 平均 CPU < 1%,最大 CPU < 1.5%。

## 产品语义

### 系统级而非进程级

菜单栏顶部与下拉 dashboard 主值都表示整机负载:

- CPU:所有物理核心 tick delta 累计,来源 `host_statistics(HOST_CPU_LOAD_INFO)`。
- Memory:Activity Monitor 风格 `internal - purgeable + wired + compressor`,排除 file-backed / speculative / purgeable cache。`kern.memorystatus_level` 仅用于健康状态/颜色,不作为主百分比。
- Disk:`statvfs(data_dir 所在挂载点)`;下拉展示 read/write IOKit Statistics(仅菜单展开时读取)。
- Network:默认走接口累计字节计数 + loopback 下行兜底;`BIFROST_TRAY_NETTOP_STATS=1` 时切换到 `nettop` 单个长驻采样器,仅作为诊断口径。

### 单一数据源 + 动态采样窗口

系统状态线程恒定 1 秒。菜单未展开:网速保持 1 秒、CPU/内存按 3 秒后台窗口刷新、磁盘容量 30 秒、默认路由/网卡列表 60 秒;菜单展开:CPU/内存/磁盘/网速全部提升到 1 秒,Disk read/write 也提升到 1 秒。菜单栏顶部与下拉 dashboard 使用同一份 `SystemStatsSnapshot`,不引入第二套口径。

### 平台差异

- macOS:默认开启,常驻 status item bitmap。
- Windows:不支持。`system_stats_supported=false`,所有 `system_stats_items` mask off。`PUT /api/config/tray` 携带系统状态字段时返回 400。
- Linux:tray helper 本身不支持,`supported=false` 且 `system_stats_supported=false`。

## 技术细节

### 配置模型

`TrayConfig`:

```rust
pub struct TrayConfig {
    pub enabled: bool,
    pub show_system_stats: bool,
    pub system_stats_items: SystemStatsItems,
}

pub struct SystemStatsItems {
    pub cpu: bool,
    pub memory: bool,
    pub disk: bool,
    pub upload: bool,
    pub download: bool,
}
```

序列化 TOML round-trip 保留字段;缺省字段按 `true` 反序列化,由 `serde` 层的 `#[serde(default = "…")]` + `Default` impl 承担。

### Admin API

`GET /api/config/tray` 与 `GET /api/config`(tray 段)返回:

```json
{
  "enabled": true,
  "show_system_stats": true,
  "system_stats_items": { "cpu": true, "memory": true, "disk": true, "upload": true, "download": true },
  "supported": true,
  "system_stats_supported": true
}
```

`PUT /api/config/tray` 允许部分 patch:

- macOS:可单独更新 `enabled`、`show_system_stats` 或 `system_stats_items.<key>`。
- Windows/Linux:只允许更新 `enabled`;携带 `show_system_stats` 或 `system_stats_items` 时返回 400 `system stats not supported on this platform`。

### CLI 相关

系统状态由 tray helper 进程持有,没有专门的 `bifrost tray` 子命令。运维排障途径:

- `bifrost __tray`(hidden):tray helper 入口,由主进程 spawn。
- `BIFROST_TRAY_NETTOP_STATS=1 bifrost __tray`:启用 `nettop` 诊断采样。
- `BIFROST_DISABLE_TRAY=1`:禁止拉起 tray helper(供 CI / E2E)。

用户面向的开关仍统一在 Settings > Tray 页面或 `PUT /api/config/tray`,避免命令行状态与前端脱轨。

### Web UI

`web/src/pages/Settings/tabs/TrayTab.tsx`:

- macOS:Tray Icon Switch + `System Stats` 分组(总开关 + CPU/Memory/Disk/Upload/Download 5 个子 Switch)。总开关 off 时子开关灰置。
- Windows/Linux:只渲染 Tray Icon Switch,并在提示区说明“系统状态在当前平台不可用”。
- 保存通过 `configApi.updateTray(patch)` 走 `PUT /api/config/tray`,失败时展示 antd `message.error`。

### 菜单栏 bitmap 渲染

- `system_stats::render_menubar_bitmap` 生成 36px 高透明模板图像,左侧绘制原尺寸 Bifrost 模板 icon,右侧按稳定列宽绘制 `C{cpu}% | M{mem}% | D{disk}% | ↑{up} ↓{down}`。
- 相邻两拍数值变化但列宽不变时,复用同尺寸 `NSBitmapImageRep` 复制像素并 `set_image`,不改变 `NSStatusItem` 宽度。
- 展开菜单期间允许同尺寸重绘顶部数值,防止 dashboard 已刷新而顶部仍停在旧值。
- 关闭 `show_system_stats`、关闭所有子项或服务未 Running 时回落到普通 Bifrost 图标。

### 采样与网速兜底

- 网速优先使用默认路由接口的 `if_data.ifi_ibytes/ifi_obytes` delta;`route -n get default` 拿不到时回退到最活跃非虚拟物理接口。
- 本机代理场景:物理接口下行 ≈ 0 且 `lo0` 明显大于物理上行,则用 `lo0` 较大方向补 download,upload 仍来自物理接口,避免把 loopback 双向镜像误报为上传。
- 首次采样、网卡新增/移除、间隔 < 900ms 时不参与速率计算。
- Hysteresis:候选接口吞吐 > 当前接口 2 倍才切换。60/40 EMA 平滑降低单样本尖峰。
- 子项关闭时跳过对应重采样(CPU 关闭不读 tick,Network 全关闭停网络采样器等)。

## Sync 边界

- Tray 相关配置属于本机偏好,不进入 rule/group sync 通道,`sync` 客户端过滤 `TrayConfig`。
- 团队/组织无法通过共享规则把某个用户的 tray 系统状态开关远端翻掉。
- 已存在的本地 `unified_config.toml` 升级后仍可读,自动补全新增字段。

## Phase 1 - 4

### Phase 1:配置与 API

- 扩展 `TrayConfig` 增加 `show_system_stats` + `system_stats_items` + `Default` impl。
- Admin `GET/PUT /api/config/tray` 返回 `supported/system_stats_supported`,PUT 分平台校验。
- 单元测试覆盖默认值、部分 TOML round-trip、Windows 400 分支。

### Phase 2:tray helper 采样与渲染

- 新增 `system_stats.rs`:`SystemStatsSnapshot`、采样线程、格式化、bitmap 渲染。
- 菜单栏 status item 使用透明模板图 + 常规字体单行文本;`NativeMenuState::refresh_in_place` 保持结构稳定。
- Windows/Linux 不启动采样线程。

### Phase 3:Web UI

- `Settings > Tray` tab、TrayTab.tsx、config API 增加 `updateTray` patch。
- Playwright `web/tests/ui/admin-settings.spec.ts` 覆盖 macOS/Windows fixture 差异。

### Phase 4:文档 / human_tests

- `human_tests/cli-tray-helper.md`、`cli-tray-dashboard.md` 补充系统状态用例。
- README/Skill 说明 `BIFROST_TRAY_NETTOP_STATS`、`BIFROST_DISABLE_TRAY` 环境变量。

## 测试方案

### 单元测试

- `bifrost-storage`:
  - `TrayConfig::default_show_system_stats_true`
  - `TrayConfig::partial_system_stats_items_defaults_missing_to_true`
  - `TrayConfig round-trip toml preserves fields`
- `bifrost-admin` handlers:
  - `GET /api/config/tray` 返回 `system_stats_supported` 与平台匹配。
  - `PUT /api/config/tray` macOS 部分 patch 只更新一个子项。
  - 非 macOS 携带系统状态字段返回 400。
- `bifrost-cli` (`tray_tests.rs`,全部真实存在):
  - `test_menu_bar_stats_title_uses_running_system_stats_only`
  - `test_menu_bar_stats_title_does_not_include_tls_state`
  - `test_menu_bar_stats_bitmap_is_compact_and_non_empty`
  - `test_menu_bar_stats_columns_align_value_and_label_rows`
  - `test_native_stats_view_is_default_on_with_explicit_opt_out`
  - `test_system_stats_config_watcher_keeps_previous_config_on_parse_error`
  - `test_native_menu_bar_stats_rows_convert_single_row_to_reference_layout`
  - `test_native_menu_bar_stats_rows_follow_each_tray_switch`
  - `test_native_network_column_uses_same_font_for_up_and_down_rows`
  - `test_native_network_column_uses_fixed_slot_with_graphic_arrow`
  - `test_native_stats_accessibility_label_reflects_status_item_content`
  - `test_native_menu_bar_stats_bitmap_uses_full_height_and_continuous_separators`
  - `test_native_menu_bar_stats_bitmap_reuses_same_size_buffer`
  - `test_macos_menu_hides_system_stats_rows`
  - `test_native_menu_bar_stats_keeps_same_size_for_live_value_refresh`
  - `test_pure_tray_icon_event_does_not_refresh_native_menu`
  - `test_background_changes_request_native_menu_refresh`

### E2E 测试

- `e2e-tests/tests/test_tray_system_stats_config.sh`:
  - 默认 `GET /api/config/tray` 字段完整。
  - macOS fixture 下关闭/开启 `show_system_stats`、单独关闭/开启 `system_stats_items.download`。
  - 非 macOS fixture 下 `system_stats_supported=false`、所有 items mask off、系统状态更新 400。
- `web/tests/ui/admin-settings.spec.ts` Playwright 覆盖 Settings Tray tab 交互。
- Tray helper 冒烟继续使用 `test_cli_tray_startup_ci.sh`。

### 真实场景测试

`human_tests/cli-tray-helper.md`(已经在仓库中):

- TC-Tray-01:macOS 默认启动 tray,菜单栏可见 `C/M/D/↑/↓`,原图标视觉大小不变。
- TC-Tray-02:关闭 `show_system_stats`,菜单栏回落到普通图标,dashboard 隐藏系统状态。
- TC-Tray-03:关闭 `system_stats_items.download`,菜单栏只剩 `↑`,dashboard `Download` 隐藏。
- TC-Tray-04:切换 Wi-Fi/有线/VPN,展示值不发生离谱尖峰。
- TC-Tray-05:执行 `curl https://speed.cloudflare.com/__down?bytes=10485760`,菜单栏 `↓` 与 Activity Monitor `Data received/sec` 数量级一致。
- TC-Tray-06:Windows/Linux fixture,Settings Tray tab 只显示 Tray Icon,`PUT` 携带系统状态字段返回 400。
- TC-Tray-07:release 采样 15 秒 warm-up + 120 秒 1 秒采样,平均 CPU < 1%,最大 CPU < 1.5%。

启动使用临时 `BIFROST_DATA_DIR`、非 9900 端口、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_DISABLE_TRAY=0`(需要 tray)、`--no-system-proxy`。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核用户目标:macOS 默认开启、菜单栏单行、Settings 独立 Tray tab、Windows 不支持系统状态。
- Review 修改文件:`TrayConfig`、admin handler、`system_stats.rs`、`menu.rs`、`tray.rs`、`TrayTab.tsx`、`configApi`、tray_tests。
- 复跑:`cargo test -p bifrost-storage tray`、`cargo test -p bifrost-admin tray`、`cargo test -p bifrost-cli tray`、`bash e2e-tests/tests/test_tray_system_stats_config.sh`、`pnpm --filter web test`。

### 第 2 轮

- 复查第 1 轮修复,确认没有把 Bifrost 进程指标误用为系统指标。
- 重点看:采样线程在子项全关闭时是否真的停止读取;menubar bitmap 是否只在同尺寸时复用 buffer;`PUT` 400 消息稳定可测;Windows fixture Playwright 通过。
- 复跑受影响测试,执行 `human_tests/cli-tray-helper.md`,补齐 coverage 门禁。

## 风险与决策

- **`nettop` 常驻默认开启**:实测本机 CPU 33–128% 不可接受,已放到 `BIFROST_TRAY_NETTOP_STATS` 显式开关。默认使用接口计数器 + loopback 下行兜底。
- **同一菜单展开期间顶部与下拉展示口径不同**:已强制 `SystemStatsSnapshot` 作为唯一数据源;菜单栏与 dashboard 主值一律走 `format_percent`,菜单关闭后 CPU/SSD 才降精度到 1% 桶。
- **Windows 支持系统状态**:notification area 无法承载横向常驻文本,把系统信息塞进弹出菜单意味着必须频繁展开;不做,只保留 Tray Icon 开关。
- **`kern.memorystatus_level` 当主百分比**:与 Activity Monitor 差异过大,只作为健康度诊断使用。
- **系统状态子项刷新频率暴露给用户**:第一版不做用户可配置刷新频率,统一 1 秒;`set_icon` bitmap 复用与列宽稳定已消除主要重绘成本。若真实用户机器出现 CPU 增高,先排查 `set_icon` / native menu 重建,而不是升配置项。
