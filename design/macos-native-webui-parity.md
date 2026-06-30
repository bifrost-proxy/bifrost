# macOS Native WebUI Parity 方案

## 目标

把 `apps/macos` 从 Native Preview 推进为 WebUI 能力对齐的原生 macOS 控制台。Native 端必须复用现有 Rust `bifrost` 服务、默认数据目录和 Admin API，不使用假数据、不另起孤立服务、不用 WebView 整页包 WebUI；页面交互、数据语义、错误态和持续更新能力以 `web/src` 为准。

本方案的验收标准不是"长得像"，而是每个 WebUI 能力都有 Native 入口、真实 API 读写、状态同步、错误反馈和可复测证据。

## 信息源

- 路由与页面入口：`web/src/App.tsx`、`web/src/components/Layout/index.tsx`。
- 全局壳层：`web/src/components/Toolbar/index.tsx`、`web/src/components/StatusBar/index.tsx`、`web/src/components/FilterPanel/index.tsx`、`web/src/components/FilterBar/index.tsx`。
- Network：`web/src/components/TrafficTable/VirtualTrafficTable.tsx`、`web/src/components/TrafficDetail/index.tsx`、`web/src/stores/useTrafficStore.ts`、`web/src/stores/useSearchStore.ts`、`web/src/stores/useFilterPanelStore.ts`。
- 业务页：`web/src/pages/Replay`、`web/src/pages/Rules`、`web/src/pages/Values`、`web/src/pages/Scripts`、`web/src/pages/AI`、`web/src/pages/DevTools`、`web/src/pages/Groups`、`web/src/pages/Notifications`、`web/src/pages/Settings`。
- API 契约：`web/src/api/traffic.ts`、`web/src/api/rules.ts`、`web/src/api/values.ts`、`web/src/api/scripts.ts`、`web/src/api/replay.ts`、`web/src/api/config.ts`、`web/src/api/proxy.ts`、`web/src/api/breakpoint.ts`、`web/src/api/metrics.ts`、`web/src/api/sync.ts`、`web/src/api/notifications.ts`、`web/src/api/group.ts`、`web/src/api/devtools.ts`、`web/src/api/cert.ts`、`web/src/api/remoteInvoke.ts`、`web/src/api/imGateway.ts`、`web/src/api/asr.ts`、`web/src/api/videos.ts`、`web/src/api/bifrost-file.ts`。
- 推送与持续更新：`web/src/services/pushService.ts` 以及各 store 的 push/polling 逻辑。

## 范围

### P0: Native Shell 与共享服务基础

- App 名、Dock、窗口和 Console 均为 `Bifrost`。
- 启动优先复用默认数据目录中已有 `bifrost` daemon；没有服务时再启动共享默认数据目录 daemon。
- macOS 窗口使用 full-size content titlebar。标题栏高度不单独浪费；右侧承载全局开关和当前页面工具按钮。
- 左侧菜单为固定窄竖栏，贯穿窗口顶部和底部；macOS 红黄绿窗口按钮位于竖栏顶部，菜单项使用图标在上、文字在下的紧凑样式，不再支持展开/折叠。菜单顺序与 WebUI 一致：Network、Replay、Rules、Values、Scripts、AI、DevTools、Groups、Notify、Settings；Groups 按 Sync 状态显隐。
- 左侧底部提供浅色/深色主题切换；主题需要覆盖所有页面、弹窗、表格、编辑器、空态和错误态。
- 底部状态栏保持 WebUI 的矮状态条能力：Proxy、Sync、TLS、上下行速率、Total、Conn、Req、Mem、CPU、Uptime、Version、Skill。
- 统一 Admin API client、CSRF/header、错误解析、loading/empty/error 状态、toast/alert 反馈。
- 统一 pushService 等价层，支持 traffic、metrics、settings、breakpoint、sync 等订阅范围。

### P1: Network 工作台

- 顶部工具区：过滤面板开关、清空、协议/类型/状态/规则/Imported tags、Add Filter、Fuzzy Search、Breakpoint、TLS Decode、System Proxy、详情面板折叠/展开。
- Filter panel：Client IP、Applications、Domains、可折叠 section、搜索、pin/filter、宽度记忆。
- Traffic table：虚拟滚动或 NSTableView 高吞吐实现；列至少覆盖 sequence、状态点、Protocol、Method、Status、Client、Port、Rules、Host、Path、Type、Size、Time、Duration；支持选择、多选、键盘导航、右键菜单、load more、auto-scroll、新流量提示。
- Traffic detail：请求/响应上下分屏、概览、Headers、Body、Raw、Cookies、Query、Messages、Script logs；body 懒加载、raw/decoded 切换、搜索、JSON/文本/图片/SSE/OpenAI-like 视图。
- Detached detail：支持详情独立窗口或原生等价交互。
- Breakpoint：全局开关、暂停请求/响应展示、编辑 body 后 resume。
- Search：fuzzy search、scope/filter、load more、cancel/reset 与 URL/persistent state 对齐。

### P1: Rules / Values / Scripts

- Rules：规则列表、组选项、搜索、选择、创建、重命名、启停、删除、reorder、share link、编辑器、保存、语法错误、引用候选、未保存提示、push 后列表同步。
- Values：列表、搜索、选择、创建、重命名、删除、编辑、保存、未保存提示、push 后同步。
- Scripts：request/response/decode/parser 类型切换、树/文件夹、多选、右键菜单、创建、重命名、删除、导入/导出、编辑器、保存、测试、sandbox 配置、测试结果与日志面板。
- 编辑器能力：Native 优先使用 AppKit text editor；如果 Monaco 级别能力暂不可达，必须列出缺口并提供等价快捷键、诊断和高亮计划，不得用纯占位替代。

### P1: Replay

- Composer / History 模式切换。
- Saved requests、groups、history 列表、搜索、计数、分页、移动、删除、清空。
- 请求编辑：method、URL、headers、query、cookies、body、form、timeout、rule config。
- 执行、取消、streaming、SSE/WS 消息、响应详情、从 Traffic 导入、从 History 复用。

### P2: Settings 与系统能力

- Settings tab 对齐 WebUI：Proxy、Certificate、Metrics、Access Control、Performance、Tray、Sync、Remote Access、Remote Invoke。
- Proxy：desktop port、system proxy、launchd/lifecycle helper、CLI proxy、proxy address、QR/copy。
- TLS/Certificate：TLS interception、unsafe SSL、disconnect on config change、include/exclude domain/app、CA 信息、本机安装、移动设备/QR/profile/trust probe。
- Metrics/Performance：overview、history、app/host metrics、record/body/cache/breakpoint 等配置滑块、clear body cache。
- Sync：启用、auto sync、remote base URL、sign in/out、run now。
- Tray：启用、系统 stats、单项 stats 开关。
- Remote / Remote Invoke：授权、pairing、grants、calls、shell config、file access policy、SSH key。

### P2: Notifications / Groups / DevTools

- Notifications：all/TLS trust/pending auth/client trust tabs、unread、status filter、mark read、dismiss、TLS passthrough/approve/skip、client trust 操作。
- Groups：managed/joined/discover、搜索、创建、公开/私有、详情、成员、邀请、权限、leave/delete、group rules。
- DevTools：页面列表、session、snapshot、Elements、Network、Storage、Console、inspect mode、目标页 WS 恢复、Traffic 映射、存储编辑、console evaluate。

### P3: AI / Agent / IM Gateway / Speech / Video

- AI 页按 WebUI 分组对齐 Agent、IM Gateway、Tools。
- Agent：model/provider/runtime 配置、chat/runner 入口、任务/历史/状态相关页面能力。
- IM Gateway：provider、target、route、schedule、history、外部 CLI config/run。
- Speech/ASR：capability gating、service start/stop、tasks、timeline、daily docs、external import、voice wake、speaker profile。
- Videos 与 `.bifrost` import/export 若 WebUI 对当前发布暴露入口，则 Native 必须提供入口或明确暂不暴露并在验收矩阵标红。

## 页面能力矩阵

| 页面 | Native 必须交付 | 验证证据 |
| --- | --- | --- |
| Shell | full-size titlebar、固定窄竖栏、窗口按钮嵌入菜单栏、主题、状态栏、全局开关、共享 daemon | Native 截图、System Events 窗口检查、`bifrost status --format json` pid/data_dir 对比 |
| Network | Toolbar、Filter panel、Table、Detail、Search、Breakpoint、Clear、Push 更新、轮询 fallback | WebUI/Native 对照截图、Admin API 记录数对比、`/api/push` connected 断言、真实请求流量回放、键盘/右键交互录像或截图 |
| Rules | CRUD、enable、reorder、share、editor、unsaved、push | Admin API rules 对比、创建/保存/启停/删除真实规则后 WebUI 与 Native 双向可见 |
| Values | CRUD、rename、editor、unsaved、push | Admin API values 对比、Native 写入后 WebUI 读取一致 |
| Scripts | 类型、树、多选、编辑、测试、日志、import/export | script API 对比、测试结果截图、导入导出文件校验 |
| Replay | composer/history/groups、execute/cancel、stream、import from traffic | 真实 mock server 请求、history 记录、response body/headers 断言 |
| Settings | 9 个 tab 的读写与错误态 | 每个 tab 至少一个读写用例，系统代理/TLS 操作使用安全临时步骤 |
| Notifications | unread/status/tabs/action | pending/mark read/action API 断言 |
| Groups | list/create/detail/member/rules | Sync enabled 环境截图与 API 断言；不可用时记录 gating 证据 |
| DevTools | pages/session/elements/network/storage/console | 目标 HTML fixture、WS session、console evaluate 断言 |
| AI | Agent/IM/Speech 能力入口与 gating | capability API、配置读写、最小真实 run 或明确环境阻塞 |

## 当前实现状态（2026-06-30）

- Shell / Network：已实现共享 daemon、标题栏工具区、固定窄竖栏、macOS 窗口按钮嵌入左侧菜单区域、明暗主题、真实 REST 初始数据、`/_bifrost/api/push` WebSocket、SSE header 探测、Network 选中详情与 Request/Response / Overview/Header/Body/Raw 分段。Network table 已完成高频性能路径重构：使用 `NSTableView.makeView(withIdentifier:)` 复用 cell、`TrafficRowViewModel` 预计算显示文本/颜色、AppModel 按 16ms batch 合并 WebSocket delta 并用 id 索引增量落表、append/remove/update 优先走 `insertRows`/`removeRows`/`reloadData(forRowIndexes:columnIndexes:)`，相同行序更新只为真实变化行重建 view model，避免每次 SwiftUI 更新都全量 `reloadData()` 或全量重算。仍需继续补齐右键菜单、detached detail、body 专用渲染、breakpoint 编辑 resume、load more、键盘多选等 WebUI parity 交互。
- Rules：本轮已补齐原生核心 CRUD，包括真实列表、选择详情、新建、保存、启停、重命名、删除确认、复制、Revert、未保存标记，并由 `--check-admin-data` 的 `crud=rules,values,scripts` smoke 覆盖 create/update/enable/rename/delete。仍未完成 reorder、share link、import/export、组规则、语法诊断、引用候选和 push 后细粒度同步。
- Values：本轮已补齐原生核心 CRUD，包括真实列表、新建、编辑保存、格式化 JSON/XML、复制、重命名、删除确认、Revert、未保存标记，并由同一 smoke 覆盖 create/update/rename/delete。仍未完成 import/export、多选右键和 WebUI 级高亮。
- Scripts：本轮已补齐 Request / Response / Decode / Parser 类型切换、真实列表、编辑保存、新建、重命名、删除确认、复制、Revert、未保存标记，并由同一 smoke 覆盖 Request script create/update/rename/delete。仍未完成树/文件夹、多选右键、测试结果/日志面板、sandbox 配置、import/export。
- Replay / AI / DevTools / Groups / Notify：目前仍是 API-backed 状态页，不是 WebUI parity；后续必须逐页替换为真实交互页面。

## 实现工作流

1. 每个页面先做 WebUI 能力审计：源码路径、路由、store、API、push、主要组件、键盘/右键/弹窗交互。
2. 为该页面补 Native typed DTO/API client，不直接把 JSON 映射到临时字典后拼 UI。
3. 先实现读路径和空态/错误态，再实现写路径和 destructive action。
4. 每个写操作必须在 Native 和 WebUI 之间做双向验证：Native 写，WebUI 读；WebUI 写，Native push/poll 后更新。
5. 所有列表页先做真实数据，再做细节交互；禁止用 sample rows 占位。
6. UI 细节按 WebUI 对照，但使用 macOS 原生控件表达：toolbar 用 icon button/switch/segmented control，表格用 NSTableView 或等价高性能组件，弹窗用 sheet/dialog。
7. 每个页面完成后更新本矩阵和 `human_tests/macos-native-webui-parity.md`，再进入下一页。

## Network 表格性能架构

目标是让 macOS Native Network 列表在真实高频流量下明显快于 WebUI，而不是复刻 WebUI 的 DOM/virtual list 成本。

### 已实施路径

- `NSTableView` cell 复用：每列固定 `NSUserInterfaceItemIdentifier`，通过 `makeView(withIdentifier:)` 复用 `TrafficCellView`，滚动时不反复创建 view hierarchy。
- 行视图模型预计算：`TrafficRowViewModel` 在数据进入表格层时计算 sequence、protocol/method/status tag、status dot、client label、rules badge、size/time/type 等展示值，cell draw 只消费 view model。
- 数据增量合并：`AppModel` 维护 `trafficRecordIndexById`，WebSocket `traffic_delta` 先进入 16ms pending batch，再按 id 原地 merge 或 append；append-only 请求不再把全量 `trafficRecords` 字典化再 sort，也不会逐条 push 触发 SwiftUI update。
- 差量 UI patch：append 走 `insertRows`，尾部删除走 `removeRows`，行更新走 `reloadData(forRowIndexes:columnIndexes:)`；相同行序的 update 先比较原始 `TrafficRecordSummary`，只有真实变化行才重建 `TrafficRowViewModel`。
- 图标缓存：client app icon 使用 `NSCache`、miss cache 和异步解析；cell draw 只读取缓存或 placeholder，加载完成后只刷新可见 Client 列，避免滚动绘制路径重复扫描 `/Applications`。
- 浮动 New Traffic 胶囊独立于表格行渲染，不参与 cell layout。
- 10 万行 smoke：`Bifrost --check-traffic-table-performance` 构造 100k synthetic rows、追加 1k rows、稀疏更新 100k rows 中的真实变化行，校验行数和 changed row bookkeeping，并输出 build/append/update 毫秒级耗时。
- Instruments 基线：交互验收时继续用 Time Profiler / Animation Hitches 采集滚动帧与主线程 commit，作为性能证据，不作为未完成代码路径。

## 验证方法

### 静态验证

- `rg "sampleRows|REQ-preview|api.local|Native preview rule editor scaffold|\\.constant\\(true\\)" apps/macos/Sources/Bifrost`
- `swift run --package-path apps/macos BifrostNativeCoreChecks`
- `swift run --package-path apps/macos Bifrost --check-icon`
- `swift run --package-path apps/macos Bifrost --check-admin-data`，必须同时输出 REST 数据摘要、`push_client_id=` 和 `sse_streams=...text/event-stream`，证明 Native 已连接 WebUI 同源 `/_bifrost/api/push` WebSocket，并能建立 WebUI 使用的 SSE stream。
- `swift run --package-path apps/macos Bifrost --check-traffic-table-performance`，必须输出 `base_rows=100000 append_rows=1000 changed_rows=...`，证明 10 万行 view model 构建、append 和稀疏 update bookkeeping 正常。
- `scripts/build-macos-native.sh --skip-sidecar --test`

### WebUI 基准采集

- 使用真实服务打开 `http://127.0.0.1:9900/_bifrost/`。
- 对每个路由采集：默认态、主要弹窗/菜单、空态、错误态、深色主题、窄窗口。
- 截图命名固定为 `/tmp/bifrost-webui-<page>-<state>.png`。
- 同时记录该页面用到的 Admin API 响应摘要，避免只靠视觉判断。

### Native 对照验证

- 使用 `open -n apps/macos/.build/Bifrost.app` 启动真实 `.app`，不启动裸 Mach-O。
- 对每个页面采集 `/tmp/bifrost-native-<page>-<state>.png`。
- Native 与 WebUI 对照检查：布局层级、控件位置、可用操作、数据一致性、loading/empty/error、暗色主题、固定窄竖栏、窗口缩放。
- 对写操作使用 API 断言最终状态，并在 WebUI 刷新后确认一致。

### 真实链路验证

- Network/Replay 使用本地 mock HTTP/WS/SSE fixture 产生真实流量。
- Network 必须同时验证 REST 初始列表、WebSocket `traffic_delta`/`traffic_deleted` 自动同步、SSE stream/header 探测和 WebSocket 失败时的 polling fallback；状态栏不能写死同步成功。
- Rules/Values/Scripts 使用默认数据目录服务的 Admin API 做真实 CRUD，并清理测试对象。
- Settings 涉及系统代理、TLS、证书、托盘时必须遵守开发安全边界：除明确测试该能力外，默认不改系统代理、不拉起托盘、不打开 Sync 登录页。
- DevTools 使用受控 HTML fixture 页面，不依赖外部网站。
- AI/ASR/IM Gateway 若能力受平台、模型或账号限制，必须通过 capability/status API 证明 gating，而不是隐藏失败。

### 完成门禁

单个页面只有同时满足以下条件，才能从矩阵标记为 done：

- WebUI 能力审计完成，源码和 API 路径明确。
- Native 无假数据，占位交互全部删除或在矩阵中标红为未完成。
- 读写 API 使用真实服务验证。
- 明暗主题、固定窄竖栏、窗口缩放、错误态、空态均截图验证。
- `human_tests/` 对应用例已执行并记录实际结果。
- 至少两轮 Review/Fix/Test 后没有 P0/P1/P2 用户可感知缺口。

## 交付节奏

1. P0 Shell + Network 完整闭环，作为视觉和数据架构基线。
2. P1 Rules / Values / Scripts / Replay，关闭最核心的代理工作流。
3. P2 Settings / Notifications / Groups / DevTools，关闭系统控制和协作能力。
4. P3 AI / Agent / IM Gateway / Speech / Video，根据 WebUI 暴露入口逐项补齐。

任何阶段都不能把"页面已出现"等同于"能力已对齐"；最终报告必须按页面矩阵列出已完成、阻塞、不适用和证据路径。
