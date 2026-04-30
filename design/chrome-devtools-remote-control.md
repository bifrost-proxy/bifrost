# Bifrost DevTools Remote Control

## 目标

当用户显式配置 `devtools://` 规则后，所有经过 Bifrost 代理且命中规则的页面都可以被 WebUI 发现并调试。这个能力覆盖移动端 Safari 等不能或不愿开启系统调试能力的场景，默认依赖 Bifrost 注入的 `page_bridge`，而不是设备系统调试接口。

WebUI 能力范围：

- Elements：展示目标页 DOM tree / DOM snapshot，支持选择节点并在目标页高亮，手动刷新后可看到 DOM 结构变化。
- Network：复用 Traffic 页面虚拟列表风格展示 bridge 捕获到的资源、fetch、XHR 等网络事件，包含序号、状态点、protocol、method、status、host、path、type、size、time 等列。
- Cookies / LocalStorage / SessionStorage：三个独立一级 tab 展示对应存储区域；支持新增、编辑、复制、删除，保存/删除通过 `storage.set` / `storage.delete` semantic command 经由 page bridge 在目标页执行实际写入。
- Console：展示完整页面 console 日志级别；支持多行输入和表达式执行；对象、数组、DOM 节点、Error 等参数以结构化值传输，展示 Chrome-like 摘要，点击后按层级展开并支持复制原始内容。

## 非目标

- 不集成官方 Chrome DevTools frontend。
- 不下载 `chrome-devtools-frontend` npm 包，不把其 tarball 或编译产物放入仓库、安装包或数据目录。
- 不提供 Chrome DevTools frontend 静态资源接口。
- 不提供由 Bifrost 启动系统 Chrome 的入口。
- 不宣传完整 Chrome DevTools parity；`page_bridge` 是降级调试能力，能力边界由 capability matrix 明确表达。

## 架构

```mermaid
sequenceDiagram
  participant Browser as Proxied Page
  participant Proxy as Bifrost Proxy
  participant Admin as Admin DevTools Broker
  participant WebUI as Bifrost WebUI DevTools

  Browser->>Proxy: HTTP document request
  Proxy->>Proxy: explicit devtools:// rule matched
  Proxy-->>Browser: inject page_bridge script
  Browser->>Admin: WS /api/devtools/bridge/:page_id/ws
  Browser->>Admin: hello / console / network / eval_result messages
  WebUI->>Admin: GET /api/devtools/pages?online=true
  WebUI->>Admin: POST /api/devtools/sessions
  WebUI->>Admin: GET /api/devtools/sessions/:id/ws
  Admin-->>Browser: snapshot_request(scope)
  Browser-->>Admin: scoped hello snapshot
  Admin-->>WebUI: live snapshot / console / network / disconnected
  WebUI->>Admin: POST /api/devtools/sessions/:id/refresh scope
  WebUI->>Admin: POST /api/devtools/sessions/:id/commands runtime.evaluate
  Admin-->>Browser: eval / overlay command messages
  Browser-->>Admin: eval result
```

### Bridge 通信机制

主通道使用 WebSocket (`/api/devtools/bridge/:page_id/ws`)。页面通过同一条连接上报 hello / console / network / eval_result / close，Admin 通过同一条连接下发 eval / overlay / snapshot_request 命令。HTTP POST bridge 端点作为兼容回退保留（hello / console / network / eval-next / eval-result / overlay-next / close）。

页面上报消息携带递增 `seq`，Admin 对每个 page 保留最近一段 `seq` 并去重，确保 WS reconnect 重放 inflight 消息时不会产生重复日志或重复网络记录。

### WebUI Session 通信

WebUI 与 Admin 建立 session WebSocket (`/api/devtools/sessions/:id/ws`)。WebUI 打开详情或切换 tab 时，由 Admin 通过目标页 bridge WS 发起 scoped `snapshot_request`，目标页立即重新读取被请求模块并推送给 WebUI。

Admin 只保留页面发现、session 路由、短期状态和必要的小型映射（client request id 到 traffic id），不保存完整 DOM / Network / Storage / Console 历史数据；完整可恢复数据以目标页面内存为主。Admin 到目标页、Admin 到 WebUI 的 live channel 使用有界队列（`mpsc::Sender`），慢消费者或断连时移除 stale sender。

WebUI 断开 session WS 时 Admin 删除对应 session sender；目标页 bridge WS 断开时 Admin 立刻向已连接 WebUI session 推送 `Disconnected`。

## 核心类型

```rust
pub struct BrowserDebugBroker {
    pages: RwLock<HashMap<String, DebugPage>>,
    sessions: RwLock<HashMap<String, DebugSession>>,
    eval_next_id: AtomicU64,
    eval_pending: RwLock<HashMap<String, Vec<BridgeEvalCommand>>>,
    eval_results: RwLock<HashMap<u64, Result<Value, String>>>,
    overlay_pending: RwLock<HashMap<String, Vec<BridgeOverlayCommand>>>,
    bridge_senders: RwLock<HashMap<String, mpsc::Sender<BridgeServerMessage>>>,
    session_senders: RwLock<HashMap<String, mpsc::Sender<DevtoolsLiveMessage>>>,
    bridge_seen_seqs: RwLock<HashMap<String, VecDeque<u64>>>,
    client_req_traffic: RwLock<HashMap<String, String>>,
    evaluate_audit: RwLock<VecDeque<EvaluateAuditRecord>>,
}

pub enum DebugAdapterKind { PageBridge }
pub enum DebugFidelity { Fallback }
pub enum DebugPageState { Candidate, Discoverable, FallbackAttached, Stale, Denied }
pub enum DevtoolsMode { Read, Control }

pub enum BridgeServerMessage {
    Eval { command: BridgeEvalCommand },
    Overlay { command: BridgeOverlayCommand },
    SnapshotRequest { scope: Option<String> },
}

pub enum DevtoolsLiveMessage {
    Snapshot { snapshot: Value },
    Console { message: ConsoleMessage },
    Network { event: NetworkEvent },
    Disconnected { page_id: String, reason: String },
}
```

## 后端接口

### DevTools 页面与会话

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/_bifrost/api/devtools/pages?online=true` | 列出可调试页面 |
| POST | `/_bifrost/api/devtools/sessions` | 创建调试 session（body: `{page_id}`） |
| GET | `/_bifrost/api/devtools/sessions/:id/snapshot` | 获取 session 元信息快照 |
| POST | `/_bifrost/api/devtools/sessions/:id/refresh` | 按 scope 请求目标页刷新（body: `{scope}`） |
| GET | `/_bifrost/api/devtools/sessions/:id/ws` | Session live-push WebSocket |
| POST | `/_bifrost/api/devtools/sessions/:id/commands` | 发送命令（dom.snapshot / dom.highlight / dom.hide_highlight / console.messages / storage.set / storage.delete / runtime.evaluate） |

### Bridge（页面注入脚本通信）

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/_bifrost/api/devtools/bridge/:page_id/ws` | Bridge WebSocket（主通道） |
| POST | `/_bifrost/api/devtools/bridge/:page_id/hello` | Bridge 握手（兼容回退） |
| POST | `/_bifrost/api/devtools/bridge/:page_id/console` | Console 事件上报（兼容回退） |
| POST | `/_bifrost/api/devtools/bridge/:page_id/network` | Network 事件上报（兼容回退） |
| POST | `/_bifrost/api/devtools/bridge/:page_id/eval-next` | 轮询待执行 eval 命令（兼容回退） |
| POST | `/_bifrost/api/devtools/bridge/:page_id/eval-result` | 返回 eval 执行结果（兼容回退） |
| POST | `/_bifrost/api/devtools/bridge/:page_id/overlay-next` | 轮询待执行 overlay 命令（兼容回退） |
| POST | `/_bifrost/api/devtools/bridge/:page_id/close` | 页面关闭通知（兼容回退） |

### CDP 兼容层

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/_bifrost/api/devtools/cdp/json/version` | CDP 版本信息 |
| GET | `/_bifrost/api/devtools/cdp/json/list` | CDP target 列表 |
| GET | `/_bifrost/api/devtools/cdp/:page_id` | CDP WebSocket（实现主要 CDP 方法子集） |

CDP WebSocket 仅允许 `localhost`/`127.0.0.1` 来源连接（或通过 `BIFROST_DEVTOOLS_ALLOWED_ORIGINS` 环境变量放行）。

已实现的 CDP 方法：`Browser.getVersion`、`Target.getTargetInfo`、`Target.getTargets`、`DOM.getDocument`、`DOM.getFlattenedDocument`、`CSS.getMatchedStylesForNode`、`CSS.getComputedStyleForNode`、`DOMStorage.getDOMStorageItems`、`Runtime.evaluate`、`Overlay.highlightNode`、`Overlay.hideHighlight`、`Page.getFrameTree` 等；其余方法返回空成功响应（stub）。

### 辅助接口

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/_bifrost/api/devtools/network/traffic/:client_req_id` | 根据 bridge 上报的 client request id 查找对应的 traffic id |
| GET | `/_bifrost/api/devtools/audit/evaluate` | 查询 evaluate 审计记录（支持 `?limit=` / `?since=`） |

## devtools:// 规则

### 协议定义

在 `crates/bifrost-core/src/protocol.rs` 中定义 `Protocol::DevTools`。

### 规则值解析

```rust
pub struct DevtoolsRule {
    pub mode: DevtoolsMode,           // Read | Control (默认 Control)
    pub inject: DevtoolsInjectMode,   // Auto | Bridge | Off (默认 Auto)
    pub deny: bool,                   // 默认 false
    pub evaluate_allowlist: Vec<String>,
    pub raw_value: String,
}
```

裸 `devtools://` 不需要任何 value；默认启用 Elements / Network / Cookies / LocalStorage / SessionStorage / Console 全部能力，包括 Storage 修改和 Runtime evaluate。

`mode=read` / `mode=control` / `evaluate_allowlist` 只作为高级限制能力保留，不出现在默认智能提示中。

### 注入决策

`devtools_bridge_requested(rules)` 检查规则是否命中且未 deny 且 inject 非 Off，满足条件时 `maybe_inject_devtools_bridge_html` 注册 page candidate 并在 HTML 响应中注入 bridge 脚本。

### 请求追踪

注入的 bridge 脚本为页面发出的同源 fetch/XHR 请求添加 `x-bifrost-client-request-id` header，代理通过 `take_devtools_client_req_id` 提取并通过 `bind_devtools_client_req_traffic` 映射到 traffic id。该 header 是 DevTools 内部映射字段，代理必须在转发和记录前剥离，不能进入目标服务端或 Traffic request headers。Network 列表以 page bridge 前端采集事件为唯一数据源；Traffic 只作为点击行后的详情补全来源，不能反向混入列表。Performance resource timing 只作为兜底采集，若同一 URL/method 已有带 client request id 的 fetch/XHR 事件，必须去重并优先保留带 id 的事件。

## WebUI 设计

### 组件结构

| 组件 | 文件 | 职责 |
|------|------|------|
| DevTools 页面 | `web/src/pages/DevTools/index.tsx` | 页面列表 + 详情容器 |
| ElementsPanel | `web/src/pages/DevTools/components/ElementsPanel.tsx` | DOM tree 展示、节点高亮、搜索 |
| NetworkPanel | `web/src/pages/DevTools/components/NetworkPanel.tsx` | 网络事件虚拟列表，复用 VirtualTrafficTable |
| ConsolePanel | `web/src/pages/DevTools/components/ConsolePanel.tsx` | Console 日志 + 表达式执行（含 Monaco editor） |
| StoragePanel | `web/src/pages/DevTools/components/StoragePanel.tsx` | Cookies / LocalStorage / SessionStorage 表格编辑 |
| shared | `web/src/pages/DevTools/components/shared.tsx` | HighlightedText、filterBySearch 等工具函数 |

### API Client

`web/src/api/devtools.ts` 导出：`listDevtoolsPages`、`openDevtoolsSession`、`getDevtoolsSnapshot`、`requestDevtoolsSnapshotRefresh`、`buildDevtoolsSessionWsUrl`、`findTrafficForDevtoolsRequest`、`sendDevtoolsCommand`。

### 交互规范

- 页面列表只展示命中显式 `devtools://` 规则且已完成 bridge `hello` 的在线页面；`Candidate` / `Stale` / `Denied` 状态不可见。
- 页面详情页头：title 右侧保留跳转 Traffic 入口；URL 在 title 下方展示，hover 后出现复制按钮。下方 DevTools content 区域占满剩余高度。
- 多页面切换重新打开对应 page session 并刷新 snapshot。
- 同一 tab 刷新或导航时，Broker 通过稳定 `tab_id` 把旧 session 迁移到新 page id，旧 page 标记为 `Stale` 后隐藏。
- Elements：DOM tree 可展开/折叠，标签名/属性名/属性值分色。首个可见节点从 `<html>` 开始，`#document` 仅作为容器。纯空白 text node 过滤。超长属性/文本默认展示 ≤120 字符预览，点击弹窗查看完整内容。节点点击调用 `dom.highlight` 在目标页显示 overlay。
- Network：复用 Traffic 页面虚拟列表结构和视觉风格。列表只展示 page bridge 前端采集事件；点击行通过 `x-bifrost-client-request-id` 映射到 Traffic 详情，在 DevTools 当前页面内展示 TrafficDetail，不跳转到 `/traffic` 路由。找不到 Traffic 记录时展示前端已上报的 URL / method / status / type / client request id / query / request headers / response headers / cache hint 等 metadata。默认不采集 request body 或 response body。
- Cookies / LocalStorage / SessionStorage：key/value 表格展示，行内新增/编辑/复制/删除。编辑默认可用，不受 mode 限制。
- Console：日志区域在上方滚动，底部多行输入框固定。每条行展示低对比度毫秒级时间。Object/Array 默认摘要，点击展开。支持浏览器标准 `%c` 样式格式化并隐藏样式参数文本。全屏编辑入口使用 JavaScript Monaco editor。执行代码作为 `input` 行、结果作为 `result` 行展示。目标页 JS 抛错以成功 HTTP 响应返回异常详情，WebUI 展示真实 JS error。
- 面板 tab 右侧提供当前模块搜索框。Elements 搜索自动展开并选中匹配节点；其他面板搜索直接过滤列表并高亮匹配文本。
- 手动刷新按钮重新读取 scoped snapshot。
- WebUI 不做高频全局轮询。页面列表低频刷新或用户点击 refresh；详情页通过 session WS 接收增量推送；隐藏 tab 销毁组件。

## page_bridge 注入脚本

实现于 `crates/bifrost-proxy/src/proxy/http/handler.rs` 中的 `devtools_bridge_script(page_id, token)` 函数。

功能：
1. 建立 WebSocket 连接到 `/_bifrost/api/devtools/bridge/{page_id}/ws`
2. 发送 `hello`（token、tab_id、title、URL、user_agent、DOM snapshot、storage、console、network）
3. 实时上报 console messages、network events
4. 接收并执行 eval / overlay / snapshot_request 命令
5. 返回 eval 执行结果
6. 监听 DOM mutation 和 storage 变化
7. 暴露 `window.__BIFROST_DEVTOOLS_BRIDGE__` shim 对象

`insert_devtools_bridge_script(html, script)` 将脚本插入 `<head>` 或 `<html>` 后，或作为前缀。

## 安全与权限

- `devtools://` 必须由规则显式配置，不允许对所有代理页面默认开启。
- 裸 `devtools://` 默认启用全部能力。
- 如果显式配置 `evaluate_allowlist`，evaluate 需匹配规则中的 allowlist。
- audit 记录保留表达式 sha256、预览、目标 URL、page id、是否被 allowlist 拒绝等信息（容量默认 1000 条，可通过 `BIFROST_DEVTOOLS_EVALUATE_AUDIT_CAPACITY` 环境变量调整）。
- bridge token 只由代理注入脚本持有，页面伪造 postMessage 或猜 token 不应改变 Admin 侧页面状态。

## 关键文件

| 文件 | 职责 |
|------|------|
| `crates/bifrost-core/src/protocol.rs` | `Protocol::DevTools` 定义 |
| `crates/bifrost-proxy/src/server.rs` | `DevtoolsRule` / `DevtoolsMode` / `DevtoolsInjectMode` 结构体 |
| `crates/bifrost-proxy/src/proxy/http/handler.rs` | 规则解析、注入决策、bridge 脚本生成 |
| `crates/bifrost-admin/src/devtools.rs` | `BrowserDebugBroker` 核心逻辑 |
| `crates/bifrost-admin/src/handlers/devtools.rs` | HTTP/WebSocket 路由处理 |
| `crates/bifrost-admin/src/router.rs` | `/api/devtools` 路由入口 |
| `web/src/pages/DevTools/` | WebUI DevTools 页面及子组件 |
| `web/src/api/devtools.ts` | 前端 API client |

## 测试

### 单元测试

- `BrowserDebugBroker::cdp_targets` 不序列化 `systemChromeFrontendUrl`。
- `BrowserDebugBroker::list_debuggable_pages` 隐藏 `Candidate` / `Stale` 页面。
- `BrowserDebugBroker::bridge_hello` 使用 `tab_id` 将刷新后的新 page id 迁移到已有 session。
- `BrowserDebugBroker::command("runtime.evaluate")` 验证裸 `devtools://` 默认可执行，并覆盖兼容模式和 allowlist。

### E2E 测试

脚本：`e2e-tests/tests/test_devtools_page_bridge_api.sh`

规则 fixtures：
- `e2e-tests/rules/devtools/page_bridge_basic.txt`
- `e2e-tests/rules/devtools/page_bridge_control.txt`
- `e2e-tests/rules/devtools/page_bridge_deny.txt`
- `e2e-tests/rules/devtools/page_bridge_control_allowlist.txt`

验证点：
- 启动临时 Bifrost 代理（临时 `BIFROST_DATA_DIR` + `--no-system-proxy`）
- 配置显式 `devtools://` 规则
- 验证 bridge 注入、页面发现、session snapshot
- 验证 WebUI 六个面板功能（Elements / Network / Cookies / LocalStorage / SessionStorage / Console）
- 验证 Elements tree 首个可见节点为 `<html>`，无空文本 DOM 行，超长属性/文本 ≤120 字符预览
- 验证 Elements 点击节点后目标页出现 highlight overlay
- 验证 Network 使用虚拟列表结构，列表以前端采集事件为准，不重复展示 performance/Traffic 派生记录；点击行在 DevTools 内复用 TrafficDetail 展示；`x-bifrost-client-request-id` 可映射到 Traffic id 且不会进入 Traffic request headers；浏览器侧 metadata 包含 status、query、request headers、response headers 且默认不采集 body
- 验证 Storage 行内新增/编辑/删除后目标页真实读到新值
- 验证 Console 执行代码展示 input/result 行，JS 抛错展示远端异常详情，`%c` 样式格式化按浏览器 Console 语义渲染
- 验证 Bridge 主要经由 WebSocket 通信
- 验证页面切换后显示对应 DOM；目标页刷新后无需退出重进
- 验证 fetch/prefetch HTML 不产生幽灵目标
- 验证 syntax API 将 `devtools://` 暴露为无参数协议

### 真实场景测试

- 更新并执行 `human_tests/chrome-devtools-remote-control.md`
- 同步更新 `human_tests/readme.md` 索引

## 校验要求

提交前必须通过：

- `pnpm --dir web run build`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- 相关 E2E：`e2e-tests/tests/test_devtools_page_bridge_api.sh`
