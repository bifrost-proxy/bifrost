# Web Admin 完整 E2E UI 用例设计

## 现状结论（2026-06-17 刷新）

本文初稿写于浏览器场景仅有少量 JSON 脚本时。截至 2026-06-17，仓库已基于 Playwright 落地一整套 Web Admin UI E2E：

- 测试目录：`web/tests/ui/`，按页面拆分成 `traffic.spec.ts`、`traffic-push.spec.ts`、`admin-rules-values.spec.ts`、`admin-scripts.spec.ts`、`admin-replay.spec.ts`、`admin-settings.spec.ts`、`pending-auth-modal.spec.ts` 等 spec
- 运行器：`web/playwright.config.ts` 通过 `tests/ui/helpers/test-env.ts` 的 `allocateUiTestEnv()` 自动分配 backend/web 端口、临时 `BIFROST_DATA_DIR`、运行目录与 PID 文件，前端由 Playwright `webServer` 启动 `pnpm exec vite --host 127.0.0.1 --port <WEB_PORT>`
- 入口脚本：`web/package.json` 暴露 `pnpm test:ui` / `pnpm test:ui:headed`
- CI：`.github/workflows/ci.yml` 的 `e2e-shell` / `e2e-macos-shell` / `e2e-windows-shell` 等 job 通过 `scripts/ci/run-e2e-shell.sh` 拉起 Playwright，并缓存 `~/.cache/ms-playwright`
- Admin 写入辅助：`web/tests/ui/helpers/admin-helpers.ts` 与 `traffic-generator.cjs` 取代了原计划的 `seed-traffic.sh` / `seed-admin-data.sh` / `assert-admin-state.js`

下面保留原始矩阵作为设计目标和补全清单；具体落地以上述 spec 为准。原文中提到的 `.trae/skills/e2e-verify/scripts/scenarios/` 浏览器 JSON 场景路径已经下线（planned, not yet shipped as of 2026-06-17），不要再以该目录作为 ground truth。

这意味着以下关键链路仍缺完整 UI 设计：

- 启动代理服务后，模拟代理请求并在 `Traffic` 页面验证列表、详情、状态、删除、刷新
- 在 UI 内操作 `Rules`，再回到代理流量验证规则生效
- 在 UI 内操作 `Values`，并验证规则/脚本对值引用生效
- 在 UI 内操作 `Scripts`，并验证请求/响应/解码脚本效果
- 在 UI 内操作 `Settings`，并验证代理配置、性能配置、访问控制等状态变化
- Push 主链路下的多页面数据同步验证

## 目标

设计一套完整的 Web Admin UI E2E 用例文档，覆盖：

- 启动临时代理服务
- 启动前端 dev server
- 启动 mock 上游服务
- 通过代理制造 HTTP / HTTPS / SSE / WebSocket 流量
- 在 UI 中操作 `Traffic / Rules / Values / Scripts / Settings / Replay`
- 验证 UI 展示、状态收敛、网络请求、副作用与 push 同步

## 测试分层

建议拆成 3 层，而不是做一个巨大的全量场景。

### 1. Core UI 套件

特点：

- 可在本机和 CI 稳定运行
- 不依赖改系统代理或装证书
- 以 `127.0.0.1`、mock server、临时数据目录为主

覆盖：

- Traffic
- Rules
- Values
- Scripts
- Replay
- Settings 中的只读项和安全配置项

### 2. Host Integration 套件

特点：

- 需要宿主机能力
- 可能修改系统代理、shell 配置、证书信任状态
- 不建议默认进 CI

覆盖：

- Settings -> System Proxy toggle
- Settings -> CLI Proxy 状态/持久化
- Settings -> Certificate 下载/二维码/安装提示

### 3. Push Sync 套件

特点：

- 验证新数据同步模型
- 建议双页面或“页面 + 直接 API 写入”联动

覆盖：

- Values 写入后 Values / Rules 页面同步
- Scripts 写入后 Scripts 页面同步
- Replay 保存列表/分组同步
- Settings 访问控制和性能配置同步

## 统一前置环境

所有 Core UI 场景统一使用：

- 独立运行目录：`./.bifrost-ui-test-runs/<run-id>/`
- 临时数据目录：`BIFROST_DATA_DIR=./.bifrost-ui-test-runs/<run-id>/data`
- 临时代理端口：每次运行自动分配，不允许写死 `9900`
- 临时前端端口：每次运行自动分配，不允许复用其他任务的 dev server
- 前端入口：`BACKEND_PORT=<PORT> WEB_PORT=<WEB_PORT> pnpm -C web dev --host 127.0.0.1 --port <WEB_PORT>`
- UI 地址：`http://127.0.0.1:<WEB_PORT>/_bifrost/`
- Mock 服务：复用 `e2e-tests/mock_servers/start_servers.sh`
- 浏览器执行器：`.trae/skills/e2e-verify/scripts/browser-test.js`

统一要求：

- 每个场景使用唯一前缀，例如 `ui-e2e-<timestamp>`
- 每次测试必须启动独立代理进程，禁止复用共享代理实例
- 场景结束必须清理 rules / values / scripts / replay 数据
- 不允许复用正式服务端口 `9900`
- 代理服务启动时应显式 `--host 127.0.0.1`

## 核心用例矩阵

下面是建议的完整场景集。

### A. 启动与基础可用性

#### UI-BOOT-001 启动链路可用

目标：

- 验证代理服务、前端 dev server、UI 首页可用

步骤：

1. 启动临时代理服务
2. 启动前端 dev server 并绑定到该代理端口
3. 打开 `/_bifrost/`
4. 验证默认跳转到 `Traffic`
5. 验证侧边栏存在 `Traffic / Replay / Rules / Values / Scripts / Settings`

断言：

- 页面无全屏错误态
- push 连接建立成功
- `Traffic` 页面基础布局可见

建议场景文件：

- `ui-bootstrap-smoke.json`

### B. Traffic 页面

#### UI-TRAFFIC-001 HTTP 流量采集与列表展示

目标：

- 启动代理后通过代理发出 HTTP 请求，验证 `Traffic` 列表收敛

步骤：

1. 清空旧流量
2. 通过代理访问 mock HTTP 接口
3. 打开 `Traffic`
4. 等待列表出现新的 request row

断言：

- 列表记录数增加
- 最新记录 method / host / path / status 正确
- 响应时间、时间戳、协议列可见

#### UI-TRAFFIC-002 详情面板与 body 展示

目标：

- 验证点击流量记录后详情、请求头、响应头、body 展示正确

步骤：

1. 选中一条带 JSON body 的请求
2. 打开 request/response body tab

断言：

- URL、method、status、headers 正确
- JSON body 可见
- 大小字段与内容不为空

#### UI-TRAFFIC-003 SSE / WebSocket 状态收敛

目标：

- 验证流式连接在列表中的状态与帧数更新

步骤：

1. 通过代理连接 SSE upstream
2. 通过代理连接 WebSocket upstream
3. 观察 `Traffic` 列表中对应记录

断言：

- SSE / WS 记录出现
- 打开时状态为进行中
- 帧数或消息数递增
- 关闭后状态收敛

说明：

- 现有 `stream-sse`、`stream-ws` 可纳入该大类，但还不够覆盖详情和列表状态检查

#### UI-TRAFFIC-004 删除流量与详情清理

目标：

- 验证删除记录后列表和详情联动清理

步骤：

1. 选中一条流量
2. 点击删除
3. 等待列表移除

断言：

- 列表中该记录消失
- 若删除的是当前详情，详情区域应清空或提示缺失

说明：

- 现有 `traffic-delete` 可升级为该标准场景

#### UI-TRAFFIC-005 搜索 / 过滤 / 清空

目标：

- 验证搜索条件与列表结果一致

步骤：

1. 造三类不同 host/path/method 的请求
2. 在 UI 中输入关键字、切换状态/协议过滤
3. 清空过滤

断言：

- 过滤结果与条件匹配
- 清空后列表恢复

### C. Rules 页面

#### UI-RULES-001 创建规则并对代理流量生效

目标：

- 在 UI 中创建规则，验证代理流量被改写

步骤：

1. 进入 `Rules`
2. 新建一条 host/headers/body 类规则
3. 保存并启用
4. 通过代理再次请求 mock 接口
5. 返回 `Traffic` 查看结果

断言：

- 新规则出现在列表中
- 启用状态正确
- 代理请求体现规则效果
- `Traffic` 详情中 matched rule 或结果可见

#### UI-RULES-002 编辑、禁用、删除规则

目标：

- 验证规则 CRUD 和生效状态

步骤：

1. 编辑已存在规则
2. 禁用规则
3. 再次发请求
4. 删除规则

断言：

- 编辑后的规则内容持久化
- 禁用后请求恢复原始行为
- 删除后列表无该规则

#### UI-RULES-003 规则引用 Values

目标：

- 验证规则编辑器对值引用的联动

步骤：

1. 先创建一个 value
2. 在 Rules 中创建使用 `{valueName}` 的规则
3. 保存后发请求

断言：

- 规则保存成功
- 请求结果使用该值

### D. Values 页面

#### UI-VALUES-001 Values CRUD

目标：

- 验证值列表创建、编辑、重命名、删除

步骤：

1. 进入 `Values`
2. 新建 value
3. 修改 value 内容
4. 重命名
5. 删除

断言：

- 列表即时更新
- 当前选中项与右侧编辑器同步
- 删除后列表和选中态收敛

#### UI-VALUES-002 Values 改动影响 Rules 生效

目标：

- 验证值变化会影响规则执行结果

步骤：

1. 规则中使用 `{tokenValue}`
2. 在 Values 页面修改 `tokenValue`
3. 再次发代理请求

断言：

- 新请求结果体现新的值

#### UI-VALUES-003 Values push 同步

目标：

- 验证页面不依赖整表重拉

步骤：

1. 打开 `Values`
2. 通过 API 或第二页面写入 value
3. 观察当前页面

断言：

- 当前页面自动出现最新值
- 不依赖手动刷新按钮

### E. Scripts 页面

#### UI-SCRIPTS-001 Request Script CRUD

目标：

- 验证 request script 的创建、编辑、删除

步骤：

1. 进入 `Scripts`
2. 在 request 类型下新建脚本
3. 编写简单头部注入逻辑
4. 保存
5. 删除

断言：

- 列表出现脚本
- 编辑器内容持久化
- 删除后列表移除

#### UI-SCRIPTS-002 Script 对代理请求生效

目标：

- 验证脚本执行影响实际流量

步骤：

1. 创建 request script 给请求注入 header
2. 配置规则引用该脚本
3. 发代理请求到 mock upstream
4. 查看 upstream 或 Traffic 详情

断言：

- 注入 header 可见
- 规则与脚本联动成功

#### UI-SCRIPTS-003 Response / Decode Script 生效

目标：

- 验证 response script 或 decode script 的效果

步骤：

1. 创建 response script 改写响应头，或 decode script 格式化 body
2. 发请求
3. 打开 Traffic 详情

断言：

- 响应头或 decode 后展示符合预期

#### UI-SCRIPTS-004 Scripts push 同步

目标：

- 验证脚本保存后列表通过 push 自动更新

步骤：

1. 打开 `Scripts`
2. 通过第二页面或 API 保存脚本
3. 观察列表

断言：

- 当前页面自动收敛

### F. Replay 页面

#### UI-REPLAY-001 保存请求到集合

目标：

- 验证从请求到 replay 集合的保存链路

步骤：

1. 在 `Traffic` 中选中一条请求
2. 保存到 replay
3. 打开 `Replay`

断言：

- 保存请求出现在 replay 列表
- 基本字段正确

#### UI-REPLAY-002 分组、移动、删除

目标：

- 验证 replay 分组和列表同步

步骤：

1. 创建 group
2. 新建 request
3. 移动到 group
4. 删除 request / group

断言：

- group 列表正确
- request 所属 group 正确
- 删除后列表即时收敛

#### UI-REPLAY-003 执行 replay

目标：

- 验证 replay 执行后生成新流量

步骤：

1. 在 Replay 页面执行已保存请求
2. 返回 Traffic

断言：

- Traffic 中出现 replay 请求记录
- request / response 结果正确

### G. Settings 页面

#### UI-SETTINGS-001 Proxy 基础信息展示

目标：

- 验证代理监听信息、地址、二维码、端口展示

步骤：

1. 进入 `Settings -> Proxy`

断言：

- host / port / addresses 正确
- 复制按钮、二维码区域可见

#### UI-SETTINGS-002 TLS 配置编辑

目标：

- 验证 TLS 拦截开关、include/exclude 配置保存

步骤：

1. 打开 `Settings -> Proxy`
2. 修改 TLS interception 开关和 pattern
3. 保存
4. 刷新页面

断言：

- 配置持久化
- 页面重开后状态一致

#### UI-SETTINGS-003 Performance 配置编辑

目标：

- 验证性能配置滑块、保存和 push 收敛

步骤：

1. 打开 `Settings -> Performance`
2. 调整 `max_records`、`max_db_size_bytes` 等
3. 保存
4. 观察当前页和第二页

断言：

- 数值变化正确
- 保存后配置持久化
- 订阅页面自动同步

#### UI-SETTINGS-004 Access Control 配置

目标：

- 验证 mode、allow_lan、whitelist、temporary whitelist

步骤：

1. 打开 `Settings -> Access`
2. 切换 mode
3. 添加 whitelist IP/CIDR
4. 切换 allow_lan
5. 添加/删除 temporary whitelist

断言：

- 列表与开关即时更新
- 刷新后仍保持
- 无页面卡死或请求悬挂

#### UI-SETTINGS-005 Pending Authorizations

目标：

- 验证待审批列表、approve/reject/clear all

步骤：

1. 构造待审批客户端
2. 打开 Access tab
3. approve / reject / clear

断言：

- 待审批列表变化正确
- whitelist / temporary whitelist 联动正确

#### UI-SETTINGS-006 Certificate 展示

目标：

- 验证证书下载链接、二维码、状态展示

步骤：

1. 打开 `Settings -> Certificate`

断言：

- 下载按钮、二维码、状态文案正确

#### UI-SETTINGS-007 System Proxy / CLI Proxy

目标：

- 验证系统代理和 CLI 代理状态展示及切换

说明：

- 该场景应归入 Host Integration 套件
- 默认不进 CI

断言：

- 状态读取正确
- 切换后状态可回读
- 场景结束必须恢复原状

## 推荐场景拆分

落地状态（2026-06-17）：原计划的 JSON 场景文件已替换为 Playwright spec。下表保留作为设计意图，但 ground truth 是 `web/tests/ui/*.spec.ts`。下面前缀以 `ui-*.json` 命名的均为 planned, not yet shipped as of 2026-06-17，对应 Playwright 实现在右侧 spec 中：

- bootstrap smoke → 由 `playwright.config.ts` 的 `webServer.url` + 各 spec 入口覆盖
- traffic 全部 → `traffic.spec.ts` / `traffic-push.spec.ts`
- rules + values → `admin-rules-values.spec.ts`
- scripts → `admin-scripts.spec.ts`
- replay → `admin-replay.spec.ts`
- settings（含 access / performance / TLS / certificate / sync / agent / IM / remote invoke）→ `admin-settings.spec.ts`
- pending authorizations → `pending-auth-modal.spec.ts`

以下为原始建议的 JSON 场景命名（planned, not yet shipped as of 2026-06-17）：

- `ui-bootstrap-smoke.json`
- `ui-traffic-http-detail.json`
- `ui-traffic-streaming-state.json`
- `ui-traffic-search-delete.json`
- `ui-rules-create-apply.json`
- `ui-rules-edit-disable-delete.json`
- `ui-values-crud-sync.json`
- `ui-values-rules-integration.json`
- `ui-scripts-crud.json`
- `ui-scripts-apply-request-response.json`
- `ui-replay-group-execute.json`
- `replay-collection-sync.json`
- `replay-execute-traffic.json`
- `replay-history-filters.json`
- `ui-settings-proxy-tls.json`
- `ui-settings-performance-sync.json`
- `ui-settings-access-control.json`
- `ui-settings-pending-authorizations.json`
- `ui-settings-certificate-readonly.json`
- `ui-settings-system-cli-proxy-host.json`

## 推荐实现顺序

落地状态（2026-06-17）：P0/P1 中绝大多数项已通过上面列出的 Playwright spec 落地；剩余少量主题（如 system/cli proxy 切换的 host 集成、双标签 push 同步专项）仍按 planned, not yet shipped as of 2026-06-17 看待。

为了控制实现成本，建议按 P0 / P1 / P2 分批落地。

### P0

- `ui-bootstrap-smoke`
- `ui-traffic-http-detail`
- `ui-rules-create-apply`
- `ui-values-crud-sync`
- `ui-scripts-crud`
- `ui-settings-access-control`
- `ui-settings-performance-sync`

### P1

- `ui-traffic-streaming-state`
- `ui-traffic-search-delete`
- `ui-values-rules-integration`
- `ui-scripts-apply-request-response`
- `ui-replay-group-execute`
- `ui-settings-pending-authorizations`

### P2

- `ui-settings-proxy-tls`
- `ui-settings-certificate-readonly`
- `ui-settings-system-cli-proxy-host`
- 双标签 push 同步专项场景

## 场景编写约束

为了让这些 UI 场景长期可维护，建议强制遵守：

- 不依赖中文文案做唯一定位，优先 `role + name`、icon、稳定 selector、数据区域结构
- 所有写操作都要有 cleanup
- 每个场景必须显式记录：
  - 前置数据
  - 触发动作
  - UI 断言
  - 网络断言
  - 清理动作
- 流量验证尽量同时看两层：
  - upstream mock 收到的真实请求
  - Admin UI 中展示的 request / response / status
- Push 验证至少保留一类“第二页面/API 写入 -> 当前页面自动收敛”的场景

## 建议补充的辅助脚本

落地状态（2026-06-17）：原文这 3 类辅助能力已被 Playwright helper 取代，不再以 shell/JS 单文件形式落地：

- 流量造数：`web/tests/ui/traffic-generator.cjs`（HTTP / SSE / WebSocket / 大 body）
- Admin 数据写入与断言：`web/tests/ui/helpers/admin-helpers.ts`（rules / values / scripts / replay / settings 通过 `/_bifrost/api` 直接 CRUD）
- 运行环境与端口分配：`web/tests/ui/helpers/test-env.ts`、`global-setup.ts`、`global-teardown.ts`

下方原计划列表仅作为历史参考（planned, not yet shipped as of 2026-06-17，并且已确定不会以下列文件名实现）：

- `seed-traffic.sh`
  - 生成 HTTP / SSE / WebSocket / 错误请求 / 大 body 请求
- `seed-admin-data.sh`
  - 创建 rules / values / scripts / replay groups / replay requests
- `assert-admin-state.js`
  - 统一断言 `/_bifrost/api` 的状态，减少浏览器脚本里塞过多业务判断

## 文档结论

刷新结论（2026-06-17）：

- 完整 Web Admin UI E2E 已落地为 `web/tests/ui/` 下的 Playwright 套件，并接入 CI 的 `e2e-shell` / `e2e-macos-shell` / `e2e-windows-shell` 等 job
- 本文上面的矩阵继续作为覆盖目标对照表使用，但具体场景命名、JSON 路径、辅助 shell 脚本均以 Playwright spec 与 `helpers/` 为准
- 仍按 planned, not yet shipped as of 2026-06-17 看待的剩余项主要集中在：system/cli proxy 等 Host Integration 场景，以及双标签 push 同步专项
