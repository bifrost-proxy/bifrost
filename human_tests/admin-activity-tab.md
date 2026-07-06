# Admin Activity Tab 真实场景测试

## 功能模块说明

验证 WebUI 新增 Activity 一级 tab：它必须作为第一个导航项和默认首页，展示代理活动概览、生效规则解析、临时端口规则详情、按应用统计的流量分布、系统代理状态，并提供可感知 hover 动效。亮色和暗色主题都必须兼容。

## 前置条件

1. 在仓库根目录启动测试用 WebUI 与后端：
   - `pnpm --dir web run test:ui -- activity-tab.spec.ts`
2. 如需人工浏览器复核，使用独立数据目录启动：
   - `CARGO_TARGET_DIR=./.bifrost-ui-target cargo build --bin bifrost`
   - `BIFROST_DATA_DIR=./.bifrost-e2e-activity ./.bifrost-ui-target/debug/bifrost start -p 8800 --unsafe-ssl --no-system-proxy`
   - 浏览器打开 `http://127.0.0.1:8800/_bifrost/`

## 测试用例列表

### TC-ACT-01 默认进入 Activity

操作步骤：

1. 打开 `http://127.0.0.1:8800/_bifrost/` 或 Playwright 中打开 `/_bifrost/`。
2. 观察 URL 和左侧导航第一个项目。

预期结果：

- URL 自动进入 `/activity`。
- 页面标题为 `Activity`。
- 左侧导航第一个 tab 为 `Activity`，并处于选中状态。

### TC-ACT-02 Activity metrics, system proxy state, and hover

操作步骤：

1. 进入 Activity 页面。
2. 查看顶部六个概览卡片。
3. 将鼠标依次悬停在任意概览卡片上。

预期结果：

- 页面展示 `Active Connections`、`Upload`、`Download`、`Requests`、`Rules`、`System Proxy` 六个卡片。
- 卡片数值来自当前 metrics / traffic / proxy 数据。
- 服务卡展示系统代理状态：系统代理开启时显示 `Enabled`，未开启时显示 `Disabled`，下方展示 `http://host:port`。
- hover 时卡片出现上浮和阴影增强，布局不抖动。

### TC-ACT-03 Active Rule Analysis

操作步骤：

1. 进入 Activity 页面。
2. 查看 `Active Rule Analysis` 区域。
3. 点击规则列表中的任一 active rule。
4. 选中 merged rules 代码块中的部分文本，点击复制按钮。
5. 清除选区后再次点击复制按钮。
6. 双击规则项。

预期结果：

- active rules 列表显示规则名和 entries 数。
- merged rules 代码块显示当前合并后的规则内容和行数。
- merged rules 代码区域撑满右侧可用高度，内容较多时在代码块内部滚动。
- 有文本选区时复制选区内容；没有选区时复制完整 merged rules。
- 单击规则项只改变选中视觉态。
- 双击规则项跳转到 Rules 页面并带上对应 rule 查询参数。

### TC-ACT-04 Traffic Distribution and hover

操作步骤：

1. 进入 Activity 页面并产生几条不同应用来源的代理流量。
2. 查看 `Traffic Distribution` 区域。
3. 将鼠标悬停在应用流量行上。

预期结果：

- 流量分布按应用请求数降序展示。
- 每行包含应用名、蓝色比例条和请求数。
- hover 时蓝色比例条出现亮度或轻微伸展变化，文字不被遮挡。

### TC-ACT-05 临时端口规则详情

操作步骤：

1. 创建一个禁用规则，例如 `activity-temp-real.test status://219 resBody://(activity-temp-rule)`。
2. 通过 Admin API 或 CLI 启动临时端口：
   - `POST /_bifrost/api/ports`，请求体包含 `port: 0`、`name: "Activity UI temporary port"`、`rule_refs: [{ "type": "local_rule", "name": "<规则名>" }]`
   - 或执行 `bifrost port bind --port 0 --name "Activity UI temporary port" --rule <规则名>`。
3. 打开 Activity 页面。
4. 将鼠标悬停在临时端口卡片上。

预期结果：

- 页面在规则解析下方显示 `Temporary Ports` 区块。
- 临时端口卡片显示 `host:port`、`running` 状态、端口名称、Bound Rules、Active Rules 和 Merged Rules。
- 端口级 Merged Rules 中包含绑定规则内容。
- hover 临时端口卡片时出现上浮和阴影增强。

### TC-ACT-06 暗色主题兼容

操作步骤：

1. 在 Activity 页面点击主题切换按钮进入暗色主题。
2. 查看顶部统计卡、规则解析面板、Merged Rules 代码块、复制按钮和流量分布。
3. 重复卡片 hover、流量行 hover 和复制按钮 hover。

预期结果：

- 页面背景、卡片、面板、文字、代码块在暗色主题下对比清晰。
- hover 动效仍可见。
- 复制按钮在暗色主题下可点击且状态可辨识。
- Temporary Ports 卡片在暗色主题下可读，hover 动效仍可见。

### TC-ACT-07 顶部指标长数值不溢出

操作步骤：

1. 进入 Activity 页面。
2. 观察顶部 `Upload`、`Download`、`Requests` 和 `System Proxy` 指标卡。
3. 使用较长的速率或计数字符串场景复核，例如 `271.2 KB/s`、`1,234,567` 或较长代理地址。

预期结果：

- 顶部指标主数值字号比旧版更克制，常见速率值完整显示在卡片内。
- 当主数值极长时，内容限制在卡片内，不横向冲出卡片或遮挡相邻卡片。
- 鼠标悬停主数值可通过浏览器 title 查看完整值。
- 卡片标题、状态圆点和 caption 不与主数值重叠。

## 清理步骤

1. 停止测试 Bifrost 进程。
2. 删除临时目录 `./.bifrost-e2e-activity`。
