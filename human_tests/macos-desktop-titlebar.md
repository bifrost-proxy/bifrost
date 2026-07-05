# macOS 桌面端标题栏融合真实场景测试

## 功能模块说明

验证 macOS Tauri 桌面端顶部系统标题栏与 Bifrost UI 融合，不再出现独立灰色标题栏；左侧菜单区域可用于拖拽移动窗口；暗色和亮色主题下顶部、侧栏、原生红黄绿窗口控制区均保持可读且无遮挡。

普通浏览器打开的 CLI Web UI 不应启用桌面拖拽能力，也不应出现 macOS 窗口控制预留区。

## 前置条件

- 当前工作目录为 Bifrost 仓库根目录。
- 构建桌面前端：
  ```bash
  pnpm --dir web run build:desktop
  ```
- 检查 Tauri 桌面端：
  ```bash
  cargo check --manifest-path desktop/src-tauri/Cargo.toml
  ```
- 启动桌面端时使用临时数据目录，并避免污染系统代理：
  ```bash
  export BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1
  export BIFROST_DISABLE_TRAY=1
  export BIFROST_DESKTOP_NO_SYSTEM_PROXY=1
  export BIFROST_DESKTOP_SKIP_CERT_PREFLIGHT=1
  export BIFROST_DATA_DIR="$(mktemp -d)"
  ```

## 测试用例列表

### TC-MDT-01 macOS 标题栏与 UI 融合

操作步骤：

1. 启动 macOS Bifrost Desktop。
2. 打开 Traffic / Network 首屏。
3. 截图观察顶部区域。

预期结果：

- 不再出现截图中独立的灰色系统标题栏。
- 原生红黄绿窗口控制仍在左上角可见。
- 左侧菜单顶部预留空间，菜单项不压住窗口控制。
- 顶部区域背景与 Bifrost UI 视觉连续，不出现突兀色块。
- Network 页面内容顶部有一致的 35px 拖拽保留区，页面按钮和筛选控件不进入该区域。

### TC-MDT-02 左侧菜单区域拖拽移动窗口

操作步骤：

1. 记录窗口当前位置。
2. 在左侧菜单顶部预留区、侧栏导航项、OpenAPI、主题切换按钮任一位置按住鼠标左键拖动。
3. 在窗口顶部 35px 空白保留区按住鼠标左键拖动。
4. 松开鼠标后再次记录窗口位置。
5. 点击 `Network` / `Rules` / `Settings` 等导航项。

预期结果：

- 拖拽后窗口位置发生变化，窗口移动由 Tauri `data-tauri-drag-region` 原生机制处理，跟随鼠标，不出现倍速或显示器分辨率比例偏差。
- 导航项点击仍正常跳转，不被拖拽逻辑吞掉。
- OpenAPI 和主题切换按钮仍正常响应点击。

### TC-MDT-03 拖拽期间不触发文本选择

操作步骤：

1. 在左侧菜单文本、顶部 35px 空白保留区按住鼠标。
2. 横向或纵向移动鼠标触发窗口拖拽。
3. 观察页面是否出现文字选中高亮。
4. 点击紧邻顶部保留区下方的页面按钮或筛选控件。
5. 松开鼠标后，在正常输入框或可编辑区域中执行文本选择。

预期结果：

- 拖拽期间页面不出现文本选中高亮，也不触发浏览器原生拖拽文本状态；拖拽区域依赖 `user-select: none` / `-webkit-user-select: none`。
- 35px 保留区下方的页面控件可以正常点击，不被透明拖拽层遮挡。
- 松开鼠标后文本选择能力恢复；输入框和编辑器仍可正常选中、编辑文本。

### TC-MDT-04 暗色和亮色主题截图

操作步骤：

1. 切到暗色主题并截图。
2. 点击左侧主题切换按钮切到亮色主题并截图。
3. 对比两个截图的顶部和侧栏区域。

预期结果：

- 暗色主题中顶部 overlay、左侧侧栏和内容区边界自然融合。
- 亮色主题中顶部 overlay 不发灰、不出现系统标题栏色块。
- 文字、图标、状态条没有明显遮挡或重叠。

### TC-MDT-05 每个页面顶部留白一致

操作步骤：

1. 依次切换 `Network`、`Replay`、`Rules`、`Values`、`Scripts`、`AI`、`DevTools`、`Notify`、`Settings`。
2. 每个页面截图观察内容区顶部。

预期结果：

- 每个页面的内容区顶部均保留约 35px 拖拽空白区域。
- 除 Network 页面外，其他页面没有标题、工具栏、列表或卡片贴住窗口顶边。
- 暗色和亮色主题下留白一致，不出现顶部元素遮挡红黄绿窗口控制区。
- 页面可点击控件不进入顶部 35px 拖拽保留区。

### TC-MDT-06 CLI Web UI 不启用桌面窗口拖拽

操作步骤：

1. 在普通浏览器打开 `http://127.0.0.1:<port>/_bifrost/`。
2. 检查左侧侧栏 DOM 或页面表现。

预期结果：

- 页面不依赖 Tauri API。
- 不出现 `desktop-macos-window-control-spacer`。
- 点击左侧菜单只做导航，不触发桌面窗口拖拽。

## 清理步骤

```bash
bifrost stop || true
rm -rf "$BIFROST_DATA_DIR"
```

## 执行记录

| 日期 | 用例 | 执行命令 / 证据 | 结果 |
| --- | --- | --- | --- |
| 2026-07-05 | TC-MDT-01 / 02 / 03 | 部分执行：`pnpm run desktop:dev` 成功启动真实 macOS 窗口；`screencapture` 得到 `/tmp/bifrost-titlebar-shots/dev-network-dark.png`、`/tmp/bifrost-titlebar-shots/dev-network-light.png`；CoreGraphics 事件拖拽左侧菜单，窗口从 `{560,230}` 到 `{680,270}`；顶部 35px 重测窗口从 `{560,230}` 到 `{600,240}`，位移与鼠标一致。随后按官方文档回收为 `data-tauri-drag-region`，需在最终构建后重跑。 | 部分通过：真实旧实现拖拽跟手已验证；官方回收后的最终版本待重跑完整截图 |
| 2026-07-05 | TC-MDT-01 / 03 / 05 | 执行 `BIFROST_DESKTOP_NO_SYSTEM_PROXY=1 BIFROST_DESKTOP_SKIP_CERT_PREFLIGHT=1 pnpm run desktop:dev` 启动真实 macOS Tauri 窗口；截图 `/tmp/bifrost-titlebar-shots/no-sidebar-wash.png`。验证 `TitleBarStyle::Overlay` 下页面不再出现系统标题栏占位；`DesktopTransitionMask` 已退场；顶部内容从 35px 保留区下方开始；移除 `macSidebarWash` 后不再出现 112px 左侧竖向渐变污染。 | 通过：当前 Settings/Proxy 页面截图验证通过；逐页全量截图仍需补齐 |
| 2026-07-05 | TC-MDT-04 / 05 / 06 | 尝试逐页截图：直接 `screencapture` 脚本误截背后浏览器；静态 `dist-desktop` Playwright mock 出现白屏，未作为通过证据。 | 未完成：需要用最终 Tauri 窗口重新逐页截图 |
