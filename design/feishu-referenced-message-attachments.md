# 飞书引用消息附件输入

## 背景

飞书回复消息事件只在当前消息上提供 `parent_id`。Bifrost 已经会通过
`GET /im/v1/messages/{parent_id}` 读取被引用消息的权威文本，但现有读取模型只保存
`msg_type`、文本和原始 JSON，随后把 `images`、`files` 明确置空。因此引用一条纯图片或
纯文件消息时，群 Prompt 会退化成“没有可读取的文本或附件内容”，资源也不会下载到
本地会话目录。

当前消息附件已经具备完整链路：从消息体提取 `image_key` / `file_key`，调用飞书消息资源
接口下载，交给 External CLI，在会话级 `attachments/<run>/images|files` 目录落盘，并把
本地绝对路径写入模型 Prompt。本需求复用该链路，不另建第二套缓存或文件协议。

## 用户目标验证清单

### 必须实现

- 回复引用一条图片消息并 @ 机器人时，自动下载图片并把本地绝对路径交给模型。
- 回复引用一条文件消息并 @ 机器人时，保留原文件名和 MIME，自动下载并把本地绝对路径
  交给模型。
- 引用附件是本轮主要处理对象；如果当前触发消息也携带附件，引用附件排在前面。
- 非终态 Turn 在进程重启后恢复时，不重新读取引用消息正文，但仍能从 SQLite 中已有原始
  内容重建资源 key 并重新下载附件。
- 群 Prompt 正确显示图片与文件的总附件数量，不再只统计图片。

### 必须不破坏

- 当前消息图片/文件下载、单聊附件、Guide/Queue 和空闲 Runner 路径继续工作。
- 资源下载始终使用被引用消息的 `message_id`，不能误用当前触发消息 ID。
- 继续校验引用消息 `chat_id` 与当前群一致，禁止跨群资源读取。
- 引用消息读取失败时仍返回可行动错误；单个附件下载失败、超限或总预算不足时，跳过该
  附件并明确提示用户和模型，但 Turn 与其他附件继续执行，不能让任务或服务整体失败。
- 引用附件的 base64 数据只存在于本轮执行事件中，不写入群消息 SQLite 或事件历史。
- 恢复或预加载事件中携带的 `data_base64` 必须先做解码大小校验；缺失或伪造的 `size_bytes`
  不能绕过 100 MiB 单文件与 250 MiB 总预算，非法 Base64 作为单项失败提示后跳过。
- 每类附件沿用单消息最多 6 个的限制；图片不超过 10 MiB。飞书官方消息资源下载接口只
  支持 100 MB 以内资源，因此文件单项限制为 100 MiB；为控制同一 Turn 的 base64 与落盘
  峰值，本次引用文件总预算为 250 MiB。

## 技术方案

### 统一解析

提取共享的飞书消息附件解析函数，输入 `msg_type + body.content JSON`，输出
`Vec<ImImageAttachment>` 和 `Vec<ImFileAttachment>`。Webhook 事件归一化和
`FeishuFetchedMessage` 都调用同一个函数，避免当前消息与引用消息支持范围漂移。

图片消息读取 `image_key`；文件消息读取 `file_key`、`file_name/name`、
`mime_type/mimeType` 和 `file_size/size/size_bytes`；富文本中的嵌套 `image_key` 继续去重
收集。

### 持久化与恢复

`im_group_messages.content_json` 已保存引用消息原始内容，无需把二进制写入 SQLite。
`ImGroupContextStore` 根据触发消息的 `parent_id` 联表读取引用消息的 `message_id`、
`message_type`、`content_json`，再调用统一解析函数重建资源元数据。这样首次执行和
`prepared/dispatched` Turn 重启恢复使用同一来源，并保持“恢复时不重新读取引用正文”
的既有契约。

### 下载与执行输入

在创建或恢复 Agent Turn 时下载引用资源：

1. 图片调用
   `GET /im/v1/messages/{quoted_message_id}/resources/{image_key}?type=image`；
2. 文件调用
   `GET /im/v1/messages/{quoted_message_id}/resources/{file_key}?type=file`；
3. 校验数量、预加载 Base64 实际大小、单项 100 MiB 平台上限和 250 MiB 总预算后，把响应转成带 `data_base64`
   的临时 `Im*Attachment`；
4. 仅在进入 Runner/Guide/Queue 前，把引用附件前置合并到当前执行事件；
5. 现有 External CLI 保存逻辑把数据写入会话附件目录，并在 `## Attached Images` /
   `## Attached Files` 中输出绝对路径。

单个引用资源下载失败、超过平台上限或超过总预算时，记录非阻塞 notice 并继续处理其余
附件。notice 会立即通过飞书回复用户“附件未加载但任务继续”，也会追加到 Runner Prompt，
防止模型误以为资源可用。发送 notice 本身失败只写日志，不中断 Turn；二进制下载错误也不
向上冒泡为服务级异常。

## 验证方案

- 单元测试：共享解析器覆盖图片、文件、富文本与空 key；消息读取覆盖引用图片/文件字段。
- Store/恢复测试：附件总数为图片+文件；由触发消息重建引用资源；关闭消息读取服务后仍
  能从 SQLite 恢复资源 key。
- 集成测试：mock 飞书消息 API 和资源 API，断言下载请求使用引用消息 ID，并验证下载失败
  会创建并继续 Turn、生成用户/模型 notice，而不是中止任务。
- Shell E2E：扩展群会话黑盒脚本，引用 mock 图片和文件，断言 Runner Prompt 包含
  `Attached Images/Files`、绝对路径、原文件名，且路径内容与 mock 响应一致。
- Human test：在 `human_tests/feishu-group-session.md` 增加引用附件用例并逐条执行。
- 覆盖率：生产 Rust 变更执行 `make coverage-changed`，changed-lines 门禁为 95%；远端 CI
  继续执行 workspace 及各 crate 的 90% 棘轮门禁。

## 飞书官方接口契约

- 读取指定消息：`GET /open-apis/im/v1/messages/{message_id}`。
- 下载消息资源（官方限制仅支持 100 MB 以内，超限错误码 `234037`）：
  `GET /open-apis/im/v1/messages/{message_id}/resources/{file_key}?type=image|file`。
- 引用正文与资源下载可使用 `im:message`、`im:message:readonly`、
  `im:message.history:readonly` 中任一权限。机器人必须与消息在同一会话内，且 `file_key`
  必须属于指定 `message_id`。
- 官方文档：
  - <https://open.feishu.cn/document/server-docs/im-v1/message/get>
  - <https://open.feishu.cn/document/server-docs/im-v1/message/get-2>

注意：以上是“入站引用资源下载”契约。最终结论中的“出站文件发送”走另一个
`POST /open-apis/im/v1/files` 接口，官方单文件上限为 30 MB，不能使用这里的 100/250 MiB
预算；出站限制和失败降级见
`design/feishu-progress-card-collapsed-summary-dark-theme-attachments.md`。
