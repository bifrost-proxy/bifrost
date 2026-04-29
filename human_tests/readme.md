# Bifrost 真实场景测试用例（Agent 自主执行）

本目录存储自然语言描述的测试用例文档，用于指导 Agent 自主进行真实场景测试。每个测试文件对应一个功能模块。

**核心定位**：`human_tests/` 是 Agent 驱动真实场景测试的标准载体。每次需求开发结束后，必须先在此目录创建或更新测试用例文档，再由 Agent 按文档逐条自主执行测试。

## 目录结构

### CLI 命令测试

| 文件 | 功能模块 | 测试用例数 | 说明 |
|------|---------|-----------|------|
| [cli-start-stop-status.md](./cli-start-stop-status.md) | CLI 服务管理 | 25 | start/stop/status 命令，含守护进程、自定义端口、TLS 选项、规则加载、SOCKS5、LAN 访问、代理认证，以及 status 活跃规则摘要 |
| [cli-start-advanced.md](./cli-start-advanced.md) | CLI Start 高级参数 | 33 | 顶层 help 的 start 参考区块同步、TLS 拦截域名/应用排除与白名单、系统代理（默认启用、--no-system-proxy 禁用、异步收敛轮询、互斥校验）、CLI 代理环境变量、访问控制模式、Badge 注入、证书检查跳过、日志配置 |
| [cli-rule-management.md](./cli-rule-management.md) | CLI 规则管理 | 45 | rule 子命令全覆盖：list/add/show/get/update/enable/disable/delete/rename/reorder/active/sync，含过滤器和 lineProps |
| [cli-rule-list-legacy-skip.md](./cli-rule-list-legacy-skip.md) | CLI `rule list` `.bifrost` 文件过滤 | 2 | 非 `.bifrost` 文件自动忽略，且 group 子目录规则仍可正常读取 |
| [cli-traffic-search.md](./cli-traffic-search.md) | CLI 流量与搜索 | 36 | traffic list/get/search/clear 命令，含多维度过滤器、搜索范围控制、交互式搜索 |
| [cli-ca-cert.md](./cli-ca-cert.md) | CLI CA 证书管理 | 12 | ca generate/export/info/install 命令，含强制重新生成、指定路径导出、证书格式验证 |
| [cli-values-scripts.md](./cli-values-scripts.md) | CLI Values 与 Scripts | 30 | value list/add/show/set/update/delete/import 和 script list/add/show/get/update/run/rename/delete |
| [cli-whitelist.md](./cli-whitelist.md) | CLI 白名单管理 | 31 | whitelist 全子命令：list/add/remove/allow-lan/status/mode/pending/approve/reject/clear-pending/add-temporary/remove-temporary |
| [cli-admin.md](./cli-admin.md) | CLI Admin 管理 | 14 | admin remote status/enable/disable、admin passwd、admin revoke-all、admin audit |
| [cli-config.md](./cli-config.md) | CLI 配置管理 | 22 | config show/get/set/add/remove/reset/clear-cache/disconnect/export/connections/memory |
| [cli-system-proxy.md](./cli-system-proxy.md) | CLI 系统代理 | 10 | system-proxy status/enable/disable，含自定义 host/port/bypass |
| [cli-group.md](./cli-group.md) | CLI Group 管理 | 14 | group list/show、group rule list/show/add/update/enable/disable/delete |
| [cli-import-export.md](./cli-import-export.md) | CLI 导入导出与杂项 | 27 | export/import、metrics、sync、version-check、upgrade、completions、install-skill，含 version-check 空输出与 install-skill 更多 agent 兼容回归验证，以及 version-check redirect 优先与 HTML highlights 降级验证 |
| [port-conflict-restart.md](./port-conflict-restart.md) | 端口冲突检测与自动重启 | 6 | 端口占用检测、进程信息显示、交互式终止确认、--yes 自动确认、PID 检测兼容性、非交互端口冲突早于系统代理摘要回归 |
| [cli-log-output-default.md](./cli-log-output-default.md) | CLI 日志输出默认行为 | 6 | --log-output 默认值修复回归：非 start 命令不写文件、start 前台不写文件、daemon 写文件、显式指定覆盖 |

### Web UI 测试

| 文件 | 功能模块 | 测试用例数 | 说明 |
|------|---------|-----------|------|
| [remote-access-web-ui.md](./remote-access-web-ui.md) | 远程访问管理 Web UI | 17 | 远程访问配置、登录、会话管理、登录记录展示 |
| [remote-access-brute-force-protection.md](./remote-access-brute-force-protection.md) | 远程访问暴力破解防护 | 13 | 登录失败计数、自动锁定、密码强度校验、本机恢复、前端锁定提示 |
| [webui-traffic.md](./webui-traffic.md) | Web UI Traffic 页面 | 45 | 流量表格、详情面板、Tab 切换、Body 视图、筛选过滤、右键菜单、WebSocket/SSE、搜索 |
| [webui-rules.md](./webui-rules.md) | Web UI Rules 页面 | 40 | 规则列表、创建/编辑/删除、语法高亮、自动补全、树形视图、Dynamic Island、Merged Rules 一键复制、导入导出、桌面端编辑器快捷键回归、Undo 后保存清理黄点、编辑器内容恢复原文后 Save 按钮禁用回归 |
| [webui-scripts.md](./webui-scripts.md) | Web UI Scripts 页面 | 21 | 脚本创建（Req/Res/Dec）、编辑、保存、测试运行、日志查看、名称校验、树形目录、桌面端编辑器快捷键回归、Undo 后保存清理黄点 |
| [webui-values.md](./webui-values.md) | Web UI Values 页面 | 20 | Value 列表、创建/编辑/删除、编辑器、规则引用、导入导出、桌面端编辑器快捷键回归、Undo 后保存清理黄点 |
| [webui-replay.md](./webui-replay.md) | Web UI Replay 页面 | 23 | HTTP 请求重放、集合管理、SSE/WebSocket 重放、curl 导入、多种 Body 类型、localhost 转发与 passthrough 优先级回归 |
| [webui-settings.md](./webui-settings.md) | Web UI Settings 页面 | 38 | Proxy/Certificate/TLS/Performance/Access Control/Appearance/Metrics/Sync 各 Tab |
| [file-access-webui.md](./file-access-webui.md) | File Access WebUI 策略配置 | 17 | Grants 行级 File Access 入口、禁止手动录入不存在 grant、只读/读写与指定/所有目录策略、SSH Key grant 继承默认 All Directories 策略、grant 删除自动清理策略、重新连接后重新配置、deny patterns、字节限制、API 验证 |
| [webui-groups.md](./webui-groups.md) | Web UI Groups 页面 | 13 | Group 列表、详情、规则管理、搜索 |
| [webui-search.md](./webui-search.md) | Web UI 搜索模式 | 12 | 搜索模式进入/退出、关键词搜索、过滤器、结果高亮、状态持久化 |
| [webui-notifications.md](./webui-notifications.md) | Web UI Notifications 页面 | 3 | 三个通知表顶部状态筛选、默认未读展示、固定分页无 page size 选择器 |
| [webui-layout-navigation.md](./webui-layout-navigation.md) | Web UI 布局与导航 | 14 | 侧边栏导航、分割面板、状态栏、Toolbar、主题切换、版本检查、拖拽导入 |
| [statusbar-proxy-popover.md](./statusbar-proxy-popover.md) | StatusBar Proxy Hover 面板 | 6 | 底部状态栏 Proxy 区域 hover 弹出 Popover，快速切换系统代理开关、地址显示、状态同步 |
| [tls-passthrough-interactive.md](./tls-passthrough-interactive.md) | TLS 不信任域名交互式 Passthrough | 8 | TLS 不信任 Toast 弹窗交互、Passthrough / Ignore 按钮、Notifications 表格操作、域名排除列表联动 |
| [tls-trust-detection.md](./tls-trust-detection.md) | TLS 信任检测改进（降低误伤） | 10 | 错误分类精细化（definite/probable/decrypt）、PossiblyNotTrusted 中间状态、MIN_DEFINITE 门槛、per-domain 追踪、WebUI 状态展示 |
| [chrome-devtools-remote-control.md](./chrome-devtools-remote-control.md) | Bifrost DevTools Remote Control | 15 | 显式裸 `devtools://` 规则驱动的代理页面发现、page_bridge 降级调试、bridge WebSocket 双向通信且页面侧无独立 HTTP 上报/轮询、WebUI session WebSocket 推送、服务端轻量路由且不保存完整调试历史、目标页有界 buffer、事件队列短延迟批量异步发送、WebUI DevTools tab 自有 Elements/Network/Cookies/LocalStorage/SessionStorage/Console 面板、紧凑详情页头、Traffic 跳转入口、URL hover 复制、content 区域填满剩余高度、右侧模块搜索、规则编辑器无参数智能提示、vConsole/Chrome DevTools 风格 Elements 树、超长值 120 字符预览与详情弹窗完整复制、存储行内新增/编辑/复制/删除、Chrome 风格 Console 底部多行输入、毫秒级低对比度时间戳、全屏 JavaScript 编辑器与 input/result/error 行、默认 Console evaluate 真实执行并展示远端 JS 异常、多页面切换、HTML candidate 幽灵页隐藏、静默但 WS 连接页面仍在线、目标页刷新后 WebUI session 自动恢复、移动 Safari UA 降级路径，以及 Chrome DevTools frontend 安装/托管/打开入口清理回归 |

### 远程调用测试

| 文件 | 功能模块 | 测试用例数 | 说明 |
|------|---------|-----------|------|
| [remote-invoke.md](./remote-invoke.md) | Remote Invoke 远程调用 | 177 | 发现模式与一次性授权码、人工授权、授权复用（30m/1h/1d/永久）、授权模式升降级、多客户端管理、有效期调整、移除授权、SSE/HTTP relay、大结果/大输入分片传输、主动取消、续流恢复、断线恢复、白名单命令全覆盖、端到端加密验证、客户端重启稳定性与 SSH route 三态同步、审计历史与过滤、第一阶段 relay v2 协议升级（`command_encrypted` / `exit_encrypted` / route-only metadata / query grant 拒绝 shell.exec / 本地与远端 relay 复测 / caller-client 双端加密 roundtrip / 真实 CLI 加密黑盒闭环 / grant crypto 持久化后 client 重启继续复用 / 同一远端重复 pair-code connect 后 caller 仍绑定最后一次 grant）、回归验证（含 SSE 事件去重 + 多实例 frame/exit 竞态 + 超时 pairing 自动清理 + 超时 pairing 不再占用 pair_slot_occupied 且审批不再报 500 + 过期 grant 自动清理 + 客户端侧 DELETE grant + relay_token 鉴权安全加固 + calls 路由迁移 + approve_pairing fingerprint 修复 + delete_grant best-effort 修复 + call_open grant 验证安全加固 + client 注册 token/challenge/签名校验 + caller 免 token 边界 + call detail 所有权隔离 + pairing decision 所有权隔离 + call frame/exit 所有权隔离 + remote traffic get sequence 映射 + remote traffic list 全量过滤参数透传 + remote traffic search `query/max_results/max_scan` 执行端透传 + remote search 流式输出 + remote search `max_results/max_scan` 执行端透传 + stderr 透传 + remote connect overload-protect 重试与提示 + grant_created/call_open 协议职责分离 + SSE 推送失败轮询容错 + pair_slot_occupied 自动清理 + pending-pairings API + relay URL 四级回退优先级 + caller identity 持久化 + SSH key 管理 API/导出/重置/撤销 + SSH 授权永久有效直到 key revoke + relay challenge/connect 最小闭环 + SSH grant relay 复用/openCall 能力验收 + revoke 后 route 收敛删除验证 + 线上 relay 的 SSH reusable/openCall 存储链路回归 + Remote Invoke 状态区合并布局回归 + Create SSH key 弹窗提示合并回归 + Shell Access 编辑器中 Policy/Profile ID 只读回归 + caller 主动取消后所有命令统一收敛为 cancelled + 本地 relay 粗粒度限流不再打断 cancel/events/exit 收尾 + 线上 relay 下 target client 取消终态稳定写入 + 共享出口 IP 下已认证 remote invoke 不再互相限流 + 远端 relay 不引入 pod-local authenticated remote limiter，`client/stream` 直接从认证结果补齐 `user_id` + relay 返回 `grant_not_found` 时 disconnect 仍删除本地连接，避免幽灵状态 + CLI `remote connect --ssh-key` 落盘与后续复用回归 + server-v4 SSH connect 挂起态持久化 caller_info，确保 SSH grant 展示调用方信息 + SSH key reset 后 worker 显式进入 reconnecting，避免 post-reset connect 命中假性离线窗口 + `call_cancel` 即使遇到本地句柄竞态也能把 Recent Calls 收敛到 `cancelled` + shell E2E 夹具与当前加密协议保持一致 + Recent Calls 参数预览/Tooltip 从本地解密 `args_json` 回退恢复 + client 本地 grant crypto 丢失后重连主动清理幽灵授权，并同步清空 caller stale connection；后续 disconnect 回归需基于 fresh reconnect 继续验证 + shell E2E 在 `--skip-build` 且缺失 sync-server dist 时自动回退源码入口 + caller `open_call` 直接携带参数摘要，且 remote invoke shell E2E 仅在 release 过期时自动重建二进制 + pair-code connect 后 Client grants 列表在短时间内稳定可见 + Recent Calls 参数预览回归脚本改为本地 mock 流量，避免公网依赖 + Recent Calls 重启后从本地落盘恢复 + 清理全部记录 + Recent Calls 命令相关文本 200 字符截断与点击详情弹窗 + Grants 首次连接/最近命令时间字段展示 + Remote Status 移除 Active Calls 重复计数 + Remote Status 未登录暗色主题 + Grants 连接方式展示 + Remote Status 未登录 Sync 暗色主题提示）、补充覆盖（多调用方并发隔离/配对码轮换/并发冲突/traffic.clear 拒绝/once consumed/grant 上限）、全局授权弹窗（自动弹出/Dismiss/Dismiss All/Authorize 下拉/Reject）、远端部署（HTTPS/SSO/多用户并发/跨公网稳定性/大结果传输/断线恢复）、交互式客户端选择（多客户端未指定 --client-id 时弹出选择菜单/模糊前缀匹配多客户端/非交互环境回退报错） |
| [remote-invoke-sshkey.md](./remote-invoke-sshkey.md) | Remote Invoke SSH Key Caller Identity | 1 | 同一 SSH key 在多个 caller 沙箱中连接同一 target 时生成不同随机 caller ID，grant 以 caller ID 隔离且 SSH key fingerprint 仅作为密钥属性保留 |
| [skill-remote.md](./skill-remote.md) | Bifrost Remote Skill | 8 | install-skill 同时安装 `bifrost` 与 `bifrost-remote`，remote skill description 表达远程设备控制能力，目标端正式启动默认使用 `bifrost start` / 9900 / 系统代理，明确查询、shell、文件三类 scope 前置准备，当前 relay-backed 子命令边界正确指向 `remote exec` 操作目标设备，要求远端工程任务先读取 `AGENTS.md` / `.agents/skills` 元信息，不包含历史版本迁移文案，且不提供 `remote traffic clear` 写操作命令 |
| [remote-command-isomorphic.md](./remote-command-isomorphic.md) | Remote Command 同构化回归 | 29 | 本地与远端 `search/traffic` 命令矩阵回归：覆盖 search/traffic list/get/clear、remote search/traffic list/get 的子命令、参数、默认值、格式输出、流式输出、remote traffic clear 不暴露边界，以及 filter-only query / 机器可读输出回归 |
| [remote-traffic-cli-enum-size.md](./remote-traffic-cli-enum-size.md) | Remote Traffic CLI 枚举体瘦身 | 3 | `RemoteTrafficCommands` large enum variant 回归：验证 `remote traffic list` 全量过滤参数解析、`remote traffic search` 参数透传，以及 clippy 不再报 `large_enum_variant` |
| [remote-shell-exec.md](./remote-shell-exec.md) | Remote Shell Exec | 26 | `bifrost remote command exec` 主链路回归：caller 不再传 `policy_id`、target 基于 grant binding 与本地 Shell Access 自动选择唯一策略、query/shell scope 隔离、策略未命中与歧义匹配拒绝、policy version 失效、`Full Access` / `Default Sandbox` 真实语义、grant 的 WebUI / CLI 编辑、target 本地 grant policy overlay 持久化、relay 仅保留最小 `grant_scope` 不存储具体策略绑定，reconnect 覆盖旧 grant / disconnect 清理残留 reusable grants 的验证；Windows shell_text Unix 路径 fallback（`/bin/bash` → `cmd`）与 UTF-8 编码处理（`chcp 65001` / PowerShell OutputEncoding）、`policy update` 命令原地更新策略元数据不破坏 grant 有效性、CLI 对裸 argv 输入的显式拒绝回归、长时间命令 stdout 流式输出回归、Windows 流式 shell 输出 E2E 回归，以及对应 shell E2E 自动化脚本回归 |
| [remote-invoke-file.md](./remote-invoke-file.md) | Remote Invoke File API | 36 | file.read/list/stat/glob/search/hash/write/edit/mkdir/mv/rm/apply-patch 的正向用例、FileAccessPolicy 的 roots/denies/symlink-escape/scope/二进制/target/只读 policy 拒绝写操作错误码回归用例、shell scope 不自动授予 file API、配对批准时 file scope 不依赖 Shell Access policy 的 CI 回归、SSH Key 默认 File Policy 配置与 reset 保留/误删自愈/旧 grant fingerprint 修复回归、并发与 grant 过期稳定性、审计日志留痕、CLI --help / --output json UX 验收、以及 coding agent 增强能力（offset/limit 行范围读取、search context 上下文行、glob/search/list 默认排除 .git/node_modules/target） |
| [grant-file-access.md](./grant-file-access.md) | Grant File Access 正交权限模型 | 18 | file_access 独立于 grant_scope 的正交权限模型：WebUI 预设策略模式、API approve/update file_access、CLI --file-access 参数、权限检查、SSE grant_created 包含 file_access 回归、approve_pairing 持久化 grant_info 回归、Full Access 端到端文件操作验证、三策略动态切换验证（read_write/read/none）、executor 写权限检查回归 |
| [grant-permission-hierarchy.md](./grant-permission-hierarchy.md) | Grant 权限层级模型 | 14 | Shell > File > Query 层级验证：Shell 默认包含 File(read_write) + Query、降级到 Query 后 shell/file 被拒、升级到 File 后 file 可用 shell 仍拒、权限切换后 grant 保持有效、无 shell policy 降级为 query、shell_grant_provision 层级默认值单元测试 |

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
| [api-push.md](./api-push.md) | Push WebSocket API | 11 | WebSocket 推送连接、订阅参数、流量/指标/概览实时推送、经代理访问管理端回归 |
| [api-replay.md](./api-replay.md) | Replay API | 18 | 重放集合管理、请求 CRUD、执行重放、历史查看、路径前缀转发回归 |
| [api-group.md](./api-group.md) | Group API | 13 | 团队组列表/详情、团队规则 CRUD、权限校验 |
| [api-search.md](./api-search.md) | Search API | 16 | 全文搜索、搜索范围、过滤条件、分页、流式搜索 |
| [api-auth.md](./api-auth.md) | Auth API | 12 | 鉴权状态查询、登录、密码管理、远程访问开关、JWT 会话吊销 |
| [api-sync.md](./api-sync.md) | Sync API | 32 | 同步状态/配置/登录/登出/运行/Session，Env/Room/User 代理转发端点，CI/沙箱 token+URL 直登 |
| [api-misc.md](./api-misc.md) | Misc API | 32 | Syntax 语法信息、App Icon、WebSocket 连接、Audit 审计日志、Bifrost File 导入导出 |

### 代理核心功能测试

| 文件 | 功能模块 | 测试用例数 | 说明 |
|------|---------|-----------|------|
| [proxy-http-https.md](./proxy-http-https.md) | HTTP/HTTPS 代理 | 26 | HTTP 转发、HTTPS CONNECT、TLS 拦截、各类规则协议、模式匹配、host 路径前缀回归、旧版 `^https://` path wildcard 兼容回归 |
| [proxy-socks5.md](./proxy-socks5.md) | SOCKS5 代理 | 3 | SOCKS5 基本代理、DNS 解析、HTTPS 透传 |
| [proxy-websocket-sse.md](./proxy-websocket-sse.md) | WebSocket/SSE 代理 | 7 | WebSocket/SSE 代理转发、帧/事件捕获、UI 消息面板、Replay WebSocket E2E 启动隔离与诊断 |
| [proxy-rules-advanced.md](./proxy-rules-advanced.md) | 规则协议全量测试 | 65 | 40+ 规则操作协议：请求/响应修改、内容注入、控制、路由、脚本、高级特性（Values 引用、模板字符串、正则捕获），含 html/js/css 内容注入协议矩阵、htmlPrepend 插入 `<html>` 后、htmlAppend 插入 `</html>` 前，以及 HTTPS 转发到 HTTP 上游、gzip HTML 响应编码一致性、mock 生成资源、通配域名根路径 htmlAppend 匹配、culture.shtml HTTPS MITM 背景图白屏和上游 HTTP/2 body 断流 fallback 真实回归 |
| [proxy-auth-brute-force.md](./proxy-auth-brute-force.md) | 代理认证暴力破解防护 | 10 | HTTP/SOCKS5 代理认证 rate limiting：失败计数、10 次封禁（429/连接拒绝）、计数重置、IP 独立追踪 |
| [rule-merge-headers.md](./rule-merge-headers.md) | 规则合并 Header 覆盖 | 6 | reqHeaders/resHeaders 同名覆盖、路径深度优先级、真实代理场景验证、转发类无回归、两条同名 key 规则覆盖+客户端同名 header、HTTPS passthrough/tunnel 客户端同名 header 去重覆盖 |
| [rule-merge-strategy.md](./rule-merge-strategy.md) | 规则合并策略全量验证 | 13 | 全量协议合并策略验证：转发类 first-match-wins、Mock 类 non-multi_match、标量值 single-match、Body/CORS/注入 last-wins、累积型 accumulate、KV 集合、特殊协议、控制类、E2E 真实代理场景 |
| [rules-e2e-fixtures.md](./rules-e2e-fixtures.md) | Rules E2E Fixtures | 2 | replay 历史夹具 `__MOCK_HTTP_PORT__` 端口占位符与并行 runner 动态 echo 端口兼容回归 |
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
| [badge-hover-panel.md](./badge-hover-panel.md) | Badge Hover 规则详情面板 | 10 | Badge hover 展开面板、规则列表展示、Merged Rules 折叠与一键复制、规则行跳转编辑页、高 z-index 浮层覆盖回归、暗色模式、缓存性能、禁用验证、Merged Rules HTML/Script 标签片段通用转义 |

### 性能与内存优化测试

| 文件 | 功能模块 | 测试用例数 | 说明 |
|------|---------|-----------|------|
| [memory-sqlite-cache-optimization.md](./memory-sqlite-cache-optimization.md) | SQLite Cache Size 与内存优化 | 6 | SQLite cache_size 降低、读连接池缩减、metadata_cache LRU 化后的功能回归与内存验证 |

### CI/DevOps 测试

| 文件 | 功能模块 | 测试用例数 | 说明 |
|------|---------|-----------|------|
| [ci-shell-e2e-sharding.md](./ci-shell-e2e-sharding.md) | CI Shell E2E 测试分片优化 | 9 | --shard N/M 参数解析、环境变量透传、分片覆盖完整性、无分片向后兼容、local-ci.sh 分片支持、单分片耗时 <5min、CI skip 列表、格式校验、系统代理用例 CI 禁跑且本地保留 |
| [ci-macos-cli-e2e-split.md](./ci-macos-cli-e2e-split.md) | CI macOS CLI/E2E 构建拆分 | 4 | macOS rules/shell E2E 仅等待 aarch64 CLI 构建，desktop bundle 与 x86_64 CLI 构建不阻塞 E2E |
| [linux-install-musl-fallback.md](./linux-install-musl-fallback.md) | Linux 旧 glibc 安装 musl 回退 | 4 | Debian 10 / glibc 2.28 自动选择 musl 预编译包，新 glibc 保持 GNU 包，npm/npx 平台包与 `bifrost upgrade` 同步回退到 musl |

---

**总计：68 个测试文件，1309 个测试用例**

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
