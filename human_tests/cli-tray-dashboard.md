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
- Header 下方仍显示 Open Traffic、Open Rules、Open Settings、Copy Proxy 等原有菜单项。
- 菜单底部展示 `Bifrost: Running on ...` 和 `Version ...` 两行信息。

### TC-TDH-02 内存和磁盘细节展示

**操作步骤**：

1. 在菜单打开状态观察 Memory 区域。
2. 观察 Disk 区域。

**预期结果**：

- Memory 区域展示 used/total、pressure 或 used 百分比。
- Memory 区域展示 compressed、cached、swap 信息；不可用时显示 `--` 而不是空白或崩溃。
- Disk 区域第一行展示 free/total。
- Disk 区域第二行展示 read/write 瞬时吞吐；采样未完成时显示 `--`，采样完成后显示类似 `read 12.3M/s   write 4.8M/s`。

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
- 菜单关闭期间不因为 dashboard stats 更新持续重建原生菜单或刷新 dashboard bitmap。
- 30 秒 idle 平均 CPU 低于 1%；RSS 不出现持续单调增长。

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
| 2026-06-22 | TC-TDH-01 | 部分通过，需人工视觉确认 | 已使用 `/tmp/bifrost-tray-dashboard.A4Uf6G` 启动本次 `target/debug/bifrost`，API ready，daemon PID 24085，tray PID 24163，`tray.log.2026-06-22` 记录 `native tray dashboard header installed items=14 width=340 height=164`；自动截图发现当前 macOS 处于锁屏界面，无法点开托盘菜单做视觉确认，需解锁后补截图 |
| 2026-06-22 | TC-TDH-02 | 部分通过，需人工视觉确认 | `cargo test -p bifrost-cli commands::tray -- --nocapture` 已验证 Memory/Disk 文案、颜色阈值、fallback、非空渲染和 I/O Registry read/write parser；原生下拉视觉细节需解锁后打开菜单确认 |
| 2026-06-22 | TC-TDH-03 | 部分通过，需人工视觉确认 | 已修复普通菜单轮询把 dashboard snapshot 置空导致 header 闪烁的问题；新日志中 header 只安装一次，后续 group 502 未再触发 `tray menu refreshed in place`；菜单打开 5 秒不关闭需解锁后手动点开状态项确认 |
| 2026-06-22 | TC-TDH-04 | 通过 | 优化前 debug tray helper idle 约 `3.3% CPU / 109456 KB RSS`；优化后关闭菜单 idle 30 秒采样为 `samples=30 avg_cpu=0.3067 min_cpu=0.0 max_cpu=1.5 avg_rss_kb=95015 min_rss_kb=92544 max_rss_kb=95664 rss_delta_kb=3120`。日志确认 header 安装一次，未出现 dashboard 持续刷新导致的菜单重建；仍有 group list 502 周期 warning，后续可单独加失败退避 |
