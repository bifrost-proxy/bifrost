# Bifrost DevTools Remote Control 真实场景测试

## 功能模块说明

验证 Bifrost 在用户显式配置裸 `devtools://` 规则后，可以对经过代理的页面建立 `page_bridge` 调试通道，并在 WebUI DevTools tab 中使用 Bifrost 自有面板完成 Elements、Network、Cookies、LocalStorage、SessionStorage、Console 调试。Elements 必须可操作，支持 DOM 树节点高亮、目标页鼠标拾取元素并自动同步 WebUI 选中节点，目标页 overlay 必须展示节点名称、尺寸、color、font、padding、margin 等核心信息；Network / Storage / Console 必须覆盖运行中增量同步，Storage 必须支持修改，Console 必须默认支持多行输入脚本执行、全屏 JavaScript 编辑和真实 JS 异常展示；每条 Console 行必须展示低对比度、小字号、精确到毫秒的输出或执行时间；Console 对象/数组输出必须按结构化值展示摘要、支持层级展开和复制原始内容。各面板必须支持右侧搜索，Elements 自动展开并选中匹配节点，列表类面板过滤并高亮匹配内容。规则编辑器智能提示不应提示 `devtools://value` 或其它必填参数。

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

### TC-CDP-16：页面桥接消息重放去重与有界队列稳定性

操作步骤：

1. 打开匹配裸 `devtools://` 规则的目标页，等待目标页 bridge WS 进入 `connected`。
2. 打开 WebUI DevTools 详情页，确认 session WS 建立并展示 Elements / Network / Storage / Console 数据。
3. 触发目标页输出 console、发起 fetch/XHR，并执行一次 WebUI Console 表达式。
4. 断开或刷新目标页，使 page bridge WS 重连并重放未确认 inflight 消息。
5. 观察 WebUI Console / Network 列表，并继续切换 tab 触发 scoped refresh。
6. 使用单元测试验证服务端桥接消息 seq 去重和 WebUI/目标页 live channel 有界容量。

预期结果：

- page bridge WS 重连后，同一 `seq` 的 console/network/eval-result/close 消息不会在服务端重复处理。
- 重放消息仍会收到 ack，避免目标页 outbox 卡住或继续风暴式重试。
- WebUI session live 通道和目标页 bridge command 通道均为有界队列；慢消费者不会导致 Bifrost admin 进程无限堆积内存。
- 队列满或连接断开时，服务端会移除 stale sender，后续刷新依赖目标页重新推送当前模块数据。

### TC-CDP-17：shell E2E fixture HTTP server 生命周期稳定性

操作步骤：

1. 在仓库根目录创建独立临时目录，例如 `TEST_ROOT="$(mktemp -d /tmp/bifrost-devtools-human.XXXXXX)"`。
2. 选择非 9900 的随机 fixture 端口，例如 `SITE_PORT=$((10000 + RANDOM % 5000))`。
3. 执行 `source ~/.zshrc && TEST_ROOT="$TEST_ROOT" SITE_PORT="$SITE_PORT" SKIP_BUILD=true bash e2e-tests/tests/test_devtools_page_bridge_api.sh`。
4. 观察脚本输出和 `/tmp` 下保存的执行日志。
5. 重复执行至少 3 次，每次使用不同 `TEST_ROOT` 和 `SITE_PORT`。

预期结果：

- 脚本尊重外部传入的 `TEST_ROOT` 与 `SITE_PORT`，并拒绝 9900 或已被占用的端口。
- fixture `http.server` 启动后先通过 `basic.html` 探活，再进入 Bifrost / Playwright 验证。
- 成功路径只在脚本结尾由 cleanup 对本脚本启动的 `http.server` PID 发送 `TERM`，随后 `wait` 回收，不出现未收敛的 `Terminated: 15` 后台作业噪声。
- 失败路径保留 `TEST_ROOT` 现场，并打印 `bifrost.log` / `site.log`，便于定位真正失败命令。
- `bifrost-admin remote_invoke worker` 的 `requires a sync session token` 日志不影响本用例结论；该日志是 CI 未登录 sync session 的已知噪声。

### TC-CDP-18：Network 行详情在 DevTools 右侧内嵌展示

操作步骤：

1. 启动临时 Bifrost 代理并配置显式 `devtools://` 规则。
2. 使用浏览器通过该代理打开测试页面，触发至少一条 `fetch('/devtools/api/extra?case=webui-network-complete')` 网络请求。
3. 打开 Bifrost WebUI 的 DevTools 页面，选择该在线页面。
4. 切换到 `Network` tab，搜索 `webui-network-complete`。
5. 点击匹配的 Network 行。

预期结果：

- WebUI URL 仍停留在 `/devtools`，不会跳转到 `/traffic`。
- Network tab 右侧直接展示复用的 TrafficDetail 详情组件，列表仍保留在左侧。
- 详情组件中可以看到对应请求 URL / query marker，Request / Response 面板可正常展开和搜索。
- 找不到对应 Traffic 记录时，右侧详情面板展示 page bridge 已上报的 URL、Method、Status、Type、Host、Path、Client Request ID 等信息，并给出匹配失败提示；不发生路由跳转。

### TC-CDP-19：Console 纯文本颜色与对象展开对齐

操作步骤：

1. 启动临时 Bifrost 代理并配置显式 `devtools://` 规则。
2. 使用浏览器通过该代理打开测试页面，页面执行 `console.log('bifrost-devtools-basic-ready')` 与 `console.log('bifrost-console-object-ready', { nested: { answer: 42 } })`。
3. 打开 WebUI DevTools，选择该页面并切换到 `Console` tab。
4. 展开包含 `bifrost-console-object-ready` 的对象输出。

预期结果：

- 普通 `console.log('text')` 的纯文本按 log/info 等级展示为普通深色文本，不使用对象字符串的红色样式。
- warn/error/debug 仍按日志等级使用对应背景和文字强调。
- 对象输出的摘要与展开树在同一条 console row 内对齐；展开后的属性树缩进在对象摘要下方，不会挤到前一个文本参数下面。
- 对象属性内部的字符串、数字、布尔等 primitive 仍保留类似 Chrome Console 的语法色。

### TC-CDP-20：DevTools 明暗主题切换回归

操作步骤：

1. 启动临时 Bifrost 代理并配置显式 `devtools://` 规则。
2. 使用浏览器通过该代理打开测试页面。
3. 打开 WebUI DevTools，选择该在线页面进入详情页。
4. 点击 WebUI 侧边栏主题切换按钮切换到 dark theme。
5. 依次观察 Elements、Network、Cookies、LocalStorage、SessionStorage、Console 面板。
6. 打开 Console 全屏 JavaScript 编辑器。

预期结果：

- DevTools 页面外层、workspace、卡片、表格、DOM tree、Storage 表格、Console row 和 fallback network detail 都跟随 dark theme，不出现整块白底。
- 搜索高亮、选中 DOM 行、错误/警告/结果 Console 行在暗色主题下仍可读。
- Console 全屏 JavaScript 编辑器使用暗色 Monaco 主题。
- 切回 light theme 后 DevTools 所有面板恢复浅色主题，功能状态不丢失。

### TC-CDP-21：DevTools 详情路由刷新恢复

操作步骤：

1. 启动临时 Bifrost 代理并配置显式 `devtools://` 规则。
2. 使用浏览器通过该代理打开测试页面。
3. 打开 WebUI DevTools 页面列表，点击该在线页面卡片。
4. 观察 WebUI 地址栏。
5. 刷新 WebUI 管理端页面。

预期结果：

- 点击在线页面后 WebUI 地址从 `/devtools` 更新为 `/devtools/:page_id`。
- 刷新 WebUI 管理端后自动回到同一个 DevTools 详情页，不回退到在线页面列表。
- 详情页恢复后 Elements tree、tab 搜索框、refresh 按钮和 Console 执行能力仍可用。
- 点击 Back 后地址回到 `/devtools`，并展示在线页面列表。

### TC-CDP-22：Network 前端采集去重与 Traffic 映射

操作步骤：

1. 启动临时 Bifrost 代理并配置显式 `devtools://` 规则。
2. 使用浏览器通过该代理打开测试页面，页面发起同源 `fetch('/devtools/api/ping?case=basic')`。
3. 通过 WebUI DevTools 或 session snapshot 查看 Network 列表。
4. 使用 Network 行中的 client request id 查询 `/api/devtools/network/traffic/:client_req_id`。
5. 打开映射到的 Traffic 详情，检查 request headers。

预期结果：

- Network 列表以 page bridge 前端采集事件为准，同一 fetch/XHR 只展示一条记录，不同时展示 performance 兜底记录或 Traffic 派生记录。
- 该记录携带与 `x-bifrost-client-request-id` 对应的 client request id。
- client request id 可以映射到对应 Traffic id，Network 行点击后可展示完整 TrafficDetail。
- `x-bifrost-client-request-id` 不会出现在 Traffic request headers 中，也不会转发给目标服务端。

### TC-CDP-23：Storage 大数据量切换性能

操作步骤：

1. 启动临时 Bifrost 代理并配置显式 `devtools://` 规则。
2. 使用浏览器通过该代理打开测试页面。
3. 在目标页 localStorage 与 sessionStorage 中分别写入 400 个以上 key/value。
4. 打开 WebUI DevTools 详情页，切换 LocalStorage、SessionStorage、Cookies tab。
5. 观察 tab 切换响应时间和 DOM 渲染行数。

预期结果：

- 点击 Storage 相关 tab 后 UI 立即切换，不出现数秒卡顿。
- LocalStorage / SessionStorage 只渲染视口附近的可见行，不一次性渲染所有几百上千条数据。
- 搜索、行内编辑、复制、删除仍可用。
- 切换 tab 触发的 storage snapshot 刷新是异步的，不阻塞 tab 激活状态更新。
- **执行记录（2026-05-02）**：
  - `BIFROST_E2E_REPORT_DIR=/tmp/bifrost-e2e-shell-shard3-fixed2 BIFROST_E2E_SHELL_JOBS=16 BIFROST_E2E_RETRY_FAILED_ONCE=1 BIFROST_E2E_HTTP_RETRIES=2 TIMEOUT=90 bash scripts/ci/local-ci.sh --skip-static --e2e-only shell --shard 3/3`：PASS，shard 3 共 25 个 shell 用例全部通过；其中 `test_devtools_page_bridge_api.sh` 28s 通过，覆盖 AV-CDP-40 Storage 大数据量虚拟列表、tab 切换性能和后续行内新增/编辑/复制/删除

### TC-CDP-24：Console 标准 `%c` 样式格式化

操作步骤：

1. 启动临时 Bifrost 代理并配置显式 `devtools://` 规则。
2. 使用浏览器通过该代理打开测试页面。
3. 在目标页执行 `console.log('%cbifrost-console-css-live', 'color: rgb(255, 0, 0); font-weight: 700')`。
4. 打开 WebUI DevTools，选择该页面并切换到 `Console` tab。

预期结果：

- Console 列表只展示 `bifrost-console-css-live` 文本，不把 `%c` 占位符或 `color: ...` 样式参数当普通文本展示。
- 文本应用 `color: rgb(255, 0, 0)` 和加粗样式。
- 同一条 console row 仍可被搜索过滤，复制原始内容按钮仍可用。

### TC-CDP-25：Network 浏览器侧 metadata 采集

操作步骤：

1. 启动临时 Bifrost 代理并配置显式 `devtools://` 规则。
2. 使用浏览器通过该代理打开测试页面。
3. 在目标页执行 `fetch('/devtools/api/meta?case=network-meta&foo=bar', { headers: { 'x-bifrost-fixture-header': 'fixture-request' } })`。
4. 打开 WebUI DevTools，选择该页面并切换到 `Network` tab。
5. 点击对应 Network 行查看右侧详情。

预期结果：

- Network 行展示明确 status，能区分请求已发出、失败、成功或可能来自缓存，不因缺少 status 误判。
- 右侧详情展示 URL query 参数，例如 `foo=bar`。
- 右侧详情展示浏览器侧采集的 request headers，例如 `x-bifrost-fixture-header: fixture-request`，但不展示内部 `x-bifrost-client-request-id`。
- 右侧详情展示浏览器可读的 response headers。
- 默认不采集 request body 或 response body。

### TC-CDP-26：Elements 目标页鼠标拾取与样式信息 overlay

操作步骤：

1. 启动临时 Bifrost 代理并配置显式 `devtools://` 规则。
2. 使用浏览器通过该代理打开测试页面。
3. 打开 WebUI DevTools 详情页，切换到 `Elements` tab。
4. 点击 Elements tab 右侧的元素拾取按钮。
5. 在目标页面移动鼠标到 `#debug-fixture`，再点击该元素。

预期结果：

- 目标页面进入鼠标选择节点模式，hover 时显示高亮 overlay。
- 点击目标页元素不会触发原页面默认跳转或按钮行为，拾取模式自动退出。
- WebUI Elements 面板自动展开并选中对应 `#debug-fixture` DOM row。
- 目标页 overlay 保持显示，信息卡包含节点名称、尺寸、`Color`、`Font`、`Padding`、`Margin`。
- 目标页 overlay 信息卡保持合理宽度和两列布局，靠近视口边缘时不会被压成逐字竖排，长 `Font` 值在卡片内自然换行。
- 整个流程通过 page bridge WebSocket 双向通信完成，不新增独立 HTTP 轮询。

### TC-CDP-27：Network 标签资源精准映射 Traffic

操作步骤：

1. 启动临时 Bifrost 代理并配置显式 `devtools://` 规则。
2. 使用浏览器通过该代理打开包含 `<img>` / `<script>` / `<link>` 资源标签的测试页面。
3. 切换到 WebUI DevTools `Network` tab。
4. 搜索由标签触发的资源请求并打开详情。

预期结果：

- bridge 脚本在 HTML 最前面启动；静态资源标签 URL 被写入内部 `__bifrost_client_req_id`。
- 动态 `img.src = ...` 或 `setAttribute('src', ...)` 产生的资源请求也带有内部 id。
- Bifrost 代理在处理请求最前面提取并删除该 query，真实上游请求、Traffic URL、Traffic request headers 和 WebUI 展示 URL 均不包含 `__bifrost_client_req_id`。
- Network 行可以通过该内部 id 精确映射到对应 Traffic 记录，展示完整 status、method、protocol、URL、query、request headers、response headers、size 与耗时。

### TC-CDP-28：TLS 全截包 Network 与 Traffic 完整匹配

操作步骤：

1. 启动临时 HTTPS fixture 站点。
2. 启动临时 Bifrost 代理，配置 `tlsIntercept:// devtools://` 规则，并使用浏览器全量代理访问 HTTPS fixture。
3. 允许测试浏览器忽略本地 MITM 证书错误。
4. 在目标页触发 fetch/XHR 与标签资源请求。
5. 打开 WebUI DevTools `Network` tab，逐条查看对应请求详情，并用 Traffic API 查询匹配记录。

预期结果：

- fetch/XHR 请求通过 `x-bifrost-client-request-id` 精确映射 Traffic。
- 标签资源请求通过 `__bifrost_client_req_id` 精确映射 Traffic。
- 所有匹配请求在 Network 中展示完整基础信息，不只显示 protocol 和 URL。
- Traffic 记录的 URL、method、status 与 Network 事件一致。
- 内部 header/query 不会出现在真实 HTTPS 上游、Traffic URL 或 Traffic request headers。

### TC-CDP-29：DevTools 详情刷新不触发目标页请求

操作步骤：

1. 启动临时 Bifrost 代理并配置显式 `devtools://` 规则。
2. 使用浏览器通过该代理打开测试页面，记录目标页 `sessionStorage` load counter 与业务 fetch counter。
3. 打开 WebUI DevTools 详情页。
4. 点击详情页右上角刷新按钮，并在 Elements / Network / Storage tab 下分别重复。

预期结果：

- WebUI 详情刷新只通过 session WebSocket 发送 snapshot request。
- 目标页不 reload，load counter 不增加。
- 目标页不会因为 WebUI 刷新重新发起页面业务请求，业务 fetch counter 不增加。
- bridge 不产生独立 HTTP 上报或轮询风暴。

### TC-CDP-30：Network Traffic 匹配失败兜底展示

操作步骤：

1. 启动临时 Bifrost 代理并配置显式 `devtools://` 规则。
2. 使用浏览器通过该代理打开测试页面。
3. 触发一个带自定义 request header 的 fetch 请求，并触发一个动态插入的 `<img>` 标签请求。
4. 在 WebUI DevTools `Network` tab 中模拟 Traffic 映射接口不可用或返回 404。
5. 搜索并打开上述请求的 Network 详情。

预期结果：

- 即使无法匹配 Traffic 详情，Network 列表仍展示发起端采集到的 URL、method、status、type、query 与时间。
- fetch/XHR 请求的 request headers、response headers 仍在 fallback 详情中展示。
- 动态标签资源请求通过 PerformanceResourceTiming 兜底展示 status；无法读取浏览器不开放的 response headers 时，不阻塞基础 Network 可用性。
- fallback 详情在 DevTools 当前页面右侧展示，不跳转到 Traffic 页面。

### TC-CDP-31：同 URL 多页面独立在线与调试隔离

操作步骤：

1. 启动临时 Bifrost 代理并配置显式 `devtools://` 规则。
2. 使用同一个浏览器通过代理打开两个相同 URL 的页面，例如两个 `https://nextoncall.bytedance.net/assistant` 标签页。
3. 保持两个目标页都在线，打开 WebUI DevTools 页面列表。
4. 分别进入两个卡片的详情页，切换 Console / Network / Storage tab。
5. 在第一个目标页写入 `localStorage.setItem('bifrost-tab-marker', 'tab-a')`，在第二个目标页写入 `tab-b`，并分别刷新 DevTools Storage snapshot。

预期结果：

- WebUI DevTools 页面列表展示两个在线页面，而不是被相同 URL 合并成一个。
- 两个页面拥有不同 page id / session，并且同名 URL 不会触发 URL + 时间猜测合并。
- 分别进入两个详情页时 Console、Network、Storage 数据互不串台。
- 如果浏览器复制标签页导致 `sessionStorage` 或 `window.name` 中的 tab id 被克隆，Bifrost broker 也必须识别旧页面仍有 bridge WS 在线，并给新页面派生独立 tab id，不覆盖旧页面。

### TC-CDP-32：client request id 首次绑定与 replay 隔离

操作步骤：

1. 启动临时 Bifrost 代理并配置显式 `devtools://` 规则。
2. 使用浏览器通过代理打开测试页面，并触发一条带 `x-bifrost-client-request-id` 的 fetch/XHR。
3. 记录该 Network 行的 client request id，并查询 `/api/devtools/network/traffic/:client_req_id`。
4. 模拟或执行一次同 client request id 的重放 traffic 写入。
5. 再次查询同一个 client request id。

预期结果：

- `client request id -> traffic id` 映射写入 Traffic DB 的 `traffic_records.devtools_client_req_id`，不保存在 DevTools broker 内存映射中。
- 查询结果始终返回第一条非 replay Traffic 记录。
- Replay 产生的 Traffic 不会覆盖或绑定原始 DevTools client request id。
- 找不到映射时只展示 fallback 详情，不使用 URL + 时间窗口猜测 Traffic。

### TC-CDP-33：fetch/XHR wrapper 与 PerformanceResourceTiming 去重

操作步骤：

1. 启动临时 Bifrost 代理并配置显式 `devtools://` 规则。
2. 使用浏览器通过代理打开测试页面。
3. 在目标页触发 `fetch('/devtools/api/dedupe?case=fetch-performance')`。
4. 打开 WebUI DevTools `Network` tab，搜索 `fetch-performance`。
5. 打开对应 Network 行详情。

预期结果：

- Network 列表中该 fetch 请求只展示一条记录。
- 保留的记录携带 `client_req_id`，可通过 Traffic DB 精确映射到 Traffic 详情。
- PerformanceResourceTiming 兜底行即使带有 `responseStatus`，或因浏览器限制只能默认上报 `GET` method，也不会与 fetch/XHR wrapper 行重复展示。
- 详情中展示浏览器侧 request headers / response headers 与 TrafficDetail 补全信息。

### TC-CDP-34：Service Worker 页面资源 URL 注入安全回归

操作步骤：

1. 启动临时 Bifrost 代理并配置显式 `devtools://` 规则。
2. 使用浏览器通过代理打开一个注册并受 Service Worker 控制的测试页面。
3. 页面动态插入跨域 `<script>` 或 `<img>`，并由 Service Worker 参与路由处理。
4. 打开浏览器控制台和 WebUI DevTools `Network` tab，观察资源加载和 Network 记录。

预期结果：

- bridge 脚本不会给 Service Worker 已控制页面的动态标签资源追加 `__bifrost_client_req_id`。
- bridge 脚本不会给跨域或 protocol-relative 标签资源追加内部 query。
- 目标页面不出现因内部 query 破坏 Service Worker 路由导致的 `no-response` / `AbortError` / `Failed to fetch`。
- 无法精确映射 Traffic 的资源仍通过 PerformanceResourceTiming 兜底展示 URL、method、status/type、query 与 cache hint。

### TC-CDP-35：DevTools broker 忙碌时不阻塞代理主流程

操作步骤：

1. 启动临时 Bifrost 代理并配置显式 `devtools://` 规则。
2. 使用浏览器通过代理打开测试页面，持续产生大量 console/network bridge 事件。
3. 同时反复打开 WebUI DevTools 列表、进入详情、刷新 WebUI 详情页。
4. 使用 curl 请求 `/_bifrost/api/status` 和普通代理目标请求验证代理仍可响应。

预期结果：

- DevTools broker 不因 `pages` 锁竞争阻塞 Admin API 或代理主流程。
- `/_bifrost/api/status` 在压力期间仍能返回响应，不出现连接已建立但一直无响应。
- 高频 console/network 事件在 broker 繁忙时允许丢弃单条事件，但页面、WebUI session 和代理进程不能卡死。
- 多页面或同 URL 多标签页仍保持独立 page id，不通过 URL + 时间猜测合并。

### TC-CDP-36：HTTP fixture API 路由不返回 404

操作步骤：

1. 启动 `e2e-tests/tests/test_devtools_page_bridge_api.sh` 使用的临时 HTTP fixture。
2. 使用真实浏览器经 Bifrost 代理访问 `basic.html`。
3. 观察目标页触发的 `/devtools/api/ping?case=basic`、`/devtools/api/static-resource?case=static-img&foo=tag`。
4. 在 DevTools 测试流程中执行 `fetch('/devtools/api/meta?case=network-meta&foo=bar')`。
5. 继续等待 WebUI DevTools 页面 locator 和 Network 断言完成。

预期结果：

- HTTP fixture 对 `/devtools/api/*` 返回 `200` JSON，包含 `{ "ok": true, "url": "<原请求路径>" }`。
- `site.log` 中不再出现 `/devtools/api/ping`、`/devtools/api/static-resource`、`/devtools/api/meta` 的 404。
- WebUI DevTools 的 locator 等待不因 HTTP fixture API 404 超时。
- `bash e2e-tests/tests/test_devtools_page_bridge_api.sh` 最终通过。

### TC-CDP-37：WebUI DevTools 侧栏入口稳定定位

操作步骤：

1. 启动 `e2e-tests/tests/test_devtools_page_bridge_api.sh`。
2. 完成 HTTP、Service Worker、TLS 全截包 DevTools page_bridge 场景。
3. 脚本打开 `http://127.0.0.1:<proxy_port>/_bifrost/`。
4. 等待侧栏中 `data-testid="app-sidebar-nav-item"` 且 `data-nav-label="DevTools"` 的导航项可见。
5. 读取全部侧栏 `data-nav-label`，确认 `DevTools` 位于 `Scripts` 之后。
6. 点击该稳定导航项进入 DevTools 页面列表。

预期结果：

- WebUI 不再只依赖可见文本 `DevTools` 完成等待和点击。
- 侧栏入口在 CI 浏览器、折叠/图标侧栏或字体渲染延迟下仍可稳定定位。
- 如果入口不可见，失败信息包含当前 URL、document title、侧栏 `data-nav-label` / `data-nav-key` 列表和页面文本摘要，便于定位是 WebUI 未加载、路由错误还是导航项缺失。
- `DevTools` 入口仍位于 `Scripts` 之后。
- 点击后 `devtools-page-list` 可见，后续 DevTools 页面列表和详情断言继续执行。

### TC-CDP-38：CI release artifact 内嵌真实 WebUI

操作步骤：

1. 在 CI `build-e2e` job 中安装 WebUI 依赖。
2. 在 CI `build-cli-macos-aarch64` job 中安装 WebUI 依赖，因为 macOS DevTools shell E2E 下载该 artifact。
3. 执行 release binary 构建，构建命令不得设置 `SKIP_FRONTEND_BUILD=1`。
4. 在 Linux 和 macOS aarch64 shell E2E shard 中下载对应 release binary。
5. 启动 `e2e-tests/tests/test_devtools_page_bridge_api.sh`，脚本打开 `http://127.0.0.1:<proxy_port>/_bifrost/`。
6. 等待侧栏中 `data-testid="app-sidebar-nav-item"` 且 `data-nav-label="DevTools"` 的导航项可见。

预期结果：

- Linux 与 macOS aarch64 DevTools shell E2E 使用的 CI release binary 都返回真实 Bifrost WebUI，而不是 `Frontend not built` 占位页。
- DevTools 侧栏入口可见，`data-nav-label` 列表包含 `Scripts` 和 `DevTools`。
- `DevTools` 入口仍位于 `Scripts` 之后。
- `test_devtools_page_bridge_api.sh` 在 CI shell shard 中可以进入 `devtools-page-list` 并继续完成后续 page_bridge 断言。

### TC-CDP-39：Network snapshot 重放不重复展示同一 bridge 请求

操作步骤：

1. 启动 `e2e-tests/tests/test_devtools_page_bridge_api.sh`。
2. 脚本访问 `basic.html?case=av-cdp-01`，目标页初始化时发起 `fetch('/devtools/api/ping?case=basic')`。
3. 建立 DevTools session WebSocket。
4. 调用 `POST /_bifrost/api/devtools/sessions/:session_id/refresh`，body 为 `{"scope":"full"}`，触发 page bridge 重新上报 full snapshot。
5. 读取 session snapshot 中 `network` 列表，筛选 URL 包含 `/devtools/api/ping?case=basic` 的记录。
6. 查询 `GET /_bifrost/api/devtools/network/traffic/:client_req_id`，再读取对应 Traffic 详情。

预期结果：

- `/devtools/api/ping?case=basic` 在 snapshot `network` 列表中只出现 1 条。
- 该记录包含 `client_req_id`，证明来自 frontend bridge 采集事件，而不是单独的 PerformanceResourceTiming fallback。
- 同一个 `client_req_id` 不会因为 live network 上报与后续 full snapshot 重放而重复进入 Admin broker 缓存。
- 通过 `client_req_id` 能精确映射到 Traffic 记录。
- Traffic 记录的 method、status、URL 与 Network 事件一致。
- Traffic request headers 中不包含内部 `x-bifrost-client-request-id`。

### TC-CDP-40：Network 搜索后点击匹配行展示 fallback 详情

操作步骤：

1. 启动 `e2e-tests/tests/test_devtools_page_bridge_api.sh`。
2. 脚本在目标页发起 Traffic 无法匹配的 bridge-only 请求：`/devtools/api/missing?case=bridge-only-detail&foo=bar`。
3. 在 WebUI DevTools Network tab 的搜索框输入 `bridge-only-detail`。
4. 等待 `data-testid="traffic-row"` 中包含 `bridge-only-detail` 的行可见。
5. 点击该匹配行，而不是点击当前列表首行。
6. 验证右侧 `devtools-network-fallback-detail` 展示 `bridge-only-detail`、`404` 和 query 参数 `foo=bar`。
7. 再发起动态标签资源 fallback 请求 `/devtools/api/static-resource?case=bridge-only-tag-fallback&foo=tagfallback`。
8. 搜索 `bridge-only-tag-fallback`，等待包含该 URL 的匹配行可见后点击该行。
9. 验证右侧 fallback 详情展示 `bridge-only-tag-fallback`、`404` 和 query 参数 `foo=tagfallback`。
10. 再发起可映射 Traffic 详情的动态标签资源 `/devtools/api/parser-resource?case=ui-traffic-enrich&foo=img`。
11. 搜索 `ui-traffic-enrich`，等待包含该 URL 的匹配行可见后点击该行。
12. 验证右侧 `devtools-network-detail` 展示 `traffic-detail`，且详情中包含 `ui-traffic-enrich`。

预期结果：

- 搜索后点击的是包含目标业务 URL 的具体虚拟列表行。
- 不会因虚拟列表复用、旧记录排在第一行或搜索状态更新时序导致点击到其它请求。
- fetch fallback 与动态标签资源 fallback 都在 DevTools Network 当前页右侧展示发起端 metadata。
- Traffic 可映射动态标签资源也按具体匹配行点击，不会因同一文本同时出现在路径列、完整 URL、详情等多个元素中触发 Playwright strict mode 假失败。
- 用例在 macOS shell shard 与 Linux shell shard 下都不依赖列表首行顺序。

### TC-CDP-41：Network 详情等待 Traffic 映射落库完成

操作步骤：

1. 启动 `e2e-tests/tests/test_devtools_page_bridge_api.sh`。
2. 在目标页动态创建 `<img src="/devtools/api/parser-resource?case=ui-traffic-enrich&foo=img">`。
3. 立即点击 WebUI DevTools 详情页 refresh 按钮，并在 Network tab 搜索 `ui-traffic-enrich`。
4. 点击包含 `ui-traffic-enrich` 的 `data-testid="traffic-row"`。
5. 观察右侧 `devtools-network-detail`。

预期结果：

- WebUI 在点击后可短暂显示 TrafficDetail loading 状态，等待 `client_req_id -> traffic id` 映射落库。
- 如果映射在短时间内完成，右侧最终展示 `traffic-detail`，且详情包含 `ui-traffic-enrich`。
- 不会因为 bridge network 事件先于 Traffic 记录落库而立即固定为 fallback 详情。
- 若映射持续不可用，仍保留 fallback 详情路径，不跳转 `/traffic`。

### TC-CDP-42：Console 对象展开点击在 CI 时序下稳定

操作步骤：

1. 启动 `e2e-tests/tests/test_devtools_page_bridge_api.sh`。
2. 让目标页输出 `console.log('bifrost-console-object-ready', { pageId: 'basic', nested: { answer: 42 }, items: ['alpha', 'beta'] })`。
3. 打开 WebUI DevTools 详情页并切换到 `Console` tab。
4. 找到包含 `bifrost-console-object-ready` 的 `devtools-console-row-log`。
5. 点击对象摘要的展开按钮；如果 WebUI snapshot/live 更新时序让属性行尚未出现，按同一行内最后一个展开按钮重试，直到 `nested:` 与 `items:` 同时可见或超时。
6. 点击该行复制按钮并读取剪贴板。

预期结果：

- Console 行默认展示 `Object { ... }` 摘要。
- 展开后同一行内稳定显示 `nested:` 与 `items:`，不会因一次点击后 UI 重渲染导致 locator 假超时。
- `nested:` 属性行缩进在对象摘要下方，对象展开树仍保持对齐。
- 剪贴板 raw 内容包含 `bifrost-console-object-ready` 与 `"nested"`。

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
- 2026-04-30：通过。按稳定性 review 结果补充 TC-CDP-16，验证服务端 page bridge seq 去重与 live channel 有界队列保护，并用重建后的 release 二进制复测真实 DevTools 端到端流程。执行命令：`source ~/.zshrc && cargo test -p bifrost-admin devtools::tests::test_page_bridge --all-features`，输出 `6 passed; 0 failed`，包含 `test_page_bridge_seq_dedupes_replayed_messages` 与 `test_page_bridge_live_queues_are_bounded`；随后执行 `source ~/.zshrc && cargo build --release --bin bifrost` 和 `source ~/.zshrc && SKIP_BUILD=true e2e-tests/tests/test_devtools_page_bridge_api.sh`，输出 `DevTools custom bridge E2E passed: WS-only page bridge, lightweight WebUI session snapshot refresh, elements/network/storage/console, Traffic-style network table, structured console object expansion/copy, UI search/layout, page switching, reload recovery, and Chrome frontend cleanup passed`。
- 2026-04-30：通过。针对 CI shell E2E part3 中 fixture `http.server` 被 SIGTERM 后未收敛的问题，补充并执行 TC-CDP-17。执行命令：`source ~/.zshrc && TEST_ROOT="$(mktemp -d /tmp/codex-devtools-human.XXXXXX)" SITE_PORT=$((10000 + RANDOM % 5000)) SKIP_BUILD=true bash e2e-tests/tests/test_devtools_page_bridge_api.sh`，重复 3 次并保存日志到 `/tmp/codex-fix-e2e/gate-1.log`、`/tmp/codex-fix-e2e/gate-2.log`、`/tmp/codex-fix-e2e/gate-3.log`。预期输出：`DevTools custom bridge E2E passed: WS-only page bridge, lightweight WebUI session snapshot refresh, elements/network/storage/console, Traffic-style network table, structured console object expansion/copy, UI search/layout, page switching, reload recovery, and Chrome frontend cleanup passed`，且 cleanup 只在脚本结尾停止自有 `http.server` PID。
- 2026-04-30：通过。补充并执行 TC-CDP-18，验证 DevTools Network 行点击后不会跳转 `/traffic`，而是在当前 DevTools Network tab 右侧内嵌展示复用的 TrafficDetail 详情组件；找不到 Traffic 记录时，右侧 fallback 详情展示 page bridge 已上报的 URL 等信息。执行命令：`source ~/.zshrc && SKIP_BUILD=true e2e-tests/tests/test_devtools_page_bridge_api.sh`。输出：`DevTools custom bridge E2E passed: WS-only page bridge, lightweight WebUI session snapshot refresh, elements/network/storage/console, Traffic-style network table with inline detail, structured console object expansion/copy, UI search/layout, page switching, reload recovery, and Chrome frontend cleanup passed`。
- 2026-04-30：通过。补充并执行 TC-CDP-19，验证顶层纯文本 console.log 不再渲染为红色对象字符串，对象展开树缩进对齐在对象摘要下方。执行命令：`source ~/.zshrc && SKIP_BUILD=true e2e-tests/tests/test_devtools_page_bridge_api.sh`。输出包含 `Traffic-style network table with inline detail`，并通过 AV-CDP-36 颜色与布局断言。
- 2026-04-30：通过。补充并执行 TC-CDP-20 / TC-CDP-21 / TC-CDP-22 / TC-CDP-23，验证 DevTools 页面、workspace、Elements、Network、Storage、Console 与全屏 JavaScript 编辑器跟随 WebUI dark theme；选择在线页面后 URL 包含 page id，刷新 WebUI 后自动恢复详情页；Network 列表以前端采集为准并通过 `x-bifrost-client-request-id` 映射 Traffic 详情，`x-bifrost-client-request-id` 不出现在 Traffic request headers；Storage 在 400+ localStorage/sessionStorage 数据下使用虚拟列表，tab 切换在 2500ms 内完成，搜索后行内编辑、复制、删除仍可用。执行命令：`source ~/.zshrc && cargo build --release --bin bifrost`，随后执行 `source ~/.zshrc && SKIP_BUILD=true e2e-tests/tests/test_devtools_page_bridge_api.sh`。输出：`DevTools custom bridge E2E passed: WS-only page bridge, lightweight WebUI session snapshot refresh, elements/network/storage/console, Traffic-style network table with inline detail, structured console object expansion/copy, UI search/layout, page switching, reload recovery, and Chrome frontend cleanup passed`。
- 2026-04-30：通过。补充并执行 TC-CDP-24 / TC-CDP-25，验证 Console 支持浏览器标准 `%c` 样式格式化，不显示 `%c` 和样式参数文本，并应用白名单内联样式；验证 Network 浏览器侧采集 status、query、request headers、response headers 和 cache hint，默认不采集 body，并在 DevTools 右侧详情展示。执行命令：`source ~/.zshrc && cargo build --release --bin bifrost`，随后执行 `source ~/.zshrc && SKIP_BUILD=true e2e-tests/tests/test_devtools_page_bridge_api.sh`。输出：`DevTools custom bridge E2E passed: WS-only page bridge, lightweight WebUI session snapshot refresh, elements/network/storage/console, Traffic-style network table with inline detail, structured console object expansion/copy, UI search/layout, page switching, reload recovery, and Chrome frontend cleanup passed`。
- 2026-04-30：通过。补充并执行 TC-CDP-26，验证 Elements 目标页鼠标拾取元素通过 WS 回传 node id，WebUI 自动展开并选中 DOM row，目标页 overlay 信息卡展示节点名称、尺寸、Color、Font、Padding、Margin，并修复长 Font 内容导致信息卡异常变窄的问题。执行命令：`source ~/.zshrc && cargo build --release --bin bifrost`，随后执行 `source ~/.zshrc && SKIP_BUILD=true e2e-tests/tests/test_devtools_page_bridge_api.sh`。
- 2026-04-30：通过。补充并执行 TC-CDP-27 / TC-CDP-28 / TC-CDP-29 / TC-CDP-30 / TC-CDP-33，验证标签资源通过安全的同源内部 query id 精准映射 Traffic，TLS 全截包浏览器代理下 Network 与 Traffic 可通过 `x-bifrost-client-request-id` 精确匹配，WebUI DevTools 详情刷新不会触发目标页 reload 或业务请求，Traffic 匹配失败时仍展示发起端基础 Network 信息，且同一 fetch 不会同时展示 hook 行和 PerformanceResourceTiming fallback 行。执行命令：`source ~/.zshrc && SKIP_BUILD=true e2e-tests/tests/test_devtools_page_bridge_api.sh`。输出：`DevTools custom bridge E2E passed: WS-only page bridge, lightweight WebUI session snapshot refresh, elements/network/storage/console, Traffic-style network table with inline detail, structured console object expansion/copy, UI search/layout, page switching, reload recovery, and Chrome frontend cleanup passed`。
- 2026-04-30：通过。补充并执行 TC-CDP-34 / TC-CDP-35，验证 Service Worker / 跨域标签资源不被内部 query 污染，以及 DevTools broker 在页面高频上报或锁竞争时不会阻塞代理主流程。执行命令：`source ~/.zshrc && SKIP_BUILD=true e2e-tests/tests/test_devtools_page_bridge_api.sh`。输出：`DevTools custom bridge E2E passed: WS-only page bridge, lightweight WebUI session snapshot refresh, elements/network/storage/console, Traffic-style network table with inline detail, structured console object expansion/copy, UI search/layout, page switching, reload recovery, and Chrome frontend cleanup passed`。
- 2026-04-30：通过。补充并执行 TC-CDP-36，验证 HTTP fixture 的 `/devtools/api/*` 路由返回 200 JSON，避免 `basic.html` 的业务 fetch/tag 资源请求在 Python 静态服务器下返回 404 并导致 DevTools locator 等待超时。执行命令：`bash e2e-tests/tests/test_devtools_page_bridge_api.sh`。输出：`DevTools custom bridge E2E passed: WS-only page bridge, lightweight WebUI session snapshot refresh, elements/network/storage/console, Traffic-style network table with inline detail, structured console object expansion/copy, UI search/layout, page switching, reload recovery, and Chrome frontend cleanup passed`。
- 2026-04-30：通过。补充并执行 TC-CDP-37，验证 WebUI DevTools 侧栏入口使用稳定 `data-testid="app-sidebar-nav-item"` + `data-nav-label="DevTools"` 定位，不再依赖可见文本等待；同时验证 DevTools 入口仍位于 Scripts 之后，点击后进入 `devtools-page-list`。执行命令：`bash -n e2e-tests/tests/test_devtools_page_bridge_api.sh`，随后执行 `bash e2e-tests/tests/test_devtools_page_bridge_api.sh`。输出：`DevTools custom bridge E2E passed: WS-only page bridge, lightweight WebUI session snapshot refresh, elements/network/storage/console, Traffic-style network table with inline detail, structured console object expansion/copy, UI search/layout, page switching, reload recovery, and Chrome frontend cleanup passed`。
- 2026-04-30：通过。补充并执行 TC-CDP-38，验证 CI `build-e2e` release artifact 构建命令不再设置 `SKIP_FRONTEND_BUILD=1`，从而让 shell shard 下载的 release binary 内嵌真实 WebUI，避免 DevTools 入口定位时只看到 `Frontend not built` 占位页。执行命令：`pnpm --dir web run build`，随后执行 `cargo build --release --bin bifrost` 和 `SKIP_BUILD=true bash e2e-tests/tests/test_devtools_page_bridge_api.sh`。
- 2026-04-30：通过。继续执行 TC-CDP-38 的 macOS aarch64 artifact 回归验证，确认 `build-cli-macos-aarch64` 安装 WebUI 构建依赖且不再设置 `SKIP_FRONTEND_BUILD=1`，避免 macOS shell shard 下载占位 WebUI。执行命令：`pnpm --dir web run build`，随后执行 `cargo build -p bifrost-cli --release --target aarch64-apple-darwin`，再用临时 `BIFROST_DATA_DIR` 和随机非 9900 端口启动 `target/aarch64-apple-darwin/release/bifrost start --unsafe-ssl --no-system-proxy`，通过 `curl http://127.0.0.1:<port>/_bifrost/` 验证返回真实 WebUI 且不包含 `Frontend not built`。
- 2026-05-01：通过。补充并执行 TC-CDP-39，验证 Admin broker 对 live network 与 full snapshot replay 使用同一套 `client_req_id` 去重逻辑，避免 CI 中 `/devtools/api/ping?case=basic` 概率性重复展示。执行命令：`source ~/.zshrc && cargo test -p bifrost-admin devtools::tests::test_page_bridge_network_cache --all-features`，输出 `2 passed; 0 failed`；随后执行 `source ~/.zshrc && bash e2e-tests/tests/test_devtools_page_bridge_api.sh`。输出：`DevTools custom bridge E2E passed: WS-only page bridge, lightweight WebUI session snapshot refresh, elements/network/storage/console, Traffic-style network table with inline detail, structured console object expansion/copy, UI search/layout, page switching, reload recovery, and Chrome frontend cleanup passed`。
- 2026-05-01：通过。补充并执行 TC-CDP-40，验证 DevTools Network 搜索后点击包含目标业务 URL 的具体虚拟列表行，避免 macOS shell shard 中 fallback detail 点击到旧首行。首次执行因当前 worktree 的 `mise.toml` 未信任导致 fixture server 未启动；执行 `mise trust <USER_HOME>/work/github/bifrost-devtools-avcdp39/mise.toml` 后重跑通过。执行命令：`source ~/.zshrc && SKIP_BUILD=true bash e2e-tests/tests/test_devtools_page_bridge_api.sh`。输出：`DevTools custom bridge E2E passed: WS-only page bridge, lightweight WebUI session snapshot refresh, elements/network/storage/console, Traffic-style network table with inline detail, structured console object expansion/copy, UI search/layout, page switching, reload recovery, and Chrome frontend cleanup passed`。
- 2026-05-01：通过。根据 CI #827 Linux shell shard 3 新失败补充 TC-CDP-40 的 Traffic 可映射动态标签资源回归，避免 `ui-traffic-enrich` 文本同时出现在多处时触发 Playwright strict mode，也避免依赖行内状态列时序。执行命令：`source ~/.zshrc && SKIP_BUILD=true bash e2e-tests/tests/test_devtools_page_bridge_api.sh`。输出：`DevTools custom bridge E2E passed: WS-only page bridge, lightweight WebUI session snapshot refresh, elements/network/storage/console, Traffic-style network table with inline detail, structured console object expansion/copy, UI search/layout, page switching, reload recovery, and Chrome frontend cleanup passed`。
- 2026-05-01：通过。根据 CI #829 macOS aarch64 shell shard 3 新失败补充 TC-CDP-41，验证点击动态标签资源 Network 行时，WebUI 会短暂等待 `client_req_id -> traffic id` 映射落库，不因 page bridge network 事件先到而概率性固定为 fallback 详情。执行命令：`source ~/.zshrc && bash e2e-tests/tests/test_devtools_page_bridge_api.sh`。输出：`DevTools custom bridge E2E passed: WS-only page bridge, lightweight WebUI session snapshot refresh, elements/network/storage/console, Traffic-style network table with inline detail, structured console object expansion/copy, UI search/layout, page switching, reload recovery, and Chrome frontend cleanup passed`。
- 2026-05-06：通过。根据 CI `25391191157` macOS aarch64 shell shard 3 的 Console 对象展开一次性等待超时，补充并执行 TC-CDP-42。执行命令：`bash -n e2e-tests/tests/test_devtools_page_bridge_api.sh`；随后执行 `pnpm --dir web install --frozen-lockfile`、`pnpm --dir web run build`、`cargo build --release --bin bifrost` 和 `SKIP_BUILD=true bash e2e-tests/tests/test_devtools_page_bridge_api.sh`。输出：`DevTools custom bridge E2E passed: WS-only page bridge, lightweight WebUI session snapshot refresh, elements/network/storage/console, Traffic-style network table with inline detail, structured console object expansion/copy, UI search/layout, page switching, reload recovery, and Chrome frontend cleanup passed`。
