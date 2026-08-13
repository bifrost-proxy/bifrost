# IM Agent 回复文件路径解析

## 背景

Agent 最终回复已经能够把 Markdown 图片和普通 Markdown 文件链接转换为 IM 原生图片或
文件消息，但发现逻辑仍依赖 `![...](...)` / `[...] (...)` 语法。Codex、Trae 和其他
Runner 经常直接输出绝对路径、相对工作目录路径、反引号路径或带源码位置的路径；文件
真实存在时，旧实现仍可能漏发。这个缺口同时影响飞书与微信，不能在某个 Provider 的
渲染器中单独修补。

## 目标

- 从 Agent 回复中发现 Markdown 图片、Markdown 普通链接、裸绝对路径、`./` / `../`
  相对路径、工作目录相对路径，以及被反引号、引号或尖括号包裹的路径。
- 路径必须解析到真实普通文件，并匹配既有图片或附件扩展名白名单，才进入发送队列。
- 复用一个扩展名与安全规则来源，覆盖图片、文档、配置、Office、压缩包、patch/diff、
  音频和视频，不把实现限制在图片。
- 提取结果保持 Provider 中立。飞书继续使用 image/file API；微信继续走 iLink CDN
  加密上传与原生 image/file/media 消息。
- 保留现有源码文件和敏感配置拒绝规则、大小限制、失败通知与 fenced code block 隔离。

## 非目标

- 不扫描回复中未提及的目录，不根据目录内容猜测附件。
- 不放开源码或 `.env*`、credentials、secrets、私钥等敏感文件。
- 不改变远程 URL 的下载白名单和下载行为。
- 不把 SVG 伪装成飞书图片上传。SVG 有同名 PNG 时可使用 PNG 作为图片预览；否则作为
  普通文件发送。

## 设计

### 共享解析阶段

`agent_reply_attachments` 负责从 fenced code block 之外的文本收集候选 token：

1. Markdown 图片和普通链接；
2. 反引号、单/双引号、尖括号包裹的路径；
3. 裸绝对路径、`file://`、`./`、`../` 和能在 Runner 工作目录中命中的相对路径。

候选先移除 Markdown title、终止标点和最多两段 `:line[:column]` 源码位置，再按以下顺序
解析：

1. `file://` 或绝对路径直接解析；
2. 相对路径以当前 Runner `work_dir` 解析；
3. 调用方未提供 `work_dir` 时，沿用 Provider agent work dir fallback。

解析后要求 `metadata.is_file()`，使用 canonical path 去重；缺失、目录、未知扩展名、源码
和敏感配置均忽略。为保持已有 Markdown 链接兼容性，标签明确包含“附件 / 下载 / file /
attachment”时仍沿用原有显式附件规则；这个兼容规则不适用于新支持的裸路径。解析器不读取
文件正文，上传阶段继续执行现有大小与空文件门禁。

### 分类与文本处理

- PNG/JPEG/GIF/WEBP/BMP 进入 `AgentReplyLocalImage`。
- SVG 若存在同 stem 的 `.png`，PNG 进入图片队列且 SVG 仍按原始链接语义决定是否作为
  文件发送；裸 SVG 默认只发送 SVG 文件，避免产生未承诺的额外附件。
- 其他既有允许扩展进入 `AgentReplyLocalAttachment`，MIME 继续由 `mime_guess` 推断。
- 同一 canonical path 无论由 Markdown、裸路径或不同包裹形式出现多少次，只发送一次。
- 已解析的 Markdown 图片沿用现有正文剥离；普通文件路径和裸路径保留在正文中，作为
  用户可见的来源说明，不因发送附件而破坏总结结构。

### Provider 投递

解析器只产生图片和文件两个 Provider 中立队列：

- Feishu：图片走 `upload_image/send_image`，文件走 `upload_file/send_file`；已在卡片内
  转成 `image_key` 的显式 Markdown 图片不重复发送。
- Weixin：相同队列由 `ImProviderClient` 路由到现有 iLink CDN 上传、加密和原生消息发送。
- 其他 Provider 沿用 capability/unsupported 行为；单个附件失败不改变终态任务结论。

## 安全与兼容性

- 只处理回复文本显式出现的候选，不做递归、glob 或目录遍历。
- fenced code block 中的示例路径不处理，避免文档/日志示例触发意外发送。
- 源码与敏感文件 deny rule 优先于扩展名和“附件/下载/file”等标签。
- 远程 URL 与 Feishu `img_*` key 不进入本地路径解析。
- 路径解析与投递分离，因此飞书、微信共享发现结果，但各自保持原生协议行为。

## 验证

- 单元测试覆盖原始 case 的 MD/SVG/PNG、裸绝对和相对路径、包裹形式、源码位置、所有
  扩展类型族、缺失文件/目录/重复路径、源码与敏感文件拒绝、code fence 隔离。
- 飞书 E2E 断言终态回复发现普通链接和裸路径后产生正确 image/file 请求。
- 微信 E2E 断言同样的回复文本进入原生 CDN image/file 上传与 sendmessage 流程。
- human tests 分别通过隔离 Bifrost Service、mock Runner 与 loopback Feishu/Weixin 协议端点，
  核对文本终态、图片和多类型文件消息；不把本地协议验证表述为真实租户投递。
- 生产 Rust 变更执行 `make coverage-changed`，并由远端 CI 执行 workspace coverage gate。
