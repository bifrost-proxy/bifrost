# IM Gateway 顶级网关设计

> 状态：讨论稿
> 范围：先设计，不实现
> 目标版本：第一阶段支持 Feishu，架构预留 WeChat 等其他 IM provider

## 背景

当前 Remote Invoke 已经具备跨设备发起远程命令的基础能力，但用户提出的 IM 能力不应被建模为 Remote Invoke 的一个子功能。它本质上是一个顶级消息网关：

- 能配置 IM 机器人凭据和目标接收对象
- 能主动发送 IM 消息，第一阶段要求支持飞书兼容的卡片消息
- 能建立与飞书开放平台的长连接，接收机器人消息事件和卡片回调
- 能把收到的 IM 消息路由到任务执行器；第一版本仅执行脚本并在脚本完成后返回消息，后续预留触发已有任务等 agent 能力
- 能管理定时任务，包括新增、删除、修改、暂停、恢复、手动执行、超时控制和执行历史
- 后续需要平滑接入 WeChat 等其他 IM 通道

因此本方案把能力命名为 `IM Gateway`，作为 Settings 中的一级 Tab 和 CLI 顶级命令，而不是放进 `Remote Invoke` Tab。

## 设计结论

1. 新增顶级模块 `IM Gateway`。
2. WebUI 在 Settings 下新增一级 Tab：`IM Gateway`。
3. Feishu 是第一阶段 provider，但内部抽象必须按多 IM provider 设计。
4. Feishu 必须使用开放平台长连接模式接收事件，不使用 Webhook 作为第一阶段主路径。
5. `app_id` / `app_secret` 只保存在目标 Bifrost 本地数据目录，不通过 relay 明文传输，不写入 Recent Calls 明文，不写入 caller 本地连接文件。
6. Remote Invoke 只作为“远程管理 IM Gateway”的控制入口。飞书长连接始终由目标端 Bifrost 本地进程直接连接飞书开放平台。
7. 远程创建或修改可执行脚本的 route / schedule 是高权限持久化执行能力，必须独立授权，不能复用普通 query 或 shell grant。
8. IM Gateway 的脚本执行策略必须受 Remote Invoke 权限体系约束，不能另起一套绕过 Remote Invoke 的脚本执行权限。
9. 每个 IM provider/config 都必须绑定一个 Remote Invoke 风格的 IM grant。Feishu 第一版本默认以 `app_id` 作为 grant identity key，保持与 remote 其他连接通道一致的授权管理风格。

## 目标与非目标

### 目标

- Settings 新增一级 `IM Gateway` Tab，独立于 `Remote Invoke`。
- 支持配置 Feishu provider：
  - `app_id`
  - `app_secret`
  - API base URL，默认 `https://open.feishu.cn/open-apis`
  - 长连接开关
  - 事件订阅状态展示
- 支持配置目标接收对象：
  - 人
  - 群
  - 邮箱或其他飞书支持的 receive id 类型
- 支持直接发送飞书兼容卡片消息：
  - 本地 CLI
  - 本地 Admin API
  - Remote CLI
  - WebUI
- 支持 Feishu 长连接事件接收：
  - 消息事件
  - 机器人被 @ 的消息
  - 后续可扩展更多事件类型
- 支持事件路由：
  - 第一版本仅支持接收消息后执行脚本，并在脚本执行完成后把结果作为消息返回
  - 按 provider / chat_id / user_id / keyword / regex 匹配
  - 为后续 agent 任务执行、卡片交互回调、触发已有任务等能力预留统一路由模型
- 支持定时任务：
  - cron 表达式
  - fixed interval
  - 手动 run once
  - timeout
  - 并发限制
  - pause / resume
  - 执行历史
- 支持远程管理 IM Gateway：
  - `bifrost remote im ...`
  - 需要独立的 `remote_im_gateway` 或等效 grant scope
  - 面向各种 coding agent / automation agent 的运行场景，提供安全的通知与任务配置能力：
    - 检查 IM 通道是否可用
    - 检查指定 target 是否可发送
    - 发送通知卡片
    - 创建、修改、暂停、恢复、删除定时通知任务
    - 手动执行已配置任务
    - 查询任务运行状态和必要的执行摘要
  - 明确不提供通过 remote 获取密钥、获取完整 provider 配置详情、导出原始配置的能力

### 非目标

- 第一阶段不实现 WeChat，只保证 provider 抽象能承载。
- 第一阶段不做通用工作流编排器，只提供 IM event -> route -> task 的最小闭环。
- 第一阶段不把 IM Gateway 放到 relay 执行。relay 只转发加密 remote command。
- 第一阶段不把 Feishu event 原始 payload 永久完整保存为默认行为。
- 第一阶段不支持从任意未授权群聊触发脚本执行。
- 第一阶段不支持跨设备共享同一个 provider secret。
- 第一阶段不支持 `bifrost remote im` 读取 `app_secret`、secret_ref 明文、完整 provider config、完整 raw event payload。
- 第一阶段不支持远程导出可直接迁移到另一台机器的 IM Gateway 配置包。

## 用户体验

### Settings 一级 Tab

Settings 页面新增一级 Tab：

```text
Settings
  - Proxy
  - Certificate
  - TLS / Interception
  - Remote Invoke
  - IM Gateway
  - Performance
  - Access Control
  - Appearance
  - Metrics
  - Sync
```

`Remote Invoke` 继续管理远程授权、连接、Recent Calls、Shell Access、File Access 等远控相关内容。

`IM Gateway` 独立管理 IM 通道、发送、事件接收和任务。

### IM Gateway Tab 内部结构

建议内部使用二级页签：

```text
IM Gateway
  - Connections
  - Targets
  - Routes
  - Schedules
  - History
```

#### Connections

展示 provider 和长连接状态：

- provider 名称
- provider 类型：`feishu`
- enabled 状态
- app_id，展示脱敏值
- secret 状态：已配置 / 未配置，永不展示明文
- 长连接状态：
  - disconnected
  - connecting
  - connected
  - reconnecting
  - failed
- 最近连接时间
- 最近事件时间
- 重连次数
- 最近错误
- 订阅事件列表

#### Targets

管理接收对象：

- target id
- provider id
- receive_id_type
- receive_id
- display name
- 默认消息类型
- enabled

第一阶段 Feishu 支持：

- `open_id`
- `user_id`
- `union_id`
- `email`
- `chat_id`

#### Routes

管理 inbound event 触发规则：

- route id / name
- provider id
- event type，第一版本仅启用 message receive / app mention 这类消息接收事件
- match 条件
- action 类型，第一版本固定为 script then reply
- timeout
- enabled
- 最近触发状态

示例：

```text
收到 oc_xxx 群里的 "/check bifrost" 消息
  -> 执行 ./scripts/check-service.sh bifrost
  -> 脚本执行完成后，把 stdout / stderr 摘要转成消息回复到原群
```

第一版本不交付以下 inbound action，只在模型上保留扩展空间：

- 收到消息后触发已有 schedule
- 收到消息后只发送静态卡片、不执行脚本
- 卡片交互回调触发任务
- 多轮 agent 会话

#### Schedules

管理定时任务：

- schedule id / name
- target id
- cron 或 interval
- script
- timeout
- enabled / paused
- next_run_at
- last_run_at
- last_status

#### History

展示两类历史：

- inbound events：收到的 IM 事件摘要
- task runs：route / schedule / manual run 的执行历史

历史展示默认脱敏，只保留必要摘要。需要排查时可通过受控开关保存更详细 payload。

## 架构

### 模块分层

```text
crates/bifrost-admin/src/im_gateway/
  mod.rs
  types.rs
  provider.rs
  feishu.rs
  connection.rs
  target_store.rs
  provider_store.rs
  route_store.rs
  schedule_store.rs
  event_store.rs
  run_store.rs
  scheduler.rs
  event_router.rs
  task_executor.rs

crates/bifrost-admin/src/handlers/im_gateway.rs

crates/bifrost-cli/src/commands/im.rs

web/src/pages/Settings/tabs/ImGatewayTab.tsx
web/src/api/imGateway.ts
```

### 逻辑链路

Outbound：

```text
CLI / WebUI / Remote command
  -> Admin API / Remote Invoke encrypted command
  -> IM Gateway sender
  -> Provider adapter
  -> Feishu OpenAPI
  -> message_id / error
  -> History
```

Inbound：

```text
Feishu Open Platform
  -> long connection event
  -> Feishu provider connection
  -> IM Gateway event normalizer
  -> Event Router
  -> Task Executor
  -> optional card reply
  -> History
```

Schedule：

```text
Scheduler tick
  -> due schedule
  -> Task Executor
  -> stdout/stderr/status
  -> optional card send
  -> Run History
  -> compute next_run_at
```

Remote management：

```text
bifrost remote im ...
  -> existing remote invoke relay path
  -> encrypted RemoteCommand(kind = im.gateway)
  -> target local IM Gateway executor
  -> result
```

飞书长连接不经过 Bifrost relay：

```text
target Bifrost process <-> Feishu Open Platform WebSocket
```

## Provider 抽象

### Trait 形态

```rust
#[async_trait]
pub trait ImProvider: Send + Sync {
    fn provider_type(&self) -> ImProviderType;
    async fn validate_config(&self, config: &ProviderConfig) -> Result<ProviderValidation>;
    async fn connect_events(&self, config: ProviderConfig, sink: EventSink) -> Result<ConnectionHandle>;
    async fn send_card(&self, config: &ProviderConfig, target: &ImTarget, card: serde_json::Value, opts: SendOptions) -> Result<SendResult>;
}
```

### Provider 类型

```rust
pub enum ImProviderType {
    Feishu,
    WeChat,
    Webhook,
}
```

第一阶段只实现 `Feishu`，但 store / API / UI 均按 `provider_type` 分发。

## Feishu Provider

### 认证

配置项：

```json
{
  "id": "feishu-main",
  "type": "feishu",
  "enabled": true,
  "display_name": "Feishu Main",
  "base_url": "https://open.feishu.cn/open-apis",
  "app_id": "cli_xxx",
  "secret_ref": "local:feishu-main",
  "event_connection_enabled": true,
  "event_types": [
    "im.message.receive_v1",
    "card.action.trigger"
  ]
}
```

`app_secret` 明文不直接进入 provider config，而是进入本地 secret store。第一阶段可以先用同一个 JSON 文件存储，但字段必须加密或至少与普通 config 分离，并在所有输出中 mask。

每个 provider/config 创建时必须同步创建或绑定一条本地 IM grant：

```json
{
  "grant_scope": "remote_im_gateway",
  "im_provider_id": "feishu-main",
  "im_provider_type": "feishu",
  "im_grant_key": "cli_xxx",
  "im_grant_key_type": "app_id",
  "status": "active"
}
```

第一版本 Feishu 默认使用 `app_id` 作为 `im_grant_key`。如果后续 provider 没有 app_id，则必须定义同等稳定的 provider identity key。该 grant 进入 Remote Invoke 的 grant 管理视图和 API，而不是只存在于 IM Gateway 内部。

### Token 缓存

Feishu provider 本地维护 token cache：

- key：provider id
- value：tenant_access_token / expire_at
- 提前刷新窗口：到期前 5 分钟
- 失败后指数退避

### 发送卡片

发送接口抽象参数：

```json
{
  "target_id": "oncall-group",
  "msg_type": "interactive",
  "card": {},
  "uuid": "optional-idempotency-key"
}
```

Feishu provider 转换为飞书消息接口：

- query：`receive_id_type=<target.receive_id_type>`
- body：
  - `receive_id`
  - `msg_type`
  - `content`
  - `uuid`

卡片内容保持飞书兼容 JSON。IM Gateway 不重新发明卡片 DSL，后续如需更友好的模板再加 `card_template` 层。

### 长连接

第一阶段要求使用飞书开放平台长连接模式：

- Bifrost 本地进程通过 SDK 或等效 WebSocket client 建立连接
- 连接由 provider connection manager 管理
- 支持自动重连
- 支持状态上报到 WebUI
- 支持 event normalizer 把 Feishu 事件转换为统一 `ImEvent`

统一事件模型：

```json
{
  "event_id": "evt_xxx",
  "provider_id": "feishu-main",
  "provider_type": "feishu",
  "event_type": "message.receive",
  "source": {
    "chat_id": "oc_xxx",
    "user_id": "ou_xxx",
    "message_id": "om_xxx"
  },
  "message": {
    "text": "/check bifrost",
    "mentions": [],
    "raw_type": "text"
  },
  "received_at": 1710000000000,
  "raw_digest": "sha256:..."
}
```

默认不持久化完整 raw payload。需要排查时可配置短期 debug retention。

## 数据模型

### ProviderConfig

```rust
pub struct ImProviderConfig {
    pub id: String,
    pub provider_type: ImProviderType,
    pub display_name: String,
    pub enabled: bool,
    pub base_url: Option<String>,
    pub app_id: Option<String>,
    pub secret_ref: Option<String>,
    pub event_connection_enabled: bool,
    pub event_types: Vec<String>,
    pub created_at: u64,
    pub updated_at: u64,
}
```

### Target

```rust
pub struct ImTarget {
    pub id: String,
    pub provider_id: String,
    pub display_name: String,
    pub receive_id_type: String,
    pub receive_id: String,
    pub default_msg_type: String,
    pub enabled: bool,
    pub created_at: u64,
    pub updated_at: u64,
}
```

### Route

```rust
pub struct ImRoute {
    pub id: String,
    pub provider_id: String,
    pub name: String,
    pub enabled: bool,
    pub event_type: ImEventType,
    pub matcher: ImEventMatcher,
    pub action: ImRouteAction,
    pub timeout_ms: u64,
    pub max_output_bytes: u64,
    pub created_at: u64,
    pub updated_at: u64,
}
```

Matcher：

```rust
pub struct ImEventMatcher {
    pub chat_ids: Vec<String>,
    pub user_ids: Vec<String>,
    pub keyword: Option<String>,
    pub regex: Option<String>,
    pub card_action_values: Vec<String>,
}
```

Action：

```rust
pub enum ImRouteAction {
    RunScriptAndReply {
        script_text: Option<String>,
        script_file: Option<String>,
        cwd: Option<String>,
        env: BTreeMap<String, String>,
        reply_target: ReplyTarget,
        reply_mode: ReplyMode,
    },
}
```

第一版本只实现 `RunScriptAndReply`。`RunSchedule`、`SendCard`、`CardAction`、`AgentTask` 等 action 类型作为后续扩展，不进入 V1 交付范围。

### Schedule

```rust
pub struct ImSchedule {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub target_id: String,
    pub trigger: ScheduleTrigger,
    pub script: TaskScript,
    pub timeout_ms: u64,
    pub max_output_bytes: u64,
    pub concurrency_policy: ConcurrencyPolicy,
    pub retry: RetryPolicy,
    pub next_run_at: Option<u64>,
    pub last_run_at: Option<u64>,
    pub created_at: u64,
    pub updated_at: u64,
}
```

Trigger：

```rust
pub enum ScheduleTrigger {
    Cron { expr: String, timezone: String },
    Interval { every_ms: u64 },
}
```

### Run History

```rust
pub struct ImTaskRun {
    pub run_id: String,
    pub trigger_source: TriggerSource,
    pub route_id: Option<String>,
    pub schedule_id: Option<String>,
    pub provider_id: Option<String>,
    pub target_id: Option<String>,
    pub status: TaskRunStatus,
    pub started_at: u64,
    pub ended_at: Option<u64>,
    pub duration_ms: Option<u64>,
    pub exit_code: Option<i32>,
    pub stdout_preview: Option<String>,
    pub stderr_preview: Option<String>,
    pub stdout_digest: Option<String>,
    pub stderr_digest: Option<String>,
    pub error: Option<String>,
}
```

## 持久化

建议文件：

```text
BIFROST_DATA_DIR/admin/im_gateway_providers.json
BIFROST_DATA_DIR/admin/im_gateway_secrets.json
BIFROST_DATA_DIR/admin/im_gateway_targets.json
BIFROST_DATA_DIR/admin/im_gateway_routes.json
BIFROST_DATA_DIR/admin/im_gateway_schedules.json
BIFROST_DATA_DIR/admin/im_gateway_events.json
BIFROST_DATA_DIR/admin/im_gateway_runs.json
```

第一阶段可沿用现有 Remote Invoke store 的版本化 JSON 模式：

- 每个文件带 version
- 不兼容版本可 reset，因项目数据库协议允许不兼容重建
- run / event history 做数量和时间裁剪
- secret store 输出永远 mask

## CLI 设计

### 本地命令

```bash
bifrost im provider list
bifrost im provider add feishu-main --type feishu --app-id cli_xxx --secret env:FEISHU_APP_SECRET
bifrost im provider update feishu-main --enable-long-connection true
bifrost im provider delete feishu-main
bifrost im provider status feishu-main

bifrost im target list
bifrost im target add oncall-group --provider feishu-main --receive-id-type chat_id --receive-id oc_xxx
bifrost im target update oncall-group --receive-id oc_yyy
bifrost im target delete oncall-group

bifrost im send --target oncall-group --card-file ./card.json
bifrost im send --target oncall-group --card-json '{"config":{},"elements":[]}'

bifrost im route list
bifrost im route add deploy-check --provider feishu-main --event message.receive --chat-id oc_xxx --regex '^/check (?P<service>\\S+)$' --script-file ./check-service.sh --reply card
bifrost im route pause deploy-check
bifrost im route resume deploy-check
bifrost im route delete deploy-check

bifrost im schedule list
bifrost im schedule add health-check --target oncall-group --cron '*/5 * * * *' --script-file ./health-check.sh --timeout-ms 30000
bifrost im schedule update health-check --cron '*/10 * * * *'
bifrost im schedule pause health-check
bifrost im schedule resume health-check
bifrost im schedule run health-check
bifrost im schedule logs health-check
bifrost im schedule delete health-check

bifrost im history events
bifrost im history runs
```

### 远程命令

```bash
bifrost remote im provider list
bifrost remote im provider status feishu-main
bifrost remote im channel check --provider feishu-main
bifrost remote im channel check --target oncall-group
bifrost remote im target list
bifrost remote im send --target oncall-group --card-file ./card.json
bifrost remote im route list
bifrost remote im route add deploy-check --provider feishu-main --event message.receive --chat-id oc_xxx --regex '^/check (?P<service>\\S+)$' --script-file ./check-service.sh --reply card
bifrost remote im schedule add health-check --target oncall-group --cron '*/5 * * * *' --script-file ./health-check.sh --timeout-ms 30000
bifrost remote im schedule pause health-check
bifrost remote im schedule resume health-check
bifrost remote im schedule run health-check
bifrost remote im history runs
```

远程命令不会把 provider secret 带到 caller。配置 secret 的远程命令第一阶段禁止；如果后续确实需要远程引导配置，只能写入 target 本地已有 secret reference，例如由本地管理员预先创建的 `local:feishu-main`，不允许 caller 直接传 secret 明文。

远程 provider / target 查询也必须返回安全摘要，不返回完整配置：

```json
{
  "provider_id": "feishu-main",
  "provider_type": "feishu",
  "enabled": true,
  "connection_state": "connected",
  "app_id_masked": "cli_***abcd",
  "secret_configured": true,
  "last_connected_at": 1710000000000,
  "last_event_at": 1710000005000,
  "last_error": null
}
```

不允许 remote 返回：

- `app_secret`
- 可还原 secret 的 `secret_ref` 明文
- 完整 provider JSON
- 完整 raw event payload
- token cache
- 长连接内部鉴权材料

## Admin API

```text
GET    /api/im-gateway/providers
POST   /api/im-gateway/providers
GET    /api/im-gateway/providers/:id
PATCH  /api/im-gateway/providers/:id
DELETE /api/im-gateway/providers/:id
GET    /api/im-gateway/providers/:id/status

GET    /api/im-gateway/targets
POST   /api/im-gateway/targets
PATCH  /api/im-gateway/targets/:id
DELETE /api/im-gateway/targets/:id

POST   /api/im-gateway/messages/send

GET    /api/im-gateway/routes
POST   /api/im-gateway/routes
PATCH  /api/im-gateway/routes/:id
DELETE /api/im-gateway/routes/:id
POST   /api/im-gateway/routes/:id/pause
POST   /api/im-gateway/routes/:id/resume

GET    /api/im-gateway/schedules
POST   /api/im-gateway/schedules
PATCH  /api/im-gateway/schedules/:id
DELETE /api/im-gateway/schedules/:id
POST   /api/im-gateway/schedules/:id/pause
POST   /api/im-gateway/schedules/:id/resume
POST   /api/im-gateway/schedules/:id/run
GET    /api/im-gateway/schedules/:id/runs

GET    /api/im-gateway/history/events
GET    /api/im-gateway/history/runs
```

## Remote Invoke 协议扩展

新增命令 kind：

```rust
CommandKind::ImGateway
```

序列化值：

```text
im.gateway
```

新增 grant scope：

```rust
GrantScope::RemoteImGateway
```

或者拆成更细粒度：

```rust
GrantScope::RemoteImGatewayRead
GrantScope::RemoteImGatewaySend
GrantScope::RemoteImGatewayManage
```

建议第一阶段采用单 scope + command 内细分权限字段。核心目标是让运行在各种 agent 里的 Remote 模块可以安全使用 IM Gateway 做通知和定时通知，而不是远程读取或搬运 IM Gateway 的敏感配置。

- read：list/status/history
- send：send card
- manage：provider/target/route/schedule write
- execute：manual run / route script / schedule script

其中 read 权限只允许读取安全摘要：

- provider / channel 健康状态
- target 列表摘要
- route / schedule 列表摘要
- run history 摘要
- 最近错误摘要

read 权限不允许读取：

- provider secret
- 完整 provider config
- token cache
- raw event payload
- 完整 card payload 历史

Remote Agent 推荐使用的最小能力面：

```bash
bifrost remote im channel check --target oncall-group
bifrost remote im send --target oncall-group --card-file ./notify.json
bifrost remote im schedule add agent-heartbeat --target oncall-group --cron '*/30 * * * *' --card-file ./heartbeat.json
bifrost remote im schedule pause agent-heartbeat
bifrost remote im schedule resume agent-heartbeat
bifrost remote im schedule run agent-heartbeat
bifrost remote im history runs --schedule agent-heartbeat
```

这些命令的返回值必须适合 agent 消费：

- 支持 `--format json`
- 有稳定错误码
- 不包含密钥或完整敏感配置
- send 成功时返回 provider message id / request id
- schedule 操作成功时返回 schedule id / next_run_at / status

审批弹窗必须清楚展示高风险能力：

```text
This grant can manage IM Gateway on this device.
It may create scheduled scripts or routes that execute commands after IM events.
```

Remote Invoke relay 只需要：

- 允许 `command_kind=im.gateway`
- 做 grant scope 校验
- 继续端到端加密 command payload
- 只保存 route-only summary，不保存 provider secret 或完整卡片明文

relay 和 caller 本地连接文件都不得保存 IM Gateway 密钥材料或完整配置。call summary 只能是可审计的摘要，例如：

```json
{
  "command_preview": "im.send target=oncall-group",
  "masked_args_json": "{\"target\":\"oncall-group\",\"msg_type\":\"interactive\",\"card_digest\":\"sha256:...\"}"
}
```

### IM grant 管理

Remote Invoke 模块需要新增对 IM grant 的管理视图和 API，使 IM 通道权限与现有 remote 连接通道保持同一种处理风格。

IM grant 的来源：

- 创建 Feishu provider/config 时自动创建或绑定。
- 默认 key 为 `app_id`。
- grant 与 provider/config 生命周期绑定；provider 删除后 grant 应标记 removed 或 revoked。
- provider disable 时 grant 不删除，但状态检查应返回 channel disabled。

IM grant 的管理能力：

```bash
bifrost remote grant list --scope remote_im_gateway
bifrost remote grant show <grant-id>
bifrost remote grant update <grant-id> --scope remote_im_gateway --im-permission send,manage_schedule
bifrost remote grant revoke <grant-id>
```

WebUI `Remote Invoke` 的 Grants 列表也要展示 IM grant：

- grant scope：`remote_im_gateway`
- provider type：`feishu`
- provider display name
- identity key：脱敏后的 `app_id`
- permissions：read / send / manage_schedule / manage_route / execute_script
- status
- last_command_at

IM Gateway Tab 可以展示 grant 状态摘要，但 grant 的完整管理入口应复用 Remote Invoke grant 管理，避免出现两套权限事实源。

建议第一版本 IM permissions：

```text
read_status
send_message
manage_schedule
manage_route
execute_script
```

其中 `execute_script` 是最高风险权限：无论脚本来自 route 还是 schedule，只要会实际执行本地脚本，就必须检查该 permission。

### 权限检查规则

所有 IM Gateway 操作都必须先解析 provider/config 对应的 IM grant，再判断具体 permission：

| 操作 | 必需权限 |
| --- | --- |
| channel check | `read_status` |
| target list safe summary | `read_status` |
| send card/message | `send_message` |
| schedule add/update/delete/pause/resume | `manage_schedule` |
| schedule manual run | `manage_schedule` + `execute_script` |
| route add/update/delete/pause/resume | `manage_route` |
| inbound route script execution | `execute_script` |
| history safe summary | `read_status` |

缺少 permission 时，必须拒绝并写入审计。不能因为消息来自 Feishu 长连接就绕过 Remote Invoke grant；inbound event 触发脚本同样要用 provider 绑定的 IM grant 做策略检查。

## 任务执行器

### 脚本执行

第一阶段任务脚本字段：

```rust
pub struct TaskScript {
    pub script_text: Option<String>,
    pub script_file: Option<String>,
    pub cwd: Option<String>,
    pub env: BTreeMap<String, String>,
}
```

执行约束：

- 执行任何 route / schedule 脚本前，必须读取 provider/config 绑定的 IM grant，并确认具备 `execute_script` 权限。
- route / schedule 配置变更必须分别检查 `manage_route` / `manage_schedule` 权限。
- 手动执行 schedule 需要同时具备 `manage_schedule` 和 `execute_script` 权限。
- 必须设置 timeout
- 必须设置 max_output_bytes
- 默认不继承完整环境变量
- env key 必须在 allowlist 中
- cwd 必须在 allowlist 中，或由本地配置限定
- route 触发和 schedule 触发都必须走同一个 executor

建议复用 Remote Shell 的 policy 校验能力，避免产生第二套 shell 安全模型。IM Gateway 只负责根据 IM grant 判断是否允许进入脚本执行阶段；进入执行阶段后，cwd/env/timeout/output 等仍复用现有 shell policy 风格做细粒度约束。

### 并发策略

```rust
pub enum ConcurrencyPolicy {
    Forbid,
    SkipIfRunning,
    QueueOne,
}
```

第一阶段默认 `SkipIfRunning`。

### 超时

- 默认 timeout：30 秒
- 最大 timeout：由本地 IM Gateway 配置限制
- 超时后 kill 子进程
- run status 标记为 `timeout`
- 发送可选失败卡片

## 安全设计

### Secret 保护

- `app_secret` 不出现在 CLI 输出
- `app_secret` 不出现在 WebUI 明文
- `app_secret` 不进入 relay
- `app_secret` 不进入 caller local state
- `app_secret` 不进入 Recent Calls
- provider config 导出时默认不包含 secret

### Inbound 触发保护

默认拒绝所有 inbound event 触发脚本，必须显式配置 route。

route 必须限定至少一个触发边界：

- chat_id
- user_id
- card action value
- keyword / regex

不允许创建“任意消息触发任意脚本”的默认 route。

### Remote 管理保护

远程新增 route / schedule 是持久化执行能力，必须独立授权。

建议审批 UI 明确区分：

- send only
- read status/history
- manage targets/providers
- manage routes/schedules
- execute scripts

### 审计

所有管理操作写入 admin audit：

- provider create/update/delete
- target create/update/delete
- route create/update/delete/pause/resume
- schedule create/update/delete/pause/resume/manual run
- remote im send
- inbound event triggered task

### 数据保留

默认：

- event history：最近 1000 条或 7 天
- run history：最近 2000 条或 30 天
- stdout/stderr preview：最多 4 KiB
- 完整 stdout/stderr 默认不落盘，只保存 digest

## 错误处理

Feishu provider 需要把错误分层：

- config_invalid：app_id / secret / event config 错误
- auth_failed：token 获取失败
- connection_failed：长连接失败
- connection_reconnecting：自动重连中
- send_failed：发送消息失败
- rate_limited：飞书限流
- target_invalid：receive_id 不合法或无权限
- task_timeout：任务超时
- task_rejected：安全策略拒绝

WebUI 和 CLI 输出必须避免泄露 secret，同时保留 request id / error code 方便排查。

## 测试方案

本节是实现前的测试用例设计草案。正式实现时必须把真实场景用例落到 `human_tests/im-gateway.md` 并立即逐条执行；当前仍处于方案 review 阶段，所以不提前创建 human_tests 文件。

### 单元测试

| 用例 ID | 测试函数 | 前置数据 | 步骤 | 断言 |
| --- | --- | --- | --- | --- |
| UT-IMG-01 | `test_im_provider_config_masks_secret` | provider 含 `app_id=cli_abc`、`app_secret=sk_xxx` | 序列化 safe summary / CLI display / WebUI DTO | 输出包含 masked app_id 和 `secret_configured=true`，不包含 `sk_xxx`、token、secret_ref 明文 |
| UT-IMG-02 | `test_feishu_provider_creates_default_grant_from_app_id` | Feishu provider config | 调用 provider create store | 自动创建 `remote_im_gateway` grant，`im_grant_key=app_id`，scope 和 permission 默认值正确 |
| UT-IMG-03 | `test_feishu_token_cache_refreshes_before_expiry` | token 还有 4 分钟过期 | 调用 token resolver | 触发刷新；未到刷新窗口时复用缓存 |
| UT-IMG-04 | `test_feishu_send_card_builds_receive_id_type_query` | target 为 `chat_id=oc_xxx` | 构造 send request | URL query 包含 `receive_id_type=chat_id`，body 包含 `receive_id`、`msg_type`、`content`、`uuid` |
| UT-IMG-05 | `test_im_target_rejects_invalid_receive_id_type` | `receive_id_type=bad_id` | 创建 target | 返回校验错误，不落盘 |
| UT-IMG-06 | `test_im_route_requires_trigger_boundary` | route 无 chat/user/keyword/regex 限制 | 创建 route | 拒绝创建，错误说明必须设置触发边界 |
| UT-IMG-07 | `test_im_event_normalizer_maps_feishu_message_receive` | Feishu `im.message.receive_v1` payload | 归一化事件 | 得到 `ImEvent{event_type=message.receive, chat_id, user_id, text}`，raw 只保存 digest |
| UT-IMG-08 | `test_im_route_regex_extracts_named_groups` | regex `^/check (?P<service>\\S+)$`，消息 `/check bifrost` | route matcher 匹配 | 命中 route，脚本 env 中包含 `BIFROST_IM_MATCH_SERVICE=bifrost` |
| UT-IMG-09 | `test_im_route_v1_accepts_run_script_and_reply_only` | action 分别为 `RunScriptAndReply`、`RunSchedule`、`SendCard` | 创建 route | V1 只接受 `RunScriptAndReply`，其他 action 返回 unsupported |
| UT-IMG-10 | `test_im_route_reply_uses_script_completion_output` | 脚本 stdout/stderr/exit_code | 构造 reply message | 成功时回复 stdout 摘要；失败时回复 exit_code + stderr 摘要；超长输出被截断并保留 digest |
| UT-IMG-11 | `test_im_scheduler_computes_next_run_for_cron` | cron `*/5 * * * *` | 计算 next_run_at | next_run_at 大于 now 且符合 cron |
| UT-IMG-12 | `test_im_scheduler_timeout_marks_run_timeout` | sleep 脚本，timeout 100ms | 执行 schedule | 子进程被终止，run status 为 `timeout`，history 有记录 |
| UT-IMG-13 | `test_im_permission_matrix_rejects_missing_execute_script` | IM grant 只有 `read_status/send_message` | inbound message 命中 route | 记录 event，但拒绝执行脚本，写 audit，run status 为 rejected |
| UT-IMG-14 | `test_im_permission_matrix_allows_send_without_execute` | IM grant 有 `send_message` 无 `execute_script` | 执行 send card | send 成功；script run 仍被拒绝 |
| UT-IMG-15 | `test_remote_im_command_summary_masks_card_and_secret` | remote im send card + provider secret | 构造 remote command summary | summary 只含 target/msg_type/card_digest，不含完整 card、secret、token |
| UT-IMG-16 | `test_remote_im_provider_safe_summary_excludes_config_details` | provider config + secret + token cache | remote provider status/list DTO | 返回 channel state、masked app_id、secret_configured，不返回完整 provider JSON |
| UT-IMG-17 | `test_im_grant_revocation_blocks_send_schedule_and_inbound_script` | revoked IM grant | 分别执行 send、manual schedule run、inbound route | 三者都被拒绝，错误码稳定 |
| UT-IMG-18 | `test_im_script_policy_applies_cwd_env_timeout_output_limits` | cwd/env/timeout/output policy | 执行脚本 | 未授权 cwd/env 被拒，timeout 生效，输出被限制 |

### E2E 测试

E2E 必须使用 fake Feishu server，不能依赖真实飞书环境。fake server 至少包含：

- token endpoint
- message send endpoint
- long-connection mock WebSocket endpoint
- event push 控制接口
- request capture API

所有 E2E 启动 Bifrost 时必须使用临时 `BIFROST_DATA_DIR`，不能使用 9900 端口，必须带 `--no-system-proxy`。

#### `test_im_gateway_feishu_e2e.sh`

| 用例 ID | 场景 | 步骤 | 断言 |
| --- | --- | --- | --- |
| E2E-IMG-01 | 本地 provider + grant 初始化 | 启动 fake Feishu；启动 Bifrost；创建 provider | provider status 为 enabled；Remote Invoke Grants API 返回 `remote_im_gateway` grant，identity key 为 masked app_id |
| E2E-IMG-02 | 本地 target 创建与通道检查 | 创建 `chat_id` target；执行 `bifrost im channel check --target ...` | 返回 connected / sendable；不输出 secret |
| E2E-IMG-03 | 本地直接发送卡片 | 执行 `bifrost im send --target ... --card-file card.json --format json` | fake Feishu 捕获 `receive_id_type=chat_id`、`msg_type=interactive`；CLI 返回 message_id/request_id |
| E2E-IMG-04 | 长连接接收消息并执行脚本回复 | fake WebSocket 推送 `/check bifrost`；route 执行脚本 `printf ok` | 脚本完成后 fake Feishu 收到回复消息；run history 为 success；event history 有摘要 |
| E2E-IMG-05 | route V1 只支持脚本后回复 | 尝试创建 `RunSchedule` 或 `SendCard` inbound action | 创建失败，提示 V1 only supports script then reply |
| E2E-IMG-06 | route 无触发边界拒绝 | 创建无 chat_id/user_id/keyword/regex 的脚本 route | 创建失败；不会注册危险 route |
| E2E-IMG-07 | 缺少 execute_script 拒绝 inbound 脚本 | 移除 grant `execute_script`；推送匹配消息 | event 被记录；脚本未执行；fake Feishu 收到权限拒绝回复或 history 记录 rejected |
| E2E-IMG-08 | schedule 手动执行发送通知 | 创建 schedule；执行 `bifrost im schedule run ...` | fake Feishu 收到消息；run history 为 success |
| E2E-IMG-09 | schedule 超时 | 创建 sleep 脚本 schedule，timeout 100ms；手动 run | 子进程被终止；run history 为 timeout；有错误摘要 |
| E2E-IMG-10 | secret 防泄露 | 执行 provider list/status/history/runs；读取 call summary 和 history 文件 | 不包含 app_secret、token cache、secret_ref 明文、完整 provider JSON |

#### `test_remote_im_gateway_e2e.sh`

| 用例 ID | 场景 | 步骤 | 断言 |
| --- | --- | --- | --- |
| E2E-RIMG-01 | remote 授权 IM grant | 启动 relay / target / caller；完成 pairing；授权 `remote_im_gateway` | caller 可执行 `remote im channel check`；target Grants 列表显示 IM grant |
| E2E-RIMG-02 | remote channel check 安全摘要 | 执行 `bifrost remote im channel check --target ... --format json` | 返回 provider/target 可用性摘要；无 secret/config 详情 |
| E2E-RIMG-03 | remote send 通知 | 执行 `bifrost remote im send --target ... --card-file ...` | target fake Feishu 收到消息；caller 收到 message_id/request_id |
| E2E-RIMG-04 | remote schedule 管理 | 执行 add/pause/resume/run/delete | 每步返回稳定 JSON；run 时 fake Feishu 收到消息；delete 后列表不可见 |
| E2E-RIMG-05 | remote 不允许读取密钥/完整配置 | 执行 provider list/status/show 类命令 | 只返回 safe summary；没有 app_secret、secret_ref 明文、token cache、完整 provider JSON |
| E2E-RIMG-06 | revoke IM grant 后拒绝 remote 能力 | revoke grant；再次执行 channel check/send/schedule run | 全部拒绝，错误码为 permission denied / grant revoked |
| E2E-RIMG-07 | relay 不保存敏感明文 | 检查 relay call record / events / logs | call summary 只有 digest 和 target；无完整 card、secret、token |
| E2E-RIMG-08 | inbound 脚本仍受 IM grant 限制 | 通过 remote 去掉 `execute_script`；fake Feishu 推送匹配消息 | 脚本不执行；history 记录 permission denied |

### Human Tests

实现阶段必须新增：

```text
human_tests/im-gateway.md
```

并更新：

```text
human_tests/readme.md
```

建议正式用例：

| 用例 ID | 名称 | 前置条件 | 操作步骤 | 预期结果 |
| --- | --- | --- | --- | --- |
| TC-IMG-01 | Settings 一级 Tab | Bifrost 以临时数据目录启动 | 打开 Settings | 侧边 Tab 中出现 `IM Gateway`，且不是 Remote Invoke 子面板 |
| TC-IMG-02 | 创建 Feishu provider 并脱敏 | fake Feishu 可用 | 在 IM Gateway 创建 provider，输入 app_id/app_secret | 页面只显示 masked app_id 和 secret configured；不显示 secret 明文 |
| TC-IMG-03 | provider 自动创建 IM grant | 完成 TC-IMG-02 | 打开 Remote Invoke Grants | 出现 `remote_im_gateway` grant，identity key 为 masked app_id |
| TC-IMG-04 | Feishu 长连接 connected | provider 已启用长连接 | 在 Connections 查看状态 | 状态变为 connected，显示 last_connected_at |
| TC-IMG-05 | 创建 chat target | provider connected | 创建 chat_id target | target 列表显示 target，可执行 channel check |
| TC-IMG-06 | 本地发送卡片 | target 存在 | 上传/输入卡片 JSON，点击 Send | fake Feishu 或真实测试通道收到卡片，页面显示 message_id/request_id |
| TC-IMG-07 | 接收消息执行脚本并回复 | route 绑定 `/check xxx`，脚本输出 `ok` | 通过 fake Feishu 推送或真实飞书发送 `/check bifrost` | 脚本执行完成后原群收到结果消息，History 有 event 和 run |
| TC-IMG-08 | V1 拒绝非脚本后回复 action | provider connected | 尝试创建“收到消息后触发已有 schedule”的 route | UI/API 拒绝，提示 V1 only supports script then reply |
| TC-IMG-09 | 无边界 route 被拒绝 | provider connected | 创建没有 chat/user/keyword/regex 的脚本 route | 创建失败，提示必须设置触发边界 |
| TC-IMG-10 | schedule 创建与 next_run | target 存在 | 创建 cron schedule | 列表显示 enabled 和 next_run_at |
| TC-IMG-11 | 手动执行 schedule | schedule 存在 | 点击 Run now | 产生 run history，收到通知消息 |
| TC-IMG-12 | 暂停 schedule | schedule enabled | 点击 Pause，等待一个周期 | 状态为 paused，不自动执行 |
| TC-IMG-13 | 超时 schedule | sleep 脚本，timeout 很短 | 手动执行 | run status 为 timeout，子进程被终止 |
| TC-IMG-14 | Remote channel check | 完成 remote pairing 和 IM grant 授权 | 执行 `bifrost remote im channel check --target ... --format json` | 返回 safe summary，不含密钥/完整配置 |
| TC-IMG-15 | Remote send 通知 | remote 已授权 | 执行 `bifrost remote im send --target ... --card-file ...` | 消息发送成功，返回 message_id/request_id |
| TC-IMG-16 | Remote 管理定时任务 | remote 已授权 | 执行 schedule add/pause/resume/run/delete | 每步成功，run 时收到通知 |
| TC-IMG-17 | Remote 不可读密钥配置 | provider 有 secret | 执行 provider list/status/show 类 remote 命令 | 输出不包含 app_secret、secret_ref 明文、token cache、完整 provider JSON |
| TC-IMG-18 | 撤销 IM grant 后拒绝能力 | IM grant active | 在 Remote Invoke Grants revoke IM grant，再执行 send/schedule/inbound route | 全部被拒绝，History/Audit 有权限拒绝记录 |
| TC-IMG-19 | 缺少 execute_script 拒绝 inbound 脚本 | IM grant 仅保留 send/read | 发送命中 route 的消息 | event 被记录，脚本不执行，返回/记录 permission denied |
| TC-IMG-20 | 脚本策略受 Remote Shell 风格限制 | route 脚本使用未授权 cwd/env | 发送命中 route 的消息 | 脚本执行被拒绝，错误指向 cwd/env policy |
| TC-IMG-21 | 输出与日志不泄露 secret | 已执行 send/route/schedule/remote 命令 | 检查 CLI 输出、Recent Calls、IM History、relay 日志、store 文件 | 均不包含 app_secret/token/secret_ref 明文 |

## 校验要求

实现完成后至少执行：

```bash
cargo test -p bifrost-admin im_gateway -- --nocapture
cargo test -p bifrost-cli remote_im -- --nocapture
bash e2e-tests/tests/test_im_gateway_feishu_e2e.sh
bash e2e-tests/tests/test_remote_im_gateway_e2e.sh
cargo test --workspace --all-features
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

如果修改了前端：

```bash
pnpm --dir web test
pnpm --dir web lint
```

最后按修改范围选择执行：

```bash
bash scripts/ci/local-ci.sh --e2e-only shell
```

如果 IM Gateway E2E 被纳入新 suite，则改为对应 suite。

## 文档更新要求

实现阶段需要更新：

- `design/im-gateway.md`
- `human_tests/im-gateway.md`
- `human_tests/readme.md`
- 顶层 CLI help 中新增 `im` 命令说明
- 如 README 维护功能列表，则补充 `IM Gateway`
- 如 `skill_remote.md` 暴露 remote 能力，需要补充 `remote im` 的边界和授权要求

## 开放问题

1. `remote_im_gateway` 是否拆成 read / send / manage / execute 四个 scope？
2. 远程配置 provider secret 第一阶段已建议禁止；是否需要提供“引用 target 本地已有 secret_ref”的受控配置入口？
3. Feishu 长连接是否使用官方 Rust SDK、引入轻量 WebSocket client，还是通过独立 helper 进程承载？
4. route 脚本是否完全复用 Remote Shell policy，还是 IM Gateway 单独维护一套 task policy？
5. schedule 错过执行窗口后是否需要 catch-up？当前建议默认不补跑。
6. History 是否需要提供短期 raw payload debug 模式？当前建议默认关闭。
7. 第一阶段是否只支持飞书中国区 `open.feishu.cn`，还是同时抽象 Lark 国际区 base URL？
