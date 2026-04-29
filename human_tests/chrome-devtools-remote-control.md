# Bifrost DevTools Remote Control 真实场景测试

## 功能模块说明

验证 Bifrost 在用户显式配置裸 `devtools://` 规则后，可以对经过代理的页面建立 `page_bridge` 调试通道，并在 WebUI DevTools tab 中使用 Bifrost 自有面板完成 Elements、Network、Cookies、LocalStorage、SessionStorage、Console 调试。Elements 必须可操作，Network / Storage / Console 必须覆盖运行中增量同步，Storage 必须支持修改，Console 必须默认支持多行输入脚本执行、全屏 JavaScript 编辑和真实 JS 异常展示；每条 Console 行必须展示低对比度、小字号、精确到毫秒的输出或执行时间；Console 对象/数组输出必须按结构化值展示摘要、支持层级展开和复制原始内容。各面板必须支持右侧搜索，Elements 自动展开并选中匹配节点，列表类面板过滤并高亮匹配内容。规则编辑器智能提示不应提示 `devtools://value` 或其它必填参数。

页面 bridge 与 Bifrost Admin 的主通信通道必须使用 WebSocket 双向通信。页面不得通过独立 HTTP 请求上报 hello / network / console / eval_result，也不得通过 `eval-next` / `overlay-next` 轮询拉取命令；采集事件需要先进入内存队列，再按短延迟批量异步 flush 到 WS，避免阻塞原页面或造成请求风暴。WebUI 详情页也必须通过 session WebSocket 接收目标页推送；Bifrost Admin 只负责轻量路由、短期状态和有限 ring buffer，不做完整历史数据缓存。WebUI 连接建立或切换 tab 时从目标页重新拉取当前模块数据；任一端断开时另一端必须感知断开状态。

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
3. 配置 `http://devtools-fixture.test:<port>/* devtools:// host://127.0.0.1:<port>`。
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
3. 清空目标页 performance resource timings，等待 2.6 秒。
4. 统计目标页 performance 中 `/_bifrost/api/devtools/bridge/*` HTTP 资源请求数。
5. 通过 WebUI Console 执行脚本，确认命令可以经 bridge 返回结果。

预期结果：

- 返回 `connected`。
- 页面伪造 postMessage 或猜测 token 不会改变 Admin 侧页面状态。
- 空闲 2.6 秒内 bridge HTTP 上报或轮询请求总数为 0，不出现每秒数十次的请求风暴。
- Console 执行结果正常返回，证明 WebSocket 双向命令通道可用。

### TC-CDP-03：WebUI session WS 按需拉取真实页面数据

操作步骤：

1. 打开目标页对应 DevTools session。
2. 建立 `GET /_bifrost/api/devtools/sessions/:session_id/ws` WebSocket。
3. 调用 `POST /_bifrost/api/devtools/sessions/:session_id/refresh`，body 为 `{"scope":"full"}`。
4. 等待 session WebSocket 收到目标页推送的 `snapshot` 消息。

预期结果：

- `dom_tree` 或 `dom_snapshot` 包含 `debug-fixture`。
- `console` 包含 `bifrost-devtools-basic-ready`。
- `network` 包含 `/devtools/api/ping`。
- `storage.local_storage` 包含 `bifrost-storage-key=storage-ready`。
- `storage.session_storage` 包含 `bifrost-session-key=session-ready`。
- `storage.cookies` 包含 `bifrost-cookie-key=cookie-ready`。
- `GET /snapshot` 只作为轻量页面元信息兜底，不承担完整 DOM / Network / Storage / Console 历史缓存职责。

### TC-CDP-04：WebUI DevTools 页面列表可选择在线页面

操作步骤：

1. 打开 `http://127.0.0.1:<proxy_port>/_bifrost/`。
2. 点击侧边栏 `DevTools`。
3. 在搜索框输入 `av-cdp-control`。
4. 点击 `Bifrost DevTools Basic` 卡片。

预期结果：

- 页面列表只展示匹配的在线目标页。
- 进入详情页后展示目标页标题和 URL。
- 详情页不再展示 Adapter / Mode / Rule / Traffic 四个信息卡，避免挤占调试区高度。
- 目标页标题右侧展示跳转 Traffic 的入口，点击后进入 Traffic 页面并选中对应记录。
- URL hover 后出现复制按钮，点击后真实 clipboard 内容为目标页 URL。
- 下方 DevTools content 区域占满剩余空间，不保留被信息卡挤出的空白区域。
- 下方 DevTools content 区域左右 padding 对称，Elements 面板超长 DOM 内容不会撑出容器或挤掉右侧搜索框。
- 详情页存在 `Elements`、`Network`、`Cookies`、`LocalStorage`、`SessionStorage`、`Console` 六个一级 tab。

### TC-CDP-05：WebUI Elements 面板展示 DOM、节点操作与手动刷新

操作步骤：

1. 在 WebUI DevTools 详情页打开 `Elements` tab。
2. 点击包含 `debug-fixture` 的 DOM 节点。
3. 在目标页追加 `#debug-fixture-manual-refresh` 节点。
4. 点击 WebUI 详情页 refresh 按钮。
5. 在 tab 右侧搜索框输入 `manual-refresh`。

预期结果：

- 面板展示 Chrome DevTools 风格的可展开/折叠 DOM tree，不展示右侧 selected node 详情侧边栏。
- DOM tree 中标签名、属性名、属性值有区分渲染，闭合标签和空标签展示符合 HTML tree 习惯。
- DOM tree 不展示只有箭头、没有文本的 `#document` 幽灵 root，首个可见节点从 `<html>` 开始。
- DOM tree 不展示纯换行/缩进 text node 形成的空白行。
- DOM tree 中超长属性或脚本文本默认只展示不超过 120 字符的预览，不导致整个工作区横向溢出。
- 点击超长内容旁的详情入口会打开弹窗展示完整单项内容；弹窗 `Copy` 按钮可复制完整内容到真实 clipboard。
- 内容包含 `debug-fixture`。
- 点击 DOM 节点后，目标页出现 `#__bifrost_devtools_highlight__` overlay。
- 点击 DOM 节点后，目标页出现 `#__bifrost_devtools_highlight__` overlay，WebUI 内该节点保持选中高亮。
- 手动刷新后，Elements 面板包含 `debug-fixture-manual-refresh`。
- 搜索 `manual-refresh` 后，Elements 自动展开并选中第一个匹配节点。
- 不出现官方 Chrome DevTools iframe。

### TC-CDP-06：WebUI Network 面板展示完整网络事件与新增记录

操作步骤：

1. 在 WebUI DevTools 详情页打开 `Network` tab。
2. 目标页运行时发起 `fetch('/devtools/api/extra?case=webui-network-complete')`。
3. 点击 WebUI 详情页 refresh 按钮。
4. 在 tab 右侧搜索框输入 `webui-network-complete`。

预期结果：

- 面板复用 Traffic 页面列表风格，展示序号、状态点、Protocol、Method、Status、Host、Path、Type、Size、Time 等列。
- 面板 DOM 中存在 `traffic-table` 虚拟列表结构。
- 内容包含 `/devtools/api/ping`。
- 刷新后内容包含 `webui-network-complete`。
- 搜索后 Network 列表只展示匹配记录，并高亮匹配内容。

### TC-CDP-07：WebUI 存储一级 tab 展示、同步、复制、行内修改与删除

操作步骤：

1. 在 WebUI DevTools 详情页分别打开 `Cookies`、`LocalStorage`、`SessionStorage` 一级 tab。
3. 目标页运行时设置 `bifrost-cookie-live`、`bifrost-storage-live`、`bifrost-session-live`。
4. 点击 WebUI 详情页 refresh 按钮。
5. 在 `LocalStorage` tab 点击 `bifrost-storage-live` 行的复制按钮，读取真实 clipboard。
6. 点击 `bifrost-storage-live` 行的编辑按钮，确认该行进入行内编辑态，并自动填入 key、value。
7. 切换 `Cookies` tab，点击 Add，在行内写入 `bifrost-cookie-edit=cookie-edited`。
8. 切换 `LocalStorage` tab，点击 Add，在行内写入 `bifrost-storage-edit=storage-edited`。
9. 点击 `bifrost-storage-edit` 行删除按钮。
10. 切换 `SessionStorage` tab，点击 Add，在行内写入 `bifrost-session-edit=session-edited`。
11. 在目标页读取 cookie/localStorage/sessionStorage。
12. 点击 WebUI 详情页 refresh 按钮。
13. 使用裸 `devtools://` 的页面 session 直接调用 `storage.set` 写入 `bifrost-default-storage-edit=default-ok`。
14. 在 `LocalStorage` tab 右侧搜索框输入 `bifrost-storage-live`。

预期结果：

- Cookies 区域包含 `bifrost-cookie-key`。
- Local Storage 区域包含 `bifrost-storage-key`。
- Session Storage 区域包含 `bifrost-session-key`。
- `Cookies` / `LocalStorage` / `SessionStorage` 作为三个一级 tab 展示，不再嵌套在 `Storage` tab 内。
- 刷新后 Cookies 区域包含 `bifrost-cookie-live`。
- 刷新后 Local Storage 区域包含 `bifrost-storage-live`。
- 刷新后 Session Storage 区域包含 `bifrost-session-live`。
- 复制 `bifrost-storage-live` 后真实 clipboard 内容为 `storage-live`。
- 行内编辑入口会把 `bifrost-storage-live=storage-live` 自动带入当前行编辑器。
- 目标页真实读到 `bifrost-cookie-edit=cookie-edited`。
- 目标页曾真实读到 `localStorage.getItem('bifrost-storage-edit') === 'storage-edited'`。
- 删除后目标页真实读到 `localStorage.getItem('bifrost-storage-edit') === null`，WebUI 不再展示该 key。
- 目标页真实读到 `sessionStorage.getItem('bifrost-session-edit') === 'session-edited'`。
- 再次刷新后 WebUI 三个存储面板展示对应编辑后的值。
- Storage 编辑默认可用，不显示 `Storage editing requires mode=control.`，目标页真实读到 `localStorage.getItem('bifrost-default-storage-edit') === 'default-ok'`。
- 搜索后 LocalStorage 列表只展示匹配行，并高亮匹配内容。

### TC-CDP-08：WebUI Console 面板展示完整日志、多行输入并执行脚本

操作步骤：

1. 使用裸 `devtools://` 规则访问目标页。
2. 在 WebUI DevTools 详情页打开 `Console` tab。
3. 目标页运行时输出 `console.info('bifrost-console-info-live')`、`console.error('bifrost-console-error-live')` 和 `console.log('bifrost-console-object-ready', { pageId: 'basic', nested: { answer: 42 }, items: ['alpha', 'beta'] })`。
4. 点击 WebUI 详情页 refresh 按钮。
5. 在底部固定输入框填入脚本 `document.title`。
6. 点击 `Run`。
7. 在底部固定输入框填入 `window.reload()`。
8. 点击 `Run`。
9. 点击输入框右侧全屏编辑按钮，在全屏 JavaScript 编辑器输入多行脚本 `(() => {\n  return document.title + " fullscreen";\n})()` 并运行。
10. 在 tab 右侧搜索框输入 `bifrost-console-error-live`。

预期结果：

- Console 日志包含 `bifrost-devtools-basic-ready`。
- Console 日志包含 `bifrost-devtools-warning-ready`。
- 刷新后 Console 日志包含 `bifrost-console-info-live`。
- 刷新后 Console 日志包含 `bifrost-console-debug-live`。
- 刷新后 Console 日志包含 `bifrost-console-error-live`。
- Console 按 log/info/warn/error/debug 区分不同等级。
- Console 对象日志默认展示 `Object { ... }` 摘要，而不是把对象拍平成不可读的长字符串。
- 点击对象摘要后，Console 行内按层级展开，能看到 `nested`、`items` 等属性，以及数组索引。
- 点击复制按钮后，剪贴板包含原始 console 内容，例如 `bifrost-console-object-ready` 和 `"nested"`。
- 每条 Console 行展示 `HH:mm:ss.SSS` 格式时间信息，文字低对比度、小字号，不干扰主要日志内容。
- 输入框始终固定在 Console 面板底部，不因日志滚动离开面板。
- 执行后 Console 列表展示一条 input 行，内容为输入的代码。
- 执行后 Console 列表展示一条 result 行，内容为执行结果。
- 执行结果显示 `Bifrost DevTools Basic`。
- 执行 `window.reload()` 后 Console 列表展示一条 error 行，内容包含远端 JS 异常，例如 `window.reload is not a function`，不显示 `Request failed with status code 400`。
- 全屏 JavaScript 编辑器可以打开、输入多行脚本并运行；Console 列表展示对应 input 行和 `Bifrost DevTools Basic fullscreen` result 行。
- 搜索后 Console 只展示匹配日志，并高亮匹配内容。
- 执行行为进入 evaluate audit。

### TC-CDP-09：裸 devtools:// 默认启用完整 Console 能力

操作步骤：

1. 使用裸 `devtools://` 规则访问目标页。
2. 打开 WebUI `Console` tab。
3. 通过 API 直接执行 `runtime.evaluate`，表达式为 `document.title`。

预期结果：

- 页面不提示 evaluate 需要 `mode=control`。
- `Run` 按钮可用。
- 通过 API 直接执行 `runtime.evaluate` 返回 `Bifrost DevTools Basic`。

### TC-CDP-10：多页面切换不复用旧页面状态

操作步骤：

1. 打开 primary 页面 `basic.html?case=av-cdp-control`。
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

### TC-CDP-12：SPA 路由切换或 HTML 预取不会展示幽灵页面

操作步骤：

1. 打开命中 `devtools://` 规则的 primary 页面。
2. 在目标页内发起不会执行脚本的 HTML 请求，例如 `fetch('/secondary.html?case=ghost-fetch').then(r => r.text())`。
3. 调用 `GET /_bifrost/api/devtools/pages?online=true`。
4. 调用 `GET /_bifrost/api/devtools/cdp/json/list`。
5. 在 WebUI DevTools 页面列表搜索当前业务页关键字。

预期结果：

- pages API 不包含 `case=ghost-fetch` 这类只完成 HTML 注入但未执行 bridge hello 的候选页。
- CDP target 列表不包含 `case=ghost-fetch`。
- WebUI 页面列表只展示真实在线页面，不展示 `(untitled)` 的 candidate/read 幽灵卡片。
- 真实独立 tab 打开同 URL 时仍保留为独立目标，不被错误去重。

### TC-CDP-13：目标页刷新后 WebUI 详情页自动恢复监听

操作步骤：

1. WebUI DevTools 选择一个裸 `devtools://` 规则发现的在线页面，进入详情页。
2. 打开 `Console` tab 并执行 `document.title`，确认执行成功。
3. 刷新目标页面本身，等待目标页 bridge 状态重新变为 `connected`。
4. 不退出 WebUI 详情页，点击 WebUI refresh。
5. 打开 `Elements` tab，确认 DOM 内容仍可读取。
6. 再次打开 `Console` tab 并执行 `document.title`。
7. 返回页面列表，搜索当前页面。

预期结果：

- WebUI 不出现 `400` / `page not found` 这类必须退出重进才能恢复的错误。
- WebUI refresh 后 Elements 面板仍包含目标页 DOM。
- Console 再次执行 `document.title` 仍返回目标页标题。
- 页面列表只保留刷新后的一个真实在线页面卡片。

### TC-CDP-14：Chrome DevTools frontend 相关能力已清理

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

### TC-CDP-15：WebUI session WS 与轻量服务端缓存回归

操作步骤：

1. 打开 WebUI DevTools 详情页。
2. 在浏览器开发者工具 Network 中过滤 `/_bifrost/api/devtools/sessions/`。
3. 切换 Elements、Network、LocalStorage、Console tab。
4. 保持目标页静默超过 60 秒，不输出 console、不发起请求。
5. 返回页面列表，点击 Refresh Pages。
6. 刷新 WebUI 管理端页面后重新进入同一目标页。
7. 关闭目标页。

预期结果：

- WebUI 详情页存在一条 session WebSocket 连接。
- tab 切换只触发当前模块 scoped refresh，不出现高频全局 snapshot 轮询。
- 目标页静默但 bridge WS 仍连接时，页面仍出现在在线页面列表。
- WebUI 管理端刷新后，重新进入详情页会由目标页重新推送 DOM / Network / Storage / Console 数据；目标页保存有界 console/network buffer 并实时读取 DOM/storage，Bifrost 服务端不保存完整历史缓存。
- 目标页关闭后 WebUI 收到 disconnected 状态；页面不再作为在线可调试页面展示。

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
- 2026-04-29：通过。针对线上反馈的路由切换幽灵页和目标页刷新后 WebUI 详情页 400 问题，补充并执行 TC-CDP-12 / TC-CDP-13。执行命令：`source ~/.zshrc && cargo build --release --bin bifrost`，随后执行 `source ~/.zshrc && SKIP_BUILD=true e2e-tests/tests/test_devtools_page_bridge_api.sh`。验证 HTML fetch 产生的未执行 candidate 不进入 pages API / CDP target / WebUI 列表；验证目标页刷新后不退出 WebUI 详情页即可 refresh Elements，并再次执行 Console `document.title`。输出：`AV-CDP-01/02/03/04/05/06/09/10/11/12/13/14/15/16/17/19/20/21/22 plus custom WebUI elements highlight/manual refresh, complete network/storage sync and storage edit, console sync/evaluate, page switching, ghost candidate hiding, reload recovery, and Chrome frontend cleanup passed`。
- 2026-04-29：通过。针对 Elements 面板存在空白行和幽灵 root 的视觉问题，补充并执行 TC-CDP-05 回归断言：Elements tree 首个可见 DOM 节点必须是 `<html>`，且不存在空文本 DOM 行。执行命令：`source ~/.zshrc && SKIP_BUILD=true e2e-tests/tests/test_devtools_page_bridge_api.sh`。输出：`AV-CDP-01/02/03/04/05/06/09/10/11/12/13/14/15/16/17/19/20/21/22/23 plus custom WebUI elements highlight/manual refresh, complete network/storage sync and storage edit, console sync/evaluate, page switching, ghost candidate hiding, reload recovery, clean Elements tree rendering, and Chrome frontend cleanup passed`。
- 2026-04-29：通过。按产品调整删除 Elements 右侧 selected node 侧边栏，保留 DOM tree 选中和目标页 highlight。执行命令：`source ~/.zshrc && SKIP_BUILD=true e2e-tests/tests/test_devtools_page_bridge_api.sh`。输出：`AV-CDP-01/02/03/04/05/06/09/10/11/12/13/14/15/16/17/19/20/21/22/23/24 plus custom WebUI elements highlight/manual refresh, complete network/storage sync and storage edit, console sync/evaluate, page switching, ghost candidate hiding, reload recovery, clean Elements tree rendering, removed Elements sidebar, and Chrome frontend cleanup passed`。
- 2026-04-29：通过。按产品调整移除 Storage 的 `mode=control` 限制，验证 read-mode session 也可以通过 `storage.set` 写入目标页 localStorage。执行命令：`source ~/.zshrc && SKIP_BUILD=true e2e-tests/tests/test_devtools_page_bridge_api.sh`。输出：`AV-CDP-01/02/03/04/05/06/09/10/11/12/13/14/15/16/17/19/20/21/22/23/24/25 plus custom WebUI elements highlight/manual refresh, complete network/storage sync and storage edit, read-mode storage edit, console sync/evaluate, page switching, ghost candidate hiding, reload recovery, clean Elements tree rendering, removed Elements sidebar, and Chrome frontend cleanup passed`。
- 2026-04-29：通过。按 WebSocket-only 与轻量服务端缓存要求复测，验证 bridge 无 HTTP 上报风暴、WebUI session WS 建链后按 `scope=full` 从目标页重新拉取 DOM / Network / Storage / Console，目标页刷新后旧 sender 不覆盖新 sender，静默但 WS 仍连接的 secondary 页面仍可从在线列表切换调试，Back 后晚到 snapshot 不会把详情页复活。执行命令：`source ~/.zshrc && cargo build --release --bin bifrost`，随后执行 `source ~/.zshrc && SKIP_BUILD=true e2e-tests/tests/test_devtools_page_bridge_api.sh`。输出：`DevTools custom bridge E2E passed: WS-only page bridge, lightweight WebUI session snapshot refresh, elements/network/storage/console, UI search/layout, page switching, reload recovery, and Chrome frontend cleanup passed`。
- 2026-04-30：通过。按 Console 结构化对象展示要求复测，验证 page bridge 上报 console `args/raw`，WebUI 默认展示 Object 摘要，点击后展开 `nested` / `items`，复制按钮可复制原始序列化内容。执行命令：`source ~/.zshrc && cargo build --release --bin bifrost`，随后执行 `source ~/.zshrc && SKIP_BUILD=true e2e-tests/tests/test_devtools_page_bridge_api.sh`。输出：`DevTools custom bridge E2E passed: WS-only page bridge, lightweight WebUI session snapshot refresh, elements/network/storage/console, structured console object expansion/copy, UI search/layout, page switching, reload recovery, and Chrome frontend cleanup passed`。
- 2026-04-30：通过。按 Network 列表与 Traffic 页面体验一致要求复测，验证 DevTools Network 复用 `traffic-table` 虚拟列表结构，展示 Protocol / Method / Status / Host / Path 等列，搜索过滤和新增网络记录仍可用。执行命令：`source ~/.zshrc && SKIP_BUILD=true e2e-tests/tests/test_devtools_page_bridge_api.sh`。输出：`DevTools custom bridge E2E passed: WS-only page bridge, lightweight WebUI session snapshot refresh, elements/network/storage/console, Traffic-style network table, structured console object expansion/copy, UI search/layout, page switching, reload recovery, and Chrome frontend cleanup passed`。
