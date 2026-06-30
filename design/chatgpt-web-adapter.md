# ChatGPT Web Adapter 技术方案

> 状态：实施中
> 范围：最终态技术方案与当前实现对齐；正式实现入口位于 `crates/bifrost-admin/src/im_gateway/chatgpt_web.rs`，配套子模块拆分到 `crates/bifrost-admin/src/im_gateway/chatgpt_web/{artifacts,browser,diagnostics,images,interaction,native,send,storage,tests}.rs`。
> 核心结论：ChatGPT Web 的最终形态是新增 adapter：`chatgpt_web`。runner 是可命名、可复用、可被 Provider 覆盖选择的配置实例；adapter 是 runner 背后的执行实现。
> 命名提示：本文使用 `AgentRunner*` 作为最终概念命名；实际代码当前仍沿用 `ExternalCli*` 系列类型（`ExternalCliGatewayConfig` / `ExternalCliAgentSettings` / `ExternalCliChannelSettings` / `ExternalCliRunRequest`），公共 API 路径前缀为 `/_bifrost/api/im-gateway/chat`，并未单独切出 `/_bifrost/api/agent/runners` 命名空间，重命名工作 (planned, not yet shipped as of 2026-06-16)。

## 背景

Bifrost Agent 执行配置最终拆成两层：

- Runner：实例层。`AgentRunnerRegistry` 承载 `defaultRunnerId + runners + channels`，Provider/Channel 只覆盖 `runnerId / enabled / deliveryMode`。
- Adapter：实现层。每个 runner 里有 `adapter` 和 `adapterConfig`，内置 adapter 包括 `codex / custom / mock / chatgpt_web`。
- Chat Gateway：调用层。`/_bifrost/api/im-gateway/chat`、`/chat/stream`、`/runs/:runId`、`/runs/:runId/stop` 提供 run、stream、detail、stop 的统一入口。
- Work dir：上下文层。work dir 不属于 runner 配置，而是从 IM Provider Agent work dir 继承，缺省再从 Agent 全局 work dir 继承。

所以 ChatGPT Web 的正确落点是新增内置 adapter `chatgpt_web`：

```text
runner_id = "chatgpt-web"
  enabled = true
  adapter = "chatgpt_web"
  adapterConfig = { browser, auth, chatgpt }
```

用户在 AI -> Agent -> Runners 中新增或编辑一个 runner，选择 Adapter = ChatGPT Web；Provider 只需要 override 到这个 runner id。

## 用户目标验证清单

### 必须实现

- 新增 `chatgpt_web` adapter，支持创建对话、发起对话、列出对话、获取消息、等待输出结果、展示最终结果。
- 一个设备只需要登录一次；登录态存储在运行 Bifrost 的本机数据目录，之后的 run 自动复用。
- 每次 run 开始前自动检查登录态；登录失效时给出明确反馈，并由 Bifrost 后端自动弹出 Edge/Chromium 浏览器让用户完成登录。
- 登录弹窗、cookie/header 提取、登录状态验证都属于 `chatgpt_web` adapter 能力，不能依赖 Agent 操作浏览器，也不能封装为外部脚本或 skill。
- 与 runner / adapter 抽象对齐：Chat Gateway、IM Provider、IM Route、Schedule、WebUI 都选择 runner；runner 再通过 `adapter = "chatgpt_web"` 进入 ChatGPT Web 执行实现。

### 必须不破坏

- 不改变现有 runner 选择语义：`default_runner_id`、Provider channel `runner_id` override、delivery mode 继续生效。
- 不改变现有 `codex / custom / mock` adapter、run detail、stop API 和 IM 入站主流程。
- 不把 ChatGPT Web cookie、Authorization、Cloudflare token、sentinel token、完整 headers 写入普通日志、message log、run summary 或可远程读取的接口。
- 不把登录态跨设备同步；登录态仅属于运行 Bifrost 的本机设备。
- 不把 Bifrost Traffic 抓到的历史 token 当作长期认证来源；Traffic 只用于恢复接口契约和调试。
- 不把 work dir 放回 runner 配置；ChatGPT Web adapter 不使用 repo work dir 作为认证或请求来源。

### 必须真实验证

- 用 Bifrost Traffic 确认 Edge/Chromium 中 `chatgpt.com` 的真实接口契约。
- 用本机真实浏览器 profile 完成首次登录，随后关闭浏览器再用 `chatgpt_web` adapter 执行 `list/get/ask`。
- 手动使登录态失效或使用空 profile，验证 adapter 自动弹出浏览器并给出可理解反馈。
- 验证最终结果来自 `GET /backend-api/conversation/{conversation_id}` 的 `current_node` assistant text，而不是只读取提交请求的短 SSE handoff。

## 概念模型

### Runner

Runner 是一份可命名配置，用来描述“这个 Agent 入口应该如何执行”：

- id：例如 `codex`、`chatgpt-web`、`ops-codex`。
- enabled：是否启用。
- adapter：执行实现，例如 `codex`、`custom`、`mock`、`chatgpt_web`。
- adapterConfig：adapter 私有配置。
- instructions / skillPaths / injectBifrostTools：仅对支持这些能力的 adapter 生效。
- deliveryMode：默认 IM 投递策略。

Runner 不等于某种 runtime 类型，也不应该按产品能力膨胀成 `ChatGptWebRunnerKind`。

### Adapter

Adapter 是 runner 背后的执行实现：

- `codex`：进程型 adapter，构造 `codex exec --json`，解析 JSONL。
- `custom`：进程型 adapter，按用户提供 command/args 执行。
- `mock`：测试 adapter。
- `chatgpt_web`：原生 Web adapter，负责浏览器登录态、ChatGPT Web API、等待最终回答、artifact 脱敏。

ChatGPT Web 与 Codex 的差异应被 adapter 吸收，而不是泄漏到 IM event loop、Provider 配置或 Chat Gateway API。

## 架构结论

```text
AI -> Agent -> Runners
        |
        +-- runner "codex"
        |     adapter = "codex"
        |
        +-- runner "custom-local"
        |     adapter = "custom"
        |
        +-- runner "chatgpt-web"
              adapter = "chatgpt_web"

Chat Gateway / IM Provider Agent / IM Route / Schedule
        |
        v
resolve effective runner_id
        |
        v
load runner settings
        |
        v
Adapter Registry dispatch by runner.adapter
        |
        +-- codex adapter
        +-- custom adapter
        +-- mock adapter
        +-- chatgpt_web adapter
```

最终模型只保留 “Agent Runner + Adapter Registry”。ChatGPT Web 的所有差异都收敛在 `chatgpt_web` adapter 内，不进入 runner 类型、Provider 配置或 route action 枚举。

## 结构化输出与投递

ChatGPT Web 页面可能把一次回答渲染成多条 assistant message：前面的短消息通常是过程性说明，最后一条才是最终答案。adapter 以 `data-message-author-role="assistant"` + `data-message-id` 作为自然批次边界，按页面顺序整理输出：

- CLI / Chat Gateway stream：输出 NDJSON 事件。过程批次为 `assistant_delta`，`raw.phase = "thinking"`；可识别的页面工具调用为 `tool_finished`，`raw.phase = "tool"`；最后答案为 `assistant_final`，`raw.phase = "final"`。
- IM 通道：只投递过程批次和最终答案，不投递工具调用事件，避免 IM 中出现调试噪音。
- 图片结果：ChatGPT 生成图会先缓存到本机附件目录。IM 文本卡片会移除本地图片 Markdown，随后按图片出现顺序逐张发送图片消息；CLI JSON 保留最终文本中的本地图片 Markdown，便于调用方读取附件路径。
- DOM fallback 不能只凭文本判断最终态。文本只作为候选内容；真正允许重新提取最新消息并返回前，必须确认页面控制态已经恢复：stop/cancel 按钮消失、composer 可见且可用，且当 composer 内仍有待发送文本时提交按钮必须可见。空 composer 下 ChatGPT 会把发送位切回语音/提交控制，不能要求 send button 仍显示或处于 disabled 状态，否则会把已完成回答误判为仍在输出。“正在创建图片 / 正在生成图片 / 正在打草稿 / Drafting / Thinking”等状态文案只是额外保护，不能取代按钮状态判定。若用户 prompt 中明确要求 `N` 张图片，下载阶段以 `N` 作为最低期望数量；首次 DOM 只看到较少图片时继续滚动/监听网络补齐，避免页面懒加载导致少发最后几张。
- DOM fallback 的候选稳定性必须基于内容签名，而不是只看文本长度。签名至少覆盖 turn/message id、最终文本、自然批次数、图片数和附件数；同长度文本变化或新增批次都必须重置稳定窗口。对短的过程性 prelude（例如“我会先筛选/搜索/整理...”）必须采用更长的稳定窗口，避免 ChatGPT Web 在真正搜索、浏览或最终答案节点出现前，把第一段计划说明误判为最终回复。若发送阶段已经发生 `stream_handoff` 但没有捕获到可解析的 SSE final，DOM fallback 还必须增加结构性最小等待窗口；即使 composer 已恢复、发送位变成语音按钮，也不能只按控件空闲就返回。`data-testid="stop-button"` 以及中英文 Stop/Cancel aria-label 表示仍在处理，`Start dictation` / `Start Voice` / `开始听写` / `启动语音功能` 语音按钮配合 composer 空闲表示可收尾；发送按钮 selector 不得使用 `composer-submit-btn` 这类 stop button 也会携带的非语义 class。微信/飞书等 IM 通道绑定 `chatgpt_web` runner 时，最终回写必须来自真正完成后的答案文本，而不是这类短 planning 段。
- ChatGPT Web 的生成图结果有时不会出现在 `data-message-author-role="assistant"` 节点中，而是渲染成后续 `section[data-testid^="conversation-turn-"]`，正文只有 `ChatGPT 说：`，图片在 section 内的 `estuary/content` URL。DOM fallback 必须把最后一个 user turn 之后的这类 image-only section 当作 assistant 结果处理，否则 CLI/IM 会一直等待，或漏发最后一张图片；但如果 section 还只是空壳 `ChatGPT 说：` 或 `最后微调一下...`，且图片数为 0，必须继续等待。
- DOM 提取和 `allMarkdownTexts` 自然批次必须保存完整文本，不允许使用固定字符数截断；`textLength` 只用于诊断，`response`、`last_message.md` 和 `result.json` 必须能保留长任务最终输出全文。

## 配置模型

最终配置只保留 Agent Runner Registry。最终目标命名为 `AgentRunnerRegistry / AgentRunnerSettings / AgentRunnerChannelSettings`，但当前已实现的真实类型仍位于 `crates/bifrost-admin/src/im_gateway/external_cli/mod.rs`，名为 `ExternalCliGatewayConfig / ExternalCliAgentSettings / ExternalCliChannelSettings`，`adapter_config` 字段类型为强类型 `ExternalCliAdapterConfig` 而非 `serde_json::Value`（重命名为 `AgentRunner*` 与 `adapter_config: serde_json::Value` adapter-specific schema 拆分 planned, not yet shipped as of 2026-06-16）：

```rust
// 目标命名（planned, not yet shipped as of 2026-06-16）
pub struct AgentRunnerSettings {
    pub enabled: bool,
    pub adapter: String,
    pub instructions: Option<String>,
    pub adapter_config: serde_json::Value,
    pub inject_bifrost_tools: bool,
    pub skill_paths: Vec<String>,
    pub delivery_mode: DeliveryMode,
}

pub struct AgentRunnerRegistry {
    pub version: u32,
    pub default_runner_id: String,
    pub runners: BTreeMap<String, AgentRunnerSettings>,
    pub channels: BTreeMap<String, AgentRunnerChannelSettings>,
}

pub struct AgentRunnerChannelSettings {
    pub enabled: Option<bool>,
    pub runner_id: Option<String>,
    pub delivery_mode: Option<DeliveryMode>,
}

// 实际已落地类型（shipped）
pub struct ExternalCliGatewayConfig {
    pub version: u32,
    pub default_runner_id: String,
    pub runners: BTreeMap<String, ExternalCliAgentSettings>,
    pub channels: BTreeMap<String, ExternalCliChannelSettings>,
}
```

配置约束：

- `adapterConfig` 是 adapter-specific schema，由 `adapter` 决定解析方式。
- 后端必须按 adapter capabilities 校验 `adapterConfig`，拒绝未知 adapter 和不适用字段。
- WebUI 根据 adapter capabilities 展示字段，不再假设所有 adapter 都有 executable、args、skillPaths、injectBifrostTools。
- ChatGPT Web Runner 的浏览器用户数据只能来自配置中的共享 profile：默认路径为 `BIFROST_DATA_DIR/agent/im_gateway/chatgpt_web/browser_profile`。无论是创建新对话还是续接已有 conversation，发送和等待阶段都必须复用这一个 `profileDir`，以便登录态、CDP tab 池和浏览器进程生命周期统一管理。
- `BIFROST_DATA_DIR/im_gateway/runs/<run_id>/` 只允许保存 run artifact（如 `prompt.md`、`conversation_handoff.json`、`conversation_final.json`、诊断文件和下载产物），禁止在 run 目录下创建 `chatgpt_web_fresh_profile` 或其他完整 Chromium 用户数据目录。run-local profile 会绕开共享 profile 的进程池和清理入口，导致孤儿 Edge/Chrome 进程、数百 MB 缓存和长期性能损耗。

### ChatGPT Web Runner 配置示例

```json
{
  "version": 1,
  "defaultRunnerId": "codex",
  "runners": {
    "codex": {
      "enabled": true,
      "adapter": "codex",
      "adapterConfig": {},
      "injectBifrostTools": true,
      "skillPaths": [],
      "deliveryMode": "final_reply"
    },
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
      "skillPaths": [],
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
```

这里 `chatgpt-web` 是 runner id，`chatgpt_web` 是 adapter id。文档、API、UI 必须清楚区分这两个概念。

## Adapter Registry

最终 adapter trait 是通用 Agent Runner Adapter。通用 `AgentRunnerAdapter` trait 与 `AdapterCapabilities` 结构是目标抽象 (planned, not yet shipped as of 2026-06-16)；当前已落地的实现路径是：`chatgpt_web` 模块直接复用 `ExternalCliRunRequest` / `ExternalCliRunStatus` / `ExternalCliProgressEvent*` 完成入参、状态与事件，在 `chatgpt_web.rs` 中以 `ADAPTER_ID = "chatgpt_web"` 常量与显式 dispatch 方式区分；其他 adapter (`codex / custom / mock`) 同样走 `external_cli` 进程托管路径。

```rust
// 目标 trait（planned, not yet shipped as of 2026-06-16）
#[async_trait]
trait AgentRunnerAdapter {
    fn adapter_id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn capabilities(&self) -> AdapterCapabilities;

    async fn run_turn(
        &self,
        input: AgentRunInput,
        settings: AgentRunnerSettings,
        sink: AgentProgressSink,
    ) -> Result<AgentRunResult>;

    async fn stop(&self, run_id: &str) -> Result<()>;
}
```

Process-backed adapter 复用进程托管 helper：

```rust
trait ProcessBackedAdapter {
    fn build_command(&self, snapshot: &AgentRuntimeSnapshot) -> Result<CommandSpec>;
    fn build_prompt(&self, snapshot: &AgentRuntimeSnapshot, input: &AgentRunInput) -> Result<String>;
    fn parse_stdout_line(&self, line: &str) -> CliEventParseResult;
    fn final_response(&self, run_dir: &Path, parsed: &ParsedCliRun) -> Result<String>;
}
```

`codex / custom / mock` 走进程托管路径。`chatgpt_web` 不实现 `build_command()`，而是直接实现 `AgentRunnerAdapter::run_turn()`：

```text
ChatGptWebAdapter::run_turn
  -> ensure_authenticated
  -> dispatch operation
  -> write artifacts
  -> emit canonical progress events
  -> return final result
```

## Adapter Capabilities

每个 adapter 需要声明能力，驱动 WebUI 和请求校验：

```json
{
  "adapterId": "chatgpt_web",
  "displayName": "ChatGPT Web",
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
```

WebUI 选择 `chatgpt_web` 后：

- 隐藏 executable / args / env / sandbox / approval policy / skill paths。
- 展示 Browser、Auth、ChatGPT、Check Login、Open Login Browser、Logout、Test Ask。
- 明确显示该 runner 不使用 work dir。

## Work Dir 边界

Work dir 是 Agent/Provider 的运行上下文，不是 runner 属性，也不是 adapter 认证来源：

- `codex/custom` adapter：执行请求时从 IM Provider Agent work dir 继承，没有 provider override 时使用 Agent 全局 work dir。
- `chatgpt_web` adapter：不需要 repo work dir 调 ChatGPT Web；run 归属、审计和 session key 仍可继承同一 provider/global 上下文。
- WebUI 中 runner 编辑页不展示 work dir；work dir 只在 Agent 全局配置和 IM Provider Agent 配置中出现。

## Chat Gateway Request Contract

Chat Gateway 始终先解析 runner，再由 runner.adapter 分发。请求包含 `operation` 和 `params`，用于支持 ChatGPT Web 的 list/get/wait/create 等非普通问答操作：

```json
{
  "runnerId": "chatgpt-web",
  "operation": "ask",
  "message": "你好，你会什么",
  "providerId": "feishu-sre",
  "sessionKey": "feishu-sre:user:123",
  "deliveryMode": "final_reply",
  "params": {
    "conversationId": null,
    "model": "auto",
        "timeoutSecs": 7200
  }
}
```

解析规则：

- 如果传了 `runnerId`，以指定 runner 配置为准。
- 如果未传 `runnerId`，使用 provider/channel effective runner，再退回 global default runner。
- 请求不直接传 `adapter`；adapter 只能来自 runner 配置。

支持操作：

- `ask`：组合 `ensure_authenticated -> send -> wait -> get -> final_response`。
- `list`：列出最近会话。
- `create`：创建或准备新会话上下文，返回可用于后续 `send/wait/get` 的会话状态。
- `send`：向新会话或已有会话发送用户消息。
- `wait`：等待指定 conversation 产出最终 assistant 消息。
- `get`：获取指定 conversation 的脱敏消息摘要。

返回结构需要带 runner 和 adapter：

```json
{
  "runId": "runner-run-id",
  "status": "succeeded",
  "response": "最终回答",
  "runner": {
    "id": "chatgpt-web",
    "adapter": "chatgpt_web"
  },
  "artifacts": {
    "conversationId": "abc",
    "finalNodeId": "node-id"
  }
}
```

## 登录态与浏览器弹窗

### 可行性探针结果

已通过 Bifrost Traffic 和一次性技术探针验证登录态获取与验证路径。探针脚本只用于可行性验证，不属于正式运行时代码，正式实现已收敛到 `chatgpt_web` adapter 内。

真实 Edge 流量定位结果：

- ChatGPT Web 登录成功后，页面真实请求会调用 `GET /backend-api/accounts/check/v4-2023-04-27?timezone_offset_min=<offset>`，并携带应用运行时生成的认证 header，例如 `Authorization`、`chatgpt-account-id`、`oai-device-id`、`oai-session-id`、`x-oai-is`。
- `accounts/check` 是当前登录成功的主判定接口：返回 JSON object，包含 `accounts / account_ordering`；可用账号满足 `can_access_with_session=true`、`account.account_user_id` 为用户形态、`account.account_id` 存在、`account.is_deactivated=false`。
- `Authorization` Bearer JWT 是登录身份的辅助强信号。执行器只做本地脱敏解析，不做远端验证、不打印原文；判定要求 profile email 存在且 verified、`user_id/chatgpt_user_id/account_user_id` 至少一个为 `user-` 形态、`chatgpt_account_id` 存在。
- `GET /backend-api/me` 只能作为辅助身份/匿名对照。实测登录后，脚本直接 `fetch('/backend-api/me')` 仍可能返回 anonymous 形态，因此不能把它作为唯一强登录判定。
- 匿名态也有身份接口：`GET /backend-anon/me`。它与未带应用认证 header 的 `/backend-api/me` 很接近，典型脱敏 shape 为 `idKind=anonymous`、`hasEmail=false`、`emailEmpty=true`。
- `GET /backend-api/conversations` 不能作为登录态判定依据，因为匿名 profile 下该接口也可能返回 `200` 和 `items / total / limit / offset` 结构。

技术探针验证结果：

- 使用 CDP 启动 Bifrost 管理的独立 Edge profile；正式实现从 Runner 配置读取 profile dir，默认落在 Bifrost 数据目录下。
- CDP `Network.requestWillBeSent` / `Network.requestWillBeSentExtraInfo` 监听页面真实 `accounts/check` 请求，捕获可复用认证 header；真实值只进入 `auth-state.json`，summary 仅记录 header 是否存在和值长度。
- 捕获到 `Authorization` 后，脚本脱敏解析 JWT payload 并输出 `identityComplete/profileEmail/userIdentity/accountIdentity` 四个布尔信号，不输出 email、user id、account id 或 token。
- direct browser fetch 仍返回 guest/anonymous：`/backend-api/me` 为 `idKind=anonymous`，direct `accounts/check` 为 `planType=guest`，因此不能用裸 fetch 证明登录成功。
- 捕获页面真实请求头后，native HTTP 使用导出的 Cookie header、浏览器 user-agent 和捕获的认证 header 调用 `accounts/check`，返回 `nativeLoggedIn=true`。
- 验证输出为 `RESULT effectiveLoggedIn=true browserLoggedIn=false nativeLoggedIn=true identityComplete=true profileEmail=true userIdentity=true accountIdentity=true conversationReadable=true cookies=33`。
- 同一匿名 profile 下 `conversationsProbe.readable=true`，证明会话读接口可用不等于登录成功。
- 完整任务验证已跑通：native HTTP 可列出会话；native HTTP 直接写 `POST /backend-api/f/conversation` 会被服务以 unusual activity 拒绝；正式写路径必须通过 CDP 操作受控 Edge 页面，打开新会话或 `/c/{conversation_id}` 后在可见 composer 中输入并点击发送，让 ChatGPT Web 前端自行生成 sentinel/turn headers。
- 三轮消息验证使用 `你好`、`你是谁`、`你可以做什么`，同一 conversation id 为 `6a0465ab-59ec-83ec-ae53-a4cc614c2883`，每轮均从 `GET /backend-api/conversation/{conversation_id}` 轮询到 `finished_successfully` assistant 结果。
- headless 执行验证已跑通：登录仍由可见浏览器完成；执行阶段使用同一 profile 的 headless Edge，不抢桌面焦点，成功创建新 conversation `6a0469a0-8278-83ec-8dd7-822a77e96cfa` 并连续写入 `你好`、`你是谁`、`你可以做什么` 三条消息，三轮结果均为 `finished_successfully`。
- headless 输入细节：ChatGPT 页面同时存在隐藏 textarea 和可见 `#prompt-textarea`，执行器必须只选择可见 composer。若误选隐藏 textarea，send button 会保持 disabled，必须输出 `browser_ui` 诊断而不是静默重试。
- 探针清理验证：每次启动浏览器时记录本次 CDP target id，结束时默认调用 `/json/close/<targetId>` 关闭 tab，再清理浏览器进程；否则同一 profile 会持续累积 ChatGPT tabs，打开次数越多越卡。只有显式 `keepBrowserOpenAfterLogin=true` / `--keep-open` 时才保留窗口。
- 通过 CDP `Network.getAllCookies` 导出 `chatgpt.com` 相关 cookies；真实 cookie 只写入 Runner 配置指向的 `auth_state.json`，文件权限为 `0600`；终端与 summary 只输出 cookie 名称、domain、valueLength 等脱敏信息。

结论：

- BrowserLoginBroker 使用“受控浏览器 profile + CDP + browser-context fetch”获取登录态是可行的。
- 登录态有效性不能只看 cookie 是否存在，也不能只看 `/backend-api/conversations` 是否可读，更不能只看裸 fetch 的 `/backend-api/me`。
- 强登录判定必须以现场 profile 的真实应用请求为准：捕获 `accounts/check` 的运行时认证 header，脱敏解析 JWT identity claims，再用这些 header + cookie 复验 `accounts/check`。只有 JWT 身份完整且 `accounts/check` 命中可用账号才是 `LoggedIn`。
- `GET /backend-anon/me` 是匿名态对照接口，用于区分默认匿名可对话状态。
- 读类接口的 native HTTP probe 在当前环境下可行，足以作为登录态快速验证路径；但 native probe 的目标必须是 `accounts/check`，不是 conversations，也不是裸 `/backend-api/me`。
- cookie + 捕获 header 可支撑 `list/get/wait` 这类 native 读路径；写路径必须使用浏览器 UI 触发，因为写请求依赖浏览器环境、sentinel、turnstile、turn trace 和 challenge 状态，native 复刻会触发风控。追加消息不能先 direct fetch 再 fallback，必须先进入指定 conversation 页面再操作 composer。

### 登录态状态机

```text
NotConfigured
  -> LoggedOut
  -> Checking
  -> LoggedIn
  -> Expired
  -> AuthRequired
  -> BrowserOpened
  -> LoggedIn
  -> LoginStopped
  -> LoginWindowClosed
```

每次 `run_turn()` 先执行 `ensure_authenticated()`：

1. 读取 `auth_state.json`。
2. 使用保存的 cookie、user-agent 和捕获的认证 header 对 `GET /backend-api/accounts/check/v4-2023-04-27?timezone_offset_min=<offset>` 做 native HTTP 轻量探测。
3. 同时满足以下三组信号时，状态为 `LoggedIn`：
   - browser traffic identity：真实 `accounts/check` 请求存在 `Authorization` Bearer token。
   - JWT identity：脱敏 claims 中 profile email 存在且 verified，`user_id/chatgpt_user_id/account_user_id` 至少一个是 `user-` 形态，`chatgpt_account_id` 存在，且 `exp` 未过期。
   - account check：返回 200，且至少一个账号满足 `can_access_with_session=true`、`account.account_user_id` 为用户形态、`account.account_id` 存在、`account.is_deactivated=false`。
4. 返回 guest account、anonymous `/backend-api/me`、JWT 缺少 email/user/account、JWT 已过期、401/403、Cloudflare challenge、HTML 登录页、sentinel/challenge 错误、账号 fingerprint 不匹配时，进入 `AuthRequired`。
5. 如果 runner 的 `adapterConfig.browser.openOnAuthRequired=true`，由 Bifrost 后端调用 `BrowserLoginBroker::open_login()` 弹出浏览器。
6. `BrowserLoginBroker` 使用受控浏览器 profile 通过 CDP 打开 `https://chatgpt.com/`，监听页面真实 `accounts/check` 请求并捕获可复用认证 header。
7. 捕获认证 header 后，使用 CDP 导出 `chatgpt.com` cookies，连同必要 header 写回 `auth_state.json`，权限必须是 `0600`。
8. 写回后立即再执行一次 native HTTP probe。native probe 通过则继续当前 run；如果 native probe 因本机网络、代理或 TLS 传输问题无法发出请求，但同一次浏览器真实 `accounts/check` 响应已经证明可用账号存在，则保留登录态并把状态标记为 `LoggedIn`，同时在状态 message 中保留 native probe 失败原因。该 browser proof 只作为刚捕获后的短期兜底，不能在后续 run 中替代当前 native `accounts/check`；否则旧 proof 会把已跳到登录页的 profile 误判为已登录。
9. 登录等待没有固定超时：只有用户完成登录、用户关闭登录浏览器窗口，或 WebUI 主动调用 stop login 时才结束。关闭窗口或主动停止都返回 `auth_required`，错误文本需要明确区分 `login window was closed` 和 `login was stopped by request`。

### BrowserLoginBroker

`BrowserLoginBroker` 是 Bifrost 后端能力，不是 Agent browser automation：

```text
BrowserLoginBroker
  - locate Edge / Chrome executable
  - allocate remote-debugging-port
  - launch with persistent profile dir
  - open https://chatgpt.com/
  - listen for real /backend-api/accounts/check request headers through CDP Network events
  - use /backend-api/me and /backend-anon/me only as diagnostic anonymous checks
  - export chatgpt.com cookies / selected storage state / captured auth headers
  - capture real accounts/check response body as browser proof
  - verify exported cookies and captured auth headers through native HTTP accounts/check probe when local transport allows it
  - keep waiting until login succeeds, the user closes the browser, or WebUI sends stop login
  - close the opened CDP target unless keep-browser-open is explicitly enabled
  - redact and persist auth_state
```

用户只看到登录窗口并完成登录。Agent 不参与点击、输入或流程控制。

登录成功后浏览器是否保持打开由 `adapterConfig.browser.keepBrowserOpenAfterLogin` 控制。

### ExecutionBrowserController

执行阶段由 `ExecutionBrowserController` 负责。当前真实 IM 稳定性验证期默认使用 headed Edge/Chromium，便于用户和 Agent 共同观察真实页面输入、清空 composer 和发送动作；稳定后可再把 Runner Execution Mode 切回 `headless`，避免正常 IM 执行抢占桌面焦点：

```text
ExecutionBrowserController
  - launch Edge/Chromium with the runner profile in headed mode by default during live IM validation
  - open https://chatgpt.com/
  - wait for visible composer, not the hidden textarea
  - create a new conversation by sending the first message without conversation_id
  - append to an existing conversation by opening /c/{conversation_id}, waiting for the visible composer, then inserting text and clicking the page send button
- capture /backend-api/f/conversation SSE handoff for conversation_id and turn_exchange_id when available
- keep the handoff wait short and recover from interrupted handoff streams by reading the active `/c/{conversation_id}` URL or the already known conversation id
- during handoff waits, run a heartbeat: fail fast if the browser process exits, the CDP WebSocket closes, or the page cannot answer a short `Runtime.evaluate` probe
- poll /backend-api/conversation/{conversation_id} until assistant message status is finished_successfully; each native read uses a bounded request timeout, treats temporary read failures as heartbeat-visible transient state, and fails after a continuous unreadable grace window instead of holding the IM queue indefinitely
- if native read/auth endpoints return transport-level 403/429/5xx while the browser profile has a freshly captured accounts/check proof from the current login flow, fall back to browser-context evidence before declaring auth failure; stale browser proof or expired Authorization must produce `auth_required`
- 性能优化：`chatgpt_web` native HTTP 读路径复用单例 `reqwest::Client`，避免每次 list/get/wait 重建 DNS/TLS/连接池；handoff 阶段在 `Network.dataReceived` 中尽早读取 SSE chunk，拿到 `conversation_id` 后进入最多 3 秒的 quick-complete 窗口，若短回答已经完成则直接解析完整 SSE，否则立即返回 conversation id 交给最终轮询。
- 交互逻辑拆分：会话映射、conversation 摘要、最终消息轮询、stop marker 检查等与页面发送解耦到 `chatgpt_web/interaction.rs`，主模块保留 adapter 编排职责，确保单文件继续低于 1500 行。
- 轮询策略：`wait_final` 首次可立即读取 conversation detail，后续使用 300ms 起步的指数退避，最大值不低于 `pollIntervalMs.max(2000)`；短回答降低首包后等待，长任务减少无效请求。
- UI 发送路径缩短固定等待：composer 输入后的固定等待从 500ms 降到 150ms，composer ready 后 React hydration settling 从 1500ms 降到 400ms；send button readiness 仍保留显式检查，避免只靠 sleep 判断。
- handoff 恢复判定：当短 SSE 中断但已捕获 `POST /backend-api/f/conversation` 时，`eventTypes` 额外记录 `browser_post_captured`，提交确认阶段不再无谓等待用户消息再次可见；如果没有 POST 或 SSE 证据，仍保持原有 `wait_user_message_visible` 防重入保护。
  - close the CDP target and browser process after the run
```

如果页面出现 composer 缺失、send button disabled、Cloudflare/challenge 页面、401/403、native 读路径失效，或 heartbeat 判定浏览器/CDP/page 不再可用，run 必须返回结构化错误并释放 IM active session，避免后续消息一直停留在“排队”。短 SSE handoff 断开不能直接判定 run 失败；adapter 必须先尝试从页面 URL、session 映射或显式 conversationId 恢复，再进入长轮询等待：

```json
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
```

错误诊断只能包含 URL、title、状态、元素计数、按钮状态、HTTP status、request id 等脱敏信息，不能包含 cookie、Authorization、`x-oai-is`、prompt 全文以外的页面私密内容。

### 登录失效反馈

HTTP Chat Gateway 响应：

```json
{
  "status": "auth_required",
  "response": "ChatGPT Web 登录已失效，已在本机打开 Edge 登录窗口。请完成登录后重试或等待当前 run 自动继续。",
  "runner": {
    "id": "chatgpt-web",
    "adapter": "chatgpt_web"
  },
  "auth": {
    "state": "browser_opened",
    "loginUrl": "https://chatgpt.com/",
    "deadlineMs": 1778669999000
  }
}
```

IM 触发时，先回复原通道：

```text
ChatGPT Web 登录已失效。我已在运行 Bifrost 的这台设备上打开 Edge 登录窗口，请完成登录后重新发送消息。
```

如果设备无 GUI、找不到浏览器或处于 remote/headless 环境，返回：

```text
auth_required: 当前设备无法自动打开浏览器，请在 Bifrost WebUI 的 AI -> Agent -> Runners 中打开 chatgpt_web adapter 的登录入口。
```

## ChatGPT Web 请求策略

真实流量恢复时使用 Bifrost Traffic 查询 Edge/Chromium 对 `https://chatgpt.com` 的请求。目标主链路：

```text
POST /backend-api/f/conversation
  -> text/event-stream handoff
  -> conversation_id + turn_exchange_id

GET /backend-api/conversations
  -> 会话列表

GET /backend-api/conversation/{conversation_id}
  -> mapping + current_node + 最终 assistant message

GET /backend-api/me
  -> 辅助身份/匿名对照，不能单独作为登录强判定

GET /backend-anon/me
  -> 匿名态身份对照

GET /backend-api/accounts/check/v4-2023-04-27
  -> 登录账号强校验，必须复用页面真实请求的认证 header
```

### 读写分层

读类接口优先 native HTTP：

- `GET /backend-api/conversations`
- `GET /backend-api/conversation/{conversation_id}`
- `GET /backend-api/conversation/{conversation_id}/stream_status`

native HTTP 读路径必须复用 `ensure_authenticated()` 保存的 Cookie header、user-agent 和页面真实请求中捕获的认证 header，并在每轮 run 开始前通过 `accounts/check` 验证登录账号。`conversations` 只验证会话列表读取能力，不能替代身份校验。若 native probe 失败但浏览器真实请求仍能通过 `accounts/check`，读类接口可以降级到 browser-context fetch；此时 run artifact 要记录降级原因，但不能记录 cookie/header value。

写类接口优先 headless browser-context 真实前端触发：

- `POST /backend-api/f/conversation`
- `POST /backend-api/f/conversation/prepare`
- `POST /backend-api/conversation/init`

原因：写请求包含 sentinel、conduit、Cloudflare、browser environment、turn trace 等动态 token。长期复刻抓包 header 风险高，且实验中 native POST 已触发 unusual activity。最终方案必须让 ChatGPT Web 前端在 headless browser context 中生成并发送写请求。

如果 headless browser context 不可用，不能自动降级为 native 写请求；必须将失败归类为 `AuthRequired`、`ChallengeRequired`、`BrowserUiBlocked` 或 `UnsupportedWebContract`，并把脱敏诊断返回给调用方。

### 操作行为

`list`：

```text
GET /backend-api/conversations?offset=<offset>&limit=<limit>&order=updated&is_archived=false&is_starred=false
```

返回会话 id、title、create_time、update_time、is_archived、is_temporary_chat。

`create`：

- 创建新会话上下文并返回本地 root message id。
- 如果 ChatGPT Web 不支持空会话持久化，则返回待发送上下文；真实 `conversation_id` 由首次 `send` 后的 handoff 决定。

`send`：

- 使用 headless browser context 操作可见 composer，由 ChatGPT Web 前端触发 `POST /backend-api/f/conversation`。
- payload 至少包含 user message、parent message id、model、timezone、conversation mode、buffering/supported encodings。
- 返回 `conversation_id`、`turn_exchange_id`、resume/subscribe topic。

`wait`：

- 优先轮询 `GET /backend-api/conversation/{conversation_id}` 等待最终结果，WebSocket token 增量作为同一 adapter 内的能力扩展。
- 停止条件：`current_node` 对应 message 是 assistant text、状态 finished、content parts 非空。
- 每 `pollIntervalMs` 轮询一次，最长 `timeoutSecs`；默认 7200 秒，允许用户配置更长时间，以覆盖两小时以上的长任务。`POST /backend-api/f/conversation` handoff 只用于快速拿 conversation id，等待窗口独立且较短，不能复用长任务超时。
- 长任务等待必须持续检查 run 目录下的 `stop_requested` marker；用户调用 `/runs/:runId/stop` 后，adapter 在提交确认等待或最终结果轮询中尽快收敛为 `stopped`，不能等到 Run Timeout Seconds 自然耗尽。
- 超时返回 `timed_out`，artifact 保留最近一次脱敏响应摘要。

`get`：

- 返回完整消息树的脱敏摘要：user text、assistant text、message id、parent/children、status、create/update time。
- 不默认返回 hidden system、thoughts、reasoning recap、内部 metadata 原文。

`ask`：

```text
ensure_authenticated
send
wait
get
final_response
```

## Run Artifact 与安全脱敏

每次 run 写入：

```text
runs/<run_id>/
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
```

安全规则：

- `auth_state.json` 权限必须为 `0600`。
- `auth_state.json` 可以保存真实 cookie value，但只能存放在本机 Bifrost 数据目录，不进入 run artifact、日志、Remote IM 或 WebUI 普通详情。
- `request.json` 不写入 cookie、Authorization、sentinel token、x-oai-is、x-conduit-token。
- `runtime_snapshot.json` 记录 runner id、adapter id、adapter capabilities、header keys，不记录 header values。
- `conversation_final.json` 默认只保存消息摘要和最终 assistant text，不保存完整 raw mapping；debug 模式也必须先脱敏。
- 任意失败路径必须尽力写入 `failure_diagnostics.json`。如果本轮已知 `conversation_id`，同时保存 ChatGPT conversation API 的原始响应到 `conversation_response.json`；无论是否已知 `conversation_id`，都尽力用同一登录态打开目标页或首页，保存 `page_dom.html` 和 `page_dom.json`，用于下一轮复盘页面是否在新建会话、目标 `/c/{conversationId}` 或异常状态。
- `page_dom.json` 只保存 URL、标题、readyState、composer/landmark 摘要和截断 body text；完整 DOM 结构写入 `page_dom.html`，避免把超大 HTML 塞进 run summary。失败诊断抓取本身失败时，也必须在 `failure_diagnostics.json` 中记录 `capture_failed` / `capture_timeout`，不能静默忽略。
- Admin API、Remote IM、message log 都不能返回 `auth_state.json` 或 raw headers。

敏感字段匹配：

```text
authorization
cookie
set-cookie
x-oai-is
x-conduit-token
openai-sentinel-*
cf_clearance
__Secure-next-auth.session-token
_puid
_uasid
_umsid
```

## API 设计

### Runner Registry API

目标 API 围绕 Agent Runner 和 Adapter；当前已落地的实际路径仍统一挂在 `/_bifrost/api/im-gateway/chat` 前缀下，未单独切出 `/_bifrost/api/agent/runners` 命名空间 (`/_bifrost/api/agent/runners*` 路由 planned, not yet shipped as of 2026-06-16)：

```text
# 目标 API（planned, not yet shipped as of 2026-06-16）
GET   /_bifrost/api/agent/runners
PATCH /_bifrost/api/agent/runners
GET   /_bifrost/api/agent/adapters
GET   /_bifrost/api/agent/runners/:runner_id
PATCH /_bifrost/api/agent/runners/:runner_id
PATCH /_bifrost/api/agent/runners/channels/:provider_id

# 已实现的实际路由（shipped）
GET   /_bifrost/api/im-gateway/chat/config              # 全量 ExternalCliGatewayConfig
PATCH /_bifrost/api/im-gateway/chat/config              # 全量替换 ExternalCliGatewayConfig
POST  /_bifrost/api/im-gateway/chat/stream              # 单次 run，SSE 流
POST  /_bifrost/api/im-gateway/chat/runner-calls/stream # caller -> runner 嵌套调用
GET   /_bifrost/api/im-gateway/chat/runs/:run_id        # run 详情
POST  /_bifrost/api/im-gateway/chat/runs/:run_id/stop   # stop run
```

Chat Gateway 只负责执行 runner，不负责配置 runner schema。Runner 和 Adapter 配置最终归属于 Agent 域，当前仍由同一 chat gateway `/config` 端点承载。

### Adapter Status API

`chatgpt_web` 需要登录相关动作。建议按 runner 暴露，因为状态来自 runner 的 adapterConfig/profile；当前已落地的真实路径仍按 adapter 而不是 runner 暴露 (`/_bifrost/api/agent/runners/:runner_id/adapter-*` 命名 planned, not yet shipped as of 2026-06-16)：

```text
# 目标 API（planned, not yet shipped as of 2026-06-16）
GET  /_bifrost/api/agent/runners/:runner_id/adapter-status
POST /_bifrost/api/agent/runners/:runner_id/adapter-actions/check-login
POST /_bifrost/api/agent/runners/:runner_id/adapter-actions/open-login
POST /_bifrost/api/agent/runners/:runner_id/adapter-actions/logout

# 已实现的实际路由（shipped），通过 ?runnerId= 或 body 中 runnerId 选择具体 runner
GET  /_bifrost/api/im-gateway/chat/adapters/chatgpt-web/auth/status
POST /_bifrost/api/im-gateway/chat/adapters/chatgpt-web/auth/open
POST /_bifrost/api/im-gateway/chat/adapters/chatgpt-web/auth/stop
```

## WebUI 设计

入口保持：

```text
AI
  Agent
    Runners
```

Runner 列表展示：

- Runner ID
- Adapter：`codex / custom / mock / chatgpt_web`
- Enabled
- Delivery mode
- Last run status

Runner 编辑弹窗：

- `Adapter` select 增加 `ChatGPT Web`，值为 `chatgpt_web`。
- 当 adapter = `chatgpt_web`：
  - 隐藏 executable / args / env / sandbox / approval policy / skill paths / inject Bifrost tools。
  - 展示 Browser、Auth、ChatGPT 配置。
  - 展示 Check Login、Open Login Browser、Logout、Test Ask。
  - Auth state：Logged in / Expired / Browser opened / Login timeout。
  - Account：masked display name/email，如果能从安全接口解析。
  - Browser profile path 折叠展示。

Provider 编辑页只展示：

- inherit global runner
- override runner id
- delivery mode override
- effective runner preview：包含 runner id 和 adapter id。

亮色/暗色主题都必须验证。

## 与 IM Route / Schedule 集成

### IM Provider Agent 默认链路

默认 AgentChat 入站不需要新增 ChatGPT 专属 action：

```text
incoming IM message
  -> provider agent config
  -> resolve effective runner
  -> load runner.adapter
  -> dispatch to chatgpt_web adapter when adapter = "chatgpt_web"
```

用户只要在 Provider Agent 配置里选择 runner `chatgpt-web`，普通消息就会走 ChatGPT Web adapter。

### 显式 Route Action

显式 route action 使用通用 runner action：

```json
{
  "type": "runner_agent_chat",
  "runner_id": "chatgpt-web",
  "operation": "ask",
  "reply_target": "original_chat",
  "delivery_mode": "final_reply"
}
```

不提供 `ChatGptWebAgentChat` 这种一功能一 action 的枚举。

### Schedule

Schedule 选择 runner，由 runner 的 adapter 决定执行实现：

```json
{
  "task_type": "runner",
  "runner": {
    "runner_id": "chatgpt-web",
    "operation": "ask",
    "prompt": "总结今天的 ChatGPT 会话"
  }
}
```

## 验证计划

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

新增 mock ChatGPT Web server：

- `GET /backend-api/accounts/check/v4-2023-04-27` 返回 logged in account / guest account / challenge 三种响应。
- `GET /backend-api/me` 返回辅助 identity / anonymous / challenge 三种响应，但不作为唯一登录判定。
- `GET /backend-anon/me` 返回 anonymous 对照响应。
- `GET /backend-api/conversations` 返回 readable / challenge 响应，并覆盖 anonymous readable 但 auth required 的场景。
- `POST /backend-api/f/conversation` 返回 handoff。
- `GET /backend-api/conversation/:id` 先 streaming 后 finished。

新增 E2E：

- `im_gateway_chatgpt_web_adapter_list`
- `im_gateway_chatgpt_web_adapter_ask_wait_final`
- `im_gateway_chatgpt_web_adapter_auth_required_opens_login`
- `im_gateway_chatgpt_web_adapter_redacts_artifacts`
- `agent_runner_chatgpt_web_adapter_channel_override`
- `chatgpt_web_handoff_submission_evidence` 覆盖 `browser_post_captured` 对 handoff recovery 的提交证据判定，避免短 SSE 中断后重复等待用户消息可见。
- `chatgpt_web_browser_defaults_to_headed_mode` 覆盖当前默认可见浏览器，确保真实 IM 验证期能观察页面实际输入与发送动作。
- `chatgpt_web_handoff_heartbeat` 覆盖浏览器退出、CDP 断开、页面 probe 失败时返回 `browser_unavailable`，避免 active run 静默占住 IM 队列。
- `chatgpt_web_target_page_state` 覆盖新建会话与追加会话的目标页判断：发送前必须处于稳定 `new_conversation` 或目标 `/c/{conversationId}`，并且 composer 可见。
- `im_gateway_mock_inbound_chatgpt_web_queue` 通过 `POST /_bifrost/api/im-gateway/debug/mock-inbound` 注入 IM 入站事件，复用 event loop / queue / runner 链路验证连续消息不再无请求长时间卡住。
- `im_gateway_external_cli_session_records` 覆盖 `chatgpt_web` / external runner 链路的 Session 记录：active session history 中可见用户输入与 assistant 输出；持久化 JSONL 中记录 `session_start`、`user_message`、runner `tool_call`、`tool_result` 成功/失败状态和 `assistant_message`，WebUI Session Detail 可追踪输入、输出、run id、artifacts 路径与异常。
- `generated_image_tool_result_is_final_and_counts_all_images` 覆盖 ChatGPT Web `image_gen` tool 结果可作为图片生成完成态，避免页面已有图片但 runner 仍等待最终 assistant 文本。
- `generated_image_tool_result_is_final_and_counts_all_images` 同时覆盖 ChatGPT Web `image_asset_pointer: sediment://file_...` 被解析成 `fileId`，下载阶段优先调用 `/backend-api/files/{fileId}/download` 获取签名原图 URL；DOM/Network 中的 estuary URL 只作为懒加载兜底。
- `dom_content_accepts_short_text_replies` 覆盖 DOM fallback 不再用固定字符数阈值过滤短回答，`OK`、`好` 等有效短回复必须投递到 IM。
- `dom_output_state_waits_for_generation_controls_to_finish` 覆盖 DOM fallback 的完成判定必须同时观察输出状态、stop 按钮、composer 与提交控制恢复状态；页面仍显示 `正在创建图片` 且发送区未恢复时只能继续等待，空 composer 下语音/提交控制恢复不应被误判为仍在输出。
- `stream_handoff_without_sse_requires_dom_grace` 覆盖 `stream_handoff` 且没有 SSE final 时，DOM fallback 必须等待结构性宽限窗口，不能在 ChatGPT 仍处理时把早期 DOM 段落当最终回复。
- `try_extract_dom_outcome` 的页面脚本覆盖 role-less image turn 兼容：最后一个 user turn 后若出现只有 `ChatGPT 说：` 与 `estuary/content` 图片的 conversation section，也必须作为 assistant 图片结果提取；只有空壳文本或 `最后微调一下...` 状态且无图片时必须继续等待。
- `page_url_matches_conversation` / `BrowserSession::find_conversation_page` 覆盖服务重启后的 headed browser 复用：当内存 tab pool 为空但 CDP `/json/list` 仍有目标 `/c/{conversation_id}` 时，发送阶段 attach 到现有 target、重新注册入池并复用，避免不断新开同一个 conversation 的重复 tab。
- `generated_image_assets_without_finished_path_list_are_final_when_not_streaming` / `generated_image_assets_wait_when_any_message_is_in_progress` 覆盖 asset-only 图片结果：无 `in_progress` 时可直接用 `image_asset_pointer` 完成，仍有消息生成中时不得提前结束。
- `chatgpt_web_delivery_uses_final_response_when_images_are_appended` 覆盖本地图片 Markdown 已追加到最终 `response` 时，IM 投递必须使用该最终 response，而不是下载前的 `all_texts`。
- `chatgpt_web_delivery_preserves_natural_process_batches` 覆盖 ChatGPT Web 多段文本投递：页面自然分批出现的过程/思考消息按批次分别投递，最后最终结论作为最后一批；图片场景仍强制使用带图片 Markdown 的最终 response。
- `chatgpt_web_dom_extraction_does_not_truncate_response_text` 覆盖 DOM 提取脚本不得对最终 `text` 或 `allMarkdownTexts` 执行固定 10000 字符截断，保证长结果 artifact 和 IM 最终输出使用全文。
- `agent_reply_collects_and_strips_generated_local_images`、`send_image_uploads_original_bytes_to_cdn_and_sends_image_item` 覆盖 ChatGPT Web / external runner 返回本地图片 Markdown 时，IM 回复文本剥离本地路径，并通过各 IM provider 的图片发送模式投递原图；Weixin 路径会先用 provider 返回的 `upload_param` 以 `POST` 上传原图密文字节到 Weixin CDN，再发送 `image_item`。
- `chatgpt_web_startup_auth_runners_include_all_web_runners` 覆盖服务启动预检的 runner 选择：只要 Runners 配置中存在 `adapter=chatgpt_web`，不依赖 IM channel 是否已启用，都要纳入启动登录态检查。
- `chatgpt_web_startup_auth_dry_run_reports_login_prompt` 和 `test_chatgpt_web_startup_auth_preflight.sh` 覆盖无登录态启动时的预检行为：复用强登录态判定，缺失/失效时后台拉起登录浏览器；E2E 使用 dry-run 钩子避免 CI 弹真实浏览器，同时验证启动日志和 auth status。

### Human Tests

实现时新增 `human_tests/chatgpt-web-adapter.md` 并同步索引，至少覆盖：

- TC-CWA-01 新建 runner，选择 adapter = `chatgpt_web`。
- TC-CWA-02 首次无登录态触发浏览器登录。
- TC-CWA-03 登录完成后同设备二次 run 不再弹窗。
- TC-CWA-04 登录失效后返回 auth_required 并重新弹窗。
- TC-CWA-05 `list` 展示真实 ChatGPT 会话列表。
- TC-CWA-06 `ask` 发起新会话并等待最终回答。
- TC-CWA-07 run detail 和日志不包含 cookie/token。
- TC-CWA-08 Provider Agent 选择 `chatgpt-web` runner 后，普通 IM 入站消息走 `chatgpt_web` adapter。
- TC-CWA-09 `/stop` 能停止当前 ChatGPT Web run 或等待流程。
- TC-CWA-12 性能回归：连续 ask 同一 session 时，确认请求仍走 ChatGPT Web 前端发送路径，且日志中包含 send/wait/total 耗时，短回答不再被固定 sleep 主导。
- TC-CWA-13 可观察执行：默认 Execution Mode 为 `headed`；运行时弹出真实浏览器窗口，用户可以看到页面输入与发送动作。
- TC-CWA-14 handoff 心跳：发送后如果浏览器进程退出、CDP 断开或页面不响应 probe，run 快速失败为 `browser_unavailable`，IM active session 被释放，后续排队消息不会等待 7200 秒。
- TC-CWA-15 mock IM 入站端到端：通过 debug mock inbound 接口连续注入消息，验证首条新建会话、后续追加到目标会话、active run 期间排队和队列消费。
- TC-CWA-16 Session 记录：IM 入站触发 `chatgpt_web` 后，AI -> Agent -> Sessions 的 active detail 能看到用户输入与最终输出；History Event Timeline 能看到 `session_start`、`user_message`、runner `tool_call`、`tool_result`、`assistant_message`。Runner 失败时，active detail 显示 `Runner failed:*`，History 中 `tool_result.success=false` 且包含脱敏异常信息。
- TC-CWA-17 生成图片原图发送：通过 IM 通道提问 `帮我生成4张可爱的小猫咪`，ChatGPT Web 识别 `image_gen` tool 结果，优先从 conversation JSON 的 `image_asset_pointer: sediment://file_...` 提取 `fileId` 并调用 `/backend-api/files/{fileId}/download` 获取签名原图 URL；如果结构化字段缺失，再从目标会话页面 / Network 解析 `/backend-api/estuary/content` 原图 URL。所有图片必须下载并缓存到数据目录附件存储，再通过可用 IM provider 逐张独立发送原图；Weixin 通道必须先按 Weixin 协议加密原图字节、`POST` 到 CDN，再发送 4 条包含 `image_item` 的 `image` 消息；Feishu 通道必须调用 Feishu 图片上传得到 `image_key`，再发送 Feishu `image` 消息。不同 provider 不共享图片发送 payload。
- TC-CWA-18 失败现场诊断：任意 ChatGPT Web runner 失败时，本轮 `chat_runs/<run_id>/` 下必须包含 `failure_diagnostics.json`；如果已知 `conversation_id`，必须尽力包含 `conversation_response.json`；必须尽力包含 `page_dom.html` 与 `page_dom.json`。这些 artifact 用于定位页面是否处于新建会话、目标会话、错误会话、登录挑战或 composer 异常状态。
- TC-CWA-25 服务启动登录态预检：当 Runners 配置包含 `adapter=chatgpt_web` 的自定义 runner 时，`bifrost start` 前台和 daemon 模式都在后台执行一次强登录态检查。若 `auth_status` 已证明可用，只记录 ready；若缺失、过期或 native/browser proof 不成立，则自动打开登录浏览器并等待用户登录，不等到首次 runner 使用时才暴露不可用。

### Review/Fix/Test

第 1 轮：

- 复核 runner 和 adapter 概念是否清晰：runner id 负责选择实例，adapter id 负责执行实现。
- Review auth state、artifact、Admin API 是否存在 secret 泄漏。
- 跑 adapter registry 单元测试、ChatGPT mock 单元测试和 mock E2E。

第 2 轮：

- 复查第 1 轮修复后的 diff。
- 真实浏览器登录验证一次，复跑 `list/ask/get/wait`。
- 检查 WebUI Runner 页面、Provider runner override、human_tests 文档和 readme 索引。

如第 2 轮发现 runner/adapter 概念混淆、登录态、浏览器弹窗、runner 选择或 token 脱敏问题，必须追加第 3 轮。

## 已知风险

- `chatgpt_web` 不是 CLI，公共概念必须命名为 Agent Runner / Adapter，不能让 CLI 专属命名进入最终 API、WebUI 或文档。
- `adapterConfig` 必须是 adapter-specific schema，并配合 capabilities 校验。
- ChatGPT Web 接口不是稳定公开 API，字段和 sentinel 机制可能变化；接口契约必须集中在 `ChatGptWebClient`。
- 写请求依赖浏览器环境，native HTTP replay 已验证会触发风控；写路径强制走 headless browser-context / 真实前端触发。
- 无 GUI 设备无法自动弹窗，必须提供 WebUI 登录入口和明确错误。
- headless 执行仍可能遇到 Cloudflare/challenge、composer 缺失或 send button disabled；必须 fail-closed 并返回脱敏诊断。
- 同一设备多账号切换时必须检测 account fingerprint，避免把旧账号登录态误用到新 run。
- WebSocket 增量输出与轮询等待属于同一个 `chatgpt_web` adapter 能力；即使增量不可用，等待最终结果必须可靠。

## 待审查问题

1. `chatgpt_web` adapter 的 `adapterConfig` schema 是否按本文拆成 `browser / chatgpt / auth` 三块？
2. run artifact 是否默认只保存脱敏 conversation summary，不保存完整 raw mapping？
