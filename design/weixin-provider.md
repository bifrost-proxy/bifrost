# Weixin IM Provider 方案

## 背景

微信接入现在只作为 IM Gateway 原生 `weixin` provider 存在。Provider 负责扫码登录、长轮询收消息、把微信消息归一化成 `ImEvent`、触发现有 Agent 路由，并把 Agent 回复回写到微信 ClawBot。所有 `/status`、`/help`、guide、queue、最终回复、图片多模态等能力复用 IM Gateway 与 Agent 现有通道能力。

## 用户目标验证清单

### 必须实现

- IM Gateway 新增 `weixin` provider 类型，用户可在 WebUI Connections 中创建、扫码登录、连接和停用。
- Weixin provider 登录使用 iLink 扫码接口，完成后把 bot token 存入 provider `secret_ref`，API/WebUI 响应不得泄露 token。
- WebUI 创建或检测到 Weixin provider 未授权/失效时，自动弹出二维码、自动轮询授权状态，并在过期时刷新二维码。
- Weixin provider 使用 `getupdates` 轮询消息并归一化为 `message.receive` 事件。
- 用户从微信 ClawBot 发送普通消息后，默认 Agent chat 链路应启动任务，并立即回写开始处理反馈。
- Weixin provider 使用官方 `sendmessage` 结构把 Agent 回复或手动发送内容回写到微信消息发送方。
- Weixin provider 入站消息不使用 owner 过滤器；`owner_open_id` 只记录扫码登录账号。Feishu provider 继续保持 owner 安全边界。
- 微信通道支持所有 Agent `/xxx` 命令，行为与飞书 IM 通道一致。
- 微信图片消息需要解析 `image_item/media`，下载图片，必要时解密，并以多模态图片输入传给模型。
- Agent / ChatGPT Web 生成的本地图片需要按原图通过微信通道独立发送为图片消息，不能只退化为 Markdown 文本或卡片正文。
- 最终回复直接发送模型最终文本，不添加 `Bifrost AI`、`等待任务说明` 等卡片前缀。

### 必须不破坏

- 不复用或污染 `~/.openclaw`。
- 不打印 token、context token、cookie 或二维码内部 poll key。
- 不启动系统代理；相关 E2E 必须使用 `--no-system-proxy`。
- Feishu owner 过滤、卡片进度和回复目标逻辑保持原有安全边界。
- 现有 Agent 扩展系统保持原样，微信通道只出现在 IM Gateway provider 管理入口中。

### 必须真实验证

- 单元测试覆盖 Weixin 登录、事件归一化、sendmessage 错误识别、官方消息格式、回复目标、图片解析、图片下载与 AES-128-ECB 解密。
- E2E 使用 iLink 兼容 mock 验证 provider 扫码开始、自动状态检查、扫码完成、连接轮询、事件入库、图片附件归一化和 `sendmessage` 回写。
- human_tests 必须启动真实 Bifrost，让用户在 WebUI 创建/启用 Weixin IM provider，扫码后从微信 ClawBot 发消息，由 Agent 观察日志、事件、消息记录、图片下载和回复。

## 实现逻辑

### Provider 配置

`ImProviderType` 增加 `Weixin`。扫码完成后 provider 持久化 `app_id`、`owner_open_id`、`base_url`、`secret_ref`、`event_connection_enabled` 和 `event_types`。其中 `secret_ref` 是 iLink bot token，只本地保存，API 返回仅暴露 `secret_configured`。

### 登录 API

新增 provider 级 API：

```text
POST /api/im-gateway/providers/:id/weixin-login/start
GET  /api/im-gateway/providers/:id/weixin-login/status
POST /api/im-gateway/providers/:id/weixin-login/complete
```

`start` 调 `GET /ilink/bot/get_bot_qrcode?bot_type=3`，只返回可扫码的 `scan_url` 和 `expires_in_seconds`，不返回 poll key。WebUI 创建 Weixin provider 后立即调用 `start` 并弹出二维码 Modal；Modal 每 2 秒调用 `status` 自动检查扫码/授权状态。二维码临近过期或已过期时，WebUI 自动调用 `start` 刷新二维码，不要求用户手动点击“已扫码”。`status` 看到 confirmed/authorized 后会在服务端完成 provider 更新并返回脱敏 provider/account 摘要。

### 事件连接

`ImConnectionManager` 持有 `WeixinProvider`。`connect_events` 以固定间隔调用 `POST /ilink/bot/getupdates`，认证头为 `AuthorizationType: ilink_bot_token` 和 `Authorization: Bearer <bot_token>`。

返回消息被归一化为 `ImEvent`：`provider_type = weixin`、`event_type = message.receive`、`source.user_id/source.chat_id = from_user_id`、`source.message_id = message_id`、`message.text` 优先读取 `text/content/msg/message`，并兼容真实 iLink `msgs` + `sync_buf` + `item_list[].text_item.text` 结构。数字型 `message_id` 会转为字符串；缺失 ID 时使用 raw JSON 稳定 hash，避免 timestamp 造成重复事件不可去重。

Feishu 入站继续按 `owner_open_id` 过滤非 owner 消息；Weixin 入站不做 owner 过滤，避免把普通微信发送方误拦截。

### 回复回写

`ImProviderClient` 抽象 Feishu/Weixin 发送能力。Weixin 的 `send_text` 调 `sendmessage`，payload 使用官方 `msg + base_info` 结构，`msg.item_list[].text_item.text` 承载文本；`send_card` 只把卡片正文降级为文本后调 `send_text`。poll 到消息时保存 `context_token`，后续向同一用户发送时会带上该 token。

Agent 回复目标按 provider 区分：Weixin 优先回写本次入站事件的 `source.chat_id/source.user_id`，不会优先发给 `owner_open_id`；Feishu 继续优先发给 owner。消息日志记录真实目标 ID，避免只显示内部 `__agent_reply__` 占位符。

Weixin 没有 Feishu CardKit 进度卡能力，因此默认 Agent chat 进入真实 turn loop 前会先向消息发送方发送一条纯文本即时反馈；运行中再次收到同一会话消息时沿用 guide/queue 逻辑并立即回执“已注入引导消息”或排队状态。`/status`、`/stop`、session-free 命令以及其余 Agent slash 命令都走和 Feishu 相同的 IM 事件入口。

### 图片消息

图片消息从 `item_list` 中 `type=2` / `image_item` 提取为 `ImImageAttachment`，保留 `media.full_url`、`media.encrypt_query_param` 和 AES key。图片-only 消息不再把 raw JSON 当文本，而是使用默认图片理解提示进入 Agent 多模态路径。

图片下载走 `ImProviderClient::download_message_image_resource`：优先使用服务端返回的 `full_url`，没有时使用 `https://novac2c.cdn.weixin.qq.com/c2c/download?encrypted_query_param=...` 作为 CDN fallback；如果 `image_item.aeskey` 或 `media.aes_key` 存在，则按 OpenClaw Weixin 协议使用 AES-128-ECB + PKCS7 解密后，再以 base64 data URL 形式传给模型。

出站图片走同一 `ImProviderClient` 抽象。Agent 回复解析到本地 Markdown 图片（例如 `![cat](./cat.png)`）时，先从文本回复中移除本地图片引用，再逐张调用 provider `upload_image/send_image` 独立发送原图。Weixin 的 `upload_image` 在进程内保存待发送原始字节；`send_image` 拿到目标用户后按 OpenClaw Weixin 协议调用 `getuploadurl`，使用随机 16 字节 AES key 执行 AES-128-ECB + PKCS7 加密，把密文上传 CDN，并用返回的 `x-encrypted-param` 构造 `sendmessage` 的 `image_item.media`。消息日志必须记录 `msg_type=image`、目标用户、发送状态和错误，但不能记录图片原始字节。

## 测试方案

### 单元测试

- `normalize_update_extracts_ilink_item_list_text_and_numeric_message_id`：验证真实 iLink 文本消息结构。
- `normalize_update_extracts_ilink_image_item_for_multimodal_agent`：验证图片附件提取。
- `download_message_image_resource_fetches_plain_full_url`：验证图片下载。
- `decrypt_aes_128_ecb_accepts_base64_raw_key`：验证 AES-128-ECB 解密。
- `send_image_uploads_original_bytes_to_cdn_and_sends_image_item`：验证出站图片原始字节被加密上传到 CDN，并以 `image_item.media` 独立发送。
- `send_image_returns_config_error_for_unknown_image_key`：验证未知图片 key 不会发送空消息。
- `agent_reply_target_uses_weixin_sender_instead_of_owner`：验证回复目标使用微信发送方。
- `im_event_loop_forwards_image_attachment_to_agent_chat`：验证 IM 图片传入 Agent 多模态请求。

### E2E 测试

`e2e-tests/tests/test_weixin_provider_e2e.sh`：

1. 构建真实 `bifrost`。
2. 使用临时 `BIFROST_DATA_DIR` 启动真实进程，显式 `--no-system-proxy`。
3. 使用本地 iLink 兼容 mock 验证 provider 级 `login start/status/complete`，并断言响应不泄露 poll key 或 token。
4. 连接 provider，mock `getupdates` 返回真实形态 `msgs/sync_buf/item_list` 文本和图片消息，断言 `history/events` 出现 `provider_type == weixin` 的 `message.receive`。
5. 断言图片事件包含 `message.images[0].download_url` 和 file key。
6. 调用 `messages/send`，断言 mock `sendmessage` 收到目标用户、文本、`context_token` 和官方 `base_info`。
7. 调用 Weixin provider 图片发送路径，断言 mock `getuploadurl` 收到原始大小、MD5、目标用户和 AES key，mock CDN 收到非空密文，解密后等于原图字节，最终 mock `sendmessage` 收到 `item_list[].type=2` 与 `image_item.media.encrypt_query_param`。

### human_tests

`human_tests/weixin-provider.md` 覆盖 WebUI 创建 Weixin provider、自动二维码、扫码登录、消息触发 Agent、即时反馈、guide/queue/slash 命令、最终纯正文回写、图片下载和模型多模态输入，以及 Agent 生成图片经 Weixin 独立发送原图。

## Review/Fix/Test 闭环方案

### 第 1 轮

- 复核用户目标、设计文档、代码 diff 和新增测试。
- 运行 Weixin 相关单元测试。
- 运行 `test_weixin_provider_e2e.sh`。
- 修复发现的路径不一致、发送格式、图片解析或 API 状态问题。

### 第 2 轮

- 再次检查 `git status --short`、`git diff`、文档/索引和 human_tests 实际结果。
- 复跑受影响单测和 E2E。
- 执行 rust-project-validate 要求的 fmt、clippy、workspace test。

## 文档更新要求

- 更新本设计文档。
- 新增或更新 `human_tests/weixin-provider.md`。
- 更新 `human_tests/readme.md` 索引。
