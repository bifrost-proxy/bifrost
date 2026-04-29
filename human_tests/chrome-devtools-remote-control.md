# Chrome DevTools Remote Control 真实场景测试

## 功能模块说明

验证 Bifrost 在用户显式配置 `devtools://` 规则后，能对经过代理的页面建立 page_bridge 降级调试通道，并在 WebUI DevTools tab 中选择在线页面进行基础调试。当前阶段覆盖不依赖系统调试开关的 page_bridge 能力：页面发现、Console 收集、DOM Snapshot、系统 Chrome 自带 DevTools frontend 零安装入口、WebUI 点击后端启动真实 Chrome/Chromium DevTools target、可选官方 frontend 按需缓存与 iframe 内嵌入口、受控 CDP discovery/WebSocket endpoint、只读策略拒绝控制命令、多页面切换，以及移动端 Safari UA 降级路径模拟。

## 前置条件

- 当前目录为 Bifrost 仓库根目录。
- 测试不得使用正式端口 `9900`。
- 启动 Bifrost 必须使用临时 `BIFROST_DATA_DIR`，并携带 `--no-system-proxy`。
- 已安装 WebUI 依赖，可通过 `web/node_modules/playwright` 启动浏览器。
- 执行统一自动化场景：

```bash
bash e2e-tests/tests/test_devtools_page_bridge_api.sh
```

该脚本会自动分配端口、启动本地测试站点、启动 Bifrost、配置 `devtools://mode=read,inject=bridge,csp=respect` 规则、通过代理访问页面、打开 WebUI DevTools tab，并在结束时清理临时目录与进程。

## 测试用例列表

### TC-CDP-01：命中 devtools 规则的代理页面自动建立 page_bridge

**操作步骤**

1. 执行 `bash e2e-tests/tests/test_devtools_page_bridge_api.sh`。
2. 脚本启动代理和本地测试站点后，通过 Admin API 创建 `devtools-page-bridge-api` 规则。
3. 脚本通过代理打开 `http://devtools-fixture.test:<site_port>/basic.html?case=av-cdp-01`。
4. 脚本在页面内检查 `#__bifrost_devtools_bridge__` 与 `window.__BIFROST_DEVTOOLS_BRIDGE__` 状态。
5. 脚本调用 `GET /_bifrost/api/devtools/pages?online=true`、`POST /_bifrost/api/devtools/sessions` 和 session snapshot API。

**预期结果**

- 页面内存在 `#__bifrost_devtools_bridge__`。
- `window.__BIFROST_DEVTOOLS_BRIDGE__.state` 为 `connected`。
- DevTools pages API 返回 URL 包含 `case=av-cdp-01` 的在线页面。
- 页面状态为 `discoverable`，adapter 为 `page_bridge`，fidelity 为 `fallback`。
- session snapshot 包含 `id="debug-fixture"` 的 DOM 内容。
- console snapshot 包含 `bifrost-devtools-basic-ready`。

### TC-CDP-02：没有 devtools 规则时页面不注入且不出现在在线列表

**操作步骤**

1. 执行 `bash e2e-tests/tests/test_devtools_page_bridge_api.sh`。
2. 在创建 devtools 规则前，脚本通过代理打开 `http://127.0.0.1:<site_port>/basic.html?case=no-rule`。
3. 脚本检查页面内注入状态。
4. 脚本调用 `GET /_bifrost/api/devtools/pages?online=true`。

**预期结果**

- 页面内不存在 `#__bifrost_devtools_bridge__`。
- `window.__BIFROST_DEVTOOLS_BRIDGE__` 不存在。
- DevTools pages API 不返回 URL 包含 `case=no-rule` 的页面。

### TC-CDP-03：WebUI DevTools tab 以全屏卡片列表选择在线页面

**操作步骤**

1. 执行 `bash e2e-tests/tests/test_devtools_page_bridge_api.sh`。
2. 脚本打开 `http://127.0.0.1:<proxy_port>/_bifrost/`。
3. 脚本点击侧边栏 `DevTools`。
4. 脚本检查页面主体为 Online Pages 卡片列表，而不是左侧窄列表加右侧空白面板。
5. 脚本在 Online Pages 搜索框输入 `av-cdp-01`。
6. 脚本点击 `Bifrost DevTools Basic` 卡片。
7. 脚本检查页面导航到全屏详情视图，左上角出现 `Back` 返回按钮。
8. 脚本检查详情视图中的 `Debug URL`、`Copy Debug URL`、`Open in Chrome DevTools` 和 `Install Chrome DevTools`。
9. 脚本点击 `Copy Debug URL` 并读取浏览器剪贴板内容。

**预期结果**

- DevTools tab 中可以看到占满主区域的页面卡片，并可选择 `Bifrost DevTools Basic`。
- 点击卡片后进入全屏详情视图，不再保留左侧页面列表；左上角 `Back` 按钮可返回列表。
- 详情视图只展示目标标题、URL、在线状态和调试入口，不展示复杂 CDP JSON 或 fallback 诊断数据。
- `Debug URL` 文本框包含以 `devtools://devtools/bundled/inspector.html?ws=` 开头的地址。
- `Copy Debug URL` 按钮可用，且剪贴板内容等于当前 Debug URL。
- 当前 WebUI 运行在 Chrome/Chromium/Edge 时，展示 `Open in Chrome DevTools` 按钮。
- `Install Chrome DevTools` 按钮可用，但默认不触发安装。

### TC-CDP-04：只读 page_bridge 会话拒绝控制命令

**操作步骤**

1. 执行 `bash e2e-tests/tests/test_devtools_page_bridge_api.sh`。
2. 脚本打开命中 `devtools://mode=read` 的页面并创建 session。
3. 脚本向 `POST /_bifrost/api/devtools/sessions/:id/commands` 发送 `runtime.evaluate` 命令。

**预期结果**

- 命令请求返回 4xx。
- 响应体包含 `requires_control`。
- 页面仍保持在线，Console 与 DOM Snapshot 不受影响。

### TC-CDP-05：移动端 Safari UA 降级路径不依赖系统调试能力

**操作步骤**

1. 执行 `bash e2e-tests/tests/test_devtools_page_bridge_api.sh`。
2. 脚本创建移动端上下文，使用 iPhone Safari User-Agent、移动视口和 touch 能力。
3. 脚本通过 Bifrost 代理打开 `http://devtools-fixture.test:<site_port>/basic.html?case=av-cdp-mobile`。
4. 脚本检查页面 bridge 连接状态，并调用 DevTools pages API。

**预期结果**

- 移动端页面内 `window.__BIFROST_DEVTOOLS_BRIDGE__.state` 为 `connected`。
- DevTools pages API 返回 URL 包含 `case=av-cdp-mobile` 的页面。
- 页面 adapter 为 `page_bridge`，fidelity 为 `fallback`。
- 页面 user_agent 保留 `Mobile` 与 `Safari` 信息。
- 全流程不需要开启系统 Web Inspector、远程调试端口或设备系统调试开关。

### TC-CDP-06：默认不下载 Chrome DevTools frontend 大资源

**操作步骤**

1. 执行 `bash e2e-tests/tests/test_devtools_page_bridge_api.sh`。
2. 脚本在 fresh `BIFROST_DATA_DIR` 中调用 `GET /_bifrost/api/devtools/frontend/status`。
3. 脚本在未调用 install API 的情况下请求 `GET /_bifrost/api/devtools/frontend/inspector.html?ws=127.0.0.1:<proxy_port>/_bifrost/api/devtools/cdp/<page_id>`。

**预期结果**

- fresh 数据目录的 frontend status 初始为未安装。
- 未显式安装时，`inspector.html` 返回 404，不会偷偷下载或托管大资源。
- 仓库中不存在 `web/chrome-devtools-frontend-*.tgz` 或 `web/node_modules/chrome-devtools-frontend*` 这类大资源。

### TC-CDP-07：点击 WebUI 按钮后系统 Chrome 自带 DevTools frontend 真实打开受控 target

**操作步骤**

1. 执行 `bash e2e-tests/tests/test_devtools_page_bridge_api.sh`。
2. 脚本在页面注册成功后调用 `GET /_bifrost/api/devtools/cdp/json/list`。
3. 脚本调用 `GET /_bifrost/api/devtools/cdp/json/version`。
4. 脚本连接 `webSocketDebuggerUrl` 指向的 `/_bifrost/api/devtools/cdp/:page_id` WebSocket。
5. 脚本向该 WebSocket 发送 `Browser.getVersion`、`DOM.getDocument`、`Runtime.evaluate`、`Page.getResourceTree`、`Page.getFrameTree`、`CSS.getMatchedStylesForNode`、`CSS.getComputedStyleForNode`、`CSS.getInlineStylesForNode`、`Runtime.getHeapUsage`、`Network.enable`、`Page.enable`、`Debugger.enable`。
6. 脚本打开 WebUI DevTools tab，选择目标页面后停留在 `Chrome Frontend` 面板。
7. 脚本点击 `Open in Chrome DevTools`。
8. 脚本通过测试专用真实 Chrome/Chromium 的 remote debugging `/json/list` 轮询目标列表。

**预期结果**

- `/json/list` 返回当前页面 target，并包含 `webSocketDebuggerUrl` 与 `systemChromeFrontendUrl`。
- `/json/version` 返回 `Protocol-Version` 为 `1.3`。
- CDP WebSocket 对 `Browser.getVersion` 返回 `Bifrost DevTools Bridge`。
- CDP WebSocket 对 `DOM.getDocument` 返回 `#document` root。
- CDP WebSocket 对只读模式下的 `Runtime.evaluate` 返回 `requires_control`，不绕过策略。
- CDP WebSocket 对 Page、CSS、Runtime heap、Network、Debugger 基础方法均返回成功响应。
- WebUI 的 `Chrome Frontend` 面板默认展示 `Open in Chrome DevTools` 链接。
- 点击 `Open in Chrome DevTools` 后，Bifrost 后端启动真实 Chrome/Chromium 并通过 Chrome remote debugging 创建 `devtools://devtools/bundled/inspector.html?ws=...` target。
- Chrome remote debugging `/json/list` 中能看到 URL 包含当前 `page_id` 的 `devtools://` target，并包含 `webSocketDebuggerUrl`。
- WebUI 同时展示 `Install Chrome DevTools`，但默认不触发安装。

### TC-CDP-08：显式安装后可用官方 frontend iframe 内嵌模式

**操作步骤**

1. 执行 `BIFROST_TEST_INSTALL_EMBEDDED_DEVTOOLS=1 bash e2e-tests/tests/test_devtools_page_bridge_api.sh`。
2. 脚本打开 WebUI DevTools tab，选择目标页面。
3. 脚本点击右侧详情区的 `Install Chrome DevTools` 按钮。
4. 脚本检查页面出现进度条。
5. 脚本等待安装完成后右侧区域自动切换为 Chrome Frontend iframe。
6. 脚本监听 iframe 内 Chrome DevTools frontend 打开的 CDP WebSocket frames。
7. 脚本请求 `GET /_bifrost/api/devtools/frontend/inspector.html?ws=127.0.0.1:<proxy_port>/_bifrost/api/devtools/cdp/<page_id>`。

**预期结果**

- install API 返回 `installed=true` 且 `state=installed`。
- `installPath` 位于当前 `BIFROST_DATA_DIR/admin/devtools-frontend/chrome-devtools-frontend-<version>/`。
- `totalSizeBytes` 大于 1MB，证明资源来自按需下载解包而不是仓库内占位文件。
- `inspector.html` 返回 200，内容包含 Chrome DevTools frontend 入口特征。
- 点击安装后页面出现 `progressbar`，安装期间用户能看到状态进度。
- WebUI 的 `Chrome Frontend` 面板出现 `iframe[title="Chrome DevTools Frontend"]`。
- iframe `src` 指向 `/_bifrost/api/devtools/frontend/inspector.html?ws=.../_bifrost/api/devtools/cdp/:page_id`。
- Chrome DevTools frontend 必须真实打开 `/_bifrost/api/devtools/cdp/:page_id` WebSocket。
- DevTools frontend 启动必需 CDP 方法必须都被请求并收到响应：`Network.enable`、`Page.enable`、`Page.getResourceTree`、`Runtime.enable`、`Debugger.enable`、`DOM.enable`、`CSS.enable`、`Target.setAutoAttach`、`Target.setDiscoverTargets`、`DOM.getDocument`、`CSS.getMatchedStylesForNode`、`CSS.getComputedStyleForNode`。
- CDP WebSocket 不允许出现缺失 response id。
- CDP WebSocket 不允许出现 `unsupported CDP method` 错误。
- Chrome DevTools frontend 页面控制台不允许出现 error 日志。

### TC-CDP-09：WebUI 与内置 Chrome DevTools frontend 支持多页面切换

**操作步骤**

1. 执行 `BIFROST_TEST_INSTALL_EMBEDDED_DEVTOOLS=1 bash e2e-tests/tests/test_devtools_page_bridge_api.sh`。
2. 脚本通过代理分别打开 `basic.html?case=av-cdp-01` 与 `secondary.html?case=av-cdp-secondary`。
3. 脚本调用 DevTools pages API 与 CDP `/json/list`。
4. 脚本在 WebUI DevTools tab 中先选择 `Bifrost DevTools Basic` 并完成内置 frontend 安装。
5. 脚本监听官方 Chrome DevTools frontend iframe 打开的 Bifrost CDP WebSocket frames。
6. 脚本在左侧搜索 `secondary`，点击 `Bifrost DevTools Secondary`。
7. 脚本检查 iframe `src` 切换到第二个页面对应的 `page_id`，并继续监听第二个页面的 CDP WebSocket frames。

**预期结果**

- DevTools pages API 同时返回两个不同 `page_id` 的在线页面。
- `/json/list` 同时返回两个不同 page target，且各自包含独立 `webSocketDebuggerUrl`。
- WebUI 选择第一个页面时 iframe `src` 包含第一个 `page_id`。
- 切换到第二个页面后 iframe `src` 包含第二个 `page_id`。
- 两个页面各自都真实打开过 `/_bifrost/api/devtools/cdp/:page_id` WebSocket。
- 两个页面各自的官方 Chrome DevTools frontend 启动 CDP 方法都有响应，且无 `unsupported CDP method`、无缺失 response id、无 frontend console error。

### TC-CDP-10：WebUI 侧边栏 DevTools 入口位于 Scripts 之后

**操作步骤**

1. 执行 `bash e2e-tests/tests/test_devtools_page_bridge_api.sh`。
2. 脚本打开 `http://127.0.0.1:<proxy_port>/_bifrost/`。
3. 脚本读取侧边栏所有 `data-testid="app-sidebar-nav-item"` 项的 `data-nav-label`。
4. 脚本比较 `Scripts` 与 `DevTools` 在侧边栏中的索引。
5. 脚本继续点击 `DevTools` 并完成在线页面选择，确认入口仍可正常打开。

**预期结果**

- 侧边栏中同时存在 `Scripts` 与 `DevTools`。
- `DevTools` 的索引必须大于 `Scripts`，不出现在 `Network` 后的高优先级位置。
- 点击 `DevTools` 后仍进入 DevTools 页面，并可选择 `Bifrost DevTools Basic`。

### TC-CDP-11：Chrome DevTools frontend flattened session 不空白

**操作步骤**

1. 执行 `bash e2e-tests/tests/test_devtools_page_bridge_api.sh`。
2. 脚本连接 `webSocketDebuggerUrl` 指向的 `/_bifrost/api/devtools/cdp/:page_id` WebSocket。
3. 脚本发送 `Target.attachToTarget(flatten=true)` 并记录返回的 `sessionId`。
4. 脚本携带该 `sessionId` 发送 `Runtime.enable`、`DOM.getDocument`、`Network.enable`、`DOMStorage.getDOMStorageItems` 和 `Page.startScreencast`。
5. 脚本打开 WebUI DevTools tab 并监听官方 Chrome DevTools frontend iframe 的 CDP WebSocket frames。

**预期结果**

- `Target.attachToTarget` 返回非空 `sessionId`。
- 携带 `sessionId` 的 CDP request 都收到携带同一 `sessionId` 的 response。
- `Runtime.executionContextCreated` 等事件携带同一 `sessionId`。
- `DOM.getDocument` 返回真实页面 DOM，包含 `debug-fixture` 节点，不是固定空 `<html>` skeleton。
- `CSS.getInlineStylesForNode` 和 `CSS.getComputedStyleForNode` 返回真实可消费的样式数据，选中 DOM 后 Styles/Metrics 不报错。
- `Runtime.enable` 后能看到页面 console log/warn buffer。
- `Network.enable` 后能看到页面资源/fetch/XHR 网络事件。
- `DOMStorage.getDOMStorageItems` 能读到页面 localStorage 中的 `bifrost-storage-key`。
- `Page.startScreencast` 返回明确 `screencast_disabled`，且不会收到 `Page.screencastFrame`；page_bridge 不再同步 canvas/html-to-image 近似画面。
- Chrome DevTools frontend 抓包中，所有携带 `sessionId` 的 request 都能匹配到同 `sessionId` response，避免 DevTools 面板只显示 URL、内容区域空白。

### TC-CDP-12：页面数据实时刷新

**操作步骤**

1. 执行 `bash e2e-tests/tests/test_devtools_page_bridge_api.sh`。
2. 脚本连接 `/_bifrost/api/devtools/cdp/:page_id` WebSocket 并 attach flattened session。
3. 脚本启用 `Runtime`、`DOM`、`Network`，并调用一次 `Page.startScreencast` 验证该能力被明确关闭。
4. 脚本在已打开的代理页面中追加 DOM 节点、写入 localStorage、输出 console error，并发起新的 fetch 请求。

**预期结果**

- CDP WebSocket 在不重连的情况下收到新的 `Runtime.consoleAPICalled`。
- CDP WebSocket 收到新的 `Network.requestWillBeSent`。
- CDP WebSocket 收到 `DOM.documentUpdated`，随后 `DOM.getDocument` 能看到新增节点。
- `DOMStorage.getDOMStorageItems` 能看到新增 localStorage key。
- `Page.startScreencast` 返回 `screencast_disabled`，且不会收到任何 `Page.screencastFrame`。

### TC-CDP-13：同 tab reload 去重且同 URL 独立 tab 保留

**操作步骤**

1. 执行 `bash e2e-tests/tests/test_devtools_page_bridge_api.sh`。
2. 脚本通过代理打开命中 `devtools://` 的基础页面。
3. 脚本 reload 同一个 tab 并查询 DevTools pages API。
4. 脚本再打开一个新的独立 tab，访问同一个 URL，并再次查询 DevTools pages API。
5. 脚本打开 WebUI DevTools tab，搜索基础页面标题。

**预期结果**

- 同一个 tab reload 后，DevTools pages API 中该 URL 只有一个在线 target。
- 新的独立 tab 访问同一 URL 后，DevTools pages API 中该 URL 有两个不同 target。
- 关闭独立 tab 后，WebUI Online Pages 搜索同一标题只显示当前仍打开的 target，不残留已关闭页面。

### TC-CDP-14：control mode Console evaluate 真实执行

**操作步骤**

1. 执行 `bash e2e-tests/tests/test_devtools_page_bridge_api.sh`。
2. 脚本先验证 `mode=read` 下 `Runtime.evaluate` 返回 `requires_control`。
3. 脚本把规则更新为 `devtools://mode=control,inject=bridge` 并 reload 页面。
4. 脚本通过 CDP `Runtime.evaluate` 执行 `document.querySelector("#debug-fixture").dataset.case`。

**预期结果**

- `mode=read` 不允许执行脚本，返回明确 `requires_control`。
- `mode=control` 下 `Runtime.evaluate` 由 page_bridge 投递到真实页面执行，返回 `basic`。
- WebUI 打开的系统 Chrome DevTools target URL 指向当前 control target，截图可采集，说明不是只生成了 URL。

### TC-CDP-15：WebUI DevTools 详情页可返回列表并切换页面

**操作步骤**

1. 执行 `BIFROST_TEST_INSTALL_EMBEDDED_DEVTOOLS=1 bash e2e-tests/tests/test_devtools_page_bridge_api.sh`。
2. 脚本打开 WebUI DevTools tab，搜索并点击 `Bifrost DevTools Basic` 卡片进入详情。
3. 脚本完成 embedded Chrome DevTools frontend 安装并确认 iframe 已连接 primary page 的 CDP WebSocket。
4. 脚本点击详情页左上角 `Back`。
5. 脚本在返回后的卡片列表搜索 `secondary`，点击 `Bifrost DevTools Secondary` 卡片。
6. 脚本检查详情页 iframe `src` 切换到 secondary page 的 `page_id`，并再次校验 CDP startup protocol。

**预期结果**

- `Back` 按钮从全屏详情返回 Online Pages 卡片列表。
- 返回列表后搜索框可继续使用，页面卡片可再次点击。
- 选择 secondary 页面后，详情页中的 Chrome DevTools frontend 连接 secondary page 的 CDP endpoint，不复用 primary page 的 target。

### TC-CDP-16：DOM 同步只在明确变化时触发，避免 Elements 选中状态抖动

**操作步骤**

1. 执行 `bash e2e-tests/tests/test_devtools_page_bridge_api.sh`。
2. 脚本连接 `/_bifrost/api/devtools/cdp/:page_id` WebSocket 并 attach flattened session。
3. 脚本启用 `DOM.enable` 后等待初始 `DOM.documentUpdated`。
4. 在不修改页面 DOM 的情况下继续等待超过 2 秒。
5. 脚本统计等待前后的 `DOM.documentUpdated` 事件数量。

**预期结果**

- 未发生 DOM mutation 时，不会重复发送 `DOM.documentUpdated`。
- page_bridge 不再每 5 秒整页 `hello()` 同步 DOM，也不会因为定时刷新导致 Elements 树重建。
- Console、Network、Storage 仍在各自发生变化时同步，不依赖整页 DOM 重传。

### TC-CDP-17：Elements 选中 DOM 节点时目标页面显示高亮线框

**操作步骤**

1. 执行 `bash e2e-tests/tests/test_devtools_page_bridge_api.sh`。
2. 脚本通过 CDP `DOM.getDocument` 找到 `#debug-fixture` 的 `nodeId`。
3. 脚本发送 `Overlay.highlightNode`，参数为该 `nodeId`。
4. 脚本在真实目标页面中检查 `#__bifrost_devtools_highlight__` 已出现且有可见尺寸。
5. 脚本发送 `Overlay.hideHighlight`。
6. 脚本检查目标页面高亮线框隐藏。

**预期结果**

- `Overlay.highlightNode` 不需要 control mode，属于只读检查体验能力。
- 目标页面显示蓝色线框与轻量遮罩，位置跟随被选中的真实 DOM 节点。
- `Overlay.hideHighlight` 后线框消失，不影响页面点击和布局。

### TC-CDP-18：Embedded Chrome DevTools 完全禁用左侧 screencast 渲染入口

**操作步骤**

1. 执行 `BIFROST_TEST_INSTALL_EMBEDDED_DEVTOOLS=1 bash e2e-tests/tests/test_devtools_page_bridge_api.sh`。
2. 脚本打开 WebUI DevTools tab，选择目标页面并安装 embedded Chrome DevTools frontend。
3. 脚本等待 `iframe[title="Chrome DevTools Frontend"]` 加载完成。
4. 脚本进入 iframe，检查 `Toggle screencast`、手机图标和 `.screencast` 区域是否仍可见。
5. 脚本检查主 inspector 区域宽度，确认没有为左侧 screencast 预留大块空白。

**预期结果**

- embedded Chrome DevTools frontend 不展示左侧页面渲染/screencast 画面。
- embedded Chrome DevTools frontend 不展示可点击的手机/`Toggle screencast` 切换入口。
- DevTools 主区域由 Elements、Console、Network、Application 等调试面板占用，不再保留左侧空白画面区域。
- `Page.startScreencast` 仍返回 `screencast_disabled`，前后端行为一致。

### TC-CDP-19：Elements 选中节点不会被内部 overlay 或属性噪声刷新冲掉

**操作步骤**

1. 执行 `bash e2e-tests/tests/test_devtools_page_bridge_api.sh`。
2. 脚本连接 `/_bifrost/api/devtools/cdp/:page_id` WebSocket 并 attach flattened session。
3. 脚本启用 `DOM.enable`，调用 `DOM.getDocument` 找到 `#debug-fixture` 的 `nodeId`。
4. 脚本发送 `Overlay.highlightNode`，模拟 Elements 选中节点后目标页面显示高亮线框。
5. 脚本在真实页面上连续修改 `#debug-fixture` 的属性、inline style 和 `document.body` 的 class，模拟应用运行时的样式/状态噪声。
6. 脚本等待超过 1.8 秒并统计 `DOM.documentUpdated` 事件数量。
7. 脚本继续用原 `nodeId` 调用 `CSS.getInlineStylesForNode`。
8. 脚本再追加一个真实子节点 `#debug-fixture-structural`，验证结构变化仍会触发 DOM 刷新。

**预期结果**

- `Overlay.highlightNode` 创建/更新 Bifrost 内部高亮线框时，不触发额外 `DOM.documentUpdated`。
- 连续属性、style、class 噪声不触发整页 `DOM.documentUpdated`，Elements 不反复重建树，选中状态不会因为通讯同步丢失。
- 原 `nodeId` 在噪声后仍可用于 `CSS.getInlineStylesForNode`，说明选中节点语义稳定。
- 真实外部 childList 结构变化仍触发 `DOM.documentUpdated`，随后 `DOM.getDocument` 能看到新增 `#debug-fixture-structural`。

### TC-CDP-20：CDP shim 协议矩阵逐项端到端验证

**操作步骤**

1. 执行 `bash e2e-tests/tests/test_devtools_page_bridge_api.sh`。
2. 脚本连接真实 `/_bifrost/api/devtools/cdp/:page_id` WebSocket。
3. 脚本发送 `Target.attachToTarget(flatten=true)` 并记录 `sessionId`。
4. 脚本使用同一 `sessionId` 逐项发送当前 page_bridge CDP shim 覆盖的 Browser/Target/Runtime/Page/DOM/CSS/Network/DOMStorage/IndexedDB/CacheStorage/Storage/Log/Debugger/Overlay/Accessibility/Performance/Profiler/Security/Inspector/ServiceWorker/Audits/Animation/Autofill/Emulation/DOMDebugger 方法。
5. 脚本对真实数据类方法断言返回内容可消费：`DOM.getDocument`/`DOM.getFlattenedDocument` 包含 `#debug-fixture`，CSS inline/computed style 包含真实 `rgb(11, 22, 33)`，Network enable 推送页面 fetch 事件，DOMStorage local/session storage 包含 fixture key，Storage key 绑定页面 origin。
6. 脚本对 Chrome DevTools frontend 启动兼容类方法断言返回成功响应，并确认 response `sessionId` 与 request 一致。
7. 脚本对 `Page.startScreencast`、`Page.stopScreencast`、`Page.screencastFrameAck` 断言返回 `screencast_disabled`，且不产生 `Page.screencastFrame`。
8. 脚本对未实现或敏感方法发送 `Page.captureScreenshot`、`Input.dispatchMouseEvent`、`Debugger.setBreakpointByUrl`、`Network.getResponseBody`、`Network.getCookies`、`Storage.getCookies`、`Security.getSecurityState`、`Profiler.start`、`HeapProfiler.enable`。

**预期结果**

- 每个已实现的 CDP method 都收到 response，不允许缺失 id。
- 所有携带 `sessionId` 的 response 必须带回同一个 `sessionId`。
- 真实数据类 method 必须返回页面实际 DOM/CSS/Network/Storage 数据，不能返回固定 demo 或空壳。
- Chrome DevTools frontend 兼容 no-op method 返回 `{}` success，不触发 frontend unsupported 错误。
- read mode 下 `Runtime.evaluate` 返回 `requires_control`，control mode 的执行能力由 TC-CDP-14 独立验证。
- screencast 三个 method 都明确返回 `screencast_disabled`，并且没有任何截图/画面同步事件。
- 未实现或敏感 method 都返回稳定 `unsupported CDP method` 错误，不能静默成功。

### TC-CDP-21：rules parallel fixtures 跳过 DevTools 专用规则夹具

**操作步骤**

1. 执行 `bash -n e2e-tests/run_all_tests_parallel.sh && bash -n e2e-tests/test_rules.sh`。
2. 执行 `bash e2e-tests/run_all_tests_parallel.sh -c devtools --no-build`。
3. 执行 `bash e2e-tests/run_all_tests_parallel.sh --no-build`。
4. 检查收集阶段和最终测试结果。

**预期结果**

- `devtools/page_bridge_basic.txt`、`devtools/page_bridge_control.txt`、`devtools/page_bridge_control_allowlist.txt`、`devtools/page_bridge_deny.txt` 不进入通用 rules parallel fixture 执行队列。
- `-c devtools` 输出 `没有找到测试文件` 并以 0 退出，表示该目录下夹具均由专用 DevTools E2E 覆盖。
- 全量 rules parallel fixture 不再因为 DevTools 专用夹具中的 `devtools://` 或动态 `__SITE_PORT__` 占位符失败。
- DevTools page_bridge 能力仍由 `test_devtools_page_bridge_api.sh` 逐项验证。

## 清理步骤

- `test_devtools_page_bridge_api.sh` 退出时会自动终止 Bifrost 进程和本地 HTTP server。
- `test_devtools_page_bridge_api.sh` 退出时会自动删除临时 `BIFROST_DATA_DIR`。
- 如果脚本异常中断，执行 `ps aux | grep bifrost` 与 `lsof -iTCP -sTCP:LISTEN` 查找残留测试进程并手动终止。

## 本轮执行记录

- 执行时间：2026-04-29
- 执行命令 1：`bash e2e-tests/tests/test_devtools_page_bridge_api.sh`
- 实际结果 1：通过。脚本输出 `AV-CDP-01/02/03/04/05/06/07/09/10/11/12/13/14/15/16/17/19/20 plus WebUI card navigation, protocol matrix, and system Chrome open passed`；覆盖 TC-CDP-11 的 flattened CDP session response/event 路由、DOM/CSS/console/network/storage 真实页面数据映射、TC-CDP-12 实时刷新、TC-CDP-13 页面身份语义、TC-CDP-14 control mode evaluate、TC-CDP-15 卡片列表到全屏详情的导航、TC-CDP-16 DOM 变化驱动同步、TC-CDP-17 目标页面 DOM 高亮、TC-CDP-19 内部 overlay 与属性噪声不触发整页 DOM 刷新，以及 TC-CDP-20 CDP shim 协议矩阵逐项端到端验证；脚本实际点击 `Open in Chrome DevTools` 并通过系统 Chrome remote debugging 验证 `devtools://` target URL 与截图。
- 执行命令 2：`BIFROST_TEST_INSTALL_EMBEDDED_DEVTOOLS=1 bash e2e-tests/tests/test_devtools_page_bridge_api.sh`
- 实际结果 2：通过。脚本输出 `AV-CDP-01/02/03/04/05/06/08/09/10/11/12/13/14/15/16/17/18/19/20 plus embedded Chrome DevTools install, iframe, protocol matrix, card navigation, page switching, no screencast pane, and stable Elements selection passed`；官方 Chrome DevTools frontend iframe 抓包中，携带 `sessionId` 的 request 均匹配到同 `sessionId` response，且无缺失 response id、无 `unsupported CDP method`、无 DevTools frontend 控制台 error；脚本真实点击 `Back` 返回卡片列表，再选择 secondary 卡片并验证 iframe 切换到 secondary page；iframe 内看不到 screencast 画面、手机切换按钮或左侧空白渲染区；内部 overlay 与属性噪声不会触发 Elements 整树刷新；协议矩阵逐项验证通过，敏感或未实现方法均返回稳定 CDP error。
- 执行命令 3：`bash -n e2e-tests/run_all_tests_parallel.sh && bash -n e2e-tests/test_rules.sh`
- 实际结果 3：通过。两个 shell 脚本语法检查均通过。
- 执行命令 4：`bash e2e-tests/run_all_tests_parallel.sh -c devtools --no-build`
- 实际结果 4：通过。输出 `没有找到测试文件` 并以 0 退出，确认 devtools 目录下规则夹具已从通用 rules parallel fixture 收集队列排除，交由专用 DevTools E2E 验证。
- 执行命令 5：`bash e2e-tests/run_all_tests_parallel.sh --no-build`
- 实际结果 5：通过。最终结果为 65 个测试套件通过、0 个失败；未再执行 `devtools/page_bridge_basic.txt`、`devtools/page_bridge_control.txt`、`devtools/page_bridge_control_allowlist.txt`、`devtools/page_bridge_deny.txt`。
- 用例结论：TC-CDP-01、TC-CDP-02、TC-CDP-03、TC-CDP-04、TC-CDP-05、TC-CDP-06、TC-CDP-07、TC-CDP-08、TC-CDP-09、TC-CDP-10、TC-CDP-11、TC-CDP-12、TC-CDP-13、TC-CDP-14、TC-CDP-15、TC-CDP-16、TC-CDP-17、TC-CDP-18、TC-CDP-19、TC-CDP-20、TC-CDP-21 均通过。
