# Weixin Native Agent Experience 落地方案

## 1. 文档状态

- 设计目标：把 Bifrost 的 Provider 中立 Agent Progress、附件链路和微信 iLink 原生能力接起来，使微信从“文本 + 图片”升级为“Typing + 结构化工具进度 + 文件/视频/语音输入 + 可靠长轮询”。
- 基线：Bifrost `origin/main` `51b6420b`（2026-08-12）。
- 上游证据：[`Tencent/openclaw-weixin`](https://github.com/Tencent/openclaw-weixin) `cef0bfc390393f716903e16d50408118047f87e0`（2026-08-12 拉取）。
- 实施分支：`codex/weixin-native-agent-design`；PR：`#483`。本文同时记录目标设计与该分支的落地状态；PR 合入前，生产 `main` 仍以其代码 capability 返回值为准。

### 1.1 分支实施状态（2026-08-12）

- 已完成 capability-driven Progress 选择：飞书继续使用 MutableCard，微信使用独立 `WeixinProgressSession` 消费同一批 `AgentTurnProgressEvent`。为降低飞书回归风险，首版 Registry 使用 Feishu/Weixin 两个类型化 session map，而没有立即引入 trait object。
- 已完成微信 Typing ticket 获取、5 秒 keepalive、finish/drop CANCEL；工具事件映射为 type 11/12，同一 turn 的工具、final 文本、IMAGE/FILE/VIDEO 共用 `run_id`，每条消息使用独立 client ID。
- 已完成 FILE/VIDEO/VOICE 入站归一化、官方 CDN 下载、AES-128-ECB + PKCS7 解密、单文件 100 MiB/单消息 250 MiB/最多 6 个附件限制。
- 已完成 IMAGE/FILE/VIDEO 出站统一 pending media 与 CDN 加密上传：图片 type 2；文档、补丁、普通文件、音频 type 4；只有 MIME 与 MP4/WebM 签名一致的视频才发 type 5，否则安全退回 FILE。
- Agent final 的本地 Markdown 图片和附件会拆成通道原生消息；本地视频、音频、补丁扩展名可识别，MIME 从路径推断后再交给 Provider 分类。
- 已完成动态 long-poll timeout、2s/30s 退避、非零 `ret/errcode` 检查、`-14 -> authentication_required`、加密 cursor 原子持久化与 WebUI 重新扫码恢复。
- 自动化专项 E2E 已验证 Typing start/keepalive/cancel、11/12/final 顺序、统一 `run_id`、IMAGE/FILE/VIDEO、CDN 密文逐字节还原和 cursor 恢复。真实账号展示和通道实发仍按 `human_tests/weixin-provider.md` 执行。

## 2. 结论

微信不应模拟飞书 CardKit。两个 Provider 应消费同一套 `AgentTurnProgressEvent`，但选择不同的表现后端：

```text
AgentTurnProgressEvent
          |
          v
ImProgressPresentation
          |
          +-- MutableCard       -> Feishu CardKit create/patch/finalize
          +-- StructuredEvents  -> Weixin Typing + TOOL_CALL_START/RESULT + final
          +-- TextOnly          -> 其他 Provider 的兼容降级
```

首个可交付版本应按以下顺序落地：

1. 先修微信 long-poll 的超时、cursor、错误码和退出语义，保证后续消息不建立在不稳定连接上。
2. 增加 channel capability 与 turn-scoped delivery context，打通 `run_id`。
3. 接入 Typing 和结构化工具进度；任何进度失败都不得阻断最终回复。
4. 补齐通用文件入站/出站；复用已有 Agent 文件入口。
5. 再补视频和语音入站；有转写直接进 prompt，无转写作为音频附件，不在首版强制本地 ASR。
6. 最后优化微信文本 renderer；保留当前“整段首发失败后才拆分”的正确长文本行为。

## 3. 已核验的当前事实

| 领域 | Bifrost `main` | 官方上游 | 设计判断 |
| --- | --- | --- | --- |
| Progress 选择 | `resolve_external_cli_delivery_mode` 只对 Feishu 默认启用 ProgressCard | 微信支持 11/12 工具事件 | 去掉 Provider 类型硬编码，改为 capability 选择 |
| Progress Registry | `ImAgentProgressRegistry` 内部保存 `FeishuProgressCardSession` | 微信每 turn 创建 `run_id` | Registry 改为保存 Provider presentation session |
| Typing | 未调用 `getconfig/sendtyping` | ticket + 5 秒 keepalive | P0，best-effort，不影响 Agent turn |
| 文件入站 | 微信 normalize 固定 `files: Vec::new()` | FILE/VIDEO/VOICE 均可下载解密 | P0 文件，P1 视频/语音 |
| 文件出站 | `ImProvider` 已有 `upload_file/send_file`，微信 client 显式拒绝 | IMAGE/VIDEO/FILE 均已实现 | P0 补微信实现，不改上层发送协议 |
| 媒体元数据 | `ImImageAttachment` 有 AES/CDN 字段，`ImFileAttachment` 没有 | 各类媒体共用 `CDNMedia` | 给文件附件兼容新增 media kind 与 AES/CDN 字段 |
| long-poll | client 默认 30 秒；cursor 仅内存；未检查 `ret/errcode`；固定 3 秒 interval | 默认/建议 timeout 35 秒；cursor 落盘；失败退避；`-14` 特判 | P0 Reliability，改成连续 long-poll 状态机 |
| 长文本 | 先完整发送；仅失败后 1000 chars/3000 bytes 分片重试 | channel soft limit 4000 | 当前行为正确；首版不改，只把 renderer/阈值列为后续优化 |
| 语音 ASR | 有独立 ASR 能力，但本地 ASR 只支持 Apple Silicon macOS | `voice_item.text` 可直接使用 | 首版 transcript-first；无 transcript 作为附件，ASR 自动化另立能力开关 |

调研稿中“当前代码主动预切长文本”的判断与 `main` 不符；`WeixinProvider::send_text_with_client_id` 已经是整段首发失败后才分片。本文不把该项列入 P0 修复。

## 4. 目标与非目标

### 4.1 必须实现

- 微信收到消息后使用原生 Typing，turn 结束、失败、取消或 task abort 时可靠 CANCEL。
- `ToolStarted/ToolFinished` 映射为类型 11/12，单 turn 的进度、最终文本、图片和文件共享同一个 `channel_run_id`。
- 微信 FILE 入站可归一化、限流下载、AES-128-ECB + PKCS7 解密并进入 `ExternalCliChatInput.files`。
- 微信拆分投递的相邻文本与图片/文件在同会话 3 秒窗口内聚合；附件下载解密与窗口收集并行，最终只启动一个 Agent turn。
- 微信 FILE 出站复用 Agent 回复附件链路，完成 CDN 加密上传和 `type=4` 发送。
- long-poll timeout 始终大于服务端建议值并可被 shutdown 立即取消；cursor 原子持久化；非零 `ret/errcode` 不得被当作成功。
- 所有新增能力是 capability-driven；handler 不再通过 `provider_type == Feishu` 判断是否能展示 Progress。

### 4.2 必须不破坏

- 飞书进度卡 create/patch/rollover、群聊 reply/thread、reaction/recall 行为不变。
- `ImEventMessage.images` 与 `files` 的上层分流保持兼容：图片进入多模态，其他媒体进入文件附件。
- 微信仍要求有效 `context_token` 才能出站；`typing_ticket`、AES key、context token 不进入普通日志、history preview 或 API 响应。
- 进度消息、Typing 或媒体单项失败不能让已经成功完成的 Agent turn 变成失败。
- 当前长文本 full-first + failure-split 行为保持不变。

### 4.3 本轮明确不做

- 微信群聊、@Bot、Thread/Topic、Reaction、Recall、CardKit 模拟。
- 原生语音气泡出站；音频文件先按 FILE 发送。
- 自动对所有无 transcript 语音调用本地 ASR。该能力受平台、模型安装和资源互斥约束，应在媒体链路稳定后单独设计。
- 对 outbound `ref_msg` 宣称正式支持；待官方发送路径和真实账号验证稳定后再开放 capability。

## 5. Capability 模型

### 5.1 兼容扩展，不直接删除 `ImSendCapabilities`

新增 Provider 级模型，现有发送 API 可继续读取 `send` 字段：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImProgressPresentation {
    MutableCard,
    StructuredEvents,
    TextOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImInteractionCapabilities {
    pub typing: bool,
    pub progress: ImProgressPresentation,
    pub mutable_message: bool,
    pub native_reply: bool,
    pub reactions: bool,
    pub recall: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImConversationCapabilities {
    pub direct: bool,
    pub group: bool,
    pub thread: bool,
    pub mention: bool,
    pub requires_context: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImChannelCapabilities {
    pub send: ImSendCapabilities,
    pub interaction: ImInteractionCapabilities,
    pub conversation: ImConversationCapabilities,
}
```

`ImProvider` 增加：

```rust
fn channel_capabilities(&self, config: &ImProviderConfig) -> ImChannelCapabilities;

fn send_capabilities(&self, config: &ImProviderConfig) -> ImSendCapabilities {
    self.channel_capabilities(config).send
}
```

迁移期保留 `send_capabilities`，Admin API 的旧 JSON 不删字段。新客户端可读取完整 channel capability；旧客户端无感。

### 5.2 目标矩阵

| 能力 | Feishu | Weixin |
| --- | --- | --- |
| progress | `mutable_card` | `structured_events` |
| typing | false | true（实际 ticket 缺失时运行期降级） |
| text | native | native |
| markdown | native | degraded → readable text |
| image | native | native |
| file | native | native |
| video | unsupported（保持现状） | native，首轮可经 file API 路由 |
| voice in | unsupported（保持现状） | native |
| mutable message | true | false |
| direct/group/thread | true/true/true | true/false/false |
| requires context | false | true |

## 6. Progress 架构

### 6.1 把 Registry 的存储对象提升为 presentation session

保留 `ImAgentProgressSnapshot` 作为 Provider 中立投影模型。原始方案考虑把 registry 从“只保存飞书 session”改成保存 trait object：

```rust
#[async_trait]
pub trait ImProgressSession: Send + Sync {
    fn presentation(&self) -> ImProgressPresentation;
    fn channel_run_id(&self) -> &str;
    fn artifacts(&self) -> Vec<ProgressDeliveryArtifact>;

    async fn apply_events(&mut self, events: Vec<AgentTurnProgressEvent>) -> Result<()>;
    async fn update_queue_state(
        &mut self,
        items: Vec<QueueItem>,
        guide_pending: bool,
        notice: Option<String>,
    ) -> Result<()>;
    async fn update_runner_summary(&mut self, runner: ProgressRunnerSummary) -> Result<()>;
    async fn restart_turn(&mut self, initial_message: &str) -> Result<()>;
    async fn finish(&mut self, final_text: &str, failed: bool) -> Result<()>;
}
```

- `FeishuProgressCardSession` 实现该 trait；`artifacts()` 返回卡片消息 ID，现有 thread anchor 逻辑继续工作。
- 新增 `WeixinProgressSession`；`artifacts()` 返回已发工具事件的 message/client ID，群聊 anchor 不消费这些数据。
- `ImAgentProgressRegistry` 对外保留当前 `apply_events/update_queue_state/update_runner_summary/finish` 入口，减少 event loop 改动。
- `start_progress_for_provider(...)` 是唯一 factory，根据 `channel_capabilities().interaction.progress` 创建 session。业务 handler 不再直接调用 `start_feishu_*`。

实施分支选择更小的兼容改动：Registry 内保留现有 Feishu map，并新增类型化的 Weixin map；公共 `apply_events/finish` 入口按实际 session 分派。这样不改写飞书 session trait 边界，Card builder、预算、rollover 逻辑保持原地，同时仍由 `ImProgressPresentation` capability 决定创建哪种 session。后续 Provider 增多时再提取上述 trait。

### 6.2 turn-scoped delivery context

微信的 `run_id` 是通道展示关联 ID，不等同于外部 Runner 最终返回的 `result.run_id`。开始 turn 时先创建 UUID：

```rust
pub struct ImDeliveryContext {
    pub client_message_id: Option<String>,
    pub channel_run_id: Option<String>,
    pub agent_run_id: Option<String>,
}
```

- `channel_run_id`：presentation 创建时生成，在整个 turn 内固定。
- `agent_run_id`：Runner 返回后补充，用于本地日志关联，不回写替换已经发出的 `channel_run_id`。
- `client_message_id`：每条消息独立，继续承担幂等键；不能拿同一个 client ID 发送多条进度。

实施分支没有扩大公共发送 trait：`WeixinProgressSession` 在开始时通过 `(provider/account, target)` 注册 active channel run，现有 text/image/file 发送方法从该 turn-scoped map 读取并写入 `msg.run_id`，final 全部发送结束后显式释放。Feishu 完全不消费该 map。Runner 自身的 `agent_run_id` 继续留在既有本地运行记录中，本轮不为 `ImMessageLog` 增加尚无消费方的关联字段。

### 6.3 Weixin event 映射

| Agent event | 微信行为 | 说明 |
| --- | --- | --- |
| session start | `sendTyping(TYPING)` | ticket 缺失或失败时静默降级 |
| `ToolStarted` | `TOOL_CALL_START` | `is_completed=false` |
| `ToolFinished` | `TOOL_CALL_RESULT` | status 取 `success/failed` |
| `PlanUpdated` / `Status` / `SubAgentUpdated` | 不单独发消息 | 保留在 snapshot/本地历史，避免刷屏 |
| `AssistantDelta` | 不逐 token 发送 | 微信不支持 patch，等待 final |
| `TurnFinished` | CANCEL Typing，然后最终文本/媒体 | 全部带同一 `run_id` |
| `TurnFailed` | CANCEL Typing，然后失败摘要 | 不泄露内部错误全文 |
| task abort/drop | best-effort CANCEL | watchdog 兜底停止 keepalive |

当前 `AgentTurnProgressEvent` 没有工具调用 ID。首版由 `WeixinProgressSession` 维护 `PendingToolCall` 队列：

1. start 时按 turn 内递增序号生成 `<run-short>-tool-0001`。
2. finish 优先按 `(tool_name, normalized arguments hash)` 匹配最早未完成项，再按同名匹配。
3. 无法唯一匹配时省略协议允许为空的 `tool_call_id`，不得错误关联。

后续若 Agent 核心事件增加稳定 `tool_call_id`，presentation 直接透传，删除启发式匹配。

### 6.4 进度失败语义

- Typing、工具 start/result 都是 `best_effort`；失败写 message log/metric 后继续 turn。
- final 文本仍使用现有可靠发送路径；只有 final 失败才影响 outbound delivery 状态。
- 相同 progress event 通过 `<channel_run_id>:<sequence>:<phase>` 生成 client ID；进程内重试复用该 ID。
- 不对 failed start 补发伪 result；本地 metric 记录不完整 pair。

## 7. Typing 设计

新增 `WeixinUserConfigCache`，key 为 `(provider_id, receive_id)`：

- 首次 turn 调 `POST /ilink/bot/getconfig` 获取 `typing_ticket`。
- ticket 仅存内存，不落盘；缓存 TTL 20–24 小时随机抖动，避免集中刷新。
- `sendtyping status=1` 成功后每 5 秒 keepalive。
- finish/failure/cancel 调 `status=2`；停止 keepalive 后再发最终回复。
- ticket 错误时清缓存并最多刷新一次；仍失败则本 turn 降级为无 Typing。

用显式 `TypingLease::finish().await` 处理正常路径，用 task abort handle/watchdog 处理 drop 路径。Rust `Drop` 中不启动不可等待的网络请求。

进程关闭顺序固定为：停止接收新 turn → 给 active progress session 一个有界 drain 窗口并 CANCEL Typing → abort 剩余 keepalive task → 停止 Provider long-poll/HTTP runtime。不能先销毁 Provider 再尝试 CANCEL。

## 8. 媒体与附件

### 8.1 不做 `images/files` 大爆炸合并

上层已有明确分流：`images -> ChatImageInput`，`files -> ExternalCliFileInput`。首版保留该 API，只给文件附件补齐微信需要的定位信息：

```rust
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImFileMediaKind {
    #[default]
    File,
    Video,
    Voice,
}

pub struct ImFileAttachment {
    // 现有字段保持
    pub file_key: String,
    pub name: Option<String>,
    pub mime_type: Option<String>,
    pub size_bytes: Option<u64>,
    pub data_base64: Option<String>,
    pub download_url: Option<String>,

    // 兼容新增
    pub media_kind: ImFileMediaKind,
    pub encrypted_query_param: Option<String>,
    pub aes_key: Option<String>,
    pub transcript: Option<String>,
    pub duration_ms: Option<u64>,
    pub codec: Option<String>,
}
```

全部新增字段带 `#[serde(default)]` 或 `skip_serializing_if`，不破坏旧事件和存量 JSON。图片继续使用 `ImImageAttachment`；Provider 内部提取共享 helper `parse_cdn_media` 和 `download_and_decrypt_media`，避免加密逻辑复制四份。

### 8.2 入站映射

| iLink item | Bifrost event | Runner 输入 |
| --- | --- | --- |
| IMAGE(2) | `message.images` | `ChatImageInput` |
| FILE(4) | `message.files`, kind=file | `ExternalCliFileInput` |
| VIDEO(5) | `message.files`, kind=video, MIME `video/mp4` | `ExternalCliFileInput` |
| VOICE(3) 有 `text` | 将 `【语音转写】\\n{text}` 合并进 message text | 普通 prompt；不强制下载音频 |
| VOICE(3) 无 `text` | `message.files`, kind=voice | 音频附件；Agent 决定如何处理 |

如果一条消息同时包含文本和语音转写，保留二者并明确分区，避免把转写误当原始文本。多个媒体 item 全部归一化，不能沿用官方插件“只取第一个可下载媒体”的限制。

无 transcript 的语音解密后按协议 codec 处理：SILK 转码器可用时生成 WAV；不可用时保留 `audio/silk` 原始附件并在 prompt 中加入“语音未转写”的可读提示，不能伪装成通用 WAV。自动 ASR 仍不属于首版。

### 8.3 下载与解密

`WeixinProvider::download_message_file_resource` 与图片共用以下流程：

1. 优先使用 `full_url`；缺失时用 `encrypt_query_param` 构造 CDN download URL。
2. production 只允许官方 CDN host；debug E2E 仅在显式 loopback allowlist 下允许本地 mock。
3. 禁用环境代理，限制重定向次数，并校验最终 host，防止 provider payload 变成 SSRF 跳板。
4. 先检查 `Content-Length`，再流式读取并执行硬上限；不能使用无界 `response.bytes()`。
5. 有 AES key 时支持 16-byte base64、32-char hex，以及历史的 base64(hex ASCII) 兼容格式。
6. AES-128-ECB + PKCS7 解密失败返回明确 media kind/file key，但日志不打印 key 内容。
7. 单文件上限 100 MiB、单消息最多 6 个附件、总量 250 MiB；超限附件跳过并记录可观测错误，不把 base64 写入日志。

### 8.4 相邻文本与附件聚合

微信客户端可能把用户一次“说明文字 + 图片/文件”操作拆成同一 sender 的两个事件，并产生约 1–2 秒间隔。Bifrost 以首事件 `received_at` 为基准保留 3 秒 companion window：

1. 首事件完成路由后先注册 session mailbox，再开始任何微信 CDN 下载，避免下载期间到达的配套事件被 busy-mode 当成独立 turn。
2. 首事件附件下载/AES 解密与 mailbox 收集并行；窗口内到达的附件随后下载，最终把全部文字、图片和文件组装成一个 `ExternalCliChatInput`。
3. 文本先到、附件先到均适用；模型实际收到合并后的文字，以及 External CLI 保存得到的附件绝对路径。
4. 只有纯文本事件时不合并语义，后续文本进入既有 FIFO queue；slash command 始终独立排队。
5. mailbox 以 `session_key` 隔离，不跨 provider/sender 合并；超过 3 秒的事件继续走正常并发/排队路径。

普通微信非 slash 消息因此最多增加约 3 秒首轮延迟，这是避免错误拆 turn 的显式体验权衡。

### 8.5 出站统一存储和上传

把只支持图片的 `OutboundImage` 替换为内部 `PendingOutboundMedia`：

```rust
struct PendingOutboundMedia {
    kind: OutboundMediaKind, // Image | File | Video
    file_name: String,
    bytes: Vec<u8>,
    mime_type: Option<String>,
    created_at_ms: u64,
}
```

- `upload_image` 写入 kind=image。
- `upload_file` 用 MIME 与文件签名共同判断；两者一致且为支持的视频格式时选择 video，缺失或冲突时安全退回 file，不能仅信任调用方 MIME。
- `send_image/send_file` 取得 target 后执行公共 `upload_media_for_target`：MD5 → random AES key/filekey → `getuploadurl(media_type)` → CDN upload → 构造 MessageItem。
- image/video/file 分别发送 type 2/5/4；音频出站仍按 file/type 4。
- 首版出站上限与现有 Agent reply 约束一致：image 10 MiB，file/video 30 MiB；iLink 更高上限只有真实账号验证后才提高。
- pending store 设置 10 分钟 TTL、256 项和 256 MiB 总量上限；成功、失败和超时都清理，避免当前图片失败路径长期占内存。
- Agent 回复附件现有 `prepare_agent_reply_text_and_images_with_downloads` 和 `send_agent_reply_*` 不改变调用意图，只让 Weixin 的 `upload_file/send_file` 从 unsupported 变为可用。

## 9. long-poll Reliability

### 9.1 状态机

取消固定 3 秒 interval；一次 long-poll 返回后立即进入下一次：

```text
LoadCursor
    |
    v
Polling --success--> PersistCursorAfterEnqueue --immediate--> Polling
    |
    +-- network/API error (1..2) --> wait 2s --> Polling
    +-- 3 consecutive errors ------> wait 30s --> Polling
    +-- ret/errcode -14 -----------> AuthenticationRequired --> stop
    +-- shutdown ------------------> cancel request --> Stopped
```

### 9.2 timeout

- long-poll 使用独立 HTTP client，不继承普通 API 的 30 秒全局 timeout。
- 初始 `server_timeout_ms = 35_000`。
- 每次响应若带合法 `longpolling_timeout_ms`，clamp 到 5–120 秒并作为下一轮 server timeout。
- request timeout = `server_timeout_ms + 5_000ms` network margin。
- shutdown 通过 `tokio::select!`/cancellation token 取消 in-flight request，不等待服务端超时。

### 9.3 cursor 持久化与交付顺序

新增 `WeixinSyncCursorStore`，路径放在 Bifrost data dir 的 admin 存储区，采用 versioned JSON、0600、临时文件 + fsync + atomic rename。entry key 使用 `SHA-256(provider_id + NUL + account_id)`，value 使用现有 `LocalSecretKey` 加密，避免多 Provider/账号互相覆盖，也不把 opaque cursor 明文落盘。单 cursor 最大 1 MiB，损坏或解密失败只丢弃对应 entry。

顺序必须是：

1. 校验 HTTP 与 `ret/errcode`。
2. normalize 全部消息。
3. 把事件成功 enqueue 给统一 event loop。
4. 原子保存新 cursor。
5. 更新内存 cursor。

如果第 3/4 步失败，不推进 cursor；重启后允许重复投递，并依赖稳定 `event_id` 和现有去重窗口实现 at-least-once，而不是冒险丢消息。

`-14` 表示当前授权不可继续使用：连接状态改为 `authentication_required`，停止本轮 poll，WebUI 提示重新扫码。Bifrost 不在旧 token 上无限快速重试。

## 10. 文本 renderer

P0 不引入完整 Markdown AST 重写。先把当前规则明确为 `WeixinTextRenderer` 接口，并保持输出兼容：

- 本地/远程图片仍拆成真实图片消息。
- 可下载附件链接仍拆成真实文件消息。
- 文本保留标题、列表、代码块和表格可读结构；只清理微信明显无法显示的 marker。
- 微信原生文本气泡会把单个 LF 折叠为空格；发送前把单换行提升为可见的空行分隔，同时保留已有连续空行，保证 `/help` 等逐行内容不会连成一段。
- 继续先整段发送；仅 `sendmessage` 失败后按安全阈值分片重试。

P1 再评估 4000 soft limit。没有真实账号上限证据前，不把 full-first 改成预切分，也不声称 4000 是 iLink 服务端硬限制。

## 11. 文件级实施清单

| 文件/模块 | 变更 |
| --- | --- |
| `im_gateway/types.rs` | channel capabilities、文件 media kind/AES/CDN 字段、`authentication_required` 状态 |
| `im_gateway/provider.rs` | `channel_capabilities` 默认适配，保留原发送 trait 兼容性 |
| `handlers/im_gateway/service.rs` | client 分派新增 capabilities/context/file download，不再拒绝微信文件 |
| `im_gateway/progress_card.rs` | 保留 snapshot/Feishu renderer；Registry 新增类型化 Weixin session map 与公共分派 |
| `im_gateway/weixin_progress.rs`（新） | Typing lease、工具 correlator、11/12 payload、turn finish |
| `im_gateway/weixin.rs` | API types/helper、dynamic poll、cursor、文件/视频上传下载、run_id 发送、稳定纯文本换行渲染 |
| `im_gateway/weixin_sync_store.rs`（新） | versioned atomic cursor store |
| `im_gateway/connection.rs` | 消费微信连接状态事件，把 `-14` 暴露为 `authentication_required` 并停止旧连接 |
| `handlers/im_gateway/providers.rs` | 状态 API 返回可操作的重新扫码语义，复用现有 login start/status/complete |
| `handlers/im_gateway/event_loop/external_runner.rs` | capability-driven progress start/finalize、微信 3 秒 companion 聚合，删除 Feishu 类型硬编码 |
| `handlers/im_gateway/agent_reply*.rs` | 把同一 delivery context 传给最终文本、图片、文件 |
| `web/src/api/imGateway.ts` / `ImGatewayTab.tsx` | 识别 `authentication_required`，展示并触发现有二维码恢复流程 |
| `e2e-tests/tests/test_weixin_provider_e2e.sh` | typing/progress/file/cursor/restart 场景 |
| `human_tests/weixin-provider.md` | 真实微信 Typing、工具进度、文件、语音、重启恢复用例 |

## 12. PR/阶段拆分

### PR 1：基础抽象（无行为变化）

- 新增 `ImChannelCapabilities`、`ImProgressPresentation` 与微信 turn-scoped active run map。
- 旧发送方法与 JSON 保持兼容。
- Feishu/Weixin capability matrix 单测。
- 完成标准：现有 Feishu/Weixin E2E 全部不变通过。

### PR 2：long-poll Reliability

- 独立 long-poll client、动态 timeout、cursor store、错误状态机、shutdown cancel。
- 连接状态暴露 `authentication_required` 和连续失败原因。
- 完成标准：延迟 35 秒 mock 不超时；重启带 cursor 续拉；cursor 保存失败不推进；`-14` 停止并在 Connections UI 提供重新扫码入口。

### PR 3：Typing + Structured Progress + run_id

- progress session trait/registry；Feishu adapter 保持原行为。
- Weixin config cache、Typing lease、11/12 映射、同 turn run_id。
- final 文本/图片携带相同 run_id。
- 完成标准：mock 观察到 start → keepalive → tool start/result → cancel → final，失败注入不阻断 final。

### PR 4：FILE 入站/出站

- `ImFileAttachment` 兼容字段、download/decrypt、pending outbound media、type 4。
- capability 从 unsupported 改为 native。
- 完成标准：真实 event → Agent file input；Agent attachment → CDN decrypt 后字节完全一致 → FILE message。

### PR 5：VIDEO/VOICE 入站与 VIDEO 出站

- type 5 下载/上传；voice transcript-first；无 transcript 作为音频文件。
- 完成标准：多 item 不丢；转写文本分区正确；无转写音频可落到 Runner 附件；视频字节一致。

### PR 6：WeixinTextRenderer 优化

- Markdown 可读性、长文本实测阈值、失败降级。
- 只有真实账号证据支持时才调整 1000/3000 fallback 阈值。

合并依赖为 `PR1 -> PR2 -> PR3 -> PR4 -> PR5`；PR6 可在 PR4 后并行。不要把所有能力压进一个超大 PR。

## 13. 测试与覆盖率计划

### 13.1 单元测试

- capability：Feishu=`mutable_card`、Weixin=`structured_events`，旧 send capability JSON 保持兼容。
- progress：同 turn 共用 run_id；client ID 每事件唯一；start/result 配对；乱序/缺 start 不错误关联；发送失败仍完成 final。
- typing：cache hit/miss/过期；5 秒 keepalive；finish/failure/abort cancel；ticket 刷新最多一次。
- lifecycle：shutdown 先 CANCEL/drain progress，后停止 Provider；超时后 abort keepalive 不阻塞退出。
- poll：35 秒超时 margin；server timeout clamp；ret/errcode；`-14`；2s/30s backoff；shutdown cancel。
- connection/UI：`authentication_required` 序列化兼容、状态 API 脱敏、Connections UI 重新扫码动作。
- cursor：多 provider/account 隔离；本地加密；原子重启恢复；损坏/超限拒绝；保存失败不更新内存；0600。
- media：FILE/VIDEO/VOICE normalize；hex/base64 AES key；PKCS7 解密；大小上限；SSRF/redirect 拒绝；MIME/签名冲突退回 FILE；SILK 转码可用/不可用两条路径；pending store 清理。
- outbound：FILE type 4、VIDEO type 5、MD5/size/media_type、CDN 密文可还原原文、全部携带 run_id。

### 13.2 E2E

扩展现有 `test_weixin_provider_e2e.sh`，不另起一套重复 mock：

1. mock `getupdates` 首轮等待超过 30 秒并返回 `longpolling_timeout_ms=35000`。
2. 重启真实 Bifrost，断言请求携带前一轮 cursor 且旧消息不重复进入 history。
3. 注入 text + file + voice transcript + video 多 item，断言 event 归一化完整。
4. 运行 mock external runner，断言 Typing、工具 11/12、final 的同一 run_id 和时序。
5. 注入 typing/progress API 失败，断言 final 仍成功。
6. 文件/视频出站解密 CDN 密文，逐字节等于原始 fixture。
7. 返回 `-14`，断言连接状态可诊断且 poll 不形成忙循环。
8. 让 mock `getupdates` 以约 2 秒间隔返回文本和 inline 图片，断言 external runner 只启动一次，prompt 同时包含原文本和 `Attached Images` 本地绝对路径。
9. 发送含单换行的帮助文本，断言微信 payload 使用可见的双 LF 分隔且现有段落不会继续膨胀。

### 13.3 human_tests

更新 `human_tests/weixin-provider.md`，真实执行：

- 发起 30 秒以上真实 Agent 任务，观察 Typing 持续且最终消失。
- 触发至少两个工具调用，观察微信原生 start/result 顺序和最终回复。
- 发送文档、视频、有转写语音、无转写语音，验证 Agent 实际可见内容。
- Agent 返回文件，验证微信收到的文件名、大小、内容。
- 分别测试“文字后发图片”和“图片后发文字”，确认只产生一个 Agent turn，回答同时理解文字与附件。
- 发送 `/help`，确认每条命令独立成行、分组之间留空，不再连成一整段。
- turn 运行时停止/重启 Bifrost，验证 Typing 不永久残留、cursor 恢复且消息不丢。

### 13.4 coverage 90% 门禁

每个修改生产 Rust 的 PR 都必须：

1. 先补对应单测/E2E。
2. 推送前执行无 filter 的 `make coverage-changed`，changed-lines 达到 95%。
3. 远端通过 `bash scripts/ci/coverage-all.sh --json --gate` 的 crate/workspace 门禁。
4. `weixin.rs` 若继续膨胀会降低可测性，因此 poll、sync store、progress、media 应按上文拆模块。

## 14. 可观测性与安全

建议 metrics/log fields：

- `weixin_poll_latency_ms`、`weixin_poll_failures_total{code}`、`weixin_cursor_persist_failures_total`。
- `weixin_typing_start/cancel/keepalive_total{status}`。
- `weixin_progress_events_total{type,status}`、`channel_run_id`（可记录 UUID，不记录 ticket/token）。
- `weixin_media_download/upload_bytes{kind}`、`weixin_media_failures_total{stage,kind}`。

必须统一 redaction：`bot_token`、`context_token`、`typing_ticket`、`aes_key`、完整 `encrypt_query_param`、带签名 CDN URL 均不得进入日志。错误日志只记录 provider ID、media kind、file key 的短 hash、HTTP status 和协议错误码。

## 15. 发布与回滚

- capability 只有在对应实现合入后才从 unsupported 切 native，设计文档不能提前让 API 宣称可用。
- Typing ticket 缺失自动降级；结构化进度失败自动退化为 final-only；不需要用户配置开关才能保住基本可用性。
- PR3 上线后重点观察 progress/typing error ratio；异常时可通过 capability 临时回退 `TextOnly`，无需回滚媒体或 poll 修复。
- PR4/PR5 的媒体 capability 可按 kind 独立关闭；下载失败只跳过该附件并给用户可读提示。
- cursor store 格式带 version；回滚旧版本时不会读取该文件，但服务端仍可从空 cursor 恢复。由于可能重放，稳定 event ID 去重必须先验证。

## 16. 验收门禁

以下全部满足才可称“微信 Native Agent Experience 已落地”。分支自动化已关闭的条目标为完成；真实账号、全量验证和 CI 在本任务后续继续执行：

- [x] handler 按 capability 选择 Progress；飞书行为由原回归测试守护。
- [x] 同一微信 turn 的工具事件、final 和媒体有同一 `run_id`，每条消息有独立 client ID。
- [x] Typing 在 success/failure/cancel/drop 路径均停止；进程 shutdown 由连接 drain/abort 路径守护。
- [x] 文件入站/出站 mock 真实字节验证通过，超限、解密错误和 SSRF/redirect 路径被拒绝。
- [x] long-poll request timeout 带 network margin，cursor 可恢复，`-14` 可诊断且不忙循环。
- [x] 微信相邻文本/附件在 3 秒窗口合并为单 turn，下载期间仍持续收集 companion 事件。
- [x] 微信纯文本单换行被渲染为客户端可见分隔，长文本仍保持 full-first/failure-split。
- [ ] 单元测试、E2E、human_tests、`make coverage-changed`、workspace all-features 和远端 coverage gate 均通过。
- [ ] 至少两轮 Review/Fix/Test 已关闭发现的问题，设计、代码、capability、测试和文档一致。

## 17. 真实账号验收时确认的协议点

这些问题不阻塞 PR1/PR2，但在打开对应 capability 前必须用真实账号确认：

1. TOOL_CALL 11/12 在不同微信客户端版本的实际展示，以及 `tool_call_id` 缺失时的行为。
2. FILE/VIDEO 的真实单文件上限、文件名编码和 MIME 展示。
3. `voice_item.text` 的可用率、语言和空字符串语义。
4. `sendtyping` ticket 失效错误码，是否需要除 TTL 外的强制刷新条件。
5. server 对 `run_id` 格式/长度的限制；首版使用标准 UUID。
