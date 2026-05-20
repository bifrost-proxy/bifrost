# Bifrost 真实场景测试用例（Agent 自主执行）

本目录存储自然语言描述的测试用例文档，用于指导 Agent 自主进行真实场景测试。每个测试文件对应一个功能模块。

**核心定位**：`human_tests/` 是 Agent 驱动真实场景测试的标准载体。每次需求开发结束后，必须先在此目录创建或更新测试用例文档，再由 Agent 按文档逐条自主执行测试。

## 目录结构

### CLI 命令测试

| 文件 | 功能模块 | 测试用例数 | 说明 |
|------|---------|-----------|------|
| [cli-start-stop-status.md](./cli-start-stop-status.md) | CLI 服务管理 | 28 | start/stop/status 命令，含守护进程、自定义端口、TLS 选项、规则加载、SOCKS5、LAN 访问、代理认证、status 顶部代理能力/TLS 边界概览、默认端口规则摘要分区、status 活跃规则摘要，以及 listener 失败退出与 daemon readiness 回归 |
| [cli-start-advanced.md](./cli-start-advanced.md) | CLI Start 高级参数 | 33 | 顶层 help 短链化、按场景组织的 CLI 快速开始、同一 Bifrost 服务服务多个应用/开发任务、Agent 协作开发业务 Skill 场景、完整 CLI 详细文档入口、全局 Values 推荐边界、TLS 拦截域名/应用排除与白名单、系统代理（默认启用、--no-system-proxy 禁用、异步收敛轮询、互斥校验）、CLI 代理环境变量、访问控制模式、Badge 注入、证书检查跳过、日志配置 |
| [cli-rule-management.md](./cli-rule-management.md) | CLI 规则管理 | 45 | rule 子命令全覆盖：list/add/show/get/update/enable/disable/delete/rename/reorder/active/sync，含过滤器和 lineProps |
| [cli-rule-list-legacy-skip.md](./cli-rule-list-legacy-skip.md) | CLI `rule list` `.bifrost` 文件过滤 | 2 | 非 `.bifrost` 文件自动忽略，且 group 子目录规则仍可正常读取 |
| [cli-traffic-search.md](./cli-traffic-search.md) | CLI 流量与搜索 | 37 | traffic list/get/search/clear 命令，含多维度过滤器、代理入口端口筛选、搜索范围控制、交互式搜索 |
| [cli-ca-cert.md](./cli-ca-cert.md) | CLI CA 证书管理 | 12 | ca generate/export/info/install 命令，含强制重新生成、指定路径导出、证书格式验证 |
| [cli-values-scripts.md](./cli-values-scripts.md) | CLI Values 与 Scripts | 30 | value list/add/show/set/update/delete/import 和 script list/add/show/get/update/run/rename/delete |
| [cli-whitelist.md](./cli-whitelist.md) | CLI 白名单管理 | 31 | whitelist 全子命令：list/add/remove/allow-lan/status/mode/pending/approve/reject/clear-pending/add-temporary/remove-temporary |
| [cli-admin.md](./cli-admin.md) | CLI Admin 管理 | 14 | admin remote status/enable/disable、admin passwd、admin revoke-all、admin audit |
| [cli-config.md](./cli-config.md) | CLI 配置管理 | 22 | config show/get/set/add/remove/reset/clear-cache/disconnect/export/connections/memory |
| [cli-system-proxy.md](./cli-system-proxy.md) | CLI 系统代理 | 10 | system-proxy status/enable/disable，含自定义 host/port/bypass |
| [cli-group.md](./cli-group.md) | CLI Group 管理 | 15 | group list/show、group rule list/show/add/update/enable/disable/delete，以及 Group CLI mock 单测并发稳定性回归 |
| [temporary-port-rule-bindings.md](./temporary-port-rule-bindings.md) | 临时端口规则绑定 | 22 | `bifrost port` 临时端口绑定/查看/更新/销毁，规则名/规则文件/规则原文输入，端口级规则隔离，status 底部临时端口绑定规则区块，端口分配冲突回归，Traffic CLI/API/Web 端口展示回归，无规则命中流量端口归因回归，Settings Proxy 临时端口卡片回归，临时端口 Badge 规则视图回归，Traffic list/get 子命令 `--port` 解析回归，CLI help / 安装用 SKILL.md 文档同步验收，以及 light/dark 主题验证 |
| [cli-import-export.md](./cli-import-export.md) | CLI 导入导出与杂项 | 27 | export/import、metrics、sync、version-check、upgrade、completions、install-skill，含 version-check 空输出与 install-skill 更多 agent 兼容回归验证，以及 version-check redirect 优先与 HTML highlights 降级验证 |
| [port-conflict-restart.md](./port-conflict-restart.md) | 端口冲突检测与自动重启 | 6 | 端口占用检测、进程信息显示、交互式终止确认、--yes 自动确认、PID 检测兼容性、非交互端口冲突早于系统代理摘要回归 |
| [cli-log-output-default.md](./cli-log-output-default.md) | CLI 日志输出默认行为 | 8 | --log-output 默认值修复回归：非 start 命令不写文件、start 前台不写文件、daemon 写文件、显式指定覆盖，以及默认 info 日志隐藏常态连接生命周期与规则命中噪声 |
| [docs-implementation-sync.md](./docs-implementation-sync.md) | Docs 与实现同步质检 | 7 | docs/CLI/Scripts/规则协议说明与当前 `bifrost --help`、traffic/search/remote file help、ScriptType::Parser、bp/devtools 协议、过滤器 resolver 边界、workspace crate 架构索引、Markdown 相对链接，以及规则语法示例保持一致 |

### Web UI 测试

| 文件 | 功能模块 | 测试用例数 | 说明 |
|------|---------|-----------|------|
| [remote-access-web-ui.md](./remote-access-web-ui.md) | 远程访问管理 Web UI | 17 | 远程访问配置、登录、会话管理、登录记录展示 |
| [remote-access-brute-force-protection.md](./remote-access-brute-force-protection.md) | 远程访问暴力破解防护 | 13 | 登录失败计数、自动锁定、密码强度校验、本机恢复、前端锁定提示 |
| [webui-traffic.md](./webui-traffic.md) | Web UI Traffic 页面 | 48 | 流量表格、详情面板、Tab 切换、Body 视图、筛选过滤（含主筛选器按代理端口过滤与临时停用单条条件）、右键菜单、WebSocket/SSE、搜索、高并发 CONNECT 压力 |
| [webui-rules.md](./webui-rules.md) | Web UI Rules 页面 | 43 | 规则列表、创建/编辑/删除、排序方式 UI 配置持久化、语法高亮、自动补全、树形视图、Dynamic Island、Merged Rules 一键复制、Group active summary 与代理运行时本地 fallback、远端失败和快速本地变更稳定性、Group name 深链不返回 502、导入导出、桌面端编辑器快捷键回归、Undo 后保存清理黄点、编辑器内容恢复原文后 Save 按钮禁用回归 |
| [webui-scripts.md](./webui-scripts.md) | Web UI Scripts 页面 | 25 | 脚本创建（Req/Res/Dec/Parser）、顶部 + 创建菜单、... 更多操作菜单、真实 Import 文件选择器、编辑、保存、测试运行、日志查看、名称校验、树形目录、Parser/Decode 运行时上下文字段补全、桌面端编辑器快捷键回归、Undo 后保存清理黄点 |
| [webui-values.md](./webui-values.md) | Web UI Values 页面 | 20 | Value 列表、创建/编辑/删除、编辑器、规则引用、导入导出、桌面端编辑器快捷键回归、Undo 后保存清理黄点 |
| [webui-replay.md](./webui-replay.md) | Web UI Replay 页面 | 23 | HTTP 请求重放、集合管理、SSE/WebSocket 重放、curl 导入、多种 Body 类型、localhost 转发与 passthrough 优先级回归 |
| [webui-settings.md](./webui-settings.md) | Web UI Settings 页面 | 38 | Proxy/Certificate/TLS/Performance/Access Control/Appearance/Metrics/Sync 各 Tab |
| [skill-creator.md](./skill-creator.md) | Skill Creator WebUI 与 Agent 子系统 | 15 | Skill Creator crate、Agent slash router、Admin CRUD、WebUI Skills 面板（只读详情/删除/导入 zip/固定分页）、E2E create-test-invoke-delete-import、executor 环境白名单、registry watcher 单 slug 热重载、checksum 缺失 manifest、packager import scope 保留、authoring.test 非法状态 |
| [weixin-provider.md](./weixin-provider.md) | Weixin Provider | 8 | 原生 `weixin` IM provider 扫码登录、二维码自动轮询/刷新、微信文本消息触发 Agent、guide/queue/slash 命令回执、最终纯正文回写、History 表格窄宽度滚动与 Tooltip、图片消息下载后传给多模态模型、Agent 生成图片通过微信独立发送原图 |
| [file-access-webui.md](./file-access-webui.md) | File Access WebUI 策略配置 | 17 | Grants 行级 File Access 入口、禁止手动录入不存在 grant、只读/读写与指定/所有目录策略、SSH Key grant 继承默认 All Directories 策略、grant 删除自动清理策略、重新连接后重新配置、deny patterns、字节限制、API 验证 |
| [webui-groups.md](./webui-groups.md) | Web UI Groups 页面 | 13 | Group 列表、详情、规则管理、搜索 |
| [webui-search.md](./webui-search.md) | Web UI 搜索模式 | 12 | 搜索模式进入/退出、关键词搜索、过滤器、结果高亮、状态持久化 |
| [webui-notifications.md](./webui-notifications.md) | Web UI Notifications 页面 | 3 | 三个通知表顶部状态筛选、默认未读展示、固定分页无 page size 选择器 |
| [webui-layout-navigation.md](./webui-layout-navigation.md) | Web UI 布局与导航 | 16 | 侧边栏导航、侧边栏小窗口滚动、分割面板、状态栏、Toolbar、主题切换、版本检查、升级命令复制、拖拽导入 |
| [webui-ai-skill-assistant.md](./webui-ai-skill-assistant.md) | WebUI AI Skill Assistant | 6 | 全局右下角 AI skill 引导入口、hover 浮窗、hover 延迟关闭、安装命令复制、仓库 SKILL.md 链接、拖拽位置、点击隐藏，以及真实 Codex CLI 调用、亮色/暗色主题验证 |
| [chatgpt-web-adapter.md](./chatgpt-web-adapter.md) | ChatGPT Web Adapter | 23 | IM Gateway Runner 内置 `chatgpt_web` adapter：WebUI 配置、登录态强校验、自动弹出浏览器且无固定登录等待超时、Stop Login 主动停止、默认 headed 创建/追加对话并便于观察真实页面操作、追加消息必须进入稳定的新建页或目标 `/c/{conversationId}` 并通过浏览器 UI composer 发送、超过 120 字符的长输入使用页面内 paste 路径避免 CDP `Input.insertText` 卡死、超长输入避免 ChatGPT 粘贴附件模式、mock IM 入站连续注入验证队列消费、handoff heartbeat 防静默卡住、list/get/wait、消息列表、长任务超时、run stop、SSE handoff 中断恢复、发送/等待性能回归、短文本回复不得被 DOM fallback 字符阈值丢弃，DOM fallback 必须等输出状态和 composer/send 控件恢复后才结束、CLI JSON 按 thinking/tool/final 输出 NDJSON，IM 只分批投递 thinking/final 且不投递工具调用、图片生成状态文案/空壳不能提前结束，role-less image section 也能作为最终图片结果，明确请求 N 张图片时继续补齐懒加载图片、Session 记录可在 WebUI 查看输入/输出/异常、生成图片原图解析并缓存到数据目录附件存储，再按 Weixin POST CDN `image_item` / Feishu `image_key` 等 IM 通道各自图片模式逐张发送、失败时在 chat_runs 写入 failure_diagnostics、page_dom 和 conversation_response 诊断、脱敏 artifacts 与登录失效反馈、TC-CWA-19 ConversationTab 长驻池（容量 16，按 conversation_id 复用，服务重启后 attach 现有 conversation tab，LRU 淘汰，模式切换/进程死亡时清空） |
| [webui-static-assets.md](./webui-static-assets.md) | WebUI Static Assets | 3 | WebUI 静态资源 gzip 嵌入发布、gzip 客户端直接接收压缩响应、非 gzip 客户端升级提示、SPA 深链 fallback |
| [statusbar-proxy-popover.md](./statusbar-proxy-popover.md) | StatusBar Proxy Hover 面板 | 6 | 底部状态栏 Proxy 区域 hover 弹出 Popover，快速切换系统代理开关、地址显示、状态同步 |
| [tls-passthrough-interactive.md](./tls-passthrough-interactive.md) | TLS 不信任域名交互式 Passthrough | 8 | TLS 不信任 Toast 弹窗交互、Passthrough / Ignore 按钮、Notifications 表格操作、域名排除列表联动 |
| [tls-trust-detection.md](./tls-trust-detection.md) | TLS 信任检测改进（降低误伤） | 10 | 错误分类精细化（definite/probable/decrypt）、PossiblyNotTrusted 中间状态、MIN_DEFINITE 门槛、per-domain 追踪、WebUI 状态展示 |
| [chrome-devtools-remote-control.md](./chrome-devtools-remote-control.md) | Bifrost DevTools Remote Control | 42 | 显式裸 `devtools://` 规则驱动的代理页面发现、page_bridge 降级调试、bridge WebSocket 双向通信且页面侧无独立 HTTP 上报/轮询、WebUI session WebSocket 推送、服务端轻量路由且不保存完整调试历史、目标页有界 buffer、事件队列短延迟批量异步发送、服务端 bridge 消息重放去重、WebUI/目标页 live channel 有界队列保护、broker 忙碌时不阻塞代理主流程、shell E2E fixture HTTP server 端口探活与 cleanup PID 精确回收、HTTP fixture API 路由返回 200 避免 DevTools 等待超时、WebUI DevTools 侧栏入口使用稳定 data-nav-label 定位避免 locator 假超时、Linux 与 macOS aarch64 CI release artifact 内嵌真实 WebUI 避免 Frontend not built 占位页、WebUI DevTools tab 自有 Elements/Network/Cookies/LocalStorage/SessionStorage/Console 面板、Elements 目标页鼠标拾取节点、WebUI 自动展开选中对应 DOM row、目标页 overlay 展示节点名称/尺寸/Color/Font/Padding/Margin、Network 复用 Traffic 页面虚拟列表风格并内嵌复用 TrafficDetail 展示详情、Network 前端采集去重与 `x-bifrost-client-request-id` / 安全同源内部 query id 精准映射 Traffic、Network 点击详情最多约 10 秒重试等待 Traffic 映射落库、Admin broker 对 live network 与 snapshot 重放按 `client_req_id` 去重、Service Worker / 跨域标签资源不被内部 query 污染、Network 浏览器侧 status/query/request headers/response headers metadata 采集、动态标签资源 request id 与 status 兜底展示、Traffic 匹配失败时仍展示发起端基础信息、Network 搜索后点击匹配业务 URL 的具体虚拟列表行避免 fallback 详情概率性落到旧首行、Storage 大数据量虚拟列表与 tab 切换性能、Console 标准 `%c` 样式格式化、Console 纯文本按日志等级着色且对象展开树对齐、Console 对象展开点击在 CI 时序下重试到属性可见、DevTools 明暗主题切换、详情路由刷新恢复、紧凑详情页头、Traffic 跳转入口、URL hover 复制、content 区域填满剩余高度、右侧模块搜索、规则编辑器无参数智能提示、vConsole/Chrome DevTools 风格 Elements 树、超长值 120 字符预览与详情弹窗完整复制、存储行内新增/编辑/复制/删除、Chrome 风格 Console 底部多行输入、对象/数组结构化摘要与层级展开、原始内容一键复制、毫秒级低对比度时间戳、全屏 JavaScript 编辑器与 input/result/error 行、默认 Console evaluate 真实执行并展示远端 JS 异常、多页面切换、HTML candidate 幽灵页隐藏、静默但 WS 连接页面仍在线、目标页刷新后 WebUI session 自动恢复、移动 Safari UA 降级路径，以及 Chrome DevTools frontend 安装/托管/打开入口清理回归 |

### 远程调用测试

| 文件 | 功能模块 | 测试用例数 | 说明 |
|------|---------|-----------|------|
| [remote-invoke.md](./remote-invoke.md) | Remote Invoke 远程调用 | 182 | 发现模式与一次性授权码、人工授权、授权复用（30m/1h/1d/永久）、授权模式升降级、多客户端管理、有效期调整、移除授权、SSE/HTTP relay、大结果/大输入分片传输、主动取消、续流恢复、断线恢复、白名单命令全覆盖、端到端加密验证、客户端重启稳定性与 SSH route 三态同步、审计历史与过滤、第一阶段 relay v2 协议升级（`command_encrypted` / `exit_encrypted` / route-only metadata / query grant 拒绝 shell.exec / 本地与远端 relay 复测 / caller-client 双端加密 roundtrip / 真实 CLI 加密黑盒闭环 / grant crypto 持久化后 client 重启继续复用 / 同一远端重复 pair-code connect 后 caller 仍绑定最后一次 grant）、回归验证（含 SSE 事件去重 + 多实例 frame/exit 竞态 + 超时 pairing 自动清理 + 超时 pairing 不再占用 pair_slot_occupied 且审批不再报 500 + 过期 grant 自动清理 + 客户端侧 DELETE grant + relay_token 鉴权安全加固 + calls 路由迁移 + approve_pairing fingerprint 修复 + delete_grant best-effort 修复 + call_open grant 验证安全加固 + client 注册 token/challenge/签名校验 + caller 免 token 边界 + call detail 所有权隔离 + pairing decision 所有权隔离 + call frame/exit 所有权隔离 + remote traffic get sequence 映射 + remote traffic list 全量过滤参数透传 + remote traffic search `query/max_results/max_scan` 执行端透传 + remote search 流式输出 + remote search `max_results/max_scan` 执行端透传 + stderr 透传 + remote connect overload-protect 重试与提示 + grant_created/call_open 协议职责分离 + SSE 推送失败轮询容错 + pair_slot_occupied 自动清理 + pending-pairings API + relay URL 四级回退优先级 + caller identity 持久化 + SSH key 管理 API/导出/重置/撤销 + SSH 授权永久有效直到 key revoke + relay challenge/connect 最小闭环 + SSH grant relay 复用/openCall 能力验收 + revoke 后 route 收敛删除验证 + 线上 relay 的 SSH reusable/openCall 存储链路回归 + Remote Invoke 状态区合并布局回归 + Create SSH key 弹窗提示合并回归 + Shell Access 编辑器中 Policy/Profile ID 只读回归 + caller 主动取消后所有命令统一收敛为 cancelled + 本地 relay 粗粒度限流不再打断 cancel/events/exit 收尾 + 线上 relay 下 target client 取消终态稳定写入 + 共享出口 IP 下已认证 remote invoke 不再互相限流 + 远端 relay 不引入 pod-local authenticated remote limiter，`client/stream` 直接从认证结果补齐 `user_id` + relay 返回 `grant_not_found` 时 disconnect 仍删除本地连接，避免幽灵状态 + CLI `remote connect --ssh-key` 落盘与后续复用回归 + server-v4 SSH connect 挂起态持久化 caller_info，确保 SSH grant 展示调用方信息 + SSH key reset 后 worker 显式进入 reconnecting，避免 post-reset connect 命中假性离线窗口 + SSH key reset 后 worker 重连等待覆盖 CI 高并发窗口并输出诊断 + `call_cancel` 即使遇到本地句柄竞态也能把 Recent Calls 收敛到 `cancelled` + shell E2E 夹具与当前加密协议保持一致 + Recent Calls 参数预览/Tooltip 从本地解密 `args_json` 回退恢复 + client 本地 grant crypto 丢失后重连主动清理幽灵授权，caller 在 open_call 收到 grant_not_active 时也归一化提示 expired/re-connect 并清空 stale connection；后续 disconnect 回归需基于 fresh reconnect 继续验证 + shell E2E 在 `--skip-build` 且缺失 sync-server dist 时自动回退源码入口 + sync-server dist 陈旧时自动回退源码入口，避免 remote file / SSH / shell streaming 运行旧 relay + caller `open_call` 直接携带参数摘要，且 remote invoke shell E2E 仅在 release 过期时自动重建二进制 + pair-code connect 后 Client grants 列表在短时间内稳定可见 + Recent Calls 参数预览回归脚本改为本地 mock 流量，避免公网依赖 + Recent Calls 本地 mock fixture 端口 fallback 后继续使用实际端口 + Recent Calls 重启后从本地落盘恢复 + 清理全部记录 + Recent Calls 命令相关文本 200 字符截断与点击详情弹窗 + Grants 首次连接/最近命令时间字段展示且执行命令后首次连接时间严格稳定 + Remote Status 移除 Active Calls 重复计数 + Remote Status 未登录暗色主题 + Grants 连接方式展示 + Remote Status 未登录 Sync 暗色主题提示 + 未登录 sync session 时 relay 注册日志降噪）、补充覆盖（多调用方并发隔离/配对码轮换/并发冲突/traffic.clear 拒绝/once consumed/grant 上限）、全局授权弹窗（自动弹出/Dismiss/Dismiss All/Authorize 下拉/Reject）、远端部署（HTTPS/SSO/多用户并发/跨公网稳定性/大结果传输/断线恢复）、交互式客户端选择（多客户端未指定 --client-id 时弹出选择菜单/模糊前缀匹配多客户端/非交互环境回退报错） |
| [remote-invoke-sshkey.md](./remote-invoke-sshkey.md) | Remote Invoke SSH Key Caller Identity | 1 | 同一 SSH key 在多个 caller 沙箱中连接同一 target 时生成不同随机 caller ID，grant 以 caller ID 隔离且 SSH key fingerprint 仅作为密钥属性保留 |
| [skill-remote.md](./skill-remote.md) | Bifrost Remote Skill | 8 | install-skill 同时安装 `bifrost` 与 `bifrost-remote`，remote skill description 表达远程设备控制能力，目标端正式启动默认使用 `bifrost start` / 9900 / 系统代理，明确查询、shell、文件三类 scope 前置准备，当前 relay-backed 子命令边界正确指向 `remote exec` 操作目标设备，要求远端工程任务先读取 `AGENTS.md` / `.agents/skills` 元信息，不包含历史版本迁移文案，且不提供 `remote traffic clear` 写操作命令 |
| [remote-command-isomorphic.md](./remote-command-isomorphic.md) | Remote Command 同构化回归 | 30 | 本地与远端 `search/traffic` 命令矩阵回归：覆盖 search/traffic list/get/clear、remote search/traffic list/get 的子命令、参数、默认值、格式输出、流式输出、remote traffic clear 不暴露边界，以及 filter-only query / 机器可读输出 / CI shell shard 预构建二进制复用回归 |
| [remote-traffic-cli-enum-size.md](./remote-traffic-cli-enum-size.md) | Remote Traffic CLI 枚举体瘦身 | 3 | `RemoteTrafficCommands` large enum variant 回归：验证 `remote traffic list` 全量过滤参数解析、`remote traffic search` 参数透传，以及 clippy 不再报 `large_enum_variant` |
| [remote-shell-exec.md](./remote-shell-exec.md) | Remote Shell Exec | 30 | `bifrost remote command exec` 主链路回归：caller 不再传 `policy_id`、target 基于 grant binding 与本地 Shell Access 自动选择唯一策略、query/shell scope 隔离、策略未命中与歧义匹配拒绝、policy version 失效、`Full Access` / `Default Sandbox` 真实语义、grant 的 WebUI / CLI 编辑、target 本地 grant policy overlay 持久化、relay 仅保留最小 `grant_scope` 不存储具体策略绑定，reconnect 覆盖旧 grant / disconnect 清理残留 reusable grants 的验证；Windows shell_text Unix 路径 fallback（`/bin/bash` → `cmd`）与 UTF-8 编码处理（`chcp 65001` / PowerShell OutputEncoding）、`policy update` 命令原地更新策略元数据不破坏 grant 有效性、CLI 对裸 argv 输入的显式拒绝回归、长时间命令 stdout 流式输出回归、caller-to-client stdin frame 转发到 executor active session、真实 `remote exec --interactive` stdin 转发、`--pty` 真 PTY isatty 与 raw mode 恢复、stdin/PTY 首帧不丢失、Windows 流式 shell 输出 E2E 回归，以及对应真实链路回归 |
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
| [api-replay.md](./api-replay.md) | Replay API | 21 | 重放集合管理、请求 CRUD、执行重放、历史查看、路径前缀转发回归、响应 Body 规则 resMerge 回归、Replay request/response 规则覆盖回归、Replay 规则 Shell E2E 回归 |
| [api-group.md](./api-group.md) | Group API | 13 | 团队组列表/详情、团队规则 CRUD、权限校验 |
| [api-search.md](./api-search.md) | Search API | 16 | 全文搜索、搜索范围、过滤条件、分页、流式搜索 |
| [api-auth.md](./api-auth.md) | Auth API | 12 | 鉴权状态查询、登录、密码管理、远程访问开关、JWT 会话吊销 |
| [api-sync.md](./api-sync.md) | Sync API | 32 | 同步状态/配置/登录/登出/运行/Session，Env/Room/User 代理转发端点，CI/沙箱 token+URL 直登 |
| [api-misc.md](./api-misc.md) | Misc API | 32 | Syntax 语法信息、App Icon、WebSocket 连接、Audit 审计日志、Bifrost File 导入导出 |

### 代理核心功能测试

| 文件 | 功能模块 | 测试用例数 | 说明 |
|------|---------|-----------|------|
| [proxy-http-https.md](./proxy-http-https.md) | HTTP/HTTPS 代理 | 27 | HTTP 转发、HTTPS CONNECT、TLS 拦截、各类规则协议、模式匹配、host 路径前缀回归、host 精确路径不补尾斜杠回归、旧版 `^https://` path wildcard 兼容回归 |
| [proxy-socks5.md](./proxy-socks5.md) | SOCKS5 代理 | 5 | SOCKS5 基本代理、DNS 解析、HTTPS 透传、UDP ASSOCIATE 启动就绪回归、统一代理 UDP relay 端口 fallback 与 Windows ARM runner 并发回归 |
| [proxy-websocket-sse.md](./proxy-websocket-sse.md) | WebSocket/SSE 代理 | 8 | WebSocket/SSE 代理转发、帧/事件捕获、UI 消息面板、Replay WebSocket E2E 启动隔离与诊断、Frames API SSE 前置流量回归 |
| [proxy-rules-advanced.md](./proxy-rules-advanced.md) | 规则协议全量测试 | 72 | 40+ 规则操作协议：请求/响应修改、内容注入、控制、路由、脚本、高级特性（Values 引用、模板字符串、正则捕获），含 html/js/css 内容注入协议矩阵、htmlPrepend 插入 `<html>` 后、htmlAppend 插入 `</html>` 前，以及 HTTPS 转发到 HTTP 上游、gzip HTML 响应编码一致性、gzip JSON reqMerge/resMerge 合并回归、HTTPS 解包 gzip JSON reqMerge/resMerge 合并回归、脚本与 mock 路径压缩 Body 修改回归、mock 生成资源、通配域名根路径 htmlAppend 匹配、culture.shtml HTTPS MITM 背景图白屏、上游 HTTP/2 body 断流 fallback、无规则命中时已知长度响应头透明转发真实回归、reqHeaders Markdown value 中 `#` 注释行回归 |
| [bp-protocol-parser.md](./bp-protocol-parser.md) | BP 协议脚本解析 | 17 | `bp://<script>` + `decode://bp` 本地/远程 parser 解析，远程脚本下载缓存，远程下载超时/超大响应失败且不污染缓存，Traffic 详情 decoded body 展示，Body 面板 raw/decoded 切换且 raw 精确展示解码前二进制，`bifrost search` 与 Search SSE 搜索解析后内容，本地 parser 名称路径穿越拒绝，内置 `build_in_bp` 自动释放覆盖与规则编辑智能提示/hover 说明，`decode://bp` 内置校验、`bp://` parser 脚本列表补全、本地 parser 跳转到 Scripts 页面、query 高亮不污染后续协议、远端 URL 和绝对路径兼容，`build_in_bp.js` 对 next_agent PSM 的 BAM metadata/Thrift 双向二进制解包路径验证、默认 Bifrost sync token 换取 server `bam_token`，以及参考脚本/相关文档不暴露明文默认域名 |
| [proxy-auth-brute-force.md](./proxy-auth-brute-force.md) | 代理认证暴力破解防护 | 10 | HTTP/SOCKS5 代理认证 rate limiting：失败计数、10 次封禁（429/连接拒绝）、计数重置、IP 独立追踪 |
| [rule-merge-headers.md](./rule-merge-headers.md) | 规则合并 Header 覆盖 | 6 | reqHeaders/resHeaders 同名覆盖、路径深度优先级、真实代理场景验证、转发类无回归、两条同名 key 规则覆盖+客户端同名 header、HTTPS passthrough/tunnel 客户端同名 header 去重覆盖 |
| [rule-merge-strategy.md](./rule-merge-strategy.md) | 规则合并策略全量验证 | 13 | 全量协议合并策略验证：转发类 first-match-wins、Mock 类 non-multi_match、标量值 single-match、Body/CORS/注入 last-wins、累积型 accumulate、KV 集合、特殊协议、控制类、E2E 真实代理场景 |
| [rules-e2e-fixtures.md](./rules-e2e-fixtures.md) | Rules E2E Fixtures | 13 | replay 历史夹具 `__MOCK_HTTP_PORT__` 端口占位符、并行 runner 动态 echo 端口兼容，以及 Windows rules 共享 mock outage 后串行重试全部失败套件、suite 日志路径识别、timeout 诊断、CI 预算、bifrost-e2e admin 临时数据目录重复端口重跑隔离、macOS Rules CI 失败夹具语义回归、tunnel 请求侧规则一致性回归、urlParams `&` 分隔多参数解析回归、Rules CI harness 断言逻辑回归、Windows ARM Rules 慢平台 fixture timeout / CI 分片回归、Windows x86 Rules 4 分片 30 分钟 job envelope 预算回归和 Windows Rules CLI-only 构建依赖回归 |
| [rule-operators-audit-fix.md](./rule-operators-audit-fix.md) | 规则操作符审计修复 | 6 | forwardedFor/responseFor applier 实现、pac 未实现标记、test_rules.sh fake-pass 清理回归 |
| [mock-file-serving.md](./mock-file-serving.md) | Mock File Serving | 6 | file://协议二进制文件（PNG/图片）返回、JSON/HTML 文本文件、tpl://模板变量替换、Content-Type 自动检测、HTTPS TLS 拦截路径回归 |
| [traffic-cleanup.md](./traffic-cleanup.md) | 流量记录清理逻辑 | 7 | 记录数超 115% 触发清理到 80% 水位、清理期间新流量落盘、Body 缓存文件清理、磁盘总量清理 body 同步、过度删除回归验证 |
| [async-traffic.md](./async-traffic.md) | Async Traffic Writer | 2 | 异步流量记录写入、更新合并和跨批次处理的 CLI 驱动回归验证 |

### 网络与访问控制测试

| 文件 | 功能模块 | 测试用例数 | 说明 |
|------|---------|-----------|------|
| [cgn-address-support.md](./cgn-address-support.md) | CGN 地址段支持与同子网局域网判定 | 9 | RFC 6598 CGN 100.64.0.0/10 地址段识别、同子网判定、allow_lan 联动、IP 列表展示、边界值验证 |
| [network-refresh.md](./network-refresh.md) | 网络变化自动刷新子网信息 | 8 | VPN 连接/断开后子网自动刷新、WiFi 切换 IP 更新、访问控制策略实时同步、WebUI 实时推送 |

### 注入功能测试

| 文件 | 功能模块 | 测试用例数 | 说明 |
|------|---------|-----------|------|
| [badge-hover-panel.md](./badge-hover-panel.md) | Badge Hover 规则详情面板 | 13 | Badge hover 展开面板、规则列表展示、Merged Rules 折叠与一键复制、规则行跳转编辑页、高 z-index 浮层覆盖回归、暗色模式、缓存性能、禁用验证、Merged Rules HTML/Script 标签片段通用转义、误标 HTML 响应头的 JSON 数据接口不注入、Group 规则启用后缓存与代理运行时刷新、快速启停最终一致性 |

### 性能与内存优化测试

| 文件 | 功能模块 | 测试用例数 | 说明 |
|------|---------|-----------|------|
| [memory-sqlite-cache-optimization.md](./memory-sqlite-cache-optimization.md) | SQLite Cache Size 与内存优化 | 6 | SQLite cache_size 降低、读连接池缩减、metadata_cache LRU 化后的功能回归与内存验证 |
| [long-term-memory.md](./long-term-memory.md) | Long-term Memory 长期记忆系统 | 11 | 文件记忆目录、`raw_memories.md`/`rollout_summaries` 追溯文件、无数据库 bounded Phase 2 consolidation、文件锁、原子写、按需加载说明注入、关闭召回、`/remember` 文件追加、不创建 SQLite、Admin 文件 API、WebUI 文件视图、导入导出、真实对话接口自动生成并跨独立 Session 消费，以及自动记忆与真实对话 shell E2E mock 均和当前 Phase 1/Phase 2 prompt 对齐回归 |

### CI/DevOps 测试

| 文件 | 功能模块 | 测试用例数 | 说明 |
|------|---------|-----------|------|
| [ci-cross-build.md](./ci-cross-build.md) | CI Cross Build | 5 | PR CI 与 release workflow 的 Linux cross build 禁用 Docker buildkit，armv7 pre-build 使用 HTTPS Ubuntu 源和 apt retry，aarch64-musl cross build 对 GHCR 临时超时进行有限重试，避免 buildx/buildkit、HTTP apt mirror 或容器镜像拉取波动导致失败，并由远端 CI 验证 |
| [ci-shell-e2e-sharding.md](./ci-shell-e2e-sharding.md) | CI Shell E2E 测试分片优化 | 26 | --shard N/M 参数解析、环境变量透传、分片覆盖完整性、无分片向后兼容、local-ci.sh 分片支持、单分片在 CI 预算内完成、CI skip 列表、格式校验、系统代理用例 CI 禁跑且本地保留、隐藏日志 artifact 上传与失败摘要诊断、CLI offline help alternation 回归、CLI offline Broken pipe 回归、失败日志 dump pipefail guard 回归、unsafe_ssl 自带 HTTPS mock fixture 回归、并行 shell 调度器与顶层 final status 全 PASS 后返回 0 回归、SSE replay timeout 边界回归、macOS CI post-timeout 连接噪声回归、unsafe_ssl 管理端端口碰撞回归、long-term memory frontend build 竞争回归、Agent/IM human-api 并行端口隔离回归、remote relay fallback 预构建 binary 复用回归、Linux/macOS shell E2E timeout 预算回归、main push CI concurrency 取消旧 run 回归、Linux/macOS shell shard 内部并发预算回归、shell E2E 默认 Cargo 解析回归、site docs sync 缺失 site 依赖自举回归 |
| [ci-macos-cli-e2e-split.md](./ci-macos-cli-e2e-split.md) | CI macOS CLI/E2E 构建拆分 | 5 | macOS rules/shell E2E 仅等待 aarch64 CLI 构建，desktop bundle 与 x86_64 CLI 构建不阻塞 E2E，并确保 Tauri desktop bundle 使用真实 Cargo/Rustc 工具链 |
| [ci-windows-e2e-runner.md](./ci-windows-e2e-runner.md) | CI Windows E2E Runner | 3 | Windows `E2E Runner` 在 `cargo run -p bifrost-e2e` 前预安装 `rust-src`，E2E 入口显式绑定当前工具链 `rustc`，避免 rustup component conflict 和 Cargo/Rustc 版本混用，并通过最新 CI run 观察确认 |
| [skill-loading-e2e.md](./skill-loading-e2e.md) | Skill Loading E2E 一致性 | 11 | 4 scope 加载可见性、优先级覆盖、启用/停用一致性（管理端→消费端）、prompt 渐进式披露（metadata 注入、body 按需读取）、slash 命令解析、default_roots 路径对齐、隐藏目录过滤、嵌套发现、单元测试回归 |
| [linux-install-musl-fallback.md](./linux-install-musl-fallback.md) | Linux 旧 glibc 安装 musl 回退 | 4 | Debian 10 / glibc 2.28 自动选择 musl 预编译包，新 glibc 保持 GNU 包，npm/npx 平台包与 `bifrost upgrade` 同步回退到 musl |
| [codex-task-dispatch.md](./codex-task-dispatch.md) | Codex 异步任务派发 | 5 | 后台启动 Codex 任务、watch 最近任务、prompt 缺失报错、PATH 隔离无 codex 报错、同名任务历史产物不覆盖 |
| [codex-task-inspector.md](./codex-task-inspector.md) | Codex 任务巡检 Skill | 6 | 先探测 Codex 实际数据目录（优先 `CODEX_HOME`，否则 `$HOME/.codex`），再按 rollout/session id 读取权威 jsonl；仅在明确指向仓库追踪文件时走 `.codex-tasks`：PID 存活判断、`*-last.md` 结论提取、CI poll 运行中/失败识别，以及本地状态与 CI 状态分层汇总 |
| [daily-agent-records.md](./daily-agent-records.md) | Daily Agent Records | 2 | 运行记录页合并 processed state 与磁盘 report 目录，兼容历史 `Report` 大写目录；状态文件缺失时仍展示已有报告，状态文件存在时保留元数据并补齐 report_path |
| [asr-scheduled-task-plan-b.md](./asr-scheduled-task-plan-b.md) | ASR 定时任务 Runtime 策略 | 35 | runtime_strategy 默认 reuse_per_file、fork_per_chunk/reuse_server/reuse_per_file/auto/compare 对照实验、0字节跳过、chunk进度实时更新、串行处理、chunk失败重试继续并刷新 transcript/timeline/metadata/files/daily docs、WebUI 批量排队重试所有 failed chunks、ASR jobs 模块拆分后 API 行为不变、Daily Agent Runner 方案文档验收与实现闭环回归（Bifrost Agent 可执行、report 缺失不写 processed、retry 刷新 daily 后触发 Runner、任务创建初始化 daily workspace、ChatGPT Web 固定 conversation 首轮/后续投递差异、IM provider/target 绑定、WebUI Runner 单下拉复用 Runners 配置、IM Channel 单下拉复用通道配置、默认目录 live WebUI 切 Runner/配 IM Channel、默认目录 live ASR Daily Agent 跑通 bifrost_agent、codex、web 并产出 report、默认目录 live IM Channel 实发成功且 self-call 读取 runtime.json 端口、ChatGPT Web 大输入 paste 路径真实通过、默认目录多文件 ChatGPT Web Daily Agent 按日顺序处理、FullReport 原文分片且不降级摘要、Processed Documents report 可点击全屏 Markdown 详情并支持路由刷新恢复、Agent Instructions 自适应高度且无内部滚动条、WebUI lint 修复、daily_agent.rs 小于 1500 行）、native ASR 与托管 asr-server 模型感知内存保护、memory-limit hint 复用、强制 pause 立即释放 ASR/ffmpeg 子进程、长音频逐 chunk 切片、30 分钟真实音频性能基准、文件开始时间/执行耗时展示、服务重启后孤儿 processing 自动恢复 pending、任务详情默认优先展示未完成文件并支持 Processing/Pending/Completed/Failed/All 状态筛选、reuse_per_file 服务死亡后自动切到 fork_per_chunk、chunk_metrics 与 fallback_reason 日志/状态证据、批量结果、运行中 pause/resume 资源让路 |
| [qwen3-asr-local-server.md](./qwen3-asr-local-server.md) | Qwen3-ASR 本地 API Server + WebUI | 20 | Apple Silicon/32GB 依赖检查、Rust 通用下载模块断点续传与后台进度、Qwen3-ASR-1.7B 非交互初始化、Qwen3-ASR-0.6B 初始化下载绕过环境代理回归、WebUI/CLI 启动服务自检自动修复缺失资源和 FFmpeg、CLI/API 转写、长音频切片、`bifrost ai asr` 服务控制与单文件标准输出、`bifrost ai asr task` 检查目录定时任务/文件/按日 Markdown 文档并支持 runtime 默认端口、AI Tools ASR 异步初始化状态/下载进度/错误、刷新后重连初始化流继续显示后台下载任务、WebUI 单卡片文件输入/转写布局、文件进度仅在文件转写时展示、麦克风 WebSocket 实时入口、麦克风实时电平音轨、ASR 目录 hourly/daily/weekly/monthly 定时任务、Directory Tasks 在首页前移到 Speech Converter 下方、点击任务进入 ASR 子页面查看逐文件结果/录音元信息，任务文件表格不撑出页面且分页大小切换受控，Daily Docs tab 按天展示聚合 Markdown 并可点入完整内容页，点击已处理文件进入单文件详情播放源音频并双向绑定 timeline：每个 segment 最大 30 秒、点击字幕时间点跳转音频，播放或拖动音频时字幕自动高亮滚动，手动滚动字幕时自动跟随暂停并在 5 秒无操作后恢复，暂停期间用户操作音频播放轴或点击字幕时间点会立即恢复自动跟随、ASR 任务详情展示原音频占用并一键清理已成功转写原文件、Daily Agent 配置执行与运行记录拆分为平级 tab、API WebSocket 实时转写、缺失模型文件下载进度、CI 禁止模型下载部署 |
| [docs-site-generator.md](./docs-site-generator.md) | Docs Site Generator | 5 | 文档站点同步完整性、未来新增 docs 文档自动纳入、真实 Astro/Starlight 部署构建路径、历史深链重定向、全站站内链接和部署产物清理验证 |
| [utf8-safe-preview.md](./utf8-safe-preview.md) | UTF-8 安全 Preview 截断 | 3 | Agent compaction tool arguments、IM Gateway 任务输出、CLI/API/E2E 错误 preview 在中文/emoji 多字节边界截断时不触发 char boundary panic |
| [web-lint-cleanup.md](./web-lint-cleanup.md) | Web ESLint 清理 | 2 | web 全量 ESLint 零错误零警告与 TypeScript/Vite build 未退化 |
| [storage-e2e-safety.md](./storage-e2e-safety.md) | Storage and E2E Safety | 3 | temp-env 作用域编译回归、core size guard 单元回归、storage rules size guard 编译回归 |
| [agent-development-review-loop.md](./agent-development-review-loop.md) | Agent Development Review Loop | 7 | Agent 开发任务至少两轮目标复核、代码 review、修复问题、测试运行、结果复盘闭环，持续改进引导语、任务模式判定、任务启动工作区检查、并行开发优先 worktree 隔离、证据台账、完成定义、用户目标验证清单、git diff/status 复核、测试失败归因、最终交付验证矩阵，以及 AGENTS/design/human_tests 索引同步 |
| [agent-codex-alignment.md](./agent-codex-alignment.md) | Agent Codex Alignment | 8 | 默认 prompt 不泄露兼容实现说明、MCP resource canonical 工具名、shell_command/local_shell 历史 alias 已移除、真实 Bifrost 服务 `/agent/chat` 覆盖 MCP resource / update_plan / set_title / tool_search / 并发工具批、turn events、FuturesOrdered 并发工具批与 history 顺序回填、CI 预构建 release binary 回归、P1 工具链回归 |

### IM Gateway 测试

| 文件 | 功能模块 | 测试用例数 | 说明 |
|------|---------|-----------|------|
| [im-gateway.md](./im-gateway.md) | IM Gateway 网关模块 | 55 | AI 一级页内 IM Gateway 子导航、Handler 模块化拆分回归、CI Windows Rules timeout 回归、Connections Provider 单列卡片与关键字段 hover 复制、窄宽度布局滚动回归、CLI im 命令、API CRUD（Provider/Target/Route/Schedule/History）、WebUI 渲染、创建 Provider 时 app_secret 正确保存且响应脱敏、Display Name 可省略、创建后立即连接并通知 owner 且通知包含 Provider 自定义工作目录、两个飞书机器人 token 缓存隔离、编辑 Provider 补填 App Secret、重复 Provider ID 错误展示、owner_open_id 安全过滤、Outbound/Inbound 消息记录、WebSocket 长连接、OK Reaction、Schedule 手动执行与结果发送、Schedule 手动新增 Script/Agent、Schedule 详情与运行历史、Schedule 绑定消息通道、真实 Agent Schedule 使用绑定 IM 通道发送消息、Schedule Agent 可选择 Runner、配置默认执行目录、持久化 ChatGPT/Codex 对话引用与 ChatGPT Web 初始化 Prompt、History Task Runs 完整详情、Agent Chat 创建和更新 Script/Agent 定时任务、Agent schedule CRUD 工具、CLI messages 命令（list/clear/direction/source 筛选）、IM CLI 未传 provider 时选择 provider 并默认发送给 owner、图片消息上传发送、图文卡片便捷发送、原始 card JSON 兼容、Agent Markdown 图片自动上传/远端文件附件下载保留与 IM 通道发送 |
| [im-gateway-agent.md](./im-gateway-agent.md) | IM Gateway Agent 对话能力 | 106 | AI 一级页整合 Agent 与 IM Gateway 子导航、Agent 配置 API、分层 prompt、Sessions/History/WebUI、飞书消息触发 Agent、多轮上下文、/clear、/status、/stop、MCP/Skills/AGENTS.md、Provider 级 Agent 配置、动态工作目录、Agent 默认发送通道、send_msg 默认消息通道、send_msg 统一工具与 schedule 绑定消息通道、send_msg 默认通道真实链路、真实用户默认 IM 通道 send_msg 模型兼容链路、Agent 模型 reasoning 参数可在 WebUI 关闭、Provider Connection 与 Memories 默认值 placeholder、Provider 卡片详情布局与操作按钮去重、Runtime Settings 恢复默认值、retry 与 orphan tool 防回归、飞书卡片折叠面板与流式进度卡片（Agent loop CardKit streaming progress、标题跟随 set_title、可选 plan/tool/thinking 模块、plan 标题显示当前任务、thinking 标题显示一行摘要、最终输出置底、guide/queue 同卡刷新折叠状态区并在标题给出轻量可见反馈）、Session Title、Goal 模式、图片多模态理解、边界测试与回归（CI E2E 启动器、tool message 序列、默认 Bifrost 代理入 Traffic 等） |
| [im-gateway-external-cli-chat-gateway.md](./im-gateway-external-cli-chat-gateway.md) | Agent Custom Runner / Chat Gateway | 19 | HTTP Chat Gateway 触发自定义 CLI Runner、Codex adapter 命令契约、真实 Codex CLI 调用、非 Codex adapter run pipeline、run artifacts/progress events、全局默认配置、单 IM 通道覆盖、effective config 来源预览、NDJSON stream、stream start/finish 去重、Codex warning 不误报 run_failed、run stop marker、work_dir allowlist、ExternalCliAgentChat route action、默认不发送真实 IM、Agent Runners WebUI、Provider/Global 工作目录降级与 Codex `--cd` 注入、亮色/暗色主题 |
| [im-help-command.md](./im-help-command.md) | IM /help 命令帮助信息 | 3 | /help 返回所有可用命令列表及描述、不再返回"未知命令"、真正未知命令仍报错 |
| [im-guide-queue-mode.md](./im-guide-queue-mode.md) | IM 引导模式和排队模式 | 14 | SessionQueueManager 单元测试（14项，含多 guide 累积、pending status、turn-end guide drain 与 guide 优先于 queue 回归）、guide_channel 字段集成、服务启动、API 验证、handle_busy_message 路由（/q 排队、/rq 删除、默认引导）、tokio::select! 交错处理、并发事件路由、mid-turn 注入、`/agent/chat` 注入式 guide/queue 黑盒真实链路（多 guide `/status` 明细与合并消费、turn-end guide、FIFO drain、guide 优先、空白忽略）、全量测试 |
| [im-markdown-converter.md](./im-markdown-converter.md) | IM Markdown 格式转换器 | 10 | 标准 CommonMark → 飞书卡片 Markdown 转换：代码块语言标准化、图片 URL 转文字链接、任务列表 emoji 替换、水平分割线统一、HTML 标签过滤、UTF-8 多字节字符兼容、代码块内容保护、Bold+Italic 组合、脚注处理、综合场景 |
| [agent-builtin-commands.md](./agent-builtin-commands.md) | Agent 内置命令全面测试 | 34 | 11 个内置斜杠命令全覆盖：/help、/status、/clear、/reset、/undo、/compact、/remember、/memories、/forget、/resume、/skill，含无参数边界、未知命令、/resume 空会话回归、并发忙碌时 session-free 立即响应，以及 /status 工作路径、运行中 loop/token/context/压缩次数实时指标、默认 250k context window、/agent/chat /status 纯读不抢占 session、/agent/chat 首条 /status 保留请求 work_dir、/status 抢先创建空 session 后业务 turn 覆盖 work_dir 回归、工具结果追加后 context 增量口径和 CI 高负载采样窗口加固、自动压缩判断使用 last usage + appended items、emergency compaction 统计事件完备、guide/pending queue 继续 loop 前压缩、history 改写后 active status 立即刷新、replacement history summary-last 与非 system initial context reinjection、Codex local compaction 模板/text-only/token budget 对齐、summary 生成请求使用 structured history + user prompt、summary 请求超上下文后移除最老 history item 并重试、summary transient error 按 provider retry budget 退避重试、base instructions token snapshot、mid-turn compact 后 base 不进 history 且非 system context/memory 不重复注入、普通 turn pre-sampling `PreTurn + DoNotInject` 自动压缩 |
| [agent-builtin-tools-completeness.md](./agent-builtin-tools-completeness.md) | Agent 内置工具完备性 | 17 | `exec_command` 短命令、长任务真实进程 session + `write_stdin` 轮询到最终 exit code、交互任务 session + `write_stdin`、默认不注册且源码删除 `shell`/`shell_pty`、运行时拒绝 legacy/未暴露终端协议字段、真实 `/agent/chat` 工具列表和 base instructions 只暴露/推荐 `exec_command`/`write_stdin` 终端协议、`exec_command tty=true` 真实 PTY + Codex CLI interactive 回归、真实 `/agent/chat` 由真实模型调度 PTY 与 Codex interactive 追加引导问题、delegated/交互/长任务请求必须使用 `exec_command` + `write_stdin`、真实 `/agent/chat` 通过 `exec_command` 启动 Codex CLI 创建宣传网页并追加引导消息、`view_image` data URL、`request_user_input` 不可交互边界、`tool_search` deferred 暴露、workspace all-features 编译回归、真实 Bifrost chat 默认直暴工具调用、本地 CI 静态门禁、MCP `>= 100` 阈值 deferred loading、真实 Bifrost 注册 100 个 MCP tools 后搜索并调用 |
| [agent-p1-tools.md](./agent-p1-tools.md) | Agent P1 Tools 对齐 | 6 | `/goal` 显式入口、Goal 生命周期、`apply_patch`、raw patch body 兼容、`exec_command` + `write_stdin` 统一终端会话复用与交互输入、真实 exit_code 收敛、`bifrost-agent` 全量回归 |
| [update-plan.md](./update-plan.md) | Update Plan 工具 | 3 | 真实 Bifrost + Admin API + mock model server 黑盒验证 update_plan 工具注册、runtime 强制收口未完成计划、`plan_steps` 最终返回与 helper 回归测试 |
| [agent-loop-timeouts.md](./agent-loop-timeouts.md) | Agent Loop Runtime Limits | 3 | 真实 Bifrost + Admin API + mock model server 黑盒验证默认 600 秒级超时、1000 次迭代上限，以及 35+ 次工具调用不会在 30 次时提前中断 |
| [agent-session-persistence.md](./agent-session-persistence.md) | Agent Session 持久化 | 15 | Session JSONL 文件生成、事件类型完整性（session_start/user_message/assistant_message/tool_call/tool_result）、跨 turn 复用 recorder、History 列表/详情/删除 API、WebUI 事件时间线查看与删除、Sessions 列表 title/整行点击进入详情、详情页 Messages/Settings Tab 与内容区域真实滚动、暗色主题兼容、恢复持久化 session 后继续 tool loop 回归 |
| [agent-runtime-review-fixes.md](./agent-runtime-review-fixes.md) | Agent Runtime Review Fixes | 3 | AgentSession 自动装配 SkillRegistry、MEMORY.md 并发 append 文件锁、system prompt 注入有界 Skill 摘要 |
| [agent-skills-admin-cli.md](./agent-skills-admin-cli.md) | Agent Skills Admin and CLI | 3 | Skill import multipart/bytes 接口、AgentSkillError 错误码分层、IM CLI secret 缺失错误 |

### Agent MCP 协议模块测试

| 文件 | 功能模块 | 测试用例数 | 说明 |
|------|---------|-----------|------|
| [mcp-oauth.md](./mcp-oauth.md) | MCP OAuth | 1 | OAuth token store Auto 模式 keyring 可用性 roundtrip 检测与文件 fallback 回归 |
| [mcp-elicitation-resources.md](./mcp-elicitation-resources.md) | MCP Elicitation 与 Resources 协议 | 11 | 类型序列化、策略判断、Handler 行为、PauseState RAII、Codex canonical resource 工具定义、trait 解耦、缓存行为、错误处理 |
| [mcp-servers.md](./mcp-servers.md) | MCP Servers 可用性状态 | 3 | Settings -> Agent -> MCP Servers 页面进入自动检查 configured MCP 可用性，展示 available、unavailable、disabled 差异和暗色主题可读性 |

---

**总计：98 个测试文件，1685 个测试用例**

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
