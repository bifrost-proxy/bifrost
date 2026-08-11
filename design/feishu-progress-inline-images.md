# 飞书过程卡与终态卡内联图片

## 背景

飞书 Card 2.0 的 Markdown 图片语法只接受通过飞书图片上传接口获得的
`image_key`：`![说明](image_key)`。普通 HTTP URL 会被现有 Markdown 转换器降级为
文字链接，本地路径则无法被飞书客户端访问。

当前最终回复链路能够发现本地或远程 Markdown 图片、上传到飞书，但会先从卡片正文
移除图片，再补发独立图片消息；进度卡链路只执行同步 Markdown 转换，不负责下载或
上传。因此过程卡、原任务卡的最终结论和独立终态卡没有一致的内联图片语义。

## 用户目标验证清单

### 必须实现

- 运行中的公开 assistant 输出出现完整 Markdown 图片引用后，上传到飞书并在过程卡原位渲染。
- 原任务卡收尾后的“最终结论/失败结论”和独立终态卡复用同一份已解析 Markdown，图片均在卡内原位渲染。
- 本地相对路径按本次 Runner 的真实工作目录解析；HTTP/HTTPS Markdown 图片先受控下载，再上传飞书。
- 同一张图片在同一 provider 下复用上传结果，终态不再补发重复的独立图片消息。
- 飞书上传失败时不阻断进度更新或任务收尾，在原位置展示明确的上传失败降级文案。

### 必须不破坏

- 已经是 `img_*` 的飞书图片 key 原样保留，不重复上传。
- fenced code block 中的 Markdown 图片示例不解析、不上传。
- 非飞书 provider 继续沿用既有独立图片消息行为。
- 最终回复中的普通文件、压缩包等非图片附件继续上传并发送。
- 卡片字节/组件预算、streaming 设置、稳定 element ID、终态引用链和亮暗主题语义保持不变。
- 图片网络和文件 IO 不在 progress session mutex 持有期间执行，避免阻塞普通进度刷新。

### 必须真实验证

- 单元测试覆盖本地图片、远程图片、现有 image_key、代码块、上传失败、缓存复用和事件预处理。
- mock Feishu HTTP 集成测试断言 `/im/v1/images` multipart 上传后，CardKit 更新载荷包含返回的 `image_key`。
- shell E2E 断言原任务终态卡和独立终态卡都内联同一个 `image_key`，且不再发送 `msg_type=image` 重复消息。
- `human_tests/feishu-progress-card.md` 增加过程和终态内联图片用例并逐条执行。

## 技术方案

### 统一图片解析器

在 IM Gateway 提供共享的异步飞书 Markdown 图片解析函数。解析器跳过 fenced code block，
逐个识别 `![alt](destination)`：

1. `img_*`：原样保留。
2. 本地路径或 `file://`：按 Runner work dir 解析，校验普通文件和 10 MiB 上限，上传为
   `image_type=message`。
3. HTTP/HTTPS：使用统一出站客户端下载，校验成功状态、图片 Content-Type、非空内容，
   并按响应分片执行 10 MiB 硬上限，避免未知或伪造 Content-Length 的超大响应被完整载入
   内存，再走同一上传缓存。
4. 失败：替换为 `[alt 未能上传]`，记录 provider、来源和错误，但不暴露本地绝对路径到卡片。

上传成功后改写成 `![alt](img_v3_xxx)`，再交给现有
`convert_to_feishu_markdown`。本地缓存使用 provider + canonical path + size + mtime 指纹，
远程缓存使用 provider + URL + 内容摘要，保证同一图片复用上传结果；进程内缓存最多保留
256 项，避免长期运行时无界增长。

### 过程卡接入

`FeishuProgressCardSession` 保存本轮图片基准目录。Registry 在获取 session context 后先释放
mutex，再异步预处理 `AssistantDelta`、`AssistantFinal`、`TurnFinished` 和 `ProposedPlan`
中的完整 Markdown 图片，最后重新加锁按原顺序合并事件和刷新卡片。

逐字符流中尚未闭合的图片语法保持原文本；每批事件先合并进快照，再解析累计后的完整
Markdown，因此即使 `![alt](path)` 被拆到多个 `AssistantDelta`，也会在闭合后完成上传和
改写。这样不会为了每个字符发请求，也不会在锁内等待网络。

### 终态双卡接入

外部 Runner 结束时，在 `progress_registry.finish` 之前把最终文本解析一次：

- 解析后的文本更新原进度卡最终结论；
- 同一文本发送独立成功/失败终态卡；
- 因图片语法已变成 `img_*`，后续附件提取不会再生成独立图片消息；普通文件附件不受影响。

普通非 progress 的飞书 Agent reply 同样直接使用统一解析器，避免继续产生“卡片无图 +
独立图片消息”的不一致体验。非飞书通道维持旧行为。

## 风险与降级

- 图片下载或飞书上传可能慢于文本事件；解析在 session 锁外执行，事件在同一 coalescer 内仍按批次顺序提交。
- 飞书上传限制为 10 MiB；GIF 分辨率不超过 2000×2000，其他格式不超过 12000×12000。
  服务端拒绝时走原位失败文案，不阻断任务。
- CardKit Markdown 使用 `image_key` 能保留图文顺序且无需新增组件；第一阶段不拆成独立 `img`
  组件，避免破坏现有元素预算和局部 content patch。
- 远程图片继续沿用现有 30 秒超时、10 MiB、Content-Type 和图片扩展名判断；不扩大为扫描
  工作目录或自动抓取普通链接。

## 飞书官方接口契约

- 图片上传：`POST /open-apis/im/v1/images`，`multipart/form-data` 必填
  `image_type=message` 和二进制 `image`，响应字段为 `data.image_key`。
- 权限：应用需启用机器人能力，并开通 `im:resource` 或 `im:resource:upload` 任一权限。
- 限制：图片必须非空且不超过 10 MiB；GIF 不超过 2000×2000，其他消息图片不超过
  12000×12000。格式支持 JPG/JPEG/PNG/WEBP/GIF/BMP/ICO/TIFF/HEIC。
- Card JSON 2.0 富文本图片语法：`![hover_text](image_key)`；普通 HTTP URL 不能替代
  上传后的 `image_key`。
- 官方文档：
  - <https://open.feishu.cn/document/uAjLw4CM/ukTMukTMukTM/reference/im-v1/image/create>
  - <https://open.feishu.cn/document/feishu-cards/card-json-v2-components/content-components/rich-text>
