# ChatGPT Web Adapter 设计方案

> 状态：实施中
> 实现入口：`crates/bifrost-admin/src/im_gateway/chatgpt_web.rs` 与 `chatgpt_web/{artifacts,browser,diagnostics,images,interaction,native,send,storage,tests}.rs`。
> 命名注：文档使用 `AgentRunner*` 作为最终概念命名；实际代码当前仍沿用 `ExternalCli*`（`ExternalCliGatewayConfig / ExternalCliAgentSettings / ExternalCliChannelSettings / ExternalCliRunRequest`），Chat Gateway 公共前缀为 `/_bifrost/api/im-gateway/chat`；`/agent/runners` 独立命名空间 planned, not yet shipped as of 2026-06-16。

## 背景

Agent Chat runner 执行配置最终拆成两层：

- Runner：实例层。`AgentRunnerRegistry` 承载 `defaultRunnerId + runners + channels`，Provider/Channel 只覆盖 `runnerId / enabled / deliveryMode`。
- Adapter：实现层。每个 runner 里有 `adapter` 与 `adapterConfig`，内置 adapter 包括 `codex / custom / mock / chatgpt_web`。
- Chat Gateway：调用层。`/_bifrost/api/im-gateway/chat`、`/chat/stream`、`/runs/:runId`、`/runs/:runId/stop` 提供 run/stream/detail/stop 的统一入口。
- Work dir：上下文层。work dir 不属于 runner 配置，从 IM Provider Agent work dir 继承，缺省再从 Agent 全局 work dir 继承。

ChatGPT Web 的正确落点是新增内置 adapter `chatgpt_web`：Runner 只是普通 Agent Runner，adapter 承担浏览器登录、CDP 控制、native HTTP 读写、artifact 脱敏等全部差异。IM Route、Schedule、WebUI 都只选择 runner；ChatGPT Web 的差异不能泄漏到 runner 类型、Provider 配置或 route action 枚举。

## 用户目标验证清单

### 必须实现

- 新增 `chatgpt_web` adapter，支持创建对话、发起对话、列出对话、获取消息、等待输出结果、展示最终结果。
- 一个设备只登录一次；登录态存储在运行 Bifrost 的本机数据目录，后续 run 自动复用。
- 每次 run 开始前自动检查登录态；登录失效时给出明确反馈，并由 Bifrost 后端自动弹出 Edge/Chromium 浏览器让用户完成登录。
- 登录完成不能只依赖 `Authorization` 或 `accounts/check` 流量证明；登录页必须同时出现可见且可用的 composer，并且不再显示账号选择器，否则继续等待用户选定账号。“欢迎回来”只有出现在可见 dialog 中才视为阻塞信号，普通首页正文的欢迎语不能单独阻塞登录完成。
- headless 配置遇到登录页 / Cloudflare / 真人验证时，adapter 必须临时切换到 headed 等待用户处理并刷新登录态；处理完成后自动关闭 headed browser，让当前重试与后续运行都恢复 headless。
- 登录弹窗、cookie/header 提取、登录状态验证都属于 `chatgpt_web` adapter 能力，不能依赖 Agent 操作浏览器或外部脚本/skill。
- 与 runner/adapter 抽象对齐：Chat Gateway、IM Provider、IM Route、Schedule、WebUI 都选择 runner；runner 再通过 `adapter = "chatgpt_web"` 进入 ChatGPT Web 执行实现。

### 必须不破坏

- 现有 runner 选择语义：`default_runner_id`、Provider channel `runner_id` override、delivery mode 继续生效。
- 现有 `codex / custom / mock` adapter、run detail、stop API 与 IM 入站主流程。
- 不把 ChatGPT Web cookie、Authorization、Cloudflare token、sentinel token、完整 headers 写入普通日志、message log、run summary 或可远程读取的接口。
- 不跨设备同步登录态；登录态仅属于运行 Bifrost 的本机设备。
- 不把 Bifrost Traffic 抓到的历史 token 当作长期认证来源；Traffic 只用于恢复接口契约与调试。
- 不把 work dir 放回 runner 配置；ChatGPT Web adapter 不使用 repo work dir 作为认证或请求来源。
- headless blocker 临时 headed fallback 不写回 runner 配置；fallback 只作用于当前 run。

### 必须真实验证

- 用 Bifrost Traffic 确认 Edge/Chromium 中 `chatgpt.com` 的真实接口契约。
- 用本机真实浏览器 profile 完成首次登录，随后关闭浏览器再用 `chatgpt_web` adapter 执行 `list/get/ask`。
- 手动使登录态失效或使用空 profile，验证 adapter 自动弹出浏览器并给出可理解反馈。
- 使用停留在“欢迎回来 / 选择一个账户”的 profile，验证 adapter 不会误报 `LoggedIn`；选定账号、composer 可用后才结束登录等待。
- 验证最终结果来自 `GET /backend-api/conversation/{conversation_id}` 的 `current_node` assistant text，而不是提交请求的短 SSE handoff。

## 产品语义

### Runner 与 Adapter

- Runner 是可命名配置：`id / enabled / adapter / adapterConfig / instructions / skillPaths / injectBifrostTools / deliveryMode`。
- Adapter 是执行实现：`codex / custom / mock / chatgpt_web`。ChatGPT Web 与 Codex 的差异应被 adapter 吸收，而不是泄漏到 IM event loop、Provider 配置或 Chat Gateway API。

架构最终形态：

~~~text
AI -> Agent -> Runners
        |
        +-- runner "codex"        adapter = "codex"
        +-- runner "custom-local" adapter = "custom"
        +-- runner "chatgpt-web"  adapter = "chatgpt_web"

Chat Gateway / IM Provider Agent / IM Route / Schedule
        -> resolve effective runner_id
        -> load runner settings
        -> Adapter Registry dispatch by runner.adapter
~~~

### 结构化输出与投递

ChatGPT Web 页面可能把一次回答渲染成多条 assistant message：前面的短消息通常是过程性说明，最后一条才是最终答案。adapter 以 `data-message-author-role="assistant"` + `data-message-id` 作为自然批次边界，按页面顺序整理输出：

- CLI / Chat Gateway stream：过程批次为 `assistant_delta`（`raw.phase = "thinking"`）；可识别工具调用为 `tool_finished`（`raw.phase = "tool"`）；最后答案为 `assistant_final`（`raw.phase = "final"`）。
- IM 通道：只投递过程批次与最终答案，不投递工具调用事件，避免 IM 出现调试噪音。
- 图片结果：ChatGPT 生成图先缓存到本机附件目录；IM 文本卡片移除本地图片 Markdown 后按顺序逐张发送图片消息；CLI JSON 保留最终文本中的本地图片 Markdown，便于调用方读取附件路径。
- DOM fallback 不能只凭文本判断最终态。文本只作为候选内容；允许重新提取最新消息并返回前必须确认页面控制态已恢复：stop/cancel 按钮消失、composer 可见且可用；composer 中仍有待发送文本时提交按钮必须可见。空 composer 下发送位切回语音/提交控制不表示仍在输出。
- DOM fallback 候选稳定性基于内容签名（turn/message id、最终文本、自然批次数、图片数、附件数），而非文本长度。签名变化即重置稳定窗口。硬 busy 信号：`data-testid="stop-button"` 与中英文 Stop/Cancel aria-label。页面空闲后按签名短稳定确认：图片/短文本 2s、中等文本 3s、超长文本 5s。发送按钮 selector 禁止使用 `composer-submit-btn` 这类 stop 也带的非语义 class；发送前必须重新检查 stop button，可见则清空 composer 草稿并等待 stop 消失后再发送；超时才返回 `conversation_busy`。
- `wait` operation 不能只依赖进程内 `ConversationTab` 池。send 结束后浏览器 tab 仍打开，但内存池会随进程退出而消失。DOM fallback 找不到 tab 时，必须通过共享 browser session 的 DevTools target 列表重新发现 `/c/{conversationId}`，attach CDP 后再提取；只有浏览器也找不到会话页时才返回 `NotFound`。
- 图片消息可能渲染为后续 `section[data-testid^="conversation-turn-"]`（正文只有 `ChatGPT 说：`），图片在 section 内的 `estuary/content` URL。DOM fallback 必须把最后一个 user turn 之后的这类 image-only section 当作 assistant 结果；空壳文本且图片数为 0 必须继续等待。
- DOM 提取和 `allMarkdownTexts` 自然批次必须保存完整文本，不允许固定字符数截断；`response`、`last_message.md`、`result.json` 必须保留长任务最终输出全文。

## 技术细节

### 配置模型

目标命名 `AgentRunnerSettings / AgentRunnerRegistry / AgentRunnerChannelSettings`（planned, not yet shipped as of 2026-06-16）；当前落地 `ExternalCliGatewayConfig / ExternalCliAgentSettings / ExternalCliChannelSettings`，`adapter_config` 字段目前是强类型 `ExternalCliAdapterConfig`（重命名与 `adapter_config: serde_json::Value` adapter-specific schema 拆分 planned）。

约束：

- `adapterConfig` 是 adapter-specific schema，由 `adapter` 决定解析方式。
- 后端按 adapter capabilities 校验，拒绝未知 adapter 与不适用字段。
- WebUI 根据 adapter capabilities 展示字段。
- ChatGPT Web Runner 的浏览器用户数据只能来自共享 profile：默认路径 `BIFROST_DATA_DIR/agent/im_gateway/chatgpt_web/browser_profile`；send 与 wait 复用同一 `profileDir`。
- `BIFROST_DATA_DIR/im_gateway/runs/<run_id>/` 只允许保存 run artifact，禁止创建 run-local 完整 Chromium 用户数据目录，避免孤儿浏览器进程与缓存膨胀。

Runner 配置样例：

~~~json
{
  "version": 1,
  "defaultRunnerId": "codex",
  "runners": {
    "chatgpt-web": {
      "enabled": true,
      "adapter": "chatgpt_web",
      "adapterConfig": {
        "browser": {
          "channel": "edge",
          "profileDir": "admin/chatgpt_web/browser_profile",
          "openOnAuthRequired": true,
          "keepBrowserOpenAfterLogin": false,
          "executionMode": "headed",
          "closeTargetAfterRun": true
        },
        "chatgpt": {
          "baseUrl": "https://chatgpt.com",
          "defaultModel": "auto",
          "timezone": "Asia/Shanghai",
          "nativeHttpForRead": true,
          "browserFetchForWrite": true,
          "pollIntervalMs": 1000,
          "timeoutSecs": 7200
        },
        "auth": {
          "statePath": "admin/chatgpt_web/auth_state.json",
          "accountFingerprintPolicy": "warn_and_require_relogin"
        }
      },
      "injectBifrostTools": false,
      "deliveryMode": "final_reply"
    }
  },
  "channels": {
    "feishu-sre": {
      "runnerId": "chatgpt-web",
      "enabled": true,
      "deliveryMode": "final_reply"
    }
  }
}
~~~

`chatgpt-web` 是 runner id，`chatgpt_web` 是 adapter id。文档、API、UI 必须清楚区分这两个概念。

### Adapter Trait

目标 trait（planned）：

~~~rust
#[async_trait]
trait AgentRunnerAdapter {
    fn adapter_id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn capabilities(&self) -> AdapterCapabilities;
    async fn run_turn(&self, input: AgentRunInput, settings: AgentRunnerSettings, sink: AgentProgressSink) -> Result<AgentRunResult>;
    async fn stop(&self, run_id: &str) -> Result<()>;
}
~~~

当前落地：`chatgpt_web` 直接复用 `ExternalCliRunRequest / ExternalCliRunStatus / ExternalCliProgressEvent*`，以 `ADAPTER_ID = "chatgpt_web"` 常量与显式 dispatch 区分。

`ChatGptWebAdapter::run_turn` 主流程：`ensure_authenticated -> dispatch operation -> write artifacts -> emit canonical progress events -> return final result`。

### Adapter Capabilities

~~~json
{
  "adapterId": "chatgpt_web",
  "capabilities": {
    "process": false,
    "browserLogin": true,
    "conversationList": true,
    "conversationGet": true,
    "conversationCreate": true,
    "conversationWait": true,
    "assistantFinal": true,
    "assistantDelta": false,
    "skills": false,
    "bifrostTools": false,
    "workDir": false,
    "stop": true
  }
}
~~~

WebUI 选择 `chatgpt_web` 后：隐藏 executable/args/env/sandbox/approval policy/skill paths；展示 Browser、Auth、ChatGPT、Check Login、Open Login Browser、Logout、Test Ask；明确显示不使用 work dir。

### 登录态与 BrowserLoginBroker

登录态状态机：`NotConfigured -> LoggedOut -> Checking -> LoggedIn -> Expired -> AuthRequired -> BrowserOpened -> LoggedIn -> LoginStopped -> LoginWindowClosed`。

`ensure_authenticated()` 流程：

1. 读取 `auth_state.json`。
2. 使用保存的 cookie、user-agent 与捕获的认证 header 对 `GET /backend-api/accounts/check/v4-2023-04-27?timezone_offset_min=<offset>` 做 native HTTP 轻量探测。
3. 同时满足三组信号才是 `LoggedIn`：
   - browser traffic identity：真实 `accounts/check` 请求存在 `Authorization` Bearer token。
   - JWT identity：脱敏 claims 中 profile email 存在且 verified，`user_id/chatgpt_user_id/account_user_id` 至少一个是 `user-` 形态，`chatgpt_account_id` 存在，`exp` 未过期。
   - account check：返回 200，至少一个账号满足 `can_access_with_session=true / account_user_id 为用户形态 / account_id 存在 / is_deactivated=false`。
4. 返回 guest account / anonymous / JWT 缺少字段 / JWT 过期 / 401/403 / Cloudflare / HTML 登录页 / sentinel 错误 / 账号 fingerprint 不匹配时，进入 `AuthRequired`。
5. runner `browser.openOnAuthRequired=true` 时由 `BrowserLoginBroker::open_login()` 弹出浏览器。
6. Broker 通过 CDP 打开 `https://chatgpt.com/`，监听真实 `accounts/check` 请求捕获可复用认证 header。
7. 捕获后还要确认账号选择器已消失、composer 可见且可用，再导出 `chatgpt.com` cookies + header 写回 `auth_state.json`（权限 `0600`）；若页面仍停留在账号选择器则继续等待。
8. 立即再执行一次 native HTTP probe。native 通过则继续 run；native 因本机网络/代理/TLS 失败但当次浏览器 `accounts/check` 响应证明可用账号存在时，短期兜底为 `LoggedIn` 并保留失败原因；不能在后续 run 中替代当前 native `accounts/check`。
9. 登录等待无固定超时：用户完成 / 关闭登录窗口 / WebUI stop login 时才结束。关闭窗口或主动停止都返回 `auth_required`，错误文本区分 `login window was closed` 与 `login was stopped by request`。

`BrowserLoginBroker` 是 Bifrost 后端能力，不是 Agent browser automation：定位 Edge/Chrome 可执行文件、分配 remote debugging 端口、启动持久 profile、监听 CDP Network 事件、导出 cookies/headers、以 native HTTP 复验、按需保留或关闭 CDP target、脱敏写入 auth state。

### ExecutionBrowserController

执行阶段由 `ExecutionBrowserController` 负责。真实 IM 稳定性验证期默认 headed，便于观察页面输入与发送；稳定后可切回 headless。主流程：headed 启动 Edge/Chromium 与 runner profile；等待可见 composer（非隐藏 textarea）；新会话通过发送无 `conversation_id` 的首条消息创建；续接会话打开 `/c/{conversation_id}`，等待可见 composer 后插入文本并点击发送；捕获 `/backend-api/f/conversation` SSE handoff 获取 `conversation_id` 与 `turn_exchange_id`；轮询 `GET /backend-api/conversation/{conversation_id}` 直到 assistant 状态 `finished_successfully`；每次 native read 使用有界超时，短暂失败按 heartbeat-visible 状态计；长时间不可读按 grace window 收敛为失败，不永久占住 IM 队列。

心跳与恢复：handoff 等待期间检查浏览器进程、CDP WebSocket、页面 `Runtime.evaluate` probe；任一失败快速返回 `browser_unavailable`。短 SSE handoff 中断不能直接判失败：必须先从页面 URL、session 映射或显式 `conversationId` 恢复，再进入长轮询。

失败例：

~~~json
{
  "status": "failed",
  "error": {
    "kind": "browser_ui",
    "message": "composer not ready",
    "diagnostic": {
      "url": "https://chatgpt.com/",
      "title": "ChatGPT",
      "composerCount": 0
    }
  }
}
~~~

诊断仅含 URL、title、状态、元素计数、按钮状态、HTTP status、request id；禁止 cookie/Authorization/`x-oai-is`/页面私密内容。

### 读写分层

读类接口优先 native HTTP，共用 `ensure_authenticated()` 保存的 Cookie/UA/认证 header：

- `GET /backend-api/conversations`
- `GET /backend-api/conversation/{conversation_id}`
- `GET /backend-api/conversation/{conversation_id}/stream_status`

native 失败但浏览器 `accounts/check` 仍成功时，读类接口可降级 browser-context fetch，并在 artifact 记录降级原因（不含 secret）。

写类接口必须 headless browser-context 通过真实前端触发：

- `POST /backend-api/f/conversation`
- `POST /backend-api/f/conversation/prepare`
- `POST /backend-api/conversation/init`

写请求依赖 sentinel、conduit、Cloudflare、browser environment、turn trace 等动态 token；native 复刻已触发 unusual activity。headless browser context 不可用时不能自动降级 native 写请求；必须归类为 `AuthRequired / ChallengeRequired / BrowserUiBlocked / UnsupportedWebContract` 并返回脱敏诊断。

### 操作行为

- `list`：`GET /backend-api/conversations?offset=&limit=&order=updated&is_archived=false&is_starred=false` 返回 id/title/create_time/update_time/is_archived/is_temporary_chat。
- `create`：创建新会话上下文并返回本地 root message id；若 ChatGPT Web 不支持空会话持久化，则返回待发送上下文，真实 `conversation_id` 由首次 send 后 handoff 决定。
- `send`：headless browser context 操作可见 composer，由 ChatGPT Web 前端触发 `POST /backend-api/f/conversation`；返回 `conversation_id / turn_exchange_id / resume topic`。
- `wait`：轮询 `GET /backend-api/conversation/{conversation_id}` 等待最终结果；停止条件为 `current_node` 是 assistant text、状态 finished、content parts 非空。每 `pollIntervalMs` 轮询一次，最长 `timeoutSecs`（默认 7200s，允许扩展）。长任务持续检查 run 目录下 `stop_requested` marker；超时返回 `timed_out`。
- `get`：返回完整消息树脱敏摘要（user text / assistant text / message id / parent / children / status / create/update time）；默认不返回 hidden system、thoughts、reasoning recap 与内部 metadata 原文。
- `ask`：组合 `ensure_authenticated -> send -> wait -> get -> final_response`。

## CLI

CLI 侧无 ChatGPT Web 专属命令。通用 `bifrost im-gateway chat run --runner chatgpt-web ...` 走 Chat Gateway，adapter 差异透明。诊断入口通过 `bifrost im-gateway chat adapter status --runner chatgpt-web` 查询登录态。

## Web / Admin API

### Chat Gateway 请求

~~~json
{
  "runnerId": "chatgpt-web",
  "operation": "ask",
  "message": "你好，你会什么",
  "providerId": "feishu-sre",
  "sessionKey": "feishu-sre:user:123",
  "deliveryMode": "final_reply",
  "params": { "conversationId": null, "model": "auto", "timeoutSecs": 7200 }
}
~~~

解析规则：`runnerId` 显式优先；未传则用 provider/channel effective runner，再退回 global default runner。请求不直接传 `adapter`；adapter 只能来自 runner 配置。支持 `ask / list / create / send / wait / get`。

响应带 runner + adapter：

~~~json
{
  "runId": "runner-run-id",
  "status": "succeeded",
  "response": "最终回答",
  "runner": { "id": "chatgpt-web", "adapter": "chatgpt_web" },
  "artifacts": { "conversationId": "abc", "finalNodeId": "node-id" }
}
~~~

### 已实现路由

~~~text
GET   /_bifrost/api/im-gateway/chat/config              # 全量 ExternalCliGatewayConfig
PATCH /_bifrost/api/im-gateway/chat/config              # 全量替换
POST  /_bifrost/api/im-gateway/chat/stream              # 单次 run，SSE 流
POST  /_bifrost/api/im-gateway/chat/runner-calls/stream # caller -> runner 嵌套调用
GET   /_bifrost/api/im-gateway/chat/runs/:run_id        # run 详情
POST  /_bifrost/api/im-gateway/chat/runs/:run_id/stop   # stop run
GET   /_bifrost/api/im-gateway/chat/adapters/chatgpt-web/auth/status
POST  /_bifrost/api/im-gateway/chat/adapters/chatgpt-web/auth/open
POST  /_bifrost/api/im-gateway/chat/adapters/chatgpt-web/auth/stop
~~~

目标路由 `/_bifrost/api/agent/runners*` 与 `/_bifrost/api/agent/runners/:runner_id/adapter-actions/*` planned, not yet shipped as of 2026-06-16。

### WebUI

入口 `AI -> Agent -> Runners`。Runner 列表展示 ID / Adapter / Enabled / Delivery mode / Last run status。Runner 编辑弹窗新增 `ChatGPT Web` adapter；选中后隐藏 CLI 专属字段，展示 Browser / Auth / ChatGPT 配置块，附 Check Login / Open Login Browser / Logout / Test Ask 按钮；Auth state 显示 Logged in / Expired / Browser opened / Login timeout；账号显示脱敏 display name/email；Browser profile path 折叠展示。Provider 编辑页只展示 inherit / override runner id / delivery mode override / effective runner preview。亮暗主题都必须验证。

## Sync 边界

- Runner 配置与 provider channel override 参与既有 Chat Gateway config sync 语义；`chatgpt_web` adapter 不额外新增 sync。
- `auth_state.json`、Cookie、Authorization、sentinel 只存本机；禁止进入 sync、run summary、Remote IM 与 WebUI 普通详情。
- `runner-call:*` 子会话不同步。
- 多设备之间需要各自登录一次 ChatGPT Web；跨设备共享登录态不在本方案范围。

## Run Artifact 与安全脱敏

每次 run 写入 `runs/<run_id>/`：

~~~text
request.json
runtime_snapshot.json
auth_probe.json
conversation_handoff.json
conversation_final.json
failure_diagnostics.json
conversation_response.json
page_dom.json
page_dom.html
normalized_events.jsonl
result.json
~~~

安全规则：

- `auth_state.json` 权限 `0600`，仅本机 Bifrost 数据目录；不进入 run artifact、日志、Remote IM 或 WebUI 普通详情。
- `request.json` 不写入 cookie / Authorization / sentinel token / x-oai-is / x-conduit-token。
- `runtime_snapshot.json` 记录 runner id / adapter id / capabilities / header keys（不含 values）。
- `conversation_final.json` 默认只保存消息摘要与最终 assistant text；debug 模式也必须先脱敏。
- 任意失败必须尽力写 `failure_diagnostics.json`；已知 `conversation_id` 时同时保存 `conversation_response.json`；无论是否已知 `conversation_id` 都尽力保存 `page_dom.{html,json}`。
- `page_dom.json` 只保存 URL/title/readyState/composer 摘要/截断 body text；完整 DOM 写入 `page_dom.html`。抓取失败必须记录 `capture_failed / capture_timeout`。
- Admin API、Remote IM、message log 都不能返回 `auth_state.json` 或 raw headers。

敏感字段匹配集合：`authorization / cookie / set-cookie / x-oai-is / x-conduit-token / openai-sentinel-* / cf_clearance / __Secure-next-auth.session-token / _puid / _uasid / _umsid`。

## 与 IM Route / Schedule 集成

- IM Provider Agent 默认链路：`incoming IM -> provider agent config -> resolve effective runner -> load runner.adapter -> dispatch chatgpt_web`。用户只要在 Provider Agent 配置里选 runner `chatgpt-web`，普通消息就走 ChatGPT Web adapter。
- 显式 route action：使用通用 `runner_agent_chat`，不提供 `ChatGptWebAgentChat` 单功能 action。

~~~json
{ "type": "runner_agent_chat", "runner_id": "chatgpt-web", "operation": "ask", "reply_target": "original_chat", "delivery_mode": "final_reply" }
~~~

- Schedule 选择 runner，由 runner adapter 决定执行实现：`{ "task_type": "runner", "runner": { "runner_id": "chatgpt-web", "operation": "ask", "prompt": "..." } }`。

## 实现切分

### Phase 1：Adapter 骨架

- 新增 `chatgpt_web` adapter 常量、adapter dispatch 分支、`ExternalCliAdapterConfig` 中 `chatgpt_web` schema。
- Capabilities 结构与 WebUI 字段可见性。

### Phase 2：登录与 native probe

- `BrowserLoginBroker`：CDP 启动 Edge/Chrome、Network 事件监听、`accounts/check` header 捕获、cookie 导出、auth_state 写盘。
- `ensure_authenticated` 三信号判定、browser proof 短期兜底。
- Adapter 登录 API：`auth/status / auth/open / auth/stop`。

### Phase 3：执行与结果

- `ExecutionBrowserController`：composer 可见性检测、handoff 捕获、`conversation_id` 恢复、`current_node` 轮询。
- DOM fallback 与自然批次投递：进程/思考/最终三段、图片 section 兼容、long text 全文保留。
- Native 读路径复用 `reqwest::Client`、轮询指数退避、artifact 写盘与脱敏。

### Phase 4：接入 IM / Schedule / WebUI 与文档

- Provider Agent channel `runnerId` override 端到端联调。
- Schedule `runner` task type 联调。
- WebUI Runner 编辑页 adapter fields、登录 action。
- human_tests / e2e / 索引更新。

## 测试方案

### 单元测试

- `agent_runner_registry_accepts_chatgpt_web_adapter`
- `agent_runner_registry_rejects_unknown_adapter`
- `adapter_capabilities_hide_cli_only_fields_for_chatgpt_web`
- `chatgpt_auth_state_redacts_sensitive_headers`
- `chatgpt_auth_probe_classifies_accounts_check_logged_in`
- `chatgpt_auth_probe_requires_captured_runtime_headers`
- `chatgpt_auth_probe_classifies_anonymous_as_auth_required`
- `chatgpt_auth_probe_classifies_expired`
- `chatgpt_auth_probe_rejects_stale_browser_proof_on_native_403`
- `chatgpt_conversation_readable_does_not_imply_logged_in`
- `chatgpt_conversation_list_parses_items`
- `chatgpt_conversation_current_node_extracts_final_text`
- `chatgpt_wait_times_out_without_finished_assistant`
- `chatgpt_run_artifacts_do_not_contain_cookie_or_authorization`

### E2E 测试

Mock ChatGPT Web server 覆盖 `accounts/check`（logged in / guest / challenge）、`/backend-api/me`、`/backend-anon/me`、`/backend-api/conversations`、`POST /backend-api/f/conversation`、`GET /backend-api/conversation/:id`。

新增 E2E：

- `im_gateway_chatgpt_web_adapter_list`
- `im_gateway_chatgpt_web_adapter_ask_wait_final`
- `im_gateway_chatgpt_web_adapter_auth_required_opens_login`
- `im_gateway_chatgpt_web_adapter_redacts_artifacts`
- `agent_runner_chatgpt_web_adapter_channel_override`
- `chatgpt_web_handoff_submission_evidence`
- `chatgpt_web_browser_defaults_to_headed_mode`
- `chatgpt_web_handoff_heartbeat`
- `chatgpt_web_target_page_state`
- `im_gateway_mock_inbound_chatgpt_web_queue`
- `im_gateway_external_cli_session_records`
- `generated_image_tool_result_is_final_and_counts_all_images`
- `dom_content_accepts_short_text_replies`
- `dom_output_state_waits_for_generation_controls_to_finish`
- `required_dom_stable_for_keeps_completed_dom_responsive`
- `try_extract_dom_outcome`
- `page_url_matches_conversation` / `BrowserSession::find_conversation_page`
- `generated_image_assets_without_finished_path_list_are_final_when_not_streaming`
- `generated_image_assets_wait_when_any_message_is_in_progress`
- `chatgpt_web_delivery_uses_final_response_when_images_are_appended`
- `chatgpt_web_delivery_preserves_natural_process_batches`
- `chatgpt_web_dom_extraction_does_not_truncate_response_text`
- `agent_reply_collects_and_strips_generated_local_images`
- `send_image_uploads_original_bytes_to_cdn_and_sends_image_item`
- `chatgpt_web_startup_auth_runners_include_all_web_runners`
- `chatgpt_web_startup_auth_dry_run_reports_login_prompt` + `test_chatgpt_web_startup_auth_preflight.sh`

### human_tests

`human_tests/chatgpt-web-adapter.md`：

- TC-CWA-01 新建 runner，选择 adapter = `chatgpt_web`。
- TC-CWA-02 首次无登录态触发浏览器登录。
- TC-CWA-03 登录完成后同设备二次 run 不再弹窗。
- TC-CWA-04 登录失效后返回 auth_required 并重新弹窗。
- TC-CWA-05 `list` 展示真实 ChatGPT 会话列表。
- TC-CWA-06 `ask` 发起新会话并等待最终回答。
- TC-CWA-07 run detail 和日志不包含 cookie/token。
- TC-CWA-08 Provider Agent 选 `chatgpt-web` runner 后普通 IM 入站消息走 `chatgpt_web` adapter。
- TC-CWA-09 `/stop` 能停止当前 ChatGPT Web run 或等待流程。
- TC-CWA-12 性能回归：连续 ask 同一 session 走 ChatGPT Web 前端发送路径，日志含 send/wait/total 耗时。
- TC-CWA-13 可观察执行：默认 Execution Mode 为 `headed`，运行时弹出真实浏览器窗口。
- TC-CWA-14 handoff 心跳：浏览器/CDP/page probe 失败时快速失败为 `browser_unavailable`，IM active session 被释放。
- TC-CWA-15 mock IM 入站端到端：debug mock inbound 连续注入消息，验证新建 / 追加 / 排队 / 消费。
- TC-CWA-16 Session 记录：AI -> Agent -> Sessions 显示 user 输入与最终输出；History Event Timeline 记录 `session_start / user_message / tool_call / tool_result / assistant_message`。失败时 `tool_result.success=false` 且含脱敏异常。
- TC-CWA-17 生成图片原图发送：`image_gen` tool 结果优先解析 `image_asset_pointer: sediment://file_...` 走 `/backend-api/files/{fileId}/download`；缺失字段时降级 `estuary/content`。图片下载缓存后按 IM provider 独立协议发送（Weixin `image_item` 需先加密上传 CDN；Feishu `image` 需 image_key）。
- TC-CWA-18 失败现场诊断：本轮 `runs/<run_id>/` 必含 `failure_diagnostics.json`；已知 `conversation_id` 必含 `conversation_response.json`；尽力 `page_dom.{html,json}`。
- TC-CWA-25 服务启动登录态预检：Runners 含 `adapter=chatgpt_web` 时，`bifrost start` 前台与 daemon 都后台执行一次强登录态检查；缺失/失效时自动开登录浏览器。

### 覆盖率与项目校验

- `cargo test -p bifrost-admin im_gateway::chatgpt_web`
- `cargo test -p bifrost-admin handlers::im_gateway::chat_gateway`
- Chat Gateway mock E2E + IM 入站 mock E2E
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `pnpm --dir web run build`
- `rust-project-validate`

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核 runner 与 adapter 概念是否清晰：runner id 选择实例，adapter id 选择实现。
- Review auth state、artifact、Admin API 是否存在 secret 泄漏。
- 跑 adapter registry 单元测试、ChatGPT mock 单元测试和 mock E2E。

### 第 2 轮

- 复查第 1 轮修复后的 diff。
- 真实浏览器登录验证一次，复跑 `list/ask/get/wait`。
- 检查 WebUI Runner 页面、Provider runner override、human_tests 文档和 readme 索引。

第 2 轮若发现 runner/adapter 概念混淆、登录态、浏览器弹窗、runner 选择或 token 脱敏问题，必须追加第 3 轮。

## 风险与决策

- `chatgpt_web` 不是 CLI，公共概念必须命名为 Agent Runner / Adapter，不能让 CLI 专属命名进入最终 API、WebUI 或文档。
- `adapterConfig` 必须是 adapter-specific schema，并配合 capabilities 校验。
- ChatGPT Web 接口不是稳定公开 API，字段与 sentinel 机制可能变化；接口契约集中在 `ChatGptWebClient`。
- 写请求依赖浏览器环境，native HTTP replay 已验证会触发风控；写路径强制走 headless browser-context / 真实前端触发。
- 无 GUI 设备无法自动弹窗，必须提供 WebUI 登录入口和明确错误。
- headless 执行仍可能遇到 Cloudflare/challenge、composer 缺失、send button disabled；必须 fail-closed 并返回脱敏诊断。
- 同一设备多账号切换时必须检测 account fingerprint，避免旧账号登录态误用到新 run。
- WebSocket 增量输出与轮询等待属于同一个 `chatgpt_web` adapter 能力；即使增量不可用，等待最终结果必须可靠。

## 待审查问题

1. `chatgpt_web` adapter 的 `adapterConfig` schema 是否按 `browser / chatgpt / auth` 三块严格校验？
2. run artifact 是否默认只保存脱敏 conversation summary，不保存完整 raw mapping？
