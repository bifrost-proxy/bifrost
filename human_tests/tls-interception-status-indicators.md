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

### TC-TLS-SI-03 Tray 顶部不展示 TLS 角标

操作步骤：

1. 使用临时数据目录启动允许 tray 的 Bifrost，并开启全局 HTTPS Interception。
2. 观察系统 tray/menu bar 图标和 macOS 系统状态标题。
3. 打开 tray 下拉菜单，查看 TLS 状态。

预期结果：

- 全局 HTTPS Interception 启用时，tray 顶部图标不出现 TLS 角标。
- macOS 启用系统状态标题时，菜单栏标题不显示 `TLS` 短标签。
- TLS 状态只在下拉菜单的 `TLS Interception: On/Off` 行展示。

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

### TC-TLS-SI-05 Tray TLS 切换不影响顶部标题

操作步骤：

1. 使用临时数据目录启动允许 tray 的 Bifrost，启用 macOS 系统状态标题。
2. 打开 tray 下拉菜单，点击 `TLS Interception: Off`。
3. 观察菜单栏系统状态标题。
4. 再点击 `TLS Interception: On` 关闭。

预期结果：

- tray 自己发起 TLS 开关后，下拉菜单进入 pending 并最终更新 `TLS Interception: On/Off`。
- 不新增 1 秒级常驻 HTTP 轮询；外部 Web UI 修改仍复用现有菜单数据刷新。
- 菜单栏系统状态标题只展示 CPU/MEM/SSD/网络状态，不拼接 `TLS`。
- 开关前后 tray 顶部图标不出现 TLS 角标。

## 清理步骤

1. 停止测试服务：`bifrost stop` 或终止本次临时数据目录对应的测试进程。
2. 确认没有残留 tray helper。
3. 删除本次测试使用的临时数据目录。

## 执行记录

| 日期 | 用例 | 结果 |
| --- | --- | --- |
| 2026-06-26 | TC-TLS-SI-01 | 通过。执行 `pnpm --dir web exec vitest run src/components/StatusBar/statusIndicators.test.ts`，3 个状态派生单测通过；执行 `pnpm --dir web exec playwright test tests/ui/admin-settings.spec.ts -g "底部状态栏展示全局 HTTPS Interception 动画警示"`，真实 Chromium 先通过 Admin API 写入 `enable_tls_interception=true`，再打开 Traffic 页面，断言底部状态栏 `data-tls-state=full`、显示 `TLS: Full On`、active pulse/jump class 存在，点击后跳转到 `/_bifrost/settings?tab=tls`，最后恢复原 TLS 配置。 |
| 2026-06-26 | TC-TLS-SI-02 | 通过。同一 Playwright 用例在亮色主题和点击 `theme-toggle` 后的暗色主题分别断言 `TLS: Full On` 可见、active class 存在、文本颜色不等于页面背景色、状态块宽高未塌缩，未发现布局溢出或遮挡。 |
| 2026-06-26 | TC-TLS-SI-03 | 通过。按用户反馈取消 tray 顶部 TLS 角标后，执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli commands::tray --lib --all-features -- --nocapture`，150 个 tray 单测通过；其中 `test_menu_bar_stats_title_does_not_include_tls_state` 验证全局 TLS 开启时 macOS 系统状态标题仍只展示 `C20% | M55%`，不拼接 `TLS`。 |
| 2026-06-26 | TC-TLS-SI-04 | 通过。真实 tray 预览展开菜单后，System Events 读取菜单项顺序包含 `Stop Bifrost -> System Proxy -> TLS Interception: On -> Open Logs`，确认 `System Proxy` 未被改成子菜单，`TLS Interception: On` 作为独立可操作行展示在其下方。 |
| 2026-06-26 | TC-TLS-SI-05 | 通过。执行 `SKIP_FRONTEND_BUILD=1 cargo test -p bifrost-cli commands::tray --lib --all-features -- --nocapture`，150 个 tray 单测通过；其中 `test_tls_pending_action_updates_menu_snapshot_without_badge_title` 验证 tray 点击 TLS 开关进入 pending 时本地 snapshot 立即变为 enabled，但 macOS 系统状态标题仍保持 `C20% | M55%`，TLS 状态只留在下拉菜单。 |
