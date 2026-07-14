# WebUI Layout Navigation 设计方案

## 背景

Admin WebUI 采用固定宽度的左侧 sidebar + 顶部/底部工具栏 + 三栏主体。以下几个体验坑必须一起治理：

1. **小屏 sidebar 溢出**：一级导航项 + 底部主题切换按钮同列时，窗口较矮会让最下方的 tab（Settings / Notify 等）被状态栏挤压或不可见；导航区没有独立滚动。
2. **状态栏 Sync 入口路径过长**：底部 `statusbar-sync` 只展示状态 + tooltip；用户要处理 Sync 登录 / 远端地址 / 手动同步 / 断线重连时必须多点几次到 Settings 内部切换到 Sync tab。
3. **版本弹窗 Copy 假成功**：`copyToClipboard()` 存在 fallback，`document.execCommand('copy')` 在嵌入式浏览器可能返回 `true` 但未真正写入剪贴板；版本弹窗又没有检查返回值，会出现 "提示复制成功但实际剪贴板为空" 的用户投诉。
4. **Safari/Tauri 窄栏裁切**：导航滚动容器使用 `scrollbar-gutter: stable` 时，WebKit 会在 50px sidebar 内永久预留滚动条宽度，固定宽度导航项居中后被 `overflow-x: hidden` 横向裁掉；Tauri macOS 系统 WebView 会复现同一问题。

本设计把三件事合并处理，因为它们都由 Layout / StatusBar / 剪贴板工具在同一批 UX polish 中改动。

## 用户目标验证清单

### 必须实现

- **小屏 sidebar**：导航容器有独立滚动能力，底部主题切换按钮固定在 sidebar 底部；小窗口下能滚到所有 tab。
- **WebKit sidebar**：Safari 与 Tauri macOS 中所有一级导航图标和英文标签完整显示，不因滚动条 gutter 缺字。
- **状态栏 Sync 快捷入口**：点击底部 `statusbar-sync` 直接跳到 `Settings → Sync tab`；支持键盘 Enter/Space；tooltip、状态色、`data-sync-state` / `data-sync-action` 测试属性保留。
- **Copy 命令真实写入**：`copyToClipboard()` 只有在真的写入剪贴板时才返回 `true`；版本弹窗根据返回值给出成功/失败提示。
- 三处改动都通过 Playwright 与 human_tests 真实浏览器验证。

### 必须不破坏

- sidebar 外宽继续保持 50px；大屏视觉与 hover 效果不变。
- 底部状态栏其余节点（DevMode、Cert、Data Dir、Sync 之外的状态）交互不变。
- 亮色 / 暗色主题的字色、hover 色继续用 Ant Design token (`colorFillSecondary` 等)。
- Ant Layout / SplitPane / FilterPanel 折叠展开不受影响。
- 现有 `web/tests/ui/*.spec.ts` 未修改的用例应继续通过。

### 必须真实验证

- 手动或 Playwright 缩小 viewport 到 700px 高，滚动到 sidebar 底部仍可点 Settings / Notify。
- Playwright WebKit/Chromium 与真实 Safari/Chrome/Tauri 均验证亮色、暗色和小窗口下标签横向边界完整。
- 手动或 Playwright 点击 `statusbar-sync` 后 URL 变为 `/_bifrost/settings?tab=sync` 且 Sync tab `aria-selected=true`；键盘 Enter 也生效。
- 打开版本弹窗点 Copy 后 `navigator.clipboard.readText()` 返回真实命令；执行环境不支持剪贴板 API 时提示失败。

## 产品语义

### Sidebar 滚动模型

- 整个 sidebar 固定宽度、整高。
- 内部由 "导航滚动容器" + "固定底部主题按钮" 组成：
  - 导航容器：`flex: 1`、`minHeight: 0`、`overflowY: auto`、`overflowX: hidden`。
  - 导航容器不使用稳定 gutter：`scrollbarGutter: auto`；窄栏通过 `scrollbarWidth: none` 与局部 `::-webkit-scrollbar { width: 0; height: 0; }` 隐藏原生滚动条轨道，但保留滚动能力。
  - 每个 tab item `minHeight: 64`、`flexShrink: 0`，保证识别度。
  - `data-testid="app-sidebar-nav-scroll"` 用作 Playwright 断言容器可滚。
- 底部主题按钮不放入滚动容器；用户永远能一键切主题。

### 状态栏 Sync 快捷入口

- `statusbar-sync` 保留原有：状态文本 (`Sync: Off / Syncing / Local / Sign in / Synced / Connected`)、详细 Tooltip、`data-sync-state` / `data-sync-action`。
- 增加点击 / 键盘行为：`navigate("/settings?tab=sync")`。
- `role="button"`, `tabIndex={0}`, `onKeyDown` 处理 Enter / Space。
- Hover 背景 `token.colorFillSecondary`；亮暗主题自适应。

### Copy 命令真实写入

- 三层 fallback：
  1. Desktop 壳 Tauri 原生 `write_clipboard`（若存在）。
  2. Web 现代 API `navigator.clipboard.writeText(text)`。
  3. Legacy fallback：真实 `copy` 事件写入 `event.clipboardData.setData('text/plain', text)`；只有 `execCommand('copy')` 返回成功且 copy 事件实际写入数据才返回 `true`。
- 版本弹窗 `handleCopyCommand`：`if (await copyToClipboard(text)) { message.success(...) } else { message.error(...) }`。

## 技术细节

### `web/src/components/Layout/index.tsx`

关键代码位置：

- Sidebar container：`styles.sidebar`，宽度引用 `APP_SIDEBAR_WIDTH`。
- 导航滚动容器：`className="app-sidebar-nav-scroll"` + `data-testid="app-sidebar-nav-scroll"`，基础几何契约来自 `sidebarLayout.ts`。
- 每个导航项：`data-testid="app-sidebar-nav-item"`，图标/标签分别使用 `app-sidebar-nav-icon` / `app-sidebar-nav-label`。
- 底部主题按钮 / OpenAPI：`data-testid="app-sidebar-openapi"`；不放入 menuScroll。

样式片段（伪代码）：

```tsx
const styles = {
  sidebar: {
    width: APP_SIDEBAR_WIDTH, // 50px
    height: '100%',
    display: 'flex',
    flexDirection: 'column',
    background: token.colorBgContainer,
  },
  menuScroll: {
    flex: 1,
    minHeight: 0,
    overflowY: 'auto',
    overflowX: 'hidden',
    scrollbarGutter: 'auto',
    scrollbarWidth: 'none',
  },
  navItem: {
    minHeight: 64,
    flexShrink: 0,
  },
  themeButton: {
    padding: 12,
    // fixed at bottom, outside menuScroll
  },
};
```

### `web/src/components/StatusBar/index.tsx`

- `data-testid="statusbar-sync"`（第 563 行）。
- Hover 背景 `token.colorFillSecondary`（第 477、533、569、643 行等）。
- 点击行为：

```tsx
const navigate = useNavigate();

<div
  data-testid="statusbar-sync"
  role="button"
  tabIndex={0}
  onClick={() => navigate('/settings?tab=sync')}
  onKeyDown={(e) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      navigate('/settings?tab=sync');
    }
  }}
  onMouseEnter={(e) => {
    e.currentTarget.style.backgroundColor = token.colorFillSecondary;
  }}
  onMouseLeave={(e) => {
    e.currentTarget.style.backgroundColor = 'transparent';
  }}
>
  {/* existing tooltip + state text */}
</div>
```

Settings 页面根据 `?tab=sync` 联动，Tab `aria-selected=true`。

### `web/src/utils/clipboard.ts`

```ts
export async function copyToClipboard(text: string): Promise<boolean> {
  // 1) Tauri native (Desktop shell)
  if (window.__TAURI__?.invoke) {
    try {
      await window.__TAURI__.invoke('write_clipboard', { text });
      return true;
    } catch { /* fall through */ }
  }

  // 2) Modern browser API
  if (navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch { /* fall through */ }
  }

  // 3) Legacy fallback: real copy event
  let wrote = false;
  const listener = (e: ClipboardEvent) => {
    e.preventDefault();
    e.clipboardData?.setData('text/plain', text);
    wrote = true;
  };
  document.addEventListener('copy', listener);
  const ok = document.execCommand('copy');
  document.removeEventListener('copy', listener);
  return ok && wrote;
}
```

- 只有 `ok && wrote` 才返回 `true`；避免 execCommand 假阳性。
- 单元测试 `web/src/utils/clipboard.test.ts` 覆盖：async API 成功、async API 失败降级、legacy 真实写入、legacy 假成功、Tauri 环境。

### 版本弹窗 `handleCopyCommand`

- 位置：`web/src/components/VersionUpgradeModal/*` 或 Settings 内的版本区块（沿用现有位置）。
- 修改：

```tsx
const handleCopyCommand = async () => {
  const ok = await copyToClipboard(upgradeCommand);
  if (ok) {
    message.success(t('command_copied'));
  } else {
    message.error(t('command_copy_failed'));
  }
};
```

## CLI + Web + Admin API 边界

### CLI

- 不涉及 CLI 变更。

### Web UI

- 三处 UX 改动均在前端；对后端接口无影响。
- data-testid 保留列表：
  - `app-sidebar-nav-scroll`
  - `app-sidebar-nav-item`
  - `app-sidebar-nav-icon`
  - `app-sidebar-nav-label`
  - `app-sidebar-openapi`
  - `statusbar-sync`（携带 `data-sync-state` / `data-sync-action`）

### Admin API

- 不新增接口。Sync tab 点击后，Settings 页仍走已有 `/api/sync/*` 接口。

## Sync 边界

- 本文档不改变 Sync 后端行为；仅为 Sync 增加更短的 UI 入口。
- Sync 数据仍由 `crates/bifrost-sync` 拥有，前端只通过快捷入口进入既有配置页。

## Phase 拆分

### Phase 1：Sidebar 滚动

- 调整 `Layout/index.tsx` 的 sidebar 结构。
- 更新 Playwright 与 human_tests 覆盖小窗口场景。

### Phase 2：状态栏 Sync 快捷入口

- 修改 `StatusBar/index.tsx`；Settings 联动 `?tab=sync`。
- 新增/更新 `web/tests/ui/admin-settings.spec.ts` 用例。
- human_tests 新增 `TC-WLN-17`。

### Phase 3：Copy 命令真实写入

- 重写 `web/src/utils/clipboard.ts` 与 `clipboard.test.ts`。
- 版本弹窗 `handleCopyCommand` 接入返回值。
- `web/tests/ui/admin-settings.spec.ts` 增加版本弹窗 Copy 真实读剪贴板断言。
- human_tests 补 `TC-WLN-16`。

### Phase 4：Cleanup

- 检查其它模块调用 `copyToClipboard()` 是否都检查了返回值；未检查的按 Rules / Replay 现有模式补齐（下 wave 处理，不阻塞本 doc）。

## 测试方案

### 单元测试

- `web/src/utils/clipboard.test.ts`：
  - 同步 copy 事件写入成功。
  - `execCommand` 假成功（`ok=true, wrote=false`）→ 返回 false。
  - async Clipboard API 成功。
  - Tauri 环境优先路径。

### Playwright

- `web/tests/ui/admin-settings.spec.ts`：
  - 打开 Traffic 页面 → 点击 `data-testid=statusbar-sync` → 断言 URL 为 `/_bifrost/settings?tab=sync`、Sync tab `aria-selected=true`。
  - 版本弹窗点 Copy → `page.evaluate(() => navigator.clipboard.readText())` 返回 `bifrost upgrade`。
- 需要新增：
  - `web/tests/ui/admin-layout.spec.ts`（若不存在），覆盖 700x800 viewport 下 sidebar 滚到底部仍可点 Settings。

### human_tests

`human_tests/webui-layout-navigation.md`：

- TC-WLN-01..14：保持既有布局 / 主题 / 面板用例。
- TC-WLN-15：小窗口下左侧导航栏支持滚动（现有）。
- TC-WLN-16：版本更新弹窗 Upgrade Command Copy 真实写入剪贴板（现有，本 doc 使之落地）。
- TC-WLN-17：底部 Sync 状态栏跳转到 Settings Sync（现有，本 doc 使之落地，覆盖鼠标点击 / 键盘 Enter / Tooltip 保留 / 亮暗主题可读性）。

### 后端 / 其它

- 无 backend 改动；无需 cargo 相关测试；`rust-project-validate` 中的 fmt / clippy / test 兜底即可。

## Review / Fix / Test 闭环

### Round 1

- 复读三段用户目标：sidebar 滚动、Sync 快捷入口、Copy 真实写入。
- 复查 diff：`Layout/index.tsx`、`StatusBar/index.tsx`、`utils/clipboard.ts` (+ 测试)、版本弹窗组件、`web/tests/ui/admin-settings.spec.ts`、`human_tests/webui-layout-navigation.md`。
- 复测：
  - `pnpm --filter web lint`
  - `pnpm --filter web test`（含 clipboard.test.ts）
  - `pnpm --filter web build`
  - Playwright targeted：`pnpm --filter web test:e2e --grep "statusbar-sync"` 与版本弹窗 Copy。

### Round 2

- 复查 Round 1 修复；确认 `data-testid` 未改名（下游依赖）。
- 复审 SplitPane / FilterPanel / 三栏面板未被误伤。
- 全量跑 `pnpm --filter web test:e2e`；观察是否引起 flaky。

## 校验要求

- `pnpm --filter web lint`
- `pnpm --filter web test`
- `pnpm --filter web build`
- Playwright：`web/tests/ui/admin-settings.spec.ts`、`web/tests/ui/admin-layout.spec.ts`（若存在）。
- 收尾按 `rust-project-validate` 兜底 `cargo fmt`、`cargo clippy`、`cargo test --workspace --all-features`；无 Rust 改动时也建议跑一次确认不引入回归。

## 文档更新要求

- 更新 `human_tests/webui-layout-navigation.md` TC-WLN-15/16/17 用例内容与真实执行记录。
- 更新 `human_tests/readme.md` 用例数量。
- 不涉及 README / CLI help / site 文档改动。

## 风险与决策点

- **滚动条视觉**：某些浏览器 macOS 隐藏滚动条会让滚动能力不易被发现；必要时增加 fade indicator，但会引入额外样式债务，本 doc 不做。
- **`?tab=sync` 查询串耦合**：Settings 页面必须持续支持 `?tab=<name>`；改名前需要在 Layout / StatusBar 中一起更新。
- **Copy fallback 兼容性**：某些 iframe 环境仍会因剪贴板 API 权限被拒；`copyToClipboard()` 会返回 false，用户会看到失败提示，这是预期行为。
- **主题 token 稳定性**：`colorFillSecondary` 是 Ant Design 5 的稳定 token；未来升级 Ant 时需要重新验证 hover 观感。
- **Playwright 依赖 `clipboard-read` 权限**：`admin-settings.spec.ts` 需要在 Playwright config 中 `permissions: ['clipboard-read', 'clipboard-write']`。若配置缺失会导致 `readText()` 抛权限错误，需在 Round 1 检查。
- **回归风险**：小 viewport Playwright 用例可能在 CI 上不稳定；应固定 viewport 到 `900x700` 并使用 `waitForSelector('[data-testid=app-sidebar-nav-item]')` 等确定性等待。
