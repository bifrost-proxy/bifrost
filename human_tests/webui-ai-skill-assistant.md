# WebUI AI Skill Assistant 真实场景测试

## 功能模块说明

验证 WebUI 全局 AI skill 引导入口的真实用户体验：右下角跳动入口默认可见，悬浮展示安装 skill 的浮窗，支持复制安装命令、查看经典应用场景、跳转仓库 `SKILL.md`，并且支持拖拽位置和点击隐藏，不影响用户继续使用主界面。

## 前置条件

- 当前工作目录为 Bifrost 仓库根目录。
- 不使用正式 `9900` 端口。
- UI 测试必须使用临时数据目录，并通过现有 Playwright global setup 启动最新编译的 Bifrost 后端；该流程会使用 `--no-system-proxy`。
- WebUI 已使用最新源码构建或由 Vite dev server 提供。

## 测试用例列表

### TC-AISA-01 默认入口与 hover 浮窗

操作步骤：

1. 打开 WebUI Traffic 页面：`http://127.0.0.1:<web_port>/_bifrost/traffic`。
2. 确认页面右下角存在 AI 跳动入口。
3. 将鼠标移动到 AI 入口上。
4. 查看浮窗内容。

预期结果：

- AI 入口默认可见，位于右下角且不遮挡主要流量表格。
- hover 后浮窗出现。
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

### TC-AISA-04 拖拽位置与点击隐藏

操作步骤：

1. 按住 AI 入口并拖动到页面内其他位置。
2. 松开鼠标，确认入口仍可见。
3. 再次单击 AI 入口。

预期结果：

- 拖拽后入口位置发生变化，且仍保持在视口内。
- 拖拽不会触发隐藏。
- 单击入口后入口消失，避免继续遮挡用户操作。

### TC-AISA-05 亮色与暗色主题

操作步骤：

1. 在亮色主题下 hover AI 入口，检查浮窗。
2. 点击左侧底部主题按钮切换暗色主题。
3. 再次 hover AI 入口，检查浮窗。

预期结果：

- 亮色主题下入口、浮窗、命令文本、按钮和链接均清晰可读。
- 暗色主题下入口、浮窗、命令文本、按钮和链接均清晰可读。
- 主题切换后 AI 入口仍可 hover、拖拽和点击隐藏。

### TC-AISA-06 回归：鼠标移向浮窗时不会立即消失

操作步骤：

1. 打开 WebUI Traffic 页面。
2. hover AI 入口，使浮窗出现。
3. 将鼠标从入口向浮窗方向移动，短暂经过入口与浮窗之间的空隙。
4. 继续移动到浮窗内部，并点击 `Copy` 按钮。

预期结果：

- 鼠标经过入口与浮窗之间的空隙时，浮窗不会立即消失。
- 鼠标进入浮窗后浮窗保持打开。
- `Copy` 按钮可点击，并出现复制成功提示。

## 清理步骤

- 关闭 Playwright 浏览器。
- 由 Playwright global teardown 清理 UI 测试临时进程。
- 如手动启动过服务，停止对应临时端口服务并删除临时数据目录。

## 执行记录

- 2026-05-08：执行 `pnpm --dir web test:ui ai-skill-assistant.spec.ts`。
- TC-AISA-01 通过：Playwright 打开 Traffic 页面后确认 AI 入口可见，hover 后浮窗出现，安装命令和三类经典应用场景均可见。
- TC-AISA-02 通过：点击 Copy 后页面出现 `Skill install command copied` 成功提示，浮窗保持可用。
- TC-AISA-03 通过：`SKILL.md` 链接目标确认为 `https://github.com/bifrost-proxy/bifrost/blob/main/SKILL.md`。
- TC-AISA-04 通过：拖拽后入口坐标变化超过 40px，入口仍可见；随后单击入口后入口隐藏。
- TC-AISA-05 通过：切换暗色主题后再次 hover，浮窗标题和 `SKILL.md` 链接可见，暗色主题下内容可读。
- TC-AISA-06 通过：补充 hover 延迟回归，Playwright 将鼠标从入口移动到入口与浮窗之间的空隙并等待 220ms，浮窗仍保持可见；随后进入浮窗并点击 Copy，成功出现复制提示。
