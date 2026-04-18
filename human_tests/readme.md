# Bifrost 真实场景测试用例（Agent 自主执行）

本目录存储自然语言描述的测试用例文档，用于指导 Agent 自主进行真实场景测试。每个测试文件对应一个功能模块。

**核心定位**：`human_tests/` 是 Agent 驱动真实场景测试的标准载体。每次需求开发结束后，必须先在此目录创建或更新测试用例文档，再由 Agent 按文档逐条自主执行测试。

## 目录结构

### CLI 命令测试

| 文件 | 功能模块 | 测试用例数 | 说明 |
|------|---------|-----------|------|
| [cli-start-stop-status.md](./cli-start-stop-status.md) | CLI 服务管理 | 25 | start/stop/status 命令，含守护进程、自定义端口、TLS 选项、规则加载、SOCKS5、LAN 访问、代理认证，以及 status 活跃规则摘要 |
| [cli-start-advanced.md](./cli-start-advanced.md) | CLI Start 高级参数 | 33 | TLS 拦截域名/应用排除与白名单、系统代理（默认启用、--no-system-proxy 禁用、互斥校验）、CLI 代理环境变量、访问控制模式、Badge 注入、证书检查跳过、日志配置 |
| [cli-rule-management.md](./cli-rule-management.md) | CLI 规则管理 | 45 | rule 子命令全覆盖：list/add/show/get/update/enable/disable/delete/rename/reorder/active/sync，含过滤器和 lineProps |
| [cli-rule-list-legacy-skip.md](./cli-rule-list-legacy-skip.md) | CLI `rule list` legacy 容错 | 1 | 损坏 legacy 规则文件跳过回归，确认仅列出本地可用规则 |
| [cli-traffic-search.md](./cli-traffic-search.md) | CLI 流量与搜索 | 36 | traffic list/get/search/clear 命令，含多维度过滤器、搜索范围控制、交互式搜索 |
| [cli-ca-cert.md](./cli-ca-cert.md) | CLI CA 证书管理 | 12 | ca generate/export/info/install 命令，含强制重新生成、指定路径导出、证书格式验证 |
| [cli-values-scripts.md](./cli-values-scripts.md) | CLI Values 与 Scripts | 30 | value list/add/show/set/update/delete/import 和 script list/add/show/get/update/run/rename/delete |
| [cli-whitelist.md](./cli-whitelist.md) | CLI 白名单管理 | 31 | whitelist 全子命令：list/add/remove/allow-lan/status/mode/pending/approve/reject/clear-pending/add-temporary/remove-temporary |
| [cli-admin.md](./cli-admin.md) | CLI Admin 管理 | 14 | admin remote status/enable/disable、admin passwd、admin revoke-all、admin audit |
| [cli-config.md](./cli-config.md) | CLI 配置管理 | 22 | config show/get/set/add/remove/reset/clear-cache/disconnect/export/connections/memory |
| [cli-system-proxy.md](./cli-system-proxy.md) | CLI 系统代理 | 10 | system-proxy status/enable/disable，含自定义 host/port/bypass |
| [cli-group.md](./cli-group.md) | CLI Group 管理 | 14 | group list/show、group rule list/show/add/update/enable/disable/delete |
| [cli-import-export.md](./cli-import-export.md) | CLI 导入导出与杂项 | 26 | export/import、metrics、sync、version-check、upgrade、completions、install-skill，含 version-check 空输出与 install-skill 更多 agent 兼容回归验证，以及 version-check redirect 优先与 HTML highlights 降级验证 |
| [port-conflict-restart.md](./port-conflict-restart.md) | 端口冲突检测与自动重启 | 5 | 端口占用检测、进程信息显示、交互式终止确认、--yes 自动确认、PID 检测兼容性 |
| [cli-log-output-default.md](./cli-log-output-default.md) | CLI 日志输出默认行为 | 6 | --log-output 默认值修复回归：非 start 命令不写文件、start 前台不写文件、daemon 写文件、显式指定覆盖 |

### Web UI 测试

| 文件 | 功能模块 | 测试用例数 | 说明 |
|------|---------|-----------|------|
| [remote-access-web-ui.md](./remote-access-web-ui.md) | 远程访问管理 Web UI | 17 | 远程访问配置、登录、会话管理、登录记录展示 |
| [remote-access-brute-force-protection.md](./remote-access-brute-force-protection.md) | 远程访问暴力破解防护 | 13 | 登录失败计数、自动锁定、密码强度校验、本机恢复、前端锁定提示 |
| [webui-traffic.md](./webui-traffic.md) | Web UI Traffic 页面 | 45 | 流量表格、详情面板、Tab 切换、Body 视图、筛选过滤、右键菜单、WebSocket/SSE、搜索 |
| [webui-rules.md](./webui-rules.md) | Web UI Rules 页面 | 38 | 规则列表、创建/编辑/删除、语法高亮、自动补全、树形视图、Dynamic Island、导入导出、桌面端编辑器快捷键回归、Undo 后保存清理黄点 |
| [webui-scripts.md](./webui-scripts.md) | Web UI Scripts 页面 | 21 | 脚本创建（Req/Res/Dec）、编辑、保存、测试运行、日志查看、名称校验、树形目录、桌面端编辑器快捷键回归、Undo 后保存清理黄点 |
| [webui-values.md](./webui-values.md) | Web UI Values 页面 | 20 | Value 列表、创建/编辑/删除、编辑器、规则引用、导入导出、桌面端编辑器快捷键回归、Undo 后保存清理黄点 |
| [webui-replay.md](./webui-replay.md) | Web UI Replay 页面 | 22 | HTTP 请求重放、集合管理、SSE/WebSocket 重放、curl 导入、多种 Body 类型 |
| [webui-settings.md](./webui-settings.md) | Web UI Settings 页面 | 38 | Proxy/Certificate/TLS/Performance/Access Control/Appearance/Metrics/Sync 各 Tab |
| [webui-groups.md](./webui-groups.md) | Web UI Groups 页面 | 13 | Group 列表、详情、规则管理、搜索 |
| [webui-search.md](./webui-search.md) | Web UI 搜索模式 | 12 | 搜索模式进入/退出、关键词搜索、过滤器、结果高亮、状态持久化 |
| [webui-layout-navigation.md](./webui-layout-navigation.md) | Web UI 布局与导航 | 14 | 侧边栏导航、分割面板、状态栏、Toolbar、主题切换、版本检查、拖拽导入 |
| [statusbar-proxy-popover.md](./statusbar-proxy-popover.md) | StatusBar Proxy Hover 面板 | 6 | 底部状态栏 Proxy 区域 hover 弹出 Popover，快速切换系统代理开关、地址显示、状态同步 |
| [tls-passthrough-interactive.md](./tls-passthrough-interactive.md) | TLS 不信任域名交互式 Passthrough | 8 | TLS 不信任 Toast 弹窗交互、Passthrough / Ignore 按钮、Notifications 表格操作、域名排除列表联动 |

### Admin API 测试

| 文件 | 功能模块 | 测试用例数 | 说明 |
|------|---------|-----------|------|
| [api-rules.md](./api-rules.md) | Rules API | 13 | 规则 CRUD、启用/禁用、特殊字符名称、重复创建、rule_count 验证 |
| [api-traffic.md](./api-traffic.md) | Traffic API | 23 | 流量列表/详情/Body、增量更新、多维度过滤、WebSocket 帧、SSE 流 |
| [api-values.md](./api-values.md) | Values API | 15 | Values CRUD、列表查询、边界条件、错误处理 |
| [api-whitelist.md](./api-whitelist.md) | Whitelist API | 27 | 白名单增删、模式切换、allow-lan、临时白名单、待授权管理、SSE 事件流 |
| [api-cert.md](./api-cert.md) | Cert API | 8 | 证书信息、CA 下载、QR 码生成 |
| [api-proxy.md](./api-proxy.md) | Proxy API | 13 | 系统代理控制、CLI 代理状态、代理地址、QR 码 |
| [api-config.md](./api-config.md) | Config API | 21 | 全量配置、TLS 配置、性能配置、缓存清理、连接断开 |
| [api-config-advanced.md](./api-config-advanced.md) | Config API（高级） | 30 | Sandbox 沙箱配置、Server 服务器配置、UI 配置、IP-TLS Pending 管理、活跃连接管理 |
| [api-metrics.md](./api-metrics.md) | Metrics API | 15 | 当前指标、历史指标、应用统计、主机统计 |
| [api-system.md](./api-system.md) | System API | 16 | 系统信息、概览、内存诊断 |
| [api-scripts.md](./api-scripts.md) | Scripts API | 30 | 脚本 CRUD、重命名、运行测试、名称校验、内置脚本保护 |
| [api-push.md](./api-push.md) | Push WebSocket API | 10 | WebSocket 推送连接、订阅参数、流量/指标/概览实时推送 |
| [api-replay.md](./api-replay.md) | Replay API | 17 | 重放集合管理、请求 CRUD、执行重放、历史查看 |
| [api-group.md](./api-group.md) | Group API | 13 | 团队组列表/详情、团队规则 CRUD、权限校验 |
| [api-search.md](./api-search.md) | Search API | 16 | 全文搜索、搜索范围、过滤条件、分页、流式搜索 |
| [api-auth.md](./api-auth.md) | Auth API | 12 | 鉴权状态查询、登录、密码管理、远程访问开关、JWT 会话吊销 |
| [api-sync.md](./api-sync.md) | Sync API | 30 | 同步状态/配置/登录/登出/运行/Session，Env/Room/User 代理转发端点 |
| [api-misc.md](./api-misc.md) | Misc API | 32 | Syntax 语法信息、App Icon、WebSocket 连接、Audit 审计日志、Bifrost File 导入导出 |

### 代理核心功能测试

| 文件 | 功能模块 | 测试用例数 | 说明 |
|------|---------|-----------|------|
| [proxy-http-https.md](./proxy-http-https.md) | HTTP/HTTPS 代理 | 24 | HTTP 转发、HTTPS CONNECT、TLS 拦截、各类规则协议、模式匹配 |
| [proxy-socks5.md](./proxy-socks5.md) | SOCKS5 代理 | 3 | SOCKS5 基本代理、DNS 解析、HTTPS 透传 |
| [proxy-websocket-sse.md](./proxy-websocket-sse.md) | WebSocket/SSE 代理 | 6 | WebSocket/SSE 代理转发、帧/事件捕获、UI 消息面板 |
| [proxy-rules-advanced.md](./proxy-rules-advanced.md) | 规则协议全量测试 | 58 | 40+ 规则操作协议：请求/响应修改、内容注入、控制、路由、脚本、高级特性（Values 引用、模板字符串、正则捕获） |
| [proxy-auth-brute-force.md](./proxy-auth-brute-force.md) | 代理认证暴力破解防护 | 10 | HTTP/SOCKS5 代理认证 rate limiting：失败计数、10 次封禁（429/连接拒绝）、计数重置、IP 独立追踪 |
| [rule-merge-headers.md](./rule-merge-headers.md) | 规则合并 Header 覆盖 | 5 | reqHeaders/resHeaders 同名覆盖、路径深度优先级、真实代理场景验证、转发类无回归、两条同名 key 规则覆盖+客户端同名 header |
| [rule-merge-strategy.md](./rule-merge-strategy.md) | 规则合并策略全量验证 | 13 | 全量协议合并策略验证：转发类 first-match-wins、Mock 类 non-multi_match、标量值 single-match、Body/CORS/注入 last-wins、累积型 accumulate、KV 集合、特殊协议、控制类、E2E 真实代理场景 |
| [mock-file-serving.md](./mock-file-serving.md) | Mock File Serving | 6 | file://协议二进制文件（PNG/图片）返回、JSON/HTML 文本文件、tpl://模板变量替换、Content-Type 自动检测、HTTPS TLS 拦截路径回归 |
| [traffic-cleanup.md](./traffic-cleanup.md) | 流量记录清理逻辑 | 7 | 记录数超 115% 触发清理到 80% 水位、清理期间新流量落盘、Body 缓存文件清理、磁盘总量清理 body 同步、过度删除回归验证 |

### 网络与访问控制测试

| 文件 | 功能模块 | 测试用例数 | 说明 |
|------|---------|-----------|------|
| [cgn-address-support.md](./cgn-address-support.md) | CGN 地址段支持与同子网局域网判定 | 9 | RFC 6598 CGN 100.64.0.0/10 地址段识别、同子网判定、allow_lan 联动、IP 列表展示、边界值验证 |
| [network-refresh.md](./network-refresh.md) | 网络变化自动刷新子网信息 | 8 | VPN 连接/断开后子网自动刷新、WiFi 切换 IP 更新、访问控制策略实时同步、WebUI 实时推送 |

### 注入功能测试

| 文件 | 功能模块 | 测试用例数 | 说明 |
|------|---------|-----------|------|
| [badge-hover-panel.md](./badge-hover-panel.md) | Badge Hover 规则详情面板 | 7 | Badge hover 展开面板、规则列表展示、Merged Rules 折叠、规则行跳转编辑页、暗色模式、缓存性能、禁用验证 |

### 性能与内存优化测试

| 文件 | 功能模块 | 测试用例数 | 说明 |
|------|---------|-----------|------|
| [memory-sqlite-cache-optimization.md](./memory-sqlite-cache-optimization.md) | SQLite Cache Size 与内存优化 | 6 | SQLite cache_size 降低、读连接池缩减、metadata_cache LRU 化后的功能回归与内存验证 |

---

**总计：55 个测试文件，1059 个测试用例**

## 工作流程

### 1. 编写测试用例文档（开发完成后）

- 在本目录下创建 `功能模块名.md`
- 文档必须包含：前置条件、测试用例列表（编号 + 步骤 + 预期结果）、清理步骤
- 同步更新本文件（`readme.md`）的索引表

### 2. Agent 按用例自主执行测试

- Agent 读取对应的测试用例文档
- 按用例编号逐条执行：
  - **Web UI 用例**：通过 Chrome DevTools MCP 进行真实浏览器操作
  - **CLI 用例**：直接执行命令并验证输出
  - **API 用例**：通过 curl 或等效方式发起请求并验证响应
- 每个用例执行后，将实际结果与预期结果对比
- 如有不一致，修复代码后重新执行

## 约定

- 所有测试启动服务时必须使用临时数据目录（`BIFROST_DATA_DIR=./.bifrost-test`），避免影响正式环境
- 测试端口避免使用 9900（正式环境端口），推荐使用 8800 或其他端口
- 每个测试文件包含：前置条件、测试步骤、预期结果、清理步骤
- 用例编号格式：`TC-{模块缩写}-{序号}`（如 `TC-RA-01`）
