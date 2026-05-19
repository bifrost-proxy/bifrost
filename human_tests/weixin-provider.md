# Weixin IM Provider 真实场景测试

## 功能模块说明

本文档验证 Bifrost 的原生 `weixin` IM provider：WebUI 创建连接、自动二维码登录、连接状态、微信消息触发 Agent、即时反馈/guide/queue/slash 命令、最终回复回写、微信图片下载后传给多模态模型，以及 Agent 生成图片通过微信独立发送原图。

对应设计文档：`design/weixin-provider.md`

## 前置条件

- 当前仓库位于 Bifrost 工程根目录。
- 使用临时 `BIFROST_DATA_DIR`，避免污染本机 `~/.bifrost`。
- 启动 Bifrost 时必须携带 `--no-system-proxy`。
- 已构建或即将构建最新 `bifrost` 二进制。
- 如执行真实微信用例，需要用户使用微信扫描 WebUI 展示的二维码。

## 测试用例列表

### TC-WIP-01：Provider 扫码登录 API 不泄露密钥

**操作步骤：**

1. 使用 E2E mock 或 WebUI 创建 `provider_type=weixin` 的 provider。
2. 调用：
   ```bash
   curl -fsS -X POST "http://127.0.0.1:<port>/_bifrost/api/im-gateway/providers/<id>/weixin-login/start" -H 'Content-Type: application/json' -d '{}'
   curl -fsS "http://127.0.0.1:<port>/_bifrost/api/im-gateway/providers/<id>/weixin-login/status"
   curl -fsS -X POST "http://127.0.0.1:<port>/_bifrost/api/im-gateway/providers/<id>/weixin-login/complete" -H 'Content-Type: application/json' -d '{}'
   ```

**预期结果：**

- `start` 返回 `scan_url` 和过期时间，不返回二维码 poll key。
- `status` 自动检查扫码状态，不要求用户手动声明已扫码。
- `complete/status` 返回的 provider 只展示 `secret_configured`，不泄露 `secret_ref` 或 token。

**实际执行结果：**

- 已执行通过：同一次 E2E 调用真实 provider API，`start` 返回 `scan_url` 和 `expires_in_seconds`，`status` 自动检查到 `confirmed` 并返回脱敏 provider/account；响应未包含 poll key、`secret_ref` 或 token。

### TC-WIP-02：WebUI 创建 Weixin provider 后自动弹出二维码并自动刷新

**操作步骤：**

1. 启动真实 Bifrost：
   ```bash
   BIFROST_DATA_DIR="<tmp>" target/debug/bifrost start --host 127.0.0.1 -p 18938 --unsafe-ssl --no-system-proxy
   ```
2. 打开 `http://127.0.0.1:18938/_bifrost/settings?tab=im-gateway&imGatewaySection=connections`。
3. 创建 `weixin` provider。
4. 观察二维码弹窗、状态文案和过期刷新。

**预期结果：**

- 创建后自动弹出二维码。
- UI 自动轮询授权状态。
- 二维码过期或 provider 失效时，UI 展示 expired/失效状态，并自动刷新二维码。
- 用户不需要手动检查“是否已扫码/是否授权成功”。

**实际执行结果：**

- 已执行过真实 WebUI 链路并修复：创建后二维码自动弹出，过期后 UI 展示过期并刷新，授权成功后 provider 自动连接。

### TC-WIP-03：微信文本消息触发 Agent 并回写给原发送方

**操作步骤：**

1. 使用已扫码授权的 `weixin` provider 启动真实 Bifrost：
   ```bash
   BIFROST_DATA_DIR="<tmp>" RUST_LOG=bifrost_admin=debug,bifrost_agent=info,info target/debug/bifrost start -p 18938 --unsafe-ssl --no-system-proxy
   ```
2. 从微信 ClawBot 向该 bot 发送一条普通消息。
3. 观察 Bifrost 日志、`history/events` 和消息日志。

**预期结果：**

- 收到微信消息后立即有 outbound 记录，内容为开始处理反馈。
- Agent loop 完成后给原微信发送方回写最终回复，`target_id` 是本次入站 `source.chat_id/source.user_id`。
- 最终回复正文不带 `Bifrost AI`、`等待任务说明` 或其它卡片标题前缀。
- Weixin 入站不受 owner 过滤器限制。

**实际执行结果：**

- 已执行真实链路：2026-05-12 09:48:14 收到微信文本事件 `7459902249228129800`，09:48:16 回写 `已开始处理。`，09:48:25 最终回复发送到 `o9cq80wqfvOh3cJ69ywGqu9cGdqM@im.wechat`，最终回复无 `Bifrost AI` 前缀。

### TC-WIP-04：运行中追加消息进入 guide/queue，slash 命令即时响应

**操作步骤：**

1. 在 Agent turn 执行期间，从微信 ClawBot 继续发送普通消息。
2. 再发送 `/status`、`/help` 等命令。
3. 观察日志和微信回执。

**预期结果：**

- 追加普通消息立即回执“已注入引导消息”或排队状态。
- `/status`、`/help` 等命令即时返回，不进入错误的普通聊天分支。
- 能力与飞书 IM 通道一致。

**实际执行结果：**

- 已执行真实 guide 链路：2026-05-12 09:48:19 运行中追加消息后日志出现 `guide message injected via IM`，09:48:20 微信发送成功。`/status` 仍需在最终回归中单独复测。

### TC-WIP-05：Provider E2E 解析文本和图片事件

**操作步骤：**

1. 执行：
   ```bash
   bash e2e-tests/tests/test_weixin_provider_e2e.sh
   ```

**预期结果：**

- mock `getupdates` 文本事件进入 `history/events`，`provider_type == weixin`。
- mock 图片事件进入 `history/events`，`message.images[0].file_key == mock-image-1`。
- 图片附件 `download_url` 指向 mock CDN。
- `sendmessage` 使用官方 `msg + base_info` 结构，并携带保存的 `context_token`。

**实际执行结果：**

- 已执行通过：同一次 E2E 中 mock `getupdates` 投递文本和图片事件，`history/events` 中分别出现 `7459890488013826184` 文本事件和 `7459890488013826185` 图片事件；图片附件 `file_key == mock-image-1` 且 `download_url` 指向 mock CDN；`messages/send` 命中 mock `sendmessage`，payload 包含 `msg + base_info`、目标用户、文本、`context_token == ctx-1` 和 `channel_version == 1.0.3`。

### TC-WIP-06：History Events 表格窄宽度展示

**操作步骤：**

1. 打开 `http://127.0.0.1:18938/_bifrost/settings?tab=im-gateway&imGatewaySection=history`。
2. 将窗口宽度缩小到约 1200px。
3. 查看 Events 表格，观察 Event ID、Source、Message 三列。
4. 鼠标悬停在被省略的 Source 或 Message 上。

**预期结果：**

- History 内容区域从顶部开始展示，不在剩余空间中垂直居中。
- Events 表格在宽度不足时出现横向滚动，不挤压到列内容断行。
- Event ID、Source、Message 均为单行省略。
- Source 或 Message 过长时，hover 展示完整 Tooltip。

**实际执行结果：**

- 已执行通过：启动 rebase 后最新构建的 Bifrost `127.0.0.1:18938`，在 744x980 窄视口打开 History 页面；Events 表格 `scrollWidth=1390`、`clientWidth=662`、`overflowX=auto`，可横向滚动到 Message 和 Time 列；History 区域 `top=116`，表格 `top=218`，未垂直居中。Message 长 JSON 单元格单行省略，hover 后出现 `.im-gateway-overflow-tooltip`，tooltip 内容包含完整 `mmassistant_bypmsg_inbox...` client_id。

### TC-WIP-07：微信图片消息下载并传给多模态模型

**操作步骤：**

1. 使用已扫码授权的 `weixin` provider 启动真实 Bifrost。
2. 从微信 ClawBot 向 bot 发送一张图片，可以附带一句 `请描述这张图`。
3. 观察 Bifrost 日志和消息历史。
4. 等待 Agent 回复完成。

**预期结果：**

- 入站事件 `message.images` 至少包含 1 个附件，附件来自 `image_item.media.full_url` 或 `image_item.media.encrypt_query_param`。
- Bifrost 日志出现 `downloaded IM image resource for agent multimodal input`，包含 `mime_type=image/...` 和非 0 `byte_len`。
- Agent 请求包含多模态图片输入，模型按图片内容回复，而不是只看到 raw JSON。
- 图片-only 消息使用默认图片理解提示，不把整段 iLink raw JSON 当作用户文本。

**实际执行结果：**

- 已执行真实链路：2026-05-12 10:09:35 收到微信图片事件 `7459907566129128968`，日志显示 `image_count=1`；10:09:36 下载图片成功，`mime_type=image/png`、`byte_len=230451`；随后 Agent 发起多模态模型请求并完成回复。10:09:45 第二张图片事件 `7459907662434641032` 也完成下载与 Agent turn。

### TC-WIP-08：Agent 生成图片通过 Weixin 独立发送原图

**操作步骤：**

1. 使用已扫码授权的 `weixin` provider，确保 Agent runner 指向 ChatGPT Web 或其他会生成本地图片文件并返回 Markdown 图片引用的 runner。
2. 从微信 ClawBot 发送：`帮我生成4张可爱的小猫咪`。
3. 等待 Agent 最终回复完成，观察 Bifrost 日志、消息日志和微信侧收到的消息。
4. 如无真实微信环境，使用 Weixin mock 服务执行 provider 图片发送子路径，记录 mock `getuploadurl`、CDN upload 和 `sendmessage` 请求。

**预期结果：**

- Agent 最终文本回复不暴露 `./*.png`、`file://` 或本机绝对路径。
- Bifrost 从回复 Markdown 中解析出 4 张本地图片，逐张上传并发送。
- Weixin 图片发送使用 `getuploadurl` 获取上传参数，上传内容为原图字节经 AES-128-ECB + PKCS7 加密后的密文；解密后与原图字节一致。
- 最终 `sendmessage` payload 中每张图片均为 `item_list[0].type == 2`，并包含 `image_item.media.encrypt_query_param`、`image_item.media.aes_key` 和 `image_item.aeskey`。
- Message History 中每张图片都有一条 outbound 记录，`msg_type=image`、`status=success`；如果 provider 返回错误，记录明确错误，不影响 active run 释放。

**实际执行结果：**

- 已执行代码级 mock 回归：`cargo test -p bifrost-admin weixin::weixin_tests::send_image_uploads_original_bytes_to_cdn_and_sends_image_item -- --nocapture` 通过。测试中原始字节 `original-cat-bytes` 先经 Weixin provider 保存，再在 `send_image` 时调用 mock `getuploadurl`；mock CDN 收到非空密文，使用 payload 中的 AES key 解密后等于原图字节；最终 mock `sendmessage` 收到 `item_list[0].type == 2` 和 `image_item.media.encrypt_query_param == download-param`。
- 已执行未知 key 回归：`cargo test -p bifrost-admin weixin::weixin_tests::send_image_returns_config_error_for_unknown_image_key -- --nocapture` 通过，确认不会发送缺失图片内容的空图片消息。

## 清理步骤

- 停止 E2E 或真实验证启动的 Bifrost 进程。
- 删除临时 `BIFROST_DATA_DIR`。
