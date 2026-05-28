# WebUI AI Skill Assistant 真实场景测试

## 功能模块说明

验证 WebUI 全局 AI skill 引导入口的真实用户体验：入口位于主状态栏底部版本号旁边，点击 `Skill` 后在该位置上方展示安装 skill 的浮窗，支持复制安装命令、查看经典应用场景、跳转仓库 `SKILL.md`，不再使用会遮挡主内容的悬浮气泡。

## 前置条件

- 当前工作目录为 Bifrost 仓库根目录。
- 不使用正式 `9900` 端口。
- UI 测试必须使用临时数据目录，并通过现有 Playwright global setup 启动最新编译的 Bifrost 后端；该流程会使用 `--no-system-proxy`。
- WebUI 已使用最新源码构建或由 Vite dev server 提供。

## 测试用例列表

### TC-AISA-01 状态栏入口与点击浮窗

操作步骤：

1. 打开 WebUI Traffic 页面：`http://127.0.0.1:<web_port>/_bifrost/traffic`。
2. 确认页面右下角不存在旧 AI 悬浮气泡。
3. 在底部状态栏版本号旁边找到 `Skill` 入口。
4. 点击 `Skill` 入口。
4. 查看浮窗内容。

预期结果：

- 旧悬浮气泡不再渲染，不遮挡主要流量表格或对话内容。
- `Skill` 入口位于底部状态栏版本号旁边。
- 点击后浮窗在 `Skill` 入口上方出现。
- 浮窗展示安装命令 `bifrost install-skill -y`。
- 浮窗展示三类经典应用场景：通过 AI 操作规则增删改查、流量搜索和问题排查、多端口独立规则。

### TC-AISA-02 复制安装命令

操作步骤：

1. 在浮窗打开状态下点击 `Copy` 按钮。
2. 观察页面提示。

预期结果：

- 页面出现复制成功提示。
- 被复制的命令为 `bifrost install-skill -y`。
- 浮窗不会因为点击复制按钮而关闭或隐藏入口。

### TC-AISA-03 仓库 SKILL.md 链接

操作步骤：

1. 在浮窗打开状态下检查 `SKILL.md` 链接。
2. 点击或检查链接目标。

预期结果：

- 链接目标为 `https://github.com/bifrost-proxy/bifrost/blob/main/SKILL.md`。
- 点击后在新页面打开仓库 `SKILL.md`，不破坏当前 WebUI 页面状态。

### TC-AISA-04 点击关闭且不保留悬浮交互

操作步骤：

1. 确认页面不存在 `ai-skill-assistant-launcher` 悬浮入口。
2. 点击状态栏 `Skill` 入口打开浮窗。
3. 再次点击状态栏 `Skill` 入口。

预期结果：

- 不再支持或展示拖拽气泡，浮窗文案不包含“拖拽气泡”。
- 第二次点击后浮窗关闭。
- 状态栏 `Skill` 入口仍保留，后续可再次打开。

### TC-AISA-05 亮色与暗色主题

操作步骤：

1. 在亮色主题下点击状态栏 `Skill` 入口，检查浮窗。
2. 点击左侧底部主题按钮切换暗色主题。
3. 再次点击状态栏 `Skill` 入口，检查浮窗。

预期结果：

- 亮色主题下状态栏入口、浮窗、命令文本、按钮和链接均清晰可读。
- 暗色主题下状态栏入口、浮窗、命令文本、按钮和链接均清晰可读。
- 主题切换后状态栏 `Skill` 入口仍可点击打开浮窗。

### TC-AISA-06 回归：浮窗锚定在状态栏入口上方

操作步骤：

1. 打开 WebUI Traffic 页面。
2. 点击底部状态栏 `Skill` 入口，使浮窗出现。
3. 检查浮窗和 `Skill` 入口的位置。
4. 点击 `Copy` 按钮。

预期结果：

- 浮窗底部位于 `Skill` 入口上方，不在页面中间或旧悬浮气泡位置出现。
- 浮窗保持打开直到用户再次点击入口或点击外部区域。
- `Copy` 按钮可点击，并出现复制成功提示。

## 清理步骤

- 关闭 Playwright 浏览器。
- 由 Playwright global teardown 清理 UI 测试临时进程。
- 如手动启动过服务，停止对应临时端口服务并删除临时数据目录。

## 执行记录

- 2026-05-08：执行 `pnpm --dir web test:ui ai-skill-assistant.spec.ts`，验证旧悬浮气泡版本。
- 2026-05-28：更新用例后立即执行 `pnpm --dir web exec playwright test tests/ui/ai-skill-assistant.spec.ts --reporter=line`。TC-AISA-01 通过：Playwright 打开 Traffic 页面后确认旧 `ai-skill-assistant-launcher` 不存在，状态栏版本号旁边展示 `Skill`，点击后浮窗在入口上方出现，安装命令和三类经典应用场景均可见。TC-AISA-02 通过：点击 Copy 后页面出现 `Skill install command copied` 成功提示。TC-AISA-03 通过：`SKILL.md` 链接目标确认为 `https://github.com/bifrost-proxy/bifrost/blob/main/SKILL.md`。TC-AISA-04 通过：浮窗文案不包含“拖拽气泡”，再次点击 `Skill` 后浮窗关闭且状态栏入口保留。TC-AISA-05 通过：切换暗色主题后再次点击，浮窗标题和 `SKILL.md` 链接可见，暗色主题下内容可读。TC-AISA-06 通过：量测浮窗位于 `Skill` 入口上方，Copy 按钮可点击并成功出现复制提示。补充真实 3000 dev 页面交叉验证：执行 `WEB_PORT=3000 BACKEND_PORT=8800 pnpm --dir web dev --host 127.0.0.1`，打开 `http://127.0.0.1:3000/_bifrost/traffic`，旧 launcher 数量为 `0`；`Skill` 入口 bbox 为 `x=1742.078125,y=981.5,width=45.921875,height=18`，版本号 bbox 为 `x=1675.046875,y=980.65625,width=54.03125,height=19.703125`，浮层 bbox 为 `y=714.8124389648438,bottom=978.18359375`，确认入口位于版本号右侧且浮层在入口上方。
