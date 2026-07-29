# Weixin IM Provider 方案

## 背景

微信接入作为 IM Gateway 原生 `weixin` provider 存在（`crates/bifrost-admin/src/im_gateway/weixin.rs`）。Provider 负责扫码登录、长轮询收消息、把微信消息归一化成 `ImEvent`、触发现有 Agent 路由，并把 Agent 回复回写到微信 ClawBot。所有 `/status`、`/help`、guide、queue、最终回复、图片多模态等能力复用 IM Gateway 与 Agent 现有通道能力。

Provider 底座 iLink：`https://ilinkai.weixin.qq.com`，CDN 底座：`https://novac2c.cdn.weixin.qq.com/c2c`。登录使用 iLink 扫码接口，消息拉取 `getupdates`，回写 `sendmessage`；出站图片走 `getuploadurl` + AES-128-ECB + PKCS7 加密上传 CDN，再以 `image_item.media.encrypt_query_param` 承载。

## 用户目标验证清单

### 必须实现

- IM Gateway 新增 `weixin` provider 类型，用户可在 WebUI Connections 中创建、扫码登录、连接和停用。
- Weixin provider 登录使用 iLink 扫码接口，完成后把 bot token 存入 provider `secret_ref`，API/WebUI 响应仅返回 `secret_configured`，不得泄露 token 或 poll key。
- WebUI 创建或检测到 Weixin provider 未授权/失效时，自动弹出二维码 Modal，每 2 秒 poll `status` 自动检查扫码/授权状态，二维码临近过期或过期时自动刷新，无需用户手动点"已扫码"。
- Weixin provider 使用 `POST /ilink/bot/getupdates` 轮询消息并归一化为 `message.receive` 事件。
- 微信 ClawBot 普通消息触发默认 Agent chat 链路，向发送方立即回写一条纯文本即时反馈（Weixin 没有 Feishu CardKit 进度卡）。
- Weixin `send_text` 走官方 `sendmessage` 结构，payload 使用 `msg + base_info`，`msg.item_list[].text_item.text` 承载文本；`send_card` 只把卡片正文降级为文本后调 `send_text`。
- Weixin 入站消息不使用 owner 过滤；`owner_open_id` 只记录扫码登录账号。Feishu provider 继续保持 owner 安全边界。
- 微信通道支持所有 Agent `/xxx` 命令，行为与飞书 IM 通道一致。
- 微信图片消息解析 `image_item/media`，下载图片，必要时按 AES-128-ECB + PKCS7 解密，并以 base64 data URL 送入模型多模态输入。
- Agent / ChatGPT Web 生成的本地图片按原图通过微信通道独立发送为图片消息，不能只退化为 Markdown 文本或卡片正文。
- 最终回复直接发送模型最终文本，不添加 `Bifrost AI`、`等待任务说明` 等卡片前缀。
- 当 Weixin `sendmessage` 对长文本返回失败（`ret` 或 `errcode` 非零，或网络错误）时，必须按字符边界拆分成多条 `[i/N]` 前缀的小文本按序补发，不能丢弃最终回复。切分保留全部 Unicode 内容和换行。

### 必须不破坏

- 不复用或污染 `~/.openclaw`。
- 不打印 token、context token、cookie 或二维码内部 poll key。
- 不启动系统代理；相关 E2E 必须使用 `--no-system-proxy`。
- Feishu owner 过滤、卡片进度、回复目标逻辑保持原有安全边界。
- 现有 Agent 扩展系统保持原样，微信通道只出现在 IM Gateway provider 管理入口。

### 必须真实验证

- 单元测试覆盖 Weixin 登录、事件归一化、`sendmessage` 错误识别、官方消息格式、回复目标、图片解析、图片下载与 AES-128-ECB 解密。
- E2E `test_weixin_provider_e2e.sh` 使用 iLink 兼容 mock 验证扫码开始、自动状态检查、扫码完成、连接轮询、事件入库、图片附件归一化、`sendmessage` 回写、图片出站原图上传 CDN。
- `test_im_gateway_long_reply_delivery_regression.sh` 覆盖长文本首次失败后拆分补发回归。
- human_tests 必须启动真实 Bifrost，让用户在 WebUI 创建/启用 Weixin IM provider，扫码后从微信 ClawBot 发消息，由 Agent 观察日志、事件、消息记录、图片下载和回复。

## 产品语义

### Provider 配置

`ImProviderType` 增加 `Weixin`。扫码完成后 provider 持久化 `app_id`、`owner_open_id`、`base_url`、`secret_ref`、`event_connection_enabled` 和 `event_types`。其中 `secret_ref` 是 iLink bot token，只本地保存，API 返回仅暴露 `secret_configured`。

### 登录 API

Provider 级 API 由 `crates/bifrost-admin/src/handlers/im_gateway/providers.rs` 提供：

```text
POST /api/im-gateway/providers/:id/weixin-login/start
GET  /api/im-gateway/providers/:id/weixin-login/status
POST /api/im-gateway/providers/:id/weixin-login/complete
```

- `start`：调 `GET /ilink/bot/get_bot_qrcode?bot_type=3`，只返回可扫码的 `scan_url` 和 `expires_in_seconds`，不返回 poll key。WebUI 创建 Weixin provider 后立即调 `start` 并弹出 Modal。
- `status`：每 2 秒 poll，看到 confirmed/authorized 后在服务端完成 provider 更新，返回脱敏 provider/account 摘要；未授权时返回 pending 状态。
- `complete`：登录状态确认后清理 `weixin_login_pending` 并回写 provider `secret_ref`。

Modal 每 2 秒 poll 一次；二维码临近过期或已过期时 WebUI 自动重新调用 `start` 刷新，用户无需手动"已扫码"。

iLink 的 `get_qrcode_status` 本身是长轮询，请求在未扫码时可能约 35 秒才返回 `wait`。登录 API 使用独立的 75 秒 HTTP timeout，必须长于二维码 60 秒生命周期；普通发消息、下载等请求仍保留 30 秒 timeout，避免登录长轮询扩大其他接口的故障等待时间。

### 事件连接

`ImConnectionManager` 持有 `WeixinProvider`。`connect_events` 以固定 `DEFAULT_POLL_INTERVAL_MS = 3_000` ms 间隔调用 `POST /ilink/bot/getupdates`，认证头为：

```
AuthorizationType: ilink_bot_token
Authorization:     Bearer <bot_token>
```

返回消息归一化为 `ImEvent`：`provider_type = weixin`、`event_type = message.receive`、`source.user_id/source.chat_id = from_user_id`、`source.message_id = message_id`、`message.text` 优先读取 `text/content/msg/message`，并兼容真实 iLink `msgs` + `sync_buf` + `item_list[].text_item.text` 结构。数字型 `message_id` 会转为字符串；缺失 ID 时使用 raw JSON 稳定 hash，避免 timestamp 造成重复事件不可去重。

引用消息从 `item_list[].ref_msg.message_item` 归一化到 `message.reply_to`，保留被引用消息的 `msg_id/message_id`、`create_time_ms` 和上游可能内联返回的 `text_item.text`。如果 iLink 只返回 ID 和时间，Agent 入口会先按同 provider 的真实消息 ID 查找本地消息记录，再按同一聊天对象与最近发送时间回退匹配；找到后把引用原文/链接与当前问题分区传给 Runner。引用内容按字符限制长度，找不到原文时仍继续处理当前消息。

Feishu 入站继续按 `owner_open_id` 过滤非 owner 消息；Weixin 入站不做 owner 过滤，避免把普通微信发送方误拦截。

### 回复回写

`ImProviderClient` 抽象 Feishu/Weixin 发送能力。Weixin 的 `send_text` 调 `POST /ilink/bot/sendmessage`，payload 使用官方 `msg + base_info` 结构，`msg.item_list[].text_item.text` 承载文本；`send_card` 只把卡片正文降级为文本后调 `send_text`。poll 到消息时保存 `context_token`，后续向同一用户发送时带上该 token。

`send_text` 保持"先按原始完整文本发送"的默认路径；只有原始 `sendmessage` 返回网络错误或非零 `ret/errcode` 时，才按字符边界把文本切成多条小消息并追加 `[i/N]` 顺序前缀逐条重试。切分限制：`TEXT_RETRY_CHUNK_MAX_CHARS = 1_000` chars，`TEXT_RETRY_CHUNK_MAX_BYTES = 3_000` bytes。切分必须保留全部 Unicode 内容和换行，任一补发分片失败时返回包含原始错误与分片序号的错误，便于从消息日志定位失败点。

Agent 回复目标按 provider 区分：Weixin 优先回写本次入站事件的 `source.chat_id/source.user_id`，不会优先发给 `owner_open_id`；Feishu 继续优先发给 owner。消息日志记录真实目标 ID，避免只显示内部 `__agent_reply__` 占位符。

Weixin 没有 Feishu CardKit 进度卡能力，因此默认 Agent chat 进入真实 turn loop 前会先向消息发送方发送一条纯文本即时反馈。运行中再次收到同一会话的普通消息时默认进入 FIFO queue，并立即回执排队状态；只有显式 `/g` 才尝试注入当前 turn。`/status`、`/stop`、session-free 命令以及其余 Agent slash 命令都走和 Feishu 相同的 IM 事件入口。

### 入站图片

图片消息从 `item_list` 中 `type=2` / `image_item` 提取为 `ImImageAttachment`，保留 `media.full_url`、`media.encrypt_query_param` 和 AES key。图片-only 消息不再把 raw JSON 当文本，而是使用默认图片理解提示进入 Agent 多模态路径。

图片下载走 `ImProviderClient::download_message_image_resource`：优先使用服务端返回的 `full_url`，没有时使用 `https://novac2c.cdn.weixin.qq.com/c2c/download?encrypted_query_param=...` 作为 CDN fallback；如果 `image_item.aeskey` 或 `media.aes_key` 存在，则按 OpenClaw Weixin 协议使用 AES-128-ECB + PKCS7 解密后，再以 base64 data URL 形式传给模型。

### 出站图片

出站图片走同一 `ImProviderClient` 抽象。Agent 回复解析到本地 Markdown 图片（例如 `![cat](./cat.png)`）时，先从文本回复中移除本地图片引用，再逐张调用 provider `upload_image/send_image` 独立发送原图。Weixin 的 `upload_image` 在进程内保存待发送原始字节；`send_image` 拿到目标用户后按 OpenClaw Weixin 协议调用 `POST /ilink/bot/getuploadurl`，使用随机 16 字节 AES key 执行 AES-128-ECB + PKCS7 加密，把密文上传 CDN，并用返回的 `x-encrypted-param` 构造 `sendmessage` 的 `image_item.media`。消息日志必须记录 `msg_type=image`、目标用户、发送状态和错误，但不能记录图片原始字节。

## 技术细节

### 关键常量（`weixin.rs`）

```
DEFAULT_BASE_URL                = "https://ilinkai.weixin.qq.com"
DEFAULT_CDN_BASE_URL            = "https://novac2c.cdn.weixin.qq.com/c2c"
DEFAULT_POLL_INTERVAL_MS        = 3_000
DEFAULT_HTTP_TIMEOUT            = 30s
LOGIN_HTTP_TIMEOUT              = 75s
LOGIN_QR_EXPIRES_IN_SECONDS     = 60
TEXT_RETRY_CHUNK_MAX_CHARS      = 1_000
TEXT_RETRY_CHUNK_MAX_BYTES      = 3_000
```

### 关键方法

- `WeixinProvider::start_login(base_url)`
- `WeixinProvider::complete_login(...)`
- `WeixinProvider::normalize_login_account(...)`
- `WeixinProvider::send_text(target, text, context_token)`（首发全量 → 失败后 `[i/N]` 拆分补发）
- `WeixinProvider::download_message_image_resource(...)`
- `WeixinProvider::send_image(target, image_key)`
- `service.weixin_login_pending`：短周期存储扫码 pending 状态。

## CLI + Web + Admin API

### Admin API

- `POST /api/im-gateway/providers/:id/weixin-login/start`
- `GET  /api/im-gateway/providers/:id/weixin-login/status`
- `POST /api/im-gateway/providers/:id/weixin-login/complete`
- `POST /api/im-gateway/providers/:id/connect`
- `POST /api/im-gateway/providers/:id/disconnect`
- `GET/DELETE /api/im-gateway/providers/:id/messages`

Provider 秘密不在响应体里出现；接口只返回 `secret_configured`。

### Web

- `web/src/pages/Settings/tabs/ImGatewayTab.tsx`：Weixin provider 创建/编辑面板，含扫码 Modal 与自动 poll。
- `web/src/api/imGateway.ts`：登录/连接/消息 API 封装。

### CLI

无 Weixin 专属子命令；通过 Admin API + WebUI 完成扫码。CLI 侧只暴露通用的 `bifrost im-gateway providers` 列表（若已存在）。

## Sync 边界

- Provider 秘密（bot token）仅本地保存，不参与多设备 sync。
- 消息日志本地 SQLite 存储，不参与 sync。
- `owner_open_id`、`app_id` 等元数据可随 provider 配置备份，但导出/导入时必须清空 `secret_ref`。
- 与 Feishu owner 安全边界不共享：Weixin 侧显式跳过 owner 过滤，任何跨 provider 逻辑必须显式判断 `provider_type`。

## Phase 拆分

### Phase 1：登录与 provider 骨架

- `ImProviderType::Weixin`、provider 配置、`secret_ref` 存储。
- Admin API `weixin-login/start|status|complete`。
- WebUI 扫码 Modal + 自动 poll + 二维码过期自动刷新。

### Phase 2：事件与文本回写

- `connect_events` 拉 `getupdates`。
- 归一化 `msgs/sync_buf/item_list` 到 `ImEvent`。
- `send_text` 官方 `msg + base_info` 结构。
- 长文本失败拆分补发。
- Agent slash 命令、guide/queue、即时反馈。

### Phase 3：图片多模态

- 入站图片解析 + AES-128-ECB 解密 + base64 data URL。
- 出站 Agent 生成图片走 `upload_image` + `send_image`。
- `getuploadurl` + AES 加密上传 CDN + `image_item.media.encrypt_query_param` 构造。

### Phase 4：E2E 与文档

- `test_weixin_provider_e2e.sh` + `test_im_gateway_long_reply_delivery_regression.sh` 稳定。
- `human_tests/weixin-provider.md` 与 `human_tests/readme.md` 索引同步。
- 更新 `im-gateway.md` 描述新增 provider 类型。

## 测试方案

### 单元测试（`crates/bifrost-admin/src/im_gateway/weixin_tests.rs`）

- `normalize_login_account_uses_ilink_fields_and_redirected_base_url`
- `normalize_update_converts_weixin_message_to_im_event`
- `normalize_update_extracts_ilink_item_list_text_and_numeric_message_id`
- `normalize_update_extracts_ilink_image_item_for_multimodal_agent`
- `decrypt_aes_128_ecb_accepts_base64_raw_key`
- `download_message_image_resource_fetches_plain_full_url`
- `send_image_uploads_original_bytes_to_cdn_and_sends_image_item`
- `send_image_returns_config_error_for_unknown_image_key`
- `send_error_message_detects_non_zero_ilink_code`
- `send_error_message_detects_non_zero_ilink_ret`
- `split_text_for_retry_preserves_multibyte_content`
- `im_long_reply_delivery_send_text_retries_failed_long_message_as_split_messages`
- `card_to_text_omits_header_when_body_has_content`
- `validate_config_requires_completed_qr_login`

### E2E 测试

`e2e-tests/tests/test_weixin_provider_e2e.sh`：

1. 构建真实 `bifrost`。
2. 使用临时 `BIFROST_DATA_DIR` 启动真实进程，显式 `--no-system-proxy`。
3. iLink 兼容 mock 验证 provider 级 `login start/status/complete`，断言响应不泄露 poll key 或 token。
4. 连接 provider，mock `getupdates` 返回真实形态 `msgs/sync_buf/item_list` 文本和图片消息，断言 `history/events` 出现 `provider_type == weixin` 的 `message.receive`。
5. 断言图片事件包含 `message.images[0].download_url` 和 file key。
6. 调用 `messages/send`，断言 mock `sendmessage` 收到目标用户、文本、`context_token` 与官方 `base_info`。
7. 调用 Weixin provider 图片发送路径，断言 mock `getuploadurl` 收到原始大小、MD5、目标用户和 AES key，mock CDN 收到非空密文，解密后等于原图字节，最终 mock `sendmessage` 收到 `item_list[].type=2` 与 `image_item.media.encrypt_query_param`。
8. 删除 provider 后确认 `getupdates` 停止（`UPDATES_BEFORE_DELETE` vs `UPDATES_AFTER_DELETE`）。

`e2e-tests/tests/test_im_gateway_long_reply_delivery_regression.sh` 覆盖长回复回归：ChatGPT Web DOM 提取不截断保护、Weixin 长文本首次失败后拆分补发。

### human_tests

`human_tests/weixin-provider.md` 覆盖 WebUI 创建 Weixin provider、自动二维码、扫码登录、消息触发 Agent、即时反馈、guide/queue/slash 命令、最终纯正文回写、图片下载和模型多模态输入，以及 Agent 生成图片经 Weixin 独立发送原图。启动方式：临时 `BIFROST_DATA_DIR`、非 9900 端口、`--no-system-proxy`、`BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT=1`。

## Review/Fix/Test 闭环

### 第 1 轮

- 复核用户目标、设计文档、代码 diff 和新增测试。
- 运行 Weixin 相关单元测试（`cargo test -p bifrost-admin im_gateway::weixin`）。
- 运行 `test_weixin_provider_e2e.sh`。
- 修复发现的路径不一致、发送格式、图片解析或 API 状态问题。

### 第 2 轮

- 再次检查 `git status --short`、`git diff`、文档/索引与 human_tests 实际结果。
- 复跑受影响单测和 E2E，含长回复回归。
- 执行 `cargo fmt --all -- --check` / `cargo clippy --workspace --all-targets --all-features -- -D warnings` / `cargo test --workspace --all-features`。

## 风险与决策点

- iLink 官方 `sendmessage` 结构对 payload 严格校验，任何字段（`base_info.channel_version` 等）缺失都会拒绝；mock 与真实服务需保持字段同步。
- AES-128-ECB + PKCS7 是 OpenClaw Weixin 协议约定，切换到 GCM/CBC 会破坏历史消息解密，只能在协议侧统一升级后再改。
- 长文本拆分策略以字符边界为主：如果未来收到超长 emoji ZWJ 序列可能被切分，需要评估是否再引入 grapheme cluster 边界。
- 微信通道不做 owner 过滤是有意为之：普通 ClawBot 群聊里所有 sender 都合法。若产品未来要求限定 sender 集合，需要在 provider 配置里新增白名单，而不是复用 Feishu owner 语义。
- 出站图片按原图独立发送，未做压缩；对超大图（>10MB）需要评估是否引入本地压缩，避免 CDN 上传超时。
