# Bifrost DevTools Remote Control 真实场景测试

## 功能模块说明

验证 Bifrost 在用户显式配置 `devtools://` 规则后，可以对经过代理的页面建立 `page_bridge` 调试通道，并在 WebUI DevTools tab 中使用 Bifrost 自有面板完成 Elements、Network、Storage、Console 调试。Elements 必须可操作，Network / Storage / Console 必须覆盖运行中增量同步，Storage 必须支持受控修改，Console 必须支持输入脚本执行。

本模块已废弃 Chrome DevTools frontend 集成路线：WebUI 不应再出现安装官方 frontend、打开系统 Chrome DevTools、复制 `devtools://devtools/bundled/inspector.html` 调试地址、内嵌官方 iframe 等入口；后端也不应再提供这些安装或托管接口。

## 前置条件

- 在仓库根目录执行。
- 任意 shell 命令前先执行 `source ~/.zshrc`。
- 测试服务必须使用临时 `BIFROST_DATA_DIR`。
- 启动 Bifrost 时必须使用 `--no-system-proxy`。
- 测试端口不得使用 `9900`。
- 需要 Playwright Chromium 依赖。

推荐自动执行命令：

```bash
source ~/.zshrc && e2e-tests/tests/test_devtools_page_bridge_api.sh
```

## 测试用例列表

### TC-CDP-01：显式 devtools:// 规则才发现页面

操作步骤：

1. 启动临时 HTTP fixture 站点。
2. 启动临时 Bifrost 代理。
3. 配置 `http://devtools-fixture.test:<port>/* devtools://mode=read,inject=bridge host://127.0.0.1:<port>`。
4. 使用真实浏览器经 Bifrost 代理访问 `basic.html?case=av-cdp-01`。
5. 调用 `GET /_bifrost/api/devtools/pages?online=true`。

预期结果：

- 页面列表包含目标页。
- 页面 `adapter=page_bridge`。
- 页面 `fidelity=fallback`。
- 页面 `state=discoverable`。
- 页面 title 为 `Bifrost DevTools Basic`。

### TC-CDP-02：bridge 注入状态可被页面观测

操作步骤：

1. 使用已命中规则的目标页面。
2. 在目标页执行 `window.__BIFROST_DEVTOOLS_BRIDGE__?.state`。

预期结果：

- 返回 `connected`。
- 页面伪造 postMessage 或猜测 token 不会改变 Admin 侧页面状态。

### TC-CDP-03：后端 snapshot 返回真实页面数据

操作步骤：

1. 打开目标页对应 DevTools session。
2. 调用 `GET /_bifrost/api/devtools/sessions/:session_id/snapshot`。

预期结果：

- `dom_tree` 或 `dom_snapshot` 包含 `debug-fixture`。
- `console` 包含 `bifrost-devtools-basic-ready`。
- `network` 包含 `/devtools/api/ping`。
- `storage.local_storage` 包含 `bifrost-storage-key=storage-ready`。
- `storage.session_storage` 包含 `bifrost-session-key=session-ready`。
- `storage.cookies` 包含 `bifrost-cookie-key=cookie-ready`。

### TC-CDP-04：WebUI DevTools 页面列表可选择在线页面

操作步骤：

1. 打开 `http://127.0.0.1:<proxy_port>/_bifrost/`。
2. 点击侧边栏 `DevTools`。
3. 在搜索框输入 `av-cdp-allowlist`。
4. 点击 `Bifrost DevTools Basic` 卡片。

预期结果：

- 页面列表只展示匹配的在线目标页。
- 进入详情页后展示目标页标题、URL、adapter、mode、rule、traffic。
- 详情页存在 `Elements`、`Network`、`Storage`、`Console` 四个 tab。

### TC-CDP-05：WebUI Elements 面板展示 DOM、节点操作与手动刷新

操作步骤：

1. 在 WebUI DevTools 详情页打开 `Elements` tab。
2. 点击包含 `debug-fixture` 的 DOM 节点。
3. 在目标页追加 `#debug-fixture-manual-refresh` 节点。
4. 点击 WebUI 详情页 refresh 按钮。

预期结果：

- 面板展示 Chrome DevTools 风格的左右分栏：左侧为可展开/折叠 DOM tree，右侧为选中节点详情。
- DOM tree 中标签名、属性名、属性值有区分渲染，闭合标签和空标签展示符合 HTML tree 习惯。
- 内容包含 `debug-fixture`。
- 点击 DOM 节点后，目标页出现 `#__bifrost_devtools_highlight__` overlay。
- 点击 DOM 节点后，右侧 selected node inspector 展示 `debug-fixture` 和 `data-case`。
- 手动刷新后，Elements 面板包含 `debug-fixture-manual-refresh`。
- 不出现官方 Chrome DevTools iframe。

### TC-CDP-06：WebUI Network 面板展示完整网络事件与新增记录

操作步骤：

1. 在 WebUI DevTools 详情页打开 `Network` tab。
2. 目标页运行时发起 `fetch('/devtools/api/extra?case=webui-network-complete')`。
3. 点击 WebUI 详情页 refresh 按钮。

预期结果：

- 面板展示 method、status、type、URL。
- 内容包含 `/devtools/api/ping`。
- 刷新后内容包含 `webui-network-complete`。

### TC-CDP-07：WebUI Storage 面板展示、同步并修改 cookies 与 Web Storage

操作步骤：

1. 在 WebUI DevTools 详情页打开 `Storage` tab。
2. 目标页运行时设置 `bifrost-cookie-live`、`bifrost-storage-live`、`bifrost-session-live`。
3. 点击 WebUI 详情页 refresh 按钮。
4. 点击 `bifrost-storage-live` 行的编辑按钮，确认编辑器自动填入 Local Storage、key、value。
5. 在 WebUI Storage 编辑器选择 Cookie，写入 `bifrost-cookie-edit=cookie-edited`。
6. 在 WebUI Storage 编辑器选择 Local Storage，写入 `bifrost-storage-edit=storage-edited`。
7. 在 WebUI Storage 编辑器选择 Session Storage，写入 `bifrost-session-edit=session-edited`。
8. 在目标页读取 cookie/localStorage/sessionStorage。
9. 点击 WebUI 详情页 refresh 按钮。

预期结果：

- Cookies 区域包含 `bifrost-cookie-key`。
- Local Storage 区域包含 `bifrost-storage-key`。
- Session Storage 区域包含 `bifrost-session-key`。
- 刷新后 Cookies 区域包含 `bifrost-cookie-live`。
- 刷新后 Local Storage 区域包含 `bifrost-storage-live`。
- 刷新后 Session Storage 区域包含 `bifrost-session-live`。
- 行内编辑入口会把 `bifrost-storage-live=storage-live` 自动带入编辑器。
- 目标页真实读到 `bifrost-cookie-edit=cookie-edited`。
- 目标页真实读到 `localStorage.getItem('bifrost-storage-edit') === 'storage-edited'`。
- 目标页真实读到 `sessionStorage.getItem('bifrost-session-edit') === 'session-edited'`。
- 再次刷新后 WebUI Storage 面板展示三个编辑后的值。

### TC-CDP-08：WebUI Console 面板展示完整日志并执行输入脚本

操作步骤：

1. 使用 `mode=control,evaluate_allowlist=["^document\\.title$"]` 规则访问目标页。
2. 在 WebUI DevTools 详情页打开 `Console` tab。
3. 目标页运行时输出 `console.info('bifrost-console-info-live')` 和 `console.error('bifrost-console-error-live')`。
4. 点击 WebUI 详情页 refresh 按钮。
5. 在输入框填入 `document.title`。
6. 点击 `Run`。

预期结果：

- Console 日志包含 `bifrost-devtools-basic-ready`。
- Console 日志包含 `bifrost-devtools-warning-ready`。
- 刷新后 Console 日志包含 `bifrost-console-info-live`。
- 刷新后 Console 日志包含 `bifrost-console-error-live`。
- 执行结果显示 `Bifrost DevTools Basic`。
- 执行行为进入 evaluate audit。

### TC-CDP-09：read mode 禁止 Console 执行

操作步骤：

1. 使用 `mode=read` 规则访问目标页。
2. 打开 WebUI `Console` tab。

预期结果：

- 页面提示 evaluate 需要 `mode=control`。
- `Run` 按钮不可用。
- 通过 API 直接执行 `runtime.evaluate` 返回 `requires_control`。

### TC-CDP-10：多页面切换不复用旧页面状态

操作步骤：

1. 打开 primary 页面 `basic.html?case=av-cdp-allowlist`。
2. 打开 secondary 页面 `secondary.html?case=av-cdp-secondary`。
3. 在 WebUI DevTools 中先选择 primary，再返回列表选择 secondary。

预期结果：

- primary 和 secondary 有不同 page id。
- secondary 详情页 Elements 面板包含 `debug-fixture-secondary`。
- secondary 详情页不复用 primary 的 console/storage/DOM。

### TC-CDP-11：移动 Safari UA 降级路径可发现

操作步骤：

1. 使用 Playwright 创建带移动 Safari UA 的浏览器上下文。
2. 经 Bifrost 代理访问命中 `devtools://` 规则的页面。
3. 调用 pages API。

预期结果：

- 目标页出现在在线页面列表中。
- `adapter=page_bridge`。
- `fidelity=fallback`。
- `user_agent` 保留 Mobile Safari 特征。

### TC-CDP-12：Chrome DevTools frontend 相关能力已清理

操作步骤：

1. 调用 `GET /_bifrost/api/devtools/cdp/json/list`。
2. 调用 `GET /_bifrost/api/devtools/frontend/inspector.html?ws=...`。
3. 打开 WebUI DevTools 详情页。

预期结果：

- `/json/list` 中的 target 不包含 `systemChromeFrontendUrl`。
- `/api/devtools/frontend/inspector.html` 返回 404。
- WebUI 不显示 `Debug URL`。
- WebUI 不显示 `Copy Debug URL`。
- WebUI 不显示 `Open in Chrome DevTools`。
- WebUI 不显示 `Install Chrome DevTools`。
- WebUI 不存在 `iframe[title="Chrome DevTools Frontend"]`。

## 清理步骤

- 停止临时 Bifrost 进程。
- 停止临时 fixture HTTP server。
- 删除临时 `BIFROST_DATA_DIR`。
- 删除测试临时目录。

## 本轮执行记录

- 2026-04-29：上一轮基础版本通过。执行命令：`source ~/.zshrc && e2e-tests/tests/test_devtools_page_bridge_api.sh`。脚本真实启动临时 Bifrost 代理和 fixture 站点，使用 Playwright 浏览器访问目标页，进入 WebUI DevTools，验证 Elements / Network / Storage / Console 自有面板、多页面切换、control mode `document.title` 执行，以及 Chrome DevTools frontend 安装/托管/打开入口清理。
- 2026-04-29：通过。按“完备端到端测试”要求补充并执行 Elements 节点高亮操作、DOM 手动刷新、Network 新增记录、Storage 运行中 cookie/localStorage/sessionStorage 同步、Storage 受控修改、Console info/error 日志同步与输入脚本执行。执行命令：`source ~/.zshrc && cargo fmt --all -- --check && e2e-tests/tests/test_devtools_page_bridge_api.sh`。输出：`AV-CDP-01/02/03/04/05/06/09/10/11/12/13/14/15/16/17/19/20 plus custom WebUI elements highlight/manual refresh, complete network/storage sync and storage edit, console sync/evaluate, page switching, and Chrome frontend cleanup passed`。
- 2026-04-29：通过。重建 release 二进制后复测同一真实场景，确认发布产物不再暴露 `systemChromeFrontendUrl`，并且 Storage 修改能力在 release 产物中可用。执行命令：`source ~/.zshrc && cargo build --release --bin bifrost`，随后执行 `source ~/.zshrc && SKIP_BUILD=true e2e-tests/tests/test_devtools_page_bridge_api.sh`。输出：`AV-CDP-01/02/03/04/05/06/09/10/11/12/13/14/15/16/17/19/20 plus custom WebUI elements highlight/manual refresh, complete network/storage sync and storage edit, console sync/evaluate, page switching, and Chrome frontend cleanup passed`。
- 2026-04-29：通过。参考 vConsole Element/Storage 插件后，验证 Elements 左右分栏、DOM tree 标签/属性展示、selected node inspector、Storage 行编辑入口。执行命令：`source ~/.zshrc && SKIP_BUILD=true e2e-tests/tests/test_devtools_page_bridge_api.sh`。输出：`AV-CDP-01/02/03/04/05/06/09/10/11/12/13/14/15/16/17/19/20 plus custom WebUI elements highlight/manual refresh, complete network/storage sync and storage edit, console sync/evaluate, page switching, and Chrome frontend cleanup passed`。
