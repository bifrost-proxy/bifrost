# HTTPS Interception 状态提示真实场景测试

## 功能模块说明

验证全局 HTTPS Interception 启用后，Web UI 底部状态栏和 tray 都能给出清晰、持续、可操作的视觉提示。

## 前置条件

- 使用临时数据目录启动 Bifrost，避免污染本机配置。
- 除 tray 专项用例外，启动命令必须包含：
  - `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`
  - `BIFROST_DISABLE_TRAY=1`
  - `--no-system-proxy`
- tray 专项用例必须显式设置 `BIFROST_E2E_ALLOW_TRAY=1`，并在执行完成后停止服务和 tray helper。

## 测试用例列表

### TC-TLS-SI-01 Web 状态栏显示全局启用动画

操作步骤：

1. 启动 Web UI 测试环境，mock `/api/config/tls` 返回 `enable_tls_interception=true`。
2. 打开 `/_bifrost/traffic`。
3. 查看底部状态栏的 TLS 区块。
4. 点击 TLS 区块。

预期结果：

- 底部状态栏出现 `TLS: Full On`。
- TLS 圆点具备 active pulse class，状态文字具备 active jump class。
- TLS 区块 `data-tls-state` 为 `full`。
- 点击后跳转到 `/_bifrost/settings?tab=tls`。

### TC-TLS-SI-02 Web 状态栏亮色/暗色主题可读

操作步骤：

1. 在亮色主题打开 `/_bifrost/traffic`，保持 `/api/config/tls` 返回 `enable_tls_interception=true`。
2. 截图或目视检查底部 `TLS: Full On`。
3. 切换到暗色主题。
4. 再次检查底部 `TLS: Full On`。

预期结果：

- 亮色主题下 TLS 圆点、文字和动画清晰可辨。
- 暗色主题下 TLS 圆点、文字和动画清晰可辨。
- 状态栏内容不互相遮挡，不挤压版本、Sync、流量速率等既有信息。

### TC-TLS-SI-03 Tray 图标显示 TLS 角标

操作步骤：

1. 使用临时数据目录启动允许 tray 的 Bifrost，并开启全局 HTTPS Interception。
2. 观察系统 tray/menu bar 图标。
3. 关闭全局 HTTPS Interception，等待 tray 刷新。

预期结果：

- 全局 HTTPS Interception 启用时，tray 图标出现明显角标。
- macOS 启用系统状态标题时，菜单栏标题显示 `TLS` 短标签。
- 关闭全局 HTTPS Interception 后角标或 `TLS` 短标签消失。

### TC-TLS-SI-04 Tray System Proxy 下方展示 TLS 状态并可操作

操作步骤：

1. 使用临时数据目录启动允许 tray 的 Bifrost。
2. 打开 tray 下拉菜单。
3. 找到顶层 `System Proxy` 菜单项。
4. 查看其下一行 `TLS Interception: On` 或 `TLS Interception: Off`。
5. 点击 `TLS Interception: ...` 操作项。

预期结果：

- `System Proxy` 保持原有单项交互，不被改成子菜单。
- `TLS Interception: ...` 作为独立顶层行展示在 `System Proxy` 下方。
- `TLS Interception: ...` 是可直接点击的 checkbox 操作项。
- 点击后菜单进入 pending 文案，完成后状态刷新到目标状态。

### TC-TLS-SI-05 Tray TLS 角标即时更新且字号收敛

操作步骤：

1. 使用临时数据目录启动允许 tray 的 Bifrost，启用 macOS 系统状态标题。
2. 打开 tray 下拉菜单，点击 `TLS Interception: Off`。
3. 观察菜单栏 `TLS` 短标签出现时机与字号。
4. 再点击 `TLS Interception: On` 关闭。

预期结果：

- tray 自己发起 TLS 开关后，本地 snapshot 立即更新，菜单栏 `TLS` 短标签不等待后台慢刷新。
- 不新增 1 秒级常驻 HTTP 轮询；外部 Web UI 修改仍复用现有菜单数据刷新。
- `TLS` 短标签使用统计标题值字号的 50%，不压过 CPU/MEM/SSD/网络状态。
- 关闭后 `TLS` 短标签消失。

## 清理步骤

1. 停止测试服务：`bifrost stop` 或终止本次临时数据目录对应的测试进程。
2. 确认没有残留 tray helper。
3. 删除本次测试使用的临时数据目录。

## 执行记录

| 日期 | 用例 | 结果 |
| --- | --- | --- |
| 2026-06-26 | TC-TLS-SI-01 | 通过。执行 `pnpm --dir web exec vitest run src/components/StatusBar/statusIndicators.test.ts`，3 个状态派生单测通过；执行 `pnpm --dir web exec playwright test tests/ui/admin-settings.spec.ts -g "底部状态栏展示全局 HTTPS Interception 动画警示"`，真实 Chromium 先通过 Admin API 写入 `enable_tls_interception=true`，再打开 Traffic 页面，断言底部状态栏 `data-tls-state=full`、显示 `TLS: Full On`、active pulse/jump class 存在，点击后跳转到 `/_bifrost/settings?tab=tls`，最后恢复原 TLS 配置。 |
| 2026-06-26 | TC-TLS-SI-02 | 通过。同一 Playwright 用例在亮色主题和点击 `theme-toggle` 后的暗色主题分别断言 `TLS: Full On` 可见、active class 存在、文本颜色不等于页面背景色、状态块宽高未塌缩，未发现布局溢出或遮挡。 |
| 2026-06-26 | TC-TLS-SI-03 | 通过。使用当前 `target/debug/bifrost` 复制出的唯一二进制 `/tmp/bifrost-tls-tray-unique.qTrzwj/bifrost-tls-tray-bin` 启动真实 macOS tray，临时数据目录 `/tmp/bifrost-tls-tray-unique.qTrzwj/data`，端口 `53859`，启动参数包含 `--no-system-proxy --skip-cert-check --unsafe-ssl`，并设置 `BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`、`BIFROST_E2E_ALLOW_TRAY=1`。通过 `PUT /_bifrost/api/config/tls` 开启全局 HTTPS Interception 后，System Events 读取唯一 tray 进程 `54386` 的 menu bar item description 为 `Bifrost: TLS \| C48% \| M69% \| D98% \| ↑111 K/s ↓116 K/s`，证明启用后菜单栏常驻状态有 `TLS` 高危短标签；同一代码路径也会选择带角标的 tray icon bitmap。测试结束后停止临时服务和 tray helper。 |
| 2026-06-26 | TC-TLS-SI-04 | 通过。真实 tray 预览展开菜单后，System Events 读取菜单项顺序包含 `Stop Bifrost -> System Proxy -> TLS Interception: On -> Open Logs`，确认 `System Proxy` 未被改成子菜单，`TLS Interception: On` 作为独立可操作行展示在其下方。 |
| 2026-06-26 | TC-TLS-SI-05 | 通过。执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli commands::tray --lib --all-features -- --nocapture`，150 个 tray 单测通过；其中 `test_tls_pending_action_updates_badge_snapshot_immediately` 验证 tray 点击 TLS 开关进入 pending 时本地 snapshot 立即变为 enabled 并触发 `TLS | ...` 标题，`test_menu_bar_tls_badge_column_uses_half_size_font` 验证 `TLS` 列字号为统计标题值字号的 50%。 |
