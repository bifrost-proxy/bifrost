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

### TC-ACT-08 Merged Rules 智能高亮生效与覆盖规则

操作步骤：

1. 启用包含以下内容的规则组合：
   ```text
   https://nextoncall.bytedance.net/api/v1/oncall/ reqHeaders://{"x-tt-env":"ppe_old","x-use-ppe":"1"}
   https://nextoncall.bytedance.net/api/v1/oncall/ passthrough://
   https://nextoncall.bytedance.net/api/v1/oncall/ reqHeaders://{"x-tt-env":"ppe_new","x-use-ppe":"1"}
   https://nextoncall.bytedance.net/api/v1/oncall/ passthrough://
   ```
2. 打开 Activity 页面。
3. 在 `Active Rule Analysis` 的 `Merged Rules` 顶部观察 active rule sets 标签区。
4. 点击任意 active rule set 标签。
5. 返回 Activity，在 `Merged Rules` 代码区观察行级高亮。
6. 鼠标悬浮旧 `reqHeaders` 行、最终 `reqHeaders` 行和后一个 `passthrough://` 行。
7. 选中代码区中的部分文本并点击复制；再清除选区后点击复制。
8. 将旧 `reqHeaders` 行扩展为较长 JSON header 值，观察代码区是否仍在面板宽度内自动换行。

预期结果：

- Active rule sets 以小字号标签展示在 Merged Rules 顶部，从左到右、从上到下平铺，不再占用左侧大卡片列，也不显示额外蓝色圆点。
- 单击 active rule set 标签会跳转到对应 Rules 详情页。
- Temporary Ports 中每个临时端口都是独立全宽卡片，多个端口从上到下排列，不在一行显示多个端口卡片。
- 临时端口卡片内的 Merged Rules 随规则内容自然撑高，不出现内部小滚动框；长规则仍在卡片宽度内换行。
- 旧 `reqHeaders` 行被标记为 covered，hover 解释同 matcher 下请求头被后续规则替换。
- 最终 `reqHeaders` 行被标记为 effective，hover 解释它是最终请求头写入。
- 第一条 `passthrough://` 行为 effective，后一个同 matcher `passthrough://` 行为 covered。
- hover 文案包含覆盖来源行号，不需要用户手动推理规则优先级。
- 每条规则左侧展示可见行号，hover 文案中的 `line N` 可以直接对照定位。
- 长 URL / 长 JSON header 在代码区内自动换行，不出现横向撑出 Activity 面板或遮挡右侧内容。
- 复制选区/全文时只复制原始 merged rules 文本，不复制状态点、覆盖解释或其他 UI 文案。
- 暗色主题下 active / covered / partial 的背景、边线和文本都清晰可读。

执行记录（2026-07-07，本地纯前端 smoke）：

- ✅ PASS：执行 `WEB_PORT=3108 PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH='/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' pnpm --dir web exec playwright test tests/ui/activity-tab.spec.ts --config=playwright.frontend.config.ts -g "Activity tab is first"`。测试使用 Vite 纯前端服务和 mock Admin API；验证 Activity Merged Rules 顶部 active rule set 标签、单击标签跳转 Rules 详情、nextoncall 重复规则样例、Temporary Ports 两个端口纵向独立卡片和临时端口 Merged Rules 无内部滚动，断言 3 行 active / 2 行 covered，hover 旧 `reqHeaders` 行出现 `Request headers are replaced by line`，断言可见行号和长 header 不产生横向溢出，并复测选区复制、全文复制、卡片 hover、流量行 hover 和暗色主题基础可见性。

## 清理步骤

1. 停止测试 Bifrost 进程。
2. 删除临时目录 `./.bifrost-e2e-activity`。
