# IM Gateway 设计方案

## 背景

Bifrost 早期只提供 CLI/Web/Remote Invoke 三个入口，用户希望把飞书、微信这类 IM 通道纳入统一接入点：既能让 Bot 在 IM 侧接收消息、触发脚本或 Agent 任务，也能让本地或远端调用主动向 IM 通道发送结果。此前相关能力散落在多个原型脚本里，缺少 provider 抽象、目标/路由/调度模型、持久化、权限边界和统一的 CLI/Admin API。

IM Gateway 把这些能力收敛为一个 Bifrost 内部模块：

- 以 `ImProvider` 抽象为中心，第一版内置 Feishu / Weixin，预留 WeChat / Webhook 扩展位。
- 复用 Bifrost 已有的 Remote Invoke 授权、Admin API、CLI 分发和 WebUI 布局。
- 保证 secret、token、完整 provider config 不被泄漏到日志、Remote Invoke summary 或前端 DTO。

IM Gateway 不承担 Runner loop 本身；外部 Runner 相关内容在 `im-gateway-codex-cli-chat-gateway.md`、`external-runner-live-guide.md`、`im-markdown-converter.md` 单独展开。本文只关注 gateway 层：连接、目标、路由、调度、消息发送、事件循环、持久化、权限与远端管理。

## 用户目标验证清单

### 必须实现

- Provider 抽象：`ImProvider` trait 覆盖 `validate_config` / `connect_events` / `send_card`；`ImProviderType` 至少包含 `Feishu` / `Weixin`，预留 `WeChat` / `Webhook`。
- Provider 生命周期：创建、编辑、启用、禁用、删除；`app_secret` 只保存为 `secret_ref`，API/CLI/WebUI 只展示 masked app_id + `secret_configured=true`。
- Feishu 长连接：单应用最多 50 个连接，事件推送 3s 内 ACK；`tenant_access_token` 按 `base_url + app_id + app_secret` 指纹缓存，多机器人隔离。
- Target 管理：`ImTarget` 存 `provider_id`、`receive_id`、`receive_id_type ∈ {chat_id, open_id, user_id, union_id, email}`，channel check 可探测 sendable。
- Route 管理：`ImRoute` 至少绑定 chat_id / user_id / keyword / regex 之一；V1 只接受 `RunScriptAndReply` 动作，脚本完成后把 stdout/stderr 摘要回复给触发消息所在会话。
- Schedule 管理：`ImSchedule` 支持 cron / interval，`task_type ∈ {script, agent}`；`ImScheduler` 在服务启动时开启常驻循环，按 `next_run_at` 消费。schedule 必须绑定 `message_channel`。
- 消息发送：Admin API + CLI 直接向 target/owner 发送 text / markdown / interactive card；`bifrost im send` 未传 `--target` 时默认发到 provider owner，`target_id="__owner__"`。
- 事件历史 / Run 历史 / outbound message log：默认最多 1000 / 2000 条或 7 / 30 天；stdout/stderr 预览 ≤ 4 KiB；完整输出默认只留摘要 + digest。
- Remote Invoke 扩展：新增 `CommandKind::ImGateway` 与 `GrantScope::RemoteImGateway`；细分 `ReadStatus` / `SendMessage` / `ManageTarget` / `ManageRoute` / `ManageSchedule` / `ExecuteScript` 权限位。
- WebUI 一级导航：`AI → IM Gateway`，子面板 `Connections` / `Targets` / `Routes` / `Schedules` / `History`；Provider 卡片单列布局、宽度 ≤ 750px 居中；Add Provider 三步向导（选类型 → 连接账号 → 高级配置）。

### 必须不破坏

- 不改变 Bifrost 已有的 CLI / Web / Admin API / Remote Invoke / Rules / Traffic 主流程。
- 不引入需要外部服务才能启动的强依赖；未启用 IM Gateway 的用户不感知 gateway 生命周期。
- `bifrost im *` 全部命令，无论本地还是 remote，都必须走同一份 policy / audit 检查；不能出现绕开权限的隐藏子命令。
- Provider `enabled=false` 或 `event_connection_enabled=false` 时必须立即停止长连接，重启后也不会被自动拉起。
- Remote Invoke summary、Recent Calls、message log 必须严格脱敏；已经 masked 的字段禁止在其他 DTO 里以明文回退。
- 不要求 secret 在远端保存；远端管理默认拒绝 provider secret 更新。

### 必须真实验证

- 通过真实飞书应用发起长连接，触发消息接收 / 脚本执行 / 卡片回复；owner 上线通知包含设备名与工作目录。
- 使用 `bifrost im send` 与 Admin API 向真实群组、真实 owner 分别发送 text / markdown / 卡片；同一进程内两个飞书机器人互不串号。
- 使用微信 Provider 完成二维码登录、long-poll 事件接收；删除或禁用 Provider 后不再出现 `weixin poll failed`。
- Remote pairing 授权 `remote_im_gateway` 后，caller 可执行 channel check、send、schedule add/pause/resume/run/delete；revoke 后立即被拒。
- schedule cron/interval loop 到点自动执行，Script 与 Agent 两类任务都记录 run history、通过绑定 `message_channel` 发送通知。

## 产品语义

### Gateway 是可插拔的 IM 接入层

Gateway 只负责“把 IM 消息接进来 / 把消息发出去 / 把定时任务派发出去”。业务侧的 Agent loop、Chat Gateway、进度卡、Markdown 转换均由上层模块消费 gateway 提供的 provider / target / event 抽象。这样：

- 新增 Feishu 或 Weixin 之外的 provider 只需要实现 `ImProvider` trait，注册到 `ImConnectionManager`，声明 `ImSendCapability`，其他模块不需要感知。
- Agent、Chat Gateway、Schedule 之间没有互相直连；他们全部通过 gateway 的 `send_msg`、outbound message log 和 target/route/schedule 抽象协作。
- 停用 gateway 或某个 provider 时，其他模块只失去发送能力，不会崩溃。

### Provider 能力驱动 UI 与工具 schema

Provider 声明 `ImSendCapability`（支持的消息类型、是否支持流式卡片、是否支持撤回、是否支持图片附件等）。Agent 的 `send_msg` 工具、进度卡渲染、Add Provider 表单、Schedule 通知等入口都按 capability 动态裁剪：飞书暴露 text/markdown/interactive card，微信只暴露当前支持的类型；不支持的字段直接从 schema/表单中隐藏，不出现在 tool description。

### 权限先审再执行

任何 IM 操作都先经过：

1. `ImProviderPolicy`：本地 provider 层权限，identity key 为 masked app_id。默认只有 `read_status/send_message`；`execute_script` / `manage_route` / `manage_schedule` 必须显式开启。
2. Remote Invoke `RemoteImGatewayAction` + `remote_im_gateway` grant：远端管理必须显式授权，`send/manage/execute` 之间不能被合并授予。
3. 具体命令 handler 的字段校验：例如 route 无触发边界拒绝、target `receive_id_type` 白名单、schedule `message_channel` 必填。

三层任一失败都立即 `permission_denied`，并写入 audit log；输出必须只保留 masked app_id、request id、错误码。

### secret 只出现在唯一持久化点

`app_secret`、token cache、pending 登录 secret 只保留在服务端 `secret_ref` / 内存 session；任何 API/DTO/日志/summary/history 出口都执行 `sanitize_provider`，只返回 `secret_configured`。远端管理默认不允许更新 secret；如未来支持“引用 target 本地已有 secret_ref”这类受控入口，需要单独授权位。

## 技术细节

### 模块布局

`crates/bifrost-admin/src/im_gateway/`：

- `mod.rs`：模块入口。
- `types.rs`：`ImProviderType` / `ImTarget` / `ImRoute` / `ImRouteAction` / `ImSchedule` / `ImTaskRun` / `ImEvent` / `ImMessageChannelBinding` / `MessageTargetMode` / `ImSendCapability` 等基础类型。
- `provider.rs`：`ImProvider` trait 与 `ImConnectionManager`。
- `feishu.rs` / `weixin.rs`：内置 provider 实现。
- `connection.rs`：长连接生命周期、reconnect supervisor（60s）。
- `provider_store.rs` / `target_store.rs` / `route_store.rs` / `schedule_store.rs` / `event_store.rs` / `run_store.rs` / `message_log_store.rs` / `external_cli_store.rs`：分文件 JSON 持久化。
- `scheduler.rs`：cron/interval 计算与常驻 loop。
- `event_router.rs`：Provider event → route matcher → task 派发。
- `task_executor.rs`：Script 任务执行器（cwd/env/timeout/输出上限）。

Handler 层 `crates/bifrost-admin/src/handlers/im_gateway/` 按功能拆分：`service.rs` / `providers.rs` / `event_loop.rs` / `agent_chat.rs` / `agent_reply.rs` / `agent_reply_attachments.rs` / `messages.rs` / `schedules.rs` / `agent_api.rs` / `chat_gateway.rs` / `busy_message_mode.rs` / `debug_inbound.rs` / `utils.rs`；子模块通过真实 `mod` 边界拆分。

### `ImProvider` trait

```rust
#[async_trait]
pub trait ImProvider: Send + Sync {
    fn provider_type(&self) -> ImProviderType;
    async fn validate_config(&self, config: &ProviderConfig) -> Result<ProviderValidation>;
    async fn connect_events(
        &self,
        config: ProviderConfig,
        sink: EventSink,
    ) -> Result<ConnectionHandle>;
    async fn send_card(
        &self,
        config: &ProviderConfig,
        target: &ImTarget,
        card: serde_json::Value,
        opts: SendOptions,
    ) -> Result<SendResult>;
}

pub enum ImProviderType { Feishu, Weixin, WeChat, Webhook }
```

`connect_events` 返回的 `ConnectionHandle` 支持 explicit `stop()`；`ImConnectionManager` 保证 `DELETE /providers/:id`、`PATCH enabled=false`、`event_connection_enabled=false` 时立即 `stop_connection(id)` 并清理微信登录 pending。

### Feishu Provider

- 优先使用 CardKit 长连接，第一版走轻量 WebSocket 客户端；未来官方 Rust SDK 稳定后再切换。
- 单应用最多 50 条长连接；启动时按 `secret_configured && enabled && event_connection_enabled` 自动连接；60s reconnect supervisor 保证短暂断线自动恢复。
- `tenant_access_token` 缓存 `HashMap<TokenCacheKey, TokenCache>`；`TokenCacheKey = SHA256(base_url + app_id + app_secret)`；`app_secret` 只参与摘要，不入日志。
- send API 请求 `?receive_id_type=<chat_id|open_id|...>`，body 包含 `receive_id / msg_type / content / uuid`；uuid 使用短随机值。
- Owner 上线通知：`build_online_notification_message(provider)` 输出 `你好，Bifrost 助手上线了` + `设备名称：...` + `工作目录：...`；设备名解析顺序 `BIFROST_DEVICE_NAME → COMPUTERNAME → scutil ComputerName → HOSTNAME → hostname → unknown`。重启上线通知补充当前 Runner 与最近轮次上下文。
- Add Provider 三步向导中 Feishu 走 device-style app registration：`POST /providers/feishu-setup/start` → 前端展示二维码 → 轮询 `status` → `POST /provider` 合成 provider。App Secret 仅在服务端内存 session 停留，落地时立即 `sanitize_provider`。
- Feishu Provider 使用的直连 HTTP client 不继承 Bifrost 自身代理与环境代理，避免服务端出站被本机代理回环影响。

### Weixin Provider

- 通过 iLink API 长轮询：`getupdates` 每 3s 一次；Provider 删除后必须停止 poll task，并清理登录 pending。
- 登录二维码：`POST /providers/:id/weixin-login/start` 返回二维码；扫码确认后写入 provider secret 并进入常驻 poll。
- 发送侧支持 text / markdown / image；不支持类型不进入 `ImSendCapability`。

### Route / Task Executor

`ImRouteAction::RunScriptAndReply { script_text | script_file, cwd, env, reply_target, reply_mode }`。V1 只支持此单 action；`RunSchedule` / `SendCard` 等 inbound action 明确拒绝。

Route 校验：至少配置 `chat_id | user_id | keyword | regex` 之一；regex 支持 named groups，命中后写入 `BIFROST_IM_MATCH_<NAME>` 环境变量。

Task Executor 应用 cwd/env/timeout/output policy：

- cwd 必须在允许目录白名单内。
- env 只透传显式声明的 key。
- timeout 到期后 SIGTERM → SIGKILL；run status = `timeout`。
- stdout/stderr 只保留 ≤ 4 KiB preview + digest；完整输出默认丢弃。

`reply_mode`：`stdout_summary` 回复 stdout 摘要；`stderr_on_failure` 失败时回复 exit_code + stderr 摘要；`none` 不回复。

### Schedule

`ImSchedule` 字段：

- `id / name / task_type ∈ {script, agent} / trigger ∈ {cron, interval} / message_channel`
- `script { script_text | script_file, cwd, env, timeout }`
- `agent { prompt / session_key / work_dir / system_prompt / runner_id / initial_prompt / conversation_ref }`
- `enabled / next_run_at / last_run_at / last_run_status`

Scheduler：`ImGatewayService::start_scheduler()` 在启动时开常驻 loop；`compute_next_run_at()` 支持 cron / interval / catch-up 关闭。手动 `POST /schedules/:id/run` 与自动触发共用同一执行函数。

Schedule 通道绑定必须显式；`message_channel.target_mode` 覆盖 `SourceThread` / `SourceUser` / `Owner` / `ConfiguredTarget`。执行完成后使用绑定通道的默认接收者发送通知，Agent 任务优先使用 `agent_final_response`。

ChatGPT Web Runner 与 Codex Runner 在 schedule 首次执行成功后把 `conversation_id` / `thread_id` 回写到 `agent.conversation_ref`，后续 run 复用同一会话，除非显式重置 schedule。

### Remote Invoke 协议扩展

```rust
pub enum CommandKind {
    // ... existing
    ImGateway,
}

pub enum GrantScope {
    // ... existing
    RemoteImGateway,
}

pub struct RemoteImGatewayCommand {
    pub action: RemoteImGatewayAction,
    pub payload: serde_json::Value,
    pub summary: RemoteImGatewaySummary, // masked, digest-only
}

pub enum RemoteImGatewayAction {
    ProviderList, ProviderStatus, ChannelCheck,
    SendMessage, SendCard,
    RouteList, RouteCreate, RoutePause, RouteResume, RouteDelete,
    ScheduleList, ScheduleAdd, SchedulePause, ScheduleResume, ScheduleRun, ScheduleDelete,
    EventHistory, RunHistory,
}
```

权限矩阵：

| Action 组 | 需要 grant 权限位 |
| --- | --- |
| `ProviderList` / `ProviderStatus` / `ChannelCheck` / `EventHistory` / `RunHistory` | `ReadStatus` |
| `SendMessage` / `SendCard` | `SendMessage` |
| `RouteCreate` / `RoutePause` / `RouteResume` / `RouteDelete` | `ManageRoute` |
| `ScheduleAdd` / `SchedulePause` / `ScheduleResume` / `ScheduleRun` / `ScheduleDelete` | `ManageSchedule` |
| script route 触发 | `ExecuteScript`（本地 provider policy，Remote 不能提权） |

`RemoteImGatewaySummary` 只保存 `target_type / target_masked / msg_type / content_digest / provider_masked / request_id`；relay call record、Recent Calls 均基于 summary。

### 持久化

`$BIFROST_DATA_DIR/im_gateway_*.json` 按功能拆分：

- `im_gateway_providers.json`
- `im_gateway_targets.json`
- `im_gateway_routes.json`
- `im_gateway_schedules.json`
- `im_gateway_events.json`
- `im_gateway_runs.json`
- `im_gateway_message_logs.json`
- `im_gateway_external_cli_agent.json`

每个 store 使用乐观锁 + 单文件写入，保留 sanitize 后的字段；secret 通过 `secret_ref` 引用独立 secret store。

### WebUI 信息架构

一级导航：`AI → IM Gateway`。子面板：

- **Connections**：Provider 列表，卡片单列 + 居中 750px；卡片顶部展示名称、类型、连接状态、启用、连接模式；正文一行一个字段展示 Secret / App ID / Owner ID / Agent Runner / Agent Work Dir；App ID / Owner ID / Work Dir 悬浮显示复制按钮，Secret 只显示 configured/missing。Add Provider 三步向导；Edit 表单密度紧凑（`im-provider-edit-form-compact` class + `size="small"`）。
- **Targets**：列表 + 编辑，channel check 按钮。
- **Routes**：列表 + 编辑，V1 只暴露 `RunScriptAndReply`；trigger 必填。
- **Schedules**：列表 + 详情弹窗；Add 弹窗支持 Script / Agent 两类任务，Agent 需选 Runner + preset prompt + work dir；每行支持 Run now、Pause、Resume、Delete；ChatGPT Web Runner 展示 `First-round Prompt`。
- **History**：Events / Runs / Outbound Messages 三 tab；Runs 点击进入详情复用 Schedule run detail。

窄宽度下 Connections/Routes 三列详情改为自适应网格；Targets/Schedules 表格开启横向滚动。

### IM 通道快路径命令

在 IM event loop idle/session-free 分支内拦截（不进入模型、不注册到 WebUI/API Agent slash router）：

- `/help`：追加 `IM 通道命令` 分组，列出 `/cwd`、`/runner`、`/q`、`/rq`、`/g`。
- `/cwd <绝对路径>`：切换当前 Provider `agent_config.work_dir`；路径必须存在且是目录；运行中排队到当前任务结束后执行；切换后清理 session history 和 thread 元数据。
- `/runner`：读取 `ExternalCliGatewayConfig` 输出可用 Runner 列表；`/runner <Runner>` 切换当前 Provider `agent_config.runner`；运行中只提示不抢占。
- `/q <消息>` / `/rq <序号>` / `/g <引导内容>`：队列控制命令，不作为普通用户消息。

## CLI + Web + Admin API

### CLI

- `bifrost im provider add|list|status|edit|remove [--provider <id>]`
- `bifrost im target add|list|remove|check --provider <id> --receive-id <...> --receive-id-type <chat_id|open_id|...>`
- `bifrost im route add|list|pause|resume|remove --provider <id> --trigger <chat_id|user|keyword|regex> --script-file <...>`
- `bifrost im schedule add|list|pause|resume|run|remove --provider <id> [--cron <expr>|--interval <secs>] [--script-file <...>|--agent-prompt <...>|--agent-prompt-file <...>|--agent-session-key <...>|--agent-work-dir <...>|--agent-system-prompt <...>] [--target <...>|--provider-owner]`
- `bifrost im send [--provider <id>] [--target <...>] --text <...>|--markdown-file <...>|--card-file <...>`
- `bifrost im history events|runs|messages --provider <id>`
- `bifrost im channel check --provider <id> --target <...>`

Provider 选择：显式 `--provider` 优先；单 enabled provider 自动选中；多 enabled provider 非交互式返回错误、交互式弹选择列表。

远端命令通过 `bifrost remote im ...` 使用等价子命令，透传到 `CommandKind::ImGateway`，落到 `RemoteImGatewayAction`。

### Admin API

`/_bifrost/api/im-gateway/*`：

- `POST/GET/PATCH/DELETE /providers[/:id]`
- `POST /providers/feishu-setup/start`, `GET /providers/feishu-setup/:session/status`, `POST /providers/feishu-setup/:session/provider`
- `POST /providers/:id/weixin-login/start`
- `POST/GET/PATCH/DELETE /targets[/:id]`
- `POST /targets/:id/check`
- `POST/GET/PATCH/DELETE /routes[/:id]`
- `POST/GET/PATCH/DELETE /schedules[/:id]`
- `POST /schedules/:id/run`
- `POST /messages/send`
- `GET /events`, `GET /runs`, `GET /messages`
- `GET/PATCH /agent`（Agent 全局配置 + `resolved_work_dir` 展示字段）

所有 provider DTO 走 `sanitize_provider`；Feishu setup status 只回 App ID、owner、`secret_configured=true`。

### Web

- 主侧栏 `AI` 一级导航 → 子导航 `IM Gateway`（不是 Remote Invoke 子面板）。
- Connections / Targets / Routes / Schedules / History 五个子面板；亮色/暗色主题同时验证。
- Add Provider 三步向导；Edit 弹窗紧凑；Provider 列表居中 750px；Provider 卡片单列布局；schedule 详情弹窗覆盖运行历史；History 行可点开详情。

## Sync 边界

- Provider / target / route / schedule / message log 走本地 store，Sync 只镜像 sanitize 后的元数据，不镜像 secret / token / 完整 provider config。
- Feishu 长连接状态、token cache、pending 登录 session 仅内存维护，不 Sync。
- schedule run detail、outbound message log、event history 中 stdout/stderr preview 默认不上传 Sync。
- Remote Invoke summary 严格脱敏；Recent Calls 只显示 masked 字段和 request id。
- 单元测试样本文件不进 Sync；测试凭据只允许从 `<USER_HOME>/ak.txt` 读取，禁止入库、日志、CLI 输出、截图。

## 实现切分

### Phase 1：Provider 抽象与 Feishu 长连接

- `ImProvider` trait + `ImConnectionManager` + `feishu.rs`。
- Provider CRUD、`sanitize_provider`、token cache 隔离、owner 通知。
- Admin API providers/targets 与 CLI `bifrost im provider|target`。
- 单元：`test_im_provider_config_masks_secret`、`test_feishu_token_cache_refreshes_before_expiry`、`test_feishu_send_card_builds_receive_id_type_query`。

### Phase 2：Route / Task Executor / Schedule

- `event_router.rs` + `task_executor.rs` + `scheduler.rs`。
- Route V1 `RunScriptAndReply`；trigger 边界校验；regex named groups → env。
- Schedule cron/interval loop；Script/Agent 两类任务；`message_channel` 必填。
- 单元：`test_im_route_requires_trigger_boundary`、`test_im_route_regex_extracts_named_groups`、`test_im_scheduler_computes_next_run_for_cron`、`test_im_scheduler_timeout_marks_run_timeout`。

### Phase 3：Weixin + Remote Invoke + WebUI

- Weixin Provider + 二维码登录 + poll 停止。
- Remote Invoke `CommandKind::ImGateway` + `GrantScope::RemoteImGateway` + 权限矩阵。
- WebUI Connections/Targets/Routes/Schedules/History 五面板；Add Provider 三步向导；Edit 表单紧凑；Provider 卡片单列 + 750px 居中。
- CLI `bifrost remote im ...`。

### Phase 4：命令快路径 + 观察性完善

- IM `/help` / `/cwd` / `/runner` / `/q` / `/rq` / `/g` 快路径。
- Provider 删除/禁用立即停止长连接；auto-connect / supervisor 尊重 `enabled && event_connection_enabled && secret_ref`。
- Provider/IM `/status` 展示 `resolved_work_dir`；重启上线通知补 Runner + 最近轮次。
- Outbound message log `trigger` 区分 `agent_tool:send_msg` / `schedule:<id>` / `manual_run:<id>` / `route:<id>` / `remote:<caller>`。

## 测试方案

### 单元测试

`crates/bifrost-admin` 内至少覆盖：

- `test_im_provider_config_masks_secret`、`test_feishu_provider_creates_default_policy_from_app_id`、`test_feishu_token_cache_refreshes_before_expiry`、`test_token_cache_key_isolated_by_app_id/secret/base_url`。
- `test_feishu_send_card_builds_receive_id_type_query`、`test_im_target_rejects_invalid_receive_id_type`。
- `test_im_route_requires_trigger_boundary`、`test_im_route_regex_extracts_named_groups`、`test_im_route_v1_accepts_run_script_and_reply_only`、`test_im_route_reply_uses_script_completion_output`。
- `test_im_scheduler_computes_next_run_for_cron`、`test_im_scheduler_timeout_marks_run_timeout`、`schedule_tools_create_update_list_delete_agent_schedule`、`schedule_agent_can_run_selected_external_runner_with_initial_prompt`、`schedule_external_runner_executes_from_configured_work_dir`。
- `test_im_policy_rejects_missing_execute_script`、`test_im_policy_allows_send_without_execute`、`test_im_policy_revocation_blocks_send_schedule_and_inbound_script`、`test_im_script_policy_applies_cwd_env_timeout_output_limits`。
- `test_remote_im_command_summary_masks_card_and_secret`、`test_remote_im_provider_safe_summary_excludes_config_details`。
- `provider_patch_detects_connection_disabling_changes`、`event_connection_requires_enabled_long_connection_and_secret`。
- `provider_agent_work_dir_resolves_global_default_directory`、`agent_config_response_includes_resolved_work_dir`、`im_status_text_uses_resolved_default_work_dir_when_session_has_no_override`。
- `agent_reply_target_uses_feishu_chat_id_for_event_channel` / `agent_reply_target_uses_feishu_open_id_without_chat_id` / `agent_reply_target_uses_weixin_sender_instead_of_owner`。
- `im_cwd_command_parses_existing_absolute_directory` / `im_cwd_command_rejects_invalid_paths` / `im_cwd_command_persists_provider_and_reinitializes_idle_session`。
- `im_help_includes_im_only_commands_without_dropping_builtins`、`im_runner_command_*`。
- `online_notification_message_uses_provider_work_dir_override` / `online_notification_message_falls_back_to_process_work_dir`。
- `provider_create_payload_maps_app_secret_without_exposing_it`。

### E2E 测试

- `test_im_gateway_feishu_e2e.sh`：fake Feishu server（token endpoint、send endpoint、mock WebSocket、event push 控制、request capture）；覆盖 provider 初始化、target 创建、卡片发送、长连接接收 + route 脚本回复、V1 action 拒绝、无边界 route 拒绝、缺 execute_script 拒绝、schedule 手动执行、schedule 超时、secret 防泄露。
- `test_remote_im_gateway_e2e.sh`：pairing + `remote_im_gateway` 授权后覆盖 channel check、send、schedule CRUD、revoke 后拒绝、relay 不留敏感、inbound 脚本仍受 provider policy 限制。
- `test_weixin_provider_e2e.sh`：mock iLink，删除 provider 后停止 poll；`event_connection_enabled=false` 且有 secret 时重启不 poll。
- `test_im_cli_provider_selection_send_owner.sh`：`bifrost im send --text ...` 单 provider 自动选择并默认发 owner。
- 所有 E2E 用临时 `BIFROST_DATA_DIR`、非 9900 端口、`--no-system-proxy`。

### 真实场景测试

`human_tests/im-gateway.md` 覆盖：

- TC-IMG-01 ~ 21：一级导航、Provider 创建脱敏、自动 policy、Feishu 长连接、target/route/schedule CRUD、V1 action 拒绝、无边界 route 拒绝、schedule 手动/暂停/超时、Remote channel check/send/schedule 管理、revoke 后拒绝、缺 execute_script 拒绝、脚本 cwd/env/output 限制、secret 防泄露。
- TC-IMG-32 Provider 创建 secret 映射回归、TC-IMG-35 多机器人 token 隔离、TC-IMG-37 CLI provider 选择、TC-IMG-61 Provider 默认工作目录展示、TC-IMG-62 owner 上线通知设备名、TC-IMG-66 微信删除后停止 poll、TC-IMG-67 Feishu Agent 回复跟随来源 chat_id。
- Add Provider 三步向导（Weixin / Feishu 二维码流程）、Edit 表单密度、Provider 列表宽度约束、Schedules Script/Agent 手动新增、Schedule 详情运行历史、Provider 卡片单列 hover 复制。
- IM 通道 `/help` / `/cwd` / `/runner` 命令；亮色/暗色主题双重验证。

### 覆盖率与项目校验

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p bifrost-admin im_gateway -- --nocapture`
- `cargo test -p bifrost-cli remote_im -- --nocapture`
- 受影响 E2E：`test_im_gateway_feishu_e2e.sh` / `test_remote_im_gateway_e2e.sh` / `test_weixin_provider_e2e.sh` / `test_im_cli_provider_selection_send_owner.sh`。
- `cargo test --workspace --all-features`
- 前端改动 `pnpm --dir web test` / `pnpm --dir web lint` / `pnpm --dir web build`。
- 按修改范围评估 `scripts/ci/local-ci.sh`（如新增 IM Gateway suite 需并入）。

本机 no-local-coverage 约定下不跑 `make coverage`。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标：provider 抽象、Feishu 长连接与 token 隔离、Weixin poll 生命周期、route V1 action、schedule cron/interval + message_channel、`bifrost im send` 默认 owner、Remote Invoke 权限矩阵、WebUI 三步向导 + 单列卡片、`/cwd` `/runner` `/help` 快路径。
- 复核 diff：Handler 模块化拆分是否遵守 `pub(super)` 边界、单文件 ≤ 1500 行；`sanitize_provider` 是否覆盖所有 provider 出口；Remote summary 不含完整 card。
- 复测：`cargo test -p bifrost-admin im_gateway`、focused unit（token cache 隔离、agent_reply_target、im_cwd_command、im_runner_command、schedule_tools、online_notification_message、provider_patch_detects_connection_disabling_changes）；抓 outbound message log 抽样验证脱敏。

### 第 2 轮

- 复查第 1 轮修复后的 diff、human_tests 索引、Remote grant 显示。
- 复跑 Feishu / Remote / Weixin / CLI provider 选择四条 E2E 与 Playwright IM 面板脚本。
- 检查真实上线通知设备名、亮暗主题、Add Provider 二维码流程；如发现 provider capability、Remote 授权、secret 脱敏、schedule loop 或 message_channel 漂移缺口，追加第 3 轮。

## 风险与决策点

- **Provider secret 远端管理**：第一阶段禁止远端更新 secret；后续若开放，必须新增独立 grant，且只允许引用已经存在的 `secret_ref`。
- **Schedule catch-up**：默认不补跑错过窗口；如需要补跑，须显式打开开关并写审计。
- **History raw payload debug**：默认关闭，只留 sanitize summary + digest；调试模式必须显式开启并限时。
- **Feishu 中国区 vs Lark 国际区**：第一阶段只锁定 `open.feishu.cn`；Lark 国际区抽象为 `base_url`，只在 tenant_brand=lark 时使用 Lark accounts endpoint。
- **Feishu 长连接实现**：先用轻量 WebSocket；若官方 Rust SDK 稳定，评估切换成本。
- **多实例 provider 冲突**：飞书单应用最多 50 连接，集群模式随机投递；第一版不共享 provider secret，多实例在线只做 UI 提示。
- **微信 iLink 稳定性**：iLink 接口无正式官方文档；出现协议变化时必须回退到二维码重登流程；相关错误码走 `send_failed` / `rate_limited` / `auth_failed` 分类，不再直接 panic。
- **Remote Invoke summary 反向工程**：所有 Remote 出口只保留 masked/digest 字段；如未来需要暴露 `content_preview`，必须走独立 grant，并只保留 UTF-8 安全 ≤ 200 字符预览。
