# CLI Tray Dashboard Header

## 功能模块说明

验证 macOS Bifrost 托盘菜单顶部 dashboard header：点击菜单栏状态项后，原生下拉菜单顶部展示 CPU、内存、磁盘和网络细节；Bifrost 运行状态和版本号移动到菜单底部，且原菜单操作保持可用。

## 前置条件

1. 在 macOS 上执行。
2. 使用本次构建产物，避免旧二进制影响验证。
3. 使用临时数据目录，避免污染真实配置。
4. 启动服务时必须禁用系统代理和 Sync 自动登录弹窗：
   ```bash
   export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
   export BIFROST_E2E_ALLOW_TRAY=1
   export BIFROST_DATA_DIR="$(mktemp -d /tmp/bifrost-tray-dashboard.XXXXXX)"
   cargo build --bin bifrost
   ./target/debug/bifrost start -d --host 127.0.0.1 -p 18890 --unsafe-ssl --no-system-proxy --skip-cert-check
   ```
5. 确认服务 ready：
   ```bash
   curl -sS http://127.0.0.1:18890/_bifrost/api/proxy/address
   ```

## 测试用例列表

### TC-TDH-01 菜单顶部展示 dashboard header

**操作步骤**：

1. 等待 Bifrost 托盘 helper 出现在 macOS 菜单栏。
2. 点击 Bifrost 菜单栏状态项打开下拉菜单。
3. 观察菜单顶部 header。

**预期结果**：

- 下拉菜单顶部展示一块固定高度 dashboard header。
- Header 包含 CPU、Memory、Disk、Network 四个区域，不再重复展示 `Bifrost` 标题和 Running 状态。
- Header 使用不同颜色区分状态和网络方向。
- CPU 区域不展示 Bifrost 自身进程负载或 load average；右侧只展示逻辑核心数，支持时显示 P/E 核心分布，例如 `cores 16 (P12 / E4)`。
- Network 区域下方有一条分隔线，把 header 和 Open Traffic 等普通菜单项隔开。
- Header 下方仍显示 Open Traffic、Open Rules、Open Settings、Copy Proxy 等原有菜单项。
- 菜单底部展示 `Bifrost: Running on ...` 和 `Version ...` 两行信息。

### TC-TDH-02 内存和磁盘细节展示

**操作步骤**：

1. 在菜单打开状态观察 Memory 区域。
2. 观察 Disk 区域。

**预期结果**：

- Memory 区域左侧主值展示与顶部 `MEM` 一致的 memory used 百分比，后面用较小字号括号展示基于 pressure 的健康状态，例如 `70% (Healthy)`、`70% (Pressure)` 或 `86% (Critical)`；无 pressure 时健康状态也用 used 百分比兜底。
- Memory 区域右侧第一行展示 used/total，第二行展示 compressed、cached、swap 信息；不可用时显示 `--` 而不是空白或崩溃。
- Disk 区域第一行展示 free/total。
- Disk 区域第二行展示 read/write 瞬时吞吐；首次打开菜单时允许短暂显示 `collecting`，约 1 秒采样窗口后显示类似 `read 12.3M/s   write 4.8M/s`，无 I/O 时显示 `0B/s` 而不是空白。

### TC-TDH-03 菜单打开期间刷新不关闭菜单

**操作步骤**：

1. 打开 Bifrost 下拉菜单并保持 5 秒以上。
2. 观察顶部 header 的 CPU/Network/Disk 数值是否刷新。
3. 不点击其他位置，确认菜单是否保持打开。

**预期结果**：

- 菜单不会因为后台 stats 更新而自动关闭。
- Header 可以刷新数值。
- 下方菜单项仍可点击；点击 Open Traffic 可以打开管理页。

### TC-TDH-04 idle 状态不持续重绘 dashboard

**操作步骤**：

1. 使用临时数据目录启动本次 debug build。
2. 不打开托盘菜单，等待 dashboard header 完成首次安装。
3. 观察 `tray.log`，确认 header 只安装一次，普通 stats 更新不触发持续 `tray menu refreshed`。
4. 连续 30 秒采样 tray helper 的 `%CPU` 和 RSS。

**预期结果**：

- `tray.log` 只出现一次 `native tray dashboard header installed ...`。
- 菜单关闭期间不因为 dashboard stats 更新持续重建原生菜单、刷新 dashboard bitmap 或推进隐藏详细行 generation。
- 30 秒 idle 平均 CPU 低于 1%；RSS 不出现持续单调增长。

### TC-TDH-05 长 Memory 明细不被菜单宽度裁剪

**操作步骤**：

1. 执行 dashboard 渲染回归：
   ```bash
   cargo test -p bifrost-cli dashboard -- --nocapture
   ```
2. 使用本次源码启动 macOS tray helper，打开下拉菜单。
3. 观察 Memory 区域第二行，尤其是 `comp ...  cache ...  swap ... / ...`。
4. 查看 `tray.log` 中 `native tray dashboard header installed` 的 `width` 字段。

**预期结果**：

- `render_dashboard_expands_width_for_long_memory_detail_line` 通过，证明 bitmap 宽度至少覆盖长 Memory 明细右边缘。
- 下拉菜单顶部 header 宽度随右侧最长文本扩展，Memory 第二行完整显示，不出现 `swap used / total` 右侧被裁剪。
- `NSImageView` frame 使用实际 bitmap 宽度；日志中的 `width` 可大于旧固定值 `340`，但高度保持不变。

### TC-TDH-06 菜单打开期间顶部指标与 dashboard 同步刷新

**操作步骤**：

1. 执行顶部 status item 渲染回归：
   ```bash
   cargo test -p bifrost-cli test_native_menu_bar_stats_keeps_same_size_for_live_value_refresh -- --nocapture
   ```
2. 使用本次源码启动 macOS tray helper，保持系统状态 Upload/Download 开启。
3. 通过 Bifrost HTTP proxy 触发持续下载流量，例如：
   ```bash
   curl -x "http://127.0.0.1:<port>" -k -L \
     "https://speed.cloudflare.com/__down?bytes=20971520" -o /dev/null
   ```
4. 下载过程中打开 Bifrost 下拉菜单，连续观察顶部菜单栏 status item 与下拉 dashboard 的 CPU、Memory、Disk、Network 区域。

**预期结果**：

- 网速采样始终保持 1 秒心跳；后台不把 Upload/Download 降到 3 秒或更慢。
- 菜单展开期间，顶部 status item 与下拉 dashboard 使用同一份系统状态 snapshot 刷新；CPU、MEM、SSD、up/down 主值不会长期停留在不同时间点。
- 刷新方案是动态的：菜单关闭时 Network 继续约 1 秒采样，CPU/Memory 按 3 秒后台窗口复用最近值，Disk 容量按 30 秒后台窗口复用最近值且不采 read/write；菜单打开时 CPU/Memory/Disk 容量、Disk read/write 与 Network 主值都提升到约 1 秒刷新。
- 顶部 status item 在菜单关闭后台状态可以对 CPU/SSD 百分比做 1% 整数桶以减少小数抖动；菜单打开前台对照时必须关闭桶化，CPU/SSD 与 dashboard 主值精确到同一 `format_percent` 口径。
- dashboard 左侧 Memory 主值与顶部 `MEM` 使用同一个 memory used 百分比口径；pressure 只影响括号健康状态和颜色。
- 同尺寸 status item bitmap 更新不会关闭下拉菜单，也不会改变原生菜单结构。
- dashboard 仍保持 TC-TDH-05 的宽度修复，Memory 明细不被裁剪。

## 清理步骤

1. 停止 Bifrost 服务：
   ```bash
   BIFROST_DATA_DIR="$BIFROST_DATA_DIR" ./target/debug/bifrost stop
   ```
2. 如 tray helper 仍在，执行：
   ```bash
   pkill -f "bifrost __tray --data-dir $BIFROST_DATA_DIR" || true
   ```
3. 删除临时数据目录：
   ```bash
   rm -rf "$BIFROST_DATA_DIR"
   ```

## 执行记录

| 日期 | 用例 | 结果 | 说明 |
| --- | --- | --- | --- |
| 2026-06-23 | TC-TDH-06 | 已执行，通过 | 根据用户反馈“后台运行 CPU 应在 1% 以内”，对当前已运行安装版 `9900` 进程做只读采样：daemon PID `88150`、tray helper PID `88204`。`ps` 1 秒采样显示 tray helper 存在 `2%` 到 `6.8%` 尖峰，30 秒 CSV 统计中 tray helper `avg_cpu=1.689 p95_cpu=5.400 max_cpu=5.700 last_rss_kb=96288`；`sample 88204 15` 显示热点主要落在 AppKit `NSStatusItem _updateReplicants` / `cacheDisplayInRect` / `drawStylizedImage` / `CGContextClipToMask`，说明 1% 桶本身不是采集成本来源，但会让后台标题变化更频繁并触发 AppKit 重绘。根据用户进一步确认，最终不采用整条标题后台重绘节流，以免破坏网速 1 秒刷新；改为后台 Network 保持 1 秒采样，CPU/Memory 按 3 秒窗口采样和复用，Disk 后台只读容量且按 30 秒窗口刷新，Disk read/write 仅菜单展开时按 1 秒采样，菜单展开时 CPU/Memory/Disk 容量与 Network 仍全部 1 秒。启动最新源码调试实例 `/tmp/bifrost-tray-bg-perf.8ECO4C`，端口 `59934`、daemon PID `82935`、tray PID `82989`，API 返回 system stats 和 cpu/memory/disk/upload/download 全部开启；warm-up 后关闭菜单后台 60 秒采样结果为 `avg_cpu=0.477 p95_cpu=1.300 max_cpu=2.000 last_rss_kb=101936`，均值低于 1%。继续检查 tray 写盘行为：`lsof -p 82989` 显示 tray helper 长期开启 `logs/tray.log.2026-06-23` 和 `tray.lock` 写句柄；日志文件大小采样显示此前因 `/api/group` 返回 502，每约 60 秒写入两条 WARN，文件从 `7326` bytes 增至 `7573` bytes。修复后 remote group 同一轮故障只首次 WARN，后续重复失败降为 DEBUG 并避免外层重复 WARN，降低后台周期性日志写盘。执行 `BIFROST_BIN="$PWD/target/debug/bifrost" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 bash e2e-tests/tests/test_tray_system_stats_config.sh` 通过，`63 passed, 0 failed`。 |
| 2026-06-23 | TC-TDH-06 | 已执行，性能优化探索收口 | 针对菜单展开后 dashboard 明细带来的 CPU 损耗，用 release binary、临时数据目录和真实 macOS 原生菜单展开状态采样。基线实例 `/tmp/bifrost-tray-perf-open.39udqD` 端口 `60733`、tray PID `63368`，60 个 1 秒样本为 `avg_cpu=0.6417 max_cpu=3.0000`；`sample` 显示一类尖峰来自 AppKit `NSStatusItem` snapshot/redraw。保留低风险优化：同尺寸 status item bitmap 更新时只拷贝像素并 `setNeedsDisplay`，不再重复 `setImage`/`setImagePosition`。随后发现高 CPU 样本中 `bifrost-tray-system-stats` 线程大量命中 IOKit `IOServiceGetMatchingService`，因此保留 `IOBlockStorageDriver` service handle 缓存，避免菜单展开期间每秒重新匹配 service；Disk read/write Statistics 仍保持 1 秒读取，Network 仍保持 1 秒采样。曾尝试把 Disk read/write 明细降到 5 秒窗口，但用户确认这会损害展开菜单实时性，已撤回，最终版本不降低展开态刷新频率。执行 `cargo fmt --all -- --check` 通过；执行 `cargo test -p bifrost-cli system_stats -- --nocapture` 通过 lib/main 各 43/43；执行 `cargo test -p bifrost-cli test_native_menu_bar_stats_keeps_same_size_for_live_value_refresh -- --nocapture` 通过 lib/main 各 1/1。 |
| 2026-06-23 | TC-TDH-06 | 已执行，等待人工目视确认 | 根据用户反馈后台 5% 桶过粗，将菜单关闭后台状态的 CPU/SSD 百分比从 5% 桶改为 1% 整数桶：例如 `Disk 8.6%` 后台显示 `D9%`，不再压到 `D5%`。菜单展开前台仍继续走 exact `format_percent`，不受该后台桶影响；Network 仍保持 1 秒采样。执行 `cargo fmt --all -- --check` 通过；执行 `cargo test -p bifrost-cli menu_lines_rounds_background_menu_bar_percent_fields_to_one_percent -- --nocapture` 通过；执行 `cargo test -p bifrost-cli system_stats -- --nocapture` 通过 lib/main 各 43/43；执行 `cargo clippy -p bifrost-cli --all-targets --all-features -- -D warnings` 通过。 |
| 2026-06-23 | TC-TDH-06 | 已执行，等待人工目视确认 | 针对用户截图中顶部 `SSD 90%` 而 dashboard `Disk 93%`、以及 CPU 也存在类似不一致的问题，确认根因是顶部 status item 在后台性能优化中对 CPU/Disk 百分比做 5% 桶化，菜单展开后仍沿用了该降精度显示。修复后保留后台桶化减少菜单关闭时的 status item 重绘；菜单打开期间调用 `menu_lines_for_menu_state(..., menu_is_open=true)`，CPU/SSD 使用与 dashboard 相同的 `format_percent` 口径，精确到个位数或小数位。执行 `cargo test -p bifrost-cli menu_lines_uses_exact_menu_bar_percent_fields_while_menu_is_open -- --nocapture` 通过，lib/main 各 1/1，固定验证 `CPU 26.4` 和 `Disk 92.7` 在前台输出 `C26%`、`D93%`；执行 `cargo test -p bifrost-cli system_stats -- --nocapture` 通过，lib/main 各 43/43；执行 `cargo test -p bifrost-cli commands::tray -- --nocapture` 通过，lib/main 各 133/133。已重新编译并重启最新源码调试实例 `/tmp/bifrost-tray-net-live.ZItL1s`，daemon PID `90560`、tray PID `90605`、端口 `59933`，`GET /_bifrost/api/config/tray` 返回系统状态与 cpu/memory/disk/upload/download 全部开启，`tray.log.2026-06-23` 在 `05:05` 后记录 `native macOS tray stats view enabled as primary status item` 与 `native tray dashboard header installed items=14 width=375 height=176`；`pgrep -x nettop` 无输出，证明默认仍未启用高成本 nettop。真实视觉需用户打开底部显示 `127.0.0.1:59933` 的菜单确认。 |
| 2026-06-23 | TC-TDH-06 | 已执行，等待人工目视确认 | 针对用户确认“核心是统一来源、统一刷新方案，只是在不同情况下动态改变刷新频率”，将 tray 系统状态收敛为单一 `SystemStatsSnapshot` 数据源：顶部 status item 与下拉 dashboard 都从同一份快照渲染；菜单未展开时 Network 保持 1 秒系统线程和 900ms 最小采样窗口，CPU/Memory 按 3 秒后台窗口复用最近值，Disk 容量按 30 秒后台窗口复用最近值，Disk read/write 只在菜单展开时采样；菜单展开时 CPU/Memory/Disk 容量与 Network 主值统一提升到约 1 秒刷新。执行 `cargo test -p bifrost-cli tray::system_stats::tests::sample_refreshes_main_metrics_every_second_while_menu_is_open -- --nocapture` 通过，lib/main 各 1/1；执行 `cargo test -p bifrost-cli commands::tray -- --nocapture` 通过，lib/main 各 132/132；执行 `BIFROST_BIN="$PWD/target/debug/bifrost" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 bash e2e-tests/tests/test_tray_system_stats_config.sh` 通过 63/63；执行 `BIFROST_BIN="$PWD/target/debug/bifrost" BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1 SKIP_BUILD=true bash e2e-tests/tests/test_cli_tray_startup_ci.sh` 通过。已重新编译并重启最新源码调试实例 `/tmp/bifrost-tray-net-live.ZItL1s`，daemon PID `49478`、tray PID `49578`、端口 `59933`，`GET /_bifrost/api/config/tray` 返回系统状态与 cpu/memory/disk/upload/download 全部开启，`tray.log.2026-06-23` 记录 `native macOS tray stats view enabled as primary status item` 与 `native tray dashboard header installed items=14 width=375 height=176`；通过 `curl -x http://127.0.0.1:59933 -k -L https://speed.cloudflare.com/__down?bytes=5242880` 触发 10 轮下载，速度约 `1.9MB/s` 到 `3.2MB/s`，用于用户打开底部显示 `127.0.0.1:59933` 的菜单目视确认。 |
| 2026-06-23 | TC-TDH-06 | 已执行，等待人工目视确认 | 根据用户再次截图反馈，确认旧实现存在两类问题：顶部 status item 与 dashboard 同尺寸更新未稳定重绘，以及 Memory 主值口径不同（顶部 `MEM` 为 used%，dashboard 左侧为 pressure%）。修复后 dashboard 左侧 Memory 改为 used%，pressure 仅作为健康状态；重启最新源码实例 `/tmp/bifrost-tray-net-live.ZItL1s`，daemon PID `2745`、tray PID `2775`，`tray.log.2026-06-23` 记录 `native tray dashboard header installed items=14 width=375 height=176`。通过 `curl -x http://127.0.0.1:59933 -k -L https://speed.cloudflare.com/__down?bytes=10485760` 触发 2 轮下载，样本下载速度约 `637445`、`1784688` B/s，tray helper 采样 CPU `0.1%`，`pgrep -x nettop` 无输出。当前本机同时存在安装版 9900 tray helper 和调试版 59933 tray helper，真实目视需点开底部显示 `127.0.0.1:59933` 的菜单项。 |
| 2026-06-23 | TC-TDH-06 | 已执行，等待人工目视确认 | 针对用户截图中顶部菜单栏与下拉 dashboard 的 CPU/MEM/SSD/Network 主值不同步、Memory 主值口径不同的问题，保留 stats 线程 1 秒心跳和 900ms 网络最小采样窗口，修复同尺寸 native status item bitmap 在菜单打开期间的重绘路径，并将 dashboard 左侧 Memory 主值改为与顶部 `MEM` 相同的 used 百分比，pressure 只作为健康状态。执行 `cargo test -p bifrost-cli test_native_menu_bar_stats_keeps_same_size_for_live_value_refresh -- --nocapture` 通过，lib/main 各 1/1，证明截图类数值变化不会改变顶部 bitmap 宽高，可安全复用 image rep 刷新像素。使用临时数据目录 `/tmp/bifrost-tray-net-live.ZItL1s`、端口 `59933` 重启最新源码，daemon PID `87615`、tray PID `87665`，`tray.log.2026-06-23` 记录 `native tray dashboard header installed items=14 width=375 height=176`；通过 `curl -x http://127.0.0.1:59933 -k -L https://speed.cloudflare.com/__down?bytes=20971520` 触发 3 轮下载，样本下载速度约 `264314`、`1351506`、`1671433` B/s，`pgrep -x nettop` 无输出，证明默认未启用高成本 nettop。由于本机 `osascript` 辅助功能读取菜单被 TCC 拒绝，顶部与 dashboard 同步的最终视觉以用户桌面目视确认为准。 |
| 2026-06-23 | TC-TDH-05 | 已执行，等待人工目视确认 | 针对用户截图中 Memory 第二行 `swap used / total` 被右侧裁剪的问题修复 dashboard 宽度计算。执行 `cargo test -p bifrost-cli dashboard -- --nocapture` 通过，lib/main 各 9/9，新增 `render_dashboard_expands_width_for_long_memory_detail_line` 覆盖 `comp/cache/swap 10.6G / 13.4G` 长明细，断言 bitmap 宽度覆盖右侧文本边缘。使用临时数据目录 `/tmp/bifrost-tray-net-live.ZItL1s`、端口 `59933` 通过 `cargo run --bin bifrost -- start -d -y --host 127.0.0.1 -p 59933 --unsafe-ssl --no-system-proxy --skip-cert-check` 启动最新源码，`tray.log.2026-06-23` 记录旧版本 header 为 `width=340 height=176`，修复后为 `width=375 height=176`；本机 `osascript` 辅助功能读取菜单被 TCC 拒绝，最终视觉以用户桌面目视确认为准。 |
| 2026-06-22 | TC-TDH-01 | 部分通过，需人工视觉确认 | 已使用 `/tmp/bifrost-tray-dashboard.A4Uf6G` 启动本次 `target/debug/bifrost`，API ready，daemon PID 24085，tray PID 24163，`tray.log.2026-06-22` 记录 `native tray dashboard header installed items=14 width=340 height=164`；自动截图发现当前 macOS 处于锁屏界面，无法点开托盘菜单做视觉确认，需解锁后补截图 |
| 2026-06-22 | TC-TDH-02 | 部分通过，需人工视觉确认 | `cargo test -p bifrost-cli commands::tray -- --nocapture` 已验证 Memory/Disk 文案、颜色阈值、fallback、非空渲染和 Disk I/O counter delta；生产采集改为菜单打开期间通过 IOKit 读取 read/write counter，原生下拉视觉细节需解锁后打开菜单确认 |
| 2026-06-22 | TC-TDH-03 | 部分通过，需人工视觉确认 | 已修复普通菜单轮询把 dashboard snapshot 置空导致 header 闪烁的问题；新日志中 header 只安装一次，后续 group 502 未再触发 `tray menu refreshed in place`；菜单打开 5 秒不关闭需解锁后手动点开状态项确认 |
| 2026-06-22 | TC-TDH-04 | 通过 | 优化前 debug tray helper idle 约 `3.3% CPU / 109456 KB RSS`；IOKit 采样阶段关闭菜单 idle 30 秒为 `samples=30 avg_cpu=1.0033 min_cpu=0.0 max_cpu=4.9 avg_rss_kb=103095 min_rss_kb=103008 max_rss_kb=103296 rss_delta_kb=288`；最终优化后关闭菜单 idle 30 秒为 `samples=30 avg_cpu=0.5733 min_cpu=0.0 max_cpu=7.3 avg_rss_kb=87370 min_rss_kb=85744 max_rss_kb=89392 rss_delta_kb=3648`。日志确认 header 安装一次，菜单关闭时不更新 dashboard snapshot，group list 502 warning 退避到约 60 秒 |
