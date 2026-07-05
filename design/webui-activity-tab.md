# WebUI Activity Tab 设计方案

## 背景

WebUI 现有首屏默认进入 Network，适合排查单条请求，但不适合作为代理运行中的总览页。本次新增 `Activity` 一级 tab，作为第一个导航项和默认入口，提供运行态概览、当前生效规则解析和按应用统计的流量分布。

## 用户目标验证清单

### 必须实现

- 左侧一级导航首位展示 `Activity`，根路径 `/` 默认跳转 `/activity`。
- `Activity` 页面展示六个概览卡片：`Active Connections`、`Upload`、`Download`、`Requests`、`Rules`、`System Proxy`。
- 页面展示当前 active rules、merged rules 内容和行数。
- Merged rules 支持一键复制；如果用户选中了代码块内文本，则复制选区，否则复制完整 merged rules。
- Merged rules 代码区域自动撑满规则解析面板的剩余高度，并在内容较长时内部滚动。
- 如果存在临时端口，页面在规则解析下方展示 `Temporary Ports` 区块，包含端口地址、状态、名称、绑定规则、active rules 和端口级 merged rules。
- 页面展示按应用统计的流量分布条形图。
- 卡片、规则项、流量条具备明确 hover 动效。
- 服务卡展示系统代理是否开启，而不是 CLI 代理进程状态。
- 亮色和暗色主题下都保持可读、对比清晰和 hover 可见。
- Activity 页面面向 UI 展示的文案统一使用英文，避免中英文混杂。

### 必须不破坏

- Network、Replay、Rules 等旧 tab 路由和 sidebar 滚动行为不变。
- Traffic 实时数据订阅仍只在 Activity、Network、Traffic Detail 这类需要流量数据的页面开启。
- 不新增后端接口，复用已有 `/metrics` / `/system/overview` / `/traffic/updates` / `/rules/active-summary` / `/proxy/cli` 数据。

### 必须真实验证

- 打开 `/_bifrost/` 后自动进入 `/_bifrost/activity`，首屏显示 `Activity`。
- Activity 是左侧导航第一个 tab，且处于 active 状态。
- Hover 概览卡片时 transform 上浮；hover 流量条时 fill 发生 transform/filter 变化。
- active rules 和 merged rules 与 mock/真实 API 返回一致。

## 技术方案

- 新增 `web/src/pages/Activity/index.tsx` 与 CSS module。
- 页面数据来源：
  - `useMetricsStore.current` / `overview`：速率、请求、连接、规则总数、服务端口。
  - `useTrafficStore.records` / `clientAppCounts`：本地流量窗口与应用分布。
  - `getActiveSummary()`：active rules 和 merged rules。
  - `useProxyStore.systemProxy`：系统代理开启状态和地址。
  - `getTemporaryPorts()` / `getTemporaryPortActiveSummary(port)`：临时端口绑定和端口级规则解析。
- `App.tsx` 将 index route 改为 `/activity`，并把 `/activity` 计入 `trafficEnabled`。
- `Layout/index.tsx` 将 Activity 放在 `menuItems` 第一位。

## 验证设计

- Playwright `activity-tab.spec.ts` 拦截 Admin API，断言默认路由、导航顺序、关键数值、规则内容、系统代理状态、复制按钮、临时端口区块、亮/暗主题和 hover 样式变化；另有真实 API 用例创建临时端口 listener 后验证 Activity 交互。
- human_tests `admin-activity-tab.md` 记录真实场景用例，覆盖默认入口、hover 动效、规则解析、复制、撑满高度、临时端口、流量分布和系统代理状态。
